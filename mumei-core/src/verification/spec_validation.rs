use super::module_env::ModuleEnv;
use super::property_based::{
    run_property_based_test, PropertyBasedTestConfig, PropertyBasedTestResult,
};
use super::translator::{
    apply_refinement_constraint, expr_to_z3, param_z3_value, VCtx, DEFAULT_CONSTRAINT_BUDGET,
};
use super::types::Env;
use super::SpecContradiction;
use super::{parse_expression, Atom, Bool, Config, Dynamic, HashMap, Int, SatResult, Solver};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecValidationResult {
    pub status: String,
    #[serde(default = "default_is_satisfiable")]
    pub is_satisfiable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contradiction_details: Option<String>,
    pub trace_id: Option<String>,
    pub spec_metadata: HashMap<String, String>,
    pub traceability_hash: String,
    pub traceability_coverage: f64,
    pub checked_requires: bool,
    pub checked_ensures: usize,
    pub checked_refinements: usize,
    pub ensures_implication_checks: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub property_based_test: Option<PropertyBasedTestResult>,
    pub diagnostics: Vec<String>,
}

fn default_is_satisfiable() -> bool {
    true
}

impl SpecValidationResult {
    pub fn from_contradiction(atom: &Atom, contradiction: &SpecContradiction) -> Self {
        let trace_id = effective_trace_id(atom);
        let spec_metadata = effective_spec_metadata(atom);
        let contradiction_details = format!(
            "{}: {} (constraints: {:?})",
            contradiction.kind, contradiction.message, contradiction.constraints
        );
        Self {
            status: "unsatisfiable".to_string(),
            is_satisfiable: false,
            contradiction_details: Some(contradiction_details.clone()),
            trace_id: trace_id.clone(),
            spec_metadata: spec_metadata.clone(),
            traceability_hash: calculate_traceability_hash(atom),
            traceability_coverage: traceability_coverage(atom, trace_id.as_ref(), &spec_metadata),
            checked_requires: !contradiction.kind.starts_with("refinement_"),
            checked_ensures: 0,
            checked_refinements: 0,
            ensures_implication_checks: 0,
            property_based_test: None,
            diagnostics: vec![
                contradiction_details,
                contradiction.natural_language_explanation.clone(),
                format!("Suggested fix: {}", contradiction.suggested_fix),
            ],
        }
    }
}

