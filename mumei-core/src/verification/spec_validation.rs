use super::module_env::ModuleEnv;
use super::property_based::{
    run_property_based_test_with_mode, PropertyBasedTestConfig, PropertyBasedTestResult,
};
use super::translator::{
    apply_refinement_constraint, expr_to_z3, param_z3_value, seed_tuple_result_components,
    tuple_component_types, VCtx, DEFAULT_CONSTRAINT_BUDGET, I64_BITS,
    UNSUPPORTED_TUPLE_RESULT_INDEXING,
};
use super::types::Env;
use super::SpecContradiction;
use super::{
    parse_expression, Atom, Bool, Config, Context, Dynamic, HashMap, Int, SatResult, Solver, BV,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

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

/// Default per-solver-call budget for spec validation when the caller does not
/// supply one.
pub const DEFAULT_SPEC_VALIDATION_TIMEOUT_MS: u64 = 5000;
// Z3's soft timeout gets first chance; interrupt is fallback for goals that never reach a decision point.
const SOLVER_INTERRUPT_GRACE_MS: u64 = 500;

/// Lowest linked libz3 that honours `ContextHandle::interrupt()`, which is what makes
/// `--solver-timeout` a hard bound on spec-health checks.
pub const MIN_HARD_TIMEOUT_Z3_VERSION: (u32, u32) = (4, 14);

/// Return the linked libz3 version as `(major, minor, build, revision)`.
pub fn linked_z3_version() -> (u32, u32, u32, u32) {
    let mut major = 0;
    let mut minor = 0;
    let mut build = 0;
    let mut revision = 0;
    unsafe {
        z3_sys::Z3_get_version(&mut major, &mut minor, &mut build, &mut revision);
    }
    (major, minor, build, revision)
}

/// Whether the linked libz3 reliably enforces the interrupt-backed deadline.
pub fn solver_timeout_is_hard() -> bool {
    let (major, minor, _, _) = linked_z3_version();
    (major, minor) >= MIN_HARD_TIMEOUT_Z3_VERSION
}

pub fn check_spec_satisfiability(
    atom: &Atom,
    module_env: &ModuleEnv,
) -> Result<SpecValidationResult, SpecContradiction> {
    check_spec_satisfiability_with_property_based(atom, module_env, None, false, false)
}

fn check_with_deadline(solver: &Solver<'_>, ctx: &Context, timeout_ms: u64) -> SatResult {
    let handle = ctx.handle();
    let done = Mutex::new(false);
    let wake = Condvar::new();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let deadline = Instant::now()
                + Duration::from_millis(timeout_ms.saturating_add(SOLVER_INTERRUPT_GRACE_MS));
            let mut done_guard = done.lock().expect("solver watchdog mutex poisoned");
            while !*done_guard {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    drop(done_guard);
                    handle.interrupt();
                    return;
                }
                let (guard, wait_result) = wake
                    .wait_timeout_while(done_guard, remaining, |finished| !*finished)
                    .expect("solver watchdog condvar poisoned");
                done_guard = guard;
                if wait_result.timed_out() && !*done_guard {
                    drop(done_guard);
                    handle.interrupt();
                    return;
                }
            }
        });
        let result = solver.check();
        *done.lock().expect("solver watchdog mutex poisoned") = true;
        wake.notify_one();
        result
    })
}

pub fn check_spec_satisfiability_with_property_based(
    atom: &Atom,
    module_env: &ModuleEnv,
    property_based_config: Option<&PropertyBasedTestConfig>,
    ieee754_f64: bool,
    bitvec_i64: bool,
) -> Result<SpecValidationResult, SpecContradiction> {
    check_spec_satisfiability_with_timeout(
        atom,
        module_env,
        property_based_config,
        ieee754_f64,
        bitvec_i64,
        DEFAULT_SPEC_VALIDATION_TIMEOUT_MS,
    )
}

