#![allow(unused_imports)]
use super::super::support::*;
use super::super::*;
use super::*;
use crate::lowering::{lower, LoweredType};
use serde_json::json;

pub(crate) fn apply_refinement_constraint<'a>(
    vc: &VCtx<'a>,
    solver: &Solver<'a>,
    var_name: &str,
    refined: &RefinedType,
    global_env: &mut Env<'a>,
) -> MumeiResult<()> {
    let ctx = vc.ctx;
    // Type System 2.0: ベース型に基づいて変数を生成
    let var_z3: Dynamic = match refined._base_type.as_str() {
        // f64 エンコーディング（デフォルト Real、`--ieee754-f64` で
        // Float）に合わせ、パラメータ側の `param_z3_value` と同一
        // ソートにする。
        "f64" if vc.ieee754_f64 => Float::new_const(ctx, var_name, 11, 53).into(),
        "f64" => Real::new_const(ctx, var_name).into(),
        "u64" => {
            let v = Int::new_const(ctx, var_name);
            let track_u64 = Bool::new_const(ctx, format!("track_u64_nonneg_{}", var_name).as_str());
            solver.assert_and_track(&v.ge(&Int::from_i64(ctx, 0)), &track_u64);
            profile_solver_assertion(vc, &format!("u64_nonneg_{}", var_name), None);
            v.into()
        }
        // Plan 9-7: Str base type uses Z3 String Sort for refinement constraints
        "Str" => Z3String::new_const(ctx, var_name).into(),
        // A refined `i64` keeps the encoding of the run, so a refinement alias
        // does not silently return an overflow-sensitive parameter to the
        // unbounded `Int` encoding.
        "i64" if vc.bitvec_i64 => BV::new_const(ctx, var_name, I64_BITS).into(),
        _ => Int::new_const(ctx, var_name).into(),
    };

    global_env.insert(var_name.to_string(), var_z3.clone());

    let mut local_env = global_env.clone();
    local_env.insert(refined.operand.clone(), var_z3);

    let predicate_ast = parse_expression(&refined.predicate_raw);
    let predicate_z3 = expr_to_z3(vc, &predicate_ast, &mut local_env, None)?
        .as_bool()
        .ok_or(
            MumeiError::type_error_at(
                format!("Predicate for {} must be boolean", refined.name),
                refined.span.clone(),
            )
            .with_help(format!(
                "型 '{}' の制約が boolean 式である必要があります",
                refined.name
            )),
        )?;

    let track_label = format!("track_refined_type_{}::{}", var_name, refined.name);
    let track_bool = Bool::new_const(ctx, track_label.as_str());
    solver.assert_and_track(&predicate_z3, &track_bool);
    profile_solver_assertion(
        vc,
        &format!("refined_type_{}::{}", var_name, refined.name),
        Some(refined.span.to_string()),
    );
    Ok(())
}

// =============================================================================
// Subsumption Check: atom_ref contract implication
// =============================================================================
//
// When `atom_ref(concrete)` is passed to a parameter with `contract(f)`,
// verify that the concrete atom's ensures clause *implies* the contract's
// ensures clause under the concrete atom's precondition.  This is a
// universal validity check:
//
//   ∀ params. (concrete.requires ∧ concrete.ensures) ⇒ contract.ensures
//
// If the implication does not hold, verification fails closed.

