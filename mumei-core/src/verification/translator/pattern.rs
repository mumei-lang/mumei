#![allow(unused_imports)]
use super::super::support::*;
use super::super::*;
use super::*;
use crate::lowering::{lower, LoweredType};
use serde_json::json;

pub(crate) fn pattern_to_z3_condition<'a>(
    ctx: &'a Context,
    pattern: &Pattern,
    target: &Dynamic<'a>,
    env: &mut Env<'a>,
    vc: &VCtx<'a>,
    solver_opt: Option<&Solver<'a>>,
    decl_hint: Option<&str>,
) -> MumeiResult<Bool<'a>> {
    match pattern {
        Pattern::Wildcard | Pattern::Variable(_) => Ok(Bool::from_bool(ctx, true)),
        Pattern::Literal(n) => {
            // Compare in the target's own sort: under `--bitvec-i64` (or any
            // contract that forces BV encoding, e.g. bitwise ops) `as_int()`
            // fails and an unconstrained `__match_target` would silently make
            // every literal arm unreachable.
            if let Some(target_bv) = target.as_bv() {
                let lit = BV::from_i64(ctx, *n, target_bv.get_size());
                return Ok(target_bv._eq(&lit));
            }
            let target_int = target
                .as_int()
                .unwrap_or(Int::new_const(ctx, "__match_target"));
            let lit = Int::from_i64(ctx, *n);
            Ok(target_int._eq(&lit))
        }
        Pattern::Variant {
            variant_name,
            fields,
        } => {
            // Qualified `E::V` names resolve by the leaf after the owner
            // check inside `resolve_variant_owner`.
            let variant_leaf = datatype::variant_leaf(variant_name);
            // P10-C: a `Datatype`-sorted target matches by the variant's
            // `is-<V>` tester, and payload fields project through the real
            // selectors — so a `Str` field binds as a Z3 `String` and a field
            // pattern like `"x"` can actually constrain it.
            if target.as_datatype().is_some() {
                if let Some((sort, is_v, variant_idx)) =
                    datatype::variant_tester_condition(vc, target, variant_name)
                {
                    let mut field_conditions: Vec<Bool> = vec![is_v];
                    // arity comes from the matched sort's accessors — the sort
                    // already disambiguates same-named variants across enums.
                    for (i, field_pattern) in fields.iter().enumerate() {
                        let field_sym: Dynamic =
                            datatype::variant_selector_apply(&sort, variant_idx, i, target)
                                .unwrap_or_else(|| {
                                    Int::new_const(
                                        ctx,
                                        format!("__proj_{}_{}", variant_leaf, i).as_str(),
                                    )
                                    .into()
                                });
                        env.insert(format!("__proj_{}_{}", variant_leaf, i), field_sym.clone());
                        let field_cond = pattern_to_z3_condition(
                            ctx,
                            field_pattern,
                            &field_sym,
                            env,
                            vc,
                            solver_opt,
                            None,
                        )?;
                        field_conditions.push(field_cond);
                    }
                    let cond_refs: Vec<&Bool> = field_conditions.iter().collect();
                    return Ok(Bool::and(ctx, &cond_refs));
                }
            }
            if let Some(enum_def) =
                datatype::resolve_variant_owner(vc, target, variant_name, decl_hint)?
            {
                let variant_idx = enum_def
                    .variants
                    .iter()
                    .position(|v| v.name == variant_leaf)
                    .unwrap_or(0) as i64;

                let tag = target
                    .as_int()
                    .unwrap_or(Int::new_const(ctx, "__match_tag"));
                let tag_match = tag._eq(&Int::from_i64(ctx, variant_idx));

                let variant_def = &enum_def.variants[variant_idx as usize];
                let mut field_conditions: Vec<Bool> = vec![tag_match];

                for (i, field_pattern) in fields.iter().enumerate() {
                    // Projector シンボル: __proj_{Enum}_{Variant}_{i}
                    // enum 名を含めることで variant 名の enum 間衝突
                    // （prelude List vs user IntList の Cons 等）でも一意になり、
                    // `match t` した束縛 tail も名前から宣言型を復元できる。
                    let proj_name = format!("__proj_{}_{}_{}", enum_def.name, variant_leaf, i);
                    let field_sym: Dynamic = if i < variant_def.fields.len() {
                        let field_type = &variant_def.fields[i];
                        // 再帰的 ADT: フィールド型が自身の Enum なら tag として Int を使用
                        let base = if *field_type == enum_def.name {
                            "i64".to_string() // 再帰フィールドは tag 値
                        } else {
                            vc.module_env.resolve_base_type(field_type)
                        };
                        match base.as_str() {
                            "f64" => Float::new_const(ctx, proj_name.as_str(), 11, 53).into(),
                            _ => Int::new_const(ctx, proj_name.as_str()).into(),
                        }
                    } else {
                        Int::new_const(ctx, proj_name.as_str()).into()
                    };

                    // env にも projector を登録（body 内で参照可能にする）
                    env.insert(proj_name.clone(), field_sym.clone());

                    // 再帰フィールドの場合: ドメイン制約を追加
                    if i < variant_def.fields.len() && variant_def.fields[i] == enum_def.name {
                        if let Some(solver) = solver_opt {
                            if let Some(field_int) = field_sym.as_int() {
                                let n = enum_def.variants.len() as i64;
                                solver.assert(&field_int.ge(&Int::from_i64(ctx, 0)));
                                solver.assert(&field_int.lt(&Int::from_i64(ctx, n)));
                            }
                        }
                    }

                    // フィールドの宣言型を次段の hint として渡す
                    // （再帰 `Self` フィールド → 自身の enum 名）
                    let field_hint: Option<String> = if i < variant_def.fields.len() {
                        let ft = &variant_def.fields[i];
                        let resolved = if *ft == enum_def.name {
                            enum_def.name.clone()
                        } else {
                            vc.module_env.resolve_base_type(ft)
                        };
                        vc.module_env.get_enum(&resolved).map(|_| resolved)
                    } else {
                        None
                    };
                    // 再帰的にフィールドパターンの条件を生成
                    let field_cond = pattern_to_z3_condition(
                        ctx,
                        field_pattern,
                        &field_sym,
                        env,
                        vc,
                        solver_opt,
                        field_hint.as_deref(),
                    )?;
                    field_conditions.push(field_cond);
                }

                let cond_refs: Vec<&Bool> = field_conditions.iter().collect();
                Ok(Bool::and(ctx, &cond_refs))
            } else {
                let tag = target
                    .as_int()
                    .unwrap_or(Int::new_const(ctx, "__match_tag"));
                let hash = variant_leaf
                    .bytes()
                    .fold(0i64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as i64));
                Ok(tag._eq(&Int::from_i64(ctx, hash)))
            }
        }
    }
}

