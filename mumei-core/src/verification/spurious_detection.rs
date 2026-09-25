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

/// Value produced by replaying a body under concrete counterexample
/// inputs. `Lambda` is the closure a `let f = |…| …` binding produces:
/// its param names and body plus the env captured at the binding site,
/// so `f(args)` can be replayed just like the Z3 translator inlines it.
#[derive(Debug, Clone)]
enum EvalValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Lambda {
        params: Vec<String>,
        body: Box<Stmt>,
        env: EvalEnv,
    },
}

impl PartialEq for EvalValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (EvalValue::Int(a), EvalValue::Int(b)) => a == b,
            (EvalValue::Float(a), EvalValue::Float(b)) => a == b,
            (EvalValue::Bool(a), EvalValue::Bool(b)) => a == b,
            (EvalValue::String(a), EvalValue::String(b)) => a == b,
            // Two closures are never proven equal for replay purposes.
            _ => false,
        }
    }
}

/// A concrete counterexample value extracted from a Z3 model.
///
/// Extends the historical integer-only model representation so that `f64`
/// counterexamples (encoded as Z3 `Real` rationals or IEEE 754 `Float`s) can be
/// replayed under Mumei semantics. `Bool` keeps `result`-style boolean models
/// exact instead of round-tripping through `0`/`1`.
#[derive(Debug, Clone, PartialEq)]
pub enum CexValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
}

