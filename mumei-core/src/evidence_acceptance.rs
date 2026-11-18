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
use std::collections::HashSet;
use std::fmt;

use crate::verification_evidence::{self, Evidence, MAX_INPUT_BYTES};

/// Schema identifier a requirement document must declare.
pub const REQUIREMENT_SCHEMA: &str = "mumei.evidence-requirement/v1";
/// Schema identifier of the emitted acceptance descriptor.
pub const ACCEPTANCE_SCHEMA: &str = "mumei.evidence-acceptance/v1";

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

#[cfg(test)]
mod tests;
