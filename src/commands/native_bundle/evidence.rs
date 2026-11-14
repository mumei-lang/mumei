//! Representation of successful local rechecks, not transferable proof acceptance.

use mumei_core::native_bundle::{
    sha256, BuildSettings, BundleError, BundleFile, NativeBundleRecord, PROFILE, RECHECK_PROFILE,
    RECHECK_RESOURCE_LIMIT,
};
use mumei_core::verification_evidence::{
    Adapter, Correspondence, Evidence, Identity, Provenance, Scope, Semantics, Subject, Tool,
    SCHEMA_VERSION,
};
use mumei_emit_llvm::translation_validation::ValidationReport;
use serde::{Deserialize, Serialize};

const SCHEMA: &str = "mumei.native-consumer-evidence/v1";
const EXCLUSIONS: &[&str] = &[
    "producer_and_report_authenticity",
    "parser_encoder_and_solver_correctness",
    "llvm_to_object_and_optimizer_correctness",
    "linker_runtime_system_libraries_and_abi",
    "os_exit_status_and_final_executable_semantics",
    "dependency_closure_and_hermetic_toolchain",
    "proof_acceptance_seal_grades_and_lean_kernel_checks",
];
const ASSUMPTIONS: &[&str] = &[
    "expected_build_id_supplied_through_a_separately_trusted_channel",
    "sha256_collision_resistance",
    "restricted_source_and_llvm_encoders_linked_z3_and_llvm17_are_trusted",
    "successful_unbounded_queries_are_only_within_the_named_restricted_profiles",
];

#[derive(Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct NativeConsumerEvidence {
    schema_version: String,
    build_id: String,
    scope: String,
    acceptance: String,
    evidence: [Evidence; 3],
}

pub(super) fn decode(bytes: &[u8]) -> Result<NativeConsumerEvidence, BundleError> {
    let report: NativeConsumerEvidence = serde_json::from_slice(bytes)
        .map_err(|_| BundleError("invalid native evidence representation"))?;
    validate(&report)?;
    Ok(report)
}

fn validate(report: &NativeConsumerEvidence) -> Result<(), BundleError> {
    for record in &report.evidence {
        record
            .validate()
            .map_err(|_| BundleError("invalid native evidence representation"))?;
    }
    Ok(())
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

pub(super) fn from_recheck(
    expected_id: &str,
    record: &NativeBundleRecord,
    report: &ValidationReport,
) -> Result<NativeConsumerEvidence, BundleError> {
    let configuration = serde_json::to_vec(&serde_json::json!({
        "identity_profile": PROFILE,
        "semantic_contract_version": mumei_core::semantic_contract::SEMANTIC_CONTRACT_VERSION,
        "source_semantic_profile": report.source_profile,
        "source_profile": RECHECK_PROFILE,
        "source_timeout_ms": BuildSettings::default().solver_timeout_ms,
        "source_resource_limit": RECHECK_RESOURCE_LIMIT,
        "llvm_profile": report.profile,
        "llvm_timeout_ms": report.timeout_ms,
        "llvm_resource_limit": report.resource_limit,
        "target_triple": report.target,
        "data_layout": report.data_layout,
    }))
    .map_err(|_| BundleError("cannot encode consumer settings"))?;
    let fingerprint = sha256(&configuration);
    let version = mumei_core::verification::linked_z3_version();
    let z3_version = format!("{}.{}.{}.{}", version.0, version.1, version.2, version.3);
    let make = |subject: Subject,
                specification: Identity,
                obligation_id: &str,
                profile: &str,
                integer_model: &str,
                target_triple: Option<&str>,
                method: &str,
                scope: &str,
                correspondence: Correspondence,
                tool: Tool,
                artifact_paths: &[&str]| Evidence {
        schema_version: SCHEMA_VERSION.into(),
        subject,
        specification,
        obligation_id: obligation_id.into(),
        semantics: Semantics {
            profile: profile.into(),
            integer_model: integer_model.into(),
            target_triple: target_triple.map(str::to_owned),
        },
        method: method.into(),
        execution: "completed".into(),
        result: "established".into(),
        scope: Scope {
            kind: scope.into(),
            bounds: Vec::new(),
        },
        correspondence,
        assumptions: strings(ASSUMPTIONS),
        exclusions: strings(EXCLUSIONS),
        provenance: Provenance {
            adapter: Adapter {
                id: "mumei.native-consumer".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                protocol_version: SCHEMA.into(),
            },
            tool,
            configuration_fingerprint: fingerprint.clone(),
            translator_version: None,
            bridge_lemma_hash: None,
        },
        artifact_paths: strings(artifact_paths),
    };
    let evidence = NativeConsumerEvidence {
        schema_version: SCHEMA.into(),
        build_id: expected_id.to_owned(),
        scope: super::LLVM_RECHECK_SCOPE.into(),
        acceptance: "not_evaluated".into(),
        evidence: [
            make(
                Subject { id: "program".into(), kind: "native_artifact".into(), sha256: record.files[&BundleFile::Executable].sha256.clone() },
                Identity { id: "native-bundle.json".into(), sha256: expected_id.to_owned() },
                "native_bundle_identity_consistency", PROFILE, "not_applicable", Some(report.target),
                "identity_check", "identity",
                Correspondence { relation: "unchecked".into(), status: "unverified".into(), notes: strings(&["pinned_source_certificate_and_bundle_member_identities_only_not_semantic_correspondence"]) },
                Tool { name: "mumei".into(), version: env!("CARGO_PKG_VERSION").to_owned() },
                &["native-bundle.json", "program.ll", "program.o", "proof-cert.json", "program"],
            ),
            make(
                Subject { id: "native-source.mm".into(), kind: "mumei_source".into(), sha256: report.source_sha256.clone() },
                Identity { id: "main.contract".into(), sha256: record.source.specification_sha256.clone() },
                "main_source_postcondition", RECHECK_PROFILE, "wrapping_i64", None,
                "deductive_proof", "unbounded",
                Correspondence { relation: "exact_model".into(), status: "assumed".into(), notes: strings(&["restricted_c03_source_encoding_and_parser_normalization_are_trusted_not_independently_proved"]) },
                Tool { name: "Z3".into(), version: z3_version.clone() },
                &[],
            ),
            make(
                Subject { id: "program.ll".into(), kind: "lowered_ir".into(), sha256: report.llvm_sha256.clone() },
                Identity { id: "native-source.mm#main.body".into(), sha256: report.source_sha256.clone() },
                "main_source_to_llvm_user_body", report.profile, "wrapping_i64", Some(report.target),
                "deductive_proof", "unbounded",
                Correspondence { relation: "exact_model".into(), status: "validated".into(), notes: strings(&[
                    "full_i64_user_body_compared_to_source_within_the_restricted_profile",
                    "wrapper_call_trunc_ret_shape_only_returns_low_32_bits_not_an_os_exit_status_proof",
                ]) },
                Tool { name: "Z3".into(), version: z3_version },
                &["program.ll"],
            ),
        ],
    };
    validate(&evidence)?;
    Ok(evidence)
}
