#![allow(unused_imports)]
use super::super::support::*;
use super::super::*;
use super::*;
use crate::lowering::{lower, LoweredType};
use crate::verification::invariant_inference::{
    expr_to_string, generate_candidates, InferredInvariant,
};
use crate::verification::translator::may_write::{callee_may_write_args, CalleeRef};
use crate::verification::translator::z3_types::{
    array_root_ast, with_array_shared_markers_restored,
};
use serde_json::json;

const ENUM_STRUCT_ARRAY_REBUILD_KEY: &str = "__enum_struct_array_rebuild";

fn array_binding_is_shared_source(value: &Expr, env: &Env<'_>) -> bool {
    match value {
        Expr::Variable(name) => super::z3_types::is_array_shared(env, name),
        Expr::FieldAccess(_, _) => true,
        _ => false,
    }
}

fn mark_array_source_shared<'a>(vc: &VCtx<'a>, env: &mut Env<'a>, value: &Expr) {
    let Expr::Variable(name) = value else {
        return;
    };
    let is_array = env
        .get(&format!("__z3_arr_{name}"))
        .and_then(|value| value.as_array())
        .is_some()
        || env.get(name).and_then(|value| value.as_array()).is_some();
    if is_array {
        super::z3_types::mark_array_shared(vc.ctx, env, name);
    }
}

fn collect_array_shared_markers<'a>(env: &Env<'a>) -> Env<'a> {
    env.iter()
        .filter(|(key, value)| {
            key.starts_with(super::z3_types::ARRAY_SHARED_PREFIX)
                && value
                    .as_bool()
                    .and_then(|marker| marker.as_bool())
                    .unwrap_or(false)
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// Insert `__z3_arr_<var>` for every caller variable the call may store
/// through — the same may-write set `havoc_array_args` /
/// `apply_local_lambda` havoc at call evaluation, so loop-carried havoc
/// matches it exactly.
fn mark_call_store_args(
    vc: &VCtx<'_>,
    callee: CalleeRef<'_>,
    args: &[Expr],
    env: &Env,
    out: &mut std::collections::HashSet<String>,
) {
    for var in callee_may_write_args(vc, callee, args, env) {
        out.insert(format!("__z3_arr_{var}"));
        mark_all_struct_array_leaf_havoc(vc, env, &var, out);
    }
}

fn mark_all_struct_array_leaf_havoc(
    vc: &VCtx<'_>,
    env: &Env<'_>,
    name: &str,
    out: &mut std::collections::HashSet<String>,
) {
    let mut aliases = std::collections::HashSet::from([name.to_string()]);
    let array = env
        .get(&format!("__z3_arr_{name}"))
        .and_then(|value| value.as_array())
        .or_else(|| env.get(name).and_then(|value| value.as_array()));
    if let Some(array) = array {
        let root = array_root_ast(&array);
        aliases.extend(env.iter().filter_map(|(key, value)| {
            key.strip_prefix("__z3_arr_").and_then(|alias| {
                value
                    .as_array()
                    .is_some_and(|candidate| array_root_ast(&candidate) == root)
                    .then(|| alias.to_string())
            })
        }));
    }

    for alias in aliases {
        let mut element_type = super::z3_types::array_element_type_name(&alias, vc);
        if super::z3_types::struct_array_leaf_types(&element_type, vc.module_env).is_none() {
            if let Some(array) = env
                .get(&format!("__z3_arr_{alias}"))
                .and_then(|value| value.as_array())
                .or_else(|| env.get(&alias).and_then(|value| value.as_array()))
            {
                element_type = super::z3_types::z3_range_type_name(&array.get_sort());
            }
        }
        if let Some(leaves) = super::z3_types::struct_array_leaf_types(&element_type, vc.module_env)
        {
            out.insert(format!("__z3_arr_{alias}"));
            out.extend(
                leaves
                    .into_iter()
                    .map(|(path, _)| super::z3_types::struct_array_leaf_key(&alias, &path)),
            );
        }
    }
}

/// Collect the env names a statement can overwrite: `Assign` targets plus the
/// `__z3_arr_<name>` keys `ArrayStore` writes — and the same keys for array
/// variables passed to a call that stores through its param (the runtime
/// applies `havoc_array_args`, so the slot is loop-carried). Used to havoc
/// loop-carried variables before induction checks.
fn collect_assigned_vars(
    env: &Env,
    vc: &VCtx<'_>,
    stmt: &Stmt,
    out: &mut std::collections::HashSet<String>,
) {
    match stmt {
        Stmt::Assign { var, value, .. } => {
            // A field write `q.f = v` rebinds the whole `q` aggregate —
            // havoc must reset `q` itself, since a struct's env keys are
            // named `q_f`/`__struct_q_f`, not `q.f`. The dotted name is
            // kept too: resource states live in env under the verbatim
            // `res.field` key, and the `env.contains_key` retain simply
            // drops it when no such binding exists.
            match var.split_once('.') {
                Some((base, _)) => {
                    out.insert(base.to_string());
                    out.insert(var.clone());
                }
                None => {
                    out.insert(var.clone());
                    mark_all_struct_array_leaf_havoc(vc, env, var, out);
                }
            }
            collect_expr_assigned_vars(env, vc, value, out, true);
        }
        Stmt::ArrayStore {
            array,
            index,
            field_path,
            value,
            ..
        } => {
            let element_type = super::z3_types::array_element_type_name(array, vc);
            if let Some(field_path) = field_path {
                out.insert(super::z3_types::struct_array_leaf_key(
                    array,
                    &field_path.join("_"),
                ));
                out.insert(format!("__z3_arr_{array}"));
            } else if let Some(leaves) =
                super::z3_types::struct_array_leaf_types(&element_type, vc.module_env)
            {
                out.extend(
                    leaves
                        .into_iter()
                        .map(|(path, _)| super::z3_types::struct_array_leaf_key(array, &path)),
                );
                out.insert(format!("__z3_arr_{array}"));
            } else {
                out.insert(format!("__z3_arr_{}", array));
            }
            collect_expr_assigned_vars(env, vc, index, out, true);
            collect_expr_assigned_vars(env, vc, value, out, true);
        }
        Stmt::Block(stmts, _) => {
            for s in stmts {
                collect_assigned_vars(env, vc, s, out);
            }
        }
        Stmt::While {
            cond,
            invariant,
            decreases,
            body,
            ..
        } => {
            collect_assigned_vars(env, vc, body, out);
            // Conditions/invariants/decreases are only *evaluated* on the
            // havoced envs — calls inside them havoc live at eval time and
            // don't belong in the modified set.
            collect_expr_assigned_vars(env, vc, cond, out, false);
            collect_expr_assigned_vars(env, vc, invariant, out, false);
            if let Some(d) = decreases {
                collect_expr_assigned_vars(env, vc, d, out, false);
            }
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
            collect_assigned_vars(env, vc, body, out);
        }
        Stmt::TaskGroup { children, .. } => {
            for c in children {
                collect_assigned_vars(env, vc, c, out);
            }
        }
        Stmt::Expr(expr, _) => collect_expr_assigned_vars(env, vc, expr, out, true),
        Stmt::Let { value, .. } => collect_expr_assigned_vars(env, vc, value, out, true),
        _ => {}
    }
}

/// Recursive companion of `collect_assigned_vars` over expressions that embed
/// statement bodies (`if`, `match`, `async`, lambdas). `in_stmt_ctx` marks
/// expressions reachable as executed code — only then do calls contribute
/// array-store args to the modified set.
fn collect_expr_assigned_vars(
    env: &Env,
    vc: &VCtx<'_>,
    expr: &Expr,
    out: &mut std::collections::HashSet<String>,
    in_stmt_ctx: bool,
) {
    match expr {
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_expr_assigned_vars(env, vc, cond, out, in_stmt_ctx);
            collect_assigned_vars(env, vc, then_branch, out);
            collect_assigned_vars(env, vc, else_branch, out);
        }
        Expr::Block(stmt) => collect_assigned_vars(env, vc, stmt, out),
        Expr::Match { target, arms } => {
            collect_expr_assigned_vars(env, vc, target, out, in_stmt_ctx);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_expr_assigned_vars(env, vc, guard, out, in_stmt_ctx);
                }
                collect_assigned_vars(env, vc, &arm.body, out);
            }
        }
        Expr::Async { body } | Expr::Lambda { body, .. } => {
            collect_assigned_vars(env, vc, body, out)
        }
        Expr::Await { expr } | Expr::FieldAccess(expr, _) | Expr::ChanRecv { channel: expr } => {
            collect_expr_assigned_vars(env, vc, expr, out, in_stmt_ctx);
        }
        Expr::ArrayAccess(_, index) => collect_expr_assigned_vars(env, vc, index, out, in_stmt_ctx),
        Expr::ArrayLit(elems) => {
            for e in elems {
                collect_expr_assigned_vars(env, vc, e, out, in_stmt_ctx);
            }
        }
        Expr::BinaryOp(l, _, r) => {
            collect_expr_assigned_vars(env, vc, l, out, in_stmt_ctx);
            collect_expr_assigned_vars(env, vc, r, out, in_stmt_ctx);
        }
        Expr::Call(callee_name, args) => {
            for a in args {
                collect_expr_assigned_vars(env, vc, a, out, in_stmt_ctx);
            }
            if in_stmt_ctx {
                mark_call_store_args(vc, CalleeRef::Name(callee_name), args, env, out);
            }
        }
        Expr::Perform { args, .. } => {
            for a in args {
                collect_expr_assigned_vars(env, vc, a, out, in_stmt_ctx);
            }
        }
        Expr::CallRef { callee, args } => {
            collect_expr_assigned_vars(env, vc, callee, out, in_stmt_ctx);
            for a in args {
                collect_expr_assigned_vars(env, vc, a, out, in_stmt_ctx);
            }
            if in_stmt_ctx {
                mark_call_store_args(vc, CalleeRef::Expr(callee), args, env, out);
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, v) in fields {
                collect_expr_assigned_vars(env, vc, v, out, in_stmt_ctx);
            }
        }
        Expr::ChanSend { channel, value } => {
            collect_expr_assigned_vars(env, vc, channel, out, in_stmt_ctx);
            collect_expr_assigned_vars(env, vc, value, out, in_stmt_ctx);
        }
        _ => {}
    }
}

/// Rebind each name in `vars` to a fresh unconstrained constant of its
/// existing sort.
fn havoc_vars<'a>(
    vc: &VCtx<'a>,
    env: &mut Env<'a>,
    vars: &std::collections::HashSet<String>,
    solver_opt: Option<&Solver<'a>>,
) {
    static HAVOC_UID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let ctx = vc.ctx;
    for name in vars {
        if let Some(old) = env.get(name) {
            let uid = HAVOC_UID.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let fresh_name = format!("__havoc_{}_{}", name, uid);
            let fresh: Dynamic = match old.get_sort().kind() {
                z3::SortKind::Int => Int::fresh_const(ctx, &fresh_name).into(),
                z3::SortKind::Bool => Bool::fresh_const(ctx, &fresh_name).into(),
                z3::SortKind::Real => Real::fresh_const(ctx, &fresh_name).into(),
                z3::SortKind::BV => {
                    let width = old.as_bv().map(|b| b.get_size()).unwrap_or(64);
                    BV::fresh_const(ctx, &fresh_name, width).into()
                }
                z3::SortKind::Array => {
                    let root = old.as_array().map(|a| array_root_ast(&a));
                    let fresh_arr = super::z3_types::z3_fresh_const_like(ctx, &fresh_name, old);
                    // Aliases (`let b = a`, sibling `__z3_arr_` slots) share
                    // the same backing root — rebind them too or their reads
                    // keep answering with the pre-loop entry const.
                    if let Some(root) = root {
                        let shared: Vec<String> = env
                            .iter()
                            .filter(|(_, v)| {
                                v.as_array().is_some_and(|a| array_root_ast(&a) == root)
                            })
                            .map(|(k, _)| k.clone())
                            .collect();
                        let mut struct_bindings = Vec::new();
                        for key in &shared {
                            if let Some(binding) = key.strip_prefix("__z3_arr_") {
                                struct_bindings.push(binding.to_string());
                            } else if !key.starts_with("__struct_arr#") {
                                struct_bindings.push(key.clone());
                            }
                        }
                        for key in shared {
                            env.insert(key, fresh_arr.clone());
                        }
                        let has_explicit_leaf_havoc = struct_bindings.iter().any(|binding| {
                            let prefix = format!("__struct_arr#{binding}#");
                            vars.iter().any(|key| key.starts_with(&prefix))
                        });
                        if !has_explicit_leaf_havoc {
                            super::z3_types::havoc_struct_array_leaf_arrays(
                                vc,
                                &struct_bindings,
                                env,
                            );
                        }
                    }
                    fresh_arr
                }
                // Str (Seq), Datatype, and any other sort: keep the sort but
                // drop the value — cloning `old` would let a loop-modified
                // `Str` prove `s == "x"` with its pre-loop value (unsound).
                // `Dynamic` has no `fresh_const`, so mint the const through
                // the C API at the same raw sort.
                _ => {
                    let raw_ctx = raw_z3_context(ctx);
                    let c_name = std::ffi::CString::new(fresh_name.as_str())
                        .unwrap_or_else(|_| std::ffi::CString::new("__havoc").unwrap());
                    let ast = unsafe {
                        z3_sys::Z3_mk_fresh_const(
                            raw_ctx,
                            c_name.as_ptr(),
                            z3_sys::Z3_get_sort(raw_ctx, old.get_z3_ast()),
                        )
                    };
                    unsafe { Dynamic::wrap(ctx, ast) }
                }
            };
            env.insert(name.clone(), fresh.clone());
            // Havoc forgets the variable's value, so a lambda bound before
            // the loop must not keep answering `name(args)` inside the
            // induction step — drop the binding and let the call fail
            // closed as an unknown function instead.
            vc.local_lambdas.borrow_mut().remove(name);
            // A tracked array's `__z3_arr_`/`len_` slots must havoc with the
            // binding — otherwise `a[i]` reads the pre-loop store chain.
            let arr_key = format!("__z3_arr_{name}");
            if env.contains_key(&arr_key) {
                env.insert(arr_key.clone(), fresh);
            }
            let len_key = array_len_key(name);
            if env.contains_key(&len_key) {
                let len = env
                    .get(&arr_key)
                    .and_then(|value| value.as_array())
                    .map(|array| array_len_for_value(vc, env, &array, solver_opt))
                    .unwrap_or_else(|| {
                        Int::fresh_const(ctx, &array_len_key(&format!("__havoc_{name}_{uid}")))
                            .into()
                    });
                env.insert(len_key, len);
            }
            // A struct binding's field projections are separate env
            // keys keyed on the binding (`q_x`, `__struct_q_x`) and on
            // the live handle (`__struct_<handle>_x`). Havoc the
            // binding-keyed slots and mirror them under the fresh
            // handle's name, or `q.f` resolves to the pre-loop
            // projection (or fails to resolve at all).
            if let Some(struct_type) =
                crate::verification::support::datatype::binding_declared_type(vc, name)
                    .and_then(|ty| canonical_struct_type(vc, &ty))
            {
                if let Some(sdef) = vc.module_env.get_struct(&struct_type) {
                    let slot_keys = struct_binding_slot_keys(vc.module_env, name, sdef, env);
                    havoc_vars(vc, env, &slot_keys, solver_opt);
                    if let Some(handle) = env.get(name).cloned() {
                        bind_havoced_struct_fields_to_handle(
                            vc,
                            env,
                            name,
                            sdef,
                            &handle,
                            0,
                            &mut vec![struct_type],
                        );
                    }
                }
            }
        }
    }
}

