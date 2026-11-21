//! Static representation gate for `mumei.verifier-adapter/v1` manifests.
//!
//! Manifest-declared argv and stdin remain inert unless host policy opts in
//! through `allow_manifest_attempt_argv` and
//! `allow_manifest_attempt_stdin`; a host argv map takes precedence.
//! Verdict markers describe an adapter self-report vocabulary, never a
//! verdict. `max_memory_mb` is the adapter's declared ceiling: planning uses
//! the smaller of it and the policy memory bound; a scheduled attempt uses it
//! only when the host declares no memory bound. Core does not enforce it.
//! Core rejects escaped lone-surrogate strings, including argv items, as a
//! deliberate fail-closed narrowing from the agent's Python representation.

use crate::strict_json::{self, field, object, require_keys, Error as StrictJsonError, JsonValue};
use std::collections::{BTreeSet, HashSet};
use std::fmt;

pub const MAX_INPUT_BYTES: usize = 65_536;
const MAX_SETTINGS: usize = 32;
const MAX_VERDICT_MARKERS: usize = 32;
const MAX_ATTEMPT_STDIN_BYTES: usize = 65_536;
const MAX_MARKER_BYTES: usize = 256;
const SCHEMA_VERSION: &str = "mumei.verifier-adapter/v1";
const PERMISSIONS: &[&str] = &[
    "filesystem.read",
    "filesystem.write",
    "environment.read",
    "network.connect",
    "process.spawn",
];
const METHODS: &[&str] = &[
    "deductive_proof",
    "abstract_interpretation",
    "bounded_model_checking",
    "finite_execution",
    "static_heuristic",
    "identity_check",
    "summary_derivation",
];
const LOCATIONS: &[&str] = &["local", "container", "remote"];
const SCOPES: &[&str] = &["unbounded", "bounded", "finite", "identity_only"];
const CORRESPONDENCE_STATUSES: &[&str] = &["validated", "assumed", "unverified"];
const VERDICT_OUTCOMES: &[&str] = &["valid", "failed", "inconclusive"];
const MARKER_STREAMS: &[&str] = &["stdout", "stderr"];
const ROOT_FIELDS: &[&str] = &[
    "schema_version",
    "adapter_id",
    "adapter_version",
    "profiles",
    "methods",
    "logic_fragments",
    "targets",
    "protocol_version",
    "priority",
    "enabled",
    "location",
    "max_timeout_ms",
    "max_memory_mb",
    "result_contracts",
    "setting_specs",
    "input_formats",
    "required_permissions",
    "probe_argv",
    "attempt_argv",
    "attempt_stdin",
    "verdict_markers",
];
const REQUIRED_ROOT_FIELDS: &[&str] = &[
    "schema_version",
    "adapter_id",
    "adapter_version",
    "profiles",
    "methods",
];
const CONTRACT_FIELDS: &[&str] = &[
    "profile",
    "method",
    "scope",
    "bounds",
    "assumptions",
    "correspondence_relation",
    "correspondence_status",
];
const REQUIRED_CONTRACT_FIELDS: &[&str] = &[
    "profile",
    "method",
    "scope",
    "correspondence_relation",
    "correspondence_status",
];
const SPEC_FIELDS: &[&str] = &["name", "kind", "default", "minimum", "maximum", "choices"];
const REQUIRED_SPEC_FIELDS: &[&str] = &["name", "kind", "default"];
const MARKER_FIELDS: &[&str] = &["stream", "contains", "outcome"];
const RESERVED_SETTING_COMPONENTS: &[&str] = &[
    "key",
    "token",
    "secret",
    "password",
    "credential",
    "credentials",
    "auth",
    "authentication",
    "authorization",
    "cookie",
    "endpoint",
    "url",
    "path",
    "command",
    "argv",
    "args",
    "env",
    "environment",
    "header",
    "headers",
    "executable",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepresentationError {
    InputTooLarge,
    InvalidShape,
    InvalidValue,
}

impl fmt::Display for RepresentationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid adapter-manifest representation")
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdapterManifest {
    pub attempt_argv_declared: bool,
    pub attempt_stdin_declared: bool,
    pub verdict_marker_count: usize,
}

#[derive(Debug, Hash, PartialEq, Eq)]
struct ContractIdentity {
    profile: String,
    method: String,
    scope: String,
    bounds: Vec<String>,
    assumptions: Vec<String>,
    correspondence_relation: String,
    correspondence_status: String,
}

pub fn decode(bytes: &[u8]) -> Result<AdapterManifest, RepresentationError> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(RepresentationError::InputTooLarge);
    }
    let value = strict_json::parse(bytes)?;
    let root = object(&value)?;
    ensure_fields(root, ROOT_FIELDS, REQUIRED_ROOT_FIELDS)?;

    if string_field(root, "schema_version")? != SCHEMA_VERSION {
        return Err(RepresentationError::InvalidValue);
    }
    let adapter_id = string_field(root, "adapter_id")?;
    if !valid_label(adapter_id) {
        return Err(RepresentationError::InvalidValue);
    }
    if !valid_version(string_field(root, "adapter_version")?)
        || !valid_label(optional_string(root, "protocol_version", SCHEMA_VERSION)?)
    {
        return Err(RepresentationError::InvalidValue);
    }

    let profiles = string_set(field(root, "profiles")?, false)?;
    let methods = string_set(field(root, "methods")?, false)?;
    if methods
        .iter()
        .any(|method| !METHODS.contains(&method.as_str()))
    {
        return Err(RepresentationError::InvalidValue);
    }
    for key in ["logic_fragments", "targets", "input_formats"] {
        if let Some(value) = root.get(key) {
            string_set(value, true)?;
        }
    }

    if let Some(priority) = root.get("priority") {
        require_integer_token(priority)?;
    }
    if let Some(enabled) = root.get("enabled") {
        if !matches!(enabled, JsonValue::Bool(_)) {
            return Err(RepresentationError::InvalidValue);
        }
    }
    if let Some(location) = root.get("location") {
        if !LOCATIONS.contains(&string_value(location)?) {
            return Err(RepresentationError::InvalidValue);
        }
    }
    for key in ["max_timeout_ms", "max_memory_mb"] {
        if let Some(value) = root.get(key) {
            require_positive_integer_token(value)?;
        }
    }

    let mut contracts = HashSet::new();
    if let Some(value) = root.get("result_contracts") {
        let entries = array(value)?;
        for entry in entries {
            let identity = validate_contract(entry, &profiles, &methods)?;
            if !contracts.insert(identity) {
                return Err(RepresentationError::InvalidValue);
            }
        }
    }

    if let Some(value) = root.get("setting_specs") {
        let entries = array(value)?;
        if entries.len() > MAX_SETTINGS {
            return Err(RepresentationError::InvalidValue);
        }
        let mut names = HashSet::new();
        for entry in entries {
            let name = validate_spec(entry)?;
            if !names.insert(name.to_owned()) {
                return Err(RepresentationError::InvalidValue);
            }
        }
    }

    if let Some(value) = root.get("required_permissions") {
        if !matches!(value, JsonValue::Null) {
            let permissions = string_set(value, true)?;
            if permissions
                .iter()
                .any(|permission| !PERMISSIONS.contains(&permission.as_str()))
            {
                return Err(RepresentationError::InvalidValue);
            }
        }
    }

    for key in ["probe_argv", "attempt_argv"] {
        if let Some(value) = root.get(key) {
            validate_argv(value)?;
        }
    }
    let attempt_argv_declared = optional_argv_declared(root.get("attempt_argv"))?;
    let attempt_stdin_declared = match root.get("attempt_stdin") {
        None | Some(JsonValue::Null) => false,
        Some(JsonValue::String(value)) => {
            if value.len() > MAX_ATTEMPT_STDIN_BYTES {
                return Err(RepresentationError::InvalidValue);
            }
            true
        }
        Some(_) => return Err(RepresentationError::InvalidValue),
    };
    let verdict_marker_count = validate_verdict_markers(root.get("verdict_markers"))?;

    Ok(AdapterManifest {
        attempt_argv_declared,
        attempt_stdin_declared,
        verdict_marker_count,
    })
}