pub fn calculate_traceability_hash(atom: &Atom) -> String {
    let trace_id = effective_trace_id(atom);
    let spec_metadata = effective_spec_metadata(atom);
    let mut hasher = Sha256::new();
    hasher.update(trace_id.as_deref().unwrap_or("").as_bytes());

    let mut metadata: Vec<(&String, &String)> = spec_metadata.iter().collect();
    metadata.sort_by_key(|(key, _)| *key);
    for (key, value) in metadata {
        hasher.update(key.as_bytes());
        hasher.update(b"=");
        hasher.update(value.as_bytes());
        hasher.update(b";");
    }

    hasher.update(atom.requires.as_bytes());
    hasher.update(atom.ensures.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn check_spec_satisfiability(
    atom: &Atom,
    module_env: &ModuleEnv,
) -> Result<SpecValidationResult, SpecContradiction> {
    check_spec_satisfiability_with_property_based(atom, module_env, None, false)
}

pub fn check_spec_satisfiability_with_property_based(
    atom: &Atom,
    module_env: &ModuleEnv,
    property_based_config: Option<&PropertyBasedTestConfig>,
    ieee754_f64: bool,
) -> Result<SpecValidationResult, SpecContradiction> {
    let mut diagnostics = Vec::new();
    let checked_refinements = check_standalone_refinements(atom, module_env)?;

    let mut cfg = Config::new();
    cfg.set_timeout_msec(5000);
    let ctx = super::Context::new(&cfg);
    let solver = Solver::new(&ctx);
    let vc = validation_ctx(&ctx, module_env, atom, ieee754_f64);
    let mut env = seed_env(&ctx, atom, module_env, ieee754_f64);
    let mut had_clause_skips = false;
    assert_parameter_refinements(&vc, &solver, atom, module_env, &mut env)?;
    let checked_requires =
        match assert_clause(&vc, &solver, &mut env, atom, &atom.requires, "requires")? {
            ClauseLoweringOutcome::Applied => true,
            ClauseLoweringOutcome::Skipped(warning) => {
                had_clause_skips = true;
                push_skip_warning(&mut diagnostics, warning);
                false
            }
        };

    if solver.check() == SatResult::Unsat {
        return Err(SpecContradiction::new(
            &atom.name,
            "requires_unsat",
            "requires clause is unsatisfiable before proof attempt",
            vec![atom.requires.clone()],
            atom.span.clone(),
        ));
    }

    if super::detect_logic_fragment_tags(atom, module_env)
        .iter()
        .any(|tag| tag == "finite_field")
    {
        let trace_id = effective_trace_id(atom);
        let spec_metadata = effective_spec_metadata(atom);
        return Ok(SpecValidationResult {
            status: "unknown_fragment".to_string(),
            is_satisfiable: true,
            contradiction_details: None,
            trace_id: trace_id.clone(),
            spec_metadata: spec_metadata.clone(),
            traceability_hash: calculate_traceability_hash(atom),
            traceability_coverage: traceability_coverage(atom, trace_id.as_ref(), &spec_metadata),
            checked_requires: true,
            checked_ensures: 0,
            checked_refinements,
            ensures_implication_checks: 0,
            property_based_test: None,
            diagnostics: vec![
                "finite_field helpers are checked by the Lean bridge after Z3 unknown routing"
                    .to_string(),
            ],
        });
    }

    let ensure_clauses = split_top_level_conjunctions(&atom.ensures);
    let mut checked_ensures = 0usize;
    for (index, clause) in ensure_clauses.iter().enumerate() {
        let local_solver = Solver::new(&ctx);
        let mut local_env = seed_env(&ctx, atom, module_env, ieee754_f64);
        assert_parameter_refinements(&vc, &local_solver, atom, module_env, &mut local_env)?;
        if let ClauseLoweringOutcome::Skipped(warning) = assert_clause(
            &vc,
            &local_solver,
            &mut local_env,
            atom,
            &atom.requires,
            "requires",
        )? {
            had_clause_skips = true;
            push_skip_warning(&mut diagnostics, warning);
        }
        match assert_clause(&vc, &local_solver, &mut local_env, atom, clause, "ensures")? {
            ClauseLoweringOutcome::Applied => {
                checked_ensures += 1;
            }
            ClauseLoweringOutcome::Skipped(warning) => {
                had_clause_skips = true;
                push_skip_warning(&mut diagnostics, warning);
            }
        }
        if local_solver.check() == SatResult::Unsat {
            return Err(SpecContradiction::new(
                &atom.name,
                "ensures_unsat",
                format!("ensures clause {} is inconsistent with requires", index + 1),
                vec![atom.requires.clone(), clause.clone()],
                atom.span.clone(),
            ));
        }
    }

    if !ensure_clauses.is_empty() {
        let combined_solver = Solver::new(&ctx);
        let mut combined_env = seed_env(&ctx, atom, module_env, ieee754_f64);
        assert_parameter_refinements(&vc, &combined_solver, atom, module_env, &mut combined_env)?;
        if let ClauseLoweringOutcome::Skipped(warning) = assert_clause(
            &vc,
            &combined_solver,
            &mut combined_env,
            atom,
            &atom.requires,
            "requires",
        )? {
            had_clause_skips = true;
            push_skip_warning(&mut diagnostics, warning);
        }
        for clause in &ensure_clauses {
            if let ClauseLoweringOutcome::Skipped(warning) = assert_clause(
                &vc,
                &combined_solver,
                &mut combined_env,
                atom,
                clause,
                "ensures",
            )? {
                had_clause_skips = true;
                push_skip_warning(&mut diagnostics, warning);
            }
        }
        if combined_solver.check() == SatResult::Unsat {
            let mut constraints = Vec::with_capacity(ensure_clauses.len() + 1);
            constraints.push(atom.requires.clone());
            constraints.extend(ensure_clauses.clone());
            return Err(SpecContradiction::new(
                &atom.name,
                "ensures_conflict",
                "ensures clauses are mutually inconsistent under requires",
                constraints,
                atom.span.clone(),
            ));
        }
    }

    let mut implication_checks = 0usize;
    for (left_index, left) in ensure_clauses.iter().enumerate() {
        for (right_index, right) in ensure_clauses.iter().enumerate() {
            if left_index == right_index {
                continue;
            }
            let local_solver = Solver::new(&ctx);
            let mut local_env = seed_env(&ctx, atom, module_env, ieee754_f64);
            assert_parameter_refinements(&vc, &local_solver, atom, module_env, &mut local_env)?;
            if let ClauseLoweringOutcome::Skipped(warning) = assert_clause(
                &vc,
                &local_solver,
                &mut local_env,
                atom,
                &atom.requires,
                "requires",
            )? {
                had_clause_skips = true;
                push_skip_warning(&mut diagnostics, warning);
                continue;
            }
            if let ClauseLoweringOutcome::Skipped(warning) =
                assert_clause(&vc, &local_solver, &mut local_env, atom, left, "ensures")?
            {
                had_clause_skips = true;
                push_skip_warning(&mut diagnostics, warning);
                continue;
            }
            if let ClauseLoweringOutcome::Skipped(warning) =
                assert_negated_clause(&vc, &local_solver, &mut local_env, atom, right, "ensures")?
            {
                had_clause_skips = true;
                push_skip_warning(&mut diagnostics, warning);
                continue;
            }
            implication_checks += 1;
            if local_solver.check() == SatResult::Unsat {
                diagnostics.push(format!(
                    "ensures clause {} implies clause {} under requires",
                    left_index + 1,
                    right_index + 1
                ));
            }
        }
    }

    let trace_id = effective_trace_id(atom);
    let spec_metadata = effective_spec_metadata(atom);

    let property_based_test = property_based_config.map(|config| {
        let result = run_property_based_test(atom, module_env, config);
        diagnostics.extend(
            result
                .diagnostics
                .iter()
                .map(|diagnostic| format!("property-based: {diagnostic}")),
        );
        result
    });

    Ok(SpecValidationResult {
        status: if had_clause_skips {
            "satisfiable_with_skips".to_string()
        } else {
            "satisfiable".to_string()
        },
        is_satisfiable: true,
        contradiction_details: None,
        trace_id: trace_id.clone(),
        spec_metadata: spec_metadata.clone(),
        traceability_hash: calculate_traceability_hash(atom),
        traceability_coverage: traceability_coverage(atom, trace_id.as_ref(), &spec_metadata),
        checked_requires,
        checked_ensures,
        checked_refinements,
        ensures_implication_checks: implication_checks,
        property_based_test,
        diagnostics,
    })
}

fn validation_ctx<'a>(
    ctx: &'a super::Context,
    module_env: &'a ModuleEnv,
    atom: &'a Atom,
    ieee754_f64: bool,
) -> VCtx<'a> {
    VCtx {
        ctx,
        module_env,
        current_atom: Some(atom),
        linearity_ctx: None,
        effect_ctx: None,
        constraint_count: None,
        constraint_budget: DEFAULT_CONSTRAINT_BUDGET,
        has_string_constraints: None,
        path_cond_stack: std::cell::RefCell::new(Vec::new()),
        profiler: None,
        ieee754_f64,
    }
}