/// Check that `concrete_atom.requires ∧ concrete_atom.ensures` implies
/// `contract_ensures`.
///
/// Uses a Z3 solver scope (push/pop) to avoid polluting the caller's context.
#[allow(clippy::too_many_arguments)]
pub(crate) fn check_contract_subsumption<'a>(
    vc: &VCtx<'a>,
    concrete_atom: &Atom,
    contract_ensures: &str,
    _contract_requires: Option<&str>, // reserved for future use
    callee_name: &str,
    param_name: &str,
    solver: &Solver<'a>,
    ctx: &'a Context,
) -> Result<(), MumeiError> {
    // Skip when the contract requires nothing — any ensures trivially implies "true".
    // NOTE: We intentionally do NOT skip when concrete_atom.ensures == "true".
    // An atom with ensures: true guarantees nothing, so it cannot imply a
    // non-trivial contract like `result >= 0`. The Z3 check below will correctly
    // find a counterexample in that case.
    if contract_ensures.trim() == "true" {
        return Ok(());
    }

    // Build separate environments for the concrete atom and the contract.
    // Both environments share the same position-indexed Z3 values, but the
    // contract uses CallRef's positional aliases rather than concrete names.
    let mut concrete_env: Env<'_> = HashMap::new();
    let mut contract_env: Env<'_> = HashMap::new();
    let mut param_values = Vec::with_capacity(concrete_atom.params.len());
    let mut array_len_constraints = Vec::new();
    for (i, param) in concrete_atom.params.iter().enumerate() {
        let z3_var = param_z3_value(
            ctx,
            &format!("__sub_{}", param.name),
            param.type_name.as_deref(),
            vc.module_env,
            vc.ieee754_f64,
            vc.bitvec_i64,
        );
        if param
            .type_name
            .as_deref()
            .is_some_and(|type_name| type_name.trim_start().starts_with('['))
        {
            let len = array_len_symbol(ctx, &format!("__sub_len_{}", param.name), vc.bitvec_i64);
            if let Some(nonneg) = nonneg_constraint(ctx, &len) {
                array_len_constraints.push(nonneg);
            }
            concrete_env.insert(format!("len_{}", param.name), len.clone());
            contract_env.insert(format!("len_arg{i}"), len.clone());
            if i == 0 {
                contract_env.insert("len_x".to_string(), len.clone());
            } else if i == 1 {
                contract_env.insert("len_y".to_string(), len.clone());
            }
        }
        param_values.push(z3_var.clone());
        concrete_env.insert(param.name.clone(), z3_var);
    }

    for (i, value) in param_values.iter().enumerate() {
        contract_env.insert(format!("arg{i}"), value.clone());
    }
    if let Some(first) = param_values.first() {
        contract_env.insert("x".to_string(), first.clone());
    }
    if let Some(second) = param_values.get(1) {
        contract_env.insert("y".to_string(), second.clone());
    }

    // Create a fresh symbolic result that both ensures clauses reference.
    let result_var = param_z3_value(
        ctx,
        "__sub_result",
        concrete_atom.return_type.as_deref(),
        vc.module_env,
        vc.ieee754_f64,
        vc.bitvec_i64,
    );
    concrete_env.insert("result".to_string(), result_var.clone());
    contract_env.insert("result".to_string(), result_var);

    // The parser represents `true` / `false` as Expr::Variable("true"|"false").
    // Pre-bind them to Z3 Bool constants so expr_to_z3 produces Bool sort
    // instead of an unbound Int, which would fail the as_bool() gate below.
    concrete_env.insert(
        "true".to_string(),
        z3::ast::Bool::from_bool(ctx, true).into(),
    );
    concrete_env.insert(
        "false".to_string(),
        z3::ast::Bool::from_bool(ctx, false).into(),
    );
    contract_env.insert(
        "true".to_string(),
        z3::ast::Bool::from_bool(ctx, true).into(),
    );
    contract_env.insert(
        "false".to_string(),
        z3::ast::Bool::from_bool(ctx, false).into(),
    );

    // --- Assert concrete atom's requires clause (precondition) ---
    // Without this, the check would ask "for ALL params, does ensures ⇒
    // contract?" which is too strong. We need "for params satisfying
    // requires, does ensures ⇒ contract?".
    let concrete_req = concrete_atom.requires.trim();
    let requires_bool_opt = if concrete_req != "true" && !concrete_req.is_empty() {
        let req_ast = parse_expression(concrete_req);
        let value = expr_to_z3(vc, &req_ast, &mut concrete_env, None).map_err(|error| {
            MumeiError::verification(format!(
                "Contract subsumption could not lower {}.requires: {}",
                concrete_atom.name, error
            ))
        })?;
        Some(value.as_bool().ok_or_else(|| {
            MumeiError::verification(format!(
                "Contract subsumption requires '{}' to be boolean",
                concrete_atom.requires
            ))
        })?)
    } else {
        None
    };

    // Parse and evaluate the concrete atom's ensures.
    let concrete_ens_ast = parse_expression(&concrete_atom.ensures);
    let concrete_ens_z3 =
        expr_to_z3(vc, &concrete_ens_ast, &mut concrete_env, None).map_err(|error| {
            MumeiError::verification(format!(
                "Contract subsumption could not lower {}.ensures: {}",
                concrete_atom.name, error
            ))
        })?;

    // Parse and evaluate the contract's ensures.
    let contract_ens_ast = parse_expression(contract_ensures);
    let contract_ens_z3 =
        expr_to_z3(vc, &contract_ens_ast, &mut contract_env, None).map_err(|error| {
            MumeiError::verification(format!(
                "Contract subsumption could not lower contract ensures '{}': {}",
                contract_ensures, error
            ))
        })?;

    // Both must be booleans for an implication check.
    let (concrete_bool, contract_bool) =
        match (concrete_ens_z3.as_bool(), contract_ens_z3.as_bool()) {
            (Some(c), Some(ct)) => (c, ct),
            _ => {
                return Err(MumeiError::verification(format!(
                    "Contract subsumption ensures must be boolean: concrete '{}' and contract '{}'",
                    concrete_atom.ensures, contract_ensures
                )))
            }
        };

    // Check: requires ∧ concrete_ensures ∧ ¬contract_ensures is UNSAT
    //        ⟺  (requires ∧ concrete_ensures) ⇒ contract_ensures
    solver.push();
    for constraint in &array_len_constraints {
        solver.assert(constraint);
    }
    if let Some(ref req_bool) = requires_bool_opt {
        solver.assert(req_bool);
    }
    solver.assert(&concrete_bool);
    solver.assert(&contract_bool.not());
    let sat_result = solver.check();
    solver.pop(1);

    if sat_result == SatResult::Sat {
        return Err(MumeiError::verification(format!(
            "Contract subsumption failed: atom_ref({}) passed to {}.{} — concrete ensures '{}' does not imply contract ensures '{}'",
            concrete_atom.name, callee_name, param_name, concrete_atom.ensures, contract_ensures
        )));
    }
    if sat_result == SatResult::Unknown {
        return Err(MumeiError::verification(format!(
            "Contract subsumption for atom_ref({}) passed to {}.{} could not be decided (Z3 unknown)",
            concrete_atom.name, callee_name, param_name
        )));
    }
    Ok(())
}

