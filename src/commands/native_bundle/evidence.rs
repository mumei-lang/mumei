//! Representation of successful local rechecks, not transferable proof acceptance.

use mumei_core::native_bundle::{
    sha256, BuildSettings, BundleError, BundleFile, ModuleNativeBundleRecord, NativeBundleRecord,
    PROFILE, RECHECK_PROFILE, RECHECK_RESOURCE_LIMIT,
};
use mumei_core::verification_evidence::{
    Adapter, Correspondence, Evidence, Identity, Provenance, Scope, Semantics, Subject, Tool,
    SCHEMA_VERSION,
};
use mumei_emit_llvm::translation_validation::{ModuleValidationReport, ValidationReport};
use serde::{Deserialize, Serialize};

const SCHEMA: &str = "mumei.native-consumer-evidence/v1";
const MODULE_SCHEMA: &str = "mumei.native-module-consumer-evidence/v1";
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

/// The module family of consumer evidence: one identity record plus a
/// source-postcondition and a source-to-LLVM record per atom, so the atom
/// count cannot be fixed the way the single-atom `[Evidence; 3]` is.
#[derive(Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct NativeModuleConsumerEvidence {
    schema_version: String,
    build_id: String,
    scope: String,
    acceptance: String,
    evidence: Vec<Evidence>,
}

/// Saved evidence decoded by its own `schema_version` — the families
/// cannot cross-parse, so a single-atom file can never stand in for a
/// module report or vice versa.
pub(super) enum ConsumerEvidence {
    Single(Box<NativeConsumerEvidence>),
    Module(NativeModuleConsumerEvidence),
}

pub(super) fn decode(bytes: &[u8]) -> Result<ConsumerEvidence, BundleError> {
    let schema = serde_json::from_slice::<serde_json::Value>(bytes)
        .map_err(|_| BundleError("invalid native evidence representation"))?
        .get("schema_version")
        .and_then(|value| value.as_str())
        .map(str::to_owned);
    match schema.as_deref() {
        Some(SCHEMA) => {
            let report: NativeConsumerEvidence = serde_json::from_slice(bytes)
                .map_err(|_| BundleError("invalid native evidence representation"))?;
            validate(&report.evidence)?;
            Ok(ConsumerEvidence::Single(Box::new(report)))
        }
        Some(MODULE_SCHEMA) => {
            let report: NativeModuleConsumerEvidence = serde_json::from_slice(bytes)
                .map_err(|_| BundleError("invalid native evidence representation"))?;
            validate(&report.evidence)?;
            Ok(ConsumerEvidence::Module(report))
        }
        _ => Err(BundleError("invalid native evidence representation")),
    }
}

fn validate(evidence: &[Evidence]) -> Result<(), BundleError> {
    for record in evidence {
        record
            .validate()
            .map_err(|_| BundleError("invalid native evidence representation"))?;
    }
    Ok(())
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn z3_version() -> String {
    let version = mumei_core::verification::linked_z3_version();
    format!("{}.{}.{}.{}", version.0, version.1, version.2, version.3)
}

#[allow(clippy::too_many_arguments)]
fn make_evidence(
    fingerprint: &str,
    protocol: &str,
    subject: Subject,
    specification: Identity,
    obligation_id: &str,
    profile: &str,
    integer_model: &str,
    target_triple: Option<&str>,
    method: &str,
    scope: &str,
    correspondence: Correspondence,
    tool: Tool,
    artifact_paths: &[&str],
) -> Evidence {
    Evidence {
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
                protocol_version: protocol.into(),
            },
            tool,
            configuration_fingerprint: fingerprint.into(),
            translator_version: None,
            bridge_lemma_hash: None,
        },
        artifact_paths: strings(artifact_paths),
    }
}

fn configuration_fingerprint(
    source_profile: &str,
    llvm_profile: &str,
    llvm_timeout_ms: u32,
    llvm_resource_limit: u32,
    target: &str,
    data_layout: &str,
) -> Result<String, BundleError> {
    let configuration = serde_json::to_vec(&serde_json::json!({
        "identity_profile": PROFILE,
        "semantic_contract_version": mumei_core::semantic_contract::SEMANTIC_CONTRACT_VERSION,
        "source_semantic_profile": source_profile,
        "source_profile": RECHECK_PROFILE,
        "source_timeout_ms": BuildSettings::default().solver_timeout_ms,
        "source_resource_limit": RECHECK_RESOURCE_LIMIT,
        "llvm_profile": llvm_profile,
        "llvm_timeout_ms": llvm_timeout_ms,
        "llvm_resource_limit": llvm_resource_limit,
        "target_triple": target,
        "data_layout": data_layout,
    }))
    .map_err(|_| BundleError("cannot encode consumer settings"))?;
    Ok(sha256(&configuration))
}