fn check_while_invariant<'a>(
    vc: &VCtx<'a>,
    invariant: &Expr,
    cond: &Expr,
    body: &Stmt,
    env: &mut Env<'a>,
    solver: &Solver<'a>,
    modified: &std::collections::BTreeSet<String>,
) -> MumeiResult<bool> {
    let ctx = vc.ctx;
    let path_cond = vc.path_cond_conj();
    let env_snapshot = env.clone();
    let linearity_snapshot = vc.linearity_ctx.map(|cell| cell.borrow().clone());
    let effect_snapshot = vc.effect_ctx.map(|cell| cell.borrow().clone());
    let constraint_count_snapshot = vc.constraint_count.map(|cell| cell.get());
    let string_constraints_snapshot = vc.has_string_constraints.map(|cell| cell.get());
    let path_cond_snapshot = vc.path_cond_stack.borrow().clone();
    let inferred_len_snapshot = vc.inferred_invariants.map(|cell| cell.borrow().len());
    let bv_shift_snapshot = vc.bv_shift_obligations.borrow().clone();
    let bv_div_snapshot = vc.bv_div_obligations.borrow().clone();
    let clause_context_snapshot = vc.clause_context.borrow().clone();
    let enum_sorts_snapshot = vc.enum_sorts.borrow().clone();
    let types_snapshot = vc.local_enum_types.borrow().clone();
    let int_types_snapshot = vc.local_int_types.borrow().clone();
    let array_elem_types_snapshot = vc.local_array_elem_types.borrow().clone();
    let lambdas_snapshot = vc.local_lambdas.borrow().clone();
    let call_result_lens_snapshot = vc.call_result_lens.borrow().clone();
    let restore = || {
        if let (Some(cell), Some(snapshot)) = (vc.linearity_ctx, linearity_snapshot.as_ref()) {
            *cell.borrow_mut() = snapshot.clone();
        }
        if let (Some(cell), Some(snapshot)) = (vc.effect_ctx, effect_snapshot.as_ref()) {
            *cell.borrow_mut() = snapshot.clone();
        }
        if let Some(cell) = vc.constraint_count {
            cell.set(constraint_count_snapshot.unwrap_or_default());
        }
        if let Some(cell) = vc.has_string_constraints {
            cell.set(string_constraints_snapshot.unwrap_or(false));
        }
        *vc.path_cond_stack.borrow_mut() = path_cond_snapshot.clone();
        if let (Some(cell), Some(length)) = (vc.inferred_invariants, inferred_len_snapshot) {
            cell.borrow_mut().truncate(length);
        }
        *vc.bv_shift_obligations.borrow_mut() = bv_shift_snapshot.clone();
        *vc.bv_div_obligations.borrow_mut() = bv_div_snapshot.clone();
        *vc.clause_context.borrow_mut() = clause_context_snapshot.clone();
        *vc.enum_sorts.borrow_mut() = enum_sorts_snapshot.clone();
        *vc.local_enum_types.borrow_mut() = types_snapshot.clone();
        *vc.local_int_types.borrow_mut() = int_types_snapshot.clone();
        *vc.local_array_elem_types.borrow_mut() = array_elem_types_snapshot.clone();
        *vc.local_lambdas.borrow_mut() = lambdas_snapshot.clone();
        *vc.call_result_lens.borrow_mut() = call_result_lens_snapshot.clone();
    };
    let mut loop_shared_markers = std::collections::HashMap::new();
    let result = (|| -> MumeiResult<bool> {
        let inv =
            with_array_shared_markers_restored(env, |env| expr_to_z3(vc, invariant, env, None))?
                .as_bool()
                .ok_or(MumeiError::type_error("Invariant must be boolean"))?;
        solver.push();
        solver.assert(&Bool::and(ctx, &[&path_cond, &inv.not()]));
        let base = crate::verification::phase_artifacts::check(solver);
        solver.pop(1);
        if base != SatResult::Unsat {
            return Ok(false);
        }

        let mut step_env = env.clone();
        let modified_set: std::collections::HashSet<String> = modified.iter().cloned().collect();
        havoc_vars(vc, &mut step_env, &modified_set, Some(solver));
        let step = (|| -> MumeiResult<SatResult> {
            let inv_h = with_array_shared_markers_restored(&mut step_env, |env| {
                expr_to_z3(vc, invariant, env, None)
            })?
            .as_bool()
            .ok_or(MumeiError::type_error("Invariant must be boolean"))?;
            let c_h = expr_to_z3(vc, cond, &mut step_env, None)?
                .as_bool()
                .ok_or(MumeiError::type_error("While condition must be boolean"))?;
            solver.push();
            let result = (|| -> MumeiResult<SatResult> {
                solver.assert(&inv_h);
                solver.assert(&c_h);
                stmt_to_z3(vc, body, &mut step_env, Some(solver))?;
                loop_shared_markers.extend(collect_array_shared_markers(&step_env));
                let inv_after = with_array_shared_markers_restored(&mut step_env, |env| {
                    expr_to_z3(vc, invariant, env, None)
                })?
                .as_bool()
                .ok_or(MumeiError::type_error("Invariant must be boolean"))?;
                solver.assert(&inv_after.not());
                Ok(crate::verification::phase_artifacts::check(solver))
            })();
            solver.pop(1);
            result
        })()?;
        Ok(step == SatResult::Unsat)
    })();
    restore();
    *env = env_snapshot;
    env.extend(loop_shared_markers);
    result
}

/// Record the fixed-width integer type of a let/assign binding (`i64` when
/// the value declares none), so a local shadowing a `u64` parameter is
/// checked as `i64` and a local computed from one is checked as `u64`.
fn record_binding_int_type(vc: &VCtx, var: &str, value: &Expr) {
    let ty = crate::verification::translator::expr::fixed_width_int_type(vc, value);
    vc.local_int_types
        .borrow_mut()
        .insert(var.to_string(), ty.unwrap_or("i64"));
}

/// Record (or clear, when the value has no inferable enum type) the
/// inferred declared enum type of a let/assign binding in
/// `vc.local_enum_types`, so a later `match var` resolves variant owners via
/// the declared type rather than guessing across colliding prelude variants.
fn record_binding_enum_type(vc: &VCtx, var: &str, value: &Expr) {
    match crate::verification::support::datatype::infer_expr_enum_name(vc, value) {
        Some(name) => {
            vc.local_enum_types
                .borrow_mut()
                .insert(var.to_string(), name);
        }
        None => {
            vc.local_enum_types.borrow_mut().remove(var);
        }
    }
}

/// Record (or clear, when the value has no inferable struct type) the
/// declared struct type of a let/assign binding in
/// `vc.local_struct_types`, so a later `var.f = v` write resolves the
/// binding's `StructDef` — its field declarations, refined-type
/// predicates and invariants — like a declared struct parameter does.
fn record_binding_struct_type(vc: &VCtx, var: &str, value: &Expr) {
    let ty = match value {
        Expr::StructInit { type_name, .. } => Some(type_name.clone()),
        Expr::ArrayAccess(array, _) => {
            let element_type = super::z3_types::array_element_type_name(array, vc);
            super::z3_types::struct_array_leaf_types(&element_type, vc.module_env)
                .map(|_| element_type)
        }
        _ => crate::verification::support::datatype::declared_type_of_expr(vc, value),
    }
    .and_then(|ty| canonical_struct_type(vc, &ty));
    match ty {
        Some(name) => {
            vc.local_struct_types
                .borrow_mut()
                .insert(var.to_string(), name);
        }
        None => {
            vc.local_struct_types.borrow_mut().remove(var);
        }
    }
}

fn canonical_struct_type_in_module(module_env: &ModuleEnv, type_name: &str) -> Option<String> {
    let resolved = module_env.resolve_base_type(type_name);
    let resolved = resolved.trim();
    if resolved.starts_with('[') || resolved.ends_with("[]") {
        return None;
    }
    let base = crate::verification::support::datatype::type_name_base(resolved).to_string();
    module_env.get_struct(&base).map(|_| base)
}

fn canonical_struct_type(vc: &VCtx, type_name: &str) -> Option<String> {
    canonical_struct_type_in_module(vc.module_env, type_name)
}

fn struct_field_path_exists(
    module_env: &ModuleEnv,
    sdef: &crate::parser::ast::StructDef,
    path: &str,
    memo: &mut std::collections::HashMap<(String, String), bool>,
) -> bool {
    let key = (sdef.name.clone(), path.to_string());
    if let Some(found) = memo.get(&key) {
        return *found;
    }
    let found = sdef.fields.iter().any(|field| {
        if path == field.name {
            return true;
        }
        let Some(rest) = path
            .strip_prefix(&field.name)
            .and_then(|suffix| suffix.strip_prefix('_'))
        else {
            return false;
        };
        let resolved_field_type = module_env.resolve_base_type(&field.type_name);
        if resolved_field_type.trim().starts_with('[') || resolved_field_type.trim().ends_with("[]")
        {
            return false;
        }
        let Some(nested_type) = canonical_struct_type_in_module(module_env, &resolved_field_type)
        else {
            return false;
        };
        let Some(nested) = module_env.get_struct(&nested_type) else {
            return false;
        };
        struct_field_path_exists(module_env, nested, rest, memo)
    });
    memo.insert(key, found);
    found
}

fn is_struct_projection_slot_name(
    module_env: &ModuleEnv,
    binding: &str,
    sdef: &crate::parser::ast::StructDef,
    name: &str,
) -> bool {
    let mut memo = std::collections::HashMap::new();
    is_struct_projection_slot_name_with_memo(module_env, binding, sdef, name, &mut memo)
}

fn is_struct_projection_slot_name_with_memo(
    module_env: &ModuleEnv,
    binding: &str,
    sdef: &crate::parser::ast::StructDef,
    name: &str,
    memo: &mut std::collections::HashMap<(String, String), bool>,
) -> bool {
    [
        format!("{binding}_"),
        format!("{}{}_", STRUCT_FIELD_PREFIX, binding),
    ]
    .iter()
    .filter_map(|prefix| name.strip_prefix(prefix))
    .any(|path| struct_field_path_exists(module_env, sdef, path, memo))
}

