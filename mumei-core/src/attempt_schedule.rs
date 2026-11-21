//! Static representation gate for agent-produced attempt schedules.
//!
//! A `selected` outcome names only an adapter's declared-success exit; it is
//! not acceptance. `reported_outcome` is the adapter's manifest-declared
//! self-report, never result polarity or a verdict. The record does not attest
//! manifest-declared argv, stdin, or memory bounds. Embedded
//! `evidence_acceptance` is carried, not re-judged. This module performs no
//! adapter execution, claim-truth check, resource enforcement, or grade
//! promotion.

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::Number;
use std::collections::BTreeMap;
use std::fmt;

pub const MAX_INPUT_BYTES: usize = 65_536;
const MAX_ATTEMPTS: u64 = 8;
const SCHEDULE_SCHEMA_VERSION: &str = "mumei.attempt-schedule/v1";
const EVIDENCE_CHECK_SCHEMA_VERSION: &str = "mumei.verifier-evidence-check/v1";
const EVIDENCE_ACCEPTANCE_SCHEMA_VERSIONS: &[&str] = &[
    "mumei.evidence-check-acceptance/v1",
    "mumei.evidence-acceptance/v1",
    "mumei.evidence-requirement-list-acceptance/v1",
];
const TOP_LEVEL_KEYS: &[&str] = &[
    "schema_version",
    "outcome",
    "selected_adapter_id",
    "fallback_permitted",
    "max_attempts",
    "attempts",
    "evidence_check",
    "evidence_acceptance",
];
const ATTEMPT_KEYS: &[&str] = &[
    "ordinal",
    "adapter_id",
    "plan_fingerprint",
    "request_fingerprint",
    "terminal_state",
    "exit_code",
    "reported_outcome",
];
const EVIDENCE_CHECK_KEYS: &[&str] = &[
    "schema_version",
    "request_fingerprint",
    "attempt_number",
    "output_fingerprint",
    "outcome",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepresentationError {
    InputTooLarge,
    InvalidShape,
    InvalidValue,
    InconsistentSchedule,
}

impl fmt::Display for RepresentationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid attempt-schedule representation")
    }
}

impl std::error::Error for RepresentationError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleOutcome {
    Selected,
    Exhausted,
    NotSchedulable,
    FallbackDenied,
    Cancelled,
}

impl ScheduleOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Selected => "selected",
            Self::Exhausted => "exhausted",
            Self::NotSchedulable => "not_schedulable",
            Self::FallbackDenied => "fallback_denied",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Schedule {
    pub outcome: ScheduleOutcome,
    pub attempt_count: usize,
}

#[derive(Debug)]
enum JsonValue {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
}

impl<'de> Deserialize<'de> for JsonValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(JsonValueVisitor)
    }
}

struct JsonValueVisitor;

impl<'de> Visitor<'de> for JsonValueVisitor {
    type Value = JsonValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(JsonValue::Null)
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(JsonValue::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(JsonValue::Number(Number::from(value)))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(JsonValue::Number(Number::from(value)))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Number::from_f64(value)
            .map(JsonValue::Number)
            .ok_or_else(|| E::custom("invalid JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(JsonValue::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(JsonValue::String(value))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element()? {
            values.push(value);
        }
        Ok(JsonValue::Array(values))
    }

    fn visit_map<A>(self, mut mapping: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = BTreeMap::new();
        while let Some(key) = mapping.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(de::Error::custom("duplicate JSON object key"));
            }
            values.insert(key, mapping.next_value()?);
        }
        Ok(JsonValue::Object(values))
    }
}

