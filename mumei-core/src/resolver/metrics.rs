use crate::proof_cert;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LeanEscalationMetrics {
    pub escalation_attempts: usize,
    pub lean_successes: usize,
    #[serde(default)]
    pub lean_verified_accepted: usize,
    pub partial_translation: usize,
    pub manual_required: usize,
    #[serde(default)]
    pub by_atom: HashMap<String, String>,
    #[serde(default)]
    pub by_logic_fragment: HashMap<String, usize>,
    #[serde(default)]
    pub by_failure_reason: HashMap<String, usize>,
    #[serde(default)]
    pub successes_by_failure_reason: HashMap<String, usize>,
}

impl LeanEscalationMetrics {
    pub fn record_candidate(&mut self, candidate: &proof_cert::EscalationCandidate) {
        self.record_escalation(
            &candidate.name,
            candidate.escalation_reason.as_str(),
            &candidate.logic_fragment_tags,
        );
        let lean_metadata = candidate
            .lean_result_metadata
            .as_ref()
            .or(candidate.lean_metadata.as_ref());
        match lean_metadata.map(|metadata| metadata.status.as_str()) {
            Some("lean_verified") => {
                self.lean_successes += 1;
                *self
                    .successes_by_failure_reason
                    .entry(candidate.escalation_reason.as_str().to_string())
                    .or_insert(0) += 1;
            }
            Some("partial_translation") => self.partial_translation += 1,
            _ if candidate.manual_lemma_reason.is_some() => self.manual_required += 1,
            _ => {}
        }
    }

    pub fn record_atom_certificate(&mut self, atom: &proof_cert::AtomCertificate) {
        if let Some(reason) = &atom.escalation_reason {
            self.record_escalation(&atom.name, reason.as_str(), &atom.logic_fragment_tags);
        }
    }

    pub fn record_lean_verified_acceptance(&mut self, atom: &proof_cert::AtomCertificate) {
        let reason = atom
            .escalation_reason
            .map(|reason| reason.as_str())
            .unwrap_or("lean_verified_import_acceptance");
        if !self.by_atom.contains_key(&atom.name) {
            self.record_escalation(&atom.name, reason, &atom.logic_fragment_tags);
        }
        self.lean_successes += 1;
        self.lean_verified_accepted += 1;
        *self
            .successes_by_failure_reason
            .entry(reason.to_string())
            .or_insert(0) += 1;
    }

    fn record_escalation(&mut self, name: &str, reason: &str, logic_fragment_tags: &[String]) {
        self.escalation_attempts += 1;
        self.by_atom.insert(name.to_string(), reason.to_string());
        *self
            .by_failure_reason
            .entry(reason.to_string())
            .or_insert(0) += 1;
        for tag in logic_fragment_tags {
            *self.by_logic_fragment.entry(tag.clone()).or_insert(0) += 1;
        }
    }

    pub fn identify_low_success_categories(&self) -> Vec<String> {
        let mut low_success = Vec::new();
        for (reason, attempts) in &self.by_failure_reason {
            let successes = self
                .successes_by_failure_reason
                .get(reason)
                .copied()
                .unwrap_or(0);
            if *attempts > 0 && successes as f64 / (*attempts as f64) < 0.5 {
                low_success.push(reason.clone());
            }
        }
        low_success.sort();
        low_success
    }

    pub fn to_summary_json(&self) -> serde_json::Value {
        serde_json::json!({
            "escalation_attempts": self.escalation_attempts,
            "lean_successes": self.lean_successes,
            "lean_verified_accepted": self.lean_verified_accepted,
            "partial_translation": self.partial_translation,
            "manual_required": self.manual_required,
            "success_rate": if self.escalation_attempts > 0 {
                self.lean_successes as f64 / self.escalation_attempts as f64
            } else {
                0.0
            },
            "by_failure_reason": self.by_failure_reason,
            "successes_by_failure_reason": self.successes_by_failure_reason,
            "by_logic_fragment": self.by_logic_fragment,
            "low_success_categories": self.identify_low_success_categories(),
        })
    }

    pub(crate) fn merge(&mut self, other: Self) {
        self.escalation_attempts += other.escalation_attempts;
        self.lean_successes += other.lean_successes;
        self.lean_verified_accepted += other.lean_verified_accepted;
        self.partial_translation += other.partial_translation;
        self.manual_required += other.manual_required;
        self.by_atom.extend(other.by_atom);
        for (tag, count) in other.by_logic_fragment {
            *self.by_logic_fragment.entry(tag).or_insert(0) += count;
        }
        for (reason, count) in other.by_failure_reason {
            *self.by_failure_reason.entry(reason).or_insert(0) += count;
        }
        for (reason, count) in other.successes_by_failure_reason {
            *self.successes_by_failure_reason.entry(reason).or_insert(0) += count;
        }
    }

    pub(crate) fn has_activity(&self) -> bool {
        self.escalation_attempts > 0
            || self.lean_successes > 0
            || self.partial_translation > 0
            || self.manual_required > 0
    }
}

/// Build and write an `EscalationBundle` from a `ProofCertificate` using the
/// same candidate filtering as `proof_cert::generate_escalation_bundle`.
pub fn emit_escalation_bundle(
    cert: &proof_cert::ProofCertificate,
    output_path: &std::path::Path,
) -> Result<(), String> {
    let bundle = proof_cert::generate_escalation_bundle(cert);
    proof_cert::save_escalation_bundle(&bundle, output_path)
}