const MAX_PROJECTION_CHAIN_DEPTH: usize = 16;
const MAX_PROJECTION_COLLISION_SEARCH_NODES: usize = 200_000;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum ProjectionSide {
    First,
    Second,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ProjectionSearchState {
    first_type: Option<String>,
    first_depth: usize,
    second_type: Option<String>,
    second_depth: usize,
    ahead: Option<ProjectionSide>,
    buffer: String,
    diverged: bool,
}

#[derive(Clone, Debug)]
struct ProjectionSearchNode {
    state: ProjectionSearchState,
    first_chain: Vec<String>,
    second_chain: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProjectionCollisionError {
    LimitExceeded,
}

fn projection_chains_collide(
    module_env: &ModuleEnv,
    first: (&str, &str),
    second: (&str, &str),
    require_distinct: bool,
) -> Result<Option<(String, String, String)>, ProjectionCollisionError> {
    let (first_type, first_prefix) = first;
    let (second_type, second_prefix) = second;
    if module_env.get_struct(first_type).is_none() || module_env.get_struct(second_type).is_none() {
        return Ok(None);
    }

    let (ahead, buffer) = if first_prefix == second_prefix {
        (None, String::new())
    } else if let Some(suffix) = first_prefix.strip_prefix(second_prefix) {
        (Some(ProjectionSide::First), suffix.to_string())
    } else if let Some(suffix) = second_prefix.strip_prefix(first_prefix) {
        (Some(ProjectionSide::Second), suffix.to_string())
    } else {
        return Ok(None);
    };

    let initial_state = ProjectionSearchState {
        first_type: Some(first_type.to_string()),
        first_depth: 0,
        second_type: Some(second_type.to_string()),
        second_depth: 0,
        ahead,
        buffer,
        diverged: false,
    };
    let mut pending = std::collections::VecDeque::from([ProjectionSearchNode {
        state: initial_state.clone(),
        first_chain: Vec::new(),
        second_chain: Vec::new(),
    }]);
    let mut visited = std::collections::HashSet::from([initial_state]);

    while let Some(node) = pending.pop_front() {
        let state = &node.state;
        if state.buffer.is_empty()
            && state.first_depth > 0
            && state.second_depth > 0
            && (!require_distinct || state.diverged || state.first_depth != state.second_depth)
        {
            let first_chain = node.first_chain.join(".");
            let second_chain = node.second_chain.join(".");
            let flattened = flattened_projection_name(first_prefix, &node.first_chain);
            return Ok(Some((first_chain, second_chain, flattened)));
        }

        let sides = match state.ahead {
            Some(ProjectionSide::First) => [Some(ProjectionSide::Second), None],
            Some(ProjectionSide::Second) => [Some(ProjectionSide::First), None],
            None => [Some(ProjectionSide::First), Some(ProjectionSide::Second)],
        };
        for side in sides.into_iter().flatten() {
            let (current_type, depth, prefix) = match side {
                ProjectionSide::First => {
                    (state.first_type.as_deref(), state.first_depth, first_prefix)
                }
                ProjectionSide::Second => (
                    state.second_type.as_deref(),
                    state.second_depth,
                    second_prefix,
                ),
            };
            let Some(current_type) = current_type else {
                continue;
            };
            if depth > MAX_PROJECTION_CHAIN_DEPTH {
                continue;
            }
            let Some(sdef) = module_env.get_struct(current_type) else {
                continue;
            };
            let extensions = sdef
                .fields
                .iter()
                .map(|field| {
                    let nested_type = if depth < MAX_PROJECTION_CHAIN_DEPTH {
                        canonical_struct_type_in_module(module_env, &field.type_name)
                    } else {
                        None
                    };
                    (field.name.clone(), nested_type)
                })
                .collect::<Vec<_>>();

            for (field, nested_type) in extensions {
                let extension = if depth == 0 && prefix.is_empty() {
                    field.clone()
                } else {
                    format!("_{field}")
                };
                let (next_ahead, next_buffer) = match state.ahead {
                    None => (Some(side), extension),
                    Some(ahead_side) => {
                        if extension.starts_with(&state.buffer) {
                            let remainder = extension[state.buffer.len()..].to_string();
                            if remainder.is_empty() {
                                (None, String::new())
                            } else {
                                (Some(side), remainder)
                            }
                        } else if state.buffer.starts_with(&extension) {
                            let remainder = state.buffer[extension.len()..].to_string();
                            if remainder.is_empty() {
                                (None, String::new())
                            } else {
                                (Some(ahead_side), remainder)
                            }
                        } else {
                            continue;
                        }
                    }
                };

                let mut first_chain = node.first_chain.clone();
                let mut second_chain = node.second_chain.clone();
                let mut first_type = state.first_type.clone();
                let mut first_depth = state.first_depth;
                let mut second_type = state.second_type.clone();
                let mut second_depth = state.second_depth;
                match side {
                    ProjectionSide::First => {
                        first_chain.push(field);
                        first_type = nested_type;
                        first_depth += 1;
                    }
                    ProjectionSide::Second => {
                        second_chain.push(field);
                        second_type = nested_type;
                        second_depth += 1;
                    }
                }
                let diverged = state.diverged
                    || first_chain
                        .iter()
                        .zip(&second_chain)
                        .any(|(left, right)| left != right);
                let next_state = ProjectionSearchState {
                    first_type,
                    first_depth,
                    second_type,
                    second_depth,
                    ahead: next_ahead,
                    buffer: next_buffer,
                    diverged,
                };
                if visited.contains(&next_state) {
                    continue;
                }
                if visited.len() >= MAX_PROJECTION_COLLISION_SEARCH_NODES {
                    return Err(ProjectionCollisionError::LimitExceeded);
                }
                visited.insert(next_state.clone());
                pending.push_back(ProjectionSearchNode {
                    state: next_state,
                    first_chain,
                    second_chain,
                });
            }
        }
    }

    Ok(None)
}

fn flattened_projection_name(prefix: &str, chain: &[String]) -> String {
    if chain.is_empty() {
        return prefix.to_string();
    }
    let path = chain.join("_");
    if prefix.is_empty() {
        path
    } else {
        format!("{prefix}_{path}")
    }
}

pub(crate) fn reject_struct_projection_path_collisions(
    module_env: &ModuleEnv,
    atom: &crate::parser::Atom,
) -> MumeiResult<()> {
    let mut structs = module_env.structs.values().collect::<Vec<_>>();
    structs.sort_unstable_by(|left, right| left.name.cmp(&right.name));

    for sdef in structs {
        match projection_chains_collide(module_env, (&sdef.name, ""), (&sdef.name, ""), true) {
            Ok(Some((first, second, flattened))) => {
                let (path_a, path_b) = if first <= second {
                    (first, second)
                } else {
                    (second, first)
                };
                return Err(MumeiError::type_error_at(
                    format!(
                        "struct '{}' has fields whose flattened projection paths collide ('{path_a}' and '{path_b}' both map to '{flattened}'); rename a field",
                        sdef.name
                    ),
                    atom.span.clone(),
                ));
            }
            Err(ProjectionCollisionError::LimitExceeded) => {
                return Err(MumeiError::type_error_at(
                    format!(
                        "struct '{}' projection paths exceed the bounded collision-check limit; cannot establish collision-free fields",
                        sdef.name
                    ),
                    atom.span.clone(),
                ));
            }
            Ok(None) => {}
        }
    }

    Ok(())
}

pub(crate) fn reject_parameter_projection_slot_collisions(
    module_env: &ModuleEnv,
    atom: &crate::parser::Atom,
) -> MumeiResult<()> {
    let struct_types = atom
        .params
        .iter()
        .map(|param| {
            param
                .type_name
                .as_deref()
                .and_then(|type_name| canonical_struct_type_in_module(module_env, type_name))
        })
        .collect::<Vec<_>>();

    for (p_index, p) in atom.params.iter().enumerate() {
        let Some(p_type) = struct_types[p_index].as_deref() else {
            continue;
        };
        let Some(p_struct) = module_env.get_struct(p_type) else {
            continue;
        };
        let mut path_memo = std::collections::HashMap::new();

        for (q_index, q) in atom.params.iter().enumerate() {
            if p_index == q_index {
                continue;
            }
            let collision_error = || {
                MumeiError::type_error_at(
                    format!(
                        "parameter '{}' collides with a field projection slot of struct parameter '{}'; rename the parameter",
                        q.name, p.name
                    ),
                    atom.span.clone(),
                )
            };

            if is_struct_projection_slot_name_with_memo(
                module_env,
                &p.name,
                p_struct,
                &q.name,
                &mut path_memo,
            ) {
                return Err(collision_error());
            }

            let Some(q_type) = struct_types[q_index].as_deref() else {
                continue;
            };
            let p_prefixes = [p.name.clone(), format!("{STRUCT_FIELD_PREFIX}{}", p.name)];
            let q_prefixes = [q.name.clone(), format!("{STRUCT_FIELD_PREFIX}{}", q.name)];
            for p_prefix in &p_prefixes {
                for q_prefix in &q_prefixes {
                    match projection_chains_collide(
                        module_env,
                        (p_type, p_prefix.as_str()),
                        (q_type, q_prefix.as_str()),
                        false,
                    ) {
                        Ok(Some(_)) | Err(ProjectionCollisionError::LimitExceeded) => {
                            return Err(collision_error());
                        }
                        Ok(None) => {}
                    }
                }
            }
        }
    }
    Ok(())
}

fn reject_struct_projection_slot_collisions(
    vc: &VCtx,
    var: &str,
    value: &Expr,
    env: &Env<'_>,
) -> MumeiResult<()> {
    let active_struct_types = vc.local_struct_types.borrow().clone();
    for (binding, type_name) in &active_struct_types {
        if binding == var || !env.contains_key(binding) {
            continue;
        }
        let Some(sdef) = vc
            .module_env
            .get_struct(&canonical_struct_type(vc, type_name).unwrap_or_default())
        else {
            continue;
        };
        if is_struct_projection_slot_name(vc.module_env, binding, sdef, var) {
            return Err(MumeiError::type_error(format!(
                "local '{var}' collides with a field projection slot for struct binding '{binding}'; rename the local"
            )));
        }
    }

    let new_struct_type = match value {
        Expr::StructInit { type_name, .. } => Some(type_name.clone()),
        _ => crate::verification::support::datatype::declared_type_of_expr(vc, value),
    }
    .and_then(|type_name| canonical_struct_type(vc, &type_name));
    let Some(new_struct_type) = new_struct_type else {
        return Ok(());
    };
    if active_struct_types.contains_key(var) {
        return Ok(());
    }
    let Some(sdef) = vc.module_env.get_struct(&new_struct_type) else {
        return Ok(());
    };
    for name in env.keys() {
        if is_struct_projection_slot_name(vc.module_env, var, sdef, name) {
            return Err(MumeiError::type_error(format!(
                "field projection slot '{name}' for local struct '{var}' collides with an existing local; rename the local"
            )));
        }
    }
    Ok(())
}

fn struct_binding_slot_keys<'a>(
    module_env: &ModuleEnv,
    binding: &str,
    sdef: &crate::parser::ast::StructDef,
    env: &Env<'a>,
) -> std::collections::HashSet<String> {
    fn collect<'a>(
        module_env: &ModuleEnv,
        binding: &str,
        sdef: &crate::parser::ast::StructDef,
        env: &Env<'a>,
        depth: usize,
        type_path: &mut Vec<String>,
        keys: &mut std::collections::HashSet<String>,
    ) {
        const MAX_NESTED_DEPTH: usize = 16;
        for field in &sdef.fields {
            let direct_key = format!("{binding}_{}", field.name);
            let struct_key = struct_field_key(binding, &field.name);
            for key in [direct_key.clone(), struct_key] {
                if env.contains_key(&key) {
                    keys.insert(key);
                }
            }
            let resolved = module_env.resolve_base_type(&field.type_name);
            if depth >= MAX_NESTED_DEPTH
                || resolved.trim().starts_with('[')
                || resolved.trim().ends_with("[]")
            {
                continue;
            }
            let Some(nested_type) = canonical_struct_type_in_module(module_env, &resolved) else {
                continue;
            };
            let direct_prefix = format!("{direct_key}_");
            let struct_prefix = format!("{STRUCT_FIELD_PREFIX}{direct_key}_");
            let has_nested_slots = env
                .keys()
                .any(|key| key.starts_with(&direct_prefix) || key.starts_with(&struct_prefix));
            if !has_nested_slots || type_path.contains(&nested_type) {
                continue;
            }
            if let Some(nested) = module_env.get_struct(&nested_type) {
                type_path.push(nested_type);
                collect(
                    module_env,
                    &direct_key,
                    nested,
                    env,
                    depth + 1,
                    type_path,
                    keys,
                );
                type_path.pop();
            }
        }
    }

    let mut keys = std::collections::HashSet::new();
    collect(
        module_env,
        binding,
        sdef,
        env,
        0,
        &mut vec![sdef.name.clone()],
        &mut keys,
    );
    keys
}

fn bind_havoced_struct_fields_to_handle<'a>(
    vc: &VCtx<'a>,
    env: &mut Env<'a>,
    binding: &str,
    sdef: &crate::parser::ast::StructDef,
    handle: &Dynamic<'a>,
    depth: usize,
    type_path: &mut Vec<String>,
) {
    const MAX_NESTED_DEPTH: usize = 16;
    let Some(handle_name) = (handle.kind() == z3::AstKind::App
        && handle.decl().kind() == z3::DeclKind::UNINTERPRETED)
        .then(|| handle.decl().name())
    else {
        return;
    };
    let fields: Vec<(String, Dynamic<'a>)> = sdef
        .fields
        .iter()
        .filter_map(|field| {
            env.get(&struct_field_key(binding, &field.name))
                .or_else(|| env.get(&format!("{binding}_{}", field.name)))
                .map(|value| (field.name.clone(), value.clone()))
        })
        .collect();
    if fields.is_empty() {
        return;
    }
    bind_struct_fields(env, &handle_name, &fields);

    if depth >= MAX_NESTED_DEPTH {
        return;
    }
    for field in &sdef.fields {
        let Some(nested_type) = canonical_struct_type(vc, &field.type_name) else {
            continue;
        };
        if type_path.contains(&nested_type) {
            continue;
        }
        let Some(nested) = vc.module_env.get_struct(&nested_type) else {
            continue;
        };
        let Some(value) = fields
            .iter()
            .find(|(name, _)| name == &field.name)
            .map(|(_, value)| value)
        else {
            continue;
        };
        type_path.push(nested_type);
        bind_havoced_struct_fields_to_handle(
            vc,
            env,
            &format!("{binding}_{}", field.name),
            nested,
            value,
            depth + 1,
            type_path,
        );
        type_path.pop();

        let nested_fields = nested_field_bindings(env, value);
        if !nested_fields.is_empty() {
            bind_struct_fields(
                env,
                &format!("{handle_name}_{}", field.name),
                &nested_fields,
            );
        }
    }
}

fn rebind_struct_binding_slots<'a>(
    vc: &VCtx<'a>,
    binding: &str,
    value: &Dynamic<'a>,
    env: &mut Env<'a>,
    solver_opt: Option<&Solver<'a>>,
) {
    let Some(struct_type) =
        crate::verification::support::datatype::binding_declared_type(vc, binding)
            .and_then(|ty| canonical_struct_type(vc, &ty))
    else {
        return;
    };
    let Some(sdef) = vc.module_env.get_struct(&struct_type) else {
        return;
    };

    let slot_keys = struct_binding_slot_keys(vc.module_env, binding, sdef, env);
    let fields = nested_field_bindings(env, value);
    let mut rebound = std::collections::HashSet::new();
    for (field, _) in &fields {
        rebound.insert(format!("{binding}_{field}"));
        rebound.insert(struct_field_key(binding, field));
    }
    if !fields.is_empty() {
        bind_struct_fields(env, binding, &fields);
    }

    let unbound: std::collections::HashSet<String> =
        slot_keys.difference(&rebound).cloned().collect();
    if !unbound.is_empty() {
        havoc_vars(vc, env, &unbound, solver_opt);
    }
}

/// Parameter count of a `LocalLambda`, following `Ite` branches — a call
/// through a conditional lambda can only pick one arity, so `Ite` reports
/// a count only when both sides agree.
fn local_lambda_arity(lambda: &LocalLambda) -> Option<usize> {
    match lambda {
        LocalLambda::Closure { expr, .. } => match expr {
            Expr::Lambda { params, .. } => Some(params.len()),
            _ => None,
        },
        LocalLambda::Ite {
            then_lam, else_lam, ..
        } => {
            let t = local_lambda_arity(then_lam)?;
            (local_lambda_arity(else_lam)? == t).then_some(t)
        }
        LocalLambda::Opaque => None,
    }
}

/// Resolve an expression to the lambda it denotes: a `Lambda` literal wraps
/// directly, a `Variable` aliases the current binding, and `if c { f }
/// else { g }` (branches resolving recursively) becomes an `Ite` whose
/// `cond` is evaluated in the current env — the let-site snapshot that
/// fixes which body a later `h(args)` inlines.
fn resolve_lambda_expr<'a>(
    vc: &VCtx<'a>,
    expr: &Expr,
    env: &mut Env<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> Option<std::rc::Rc<LocalLambda<'a>>> {
    match expr {
        Expr::Block(stmt) => resolve_lambda_stmt(vc, stmt, env, solver_opt),
        Expr::Lambda { .. } => Some(std::rc::Rc::new(LocalLambda::Closure {
            expr: expr.clone(),
            captured: vc.local_lambdas.borrow().clone(),
        })),
        Expr::Variable(src) => vc.local_lambdas.borrow().get(src).cloned(),
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            let then_lam = resolve_lambda_stmt(vc, then_branch, env, solver_opt)?;
            let else_lam = resolve_lambda_stmt(vc, else_branch, env, solver_opt)?;
            if local_lambda_arity(&then_lam) != local_lambda_arity(&else_lam) {
                return None;
            }
            let cond_ast = expr_to_z3(vc, cond, env, solver_opt).ok()?.as_bool()?;
            Some(std::rc::Rc::new(LocalLambda::Ite {
                cond: cond_ast,
                then_lam,
                else_lam,
            }))
        }
        // `let m = match t { 1 => f, _ => g }` — each arm's tail resolves to
        // a lambda; the binding becomes the ite chain
        // `ite(t==1, f, ite(…, g))` with first-match order preserved.
        // Scoped to Literal / Wildcard / Variable patterns without guards:
        // those conditions evaluate to a plain `target == lit` (or true),
        // which the LLVM selector can reproduce exactly — richer patterns
        // stay unbound so `m(args)` fails closed on both sides.
        Expr::Match { target, arms } => {
            if arms.is_empty() {
                return None;
            }
            let ctx = vc.ctx;
            let target_z3 = expr_to_z3(vc, target, env, solver_opt).ok()?;
            let mut chain: Vec<(z3::ast::Bool<'a>, std::rc::Rc<LocalLambda<'a>>)> = Vec::new();
            let mut arity: Option<usize> = None;
            for arm in arms {
                if arm.guard.is_some() {
                    return None;
                }
                let arm_cond = match &arm.pattern {
                    crate::parser::Pattern::Wildcard | crate::parser::Pattern::Variable(_) => {
                        z3::ast::Bool::from_bool(ctx, true)
                    }
                    crate::parser::Pattern::Literal(n) => {
                        if let Some(bv) = target_z3.as_bv() {
                            let lit = z3::ast::BV::from_i64(ctx, *n, bv.get_size());
                            bv._eq(&lit)
                        } else {
                            let t_int = target_z3.as_int()?;
                            t_int._eq(&z3::ast::Int::from_i64(ctx, *n))
                        }
                    }
                    _ => return None,
                };
                // Arm tail must be a syntactically-flat Variable/Lambda
                // (nested if inside an arm stays unbound: codegen can't
                // compose the arm condition with inner branch conditions
                // without a deeper merge).
                let tail_expr = match arm.body.as_ref() {
                    Stmt::Expr(e, _) => e,
                    Stmt::Block(stmts, _) => match stmts.last() {
                        Some(Stmt::Expr(e, _)) => e,
                        _ => return None,
                    },
                    _ => return None,
                };
                if !matches!(tail_expr, Expr::Variable(_) | Expr::Lambda { .. }) {
                    return None;
                }
                let lam = resolve_lambda_expr(vc, tail_expr, env, solver_opt)?;
                let a = local_lambda_arity(&lam)?;
                match arity {
                    Some(prev) if prev != a => return None,
                    _ => arity = Some(a),
                }
                chain.push((arm_cond, lam));
            }
            // First-match order: the last arm is the else leaf.
            let (_, mut acc) = chain.pop()?;
            for (cond_i, lam_i) in chain.into_iter().rev() {
                acc = std::rc::Rc::new(LocalLambda::Ite {
                    cond: cond_i,
                    then_lam: lam_i,
                    else_lam: acc,
                });
            }
            Some(acc)
        }
        _ => None,
    }
}

fn resolve_lambda_stmt<'a>(
    vc: &VCtx<'a>,
    stmt: &Stmt,
    env: &mut Env<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> Option<std::rc::Rc<LocalLambda<'a>>> {
    match stmt {
        Stmt::Expr(e, _) => resolve_lambda_expr(vc, e, env, solver_opt),
        Stmt::Block(stmts, _) => {
            let saved_lambdas = vc.local_lambdas.borrow().clone();
            for stmt in stmts.iter().take(stmts.len().saturating_sub(1)) {
                let binding = match stmt {
                    Stmt::Let { var, value, .. } | Stmt::Assign { var, value, .. } => {
                        Some((var, value))
                    }
                    _ => None,
                };
                if let Some((var, value)) = binding {
                    let mut value_env = env.clone();
                    match resolve_lambda_expr(vc, value, &mut value_env, solver_opt) {
                        Some(lambda) => {
                            vc.local_lambdas.borrow_mut().insert(var.clone(), lambda);
                        }
                        None => {
                            vc.local_lambdas.borrow_mut().remove(var);
                        }
                    }
                }
            }
            let result = stmts
                .last()
                .and_then(|s| resolve_lambda_stmt(vc, s, env, solver_opt));
            *vc.local_lambdas.borrow_mut() = saved_lambdas;
            result
        }
        _ => None,
    }
}