pub fn decode(bytes: &[u8]) -> Result<Schedule, RepresentationError> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(RepresentationError::InputTooLarge);
    }
    if bytes
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace())
        != Some(b'{')
    {
        return Err(RepresentationError::InvalidShape);
    }

    let value: JsonValue =
        serde_json::from_slice(bytes).map_err(|_| RepresentationError::InvalidShape)?;
    let root = object(&value)?;
    require_keys(root, TOP_LEVEL_KEYS)?;

    if string_field(root, "schema_version")? != SCHEDULE_SCHEMA_VERSION {
        return Err(RepresentationError::InvalidValue);
    }
    let outcome = match string_field(root, "outcome")? {
        "selected" => ScheduleOutcome::Selected,
        "exhausted" => ScheduleOutcome::Exhausted,
        "not_schedulable" => ScheduleOutcome::NotSchedulable,
        "fallback_denied" => ScheduleOutcome::FallbackDenied,
        "cancelled" => ScheduleOutcome::Cancelled,
        _ => return Err(RepresentationError::InvalidValue),
    };
    let selected_adapter_id = nullable_string_field(root, "selected_adapter_id")?;
    if selected_adapter_id.is_some_and(|label| !valid_label(label)) {
        return Err(RepresentationError::InvalidValue);
    }
    let _fallback_permitted = boolean_field(root, "fallback_permitted")?;

    let max_attempts = unsigned_field(root, "max_attempts")?;
    if !(1..=MAX_ATTEMPTS).contains(&max_attempts) {
        return Err(RepresentationError::InvalidValue);
    }

    let attempts = match field(root, "attempts")? {
        JsonValue::Array(attempts) => attempts,
        _ => return Err(RepresentationError::InvalidValue),
    };
    if attempts.len() as u64 > max_attempts {
        return Err(RepresentationError::InconsistentSchedule);
    }

    for (index, attempt) in attempts.iter().enumerate() {
        validate_attempt(attempt, index)?;
    }

    let evidence_check = validate_evidence_check(field(root, "evidence_check")?)?;
    let evidence_acceptance = validate_evidence_acceptance(field(root, "evidence_acceptance")?)?;

    let selected = outcome == ScheduleOutcome::Selected;
    if selected != selected_adapter_id.is_some() {
        return Err(RepresentationError::InconsistentSchedule);
    }
    if selected {
        let Some(last_attempt) = attempts.last() else {
            return Err(RepresentationError::InconsistentSchedule);
        };
        let last_attempt = object(last_attempt)?;
        if string_field(last_attempt, "terminal_state")? != "exited"
            || string_field(last_attempt, "adapter_id")? != selected_adapter_id.unwrap()
        {
            return Err(RepresentationError::InconsistentSchedule);
        }
    }
    if !selected && (evidence_check || evidence_acceptance) {
        return Err(RepresentationError::InconsistentSchedule);
    }
    if evidence_acceptance && !evidence_check {
        return Err(RepresentationError::InconsistentSchedule);
    }

    Ok(Schedule {
        outcome,
        attempt_count: attempts.len(),
    })
}

fn validate_attempt(value: &JsonValue, index: usize) -> Result<(), RepresentationError> {
    let attempt = object(value)?;
    require_keys(attempt, ATTEMPT_KEYS)?;

    let ordinal = unsigned_field(attempt, "ordinal")?;
    if !(1..=MAX_ATTEMPTS).contains(&ordinal) || ordinal != index as u64 + 1 {
        return Err(RepresentationError::InconsistentSchedule);
    }
    let adapter_id = string_field(attempt, "adapter_id")?;
    if !valid_label(adapter_id) {
        return Err(RepresentationError::InvalidValue);
    }
    if !valid_fingerprint(string_field(attempt, "plan_fingerprint")?) {
        return Err(RepresentationError::InvalidValue);
    }
    if let Some(fingerprint) = nullable_string_field(attempt, "request_fingerprint")? {
        if !valid_fingerprint(fingerprint) {
            return Err(RepresentationError::InvalidValue);
        }
    }

    let terminal_state = string_field(attempt, "terminal_state")?;
    if !matches!(
        terminal_state,
        "exited" | "failed" | "timed_out" | "cancelled" | "not_dispatched"
    ) {
        return Err(RepresentationError::InvalidValue);
    }
    let exit_code = field(attempt, "exit_code")?;
    if !matches!(exit_code, JsonValue::Null) && !is_integer(exit_code) {
        return Err(RepresentationError::InvalidValue);
    }
    if !matches!(field(attempt, "reported_outcome")?, JsonValue::Null) {
        let reported_outcome = string_field(attempt, "reported_outcome")?;
        if !matches!(reported_outcome, "valid" | "failed" | "inconclusive") {
            return Err(RepresentationError::InvalidValue);
        }
    }

    if terminal_state != "exited"
        && (!matches!(exit_code, JsonValue::Null)
            || !matches!(field(attempt, "reported_outcome")?, JsonValue::Null))
    {
        return Err(RepresentationError::InconsistentSchedule);
    }
    Ok(())
}

