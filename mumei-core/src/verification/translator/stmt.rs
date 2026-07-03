#![allow(unused_imports)]
use super::super::support::*;
use super::super::*;
use super::*;
use crate::lowering::{lower, LoweredType};
use serde_json::json;

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
            env.insert(var.clone(), val.clone());
            profile_solver_assertion(vc, &format!("let_{}", var), None);
            Ok(val)
        }
        Stmt::Assign { var, value, .. } => {
            let val = expr_to_z3(vc, value, env, solver_opt)?;
            env.insert(var.clone(), val.clone());
            profile_solver_assertion(vc, &format!("assign_{}", var), None);
            Ok(val)
        }
        Stmt::ArrayStore {
            array,
            index,
            value,
            ..
        } => {
            let idx = expr_to_z3(vc, index, env, solver_opt)?
                .as_int()
                .ok_or(MumeiError::type_error("Array index must be integer"))?;
            let val = expr_to_z3(vc, value, env, solver_opt)?;
            let stored_val = coerce_array_store_value(vc, array, val)?;

            // OOB check mirrors `Expr::ArrayAccess`: store at an index that may
            // fall outside `[0, len_<name>)` is flagged as a verification
            // error with a counter-example hint.
            if let Some(solver) = solver_opt {
                let len_name = format!("len_{}", array);
                let len = if let Some(existing) = env.get(&len_name) {
                    existing
                        .as_int()
                        .unwrap_or_else(|| Int::new_const(ctx, len_name.as_str()))
                } else {
                    let l = Int::new_const(ctx, len_name.as_str());
                    solver.assert(&l.ge(&Int::from_i64(ctx, 0)));
                    env.insert(len_name.clone(), l.clone().into());
                    l
                };
                let safe = Bool::and(ctx, &[&idx.ge(&Int::from_i64(ctx, 0)), &idx.lt(&len)]);
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
                let inv = expr_to_z3(vc, invariant, env, None)?
                    .as_bool()
                    .ok_or(MumeiError::type_error("Invariant must be boolean"))?;

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

                // Inductive step
                let c = expr_to_z3(vc, cond, env, None)?
                    .as_bool()
                    .ok_or(MumeiError::type_error("While condition must be boolean"))?;

                {
                    let env_snapshot = env.clone();
                    solver.push();
                    solver.assert(&inv);
                    solver.assert(&c);
                    stmt_to_z3(vc, body, env, Some(solver))?;

                    let inv_after = expr_to_z3(vc, invariant, env, None)?
                        .as_bool()
                        .ok_or(MumeiError::type_error("Invariant must be boolean"))?;

                    solver.assert(&inv_after.not());
                    if solver.check() == SatResult::Sat {
                        solver.pop(1);
                        return Err(MumeiError::verification("Invariant not preserved"));
                    }
                    solver.pop(1);
                    *env = env_snapshot;
                }

                // Termination Check
                if let Some(dec_expr) = decreases {
                    let env_snapshot = env.clone();
                    let v_before = expr_to_z3(vc, dec_expr, env, None)?.as_int().ok_or(
                        MumeiError::type_error("decreases expression must be integer"),
                    )?;
                    solver.push();
                    solver.assert(&inv);
                    solver.assert(&c);
                    solver.assert(&v_before.lt(&Int::from_i64(ctx, 0)));
                    if solver.check() == SatResult::Sat {
                        solver.pop(1);
                        return Err(MumeiError::verification(
                            "Termination check failed: decreases expression may be negative",
                        ));
                    }
                    solver.pop(1);
                    solver.push();
                    solver.assert(&inv);
                    solver.assert(&c);
                    stmt_to_z3(vc, body, env, Some(solver))?;
                    let v_after = expr_to_z3(vc, dec_expr, env, None)?.as_int().ok_or(
                        MumeiError::type_error("decreases expression must be integer"),
                    )?;
                    solver.assert(&v_after.ge(&v_before));
                    if solver.check() == SatResult::Sat {
                        solver.pop(1);
                        *env = env_snapshot;
                        return Err(MumeiError::verification(
                            "Termination check failed: decreases expression does not strictly decrease"
                        ));
                    }
                    solver.pop(1);
                    *env = env_snapshot;
                }
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
                    }
                }
            }
            let parent_done = Bool::new_const(ctx, "__task_group_parent_done");
            match join_semantics {
                JoinSemantics::All => {
                    if let Some(solver) = solver_opt {
                        for done_var in &child_done_vars {
                            solver.assert(&parent_done.implies(done_var));
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
