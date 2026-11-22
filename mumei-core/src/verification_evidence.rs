//! Common v1 representation, not proof acceptance, freshness or artifact assurance.
//!
//! `decode` checks both closed JSON shape and the v1 combination rules. Serde
//! deserialization alone checks shape; constructed/deserialized values must call
//! `Evidence::validate` before use. Neither API follows paths, executes tools,
//! checks a claim's truth or changes certificate/grade/Lean policy.

use serde::{Deserialize, Deserializer, Serialize};
use std::{collections::HashSet, fmt};

pub const SCHEMA_VERSION: &str = "mumei.verification-evidence/v1";
pub const LIST_SCHEMA_VERSION: &str = "mumei.verification-evidence-list/v1";
pub const MAX_INPUT_BYTES: usize = 65_536;
/// Bound on records admitted by one list envelope — the same bounded-input
/// posture as the single-record gate, not an unbounded report.
pub const MAX_EVIDENCE_RECORDS: usize = 64;

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub schema_version: String,
    pub subject: Subject,
    pub specification: Identity,
    pub obligation_id: String,
    pub semantics: Semantics,
    pub method: String,
    pub execution: String,
    pub result: String,
    pub scope: Scope,
    pub correspondence: Correspondence,
    pub assumptions: Vec<String>,
    pub exclusions: Vec<String>,
    pub provenance: Provenance,
    pub artifact_paths: Vec<String>,
    /// Declared production timestamp — present only when the producer
    /// was given one (`evidence verify-source --produced-at`); the
    /// record stays byte-identical when absent.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "required_when_present"
    )]
    pub produced_at: Option<String>,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Subject {
    pub id: String,
    pub kind: String,
    pub sha256: String,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub id: String,
    pub sha256: String,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Semantics {
    pub profile: String,
    pub integer_model: String,
    #[serde(deserialize_with = "required_nullable_string")]
    pub target_triple: Option<String>,
}

fn required_nullable_string<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub kind: String,
    pub bounds: Vec<String>,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Correspondence {
    pub relation: String,
    pub status: String,
    pub notes: Vec<String>,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub adapter: Adapter,
    pub tool: Tool,
    pub configuration_fingerprint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub translator_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bridge_lemma_hash: Option<String>,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Adapter {
    pub id: String,
    pub version: String,
    pub protocol_version: String,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Tool {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepresentationError {
    InputTooLarge,
    InvalidShape,
    InvalidValue,
    InvalidDigest,
    InvalidList,
    InconsistentOutcome,
    InconsistentScope,
    InconsistentCorrespondence,
}

impl fmt::Display for RepresentationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InputTooLarge => "verification evidence input exceeds size limit",
            Self::InvalidShape => "invalid verification evidence JSON shape",
            Self::InvalidValue => "invalid verification evidence field value",
            Self::InvalidDigest => "invalid verification evidence digest",
            Self::InvalidList => "invalid verification evidence list",
            Self::InconsistentOutcome => "inconsistent verification evidence outcome",
            Self::InconsistentScope => "inconsistent verification evidence scope",
            Self::InconsistentCorrespondence => "inconsistent verification evidence correspondence",
        })
    }
}

impl std::error::Error for RepresentationError {}

impl fmt::Debug for Evidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Evidence").finish_non_exhaustive()
    }
}

/// Strict RFC 3339 timestamp: `YYYY-MM-DDThh:mm:ss[.f+](Z|±hh:mm)` with
/// ASCII digits only — the same closed shape the consumer-side
/// `freshness-eval` fold admits. Value ranges are checked at field
/// granularity (month 01-12, day 01-31, hour 00-23, minute 00-59,
/// second 00-60 for leap seconds); day-per-month is not modeled.
pub fn is_rfc3339_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    let digit = |index: usize| bytes.get(index).is_some_and(|byte| byte.is_ascii_digit());
    let pair = |index: usize| digit(index) && digit(index + 1);
    let number =
        |index: usize| (bytes[index] - b'0') as u32 * 10 + (bytes[index + 1] - b'0') as u32;
    if bytes.len() < 20
        || !(0..4).all(digit)
        || bytes[4] != b'-'
        || !pair(5)
        || bytes[7] != b'-'
        || !pair(8)
        || !matches!(bytes[10], b'T' | b't')
        || !pair(11)
        || bytes[13] != b':'
        || !pair(14)
        || bytes[16] != b':'
        || !pair(17)
    {
        return false;
    }
    let (month, day, hour, minute, second) =
        (number(5), number(8), number(11), number(14), number(17));
    if month == 0 || month > 12 || day == 0 || day > 31 || hour > 23 || minute > 59 || second > 60 {
        return false;
    }
    let mut index = 19;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let start = index;
        while bytes.get(index).is_some_and(|byte| byte.is_ascii_digit()) {
            index += 1;
        }
        if index == start {
            return false;
        }
    }
    match bytes.get(index) {
        Some(b'Z') | Some(b'z') => index + 1 == bytes.len(),
        Some(b'+') | Some(b'-') => {
            index + 6 == bytes.len()
                && pair(index + 1)
                && bytes[index + 3] == b':'
                && pair(index + 4)
                && number(index + 1) <= 23
                && number(index + 4) <= 59
        }
        _ => false,
    }
}

