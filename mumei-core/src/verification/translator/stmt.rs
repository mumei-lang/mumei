#![allow(unused_imports)]
use super::super::support::*;
use super::super::*;
use super::*;
use crate::lowering::{lower, LoweredType};
use crate::verification::invariant_inference::{
    expr_to_string, generate_candidates, InferredInvariant,
};
use crate::verification::translator::may_write::{callee_may_write_args, CalleeRef};
use crate::verification::translator::z3_types::array_root_ast;
use serde_json::json;

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
            out.insert(var.clone());
            collect_expr_assigned_vars(env, vc, value, out, true);
        }
        Stmt::ArrayStore {
            array,
            index,
            value,
            ..
        } => {
            out.insert(format!("__z3_arr_{}", array));
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
fn havoc_vars<'a>(vc: &VCtx<'a>, env: &mut Env<'a>, vars: &std::collections::HashSet<String>) {
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
                    // Fresh `Int -> Elem` array const — `array_domain/range`
                    // Sorts borrow the temporary, so lift only the (Copy)
                    // range kind and rebuild through `z3_array_for_sort`.
                    let root = old.as_array().map(|a| array_root_ast(&a));
                    let range = old.get_sort().array_range().map(|s| s.kind());
                    let elem_sort = match range {
                        Some(z3::SortKind::Real) => ArrayElementSort::Real,
                        Some(z3::SortKind::FloatingPoint) => ArrayElementSort::Float,
                        Some(z3::SortKind::Bool) => ArrayElementSort::Bool,
                        Some(z3::SortKind::Seq) => ArrayElementSort::Str,
                        Some(z3::SortKind::Array) => ArrayElementSort::Nested,
                        _ => ArrayElementSort::Int,
                    };
                    let fresh_arr: Dynamic = z3_array_for_sort(ctx, &fresh_name, elem_sort).into();
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
                        for key in shared {
                            env.insert(key, fresh_arr.clone());
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
                env.insert(arr_key, fresh);
            }
            let len_key = format!("len_{name}");
            if env.contains_key(&len_key) {
                env.insert(
                    len_key,
                    Int::fresh_const(ctx, &format!("__havoc_len_{}_{}", name, uid)).into(),
                );
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
        *vc.local_array_elem_types.borrow_mut() = array_elem_types_snapshot.clone();
        *vc.local_lambdas.borrow_mut() = lambdas_snapshot.clone();
        *vc.call_result_lens.borrow_mut() = call_result_lens_snapshot.clone();
    };
    let result = (|| -> MumeiResult<bool> {
        let inv = expr_to_z3(vc, invariant, env, None)?
            .as_bool()
            .ok_or(MumeiError::type_error("Invariant must be boolean"))?;
        solver.push();
        solver.assert(&Bool::and(ctx, &[&path_cond, &inv.not()]));
        let base = solver.check();
        solver.pop(1);
        if base != SatResult::Unsat {
            return Ok(false);
        }

        let mut step_env = env.clone();
        let modified_set: std::collections::HashSet<String> = modified.iter().cloned().collect();
        havoc_vars(vc, &mut step_env, &modified_set);
        let step = (|| -> MumeiResult<SatResult> {
            let inv_h = expr_to_z3(vc, invariant, &mut step_env, None)?
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
                let inv_after = expr_to_z3(vc, invariant, &mut step_env, None)?
                    .as_bool()
                    .ok_or(MumeiError::type_error("Invariant must be boolean"))?;
                solver.assert(&inv_after.not());
                Ok(solver.check())
            })();
            solver.pop(1);
            result
        })()?;
        Ok(step == SatResult::Unsat)
    })();
    restore();
    *env = env_snapshot;
    result
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
            let val = expr_to_z3(vc, value, env, solver_opt)?;
            record_binding_enum_type(vc, var, value);
            record_binding_lambda(vc, var, value, env, solver_opt);
            env.insert(var.clone(), val.clone());
            alias_struct_fields(env, var, &val);
            wire_array_slots(vc, var, Some(value), &val, env);
            profile_solver_assertion(vc, &format!("let_{}", var), None);
            Ok(val)
        }
        Stmt::Assign { var, value, .. } => {
            if var.contains('.') {
                let Some((resource, _field)) = vc.module_env.is_resource_state_var(var) else {
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
            let val = expr_to_z3(vc, value, env, solver_opt)?;
            record_binding_enum_type(vc, var, value);
            record_binding_lambda(vc, var, value, env, solver_opt);
            env.insert(var.clone(), val.clone());
            alias_struct_fields(env, var, &val);
            wire_array_slots(vc, var, Some(value), &val, env);
            profile_solver_assertion(vc, &format!("assign_{}", var), None);
            Ok(val)
        }
        Stmt::ArrayStore {
            array,
            index,
            value,
            ..
        } => {
            // Indices stay `Int`-sorted under `--bitvec-i64`; bridge a `BV(64)`
            // index through the signed bit-vector/integer conversion.
            let idx_value = expr_to_z3(vc, index, env, solver_opt)?;
            let idx = as_int_like(&idx_value)
                .ok_or(MumeiError::type_error("Array index must be integer"))?;
            let val = expr_to_z3(vc, value, env, solver_opt)?;
            let stored_val = coerce_array_store_value(vc, array, val)?;

            // OOB check mirrors `Expr::ArrayAccess`: store at an index that may
            // fall outside `[0, len_<name>)` is flagged as a verification
            // error with a counter-example hint.
            if let Some(solver) = solver_opt {
                let len = array_len_value(ctx, env, array, vc.bitvec_i64, Some(solver));
                let safe = index_in_bounds(ctx, &idx_value, &len)
                    .ok_or(MumeiError::type_error("Array index must be integer"))?;
                solver.push();
                solver.assert(&safe.not());
                if solver.check() == SatResult::Sat {
                    solver.pop(1);
                    return Err(MumeiError::verification(format!(
                        "Potential Out-of-Bounds store on '{}' (index may be < 0 or >= len_{})",
                        array, array
                    ))
                    .with_help(
                        "requires にストアインデックスの範囲制約 (0 <= idx < len) を追加してください",
                    ));
                }
                solver.pop(1);
            }

            let arr_key = format!("__z3_arr_{}", array);
            let current_arr = z3_dynamic_array(vc, array, env);
            let new_arr = current_arr.store(&idx, &stored_val);
            env.insert(arr_key, new_arr.into());
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
                let inv = expr_to_z3(vc, &effective_invariant, env, None)?
                    .as_bool()
                    .ok_or(MumeiError::type_error("Invariant must be boolean"))?;
                rebind_deferred_obligations(vc, marks, &inv);
                let path_cond = vc.path_cond_conj();
                solver.push();
                solver.assert(&Bool::and(ctx, &[&path_cond, &inv.not()]));
                if solver.check() == SatResult::Sat {
                    solver.pop(1);
                    return Err(MumeiError::verification("Invariant fails initially"));
                }
                solver.pop(1);

                // Inductive step — on a havoced env: the invariant must be
                // preserved from ANY state satisfying it, not just the concrete
                // loop-entry bindings.
                {
                    let env_snapshot = env.clone();
                    let types_snapshot = vc.local_enum_types.borrow().clone();
                    let lambdas_snapshot = vc.local_lambdas.borrow().clone();
                    let mut step_env = env.clone();
                    havoc_vars(vc, &mut step_env, &modified);
                    let marks = obligation_marks(vc);
                    let inv_h = expr_to_z3(vc, &effective_invariant, &mut step_env, None)?
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
                    let inv_after = expr_to_z3(vc, &effective_invariant, &mut step_env, None)?
                        .as_bool()
                        .ok_or(MumeiError::type_error("Invariant must be boolean"))?;
                    solver.assert(&inv_after.not());
                    if solver.check() == SatResult::Sat {
                        solver.pop(1);
                        return Err(MumeiError::verification("Invariant not preserved"));
                    }
                    solver.pop(1);
                    *env = env_snapshot;
                    *vc.local_enum_types.borrow_mut() = types_snapshot;
                    *vc.local_lambdas.borrow_mut() = lambdas_snapshot;
                }
                let invariant = &effective_invariant;

                // Termination Check — again under havoced pre-state.
                if let Some(dec_expr) = decreases {
                    let env_snapshot = env.clone();
                    let types_snapshot = vc.local_enum_types.borrow().clone();
                    let lambdas_snapshot = vc.local_lambdas.borrow().clone();
                    let mut term_env = env.clone();
                    havoc_vars(vc, &mut term_env, &modified);
                    let marks = obligation_marks(vc);
                    let inv_h = expr_to_z3(vc, invariant, &mut term_env, None)?
                        .as_bool()
                        .ok_or(MumeiError::type_error("Invariant must be boolean"))?;
                    rebind_deferred_obligations(vc, marks, &inv_h);
                    let marks = obligation_marks(vc);
                    let c_h = expr_to_z3(vc, cond, &mut term_env, None)?
                        .as_bool()
                        .ok_or(MumeiError::type_error("While condition must be boolean"))?;
                    rebind_deferred_obligations(vc, marks, &inv_h);
                    let marks = obligation_marks(vc);
                    let v_before = as_int_like(&expr_to_z3(vc, dec_expr, &mut term_env, None)?)
                        .ok_or(MumeiError::type_error(
                            "decreases expression must be integer",
                        ))?;
                    rebind_deferred_obligations(vc, marks, &inv_h);
                    solver.push();
                    solver.assert(&inv_h);
                    solver.assert(&c_h);
                    solver.assert(&v_before.lt(&Int::from_i64(ctx, 0)));
                    if solver.check() == SatResult::Sat {
                        solver.pop(1);
                        *env = env_snapshot;
                        *vc.local_enum_types.borrow_mut() = types_snapshot.clone();
                        *vc.local_lambdas.borrow_mut() = lambdas_snapshot.clone();
                        return Err(MumeiError::verification(
                            "Termination check failed: decreases expression may be negative",
                        ));
                    }
                    solver.pop(1);
                    solver.push();
                    solver.assert(&inv_h);
                    solver.assert(&c_h);
                    stmt_to_z3(vc, body, &mut term_env, Some(solver))?;
                    let v_after = as_int_like(&expr_to_z3(vc, dec_expr, &mut term_env, None)?)
                        .ok_or(MumeiError::type_error(
                            "decreases expression must be integer",
                        ))?;
                    solver.assert(&v_after.ge(&v_before));
                    if solver.check() == SatResult::Sat {
                        solver.pop(1);
                        *env = env_snapshot;
                        *vc.local_enum_types.borrow_mut() = types_snapshot.clone();
                        *vc.local_lambdas.borrow_mut() = lambdas_snapshot.clone();
                        return Err(MumeiError::verification(
                            "Termination check failed: decreases expression does not strictly decrease"
                        ));
                    }
                    solver.pop(1);
                    *env = env_snapshot;
                    *vc.local_enum_types.borrow_mut() = types_snapshot.clone();
                    *vc.local_lambdas.borrow_mut() = lambdas_snapshot.clone();
                }

                // Post-loop state: havoc the loop-carried vars once more and
                // assert `invariant ∧ ¬cond` so downstream statements and the
                // `ensures` check see exit facts (previously env kept
                // pre-loop bindings and no exit facts reached the solver).
                let mut post_env = env.clone();
                havoc_vars(vc, &mut post_env, &modified);
                let marks = obligation_marks(vc);
                let inv_post = expr_to_z3(vc, invariant, &mut post_env, None)?
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
                *env = post_env;
                return Ok(Bool::and(ctx, &[&inv_post, &c_not_post]).into());
            }

            let inv = expr_to_z3(vc, invariant, env, None)?
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
                        let value = param_z3_value(
                            ctx,
                            &format!("{key}#acq{acquire_id}"),
                            Some(&field.ty),
                            vc.module_env,
                            vc.ieee754_f64,
                            vc.bitvec_i64,
                        );
                        env.insert(key, value);
                    }
                    if let (Some(invariant), Some(solver)) =
                        (resource_def.invariant.as_ref(), solver_opt)
                    {
                        let inv = expr_to_z3(vc, invariant, env, Some(solver))?
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
                        let inv_after = expr_to_z3(vc, invariant, env, Some(solver))?
                            .as_bool()
                            .ok_or(MumeiError::type_error("resource invariant must be boolean"))?;
                        solver.push();
                        solver.assert(&inv_after.not());
                        let result = solver.check();
                        solver.pop(1);
                        match result {
                            z3::SatResult::Sat => {
                                return Err(MumeiError::verification(format!(
                                    "shared invariant of resource '{}' not re-established at unlock: {}",
                                    resource,
                                    crate::verification::support::expr_to_source_string(invariant)
                                )));
                            }
                            z3::SatResult::Unknown => {
                                return Err(MumeiError::verification(format!(
                                    "shared invariant of resource '{}' not re-established at unlock: {} (solver returned unknown)",
                                    resource,
                                    crate::verification::support::expr_to_source_string(invariant)
                                )));
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