fn seed_env<'a>(
    ctx: &'a super::Context,
    atom: &Atom,
    module_env: &ModuleEnv,
    ieee754_f64: bool,
) -> Env<'a> {
    let mut env: Env<'a> = HashMap::new();
    env.insert("true".to_string(), Bool::from_bool(ctx, true).into());
    env.insert("false".to_string(), Bool::from_bool(ctx, false).into());
    for param in &atom.params {
        env.insert(
            param.name.clone(),
            param_z3_value(
                ctx,
                &param.name,
                param.type_name.as_deref(),
                module_env,
                ieee754_f64,
            ),
        );
    }
    env.insert(
        "result".to_string(),
        result_z3_value(ctx, atom.return_type.as_deref(), module_env, ieee754_f64),
    );
    env
}

fn result_z3_value<'a>(
    ctx: &'a super::Context,
    return_type: Option<&str>,
    module_env: &ModuleEnv,
    ieee754_f64: bool,
) -> Dynamic<'a> {
    match return_type {
        Some(type_name) => param_z3_value(ctx, "result", Some(type_name), module_env, ieee754_f64),
        None => Int::new_const(ctx, "result").into(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ClauseLoweringOutcome {
    Applied,
    Skipped(String),
}

pub(crate) fn unsupported_clause_warning(
    label: &str,
    clause: &str,
    err: &impl std::fmt::Display,
) -> String {
    format!(
        "Skipped unsupported Z3 clause: {} clause '{}': {}",
        label, clause, err
    )
}

pub(crate) fn push_skip_warning(diagnostics: &mut Vec<String>, warning: String) {
    if warning.starts_with("Skipped unsupported Z3 clause:") && diagnostics.contains(&warning) {
        return;
    }
    diagnostics.push(warning);
}

pub(crate) fn is_unsupported_clause_error(err: &impl std::fmt::Display) -> bool {
    let message = err.to_string();
    message.contains("Unknown function:")
        || message.contains("requires exactly 4 arguments")
        || message.contains("first argument must be a variable name")
        || message.contains("start must be integer")
        || message.contains("end must be integer")
        || message.contains("condition must be boolean")
        || message.contains(
            "Unsupported exponentiation: exponent must be a non-negative integer constant",
        )
}

fn assert_parameter_refinements<'a>(
    vc: &VCtx<'a>,
    solver: &Solver<'a>,
    atom: &Atom,
    module_env: &ModuleEnv,
    env: &mut Env<'a>,
) -> Result<(), SpecContradiction> {
    for param in &atom.params {
        let Some(type_name) = param.type_name.as_deref() else {
            continue;
        };
        let Some(refined) = module_env.get_type(type_name) else {
            continue;
        };
        apply_refinement_constraint(vc, solver, &param.name, refined, env).map_err(|err| {
            SpecContradiction::new(
                &atom.name,
                "refinement_invalid",
                format!(
                    "failed to lower refinement type '{}': {}",
                    refined.name, err
                ),
                vec![refined.predicate_raw.clone()],
                refined.span.clone(),
            )
        })?;
    }
    Ok(())
}

fn assert_clause<'a>(
    vc: &VCtx<'a>,
    solver: &Solver<'a>,
    env: &mut Env<'a>,
    atom: &Atom,
    clause: &str,
    label: &str,
) -> Result<ClauseLoweringOutcome, SpecContradiction> {
    let trimmed = clause.trim();
    if trimmed.is_empty() || trimmed == "true" {
        return Ok(ClauseLoweringOutcome::Applied);
    }
    let clause_ast = parse_expression(trimmed);
    let clause_z3 = match expr_to_z3(vc, &clause_ast, env, None) {
        Ok(value) => value,
        Err(err) if is_unsupported_clause_error(&err) => {
            return Ok(ClauseLoweringOutcome::Skipped(unsupported_clause_warning(
                label, trimmed, &err,
            )));
        }
        Err(err) => {
            return Err(SpecContradiction::new(
                &atom.name,
                "spec_lowering_failed",
                format!("failed to lower {} clause '{}': {}", label, trimmed, err),
                vec![trimmed.to_string()],
                atom.span.clone(),
            ));
        }
    };
    let Some(clause_bool) = clause_z3.as_bool() else {
        return Err(SpecContradiction::new(
            &atom.name,
            "spec_not_boolean",
            format!("{} clause '{}' must lower to boolean", label, trimmed),
            vec![trimmed.to_string()],
            atom.span.clone(),
        ));
    };
    solver.assert(&clause_bool);
    Ok(ClauseLoweringOutcome::Applied)
}

