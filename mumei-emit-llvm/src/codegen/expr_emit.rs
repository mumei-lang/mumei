use crate::codegen::lowering::{
    array_elem_llvm_type, array_struct_type, bitpreserve_cast, box_payload_to_i64,
    declare_chan_send_owned, ensure_type_acyclic, enum_llvm_type, payload_needs_box,
    release_boxed_payload, resolve_param_type, resolve_return_type, type_contains_array_enum,
    unbox_payload_from_i64, wide_int_cast, ArrayPtr, ArrayPtrMap,
};
use crate::codegen::pattern_emit::{
    bind_pattern_variables, compile_pattern_test, find_field_index, find_field_index_by_name,
};
use crate::codegen::stmt_emit::{
    compile_hir_stmt, deep_aggregate_field_slot, deep_slot_type_error,
};
use crate::codegen::task_runtime::{
    compile_task_spawn, declare_task_group_any_externs, emit_task_join_only, emit_task_join_raw,
    emit_task_spawn_only, static_next_task_group_id, PendingTask, TaskGroupAnyContext,
};
use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::{Linkage, Module};
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum};
use inkwell::values::{
    AnyValue, AsValueRef, BasicMetadataValueEnum, BasicValue, BasicValueEnum, FunctionValue,
};
use inkwell::AddressSpace;
use inkwell::{FloatPredicate, IntPredicate};
use mumei_core::hir::{
    collect_assigned_outer_variables_match_arm, collect_assigned_outer_variables_stmt, HirBinOp,
    HirExpr, HirJoin, HirPattern, HirStmt,
};
use mumei_core::verification::{ModuleEnv, MumeiError, MumeiResult};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

type ArrayMergeState<'a> = (
    HashMap<String, BasicValueEnum<'a>>,
    ArrayPtrMap<'a>,
    BasicBlock<'a>,
    HashSet<String>,
);

/// P25 — key under which a `chan<T>` binding records its declared payload
/// type inside `var_types`. `<` can never appear in a Mumei identifier, so
/// these entries never collide with a variable's struct type entry.
pub(crate) fn chan_payload_key(var: &str) -> String {
    format!("<chan>{}", var)
}

/// Marker recorded in `var_types[name]` when `let f = |…| …` or `f = |…|`
/// binds a lambda: `@lam:<lifted_fn>:<cap1,cap2,…>`. `@` never appears in a
/// Mumei identifier, so the marker cannot collide with a real type entry.
pub(crate) const LAMBDA_MARK: &str = "@lam:";

pub(crate) fn lambda_marker(fn_name: &str, captures: &[String]) -> String {
    format!("{}{}:{}", LAMBDA_MARK, fn_name, captures.join(","))
}

/// Parse a `@lam:` marker into (lifted fn name, capture names).
fn parse_lambda_marker(mark: &str) -> Option<(String, Vec<String>)> {
    let rest = mark.strip_prefix(LAMBDA_MARK)?;
    let (fn_name, caps) = rest.split_once(':')?;
    let caps = if caps.is_empty() {
        Vec::new()
    } else {
        caps.split(',').map(|s| s.to_string()).collect()
    };
    Some((fn_name.to_string(), caps))
}

/// Snapshot/merge helpers for branch-scoped lambda bindings: a lambda bound
/// on only one `if`/`else` side — or bound to different lifted functions —
/// must not stay callable after the merge, mirroring the verifier's
/// branch-scoped `local_lambdas` merge.
pub(crate) fn lambda_marks(var_types: &HashMap<String, String>) -> HashMap<String, String> {
    var_types
        .iter()
        .filter(|(_, v)| v.starts_with(LAMBDA_MARK))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Merge lambda markers after an `if`/`else` — mirrors the verifier's
/// `local_lambdas` merge: a binding on only one branch leaks through, while
/// a name bound on both survives only when both sides kept the *same*
/// lifted function (a conflicting rebind drops the entry so a post-branch
/// call fails closed instead of picking one side's body).
pub(crate) fn merge_lambda_marks(
    var_types: &mut HashMap<String, String>,
    then_marks: &HashMap<String, String>,
    else_marks: &HashMap<String, String>,
) {
    var_types.retain(|_, v| !v.starts_with(LAMBDA_MARK));
    for (name, mark) in then_marks {
        match else_marks.get(name) {
            Some(m) if m != mark => {}
            _ => {
                var_types.insert(name.clone(), mark.clone());
            }
        }
    }
    for (name, mark) in else_marks {
        if !then_marks.contains_key(name) {
            var_types.insert(name.clone(), mark.clone());
        }
    }
}

/// Bind a value that arrived as a lifted-function argument: array fat
/// pointers decompose into `array_ptrs` + the len scalar (mirroring how
/// `driver.rs` binds `[T]` atom params); struct/enum/lambda type entries
/// carry over so field access and nested indirect calls still resolve.
#[allow(clippy::too_many_arguments)]
fn bind_lambda_arg<'a>(
    context: &'a Context,
    builder: &Builder<'a>,
    name: &str,
    val: BasicValueEnum<'a>,
    declared_ty: Option<&str>,
    src_array: Option<ArrayPtr<'a>>,
    src_var_ty: Option<&String>,
    variables: &mut HashMap<String, BasicValueEnum<'a>>,
    var_types: &mut HashMap<String, String>,
    array_ptrs: &mut ArrayPtrMap<'a>,
    module_env: &ModuleEnv,
) -> MumeiResult<()> {
    let is_fat_array = src_array.is_some()
        || declared_ty.is_some_and(|t| {
            matches!(
                mumei_core::lowering::lower(&module_env.resolve_base_type(t)),
                mumei_core::lowering::LoweredType::Array(_)
            )
        });
    if is_fat_array {
        let Some(struct_val) = val.is_struct_value().then(|| val.into_struct_value()) else {
            return Err(MumeiError::codegen(format!(
                "lambda arg `{name}` must be an array fat pointer"
            )));
        };
        let len_val = builder
            .build_extract_value(struct_val, 0, &format!("{name}_len"))
            .map_err(|e| MumeiError::codegen(format!("lambda arg extract failed: {e:?}")))?;
        let data_ptr = builder
            .build_extract_value(struct_val, 1, &format!("{name}_data"))
            .map_err(|e| MumeiError::codegen(format!("lambda arg extract failed: {e:?}")))?;
        let declared_elem_ty = match declared_ty {
            Some(type_name) => {
                super::lowering::array_elem_llvm_type(context, type_name, module_env)?
            }
            None => None,
        };
        let elem_ty = src_array
            .map(|(_, e, _)| e)
            .or(declared_elem_ty)
            .unwrap_or_else(|| context.i64_type().into());
        array_ptrs.insert(name.to_string(), (len_val, elem_ty, data_ptr));
        // A `Str` binding's scalar value is the `*i8` byte pointer, not
        // the length (mirroring the `let s = "lit"` arm) — otherwise
        // `concat(s, …)` inside the lambda hands an i64 to a string use.
        let scalar = if elem_ty == context.i8_type().into() {
            data_ptr
        } else {
            len_val
        };
        variables.insert(name.to_string(), scalar);
        return Ok(());
    }
    variables.insert(name.to_string(), val);
    if let Some(t) = src_var_ty {
        var_types.insert(name.to_string(), t.clone());
    }
    Ok(())
}

/// Lift `|params| body` into a private module-level function whose leading
/// parameters are the lambda's captures — a later `f(args)` passes the
/// call-site values of the captured names, matching how the verifier
/// inlines the body under `local_lambdas`. Returns the lifted fn name.
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_lambda_function<'a>(
    context: &'a Context,
    module: &Module<'a>,
    caller_fn: &FunctionValue<'a>,
    var_name: &str,
    params: &[mumei_core::hir::HirLambdaParam],
    return_type: Option<&String>,
    captures: &[String],
    body: &HirStmt,
    outer_variables: &HashMap<String, BasicValueEnum<'a>>,
    outer_var_types: &HashMap<String, String>,
    outer_array_ptrs: &ArrayPtrMap<'a>,
    module_env: &ModuleEnv,
) -> MumeiResult<(String, Vec<String>)> {
    let base = format!(
        "__lam_{}_{}",
        caller_fn.get_name().to_string_lossy(),
        var_name
    );
    let mut fn_name = base.clone();
    let mut counter = 0u32;
    while module.get_function(&fn_name).is_some() {
        counter += 1;
        fn_name = format!("{base}_{counter}");
    }

    // Lambdas visible at the let site seed the body's `@lam:` scope (a call
    // to one inside the body resolves the lifted fn by name — no param
    // needed), but their *value* captures must ride along as extra params:
    // `let f = |x| x+k; let g = |y| f(y)` — `f`'s `k` is read from
    // `variables` inside `g`'s body, so `g` must take `k` too.
    let mut extra_caps: Vec<String> = Vec::new();
    let lam_marks = lambda_marks(outer_var_types);
    let mut seed: Vec<String> = lam_marks
        .values()
        .filter_map(|m| parse_lambda_marker(m).map(|(_, c)| c))
        .flatten()
        .collect();
    let mut seen: std::collections::HashSet<String> = seed.iter().cloned().collect();
    while let Some(cap) = seed.pop() {
        if let Some(m) = lam_marks.get(&cap) {
            // A captured lambda needs no value param (its marker resolves);
            // but its own captures contribute transitively.
            if let Some((_, sub_caps)) = parse_lambda_marker(m) {
                for c in sub_caps {
                    if seen.insert(c.clone()) {
                        seed.push(c);
                    }
                }
            }
            continue;
        }
        let in_scope = params.iter().any(|p| p.name == cap);
        if captures.contains(&cap) || extra_caps.contains(&cap) || in_scope {
            continue;
        }
        extra_caps.push(cap);
    }
    let all_caps: Vec<String> = captures.iter().cloned().chain(extra_caps).collect();

    // Signature: captures first (LLVM types from the binding site's values —
    // `let f = |x| x + k` needs no annotation on `k`), then declared params.
    let mut param_types: Vec<BasicMetadataTypeEnum> = Vec::new();
    for cap in &all_caps {
        if outer_array_ptrs.contains_key(cap.as_str()) {
            // Array captures travel as the `{i64, ptr}` fat pointer, not
            // the len scalar `variables[cap]` holds.
            param_types.push(array_struct_type(context).into());
            continue;
        }
        if lam_marks.contains_key(cap) {
            // Lambda captures pass the fn pointer (uniformity); the body's
            // marker resolves the call by name regardless.
            param_types.push(context.ptr_type(AddressSpace::default()).into());
            continue;
        }
        let Some(v) = outer_variables.get(cap.as_str()) else {
            return Err(MumeiError::codegen(format!(
                "lambda `{var_name}` captures `{cap}`, which is not bound at the let site"
            )));
        };
        param_types.push(v.get_type().into());
    }
    for p in params {
        param_types.push(
            resolve_param_type(
                context,
                p.type_ref.as_ref().map(|t| t.to_string()).as_deref(),
                module_env,
            )?
            .into(),
        );
    }
    let mut ret_ty = resolve_param_type(context, return_type.map(|s| s.as_str()), module_env)?;

    // An undeclared `|x: f64| x * 2.0` returns `double`, not `i64` — probe
    // the body's actual result type by compiling once, then rebuild with
    // the discovered return type. Nested lambdas lifted during the probe
    // stay in the module as dead private fns (re-emit gets a `_N` suffix
    // and the body's marker names the re-emitted one consistently).
    for attempt in 0..2 {
        let lam_fn = module.add_function(
            &fn_name,
            ret_ty.fn_type(&param_types, false),
            Some(inkwell::module::Linkage::Private),
        );

        // Compile the body on a separate builder so the caller's insertion
        // point is preserved; the body gets a fresh scope seeded with args.
        let lam_builder = context.create_builder();
        let entry = context.append_basic_block(lam_fn, "entry");
        lam_builder.position_at_end(entry);

        let mut variables: HashMap<String, BasicValueEnum> = HashMap::new();
        let mut var_types: HashMap<String, String> = HashMap::new();
        let mut array_ptrs = ArrayPtrMap::default();
        // Seed the body with the lambda scope live at the let site — a call
        // to an outer lambda inside the body resolves its `@lam:` marker.
        var_types.extend(lam_marks.iter().map(|(k, v)| (k.clone(), v.clone())));

        for (i, cap) in all_caps.iter().enumerate() {
            let val = lam_fn.get_nth_param(i as u32).unwrap();
            bind_lambda_arg(
                context,
                &lam_builder,
                cap,
                val,
                None,
                outer_array_ptrs.get(cap.as_str()).copied(),
                outer_var_types.get(cap.as_str()),
                &mut variables,
                &mut var_types,
                &mut array_ptrs,
                module_env,
            )?;
            if outer_array_ptrs.stack_backed.contains(cap) {
                array_ptrs.stack_backed.insert(cap.clone());
            }
        }
        for (i, p) in params.iter().enumerate() {
            let val = lam_fn.get_nth_param((all_caps.len() + i) as u32).unwrap();
            let ty = p.type_ref.as_ref().map(|t| t.to_string());
            let ty_hint = ty
                .as_deref()
                .map(|t| resolve_named_type(module_env, t))
                .filter(|base| {
                    module_env.get_struct(base).is_some() || module_env.get_enum(base).is_some()
                });
            bind_lambda_arg(
                context,
                &lam_builder,
                &p.name,
                val,
                ty.as_deref(),
                None,
                ty_hint.as_ref(),
                &mut variables,
                &mut var_types,
                &mut array_ptrs,
                module_env,
            )?;
            // `|f| f(1)` shadows an outer lambda `f` — the seeded marker
            // must not redirect calls meant for the parameter.
            if ty_hint.is_none() {
                var_types.remove(&p.name);
            }
        }

        let inferred_return = return_type
            .cloned()
            .or_else(|| infer_stmt_struct_type_name(body, &var_types, module_env));
        array_ptrs.return_contains_array_enum = inferred_return
            .as_deref()
            .is_some_and(|name| type_contains_array_enum(module_env, name));
        let result = compile_hir_stmt(
            context,
            &lam_builder,
            module,
            &lam_fn,
            body,
            &mut variables,
            &mut var_types,
            &mut array_ptrs,
            module_env,
        )?;

        let last_block = lam_builder.get_insert_block().unwrap();
        if last_block.get_terminator().is_none() {
            if result.get_type() != ret_ty {
                if return_type.is_none() && attempt == 0 {
                    ret_ty = result.get_type();
                    unsafe { lam_fn.delete() };
                    continue;
                }
                return Err(MumeiError::codegen(format!(
                    "lambda `{var_name}` returns {:?} but its signature expects {:?}",
                    result.get_type(),
                    ret_ty
                )));
            }
            lam_builder
                .build_return(Some(&result))
                .map_err(|e| MumeiError::codegen(format!("lambda return failed: {e:?}")))?;
        }
        return Ok((fn_name, all_caps));
    }
    unreachable!()
}

/// Push the leading capture arguments for a lifted-lambda call: each cap
/// name resolves to the current `variables` value, repacking array slots
/// into the `{i64, ptr}` fat pointer.
fn push_lambda_captures<'a>(
    context: &'a Context,
    builder: &Builder<'a>,
    caps: &[String],
    variables: &HashMap<String, BasicValueEnum<'a>>,
    array_ptrs: &ArrayPtrMap<'a>,
) -> MumeiResult<Vec<BasicMetadataValueEnum<'a>>> {
    let mut arg_vals: Vec<BasicMetadataValueEnum> = Vec::with_capacity(caps.len());
    for cap in caps {
        if let Some(&(len_val, _elem_ty, data_ptr)) = array_ptrs.get(cap.as_str()) {
            let struct_ty = array_struct_type(context);
            let mut agg: inkwell::values::AggregateValueEnum = struct_ty.get_undef().into();
            agg = builder
                .build_insert_value(agg, len_val, 0, "lamcap_len")
                .map_err(|e| MumeiError::codegen(format!("array capture pack failed: {e:?}")))?;
            agg = builder
                .build_insert_value(agg, data_ptr, 1, "lamcap_data")
                .map_err(|e| MumeiError::codegen(format!("array capture pack failed: {e:?}")))?;
            let agg_val: BasicValueEnum = match agg {
                inkwell::values::AggregateValueEnum::StructValue(s) => s.into(),
                inkwell::values::AggregateValueEnum::ArrayValue(a) => a.into(),
            };
            arg_vals.push(agg_val.into());
        } else if let Some(v) = variables.get(cap.as_str()) {
            arg_vals.push((*v).into());
        } else {
            return Err(MumeiError::codegen(format!(
                "lambda capture `{cap}` is not bound at the call site"
            )));
        }
    }
    Ok(arg_vals)
}

/// Collect the `let h = if c { f } else { g }` branch shape into
/// `(conditions, marker)` leaves: a leaf is a `Variable` carrying a `@lam:`
/// marker (or an inline `Lambda` literal, lifted on the spot), an
/// `IfThenElse` recurses with `cond` accumulated onto each leaf.
/// Returns None when any leaf isn't a lambda — the caller then falls back
/// to the generic expression compile and the name stays uncallable.
/// One conjunct of a lambda-selector leaf's taken-condition: an `if`
/// condition evaluated as truthy (`!= 0`), or a match arm's
/// `target == literal` equality. A leaf's conds are ANDed.
pub(crate) type SelScope<'a> = Rc<(HashMap<String, BasicValueEnum<'a>>, HashMap<String, String>)>;

#[derive(Clone)]
pub(crate) enum SelCond<'e, 'a> {
    Truthy(&'e HirExpr, Option<SelScope<'a>>),
    MatchEq(&'e HirExpr, i64, Option<SelScope<'a>>),
}