impl CexValue {
    fn to_eval(&self) -> EvalValue {
        match self {
            CexValue::Int(value) => EvalValue::Int(*value),
            CexValue::Float(value) => EvalValue::Float(*value),
            CexValue::Bool(value) => EvalValue::Bool(*value),
            CexValue::Str(value) => EvalValue::String(value.clone()),
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
        Ok(
            result @ (EvalValue::Int(_)
            | EvalValue::Float(_)
            | EvalValue::Bool(_)
            | EvalValue::String(_)),
        ) => {
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
        Ok(EvalValue::Lambda { .. }) => {
            return invalid_counterexample_result(
                atom,
                symbol_provenance,
                false,
                "lambda result is not replayable in a counterexample model".to_string(),
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

    let body = parse_body_expr(&atom.body_expr);
    // `let f = |…| …` binds a real body — `f(…)`/`call(f, …)` is
    // interpreted, so the name must not be flagged as uninterpreted.
    let lambda_names = collect_lambda_binding_names(&body);
    for expr in [
        parse_expression(&atom.requires),
        parse_expression(&atom.ensures),
    ] {
        collect_expr_symbols(&expr, module_env, &mut symbols, &mut seen, &lambda_names);
    }
    collect_stmt_symbols(&body, module_env, &mut symbols, &mut seen, &lambda_names);

    symbols
}

/// Names `let`/`assign`-bound to a lambda literal (`let f = |…| …`) or
/// aliased from one (`let g = f`), walked in binding order. Used to keep
/// indirect calls out of the uninterpreted-symbol report — the call has
/// a real body behind it.
fn collect_lambda_binding_names(stmt: &Stmt) -> HashSet<String> {
    let mut names = HashSet::new();
    collect_lambda_binding_names_stmt(stmt, &mut names);
    names
}

fn collect_lambda_binding_names_stmt(stmt: &Stmt, out: &mut HashSet<String>) {
    match stmt {
        Stmt::Let { var, value, .. } | Stmt::Assign { var, value, .. } => {
            if expr_resolves_to_lambda(value, out) {
                out.insert(var.clone());
            } else {
                // Rebinding to a non-lambda clears the binding — a
                // `f(…)` issued against the new value is uninterpreted.
                out.remove(var);
            }
            collect_lambda_binding_names_expr(value, out);
        }
        Stmt::Block(stmts, _)
        | Stmt::TaskGroup {
            children: stmts, ..
        } => {
            for stmt in stmts {
                collect_lambda_binding_names_stmt(stmt, out);
            }
        }
        Stmt::Expr(expr, _) => collect_lambda_binding_names_expr(expr, out),
        Stmt::ArrayStore { index, value, .. } => {
            collect_lambda_binding_names_expr(index, out);
            collect_lambda_binding_names_expr(value, out);
        }
        Stmt::While {
            cond,
            invariant,
            decreases,
            body,
            ..
        } => {
            collect_lambda_binding_names_expr(cond, out);
            collect_lambda_binding_names_expr(invariant, out);
            if let Some(decreases) = decreases {
                collect_lambda_binding_names_expr(decreases, out);
            }
            collect_lambda_binding_names_stmt(body, out);
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
            collect_lambda_binding_names_stmt(body, out)
        }
        Stmt::Cancel { .. } => {}
    }
}

/// Whether a `let`/`assign` right-hand side resolves to a lambda binding:
/// a literal, an alias of a known lambda name, or a conditional
/// (`if`/`match`) whose every leaf tail does. Mirrors the verifier's
/// `resolve_lambda_expr` flatness gate so `let h = if … {f} else {g}`
/// and `let m = match t { 1 => f, _ => g }` keep `h`/`m` out of the
/// uninterpreted-symbol report.
fn expr_resolves_to_lambda(expr: &Expr, out: &HashSet<String>) -> bool {
    match expr {
        Expr::Lambda { .. } => true,
        Expr::Variable(src) => out.contains(src),
        Expr::IfThenElse {
            then_branch,
            else_branch,
            ..
        } => {
            stmt_tail_resolves_to_lambda(then_branch, out)
                && stmt_tail_resolves_to_lambda(else_branch, out)
        }
        Expr::Match { arms, .. } => {
            !arms.is_empty()
                && arms.iter().all(|arm| {
                    arm.guard.is_none()
                        && matches!(
                            arm.pattern,
                            crate::parser::Pattern::Wildcard
                                | crate::parser::Pattern::Variable(_)
                                | crate::parser::Pattern::Literal(_)
                        )
                        && stmt_tail_resolves_to_lambda(&arm.body, out)
                })
        }
        _ => false,
    }
}

fn stmt_tail_resolves_to_lambda(stmt: &Stmt, out: &HashSet<String>) -> bool {
    match stmt {
        Stmt::Expr(e, _) => expr_resolves_to_lambda(e, out),
        Stmt::Block(stmts, _) => stmts
            .last()
            .is_some_and(|s| stmt_tail_resolves_to_lambda(s, out)),
        _ => false,
    }
}

fn collect_lambda_binding_names_expr(expr: &Expr, out: &mut HashSet<String>) {
    match expr {
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_lambda_binding_names_expr(cond, out);
            collect_lambda_binding_names_stmt(then_branch, out);
            collect_lambda_binding_names_stmt(else_branch, out);
        }
        Expr::Match { target, arms } => {
            collect_lambda_binding_names_expr(target, out);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_lambda_binding_names_expr(guard, out);
                }
                collect_lambda_binding_names_stmt(&arm.body, out);
            }
        }
        Expr::Async { body } | Expr::Lambda { body, .. } => {
            collect_lambda_binding_names_stmt(body, out)
        }
        Expr::Await { expr } | Expr::FieldAccess(expr, _) | Expr::ChanRecv { channel: expr } => {
            collect_lambda_binding_names_expr(expr, out)
        }
        Expr::BinaryOp(l, _, r) => {
            collect_lambda_binding_names_expr(l, out);
            collect_lambda_binding_names_expr(r, out);
        }
        Expr::Call(_, args) | Expr::Perform { args, .. } => {
            for arg in args {
                collect_lambda_binding_names_expr(arg, out);
            }
        }
        Expr::CallRef { callee, args } => {
            collect_lambda_binding_names_expr(callee, out);
            for arg in args {
                collect_lambda_binding_names_expr(arg, out);
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_lambda_binding_names_expr(value, out);
            }
        }
        Expr::ArrayLit(elements) => {
            for element in elements {
                collect_lambda_binding_names_expr(element, out);
            }
        }
        Expr::ArrayAccess(_, index) => collect_lambda_binding_names_expr(index, out),
        Expr::ChanSend { channel, value } => {
            collect_lambda_binding_names_expr(channel, out);
            collect_lambda_binding_names_expr(value, out);
        }
        _ => {}
    }
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
        EvalValue::String(_) | EvalValue::Lambda { .. } => {
            Err("non-scalar clause cannot be evaluated as bool".to_string())
        }
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
                scalar @ (EvalValue::Int(_)
                | EvalValue::Float(_)
                | EvalValue::Bool(_)
                | EvalValue::String(_)
                | EvalValue::Lambda { .. }) => {
                    env.insert(var.clone(), scalar.clone());
                    Ok(scalar)
                }
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
        Expr::ArrayLit(_) => Err("array literal has no scalar value".to_string()),
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
            EvalValue::String(_) | EvalValue::Lambda { .. } => {
                Err("if condition is not boolean".to_string())
            }
        },
        Expr::Call(name, args) => {
            // These names are always handled as string builtins by the Z3
            // translator, even when a user atom or lambda has the same name.
            if matches!(
                name.as_str(),
                "len" | "starts_with" | "ends_with" | "contains" | "not_contains"
            ) {
                return eval_string_builtin(name, args, env, module_env, depth + 1)
                    .expect("known string builtin");
            }
            // `let f = |…| …; f(args)` — the call replays by binding the
            // params to the concrete args inside the lambda's captured env,
            // mirroring the translator's apply_local_lambda.
            let lambda = match env.get(name) {
                Some(lambda @ EvalValue::Lambda { .. }) => Some(lambda.clone()),
                _ => None,
            };
            match lambda {
                Some(lambda) => eval_lambda_call(name, &lambda, args, env, module_env, depth + 1),
                None if !user_defines_callee(module_env, name, env) => {
                    if let Some(result) =
                        eval_string_builtin(name, args, env, module_env, depth + 1)
                    {
                        return result;
                    }
                    eval_atom_call(name, args, env, module_env, depth + 1)
                }
                None => eval_atom_call(name, args, env, module_env, depth + 1),
            }
        }
        Expr::Lambda { params, body, .. } => Ok(EvalValue::Lambda {
            params: params.iter().map(|p| p.name.clone()).collect(),
            body: body.clone(),
            env: env.clone(),
        }),
        // `call(f, …)` / `call(|a| …, …)` — replay a lambda-valued callee.
        Expr::CallRef { callee, args } => match eval_expr(callee, env, module_env, depth + 1)? {
            lambda @ EvalValue::Lambda { .. } => {
                eval_lambda_call("call", &lambda, args, env, module_env, depth + 1)
            }
            _ => Err("call_ref callee is not a replayable lambda".to_string()),
        },
        // `match t { 1 => f, _ => g }` — replay first-match order over the
        // concrete scrutinee; the arm value can be a lambda (selector
        // bindings) or any scalar. Pattern forms the replay can't decide
        // (Variant tag machinery) stay unevaluable.
        Expr::Match { target, arms } => {
            let target_value = eval_expr(target, env, module_env, depth + 1)?;
            for arm in arms {
                let (matched, bound) = match &arm.pattern {
                    crate::parser::Pattern::Wildcard => (true, None),
                    crate::parser::Pattern::Variable(name) => (true, Some(name.clone())),
                    crate::parser::Pattern::Literal(n) => match &target_value {
                        EvalValue::Int(v) => (v == n, None),
                        _ => (false, None),
                    },
                    _ => {
                        return Err(
                            "match pattern is not evaluable in counterexample replay".to_string()
                        )
                    }
                };
                if !matched {
                    continue;
                }
                if let Some(guard) = &arm.guard {
                    match eval_expr(guard, env, module_env, depth + 1) {
                        Ok(EvalValue::Bool(true)) => {}
                        Ok(EvalValue::Bool(false)) => continue,
                        _ => {
                            return Err(
                                "match guard is not evaluable in counterexample replay".to_string()
                            )
                        }
                    }
                }
                if let Some(name) = bound {
                    // Pattern bindings are arm-local: restore the prior
                    // binding (or absence) so the name doesn't leak into the
                    // post-match replay env.
                    let prior = env.insert(name.clone(), target_value.clone());
                    let result = eval_stmt(&arm.body, env, module_env, depth + 1);
                    match prior {
                        Some(old) => {
                            env.insert(name, old);
                        }
                        None => {
                            env.remove(&name);
                        }
                    }
                    return result;
                }
                return eval_stmt(&arm.body, env, module_env, depth + 1);
            }
            Err("non-exhaustive match in counterexample replay".to_string())
        }
        Expr::ArrayAccess(_, _)
        | Expr::StructInit { .. }
        | Expr::FieldAccess(_, _)
        | Expr::Async { .. }
        | Expr::Await { .. }
        | Expr::AtomRef { .. }
        | Expr::Perform { .. }
        | Expr::ChanSend { .. }
        | Expr::ChanRecv { .. } => {
            Err("expression form is not evaluable in counterexample replay".to_string())
        }
    }
}