fn assert_negated_clause<'a>(
    vc: &VCtx<'a>,
    solver: &Solver<'a>,
    env: &mut Env<'a>,
    atom: &Atom,
    clause: &str,
    label: &str,
) -> Result<ClauseLoweringOutcome, SpecContradiction> {
    let trimmed = clause.trim();
    if trimmed.is_empty() || trimmed == "true" {
        solver.assert(&Bool::from_bool(vc.ctx, false));
        return Ok(ClauseLoweringOutcome::Applied);
    }
    let clause_ast = parse_expression(trimmed);
    let clause_z3 = match expr_to_z3(vc, &clause_ast, env, None) {
        Ok(value) => value,
        Err(err) if is_unsupported_clause_error(&err) => {
            return Ok(ClauseLoweringOutcome::Skipped(unsupported_clause_warning(
                label, trimmed, &err,
            )));
        }
        Err(err) => {
            return Err(SpecContradiction::new(
                &atom.name,
                "spec_lowering_failed",
                format!(
                    "failed to lower negated {} clause '{}': {}",
                    label, trimmed, err
                ),
                vec![trimmed.to_string()],
                atom.span.clone(),
            ));
        }
    };
    let Some(clause_bool) = clause_z3.as_bool() else {
        return Err(SpecContradiction::new(
            &atom.name,
            "spec_not_boolean",
            format!("{} clause '{}' must lower to boolean", label, trimmed),
            vec![trimmed.to_string()],
            atom.span.clone(),
        ));
    };
    solver.assert(&clause_bool.not());
    Ok(ClauseLoweringOutcome::Applied)
}

