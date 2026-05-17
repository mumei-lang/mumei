use super::fragment::*;
use super::module_env::*;
use super::nlae_reporter::*;
use super::support::*;
use super::translator::*;
use super::types::*;
use super::*;

/// mumei.toml の [proof]/[build] 設定を反映した verify
/// timeout_ms: Z3 ソルバのタイムアウト（ミリ秒）
/// global_max_unroll: BMC のグローバル展開深度
pub fn verify_with_config(
    hir_atom: &HirAtom,
    output_dir: &Path,
    module_env: &ModuleEnv,
    timeout_ms: u64,
    _global_max_unroll: usize,
) -> MumeiResult<()> {
    verify_inner(hir_atom, output_dir, module_env, timeout_ms, true)
}

pub fn verify_with_verification_config(
    hir_atom: &HirAtom,
    output_dir: &Path,
    module_env: &ModuleEnv,
    config: &VerificationConfig,
) -> MumeiResult<()> {
    verify_inner(
        hir_atom,
        output_dir,
        module_env,
        config.timeout_ms,
        config.enable_spurious_detection,
    )
}

pub fn verify_module(
    module_env: &ModuleEnv,
    config: &VerificationConfig,
) -> MumeiResult<ModuleVerificationReport> {
    let cross_spec = if config.enable_cross_spec_verification {
        Some(CrossSpecVerifier::new(module_env).verify_all())
    } else {
        None
    };
    let decidable_fragment = if config.collect_decidable_fragment_metrics {
        Some(collect_decidable_fragment_metrics(module_env))
    } else {
        None
    };

    Ok(ModuleVerificationReport {
        cross_spec,
        decidable_fragment,
    })
}

pub fn verify(hir_atom: &HirAtom, output_dir: &Path, module_env: &ModuleEnv) -> MumeiResult<()> {
    verify_inner(hir_atom, output_dir, module_env, 10000, true)
}

/// Compile-time metrics for a single atom verification.
/// Tracks the duration of each phase and total constraint count.
pub struct VerificationMetrics {
    pub atom_name: String,
    pub phase_times: Vec<(String, std::time::Duration)>,
    pub total_constraints: usize,
    pub z3_check_time: std::time::Duration,
}

impl VerificationMetrics {
    pub(crate) fn new(atom_name: &str) -> Self {
        Self {
            atom_name: atom_name.to_string(),
            phase_times: Vec::new(),
            total_constraints: 0,
            z3_check_time: std::time::Duration::ZERO,
        }
    }

    pub(crate) fn record_phase(&mut self, name: &str, duration: std::time::Duration) {
        self.phase_times.push((name.to_string(), duration));
    }

    /// Print metrics to stderr (for --verbose / debug output).
    pub fn print_summary(&self) {
        eprintln!("  [metrics] atom '{}' verification phases:", self.atom_name);
        for (name, dur) in &self.phase_times {
            eprintln!("    {}: {:.3}ms", name, dur.as_secs_f64() * 1000.0);
        }
        eprintln!(
            "    total_constraints: {}, z3_check: {:.3}ms",
            self.total_constraints,
            self.z3_check_time.as_secs_f64() * 1000.0
        );
    }
}

/// PR 1: Centralised Z3 solver tuning for atoms whose contracts mix
/// `forall(i, …, arr[i] …)` quantifiers with `Array::store` updates.
///
/// We enable model-based quantifier instantiation (`smt.mbqi`) on the
/// solver so that `forall(i, …, arr[i] …)` pattern triggers fire under
/// post-store array states. `mbqi` is a module-level (`smt.*`) param so
/// it must be set on the solver via `Params`, not on `Config` (which
/// only accepts global Z3 parameters).
///
/// We deliberately do NOT also tune `qi.eager_threshold` here:
/// empirically, the z3-rs binding (0.12) silently drops the surrounding
/// solver assertions when an unrecognized solver param is set, which
/// would erase the requires-side forall and make every quantified
/// ensures fail. `mbqi` alone is enough to recover post-store ensures
/// verification on top of the pattern extraction performed by
/// `expr_to_z3` / `stmt_to_z3`.
///
/// Centralising this in a helper keeps the verifier's main z3 setup
/// focused on the verification body and gives future tuning passes a
/// single place to extend (e.g. per-atom `forall+store` heuristics).
pub(crate) fn configure_array_quantifier_params(ctx: &Context, solver: &Solver) {
    let mut params = z3::Params::new(ctx);
    params.set_bool("mbqi", true);
    solver.set_params(&params);
}