/// Replay `lambda(args)` concretely: params bind inside the closure's
/// captured env, then the body evaluates under it. Keeps counterexample
/// validation honest for `f(3)`-style indirect calls.
fn eval_lambda_call(
    name: &str,
    lambda: &EvalValue,
    args: &[Expr],
    env: &mut EvalEnv,
    module_env: &ModuleEnv,
    depth: usize,
) -> Result<EvalValue, String> {
    let EvalValue::Lambda {
        params,
        body,
        env: captured,
    } = lambda
    else {
        return Err(format!("'{name}' is not a replayable lambda"));
    };
    if params.len() != args.len() {
        return Err(format!("arity mismatch for lambda '{name}'"));
    }
    let mut call_env = captured.clone();
    for (param, arg) in params.iter().zip(args) {
        let value = eval_expr(arg, env, module_env, depth)?;
        match value {
            scalar @ (EvalValue::Int(_)
            | EvalValue::Float(_)
            | EvalValue::Bool(_)
            | EvalValue::String(_)
            | EvalValue::Lambda { .. }) => {
                call_env.insert(param.clone(), scalar);
            }
        }
    }
    eval_stmt(body, &mut call_env, module_env, depth)
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
            value @ (EvalValue::Int(_)
            | EvalValue::Float(_)
            | EvalValue::Bool(_)
            | EvalValue::String(_)
            | EvalValue::Lambda { .. }) => {
                call_env.insert(param.name.clone(), value);
            }
        }
    }

    if !eval_bool_clause(&callee.requires, &mut call_env, module_env)? {
        return Err(format!("callee '{}' requires clause is false", name));
    }
    let body = parse_body_expr(&callee.body_expr);
    let result = eval_stmt(&body, &mut call_env, module_env, depth)?;
    match &result {
        EvalValue::Int(_)
        | EvalValue::Float(_)
        | EvalValue::Bool(_)
        | EvalValue::String(_)
        | EvalValue::Lambda { .. } => {
            call_env.insert("result".to_string(), result.clone());
        }
    }
    if !eval_bool_clause(&callee.ensures, &mut call_env, module_env)? {
        return Err(format!("callee '{}' ensures clause is false", name));
    }
    Ok(result)
}