pub(crate) fn propagate_equality_from_ensures<'a>(
    vc: &VCtx<'a>,
    expr: &Expr,
    result_z3: &Dynamic<'a>,
    call_env: &mut Env<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> MumeiResult<()> {
    match expr {
        // && で結合された複合条件: 左右を再帰的に処理
        Expr::BinaryOp(left, Op::And, right) => {
            propagate_equality_from_ensures(vc, left, result_z3, call_env, solver_opt)?;
            propagate_equality_from_ensures(vc, right, result_z3, call_env, solver_opt)?;
        }
        // result == <expr> の等式
        Expr::BinaryOp(left, Op::Eq, right) => {
            let is_result_left = matches!(left.as_ref(), Expr::Variable(ref v) if v == "result");
            let is_result_right = matches!(right.as_ref(), Expr::Variable(ref v) if v == "result");

            if is_result_left {
                if let Ok(rhs_val) = expr_to_z3(vc, right, call_env, None) {
                    if let Some(solver) = solver_opt {
                        assert_result_equality(vc, solver, result_z3, &rhs_val);
                    }
                }
            } else if is_result_right {
                if let Ok(lhs_val) = expr_to_z3(vc, left, call_env, None) {
                    if let Some(solver) = solver_opt {
                        assert_result_equality(vc, solver, result_z3, &lhs_val);
                    }
                }
            }
        }
        _ => {
            // 等式でも && でもない条件はスキップ（既に全体の ensures として assert 済み）
        }
    }
    Ok(())
}

/// `result == <expr>` の等式伝搬で、シンボリック result を値に束縛する。
/// result のソート（Int / Real / Float / Bool / String）ごとに値側を必要に
/// 応じて強制変換する。ソートが合わず強制変換もできない場合は何も
/// assert しない（全体の ensures assert には影響しない）。
pub(crate) fn assert_result_equality<'a>(
    vc: &VCtx<'a>,
    solver: &Solver<'a>,
    result_z3: &Dynamic<'a>,
    value: &Dynamic<'a>,
) {
    if let (Some(res), Some(val)) = (result_z3.as_int(), value.as_int()) {
        solver.assert(&res._eq(&val));
    } else if let Some(res) = result_z3.as_real() {
        if let Some(val) = value
            .as_real()
            .or_else(|| value.as_int().map(|i| i.to_real()))
        {
            solver.assert(&res._eq(&val));
        }
    } else if let Some(res) = result_z3.as_float() {
        let rne = round_nearest_even(vc.ctx);
        if let Some(val) = coerce_to_float(vc.ctx, value, &rne) {
            solver.assert(&res._eq(&val));
        }
    } else if let (Some(res), Some(val)) = (result_z3.as_bool(), value.as_bool()) {
        solver.assert(&res._eq(&val));
    } else if let (Some(res), Some(val)) = (result_z3.as_string(), value.as_string()) {
        solver.assert(&res._eq(&val));
    }
}
