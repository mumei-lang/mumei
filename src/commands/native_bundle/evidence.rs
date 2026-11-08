//! Representation of successful local rechecks, not transferable proof acceptance.

use mumei_core::native_bundle::{
    sha256, BuildSettings, BundleError, BundleFile, NativeBundleRecord, PROFILE, RECHECK_PROFILE,
    RECHECK_RESOURCE_LIMIT,
};
use mumei_emit_llvm::translation_validation::ValidationReport;
use serde::Serialize;

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

#[derive(Serialize)]
pub(super) struct NativeConsumerEvidence {
    schema_version: &'static str,
    build_id: String,
    scope: &'static str,
    acceptance: &'static str,
    evidence: [Evidence; 3],
}

#[derive(Serialize)]
struct Evidence {
    schema_version: &'static str,
    subject: Subject,
    specification: Identity,
    obligation_id: &'static str,
    semantics: Semantics,
    method: &'static str,
    execution: &'static str,
    result: &'static str,
    scope: Scope,
    correspondence: Correspondence,
    assumptions: &'static [&'static str],
    exclusions: &'static [&'static str],
    provenance: Provenance,
    artifact_paths: &'static [&'static str],
}

#[derive(Serialize)]
struct Subject {
    id: &'static str,
    kind: &'static str,
    sha256: String,
}

#[derive(Serialize)]
struct Identity {
    id: &'static str,
    sha256: String,
}

#[derive(Serialize)]
struct Semantics {
    profile: &'static str,
    integer_model: &'static str,
    target_triple: Option<&'static str>,
}

#[derive(Serialize)]
struct Scope {
    kind: &'static str,
    bounds: [&'static str; 0],
}

#[derive(Serialize)]
struct Correspondence {
    relation: &'static str,
    status: &'static str,
    notes: &'static [&'static str],
}

#[derive(Serialize)]
struct Provenance {
    adapter: Adapter,
    tool: Tool,
    configuration_fingerprint: String,
}

#[derive(Serialize)]
struct Adapter {
    id: &'static str,
    version: &'static str,
    protocol_version: &'static str,
}

#[derive(Serialize)]
struct Tool {
    name: &'static str,
    version: String,
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
                obligation_id,
                profile,
                integer_model,
                target_triple,
                method,
                scope,
                correspondence: Correspondence,
                tool: Tool,
                artifact_paths| Evidence {
        schema_version: "mumei.verification-evidence/v1",
        subject,
        specification,
        obligation_id,
        semantics: Semantics {
            profile,
            integer_model,
            target_triple,
        },
        method,
        execution: "completed",
        result: "established",
        scope: Scope {
            kind: scope,
            bounds: [],
        },
        correspondence,
        assumptions: ASSUMPTIONS,
        exclusions: EXCLUSIONS,
        provenance: Provenance {
            adapter: Adapter {
                id: "mumei.native-consumer",
                version: env!("CARGO_PKG_VERSION"),
                protocol_version: SCHEMA,
            },
            tool,
            configuration_fingerprint: fingerprint.clone(),
        },
        artifact_paths,
    };
    Ok(NativeConsumerEvidence {
        schema_version: SCHEMA,
        build_id: expected_id.to_owned(),
        scope: super::LLVM_RECHECK_SCOPE,
        acceptance: "not_evaluated",
        evidence: [
            make(
                Subject { id: "program", kind: "native_artifact", sha256: record.files[&BundleFile::Executable].sha256.clone() },
                Identity { id: "native-bundle.json", sha256: expected_id.to_owned() },
                "native_bundle_identity_consistency", PROFILE, "not_applicable", Some(report.target),
                "identity_check", "identity",
                Correspondence { relation: "unchecked", status: "unverified", notes: &["pinned_source_certificate_and_bundle_member_identities_only_not_semantic_correspondence"] },
                Tool { name: "mumei", version: env!("CARGO_PKG_VERSION").to_owned() },
                &["native-bundle.json", "program.ll", "program.o", "proof-cert.json", "program"],
            ),
            make(
                Subject { id: "native-source.mm", kind: "mumei_source", sha256: report.source_sha256.clone() },
                Identity { id: "main.contract", sha256: record.source.specification_sha256.clone() },
                "main_source_postcondition", RECHECK_PROFILE, "checked_i64", None,
                "deductive_proof", "unbounded",
                Correspondence { relation: "exact_model", status: "assumed", notes: &["restricted_c03_source_encoding_and_parser_normalization_are_trusted_not_independently_proved"] },
                Tool { name: "Z3", version: z3_version.clone() },
                &[],
            ),
            make(
                Subject { id: "program.ll", kind: "lowered_ir", sha256: report.llvm_sha256.clone() },
                Identity { id: "native-source.mm#main.body", sha256: report.source_sha256.clone() },
                "main_source_to_llvm_user_body", report.profile, "checked_i64", Some(report.target),
                "deductive_proof", "unbounded",
                Correspondence { relation: "exact_model", status: "validated", notes: &[
                    "full_i64_user_body_compared_to_source_within_the_restricted_profile",
                    "wrapper_call_trunc_ret_shape_only_returns_low_32_bits_not_an_os_exit_status_proof",
                ] },
                Tool { name: "Z3", version: z3_version },
                &["program.ll"],
            ),
        ],
    })
}