/// Record (or clear) the lambda bound by a `let`/`assign` so a later
/// `var(args)` / `call(var, args)` can inline the body. `let g = f`
/// where `f` is lambda-bound aliases the binding; rebinding to any other
/// value drops the entry so a stale lambda never answers a call issued
/// to a re-bound name.
fn record_binding_lambda<'a>(
    vc: &VCtx<'a>,
    var: &str,
    value: &Expr,
    env: &mut Env<'a>,
    solver_opt: Option<&Solver<'a>>,
) {
    match resolve_lambda_expr(vc, value, env, solver_opt) {
        Some(lambda) => vc
            .local_lambdas
            .borrow_mut()
            .insert(var.to_string(), lambda),
        None => vc.local_lambdas.borrow_mut().remove(var),
    };
}

fn struct_array_element_handle_from_leaves<'a>(
    vc: &VCtx<'a>,
    array: &str,
    index: &Int<'a>,
    type_name: &str,
    prefix: &str,
    env: &mut Env<'a>,
    depth: usize,
) -> Result<Dynamic<'a>, MumeiError> {
    if depth > 16 {
        return Err(MumeiError::type_error(
            "nested struct-array element exceeds the supported depth",
        ));
    }
    let resolved = vc.module_env.resolve_base_type(type_name);
    let base = crate::verification::support::datatype::type_name_base(&resolved);
    let sdef = vc
        .module_env
        .get_struct(&resolved)
        .or_else(|| vc.module_env.get_struct(base))
        .ok_or_else(|| MumeiError::type_error(format!("'{type_name}' is not a struct")))?;
    let handle = fresh_struct_handle(vc.ctx, base);
    let handle_name = handle.decl().name().to_string();
    let mut fields = Vec::with_capacity(sdef.fields.len());
    for field in &sdef.fields {
        let path = if prefix.is_empty() {
            field.name.clone()
        } else {
            format!("{prefix}_{}", field.name)
        };
        let field_type = vc.module_env.resolve_base_type(&field.type_name);
        let field_base = crate::verification::support::datatype::type_name_base(&field_type);
        let nested = vc
            .module_env
            .get_struct(&field_type)
            .or_else(|| vc.module_env.get_struct(field_base));
        let value = if let Some(nested) = nested {
            struct_array_element_handle_from_leaves(
                vc,
                array,
                index,
                &nested.name,
                &path,
                env,
                depth + 1,
            )?
        } else {
            let leaf = super::z3_types::struct_array_leaf_array(vc, array, &path, &field_type, env);
            leaf.select(index)
        };
        fields.push((field.name.clone(), value));
    }
    bind_struct_fields(env, &handle_name, &fields);
    Ok(handle)
}

/// `q.f = v` on a declared struct binding: re-check the written field's
/// refined-type predicates and `where` constraint and the struct's
/// invariants against the merged field set, then re-home `q` on a fresh
/// handle so a sibling alias of `q` keeps projecting its own (unwritten)
/// field state. `None` falls through to the resource-state path.
fn struct_field_assign<'a>(
    vc: &VCtx<'a>,
    var: &str,
    value: &Expr,
    env: &mut Env<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> Result<Option<Dynamic<'a>>, MumeiError> {
    let ctx = vc.ctx;
    let Some((base, field)) = var.split_once('.') else {
        return Ok(None);
    };
    let Some(type_name) = vc.local_struct_types.borrow().get(base).cloned() else {
        return Ok(None);
    };
    let segments: Vec<&str> = field.split('.').collect();
    if segments.len() > 1 {
        if value.binds_name(base) {
            return Err(MumeiError::type_error(format!(
                "field write '{var}' value must not rebind '{base}'",
            )));
        }
        // `q.inner.a = v` desugars to a whole-subtree write of the
        // parent struct: `q.inner = Inner { a: v, <others>: q.inner.<f> }`.
        // Re-running the write against the parent path reuses the field
        // contract, invariant and fresh-handle logic at every depth —
        // recursion peels one segment per level.
        let mut parent_ty = type_name.as_str();
        for (index, segment) in segments[..segments.len() - 1].iter().enumerate() {
            let resolved_parent_ty = vc.module_env.resolve_base_type(parent_ty);
            let Some(sdef) = vc.module_env.get_struct(&resolved_parent_ty) else {
                return Ok(None);
            };
            let Some(f) = sdef.fields.iter().find(|f| f.name == *segment) else {
                return Ok(None);
            };
            parent_ty = f.type_name.as_str();
            let resolved_field_ty = vc.module_env.resolve_base_type(parent_ty);
            if index + 1 < segments.len() {
                if let Some(edef) = vc.module_env.get_enum(&resolved_field_ty) {
                    if !datatype::is_finite_adt(edef, vc.module_env) {
                        return Ok(None);
                    }
                    let prefix = segments[..=index].join(".");
                    let rest = segments[index + 1..].join(".");
                    let prefix_expr = segments[..=index]
                        .iter()
                        .fold(Expr::Variable(base.to_string()), |acc, segment| {
                            Expr::FieldAccess(Box::new(acc), (*segment).to_string())
                        });
                    let slot = expr_to_z3(vc, &prefix_expr, env, solver_opt)?;
                    let temp = enum_slot_temp_name(vc, env);
                    env.insert(temp.clone(), slot);
                    vc.local_enum_types
                        .borrow_mut()
                        .insert(temp.clone(), edef.name.clone());
                    let result = (|| {
                        let enum_path = format!("{temp}.{rest}");
                        let Some(written) =
                            enum_field_assign(vc, &enum_path, value, env, solver_opt)?
                        else {
                            return Ok(None);
                        };
                        let field_path = format!("{base}.{prefix}");
                        let temp_expr = Expr::Variable(temp.clone());
                        let Some(_) =
                            struct_field_assign(vc, &field_path, &temp_expr, env, solver_opt)?
                        else {
                            return Ok(None);
                        };
                        Ok(Some(written))
                    })();
                    remove_enum_slot_temp(vc, env, &temp);
                    return result;
                }
            }
        }
        let leaf = segments[segments.len() - 1];
        let resolved_parent_ty = vc.module_env.resolve_base_type(parent_ty);
        let Some(parent_sdef) = vc.module_env.get_struct(&resolved_parent_ty) else {
            return Ok(None);
        };
        if !parent_sdef.fields.iter().any(|f| f.name == leaf) {
            return Ok(None);
        }
        let parent_expr = segments[..segments.len() - 1]
            .iter()
            .fold(Expr::Variable(base.to_string()), |acc, segment| {
                Expr::FieldAccess(Box::new(acc), segment.to_string())
            });
        let val = expr_to_z3(vc, value, env, solver_opt)?;
        static RHS_TEMP_COUNTER: std::sync::atomic::AtomicUsize =
            std::sync::atomic::AtomicUsize::new(0);
        let rhs_name = loop {
            let id = RHS_TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let name = format!("__field_write_rhs_{id}");
            let occupied = env.contains_key(&name)
                || vc.local_struct_types.borrow().contains_key(&name)
                || vc.local_enum_types.borrow().contains_key(&name);
            if !occupied {
                break name;
            }
        };
        env.insert(rhs_name.clone(), val.clone());
        let init_fields: Vec<(String, Expr)> = parent_sdef
            .fields
            .iter()
            .map(|f| {
                if f.name == leaf {
                    (f.name.clone(), Expr::Variable(rhs_name.clone()))
                } else {
                    (
                        f.name.clone(),
                        Expr::FieldAccess(Box::new(parent_expr.clone()), f.name.clone()),
                    )
                }
            })
            .collect();
        let desugared = Expr::StructInit {
            type_name: parent_ty.to_string(),
            fields: init_fields,
        };
        let parent_var = format!("{}.{}", base, segments[..segments.len() - 1].join("."));
        let result = struct_field_assign(vc, &parent_var, &desugared, env, solver_opt);
        env.remove(&rhs_name);
        if result?.is_none() {
            return Ok(None);
        }
        return Ok(Some(val));
    }
    let Some(sdef) = vc.module_env.get_struct(&type_name) else {
        return Ok(None);
    };
    let Some(sfield) = sdef.fields.iter().find(|f| f.name == field) else {
        return Ok(None);
    };
    // A value that rebinds `base` (`q.x = { q.y = 2; 1 }`, a shadowing
    // `let q`, a whole-struct `q = …`) would leave the field snapshot
    // and the emitted `insertvalue` disagreeing about which `q` they
    // update — reject rather than pick a winner.
    if value.binds_name(base) {
        return Err(MumeiError::type_error(format!(
            "field write '{var}' value must not rebind '{base}'",
        )));
    }
    let Some(fields) = env
        .get(base)
        .and_then(|bound| struct_fields_of_value(env, bound, sdef))
    else {
        // A moved or otherwise unbound base falls through to the
        // resource-state error — the executor's move pre-pass rejects
        // genuine use-after-move before translation reaches here.
        return Ok(None);
    };
    let mut val = expr_to_z3(vc, value, env, solver_opt)?;
    let array_field = datatype::field_leaf_types(&sfield.type_name, vc.module_env)
        .is_some_and(|leaves| datatype::array_payload_element_type(&leaves).is_some());
    if array_field {
        let array = match value {
            Expr::Variable(name) => env
                .get(&format!("__z3_arr_{name}"))
                .and_then(|value| value.as_array())
                .or_else(|| val.as_array()),
            _ => val.as_array(),
        };
        if let Some(array) = array {
            let array_value: Dynamic = array.clone().into();
            let len =
                super::z3_types::array_len_for_expr(vc, env, Some(value), &array_value, solver_opt);
            super::z3_types::register_array_value_len(env, &array, len);
            val = array_value;
        }
    }
    let slot_key = format!("{base}_{field}");
    if let Some(slot) = env.get(&slot_key) {
        if val.get_sort() != slot.get_sort() {
            return Err(MumeiError::type_error(format!(
                "field '{var}' has type {} but value has type {}",
                slot.get_sort(),
                val.get_sort()
            )));
        }
    }
    // The write establishes the declared field contract: the refined-alias
    // predicates must hold unconditionally (only `Unsat` proves them) and
    // the `where v` clause must hold under the current path condition —
    // the same obligations `StructInit` runs per field.
    if let Some(solver) = solver_opt {
        for (decl, predicate) in field_type_predicates(vc, sfield, &val, env)? {
            solver.push();
            solver.assert(&vc.path_cond_conj());
            solver.assert(&predicate.not());
            let verdict = crate::verification::phase_artifacts::check(solver);
            solver.pop(1);
            if verdict != SatResult::Unsat {
                return Err(MumeiError::verification(format!(
                    "Struct '{type_name}' field '{field}' type '{decl}' predicate violated",
                ))
                .with_verdict(crate::verification::VerdictKind::Rejected));
            }
        }
        if let Some(constraint_raw) = &sfield.constraint {
            let mut local_env = env.clone();
            local_env.insert("v".to_string(), val.clone());
            let constraint_z3 =
                expr_to_z3(vc, &parse_expression(constraint_raw), &mut local_env, None)?;
            if let Some(constraint_bool) = constraint_z3.as_bool() {
                solver.push();
                solver.assert(&vc.path_cond_conj());
                solver.assert(&constraint_bool.not());
                let verdict = crate::verification::phase_artifacts::check(solver);
                solver.pop(1);
                if verdict == SatResult::Sat {
                    return Err(MumeiError::verification(format!(
                        "Struct '{type_name}' field '{field}' constraint violated: {constraint_raw}",
                    ))
                    .with_verdict(crate::verification::VerdictKind::Rejected));
                }
            }
        }
    }
    let merged: Vec<(String, Dynamic)> = fields
        .into_iter()
        .map(|(name, old)| {
            if name == field {
                (name, val.clone())
            } else {
                (name, old)
            }
        })
        .collect();
    if let Some(solver) = solver_opt {
        check_struct_invariants(
            vc,
            solver,
            sdef,
            &format!("field write to '{var}'"),
            &merged,
            env,
        )?;
    }
    // Re-home the binding on a fresh handle carrying the merged fields:
    // `env[base]` must not keep the old handle, because a sibling alias
    // (`let r = q`) binds `__struct_<old-handle>_<field>` projections —
    // updating those in place would rewrite the alias's view of `q`.
    let handle = fresh_struct_handle(ctx, &type_name);
    let handle_name = handle.decl().name();
    bind_struct_fields(env, &handle_name, &merged);
    bind_struct_fields(env, base, &merged);
    // A struct-typed field (`q.inner = w`) rebinds `q_inner` to the
    // new handle — its leaf keys (`__struct_q_inner_a`, `q_inner_a`)
    // would otherwise stay on the pre-write values and a later
    // `q.inner.a` read would resolve through the direct-path keys to
    // stale leaves. Projecting the written value's fields into the
    // `<base>_<field>` binding re-seeds them, including deeper nested
    // paths (flattened field names compose, e.g. `q_inner_inner_a`).
    for (name, value) in &merged {
        // Deep leaves re-seed too — a shallow bind leaves
        // `q.inner.b.c` reading the pre-write symbols for 3+ levels.
        // Seed under both the direct-path binding (`q_inner_b_c`) and
        // the new handle (`__struct_<handle>_inner_b_c`) so post-write
        // `result`-style projections pick up the deep leaves as well.
        let fields = nested_field_bindings(env, value);
        if !fields.is_empty() {
            bind_struct_fields(env, &format!("{base}_{name}"), &fields);
            bind_struct_fields(env, &format!("{handle_name}_{name}"), &fields);
        }
    }
    env.insert(base.to_string(), handle);
    profile_solver_assertion(vc, &format!("assign_{}", var), None);
    Ok(Some(val))
}

