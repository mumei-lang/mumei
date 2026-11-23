use crate::codegen::expr_emit::{
    chan_payload_key, chan_payload_type_name, compile_hir_expr, copy_array_to_heap,
    emit_array_literal, hir_int_type_name, infer_struct_type_name, is_fixed_int_type_name,
    resolve_named_type, LAMBDA_MARK,
};
use crate::codegen::lowering::{
    array_elem_llvm_type, bitpreserve_cast, resolve_param_type, struct_llvm_type,
    type_contains_array_enum, wide_int_cast, ArrayPtrMap,
};
use crate::codegen::pattern_emit::find_field_index;
use crate::codegen::task_runtime::declare_task_group_should_cancel_current_extern;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::values::{AsValueRef, BasicValueEnum, FunctionValue, PhiValue};
use inkwell::AddressSpace;
use inkwell::IntPredicate;
use mumei_core::hir::{collect_assigned_outer_variables_stmt, HirExpr, HirStmt};
use mumei_core::verification::{ModuleEnv, MumeiError, MumeiResult};
use std::collections::HashMap;

fn enum_value_is_stack_backed(
    expr: &HirExpr,
    var_types: &HashMap<String, String>,
    array_ptrs: &ArrayPtrMap<'_>,
    module_env: &ModuleEnv,
) -> MumeiResult<bool> {
    match expr {
        HirExpr::VariantInit {
            enum_name,
            variant_name,
            fields,
        } => {
            let Some(variant) = module_env
                .get_enum(enum_name)
                .and_then(|enum_def| enum_def.variants.iter().find(|v| v.name == *variant_name))
            else {
                return Ok(false);
            };
            for (field_type, field_expr) in variant.field_types.iter().zip(fields) {
                let field_type = module_env.resolve_base_type(&field_type.display_name());
                if field_type.starts_with('[') || field_type.starts_with("[]<") {
                    let stack_backed = super::expr_emit::array_tail_binding(field_expr)
                        .map(|name| {
                            array_ptrs.stack_backed.contains(name) || !array_ptrs.contains_key(name)
                        })
                        .unwrap_or(true);
                    if stack_backed {
                        return Ok(true);
                    }
                }
            }
            Ok(false)
        }
        HirExpr::Variable(name) => Ok(array_ptrs.stack_backed.contains(name)),
        _ => Ok(
            super::expr_emit::infer_struct_type_name(expr, var_types, module_env)
                .is_some_and(|type_name| type_contains_array_enum(module_env, &type_name)),
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_deep_aggregate_field_write<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &Module<'ctx>,
    function: &FunctionValue<'ctx>,
    var: &str,
    base: &str,
    root_type: &str,
    segments: &[&str],
    value: &HirExpr,
    variables: &mut HashMap<String, BasicValueEnum<'ctx>>,
    var_types: &mut HashMap<String, String>,
    array_ptrs: &mut ArrayPtrMap<'ctx>,
    module_env: &ModuleEnv,
) -> MumeiResult<Option<BasicValueEnum<'ctx>>> {
    let resolved_root_type = module_env.resolve_base_type(root_type);
    let root_is_struct = module_env.get_struct(&resolved_root_type).is_some();
    if !root_is_struct && module_env.get_enum(&resolved_root_type).is_none() {
        return Ok(None);
    }

    let mut parent_type = resolved_root_type;
    let mut parent_types = Vec::with_capacity(segments.len());
    let mut declared_types = Vec::with_capacity(segments.len());
    let mut enum_payload_slots = Vec::with_capacity(segments.len());
    let mut index_path = Vec::with_capacity(segments.len());
    for segment in segments {
        let Some((index, declared_type, enum_payload_slot)) =
            deep_aggregate_field_slot(module_env, &parent_type, segment, var)?
        else {
            return Ok(None);
        };
        parent_types.push(parent_type);
        parent_type = module_env.resolve_base_type(&declared_type);
        index_path.push(index);
        declared_types.push(declared_type);
        enum_payload_slots.push(enum_payload_slot);
    }

    let old = variables.get(base).copied().ok_or_else(|| {
        MumeiError::codegen(format!(
            "aggregate binding '{base}' has no value for deep field write '{var}'"
        ))
    })?;
    let BasicValueEnum::StructValue(root_aggregate) = old else {
        return Err(MumeiError::codegen(format!(
            "aggregate binding '{base}' is not an LLVM struct for deep field write '{var}'"
        )));
    };
    let mut ancestors = vec![root_aggregate];
    for (level, segment) in segments[..segments.len() - 1].iter().enumerate() {
        let aggregate = ancestors[level];
        let index = index_path[level];
        let expected_type = resolve_param_type(context, Some(&declared_types[level]), module_env)?;
        let slot_type = aggregate
            .get_type()
            .get_field_type_at_index(index)
            .ok_or_else(|| {
                MumeiError::codegen(format!("aggregate path '{var}' has no slot '{segment}'"))
            })?;
        if slot_type != expected_type {
            return Err(deep_slot_type_error(
                module_env,
                &parent_types[level],
                &declared_types[level],
                index,
                enum_payload_slots[level],
            ));
        }
        let nested = llvm!(builder.build_extract_value(aggregate, index, "deep_field"));
        if nested.get_type() != expected_type {
            return Err(deep_slot_type_error(
                module_env,
                &parent_types[level],
                &declared_types[level],
                index,
                enum_payload_slots[level],
            ));
        }
        let BasicValueEnum::StructValue(nested) = nested else {
            return Err(MumeiError::codegen(format!(
                "deep field path '{var}' crosses non-aggregate field '{segment}'"
            )));
        };
        ancestors.push(nested);
    }

    let leaf_level = segments.len() - 1;
    let leaf_index = index_path[leaf_level];
    let expected_type = resolve_param_type(context, Some(&declared_types[leaf_level]), module_env)?;
    let slot_type = ancestors[leaf_level]
        .get_type()
        .get_field_type_at_index(leaf_index)
        .ok_or_else(|| {
            MumeiError::codegen(format!(
                "aggregate path '{var}' has no slot '{}'",
                segments[leaf_level]
            ))
        })?;
    if slot_type != expected_type {
        return Err(deep_slot_type_error(
            module_env,
            &parent_types[leaf_level],
            &declared_types[leaf_level],
            leaf_index,
            enum_payload_slots[leaf_level],
        ));
    }

    let written = compile_hir_expr(
        context, builder, module, function, value, variables, var_types, array_ptrs, module_env,
    )?;
    let leaf_type = module_env.resolve_base_type(&declared_types[leaf_level]);
    let written = if leaf_type == "i128" || leaf_type == "u128" {
        wide_int_cast(builder, written, expected_type, leaf_type == "i128")?
    } else {
        written
    };
    if written.get_type() != expected_type {
        return Err(MumeiError::codegen(format!(
            "deep field path '{var}' expects value type {expected_type}, got {}",
            written.get_type()
        )));
    }

    let mut rebuilt: BasicValueEnum = written;
    let mut first_subtree = None;
    for level in (0..segments.len()).rev() {
        rebuilt =
            llvm!(builder.build_insert_value(ancestors[level], rebuilt, index_path[level], var))
                .into_struct_value()
                .into();
        if root_is_struct && level == 1 {
            first_subtree = Some(rebuilt);
        }
    }
    variables.insert(base.to_string(), rebuilt);
    if let Some(subtree) = first_subtree {
        for key in [
            format!("__struct_{base}_{}", segments[0]),
            format!("{base}_{}", segments[0]),
        ] {
            variables.insert(key, subtree);
        }
    }
    rebind_chan_payload(base, value, var_types);
    Ok(Some(written))
}

pub(crate) fn deep_aggregate_field_slot(
    module_env: &ModuleEnv,
    parent_type: &str,
    field: &str,
    var: &str,
) -> MumeiResult<Option<(u32, String, bool)>> {
    let parent_type = module_env.resolve_base_type(parent_type);
    if let Some(sdef) = module_env.get_struct(&parent_type) {
        let Some((index, declared_type)) = sdef
            .fields
            .iter()
            .enumerate()
            .find(|(_, candidate)| candidate.name == field)
            .map(|(index, candidate)| (index as u32, candidate.type_name.clone()))
        else {
            return Ok(None);
        };
        return Ok(Some((index, declared_type, false)));
    }
    let Some(edef) = module_env.get_enum(&parent_type) else {
        return Ok(None);
    };
    let mut owner = None;
    for variant in &edef.variants {
        let Some(index) = variant.fields.iter().position(|decl| {
            decl.split_once(':')
                .is_some_and(|(name, _)| name.trim() == field)
        }) else {
            continue;
        };
        if owner.is_some() {
            return Err(MumeiError::codegen(format!(
                "enum field path '{var}' has an ambiguous payload owner"
            )));
        }
        let Some(declared_type) = variant.field_types.get(index) else {
            return Err(MumeiError::codegen(format!(
                "enum payload '{field}' has no declared type"
            )));
        };
        owner = Some((index, declared_type.name.clone()));
    }
    let Some((index, declared_type)) = owner else {
        return Err(MumeiError::codegen(format!(
            "enum field path '{var}' has no unique named payload owner"
        )));
    };
    Ok(Some((1 + index as u32, declared_type, true)))
}

pub(crate) fn deep_slot_type_error(
    module_env: &ModuleEnv,
    parent_type: &str,
    declared_type: &str,
    index: u32,
    enum_payload_slot: bool,
) -> MumeiError {
    if enum_payload_slot {
        let parent_type = module_env.resolve_base_type(parent_type);
        let declared_type = module_env.resolve_base_type(declared_type);
        let slot = index.saturating_sub(1);
        let declared_kind = if module_env.get_struct(&declared_type).is_some() {
            "struct"
        } else if module_env.get_enum(&declared_type).is_some() {
            "enum"
        } else {
            "payload"
        };
        MumeiError::codegen(format!(
            "enum '{parent_type}' payload slot {slot} does not match its declared {declared_kind} type"
        ))
    } else {
        MumeiError::codegen(format!(
            "aggregate field in deep path does not match its declared type (index {index})"
        ))
    }
}

/// P25: a binding that aliases a `chan<T>` handle inherits the channel's
/// declared payload type; any other binding drops a stale one.
fn rebind_chan_payload(
    var: &str,
    value: &mumei_core::hir::HirExpr,
    var_types: &mut HashMap<String, String>,
) {
    match chan_payload_type_name(value, var_types) {
        Some(payload) => {
            var_types.insert(chan_payload_key(var), payload);
        }
        None => {
            var_types.remove(&chan_payload_key(var));
        }
    }
}

/// `let/assign h = if c { f } else { g }` where every branch tail resolves
/// to a lambda binding: lift a `__lamsel_*` dispatcher whose first param is
/// the branch index computed at the binding site, and mark `h` so calls
/// route through it. Returns `Ok(None)` when any leaf isn't lambda-shaped —
/// the caller then takes the generic path and `h` stays uncallable.
#[allow(clippy::too_many_arguments)]
fn bind_lambda_selector<'a>(
    context: &'a Context,
    builder: &Builder<'a>,
    module: &Module<'a>,
    function: &FunctionValue<'a>,
    var: &str,
    value: &HirExpr,
    variables: &mut HashMap<String, BasicValueEnum<'a>>,
    var_types: &mut HashMap<String, String>,
    array_ptrs: &mut ArrayPtrMap<'a>,
    module_env: &ModuleEnv,
) -> MumeiResult<Option<BasicValueEnum<'a>>> {
    let Some(branches) = super::expr_emit::collect_lambda_branches(
        context,
        module,
        function,
        value,
        Vec::new(),
        variables,
        var_types,
        array_ptrs,
        module_env,
        None,
    )?
    else {
        return Ok(None);
    };
    let selector_branches: Vec<(String, Option<super::expr_emit::SelScope<'a>>)> = branches
        .iter()
        .map(|(_, mark, scope)| (mark.clone(), scope.clone()))
        .collect();
    // The selector index is frozen at the binding site: fold the branch
    // conditions back-to-front into a nested `select` so a later rebind of a
    // cond variable cannot re-pick the branch.
    let n = branches.len();
    let mut sel: BasicValueEnum = context.i64_type().const_int((n - 1) as u64, false).into();
    for i in (0..n - 1).rev() {
        let mut taken = context.bool_type().const_int(1, false);
        for cond in &branches[i].0 {
            let c_bool = match cond {
                super::expr_emit::SelCond::Truthy(e, scope) => {
                    let c = if let Some(scope) = scope {
                        let mut scope_variables = scope.0.clone();
                        let mut scope_var_types = scope.1.clone();
                        compile_hir_expr(
                            context,
                            builder,
                            module,
                            function,
                            e,
                            &mut scope_variables,
                            &mut scope_var_types,
                            array_ptrs,
                            module_env,
                        )?
                    } else {
                        compile_hir_expr(
                            context, builder, module, function, e, variables, var_types,
                            array_ptrs, module_env,
                        )?
                    };
                    builder
                        .build_int_compare(
                            IntPredicate::NE,
                            c.into_int_value(),
                            context.i64_type().const_int(0, false),
                            "sel_cond",
                        )
                        .map_err(|e| {
                            MumeiError::codegen(format!("lambda sel cond failed: {e:?}"))
                        })?
                }
                super::expr_emit::SelCond::MatchEq(t, lit, scope) => {
                    let tv = if let Some(scope) = scope {
                        let mut scope_variables = scope.0.clone();
                        let mut scope_var_types = scope.1.clone();
                        compile_hir_expr(
                            context,
                            builder,
                            module,
                            function,
                            t,
                            &mut scope_variables,
                            &mut scope_var_types,
                            array_ptrs,
                            module_env,
                        )?
                    } else {
                        compile_hir_expr(
                            context, builder, module, function, t, variables, var_types,
                            array_ptrs, module_env,
                        )?
                    };
                    // Literal patterns only compare integer targets — a
                    // non-i64 scrutinee (f64, Str) can't form this selector,
                    // so fall back to the generic path instead of panicking.
                    if !tv.is_int_value() {
                        return Ok(None);
                    }
                    builder
                        .build_int_compare(
                            IntPredicate::EQ,
                            tv.into_int_value(),
                            context.i64_type().const_int(*lit as u64, true),
                            "sel_eq",
                        )
                        .map_err(|e| {
                            MumeiError::codegen(format!("lambda sel match cond failed: {e:?}"))
                        })?
                }
            };
            taken = builder
                .build_and(taken, c_bool, "sel_and")
                .map_err(|e| MumeiError::codegen(format!("lambda sel and failed: {e:?}")))?;
        }
        sel = builder
            .build_select(
                taken,
                context.i64_type().const_int(i as u64, false),
                sel.into_int_value(),
                "sel_pick",
            )
            .map_err(|e| MumeiError::codegen(format!("lambda sel select failed: {e:?}")))?;
    }
    let Some((fn_name, all_caps)) = super::expr_emit::emit_lambda_selector(
        context,
        module,
        function,
        var,
        &selector_branches,
        variables,
        var_types,
        array_ptrs,
        module_env,
    )?
    else {
        return Ok(None);
    };
    let lam_fn = module.get_function(&fn_name).unwrap();
    let ptr: BasicValueEnum = lam_fn.as_global_value().as_pointer_value().into();
    // `all_caps[0]` is the per-binding sel name the dispatcher minted —
    // unique across rebindings so a nested `_ => m` leaf keeps resolving
    // to the older selector's frozen index.
    let sel_name = all_caps[0].clone();
    variables.insert(sel_name, sel);
    variables.insert(var.to_string(), ptr);
    var_types.insert(
        var.to_string(),
        super::expr_emit::lambda_marker(&fn_name, &all_caps),
    );
    Ok(Some(ptr))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn compile_hir_stmt<'a>(
    context: &'a Context,
    builder: &Builder<'a>,
    module: &Module<'a>,
    function: &FunctionValue<'a>,
    stmt: &HirStmt,
    variables: &mut HashMap<String, BasicValueEnum<'a>>,
    var_types: &mut HashMap<String, String>,
    array_ptrs: &mut ArrayPtrMap<'a>,
    module_env: &ModuleEnv,
) -> MumeiResult<BasicValueEnum<'a>> {
    match stmt {
        HirStmt::Let { var, ty, value } => {
            // `let h = if c { f } else { g }` / `let m = match t { … => f }`
            // where every branch resolves to a lambda: emit a selector
            // dispatcher `__lamsel_*` and bind the name so `h(args)` calls
            // the branch the let-site condition picked.
            if matches!(
                value.as_ref(),
                HirExpr::IfThenElse { .. } | HirExpr::Match { .. }
            ) {
                if let Some(ptr) = bind_lambda_selector(
                    context, builder, module, function, var, value, variables, var_types,
                    array_ptrs, module_env,
                )? {
                    return Ok(ptr);
                }
            }
            // `let f = |params| body` — lift the lambda to a private
            // module-level function and mark `f` so `f(args)` /
            // `call(f, args)` resolve it. Captures pass as leading args.
            if let HirExpr::Lambda {
                params,
                return_type,
                body,
                captures,
            } = value.as_ref()
            {
                let (fn_name, all_caps) = super::expr_emit::emit_lambda_function(
                    context,
                    module,
                    function,
                    var,
                    params,
                    return_type.as_ref(),
                    captures,
                    body,
                    variables,
                    var_types,
                    array_ptrs,
                    module_env,
                )?;
                let lam_fn = module.get_function(&fn_name).unwrap();
                let ptr: BasicValueEnum = lam_fn.as_global_value().as_pointer_value().into();
                variables.insert(var.clone(), ptr);
                var_types.insert(
                    var.clone(),
                    super::expr_emit::lambda_marker(&fn_name, &all_caps),
                );
                return Ok(ptr);
            }
            // `let a = [e0, e1, …]` — materialise the literal into an alloca'd
            // array and register it as a fat pointer `(len, elem_ty, data_ptr)`
            // exactly like an `[T]` parameter, so `a[i]` reads, `a[i] = x`
            // stores, and `len(a)` share the existing array_ptrs machinery.
            if let HirExpr::ArrayLit(elements) = value.as_ref() {
                let (len_val, elem_ty, data_ptr) = emit_array_literal(
                    context, builder, module, function, elements, variables, var_types, array_ptrs,
                    module_env,
                )?;
                array_ptrs.insert(var.clone(), (len_val, elem_ty, data_ptr));
                array_ptrs.stack_backed.insert(var.clone());
                variables.insert(var.clone(), len_val);
                let array_type = ty.clone().or_else(|| {
                    elements
                        .first()
                        .and_then(|element| {
                            super::expr_emit::infer_struct_type_name(element, var_types, module_env)
                        })
                        .filter(|element_type| {
                            module_env
                                .get_enum(&super::expr_emit::resolve_named_type(
                                    module_env,
                                    element_type,
                                ))
                                .is_some()
                                || module_env
                                    .get_struct(&super::expr_emit::resolve_named_type(
                                        module_env,
                                        element_type,
                                    ))
                                    .is_some()
                        })
                        .map(|element_type| format!("[{element_type}]"))
                });
                if let Some(array_type) = array_type {
                    var_types.insert(var.clone(), array_type);
                }
                if var_types
                    .get(var.as_str())
                    .is_some_and(|mark| mark.starts_with(LAMBDA_MARK))
                {
                    var_types.remove(var.as_str());
                }
                return Ok(len_val);
            }
            // `let s = "lit"` (`pure-i64-composite/v13`): the `*i8` global
            // stays the bound value (concat/extern uses are unchanged) and
            // a read-only fat pointer `(len, i8, data)` registers beside
            // it so `s[i]`/`len(s)` emit the same bounds-checked byte read
            // the verifier's bounded-memory model encodes.
            if let HirExpr::StringLit(s) = value.as_ref() {
                let global = builder.build_global_string_ptr(s, "str_lit").map_err(|e| {
                    MumeiError::codegen(format!("Failed to build string literal: {:?}", e))
                })?;
                let data_ptr: BasicValueEnum = global.as_pointer_value().into();
                let len_val: BasicValueEnum =
                    context.i64_type().const_int(s.len() as u64, false).into();
                array_ptrs.insert(var.clone(), (len_val, context.i8_type().into(), data_ptr));
                array_ptrs.stack_backed.remove(var);
                variables.insert(var.clone(), data_ptr);
                return Ok(data_ptr);
            }
            if let HirExpr::FieldAccess(inner, field_name) = value.as_ref() {
                if let Some(enum_name) = (field_name != "tag")
                    .then(|| infer_struct_type_name(inner, var_types, module_env))
                    .flatten()
                {
                    let enum_name = module_env.resolve_base_type(&enum_name);
                    if let Some(enum_def) = module_env.get_enum(&enum_name) {
                        let mut field_type = None;
                        for variant in &enum_def.variants {
                            if let Some(index) = variant.fields.iter().position(|decl| {
                                decl.split_once(':')
                                    .is_some_and(|(name, _)| name.trim() == field_name)
                            }) {
                                if field_type.is_some() {
                                    return Err(MumeiError::codegen(format!(
                                        "enum field read '{field_name}' has an ambiguous payload owner"
                                    )));
                                }
                                field_type = variant
                                    .field_types
                                    .get(index)
                                    .map(|field| field.display_name());
                            }
                        }
                        if let Some(field_type) = field_type {
                            if let Some(elem_ty) =
                                array_elem_llvm_type(context, &field_type, module_env)?
                            {
                                let source_stack_backed = match inner.as_ref() {
                                    HirExpr::Variable(name) => {
                                        array_ptrs.stack_backed.contains(name)
                                    }
                                    _ => true,
                                };
                                let field_value = compile_hir_expr(
                                    context, builder, module, function, value, variables,
                                    var_types, array_ptrs, module_env,
                                )?;
                                let BasicValueEnum::StructValue(fat_ptr) = field_value else {
                                    return Err(MumeiError::codegen(format!(
                                        "array payload field '{field_name}' is not a fat-pointer aggregate"
                                    )));
                                };
                                let len = llvm!(builder.build_extract_value(
                                    fat_ptr,
                                    0,
                                    &format!("{var}_len")
                                ));
                                let data = llvm!(builder.build_extract_value(
                                    fat_ptr,
                                    1,
                                    &format!("{var}_data")
                                ));
                                array_ptrs.insert(var.clone(), (len, elem_ty, data));
                                if source_stack_backed {
                                    array_ptrs.stack_backed.insert(var.clone());
                                } else {
                                    array_ptrs.stack_backed.remove(var);
                                }
                                variables.insert(var.clone(), len);
                                var_types.insert(var.clone(), field_type);
                                return Ok(len);
                            }
                        }
                    }
                }
            }
            // `let a = arr` — the binding aliases the tracked fat pointer so
            // `a[i]`/`len(a)`/`a[i] = x` hit the source's slots.
            if let HirExpr::Variable(src) = value.as_ref() {
                if let Some(&(len_val, elem_ty, data_ptr)) = array_ptrs.get(src.as_str()) {
                    let stack_backed = array_ptrs.stack_backed.contains(src);
                    array_ptrs.insert(var.clone(), (len_val, elem_ty, data_ptr));
                    if stack_backed {
                        array_ptrs.stack_backed.insert(var.clone());
                    } else {
                        array_ptrs.stack_backed.remove(var);
                    }
                    if let Some(array_type) = var_types.get(src.as_str()).cloned() {
                        if array_type.starts_with('[') || array_type.starts_with("[]<") {
                            var_types.insert(var.clone(), array_type);
                        }
                    }
                    // A `Str` (`pure-i64-composite/v13`, `i8` cells) keeps
                    // its `*i8` pointer as the bound value — concat and
                    // extern uses need it — while integer arrays expose
                    // their length in scalar position.
                    variables.insert(
                        var.clone(),
                        if elem_ty == context.i8_type().into() {
                            data_ptr
                        } else {
                            len_val
                        },
                    );
                    if var_types
                        .get(var.as_str())
                        .is_some_and(|mark| mark.starts_with(LAMBDA_MARK))
                    {
                        var_types.remove(var.as_str());
                    }
                    return Ok(if elem_ty == context.i8_type().into() {
                        data_ptr
                    } else {
                        len_val
                    });
                }
                // `let g = f` on a lambda binding: alias the same lifted fn
                // (same captures) by copying the `@lam:` marker.
                if let Some(mark) = var_types.get(src.as_str()) {
                    if mark.starts_with(super::expr_emit::LAMBDA_MARK) {
                        let mark = mark.clone();
                        let ptr = variables
                            .get(src.as_str())
                            .copied()
                            .unwrap_or_else(|| context.i64_type().const_int(0, false).into());
                        variables.insert(var.clone(), ptr);
                        var_types.insert(var.clone(), mark);
                        return Ok(ptr);
                    }
                }
            }
            let val = compile_hir_expr(
                context, builder, module, function, value, variables, var_types, array_ptrs,
                module_env,
            )?;
            let stack_backed_enum =
                enum_value_is_stack_backed(value, var_types, array_ptrs, module_env)?;
            // `let x: i128 = …` (`pure-i64-composite/v21`): the declared
            // 128-bit leaf binds a native `i128` — a literal or narrower
            // RHS zero-extends into the slot.
            let declared_base = ty
                .as_deref()
                .map(|ty_name| resolve_named_type(module_env, ty_name));
            if declared_base
                .as_deref()
                .is_some_and(|base| base.starts_with('[') || base.starts_with("[]<"))
            {
                var_types.insert(var.clone(), declared_base.clone().unwrap());
            }
            let val = if matches!(declared_base.as_deref(), Some("i128") | Some("u128")) {
                wide_int_cast(
                    builder,
                    val,
                    context.i128_type().into(),
                    declared_base.as_deref() == Some("i128"),
                )?
            } else {
                val
            };
            array_ptrs.remove(var.as_str());
            array_ptrs.stack_backed.remove(var.as_str());
            if stack_backed_enum {
                array_ptrs.stack_backed.insert(var.clone());
            }
            // `let r = <expr>` where the expression yields `[T]` (a call,
            // an `if`/`match` producing a fat pointer, or an array
            // binding): the `{i64, ptr}` is unpacked into `array_ptrs` so
            // `r[i]`/`len(r)` work like any other array binding. Bound
            // arrays always own a fresh heap copy — writes through the
            // binding must never reach the source's storage.
            if let Some(ret_base) = expr_array_type_name(value, var_types, module_env) {
                // A `Variable` tail resolves through the source binding's
                // `array_ptrs` slots; other array expressions unpack the
                // fat `{i64, ptr}` they produced.
                let source = match value.as_ref() {
                    HirExpr::Variable(name) => array_ptrs.get(name.as_str()).copied(),
                    _ => match val {
                        BasicValueEnum::StructValue(fat_ptr) => {
                            let len =
                                llvm!(builder.build_extract_value(fat_ptr, 0, "call_arr_len"));
                            let data =
                                llvm!(builder.build_extract_value(fat_ptr, 1, "call_arr_data"));
                            array_elem_llvm_type(context, &ret_base, module_env)?
                                .map(|elem_ty| (len, elem_ty, data))
                        }
                        _ => None,
                    },
                };
                if let Some((len, elem_ty, data)) = source {
                    // The verifier treats each binding as an independent
                    // array value — writes through `var` must never reach
                    // the callee's buffer or an aliased source — so bound
                    // arrays always own a fresh heap copy and are never
                    // stack-backed.
                    let heap = copy_array_to_heap(context, builder, module, len, elem_ty, data)?;
                    array_ptrs.insert(var.clone(), (len, elem_ty, heap));
                    var_types.insert(var.clone(), ret_base);
                    variables.insert(var.clone(), len);
                    return Ok(len);
                }
            }
            variables.insert(var.clone(), val);
            let inferred_ty = declared_base
                .clone()
                .filter(|base| {
                    module_env.get_struct(base).is_some() || module_env.get_enum(base).is_some()
                })
                .or_else(|| infer_struct_type_name(value, var_types, module_env));
            if let Some(struct_ty) = inferred_ty {
                var_types.insert(var.clone(), struct_ty);
            } else {
                // Record the scalar's declared/inferred integer type too: a
                // `let t = <u64 expr>` keeps `u64` so later `t / 2` lowers to
                // `udiv` (mirrors the verifier's `record_binding_int_type`).
                let int_ty = declared_base
                    .filter(|base| is_fixed_int_type_name(base))
                    .or_else(|| hir_int_type_name(value, var_types, module_env))
                    .filter(|base| is_fixed_int_type_name(base));
                match int_ty {
                    Some(int_ty) => {
                        var_types.insert(var.clone(), int_ty);
                    }
                    // Clear any stale type from a prior binding of this name
                    // so a later field access cannot resolve against the
                    // wrong struct.
                    None => {
                        var_types.remove(var);
                    }
                }
            }
            rebind_chan_payload(var, value, var_types);
            Ok(val)
        }
        HirStmt::Assign { var, value } => {
            if let Some((base, field)) = var.split_once('.') {
                if let Some(struct_ty) = var_types.get(base).cloned() {
                    // The write path's index list: each segment resolves
                    // on the declared struct it descends into — no
                    // every-struct scan, so `q.inner.a` cannot land on
                    // an unrelated type's same-named field.
                    let segments: Vec<&str> = field.split('.').collect();
                    if segments.len() > 1 {
                        if let Some(written) = emit_deep_aggregate_field_write(
                            context, builder, module, function, var, base, &struct_ty, &segments,
                            value, variables, var_types, array_ptrs, module_env,
                        )? {
                            return Ok(written);
                        }
                    }
                    let mut index_path: Vec<u32> = Vec::with_capacity(segments.len());
                    let mut level_ty: &str = struct_ty.as_str();
                    let mut resolved = segments.len() > 1;
                    if segments.len() == 1 {
                        // An enum-typed base skips the name-only
                        // fallback: a same-named field on an
                        // unrelated struct would resolve a payload
                        // write to the wrong aggregate slot.
                        if module_env.get_enum(&struct_ty).is_none() {
                            if let Some(idx) = find_field_index(&struct_ty, field, module_env) {
                                index_path.push(idx);
                                resolved = true;
                            }
                        }
                    } else {
                        for segment in &segments {
                            let Some(sdef) = module_env.get_struct(level_ty) else {
                                resolved = false;
                                break;
                            };
                            let Some(pos) = sdef.fields.iter().position(|f| f.name == *segment)
                            else {
                                resolved = false;
                                break;
                            };
                            index_path.push(pos as u32);
                            level_ty = sdef.fields[pos].type_name.as_str();
                        }
                    }
                    if resolved {
                        let val = compile_hir_expr(
                            context, builder, module, function, value, variables, var_types,
                            array_ptrs, module_env,
                        )?;
                        // A 128-bit leaf field (`pure-i64-composite/v21`)
                        // takes a native `i128` — a literal or narrower
                        // written value zero-extends into the slot.
                        let leaf_type_name = if segments.len() == 1 {
                            module_env
                                .get_struct(&struct_ty)
                                .and_then(|sd| {
                                    index_path.first().and_then(|&i| sd.fields.get(i as usize))
                                })
                                .map(|f| f.type_name.as_str())
                        } else {
                            Some(level_ty)
                        };
                        let val = match leaf_type_name.map(|t| resolve_named_type(module_env, t)) {
                            Some(base) if base == "i128" || base == "u128" => wide_int_cast(
                                builder,
                                val,
                                context.i128_type().into(),
                                base == "i128",
                            )?,
                            _ => val,
                        };
                        let old = variables.get(base).copied().ok_or_else(|| {
                            MumeiError::codegen(format!(
                                "struct binding '{base}' has no aggregate value"
                            ))
                        })?;
                        // Rebuild the insertvalue chain bottom-up:
                        // extract each ancestor aggregate, replace the
                        // leaf, then wrap each level back — the same
                        // `insertvalue` path the source model's nested
                        // composite update mirrors.
                        let mut ancestors: Vec<inkwell::values::StructValue> =
                            vec![old.into_struct_value()];
                        for &idx in &index_path[..index_path.len() - 1] {
                            let sub = llvm!(builder.build_extract_value(
                                *ancestors.last().unwrap(),
                                idx,
                                "nest"
                            ));
                            ancestors.push(sub.into_struct_value());
                        }
                        let mut cur: BasicValueEnum = val;
                        let mut first_subtree: Option<BasicValueEnum> = None;
                        for (level, &idx) in index_path.iter().enumerate().rev() {
                            cur =
                                llvm!(builder.build_insert_value(ancestors[level], cur, idx, var))
                                    .into_struct_value()
                                    .into();
                            if level == 1 {
                                first_subtree = Some(cur);
                            }
                        }
                        variables.insert(base.to_string(), cur);
                        if let Some(subtree) = first_subtree {
                            for key in [
                                format!("__struct_{base}_{}", segments[0]),
                                format!("{base}_{}", segments[0]),
                            ] {
                                variables.insert(key, subtree);
                            }
                        } else {
                            for key in [
                                format!("__struct_{base}_{field}"),
                                format!("{base}_{field}"),
                            ] {
                                variables.insert(key, val);
                            }
                        }
                        rebind_chan_payload(base, value, var_types);
                        return Ok(val);
                    }
                    // Enum payload writes use the receiver's declared enum
                    // and only named fields owned by exactly one variant.
                    if !resolved {
                        if let Some(edef) = module_env.get_enum(&struct_ty) {
                            let payload_field = segments[0];
                            let mut owner: Option<(usize, usize)> = None;
                            let mut ambiguous = false;
                            for (vidx, variant) in edef.variants.iter().enumerate() {
                                // `fields` keeps a named payload's
                                // verbatim `name: type` text — a
                                // positional payload has no name a
                                // write can address.
                                if let Some(fidx) = variant.fields.iter().position(|f| {
                                    f.split_once(':')
                                        .map(|(n, _)| n.trim() == payload_field)
                                        .unwrap_or(false)
                                }) {
                                    if owner.is_some() {
                                        ambiguous = true;
                                        break;
                                    }
                                    owner = Some((vidx, fidx));
                                }
                            }
                            if segments.len() > 1 && ambiguous {
                                return Err(MumeiError::codegen(format!(
                                    "enum field write '{var}' has an ambiguous payload owner"
                                )));
                            }
                            if let (Some((vidx, fidx)), false) = (owner, ambiguous) {
                                if segments.len() > 1 {
                                    let field_type = edef.variants[vidx]
                                        .field_types
                                        .get(fidx)
                                        .map(|field| field.name.as_str())
                                        .ok_or_else(|| {
                                            MumeiError::codegen(format!(
                                                "enum payload '{payload_field}' has no declared type"
                                            ))
                                        })?;
                                    let payload_type = module_env.resolve_base_type(field_type);
                                    let payload_sdef = module_env
                                        .get_struct(&payload_type)
                                        .ok_or_else(|| {
                                            MumeiError::codegen(format!(
                                                "deep enum payload write '{var}' requires a struct payload"
                                            ))
                                        })?;
                                    let old = variables.get(base).copied().ok_or_else(|| {
                                        MumeiError::codegen(format!(
                                            "enum binding '{base}' has no aggregate value"
                                        ))
                                    })?;
                                    let agg = old.into_struct_value();
                                    let payload_index = 1 + fidx as u32;
                                    let slot_ty = agg
                                        .get_type()
                                        .get_field_type_at_index(payload_index)
                                        .ok_or_else(|| {
                                            MumeiError::codegen(format!(
                                                "enum '{struct_ty}' has no payload slot {fidx}"
                                            ))
                                        })?;
                                    let declared_payload_ty: inkwell::types::BasicTypeEnum =
                                        struct_llvm_type(context, payload_sdef, module_env)?.into();
                                    if slot_ty != declared_payload_ty {
                                        return Err(MumeiError::codegen(format!(
                                            "enum '{struct_ty}' payload slot {fidx} does not match its declared struct type"
                                        )));
                                    }
                                    let payload = llvm!(builder.build_extract_value(
                                        agg,
                                        payload_index,
                                        "enum_payload"
                                    ))
                                    .into_struct_value();

                                    let mut level_type = payload_type.as_str();
                                    let mut index_path = Vec::with_capacity(segments.len() - 1);
                                    for segment in &segments[1..] {
                                        let resolved_type =
                                            module_env.resolve_base_type(level_type);
                                        let sdef =
                                            module_env.get_struct(&resolved_type).ok_or_else(|| {
                                                MumeiError::codegen(format!(
                                                    "enum payload path '{var}' crosses a non-struct field"
                                                ))
                                            })?;
                                        let field_index = sdef
                                            .fields
                                            .iter()
                                            .position(|field| field.name == *segment)
                                            .ok_or_else(|| {
                                                MumeiError::codegen(format!(
                                                    "enum payload path '{var}' has no field '{segment}'"
                                                ))
                                            })?;
                                        index_path.push(field_index as u32);
                                        level_type = sdef.fields[field_index].type_name.as_str();
                                    }

                                    let val = compile_hir_expr(
                                        context, builder, module, function, value, variables,
                                        var_types, array_ptrs, module_env,
                                    )?;
                                    let expected_ty =
                                        resolve_param_type(context, Some(level_type), module_env)?;
                                    let leaf_type = module_env.resolve_base_type(level_type);
                                    let val = if leaf_type == "i128" || leaf_type == "u128" {
                                        wide_int_cast(
                                            builder,
                                            val,
                                            expected_ty,
                                            leaf_type == "i128",
                                        )?
                                    } else {
                                        val
                                    };
                                    if val.get_type() != expected_ty {
                                        return Err(MumeiError::codegen(format!(
                                            "enum payload path '{var}' expects value type {expected_ty}, got {}",
                                            val.get_type()
                                        )));
                                    }

                                    let mut ancestors: Vec<inkwell::values::StructValue> =
                                        vec![payload];
                                    for &index in &index_path[..index_path.len() - 1] {
                                        let nested = llvm!(builder.build_extract_value(
                                            *ancestors.last().unwrap(),
                                            index,
                                            "enum_payload_nested"
                                        ));
                                        ancestors.push(nested.into_struct_value());
                                    }
                                    let mut rebuilt: BasicValueEnum = val;
                                    for (level, &index) in index_path.iter().enumerate().rev() {
                                        rebuilt = llvm!(builder.build_insert_value(
                                            ancestors[level],
                                            rebuilt,
                                            index,
                                            "enum_payload_update"
                                        ))
                                        .into_struct_value()
                                        .into();
                                    }
                                    let updated = llvm!(builder.build_insert_value(
                                        agg,
                                        rebuilt,
                                        payload_index,
                                        var
                                    ));
                                    variables.insert(
                                        base.to_string(),
                                        updated.into_struct_value().into(),
                                    );
                                    return Ok(val);
                                }
                                let field_type = edef.variants[vidx]
                                    .field_types
                                    .get(fidx)
                                    .map(|field| field.display_name())
                                    .ok_or_else(|| {
                                        MumeiError::codegen(format!(
                                            "enum payload '{payload_field}' has no declared type"
                                        ))
                                    })?;
                                let stack_backed_array =
                                    if array_elem_llvm_type(context, &field_type, module_env)?
                                        .is_some()
                                    {
                                        match value.as_ref() {
                                            HirExpr::Variable(name) => {
                                                array_ptrs.stack_backed.contains(name)
                                                    || !array_ptrs.contains_key(name)
                                            }
                                            _ => true,
                                        }
                                    } else {
                                        false
                                    };
                                if stack_backed_array {
                                    return Err(MumeiError::codegen(format!(
                                        "stack-backed array payload cannot be written into enum field '{var}'"
                                    )));
                                }
                                let val = compile_hir_expr(
                                    context, builder, module, function, value, variables,
                                    var_types, array_ptrs, module_env,
                                )?;
                                let old = variables.get(base).copied().ok_or_else(|| {
                                    MumeiError::codegen(format!(
                                        "enum binding '{base}' has no aggregate value"
                                    ))
                                })?;
                                // The union slot's LLVM type is the
                                // widest across variants — coerce
                                // the written value the same way the
                                // constructor does, or an i64 write
                                // into an f64 slot mismatches.
                                let agg = old.into_struct_value();
                                let Some(slot_ty) =
                                    agg.get_type().get_field_type_at_index(1 + fidx as u32)
                                else {
                                    return Err(MumeiError::codegen(format!(
                                        "enum '{struct_ty}' has no payload slot {fidx}"
                                    )));
                                };
                                let coerced = bitpreserve_cast(builder, val, slot_ty)?;
                                let cur = llvm!(builder.build_insert_value(
                                    agg,
                                    coerced,
                                    1 + fidx as u32,
                                    var
                                ));
                                variables.insert(base.to_string(), cur.into_struct_value().into());
                                return Ok(val);
                            }
                            if segments.len() > 1 {
                                return Err(MumeiError::codegen(format!(
                                    "enum field write '{var}' has no unique named payload owner"
                                )));
                            }
                        }
                    }
                }
            }
            if let Some((resource, field)) = module_env.is_resource_state_var(var) {
                let val = compile_hir_expr(
                    context, builder, module, function, value, variables, var_types, array_ptrs,
                    module_env,
                )?;
                let global_name = format!("__mumei_res_{}_{}", resource.name, field.name);
                let global = module.get_global(&global_name).ok_or_else(|| {
                    MumeiError::codegen(format!("missing resource state global '{global_name}'"))
                })?;
                llvm!(builder.build_store(global.as_pointer_value(), val));
                return Ok(val);
            }
            // `h = if c { f } else { g }` / `m = match …` — same selector
            // binding as `let`.
            if matches!(
                value.as_ref(),
                HirExpr::IfThenElse { .. } | HirExpr::Match { .. }
            ) {
                if let Some(ptr) = bind_lambda_selector(
                    context, builder, module, function, var, value, variables, var_types,
                    array_ptrs, module_env,
                )? {
                    return Ok(ptr);
                }
            }
            // `f = |params| body` — rebind the name to a freshly lifted fn,
            // replacing any earlier `@lam:` marker like the verifier's
            // `local_lambdas` rebind does.
            if let HirExpr::Lambda {
                params,
                return_type,
                body,
                captures,
            } = value.as_ref()
            {
                let (fn_name, all_caps) = super::expr_emit::emit_lambda_function(
                    context,
                    module,
                    function,
                    var,
                    params,
                    return_type.as_ref(),
                    captures,
                    body,
                    variables,
                    var_types,
                    array_ptrs,
                    module_env,
                )?;
                let lam_fn = module.get_function(&fn_name).unwrap();
                let ptr: BasicValueEnum = lam_fn.as_global_value().as_pointer_value().into();
                variables.insert(var.clone(), ptr);
                var_types.insert(
                    var.clone(),
                    super::expr_emit::lambda_marker(&fn_name, &all_caps),
                );
                return Ok(ptr);
            }
            if let HirExpr::ArrayLit(elements) = value.as_ref() {
                let (len_val, elem_ty, data_ptr) = emit_array_literal(
                    context, builder, module, function, elements, variables, var_types, array_ptrs,
                    module_env,
                )?;
                array_ptrs.insert(var.clone(), (len_val, elem_ty, data_ptr));
                array_ptrs.stack_backed.insert(var.clone());
                variables.insert(var.clone(), len_val);
                if var_types
                    .get(var.as_str())
                    .is_some_and(|mark| mark.starts_with(LAMBDA_MARK))
                {
                    var_types.remove(var.as_str());
                }
                return Ok(len_val);
            }
            // `s = "lit"` on a `Str` binding rebinds the byte sequence —
            // the composite fragment's immutability gate rejects the
            // statement upstream, so this path only serves general
            // builds, which allowed `let s = "a"; s = "b"` before v13
            // registered the fat pointer.
            if let HirExpr::StringLit(lit) = value.as_ref() {
                if let Some(&(_, elem_ty, _)) = array_ptrs.get(var.as_str()) {
                    if elem_ty == context.i8_type().into() {
                        let global =
                            builder
                                .build_global_string_ptr(lit, "str_lit")
                                .map_err(|e| {
                                    MumeiError::codegen(format!(
                                        "Failed to build string literal: {:?}",
                                        e
                                    ))
                                })?;
                        let data_ptr: BasicValueEnum = global.as_pointer_value().into();
                        let len_val: BasicValueEnum =
                            context.i64_type().const_int(lit.len() as u64, false).into();
                        array_ptrs
                            .insert(var.clone(), (len_val, context.i8_type().into(), data_ptr));
                        array_ptrs.stack_backed.remove(var);
                        variables.insert(var.clone(), data_ptr);
                        return Ok(data_ptr);
                    }
                }
            }
            if let HirExpr::Variable(src) = value.as_ref() {
                if let Some(&(len_val, elem_ty, data_ptr)) = array_ptrs.get(src.as_str()) {
                    let stack_backed = array_ptrs.stack_backed.contains(src);
                    array_ptrs.insert(var.clone(), (len_val, elem_ty, data_ptr));
                    if stack_backed {
                        array_ptrs.stack_backed.insert(var.clone());
                    } else {
                        array_ptrs.stack_backed.remove(var);
                    }
                    // `i8` elements mark a `Str` binding: its scalar value
                    // is the `*i8` pointer (concat/extern uses), not the
                    // length integer arrays expose.
                    let scalar = if elem_ty == context.i8_type().into() {
                        data_ptr
                    } else {
                        len_val
                    };
                    variables.insert(var.clone(), scalar);
                    if var_types
                        .get(var.as_str())
                        .is_some_and(|mark| mark.starts_with(LAMBDA_MARK))
                    {
                        var_types.remove(var.as_str());
                    }
                    return Ok(scalar);
                }
            }
            // `t = <expr>` on an array binding where the expression
            // yields `[T]` (call, `if`/`match` producing a fat pointer) —
            // same fat-pointer unpack as the `let` path. The rebound
            // copy is always heap-owned, so `stack_backed` is cleared.
            if let Some(ret_base) = expr_array_type_name(value, var_types, module_env) {
                // A `Variable` tail resolves through the source binding's
                // `array_ptrs` slots; other array expressions unpack the
                // fat `{i64, ptr}` they produced.
                let source = match value.as_ref() {
                    HirExpr::Variable(name) => array_ptrs.get(name.as_str()).copied(),
                    _ => match compile_hir_expr(
                        context, builder, module, function, value, variables, var_types,
                        array_ptrs, module_env,
                    )? {
                        BasicValueEnum::StructValue(fat_ptr) => {
                            let len =
                                llvm!(builder.build_extract_value(fat_ptr, 0, "call_arr_len"));
                            let data =
                                llvm!(builder.build_extract_value(fat_ptr, 1, "call_arr_data"));
                            array_elem_llvm_type(context, &ret_base, module_env)?
                                .map(|elem_ty| (len, elem_ty, data))
                        }
                        _ => None,
                    },
                };
                if let Some((len, elem_ty, data)) = source {
                    // Rebinding replaces the stored slots outright — and
                    // the bound copy is always heap-owned (writes through
                    // `var` must not alias the source), so the
                    // stack-backed flag is cleared.
                    array_ptrs.stack_backed.remove(var);
                    let heap = copy_array_to_heap(context, builder, module, len, elem_ty, data)?;
                    array_ptrs.insert(var.clone(), (len, elem_ty, heap));
                    var_types.insert(var.clone(), ret_base);
                    variables.insert(var.clone(), len);
                    return Ok(len);
                }
            }
            let was_str = matches!(
                array_ptrs.get(var.as_str()),
                Some(&(_, elem_ty, _)) if elem_ty == context.i8_type().into()
            );
            if array_ptrs.contains_key(var.as_str()) && !was_str {
                return Err(MumeiError::codegen(format!(
                    "array '{var}' can only be reassigned from an array literal, another array binding, or an array call result"
                )));
            }
            let val = compile_hir_expr(
                context, builder, module, function, value, variables, var_types, array_ptrs,
                module_env,
            )?;
            let stack_backed_enum =
                enum_value_is_stack_backed(value, var_types, array_ptrs, module_env)?;
            if was_str {
                // A `Str` binding reassigned to a non-array value keeps
                // its pre-v13 emit semantics; drop the stale fat pointer
                // so a later `s[i]` cannot read the old bytes. The drop
                // must follow the RHS: a `s = f(...)` call whose lambda
                // captures `s` packs the capture from `array_ptrs`.
                array_ptrs.remove(var.as_str());
                array_ptrs.stack_backed.remove(var.as_str());
            }
            // `x = …` on a declared 128-bit binding widens the RHS the
            // same way `let` does (`pure-i64-composite/v21`).
            let val = match var_types
                .get(var.as_str())
                .map(|t| resolve_named_type(module_env, t))
            {
                Some(base) if base == "i128" || base == "u128" => {
                    wide_int_cast(builder, val, context.i128_type().into(), base == "i128")?
                }
                _ => val,
            };
            variables.insert(var.clone(), val);
            array_ptrs.stack_backed.remove(var.as_str());
            if stack_backed_enum {
                array_ptrs.stack_backed.insert(var.clone());
            }
            if let Some(struct_ty) = infer_struct_type_name(value, var_types, module_env) {
                var_types.insert(var.clone(), struct_ty);
            } else {
                // Same scalar tracking as `let`: `t = <u64 expr>` keeps `u64`
                // so later `t / 2` lowers to `udiv`.
                match hir_int_type_name(value, var_types, module_env)
                    .filter(|base| is_fixed_int_type_name(base))
                {
                    Some(int_ty) => {
                        var_types.insert(var.clone(), int_ty);
                    }
                    // Reassignment to a value with no known type must not
                    // leave a stale mapping from the previous binding.
                    None => {
                        var_types.remove(var);
                    }
                }
            }
            rebind_chan_payload(var, value, var_types);
            Ok(val)
        }
        HirStmt::ArrayStore {
            array,
            index,
            field_path,
            value,
        } => {
            let val = compile_hir_expr(
                context, builder, module, function, value, variables, var_types, array_ptrs,
                module_env,
            )?;
            let idx = compile_hir_expr(
                context, builder, module, function, index, variables, var_types, array_ptrs,
                module_env,
            )?
            .into_int_value();
            if let Some((len_val, elem_ty, data_ptr_val)) = array_ptrs.get(array.as_str()) {
                let data_ptr = data_ptr_val.into_pointer_value();
                let len_int = len_val.into_int_value();
                let (store_ty, field_indices) = if let Some(field_path) = field_path {
                    let element_type = infer_struct_type_name(
                        &HirExpr::ArrayAccess(array.clone(), index.clone()),
                        var_types,
                        module_env,
                    )
                    .ok_or_else(|| {
                        MumeiError::codegen(format!(
                            "Array '{}' field store requires a struct element",
                            array
                        ))
                    })?;
                    let mut current_type = element_type;
                    let mut current_llvm_type = *elem_ty;
                    let mut indices = Vec::with_capacity(field_path.len());
                    for field in field_path {
                        let resolved = module_env.resolve_base_type(&current_type);
                        let struct_def = module_env.get_struct(&resolved).ok_or_else(|| {
                            MumeiError::codegen(format!(
                                "Array '{}' field path contains non-struct type '{}'",
                                array, resolved
                            ))
                        })?;
                        let field_index = struct_def
                            .fields
                            .iter()
                            .position(|candidate| candidate.name == *field)
                            .ok_or_else(|| {
                                MumeiError::codegen(format!(
                                    "Array '{}' struct '{}' has no field '{}'",
                                    array, resolved, field
                                ))
                            })?;
                        let inkwell::types::BasicTypeEnum::StructType(struct_type) =
                            current_llvm_type
                        else {
                            return Err(MumeiError::codegen(format!(
                                "Array '{}' field path for '{}' does not match its LLVM struct layout",
                                array, field
                            )));
                        };
                        current_llvm_type = struct_type
                            .get_field_type_at_index(field_index as u32)
                            .ok_or_else(|| {
                                MumeiError::codegen(format!(
                                    "Array '{}' field '{}' is missing from its LLVM struct layout",
                                    array, field
                                ))
                            })?;
                        current_type = struct_def.fields[field_index].type_name.clone();
                        indices.push(field_index as u32);
                    }
                    (current_llvm_type, Some(indices))
                } else {
                    (*elem_ty, None)
                };

                // Guard the GEP + store with an in-bounds check that mirrors
                // ArrayAccess read-side handling. Out-of-bounds writes are
                // skipped rather than crashing the process.
                let in_bounds = llvm!(builder.build_int_compare(
                    IntPredicate::SLT,
                    idx,
                    len_int,
                    "store_bounds_check"
                ));
                let non_neg = llvm!(builder.build_int_compare(
                    IntPredicate::SGE,
                    idx,
                    context.i64_type().const_int(0, false),
                    "store_non_neg_check"
                ));
                let safe = llvm!(builder.build_and(in_bounds, non_neg, "store_safe"));

                let safe_block = context.append_basic_block(*function, "arr.store.safe");
                let merge_block = context.append_basic_block(*function, "arr.store.merge");
                llvm!(builder.build_conditional_branch(safe, safe_block, merge_block));

                builder.position_at_end(safe_block);
                // Coerce the stored value to the array's declared element
                // type — `arr[i] = 42` on a `[f64]` stores 42.0.
                let stored = match (&store_ty, val) {
                    (
                        inkwell::types::BasicTypeEnum::FloatType(ft),
                        BasicValueEnum::IntValue(iv),
                    ) => llvm!(builder.build_signed_int_to_float(iv, *ft, "store_i2f")).into(),
                    _ if store_ty == val.get_type() => val,
                    _ => crate::codegen::lowering::bitpreserve_cast(builder, val, store_ty)
                        .map_err(|e| {
                            MumeiError::codegen(format!(
                                "Array store value type mismatch on '{}': {}",
                                array, e
                            ))
                        })?,
                };
                let elem_ptr = unsafe {
                    llvm!(builder.build_gep(*elem_ty, data_ptr, &[idx], "store_elem_ptr"))
                };
                let store_ptr = if let Some(field_indices) = field_indices {
                    let mut field_ptr = elem_ptr;
                    let mut struct_type = elem_ty.into_struct_type();
                    for (position, field_index) in field_indices.iter().enumerate() {
                        field_ptr = llvm!(builder.build_struct_gep(
                            struct_type,
                            field_ptr,
                            *field_index,
                            "store_field_ptr"
                        ));
                        if position + 1 < field_indices.len() {
                            let inkwell::types::BasicTypeEnum::StructType(nested_type) = struct_type
                                .get_field_type_at_index(*field_index)
                                .ok_or_else(|| {
                                    MumeiError::codegen(format!(
                                        "Array '{}' field path does not match its LLVM struct layout",
                                        array
                                    ))
                                })?
                            else {
                                return Err(MumeiError::codegen(format!(
                                    "Array '{}' field path descends through a non-struct field",
                                    array
                                )));
                            };
                            struct_type = nested_type;
                        }
                    }
                    field_ptr
                } else {
                    elem_ptr
                };
                llvm!(builder.build_store(store_ptr, stored));
                llvm!(builder.build_unconditional_branch(merge_block));

                builder.position_at_end(merge_block);
                Ok(val)
            } else {
                Err(MumeiError::codegen(format!(
                    "Array '{}' not found as fat pointer parameter",
                    array
                )))
            }
        }
        HirStmt::While {
            cond,
            invariant: _,
            decreases: _,
            body,
        } => {
            let header_block = context.append_basic_block(*function, "loop.header");
            let cond_block = context.append_basic_block(*function, "loop.cond");
            let body_block = context.append_basic_block(*function, "loop.body");
            let after_block = context.append_basic_block(*function, "loop.after");

            let pre_loop_vars = variables.clone();
            let pre_loop_var_types = var_types.clone();
            let pre_loop_arrays = array_ptrs.clone();
            let entry_end_block = builder.get_insert_block().unwrap();

            llvm!(builder.build_unconditional_branch(header_block));

            builder.position_at_end(header_block);
            let mut phi_nodes: Vec<(String, PhiValue<'a>)> = Vec::new();
            for (name, pre_val) in &pre_loop_vars {
                let phi = llvm!(builder.build_phi(pre_val.get_type(), &format!("phi_{}", name)));
                phi.add_incoming(&[(pre_val, entry_end_block)]);
                phi_nodes.push((name.clone(), phi));
                variables.insert(name.clone(), phi.as_basic_value());
            }
            let should_cancel_fn = declare_task_group_should_cancel_current_extern(context, module);
            let cancel_flag =
                llvm!(builder.build_call(should_cancel_fn, &[], "task_group_cancel_check",))
                    .try_as_basic_value()
                    .left()
                    .ok_or_else(|| MumeiError::codegen("cancel check returned void".to_string()))?
                    .into_int_value();
            let is_cancelled = llvm!(builder.build_int_compare(
                IntPredicate::NE,
                cancel_flag,
                context.i64_type().const_int(0, false),
                "loop_cancelled",
            ));
            llvm!(builder.build_conditional_branch(is_cancelled, after_block, cond_block));

            builder.position_at_end(cond_block);
            let cond_val = compile_hir_expr(
                context, builder, module, function, cond, variables, var_types, array_ptrs,
                module_env,
            )?
            .into_int_value();
            let cond_bool = llvm!(builder.build_int_compare(
                IntPredicate::NE,
                cond_val,
                context.i64_type().const_int(0, false),
                "loop_cond"
            ));
            llvm!(builder.build_conditional_branch(cond_bool, body_block, after_block));

            builder.position_at_end(body_block);
            compile_hir_stmt(
                context, builder, module, function, body, variables, var_types, array_ptrs,
                module_env,
            )?;
            let body_end_block = builder.get_insert_block().unwrap();
            let body_assigned = collect_assigned_outer_variables_stmt(body);
            for (name, pre_array) in pre_loop_arrays.iter() {
                let changed = array_ptrs.get(name).is_none_or(|body_array| {
                    body_array.0.as_value_ref() != pre_array.0.as_value_ref()
                        || body_array.1 != pre_array.1
                        || body_array.2.as_value_ref() != pre_array.2.as_value_ref()
                });
                if body_assigned.contains(name) && changed {
                    return Err(MumeiError::codegen(format!(
                        "array '{name}' is reassigned inside a loop, which native codegen does not support yet"
                    )));
                }
                array_ptrs.insert(name.clone(), *pre_array);
            }
            array_ptrs.retain(|name, _| pre_loop_arrays.contains_key(name));

            for (name, phi) in &phi_nodes {
                if let Some(body_val) = variables.get(name) {
                    phi.add_incoming(&[(body_val, body_end_block)]);
                }
            }

            llvm!(builder.build_unconditional_branch(header_block));

            builder.position_at_end(after_block);
            for (name, phi) in &phi_nodes {
                variables.insert(name.clone(), phi.as_basic_value());
            }
            // Names bound only inside the body leave scope here; keeping
            // them would carry a body-block value into a later loop's
            // phi, whose incoming block it cannot dominate. var_types
            // follows the same scope — a body-local lambda marker left
            // behind would stay "callable" and demand capture values
            // that no longer exist.
            variables.retain(|name, _| pre_loop_vars.contains_key(name));
            var_types.retain(|name, _| pre_loop_var_types.contains_key(name));
            Ok(context.i64_type().const_int(0, false).into())
        }
        HirStmt::Block { stmts, tail_expr } => {
            let mut last_val: BasicValueEnum = context.i64_type().const_int(0, false).into();
            for s in stmts {
                last_val = compile_hir_stmt(
                    context, builder, module, function, s, variables, var_types, array_ptrs,
                    module_env,
                )?;
            }
            if let Some(tail) = tail_expr {
                last_val = compile_hir_expr(
                    context, builder, module, function, tail, variables, var_types, array_ptrs,
                    module_env,
                )?;
            }
            Ok(last_val)
        }
        HirStmt::Acquire { resource, body } => {
            let ptr_type = context.ptr_type(AddressSpace::default());
            let i32_type = context.i32_type();

            let lock_fn = module
                .get_function("pthread_mutex_lock")
                .unwrap_or_else(|| {
                    let fn_type = i32_type.fn_type(&[ptr_type.into()], false);
                    module.add_function(
                        "pthread_mutex_lock",
                        fn_type,
                        Some(inkwell::module::Linkage::External),
                    )
                });
            let unlock_fn = module
                .get_function("pthread_mutex_unlock")
                .unwrap_or_else(|| {
                    let fn_type = i32_type.fn_type(&[ptr_type.into()], false);
                    module.add_function(
                        "pthread_mutex_unlock",
                        fn_type,
                        Some(inkwell::module::Linkage::External),
                    )
                });
            let get_resource_fn = module
                .get_function("__mumei_get_resource_mutex")
                .unwrap_or_else(|| {
                    let fn_type = ptr_type.fn_type(&[ptr_type.into()], false);
                    module.add_function(
                        "__mumei_get_resource_mutex",
                        fn_type,
                        Some(inkwell::module::Linkage::External),
                    )
                });
            let resource_name = builder
                .build_global_string_ptr(resource, &format!("resource_name_{}", resource))
                .map_err(|e| MumeiError::codegen(format!("Failed to build resource name: {e}")))?;
            let mutex_call = llvm!(builder.build_call(
                get_resource_fn,
                &[resource_name.as_pointer_value().into()],
                &format!("resource_{}", resource)
            ));
            let mutex_ptr = mutex_call
                .try_as_basic_value()
                .left()
                .ok_or_else(|| {
                    MumeiError::codegen("resource mutex helper returned void".to_string())
                })?
                .into_pointer_value();

            llvm!(builder.build_call(lock_fn, &[mutex_ptr.into()], &format!("lock_{}", resource)));

            let body_result = compile_hir_stmt(
                context, builder, module, function, body, variables, var_types, array_ptrs,
                module_env,
            )?;

            llvm!(builder.build_call(
                unlock_fn,
                &[mutex_ptr.into()],
                &format!("unlock_{}", resource)
            ));

            Ok(body_result)
        }
        HirStmt::Expr(expr) => compile_hir_expr(
            context, builder, module, function, expr, variables, var_types, array_ptrs, module_env,
        ),
    }
}