fn check_standalone_refinements(
    atom: &Atom,
    module_env: &ModuleEnv,
) -> Result<usize, SpecContradiction> {
    let mut checked = 0usize;
    for refined in module_env.types.values() {
        checked += 1;
        let mut cfg = Config::new();
        cfg.set_timeout_msec(5000);
        let ctx = super::Context::new(&cfg);
        let solver = Solver::new(&ctx);
        let vc = validation_ctx(&ctx, module_env, atom, false);
        let mut env: Env<'_> = HashMap::new();
        env.insert("true".to_string(), Bool::from_bool(&ctx, true).into());
        env.insert("false".to_string(), Bool::from_bool(&ctx, false).into());
        apply_refinement_constraint(&vc, &solver, &refined.operand, refined, &mut env).map_err(
            |err| {
                SpecContradiction::new(
                    &atom.name,
                    "refinement_invalid",
                    format!(
                        "failed to lower refinement type '{}': {}",
                        refined.name, err
                    ),
                    vec![refined.predicate_raw.clone()],
                    refined.span.clone(),
                )
            },
        )?;
        if solver.check() == SatResult::Unsat {
            return Err(SpecContradiction::new(
                &atom.name,
                "refinement_unsat",
                format!("refinement type '{}' is unsatisfiable", refined.name),
                vec![refined.predicate_raw.clone()],
                refined.span.clone(),
            ));
        }
    }
    Ok(checked)
}

pub(crate) fn split_top_level_conjunctions(input: &str) -> Vec<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed == "true" {
        return Vec::new();
    }

    let mut clauses = Vec::new();
    let mut start = 0usize;
    let mut depth = 0i32;
    let chars: Vec<(usize, char)> = trimmed.char_indices().collect();
    let mut index = 0usize;
    while index < chars.len() {
        let (byte_index, ch) = chars[index];
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            '&' if depth == 0 && index + 1 < chars.len() && chars[index + 1].1 == '&' => {
                let clause = trimmed[start..byte_index].trim();
                if !clause.is_empty() {
                    clauses.push(strip_wrapping_parens(clause).to_string());
                }
                start = chars[index + 1].0 + chars[index + 1].1.len_utf8();
                index += 1;
            }
            _ => {}
        }
        index += 1;
    }

    let clause = trimmed[start..].trim();
    if !clause.is_empty() {
        clauses.push(strip_wrapping_parens(clause).to_string());
    }
    clauses
}

