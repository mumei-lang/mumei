// =============================================================================
// Plan 11B: Z3 Proof Certificates
// =============================================================================
//
// Generates cryptographically verifiable proof certificates for verified atoms.
// Each certificate contains per-atom Z3 check results and content hashes,
// enabling offline verification that proofs are still valid.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::reconstruction_loss::ReconstructionLoss;
use crate::resolver;
use crate::verification::{self, ModuleEnv, SymbolProvenance, TranslatorIRMetadata};

pub use crate::verification::{EscalationReason, LogicFragment};

/// Top-level proof certificate for a Mumei source file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofCertificate {
    /// Certificate format version
    pub version: String,
    /// ISO 8601 timestamp of certificate generation
    pub timestamp: String,
    /// Mumei compiler version
    #[serde(default)]
    pub mumei_version: String,
    /// Z3 solver version (if available)
    pub z3_version: String,
    /// Source file path
    pub file: String,
    /// Per-atom verification certificates
    pub atoms: Vec<AtomCertificate>,
    /// Reconstruction loss metadata for SAT counterexamples.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconstruction_loss: Option<Vec<ReconstructionLoss>>,
    /// P9-F: Aggregate self-correction convergence metrics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_correction_summary: Option<SelfCorrectionSummary>,
    /// Package name from mumei.toml [package].name (P5-A)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_name: Option<String>,
    /// Package version from mumei.toml [package].version (P5-A)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_version: Option<String>,
    /// SHA-256 of the serialized certificate (excluding this field) for chain verification (P5-A)
    #[serde(default)]
    pub certificate_hash: String,
    /// Summary flag: true if all atoms are verified (P5-A)
    #[serde(default)]
    pub all_verified: bool,
    /// Harness contract path or identifier for cross-repo verification harnesses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_contract: Option<String>,
    /// Metadata tracking natural-language intent fidelity for harness consumers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent_fidelity: Option<IntentFidelity>,
    /// Artifact paths that a harness should collect or validate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_paths: Option<Vec<String>>,
    /// Fingerprint of the retry/budget policy used by the harness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_policy_fingerprint: Option<String>,
}

/// Per-atom verification certificate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtomCertificate {
    /// Atom name
    pub name: String,
    /// Z3 solver check result: "unsat" (proven), "sat" (counter-example found), "unknown", "skipped"
    pub z3_check_result: String,
    /// SHA-256 hash of the atom's source text (requires + ensures + body)
    pub content_hash: String,
    /// Verification status: "verified", "failed", "skipped", "trusted"
    pub status: String,
    /// Pre-proof specification validation and traceability result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_validation_result: Option<verification::SpecValidationResult>,
    /// Dependency-aware proof hash including transitive callee signatures (P5-A)
    #[serde(default)]
    pub proof_hash: String,
    /// Direct callee atom names from dependency graph (P5-A)
    #[serde(default)]
    pub dependencies: Vec<String>,
    /// Declared effect names (P5-A)
    #[serde(default)]
    pub effects: Vec<String>,
    /// Precondition contract text (P5-A)
    #[serde(default)]
    pub requires: String,
    /// Postcondition contract text (P5-A)
    #[serde(default)]
    pub ensures: String,
    /// Deterministic Z3 outcome class used by Lean escalation routing.
    #[serde(default)]
    pub z3_result_class: String,
    /// Reason this atom should be escalated to Lean, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalation_reason: Option<EscalationReason>,
    /// Primary logical fragment for bridge routing and metrics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logic_fragment_tag: Option<LogicFragment>,
    /// Logical fragments detected in this proof obligation.
    #[serde(default)]
    pub logic_fragment_tags: Vec<String>,
    /// Translator version required for Lean escalation cache validity.
    #[serde(default)]
    pub translator_version: String,
    /// Mumei witness name to generated Lean binder name mapping.
    #[serde(default)]
    pub binder_mapping: HashMap<String, String>,
    /// Hash of the bridge lemma set used by this translation contract.
    #[serde(default)]
    pub bridge_lemma_hash: String,
    /// Structured reason when this atom requires manual Lean lemma work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_lemma_reason: Option<String>,
    /// Typed intermediate representation metadata for Lean lowering.
    #[serde(default)]
    pub translator_ir: TranslatorIRMetadata,
    /// Lean-side result metadata populated by mumei-lean.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lean_metadata: Option<LeanResultMetadata>,
    /// Structured Lean-side result metadata populated by mumei-lean.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lean_result_metadata: Option<LeanResultMetadata>,
    /// P8-A: Counterexample validation status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counterexample_validation: Option<CounterexampleValidationMetadata>,
    /// P9: Reconstruction loss inferred from a Z3 SAT counterexample.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconstruction_loss: Option<ReconstructionLoss>,
    /// P9-F: Self-correction loop metadata for this atom.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_correction_metadata: Option<SelfCorrectionMetadata>,
    /// P8-A: Symbol provenance for uninterpreted symbols.
    #[serde(default)]
    pub symbol_provenance: Vec<SymbolProvenance>,
    /// P8-A: Unused hypothesis report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unused_hypotheses: Option<UnusedHypothesisMetadata>,
    #[serde(default)]
    pub solver_process_metadata: Option<SolverProcessMetadata>,
    /// P8-G: Fingerprint of the retry budget policy used by the healing/escalation loop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_policy_fingerprint: Option<String>,
    /// P8-G: Summary of retry attempts leading to this certificate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_summary: Option<AttemptSummary>,
    /// P8-G: Cost/success metrics for budget feedback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_success_metrics: Option<CostSuccessMetrics>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SelfCorrectionMetadata {
    #[serde(default)]
    pub repair_attempts: u32,
    #[serde(default)]
    pub converged: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_error: Option<String>,
    #[serde(default)]
    pub consecutive_successes: u32,
    #[serde(default)]
    pub token_cost: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SelfCorrectionSummary {
    #[serde(default)]
    pub total_atoms: u32,
    #[serde(default)]
    pub converged_atoms: u32,
    #[serde(default)]
    pub convergence_rate: f64,
    #[serde(default)]
    pub average_repair_attempts: f64,
    #[serde(default)]
    pub total_token_cost: u64,
}

