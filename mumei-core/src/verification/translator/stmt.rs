#![allow(unused_imports)]
use super::super::support::*;
use super::super::*;
use super::*;
use crate::lowering::{lower, LoweredType};
use serde_json::json;

/// Collect the env names a statement can overwrite: `Assign` targets plus the
/// `__z3_arr_<name>` keys `ArrayStore` writes. Used to havoc loop-carried
/// variables before induction checks.
fn collect_assigned_vars(stmt: &Stmt, out: &mut std::collections::HashSet<String>) {
    match stmt {
        Stmt::Assign { var, value, .. } => {
            out.insert(var.clone());
            collect_expr_assigned_vars(value, out);
        }
        Stmt::ArrayStore {
            array,
            index,
            value,
            ..
        } => {
            out.insert(format!("__z3_arr_{}", array));
            collect_expr_assigned_vars(index, out);
            collect_expr_assigned_vars(value, out);
        }
        Stmt::Block(stmts, _) => {
            for s in stmts {
                collect_assigned_vars(s, out);
            }
        }
        Stmt::While {
            cond,
            invariant,
            decreases,
            body,
            ..
        } => {
            collect_assigned_vars(body, out);
            collect_expr_assigned_vars(cond, out);
            collect_expr_assigned_vars(invariant, out);
            if let Some(d) = decreases {
                collect_expr_assigned_vars(d, out);
            }
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
            collect_assigned_vars(body, out);
        }
        Stmt::TaskGroup { children, .. } => {
            for c in children {
                collect_assigned_vars(c, out);
            }
        }
        Stmt::Expr(expr, _) => collect_expr_assigned_vars(expr, out),
        Stmt::Let { value, .. } => collect_expr_assigned_vars(value, out),
        _ => {}
    }
}

/// Recursive companion of `collect_assigned_vars` over expressions that embed
/// statement bodies (`if`, `match`, `async`, lambdas).
fn collect_expr_assigned_vars(expr: &Expr, out: &mut std::collections::HashSet<String>) {
    match expr {
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_expr_assigned_vars(cond, out);
            collect_assigned_vars(then_branch, out);
            collect_assigned_vars(else_branch, out);
        }
        Expr::Match { target, arms } => {
            collect_expr_assigned_vars(target, out);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_expr_assigned_vars(guard, out);
                }
                collect_assigned_vars(&arm.body, out);
            }
        }
        Expr::Async { body } | Expr::Lambda { body, .. } => collect_assigned_vars(body, out),
        Expr::Await { expr } | Expr::FieldAccess(expr, _) | Expr::ChanRecv { channel: expr } => {
            collect_expr_assigned_vars(expr, out);
        }
        Expr::ArrayAccess(_, index) => collect_expr_assigned_vars(index, out),
        Expr::BinaryOp(l, _, r) => {
            collect_expr_assigned_vars(l, out);
            collect_expr_assigned_vars(r, out);
        }
        Expr::Call(_, args) | Expr::Perform { args, .. } => {
            for a in args {
                collect_expr_assigned_vars(a, out);
            }
        }
        Expr::CallRef { callee, args } => {
            collect_expr_assigned_vars(callee, out);
            for a in args {
                collect_expr_assigned_vars(a, out);
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, v) in fields {
                collect_expr_assigned_vars(v, out);
            }
        }
        Expr::ChanSend { channel, value } => {
            collect_expr_assigned_vars(channel, out);
            collect_expr_assigned_vars(value, out);
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
                    let range = old.get_sort().array_range().map(|s| s.kind());
                    let elem_sort = match range {
                        Some(z3::SortKind::Real) => ArrayElementSort::Real,
                        Some(z3::SortKind::FloatingPoint) => ArrayElementSort::Float,
                        Some(z3::SortKind::Bool) => ArrayElementSort::Bool,
                        Some(z3::SortKind::Seq) => ArrayElementSort::Str,
                        Some(z3::SortKind::Array) => ArrayElementSort::Nested,
                        _ => ArrayElementSort::Int,
                    };
                    z3_array_for_sort(ctx, &fresh_name, elem_sort).into()
                }
                _ => old.clone(),
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