pub(crate) fn verify_inner(
    hir_atom: &HirAtom,
    output_dir: &Path,
    module_env: &ModuleEnv,
    timeout_ms: u64,
    enable_spurious_detection: bool,
) -> MumeiResult<()> {
    let atom = &hir_atom.atom;
    let mut metrics = VerificationMetrics::new(&atom.name);

    // ジェネリック atom は単相化後に検証される
    // 例: pipe<E: Effect> は検証スキップ、pipe<FileWrite> が検証対象
    if !atom.type_params.is_empty() {
        return Ok(());
    }

    // Phase 0: 信頼レベルチェック（Trust Boundary）
    match &atom.trust_level {
        TrustLevel::Trusted => {
            // trusted atom: body の検証をスキップし、契約（requires/ensures）のみ信頼する。
            // 呼び出し元は契約に基づいて Compositional Verification を行う。
            save_visualizer_report(
                output_dir,
                "trusted",
                &atom.name,
                "N/A",
                "N/A",
                "Trusted: body verification skipped, contract assumed correct.",
                None,
                "",
                None,
                Some(&atom.span),
                None,
            );
            return Ok(());
        }
        TrustLevel::Unverified => {
            // unverified atom: 警告を出すが、検証は続行する。
            // ensures が non-trivial な場合のみ検証を試みる。
            eprintln!(
                "  ⚠️  Warning: atom '{}' is marked as 'unverified'. \
                       Verification results may be incomplete.",
                atom.name
            );
            if atom.ensures.trim() == "true" && atom.requires.trim() == "true" {
                // 契約が trivial な場合、検証する意味がないのでスキップ
                save_visualizer_report(
                    output_dir,
                    "unverified",
                    &atom.name,
                    "N/A",
                    "N/A",
                    "Unverified: no contract to verify.",
                    None,
                    "",
                    None,
                    Some(&atom.span),
                    None,
                );
                return Ok(());
            }
        }
        TrustLevel::Verified => {
            // 通常の検証フロー
        }
    }

    // Phase 1a: リソース階層検証（デッドロック防止）
    let phase_start = std::time::Instant::now();
    verify_resource_hierarchy(atom, module_env)?;
    metrics.record_phase("Phase 1a: resource hierarchy", phase_start.elapsed());

    // Phase 1f: エフェクト包含検証（副作用安全性）
    let phase_start = std::time::Instant::now();
    if let Err(e) = verify_effect_containment(atom, &hir_atom.body_stmt, module_env) {
        // Save structured effect violation report for self-healing integration.
        // Extract missing effects from the error to produce a structured report.
        let allowed_leaves = module_env.resolve_leaf_effects_from_effects(&atom.effects);
        let caller_effect_names: Vec<String> =
            atom.effects.iter().map(|e| e.name.clone()).collect();

        // まず callee ベースの違反を探す（effect_propagation）
        let callees = collect_callees_stmt(&hir_atom.body_stmt);
        let mut missing_all: Vec<String> = Vec::new();
        let mut violating_callee = String::new();
        let mut callee_effs: Vec<String> = Vec::new();
        for callee_name in &callees {
            if let Some(callee_atom) = module_env.get_atom(callee_name) {
                if !callee_atom.effects.is_empty() {
                    let callee_leaves =
                        module_env.resolve_leaf_effects_from_effects(&callee_atom.effects);
                    let missing: Vec<String> = callee_leaves
                        .iter()
                        .filter(|callee_eff| {
                            !allowed_leaves.contains(*callee_eff)
                                && !allowed_leaves
                                    .iter()
                                    .any(|allowed| module_env.is_subeffect(callee_eff, allowed))
                        })
                        .cloned()
                        .collect();
                    if !missing.is_empty() {
                        violating_callee = callee_name.clone();
                        callee_effs = callee_atom.effects.iter().map(|e| e.name.clone()).collect();
                        missing_all = missing;
                        break;
                    }
                }
            }
        }
        if !missing_all.is_empty() {
            save_effect_propagation_report(
                output_dir,
                &atom.name,
                &violating_callee,
                &caller_effect_names,
                &callee_effs,
                &missing_all,
            );
        } else {
            // callee ループでは見つからなかった場合、atom_ref パラメータの effect_set 違反を確認する
            for param in &atom.params {
                if let Some(ref type_ref) = param.type_ref {
                    if type_ref.is_fn_type() {
                        if let Some(ref effect_set) = type_ref.effect_set {
                            let param_leaves = module_env.resolve_leaf_effects(effect_set);
                            let missing: Vec<String> = param_leaves
                                .iter()
                                .filter(|eff| {
                                    !allowed_leaves.contains(*eff)
                                        && !allowed_leaves
                                            .iter()
                                            .any(|allowed| module_env.is_subeffect(eff, allowed))
                                })
                                .cloned()
                                .collect();
                            if !missing.is_empty() {
                                save_effect_polymorphism_report(
                                    output_dir,
                                    &atom.name,
                                    &param.name,
                                    effect_set,
                                    &caller_effect_names,
                                    &missing,
                                );
                                break;
                            }
                        }
                    }
                }
            }
        }
        return Err(e);
    }
    metrics.record_phase("Phase 1f: effect containment", phase_start.elapsed());

    // Phase 1b: 有界モデル検査（ループ内 acquire パターン）
    let phase_start = std::time::Instant::now();
    verify_bmc_resource_safety(atom, &hir_atom.body_stmt, module_env)?;
    metrics.record_phase("Phase 1b: BMC resource safety", phase_start.elapsed());

    // Phase 1c: 再帰的 async 呼び出しの深度検証
    let phase_start = std::time::Instant::now();
    verify_async_recursion_depth(atom, &hir_atom.body_stmt, module_env)?;
    metrics.record_phase("Phase 1c: async recursion depth", phase_start.elapsed());

    // Phase 1d: atom レベル invariant の帰納的検証
    let phase_start = std::time::Instant::now();
    if let Some(ref invariant_expr) = atom.invariant {
        verify_atom_invariant(atom, &hir_atom.body_stmt, invariant_expr, module_env)?;
    }
    metrics.record_phase("Phase 1d: atom invariant", phase_start.elapsed());

    // Phase 1e: Call Graph サイクル検知（間接再帰の検出）
    let phase_start = std::time::Instant::now();
    verify_call_graph_cycles(atom, module_env)?;
    metrics.record_phase("Phase 1e: call graph cycles", phase_start.elapsed());

    // Phase 1f-2: エフェクト整合性チェック（宣言 vs 推論、警告レベル）
    // Note: verify_effect_consistency always returns Ok(()) — warnings are emitted
    // via eprintln! inside the function itself.
    let _ = verify_effect_consistency(atom, module_env);

    // Phase 1g: エフェクトパラメータ制約検証
    let phase_start = std::time::Instant::now();
    verify_effect_params(atom, module_env)?;
    metrics.record_phase("Phase 1g: effect params", phase_start.elapsed());

    // Phase 1h: MIR-based move analysis (Phase 4c integrated)
    // Lower HIR to MIR and run forward dataflow move analysis.
    // Copy types (Int, Nat, Bool, f64, etc.) are distinguished from Move types
    // via the Movability field on LocalDecl. Copy types are never consumed by
    // Rvalue::Use, so violations are only reported for Move types.
    // Move type violations are hard errors; Copy type false positives are eliminated.
    let phase_start = std::time::Instant::now();
    let mir_body = crate::mir::lower_hir_to_mir(hir_atom);
    let move_conflict_locals: Vec<(crate::mir::Local, crate::mir::BasicBlockId)> = Vec::new();
    if mir_body.check_analysis_budget().is_ok() {
        let move_result = crate::mir_analysis::analyze_moves(&mir_body);
        if let Some(v) = move_result.violations.first() {
            // Look up the local's name for better error messages
            let local_name = mir_body
                .locals
                .iter()
                .find(|d| d.local == v.local)
                .and_then(|d| d.name.clone())
                .unwrap_or_else(|| format!("_{}", v.local.0));
            match v.kind {
                crate::mir_analysis::MoveViolationKind::UseAfterMove => {
                    return Err(MumeiError::verification(format!(
                        "use of moved value `{}`: Local({}) was used after being moved in block {}",
                        local_name, v.local.0, v.block_id
                    )));
                }
                crate::mir_analysis::MoveViolationKind::DoubleMove => {
                    return Err(MumeiError::verification(format!(
                        "value `{}` moved twice: Local({}) was moved more than once in block {}",
                        local_name, v.local.0, v.block_id
                    )));
                }
                crate::mir_analysis::MoveViolationKind::ConflictingMerge => {
                    return Err(MumeiError::verification(format!(
                        "conflicting ownership of `{}`: Local({}) is alive on one control-flow path \
                         but consumed on another at merge point (block {})",
                        local_name, v.local.0, v.block_id
                    )));
                }
            }
        }
    }
    metrics.record_phase("Phase 1h: MIR move analysis", phase_start.elapsed());

    // Phase 1i: Temporal effect verification (stateful effects)
    // Build state machines from effect_defs and run forward dataflow analysis
    // on the MIR to verify that perform operations occur in valid states.
    let phase_start = std::time::Instant::now();
    {
        let mut state_machines: std::collections::HashMap<
            String,
            crate::mir_analysis::EffectStateMachine,
        > = std::collections::HashMap::new();

        // Build state machines from all known effect_defs
        for (name, def) in &module_env.effect_defs {
            if let Some(sm) = crate::mir_analysis::EffectStateMachine::from_effect_def(def) {
                state_machines.insert(name.clone(), sm);
            }
        }
        // Also check effects map
        for (name, def) in &module_env.effects {
            if !state_machines.contains_key(name) {
                if let Some(sm) = crate::mir_analysis::EffectStateMachine::from_effect_def(def) {
                    state_machines.insert(name.clone(), sm);
                }
            }
        }

        // Modular Verification: Override initial states from effect_pre contracts
        for (effect_name, pre_state) in &atom.effect_pre {
            if let Some(sm) = state_machines.get_mut(effect_name) {
                if sm.states.contains(pre_state) {
                    sm.initial_state = pre_state.clone();
                } else {
                    return Err(MumeiError::verification(format!(
                        "effect_pre: state '{}' is not a valid state for effect '{}' (valid states: {:?})",
                        pre_state, effect_name, sm.states
                    )));
                }
            } else {
                eprintln!(
                    "  ⚠️  effect_pre: no state machine found for effect '{}' (stateless effects are ignored)",
                    effect_name
                );
            }
        }

        if !state_machines.is_empty() && mir_body.check_analysis_budget().is_ok() {
            // Build callee effect contracts for cross-atom composition
            let mut callee_contracts: std::collections::HashMap<
                String,
                crate::mir_analysis::AtomEffectContract,
            > = std::collections::HashMap::new();
            for (atom_name, callee_atom) in &module_env.atoms {
                if !callee_atom.effect_pre.is_empty() || !callee_atom.effect_post.is_empty() {
                    callee_contracts.insert(
                        atom_name.clone(),
                        crate::mir_analysis::AtomEffectContract {
                            effect_pre: callee_atom.effect_pre.clone(),
                            effect_post: callee_atom.effect_post.clone(),
                        },
                    );
                }
            }
            let temporal_result = crate::mir_analysis::analyze_temporal_effects_with_contracts(
                &mir_body,
                &state_machines,
                if callee_contracts.is_empty() {
                    None
                } else {
                    Some(&callee_contracts)
                },
            );

            for v in &temporal_result.violations {
                match v.kind {
                    crate::mir_analysis::TemporalViolationKind::InvalidPreState => {
                        // Hard error: operation performed in wrong state
                        return Err(MumeiError::verification(format!(
                            "Temporal effect violation (InvalidPreState): '{}' operation '{}' requires state '{}' \
                             but current state is '{}' (block {})",
                            v.effect, v.operation, v.expected_state, v.actual_state, v.block_id
                        )));
                    }
                    crate::mir_analysis::TemporalViolationKind::ConflictingState => {
                        // Plan 20: Z3 Int Sort constraint generation for conflicting
                        // states at merge points.  We encode each state as an integer
                        // and ask Z3 whether both predecessor states can be satisfied
                        // simultaneously — if UNSAT the conflict is irreconcilable.

                        // Look up the state machine for this effect.
                        if let Some(sm) = state_machines.get(&v.effect) {
                            let expected_int = encode_effect_state(sm, &v.expected_state);
                            let actual_int = encode_effect_state(sm, &v.actual_state);

                            // Only proceed if both states are known.
                            if expected_int >= 0 && actual_int >= 0 {
                                // Check constraint budget: each Z3 probe costs ~4
                                // assertions (variable, branch-a, branch-b, equality).
                                // Phase 1i runs before the main solver is created, so
                                // we use mir_body complexity as a proxy budget check.
                                let budget_ok = mir_body.complexity() < DEFAULT_CONSTRAINT_BUDGET;

                                if budget_ok {
                                    // Create a scoped Z3 context + solver for this probe.
                                    let z3_cfg = Config::new();
                                    let z3_ctx = Context::new(&z3_cfg);
                                    let z3_solver = Solver::new(&z3_ctx);

                                    // Z3 Int variable: __effect_state_{effect}_{block_id}
                                    let var_name =
                                        format!("__effect_state_{}_{}", v.effect, v.block_id);
                                    let state_var = Int::new_const(&z3_ctx, var_name.as_str());

                                    // Assert: state_var == expected (from one branch)
                                    let eq_expected =
                                        state_var._eq(&Int::from_i64(&z3_ctx, expected_int));
                                    // Assert: state_var == actual (from other branch)
                                    let eq_actual =
                                        state_var._eq(&Int::from_i64(&z3_ctx, actual_int));

                                    // Both must hold simultaneously at the merge point.
                                    z3_solver.assert(&eq_expected);
                                    z3_solver.assert(&eq_actual);

                                    // Also constrain variable to valid state range.
                                    let num_states = sm.states.len() as i64;
                                    z3_solver.assert(&state_var.ge(&Int::from_i64(&z3_ctx, 0)));
                                    z3_solver
                                        .assert(&state_var.lt(&Int::from_i64(&z3_ctx, num_states)));

                                    match z3_solver.check() {
                                        SatResult::Unsat => {
                                            // Irreconcilable: the two branches require
                                            // mutually exclusive states → hard error.
                                            return Err(MumeiError::verification(format!(
                                                "Temporal effect conflict (Z3 UNSAT): effect '{}' \
                                                 has irreconcilable states at merge point (block {}): \
                                                 '{}' (={}) vs '{}' (={}). \
                                                 The conflict cannot be resolved.",
                                                v.effect, v.block_id,
                                                v.expected_state, expected_int,
                                                v.actual_state, actual_int,
                                            )));
                                        }
                                        SatResult::Sat => {
                                            // SAT means the states are actually compatible
                                            // (should not normally happen for truly different
                                            // states, but could occur with aliased encodings).
                                            // Emit info diagnostic.
                                            eprintln!(
                                                "  \u{2139}\u{fe0f}  Temporal effect info: '{}' conflicting states \
                                                 at block {} resolved by Z3 (SAT): '{}' vs '{}'.",
                                                v.effect, v.block_id,
                                                v.expected_state, v.actual_state
                                            );
                                        }
                                        SatResult::Unknown => {
                                            // Solver timeout / unknown — keep as warning.
                                            eprintln!(
                                                "  \u{26a0}\u{fe0f}  Temporal effect warning: '{}' conflicting states \
                                                 at block {}: Z3 returned Unknown for '{}' vs '{}'.",
                                                v.effect, v.block_id,
                                                v.expected_state, v.actual_state
                                            );
                                        }
                                    }
                                } else {
                                    // Budget exceeded — fall back to warning.
                                    eprintln!(
                                        "  \u{26a0}\u{fe0f}  Temporal effect warning: '{}' has conflicting states \
                                         at merge point (block {}): '{}' vs '{}'. \
                                         Constraint budget exceeded, Z3 probe skipped.",
                                        v.effect, v.block_id,
                                        v.expected_state, v.actual_state
                                    );
                                }
                            } else {
                                // Unknown state name — fall back to warning.
                                eprintln!(
                                    "  \u{26a0}\u{fe0f}  Temporal effect warning: '{}' has conflicting states \
                                     at merge point (block {}): '{}' vs '{}'. \
                                     State encoding failed.",
                                    v.effect, v.block_id,
                                    v.expected_state, v.actual_state
                                );
                            }
                        } else {
                            // No state machine found — fall back to warning.
                            eprintln!(
                                "  \u{26a0}\u{fe0f}  Temporal effect warning: '{}' has conflicting states \
                                 at merge point (block {}): '{}' vs '{}'. \
                                 No state machine found.",
                                v.effect, v.block_id,
                                v.expected_state, v.actual_state
                            );
                        }
                    }
                    crate::mir_analysis::TemporalViolationKind::UnexpectedFinalState => {
                        // Hard error: effect left in unexpected state at exit
                        return Err(MumeiError::verification(format!(
                            "Temporal effect violation: effect '{}' has final state '{}' \
                             but effect_post declares '{}' (block {})",
                            v.effect, v.actual_state, v.expected_state, v.block_id
                        )));
                    }
                }
            }

            // Modular Verification: Check effect_post contracts against exit states
            if !atom.effect_post.is_empty() {
                for (effect_name, expected_post) in &atom.effect_post {
                    // Validate that the effect has a state machine
                    if !state_machines.contains_key(effect_name) {
                        eprintln!(
                            "  ⚠️  effect_post: no state machine found for effect '{}' (stateless effects are ignored)",
                            effect_name
                        );
                        continue;
                    }
                    // Validate that the expected post-state is a valid state
                    if let Some(sm) = state_machines.get(effect_name) {
                        if !sm.states.contains(expected_post) {
                            return Err(MumeiError::verification(format!(
                                "effect_post: state '{}' is not a valid state for effect '{}' (valid states: {:?})",
                                expected_post, effect_name, sm.states
                            )));
                        }
                    }
                    // Find the exit state for this effect from the last basic block(s)
                    // that have a Return terminator
                    let mut found_exit = false;
                    for (block_id, exit_map) in &temporal_result.exit_states {
                        let block = &mir_body.blocks[*block_id];
                        if matches!(block.terminator, crate::mir::Terminator::Return(_)) {
                            if let Some(actual_state) = exit_map.get(effect_name) {
                                found_exit = true;
                                if actual_state != expected_post {
                                    return Err(MumeiError::verification(format!(
                                        "Temporal effect violation: effect '{}' has final state '{}' \
                                         but effect_post declares '{}' (block {})",
                                        effect_name, actual_state, expected_post, block_id
                                    )));
                                }
                            }
                        }
                    }
                    if !found_exit {
                        eprintln!(
                            "  ⚠️  effect_post: effect '{}' has no tracked exit state \
                             (no perform operations for this effect in the body)",
                            effect_name
                        );
                    }
                }
            }
        }
    }
    metrics.record_phase("Phase 1i: temporal effects", phase_start.elapsed());

    // ✅ Phase 4c complete (Plan 19): MIR lowering now covers all expression forms
    // (Match, Lambda, Async, Await, Task, TaskGroup, ChanSend, ChanRecv, etc.).
    // Primary move analysis is handled by Phase 1h above (MIR MoveAnalysis).
    // LinearityCtx below is retained only for Z3-level borrow/consume tracking.

    // Sort-aware timeout: if has_string_constraints is true, double the timeout.
    // Z3 String Sort is now integrated for effect parameter constraints.
    // When string constraints are present, solving is significantly slower,
    // so we double the timeout to accommodate.
    let has_string_constraints_cell_pre = std::cell::Cell::new(false);
    // Pre-scan (1): check if any declared effect has Str-typed params with constraints
    // that would need Z3 String Sort (variable params, not constant-folded).
    for eff in &atom.effects {
        let effect_def = module_env
            .effect_defs
            .get(&eff.name)
            .or_else(|| module_env.effects.get(&eff.name));
        if let Some(def) = effect_def {
            if def.constraint.is_some() {
                for p in &eff.params {
                    if !p.is_constant {
                        has_string_constraints_cell_pre.set(true);
                    }
                }
            }
        }
    }
    // Pre-scan (2): also check the body for `perform` expressions with
    // non-constant args whose EffectDef has a constraint. This catches cases
    // where the atom declares `effects: [FileRead("/tmp/")]` (constant) but
    // the body does `perform FileRead.read(some_variable)`.
    fn body_has_symbolic_perform_args(stmt: &Stmt, module_env: &ModuleEnv) -> bool {
        match stmt {
            Stmt::Block(stmts, _) => stmts
                .iter()
                .any(|s| body_has_symbolic_perform_args(s, module_env)),
            Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
                expr_has_symbolic_perform_args(value, module_env)
            }
            Stmt::ArrayStore { index, value, .. } => {
                expr_has_symbolic_perform_args(index, module_env)
                    || expr_has_symbolic_perform_args(value, module_env)
            }
            Stmt::While { cond, body, .. } => {
                expr_has_symbolic_perform_args(cond, module_env)
                    || body_has_symbolic_perform_args(body, module_env)
            }
            Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
                body_has_symbolic_perform_args(body, module_env)
            }
            Stmt::TaskGroup { children, .. } => children
                .iter()
                .any(|c| body_has_symbolic_perform_args(c, module_env)),
            Stmt::Expr(e, _) => expr_has_symbolic_perform_args(e, module_env),
            // Plan 8: Cancel statement has no perform args
            Stmt::Cancel { .. } => false,
        }
    }
    fn expr_has_symbolic_perform_args(expr: &Expr, module_env: &ModuleEnv) -> bool {
        match expr {
            Expr::Perform { effect, args, .. } => {
                let effect_def = module_env
                    .effect_defs
                    .get(effect.as_str())
                    .or_else(|| module_env.effects.get(effect.as_str()));
                if let Some(def) = effect_def {
                    if def.constraint.is_some() {
                        for arg in args {
                            if !matches!(arg, Expr::Number(_) | Expr::Float(_)) {
                                return true;
                            }
                        }
                    }
                }
                // Also recurse into args
                args.iter()
                    .any(|a| expr_has_symbolic_perform_args(a, module_env))
            }
            Expr::IfThenElse {
                cond,
                then_branch,
                else_branch,
            } => {
                expr_has_symbolic_perform_args(cond, module_env)
                    || body_has_symbolic_perform_args(then_branch, module_env)
                    || body_has_symbolic_perform_args(else_branch, module_env)
            }
            Expr::BinaryOp(l, _, r) => {
                expr_has_symbolic_perform_args(l, module_env)
                    || expr_has_symbolic_perform_args(r, module_env)
            }
            Expr::Call(_, args) => args
                .iter()
                .any(|a| expr_has_symbolic_perform_args(a, module_env)),
            Expr::Async { body } => body_has_symbolic_perform_args(body, module_env),
            Expr::Await { expr } => expr_has_symbolic_perform_args(expr, module_env),
            Expr::Lambda { body, .. } => body_has_symbolic_perform_args(body, module_env),
            Expr::CallRef { callee, args } => {
                expr_has_symbolic_perform_args(callee, module_env)
                    || args
                        .iter()
                        .any(|a| expr_has_symbolic_perform_args(a, module_env))
            }
            Expr::Match { target, arms } => {
                expr_has_symbolic_perform_args(target, module_env)
                    || arms
                        .iter()
                        .any(|arm| body_has_symbolic_perform_args(&arm.body, module_env))
            }
            // Plan 8: Channel operations — traverse sub-expressions for symbolic perform args
            Expr::ChanSend { channel, value } => {
                expr_has_symbolic_perform_args(channel, module_env)
                    || expr_has_symbolic_perform_args(value, module_env)
            }
            Expr::ChanRecv { channel } => expr_has_symbolic_perform_args(channel, module_env),
            // NOTE: StructInit, FieldAccess, and ArrayAccess contain sub-expressions
            // that could hold nested Perform nodes, but are not recursed into here.
            // This means the pre-scan conservatively under-estimates: if a perform with
            // a variable arg appears inside a struct field initializer, field access base,
            // or array index, the timeout won't be doubled. This is safe (slower, not
            // incorrect) and these patterns are rare in practice.
            _ => false,
        }
    }
    if !has_string_constraints_cell_pre.get()
        && body_has_symbolic_perform_args(&hir_atom.body_stmt, module_env)
    {
        has_string_constraints_cell_pre.set(true);
    }
    // Detect array-heavy forall constraints (forall over `arr[...]` accesses).
    // These benefit from a longer timeout and model-based quantifier
    // instantiation, mirroring the way String-Sort constraints already
    // double the timeout above.
    let has_array_forall = atom.forall_constraints.iter().any(|q| {
        // Cheap textual heuristic: the parsed condition string contains a
        // `<name>[` token. Avoids paying parse cost twice and matches the
        // common `arr[i]`, `data[k]` shapes used in the std lib.
        q.condition.contains('[')
    });
    // TODO(timeout-multiplier): When an atom carries BOTH string constraints
    // and `forall + arr[i]` quantifiers, the string-constraint branch (2x)
    // currently wins over the array-forall branch (3x), giving a *shorter*
    // effective timeout than the forall-only case. No real atom hits this
    // today, but if one does, switch to `max(string_mult, array_mult)` (or
    // a multiplicative composition) — see PR #174 review thread.
    let effective_timeout = if has_string_constraints_cell_pre.get() {
        timeout_ms * 2
    } else if has_array_forall {
        timeout_ms * 3
    } else {
        timeout_ms
    };

    let mut cfg = Config::new();
    cfg.set_timeout_msec(effective_timeout);
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);
    if has_array_forall {
        configure_array_quantifier_params(&ctx, &solver);
    }

    // linearity_ctx is wrapped in RefCell so expr_to_z3/stmt_to_z3 can mutate it
    // without requiring signature changes to every recursive call site.
    let linearity_ctx_cell = std::cell::RefCell::new(LinearityCtx::new());
    // Build EffectCtx from the atom's declared effects (transitively resolved)
    let allowed_effects_set = module_env.resolve_effect_set_from_effects(&atom.effects);
    let effect_ctx_cell = std::cell::RefCell::new(EffectCtx::new(allowed_effects_set));
    // Per-atom constraint budget tracking
    let constraint_count_cell = std::cell::Cell::new(0usize);
    // Sort-aware timeout: flag for Z3 String Sort constraints.
    // Set to true when Z3 String constraints are added during expr_to_z3.
    let has_string_constraints_cell = std::cell::Cell::new(has_string_constraints_cell_pre.get());
    let vc = VCtx {
        ctx: &ctx,
        module_env,
        current_atom: Some(atom),
        linearity_ctx: Some(&linearity_ctx_cell),
        effect_ctx: Some(&effect_ctx_cell),
        constraint_count: Some(&constraint_count_cell),
        constraint_budget: DEFAULT_CONSTRAINT_BUDGET,
        has_string_constraints: Some(&has_string_constraints_cell),
        path_cond_stack: std::cell::RefCell::new(Vec::new()),
    };

    let mut env: Env = HashMap::new();

    // Plan 9: Pre-register parameters with correct Z3 Sort based on their base type.
    // Without this, Str-typed parameters without refinement types would be lazily
    // created as Int in expr_to_z3's Variable fallback, causing string operations
    // (concatenation, equality) to silently produce incorrect verification results.
    // This matches the treatment in verify_atom_invariant (line 3206-3218).
    for param in &atom.params {
        let var = param_z3_value(
            &ctx,
            param.name.as_str(),
            param.type_name.as_deref(),
            module_env,
        );
        env.insert(param.name.clone(), var);
    }

    // Phase 1h (continued): ConflictingMerge Z3 infrastructure.
    // With Phase 4c Copy/Move type distinction integrated, move violations for
    // Move types are now hard errors (returned above). This loop registers Z3
    // variables for any remaining conflict locals as infrastructure for future
    // ownership constraint integration.
    for (local, block_id) in &move_conflict_locals {
        let var_name = format!("__move_conflict_{}_{}", local.0, block_id);
        let conflict_var = Bool::new_const(&ctx, var_name.as_str());
        solver.assert(&conflict_var);
    }

    // Phase 2b: エフェクト許可セットを Z3 環境に注入
    {
        let allowed_effects = module_env.resolve_effect_set_from_effects(&atom.effects);
        for effect_name in &allowed_effects {
            let allowed_name = format!("__effect_allowed_{}", effect_name);
            env.insert(allowed_name, Bool::from_bool(&ctx, true).into());
        }
    }

    // 1. 量子化制約の処理
    for (q_index, q) in atom.forall_constraints.iter().enumerate() {
        let i = Int::new_const(&ctx, q.var.as_str());
        // Parse start as an expression (supports identifiers, arithmetic, e.g. "n - 1")
        let start = if let Ok(val) = q.start.parse::<i64>() {
            Int::from_i64(&ctx, val)
        } else {
            let ast = parse_expression(&q.start);
            expr_to_z3(&vc, &ast, &mut env, None)?
                .as_int()
                .unwrap_or(Int::new_const(&ctx, q.start.as_str()))
        };
        let end = if let Ok(val) = q.end.parse::<i64>() {
            Int::from_i64(&ctx, val)
        } else {
            // Parse end as expression to support `n - 1` etc.
            let ast = parse_expression(&q.end);
            expr_to_z3(&vc, &ast, &mut env, None)?
                .as_int()
                .unwrap_or(Int::new_const(&ctx, q.end.as_str()))
        };

        let range_cond = Bool::and(&ctx, &[&i.ge(&start), &i.lt(&end)]);
        let expr_ast = parse_expression(&q.condition);
        let condition_z3 = expr_to_z3(&vc, &expr_ast, &mut env, None)?
            .as_bool()
            .ok_or(MumeiError::verification_at(
                "Condition must be boolean",
                atom.span.clone(),
            ))?;

        // Extract `arr[<idx>]` sub-expressions from the forall condition so we
        // can (1) propagate them as explicit Z3 quantifier patterns for
        // E-matching, and (2) expose a "length >= max_index + 1" assumption so
        // downstream ArrayAccess bounds-checks do not flag out-of-bounds on
        // indices that the user has already certified as valid via the forall.
        let arr_accesses = collect_array_accesses(&expr_ast);

        let body = range_cond.implies(&condition_z3);
        let body_exists = Bool::and(&ctx, &[&range_cond, &condition_z3]);

        let quantifier_expr = match q.q_type {
            QuantifierType::ForAll => {
                let mut pattern_asts: Vec<Dynamic> = Vec::new();
                for (arr_name, idx_expr) in &arr_accesses {
                    if let Ok(idx_z3) = expr_to_z3(&vc, idx_expr, &mut env, None) {
                        if let Some(idx_int) = idx_z3.as_int() {
                            pattern_asts
                                .push(z3_dynamic_array(&vc, arr_name, &env).select(&idx_int));
                        }
                    }
                }
                if pattern_asts.is_empty() {
                    z3::ast::forall_const(&ctx, &[&i], &[], &body)
                } else {
                    let pattern_refs: Vec<&dyn z3::ast::Ast> = pattern_asts
                        .iter()
                        .map(|d| d as &dyn z3::ast::Ast)
                        .collect();
                    let pattern = z3::Pattern::new(&ctx, &pattern_refs);
                    z3::ast::forall_const(&ctx, &[&i], &[&pattern], &body)
                }
            }
            QuantifierType::Exists => z3::ast::exists_const(&ctx, &[&i], &[], &body_exists),
        };
        let track_label = format!("track_quantifier_{}", q_index);
        let track_bool = Bool::new_const(&ctx, track_label.as_str());
        solver.assert_and_track(&quantifier_expr, &track_bool);

        // Assert a length bound for each array referenced in the forall
        // condition so that ArrayAccess's OOB check does not fail on
        // indices that the forall already certifies as valid. For each
        // `<name>[idx_expr]` under `forall i ∈ [start, end)`, we assert
        // `len_<name> > idx_expr` under the same quantifier. With
        // pattern-matching on `arr[idx]`, Z3 can then conclude
        // `idx < len_<name>` at body usage.
        if q.q_type == QuantifierType::ForAll && !arr_accesses.is_empty() {
            for (access_index, (arr_name, idx_expr)) in arr_accesses.iter().enumerate() {
                // Look up / create the length constant under the same
                // `len_<name>` convention used by `expr_to_z3`'s
                // `Expr::ArrayAccess` OOB check, so the bound we assert here
                // lines up with the check downstream. Previously this was
                // hard-coded to `len_arr`, which happened to work because
                // every std atom today binds a single `arr`, but would silently
                // mis-bind bounds once another array (e.g. `data`, `aux`) is
                // used in a forall condition.
                let len_name = format!("len_{}", arr_name);
                let len_var = if let Some(existing) = env.get(&len_name) {
                    existing
                        .as_int()
                        .unwrap_or_else(|| Int::new_const(&ctx, len_name.as_str()))
                } else {
                    let l = Int::new_const(&ctx, len_name.as_str());
                    solver.assert(&l.ge(&Int::from_i64(&ctx, 0)));
                    env.insert(len_name.clone(), l.clone().into());
                    l
                };
                if let Ok(idx_z3) = expr_to_z3(&vc, idx_expr, &mut env, None) {
                    if let Some(idx_int) = idx_z3.as_int() {
                        let body = range_cond.implies(&Bool::and(
                            &ctx,
                            &[&idx_int.ge(&Int::from_i64(&ctx, 0)), &idx_int.lt(&len_var)],
                        ));
                        let pattern_ast = z3_dynamic_array(&vc, arr_name, &env).select(&idx_int);
                        let pattern_refs: Vec<&dyn z3::ast::Ast> =
                            vec![&pattern_ast as &dyn z3::ast::Ast];
                        let pattern = z3::Pattern::new(&ctx, &pattern_refs);
                        let len_forall = z3::ast::forall_const(&ctx, &[&i], &[&pattern], &body);
                        // Include `access_index` so that each arr[..] access
                        // in a multi-access forall (e.g. `arr[i] <= arr[i + 1]`)
                        // gets a distinct unsat-core tracking label.
                        let track_label = format!(
                            "track_quantifier_{}_{}_len_bound_{}",
                            q_index, arr_name, access_index
                        );
                        let track_bool = Bool::new_const(&ctx, track_label.as_str());
                        solver.assert_and_track(&len_forall, &track_bool);
                    }
                }
            }
        }
    }

    // 2. 引数（params）に対する精緻型制約の自動適用
    for param in &atom.params {
        if let Some(type_name) = &param.type_name {
            if let Some(refined) = module_env.get_type(type_name) {
                apply_refinement_constraint(&vc, &solver, &param.name, refined, &mut env)?;
            }
        }
    }

    // 2b. 引数（params）に対する構造体フィールド制約の自動適用
    for param in &atom.params {
        if let Some(type_name) = &param.type_name {
            if let Some(sdef) = module_env.get_struct(type_name) {
                // 構造体の各フィールドをシンボリック変数として env に登録し、制約を適用
                for field in &sdef.fields {
                    let field_var_name = format!("{}_{}", param.name, field.name);
                    let base = module_env.resolve_base_type(&field.type_name);
                    let field_z3: Dynamic = match base.as_str() {
                        "f64" => Float::new_const(&ctx, field_var_name.as_str(), 11, 53).into(),
                        // Plan 9: Str fields as Z3 String Sort
                        "Str" => Z3String::new_const(&ctx, field_var_name.as_str()).into(),
                        _ => Int::new_const(&ctx, field_var_name.as_str()).into(),
                    };
                    env.insert(field_var_name.clone(), field_z3.clone());
                    // qualified name も登録
                    let qualified = format!("__struct_{}_{}", param.name, field.name);
                    env.insert(qualified, field_z3.clone());

                    // フィールド制約を solver に assert
                    if let Some(constraint_raw) = &field.constraint {
                        let mut local_env = env.clone();
                        local_env.insert("v".to_string(), field_z3);
                        let constraint_ast = parse_expression(constraint_raw);
                        let constraint_z3 = expr_to_z3(&vc, &constraint_ast, &mut local_env, None)?;
                        if let Some(constraint_bool) = constraint_z3.as_bool() {
                            let track_label =
                                format!("track_struct_field_{}::{}", param.name, field.name);
                            let track_bool = Bool::new_const(&ctx, track_label.as_str());
                            solver.assert_and_track(&constraint_bool, &track_bool);
                        }
                    }
                }
            }
        }
    }

    // 2c. 全パラメータに対して配列長シンボルを事前生成
    #[allow(clippy::map_entry)]
    for param in &atom.params {
        let len_name = format!("len_{}", param.name);
        if !env.contains_key(&len_name) {
            let len_var = Int::new_const(&ctx, len_name.as_str());
            solver.assert(&len_var.ge(&Int::from_i64(&ctx, 0)));
            env.insert(len_name, len_var.into());
        }
    }

    // 2d. 線形性チェック: consumed_params + ref パラメータの Z3 シンボリック Bool 連携
    // consume 宣言されたパラメータに対して is_alive フラグを Z3 上で追跡する。
    // ref パラメータに対しては借用カウントを追跡し、借用中の consume を禁止する。
    // linearity_ctx_cell is shared with VCtx (created above) via RefCell.

    // consume 対象パラメータの登録
    if !atom.consumed_params.is_empty() {
        for param_name in &atom.consumed_params {
            // パラメータが実際に存在するか検証
            if !atom.params.iter().any(|p| p.name == *param_name) {
                return Err(MumeiError::type_error_at(
                    format!(
                        "consume target '{}' is not a parameter of atom '{}'",
                        param_name, atom.name
                    ),
                    atom.span.clone(),
                ));
            }
            // ref / ref mut パラメータは consume できない
            if atom
                .params
                .iter()
                .any(|p| p.name == *param_name && (p.is_ref || p.is_ref_mut))
            {
                let kind = if atom
                    .params
                    .iter()
                    .any(|p| p.name == *param_name && p.is_ref_mut)
                {
                    "ref mut"
                } else {
                    "ref"
                };
                return Err(MumeiError::type_error_at(
                    format!("Cannot consume {} parameter '{}' in atom '{}': {} parameters are borrowed, not owned", kind, param_name, atom.name, kind),
                    atom.span.clone()
                ));
            }
            // LinearityCtx に登録
            linearity_ctx_cell.borrow_mut().register(param_name);

            // Z3 上で is_alive シンボリック Bool を作成し、初期値 true を assert
            let alive_name = format!("__alive_{}", param_name);
            let alive_bool = Bool::new_const(&ctx, alive_name.as_str());
            solver.assert(&alive_bool); // 初期状態: alive = true
            env.insert(alive_name, alive_bool.into());
        }
    }

    // ref / ref mut パラメータの借用登録
    // ref パラメータは読み取り専用で貸し出される。
    // ref mut パラメータは排他的な書き込み参照として貸し出される。
    // 借用中は元の所有者（呼び出し元）が consume/free できない。
    // この制約は呼び出し元の verify() で検証される（Compositional Verification）。
    for param in &atom.params {
        if param.is_ref || param.is_ref_mut {
            // ref/ref mut パラメータを LinearityCtx に登録（借用として）
            linearity_ctx_cell.borrow_mut().register(&param.name);

            // Z3 上で borrowed フラグを作成
            let borrowed_name = format!("__borrowed_{}", param.name);
            let borrowed_bool = Bool::new_const(&ctx, borrowed_name.as_str());
            solver.assert(&borrowed_bool); // 借用中: true
            env.insert(borrowed_name, borrowed_bool.into());

            // ref/ref mut パラメータは consume 不可であることを Z3 で表現
            // __alive_{name} は常に true（借用中は解放不可）
            let alive_name = format!("__alive_{}", param.name);
            let alive_bool = Bool::new_const(&ctx, alive_name.as_str());
            solver.assert(&alive_bool); // ref は常に alive
            env.insert(alive_name, alive_bool.into());

            // ref mut の場合: 排他的アクセス（exclusive）を Z3 で表現
            if param.is_ref_mut {
                let exclusive_name = format!("__exclusive_{}", param.name);
                let exclusive_bool = Bool::new_const(&ctx, exclusive_name.as_str());
                solver.assert(&exclusive_bool); // exclusive = true
                env.insert(exclusive_name, exclusive_bool.into());
            }
        }
    }

    // 3. 前提条件 (requires)
    // NOTE: requires は エイリアシング検証より先に assert する必要がある。
    // requires: x != y; のような制約がエイリアシング検証で活用されるため。
    if atom.requires.trim() != "true" {
        let req_ast = parse_expression(&atom.requires);
        let req_z3 = expr_to_z3(&vc, &req_ast, &mut env, None)?;
        if let Some(req_bool) = req_z3.as_bool() {
            let track_requires = Bool::new_const(&ctx, "track_requires");
            solver.assert_and_track(&req_bool, &track_requires);
        }
    }

    // 3b. エイリアシング検証 (Aliasing Prevention)
    // requires が assert された後に実行する。
    // これにより requires: x != y; のような制約が Z3 で活用され、
    // 「provably distinct」なパラメータはエイリアシングエラーにならない。
    //
    // ref mut パラメータが存在する場合、同じ型の他の ref/ref mut パラメータ
    // とのエイリアシング（同一データへの複数参照）を禁止する。
    //
    // Rust の借用規則と同等:
    // - &mut T が存在する場合、同じデータへの &T も &mut T も存在できない
    // - &T は複数同時に存在可能
    //
    // Z3 制約:
    // ∀ p1, p2 ∈ params:
    //   p1.is_ref_mut ∧ p1.type == p2.type ∧ p1 ≠ p2
    //   → ¬(p2.is_ref ∨ p2.is_ref_mut)  // エイリアシング禁止
    {
        let ref_mut_params: Vec<&crate::parser::Param> =
            atom.params.iter().filter(|p| p.is_ref_mut).collect();

        for ref_mut_p in &ref_mut_params {
            for other_p in &atom.params {
                if other_p.name == ref_mut_p.name {
                    continue; // 自分自身はスキップ
                }
                // 同じ型の ref または ref mut パラメータがある場合、エイリアシングの可能性
                if (other_p.is_ref || other_p.is_ref_mut)
                    && other_p.type_name == ref_mut_p.type_name
                {
                    // Z3 で同一データへの参照でないことを検証
                    // パラメータが異なる値を持つことを確認
                    // （同じ値を持つ場合、エイリアシングが発生）
                    if let (Some(ref_mut_val), Some(other_val)) =
                        (env.get(&ref_mut_p.name), env.get(&other_p.name))
                    {
                        if let (Some(rm_int), Some(ot_int)) =
                            (ref_mut_val.as_int(), other_val.as_int())
                        {
                            // ref_mut_val == other_val が SAT ならエイリアシングの可能性あり
                            solver.push();
                            solver.assert(&rm_int._eq(&ot_int));
                            if solver.check() == SatResult::Sat {
                                solver.pop(1);
                                let other_kind = if other_p.is_ref_mut { "ref mut" } else { "ref" };
                                return Err(MumeiError::verification_at(
                                    format!(
                                        "Aliasing violation in atom '{}': \
                                         'ref mut {}' and '{} {}' may reference the same data (type: {}). \
                                         A mutable reference requires exclusive access — \
                                         no other references to the same data are allowed.\n  \
                                         Hint: Use different types, or ensure the values are provably distinct \
                                         via requires.",
                                        atom.name, ref_mut_p.name, other_kind, other_p.name,
                                        ref_mut_p.type_name.as_deref().unwrap_or("unknown")
                                    ),
                                    atom.span.clone()
                                ));
                            }
                            solver.pop(1);
                        }
                    }
                }
            }
        }
    }

    // 4. ボディの検証
    let phase_start = std::time::Instant::now();
    let body_result = match stmt_to_z3(&vc, &hir_atom.body_stmt, &mut env, Some(&solver)) {
        Ok(val) => val,
        Err(e) => {
            // Body evaluation errors (e.g., division by zero, out-of-bounds) propagate
            // before reaching the postcondition check. Write a failure report so the
            // MCP self-healing flow does not read a stale report.json from a prior run.
            let err_str = format!("{}", e);
            // If this is an effect mismatch violation, save a structured report
            if err_str.contains("Effect violation: 'perform ") {
                // Extract effect name and operation from error message
                // Format: "Effect violation: 'perform Effect.op' requires [Effect] effect, ..."
                if let Some(start) = err_str.find("requires [") {
                    let after = &err_str[start + 10..];
                    if let Some(end) = after.find(']') {
                        let required_effect = &after[..end];
                        let source_op = err_str
                            .find("'perform ")
                            .and_then(|s| {
                                let rest = &err_str[s + 9..];
                                rest.find('\'').map(|e| rest[..e].to_string())
                            })
                            .unwrap_or_default();
                        let effect_names: Vec<String> =
                            atom.effects.iter().map(|e| e.name.clone()).collect();
                        save_effect_violation_report(
                            output_dir,
                            &atom.name,
                            &effect_names,
                            required_effect,
                            &source_op,
                            &[
                                format!("Add '{}' to the effects declaration", required_effect),
                                format!("Remove the call to 'perform {}'", source_op),
                            ],
                        );
                    }
                }
            }
            // Determine failure type: division-by-zero and budget exceeded get their own categories
            let body_failure_type = if err_str.contains("division by zero") {
                FAILURE_DIVISION_BY_ZERO
            } else if err_str.contains("Constraint budget exceeded") {
                "constraint_budget_exceeded"
            } else {
                FAILURE_PRECONDITION_VIOLATED
            };
            let constraint_mappings = build_constraint_mappings_for_atom(atom, module_env);
            let semantic_fb =
                build_semantic_feedback(&constraint_mappings, None, atom, body_failure_type, None);
            save_visualizer_report(
                output_dir,
                "failed",
                &atom.name,
                "N/A",
                "N/A",
                &err_str,
                None,
                body_failure_type,
                semantic_fb.as_ref(),
                Some(&atom.span),
                Some(&constraint_mappings),
            );
            return Err(e);
        }
    };

    metrics.record_phase("Phase 4: body evaluation", phase_start.elapsed());

    // 4b. Taint Analysis: unverified 関数の呼び出しを検出し警告
    check_taint_propagation(atom, &hir_atom.body_stmt, &env, module_env);

    // 5. 事後条件 (ensures)
    let phase_start = std::time::Instant::now();
    if atom.ensures.trim() != "true" {
        env.insert("result".to_string(), body_result);
        let ens_ast = parse_expression(&atom.ensures);
        let ens_z3 = expr_to_z3(&vc, &ens_ast, &mut env, None)?;
        if let Some(ens_bool) = ens_z3.as_bool() {
            solver.push();
            solver.assert(&ens_bool.not());
            let ensures_check = solver.check();
            if ensures_check == SatResult::Sat {
                // Extract counterexample from Z3 model
                let (ce_a, ce_b, ce_value) = if let Some(model) = solver.get_model() {
                    let mut ce_json = serde_json::Map::new();
                    for param in &atom.params {
                        if let Some(var_z3) = env.get(&param.name) {
                            if let Some(val) = model.eval(var_z3, true) {
                                let val_str = format!("{}", val);
                                ce_json.insert(param.name.clone(), json!(val_str));
                            }
                        }
                    }
                    let a_str = ce_json
                        .get(atom.params.first().map(|p| p.name.as_str()).unwrap_or(""))
                        .and_then(|v| v.as_str())
                        .unwrap_or("N/A")
                        .to_string();
                    let b_str = ce_json
                        .get(atom.params.get(1).map(|p| p.name.as_str()).unwrap_or(""))
                        .and_then(|v| v.as_str())
                        .unwrap_or("N/A")
                        .to_string();
                    let ce_val = if ce_json.is_empty() {
                        None
                    } else {
                        Some(serde_json::Value::Object(ce_json))
                    };
                    if enable_spurious_detection {
                        let mut model_map = HashMap::new();
                        for (name, var_z3) in &env {
                            if let Some(val) = model.eval(var_z3, true) {
                                if let Some(int_value) = z3_dynamic_to_i64(&val) {
                                    model_map.insert(name.clone(), int_value);
                                }
                            }
                        }
                        let validation = validate_counterexample(atom, &model_map, module_env);
                        if validation.validation_status == "spurious_candidate" {
                            solver.pop(1);
                            let symbols = validation
                                .symbol_provenance
                                .iter()
                                .map(|symbol| format!("{} ({})", symbol.symbol_name, symbol.source))
                                .collect::<Vec<_>>()
                                .join(", ");
                            return Err(MumeiError::verification_at(
                                format!(
                                    "Spurious counterexample detected for atom '{}'",
                                    atom.name
                                ),
                                atom.span.clone(),
                            )
                            .with_help(format!(
                                "Counterexample depends on uninterpreted symbols: {}. Consider escalating to Lean or expanding the symbol.",
                                symbols
                            ))
                            .with_counterexample(ce_val.clone()));
                        }
                    }
                    (a_str, b_str, ce_val)
                } else {
                    ("N/A".to_string(), "N/A".to_string(), None)
                };
                solver.pop(1);
                let constraint_mappings = build_constraint_mappings_for_atom(atom, module_env);
                let semantic_fb = build_semantic_feedback(
                    &constraint_mappings,
                    ce_value.as_ref(),
                    atom,
                    FAILURE_POSTCONDITION_VIOLATED,
                    None,
                );
                save_visualizer_report(
                    output_dir,
                    "failed",
                    &atom.name,
                    &ce_a,
                    &ce_b,
                    "Postcondition violated.",
                    ce_value.as_ref(),
                    FAILURE_POSTCONDITION_VIOLATED,
                    semantic_fb.as_ref(),
                    Some(&atom.span),
                    Some(&constraint_mappings),
                );
                metrics.record_phase(
                    "Phase 5: ensures verification (failed)",
                    phase_start.elapsed(),
                );
                metrics.total_constraints = constraint_count_cell.get();
                metrics.print_summary();
                // Feature 3d: Add related spans for constraint definition locations
                let mut err = MumeiError::verification_at(
                    "Postcondition (ensures) is not satisfied.",
                    atom.span.clone(),
                )
                .with_help("ensures の条件を確認してください。body の返り値が事後条件を満たすか検討してください")
                .with_counterexample(ce_value.clone());
                for mapping in &constraint_mappings {
                    if mapping.span.line > 0 {
                        let related_src_span = span_to_source_span("", &mapping.span);
                        err = err.with_related(
                            related_src_span,
                            format!("constraint on '{}' defined here", mapping.param_name),
                            miette::NamedSource::new(
                                if mapping.span.file.is_empty() {
                                    "<unknown>"
                                } else {
                                    &mapping.span.file
                                },
                                String::new(),
                            ),
                            format!("type constraint: {}", mapping.predicate_raw),
                            mapping.span.clone(),
                        );
                    }
                }
                return Err(err);
            }
            if ensures_check == SatResult::Unknown {
                solver.pop(1);
                metrics.record_phase(
                    "Phase 5: ensures verification (unknown)",
                    phase_start.elapsed(),
                );
                metrics.total_constraints = constraint_count_cell.get();
                metrics.print_summary();
                return Err(MumeiError::verification_at(
                    "Z3 returned unknown while checking the postcondition.",
                    atom.span.clone(),
                ));
            }
            solver.pop(1);
        }
        env.remove("result");
    }

    // 5b. 線形性チェック: consume 対象パラメータの検証
    // body 実行後、consume 宣言されたパラメータが正しく消費されていることを確認。
    // LinearityCtx に蓄積された違反（二重解放・Use-After-Free）があればエラー。
    if !atom.consumed_params.is_empty() {
        // consume 対象パラメータを消費済みとしてマーク
        for param_name in &atom.consumed_params {
            if let Err(e) = linearity_ctx_cell.borrow_mut().consume(param_name) {
                return Err(MumeiError::verification_at(
                    format!("Linearity violation in atom '{}': {}", atom.name, e),
                    atom.span.clone(),
                ));
            }

            // Z3 上で is_alive を false に更新（消費後のアクセスを禁止）
            let alive_name = format!("__alive_{}", param_name);
            let alive_false = Bool::from_bool(&ctx, false);
            env.insert(alive_name, alive_false.into());
        }

        // 蓄積された違反をチェック
        let lctx_guard = linearity_ctx_cell.borrow();
        if lctx_guard.has_violations() {
            let violations_list = lctx_guard.get_violations();
            let violations = violations_list.join("\n  ");
            let linearity_fb = build_linearity_feedback(&atom.name, violations_list, &atom.span);
            save_visualizer_report(
                output_dir,
                "failed",
                &atom.name,
                "N/A",
                "N/A",
                &format!("Linearity violations in atom '{}'", atom.name),
                None,
                FAILURE_LINEARITY_VIOLATED,
                Some(&linearity_fb),
                Some(&atom.span),
                None,
            );
            return Err(MumeiError::verification_at(
                format!(
                    "Linearity violations in atom '{}':\n  {}",
                    atom.name, violations
                ),
                atom.span.clone(),
            ));
        }
    }

    metrics.record_phase("Phase 5: ensures verification", phase_start.elapsed());

    let z3_check_start = std::time::Instant::now();
    let final_check = solver.check();
    if final_check == SatResult::Unsat {
        let unsat_core = solver.get_unsat_core();
        let core_labels: Vec<String> = unsat_core
            .iter()
            .map(|b| normalize_tracking_label(&b.decl().name()))
            .collect();
        let minimal_core = extract_minimal_unsat_core(&solver, &core_labels, &ctx);

        let structured_labels: Vec<StructuredLabel> = core_labels
            .iter()
            .filter_map(|label| parse_tracking_label(label))
            .collect();

        let conflicting_constraints: Vec<String> = structured_labels
            .iter()
            .map(|sl| sl.description.clone())
            .collect();

        let contradiction_fb = build_contradiction_feedback(
            &atom.name,
            &conflicting_constraints,
            &core_labels,
            &structured_labels,
            Some(&minimal_core),
        );

        save_visualizer_report(
            output_dir,
            "failed",
            &atom.name,
            "N/A",
            "N/A",
            "Logic contradiction.",
            None,
            FAILURE_INVARIANT_VIOLATED,
            Some(&contradiction_fb),
            Some(&atom.span),
            None,
        );

        let constraint_summary = if conflicting_constraints.is_empty() {
            "Contradiction found.".to_string()
        } else {
            format!(
                "Contradiction found. Conflicting constraints: [{}]",
                conflicting_constraints.join(", ")
            )
        };

        metrics.z3_check_time = z3_check_start.elapsed();
        metrics.total_constraints = constraint_count_cell.get();
        metrics.record_phase(
            "Phase 6: final Z3 check (contradiction)",
            z3_check_start.elapsed(),
        );
        metrics.print_summary();
        return Err(MumeiError::verification_at(
            constraint_summary,
            atom.span.clone(),
        ));
    }
    if final_check == SatResult::Unknown {
        metrics.z3_check_time = z3_check_start.elapsed();
        metrics.total_constraints = constraint_count_cell.get();
        metrics.record_phase(
            "Phase 6: final Z3 check (unknown)",
            z3_check_start.elapsed(),
        );
        metrics.print_summary();
        return Err(MumeiError::verification_at(
            "Z3 returned unknown during the final consistency check.",
            atom.span.clone(),
        ));
    }

    metrics.z3_check_time = z3_check_start.elapsed();
    metrics.total_constraints = constraint_count_cell.get();
    metrics.record_phase("Phase 6: final Z3 check", z3_check_start.elapsed());

    // Print metrics summary (always for now; future: gate behind --verbose)
    metrics.print_summary();

    save_visualizer_report(
        output_dir,
        "success",
        &atom.name,
        "N/A",
        "N/A",
        "Verified safe.",
        None,
        "",
        None,
        Some(&atom.span),
        None,
    );
    Ok(())
}