fn user_defines_callee(module_env: &ModuleEnv, name: &str, env: &EvalEnv) -> bool {
    matches!(env.get(name), Some(EvalValue::Lambda { .. }))
        || lookup_call_atom(module_env, name).is_some()
}

fn eval_string_builtin(
    name: &str,
    args: &[Expr],
    env: &mut EvalEnv,
    module_env: &ModuleEnv,
    depth: usize,
) -> Option<Result<EvalValue, String>> {
    let arity = match name {
        "len" | "is_empty" => 1,
        "starts_with" | "ends_with" | "contains" | "not_contains" | "index_of" => 2,
        "substr" => 3,
        "char_at" => 2,
        _ => return None,
    };
    if args.len() != arity {
        return Some(Err(format!("{name}() expects {arity} arguments")));
    }
    let values = match args
        .iter()
        .map(|arg| eval_expr(arg, env, module_env, depth))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(values) => values,
        Err(err) => return Some(Err(err)),
    };
    let string = |value: &EvalValue| match value {
        EvalValue::String(value) => Ok(value.clone()),
        _ => Err(format!("{name}() expects Str arguments")),
    };
    let result = match name {
        "len" => string(&values[0]).map(|value| EvalValue::Int(value.chars().count() as i64)),
        "is_empty" => string(&values[0]).map(|value| EvalValue::Bool(value.is_empty())),
        "starts_with" => string(&values[0]).and_then(|value| {
            string(&values[1]).map(|pat| EvalValue::Bool(value.starts_with(&pat)))
        }),
        "ends_with" => string(&values[0])
            .and_then(|value| string(&values[1]).map(|pat| EvalValue::Bool(value.ends_with(&pat)))),
        "contains" => string(&values[0])
            .and_then(|value| string(&values[1]).map(|pat| EvalValue::Bool(value.contains(&pat)))),
        "not_contains" => string(&values[0])
            .and_then(|value| string(&values[1]).map(|pat| EvalValue::Bool(!value.contains(&pat)))),
        "index_of" => string(&values[0]).and_then(|value| {
            string(&values[1]).map(|pat| {
                let index = value
                    .find(&pat)
                    .map(|byte_index| value[..byte_index].chars().count() as i64)
                    .unwrap_or(-1);
                EvalValue::Int(index)
            })
        }),
        "substr" => {
            let value = string(&values[0]);
            let start = match &values[1] {
                EvalValue::Int(value) => *value,
                _ => return Some(Err("substr() expects integer start/count".to_string())),
            };
            let count = match &values[2] {
                EvalValue::Int(value) => *value,
                _ => return Some(Err("substr() expects integer start/count".to_string())),
            };
            value.map(|value| {
                let chars: Vec<char> = value.chars().collect();
                let start = usize::try_from(start)
                    .unwrap_or(chars.len())
                    .min(chars.len());
                let count = usize::try_from(count).unwrap_or(0);
                EvalValue::String(chars.into_iter().skip(start).take(count).collect())
            })
        }
        "char_at" => {
            let value = string(&values[0]);
            let index = match &values[1] {
                EvalValue::Int(value) => *value,
                _ => return Some(Err("char_at() expects an integer index".to_string())),
            };
            value.map(|value| {
                let character = usize::try_from(index)
                    .ok()
                    .and_then(|index| value.chars().nth(index))
                    .map(|character| character.to_string())
                    .unwrap_or_default();
                EvalValue::String(character)
            })
        }
        _ => unreachable!(),
    };
    Some(result)
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
                // Bit semantics on the two's complement `i64` bit pattern,
                // matching the `BV(64)` encoding used under `--bitvec-i64`.
                Op::BitAnd => Ok(EvalValue::Int(left & right)),
                Op::BitOr => Ok(EvalValue::Int(left | right)),
                Op::BitXor => Ok(EvalValue::Int(left ^ right)),
                Op::Shl if (0..64).contains(&right) => {
                    Ok(EvalValue::Int(((left as u64) << right) as i64))
                }
                Op::Shr if (0..64).contains(&right) => Ok(EvalValue::Int(left >> right)),
                Op::Shl | Op::Shr => {
                    Err("shift amount outside 0..64 during counterexample replay".to_string())
                }
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
                    Op::BitAnd | Op::BitOr | Op::BitXor | Op::Shl | Op::Shr => Err(
                        "bitwise operator applied to f64 during counterexample replay".to_string(),
                    ),
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
        EvalValue::Bool(_) | EvalValue::String(_) | EvalValue::Lambda { .. } => None,
    }
}

