use crate::reconstruction_loss::ReconstructionLoss;
use crate::verification::{
    self, EscalationReason, LogicFragment, SymbolProvenance, TranslatorIRMetadata,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
    /// Z3 solver check result: "unsat" (proven), "sat" (counter-example found),
    /// "unknown", "skipped", "lean_verified", "timeout", "resource_limit",
    /// or "spurious_candidate"
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
    /// Source body expression used by Lean body-semantics lowering.
    #[serde(default)]
    pub body_expr: String,
    /// Human-readable body summary for bridge diagnostics.
    #[serde(default)]
    pub body_summary: String,
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

    pub(crate) fn rank(self) -> u8 {
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
