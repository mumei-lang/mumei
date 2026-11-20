//! Declared-requirement acceptance over common Evidence v1 records.
//!
//! `mumei evidence accept` normalizes one declared requirement document
//! (`mumei.evidence-requirement/v1`) against one decoded
//! `mumei.verification-evidence/v1` record and emits a bounded verdict —
//! `accepted`, `rejected` or `inconclusive` — with per-check outcomes.
//!
//! This is a *normalization* only: no tool executes, no fresh query runs,
//! and no producer authenticity, freshness, artifact identity or claim
//! truth is established. Per C02, a record whose result is `unknown`,
//! `unsupported`, `not_checked` or `inconsistent`, or whose execution did
//! not complete, can never satisfy an assurance requirement; a
//! `counterexample`/`finding` result or a mismatched pin is an
//! affirmative rejection, not an inconclusive one. Finite execution
//! satisfies only the `observation` floor — it can never answer a
//! proof-level requirement.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fmt;

use crate::verification_evidence::{self, Evidence, MAX_INPUT_BYTES};

/// Schema identifier a requirement document must declare.
pub const REQUIREMENT_SCHEMA: &str = "mumei.evidence-requirement/v1";
/// Schema identifier a bounded requirement-list document must declare.
pub const REQUIREMENT_LIST_SCHEMA: &str = "mumei.evidence-requirement-list/v1";
/// Schema identifier of the emitted acceptance descriptor.
pub const ACCEPTANCE_SCHEMA: &str = "mumei.evidence-acceptance/v1";
/// Cap on requirements in one declared list — the document stays bounded
/// like every other admission surface.
pub const MAX_REQUIREMENTS: usize = 16;

const ASSURANCE_FLOORS: &[&str] = &[
    "unbounded_proof",
    "bounded_proof",
    "bounded_check",
    "observation",
    "identity",
];

const CORRESPONDENCE_STATUSES: &[&str] = &["validated", "assumed", "unverified"];

/// One declared requirement: which record pins must hold and the
/// assurance floor the record's outcome must reach. Strictly shaped —
/// unknown fields, wrong types and out-of-vocabulary values are
/// representation errors, never silently ignored.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requirement {
    pub schema_version: String,
    /// Lowercase SHA-256 hex the record's `subject.sha256` must equal.
    pub subject_sha256: String,
    /// Obligation label the record's `obligation_id` must equal.
    pub obligation_id: String,
    pub semantics: RequiredSemantics,
    /// Weakest assurance the record may carry, one of
    /// `unbounded_proof` | `bounded_proof` | `bounded_check` |
    /// `observation` | `identity`.
    pub minimum_assurance: String,
    /// Correspondence statuses the consumer accepts; `unsupported` is
    /// always a rejection, a listed status passes and any other is
    /// inconclusive.
    pub correspondence_status: Vec<String>,
    /// Assumption labels the record must not carry; any intersection is
    /// an affirmative rejection.
    pub excluded_assumptions: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequiredSemantics {
    pub profile: String,
    pub integer_model: String,
    /// Optional pin on the record's `semantics.target_triple`; `null`
    /// means the triple is not pinned.
    pub target_triple: Option<String>,
}

/// Errors the acceptance boundary can produce; the CLI maps each to a
/// static diagnostic without echoing input content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AcceptanceError {
    InputTooLarge,
    InvalidRequirement,
    InvalidEvidence(verification_evidence::RepresentationError),
}

impl fmt::Display for AcceptanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InputTooLarge => "evidence acceptance input exceeds size limit",
            Self::InvalidRequirement => "invalid evidence requirement representation",
            Self::InvalidEvidence(error) => return error.fmt(f),
        })
    }
}

impl std::error::Error for AcceptanceError {}

