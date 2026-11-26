//! Static representation and consistency checks for scheduled evidence checks.
//!
//! `accepted` records the producer's binding of a declared check outcome to
//! its current candidate; it is not Core Evidence acceptance. `candidate_match`
//! is not rechecked because Core has no attempt-sequence snapshot.

use crate::strict_json::{
    self, boolean_field, field, object, require_keys, string_field, unsigned_field,
    Error as StrictJsonError, JsonValue,
};
use std::collections::BTreeMap;
use std::fmt;

pub const MAX_INPUT_BYTES: usize = 65_536;
const MAX_COUNTER: u64 = i64::MAX as u64;
const CHECK_SCHEMA_VERSION: &str = "mumei.verifier-evidence-check/v1";
const ACCEPTANCE_SCHEMA_VERSION: &str = "mumei.evidence-check-acceptance/v1";
const CHECK_KEYS: &[&str] = &[
    "schema_version",
    "request_fingerprint",
    "attempt_number",
    "output_fingerprint",
    "outcome",
];
const ACCEPTANCE_REQUIRED_KEYS: &[&str] = &[
    "schema_version",
    "request_fingerprint",
    "attempt_number",
    "output_fingerprint",
    "outcome",
    "candidate_match",
    "accepted",
    "checks_not_performed",
];
const ACCEPTANCE_KEYS: &[&str] = &[
    "schema_version",
    "request_fingerprint",
    "attempt_number",
    "output_fingerprint",
    "outcome",
    "candidate_match",
    "accepted",
    "checks_not_performed",
    "requirement_fingerprint",
];
const OUTCOMES: &[&str] = &[
    "valid",
    "failed",
    "cancelled",
    "accepted",
    "rejected",
    "inconclusive",
];
const CHECKS_NOT_PERFORMED: &[&str] = &[
    "claim_truth",
    "file_identity",
    "freshness",
    "producer_authenticity",
    "semantic_correspondence",
    "artifact_semantics",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepresentationError {
    InputTooLarge,
    InvalidShape,
    InvalidValue,
    InconsistentAcceptance,
}

impl fmt::Display for RepresentationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid evidence-check acceptance representation")
    }
}

impl std::error::Error for RepresentationError {}