/// A named enum payload write is defined only when the payload belongs to
/// exactly one declared variant `V` and the tag is provably `V` under the
/// path condition. Deep paths through struct payloads reuse
/// `struct_field_assign` before rebuilding `e = V(sel_0(e),…,v,…)`, so the
/// ordinary field obligations apply at every depth. `None` falls through to
/// the resource-state error.
fn enum_field_assign<'a>(
    vc: &VCtx<'a>,
    var: &str,
    value: &Expr,
    env: &mut Env<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> Result<Option<Dynamic<'a>>, MumeiError> {
    let Some((base, field)) = var.split_once('.') else {
        return Ok(None);
    };
    let Some(type_name) = vc.local_enum_types.borrow().get(base).cloned() else {
        return Ok(None);
    };
    let Some(edef) = vc.module_env.get_enum(&type_name) else {
        return Ok(None);
    };
    let segments: Vec<&str> = field.split('.').collect();
    let payload_field = segments[0];
    // Admit a field owned by exactly one variant: an ambiguous `e.p`
    // could be a write under either tag and has no single slot to land
    // on.
    // `variant.fields` keeps a named payload's verbatim `name: type`
    // text; a positional payload (`Tcp(i64)`) carries no name a write
    // could address, matching the source model's `#index` slots.
    let mut owner: Option<(usize, usize)> = None;
    for (vidx, variant) in edef.variants.iter().enumerate() {
        let Some(fidx) = variant.fields.iter().position(|f| {
            f.split_once(':')
                .is_some_and(|(n, _)| n.trim() == payload_field)
        }) else {
            continue;
        };
        if owner.is_some() {
            return Err(MumeiError::type_error(format!(
                "enum field write '{var}': field '{payload_field}' is a payload of more than one variant — narrow the tag with a match",
            )));
        }
        owner = Some((vidx, fidx));
    }
    let Some((vidx, fidx)) = owner else {
        return Ok(None);
    };
    if let Some(target_enum) = datatype::recursive_field_target(
        edef,
        &edef.variants[vidx].field_types[fidx],
        vc.module_env,
    ) {
        return Err(MumeiError::type_error(format!(
            "enum field write '{var}' cannot update or traverse a recursive self payload targeting '{}'",
            target_enum.name
        )));
    }
    if value.binds_name(base) {
        return Err(MumeiError::type_error(format!(
            "field write '{var}' value must not rebind '{base}'",
        )));
    }
    let Some(target) = env.get(base).cloned() else {
        return Ok(None);
    };
    let variant_name = edef.variants[vidx].name.clone();
    let Some((sort, is_v, _, _)) = datatype::variant_tester_condition(vc, &target, &variant_name)
    else {
        return Ok(None);
    };
    // `path_cond ∧ ¬is_V(e)` must be unsatisfiable — the write is
    // defined only where the tag carries the owning variant.
    if let Some(solver) = solver_opt {
        solver.push();
        solver.assert(&vc.path_cond_conj());
        solver.assert(&is_v.not());
        let verdict = crate::verification::phase_artifacts::check(solver);
        solver.pop(1);
        if verdict != SatResult::Unsat {
            return Err(MumeiError::verification(format!(
                "enum field write '{var}' requires '{base}' to hold variant '{variant_name}' under the current path",
            ))
            .with_verdict(crate::verification::VerdictKind::Rejected));
        }
    }
    if segments.len() > 1 {
        let field_type = datatype::payload_field_type_name(&edef.variants[vidx].field_types[fidx]);
        let resolved_type = vc.module_env.resolve_base_type(&field_type);
        if let Some(inner_enum) = vc.module_env.get_enum(&resolved_type) {
            if !datatype::is_finite_adt(inner_enum, vc.module_env) {
                return Ok(None);
            }
            let Some((leaf_off, leaf_width)) =
                datatype::payload_leaf_range(edef, vc.module_env, vidx, fidx)
            else {
                return Ok(None);
            };
            if leaf_width != 1 {
                return Ok(None);
            }
            let Some(slot) = datatype::variant_selector_apply(&sort, vidx, leaf_off, &target)
            else {
                return Ok(None);
            };
            let temp = enum_slot_temp_name(vc, env);
            env.insert(temp.clone(), slot);
            vc.local_enum_types
                .borrow_mut()
                .insert(temp.clone(), inner_enum.name.clone());
            let result = (|| {
                let inner_path = format!("{temp}.{}", segments[1..].join("."));
                let Some(written) = enum_field_assign(vc, &inner_path, value, env, solver_opt)?
                else {
                    return Ok(None);
                };
                let payload_path = format!("{base}.{payload_field}");
                let temp_expr = Expr::Variable(temp.clone());
                let Some(_) = enum_field_assign(vc, &payload_path, &temp_expr, env, solver_opt)?
                else {
                    return Ok(None);
                };
                Ok(Some(written))
            })();
            remove_enum_slot_temp(vc, env, &temp);
            return result;
        }
        let Some(sdef) = vc.module_env.structs.get(&resolved_type) else {
            return Ok(None);
        };
        return enum_struct_payload_field_assign(
            vc,
            base,
            payload_field,
            &segments[1..].join("."),
            value,
            edef,
            vidx,
            fidx,
            &sort,
            &target,
            sdef,
            env,
            solver_opt,
        );
    }
    let val = expr_to_z3(vc, value, env, solver_opt)?;
    // A struct-typed payload slot flattens into `width` consecutive
    // accessor leaves; the write consumes the value's projected leaves
    // one-for-one. Scalar slots keep the single-accessor check.
    let (leaf_off, leaf_width) =
        datatype::payload_leaf_range(edef, vc.module_env, vidx, fidx).unwrap_or((fidx, 1));
    let field_type = datatype::payload_field_type_name(&edef.variants[vidx].field_types[fidx]);
    let composite_sdef = vc
        .module_env
        .structs
        .get(&vc.module_env.resolve_base_type(&field_type));
    let field_leaves = datatype::field_leaf_types(&field_type, vc.module_env);
    if composite_sdef.is_some()
        && !env.contains_key(ENUM_STRUCT_ARRAY_REBUILD_KEY)
        && field_leaves
            .as_ref()
            .is_some_and(|leaves| datatype::array_payload_leaf_pairs(leaves).next().is_some())
    {
        return Err(MumeiError::type_error(format!(
            "enum field write '{var}' cannot replace a struct payload containing array fields"
        )));
    }
    let array_payload = field_leaves
        .as_ref()
        .and_then(|leaves| datatype::array_payload_element_type(leaves))
        .is_some();
    let val = if array_payload {
        if let Expr::Variable(name) = value {
            env.get(&format!("__z3_arr_{name}"))
                .and_then(|value| value.as_array())
                .map(Into::into)
                .unwrap_or(val)
        } else {
            val
        }
    } else {
        val
    };
    let val_leaves: Vec<Dynamic> = if array_payload {
        if val.as_array().is_none() {
            return Err(MumeiError::type_error(format!(
                "enum field '{var}' expects an array value"
            )));
        }
        let length = super::z3_types::array_len_for_expr(vc, env, Some(value), &val, solver_opt);
        vec![val.clone(), length]
    } else if composite_sdef.is_some() {
        // Flatten deeper than direct fields — nested payloads bind leaves
        // like `inner_x`, keyed the same way `nested_field_bindings` reports
        // the value's env entries.
        let Some(fields) = field_leaves
            .and_then(|lt| datatype::flattened_value_leaves(vc, env, &val, &lt, solver_opt))
        else {
            return Err(MumeiError::type_error(format!(
                "enum field '{var}' expects a struct value"
            )));
        };
        fields
    } else {
        vec![val.clone()]
    };
    if val_leaves.len() != leaf_width {
        return Err(MumeiError::type_error(format!(
            "enum field '{var}' has {} payload leaves but the value supplies {}",
            leaf_width,
            val_leaves.len()
        )));
    }
    for (j, leaf) in val_leaves.iter().enumerate() {
        if let Some(sel) = datatype::variant_selector_apply(&sort, vidx, leaf_off + j, &target) {
            if leaf.get_sort() != sel.get_sort() {
                return Err(MumeiError::type_error(format!(
                    "enum field '{var}' leaf {j} has type {} but value has type {}",
                    sel.get_sort(),
                    leaf.get_sort()
                )));
            }
        }
    }
    // A payload declared with a refined-alias type must satisfy the
    // alias's conjuncts — match-arm payload binding assumes them on
    // the stored value, so admitting a violating write would let
    // later proofs lean on a predicate the write never established
    // (the same obligation `struct_field_assign` runs per field).
    if let Some(field_ty) = edef.variants[vidx].field_types.get(fidx) {
        if let Some(solver) = solver_opt {
            if vc.module_env.types.contains_key(&field_ty.name) {
                let chain = vc
                    .module_env
                    .refined_chain(&field_ty.name)
                    .map_err(MumeiError::type_error)?;
                for decl in chain {
                    let mut local_env = env.clone();
                    local_env.insert(decl.operand.clone(), val.clone());
                    let predicate_ast = parse_expression(&decl.predicate_raw);
                    let Some(predicate) =
                        refined_predicate_to_z3(vc, decl, &predicate_ast, &mut local_env, None)?
                            .as_bool()
                    else {
                        continue;
                    };
                    solver.push();
                    solver.assert(&vc.path_cond_conj());
                    solver.assert(&predicate.not());
                    let verdict = crate::verification::phase_artifacts::check(solver);
                    solver.pop(1);
                    if verdict != SatResult::Unsat {
                        return Err(MumeiError::verification(format!(
                            "enum field write '{var}': field '{field}' type '{}' predicate violated",
                            field_ty.name
                        ))
                        .with_verdict(crate::verification::VerdictKind::Rejected));
                    }
                }
            }
        }
    }
    // Rebuild `e = V(sel_0(e),…,v,…)` at the flattened leaf arity so
    // every accessor agrees with the written slot and the untouched
    // payload bits — composite fields contribute their selector leaves.
    let variant = &edef.variants[vidx];
    let total_leaves: usize = variant
        .field_types
        .iter()
        .enumerate()
        .map(|(i, _)| {
            datatype::payload_leaf_range(edef, vc.module_env, vidx, i)
                .map(|(_, w)| w)
                .unwrap_or(1)
        })
        .sum();
    let mut leaf_args: Vec<Dynamic> = Vec::with_capacity(total_leaves);
    for i in 0..variant.field_types.len() {
        let (off, width) =
            datatype::payload_leaf_range(edef, vc.module_env, vidx, i).unwrap_or((i, 1));
        if i == fidx {
            leaf_args.extend(val_leaves.iter().cloned());
        } else {
            for j in off..off + width {
                let Some(sel) = datatype::variant_selector_apply(&sort, vidx, j, &target) else {
                    return Ok(None);
                };
                leaf_args.push(sel);
            }
        }
    }
    if let Some(solver) = solver_opt {
        datatype::check_enum_field_payload_domains(vc, solver, edef, variant, fidx, &leaf_args)?;
    }
    let Some(rebuilt) = datatype::enum_ctor_apply_leaves(vc, edef, vidx, &leaf_args) else {
        return Ok(None);
    };
    env.insert(base.to_string(), rebuilt);
    profile_solver_assertion(vc, &format!("assign_{}", var), None);
    Ok(Some(val))
}

fn enum_payload_temp_name(vc: &VCtx<'_>, env: &Env<'_>) -> String {
    static TEMP_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    loop {
        let id = TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let name = format!("__enum_payload_{id}");
        let field_prefix = format!("{name}_");
        let struct_prefix = format!("__struct_{name}_");
        let occupied = env.keys().any(|key| {
            key == &name || key.starts_with(&field_prefix) || key.starts_with(&struct_prefix)
        }) || vc.local_struct_types.borrow().contains_key(&name)
            || vc.local_enum_types.borrow().contains_key(&name);
        if !occupied {
            return name;
        }
    }
}

fn enum_slot_temp_name(vc: &VCtx<'_>, env: &Env<'_>) -> String {
    static TEMP_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    loop {
        let id = TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let name = format!("__enum_slot_{id}");
        let field_prefix = format!("{name}_");
        let struct_prefix = format!("__struct_{name}_");
        let occupied = env.keys().any(|key| {
            key == &name || key.starts_with(&field_prefix) || key.starts_with(&struct_prefix)
        }) || vc.local_struct_types.borrow().contains_key(&name)
            || vc.local_enum_types.borrow().contains_key(&name);
        if !occupied {
            return name;
        }
    }
}

fn remove_enum_slot_temp(vc: &VCtx<'_>, env: &mut Env<'_>, temp: &str) {
    let field_prefix = format!("{temp}_");
    let struct_prefix = format!("__struct_{temp}_");
    let keys: Vec<String> = env
        .keys()
        .filter(|key| {
            key.as_str() == temp
                || key.starts_with(&field_prefix)
                || key.starts_with(&struct_prefix)
        })
        .cloned()
        .collect();
    for key in keys {
        env.remove(&key);
    }
    vc.local_enum_types.borrow_mut().remove(temp);
    vc.local_struct_types.borrow_mut().remove(temp);
}

fn collect_struct_handle_names(
    vc: &VCtx<'_>,
    env: &Env<'_>,
    value: &Dynamic<'_>,
    type_name: &str,
    depth: usize,
    handles: &mut std::collections::HashSet<String>,
) {
    if depth > 32 || value.kind() != z3::AstKind::App {
        return;
    }
    let decl = value.decl();
    if decl.kind() == z3::DeclKind::ITE {
        for child in value.children().iter().skip(1) {
            collect_struct_handle_names(vc, env, child, type_name, depth + 1, handles);
        }
        return;
    }
    if decl.kind() != z3::DeclKind::UNINTERPRETED || decl.arity() != 0 {
        return;
    }
    if !handles.insert(decl.name().to_string()) {
        return;
    }
    let resolved_type = vc.module_env.resolve_base_type(type_name);
    let Some(sdef) = vc.module_env.structs.get(&resolved_type) else {
        return;
    };
    let Some(fields) = struct_fields_of_value(env, value, sdef) else {
        return;
    };
    for field in &sdef.fields {
        let resolved_field_type = vc.module_env.resolve_base_type(&field.type_name);
        if vc.module_env.structs.contains_key(&resolved_field_type) {
            if let Some((_, field_value)) = fields.iter().find(|(name, _)| name == &field.name) {
                collect_struct_handle_names(
                    vc,
                    env,
                    field_value,
                    &field.type_name,
                    depth + 1,
                    handles,
                );
            }
        }
    }
}

fn enum_payload_path_type(vc: &VCtx<'_>, mut type_name: String, path: &str) -> Option<String> {
    for segment in path.split('.') {
        let resolved_type = vc.module_env.resolve_base_type(&type_name);
        let sdef = vc.module_env.structs.get(&resolved_type)?;
        type_name = sdef
            .fields
            .iter()
            .find(|field| field.name == segment)?
            .type_name
            .clone();
    }
    Some(type_name)
}

fn remove_struct_handle_fields(env: &mut Env<'_>, handles: &std::collections::HashSet<String>) {
    let prefixes: Vec<String> = handles
        .iter()
        .flat_map(|handle| [format!("__struct_{handle}_"), format!("{handle}_")])
        .collect();
    let keys: Vec<String> = env
        .keys()
        .filter(|key| prefixes.iter().any(|prefix| key.starts_with(prefix)))
        .cloned()
        .collect();
    for key in keys {
        env.remove(&key);
    }
}

#[allow(clippy::too_many_arguments)]
fn enum_struct_payload_field_assign<'a>(
    vc: &VCtx<'a>,
    base: &str,
    payload_field: &str,
    path: &str,
    value: &Expr,
    edef: &crate::parser::EnumDef,
    vidx: usize,
    fidx: usize,
    sort: &z3::DatatypeSort<'a>,
    target: &Dynamic<'a>,
    sdef: &StructDef,
    env: &mut Env<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> Result<Option<Dynamic<'a>>, MumeiError> {
    let field_type = datatype::payload_field_type_name(&edef.variants[vidx].field_types[fidx]);
    let Some(leaf_types) = datatype::field_leaf_types(&field_type, vc.module_env) else {
        return Ok(None);
    };
    let Some((leaf_off, leaf_width)) =
        datatype::payload_leaf_range(edef, vc.module_env, vidx, fidx)
    else {
        return Ok(None);
    };
    if leaf_width != leaf_types.len() {
        return Ok(None);
    }
    let mut leaves = Vec::with_capacity(leaf_width);
    for (offset, leaf_type) in leaf_types.iter().enumerate() {
        let Some(leaf) = datatype::variant_selector_apply(sort, vidx, leaf_off + offset, target)
        else {
            return Ok(None);
        };
        leaves.push((leaf_type.0.clone(), leaf));
    }

    let temp = enum_payload_temp_name(vc, env);
    let setup_keys_before: std::collections::HashSet<String> = env.keys().cloned().collect();
    let leaf_values: Vec<Dynamic> = leaves.iter().map(|(_, leaf)| leaf.clone()).collect();
    let Some(direct) = crate::verification::translator::pattern::rebuild_payload_direct_fields(
        vc.ctx,
        vc,
        env,
        sdef,
        &leaf_values,
    ) else {
        let new_keys: Vec<String> = env
            .keys()
            .filter(|key| !setup_keys_before.contains(*key))
            .cloned()
            .collect();
        for key in new_keys {
            env.remove(&key);
        }
        return Ok(None);
    };
    let handle = fresh_struct_handle(vc.ctx, &sdef.name);
    let handle_name = handle.decl().name();
    bind_struct_fields(env, &handle_name, &direct);
    let array_length_indices: std::collections::HashSet<usize> =
        datatype::array_payload_leaf_pairs(&leaf_types)
            .map(|(array_index, _)| array_index + 1)
            .collect();
    let struct_leaves: Vec<(String, Dynamic)> = leaves
        .iter()
        .enumerate()
        .filter(|(index, _)| !array_length_indices.contains(index))
        .map(|(_, leaf)| leaf.clone())
        .collect();
    bind_struct_fields(env, &temp, &struct_leaves);
    bind_struct_fields(env, &temp, &direct);
    env.insert(temp.clone(), handle);
    vc.local_struct_types
        .borrow_mut()
        .insert(temp.clone(), sdef.name.clone());
    let setup_keys: Vec<String> = env
        .keys()
        .filter(|key| !setup_keys_before.contains(*key))
        .cloned()
        .collect();

    let path_type = enum_payload_path_type(vc, sdef.name.clone(), path);
    let env_keys_before_write: std::collections::HashSet<String> = env.keys().cloned().collect();
    env.insert(
        ENUM_STRUCT_ARRAY_REBUILD_KEY.to_string(),
        Bool::from_bool(vc.ctx, true).into(),
    );
    let result = (|| {
        let temp_path = format!("{temp}.{path}");
        let Some(written) = struct_field_assign(vc, &temp_path, value, env, solver_opt)? else {
            return Ok(None);
        };
        let enum_path = format!("{base}.{payload_field}");
        let temp_expr = Expr::Variable(temp.clone());
        let Some(_) = enum_field_assign(vc, &enum_path, &temp_expr, env, solver_opt)? else {
            return Ok(None);
        };
        Ok(Some(written))
    })();
    env.remove(ENUM_STRUCT_ARRAY_REBUILD_KEY);

    let mut temp_handles = std::collections::HashSet::new();
    if let Some(temp_value) = env.get(&temp) {
        collect_struct_handle_names(vc, env, temp_value, &sdef.name, 0, &mut temp_handles);
    }
    if let (Ok(Some(written)), Some(path_type)) = (&result, path_type.as_deref()) {
        let mut result_handles = std::collections::HashSet::new();
        collect_struct_handle_names(vc, env, written, path_type, 0, &mut result_handles);
        temp_handles.retain(|handle| !result_handles.contains(handle));
    }
    remove_struct_handle_fields(env, &temp_handles);
    for key in setup_keys {
        env.remove(&key);
    }
    let temp_prefix = format!("{temp}_");
    let struct_prefix = format!("__struct_{temp}_");
    let temp_keys: Vec<String> = env
        .keys()
        .filter(|key| {
            key.as_str() == temp || key.starts_with(&temp_prefix) || key.starts_with(&struct_prefix)
        })
        .cloned()
        .collect();
    for key in temp_keys {
        env.remove(&key);
    }
    vc.local_struct_types.borrow_mut().remove(&temp);
    if result.is_err() || matches!(&result, Ok(None)) {
        let new_keys: Vec<String> = env
            .keys()
            .filter(|key| !env_keys_before_write.contains(*key))
            .cloned()
            .collect();
        for key in new_keys {
            env.remove(&key);
        }
    }
    result
}