/// パターンから変数バインドを env に登録する（再帰的: ネストパターン対応）
///
/// Phase 1-B: projector シンボルを使ったバインド
/// - Variable(name) → target の値を name にバインド
/// - Variant の fields 内の Variable → projector シンボル `__proj_{Variant}_{i}` にバインド
/// - Variant の fields 内の Variant → 再帰的に projector を生成してバインド
pub(crate) fn pattern_bind_variables<'a>(
    ctx: &'a Context,
    pattern: &Pattern,
    target: &Dynamic<'a>,
    env: &mut Env<'a>,
    vc: &VCtx<'a>,
    decl_hint: Option<&str>,
) {
    let module_env = vc.module_env;
    match pattern {
        Pattern::Variable(name) => {
            env.insert(name.clone(), target.clone());
        }
        Pattern::Variant {
            variant_name,
            fields,
        } => {
            let variant_leaf = datatype::variant_leaf(variant_name);
            // P10-C: datatype targets bind fields through the variant's
            // selectors so the bound symbol carries the payload's real sort
            // (String/Real/Bool), not an unconstrained Int projector.
            if target.as_datatype().is_some() {
                if let Some((sort, _is_v, variant_idx)) =
                    datatype::variant_tester_condition(vc, target, variant_name)
                {
                    {
                        for (i, field_pattern) in fields.iter().enumerate() {
                            if i >= sort.variants[variant_idx].accessors.len() {
                                continue;
                            }
                            let Some(field_sym) =
                                datatype::variant_selector_apply(&sort, variant_idx, i, target)
                            else {
                                continue;
                            };
                            env.insert(format!("__proj_{}_{}", variant_leaf, i), field_sym.clone());
                            match field_pattern {
                                Pattern::Variable(fname) => {
                                    env.insert(fname.clone(), field_sym.clone());
                                }
                                Pattern::Variant { .. } => {
                                    pattern_bind_variables(
                                        ctx,
                                        field_pattern,
                                        &field_sym,
                                        env,
                                        vc,
                                        None,
                                    );
                                }
                                _ => {}
                            }
                        }
                    }
                    return;
                }
            }
            if let Ok(Some(enum_def)) =
                datatype::resolve_variant_owner(vc, target, variant_name, decl_hint)
            {
                if let Some(variant_def) = enum_def.variants.iter().find(|v| v.name == variant_leaf)
                {
                    for (i, field_pattern) in fields.iter().enumerate() {
                        let proj_name = format!("__proj_{}_{}_{}", enum_def.name, variant_leaf, i);
                        let field_sym: Dynamic = if i < variant_def.fields.len() {
                            let field_type = &variant_def.fields[i];
                            let base = if *field_type == enum_def.name {
                                "i64".to_string()
                            } else {
                                module_env.resolve_base_type(field_type)
                            };
                            match base.as_str() {
                                "f64" => Float::new_const(ctx, proj_name.as_str(), 11, 53).into(),
                                _ => Int::new_const(ctx, proj_name.as_str()).into(),
                            }
                        } else {
                            Int::new_const(ctx, proj_name.as_str()).into()
                        };
                        env.insert(proj_name.clone(), field_sym.clone());

                        // Variable パターン: projector を変数名にもバインド
                        match field_pattern {
                            Pattern::Variable(fname) => {
                                env.insert(fname.clone(), field_sym.clone());
                            }
                            Pattern::Variant { .. } => {
                                // ネストした Variant: フィールドの宣言型
                                // （再帰 `Self` → 自身の enum 名）を hint に
                                let field_hint: Option<String> =
                                    variant_def.fields.get(i).and_then(|ft| {
                                        let resolved = if *ft == enum_def.name {
                                            enum_def.name.clone()
                                        } else {
                                            module_env.resolve_base_type(ft)
                                        };
                                        module_env.get_enum(&resolved).map(|_| resolved)
                                    });
                                pattern_bind_variables(
                                    ctx,
                                    field_pattern,
                                    &field_sym,
                                    env,
                                    vc,
                                    field_hint.as_deref(),
                                );
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        Pattern::Wildcard | Pattern::Literal(_) => {}
    }
}

/// アームの Variant パターンから対応する EnumDef を検出する。
/// 最初に解決できた Variant パターンの所属 Enum を返す。複数の Enum が
/// 同名 Variant を持つ場合は match target の宣言型で決定する
/// （`resolve_variant_owner` — HashMap の先見つけ順に依存しない）。
pub(crate) fn detect_enum_from_arms<'a>(
    arms: &[MatchArm],
    vc: &VCtx<'a>,
    target: &Dynamic<'a>,
    decl_hint: Option<&str>,
) -> Option<&'a EnumDef> {
    for arm in arms {
        if let Pattern::Variant { variant_name, .. } = &arm.pattern {
            if let Ok(Some(enum_def)) =
                datatype::resolve_variant_owner(vc, target, variant_name, decl_hint)
            {
                return Some(enum_def);
            }
        }
    }
    None
}

/// Z3 Model から反例の文字列表現を生成する。
/// Enum ドメイン制約が注入されている場合、tag 値からバリアント名+フィールド値を表示する。
pub(crate) fn format_counterexample<'a>(
    model: &z3::Model,
    target: &Dynamic<'a>,
    arms: &[MatchArm],
    vc: &VCtx<'a>,
    decl_hint: Option<&str>,
) -> String {
    // アームから Enum 定義を特定（ドメイン制約と同じロジック）
    let enum_ctx = detect_enum_from_arms(arms, vc, target, decl_hint);
    let module_env = vc.module_env;

    // ターゲット変数の具体的な値を取得
    if let Some(target_val) = model.eval(target, true) {
        let target_str = format!("{}", target_val);

        // Enum の場合: tag 値からバリアント名を逆引き
        if let Some(target_int) = target_val.as_int() {
            let tag_str = format!("{}", target_int);
            if let Ok(tag_val) = tag_str.parse::<i64>() {
                // まず arms から特定した Enum を優先的に使用
                if let Some(edef) = enum_ctx {
                    if let Some(variant) = edef.variants.get(tag_val as usize) {
                        // フィールド値も model から取得を試みる
                        let mut field_vals = Vec::new();
                        for (i, field_type) in variant.fields.iter().enumerate() {
                            let _field_sym_name =
                                format!("__proj_{}_{}_{}", edef.name, variant.name, i);
                            // model 内のシンボルを探す（存在すれば具体値を表示）
                            let field_str = format!("{}=?", field_type);
                            field_vals.push(field_str);
                        }
                        let fields_display = if field_vals.is_empty() {
                            String::new()
                        } else {
                            format!("({})", field_vals.join(", "))
                        };
                        return format!(
                            "{}::{}{} (tag={}) -- missing from match arms",
                            edef.name, variant.name, fields_display, tag_val
                        );
                    }
                }
                // フォールバック: module_env の全 Enum 定義を走査
                for (enum_name, enum_def) in module_env.enums.iter() {
                    if let Some(variant) = enum_def.variants.get(tag_val as usize) {
                        return format!(
                            "{}::{} (tag={}) -- missing from match arms",
                            enum_name, variant.name, tag_val
                        );
                    }
                }
            }
            // 整数リテラルとしてフォールバック
            return format!("value = {} -- no matching arm", tag_str);
        }

        format!("value = {} -- no matching arm", target_str)
    } else {
        // 評価に失敗した場合、アームの情報からヒントを生成
        let covered: Vec<String> = arms
            .iter()
            .map(|arm| match &arm.pattern {
                Pattern::Literal(n) => format!("{}", n),
                Pattern::Variant { variant_name, .. } => variant_name.clone(),
                Pattern::Variable(name) => format!("_{} (bind)", name),
                Pattern::Wildcard => "_".to_string(),
            })
            .collect();
        format!(
            "(could not evaluate; covered patterns: [{}])",
            covered.join(", ")
        )
    }
}