type SelBranch<'e, 'a> = (Vec<SelCond<'e, 'a>>, String, Option<SelScope<'a>>);

/// Lambda-only block prefix: every non-tail stmt must be `let v = |…| …`
/// or `let v = <lambda-bound var>`; anything else makes the selector
/// fall back to the generic expression path.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn block_lambda_scope<'a>(
    context: &'a Context,
    module: &Module<'a>,
    caller_fn: &FunctionValue<'a>,
    stmts: &[HirStmt],
    variables: &HashMap<String, BasicValueEnum<'a>>,
    var_types: &HashMap<String, String>,
    array_ptrs: &ArrayPtrMap<'a>,
    module_env: &ModuleEnv,
) -> MumeiResult<Option<(HashMap<String, BasicValueEnum<'a>>, HashMap<String, String>)>> {
    let mut local_variables = variables.clone();
    let mut local_var_types = var_types.clone();
    for stmt in stmts {
        match stmt {
            HirStmt::Let { var, value, .. } => match value.as_ref() {
                HirExpr::Lambda {
                    params,
                    return_type,
                    captures,
                    body,
                } => {
                    let (fn_name, all_caps) = emit_lambda_function(
                        context,
                        module,
                        caller_fn,
                        var,
                        params,
                        return_type.as_ref(),
                        captures,
                        body,
                        &local_variables,
                        &local_var_types,
                        array_ptrs,
                        module_env,
                    )?;
                    let lam_fn = module.get_function(&fn_name).unwrap();
                    let ptr: BasicValueEnum = lam_fn.as_global_value().as_pointer_value().into();
                    local_variables.insert(var.clone(), ptr);
                    local_var_types.insert(var.clone(), lambda_marker(&fn_name, &all_caps));
                }
                HirExpr::Variable(src)
                    if local_var_types
                        .get(src.as_str())
                        .is_some_and(|mark| mark.starts_with(LAMBDA_MARK)) =>
                {
                    let mark = local_var_types.get(src.as_str()).cloned().unwrap();
                    let ptr = local_variables
                        .get(src.as_str())
                        .copied()
                        .unwrap_or_else(|| context.i64_type().const_int(0, false).into());
                    local_variables.insert(var.clone(), ptr);
                    local_var_types.insert(var.clone(), mark);
                }
                _ => return Ok(None),
            },
            _ => return Ok(None),
        }
    }
    Ok(Some((local_variables, local_var_types)))
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn collect_lambda_branch_body<'e, 'a>(
    context: &'a Context,
    module: &Module<'a>,
    caller_fn: &FunctionValue<'a>,
    stmt: &'e HirStmt,
    conds: Vec<SelCond<'e, 'a>>,
    variables: &HashMap<String, BasicValueEnum<'a>>,
    var_types: &HashMap<String, String>,
    array_ptrs: &ArrayPtrMap<'a>,
    module_env: &ModuleEnv,
    require_simple_tail: bool,
    scope: Option<SelScope<'a>>,
) -> MumeiResult<Option<Vec<SelBranch<'e, 'a>>>> {
    match stmt {
        HirStmt::Block {
            stmts,
            tail_expr: Some(tail),
        } if !stmts.is_empty() => {
            if require_simple_tail
                && !matches!(tail.as_ref(), HirExpr::Variable(_) | HirExpr::Lambda { .. })
            {
                return Ok(None);
            }
            let Some((local_variables, local_var_types)) = block_lambda_scope(
                context, module, caller_fn, stmts, variables, var_types, array_ptrs, module_env,
            )?
            else {
                return Ok(None);
            };
            let scope = Some(Rc::new((local_variables.clone(), local_var_types.clone())));
            collect_lambda_branches(
                context,
                module,
                caller_fn,
                tail,
                conds,
                &local_variables,
                &local_var_types,
                array_ptrs,
                module_env,
                scope,
            )
        }
        _ => {
            let Some(tail) = hir_stmt_tail_expr(stmt) else {
                return Ok(None);
            };
            if require_simple_tail && !matches!(tail, HirExpr::Variable(_) | HirExpr::Lambda { .. })
            {
                return Ok(None);
            }
            collect_lambda_branches(
                context, module, caller_fn, tail, conds, variables, var_types, array_ptrs,
                module_env, scope,
            )
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) fn collect_lambda_branches<'e, 'a>(
    context: &'a Context,
    module: &Module<'a>,
    caller_fn: &FunctionValue<'a>,
    expr: &'e HirExpr,
    conds: Vec<SelCond<'e, 'a>>,
    variables: &HashMap<String, BasicValueEnum<'a>>,
    var_types: &HashMap<String, String>,
    array_ptrs: &ArrayPtrMap<'a>,
    module_env: &ModuleEnv,
    scope: Option<SelScope<'a>>,
) -> MumeiResult<Option<Vec<SelBranch<'e, 'a>>>> {
    // No depth cap: the verifier's `resolve_lambda_stmt` recurses without a
    // limit, so a cap here would make codegen reject programs verify accepts.
    match expr {
        HirExpr::Variable(name) => Ok(var_types
            .get(name.as_str())
            .filter(|m| m.starts_with(LAMBDA_MARK))
            .map(|mark| vec![(conds, mark.clone(), scope.clone())])),
        HirExpr::Lambda {
            params,
            return_type,
            captures,
            body,
        } => {
            // An inline `|…| …` leaf lifts like any other bound lambda; the
            // lifted fn's marker is what the branch entry records.
            let (fn_name, all_caps) = emit_lambda_function(
                context,
                module,
                caller_fn,
                "branch",
                params,
                return_type.as_ref(),
                captures,
                body,
                variables,
                var_types,
                array_ptrs,
                module_env,
            )?;
            Ok(Some(vec![(
                conds,
                lambda_marker(&fn_name, &all_caps),
                scope,
            )]))
        }
        HirExpr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            let mut then_conds = conds.clone();
            then_conds.push(SelCond::Truthy(cond, scope.clone()));
            let Some(mut then_branches) = collect_lambda_branch_body(
                context,
                module,
                caller_fn,
                then_branch,
                then_conds,
                variables,
                var_types,
                array_ptrs,
                module_env,
                false,
                scope.clone(),
            )?
            else {
                return Ok(None);
            };
            let Some(mut else_branches) = collect_lambda_branch_body(
                context,
                module,
                caller_fn,
                else_branch,
                conds,
                variables,
                var_types,
                array_ptrs,
                module_env,
                false,
                scope,
            )?
            else {
                return Ok(None);
            };
            then_branches.append(&mut else_branches);
            Ok(Some(then_branches))
        }
        // `let m = match t { 1 => f, _ => g }` — each arm tail is a
        // lambda leaf guarded by a `MatchEq`/`Always` condition, in
        // first-match order. Same scoping as the verifier: only
        // Literal/Wildcard/Variable patterns and no guards.
        HirExpr::Match { target, arms } => {
            if arms.is_empty() {
                return Ok(None);
            }
            let mut out: Vec<SelBranch<'e, 'a>> = Vec::new();
            for arm in arms {
                if arm.guard.is_some() {
                    return Ok(None);
                }
                let mut leaf_conds = conds.clone();
                match &arm.pattern {
                    HirPattern::Wildcard | HirPattern::Variable(_) => {}
                    HirPattern::Literal(n) => {
                        leaf_conds.push(SelCond::MatchEq(target.as_ref(), *n, scope.clone()));
                    }
                    _ => return Ok(None),
                }
                let Some(mut leaves) = collect_lambda_branch_body(
                    context,
                    module,
                    caller_fn,
                    &arm.body,
                    leaf_conds,
                    variables,
                    var_types,
                    array_ptrs,
                    module_env,
                    true,
                    scope.clone(),
                )?
                else {
                    return Ok(None);
                };
                out.append(&mut leaves);
            }
            Ok(Some(out))
        }
        _ => Ok(None),
    }
}

/// The tail expression of a lambda-shaped branch body — a bare `Expr` stmt
/// or the last stmt of a `Block`.
fn hir_stmt_tail_expr(stmt: &HirStmt) -> Option<&HirExpr> {
    match stmt {
        HirStmt::Expr(e) => Some(e),
        HirStmt::Block { tail_expr, .. } => tail_expr.as_ref().map(|e| e.as_ref()),
        _ => None,
    }
}

/// Emit `__lamsel_<caller>_<var>` — a dispatcher for
/// `let h = if c { f } else { g }`. Signature:
/// `(i64 sel, <union of every branch's captures>, <shared arg params>)`.
/// `sel` picks the branch: index 0..n-1 for conditional leaves, n-1 for the
/// trailing unconditional else leaf. The body's `sel`-indexed switch calls
/// the chosen lifted function with its own capture list.
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_lambda_selector<'a>(
    context: &'a Context,
    module: &Module<'a>,
    caller_fn: &FunctionValue<'a>,
    var_name: &str,
    branches: &[(String, Option<SelScope<'a>>)],
    variables: &HashMap<String, BasicValueEnum<'a>>,
    var_types: &HashMap<String, String>,
    array_ptrs: &ArrayPtrMap<'a>,
    module_env: &ModuleEnv,
) -> MumeiResult<Option<(String, Vec<String>)>> {
    // Resolve each branch's lifted fn and marker captures.
    let mut branch_fns: Vec<FunctionValue<'a>> = Vec::with_capacity(branches.len());
    let mut branch_caps: Vec<Vec<String>> = Vec::with_capacity(branches.len());
    let mut branch_scopes: Vec<Option<SelScope<'a>>> = Vec::with_capacity(branches.len());
    for (mark, scope) in branches {
        let Some((fn_name, caps)) = parse_lambda_marker(mark) else {
            return Ok(None);
        };
        let Some(lam_fn) = module.get_function(&fn_name) else {
            return Ok(None);
        };
        branch_fns.push(lam_fn);
        branch_caps.push(caps);
        branch_scopes.push(scope.clone());
    }

    // Arity + argument types + return type must agree across branches —
    // otherwise no single dispatcher signature exists (fail closed).
    let arg_arity = |i: usize| branch_fns[i].count_params() as usize - branch_caps[i].len();
    let arity = arg_arity(0);
    if branch_fns
        .iter()
        .enumerate()
        .any(|(i, _)| arg_arity(i) != arity)
    {
        return Ok(None);
    }
    let arg_types_of = |i: usize| -> Vec<BasicMetadataTypeEnum<'a>> {
        branch_fns[i]
            .get_type()
            .get_param_types()
            .into_iter()
            .skip(branch_caps[i].len())
            .map(|t| t.into())
            .collect()
    };
    let arg_types = arg_types_of(0);
    if (1..branches.len()).any(|i| arg_types_of(i) != arg_types) {
        return Ok(None);
    }
    let ret_ty = branch_fns[0].get_type().get_return_type();
    if branch_fns
        .iter()
        .any(|f| f.get_type().get_return_type() != ret_ty)
    {
        return Ok(None);
    }
    let Some(ret_ty) = ret_ty else {
        return Ok(None);
    };

    // Ordered union of all outer branch captures. Lambda captures that only
    // exist in a branch's block scope are supplied directly in that branch's
    // dispatch path instead of becoming dispatcher parameters.
    let mut union_caps: Vec<String> = Vec::new();
    let mut scope_lambda_caps: Vec<Vec<(String, BasicValueEnum<'a>)>> =
        vec![Vec::new(); branches.len()];
    for (i, caps) in branch_caps.iter().enumerate() {
        for c in caps {
            if array_ptrs.contains_key(c.as_str()) || variables.contains_key(c.as_str()) {
                if !union_caps.contains(c) {
                    union_caps.push(c.clone());
                }
                continue;
            }
            let Some(scope) = branch_scopes[i].as_ref() else {
                return Ok(None);
            };
            let Some(mark) = scope
                .1
                .get(c.as_str())
                .filter(|mark| mark.starts_with(LAMBDA_MARK))
            else {
                return Ok(None);
            };
            let Some(ptr) = scope.0.get(c.as_str()).copied() else {
                return Ok(None);
            };
            if !scope_lambda_caps[i].iter().any(|(name, _)| name == c) {
                scope_lambda_caps[i].push((c.clone(), ptr));
            }
            if let Some((_, transitive_caps)) = parse_lambda_marker(mark) {
                for transitive_cap in transitive_caps {
                    if (array_ptrs.contains_key(transitive_cap.as_str())
                        || variables.contains_key(transitive_cap.as_str()))
                        && !union_caps.contains(&transitive_cap)
                    {
                        union_caps.push(transitive_cap);
                    }
                }
            }
        }
    }

    // Dispatcher signature: (i64 sel, union caps, args).
    let mut param_types: Vec<BasicMetadataTypeEnum> = vec![context.i64_type().into()];
    for cap in &union_caps {
        if array_ptrs.contains_key(cap.as_str()) {
            param_types.push(array_struct_type(context).into());
        } else if let Some(v) = variables.get(cap.as_str()) {
            param_types.push(v.get_type().into());
        } else {
            return Ok(None);
        }
    }
    param_types.extend(arg_types.iter().cloned());

    let fn_name = format!(
        "__lamsel_{}_{}",
        caller_fn.get_name().to_string_lossy(),
        var_name
    );
    let fn_name = if module.get_function(&fn_name).is_some() {
        let mut n = 1;
        loop {
            let cand = format!("{fn_name}_{n}");
            if module.get_function(&cand).is_none() {
                break cand;
            }
            n += 1;
        }
    } else {
        fn_name
    };
    let lam_fn = module.add_function(
        &fn_name,
        ret_ty.fn_type(&param_types, false),
        Some(inkwell::module::Linkage::Private),
    );

    // Body scope: sel + union caps + args bound by position. The sel
    // binding name derives from the deduped dispatcher name so a rebinding
    // (`let m = …; let m = …`) mints a fresh slot — the old binding's sel
    // value must stay reachable for nested references like `_ => m`.
    let sel_name = format!("__sel#{var_name}#{fn_name}");
    let lam_builder = context.create_builder();
    let entry = context.append_basic_block(lam_fn, "entry");
    lam_builder.position_at_end(entry);
    let mut scope_vars: HashMap<String, BasicValueEnum> = HashMap::new();
    let mut scope_var_types: HashMap<String, String> = HashMap::new();
    let mut array_ptrs_local = ArrayPtrMap::default();
    let mut arg_vals: Vec<BasicValueEnum> = Vec::new();

    let sel_val = lam_fn.get_nth_param(0).unwrap().into_int_value();
    scope_vars.insert(sel_name.clone(), sel_val.into());
    for (i, cap) in union_caps.iter().enumerate() {
        let val = lam_fn.get_nth_param(1 + i as u32).unwrap();
        bind_lambda_arg(
            context,
            &lam_builder,
            cap,
            val,
            None,
            array_ptrs.get(cap.as_str()).copied(),
            var_types.get(cap.as_str()),
            &mut scope_vars,
            &mut scope_var_types,
            &mut array_ptrs_local,
            module_env,
        )?;
        if array_ptrs.stack_backed.contains(cap) {
            array_ptrs_local.stack_backed.insert(cap.clone());
        }
    }
    for i in 0..arity {
        arg_vals.push(
            lam_fn
                .get_nth_param((1 + union_caps.len() + i) as u32)
                .unwrap(),
        );
    }

    // Dispatch chain: `sel == i` → call branch i; the final leaf is the
    // unconditional else (its conds vec is empty).
    let n = branches.len();
    for i in 0..n {
        if i + 1 < n {
            // `sel == i` → call branch i, else fall through to the next check.
            let cond_i = lam_builder
                .build_int_compare(
                    inkwell::IntPredicate::EQ,
                    sel_val,
                    context.i64_type().const_int(i as u64, false),
                    "sel_eq",
                )
                .map_err(|e| MumeiError::codegen(format!("sel compare failed: {e:?}")))?;
            let call_block = context.append_basic_block(lam_fn, &format!("sel_call_{i}"));
            let next_block = context.append_basic_block(lam_fn, &format!("sel_next_{i}"));
            lam_builder
                .build_conditional_branch(cond_i, call_block, next_block)
                .map_err(|e| MumeiError::codegen(format!("sel branch failed: {e:?}")))?;
            lam_builder.position_at_end(call_block);
            let mut branch_scope_vars = scope_vars.clone();
            for (name, ptr) in &scope_lambda_caps[i] {
                branch_scope_vars.insert(name.clone(), *ptr);
            }
            let mut vals = push_lambda_captures(
                context,
                &lam_builder,
                &branch_caps[i],
                &branch_scope_vars,
                &array_ptrs_local,
            )?;
            vals.extend(
                arg_vals
                    .iter()
                    .map(|v| -> BasicMetadataValueEnum { (*v).into() }),
            );
            let call = lam_builder
                .build_call(branch_fns[i], &vals, "sel_call")
                .map_err(|e| MumeiError::codegen(format!("lambda dispatch call failed: {e:?}")))?;
            let result: BasicValueEnum = call
                .try_as_basic_value()
                .left()
                .unwrap_or_else(|| context.i64_type().const_int(0, false).into());
            lam_builder
                .build_return(Some(&result))
                .map_err(|e| MumeiError::codegen(format!("lambda dispatch ret failed: {e:?}")))?;
            lam_builder.position_at_end(next_block);
        } else {
            // The trailing else leaf is unconditional: emit its call inline
            // at the current position.
            let mut branch_scope_vars = scope_vars.clone();
            for (name, ptr) in &scope_lambda_caps[i] {
                branch_scope_vars.insert(name.clone(), *ptr);
            }
            let mut vals = push_lambda_captures(
                context,
                &lam_builder,
                &branch_caps[i],
                &branch_scope_vars,
                &array_ptrs_local,
            )?;
            vals.extend(
                arg_vals
                    .iter()
                    .map(|v| -> BasicMetadataValueEnum { (*v).into() }),
            );
            let call = lam_builder
                .build_call(branch_fns[i], &vals, "sel_call")
                .map_err(|e| MumeiError::codegen(format!("lambda dispatch call failed: {e:?}")))?;
            let result: BasicValueEnum = call
                .try_as_basic_value()
                .left()
                .unwrap_or_else(|| context.i64_type().const_int(0, false).into());
            lam_builder
                .build_return(Some(&result))
                .map_err(|e| MumeiError::codegen(format!("lambda dispatch ret failed: {e:?}")))?;
        }
    }
    let mut all_caps = vec![sel_name];
    for cap in union_caps {
        if !all_caps.contains(&cap) {
            all_caps.push(cap);
        }
    }
    Ok(Some((fn_name, all_caps)))
}

/// Emit a call through a `@lam:` marker: `f(args)` on a `let`/`assign`-bound
/// lambda resolves to the lifted private function; capture values are read
/// from `variables` at the call site and passed as leading arguments.
#[allow(clippy::too_many_arguments)]
fn emit_lambda_call<'a>(
    context: &'a Context,
    builder: &Builder<'a>,
    module: &Module<'a>,
    function: &FunctionValue<'a>,
    mark: &str,
    args: &[HirExpr],
    variables: &mut HashMap<String, BasicValueEnum<'a>>,
    var_types: &mut HashMap<String, String>,
    array_ptrs: &mut ArrayPtrMap<'a>,
    module_env: &ModuleEnv,
) -> MumeiResult<BasicValueEnum<'a>> {
    let Some((fn_name, caps)) = parse_lambda_marker(mark) else {
        return Err(MumeiError::codegen(format!(
            "malformed lambda binding marker: {mark}"
        )));
    };
    let Some(lam_fn) = module.get_function(&fn_name) else {
        return Err(MumeiError::codegen(format!(
            "lifted lambda function {fn_name} not found"
        )));
    };
    let expected = lam_fn.count_params() as usize;
    if args.len() + caps.len() != expected {
        return Err(MumeiError::codegen(format!(
            "lambda call arity mismatch: {} args + {} captures != {} params",
            args.len(),
            caps.len(),
            expected
        )));
    }
    let mut arg_vals: Vec<BasicMetadataValueEnum> =
        push_lambda_captures(context, builder, &caps, variables, array_ptrs)?;
    for arg in args {
        arg_vals.push(
            compile_hir_expr(
                context, builder, module, function, arg, variables, var_types, array_ptrs,
                module_env,
            )?
            .into(),
        );
    }
    let call_result = builder
        .build_call(lam_fn, &arg_vals, "lam_call")
        .map_err(|e| MumeiError::codegen(format!("lambda call failed: {e:?}")))?;
    Ok(call_result
        .try_as_basic_value()
        .left()
        .unwrap_or(context.i64_type().const_int(0, false).into()))
}

/// Numerically convert `val` to a channel's declared payload type before it is
/// bit-preserved into the runtime's i64 slot, so `send(ch, 1)` on a
/// `chan<f64>` transports `1.0` rather than the integer's bit pattern (which
/// `recv` would then reinterpret as a double). Int / float promotion follows
/// the same rule as mixed arithmetic; every other type pairing is left to
/// `bitpreserve_cast`.
fn coerce_to_chan_payload<'a>(
    builder: &Builder<'a>,
    val: BasicValueEnum<'a>,
    payload_ty: BasicTypeEnum<'a>,
) -> MumeiResult<BasicValueEnum<'a>> {
    match (payload_ty, val) {
        (BasicTypeEnum::FloatType(ty), BasicValueEnum::IntValue(v)) => {
            Ok(llvm!(builder.build_signed_int_to_float(v, ty, "payload_int_to_float")).into())
        }
        (BasicTypeEnum::IntType(ty), BasicValueEnum::FloatValue(v)) => {
            Ok(llvm!(builder.build_float_to_signed_int(v, ty, "payload_float_to_int")).into())
        }
        _ => Ok(val),
    }
}

/// Array payloads are fat pointers `(len, data)` into the owner's element
/// storage; a variable expression only materialises the length, so there is no
/// by-value array to box. Reject them rather than transport a length that
/// `recv` would dereference as a box address.
fn reject_array_chan_payload(payload_ty_name: &str, op: &str) -> MumeiResult<()> {
    if payload_ty_name.trim().starts_with('[') {
        return Err(MumeiError::codegen(format!(
            "channel {op} of an array payload is not supported: arrays are fat pointers into \
             the owner's storage and have no by-value channel encoding"
        )));
    }
    Ok(())
}

/// Element-wise `i1` equality of one enum payload leaf: integers and
/// floats compare directly, `Str` pointer slots defer to `mumei_str_eq`,
/// and an aggregate leaf — a nested enum or struct payload
/// (`pure-i64-composite/v23`) — recurses field by field. Undef fields
/// (unit-variant padding) carry no information and are skipped.
fn enum_leaf_eq<'a>(
    builder: &Builder<'a>,
    context: &'a Context,
    module: &Module<'a>,
    lf: BasicValueEnum<'a>,
    rf: BasicValueEnum<'a>,
) -> MumeiResult<inkwell::values::IntValue<'a>> {
    fn is_undef(v: BasicValueEnum) -> bool {
        match v {
            BasicValueEnum::IntValue(v) => v.is_undef(),
            BasicValueEnum::FloatValue(v) => v.is_undef(),
            BasicValueEnum::PointerValue(v) => v.is_undef(),
            BasicValueEnum::StructValue(v) => v.is_undef(),
            BasicValueEnum::ArrayValue(v) => v.is_undef(),
            BasicValueEnum::VectorValue(v) => v.is_undef(),
        }
    }
    match lf {
        BasicValueEnum::IntValue(l) => Ok(llvm!(builder.build_int_compare(
            IntPredicate::EQ,
            l,
            rf.into_int_value(),
            "enum_eq_int"
        ))),
        BasicValueEnum::FloatValue(l) => Ok(llvm!(builder.build_float_compare(
            FloatPredicate::OEQ,
            l,
            rf.into_float_value(),
            "enum_eq_float"
        ))),
        // `Str` payloads are pointers — compare contents via the runtime
        // helper.
        BasicValueEnum::PointerValue(_) => {
            let str_eq_fn = module.get_function("mumei_str_eq").unwrap_or_else(|| {
                let ptr_t = context.ptr_type(inkwell::AddressSpace::default());
                let fn_type = context
                    .i64_type()
                    .fn_type(&[ptr_t.into(), ptr_t.into()], false);
                module.add_function("mumei_str_eq", fn_type, None)
            });
            let res = llvm!(builder.build_call(str_eq_fn, &[lf.into(), rf.into()], "enum_eq_str"));
            let res_int = res
                .try_as_basic_value()
                .left()
                .ok_or(MumeiError::codegen("str_eq returned void".to_string()))?
                .into_int_value();
            Ok(llvm!(builder.build_int_compare(
                IntPredicate::NE,
                res_int,
                context.i64_type().const_int(0, false),
                "enum_eq_str_bool"
            )))
        }
        BasicValueEnum::StructValue(ls) => {
            let rs = rf.into_struct_value();
            let mut acc = context.bool_type().const_int(1, false);
            for idx in 0..ls.get_type().count_fields() {
                let le = llvm!(builder.build_extract_value(ls, idx, "enum_eq_l"));
                let re = llvm!(builder.build_extract_value(rs, idx, "enum_eq_r"));
                if is_undef(le) || is_undef(re) {
                    continue;
                }
                acc = llvm!(builder.build_and(
                    acc,
                    enum_leaf_eq(builder, context, module, le, re)?,
                    "enum_eq_and"
                ));
            }
            Ok(acc)
        }
        BasicValueEnum::ArrayValue(la) => {
            let ra = rf.into_array_value();
            let mut acc = context.bool_type().const_int(1, false);
            for idx in 0..la.get_type().len() {
                let le = llvm!(builder.build_extract_value(la, idx, "enum_eq_l"));
                let re = llvm!(builder.build_extract_value(ra, idx, "enum_eq_r"));
                if is_undef(le) || is_undef(re) {
                    continue;
                }
                acc = llvm!(builder.build_and(
                    acc,
                    enum_leaf_eq(builder, context, module, le, re)?,
                    "enum_eq_and"
                ));
            }
            Ok(acc)
        }
        BasicValueEnum::VectorValue(_) => Err(MumeiError::codegen(
            "enum payload leaf equality of a vector value is not supported".to_string(),
        )),
    }
}

/// Declared payload type of the channel `expr` denotes, when the channel is a
/// binding introduced with a `chan<T>` type.
pub(crate) fn chan_payload_type_name(
    expr: &HirExpr,
    var_types: &HashMap<String, String>,
) -> Option<String> {
    match expr {
        HirExpr::Variable(name) => var_types.get(&chan_payload_key(name)).cloned(),
        _ => None,
    }
}

/// Resolve a declared type name to its base name with generics stripped —
/// `get_enum`/`get_struct` are keyed on the unparameterized name, so
/// `Option<i64>` resolves to `Option`.
/// A resolved base name that denotes a fixed-width machine integer
/// (`i8`..`i64`, `u8`..`u64`) — the names `hir_int_type_name` may legitimately
/// return for a scalar binding.
pub(crate) fn is_fixed_int_type_name(base: &str) -> bool {
    matches!(
        base,
        "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "i128" | "u128"
    )
}

/// Resolved integer type of a `HirExpr` operand — `Some("u64")` marks a
/// `u64` operand, `Some("i64")` a declared signed one, `None` an adaptive
/// literal or unresolvable expr. Mirrors the verifier's
/// `fixed_width_int_type` so codegen picks `udiv`/`urem`/`lshr`/`icmp u*`
/// for the same operands the verifier encoded with unsigned BV ops.
pub(crate) fn hir_int_type_name(
    expr: &HirExpr,
    var_types: &HashMap<String, String>,
    module_env: &ModuleEnv,
) -> Option<String> {
    match expr {
        HirExpr::Variable(name) => var_types
            .get(name)
            .map(|t| resolve_named_type(module_env, t)),
        HirExpr::Call { name, .. } => module_env
            .get_atom(name)
            .and_then(|atom| atom.return_type.clone())
            .map(|t| resolve_named_type(module_env, &t)),
        // Only integer-producing operators propagate a leaf's signedness —
        // a comparison or logical op yields `bool`, so `(a > 0) == (s.n > 0)`
        // must not read `u64`/`i64` from its operands and trip the mix check.
        HirExpr::BinaryOp(
            l,
            HirBinOp::Add
            | HirBinOp::Sub
            | HirBinOp::Mul
            | HirBinOp::Pow
            | HirBinOp::Div
            | HirBinOp::Rem
            | HirBinOp::BitAnd
            | HirBinOp::BitOr
            | HirBinOp::BitXor
            | HirBinOp::Shl
            | HirBinOp::Shr,
            r,
        ) => hir_int_type_name(l, var_types, module_env)
            .or_else(|| hir_int_type_name(r, var_types, module_env)),
        HirExpr::IfThenElse {
            then_branch,
            else_branch,
            ..
        } => stmt_int_type_name(then_branch, var_types, module_env)
            .or_else(|| stmt_int_type_name(else_branch, var_types, module_env)),
        HirExpr::FieldAccess(inner, field) => infer_struct_type_name(inner, var_types, module_env)
            .and_then(|s| {
                module_env
                    .get_struct(&s)
                    .and_then(|sdef| sdef.fields.iter().find(|f| f.name == *field))
                    .map(|f| resolve_named_type(module_env, &f.type_name))
            }),
        _ => None,
    }
}

