use crate::cli::Command;
use crate::feedback::*;
use crate::pipeline::*;
use mumei_core::hir::lower_atom_to_hir_with_env;
use mumei_core::parser::Item;
use mumei_core::{
    cross_spec, manifest, mir, mir_analysis, parser, proof_cert,
    reconstruction_loss::ReconstructionLoss, resolver, structured_feedback::StructuredFeedback,
    verification,
};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

pub(crate) fn cmd_verify_command(command: Command) {
    let Command::Verify {
        input,
        task_id,
        solver_timeout,
        cache_scope,
        proof_cert,
        escalate_lean,
        emit,
        no_emit,
        output,
        report_dir,
        json,
        strict_imports,
        allow_lean_verified,
        cross_spec_verify,
        cross_spec_files,
        enable_spurious_detection,
        disable_spurious_detection,
        property_based_test,
        warn_fragment,
        property_based_test_count,
        property_based_test_seed,
        property_based_test_max_shrink_steps,
        harness_contract,
        intent_fidelity,
        artifact_paths,
        budget_policy_fingerprint,
        emit_contract_manifest,
        enable_vacuity_check,
        detect_loops,
        suggest_cegis,
        detect_spec_drift,
    } = command
    else {
        unreachable!("cmd_verify_command called with non-verify command");
    };
    let allow_lean_verified = allow_lean_verified || escalate_lean;
    let no_emit_escalation_metrics = no_emit.iter().any(|target| target == "escalation-metrics");
    if let Some(other) = no_emit
        .iter()
        .find(|target| target.as_str() != "escalation-metrics")
    {
        eprintln!(
            "Unsupported verify --no-emit target '{}'. Supported values: escalation-metrics",
            other
        );
        std::process::exit(1);
    }
    let emit_escalation_bundle = matches!(emit.as_deref(), Some("escalation-bundle"));
    let emit_escalation_metrics =
        matches!(emit.as_deref(), Some("escalation-metrics")) && !no_emit_escalation_metrics;
    let emit_decidable_metrics = matches!(emit.as_deref(), Some("decidable-metrics"));
    let emit_reconstruction_loss = matches!(emit.as_deref(), Some("reconstruction-loss"));
    let emit_loss_vector = matches!(emit.as_deref(), Some("loss-vector"));
    let emit_structured_feedback = matches!(emit.as_deref(), Some("structured-feedback"));
    let emit_human_review_queue = matches!(emit.as_deref(), Some("human-review-queue"));
    if let Some(other) = emit.as_deref() {
        if !emit_escalation_bundle
            && !matches!(other, "escalation-metrics")
            && !emit_decidable_metrics
            && !emit_reconstruction_loss
            && !emit_loss_vector
            && !emit_structured_feedback
            && !emit_human_review_queue
        {
            eprintln!(
                    "Unsupported verify --emit target '{}'. Supported values: escalation-bundle, escalation-metrics, decidable-metrics, reconstruction-loss, loss-vector, structured-feedback, human-review-queue",
                    other
                );
            std::process::exit(1);
        }
    }
    let harness_contract = resolve_harness_contract(harness_contract);
    let intent_fidelity = resolve_intent_fidelity(intent_fidelity);
    let artifact_paths = resolve_artifact_paths(artifact_paths);
    let budget_policy_fingerprint = resolve_budget_policy_fingerprint(budget_policy_fingerprint);
    let enable_cross_spec = cross_spec_verify || !cross_spec_files.is_empty();
    let enable_spurious = enable_spurious_detection || !disable_spurious_detection;
    let input_path = Path::new(&input);
    if input_path.is_dir() {
        let mut files = collect_mm_files(input_path);
        files.sort();
        if files.is_empty() {
            eprintln!("❌ No .mm files found in '{}'", input);
            std::process::exit(1);
        }
        println!(
            "🗡️  Mumei verify: verifying {} file(s) in '{}'...",
            files.len(),
            input
        );
        let mut total_ok = 0usize;
        let mut total_fail = 0usize;
        for file in &files {
            let file_str = file.to_string_lossy().to_string();
            let has_failure = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                cmd_verify(VerifyOptions {
                    input: &file_str,
                    task_id: task_id.as_deref(),
                    solver_timeout,
                    cache_scope: &cache_scope,
                    generate_proof_cert: proof_cert,
                    escalate_lean,
                    emit_escalation_bundle,
                    emit_escalation_metrics,
                    emit_decidable_metrics,
                    emit_reconstruction_loss,
                    emit_loss_vector,
                    emit_structured_feedback,
                    emit_human_review_queue,
                    cert_output: output.as_deref(),
                    report_dir: report_dir.as_deref(),
                    json_output: json,
                    strict_imports,
                    allow_lean_verified,
                    enable_cross_spec_verification: enable_cross_spec,
                    cross_spec_files: &cross_spec_files,
                    enable_spurious_detection: enable_spurious,
                    property_based_test,
                    warn_fragment,
                    property_based_test_count,
                    property_based_test_seed,
                    property_based_test_max_shrink_steps,
                    harness_contract: harness_contract.clone(),
                    intent_fidelity: intent_fidelity.clone(),
                    artifact_paths: artifact_paths.clone(),
                    budget_policy_fingerprint: budget_policy_fingerprint.clone(),
                    emit_contract_manifest,
                    enable_vacuity_check,
                    detect_loops,
                    suggest_cegis,
                })
            })) {
                Ok(has_failure) => has_failure,
                Err(_) => {
                    eprintln!("  ❌ '{}': parse error (panic)", file_str);
                    true
                }
            };
            if has_failure {
                total_fail += 1;
            } else {
                total_ok += 1;
            }
        }
        println!(
            "\n🗡️  Directory verify summary: {} passed, {} failed",
            total_ok, total_fail
        );
        if total_fail > 0 {
            std::process::exit(1);
        }
    } else {
        let has_failure = cmd_verify(VerifyOptions {
            input: &input,
            task_id: task_id.as_deref(),
            solver_timeout,
            cache_scope: &cache_scope,
            generate_proof_cert: proof_cert,
            escalate_lean,
            emit_escalation_bundle,
            emit_escalation_metrics,
            emit_decidable_metrics,
            emit_reconstruction_loss,
            emit_loss_vector,
            emit_structured_feedback,
            emit_human_review_queue,
            cert_output: output.as_deref(),
            report_dir: report_dir.as_deref(),
            json_output: json,
            strict_imports,
            allow_lean_verified,
            enable_cross_spec_verification: enable_cross_spec,
            cross_spec_files: &cross_spec_files,
            enable_spurious_detection: enable_spurious,
            property_based_test,
            warn_fragment,
            property_based_test_count,
            property_based_test_seed,
            property_based_test_max_shrink_steps,
            harness_contract,
            intent_fidelity,
            artifact_paths,
            budget_policy_fingerprint,
            emit_contract_manifest,
            enable_vacuity_check,
            detect_loops,
            suggest_cegis,
        });
        // --detect-spec-drift: compare old cert with newly generated cert
        if let Some(ref old_cert_path) = detect_spec_drift {
            let old_cert_file = std::fs::read_to_string(old_cert_path);
            match old_cert_file {
                Ok(old_json) => {
                    let old_cert_parsed: Result<proof_cert::ProofCertificate, _> =
                        serde_json::from_str(&old_json);
                    match old_cert_parsed {
                        Ok(old_cert) => {
                            // Determine the path to the new cert
                            let new_cert_path = if let Some(ref out) = output {
                                PathBuf::from(out)
                            } else {
                                let stem = Path::new(&input)
                                    .file_stem()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .to_string();
                                PathBuf::from(format!("{}.proof.json", stem))
                            };
                            if let Ok(new_json) = std::fs::read_to_string(&new_cert_path) {
                                if let Ok(new_cert) =
                                    serde_json::from_str::<proof_cert::ProofCertificate>(&new_json)
                                {
                                    let drift_report =
                                        cross_spec::drift_detector::detect_spec_drift(
                                            &old_cert, &new_cert,
                                        );
                                    if drift_report.drift_detected {
                                        eprintln!("⚠️  Spec drift detected:");
                                        if !drift_report.changed_atoms.is_empty() {
                                            eprintln!(
                                                "  Changed: {:?}",
                                                drift_report.changed_atoms
                                            );
                                        }
                                        if !drift_report.new_atoms.is_empty() {
                                            eprintln!("  New: {:?}", drift_report.new_atoms);
                                        }
                                        if !drift_report.removed_atoms.is_empty() {
                                            eprintln!(
                                                "  Removed: {:?}",
                                                drift_report.removed_atoms
                                            );
                                        }
                                        let drift_json =
                                            serde_json::to_string_pretty(&drift_report)
                                                .unwrap_or_default();
                                        eprintln!("{}", drift_json);
                                    } else {
                                        eprintln!("✅ No spec drift detected.");
                                    }
                                } else {
                                    eprintln!("⚠️  --detect-spec-drift: could not parse new proof certificate at {:?}", new_cert_path);
                                }
                            } else {
                                eprintln!("⚠️  --detect-spec-drift: no new proof certificate found at {:?}. Use --proof-cert to generate one.", new_cert_path);
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "⚠️  --detect-spec-drift: failed to parse old certificate: {}",
                                e
                            );
                        }
                    }
                }
                Err(e) => {
                    eprintln!(
                        "⚠️  --detect-spec-drift: failed to read old certificate {}: {}",
                        old_cert_path, e
                    );
                }
            }
        }
        if has_failure {
            std::process::exit(1);
        }
    }
}

