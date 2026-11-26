//! Closed-shape representation and consistency gate for the native object-route
//! acceptance descriptor (`mumei.native-evidence-acceptance/v1`) emitted by
//! `mumei native-bundle accept-object --json`, plus honest projection into the
//! backend-neutral Common Evidence contract (`mumei.verification-evidence/v1`).
//!
//! Validation establishes representation and internal consistency only: it does
//! not rerun Core rechecks, authenticate the producer or descriptor, validate
//! ABI/runtime/OS semantics, or promote a Seal Grade.

use crate::strict_json::{
    self, boolean_field, field, nullable_string_field, object, require_keys, string_field,
    unsigned_field, Error as StrictJsonError, JsonValue,
};
use crate::verification_evidence::{
    self, Adapter, Correspondence, Evidence, Identity, Provenance, Scope, Semantics, Subject, Tool,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub const SCHEMA_VERSION: &str = "mumei.native-evidence-acceptance/v1";
pub const SCOPE_WITH_OBJECT_LINK: &str = "native_bundle_object_translation_correspondence";
pub const SCOPE_WITHOUT_OBJECT_LINK: &str =
    "native_bundle_object_translation_correspondence_without_object_link";
pub const MAX_INPUT_BYTES: usize = 262_144;

/// Correspondence items required to be `checked` in every accepted object-route
/// descriptor (DESIGN_OBJECT_ACCEPTANCE.md §4-2).
pub const REQUIRED_CORRESPONDENCE_ITEMS: [&str; 10] = [
    "function_coverage",
    "function_count",
    "llvm_ir_object_equivalence",
    "relocation_targets",
    "relocation_bounds",
    "relocation_resolved_values",
    "relocation_applied_values",
    "hir_llvm_model_correspondence",
    "object_data_correspondence",
    "object_symbol_attribute_correspondence",
];

const REQUIRED_CHECKS_NOT_PERFORMED: &[&str] = &[
    "producer_authenticity",
    "descriptor_authenticity",
    "historical_execution",
    "register_convention_validation",
    "optimizer_semantics",
    "abi_runtime_semantics",
    "runtime_observation_semantics",
    "grade_promotion",
    "lean_kernel_checks",
];

const AGENT_OWNED_CHECK_NAME: &str = "object_translation_correspondence";
const CORE_OBJECT_SOURCE_CHECK_NAME: &str = "object_source_machine_code_correspondence";
const OBJECT_LINK_CHECK_NAME: &str = "object_link_correspondence";
const CHECKED_OBJECT_LINK_STATUS: &str = "checked_object_link_correspondence";
const UNCHECKED_OBJECT_LINK_STATUS: &str = "unchecked_not_translation_validated";

const TOP_LEVEL_KEYS: &[&str] = &[
    "schema_version",
    "acceptance",
    "accepted",
    "scope",
    "checks_performed",
    "checks_not_performed",
    "build_id",
    "llvm_sha256",
    "object_sha256",
    "evidence_fingerprint",
    "register_convention_assumed",
    "input_domain",
    "functions",
    "object_translation",
    "source_machine_tv",
];

const OBJECT_TRANSLATION_KEYS: &[&str] = &[
    "status",
    "transformations",
    "functions",
    "relocations",
    "correspondence",
];

const OBJECT_FUNCTION_KEYS: &[&str] = &[
    "ir_function",
    "object_symbol",
    "object_section",
    "object_offset",
    "object_size",
    "executable_address",
];

const OBJECT_RELOCATION_KEYS: &[&str] = &[
    "object_section",
    "object_offset",
    "kind",
    "target_symbol",
    "addend",
    "resolved_value",
    "applied_offset",
    "assumption",
];

const CORRESPONDENCE_KEYS: &[&str] = &["status", "checks"];
const CORRESPONDENCE_CHECK_KEYS: &[&str] = &["item", "result", "detail"];
const SOURCE_MACHINE_TV_KEYS: &[&str] = &[
    "status",
    "non_equivalent_functions",
    "unchecked_domains",
    "object_link_correspondence",
];
const NON_EQUIVALENT_FUNCTION_KEYS: &[&str] = &["name", "outcome", "reason"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepresentationError {
    InputTooLarge,
    InvalidShape,
    InvalidValue,
    InvalidDigest,
    InconsistentDescriptor,
}

impl fmt::Display for RepresentationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InputTooLarge => "object acceptance descriptor input exceeds size limit",
            Self::InvalidShape => "invalid object acceptance descriptor JSON shape",
            Self::InvalidValue => "invalid object acceptance descriptor field value",
            Self::InvalidDigest => "invalid object acceptance descriptor digest",
            Self::InconsistentDescriptor => "inconsistent object acceptance descriptor",
        })
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ObjectAcceptanceDescriptor {
    pub schema_version: String,
    pub acceptance: String,
    pub accepted: bool,
    pub scope: String,
    pub checks_performed: Vec<String>,
    pub checks_not_performed: Vec<String>,
    pub build_id: String,
    pub llvm_sha256: String,
    pub object_sha256: String,
    pub evidence_fingerprint: String,
    pub register_convention_assumed: String,
    pub input_domain: String,
    pub functions: BTreeMap<String, FunctionOutcomeEntry>,
    pub object_translation: ObjectTranslation,
    pub source_machine_tv: SourceMachineTv,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FunctionOutcomeEntry {
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callee_saved_preservation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ObjectTranslation {
    pub status: String,
    pub transformations: Vec<TransformationEdge>,
    pub functions: Vec<ObjectFunctionRecord>,
    pub relocations: Vec<ObjectRelocationRecord>,
    pub correspondence: ObjectCorrespondence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TransformationEdge {
    pub from: String,
    pub to: String,
    pub relation: String,
    pub output_identity: Option<String>,
    pub validation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ObjectFunctionRecord {
    pub ir_function: String,
    pub object_symbol: String,
    pub object_section: String,
    pub object_offset: u64,
    pub object_size: u64,
    pub executable_address: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ObjectRelocationRecord {
    pub object_section: String,
    pub object_offset: u64,
    pub kind: String,
    pub target_symbol: String,
    pub addend: i64,
    pub resolved_value: Option<i64>,
    pub applied_offset: Option<u64>,
    pub assumption: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ObjectCorrespondence {
    pub status: String,
    pub checks: Vec<CorrespondenceCheck>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CorrespondenceCheck {
    pub item: String,
    pub result: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceMachineTv {
    pub status: String,
    pub non_equivalent_functions: Vec<NonEquivalentFunction>,
    pub unchecked_domains: Vec<String>,
    pub object_link_correspondence: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NonEquivalentFunction {
    pub name: String,
    pub outcome: String,
    pub reason: String,
}

pub fn decode(bytes: &[u8]) -> Result<ObjectAcceptanceDescriptor, RepresentationError> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(RepresentationError::InputTooLarge);
    }
    let root = strict_json::parse(bytes)?;
    strict_json::reject_unrepresentable_integers(&root)?;
    let map = object(&root)?;
    require_keys(map, TOP_LEVEL_KEYS)?;

    let schema_version = string_field(map, "schema_version")?;
    if schema_version != SCHEMA_VERSION {
        return Err(RepresentationError::InvalidValue);
    }
    let acceptance = string_field(map, "acceptance")?;
    if acceptance != "evaluated" {
        return Err(RepresentationError::InvalidValue);
    }
    let accepted = boolean_field(map, "accepted")?;
    if !accepted {
        return Err(RepresentationError::InconsistentDescriptor);
    }
    let scope = string_field(map, "scope")?;
    let has_object_link = match scope {
        SCOPE_WITH_OBJECT_LINK => true,
        SCOPE_WITHOUT_OBJECT_LINK => false,
        _ => return Err(RepresentationError::InvalidValue),
    };

    let checks_performed = parse_unique_non_empty_string_list(field(map, "checks_performed")?)?;
    let checks_not_performed =
        parse_unique_non_empty_string_list(field(map, "checks_not_performed")?)?;

    let build_id = parse_sha256(string_field(map, "build_id")?)?;
    let llvm_sha256 = parse_sha256(string_field(map, "llvm_sha256")?)?;
    let object_sha256 = parse_sha256(string_field(map, "object_sha256")?)?;
    let evidence_fingerprint = parse_sha256(string_field(map, "evidence_fingerprint")?)?;

    let register_convention_assumed =
        parse_non_empty_string(string_field(map, "register_convention_assumed")?)?;
    let input_domain = parse_non_empty_string(string_field(map, "input_domain")?)?;

    let functions = parse_functions(field(map, "functions")?)?;
    let object_translation = parse_object_translation(field(map, "object_translation")?)?;
    let source_machine_tv = parse_source_machine_tv(field(map, "source_machine_tv")?)?;

    validate_consistency(
        has_object_link,
        &checks_performed,
        &checks_not_performed,
        &object_sha256,
        &functions,
        &object_translation,
        &source_machine_tv,
    )?;

    Ok(ObjectAcceptanceDescriptor {
        schema_version: schema_version.to_owned(),
        acceptance: acceptance.to_owned(),
        accepted,
        scope: scope.to_owned(),
        checks_performed,
        checks_not_performed,
        build_id,
        llvm_sha256,
        object_sha256,
        evidence_fingerprint,
        register_convention_assumed,
        input_domain,
        functions,
        object_translation,
        source_machine_tv,
    })
}

impl ObjectAcceptanceDescriptor {
    /// Projects a validated object-route acceptance descriptor into a bounded
    /// Common Evidence v1 record (`mumei.verification-evidence/v1`).
    ///
    /// The projected record preserves every unchecked domain in `exclusions`,
    /// declares `bounded_model_checking` over `wrapping_i64` with explicit
    /// `scope.bounds` and `assumptions`, carries `evidence_fingerprint` in
    /// `provenance.configuration_fingerprint`, and never claims unbounded
    /// deductive proof, producer authenticity, or Seal Grade promotion.
    pub fn to_verification_evidence(
        &self,
        tool_version: &str,
    ) -> Result<Evidence, verification_evidence::RepresentationError> {
        let uses_non_i64_integer_model = self.register_convention_assumed.contains("128_bit")
            || self.input_domain.contains("128_bit")
            || self.input_domain.contains("i128")
            || self.input_domain.contains("u128")
            || self
                .checks_performed
                .iter()
                .any(|check| check.contains("128") || check.contains("wide_scalar"));
        let evidence = Evidence {
            schema_version: verification_evidence::SCHEMA_VERSION.to_owned(),
            subject: Subject {
                id: format!("native-bundle:{}:object", self.build_id),
                kind: "native_artifact".to_owned(),
                sha256: self.object_sha256.clone(),
            },
            specification: Identity {
                id: format!("native-bundle:{}:llvm-ir", self.build_id),
                sha256: self.llvm_sha256.clone(),
            },
            obligation_id: format!("native-bundle:{}:{}", self.build_id, self.scope),
            semantics: Semantics {
                profile: self.scope.clone(),
                integer_model: if uses_non_i64_integer_model {
                    "not_applicable".to_owned()
                } else {
                    "wrapping_i64".to_owned()
                },
                target_triple: None,
            },
            method: "bounded_model_checking".to_owned(),
            execution: "completed".to_owned(),
            result: "established".to_owned(),
            scope: Scope {
                kind: "bounded".to_owned(),
                bounds: vec![
                    format!(
                        "register_convention_assumed:{}",
                        self.register_convention_assumed
                    ),
                    format!("input_domain:{}", self.input_domain),
                    format!(
                        "object_link_correspondence:{}",
                        self.source_machine_tv.object_link_correspondence
                    ),
                    format!("scope:{}", self.scope),
                ],
            },
            correspondence: Correspondence {
                relation: "property_preserving".to_owned(),
                status: "validated".to_owned(),
                notes: vec![
                    format!(
                        "object_translation_status:{}",
                        self.object_translation.status
                    ),
                    format!("source_machine_tv_status:{}", self.source_machine_tv.status),
                    format!("build_id:{}", self.build_id),
                    format!("evidence_fingerprint:{}", self.evidence_fingerprint),
                    format!(
                        "checked_correspondence_items:{}",
                        self.object_translation.correspondence.checks.len()
                    ),
                    format!("checked_functions:{}", self.functions.len()),
                ],
            },
            assumptions: vec![
                format!(
                    "register_convention_assumed:{}",
                    self.register_convention_assumed
                ),
                format!("input_domain:{}", self.input_domain),
                "producer_authenticity_unverified".to_owned(),
                "descriptor_authenticity_unverified".to_owned(),
            ],
            exclusions: self.checks_not_performed.clone(),
            provenance: Provenance {
                adapter: Adapter {
                    id: "mumei-core-object-acceptance".to_owned(),
                    version: tool_version.to_owned(),
                    protocol_version: SCHEMA_VERSION.to_owned(),
                },
                tool: Tool {
                    name: "mumei".to_owned(),
                    version: tool_version.to_owned(),
                },
                configuration_fingerprint: self.evidence_fingerprint.clone(),
                translator_version: None,
                bridge_lemma_hash: None,
            },
            artifact_paths: Vec::new(),
            produced_at: None,
        };
        evidence.validate()?;
        Ok(evidence)
    }
}

fn validate_consistency(
    has_object_link: bool,
    checks_performed: &[String],
    checks_not_performed: &[String],
    object_sha256: &str,
    functions: &BTreeMap<String, FunctionOutcomeEntry>,
    object_translation: &ObjectTranslation,
    source_machine_tv: &SourceMachineTv,
) -> Result<(), RepresentationError> {
    let performed: BTreeSet<&str> = checks_performed.iter().map(String::as_str).collect();
    let not_performed: BTreeSet<&str> = checks_not_performed.iter().map(String::as_str).collect();

    if !performed.is_disjoint(&not_performed) {
        return Err(RepresentationError::InconsistentDescriptor);
    }
    if performed.contains(AGENT_OWNED_CHECK_NAME) || not_performed.contains(AGENT_OWNED_CHECK_NAME)
    {
        return Err(RepresentationError::InconsistentDescriptor);
    }
    if !performed.contains(CORE_OBJECT_SOURCE_CHECK_NAME) {
        return Err(RepresentationError::InconsistentDescriptor);
    }
    for required in REQUIRED_CORRESPONDENCE_ITEMS {
        if !performed.contains(required) {
            return Err(RepresentationError::InconsistentDescriptor);
        }
    }
    for required in REQUIRED_CHECKS_NOT_PERFORMED {
        if !not_performed.contains(required) {
            return Err(RepresentationError::InconsistentDescriptor);
        }
    }

    let unchecked_domains: BTreeSet<&str> = source_machine_tv
        .unchecked_domains
        .iter()
        .map(String::as_str)
        .collect();
    if unchecked_domains != not_performed {
        return Err(RepresentationError::InconsistentDescriptor);
    }

    if has_object_link {
        if !performed.contains(OBJECT_LINK_CHECK_NAME)
            || source_machine_tv.object_link_correspondence != CHECKED_OBJECT_LINK_STATUS
        {
            return Err(RepresentationError::InconsistentDescriptor);
        }
    } else if !not_performed.contains(OBJECT_LINK_CHECK_NAME)
        || source_machine_tv.object_link_correspondence != UNCHECKED_OBJECT_LINK_STATUS
    {
        return Err(RepresentationError::InconsistentDescriptor);
    }

    // At least one function must be equivalent within profile.
    if !functions
        .values()
        .any(|entry| entry.outcome == "equivalent_within_profile")
    {
        return Err(RepresentationError::InconsistentDescriptor);
    }

    // Non-equivalent functions in `source_machine_tv` must match the
    // `outside_profile` entries in `functions`.
    let outside_from_functions: BTreeSet<&str> = functions
        .iter()
        .filter_map(|(name, entry)| (entry.outcome == "outside_profile").then_some(name.as_str()))
        .collect();
    let mut outside_from_summary = BTreeSet::new();
    for entry in &source_machine_tv.non_equivalent_functions {
        if !outside_from_summary.insert(entry.name.as_str()) {
            return Err(RepresentationError::InconsistentDescriptor);
        }
    }
    if outside_from_functions != outside_from_summary {
        return Err(RepresentationError::InconsistentDescriptor);
    }

    // Correspondence checks must cover all required items without duplicates.
    let mut checked_items = BTreeSet::new();
    for check in &object_translation.correspondence.checks {
        if !checked_items.insert(check.item.as_str()) {
            return Err(RepresentationError::InconsistentDescriptor);
        }
    }
    for required in REQUIRED_CORRESPONDENCE_ITEMS {
        if !checked_items.contains(required) {
            return Err(RepresentationError::InconsistentDescriptor);
        }
    }

    // Transformation edges must agree with `object_sha256` and `object_link_correspondence`.
    let expected_link_result = if has_object_link {
        "checked_correspondence"
    } else {
        UNCHECKED_OBJECT_LINK_STATUS
    };
    let llvm_to_obj = &object_translation.transformations[0];
    if llvm_to_obj.from != "llvm_ir"
        || llvm_to_obj.to != "object_code"
        || llvm_to_obj.output_identity.as_deref() != Some(object_sha256)
        || llvm_to_obj.result.as_deref() != Some(expected_link_result)
    {
        return Err(RepresentationError::InconsistentDescriptor);
    }
    let obj_to_exec = &object_translation.transformations[1];
    if obj_to_exec.from != "object_code"
        || obj_to_exec.to != "linked_executable"
        || obj_to_exec.result.as_deref() != Some(expected_link_result)
    {
        return Err(RepresentationError::InconsistentDescriptor);
    }
    let source_to_machine = &object_translation.transformations[2];
    if source_to_machine.from != "source_atom"
        || source_to_machine.to != "machine_code"
        || source_to_machine.validation != "checked_source_machine_tv_within_profile"
        || source_to_machine.result.is_some()
    {
        return Err(RepresentationError::InconsistentDescriptor);
    }
    let hir_to_llvm = &object_translation.transformations[3];
    if hir_to_llvm.from != "hir"
        || hir_to_llvm.to != "llvm_ir"
        || hir_to_llvm.validation != "checked_hir_llvm_model_correspondence"
        || hir_to_llvm.result.as_deref() != Some("checked_correspondence")
    {
        return Err(RepresentationError::InconsistentDescriptor);
    }

    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn parse_sha256(value: &str) -> Result<String, RepresentationError> {
    if is_sha256(value) {
        Ok(value.to_owned())
    } else {
        Err(RepresentationError::InvalidDigest)
    }
}

fn parse_non_empty_string(value: &str) -> Result<String, RepresentationError> {
    if value.is_empty() {
        Err(RepresentationError::InvalidValue)
    } else {
        Ok(value.to_owned())
    }
}

fn parse_unique_non_empty_string_list(
    value: &JsonValue,
) -> Result<Vec<String>, RepresentationError> {
    let items = match value {
        JsonValue::Array(items) => items,
        _ => return Err(RepresentationError::InvalidValue),
    };
    if items.is_empty() {
        return Err(RepresentationError::InvalidValue);
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let text = match item {
            JsonValue::String(text) if !text.is_empty() => text,
            _ => return Err(RepresentationError::InvalidValue),
        };
        if !seen.insert(text.as_str()) {
            return Err(RepresentationError::InvalidValue);
        }
        out.push(text.clone());
    }
    Ok(out)
}

fn parse_functions(
    value: &JsonValue,
) -> Result<BTreeMap<String, FunctionOutcomeEntry>, RepresentationError> {
    let map = object(value)?;
    if map.is_empty() {
        return Err(RepresentationError::InvalidValue);
    }
    let mut out = BTreeMap::new();
    for (name, raw_entry) in map {
        if name.is_empty() {
            return Err(RepresentationError::InvalidValue);
        }
        let entry_map = object(raw_entry)?;
        for key in entry_map.keys() {
            if !matches!(
                key.as_str(),
                "outcome" | "callee_saved_preservation" | "reason" | "role"
            ) {
                return Err(RepresentationError::InvalidShape);
            }
        }
        let outcome = string_field(entry_map, "outcome")?;
        let callee_saved_preservation = match entry_map.get("callee_saved_preservation") {
            Some(JsonValue::String(v))
                if matches!(v.as_str(), "preserved" | "violated" | "unknown") =>
            {
                Some(v.clone())
            }
            Some(_) => return Err(RepresentationError::InvalidValue),
            None => None,
        };
        let reason = match entry_map.get("reason") {
            Some(JsonValue::String(v)) if v == "not_a_source_atom" => Some(v.clone()),
            Some(_) => return Err(RepresentationError::InvalidValue),
            None => None,
        };
        let role = match entry_map.get("role") {
            Some(JsonValue::String(v)) if v == "entry_wrapper" => Some(v.clone()),
            Some(_) => return Err(RepresentationError::InvalidValue),
            None => None,
        };
        match outcome {
            "equivalent_within_profile" => {
                if reason.is_some() {
                    return Err(RepresentationError::InconsistentDescriptor);
                }
            }
            "outside_profile" => {
                if reason.is_none() || callee_saved_preservation.is_some() {
                    return Err(RepresentationError::InconsistentDescriptor);
                }
            }
            _ => return Err(RepresentationError::InvalidValue),
        }
        out.insert(
            name.clone(),
            FunctionOutcomeEntry {
                outcome: outcome.to_owned(),
                callee_saved_preservation,
                reason,
                role,
            },
        );
    }
    Ok(out)
}

fn parse_object_translation(value: &JsonValue) -> Result<ObjectTranslation, RepresentationError> {
    let map = object(value)?;
    require_keys(map, OBJECT_TRANSLATION_KEYS)?;

    let status = string_field(map, "status")?;
    if status != "checked_correspondence" {
        return Err(RepresentationError::InvalidValue);
    }

    let transformations_raw = match field(map, "transformations")? {
        JsonValue::Array(items) if items.len() == 4 => items,
        _ => return Err(RepresentationError::InvalidValue),
    };
    let mut transformations = Vec::with_capacity(4);
    for raw_edge in transformations_raw {
        transformations.push(parse_transformation_edge(raw_edge)?);
    }

    let functions_raw = match field(map, "functions")? {
        JsonValue::Array(items) => items,
        _ => return Err(RepresentationError::InvalidValue),
    };
    let mut functions = Vec::with_capacity(functions_raw.len());
    for raw_fn in functions_raw {
        functions.push(parse_object_function(raw_fn)?);
    }

    let relocations_raw = match field(map, "relocations")? {
        JsonValue::Array(items) => items,
        _ => return Err(RepresentationError::InvalidValue),
    };
    let mut relocations = Vec::with_capacity(relocations_raw.len());
    for raw_reloc in relocations_raw {
        relocations.push(parse_object_relocation(raw_reloc)?);
    }

    let correspondence = parse_correspondence(field(map, "correspondence")?)?;

    Ok(ObjectTranslation {
        status: status.to_owned(),
        transformations,
        functions,
        relocations,
        correspondence,
    })
}

fn parse_transformation_edge(value: &JsonValue) -> Result<TransformationEdge, RepresentationError> {
    let map = object(value)?;
    for key in map.keys() {
        if !matches!(
            key.as_str(),
            "from" | "to" | "relation" | "output_identity" | "validation" | "result"
        ) {
            return Err(RepresentationError::InvalidShape);
        }
    }
    let from = parse_non_empty_string(string_field(map, "from")?)?;
    let to = parse_non_empty_string(string_field(map, "to")?)?;
    let relation = parse_non_empty_string(string_field(map, "relation")?)?;
    let output_identity = match nullable_string_field(map, "output_identity")? {
        Some(digest) => Some(parse_sha256(digest)?),
        None => None,
    };
    let validation = parse_non_empty_string(string_field(map, "validation")?)?;
    let result = match map.get("result") {
        Some(JsonValue::String(v))
            if matches!(
                v.as_str(),
                "checked_correspondence" | UNCHECKED_OBJECT_LINK_STATUS
            ) =>
        {
            Some(v.clone())
        }
        Some(_) => return Err(RepresentationError::InvalidValue),
        None => None,
    };
    Ok(TransformationEdge {
        from,
        to,
        relation,
        output_identity,
        validation,
        result,
    })
}

fn parse_object_function(value: &JsonValue) -> Result<ObjectFunctionRecord, RepresentationError> {
    let map = object(value)?;
    require_keys(map, OBJECT_FUNCTION_KEYS)?;
    let ir_function = parse_non_empty_string(string_field(map, "ir_function")?)?;
    let object_symbol = parse_non_empty_string(string_field(map, "object_symbol")?)?;
    let object_section = parse_non_empty_string(string_field(map, "object_section")?)?;
    let object_offset = unsigned_field(map, "object_offset")?;
    let object_size = unsigned_field(map, "object_size")?;
    let executable_address = match field(map, "executable_address")? {
        JsonValue::Null => None,
        JsonValue::Number(num) => Some(num.as_u64().ok_or(RepresentationError::InvalidValue)?),
        _ => return Err(RepresentationError::InvalidValue),
    };
    Ok(ObjectFunctionRecord {
        ir_function,
        object_symbol,
        object_section,
        object_offset,
        object_size,
        executable_address,
    })
}

fn parse_object_relocation(
    value: &JsonValue,
) -> Result<ObjectRelocationRecord, RepresentationError> {
    let map = object(value)?;
    require_keys(map, OBJECT_RELOCATION_KEYS)?;
    let object_section = parse_non_empty_string(string_field(map, "object_section")?)?;
    let object_offset = unsigned_field(map, "object_offset")?;
    let kind = parse_non_empty_string(string_field(map, "kind")?)?;
    let target_symbol = parse_non_empty_string(string_field(map, "target_symbol")?)?;
    let addend = match field(map, "addend")? {
        JsonValue::Number(num) => num
            .as_i64_integer()
            .ok_or(RepresentationError::InvalidValue)?,
        _ => return Err(RepresentationError::InvalidValue),
    };
    let resolved_value = match field(map, "resolved_value")? {
        JsonValue::Null => None,
        JsonValue::Number(num) => Some(
            num.as_i64_integer()
                .ok_or(RepresentationError::InvalidValue)?,
        ),
        _ => return Err(RepresentationError::InvalidValue),
    };
    let applied_offset = match field(map, "applied_offset")? {
        JsonValue::Null => None,
        JsonValue::Number(num) => Some(num.as_u64().ok_or(RepresentationError::InvalidValue)?),
        _ => return Err(RepresentationError::InvalidValue),
    };
    let assumption = match nullable_string_field(map, "assumption")? {
        Some(text) => Some(parse_non_empty_string(text)?),
        None => None,
    };
    Ok(ObjectRelocationRecord {
        object_section,
        object_offset,
        kind,
        target_symbol,
        addend,
        resolved_value,
        applied_offset,
        assumption,
    })
}

fn parse_correspondence(value: &JsonValue) -> Result<ObjectCorrespondence, RepresentationError> {
    let map = object(value)?;
    require_keys(map, CORRESPONDENCE_KEYS)?;
    let status = string_field(map, "status")?;
    if status != "checked_correspondence" {
        return Err(RepresentationError::InvalidValue);
    }
    let checks_raw = match field(map, "checks")? {
        JsonValue::Array(items) if items.len() >= REQUIRED_CORRESPONDENCE_ITEMS.len() => items,
        _ => return Err(RepresentationError::InvalidValue),
    };
    let mut checks = Vec::with_capacity(checks_raw.len());
    for raw_check in checks_raw {
        let check_map = object(raw_check)?;
        require_keys(check_map, CORRESPONDENCE_CHECK_KEYS)?;
        let item = parse_non_empty_string(string_field(check_map, "item")?)?;
        let result = string_field(check_map, "result")?;
        if result != "checked" {
            return Err(RepresentationError::InconsistentDescriptor);
        }
        let detail = string_field(check_map, "detail")?.to_owned();
        checks.push(CorrespondenceCheck {
            item,
            result: result.to_owned(),
            detail,
        });
    }
    Ok(ObjectCorrespondence {
        status: status.to_owned(),
        checks,
    })
}

fn parse_source_machine_tv(value: &JsonValue) -> Result<SourceMachineTv, RepresentationError> {
    let map = object(value)?;
    require_keys(map, SOURCE_MACHINE_TV_KEYS)?;
    let status = string_field(map, "status")?;
    if status != "checked_source_machine_tv_within_profile" {
        return Err(RepresentationError::InvalidValue);
    }
    let non_equiv_raw = match field(map, "non_equivalent_functions")? {
        JsonValue::Array(items) => items,
        _ => return Err(RepresentationError::InvalidValue),
    };
    let mut non_equivalent_functions = Vec::with_capacity(non_equiv_raw.len());
    for raw_entry in non_equiv_raw {
        let entry_map = object(raw_entry)?;
        require_keys(entry_map, NON_EQUIVALENT_FUNCTION_KEYS)?;
        let name = parse_non_empty_string(string_field(entry_map, "name")?)?;
        let outcome = string_field(entry_map, "outcome")?;
        let reason = string_field(entry_map, "reason")?;
        if outcome != "outside_profile" || reason != "not_a_source_atom" {
            return Err(RepresentationError::InconsistentDescriptor);
        }
        non_equivalent_functions.push(NonEquivalentFunction {
            name,
            outcome: outcome.to_owned(),
            reason: reason.to_owned(),
        });
    }
    let unchecked_domains = parse_unique_non_empty_string_list(field(map, "unchecked_domains")?)?;
    let object_link_correspondence = string_field(map, "object_link_correspondence")?;
    if !matches!(
        object_link_correspondence,
        CHECKED_OBJECT_LINK_STATUS | UNCHECKED_OBJECT_LINK_STATUS
    ) {
        return Err(RepresentationError::InvalidValue);
    }
    Ok(SourceMachineTv {
        status: status.to_owned(),
        non_equivalent_functions,
        unchecked_domains,
        object_link_correspondence: object_link_correspondence.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    const FIXTURE_BYTES: &[u8] = include_bytes!(
        "../../tests/fixtures/verification-evidence/native_object_acceptance_shape.json"
    );

    fn fixture_value() -> Value {
        serde_json::from_slice(FIXTURE_BYTES).expect("fixture JSON is valid")
    }

    #[test]
    fn decodes_fixture_and_projects_valid_verification_evidence() {
        let descriptor = decode(FIXTURE_BYTES).expect("fixture must decode");
        assert_eq!(descriptor.scope, SCOPE_WITH_OBJECT_LINK);
        let evidence = descriptor
            .to_verification_evidence("0.4.0-test")
            .expect("projection must satisfy Common Evidence v1");
        assert_eq!(
            evidence.schema_version,
            verification_evidence::SCHEMA_VERSION
        );
        assert_eq!(evidence.subject.kind, "native_artifact");
        assert_eq!(evidence.subject.sha256, descriptor.object_sha256);
        assert_eq!(evidence.specification.sha256, descriptor.llvm_sha256);
        assert_eq!(evidence.method, "bounded_model_checking");
        assert_eq!(evidence.scope.kind, "bounded");
        assert_eq!(evidence.semantics.integer_model, "wrapping_i64");
        assert_eq!(
            evidence.provenance.configuration_fingerprint,
            descriptor.evidence_fingerprint
        );
        assert!(evidence
            .exclusions
            .iter()
            .any(|item| item == "producer_authenticity"));
        assert!(evidence
            .exclusions
            .iter()
            .any(|item| item == "grade_promotion"));
        assert_eq!(
            evidence.obligation_id,
            format!("native-bundle:{}:{}", descriptor.build_id, descriptor.scope)
        );
        // Round-trip through Common Evidence v1 strict decoder.
        let encoded = serde_json::to_vec(&evidence).expect("serialize evidence");
        verification_evidence::decode(&encoded).expect("evidence must decode strictly");
    }

    #[test]
    fn independent_builds_with_same_scope_have_no_consistency_conflict() {
        let a = decode(FIXTURE_BYTES)
            .unwrap()
            .to_verification_evidence("0.4.0-test")
            .unwrap();
        let mut other = fixture_value();
        other["build_id"] =
            json!("1111111111111111111111111111111111111111111111111111111111111111");
        other["object_sha256"] =
            json!("2222222222222222222222222222222222222222222222222222222222222222");
        other["object_translation"]["transformations"][0]["output_identity"] =
            json!("2222222222222222222222222222222222222222222222222222222222222222");
        let b = decode(&serde_json::to_vec(&other).unwrap())
            .unwrap()
            .to_verification_evidence("0.4.0-test")
            .unwrap();
        let report = verification_evidence::consistency_check(&[a, b]);
        assert!(report.consistent, "independent builds must not conflict");
    }

    #[test]
    fn non_64_bit_input_domain_maps_to_not_applicable_integer_model() {
        let mut wide = fixture_value();
        wide["register_convention_assumed"] = json!(
            "sysv_x86_64_integer_args_rdi_rsi_rdx_rcx_r8_r9_128_bit_scalars_in_consecutive_gpr_pairs_low_first_result_rax_rdx"
        );
        let evidence = decode(&serde_json::to_vec(&wide).unwrap())
            .unwrap()
            .to_verification_evidence("0.4.0-test")
            .unwrap();
        assert_eq!(evidence.semantics.integer_model, "not_applicable");

        let mut wide_domain = fixture_value();
        wide_domain["input_domain"] = json!("unconstrained_128_bit_bitvectors");
        let evidence2 = decode(&serde_json::to_vec(&wide_domain).unwrap())
            .unwrap()
            .to_verification_evidence("0.4.0-test")
            .unwrap();
        assert_eq!(evidence2.semantics.integer_model, "not_applicable");
    }

    #[test]
    fn decodes_without_object_link_scope() {
        let mut value = fixture_value();
        value["scope"] = json!(SCOPE_WITHOUT_OBJECT_LINK);
        value["checks_performed"]
            .as_array_mut()
            .unwrap()
            .retain(|v| v != OBJECT_LINK_CHECK_NAME);
        value["checks_not_performed"]
            .as_array_mut()
            .unwrap()
            .push(json!(OBJECT_LINK_CHECK_NAME));
        value["source_machine_tv"]["object_link_correspondence"] =
            json!(UNCHECKED_OBJECT_LINK_STATUS);
        value["source_machine_tv"]["unchecked_domains"]
            .as_array_mut()
            .unwrap()
            .push(json!(OBJECT_LINK_CHECK_NAME));
        value["object_translation"]["transformations"][0]["result"] =
            json!(UNCHECKED_OBJECT_LINK_STATUS);
        value["object_translation"]["transformations"][1]["result"] =
            json!(UNCHECKED_OBJECT_LINK_STATUS);
        let bytes = serde_json::to_vec(&value).unwrap();
        let descriptor = decode(&bytes).expect("without-link descriptor must decode");
        assert_eq!(descriptor.scope, SCOPE_WITHOUT_OBJECT_LINK);
        descriptor
            .to_verification_evidence("0.4.0-test")
            .expect("without-link projection must validate");
    }

    #[test]
    fn rejects_duplicate_keys() {
        let raw = r#"{"schema_version":"mumei.native-evidence-acceptance/v1","schema_version":"mumei.native-evidence-acceptance/v1"}"#;
        assert_eq!(
            decode(raw.as_bytes()),
            Err(RepresentationError::InvalidShape)
        );
    }

    #[test]
    fn rejects_oversized_input() {
        let oversized = vec![b' '; MAX_INPUT_BYTES + 1];
        assert_eq!(decode(&oversized), Err(RepresentationError::InputTooLarge));
    }

    #[test]
    fn rejects_agent_owned_check_name_in_core_descriptor() {
        let mut value = fixture_value();
        value["checks_performed"]
            .as_array_mut()
            .unwrap()
            .push(json!(AGENT_OWNED_CHECK_NAME));
        let bytes = serde_json::to_vec(&value).unwrap();
        assert_eq!(
            decode(&bytes),
            Err(RepresentationError::InconsistentDescriptor)
        );
    }

    #[test]
    fn rejects_overlapping_performed_and_not_performed_checks() {
        let mut value = fixture_value();
        value["checks_not_performed"]
            .as_array_mut()
            .unwrap()
            .push(json!("function_coverage"));
        let bytes = serde_json::to_vec(&value).unwrap();
        assert_eq!(
            decode(&bytes),
            Err(RepresentationError::InconsistentDescriptor)
        );
    }

    #[test]
    fn rejects_missing_required_correspondence_item() {
        let mut value = fixture_value();
        value["object_translation"]["correspondence"]["checks"]
            .as_array_mut()
            .unwrap()
            .pop();
        let bytes = serde_json::to_vec(&value).unwrap();
        assert_eq!(decode(&bytes), Err(RepresentationError::InvalidValue));
    }

    #[test]
    fn rejects_scope_and_object_link_correspondence_mismatch() {
        let mut value = fixture_value();
        value["source_machine_tv"]["object_link_correspondence"] =
            json!(UNCHECKED_OBJECT_LINK_STATUS);
        let bytes = serde_json::to_vec(&value).unwrap();
        assert_eq!(
            decode(&bytes),
            Err(RepresentationError::InconsistentDescriptor)
        );
    }

    #[test]
    fn rejects_object_sha256_mismatch_with_transformation_edge() {
        let mut value = fixture_value();
        value["object_translation"]["transformations"][0]["output_identity"] =
            json!("0000000000000000000000000000000000000000000000000000000000000099");
        let bytes = serde_json::to_vec(&value).unwrap();
        assert_eq!(
            decode(&bytes),
            Err(RepresentationError::InconsistentDescriptor)
        );
    }

    #[test]
    fn rejects_non_equivalent_functions_summary_mismatch() {
        let mut value = fixture_value();
        value["functions"]["helper"] = json!({
            "outcome": "outside_profile",
            "reason": "not_a_source_atom"
        });
        let bytes = serde_json::to_vec(&value).unwrap();
        assert_eq!(
            decode(&bytes),
            Err(RepresentationError::InconsistentDescriptor)
        );
    }

    #[test]
    fn accepts_consistent_outside_profile_helper_function() {
        let mut value = fixture_value();
        value["functions"]["helper"] = json!({
            "outcome": "outside_profile",
            "reason": "not_a_source_atom"
        });
        value["source_machine_tv"]["non_equivalent_functions"] = json!([{
            "name": "helper",
            "outcome": "outside_profile",
            "reason": "not_a_source_atom"
        }]);
        let bytes = serde_json::to_vec(&value).unwrap();
        assert!(decode(&bytes).is_ok());
    }

    #[test]
    fn rejects_unchecked_domains_disagreeing_with_checks_not_performed() {
        let mut value = fixture_value();
        value["source_machine_tv"]["unchecked_domains"]
            .as_array_mut()
            .unwrap()
            .push(json!("extra_unchecked_domain"));
        let bytes = serde_json::to_vec(&value).unwrap();
        assert_eq!(
            decode(&bytes),
            Err(RepresentationError::InconsistentDescriptor)
        );
    }

    #[test]
    fn rejects_contradictory_source_machine_and_hir_llvm_edges() {
        let mut bad_source = fixture_value();
        bad_source["object_translation"]["transformations"][2]["validation"] =
            json!(UNCHECKED_OBJECT_LINK_STATUS);
        assert_eq!(
            decode(&serde_json::to_vec(&bad_source).unwrap()),
            Err(RepresentationError::InconsistentDescriptor)
        );

        let mut bad_hir = fixture_value();
        bad_hir["object_translation"]["transformations"][3]["result"] =
            json!(UNCHECKED_OBJECT_LINK_STATUS);
        assert_eq!(
            decode(&serde_json::to_vec(&bad_hir).unwrap()),
            Err(RepresentationError::InconsistentDescriptor)
        );
    }
}