fn is_json_object(bytes: &[u8]) -> bool {
    bytes.iter().find(|byte| !byte.is_ascii_whitespace()) == Some(&b'{')
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn unique_nonempty(values: &[String]) -> bool {
    let mut seen = HashSet::new();
    values
        .iter()
        .all(|value| !value.is_empty() && seen.insert(value.as_str()))
}

impl Requirement {
    fn validate(&self) -> Result<(), AcceptanceError> {
        if self.schema_version != REQUIREMENT_SCHEMA
            || !digest(&self.subject_sha256)
            || self.obligation_id.is_empty()
            || self.semantics.profile.is_empty()
            || self.semantics.integer_model.is_empty()
            || self
                .semantics
                .target_triple
                .as_ref()
                .is_some_and(|value| value.is_empty())
            || !ASSURANCE_FLOORS.contains(&self.minimum_assurance.as_str())
            || self.correspondence_status.is_empty()
            || !unique_nonempty(&self.correspondence_status)
            || !self
                .correspondence_status
                .iter()
                .all(|status| CORRESPONDENCE_STATUSES.contains(&status.as_str()))
            || !unique_nonempty(&self.excluded_assumptions)
        {
            return Err(AcceptanceError::InvalidRequirement);
        }
        Ok(())
    }
}

/// Decode one declared requirement: JSON object, closed shape, bounded
/// input, then field-level validation. Representation only.
pub fn decode_requirement(bytes: &[u8]) -> Result<Requirement, AcceptanceError> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(AcceptanceError::InputTooLarge);
    }
    if !is_json_object(bytes) {
        return Err(AcceptanceError::InvalidRequirement);
    }
    let requirement: Requirement =
        serde_json::from_slice(bytes).map_err(|_| AcceptanceError::InvalidRequirement)?;
    requirement.validate()?;
    Ok(requirement)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RequirementList {
    schema_version: String,
    requirements: Vec<Box<serde_json::value::RawValue>>,
}

/// Decode a bounded `mumei.evidence-requirement-list/v1` document: a
/// non-empty list of declared requirements, each element's raw bytes
/// routed through `decode_requirement` so the closed-shape and
/// field-level gates apply exactly as they do standalone. An empty list
/// is a representation error — a vacuous conjunction must never be
/// admitted as "all requirements accepted".
pub fn decode_requirement_list(bytes: &[u8]) -> Result<Vec<Requirement>, AcceptanceError> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(AcceptanceError::InputTooLarge);
    }
    if !is_json_object(bytes) {
        return Err(AcceptanceError::InvalidRequirement);
    }
    let envelope: RequirementList =
        serde_json::from_slice(bytes).map_err(|_| AcceptanceError::InvalidRequirement)?;
    if envelope.schema_version != REQUIREMENT_LIST_SCHEMA
        || envelope.requirements.is_empty()
        || envelope.requirements.len() > MAX_REQUIREMENTS
    {
        return Err(AcceptanceError::InvalidRequirement);
    }
    envelope
        .requirements
        .iter()
        .map(|element| decode_requirement(element.get().as_bytes()))
        .collect()
}

/// One check's outcome: `passed` | `failed` | `indeterminate`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckOutcome {
    pub name: &'static str,
    pub outcome: &'static str,
}

const PASSED: &str = "passed";
const FAILED: &str = "failed";
const INDETERMINATE: &str = "indeterminate";

/// The normalized verdict: `accepted` only when every check passed,
/// `rejected` on any affirmative failure, `inconclusive` when no check
/// failed but some could not be decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Accepted,
    Rejected,
    Inconclusive,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Inconclusive => "inconclusive",
        }
    }
}

/// The full evaluation: the verdict plus every check's outcome in
/// declaration order.
pub struct Evaluation {
    pub verdict: Verdict,
    pub checks: Vec<CheckOutcome>,
}

fn allowed_methods(floor: &str) -> &'static [&'static str] {
    match floor {
        "unbounded_proof" | "bounded_proof" => &["deductive_proof", "abstract_interpretation"],
        "bounded_check" => &[
            "deductive_proof",
            "abstract_interpretation",
            "bounded_model_checking",
        ],
        "observation" => &[
            "deductive_proof",
            "abstract_interpretation",
            "bounded_model_checking",
            "finite_execution",
        ],
        _ => &["identity_check"],
    }
}

fn allowed_scopes(floor: &str) -> &'static [&'static str] {
    match floor {
        "unbounded_proof" => &["unbounded"],
        "bounded_proof" | "bounded_check" => &["unbounded", "bounded"],
        "observation" => &["unbounded", "bounded", "finite"],
        _ => &["identity"],
    }
}