pub(super) fn from_recheck(
    expected_id: &str,
    record: &NativeBundleRecord,
    report: &ValidationReport,
) -> Result<NativeConsumerEvidence, BundleError> {
    let fingerprint = configuration_fingerprint(
        report.source_profile,
        report.profile,
        report.timeout_ms,
        report.resource_limit,
        report.target,
        report.data_layout,
    )?;
    let z3_version = z3_version();
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
                artifact_paths: &[&str]|
     -> Evidence {
        make_evidence(
            &fingerprint,
            SCHEMA,
            subject,
            specification,
            obligation_id,
            profile,
            integer_model,
            target_triple,
            method,
            scope,
            correspondence,
            tool,
            artifact_paths,
        )
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
    validate(&evidence.evidence)?;
    Ok(evidence)
}

/// Module evidence: one identity record plus, per atom in the record's
/// sorted binding map, a source-postcondition record and a
/// source-to-LLVM translation record — `1 + 2·atoms` in canonical order.
/// Emitted only on aggregate `EquivalentWithinProfile`, so every atom's
/// outcome is `established`; there is no partial-success module report.
pub(super) fn from_module_recheck(
    expected_id: &str,
    record: &ModuleNativeBundleRecord,
    report: &ModuleValidationReport,
) -> Result<NativeModuleConsumerEvidence, BundleError> {
    let fingerprint = configuration_fingerprint(
        report.source_profile,
        report.profile,
        report.timeout_ms,
        report.resource_limit,
        report.target,
        report.data_layout,
    )?;
    let z3_version = z3_version();
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
                artifact_paths: &[&str]|
     -> Evidence {
        make_evidence(
            &fingerprint,
            MODULE_SCHEMA,
            subject,
            specification,
            obligation_id,
            profile,
            integer_model,
            target_triple,
            method,
            scope,
            correspondence,
            tool,
            artifact_paths,
        )
    };
    let mut evidence = Vec::with_capacity(1 + 2 * record.source.atoms.len());
    evidence.push(make(
        Subject { id: "program".into(), kind: "native_artifact".into(), sha256: record.files[&BundleFile::Executable].sha256.clone() },
        Identity { id: "native-bundle.json".into(), sha256: expected_id.to_owned() },
        "native_bundle_identity_consistency", PROFILE, "not_applicable", Some(report.target),
        "identity_check", "identity",
        Correspondence { relation: "unchecked".into(), status: "unverified".into(), notes: strings(&["pinned_source_certificate_and_bundle_member_identities_only_not_semantic_correspondence"]) },
        Tool { name: "mumei".into(), version: env!("CARGO_PKG_VERSION").to_owned() },
        &["native-bundle.json", "program.ll", "program.o", "proof-cert.json", "program"],
    ));
    for (atom, binding) in &record.source.atoms {
        evidence.push(make(
            Subject { id: format!("native-source.mm#{atom}"), kind: "mumei_source".into(), sha256: report.source_sha256.clone() },
            Identity { id: format!("{atom}.contract"), sha256: binding.specification_sha256.clone() },
            &format!("{atom}_source_postcondition"), RECHECK_PROFILE, "wrapping_i64", None,
            "deductive_proof", "unbounded",
            Correspondence { relation: "exact_model".into(), status: "assumed".into(), notes: strings(&["restricted_c03_source_encoding_and_parser_normalization_are_trusted_not_independently_proved"]) },
            Tool { name: "Z3".into(), version: z3_version.clone() },
            &[],
        ));
        evidence.push(make(
            Subject { id: "program.ll".into(), kind: "lowered_ir".into(), sha256: report.llvm_sha256.clone() },
            Identity { id: format!("native-source.mm#{atom}.body"), sha256: report.source_sha256.clone() },
            &format!("{atom}_source_to_llvm_user_body"), report.profile, "wrapping_i64", Some(report.target),
            "deductive_proof", "unbounded",
            Correspondence { relation: "exact_model".into(), status: "validated".into(), notes: strings(&[
                "full_i64_user_body_compared_to_source_within_the_restricted_profile",
                "wrapper_call_trunc_ret_shape_only_returns_low_32_bits_not_an_os_exit_status_proof",
            ]) },
            Tool { name: "Z3".into(), version: z3_version.clone() },
            &["program.ll"],
        ));
    }
    let evidence = NativeModuleConsumerEvidence {
        schema_version: MODULE_SCHEMA.into(),
        build_id: expected_id.to_owned(),
        scope: super::LLVM_RECHECK_SCOPE.into(),
        acceptance: "not_evaluated".into(),
        evidence,
    };
    validate(&evidence.evidence)?;
    Ok(evidence)
}
