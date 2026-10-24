use crate::parser::TrustLevel;
use crate::proof_cert::{self, status, AtomCertificate, ProofCertificate};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const SCHEMA: &str = "mumei.claimbook/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SealGrade {
    #[serde(rename = "G0")]
    G0,
    #[serde(rename = "G1")]
    G1,
    #[serde(rename = "G2")]
    G2,
    #[serde(rename = "G3")]
    G3,
    #[serde(rename = "G4")]
    G4,
    #[serde(rename = "G5")]
    G5,
}

impl SealGrade {
    pub fn label(self) -> &'static str {
        match self {
            Self::G0 => "open",
            Self::G1 => "vouched",
            Self::G2 => "observed",
            Self::G3 => "range-sealed",
            Self::G4 => "machine-sealed",
            Self::G5 => "dual-sealed",
        }
    }

    fn all() -> [Self; 6] {
        [Self::G0, Self::G1, Self::G2, Self::G3, Self::G4, Self::G5]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    Fresh,
    Drifted,
    Uncertified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CertificateIntegrity {
    Match,
    Mismatch,
    IncomparableVersion,
    Absent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Claim {
    pub atom: String,
    pub grade: SealGrade,
    pub grade_label: String,
    pub effective_grade: SealGrade,
    pub effective_grade_label: String,
    pub evidence_kind: String,
    pub freshness: Freshness,
    pub reason: String,
    pub requires: String,
    pub ensures: String,
    pub assumed_clauses: Vec<String>,
    pub dependencies: Vec<String>,
    pub content_hash: Option<String>,
    pub proof_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrphanedEntry {
    pub atom: String,
    pub content_hash: String,
    pub proof_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaimBookSummary {
    pub by_effective_grade: BTreeMap<SealGrade, usize>,
    pub by_freshness: BTreeMap<Freshness, usize>,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaimBookGenerator {
    pub mumei_version: Option<String>,
    pub z3_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaimBook {
    pub schema: String,
    pub input: String,
    pub certificate: String,
    pub certificate_integrity: CertificateIntegrity,
    pub generator: ClaimBookGenerator,
    pub summary: ClaimBookSummary,
    pub claims: Vec<Claim>,
    pub orphaned_entries: Vec<OrphanedEntry>,
}

pub fn own_grade(
    atom: &AtomCertificate,
    source_trust_level: &TrustLevel,
    allow_lean_verified: bool,
) -> (SealGrade, String, String) {
    let spec_is_satisfiable = atom
        .spec_validation_result
        .as_ref()
        .is_none_or(|result| result.is_satisfiable);
    if !spec_is_satisfiable {
        return (
            SealGrade::G0,
            atom.z3_check_result.clone(),
            "the specification is unsatisfiable".to_string(),
        );
    }

    match source_trust_level {
        TrustLevel::Trusted => {
            return (
                SealGrade::G1,
                "trusted".to_string(),
                "the contract is marked trusted; its body is not checked".to_string(),
            );
        }
        TrustLevel::Unverified => {
            return (
                SealGrade::G0,
                atom.z3_check_result.clone(),
                "the atom is marked unverified".to_string(),
            );
        }
        TrustLevel::Verified => {}
    }

    if atom.status == status::TRUSTED {
        return (
            SealGrade::G1,
            "trusted".to_string(),
            "the contract is marked trusted".to_string(),
        );
    }

    if atom.status == status::VERIFIED && atom.z3_check_result == status::Z3_UNSAT {
        return (
            SealGrade::G4,
            "z3".to_string(),
            "Z3 discharged the obligation".to_string(),
        );
    }

    if atom.status == status::VERIFIED
        && atom.z3_check_result == status::Z3_LEAN_VERIFIED
        && allow_lean_verified
        && proof_cert::lean_certificate_metadata_is_current(atom)
        && matches!(
            proof_cert::lean_axiom_audit(atom),
            proof_cert::LeanAxiomAudit::Passed { .. }
        )
    {
        return (
            SealGrade::G4,
            "lean".to_string(),
            "Lean metadata is current and its axiom audit passed".to_string(),
        );
    }

    let reason = if atom.z3_check_result == status::Z3_LEAN_VERIFIED {
        if !allow_lean_verified {
            "Lean evidence requires --allow-lean-verified"
        } else if !proof_cert::lean_certificate_metadata_is_current(atom) {
            "Lean metadata is stale or incomplete"
        } else {
            "Lean axiom audit did not pass"
        }
    } else if atom.status == status::VERIFIED {
        "the verification result did not establish the contract"
    } else {
        "the certificate status did not establish the contract"
    };
    (
        SealGrade::G0,
        atom.z3_check_result.clone(),
        reason.to_string(),
    )
}

pub fn freshness_from_verification_status(value: &str) -> Freshness {
    match value {
        status::CERTIFICATE_CHANGED | status::CERTIFICATE_STALE_TRANSLATOR => Freshness::Drifted,
        status::CERTIFICATE_PROVEN
        | status::CERTIFICATE_UNPROVEN
        | status::CERTIFICATE_AXIOM_REJECTED => Freshness::Fresh,
        _ => Freshness::Uncertified,
    }
}

pub fn certificate_integrity(cert: Option<&ProofCertificate>) -> CertificateIntegrity {
    match cert.map(proof_cert::check_certificate_hash) {
        None | Some(proof_cert::CertificateHashCheck::Absent) => CertificateIntegrity::Absent,
        Some(proof_cert::CertificateHashCheck::Match) => CertificateIntegrity::Match,
        Some(proof_cert::CertificateHashCheck::Mismatch {
            comparable: true, ..
        }) => CertificateIntegrity::Mismatch,
        Some(proof_cert::CertificateHashCheck::Mismatch {
            comparable: false, ..
        }) => CertificateIntegrity::IncomparableVersion,
    }
}

pub fn compute_effective_grades(claims: &BTreeMap<String, Claim>) -> BTreeMap<String, SealGrade> {
    claims
        .iter()
        .map(|(name, claim)| {
            let grade = if claim.freshness == Freshness::Fresh {
                claim.grade
            } else {
                SealGrade::G0
            };
            (name.clone(), grade)
        })
        .collect()
}

pub fn create_claimbook(
    input: impl Into<String>,
    certificate_path: impl Into<String>,
    cert: Option<&ProofCertificate>,
    atoms: &[&crate::parser::Atom],
    allow_lean_verified: bool,
) -> ClaimBook {
    let input = input.into();
    let certificate_path = certificate_path.into();
    let integrity = certificate_integrity(cert);
    let verification_results: BTreeMap<String, String> = cert
        .map(|cert| {
            proof_cert::verify_certificate(cert, atoms, allow_lean_verified)
                .into_iter()
                .collect()
        })
        .unwrap_or_default();
    let certified: BTreeMap<&str, &AtomCertificate> = cert
        .into_iter()
        .flat_map(|cert| cert.atoms.iter())
        .map(|atom| (atom.name.as_str(), atom))
        .collect();
    let source_atoms: BTreeMap<&str, &crate::parser::Atom> = atoms
        .iter()
        .map(|atom| (atom.name.as_str(), *atom))
        .collect();

    let mut claims = BTreeMap::new();
    for (name, source_atom) in &source_atoms {
        let entry = certified.get(name).copied();
        let verification_result = entry
            .and_then(|_| verification_results.get(*name))
            .map(String::as_str);
        let stale_lean_metadata = entry.is_some_and(|entry| {
            entry.z3_check_result == status::Z3_LEAN_VERIFIED
                && !proof_cert::lean_certificate_metadata_is_current(entry)
        });
        let (freshness, freshness_reason) = if integrity == CertificateIntegrity::Mismatch {
            (
                Freshness::Drifted,
                Some("certificate integrity hash does not match"),
            )
        } else if verification_result == Some(status::CERTIFICATE_CHANGED) {
            (
                Freshness::Drifted,
                Some("source content differs from its certificate"),
            )
        } else if stale_lean_metadata
            || verification_result == Some(status::CERTIFICATE_STALE_TRANSLATOR)
        {
            (
                Freshness::Drifted,
                Some("Lean translator metadata is stale; regenerate the Lean certificate"),
            )
        } else {
            match verification_result
                .map(freshness_from_verification_status)
                .unwrap_or(Freshness::Uncertified)
            {
                Freshness::Fresh => (Freshness::Fresh, None),
                Freshness::Drifted => (
                    Freshness::Drifted,
                    Some("source content differs from its certificate"),
                ),
                Freshness::Uncertified => (
                    Freshness::Uncertified,
                    Some("no current certificate entry exists"),
                ),
            }
        };

        let (grade, evidence_kind, own_reason) = entry
            .map(|entry| own_grade(entry, &source_atom.trust_level, allow_lean_verified))
            .unwrap_or((
                SealGrade::G0,
                "none".to_string(),
                "no certificate entry exists for this atom".to_string(),
            ));
        let reason = match freshness_reason {
            Some(reason)
                if reason
                    == "Lean translator metadata is stale; regenerate the Lean certificate" =>
            {
                reason.to_string()
            }
            Some(reason) if entry.is_some() => format!("{reason}; {own_reason}"),
            Some(reason) => reason.to_string(),
            None => own_reason,
        };

        let claim = Claim {
            atom: (*name).to_string(),
            grade,
            grade_label: grade.label().to_string(),
            effective_grade: SealGrade::G0,
            effective_grade_label: SealGrade::G0.label().to_string(),
            evidence_kind,
            freshness,
            reason,
            requires: source_atom.requires.clone(),
            ensures: source_atom.ensures.clone(),
            assumed_clauses: entry
                .map(|entry| entry.assumed_clauses.clone())
                .unwrap_or_default(),
            dependencies: entry
                .map(|entry| entry.dependencies.clone())
                .unwrap_or_default(),
            content_hash: entry.map(|entry| entry.content_hash.clone()),
            proof_hash: entry.map(|entry| entry.proof_hash.clone()),
        };
        claims.insert((*name).to_string(), claim);
    }

    let effective_grades = compute_effective_grades(&claims);
    for (name, grade) in effective_grades {
        if let Some(claim) = claims.get_mut(&name) {
            claim.effective_grade = grade;
            claim.effective_grade_label = grade.label().to_string();
        }
    }

    let mut by_effective_grade = SealGrade::all()
        .into_iter()
        .map(|grade| (grade, 0))
        .collect::<BTreeMap<_, _>>();
    let mut by_freshness = [Freshness::Fresh, Freshness::Drifted, Freshness::Uncertified]
        .into_iter()
        .map(|freshness| (freshness, 0))
        .collect::<BTreeMap<_, _>>();
    for claim in claims.values() {
        *by_effective_grade
            .get_mut(&claim.effective_grade)
            .expect("all grades are initialized") += 1;
        *by_freshness
            .get_mut(&claim.freshness)
            .expect("all freshness values are initialized") += 1;
    }

    let mut orphaned_entries = cert
        .into_iter()
        .flat_map(|cert| cert.atoms.iter())
        .filter(|entry| !source_atoms.contains_key(entry.name.as_str()))
        .map(|entry| OrphanedEntry {
            atom: entry.name.clone(),
            content_hash: entry.content_hash.clone(),
            proof_hash: entry.proof_hash.clone(),
        })
        .collect::<Vec<_>>();
    orphaned_entries.sort_by(|left, right| left.atom.cmp(&right.atom));

    ClaimBook {
        schema: SCHEMA.to_string(),
        input,
        certificate: certificate_path,
        certificate_integrity: integrity,
        generator: ClaimBookGenerator {
            mumei_version: cert.map(|cert| cert.mumei_version.clone()),
            z3_version: cert.map(|cert| cert.z3_version.clone()),
        },
        summary: ClaimBookSummary {
            by_effective_grade,
            by_freshness,
            total: claims.len(),
        },
        claims: claims.into_values().collect(),
        orphaned_entries,
    }
}

pub fn render_markdown(book: &ClaimBook) -> String {
    let mut markdown = format!("# Claim Book\n\nInput: `{}`\n\n", book.input);
    markdown.push_str("## Summary\n\n| Effective grade | Count |\n|---|---:|\n");
    for grade in SealGrade::all() {
        let count = book
            .summary
            .by_effective_grade
            .get(&grade)
            .copied()
            .unwrap_or(0);
        markdown.push_str(&format!(
            "| {} ({}) | {} |\n",
            grade.label(),
            grade_code(grade),
            count
        ));
    }
    markdown.push_str("\n| Freshness | Count |\n|---|---:|\n");
    for freshness in [Freshness::Fresh, Freshness::Drifted, Freshness::Uncertified] {
        let count = book
            .summary
            .by_freshness
            .get(&freshness)
            .copied()
            .unwrap_or(0);
        markdown.push_str(&format!("| {} | {} |\n", freshness_name(freshness), count));
    }

    markdown.push_str("\n## Drifted or uncertified\n\n");
    append_claim_table(
        &mut markdown,
        book.claims
            .iter()
            .filter(|claim| claim.freshness != Freshness::Fresh),
    );

    for grade in SealGrade::all().into_iter().rev() {
        markdown.push_str(&format!(
            "\n## {} ({})\n\n",
            grade.label(),
            grade_code(grade)
        ));
        append_claim_table(
            &mut markdown,
            book.claims
                .iter()
                .filter(|claim| claim.effective_grade == grade),
        );
    }

    if !book.orphaned_entries.is_empty() {
        markdown.push_str("\n## Orphaned certificate entries\n\n| Atom |\n|---|\n");
        for entry in &book.orphaned_entries {
            markdown.push_str(&format!("| `{}` |\n", entry.atom));
        }
    }
    markdown
}

fn append_claim_table<'a>(markdown: &mut String, claims: impl Iterator<Item = &'a Claim>) {
    markdown.push_str("| Atom | Own grade | Freshness | Evidence | Reason |\n");
    markdown.push_str("|---|---|---|---|---|\n");
    let mut count = 0;
    for claim in claims {
        count += 1;
        markdown.push_str(&format!(
            "| `{}` | {} ({}) | {} | {} | {} |\n",
            claim.atom,
            claim.grade_label,
            grade_code(claim.grade),
            freshness_name(claim.freshness),
            claim.evidence_kind,
            claim.reason.replace('|', "\\|")
        ));
    }
    if count == 0 {
        markdown.push_str("| — | — | — | — | — |\n");
    }
}

fn grade_code(grade: SealGrade) -> &'static str {
    match grade {
        SealGrade::G0 => "G0",
        SealGrade::G1 => "G1",
        SealGrade::G2 => "G2",
        SealGrade::G3 => "G3",
        SealGrade::G4 => "G4",
        SealGrade::G5 => "G5",
    }
}

fn freshness_name(freshness: Freshness) -> &'static str {
    match freshness {
        Freshness::Fresh => "fresh",
        Freshness::Drifted => "drifted",
        Freshness::Uncertified => "uncertified",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verification;
    use serde_json::{json, Value};

    fn atom_certificate(z3_result: &str, verification_status: &str) -> AtomCertificate {
        serde_json::from_value(json!({
            "name": "sample",
            "z3_check_result": z3_result,
            "content_hash": "content",
            "status": verification_status
        }))
        .expect("construct atom certificate")
    }

    fn lean_atom(audit: Option<&str>, axioms: Option<Vec<&str>>) -> AtomCertificate {
        let mut atom = atom_certificate(status::Z3_LEAN_VERIFIED, status::VERIFIED);
        atom.translator_version = verification::LEAN_TRANSLATOR_VERSION.to_string();
        atom.bridge_lemma_hash = verification::LEAN_BRIDGE_LEMMA_HASH.to_string();
        atom.lean_result_metadata = Some(crate::proof_cert::LeanResultMetadata {
            status: status::LEAN_STATUS_VERIFIED.to_string(),
            theorem_name: "sample_spec".to_string(),
            translator_version: verification::LEAN_TRANSLATOR_VERSION.to_string(),
            bridge_lemma_hash: verification::LEAN_BRIDGE_LEMMA_HASH.to_string(),
            kernel_axioms: axioms.map(|values| values.into_iter().map(str::to_string).collect()),
            axiom_audit: audit.map(str::to_string),
            ..Default::default()
        });
        atom
    }

    fn set_spec_satisfiable(atom: &mut AtomCertificate, is_satisfiable: bool) {
        atom.spec_validation_result = Some(
            serde_json::from_value(json!({
                "status": if is_satisfiable { "satisfiable" } else { "unsatisfiable" },
                "is_satisfiable": is_satisfiable,
                "trace_id": null,
                "spec_metadata": {},
                "traceability_hash": "",
                "traceability_coverage": 0.0,
                "checked_requires": true,
                "checked_ensures": 0,
                "checked_refinements": 0,
                "ensures_implication_checks": 0,
                "diagnostics": []
            }))
            .expect("construct spec validation"),
        );
    }

    fn proof_certificate() -> ProofCertificate {
        serde_json::from_value(json!({
            "version": "1.1",
            "timestamp": "2026-01-01T00:00:00Z",
            "mumei_version": env!("CARGO_PKG_VERSION"),
            "z3_version": "4.14.1",
            "file": "sample.mm",
            "atoms": [],
            "certificate_hash": ""
        }))
        .expect("construct proof certificate")
    }

    fn stale_lean_certificate(source: &crate::parser::Atom) -> ProofCertificate {
        let results = std::collections::HashMap::from([(
            source.name.clone(),
            (
                status::Z3_LEAN_VERIFIED.to_string(),
                status::VERIFIED.to_string(),
            ),
        )]);
        let mut cert = proof_cert::generate_certificate(
            "sample.mm",
            &[source],
            &results,
            &verification::ModuleEnv::new(),
            None,
            None,
            None,
        );
        let current_metadata = lean_atom(Some("passed"), Some(vec!["propext"]));
        cert.atoms[0].translator_version = "old-translator".to_string();
        cert.atoms[0].bridge_lemma_hash = verification::LEAN_BRIDGE_LEMMA_HASH.to_string();
        cert.atoms[0].lean_result_metadata = current_metadata.lean_result_metadata;
        proof_cert::refresh_certificate_integrity(&mut cert);
        cert
    }

    fn source_atom(name: &str) -> crate::parser::Atom {
        crate::parser::Atom {
            name: name.to_string(),
            type_params: vec![],
            where_bounds: vec![],
            params: vec![],
            trace_id: None,
            spec_metadata: std::collections::HashMap::new(),
            clause_labels: vec![],
            clause_modes: vec![],
            covers: vec![],
            requires: "true".to_string(),
            forall_constraints: vec![],
            ensures: "true".to_string(),
            body_expr: "0".to_string(),
            consumed_params: vec![],
            resources: vec![],
            is_async: false,
            trust_level: crate::parser::TrustLevel::Verified,
            max_unroll: None,
            invariant: None,
            effects: vec![],
            return_type: None,
            decreases: None,
            span: crate::parser::Span::new("sample.mm", 0, 0, 0),
            effect_pre: std::collections::HashMap::new(),
            effect_post: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn every_z3_result_and_status_has_the_expected_own_grade() {
        for z3_result in status::Z3_CHECK_RESULTS {
            for verification_status in status::VERIFICATION_STATUSES {
                let atom = atom_certificate(z3_result, verification_status);
                let expected = if verification_status == status::TRUSTED {
                    SealGrade::G1
                } else if verification_status == status::VERIFIED && z3_result == status::Z3_UNSAT {
                    SealGrade::G4
                } else {
                    SealGrade::G0
                };
                assert_eq!(
                    own_grade(&atom, &TrustLevel::Verified, false).0,
                    expected,
                    "{z3_result} × {verification_status}"
                );
            }
        }
    }

    #[test]
    fn lean_evidence_requires_opt_in_current_metadata_and_a_passing_audit() {
        let current = lean_atom(Some("passed"), Some(vec!["propext"]));
        assert_eq!(
            own_grade(&current, &TrustLevel::Verified, false).0,
            SealGrade::G0
        );
        assert_eq!(
            own_grade(&current, &TrustLevel::Verified, true).0,
            SealGrade::G4
        );
        assert_eq!(own_grade(&current, &TrustLevel::Verified, true).1, "lean");

        let mut stale = current.clone();
        stale.translator_version = "old-translator".to_string();
        assert_eq!(
            own_grade(&stale, &TrustLevel::Verified, true).0,
            SealGrade::G0
        );

        for (audit, axioms) in [
            (Some("rejected"), Some(vec!["propext"])),
            (Some("error"), Some(vec!["propext"])),
            (None, None),
        ] {
            assert_eq!(
                own_grade(&lean_atom(audit, axioms), &TrustLevel::Verified, true).0,
                SealGrade::G0
            );
        }
    }

    #[test]
    fn an_unsatisfiable_specification_is_open_even_if_other_evidence_is_present() {
        let mut atom = atom_certificate(status::Z3_UNSAT, status::VERIFIED);
        set_spec_satisfiable(&mut atom, false);
        assert_eq!(
            own_grade(&atom, &TrustLevel::Verified, false).0,
            SealGrade::G0
        );
        assert_eq!(
            own_grade(&atom, &TrustLevel::Verified, false).2,
            "the specification is unsatisfiable"
        );
    }

    #[test]
    fn satisfiable_specification_does_not_block_machine_grade() {
        let mut atom = atom_certificate(status::Z3_UNSAT, status::VERIFIED);
        set_spec_satisfiable(&mut atom, true);
        assert_eq!(
            own_grade(&atom, &TrustLevel::Verified, false).0,
            SealGrade::G4
        );
    }

    #[test]
    fn source_trust_level_overrides_solver_result_for_trusted_and_unverified_atoms() {
        let trusted_unsat = atom_certificate(status::Z3_UNSAT, status::VERIFIED);
        let trusted_grade = own_grade(&trusted_unsat, &TrustLevel::Trusted, false);
        assert_eq!(trusted_grade.0, SealGrade::G1);
        assert_eq!(trusted_grade.1, "trusted");
        assert_eq!(
            trusted_grade.2,
            "the contract is marked trusted; its body is not checked"
        );

        let trusted_sat = atom_certificate(status::Z3_SAT, status::VERIFIED);
        assert_eq!(
            own_grade(&trusted_sat, &TrustLevel::Trusted, false).0,
            SealGrade::G1
        );

        let unverified_unsat = atom_certificate(status::Z3_UNSAT, status::VERIFIED);
        let unverified_grade = own_grade(&unverified_unsat, &TrustLevel::Unverified, false);
        assert_eq!(unverified_grade.0, SealGrade::G0);
        assert_eq!(unverified_grade.1, status::Z3_UNSAT);
        assert_eq!(unverified_grade.2, "the atom is marked unverified");
    }

    #[test]
    fn freshness_maps_every_certificate_verification_result() {
        for (value, expected) in [
            (status::CERTIFICATE_CHANGED, Freshness::Drifted),
            (status::CERTIFICATE_STALE_TRANSLATOR, Freshness::Drifted),
            (status::CERTIFICATE_PROVEN, Freshness::Fresh),
            (status::CERTIFICATE_UNPROVEN, Freshness::Fresh),
            (status::CERTIFICATE_AXIOM_REJECTED, Freshness::Fresh),
            (status::CERTIFICATE_MISSING, Freshness::Uncertified),
        ] {
            assert_eq!(freshness_from_verification_status(value), expected);
        }
    }

    #[test]
    fn stale_lean_metadata_drifts_without_opt_in_and_keeps_its_cause() {
        let source = source_atom("sample");
        let cert = stale_lean_certificate(&source);
        let atoms = [&source];

        let without_opt_in =
            create_claimbook("sample.mm", "sample.proof.json", Some(&cert), &atoms, false);
        let without_claim = &without_opt_in.claims[0];
        assert_eq!(without_claim.freshness, Freshness::Drifted);
        assert_eq!(
            without_claim.reason,
            "Lean translator metadata is stale; regenerate the Lean certificate"
        );

        let with_opt_in =
            create_claimbook("sample.mm", "sample.proof.json", Some(&cert), &atoms, true);
        let with_claim = &with_opt_in.claims[0];
        assert_eq!(with_claim.freshness, Freshness::Drifted);
        assert_eq!(with_claim.reason, without_claim.reason);

        let mut changed_source = source.clone();
        changed_source.body_expr.push_str(" + 1");
        let changed_atoms = [&changed_source];
        let changed = create_claimbook(
            "sample.mm",
            "sample.proof.json",
            Some(&cert),
            &changed_atoms,
            false,
        );
        assert_eq!(changed.claims[0].freshness, Freshness::Drifted);
        assert!(changed.claims[0]
            .reason
            .starts_with("source content differs from its certificate"));
    }

    #[test]
    fn certificate_integrity_distinguishes_match_absence_and_mismatch_versions() {
        let mut cert = proof_certificate();
        assert_eq!(
            certificate_integrity(Some(&cert)),
            CertificateIntegrity::Absent
        );

        proof_cert::refresh_certificate_integrity(&mut cert);
        assert_eq!(
            certificate_integrity(Some(&cert)),
            CertificateIntegrity::Match
        );
        cert.z3_version.push_str("-edited");
        assert_eq!(
            certificate_integrity(Some(&cert)),
            CertificateIntegrity::Mismatch
        );

        cert.mumei_version = "another-version".to_string();
        assert_eq!(
            certificate_integrity(Some(&cert)),
            CertificateIntegrity::IncomparableVersion
        );
        assert_eq!(certificate_integrity(None), CertificateIntegrity::Absent);
    }

    #[test]
    fn json_serialization_is_deterministic_and_claims_are_sorted() {
        let first = source_atom("zeta");
        let second = source_atom("alpha");
        let atoms = [&first, &second];
        let book = create_claimbook("sample.mm", "sample.proof.json", None, &atoms, false);
        let first_json = serde_json::to_string_pretty(&book).expect("serialize claimbook");
        let second_json = serde_json::to_string_pretty(&book).expect("serialize claimbook again");
        assert_eq!(first_json, second_json);
        assert_eq!(book.claims[0].atom, "alpha");
        assert_eq!(book.claims[1].atom, "zeta");
        let value: Value = serde_json::from_str(&first_json).expect("parse claimbook JSON");
        assert_eq!(value["schema"], SCHEMA);
        assert_eq!(value["claims"][0]["grade"], "G0");
        assert_eq!(value["claims"][0]["grade_label"], "open");
    }
}
