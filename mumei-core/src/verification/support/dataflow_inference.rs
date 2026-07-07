#![allow(unused_imports)]
use super::super::module_env::*;
use super::super::translator::*;
use super::super::types::*;
use super::super::*;
use super::call_graph::{
    collect_callees_stmt, collect_callees_with_args_stmt, expr_to_source_string,
};
use crate::hir::HirAtom;
use crate::parser::*;
use regex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};

// =============================================================================
// Source-map data flow trace (spurious counterexample debugging)
// =============================================================================

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct DataFlowTrace {
    pub initial_state: Vec<VariableState>,
    pub execution_path: Vec<ExecutionStep>,
    pub violation: ViolationInfo,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct VariableState {
    pub name: String,
    pub value: String,
    pub line: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ExecutionStep {
    pub line: usize,
    pub expression: String,
    pub mutations: Vec<VariableMutation>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct VariableMutation {
    pub name: String,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ViolationInfo {
    pub line: usize,
    pub contract_type: String,
    pub expression: String,
    pub evaluated_as: String,
}

#[derive(Debug, Clone, PartialEq)]
enum TraceValue {
    Int(i64),
    Bool(bool),
    String(String),
}

/// Build an LLM-readable data-flow trace from a concrete Z3 model.
///
/// The trace replays the HIR body under Mumei semantics, records source-line
/// mutations, and localizes the failing postcondition. It is intentionally
/// conservative: unsupported statement/expression forms return `None` rather
/// than producing misleading debug stories.
pub fn build_data_flow_trace(
    atom: &Atom,
    model: &HashMap<String, i64>,
    module_env: &ModuleEnv,
    hir_atom: &HirAtom,
) -> Option<DataFlowTrace> {
    let mut env = model.clone();
    let initial_state: Vec<VariableState> = atom
        .params
        .iter()
        .filter_map(|param| {
            let value = model.get(&param.name)?;
            Some(VariableState {
                name: param.name.clone(),
                value: value.to_string(),
                line: atom.span.line,
            })
        })
        .collect();

    let mut execution_path = Vec::new();
    let result = trace_stmt(
        &hir_atom.body_stmt,
        &mut env,
        module_env,
        &mut execution_path,
        0,
    )?;
    match &result {
        TraceValue::Int(value) => {
            env.insert("result".to_string(), *value);
        }
        TraceValue::Bool(value) => {
            env.insert("result".to_string(), i64::from(*value));
        }
        TraceValue::String(_) => {}
    }

    let ensures_expr = parse_expression(&atom.ensures);
    let evaluated = trace_eval_expr(&ensures_expr, &mut env, module_env, 0)?;
    let is_satisfied = trace_value_as_bool(&evaluated)?;
    if is_satisfied {
        return None;
    }

    Some(DataFlowTrace {
        initial_state,
        execution_path,
        violation: ViolationInfo {
            line: atom.span.line,
            contract_type: "ensures".to_string(),
            expression: atom.ensures.clone(),
            evaluated_as: format!(
                "{} ({})",
                trace_evaluated_expression(&ensures_expr, &env),
                if is_satisfied { "TRUE" } else { "FALSE" }
            ),
        },
    })
}

fn trace_stmt(
    stmt: &Stmt,
    env: &mut HashMap<String, i64>,
    module_env: &ModuleEnv,
    execution_path: &mut Vec<ExecutionStep>,
    depth: usize,
) -> Option<TraceValue> {
    match stmt {
        Stmt::Let { var, value, span } | Stmt::Assign { var, value, span } => {
            let before = env
                .get(var)
                .map(i64::to_string)
                .unwrap_or_else(|| "<unbound>".to_string());
            let eval = trace_eval_expr(value, env, module_env, depth)?;
            let after = trace_value_as_int(&eval)?;
            env.insert(var.clone(), after);
            let after_string = after.to_string();
            execution_path.push(ExecutionStep {
                line: span.line,
                expression: format!("{} = {}", var, expr_to_source_string(value)),
                mutations: vec![VariableMutation {
                    name: var.clone(),
                    before,
                    after: after_string,
                }],
            });
            Some(TraceValue::Int(after))
        }
        Stmt::Block(stmts, _) => {
            let mut last = TraceValue::Int(0);
            for stmt in stmts {
                last = trace_stmt(stmt, env, module_env, execution_path, depth)?;
            }
            Some(last)
        }
        Stmt::Expr(expr, span) => {
            let value = trace_eval_expr(expr, env, module_env, depth)?;
            execution_path.push(ExecutionStep {
                line: span.line,
                expression: expr_to_source_string(expr),
                mutations: Vec::new(),
            });
            Some(value)
        }
        Stmt::While { .. }
        | Stmt::Acquire { .. }
        | Stmt::Task { .. }
        | Stmt::TaskGroup { .. }
        | Stmt::Cancel { .. }
        | Stmt::ArrayStore { .. } => None,
    }
}

fn trace_eval_expr(
    expr: &Expr,
    env: &mut HashMap<String, i64>,
    module_env: &ModuleEnv,
    depth: usize,
) -> Option<TraceValue> {
    if depth > 8 {
        return None;
    }

    match expr {
        Expr::Number(value) => Some(TraceValue::Int(*value)),
        Expr::Float(_) => None,
        Expr::StringLit(value) => Some(TraceValue::String(value.clone())),
        Expr::Variable(name) if name == "true" => Some(TraceValue::Bool(true)),
        Expr::Variable(name) if name == "false" => Some(TraceValue::Bool(false)),
        Expr::Variable(name) => env.get(name).copied().map(TraceValue::Int),
        Expr::BinaryOp(left, op, right) => {
            let left_value = trace_eval_expr(left, env, module_env, depth)?;
            let right_value = trace_eval_expr(right, env, module_env, depth)?;
            trace_eval_binary(left_value, op, right_value)
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            let cond_value = trace_eval_expr(cond, env, module_env, depth)?;
            if trace_value_as_bool(&cond_value)? {
                trace_stmt(then_branch, env, module_env, &mut Vec::new(), depth)
            } else {
                trace_stmt(else_branch, env, module_env, &mut Vec::new(), depth)
            }
        }
        Expr::Call(name, args) => trace_eval_atom_call(name, args, env, module_env, depth + 1),
        Expr::ArrayAccess(_, _)
        | Expr::StructInit { .. }
        | Expr::FieldAccess(_, _)
        | Expr::Match { .. }
        | Expr::Async { .. }
        | Expr::Await { .. }
        | Expr::AtomRef { .. }
        | Expr::CallRef { .. }
        | Expr::Perform { .. }
        | Expr::Lambda { .. }
        | Expr::ChanSend { .. }
        | Expr::ChanRecv { .. } => None,
    }
}

fn trace_eval_atom_call(
    name: &str,
    args: &[Expr],
    env: &mut HashMap<String, i64>,
    module_env: &ModuleEnv,
    depth: usize,
) -> Option<TraceValue> {
    let callee = module_env.get_atom(name)?;
    if callee.trust_level == TrustLevel::Trusted || args.len() != callee.params.len() {
        return None;
    }

    let mut call_env = HashMap::new();
    for (param, arg) in callee.params.iter().zip(args) {
        let value = trace_eval_expr(arg, env, module_env, depth)?;
        call_env.insert(param.name.clone(), trace_value_as_int(&value)?);
    }

    if !trace_eval_bool_clause(&callee.requires, &mut call_env, module_env)? {
        return None;
    }
    let body = parse_body_expr(&callee.body_expr);
    let result = trace_stmt(&body, &mut call_env, module_env, &mut Vec::new(), depth)?;
    match &result {
        TraceValue::Int(value) => {
            call_env.insert("result".to_string(), *value);
        }
        TraceValue::Bool(value) => {
            call_env.insert("result".to_string(), i64::from(*value));
        }
        TraceValue::String(_) => {}
    }
    if !trace_eval_bool_clause(&callee.ensures, &mut call_env, module_env)? {
        return None;
    }
    Some(result)
}

fn trace_eval_bool_clause(
    clause: &str,
    env: &mut HashMap<String, i64>,
    module_env: &ModuleEnv,
) -> Option<bool> {
    if clause.trim().is_empty() || clause.trim() == "true" {
        return Some(true);
    }
    let expr = parse_expression(clause);
    let value = trace_eval_expr(&expr, env, module_env, 0)?;
    trace_value_as_bool(&value)
}

fn trace_eval_binary(left: TraceValue, op: &Op, right: TraceValue) -> Option<TraceValue> {
    match (left, right) {
        (TraceValue::Int(left), TraceValue::Int(right)) => match op {
            Op::Add => Some(TraceValue::Int(left + right)),
            Op::Sub => Some(TraceValue::Int(left - right)),
            Op::Mul => Some(TraceValue::Int(left * right)),
            Op::Pow if right >= 0 => left.checked_pow(right as u32).map(TraceValue::Int),
            Op::Pow => None,
            Op::Div if right != 0 => Some(TraceValue::Int(left / right)),
            Op::Div => None,
            Op::Eq => Some(TraceValue::Bool(left == right)),
            Op::Neq => Some(TraceValue::Bool(left != right)),
            Op::Gt => Some(TraceValue::Bool(left > right)),
            Op::Lt => Some(TraceValue::Bool(left < right)),
            Op::Ge => Some(TraceValue::Bool(left >= right)),
            Op::Le => Some(TraceValue::Bool(left <= right)),
            Op::And => Some(TraceValue::Bool(left != 0 && right != 0)),
            Op::Or => Some(TraceValue::Bool(left != 0 || right != 0)),
            Op::Implies => Some(TraceValue::Bool(left == 0 || right != 0)),
        },
        (TraceValue::Bool(left), TraceValue::Bool(right)) => match op {
            Op::Eq => Some(TraceValue::Bool(left == right)),
            Op::Neq => Some(TraceValue::Bool(left != right)),
            Op::And => Some(TraceValue::Bool(left && right)),
            Op::Or => Some(TraceValue::Bool(left || right)),
            Op::Implies => Some(TraceValue::Bool(!left || right)),
            _ => None,
        },
        (TraceValue::String(left), TraceValue::String(right)) => match op {
            Op::Eq => Some(TraceValue::Bool(left == right)),
            Op::Neq => Some(TraceValue::Bool(left != right)),
            _ => None,
        },
        (left, right) => {
            let left_int = trace_value_as_int(&left);
            let right_int = trace_value_as_int(&right);
            let left_bool = trace_value_as_bool(&left);
            let right_bool = trace_value_as_bool(&right);
            match (left_int, right_int, left_bool, right_bool, op) {
                (Some(left), Some(right), _, _, Op::Eq) => Some(TraceValue::Bool(left == right)),
                (Some(left), Some(right), _, _, Op::Neq) => Some(TraceValue::Bool(left != right)),
                (_, _, Some(left), Some(right), Op::And) => Some(TraceValue::Bool(left && right)),
                (_, _, Some(left), Some(right), Op::Or) => Some(TraceValue::Bool(left || right)),
                (_, _, Some(left), Some(right), Op::Implies) => {
                    Some(TraceValue::Bool(!left || right))
                }
                _ => None,
            }
        }
    }
}

fn trace_value_as_int(value: &TraceValue) -> Option<i64> {
    match value {
        TraceValue::Int(value) => Some(*value),
        TraceValue::Bool(value) => Some(i64::from(*value)),
        TraceValue::String(_) => None,
    }
}

fn trace_value_as_bool(value: &TraceValue) -> Option<bool> {
    match value {
        TraceValue::Bool(value) => Some(*value),
        TraceValue::Int(value) => Some(*value != 0),
        TraceValue::String(_) => None,
    }
}

fn trace_evaluated_expression(expr: &Expr, env: &HashMap<String, i64>) -> String {
    match expr {
        Expr::Number(value) => value.to_string(),
        Expr::Float(value) => value.to_string(),
        Expr::StringLit(value) => format!("\"{}\"", value),
        Expr::Variable(name) if name == "true" || name == "false" => name.clone(),
        Expr::Variable(name) => env.get(name).map_or_else(|| name.clone(), i64::to_string),
        Expr::BinaryOp(left, op, right) => format!(
            "{} {} {}",
            trace_evaluated_expression(left, env),
            trace_op_symbol(op),
            trace_evaluated_expression(right, env)
        ),
        Expr::Call(name, args) => {
            let args = args
                .iter()
                .map(|arg| trace_evaluated_expression(arg, env))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}({})", name, args)
        }
        Expr::FieldAccess(base, field) => {
            format!("{}.{}", trace_evaluated_expression(base, env), field)
        }
        Expr::ArrayAccess(name, idx) => {
            format!("{}[{}]", name, trace_evaluated_expression(idx, env))
        }
        _ => expr_to_source_string(expr),
    }
}

fn trace_op_symbol(op: &Op) -> &'static str {
    match op {
        Op::Add => "+",
        Op::Sub => "-",
        Op::Mul => "*",
        Op::Div => "/",
        Op::Eq => "==",
        Op::Neq => "!=",
        Op::Gt => ">",
        Op::Lt => "<",
        Op::Ge => ">=",
        Op::Le => "<=",
        Op::And => "&&",
        Op::Or => "||",
        Op::Implies => "==>",
        Op::Pow => "**",
    }
}

// =============================================================================
// Step 3: Effect Inference（エフェクト推論）
// =============================================================================

/// body 内の関数呼び出しからエフェクトセットを推論する。
/// 呼び出し先 atom の effects フィールドを再帰的に集約する。
/// 親エフェクトへの暗黙的包含も解決する。
pub(crate) fn infer_effects(atom: &Atom, body_stmt: &Stmt, module_env: &ModuleEnv) -> Vec<Effect> {
    let callees = collect_callees_stmt(body_stmt);
    let mut inferred = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();
    for callee_name in &callees {
        if let Some(callee) = module_env.get_atom(callee_name) {
            for eff in &callee.effects {
                if seen_names.insert(eff.name.clone()) {
                    inferred.push(eff.clone());
                }
                // NOTE: ancestors are NOT added to seen_names to avoid suppressing
                // explicit effect requirements from other callees. The deduplication
                // via seen_names only applies to effects with the exact same name.
                // Subtype coverage is handled separately by infer_effects_json's
                // is_subeffect() check when computing missing_effects.
            }
        }
    }

    // atom_ref パラメータの effect_set からもエフェクトを推論
    for param in &atom.params {
        if let Some(ref type_ref) = param.type_ref {
            if type_ref.is_fn_type() {
                if let Some(ref effect_set) = type_ref.effect_set {
                    for eff_name in effect_set {
                        if seen_names.insert(eff_name.clone()) {
                            inferred.push(Effect::simple(eff_name));
                        }
                    }
                }
            }
        }
    }

    inferred
}

/// 全 atom のエフェクト推論結果を JSON で出力する。
/// MCP の get_inferred_effects ツールから呼ばれる。
pub fn infer_effects_json(items: &[Item], module_env: &ModuleEnv) -> serde_json::Value {
    let mut results = Vec::new();
    for item in items {
        if let Item::Atom(atom) = item {
            let declared: Vec<String> = atom.effects.iter().map(|e| e.name.clone()).collect();
            let body_stmt = parse_body_expr(&atom.body_expr);
            let inferred = infer_effects(atom, &body_stmt, module_env);
            let inferred_names: Vec<String> = inferred.iter().map(|e| e.name.clone()).collect();
            let missing: Vec<String> = inferred_names
                .iter()
                .filter(|n| {
                    !declared.contains(n) && !declared.iter().any(|d| module_env.is_subeffect(n, d))
                })
                .cloned()
                .collect();
            let suggestion = if missing.is_empty() {
                serde_json::Value::Null
            } else {
                let all_effects: Vec<String> =
                    declared.iter().chain(missing.iter()).cloned().collect();
                serde_json::Value::String(format!("effects: [{}];", all_effects.join(", ")))
            };
            results.push(serde_json::json!({
                "atom": atom.name,
                "declared_effects": declared,
                "inferred_effects": inferred_names,
                "missing_effects": missing,
                "suggestion": suggestion
            }));
        }
    }
    serde_json::json!({ "effects_analysis": results })
}

// =============================================================================
// Plan 13: Contract Inference Engine
// =============================================================================
// Dataflow analysis to infer requires/ensures contracts for atoms.
// - infer_requires: divisor tracking + callee requires propagation
// - infer_ensures: simple body expression analysis + non-negativity

/// Collect all divisor expressions from a statement (Plan 13-1 helper).
/// Tracks expressions used as right-hand side of division operations.
pub(crate) fn collect_divisors_expr(expr: &Expr) -> Vec<String> {
    let mut divisors = Vec::new();
    match expr {
        Expr::BinaryOp(lhs, Op::Div, rhs) => {
            // The right-hand side is a divisor
            match rhs.as_ref() {
                Expr::Variable(name) => divisors.push(name.clone()),
                Expr::Number(n) if *n == 0 => divisors.push("0".to_string()),
                _ => {}
            }
            // Also recurse into sub-expressions (both sides)
            divisors.extend(collect_divisors_expr(lhs));
            divisors.extend(collect_divisors_expr(rhs));
        }
        Expr::BinaryOp(lhs, _, rhs) => {
            divisors.extend(collect_divisors_expr(lhs));
            divisors.extend(collect_divisors_expr(rhs));
        }
        Expr::Call(_, args) => {
            for arg in args {
                divisors.extend(collect_divisors_expr(arg));
            }
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            divisors.extend(collect_divisors_expr(cond));
            divisors.extend(collect_divisors_stmt(then_branch));
            divisors.extend(collect_divisors_stmt(else_branch));
        }
        Expr::Match { target, arms } => {
            divisors.extend(collect_divisors_expr(target));
            for arm in arms {
                divisors.extend(collect_divisors_stmt(&arm.body));
            }
        }
        Expr::Async { body } => {
            divisors.extend(collect_divisors_stmt(body));
        }
        Expr::Await { expr } => {
            divisors.extend(collect_divisors_expr(expr));
        }
        Expr::Lambda { body, .. } => {
            divisors.extend(collect_divisors_stmt(body));
        }
        Expr::Perform { args, .. } => {
            for arg in args {
                divisors.extend(collect_divisors_expr(arg));
            }
        }
        Expr::CallRef { callee, args } => {
            divisors.extend(collect_divisors_expr(callee));
            for arg in args {
                divisors.extend(collect_divisors_expr(arg));
            }
        }
        Expr::ChanSend { channel, value } => {
            divisors.extend(collect_divisors_expr(channel));
            divisors.extend(collect_divisors_expr(value));
        }
        Expr::ChanRecv { channel } => {
            divisors.extend(collect_divisors_expr(channel));
        }
        _ => {}
    }
    divisors
}

/// Collect all divisor expressions from a statement.
pub(crate) fn collect_divisors_stmt(stmt: &Stmt) -> Vec<String> {
    let mut divisors = Vec::new();
    match stmt {
        Stmt::Block(stmts, _) => {
            for s in stmts {
                divisors.extend(collect_divisors_stmt(s));
            }
        }
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            divisors.extend(collect_divisors_expr(value));
        }
        Stmt::ArrayStore { index, value, .. } => {
            divisors.extend(collect_divisors_expr(index));
            divisors.extend(collect_divisors_expr(value));
        }
        Stmt::While { cond, body, .. } => {
            divisors.extend(collect_divisors_expr(cond));
            divisors.extend(collect_divisors_stmt(body));
        }
        Stmt::Acquire { body, .. } => {
            divisors.extend(collect_divisors_stmt(body));
        }
        Stmt::Task { body, .. } => {
            divisors.extend(collect_divisors_stmt(body));
        }
        Stmt::TaskGroup { children, .. } => {
            for child in children {
                divisors.extend(collect_divisors_stmt(child));
            }
        }
        Stmt::Expr(e, _) => {
            divisors.extend(collect_divisors_expr(e));
        }
        _ => {}
    }
    divisors
}

/// Infer requires constraints for an atom (Plan 13-1).
/// Analyzes the body to find:
/// 1. Division operations → "divisor != 0"
/// 2. Callee requires propagation → caller must satisfy callee's requires
pub(crate) fn infer_requires(atom: &Atom, module_env: &ModuleEnv) -> Vec<String> {
    let mut requires = Vec::new();
    let mut seen = HashSet::new();
    let body_stmt = parse_body_expr(&atom.body_expr);

    // 1. Divisor tracking
    let divisors = collect_divisors_stmt(&body_stmt);
    let param_names: HashSet<String> = atom.params.iter().map(|p| p.name.clone()).collect();
    for div in &divisors {
        if param_names.contains(div) && seen.insert(format!("{} != 0", div)) {
            // Check if already covered by refinement type
            let is_covered = atom.params.iter().any(|p| {
                if p.name == *div {
                    if let Some(ref tr) = p.type_ref {
                        if let Some(rt) = module_env.get_type(&tr.name) {
                            // If the type already ensures non-zero (e.g., Pos type)
                            return rt.predicate_raw.contains("> 0")
                                || rt.predicate_raw.contains("!= 0");
                        }
                    }
                }
                false
            });
            if !is_covered {
                requires.push(format!("{} != 0", div));
            }
        }
    }

    // 2. Callee requires propagation with argument substitution
    // callee の param_name → caller の引数式文字列のマッピングを構築し、
    // callee の requires 内のパラメータ名を caller の引数式で置換してから伝播する。
    let callees_with_args = collect_callees_with_args_stmt(&body_stmt);
    for (callee_name, call_args) in &callees_with_args {
        if let Some(callee_atom) = module_env.get_atom(callee_name) {
            if callee_atom.requires != "true" && !callee_atom.requires.is_empty() {
                let mut substituted_req = callee_atom.requires.clone();
                // callee の仮引数名と呼び出し引数を zip して置換
                // 同時置換: まずパラメータ名をユニークなプレースホルダに置換し、
                // 次にプレースホルダを引数式に置換する。
                // これにより逐次置換の衝突（例: a→b, b→a で a>b が a>a になる）を防ぐ。
                // First pass: param names → unique placeholders
                for (i, param) in callee_atom.params.iter().enumerate() {
                    let param_re =
                        regex::Regex::new(&format!(r"\b{}\b", regex::escape(&param.name))).unwrap();
                    substituted_req = param_re
                        .replace_all(
                            &substituted_req,
                            regex::NoExpand(&format!("__PARAM_{}__", i)),
                        )
                        .to_string();
                }
                // Second pass: placeholders → argument expressions
                for (i, arg_expr) in call_args.iter().enumerate() {
                    let arg_str = expr_to_source_string(arg_expr);
                    substituted_req =
                        substituted_req.replace(&format!("__PARAM_{}__", i), &arg_str);
                }
                if seen.insert(substituted_req.clone()) {
                    requires.push(substituted_req);
                }
            }
        }
    }

    requires
}

/// Infer ensures constraints for an atom (Plan 13-2).
/// Analyzes the body to find:
/// 1. Simple body expressions → "result == expr"
/// 2. Non-negativity analysis → "result >= 0" if all paths return non-negative
pub(crate) fn infer_ensures(atom: &Atom, module_env: &ModuleEnv) -> Vec<String> {
    let mut ensures = Vec::new();

    // Simple body expression analysis
    let body_expr_str = atom.body_expr.trim();
    let is_simple = !body_expr_str.contains("if ")
        && !body_expr_str.contains("while ")
        && !body_expr_str.contains("match ")
        && !body_expr_str.contains('{');

    if is_simple && !body_expr_str.is_empty() {
        // Check if the body is a simple arithmetic expression
        let body_stmt = parse_body_expr(&atom.body_expr);
        if let Stmt::Let { .. } = &body_stmt {
            // Skip complex let bindings
        } else {
            ensures.push(format!("result == {}", body_expr_str));
        }
    }

    // Non-negativity analysis: check if all parameters involved are non-negative types
    let param_names: HashSet<String> = atom.params.iter().map(|p| p.name.clone()).collect();
    let all_params_nonneg = atom.params.iter().all(|p| {
        if let Some(ref tr) = p.type_ref {
            if let Some(rt) = module_env.get_type(&tr.name) {
                return rt.predicate_raw.contains(">= 0") || rt.predicate_raw.contains("> 0");
            }
            // Nat type
            if tr.name == "Nat" {
                return true;
            }
        }
        false
    });

    if all_params_nonneg && !param_names.is_empty() {
        // Check if body only uses addition/multiplication (preserves non-negativity).
        // Use character-level check (not space-delimited) to avoid missing `a-b`, `a/b`, `a%b`.
        let body_only_nonneg_ops = !body_expr_str.contains('-')
            && !body_expr_str.contains('/')
            && !body_expr_str.contains('%');
        if body_only_nonneg_ops {
            ensures.push("result >= 0".to_string());
        }
    }

    ensures
}

/// Infer contracts for all atoms in JSON format (Plan 13-3).
/// Called by the CLI command `mumei infer-contracts` and MCP tool.
pub fn infer_contracts_json(items: &[Item], module_env: &ModuleEnv) -> serde_json::Value {
    let mut results = Vec::new();
    for item in items {
        if let Item::Atom(atom) = item {
            let inferred_requires = infer_requires(atom, module_env);
            let inferred_ensures = infer_ensures(atom, module_env);
            let declared_requires = atom.requires.clone();
            let declared_ensures = atom.ensures.clone();

            // Filter out inferred requires already covered by declared
            let new_requires: Vec<String> = inferred_requires
                .iter()
                .filter(|r| !declared_requires.contains(r.as_str()))
                .cloned()
                .collect();

            // Filter out inferred ensures already covered by declared
            let new_ensures: Vec<String> = inferred_ensures
                .iter()
                .filter(|e| !declared_ensures.contains(e.as_str()))
                .cloned()
                .collect();

            let suggestion_requires = if new_requires.is_empty() {
                serde_json::Value::Null
            } else {
                let all_reqs: Vec<String> = if declared_requires == "true" {
                    new_requires.clone()
                } else {
                    let mut all = vec![declared_requires.clone()];
                    all.extend(new_requires.clone());
                    all
                };
                serde_json::Value::String(format!("requires: {};", all_reqs.join(" && ")))
            };

            let suggestion_ensures = if new_ensures.is_empty() {
                serde_json::Value::Null
            } else {
                let all_ens: Vec<String> = if declared_ensures == "true" {
                    new_ensures.clone()
                } else {
                    let mut all = vec![declared_ensures.clone()];
                    all.extend(new_ensures.clone());
                    all
                };
                serde_json::Value::String(format!("ensures: {};", all_ens.join(" && ")))
            };

            results.push(serde_json::json!({
                "atom": atom.name,
                "declared_requires": declared_requires,
                "declared_ensures": declared_ensures,
                "inferred_requires": inferred_requires,
                "inferred_ensures": inferred_ensures,
                "new_requires": new_requires,
                "new_ensures": new_ensures,
                "suggestion_requires": suggestion_requires,
                "suggestion_ensures": suggestion_ensures,
            }));
        }
    }
    serde_json::json!({ "contracts_analysis": results })
}
