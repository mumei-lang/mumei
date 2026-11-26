//! Static representation gate for agent-produced attempt schedules.
//!
//! A `selected` outcome names only an adapter's declared-success exit; it is
//! not acceptance. `reported_outcome` is the adapter's manifest-declared
//! self-report, never result polarity or a verdict. The record does not attest
//! manifest-declared argv, stdin, or memory bounds. Embedded
//! `evidence_acceptance` is carried, not re-judged. This module performs no
//! adapter execution, claim-truth check, resource enforcement, or grade
//! promotion.
//! `enforcement` is the agent's self-reported effective dispatch boundary,
//! checked for shape only; it is not Evidence or a resource-enforcement
//! attestation.

use crate::evidence_check_acceptance::{self, RepresentationError as EvidenceCheckError};
use crate::strict_json::{
    self, boolean_field, field, is_integer, nullable_string_field, object, require_keys,
    string_field, unsigned_field, Error as StrictJsonError, JsonValue,
};
use std::fmt;

pub const MAX_INPUT_BYTES: usize = 65_536;
const MAX_ATTEMPTS: u64 = 8;
const SCHEDULE_SCHEMA_VERSION: &str = "mumei.attempt-schedule/v1";
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
const ENFORCEMENT_KEYS: &[&str] = &[
    "environment",
    "network",
    "filesystem_write",
    "filesystem_read",
    "process_spawn",
    "memory_bound_mb",
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

    let value = strict_json::parse(bytes)?;
    strict_json::reject_unrepresentable_integers(&value)?;
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

    let mut enforcement_presence = None;
    for (index, attempt) in attempts.iter().enumerate() {
        let has_enforcement = validate_attempt(attempt, index)?;
        if enforcement_presence.is_some_and(|expected| expected != has_enforcement) {
            return Err(RepresentationError::InconsistentSchedule);
        }
        enforcement_presence = Some(has_enforcement);
    }

    let evidence_check = match field(root, "evidence_check")? {
        JsonValue::Null => None,
        value => Some(
            evidence_check_acceptance::validate_check(value).map_err(map_evidence_check_error)?,
        ),
    };
    let evidence_acceptance = match field(root, "evidence_acceptance")? {
        JsonValue::Null => None,
        value => Some(
            evidence_check_acceptance::validate_acceptance(value)
                .map_err(map_evidence_check_error)?,
        ),
    };

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
        if let Some(check) = &evidence_check {
            if nullable_string_field(last_attempt, "request_fingerprint")?
                != Some(check.request_fingerprint.as_str())
            {
                return Err(RepresentationError::InconsistentSchedule);
            }
        }
    }
    if let Some(acceptance) = &evidence_acceptance {
        let Some(check) = &evidence_check else {
            return Err(RepresentationError::InconsistentSchedule);
        };
        if acceptance.request_fingerprint != check.request_fingerprint
            || acceptance.attempt_number != check.attempt_number
            || acceptance.output_fingerprint != check.output_fingerprint
            || acceptance.outcome != check.outcome
        {
            return Err(RepresentationError::InconsistentSchedule);
        }
    }
    if !selected && (evidence_check.is_some() || evidence_acceptance.is_some()) {
        return Err(RepresentationError::InconsistentSchedule);
    }
    Ok(Schedule {
        outcome,
        attempt_count: attempts.len(),
    })
}