fn strip_wrapping_parens(input: &str) -> &str {
    let trimmed = input.trim();
    if trimmed.starts_with('(') && trimmed.ends_with(')') {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    }
}

fn effective_trace_id(atom: &Atom) -> Option<String> {
    atom.trace_id
        .clone()
        .filter(|trace_id| !trace_id.trim().is_empty())
        .or_else(|| {
            std::env::var("MUMEI_TRACE_ID")
                .ok()
                .filter(|trace_id| !trace_id.trim().is_empty())
        })
}

fn effective_spec_metadata(atom: &Atom) -> HashMap<String, String> {
    if !atom.spec_metadata.is_empty() {
        return atom.spec_metadata.clone();
    }

    std::env::var("MUMEI_SPEC_METADATA")
        .ok()
        .and_then(|raw| serde_json::from_str::<HashMap<String, String>>(&raw).ok())
        .unwrap_or_default()
}

fn traceability_coverage(
    atom: &Atom,
    trace_id: Option<&String>,
    spec_metadata: &HashMap<String, String>,
) -> f64 {
    let mut covered = 0usize;
    if trace_id
        .map(|trace_id| !trace_id.trim().is_empty())
        .unwrap_or(false)
    {
        covered += 1;
    }
    if !spec_metadata.is_empty() {
        covered += 1;
    }
    if !atom.requires.trim().is_empty() && atom.requires.trim() != "true" {
        covered += 1;
    }
    if !atom.ensures.trim().is_empty() && atom.ensures.trim() != "true" {
        covered += 1;
    }
    covered as f64 / 4.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_atom;

    #[test]
    fn contradictory_requires_are_rejected() {
        let atom = parse_atom(
            r#"
atom impossible(x: i64) -> i64
  requires: x > 0 && x <= 0;
  ensures: true;
  body: x;
"#,
        );
        let module_env = ModuleEnv::new();

        let err = check_spec_satisfiability(&atom, &module_env).unwrap_err();
        assert_eq!(err.kind, "requires_unsat");

        let result = SpecValidationResult::from_contradiction(&atom, &err);
        assert!(!result.is_satisfiable);
        assert_eq!(result.status, "unsatisfiable");
        assert!(result.contradiction_details.is_some());
    }

    #[test]
    fn traceability_env_metadata_reaches_full_coverage() {
        let atom = parse_atom(
            r#"
atom increment(x: i64) -> i64
  requires: x >= 0;
  ensures: result > x;
  body: x + 1;
"#,
        );
        std::env::set_var("MUMEI_TRACE_ID", "REQ-42");
        std::env::set_var(
            "MUMEI_SPEC_METADATA",
            r#"{"source":"forge_task","requirement_id":"REQ-42"}"#,
        );
        let module_env = ModuleEnv::new();

        let result = check_spec_satisfiability(&atom, &module_env).unwrap();
        std::env::remove_var("MUMEI_TRACE_ID");
        std::env::remove_var("MUMEI_SPEC_METADATA");

        assert!(result.is_satisfiable);
        assert!(result.contradiction_details.is_none());
        assert_eq!(result.trace_id.as_deref(), Some("REQ-42"));
        assert_eq!(result.traceability_hash.len(), 64);
        assert_eq!(result.traceability_coverage, 1.0);
    }

    #[test]
    fn unsupported_function_clause_is_skipped_without_failing_verification() {
        let atom = parse_atom(
            r#"
atom passthrough(x: i64) -> i64
  requires: x >= 0;
  ensures: result == x && is_hex_digit(x);
  body: x;
"#,
        );
        let module_env = ModuleEnv::new();

        let result = check_spec_satisfiability(&atom, &module_env).unwrap();

        assert!(result.is_satisfiable);
        assert_eq!(result.status, "satisfiable_with_skips");
        assert!(result.checked_requires);
        assert_eq!(result.checked_ensures, 1);
        let warning_prefix =
            "Skipped unsupported Z3 clause: ensures clause 'is_hex_digit(x)': Verification Error: Unknown function: is_hex_digit";
        assert_eq!(
            result
                .diagnostics
                .iter()
                .filter(|diag| diag.starts_with(warning_prefix))
                .count(),
            1,
            "expected skipped-clause warning exactly once in diagnostics: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn malformed_quantifier_clause_is_skipped_without_failing_verification() {
        let atom = parse_atom(
            r#"
atom passthrough_with_quantifier(x: i64) -> i64
  requires: x >= 0;
  ensures: result == x && forall(i, 0, x);
  body: x;
"#,
        );
        let module_env = ModuleEnv::new();

        let result = check_spec_satisfiability(&atom, &module_env).unwrap();

        assert!(result.is_satisfiable);
        assert_eq!(result.status, "satisfiable_with_skips");
        assert!(result.checked_requires);
        assert_eq!(result.checked_ensures, 1);
        let warning_prefix = "Skipped unsupported Z3 clause: ensures clause 'forall(i, 0, x)': Verification Error: forall() requires exactly 4 arguments: (var, start, end, condition)";
        assert_eq!(
            result
                .diagnostics
                .iter()
                .filter(|diag| diag.starts_with(warning_prefix))
                .count(),
            1,
            "expected skipped-clause warning exactly once in diagnostics: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn symbolic_exponent_clause_is_skipped_without_failing_verification() {
        let atom = parse_atom(
            r#"
atom passthrough_pow(x: i64, y: i64) -> i64
  requires: x >= 0;
  ensures: result == x**y && result == x;
  body: x;
"#,
        );
        let module_env = ModuleEnv::new();

        let result = check_spec_satisfiability(&atom, &module_env).unwrap();

        assert!(result.is_satisfiable);
        assert_eq!(result.status, "satisfiable_with_skips");
        assert!(result.checked_requires);
        assert_eq!(result.checked_ensures, 1);
        let warning_prefix =
            "Skipped unsupported Z3 clause: ensures clause 'result == x ** y': Verification Error: Unsupported exponentiation: exponent must be a non-negative integer constant";
        assert_eq!(
            result
                .diagnostics
                .iter()
                .filter(|diag| diag.starts_with(warning_prefix))
                .count(),
            1,
            "expected skipped-clause warning exactly once in diagnostics: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn unsupported_clause_whitelist_includes_symbolic_exponent_errors() {
        let err = crate::verification::MumeiError::verification(
            "Unsupported exponentiation: exponent must be a non-negative integer constant",
        );

        assert!(is_unsupported_clause_error(&err));
    }

    #[test]
    fn skipped_requires_clause_marks_requires_unchecked() {
        let atom = parse_atom(
            r#"
atom passthrough_requires(x: i64) -> i64
  requires: is_hex_digit(x);
  ensures: result == x;
  body: x;
"#,
        );
        let module_env = ModuleEnv::new();

        let result = check_spec_satisfiability(&atom, &module_env).unwrap();

        assert!(result.is_satisfiable);
        assert_eq!(result.status, "satisfiable_with_skips");
        assert!(!result.checked_requires);
        assert_eq!(result.checked_ensures, 1);
        let warning_prefix =
            "Skipped unsupported Z3 clause: requires clause 'is_hex_digit(x)': Verification Error: Unknown function: is_hex_digit";
        assert_eq!(
            result
                .diagnostics
                .iter()
                .filter(|diag| diag.starts_with(warning_prefix))
                .count(),
            1,
            "expected skipped-clause warning exactly once in diagnostics: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn lowerable_clauses_keep_satisfiable_status() {
        let atom = parse_atom(
            r#"
atom passthrough_clean(x: i64) -> i64
  requires: x >= 0;
  ensures: result == x;
  body: x;
"#,
        );
        let module_env = ModuleEnv::new();

        let result = check_spec_satisfiability(&atom, &module_env).unwrap();

        assert!(result.is_satisfiable);
        assert_eq!(result.status, "satisfiable");
        assert!(result.checked_requires);
        assert_eq!(result.checked_ensures, 1);
    }
}