pub(crate) struct VerifyOptions<'a> {
    pub(crate) input: &'a str,
    pub(crate) task_id: Option<&'a str>,
    pub(crate) solver_timeout: Option<u64>,
    pub(crate) cache_scope: &'a str,
    pub(crate) generate_proof_cert: bool,
    pub(crate) escalate_lean: bool,
    pub(crate) emit_escalation_bundle: bool,
    pub(crate) emit_escalation_metrics: bool,
    pub(crate) emit_decidable_metrics: bool,
    pub(crate) emit_reconstruction_loss: bool,
    pub(crate) emit_loss_vector: bool,
    pub(crate) emit_structured_feedback: bool,
    pub(crate) emit_human_review_queue: bool,
    pub(crate) cert_output: Option<&'a str>,
    pub(crate) report_dir: Option<&'a str>,
    pub(crate) json_output: bool,
    pub(crate) strict_imports: bool,
    pub(crate) allow_lean_verified: bool,
    pub(crate) enable_cross_spec_verification: bool,
    pub(crate) cross_spec_files: &'a [String],
    pub(crate) enable_spurious_detection: bool,
    pub(crate) property_based_test: bool,
    pub(crate) warn_fragment: bool,
    pub(crate) property_based_test_count: usize,
    pub(crate) property_based_test_seed: Option<u64>,
    pub(crate) property_based_test_max_shrink_steps: usize,
    pub(crate) harness_contract: Option<String>,
    pub(crate) intent_fidelity: Option<proof_cert::IntentFidelity>,
    pub(crate) artifact_paths: Option<Vec<String>>,
    pub(crate) budget_policy_fingerprint: Option<String>,
    pub(crate) emit_contract_manifest: bool,
    pub(crate) enable_vacuity_check: bool,
    pub(crate) detect_loops: bool,
    suggest_cegis: bool,
}

struct VerifyContext<'a> {
    module_env: &'a mut verification::ModuleEnv,
    verification_cache: &'a mut std::collections::HashMap<String, resolver::VerificationCacheEntry>,
    verification_config: &'a verification::VerificationConfig,
    output_dir: &'a Path,
    source: &'a str,
    json_output: bool,
    quiet_output: bool,
    emit_structured_feedback: bool,
    emit_lean_artifacts: bool,
    enable_spurious_detection: bool,
    warn_fragment: bool,
    diagnostics: &'a mut Vec<verification::Diagnostic>,
    loss_vectors: &'a mut Vec<serde_json::Value>,
    structured_feedbacks: &'a mut Vec<StructuredFeedback>,
    reconstruction_losses: &'a mut std::collections::HashMap<String, ReconstructionLoss>,
    cert_results: &'a mut std::collections::HashMap<String, (String, String)>,
    verified: &'a mut usize,
    failed: &'a mut usize,
    skipped: &'a mut usize,
    escalated: &'a mut usize,
}