/// Parse a strict RFC 3339 timestamp into conservative nanosecond bounds.
///
/// The low bound truncates fractional precision after nine digits. If any
/// discarded digit is nonzero, the high bound is one nanosecond above low.
/// Calendar dates and leap seconds are validated more strictly than the
/// representation-level timestamp predicate.
pub fn rfc3339_instant(value: &str) -> Option<(i128, i128)> {
    if !is_rfc3339_timestamp(value) {
        return None;
    }

    let bytes = value.as_bytes();
    let year = decimal_pair_or_quad(bytes, 0, 4)? as i64;
    let month = decimal_pair_or_quad(bytes, 5, 2)?;
    let day = decimal_pair_or_quad(bytes, 8, 2)?;
    let hour = decimal_pair_or_quad(bytes, 11, 2)?;
    let minute = decimal_pair_or_quad(bytes, 14, 2)?;
    let second = decimal_pair_or_quad(bytes, 17, 2)?;
    if year == 0 || second == 60 {
        return None;
    }

    let leap_year = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap_year => 29,
        2 => 28,
        _ => return None,
    };
    if day == 0 || day > days_in_month {
        return None;
    }

    let mut index = 19;
    let fraction = if bytes.get(index) == Some(&b'.') {
        let start = index + 1;
        index = start;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        &bytes[start..index]
    } else {
        &[]
    };

    let offset_seconds = match bytes.get(index).copied()? {
        b'Z' | b'z' => 0i128,
        sign @ (b'+' | b'-') => {
            let offset_hour = decimal_pair_or_quad(bytes, index + 1, 2)? as i128;
            let offset_minute = decimal_pair_or_quad(bytes, index + 4, 2)? as i128;
            let seconds = offset_hour * 3_600 + offset_minute * 60;
            if sign == b'+' {
                seconds
            } else {
                -seconds
            }
        }
        _ => return None,
    };

    let mut nanoseconds = 0i128;
    for byte in fraction.iter().take(9) {
        nanoseconds = nanoseconds * 10 + i128::from(*byte - b'0');
    }
    for _ in fraction.len().min(9)..9 {
        nanoseconds *= 10;
    }

    let days = days_from_civil(year, month as i64, day as i64);
    let local_seconds =
        days * 86_400 + i128::from(hour) * 3_600 + i128::from(minute) * 60 + i128::from(second);
    let low = (local_seconds - offset_seconds) * 1_000_000_000 + nanoseconds;
    let uncertain_tail = fraction.iter().skip(9).any(|byte| *byte != b'0');
    let high = low + if uncertain_tail { 1 } else { 0 };
    Some((low, high))
}

fn decimal_pair_or_quad(bytes: &[u8], start: usize, width: usize) -> Option<u32> {
    bytes
        .get(start..start + width)?
        .iter()
        .try_fold(0u32, |value, byte| {
            if byte.is_ascii_digit() {
                Some(value * 10 + u32::from(*byte - b'0'))
            } else {
                None
            }
        })
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i128 {
    let adjusted_year = year - if month <= 2 { 1 } else { 0 };
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year - era * 400;
    let adjusted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * adjusted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    i128::from(era * 146_097 + day_of_era - 719_468)
}

/// An explicit `null` is not "absent": the standalone schema admits
/// `produced_at` only as a string, so a null must fail shape here too
/// instead of decoding to `None`.
fn required_when_present<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)?
        .ok_or_else(|| serde::de::Error::custom("produced_at must be a string"))
        .map(Some)
}

/// Records are JSON objects only: serde's derived struct shape would also
/// admit a field-ordered JSON sequence, which the closed-shape contract
/// does not.
fn is_json_object(bytes: &[u8]) -> bool {
    bytes.iter().find(|byte| !byte.is_ascii_whitespace()) == Some(&b'{')
}

pub fn decode(bytes: &[u8]) -> Result<Evidence, RepresentationError> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(RepresentationError::InputTooLarge);
    }
    if !is_json_object(bytes) {
        return Err(RepresentationError::InvalidShape);
    }
    let evidence: Evidence =
        serde_json::from_slice(bytes).map_err(|_| RepresentationError::InvalidShape)?;
    evidence.validate()?;
    Ok(evidence)
}