fn ensure_fields(
    object: &std::collections::BTreeMap<String, JsonValue>,
    allowed: &[&str],
    required: &[&str],
) -> Result<(), RepresentationError> {
    if object.keys().any(|key| !allowed.contains(&key.as_str()))
        || required.iter().any(|key| !object.contains_key(*key))
    {
        Err(RepresentationError::InvalidShape)
    } else {
        Ok(())
    }
}

fn string_set(value: &JsonValue, empty: bool) -> Result<BTreeSet<String>, RepresentationError> {
    let values = array(value)?;
    let mut output = BTreeSet::new();
    for value in values {
        let label = string_value(value)?;
        if !valid_label(label) {
            return Err(RepresentationError::InvalidValue);
        }
        output.insert(label.to_owned());
    }
    if !empty && output.is_empty() {
        return Err(RepresentationError::InvalidValue);
    }
    Ok(output)
}

fn validate_contract(
    value: &JsonValue,
    profiles: &BTreeSet<String>,
    methods: &BTreeSet<String>,
) -> Result<ContractIdentity, RepresentationError> {
    let contract = object(value)?;
    ensure_fields(contract, CONTRACT_FIELDS, REQUIRED_CONTRACT_FIELDS)?;

    let profile = string_field(contract, "profile")?.to_owned();
    let method = string_field(contract, "method")?.to_owned();
    let scope = string_field(contract, "scope")?.to_owned();
    let bounds = optional_string_set(contract, "bounds")?;
    let assumptions = optional_string_set(contract, "assumptions")?;
    let correspondence_relation = string_field(contract, "correspondence_relation")?.to_owned();
    let correspondence_status = string_field(contract, "correspondence_status")?.to_owned();
    if !valid_label(&profile)
        || !METHODS.contains(&method.as_str())
        || !SCOPES.contains(&scope.as_str())
        || !CORRESPONDENCE_STATUSES.contains(&correspondence_status.as_str())
        || !valid_label(&correspondence_relation)
        || !profiles.contains(&profile)
        || !methods.contains(&method)
        || (!bounds.is_empty() != matches!(scope.as_str(), "bounded" | "finite"))
    {
        return Err(RepresentationError::InvalidValue);
    }
    Ok(ContractIdentity {
        profile,
        method,
        scope,
        bounds: bounds.into_iter().collect(),
        assumptions: assumptions.into_iter().collect(),
        correspondence_relation,
        correspondence_status,
    })
}