fn validate_evidence_check(value: &JsonValue) -> Result<bool, RepresentationError> {
    let JsonValue::Object(check) = value else {
        return if matches!(value, JsonValue::Null) {
            Ok(false)
        } else {
            Err(RepresentationError::InvalidValue)
        };
    };
    require_keys(check, EVIDENCE_CHECK_KEYS)?;
    if string_field(check, "schema_version")? != EVIDENCE_CHECK_SCHEMA_VERSION
        || !valid_fingerprint(string_field(check, "request_fingerprint")?)
        || unsigned_field(check, "attempt_number")? == 0
        || !valid_fingerprint(string_field(check, "output_fingerprint")?)
        || !matches!(
            string_field(check, "outcome")?,
            "valid" | "failed" | "cancelled" | "accepted" | "rejected" | "inconclusive"
        )
    {
        return Err(RepresentationError::InvalidValue);
    }
    Ok(true)
}

fn validate_evidence_acceptance(value: &JsonValue) -> Result<bool, RepresentationError> {
    let JsonValue::Object(acceptance) = value else {
        return if matches!(value, JsonValue::Null) {
            Ok(false)
        } else {
            Err(RepresentationError::InvalidValue)
        };
    };
    let schema_version = string_field(acceptance, "schema_version")?;
    if !EVIDENCE_ACCEPTANCE_SCHEMA_VERSIONS.contains(&schema_version) {
        return Err(RepresentationError::InvalidValue);
    }
    Ok(true)
}

fn object(value: &JsonValue) -> Result<&BTreeMap<String, JsonValue>, RepresentationError> {
    match value {
        JsonValue::Object(object) => Ok(object),
        _ => Err(RepresentationError::InvalidShape),
    }
}

fn require_keys(
    object: &BTreeMap<String, JsonValue>,
    expected: &[&str],
) -> Result<(), RepresentationError> {
    if object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key)) {
        Ok(())
    } else {
        Err(RepresentationError::InvalidShape)
    }
}

fn field<'a>(
    object: &'a BTreeMap<String, JsonValue>,
    key: &str,
) -> Result<&'a JsonValue, RepresentationError> {
    object.get(key).ok_or(RepresentationError::InvalidShape)
}

fn string_field<'a>(
    object: &'a BTreeMap<String, JsonValue>,
    key: &str,
) -> Result<&'a str, RepresentationError> {
    match field(object, key)? {
        JsonValue::String(value) => Ok(value),
        _ => Err(RepresentationError::InvalidValue),
    }
}

fn nullable_string_field<'a>(
    object: &'a BTreeMap<String, JsonValue>,
    key: &str,
) -> Result<Option<&'a str>, RepresentationError> {
    match field(object, key)? {
        JsonValue::Null => Ok(None),
        JsonValue::String(value) => Ok(Some(value)),
        _ => Err(RepresentationError::InvalidValue),
    }
}

fn unsigned_field(
    object: &BTreeMap<String, JsonValue>,
    key: &str,
) -> Result<u64, RepresentationError> {
    match field(object, key)? {
        JsonValue::Number(number) => number.as_u64().ok_or(RepresentationError::InvalidValue),
        _ => Err(RepresentationError::InvalidValue),
    }
}

fn boolean_field(
    object: &BTreeMap<String, JsonValue>,
    key: &str,
) -> Result<bool, RepresentationError> {
    match field(object, key)? {
        JsonValue::Bool(value) => Ok(*value),
        _ => Err(RepresentationError::InvalidValue),
    }
}

fn is_integer(value: &JsonValue) -> bool {
    match value {
        JsonValue::Number(number) => number.as_i64().is_some() || number.as_u64().is_some(),
        _ => false,
    }
}

fn valid_label(label: &str) -> bool {
    let length = label.chars().count();
    (1..=128).contains(&length) && !label.trim().is_empty() && label.trim() == label
}

fn valid_fingerprint(fingerprint: &str) -> bool {
    fingerprint.len() == 64
        && fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::{decode, RepresentationError, MAX_INPUT_BYTES};

    #[test]
    fn attempt_schedule_decode_rejects_duplicate_json_keys() {
        let duplicate =
            br#"{"schema_version":"mumei.attempt-schedule/v1","schema_version":"mumei.attempt-schedule/v1"}"#;
        assert!(decode(duplicate).is_err());
    }

    #[test]
    fn attempt_schedule_decode_rejects_non_object_json() {
        assert_eq!(decode(b"[]"), Err(RepresentationError::InvalidShape));
    }

    #[test]
    fn attempt_schedule_decode_enforces_the_input_bound() {
        let input = vec![b' '; MAX_INPUT_BYTES + 1];
        assert_eq!(decode(&input), Err(RepresentationError::InputTooLarge));
    }
}