/// The last expression a statement contributes as its value — mirrors
/// what `compile_hir_stmt` returns for expression/block tails.
fn stmt_tail_expr(stmt: &HirStmt) -> Option<&HirExpr> {
    match stmt {
        HirStmt::Expr(e) => Some(e),
        HirStmt::Block { tail_expr, .. } => tail_expr.as_deref(),
        _ => None,
    }
}

/// Resolved base type name of an expression that evaluates to an array
/// (`[T]`): a call whose callee declares or infers an array return, a
/// binding whose recorded type is an array, or an `if`/`match` whose
/// branch tails do — else `None`.
fn expr_array_type_name(
    expr: &HirExpr,
    var_types: &HashMap<String, String>,
    module_env: &ModuleEnv,
) -> Option<String> {
    let type_name = match expr {
        HirExpr::Call { name, .. } => {
            let fqn_name = name.replace('.', "::");
            let callee = module_env
                .get_atom(name)
                .or_else(|| module_env.get_atom(&fqn_name))?;
            callee.return_type.clone().or_else(|| {
                mumei_core::hir::lower_atom_to_hir(callee)
                    .signature
                    .inferred_return_type
            })?
        }
        HirExpr::Variable(name) => var_types.get(name.as_str())?.clone(),
        // A literal tail always produces `[i64]` here — element types in
        // this lane are profile-int — so `let t = if c {[1,2]} else
        // {[3,4]}` binds the merged fat pointer.
        HirExpr::ArrayLit(_) => "[i64]".to_string(),
        HirExpr::IfThenElse {
            then_branch,
            else_branch,
            ..
        } => {
            return [then_branch.as_ref(), else_branch.as_ref()]
                .into_iter()
                .filter_map(stmt_tail_expr)
                .find_map(|tail| expr_array_type_name(tail, var_types, module_env));
        }
        HirExpr::Match { arms, .. } => {
            return arms
                .iter()
                .filter_map(|arm| stmt_tail_expr(&arm.body))
                .find_map(|tail| expr_array_type_name(tail, var_types, module_env));
        }
        _ => return None,
    };
    let base = module_env.resolve_base_type(&type_name);
    matches!(
        mumei_core::lowering::lower(&base),
        mumei_core::lowering::LoweredType::Array(_)
    )
    .then_some(base)
}

// `expr_array_stack_backed` was removed: bound arrays are always
// heap-copied at bind time, so no expression can surface stack-backed
// storage through a binding any more. `driver::tail_slots` still
// classifies *return* tails (literals and stack-bound aliases) itself.