/// `let a = arr` / `a = arr` where `arr` is an array name: alias the backing
/// Z3 array const and `len_<name>` symbol so `a[i]` accesses and stores share
/// `arr`'s constraint state instead of starting over with an unconstrained
/// fresh array (an unconstrained `len_a` would make `a[0]` unprovably in
/// bounds even when `forall`/store history pinned `len_arr`).
pub(crate) fn stmt_to_z3<'a>(
    vc: &VCtx<'a>,
    stmt: &Stmt,
    env: &mut Env<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> DynResult<'a> {
    let ctx = vc.ctx;
    match stmt {
        Stmt::Let { var, value, .. } => {
            reject_struct_projection_slot_collisions(vc, var, value, env)?;
            // A `let` may re-bind a composite name only to a value of
            // the SAME struct type (a full re-init overwrites every
            // `q_f`/`__struct_q_f` slot, so no stale value survives).
            // Shadowing with a scalar or a different struct leaves the
            // name-keyed field slots bound to the old struct's values —
            // reads through the new binding would see the stale slots.
            if let Some(existing) =
                crate::verification::support::datatype::binding_declared_type(vc, var)
                    .and_then(|ty| canonical_struct_type(vc, &ty))
            {
                let new_ty = match value.as_ref() {
                    Expr::StructInit { type_name, .. } => canonical_struct_type(vc, type_name),
                    _ => crate::verification::support::datatype::declared_type_of_expr(vc, value)
                        .and_then(|ty| canonical_struct_type(vc, &ty)),
                };
                if new_ty.as_deref() != Some(existing.as_str()) {
                    return Err(MumeiError::type_error(format!(
                        "let '{var}' shadows a struct binding with a different type — rename the local"
                    )));
                }
            }
            let val = match value.as_ref() {
                Expr::ArrayAccess(array, index) => {
                    super::expr::bind_struct_array_element(vc, var, array, index, env, solver_opt)?
                        .map(Ok)
                        .unwrap_or_else(|| expr_to_z3(vc, value, env, solver_opt))?
                }
                _ => expr_to_z3(vc, value, env, solver_opt)?,
            };
            let shared = val.as_array().is_some() && array_binding_is_shared_source(value, env);
            super::z3_types::clear_array_shared(ctx, env, var);
            record_binding_enum_type(vc, var, value);
            record_binding_int_type(vc, var, value);
            record_binding_struct_type(vc, var, value);
            record_binding_lambda(vc, var, value, env, solver_opt);
            env.insert(var.clone(), val.clone());
            alias_struct_fields(env, var, &val);
            rebind_struct_binding_slots(vc, var, &val, env, solver_opt);
            wire_array_slots(vc, var, Some(value), &val, env, solver_opt)?;
            if shared {
                super::z3_types::mark_array_shared(ctx, env, var);
            }
            profile_solver_assertion(vc, &format!("let_{}", var), None);
            Ok(val)
        }
        Stmt::Assign { var, value, .. } => {
            if var.contains('.') {
                mark_array_source_shared(vc, env, value);
                let Some((resource, _field)) = vc.module_env.is_resource_state_var(var) else {
                    if let Some(val) = struct_field_assign(vc, var, value, env, solver_opt)? {
                        return Ok(val);
                    }
                    if let Some(val) = enum_field_assign(vc, var, value, env, solver_opt)? {
                        return Ok(val);
                    }
                    return Err(MumeiError::verification(format!(
                        "unknown resource state '{var}'"
                    )));
                };
                if resource.mode == crate::parser::ResourceMode::Shared {
                    return Err(MumeiError::verification(format!(
                        "cannot write shared state '{var}' under mode: shared (read-only)"
                    )));
                }
                if !env.contains_key(var) {
                    return Err(MumeiError::verification(format!(
                        "shared state '{var}' written outside 'acquire {}'",
                        resource.name
                    )));
                }
                let val = expr_to_z3(vc, value, env, solver_opt)?;
                let expected: Dynamic = match _field.ty.as_str() {
                    "bool" => Bool::from_bool(vc.ctx, false).into(),
                    "i64" if vc.bitvec_i64 => BV::from_i64(vc.ctx, 0, 64).into(),
                    "i64" => Int::from_i64(vc.ctx, 0).into(),
                    "f64" if vc.ieee754_f64 => Float::from_f64(vc.ctx, 0.0).into(),
                    "f64" => Real::from_real(vc.ctx, 0, 1).into(),
                    _ => unreachable!("resource field types are parser-validated"),
                };
                if val.get_sort() != expected.get_sort() {
                    return Err(MumeiError::type_error(format!(
                        "shared state '{var}' has type {} but value has type {}",
                        _field.ty,
                        val.get_sort()
                    )));
                }
                env.insert(var.clone(), val.clone());
                profile_solver_assertion(vc, &format!("assign_{}", var), None);
                return Ok(val);
            }
            reject_struct_projection_slot_collisions(vc, var, value, env)?;
            let val = match value.as_ref() {
                Expr::ArrayAccess(array, index) => {
                    super::expr::bind_struct_array_element(vc, var, array, index, env, solver_opt)?
                        .map(Ok)
                        .unwrap_or_else(|| expr_to_z3(vc, value, env, solver_opt))?
                }
                _ => expr_to_z3(vc, value, env, solver_opt)?,
            };
            let shared = val.as_array().is_some() && array_binding_is_shared_source(value, env);
            super::z3_types::clear_array_shared(ctx, env, var);
            record_binding_enum_type(vc, var, value);
            record_binding_int_type(vc, var, value);
            record_binding_struct_type(vc, var, value);
            record_binding_lambda(vc, var, value, env, solver_opt);
            env.insert(var.clone(), val.clone());
            alias_struct_fields(env, var, &val);
            rebind_struct_binding_slots(vc, var, &val, env, solver_opt);
            wire_array_slots(vc, var, Some(value), &val, env, solver_opt)?;
            if shared {
                super::z3_types::mark_array_shared(ctx, env, var);
            }
            profile_solver_assertion(vc, &format!("assign_{}", var), None);
            Ok(val)
        }
        Stmt::ArrayStore {
            array,
            index,
            field_path,
            value,
            ..
        } => {
            let field_path = field_path.as_ref();
            if super::z3_types::is_array_shared(env, array) {
                return Err(super::z3_types::shared_array_mutation_error(array));
            }
            let val = expr_to_z3(vc, value, env, solver_opt)?;
            // Indices stay `Int`-sorted under `--bitvec-i64`; bridge a `BV(64)`
            // index through the signed bit-vector/integer conversion.
            let idx_value = expr_to_z3(vc, index, env, solver_opt)?;
            let idx = as_int_like(&idx_value)
                .ok_or(MumeiError::type_error("Array index must be integer"))?;
            let element_type = super::z3_types::array_element_type_name(array, vc);
            super::z3_types::validate_array_element_type(vc, &element_type)?;
            let element_sort = super::z3_types::array_element_sort_from_type(
                &element_type,
                vc.module_env,
                vc.ieee754_f64,
            );
            let struct_leaf_values = if let Some(field_path) = field_path {
                if element_sort != super::z3_types::ArrayElementSort::Struct {
                    return Err(MumeiError::type_error(format!(
                        "field store requires an array of structs, found '{element_type}'"
                    )));
                }
                let leaves = super::z3_types::struct_array_leaf_types(&element_type, vc.module_env)
                    .ok_or_else(|| {
                        MumeiError::type_error(format!(
                            "array element struct '{element_type}' must have only scalar leaves"
                        ))
                    })?;
                let path = field_path.join("_");
                let leaf_type = leaves
                    .iter()
                    .find(|(leaf_path, _)| leaf_path == &path)
                    .map(|(_, leaf_type)| leaf_type)
                    .ok_or_else(|| {
                        MumeiError::type_error(format!(
                            "array element struct '{element_type}' has no scalar leaf '{path}'"
                        ))
                    })?;
                let sort = super::z3_types::array_element_sort_from_type(
                    leaf_type,
                    vc.module_env,
                    vc.ieee754_f64,
                );
                let field_value = super::z3_types::coerce_to_array_elem_sort(vc, sort, &val)?;
                if sort == super::z3_types::ArrayElementSort::Enum {
                    super::z3_types::check_enum_array_store_type(vc, leaf_type, &field_value)?;
                }
                let prior_env_keys: std::collections::HashSet<String> =
                    env.keys().cloned().collect();
                let prior_struct_types: std::collections::HashSet<String> =
                    vc.local_struct_types.borrow().keys().cloned().collect();
                let prior_enum_types: std::collections::HashSet<String> =
                    vc.local_enum_types.borrow().keys().cloned().collect();
                let element = struct_array_element_handle_from_leaves(
                    vc,
                    array,
                    &idx,
                    &element_type,
                    "",
                    env,
                    0,
                )?;
                let element_name = element.decl().name().to_string();
                vc.local_struct_types
                    .borrow_mut()
                    .insert(element_name.clone(), element_type.clone());
                env.insert(element_name.clone(), element);
                let rhs_name = enum_slot_temp_name(vc, env);
                env.insert(rhs_name.clone(), field_value);
                let validation = (|| {
                    let field = format!("{element_name}.{}", field_path.join("."));
                    struct_field_assign(
                        vc,
                        &field,
                        &Expr::Variable(rhs_name.clone()),
                        env,
                        solver_opt,
                    )?
                    .ok_or_else(|| {
                        MumeiError::type_error(format!(
                            "array element struct '{element_type}' has no field '{}'",
                            field_path.join(".")
                        ))
                    })?;
                    let updated = env.get(&element_name).cloned().ok_or_else(|| {
                        MumeiError::type_error("failed to rebuild struct-array element")
                    })?;
                    super::expr::struct_array_leaf_values_from_value(
                        vc,
                        env,
                        &updated,
                        &element_type,
                    )
                    .map(|values| {
                        values
                            .into_iter()
                            .map(|(path, _, value)| (path, value))
                            .collect::<Vec<_>>()
                    })
                })();
                env.retain(|key, _| {
                    prior_env_keys.contains(key)
                        || (!key.starts_with("__mumei_struct_") && !key.starts_with("__struct_"))
                });
                vc.local_struct_types.borrow_mut().remove(&element_name);
                vc.local_struct_types
                    .borrow_mut()
                    .retain(|name, _| prior_struct_types.contains(name));
                vc.local_enum_types
                    .borrow_mut()
                    .retain(|name, _| prior_enum_types.contains(name));
                env.remove(&rhs_name);
                Some(validation?)
            } else if element_sort == super::z3_types::ArrayElementSort::Struct {
                let value_type =
                    super::expr::struct_array_expr_type(vc, value).ok_or_else(|| {
                        MumeiError::type_error(format!(
                            "array store value must be a struct of type '{element_type}'"
                        ))
                    })?;
                let resolved_element_type = vc.module_env.resolve_base_type(&element_type);
                let element_base =
                    crate::verification::support::datatype::type_name_base(&resolved_element_type)
                        .to_owned();
                if value_type != element_base {
                    return Err(MumeiError::type_error(format!(
                        "array store value has struct type '{value_type}', expected '{element_type}'"
                    )));
                }
                let leaves = super::z3_types::struct_array_leaf_types(&element_type, vc.module_env)
                    .ok_or_else(|| {
                        MumeiError::type_error(format!(
                            "array element struct '{element_type}' must have only scalar leaves"
                        ))
                    })?;
                let values =
                    super::expr::struct_array_leaf_values_from_value(vc, env, &val, &element_type)?;
                Some(
                    leaves
                        .into_iter()
                        .map(|(path, leaf_type)| {
                            let value = values
                                .iter()
                                .find(|(value_path, _, _)| value_path == &path)
                                .map(|(_, _, value)| value)
                                .ok_or_else(|| {
                                    MumeiError::type_error(format!(
                                        "struct array store is missing leaf '{path}'"
                                    ))
                                })?;
                            let sort = super::z3_types::array_element_sort_from_type(
                                &leaf_type,
                                vc.module_env,
                                vc.ieee754_f64,
                            );
                            let value =
                                super::z3_types::coerce_to_array_elem_sort(vc, sort, value)?;
                            Ok((path, value))
                        })
                        .collect::<Result<Vec<_>, MumeiError>>()?,
                )
            } else {
                None
            };
            let stored_val = if field_path.is_some() {
                val.clone()
            } else if struct_leaf_values.is_some() {
                val.as_int().map(Into::into).ok_or_else(|| {
                    MumeiError::type_error(format!(
                        "array store value must be a struct of type '{element_type}'"
                    ))
                })?
            } else {
                coerce_array_store_value(vc, array, val)?
            };

            // OOB check mirrors `Expr::ArrayAccess`: store at an index that may
            // fall outside `[0, len_<name>)` is flagged as a verification
            // error with a counter-example hint.
            if let Some(solver) = solver_opt {
                let len = array_len_value(ctx, env, array, vc.bitvec_i64, Some(solver));
                let safe = index_in_bounds(ctx, &idx_value, &len)
                    .ok_or(MumeiError::type_error("Array index must be integer"))?;
                solver.push();
                for cond in vc.path_cond_stack.borrow().iter() {
                    solver.assert(cond);
                }
                solver.assert(&safe.not());
                if crate::verification::phase_artifacts::check(solver) == SatResult::Sat {
                    solver.pop(1);
                    return Err(MumeiError::verification(format!(
                        "Potential Out-of-Bounds store on '{}' (index may be < 0 or >= len_{})",
                        array, array
                    ))
                    .with_help(
                        "requires にストアインデックスの範囲制約 (0 <= idx < len) を追加してください",
                    )
                    .with_verdict(crate::verification::VerdictKind::Rejected));
                }
                solver.pop(1);
            }

            if field_path.is_none() {
                let arr_key = format!("__z3_arr_{}", array);
                let current_arr = z3_dynamic_array(vc, array, env);
                let carrier_root = super::z3_types::array_root_ast(&current_arr);
                let new_arr = current_arr.store(&idx, &stored_val);
                let carrier_aliases: Vec<String> = env
                    .iter()
                    .filter(|(_, value)| {
                        value.as_array().is_some_and(|candidate| {
                            super::z3_types::array_root_ast(&candidate) == carrier_root
                        })
                    })
                    .map(|(key, _)| key.clone())
                    .collect();
                if carrier_aliases.is_empty() {
                    env.insert(arr_key, new_arr.into());
                } else {
                    for key in carrier_aliases {
                        env.insert(key, new_arr.clone().into());
                    }
                }
            }
            if let Some(leaves) = struct_leaf_values {
                let leaf_types =
                    super::z3_types::struct_array_leaf_types(&element_type, vc.module_env)
                        .expect("validated struct array retains its leaf types");
                for (path, value) in leaves {
                    let leaf_type = leaf_types
                        .iter()
                        .find(|(leaf_path, _)| leaf_path == &path)
                        .map(|(_, leaf_type)| leaf_type)
                        .expect("validated struct array retains its leaf type");
                    let leaf_array =
                        super::z3_types::struct_array_leaf_array(vc, array, &path, leaf_type, env);
                    let leaf_root = super::z3_types::array_root_ast(&leaf_array);
                    let updated = leaf_array.store(&idx, &value);
                    let aliases: Vec<String> = env
                        .iter()
                        .filter(|(_, current)| {
                            current.as_array().is_some_and(|candidate| {
                                super::z3_types::array_root_ast(&candidate) == leaf_root
                            })
                        })
                        .map(|(key, _)| key.clone())
                        .collect();
                    if aliases.is_empty() {
                        env.insert(
                            super::z3_types::struct_array_leaf_key(array, &path),
                            updated.into(),
                        );
                    } else {
                        for key in aliases {
                            env.insert(key, updated.clone().into());
                        }
                    }
                }
            }
            profile_solver_assertion(vc, &format!("array_store_{}", array), None);

            Ok(stored_val)
        }
        Stmt::Block(stmts, _) => {
            let mut last: Dynamic = Int::from_i64(ctx, 0).into();
            for s in stmts {
                last = stmt_to_z3(vc, s, env, solver_opt)?;
            }
            Ok(last)
        }
        Stmt::While {
            cond,
            invariant,
            decreases,
            body,
            span,
        } => {
            // Loop Invariant 検証ロジック
            if let Some(solver) = solver_opt {
                let mut loop_shared_markers = std::collections::HashMap::new();
                // Vars the body assigns that were bound before the loop —
                // they are havoced so induction is checked from *any* state
                // satisfying the invariant, not only the concrete entry
                // state, which would mask violations on later iterations.
                let mut modified = std::collections::HashSet::new();
                collect_assigned_vars(env, vc, body, &mut modified);
                // `__z3_arr_<v>` for a param array only materializes on first
                // access, so a loop that stores into `v` without an earlier
                // `v[i]` read finds no slot in `env` — the retain below would
                // drop it and the post-loop state would read the *entry*
                // array, letting stale `requires` facts wrong-verify.
                // Materialize the slot (aliased to the same const) so the
                // array is havoced like a local literal's slot is.
                for name in modified.clone() {
                    if let Some(var) = name.strip_prefix("__z3_arr_") {
                        if !env.contains_key(&name) {
                            if let Some(arr) = env.get(var).and_then(|d| d.as_array()) {
                                env.insert(name, arr.into());
                            }
                        }
                    }
                }
                modified.retain(|name| env.contains_key(name));
                let modified_btree: std::collections::BTreeSet<String> =
                    modified.iter().cloned().collect();
                let modified_bases: std::collections::BTreeSet<String> = modified_btree
                    .iter()
                    .map(|name| name.strip_prefix("__z3_arr_").unwrap_or(name).to_string())
                    .collect();
                let inferred = matches!(
                    invariant.as_ref(),
                    Expr::Variable(name) if name == "__mumei_infer_invariant"
                );
                let mut effective_invariant = invariant.as_ref().clone();
                let mut inferred_adopted = Vec::new();
                if inferred {
                    let loop_id = {
                        let mut counter = vc.loop_counter.borrow_mut();
                        let id = *counter;
                        *counter += 1;
                        id
                    };
                    let init_prefix = format!("__loop{loop_id}_init_");
                    for base in &modified_bases {
                        let snapshot_key = format!("{init_prefix}{base}");
                        if env.contains_key(&snapshot_key) {
                            return Err(MumeiError::verification(format!(
                                "identifier '{snapshot_key}' is reserved for loop invariant inference"
                            )));
                        }
                        let array_key = format!("__z3_arr_{base}");
                        if let Some(value) = env
                            .get(&array_key)
                            .cloned()
                            .or_else(|| env.get(base).cloned())
                        {
                            env.insert(snapshot_key, value);
                        }
                    }
                    let candidates = generate_candidates(cond, body, &modified_btree, &init_prefix);
                    for candidate in &candidates {
                        let combined = if inferred_adopted.is_empty() {
                            candidate.clone()
                        } else {
                            inferred_adopted.iter().cloned().fold(
                                candidate.clone(),
                                |acc, adopted| {
                                    Expr::BinaryOp(
                                        Box::new(adopted),
                                        crate::parser::Op::And,
                                        Box::new(acc),
                                    )
                                },
                            )
                        };
                        if matches!(
                            check_while_invariant(
                                vc,
                                &combined,
                                cond,
                                body,
                                env,
                                solver,
                                &modified_btree,
                            ),
                            Ok(true)
                        ) {
                            inferred_adopted.push(candidate.clone());
                        }
                    }
                    if inferred_adopted.is_empty() {
                        return Err(MumeiError::verification(format!(
                            "while loop requires an 'invariant' clause: inference tried {} candidates, none verified",
                            candidates.len()
                        )));
                    }
                    effective_invariant = inferred_adopted
                        .iter()
                        .cloned()
                        .reduce(|left, right| {
                            Expr::BinaryOp(Box::new(left), crate::parser::Op::And, Box::new(right))
                        })
                        .expect("inference adopted at least one candidate");
                    if let Some(report) = vc.inferred_invariants {
                        let entry = InferredInvariant {
                            line: span.line,
                            column: span.col,
                            candidates_tried: candidates.len(),
                            adopted: inferred_adopted.iter().map(expr_to_string).collect(),
                        };
                        let mut reports = report.borrow_mut();
                        if let Some(existing) = reports
                            .iter_mut()
                            .find(|item| item.line == entry.line && item.column == entry.column)
                        {
                            *existing = entry;
                        } else {
                            reports.push(entry);
                        }
                    }
                }

                let marks = obligation_marks(vc);
                let inv = with_array_shared_markers_restored(env, |env| {
                    expr_to_z3(vc, &effective_invariant, env, None)
                })?
                .as_bool()
                .ok_or(MumeiError::type_error("Invariant must be boolean"))?;
                rebind_deferred_obligations(vc, marks, &inv);
                let path_cond = vc.path_cond_conj();
                solver.push();
                solver.assert(&Bool::and(ctx, &[&path_cond, &inv.not()]));
                if crate::verification::phase_artifacts::check(solver) == SatResult::Sat {
                    solver.pop(1);
                    return Err(MumeiError::verification("Invariant fails initially")
                        .with_verdict(crate::verification::VerdictKind::Rejected));
                }
                solver.pop(1);

                // Inductive step — on a havoced env: the invariant must be
                // preserved from ANY state satisfying it, not just the concrete
                // loop-entry bindings.
                {
                    let env_snapshot = env.clone();
                    let types_snapshot = vc.local_enum_types.borrow().clone();
                    let int_types_snapshot = vc.local_int_types.borrow().clone();
                    let lambdas_snapshot = vc.local_lambdas.borrow().clone();
                    let mut step_env = env.clone();
                    havoc_vars(vc, &mut step_env, &modified, Some(solver));
                    let marks = obligation_marks(vc);
                    let inv_h = with_array_shared_markers_restored(&mut step_env, |env| {
                        expr_to_z3(vc, &effective_invariant, env, None)
                    })?
                    .as_bool()
                    .ok_or(MumeiError::type_error("Invariant must be boolean"))?;
                    rebind_deferred_obligations(vc, marks, &inv_h);
                    let marks = obligation_marks(vc);
                    let c_h = expr_to_z3(vc, cond, &mut step_env, None)?
                        .as_bool()
                        .ok_or(MumeiError::type_error("While condition must be boolean"))?;
                    rebind_deferred_obligations(vc, marks, &inv_h);
                    solver.push();
                    solver.assert(&inv_h);
                    solver.assert(&c_h);
                    stmt_to_z3(vc, body, &mut step_env, Some(solver))?;
                    loop_shared_markers.extend(collect_array_shared_markers(&step_env));
                    let inv_after = with_array_shared_markers_restored(&mut step_env, |env| {
                        expr_to_z3(vc, &effective_invariant, env, None)
                    })?
                    .as_bool()
                    .ok_or(MumeiError::type_error("Invariant must be boolean"))?;
                    solver.assert(&inv_after.not());
                    if crate::verification::phase_artifacts::check(solver) == SatResult::Sat {
                        solver.pop(1);
                        return Err(MumeiError::verification("Invariant not preserved")
                            .with_verdict(crate::verification::VerdictKind::Rejected));
                    }
                    solver.pop(1);
                    *env = env_snapshot;
                    *vc.local_enum_types.borrow_mut() = types_snapshot;
                    *vc.local_int_types.borrow_mut() = int_types_snapshot.clone();
                    *vc.local_lambdas.borrow_mut() = lambdas_snapshot;
                }
                let invariant = &effective_invariant;

                // Termination Check — again under havoced pre-state.
                if let Some(dec_expr) = decreases {
                    let env_snapshot = env.clone();
                    let types_snapshot = vc.local_enum_types.borrow().clone();
                    let int_types_snapshot = vc.local_int_types.borrow().clone();
                    let lambdas_snapshot = vc.local_lambdas.borrow().clone();
                    let mut term_env = env.clone();
                    havoc_vars(vc, &mut term_env, &modified, Some(solver));
                    let marks = obligation_marks(vc);
                    let inv_h = with_array_shared_markers_restored(&mut term_env, |env| {
                        expr_to_z3(vc, invariant, env, None)
                    })?
                    .as_bool()
                    .ok_or(MumeiError::type_error("Invariant must be boolean"))?;
                    rebind_deferred_obligations(vc, marks, &inv_h);
                    let marks = obligation_marks(vc);
                    let c_h = expr_to_z3(vc, cond, &mut term_env, None)?
                        .as_bool()
                        .ok_or(MumeiError::type_error("While condition must be boolean"))?;
                    rebind_deferred_obligations(vc, marks, &inv_h);
                    let marks = obligation_marks(vc);
                    let v_before =
                        as_int_like(&with_array_shared_markers_restored(&mut term_env, |env| {
                            expr_to_z3(vc, dec_expr, env, None)
                        })?)
                        .ok_or(MumeiError::type_error(
                            "decreases expression must be integer",
                        ))?;
                    rebind_deferred_obligations(vc, marks, &inv_h);
                    solver.push();
                    solver.assert(&inv_h);
                    solver.assert(&c_h);
                    solver.assert(&v_before.lt(&Int::from_i64(ctx, 0)));
                    if crate::verification::phase_artifacts::check(solver) == SatResult::Sat {
                        solver.pop(1);
                        *env = env_snapshot;
                        *vc.local_enum_types.borrow_mut() = types_snapshot.clone();
                        *vc.local_int_types.borrow_mut() = int_types_snapshot.clone();
                        *vc.local_lambdas.borrow_mut() = lambdas_snapshot.clone();
                        return Err(MumeiError::verification(
                            "Termination check failed: decreases expression may be negative",
                        )
                        .with_verdict(crate::verification::VerdictKind::Rejected));
                    }
                    solver.pop(1);
                    solver.push();
                    solver.assert(&inv_h);
                    solver.assert(&c_h);
                    stmt_to_z3(vc, body, &mut term_env, Some(solver))?;
                    loop_shared_markers.extend(collect_array_shared_markers(&term_env));
                    let v_after =
                        as_int_like(&with_array_shared_markers_restored(&mut term_env, |env| {
                            expr_to_z3(vc, dec_expr, env, None)
                        })?)
                        .ok_or(MumeiError::type_error(
                            "decreases expression must be integer",
                        ))?;
                    solver.assert(&v_after.ge(&v_before));
                    if crate::verification::phase_artifacts::check(solver) == SatResult::Sat {
                        solver.pop(1);
                        *env = env_snapshot;
                        *vc.local_enum_types.borrow_mut() = types_snapshot.clone();
                        *vc.local_int_types.borrow_mut() = int_types_snapshot.clone();
                        *vc.local_lambdas.borrow_mut() = lambdas_snapshot.clone();
                        return Err(MumeiError::verification(
                            "Termination check failed: decreases expression does not strictly decrease"
                        )
                        .with_verdict(crate::verification::VerdictKind::Rejected));
                    }
                    solver.pop(1);
                    *env = env_snapshot;
                    *vc.local_enum_types.borrow_mut() = types_snapshot.clone();
                    *vc.local_int_types.borrow_mut() = int_types_snapshot.clone();
                    *vc.local_lambdas.borrow_mut() = lambdas_snapshot.clone();
                }

                // Post-loop state: havoc the loop-carried vars once more and
                // assert `invariant ∧ ¬cond` so downstream statements and the
                // `ensures` check see exit facts (previously env kept
                // pre-loop bindings and no exit facts reached the solver).
                let mut post_env = env.clone();
                havoc_vars(vc, &mut post_env, &modified, Some(solver));
                let marks = obligation_marks(vc);
                let inv_post = with_array_shared_markers_restored(&mut post_env, |env| {
                    expr_to_z3(vc, invariant, env, None)
                })?
                .as_bool()
                .ok_or(MumeiError::type_error("Invariant must be boolean"))?;
                rebind_deferred_obligations(vc, marks, &inv_post);
                let marks = obligation_marks(vc);
                let c_not_post = expr_to_z3(vc, cond, &mut post_env, None)?
                    .as_bool()
                    .ok_or(MumeiError::type_error("While condition must be boolean"))?
                    .not();
                rebind_deferred_obligations(vc, marks, &inv_post);
                solver.assert(&Bool::and(ctx, &[&inv_post, &c_not_post]));
                post_env.extend(loop_shared_markers);
                *env = post_env;
                return Ok(Bool::and(ctx, &[&inv_post, &c_not_post]).into());
            }

            let inv = with_array_shared_markers_restored(env, |env| {
                expr_to_z3(vc, invariant, env, None)
            })?
            .as_bool()
            .ok_or(MumeiError::type_error("Invariant must be boolean"))?;
            let c_not = expr_to_z3(vc, cond, env, None)?
                .as_bool()
                .ok_or(MumeiError::type_error("While condition must be boolean"))?
                .not();
            Ok(Bool::and(ctx, &[&inv, &c_not]).into())
        }
        Stmt::Acquire { resource, body, .. } => {
            let held_name = format!("__resource_held_{}", resource);
            let held_bool = Bool::new_const(ctx, held_name.as_str());
            let outermost = {
                let mut held = vc.held_resources.borrow_mut();
                let depth = held.entry(resource.clone()).or_insert(0);
                let outermost = *depth == 0;
                *depth += 1;
                outermost
            };
            if let Some(resource_def) = vc.module_env.get_resource(resource) {
                if outermost {
                    let acquire_id = {
                        let mut counter = vc.acquire_counter.borrow_mut();
                        let id = *counter;
                        *counter += 1;
                        id
                    };
                    for field in &resource_def.state {
                        let key = format!("{resource}.{}", field.name);
                        let value = crate::verification::support::datatype::param_z3_value_for_vc(
                            vc,
                            &format!("{key}#acq{acquire_id}"),
                            Some(&field.ty),
                        );
                        env.insert(key, value);
                    }
                    if let (Some(invariant), Some(solver)) =
                        (resource_def.invariant.as_ref(), solver_opt)
                    {
                        let inv = with_array_shared_markers_restored(env, |env| {
                            expr_to_z3(vc, invariant, env, Some(solver))
                        })?
                        .as_bool()
                        .ok_or(MumeiError::type_error("resource invariant must be boolean"))?;
                        solver.assert(&inv);
                    }
                }
            }
            if let Some(solver) = solver_opt {
                solver.assert(&held_bool);
            }
            env.insert(held_name.clone(), held_bool.into());
            let body_result = stmt_to_z3(vc, body, env, solver_opt)?;
            if outermost {
                if let (Some(resource_def), Some(solver)) =
                    (vc.module_env.get_resource(resource), solver_opt)
                {
                    if let Some(invariant) = resource_def.invariant.as_ref() {
                        let inv_after = with_array_shared_markers_restored(env, |env| {
                            expr_to_z3(vc, invariant, env, Some(solver))
                        })?
                        .as_bool()
                        .ok_or(MumeiError::type_error("resource invariant must be boolean"))?;
                        solver.push();
                        solver.assert(&inv_after.not());
                        let result = crate::verification::phase_artifacts::check(solver);
                        solver.pop(1);
                        match result {
                            z3::SatResult::Sat => {
                                return Err(MumeiError::verification(format!(
                                    "shared invariant of resource '{}' not re-established at unlock: {}",
                                    resource,
                                    crate::verification::support::expr_to_source_string(invariant)
                                ))
                                .with_verdict(crate::verification::VerdictKind::Rejected));
                            }
                            z3::SatResult::Unknown => {
                                return Err(MumeiError::verification(format!(
                                    "shared invariant of resource '{}' not re-established at unlock: {} (solver returned unknown)",
                                    resource,
                                    crate::verification::support::expr_to_source_string(invariant)
                                ))
                                .with_verdict(crate::verification::VerdictKind::Inconclusive));
                            }
                            z3::SatResult::Unsat => {}
                        }
                    }
                }
                if let Some(resource_def) = vc.module_env.get_resource(resource) {
                    for field in &resource_def.state {
                        env.remove(&format!("{resource}.{}", field.name));
                    }
                }
            }
            let mut held = vc.held_resources.borrow_mut();
            if let Some(depth) = held.get_mut(resource) {
                *depth -= 1;
                if *depth == 0 {
                    held.remove(resource);
                }
            }
            let released = Bool::from_bool(ctx, false);
            env.insert(held_name, released.into());
            Ok(body_result)
        }
        Stmt::Task { body, group, .. } => {
            static TASK_COUNTER: std::sync::atomic::AtomicUsize =
                std::sync::atomic::AtomicUsize::new(0);
            let task_uid = TASK_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let task_id = format!(
                "__task_{}_{}",
                group.as_deref().unwrap_or("default"),
                task_uid
            );
            let task_alive = Bool::new_const(ctx, format!("{}_alive", task_id).as_str());
            env.insert(format!("{}_alive", task_id), task_alive.into());
            let body_result = stmt_to_z3(vc, body, env, solver_opt)?;
            let task_done = Bool::new_const(ctx, format!("{}_done", task_id).as_str());
            env.insert(format!("{}_done", task_id), task_done.into());
            Ok(body_result)
        }
        Stmt::TaskGroup {
            children,
            join_semantics,
            ..
        } => {
            let mut child_results = Vec::new();
            let mut child_done_vars = Vec::new();
            let mut child_cancelled_vars = Vec::new();
            let mut child_resource_released_vars = Vec::new();
            for (i, child) in children.iter().enumerate() {
                let child_id = format!("__task_group_child_{}", i);
                let child_alive = Bool::new_const(ctx, format!("{}_alive", child_id).as_str());
                env.insert(format!("{}_alive", child_id), child_alive.clone().into());
                let result = stmt_to_z3(vc, child, env, solver_opt)?;
                child_results.push(result);
                let done_var = Bool::new_const(ctx, format!("{}_done", child_id).as_str());
                child_done_vars.push(done_var.clone());
                env.insert(format!("{}_done", child_id), done_var.into());
                let cancelled_var =
                    Bool::new_const(ctx, format!("{}_cancelled", child_id).as_str());
                child_cancelled_vars.push(cancelled_var.clone());
                env.insert(
                    format!("{}_cancelled", child_id),
                    cancelled_var.clone().into(),
                );
                if let Some(solver) = solver_opt {
                    solver.assert(&cancelled_var.implies(&child_alive.not()));
                    for resource in collect_acquire_resources_stmt(child) {
                        let released_var = Bool::new_const(
                            ctx,
                            format!("{}_resource_{}_released", child_id, resource).as_str(),
                        );
                        env.insert(
                            format!("{}_resource_{}_released", child_id, resource),
                            released_var.clone().into(),
                        );
                        solver.assert(&cancelled_var.implies(&released_var));
                        child_resource_released_vars.push(released_var);
                    }
                }
            }
            let parent_done = Bool::new_const(ctx, "__task_group_parent_done");
            if let Some(solver) = solver_opt {
                // Structured concurrency: the group only completes once every
                // child released the resources it acquired — whether the child
                // finished normally or was cancelled by a winning sibling.
                for released_var in &child_resource_released_vars {
                    solver.assert(&parent_done.implies(released_var));
                }
            }
            match join_semantics {
                JoinSemantics::All => {
                    if let Some(solver) = solver_opt {
                        for done_var in &child_done_vars {
                            solver.assert(&parent_done.implies(done_var));
                        }
                        // `all` never cancels a child: every child runs to
                        // completion, so its resources are released along the
                        // normal exit path rather than the cancellation path.
                        for cancelled_var in &child_cancelled_vars {
                            solver.assert(&parent_done.implies(&cancelled_var.not()));
                        }
                    }
                    if let Some(last) = child_results.last() {
                        Ok(last.clone())
                    } else {
                        Ok(Int::from_i64(ctx, 0).into())
                    }
                }
                JoinSemantics::Any => {
                    let any_result = Int::new_const(ctx, "__task_group_any_result");
                    if let Some(solver) = solver_opt {
                        if child_done_vars.is_empty() {
                            solver.assert(&parent_done.not());
                        } else {
                            let winner_cases = child_done_vars
                                .iter()
                                .enumerate()
                                .map(|(winner_idx, done_var)| {
                                    let mut clauses: Vec<Bool<'_>> = vec![done_var.clone()];
                                    if let Some(child_result) = child_results[winner_idx].as_int() {
                                        clauses.push(any_result._eq(&child_result));
                                    } else {
                                        clauses.push(any_result._eq(&Int::from_i64(ctx, 0)));
                                    }
                                    for (child_idx, cancelled_var) in
                                        child_cancelled_vars.iter().enumerate()
                                    {
                                        if child_idx != winner_idx {
                                            clauses.push(cancelled_var.clone());
                                        }
                                    }
                                    Bool::and(ctx, &clauses.iter().collect::<Vec<_>>())
                                })
                                .collect::<Vec<_>>();
                            let any_winner =
                                Bool::or(ctx, &winner_cases.iter().collect::<Vec<_>>());
                            solver.assert(&any_winner);
                        }
                    }
                    Ok(any_result.into())
                }
            }
        }
        Stmt::Expr(e, _) => expr_to_z3(vc, e, env, solver_opt),
        // Plan 8: Cancel statement — no-op in Z3 verification
        Stmt::Cancel { .. } => Ok(Int::from_i64(ctx, 0).into()),
    }
}

