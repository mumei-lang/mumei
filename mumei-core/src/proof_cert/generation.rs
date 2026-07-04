use super::models::{
    AtomCertificate, CounterexampleValidationMetadata, HarnessCertificateMetadata, IntentFidelity,
    ProofCertificate, SelfCorrectionMetadata, SelfCorrectionSummary, SolverProcessMetadata,
    UnusedHypothesisMetadata,
};
use super::status;
use super::validation::{
    compute_certificate_hash, manual_lemma_reason_for_atom, parse_unsat_core_labels,
};
use crate::reconstruction_loss::ReconstructionLoss;
use crate::resolver;
use crate::verification::{self, ModuleEnv};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

pub fn compute_atom_content_hash(name: &str, requires: &str, ensures: &str, body: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(name.as_bytes());
    hasher.update(b"\n---requires---\n");
    hasher.update(requires.as_bytes());
    hasher.update(b"\n---ensures---\n");
    hasher.update(ensures.as_bytes());
    hasher.update(b"\n---body---\n");
    hasher.update(body.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Get Z3 version string by running `z3 --version`.
pub fn get_z3_version() -> String {
    std::process::Command::new("z3")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn self_correction_metadata_from_env(atom_name: &str) -> Option<SelfCorrectionMetadata> {
    let raw = std::env::var("MUMEI_SELF_CORRECTION_METADATA").ok()?;
    if raw.trim().is_empty() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    if value.get("repair_attempts").is_some() {
        return serde_json::from_value(value).ok();
    }
    value
        .get(atom_name)
        .cloned()
        .and_then(|item| serde_json::from_value(item).ok())
}

/// Generate a proof certificate from verification results.
///
/// `verification_results` maps atom_name → (z3_check_result, status).
/// z3_check_result is "unsat"/"sat"/"unknown"/"skipped"/"lean_verified"/
/// "timeout"/"resource_limit"/"spurious_candidate".
/// status is "verified"/"failed"/"skipped"/"trusted".
///
/// P5-A: Extended with `module_env` for proof hash and dependency info,
/// and optional `package_name`/`package_version` from mumei.toml.
pub fn generate_certificate(
    file: &str,
    atoms: &[&crate::parser::Atom],
    verification_results: &HashMap<String, (String, String)>,
    module_env: &ModuleEnv,
    package_name: Option<&str>,
    package_version: Option<&str>,
    harness_metadata: Option<HarnessCertificateMetadata>,
) -> ProofCertificate {
    generate_certificate_with_reconstruction_losses(
        file,
        atoms,
        verification_results,
        module_env,
        package_name,
        package_version,
        harness_metadata,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn generate_certificate_with_reconstruction_losses(
    file: &str,
    atoms: &[&crate::parser::Atom],
    verification_results: &HashMap<String, (String, String)>,
    module_env: &ModuleEnv,
    package_name: Option<&str>,
    package_version: Option<&str>,
    harness_metadata: Option<HarnessCertificateMetadata>,
    reconstruction_losses: Option<&HashMap<String, ReconstructionLoss>>,
) -> ProofCertificate {
    let now = chrono_like_now();
    let harness_metadata = harness_metadata.unwrap_or_default();
    let atom_certs: Vec<AtomCertificate> = atoms
        .iter()
        .map(|atom| {
            let content_hash = compute_atom_content_hash(
                &atom.name,
                &atom.requires,
                &atom.ensures,
                &atom.body_expr,
            );
            let (z3_result, status) = verification_results
                .get(&atom.name)
                .cloned()
                .unwrap_or_else(|| (status::Z3_SKIPPED.to_string(), status::SKIPPED.to_string()));

            // P5-A: Compute proof hash with transitive dependencies
            let proof_hash = resolver::compute_proof_hash(atom, module_env);

            // P5-A: Direct callee atom names from dependency graph
            let dependencies: Vec<String> = module_env
                .dependency_graph
                .get(&atom.name)
                .map(|s| {
                    let mut deps: Vec<String> = s.iter().cloned().collect();
                    deps.sort();
                    deps
                })
                .unwrap_or_default();

            // P5-A: Declared effect names
            let effects: Vec<String> = atom.effects.iter().map(|e| e.name.clone()).collect();
            let classification = verification::classify_atom_for_lean_escalation(
                atom, module_env, &z3_result, &status,
            );
            let mut translator_ir = verification::build_translator_ir_metadata(atom, module_env);
            let manual_lemma_reason =
                manual_lemma_reason_for_atom(atom, &classification.logic_fragment_tags);
            translator_ir.manual_lemma_reason = manual_lemma_reason.clone();
            let binder_mapping = translator_ir
                .binders
                .iter()
                .map(|binder| (binder.mumei_name.clone(), binder.lean_name.clone()))
                .collect();
            let counterexample_validation = if z3_result == status::Z3_SAT {
                let validation =
                    verification::validate_counterexample(atom, &HashMap::new(), module_env);
                Some(CounterexampleValidationMetadata {
                    validation_status: validation.validation_status,
                    validated_at: now.clone(),
                    failed_constraints: validation.failed_constraints,
                })
            } else {
                None
            };
            let symbol_provenance =
                verification::detect_uninterpreted_symbols(atom, &HashMap::new(), module_env);
            let unused_hypotheses = parse_unsat_core_labels(&z3_result).map(|unsat_core| {
                let report = verification::detect_unused_hypotheses(atom, &unsat_core, module_env);
                UnusedHypothesisMetadata {
                    unused_requires: report.unused_requires,
                    unused_invariants: report.unused_invariants,
                    unused_effect_constraints: report.unused_effect_constraints,
                    minimal_constraint_set: report.minimal_constraint_set,
                }
            });
            let spec_validation_result = Some(
                verification::check_spec_satisfiability(atom, module_env).unwrap_or_else(|err| {
                    verification::SpecValidationResult::from_contradiction(atom, &err)
                }),
            );
            let solver_process_metadata = solver_process_metadata_from_env(&content_hash);
            let reconstruction_loss = reconstruction_losses
                .and_then(|losses| losses.get(&atom.name))
                .cloned();
            let self_correction_metadata = self_correction_metadata_from_env(&atom.name);

            AtomCertificate {
                name: atom.name.clone(),
                z3_check_result: z3_result,
                content_hash,
                status,
                spec_validation_result,
                proof_hash,
                dependencies,
                effects,
                requires: atom.requires.clone(),
                ensures: atom.ensures.clone(),
                body_expr: atom.body_expr.clone(),
                body_summary: atom.body_expr.clone(),
                z3_result_class: classification.z3_result_class,
                escalation_reason: classification.escalation_reason,
                logic_fragment_tag: classification.logic_fragment_tag,
                logic_fragment_tags: classification.logic_fragment_tags,
                translator_version: verification::LEAN_TRANSLATOR_VERSION.to_string(),
                binder_mapping,
                bridge_lemma_hash: verification::LEAN_BRIDGE_LEMMA_HASH.to_string(),
                manual_lemma_reason,
                translator_ir,
                lean_metadata: None,
                lean_result_metadata: None,
                counterexample_validation,
                reconstruction_loss,
                self_correction_metadata,
                symbol_provenance,
                unused_hypotheses,
                solver_process_metadata,
                retry_policy_fingerprint: None,
                attempt_summary: None,
                cost_success_metrics: None,
            }
        })
        .collect();

    let all_verified = atom_certs.iter().all(|ac| {
        (ac.status == status::VERIFIED || ac.status == status::TRUSTED)
            && ac
                .spec_validation_result
                .as_ref()
                .map(|validation| validation.is_satisfiable)
                .unwrap_or(true)
    });
    let self_correction_summary = SelfCorrectionSummary::from_atom_metadata(&atom_certs);

    // Build certificate without hash first, then compute hash
    let reconstruction_loss = reconstruction_losses.and_then(|losses| {
        let mut entries: Vec<ReconstructionLoss> = atoms
            .iter()
            .filter_map(|atom| losses.get(&atom.name).cloned())
            .collect();
        if entries.is_empty() {
            None
        } else {
            entries.sort_by(|left, right| left.violated_property.cmp(&right.violated_property));
            Some(entries)
        }
    });

    let mut cert = ProofCertificate {
        version: "1.0".to_string(),
        timestamp: now,
        mumei_version: env!("CARGO_PKG_VERSION").to_string(),
        z3_version: get_z3_version(),
        file: file.to_string(),
        atoms: atom_certs,
        reconstruction_loss,
        self_correction_summary,
        package_name: package_name.map(|s| s.to_string()),
        package_version: package_version.map(|s| s.to_string()),
        certificate_hash: String::new(),
        all_verified,
        harness_contract: harness_metadata.harness_contract,
        intent_fidelity: harness_metadata.intent_fidelity,
        artifact_paths: harness_metadata.artifact_paths,
        budget_policy_fingerprint: harness_metadata.budget_policy_fingerprint,
    };

    // P5-A: Compute certificate_hash as SHA-256 of serialized cert (with empty hash field)
    cert.certificate_hash = compute_certificate_hash(&cert);

    cert
}

/// Compute SHA-256 hash of arbitrary data (utility for other crates).
pub fn compute_sha256(data: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

pub fn harness_contract_from_env() -> Option<String> {
    env_nonempty("MUMEI_HARNESS_CONTRACT")
}

pub fn artifact_paths_from_env() -> Option<Vec<String>> {
    env_nonempty("MUMEI_ARTIFACT_PATHS").and_then(|value| {
        let paths: Vec<String> = value
            .split(',')
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .collect();
        if paths.is_empty() {
            None
        } else {
            Some(paths)
        }
    })
}

pub fn intent_fidelity_from_env() -> Option<IntentFidelity> {
    let natural_language_prompt_hash = env_nonempty("MUMEI_INTENT_PROMPT_HASH");
    let raw_spec_traceability_score = env_nonempty("MUMEI_SPEC_TRACEABILITY_SCORE");
    let spec_traceability_score = raw_spec_traceability_score
        .as_deref()
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or_default();
    let raw_semantic_drift_detected = env_nonempty("MUMEI_SEMANTIC_DRIFT_DETECTED");
    let semantic_drift_detected = raw_semantic_drift_detected
        .as_deref()
        .and_then(|value| value.parse::<bool>().ok())
        .unwrap_or_default();
    let raw_manual_review_required = env_nonempty("MUMEI_MANUAL_REVIEW_REQUIRED");
    let manual_review_required = raw_manual_review_required
        .as_deref()
        .and_then(|value| value.parse::<bool>().ok())
        .unwrap_or_default();
    let has_metadata = natural_language_prompt_hash.is_some()
        || raw_spec_traceability_score.is_some()
        || raw_semantic_drift_detected.is_some()
        || raw_manual_review_required.is_some();
    if has_metadata {
        Some(IntentFidelity {
            natural_language_prompt_hash,
            spec_traceability_score,
            semantic_drift_detected,
            manual_review_required,
        })
    } else {
        None
    }
}

pub fn budget_policy_fingerprint_from_env() -> Option<String> {
    env_nonempty("MUMEI_BUDGET_POLICY_FINGERPRINT")
}

fn solver_process_metadata_from_env(content_hash: &str) -> Option<SolverProcessMetadata> {
    let task_id = env_nonempty("MUMEI_TASK_ID");
    let generation_id = env_nonempty("MUMEI_GENERATION_ID").unwrap_or_default();
    let timeout_ms = env_nonempty("MUMEI_VERIFICATION_TIMEOUT_MS")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default();
    let explicit_fingerprint = env_nonempty("MUMEI_SOLVER_CONFIG_FINGERPRINT");
    let solver_config_fingerprint = explicit_fingerprint.clone().unwrap_or_else(|| {
        verification::compute_solver_config_fingerprint(timeout_ms, true, false, false, true)
    });
    let explicit_cache_key = env_nonempty("MUMEI_SOLVER_CACHE_KEY");
    let cache_key = explicit_cache_key.clone().unwrap_or_else(|| {
        compute_sha256(&format!("{}:{}", content_hash, solver_config_fingerprint))
    });
    let cancel_reason = env_nonempty("MUMEI_CANCEL_REASON");
    let process_start_time = env_nonempty("MUMEI_SOLVER_PROCESS_START_TIME").unwrap_or_default();
    let has_metadata = task_id.is_some()
        || !generation_id.is_empty()
        || timeout_ms > 0
        || explicit_fingerprint.is_some()
        || explicit_cache_key.is_some()
        || cancel_reason.is_some()
        || !process_start_time.is_empty();
    if !has_metadata {
        return None;
    }
    Some(SolverProcessMetadata {
        task_id,
        cache_key,
        generation_id,
        solver_config_fingerprint,
        timeout_ms,
        cancel_reason,
        process_start_time,
        process_end_time: chrono_like_now(),
    })
}

fn chrono_like_now() -> String {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    // Simple UTC timestamp formatting
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Approximate date calculation (good enough for timestamps)
    let mut year = 1970i64;
    let mut remaining_days = days as i64;
    loop {
        let days_in_year = if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
            366
        } else {
            365
        };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }
    let is_leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days = [
        31,
        if is_leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1;
    for &md in &month_days {
        if remaining_days < md {
            break;
        }
        remaining_days -= md;
        month += 1;
    }
    let day = remaining_days + 1;

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hours, minutes, seconds
    )
}
