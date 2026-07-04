#![allow(unused_imports)]
use super::super::support::*;
use super::super::*;
use super::*;
use crate::lowering::{lower, LoweredType};
use serde_json::json;

pub(crate) fn expr_to_z3<'a>(
    vc: &VCtx<'a>,
    expr: &Expr,
    env: &mut Env<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> DynResult<'a> {
    // Per-atom constraint budget: increment count and check limit.
    // This tracks the number of Z3 AST nodes generated, which correlates
    // with solver.assert() pressure and overall verification complexity.
    if solver_opt.is_some() {
        let atom_name = vc
            .current_atom
            .map(|a| a.name.as_str())
            .unwrap_or("<unknown>");
        check_constraint_budget(vc, atom_name)?;
    }

    let ctx = vc.ctx;
    match expr {
        Expr::Number(n) => Ok(Int::from_i64(ctx, *n).into()),
        Expr::Float(f) => {
            if vc.ieee754_f64 {
                // Opt-in IEEE 754: preserve the exact binary64 bit pattern.
                Ok(float_from_f64(ctx, *f).into())
            } else {
                // Default: exact-rational `Real` encoding.
                Ok(real_from_f64(ctx, *f).into())
            }
        }
        // Plan 9: String literal to Z3 String Sort
        Expr::StringLit(s) => Ok(Z3String::from_str(ctx, s).unwrap().into()),
        Expr::Variable(name) => {
            // Wire check_alive() into variable access: if the variable has been
            // consumed, report a use-after-consume error.
            if let Some(lctx_cell) = vc.linearity_ctx {
                if let Err(e) = lctx_cell.borrow_mut().check_alive(name) {
                    return Err(MumeiError::verification(format!(
                        "Linearity violation: {}",
                        e
                    )));
                }
            }
            // The parser represents `true` / `false` boolean literals as
            // Expr::Variable("true"|"false"). Without special handling they
            // would be resolved as Int consts via the unwrap_or_else fallback
            // below, which breaks `&&`/`||` operands. This also makes
            // strip_quantifiers' `forall(...)` → `true` substitution safe for
            // non-trusted atoms that mix forall with `&&`.
            if let Some(existing) = env.get(name) {
                return Ok(existing.clone());
            }
            if name == "true" {
                return Ok(Bool::from_bool(ctx, true).into());
            }
            if name == "false" {
                return Ok(Bool::from_bool(ctx, false).into());
            }
            Ok(Int::new_const(ctx, name.as_str()).into())
        }
        Expr::Call(name, args) => {
            if is_temporal_witness_predicate(name) {
                return temporal_witness_call_to_z3(vc, name, args, env, solver_opt);
            }
            match name.as_str() {
                // =============================================================
                // ensures / invariant 内の forall/exists 量化子サポート
                // =============================================================
                // forall(var, start, end, condition) → Z3 ∀ var ∈ [start, end). condition
                // exists(var, start, end, condition) → Z3 ∃ var ∈ [start, end). condition
                //
                // これにより ensures: forall(i, 0, result - 1, arr[i] <= arr[i+1])
                // のようなソート済み不変量を事後条件として記述・検証できる。
                "forall" | "exists" => {
                    if args.len() != 4 {
                        return Err(MumeiError::verification(format!(
                            "{}() requires exactly 4 arguments: (var, start, end, condition)",
                            name
                        )));
                    }
                    // 第1引数: 束縛変数名
                    let var_name = match &args[0] {
                        Expr::Variable(v) => v.clone(),
                        _ => {
                            return Err(MumeiError::verification(format!(
                                "{}(): first argument must be a variable name",
                                name
                            )))
                        }
                    };

                    // 第2引数: 範囲の開始
                    let start_z3 = expr_to_z3(vc, &args[1], env, None)?.as_int().ok_or(
                        MumeiError::type_error(format!("{}(): start must be integer", name)),
                    )?;

                    // 第3引数: 範囲の終了
                    let end_z3 = expr_to_z3(vc, &args[2], env, None)?.as_int().ok_or(
                        MumeiError::type_error(format!("{}(): end must be integer", name)),
                    )?;

                    // 束縛変数を一時的に env に追加して condition を評価
                    let bound_var = Int::new_const(ctx, var_name.as_str());
                    let old_val = env.insert(var_name.clone(), bound_var.clone().into());

                    let range_cond =
                        Bool::and(ctx, &[&bound_var.ge(&start_z3), &bound_var.lt(&end_z3)]);

                    let condition_z3 = expr_to_z3(vc, &args[3], env, None)?.as_bool().ok_or(
                        MumeiError::type_error(format!("{}(): condition must be boolean", name)),
                    )?;

                    // 束縛変数を env から復元
                    if let Some(old) = old_val {
                        env.insert(var_name, old);
                    } else {
                        env.remove(&var_name);
                    }

                    let quantifier_expr = if name == "forall" {
                        // ∀ var ∈ [start, end). condition
                        // Build E-matching patterns from `arr[idx]` accesses
                        // inside the condition. Mirrors the requires-side
                        // pattern extraction at L5950-5979 so that array-
                        // heavy `ensures` / loop invariants instantiate the
                        // same way as their requires counterparts. Each
                        // access uses `__z3_arr_<name>` when present so that
                        // patterns refer to the post-store array state seen
                        // by the body of the forall.
                        let arr_accesses = collect_array_accesses(&args[3]);
                        let mut pattern_asts: Vec<Dynamic> = Vec::new();
                        // Re-bind the quantified variable while we lower
                        // index expressions so that they see `var_name`.
                        let var_re = match &args[0] {
                            Expr::Variable(v) => v.clone(),
                            _ => String::new(),
                        };
                        let saved = if !var_re.is_empty() {
                            env.insert(var_re.clone(), bound_var.clone().into())
                        } else {
                            None
                        };
                        for (arr_name, idx_expr) in &arr_accesses {
                            if let Ok(idx_z3) = expr_to_z3(vc, idx_expr, env, None) {
                                if let Some(idx_int) = idx_z3.as_int() {
                                    pattern_asts
                                        .push(z3_dynamic_array(vc, arr_name, env).select(&idx_int));
                                }
                            }
                        }
                        // Restore env for var_re
                        if !var_re.is_empty() {
                            if let Some(old) = saved {
                                env.insert(var_re, old);
                            } else {
                                env.remove(&var_re);
                            }
                        }
                        let body = range_cond.implies(&condition_z3);
                        if pattern_asts.is_empty() {
                            z3::ast::forall_const(ctx, &[&bound_var], &[], &body)
                        } else {
                            let pattern_refs: Vec<&dyn z3::ast::Ast> = pattern_asts
                                .iter()
                                .map(|d| d as &dyn z3::ast::Ast)
                                .collect();
                            let pattern = z3::Pattern::new(ctx, &pattern_refs);
                            z3::ast::forall_const(ctx, &[&bound_var], &[&pattern], &body)
                        }
                    } else {
                        // ∃ var ∈ [start, end). condition
                        z3::ast::exists_const(
                            ctx,
                            &[&bound_var],
                            &[],
                            &Bool::and(ctx, &[&range_cond, &condition_z3]),
                        )
                    };

                    Ok(quantifier_expr.into())
                }
                "len" => {
                    // len(arr_name) → 配列名に紐づくシンボリック長を返す
                    // len_<name> >= 0 の制約を自動付与
                    let arr_name = if !args.is_empty() {
                        if let Expr::Variable(name) = &args[0] {
                            name.clone()
                        } else {
                            "arr".to_string()
                        }
                    } else {
                        "arr".to_string()
                    };
                    let len_name = format!("len_{}", arr_name);
                    let len_var = Int::new_const(ctx, len_name.as_str());
                    if let Some(solver) = solver_opt {
                        solver.assert(&len_var.ge(&Int::from_i64(ctx, 0)));
                    }
                    env.insert(len_name, len_var.clone().into());
                    Ok(len_var.into())
                }
                "sqrt" => {
                    // Z3 0.12 の Float には sqrt メソッドがないため、
                    // シンボリック変数として扱い、sqrt(x) >= 0 の制約を付与
                    let _val = expr_to_z3(vc, &args[0], env, solver_opt)?;
                    let result = Float::new_const(ctx, "sqrt_result", 11, 53);
                    if let Some(solver) = solver_opt {
                        let zero = Float::from_f64(ctx, 0.0);
                        solver.assert(&result.ge(&zero));
                    }
                    Ok(result.into())
                }
                "cast_to_int" => {
                    // Z3 0.12 では Float->Int 直接変換がないため、シンボリック整数を返す
                    let _val = expr_to_z3(vc, &args[0], env, solver_opt)?;
                    Ok(Int::new_const(ctx, "cast_result").into())
                }
                // =============================================================
                // Built-in string constraint functions for requires/ensures
                // =============================================================
                // These are parsed as Expr::Call by the parser but need special
                // handling in Z3 to produce Bool constraints on Z3 String Sort.
                "starts_with" | "ends_with" | "contains" | "not_contains" => {
                    if args.len() != 2 {
                        return Err(MumeiError::verification(format!(
                            "{}() requires exactly 2 arguments: (string_var, \"pattern\")",
                            name
                        )));
                    }
                    let str_val = expr_to_z3(vc, &args[0], env, solver_opt)?;
                    let pattern_val = expr_to_z3(vc, &args[1], env, solver_opt)?;

                    // Both arguments must be Z3 String Sort
                    if let (Some(str_z3), Some(pat_z3)) =
                        (str_val.as_string(), pattern_val.as_string())
                    {
                        mark_string_constraints(vc);
                        let result: Bool = match name.as_str() {
                            "starts_with" => pat_z3.prefix(&str_z3),
                            "ends_with" => pat_z3.suffix(&str_z3),
                            "contains" => str_z3.contains(&pat_z3),
                            "not_contains" => str_z3.contains(&pat_z3).not(),
                            _ => unreachable!(),
                        };
                        Ok(result.into())
                    } else {
                        // Fallback: if operands are not strings, return true (no constraint).
                        // This handles cases where the variable hasn't been typed as Str
                        // (e.g., an i64 parameter). Str-typed parameters are correctly
                        // lowered to Z3 String Sort at parameter pre-registration, so this
                        // branch only fires for genuinely non-string variables.
                        //
                        // NOTE: This is a permissive fallback. If a user writes
                        // `not_contains(int_var, "..")` where int_var is i64, the constraint
                        // is silently dropped. This is acceptable because string constraint
                        // functions are only meaningful on Str-typed parameters, and using
                        // them on non-Str types is a user error that should ideally be
                        // caught by a type checker (not yet implemented for requires/ensures).
                        Ok(Bool::from_bool(ctx, true).into())
                    }
                }
                _ => {
                    // ユーザー定義関数呼び出し: 契約による検証（Compositional Verification）
                    // 呼び出し先の requires を現在のコンテキストで証明し、
                    // 成功すれば ensures を事実として追加する
                    //
                    // FQN dot-notation サポート:
                    // "math.add" → "math::add" として ModuleEnv から解決する。
                    // これにより `math.add(x, y)` と `math::add(x, y)` の両方が動作する。
                    let fqn_name = name.replace('.', "::");
                    let resolved_callee = vc
                        .module_env
                        .get_atom(name)
                        .cloned()
                        .or_else(|| vc.module_env.get_atom(&fqn_name).cloned());
                    if let Some(callee) = resolved_callee {
                        // 引数を評価
                        let mut arg_vals = Vec::new();
                        for arg in args {
                            arg_vals.push(expr_to_z3(vc, arg, env, solver_opt)?);
                        }

                        // 仮引数名と実引数値の対応を構築
                        let mut call_env = env.clone();
                        for (i, param) in callee.params.iter().enumerate() {
                            if let Some(val) = arg_vals.get(i) {
                                call_env.insert(param.name.clone(), val.clone());
                            }
                        }

                        // 呼び出し先の精緻型制約を call_env に適用
                        for (i, param) in callee.params.iter().enumerate() {
                            if let Some(type_name) = &param.type_name {
                                if let Some(refined) = vc.module_env.get_type(type_name) {
                                    // 実引数値を精緻型の述語変数に束縛して制約を検証
                                    if let Some(val) = arg_vals.get(i) {
                                        call_env.insert(refined.operand.clone(), val.clone());
                                    }
                                }
                            }
                        }

                        // Wire borrow()/consume() into call-site argument handling.
                        // For each callee parameter, if it is `ref`/`ref mut`, call borrow().
                        // If the callee has `consumed_params`, call consume() for the
                        // corresponding argument variable.
                        if let Some(lctx_cell) = vc.linearity_ctx {
                            // Handle ref/ref mut parameters → borrow
                            for (i, param) in callee.params.iter().enumerate() {
                                if param.is_ref || param.is_ref_mut {
                                    if let Some(Expr::Variable(arg_name)) = args.get(i) {
                                        let _ = lctx_cell.borrow_mut().borrow(arg_name, name);
                                    }
                                }
                            }
                            // Handle consumed_params → consume
                            for consumed_name in &callee.consumed_params {
                                if let Some(idx) =
                                    callee.params.iter().position(|p| p.name == *consumed_name)
                                {
                                    if let Some(Expr::Variable(arg_name)) = args.get(idx) {
                                        if let Err(e) = lctx_cell.borrow_mut().consume(arg_name) {
                                            return Err(MumeiError::verification(format!(
                                                "Linearity violation at call to '{}': {}",
                                                name, e
                                            )));
                                        }
                                    }
                                }
                            }
                        }

                        // requires の検証: 呼び出し元のコンテキストで事前条件が満たされるか
                        if callee.requires.trim() != "true" {
                            if let Some(solver) = solver_opt {
                                let req_ast = parse_expression(&callee.requires);
                                let req_z3 = expr_to_z3(vc, &req_ast, &mut call_env, None)?;
                                if let Some(req_bool) = req_z3.as_bool() {
                                    solver.push();
                                    solver.assert(&req_bool.not());
                                    if solver.check() == SatResult::Sat {
                                        // Extract counterexample: concrete argument values
                                        // that violate the callee's precondition.
                                        let ce_value = if let Some(model) = solver.get_model() {
                                            let mut ce_json = serde_json::Map::new();
                                            for (i, param) in callee.params.iter().enumerate() {
                                                if let Some(arg_val) = arg_vals.get(i) {
                                                    if let Some(val) = model.eval(arg_val, true) {
                                                        let val_str = format!("{}", val);
                                                        ce_json.insert(
                                                            param.name.clone(),
                                                            json!(val_str),
                                                        );
                                                    }
                                                }
                                            }
                                            if ce_json.is_empty() {
                                                None
                                            } else {
                                                Some(serde_json::Value::Object(ce_json))
                                            }
                                        } else {
                                            None
                                        };
                                        solver.pop(1);
                                        return Err(MumeiError::verification(
                                            format!("Call to '{}': precondition (requires) not satisfied at call site", name)
                                        ).with_help("呼び出し元で事前条件を満たしていません。引数の制約を確認してください")
                                        .with_counterexample(ce_value));
                                    }
                                    solver.pop(1);
                                }
                            }
                        }

                        // Trait method param_constraints の検証:
                        // 呼び出し先がトレイトメソッドの実装である場合、param_constraints を
                        // 呼び出し元のコンテキストで検証する。
                        // get_traits_for_method で全候補を取得し、find_impl で callee の型に
                        // 対して実際にトレイトが impl されている候補のみ適用する。
                        if let Some(solver) = solver_opt {
                            let callee_type = callee
                                .params
                                .first()
                                .and_then(|p| p.type_name.as_deref())
                                .unwrap_or("i64");
                            let candidates = vc.module_env.get_traits_for_method(name);
                            // find_impl で正しいトレイトを絞り込む
                            let matched = candidates
                                .iter()
                                .find(|(tn, _)| vc.module_env.find_impl(tn, callee_type).is_some());
                            if let Some((_trait_name, trait_method)) = matched {
                                for (i, constraint_opt) in
                                    trait_method.param_constraints.iter().enumerate()
                                {
                                    if let Some(constraint) = constraint_opt {
                                        if let Some(arg_val) = arg_vals.get(i) {
                                            let param_name = callee
                                                .params
                                                .get(i)
                                                .map(|p| p.name.as_str())
                                                .unwrap_or("v");
                                            let concrete_constraint: String =
                                                replace_constraint_placeholder(
                                                    constraint, param_name,
                                                );
                                            let mut constraint_env: Env = env.clone();
                                            constraint_env
                                                .insert(param_name.to_string(), arg_val.clone());
                                            let constraint_ast =
                                                parse_expression(&concrete_constraint);
                                            if let Ok(constraint_z3) = expr_to_z3(
                                                vc,
                                                &constraint_ast,
                                                &mut constraint_env,
                                                None,
                                            ) {
                                                if let Some(constraint_bool) =
                                                    constraint_z3.as_bool()
                                                {
                                                    solver.push();
                                                    solver.assert(&constraint_bool.not());
                                                    if solver.check() == SatResult::Sat {
                                                        solver.pop(1);
                                                        return Err(MumeiError::verification(
                                                            format!(
                                                                "Call to '{}': trait method parameter constraint '{}' not satisfied for argument {}",
                                                                name, constraint, i
                                                            )
                                                        ).with_help(
                                                            "トレイトメソッドのパラメータ制約が満たされていません。引数の値を確認してください"
                                                        ));
                                                    }
                                                    solver.pop(1);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // ensures からシンボリック結果を生成し、事後条件を事実として追加
                        static CALL_COUNTER: std::sync::atomic::AtomicUsize =
                            std::sync::atomic::AtomicUsize::new(0);
                        let call_id =
                            CALL_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        let result_name = format!("call_{}_{}", name, call_id);

                        // 戻り値型の推定: 呼び出し先パラメータに f64 型があれば Float、なければ Int
                        let has_float = callee.params.iter().any(|p| {
                            p.type_name
                                .as_deref()
                                .map(|t| vc.module_env.resolve_base_type(t) == "f64")
                                .unwrap_or(false)
                        });
                        let result_z3: Dynamic = if has_float {
                            Float::new_const(ctx, result_name.as_str(), 11, 53).into()
                        } else {
                            Int::new_const(ctx, result_name.as_str()).into()
                        };

                        // ensures を事実として solver に追加（result を呼び出し結果に束縛）
                        //
                        // Equality Ensures Propagation:
                        // ensures 内に `result == expr` の形式の等式が含まれる場合、
                        // シンボリック result を具体的な式に直接束縛する。
                        // これにより `let x = increment(n);` で `x == n + 1` が
                        // 呼び出し元のコンテキストに伝播し、連鎖呼び出しの検証精度が向上する。
                        //
                        // 例: ensures: result == n + 1;
                        //   → call_env に result = call_increment_0 を挿入
                        //   → Z3 に call_increment_0 == n + 1 を assert
                        //   → 後続の `increment(x)` で x >= 1 だけでなく x == n + 1 が使える
                        if callee.ensures.trim() != "true" {
                            call_env.insert("result".to_string(), result_z3.clone());
                            let ens_ast = parse_expression(&callee.ensures);

                            // Equality ensures の特別処理:
                            // ensures が `result == expr` の形式の場合、
                            // expr を評価して result と等価であることを直接 assert する。
                            // これにより Z3 が等式を完全に活用できる。
                            let ens_z3 = expr_to_z3(vc, &ens_ast, &mut call_env, None)?;
                            if let Some(ens_bool) = ens_z3.as_bool() {
                                if let Some(solver) = solver_opt {
                                    solver.assert(&ens_bool);
                                }
                            }

                            // 追加: ensures 式が `result == expr` の形式かチェックし、
                            // 該当する場合は result のシンボリック値に対して
                            // 等式制約を明示的に追加する（Z3 の等式推論を強化）
                            if let Expr::BinaryOp(left, Op::Eq, right) = &ens_ast {
                                if let Expr::Variable(ref var_name) = left.as_ref() {
                                    if var_name == "result" {
                                        // ensures: result == <expr> の場合
                                        // <expr> を call_env で評価し、result_z3 == eval(<expr>) を assert
                                        if let Ok(rhs_val) =
                                            expr_to_z3(vc, right, &mut call_env, None)
                                        {
                                            if let Some(solver) = solver_opt {
                                                if let (Some(res_int), Some(rhs_int)) =
                                                    (result_z3.as_int(), rhs_val.as_int())
                                                {
                                                    solver.assert(&res_int._eq(&rhs_int));
                                                } else if let (Some(res_float), Some(rhs_float)) =
                                                    (result_z3.as_float(), rhs_val.as_float())
                                                {
                                                    solver.assert(&res_float._eq(&rhs_float));
                                                }
                                            }
                                        }
                                    }
                                }
                                // ensures: <expr> == result の逆順もサポート
                                if let Expr::Variable(ref var_name) = right.as_ref() {
                                    if var_name == "result" {
                                        if let Ok(lhs_val) =
                                            expr_to_z3(vc, left, &mut call_env, None)
                                        {
                                            if let Some(solver) = solver_opt {
                                                if let (Some(res_int), Some(lhs_int)) =
                                                    (result_z3.as_int(), lhs_val.as_int())
                                                {
                                                    solver.assert(&res_int._eq(&lhs_int));
                                                } else if let (Some(res_float), Some(lhs_float)) =
                                                    (result_z3.as_float(), lhs_val.as_float())
                                                {
                                                    solver.assert(&res_float._eq(&lhs_float));
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            // 複合 ensures（&& で結合された複数条件）内の等式も伝播
                            // ensures: result >= 0 && result == n + 1 のような場合
                            propagate_equality_from_ensures(
                                vc,
                                &ens_ast,
                                &result_z3,
                                &mut call_env,
                                solver_opt,
                            )?;
                        }

                        if let Some(solver) = solver_opt {
                            assert_temporal_effect_transition(
                                vc,
                                solver,
                                &callee.name,
                                &arg_vals,
                                &result_z3,
                            )?;
                        }

                        // Release borrows after call returns.
                        // Once the callee has finished, ref/ref mut borrows are released
                        // so the owner can be consumed or re-borrowed.
                        if let Some(lctx_cell) = vc.linearity_ctx {
                            for (i, param) in callee.params.iter().enumerate() {
                                if param.is_ref || param.is_ref_mut {
                                    if let Some(Expr::Variable(arg_name)) = args.get(i) {
                                        lctx_cell.borrow_mut().release_borrow(arg_name, name);
                                    }
                                }
                            }
                        }

                        // =============================================================
                        // Subsumption Check: atom_ref argument vs contract ensures
                        // =============================================================
                        // When a callee parameter has fn_contract_ensures and the
                        // corresponding argument is atom_ref(concrete_name), verify
                        // that the concrete atom's ensures implies the contract's
                        // ensures.  Emit a warning (not a hard error) to maintain
                        // backward compatibility.
                        if let Some(solver) = solver_opt {
                            for (i, param) in callee.params.iter().enumerate() {
                                if let Some(ref contract_ensures) = param.fn_contract_ensures {
                                    if let Some(Expr::AtomRef {
                                        name: ref concrete_name,
                                    }) = args.get(i)
                                    {
                                        if let Some(concrete_atom) =
                                            vc.module_env.get_atom(concrete_name).cloned()
                                        {
                                            check_contract_subsumption(
                                                vc,
                                                &concrete_atom,
                                                contract_ensures,
                                                param.fn_contract_requires.as_deref(),
                                                name,
                                                &param.name,
                                                solver,
                                                ctx,
                                            );
                                        }
                                    }
                                }
                            }
                        }

                        // Taint Analysis: 呼び出し先が unverified の場合、
                        // 戻り値を __tainted_ マーカーで汚染済みとしてマークする。
                        if callee.trust_level == TrustLevel::Unverified {
                            let taint_key = format!("__tainted_{}", result_name);
                            let taint_marker = Bool::from_bool(ctx, true);
                            env.insert(taint_key, taint_marker.into());
                        }

                        Ok(result_z3)
                    } else {
                        Err(MumeiError::verification(format!(
                            "Unknown function: {}",
                            name
                        )))
                    }
                }
            }
        }
        Expr::ArrayAccess(name, index_expr) => {
            let idx = expr_to_z3(vc, index_expr, env, solver_opt)?
                .as_int()
                .ok_or(MumeiError::type_error("Index must be integer"))?;

            // 配列名に紐づく長さシンボルを使った境界チェック
            if let Some(solver) = solver_opt {
                let len_name = format!("len_{}", name);
                let len = if let Some(existing) = env.get(&len_name) {
                    existing
                        .as_int()
                        .unwrap_or(Int::new_const(ctx, len_name.as_str()))
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
                        "Potential Out-of-Bounds on '{}' (index may be < 0 or >= len_{})",
                        name, name
                    ))
                    .with_help(
                        "requires にインデックスの範囲制約 (0 <= idx < len) を追加してください",
                    ));
                }
                solver.pop(1);
            }
            Ok(z3_dynamic_array(vc, name, env).select(&idx))
        }
        Expr::BinaryOp(left, op, right) => {
            let l = expr_to_z3(vc, left, env, solver_opt)?;
            let r = expr_to_z3(vc, right, env, solver_opt)?;

            // Plan 9-8: String concatenation — if both operands are Z3 String Sort
            if l.get_sort() == z3::Sort::string(ctx) && r.get_sort() == z3::Sort::string(ctx) {
                mark_string_constraints(vc);
                let ls = l.as_string().ok_or("Expected string for Str op")?;
                let rs = r.as_string().ok_or("Expected string for Str op")?;
                return match op {
                    Op::Add => {
                        // Z3 string concatenation — return concat directly
                        let result = z3::ast::String::concat(ctx, &[&ls, &rs]);
                        Ok(result.into())
                    }
                    Op::Eq => Ok(ls._eq(&rs).into()),
                    Op::Neq => Ok(ls._eq(&rs).not().into()),
                    _ => Err(format!("Unsupported operator {:?} for Str type", op).into()),
                };
            }

            if l.as_real().is_some() || r.as_real().is_some() {
                let lr = l
                    .as_real()
                    .or_else(|| l.as_int().map(|i| i.to_real()))
                    .unwrap_or_else(|| Real::from_real(ctx, 0, 1));
                let rr = r
                    .as_real()
                    .or_else(|| r.as_int().map(|i| i.to_real()))
                    .unwrap_or_else(|| Real::from_real(ctx, 0, 1));
                match op {
                    Op::Gt => Ok(lr.gt(&rr).into()),
                    Op::Lt => Ok(lr.lt(&rr).into()),
                    Op::Ge => Ok(lr.ge(&rr).into()),
                    Op::Le => Ok(lr.le(&rr).into()),
                    Op::Eq => Ok(lr._eq(&rr).into()),
                    Op::Neq => Ok(lr._eq(&rr).not().into()),
                    Op::Add => Ok(Real::add(ctx, &[&lr, &rr]).into()),
                    Op::Sub => Ok(Real::sub(ctx, &[&lr, &rr]).into()),
                    Op::Mul => Ok(Real::mul(ctx, &[&lr, &rr]).into()),
                    Op::Div => Ok(lr.div(&rr).into()),
                    _ => Err("Invalid real op".into()),
                }
            } else if l.as_float().is_some() || r.as_float().is_some() {
                // Opt-in IEEE 754 (`--ieee754-f64`) path: `f64` values are Z3
                // binary64 `Float`s. Lower arithmetic to the FP theory with the
                // round-nearest-ties-to-even mode (matching hardware `f64`) and
                // use `fp` comparisons / `fp.eq` — never an unconstrained fresh
                // constant, so the actual FP semantics are modeled.
                let rne = round_nearest_even(ctx);
                let lf = coerce_to_float(ctx, &l, &rne).ok_or("Expected float operand")?;
                let rf = coerce_to_float(ctx, &r, &rne).ok_or("Expected float operand")?;
                match op {
                    Op::Gt => Ok(lf.gt(&rf).into()),
                    Op::Lt => Ok(lf.lt(&rf).into()),
                    Op::Ge => Ok(lf.ge(&rf).into()),
                    Op::Le => Ok(lf.le(&rf).into()),
                    Op::Eq => Ok(float_eq(ctx, &lf, &rf).into()),
                    Op::Neq => Ok(float_eq(ctx, &lf, &rf).not().into()),
                    Op::Add => Ok(rne.add(&lf, &rf).into()),
                    Op::Sub => Ok(rne.sub(&lf, &rf).into()),
                    Op::Mul => Ok(rne.mul(&lf, &rf).into()),
                    Op::Div => Ok(rne.div(&lf, &rf).into()),
                    _ => Err("Invalid float op".into()),
                }
            } else {
                // Boolean 演算子は as_int() の前に処理する（オペランドが Bool のため）
                match op {
                    Op::And => {
                        let lb = l.as_bool().ok_or("Expected bool for &&")?;
                        let rb = r.as_bool().ok_or("Expected bool for &&")?;
                        return Ok(Bool::and(ctx, &[&lb, &rb]).into());
                    }
                    Op::Or => {
                        let lb = l.as_bool().ok_or("Expected bool for ||")?;
                        let rb = r.as_bool().ok_or("Expected bool for ||")?;
                        return Ok(Bool::or(ctx, &[&lb, &rb]).into());
                    }
                    Op::Implies => {
                        let lb = l.as_bool().ok_or("Expected bool for =>")?;
                        let rb = r.as_bool().ok_or("Expected bool for =>")?;
                        return Ok(lb.implies(&rb).into());
                    }
                    Op::Eq | Op::Neq if l.as_bool().is_some() || r.as_bool().is_some() => {
                        let lb = l.as_bool().ok_or("Expected bool for ==")?;
                        let rb = r.as_bool().ok_or("Expected bool for ==")?;
                        let eq = lb._eq(&rb);
                        return Ok(if matches!(op, Op::Neq) {
                            eq.not().into()
                        } else {
                            eq.into()
                        });
                    }
                    _ => {}
                }
                let li = l.as_int().ok_or("Expected int")?;
                let ri = r.as_int().ok_or("Expected int")?;
                match op {
                    Op::Add => Ok((&li + &ri).into()),
                    Op::Sub => Ok((&li - &ri).into()),
                    Op::Mul => Ok((&li * &ri).into()),
                    Op::Div => {
                        if let Some(solver) = solver_opt {
                            solver.push();
                            solver.assert(&ri._eq(&Int::from_i64(ctx, 0)));
                            if solver.check() == SatResult::Sat {
                                // Extract counterexample: find which variables cause divisor == 0
                                let (ce_hint, div_feedback) =
                                    if let Some(model) = solver.get_model() {
                                        let divisor_val = model
                                            .eval(&ri, true)
                                            .map(|v| format!("{}", v))
                                            .unwrap_or_else(|| "0".to_string());
                                        let dividend_val = model
                                            .eval(&li, true)
                                            .map(|v| format!("{}", v))
                                            .unwrap_or_else(|| "?".to_string());
                                        let hint = format!(
                                            " Counter-example: dividend = {}, divisor = {}",
                                            dividend_val, divisor_val
                                        );
                                        let fb = build_division_by_zero_feedback(
                                            &dividend_val,
                                            &divisor_val,
                                        );
                                        (hint, Some(fb))
                                    } else {
                                        (String::new(), None)
                                    };
                                // Attach structured feedback to error message for upstream reporting
                                let feedback_hint = div_feedback
                                    .as_ref()
                                    .map(|fb| format!(" [semantic_feedback: {}]", fb))
                                    .unwrap_or_default();
                                let ce_value = if let Some(model) = solver.get_model() {
                                    let dividend_val = model
                                        .eval(&li, true)
                                        .map(|v| format!("{}", v))
                                        .unwrap_or_else(|| "?".to_string());
                                    let divisor_val = model
                                        .eval(&ri, true)
                                        .map(|v| format!("{}", v))
                                        .unwrap_or_else(|| "0".to_string());
                                    Some(serde_json::json!({
                                        "dividend": dividend_val,
                                        "divisor": divisor_val,
                                    }))
                                } else {
                                    None
                                };
                                solver.pop(1);
                                return Err(MumeiError::verification(format!(
                                    "Potential division by zero.{}{}",
                                    ce_hint, feedback_hint
                                ))
                                .with_help("Add a condition divisor != 0 to requires")
                                .with_counterexample(ce_value));
                            }
                            solver.pop(1);
                        }
                        Ok((&li / &ri).into())
                    }
                    Op::Gt => Ok(li.gt(&ri).into()),
                    Op::Lt => Ok(li.lt(&ri).into()),
                    Op::Ge => Ok(li.ge(&ri).into()),
                    Op::Le => Ok(li.le(&ri).into()),
                    Op::Eq => Ok(li._eq(&ri).into()),
                    Op::Neq => Ok(li._eq(&ri).not().into()),
                    _ => Err(MumeiError::verification(format!(
                        "Unsupported int operator {:?}",
                        op
                    ))),
                }
            }
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            let c = expr_to_z3(vc, cond, env, solver_opt)?
                .as_bool()
                .ok_or(MumeiError::type_error("If condition must be boolean"))?;
            // Track the path condition for sub-statements that need it
            // (currently only the loop-invariant base case inside the
            // branch), without touching the solver's persistent assertion
            // stack. This lets, e.g., `if n <= 1 { n } else { let i = 1;
            // while i < n invariant: i >= 1 && i <= n … }` verify because
            // the inner invariant base case can rely on `n > 1` from the
            // surrounding else-guard, while still keeping `let left =
            // merge_sort(mid)` ensures-asserts (`left == mid`) live for the
            // outer postcondition check.
            vc.path_cond_stack.borrow_mut().push(c.clone());
            let t = stmt_to_z3(vc, then_branch, env, solver_opt);
            vc.path_cond_stack.borrow_mut().pop();
            let t = t?;
            vc.path_cond_stack.borrow_mut().push(c.not());
            let e = stmt_to_z3(vc, else_branch, env, solver_opt);
            vc.path_cond_stack.borrow_mut().pop();
            let e = e?;
            Ok(c.ite(&t, &e))
        }

        Expr::StructInit { type_name, fields } => {
            // 構造体の各フィールドを検証し、env に登録
            // フィールドに精緻型制約がある場合は solver で検証する
            let mut last: Dynamic = Int::from_i64(ctx, 0).into();
            for (field_name, field_expr) in fields {
                let val = expr_to_z3(vc, field_expr, env, solver_opt)?;
                let qualified_name = format!("__struct_{}_{}", type_name, field_name);
                env.insert(qualified_name, val.clone());
                last = val.clone();

                // フィールド制約の検証: 構造体定義から constraint を取得
                if let Some(sdef) = vc.module_env.get_struct(type_name) {
                    if let Some(sfield) = sdef.fields.iter().find(|f| f.name == *field_name) {
                        if let Some(constraint_raw) = &sfield.constraint {
                            // constraint 内の "v" をフィールド値に置き換えて検証
                            let mut local_env = env.clone();
                            local_env.insert("v".to_string(), val.clone());
                            let constraint_ast = parse_expression(constraint_raw);
                            let constraint_z3 =
                                expr_to_z3(vc, &constraint_ast, &mut local_env, None)?;
                            if let Some(constraint_bool) = constraint_z3.as_bool() {
                                if let Some(solver) = solver_opt {
                                    solver.push();
                                    solver.assert(&constraint_bool.not());
                                    if solver.check() == SatResult::Sat {
                                        solver.pop(1);
                                        return Err(MumeiError::verification(format!(
                                            "Struct '{}' field '{}' constraint violated: {}",
                                            type_name, field_name, constraint_raw
                                        )));
                                    }
                                    solver.pop(1);
                                }
                            }
                        }
                    }
                }
            }
            Ok(last)
        }
        Expr::Match { target, arms } => {
            let target_z3 = expr_to_z3(vc, target, env, solver_opt)?;

            // ========================================================
            // Enum ドメイン制約の自動注入
            // ========================================================
            // アームに Variant パターンが含まれる場合、対応する EnumDef を探し、
            // target の値域を 0..n_variants に制約する。
            // これにより Z3 が「これら以外のバリアントは存在しない」ことを知り、
            // 網羅性チェックの信頼性が 100% になる。
            if let Some(solver) = solver_opt {
                if let Some(enum_def) = detect_enum_from_arms(arms, vc.module_env) {
                    let n = enum_def.variants.len() as i64;
                    if let Some(tag_int) = target_z3.as_int() {
                        // tag ∈ [0, n_variants)
                        solver.assert(&tag_int.ge(&Int::from_i64(ctx, 0)));
                        solver.assert(&tag_int.lt(&Int::from_i64(ctx, n)));
                    }
                }
            }

            // ========================================================
            // Z3 網羅性チェック (Exhaustiveness Check)
            // ========================================================
            // 各アームの条件 P_i を構築し、¬(P_1 ∨ P_2 ∨ ... ∨ P_n) が
            // Unsat であることを証明する。Sat なら網羅性欠如エラー。
            if let Some(solver) = solver_opt {
                let mut arm_conditions: Vec<Bool> = Vec::new();
                for arm in arms {
                    let cond = pattern_to_z3_condition(
                        ctx,
                        &arm.pattern,
                        &target_z3,
                        env,
                        vc,
                        solver_opt,
                    )?;
                    // ガード条件がある場合は AND で結合
                    let full_cond = if let Some(guard) = &arm.guard {
                        let guard_z3 = expr_to_z3(vc, guard, env, None)?
                            .as_bool()
                            .ok_or(MumeiError::type_error("Guard must be boolean"))?;
                        Bool::and(ctx, &[&cond, &guard_z3])
                    } else {
                        cond
                    };
                    arm_conditions.push(full_cond);
                }

                // 網羅性: ¬(P_1 ∨ ... ∨ P_n) が Unsat か？
                let arm_refs: Vec<&Bool> = arm_conditions.iter().collect();
                let coverage = Bool::or(ctx, &arm_refs);
                solver.push();
                solver.assert(&coverage.not());
                let exhaustive = solver.check() == SatResult::Unsat;
                solver.pop(1);

                if !exhaustive {
                    // 反例（Counter-example）の取得と表示
                    // solver はまだ Sat 状態なので、再度チェックして model を取得
                    solver.push();
                    solver.assert(&coverage.not());
                    if solver.check() == SatResult::Sat {
                        let counterexample = if let Some(model) = solver.get_model() {
                            // ターゲット変数の具体的な値を取得
                            format_counterexample(&model, &target_z3, arms, vc.module_env)
                        } else {
                            "unknown value".to_string()
                        };
                        solver.pop(1);
                        let ce_value = serde_json::json!({
                            "target": counterexample,
                        });
                        return Err(MumeiError::verification(
                            format!(
                                "Match is not exhaustive: the following value is not covered by any arm:\n  Counter-example: {}",
                                counterexample
                            )
                        ).with_counterexample(Some(ce_value)));
                    }
                    solver.pop(1);
                    return Err(MumeiError::verification(
                        "Match is not exhaustive: there exist values not covered by any arm.",
                    ));
                }
            }

            // ========================================================
            // Match 式の値の構築（if-then-else チェーンとして Z3 式を構築）
            // ========================================================
            // A. デフォルトアーム最適化:
            //    _ アームの body 評価時に、先行アームの否定を事前条件として
            //    env/solver に追加し、デフォルトアーム内の検証精度を向上させる。
            let mut accumulated_negations: Vec<Bool> = Vec::new();
            let mut result: Option<Dynamic> = None;

            for arm in arms.iter().rev() {
                let mut arm_env = env.clone();

                // B. ネストパターンの再帰解体:
                //    pattern_bind_variables が再帰的にパターンを分解し、
                //    バインド変数を arm_env に登録する。
                pattern_bind_variables(ctx, &arm.pattern, &target_z3, &mut arm_env, vc.module_env);

                let arm_cond = pattern_to_z3_condition(
                    ctx,
                    &arm.pattern,
                    &target_z3,
                    &mut arm_env,
                    vc,
                    solver_opt,
                )?;
                let full_cond = if let Some(guard) = &arm.guard {
                    let guard_z3 = expr_to_z3(vc, guard, &mut arm_env, None)?
                        .as_bool()
                        .ok_or(MumeiError::type_error("Guard must be boolean"))?;
                    Bool::and(ctx, &[&arm_cond, &guard_z3])
                } else {
                    arm_cond
                };

                // A. デフォルトアーム最適化: Wildcard/Variable パターンの場合、
                //    先行アームの否定条件を solver に追加して body を検証
                if let Some(solver) = solver_opt {
                    if matches!(arm.pattern, Pattern::Wildcard | Pattern::Variable(_))
                        && !accumulated_negations.is_empty()
                    {
                        let neg_refs: Vec<&Bool> = accumulated_negations.iter().collect();
                        let prior_negation = Bool::and(ctx, &neg_refs);
                        solver.push();
                        solver.assert(&prior_negation);
                        let body_val = stmt_to_z3(vc, &arm.body, &mut arm_env, solver_opt)?;
                        solver.pop(1);
                        result = Some(match result {
                            Some(else_val) => full_cond.ite(&body_val, &else_val),
                            None => body_val,
                        });
                        accumulated_negations.push(full_cond.not());
                        continue;
                    }
                }

                let body_val = stmt_to_z3(vc, &arm.body, &mut arm_env, solver_opt)?;
                result = Some(match result {
                    Some(else_val) => full_cond.ite(&body_val, &else_val),
                    None => body_val,
                });
                accumulated_negations.push(full_cond.not());
            }

            result.ok_or_else(|| MumeiError::verification("Match expression has no arms"))
        }

        // =================================================================
        // 非同期処理 + リソース管理の Z3 検証
        // =================================================================
        Expr::Perform {
            effect,
            operation,
            args: perform_args,
        } => {
            // Effect system: record effect usage and verify against allowed set
            // Record that this effect was used
            let used_name = format!("__effect_used_{}", effect);
            let used_bool = Bool::from_bool(ctx, true);
            env.insert(used_name.clone(), used_bool.into());

            // Wire EffectCtx: track the performed effect
            if let Some(ectx_cell) = vc.effect_ctx {
                let mut ectx = ectx_cell.borrow_mut();
                // Record usage; violations are warnings here (Z3 check below is authoritative)
                let _ = ectx.perform_effect(effect);
            }

            // Check SecurityPolicy parameter constraints if available.
            // Currently only checks constant arguments (Number-based path IDs);
            // symbolic arguments are validated via Z3 constraints in verify_effect_params.
            if let Some(ref policy) = vc.module_env.security_policy {
                if !policy.is_effect_allowed(effect) {
                    return Err(MumeiError::verification(format!(
                        "Security policy violation: effect '{}' is not permitted by the \
                         current security policy",
                        effect
                    )));
                }
            }

            // Check against allowed effects via Z3 environment
            let allowed_name = format!("__effect_allowed_{}", effect);
            if env.get(&allowed_name).is_none() {
                // Effect not in allowed set — immediate violation
                return Err(MumeiError::verification(format!(
                    "Effect violation: 'perform {}.{}' requires [{}] effect, \
                         but it is not declared in the current atom's effects set.",
                    effect, operation, effect
                ))
                .with_help(format!(
                    "Fix option 1: Add '{}' to the effects declaration: effects: [{}];\n\
                         Fix option 2: Remove the call to 'perform {}.{}'.",
                    effect, effect, effect, operation
                )));
            }

            // If solver is available, assert the Z3 containment constraint
            if let Some(solver) = solver_opt {
                let used_z3 = Bool::from_bool(ctx, true);
                let allowed_z3 = Bool::from_bool(ctx, true); // already proven allowed
                                                             // Assert: used → allowed (trivially true when allowed)
                solver.assert(&used_z3.implies(&allowed_z3));
            }

            // Process arguments and collect Z3 values
            let mut arg_z3_values: Vec<Dynamic> = Vec::new();
            for arg in perform_args {
                let val = expr_to_z3(vc, arg, env, solver_opt)?;
                arg_z3_values.push(val);
            }

            // Z3 String Sort: verify symbolic parameter constraints
            // Look up the EffectDef to get constraint and param definitions
            let effect_def = vc
                .module_env
                .effect_defs
                .get(effect.as_str())
                .or_else(|| vc.module_env.effects.get(effect.as_str()))
                .cloned();
            if let Some(def) = effect_def {
                if let Some(ref constraint) = def.constraint {
                    // For each argument, check if it's a symbolic (non-constant) value
                    // that needs Z3 String constraint verification.
                    // NOTE: Currently def.constraint is a single string (e.g., "starts_with(path, \"/tmp/\")")
                    // that is applied to ALL non-constant args. This is correct for single-parameter
                    // effects (the only kind currently supported by the parser), but would incorrectly
                    // apply a path-specific constraint to unrelated parameters if multi-parameter
                    // effects like FileOp(path: Str, mode: Str) are added. When that happens,
                    // extract the parameter name from the constraint string, find its index in
                    // def.params, and only apply the constraint when `i` matches that index.
                    for (i, arg) in perform_args.iter().enumerate() {
                        // Number/Float literals are constants already checked
                        // by verify_effect_params (Phase 1g). Skip Z3 String here.
                        // Variables and other expressions need symbolic verification.
                        let is_constant = matches!(arg, Expr::Number(_) | Expr::Float(_));
                        if is_constant {
                            // Constant args are already checked by check_constant_constraint
                            // in verify_effect_params (Phase 1g). Skip Z3 String here.
                            continue;
                        }
                        // Symbolic argument: verify constraint using Z3 String Sort
                        if let Some(solver) = solver_opt {
                            let param_name =
                                def.params.get(i).map(|p| p.name.as_str()).unwrap_or("arg");
                            // Use a unique counter to distinguish different perform call sites.
                            // Without this, Z3 reuses the same constant for the same name,
                            // incorrectly merging constraints from distinct call sites.
                            static EFFECT_STR_COUNTER: std::sync::atomic::AtomicUsize =
                                std::sync::atomic::AtomicUsize::new(0);
                            let unique_id = EFFECT_STR_COUNTER
                                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            let z3_str_name = format!(
                                "__effect_str_{}_{}_{}_{}",
                                effect, operation, param_name, unique_id
                            );

                            // Plan 10: Use arg_z3_values[i] directly when it has Z3 String Sort.
                            // This enables dynamically constructed strings (e.g., "/tmp/" + var + "/file.txt")
                            // to be directly checked against constraints like starts_with(path, "/tmp/").
                            let param_z3_str = if i < arg_z3_values.len() {
                                if let Some(existing_str) = arg_z3_values[i].as_string() {
                                    // The argument was already evaluated to a Z3 String by expr_to_z3.
                                    // Use it directly — this preserves concat/variable relationships.
                                    existing_str
                                } else {
                                    // Non-string Z3 value: create a fresh Z3 String variable
                                    // and try to connect it to any known string variable in env.
                                    let fresh = Z3String::new_const(ctx, z3_str_name.as_str());
                                    if let Expr::Variable(var_name) = arg {
                                        let str_env_key = format!("__str_{}", var_name);
                                        let found_str = env
                                            .get(&str_env_key)
                                            .and_then(|v| v.as_string())
                                            .or_else(|| {
                                                env.get(var_name).and_then(|v| v.as_string())
                                            });
                                        if let Some(existing_s) = found_str {
                                            solver.assert(&fresh._eq(&existing_s));
                                        }
                                    }
                                    fresh
                                }
                            } else {
                                // Fallback: create a fresh Z3 String variable
                                let fresh = Z3String::new_const(ctx, z3_str_name.as_str());
                                if let Expr::Variable(var_name) = arg {
                                    let str_env_key = format!("__str_{}", var_name);
                                    let found_str = env
                                        .get(&str_env_key)
                                        .and_then(|v| v.as_string())
                                        .or_else(|| env.get(var_name).and_then(|v| v.as_string()));
                                    if let Some(existing_s) = found_str {
                                        solver.assert(&fresh._eq(&existing_s));
                                    }
                                }
                                fresh
                            };

                            // Parse the constraint and assert it
                            if let Some(constraint_bool) =
                                parse_constraint_to_z3_string(ctx, constraint, &param_z3_str)
                            {
                                // Set has_string_constraints flag for sort-aware timeout
                                if let Some(flag) = vc.has_string_constraints {
                                    flag.set(true);
                                }
                                // Check constraint budget
                                if let Some(count) = vc.constraint_count {
                                    let current = count.get();
                                    if current >= vc.constraint_budget {
                                        return Err(MumeiError::verification(format!(
                                            "Constraint budget exceeded for effect '{}' \
                                             string constraint: {} constraints (limit: {})",
                                            effect, current, vc.constraint_budget
                                        )));
                                    }
                                    count.set(current + 1);
                                }
                                let track_label = format!(
                                    "track_effect_str_{}_{}_{}_{}",
                                    effect, operation, param_name, unique_id
                                );
                                let track_bool = Bool::new_const(ctx, track_label.as_str());
                                solver.assert_and_track(&constraint_bool, &track_bool);
                                profile_solver_assertion(vc, &track_label, None);
                            }

                            // Store the Z3 String variable in env for downstream use
                            env.insert(z3_str_name, param_z3_str.into());
                        }
                    }
                }
            }

            // Return a symbolic result value.
            // Use Z3 String Sort if the effect has Str-typed parameters,
            // since the operation may return a string (e.g., http_request_path).
            // Otherwise default to Int (status codes, handles, etc.).
            //
            // NOTE: This is a heuristic. Ideally, EffectDef would carry a
            // per-operation return type (e.g., `read -> Str`, `write -> i64`),
            // but the current parser does not record return types for effect
            // operations. Using "any param is Str → result is Str" is a
            // conservative approximation that prevents Z3 Sort mismatches when
            // the perform result is later used in string operations. When the
            // parser gains per-operation return type info, this heuristic
            // should be replaced with a direct lookup.
            //
            // IMPACT: This changes the return type for pre-existing effects
            // with Str params (e.g., HttpGet(url: Str), HttpPost(url: Str))
            // from Int to Z3String. No current code is broken because all
            // atoms discard the perform result (e.g., `perform X.op(url); 1`).
            // Future code that uses the perform result in an integer context
            // (e.g., `let x = perform HttpGet.request(url); x + 1`) would get
            // a Z3 Sort mismatch error.
            let result_name = format!("__perform_{}_{}", effect, operation);
            let has_str_params = vc
                .module_env
                .effect_defs
                .get(effect.as_str())
                .or_else(|| vc.module_env.effects.get(effect.as_str()))
                .map(|def| def.params.iter().any(|p| p.type_name == "Str"))
                .unwrap_or(false);
            if has_str_params {
                Ok(Z3String::new_const(ctx, result_name.as_str()).into())
            } else {
                Ok(Int::new_const(ctx, result_name.as_str()).into())
            }
        }

        Expr::Async { body } => {
            // async ブロック: body を非同期コンテキストとして検証する。
            // Z3 上では通常の式として扱い、結果をシンボリック値として返す。
            // await ポイントでの所有権検証は Await 式で行う。
            stmt_to_z3(vc, body, env, solver_opt)
        }
        Expr::Await { expr } => {
            // =============================================================
            // await 跨ぎの安全性検証 (Await Safety Verification)
            // =============================================================
            //
            // await ポイントはコルーチンの中断点であり、以下の安全性を検証する:
            //
            // 1. リソース保持検証 (Resource Held Across Await):
            //    acquire ブロック内で await を呼ぶと、リソースを保持したまま
            //    スレッドが中断される。これはデッドロックの典型パターン。
            //    env 内の __resource_held_* が true のリソースを検出してエラーにする。
            //
            // 2. 所有権一貫性検証 (Ownership Consistency):
            //    await 前に消費済み（__alive_ = false）の変数が、await 後に
            //    アクセスされないことを確認する。Z3 で __alive_ フラグをチェック。

            // --- 1. リソース保持検証 ---
            // env 内の __resource_held_* キーを走査し、Z3 で true かどうかを確認する。
            // acquire ブロック内で await を呼ぶパターンを検出する。
            if let Some(solver) = solver_opt {
                let held_resources: Vec<String> = env
                    .keys()
                    .filter(|k| k.starts_with("__resource_held_"))
                    .cloned()
                    .collect();

                for held_key in &held_resources {
                    let resource_name = held_key
                        .strip_prefix("__resource_held_")
                        .unwrap_or(held_key);
                    if let Some(held_val) = env.get(held_key) {
                        // Z3 で held_val == true が証明可能かチェック
                        // （acquire ブロック内なら held = true が assert されている）
                        if let Some(held_bool) = held_val.as_bool() {
                            solver.push();
                            // held が true であることを仮定し、矛盾がなければ保持中
                            solver.assert(&held_bool);
                            if solver.check() != SatResult::Unsat {
                                solver.pop(1);
                                return Err(MumeiError::verification(
                                    format!(
                                        "Unsafe await: resource '{}' is held across an await point. \
                                         This can cause deadlock because the resource lock is not released \
                                         during suspension. Move the await outside the acquire block, or \
                                         release the resource before awaiting.\n  \
                                         Hint: acquire {} {{ ... }}; let val = await expr; // OK\n  \
                                         Bad:  acquire {} {{ let val = await expr; ... }}  // deadlock risk",
                                        resource_name, resource_name, resource_name
                                    )
                                ));
                            }
                            solver.pop(1);
                        }
                    }
                }
            }

            // --- 2. 所有権一貫性検証 ---
            // await 前に消費済みの変数を検出し、Z3 で __alive_ = false を確認する。
            // 消費済み変数が await 後にアクセスされる可能性がある場合、警告する。
            if let Some(solver) = solver_opt {
                let consumed_vars: Vec<String> = env
                    .keys()
                    .filter(|k| k.starts_with("__alive_"))
                    .cloned()
                    .collect();

                for alive_key in &consumed_vars {
                    let var_name = alive_key.strip_prefix("__alive_").unwrap_or(alive_key);
                    if let Some(alive_val) = env.get(alive_key) {
                        if let Some(alive_bool) = alive_val.as_bool() {
                            // __alive_ が false（消費済み）であることを Z3 で確認
                            solver.push();
                            solver.assert(&alive_bool.not()); // alive = false を仮定
                            if solver.check() == SatResult::Sat {
                                // 消費済み変数が存在する → await 後のアクセスは use-after-free
                                // await ポイントでの状態をマーク（後続の検証で参照）
                                let await_consumed_key = format!("__await_consumed_{}", var_name);
                                let marker = Bool::from_bool(vc.ctx, true);
                                env.insert(await_consumed_key, marker.into());
                            }
                            solver.pop(1);
                        }
                    }
                }
            }

            // 内側の式を評価してシンボリック結果を返す
            let inner_result = expr_to_z3(vc, expr, env, solver_opt)?;
            Ok(inner_result)
        }

        // =================================================================
        // Higher-Order Functions (Phase A): atom_ref + call
        // =================================================================
        Expr::AtomRef { name } => {
            // atom_ref(some_atom): ModuleEnv から atom 定義を取得し、
            // シンボリック値を生成する。呼び出し先の atom の契約情報は
            // CallRef 時に展開される。
            if vc.module_env.get_atom(name).is_none() {
                return Err(MumeiError::verification(format!(
                    "atom_ref: unknown atom '{}'",
                    name
                )));
            }
            // atom_ref はシンボリックな関数参照として Int 値を生成
            // （実行時は関数ポインタ、Z3 上はシンボリック識別子）
            let ref_name = format!("__atom_ref_{}", name);
            let ref_val = Int::new_const(ctx, ref_name.as_str());
            env.insert(ref_name, ref_val.clone().into());
            Ok(ref_val.into())
        }
        Expr::CallRef { callee, args } => {
            // call(callee_expr, arg1, arg2, ...):
            // callee が AtomRef の場合、参照先の atom の契約を展開して検証する。
            // - requires を呼び出し元のコンテキストで検証
            // - ensures を事実として solver に追加

            // callee を評価
            let _callee_val = expr_to_z3(vc, callee, env, solver_opt)?;

            // callee が AtomRef の場合、参照先の atom 名を取得
            let atom_name = if let Expr::AtomRef { name } = callee.as_ref() {
                Some(name.clone())
            } else if let Expr::Variable(var_name) = callee.as_ref() {
                // 変数が atom_ref として束縛されている場合
                // env から __atom_ref_ プレフィックスで探す
                if env.contains_key(&format!("__atom_ref_{}", var_name)) {
                    Some(var_name.clone())
                } else {
                    None
                }
            } else {
                None
            };

            if let Some(ref callee_name) = atom_name {
                if let Some(callee_atom) = vc.module_env.get_atom(callee_name).cloned() {
                    // 引数を Z3 で評価
                    let mut arg_vals = Vec::new();
                    for arg in args {
                        arg_vals.push(expr_to_z3(vc, arg, env, solver_opt)?);
                    }

                    // 呼び出し先のパラメータ名に引数をマッピング
                    let mut call_env = env.clone();
                    for (i, param) in callee_atom.params.iter().enumerate() {
                        if let Some(arg_val) = arg_vals.get(i) {
                            call_env.insert(param.name.clone(), arg_val.clone());
                        }
                    }

                    // requires を呼び出し元のコンテキストで検証
                    if callee_atom.requires.trim() != "true" {
                        let req_ast = parse_expression(&callee_atom.requires);
                        let req_z3 = expr_to_z3(vc, &req_ast, &mut call_env, None)?;
                        if let Some(req_bool) = req_z3.as_bool() {
                            if let Some(solver) = solver_opt {
                                solver.push();
                                solver.assert(&req_bool.not());
                                if solver.check() == SatResult::Sat {
                                    solver.pop(1);
                                    return Err(MumeiError::verification(format!(
                                        "call(atom_ref({})): precondition '{}' may not hold at call site",
                                        callee_name, callee_atom.requires
                                    ))
                                    .with_help(
                                        "呼び出し元で事前条件を満たしていません。引数の制約を確認してください",
                                    ));
                                }
                                solver.pop(1);
                            }
                        }
                    }

                    // ensures を事実として solver に追加（Equality Ensures Propagation）
                    static CALL_REF_COUNTER: std::sync::atomic::AtomicUsize =
                        std::sync::atomic::AtomicUsize::new(0);
                    let call_id =
                        CALL_REF_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let result_name = format!("call_ref_{}_{}", callee_name, call_id);
                    let result_z3: Dynamic = Int::new_const(ctx, result_name.as_str()).into();

                    if callee_atom.ensures.trim() != "true" {
                        call_env.insert("result".to_string(), result_z3.clone());
                        let ens_ast = parse_expression(&callee_atom.ensures);
                        let ens_z3 = expr_to_z3(vc, &ens_ast, &mut call_env, None)?;
                        if let Some(ens_bool) = ens_z3.as_bool() {
                            if let Some(solver) = solver_opt {
                                solver.assert(&ens_bool);
                            }
                        }

                        // Equality ensures の特別処理
                        if let Expr::BinaryOp(left, Op::Eq, right) = &ens_ast {
                            if let Expr::Variable(ref var_name) = left.as_ref() {
                                if var_name == "result" {
                                    if let Ok(rhs_val) = expr_to_z3(vc, right, &mut call_env, None)
                                    {
                                        if let Some(solver) = solver_opt {
                                            if let (Some(res_int), Some(rhs_int)) =
                                                (result_z3.as_int(), rhs_val.as_int())
                                            {
                                                solver.assert(&res_int._eq(&rhs_int));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    return Ok(result_z3);
                }
            }

            // =============================================================
            // Phase B: call_with_contract — パラメトリック関数型の契約展開
            // =============================================================
            // callee が Variable で、current_atom のパラメータに contract(f) が
            // 宣言されている場合、その契約を使って結果を制約する。
            // これにより trusted マーカーなしで高階関数を検証できる。

            let mut arg_vals = Vec::new();
            for arg in args {
                arg_vals.push(expr_to_z3(vc, arg, env, solver_opt)?);
            }

            static DYNAMIC_CALL_COUNTER: std::sync::atomic::AtomicUsize =
                std::sync::atomic::AtomicUsize::new(0);
            let id = DYNAMIC_CALL_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let result: Dynamic = Int::new_const(ctx, format!("call_ref_dynamic_{}", id)).into();

            // callee が Variable の場合、current_atom のパラメータから contract 情報を取得
            if let Expr::Variable(callee_var_name) = callee.as_ref() {
                if let Some(current_atom) = vc.current_atom {
                    if let Some(param) = current_atom
                        .params
                        .iter()
                        .find(|p| p.name == *callee_var_name)
                    {
                        // contract(f): ensures: <expr> が宣言されている場合
                        if let Some(ref fn_ensures) = param.fn_contract_ensures {
                            let mut contract_env = env.clone();

                            // atom_ref のパラメータ型情報から引数名を生成
                            // atom_ref(i64) -> i64 の場合、arg0 として引数をマッピング
                            // atom_ref(i64, i64) -> i64 の場合、arg0, arg1 として引数をマッピング
                            for (i, arg_val) in arg_vals.iter().enumerate() {
                                contract_env.insert(format!("arg{}", i), arg_val.clone());
                            }

                            // 最初の引数を "x" としてもマッピング（よくある1引数パターン用）
                            if let Some(first_arg) = arg_vals.first() {
                                contract_env.insert("x".to_string(), first_arg.clone());
                            }
                            // 2引数の場合 "y" もマッピング
                            if let Some(second_arg) = arg_vals.get(1) {
                                contract_env.insert("y".to_string(), second_arg.clone());
                            }

                            // result をマッピング
                            contract_env.insert("result".to_string(), result.clone());

                            // requires の検証（宣言されている場合）
                            if let Some(ref fn_requires) = param.fn_contract_requires {
                                if fn_requires.trim() != "true" {
                                    let req_ast = parse_expression(fn_requires);
                                    let req_z3 = expr_to_z3(vc, &req_ast, &mut contract_env, None)
                                        .map_err(|e| MumeiError::verification(format!(
                                            "call_with_contract({}): failed to evaluate requires '{}': {}",
                                            callee_var_name, fn_requires, e
                                        )))?;
                                    if let Some(req_bool) = req_z3.as_bool() {
                                        if let Some(solver) = solver_opt {
                                            solver.push();
                                            solver.assert(&req_bool.not());
                                            if solver.check() == SatResult::Sat {
                                                solver.pop(1);
                                                return Err(MumeiError::verification(format!(
                                                    "call_with_contract({}): precondition '{}' may not hold at call site",
                                                    callee_var_name, fn_requires
                                                ))
                                                .with_help(
                                                    "関数パラメータの事前条件を満たしていません。引数の制約を確認してください",
                                                ));
                                            }
                                            solver.pop(1);
                                        }
                                    }
                                }
                            }

                            // ensures を事実として solver に追加
                            if fn_ensures.trim() != "true" {
                                let ens_ast = parse_expression(fn_ensures);
                                let ens_z3 = expr_to_z3(vc, &ens_ast, &mut contract_env, None)
                                    .map_err(|e| MumeiError::verification(format!(
                                        "call_with_contract({}): failed to evaluate ensures '{}': {}",
                                        callee_var_name, fn_ensures, e
                                    )))?;
                                if let Some(ens_bool) = ens_z3.as_bool() {
                                    if let Some(solver) = solver_opt {
                                        solver.assert(&ens_bool);
                                        profile_solver_assertion(
                                            vc,
                                            &format!(
                                                "call_with_contract_{}_ensures",
                                                callee_var_name
                                            ),
                                            None,
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }

            Ok(result)
        }

        Expr::FieldAccess(inner_expr, field_name) => {
            // ネスト構造体のフィールドアクセスを再帰的に解決する。
            //
            // 1段階: v.x → env["__struct_v_x"] or env["v_x"]
            // 2段階: v.point.x → まず v.point を解決し、その結果から .x を解決
            //
            // 解決戦略:
            // A. 内側の式が Variable の場合: 直接 env から探す
            // B. 内側の式が FieldAccess の場合: 再帰的に解決し、
            //    結果のパスを使って env から探す
            // C. どちらでもない場合: 式を評価してシンボリック変数を生成

            // フラットなパス文字列を構築するヘルパー
            // v.point.x → "v_point_x" のようなパスを生成
            fn build_field_path(expr: &Expr) -> Option<Vec<String>> {
                match expr {
                    Expr::Variable(name) => Some(vec![name.clone()]),
                    Expr::FieldAccess(inner, field) => {
                        let mut path = build_field_path(inner)?;
                        path.push(field.clone());
                        Some(path)
                    }
                    _ => None,
                }
            }

            // 完全なフィールドパスを構築（例: ["v", "point", "x"]）
            let full_path = {
                let mut path = build_field_path(inner_expr).unwrap_or_default();
                path.push(field_name.clone());
                path
            };

            if full_path.len() >= 2 {
                // パスの各プレフィックスで env を探索
                // 例: ["v", "point", "x"] → "v_point_x", "__struct_v_point_x"
                let underscore_path = full_path.join("_");
                let struct_path = format!("__struct_{}", underscore_path);

                // 直接パスで見つかればそれを返す
                if let Some(val) = env.get(&struct_path) {
                    return Ok(val.clone());
                }
                if let Some(val) = env.get(&underscore_path) {
                    return Ok(val.clone());
                }

                // 1段階ずつ解決を試みる
                // 例: v.point → env["__struct_v_point"] or env["v_point"]
                //     その結果が構造体型なら、.x のフィールドをさらに解決
                if full_path.len() == 2 {
                    // 単純な1段階アクセス: v.x
                    let var_name = &full_path[0];
                    let candidates = [
                        format!("__struct_{}_{}", var_name, field_name),
                        format!("{}_{}", var_name, field_name),
                    ];
                    for candidate in &candidates {
                        if let Some(val) = env.get(candidate) {
                            return Ok(val.clone());
                        }
                    }
                }

                // ネスト構造体の再帰解決:
                // 内側の式を先に Z3 で評価し、結果を env に登録してからフィールドを解決
                let _base_val = expr_to_z3(vc, inner_expr, env, solver_opt)?;

                // 内側の式の型を推定し、構造体定義からフィールドの型を取得
                // フィールドの精緻型制約も再帰的に適用する
                let nested_sym_name = format!(
                    "{}_{}",
                    underscore_path
                        .rsplit_once('_')
                        .map(|(prefix, _)| prefix)
                        .unwrap_or(&underscore_path),
                    field_name
                );
                let sym = if let Some(val) = env.get(&nested_sym_name) {
                    return Ok(val.clone());
                } else {
                    let s = Int::new_const(ctx, full_path.join("_").as_str());
                    env.insert(full_path.join("_"), s.clone().into());
                    s
                };
                Ok(sym.into())
            } else {
                // パスが構築できない場合: 式を評価してシンボリック変数を生成
                let _base = expr_to_z3(vc, inner_expr, env, solver_opt)?;
                let sym = Int::new_const(ctx, format!("field_{}", field_name));
                Ok(sym.into())
            }
        }
        // Lambda 式: Z3 uninterpreted function として表現する
        // 将来のフェーズでキャプチャ変数の環境アサーションと
        // 高階関数コントラクトの検証を追加する
        Expr::Lambda { params, body, .. } => {
            // Create a fresh symbolic value for the lambda
            // Lambda bodies will be verified when called via higher-order function contracts
            // Use a unique counter to avoid Z3 constant name collisions when multiple lambdas
            // with the same arity appear in the same atom body (e.g., `let f = |x| x+1; let g = |x| x-1;`).
            static LAMBDA_COUNTER: std::sync::atomic::AtomicUsize =
                std::sync::atomic::AtomicUsize::new(0);
            let lambda_id = LAMBDA_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let lambda_name = format!("__lambda_{}_{}", params.len(), lambda_id);
            let lambda_sym = Int::new_const(ctx, lambda_name.as_str());

            // Register parameter names in a sub-environment for body verification
            let mut lambda_env = env.clone();
            for p in params {
                let p_sym = Int::new_const(ctx, p.name.as_str());
                lambda_env.insert(p.name.clone(), p_sym.into());
            }

            // Verify the lambda body in the sub-environment
            let _body_val = stmt_to_z3(vc, body, &mut lambda_env, solver_opt)?;

            Ok(lambda_sym.into())
        }
        // Plan 8: Channel send — evaluate channel and value, return unit
        Expr::ChanSend { channel, value } => {
            let _ch = expr_to_z3(vc, channel, env, solver_opt)?;
            let _val = expr_to_z3(vc, value, env, solver_opt)?;
            Ok(Int::from_i64(ctx, 0).into())
        }
        // Plan 8: Channel recv — evaluate channel, return symbolic int
        Expr::ChanRecv { channel } => {
            let _ch = expr_to_z3(vc, channel, env, solver_opt)?;
            static RECV_COUNTER: std::sync::atomic::AtomicUsize =
                std::sync::atomic::AtomicUsize::new(0);
            let recv_id = RECV_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let recv_sym = Int::new_const(ctx, format!("__chan_recv_{}", recv_id).as_str());
            Ok(recv_sym.into())
        }
    }
}