/// Normalize a decoded requirement and record into a verdict with
/// per-check outcomes. Every check is evaluated — a failed pin does not
/// hide a later indeterminate outcome.
pub fn evaluate(requirement: &Requirement, evidence: &Evidence) -> Evaluation {
    let mut checks = Vec::with_capacity(11);
    let pin = |name: &'static str, matched: bool| CheckOutcome {
        name,
        outcome: if matched { PASSED } else { FAILED },
    };
    checks.push(pin(
        "subject_pin",
        evidence.subject.sha256 == requirement.subject_sha256,
    ));
    checks.push(pin(
        "obligation_pin",
        evidence.obligation_id == requirement.obligation_id,
    ));
    checks.push(pin(
        "profile_pin",
        evidence.semantics.profile == requirement.semantics.profile,
    ));
    checks.push(pin(
        "integer_model_pin",
        evidence.semantics.integer_model == requirement.semantics.integer_model,
    ));
    if let Some(triple) = &requirement.semantics.target_triple {
        checks.push(pin(
            "target_triple_pin",
            evidence.semantics.target_triple.as_ref() == Some(triple),
        ));
    }
    checks.push(CheckOutcome {
        name: "execution_completed",
        outcome: if evidence.execution == "completed" {
            PASSED
        } else {
            INDETERMINATE
        },
    });
    checks.push(CheckOutcome {
        name: "result_polarity",
        outcome: match evidence.result.as_str() {
            "established" | "observed_no_failure" => PASSED,
            "counterexample" | "finding" => FAILED,
            _ => INDETERMINATE,
        },
    });
    checks.push(CheckOutcome {
        name: "method_sufficient",
        outcome: if allowed_methods(&requirement.minimum_assurance)
            .contains(&evidence.method.as_str())
        {
            PASSED
        } else {
            FAILED
        },
    });
    checks.push(CheckOutcome {
        name: "scope_sufficient",
        outcome: if allowed_scopes(&requirement.minimum_assurance)
            .contains(&evidence.scope.kind.as_str())
        {
            PASSED
        } else {
            FAILED
        },
    });
    checks.push(CheckOutcome {
        name: "correspondence_status",
        outcome: if requirement
            .correspondence_status
            .contains(&evidence.correspondence.status)
        {
            PASSED
        } else if evidence.correspondence.status == "unsupported" {
            FAILED
        } else {
            INDETERMINATE
        },
    });
    checks.push(CheckOutcome {
        name: "excluded_assumptions",
        outcome: if evidence
            .assumptions
            .iter()
            .any(|assumption| requirement.excluded_assumptions.contains(assumption))
        {
            FAILED
        } else {
            PASSED
        },
    });
    let verdict = if checks.iter().any(|check| check.outcome == FAILED) {
        Verdict::Rejected
    } else if checks.iter().any(|check| check.outcome == INDETERMINATE) {
        Verdict::Inconclusive
    } else {
        Verdict::Accepted
    };
    Evaluation { verdict, checks }
}

/// Schema identifier of the conjunctive requirement-list acceptance
/// descriptor a promotable gate consumes.
pub const REQUIREMENT_LIST_ACCEPTANCE_SCHEMA: &str =
    "mumei.evidence-requirement-list-acceptance/v1";

/// Size bound for the acceptance descriptor a promotable gate reads.
/// The descriptor repeats per-requirement per-record check outcomes, so
/// it is larger than a single input document — bounded at 4 MiB, far
/// above the worst case for 16 requirements × 64 records.
pub const MAX_ACCEPTANCE_DESCRIPTOR_BYTES: usize = 4 * 1024 * 1024;

/// The fields a promotable gate pins on, out of a
/// `mumei.evidence-requirement-list-acceptance/v1` descriptor. Unknown
/// fields are ignored — the pinned surface is the declared schema,
/// verdict, fingerprints and consistency, not the full body.
#[derive(Deserialize)]
struct AcceptanceDescriptor {
    schema_version: String,
    verdict: String,
    requirement_schema_version: String,
    requirement_list_fingerprint: String,
    evidence_schema_version: String,
    #[serde(default)]
    evidence_fingerprint: Option<String>,
    #[serde(default)]
    list_fingerprint: Option<String>,
    consistent: bool,
    conflict_count: u64,
}