/// Record (or clear) the lambda bound by a `let`/`assign` so a later
/// `var(args)` / `call(var, args)` can inline the body. `let g = f`
/// where `f` is lambda-bound aliases the binding; rebinding to any other
/// value drops the entry so a stale lambda never answers a call issued
/// to a re-bound name.
fn record_binding_lambda(vc: &VCtx, var: &str, value: &Expr) {
    let bound = match value {
        Expr::Lambda { .. } => Some(std::rc::Rc::new(LocalLambda::Closure {
            expr: value.clone(),
            captured: vc.local_lambdas.borrow().clone(),
        })),
        Expr::Variable(src) => vc.local_lambdas.borrow().get(src).cloned(),
        _ => None,
    };
    match bound {
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
            record_binding_lambda(vc, var, value);
            env.insert(var.clone(), val.clone());
            alias_struct_fields(env, var, &val);
            wire_array_slots(vc, var, Some(value), &val, env);
            profile_solver_assertion(vc, &format!("let_{}", var), None);
            Ok(val)
        }
        Stmt::Assign { var, value, .. } => {
            let val = expr_to_z3(vc, value, env, solver_opt)?;
            record_binding_enum_type(vc, var, value);
            record_binding_lambda(vc, var, value);
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
            ..
        } => {
            // Loop Invariant 検証ロジック
            if let Some(solver) = solver_opt {
                // Vars the body assigns that were bound before the loop —
                // they are havoced so induction is checked from *any* state
                // satisfying the invariant, not only the concrete entry
                // state, which would mask violations on later iterations.
                let mut modified = std::collections::HashSet::new();
                collect_assigned_vars(body, &mut modified);
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

                let marks = obligation_marks(vc);
                let inv = expr_to_z3(vc, invariant, env, None)?
                    .as_bool()
                    .ok_or(MumeiError::type_error("Invariant must be boolean"))?;
                rebind_deferred_obligations(vc, marks, &inv);

                // Base case — conjoin path conditions from any enclosing
                // `if/else` branches so that loop bodies inside e.g. the
                // `else` of `if n <= 1 { … } else { let i = 1; while … }`
                // can rely on the corresponding guard (here `n > 1`).
                let path_cond = vc.path_cond_conj();
                solver.push();
                solver.assert(&Bool::and(ctx, &[&path_cond, &inv.not()]));
                if solver.check() == SatResult::Sat {
                    solver.pop(1);
                    return Err(MumeiError::verification("Invariant fails initially"));
                }
                solver.pop(1);

                // Inductive step — on a havoced env: the invariant must be
                // preserved from ANY state satisfying it, not just the
                // concrete loop-entry bindings.
                {
                    let env_snapshot = env.clone();
                    let types_snapshot = vc.local_enum_types.borrow().clone();
                    let lambdas_snapshot = vc.local_lambdas.borrow().clone();
                    let mut step_env = env.clone();
                    havoc_vars(vc, &mut step_env, &modified);
                    let marks = obligation_marks(vc);
                    let inv_h = expr_to_z3(vc, invariant, &mut step_env, None)?
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

                    let inv_after = expr_to_z3(vc, invariant, &mut step_env, None)?
                        .as_bool()
                        .ok_or(MumeiError::type_error("Invariant must be boolean"))?;

                    solver.assert(&inv_after.not());
                    if solver.check() == SatResult::Sat {
                        solver.pop(1);
                        return Err(MumeiError::verification("Invariant not preserved"));
                    }
                    solver.pop(1);
                    *env = env_snapshot;
                    *vc.local_enum_types.borrow_mut() = types_snapshot.clone();
                    *vc.local_lambdas.borrow_mut() = lambdas_snapshot.clone();
                }

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
            if let Some(solver) = solver_opt {
                solver.assert(&held_bool);
            }
            env.insert(held_name.clone(), held_bool.into());
            let body_result = stmt_to_z3(vc, body, env, solver_opt)?;
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