fn verify_single_atom(atom: &parser::Atom, name: &str, ctx: &mut VerifyContext<'_>) {
    let has_fragment_warning =
        verification::outside_decidable_fragment_diagnostic(atom, ctx.module_env).is_some();
    if ctx.warn_fragment {
        let _ = collect_decidable_fragment_diagnostic(atom, ctx.module_env, ctx.json_output)
            .inspect(|d| ctx.diagnostics.push(d.clone()))
            .is_some();
    }
    let promote_outside_fragment = ctx.emit_lean_artifacts && has_fragment_warning;
    if ctx.module_env.is_verified(name) {
        if !ctx.quiet_output {
            println!("  ⚖️  '{}': skipped (imported, contract-trusted)", name);
        }
        if ctx.emit_structured_feedback {
            ctx.structured_feedbacks
                .push(structured_feedback_for_passed_atom(atom));
        }
        if promote_outside_fragment {
            ctx.cert_results.insert(
                name.to_string(),
                ("skipped".to_string(), "escalation_candidate".to_string()),
            );
            *ctx.escalated += 1;
        }
        return;
    }

    let proof_flags = if ctx.verification_config.enable_vacuity_check {
        &["enable_vacuity_check"][..]
    } else {
        &[][..]
    };
    let proof_hash = resolver::compute_proof_hash_with_flags(atom, ctx.module_env, proof_flags);

    if let Some(cached_entry) = ctx.verification_cache.get(name) {
        if cached_entry.proof_hash == proof_hash {
            if !ctx.quiet_output {
                println!("  ⚖️  '{}': skipped (unchanged, cached) ⏩", name);
            }
            ctx.module_env.mark_verified(name);
            ctx.cert_results.insert(
                name.to_string(),
                (
                    "unsat".to_string(),
                    if promote_outside_fragment {
                        "escalation_candidate".to_string()
                    } else {
                        "verified".to_string()
                    },
                ),
            );
            if promote_outside_fragment {
                *ctx.escalated += 1;
            }
            if ctx.emit_structured_feedback {
                ctx.structured_feedbacks
                    .push(structured_feedback_for_passed_atom(atom));
            }
            *ctx.skipped += 1;
            return;
        }
    }

    let deps: Vec<String> = ctx
        .module_env
        .dependency_graph
        .get(name)
        .map(|s| s.iter().cloned().collect())
        .unwrap_or_default();
    let type_deps: Vec<String> = atom
        .params
        .iter()
        .filter_map(|p| p.type_ref.as_ref().map(|tr| tr.name.clone()))
        .filter(|tn| ctx.module_env.get_type(tn).is_some())
        .collect();

    let hir_atom = lower_atom_to_hir_with_env(atom, Some(ctx.module_env));
    let mir_body = mir::lower_hir_to_mir(&hir_atom);
    match mir_body.check_analysis_budget() {
        Ok(()) => {
            let liveness = mir_analysis::compute_liveness(&mir_body);
            let mut mir_body_mut = mir_body;
            mir_analysis::insert_drops(&mut mir_body_mut, &liveness);
        }
        Err(msg) => {
            eprintln!("  ⚠️  {}", msg);
        }
    }

    match verification::verify_with_verification_config(
        &hir_atom,
        ctx.output_dir,
        ctx.module_env,
        ctx.verification_config,
    ) {
        Ok(_) => {
            if !ctx.quiet_output {
                println!("  ⚖️  '{}': verified ✅", name);
            }
            ctx.module_env.mark_verified(name);
            ctx.cert_results.insert(
                name.to_string(),
                (
                    "unsat".to_string(),
                    if promote_outside_fragment {
                        "escalation_candidate".to_string()
                    } else {
                        "verified".to_string()
                    },
                ),
            );
            if promote_outside_fragment {
                *ctx.escalated += 1;
            }
            if ctx.emit_structured_feedback {
                ctx.structured_feedbacks
                    .push(structured_feedback_for_passed_atom(atom));
            }
            ctx.verification_cache.insert(
                name.to_string(),
                resolver::VerificationCacheEntry {
                    proof_hash,
                    result: "verified".to_string(),
                    dependencies: deps,
                    type_deps,
                    timestamp: format!(
                        "{}s",
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs()
                    ),
                },
            );
            *ctx.verified += 1;
        }
        Err(e) => {
            let error_text = format!("{e}");
            let counterexample_model = counterexample_model_from_error(&e);
            let counterexample_value = if let verification::MumeiError::VerificationError {
                counterexample: Some(counterexample),
                ..
            } = &e
            {
                Some(counterexample.clone())
            } else {
                None
            };
            let reconstruction_loss = reconstruction_loss_from_error(&e, &atom.ensures);
            if !ctx.quiet_output {
                let resolved = resolve_source_for_span(ctx.source, &atom.span);
                let e = e.with_source(&resolved, &atom.span);
                eprintln!("{:?}", miette::Report::new(e));
            }
            let z3_result = verification::z3_result_from_error_message(&error_text)
                .unwrap_or("sat")
                .to_string();
            if ctx.enable_spurious_detection && z3_result == "sat" {
                let validation = verification::validate_counterexample(
                    atom,
                    &counterexample_model,
                    ctx.module_env,
                );
                emit_spurious_counterexample_diagnostic(&validation, name, ctx.json_output);
            }
            if z3_result == "sat" {
                if let Some(loss) = reconstruction_loss {
                    if !ctx.quiet_output {
                        println!(
                            "  🧭 Reconstruction loss for '{}': {:?}",
                            name, loss.loss_vector
                        );
                    }
                    ctx.reconstruction_losses.insert(name.to_string(), loss);
                }
            }
            if z3_result == "sat" {
                let loss_vector = verification::build_loss_vector(
                    atom,
                    verification::FAILURE_POSTCONDITION_VIOLATED,
                    counterexample_value.as_ref(),
                    &atom.span,
                );
                if !verification::is_reconstruction_loss_empty(&loss_vector) {
                    ctx.loss_vectors.push(loss_vector);
                }
            }
            let classification = verification::classify_atom_for_lean_escalation(
                atom,
                ctx.module_env,
                &z3_result,
                "failed",
            );
            let status = if ctx.emit_lean_artifacts && classification.should_escalate {
                if !ctx.quiet_output {
                    if z3_result == "unknown" {
                        println!(
                            "  Z3 returned unknown for atom '{}', escalating to Lean 4...",
                            name
                        );
                    } else {
                        println!(
                            "  [lean] '{}': marked for escalation ({})",
                            name,
                            classification
                                .escalation_reason
                                .map(|reason| reason.as_str())
                                .unwrap_or("lean_escalation")
                        );
                    }
                }
                *ctx.escalated += 1;
                "escalation_candidate"
            } else {
                *ctx.failed += 1;
                "failed"
            };
            ctx.cert_results
                .insert(name.to_string(), (z3_result, status.to_string()));
            if ctx.emit_structured_feedback {
                ctx.structured_feedbacks
                    .push(structured_feedback_from_report_file(
                        ctx.output_dir,
                        atom,
                        Some(&error_text),
                    ));
            }
            ctx.verification_cache.remove(name);
        }
    }
}

pub(crate) fn verify_escalation_bundle_path(input: &str, output: Option<&str>) -> PathBuf {
    if let Some(output) = output {
        PathBuf::from(output)
    } else {
        Path::new(input).with_extension("escalation-bundle.json")
    }
}

pub(crate) fn verify_human_review_queue_path(output_dir: &Path, output: Option<&str>) -> PathBuf {
    if let Some(output) = output {
        PathBuf::from(output).with_extension("human-review-queue.json")
    } else {
        output_dir.join("human_review_queue.json")
    }
}

#[derive(Debug, Default)]
struct LeanBridgeApplyStats {
    lean_verified: usize,
    newly_proven: usize,
    lean_verified_atoms: Vec<String>,
}

