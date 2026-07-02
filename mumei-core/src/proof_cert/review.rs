use super::models::{
    AtomCertificate, EscalationBundle, EscalationBundleSummary, EscalationCandidate,
    HumanReviewEntry, HumanReviewPriority, HumanReviewQueue, ProofCertificate,
};
use super::status;
use crate::verification::EscalationReason;
use std::collections::HashSet;

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

    if atom.z3_result_class == status::Z3_UNKNOWN || atom.z3_check_result == status::Z3_UNKNOWN {
        return Some(HumanReviewEntry {
            atom_name: atom.name.clone(),
            review_reason: "z3_unknown".to_string(),
            priority: HumanReviewPriority::High,
            spec_text: atom_spec_text(atom),
            suggested_action: "Escalate this atom with --escalate-lean or simplify the specification into a Z3-decidable fragment.".to_string(),
        });
    }

    if atom.status == status::ESCALATION_CANDIDATE {
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

    if atom.status == status::TRUSTED
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
