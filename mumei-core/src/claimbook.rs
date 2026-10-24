use crate::parser::{ClauseTrustMode, TrustLevel};
use crate::proof_cert::{self, status, AtomCertificate, ProofCertificate};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

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
    pub effective_grade_complete: bool,
    pub evidence_kind: String,
    pub freshness: Freshness,
    pub reason: String,
    pub requires: String,
    pub ensures: String,
    pub assumed_clauses: Vec<String>,
    pub dependencies: Vec<String>,
    pub unresolved_dependencies: Vec<String>,
    pub limiting_premises: Vec<LimitingPremise>,
    pub content_hash: Option<String>,
    pub proof_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct LimitingPremise {
    pub kind: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_from: Option<String>,
    pub grade: SealGrade,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct TrustedBaseAssumedClause {
    pub atom: String,
    pub clause: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrustedBase {
    pub solver_version: Option<String>,
    pub mumei_version: Option<String>,
    pub translator_versions: Vec<String>,
    pub bridge_lemma_hashes: Vec<String>,
    pub kernel_axioms: Vec<String>,
    pub trusted_atoms: Vec<String>,
    pub assumed_clauses: Vec<TrustedBaseAssumedClause>,
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
    pub trusted_base: TrustedBase,
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
        if !proof_cert::lean_translator_metadata_is_current(atom) {
            "Lean metadata is stale or incomplete"
        } else if matches!(
            proof_cert::lean_axiom_audit(atom),
            proof_cert::LeanAxiomAudit::Rejected { .. } | proof_cert::LeanAxiomAudit::Error
        ) {
            "Lean axiom audit did not pass"
        } else if !allow_lean_verified {
            "Lean evidence requires --allow-lean-verified"
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
        | status::CERTIFICATE_TRUSTED
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
    let mut effective_grades = claims
        .iter()
        .map(|(name, claim)| {
            let fresh_grade = if claim.freshness == Freshness::Fresh {
                claim.grade
            } else {
                SealGrade::G0
            };
            let base_grade = if claim.assumed_clauses.is_empty() {
                fresh_grade
            } else {
                fresh_grade.min(SealGrade::G1)
            };
            (name.clone(), base_grade)
        })
        .collect::<BTreeMap<_, _>>();

    loop {
        let next_grades = claims
            .iter()
            .map(|(name, claim)| {
                let grade = claim
                    .dependencies
                    .iter()
                    .filter_map(|dependency| effective_grades.get(dependency))
                    .copied()
                    .fold(effective_grades[name], SealGrade::min);
                (name.clone(), grade)
            })
            .collect::<BTreeMap<_, _>>();
        if next_grades == effective_grades {
            break;
        }
        effective_grades = next_grades;
    }

    effective_grades
}

fn apply_effective_grades(
    claims: &mut BTreeMap<String, Claim>,
    method_name_matches: &BTreeMap<String, BTreeMap<String, String>>,
) {
    let effective_grades = compute_effective_grades(claims);
    let claim_names = claims.keys().cloned().collect::<BTreeSet<_>>();
    let mut completeness = claims
        .iter()
        .map(|(name, claim)| {
            (
                name.clone(),
                claim
                    .dependencies
                    .iter()
                    .all(|dependency| claim_names.contains(dependency))
                    && !method_name_matches.contains_key(name),
            )
        })
        .collect::<BTreeMap<_, _>>();
    loop {
        let next_completeness = claims
            .iter()
            .map(|(name, claim)| {
                let complete = completeness[name]
                    && claim
                        .dependencies
                        .iter()
                        .all(|dependency| completeness.get(dependency).copied().unwrap_or(false));
                (name.clone(), complete)
            })
            .collect::<BTreeMap<_, _>>();
        if next_completeness == completeness {
            break;
        }
        completeness = next_completeness;
    }

    for (name, claim) in claims.iter_mut() {
        let effective_grade = effective_grades[name];
        claim.effective_grade = effective_grade;
        claim.effective_grade_label = effective_grade.label().to_string();
        claim.unresolved_dependencies = claim
            .dependencies
            .iter()
            .filter(|dependency| !claim_names.contains(*dependency))
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        claim.effective_grade_complete = completeness[name];
        claim.limiting_premises = limiting_premises(
            claim,
            effective_grade,
            &effective_grades,
            method_name_matches.get(name),
        );
    }
}

fn resolve_claim_dependencies(
    claims: &mut BTreeMap<String, Claim>,
) -> BTreeMap<String, BTreeMap<String, String>> {
    let claim_names = claims.keys().cloned().collect::<BTreeSet<_>>();
    let mut method_name_matches = BTreeMap::new();
    for (claim_name, claim) in claims.iter_mut() {
        let mut claim_method_name_matches = BTreeMap::new();
        claim.dependencies = claim
            .dependencies
            .iter()
            .map(|dependency| {
                if claim_names.contains(dependency) {
                    return dependency.clone();
                }

                let suffix = format!("::{dependency}");
                let mut qualified_matches =
                    claim_names.iter().filter(|name| name.ends_with(&suffix));
                match (qualified_matches.next(), qualified_matches.next()) {
                    (Some(match_name), None) => {
                        claim_method_name_matches.insert(match_name.clone(), dependency.clone());
                        match_name.clone()
                    }
                    _ => dependency.clone(),
                }
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if !claim_method_name_matches.is_empty() {
            method_name_matches.insert(claim_name.clone(), claim_method_name_matches);
        }
    }
    method_name_matches
}

fn limiting_premises(
    claim: &Claim,
    effective_grade: SealGrade,
    effective_grades: &BTreeMap<String, SealGrade>,
    method_name_matches: Option<&BTreeMap<String, String>>,
) -> Vec<LimitingPremise> {
    if effective_grade == claim.grade {
        return Vec::new();
    }

    let mut premises = BTreeSet::new();
    if claim.freshness != Freshness::Fresh && claim.grade > SealGrade::G0 {
        premises.insert(LimitingPremise {
            kind: "freshness".to_string(),
            name: freshness_name(claim.freshness).to_string(),
            matched_from: None,
            grade: SealGrade::G0,
        });
    }

    let fresh_grade = if claim.freshness == Freshness::Fresh {
        claim.grade
    } else {
        SealGrade::G0
    };
    if fresh_grade > SealGrade::G1 {
        for clause in &claim.assumed_clauses {
            premises.insert(LimitingPremise {
                kind: "assumed_clause".to_string(),
                name: assumed_clause_text(clause).to_string(),
                matched_from: None,
                grade: SealGrade::G1,
            });
        }
    }

    for dependency in &claim.dependencies {
        if let Some(dependency_grade) = effective_grades.get(dependency) {
            if *dependency_grade < claim.grade {
                premises.insert(LimitingPremise {
                    kind: "dependency".to_string(),
                    name: dependency.clone(),
                    matched_from: method_name_matches
                        .and_then(|matches| matches.get(dependency))
                        .cloned(),
                    grade: *dependency_grade,
                });
            }
        }
    }

    premises.into_iter().collect()
}

fn assumed_clause_text(clause: &str) -> &str {
    clause
        .strip_prefix("requires: ")
        .or_else(|| clause.strip_prefix("ensures: "))
        .unwrap_or(clause)
}

fn trusted_base(cert: Option<&ProofCertificate>, atoms: &[&crate::parser::Atom]) -> TrustedBase {
    let source_atoms: BTreeMap<&str, &crate::parser::Atom> = atoms
        .iter()
        .map(|atom| (atom.name.as_str(), *atom))
        .collect();

    let mut trusted_atoms = BTreeSet::new();
    let mut assumed_clauses = BTreeSet::new();
    for (name, atom) in &source_atoms {
        if atom.trust_level == TrustLevel::Trusted {
            trusted_atoms.insert((*name).to_string());
        }
        for clause_mode in &atom.clause_modes {
            if clause_mode.mode == ClauseTrustMode::Assume {
                assumed_clauses.insert(TrustedBaseAssumedClause {
                    atom: (*name).to_string(),
                    clause: clause_mode.clause.clone(),
                });
            }
        }
    }

    let mut translator_versions = BTreeSet::new();
    let mut bridge_lemma_hashes = BTreeSet::new();
    let mut kernel_axioms = BTreeSet::new();
    if let Some(cert) = cert {
        for entry in &cert.atoms {
            if !source_atoms.contains_key(entry.name.as_str()) {
                continue;
            }
            if entry.status == status::TRUSTED {
                trusted_atoms.insert(entry.name.clone());
            }
            if !entry.translator_version.is_empty() {
                translator_versions.insert(entry.translator_version.clone());
            }
            if !entry.bridge_lemma_hash.is_empty() {
                bridge_lemma_hashes.insert(entry.bridge_lemma_hash.clone());
            }
            for metadata in entry
                .lean_metadata
                .iter()
                .chain(entry.lean_result_metadata.iter())
            {
                for axiom in metadata
                    .kernel_axioms
                    .iter()
                    .flatten()
                    .filter(|axiom| !axiom.is_empty())
                {
                    kernel_axioms.insert(axiom.clone());
                }
            }
            if source_atoms.contains_key(entry.name.as_str()) {
                for clause in &entry.assumed_clauses {
                    assumed_clauses.insert(TrustedBaseAssumedClause {
                        atom: entry.name.clone(),
                        clause: assumed_clause_text(clause).to_string(),
                    });
                }
            }
        }
    }

    TrustedBase {
        solver_version: cert.map(|cert| cert.z3_version.clone()),
        mumei_version: cert.map(|cert| cert.mumei_version.clone()),
        translator_versions: translator_versions.into_iter().collect(),
        bridge_lemma_hashes: bridge_lemma_hashes.into_iter().collect(),
        kernel_axioms: kernel_axioms.into_iter().collect(),
        trusted_atoms: trusted_atoms.into_iter().collect(),
        assumed_clauses: assumed_clauses.into_iter().collect(),
    }
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
                && !proof_cert::lean_translator_metadata_is_current(entry)
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
            effective_grade_complete: true,
            evidence_kind,
            freshness,
            reason,
            requires: source_atom.requires.clone(),
            ensures: source_atom.ensures.clone(),
            assumed_clauses: {
                let mut clauses = entry
                    .map(|entry| {
                        entry
                            .assumed_clauses
                            .iter()
                            .cloned()
                            .collect::<BTreeSet<_>>()
                    })
                    .unwrap_or_default();
                for clause_mode in &source_atom.clause_modes {
                    if clause_mode.mode == ClauseTrustMode::Assume {
                        let kind = match &clause_mode.kind {
                            crate::parser::ClauseKind::Requires => "requires",
                            crate::parser::ClauseKind::Ensures => "ensures",
                        };
                        clauses.insert(format!("{kind}: {}", clause_mode.clause));
                    }
                }
                clauses.into_iter().collect()
            },
            dependencies: entry
                .map(|entry| entry.dependencies.clone())
                .unwrap_or_default(),
            unresolved_dependencies: Vec::new(),
            limiting_premises: Vec::new(),
            content_hash: entry.map(|entry| entry.content_hash.clone()),
            proof_hash: entry.map(|entry| entry.proof_hash.clone()),
        };
        claims.insert((*name).to_string(), claim);
    }

    let method_name_matches = resolve_claim_dependencies(&mut claims);
    apply_effective_grades(&mut claims, &method_name_matches);

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
        trusted_base: trusted_base(cert, atoms),
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

    append_trusted_base(&mut markdown, &book.trusted_base);

    if !book.orphaned_entries.is_empty() {
        markdown.push_str("\n## Orphaned certificate entries\n\n| Atom |\n|---|\n");
        for entry in &book.orphaned_entries {
            markdown.push_str(&format!("| `{}` |\n", entry.atom));
        }
    }
    markdown
}

fn append_claim_table<'a>(markdown: &mut String, claims: impl Iterator<Item = &'a Claim>) {
    markdown.push_str("| Atom | Own grade | Freshness | Evidence | Reason | Limiting premises |\n");
    markdown.push_str("|---|---|---|---|---|---|\n");
    let mut count = 0;
    for claim in claims {
        count += 1;
        markdown.push_str(&format!(
            "| `{}` | {} ({}) | {} | {} | {} | {} |\n",
            claim.atom,
            claim.grade_label,
            grade_code(claim.grade),
            freshness_name(claim.freshness),
            claim.evidence_kind,
            claim.reason.replace('|', "\\|"),
            limiting_premises_text(&claim.limiting_premises)
        ));
    }
    if count == 0 {
        markdown.push_str("| — | — | — | — | — | — |\n");
    }
}

fn limiting_premises_text(premises: &[LimitingPremise]) -> String {
    if premises.is_empty() {
        return "-".to_string();
    }

    premises
        .iter()
        .map(|premise| {
            let name = premise.name.replace('|', "\\|");
            let name = match premise.matched_from.as_deref() {
                Some(matched_from) => format!(
                    "`{name}` (matched from `{}` by method name)",
                    matched_from.replace('|', "\\|")
                ),
                None => name,
            };
            format!("{} {} ({})", premise.kind, name, grade_code(premise.grade))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn append_trusted_base(markdown: &mut String, trusted_base: &TrustedBase) {
    markdown.push_str("\n## Trusted base\n\n");
    markdown.push_str(&format!(
        "- Solver version: {}\n- Mumei version: {}\n- Translator versions: {}\n- Bridge lemma hashes: {}\n- Kernel axioms: {}\n- Trusted atoms: {}\n",
        trusted_base.solver_version.as_deref().unwrap_or("—"),
        trusted_base.mumei_version.as_deref().unwrap_or("—"),
        list_or_dash(&trusted_base.translator_versions),
        list_or_dash(&trusted_base.bridge_lemma_hashes),
        list_or_dash(&trusted_base.kernel_axioms),
        list_or_dash(&trusted_base.trusted_atoms),
    ));
    markdown.push_str("- Assumed clauses:\n");
    if trusted_base.assumed_clauses.is_empty() {
        markdown.push_str("  - —\n");
    } else {
        for clause in &trusted_base.assumed_clauses {
            markdown.push_str(&format!("  - `{}`: `{}`\n", clause.atom, clause.clause));
        }
    }
}

fn list_or_dash(values: &[String]) -> String {
    if values.is_empty() {
        "—".to_string()
    } else {
        values.join(", ")
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

    fn claim(
        name: &str,
        grade: SealGrade,
        freshness: Freshness,
        dependencies: &[&str],
        assumed_clauses: &[&str],
    ) -> Claim {
        Claim {
            atom: name.to_string(),
            grade,
            grade_label: grade.label().to_string(),
            effective_grade: SealGrade::G0,
            effective_grade_label: SealGrade::G0.label().to_string(),
            effective_grade_complete: true,
            evidence_kind: "test".to_string(),
            freshness,
            reason: String::new(),
            requires: String::new(),
            ensures: String::new(),
            assumed_clauses: assumed_clauses
                .iter()
                .map(|clause| clause.to_string())
                .collect(),
            dependencies: dependencies
                .iter()
                .map(|dependency| dependency.to_string())
                .collect(),
            unresolved_dependencies: Vec::new(),
            limiting_premises: Vec::new(),
            content_hash: None,
            proof_hash: None,
        }
    }

    fn propagated_claims(claims: Vec<Claim>) -> BTreeMap<String, Claim> {
        let mut claims = claims
            .into_iter()
            .map(|claim| (claim.atom.clone(), claim))
            .collect();
        let method_name_matches = resolve_claim_dependencies(&mut claims);
        apply_effective_grades(&mut claims, &method_name_matches);
        claims
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
            (status::CERTIFICATE_TRUSTED, Freshness::Fresh),
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
    fn rejected_lean_audit_is_fresh_and_distinct_from_translator_staleness() {
        let source = source_atom("sample");
        let mut current_audit_rejected = stale_lean_certificate(&source);
        let current_metadata = lean_atom(Some("rejected"), Some(vec!["propext"]));
        current_audit_rejected.atoms[0].translator_version =
            verification::LEAN_TRANSLATOR_VERSION.to_string();
        current_audit_rejected.atoms[0].bridge_lemma_hash =
            verification::LEAN_BRIDGE_LEMMA_HASH.to_string();
        current_audit_rejected.atoms[0].lean_result_metadata =
            current_metadata.lean_result_metadata;
        proof_cert::refresh_certificate_integrity(&mut current_audit_rejected);
        let atoms = [&source];

        for allow_lean_verified in [false, true] {
            let report = create_claimbook(
                "sample.mm",
                "sample.proof.json",
                Some(&current_audit_rejected),
                &atoms,
                allow_lean_verified,
            );
            let claim = &report.claims[0];
            assert_eq!(claim.freshness, Freshness::Fresh);
            assert_eq!(claim.grade, SealGrade::G0);
            assert!(claim.reason.contains("axiom audit"), "{}", claim.reason);
            assert!(!claim.reason.contains("translator"), "{}", claim.reason);
        }

        let mut stale_translator_audit_rejected = current_audit_rejected;
        stale_translator_audit_rejected.atoms[0].translator_version = "old-translator".to_string();
        proof_cert::refresh_certificate_integrity(&mut stale_translator_audit_rejected);

        for allow_lean_verified in [false, true] {
            let report = create_claimbook(
                "sample.mm",
                "sample.proof.json",
                Some(&stale_translator_audit_rejected),
                &atoms,
                allow_lean_verified,
            );
            let claim = &report.claims[0];
            assert_eq!(claim.freshness, Freshness::Drifted);
            assert_eq!(
                claim.reason,
                "Lean translator metadata is stale; regenerate the Lean certificate"
            );
        }
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

    #[test]
    fn dependency_chains_use_direct_limiting_premises() {
        let claims = propagated_claims(vec![
            claim("a", SealGrade::G4, Freshness::Fresh, &["b"], &[]),
            claim("b", SealGrade::G4, Freshness::Fresh, &["c"], &[]),
            claim("c", SealGrade::G1, Freshness::Fresh, &[], &[]),
        ]);

        assert_eq!(claims["a"].effective_grade, SealGrade::G1);
        assert_eq!(claims["b"].effective_grade, SealGrade::G1);
        assert_eq!(claims["a"].limiting_premises[0].name, "b");
        assert_eq!(claims["b"].limiting_premises[0].name, "c");
        assert!(claims["c"].limiting_premises.is_empty());
    }

    #[test]
    fn dependency_diamonds_and_cycles_converge_to_the_minimum_grade() {
        let diamond = propagated_claims(vec![
            claim("a", SealGrade::G4, Freshness::Fresh, &["b", "c"], &[]),
            claim("b", SealGrade::G4, Freshness::Fresh, &["d"], &[]),
            claim("c", SealGrade::G4, Freshness::Fresh, &["d"], &[]),
            claim("d", SealGrade::G0, Freshness::Fresh, &[], &[]),
        ]);
        assert!(diamond
            .values()
            .all(|claim| claim.effective_grade == SealGrade::G0));

        let mutual = propagated_claims(vec![
            claim("a", SealGrade::G4, Freshness::Fresh, &["b"], &[]),
            claim("b", SealGrade::G1, Freshness::Fresh, &["a"], &[]),
        ]);
        assert_eq!(mutual["a"].effective_grade, SealGrade::G1);
        assert_eq!(mutual["b"].effective_grade, SealGrade::G1);

        let self_loop = propagated_claims(vec![claim(
            "self",
            SealGrade::G4,
            Freshness::Fresh,
            &["self"],
            &[],
        )]);
        assert_eq!(self_loop["self"].effective_grade, SealGrade::G4);
        assert!(self_loop["self"].limiting_premises.is_empty());
    }

    #[test]
    fn drifted_dependencies_and_assumed_clauses_limit_effective_grade() {
        let claims = propagated_claims(vec![
            claim("caller", SealGrade::G4, Freshness::Fresh, &["callee"], &[]),
            claim("callee", SealGrade::G4, Freshness::Drifted, &[], &[]),
        ]);
        assert_eq!(claims["callee"].effective_grade, SealGrade::G0);
        assert_eq!(claims["caller"].effective_grade, SealGrade::G0);
        assert_eq!(
            claims["caller"].limiting_premises,
            vec![LimitingPremise {
                kind: "dependency".to_string(),
                name: "callee".to_string(),
                matched_from: None,
                grade: SealGrade::G0,
            }]
        );
        assert_eq!(
            claims["callee"].limiting_premises,
            vec![LimitingPremise {
                kind: "freshness".to_string(),
                name: "drifted".to_string(),
                matched_from: None,
                grade: SealGrade::G0,
            }]
        );

        let claims = propagated_claims(vec![
            claim(
                "assumed",
                SealGrade::G4,
                Freshness::Fresh,
                &[],
                &["requires: x > 0"],
            ),
            claim(
                "open",
                SealGrade::G0,
                Freshness::Fresh,
                &[],
                &["ensures: result > 0"],
            ),
        ]);
        assert_eq!(claims["assumed"].effective_grade, SealGrade::G1);
        assert_eq!(
            claims["assumed"].limiting_premises,
            vec![LimitingPremise {
                kind: "assumed_clause".to_string(),
                name: "x > 0".to_string(),
                matched_from: None,
                grade: SealGrade::G1,
            }]
        );
        assert_eq!(claims["open"].effective_grade, SealGrade::G0);
        assert!(claims["open"].limiting_premises.is_empty());
    }

    #[test]
    fn unresolved_dependencies_are_reported_without_lowering_grade() {
        let claims = propagated_claims(vec![claim(
            "caller",
            SealGrade::G4,
            Freshness::Fresh,
            &["z_missing", "a_missing"],
            &[],
        )]);

        assert_eq!(claims["caller"].effective_grade, SealGrade::G4);
        assert_eq!(
            claims["caller"].unresolved_dependencies,
            vec!["a_missing".to_string(), "z_missing".to_string()]
        );
        assert!(!claims["caller"].effective_grade_complete);
        assert!(claims["caller"].limiting_premises.is_empty());
    }

    #[test]
    fn dependency_completeness_propagates_through_chains_and_cycles() {
        let chain = propagated_claims(vec![
            claim("a", SealGrade::G4, Freshness::Fresh, &["b"], &[]),
            claim("b", SealGrade::G4, Freshness::Fresh, &["missing"], &[]),
        ]);
        assert!(chain["a"].unresolved_dependencies.is_empty());
        assert!(!chain["a"].effective_grade_complete);
        assert_eq!(
            chain["b"].unresolved_dependencies,
            vec!["missing".to_string()]
        );
        assert!(!chain["b"].effective_grade_complete);

        let cycle = propagated_claims(vec![
            claim("a", SealGrade::G4, Freshness::Fresh, &["b"], &[]),
            claim("b", SealGrade::G4, Freshness::Fresh, &["a", "missing"], &[]),
        ]);
        assert!(cycle["a"].unresolved_dependencies.is_empty());
        assert_eq!(
            cycle["b"].unresolved_dependencies,
            vec!["missing".to_string()]
        );
        assert!(!cycle["a"].effective_grade_complete);
        assert!(!cycle["b"].effective_grade_complete);
    }

    #[test]
    fn bare_method_dependencies_resolve_only_to_a_unique_qualified_claim() {
        let caller = source_atom("caller");
        let mut point_get = source_atom("Point::get");
        point_get.trust_level = TrustLevel::Trusted;
        let outer = source_atom("outer");
        let atoms = [&caller, &point_get, &outer];
        let results = std::collections::HashMap::from([
            (
                caller.name.clone(),
                (status::Z3_UNSAT.to_string(), status::VERIFIED.to_string()),
            ),
            (
                point_get.name.clone(),
                (status::Z3_UNSAT.to_string(), status::VERIFIED.to_string()),
            ),
            (
                outer.name.clone(),
                (status::Z3_UNSAT.to_string(), status::VERIFIED.to_string()),
            ),
        ]);
        let mut cert = proof_cert::generate_certificate(
            "sample.mm",
            &atoms,
            &results,
            &verification::ModuleEnv::new(),
            None,
            None,
            None,
        );
        cert.atoms
            .iter_mut()
            .find(|entry| entry.name == "caller")
            .expect("caller certificate")
            .dependencies = vec!["get".to_string()];
        cert.atoms
            .iter_mut()
            .find(|entry| entry.name == "outer")
            .expect("outer certificate")
            .dependencies = vec!["caller".to_string()];
        proof_cert::refresh_certificate_integrity(&mut cert);

        let report = create_claimbook("sample.mm", "sample.proof.json", Some(&cert), &atoms, false);
        let caller_claim = report
            .claims
            .iter()
            .find(|claim| claim.atom == "caller")
            .expect("caller claim");
        assert_eq!(caller_claim.dependencies, vec!["Point::get".to_string()]);
        assert_eq!(caller_claim.effective_grade, SealGrade::G1);
        assert!(!caller_claim.effective_grade_complete);
        assert_eq!(caller_claim.limiting_premises.len(), 1);
        assert_eq!(caller_claim.limiting_premises[0].kind, "dependency");
        assert_eq!(caller_claim.limiting_premises[0].name, "Point::get");
        assert_eq!(
            caller_claim.limiting_premises[0].matched_from.as_deref(),
            Some("get")
        );
        assert_eq!(caller_claim.limiting_premises[0].grade, SealGrade::G1);
        let outer_claim = report
            .claims
            .iter()
            .find(|claim| claim.atom == "outer")
            .expect("outer claim");
        assert_eq!(outer_claim.effective_grade, SealGrade::G1);
        assert!(!outer_claim.effective_grade_complete);

        let mut circle_get = source_atom("Circle::get");
        circle_get.trust_level = TrustLevel::Trusted;
        let ambiguous_atoms = [&caller, &point_get, &circle_get];
        let ambiguous_results = ambiguous_atoms
            .iter()
            .map(|atom| {
                (
                    atom.name.clone(),
                    (status::Z3_UNSAT.to_string(), status::VERIFIED.to_string()),
                )
            })
            .collect();
        let mut ambiguous_cert = proof_cert::generate_certificate(
            "sample.mm",
            &ambiguous_atoms,
            &ambiguous_results,
            &verification::ModuleEnv::new(),
            None,
            None,
            None,
        );
        ambiguous_cert
            .atoms
            .iter_mut()
            .find(|entry| entry.name == "caller")
            .expect("caller certificate")
            .dependencies = vec!["get".to_string()];
        proof_cert::refresh_certificate_integrity(&mut ambiguous_cert);

        let ambiguous_report = create_claimbook(
            "sample.mm",
            "sample.proof.json",
            Some(&ambiguous_cert),
            &ambiguous_atoms,
            false,
        );
        let ambiguous_caller = ambiguous_report
            .claims
            .iter()
            .find(|claim| claim.atom == "caller")
            .expect("caller claim");
        assert_eq!(ambiguous_caller.dependencies, vec!["get".to_string()]);
        assert_eq!(
            ambiguous_caller.unresolved_dependencies,
            vec!["get".to_string()]
        );
        assert_eq!(ambiguous_caller.effective_grade, SealGrade::G4);
        assert!(!ambiguous_caller.effective_grade_complete);

        let mut exact_name_claims = BTreeMap::from([
            (
                "caller".to_string(),
                claim("caller", SealGrade::G4, Freshness::Fresh, &["get"], &[]),
            ),
            (
                "get".to_string(),
                claim("get", SealGrade::G1, Freshness::Fresh, &[], &[]),
            ),
            (
                "Point::get".to_string(),
                claim("Point::get", SealGrade::G1, Freshness::Fresh, &[], &[]),
            ),
        ]);
        let exact_method_name_matches = resolve_claim_dependencies(&mut exact_name_claims);
        apply_effective_grades(&mut exact_name_claims, &exact_method_name_matches);
        assert_eq!(
            exact_name_claims["caller"].dependencies,
            vec!["get".to_string()]
        );
        assert!(exact_name_claims["caller"].effective_grade_complete);
    }

    #[test]
    fn source_assume_clauses_join_legacy_certificate_and_trusted_base() {
        let mut source = source_atom("assumed");
        source.requires = "x > 0".to_string();
        source.clause_modes.push(crate::parser::ClauseMode {
            kind: crate::parser::ClauseKind::Requires,
            clause: "x > 0".to_string(),
            mode: ClauseTrustMode::Assume,
        });
        let atoms = [&source];
        let results = std::collections::HashMap::from([(
            source.name.clone(),
            (status::Z3_UNSAT.to_string(), status::VERIFIED.to_string()),
        )]);
        let mut cert = proof_cert::generate_certificate(
            "sample.mm",
            &atoms,
            &results,
            &verification::ModuleEnv::new(),
            None,
            None,
            None,
        );
        cert.version = "1.0".to_string();
        cert.atoms[0].content_hash = proof_cert::compute_atom_content_hash(
            &source.name,
            &source.requires,
            &source.ensures,
            &source.body_expr,
        );
        cert.atoms[0].assumed_clauses.clear();
        proof_cert::refresh_certificate_integrity(&mut cert);

        let report = create_claimbook("sample.mm", "sample.proof.json", Some(&cert), &atoms, false);
        assert_eq!(report.claims[0].assumed_clauses, vec!["requires: x > 0"]);
        assert_eq!(report.claims[0].effective_grade, SealGrade::G1);
        assert_eq!(
            report.claims[0].limiting_premises,
            vec![LimitingPremise {
                kind: "assumed_clause".to_string(),
                name: "x > 0".to_string(),
                matched_from: None,
                grade: SealGrade::G1,
            }]
        );
        assert_eq!(
            report.trusted_base.assumed_clauses,
            vec![TrustedBaseAssumedClause {
                atom: "assumed".to_string(),
                clause: "x > 0".to_string(),
            }]
        );
    }

    #[test]
    fn trusted_base_unions_certificate_and_source_data_deterministically() {
        let mut alpha = source_atom("alpha");
        alpha.trust_level = TrustLevel::Trusted;
        alpha.clause_modes.push(crate::parser::ClauseMode {
            kind: crate::parser::ClauseKind::Requires,
            clause: "x > 0".to_string(),
            mode: ClauseTrustMode::Assume,
        });
        let beta = source_atom("beta");

        let mut alpha_cert = atom_certificate(status::Z3_UNSAT, status::VERIFIED);
        alpha_cert.name = "alpha".to_string();
        alpha_cert.translator_version = "translator-z".to_string();
        alpha_cert.bridge_lemma_hash = "bridge-z".to_string();
        alpha_cert.assumed_clauses = vec!["requires: x > 0".to_string()];
        alpha_cert.lean_metadata = Some(crate::proof_cert::LeanResultMetadata {
            kernel_axioms: Some(vec!["propext".to_string(), "Quot.sound".to_string()]),
            ..Default::default()
        });
        alpha_cert.lean_result_metadata = Some(crate::proof_cert::LeanResultMetadata {
            kernel_axioms: Some(vec!["Quot.sound".to_string()]),
            ..Default::default()
        });

        let mut beta_cert = atom_certificate(status::Z3_UNSAT, status::TRUSTED);
        beta_cert.name = "beta".to_string();
        beta_cert.translator_version = "translator-a".to_string();
        beta_cert.bridge_lemma_hash = "bridge-a".to_string();
        beta_cert.assumed_clauses = vec!["ensures: result > 0".to_string()];
        beta_cert.lean_result_metadata = Some(crate::proof_cert::LeanResultMetadata {
            kernel_axioms: Some(vec!["Classical.choice".to_string(), "propext".to_string()]),
            ..Default::default()
        });

        let mut orphan = atom_certificate(status::Z3_UNSAT, status::TRUSTED);
        orphan.name = "orphan".to_string();
        orphan.translator_version = "orphan-only-translator".to_string();
        orphan.bridge_lemma_hash = "orphan-only-bridge".to_string();
        orphan.lean_metadata = Some(crate::proof_cert::LeanResultMetadata {
            kernel_axioms: Some(vec!["Orphan.kernel_axiom".to_string()]),
            ..Default::default()
        });

        let mut cert = proof_certificate();
        cert.atoms = vec![orphan, beta_cert, alpha_cert];
        let base = trusted_base(Some(&cert), &[&beta, &alpha]);

        assert_eq!(base.solver_version.as_deref(), Some("4.14.1"));
        assert_eq!(
            base.mumei_version.as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(
            base.translator_versions,
            vec!["translator-a".to_string(), "translator-z".to_string()]
        );
        assert_eq!(
            base.bridge_lemma_hashes,
            vec!["bridge-a".to_string(), "bridge-z".to_string()]
        );
        assert_eq!(
            base.kernel_axioms,
            vec![
                "Classical.choice".to_string(),
                "Quot.sound".to_string(),
                "propext".to_string()
            ]
        );
        assert_eq!(
            base.trusted_atoms,
            vec!["alpha".to_string(), "beta".to_string()]
        );
        assert_eq!(
            base.assumed_clauses,
            [
                TrustedBaseAssumedClause {
                    atom: "alpha".to_string(),
                    clause: "x > 0".to_string(),
                },
                TrustedBaseAssumedClause {
                    atom: "beta".to_string(),
                    clause: "result > 0".to_string(),
                },
            ]
        );
        assert_eq!(
            serde_json::to_string(&base).expect("serialize trusted base"),
            serde_json::to_string(&trusted_base(Some(&cert), &[&beta, &alpha]))
                .expect("serialize trusted base again")
        );
    }

    #[test]
    fn trusted_base_without_certificate_uses_source_declarations_only() {
        let mut atom = source_atom("source_trusted");
        atom.trust_level = TrustLevel::Trusted;
        atom.clause_modes.push(crate::parser::ClauseMode {
            kind: crate::parser::ClauseKind::Ensures,
            clause: "result > 0".to_string(),
            mode: ClauseTrustMode::Assume,
        });

        let base = trusted_base(None, &[&atom]);
        assert_eq!(base.solver_version, None);
        assert_eq!(base.mumei_version, None);
        assert!(base.translator_versions.is_empty());
        assert!(base.bridge_lemma_hashes.is_empty());
        assert!(base.kernel_axioms.is_empty());
        assert_eq!(base.trusted_atoms, vec!["source_trusted".to_string()]);
        assert_eq!(
            base.assumed_clauses,
            [TrustedBaseAssumedClause {
                atom: "source_trusted".to_string(),
                clause: "result > 0".to_string(),
            }]
        );
    }

    #[test]
    fn markdown_escapes_pipes_in_limiting_assumed_clauses() {
        let mut claim = claim("sample", SealGrade::G4, Freshness::Fresh, &[], &["x | y"]);
        claim.effective_grade = SealGrade::G1;
        claim.effective_grade_label = SealGrade::G1.label().to_string();
        claim.limiting_premises = vec![LimitingPremise {
            kind: "assumed_clause".to_string(),
            name: "x | y".to_string(),
            matched_from: None,
            grade: SealGrade::G1,
        }];
        let mut summary = SealGrade::all()
            .into_iter()
            .map(|grade| (grade, 0))
            .collect::<BTreeMap<_, _>>();
        summary.insert(SealGrade::G1, 1);
        let book = ClaimBook {
            schema: SCHEMA.to_string(),
            input: "sample.mm".to_string(),
            certificate: "sample.proof.json".to_string(),
            certificate_integrity: CertificateIntegrity::Absent,
            generator: ClaimBookGenerator {
                mumei_version: None,
                z3_version: None,
            },
            trusted_base: trusted_base(None, &[]),
            summary: ClaimBookSummary {
                by_effective_grade: summary,
                by_freshness: BTreeMap::from([
                    (Freshness::Fresh, 1),
                    (Freshness::Drifted, 0),
                    (Freshness::Uncertified, 0),
                ]),
                total: 1,
            },
            claims: vec![claim],
            orphaned_entries: Vec::new(),
        };

        let markdown = render_markdown(&book);
        assert!(markdown.contains("## Trusted base"));
        assert!(markdown.contains("Limiting premises"));
        assert!(markdown.contains("assumed_clause x \\| y (G1)"));
    }
}