/// Closed list-envelope shape; only `schema_version` and `evidence` may
/// appear, and the schema must name the list envelope — elements keep their
/// raw bytes so the inner gate sees exactly what the input carried.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceList {
    schema_version: String,
    evidence: Vec<Box<serde_json::value::RawValue>>,
}

/// Admits a bounded `mumei.verification-evidence-list/v1` envelope: every
/// element's raw bytes go through the single-record `decode`, so
/// duplicate-key, unknown-field and combination checks apply inside each
/// record exactly as they do standalone — a `Value` element would have
/// already collapsed duplicate keys before the inner gate could see them.
/// Representation only — no cross-record consistency, truth or acceptance
/// is implied.
pub fn decode_list(bytes: &[u8]) -> Result<Vec<Evidence>, RepresentationError> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(RepresentationError::InputTooLarge);
    }
    if !is_json_object(bytes) {
        return Err(RepresentationError::InvalidShape);
    }
    let envelope: EvidenceList =
        serde_json::from_slice(bytes).map_err(|_| RepresentationError::InvalidShape)?;
    if envelope.schema_version != LIST_SCHEMA_VERSION
        || envelope.evidence.len() > MAX_EVIDENCE_RECORDS
    {
        return Err(RepresentationError::InvalidList);
    }
    envelope
        .evidence
        .iter()
        .map(|element| decode(element.get().as_bytes()))
        .collect()
}

/// One syntactic disagreement between two records that claim the same
/// identity pin: the record indices inside the list and the field whose
/// values differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ConsistencyConflict {
    pub index_a: usize,
    pub index_b: usize,
    pub field: &'static str,
}

/// Cap on reported conflicts — the diagnostic stays bounded like every
/// other surface. `ConsistencyReport::conflict_count` keeps the true total
/// even when this cap truncates the list.
pub const MAX_CONSISTENCY_CONFLICTS: usize = 64;

/// Cross-record agreement on declared identity pins: records claiming the
/// same subject — `subject.id` **and** `subject.sha256` together, because
/// the id alone is a producer label (every standalone source record uses
/// `"source.mm"`) — must agree on `subject.kind`; records naming the same
/// `obligation_id` must pin the same subject, specification, semantics and
/// method. Outcomes (`result`, `execution`, `scope`, `correspondence`) are
/// claims, not identity — a fresh record may legitimately disagree with an
/// older one there, so they are never compared. Records sharing no pin are
/// independent, not conflicting.
#[derive(Debug, PartialEq, Eq)]
pub struct ConsistencyReport {
    /// No pair disagreed on a shared identity pin.
    pub consistent: bool,
    /// Total disagreements found — may exceed `conflicts.len()`.
    pub conflict_count: usize,
    /// The first [`MAX_CONSISTENCY_CONFLICTS`] disagreements.
    pub conflicts: Vec<ConsistencyConflict>,
}

/// Syntactic cross-record agreement on declared identity pins. Structural
/// only: it does not check that any pin is correct, current or authentic.
pub fn consistency_check(records: &[Evidence]) -> ConsistencyReport {
    let mut conflict_count = 0usize;
    let mut conflicts = Vec::new();
    for (index_a, a) in records.iter().enumerate() {
        for (index_b, b) in records.iter().enumerate().skip(index_a + 1) {
            if a.subject.id == b.subject.id
                && a.subject.sha256 == b.subject.sha256
                && a.subject.kind != b.subject.kind
            {
                conflict_count += 1;
                if conflicts.len() < MAX_CONSISTENCY_CONFLICTS {
                    conflicts.push(ConsistencyConflict {
                        index_a,
                        index_b,
                        field: "subject.kind",
                    });
                }
            }
            if a.obligation_id == b.obligation_id {
                for (field, equal) in [
                    ("subject.id", a.subject.id == b.subject.id),
                    ("subject.sha256", a.subject.sha256 == b.subject.sha256),
                    ("specification.id", a.specification.id == b.specification.id),
                    (
                        "specification.sha256",
                        a.specification.sha256 == b.specification.sha256,
                    ),
                    (
                        "semantics.profile",
                        a.semantics.profile == b.semantics.profile,
                    ),
                    (
                        "semantics.integer_model",
                        a.semantics.integer_model == b.semantics.integer_model,
                    ),
                    (
                        "semantics.target_triple",
                        a.semantics.target_triple == b.semantics.target_triple,
                    ),
                    ("method", a.method == b.method),
                ] {
                    if !equal {
                        conflict_count += 1;
                        if conflicts.len() < MAX_CONSISTENCY_CONFLICTS {
                            conflicts.push(ConsistencyConflict {
                                index_a,
                                index_b,
                                field,
                            });
                        }
                    }
                }
            }
        }
    }
    ConsistencyReport {
        consistent: conflict_count == 0,
        conflict_count,
        conflicts,
    }
}

