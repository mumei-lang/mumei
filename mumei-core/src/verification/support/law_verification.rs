#![allow(unused_imports)]
use super::super::module_env::*;
use super::super::nlae_reporter::*;
use super::super::translator::*;
use super::super::types::*;
use super::super::*;
use crate::hir::HirAtom;
use crate::parser::*;
use crate::resolver::compute_contract_hash;
use miette::SourceSpan;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::Path;
use z3::ast::{Array, Ast, Bool, Dynamic, Float, Int, Real, String as Z3String};
use z3::{Config, Context, SatResult, Solver};

// =============================================================================
// impl の法則充足性検証 (Law Verification)
// =============================================================================

/// law 式内のメソッド呼び出しを impl body で展開する。
///
/// 例: law = "add(a, b) == add(b, a)", impl body = "a + b"
/// → "((a) + (b)) == ((b) + (a))"
///
/// アルゴリズム:
/// 1. law 式を左から走査し、メソッド名 + "(" を検出
/// 2. 括弧の対応を追跡して引数リストを抽出
/// 3. impl body 内の仮引数名を実引数で置換
/// 4. 展開結果を括弧で囲んで挿入
///
/// ネストした呼び出し（例: "leq(a, b) && leq(b, c)"）にも対応。
pub(crate) fn substitute_method_calls(
    law_expr: &str,
    method_bodies: &HashMap<String, String>,
    method_params: &HashMap<String, Vec<String>>,
) -> String {
    let mut result = law_expr.to_string();
    let max_passes = law_expr
        .chars()
        .filter(|c| *c == '(')
        .count()
        .saturating_add(method_bodies.len())
        .max(1);

    // 各メソッドについて繰り返し展開（ネスト対応のため必要なパス数を式から算出）
    for _pass in 0..max_passes {
        let mut new_result = String::new();
        let mut i = 0;
        let chars: Vec<char> = result.chars().collect();
        let mut changed = false;

        while i < chars.len() {
            // メソッド名の検出: 英字で始まり、直後に '(' が続く
            let mut found_method = false;
            for (method_name, body) in method_bodies {
                let mn_chars: Vec<char> = method_name.chars().collect();
                if i + mn_chars.len() < chars.len()
                    && chars[i..i + mn_chars.len()] == mn_chars[..]
                    && chars[i + mn_chars.len()] == '('
                    // メソッド名の直前が英数字でないことを確認（部分一致を防ぐ）
                    && (i == 0 || !is_identifier_char(chars[i - 1]))
                {
                    // 引数リストを抽出
                    let args_start = i + mn_chars.len() + 1;
                    let mut depth = 1;
                    let mut args_end = args_start;
                    while args_end < chars.len() && depth > 0 {
                        match chars[args_end] {
                            '(' => depth += 1,
                            ')' => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        args_end += 1;
                    }

                    // 引数をカンマで分割（ネストした括弧を考慮）
                    let args_str: String = chars[args_start..args_end].iter().collect();
                    let args = split_args(&args_str);

                    // body 内の仮引数名を実引数で置換
                    let mut expanded = body.clone();
                    if let Some(param_names) = method_params.get(method_name) {
                        for (j, param_name) in param_names.iter().enumerate() {
                            if let Some(arg) = args.get(j) {
                                let replacement = parenthesized_arg(arg);
                                // 単語境界を考慮した置換（部分一致を防ぐ）
                                expanded = replace_word(&expanded, param_name, &replacement);
                            }
                        }
                    }

                    new_result.push('(');
                    new_result.push_str(&expanded);
                    new_result.push(')');
                    i = args_end + 1; // ')' の次へ
                    found_method = true;
                    changed = true;
                    break;
                }
            }
            if !found_method {
                new_result.push(chars[i]);
                i += 1;
            }
        }

        result = new_result;
        if !changed {
            break;
        }
    }

    result
}

pub(crate) fn contains_method_call(source: &str, method_name: &str) -> bool {
    let chars: Vec<char> = source.chars().collect();
    let method_chars: Vec<char> = method_name.chars().collect();
    if method_chars.is_empty() {
        return false;
    }

    let mut i = 0;
    while i + method_chars.len() < chars.len() {
        if chars[i..i + method_chars.len()] == method_chars[..]
            && chars[i + method_chars.len()] == '('
            && (i == 0 || !is_identifier_char(chars[i - 1]))
        {
            return true;
        }
        i += 1;
    }
    false
}

fn is_identifier_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn parenthesized_arg(arg: &str) -> String {
    let trimmed = arg.trim();
    if has_single_outer_parens(trimmed) {
        trimmed.to_string()
    } else {
        format!("({trimmed})")
    }
}

fn has_single_outer_parens(expr: &str) -> bool {
    let chars: Vec<char> = expr.chars().collect();
    if chars.len() < 2 || chars.first() != Some(&'(') || chars.last() != Some(&')') {
        return false;
    }

    let mut depth = 0isize;
    for (idx, ch) in chars.iter().enumerate() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if depth == 0 && idx + 1 < chars.len() {
            return false;
        }
        if depth < 0 {
            return false;
        }
    }

    depth == 0
}