fn optional_string_set(
    object: &std::collections::BTreeMap<String, JsonValue>,
    key: &str,
) -> Result<BTreeSet<String>, RepresentationError> {
    match object.get(key) {
        None => Ok(BTreeSet::new()),
        Some(value) => string_set(value, true),
    }
}

fn validate_spec(value: &JsonValue) -> Result<&str, RepresentationError> {
    let spec = object(value)?;
    ensure_fields(spec, SPEC_FIELDS, REQUIRED_SPEC_FIELDS)?;
    let name = string_field(spec, "name")?;
    let kind = string_field(spec, "kind")?;
    if !valid_setting_name(name) {
        return Err(RepresentationError::InvalidValue);
    }

    let minimum = optional_integer(spec.get("minimum"))?;
    let maximum = optional_integer(spec.get("maximum"))?;
    let choices = match spec.get("choices") {
        None => BTreeSet::new(),
        Some(value) => string_set(value, true)?,
    };
    if choices.len() > MAX_SETTINGS {
        return Err(RepresentationError::InvalidValue);
    }

    match kind {
        "integer" => {
            let (Some(minimum), Some(maximum)) = (minimum, maximum) else {
                return Err(RepresentationError::InvalidValue);
            };
            if minimum > maximum {
                return Err(RepresentationError::InvalidValue);
            }
            if !choices.is_empty() {
                return Err(RepresentationError::InvalidValue);
            }
            let default = integer_value(field(spec, "default")?)?;
            if !(minimum..=maximum).contains(&default) {
                return Err(RepresentationError::InvalidValue);
            }
        }
        "boolean" => {
            if minimum.is_some() || maximum.is_some() || !choices.is_empty() {
                return Err(RepresentationError::InvalidValue);
            }
            if !matches!(field(spec, "default")?, JsonValue::Bool(_)) {
                return Err(RepresentationError::InvalidValue);
            }
        }
        "choice" => {
            if minimum.is_some() || maximum.is_some() || choices.is_empty() {
                return Err(RepresentationError::InvalidValue);
            }
            if choices
                .iter()
                .any(|choice| choice.chars().count() > 64 || !valid_label(choice))
            {
                return Err(RepresentationError::InvalidValue);
            }
            let default = string_field(spec, "default")?;
            if !choices.contains(default) {
                return Err(RepresentationError::InvalidValue);
            }
        }
        _ => return Err(RepresentationError::InvalidValue),
    }
    Ok(name)
}

fn validate_argv(value: &JsonValue) -> Result<(), RepresentationError> {
    if matches!(value, JsonValue::Null) {
        return Ok(());
    }
    let values = array(value)?;
    if values.is_empty()
        || values
            .iter()
            .any(|value| !matches!(value, JsonValue::String(argument) if !argument.is_empty()))
    {
        return Err(RepresentationError::InvalidValue);
    }
    Ok(())
}

fn optional_argv_declared(value: Option<&JsonValue>) -> Result<bool, RepresentationError> {
    match value {
        None | Some(JsonValue::Null) => Ok(false),
        Some(value) => {
            validate_argv(value)?;
            Ok(true)
        }
    }
}

fn validate_verdict_markers(value: Option<&JsonValue>) -> Result<usize, RepresentationError> {
    let Some(value) = value else {
        return Ok(0);
    };
    if matches!(value, JsonValue::Null) {
        return Ok(0);
    }
    let markers = array(value)?;
    if markers.len() > MAX_VERDICT_MARKERS {
        return Err(RepresentationError::InvalidValue);
    }
    for marker in markers {
        let marker = object(marker)?;
        require_keys(marker, MARKER_FIELDS)?;
        let stream = string_field(marker, "stream")?;
        let contains = string_field(marker, "contains")?;
        let outcome = string_field(marker, "outcome")?;
        if !MARKER_STREAMS.contains(&stream)
            || contains.is_empty()
            || contains.len() > MAX_MARKER_BYTES
            || !VERDICT_OUTCOMES.contains(&outcome)
        {
            return Err(RepresentationError::InvalidValue);
        }
    }
    Ok(markers.len())
}