/// Same as [`check_spec_satisfiability_with_property_based`], but bounds every
/// solver call by `timeout_ms` so that `--solver-timeout` also applies to the
/// spec-health phase instead of only to the proof phase.
pub fn check_spec_satisfiability_with_timeout(
    atom: &Atom,
    module_env: &ModuleEnv,
    property_based_config: Option<&PropertyBasedTestConfig>,
    ieee754_f64: bool,
    bitvec_i64: bool,
    timeout_ms: u64,
) -> Result<SpecValidationResult, SpecContradiction> {
    let timeout_ms = if timeout_ms == 0 {
        DEFAULT_SPEC_VALIDATION_TIMEOUT_MS
    } else {
        timeout_ms.min(DEFAULT_SPEC_VALIDATION_TIMEOUT_MS)
    };
    // Callers without an explicit mode (certificate generation, tooling) still
    // have to encode a bit-vector contract as `BV(64)`; otherwise a healthy
    // spec is reported as unlowerable or contradictory.
    let bitvec_i64_global = bitvec_i64;
    let bitvec_i64 = bitvec_i64
        || super::fragment::atom_requires_bitvector_semantics_in_module(atom, module_env);
    let mut diagnostics = Vec::new();
    let checked_refinements = check_standalone_refinements(
        atom,
        module_env,
        timeout_ms,
        ieee754_f64,
        bitvec_i64,
        bitvec_i64_global,
    )?;

    let mut cfg = Config::new();
    cfg.set_timeout_msec(timeout_ms);
    let ctx = super::Context::new(&cfg);
    let solver = Solver::new(&ctx);
    let vc = validation_ctx(
        &ctx,
        module_env,
        atom,
        ieee754_f64,
        bitvec_i64,
        bitvec_i64_global,
    );
    let mut env = seed_env(&ctx, atom, module_env, ieee754_f64, bitvec_i64);
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

    if check_with_deadline(&solver, &ctx, timeout_ms) == SatResult::Unsat {
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
        let mut local_env = seed_env(&ctx, atom, module_env, ieee754_f64, bitvec_i64);
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
        if check_with_deadline(&local_solver, &ctx, timeout_ms) == SatResult::Unsat {
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
        let mut combined_env = seed_env(&ctx, atom, module_env, ieee754_f64, bitvec_i64);
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
        if check_with_deadline(&combined_solver, &ctx, timeout_ms) == SatResult::Unsat {
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
            let mut local_env = seed_env(&ctx, atom, module_env, ieee754_f64, bitvec_i64);
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
            if check_with_deadline(&local_solver, &ctx, timeout_ms) == SatResult::Unsat {
                diagnostics.push(format!(
                    "ensures clause {} implies clause {} under requires",
                    left_index + 1,
                    right_index + 1
                ));
            }
        }
    }

    // Shift amounts in the contract are lowered without a solver, so their
    // `0 <= n < 64` range is only decidable here, where the requires clause is
    // asserted. A spec whose shifts are unbounded is rejected by verification,
    // and must not be reported healthy. Every clause solver above shares `vc`,
    // so this discharges the obligations collected in all of them against the
    // requires-only assumption the proof itself uses.
    match super::translator::shift_range_status(&vc, &solver) {
        super::translator::ShiftRangeStatus::Bounded => {}
        super::translator::ShiftRangeStatus::OutOfRange => {
            return Err(SpecContradiction::new(
                &atom.name,
                "shift_out_of_range",
                "shift amount may be outside 0..64 under requires",
                vec![atom.requires.clone(), atom.ensures.clone()],
                atom.span.clone(),
            ));
        }
        super::translator::ShiftRangeStatus::Undecided => {
            return Err(SpecContradiction::new(
                &atom.name,
                "shift_range_unknown",
                "Z3 returned unknown for the 0..64 shift range under requires",
                vec![atom.requires.clone(), atom.ensures.clone()],
                atom.span.clone(),
            ));
        }
    }

    let trace_id = effective_trace_id(atom);
    let spec_metadata = effective_spec_metadata(atom);

    let property_based_test = property_based_config.map(|config| {
        let result = run_property_based_test_with_mode(atom, module_env, config, bitvec_i64);
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
    bitvec_i64: bool,
    bitvec_i64_global: bool,
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
        bitvec_i64,
        bv_shift_obligations: std::cell::RefCell::new(Vec::new()),
        bitvec_i64_global,
    }
}

fn seed_env<'a>(
    ctx: &'a super::Context,
    atom: &Atom,
    module_env: &ModuleEnv,
    ieee754_f64: bool,
    bitvec_i64: bool,
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
                bitvec_i64,
            ),
        );
    }
    if tuple_component_types(atom.return_type.as_deref()).is_none() {
        env.insert(
            "result".to_string(),
            result_z3_value(
                ctx,
                atom.return_type.as_deref(),
                module_env,
                ieee754_f64,
                bitvec_i64,
            ),
        );
    }
    seed_tuple_result_components(
        ctx,
        &mut env,
        "result",
        atom.return_type.as_deref(),
        module_env,
        ieee754_f64,
        bitvec_i64,
    );
    env
}

