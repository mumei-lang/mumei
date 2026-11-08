//! Common v1 representation, not proof acceptance, freshness or artifact assurance.
//!
//! `decode` checks both closed JSON shape and the v1 combination rules. Serde
//! deserialization alone checks shape; constructed/deserialized values must call
//! `Evidence::validate` before use. Neither API follows paths, executes tools,
//! checks a claim's truth or changes certificate/grade/Lean policy.

use serde::{Deserialize, Deserializer, Serialize};
use std::{collections::HashSet, fmt};

pub const SCHEMA_VERSION: &str = "mumei.verification-evidence/v1";
pub const MAX_INPUT_BYTES: usize = 65_536;

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

pub fn decode(bytes: &[u8]) -> Result<Evidence, RepresentationError> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(RepresentationError::InputTooLarge);
    }
    let evidence: Evidence =
        serde_json::from_slice(bytes).map_err(|_| RepresentationError::InvalidShape)?;
    evidence.validate()?;
    Ok(evidence)
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