fn optional_string<'a>(
    object: &'a std::collections::BTreeMap<String, JsonValue>,
    key: &str,
    default: &'a str,
) -> Result<&'a str, RepresentationError> {
    match object.get(key) {
        None => Ok(default),
        Some(value) => string_value(value),
    }
}

fn optional_integer(value: Option<&JsonValue>) -> Result<Option<i64>, RepresentationError> {
    match value {
        None | Some(JsonValue::Null) => Ok(None),
        Some(value) => Ok(Some(integer_value(value)?)),
    }
}

fn require_integer_token(value: &JsonValue) -> Result<(), RepresentationError> {
    match value {
        JsonValue::Number(number) if number.is_integer_token() => Ok(()),
        _ => Err(RepresentationError::InvalidValue),
    }
}

fn require_positive_integer_token(value: &JsonValue) -> Result<(), RepresentationError> {
    match value {
        JsonValue::Number(number) if number.is_positive_integer_token() => Ok(()),
        _ => Err(RepresentationError::InvalidValue),
    }
}

fn integer_value(value: &JsonValue) -> Result<i64, RepresentationError> {
    match value {
        JsonValue::Number(number) => number
            .as_i64_integer()
            .ok_or(RepresentationError::InvalidValue),
        _ => Err(RepresentationError::InvalidValue),
    }
}

fn array(value: &JsonValue) -> Result<&[JsonValue], RepresentationError> {
    match value {
        JsonValue::Array(values) => Ok(values),
        _ => Err(RepresentationError::InvalidValue),
    }
}

fn string_field<'a>(
    object: &'a std::collections::BTreeMap<String, JsonValue>,
    key: &str,
) -> Result<&'a str, RepresentationError> {
    string_value(field(object, key)?)
}

fn string_value(value: &JsonValue) -> Result<&str, RepresentationError> {
    match value {
        JsonValue::String(value) => Ok(value),
        _ => Err(RepresentationError::InvalidValue),
    }
}

fn valid_label(value: &str) -> bool {
    let mut segments = value.split('/');
    let Some(first) = segments.next() else {
        return false;
    };
    let mut first_bytes = first.bytes();
    if !first_bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic())
        || !first_bytes.all(label_tail_byte)
    {
        return false;
    }
    segments.all(|segment| !segment.is_empty() && segment.bytes().all(label_tail_byte))
}

fn label_tail_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-')
}

fn valid_version(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = consume_digits(bytes, 0);
    if index == 0 {
        return false;
    }
    while bytes.get(index) == Some(&b'.') {
        index += 1;
        let next = consume_digits(bytes, index);
        if next == index {
            return false;
        }
        index = next;
    }
    if bytes
        .get(index)
        .is_some_and(|byte| matches!(*byte, b'-' | b'+'))
    {
        index += 1;
        let suffix = &bytes[index..];
        if suffix.is_empty()
            || !suffix
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'-'))
        {
            return false;
        }
        index = bytes.len();
    }
    index == bytes.len()
}

fn consume_digits(bytes: &[u8], mut index: usize) -> usize {
    while bytes.get(index).is_some_and(|byte| byte.is_ascii_digit()) {
        index += 1;
    }
    index
}

fn valid_setting_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 || !bytes[0].is_ascii_lowercase() {
        return false;
    }
    let mut previous_underscore = false;
    for byte in bytes.iter().copied().skip(1) {
        if byte == b'_' {
            if previous_underscore {
                return false;
            }
            previous_underscore = true;
        } else if byte.is_ascii_lowercase() || byte.is_ascii_digit() {
            previous_underscore = false;
        } else {
            return false;
        }
    }
    if previous_underscore {
        return false;
    }
    !value
        .split('_')
        .any(|component| RESERVED_SETTING_COMPONENTS.contains(&component))
}

#[cfg(test)]
mod tests {
    use super::{decode, RepresentationError, MAX_INPUT_BYTES};

    const MINIMAL: &str = r#"{"schema_version":"mumei.verifier-adapter/v1","adapter_id":"fixture","adapter_version":"1.0.0","profiles":["mumei.pure-i64-comparison/v1"],"methods":["deductive_proof"]}"#;

    #[test]
    fn decode_enforces_the_input_size_limit() {
        let mut input = MINIMAL.as_bytes().to_vec();
        input.resize(MAX_INPUT_BYTES, b' ');
        assert!(decode(&input).is_ok());

        input.push(b' ');
        assert_eq!(decode(&input), Err(RepresentationError::InputTooLarge));
    }
}