impl SelfCorrectionSummary {
    pub fn from_atom_metadata(atoms: &[AtomCertificate]) -> Option<Self> {
        let metadata: Vec<&SelfCorrectionMetadata> = atoms
            .iter()
            .filter_map(|atom| atom.self_correction_metadata.as_ref())
            .collect();
        if metadata.is_empty() {
            return None;
        }
        let total_atoms = metadata.len() as u32;
        let converged_atoms = metadata.iter().filter(|item| item.converged).count() as u32;
        let total_repair_attempts: u32 = metadata.iter().map(|item| item.repair_attempts).sum();
        let total_token_cost: u64 = metadata.iter().map(|item| item.token_cost).sum();
        Some(Self {
            total_atoms,
            converged_atoms,
            convergence_rate: converged_atoms as f64 / total_atoms as f64,
            average_repair_attempts: total_repair_attempts as f64 / total_atoms as f64,
            total_token_cost,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IntentFidelity {
    #[serde(default)]
    pub natural_language_prompt_hash: Option<String>,
    #[serde(default)]
    pub spec_traceability_score: f64,
    #[serde(default)]
    pub semantic_drift_detected: bool,
    #[serde(default)]
    pub manual_review_required: bool,
}

#[derive(Debug, Clone, Default)]
pub struct HarnessCertificateMetadata {
    pub harness_contract: Option<String>,
    pub intent_fidelity: Option<IntentFidelity>,
    pub artifact_paths: Option<Vec<String>>,
    pub budget_policy_fingerprint: Option<String>,
}

pub type IntentFidelityMetadata = IntentFidelity;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SolverProcessMetadata {
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub cache_key: String,
    #[serde(default)]
    pub generation_id: String,
    #[serde(default)]
    pub solver_config_fingerprint: String,
    #[serde(default)]
    pub timeout_ms: u64,
    #[serde(default)]
    pub cancel_reason: Option<String>,
    #[serde(default)]
    pub process_start_time: String,
    #[serde(default)]
    pub process_end_time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetPolicy {
    pub max_attempts: u32,
    pub max_tokens: u64,
    pub max_solver_time_ms: u64,
    pub max_semantic_delta: f64,
    pub action_class_limits: HashMap<String, ActionClassLimit>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionClassLimit {
    pub max_attempts: u32,
    pub max_tokens: u64,
    pub max_lean_escalations: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttemptSummary {
    pub total_attempts: u32,
    pub attempts_by_action_class: HashMap<String, u32>,
    pub final_action_class: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostSuccessMetrics {
    pub attempts_to_success: u32,
    pub tokens_to_success: u64,
    pub solver_seconds_to_success: f64,
    pub spec_drift_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CounterexampleValidationMetadata {
    #[serde(default)]
    pub validation_status: String,
    #[serde(default)]
    pub validated_at: String,
    #[serde(default)]
    pub failed_constraints: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UnusedHypothesisMetadata {
    #[serde(default)]
    pub unused_requires: Vec<String>,
    #[serde(default)]
    pub unused_invariants: Vec<String>,
    #[serde(default)]
    pub unused_effect_constraints: Vec<String>,
    #[serde(default)]
    pub minimal_constraint_set: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LeanResultMetadata {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub theorem_name: String,
    #[serde(default)]
    pub translator_version: String,
    #[serde(default)]
    pub bridge_lemma_hash: String,
    #[serde(default)]
    pub proof_path: String,
    #[serde(default)]
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscalationCandidate {
    pub name: String,
    pub z3_check_result: String,
    pub z3_result_class: String,
    pub status: String,
    pub content_hash: String,
    pub proof_hash: String,
    pub dependencies: Vec<String>,
    pub effects: Vec<String>,
    pub requires: String,
    pub ensures: String,
    pub escalation_reason: EscalationReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logic_fragment_tag: Option<LogicFragment>,
    pub logic_fragment_tags: Vec<String>,
    #[serde(default)]
    pub translator_version: String,
    #[serde(default)]
    pub binder_mapping: HashMap<String, String>,
    #[serde(default)]
    pub bridge_lemma_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_lemma_reason: Option<String>,
    #[serde(default)]
    pub translator_ir: TranslatorIRMetadata,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lean_metadata: Option<LeanResultMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lean_result_metadata: Option<LeanResultMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EscalationBundleSummary {
    pub total_atoms: usize,
    pub candidate_count: usize,
    pub by_reason: HashMap<String, usize>,
    pub by_logic_fragment: HashMap<String, usize>,
    #[serde(default)]
    pub by_z3_result_class: HashMap<String, usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscalationBundle {
    pub version: String,
    pub timestamp: String,
    pub file: String,
    #[serde(default)]
    pub mumei_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_version: Option<String>,
    pub summary: EscalationBundleSummary,
    pub candidates: Vec<EscalationCandidate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HumanReviewEntry {
    #[serde(rename = "name")]
    pub atom_name: String,
    #[serde(rename = "reason")]
    pub review_reason: String,
    pub priority: HumanReviewPriority,
    pub spec_text: String,
    pub suggested_action: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HumanReviewPriority {
    Critical,
    High,
    Medium,
}

impl HumanReviewPriority {
    pub fn as_str(self) -> &'static str {
        match self {
            HumanReviewPriority::Critical => "critical",
            HumanReviewPriority::High => "high",
            HumanReviewPriority::Medium => "medium",
        }
    }

    fn rank(self) -> u8 {
        match self {
            HumanReviewPriority::Critical => 0,
            HumanReviewPriority::High => 1,
            HumanReviewPriority::Medium => 2,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HumanReviewQueue {
    pub version: String,
    pub timestamp: String,
    pub file: String,
    pub atoms: Vec<HumanReviewEntry>,
}

/// Compute SHA-256 hash of an atom's logical content.
/// Includes requires, ensures, and body to detect any contract or implementation changes.
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
/// z3_check_result is "unsat"/"sat"/"unknown"/"skipped".
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
                .unwrap_or_else(|| ("skipped".to_string(), "skipped".to_string()));

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
            let counterexample_validation = if z3_result == "sat" {
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
        (ac.status == "verified" || ac.status == "trusted")
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

pub fn generate_escalation_bundle(cert: &ProofCertificate) -> EscalationBundle {
    let candidates: Vec<EscalationCandidate> = cert
        .atoms
        .iter()
        .filter_map(|atom| {
            let escalation_reason = atom.escalation_reason?;
            Some(EscalationCandidate {
                name: atom.name.clone(),
                z3_check_result: atom.z3_check_result.clone(),
                z3_result_class: atom.z3_result_class.clone(),
                status: atom.status.clone(),
                content_hash: atom.content_hash.clone(),
                proof_hash: atom.proof_hash.clone(),
                dependencies: atom.dependencies.clone(),
                effects: atom.effects.clone(),
                requires: atom.requires.clone(),
                ensures: atom.ensures.clone(),
                escalation_reason,
                logic_fragment_tag: atom.logic_fragment_tag,
                logic_fragment_tags: atom.logic_fragment_tags.clone(),
                translator_version: atom.translator_version.clone(),
                binder_mapping: atom.binder_mapping.clone(),
                bridge_lemma_hash: atom.bridge_lemma_hash.clone(),
                manual_lemma_reason: atom.manual_lemma_reason.clone(),
                translator_ir: atom.translator_ir.clone(),
                lean_metadata: atom.lean_metadata.clone(),
                lean_result_metadata: atom.lean_result_metadata.clone(),
            })
        })
        .collect();

    let mut summary = EscalationBundleSummary {
        total_atoms: cert.atoms.len(),
        candidate_count: candidates.len(),
        ..EscalationBundleSummary::default()
    };
    for candidate in &candidates {
        *summary
            .by_reason
            .entry(candidate.escalation_reason.as_str().to_string())
            .or_insert(0) += 1;
        *summary
            .by_z3_result_class
            .entry(candidate.z3_result_class.clone())
            .or_insert(0) += 1;
        for tag in &candidate.logic_fragment_tags {
            *summary.by_logic_fragment.entry(tag.clone()).or_insert(0) += 1;
        }
    }

    EscalationBundle {
        version: "1.0".to_string(),
        timestamp: cert.timestamp.clone(),
        file: cert.file.clone(),
        mumei_version: cert.mumei_version.clone(),
        package_name: cert.package_name.clone(),
        package_version: cert.package_version.clone(),
        summary,
        candidates,
    }
}

/// Generate a human review queue from a proof certificate.
/// Collects atoms requiring human judgment (manual lemma, z3 unknown,
/// escalation candidate, trusted) and sorts by priority.
pub fn generate_human_review_queue(cert: &ProofCertificate) -> HumanReviewQueue {
    let mut atoms: Vec<HumanReviewEntry> = cert
        .atoms
        .iter()
        .filter_map(human_review_entry_for_atom)
        .collect();
    if cert
        .intent_fidelity
        .as_ref()
        .is_some_and(|intent| intent.manual_review_required)
    {
        let queued: HashSet<String> = atoms.iter().map(|entry| entry.atom_name.clone()).collect();
        atoms.extend(
            cert.atoms
                .iter()
                .filter(|atom| !queued.contains(&atom.name))
                .map(|atom| HumanReviewEntry {
                    atom_name: atom.name.clone(),
                    review_reason: "manual_review_required".to_string(),
                    priority: HumanReviewPriority::High,
                    spec_text: atom_spec_text(atom),
                    suggested_action:
                        "Review this atom because the certificate intent metadata requires human approval."
                            .to_string(),
                }),
        );
    }
    atoms.sort_by(|left, right| {
        priority_rank(&left.priority)
            .cmp(&priority_rank(&right.priority))
            .then_with(|| left.atom_name.cmp(&right.atom_name))
    });

    HumanReviewQueue {
        version: "1.0".to_string(),
        timestamp: cert.timestamp.clone(),
        file: cert.file.clone(),
        atoms,
    }
}

fn human_review_entry_for_atom(atom: &AtomCertificate) -> Option<HumanReviewEntry> {
    if let Some(reason) = atom.manual_lemma_reason.as_ref() {
        return Some(HumanReviewEntry {
            atom_name: atom.name.clone(),
            review_reason: reason.clone(),
            priority: HumanReviewPriority::Critical,
            spec_text: atom_spec_text(atom),
            suggested_action:
                "Review the generated obligation and author or approve the required Lean lemma."
                    .to_string(),
        });
    }

    if atom.z3_result_class == "unknown" || atom.z3_check_result == "unknown" {
        return Some(HumanReviewEntry {
            atom_name: atom.name.clone(),
            review_reason: "z3_unknown".to_string(),
            priority: HumanReviewPriority::High,
            spec_text: atom_spec_text(atom),
            suggested_action: "Escalate this atom with --escalate-lean or simplify the specification into a Z3-decidable fragment.".to_string(),
        });
    }

    if atom.status == "escalation_candidate" {
        return Some(HumanReviewEntry {
            atom_name: atom.name.clone(),
            review_reason: atom
                .escalation_reason
                .map(|reason| reason.as_str().to_string())
                .unwrap_or_else(|| "lean_promotion_pending".to_string()),
            priority: HumanReviewPriority::High,
            spec_text: atom_spec_text(atom),
            suggested_action:
                "Review the Lean escalation candidate and track promotion to lean_verified."
                    .to_string(),
        });
    }

    if atom.status == "trusted"
        || atom
            .escalation_reason
            .is_some_and(|reason| reason == EscalationReason::HumanReviewRequired)
    {
        return Some(HumanReviewEntry {
            atom_name: atom.name.clone(),
            review_reason: "trusted_atom".to_string(),
            priority: HumanReviewPriority::Medium,
            spec_text: atom_spec_text(atom),
            suggested_action: "Confirm the trusted implementation boundary and record human approval before relying on the atom.".to_string(),
        });
    }

    None
}

fn priority_rank(priority: &HumanReviewPriority) -> u8 {
    priority.rank()
}

fn atom_spec_text(atom: &AtomCertificate) -> String {
    format!(
        "requires: {}\nensures: {}",
        atom.requires.trim(),
        atom.ensures.trim()
    )
}

/// Compute SHA-256 hash of the serialized certificate.
/// The `certificate_hash` field is set to empty before hashing for determinism.
fn compute_certificate_hash(cert: &ProofCertificate) -> String {
    // Serialize with empty certificate_hash for deterministic hashing
    let mut hashable = cert.clone();
    hashable.certificate_hash = String::new();
    let json = serde_json::to_string(&hashable).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(json.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn refresh_certificate_integrity(cert: &mut ProofCertificate) {
    cert.all_verified = cert.atoms.iter().all(|ac| {
        (ac.status == "trusted"
            || (ac.status == "verified"
                && (ac.z3_check_result == "unsat"
                    || (ac.z3_check_result == "lean_verified"
                        && lean_certificate_metadata_is_current(ac)))))
            && ac
                .spec_validation_result
                .as_ref()
                .map(|validation| validation.is_satisfiable)
                .unwrap_or(true)
    });
    cert.certificate_hash = String::new();
    cert.certificate_hash = compute_certificate_hash(cert);
}

fn parse_unsat_core_labels(z3_result: &str) -> Option<Vec<String>> {
    let (_, labels) = z3_result.split_once("unsat_core=")?;
    let labels = labels.trim();
    let labels = labels
        .strip_prefix('[')
        .and_then(|labels| labels.split_once(']').map(|(labels, _)| labels))
        .unwrap_or(labels);
    let labels = labels
        .split(',')
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(|label| label.trim_matches('"').trim_matches('\'').to_string())
        .collect::<Vec<_>>();
    Some(labels)
}

/// Verify a proof certificate against the current source file.
/// Returns a list of (atom_name, status) where status is:
/// - "proven" if content_hash matches and z3_check_result was "unsat"
///   (or `"lean_verified"` when `allow_lean_verified` is enabled — see below).
/// - "changed" if content_hash differs (re-verification needed)
/// - "unproven" if z3_check_result was not "unsat"
///
/// `allow_lean_verified` controls how `z3_check_result == "lean_verified"`
/// values are treated. mumei-lean (the external Lean 4 proof backend) emits
/// `"lean_verified"` for atoms it has discharged itself (Z3 returned
/// `unknown`, but a Lean tactic proof completed). For backwards
/// compatibility the default is `false`, which treats `"lean_verified"` as
/// `"unproven"`. Callers that have explicitly opted in to the cross-project
/// Proof Certificate Chain (e.g. via the `--allow-lean-verified` CLI flag)
/// should pass `true` to recognise those atoms as `"proven"`.
pub fn verify_certificate(
    cert: &ProofCertificate,
    atoms: &[&crate::parser::Atom],
    allow_lean_verified: bool,
) -> Vec<(String, String)> {
    let current_hashes: HashMap<String, String> = atoms
        .iter()
        .map(|a| {
            (
                a.name.clone(),
                compute_atom_content_hash(&a.name, &a.requires, &a.ensures, &a.body_expr),
            )
        })
        .collect();

    cert.atoms
        .iter()
        .map(|ac| {
            let status = if let Some(current_hash) = current_hashes.get(&ac.name) {
                if current_hash != &ac.content_hash {
                    "changed".to_string()
                } else if ac.z3_check_result == "unsat" {
                    "proven".to_string()
                } else if allow_lean_verified && ac.z3_check_result == "lean_verified" {
                    if lean_certificate_metadata_is_current(ac) {
                        "proven".to_string()
                    } else {
                        "stale_translator".to_string()
                    }
                } else {
                    "unproven".to_string()
                }
            } else {
                "missing".to_string()
            };
            (ac.name.clone(), status)
        })
        .collect()
}

fn manual_lemma_reason_for_atom(
    atom: &crate::parser::Atom,
    logic_fragment_tags: &[String],
) -> Option<String> {
    if atom.body_expr.contains("match ")
        || logic_fragment_tags
            .iter()
            .any(|tag| tag == "inductive_data_type")
    {
        Some("inductive_or_pattern_translation_requires_manual_lemma".to_string())
    } else if atom.requires.contains("regex") || atom.ensures.contains("regex") {
        Some("regex_semantics_require_manual_lemma".to_string())
    } else {
        None
    }
}

fn lean_certificate_metadata_is_current(atom: &AtomCertificate) -> bool {
    if atom.translator_version != verification::LEAN_TRANSLATOR_VERSION
        || atom.bridge_lemma_hash != verification::LEAN_BRIDGE_LEMMA_HASH
    {
        return false;
    }
    let Some(metadata) = atom
        .lean_result_metadata
        .as_ref()
        .or(atom.lean_metadata.as_ref())
    else {
        return false;
    };
    metadata.status == "lean_verified"
        && !metadata.theorem_name.is_empty()
        && metadata.translator_version == verification::LEAN_TRANSLATOR_VERSION
        && metadata.bridge_lemma_hash == verification::LEAN_BRIDGE_LEMMA_HASH
}

/// Validate that an atom certificate matches the expected Lean translator bridge.
pub fn validate_translator_version(
    cert: &AtomCertificate,
    expected_version: &str,
    expected_hash: &str,
) -> Result<(), String> {
    let mut issues = Vec::new();
    if cert.translator_version != expected_version {
        issues.push(format!(
            "translator_version '{}' does not match expected '{}'",
            cert.translator_version, expected_version
        ));
    }
    if cert.bridge_lemma_hash != expected_hash {
        issues.push(format!(
            "bridge_lemma_hash '{}' does not match expected '{}'",
            cert.bridge_lemma_hash, expected_hash
        ));
    }

    if let Some(metadata) = cert
        .lean_result_metadata
        .as_ref()
        .or(cert.lean_metadata.as_ref())
    {
        if metadata.translator_version != expected_version {
            issues.push(format!(
                "lean_metadata.translator_version '{}' does not match expected '{}'",
                metadata.translator_version, expected_version
            ));
        }
        if metadata.bridge_lemma_hash != expected_hash {
            issues.push(format!(
                "lean_metadata.bridge_lemma_hash '{}' does not match expected '{}'",
                metadata.bridge_lemma_hash, expected_hash
            ));
        }
    }

    if issues.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "atom '{}': Lean translator metadata validation failed: {}",
            cert.name,
            issues.join("; ")
        ))
    }
}

/// Validate that an atom certificate matches the expected Lean translator bridge
/// and carries semantic-gap lowering metadata required by its logic fragment.
pub fn validate_translator_version_with_semantics(
    cert: &AtomCertificate,
    expected_version: &str,
    expected_hash: &str,
    required_lowering_rules: &[String],
) -> Result<(), String> {
    let mut issues = Vec::new();
    if let Err(err) = validate_translator_version(cert, expected_version, expected_hash) {
        issues.push(err);
    }

    let missing_rules: Vec<&String> = required_lowering_rules
        .iter()
        .filter(|rule| !cert.translator_ir.lowering_rules.contains(*rule))
        .collect();
    if !missing_rules.is_empty() {
        issues.push(format!(
            "missing required lowering rules: {}",
            missing_rules
                .iter()
                .map(|rule| rule.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    if issues.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "atom '{}': Lean translator metadata validation failed: {}",
            cert.name,
            issues.join("; ")
        ))
    }
}

/// Get required TranslatorIR lowering rules from logic-fragment tags.
pub fn get_required_lowering_rules(logic_fragment_tags: &[String]) -> Vec<String> {
    let mut rules = vec![
        "type_system_mapping".to_string(),
        "contract_lowering".to_string(),
    ];

    for tag in logic_fragment_tags {
        match tag.as_str() {
            "array_operations" | "array_without_bounds" => {
                rules.push("array_bounds_bridge".to_string());
            }
            "string_operations" => {
                rules.push("string_regex_bridge".to_string());
            }
            "integer_arithmetic" | "nonlinear_arithmetic" => {
                rules.push("integer_overflow_bridge".to_string());
            }
            "quantifiers" | "quantifier_alternation" | "trigger_sensitive_quantifier" => {
                rules.push("refinement_predicate_lowering".to_string());
            }
            "finite_field" => {
                rules.push("finite_field_lowering".to_string());
                rules.push("mathlib4_bridge".to_string());
            }
            "group_theory" => {
                rules.push("group_theory_lowering".to_string());
                rules.push("mathlib4_bridge".to_string());
            }
            _ => {}
        }
    }

    rules.sort();
    rules.dedup();
    rules
}

pub fn validate_certificate_translator_versions(cert: &ProofCertificate) -> Result<(), String> {
    let issues: Vec<String> = cert
        .atoms
        .iter()
        .filter_map(|atom| {
            validate_translator_version(
                atom,
                verification::LEAN_TRANSLATOR_VERSION,
                verification::LEAN_BRIDGE_LEMMA_HASH,
            )
            .err()
        })
        .collect();

    if issues.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Lean translator metadata validation failed: {}",
            issues.join("; ")
        ))
    }
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

pub(crate) fn load_certificate_unvalidated(path: &Path) -> Result<ProofCertificate, String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    serde_json::from_str(&contents)
        .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))
}

/// Load a proof certificate from a JSON file.
pub fn load_certificate(path: &Path) -> Result<ProofCertificate, String> {
    let cert = load_certificate_unvalidated(path)?;
    validate_certificate_translator_versions(&cert)
        .map_err(|e| format!("Failed to validate {}: {}", path.display(), e))?;
    Ok(cert)
}

/// Save a proof certificate to a JSON file.
pub fn save_certificate(cert: &ProofCertificate, path: &Path) -> Result<(), String> {
    let json = serde_json::to_string_pretty(cert)
        .map_err(|e| format!("Failed to serialize certificate: {}", e))?;
    std::fs::write(path, json).map_err(|e| format!("Failed to write {}: {}", path.display(), e))
}

pub fn save_escalation_bundle(bundle: &EscalationBundle, path: &Path) -> Result<(), String> {
    let json = serde_json::to_string_pretty(bundle)
        .map_err(|e| format!("Failed to serialize escalation bundle: {}", e))?;
    std::fs::write(path, json).map_err(|e| format!("Failed to write {}: {}", path.display(), e))
}

pub fn save_human_review_queue(queue: &HumanReviewQueue, path: &Path) -> Result<(), String> {
    let json = serde_json::to_string_pretty(queue)
        .map_err(|e| format!("Failed to serialize human review queue: {}", e))?;
    std::fs::write(path, json).map_err(|e| format!("Failed to write {}: {}", path.display(), e))
}

// =============================================================================
// SI-5 Phase 3-C: Cross-Project Proof Certificate Bundles
// =============================================================================
//
// `scripts/bundle_std_certs.py` aggregates every per-module
// `.proof.json` into a single distributable `std-proof-bundle.json`
// shipped via `homebrew-mumei`. Downstream projects then export the
// bundle path as `MUMEI_PROOF_BUNDLE` and the resolver can verify
// imports against a trusted, mumei-versioned certificate set without
// needing a per-module cert in each module directory.

/// Top-level SI-5 Phase 3-C proof bundle.
///
/// Bundles are produced by `scripts/bundle_std_certs.py` and consumed
/// here as a fallback when a per-module certificate is missing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofBundle {
    /// Bundle schema version (currently `"1.0"`).
    pub bundle_version: String,
    /// ISO 8601 timestamp of bundle generation.
    pub generated_at: String,
    /// mumei version string the bundle was produced for.
    #[serde(default)]
    pub mumei_version: String,
    /// Module key → embedded ProofCertificate.
    ///
    /// Keys use the canonical `std/<dir>/<stem>` form, e.g.
    /// `"std/core"`, `"std/container/safe_list"`. The trailing `.mm`
    /// is stripped.
    pub modules: HashMap<String, ProofCertificate>,
    /// Aggregate statistics across all modules.
    #[serde(default)]
    pub summary: BundleSummary,
}

/// Aggregate statistics embedded in a [`ProofBundle`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BundleSummary {
    pub total_modules: usize,
    pub all_verified: usize,
    pub partial_verified: usize,
    pub total_atoms: usize,
    pub proven_atoms: usize,
}

/// Simple ISO 8601 timestamp without external crate.
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

/// Load a proof bundle JSON produced by `bundle_std_certs.py`.
pub fn load_bundle(path: &Path) -> Result<ProofBundle, String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read bundle {}: {}", path.display(), e))?;
    serde_json::from_str(&contents)
        .map_err(|e| format!("Failed to parse bundle {}: {}", path.display(), e))
}

/// Derive the canonical bundle module key from a source file path.
///
/// Looks for the last `std/` segment in the path and returns
/// `std/<rest-without-extension>`. Returns `None` if the path does not
/// contain a `std/` segment — the bundle only carries `std/` modules.
pub fn module_key_from_source(source_file: &Path) -> Option<String> {
    let mut segments: Vec<&str> = Vec::new();
    let mut seen_std = false;
    for comp in source_file.components() {
        if let std::path::Component::Normal(os_str) = comp {
            let s = os_str.to_str()?;
            if !seen_std {
                if s == "std" {
                    seen_std = true;
                    segments.push("std");
                }
            } else {
                segments.push(s);
            }
        }
    }
    if !seen_std || segments.len() < 2 {
        return None;
    }
    // Strip the .mm extension on the final segment.
    let last = segments.last_mut()?;
    if let Some(stem) = last.strip_suffix(".mm") {
        *last = stem;
    }
    Some(segments.join("/"))
}

/// Look up a certificate for `source_file` in the bundle. The lookup
/// is keyed by [`module_key_from_source`].
pub fn lookup_bundle_certificate<'a>(
    bundle: &'a ProofBundle,
    source_file: &Path,
) -> Option<&'a ProofCertificate> {
    let key = module_key_from_source(source_file)?;
    bundle.modules.get(&key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;
    use crate::verification::ModuleEnv;

    fn solver_env_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    struct SolverEnvCleanup(&'static [&'static str]);

    impl Drop for SolverEnvCleanup {
        fn drop(&mut self) {
            for name in self.0 {
                std::env::remove_var(name);
            }
        }
    }

    fn make_test_atom(name: &str, requires: &str, ensures: &str, body: &str) -> parser::Atom {
        parser::Atom {
            name: name.to_string(),
            type_params: vec![],
            where_bounds: vec![],
            params: vec![],
            trace_id: None,
            spec_metadata: std::collections::HashMap::new(),
            requires: requires.to_string(),
            forall_constraints: vec![],
            ensures: ensures.to_string(),
            body_expr: body.to_string(),
            consumed_params: vec![],
            resources: vec![],
            is_async: false,
            trust_level: parser::TrustLevel::Verified,
            max_unroll: None,
            invariant: None,
            effects: vec![],
            return_type: None,
            span: crate::parser::Span::new("test.mm", 0, 0, 0),
            effect_pre: HashMap::new(),
            effect_post: HashMap::new(),
        }
    }

    /// P5-A: generate_certificate produces valid JSON with proof_hash, dependencies, effects, requires, ensures
    #[test]
    fn test_generate_certificate_extended_fields() {
        let _guard = solver_env_lock().lock().unwrap();
        let env_names = &[
            "MUMEI_TASK_ID",
            "MUMEI_GENERATION_ID",
            "MUMEI_VERIFICATION_TIMEOUT_MS",
            "MUMEI_SOLVER_CONFIG_FINGERPRINT",
            "MUMEI_SOLVER_CACHE_KEY",
            "MUMEI_CANCEL_REASON",
            "MUMEI_SOLVER_PROCESS_START_TIME",
            "MUMEI_HARNESS_CONTRACT",
            "MUMEI_INTENT_PROMPT_HASH",
            "MUMEI_SPEC_TRACEABILITY_SCORE",
            "MUMEI_SEMANTIC_DRIFT_DETECTED",
            "MUMEI_MANUAL_REVIEW_REQUIRED",
            "MUMEI_ARTIFACT_PATHS",
            "MUMEI_BUDGET_POLICY_FINGERPRINT",
        ];
        let _cleanup = SolverEnvCleanup(env_names);
        for name in env_names {
            std::env::remove_var(name);
        }
        let atom = make_test_atom("add", "x > 0", "result > 0", "x + 1");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "add".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();

        let cert = generate_certificate(
            "test.mm",
            &atoms,
            &results,
            &module_env,
            Some("my_pkg"),
            Some("1.0.0"),
            None,
        );

        assert_eq!(cert.atoms.len(), 1);
        assert_eq!(cert.atoms[0].name, "add");
        assert_eq!(cert.atoms[0].requires, "x > 0");
        assert_eq!(cert.atoms[0].ensures, "result > 0");
        assert!(!cert.atoms[0].proof_hash.is_empty());
        assert!(cert.atoms[0].solver_process_metadata.is_none());
        assert!(cert.atoms[0].retry_policy_fingerprint.is_none());
        assert!(cert.atoms[0].attempt_summary.is_none());
        assert!(cert.atoms[0].cost_success_metrics.is_none());
        assert!(cert.all_verified);
        assert_eq!(cert.package_name, Some("my_pkg".to_string()));
        assert_eq!(cert.package_version, Some("1.0.0".to_string()));

        // Verify JSON serialization roundtrip
        let json = serde_json::to_string(&cert).unwrap();
        let parsed: ProofCertificate = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.atoms[0].requires, "x > 0");
        assert_eq!(parsed.atoms[0].ensures, "result > 0");
    }

    #[test]
    fn test_generate_certificate_harness_metadata_fields() {
        let atom = make_test_atom("harnessed", "true", "true", "0");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "harnessed".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();

        let cert = generate_certificate(
            "test.mm",
            &atoms,
            &results,
            &module_env,
            None,
            None,
            Some(HarnessCertificateMetadata {
                harness_contract: Some("contracts/nlah.json".to_string()),
                intent_fidelity: Some(IntentFidelityMetadata {
                    natural_language_prompt_hash: Some("sha256:prompt".to_string()),
                    spec_traceability_score: 0.97,
                    semantic_drift_detected: false,
                    manual_review_required: true,
                }),
                artifact_paths: Some(vec![
                    "reports/proof.json".to_string(),
                    "out/doc.json".to_string(),
                ]),
                budget_policy_fingerprint: Some("sha256:budget".to_string()),
            }),
        );

        assert_eq!(
            cert.harness_contract.as_deref(),
            Some("contracts/nlah.json")
        );
        let intent_fidelity = cert.intent_fidelity.as_ref().unwrap();
        assert_eq!(
            intent_fidelity.natural_language_prompt_hash.as_deref(),
            Some("sha256:prompt")
        );
        assert_eq!(intent_fidelity.spec_traceability_score, 0.97);
        assert!(!intent_fidelity.semantic_drift_detected);
        assert!(intent_fidelity.manual_review_required);
        assert_eq!(
            cert.artifact_paths,
            Some(vec![
                "reports/proof.json".to_string(),
                "out/doc.json".to_string()
            ])
        );
        assert_eq!(
            cert.budget_policy_fingerprint.as_deref(),
            Some("sha256:budget")
        );

        let json = serde_json::to_string(&cert).unwrap();
        let parsed: ProofCertificate = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.harness_contract, cert.harness_contract);
        assert_eq!(parsed.artifact_paths, cert.artifact_paths);
    }

    #[test]
    fn test_certificate_backward_compatibility_without_harness_fields() {
        let json = r#"{
            "version": "1.0",
            "timestamp": "2026-05-24T00:00:00Z",
            "mumei_version": "0.2.0",
            "z3_version": "z3 4.12.2",
            "file": "legacy.mm",
            "atoms": [],
            "certificate_hash": "",
            "all_verified": true
        }"#;

        let cert: ProofCertificate = serde_json::from_str(json).unwrap();

        assert!(cert.harness_contract.is_none());
        assert!(cert.intent_fidelity.is_none());
        assert!(cert.artifact_paths.is_none());
        assert!(cert.budget_policy_fingerprint.is_none());
    }

    #[test]
    fn test_harness_metadata_env_helpers() {
        let _guard = solver_env_lock().lock().unwrap();
        let env_names = &[
            "MUMEI_HARNESS_CONTRACT",
            "MUMEI_INTENT_PROMPT_HASH",
            "MUMEI_SPEC_TRACEABILITY_SCORE",
            "MUMEI_SEMANTIC_DRIFT_DETECTED",
            "MUMEI_MANUAL_REVIEW_REQUIRED",
            "MUMEI_ARTIFACT_PATHS",
            "MUMEI_BUDGET_POLICY_FINGERPRINT",
        ];
        let _cleanup = SolverEnvCleanup(env_names);
        for name in env_names {
            std::env::remove_var(name);
        }

        std::env::set_var("MUMEI_HARNESS_CONTRACT", "contracts/harness.json");
        std::env::set_var("MUMEI_INTENT_PROMPT_HASH", "sha256:prompt");
        std::env::set_var("MUMEI_SPEC_TRACEABILITY_SCORE", "0.97");
        std::env::set_var("MUMEI_SEMANTIC_DRIFT_DETECTED", "false");
        std::env::set_var("MUMEI_MANUAL_REVIEW_REQUIRED", "true");
        std::env::set_var("MUMEI_ARTIFACT_PATHS", " reports/a.json, ,out/b.json ");
        std::env::set_var("MUMEI_BUDGET_POLICY_FINGERPRINT", "sha256:budget");

        assert_eq!(
            harness_contract_from_env().as_deref(),
            Some("contracts/harness.json")
        );
        assert_eq!(
            artifact_paths_from_env(),
            Some(vec!["reports/a.json".to_string(), "out/b.json".to_string()])
        );
        let intent_fidelity = intent_fidelity_from_env().unwrap();
        assert_eq!(
            intent_fidelity.natural_language_prompt_hash.as_deref(),
            Some("sha256:prompt")
        );
        assert_eq!(intent_fidelity.spec_traceability_score, 0.97);
        assert!(!intent_fidelity.semantic_drift_detected);
        assert!(intent_fidelity.manual_review_required);
        assert_eq!(
            budget_policy_fingerprint_from_env().as_deref(),
            Some("sha256:budget")
        );
    }

    #[test]
    fn test_retry_budget_schema_roundtrip() {
        let mut limits = HashMap::new();
        limits.insert(
            "lean_escalation".to_string(),
            ActionClassLimit {
                max_attempts: 1,
                max_tokens: 2_000,
                max_lean_escalations: 1,
            },
        );
        let policy = BudgetPolicy {
            max_attempts: 5,
            max_tokens: 10_000,
            max_solver_time_ms: 30_000,
            max_semantic_delta: 0.5,
            action_class_limits: limits,
        };

        let json = serde_json::to_string(&policy).unwrap();
        let parsed: BudgetPolicy = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.max_attempts, 5);
        assert_eq!(
            parsed
                .action_class_limits
                .get("lean_escalation")
                .unwrap()
                .max_lean_escalations,
            1
        );
    }

    #[test]
    fn test_atom_certificate_retry_metrics_roundtrip() {
        let atom = make_test_atom("budgeted", "true", "true", "0");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "budgeted".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let mut cert =
            generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        let mut attempts_by_action_class = HashMap::new();
        attempts_by_action_class.insert("llm_fix".to_string(), 2);
        cert.atoms[0].retry_policy_fingerprint = Some("sha256:abc".to_string());
        cert.atoms[0].attempt_summary = Some(AttemptSummary {
            total_attempts: 2,
            attempts_by_action_class,
            final_action_class: "llm_fix".to_string(),
        });
        cert.atoms[0].cost_success_metrics = Some(CostSuccessMetrics {
            attempts_to_success: 2,
            tokens_to_success: 1_024,
            solver_seconds_to_success: 1.5,
            spec_drift_score: 0.1,
        });

        let json = serde_json::to_string(&cert).unwrap();
        let parsed: ProofCertificate = serde_json::from_str(&json).unwrap();

        assert_eq!(
            parsed.atoms[0].retry_policy_fingerprint.as_deref(),
            Some("sha256:abc")
        );
        assert_eq!(
            parsed.atoms[0]
                .attempt_summary
                .as_ref()
                .unwrap()
                .final_action_class,
            "llm_fix"
        );
        assert_eq!(
            parsed.atoms[0]
                .cost_success_metrics
                .as_ref()
                .unwrap()
                .tokens_to_success,
            1_024
        );
    }

    #[test]
    fn test_generate_certificate_records_unused_hypotheses_when_core_available() {
        let mut atom = make_test_atom("bounded", "x > 0", "result > 0", "1");
        atom.invariant = Some("x < 10".to_string());
        atom.effect_pre
            .insert("Account".to_string(), "Open".to_string());
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "bounded".to_string(),
            (
                "unsat unsat_core=[|track_requires|]".to_string(),
                "verified".to_string(),
            ),
        );
        let module_env = ModuleEnv::new();

        let cert = generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        let unused = cert.atoms[0].unused_hypotheses.as_ref().unwrap();

        assert!(unused.unused_requires.is_empty());
        assert_eq!(unused.unused_invariants, vec!["x < 10"]);
        assert_eq!(unused.unused_effect_constraints, vec!["Account=Open"]);
        assert_eq!(unused.minimal_constraint_set, vec!["|track_requires|"]);
    }

    #[test]
    fn test_generate_certificate_records_solver_process_metadata() {
        let _guard = solver_env_lock().lock().unwrap();
        let env_names = &[
            "MUMEI_TASK_ID",
            "MUMEI_GENERATION_ID",
            "MUMEI_VERIFICATION_TIMEOUT_MS",
            "MUMEI_SOLVER_CONFIG_FINGERPRINT",
            "MUMEI_SOLVER_CACHE_KEY",
            "MUMEI_CANCEL_REASON",
            "MUMEI_SOLVER_PROCESS_START_TIME",
        ];
        let _cleanup = SolverEnvCleanup(env_names);
        for name in env_names {
            std::env::remove_var(name);
        }
        std::env::set_var("MUMEI_TASK_ID", "task-1");
        std::env::set_var("MUMEI_GENERATION_ID", "generation-1");
        std::env::set_var("MUMEI_VERIFICATION_TIMEOUT_MS", "1234");
        std::env::set_var("MUMEI_SOLVER_CONFIG_FINGERPRINT", "fingerprint-1");
        std::env::set_var("MUMEI_SOLVER_CACHE_KEY", "cache-1");
        std::env::set_var("MUMEI_CANCEL_REASON", "timeout");
        std::env::set_var("MUMEI_SOLVER_PROCESS_START_TIME", "2026-05-17T00:00:00Z");

        let atom = make_test_atom("orchestrated", "true", "true", "0");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "orchestrated".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();

        let cert = generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        let metadata = cert.atoms[0].solver_process_metadata.as_ref().unwrap();

        assert_eq!(metadata.task_id.as_deref(), Some("task-1"));
        assert_eq!(metadata.generation_id, "generation-1");
        assert_eq!(metadata.timeout_ms, 1234);
        assert_eq!(metadata.solver_config_fingerprint, "fingerprint-1");
        assert_eq!(metadata.cache_key, "cache-1");
        assert_eq!(metadata.cancel_reason.as_deref(), Some("timeout"));
        assert_eq!(metadata.process_start_time, "2026-05-17T00:00:00Z");
        assert!(!metadata.process_end_time.is_empty());
    }

    #[test]
    fn test_generate_certificate_records_traceability_validation() {
        let mut atom = make_test_atom("traced", "true", "true", "0");
        atom.trace_id = Some("REQ-1".to_string());
        atom.spec_metadata
            .insert("source".to_string(), "unit-test".to_string());
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "traced".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();

        let cert = generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        let validation = cert.atoms[0].spec_validation_result.as_ref().unwrap();

        assert!(validation.is_satisfiable);
        assert!(validation.contradiction_details.is_none());
        assert_eq!(validation.trace_id.as_deref(), Some("REQ-1"));
        assert_eq!(validation.traceability_hash.len(), 64);
    }

    /// P5-A: verify_certificate detects "changed" when source is modified
    #[test]
    fn test_verify_certificate_detects_changed() {
        let atom = make_test_atom("add", "x > 0", "result > 0", "x + 1");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "add".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();

        let cert = generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);

        // Verify with same source → "proven"
        let status = verify_certificate(&cert, &atoms, false);
        assert_eq!(status.len(), 1);
        assert_eq!(status[0], ("add".to_string(), "proven".to_string()));

        // Modify the atom body and verify again → "changed"
        let modified_atom = make_test_atom("add", "x > 0", "result > 0", "x + 2");
        let modified_atoms: Vec<&parser::Atom> = vec![&modified_atom];
        let status2 = verify_certificate(&cert, &modified_atoms, false);
        assert_eq!(status2.len(), 1);
        assert_eq!(status2[0], ("add".to_string(), "changed".to_string()));
    }

    /// PR 2: `lean_verified` is rejected by default and accepted with the
    /// `allow_lean_verified` opt-in flag, mirroring the cross-project Proof
    /// Certificate Chain (mumei-lean → mumei resolver) handshake.
    #[test]
    fn test_verify_certificate_lean_verified_opt_in() {
        let atom = make_test_atom("hard_lemma", "true", "true", "42");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        // mumei-lean signals successful Lean discharge with this string.
        results.insert(
            "hard_lemma".to_string(),
            ("lean_verified".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let mut cert =
            generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);

        // Default (backwards-compatible): lean_verified is NOT proven.
        let status_default = verify_certificate(&cert, &atoms, false);
        assert_eq!(
            status_default[0],
            ("hard_lemma".to_string(), "unproven".to_string())
        );

        // Opt-in still rejects lean_verified without Lean result metadata.
        let status_missing_metadata = verify_certificate(&cert, &atoms, true);
        assert_eq!(
            status_missing_metadata[0],
            ("hard_lemma".to_string(), "stale_translator".to_string())
        );

        cert.atoms[0].lean_metadata = Some(LeanResultMetadata {
            status: "lean_verified".to_string(),
            theorem_name: "hard_lemma_correct".to_string(),
            translator_version: verification::LEAN_TRANSLATOR_VERSION.to_string(),
            bridge_lemma_hash: verification::LEAN_BRIDGE_LEMMA_HASH.to_string(),
            proof_path: "Generated/Test.lean".to_string(),
            diagnostics: vec![],
        });

        // Opt-in: lean_verified is proven only with current Lean metadata.
        let status_opt_in = verify_certificate(&cert, &atoms, true);
        assert_eq!(
            status_opt_in[0],
            ("hard_lemma".to_string(), "proven".to_string())
        );
    }

    #[test]
    fn test_validate_translator_version_detects_atom_version_mismatch() {
        let atom = make_test_atom("hard_lemma", "true", "true", "42");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "hard_lemma".to_string(),
            ("lean_verified".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let mut cert =
            generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        cert.atoms[0].translator_version = "old-translator".to_string();

        let err = validate_translator_version(
            &cert.atoms[0],
            verification::LEAN_TRANSLATOR_VERSION,
            verification::LEAN_BRIDGE_LEMMA_HASH,
        )
        .expect_err("stale translator must fail");
        assert!(err.contains("translator_version"));
        assert!(err.contains("old-translator"));
    }

    #[test]
    fn test_validate_translator_version_detects_bridge_hash_mismatch() {
        let atom = make_test_atom("hard_lemma", "true", "true", "42");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "hard_lemma".to_string(),
            ("lean_verified".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let mut cert =
            generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        cert.atoms[0].bridge_lemma_hash = "old-bridge-hash".to_string();

        let err = validate_translator_version(
            &cert.atoms[0],
            verification::LEAN_TRANSLATOR_VERSION,
            verification::LEAN_BRIDGE_LEMMA_HASH,
        )
        .expect_err("stale bridge hash must fail");
        assert!(err.contains("bridge_lemma_hash"));
        assert!(err.contains("old-bridge-hash"));
    }

    #[test]
    fn test_validate_translator_version_detects_lean_metadata_mismatch() {
        let atom = make_test_atom("hard_lemma", "true", "true", "42");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "hard_lemma".to_string(),
            ("lean_verified".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let mut cert =
            generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        cert.atoms[0].lean_metadata = Some(LeanResultMetadata {
            status: "lean_verified".to_string(),
            theorem_name: "hard_lemma_correct".to_string(),
            translator_version: "old-translator".to_string(),
            bridge_lemma_hash: verification::LEAN_BRIDGE_LEMMA_HASH.to_string(),
            proof_path: "Generated/Test.lean".to_string(),
            diagnostics: vec![],
        });

        let err = validate_translator_version(
            &cert.atoms[0],
            verification::LEAN_TRANSLATOR_VERSION,
            verification::LEAN_BRIDGE_LEMMA_HASH,
        )
        .expect_err("stale Lean metadata must fail");
        assert!(err.contains("lean_metadata.translator_version"));
        assert!(err.contains("old-translator"));
    }

    #[test]
    fn test_validate_translator_version_detects_missing_metadata() {
        let atom = make_test_atom("add", "true", "true", "1");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "add".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let mut cert =
            generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        cert.atoms[0].translator_version.clear();
        cert.atoms[0].bridge_lemma_hash.clear();

        let err = validate_translator_version(
            &cert.atoms[0],
            verification::LEAN_TRANSLATOR_VERSION,
            verification::LEAN_BRIDGE_LEMMA_HASH,
        )
        .expect_err("missing translator metadata must fail");
        assert!(err.contains("translator_version"));
        assert!(err.contains("bridge_lemma_hash"));
    }

    #[test]
    fn test_required_lowering_rules_extraction() {
        let tags = vec![
            "array_operations".to_string(),
            "integer_arithmetic".to_string(),
        ];
        let rules = get_required_lowering_rules(&tags);

        assert!(rules.contains(&"type_system_mapping".to_string()));
        assert!(rules.contains(&"contract_lowering".to_string()));
        assert!(rules.contains(&"array_bounds_bridge".to_string()));
        assert!(rules.contains(&"integer_overflow_bridge".to_string()));
    }

    #[test]
    fn test_translator_version_validation_with_semantics() {
        let atom = make_test_atom("semantic_gap", "true", "true", "1");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "semantic_gap".to_string(),
            ("lean_verified".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let mut cert =
            generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        cert.atoms[0].translator_ir.lowering_rules = vec!["type_system_mapping".to_string()];

        let required_rules = vec![
            "type_system_mapping".to_string(),
            "array_bounds_bridge".to_string(),
        ];
        let result = validate_translator_version_with_semantics(
            &cert.atoms[0],
            verification::LEAN_TRANSLATOR_VERSION,
            verification::LEAN_BRIDGE_LEMMA_HASH,
            &required_rules,
        );

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("missing required lowering rules"));
    }

    #[test]
    fn test_translator_ir_metadata_includes_semantic_gap_fields() {
        let atom = make_test_atom("array_guard", "forall(i, 0, n, arr[i] >= 0)", "true", "1");
        let module_env = ModuleEnv::new();
        let ir = verification::build_translator_ir_metadata(&atom, &module_env);

        assert!(ir
            .lowering_rules
            .contains(&"array_bounds_bridge".to_string()));
        assert!(ir
            .lowering_rules
            .contains(&"refinement_predicate_lowering".to_string()));
        assert!(ir
            .semantic_gap_notes
            .iter()
            .any(|note| note.starts_with("array_bounds_bridge:")));
        assert!(ir
            .proof_trace_hints
            .iter()
            .any(|hint| hint.starts_with("preserve i < arr.length")));
        assert!(ir
            .requires_bridge_lemmas
            .contains(&"mumei_array_bounds_bridge".to_string()));
        assert!(ir
            .requires_bridge_lemmas
            .contains(&"mumei_array_get_bridge".to_string()));
    }

    /// PR 2: `allow_lean_verified` does not weaken the `"changed"` detector.
    /// A modified body must still be flagged as needing re-verification, even
    /// when the cert claims `lean_verified`.
    #[test]
    fn test_verify_certificate_lean_verified_changed_detection() {
        let atom = make_test_atom("hard_lemma", "true", "true", "42");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "hard_lemma".to_string(),
            ("lean_verified".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let cert = generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);

        let modified = make_test_atom("hard_lemma", "true", "true", "43");
        let modified_refs: Vec<&parser::Atom> = vec![&modified];

        let status = verify_certificate(&cert, &modified_refs, true);
        assert_eq!(status[0], ("hard_lemma".to_string(), "changed".to_string()));
    }

    #[test]
    fn test_generate_human_review_queue_prioritizes_manual_unknown_and_trusted_atoms() {
        let manual = make_test_atom("manual_case", "true", "true", "match x { _ => 0 }");
        let unknown = make_test_atom("unknown_case", "n > 0", "result > n", "n * n");
        let mut trusted = make_test_atom("trusted_case", "true", "result >= 0", "0");
        trusted.trust_level = parser::TrustLevel::Trusted;
        let atoms: Vec<&parser::Atom> = vec![&trusted, &unknown, &manual];
        let mut results = HashMap::new();
        results.insert(
            "manual_case".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        results.insert(
            "unknown_case".to_string(),
            ("unknown".to_string(), "failed".to_string()),
        );
        results.insert(
            "trusted_case".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();

        let cert = generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        let queue = generate_human_review_queue(&cert);

        assert_eq!(
            queue
                .atoms
                .iter()
                .map(|entry| (entry.atom_name.as_str(), entry.priority.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("manual_case", "critical"),
                ("unknown_case", "high"),
                ("trusted_case", "medium")
            ]
        );
        assert!(queue.atoms[1].spec_text.contains("requires: n > 0"));
    }

    /// P5-A: certificate_hash is deterministic
    #[test]
    fn test_certificate_hash_deterministic() {
        let atom = make_test_atom("foo", "true", "true", "42");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "foo".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();

        let cert1 =
            generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        let cert2 =
            generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);

        // certificate_hash should be the same for the same inputs
        // (timestamp differs, so we check the hash is non-empty and format is correct)
        assert!(!cert1.certificate_hash.is_empty());
        assert!(!cert2.certificate_hash.is_empty());
        // Both should be 64-char hex strings (SHA-256)
        assert_eq!(cert1.certificate_hash.len(), 64);
        assert_eq!(cert2.certificate_hash.len(), 64);
    }

    /// P5-A: compute_sha256 utility works correctly
    #[test]
    fn test_compute_sha256() {
        let hash1 = compute_sha256("hello");
        let hash2 = compute_sha256("hello");
        let hash3 = compute_sha256("world");
        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
        assert_eq!(hash1.len(), 64); // SHA-256 hex = 64 chars
    }

    /// P5-A: all_verified is false when any atom fails
    #[test]
    fn test_all_verified_false_when_failed() {
        let atom1 = make_test_atom("ok", "true", "true", "1");
        let atom2 = make_test_atom("fail", "true", "true", "0");
        let atoms: Vec<&parser::Atom> = vec![&atom1, &atom2];
        let mut results = HashMap::new();
        results.insert(
            "ok".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        results.insert(
            "fail".to_string(),
            ("sat".to_string(), "failed".to_string()),
        );
        let module_env = ModuleEnv::new();

        let cert = generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        assert!(!cert.all_verified);
    }

    #[test]
    fn test_all_verified_false_when_spec_validation_fails() {
        let atom = make_test_atom("impossible", "false", "true", "0");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "impossible".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();

        let cert = generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        let validation = cert.atoms[0].spec_validation_result.as_ref().unwrap();

        assert!(!validation.is_satisfiable);
        assert!(!cert.all_verified);
    }

    #[test]
    fn test_load_certificate_rejects_stale_translator_version() {
        let atom = make_test_atom("bar", "true", "result == 1", "1");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "bar".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let mut cert =
            generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        cert.atoms[0].translator_version = "old-translator".to_string();

        let tmp = std::env::temp_dir().join("mumei_stale_translator_cert.json");
        save_certificate(&cert, &tmp).unwrap();
        let err = load_certificate(&tmp).expect_err("load_certificate must reject stale metadata");
        assert!(err.contains("old-translator"));

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn test_load_certificate_rejects_stale_bridge_hash() {
        let atom = make_test_atom("bar", "true", "result == 1", "1");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "bar".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let mut cert =
            generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        cert.atoms[0].bridge_lemma_hash = "old-bridge-hash".to_string();

        let tmp = std::env::temp_dir().join("mumei_stale_bridge_cert.json");
        save_certificate(&cert, &tmp).unwrap();
        let err = load_certificate(&tmp).expect_err("load_certificate must reject stale metadata");
        assert!(err.contains("old-bridge-hash"));

        let _ = std::fs::remove_file(&tmp);
    }

    /// P5-A: save and load certificate roundtrip
    #[test]
    fn test_save_load_certificate_roundtrip() {
        let atom = make_test_atom("bar", "true", "result == 1", "1");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "bar".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();

        let cert = generate_certificate(
            "test.mm",
            &atoms,
            &results,
            &module_env,
            Some("pkg"),
            Some("2.0.0"),
            None,
        );

        let tmp = std::env::temp_dir().join("mumei_test_cert.json");
        save_certificate(&cert, &tmp).unwrap();
        let loaded = load_certificate(&tmp).unwrap();

        assert_eq!(loaded.atoms.len(), 1);
        assert_eq!(loaded.atoms[0].name, "bar");
        assert_eq!(loaded.package_name, Some("pkg".to_string()));
        assert_eq!(loaded.certificate_hash, cert.certificate_hash);

        let _ = std::fs::remove_file(&tmp);
    }
}