fn format_count_map(map: &std::collections::HashMap<String, usize>) -> String {
    if map.is_empty() {
        return String::new();
    }
    let mut entries: Vec<_> = map.iter().collect();
    entries.sort_by_key(|(key, _)| *key);
    entries
        .into_iter()
        .map(|(key, count)| format!("{key}: {count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn print_escalation_bundle_summary(bundle: &proof_cert::EscalationBundle) {
    println!(
        "  Lean escalation bundle summary: candidates={}, by_reason={{{}}}",
        bundle.summary.candidate_count,
        format_count_map(&bundle.summary.by_reason)
    );
    println!(
        "  Lean escalation z3_result_class: {{{}}}",
        format_count_map(&bundle.summary.by_z3_result_class)
    );
}

fn resolve_mumei_lean_bridge() -> Result<(PathBuf, PathBuf), String> {
    if let Ok(raw) = std::env::var("MUMEI_LEAN_PATH") {
        let configured = PathBuf::from(raw);
        let script = if configured.is_file() {
            configured.clone()
        } else {
            configured.join("scripts").join("bridge.py")
        };
        if script.exists() {
            let repo_dir = script
                .parent()
                .and_then(Path::parent)
                .map(Path::to_path_buf)
                .unwrap_or(configured);
            return Ok((repo_dir, script));
        }
        return Err(format!(
            "MUMEI_LEAN_PATH is set but scripts/bridge.py was not found at {}",
            script.display()
        ));
    }

    let mut candidates = Vec::new();
    if let Ok(current_dir) = std::env::current_dir() {
        if let Some(parent) = current_dir.parent() {
            candidates.push(parent.join("mumei-lean"));
        }
        candidates.push(current_dir.join("mumei-lean"));
    }
    candidates.push(PathBuf::from("../mumei-lean"));
    candidates.push(PathBuf::from("/home/ubuntu/repos/mumei-lean"));

    for repo_dir in candidates {
        let script = repo_dir.join("scripts").join("bridge.py");
        if script.exists() {
            return Ok((repo_dir, script));
        }
    }

    Err(
        "mumei-lean bridge.py not found. Set MUMEI_LEAN_PATH to the mumei-lean repository."
            .to_string(),
    )
}

fn lean_escalation_temp_path(prefix: &str, extension: &str) -> PathBuf {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    std::env::temp_dir().join(format!(
        "mumei-{prefix}-{}-{timestamp}.{extension}",
        std::process::id()
    ))
}

fn apply_lean_cert_to_proof_certificate(
    cert: &mut proof_cert::ProofCertificate,
    lean_bundle: &proof_cert::EscalationBundle,
) -> LeanBridgeApplyStats {
    let candidates: std::collections::HashMap<_, _> = lean_bundle
        .candidates
        .iter()
        .map(|candidate| (candidate.name.as_str(), candidate))
        .collect();
    let mut stats = LeanBridgeApplyStats::default();
    for atom in &mut cert.atoms {
        let Some(candidate) = candidates.get(atom.name.as_str()) else {
            continue;
        };
        let lean_metadata = candidate
            .lean_result_metadata
            .clone()
            .or_else(|| candidate.lean_metadata.clone());
        atom.lean_metadata = lean_metadata.clone();
        atom.lean_result_metadata = lean_metadata;
        if candidate.z3_check_result == "lean_verified"
            && lean_candidate_metadata_is_current(candidate)
        {
            let was_already_proven = atom.z3_check_result == "unsat" && atom.status == "verified";
            atom.z3_check_result = "lean_verified".to_string();
            atom.status = "verified".to_string();
            atom.translator_version = candidate.translator_version.clone();
            atom.bridge_lemma_hash = candidate.bridge_lemma_hash.clone();
            stats.lean_verified += 1;
            stats.lean_verified_atoms.push(atom.name.clone());
            if !was_already_proven {
                stats.newly_proven += 1;
            }
        }
    }
    proof_cert::refresh_certificate_integrity(cert);
    stats
}

fn lean_candidate_metadata_is_current(candidate: &proof_cert::EscalationCandidate) -> bool {
    // `lean_verified` is accepted only under the current mumei-lean translator contract.
    if candidate.translator_version != verification::LEAN_TRANSLATOR_VERSION
        || candidate.bridge_lemma_hash != verification::LEAN_BRIDGE_LEMMA_HASH
    {
        return false;
    }
    let Some(metadata) = candidate
        .lean_result_metadata
        .as_ref()
        .or(candidate.lean_metadata.as_ref())
    else {
        return false;
    };
    metadata.status == "lean_verified"
        && !metadata.theorem_name.is_empty()
        && metadata.translator_version == verification::LEAN_TRANSLATOR_VERSION
        && metadata.bridge_lemma_hash == verification::LEAN_BRIDGE_LEMMA_HASH
}

fn run_lean_bridge(
    bundle: &proof_cert::EscalationBundle,
    bundle_path: &Path,
    quiet_output: bool,
) -> Result<(PathBuf, proof_cert::EscalationBundle), String> {
    if bundle.summary.candidate_count == 0 {
        if !quiet_output {
            println!("  Lean escalation: no candidates; skipping mumei-lean bridge");
        }
        return Err("no Lean escalation candidates".to_string());
    }

    let (repo_dir, bridge_script) = resolve_mumei_lean_bridge()?;
    let lean_cert_path = lean_escalation_temp_path("lean-cert", "json");
    let generated_dir = lean_escalation_temp_path("lean-generated", "dir");
    let status = ProcessCommand::new("python3")
        .arg(&bridge_script)
        .arg("--escalation-bundle")
        .arg(bundle_path)
        .arg("--lean-cert-out")
        .arg(&lean_cert_path)
        .arg("--out-dir")
        .arg(&generated_dir)
        .current_dir(&repo_dir)
        .output()
        .map_err(|err| {
            format!(
                "Failed to invoke mumei-lean bridge at {}: {}",
                bridge_script.display(),
                err
            )
        })?;

    if !quiet_output {
        let stdout = String::from_utf8_lossy(&status.stdout);
        if !stdout.trim().is_empty() {
            println!("  mumei-lean bridge stdout:\n{}", stdout.trim());
        }
        let stderr = String::from_utf8_lossy(&status.stderr);
        if !stderr.trim().is_empty() {
            eprintln!("  mumei-lean bridge stderr:\n{}", stderr.trim());
        }
    }

    if !status.status.success() {
        return Err(format!(
            "mumei-lean bridge exited with status {}",
            status.status
        ));
    }
    if !lean_cert_path.exists() {
        return Err(format!(
            "mumei-lean bridge did not write {}",
            lean_cert_path.display()
        ));
    }
    let lean_json = std::fs::read_to_string(&lean_cert_path)
        .map_err(|err| format!("Failed to read {}: {}", lean_cert_path.display(), err))?;
    let lean_bundle: proof_cert::EscalationBundle = serde_json::from_str(&lean_json)
        .map_err(|err| format!("Failed to parse {}: {}", lean_cert_path.display(), err))?;
    Ok((lean_cert_path, lean_bundle))
}

pub(crate) fn cmd_verify(options: VerifyOptions<'_>) -> bool {
    let VerifyOptions {
        input,
        task_id,
        solver_timeout,
        cache_scope,
        generate_proof_cert,
        escalate_lean,
        emit_escalation_bundle,
        emit_escalation_metrics,
        emit_decidable_metrics,
        emit_reconstruction_loss,
        emit_loss_vector,
        emit_structured_feedback,
        emit_human_review_queue,
        cert_output,
        report_dir,
        json_output,
        strict_imports,
        allow_lean_verified,
        enable_cross_spec_verification,
        cross_spec_files,
        enable_spurious_detection,
        property_based_test,
        warn_fragment,
        property_based_test_count,
        property_based_test_seed,
        property_based_test_max_shrink_steps,
        harness_contract,
        intent_fidelity,
        artifact_paths,
        budget_policy_fingerprint,
        emit_contract_manifest,
        enable_vacuity_check,
        detect_loops,
        suggest_cegis,
    } = options;
    if emit_loss_vector {
        std::env::set_var(verification::ENABLE_RECONSTRUCTION_LOSS_ENV, "1");
    }
    let structured_feedback_stdout = emit_structured_feedback && cert_output.is_none();
    let loss_vector_stdout = emit_loss_vector && cert_output.is_none();
    let quiet_output = json_output || structured_feedback_stdout || loss_vector_stdout;
    check_z3_available();
    let manifest_config = manifest::find_and_load();
    let (build_cfg, proof_cfg) = if let Some((_, ref m)) = manifest_config {
        (m.build.clone(), m.proof.clone())
    } else {
        (
            manifest::BuildConfig::default(),
            manifest::ProofConfig::default(),
        )
    };
    let enable_cross_spec_verification = enable_cross_spec_verification
        || proof_cfg.cross_spec_verify
        || !cross_spec_files.is_empty();
    let effective_timeout_ms = solver_timeout.unwrap_or(proof_cfg.timeout_ms);
    let property_based_config =
        property_based_test.then(|| verification::PropertyBasedTestConfig {
            test_count: property_based_test_count,
            max_shrink_steps: property_based_test_max_shrink_steps,
            seed: property_based_test_seed
                .unwrap_or(verification::PropertyBasedTestConfig::default().seed),
            ..verification::PropertyBasedTestConfig::default()
        });
    if !quiet_output {
        println!("🗡️  Mumei verify: verifying '{}'...", input);
        if let Some(config) = &property_based_config {
            println!(
                "  🎲 Property-based validation: {} generated input(s), seed {}",
                config.test_count, config.seed
            );
        }
    }
    let (mut items, mut module_env, mut imports, source) =
        match try_load_and_prepare_with_full_options(input, strict_imports, allow_lean_verified) {
            Ok(result) => result,
            Err(e) => {
                eprintln!("  ❌ {e}");
                return true;
            }
        };
    load_cross_spec_files(
        cross_spec_files,
        strict_imports,
        allow_lean_verified,
        &mut items,
        &mut module_env,
        &mut imports,
        !quiet_output,
    );

    let output_dir = match report_dir {
        Some(dir) => Path::new(dir),
        None => Path::new("."),
    };
    // Ensure report directory exists when explicitly specified
    if report_dir.is_some() {
        let _ = std::fs::create_dir_all(output_dir);
    }
    let verification_config = verification::VerificationConfig {
        timeout_ms: effective_timeout_ms,
        global_max_unroll: build_cfg.max_unroll,
        enable_cross_spec_verification,
        collect_decidable_fragment_metrics: emit_decidable_metrics,
        enable_spurious_detection,
        enable_vacuity_check,
        detect_loops,
        suggest_cegis,
        property_based_test: property_based_config,
    };
    let input_path = Path::new(input);
    let base_dir = input_path.parent().unwrap_or(Path::new("."));
    let cache_base_dir = if cache_scope == "global" {
        Path::new(".")
    } else {
        base_dir
    };
    if let Some(task_id) = task_id {
        std::env::set_var("MUMEI_TASK_ID", task_id);
    }
    std::env::set_var(
        "MUMEI_VERIFICATION_TIMEOUT_MS",
        effective_timeout_ms.to_string(),
    );
    std::env::set_var("MUMEI_SOLVER_CACHE_SCOPE", cache_scope);
    let mut verified = 0;
    let mut failed = 0;
    let mut skipped = 0;
    let mut escalated = 0;
    let mut loop_suggestions: Vec<serde_json::Value> = Vec::new();
    let mut reconstruction_losses: std::collections::HashMap<String, ReconstructionLoss> =
        std::collections::HashMap::new();
    let mut loss_vectors: Vec<serde_json::Value> = Vec::new();
    let mut structured_feedbacks: Vec<StructuredFeedback> = Vec::new();
    let mut diagnostics: Vec<verification::Diagnostic> = Vec::new();
    let emit_lean_artifacts = escalate_lean || emit_escalation_bundle || emit_escalation_metrics;

    // Plan 11B: Track per-atom verification results for proof certificates
    let mut cert_results: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();
    let mut lean_escalation_metrics_json: Option<serde_json::Value> = None;

    if emit_contract_manifest {
        let manifest = verification::generate_contract_manifest(&module_env);
        let manifest_path = if let (Some(output), false) = (
            cert_output,
            generate_proof_cert
                || escalate_lean
                || emit_escalation_bundle
                || emit_escalation_metrics,
        ) {
            PathBuf::from(output)
        } else {
            output_dir.join("contract-manifest.json")
        };
        match serde_json::to_string_pretty(&manifest)
            .map_err(|e| e.to_string())
            .and_then(|json| std::fs::write(&manifest_path, json).map_err(|e| e.to_string()))
        {
            Ok(()) => {
                if !quiet_output {
                    println!(
                        "  📜 Contract manifest written to: {}",
                        manifest_path.display()
                    );
                }
            }
            Err(e) => {
                if !quiet_output {
                    eprintln!("  ⚠️  Failed to write contract manifest: {}", e);
                }
                failed += 1;
            }
        }
    }

    // Feature 2: Register dependencies for all atoms before verification
    for item in &items {
        match item {
            Item::Atom(atom) => {
                if verification_config.detect_loops {
                    let loops = verification::detect_loops_needing_invariants(atom);
                    if !loops.is_empty() {
                        if !quiet_output {
                            println!(
                                "  🔁 '{}': {} loop(s) may need invariant strengthening",
                                atom.name,
                                loops.len()
                            );
                        }
                        if verification_config.suggest_cegis {
                            loop_suggestions.push(serde_json::json!({
                                "atom": atom.name,
                                "loops": loops,
                            }));
                        }
                    }
                }
                let callees = resolver::collect_callees_from_body(&atom.body_expr);
                module_env.register_dependencies(&atom.name, callees);
            }
            Item::ImplBlock(impl_block) => {
                for method in &impl_block.methods {
                    let qualified_name = format!("{}::{}", impl_block.struct_name, method.name);
                    if verification_config.detect_loops {
                        let loops = verification::detect_loops_needing_invariants(method);
                        if !loops.is_empty() {
                            if !quiet_output {
                                println!(
                                    "  🔁 '{}': {} loop(s) may need invariant strengthening",
                                    qualified_name,
                                    loops.len()
                                );
                            }
                            if verification_config.suggest_cegis {
                                loop_suggestions.push(serde_json::json!({
                                    "atom": qualified_name,
                                    "loops": loops,
                                }));
                            }
                        }
                    }
                    let callees = resolver::collect_callees_from_body(&method.body_expr);
                    module_env.register_dependencies(&qualified_name, callees);
                }
            }
            _ => {}
        }
    }

    // Feature 2: Migrate old cache and load enhanced verification cache
    resolver::migrate_old_cache(cache_base_dir);
    let mut verification_cache = resolver::load_verification_cache(cache_base_dir);

    for item in &items {
        match item {
            Item::ImplDef(impl_def) => {
                if !quiet_output {
                    println!(
                        "  🔧 Verifying impl {} for {}...",
                        impl_def.trait_name, impl_def.target_type
                    );
                }
                match verification::verify_impl(impl_def, &module_env, output_dir) {
                    Ok(_) => {
                        if !quiet_output {
                            println!("    ✅ Laws verified");
                        }
                        verified += 1;
                    }
                    Err(e) => {
                        if !quiet_output {
                            let resolved = resolve_source_for_span(&source, &impl_def.span);
                            let e = e.with_source(&resolved, &impl_def.span);
                            eprintln!("{:?}", miette::Report::new(e));
                        }
                        failed += 1;
                    }
                }
            }
            Item::Atom(atom) => {
                let mut ctx = VerifyContext {
                    module_env: &mut module_env,
                    verification_cache: &mut verification_cache,
                    verification_config: &verification_config,
                    output_dir,
                    source: &source,
                    json_output,
                    quiet_output,
                    emit_structured_feedback,
                    emit_lean_artifacts,
                    enable_spurious_detection,
                    warn_fragment,
                    diagnostics: &mut diagnostics,
                    loss_vectors: &mut loss_vectors,
                    structured_feedbacks: &mut structured_feedbacks,
                    reconstruction_losses: &mut reconstruction_losses,
                    cert_results: &mut cert_results,
                    verified: &mut verified,
                    failed: &mut failed,
                    skipped: &mut skipped,
                    escalated: &mut escalated,
                };
                verify_single_atom(atom, &atom.name, &mut ctx);
            }
            Item::ImplBlock(impl_block) => {
                for method in &impl_block.methods {
                    let qualified_name = format!("{}::{}", impl_block.struct_name, method.name);
                    let mut qualified_method = method.clone();
                    qualified_method.name = qualified_name.clone();
                    let mut ctx = VerifyContext {
                        module_env: &mut module_env,
                        verification_cache: &mut verification_cache,
                        verification_config: &verification_config,
                        output_dir,
                        source: &source,
                        json_output,
                        quiet_output,
                        emit_structured_feedback,
                        emit_lean_artifacts,
                        enable_spurious_detection,
                        warn_fragment,
                        diagnostics: &mut diagnostics,
                        loss_vectors: &mut loss_vectors,
                        structured_feedbacks: &mut structured_feedbacks,
                        reconstruction_losses: &mut reconstruction_losses,
                        cert_results: &mut cert_results,
                        verified: &mut verified,
                        failed: &mut failed,
                        skipped: &mut skipped,
                        escalated: &mut escalated,
                    };
                    verify_single_atom(&qualified_method, &qualified_name, &mut ctx);
                }
            }
            _ => {}
        }
    }

    // Feature 2: Save enhanced verification cache
    // Note: invalidate_dependents is not needed here because compute_proof_hash
    // already includes callee signatures (requires/ensures) in the hash.
    // If a callee's contract changes, all callers will have different proof hashes
    // and be re-verified automatically.
    resolver::save_verification_cache(cache_base_dir, &verification_cache);

    if emit_decidable_metrics {
        let metrics_path = cert_output
            .filter(|_| !generate_proof_cert)
            .map(PathBuf::from)
            .unwrap_or_else(|| output_dir.join("decidable_metrics.json"));
        match save_decidable_fragment_metrics(&module_env, &metrics_path, !quiet_output) {
            Ok(()) => {}
            Err(err) => {
                eprintln!("❌ {err}");
                failed += 1;
            }
        }
    }

    if emit_reconstruction_loss {
        let loss_path = cert_output
            .filter(|_| !generate_proof_cert && !emit_escalation_bundle && !emit_escalation_metrics)
            .map(PathBuf::from)
            .unwrap_or_else(|| output_dir.join("reconstruction_loss.json"));
        match save_reconstruction_losses(&reconstruction_losses, &loss_path, !quiet_output) {
            Ok(()) => {}
            Err(err) => {
                eprintln!("❌ {err}");
                failed += 1;
            }
        }
    }

    if enable_cross_spec_verification {
        match save_cross_spec_report(&module_env, output_dir, !quiet_output) {
            Ok(cross_spec_result) => {
                if cross_spec_result.summary.inconsistent_calls > 0 && !quiet_output {
                    eprintln!(
                        "Warning: {} inconsistent contract calls detected",
                        cross_spec_result.summary.inconsistent_calls
                    );
                }
                if cross_spec_result.summary.circular_dependency_count > 0 && !quiet_output {
                    eprintln!(
                        "Warning: {} circular dependencies detected",
                        cross_spec_result.summary.circular_dependency_count
                    );
                }
                if cross_spec_result.summary.global_invariant_conflict_count > 0 && !quiet_output {
                    eprintln!(
                        "Warning: {} global invariant conflicts detected",
                        cross_spec_result.summary.global_invariant_conflict_count
                    );
                }
            }
            Err(e) => {
                if !quiet_output {
                    eprintln!("  ⚠️  Failed to write cross-spec report: {}", e);
                }
                failed += 1;
            }
        }
    }

    // Plan 11B: Generate proof certificate if requested
    if generate_proof_cert
        || escalate_lean
        || emit_escalation_bundle
        || emit_escalation_metrics
        || emit_human_review_queue
    {
        let mut atom_refs: Vec<&parser::Atom> = items
            .iter()
            .filter_map(|item| {
                if let Item::Atom(a) = item {
                    Some(a)
                } else {
                    None
                }
            })
            .collect();
        // Also include ImplBlock methods in the certificate (with qualified names)
        let mut qualified_methods: Vec<parser::Atom> = Vec::new();
        for item in &items {
            if let Item::ImplBlock(ib) = item {
                for method in &ib.methods {
                    let mut qualified = method.clone();
                    qualified.name = format!("{}::{}", ib.struct_name, method.name);
                    qualified_methods.push(qualified);
                }
            }
        }
        for qm in &qualified_methods {
            atom_refs.push(qm);
        }
        let mut cert = proof_cert::generate_certificate_with_reconstruction_losses(
            input,
            &atom_refs,
            &cert_results,
            &module_env,
            None,
            None,
            Some(proof_cert::HarnessCertificateMetadata {
                harness_contract: harness_contract.clone(),
                intent_fidelity: intent_fidelity.clone(),
                artifact_paths: artifact_paths.clone(),
                budget_policy_fingerprint: budget_policy_fingerprint.clone(),
            }),
            Some(&reconstruction_losses),
        );
        let cert_path = if let Some(output) = cert_output {
            std::path::PathBuf::from(output)
        } else {
            let stem = Path::new(input).file_stem().unwrap_or_default();
            Path::new(".").join(format!("{}.proof.json", stem.to_string_lossy()))
        };

        if escalate_lean || emit_escalation_bundle || emit_escalation_metrics {
            let bundle = proof_cert::generate_escalation_bundle(&cert);
            if escalate_lean {
                let mut metrics = resolver::LeanEscalationMetrics::default();
                for candidate in &bundle.candidates {
                    metrics.record_candidate(candidate);
                }
                lean_escalation_metrics_json = Some(metrics.to_summary_json());
            }
            if emit_escalation_bundle {
                let bundle_path = verify_escalation_bundle_path(input, cert_output);
                match proof_cert::save_escalation_bundle(&bundle, &bundle_path) {
                    Ok(()) => {
                        if !quiet_output {
                            println!(
                                "  Lean escalation bundle written to: {}",
                                bundle_path.display()
                            );
                        }
                    }
                    Err(e) => {
                        if !quiet_output {
                            eprintln!("  ⚠️  Failed to write escalation bundle: {}", e);
                        }
                    }
                }
            }
            if escalate_lean {
                if bundle.summary.candidate_count == 0 {
                    if !quiet_output {
                        println!("  Lean escalation: no candidates; skipping mumei-lean bridge");
                    }
                } else {
                    let temp_bundle_path = lean_escalation_temp_path("escalation-bundle", "json");
                    match proof_cert::save_escalation_bundle(&bundle, &temp_bundle_path)
                        .and_then(|_| run_lean_bridge(&bundle, &temp_bundle_path, quiet_output))
                    {
                        Ok((lean_cert_path, lean_bundle)) => {
                            let stats =
                                apply_lean_cert_to_proof_certificate(&mut cert, &lean_bundle);
                            verified += stats.newly_proven;
                            escalated = escalated.saturating_sub(stats.lean_verified);
                            if !quiet_output {
                                for atom_name in &stats.lean_verified_atoms {
                                    println!("  lean_verified: {atom_name}");
                                }
                                println!(
                                    "  Lean bridge certificate applied: {} lean_verified atom(s) from {}",
                                    stats.lean_verified,
                                    lean_cert_path.display()
                                );
                            }
                        }
                        Err(e) => {
                            if !quiet_output {
                                eprintln!("  ⚠️  Lean escalation bridge failed: {}", e);
                            }
                            failed += 1;
                        }
                    }
                }
            }
            if emit_escalation_metrics {
                let mut metrics = resolver::LeanEscalationMetrics::default();
                for candidate in &bundle.candidates {
                    metrics.record_candidate(candidate);
                }
                let metrics_path = cert_path.with_extension("escalation-metrics.json");
                let metrics_json = metrics.to_summary_json();
                match serde_json::to_string_pretty(&metrics_json)
                    .map_err(|e| e.to_string())
                    .and_then(|json| std::fs::write(&metrics_path, json).map_err(|e| e.to_string()))
                {
                    Ok(()) => {
                        if !quiet_output {
                            println!(
                                "  📊 Escalation metrics written to: {}",
                                metrics_path.display()
                            );
                        }
                    }
                    Err(e) => {
                        if !quiet_output {
                            eprintln!("  ⚠️  Failed to write escalation metrics: {}", e);
                        }
                    }
                }
            }
            if !quiet_output {
                print_escalation_bundle_summary(&bundle);
            }
        }

        if generate_proof_cert {
            match proof_cert::save_certificate(&cert, &cert_path) {
                Ok(()) => {
                    if !quiet_output {
                        println!("  📜 Proof certificate written to: {}", cert_path.display());
                    }
                }
                Err(e) => {
                    if !quiet_output {
                        eprintln!("  ⚠️  Failed to write proof certificate: {}", e);
                    }
                }
            }
        }

        if emit_human_review_queue {
            let queue = proof_cert::generate_human_review_queue(&cert);
            let queue_path = verify_human_review_queue_path(output_dir, cert_output);
            match proof_cert::save_human_review_queue(&queue, &queue_path) {
                Ok(()) => {
                    if !quiet_output {
                        println!(
                            "  👥 Human review queue written to: {}",
                            queue_path.display()
                        );
                    }
                }
                Err(e) => {
                    if !quiet_output {
                        eprintln!("  ⚠️  Failed to write human review queue: {}", e);
                    }
                }
            }
        }

        // Task 1-B: When `--proof-cert` is active, surface atoms whose Z3
        // check returned `unknown` so the user knows which lemmas might
        // benefit from being discharged externally (e.g. via mumei-lean).
        if !quiet_output {
            let unknown_count = cert
                .atoms
                .iter()
                .filter(|a| a.z3_check_result == "unknown")
                .count();
            if unknown_count > 0 {
                println!(
                    "ℹ️  {} atom(s) returned 'unknown' from Z3. Consider running mumei-lean to discharge them.",
                    unknown_count
                );
            }
        }
    }

    if emit_structured_feedback {
        let payload = structured_feedback_payload(
            &structured_feedbacks,
            &diagnostics,
            verified,
            failed,
            skipped,
            escalated,
        );
        match serde_json::to_string_pretty(&payload) {
            Ok(serialized) => {
                if let Some(output) = cert_output {
                    if let Err(err) = std::fs::write(output, &serialized) {
                        eprintln!("❌ Failed to write structured feedback: {err}");
                        failed += 1;
                    } else if !quiet_output {
                        println!("  🧾 Structured feedback written to: {output}");
                    }
                } else {
                    println!("{serialized}");
                }
            }
            Err(err) => {
                eprintln!("❌ Failed to serialize structured feedback: {err}");
                failed += 1;
            }
        }
    }

    if emit_loss_vector {
        let payload = loss_vector_payload(&loss_vectors, failed);
        match serde_json::to_string_pretty(&payload) {
            Ok(serialized) => {
                if let Some(output) = cert_output {
                    if let Err(err) = std::fs::write(output, &serialized) {
                        eprintln!("❌ Failed to write loss vector: {err}");
                        failed += 1;
                    } else if !quiet_output {
                        println!("  🧭 Loss vector written to: {output}");
                    }
                } else {
                    println!("{serialized}");
                }
            }
            Err(err) => {
                eprintln!("❌ Failed to serialize loss vector: {err}");
                failed += 1;
            }
        }
    }

    // Proposal B: --json outputs report.json content to stdout
    if json_output {
        let report_path = output_dir.join("report.json");
        if report_path.exists() {
            match std::fs::read_to_string(&report_path) {
                Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
                    Ok(payload) => {
                        let mut payload =
                            enrich_verify_json_payload(payload, &diagnostics, &loop_suggestions);
                        if let Some(ref metrics) = lean_escalation_metrics_json {
                            payload["lean_escalation_metrics"] = metrics.clone();
                        }
                        match serde_json::to_string_pretty(&payload) {
                            Ok(json) => println!("{json}"),
                            Err(_) => println!("{}", content),
                        }
                    }
                    Err(_) => println!("{}", content),
                },
                Err(e) => {
                    eprintln!("Failed to read report.json: {}", e);
                    return true;
                }
            }
        } else {
            // No report.json produced — emit minimal JSON status
            let status = if failed > 0 { "failed" } else { "passed" };
            let mut payload = serde_json::json!({
                "status": status,
                "verified": verified,
                "failed": failed,
                "skipped": skipped,
                "escalation_candidates": escalated,
                "diagnostics": &diagnostics,
                "warnings": &diagnostics,
            });
            if let Some(ref metrics) = lean_escalation_metrics_json {
                payload["lean_escalation_metrics"] = metrics.clone();
            }
            if !loop_suggestions.is_empty() {
                payload["cegis_suggestions"] = serde_json::Value::Array(loop_suggestions.clone());
            }
            match serde_json::to_string_pretty(&payload) {
                Ok(json) => println!("{json}"),
                Err(_) => println!(
                    "{{\"status\":\"{}\",\"verified\":{},\"failed\":{},\"skipped\":{},\"escalation_candidates\":{}}}",
                    status, verified, failed, skipped, escalated
                ),
            }
        }
        return failed > 0;
    } else if structured_feedback_stdout || loss_vector_stdout {
        return failed > 0;
    } else {
        println!();
        if failed > 0 {
            eprintln!(
                "❌ Verification: {} passed, {} failed, {} skipped (cached), {} Lean escalation candidate(s)",
                verified, failed, skipped, escalated
            );
            return true;
        }
        if skipped > 0 {
            println!(
                "✅ Verification passed: {} verified, {} skipped (unchanged), {} Lean escalation candidate(s) ⚡",
                verified, skipped, escalated
            );
        } else {
            println!(
                "✅ Verification passed: {} item(s) verified, {} Lean escalation candidate(s)",
                verified, escalated
            );
        }
    }
    false
}

pub(crate) fn save_decidable_fragment_metrics(
    module_env: &verification::ModuleEnv,
    metrics_path: &Path,
    print_path: bool,
) -> Result<(), String> {
    let config = verification::VerificationConfig {
        collect_decidable_fragment_metrics: true,
        ..verification::VerificationConfig::default()
    };
    let module_report = verification::verify_module(module_env, &config)
        .map_err(|err| format!("decidable-fragment metrics failed: {err}"))?;
    let metrics = module_report
        .decidable_fragment
        .ok_or_else(|| "decidable-fragment metrics were not collected".to_string())?;
    if let Some(parent) = metrics_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create metrics directory: {err}"))?;
    }
    let payload = serde_json::to_string_pretty(&metrics)
        .map_err(|err| format!("failed to serialize decidable-fragment metrics: {err}"))?;
    std::fs::write(metrics_path, payload)
        .map_err(|err| format!("failed to write {}: {err}", metrics_path.display()))?;
    if print_path {
        println!(
            "  📊 Decidable-fragment metrics written to: {}",
            metrics_path.display()
        );
    }
    Ok(())
}