fn emit_enum_payload_field_read<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    enum_value: inkwell::values::StructValue<'ctx>,
    enum_type: &str,
    field_name: &str,
    module_env: &ModuleEnv,
) -> MumeiResult<BasicValueEnum<'ctx>> {
    let Some((index, declared_type, enum_payload_slot)) =
        deep_aggregate_field_slot(module_env, enum_type, field_name, field_name)?
    else {
        return Err(MumeiError::codegen(format!(
            "enum '{enum_type}' has no named payload '{field_name}'"
        )));
    };
    if !enum_payload_slot {
        return Err(MumeiError::codegen(format!(
            "enum '{enum_type}' field '{field_name}' is not a named payload"
        )));
    }
    let expected_type = resolve_param_type(context, Some(&declared_type), module_env)?;
    let slot_type = enum_value
        .get_type()
        .get_field_type_at_index(index)
        .ok_or_else(|| {
            MumeiError::codegen(format!(
                "enum '{enum_type}' has no payload slot for '{field_name}'"
            ))
        })?;
    if slot_type != expected_type {
        return Err(deep_slot_type_error(
            module_env,
            enum_type,
            &declared_type,
            index,
            enum_payload_slot,
        ));
    }
    let extracted = llvm!(builder.build_extract_value(
        enum_value,
        index,
        &format!("enum_payload.{field_name}")
    ));
    if extracted.get_type() != expected_type {
        return Err(deep_slot_type_error(
            module_env,
            enum_type,
            &declared_type,
            index,
            enum_payload_slot,
        ));
    }
    Ok(extracted)
}

/// The declared int type name of a statement's produced value —
/// `HirStmt::Expr` delegates to `hir_int_type_name`, a `Block`'s
/// `tail_expr` is its produced value; any other shape has no scalar
/// name (`pure-i64-composite/v22` merge widening).
fn stmt_int_type_name(
    stmt: &HirStmt,
    var_types: &HashMap<String, String>,
    module_env: &ModuleEnv,
) -> Option<String> {
    match stmt {
        HirStmt::Expr(expr) => hir_int_type_name(expr, var_types, module_env),
        HirStmt::Block { stmts, tail_expr } => {
            let mut scoped_types = var_types.clone();
            for stmt in stmts {
                if let HirStmt::Let { var, value, .. } = stmt {
                    match hir_int_type_name(value, &scoped_types, module_env)
                        .or_else(|| infer_struct_type_name(value, &scoped_types, module_env))
                    {
                        Some(ty) => {
                            scoped_types.insert(var.clone(), ty);
                        }
                        None => {
                            scoped_types.remove(var);
                        }
                    }
                }
            }
            tail_expr
                .as_deref()
                .and_then(|expr| hir_int_type_name(expr, &scoped_types, module_env))
        }
        _ => None,
    }
}

/// The phi type and widened incomings for a branch merge whose arms
/// disagree on integer width: the verifier's `Num` adaptation lets a
/// literal arm join a declared-width arm (`pure-i64-composite/v22`), so
/// a narrower LLVM incoming extends to the widest int type present —
/// sign-extending unless a wide arm's declared type is unsigned
/// (`u64`/`u128`). Non-int merges keep the first arm's type so
/// aggregate/pointer phis behave exactly as before.
fn widen_merge_incomings<'a>(
    context: &'a Context,
    builder: &Builder<'a>,
    incomings: &[(BasicValueEnum<'a>, BasicBlock<'a>)],
    declared_names: &[Option<String>],
) -> mumei_core::verification::MumeiResult<(
    inkwell::types::BasicTypeEnum<'a>,
    Vec<(BasicValueEnum<'a>, BasicBlock<'a>)>,
)> {
    let first_type = incomings
        .first()
        .map(|(v, _)| v.get_type())
        .unwrap_or_else(|| context.i64_type().into());
    if !incomings.iter().all(|(v, _)| v.get_type().is_int_type()) {
        return Ok((first_type, incomings.to_vec()));
    }
    let width = incomings
        .iter()
        .map(|(v, _)| v.get_type().into_int_type().get_bit_width())
        .max()
        .unwrap_or(64);
    let phi_type = context.custom_width_int_type(width).into();
    // The merge's signedness comes from the arms whose LLVM type is
    // already the widest — an unresolved name defaults to signed, which
    // matches `i64`/`i128` adaptation; unsigned arms resolve their
    // declared `u64`/`u128` name through `hir_int_type_name`.
    let signed = incomings
        .iter()
        .zip(declared_names.iter().chain(std::iter::repeat(&None)))
        .filter(|((v, _), _)| v.get_type() == phi_type)
        .all(|(_, name)| !matches!(name.as_deref(), Some("u64" | "u128")));
    let mut widened = Vec::with_capacity(incomings.len());
    for (v, block) in incomings {
        let v = if v.get_type() == phi_type {
            *v
        } else {
            wide_int_cast(builder, *v, phi_type, signed)?
        };
        widened.push((v, *block));
    }
    Ok((phi_type, widened))
}

/// `u64` operand semantics for an int binop: `Some(true)` unsigned,
/// `Some(false)` both signed or adaptive, `Err` on a declared u64/non-u64 mix.
/// Integer-type name for an unsigned-semantics probe: a `Str` index
/// read yields a `u64` byte leaf (`pure-i64-composite/v13`) — the i8
/// element width marks the fat pointer, since `var_types` carries no
/// marker for `let s = "lit"` bindings. Anything else delegates to
/// `hir_int_type_name`.
fn operand_int_type_name<'a>(
    expr: &HirExpr,
    var_types: &HashMap<String, String>,
    array_ptrs: &HashMap<String, ArrayPtr<'a>>,
    module_env: &ModuleEnv,
) -> Option<String> {
    if let HirExpr::ArrayAccess(name, _) = expr {
        if let Some(&(_, elem_ty, _)) = array_ptrs.get(name.as_str()) {
            if elem_ty.is_int_type() && elem_ty.into_int_type().get_bit_width() == 8 {
                return Some("u64".to_string());
            }
        }
    }
    hir_int_type_name(expr, var_types, module_env)
}

fn binop_unsigned_operands<'a>(
    left: &HirExpr,
    right: &HirExpr,
    var_types: &HashMap<String, String>,
    array_ptrs: &HashMap<String, ArrayPtr<'a>>,
    module_env: &ModuleEnv,
) -> Result<bool, MumeiError> {
    let lt = operand_int_type_name(left, var_types, array_ptrs, module_env);
    let rt = operand_int_type_name(right, var_types, array_ptrs, module_env);
    let unsigned_name = |t: Option<&str>| matches!(t, Some("u64") | Some("u128"));
    let (lu, ru) = (unsigned_name(lt.as_deref()), unsigned_name(rt.as_deref()));
    if (lu && rt.is_some() && !ru) || (ru && lt.is_some() && !lu) {
        return Err(MumeiError::codegen(
            "Cannot mix a u64 operand with a declared non-u64 integer operand".to_string(),
        ));
    }
    Ok(lu || ru)
}

pub(crate) fn resolve_named_type(module_env: &ModuleEnv, type_name: &str) -> String {
    let base = module_env.resolve_base_type(type_name);
    base.split('<')
        .next()
        .unwrap_or(base.as_str())
        .trim()
        .to_string()
}

pub(crate) fn infer_struct_type_name(
    expr: &HirExpr,
    var_types: &HashMap<String, String>,
    module_env: &ModuleEnv,
) -> Option<String> {
    match expr {
        HirExpr::Variable(name) => var_types.get(name).cloned(),
        HirExpr::ArrayAccess(name, _) => var_types.get(name).and_then(|array_type| {
            let resolved = module_env.resolve_base_type(array_type);
            let element = resolved
                .strip_prefix("[]<")
                .and_then(|inner| inner.strip_suffix('>'))
                .or_else(|| {
                    resolved
                        .strip_prefix('[')
                        .and_then(|inner| inner.strip_suffix(']'))
                })?;
            let element = resolve_named_type(module_env, element.trim());
            if module_env.get_struct(&element).is_some() || module_env.get_enum(&element).is_some()
            {
                Some(element)
            } else {
                None
            }
        }),
        HirExpr::StructInit { type_name, .. } => {
            let base = resolve_named_type(module_env, type_name);
            if module_env.get_struct(&base).is_some() {
                Some(base)
            } else {
                None
            }
        }
        HirExpr::Call { name, .. } => {
            let ret_type = module_env.get_atom(name)?.return_type.as_ref()?;
            let base = resolve_named_type(module_env, ret_type);
            if module_env.get_struct(&base).is_some() || module_env.get_enum(&base).is_some() {
                Some(base)
            } else {
                None
            }
        }
        // `let s = E::V(..)` records `E` so a later `match s` resolves the
        // owning enum deterministically (callers only consume the name as a
        // declared-type hint; struct lookups still `get_struct`-check it).
        HirExpr::VariantInit { enum_name, .. } => {
            module_env.get_enum(enum_name).map(|_| enum_name.clone())
        }
        HirExpr::FieldAccess(inner, field) => {
            // `E::V` / `E.V` unit-variant construction (a FieldAccess whose
            // qualifier is a known enum declaring `field`, not a bound
            // variable) types as the enum — `let m = Mine::Nil` records
            // `m: Mine` so a later `match m` resolves the owner.
            if let HirExpr::Variable(qual) = inner.as_ref() {
                if !var_types.contains_key(qual.as_str()) {
                    if let Some(enum_def) = module_env.get_enum(qual) {
                        if enum_def.variants.iter().any(|v| v.name == *field) {
                            return Some(enum_def.name.clone());
                        }
                    }
                }
            }
            let inner_ty = infer_struct_type_name(inner, var_types, module_env)?;
            let parent_ty = module_env.resolve_base_type(&inner_ty);
            let declared_type = if let Some(sdef) = module_env.get_struct(&parent_ty) {
                sdef.fields
                    .iter()
                    .find(|f| f.name == *field)?
                    .type_name
                    .clone()
            } else if module_env.get_enum(&parent_ty).is_some() {
                let (_, declared_type, enum_payload_slot) =
                    deep_aggregate_field_slot(module_env, &parent_ty, field, field).ok()??;
                if !enum_payload_slot {
                    return None;
                }
                declared_type
            } else {
                return None;
            };
            let base = module_env.resolve_base_type(&declared_type);
            if module_env.get_struct(&base).is_some() || module_env.get_enum(&base).is_some() {
                Some(base)
            } else {
                None
            }
        }
        // P25: `recv` on a `chan<Struct>` yields the declared payload type.
        HirExpr::ChanRecv { channel } => {
            let payload = chan_payload_type_name(channel, var_types)?;
            let base = module_env.resolve_base_type(&payload);
            if module_env.get_struct(&base).is_some() {
                Some(base)
            } else {
                None
            }
        }
        // `if c { E::V(..) } else { E::W }` types as `E` when both branches
        // agree — `let e = if …; match e` resolves the owner like a
        // declared parameter type.
        HirExpr::IfThenElse {
            then_branch,
            else_branch,
            ..
        } => {
            let t = infer_stmt_struct_type_name(then_branch, var_types, module_env);
            let e = infer_stmt_struct_type_name(else_branch, var_types, module_env);
            match (t, e) {
                (Some(t), Some(e)) if t == e => Some(t),
                _ => None,
            }
        }
        // `match e { E::V(..) => .., _ => .. }` producing enum values
        // in every arm types as that enum — the same join the
        // if/else rule above gives, needed so enum equality on a
        // match-produced operand resolves its declared type.
        HirExpr::Match { arms, .. } => {
            let mut names = arms
                .iter()
                .map(|arm| infer_stmt_struct_type_name(&arm.body, var_types, module_env));
            match names.next() {
                Some(Some(first)) if names.all(|n| n == Some(first.clone())) => Some(first),
                _ => None,
            }
        }
        // P25: a joined `task` carries its body's tail value.
        HirExpr::Task { body, .. } => infer_stmt_struct_type_name(body, var_types, module_env),
        // P25: a joined `task_group` carries a child's tail value (all children
        // share one result type, so the first that resolves decides).
        HirExpr::TaskGroup { children, .. } => children
            .iter()
            .find_map(|child| infer_stmt_struct_type_name(child, var_types, module_env)),
        _ => None,
    }
}

pub(crate) fn infer_stmt_struct_type_name(
    stmt: &HirStmt,
    var_types: &HashMap<String, String>,
    module_env: &ModuleEnv,
) -> Option<String> {
    match stmt {
        HirStmt::Expr(e) => infer_struct_type_name(e, var_types, module_env),
        HirStmt::Block { tail_expr, .. } => {
            infer_struct_type_name(tail_expr.as_deref()?, var_types, module_env)
        }
        _ => None,
    }
}

/// Materialise an array literal `[e0, e1, …]`: alloca the backing array,
/// store each element, and return `(len, elem_ty, data_ptr)` — the same
/// fat-pointer triple `array_ptrs` carries for `[T]` parameters.
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_array_literal<'a>(
    context: &'a Context,
    builder: &Builder<'a>,
    module: &Module<'a>,
    function: &FunctionValue<'a>,
    elements: &[HirExpr],
    variables: &mut HashMap<String, BasicValueEnum<'a>>,
    var_types: &mut HashMap<String, String>,
    array_ptrs: &mut ArrayPtrMap<'a>,
    module_env: &ModuleEnv,
) -> MumeiResult<(BasicValueEnum<'a>, BasicTypeEnum<'a>, BasicValueEnum<'a>)> {
    let mut elem_vals = Vec::with_capacity(elements.len());
    for element in elements {
        elem_vals.push(compile_hir_expr(
            context, builder, module, function, element, variables, var_types, array_ptrs,
            module_env,
        )?);
    }
    let elem_ty = elem_vals[0].get_type();
    let enum_names = elements
        .iter()
        .map(|element| {
            infer_struct_type_name(element, var_types, module_env)
                .filter(|type_name| module_env.get_enum(type_name).is_some())
        })
        .collect::<Vec<_>>();
    if enum_names.iter().any(Option::is_some) {
        let Some(enum_name) = enum_names.first().and_then(Option::as_deref) else {
            return Err(MumeiError::codegen(
                "array literal mixes enum and non-enum elements",
            ));
        };
        if enum_names
            .iter()
            .any(|candidate| candidate.as_deref() != Some(enum_name))
        {
            return Err(MumeiError::codegen(
                "array literal enum elements must have the same datatype",
            ));
        }
        let expected =
            super::lowering::array_elem_llvm_type(context, &format!("[{enum_name}]"), module_env)?
                .ok_or_else(|| MumeiError::codegen("enum array element type did not resolve"))?;
        if elem_ty != expected {
            return Err(MumeiError::codegen(format!(
                "enum array literal element type mismatch for '{enum_name}'"
            )));
        }
    }
    let struct_names = elements
        .iter()
        .map(|element| {
            infer_struct_type_name(element, var_types, module_env)
                .filter(|type_name| module_env.get_struct(type_name).is_some())
        })
        .collect::<Vec<_>>();
    if struct_names.iter().any(Option::is_some) {
        let Some(struct_name) = struct_names.first().and_then(Option::as_deref) else {
            return Err(MumeiError::codegen(
                "array literal mixes struct and non-struct elements",
            ));
        };
        if struct_names
            .iter()
            .any(|candidate| candidate.as_deref() != Some(struct_name))
        {
            return Err(MumeiError::codegen(
                "array literal struct elements must have the same type",
            ));
        }
        let expected = super::lowering::array_elem_llvm_type(
            context,
            &format!("[{struct_name}]"),
            module_env,
        )?
        .ok_or_else(|| MumeiError::codegen("struct array element type did not resolve"))?;
        if elem_ty != expected {
            return Err(MumeiError::codegen(format!(
                "struct array literal element type mismatch for '{struct_name}'"
            )));
        }
    }
    let n = elements.len() as u32;
    let array_ty = elem_ty.array_type(n);
    // Every `alloca` goes in the entry block — the canonical LLVM form
    // (mem2reg only promotes entry allocas, and the bounded-memory
    // validator's whitelist only admits them there). A literal in a
    // branch or loop body would otherwise emit a non-entry alloca.
    let data_ptr = {
        let placement = builder
            .get_insert_block()
            .expect("array literal emission has an insertion block");
        let entry = function
            .get_first_basic_block()
            .expect("a function being emitted has an entry block");
        if let Some(first) = entry.get_first_instruction() {
            builder.position_before(&first);
        } else {
            builder.position_at_end(entry);
        }
        let data_ptr = llvm!(builder.build_alloca(array_ty, "arrlit_data"));
        builder.position_at_end(placement);
        data_ptr
    };
    for (i, elem) in elem_vals.iter().enumerate() {
        // Int elements in an f64 literal widen via sitofp — matching the
        // verifier's int→real coercion for array literals.
        let stored = if elem_ty.is_float_type() && elem.is_int_value() {
            llvm!(builder.build_signed_int_to_float(
                elem.into_int_value(),
                context.f64_type(),
                "lit_widen"
            ))
            .into()
        } else if elem.get_type() == elem_ty {
            *elem
        } else {
            return Err(MumeiError::codegen(format!(
                "array literal elements must share a type (element {i} differs)"
            )));
        };
        let slot = unsafe {
            llvm!(builder.build_gep(
                array_ty,
                data_ptr,
                &[
                    context.i64_type().const_int(0, false),
                    context.i64_type().const_int(i as u64, false)
                ],
                "lit_slot"
            ))
        };
        llvm!(builder.build_store(slot, stored));
    }
    let len_val: BasicValueEnum = context.i64_type().const_int(n as u64, false).into();
    Ok((len_val, elem_ty, data_ptr.into()))
}

/// Copies `len` elements of `elem_ty` from `data_ptr` into freshly `malloc`ed
/// storage and returns the heap pointer. Used when a `-> [T]` atom's tail is
/// callee-local (an array literal or a stack-bound alias): the `alloca` it
/// points into dies with the frame, so the returned fat pointer must own a
/// heap copy instead. Bindings copy for the same reason in reverse — the
/// verifier treats each `let`/`=` binding as an independent array value, so
/// a `t[i] = v` write must never reach the callee's (or an aliased
/// parameter's) buffer. Ownership convention: the caller owns the copied
/// buffer (there is no `free` on this path yet — the bounded-memory model
/// does not track it).
pub(crate) fn copy_array_to_heap<'a>(
    context: &'a Context,
    builder: &Builder<'a>,
    module: &Module<'a>,
    len_val: BasicValueEnum<'a>,
    elem_ty: BasicTypeEnum<'a>,
    data_ptr: BasicValueEnum<'a>,
) -> MumeiResult<BasicValueEnum<'a>> {
    let malloc_fn = module.get_function("malloc").unwrap_or_else(|| {
        let ptr_type = context.ptr_type(inkwell::AddressSpace::default());
        let fn_type = ptr_type.fn_type(&[context.i64_type().into()], false);
        module.add_function("malloc", fn_type, Some(Linkage::External))
    });
    let elem_size = elem_ty
        .size_of()
        .ok_or_else(|| MumeiError::codegen("array element type is unsized"))?;
    let byte_size =
        llvm!(builder.build_int_mul(len_val.into_int_value(), elem_size, "ret_arr_bytes"));
    // A `std/libc` import can declare `malloc` with an integer return; the
    // real symbol still hands back a pointer, so bitcast the call result
    // instead of assuming the declaration's shape.
    let call_result = llvm!(builder.build_call(malloc_fn, &[byte_size.into()], "ret_arr_malloc"))
        .try_as_basic_value()
        .left()
        .ok_or_else(|| MumeiError::codegen("malloc returned no value"))?;
    let heap_ptr = match call_result {
        BasicValueEnum::PointerValue(p) => p,
        BasicValueEnum::IntValue(i) => llvm!(builder.build_int_to_ptr(
            i,
            context.ptr_type(inkwell::AddressSpace::default()),
            "ret_arr_malloc_ptr",
        )),
        _ => {
            return Err(MumeiError::codegen(
                "malloc returned a non-pointer, non-integer value",
            ))
        }
    };
    llvm!(builder.build_memcpy(heap_ptr, 8, data_ptr.into_pointer_value(), 8, byte_size,));
    Ok(heap_ptr.into())
}

/// `(len, data)` slots for a branch whose tail value is an array: a
/// `Variable` tail resolving through the branch's `array_ptrs` entry, or a
/// fat `{i64, ptr}` aggregate a call/`if`/`match`/`ArrayLit` tail produced.
/// String bindings also register in `array_ptrs`, but they store their
/// `*i8` in `variables` rather than the i64 length arrays carry, so they
/// return `None` and keep merging through the scalar path as pointers.
/// Extracts must be emitted inside `end_block` (before its terminator) —
/// phi incomings have to be defined in (or dominate) the predecessor they
/// attach to.
#[allow(clippy::too_many_arguments)]
fn array_result_slots<'a>(
    context: &'a Context,
    builder: &Builder<'a>,
    branch: &HirStmt,
    val: BasicValueEnum<'a>,
    arrays: &ArrayPtrMap<'a>,
    vars: &HashMap<String, BasicValueEnum<'a>>,
    end_block: BasicBlock<'a>,
) -> MumeiResult<Option<(BasicValueEnum<'a>, BasicValueEnum<'a>)>> {
    if let Some(name) = hir_stmt_tail_expr(branch).and_then(|e| {
        if let HirExpr::Variable(name) = e {
            Some(name.as_str())
        } else {
            None
        }
    }) {
        if let Some(&(len, _, data)) = arrays.get(name) {
            let string_binding = vars.get(name).is_some_and(|v| v.is_pointer_value());
            if !string_binding {
                return Ok(Some((len, data)));
            }
        }
    }
    if let BasicValueEnum::StructValue(fat) = val {
        if fat.get_type() == array_struct_type(context) {
            let saved = builder.get_insert_block();
            if let Some(term) = end_block.get_terminator() {
                builder.position_before(&term);
            } else {
                builder.position_at_end(end_block);
            }
            let len = llvm!(builder.build_extract_value(fat, 0, "arr_branch_len"));
            let data = llvm!(builder.build_extract_value(fat, 1, "arr_branch_data"));
            if let Some(saved) = saved {
                builder.position_at_end(saved);
            }
            return Ok(Some((len, data)));
        }
    }
    Ok(None)
}