fn value_as_bool(value: &EvalValue) -> Option<bool> {
    match value {
        EvalValue::Bool(value) => Some(*value),
        EvalValue::Int(value) => Some(*value != 0),
        EvalValue::Float(value) => Some(*value != 0.0),
        EvalValue::String(_) | EvalValue::Lambda { .. } => None,
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
        (CexValue::Str(m), EvalValue::String(b)) => m == b,
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
        CexValue::Bool(_) | CexValue::Str(_) => None,
    }
}

fn format_cex_value(value: &CexValue) -> String {
    match value {
        CexValue::Int(value) => value.to_string(),
        CexValue::Float(value) => value.to_string(),
        CexValue::Bool(value) => value.to_string(),
        CexValue::Str(value) => value.clone(),
    }
}

fn format_eval_value(value: &EvalValue) -> String {
    match value {
        EvalValue::Int(value) => value.to_string(),
        EvalValue::Float(value) => value.to_string(),
        EvalValue::Bool(value) => value.to_string(),
        EvalValue::String(value) => value.clone(),
        EvalValue::Lambda { .. } => "<lambda>".to_string(),
    }
}

/// Calls the Z3 translator lowers to interpreted constraints instead of an
/// uninterpreted function symbol — keep them out of the spurious-dependency
/// report so genuine counterexamples aren't mislabeled.
fn is_translated_builtin(name: &str) -> bool {
    matches!(
        name,
        "forall"
            | "exists"
            | "len"
            | "sqrt"
            | "cast_to_int"
            | "matches"
            | "match_regex"
            | "re_match"
            | "starts_with"
            | "ends_with"
            | "contains"
            | "not_contains"
            | "is_empty"
            | "index_of"
            | "substr"
            | "char_at"
    )
}