impl Evidence {
    /// Checks the standalone v1 schema rules, not the meaning or truth of a claim.
    pub fn validate(&self) -> Result<(), RepresentationError> {
        use RepresentationError::*;
        if self.schema_version != SCHEMA_VERSION
            || ![
                "mumei_source",
                "foreign_source",
                "model",
                "lowered_ir",
                "native_artifact",
                "wasm_artifact",
            ]
            .contains(&self.subject.kind.as_str())
            || ![
                "mathematical_integer",
                "checked_i64",
                "wrapping_i64",
                "not_applicable",
            ]
            .contains(&self.semantics.integer_model.as_str())
            || ![
                "completed",
                "timeout",
                "resource_limit",
                "tool_error",
                "not_run",
            ]
            .contains(&self.execution.as_str())
            || !["unbounded", "bounded", "finite", "identity"].contains(&self.scope.kind.as_str())
            || ![
                "exact_model",
                "property_preserving",
                "over_approximation",
                "under_approximation",
                "unchecked",
                "unsupported",
            ]
            .contains(&self.correspondence.relation.as_str())
            || !["validated", "assumed", "unverified", "unsupported"]
                .contains(&self.correspondence.status.as_str())
        {
            return Err(InvalidValue);
        }
        let text = [
            self.subject.id.as_str(),
            self.specification.id.as_str(),
            self.obligation_id.as_str(),
            self.semantics.profile.as_str(),
            self.provenance.adapter.id.as_str(),
            self.provenance.adapter.version.as_str(),
            self.provenance.adapter.protocol_version.as_str(),
            self.provenance.tool.name.as_str(),
            self.provenance.tool.version.as_str(),
        ];
        if text.iter().any(|value| value.is_empty())
            || self
                .semantics
                .target_triple
                .as_ref()
                .is_some_and(|value| value.is_empty())
            || self
                .provenance
                .translator_version
                .as_ref()
                .is_some_and(|value| value.is_empty())
        {
            return Err(InvalidValue);
        }
        let digest = |value: &str| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        };
        if !digest(&self.subject.sha256)
            || !digest(&self.specification.sha256)
            || !digest(&self.provenance.configuration_fingerprint)
            || self
                .provenance
                .bridge_lemma_hash
                .as_ref()
                .is_some_and(|value| !digest(value))
        {
            return Err(InvalidDigest);
        }
        for values in [
            &self.scope.bounds,
            &self.correspondence.notes,
            &self.assumptions,
            &self.exclusions,
            &self.artifact_paths,
        ] {
            let mut seen = HashSet::new();
            if values
                .iter()
                .any(|value| value.is_empty() || !seen.insert(value.as_str()))
            {
                return Err(InvalidList);
            }
        }
        let (scopes, results): (&[&str], &[&str]) = match self.method.as_str() {
            "deductive_proof" => (
                &["unbounded", "bounded"],
                &["established", "counterexample"],
            ),
            "abstract_interpretation" => (&["unbounded", "bounded"], &["established", "finding"]),
            "bounded_model_checking" => (&["bounded"], &["established", "counterexample"]),
            "finite_execution" => (&["finite"], &["observed_no_failure", "counterexample"]),
            "static_heuristic" => (&["unbounded", "bounded"], &["finding"]),
            "identity_check" => (&["identity"], &["established"]),
            _ => return Err(InvalidValue),
        };
        if !scopes.contains(&self.scope.kind.as_str())
            || matches!(self.scope.kind.as_str(), "bounded" | "finite")
                == self.scope.bounds.is_empty()
        {
            return Err(InconsistentScope);
        }
        if self
            .produced_at
            .as_ref()
            .is_some_and(|value| !is_rfc3339_timestamp(value))
        {
            return Err(InvalidValue);
        }
        if !results.contains(&self.result.as_str())
            && !["unknown", "unsupported", "not_checked", "inconsistent"]
                .contains(&self.result.as_str())
            || self.execution != "completed"
                && !["unknown", "unsupported", "not_checked"].contains(&self.result.as_str())
        {
            return Err(InconsistentOutcome);
        }
        let relation = self.correspondence.relation.as_str();
        let status = self.correspondence.status.as_str();
        if relation == "unchecked" && status != "unverified"
            || (relation == "unsupported") != (status == "unsupported")
            || relation == "unsupported"
                && !["unknown", "unsupported", "not_checked"].contains(&self.result.as_str())
        {
            return Err(InconsistentCorrespondence);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