/// Sort of the implicit `result` binding. Mirrors `param_z3_value`, so under
/// `--bitvec-i64` an `i64` result is a `BV(64)` and `ensures` clauses compare
/// bit-vector terms rather than mixing sorts.
fn result_z3_value<'a>(
    ctx: &'a super::Context,
    return_type: Option<&str>,
    module_env: &ModuleEnv,
    ieee754_f64: bool,
    bitvec_i64: bool,
) -> Dynamic<'a> {
    match return_type {
        Some(type_name) => param_z3_value(
            ctx,
            "result",
            Some(type_name),
            module_env,
            ieee754_f64,
            bitvec_i64,
        ),
        None if bitvec_i64 => BV::new_const(ctx, "result", I64_BITS).into(),
        None => Int::new_const(ctx, "result").into(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ClauseLoweringOutcome {
    Applied,
    Skipped(String),
}

pub(crate) const SKIPPED_CLAUSE_PREFIX: &str = "Skipped unsupported Z3 clause:";

pub(crate) fn unsupported_clause_warning(
    label: &str,
    clause: &str,
    err: &impl std::fmt::Display,
) -> String {
    format!(
        "{SKIPPED_CLAUSE_PREFIX} {} clause '{}': {}",
        label, clause, err
    )
}

pub(crate) fn push_skip_warning(diagnostics: &mut Vec<String>, warning: String) {
    if warning.starts_with(SKIPPED_CLAUSE_PREFIX) && diagnostics.contains(&warning) {
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
        || message.contains(UNSUPPORTED_TUPLE_RESULT_INDEXING)
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

fn normalize_foreign_boolean_literals(clause: &str) -> String {
    lazy_static::lazy_static! {
        static ref TRUE_RE: Regex = Regex::new(r"\bTrue\b").unwrap();
        static ref FALSE_RE: Regex = Regex::new(r"\bFalse\b").unwrap();
    }
    let normalized = TRUE_RE.replace_all(clause, "true");
    FALSE_RE.replace_all(&normalized, "false").into_owned()
}

fn assert_clause<'a>(
    vc: &VCtx<'a>,
    solver: &Solver<'a>,
    env: &mut Env<'a>,
    atom: &Atom,
    clause: &str,
    label: &str,
) -> Result<ClauseLoweringOutcome, SpecContradiction> {
    let clause = normalize_foreign_boolean_literals(clause);
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
    let clause = normalize_foreign_boolean_literals(clause);
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

/// Refinement predicates are lowered in the same semantic mode as the atom
/// whose spec is being checked: a refined `i64` whose predicate only holds
/// under two's complement wrapping (or names a bitwise operator) is
/// unlowerable, or spuriously unsatisfiable, in the unbounded `Int` encoding.
fn check_standalone_refinements(
    atom: &Atom,
    module_env: &ModuleEnv,
    timeout_ms: u64,
    ieee754_f64: bool,
    bitvec_i64: bool,
    bitvec_i64_global: bool,
) -> Result<usize, SpecContradiction> {
    let mut checked = 0usize;
    for refined in module_env.types.values() {
        checked += 1;
        let mut cfg = Config::new();
        cfg.set_timeout_msec(timeout_ms);
        let ctx = super::Context::new(&cfg);
        let solver = Solver::new(&ctx);
        let vc = validation_ctx(
            &ctx,
            module_env,
            atom,
            ieee754_f64,
            bitvec_i64,
            bitvec_i64_global,
        );
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
        if check_with_deadline(&solver, &ctx, timeout_ms) == SatResult::Unsat {
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
    if !(trimmed.starts_with('(') && trimmed.ends_with(')')) {
        return trimmed;
    }

    let mut depth = 0i32;
    let mut chars = trimmed.char_indices().peekable();
    while let Some((idx, ch)) = chars.next() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 && chars.peek().is_some() {
                    return trimmed;
                }
                if depth < 0 {
                    return trimmed;
                }
                if depth == 0 && idx + ch.len_utf8() != trimmed.len() {
                    return trimmed;
                }
            }
            _ => {}
        }
    }

    if depth == 0 {
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
    fn strip_wrapping_parens_only_removes_enclosing_pairs() {
        assert_eq!(strip_wrapping_parens("(a == b)"), "a == b");
        assert_eq!(
            split_top_level_conjunctions("(a && b) || (c && d)"),
            vec!["(a && b) || (c && d)".to_string()]
        );
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

    #[test]
    fn interrupted_solver_context_remains_usable() {
        use z3::ast::Ast;

        let cfg = Config::new();
        let ctx = Context::new(&cfg);
        let solver = Solver::new(&ctx);
        let a = Int::new_const(&ctx, "interrupt_a");
        let b = Int::new_const(&ctx, "interrupt_b");
        let c = Int::new_const(&ctx, "interrupt_c");
        let n = Int::new_const(&ctx, "interrupt_n");
        let result = Int::new_const(&ctx, "interrupt_result");
        let lhs = &a * &a * &a + &b * &b * &b * &c;
        let rhs = &c * &c * &c * &a * &b + &n * &n * &n * &a;
        solver.assert(&a.gt(&Int::from_i64(&ctx, 0)));
        solver.assert(&b.gt(&Int::from_i64(&ctx, 0)));
        solver.assert(&c.gt(&Int::from_i64(&ctx, 0)));
        solver.assert(&n.gt(&Int::from_i64(&ctx, 2)));
        solver.assert(&n.lt(&Int::from_i64(&ctx, 10)));
        solver.assert(&result.ge(&Int::from_i64(&ctx, 0)));
        solver.assert(&lhs._eq(&rhs).not());
        solver.assert(&a.gt(&Int::from_i64(&ctx, 1)));
        solver.assert(&b.gt(&Int::from_i64(&ctx, 1)));
        solver.assert(&(&a * &b)._eq(&Int::from_i64(&ctx, 2_305_843_009_213_693_951)));

        assert_eq!(
            check_with_deadline(&solver, &ctx, 1),
            SatResult::Unknown,
            "hard nonlinear check should be interrupted"
        );

        let recovery_solver = Solver::new(&ctx);
        recovery_solver.assert(&Int::from_i64(&ctx, 0)._eq(&Int::from_i64(&ctx, 1)));
        assert_eq!(
            check_with_deadline(&recovery_solver, &ctx, 1000),
            SatResult::Unsat,
            "the context should remain usable after an interrupt"
        );
    }

    #[test]
    fn hard_nonlinear_spec_validation_respects_timeout() {
        // Guards the pre-#523 escalation_candidate_diagnostic_carries_reason hang;
        // Z3 versions below 4.14 ignore the interrupt used by this regression.
        const HARD_NONLINEAR_CHECK_COUNT: usize = 6;
        const HARD_NONLINEAR_REGRESSION_BOUND_SECS: u64 = 5;
        let (major, minor, build, revision) = linked_z3_version();
        if !solver_timeout_is_hard() {
            eprintln!(
                "skipping hard nonlinear timeout regression on Z3 {major}.{minor}.{build}.{revision}: \
                 Z3 < 4.14 ignores ContextHandle::interrupt"
            );
            return;
        }

        let atom = parse_atom(
            r#"
atom hard_nonlinear(a: i64, b: i64, c: i64, n: i64) -> i64
  requires: a > 0 && b > 0 && c > 0 && n > 2 && n < 10;
  ensures: result >= 0 && a * a * a + b * b * b * c != c * c * c * a * b + n * n * n * a;
  body: a + b + c;
"#,
        );
        let started = Instant::now();
        let result =
            check_spec_satisfiability_with_timeout(&atom, &ModuleEnv::new(), None, false, false, 1);
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_secs(HARD_NONLINEAR_REGRESSION_BOUND_SECS),
            "hard nonlinear spec validation exceeded the timeout bound for the \
             pre-#523 escalation_candidate_diagnostic_carries_reason hang: {:?} \
             (observed across {HARD_NONLINEAR_CHECK_COUNT} sequential checks; bound: {}s)",
            elapsed,
            HARD_NONLINEAR_REGRESSION_BOUND_SECS
        );
        assert!(
            result.is_ok(),
            "an interrupted nonlinear check should not report a contradiction: {result:?}"
        );
    }
}