/// Fail-closed promotable gate over a declared acceptance triple:
/// `requirement_list_bytes` is a `mumei.evidence-requirement-list/v1`
/// document, `evidence_bytes` the record or record-list it was evaluated
/// against, and `acceptance_bytes` the
/// `mumei.evidence-requirement-list-acceptance/v1` descriptor an
/// evaluator emitted. Every check must hold for the triple to be
/// promotable; the returned strings are the reasons it is not (empty
/// means the gate passes). A gate never recomputes the verdict — it
/// trusts the descriptor only when the descriptor pins exactly these
/// declared inputs.
pub fn promotable_failures(
    requirement_list_bytes: &[u8],
    evidence_bytes: &[u8],
    acceptance_bytes: &[u8],
) -> Vec<String> {
    let mut failures = Vec::new();
    if requirement_list_bytes.len() > MAX_INPUT_BYTES {
        failures.push("requirement list exceeds the declared size bound".to_string());
    } else if decode_requirement_list(requirement_list_bytes).is_err() {
        failures.push(
            "requirement list is not a valid mumei.evidence-requirement-list/v1 declaration"
                .to_string(),
        );
    }
    if evidence_bytes.len() > MAX_INPUT_BYTES {
        failures.push("evidence input exceeds the declared size bound".to_string());
    }
    if acceptance_bytes.len() > MAX_ACCEPTANCE_DESCRIPTOR_BYTES {
        failures.push("acceptance descriptor exceeds the declared size bound".to_string());
        return failures;
    }
    let descriptor: AcceptanceDescriptor = match serde_json::from_slice(acceptance_bytes) {
        Ok(descriptor) => descriptor,
        Err(_) => {
            failures.push(
                "acceptance descriptor is not a valid mumei.evidence-requirement-list-acceptance/v1 document"
                    .to_string(),
            );
            return failures;
        }
    };
    if descriptor.schema_version != REQUIREMENT_LIST_ACCEPTANCE_SCHEMA {
        failures.push("acceptance descriptor declares the wrong schema".to_string());
    }
    if descriptor.requirement_schema_version != REQUIREMENT_LIST_SCHEMA {
        failures.push("acceptance descriptor declares the wrong requirement schema".to_string());
    }
    if descriptor.requirement_list_fingerprint
        != format!("{:x}", Sha256::digest(requirement_list_bytes))
    {
        failures
            .push("acceptance descriptor does not pin the declared requirement list".to_string());
    }
    match (
        &descriptor.evidence_fingerprint,
        &descriptor.list_fingerprint,
    ) {
        (Some(pin), None) => {
            if descriptor.evidence_schema_version != verification_evidence::SCHEMA_VERSION {
                failures
                    .push("acceptance descriptor declares the wrong evidence schema".to_string());
            }
            if *pin != format!("{:x}", Sha256::digest(evidence_bytes)) {
                failures.push(
                    "acceptance descriptor does not pin the declared evidence record".to_string(),
                );
            } else if verification_evidence::decode(evidence_bytes).is_err() {
                failures.push(
                    "evidence input is not a valid mumei.verification-evidence/v1 record"
                        .to_string(),
                );
            }
        }
        (None, Some(pin)) => {
            if descriptor.evidence_schema_version != verification_evidence::LIST_SCHEMA_VERSION {
                failures
                    .push("acceptance descriptor declares the wrong evidence schema".to_string());
            }
            if *pin != format!("{:x}", Sha256::digest(evidence_bytes)) {
                failures.push(
                    "acceptance descriptor does not pin the declared evidence list".to_string(),
                );
            } else if verification_evidence::decode_list(evidence_bytes).is_err() {
                failures.push(
                    "evidence input is not a valid mumei.verification-evidence-list/v1 document"
                        .to_string(),
                );
            }
        }
        _ => failures.push("acceptance descriptor must pin exactly one evidence input".to_string()),
    }
    if !descriptor.consistent || descriptor.conflict_count != 0 {
        failures.push("acceptance descriptor reports unresolved conflicts".to_string());
    }
    if descriptor.verdict != "accepted" {
        failures.push("acceptance descriptor verdict is not accepted".to_string());
    }
    failures
}

#[cfg(test)]
mod tests;