/// Phi-merge array bindings that branches reassigned, mirroring the scalar
/// variable merge while keeping the fat-pointer slots in sync.
#[allow(clippy::type_complexity)]
fn merge_array_ptrs<'a>(
    builder: &Builder<'a>,
    pre: &ArrayPtrMap<'a>,
    states: &[(ArrayPtrMap<'a>, BasicBlock<'a>, HashSet<String>)],
    array_ptrs: &mut ArrayPtrMap<'a>,
    variables: &mut HashMap<String, BasicValueEnum<'a>>,
) -> MumeiResult<()> {
    *array_ptrs = pre.clone();
    for (state, _, _) in states {
        array_ptrs
            .stack_backed
            .extend(state.stack_backed.iter().cloned());
    }
    for (name, pre_array) in pre.iter() {
        if !states
            .iter()
            .any(|(_, _, assigned)| assigned.contains(name))
        {
            continue;
        }
        let values: Vec<ArrayPtr<'a>> = states
            .iter()
            .map(|(state, _, assigned)| {
                if assigned.contains(name) {
                    state.get(name).copied().ok_or_else(|| {
                        MumeiError::codegen(format!(
                            "array '{name}' reassigned to a non-array value in a branch"
                        ))
                    })
                } else {
                    Ok(*pre_array)
                }
            })
            .collect::<MumeiResult<_>>()?;
        let first = values
            .first()
            .copied()
            .ok_or_else(|| MumeiError::codegen(format!("array '{name}' has no branch state")))?;
        if values.iter().any(|(_, elem_ty, data_ptr)| {
            *elem_ty != first.1 || data_ptr.get_type() != first.2.get_type()
        }) {
            return Err(MumeiError::codegen(format!(
                "incompatible branch array type for '{name}'"
            )));
        }
        if values
            .iter()
            .any(|(len, _, _)| len.get_type() != first.0.get_type())
        {
            return Err(MumeiError::codegen(format!(
                "incompatible branch array length type for '{name}'"
            )));
        }
        let is_str = first.1.is_int_type() && first.1.into_int_type().get_bit_width() == 8;
        if values.iter().all(|(len, _, data_ptr)| {
            len.as_value_ref() == first.0.as_value_ref()
                && data_ptr.as_value_ref() == first.2.as_value_ref()
        }) {
            array_ptrs.insert(name.clone(), first);
            // `i8` elements mark a `Str` binding: its scalar value is
            // the `*i8` pointer (concat/extern uses), not the length.
            variables.insert(name.clone(), if is_str { first.2 } else { first.0 });
            continue;
        }
        let len_phi = llvm!(builder.build_phi(first.0.get_type(), &format!("phi_{name}_len")));
        let len_incoming: Vec<(&dyn BasicValue<'a>, BasicBlock<'a>)> = values
            .iter()
            .zip(states.iter())
            .map(|((len, _, _), (_, block, _))| (len as &dyn BasicValue<'a>, *block))
            .collect();
        len_phi.add_incoming(&len_incoming);

        let data_phi = llvm!(builder.build_phi(first.2.get_type(), &format!("phi_{name}_data")));
        let data_incoming: Vec<(&dyn BasicValue<'a>, BasicBlock<'a>)> = values
            .iter()
            .zip(states.iter())
            .map(|((_, _, data_ptr), (_, block, _))| (data_ptr as &dyn BasicValue<'a>, *block))
            .collect();
        data_phi.add_incoming(&data_incoming);

        let len = len_phi.as_basic_value();
        let data = data_phi.as_basic_value();
        array_ptrs.insert(name.clone(), (len, first.1, data));
        variables.insert(name.clone(), if is_str { data } else { len });
    }
    Ok(())
}

pub(crate) fn array_tail_binding(expr: &HirExpr) -> Option<&str> {
    fn stmt_tail_binding(stmt: &HirStmt) -> Option<&str> {
        match stmt {
            HirStmt::Expr(expr) => array_tail_binding(expr),
            HirStmt::Block { tail_expr, .. } => array_tail_binding(tail_expr.as_deref()?),
            _ => None,
        }
    }

    match expr {
        HirExpr::Variable(name) => Some(name),
        HirExpr::IfThenElse {
            cond, then_branch, ..
        } if matches!(cond.as_ref(), HirExpr::Variable(name) if name == "true") => {
            stmt_tail_binding(then_branch)
        }
        _ => None,
    }
}

/// Emit `E::V(..)` / `E::V` as a tagged-union struct value. Fail-closed on a
/// recursive enum (the eager `enum_llvm_type` layout cannot represent it) and
/// on arity/unknown-variant mismatches — verification rejects those first, so
/// these guards are defense-in-depth for HIR built outside the verify gate.
#[allow(clippy::too_many_arguments)]
fn emit_enum_variant_init<'a>(
    context: &'a Context,
    builder: &Builder<'a>,
    module: &Module<'a>,
    function: &FunctionValue<'a>,
    variables: &mut HashMap<String, BasicValueEnum<'a>>,
    var_types: &mut HashMap<String, String>,
    array_ptrs: &mut ArrayPtrMap<'a>,
    module_env: &ModuleEnv,
    enum_name: &str,
    variant_name: &str,
    fields: &[HirExpr],
) -> MumeiResult<BasicValueEnum<'a>> {
    let enum_def = module_env.get_enum(enum_name).ok_or_else(|| {
        MumeiError::codegen(format!("Enum '{}' not found in module_env", enum_name))
    })?;
    ensure_type_acyclic(module_env, enum_name)?;
    let variant = enum_def
        .variants
        .iter()
        .find(|v| v.name == variant_name)
        .ok_or_else(|| {
            MumeiError::codegen(format!(
                "Enum '{}' has no variant named '{}'",
                enum_name, variant_name
            ))
        })?;
    if fields.len() != variant.fields.len() {
        return Err(MumeiError::codegen(format!(
            "Enum constructor '{}::{}' takes {} payload arg(s), got {}",
            enum_name,
            variant_name,
            variant.fields.len(),
            fields.len()
        )));
    }
    let variant_idx = enum_def
        .variants
        .iter()
        .position(|v| v.name == variant_name)
        .unwrap_or(0);
    let enum_type = enum_llvm_type(context, enum_def, module_env)?;
    // Payload slots this variant does not fill must read as zero, not
    // undef: the verification-side model pads the flattened slot union
    // with zeroed slots, and translation validation compares them.
    let mut val = enum_type.const_zero();
    // Set tag
    val = llvm!(builder.build_insert_value(
        val,
        context.i64_type().const_int(variant_idx as u64, false),
        0,
        "tag"
    ))
    .into_struct_value();
    // Set payload fields
    for (i, field_expr) in fields.iter().enumerate() {
        let field_type = variant.field_types[i].display_name();
        let is_array = array_elem_llvm_type(context, &field_type, module_env)?.is_some();
        let array_binding = is_array.then(|| array_tail_binding(field_expr)).flatten();
        if is_array && array_binding.is_none() {
            let conditional = match field_expr {
                HirExpr::IfThenElse { cond, .. } => {
                    !matches!(cond.as_ref(), HirExpr::Variable(name) if name == "true")
                }
                HirExpr::Match { .. } => true,
                _ => false,
            };
            if conditional {
                return Err(MumeiError::codegen(format!(
                    "array payload argument for '{enum_name}::{variant_name}' cannot use if/match control flow"
                )));
            }
        }
        let field_val = if is_array {
            if let Some(name) = array_binding {
                match field_expr {
                    HirExpr::Variable(_) => {}
                    HirExpr::IfThenElse { then_branch, .. } => {
                        super::stmt_emit::compile_hir_stmt(
                            context,
                            builder,
                            module,
                            function,
                            then_branch,
                            variables,
                            var_types,
                            array_ptrs,
                            module_env,
                        )?;
                    }
                    _ => {
                        return Err(MumeiError::codegen(format!(
                            "array payload argument for '{enum_name}::{variant_name}' has an unsupported tail binding"
                        )));
                    }
                }
                let (len, elem_ty, data) = array_ptrs.get(name).copied().ok_or_else(|| {
                    MumeiError::codegen(format!(
                        "array payload '{name}' is not registered as a fat pointer"
                    ))
                })?;
                let expected_elem_ty = array_elem_llvm_type(context, &field_type, module_env)?
                    .ok_or_else(|| {
                        MumeiError::codegen(format!(
                            "enum '{enum_name}' array payload has no scalar element type"
                        ))
                    })?;
                if elem_ty != expected_elem_ty {
                    return Err(MumeiError::codegen(format!(
                        "array payload '{name}' has an incompatible element type"
                    )));
                }
                let array_ty = array_struct_type(context);
                let mut aggregate: inkwell::values::AggregateValueEnum =
                    array_ty.get_undef().into();
                aggregate = llvm!(builder.build_insert_value(aggregate, len, 0, "payload_arr_len"));
                aggregate =
                    llvm!(builder.build_insert_value(aggregate, data, 1, "payload_arr_data"));
                match aggregate {
                    inkwell::values::AggregateValueEnum::StructValue(value) => value.into(),
                    _ => {
                        return Err(MumeiError::codegen(
                            "array payload is not a fat-pointer aggregate",
                        ))
                    }
                }
            } else {
                let value = compile_hir_expr(
                    context, builder, module, function, field_expr, variables, var_types,
                    array_ptrs, module_env,
                )?;
                if value.get_type() != array_struct_type(context).into() {
                    return Err(MumeiError::codegen(format!(
                        "array payload argument for '{enum_name}::{variant_name}' did not resolve to a fat pointer"
                    )));
                }
                value
            }
        } else {
            compile_hir_expr(
                context, builder, module, function, field_expr, variables, var_types, array_ptrs,
                module_env,
            )?
        };
        if is_array && array_ptrs.return_contains_array_enum {
            let stack_backed = array_binding
                .map(|name| {
                    array_ptrs.stack_backed.contains(name) || !array_ptrs.contains_key(name)
                })
                .unwrap_or(true);
            if stack_backed {
                return Err(MumeiError::codegen(
                    "stack-backed array payload cannot escape through an array-bearing enum return"
                        .to_string(),
                ));
            }
        }
        let slot_ty = enum_type
            .get_field_type_at_index((i + 1) as u32)
            .ok_or_else(|| {
                MumeiError::codegen(format!("Enum '{}' missing payload slot {}", enum_name, i))
            })?;
        let field_val = if field_val.get_type() != slot_ty {
            bitpreserve_cast(builder, field_val, slot_ty)?
        } else {
            field_val
        };
        val = llvm!(builder.build_insert_value(
            val,
            field_val,
            (i + 1) as u32,
            &format!("payload_{}", i)
        ))
        .into_struct_value();
    }
    Ok(val.into())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn compile_hir_expr<'a>(
    context: &'a Context,
    builder: &Builder<'a>,
    module: &Module<'a>,
    function: &FunctionValue<'a>,
    expr: &HirExpr,
    variables: &mut HashMap<String, BasicValueEnum<'a>>,
    var_types: &mut HashMap<String, String>,
    array_ptrs: &mut ArrayPtrMap<'a>,
    module_env: &ModuleEnv,
) -> MumeiResult<BasicValueEnum<'a>> {
    match expr {
        HirExpr::Number(n) => Ok(context.i64_type().const_int(*n as u64, true).into()),

        HirExpr::Float(f) => Ok(context.f64_type().const_float(*f).into()),

        // Plan 9: String literal as global string pointer
        HirExpr::StringLit(s) => {
            let global = builder.build_global_string_ptr(s, "str_lit").map_err(|e| {
                MumeiError::codegen(format!("Failed to build string literal: {:?}", e))
            })?;
            Ok(global.as_pointer_value().into())
        }

        HirExpr::ArrayLit(elements) => {
            // Literal in expression position (e.g. a call argument):
            // materialise the backing store and return the `{i64 len, ptr}`
            // fat-pointer aggregate — the same value shape `[T]` parameters
            // carry, so it can be passed to a callee directly. A `let`
            // binding registers the name into `array_ptrs` before this arm
            // (see stmt_emit), so this path only runs for anonymous literals.
            let (len_val, elem_ty, data_ptr) = emit_array_literal(
                context, builder, module, function, elements, variables, var_types, array_ptrs,
                module_env,
            )?;
            let _ = elem_ty;
            let struct_ty = context.struct_type(
                &[
                    context.i64_type().into(),
                    context.ptr_type(AddressSpace::default()).into(),
                ],
                false,
            );
            let mut agg: inkwell::values::AggregateValueEnum = struct_ty.get_undef().into();
            agg = llvm!(builder.build_insert_value(agg, len_val, 0, "arrlit_len"));
            agg = llvm!(builder.build_insert_value(agg, data_ptr, 1, "arrlit_data"));
            match agg {
                inkwell::values::AggregateValueEnum::StructValue(s) => Ok(s.into()),
                _ => Err(MumeiError::codegen(
                    "array literal aggregate is not a struct value",
                )),
            }
        }

        HirExpr::Variable(name) => variables
            .get(name.as_str())
            .cloned()
            .or_else(|| match name.as_str() {
                // Booleans are i64 0/1 at runtime — same convention as the
                // `zext i1` results of comparisons.
                "true" => Some(context.i64_type().const_int(1, false).into()),
                "false" => Some(context.i64_type().const_int(0, false).into()),
                _ => None,
            })
            .ok_or_else(|| MumeiError::codegen(format!("Undefined variable: {}", name))),

        HirExpr::Call { name, args, .. } => match name.as_str() {
            "sqrt" => {
                let arg = compile_hir_expr(
                    context, builder, module, function, &args[0], variables, var_types, array_ptrs,
                    module_env,
                )?;
                let sqrt_func = module.get_function("llvm.sqrt.f64").unwrap_or_else(|| {
                    let type_f64 = context.f64_type();
                    let fn_type = type_f64.fn_type(&[type_f64.into()], false);
                    module.add_function("llvm.sqrt.f64", fn_type, None)
                });
                let call = llvm!(builder.build_call(sqrt_func, &[arg.into()], "sqrt_tmp"));
                let result = call.as_any_value_enum();
                Ok(result.into_float_value().into())
            }
            "len" => {
                if args.len() != 1 {
                    return Err(MumeiError::codegen("len() takes exactly one argument"));
                }
                let arg = &args[0];
                // `Str` values are null-terminated string pointers — the
                // runtime length is libc `strlen`. Verification (which
                // `build` runs first) only admits `len` on arrays and `Str`,
                // so a pointer-typed value reaching codegen is a `Str`.
                let strlen_call = |module: &inkwell::module::Module<'a>,
                                   builder: &inkwell::builder::Builder<'a>,
                                   ptr: inkwell::values::PointerValue<'a>|
                 -> MumeiResult<inkwell::values::IntValue<'a>> {
                    let strlen_ty = context
                        .i64_type()
                        .fn_type(&[context.ptr_type(AddressSpace::default()).into()], false);
                    let strlen_fn = module
                        .get_function("strlen")
                        .unwrap_or_else(|| module.add_function("strlen", strlen_ty, None));
                    let call = llvm!(builder.build_call(strlen_fn, &[ptr.into()], "str_len"));
                    Ok(call.as_any_value_enum().into_int_value())
                };
                if let HirExpr::Variable(arr_name) = arg {
                    if let Some((len_val, _, _)) = array_ptrs.get(arr_name.as_str()) {
                        return Ok(*len_val);
                    }
                    if let Some(val) = variables.get(arr_name.as_str()) {
                        if val.is_pointer_value() {
                            return Ok(
                                strlen_call(module, builder, val.into_pointer_value())?.into()
                            );
                        }
                        return Err(MumeiError::codegen(format!(
                            "len() is only supported on arrays and Str; '{arr_name}' is neither"
                        )));
                    }
                }
                match arg {
                    HirExpr::StringLit(s) => {
                        // Both sides count bytes: Z3 string literals are
                        // lowered from their UTF-8 encoding, and `strlen` on
                        // the emitted globals reports bytes too.
                        Ok(context.i64_type().const_int(s.len() as u64, false).into())
                    }
                    HirExpr::ArrayLit(elements) => Ok(context
                        .i64_type()
                        .const_int(elements.len() as u64, false)
                        .into()),
                    _ => {
                        // `len(if …)`, `len(match …)`, `len(call())`, `len(s + t)`:
                        // compile the argument and take the length off the
                        // runtime value — `{i64 len, ptr}` arrays expose field
                        // 0, `Str` values go through `strlen`.
                        let v = compile_hir_expr(
                            context, builder, module, function, arg, variables, var_types,
                            array_ptrs, module_env,
                        )?;
                        if v.is_pointer_value() {
                            return Ok(strlen_call(module, builder, v.into_pointer_value())?.into());
                        }
                        if v.is_struct_value() {
                            let len_val = llvm!(builder.build_extract_value(
                                v.into_struct_value(),
                                0,
                                "len_fat"
                            ));
                            return Ok(len_val.into_int_value().into());
                        }
                        Err(MumeiError::codegen(
                            "len() is only supported on arrays and Str values",
                        ))
                    }
                }
            }
            "alloc_raw" => {
                let size_val = compile_hir_expr(
                    context, builder, module, function, &args[0], variables, var_types, array_ptrs,
                    module_env,
                )?;
                let malloc_fn = module.get_function("malloc").unwrap_or_else(|| {
                    let ptr_type = context.ptr_type(AddressSpace::default());
                    let fn_type = ptr_type.fn_type(&[context.i64_type().into()], false);
                    module.add_function("malloc", fn_type, Some(inkwell::module::Linkage::External))
                });
                let byte_size = llvm!(builder.build_int_mul(
                    size_val.into_int_value(),
                    context.i64_type().const_int(8, false),
                    "byte_size"
                ));
                let ptr =
                    llvm!(builder.build_call(malloc_fn, &[byte_size.into()], "malloc_result"));
                let ptr_val = ptr.as_any_value_enum().into_pointer_value();
                Ok(
                    llvm!(builder.build_ptr_to_int(ptr_val, context.i64_type(), "ptr_as_int"))
                        .into(),
                )
            }
            "dealloc_raw" => {
                let ptr_int = compile_hir_expr(
                    context, builder, module, function, &args[0], variables, var_types, array_ptrs,
                    module_env,
                )?;
                let free_fn = module.get_function("free").unwrap_or_else(|| {
                    let ptr_type = context.ptr_type(AddressSpace::default());
                    let fn_type = context.void_type().fn_type(&[ptr_type.into()], false);
                    module.add_function("free", fn_type, Some(inkwell::module::Linkage::External))
                });
                let ptr_val = llvm!(builder.build_int_to_ptr(
                    ptr_int.into_int_value(),
                    context.ptr_type(AddressSpace::default()),
                    "int_as_ptr"
                ));
                llvm!(builder.build_call(free_fn, &[ptr_val.into()], "free_call"));
                Ok(context.i64_type().const_int(0, false).into())
            }
            _ => {
                // A `let`/`assign`-bound lambda shadows a same-named atom —
                // the verifier resolves `local_lambdas` before the atom
                // table, so the marker check must come first.
                if let Some(mark) = var_types
                    .get(name.as_str())
                    .filter(|m| m.starts_with(LAMBDA_MARK))
                    .cloned()
                {
                    return emit_lambda_call(
                        context, builder, module, function, &mark, args, variables, var_types,
                        array_ptrs, module_env,
                    );
                }
                let fqn_name = name.replace('.', "::");
                let resolved_callee = module_env
                    .get_atom(name)
                    .or_else(|| module_env.get_atom(&fqn_name));
                let inferred_callee_return = resolved_callee
                    .filter(|callee| callee.return_type.is_none())
                    .and_then(|callee| {
                        mumei_core::hir::lower_atom_to_hir(callee)
                            .signature
                            .inferred_return_type
                    });
                let callee_returns_array_enum = resolved_callee
                    .and_then(|callee| callee.return_type.as_deref())
                    .or(inferred_callee_return.as_deref())
                    .is_some_and(|type_name| type_contains_array_enum(module_env, type_name));
                if array_ptrs.return_contains_array_enum && callee_returns_array_enum {
                    if let Some(callee) = resolved_callee {
                        for (i, arg) in args.iter().enumerate() {
                            let param_is_array = callee
                                .params
                                .get(i)
                                .and_then(|param| param.type_name.as_deref())
                                .is_some_and(|type_name| {
                                    matches!(
                                        mumei_core::lowering::lower(
                                            &module_env.resolve_base_type(type_name)
                                        ),
                                        mumei_core::lowering::LoweredType::Array(_)
                                    )
                                });
                            if !param_is_array {
                                continue;
                            }
                            let stack_backed = match arg {
                                HirExpr::Variable(array_name) => {
                                    array_ptrs.stack_backed.contains(array_name)
                                        || !array_ptrs.contains_key(array_name)
                                }
                                _ => true,
                            };
                            if stack_backed {
                                return Err(MumeiError::codegen(format!(
                                    "stack-backed array argument to '{name}' cannot escape through an array-bearing enum return"
                                )));
                            }
                        }
                    }
                }
                // Lower the callee's signature only when a fresh function
                // declaration is needed — a callee already declared in this
                // module (the common case in multi-atom files) is reused
                // directly, so each callee lowers at most once per module.
                let callee_fn = if let Some(callee) = resolved_callee {
                    let callee_symbol = callee.name.as_str();
                    if let Some(existing) = module.get_function(callee_symbol) {
                        Some(existing)
                    } else {
                        let sig = mumei_core::hir::lower_atom_to_hir(callee).signature;
                        let callee_param_types: Vec<inkwell::types::BasicMetadataTypeEnum> = sig
                            .params
                            .iter()
                            .map(|p| {
                                resolve_param_type(context, p.ty.as_deref(), module_env)
                                    .map(Into::into)
                            })
                            .collect::<MumeiResult<_>>()?;

                        // Plan 18: Resolve callee return type from its return_type annotation
                        let callee_ret_type = resolve_return_type(context, &sig, module_env)?;
                        let fn_type = callee_ret_type.fn_type(&callee_param_types, false);
                        Some(module.add_function(
                            callee_symbol,
                            fn_type,
                            Some(inkwell::module::Linkage::External),
                        ))
                    }
                } else {
                    // A callee absent from module_env may already be declared
                    // in this module — e.g. `__mumei_user_main`, the renamed
                    // `main` in binary builds (self-recursive main must call
                    // it, not the C wrapper `main`). When it is not declared
                    // yet (a caller compiled before `main` itself), declare it
                    // from `main`'s own signature so the call still resolves.
                    if let Some(existing) = module.get_function(name) {
                        Some(existing)
                    } else if name == "__mumei_user_main" {
                        if let Some(main_atom) = module_env.get_atom("main") {
                            let sig = mumei_core::hir::lower_atom_to_hir(main_atom).signature;
                            let param_types: Vec<inkwell::types::BasicMetadataTypeEnum> = sig
                                .params
                                .iter()
                                .map(|p| {
                                    resolve_param_type(context, p.ty.as_deref(), module_env)
                                        .map(Into::into)
                                })
                                .collect::<MumeiResult<_>>()?;
                            let ret = resolve_return_type(context, &sig, module_env)?;
                            Some(module.add_function(name, ret.fn_type(&param_types, false), None))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                };
                if let Some(callee_fn) = callee_fn {
                    let mut arg_vals: Vec<inkwell::values::BasicMetadataValueEnum> = Vec::new();
                    for (i, arg) in args.iter().enumerate() {
                        // `[T]` params take the `{i64, ptr}` fat pointer —
                        // a bare `compile_hir_expr` on an array var yields
                        // only the len i64 and emits malformed call IR.
                        // `param.type_name` is identical to `HirParam.ty`
                        // (`ty: param.type_name.clone()` in HIR lowering).
                        let param_is_array = resolved_callee
                            .and_then(|c| c.params.get(i))
                            .and_then(|p| p.type_name.as_deref())
                            .is_some_and(|tn| {
                                matches!(
                                    mumei_core::lowering::lower(&module_env.resolve_base_type(tn)),
                                    mumei_core::lowering::LoweredType::Array(_)
                                )
                            });
                        if param_is_array {
                            let slots = match arg {
                                HirExpr::ArrayLit(elements) => Some(emit_array_literal(
                                    context, builder, module, function, elements, variables,
                                    var_types, array_ptrs, module_env,
                                )?),
                                HirExpr::Variable(name) => array_ptrs.get(name.as_str()).copied(),
                                _ => None,
                            };
                            let Some((len_val, _elem_ty, data_ptr)) = slots else {
                                return Err(MumeiError::codegen(format!(
                                    "array argument to '{}' (param {}) must be an array                                      binding or literal (got {:?})",
                                    name, i, arg
                                )));
                            };
                            let struct_ty = array_struct_type(context);
                            let mut agg: inkwell::values::AggregateValueEnum =
                                struct_ty.get_undef().into();
                            agg = llvm!(builder.build_insert_value(agg, len_val, 0, "arg_arr_len"));
                            agg =
                                llvm!(builder.build_insert_value(agg, data_ptr, 1, "arg_arr_data"));
                            let agg_val: BasicValueEnum = match agg {
                                inkwell::values::AggregateValueEnum::StructValue(s) => s.into(),
                                inkwell::values::AggregateValueEnum::ArrayValue(a) => a.into(),
                            };
                            arg_vals.push(agg_val.into());
                            continue;
                        }
                        let val = compile_hir_expr(
                            context, builder, module, function, arg, variables, var_types,
                            array_ptrs, module_env,
                        )?;
                        // A 128-bit parameter consumes a native `i128`
                        // (`pure-i64-composite/v21`): a literal or
                        // narrower operand extends into the slot.
                        let val = match callee_fn.get_nth_param(i as u32) {
                            Some(param) => {
                                let signed = resolved_callee
                                    .and_then(|c| c.params.get(i))
                                    .and_then(|p| p.type_name.as_deref())
                                    .map(|tn| module_env.resolve_base_type(tn))
                                    .as_deref()
                                    == Some("i128");
                                wide_int_cast(builder, val, param.get_type(), signed)?
                            }
                            None => val,
                        };
                        arg_vals.push(val.into());
                    }

                    let call_name = format!("call_{}", name.replace("::", "_").replace('.', "_"));
                    let call_result = llvm!(builder.build_call(callee_fn, &arg_vals, &call_name));
                    let result = call_result.as_any_value_enum();
                    if result.is_float_value() {
                        Ok(result.into_float_value().into())
                    } else if result.is_pointer_value() {
                        Ok(result.into_pointer_value().into())
                    } else if result.is_struct_value() {
                        Ok(result.into_struct_value().into())
                    } else {
                        Ok(result.into_int_value().into())
                    }
                } else {
                    Err(MumeiError::codegen(format!("Unknown function {}", name)))
                }
            }
        },

        HirExpr::ArrayAccess(name, index_expr) => {
            let idx = compile_hir_expr(
                context, builder, module, function, index_expr, variables, var_types, array_ptrs,
                module_env,
            )?
            .into_int_value();
            if let Some((len_val, elem_ty, data_ptr_val)) = array_ptrs.get(name.as_str()) {
                let data_ptr = data_ptr_val.into_pointer_value();
                let len_int = len_val.into_int_value();
                let in_bounds = llvm!(builder.build_int_compare(
                    IntPredicate::SLT,
                    idx,
                    len_int,
                    "bounds_check"
                ));
                let non_neg = llvm!(builder.build_int_compare(
                    IntPredicate::SGE,
                    idx,
                    context.i64_type().const_int(0, false),
                    "non_neg_check"
                ));
                let safe = llvm!(builder.build_and(in_bounds, non_neg, "safe_access"));

                let safe_block = context.append_basic_block(*function, "arr.safe");
                let oob_block = context.append_basic_block(*function, "arr.oob");
                let merge_block = context.append_basic_block(*function, "arr.merge");

                llvm!(builder.build_conditional_branch(safe, safe_block, oob_block));

                builder.position_at_end(safe_block);
                let elem_ptr =
                    unsafe { llvm!(builder.build_gep(*elem_ty, data_ptr, &[idx], "elem_ptr")) };
                let loaded = llvm!(builder.build_load(*elem_ty, elem_ptr, "elem_val"));
                // `Str` cells are `i8` (`pure-i64-composite/v13`); the
                // merged read is always `i64` — zero-extend narrower
                // elements so the byte read carries 0–255 like the
                // verifier's unsigned leaf.
                let loaded = if loaded.is_int_value() {
                    let int_val = loaded.into_int_value();
                    if int_val.get_type().get_bit_width() < 64 {
                        llvm!(builder.build_int_z_extend(int_val, context.i64_type(), "elem_zext"))
                            .into()
                    } else {
                        int_val.into()
                    }
                } else {
                    loaded
                };
                let result_ty: BasicTypeEnum = if *elem_ty == context.i8_type().into() {
                    context.i64_type().into()
                } else {
                    *elem_ty
                };
                let safe_end = builder.get_insert_block().unwrap();
                llvm!(builder.build_unconditional_branch(merge_block));

                builder.position_at_end(oob_block);
                let zero_val = result_ty.const_zero();
                let oob_end = builder.get_insert_block().unwrap();
                llvm!(builder.build_unconditional_branch(merge_block));

                builder.position_at_end(merge_block);
                let phi = llvm!(builder.build_phi(result_ty, "arr_result"));
                phi.add_incoming(&[(&loaded, safe_end), (&zero_val, oob_end)]);
                Ok(phi.as_basic_value())
            } else {
                Err(MumeiError::codegen(format!(
                    "Array '{}' not found as fat pointer parameter",
                    name
                )))
            }
        }

        HirExpr::BinaryOp(left, op, right) => {
            // `&&` / `||` short-circuit: the RHS is only evaluated when the
            // LHS does not decide the result (e.g. `i > 0 && arr[i] == x`
            // must not run the bounds check on a non-positive index).
            if matches!(op, HirBinOp::And | HirBinOp::Or) {
                let lhs = compile_hir_expr(
                    context, builder, module, function, left, variables, var_types, array_ptrs,
                    module_env,
                )?;
                if !lhs.is_int_value() {
                    return Err(MumeiError::codegen(format!(
                        "operator {:?} requires integer-bool operands in codegen",
                        op
                    )));
                }
                let lhs_int = lhs.into_int_value();
                let zero = context.i64_type().const_int(0, false);
                let l_bool =
                    llvm!(builder.build_int_compare(IntPredicate::NE, lhs_int, zero, "logic_lhs"));
                let rhs_block = context.append_basic_block(*function, "logic.rhs");
                let merge_block = context.append_basic_block(*function, "logic.merge");
                let lhs_block = builder.get_insert_block().unwrap();
                if matches!(op, HirBinOp::And) {
                    llvm!(builder.build_conditional_branch(l_bool, rhs_block, merge_block));
                } else {
                    llvm!(builder.build_conditional_branch(l_bool, merge_block, rhs_block));
                }
                builder.position_at_end(rhs_block);
                let rhs = compile_hir_expr(
                    context, builder, module, function, right, variables, var_types, array_ptrs,
                    module_env,
                )?;
                if !rhs.is_int_value() {
                    return Err(MumeiError::codegen(format!(
                        "operator {:?} requires integer-bool operands in codegen",
                        op
                    )));
                }
                let r_bool = llvm!(builder.build_int_compare(
                    IntPredicate::NE,
                    rhs.into_int_value(),
                    zero,
                    "logic_rhs"
                ));
                let rhs_end = builder.get_insert_block().unwrap();
                llvm!(builder.build_unconditional_branch(merge_block));
                builder.position_at_end(merge_block);
                let phi = llvm!(builder.build_phi(context.bool_type(), "logic_result"));
                let short_val = context
                    .bool_type()
                    .const_int(matches!(op, HirBinOp::Or) as u64, false);
                phi.add_incoming(&[(&short_val, lhs_block), (&r_bool, rhs_end)]);
                return Ok(llvm!(builder.build_int_z_extend(
                    phi.as_basic_value().into_int_value(),
                    context.i64_type(),
                    "logic_ext"
                ))
                .into());
            }
            let lhs = compile_hir_expr(
                context, builder, module, function, left, variables, var_types, array_ptrs,
                module_env,
            )?;
            let rhs = compile_hir_expr(
                context, builder, module, function, right, variables, var_types, array_ptrs,
                module_env,
            )?;

            if matches!(op, HirBinOp::Eq | HirBinOp::Neq)
                && [left.as_ref(), right.as_ref()].iter().any(|operand| {
                    infer_struct_type_name(operand, var_types, module_env)
                        .is_some_and(|ty| type_contains_array_enum(module_env, &ty))
                })
            {
                return Err(MumeiError::codegen(
                    "equality on enums with array payloads is not supported by LLVM codegen",
                ));
            }

            // Plan 9-8: String operations — if both operands are pointer (Str) type
            if lhs.is_pointer_value() && rhs.is_pointer_value() {
                let ptr_type = context.ptr_type(inkwell::AddressSpace::default());
                match op {
                    HirBinOp::Add => {
                        // Call runtime helper mumei_str_concat(a, b) -> *const c_char
                        let str_concat_fn =
                            module.get_function("mumei_str_concat").unwrap_or_else(|| {
                                let fn_type =
                                    ptr_type.fn_type(&[ptr_type.into(), ptr_type.into()], false);
                                module.add_function("mumei_str_concat", fn_type, None)
                            });
                        let result = llvm!(builder.build_call(
                            str_concat_fn,
                            &[lhs.into(), rhs.into()],
                            "str_concat_tmp"
                        ));
                        return result
                            .try_as_basic_value()
                            .left()
                            .ok_or(MumeiError::codegen("str_concat returned void".to_string()));
                    }
                    HirBinOp::Eq | HirBinOp::Neq => {
                        // Call runtime helper mumei_str_eq(a, b) -> i64 (0 or 1)
                        let str_eq_fn = module.get_function("mumei_str_eq").unwrap_or_else(|| {
                            let fn_type = context
                                .i64_type()
                                .fn_type(&[ptr_type.into(), ptr_type.into()], false);
                            module.add_function("mumei_str_eq", fn_type, None)
                        });
                        let result = llvm!(builder.build_call(
                            str_eq_fn,
                            &[lhs.into(), rhs.into()],
                            "str_eq_tmp"
                        ));
                        let eq_val = result
                            .try_as_basic_value()
                            .left()
                            .ok_or(MumeiError::codegen("str_eq returned void".to_string()))?
                            .into_int_value();
                        if matches!(op, HirBinOp::Neq) {
                            // Negate: result == 0 means not equal → flip
                            let negated = llvm!(builder.build_int_compare(
                                IntPredicate::EQ,
                                eq_val,
                                context.i64_type().const_int(0, false),
                                "str_neq_tmp"
                            ));
                            return Ok(llvm!(builder.build_int_z_extend(
                                negated,
                                context.i64_type(),
                                "str_neq_ext"
                            ))
                            .into());
                        }
                        return Ok(eq_val.into());
                    }
                    HirBinOp::Pow => {
                        return Err(MumeiError::codegen(
                            "Unsupported operator Pow for Str type in codegen".to_string(),
                        ));
                    }
                    _ => {
                        return Err(MumeiError::codegen(format!(
                            "Unsupported operator {:?} for Str type in codegen",
                            op
                        )));
                    }
                }
            }

            if lhs.is_float_value() || rhs.is_float_value() {
                let l = if lhs.is_float_value() {
                    lhs.into_float_value()
                } else {
                    llvm!(builder.build_signed_int_to_float(
                        lhs.into_int_value(),
                        context.f64_type(),
                        "int_to_float_l"
                    ))
                };
                let r = if rhs.is_float_value() {
                    rhs.into_float_value()
                } else {
                    llvm!(builder.build_signed_int_to_float(
                        rhs.into_int_value(),
                        context.f64_type(),
                        "int_to_float_r"
                    ))
                };
                match op {
                    HirBinOp::Add => Ok(llvm!(builder.build_float_add(l, r, "fadd_tmp")).into()),
                    HirBinOp::Sub => Ok(llvm!(builder.build_float_sub(l, r, "fsub_tmp")).into()),
                    HirBinOp::Mul => Ok(llvm!(builder.build_float_mul(l, r, "fmul_tmp")).into()),
                    HirBinOp::Div => Ok(llvm!(builder.build_float_div(l, r, "fdiv_tmp")).into()),
                    HirBinOp::Pow => Err(MumeiError::codegen(
                        "Unsupported float operator Pow".to_string(),
                    )),
                    HirBinOp::Eq => {
                        let cmp = llvm!(builder.build_float_compare(
                            FloatPredicate::OEQ,
                            l,
                            r,
                            "fcmp_tmp"
                        ));
                        Ok(
                            llvm!(builder.build_int_z_extend(cmp, context.i64_type(), "fbool_tmp"))
                                .into(),
                        )
                    }
                    _ => Err(MumeiError::codegen(format!(
                        "Unsupported float operator {:?}",
                        op
                    ))),
                }
            } else {
                if lhs.is_struct_value() || rhs.is_struct_value() {
                    // Enum equality compares tag + every payload slot —
                    // matching the verifier's deep datatype equality (P10-C).
                    // Pointer slots are `Str` payloads → mumei_str_eq.
                    if matches!(op, HirBinOp::Eq | HirBinOp::Neq)
                        && lhs.is_struct_value()
                        && rhs.is_struct_value()
                    {
                        let l_enum = infer_struct_type_name(left, var_types, module_env)
                            .filter(|n| module_env.get_enum(n).is_some());
                        let r_enum = infer_struct_type_name(right, var_types, module_env)
                            .filter(|n| module_env.get_enum(n).is_some());
                        if l_enum.is_some() && l_enum == r_enum && lhs.get_type() == rhs.get_type()
                        {
                            let ls = lhs.into_struct_value();
                            let rs = rhs.into_struct_value();
                            let field_count = ls.get_type().count_fields();
                            let mut acc = context.bool_type().const_int(1, false);
                            for idx in 0..field_count {
                                let lf = llvm!(builder.build_extract_value(ls, idx, "enum_eq_l"));
                                let rf = llvm!(builder.build_extract_value(rs, idx, "enum_eq_r"));
                                // `undef` slots (unit variants store nothing
                                // in them) carry no information — `and` with
                                // undef would poison the whole conjunction.
                                let is_undef = |v: BasicValueEnum| match v {
                                    BasicValueEnum::IntValue(iv) => iv.is_undef(),
                                    BasicValueEnum::FloatValue(fv) => fv.is_undef(),
                                    BasicValueEnum::PointerValue(pv) => pv.is_undef(),
                                    BasicValueEnum::StructValue(sv) => sv.is_undef(),
                                    BasicValueEnum::ArrayValue(av) => av.is_undef(),
                                    BasicValueEnum::VectorValue(vv) => vv.is_undef(),
                                };
                                if is_undef(lf) || is_undef(rf) {
                                    continue;
                                }
                                let cmp: inkwell::values::IntValue =
                                    enum_leaf_eq(builder, context, module, lf, rf)?;
                                acc = llvm!(builder.build_and(acc, cmp, "enum_eq_and"));
                            }
                            if matches!(op, HirBinOp::Neq) {
                                acc = llvm!(builder.build_not(acc, "enum_neq_not"));
                            }
                            return Ok(llvm!(builder.build_int_z_extend(
                                acc,
                                context.i64_type(),
                                "enum_eq_ext"
                            ))
                            .into());
                        }
                    }
                    return Err(MumeiError::codegen(format!(
                        "operator {:?} is unsupported on struct/enum values in codegen",
                        op
                    )));
                }
                // A 128-bit operand (`pure-i64-composite/v21`) widens the
                // pair: a literal or narrower operand extends to the
                // full width — the same `widen_literal_operands` the
                // verifier applies (`sext` for `i128`, `zext` for
                // `u128`), so `a + -1` keeps `-1` on `i128`.
                let unsigned =
                    binop_unsigned_operands(left, right, var_types, array_ptrs, module_env)?;
                let (lhs, rhs) = match (lhs, rhs) {
                    (BasicValueEnum::IntValue(lv), BasicValueEnum::IntValue(rv)) => {
                        let (lw, rw) =
                            (lv.get_type().get_bit_width(), rv.get_type().get_bit_width());
                        if lw < rw {
                            (
                                if unsigned {
                                    llvm!(builder.build_int_z_extend(lv, rv.get_type(), "w_lhs"))
                                        .into()
                                } else {
                                    llvm!(builder.build_int_s_extend(lv, rv.get_type(), "w_lhs"))
                                        .into()
                                },
                                rv.into(),
                            )
                        } else if rw < lw {
                            (
                                lv.into(),
                                if unsigned {
                                    llvm!(builder.build_int_z_extend(rv, lv.get_type(), "w_rhs"))
                                        .into()
                                } else {
                                    llvm!(builder.build_int_s_extend(rv, lv.get_type(), "w_rhs"))
                                        .into()
                                },
                            )
                        } else {
                            (lv.into(), rv.into())
                        }
                    }
                    other => other,
                };
                let l = lhs.into_int_value();
                let r = rhs.into_int_value();
                match op {
                    HirBinOp::Add => Ok(llvm!(builder.build_int_add(l, r, "add_tmp")).into()),
                    HirBinOp::Sub => Ok(llvm!(builder.build_int_sub(l, r, "sub_tmp")).into()),
                    HirBinOp::Mul => Ok(llvm!(builder.build_int_mul(l, r, "mul_tmp")).into()),
                    HirBinOp::Div if unsigned => {
                        Ok(llvm!(builder.build_int_unsigned_div(l, r, "udiv_tmp")).into())
                    }
                    HirBinOp::Div => {
                        Ok(llvm!(builder.build_int_signed_div(l, r, "div_tmp")).into())
                    }
                    HirBinOp::Rem if unsigned => {
                        Ok(llvm!(builder.build_int_unsigned_rem(l, r, "urem_tmp")).into())
                    }
                    HirBinOp::Rem => {
                        // `srem i64::MIN, -1` is undefined in LLVM; `x % -1` is
                        // always 0, which `x % 1` also yields.
                        let minus_one = r.get_type().const_all_ones();
                        let is_minus_one = llvm!(builder.build_int_compare(
                            IntPredicate::EQ,
                            r,
                            minus_one,
                            "rem_is_minus_one"
                        ));
                        let safe_r = llvm!(builder.build_select(
                            is_minus_one,
                            r.get_type().const_int(1, false),
                            r,
                            "rem_divisor"
                        ))
                        .into_int_value();
                        Ok(llvm!(builder.build_int_signed_rem(l, safe_r, "rem_tmp")).into())
                    }
                    HirBinOp::Pow => Err(MumeiError::codegen(
                        "Unsupported int operator Pow".to_string(),
                    )),
                    // Bit operations on the machine `i64` bit pattern, matching
                    // the `BV(64)` semantics verified under `--bitvec-i64`.
                    // `>>` is an arithmetic (sign-propagating) shift.
                    HirBinOp::BitAnd => Ok(llvm!(builder.build_and(l, r, "and_tmp")).into()),
                    HirBinOp::BitOr => Ok(llvm!(builder.build_or(l, r, "or_tmp")).into()),
                    HirBinOp::BitXor => Ok(llvm!(builder.build_xor(l, r, "xor_tmp")).into()),
                    HirBinOp::Shl => Ok(llvm!(builder.build_left_shift(l, r, "shl_tmp")).into()),
                    HirBinOp::Shr if unsigned => {
                        Ok(llvm!(builder.build_right_shift(l, r, false, "lshr_tmp")).into())
                    }
                    HirBinOp::Shr => {
                        Ok(llvm!(builder.build_right_shift(l, r, true, "ashr_tmp")).into())
                    }
                    HirBinOp::Eq
                    | HirBinOp::Neq
                    | HirBinOp::Lt
                    | HirBinOp::Gt
                    | HirBinOp::Ge
                    | HirBinOp::Le => {
                        let pred = match (op, unsigned) {
                            (HirBinOp::Eq, _) => IntPredicate::EQ,
                            (HirBinOp::Neq, _) => IntPredicate::NE,
                            (HirBinOp::Lt, true) => IntPredicate::ULT,
                            (HirBinOp::Gt, true) => IntPredicate::UGT,
                            (HirBinOp::Ge, true) => IntPredicate::UGE,
                            (HirBinOp::Le, true) => IntPredicate::ULE,
                            (HirBinOp::Lt, false) => IntPredicate::SLT,
                            (HirBinOp::Gt, false) => IntPredicate::SGT,
                            (HirBinOp::Ge, false) => IntPredicate::SGE,
                            (HirBinOp::Le, false) => IntPredicate::SLE,
                            _ => unreachable!(),
                        };
                        let cmp = llvm!(builder.build_int_compare(pred, l, r, "cmp_tmp"));
                        Ok(
                            llvm!(builder.build_int_z_extend(cmp, context.i64_type(), "bool_tmp"))
                                .into(),
                        )
                    }
                    _ => Err(MumeiError::codegen(format!(
                        "Unsupported int operator {:?}",
                        op
                    ))),
                }
            }
        }

        HirExpr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            let cond_val = compile_hir_expr(
                context, builder, module, function, cond, variables, var_types, array_ptrs,
                module_env,
            )?
            .into_int_value();
            let cond_bool = llvm!(builder.build_int_compare(
                IntPredicate::NE,
                cond_val,
                context.i64_type().const_int(0, false),
                "if_cond"
            ));

            let then_block = context.append_basic_block(*function, "then");
            let else_block = context.append_basic_block(*function, "else");
            let merge_block = context.append_basic_block(*function, "merge");
            let pre_vars = variables.clone();
            let pre_arrays = array_ptrs.clone();
            let then_assigned = collect_assigned_outer_variables_stmt(then_branch);
            let else_assigned = collect_assigned_outer_variables_stmt(else_branch);

            // Lambda bindings are branch-scoped like the verifier's
            // `local_lambdas`: restore the pre-branch markers before the
            // else side, then keep only markers both sides agree on. The
            // same scoping applies to every `var_types` entry — a type a
            // `then` assignment records must not retype the value `else`
            // still holds from before the branch.
            let pre_var_types = var_types.clone();

            llvm!(builder.build_conditional_branch(cond_bool, then_block, else_block));

            builder.position_at_end(then_block);
            let then_val = compile_hir_stmt(
                context,
                builder,
                module,
                function,
                then_branch,
                variables,
                var_types,
                array_ptrs,
                module_env,
            )?;
            let then_vars = variables.clone();
            let then_arrays = array_ptrs.clone();
            let then_marks = lambda_marks(var_types);
            let then_var_types = var_types.clone();
            *var_types = pre_var_types.clone();
            let then_end_block = builder.get_insert_block().unwrap();
            llvm!(builder.build_unconditional_branch(merge_block));

            builder.position_at_end(else_block);
            *variables = pre_vars.clone();
            *array_ptrs = pre_arrays.clone();
            let else_val = compile_hir_stmt(
                context,
                builder,
                module,
                function,
                else_branch,
                variables,
                var_types,
                array_ptrs,
                module_env,
            )?;
            let else_vars = variables.clone();
            let else_arrays = array_ptrs.clone();
            let else_marks = lambda_marks(var_types);
            let else_var_types = var_types.clone();
            merge_lambda_marks(var_types, &then_marks, &else_marks);
            // Non-lambda type tracking merges like the values below: both
            // branches must record the same type for a name, or the name's
            // declared pre-branch type stands. An entry only one branch
            // produced is dropped unless the name existed before the
            // branch.
            let mut merged_types: HashMap<String, String> = pre_var_types
                .iter()
                .filter(|(_, v)| !v.starts_with(LAMBDA_MARK))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            merged_types.retain(|name, _| {
                then_var_types.contains_key(name) || else_var_types.contains_key(name)
            });
            for (name, ty) in &then_var_types {
                if !ty.starts_with(LAMBDA_MARK) && else_var_types.get(name) == Some(ty) {
                    merged_types.insert(name.clone(), ty.clone());
                }
            }
            var_types.retain(|_, v| v.starts_with(LAMBDA_MARK));
            var_types.extend(merged_types);
            let else_end_block = builder.get_insert_block().unwrap();
            llvm!(builder.build_unconditional_branch(merge_block));

            builder.position_at_end(merge_block);
            // Array-producing branches merge their `(len, data)` slots
            // into a single `{i64, ptr}` phi — the scalar merge below
            // would compare a call's fat pointer against a binding's
            // length. The aggregate is materialised only after every phi
            // (the slot phis, the merged variable phis and
            // `merge_array_ptrs`'s) — a non-phi instruction emitted before
            // a phi makes the block invalid.
            let then_slots = array_result_slots(
                context,
                builder,
                then_branch,
                then_val,
                &then_arrays,
                &then_vars,
                then_end_block,
            )?;
            let else_slots = array_result_slots(
                context,
                builder,
                else_branch,
                else_val,
                &else_arrays,
                &else_vars,
                else_end_block,
            )?;
            let array_merge = match (then_slots, else_slots) {
                (Some((then_len, then_data)), Some((else_len, else_data))) => {
                    if then_len.get_type() != else_len.get_type()
                        || then_data.get_type() != else_data.get_type()
                    {
                        return Err(MumeiError::codegen(
                            "incompatible branch array value shapes",
                        ));
                    }
                    let len_phi = llvm!(builder.build_phi(then_len.get_type(), "if_arr_len"));
                    len_phi
                        .add_incoming(&[(&then_len, then_end_block), (&else_len, else_end_block)]);
                    let data_phi = llvm!(builder.build_phi(then_data.get_type(), "if_arr_data"));
                    data_phi.add_incoming(&[
                        (&then_data, then_end_block),
                        (&else_data, else_end_block),
                    ]);
                    Some((len_phi.as_basic_value(), data_phi.as_basic_value()))
                }
                (Some(_), None) | (None, Some(_)) => {
                    return Err(MumeiError::codegen(
                        "if branches disagree on whether the result is an array",
                    ));
                }
                (None, None) => None,
            };
            let scalar_result = if array_merge.is_none() {
                // A `Num` literal branch adapts to the other branch's
                // declared width in the verifier's merge — widen the
                // narrower LLVM value the same way before the phi
                // (`pure-i64-composite/v22`).
                let (phi_type, widened) = widen_merge_incomings(
                    context,
                    builder,
                    &[(then_val, then_end_block), (else_val, else_end_block)],
                    &[
                        stmt_int_type_name(then_branch, &then_var_types, module_env),
                        stmt_int_type_name(else_branch, &else_var_types, module_env),
                    ],
                )?;
                let phi = llvm!(builder.build_phi(phi_type, "if_result"));
                for (val, block) in &widened {
                    phi.add_incoming(&[(val, *block)]);
                }
                Some(phi.as_basic_value())
            } else {
                None
            };
            *variables = pre_vars.clone();
            for (name, pre_val) in &pre_vars {
                let then_changed = then_assigned.contains(name);
                let else_changed = else_assigned.contains(name);
                if !then_changed && !else_changed {
                    continue;
                }
                let then_val = if then_changed {
                    then_vars.get(name).cloned().unwrap_or(*pre_val)
                } else {
                    *pre_val
                };
                let else_val = if else_changed {
                    else_vars.get(name).cloned().unwrap_or(*pre_val)
                } else {
                    *pre_val
                };
                if then_val.get_type() != else_val.get_type() {
                    return Err(MumeiError::codegen(format!(
                        "incompatible branch types for variable '{name}'"
                    )));
                }
                if then_val.as_value_ref() == else_val.as_value_ref() {
                    variables.insert(name.clone(), then_val);
                } else {
                    let merged =
                        llvm!(builder.build_phi(then_val.get_type(), &format!("phi_{name}")));
                    merged
                        .add_incoming(&[(&then_val, then_end_block), (&else_val, else_end_block)]);
                    variables.insert(name.clone(), merged.as_basic_value());
                }
            }
            merge_array_ptrs(
                builder,
                &pre_arrays,
                &[
                    (then_arrays, then_end_block, then_assigned),
                    (else_arrays, else_end_block, else_assigned),
                ],
                array_ptrs,
                variables,
            )?;
            // Materialise the merged fat pointer last: `insert_value` is
            // not a phi and must follow every phi in this block.
            let result_val = if let Some((len_val, data_val)) = array_merge {
                let struct_ty = array_struct_type(context);
                let mut agg: inkwell::values::AggregateValueEnum = struct_ty.get_undef().into();
                agg = llvm!(builder.build_insert_value(agg, len_val, 0, "if_arr_len_slot"));
                agg = llvm!(builder.build_insert_value(agg, data_val, 1, "if_arr_data_slot"));
                match agg {
                    inkwell::values::AggregateValueEnum::StructValue(s) => s.as_basic_value_enum(),
                    _ => {
                        return Err(MumeiError::codegen(
                            "array if-merge value is not a struct aggregate",
                        ))
                    }
                }
            } else {
                scalar_result.ok_or_else(|| {
                    MumeiError::codegen("if merge produced neither array nor scalar result")
                })?
            };
            Ok(result_val)
        }

        HirExpr::StructInit { type_name, fields } => {
            let mut last_val: BasicValueEnum = context.i64_type().const_int(0, false).into();
            if let Some(sdef) = module_env.get_struct(type_name) {
                for (field_name, field_expr) in fields {
                    let val = compile_hir_expr(
                        context, builder, module, function, field_expr, variables, var_types,
                        array_ptrs, module_env,
                    )?;
                    let qualified = format!("__struct_{}_{}", type_name, field_name);
                    variables.insert(qualified, val);
                }
                let field_types: Vec<inkwell::types::BasicTypeEnum> = sdef
                    .fields
                    .iter()
                    .map(|f| {
                        // Plan 9: Use resolve_param_type for consistent type resolution
                        resolve_param_type(context, Some(f.type_name.as_str()), module_env)
                    })
                    .collect::<MumeiResult<_>>()?;
                let struct_type = context.struct_type(&field_types.to_vec(), false);
                let mut struct_val = struct_type.get_undef();
                for (field_name, _) in fields.iter() {
                    let qualified = format!("__struct_{}_{}", type_name, field_name);
                    if let Some(val) = variables.get(&qualified) {
                        let def_idx = sdef
                            .fields
                            .iter()
                            .position(|f| f.name == *field_name)
                            .ok_or_else(|| {
                                MumeiError::codegen(format!(
                                    "Field '{}' not found in struct '{}'",
                                    field_name, type_name
                                ))
                            })?;
                        // A 128-bit field (`pure-i64-composite/v21`)
                        // takes a native `i128` leaf — a literal or
                        // narrower stored value extends into it.
                        let coerced = wide_int_cast(
                            builder,
                            *val,
                            field_types[def_idx],
                            module_env.resolve_base_type(&sdef.fields[def_idx].type_name) == "i128",
                        )?;
                        struct_val = llvm!(builder.build_insert_value(
                            struct_val,
                            coerced,
                            def_idx as u32,
                            &format!("struct_{}", field_name)
                        ))
                        .into_struct_value();
                    }
                }
                last_val = struct_val.into();
            } else {
                for (field_name, field_expr) in fields {
                    let val = compile_hir_expr(
                        context, builder, module, function, field_expr, variables, var_types,
                        array_ptrs, module_env,
                    )?;
                    let qualified = format!("__struct_{}_{}", type_name, field_name);
                    variables.insert(qualified, val);
                    last_val = val;
                }
            }
            Ok(last_val)
        }

        HirExpr::Match { target, arms } => {
            let target_val = compile_hir_expr(
                context, builder, module, function, target, variables, var_types, array_ptrs,
                module_env,
            )?;
            let pre_vars = variables.clone();
            let pre_arrays = array_ptrs.clone();

            // Resolve the declared type through shared inference so array
            // element reads use the same enum hint as equality expressions.
            let enum_hint =
                infer_struct_type_name(target, var_types, module_env).and_then(|type_name| {
                    let resolved = module_env.resolve_base_type(&type_name);
                    module_env.get_enum(&resolved).map(|_| resolved)
                });

            let merge_block = context.append_basic_block(*function, "match.merge");
            let unreachable_block = context.append_basic_block(*function, "match.unreachable");

            let mut incoming: Vec<(BasicValueEnum<'a>, inkwell::basic_block::BasicBlock<'a>)> =
                Vec::new();
            let mut arm_names: Vec<Option<String>> = Vec::new();
            let mut arm_states: Vec<ArrayMergeState<'a>> = Vec::new();

            let arm_count = arms.len();
            let mut try_blocks: Vec<inkwell::basic_block::BasicBlock<'a>> = Vec::new();
            for i in 0..arm_count {
                try_blocks.push(context.append_basic_block(*function, &format!("match.try_{}", i)));
            }

            llvm!(builder.build_unconditional_branch(try_blocks[0]));

            for (i, arm) in arms.iter().enumerate() {
                let try_block = try_blocks[i];
                let fail_block = if i + 1 < arm_count {
                    try_blocks[i + 1]
                } else {
                    unreachable_block
                };

                builder.position_at_end(try_block);

                let pattern_matches = compile_pattern_test(
                    context,
                    builder,
                    &arm.pattern,
                    target_val,
                    &pre_vars,
                    module_env,
                    enum_hint.as_deref(),
                )?;

                let full_cond = if let Some(guard) = &arm.guard {
                    let mut guard_vars = pre_vars.clone();
                    let mut guard_var_types = var_types.clone();
                    *array_ptrs = pre_arrays.clone();
                    bind_pattern_variables(
                        context,
                        builder,
                        &arm.pattern,
                        target_val,
                        &mut guard_vars,
                        &mut guard_var_types,
                        array_ptrs,
                        module_env,
                        enum_hint.as_deref(),
                    )?;
                    let guard_val = compile_hir_expr(
                        context,
                        builder,
                        module,
                        function,
                        guard,
                        &mut guard_vars,
                        &mut guard_var_types,
                        array_ptrs,
                        module_env,
                    )?
                    .into_int_value();
                    let guard_bool = llvm!(builder.build_int_compare(
                        IntPredicate::NE,
                        guard_val,
                        context.i64_type().const_int(0, false),
                        "guard_cond"
                    ));
                    llvm!(builder.build_and(pattern_matches, guard_bool, "match_and_guard"))
                } else {
                    pattern_matches
                };

                let body_block =
                    context.append_basic_block(*function, &format!("match.body_{}", i));
                llvm!(builder.build_conditional_branch(full_cond, body_block, fail_block));

                builder.position_at_end(body_block);
                let mut arm_vars = pre_vars.clone();
                let mut arm_var_types = var_types.clone();
                *array_ptrs = pre_arrays.clone();
                bind_pattern_variables(
                    context,
                    builder,
                    &arm.pattern,
                    target_val,
                    &mut arm_vars,
                    &mut arm_var_types,
                    array_ptrs,
                    module_env,
                    enum_hint.as_deref(),
                )?;

                let body_val = compile_hir_stmt(
                    context,
                    builder,
                    module,
                    function,
                    &arm.body,
                    &mut arm_vars,
                    &mut arm_var_types,
                    array_ptrs,
                    module_env,
                )?;
                let body_end = builder.get_insert_block().unwrap();
                let arm_arrays = array_ptrs.clone();
                llvm!(builder.build_unconditional_branch(merge_block));
                incoming.push((body_val, body_end));
                arm_names.push(stmt_int_type_name(&arm.body, &arm_var_types, module_env));
                let assigned = collect_assigned_outer_variables_match_arm(arm);
                arm_states.push((arm_vars, arm_arrays, body_end, assigned));
            }

            // Array-producing arms merge their `(len, data)` slots the same
            // way the `if` merge does — a `Variable` tail resolves through
            // its `array_ptrs` entry, an aggregate tail unpacks in the arm's
            // end block. Arms must agree on the representation: mixing a
            // fat pointer with a scalar would feed mismatched types into
            // the merge phi.
            let mut arm_slots: Vec<Option<(BasicValueEnum<'a>, BasicValueEnum<'a>)>> =
                Vec::with_capacity(incoming.len());
            for (i, (body_val, body_end)) in incoming.iter().enumerate() {
                let (arm_vars, arm_arrays, _, _) = &arm_states[i];
                arm_slots.push(array_result_slots(
                    context,
                    builder,
                    &arms[i].body,
                    *body_val,
                    arm_arrays,
                    arm_vars,
                    *body_end,
                )?);
            }
            if arm_slots.iter().any(Option::is_some) && !arm_slots.iter().all(Option::is_some) {
                return Err(MumeiError::codegen(
                    "match arms disagree on whether the result is an array",
                ));
            }
            let array_merge: Option<Vec<(BasicValueEnum<'a>, BasicValueEnum<'a>)>> =
                if arm_slots.iter().all(Option::is_some) && !arm_slots.is_empty() {
                    Some(arm_slots.into_iter().map(Option::unwrap).collect())
                } else {
                    None
                };

            // The phi type is the widest int type any arm produced — a
            // `Num` literal arm adapts to a declared-width arm in the
            // verifier's merge, so narrower incomings extend the same
            // way before the phi (`pure-i64-composite/v22`). For
            // non-int results it stays the first arm's body type
            // (Plan 18) and must be computed before creating the
            // unreachable block value.
            // The unreachable block terminates into merge_block in both
            // modes; only the scalar path needs a fallback value of
            // `phi_type`. In array mode the slot phis take a (0, null)
            // incoming from it instead.
            let mut slot_incoming: Vec<(
                BasicValueEnum<'a>,
                BasicValueEnum<'a>,
                inkwell::basic_block::BasicBlock<'a>,
            )> = Vec::new();
            let scalar_result = if let Some(slots) = &array_merge {
                for ((body_val, body_end), (len, data)) in incoming.iter().zip(slots.iter()) {
                    let _ = body_val;
                    slot_incoming.push((*len, *data, *body_end));
                }
                builder.position_at_end(unreachable_block);
                llvm!(builder.build_unconditional_branch(merge_block));
                slot_incoming.push((
                    context.i64_type().const_int(0, false).into(),
                    context
                        .ptr_type(AddressSpace::default())
                        .const_null()
                        .into(),
                    unreachable_block,
                ));
                arm_states.push((
                    pre_vars.clone(),
                    pre_arrays.clone(),
                    unreachable_block,
                    std::collections::HashSet::new(),
                ));
                builder.position_at_end(merge_block);
                None
            } else {
                // The phi type is the widest int type any arm produced — a
                // `Num` literal arm adapts to a declared-width arm in the
                // verifier's merge, so narrower incomings extend the same
                // way before the phi (`pure-i64-composite/v22`). For
                // non-int results it stays the first arm's body type
                // (Plan 18) and must be computed before creating the
                // unreachable block value.
                let (phi_type, mut incoming) =
                    widen_merge_incomings(context, builder, &incoming, &arm_names)?;

                builder.position_at_end(unreachable_block);
                // Create a zero/null value matching the inferred phi type
                let unreachable_val: BasicValueEnum = if phi_type.is_float_type() {
                    context.f64_type().const_float(0.0).into()
                } else if phi_type.is_pointer_type() {
                    context
                        .ptr_type(AddressSpace::default())
                        .const_null()
                        .into()
                } else if phi_type.is_struct_type() {
                    // The fallback arm is unreachable; a zero aggregate
                    // keeps it poison-free so translation validation can
                    // model every slot (an `undef` leaf has no slot value).
                    phi_type.into_struct_type().const_zero().into()
                } else if phi_type.is_int_type() {
                    phi_type.into_int_type().const_int(0, false).into()
                } else {
                    context.i64_type().const_int(0, false).into()
                };
                llvm!(builder.build_unconditional_branch(merge_block));
                incoming.push((unreachable_val, unreachable_block));
                arm_states.push((
                    pre_vars.clone(),
                    pre_arrays.clone(),
                    unreachable_block,
                    std::collections::HashSet::new(),
                ));

                builder.position_at_end(merge_block);
                let phi = llvm!(builder.build_phi(phi_type, "match_result"));
                for (val, block) in &incoming {
                    phi.add_incoming(&[(val, *block)]);
                }
                Some(phi.as_basic_value())
            };

            // Slot phis first — every phi in merge_block must precede the
            // variable-merge phis and any non-phi instruction.
            let slot_phis = if slot_incoming.is_empty() {
                None
            } else {
                let len_ty = slot_incoming[0].0.get_type();
                let data_ty = slot_incoming[0].1.get_type();
                let len_phi = llvm!(builder.build_phi(len_ty, "match_arr_len"));
                let data_phi = llvm!(builder.build_phi(data_ty, "match_arr_data"));
                for (len, data, block) in &slot_incoming {
                    len_phi.add_incoming(&[(len, *block)]);
                    data_phi.add_incoming(&[(data, *block)]);
                }
                Some((len_phi.as_basic_value(), data_phi.as_basic_value()))
            };
            *variables = pre_vars.clone();
            for (name, pre_val) in &pre_vars {
                let mut merged_incoming = Vec::with_capacity(arm_states.len());
                let assigned_any = arm_states
                    .iter()
                    .any(|(_, _, _, assigned)| assigned.contains(name));
                if !assigned_any {
                    continue;
                }
                for (arm_vars, _, arm_end, assigned) in &arm_states {
                    let value = if assigned.contains(name) {
                        arm_vars.get(name).cloned().unwrap_or(*pre_val)
                    } else {
                        *pre_val
                    };
                    if value.get_type() != pre_val.get_type() {
                        return Err(MumeiError::codegen(format!(
                            "incompatible match-arm type for variable '{name}'"
                        )));
                    }
                    merged_incoming.push((value, *arm_end));
                }
                let first = merged_incoming
                    .first()
                    .expect("match always has an unreachable arm");
                if merged_incoming
                    .iter()
                    .all(|(value, _)| value.as_value_ref() == first.0.as_value_ref())
                {
                    variables.insert(name.clone(), first.0);
                } else {
                    let merged =
                        llvm!(builder.build_phi(first.0.get_type(), &format!("phi_{name}")));
                    let incoming_refs: Vec<(&dyn BasicValue<'a>, _)> = merged_incoming
                        .iter()
                        .map(|(value, block)| (value as &dyn BasicValue<'a>, *block))
                        .collect();
                    merged.add_incoming(&incoming_refs);
                    variables.insert(name.clone(), merged.as_basic_value());
                }
            }
            let array_states: Vec<(
                ArrayPtrMap<'a>,
                inkwell::basic_block::BasicBlock<'a>,
                std::collections::HashSet<String>,
            )> = arm_states
                .iter()
                .map(|(_, arrays, block, assigned)| (arrays.clone(), *block, assigned.clone()))
                .collect();
            merge_array_ptrs(builder, &pre_arrays, &array_states, array_ptrs, variables)?;

            // Materialise the merged fat pointer last, after every phi.
            if let Some((len_val, data_val)) = slot_phis {
                let struct_ty = array_struct_type(context);
                let mut agg: inkwell::values::AggregateValueEnum = struct_ty.get_undef().into();
                agg = llvm!(builder.build_insert_value(agg, len_val, 0, "match_arr_len_slot"));
                agg = llvm!(builder.build_insert_value(agg, data_val, 1, "match_arr_data_slot"));
                match agg {
                    inkwell::values::AggregateValueEnum::StructValue(s) => {
                        Ok(s.as_basic_value_enum())
                    }
                    _ => Err(MumeiError::codegen(
                        "array match-merge value is not a struct aggregate",
                    )),
                }
            } else {
                Ok(scalar_result.expect("scalar merge always produces a phi"))
            }
        }

        // =================================================================
        // 非同期処理の LLVM IR 生成
        // =================================================================
        HirExpr::Async { body } => compile_hir_stmt(
            context, builder, module, function, body, variables, var_types, array_ptrs, module_env,
        ),
        HirExpr::Await { expr: await_expr } => compile_hir_expr(
            context, builder, module, function, await_expr, variables, var_types, array_ptrs,
            module_env,
        ),

        // Plan 21 — concurrency runtime: spawn the task body on its own
        // pthread, join it, and return the joined i64 result. See
        // `compile_task_spawn` for layout / capture details.
        HirExpr::Task { body, .. } => compile_task_spawn(
            context, builder, module, function, body, variables, var_types, array_ptrs, module_env,
        ),
        HirExpr::TaskGroup {
            children,
            join_semantics,
        } => {
            // Each child is constructed in `hir::lower_stmt` as
            // `HirStmt::Expr(HirExpr::Task { body, … })`. We unwrap
            // that here so `emit_task_spawn_only` operates on the
            // *task body*, not the surrounding `Task` expression.
            let any_ctx = if matches!(join_semantics, HirJoin::Any) && !children.is_empty() {
                let i64_type = context.i64_type();
                let group_id = static_next_task_group_id()
                    .ok_or_else(|| MumeiError::codegen("task_group id overflow".to_string()))?;
                let parent_entry = function.get_first_basic_block().ok_or_else(|| {
                    MumeiError::codegen(String::from("parent function has no entry block"))
                })?;
                let saved = builder.get_insert_block();
                let group_result_ptr =
                    if let Some(first_inst) = parent_entry.get_first_instruction() {
                        builder.position_before(&first_inst);
                        llvm!(builder.build_alloca(i64_type, "task_group_any_result"))
                    } else {
                        builder.position_at_end(parent_entry);
                        llvm!(builder.build_alloca(i64_type, "task_group_any_result"))
                    };
                if let Some(block) = saved {
                    builder.position_at_end(block);
                }
                llvm!(builder.build_store(group_result_ptr, i64_type.const_int(0, false)));
                let (_complete_fn, _flag_fn, reset_fn, _cancel_fn, _wait_fn, _, _, _) =
                    declare_task_group_any_externs(context, module);
                llvm!(builder.build_call(
                    reset_fn,
                    &[i64_type.const_int(group_id, false).into()],
                    "task_group_reset_call",
                ));
                Some(TaskGroupAnyContext {
                    group_id,
                    result_ptr: group_result_ptr,
                })
            } else {
                None
            };
            let mut pending: Vec<PendingTask<'_>> = Vec::with_capacity(children.len());
            for child in children {
                let task_body: &HirStmt = match child {
                    HirStmt::Expr(HirExpr::Task { body, .. }) => body.as_ref(),
                    other => other,
                };
                pending.push(emit_task_spawn_only(
                    context, builder, module, function, task_body, variables, var_types,
                    array_ptrs, module_env, any_ctx,
                )?);
            }
            if let Some(any_ctx) = any_ctx {
                let i64_type = context.i64_type();
                let (_complete_fn, _flag_fn, _reset_fn, _cancel_fn, wait_fn, group_cancel_fn, _, _) =
                    declare_task_group_any_externs(context, module);
                if builder.get_insert_block().is_none() {
                    return Err(MumeiError::codegen(String::from(
                        "task_group:any has no insertion block",
                    )));
                }
                llvm!(builder.build_call(
                    wait_fn,
                    &[i64_type.const_int(any_ctx.group_id, false).into()],
                    "task_group_any_wait_call",
                ));
                llvm!(builder.build_call(
                    group_cancel_fn,
                    &[i64_type.const_int(any_ctx.group_id, false).into()],
                    "task_group_cancel_call",
                ));
                // Losers already released their boxed results inside the
                // wrapper; only the winner's slot (mirrored into the group
                // result) is still owned, so join raw and unbox once below.
                for p in &pending {
                    let _ = emit_task_join_raw(context, builder, module, p)?;
                }
                let result = llvm!(builder.build_load(
                    i64_type,
                    any_ctx.result_ptr,
                    "task_group_any_result"
                ))
                .into_int_value();
                // P25 — restore the winner's type when every child agrees on
                // it; mixed result types keep the raw i64.
                let shared_ty = pending
                    .first()
                    .map(|p| p.result_type())
                    .filter(|ty| pending.iter().all(|p| p.result_type() == *ty));
                match shared_ty {
                    Some(ty) => unbox_payload_from_i64(context, builder, module, result, ty),
                    None if pending.iter().any(|p| payload_needs_box(p.result_type())) => {
                        Err(mumei_core::verification::MumeiError::codegen(
                            "task_group:any children must share one result type when any child \
                             returns an aggregate: the winner's box could not be restored or freed"
                                .to_string(),
                        ))
                    }
                    None => Ok(result.into()),
                }
            } else {
                let mut last_val: BasicValueEnum = context.i64_type().const_int(0, false).into();
                for p in &pending {
                    last_val = emit_task_join_only(context, builder, module, p)?;
                }
                Ok(last_val)
            }
        }

        HirExpr::AtomRef { name } => {
            let func = if let Some(f) = module.get_function(name) {
                f
            } else if let Some(callee_atom) = module_env.get_atom(name) {
                let callee_sig = mumei_core::hir::lower_atom_to_hir(callee_atom).signature;
                let callee_param_types: Vec<inkwell::types::BasicMetadataTypeEnum> = callee_sig
                    .params
                    .iter()
                    .map(|p| {
                        resolve_param_type(context, p.ty.as_deref(), module_env).map(Into::into)
                    })
                    .collect::<MumeiResult<_>>()?;
                // Plan 18: Use resolve_return_type for consistent type resolution
                let callee_ret = resolve_return_type(context, &callee_sig, module_env)?;
                let fn_type = callee_ret.fn_type(&callee_param_types, false);
                module.add_function(name, fn_type, Some(inkwell::module::Linkage::External))
            } else {
                return Err(MumeiError::codegen(format!(
                    "atom_ref: unknown function '{}'",
                    name
                )));
            };
            let fn_ptr = func.as_global_value().as_pointer_value();
            let ptr_int = llvm!(builder.build_ptr_to_int(
                fn_ptr,
                context.i64_type(),
                &format!("atom_ref_{}", name)
            ));
            Ok(ptr_int.into())
        }
        HirExpr::CallRef { callee, args } => {
            // `call(f, …)` on a `let`/`assign`-bound lambda — the variable's
            // `@lam:` marker resolves to the lifted private fn directly.
            if let HirExpr::Variable(lam_var) = callee.as_ref() {
                if let Some(mark) = var_types
                    .get(lam_var.as_str())
                    .filter(|m| m.starts_with(LAMBDA_MARK))
                    .cloned()
                {
                    return emit_lambda_call(
                        context, builder, module, function, &mark, args, variables, var_types,
                        array_ptrs, module_env,
                    );
                }
            }
            // `call(|x| …, args)` — lift the inline lambda on the spot and
            // call it; captures bind at this site.
            if let HirExpr::Lambda {
                params,
                return_type,
                body,
                captures,
            } = callee.as_ref()
            {
                let (fn_name, all_caps) = emit_lambda_function(
                    context,
                    module,
                    function,
                    "inline",
                    params,
                    return_type.as_ref(),
                    captures,
                    body,
                    variables,
                    var_types,
                    array_ptrs,
                    module_env,
                )?;
                let mark = lambda_marker(&fn_name, &all_caps);
                return emit_lambda_call(
                    context, builder, module, function, &mark, args, variables, var_types,
                    array_ptrs, module_env,
                );
            }
            let callee_val = compile_hir_expr(
                context, builder, module, function, callee, variables, var_types, array_ptrs,
                module_env,
            )?;

            let mut arg_vals: Vec<BasicValueEnum> = Vec::new();
            for arg in args {
                let val = compile_hir_expr(
                    context, builder, module, function, arg, variables, var_types, array_ptrs,
                    module_env,
                )?;
                arg_vals.push(val);
            }

            if let HirExpr::AtomRef { name } = callee.as_ref() {
                if let Some(callee_fn) = module.get_function(name) {
                    let args_meta: Vec<BasicMetadataValueEnum> =
                        arg_vals.iter().map(|v| (*v).into()).collect();
                    let call_result = llvm!(builder.build_call(
                        callee_fn,
                        &args_meta,
                        &format!("call_ref_{}", name)
                    ));
                    return Ok(call_result
                        .try_as_basic_value()
                        .left()
                        .unwrap_or(context.i64_type().const_int(0, false).into()));
                }
            }

            let callee_int = callee_val.into_int_value();
            let param_types: Vec<BasicMetadataTypeEnum> = arg_vals
                .iter()
                .map(|v| {
                    if v.is_float_value() {
                        context.f64_type().into()
                    } else if v.is_pointer_value() {
                        context.ptr_type(AddressSpace::default()).into()
                    } else {
                        context.i64_type().into()
                    }
                })
                .collect();
            // Plan 18: Try to resolve return type from callee atom definition.
            // For indirect calls via AtomRef, prefer the function's declared
            // return type when it already exists in this module (it was built
            // via resolve_return_type); only lower the signature to declare
            // or resolve a callee not yet present.
            let indirect_ret_type = if let HirExpr::AtomRef { name } = callee.as_ref() {
                if let Some(f) = module.get_function(name) {
                    f.get_type().get_return_type()
                } else {
                    if let Some(a) = module_env.get_atom(name) {
                        let callee_sig = mumei_core::hir::lower_atom_to_hir(a).signature;
                        Some(resolve_return_type(context, &callee_sig, module_env)?)
                    } else {
                        None
                    }
                }
            } else {
                None
            };
            let fn_type = if let Some(ret) = indirect_ret_type {
                ret.fn_type(&param_types, false)
            } else {
                // Fallback: infer from argument types (no atom definition available)
                let has_float_arg = arg_vals.iter().any(|v| v.is_float_value());
                if has_float_arg {
                    context.f64_type().fn_type(&param_types, false)
                } else {
                    context.i64_type().fn_type(&param_types, false)
                }
            };
            let fn_ptr = llvm!(builder.build_int_to_ptr(
                callee_int,
                context.ptr_type(inkwell::AddressSpace::default()),
                "call_ref_fn_ptr"
            ));
            let args_meta: Vec<BasicMetadataValueEnum> =
                arg_vals.iter().map(|v| (*v).into()).collect();
            let call_result = llvm!(builder.build_indirect_call(
                fn_type,
                fn_ptr,
                &args_meta,
                "call_ref_indirect"
            ));
            Ok(call_result
                .try_as_basic_value()
                .left()
                .unwrap_or(context.i64_type().const_int(0, false).into()))
        }

        HirExpr::Perform {
            effect,
            operation,
            args: perform_args,
            ..
        } => {
            let fn_name = format!("__effect_{}_{}", effect, operation);

            let mut arg_vals = Vec::new();
            for arg in perform_args {
                let val = compile_hir_expr(
                    context, builder, module, function, arg, variables, var_types, array_ptrs,
                    module_env,
                )?;
                arg_vals.push(val);
            }

            let param_types: Vec<BasicMetadataTypeEnum> = arg_vals
                .iter()
                .map(|v| {
                    if v.is_float_value() {
                        context.f64_type().into()
                    } else {
                        context.i64_type().into()
                    }
                })
                .collect();
            let fn_type = context.i64_type().fn_type(&param_types, false);
            let callee_fn = if let Some(function) = module.get_function(&fn_name) {
                function
            } else {
                let function = module.add_function(&fn_name, fn_type, None);
                let current_block = builder.get_insert_block();
                let entry = context.append_basic_block(function, "entry");
                builder.position_at_end(entry);
                llvm!(builder.build_return(Some(&context.i64_type().const_int(0, false))));
                if let Some(block) = current_block {
                    builder.position_at_end(block);
                }
                function
            };

            let args_meta: Vec<BasicMetadataValueEnum> =
                arg_vals.iter().map(|v| (*v).into()).collect();
            let call_result = llvm!(builder.build_call(callee_fn, &args_meta, "perform_result"));
            Ok(call_result
                .try_as_basic_value()
                .left()
                .unwrap_or(context.i64_type().const_int(0, false).into()))
        }

        HirExpr::Lambda {
            params, captures, ..
        } => {
            // LLVM IR: Lambda as closure struct (function pointer + captured env)
            // For now, represent as an i64 tag (closure ID) — full closure conversion
            // will be added in a future phase with __Closure_N struct generation.
            let _param_count = params.len();
            let _capture_count = captures.len();
            // Return a symbolic closure ID
            Ok(context.i64_type().const_int(0, false).into())
        }

        // Plan 21 — concurrency runtime: channel send compiles to a call
        // into the runtime helper `__mumei_chan_send(chan_id: i64, value:
        // i64) -> void`. The runtime is responsible for the
        // `pthread_mutex_lock` / `pthread_cond_signal` / unlock sequence —
        // see `runtime/mumei_runtime.c`. The channel is referred to by an
        // i64 handle; the front-end is responsible for binding `let ch =
        // chan_new()` to a unique id (current grammar's `chan` literal
        // lowers to a fresh i64, so this just threads the handle through).
        HirExpr::ChanSend { channel, value } => {
            let chan_val = compile_hir_expr(
                context, builder, module, function, channel, variables, var_types, array_ptrs,
                module_env,
            )?;
            let val = compile_hir_expr(
                context, builder, module, function, value, variables, var_types, array_ptrs,
                module_env,
            )?;
            let chan_i64 = if chan_val.is_int_value() {
                chan_val.into_int_value()
            } else {
                context.i64_type().const_int(0, false)
            };
            // P25 — marshal the payload into the runtime's i64 slot without
            // losing bits: f64 is bitcast, Str / struct pointers go through
            // `ptrtoint`. Aggregates passed by value are copied into a heap
            // box whose address travels through the slot; `recv` loads and
            // frees it, so the box has exactly one owner at any time.
            let payload = match chan_payload_type_name(channel, var_types) {
                Some(name) => {
                    reject_array_chan_payload(&name, "send")?;
                    let payload_ty = resolve_param_type(context, Some(name.as_str()), module_env)?;
                    let coerced = coerce_to_chan_payload(builder, val, payload_ty)?;
                    if (payload_needs_box(payload_ty) || payload_needs_box(coerced.get_type()))
                        && coerced.get_type() != payload_ty
                    {
                        return Err(mumei_core::verification::MumeiError::codegen(format!(
                            "channel payload of type {} does not match the declared payload type {}",
                            coerced.get_type(),
                            payload_ty
                        )));
                    }
                    coerced
                }
                None => val,
            };
            let val_i64 = box_payload_to_i64(context, builder, module, payload).map_err(|e| {
                mumei_core::verification::MumeiError::codegen(format!(
                    "channel payload of type {} cannot be sent: {}",
                    payload.get_type(),
                    e
                ))
            })?;
            if payload_needs_box(payload.get_type()) {
                // The slot carries ownership of a heap box: if the runtime drops
                // the send (task-group cancellation) nobody will `recv` it, so
                // the sender frees it.
                let send_fn = declare_chan_send_owned(context, module);
                let sent = llvm!(builder.build_call(
                    send_fn,
                    &[chan_i64.into(), val_i64.into()],
                    "chan_send_owned_call"
                ))
                .try_as_basic_value()
                .left()
                .ok_or_else(|| {
                    mumei_core::verification::MumeiError::codegen(
                        "__mumei_chan_send_owned returned void".to_string(),
                    )
                })?
                .into_int_value();
                let dropped = llvm!(builder.build_int_compare(
                    inkwell::IntPredicate::EQ,
                    sent,
                    context.i64_type().const_zero(),
                    "chan_send_dropped"
                ));
                let release_bb = context.append_basic_block(*function, "chan_send_release");
                let cont_bb = context.append_basic_block(*function, "chan_send_cont");
                llvm!(builder.build_conditional_branch(dropped, release_bb, cont_bb));
                builder.position_at_end(release_bb);
                release_boxed_payload(context, builder, module, val_i64, payload.get_type())?;
                llvm!(builder.build_unconditional_branch(cont_bb));
                builder.position_at_end(cont_bb);
            } else {
                let send_fn = module.get_function("__mumei_chan_send").unwrap_or_else(|| {
                    let i64_type = context.i64_type();
                    let fn_type = context
                        .void_type()
                        .fn_type(&[i64_type.into(), i64_type.into()], false);
                    module.add_function(
                        "__mumei_chan_send",
                        fn_type,
                        Some(inkwell::module::Linkage::External),
                    )
                });
                llvm!(builder.build_call(
                    send_fn,
                    &[chan_i64.into(), val_i64.into()],
                    "chan_send_call"
                ));
            }
            Ok(context.i64_type().const_int(0, false).into())
        }

        // Plan 21 — concurrency runtime: channel receive calls
        // `__mumei_chan_recv(chan_id: i64) -> i64`, which performs a
        // `pthread_cond_wait` loop until a value is available. See
        // `runtime/mumei_runtime.c`.
        HirExpr::ChanRecv { channel } => {
            let chan_val = compile_hir_expr(
                context, builder, module, function, channel, variables, var_types, array_ptrs,
                module_env,
            )?;
            let chan_i64 = if chan_val.is_int_value() {
                chan_val.into_int_value()
            } else {
                context.i64_type().const_int(0, false)
            };
            let recv_fn = module.get_function("__mumei_chan_recv").unwrap_or_else(|| {
                let i64_type = context.i64_type();
                let fn_type = i64_type.fn_type(&[i64_type.into()], false);
                module.add_function(
                    "__mumei_chan_recv",
                    fn_type,
                    Some(inkwell::module::Linkage::External),
                )
            });
            let call = llvm!(builder.build_call(recv_fn, &[chan_i64.into()], "chan_recv_call"));
            let raw = call
                .try_as_basic_value()
                .left()
                .unwrap_or(context.i64_type().const_int(0, false).into());
            // P25 — restore the payload's declared type from the runtime's i64
            // slot (`chan<f64>` bitcasts back to double, `chan<Str>` goes
            // through `inttoptr`, aggregates are loaded from their heap box
            // and the box is freed). Channels whose payload type is unknown
            // keep the raw i64 the runtime returned.
            let payload_name = chan_payload_type_name(channel, var_types);
            if let Some(name) = &payload_name {
                reject_array_chan_payload(name, "recv")?;
            }
            let payload_ty = payload_name
                .map(|name| resolve_param_type(context, Some(name.as_str()), module_env))
                .transpose()?
                .filter(|ty| *ty != context.i64_type().into());
            match payload_ty {
                Some(ty) => {
                    Ok(
                        unbox_payload_from_i64(context, builder, module, raw.into_int_value(), ty)
                            .unwrap_or(raw),
                    )
                }
                None => Ok(raw),
            }
        }

        // Plan 14: Enum variant construction — build tagged union struct
        HirExpr::VariantInit {
            enum_name,
            variant_name,
            fields,
        } => emit_enum_variant_init(
            context,
            builder,
            module,
            function,
            variables,
            var_types,
            array_ptrs,
            module_env,
            enum_name,
            variant_name,
            fields,
        ),

        HirExpr::FieldAccess(inner_expr, field_name) => {
            if let HirExpr::Variable(var_name) = inner_expr.as_ref() {
                let resource_name = var_name
                    .strip_prefix("__mumei_resource_")
                    .unwrap_or(var_name);
                if let Some(resource_field) = module_env.resource_field(resource_name, field_name) {
                    let global_name =
                        format!("__mumei_res_{}_{}", resource_name, resource_field.name);
                    let global = module.get_global(&global_name).ok_or_else(|| {
                        MumeiError::codegen(format!(
                            "missing resource state global '{global_name}'"
                        ))
                    })?;
                    let value_ty: inkwell::types::BasicTypeEnum = match resource_field.ty.as_str() {
                        "bool" => context.bool_type().into(),
                        "f64" => context.f64_type().into(),
                        _ => context.i64_type().into(),
                    };
                    return Ok(llvm!(builder.build_load(
                        value_ty,
                        global.as_pointer_value(),
                        &format!("{}.{}", resource_name, field_name)
                    )));
                }
                let receiver_type = infer_struct_type_name(inner_expr, var_types, module_env)
                    .map(|name| module_env.resolve_base_type(&name));
                if let Some(enum_type) = receiver_type
                    .as_deref()
                    .filter(|name| module_env.get_enum(name).is_some())
                {
                    let enum_value =
                        variables.get(var_name.as_str()).copied().ok_or_else(|| {
                            MumeiError::codegen(format!(
                            "enum binding '{var_name}' has no LLVM value for field '{field_name}'"
                        ))
                        })?;
                    if field_name == "tag" {
                        if enum_value.is_int_value() {
                            return Ok(enum_value);
                        }
                        let BasicValueEnum::StructValue(enum_value) = enum_value else {
                            return Err(MumeiError::codegen(format!(
                                "enum binding '{var_name}' is not an LLVM aggregate for its tag"
                            )));
                        };
                        return Ok(llvm!(builder.build_extract_value(
                            enum_value,
                            0,
                            &format!("{var_name}.tag")
                        )));
                    }
                    let BasicValueEnum::StructValue(enum_value) = enum_value else {
                        return Err(MumeiError::codegen(format!(
                            "enum binding '{var_name}' is not an LLVM aggregate for field '{field_name}'"
                        )));
                    };
                    return emit_enum_payload_field_read(
                        context, builder, enum_value, enum_type, field_name, module_env,
                    );
                }
                let candidates = [
                    format!("__struct_{}_{}", var_name, field_name),
                    format!("{}_{}", var_name, field_name),
                ];
                for candidate in &candidates {
                    if let Some(val) = variables.get(candidate) {
                        return Ok(*val);
                    }
                }
                if let Some(struct_val) = variables.get(var_name.as_str()) {
                    if struct_val.is_struct_value() {
                        let sv = struct_val.into_struct_value();
                        let idx = match receiver_type {
                            // A declared receiver only reads its own
                            // declared fields — an unrelated struct's
                            // same-named field must not resolve, so
                            // look the field up on `t` directly rather
                            // than through find_field_index's
                            // every-struct scan.
                            Some(t) => module_env.get_struct(&t).and_then(|sdef| {
                                sdef.fields
                                    .iter()
                                    .position(|f| f.name == *field_name)
                                    .map(|i| i as u32)
                            }),
                            None => find_field_index(var_name, field_name, module_env),
                        };
                        if let Some(idx) = idx {
                            let extracted = llvm!(builder.build_extract_value(
                                sv,
                                idx,
                                &format!("{}.{}", var_name, field_name)
                            ));
                            return Ok(extracted);
                        }
                    }
                }
                // `E::V` / `E.V` unit-variant construction — only when
                // `var_name` is NOT a bound value (a bound variable named
                // like an enum shadows it, matching the verifier's
                // `env.contains_key` precedence) and `field_name` is one of
                // the enum's declared variants.
                if !variables.contains_key(var_name.as_str())
                    && !var_types.contains_key(var_name.as_str())
                {
                    if let Some(enum_def) = module_env.get_enum(var_name) {
                        if enum_def.variants.iter().any(|v| v.name == *field_name) {
                            return emit_enum_variant_init(
                                context,
                                builder,
                                module,
                                function,
                                variables,
                                var_types,
                                array_ptrs,
                                module_env,
                                var_name,
                                field_name,
                                &[],
                            );
                        }
                    }
                }
                Err(MumeiError::codegen(format!(
                    "Field '{}' not found on '{}'",
                    field_name, var_name
                )))
            } else {
                let base_val = compile_hir_expr(
                    context, builder, module, function, inner_expr, variables, var_types,
                    array_ptrs, module_env,
                )?;
                let receiver_type = infer_struct_type_name(inner_expr, var_types, module_env)
                    .map(|name| module_env.resolve_base_type(&name));
                if let Some(enum_type) = receiver_type
                    .as_deref()
                    .filter(|name| module_env.get_enum(name).is_some())
                {
                    if field_name == "tag" {
                        if base_val.is_struct_value() {
                            return Ok(llvm!(builder.build_extract_value(
                                base_val.into_struct_value(),
                                0,
                                &format!("nested.{field_name}")
                            )));
                        }
                        return Ok(base_val);
                    }
                    let BasicValueEnum::StructValue(enum_value) = base_val else {
                        return Err(MumeiError::codegen(format!(
                            "enum field base for '{field_name}' is not an LLVM aggregate"
                        )));
                    };
                    return emit_enum_payload_field_read(
                        context, builder, enum_value, enum_type, field_name, module_env,
                    );
                }
                if base_val.is_struct_value() {
                    let sv = base_val.into_struct_value();
                    let idx = receiver_type
                        .as_deref()
                        .and_then(|t| find_field_index(t, field_name, module_env))
                        .or_else(|| find_field_index_by_name(field_name, module_env));
                    if let Some(idx) = idx {
                        let extracted = llvm!(builder.build_extract_value(
                            sv,
                            idx,
                            &format!("nested.{}", field_name)
                        ));
                        Ok(extracted)
                    } else {
                        let extracted = llvm!(builder.build_extract_value(
                            sv,
                            0,
                            &format!("field_{}", field_name)
                        ));
                        Ok(extracted)
                    }
                } else {
                    Err(MumeiError::codegen(format!(
                        "Cannot access field '{}' on non-struct value",
                        field_name
                    )))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mumei_core::parser::ast::{Span, StructDef, StructField};
    use mumei_core::parser::parse_type_ref;

    #[test]
    fn struct_init_uses_definition_order_for_field_insertion(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let context = Context::create();
        let module = context.create_module("test");
        let builder = context.create_builder();
        let struct_type = context.struct_type(
            &[context.i64_type().into(), context.f64_type().into()],
            false,
        );
        let function_type = struct_type.fn_type(&[], false);
        let function = module.add_function("test", function_type, None);
        let entry = context.append_basic_block(function, "entry");
        builder.position_at_end(entry);

        let mut module_env = ModuleEnv::new();
        module_env.register_struct(&StructDef {
            name: "Pair".to_string(),
            type_params: vec![],
            fields: vec![
                StructField {
                    name: "a".to_string(),
                    type_name: "i64".to_string(),
                    type_ref: parse_type_ref("i64"),
                    constraint: None,
                },
                StructField {
                    name: "b".to_string(),
                    type_name: "f64".to_string(),
                    type_ref: parse_type_ref("f64"),
                    constraint: None,
                },
            ],
            invariants: vec![],
            method_names: vec![],
            methods: vec![],
            span: Span::default(),
        });

        let expr = HirExpr::StructInit {
            type_name: "Pair".to_string(),
            fields: vec![
                ("b".to_string(), HirExpr::Float(1.0)),
                ("a".to_string(), HirExpr::Number(2)),
            ],
        };

        let mut variables = HashMap::new();
        let mut var_types = HashMap::new();
        let mut array_ptrs = ArrayPtrMap::default();
        let result = compile_hir_expr(
            &context,
            &builder,
            &module,
            &function,
            &expr,
            &mut variables,
            &mut var_types,
            &mut array_ptrs,
            &module_env,
        );

        assert!(result.is_ok());
        let struct_val = result.unwrap().into_struct_value();
        let a_field = llvm!(builder.build_extract_value(struct_val, 0, "a_field"));
        let b_field = llvm!(builder.build_extract_value(struct_val, 1, "b_field"));
        assert!(a_field.is_int_value());
        assert!(b_field.is_float_value());
        llvm!(builder.build_return(Some(&struct_val)));
        assert!(function.verify(false));
        Ok(())
    }

    #[test]
    fn field_access_resolves_against_known_struct_type() -> Result<(), Box<dyn std::error::Error>> {
        let context = Context::create();
        let module = context.create_module("test");
        let builder = context.create_builder();
        let function_type = context.f64_type().fn_type(&[], false);
        let function = module.add_function("test", function_type, None);
        let entry = context.append_basic_block(function, "entry");
        builder.position_at_end(entry);

        let mut module_env = ModuleEnv::new();
        module_env.register_struct(&StructDef {
            name: "A".to_string(),
            type_params: vec![],
            fields: vec![
                StructField {
                    name: "pad".to_string(),
                    type_name: "i64".to_string(),
                    type_ref: parse_type_ref("i64"),
                    constraint: None,
                },
                StructField {
                    name: "val".to_string(),
                    type_name: "f64".to_string(),
                    type_ref: parse_type_ref("f64"),
                    constraint: None,
                },
            ],
            invariants: vec![],
            method_names: vec![],
            methods: vec![],
            span: Span::default(),
        });
        module_env.register_struct(&StructDef {
            name: "B".to_string(),
            type_params: vec![],
            fields: vec![
                StructField {
                    name: "val".to_string(),
                    type_name: "i64".to_string(),
                    type_ref: parse_type_ref("i64"),
                    constraint: None,
                },
                StructField {
                    name: "extra".to_string(),
                    type_name: "f64".to_string(),
                    type_ref: parse_type_ref("f64"),
                    constraint: None,
                },
            ],
            invariants: vec![],
            method_names: vec![],
            methods: vec![],
            span: Span::default(),
        });

        let a_struct_type = context.struct_type(
            &[context.i64_type().into(), context.f64_type().into()],
            false,
        );
        let mut a_struct = a_struct_type.get_undef();
        a_struct = llvm!(builder.build_insert_value(
            a_struct,
            context.i64_type().const_int(0, false),
            0,
            "a_pad"
        ))
        .into_struct_value();
        a_struct = llvm!(builder.build_insert_value(
            a_struct,
            context.f64_type().const_float(3.5),
            1,
            "a_val"
        ))
        .into_struct_value();

        let expr = HirExpr::FieldAccess(
            Box::new(HirExpr::Variable("B".to_string())),
            "val".to_string(),
        );

        let mut variables = HashMap::new();
        variables.insert("B".to_string(), a_struct.into());
        let mut var_types = HashMap::new();
        var_types.insert("B".to_string(), "A".to_string());
        let mut array_ptrs = ArrayPtrMap::default();
        let result = compile_hir_expr(
            &context,
            &builder,
            &module,
            &function,
            &expr,
            &mut variables,
            &mut var_types,
            &mut array_ptrs,
            &module_env,
        )?;

        assert!(result.is_float_value());
        llvm!(builder.build_return(Some(&result)));
        assert!(function.verify(false));
        Ok(())
    }

    // `let s = "hi"` registers a `(len, i8, *i8)` fat pointer and `s[i]`
    // emits the bounds-checked `i8` load zero-extended to `i64` — matching
    // the verifier's byte-cell model (`pure-i64-composite/v13`).
    #[test]
    fn str_let_binds_fat_pointer_and_index_read_zero_extends(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let context = Context::create();
        let module = context.create_module("test");
        let builder = context.create_builder();
        let function = module.add_function("test", context.i64_type().fn_type(&[], false), None);
        let entry = context.append_basic_block(function, "entry");
        builder.position_at_end(entry);

        let module_env = ModuleEnv::new();
        let mut variables = HashMap::new();
        let mut var_types = HashMap::new();
        let mut array_ptrs = ArrayPtrMap::default();

        crate::codegen::stmt_emit::compile_hir_stmt(
            &context,
            &builder,
            &module,
            &function,
            &HirStmt::Let {
                var: "s".to_string(),
                ty: None,
                value: Box::new(HirExpr::StringLit("hi".to_string())),
            },
            &mut variables,
            &mut var_types,
            &mut array_ptrs,
            &module_env,
        )?;

        // The bound value stays the `*i8` pointer; the fat pointer is i8-typed.
        assert!(variables["s"].is_pointer_value());
        let (len, elem_ty, _) = array_ptrs["s"];
        assert_eq!(len.into_int_value().get_zero_extended_constant(), Some(2));
        assert_eq!(elem_ty, context.i8_type().into());

        let read = compile_hir_expr(
            &context,
            &builder,
            &module,
            &function,
            &HirExpr::ArrayAccess("s".to_string(), Box::new(HirExpr::Number(0))),
            &mut variables,
            &mut var_types,
            &mut array_ptrs,
            &module_env,
        )?;
        assert!(read.is_int_value());
        assert_eq!(read.into_int_value().get_type().get_bit_width(), 64);

        llvm!(builder.build_return(Some(&read)));
        assert!(function.verify(false));
        assert!(module.print_to_string().to_string().contains("zext i8"));
        Ok(())
    }

    // `let t = s` on a `Str` binding aliases the fat pointer and keeps the
    // `*i8` pointer as the scalar value, not the length.
    #[test]
    fn str_alias_keeps_pointer_value() -> Result<(), Box<dyn std::error::Error>> {
        let context = Context::create();
        let module = context.create_module("test");
        let builder = context.create_builder();
        let function = module.add_function("test", context.i64_type().fn_type(&[], false), None);
        let entry = context.append_basic_block(function, "entry");
        builder.position_at_end(entry);

        let module_env = ModuleEnv::new();
        let mut variables = HashMap::new();
        let mut var_types = HashMap::new();
        let mut array_ptrs = ArrayPtrMap::default();

        for stmt in [
            HirStmt::Let {
                var: "s".to_string(),
                ty: None,
                value: Box::new(HirExpr::StringLit("hi".to_string())),
            },
            HirStmt::Let {
                var: "t".to_string(),
                ty: None,
                value: Box::new(HirExpr::Variable("s".to_string())),
            },
        ] {
            crate::codegen::stmt_emit::compile_hir_stmt(
                &context,
                &builder,
                &module,
                &function,
                &stmt,
                &mut variables,
                &mut var_types,
                &mut array_ptrs,
                &module_env,
            )?;
        }

        assert!(variables["t"].is_pointer_value());
        assert_eq!(array_ptrs["t"].1, context.i8_type().into());
        llvm!(builder.build_return(Some(&context.i64_type().const_int(0, false))));
        assert!(function.verify(false));
        Ok(())
    }

    // `s[0] > (0 - 1)` must emit `ugt`: the verifier's `s[i]` byte leaf is
    // `u64`, so a signed `sgt` would return `true` for `'h' > -1` while the
    // source model says `104 > u64::MAX` is false.
    #[test]
    fn str_index_read_compares_unsigned() -> Result<(), Box<dyn std::error::Error>> {
        let context = Context::create();
        let module = context.create_module("test");
        let builder = context.create_builder();
        let function = module.add_function("test", context.i64_type().fn_type(&[], false), None);
        let entry = context.append_basic_block(function, "entry");
        builder.position_at_end(entry);

        let module_env = ModuleEnv::new();
        let mut variables = HashMap::new();
        let mut var_types = HashMap::new();
        let mut array_ptrs = ArrayPtrMap::default();

        compile_hir_stmt(
            &context,
            &builder,
            &module,
            &function,
            &HirStmt::Let {
                var: "s".to_string(),
                ty: None,
                value: Box::new(HirExpr::StringLit("hi".to_string())),
            },
            &mut variables,
            &mut var_types,
            &mut array_ptrs,
            &module_env,
        )?;

        let cmp = compile_hir_expr(
            &context,
            &builder,
            &module,
            &function,
            &HirExpr::BinaryOp(
                Box::new(HirExpr::ArrayAccess(
                    "s".to_string(),
                    Box::new(HirExpr::Number(0)),
                )),
                HirBinOp::Gt,
                Box::new(HirExpr::BinaryOp(
                    Box::new(HirExpr::Number(0)),
                    HirBinOp::Sub,
                    Box::new(HirExpr::Number(1)),
                )),
            ),
            &mut variables,
            &mut var_types,
            &mut array_ptrs,
            &module_env,
        )?;
        llvm!(builder.build_return(Some(&cmp)));
        assert!(function.verify(false));
        assert!(module.print_to_string().to_string().contains("icmp ugt"));
        Ok(())
    }

    #[test]
    fn if_expression_with_u64_branch_compares_unsigned() -> Result<(), Box<dyn std::error::Error>> {
        let context = Context::create();
        let module = context.create_module("test");
        let builder = context.create_builder();
        let function = module.add_function("test", context.i64_type().fn_type(&[], false), None);
        let entry = context.append_basic_block(function, "entry");
        builder.position_at_end(entry);

        let module_env = ModuleEnv::new();
        let mut variables = HashMap::new();
        variables.insert(
            "a".to_string(),
            context.i64_type().const_int(u64::MAX, false).into(),
        );
        variables.insert(
            "b".to_string(),
            context.i64_type().const_int(0, false).into(),
        );
        variables.insert(
            "c".to_string(),
            context.i64_type().const_int(1, false).into(),
        );
        let mut var_types = HashMap::new();
        var_types.insert("a".to_string(), "u64".to_string());
        var_types.insert("b".to_string(), "u64".to_string());
        let mut array_ptrs = ArrayPtrMap::default();

        let condition = HirExpr::BinaryOp(
            Box::new(HirExpr::IfThenElse {
                cond: Box::new(HirExpr::Variable("c".to_string())),
                then_branch: Box::new(HirStmt::Expr(HirExpr::BinaryOp(
                    Box::new(HirExpr::Variable("a".to_string())),
                    HirBinOp::BitOr,
                    Box::new(HirExpr::Number(0)),
                ))),
                else_branch: Box::new(HirStmt::Expr(HirExpr::Variable("b".to_string()))),
            }),
            HirBinOp::Le,
            Box::new(HirExpr::Number(10)),
        );
        let cmp = compile_hir_expr(
            &context,
            &builder,
            &module,
            &function,
            &condition,
            &mut variables,
            &mut var_types,
            &mut array_ptrs,
            &module_env,
        )?;
        llvm!(builder.build_return(Some(&cmp)));
        assert!(function.verify(false));
        assert!(module.print_to_string().to_string().contains("icmp ule"));
        Ok(())
    }

    #[test]
    fn if_expression_with_local_let_shadow_compares_signed(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let context = Context::create();
        let module = context.create_module("test");
        let builder = context.create_builder();
        let function = module.add_function("test", context.i64_type().fn_type(&[], false), None);
        let entry = context.append_basic_block(function, "entry");
        builder.position_at_end(entry);

        let module_env = ModuleEnv::new();
        let mut variables = HashMap::new();
        variables.insert(
            "a".to_string(),
            context.i64_type().const_int(u64::MAX, false).into(),
        );
        variables.insert(
            "c".to_string(),
            context.i64_type().const_int(1, false).into(),
        );
        let mut var_types = HashMap::new();
        var_types.insert("a".to_string(), "u64".to_string());
        let mut array_ptrs = ArrayPtrMap::default();
        let negative_one = || {
            HirExpr::BinaryOp(
                Box::new(HirExpr::Number(0)),
                HirBinOp::Sub,
                Box::new(HirExpr::Number(1)),
            )
        };
        let condition = HirExpr::BinaryOp(
            Box::new(HirExpr::IfThenElse {
                cond: Box::new(HirExpr::Variable("c".to_string())),
                then_branch: Box::new(HirStmt::Block {
                    stmts: vec![HirStmt::Let {
                        var: "a".to_string(),
                        ty: None,
                        value: Box::new(negative_one()),
                    }],
                    tail_expr: Some(Box::new(HirExpr::Variable("a".to_string()))),
                }),
                else_branch: Box::new(HirStmt::Block {
                    stmts: vec![],
                    tail_expr: Some(Box::new(negative_one())),
                }),
            }),
            HirBinOp::Lt,
            Box::new(HirExpr::Number(0)),
        );
        let cmp = compile_hir_expr(
            &context,
            &builder,
            &module,
            &function,
            &condition,
            &mut variables,
            &mut var_types,
            &mut array_ptrs,
            &module_env,
        )?;
        llvm!(builder.build_return(Some(&cmp)));
        assert!(function.verify(false));
        assert!(module.print_to_string().to_string().contains("icmp slt"));
        Ok(())
    }

    #[test]
    fn block_local_struct_field_compares_unsigned() -> Result<(), Box<dyn std::error::Error>> {
        let context = Context::create();
        let module = context.create_module("test");
        let builder = context.create_builder();
        let function = module.add_function("test", context.i64_type().fn_type(&[], false), None);
        let entry = context.append_basic_block(function, "entry");
        builder.position_at_end(entry);

        let mut module_env = ModuleEnv::new();
        module_env.register_struct(&StructDef {
            name: "S".to_string(),
            type_params: vec![],
            fields: vec![StructField {
                name: "x".to_string(),
                type_name: "u64".to_string(),
                type_ref: parse_type_ref("u64"),
                constraint: None,
            }],
            invariants: vec![],
            method_names: vec![],
            methods: vec![],
            span: Span::default(),
        });

        let mut variables = HashMap::new();
        variables.insert(
            "a".to_string(),
            context.i64_type().const_int(u64::MAX, false).into(),
        );
        variables.insert(
            "c".to_string(),
            context.i64_type().const_int(1, false).into(),
        );
        let mut var_types = HashMap::new();
        var_types.insert("a".to_string(), "u64".to_string());
        let mut array_ptrs = ArrayPtrMap::default();
        let block = || HirStmt::Block {
            stmts: vec![HirStmt::Let {
                var: "p".to_string(),
                ty: None,
                value: Box::new(HirExpr::StructInit {
                    type_name: "S".to_string(),
                    fields: vec![("x".to_string(), HirExpr::Variable("a".to_string()))],
                }),
            }],
            tail_expr: Some(Box::new(HirExpr::FieldAccess(
                Box::new(HirExpr::Variable("p".to_string())),
                "x".to_string(),
            ))),
        };
        let condition = HirExpr::BinaryOp(
            Box::new(HirExpr::IfThenElse {
                cond: Box::new(HirExpr::Variable("c".to_string())),
                then_branch: Box::new(block()),
                else_branch: Box::new(block()),
            }),
            HirBinOp::Lt,
            Box::new(HirExpr::Number(1)),
        );
        let cmp = compile_hir_expr(
            &context,
            &builder,
            &module,
            &function,
            &condition,
            &mut variables,
            &mut var_types,
            &mut array_ptrs,
            &module_env,
        )?;
        llvm!(builder.build_return(Some(&cmp)));
        assert!(function.verify(false));
        assert!(module.print_to_string().to_string().contains("icmp ult"));
        Ok(())
    }

    // `s = "bb"` rebinds the fat pointer (general builds keep their
    // pre-v13 string-reassign behavior) and `s = 5` drops it so a stale
    // `s[i]` cannot read the old bytes.
    #[test]
    fn str_reassign_rebinds_or_drops_fat_pointer() -> Result<(), Box<dyn std::error::Error>> {
        let context = Context::create();
        let module = context.create_module("test");
        let builder = context.create_builder();
        let function = module.add_function("test", context.i64_type().fn_type(&[], false), None);
        let entry = context.append_basic_block(function, "entry");
        builder.position_at_end(entry);

        let module_env = ModuleEnv::new();
        let mut variables = HashMap::new();
        let mut var_types = HashMap::new();
        let mut array_ptrs = ArrayPtrMap::default();
        macro_rules! run {
            ($stmt:expr) => {
                compile_hir_stmt(
                    &context,
                    &builder,
                    &module,
                    &function,
                    $stmt,
                    &mut variables,
                    &mut var_types,
                    &mut array_ptrs,
                    &module_env,
                )
            };
        }

        run!(&HirStmt::Let {
            var: "s".to_string(),
            ty: None,
            value: Box::new(HirExpr::StringLit("a".to_string())),
        })?;
        run!(&HirStmt::Assign {
            var: "s".to_string(),
            value: Box::new(HirExpr::StringLit("bb".to_string())),
        })?;
        assert!(variables["s"].is_pointer_value());
        assert_eq!(
            array_ptrs["s"]
                .0
                .into_int_value()
                .get_zero_extended_constant(),
            Some(2)
        );

        run!(&HirStmt::Assign {
            var: "s".to_string(),
            value: Box::new(HirExpr::Number(5)),
        })?;
        assert!(!array_ptrs.contains_key("s"));
        assert!(variables["s"].is_int_value());

        llvm!(builder.build_return(Some(&variables["s"])));
        assert!(function.verify(false));
        Ok(())
    }

    // `t = s` on a `Str` binding keeps the `*i8` pointer (not the length)
    // in scalar position — the same rule as the `let t = s` alias.
    #[test]
    fn str_assign_alias_keeps_pointer_value() -> Result<(), Box<dyn std::error::Error>> {
        let context = Context::create();
        let module = context.create_module("test");
        let builder = context.create_builder();
        let function = module.add_function("test", context.i64_type().fn_type(&[], false), None);
        let entry = context.append_basic_block(function, "entry");
        builder.position_at_end(entry);

        let module_env = ModuleEnv::new();
        let mut variables = HashMap::new();
        let mut var_types = HashMap::new();
        let mut array_ptrs = ArrayPtrMap::default();
        macro_rules! run {
            ($stmt:expr) => {
                compile_hir_stmt(
                    &context,
                    &builder,
                    &module,
                    &function,
                    $stmt,
                    &mut variables,
                    &mut var_types,
                    &mut array_ptrs,
                    &module_env,
                )
            };
        }

        for stmt in [
            HirStmt::Let {
                var: "s".to_string(),
                ty: None,
                value: Box::new(HirExpr::StringLit("hi".to_string())),
            },
            HirStmt::Let {
                var: "t".to_string(),
                ty: None,
                value: Box::new(HirExpr::Number(0)),
            },
            HirStmt::Assign {
                var: "t".to_string(),
                value: Box::new(HirExpr::Variable("s".to_string())),
            },
        ] {
            run!(&stmt)?;
        }

        assert!(variables["t"].is_pointer_value());
        assert_eq!(array_ptrs["t"].1, context.i8_type().into());
        llvm!(builder.build_return(Some(&context.i64_type().const_int(0, false))));
        assert!(function.verify(false));
        Ok(())
    }

    // A captured `Str` fat pointer binds the `*i8` data pointer inside
    // the lifted lambda — binding the length would hand an i64 to
    // `concat`/string uses.
    #[test]
    fn str_capture_binds_data_pointer() -> Result<(), Box<dyn std::error::Error>> {
        let context = Context::create();
        let module = context.create_module("test");
        let builder = context.create_builder();
        let function = module.add_function("test", context.i64_type().fn_type(&[], false), None);
        let entry = context.append_basic_block(function, "entry");
        builder.position_at_end(entry);

        let module_env = ModuleEnv::new();
        let mut variables = HashMap::new();
        let mut var_types = HashMap::new();
        let mut array_ptrs = ArrayPtrMap::default();

        let global = builder.build_global_string_ptr("hi", "str_lit")?;
        let data_ptr: BasicValueEnum = global.as_pointer_value().into();
        let len_val: BasicValueEnum = context.i64_type().const_int(2, false).into();
        let struct_ty = array_struct_type(&context);
        let mut agg: inkwell::values::AggregateValueEnum = struct_ty.get_undef().into();
        agg = llvm!(builder.build_insert_value(agg, len_val, 0, "cap_len"));
        agg = llvm!(builder.build_insert_value(agg, data_ptr, 1, "cap_data"));
        let fat: BasicValueEnum = match agg {
            inkwell::values::AggregateValueEnum::StructValue(s) => s.into(),
            _ => panic!("expected struct"),
        };

        bind_lambda_arg(
            &context,
            &builder,
            "s",
            fat,
            None,
            Some((len_val, context.i8_type().into(), data_ptr)),
            None,
            &mut variables,
            &mut var_types,
            &mut array_ptrs,
            &module_env,
        )?;

        assert!(variables["s"].is_pointer_value());
        assert_eq!(array_ptrs["s"].1, context.i8_type().into());
        llvm!(builder.build_return(Some(&context.i64_type().const_int(0, false))));
        assert!(function.verify(false));
        Ok(())
    }

    // `if c { s = "bb" }` merges the `Str` fat pointer; the merged
    // scalar must be the `*i8` data pointer — binding the length phi
    // instead makes a later `s + "!"` lower pointer concat onto an i64.
    #[test]
    fn str_branch_merge_keeps_data_pointer_scalar() -> Result<(), Box<dyn std::error::Error>> {
        let context = Context::create();
        let module = context.create_module("test");
        let builder = context.create_builder();
        let function = module.add_function("test", context.i64_type().fn_type(&[], false), None);
        let entry = context.append_basic_block(function, "entry");
        builder.position_at_end(entry);

        let module_env = ModuleEnv::new();
        let mut variables = HashMap::new();
        let mut var_types = HashMap::new();
        let mut array_ptrs = ArrayPtrMap::default();
        macro_rules! run {
            ($stmt:expr) => {
                compile_hir_stmt(
                    &context,
                    &builder,
                    &module,
                    &function,
                    $stmt,
                    &mut variables,
                    &mut var_types,
                    &mut array_ptrs,
                    &module_env,
                )
            };
        }

        run!(&HirStmt::Let {
            var: "s".to_string(),
            ty: None,
            value: Box::new(HirExpr::StringLit("a".to_string())),
        })?;
        run!(&HirStmt::Let {
            var: "c".to_string(),
            ty: None,
            value: Box::new(HirExpr::Number(1)),
        })?;
        run!(&HirStmt::Let {
            var: "r".to_string(),
            ty: None,
            value: Box::new(HirExpr::IfThenElse {
                cond: Box::new(HirExpr::Variable("c".to_string())),
                then_branch: Box::new(HirStmt::Block {
                    stmts: vec![HirStmt::Assign {
                        var: "s".to_string(),
                        value: Box::new(HirExpr::StringLit("bb".to_string())),
                    }],
                    tail_expr: Some(Box::new(HirExpr::Number(1))),
                }),
                else_branch: Box::new(HirStmt::Block {
                    stmts: vec![],
                    tail_expr: Some(Box::new(HirExpr::Number(2))),
                }),
            }),
        })?;

        assert!(variables["s"].is_pointer_value());
        assert_eq!(array_ptrs["s"].1, context.i8_type().into());

        let concat = compile_hir_expr(
            &context,
            &builder,
            &module,
            &function,
            &HirExpr::BinaryOp(
                Box::new(HirExpr::Variable("s".to_string())),
                HirBinOp::Add,
                Box::new(HirExpr::StringLit("!".to_string())),
            ),
            &mut variables,
            &mut var_types,
            &mut array_ptrs,
            &module_env,
        )?;
        assert!(concat.is_pointer_value());
        assert!(module
            .print_to_string()
            .to_string()
            .contains("mumei_str_concat"));
        llvm!(builder.build_return(Some(&context.i64_type().const_int(0, false))));
        assert!(function.verify(false));
        Ok(())
    }

    // `s = f(1)` evaluates the RHS before dropping `s`'s fat pointer:
    // a lambda capturing `s` packs `{i64, ptr}` from `array_ptrs`, so an
    // early drop would pass the raw `*i8` against a struct parameter.
    #[test]
    fn str_reassign_evaluates_rhs_before_dropping_fat_pointer(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let context = Context::create();
        let module = context.create_module("test");
        let builder = context.create_builder();
        let function = module.add_function("test", context.i64_type().fn_type(&[], false), None);
        let entry = context.append_basic_block(function, "entry");
        builder.position_at_end(entry);

        let module_env = ModuleEnv::new();
        let mut variables = HashMap::new();
        let mut var_types = HashMap::new();
        let mut array_ptrs = ArrayPtrMap::default();
        macro_rules! run {
            ($stmt:expr) => {
                compile_hir_stmt(
                    &context,
                    &builder,
                    &module,
                    &function,
                    $stmt,
                    &mut variables,
                    &mut var_types,
                    &mut array_ptrs,
                    &module_env,
                )
            };
        }

        run!(&HirStmt::Let {
            var: "s".to_string(),
            ty: None,
            value: Box::new(HirExpr::StringLit("hi".to_string())),
        })?;
        run!(&HirStmt::Let {
            var: "f".to_string(),
            ty: None,
            value: Box::new(HirExpr::Lambda {
                params: vec![mumei_core::hir::HirLambdaParam {
                    name: "x".to_string(),
                    type_ref: None,
                }],
                return_type: None,
                body: Box::new(HirStmt::Block {
                    stmts: vec![],
                    tail_expr: Some(Box::new(HirExpr::BinaryOp(
                        Box::new(HirExpr::ArrayAccess(
                            "s".to_string(),
                            Box::new(HirExpr::Number(0)),
                        )),
                        HirBinOp::Add,
                        Box::new(HirExpr::Variable("x".to_string())),
                    ))),
                }),
                captures: vec!["s".to_string()],
            }),
        })?;
        run!(&HirStmt::Assign {
            var: "s".to_string(),
            value: Box::new(HirExpr::Call {
                name: "f".to_string(),
                args: vec![HirExpr::Number(1)],
                callee_effects: None,
            }),
        })?;

        assert!(!array_ptrs.contains_key("s"));
        assert!(variables["s"].is_int_value());
        llvm!(builder.build_return(Some(&variables["s"])));
        assert!(function.verify(false));
        Ok(())
    }
}
