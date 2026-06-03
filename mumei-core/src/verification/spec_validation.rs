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
            "{}: {} (constraints: {:?})\n{}\nSuggested fix: {}",
            contradiction.kind,
            contradiction.message,
            contradiction.constraints,
            contradiction.natural_language_explanation,
            contradiction.suggested_fix
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
                contradiction.suggested_fix.clone(),
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
    check_spec_satisfiability_with_property_based(atom, module_env, None)
}

pub fn check_spec_satisfiability_with_property_based(
    atom: &Atom,
    module_env: &ModuleEnv,
    property_based_config: Option<&PropertyBasedTestConfig>,
) -> Result<SpecValidationResult, SpecContradiction> {
    let mut diagnostics = Vec::new();
    let checked_refinements = check_standalone_refinements(atom, module_env)?;

    let mut cfg = Config::new();
    cfg.set_timeout_msec(5000);
    let ctx = super::Context::new(&cfg);
    let solver = Solver::new(&ctx);
    let vc = validation_ctx(&ctx, module_env, atom);
    let mut env = seed_env(&ctx, atom, module_env);
    assert_parameter_refinements(&vc, &solver, atom, module_env, &mut env)?;
    assert_clause(&vc, &solver, &mut env, atom, &atom.requires, "requires")?;

    if solver.check() == SatResult::Unsat {
        return Err(SpecContradiction::new(
            &atom.name,
            "requires_unsat",
            "requires clause is unsatisfiable before proof attempt",
            vec![atom.requires.clone()],
            atom.span.clone(),
        ));
    }

    let ensure_clauses = split_top_level_conjunctions(&atom.ensures);
    for (index, clause) in ensure_clauses.iter().enumerate() {
        let local_solver = Solver::new(&ctx);
        let mut local_env = seed_env(&ctx, atom, module_env);
        assert_parameter_refinements(&vc, &local_solver, atom, module_env, &mut local_env)?;
        assert_clause(
            &vc,
            &local_solver,
            &mut local_env,
            atom,
            &atom.requires,
            "requires",
        )?;
        assert_clause(&vc, &local_solver, &mut local_env, atom, clause, "ensures")?;
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
        let mut combined_env = seed_env(&ctx, atom, module_env);
        assert_parameter_refinements(&vc, &combined_solver, atom, module_env, &mut combined_env)?;
        assert_clause(
            &vc,
            &combined_solver,
            &mut combined_env,
            atom,
            &atom.requires,
            "requires",
        )?;
        for clause in &ensure_clauses {
            assert_clause(
                &vc,
                &combined_solver,
                &mut combined_env,
                atom,
                clause,
                "ensures",
            )?;
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
            implication_checks += 1;
            let local_solver = Solver::new(&ctx);
            let mut local_env = seed_env(&ctx, atom, module_env);
            assert_parameter_refinements(&vc, &local_solver, atom, module_env, &mut local_env)?;
            assert_clause(
                &vc,
                &local_solver,
                &mut local_env,
                atom,
                &atom.requires,
                "requires",
            )?;
            assert_clause(&vc, &local_solver, &mut local_env, atom, left, "ensures")?;
            assert_negated_clause(&vc, &local_solver, &mut local_env, atom, right, "ensures")?;
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
        status: "satisfiable".to_string(),
        is_satisfiable: true,
        contradiction_details: None,
        trace_id: trace_id.clone(),
        spec_metadata: spec_metadata.clone(),
        traceability_hash: calculate_traceability_hash(atom),
        traceability_coverage: traceability_coverage(atom, trace_id.as_ref(), &spec_metadata),
        checked_requires: true,
        checked_ensures: ensure_clauses.len(),
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
    }
}

fn seed_env<'a>(ctx: &'a super::Context, atom: &Atom, module_env: &ModuleEnv) -> Env<'a> {
    let mut env: Env<'a> = HashMap::new();
    env.insert("true".to_string(), Bool::from_bool(ctx, true).into());
    env.insert("false".to_string(), Bool::from_bool(ctx, false).into());
    for param in &atom.params {
        env.insert(
            param.name.clone(),
            param_z3_value(ctx, &param.name, param.type_name.as_deref(), module_env),
        );
    }
    env.insert(
        "result".to_string(),
        result_z3_value(ctx, atom.return_type.as_deref(), module_env),
    );
    env
}

fn result_z3_value<'a>(
    ctx: &'a super::Context,
    return_type: Option<&str>,
    module_env: &ModuleEnv,
) -> Dynamic<'a> {
    match return_type {
        Some(type_name) => param_z3_value(ctx, "result", Some(type_name), module_env),
        None => Int::new_const(ctx, "result").into(),
    }
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
) -> Result<(), SpecContradiction> {
    let trimmed = clause.trim();
    if trimmed.is_empty() || trimmed == "true" {
        return Ok(());
    }
    let clause_ast = parse_expression(trimmed);
    let clause_z3 = expr_to_z3(vc, &clause_ast, env, None).map_err(|err| {
        SpecContradiction::new(
            &atom.name,
            "spec_lowering_failed",
            format!("failed to lower {} clause '{}': {}", label, trimmed, err),
            vec![trimmed.to_string()],
            atom.span.clone(),
        )
    })?;
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
    Ok(())
}

fn assert_negated_clause<'a>(
    vc: &VCtx<'a>,
    solver: &Solver<'a>,
    env: &mut Env<'a>,
    atom: &Atom,
    clause: &str,
    label: &str,
) -> Result<(), SpecContradiction> {
    let trimmed = clause.trim();
    if trimmed.is_empty() || trimmed == "true" {
        solver.assert(&Bool::from_bool(vc.ctx, false));
        return Ok(());
    }
    let clause_ast = parse_expression(trimmed);
    let clause_z3 = expr_to_z3(vc, &clause_ast, env, None).map_err(|err| {
        SpecContradiction::new(
            &atom.name,
            "spec_lowering_failed",
            format!(
                "failed to lower negated {} clause '{}': {}",
                label, trimmed, err
            ),
            vec![trimmed.to_string()],
            atom.span.clone(),
        )
    })?;
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
    Ok(())
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
        let vc = validation_ctx(&ctx, module_env, atom);
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

fn split_top_level_conjunctions(input: &str) -> Vec<String> {
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
}
