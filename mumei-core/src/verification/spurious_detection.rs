use super::module_env::ModuleEnv;
use crate::parser::{parse_body_expr, parse_expression, Atom, Expr, Op, Span, Stmt, TrustLevel};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CounterexampleValidationResult {
    pub is_valid: bool,
    pub validation_status: String,
    pub failed_constraints: Vec<String>,
    pub symbol_provenance: Vec<SymbolProvenance>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct SymbolProvenance {
    pub symbol_name: String,
    pub source: String,
    pub location: Option<Span>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnusedHypothesisReport {
    pub unused_requires: Vec<String>,
    pub unused_invariants: Vec<String>,
    pub unused_effect_constraints: Vec<String>,
    pub minimal_constraint_set: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
enum EvalValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
}

/// A concrete counterexample value extracted from a Z3 model.
///
/// Extends the historical integer-only model representation so that `f64`
/// counterexamples (encoded as Z3 `Real` rationals or IEEE 754 `Float`s) can be
/// replayed under Mumei semantics. `Bool` keeps `result`-style boolean models
/// exact instead of round-tripping through `0`/`1`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CexValue {
    Int(i64),
    Float(f64),
    Bool(bool),
}

impl CexValue {
    fn to_eval(self) -> EvalValue {
        match self {
            CexValue::Int(value) => EvalValue::Int(value),
            CexValue::Float(value) => EvalValue::Float(value),
            CexValue::Bool(value) => EvalValue::Bool(value),
        }
    }
}

impl From<i64> for CexValue {
    fn from(value: i64) -> Self {
        CexValue::Int(value)
    }
}

impl From<f64> for CexValue {
    fn from(value: f64) -> Self {
        CexValue::Float(value)
    }
}

impl From<bool> for CexValue {
    fn from(value: bool) -> Self {
        CexValue::Bool(value)
    }
}

type EvalEnv = HashMap<String, EvalValue>;

fn eval_env_from_model(model: &HashMap<String, CexValue>) -> EvalEnv {
    model
        .iter()
        .map(|(name, value)| (name.clone(), value.to_eval()))
        .collect()
}

pub fn validate_counterexample(
    atom: &Atom,
    model: &HashMap<String, CexValue>,
    module_env: &ModuleEnv,
) -> CounterexampleValidationResult {
    let symbol_provenance = detect_uninterpreted_symbols(atom, model, module_env);
    let mut eval_env = eval_env_from_model(model);

    match eval_bool_clause(&atom.requires, &mut eval_env, module_env) {
        Ok(true) => {}
        Ok(false) => {
            return CounterexampleValidationResult {
                is_valid: false,
                validation_status: "unvalidated".to_string(),
                failed_constraints: vec![format!("requires not satisfied: {}", atom.requires)],
                symbol_provenance,
            };
        }
        Err(err) => {
            return invalid_counterexample_result(
                atom,
                symbol_provenance,
                false,
                format!("requires not replayable: {err}"),
            );
        }
    }

    let body_stmt = parse_body_expr(&atom.body_expr);
    match eval_stmt(&body_stmt, &mut eval_env, module_env, 0) {
        Ok(result @ (EvalValue::Int(_) | EvalValue::Float(_) | EvalValue::Bool(_))) => {
            if let Some(model_result) = model.get("result") {
                if !cex_matches_eval(model_result, &result) {
                    return invalid_counterexample_result(
                        atom,
                        symbol_provenance,
                        true,
                        format!(
                            "Z3 model result {} does not match Mumei body result {}",
                            format_cex_value(model_result),
                            format_eval_value(&result)
                        ),
                    );
                }
            }
            eval_env.insert("result".to_string(), result);
        }
        Ok(EvalValue::String(_)) => {
            return invalid_counterexample_result(
                atom,
                symbol_provenance,
                false,
                "string result is not replayable in Z3 integer model".to_string(),
            );
        }
        Err(err) => {
            return invalid_counterexample_result(
                atom,
                symbol_provenance,
                false,
                format!("body not replayable: {err}"),
            );
        }
    }

    match eval_bool_clause(&atom.ensures, &mut eval_env, module_env) {
        Ok(false) => CounterexampleValidationResult {
            is_valid: true,
            validation_status: "validated".to_string(),
            failed_constraints: vec![format!("ensures: {}", atom.ensures)],
            symbol_provenance,
        },
        Ok(true) => invalid_counterexample_result(
            atom,
            symbol_provenance,
            true,
            "Z3 model does not violate ensures under Mumei semantics".to_string(),
        ),
        Err(err) => invalid_counterexample_result(
            atom,
            symbol_provenance,
            false,
            format!("ensures not replayable: {err}"),
        ),
    }
}

fn invalid_counterexample_result(
    atom: &Atom,
    symbol_provenance: Vec<SymbolProvenance>,
    force_spurious_candidate: bool,
    reason: String,
) -> CounterexampleValidationResult {
    let validation_status = if force_spurious_candidate || !symbol_provenance.is_empty() {
        "spurious_candidate"
    } else {
        "unvalidated"
    };
    let mut failed_constraints = collect_unvalidated_constraints(atom);
    failed_constraints.push(reason);
    CounterexampleValidationResult {
        is_valid: false,
        validation_status: validation_status.to_string(),
        failed_constraints,
        symbol_provenance,
    }
}

pub fn detect_uninterpreted_symbols(
    atom: &Atom,
    _model: &HashMap<String, CexValue>,
    module_env: &ModuleEnv,
) -> Vec<SymbolProvenance> {
    let mut symbols = Vec::new();
    let mut seen = HashSet::new();

    for expr in [
        parse_expression(&atom.requires),
        parse_expression(&atom.ensures),
    ] {
        collect_expr_symbols(&expr, module_env, &mut symbols, &mut seen);
    }
    let body = parse_body_expr(&atom.body_expr);
    collect_stmt_symbols(&body, module_env, &mut symbols, &mut seen);

    symbols
}

pub fn detect_unused_hypotheses(
    atom: &Atom,
    unsat_core: &[String],
    _module_env: &ModuleEnv,
) -> UnusedHypothesisReport {
    let core: HashSet<String> = unsat_core
        .iter()
        .map(|label| normalize_core_label(label))
        .collect();
    let requires_label = format!("requires:{}", atom.name);
    let invariant_label = format!("invariant:{}", atom.name);

    let unused_requires = if atom.requires.trim().is_empty()
        || atom.requires.trim() == "true"
        || core_contains_clause(&core, &requires_label, "requires")
        || core_contains_clause(&core, "track_requires", "track_requires")
    {
        Vec::new()
    } else {
        vec![atom.requires.clone()]
    };

    let unused_invariants = match &atom.invariant {
        Some(invariant)
            if !invariant.trim().is_empty()
                && !core_contains_clause(&core, &invariant_label, "invariant")
                && !core_contains_clause(&core, "track_invariant", "track_invariant") =>
        {
            vec![invariant.clone()]
        }
        _ => Vec::new(),
    };

    let mut unused_effect_constraints = Vec::new();
    for (effect, state) in &atom.effect_pre {
        let label = format!("effect_pre:{}:{}", atom.name, effect);
        if !core_contains_clause(&core, &label, "effect_pre") {
            unused_effect_constraints.push(format!("{}={}", effect, state));
        }
    }
    for (effect, state) in &atom.effect_post {
        let label = format!("effect_post:{}:{}", atom.name, effect);
        if !core_contains_clause(&core, &label, "effect_post") {
            unused_effect_constraints.push(format!("{}={}", effect, state));
        }
    }

    UnusedHypothesisReport {
        unused_requires,
        unused_invariants,
        unused_effect_constraints,
        minimal_constraint_set: unsat_core.to_vec(),
    }
}

fn eval_bool_clause(
    clause: &str,
    env: &mut EvalEnv,
    module_env: &ModuleEnv,
) -> Result<bool, String> {
    if clause.trim().is_empty() || clause.trim() == "true" {
        return Ok(true);
    }
    match eval_expr(&parse_expression(clause), env, module_env, 0)? {
        EvalValue::Bool(value) => Ok(value),
        EvalValue::Int(value) => Ok(value != 0),
        EvalValue::Float(value) => Ok(value != 0.0),
        EvalValue::String(_) => Err("string clause cannot be evaluated as bool".to_string()),
    }
}

fn eval_stmt(
    stmt: &Stmt,
    env: &mut EvalEnv,
    module_env: &ModuleEnv,
    depth: usize,
) -> Result<EvalValue, String> {
    match stmt {
        Stmt::Let { var, value, .. } | Stmt::Assign { var, value, .. } => {
            let eval = eval_expr(value, env, module_env, depth)?;
            match eval {
                EvalValue::Int(_) | EvalValue::Float(_) | EvalValue::Bool(_) => {
                    env.insert(var.clone(), eval.clone());
                    Ok(eval)
                }
                EvalValue::String(_) => Err(format!("{} is not a scalar binding", var)),
            }
        }
        Stmt::Block(stmts, _) => {
            let mut last = EvalValue::Int(0);
            for stmt in stmts {
                last = eval_stmt(stmt, env, module_env, depth)?;
            }
            Ok(last)
        }
        Stmt::Expr(expr, _) => eval_expr(expr, env, module_env, depth),
        Stmt::While { .. }
        | Stmt::Acquire { .. }
        | Stmt::Task { .. }
        | Stmt::TaskGroup { .. }
        | Stmt::Cancel { .. }
        | Stmt::ArrayStore { .. } => {
            Err("statement form is not evaluable in counterexample replay".to_string())
        }
    }
}

fn eval_expr(
    expr: &Expr,
    env: &mut EvalEnv,
    module_env: &ModuleEnv,
    depth: usize,
) -> Result<EvalValue, String> {
    if depth > 8 {
        return Err("counterexample replay recursion limit exceeded".to_string());
    }

    match expr {
        Expr::Number(value) => Ok(EvalValue::Int(*value)),
        Expr::Float(value) => Ok(EvalValue::Float(*value)),
        Expr::StringLit(value) => Ok(EvalValue::String(value.clone())),
        Expr::Variable(name) if name == "true" => Ok(EvalValue::Bool(true)),
        Expr::Variable(name) if name == "false" => Ok(EvalValue::Bool(false)),
        Expr::Variable(name) => env
            .get(name)
            .cloned()
            .ok_or_else(|| format!("missing model value for '{}'", name)),
        Expr::BinaryOp(left, op, right) => {
            let left_value = eval_expr(left, env, module_env, depth)?;
            let right_value = eval_expr(right, env, module_env, depth)?;
            eval_binary(left_value, op, right_value)
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => match eval_expr(cond, env, module_env, depth)? {
            EvalValue::Bool(true) => eval_stmt(then_branch, env, module_env, depth),
            EvalValue::Bool(false) => eval_stmt(else_branch, env, module_env, depth),
            EvalValue::Int(value) if value != 0 => eval_stmt(then_branch, env, module_env, depth),
            EvalValue::Int(_) => eval_stmt(else_branch, env, module_env, depth),
            EvalValue::Float(value) if value != 0.0 => {
                eval_stmt(then_branch, env, module_env, depth)
            }
            EvalValue::Float(_) => eval_stmt(else_branch, env, module_env, depth),
            EvalValue::String(_) => Err("if condition is not boolean".to_string()),
        },
        Expr::Call(name, args) => eval_atom_call(name, args, env, module_env, depth + 1),
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
        | Expr::ChanRecv { .. } => {
            Err("expression form is not evaluable in counterexample replay".to_string())
        }
    }
}

fn eval_atom_call(
    name: &str,
    args: &[Expr],
    env: &mut EvalEnv,
    module_env: &ModuleEnv,
    depth: usize,
) -> Result<EvalValue, String> {
    let callee = module_env
        .get_atom(name)
        .ok_or_else(|| format!("uninterpreted function '{}'", name))?;
    if callee.trust_level == TrustLevel::Trusted {
        return Err(format!("trusted atom '{}' cannot be replayed", name));
    }
    if args.len() != callee.params.len() {
        return Err(format!("arity mismatch for atom '{}'", name));
    }

    let mut call_env: EvalEnv = HashMap::new();
    for (param, arg) in callee.params.iter().zip(args) {
        match eval_expr(arg, env, module_env, depth)? {
            value @ (EvalValue::Int(_) | EvalValue::Float(_) | EvalValue::Bool(_)) => {
                call_env.insert(param.name.clone(), value);
            }
            EvalValue::String(_) => return Err(format!("non-scalar argument for atom '{}'", name)),
        }
    }

    if !eval_bool_clause(&callee.requires, &mut call_env, module_env)? {
        return Err(format!("callee '{}' requires clause is false", name));
    }
    let body = parse_body_expr(&callee.body_expr);
    let result = eval_stmt(&body, &mut call_env, module_env, depth)?;
    match &result {
        EvalValue::Int(_) | EvalValue::Float(_) | EvalValue::Bool(_) => {
            call_env.insert("result".to_string(), result.clone());
        }
        EvalValue::String(_) => {}
    }
    if !eval_bool_clause(&callee.ensures, &mut call_env, module_env)? {
        return Err(format!("callee '{}' ensures clause is false", name));
    }
    Ok(result)
}

fn eval_binary(left: EvalValue, op: &Op, right: EvalValue) -> Result<EvalValue, String> {
    match (&left, &right) {
        (EvalValue::Int(left), EvalValue::Int(right)) => {
            let (left, right) = (*left, *right);
            match op {
                Op::Add => Ok(EvalValue::Int(left + right)),
                Op::Sub => Ok(EvalValue::Int(left - right)),
                Op::Mul => Ok(EvalValue::Int(left * right)),
                Op::Pow if right >= 0 => {
                    Ok(EvalValue::Int(left.checked_pow(right as u32).ok_or_else(
                        || "integer overflow during counterexample replay".to_string(),
                    )?))
                }
                Op::Pow => Err("negative exponent during counterexample replay".to_string()),
                Op::Div if right != 0 => Ok(EvalValue::Int(left / right)),
                Op::Div => Err("division by zero during counterexample replay".to_string()),
                Op::Eq => Ok(EvalValue::Bool(left == right)),
                Op::Neq => Ok(EvalValue::Bool(left != right)),
                Op::Gt => Ok(EvalValue::Bool(left > right)),
                Op::Lt => Ok(EvalValue::Bool(left < right)),
                Op::Ge => Ok(EvalValue::Bool(left >= right)),
                Op::Le => Ok(EvalValue::Bool(left <= right)),
                Op::And => Ok(EvalValue::Bool(left != 0 && right != 0)),
                Op::Or => Ok(EvalValue::Bool(left != 0 || right != 0)),
                Op::Implies => Ok(EvalValue::Bool(left == 0 || right != 0)),
            }
        }
        (EvalValue::Bool(left), EvalValue::Bool(right)) => {
            let (left, right) = (*left, *right);
            match op {
                Op::Eq => Ok(EvalValue::Bool(left == right)),
                Op::Neq => Ok(EvalValue::Bool(left != right)),
                Op::And => Ok(EvalValue::Bool(left && right)),
                Op::Or => Ok(EvalValue::Bool(left || right)),
                Op::Implies => Ok(EvalValue::Bool(!left || right)),
                _ => Err("unsupported boolean arithmetic in counterexample replay".to_string()),
            }
        }
        (EvalValue::String(left), EvalValue::String(right)) => match op {
            Op::Eq => Ok(EvalValue::Bool(left == right)),
            Op::Neq => Ok(EvalValue::Bool(left != right)),
            _ => Err("unsupported string operation in counterexample replay".to_string()),
        },
        _ => {
            // Numeric path with at least one `f64`: replay under IEEE 754 `f64`
            // semantics (matching Mumei's runtime `f64`), promoting `i64`
            // operands to `f64`.
            if let (Some(l), Some(r)) = (value_as_f64(&left), value_as_f64(&right)) {
                return match op {
                    Op::Add => Ok(EvalValue::Float(l + r)),
                    Op::Sub => Ok(EvalValue::Float(l - r)),
                    Op::Mul => Ok(EvalValue::Float(l * r)),
                    Op::Pow => Ok(EvalValue::Float(l.powf(r))),
                    Op::Div => Ok(EvalValue::Float(l / r)),
                    Op::Eq => Ok(EvalValue::Bool(l == r)),
                    Op::Neq => Ok(EvalValue::Bool(l != r)),
                    Op::Gt => Ok(EvalValue::Bool(l > r)),
                    Op::Lt => Ok(EvalValue::Bool(l < r)),
                    Op::Ge => Ok(EvalValue::Bool(l >= r)),
                    Op::Le => Ok(EvalValue::Bool(l <= r)),
                    Op::And => Ok(EvalValue::Bool(l != 0.0 && r != 0.0)),
                    Op::Or => Ok(EvalValue::Bool(l != 0.0 || r != 0.0)),
                    Op::Implies => Ok(EvalValue::Bool(l == 0.0 || r != 0.0)),
                };
            }
            let left_bool = value_as_bool(&left);
            let right_bool = value_as_bool(&right);
            match (left_bool, right_bool, op) {
                (Some(left), Some(right), Op::And) => Ok(EvalValue::Bool(left && right)),
                (Some(left), Some(right), Op::Or) => Ok(EvalValue::Bool(left || right)),
                (Some(left), Some(right), Op::Implies) => Ok(EvalValue::Bool(!left || right)),
                _ => Err("type mismatch during counterexample replay".to_string()),
            }
        }
    }
}

fn value_as_f64(value: &EvalValue) -> Option<f64> {
    match value {
        EvalValue::Int(value) => Some(*value as f64),
        EvalValue::Float(value) => Some(*value),
        EvalValue::Bool(_) | EvalValue::String(_) => None,
    }
}

fn value_as_bool(value: &EvalValue) -> Option<bool> {
    match value {
        EvalValue::Bool(value) => Some(*value),
        EvalValue::Int(value) => Some(*value != 0),
        EvalValue::Float(value) => Some(*value != 0.0),
        EvalValue::String(_) => None,
    }
}

/// Relative/absolute tolerance for comparing a Z3 model `result` against the
/// body value recomputed under Mumei `f64` semantics. When the default `Real`
/// encoding is used, the model returns an exact rational that may not be a
/// representable `f64`, so an exact comparison would spuriously reject valid
/// counterexamples. IEEE 754 mode reproduces the value exactly and still passes.
fn floats_close(a: f64, b: f64) -> bool {
    if a == b {
        return true;
    }
    if a.is_nan() && b.is_nan() {
        return true;
    }
    let diff = (a - b).abs();
    let scale = a.abs().max(b.abs()).max(1.0);
    diff <= 1e-9 * scale
}

fn cex_matches_eval(model: &CexValue, body: &EvalValue) -> bool {
    match (model, body) {
        (CexValue::Int(m), EvalValue::Int(b)) => m == b,
        (CexValue::Bool(m), EvalValue::Bool(b)) => m == b,
        (CexValue::Int(m), EvalValue::Bool(b)) => *m == i64::from(*b),
        (CexValue::Bool(m), EvalValue::Int(b)) => i64::from(*m) == *b,
        _ => match (cex_as_f64(model), value_as_f64(body)) {
            (Some(m), Some(b)) => floats_close(m, b),
            _ => false,
        },
    }
}

/// Parse Z3's printed numeric forms into `f64`: decimals (`0.3`, `3.0?`),
/// rationals (`1/10`), and integers. The trailing `?` that Z3 appends to
/// approximate reals is stripped.
pub fn parse_z3_numeric_to_f64(text: &str) -> Option<f64> {
    let trimmed = text.trim().trim_end_matches('?').trim();
    if let Some((num, den)) = trimmed.split_once('/') {
        let num: f64 = num.trim().parse().ok()?;
        let den: f64 = den.trim().parse().ok()?;
        if den == 0.0 {
            return None;
        }
        return Some(num / den);
    }
    trimmed.parse::<f64>().ok()
}

fn cex_as_f64(value: &CexValue) -> Option<f64> {
    match value {
        CexValue::Int(value) => Some(*value as f64),
        CexValue::Float(value) => Some(*value),
        CexValue::Bool(_) => None,
    }
}

fn format_cex_value(value: &CexValue) -> String {
    match value {
        CexValue::Int(value) => value.to_string(),
        CexValue::Float(value) => value.to_string(),
        CexValue::Bool(value) => value.to_string(),
    }
}

fn format_eval_value(value: &EvalValue) -> String {
    match value {
        EvalValue::Int(value) => value.to_string(),
        EvalValue::Float(value) => value.to_string(),
        EvalValue::Bool(value) => value.to_string(),
        EvalValue::String(value) => value.clone(),
    }
}

fn collect_expr_symbols(
    expr: &Expr,
    module_env: &ModuleEnv,
    symbols: &mut Vec<SymbolProvenance>,
    seen: &mut HashSet<(String, String)>,
) {
    match expr {
        Expr::Call(name, args) => {
            if let Some(atom) = module_env.get_atom(name) {
                if atom.trust_level == TrustLevel::Trusted {
                    push_symbol(symbols, seen, name, "trusted_atom", Some(atom.span.clone()));
                }
            } else {
                push_symbol(symbols, seen, name, "uninterpreted_function", None);
            }
            for arg in args {
                collect_expr_symbols(arg, module_env, symbols, seen);
            }
        }
        Expr::AtomRef { name } => {
            if let Some(atom) = module_env.get_atom(name) {
                if atom.trust_level == TrustLevel::Trusted {
                    push_symbol(symbols, seen, name, "trusted_atom", Some(atom.span.clone()));
                }
            } else {
                push_symbol(symbols, seen, name, "unexpanded_atom", None);
            }
        }
        Expr::CallRef { callee, args } => {
            push_symbol(symbols, seen, "call_ref", "uninterpreted_function", None);
            collect_expr_symbols(callee, module_env, symbols, seen);
            for arg in args {
                collect_expr_symbols(arg, module_env, symbols, seen);
            }
        }
        Expr::BinaryOp(left, _, right) => {
            collect_expr_symbols(left, module_env, symbols, seen);
            collect_expr_symbols(right, module_env, symbols, seen);
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_expr_symbols(cond, module_env, symbols, seen);
            collect_stmt_symbols(then_branch, module_env, symbols, seen);
            collect_stmt_symbols(else_branch, module_env, symbols, seen);
        }
        Expr::ArrayAccess(_, index) => collect_expr_symbols(index, module_env, symbols, seen),
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_expr_symbols(value, module_env, symbols, seen);
            }
        }
        Expr::FieldAccess(base, _) => collect_expr_symbols(base, module_env, symbols, seen),
        Expr::Match { target, arms } => {
            collect_expr_symbols(target, module_env, symbols, seen);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_expr_symbols(guard, module_env, symbols, seen);
                }
                collect_stmt_symbols(&arm.body, module_env, symbols, seen);
            }
        }
        Expr::Async { body } | Expr::Lambda { body, .. } => {
            collect_stmt_symbols(body, module_env, symbols, seen)
        }
        Expr::Await { expr } => collect_expr_symbols(expr, module_env, symbols, seen),
        Expr::Perform { effect, args, .. } => {
            push_symbol(symbols, seen, effect, "uninterpreted_function", None);
            for arg in args {
                collect_expr_symbols(arg, module_env, symbols, seen);
            }
        }
        Expr::ChanSend { channel, value } => {
            collect_expr_symbols(channel, module_env, symbols, seen);
            collect_expr_symbols(value, module_env, symbols, seen);
        }
        Expr::ChanRecv { channel } => collect_expr_symbols(channel, module_env, symbols, seen),
        Expr::Number(_) | Expr::Float(_) | Expr::StringLit(_) | Expr::Variable(_) => {}
    }
}

