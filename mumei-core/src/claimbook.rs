use crate::controls::ControlsAtomReport;
use crate::observe::ObservationResult;
use crate::parser::{ClauseTrustMode, TrustLevel};
use crate::proof_cert::{self, status, AtomCertificate, ProofCertificate};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const SCHEMA: &str = "mumei.claimbook/v1";
const DEPENDENCY_CONTRACT_CHANGED_REASON: &str =
    "a dependency's contract changed since the certificate was produced";
const LEAN_TRANSLATOR_METADATA_STALE_REASON: &str =
    "Lean translator metadata is stale; regenerate the Lean certificate";

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discrimination: Option<ClaimDiscrimination>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation: Option<ClaimObservation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaimObservation {
    pub status: String,
    pub supporting: usize,
    pub contradicting: usize,
    pub neutral: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaimDiscrimination {
    pub status: String,
    pub detail: String,
    pub tested: usize,
    pub killed: usize,
    pub survived: usize,
    pub unanalyzable: usize,
    pub surviving: Vec<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_discrimination: Option<BTreeMap<String, usize>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_observation: Option<BTreeMap<String, usize>>,
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
    #[serde(skip)]
    recomputation: ClaimBookRecomputation,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ClaimBookRecomputation {
    unsatisfiable_specs: BTreeSet<String>,
    method_name_matches: BTreeMap<String, BTreeMap<String, String>>,
    source_dependencies: BTreeMap<String, Vec<String>>,
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
        let dual_check = proof_cert::lean_dual_check_status(atom);
        if allow_lean_verified && dual_check == proof_cert::LeanDualCheck::Passed {
            return (
                SealGrade::G5,
                "z3+lean".to_string(),
                "Z3 discharged the obligation and an independent Lean proof passed its axiom audit"
                    .to_string(),
            );
        }
        let reason = if atom.lean_dual_check.is_some() {
            let cause = if !allow_lean_verified {
                "requires --allow-lean-verified"
            } else {
                match dual_check {
                    proof_cert::LeanDualCheck::Stale => "stale translator",
                    proof_cert::LeanDualCheck::NotVerified => "Lean result is not verified",
                    proof_cert::LeanDualCheck::AxiomRejected => "axiom audit did not pass",
                    proof_cert::LeanDualCheck::Absent => "metadata is absent",
                    proof_cert::LeanDualCheck::Passed => "requires --allow-lean-verified",
                }
            };
            format!("Z3 discharged the obligation; Lean dual check not counted: {cause}")
        } else {
            "Z3 discharged the obligation".to_string()
        };
        return (SealGrade::G4, "z3".to_string(), reason);
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
        status::CERTIFICATE_CHANGED
        | status::CERTIFICATE_DEPENDENCY_CHANGED
        | status::CERTIFICATE_STALE_TRANSLATOR => Freshness::Drifted,
        status::CERTIFICATE_PROVEN
        | status::CERTIFICATE_TRUSTED
        | status::CERTIFICATE_UNPROVEN
        | status::CERTIFICATE_DEPENDENCY_UNCHECKED
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
            let fresh_grade = if claim.grade == SealGrade::G2 || claim.freshness == Freshness::Fresh
            {
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
        let claim_method_name_matches = resolve_claim_dependencies_for_claim(claim, &claim_names);
        if !claim_method_name_matches.is_empty() {
            method_name_matches.insert(claim_name.clone(), claim_method_name_matches);
        }
    }
    method_name_matches
}

fn resolve_claim_dependencies_for_claim(
    claim: &mut Claim,
    claim_names: &BTreeSet<String>,
) -> BTreeMap<String, String> {
    let mut method_name_matches = BTreeMap::new();
    claim.dependencies = claim
        .dependencies
        .iter()
        .map(|dependency| {
            if claim_names.contains(dependency) {
                return dependency.clone();
            }

            let suffix = format!("::{dependency}");
            let mut qualified_matches = claim_names.iter().filter(|name| name.ends_with(&suffix));
            match (qualified_matches.next(), qualified_matches.next()) {
                (Some(match_name), None) => {
                    method_name_matches.insert(match_name.clone(), dependency.clone());
                    match_name.clone()
                }
                _ => dependency.clone(),
            }
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
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
    if claim.freshness != Freshness::Fresh
        && claim.grade > SealGrade::G0
        && claim.grade != SealGrade::G2
    {
        premises.insert(LimitingPremise {
            kind: "freshness".to_string(),
            name: freshness_name(claim.freshness).to_string(),
            matched_from: None,
            grade: SealGrade::G0,
        });
    }

    let fresh_grade = if claim.grade == SealGrade::G2 || claim.freshness == Freshness::Fresh {
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
                    clause: format!(
                        "{}: {}",
                        match &clause_mode.kind {
                            crate::parser::ClauseKind::Requires => "requires",
                            crate::parser::ClauseKind::Ensures => "ensures",
                        },
                        clause_mode.clause
                    ),
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
                        clause: clause.clone(),
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
    env: Option<&crate::verification::ModuleEnv>,
) -> ClaimBook {
    let input = input.into();
    let certificate_path = certificate_path.into();
    let integrity = certificate_integrity(cert);
    let verification_results: BTreeMap<String, proof_cert::CertificateAtomCheck> = cert
        .map(|cert| {
            proof_cert::verify_certificate_in_env(cert, atoms, allow_lean_verified, env)
                .into_iter()
                .map(|check| (check.name.clone(), check))
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
    let source_dependencies: BTreeMap<String, Vec<String>> = atoms
        .iter()
        .map(|atom| {
            let mut dependencies = crate::resolver::collect_callees_from_atom(atom)
                .into_iter()
                .collect::<Vec<_>>();
            dependencies.sort();
            (atom.name.clone(), dependencies)
        })
        .collect();

    let mut claims = BTreeMap::new();
    let mut unsatisfiable_specs = BTreeSet::new();
    for (name, source_atom) in &source_atoms {
        let entry = certified.get(name).copied();
        if entry
            .and_then(|entry| entry.spec_validation_result.as_ref())
            .is_some_and(|result| !result.is_satisfiable)
        {
            unsatisfiable_specs.insert((*name).to_string());
        }
        let verification_result = entry
            .and_then(|_| verification_results.get(*name))
            .map(|check| check.status.as_str());
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
        } else if verification_result == Some(status::CERTIFICATE_DEPENDENCY_CHANGED) {
            (Freshness::Drifted, Some(DEPENDENCY_CONTRACT_CHANGED_REASON))
        } else if stale_lean_metadata
            || verification_result == Some(status::CERTIFICATE_STALE_TRANSLATOR)
        {
            (
                Freshness::Drifted,
                Some(LEAN_TRANSLATOR_METADATA_STALE_REASON),
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

        let (mut grade, evidence_kind, mut own_reason) = entry
            .map(|entry| own_grade(entry, &source_atom.trust_level, allow_lean_verified))
            .unwrap_or((
                SealGrade::G0,
                "none".to_string(),
                "no certificate entry exists for this atom".to_string(),
            ));
        let evidence_kind = if entry.is_some_and(|atom| atom.reused_from_store) {
            format!("{evidence_kind} (reused from local proof store)")
        } else {
            evidence_kind
        };
        if verification_result == Some(status::CERTIFICATE_DEPENDENCY_CHANGED) {
            grade = SealGrade::G0;
            own_reason = DEPENDENCY_CONTRACT_CHANGED_REASON.to_string();
        }
        let reason = match freshness_reason {
            Some(DEPENDENCY_CONTRACT_CHANGED_REASON) => {
                DEPENDENCY_CONTRACT_CHANGED_REASON.to_string()
            }
            Some(LEAN_TRANSLATOR_METADATA_STALE_REASON) => {
                LEAN_TRANSLATOR_METADATA_STALE_REASON.to_string()
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
            discrimination: None,
            observation: None,
        };
        claims.insert((*name).to_string(), claim);
    }

    let method_name_matches = resolve_claim_dependencies(&mut claims);

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

    let mut book = ClaimBook {
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
            by_effective_grade: BTreeMap::new(),
            by_freshness: BTreeMap::new(),
            total: 0,
            by_discrimination: None,
            by_observation: None,
        },
        claims: claims.into_values().collect(),
        orphaned_entries,
        recomputation: ClaimBookRecomputation {
            unsatisfiable_specs,
            method_name_matches,
            source_dependencies,
        },
    };
    recompute_claimbook_derived_fields(&mut book);
    book
}

fn recompute_claimbook_derived_fields(book: &mut ClaimBook) {
    let mut claims = std::mem::take(&mut book.claims)
        .into_iter()
        .map(|claim| (claim.atom.clone(), claim))
        .collect::<BTreeMap<_, _>>();
    apply_effective_grades(&mut claims, &book.recomputation.method_name_matches);

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

    book.summary.by_effective_grade = by_effective_grade;
    book.summary.by_freshness = by_freshness;
    book.summary.total = claims.len();
    book.claims = claims.into_values().collect();
}

pub fn attach_observations(book: &mut ClaimBook, results: &[ObservationResult]) {
    let mut by_observation = BTreeMap::new();
    let claim_names = book
        .claims
        .iter()
        .map(|claim| claim.atom.clone())
        .collect::<BTreeSet<_>>();
    for claim in &mut book.claims {
        let mut observation = ClaimObservation {
            status: String::new(),
            supporting: 0,
            contradicting: 0,
            neutral: 0,
        };
        for result in results.iter().filter(|result| result.target == claim.atom) {
            match result.verdict {
                "consistent" | "entailed" => observation.supporting += 1,
                "contradicts_ensures" | "not_entailed" => observation.contradicting += 1,
                _ => observation.neutral += 1,
            }
        }
        observation.status = if observation.contradicting > 0 {
            "contradicted"
        } else if observation.supporting > 0 {
            "supported"
        } else if observation.neutral > 0 {
            "inconclusive"
        } else {
            "not_observed"
        }
        .to_string();

        if observation.status == "supported"
            && (claim.grade == SealGrade::G0 || claim.freshness != Freshness::Fresh)
            && (claim.freshness != Freshness::Fresh
                || !book.recomputation.unsatisfiable_specs.contains(&claim.atom))
        {
            let mut dependencies = claim.dependencies.iter().cloned().collect::<BTreeSet<_>>();
            dependencies.extend(
                book.recomputation
                    .source_dependencies
                    .get(&claim.atom)
                    .into_iter()
                    .flatten()
                    .cloned(),
            );
            claim.dependencies = dependencies.into_iter().collect();
            let method_name_matches = resolve_claim_dependencies_for_claim(claim, &claim_names);
            if !method_name_matches.is_empty() {
                book.recomputation
                    .method_name_matches
                    .entry(claim.atom.clone())
                    .or_default()
                    .extend(method_name_matches);
            }
            let previous_reason = format!(
                "current test observations agree with the contract (previously {} {}: {})",
                grade_code(claim.grade),
                claim.evidence_kind,
                claim.reason
            );
            claim.grade = SealGrade::G2;
            claim.grade_label = SealGrade::G2.label().to_string();
            claim.evidence_kind = "observation".to_string();
            claim.reason = previous_reason;
        }

        *by_observation
            .entry(observation.status.clone())
            .or_insert(0) += 1;
        claim.observation = Some(observation);
    }
    book.summary.by_observation = Some(by_observation);
    recompute_claimbook_derived_fields(book);
}

pub fn attach_controls(book: &mut ClaimBook, controls: &[ControlsAtomReport]) {
    let controls_by_name = controls
        .iter()
        .map(|report| (report.name.as_str(), report))
        .collect::<BTreeMap<_, _>>();
    let mut by_discrimination = BTreeMap::new();

    for claim in &mut book.claims {
        let discrimination = controls_by_name
            .get(claim.atom.as_str())
            .map(|report| ClaimDiscrimination {
                status: report.status.to_string(),
                detail: report.detail.clone(),
                tested: report.tested,
                killed: report.killed,
                survived: report.survived,
                unanalyzable: report.unanalyzable,
                surviving: report
                    .mutants
                    .iter()
                    .filter(|mutant| mutant.outcome == "survived")
                    .map(|mutant| mutant.id.clone())
                    .collect(),
            })
            .unwrap_or_else(|| ClaimDiscrimination {
                status: "not_checked".to_string(),
                detail: "no controls entry".to_string(),
                tested: 0,
                killed: 0,
                survived: 0,
                unanalyzable: 0,
                surviving: Vec::new(),
            });
        *by_discrimination
            .entry(discrimination.status.clone())
            .or_insert(0) += 1;
        claim.discrimination = Some(discrimination);
    }

    book.summary.by_discrimination = Some(by_discrimination);
}

pub fn fails_discrimination_gate(discrimination: &ClaimDiscrimination) -> bool {
    matches!(discrimination.status.as_str(), "weak" | "inconclusive")
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
    if let Some(by_observation) = &book.summary.by_observation {
        markdown.push_str("\n| Observation | Count |\n|---|---:|\n");
        for (status, count) in by_observation {
            markdown.push_str(&format!("| {status} | {count} |\n"));
        }
    }
    if let Some(by_discrimination) = &book.summary.by_discrimination {
        let counts = by_discrimination
            .iter()
            .map(|(status, count)| format!("{status}={count}"))
            .collect::<Vec<_>>()
            .join(", ");
        markdown.push_str(&format!("\nControls: {counts}\n"));
    }

    markdown.push_str("\n## Drifted or uncertified\n\n");
    append_claim_table(
        &mut markdown,
        book.claims
            .iter()
            .filter(|claim| claim.freshness != Freshness::Fresh),
        book.summary.by_discrimination.is_some(),
        book.summary.by_observation.is_some(),
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
            book.summary.by_discrimination.is_some(),
            book.summary.by_observation.is_some(),
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

fn append_claim_table<'a>(
    markdown: &mut String,
    claims: impl Iterator<Item = &'a Claim>,
    include_controls: bool,
    include_observations: bool,
) {
    match (include_controls, include_observations) {
        (true, true) => {
            markdown.push_str("| Atom | Own grade | Freshness | Evidence | Reason | Limiting premises | Controls | Observation |\n");
            markdown.push_str("|---|---|---|---|---|---|---|---|\n");
        }
        (true, false) => {
            markdown.push_str(
                "| Atom | Own grade | Freshness | Evidence | Reason | Limiting premises | Controls |\n",
            );
            markdown.push_str("|---|---|---|---|---|---|---|\n");
        }
        (false, true) => {
            markdown.push_str(
                "| Atom | Own grade | Freshness | Evidence | Reason | Limiting premises | Observation |\n",
            );
            markdown.push_str("|---|---|---|---|---|---|---|\n");
        }
        (false, false) => {
            markdown.push_str(
                "| Atom | Own grade | Freshness | Evidence | Reason | Limiting premises |\n",
            );
            markdown.push_str("|---|---|---|---|---|---|\n");
        }
    }
    let mut count = 0;
    for claim in claims {
        count += 1;
        let row = format!(
            "| `{}` | {} ({}) | {} | {} | {} | {}",
            claim.atom,
            claim.grade_label,
            grade_code(claim.grade),
            freshness_name(claim.freshness),
            claim.evidence_kind,
            claim.reason.replace('|', "\\|"),
            limiting_premises_text(&claim.limiting_premises)
        );
        let controls = include_controls.then(|| {
            claim
                .discrimination
                .as_ref()
                .map(|discrimination| match discrimination.status.as_str() {
                    "weak" => format!("weak ({} survived)", discrimination.survived),
                    "discriminating" => {
                        format!(
                            "discriminating {}/{}",
                            discrimination.killed, discrimination.tested
                        )
                    }
                    _ => discrimination.status.clone(),
                })
                .unwrap_or_else(|| "not_checked".to_string())
        });
        if let Some(controls) = controls {
            markdown.push_str(&format!("{row} | {controls}"));
        }
        if include_observations {
            let observation = claim
                .observation
                .as_ref()
                .map(|observation| observation.status.as_str())
                .unwrap_or("not_observed");
            if include_controls {
                markdown.push_str(&format!(" | {observation}"));
            } else {
                markdown.push_str(&format!("{row} | {observation}"));
            }
        } else if !include_controls {
            markdown.push_str(&row);
        }
        markdown.push_str(" |\n");
    }
    if count == 0 {
        let mut cells = vec!["—"; 6];
        if include_controls {
            cells.push("—");
        }
        if include_observations {
            cells.push("—");
        }
        markdown.push_str(&format!("| {} |\n", cells.join(" | ")));
    }
}

pub fn limiting_premises_text(premises: &[LimitingPremise]) -> String {
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
            discrimination: None,
            observation: None,
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

    fn claimbook_with_claims(claims: Vec<Claim>) -> ClaimBook {
        let mut claims = claims
            .into_iter()
            .map(|claim| (claim.atom.clone(), claim))
            .collect::<BTreeMap<_, _>>();
        let method_name_matches = resolve_claim_dependencies(&mut claims);
        let mut book = ClaimBook {
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
                by_effective_grade: BTreeMap::new(),
                by_freshness: BTreeMap::new(),
                total: 0,
                by_discrimination: None,
                by_observation: None,
            },
            claims: claims.into_values().collect(),
            orphaned_entries: Vec::new(),
            recomputation: ClaimBookRecomputation {
                unsatisfiable_specs: BTreeSet::new(),
                method_name_matches,
                source_dependencies: BTreeMap::new(),
            },
        };
        recompute_claimbook_derived_fields(&mut book);
        book
    }

    fn observation_result(target: &str, verdict: &'static str) -> ObservationResult {
        ObservationResult {
            kind: "example".to_string(),
            target: target.to_string(),
            test: "observed_test".to_string(),
            file: "tests.mm".to_string(),
            line: 1,
            args: json!({}),
            result: None,
            predicate: None,
            verdict,
            detail: String::new(),
        }
    }

    #[test]
    fn observation_verdicts_are_classified_into_support_contradiction_and_neutral() {
        for (verdict, expected_status, supporting, contradicting, neutral) in [
            ("consistent", "supported", 1, 0, 0),
            ("contradicts_ensures", "contradicted", 0, 1, 0),
            ("outside_requires", "inconclusive", 0, 0, 1),
            ("inconclusive", "inconclusive", 0, 0, 1),
            ("rejection_explained", "inconclusive", 0, 0, 1),
            ("requires_admits_rejected", "inconclusive", 0, 0, 1),
            ("not_checked", "inconclusive", 0, 0, 1),
            ("entailed", "supported", 1, 0, 0),
            ("not_entailed", "contradicted", 0, 1, 0),
            ("domain_exceeds_requires", "inconclusive", 0, 0, 1),
        ] {
            let mut book = claimbook_with_claims(vec![claim(
                "sample",
                SealGrade::G0,
                Freshness::Uncertified,
                &[],
                &[],
            )]);
            attach_observations(&mut book, &[observation_result("sample", verdict)]);
            let observation = book.claims[0]
                .observation
                .as_ref()
                .expect("observation attached");
            assert_eq!(observation.status, expected_status, "{verdict}");
            assert_eq!(observation.supporting, supporting, "{verdict}");
            assert_eq!(observation.contradicting, contradicting, "{verdict}");
            assert_eq!(observation.neutral, neutral, "{verdict}");
        }

        let mut mixed = claimbook_with_claims(vec![claim(
            "sample",
            SealGrade::G0,
            Freshness::Uncertified,
            &[],
            &[],
        )]);
        attach_observations(
            &mut mixed,
            &[
                observation_result("sample", "consistent"),
                observation_result("sample", "contradicts_ensures"),
            ],
        );
        assert_eq!(
            mixed.claims[0].observation.as_ref().unwrap().status,
            "contradicted"
        );
        assert_eq!(mixed.claims[0].grade, SealGrade::G0);

        let mut neutral = claimbook_with_claims(vec![claim(
            "sample",
            SealGrade::G0,
            Freshness::Uncertified,
            &[],
            &[],
        )]);
        attach_observations(&mut neutral, &[observation_result("sample", "not_checked")]);
        assert_eq!(
            neutral.claims[0].observation.as_ref().unwrap().status,
            "inconclusive"
        );

        let mut unmatched = claimbook_with_claims(vec![claim(
            "sample",
            SealGrade::G0,
            Freshness::Uncertified,
            &[],
            &[],
        )]);
        attach_observations(&mut unmatched, &[]);
        assert_eq!(
            unmatched.claims[0].observation.as_ref().unwrap().status,
            "not_observed"
        );

        let mut method = claimbook_with_claims(vec![claim(
            "Point::get",
            SealGrade::G0,
            Freshness::Uncertified,
            &[],
            &[],
        )]);
        attach_observations(
            &mut method,
            &[observation_result("Point::get", "consistent")],
        );
        assert_eq!(method.claims[0].grade, SealGrade::G2);
        assert_eq!(
            method.claims[0].observation.as_ref().unwrap().status,
            "supported"
        );
    }

    #[test]
    fn supporting_observations_lift_unproven_uncertified_and_drifted_g0_claims() {
        let mut book = claimbook_with_claims(vec![
            claim("unproven", SealGrade::G0, Freshness::Fresh, &[], &[]),
            claim(
                "uncertified",
                SealGrade::G0,
                Freshness::Uncertified,
                &[],
                &[],
            ),
            claim("drifted", SealGrade::G0, Freshness::Drifted, &[], &[]),
        ]);
        attach_observations(
            &mut book,
            &[
                observation_result("unproven", "consistent"),
                observation_result("uncertified", "entailed"),
                observation_result("drifted", "consistent"),
            ],
        );
        for name in ["unproven", "uncertified", "drifted"] {
            let claim = book.claims.iter().find(|claim| claim.atom == name).unwrap();
            assert_eq!(claim.grade, SealGrade::G2, "{name}");
            assert!(claim.effective_grade_complete, "{name}");
            assert_eq!(claim.evidence_kind, "observation", "{name}");
            assert_eq!(
                claim.reason,
                "current test observations agree with the contract (previously G0 test: )",
                "{name}"
            );
        }
        let drifted = book
            .claims
            .iter()
            .find(|claim| claim.atom == "drifted")
            .unwrap();
        assert_eq!(drifted.effective_grade, SealGrade::G2);
        assert!(drifted.limiting_premises.is_empty());

        let mut unverified_source = source_atom("unverified_source");
        unverified_source.trust_level = TrustLevel::Unverified;
        let mut unverified = create_claimbook(
            "sample.mm",
            "sample.proof.json",
            None,
            &[&unverified_source],
            false,
            None,
        );
        attach_observations(
            &mut unverified,
            &[observation_result("unverified_source", "consistent")],
        );
        assert_eq!(unverified.claims[0].grade, SealGrade::G2);
    }

    #[test]
    fn lifted_uncertified_claims_use_current_source_dependencies() {
        let create_book = || {
            let mut caller = source_atom("caller");
            caller.body_expr = "callee(x)".to_string();
            let callee = source_atom("callee");
            create_claimbook(
                "sample.mm",
                "sample.proof.json",
                None,
                &[&caller, &callee],
                false,
                None,
            )
        };

        let mut caller_only = create_book();
        attach_observations(
            &mut caller_only,
            &[observation_result("caller", "consistent")],
        );
        let caller = caller_only
            .claims
            .iter()
            .find(|claim| claim.atom == "caller")
            .unwrap();
        assert_eq!(caller.grade, SealGrade::G2);
        assert_eq!(caller.effective_grade, SealGrade::G0);
        assert!(caller.effective_grade_complete);
        assert_eq!(caller.dependencies, vec!["callee"]);
        assert!(!caller_only
            .recomputation
            .method_name_matches
            .contains_key("caller"));
        assert_eq!(
            caller.limiting_premises,
            vec![LimitingPremise {
                kind: "dependency".to_string(),
                name: "callee".to_string(),
                matched_from: None,
                grade: SealGrade::G0,
            }]
        );
        assert_eq!(
            caller_only
                .claims
                .iter()
                .find(|claim| claim.atom == "callee")
                .unwrap()
                .effective_grade,
            SealGrade::G0
        );

        let mut both_supported = create_book();
        attach_observations(
            &mut both_supported,
            &[
                observation_result("caller", "consistent"),
                observation_result("callee", "consistent"),
            ],
        );
        for claim in &both_supported.claims {
            assert_eq!(claim.grade, SealGrade::G2, "{}", claim.atom);
            assert_eq!(claim.effective_grade, SealGrade::G2, "{}", claim.atom);
            assert!(claim.effective_grade_complete, "{}", claim.atom);
        }

        let mut method_caller = source_atom("method_caller");
        method_caller.body_expr = "get(x)".to_string();
        let point_get = source_atom("Point::get");
        let mut method_book = create_claimbook(
            "sample.mm",
            "sample.proof.json",
            None,
            &[&method_caller, &point_get],
            false,
            None,
        );
        attach_observations(
            &mut method_book,
            &[observation_result("method_caller", "consistent")],
        );
        let method_caller_claim = method_book
            .claims
            .iter()
            .find(|claim| claim.atom == "method_caller")
            .unwrap();
        assert_eq!(method_caller_claim.dependencies, vec!["Point::get"]);
        assert!(!method_caller_claim.effective_grade_complete);
        assert!(method_book
            .recomputation
            .method_name_matches
            .contains_key("method_caller"));
    }

    #[test]
    fn supported_observations_replace_stale_g1_and_g4_but_not_fresh_g4() {
        let mut drifted_g4 = claim("drifted_g4", SealGrade::G4, Freshness::Drifted, &[], &[]);
        drifted_g4.evidence_kind = "z3".to_string();
        drifted_g4.reason = "source content differs from its certificate".to_string();
        let mut book = claimbook_with_claims(vec![
            drifted_g4,
            claim(
                "uncertified_g1",
                SealGrade::G1,
                Freshness::Uncertified,
                &[],
                &[],
            ),
            claim(
                "contradicted_g4",
                SealGrade::G4,
                Freshness::Drifted,
                &[],
                &[],
            ),
            claim("fresh_g4", SealGrade::G4, Freshness::Fresh, &[], &[]),
        ]);
        attach_observations(
            &mut book,
            &[
                observation_result("drifted_g4", "consistent"),
                observation_result("uncertified_g1", "entailed"),
                observation_result("contradicted_g4", "contradicts_ensures"),
                observation_result("fresh_g4", "consistent"),
            ],
        );

        let drifted_g4 = book
            .claims
            .iter()
            .find(|claim| claim.atom == "drifted_g4")
            .unwrap();
        assert_eq!(drifted_g4.grade, SealGrade::G2);
        assert_eq!(drifted_g4.effective_grade, SealGrade::G2);
        assert_eq!(
            drifted_g4.reason,
            "current test observations agree with the contract (previously G4 z3: source content differs from its certificate)"
        );
        assert!(drifted_g4
            .limiting_premises
            .iter()
            .all(|premise| premise.kind != "freshness"));

        let uncertified_g1 = book
            .claims
            .iter()
            .find(|claim| claim.atom == "uncertified_g1")
            .unwrap();
        assert_eq!(uncertified_g1.grade, SealGrade::G2);
        assert_eq!(
            uncertified_g1.reason,
            "current test observations agree with the contract (previously G1 test: )"
        );

        let contradicted_g4 = book
            .claims
            .iter()
            .find(|claim| claim.atom == "contradicted_g4")
            .unwrap();
        assert_eq!(contradicted_g4.grade, SealGrade::G4);
        assert_eq!(contradicted_g4.effective_grade, SealGrade::G0);
        assert_eq!(
            contradicted_g4.limiting_premises,
            vec![LimitingPremise {
                kind: "freshness".to_string(),
                name: "drifted".to_string(),
                matched_from: None,
                grade: SealGrade::G0,
            }]
        );

        let fresh_g4 = book
            .claims
            .iter()
            .find(|claim| claim.atom == "fresh_g4")
            .unwrap();
        assert_eq!(fresh_g4.grade, SealGrade::G4);
        assert_eq!(fresh_g4.effective_grade, SealGrade::G4);
    }

    #[test]
    fn observations_do_not_lift_unsatisfiable_contradicted_or_inconclusive_g0_or_g1_g4() {
        let source = source_atom("sample");
        let results = std::collections::HashMap::from([(
            source.name.clone(),
            (status::Z3_UNSAT.to_string(), status::VERIFIED.to_string()),
        )]);
        let mut certificate = proof_cert::generate_certificate(
            "sample.mm",
            &[&source],
            &results,
            &verification::ModuleEnv::new(),
            None,
            None,
            None,
        );
        set_spec_satisfiable(&mut certificate.atoms[0], false);
        proof_cert::refresh_certificate_integrity(&mut certificate);
        let mut fresh_unsatisfiable_book = create_claimbook(
            "sample.mm",
            "sample.proof.json",
            Some(&certificate),
            &[&source],
            false,
            None,
        );
        attach_observations(
            &mut fresh_unsatisfiable_book,
            &[observation_result("sample", "consistent")],
        );
        assert_eq!(
            fresh_unsatisfiable_book.claims[0].freshness,
            Freshness::Fresh
        );
        assert_eq!(fresh_unsatisfiable_book.claims[0].grade, SealGrade::G0);
        assert!(fresh_unsatisfiable_book
            .recomputation
            .unsatisfiable_specs
            .contains("sample"));

        let mut drifted_source = source_atom("sample");
        drifted_source.body_expr = "1".to_string();
        let mut drifted_unsatisfiable_book = create_claimbook(
            "sample.mm",
            "sample.proof.json",
            Some(&certificate),
            &[&drifted_source],
            false,
            None,
        );
        assert_eq!(
            drifted_unsatisfiable_book.claims[0].freshness,
            Freshness::Drifted
        );
        attach_observations(
            &mut drifted_unsatisfiable_book,
            &[observation_result("sample", "consistent")],
        );
        assert_eq!(drifted_unsatisfiable_book.claims[0].grade, SealGrade::G2);

        let mut book = claimbook_with_claims(vec![
            claim(
                "contradicted",
                SealGrade::G0,
                Freshness::Uncertified,
                &[],
                &[],
            ),
            claim(
                "inconclusive",
                SealGrade::G0,
                Freshness::Uncertified,
                &[],
                &[],
            ),
            claim("trusted", SealGrade::G1, Freshness::Fresh, &[], &[]),
            claim("machine", SealGrade::G4, Freshness::Fresh, &[], &[]),
        ]);
        attach_observations(
            &mut book,
            &[
                observation_result("contradicted", "not_entailed"),
                observation_result("inconclusive", "not_checked"),
                observation_result("trusted", "consistent"),
                observation_result("machine", "contradicts_ensures"),
            ],
        );
        assert_eq!(
            book.claims
                .iter()
                .find(|claim| claim.atom == "contradicted")
                .unwrap()
                .grade,
            SealGrade::G0
        );
        assert_eq!(
            book.claims
                .iter()
                .find(|claim| claim.atom == "inconclusive")
                .unwrap()
                .grade,
            SealGrade::G0
        );
        assert_eq!(
            book.claims
                .iter()
                .find(|claim| claim.atom == "trusted")
                .unwrap()
                .grade,
            SealGrade::G1
        );
        assert_eq!(
            book.claims
                .iter()
                .find(|claim| claim.atom == "machine")
                .unwrap()
                .grade,
            SealGrade::G4
        );
    }

    #[test]
    fn observed_g2_assumptions_cap_at_g1_and_g2_dependencies_propagate() {
        let mut book = claimbook_with_claims(vec![
            claim(
                "assumed",
                SealGrade::G2,
                Freshness::Uncertified,
                &[],
                &["requires: x > 0"],
            ),
            claim("caller", SealGrade::G4, Freshness::Fresh, &["callee"], &[]),
            claim("callee", SealGrade::G0, Freshness::Uncertified, &[], &[]),
        ]);
        attach_observations(
            &mut book,
            &[
                observation_result("caller", "consistent"),
                observation_result("callee", "consistent"),
            ],
        );

        let assumed = book
            .claims
            .iter()
            .find(|claim| claim.atom == "assumed")
            .unwrap();
        assert_eq!(assumed.effective_grade, SealGrade::G1);
        assert_eq!(
            assumed.limiting_premises,
            vec![LimitingPremise {
                kind: "assumed_clause".to_string(),
                name: "x > 0".to_string(),
                matched_from: None,
                grade: SealGrade::G1,
            }]
        );

        let caller = book
            .claims
            .iter()
            .find(|claim| claim.atom == "caller")
            .unwrap();
        assert_eq!(caller.effective_grade, SealGrade::G2);
        assert_eq!(
            caller.limiting_premises,
            vec![LimitingPremise {
                kind: "dependency".to_string(),
                name: "callee".to_string(),
                matched_from: None,
                grade: SealGrade::G2,
            }]
        );
    }

    #[test]
    fn observation_summary_and_json_order_are_deterministic() {
        let mut book = claimbook_with_claims(vec![
            claim("alpha", SealGrade::G0, Freshness::Uncertified, &[], &[]),
            claim(
                "Point::get",
                SealGrade::G0,
                Freshness::Uncertified,
                &[],
                &[],
            ),
            claim("zeta", SealGrade::G0, Freshness::Uncertified, &[], &[]),
        ]);
        attach_observations(
            &mut book,
            &[
                observation_result("alpha", "consistent"),
                observation_result("Point::get", "contradicts_ensures"),
            ],
        );
        assert_eq!(
            book.summary.by_observation,
            Some(BTreeMap::from([
                ("contradicted".to_string(), 1),
                ("not_observed".to_string(), 1),
                ("supported".to_string(), 1),
            ]))
        );
        let markdown = render_markdown(&book);
        assert!(markdown.contains("| Observation | Count |"));

        let first = serde_json::to_string_pretty(&book).unwrap();
        let second = serde_json::to_string_pretty(&book).unwrap();
        assert_eq!(first, second);
        let contradicted = first.find("\"contradicted\"").unwrap();
        let not_observed = first.find("\"not_observed\"").unwrap();
        let supported = first.find("\"supported\"").unwrap();
        assert!(contradicted < not_observed && not_observed < supported);
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
    fn z3_dual_check_matrix_requires_current_explicitly_audited_lean_evidence_and_opt_in() {
        use crate::proof_cert::{lean_dual_check_status, LeanDualCheck, LeanResultMetadata};

        let cases = [
            ("absent", None, LeanDualCheck::Absent),
            ("passed", Some("passed"), LeanDualCheck::Passed),
            (
                "stale translator",
                Some("stale_translator"),
                LeanDualCheck::Stale,
            ),
            ("stale bridge", Some("stale_bridge"), LeanDualCheck::Stale),
            (
                "wrong status",
                Some("wrong_status"),
                LeanDualCheck::NotVerified,
            ),
            (
                "empty theorem",
                Some("empty_theorem"),
                LeanDualCheck::NotVerified,
            ),
            ("rejected", Some("rejected"), LeanDualCheck::AxiomRejected),
            ("error", Some("error"), LeanDualCheck::AxiomRejected),
            ("unaudited", Some("unaudited"), LeanDualCheck::AxiomRejected),
        ];
        for (name, mutation, expected_status) in cases {
            let mut atom = atom_certificate(status::Z3_UNSAT, status::VERIFIED);
            if let Some(mutation) = mutation {
                let mut metadata = LeanResultMetadata {
                    status: status::LEAN_STATUS_VERIFIED.to_string(),
                    theorem_name: "sample_correct".to_string(),
                    translator_version: verification::LEAN_TRANSLATOR_VERSION.to_string(),
                    bridge_lemma_hash: verification::LEAN_BRIDGE_LEMMA_HASH.to_string(),
                    kernel_axioms: Some(vec!["propext".to_string()]),
                    axiom_audit: Some("passed".to_string()),
                    ..Default::default()
                };
                match mutation {
                    "stale_translator" => metadata.translator_version = "old".to_string(),
                    "stale_bridge" => metadata.bridge_lemma_hash = "old".to_string(),
                    "wrong_status" => metadata.status = "error".to_string(),
                    "empty_theorem" => metadata.theorem_name.clear(),
                    "rejected" => metadata.axiom_audit = Some("rejected".to_string()),
                    "error" => metadata.axiom_audit = Some("error".to_string()),
                    "unaudited" => {
                        metadata.axiom_audit = None;
                        metadata.kernel_axioms = None;
                    }
                    "passed" => {}
                    other => panic!("unknown dual-check case: {other}"),
                }
                atom.lean_dual_check = Some(metadata);
            }
            assert_eq!(lean_dual_check_status(&atom), expected_status, "{name}");
            for allow in [false, true] {
                let (grade, evidence, reason) = own_grade(&atom, &TrustLevel::Verified, allow);
                assert_eq!(
                    grade,
                    if expected_status == LeanDualCheck::Passed && allow {
                        SealGrade::G5
                    } else {
                        SealGrade::G4
                    },
                    "{name}, allow={allow}"
                );
                assert_eq!(
                    evidence,
                    if expected_status == LeanDualCheck::Passed && allow {
                        "z3+lean"
                    } else {
                        "z3"
                    },
                    "{name}, allow={allow}"
                );
                if atom.lean_dual_check.is_some() && !allow {
                    assert!(reason.contains("--allow-lean-verified"), "{reason}");
                } else if atom.lean_dual_check.is_some() && expected_status != LeanDualCheck::Passed
                {
                    assert!(reason.contains("Lean dual check not counted"), "{reason}");
                }
            }
        }
    }

    #[test]
    fn dual_check_does_not_upgrade_lean_only_sat_unknown_or_trusted_atoms() {
        let mut dual_metadata = crate::proof_cert::LeanResultMetadata {
            status: status::LEAN_STATUS_VERIFIED.to_string(),
            theorem_name: "sample_correct".to_string(),
            translator_version: verification::LEAN_TRANSLATOR_VERSION.to_string(),
            bridge_lemma_hash: verification::LEAN_BRIDGE_LEMMA_HASH.to_string(),
            kernel_axioms: Some(vec!["propext".to_string()]),
            axiom_audit: Some("passed".to_string()),
            ..Default::default()
        };
        dual_metadata.proof_path = "Generated/Sample.lean".to_string();

        let mut lean_only = atom_certificate(status::Z3_LEAN_VERIFIED, status::VERIFIED);
        lean_only.translator_version = verification::LEAN_TRANSLATOR_VERSION.to_string();
        lean_only.bridge_lemma_hash = verification::LEAN_BRIDGE_LEMMA_HASH.to_string();
        lean_only.lean_result_metadata = Some(dual_metadata.clone());
        lean_only.lean_dual_check = Some(dual_metadata.clone());
        assert_eq!(
            own_grade(&lean_only, &TrustLevel::Verified, true).0,
            SealGrade::G4
        );

        for result in [status::Z3_SAT, status::Z3_UNKNOWN] {
            let mut atom = atom_certificate(result, status::VERIFIED);
            atom.lean_dual_check = Some(dual_metadata.clone());
            assert_eq!(
                own_grade(&atom, &TrustLevel::Verified, true).0,
                SealGrade::G0
            );
        }

        let mut trusted = atom_certificate(status::Z3_UNSAT, status::VERIFIED);
        trusted.lean_dual_check = Some(dual_metadata);
        assert_eq!(
            own_grade(&trusted, &TrustLevel::Trusted, true).0,
            SealGrade::G1
        );
    }

    #[test]
    fn effective_g5_is_capped_by_dependencies_assumptions_and_freshness() {
        let claims = propagated_claims(vec![
            claim("callee", SealGrade::G5, Freshness::Fresh, &[], &[]),
            claim("caller", SealGrade::G4, Freshness::Fresh, &["callee"], &[]),
        ]);
        assert_eq!(claims["caller"].effective_grade, SealGrade::G4);

        let claims = propagated_claims(vec![claim(
            "assumed",
            SealGrade::G5,
            Freshness::Fresh,
            &[],
            &["x > 0"],
        )]);
        assert_eq!(claims["assumed"].effective_grade, SealGrade::G1);

        let claims = propagated_claims(vec![claim(
            "drifted",
            SealGrade::G5,
            Freshness::Drifted,
            &[],
            &[],
        )]);
        assert_eq!(claims["drifted"].effective_grade, SealGrade::G0);

        let claims = propagated_claims(vec![
            claim("callee", SealGrade::G5, Freshness::Fresh, &[], &[]),
            claim("caller", SealGrade::G5, Freshness::Fresh, &["callee"], &[]),
        ]);
        assert_eq!(claims["caller"].effective_grade, SealGrade::G5);
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
            (status::CERTIFICATE_DEPENDENCY_CHANGED, Freshness::Drifted),
            (status::CERTIFICATE_STALE_TRANSLATOR, Freshness::Drifted),
            (status::CERTIFICATE_PROVEN, Freshness::Fresh),
            (status::CERTIFICATE_TRUSTED, Freshness::Fresh),
            (status::CERTIFICATE_UNPROVEN, Freshness::Fresh),
            (status::CERTIFICATE_DEPENDENCY_UNCHECKED, Freshness::Fresh),
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

        let without_opt_in = create_claimbook(
            "sample.mm",
            "sample.proof.json",
            Some(&cert),
            &atoms,
            false,
            None,
        );
        let without_claim = &without_opt_in.claims[0];
        assert_eq!(without_claim.freshness, Freshness::Drifted);
        assert_eq!(
            without_claim.reason,
            "Lean translator metadata is stale; regenerate the Lean certificate"
        );

        let with_opt_in = create_claimbook(
            "sample.mm",
            "sample.proof.json",
            Some(&cert),
            &atoms,
            true,
            None,
        );
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
            None,
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
                None,
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
                None,
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
        let book = create_claimbook("sample.mm", "sample.proof.json", None, &atoms, false, None);
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

        let report = create_claimbook(
            "sample.mm",
            "sample.proof.json",
            Some(&cert),
            &atoms,
            false,
            None,
        );
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
            None,
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

        let report = create_claimbook(
            "sample.mm",
            "sample.proof.json",
            Some(&cert),
            &atoms,
            false,
            None,
        );
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
                clause: "requires: x > 0".to_string(),
            }]
        );
    }

    #[test]
    fn trusted_base_keeps_requires_and_ensures_kinds_for_same_expression() {
        let mut source = source_atom("assumed");
        source.requires = "x > 0".to_string();
        source.ensures = "x > 0".to_string();
        source.clause_modes = vec![
            crate::parser::ClauseMode {
                kind: crate::parser::ClauseKind::Requires,
                clause: "x > 0".to_string(),
                mode: ClauseTrustMode::Assume,
            },
            crate::parser::ClauseMode {
                kind: crate::parser::ClauseKind::Ensures,
                clause: "x > 0".to_string(),
                mode: ClauseTrustMode::Assume,
            },
        ];
        let mut entry = atom_certificate(status::Z3_UNSAT, status::VERIFIED);
        entry.name = "assumed".to_string();
        entry.assumed_clauses = vec!["requires: x > 0".to_string(), "ensures: x > 0".to_string()];
        let mut cert = proof_certificate();
        cert.atoms = vec![entry];

        let base = trusted_base(Some(&cert), &[&source]);
        assert_eq!(
            base.assumed_clauses,
            [
                TrustedBaseAssumedClause {
                    atom: "assumed".to_string(),
                    clause: "ensures: x > 0".to_string(),
                },
                TrustedBaseAssumedClause {
                    atom: "assumed".to_string(),
                    clause: "requires: x > 0".to_string(),
                },
            ]
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
                    clause: "requires: x > 0".to_string(),
                },
                TrustedBaseAssumedClause {
                    atom: "beta".to_string(),
                    clause: "ensures: result > 0".to_string(),
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
                clause: "ensures: result > 0".to_string(),
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
                by_discrimination: None,
                by_observation: None,
            },
            claims: vec![claim],
            orphaned_entries: Vec::new(),
            recomputation: ClaimBookRecomputation::default(),
        };

        let markdown = render_markdown(&book);
        assert!(markdown.contains("## Trusted base"));
        assert!(markdown.contains("Limiting premises"));
        assert!(markdown.contains("assumed_clause x \\| y (G1)"));
    }

    #[test]
    fn controls_merge_matches_claim_names_and_handles_missing_or_extra_entries() {
        let mut book = ClaimBook {
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
                by_effective_grade: BTreeMap::new(),
                by_freshness: BTreeMap::new(),
                total: 2,
                by_discrimination: None,
                by_observation: None,
            },
            claims: vec![
                claim("Point::get", SealGrade::G4, Freshness::Fresh, &[], &[]),
                claim("missing", SealGrade::G0, Freshness::Uncertified, &[], &[]),
            ],
            orphaned_entries: Vec::new(),
            recomputation: ClaimBookRecomputation::default(),
        };
        let controls = [
            ControlsAtomReport {
                name: "Point::get".to_string(),
                status: "weak",
                detail: "one mutant survived".to_string(),
                tested: 2,
                killed: 1,
                survived: 1,
                unanalyzable: 0,
                mutants: vec![crate::controls::MutantReport {
                    id: "constant:1->0@2".to_string(),
                    operator: "constant".to_string(),
                    site: 2,
                    outcome: "survived",
                    detail: String::new(),
                }],
            },
            ControlsAtomReport {
                name: "monomorphized<i64>".to_string(),
                status: "discriminating",
                detail: "all mutants were killed".to_string(),
                tested: 1,
                killed: 1,
                survived: 0,
                unanalyzable: 0,
                mutants: Vec::new(),
            },
        ];

        attach_controls(&mut book, &controls);

        let method = book
            .claims
            .iter()
            .find(|claim| claim.atom == "Point::get")
            .expect("qualified method claim")
            .discrimination
            .as_ref()
            .expect("matched controls report");
        assert_eq!(method.status, "weak");
        assert_eq!(method.tested, 2);
        assert_eq!(method.surviving, ["constant:1->0@2"]);
        let missing = book
            .claims
            .iter()
            .find(|claim| claim.atom == "missing")
            .expect("missing controls claim")
            .discrimination
            .as_ref()
            .expect("default discrimination");
        assert_eq!(missing.status, "not_checked");
        assert_eq!(missing.detail, "no controls entry");
        assert_eq!(book.claims.len(), 2);
        assert_eq!(
            book.summary.by_discrimination,
            Some(BTreeMap::from([
                ("not_checked".to_string(), 1),
                ("weak".to_string(), 1),
            ]))
        );
    }

    #[test]
    fn discrimination_gate_rejects_weak_and_inconclusive_only() {
        let discrimination = |status: &str| ClaimDiscrimination {
            status: status.to_string(),
            detail: String::new(),
            tested: 0,
            killed: 0,
            survived: 0,
            unanalyzable: 0,
            surviving: Vec::new(),
        };

        assert!(fails_discrimination_gate(&discrimination("weak")));
        assert!(fails_discrimination_gate(&discrimination("inconclusive")));
        for status in ["discriminating", "no_mutants", "not_checked"] {
            assert!(!fails_discrimination_gate(&discrimination(status)));
        }
    }
}
