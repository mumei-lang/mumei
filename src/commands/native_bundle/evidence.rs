//! Representation of successful local rechecks, not transferable proof acceptance.

use mumei_core::native_bundle::{
    sha256, BuildSettings, BundleError, BundleFile, ModuleAtomBinding, ModuleNativeBundleRecord,
    MultifileNativeBundleRecord, NativeBundleRecord, MODULE_RECHECK_PROFILE,
    MULTIFILE_RECHECK_PROFILE, PROFILE, RECHECK_PROFILE, RECHECK_RESOURCE_LIMIT,
};
use mumei_core::parser::{parse_body_expr_checked, parse_module_checked, Item};
use mumei_core::semantic_contract::{contains_while_stmt, LOOP_BOUND};
use mumei_core::verification_evidence::{
    Adapter, Correspondence, Evidence, Identity, Provenance, Scope, Semantics, Subject, Tool,
    SCHEMA_VERSION,
};
use mumei_emit_llvm::translation_validation::{ModuleValidationReport, ValidationReport};
use serde::{Deserialize, Serialize};

const SCHEMA: &str = "mumei.native-consumer-evidence/v1";
const MODULE_SCHEMA: &str = "mumei.native-module-consumer-evidence/v1";
const MULTIFILE_SCHEMA: &str = "mumei.native-multifile-consumer-evidence/v1";
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

/// The multifile envelope shares the module layout — an identity record,
/// the deterministic merge-correspondence record, then a per-atom
/// postcondition and source-to-LLVM record for each merged atom.
pub(super) type NativeMultifileConsumerEvidence = NativeModuleConsumerEvidence;