impl From<StrictJsonError> for RepresentationError {
    fn from(error: StrictJsonError) -> Self {
        match error {
            StrictJsonError::InvalidShape => Self::InvalidShape,
            StrictJsonError::InvalidValue => Self::InvalidValue,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceCheck {
    pub request_fingerprint: String,
    pub attempt_number: u64,
    pub output_fingerprint: String,
    pub outcome: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceCheckAcceptance {
    pub request_fingerprint: String,
    pub attempt_number: u64,
    pub output_fingerprint: String,
    pub outcome: String,
    pub candidate_match: bool,
    pub accepted: bool,
    pub checks_not_performed: Vec<String>,
    pub requirement_fingerprint: Option<String>,
}

pub fn decode_check(bytes: &[u8]) -> Result<EvidenceCheck, RepresentationError> {
    let value = parse(bytes)?;
    validate_check(&value)
}

pub fn decode_acceptance(bytes: &[u8]) -> Result<EvidenceCheckAcceptance, RepresentationError> {
    let value = parse(bytes)?;
    validate_acceptance(&value)
}

pub(crate) fn validate_check(value: &JsonValue) -> Result<EvidenceCheck, RepresentationError> {
    let check = object(value)?;
    require_keys(check, CHECK_KEYS)?;

    let request_fingerprint = string_field(check, "request_fingerprint")?;
    let output_fingerprint = string_field(check, "output_fingerprint")?;
    let attempt_number = unsigned_field(check, "attempt_number")?;
    let outcome = string_field(check, "outcome")?;
    if string_field(check, "schema_version")? != CHECK_SCHEMA_VERSION
        || !valid_fingerprint(request_fingerprint)
        || !(1..=MAX_COUNTER).contains(&attempt_number)
        || !valid_fingerprint(output_fingerprint)
        || !OUTCOMES.contains(&outcome)
    {
        return Err(RepresentationError::InvalidValue);
    }

    Ok(EvidenceCheck {
        request_fingerprint: request_fingerprint.to_owned(),
        attempt_number,
        output_fingerprint: output_fingerprint.to_owned(),
        outcome: outcome.to_owned(),
    })
}

pub(crate) fn validate_acceptance(
    value: &JsonValue,
) -> Result<EvidenceCheckAcceptance, RepresentationError> {
    let acceptance = object(value)?;
    validate_acceptance_keys(acceptance)?;

    let request_fingerprint = string_field(acceptance, "request_fingerprint")?;
    let output_fingerprint = string_field(acceptance, "output_fingerprint")?;
    let attempt_number = unsigned_field(acceptance, "attempt_number")?;
    let outcome = string_field(acceptance, "outcome")?;
    let candidate_match = boolean_field(acceptance, "candidate_match")?;
    let accepted = boolean_field(acceptance, "accepted")?;
    let requirement_fingerprint = match acceptance.get("requirement_fingerprint") {
        Some(JsonValue::String(value)) if valid_fingerprint(value) => Some(value.clone()),
        Some(_) => return Err(RepresentationError::InvalidValue),
        None => None,
    };

    if string_field(acceptance, "schema_version")? != ACCEPTANCE_SCHEMA_VERSION
        || !valid_fingerprint(request_fingerprint)
        || !(1..=MAX_COUNTER).contains(&attempt_number)
        || !valid_fingerprint(output_fingerprint)
        || !OUTCOMES.contains(&outcome)
    {
        return Err(RepresentationError::InvalidValue);
    }
    validate_checks_not_performed(field(acceptance, "checks_not_performed")?)?;

    let expected_accepted = candidate_match && matches!(outcome, "valid" | "accepted");
    if accepted != expected_accepted {
        return Err(RepresentationError::InconsistentAcceptance);
    }

    Ok(EvidenceCheckAcceptance {
        request_fingerprint: request_fingerprint.to_owned(),
        attempt_number,
        output_fingerprint: output_fingerprint.to_owned(),
        outcome: outcome.to_owned(),
        candidate_match,
        accepted,
        checks_not_performed: CHECKS_NOT_PERFORMED
            .iter()
            .map(|check| (*check).to_owned())
            .collect(),
        requirement_fingerprint,
    })
}

fn parse(bytes: &[u8]) -> Result<JsonValue, RepresentationError> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(RepresentationError::InputTooLarge);
    }
    Ok(strict_json::parse(bytes)?)
}

fn validate_acceptance_keys(
    acceptance: &BTreeMap<String, JsonValue>,
) -> Result<(), RepresentationError> {
    let required_are_present = ACCEPTANCE_REQUIRED_KEYS
        .iter()
        .all(|key| acceptance.contains_key(*key));
    let all_keys_are_known = acceptance
        .keys()
        .all(|key| ACCEPTANCE_KEYS.contains(&key.as_str()));
    let expected_length = ACCEPTANCE_REQUIRED_KEYS.len()
        + if acceptance.contains_key("requirement_fingerprint") {
            1
        } else {
            0
        };
    if required_are_present && all_keys_are_known && acceptance.len() == expected_length {
        Ok(())
    } else {
        Err(RepresentationError::InvalidShape)
    }
}

fn validate_checks_not_performed(value: &JsonValue) -> Result<(), RepresentationError> {
    let JsonValue::Array(checks) = value else {
        return Err(RepresentationError::InvalidValue);
    };
    if checks.len() == CHECKS_NOT_PERFORMED.len()
        && checks.iter().zip(CHECKS_NOT_PERFORMED).all(
            |(check, expected)| matches!(check, JsonValue::String(value) if value == *expected),
        )
    {
        Ok(())
    } else {
        Err(RepresentationError::InvalidValue)
    }
}

fn valid_fingerprint(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::{decode_acceptance, decode_check, RepresentationError, MAX_INPUT_BYTES};

    const CHECK: &str = r#"{"schema_version":"mumei.verifier-evidence-check/v1","request_fingerprint":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","attempt_number":1,"output_fingerprint":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","outcome":"valid"}"#;
    const ACCEPTANCE: &str = r#"{"schema_version":"mumei.evidence-check-acceptance/v1","request_fingerprint":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","attempt_number":1,"output_fingerprint":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","outcome":"valid","candidate_match":true,"accepted":true,"checks_not_performed":["claim_truth","file_identity","freshness","producer_authenticity","semantic_correspondence","artifact_semantics"]}"#;

    #[test]
    fn decoders_enforce_the_input_size_limit() {
        let input = vec![b' '; MAX_INPUT_BYTES + 1];
        assert_eq!(
            decode_check(&input),
            Err(RepresentationError::InputTooLarge)
        );
        assert_eq!(
            decode_acceptance(&input),
            Err(RepresentationError::InputTooLarge)
        );
    }

    #[test]
    fn decodes_declared_check_and_acceptance_records() {
        assert_eq!(decode_check(CHECK.as_bytes()).unwrap().outcome, "valid");
        let acceptance = decode_acceptance(ACCEPTANCE.as_bytes()).unwrap();
        assert!(acceptance.accepted);
        assert!(acceptance.requirement_fingerprint.is_none());
    }

    #[test]
    fn check_decoder_rejects_bad_counters_and_unknown_keys() {
        let base: serde_json::Value = serde_json::from_str(CHECK).unwrap();
        for attempt_number in [
            serde_json::json!(0),
            serde_json::json!(true),
            serde_json::json!(1u64 << 63),
            serde_json::json!(1.0),
        ] {
            let mut check = base.clone();
            check["attempt_number"] = attempt_number;
            assert!(decode_check(&serde_json::to_vec(&check).unwrap()).is_err());
        }

        let mut check = base;
        check["extra"] = serde_json::json!(true);
        assert!(decode_check(&serde_json::to_vec(&check).unwrap()).is_err());
    }
}