fn z3_dynamic_to_i64(value: &Dynamic) -> Option<i64> {
    value
        .as_int()
        .and_then(|int_value| int_value.as_i64())
        .or_else(|| format!("{}", value).parse::<i64>().ok())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn save_visualizer_report(
    output_dir: &Path,
    status: &str,
    name: &str,
    a: &str,
    b: &str,
    reason: &str,
    counterexample: Option<&serde_json::Value>,
    failure_type: &str,
    semantic_feedback: Option<&serde_json::Value>,
    span: Option<&Span>,
    constraint_mappings: Option<&[ConstraintMapping]>,
) {
    let mut report = json!({
        "status": status,
        "atom": name,
        "input_a": a,
        "input_b": b,
        "reason": reason
    });
    if !failure_type.is_empty() {
        report["failure_type"] = json!(failure_type);
    }
    if let Some(ce) = counterexample {
        report["counterexample"] = ce.clone();
    }
    if let Some(sf) = semantic_feedback {
        report["semantic_feedback"] = sf.clone();
    }
    // Use contextual suggestion when counterexample/unsat_core available, fallback to static
    let structured_unsat_core = semantic_feedback.and_then(|sf| sf.get("structured_unsat_core"));
    report["suggestion"] = json!(build_contextual_suggestion(
        failure_type,
        counterexample,
        structured_unsat_core,
    ));
    if let Some(s) = span {
        report["span"] = json!({
            "file": s.file,
            "line": s.line,
            "col": s.col,
            "len": s.len
        });
    }
    // Include constraint source locations from constraint mappings (Feature 2f)
    if let Some(mappings) = constraint_mappings {
        let type_locations: Vec<serde_json::Value> = mappings
            .iter()
            .filter(|m| m.span.line > 0)
            .map(|m| {
                json!({
                    "param": m.param_name,
                    "type": m.type_name.as_deref().unwrap_or(&m.base_type),
                    "file": m.span.file,
                    "line": m.span.line,
                    "col": m.span.col
                })
            })
            .collect();
        if !type_locations.is_empty() {
            report["type_definition_locations"] = json!(type_locations);
        }
    }
    let _ = fs::create_dir_all(output_dir);
    let _ = fs::write(output_dir.join("report.json"), report.to_string());
}