fn collect_stmt_symbols(
    stmt: &Stmt,
    module_env: &ModuleEnv,
    symbols: &mut Vec<SymbolProvenance>,
    seen: &mut HashSet<(String, String)>,
) {
    match stmt {
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            collect_expr_symbols(value, module_env, symbols, seen)
        }
        Stmt::ArrayStore { index, value, .. } => {
            collect_expr_symbols(index, module_env, symbols, seen);
            collect_expr_symbols(value, module_env, symbols, seen);
        }
        Stmt::Block(stmts, _)
        | Stmt::TaskGroup {
            children: stmts, ..
        } => {
            for stmt in stmts {
                collect_stmt_symbols(stmt, module_env, symbols, seen);
            }
        }
        Stmt::While {
            cond,
            invariant,
            decreases,
            body,
            ..
        } => {
            collect_expr_symbols(cond, module_env, symbols, seen);
            collect_expr_symbols(invariant, module_env, symbols, seen);
            if let Some(decreases) = decreases {
                collect_expr_symbols(decreases, module_env, symbols, seen);
            }
            collect_stmt_symbols(body, module_env, symbols, seen);
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
            collect_stmt_symbols(body, module_env, symbols, seen)
        }
        Stmt::Expr(expr, _) => collect_expr_symbols(expr, module_env, symbols, seen),
        Stmt::Cancel { .. } => {}
    }
}