fn lookup_call_atom<'a>(module_env: &'a ModuleEnv, name: &str) -> Option<&'a Atom> {
    module_env.get_atom(name).or_else(|| {
        let fqn_name = name.replace('.', "::");
        (fqn_name != name)
            .then(|| module_env.get_atom(&fqn_name))
            .flatten()
    })
}

fn collect_expr_symbols(
    expr: &Expr,
    module_env: &ModuleEnv,
    symbols: &mut Vec<SymbolProvenance>,
    seen: &mut HashSet<(String, String)>,
    lambda_names: &HashSet<String>,
) {
    match expr {
        Expr::Call(name, args) => {
            if let Some(atom) = lookup_call_atom(module_env, name) {
                if atom.trust_level == TrustLevel::Trusted {
                    push_symbol(symbols, seen, name, "trusted_atom", Some(atom.span.clone()));
                }
            } else if !is_translated_builtin(name) && !lambda_names.contains(name) {
                // Builtin calls the Z3 translator lowers directly (len, forall,
                // string predicates, …) are interpreted, not uninterpreted —
                // flagging them mislabels a genuine counterexample as spurious.
                // The same holds for `let f = |…| …` callees: `f(…)` applies
                // the bound lambda body.
                push_symbol(symbols, seen, name, "uninterpreted_function", None);
            }
            for arg in args {
                collect_expr_symbols(arg, module_env, symbols, seen, lambda_names);
            }
        }
        Expr::ArrayLit(elements) => {
            for element in elements {
                collect_expr_symbols(element, module_env, symbols, seen, lambda_names);
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
            let lambda_callee = match callee.as_ref() {
                Expr::Variable(var) => lambda_names.contains(var),
                Expr::Lambda { .. } => true,
                _ => false,
            };
            if !lambda_callee {
                push_symbol(symbols, seen, "call_ref", "uninterpreted_function", None);
            }
            collect_expr_symbols(callee, module_env, symbols, seen, lambda_names);
            for arg in args {
                collect_expr_symbols(arg, module_env, symbols, seen, lambda_names);
            }
        }
        Expr::BinaryOp(left, _, right) => {
            collect_expr_symbols(left, module_env, symbols, seen, lambda_names);
            collect_expr_symbols(right, module_env, symbols, seen, lambda_names);
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_expr_symbols(cond, module_env, symbols, seen, lambda_names);
            collect_stmt_symbols(then_branch, module_env, symbols, seen, lambda_names);
            collect_stmt_symbols(else_branch, module_env, symbols, seen, lambda_names);
        }
        Expr::ArrayAccess(_, index) => {
            collect_expr_symbols(index, module_env, symbols, seen, lambda_names)
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_expr_symbols(value, module_env, symbols, seen, lambda_names);
            }
        }
        Expr::FieldAccess(base, _) => {
            collect_expr_symbols(base, module_env, symbols, seen, lambda_names)
        }
        Expr::Match { target, arms } => {
            collect_expr_symbols(target, module_env, symbols, seen, lambda_names);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_expr_symbols(guard, module_env, symbols, seen, lambda_names);
                }
                collect_stmt_symbols(&arm.body, module_env, symbols, seen, lambda_names);
            }
        }
        Expr::Async { body } | Expr::Lambda { body, .. } => {
            collect_stmt_symbols(body, module_env, symbols, seen, lambda_names)
        }
        Expr::Await { expr } => collect_expr_symbols(expr, module_env, symbols, seen, lambda_names),
        Expr::Perform { effect, args, .. } => {
            push_symbol(symbols, seen, effect, "uninterpreted_function", None);
            for arg in args {
                collect_expr_symbols(arg, module_env, symbols, seen, lambda_names);
            }
        }
        Expr::ChanSend { channel, value } => {
            collect_expr_symbols(channel, module_env, symbols, seen, lambda_names);
            collect_expr_symbols(value, module_env, symbols, seen, lambda_names);
        }
        Expr::ChanRecv { channel } => {
            collect_expr_symbols(channel, module_env, symbols, seen, lambda_names)
        }
        Expr::Number(_) | Expr::Float(_) | Expr::StringLit(_) | Expr::Variable(_) => {}
    }
}