/// Saved evidence decoded by its own `schema_version` — the families
/// cannot cross-parse, so a single-atom file can never stand in for a
/// module report or vice versa.
pub(super) enum ConsumerEvidence {
    Single(Box<NativeConsumerEvidence>),
    Module(NativeModuleConsumerEvidence),
    Multifile(NativeMultifileConsumerEvidence),
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
        Some(MULTIFILE_SCHEMA) => {
            let report: NativeMultifileConsumerEvidence = serde_json::from_slice(bytes)
                .map_err(|_| BundleError("invalid native evidence representation"))?;
            validate(&report.evidence)?;
            Ok(ConsumerEvidence::Multifile(report))
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
    bounds: &[&str],
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
            bounds: strings(bounds),
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
        // The native consumer emits no declared timestamp yet — the
        // `--produced-at` opt-in currently covers `verify-source`.
        produced_at: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn configuration_fingerprint(
    identity_profile: &str,
    recheck_profile: &str,
    source_profile: &str,
    llvm_profile: &str,
    llvm_timeout_ms: u32,
    llvm_resource_limit: u32,
    target: &str,
    data_layout: &str,
) -> Result<String, BundleError> {
    let configuration = serde_json::to_vec(&serde_json::json!({
        "identity_profile": identity_profile,
        "semantic_contract_version": mumei_core::semantic_contract::SEMANTIC_CONTRACT_VERSION,
        "source_semantic_profile": source_profile,
        "source_profile": recheck_profile,
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
        PROFILE,
        RECHECK_PROFILE,
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
                bounds: &[&str],
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
            bounds,
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
                "identity_check", "identity", &[],
                Correspondence { relation: "unchecked".into(), status: "unverified".into(), notes: strings(&["pinned_source_certificate_and_bundle_member_identities_only_not_semantic_correspondence"]) },
                Tool { name: "mumei".into(), version: env!("CARGO_PKG_VERSION").to_owned() },
                &["native-bundle.json", "program.ll", "program.o", "proof-cert.json", "program"],
            ),
            make(
                Subject { id: "native-source.mm".into(), kind: "mumei_source".into(), sha256: report.source_sha256.clone() },
                Identity { id: "main.contract".into(), sha256: record.source.specification_sha256.clone() },
                "main_source_postcondition", RECHECK_PROFILE, "wrapping_i64", None,
                "deductive_proof", "unbounded", &[],
                Correspondence { relation: "exact_model".into(), status: "assumed".into(), notes: strings(&["restricted_c03_source_encoding_and_parser_normalization_are_trusted_not_independently_proved"]) },
                Tool { name: "Z3".into(), version: z3_version.clone() },
                &[],
            ),
            make(
                Subject { id: "program.ll".into(), kind: "lowered_ir".into(), sha256: report.llvm_sha256.clone() },
                Identity { id: "native-source.mm#main.body".into(), sha256: report.source_sha256.clone() },
                "main_source_to_llvm_user_body", report.profile, "wrapping_i64", Some(report.target),
                "deductive_proof", "unbounded", &[],
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
    source: &str,
) -> Result<NativeModuleConsumerEvidence, BundleError> {
    // The identity record and fingerprint name the record's pinned
    // profile — a v2 bundle's admission is v2, not the v1 constant.
    let evidence = emit_module_family(
        expected_id,
        MODULE_SCHEMA,
        &record.profile,
        MODULE_RECHECK_PROFILE,
        &record.source.atoms,
        &record.files[&BundleFile::Executable].sha256,
        report,
        source,
    )?;
    Ok(NativeModuleConsumerEvidence {
        schema_version: MODULE_SCHEMA.into(),
        build_id: expected_id.to_owned(),
        scope: super::LLVM_RECHECK_SCOPE.into(),
        acceptance: "not_evaluated".into(),
        evidence,
    })
}

/// The multifile family emits the module records plus one
/// merge-correspondence record: the deterministic import-closure merge
/// re-derived from the pinned members, bound to the record's
/// `import_graph_sha256` pin and the merged text the LLVM snapshot was
/// validated against.
pub(super) fn from_multifile_recheck(
    expected_id: &str,
    record: &MultifileNativeBundleRecord,
    report: &ModuleValidationReport,
    merged: &str,
) -> Result<NativeMultifileConsumerEvidence, BundleError> {
    let mut evidence = emit_module_family(
        expected_id,
        MULTIFILE_SCHEMA,
        &record.profile,
        MULTIFILE_RECHECK_PROFILE,
        &record.source.atoms,
        &record.files[&BundleFile::Executable].sha256,
        report,
        merged,
    )?;
    let fingerprint = configuration_fingerprint(
        &record.profile,
        MULTIFILE_RECHECK_PROFILE,
        report.source_profile,
        report.profile,
        report.timeout_ms,
        report.resource_limit,
        report.target,
        report.data_layout,
    )?;
    // Inserted right after the identity record so the envelope reads in
    // the order the chain runs: identity, merge, then per-atom checks.
    evidence.insert(
        1,
        make_evidence(
            &fingerprint,
            MULTIFILE_SCHEMA,
            Subject {
                id: "native-source.mm".into(),
                kind: "mumei_source".into(),
                sha256: report.source_sha256.clone(),
            },
            Identity {
                id: "source_files.import_graph_sha256".into(),
                sha256: record.source_files.import_graph_sha256.clone(),
            },
            "multifile_merge_correspondence",
            MULTIFILE_RECHECK_PROFILE,
            "not_applicable",
            None,
            "identity_check",
            "identity",
            &[],
            Correspondence {
                relation: "exact_model".into(),
                status: "validated".into(),
                notes: strings(&[
                    "import_closure_merged_deterministically_from_pinned_member_bytes",
                    "merged_text_derives_the_pinned_llvm_snapshot_source_hash",
                ]),
            },
            Tool {
                name: "mumei".into(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            &["native-bundle.json", "sources/"],
        ),
    );
    validate(&evidence)?;
    Ok(NativeMultifileConsumerEvidence {
        schema_version: MULTIFILE_SCHEMA.into(),
        build_id: expected_id.to_owned(),
        scope: super::LLVM_RECHECK_SCOPE.into(),
        acceptance: "not_evaluated".into(),
        evidence,
    })
}

/// The shared emit for the module-shaped families: one identity record
/// plus a source-postcondition and source-to-LLVM record per atom.
#[allow(clippy::too_many_arguments)]
fn emit_module_family(
    expected_id: &str,
    protocol: &'static str,
    identity_profile: &str,
    recheck_profile: &str,
    atoms: &std::collections::BTreeMap<String, ModuleAtomBinding>,
    executable_sha256: &str,
    report: &ModuleValidationReport,
    source: &str,
) -> Result<Vec<Evidence>, BundleError> {
    // Atoms carrying a `while` are verified only where the guard fails
    // within the profile bound, so their records claim a bounded scope
    // rather than the unbounded claim a loop-free atom earns. The
    // record family does not gate on this — a bundle assembled outside
    // `native-bundle build` could carry loop atoms — so the scope is
    // re-derived from the source here instead of trusting the report.
    let loop_bound_label = format!("while_loop_guard_evaluations <= {LOOP_BOUND}");
    let bound_label: &str = &loop_bound_label;
    let mut bounded_atoms = std::collections::BTreeSet::new();
    for item in
        parse_module_checked(source).map_err(|_| BundleError("module source is not parseable"))?
    {
        let Item::Atom(atom) = item else {
            continue;
        };
        let body = parse_body_expr_checked(&atom.body_expr)
            .map_err(|_| BundleError("module atom body is not parseable"))?;
        if contains_while_stmt(&body) {
            bounded_atoms.insert(atom.name);
        }
    }
    let fingerprint = configuration_fingerprint(
        identity_profile,
        recheck_profile,
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
                bounds: &[&str],
                correspondence: Correspondence,
                tool: Tool,
                artifact_paths: &[&str]|
     -> Evidence {
        make_evidence(
            &fingerprint,
            protocol,
            subject,
            specification,
            obligation_id,
            profile,
            integer_model,
            target_triple,
            method,
            scope,
            bounds,
            correspondence,
            tool,
            artifact_paths,
        )
    };
    let mut evidence = Vec::with_capacity(1 + 2 * atoms.len());
    evidence.push(make(
        Subject { id: "program".into(), kind: "native_artifact".into(), sha256: executable_sha256.to_owned() },
        Identity { id: "native-bundle.json".into(), sha256: expected_id.to_owned() },
        "native_bundle_identity_consistency", identity_profile, "not_applicable", Some(report.target),
        "identity_check", "identity", &[],
        Correspondence { relation: "unchecked".into(), status: "unverified".into(), notes: strings(&["pinned_source_certificate_and_bundle_member_identities_only_not_semantic_correspondence"]) },
        Tool { name: "mumei".into(), version: env!("CARGO_PKG_VERSION").to_owned() },
        &["native-bundle.json", "program.ll", "program.o", "proof-cert.json", "program"],
    ));
    for (atom, binding) in atoms {
        let (scope, bounds): (&str, &[&str]) = if bounded_atoms.contains(atom) {
            ("bounded", std::slice::from_ref(&bound_label))
        } else {
            ("unbounded", &[])
        };
        evidence.push(make(
            Subject { id: format!("native-source.mm#{atom}"), kind: "mumei_source".into(), sha256: report.source_sha256.clone() },
            Identity { id: format!("{atom}.contract"), sha256: binding.specification_sha256.clone() },
            &format!("{atom}_source_postcondition"), recheck_profile, "wrapping_i64", None,
            "deductive_proof", scope, bounds,
            Correspondence { relation: "exact_model".into(), status: "assumed".into(), notes: strings(&["restricted_c03_source_encoding_and_parser_normalization_are_trusted_not_independently_proved"]) },
            Tool { name: "Z3".into(), version: z3_version.clone() },
            &[],
        ));
        evidence.push(make(
            Subject { id: "program.ll".into(), kind: "lowered_ir".into(), sha256: report.llvm_sha256.clone() },
            Identity { id: format!("native-source.mm#{atom}.body"), sha256: report.source_sha256.clone() },
            &format!("{atom}_source_to_llvm_user_body"), report.profile, "wrapping_i64", Some(report.target),
            "deductive_proof", scope, bounds,
            Correspondence { relation: "exact_model".into(), status: "validated".into(), notes: strings(&[
                "full_i64_user_body_compared_to_source_within_the_restricted_profile",
                "wrapper_call_trunc_ret_shape_only_returns_low_32_bits_not_an_os_exit_status_proof",
            ]) },
            Tool { name: "Z3".into(), version: z3_version.clone() },
            &["program.ll"],
        ));
    }
    validate(&evidence)?;
    Ok(evidence)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mumei_core::native_bundle::{
        ContentIdentity, LinkBuild, ModuleAtomBinding, ModuleSourceBinding, ObjectBuild,
        MODULE_PROFILE,
    };
    use mumei_emit_llvm::translation_validation::{FunctionOutcome, Outcome};
    use std::collections::BTreeMap;

    const WHILE_MODULE: &str = "atom count(x: i64) -> i64 requires: x >= 0; ensures: result == x; body: { let i = 0; while i < x invariant: i >= 0 && i <= x decreases: x - i { i = i + 1 }; i };\natom main() -> i64 requires: true; ensures: result == 0; body: { count(0) };";
    const LOOP_FREE_MODULE: &str = "atom helper(x: i64) -> i64 requires: x > 0; ensures: result == x + 1; body: { x + 1 };\natom main() -> i64 requires: true; ensures: result == 6; body: { helper(5) };";

    fn module_record(atoms: &[&str]) -> ModuleNativeBundleRecord {
        let mut bindings = BTreeMap::new();
        for atom in atoms {
            bindings.insert(
                (*atom).to_owned(),
                ModuleAtomBinding {
                    atom_content_hash: "a".repeat(64),
                    specification_sha256: "b".repeat(64),
                    implementation_sha256: "c".repeat(64),
                    hir_sha256: "d".repeat(64),
                    mir_sha256: "e".repeat(64),
                    translator_ir_sha256: "f".repeat(64),
                },
            );
        }
        let mut files = BTreeMap::new();
        files.insert(
            BundleFile::Executable,
            ContentIdentity {
                sha256: "1".repeat(64),
                bytes: 1,
            },
        );
        ModuleNativeBundleRecord {
            schema: "mumei.native-module-bundle/v1".into(),
            profile: MODULE_PROFILE.into(),
            scope: "identity_consistency_only".into(),
            system_scope: "none".into(),
            source: ModuleSourceBinding {
                source_sha256: "2".repeat(64),
                source_profile: "p".into(),
                atoms: bindings,
                hir_encoding: "json".into(),
                mir_encoding: "json".into(),
                translator_version: "v".into(),
                bridge_lemma_hash: "b".into(),
                compiler_version: "c".into(),
                source_dependencies_sha256: "d".into(),
            },
            object_build: ObjectBuild {
                target_triple: "x86_64-unknown-linux-gnu".into(),
                data_layout: "e".into(),
                llvm_version: "17".into(),
            },
            link_build: LinkBuild {
                driver: "cc".into(),
                driver_version: "1".into(),
                driver_target: "x86_64-linux".into(),
                linker: "gnu-ld".into(),
                linker_version: "1".into(),
                arguments_sha256: "args".into(),
                runtime_source_sha256: None,
                canonical_arguments_sha256: None,
                expected_exit_status: None,
                expected_stdout_sha256: None,
                expected_stderr_sha256: None,
                observation_envelope: None,
            },
            settings: BuildSettings::default(),
            leaf_widths: BTreeMap::new(),
            files,
        }
    }

    fn module_report(atoms: &[&str]) -> ModuleValidationReport {
        ModuleValidationReport {
            profile: "mumei.native-module-source-to-llvm-tv/v1",
            source_profile: "mumei.native-module-pure-i64/v1",
            target: "x86_64-unknown-linux-gnu",
            data_layout: "e",
            checked_stage: "llvm",
            input_domain: "i64".into(),
            unroll_bound: None,
            source_sha256: "3".repeat(64),
            llvm_sha256: "4".repeat(64),
            timeout_ms: 10_000,
            resource_limit: 0,
            functions: atoms
                .iter()
                .map(|name| FunctionOutcome {
                    function_name: (*name).to_owned(),
                    outcome: Outcome::EquivalentWithinProfile,
                })
                .collect(),
            outcome: Outcome::EquivalentWithinProfile,
        }
    }

    #[test]
    fn module_evidence_uses_module_profiles_and_bounded_scope_for_loop_atoms() {
        let record = module_record(&["count", "main"]);
        let report = module_report(&["count", "main"]);
        let evidence =
            from_module_recheck(&"5".repeat(64), &record, &report, WHILE_MODULE).unwrap();
        assert_eq!(evidence.evidence.len(), 5);
        assert_eq!(evidence.evidence[0].semantics.profile, MODULE_PROFILE);
        let count_post = evidence
            .evidence
            .iter()
            .find(|record| record.obligation_id == "count_source_postcondition")
            .unwrap();
        assert_eq!(count_post.semantics.profile, MODULE_RECHECK_PROFILE);
        assert_eq!(count_post.scope.kind, "bounded");
        assert_eq!(count_post.scope.bounds.len(), 1);
        let count_tv = evidence
            .evidence
            .iter()
            .find(|record| record.obligation_id == "count_source_to_llvm_user_body")
            .unwrap();
        assert_eq!(count_tv.scope.kind, "bounded");
        let main_post = evidence
            .evidence
            .iter()
            .find(|record| record.obligation_id == "main_source_postcondition")
            .unwrap();
        assert_eq!(main_post.semantics.profile, MODULE_RECHECK_PROFILE);
        assert_eq!(main_post.scope.kind, "unbounded");
        assert!(main_post.scope.bounds.is_empty());
    }

    #[test]
    fn module_evidence_is_unbounded_for_loop_free_atoms() {
        let record = module_record(&["helper", "main"]);
        let report = module_report(&["helper", "main"]);
        let evidence =
            from_module_recheck(&"5".repeat(64), &record, &report, LOOP_FREE_MODULE).unwrap();
        assert!(evidence
            .evidence
            .iter()
            .all(|record| record.scope.kind != "bounded"));
    }
}