fn loss_vector_payload(loss_vectors: &[serde_json::Value], failed: usize) -> serde_json::Value {
    if let Some(first) = loss_vectors.first() {
        first.clone()
    } else if failed > 0 {
        serde_json::json!({
            "status": "verification_failed",
            "error_type": null,
            "location": null,
            "reconstruction_loss": null,
            "feedback_instruction": "Verification failed before a counterexample loss vector could be produced."
        })
    } else {
        serde_json::json!({
            "status": "verification_passed",
            "error_type": null,
            "location": null,
            "reconstruction_loss": null,
            "feedback_instruction": "Verification passed; no fix is required."
        })
    }
}

pub(crate) fn save_reconstruction_losses(
    losses: &std::collections::HashMap<String, ReconstructionLoss>,
    output_path: &Path,
    print_path: bool,
) -> Result<(), String> {
    let mut atoms: Vec<&String> = losses.keys().collect();
    atoms.sort();
    let entries: Vec<serde_json::Value> = atoms
        .into_iter()
        .filter_map(|atom| {
            losses.get(atom).map(|loss| {
                serde_json::json!({
                    "atom": atom,
                    "violated_property": loss.violated_property,
                    "counter_example": loss.counter_example,
                    "loss_set_size": loss.loss_set_size,
                    "is_zero_loss": loss.is_zero_loss,
                    "loss_vector": loss.loss_vector,
                })
            })
        })
        .collect();
    let payload = serde_json::json!({
        "version": "1.0",
        "reconstruction_loss_count": entries.len(),
        "reconstruction_losses": entries,
    });
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create reconstruction-loss directory: {err}"))?;
    }
    let json = serde_json::to_string_pretty(&payload)
        .map_err(|err| format!("failed to serialize reconstruction loss: {err}"))?;
    std::fs::write(output_path, json)
        .map_err(|err| format!("failed to write {}: {err}", output_path.display()))?;
    if print_path {
        println!(
            "  🧭 Reconstruction loss written to: {} ({} atom(s))",
            output_path.display(),
            entries.len()
        );
    }
    Ok(())
}