fn collect_stmt_symbols(
    stmt: &Stmt,
    module_env: &ModuleEnv,
    symbols: &mut Vec<SymbolProvenance>,
    seen: &mut HashSet<(String, String)>,
    lambda_names: &HashSet<String>,
) {
    match stmt {
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            collect_expr_symbols(value, module_env, symbols, seen, lambda_names)
        }
        Stmt::ArrayStore { index, value, .. } => {
            collect_expr_symbols(index, module_env, symbols, seen, lambda_names);
            collect_expr_symbols(value, module_env, symbols, seen, lambda_names);
        }
        Stmt::Block(stmts, _)
        | Stmt::TaskGroup {
            children: stmts, ..
        } => {
            for stmt in stmts {
                collect_stmt_symbols(stmt, module_env, symbols, seen, lambda_names);
            }
        }
        Stmt::While {
            cond,
            invariant,
            decreases,
            body,
            ..
        } => {
            collect_expr_symbols(cond, module_env, symbols, seen, lambda_names);
            collect_expr_symbols(invariant, module_env, symbols, seen, lambda_names);
            if let Some(decreases) = decreases {
                collect_expr_symbols(decreases, module_env, symbols, seen, lambda_names);
            }
            collect_stmt_symbols(body, module_env, symbols, seen, lambda_names);
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
            collect_stmt_symbols(body, module_env, symbols, seen, lambda_names)
        }
        Stmt::Expr(expr, _) => collect_expr_symbols(expr, module_env, symbols, seen, lambda_names),
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