fn push_symbol(
    symbols: &mut Vec<SymbolProvenance>,
    seen: &mut HashSet<(String, String)>,
    symbol_name: &str,
    source: &str,
    location: Option<Span>,
) {
    let key = (symbol_name.to_string(), source.to_string());
    if seen.insert(key) {
        symbols.push(SymbolProvenance {
            symbol_name: symbol_name.to_string(),
            source: source.to_string(),
            location,
        });
    }
}

fn collect_unvalidated_constraints(atom: &Atom) -> Vec<String> {
    let mut constraints = Vec::new();
    if !atom.requires.trim().is_empty() && atom.requires.trim() != "true" {
        constraints.push(format!("requires: {}", atom.requires));
    }
    if !atom.ensures.trim().is_empty() && atom.ensures.trim() != "true" {
        constraints.push(format!("ensures: {}", atom.ensures));
    }
    for (effect, state) in &atom.effect_pre {
        constraints.push(format!("effect_pre: {}={}", effect, state));
    }
    for (effect, state) in &atom.effect_post {
        constraints.push(format!("effect_post: {}={}", effect, state));
    }
    constraints
}

fn normalize_core_label(label: &str) -> String {
    label
        .strip_prefix('|')
        .and_then(|without_prefix| without_prefix.strip_suffix('|'))
        .unwrap_or(label)
        .to_string()
}

fn core_contains_clause(core: &HashSet<String>, exact: &str, prefix: &str) -> bool {
    core.contains(exact) || core.iter().any(|entry| entry.contains(prefix))
}