pub(crate) fn save_cross_spec_report(
    module_env: &verification::ModuleEnv,
    output_dir: &Path,
    print_path: bool,
) -> Result<cross_spec::CrossSpecResult, String> {
    let config = verification::VerificationConfig {
        enable_cross_spec_verification: true,
        ..verification::VerificationConfig::default()
    };
    let module_report = verification::verify_module(module_env, &config)
        .map_err(|err| format!("cross-spec verification failed: {err}"))?;
    let cross_spec_result = module_report
        .cross_spec
        .ok_or_else(|| "cross-spec verification was not enabled".to_string())?;
    std::fs::create_dir_all(output_dir)
        .map_err(|err| format!("failed to create report directory: {err}"))?;
    let cross_spec_path = output_dir.join("cross_spec.json");
    let payload = serde_json::to_string_pretty(&cross_spec_result)
        .map_err(|err| format!("failed to serialize cross-spec report: {err}"))?;
    std::fs::write(&cross_spec_path, payload)
        .map_err(|err| format!("failed to write {}: {err}", cross_spec_path.display()))?;
    if print_path {
        println!(
            "  🔗 Cross-spec report written to: {}",
            cross_spec_path.display()
        );
    }
    Ok(cross_spec_result)
}

// =============================================================================
// mumei init — generate project template
// =============================================================================