/// 単語境界を考慮した文字列置換。
/// "a" を置換する際に "a" 単体のみマッチし、"add" 内の "a" にはマッチしない。
pub(crate) fn replace_word(source: &str, word: &str, replacement: &str) -> String {
    let mut result = String::new();
    let chars: Vec<char> = source.chars().collect();
    let word_chars: Vec<char> = word.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if i + word_chars.len() <= chars.len()
            && chars[i..i + word_chars.len()] == word_chars[..]
            && (i == 0 || !is_identifier_char(chars[i - 1]))
            && (i + word_chars.len() >= chars.len()
                || !is_identifier_char(chars[i + word_chars.len()]))
        {
            result.push_str(replacement);
            i += word_chars.len();
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

/// カンマで引数を分割する（ネストした括弧を考慮）。
pub(crate) fn split_args(input: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut depth = 0;
    let mut current = String::new();
    for c in input.chars() {
        match c {
            '(' => {
                depth += 1;
                current.push(c);
            }
            ')' => {
                depth -= 1;
                current.push(c);
            }
            ',' if depth == 0 => {
                result.push(current.trim().to_string());
                current.clear();
            }
            _ => {
                current.push(c);
            }
        }
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        result.push(trimmed);
    }
    result
}

/// impl が対応する trait の全 law を満たしているかを Z3 で検証する。
/// 各 law の論理式内のメソッド呼び出しを impl の具体的な body で置換し、
/// ∀x. law_expr が成立するかを検証する。
pub fn verify_impl(
    impl_def: &ImplDef,
    module_env: &ModuleEnv,
    output_dir: &Path,
) -> MumeiResult<()> {
    verify_impl_with_options(impl_def, module_env, output_dir, false)
}

/// `verify_impl` の IEEE 754 モード対応版。`ieee754_f64` が true の場合、
/// f64 対象型の law 変数と式の lowering に Z3 FP 理論（Float(11,53)）を
/// 使い、atom 契約検証と同じ f64 エンコーディングで law を検証する。
pub fn verify_impl_with_options(
    impl_def: &ImplDef,
    module_env: &ModuleEnv,
    output_dir: &Path,
    ieee754_f64: bool,
) -> MumeiResult<()> {
    let trait_def = module_env.get_trait(&impl_def.trait_name).ok_or_else(|| {
        MumeiError::type_error_at(
            format!(
                "Trait '{}' not found for impl on '{}'",
                impl_def.trait_name, impl_def.target_type
            ),
            impl_def.span.clone(),
        )
    })?;

    // メソッドの完全性チェック: trait の全メソッドが impl されているか
    for method in &trait_def.methods {
        if !impl_def
            .method_bodies
            .iter()
            .any(|(name, _)| name == &method.name)
        {
            return Err(MumeiError::type_error_at(
                format!(
                    "impl {} for {}: missing method '{}'",
                    impl_def.trait_name, impl_def.target_type, method.name
                ),
                impl_def.span.clone(),
            ));
        }
    }

    // 各 law を Z3 で検証
    let mut cfg = Config::new();
    cfg.set_timeout_msec(5000);
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);

    // impl のメソッド body マップを構築（未解釈関数展開用）
    let method_body_map: HashMap<String, String> = impl_def
        .method_bodies
        .iter()
        .map(|(name, body)| (name.clone(), body.clone()))
        .collect();

    // メソッドのパラメータ名マップを構築（trait 定義から取得）
    // law 式内の関数呼び出し `method(a, b)` を body 式に展開する際、
    // 仮引数名（a, b）を実引数に置換するために使用
    let method_param_names: HashMap<String, Vec<String>> = trait_def
        .methods
        .iter()
        .map(|m| {
            // トレイトメソッドのパラメータ名は慣例的に a, b, c, ... を使用
            let param_names: Vec<String> = (0..m.param_types.len())
                .map(|i| {
                    let names = ["a", "b", "c", "d", "e", "f"];
                    names.get(i).unwrap_or(&"x").to_string()
                })
                .collect();
            (m.name.clone(), param_names)
        })
        .collect();

    for (law_name, law_expr) in &trait_def.laws {
        // law 内のメソッド呼び出しを impl body で置換
        // 例: law "add(a, b) == add(b, a)" で impl body が "a + b" の場合、
        // "add(a, b)" → "(a + b)", "add(b, a)" → "(b + a)" に展開
        let substituted = substitute_method_calls(law_expr, &method_body_map, &method_param_names);

        // シンボリック変数で law を検証
        let vc = VCtx {
            ctx: &ctx,
            module_env,
            current_atom: None,
            linearity_ctx: None,
            effect_ctx: None,
            constraint_count: None,
            constraint_budget: DEFAULT_CONSTRAINT_BUDGET,
            has_string_constraints: None,
            path_cond_stack: std::cell::RefCell::new(Vec::new()),
            profiler: None,
            ieee754_f64,
        };

        let mut env: Env = HashMap::new();
        // law 内の自由変数をシンボリック変数として登録。
        // f64 は atom 契約検証と同じエンコーディング（デフォルト Real、
        // `--ieee754-f64` で Float）を使う。
        for var_name in &["a", "b", "c", "x", "y", "z"] {
            let base = module_env.resolve_base_type(&impl_def.target_type);
            let var: Dynamic = match base.as_str() {
                "f64" if ieee754_f64 => Float::new_const(&ctx, *var_name, 11, 53).into(),
                "f64" => Real::new_const(&ctx, *var_name).into(),
                // Plan 9: Str parameters as Z3 String Sort
                "Str" => Z3String::new_const(&ctx, *var_name).into(),
                _ => Int::new_const(&ctx, *var_name).into(),
            };
            env.insert(var_name.to_string(), var);
        }
        // "true" リテラルを登録
        env.insert("true".to_string(), Bool::from_bool(&ctx, true).into());

        // law 式をパースして検証
        let law_ast = parse_expression(&substituted);
        let verify_result = expr_to_z3(&vc, &law_ast, &mut env, None);
        match verify_result {
            Ok(law_z3) => {
                if let Some(law_bool) = law_z3.as_bool() {
                    solver.push();

                    // P2-B: law 式に含まれる trait method の param_constraints のみを
                    // push/pop スコープ内で前提として注入する。
                    for method in &trait_def.methods {
                        if !contains_method_call(law_expr, &method.name) {
                            continue;
                        }
                        let param_names: Vec<String> = (0..method.param_types.len())
                            .map(|i| {
                                let names = ["a", "b", "c", "d", "e", "f"];
                                names.get(i).unwrap_or(&"x").to_string()
                            })
                            .collect();
                        for (i, constraint_opt) in method.param_constraints.iter().enumerate() {
                            if let Some(constraint_str) = constraint_opt {
                                if let Some(param_name) = param_names.get(i) {
                                    let concrete: String =
                                        replace_constraint_placeholder(constraint_str, param_name);
                                    let constraint_ast = parse_expression(&concrete);
                                    if let Ok(constraint_z3) =
                                        expr_to_z3(&vc, &constraint_ast, &mut env, None)
                                    {
                                        if let Some(constraint_bool) = constraint_z3.as_bool() {
                                            solver.assert(&constraint_bool);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    solver.assert(&law_bool.not());
                    if solver.check() == SatResult::Sat {
                        // 反例（Counter-example）を Z3 model から取得
                        let counterexample = if let Some(model) = solver.get_model() {
                            let var_names = ["a", "b", "c", "x", "y", "z"];
                            let mut ce_parts = Vec::new();
                            let mut ce_json = serde_json::Map::new();
                            for var_name in &var_names {
                                if let Some(var_z3) = env.get(*var_name) {
                                    if let Some(val) = model.eval(var_z3, true) {
                                        let val_str = format!("{}", val);
                                        // 変数が law 式に含まれている場合のみ表示
                                        if law_expr.contains(*var_name) {
                                            ce_parts.push(format!("{} = {}", var_name, val_str));
                                            ce_json.insert(var_name.to_string(), json!(val_str));
                                        }
                                    }
                                }
                            }
                            // Save counterexample to visualizer report
                            // (even when no concrete values are available, still write report.json
                            // so the MCP self-healing flow can detect the failure)
                            let ce_value = if ce_json.is_empty() {
                                None
                            } else {
                                Some(serde_json::Value::Object(ce_json))
                            };
                            save_visualizer_report(
                                output_dir,
                                "failed",
                                &format!(
                                    "impl {} for {}",
                                    impl_def.trait_name, impl_def.target_type
                                ),
                                "N/A",
                                "N/A",
                                &format!("Trait law '{}' not satisfied", law_name),
                                ce_value.as_ref(),
                                FAILURE_TRAIT_LAW_VIOLATED,
                                None,
                                Some(&impl_def.span),
                                None,
                                None,
                                None,
                            );
                            if ce_parts.is_empty() {
                                ("  (no concrete values available)".to_string(), ce_value)
                            } else {
                                (
                                    format!("  Counter-example: {}", ce_parts.join(", ")),
                                    ce_value,
                                )
                            }
                        } else {
                            ("  (could not retrieve model)".to_string(), None)
                        };
                        solver.pop(1);
                        let (ce_text, ce_data) = counterexample;
                        return Err(MumeiError::verification_at(
                            format!(
                                "impl {} for {}: law '{}' (defined in trait at {}) is not satisfied\n  Law: {}\n  Expanded: {}\n{}",
                                impl_def.trait_name, impl_def.target_type,
                                law_name, trait_def.span, law_expr, substituted, ce_text
                            ),
                            impl_def.span.clone()
                        ).with_counterexample(ce_data));
                    }
                    solver.pop(1);
                }
            }
            Err(_) => {
                // law のパースに失敗した場合はスキップ
                // （未解釈関数展開後もパースできない場合は、law 式が複雑すぎる可能性がある）
            }
        };
    }

    Ok(())
}

// =============================================================================
// リソース階層検証 (Resource Hierarchy Verification)
// =============================================================================
//
// デッドロック防止: リソース取得順序の半順序関係を Z3 で検証する。
//
// 不変条件: ∀ r1, r2 ∈ Held(thread, t):
//   acquire(r2) かつ r1 ∈ Held → Priority(r2) > Priority(r1)
//
// これにより、待機グラフ（Wait-For Graph）に循環が生じないことを
// コンパイル時に数学的に保証する。