#[cfg(test)]
mod projection_slot_tests {
    use super::{
        canonical_struct_type_in_module, is_struct_projection_slot_name, projection_chains_collide,
        reject_parameter_projection_slot_collisions, struct_binding_slot_keys,
    };
    use crate::parser::{parse_module, Item};
    use crate::verification::ModuleEnv;
    use std::collections::HashSet;
    use std::time::{Duration, Instant};
    use z3::ast::Int;
    use z3::{Config, Context};

    fn module_env_with_structs(source: &str) -> ModuleEnv {
        let mut module_env = ModuleEnv::new();
        for item in parse_module(source) {
            if let Item::StructDef(sdef) = item {
                module_env.register_struct(&sdef);
            }
        }
        module_env
    }

    #[test]
    fn array_aliases_are_not_struct_parameter_types() {
        let mut module_env = ModuleEnv::new();
        let mut alias = None;
        let mut atom = None;
        for item in parse_module(
            "struct Item { x: i64 } \
             type Items = i64; \
             atom array_alias(items: Items, items_x: i64) -> i64 \
             requires: true; ensures: result == items_x; body: items_x",
        ) {
            match item {
                Item::StructDef(sdef) => module_env.register_struct(&sdef),
                Item::TypeDef(mut refined) => {
                    refined._base_type = "[Item]".to_string();
                    alias = Some(refined);
                }
                Item::Atom(parsed) => atom = Some(parsed),
                _ => {}
            }
        }
        module_env.register_type(alias.as_ref().expect("Items alias parsed"));

        assert_eq!(module_env.resolve_base_type("Items"), "[Item]");
        assert_eq!(canonical_struct_type_in_module(&module_env, "[Item]"), None);
        assert_eq!(canonical_struct_type_in_module(&module_env, "Item[]"), None);
        assert_eq!(canonical_struct_type_in_module(&module_env, "Items"), None);
        assert!(reject_parameter_projection_slot_collisions(
            &module_env,
            &atom.expect("array alias atom parsed")
        )
        .is_ok());
    }

