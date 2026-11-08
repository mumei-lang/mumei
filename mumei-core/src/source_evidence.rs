//! Fresh, bounded C03 source queries; not transferable proof acceptance.

use crate::semantic_contract::{
    self, SemanticContract, PURE_I64_PROFILE, SEMANTIC_CONTRACT_VERSION,
};
use crate::verification_evidence::{
    Adapter, Correspondence, Evidence, Identity, Provenance, Scope, Semantics, Subject, Tool,
    SCHEMA_VERSION,
};
use sha2::{Digest, Sha256};
use z3::{Config, Context, SatResult, Solver};

pub const PRODUCER_PROFILE: &str = "mumei.pure-i64-source-evidence/v1";
pub const MAX_TIMEOUT_MS: u32 = 10_000;
pub const MAX_RESOURCE_LIMIT: u32 = 1_000_000;
pub const SOURCE_LABEL: &str = "source.mm";

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub timeout_ms: u32,
    pub resource_limit: u32,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            timeout_ms: MAX_TIMEOUT_MS,
            resource_limit: MAX_RESOURCE_LIMIT,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    EstablishedWithinProfile(Box<Evidence>),
    Refuted,
    Inconclusive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SourceEvidenceError {
    #[error("invalid bounded source-query budget")]
    InvalidBudget,
    #[error("source outside the bounded pure-i64 profile with literal true precondition")]
    UnsupportedSource,
    #[error("cannot construct source evidence representation")]
    InvalidRepresentation,
}

/// Check one C03 atom over all its signed-i64 inputs. Only fresh unsat yields a
/// record; false/unknown/unsupported queries cannot emit an established record.
/// No filesystem, project configuration, cache, environment policy or tools are read.
pub fn verify(source: &str, options: Options) -> Result<Outcome, SourceEvidenceError> {
    if !(1..=MAX_TIMEOUT_MS).contains(&options.timeout_ms)
        || !(1..=MAX_RESOURCE_LIMIT).contains(&options.resource_limit)
    {
        return Err(SourceEvidenceError::InvalidBudget);
    }
    if source.len() > crate::verification_evidence::MAX_INPUT_BYTES {
        return Err(SourceEvidenceError::UnsupportedSource);
    }
    let mut config = Config::new();
    config.set_timeout_msec(u64::from(options.timeout_ms));
    config.set_param_value("rlimit", &options.resource_limit.to_string());
    let context = Context::new(&config);
    // The existing encoder bounds tokens, nesting and parameters before this
    // producer extracts identities; do not parse unbounded input first.
    let obligation = semantic_contract::encode_pure_i64_obligation(source, &context)
        .map_err(|_| SourceEvidenceError::UnsupportedSource)?;
    let contract = semantic_contract::extract_pure_i64_contract(source, SOURCE_LABEL, None)
        .map_err(|_| SourceEvidenceError::UnsupportedSource)?;
    if contract.specification.requires.trim() != "true" {
        return Err(SourceEvidenceError::UnsupportedSource);
    }
    let solver = Solver::new(&context);
    solver.assert(&obligation);
    finish(solver.check(), &contract, options)
}

fn finish(
    result: SatResult,
    contract: &SemanticContract,
    options: Options,
) -> Result<Outcome, SourceEvidenceError> {
    match result {
        SatResult::Sat => Ok(Outcome::Refuted),
        SatResult::Unknown => Ok(Outcome::Inconclusive),
        SatResult::Unsat => Ok(Outcome::EstablishedWithinProfile(Box::new(record(
            contract, options,
        )?))),
    }
}

fn record(contract: &SemanticContract, options: Options) -> Result<Evidence, SourceEvidenceError> {
    let version = crate::verification::linked_z3_version();
    let version = format!("{}.{}.{}.{}", version.0, version.1, version.2, version.3);
    let settings = serde_json::to_vec(&serde_json::json!({
        "producer_profile": PRODUCER_PROFILE,
        "semantic_contract_version": SEMANTIC_CONTRACT_VERSION,
        "semantic_profile": PURE_I64_PROFILE,
        "query": "restricted_c03_bitvector_postcondition_counterexample",
        "precondition": "literal_true_only",
        "timeout_ms": options.timeout_ms,
        "resource_limit": options.resource_limit,
        "compiler_version": env!("CARGO_PKG_VERSION"),
        "z3_version": version,
    }))
    .map_err(|_| SourceEvidenceError::InvalidRepresentation)?;
    let strings = |values: &[&str]| values.iter().map(|s| (*s).to_owned()).collect();
    let evidence = Evidence {
        schema_version: SCHEMA_VERSION.into(),
        subject: Subject { id: SOURCE_LABEL.into(), kind: "mumei_source".into(), sha256: contract.source_sha256.clone() },
        specification: Identity { id: "source.postcondition".into(), sha256: contract.specification_sha256.clone() },
        obligation_id: format!("source_postcondition:{}", contract.specification_sha256),
        semantics: Semantics { profile: PURE_I64_PROFILE.into(), integer_model: "checked_i64".into(), target_triple: None },
        method: "deductive_proof".into(), execution: "completed".into(), result: "established".into(),
        scope: Scope { kind: "unbounded".into(), bounds: Vec::new() },
        correspondence: Correspondence {
            relation: "exact_model".into(), status: "assumed".into(),
            notes: strings(&["restricted_c03_source_encoding_and_parser_normalization_are_trusted_not_independently_proved"]),
        },
        assumptions: strings(&[
            "all_parameters_are_representable_signed_64_bit_integers",
            "literal_true_precondition_prevents_vacuous_acceptance",
            "restricted_c03_parser_bitvector_encoder_and_linked_z3_are_trusted",
            "sha256_collision_resistance",
        ]),
        exclusions: strings(&[
            "producer_and_report_authenticity_and_historical_freshness",
            "parser_encoder_and_solver_correctness",
            "arithmetic_calls_imports_heap_effects_and_concurrency",
            "llvm_correspondence_optimizer_object_linker_runtime_and_abi",
            "os_exit_status_and_final_executable_semantics",
            "proof_acceptance_claim_book_seal_grades_and_lean_kernel_checks",
        ]),
        provenance: Provenance {
            adapter: Adapter { id: "mumei.source-evidence".into(), version: env!("CARGO_PKG_VERSION").into(), protocol_version: PRODUCER_PROFILE.into() },
            tool: Tool { name: "Z3".into(), version },
            configuration_fingerprint: format!("{:x}", Sha256::digest(settings)),
            translator_version: None, bridge_lemma_hash: None,
        },
        artifact_paths: Vec::new(),
    };
    evidence
        .validate()
        .map_err(|_| SourceEvidenceError::InvalidRepresentation)?;
    Ok(evidence)
}

#[cfg(test)]
mod tests;