fn validate_attempt(value: &JsonValue, index: usize) -> Result<bool, RepresentationError> {
    let attempt = object(value)?;
    let has_enforcement = require_attempt_keys(attempt)?;

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
    if has_enforcement {
        validate_enforcement(field(attempt, "enforcement")?, terminal_state)?;
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
    Ok(has_enforcement)
}

fn require_attempt_keys(
    attempt: &std::collections::BTreeMap<String, JsonValue>,
) -> Result<bool, RepresentationError> {
    let has_enforcement = attempt.contains_key("enforcement");
    if attempt.len() != ATTEMPT_KEYS.len() + usize::from(has_enforcement)
        || ATTEMPT_KEYS.iter().any(|key| !attempt.contains_key(*key))
    {
        return Err(RepresentationError::InvalidShape);
    }
    Ok(has_enforcement)
}

fn validate_enforcement(
    value: &JsonValue,
    terminal_state: &str,
) -> Result<(), RepresentationError> {
    let enforcement = match value {
        JsonValue::Null => {
            return if terminal_state == "not_dispatched" {
                Ok(())
            } else {
                Err(RepresentationError::InconsistentSchedule)
            };
        }
        JsonValue::Object(enforcement) => enforcement,
        _ => return Err(RepresentationError::InvalidShape),
    };
    if terminal_state == "not_dispatched" {
        return Err(RepresentationError::InconsistentSchedule);
    }
    if enforcement.len() != ENFORCEMENT_KEYS.len()
        || ENFORCEMENT_KEYS
            .iter()
            .any(|key| !enforcement.contains_key(*key))
    {
        return Err(RepresentationError::InvalidShape);
    }

    let environment = string_field(enforcement, "environment")?;
    if !matches!(environment, "inherited" | "minimal") {
        return Err(RepresentationError::InvalidValue);
    }
    for key in [
        "network",
        "filesystem_write",
        "filesystem_read",
        "process_spawn",
    ] {
        if !matches!(
            string_field(enforcement, key)?,
            "permitted" | "isolated" | "unenforced"
        ) {
            return Err(RepresentationError::InvalidValue);
        }
    }
    let memory_bound = field(enforcement, "memory_bound_mb")?;
    if !matches!(memory_bound, JsonValue::Null)
        && (!is_integer(memory_bound) || unsigned_field(enforcement, "memory_bound_mb")? == 0)
    {
        return Err(RepresentationError::InvalidValue);
    }
    Ok(())
}

fn map_evidence_check_error(error: EvidenceCheckError) -> RepresentationError {
    match error {
        EvidenceCheckError::InputTooLarge => RepresentationError::InvalidValue,
        EvidenceCheckError::InvalidShape => RepresentationError::InvalidShape,
        EvidenceCheckError::InvalidValue => RepresentationError::InvalidValue,
        EvidenceCheckError::InconsistentAcceptance => RepresentationError::InconsistentSchedule,
    }
}

fn valid_label(label: &str) -> bool {
    let length = label.chars().count();
    (1..=128).contains(&length) && !label.trim().is_empty() && label.trim() == label
}

impl From<StrictJsonError> for RepresentationError {
    fn from(error: StrictJsonError) -> Self {
        match error {
            StrictJsonError::InvalidShape => Self::InvalidShape,
            StrictJsonError::InvalidValue => Self::InvalidValue,
        }
    }
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
    use serde_json::{json, Value};

    fn valid_enforcement() -> Value {
        json!({
            "environment": "minimal",
            "network": "isolated",
            "filesystem_write": "permitted",
            "filesystem_read": "unenforced",
            "process_spawn": "isolated",
            "memory_bound_mb": null
        })
    }

    fn attempt(ordinal: u64, terminal_state: &str, enforcement: Option<Value>) -> Value {
        let mut attempt = json!({
            "ordinal": ordinal,
            "adapter_id": "alpha",
            "plan_fingerprint": "a".repeat(64),
            "request_fingerprint": null,
            "terminal_state": terminal_state,
            "exit_code": null,
            "reported_outcome": null
        });
        if let Some(enforcement) = enforcement {
            attempt["enforcement"] = enforcement;
        }
        attempt
    }

    fn schedule(attempts: Vec<Value>, outcome: &str) -> Vec<u8> {
        let max_attempts = attempts.len().max(1) as u64;
        serde_json::to_vec(&json!({
            "schema_version": "mumei.attempt-schedule/v1",
            "outcome": outcome,
            "selected_adapter_id": null,
            "fallback_permitted": true,
            "max_attempts": max_attempts,
            "attempts": attempts,
            "evidence_check": null,
            "evidence_acceptance": null
        }))
        .expect("encode attempt schedule")
    }

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

    #[test]
    fn attempt_schedule_accepts_legacy_and_valid_enforcement_records() {
        let legacy = schedule(vec![attempt(1, "failed", None)], "exhausted");
        assert!(decode(&legacy).is_ok());

        let with_enforcement = schedule(
            vec![attempt(1, "failed", Some(valid_enforcement()))],
            "exhausted",
        );
        assert!(decode(&with_enforcement).is_ok());

        let mut positive_memory = valid_enforcement();
        positive_memory["memory_bound_mb"] = json!(1);
        let with_positive_memory = schedule(
            vec![attempt(1, "failed", Some(positive_memory))],
            "exhausted",
        );
        assert!(decode(&with_positive_memory).is_ok());

        let mut inherited_environment = valid_enforcement();
        inherited_environment["environment"] = json!("inherited");
        let with_inherited_environment = schedule(
            vec![attempt(1, "failed", Some(inherited_environment))],
            "exhausted",
        );
        assert!(decode(&with_inherited_environment).is_ok());
    }

    #[test]
    fn attempt_schedule_rejects_invalid_enforcement_object_shapes() {
        let mut missing = valid_enforcement();
        missing
            .as_object_mut()
            .expect("enforcement object")
            .remove("filesystem_read");
        assert_eq!(
            decode(&schedule(
                vec![attempt(1, "failed", Some(missing))],
                "exhausted"
            )),
            Err(RepresentationError::InvalidShape)
        );

        let mut extra = valid_enforcement();
        extra["unexpected"] = json!(true);
        assert_eq!(
            decode(&schedule(
                vec![attempt(1, "failed", Some(extra))],
                "exhausted"
            )),
            Err(RepresentationError::InvalidShape)
        );

        assert!(decode(&schedule(
            vec![attempt(1, "failed", Some(json!("not an object")))],
            "exhausted"
        ))
        .is_err());

        let valid = schedule(
            vec![attempt(1, "failed", Some(valid_enforcement()))],
            "exhausted",
        );
        let duplicate = String::from_utf8(valid).expect("UTF-8 schedule").replacen(
            r#""environment":"minimal""#,
            r#""environment":"minimal","environment":"minimal""#,
            1,
        );
        assert!(decode(duplicate.as_bytes()).is_err());
    }

    #[test]
    fn attempt_schedule_rejects_invalid_enforcement_enums_and_memory_bounds() {
        for key in [
            "environment",
            "network",
            "filesystem_write",
            "filesystem_read",
            "process_spawn",
        ] {
            let mut invalid = valid_enforcement();
            invalid[key] = json!("unknown");
            assert!(
                decode(&schedule(
                    vec![attempt(1, "failed", Some(invalid))],
                    "exhausted"
                ))
                .is_err(),
                "accepted invalid {key}"
            );
        }

        let mut invalid_type = valid_enforcement();
        invalid_type["filesystem_read"] = json!(false);
        assert!(decode(&schedule(
            vec![attempt(1, "failed", Some(invalid_type))],
            "exhausted"
        ))
        .is_err());

        for invalid_memory in [json!(0), json!(-1), json!(1.5), json!("1"), json!(true)] {
            let mut invalid = valid_enforcement();
            invalid["memory_bound_mb"] = invalid_memory;
            assert!(
                decode(&schedule(
                    vec![attempt(1, "failed", Some(invalid))],
                    "exhausted"
                ))
                .is_err(),
                "accepted invalid memory bound"
            );
        }
    }

    #[test]
    fn attempt_schedule_enforcement_matches_terminal_state() {
        let not_dispatched = schedule(
            vec![attempt(1, "not_dispatched", Some(Value::Null))],
            "not_schedulable",
        );
        assert!(decode(&not_dispatched).is_ok());

        let not_dispatched_with_object = schedule(
            vec![attempt(1, "not_dispatched", Some(valid_enforcement()))],
            "not_schedulable",
        );
        assert_eq!(
            decode(&not_dispatched_with_object),
            Err(RepresentationError::InconsistentSchedule)
        );

        let dispatched_with_null =
            schedule(vec![attempt(1, "failed", Some(Value::Null))], "exhausted");
        assert_eq!(
            decode(&dispatched_with_null),
            Err(RepresentationError::InconsistentSchedule)
        );
    }

    #[test]
    fn attempt_schedule_rejects_mixed_enforcement_presence() {
        let mixed = schedule(
            vec![
                attempt(1, "failed", Some(valid_enforcement())),
                attempt(2, "failed", None),
            ],
            "exhausted",
        );
        assert_eq!(
            decode(&mixed),
            Err(RepresentationError::InconsistentSchedule)
        );
    }
}