    #[test]
    fn recursive_struct_projection_slot_keys_are_bounded() {
        let module_env = module_env_with_structs(
            "struct RecursiveWide { left: RecursiveWide, right: RecursiveWide, up: RecursiveWide, down: RecursiveWide }",
        );
        let sdef = module_env
            .get_struct("RecursiveWide")
            .expect("recursive struct is registered");
        let context = Context::new(&Config::new());
        let mut env: crate::verification::types::Env<'_> = std::collections::HashMap::new();
        let fields = ["left", "right", "up", "down"];
        for field in fields {
            for key in [
                format!("q_{field}"),
                format!("__struct_q_{field}"),
                format!("q_{field}_left"),
                format!("__struct_q_{field}_left"),
            ] {
                env.insert(key, Int::from_i64(&context, 0).into());
            }
        }

        let started = Instant::now();
        let keys = struct_binding_slot_keys(&module_env, "q", sdef, &env);
        let elapsed = started.elapsed();
        eprintln!("recursive slot-key discovery: {elapsed:?}");
        let expected = fields
            .iter()
            .flat_map(|field| [format!("q_{field}"), format!("__struct_q_{field}")])
            .collect::<HashSet<_>>();

        assert_eq!(keys, expected);
        assert!(
            elapsed < Duration::from_secs(2),
            "recursive slot-key discovery took {elapsed:?}"
        );
    }

    #[test]
    fn ambiguous_recursive_projection_path_search_is_bounded() {
        let module_env = module_env_with_structs(
            "struct RecursiveAmbiguous { a: RecursiveAmbiguous, aa: RecursiveAmbiguous }",
        );
        let sdef = module_env
            .get_struct("RecursiveAmbiguous")
            .expect("recursive struct is registered");
        let local = "q_a_a_a_a_a_a_a_a_a_a_a_a_a_a_a_a_a_a_a_a_z";

        let started = Instant::now();
        let collides = is_struct_projection_slot_name(&module_env, "q", sdef, local);
        let elapsed = started.elapsed();

        eprintln!("ambiguous projection-path search: {elapsed:?}");
        assert!(!collides);
        assert!(
            elapsed < Duration::from_secs(2),
            "projection-path search took {elapsed:?}"
        );
    }

    #[test]
    fn pairwise_projection_search_handles_prefix_ahead_on_either_side() {
        let module_env =
            module_env_with_structs("struct Bare { x_y: i64 } struct Prefixed { y: i64 }");
        let forward =
            projection_chains_collide(&module_env, ("Bare", ""), ("Prefixed", "x"), false)
                .expect("search stays within its node budget")
                .expect("prefixes and fields collide");
        assert_eq!(
            forward,
            ("x_y".to_string(), "y".to_string(), "x_y".to_string())
        );

        let reverse =
            projection_chains_collide(&module_env, ("Prefixed", "x"), ("Bare", ""), false)
                .expect("search stays within its node budget")
                .expect("prefixes and fields collide");
        assert_eq!(
            reverse,
            ("y".to_string(), "x_y".to_string(), "x_y".to_string())
        );
    }

    #[test]
    fn pairwise_projection_search_matches_one_token_to_a_chain() {
        let module_env = module_env_with_structs(
            "struct Whole { a_b: i64 } \
             struct Chained { a: Tail } \
             struct Tail { b: i64 }",
        );
        let collision =
            projection_chains_collide(&module_env, ("Whole", ""), ("Chained", ""), true)
                .expect("search stays within its node budget")
                .expect("one token collides with a two-token chain");
        assert_eq!(
            collision,
            ("a_b".to_string(), "a.b".to_string(), "a_b".to_string())
        );
    }

    #[test]
    fn pairwise_projection_search_preserves_underscores_inside_tokens() {
        let module_env = module_env_with_structs(
            "struct Left { a_b: LeftTail } \
             struct LeftTail { c: i64 } \
             struct Right { a: RightTail } \
             struct RightTail { b_c: i64 }",
        );
        let collision = projection_chains_collide(&module_env, ("Left", ""), ("Right", ""), true)
            .expect("search stays within its node budget")
            .expect("underscore-bearing tokens collide");
        assert_eq!(
            collision,
            (
                "a_b.c".to_string(),
                "a.b_c".to_string(),
                "a_b_c".to_string()
            )
        );
    }

    #[test]
    fn pairwise_projection_search_checks_empty_prefix_starts() {
        let module_env = module_env_with_structs(
            "struct EmptyPrefix { a: EmptyTail, a_b: i64 } \
             struct EmptyTail { b: i64 }",
        );
        let collision =
            projection_chains_collide(&module_env, ("EmptyPrefix", ""), ("EmptyPrefix", ""), true)
                .expect("search stays within its node budget")
                .expect("empty-prefix field paths collide");
        assert_eq!(
            collision,
            ("a.b".to_string(), "a_b".to_string(), "a_b".to_string())
        );
    }

    #[test]
    fn pairwise_projection_search_is_fast_for_wide_recursive_structs() {
        let module_env = module_env_with_structs(
            "struct RecursiveWide { \
                left: RecursiveWide, \
                right: RecursiveWide, \
                up: RecursiveWide, \
                down: RecursiveWide \
            }",
        );
        let started = Instant::now();
        let collision = projection_chains_collide(
            &module_env,
            ("RecursiveWide", ""),
            ("RecursiveWide", ""),
            true,
        )
        .expect("search stays within its node budget");
        let elapsed = started.elapsed();

        assert_eq!(collision, None);
        assert!(
            elapsed < Duration::from_secs(2),
            "wide recursive projection search took {elapsed:?}"
        );
    }

    #[test]
    fn recursive_parameter_overlap_requires_pairwise_slot_search() {
        let module_env = module_env_with_structs(
            "struct RecursiveParam { x: i64, a_x: i64, r: RecursiveParam }",
        );
        let sdef = module_env
            .get_struct("RecursiveParam")
            .expect("recursive struct is registered");
        assert!(!is_struct_projection_slot_name(
            &module_env,
            "p",
            sdef,
            "p_r_a"
        ));

        let collision = projection_chains_collide(
            &module_env,
            ("RecursiveParam", "p"),
            ("RecursiveParam", "p_r_a"),
            false,
        )
        .expect("search stays within its node budget")
        .expect("recursive projection slot overlaps the other parameter");
        assert_eq!(
            collision,
            ("r.a_x".to_string(), "x".to_string(), "p_r_a_x".to_string())
        );
    }
}
