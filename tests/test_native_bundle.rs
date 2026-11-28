#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use mumei_core::native_bundle::{self, BundleFile, RECORD_FILE};
use mumei_core::producer_signature;
use mumei_core::semantic_contract::{extract_pure_i64_module_contracts, RefinedAdmission};
use object::{Object, ObjectSection, ObjectSymbol, SymbolKind};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    process::{Command, Output, Stdio},
};

const GOOD: &str = "atom main() -> i64 requires: true; ensures: result == 7; body: { 7 };";
const INFERRED_I64_MODULE: &str =
    "atom inc(x: i64) requires: x >= 0 && x <= 100; ensures: result == x + 1; body: { x + 1 };\n\
     atom main() -> i64 requires: true; ensures: result == 4; body: { inc(3) };";
const STATEFUL_EFFECT_MODULE: &str = r#"
effect Vault
    states: [Idle, Checked, Effected, Interacted];
    initial: Idle;
    transition check: Idle -> Checked;
    transition update: Checked -> Effected;
    transition interact: Effected -> Interacted;

atom wide_identity(value: i128) -> i128
    requires: true;
    ensures: result == value;
    body: value;

atom main() -> i64
    effects: [Vault];
    effect_pre: { Vault: Idle };
    effect_post: { Vault: Interacted };
    requires: true;
    ensures: result == 7;
    body: {
        perform Vault.check;
        perform Vault.update;
        perform Vault.interact;
        7
    }
"#;
const SINGLE_ATOM_EFFECT_MODULE: &str = r#"
effect Door
    states: [Closed, Open];
    initial: Closed;
    transition open: Closed -> Open;

atom main() -> i64
    effects: [Door];
    effect_pre: { Door: Closed };
    effect_post: { Door: Open };
    requires: true;
    ensures: result == 0;
    body: {
        perform Door.open;
        0
    }
"#;
const ARRAY_PARAM_MODULE: &str = r#"
atom read_at(a: [i64], i: i64) -> i64
requires: i >= 0 && i < len(a);
ensures: result == a[i];
body: a[i];

atom main() -> i64
requires: true;
ensures: result == 0;
body: 0;
"#;
const NON_EXITING_REQUIRES_LOOP_MODULE: &str = r#"
atom helper() -> i64
requires: { let j = 0; while true invariant: j >= 0 decreases: 4 { j = j + 1 }; true };
ensures: result == 0;
body: 0;

atom main() -> i64
requires: true;
ensures: result == 0;
body: 0;
"#;
const BRANCH: &str = "atom absd(x: i64, y: i64) -> i64 requires: x >= 0 && x <= 1000 && y >= 0 && y <= 1000; ensures: result >= 0; body: { if x > y { x - y } else { y - x } };\n\
atom clamp(x: i64) -> i64 requires: true; ensures: result <= 10; body: { if x > 10 { 10 } else { x } };\n\
atom pick(b: bool, x: i64) -> i64 requires: true; ensures: true; body: { if b { x } else { 0 } };\n\
atom band(x: i64, y: i64) -> i64 requires: x >= 0 && x <= 100 && y >= 0 && y <= 100; ensures: true; body: { if x > 5 && y > 5 { x + y } else { 0 } };\n\
atom main() -> i64 requires: true; ensures: result >= 0; body: { absd(7, 3) };";
const SETCC_SHIFTS: &str =
    "atom lt(x: i64, y: i64) -> bool requires: true; ensures: true; body: { x < y };\n\
atom half(x: i64) -> i64 requires: x >= 0 && x <= 1000; ensures: result >= 0; body: { x / 2 };\n\
atom main() -> i64 requires: true; ensures: result >= 0; body: { half(4) };";
const BRANCHING_CALLS: &str =
    "atom helper(x: i64) -> i64 requires: x >= 0 && x <= 100; ensures: result >= 1; body: { x + 1 };\n\
atom cc(x: i64) -> i64 requires: x >= 0 && x <= 100; ensures: true; body: { if x > 5 { helper(x) } else { 0 } };\n\
atom cc_both(x: i64) -> i64 requires: x >= 0 && x <= 100; ensures: result >= 1; body: { if x > 5 { helper(x) } else { helper(5) } };\n\
atom cc_result(x: i64) -> i64 requires: x >= 0 && x <= 100; ensures: true; body: { if helper(x) > 5 { 10 } else { 0 } };\n\
atom main() -> i64 requires: true; ensures: true; body: { cc(1) };";
const DIVISION: &str = "atom md(x: i64, y: i64) -> i64 requires: x >= 0 && x <= 1000 && y >= 1 && y <= 100; ensures: true; body: { x / y };\n\
atom main() -> i64 requires: true; ensures: true; body: { md(6, 2) };";
const VARIABLE_SHIFTS: &str =
    "atom shl_v(x: i64, n: i64) -> i64 requires: n >= 0 && n <= 62 && x >= 0 && x <= 1000; ensures: true; body: { x << n };\n\
atom shr_pos(x: i64, n: i64) -> i64 requires: x >= 0 && x <= 1000 && n >= 0 && n <= 63; ensures: true; body: { x >> n };\n\
atom shr_signed(x: i64, n: i64) -> i64 requires: x >= -1000 && x <= 1000 && n >= 0 && n <= 63; ensures: true; body: { x >> n };\n\
atom shl32_v(x: i32, n: i32) -> i32 requires: x >= 0 && x <= 1000 && n >= 0 && n <= 30; ensures: true; body: { x << n };\n\
atom main() -> i64 requires: true; ensures: true; body: { 0 };";
const CALL_CHAIN: &str =
    "atom chain_leaf(x: i64) -> i64 requires: true; ensures: true; body: { x + 1 };\n\
atom chain_two(x: i64) -> i64 requires: true; ensures: true; body: { chain_leaf(x) + 1 };\n\
atom chain_three(x: i64) -> i64 requires: true; ensures: true; body: { chain_two(x) + 1 };\n\
atom chain_four(x: i64) -> i64 requires: true; ensures: true; body: { chain_three(x) + 1 };\n\
atom main() -> i64 requires: true; ensures: true; body: { chain_four(0) };";
const MANY_LIVE: &str = "atom many_live(a: i64, b: i64, c: i64, d: i64, e: i64, f: i64) -> i64 \
requires: true; ensures: true; body: {\n\
    let v0 = a + b;\n\
    let v1 = b + c;\n\
    let v2 = c + d;\n\
    let v3 = d + e;\n\
    let v4 = e + f;\n\
    let v5 = a + f;\n\
    let v6 = a + c + e;\n\
    let v7 = b + d + f;\n\
    if a < b {\n\
        v0 + v1 + v2 + v3 + v4 + v5 + v6 + v7\n\
    } else {\n\
        v0 - v1 + v2 - v3 + v4 - v5 + v6 - v7\n\
    }\n\
};\n\
atom main() -> i64 requires: true; ensures: true; body: { many_live(1, 2, 3, 4, 5, 6) };";
const STACK_ARGS: &str =
    "atom seven_args(a: i64, b: i64, c: i64, d: i64, e: i64, f: i64, g: i64) -> i64 requires: true; ensures: true; body: { a + b + c + d + e + f + g };\n\
atom calls_seven(x: i64) -> i64 requires: true; ensures: true; body: { seven_args(x, 2, 3, 4, 5, 6, 7) };\n\
atom main() -> i64 requires: true; ensures: true; body: { 0 };";
const STACK_ARGS_PUSH: &str =
    "atom eight_args(a: i64, b: i64, c: i64, d: i64, e: i64, f: i64, g: i64, h: i64) -> i64 requires: true; ensures: true; body: { a + b + c + d + e + f + g + h };\n\
atom calls_eight(x: i64) -> i64 requires: true; ensures: true; body: { eight_args(x, 2, 3, 4, 5, 6, 7, 8) };\n\
atom main() -> i64 requires: true; ensures: true; body: { calls_eight(1) };";
const LEA_SUM: &str =
    "atom lea_sum(x: i64, y: i64) -> i64 requires: true; ensures: true; body: { x + y };\n\
atom main() -> i64 requires: true; ensures: true; body: { lea_sum(1, 2) };";
const WHILE_PROBE: &str =
    "atom loop_count(n: i64) -> i64 requires: n >= 0 && n <= 10; ensures: true; body: { let i = 0; while i < n invariant: i >= 0 && i <= n decreases: n - i { i = i + 1; }; i };\n\
atom main() -> i64 requires: true; ensures: true; body: { loop_count(3) };";

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(args)
        .output()
        .unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_observation_context(descriptor: &serde_json::Value, pin_envelope: &str) {
    assert_eq!(
        descriptor["invocation"],
        serde_json::json!({
            "argv": [],
            "stdin": "closed",
            "timeout_ms": 30000,
            "channel_cap_bytes": 65536,
            "environment": "empty",
            "working_directory": "executable_directory",
        })
    );
    assert_eq!(
        descriptor["host"],
        serde_json::json!({
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "family": std::env::consts::FAMILY,
        })
    );
    assert_eq!(descriptor["pin_envelope"], pin_envelope);
}

fn build(root: &Path) -> (std::path::PathBuf, std::path::PathBuf, String) {
    let source = root.join("source.mm");
    let bundle = root.join("bundle");
    fs::write(&source, GOOD).unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    (source, bundle, id)
}

fn check(source: &Path, bundle: &Path, id: &str) -> Output {
    cli(&[
        "native-bundle",
        "check",
        bundle.to_str().unwrap(),
        source.to_str().unwrap(),
        "--expected-build-id",
        id,
    ])
}

fn recheck(source: &Path, bundle: &Path, id: &str) -> Output {
    consume("recheck-source", source, bundle, id)
}

fn recheck_llvm(source: &Path, bundle: &Path, id: &str) -> Output {
    let text = consume("recheck-llvm", source, bundle, id);
    if !text.status.success() {
        let json = recheck_llvm_json(source, bundle, id);
        assert_eq!(json.status.code(), text.status.code());
        assert!(json.stdout.is_empty());
        assert_eq!(json.stderr, text.stderr);
    }
    text
}

fn recheck_llvm_json(source: &Path, bundle: &Path, id: &str) -> Output {
    consume("recheck-llvm --json", source, bundle, id)
}

fn consume(action: &str, source: &Path, bundle: &Path, id: &str) -> Output {
    let mut args = vec!["native-bundle"];
    args.extend(action.split_whitespace());
    args.extend([
        bundle.to_str().unwrap(),
        source.to_str().unwrap(),
        "--expected-build-id",
        id,
    ]);
    cli(&args)
}

fn recheck_object(source: &Path, bundle: &Path, id: &str, json: bool) -> Output {
    consume(
        if json {
            "recheck-object --json"
        } else {
            "recheck-object"
        },
        source,
        bundle,
        id,
    )
}

fn recheck_machine_tv(source: &Path, bundle: &Path, id: &str, json: bool) -> Output {
    consume(
        if json {
            "recheck-machine-tv --json"
        } else {
            "recheck-machine-tv"
        },
        source,
        bundle,
        id,
    )
}

fn accept_object(source: &Path, bundle: &Path, id: &str, json: bool) -> Output {
    consume(
        if json {
            "accept-object --json"
        } else {
            "accept-object"
        },
        source,
        bundle,
        id,
    )
}

fn object_function_disassembly(bundle: &Path, function_name: &str) -> Vec<String> {
    let bytes = fs::read(bundle.join(BundleFile::Object.file_name())).unwrap();
    let file = object::File::parse(bytes.as_slice()).unwrap();
    let symbol = file
        .symbols()
        .find(|symbol| {
            symbol.name().ok() == Some(function_name)
                && symbol.is_global()
                && symbol.is_definition()
                && symbol.kind() == SymbolKind::Text
        })
        .unwrap();
    let section = file
        .section_by_index(symbol.section_index().unwrap())
        .unwrap();
    mumei_emit_llvm::machine_tv::disassemble(
        section
            .data_range(symbol.address(), symbol.size())
            .unwrap()
            .unwrap(),
        symbol.address(),
    )
    .unwrap()
    .into_iter()
    .map(|instruction| {
        if instruction.operands.is_empty() {
            instruction.mnemonic
        } else {
            format!("{} {}", instruction.mnemonic, instruction.operands)
        }
    })
    .collect()
}

fn repin(bundle: &Path, source: Option<&str>) -> String {
    let mut record: native_bundle::NativeBundleRecord =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    if let Some(source) = source {
        use mumei_core::{proof_cert, verification::ModuleEnv};
        let (binding, atom) = native_bundle::prepare_source(source).unwrap();
        let mut env = ModuleEnv::default();
        env.register_atom(&atom);
        let results = std::collections::HashMap::from([(
            "main".into(),
            (
                proof_cert::status::Z3_UNSAT.into(),
                proof_cert::status::VERIFIED.into(),
            ),
        )]);
        let cert = proof_cert::generate_certificate(
            native_bundle::SOURCE_LABEL,
            &[&atom],
            &results,
            &env,
            None,
            None,
            None,
        );
        fs::write(
            bundle.join("proof-cert.json"),
            serde_json::to_vec(&cert).unwrap(),
        )
        .unwrap();
        record.source = binding;
    }
    for role in BundleFile::ALL {
        record.files.insert(
            role,
            native_bundle::ContentIdentity::of(&fs::read(bundle.join(role.file_name())).unwrap()),
        );
    }
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    native_bundle::sha256(&bytes)
}

#[test]
fn observe_toolchain_json_records_linked_solver_and_toolchain_identity() {
    let output = cli(&["native-bundle", "observe-toolchain", "--json"]);
    assert_success(&output);
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let z3 = mumei_core::verification::linked_z3_version();
    let (llvm_version, target_triple) = mumei_emit_llvm::codegen::linked_llvm_toolchain();

    assert_eq!(
        document["schema_version"],
        "mumei.native-toolchain-observation/v1"
    );
    assert_eq!(
        document["scope"],
        "effective_toolchain_identity_observation_only/no_environment_or_artifact_semantic_attestation"
    );
    assert_eq!(
        document["host"],
        serde_json::json!({
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "family": std::env::consts::FAMILY,
        })
    );
    assert_eq!(document["target_triple"], target_triple);
    assert_eq!(document["solvers"]["z3"]["binding"], "linked");
    assert_eq!(
        document["solvers"]["z3"]["version"],
        format!("{}.{}.{}.{}", z3.0, z3.1, z3.2, z3.3)
    );
    assert_eq!(document["llvm"]["binding"], "linked");
    assert_eq!(document["llvm"]["version"], llvm_version);
    for key in [
        "driver",
        "driver_version",
        "driver_target",
        "linker",
        "linker_version",
    ] {
        assert!(
            document["link"][key]
                .as_str()
                .is_some_and(|value| !value.is_empty()),
            "{key}: {document}"
        );
    }
    assert!(document.get("environment").is_none());
    assert!(document.get("path").is_none());
}

#[test]
fn recheck_object_reports_single_bundle_symbol_surface() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    assert_eq!(module_record(&bundle)["profile"], native_bundle::PROFILE);

    let text = recheck_object(&source, &bundle, &id, false);
    assert_success(&text);
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(text.contains(
        "Native object symbol surface, direct call edges and linked placement correspond to the pinned LLVM IR and executable: 2 defined functions, 1 call edges, 1 resolved call sites."
    ));
    assert!(text.contains(
        "Scope: artifact_reproduction_object_symbols_call_edges_and_link_placement_only/no_instruction_abi_or_runtime_semantics"
    ));

    let output = recheck_object(&source, &bundle, &id, true);
    assert_success(&output);
    let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(descriptor.as_object().unwrap().len(), 9);
    assert_eq!(
        descriptor["schema_version"],
        "mumei.object-symbol-correspondence/v1"
    );
    assert_eq!(
        descriptor["scope"],
        "artifact_reproduction_object_symbols_call_edges_and_link_placement_only/no_instruction_abi_or_runtime_semantics"
    );
    assert_eq!(
        descriptor["defined_functions"],
        serde_json::json!(["__mumei_user_main", "main"])
    );
    assert_eq!(descriptor["external_references"], serde_json::json!([]));
    assert_eq!(
        descriptor["checks_performed"],
        serde_json::json!([
            "toolchain_object_executable_reproduction",
            "object_symbol_correspondence",
            "object_call_edge_correspondence",
            "object_link_correspondence",
        ])
    );
    assert_eq!(
        descriptor["call_edges"],
        serde_json::json!({
            "__mumei_user_main": [],
            "main": ["__mumei_user_main"],
        })
    );
    assert_eq!(descriptor["resolved_call_sites"], 1);
    assert_eq!(
        descriptor["checks_not_performed"],
        serde_json::json!([
            "optimizer_semantics",
            "raw_link_argument_vector_reproduction",
            "abi_runtime_semantics",
            "os_exit_status_observation",
            "os_stdout_observation",
            "os_stderr_observation",
            "runtime_observation_semantics",
        ])
    );
}

#[test]
fn recheck_object_reports_module_and_multifile_symbol_surfaces() {
    let module_root = tempfile::tempdir().unwrap();
    let (module_source, module_bundle, module_id) = build_source(module_root.path(), MODULE);
    let module = recheck_object(&module_source, &module_bundle, &module_id, true);
    assert_success(&module);
    let module_descriptor: serde_json::Value = serde_json::from_slice(&module.stdout).unwrap();
    assert_eq!(
        module_descriptor["defined_functions"],
        serde_json::json!(["__mumei_user_main", "helper", "main"])
    );
    assert_eq!(
        module_descriptor["call_edges"],
        serde_json::json!({
            "__mumei_user_main": ["helper"],
            "helper": [],
            "main": ["__mumei_user_main"],
        })
    );
    assert_eq!(module_descriptor["resolved_call_sites"], 2);
    assert_eq!(
        module_descriptor["external_references"],
        serde_json::json!([])
    );

    let multifile_root = tempfile::tempdir().unwrap();
    let (multifile_source, multifile_bundle, multifile_id) = build_multifile(multifile_root.path());
    let multifile = recheck_object(&multifile_source, &multifile_bundle, &multifile_id, true);
    assert_success(&multifile);
    let multifile_descriptor: serde_json::Value =
        serde_json::from_slice(&multifile.stdout).unwrap();
    assert_eq!(
        multifile_descriptor["defined_functions"],
        serde_json::json!(["__mumei_user_main", "helper", "main"])
    );
    assert_eq!(
        multifile_descriptor["call_edges"],
        serde_json::json!({
            "__mumei_user_main": ["helper"],
            "helper": [],
            "main": ["__mumei_user_main"],
        })
    );
    assert_eq!(multifile_descriptor["resolved_call_sites"], 2);
    let multifile_tv =
        recheck_machine_tv(&multifile_source, &multifile_bundle, &multifile_id, true);
    assert_success(&multifile_tv);
    let multifile_tv_descriptor: serde_json::Value =
        serde_json::from_slice(&multifile_tv.stdout).unwrap();
    assert_eq!(
        multifile_tv_descriptor["functions"],
        serde_json::json!({
            "__mumei_user_main": {
                "outcome": "equivalent_within_profile",
                "callee_saved_preservation": "preserved"
            },
            "helper": {
                "outcome": "equivalent_within_profile",
                "callee_saved_preservation": "preserved"
            },
            "main": {"outcome": "equivalent_within_profile", "callee_saved_preservation": "preserved", "role": "entry_wrapper"},
        })
    );
    assert!(multifile_tv_descriptor["checks_performed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check == "machine_code_direct_call_site_tv"));
}

#[test]
fn recheck_object_rejects_wrong_expected_build_id_like_artifact_recheck() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, _) = build(root.path());
    let wrong_id = "0".repeat(64);
    let output = recheck_object(&source, &bundle, &wrong_id, true);
    let artifact = consume("recheck-artifact", &source, &bundle, &wrong_id);
    assert!(!output.status.success());
    assert_eq!(output.status.code(), artifact.status.code());
    assert!(output.stdout.is_empty());
}

#[test]
fn recheck_machine_tv_reports_good_leaf_and_wrapper_outcomes() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    assert_eq!(
        object_function_disassembly(&bundle, "__mumei_user_main"),
        vec!["movl $7, %eax".to_owned(), "retq".to_owned()]
    );

    let text = recheck_machine_tv(&source, &bundle, &id, false);
    assert_success(&text);
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(text.contains(
        "Machine-code TV: 1 of 2 object functions equivalent to their source atoms within the acyclic-branch, conditional-call, and bounded-loop profile (0 bounded-loop functions)."
    ));
    assert!(text.contains(
        "Scope: artifact_reproduction_object_link_correspondence_and_acyclic_branch_and_conditional_call_function_machine_code_tv_only/no_abi_or_runtime_semantics"
    ));
    assert!(text.contains("Entry wrapper is equivalent within the machine-code TV profile."));
    // Step 5: the text report surfaces the aggregate correspondence
    // status (and would name each inconsistent item) — previously the
    // record existed only in `--json` output.
    assert!(text.contains("Object translation correspondence: checked_correspondence"));

    let output = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&output);
    let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(descriptor["schema_version"], "mumei.machine-code-tv/v1");
    assert_eq!(
        descriptor["scope"],
        "artifact_reproduction_object_link_correspondence_and_acyclic_branch_and_conditional_call_function_machine_code_tv_only/no_abi_or_runtime_semantics"
    );
    assert_eq!(
        descriptor["register_convention_assumed"],
        "sysv_x86_64_integer_args_rdi_rsi_rdx_rcx_r8_r9_result_rax"
    );
    assert_eq!(
        descriptor["input_domain"],
        "signed_i64_inputs_with_signature_bounded_param_domains_where_division_and_shift_defined_not_restricted_by_requires"
    );
    assert_eq!(
        descriptor["functions"],
        serde_json::json!({
            "__mumei_user_main": {
                "outcome": "equivalent_within_profile",
                "callee_saved_preservation": "preserved"
            },
            "main": {"outcome": "equivalent_within_profile", "callee_saved_preservation": "preserved", "role": "entry_wrapper"},
        })
    );
    assert_eq!(
        descriptor["checks_performed"],
        serde_json::json!([
            "toolchain_object_executable_reproduction",
            "object_symbol_correspondence",
            "object_call_edge_correspondence",
            "object_link_correspondence",
            "machine_code_leaf_function_tv",
            "machine_code_callee_saved_register_preservation",
            "machine_code_entry_wrapper_tv",
        ])
    );
    assert_eq!(
        descriptor["checks_not_performed"],
        serde_json::json!([
            "optimizer_semantics",
            "raw_link_argument_vector_reproduction",
            "abi_runtime_semantics",
            "os_exit_status_observation",
            "os_stdout_observation",
            "os_stderr_observation",
            "runtime_observation_semantics",
            "machine_code_direct_call_site_tv",
            "machine_code_forward_branch_function_tv",
            "machine_code_acyclic_branch_function_tv",
            "machine_code_conditional_call_function_tv",
            "machine_code_bounded_loop_function_tv",
            "machine_code_callee_saved_preservation_assumed_for_external_callees",
        ])
    );
    // Step 1 metadata-only object translation record: the producer lists
    // the transformation relations (kept honestly unvalidated or named by
    // the link check), the function correspondence table and the
    // relocation records — without claiming object_translation_correspondence.
    let object_translation = &descriptor["object_translation"];
    // Step 3: every correspondence check item is confirmed on the strict
    // path, so the aggregate status records checked_correspondence (still no
    // verdict-bearing claim).
    assert_eq!(object_translation["status"], "checked_correspondence");
    // Step 3': on a real CLI-produced bundle every defined named IR
    // entity is external, so the symbol-attribute item verifies
    // `checked` end to end — the precheck admits external linkage
    // only, so internal/private functions never reach it.
    let symbol_attribute_item = object_translation["correspondence"]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["item"] == "object_symbol_attribute_correspondence")
        .expect("symbol-attribute item present in the CLI descriptor");
    assert_eq!(symbol_attribute_item["result"], "checked");
    let transformations = object_translation["transformations"].as_array().unwrap();
    assert_eq!(transformations.len(), 4);
    // The digest-bearing edges keep indices 0 and 1 for consumer
    // compatibility; the source->machine and hir->llvm_ir edges are
    // appended after them.
    assert_eq!(transformations[0]["from"], "llvm_ir");
    assert_eq!(transformations[0]["to"], "object_code");
    // `validation` names the IR<->object model comparison; `result`
    // reports the memo's promotion conditions: the bundle's liftable
    // functions compared equivalent and every gate item ran.
    assert_eq!(
        transformations[0]["validation"],
        "checked_llvm_ir_object_model_correspondence"
    );
    assert_eq!(transformations[0]["result"], "checked_correspondence");
    assert_eq!(transformations[1]["from"], "object_code");
    assert_eq!(transformations[1]["to"], "linked_executable");
    // `validation` names the method; the check's outcome is a separate key.
    assert_eq!(
        transformations[1]["validation"],
        "checked_object_link_correspondence"
    );
    assert_eq!(transformations[1]["result"], "checked_correspondence");
    assert_eq!(transformations[2]["from"], "source_atom");
    assert_eq!(transformations[2]["to"], "machine_code");
    assert_eq!(
        transformations[2]["relation"],
        "machine_code_translation_validation"
    );
    // Machine code is evaluated in memory and never persisted, so its
    // edge carries no artifact digest.
    assert!(transformations[2]["output_identity"].is_null());
    // The hir->llvm_ir edge names the HIR-anchored model comparison;
    // `result` mirrors the check item: every in-subset atom proved
    // equivalent on this bundle.
    assert_eq!(transformations[3]["from"], "hir");
    assert_eq!(transformations[3]["to"], "llvm_ir");
    assert_eq!(transformations[3]["relation"], "existing_llvm_emitter");
    assert_eq!(
        transformations[3]["validation"],
        "checked_hir_llvm_model_correspondence"
    );
    assert_eq!(transformations[3]["result"], "checked_correspondence");
    assert_eq!(
        transformations[2]["validation"],
        "checked_source_machine_tv_within_profile"
    );
    let functions = object_translation["functions"].as_array().unwrap();
    assert_eq!(functions.len(), 2);
    let main_record = functions
        .iter()
        .find(|function| function["object_symbol"] == "__mumei_user_main")
        .unwrap();
    assert_eq!(main_record["object_section"], ".text");
    assert!(main_record["object_size"].as_u64().unwrap() > 0);
    assert!(main_record["executable_address"].as_u64().is_some());
    let relocations = object_translation["relocations"].as_array().unwrap();
    assert!(!relocations.is_empty());
    // Step 4 (design memo §7): every relocation entry is recorded —
    // call-site fields in `.text` plus the `.eh_frame` unwind records
    // the object always carries, each resolvable against the exe.
    for relocation in relocations {
        assert!(relocation["object_section"].as_str().is_some());
        assert!(relocation["object_offset"].as_u64().is_some());
        assert!(relocation["kind"].as_str().is_some());
        assert!(relocation["target_symbol"].as_str().is_some());
        assert!(relocation["addend"].as_i64().is_some());
    }
    let sections: BTreeSet<&str> = relocations
        .iter()
        .map(|relocation| relocation["object_section"].as_str().unwrap())
        .collect();
    assert!(sections.contains(".text"));
    assert!(sections.contains(".eh_frame"));
    let eh_frame = relocations
        .iter()
        .filter(|relocation| relocation["object_section"] == ".eh_frame")
        .collect::<Vec<_>>();
    assert!(!eh_frame.is_empty());
    for relocation in &eh_frame {
        assert_eq!(relocation["kind"], "R_X86_64_PC32");
        assert!(
            relocation["resolved_value"].as_i64().is_some(),
            "eh_frame relocation unresolved: {relocation}"
        );
        // The applied field's exe-side offset is recorded so the
        // check re-reads the exact slot, not any matching word.
        assert!(
            relocation["applied_offset"].as_u64().is_some(),
            "eh_frame relocation has no applied offset: {relocation}"
        );
    }
    // Step 2/3: the strict path confirms every correspondence check item,
    // so the aggregate status records checked_correspondence.
    let correspondence = &object_translation["correspondence"];
    assert_eq!(correspondence["status"], "checked_correspondence");
    let results: BTreeMap<String, &str> = correspondence["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|check| {
            (
                check["item"].as_str().unwrap().to_owned(),
                check["result"].as_str().unwrap(),
            )
        })
        .collect();
    for item in [
        "function_coverage",
        "function_count",
        "relocation_targets",
        "relocation_bounds",
        "relocation_resolved_values",
        "relocation_applied_values",
    ] {
        assert_eq!(results.get(item).copied(), Some("checked"), "{item}");
    }
    // Step 4 (design memo §7 direct comparison): the leaf atom's IR
    // encoding and its object code compare equivalent over the shared
    // input model — recorded as an inspected item, no verdict change.
    assert_eq!(
        results.get("llvm_ir_object_equivalence").copied(),
        Some("checked")
    );
    // Design memo §7 Step 2: the within-profile gate is emitted as
    // metadata — every source atom (and the entry wrapper) compared
    // equivalent, so the gate records checked; the residual unchecked
    // domains mirror the descriptor's checks_not_performed.
    let source_machine_tv = &descriptor["source_machine_tv"];
    assert_eq!(
        source_machine_tv["status"],
        "checked_source_machine_tv_within_profile"
    );
    assert_eq!(
        source_machine_tv["non_equivalent_functions"],
        serde_json::json!([])
    );
    // The gate covers source↔machine equivalence only; the link edge's
    // own status is named separately.
    assert_eq!(
        source_machine_tv["object_link_correspondence"],
        "checked_object_link_correspondence"
    );
    assert_eq!(
        source_machine_tv["unchecked_domains"],
        descriptor["checks_not_performed"]
    );
}

#[test]
fn hir_llvm_correspondence_models_composite_call_sites() {
    // A call chain through struct-carrying signatures — `sum(mk(1, 2))`
    // and the `sum(p)` pass-through — flattens to the same leaf slots
    // the IR `call` encoding splices, so these atoms verify inside the
    // leaf subset instead of diverging on scalarised call terms.
    for source_text in [
        "struct P { x: i64, y: i64 }\n\
         atom sum(p: P) -> i64 requires: true; ensures: true; body: { p.x + p.y };\n\
         atom mk(a: i64, b: i64) -> P requires: true; ensures: true; body: { P { x: a, y: b } };\n\
         atom caller() -> i64 requires: true; ensures: true; body: { sum(mk(1, 2)) };\n\
         atom fwd(p: P) -> i64 requires: true; ensures: true; body: { sum(p) };\n\
         atom main() -> i64 requires: true; ensures: true; body: { 0 };\n",
        "struct T { a: i64, b: i64, c: i64 }\n\
         atom sum3(p: T) -> i64 requires: true; ensures: true; body: { p.a + p.b + p.c };\n\
         atom mk3(a: i64, b: i64, c: i64) -> T requires: true; ensures: true; body: { T { a: a, b: b, c: c } };\n\
         atom caller() -> i64 requires: true; ensures: true; body: { sum3(mk3(1, 2, 3)) };\n\
         atom fwd(p: T) -> i64 requires: true; ensures: true; body: { sum3(p) };\n\
         atom main() -> i64 requires: true; ensures: true; body: { 0 };\n",
    ] {
        let root = tempfile::tempdir().unwrap();
        let (source, bundle, id) = build_source(root.path(), source_text);
        let output = recheck_machine_tv(&source, &bundle, &id, true);
        assert_success(&output);
        let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let hir_check = descriptor["object_translation"]["correspondence"]["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["item"] == "hir_llvm_model_correspondence")
            .unwrap();
        assert_eq!(hir_check["result"], "checked");
        assert_eq!(
            hir_check["detail"],
            "5 of 5 atoms equivalent within the leaf subset (0 outside)"
        );
        // The full acceptance gate accepts the module bundle.
        let accepted = accept_object(&source, &bundle, &id, false);
        assert_success(&accepted);
    }
}

#[test]
fn hir_llvm_correspondence_covers_main_callers_and_generic_structs() {
    // `caller` calls `main()`, which the bundle emitter renames to
    // `__mumei_user_main` — the model comparison must meet the emitted
    // member, not the source name. `pair_sum` takes `Pair<i64>`: the
    // generic declaration's `T` specializes from the atom's own
    // signature so a concrete all-i64 instantiation stays inside the
    // leaf subset.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "struct Pair<T> { a: T, b: T }\n\
         atom pair_sum(p: Pair<i64>) -> i64 requires: true; ensures: true; body: { p.a + p.b };\n\
         atom main() -> i64 requires: true; ensures: true; body: { 7 };\n\
         atom caller() -> i64 requires: true; ensures: true; body: { main() };\n",
    );
    let output = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&output);
    let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let checks = descriptor["object_translation"]["correspondence"]["checks"]
        .as_array()
        .unwrap();
    let hir_check = checks
        .iter()
        .find(|check| check["item"] == "hir_llvm_model_correspondence")
        .unwrap();
    assert_eq!(hir_check["result"], "checked");
    assert_eq!(
        hir_check["detail"],
        "3 of 3 atoms equivalent within the leaf subset (0 outside)"
    );
    // The edge's result mirrors the check item's aggregate.
    let transformations = descriptor["object_translation"]["transformations"]
        .as_array()
        .unwrap();
    let hir_edge = transformations
        .iter()
        .find(|edge| edge["from"] == "hir" && edge["to"] == "llvm_ir")
        .unwrap();
    assert_eq!(hir_edge["result"], "checked_correspondence");
    // The llvm_ir->object_code edge's result mirrors the
    // llvm_ir_object_equivalence item the same way.
    let object_edge = transformations
        .iter()
        .find(|edge| edge["from"] == "llvm_ir" && edge["to"] == "object_code")
        .unwrap();
    assert_eq!(object_edge["result"], "checked_correspondence");
    assert_eq!(
        object_edge["validation"],
        "checked_llvm_ir_object_model_correspondence"
    );
}

#[test]
fn recheck_machine_tv_reports_module_direct_call_and_wrapper_outcomes() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), MODULE);
    assert_eq!(
        object_function_disassembly(&bundle, "helper"),
        vec!["leaq 1(%rdi), %rax".to_owned(), "retq".to_owned()]
    );
    let main_disassembly = object_function_disassembly(&bundle, "__mumei_user_main");
    assert_eq!(
        main_disassembly,
        vec![
            "pushq %rax".to_owned(),
            "movl $5, %edi".to_owned(),
            "callq 0".to_owned(),
            "popq %rcx".to_owned(),
            "retq".to_owned(),
        ]
    );
    let output = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&output);
    let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        descriptor["functions"],
        serde_json::json!({
            "__mumei_user_main": {
                "outcome": "equivalent_within_profile",
                "callee_saved_preservation": "preserved"
            },
            "helper": {
                "outcome": "equivalent_within_profile",
                "callee_saved_preservation": "preserved"
            },
            "main": {"outcome": "equivalent_within_profile", "callee_saved_preservation": "preserved", "role": "entry_wrapper"},
        })
    );
    assert!(descriptor["checks_performed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check == "machine_code_direct_call_site_tv"));
    assert!(descriptor["checks_performed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check == "machine_code_callee_saved_register_preservation"));
    assert!(descriptor["checks_performed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check == "machine_code_entry_wrapper_tv"));
    assert!(descriptor["checks_not_performed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check == "machine_code_forward_branch_function_tv"));
    assert!(!descriptor["checks_not_performed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check == "machine_code_tv_cyclic_control_flow_functions"));
}

#[test]
fn recheck_machine_tv_checks_forward_and_acyclic_branches() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), BRANCH);
    for (symbol, expected) in [
        (
            "absd",
            vec!["movq", "subq", "jle", "retq", "subq", "movq", "retq"],
        ),
        ("clamp", vec!["movl", "cmpq", "jle", "retq", "movq", "retq"]),
        ("pick", vec!["movq", "testq", "je", "retq", "xorl", "retq"]),
        (
            "band",
            vec![
                "movq", "cmpq", "jl", "cmpq", "setge", "testb", "je", "addq", "retq", "xorl",
                "testb", "jne", "xorl", "retq",
            ],
        ),
    ] {
        let disassembly = object_function_disassembly(&bundle, symbol);
        assert_eq!(
            disassembly
                .iter()
                .map(|instruction| instruction.split_whitespace().next().unwrap())
                .collect::<Vec<_>>(),
            expected
        );
    }
    let output = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&output);
    let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        descriptor["functions"],
        serde_json::json!({
            "__mumei_user_main": {
                "outcome": "equivalent_within_profile",
                "callee_saved_preservation": "preserved"
            },
            "absd": {
                "outcome": "equivalent_within_profile",
                "callee_saved_preservation": "preserved"
            },
            "band": {
                "outcome": "equivalent_within_profile",
                "callee_saved_preservation": "preserved"
            },
            "clamp": {
                "outcome": "equivalent_within_profile",
                "callee_saved_preservation": "preserved"
            },
            "main": {"outcome": "equivalent_within_profile", "callee_saved_preservation": "preserved", "role": "entry_wrapper"},
            "pick": {
                "outcome": "equivalent_within_profile",
                "callee_saved_preservation": "preserved"
            },
        })
    );
    let performed = descriptor["checks_performed"].as_array().unwrap();
    assert!(performed
        .iter()
        .any(|check| check == "machine_code_forward_branch_function_tv"));
    assert!(performed
        .iter()
        .any(|check| check == "machine_code_acyclic_branch_function_tv"));
    assert!(performed
        .iter()
        .any(|check| check == "machine_code_direct_call_site_tv"));
    let not_performed = descriptor["checks_not_performed"].as_array().unwrap();
    assert!(!not_performed
        .iter()
        .any(|check| check == "machine_code_tv_cyclic_control_flow_functions"));
    assert!(not_performed
        .iter()
        .any(|check| check == "machine_code_leaf_function_tv"));
}

#[test]
fn recheck_machine_tv_checks_integer_division() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), DIVISION);
    let disassembly = object_function_disassembly(&bundle, "md");
    assert_eq!(
        disassembly
            .iter()
            .map(|instruction| instruction.split_whitespace().next().unwrap())
            .collect::<Vec<_>>(),
        ["movq", "movq", "orq", "shrq", "je", "cqto", "idivq", "retq", "xorl", "divl", "retq",]
    );
    let output = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&output);
    let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        descriptor["functions"]["md"]["outcome"],
        "equivalent_within_profile"
    );
    assert_eq!(
        descriptor["functions"]["__mumei_user_main"]["outcome"],
        "equivalent_within_profile"
    );
    assert_eq!(
        descriptor["functions"]["main"]["outcome"],
        "equivalent_within_profile"
    );
    assert_eq!(descriptor["functions"]["main"]["role"], "entry_wrapper");
}

#[test]
fn recheck_machine_tv_checks_branching_calls() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), BRANCHING_CALLS);
    for (symbol, expected) in [
        (
            "cc",
            vec![
                "cmpq", "jl", "pushq", "callq", "addq", "retq", "xorl", "retq",
            ],
        ),
        (
            "cc_both",
            vec!["pushq", "cmpq", "jge", "movl", "callq", "popq", "retq"],
        ),
        (
            "cc_result",
            vec![
                "pushq", "callq", "movq", "movl", "cmpq", "jle", "popq", "retq", "xorl", "popq",
                "retq",
            ],
        ),
    ] {
        assert_eq!(
            object_function_disassembly(&bundle, symbol)
                .iter()
                .map(|instruction| instruction.split_whitespace().next().unwrap())
                .collect::<Vec<_>>(),
            expected
        );
    }
    let output = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&output);
    let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        descriptor["functions"],
        serde_json::json!({
            "__mumei_user_main": {
                "outcome": "equivalent_within_profile",
                "callee_saved_preservation": "preserved"
            },
            "cc": {
                "outcome": "equivalent_within_profile",
                "callee_saved_preservation": "preserved"
            },
            "cc_both": {
                "outcome": "equivalent_within_profile",
                "callee_saved_preservation": "preserved"
            },
            "cc_result": {
                "outcome": "equivalent_within_profile",
                "callee_saved_preservation": "preserved"
            },
            "helper": {
                "outcome": "equivalent_within_profile",
                "callee_saved_preservation": "preserved"
            },
            "main": {"outcome": "equivalent_within_profile", "callee_saved_preservation": "preserved", "role": "entry_wrapper"},
        })
    );
    let performed = descriptor["checks_performed"].as_array().unwrap();
    assert!(performed
        .iter()
        .any(|check| check == "machine_code_direct_call_site_tv"));
    assert!(performed
        .iter()
        .any(|check| check == "machine_code_conditional_call_function_tv"));
    assert!(!performed
        .iter()
        .any(|check| check == "machine_code_forward_branch_function_tv"));
    assert!(!performed
        .iter()
        .any(|check| check == "machine_code_acyclic_branch_function_tv"));
    let not_performed = descriptor["checks_not_performed"].as_array().unwrap();
    assert!(not_performed
        .iter()
        .any(|check| check == "machine_code_forward_branch_function_tv"));
    assert!(not_performed
        .iter()
        .any(|check| check == "machine_code_acyclic_branch_function_tv"));
}

#[test]
fn recheck_machine_tv_checks_stack_passed_arguments() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), STACK_ARGS);
    assert!(object_function_disassembly(&bundle, "seven_args")
        .iter()
        .any(|instruction| instruction.contains("(%rsp)")));
    assert!(object_function_disassembly(&bundle, "calls_seven")
        .iter()
        .any(|instruction| instruction.contains("(%rsp)")));

    let output = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&output);
    let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    for name in ["seven_args", "calls_seven"] {
        assert_eq!(
            descriptor["functions"][name],
            serde_json::json!({
                "outcome": "equivalent_within_profile",
                "callee_saved_preservation": "preserved"
            }),
            "{name}"
        );
    }
}

#[test]
fn recheck_machine_tv_checks_pushed_immediate_stack_arguments() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), STACK_ARGS_PUSH);
    // Two outgoing stack argument words: LLVM passes them as `pushq $imm`.
    assert!(object_function_disassembly(&bundle, "calls_eight")
        .iter()
        .any(|instruction| instruction.starts_with("pushq $")));

    let output = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&output);
    let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    for name in ["eight_args", "calls_eight", "__mumei_user_main"] {
        assert_eq!(
            descriptor["functions"][name],
            serde_json::json!({
                "outcome": "equivalent_within_profile",
                "callee_saved_preservation": "preserved"
            }),
            "{name}"
        );
    }
}

#[test]
fn recheck_machine_tv_checks_setcc_and_shift_probe() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), SETCC_SHIFTS);
    let lt_disassembly = object_function_disassembly(&bundle, "lt");
    assert_eq!(
        lt_disassembly
            .iter()
            .map(|instruction| instruction.split_whitespace().next().unwrap())
            .collect::<Vec<_>>(),
        ["xorl", "cmpq", "setl", "retq"]
    );
    let half_disassembly = object_function_disassembly(&bundle, "half");
    assert_eq!(
        half_disassembly
            .iter()
            .map(|instruction| instruction.split_whitespace().next().unwrap())
            .collect::<Vec<_>>(),
        ["movq", "shrq", "addq", "sarq", "retq"]
    );
    assert!(half_disassembly
        .iter()
        .any(|instruction| instruction == "sarq %rax"));

    let output = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&output);
    let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        descriptor["functions"]["lt"],
        serde_json::json!({
            "outcome": "equivalent_within_profile",
            "callee_saved_preservation": "preserved"
        })
    );
    assert_eq!(
        descriptor["functions"]["half"],
        serde_json::json!({
            "outcome": "equivalent_within_profile",
            "callee_saved_preservation": "preserved"
        })
    );
}

#[test]
fn recheck_machine_tv_checks_variable_count_shifts() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), VARIABLE_SHIFTS);
    let shl_disassembly = object_function_disassembly(&bundle, "shl_v");
    assert_eq!(
        shl_disassembly
            .iter()
            .map(|instruction| instruction.split_whitespace().next().unwrap())
            .collect::<Vec<_>>(),
        ["movq", "movq", "shlq", "retq"]
    );
    assert!(shl_disassembly
        .iter()
        .any(|instruction| instruction.starts_with("shlq") && instruction.contains("%cl")));

    let output = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&output);
    let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    for name in ["shl_v", "shr_pos", "shr_signed", "shl32_v"] {
        assert_eq!(
            descriptor["functions"][name],
            serde_json::json!({
                "outcome": "equivalent_within_profile",
                "callee_saved_preservation": "preserved"
            }),
            "{name}"
        );
    }
}

#[test]
fn recheck_machine_tv_checks_unary_ops_and_base_index_lea() {
    let chain_root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(chain_root.path(), CALL_CHAIN);
    let output = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&output);
    let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    for name in ["chain_leaf", "chain_two", "chain_three", "chain_four"] {
        assert_eq!(
            descriptor["functions"][name],
            serde_json::json!({
                "outcome": "equivalent_within_profile",
                "callee_saved_preservation": "preserved"
            }),
            "{name}"
        );
    }

    let lea_root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(lea_root.path(), LEA_SUM);
    assert_eq!(
        object_function_disassembly(&bundle, "lea_sum"),
        ["leaq (%rdi,%rsi), %rax", "retq"]
    );
    let output = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&output);
    let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        descriptor["functions"]["lea_sum"],
        serde_json::json!({
            "outcome": "equivalent_within_profile",
            "callee_saved_preservation": "preserved"
        })
    );
}

#[test]
fn recheck_machine_tv_checks_many_live_with_solver_fallback() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), MANY_LIVE);
    let output = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&output);
    let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        descriptor["functions"]["many_live"],
        serde_json::json!({
            "outcome": "equivalent_within_profile",
            "callee_saved_preservation": "preserved"
        })
    );
    assert_eq!(
        descriptor["functions"]["__mumei_user_main"],
        serde_json::json!({
            "outcome": "equivalent_within_profile",
            "callee_saved_preservation": "preserved"
        })
    );
}

#[test]
fn recheck_machine_tv_rejects_wrong_expected_build_id_like_recheck_object() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, _) = build(root.path());
    let wrong_id = "0".repeat(64);
    let output = recheck_machine_tv(&source, &bundle, &wrong_id, true);
    let object = recheck_object(&source, &bundle, &wrong_id, true);
    assert!(!output.status.success());
    assert_eq!(output.status.code(), object.status.code());
    assert!(output.stdout.is_empty());
}

#[test]
fn clean_consumer_rechecks_copied_read_only_snapshots_without_producer_tools_or_execution() {
    use std::os::unix::fs::PermissionsExt;
    let producer = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(producer.path());
    let consumer = tempfile::tempdir().unwrap();
    let saved = save_evidence(consumer.path(), &source, &bundle, &id);
    fs::set_permissions(&saved, fs::Permissions::from_mode(0o400)).unwrap();
    let executable = consumer.path().join("mumei");
    fs::copy(env!("CARGO_BIN_EXE_mumei"), &executable).unwrap();
    let copied_source = consumer.path().join("source.mm");
    fs::copy(source, &copied_source).unwrap();
    let copied_bundle = consumer.path().join("bundle");
    fs::create_dir(&copied_bundle).unwrap();
    for name in
        std::iter::once(RECORD_FILE).chain(BundleFile::ALL.iter().map(|role| role.file_name()))
    {
        fs::copy(bundle.join(name), copied_bundle.join(name)).unwrap();
        fs::set_permissions(copied_bundle.join(name), fs::Permissions::from_mode(0o400)).unwrap();
    }
    fs::set_permissions(&copied_source, fs::Permissions::from_mode(0o400)).unwrap();
    fs::set_permissions(&copied_bundle, fs::Permissions::from_mode(0o500)).unwrap();
    producer.close().unwrap();
    fs::write(
        consumer.path().join("mumei.toml"),
        "not valid project configuration [",
    )
    .unwrap();
    let home = consumer.path().join("home");
    fs::create_dir(&home).unwrap();
    for (action, json) in [
        ("recheck-source", false),
        ("recheck-llvm", false),
        ("recheck-llvm", true),
        ("recheck-evidence", false),
    ] {
        let mut command = Command::new(&executable);
        command
            .env_clear()
            .current_dir(consumer.path())
            .env("HOME", &home)
            .env("PATH", "/nonexistent-consumer-tools");
        if let Some(path) = std::env::var_os("LD_LIBRARY_PATH") {
            command.env("LD_LIBRARY_PATH", path);
        }
        let result = command
            .args([
                "native-bundle",
                action,
                copied_bundle.to_str().unwrap(),
                copied_source.to_str().unwrap(),
                "--expected-build-id",
                &id,
            ])
            .args((action == "recheck-evidence").then_some(saved.to_str().unwrap()))
            .args(json.then_some("--json"))
            .output()
            .unwrap();
        assert_success(&result);
        let stdout = String::from_utf8_lossy(&result.stdout);
        if action == "recheck-source" {
            assert!(stdout.contains(native_bundle::RECHECK_SCOPE));
            assert!(stdout.contains(native_bundle::RECHECK_PROFILE));
        } else if json {
            let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
            assert_eq!(report["acceptance"], "not_evaluated");
            assert_eq!(report["evidence"].as_array().unwrap().len(), 3);
        } else if action == "recheck-evidence" {
            assert!(stdout.contains("Saved native Evidence consistent with fresh"));
            assert!(stdout.contains("Evidence acceptance: not evaluated"));
        } else {
            assert!(stdout.contains(mumei_emit_llvm::translation_validation::NATIVE_PROFILE));
            assert!(stdout.contains("source_postcondition_identity_and_llvm_snapshot_only"));
            assert!(stdout.contains("OS exit-status/executable semantics: not checked"));
        }
        assert!(!stdout.contains("artifact_verified"));
        assert_eq!(fs::read_dir(&copied_bundle).unwrap().count(), 5);
        assert_eq!(fs::read_dir(&home).unwrap().count(), 0);
        assert_eq!(fs::read_dir(consumer.path()).unwrap().count(), 6);
    }
    fs::set_permissions(&copied_bundle, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn self_consistent_forged_source_certificate_is_not_a_source_proof() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, _) = build(root.path());
    for text in [
        GOOD.replace("== 7", "== 8"),
        GOOD.replace("== 7", "== 1234567890123456"),
        GOOD.replace("result == 7", "false"),
    ] {
        fs::write(&source, &text).unwrap();
        let id = repin(&bundle, Some(&text));
        assert_success(&check(&source, &bundle, &id));
        for action in ["recheck-source", "recheck-llvm", "recheck-llvm --json"] {
            let result = consume(action, &source, &bundle, &id);
            assert_eq!(result.status.code(), Some(1));
            assert!(result.stdout.is_empty());
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert!(stderr.contains("source postcondition refuted"));
            assert!(!stderr.contains("1234567890123456"));
            assert!(!String::from_utf8_lossy(&result.stdout).contains("1234567890123456"));
        }
    }
}

#[test]
fn source_recheck_rejects_missing_tampered_and_unpinned_members_before_success() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    for name in
        std::iter::once(RECORD_FILE).chain(BundleFile::ALL.iter().map(|role| role.file_name()))
    {
        let path = bundle.join(name);
        let bytes = fs::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert!(!recheck(&source, &bundle, &id).status.success());
        assert!(!recheck_llvm(&source, &bundle, &id).status.success());
        fs::write(&path, b"tampered").unwrap();
        assert!(!recheck(&source, &bundle, &id).status.success());
        assert!(!recheck_llvm(&source, &bundle, &id).status.success());
        fs::write(path, bytes).unwrap();
    }
    assert!(!recheck(&source, &bundle, &"0".repeat(64)).status.success());
    assert!(!recheck_llvm(&source, &bundle, &"0".repeat(64))
        .status
        .success());
    fs::write(&source, format!("{GOOD}\n// stale source")).unwrap();
    assert!(!recheck(&source, &bundle, &id).status.success());
    assert!(!recheck_llvm(&source, &bundle, &id).status.success());
}

#[test]
fn authorized_non_executable_bytes_are_not_promoted_to_binary_assurance() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, old_id) = build(root.path());
    let marker = root.path().join("must-not-run");
    fs::write(
        bundle.join("program"),
        format!("#!/bin/sh\ntouch '{}'\nexit 99\n", marker.display()),
    )
    .unwrap();
    assert!(!recheck(&source, &bundle, &old_id).status.success());
    let id = repin(&bundle, None);
    let result = recheck(&source, &bundle, &id);
    assert_success(&result);
    assert!(String::from_utf8_lossy(&result.stdout).contains("executable semantics: not checked"));
    fs::write(bundle.join("program.o"), b"repinned non-object bytes").unwrap();
    let id = repin(&bundle, None);
    let result = recheck_llvm(&source, &bundle, &id);
    assert_success(&result);
    assert!(String::from_utf8_lossy(&result.stdout).contains("executable semantics: not checked"));
    assert!(!marker.exists());
}

#[test]
fn canonical_link_arguments_are_pinned_and_reproduced() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let mut record: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    // New records pin the canonical argument vector — reproducible by a
    // recheck on fresh staging paths, unlike the raw argv.
    let canonical = record["link_build"]["canonical_arguments_sha256"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(canonical.len(), 64);
    assert!(canonical.chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(
        canonical,
        record["link_build"]["arguments_sha256"].as_str().unwrap()
    );
    let artifact = |source: &Path, bundle: &Path, id: &str| {
        Command::new(env!("CARGO_BIN_EXE_mumei"))
            .args([
                "native-bundle",
                "recheck-artifact",
                bundle.to_str().unwrap(),
                source.to_str().unwrap(),
                "--expected-build-id",
                id,
            ])
            .output()
            .unwrap()
    };
    assert_success(&artifact(&source, &bundle, &id));
    // …and the acceptance descriptor claims the comparison that ran.
    let output = consume("accept-artifact --json", &source, &bundle, &id);
    assert_success(&output);
    let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(descriptor["checks_performed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry == "canonical_link_argument_vector_reproduction"));

    // A record pinned before the field existed carries no pin — the
    // recheck skips the comparison honestly rather than failing, and the
    // acceptance descriptor names the check among the ones not performed
    // rather than claiming it.
    record["link_build"]
        .as_object_mut()
        .unwrap()
        .remove("canonical_arguments_sha256");
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    let unpinned_id = native_bundle::sha256(&bytes);
    assert_success(&artifact(&source, &bundle, &unpinned_id));
    let output = consume("accept-artifact --json", &source, &bundle, &unpinned_id);
    assert_success(&output);
    let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(!descriptor["checks_performed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry == "canonical_link_argument_vector_reproduction"));
    assert!(descriptor["checks_not_performed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry == "canonical_link_argument_vector_reproduction"));

    // The recheck still succeeds on the unpinned record — but the
    // llvm_ir -> object_code edge must not report checked, because the
    // argv reproduction the promotion conditions require did not run.
    let output = recheck_machine_tv(&source, &bundle, &unpinned_id, true);
    assert_success(&output);
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let edge = &report["object_translation"]["transformations"][0];
    assert_eq!(edge["from"], "llvm_ir");
    assert_eq!(edge["to"], "object_code");
    assert_eq!(
        edge["validation"].as_str().unwrap(),
        "checked_llvm_ir_object_model_correspondence"
    );
    assert_eq!(
        edge["result"].as_str().unwrap(),
        "unchecked_not_translation_validated"
    );

    // …and accept-object cannot accept either: the canonical argv
    // comparison is an unconditional acceptance precondition, so the
    // unpinned record is inconclusive with no descriptor.
    let output = accept_object(&source, &bundle, &unpinned_id, true);
    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Canonical link argument vector"));

    // A wrong pin fails the reproduction check.
    record["link_build"]["canonical_arguments_sha256"] = serde_json::json!("0".repeat(64));
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!artifact(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn exit_status_is_pinned_and_reproduced() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let mut record: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    // New records pin the executable's observed exit status — `main`
    // returns 7 here — so a recheck that executes the byte-identical
    // reproduction compares against the pin.
    assert_eq!(record["link_build"]["expected_exit_status"], 7);
    let exit_status = |source: &Path, bundle: &Path, id: &str| {
        Command::new(env!("CARGO_BIN_EXE_mumei"))
            .args([
                "native-bundle",
                "recheck-exit-status",
                bundle.to_str().unwrap(),
                source.to_str().unwrap(),
                "--expected-build-id",
                id,
            ])
            .output()
            .unwrap()
    };
    assert_success(&exit_status(&source, &bundle, &id));

    // A record pinned before the field existed carries no pin — the
    // recheck cannot evaluate the comparison and reports inconclusive.
    record["link_build"]
        .as_object_mut()
        .unwrap()
        .remove("expected_exit_status");
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert_eq!(
        exit_status(&source, &bundle, &native_bundle::sha256(&bytes))
            .status
            .code(),
        Some(3)
    );

    // A wrong pin fails the reproduction check.
    record["link_build"]["expected_exit_status"] = serde_json::json!(0);
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(
        !exit_status(&source, &bundle, &native_bundle::sha256(&bytes))
            .status
            .success()
    );
}

#[test]
fn exit_status_acceptance_emits_the_observation_descriptor() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let accept = cli(&[
        "native-bundle",
        "accept-exit-status",
        bundle.to_str().unwrap(),
        source.to_str().unwrap(),
        "--expected-build-id",
        &id,
        "--json",
    ]);
    assert_success(&accept);
    let descriptor: serde_json::Value = serde_json::from_slice(&accept.stdout).unwrap();
    assert_eq!(descriptor["accepted"], true);
    assert_observation_context(&descriptor, "recorded");
    assert!(
        descriptor["checks_performed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "os_exit_status_observation"),
        "exit-status acceptance must claim the OS observation"
    );
    assert!(
        descriptor["checks_not_performed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "historical_execution"),
        "one run stays an observation, never a history"
    );
    assert!(
        !descriptor["checks_performed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "abi_runtime_semantics"),
        "no ABI/runtime claim"
    );

    // A pinless record cannot be accepted this way — inconclusive.
    let mut record: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    record["link_build"]
        .as_object_mut()
        .unwrap()
        .remove("expected_exit_status");
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    let pinless = cli(&[
        "native-bundle",
        "accept-exit-status",
        bundle.to_str().unwrap(),
        source.to_str().unwrap(),
        "--expected-build-id",
        &native_bundle::sha256(&bytes),
    ]);
    assert_eq!(pinless.status.code(), Some(3));
}

#[test]
fn stdout_is_pinned_and_reproduced() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let mut record: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    // New records pin the executable's observed stdout digest — the
    // profile's `main` is silent, so the pin is the empty-stream digest.
    assert_eq!(
        record["link_build"]["expected_stdout_sha256"],
        native_bundle::sha256(b"")
    );
    let stdout_recheck = |source: &Path, bundle: &Path, id: &str| {
        Command::new(env!("CARGO_BIN_EXE_mumei"))
            .args([
                "native-bundle",
                "recheck-stdout",
                bundle.to_str().unwrap(),
                source.to_str().unwrap(),
                "--expected-build-id",
                id,
            ])
            .output()
            .unwrap()
    };
    assert_success(&stdout_recheck(&source, &bundle, &id));

    // A record pinned before the field existed carries no pin — the
    // recheck cannot evaluate the comparison and reports inconclusive.
    record["link_build"]
        .as_object_mut()
        .unwrap()
        .remove("expected_stdout_sha256");
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert_eq!(
        stdout_recheck(&source, &bundle, &native_bundle::sha256(&bytes))
            .status
            .code(),
        Some(3)
    );

    // A wrong pin fails the reproduction check.
    record["link_build"]["expected_stdout_sha256"] = serde_json::json!("0".repeat(64));
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(
        !stdout_recheck(&source, &bundle, &native_bundle::sha256(&bytes))
            .status
            .success()
    );
}

#[test]
fn stdout_acceptance_emits_the_observation_descriptor() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let accept = cli(&[
        "native-bundle",
        "accept-stdout",
        bundle.to_str().unwrap(),
        source.to_str().unwrap(),
        "--expected-build-id",
        &id,
        "--json",
    ]);
    assert_success(&accept);
    let descriptor: serde_json::Value = serde_json::from_slice(&accept.stdout).unwrap();
    assert_eq!(descriptor["accepted"], true);
    assert_observation_context(&descriptor, "recorded");
    assert!(
        descriptor["checks_performed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "os_stdout_observation"),
        "stdout acceptance must claim the OS observation"
    );
    assert!(
        descriptor["checks_not_performed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "os_exit_status_observation"),
        "a stdout run does not compare the exit status"
    );
    assert!(
        !descriptor["checks_not_performed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "os_exit_status"),
        "the descriptor does not use the legacy check name"
    );
    assert!(
        !descriptor["checks_performed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "abi_runtime_semantics"),
        "no ABI/runtime claim"
    );

    // A pinless record cannot be accepted this way — inconclusive.
    let mut record: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    let fresh_record = record.clone();
    record["link_build"]
        .as_object_mut()
        .unwrap()
        .remove("expected_stdout_sha256");
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    let pinless = cli(&[
        "native-bundle",
        "accept-stdout",
        bundle.to_str().unwrap(),
        source.to_str().unwrap(),
        "--expected-build-id",
        &native_bundle::sha256(&bytes),
    ]);
    assert_eq!(pinless.status.code(), Some(3));

    let mut legacy_record = fresh_record;
    legacy_record["link_build"]
        .as_object_mut()
        .unwrap()
        .remove("observation_envelope");
    let bytes = serde_json::to_vec(&legacy_record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    let legacy = cli(&[
        "native-bundle",
        "accept-stdout",
        bundle.to_str().unwrap(),
        source.to_str().unwrap(),
        "--expected-build-id",
        &native_bundle::sha256(&bytes),
        "--json",
    ]);
    assert_success(&legacy);
    let legacy_descriptor: serde_json::Value = serde_json::from_slice(&legacy.stdout).unwrap();
    assert_observation_context(&legacy_descriptor, "unrecorded");
}

#[test]
fn stderr_is_pinned_and_reproduced() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let mut record: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    // New records pin the executable's observed stderr digest — the
    // profile's `main` writes nothing to either stream, so the pin is
    // the empty-stream digest.
    assert_eq!(
        record["link_build"]["expected_stderr_sha256"],
        native_bundle::sha256(b"")
    );
    let stderr_recheck = |source: &Path, bundle: &Path, id: &str| {
        Command::new(env!("CARGO_BIN_EXE_mumei"))
            .args([
                "native-bundle",
                "recheck-stderr",
                bundle.to_str().unwrap(),
                source.to_str().unwrap(),
                "--expected-build-id",
                id,
            ])
            .output()
            .unwrap()
    };
    assert_success(&stderr_recheck(&source, &bundle, &id));

    // A record pinned before the field existed carries no pin — the
    // recheck cannot evaluate the comparison and reports inconclusive.
    record["link_build"]
        .as_object_mut()
        .unwrap()
        .remove("expected_stderr_sha256");
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert_eq!(
        stderr_recheck(&source, &bundle, &native_bundle::sha256(&bytes))
            .status
            .code(),
        Some(3)
    );

    // A wrong pin fails the reproduction check.
    record["link_build"]["expected_stderr_sha256"] = serde_json::json!("0".repeat(64));
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(
        !stderr_recheck(&source, &bundle, &native_bundle::sha256(&bytes))
            .status
            .success()
    );
}

#[test]
fn runtime_observations_are_rechecked_on_one_invocation() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let record: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    assert_eq!(
        record["link_build"]["observation_envelope"],
        serde_json::json!({
            "environment": "empty",
            "working_directory": "executable_directory",
        })
    );
    let recheck_observations = |source: &Path, bundle: &Path, id: &str, json: bool| {
        let mut args = vec![
            "native-bundle",
            "recheck-observations",
            bundle.to_str().unwrap(),
            source.to_str().unwrap(),
            "--expected-build-id",
            id,
        ];
        if json {
            args.push("--json");
        }
        cli(&args)
    };

    let text = recheck_observations(&source, &bundle, &id, false);
    assert_success(&text);
    assert_eq!(
        String::from_utf8_lossy(&text.stdout),
        "Pinned toolchain outputs reproduce the pinned object and executable; one invocation of the reproduced executable matches every pinned runtime observation (exit status, stdout and stderr).\nScope: artifact_reproduction_and_runtime_observation_semantics_on_one_invocation_only/no_runtime_abi_or_semantic_proof\n"
    );

    let json = recheck_observations(&source, &bundle, &id, true);
    assert_success(&json);
    assert!(json.stdout.ends_with(b"\n"));
    let descriptor: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(descriptor.as_object().unwrap().len(), 10);
    assert_eq!(descriptor["schema_version"], "mumei.runtime-observation/v1");
    assert_eq!(descriptor["verdict"], "observed_matching");
    assert_eq!(
        descriptor["scope"],
        "artifact_reproduction_and_runtime_observation_semantics_on_one_invocation_only/no_runtime_abi_or_semantic_proof"
    );
    assert_eq!(descriptor["build_id"], id);
    assert_eq!(
        descriptor["invocation"],
        serde_json::json!({
            "argv": [],
            "stdin": "closed",
            "timeout_ms": 30000,
            "channel_cap_bytes": 65536,
            "environment": "empty",
            "working_directory": "executable_directory",
        })
    );
    assert_eq!(
        descriptor["host"],
        serde_json::json!({
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "family": std::env::consts::FAMILY,
        })
    );
    assert_eq!(descriptor["pin_envelope"], "recorded");
    assert_eq!(
        descriptor["observations"],
        serde_json::json!({
            "exit_status": record["link_build"]["expected_exit_status"],
            "stdout_sha256": record["link_build"]["expected_stdout_sha256"],
            "stderr_sha256": record["link_build"]["expected_stderr_sha256"],
        })
    );
    assert_eq!(
        descriptor["checks_performed"],
        serde_json::json!([
            "os_exit_status_observation",
            "os_stdout_observation",
            "os_stderr_observation",
            "runtime_observation_semantics",
        ])
    );
    assert_eq!(
        descriptor["checks_not_performed"],
        serde_json::json!([
            "producer_authenticity",
            "historical_execution",
            "optimizer_semantics",
            "abi_runtime_semantics",
            "grade_promotion",
            "lean_kernel_checks",
        ])
    );

    for pin in [
        "expected_exit_status",
        "expected_stdout_sha256",
        "expected_stderr_sha256",
    ] {
        let mut pinless_record = record.clone();
        pinless_record["link_build"]
            .as_object_mut()
            .unwrap()
            .remove(pin);
        let bytes = serde_json::to_vec(&pinless_record).unwrap();
        fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
        let id = native_bundle::sha256(&bytes);
        for json in [false, true] {
            let output = recheck_observations(&source, &bundle, &id, json);
            assert_eq!(output.status.code(), Some(3), "{pin}, json={json}");
            assert!(output.stdout.is_empty(), "{pin}, json={json}");
        }
    }

    for pin in [
        "expected_exit_status",
        "expected_stdout_sha256",
        "expected_stderr_sha256",
    ] {
        let mut wrong_record = record.clone();
        if pin == "expected_exit_status" {
            let expected = record["link_build"][pin].as_u64().unwrap();
            wrong_record["link_build"][pin] =
                serde_json::json!(if expected == 255 { 1 } else { expected + 1 });
        } else {
            wrong_record["link_build"][pin] = serde_json::json!("0".repeat(64));
        }
        let bytes = serde_json::to_vec(&wrong_record).unwrap();
        fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
        let id = native_bundle::sha256(&bytes);
        let output = recheck_observations(&source, &bundle, &id, true);
        assert_eq!(output.status.code(), Some(1), "{pin}");
        assert!(output.stdout.is_empty(), "{pin}");
    }

    let mut legacy_record = record.clone();
    legacy_record["link_build"]
        .as_object_mut()
        .unwrap()
        .remove("observation_envelope");
    let bytes = serde_json::to_vec(&legacy_record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    let legacy_id = native_bundle::sha256(&bytes);
    let legacy = recheck_observations(&source, &bundle, &legacy_id, true);
    assert_success(&legacy);
    let legacy_descriptor: serde_json::Value = serde_json::from_slice(&legacy.stdout).unwrap();
    assert_eq!(legacy_descriptor["pin_envelope"], "unrecorded");

    let mut mismatched_record = record.clone();
    mismatched_record["link_build"]["observation_envelope"]["environment"] =
        serde_json::json!("inherited");
    let bytes = serde_json::to_vec(&mismatched_record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    let mismatched_id = native_bundle::sha256(&bytes);
    let mismatched = recheck_observations(&source, &bundle, &mismatched_id, true);
    assert_eq!(mismatched.status.code(), Some(3));
    assert!(mismatched.stdout.is_empty());
}

#[test]
fn runtime_observations_are_accepted_on_one_invocation() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let record: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    assert_eq!(
        record["link_build"]["observation_envelope"],
        serde_json::json!({
            "environment": "empty",
            "working_directory": "executable_directory",
        })
    );
    assert!(
        record["link_build"]["canonical_arguments_sha256"].is_string(),
        "the build fixture pins canonical link arguments"
    );
    let accept_observations = |id: &str, json: bool| {
        let mut args = vec![
            "native-bundle",
            "accept-observations",
            bundle.to_str().unwrap(),
            source.to_str().unwrap(),
            "--expected-build-id",
            id,
        ];
        if json {
            args.push("--json");
        }
        cli(&args)
    };

    let json = accept_observations(&id, true);
    assert_success(&json);
    let descriptor: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(
        descriptor["schema_version"],
        "mumei.native-evidence-acceptance/v1"
    );
    assert_eq!(descriptor["accepted"], true);
    assert_eq!(descriptor["pin_envelope"], "recorded");
    assert_eq!(
        descriptor["scope"],
        "native_bundle_source_llvm_toolchain_reproduction_and_runtime_observation_semantics"
    );
    assert_eq!(
        descriptor["invocation"],
        serde_json::json!({
            "argv": [],
            "stdin": "closed",
            "timeout_ms": 30000,
            "channel_cap_bytes": 65536,
            "environment": "empty",
            "working_directory": "executable_directory",
        })
    );
    assert_eq!(
        descriptor["host"],
        serde_json::json!({
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "family": std::env::consts::FAMILY,
        })
    );
    assert_eq!(
        descriptor["checks_performed"],
        serde_json::json!([
            "bundle_identity_binding",
            "fresh_same_profile_source_recheck",
            "llvm_snapshot_translation_recheck",
            "toolchain_object_executable_reproduction",
            "canonical_link_argument_vector_reproduction",
            "os_exit_status_observation",
            "os_stdout_observation",
            "os_stderr_observation",
            "runtime_observation_semantics",
        ])
    );
    assert_eq!(
        descriptor["checks_not_performed"],
        serde_json::json!([
            "producer_authenticity",
            "historical_execution",
            "raw_link_argument_vector_reproduction",
            "optimizer_semantics",
            "abi_runtime_semantics",
            "grade_promotion",
            "lean_kernel_checks",
        ])
    );

    let text = accept_observations(&id, false);
    assert_success(&text);
    assert!(!text.stdout.is_empty());

    for pin in [
        "expected_exit_status",
        "expected_stdout_sha256",
        "expected_stderr_sha256",
    ] {
        let mut pinless_record = record.clone();
        pinless_record["link_build"]
            .as_object_mut()
            .unwrap()
            .remove(pin);
        let bytes = serde_json::to_vec(&pinless_record).unwrap();
        fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
        let id = native_bundle::sha256(&bytes);
        for json in [false, true] {
            let output = accept_observations(&id, json);
            assert_eq!(output.status.code(), Some(3), "{pin}, json={json}");
            assert!(output.stdout.is_empty(), "{pin}, json={json}");
        }
    }

    for pin in [
        "expected_exit_status",
        "expected_stdout_sha256",
        "expected_stderr_sha256",
    ] {
        let mut wrong_record = record.clone();
        if pin == "expected_exit_status" {
            let expected = record["link_build"][pin].as_u64().unwrap();
            wrong_record["link_build"][pin] =
                serde_json::json!(if expected == 255 { 1 } else { expected + 1 });
        } else {
            wrong_record["link_build"][pin] = serde_json::json!("0".repeat(64));
        }
        let bytes = serde_json::to_vec(&wrong_record).unwrap();
        fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
        let id = native_bundle::sha256(&bytes);
        let output = accept_observations(&id, true);
        assert_eq!(output.status.code(), Some(1), "{pin}");
        assert!(output.stdout.is_empty(), "{pin}");
    }
}

#[test]
fn stderr_acceptance_emits_the_observation_descriptor() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let accept = cli(&[
        "native-bundle",
        "accept-stderr",
        bundle.to_str().unwrap(),
        source.to_str().unwrap(),
        "--expected-build-id",
        &id,
        "--json",
    ]);
    assert_success(&accept);
    let descriptor: serde_json::Value = serde_json::from_slice(&accept.stdout).unwrap();
    assert_eq!(descriptor["accepted"], true);
    assert_observation_context(&descriptor, "recorded");
    assert!(
        descriptor["checks_performed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "os_stderr_observation"),
        "stderr acceptance must claim the OS observation"
    );
    for unclaimed in ["os_exit_status_observation", "os_stdout_observation"] {
        assert!(
            descriptor["checks_not_performed"]
                .as_array()
                .unwrap()
                .iter()
                .any(|check| check == unclaimed),
            "a stderr run does not claim {unclaimed}"
        );
    }
    assert!(
        !descriptor["checks_not_performed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "os_exit_status"),
        "the descriptor does not use the legacy check name"
    );
    assert!(
        !descriptor["checks_performed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "abi_runtime_semantics"),
        "no ABI/runtime claim"
    );

    // A pinless record cannot be accepted this way — inconclusive.
    let mut record: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    record["link_build"]
        .as_object_mut()
        .unwrap()
        .remove("expected_stderr_sha256");
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    let pinless = cli(&[
        "native-bundle",
        "accept-stderr",
        bundle.to_str().unwrap(),
        source.to_str().unwrap(),
        "--expected-build-id",
        &native_bundle::sha256(&bytes),
    ]);
    assert_eq!(pinless.status.code(), Some(3));
}

#[test]
fn bounded_source_and_symlinks_cannot_bypass_the_consumer() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    for (text, diagnostic) in [
        ("x".repeat(65_537), "unsupported component type or size"),
        (
            GOOD.replace(
                "{ 7 }",
                &format!("{{ {}7{} }}", "(".repeat(49), ")".repeat(49)),
            ),
            "source outside bounded C03 recheck profile",
        ),
        (
            GOOD.replace(
                "result == 7",
                &std::iter::repeat_n("result == 7", 100)
                    .collect::<Vec<_>>()
                    .join(" || "),
            ),
            "source outside bounded C03 recheck profile",
        ),
    ] {
        fs::write(&source, text).unwrap();
        let result = recheck(&source, &bundle, &id);
        assert_eq!(result.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&result.stderr).contains(diagnostic));
        assert_eq!(recheck_llvm(&source, &bundle, &id).status.code(), Some(1));
    }
    fs::write(&source, GOOD).unwrap();
    let object = bundle.join("program.o");
    fs::remove_file(&object).unwrap();
    std::os::unix::fs::symlink(&source, &object).unwrap();
    assert!(!recheck(&source, &bundle, &id).status.success());
    assert!(!recheck_llvm(&source, &bundle, &id).status.success());
}

#[test]
fn vacuous_preconditions_and_unsupported_sources_cannot_recheck_successfully() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    for text in [
        GOOD.replace("requires: true", "requires: false")
            .replace("result == 7", "false"),
        GOOD.replace("main()", "main(x: i64)"),
        GOOD.replace("main()", "other()"),
    ] {
        fs::write(&source, text).unwrap();
        let result = recheck(&source, &bundle, &id);
        assert_eq!(result.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&result.stderr)
            .contains("zero-argument main with literal true precondition"));
        assert_eq!(recheck_llvm(&source, &bundle, &id).status.code(), Some(1));
    }
    let fake = "FAKE_C10_SOURCE_DIAGNOSTIC_ONLY";
    for text in [
        GOOD.replace("{ 7 }", "{ 7 / 1 }"),
        GOOD.replace("{ 7 }", "{ 7.0 }"),
        GOOD.replace("{ 7 }", &format!("{{ {fake} }}")),
    ] {
        fs::write(&source, text).unwrap();
        let result = recheck(&source, &bundle, &id);
        assert_eq!(result.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&result.stderr)
            .contains("source outside bounded C03 recheck profile"));
        assert!(!String::from_utf8_lossy(&result.stderr).contains(fake));
        assert!(!String::from_utf8_lossy(&result.stdout).contains(fake));
        let result = recheck_llvm(&source, &bundle, &id);
        assert_eq!(result.status.code(), Some(1));
        assert!(!String::from_utf8_lossy(&result.stderr).contains(fake));
        assert!(!String::from_utf8_lossy(&result.stdout).contains(fake));
    }
}

#[test]
fn ambient_orchestration_settings_and_fake_secrets_do_not_enter_the_recheck() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, _) = build(root.path());
    let fake = "FAKE_C10_SECRET_ONLY";
    let text = format!("{GOOD}\n// {fake}");
    fs::write(&source, &text).unwrap();
    let id = repin(&bundle, Some(&text));
    for (action, json) in [
        ("recheck-source", false),
        ("recheck-llvm", false),
        ("recheck-llvm", true),
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_mumei"))
            .args([
                "native-bundle",
                action,
                bundle.to_str().unwrap(),
                source.to_str().unwrap(),
                "--expected-build-id",
                &id,
            ])
            .args(json.then_some("--json"))
            .env("MUMEI_VERIFICATION_TIMEOUT_MS", "1")
            .env("MUMEI_TASK_ID", fake)
            .env("MUMEI_CANCEL_REASON", fake)
            .env("MUMEI_SOLVER_CONFIG_FINGERPRINT", fake)
            .env("MUMEI_HARNESS_CONTRACT", fake)
            .env("MUMEI_ARTIFACT_PATHS", fake)
            .env("OPENAI_API_KEY", fake)
            .output()
            .unwrap();
        assert_success(&result);
        assert!(!String::from_utf8_lossy(&result.stdout).contains(fake));
        assert!(!String::from_utf8_lossy(&result.stderr).contains(fake));
    }
}

#[test]
fn build_binds_actual_native_output_and_source_certificate_without_proving_artifact() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let result = check(&source, &bundle, &id);
    assert_success(&result);
    assert_success(&recheck_llvm(&source, &bundle, &id));
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("identity consistent"));
    assert!(stdout.contains(native_bundle::SCOPE));
    let record: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    // Single-atom profile: main cannot call, so the dependency map is the
    // one-entry `{"main": []}` — same canonical-JSON pin convention as the
    // module family.
    assert_eq!(
        record["source"]["source_dependencies_sha256"],
        native_bundle::sha256(&serde_json::to_vec(&serde_json::json!({"main": []})).unwrap())
    );
    assert!(record["object_build"]["target_triple"]
        .as_str()
        .unwrap()
        .starts_with("x86_64-"));
    let ir = fs::read_to_string(bundle.join(BundleFile::LlvmIr.file_name())).unwrap();
    assert!(ir.contains("target triple ="));
    assert!(ir.contains("target datalayout ="));
    assert_eq!(
        Command::new(bundle.join("program"))
            .status()
            .unwrap()
            .code(),
        Some(7)
    );
    assert_success(&cli(&[
        "verify-cert",
        bundle.join("proof-cert.json").to_str().unwrap(),
        source.to_str().unwrap(),
    ]));
    assert_eq!(fs::read_dir(&bundle).unwrap().count(), 5);
    fs::write(
        &source,
        GOOD.replace("{ 7 }", "{ if 1 < 2 { 7 } else { 8 } }"),
    )
    .unwrap();
    let conditional = root.path().join("conditional");
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        conditional.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(conditional.join(RECORD_FILE)).unwrap());
    assert_success(&check(&source, &conditional, &id));
    assert_success(&recheck_llvm(&source, &conditional, &id));
}

#[test]
fn consumer_rejects_missing_replaced_stale_source_settings_and_unpinned_records() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    for role in BundleFile::ALL {
        let path = bundle.join(role.file_name());
        let original = fs::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert!(!check(&source, &bundle, &id).status.success());
        fs::write(&path, b"replaced").unwrap();
        assert!(!check(&source, &bundle, &id).status.success());
        fs::write(path, original).unwrap();
    }
    for source_text in [
        GOOD.replace("{ 7 }", "{ 8 }"),
        GOOD.replace("== 7", "== 8"),
        format!("{GOOD}\n// changed source"),
    ] {
        fs::write(&source, source_text).unwrap();
        assert!(!check(&source, &bundle, &id).status.success());
    }
    fs::write(&source, GOOD).unwrap();
    assert!(!check(&source, &bundle, &"0".repeat(64)).status.success());
    let record_path = bundle.join(RECORD_FILE);
    let original = fs::read(&record_path).unwrap();
    for pointer in [
        "/settings/relocation",
        "/object_build/target_triple",
        "/source/compiler_version",
        "/link_build/driver_version",
        "/link_build/canonical_arguments_sha256",
        "/scope",
    ] {
        let mut record: serde_json::Value = serde_json::from_slice(&original).unwrap();
        *record.pointer_mut(pointer).unwrap() = serde_json::json!("changed");
        fs::write(&record_path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(!check(&source, &bundle, &id).status.success());
        let new_id = native_bundle::sha256(&fs::read(&record_path).unwrap());
        assert!(!check(&source, &bundle, &new_id).status.success());
    }
    fs::write(record_path, original).unwrap();
    assert_success(&check(&source, &bundle, &id));
}

#[test]
fn producer_refuses_unsupported_or_false_source_and_never_clobbers_outputs() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.mm");
    let bundle = root.path().join("bundle");
    for text in [
        GOOD.replace("== 7", "== 8"),
        GOOD.replace("true;", "false;"),
        GOOD.replace("{ 7 }", "{ 7 + 1 }"),
        GOOD.replace("main()", "main(x: i64)"),
        format!("trusted {GOOD}"),
        GOOD.replace("-> i64", "-> f64"),
    ] {
        fs::write(&source, text).unwrap();
        assert!(!cli(&[
            "native-bundle",
            "build",
            source.to_str().unwrap(),
            "-o",
            bundle.to_str().unwrap()
        ])
        .status
        .success());
        assert!(!bundle.exists());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
    let (source, bundle, id) = build(root.path());
    assert!(!cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap()
    ])
    .status
    .success());
    assert_success(&check(&source, &bundle, &id));
}

#[test]
fn untrusted_metadata_and_symlinks_do_not_select_files_or_echo_secrets() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let path = bundle.join(RECORD_FILE);
    let original = fs::read(&path).unwrap();
    let fake = "FAKE_SECRET_NATIVE_BUNDLE_ONLY";
    let mut record: serde_json::Value = serde_json::from_slice(&original).unwrap();
    record[fake] = serde_json::json!("../../private");
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(&path, &bytes).unwrap();
    let output = check(&source, &bundle, &native_bundle::sha256(&bytes));
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).contains(fake));
    fs::write(&path, original).unwrap();
    let object = bundle.join("program.o");
    fs::remove_file(&object).unwrap();
    std::os::unix::fs::symlink(&source, &object).unwrap();
    assert!(!check(&source, &bundle, &id).status.success());
    fs::remove_file(&object).unwrap();
    std::os::unix::fs::symlink(&source, &object).unwrap();
    assert!(!check(&source, &bundle, fake).status.success());
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    let other = root.path().join("other");
    fs::write(&source, format!("{GOOD}\n// {fake}")).unwrap();
    let output = command
        .args([
            "native-bundle",
            "build",
            source.to_str().unwrap(),
            "-o",
            other.to_str().unwrap(),
        ])
        .env("MUMEI_HARNESS_CONTRACT", fake)
        .env("MUMEI_ARTIFACT_PATHS", fake)
        .env("OPENAI_API_KEY", fake)
        .output()
        .unwrap();
    assert_success(&output);
    assert!(!String::from_utf8_lossy(&output.stdout).contains(fake));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(fake));
    for role in BundleFile::ALL {
        assert!(!fs::read(other.join(role.file_name()))
            .unwrap()
            .windows(fake.len())
            .any(|bytes| bytes == fake.as_bytes()));
    }
    assert!(!fs::read_to_string(other.join("proof-cert.json"))
        .unwrap()
        .contains(fake));
}

#[test]
fn a_rebound_record_is_not_evidence_of_semantic_correspondence() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let other = root.path().join("wrong-bundle");
    let wrong_source = root.path().join("wrong.mm");
    fs::write(&wrong_source, GOOD.replace('7', "8")).unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        wrong_source.to_str().unwrap(),
        "-o",
        other.to_str().unwrap(),
    ]));
    let wrong_artifact = fs::read(other.join("program")).unwrap();
    fs::write(bundle.join("program"), &wrong_artifact).unwrap();
    assert!(!check(&source, &bundle, &id).status.success());
    let mut record: native_bundle::NativeBundleRecord =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    record.files.insert(
        BundleFile::Executable,
        native_bundle::ContentIdentity::of(&wrong_artifact),
    );
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &id).status.success());
    // Deliberately authorizing a new digest only establishes new byte identities.
    let repinned = check(&source, &bundle, &native_bundle::sha256(&bytes));
    assert_success(&repinned);
    assert!(String::from_utf8_lossy(&repinned.stdout).contains(native_bundle::SCOPE));
    assert_eq!(
        Command::new(bundle.join("program"))
            .status()
            .unwrap()
            .code(),
        Some(8)
    );
}

#[test]
fn unavailable_linker_leaves_no_partial_bundle() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.mm");
    let bundle = root.path().join("bundle");
    fs::write(&source, GOOD).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args([
            "native-bundle",
            "build",
            source.to_str().unwrap(),
            "-o",
            bundle.to_str().unwrap(),
        ])
        .env("PATH", "/nonexistent-native-bundle-test")
        .env("LLVM_SYS_170_PREFIX", "/nonexistent-native-bundle-test")
        .env("HOME", root.path())
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!bundle.exists());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn repinning_a_certificate_cannot_bypass_source_status_or_translator_gates() {
    use mumei_core::proof_cert;
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, _) = build(root.path());
    let record: native_bundle::NativeBundleRecord =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    let cert: proof_cert::ProofCertificate =
        serde_json::from_slice(&fs::read(bundle.join("proof-cert.json")).unwrap()).unwrap();
    for (status, result) in [
        ("trusted", "skipped"),
        ("verified", "lean_verified"),
        ("verified", "unknown"),
        ("verified", "sat"),
        ("verified", "unsat"),
    ] {
        let mut changed = cert.clone();
        changed.atoms[0].status = status.into();
        changed.atoms[0].z3_check_result = result.into();
        if result == "unsat" {
            changed.atoms[0].translator_version = "stale".into();
        }
        proof_cert::refresh_certificate_integrity(&mut changed);
        let bytes = serde_json::to_vec(&changed).unwrap();
        fs::write(bundle.join("proof-cert.json"), &bytes).unwrap();
        let mut changed_record = record.clone();
        changed_record.files.insert(
            BundleFile::Certificate,
            native_bundle::ContentIdentity::of(&bytes),
        );
        let record_bytes = serde_json::to_vec(&changed_record).unwrap();
        fs::write(bundle.join(RECORD_FILE), &record_bytes).unwrap();
        assert!(
            !check(&source, &bundle, &native_bundle::sha256(&record_bytes))
                .status
                .success()
        );
    }
}

#[test]
fn rehashed_certificate_targets_must_match_the_source_translator_identity() {
    use mumei_core::proof_cert;
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, _) = build(root.path());
    let record: native_bundle::NativeBundleRecord =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    let cert: proof_cert::ProofCertificate =
        serde_json::from_slice(&fs::read(bundle.join("proof-cert.json")).unwrap()).unwrap();
    for field in 0..7 {
        let mut changed = cert.clone();
        match field {
            0 => changed.atoms[0].translator_ir.theorem_goal = "False".into(),
            1 => changed.atoms[0].translator_ir.sort = "different".into(),
            2 => changed.atoms[0].body_expr = "{ 8 }".into(),
            3 => changed.atoms[0].ensures = "result == 8".into(),
            4 => changed.atoms[0].requires = "false".into(),
            5 => changed.atoms[0].assumed_clauses.push("ensures".into()),
            _ => changed.atoms[0].effects.push("IO".into()),
        }
        proof_cert::refresh_certificate_integrity(&mut changed);
        let bytes = serde_json::to_vec(&changed).unwrap();
        fs::write(bundle.join("proof-cert.json"), &bytes).unwrap();
        let mut changed_record = record.clone();
        changed_record.files.insert(
            BundleFile::Certificate,
            native_bundle::ContentIdentity::of(&bytes),
        );
        let record_bytes = serde_json::to_vec(&changed_record).unwrap();
        fs::write(bundle.join(RECORD_FILE), &record_bytes).unwrap();
        assert!(
            !check(&source, &bundle, &native_bundle::sha256(&record_bytes))
                .status
                .success(),
            "field {field}"
        );
    }
}

#[test]
fn repinned_wrong_llvm_is_not_a_translation_even_with_valid_source_and_certificate() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, _) = build(root.path());
    let path = bundle.join("program.ll");
    let original = fs::read_to_string(&path).unwrap();
    for value in ["8", "4294967303", "1234567890123456"] {
        let changed = original.replace("ret i64 7", &format!("ret i64 {value}"));
        assert_ne!(changed, original);
        fs::write(&path, changed).unwrap();
        let id = repin(&bundle, None);
        assert_success(&check(&source, &bundle, &id));
        assert_success(&recheck(&source, &bundle, &id));
        let result = recheck_llvm(&source, &bundle, &id);
        assert_eq!(result.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&result.stderr).contains("LLVM body differs from source"));
        assert!(!String::from_utf8_lossy(&result.stderr).contains(value));
    }
    fs::write(&path, original).unwrap();
    let changed = GOOD.replace("7", "8");
    fs::write(&source, &changed).unwrap();
    let id = repin(&bundle, Some(&changed));
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_eq!(recheck_llvm(&source, &bundle, &id).status.code(), Some(1));
}

#[test]
fn repinned_wrapper_mutations_and_private_malformed_ir_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, _) = build(root.path());
    let path = bundle.join("program.ll");
    let original = fs::read_to_string(&path).unwrap();
    let fake = "FAKE_PRIVATE_NATIVE_LLVM_ONLY";
    let mutations = [
        original.replace("ret i32 %exit_code", "ret i32 7"),
        original.replace("trunc i64 %result", "trunc i64 7"),
        original.replace("define i32 @main", "define i32 @other"),
        original.replace(
            "call i64 @__mumei_user_main()",
            "call i64 @__mumei_user_main() [ \"custom\"() ]",
        ),
        original.replace(
            "call i64 @__mumei_user_main()",
            "call i64 @__mumei_user_main() [ \"deopt\"() ]",
        ),
        format!("{original}\ndeclare void @other()"),
        format!("{original}\n@global = global i64 7"),
        original.replace("e-m:e-", "E-m:e-"),
        format!("{fake}\n{original}"),
        format!("{original};{}", "X".repeat(65_536)),
    ];
    for changed in mutations {
        assert_ne!(changed, original);
        fs::write(&path, changed).unwrap();
        let id = repin(&bundle, None);
        assert_success(&recheck(&source, &bundle, &id));
        let result = recheck_llvm(&source, &bundle, &id);
        assert_eq!(result.status.code(), Some(1));
        assert!(!String::from_utf8_lossy(&result.stdout).contains(fake));
        assert!(!String::from_utf8_lossy(&result.stderr).contains(fake));
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("outside bounded translation profile")
        );
    }
    fs::write(&path, [0xff, 0xfe]).unwrap();
    let id = repin(&bundle, None);
    assert_eq!(recheck_llvm(&source, &bundle, &id).status.code(), Some(1));
}

#[test]
fn native_ir_observations_must_match_the_snapshot_without_normalization() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, _) = build(root.path());
    let path = bundle.join(RECORD_FILE);
    let original: native_bundle::NativeBundleRecord =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for change_layout in [false, true] {
        let mut record = original.clone();
        if change_layout {
            record.object_build.data_layout = "e-i64:64".into();
        } else {
            record.object_build.target_triple =
                if record.object_build.target_triple == "x86_64-pc-linux-gnu" {
                    "x86_64-unknown-linux-gnu".into()
                } else {
                    "x86_64-pc-linux-gnu".into()
                };
        }
        let bytes = serde_json::to_vec(&record).unwrap();
        fs::write(&path, &bytes).unwrap();
        let id = native_bundle::sha256(&bytes);
        assert_success(&check(&source, &bundle, &id));
        assert_success(&recheck(&source, &bundle, &id));
        let result = recheck_llvm(&source, &bundle, &id);
        assert_eq!(result.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&result.stderr)
            .contains("snapshot/toolchain observation mismatch"));
    }
}

#[test]
fn full_native_build_rechecks_large_i64_results_without_promising_an_os_exit_code() {
    let root = tempfile::tempdir().unwrap();
    for value in ["0", "2147483648", "4294967296", "9223372036854775807"] {
        let source = root.path().join("source.mm");
        let bundle = root.path().join(format!("bundle-{value}"));
        fs::write(&source, format!("atom main() -> i64 requires: true; ensures: result == {value}; body: {{ {value} }};")).unwrap();
        assert_success(&cli(&[
            "native-bundle",
            "build",
            source.to_str().unwrap(),
            "-o",
            bundle.to_str().unwrap(),
        ]));
        let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
        let result = recheck_llvm(&source, &bundle, &id);
        assert_success(&result);
        let stdout = String::from_utf8_lossy(&result.stdout);
        assert!(stdout.contains("low 32 bits of the i64 body"));
        assert!(stdout.contains("OS exit-status/executable semantics: not checked"));
        assert!(!stdout.contains("artifact_verified"));
        let json = recheck_llvm_json(&source, &bundle, &id);
        assert_success(&json);
        let report: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
        assert_eq!(report["acceptance"], "not_evaluated");
    }
}

fn json_evidence(source: &Path, bundle: &Path, id: &str) -> serde_json::Value {
    let result = recheck_llvm_json(source, bundle, id);
    assert_success(&result);
    assert!(result.stderr.is_empty());
    serde_json::from_slice(&result.stdout).unwrap()
}

fn normalized_evidence(mut report: serde_json::Value) -> serde_json::Value {
    report["build_id"] = serde_json::json!("0".repeat(64));
    for evidence in report["evidence"].as_array_mut().unwrap() {
        for value in [
            "/subject/sha256",
            "/specification/sha256",
            "/provenance/configuration_fingerprint",
        ] {
            let digest = evidence.pointer_mut(value).unwrap();
            assert!(native_bundle::is_digest(digest.as_str().unwrap()));
            *digest = serde_json::json!("0".repeat(64));
        }
        if !evidence["semantics"]["target_triple"].is_null() {
            evidence["semantics"]["target_triple"] = serde_json::json!("runtime-target");
        }
        assert_eq!(
            evidence["provenance"]["adapter"]["version"],
            env!("CARGO_PKG_VERSION")
        );
        assert!(!evidence["provenance"]["tool"]["version"]
            .as_str()
            .unwrap()
            .is_empty());
        evidence["provenance"]["adapter"]["version"] = serde_json::json!("build-version");
        evidence["provenance"]["tool"]["version"] = serde_json::json!("runtime-version");
    }
    report
}

#[test]
fn json_evidence_separates_actual_identities_obligations_and_schema_checked_shape() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let before: Vec<_> = fs::read_dir(&bundle)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            (path.clone(), fs::read(path).unwrap())
        })
        .collect();
    let report = json_evidence(&source, &bundle, &id);
    assert_eq!(report, json_evidence(&source, &bundle, &id));
    assert_eq!(report["build_id"], id);
    let records = report["evidence"].as_array().unwrap();
    let manifest: native_bundle::NativeBundleRecord =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    assert_eq!(
        records[0]["subject"]["sha256"],
        manifest.files[&BundleFile::Executable].sha256
    );
    assert_eq!(records[0]["specification"]["sha256"], id);
    assert_eq!(
        records[1]["subject"]["sha256"],
        native_bundle::sha256(&fs::read(&source).unwrap())
    );
    assert_eq!(
        records[1]["specification"]["sha256"],
        manifest.source.specification_sha256
    );
    assert_eq!(
        records[2]["subject"]["sha256"],
        native_bundle::sha256(&fs::read(bundle.join("program.ll")).unwrap())
    );
    assert_eq!(
        records[2]["specification"]["sha256"],
        records[1]["subject"]["sha256"]
    );
    for record in &records[1..] {
        let version = mumei_core::verification::linked_z3_version();
        assert_eq!(record["provenance"]["tool"]["name"], "Z3");
        assert_eq!(
            record["provenance"]["tool"]["version"],
            format!("{}.{}.{}.{}", version.0, version.1, version.2, version.3)
        );
    }
    assert_eq!(
        records[0]["provenance"]["configuration_fingerprint"],
        records[2]["provenance"]["configuration_fingerprint"]
    );
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "fixtures/verification-evidence/native_consumer_shape.json"
    ))
    .unwrap();
    assert_eq!(normalized_evidence(report), fixture);
    for (path, bytes) in before {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    assert_eq!(fs::read_dir(&bundle).unwrap().count(), 5);
}

#[test]
fn json_llvm_obligation_binds_the_body_not_just_shared_postconditions() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.mm");
    let mut reports = Vec::new();
    for value in [7, 8] {
        fs::write(
            &source,
            format!("atom main() -> i64 requires: true; ensures: true; body: {{ {value} }};"),
        )
        .unwrap();
        let bundle = root.path().join(format!("bundle-{value}"));
        assert_success(&cli(&[
            "native-bundle",
            "build",
            source.to_str().unwrap(),
            "-o",
            bundle.to_str().unwrap(),
        ]));
        let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
        reports.push(json_evidence(&source, &bundle, &id));
    }
    assert_eq!(
        reports[0]["evidence"][1]["specification"],
        reports[1]["evidence"][1]["specification"]
    );
    assert_ne!(
        reports[0]["evidence"][2]["specification"],
        reports[1]["evidence"][2]["specification"]
    );
}

#[test]
fn json_failures_emit_no_partial_report_or_private_diagnostics() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let rejected = |pin: &str| {
        let result = recheck_llvm_json(&source, &bundle, pin);
        assert_eq!(result.status.code(), Some(1));
        assert!(result.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&result.stderr).contains(root.path().to_str().unwrap()));
    };
    for pin in ["", "untrusted", &"0".repeat(64), &format!("{id}\n")] {
        rejected(pin);
    }
    for role in BundleFile::ALL {
        let path = bundle.join(role.file_name());
        let bytes = fs::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        rejected(&id);
        fs::write(&path, b"FAKE_PRIVATE_REPORT_COMPONENT").unwrap();
        rejected(&id);
        fs::write(&path, bytes).unwrap();
    }
    for text in [
        GOOD.replace("7", "8"),
        format!("{GOOD}//FAKE_PRIVATE_STALE_SOURCE"),
        "FAKE_PRIVATE_MALFORMED_SOURCE".to_owned(),
    ] {
        fs::write(&source, text).unwrap();
        rejected(&id);
    }
    fs::write(&source, GOOD).unwrap();
    let ir_path = bundle.join("program.ll");
    let original = fs::read_to_string(&ir_path).unwrap();
    for changed in [
        original.replace("ret i64 7", "ret i64 4294967303"),
        original.replace("ret i32 %exit_code", "ret i32 7"),
        original.replace(
            "call i64 @__mumei_user_main()",
            "call i64 @__mumei_user_main() [ \"custom\"() ]",
        ),
        format!("{original}\ndeclare void @extra()"),
        format!("FAKE_PRIVATE_MALFORMED_IR{original}"),
    ] {
        fs::write(&ir_path, changed).unwrap();
        rejected(&repin(&bundle, None));
    }
}

#[test]
fn json_repinned_non_executable_artifacts_remain_identity_only_and_are_not_run() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, _) = build(root.path());
    let marker = root.path().join("MUST_NOT_EXIST");
    fs::write(bundle.join("program.o"), b"not an object").unwrap();
    fs::write(
        bundle.join("program"),
        format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
    )
    .unwrap();
    fs::set_permissions(bundle.join("program"), fs::Permissions::from_mode(0o700)).unwrap();
    let id = repin(&bundle, None);
    let report = json_evidence(&source, &bundle, &id);
    assert!(!marker.exists());
    assert_eq!(report["acceptance"], "not_evaluated");
    assert_eq!(report["evidence"][0]["method"], "identity_check");
    assert_eq!(
        report["evidence"][0]["correspondence"]["status"],
        "unverified"
    );
    assert_eq!(
        report["evidence"][0]["subject"]["sha256"],
        native_bundle::sha256(&fs::read(bundle.join("program")).unwrap())
    );
    assert!(report["evidence"][2]["exclusions"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!(
            "os_exit_status_and_final_executable_semantics"
        )));
}

#[test]
fn json_uses_local_provenance_and_non_secret_configuration_only() {
    let root = tempfile::Builder::new()
        .prefix("FAKE_PRIVATE_CALLER_PATH")
        .tempdir()
        .unwrap();
    let (source, bundle, _) = build(root.path());
    let fake = "FAKE_PRIVATE_EVIDENCE_ONLY";
    let text = format!("{GOOD}//{fake}");
    fs::write(&source, &text).unwrap();
    repin(&bundle, Some(&text));
    let path = bundle.join(RECORD_FILE);
    let mut record: native_bundle::NativeBundleRecord =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    record.object_build.llvm_version = "17.98765.43210".into();
    record.link_build.driver_version = "98765.43210".into();
    record.link_build.linker_version = "98765.43210".into();
    record.link_build.driver_target = format!("x86_64-{fake}-linux-gnu");
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(&path, &bytes).unwrap();
    let id = native_bundle::sha256(&bytes);
    let baseline = recheck_llvm_json(&source, &bundle, &id);
    assert_success(&baseline);
    let result = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args([
            "native-bundle",
            "recheck-llvm",
            bundle.to_str().unwrap(),
            source.to_str().unwrap(),
            "--expected-build-id",
            &id,
            "--json",
        ])
        .env("MUMEI_VERIFICATION_TIMEOUT_MS", "1")
        .env("MUMEI_SOLVER_CONFIG_FINGERPRINT", fake)
        .env("MUMEI_ARTIFACT_PATHS", fake)
        .env("MUMEI_TASK_ID", fake)
        .env("OPENAI_API_KEY", fake)
        .output()
        .unwrap();
    assert_success(&result);
    assert_eq!(result.stdout, baseline.stdout);
    assert!(result.stderr.is_empty());
    let stdout = String::from_utf8(result.stdout).unwrap();
    for private in [
        fake,
        "98765.43210",
        "FAKE_PRIVATE_CALLER_PATH",
        GOOD,
        root.path().to_str().unwrap(),
    ] {
        assert!(!stdout.contains(private));
    }
    let evidence: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        evidence["evidence"][2]["semantics"]["target_triple"],
        record.object_build.target_triple
    );
    let saved = save_evidence(root.path(), &source, &bundle, &id);
    let replay = replay_command(&source, &bundle, &id, &saved)
        .env("MUMEI_VERIFICATION_TIMEOUT_MS", "1")
        .env("MUMEI_SOLVER_CONFIG_FINGERPRINT", fake)
        .env("OPENAI_API_KEY", fake)
        .output()
        .unwrap();
    assert_success(&replay);
    assert!(!String::from_utf8_lossy(&replay.stdout).contains(fake));
    assert!(!String::from_utf8_lossy(&replay.stderr).contains(fake));
}

fn save_evidence(root: &Path, source: &Path, bundle: &Path, id: &str) -> std::path::PathBuf {
    let path = root.join("evidence.json");
    let output = recheck_llvm_json(source, bundle, id);
    assert_success(&output);
    fs::write(&path, output.stdout).unwrap();
    path
}

fn replay_command(source: &Path, bundle: &Path, id: &str, saved: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command.args([
        "native-bundle",
        "recheck-evidence",
        bundle.to_str().unwrap(),
        source.to_str().unwrap(),
        saved.to_str().unwrap(),
        "--expected-build-id",
        id,
    ]);
    command
}

fn accept_command(source: &Path, bundle: &Path, id: &str, saved: &Path, json: bool) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command.args([
        "native-bundle",
        "accept-evidence",
        bundle.to_str().unwrap(),
        source.to_str().unwrap(),
        saved.to_str().unwrap(),
        "--expected-build-id",
        id,
    ]);
    if json {
        command.arg("--json");
    }
    command
}

fn accept_rejected(source: &Path, bundle: &Path, id: &str, saved: &Path) -> Output {
    let output = accept_command(source, bundle, id, saved, false)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    output
}

#[test]
fn accept_evidence_emits_anonymous_acceptance_descriptor_only_on_acceptance() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let saved = save_evidence(root.path(), &source, &bundle, &id);
    for json in [false, true] {
        let output = accept_command(&source, &bundle, &id, &saved, json)
            .output()
            .unwrap();
        assert_success(&output);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!stdout.contains("FAKE_PRIVATE"));
        assert!(!stdout.contains("main"));
        if json {
            let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                descriptor["schema_version"],
                "mumei.native-evidence-acceptance/v1"
            );
            assert_eq!(descriptor["acceptance"], "evaluated");
            assert_eq!(descriptor["accepted"], true);
            // The binding fingerprint is the sha256 of the accepted
            // record's bytes — a holder can prove which record was
            // evaluated without the descriptor echoing any claim.
            assert_eq!(
                descriptor["evidence_fingerprint"],
                native_bundle::sha256(&fs::read(&saved).unwrap())
            );
            for check in [
                "bundle_identity_binding",
                "fresh_same_profile_source_recheck",
                "llvm_snapshot_translation_recheck",
                "saved_evidence_consistency",
            ] {
                assert!(descriptor["checks_performed"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|entry| entry == check));
            }
            assert!(mumei_core::verification_evidence::decode(&output.stdout).is_err());
        } else {
            assert!(stdout.contains("accepted"));
        }
    }
}

#[test]
fn accept_evidence_rejects_tampered_foreign_and_mispinned_reports() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let saved = save_evidence(root.path(), &source, &bundle, &id);
    let original = fs::read(&saved).unwrap();
    // Tampered record.
    let mut changed: serde_json::Value = serde_json::from_slice(&original).unwrap();
    changed["evidence"][1]["method"] = serde_json::json!("finite_observation");
    fs::write(&saved, serde_json::to_vec(&changed).unwrap()).unwrap();
    accept_rejected(&source, &bundle, &id, &saved);
    // Foreign-family / foreign-schema envelopes.
    for foreign in [
        serde_json::json!({"schema_version": "mumei.native-module-consumer-evidence/v1", "evidence": []}),
        serde_json::json!({"schema_version": "mumei.verification-evidence/v1"}),
    ] {
        fs::write(&saved, serde_json::to_vec(&foreign).unwrap()).unwrap();
        accept_rejected(&source, &bundle, &id, &saved);
    }
    fs::write(&saved, &original).unwrap();
    // Wrong expected build id and missing source agreement.
    accept_rejected(&source, &bundle, &"0".repeat(64), &saved);
    assert_success(
        &accept_command(&source, &bundle, &id, &saved, true)
            .output()
            .unwrap(),
    );
}

#[test]
fn accept_evidence_accepts_module_family_reports() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), MODULE);
    let saved = save_evidence(root.path(), &source, &bundle, &id);
    let output = accept_command(&source, &bundle, &id, &saved, true)
        .output()
        .unwrap();
    assert_success(&output);
    let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        descriptor["schema_version"],
        "mumei.native-evidence-acceptance/v1"
    );
    assert_eq!(descriptor["accepted"], true);
}

fn replay_rejected(source: &Path, bundle: &Path, id: &str, saved: &Path) -> Output {
    let output = replay_command(source, bundle, id, saved).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("FAKE_PRIVATE"));
    assert!(!stderr.contains(saved.to_str().unwrap()));
    output
}

#[test]
fn saved_evidence_roundtrips_without_changes_and_ignores_json_object_order() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let saved = save_evidence(root.path(), &source, &bundle, &id);
    let before = fs::read(&saved).unwrap();
    let output = replay_command(&source, &bundle, &id, &saved)
        .output()
        .unwrap();
    assert_success(&output);
    assert!(output.stderr.is_empty());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Evidence acceptance: not evaluated"));
    assert_eq!(fs::read(&saved).unwrap(), before);
    let reordered: serde_json::Value = serde_json::from_slice(&before).unwrap();
    fs::write(&saved, serde_json::to_vec(&reordered).unwrap()).unwrap();
    assert_success(
        &replay_command(&source, &bundle, &id, &saved)
            .output()
            .unwrap(),
    );
    assert_eq!(fs::read_dir(&bundle).unwrap().count(), 5);
}

#[test]
fn common_representation_is_not_native_profile_or_fresh_query_acceptance() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let saved = save_evidence(root.path(), &source, &bundle, &id);
    let original: serde_json::Value = serde_json::from_slice(&fs::read(&saved).unwrap()).unwrap();
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "fixtures/verification-evidence/synthetic_cases.json"
    ))
    .unwrap();
    for case in cases["cases"].as_array().unwrap() {
        let record = &case["evidence"];
        mumei_core::verification_evidence::decode(&serde_json::to_vec(record).unwrap()).unwrap();
        let mut report = original.clone();
        report["evidence"][1] = record.clone();
        fs::write(&saved, serde_json::to_vec(&report).unwrap()).unwrap();
        replay_rejected(&source, &bundle, &id, &saved);
    }
    for metadata in [
        serde_json::Value::Null,
        serde_json::json!("synthetic-translator/v1"),
    ] {
        let mut report = original.clone();
        for record in report["evidence"].as_array_mut().unwrap() {
            record["provenance"]["translator_version"] = metadata.clone();
            record["provenance"]["bridge_lemma_hash"] = serde_json::Value::Null;
        }
        fs::write(&saved, serde_json::to_vec(&report).unwrap()).unwrap();
        if metadata.is_null() {
            assert_success(
                &replay_command(&source, &bundle, &id, &saved)
                    .output()
                    .unwrap(),
            );
        } else {
            replay_rejected(&source, &bundle, &id, &saved);
        }
    }
    let mut missing = original.clone();
    missing["evidence"][1]["semantics"]
        .as_object_mut()
        .unwrap()
        .remove("target_triple");
    fs::write(&saved, serde_json::to_vec(&missing).unwrap()).unwrap();
    let rejected = replay_rejected(&source, &bundle, &id, &saved);
    assert!(String::from_utf8_lossy(&rejected.stderr)
        .contains("invalid native evidence representation"));
}

fn json_paths(
    value: &serde_json::Value,
    prefix: String,
    leaves: &mut Vec<String>,
    objects: &mut Vec<String>,
) {
    match value {
        serde_json::Value::Object(fields) => {
            objects.push(prefix.clone());
            for (key, value) in fields {
                json_paths(value, format!("{prefix}/{key}"), leaves, objects);
            }
        }
        serde_json::Value::Array(values) if !values.is_empty() => {
            for (index, value) in values.iter().enumerate() {
                json_paths(value, format!("{prefix}/{index}"), leaves, objects);
            }
        }
        _ => leaves.push(prefix),
    }
}

#[test]
fn replay_checks_every_field_and_rejects_unknown_fields_at_every_object_level() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let saved = save_evidence(root.path(), &source, &bundle, &id);
    let original: serde_json::Value = serde_json::from_slice(&fs::read(&saved).unwrap()).unwrap();
    let (mut leaves, mut objects) = (Vec::new(), Vec::new());
    json_paths(&original, String::new(), &mut leaves, &mut objects);
    assert!(leaves.len() > 100);
    for path in leaves {
        let mut changed = original.clone();
        *changed.pointer_mut(&path).unwrap() = serde_json::json!("FAKE_PRIVATE_FIELD_MUTATION");
        fs::write(&saved, serde_json::to_vec(&changed).unwrap()).unwrap();
        replay_rejected(&source, &bundle, &id, &saved);
    }
    for path in &objects {
        let mut changed = original.clone();
        changed
            .pointer_mut(path)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(
                "FAKE_PRIVATE_UNKNOWN_FIELD".into(),
                serde_json::json!("not_a_trust_override"),
            );
        fs::write(&saved, serde_json::to_vec(&changed).unwrap()).unwrap();
        replay_rejected(&source, &bundle, &id, &saved);
    }
    for path in objects {
        let fields = original.pointer(&path).unwrap().as_object().unwrap();
        for key in fields.keys() {
            let mut changed = original.clone();
            changed
                .pointer_mut(&path)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(key);
            fs::write(&saved, serde_json::to_vec(&changed).unwrap()).unwrap();
            replay_rejected(&source, &bundle, &id, &saved);
        }
    }
    let mut digests = vec!["/build_id".to_owned()];
    for index in 0..3 {
        for field in [
            "subject/sha256",
            "specification/sha256",
            "provenance/configuration_fingerprint",
        ] {
            digests.push(format!("/evidence/{index}/{field}"));
        }
    }
    for path in digests {
        let mut changed = original.clone();
        let value = changed.pointer_mut(&path).unwrap();
        assert_ne!(value.as_str().unwrap(), "0".repeat(64));
        *value = serde_json::json!("0".repeat(64));
        fs::write(&saved, serde_json::to_vec(&changed).unwrap()).unwrap();
        replay_rejected(&source, &bundle, &id, &saved);
    }
    for (path, value) in [
        ("/evidence/1/result", "inconclusive"),
        ("/evidence/1/execution", "timeout"),
        ("/evidence/2/method", "finite_execution"),
        (
            "/evidence/2/semantics/integer_model",
            "mathematical_integer",
        ),
        ("/evidence/2/correspondence/status", "assumed"),
        ("/evidence/1/provenance/tool/version", "0.0.0.0"),
        ("/evidence/0/provenance/adapter/version", "0.0.0"),
        ("/acceptance", "accepted"),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(path).unwrap() = serde_json::json!(value);
        fs::write(&saved, serde_json::to_vec(&changed).unwrap()).unwrap();
        replay_rejected(&source, &bundle, &id, &saved);
    }
    for count in [0, 1, 2, 4] {
        let mut changed = original.clone();
        changed["evidence"] = serde_json::json!(vec![original["evidence"][0].clone(); count]);
        fs::write(&saved, serde_json::to_vec(&changed).unwrap()).unwrap();
        replay_rejected(&source, &bundle, &id, &saved);
    }
    let mut reordered = original.clone();
    reordered["evidence"].as_array_mut().unwrap().swap(0, 2);
    fs::write(&saved, serde_json::to_vec(&reordered).unwrap()).unwrap();
    replay_rejected(&source, &bundle, &id, &saved);
    fs::write(&saved, serde_json::to_vec(&original).unwrap()).unwrap();
    assert_success(
        &replay_command(&source, &bundle, &id, &saved)
            .output()
            .unwrap(),
    );
}

#[test]
fn replay_rejects_duplicate_escaped_or_nested_fields_and_private_malformed_input() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let saved = save_evidence(root.path(), &source, &bundle, &id);
    let original = fs::read_to_string(&saved).unwrap();
    for (key, duplicate) in [
        ("\"acceptance\":", "\"acceptance\":\"FAKE_PRIVATE_DUPLICATE\",\"acceptance\":"),
        ("\"acceptance\":", "\"\\u0061cceptance\":\"not_evaluated\",\"acceptance\":"),
        ("\"configuration_fingerprint\":", "\"configuration_fingerprint\":\"FAKE_PRIVATE_DUPLICATE\",\"configuration_fingerprint\":"),
        ("\"obligation_id\":", "\"obligation_id\":\"FAKE_PRIVATE_DUPLICATE\",\"obligation_id\":"),
    ] {
        let duplicate = original.replacen(key, duplicate, 1);
        assert_ne!(duplicate, original);
        fs::write(&saved, duplicate).unwrap();
        replay_rejected(&source, &bundle, &id, &saved);
    }
    for malformed in [
        "".into(),
        "FAKE_PRIVATE_NOT_JSON".into(),
        "null".into(),
        "[]".into(),
        original[..original.len() / 2].to_owned(),
        format!("{original}{original}"),
        format!("{}0{}", "[".repeat(129), "]".repeat(129)),
    ] {
        fs::write(&saved, malformed).unwrap();
        replay_rejected(&source, &bundle, &id, &saved);
    }
    fs::write(&saved, [0xff, 0xfe]).unwrap();
    replay_rejected(&source, &bundle, &id, &saved);
}

#[test]
fn replay_file_and_pin_boundaries_are_fail_closed_without_following_report_paths() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let saved = save_evidence(root.path(), &source, &bundle, &id);
    let original = fs::read(&saved).unwrap();
    for pin in ["", "FAKE_PRIVATE_PIN", &"0".repeat(64), &format!("{id}\n")] {
        replay_rejected(&source, &bundle, pin, &saved);
    }
    let mut padded = original.clone();
    padded.resize(1024 * 1024, b' ');
    fs::write(&saved, &padded).unwrap();
    assert_success(
        &replay_command(&source, &bundle, &id, &saved)
            .output()
            .unwrap(),
    );
    padded.push(b' ');
    fs::write(&saved, padded).unwrap();
    replay_rejected(&source, &bundle, &id, &saved);
    fs::remove_file(&saved).unwrap();
    replay_rejected(&source, &bundle, &id, &saved);
    let target = root.path().join("FAKE_PRIVATE_TARGET.json");
    fs::write(&target, &original).unwrap();
    symlink(&target, &saved).unwrap();
    replay_rejected(&source, &bundle, &id, &saved);
    fs::remove_file(&saved).unwrap();
    fs::create_dir(&saved).unwrap();
    replay_rejected(&source, &bundle, &id, &saved);
    fs::remove_dir(&saved).unwrap();
    fs::write(&saved, original).unwrap();
    let snapshot = fs::read(bundle.join("program.ll")).unwrap();
    fs::remove_file(bundle.join("program.ll")).unwrap();
    replay_rejected(&source, &bundle, &id, &saved);
    fs::write(bundle.join("program.ll"), &snapshot).unwrap();
    fs::write(&source, format!("{GOOD}//FAKE_PRIVATE_STALE_SOURCE")).unwrap();
    replay_rejected(&source, &bundle, &id, &saved);
    fs::write(&source, GOOD).unwrap();
    let mut changed: serde_json::Value =
        serde_json::from_slice(&fs::read(&saved).unwrap()).unwrap();
    changed["evidence"][0]["artifact_paths"] =
        serde_json::json!(["FAKE_PRIVATE_PATH_MUST_NOT_BE_OPENED"]);
    fs::write(&saved, serde_json::to_vec(&changed).unwrap()).unwrap();
    replay_rejected(&source, &bundle, &id, &saved);
}

#[test]
fn saved_success_with_rebound_hashes_cannot_bypass_either_fresh_query() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, _) = build(root.path());
    for alter_source in [true, false] {
        fs::write(&source, GOOD).unwrap();
        let id = repin(&bundle, Some(GOOD));
        let saved = save_evidence(root.path(), &source, &bundle, &id);
        let mut report: serde_json::Value =
            serde_json::from_slice(&fs::read(&saved).unwrap()).unwrap();
        let original_ir = fs::read_to_string(bundle.join("program.ll")).unwrap();
        let text = if alter_source {
            GOOD.replace("== 7", "== 8")
        } else {
            GOOD.to_owned()
        };
        fs::write(&source, &text).unwrap();
        if !alter_source {
            fs::write(
                bundle.join("program.ll"),
                original_ir.replace("ret i64 7", "ret i64 4294967303"),
            )
            .unwrap();
        }
        let id = repin(&bundle, Some(&text));
        assert_success(&check(&source, &bundle, &id));
        let record: native_bundle::NativeBundleRecord =
            serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
        report["build_id"] = serde_json::json!(id);
        report["evidence"][0]["specification"]["sha256"] = serde_json::json!(id);
        report["evidence"][1]["subject"]["sha256"] = serde_json::json!(record.source.source_sha256);
        report["evidence"][1]["specification"]["sha256"] =
            serde_json::json!(record.source.specification_sha256);
        report["evidence"][2]["subject"]["sha256"] =
            serde_json::json!(record.files[&BundleFile::LlvmIr].sha256);
        report["evidence"][2]["specification"]["sha256"] =
            serde_json::json!(record.source.source_sha256);
        fs::write(&saved, serde_json::to_vec(&report).unwrap()).unwrap();
        let rejected = replay_rejected(&source, &bundle, &id, &saved);
        assert!(
            String::from_utf8_lossy(&rejected.stderr).contains(if alter_source {
                "source postcondition refuted"
            } else {
                "LLVM body differs from source"
            })
        );
        fs::write(bundle.join("program.ll"), original_ir).unwrap();
    }
}

#[test]
fn replay_of_rebound_garbage_artifacts_never_executes_or_promotes_binary_assurance() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let saved = save_evidence(root.path(), &source, &bundle, &id);
    let mut report: serde_json::Value = serde_json::from_slice(&fs::read(&saved).unwrap()).unwrap();
    let marker = root.path().join("MUST_NOT_RUN");
    fs::write(
        bundle.join("program"),
        format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
    )
    .unwrap();
    fs::set_permissions(bundle.join("program"), fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(bundle.join("program.o"), b"garbage object").unwrap();
    let id = repin(&bundle, None);
    replay_rejected(&source, &bundle, &id, &saved);
    report["build_id"] = serde_json::json!(id);
    report["evidence"][0]["specification"]["sha256"] = serde_json::json!(id);
    report["evidence"][0]["subject"]["sha256"] = serde_json::json!(native_bundle::sha256(
        &fs::read(bundle.join("program")).unwrap()
    ));
    fs::write(&saved, serde_json::to_vec(&report).unwrap()).unwrap();
    let output = replay_command(&source, &bundle, &id, &saved)
        .output()
        .unwrap();
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Object/linker/runtime/OS exit-status/executable semantics: not checked")
    );
    assert!(stdout.contains("Evidence acceptance: not evaluated"));
    assert!(!stdout.contains("artifact_verified"));
    assert!(!marker.exists());
}

const MODULE: &str = "atom helper(x: i64) -> i64 requires: x > 0; ensures: result == x + 1; body: { x + 1 };\natom main() -> i64 requires: true; ensures: result == 6; body: { helper(5) };";

const WRAPPED: &str = "atom id(x: i64) -> i64 requires: true; ensures: result == x; body: { x };\natom main() -> i64 requires: true; ensures: result > 0; body: { id(9223372036854775807) + 1 };";

fn build_source(root: &Path, text: &str) -> (std::path::PathBuf, std::path::PathBuf, String) {
    let source = root.join("source.mm");
    let bundle = root.join("bundle");
    fs::write(&source, text).unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    (source, bundle, id)
}

fn module_record(bundle: &Path) -> serde_json::Value {
    serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap()
}

#[test]
fn module_bundle_builds_checks_and_rechecks_within_its_own_profile() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), MODULE);
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MODULE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE);
    assert_eq!(record["source"]["atoms"].as_object().unwrap().len(), 2);
    // The dependency pin is the claimbook convention: sha256 of the
    // canonical JSON map {atom: [sorted direct callees]}.
    let dependencies = serde_json::json!({"helper": [], "main": ["helper"]});
    assert_eq!(
        record["source"]["source_dependencies_sha256"],
        native_bundle::sha256(&serde_json::to_vec(&dependencies).unwrap())
    );
    assert_success(&check(&source, &bundle, &id));
    let recheck = recheck(&source, &bundle, &id);
    assert_success(&recheck);
    let stdout = String::from_utf8_lossy(&recheck.stdout);
    assert!(stdout.contains(native_bundle::MODULE_RECHECK_PROFILE));
    let llvm = recheck_llvm(&source, &bundle, &id);
    assert_success(&llvm);
    let stdout = String::from_utf8_lossy(&llvm.stdout);
    assert!(stdout.contains(mumei_emit_llvm::translation_validation::NATIVE_MODULE_PROFILE));
}

#[test]
fn module_bundle_build_rejects_a_requires_loop_with_no_bounded_exit() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.mm");
    let bundle = root.path().join("bundle");
    fs::write(&source, NON_EXITING_REQUIRES_LOOP_MODULE).unwrap();

    let output = cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("source domain and loop exits empty on all inputs"));
    assert!(!bundle.exists());
}

#[test]
fn module_bundle_with_refined_scalar_members_builds_as_v2() {
    // A module declaring a refined scalar alias and using it in a
    // signature builds under the v2 profile: the record names the
    // refined admission, and check/recheck/recheck-llvm all re-derive
    // under that same gate.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "type Port = i64 where v >= 1 && v <= 65535;\n\
         atom listen(x: Port) -> Port requires: true; ensures: result == x; body: { x };\n\
         atom main() -> i64 requires: true; ensures: result == 80; body: { listen(80) };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MODULE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V2);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_REFINED_PROFILE
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // The emitted consumer evidence names the pinned v2 profile, not
    // the v1 constant — the identity record must say which admission
    // actually produced the bundle.
    let json = consume("recheck-llvm --json", &source, &bundle, &id);
    assert_success(&json);
    let envelope: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(
        envelope["evidence"][0]["semantics"]["profile"],
        native_bundle::MODULE_PROFILE_V2
    );
    // A v2 record re-pinned under the v1 name is not a v1 record —
    // the identity check refuses it rather than silently re-admitting
    // under the narrower gate.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn accept_object_recheck_machine_tv_rederives_refined_scalar_models() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "type Pos = i64 where v > 0;\n\
         atom inc(x: Pos) -> i64 requires: x < 9223372036854775807; ensures: result == x + 1; body: { x + 1 };\n\
         atom main() -> i64 requires: true; ensures: result == 2; body: { inc(1) };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V2);

    let recheck = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&recheck);
    let machine_tv: serde_json::Value = serde_json::from_slice(&recheck.stdout).unwrap();
    assert_eq!(
        machine_tv["functions"]["inc"]["outcome"],
        "equivalent_within_profile"
    );
    assert_eq!(
        machine_tv["input_domain"],
        "module_members_signed_i64_inputs_with_refined_scalar_signature_predicate_domains_not_process_exit_status"
    );

    let accepted = accept_object(&source, &bundle, &id, true);
    assert_success(&accepted);
    let descriptor: serde_json::Value = serde_json::from_slice(&accepted.stdout).unwrap();
    assert_eq!(descriptor["accepted"], true);
    assert_eq!(
        descriptor["functions"]["inc"]["outcome"],
        "equivalent_within_profile"
    );
    assert_eq!(descriptor["input_domain"], machine_tv["input_domain"]);
}

const COMPOSITE_MODULE: &str = "struct Point { x: i64, y: i64 }\n\
     atom gx(p: Point) -> i64 requires: p.x >= 0; ensures: result == p.x; body: { p.x };\n\
     atom fwd(p: Point) -> i64 requires: p.x >= 1; ensures: result >= 1; body: { gx(p) };\n\
     atom main() -> i64 requires: true; ensures: result == 0; body: { 0 };";

#[test]
fn module_bundle_with_struct_members_builds_as_v4_and_rechecks_its_source() {
    // Struct declarations, struct-typed parameters, field reads and a
    // same-struct pass-through call build under the composite profile;
    // check and the source recheck re-derive under that same gate.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), COMPOSITE_MODULE);
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MODULE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V4);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE
    );
    assert_eq!(record["source"]["atoms"].as_object().unwrap().len(), 3);
    assert_success(&check(&source, &bundle, &id));
    let output = recheck(&source, &bundle, &id);
    assert_success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains(native_bundle::MODULE_RECHECK_PROFILE));
    // The LLVM snapshot recheck pairs each `{ i64, i64 }` formal with
    // the struct parameter's flattened field slots.
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // A snapshot reading the fields in the wrong order is a different
    // member, even with every identity pin refreshed to match it.
    let ir = fs::read_to_string(bundle.join("program.ll")).unwrap();
    let swapped = ir
        .replace("%0, 0", "%0, 9")
        .replace("%0, 1", "%0, 0")
        .replace("%0, 9", "%0, 1");
    assert_ne!(swapped, ir);
    fs::write(bundle.join("program.ll"), swapped).unwrap();
    let swapped_id = repin_module(&bundle);
    assert_success(&check(&source, &bundle, &swapped_id));
    let llvm = recheck_llvm(&source, &bundle, &swapped_id);
    assert!(!llvm.status.success());
    fs::write(bundle.join("program.ll"), ir).unwrap();
    let id = repin_module(&bundle);
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // A v4 record re-pinned under any narrower name is refused: those
    // gates keep `struct` out of the item vocabulary.
    for narrower in [
        native_bundle::MODULE_PROFILE,
        native_bundle::MODULE_PROFILE_V2,
        native_bundle::MODULE_PROFILE_V3,
    ] {
        let mut record = module_record(&bundle);
        record["profile"] = serde_json::Value::String(narrower.into());
        let bytes = serde_json::to_vec_pretty(&record).unwrap();
        fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
        let id = native_bundle::sha256(&bytes);
        assert!(!check(&source, &bundle, &id).status.success());
        assert!(!recheck(&source, &bundle, &id).status.success());
    }
}

#[test]
fn module_bundle_with_alias_struct_fields_builds_as_v5_and_rechecks() {
    // `gx`'s postcondition holds only under `Pos`'s predicate on `p.x`:
    // the verifier now assumes it for struct fields, so the build and
    // every recheck bind the same field domain.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "type Pos = i64 where v > 0;\nstruct P { x: Pos, y: i64 }\n\
         atom gx(p: P) -> i64 requires: true; ensures: result >= 1; body: { p.x };\n\
         atom fwd(q: P) -> i64 requires: true; ensures: result >= 1; body: { gx(q) };\n\
         atom main() -> i64 requires: true; ensures: result == 0; body: { 0 };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V5);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V2
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // A v5 record re-pinned as v4 is refused: v4 keeps literal fields.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V4.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn literal_field_composite_modules_keep_pinning_v4() {
    let root = tempfile::tempdir().unwrap();
    let (_source, bundle, _id) = build_source(root.path(), COMPOSITE_MODULE);
    assert_eq!(
        module_record(&bundle)["profile"],
        native_bundle::MODULE_PROFILE_V4
    );
}

#[test]
fn module_bundle_without_structs_keeps_its_narrow_profile_under_the_composite_producer() {
    let root = tempfile::tempdir().unwrap();
    let (_source, bundle, _id) = build_source(root.path(), MODULE);
    assert_eq!(
        module_record(&bundle)["profile"],
        native_bundle::MODULE_PROFILE
    );
}

#[test]
fn effect_free_single_atom_main_keeps_the_single_bundle_profile() {
    let root = tempfile::tempdir().unwrap();
    let (_source, bundle, _id) = build_source(
        root.path(),
        "atom main() -> i64 requires: true; ensures: result == 0; body: { 0 };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::SCHEMA);
    assert_eq!(record["profile"], native_bundle::PROFILE);
}

#[test]
fn native_bundle_rejects_scalar_callback_parameter_sources() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.mm");
    let bundle = root.path().join("bundle");
    let callback_source =
        "atom zero(x: i64) -> i64 requires: true; ensures: result == 0; body: 0;\n\
        atom apply(f: atom_ref(i64) -> i64) -> i64 requires: true; \
        ensures: result >= 0; contract(f): ensures: result >= 0; body: call(f, 0);\n\
        atom main() -> i64 requires: true; ensures: result >= 0; \
        body: apply(atom_ref(zero));";
    fs::write(&source, callback_source).unwrap();

    let contracts = extract_pure_i64_module_contracts(
        callback_source,
        "callback.mm",
        None,
        RefinedAdmission::CompositeV27,
    )
    .expect("callback source should extract under CompositeV27");
    assert!(!contracts.is_empty());

    assert!(matches!(
        extract_pure_i64_module_contracts(
            callback_source,
            "callback.mm",
            None,
            RefinedAdmission::CompositeV26,
        ),
        Err(
            mumei_core::semantic_contract::SemanticContractError::Unsupported(
                "explicit owned scalar i64 signature required"
            )
        )
    ));

    let output = cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]);
    let output_text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "native bundles must stay on CompositeV26 for callback parameters; stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output_text.contains("unsupported or invalid module source"),
        "native bundle should retain its generic profile rejection; output: {output_text}"
    );
}

#[test]
fn native_bundle_rejects_inferred_refined_return_outside_composite_v26() {
    let root = tempfile::tempdir().unwrap();
    let source_path = root.path().join("source.mm");
    let bundle = root.path().join("bundle");
    let source = "type Nat = i64 where v >= 0;\n\
        atom identity(n: Nat) requires: true; ensures: result == n; body: n;\n\
        atom main() -> i64 requires: true; ensures: result >= 0; body: identity(1);";
    fs::write(&source_path, source).unwrap();

    let contracts = extract_pure_i64_module_contracts(
        source,
        "source.mm",
        None,
        RefinedAdmission::CompositeV28,
    )
    .expect("inferred refined return should extract under CompositeV28");
    assert!(contracts.iter().any(|contract| contract.profile
        == mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V28));
    assert!(matches!(
        extract_pure_i64_module_contracts(
            source,
            "source.mm",
            None,
            RefinedAdmission::CompositeV26
        ),
        Err(
            mumei_core::semantic_contract::SemanticContractError::Unsupported(
                "explicit owned scalar i64 signature required"
            )
        )
    ));

    let output = cli(&[
        "native-bundle",
        "build",
        source_path.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]);
    let output_text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success());
    assert!(
        output_text.contains("unsupported or invalid module source"),
        "native bundles must remain pinned to CompositeV26; output: {output_text}"
    );
}

#[test]
fn module_bundle_with_struct_and_single_main_takes_the_module_path() {
    // A lone `main` beside a struct declaration is still a composite
    // module — the single-atom path rejects every non-atom item.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "struct Point { x: i64 }\n\
         atom main() -> i64 requires: true; ensures: result == 0; body: { 0 };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MODULE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V4);
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    // No member takes a struct parameter, so no aggregate formal exists
    // and the LLVM snapshot recheck still reaches its scalar profile.
    assert_success(&recheck_llvm(&source, &bundle, &id));
}

#[test]
fn composite_source_recheck_refutes_wrapped_field_arithmetic() {
    // `verify` reasons over mathematical integers; the source recheck
    // encodes each flattened field as a wrapping i64, so the sum of two
    // nonnegative fields can wrap below the first one.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "struct Point { x: i64, y: i64 }\n\
         atom sum(p: Point) -> i64 requires: p.x >= 0 && p.y >= 0; ensures: result >= p.x; body: { p.x + p.y };\n\
         atom main() -> i64 requires: true; ensures: result == 0; body: { 0 };",
    );
    assert_eq!(
        module_record(&bundle)["profile"],
        native_bundle::MODULE_PROFILE_V4
    );
    assert_success(&check(&source, &bundle, &id));
    let output = recheck(&source, &bundle, &id);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("refuted"));
}

#[test]
fn multifile_bundle_with_struct_member_builds_as_v4() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        root.path().join("shapes.mm"),
        "struct Point { x: i64, y: i64 }\n\
         atom gx(p: Point) -> i64 requires: true; ensures: result == p.x; body: { p.x };\n\
         atom one(k: i64) -> i64 requires: true; ensures: result == k; body: { k };\n",
    )
    .unwrap();
    fs::write(
        &source,
        "import \"shapes\" as s;\n\natom main() -> i64 requires: true; ensures: result == 3; body: { s::one(3) };\n",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MULTIFILE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MULTIFILE_PROFILE_V4);
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // Under a v3 pin the struct member is refused at the member gate.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MULTIFILE_PROFILE_V3.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn module_bundle_with_struct_results_builds_as_v6_and_rechecks() {
    // `make` returns a struct literal, `main` reads a field of its call
    // result: the build, the source recheck and the LLVM snapshot
    // recheck all pair the returned aggregate with its field slots.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "type Pos = i64 where v > 0;\nstruct P { x: Pos, y: i64 }\n\
         struct Q { a: i64, b: i64, c: i64, d: i64 }\n\
         atom make(a: Pos) -> P requires: true; ensures: result.x == a; body: { P { x: a, y: 2 } };\n\
         atom make4(a: i64, b: i64, c: i64, d: i64) -> Q requires: true; ensures: result.a == a + 1; body: { Q { a: a + 1, b: b, c: c, d: d } };\n\
         atom uselet(a: Pos) -> i64 requires: true; ensures: result >= 1; body: { let q = make(a); q.x };\n\
         atom main() -> i64 requires: true; ensures: result == 5; body: { make(5).x };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V6);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V3
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    let machine_tv = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&machine_tv);
    let machine_tv: serde_json::Value = serde_json::from_slice(&machine_tv.stdout).unwrap();
    // Step 3d: a register-resident integer aggregate result joins the
    // direct LLVM-IR comparison — `make` records `equivalent`, while
    // callers whose struct-returning call site stays outside the
    // scalar-i64 call subset record their `NotLiftable` reason
    // (inspection only, no verdict).
    let checks = machine_tv["object_translation"]["correspondence"]["checks"]
        .as_array()
        .unwrap();
    let equivalence = checks
        .iter()
        .find(|check| check["item"] == "llvm_ir_object_equivalence")
        .unwrap();
    assert_eq!(equivalence["result"], "checked");
    let detail = equivalence["detail"].as_str().unwrap();
    assert!(detail.contains("make: equivalent"), "{detail}");
    // Step 3d second slice: a four-`i64` result goes memory-class
    // (`sret`) in the object code but still compares `equivalent`
    // against the aggregate `ret` in the produced IR.
    assert!(detail.contains("make4: equivalent"), "{detail}");
    assert!(detail.contains("uselet: unsupported_signature"), "{detail}");
    assert!(
        detail.contains("__mumei_user_main: unsupported_signature"),
        "{detail}"
    );
    // A v6 record re-pinned as v5 is refused: v5 keeps struct values out.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V5.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn multifile_bundle_with_struct_results_builds_as_v6() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        root.path().join("shapes.mm"),
        "struct Point { x: i64, y: i64 }\n\
         atom origin(k: i64) -> Point requires: true; ensures: result.y == k; body: { Point { x: 0, y: k } };\n",
    )
    .unwrap();
    fs::write(
        &source,
        "import \"shapes\" as s;\n\natom main() -> i64 requires: true; ensures: result == 3; body: { s::origin(3).y };\n",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MULTIFILE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MULTIFILE_PROFILE_V6);
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
}

#[test]
fn module_bundle_with_nested_struct_fields_builds_as_v11_and_rechecks() {
    // `set` writes a whole nested subtree (`o.i = w`) and `main` reads a
    // nested leaf of its call result: the build, the source recheck and
    // the LLVM snapshot recheck all pair the nested aggregate's leaf
    // slots under composite v8.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "struct I { a: i64, b: i64 }\nstruct O { i: I, c: i64 }\n\
         atom set(o: O, w: I) -> O requires: true; ensures: result.i.a == w.a; body: { o.i = w; o };\n\
         atom main() -> i64 requires: true; ensures: true; body: { set(O { i: I { a: 1, b: 2 }, c: 0 }, I { a: 8, b: 9 }).i.a };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V11);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V8
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // A v11 record re-pinned as v10 is refused: v10 keeps nested
    // struct fields out of the composite shape.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V10.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn module_bundle_rejects_colliding_flattened_field_names() {
    // `O { i: I, i_a: i64 }` flattens `i.a` and `i_a` onto the same
    // verifier symbol `i_a`: two independent fields would alias one
    // slot and could be proven equal, so admission must refuse it.
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        &source,
        "struct I { a: i64 }\nstruct O { i: I, i_a: i64 }\n\
         atom main() -> i64 requires: true; ensures: true; body: { 0 };",
    )
    .unwrap();
    assert!(!cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ])
    .status
    .success());
}

#[test]
fn module_bundle_rejects_exponential_struct_expansion() {
    // Every level names the same child type twice, so the flattened
    // leaf count doubles per level: the admission check must bail on
    // the bound during the count instead of materializing ~2^30 leaves
    // first (which would exhaust memory before it could reject).
    let mut decls = String::from("struct L0 { v: i64 }\n");
    for i in 1..30 {
        decls.push_str(&format!("struct L{i} {{ x: L{}, y: L{} }}\n", i - 1, i - 1));
    }
    decls.push_str("atom main() -> i64 requires: true; ensures: true; body: { 0 };");
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.mm");
    let bundle = root.path().join("bundle");
    fs::write(&source, &decls).unwrap();
    assert!(!cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ])
    .status
    .success());
}

#[test]
fn multifile_bundle_with_nested_struct_fields_builds_as_v11() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        root.path().join("shapes.mm"),
        "struct I { a: i64, b: i64 }\nstruct O { i: I, c: i64 }\n\
         atom set(o: O, w: I) -> O requires: true; ensures: result.i.a == w.a; body: { o.i = w; o };\n\
         atom get(o: O) -> i64 requires: true; ensures: result == o.i.a; body: { o.i.a };\n",
    )
    .unwrap();
    fs::write(
        &source,
        "import \"shapes\" as s;\n\natom main() -> i64 requires: true; ensures: true; body: { 0 };\n",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MULTIFILE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MULTIFILE_PROFILE_V11);
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // Under a v10 pin the same members stay refused at the member gate.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MULTIFILE_PROFILE_V10.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn module_bundle_with_generic_structs_builds_as_v12_and_rechecks() {
    // Generic `struct` declarations monomorphize to ordinary v8-shaped
    // declarations before verification: the build, the source recheck
    // and the LLVM snapshot recheck all run on the instantiated module.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "struct Pair<T> { a: T, b: T }\nstruct Box<T> { inner: T }\n\
         atom mk(a: i64) -> Pair<i64> requires: a >= 0; ensures: result.a == a && result.b == a; body: { Pair<i64>{a: a, b: a} };\n\
         atom boxed(b: Box<Pair<i64>>) -> i64 requires: true; ensures: result == b.inner.a; body: { b.inner.a };\n\
         atom main() -> i64 requires: true; ensures: result == 2; body: { let p = Pair<i64>{a: 2, b: 2}; p.a };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V12);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V9
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // A v12 record re-pinned as v11 is refused: v11 keeps generic
    // struct declarations out of the vocabulary.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V11.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn multifile_bundle_with_generic_structs_builds_as_v12() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        root.path().join("shapes.mm"),
        "struct Pair<T> { a: T, b: T }\n\
         atom sum(p: Pair<i64>) -> i64 requires: p.a >= 0 && p.b >= 0; ensures: result == p.a + p.b; body: { p.a + p.b };\n",
    )
    .unwrap();
    fs::write(
        &source,
        "import \"shapes\" as s;\n\natom main() -> i64 requires: true; ensures: true; body: { 0 };\n",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MULTIFILE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MULTIFILE_PROFILE_V12);
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // Under a v11 pin the member's generic declarations stay refused.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MULTIFILE_PROFILE_V11.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn module_bundle_with_u64_leaves_builds_as_v13_and_rechecks() {
    // `u64` leaf parameters/fields/results pin the module at composite
    // v10: unsigned comparisons and `udiv`/`urem`/`lshr` semantics on
    // both the source encoding and the emitted `icmp u*`/`udiv`/`lshr`.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "struct Bucket { total: u64, count: i64 }\n\
         atom add(b: Bucket, e: u64) -> Bucket requires: true; ensures: result.total == b.total + e && result.count == b.count; body: { Bucket{total: b.total + e, count: b.count} };\n\
         atom half(a: u64) -> u64 requires: a % 2 == 0; ensures: result * 2 == a; body: { a / 2 };\n\
         atom main() -> i64 requires: true; ensures: result == 1; body: { let b = Bucket{total: 4, count: 1}; if add(b, 2).total >= 6 { 1 } else { 0 } };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V13);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V10
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // A v13 record re-pinned as v12 is refused: v12 keeps u64 leaves
    // out of the vocabulary.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V12.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn module_bundle_with_bounded_arrays_builds_as_v14_and_rechecks() {
    // `let`-bound stack arrays pin the module at composite v11 /
    // bundle v14: the bounded-memory source encoding (SMT-array cells
    // with graceful out-of-bounds semantics) pairs with the emitter's
    // `alloca [N x i64]` + gep/load/store lowering.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "atom sum() -> i64 requires: true; ensures: result == 13; body: { let a = [1, 2, 3]; a[1] = 9; a[0] + a[1] + a[2] };\n\
         atom pick(a0: i64, i: i64) -> i64 requires: 0 <= i && i < 3; ensures: result == 7; body: { let a = [a0, 20, 30]; a[i] = 7; a[i] };\n\
         atom main() -> i64 requires: true; ensures: result == 20; body: { sum() + pick(1, 0) };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V14);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_MEMORY_PROFILE
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    let machine_tv = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&machine_tv);
    let machine_tv: serde_json::Value = serde_json::from_slice(&machine_tv.stdout).unwrap();
    for function in ["sum", "pick", "main"] {
        assert_eq!(
            machine_tv["functions"][function]["outcome"],
            "equivalent_within_profile"
        );
    }
    // Step 3c: bounded-memory functions also enter the direct LLVM-IR
    // comparison subset — the produced IR for the bounded arrays encodes
    // and compares `equivalent` against the object code, recorded on the
    // `llvm_ir_object_equivalence` item (inspection only, no verdict).
    let checks = machine_tv["object_translation"]["correspondence"]["checks"]
        .as_array()
        .unwrap();
    let equivalence = checks
        .iter()
        .find(|check| check["item"] == "llvm_ir_object_equivalence")
        .unwrap();
    assert_eq!(equivalence["result"], "checked");
    let detail = equivalence["detail"].as_str().unwrap();
    for symbol in ["sum", "pick", "__mumei_user_main"] {
        assert!(
            detail.contains(&format!("{symbol}: equivalent")),
            "{detail}"
        );
    }
    assert!(machine_tv["register_convention_assumed"]
        .as_str()
        .unwrap()
        .contains("_leaf_red_zone_frame_memory_assumed_unclobbered"));
    // DESIGN_OBJECT_ACCEPTANCE.md §4: `main` calls the array atoms,
    // which sit outside the HIR↔LLVM liftable subset, leaving
    // `hir_llvm_model_correspondence` unchecked → inconclusive.
    let accepted = accept_object(&source, &bundle, &id, true);
    assert_eq!(accepted.status.code(), Some(3));
    assert!(accepted.stdout.is_empty());
    assert!(String::from_utf8_lossy(&accepted.stderr).contains("hir_llvm_model_correspondence"));
    // A v14 record re-pinned as v13 is refused: v13 keeps arrays out of
    // the vocabulary.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V13.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn module_bundle_with_bounded_arrays_in_allocated_frame_recheck_machine_tv_and_accept_object() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "atom inc(x: i64) -> i64 requires: x >= 0 && x < 1000; ensures: result == x + 1; body: { x + 1 };\n\
         atom usea(a0: i64, i: i64) -> i64 requires: 0 <= i && i < 3 && a0 >= 0 && a0 < 1000; ensures: true; body: { let a = [a0, 20, 30]; a[i] = inc(a0); a[i] };\n\
         atom main() -> i64 requires: true; ensures: true; body: { usea(1, 1) };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V14);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_MEMORY_PROFILE
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    let machine_tv = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&machine_tv);
    let machine_tv: serde_json::Value = serde_json::from_slice(&machine_tv.stdout).unwrap();
    for function in ["inc", "usea", "main"] {
        assert_eq!(
            machine_tv["functions"][function]["outcome"],
            "equivalent_within_profile"
        );
    }
    let convention = machine_tv["register_convention_assumed"].as_str().unwrap();
    assert!(convention.contains("_allocated_frame_memory_assumed_callee_isolated"));

    let accepted = accept_object(&source, &bundle, &id, true);
    assert_success(&accepted);
    let accepted: serde_json::Value = serde_json::from_slice(&accepted.stdout).unwrap();
    assert_eq!(accepted["accepted"], true);
    let convention = accepted["register_convention_assumed"].as_str().unwrap();
    assert!(convention.contains("_allocated_frame_memory_assumed_callee_isolated"));
}

#[test]
fn multifile_bundle_with_bounded_arrays_builds_as_v14() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        root.path().join("cell.mm"),
        "atom pick(i: i64) -> i64 requires: 0 <= i && i < 2; ensures: result == 5; body: { let a = [3, 5]; a[i] = 5; a[i] };\n",
    )
    .unwrap();
    fs::write(
        &source,
        "import \"cell\" as c;\n\natom main() -> i64 requires: true; ensures: true; body: { c::pick(1) };\n",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MULTIFILE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MULTIFILE_PROFILE_V14);
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
}

#[test]
fn multifile_bundle_with_u64_leaves_builds_as_v13() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        root.path().join("amounts.mm"),
        "atom add(a: u64, b: u64) -> u64 requires: true; ensures: result == a + b; body: { a + b };\n",
    )
    .unwrap();
    fs::write(
        &source,
        "import \"amounts\" as amt;\n\natom main() -> i64 requires: true; ensures: true; body: { 0 };\n",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MULTIFILE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MULTIFILE_PROFILE_V13);
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // Under a v12 pin the member's u64 leaf stays refused.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MULTIFILE_PROFILE_V12.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn recheck_machine_tv_accept_object_u64_module() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "struct Pair { amount: u64, tag: i64 }\n\
         atom add(a: u64, b: u64) -> u64 requires: true; ensures: true; body: { a + b };\n\
         atom pass(p: Pair) -> Pair requires: true; ensures: true; body: { p };\n\
         atom callee(x: u64) -> u64 requires: true; ensures: true; body: { x + 1 };\n\
         atom caller(x: u64) -> u64 requires: true; ensures: true; body: { callee(x) };\n\
         atom bool_flag(b: bool) -> i64 requires: true; ensures: true; body: { 0 };\n\
         atom main() -> i64 requires: true; ensures: true; body: { 0 };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V13);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V10
    );

    let machine_tv = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&machine_tv);
    let machine_tv: serde_json::Value = serde_json::from_slice(&machine_tv.stdout).unwrap();
    for function in ["add", "pass", "callee", "caller", "bool_flag"] {
        assert_eq!(
            machine_tv["functions"][function]["outcome"],
            "equivalent_within_profile"
        );
    }

    let llvm = recheck_llvm(&source, &bundle, &id);
    assert_success(&llvm);
    assert_eq!(
        machine_tv["input_domain"],
        "signed_i64_inputs_with_signature_bounded_param_domains_where_division_and_shift_defined_not_restricted_by_requires"
    );

    let accepted = accept_object(&source, &bundle, &id, true);
    assert_success(&accepted);
    let accepted: serde_json::Value = serde_json::from_slice(&accepted.stdout).unwrap();
    assert_eq!(accepted["accepted"], true);
    assert_eq!(accepted["input_domain"], machine_tv["input_domain"]);
}

#[test]
fn module_bundle_with_f64_leaves_builds_as_v15_and_rechecks() {
    // `f64` leaf parameters/fields/results pin the module at composite
    // v11: one 64-bit `double` slot per leaf carrying the IEEE-754 bit
    // pattern — pass-through on both sides, never read by arithmetic
    // or comparison.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "struct B { v: f64, t: i64 }\n\
         atom wrap(x: f64, t: i64) -> B requires: t >= 0; ensures: result.t == t; body: { B{v: x, t: t} };\n\
         atom unwrap(b: B) -> f64 requires: b.t >= 0; ensures: true; body: { b.v };\n\
         atom pick(f: bool, x: f64, y: f64) -> f64 requires: true; ensures: true; body: { if f { x } else { y } };\n\
         atom main() -> i64 requires: true; ensures: result == 1; body: { 1 };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V15);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V11
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // The direct LLVM-IR comparison admits `pick`'s scalar `f64`
    // result and `wrap`'s flat mixed result. `unwrap`'s composite
    // parameter stays outside the subset.
    let machine_tv = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&machine_tv);
    let machine_tv: serde_json::Value = serde_json::from_slice(&machine_tv.stdout).unwrap();
    let checks = machine_tv["object_translation"]["correspondence"]["checks"]
        .as_array()
        .unwrap();
    let equivalence = checks
        .iter()
        .find(|check| check["item"] == "llvm_ir_object_equivalence")
        .unwrap();
    assert_eq!(equivalence["result"], "checked");
    let detail = equivalence["detail"].as_str().unwrap();
    let (_, outcomes) = detail.split_once(" (").unwrap();
    let outcomes: Vec<_> = outcomes.strip_suffix(')').unwrap().split(", ").collect();
    for expected in [
        "pick: equivalent",
        "__mumei_user_main: equivalent",
        "wrap: equivalent",
        "unwrap: unsupported_signature",
    ] {
        assert!(outcomes.contains(&expected), "{detail}");
    }
    // A v15 record re-pinned as v14 is refused: v14 keeps f64 leaves
    // out of the vocabulary.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V14.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn multifile_bundle_with_f64_leaves_builds_as_v15() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        root.path().join("rates.mm"),
        "atom copy(a: f64) -> f64 requires: true; ensures: true; body: { a };\n",
    )
    .unwrap();
    fs::write(
        &source,
        "import \"rates\" as r;\n\natom main() -> i64 requires: true; ensures: true; body: { 0 };\n",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MULTIFILE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MULTIFILE_PROFILE_V15);
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // Under a v14 pin the member's f64 leaf stays refused.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MULTIFILE_PROFILE_V14.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn module_bundle_with_nested_field_writes_builds_as_v18_and_rechecks() {
    // `bump` writes a leaf two levels deep (`o.i.a = w`): the build,
    // the source recheck and the LLVM snapshot recheck all carry the
    // desugared subtree rebuild under composite v15.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "struct I { a: i64, b: i64 }\nstruct O { i: I, c: i64 }\n\
         atom bump(o: O, w: i64) -> O requires: true; ensures: result.i.a == w; body: { o.i.a = w; o };\n\
         atom main() -> i64 requires: true; ensures: true; body: { bump(O { i: I { a: 1, b: 2 }, c: 0 }, 8).i.a };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V18);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V15
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    let machine_tv = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&machine_tv);
    let machine_tv: serde_json::Value = serde_json::from_slice(&machine_tv.stdout).unwrap();
    for function in ["bump", "main"] {
        assert_eq!(
            machine_tv["functions"][function]["outcome"],
            "equivalent_within_profile"
        );
    }
    assert!(machine_tv["checks_performed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check == "machine_code_composite_register_aggregate_tv"));
    // DESIGN_OBJECT_ACCEPTANCE.md §4: `main` calls `bump`, whose
    // struct return is outside both correspondence subsets → the
    // unchecked items keep acceptance inconclusive.
    let accepted = accept_object(&source, &bundle, &id, true);
    assert_eq!(accepted.status.code(), Some(3));
    assert!(accepted.stdout.is_empty());
    assert!(String::from_utf8_lossy(&accepted.stderr).contains("llvm_ir_object_equivalence"));
    // A v18 record re-pinned as v16 is refused: v16 keeps nested
    // leaf writes out of the composite shape.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V16.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn module_bundle_with_enum_payload_writes_builds_as_v19_and_rechecks() {
    // `bump` writes a named payload slot (`t.p = w`): the build, the
    // source recheck and the LLVM snapshot recheck all carry the
    // tag-gated payload rebuild under composite v16.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "enum C { T(p: i64), U }\n\
         atom bump(w: i64) -> C requires: true; ensures: result == C::T(w); body: { let t = C::T(0); t.p = w; t };\n\
         atom main() -> i64 requires: true; ensures: true; body: { 0 };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V19);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V16
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // A v19 record re-pinned as v18 is refused: v18 keeps enum
    // payload writes out of the composite shape.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V18.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn module_bundle_with_enum_struct_fields_builds_as_v20_and_rechecks() {
    // `pick` matches on an enum-typed struct field (`b.c`): the build,
    // the source recheck and the LLVM snapshot recheck all carry the
    // nested tagged-union field under composite v17.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "enum C { R, G(v: i64) }\nstruct B { i: i64, c: C }\n\
         atom pick(b: B) -> i64 requires: b.i >= 0; ensures: result >= 0; body: { match b.c { C::R => 1, C::G(v) => if v >= 0 { v } else { 0 }, _ => 0 } };\n\
         atom main() -> i64 requires: true; ensures: true; body: { 0 };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V20);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V17
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // A v20 record re-pinned as v19 is refused: v19 keeps enum-typed
    // struct fields out of the composite shape.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V19.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn module_bundle_with_composite_enum_payloads_builds_as_v21_and_rechecks() {
    // `step` matches on an enum carrying a struct payload (`Move(pos:
    // Point)`): the build, the source recheck and the LLVM snapshot
    // recheck all carry the composite-typed payload slot under
    // composite v18.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "struct Point { x: i64, y: i64 }\nenum Event { Ping, Move(pos: Point) }\n\
         atom step(e: Event) -> i64 requires: true; ensures: result >= 0; body: { match e { Event::Ping => 0, Event::Move(p) => if p.x + p.y >= 0 { p.x + p.y } else { 0 }, _ => 0 } };\n\
         atom main() -> i64 requires: true; ensures: true; body: { 0 };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V21);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V18
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // A v21 record re-pinned as v20 is refused: v20 keeps
    // composite-typed enum payloads out of the composite shape.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V20.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn multifile_bundle_with_composite_enum_payloads_builds_as_v21() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        root.path().join("events.mm"),
        "struct Point { x: i64, y: i64 }\nenum Event { Ping, Move(pos: Point) }\n\
         atom step(e: Event) -> i64 requires: true; ensures: result >= 0; body: { match e { Event::Ping => 0, Event::Move(p) => if p.x + p.y >= 0 { p.x + p.y } else { 0 }, _ => 0 } };\n",
    )
    .unwrap();
    fs::write(
        &source,
        "import \"events\" as ev;\n\natom main() -> i64 requires: true; ensures: true; body: { 0 };\n",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MULTIFILE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MULTIFILE_PROFILE_V21);
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // Under a v20 pin the member's composite payload stays refused.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MULTIFILE_PROFILE_V20.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn recheck_machine_tv_accept_object_f64_module_bundle() {
    // `fadd` computes real IEEE 754 binary64 arithmetic on `f64`
    // leaves (`x * y + x`): the build, the source recheck and
    // the LLVM snapshot recheck all carry the decode/compute/re-encode
    // under composite v19.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "atom fadd(x: f64, y: f64) -> f64 requires: true; ensures: true; body: { x * y + x };\n\
         atom feq(x: f64, y: f64) -> bool requires: true; ensures: result == (x == y); body: { x == y };\n\
         atom mixed(x: i64, y: f64) -> f64 requires: true; ensures: true; body: { y };\n\
         atom callee(x: f64) -> f64 requires: true; ensures: true; body: { x };\n\
         atom caller(x: f64) -> f64 requires: true; ensures: true; body: { callee(x) };\n\
         atom main() -> i64 requires: true; ensures: true; body: { 0 };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V22);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V19
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    let machine_tv = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&machine_tv);
    let machine_tv: serde_json::Value = serde_json::from_slice(&machine_tv.stdout).unwrap();
    assert_eq!(
        machine_tv["register_convention_assumed"],
        "sysv_x86_64_integer_args_rdi_rsi_rdx_rcx_r8_r9_f64_args_xmm0_7_result_rax_or_xmm0"
    );
    for function in ["fadd", "feq", "mixed", "callee", "caller"] {
        assert_eq!(
            machine_tv["functions"][function]["outcome"],
            "equivalent_within_profile"
        );
        assert_eq!(
            machine_tv["functions"][function]["callee_saved_preservation"],
            "preserved"
        );
    }
    assert!(machine_tv["checks_performed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check == "machine_code_f64_scalar_function_tv"));
    for check in [
        "machine_code_tv_mxcsr_control_state",
        "machine_code_tv_nan_payload_bits",
    ] {
        assert!(machine_tv["checks_not_performed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|not_performed| not_performed == check));
    }
    let accepted = accept_object(&source, &bundle, &id, true);
    assert_success(&accepted);
    let accepted: serde_json::Value = serde_json::from_slice(&accepted.stdout).unwrap();
    assert_eq!(accepted["accepted"], true);
    assert_eq!(
        accepted["register_convention_assumed"],
        "sysv_x86_64_integer_args_rdi_rsi_rdx_rcx_r8_r9_f64_args_xmm0_7_result_rax_or_xmm0"
    );
    assert!(accepted["checks_performed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check == "machine_code_f64_scalar_function_tv"));
    for check in [
        "machine_code_tv_mxcsr_control_state",
        "machine_code_tv_nan_payload_bits",
    ] {
        assert!(accepted["checks_not_performed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|not_performed| not_performed == check));
    }
    // A v22 record re-pinned as v21 is refused: v21 keeps `f64`
    // arithmetic out of the composite shape.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V21.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn recheck_machine_tv_accepts_small_integer_struct_aggregates() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "struct P { x: i64, y: i64 }\n\
         atom sum(p: P) -> i64 requires: true; ensures: true; body: { p.x + p.y };\n\
         atom mk(a: i64, b: i64) -> P requires: true; ensures: true; body: { P { x: a, y: b } };\n\
         atom caller() -> i64 requires: true; ensures: true; body: { sum(mk(1, 2)) };\n\
         atom fwd(p: P) -> i64 requires: true; ensures: true; body: { sum(p) };\n\
         atom main() -> i64 requires: true; ensures: true; body: { 0 };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V6);

    let machine_tv = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&machine_tv);
    let machine_tv: serde_json::Value = serde_json::from_slice(&machine_tv.stdout).unwrap();
    assert_eq!(
        machine_tv["register_convention_assumed"],
        "sysv_x86_64_integer_args_rdi_rsi_rdx_rcx_r8_r9_result_rax_two_eightbyte_integer_aggregates_in_consecutive_gprs_result_rax_rdx"
    );
    for function in ["sum", "mk", "caller", "fwd"] {
        assert_eq!(
            machine_tv["functions"][function]["outcome"],
            "equivalent_within_profile"
        );
    }
    assert!(machine_tv["checks_performed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check == "machine_code_composite_register_aggregate_tv"));

    // The composite call sites flatten to the same leaf slots the IR
    // `call` encoding splices, so every atom verifies inside the leaf
    // subset and the object is accepted.
    let accepted = accept_object(&source, &bundle, &id, true);
    assert_success(&accepted);
    let accepted: serde_json::Value = serde_json::from_slice(&accepted.stdout).unwrap();
    assert_eq!(accepted["accepted"], true);
}

#[test]
fn recheck_machine_tv_accept_object_three_leaf_struct_aggregates_in_one_source_file() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "struct T { a: i64, b: i64, c: i64 }\n\
         atom sum3(p: T) -> i64 requires: true; ensures: true; body: { p.a + p.b + p.c };\n\
         atom mk3(a: i64, b: i64, c: i64) -> T requires: true; ensures: true; body: { T { a: a, b: b, c: c } };\n\
         atom caller() -> i64 requires: true; ensures: true; body: { sum3(mk3(1, 2, 3)) };\n\
         atom fwd(p: T) -> i64 requires: true; ensures: true; body: { sum3(p) };\n\
         atom main() -> i64 requires: true; ensures: true; body: { 0 };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V6);

    let machine_tv = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&machine_tv);
    let machine_tv: serde_json::Value = serde_json::from_slice(&machine_tv.stdout).unwrap();
    assert_eq!(
        machine_tv["register_convention_assumed"],
        "sysv_x86_64_integer_args_rdi_rsi_rdx_rcx_r8_r9_result_rax_llvm_first_class_aggregates_split_into_registers_result_rax_rdx_rcx_xmm0_xmm1"
    );
    for function in ["sum3", "mk3", "caller", "fwd"] {
        assert_eq!(
            machine_tv["functions"][function]["outcome"],
            "equivalent_within_profile"
        );
    }
    assert!(machine_tv["checks_performed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check == "machine_code_composite_register_aggregate_tv"));
    assert!(machine_tv["checks_not_performed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check == "machine_code_tv_sysv_memory_class_aggregate_abi"));

    // The composite call sites flatten to the same leaf slots the IR
    // `call` encoding splices, so every atom verifies inside the leaf
    // subset and the object is accepted.
    let accepted = accept_object(&source, &bundle, &id, true);
    assert_success(&accepted);
    let accepted: serde_json::Value = serde_json::from_slice(&accepted.stdout).unwrap();
    assert_eq!(accepted["accepted"], true);
}

#[test]
fn recheck_machine_tv_accepts_mixed_f64_struct_aggregates() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "struct Q { a: i64, b: f64 }\n\
         atom mkq(a: i64, b: f64) -> Q requires: true; ensures: true; body: { Q { a: a, b: b } };\n\
         atom getb(q: Q) -> f64 requires: true; ensures: true; body: { q.b };\n\
         atom use_q(x: f64) -> f64 requires: true; ensures: true; body: { getb(mkq(1, x)) };\n\
         atom main() -> i64 requires: true; ensures: true; body: { 0 };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V15);

    let machine_tv = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&machine_tv);
    let machine_tv: serde_json::Value = serde_json::from_slice(&machine_tv.stdout).unwrap();
    assert_eq!(
        machine_tv["register_convention_assumed"],
        "sysv_x86_64_integer_args_rdi_rsi_rdx_rcx_r8_r9_f64_args_xmm0_7_result_rax_or_xmm0_two_eightbyte_aggregates_classified_integer_gprs_sse_xmm_result_rax_rdx_xmm0_xmm1"
    );
    for function in ["mkq", "getb", "use_q"] {
        assert_eq!(
            machine_tv["functions"][function]["outcome"],
            "equivalent_within_profile"
        );
    }
    for check in [
        "machine_code_f64_scalar_function_tv",
        "machine_code_composite_register_aggregate_tv",
    ] {
        assert!(machine_tv["checks_performed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|performed| performed == check));
    }
    for check in [
        "machine_code_tv_mxcsr_control_state",
        "machine_code_tv_nan_payload_bits",
    ] {
        assert!(machine_tv["checks_not_performed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|not_performed| not_performed == check));
    }

    let accepted = accept_object(&source, &bundle, &id, true);
    assert_success(&accepted);
    let accepted: serde_json::Value = serde_json::from_slice(&accepted.stdout).unwrap();
    assert_eq!(accepted["accepted"], true);
    assert_eq!(
        accepted["register_convention_assumed"],
        machine_tv["register_convention_assumed"]
    );
    assert!(accepted["checks_performed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check == "machine_code_composite_register_aggregate_tv"));
}

#[test]
fn recheck_machine_tv_accept_object_sixteen_leaf_struct_arguments_spill_to_stack() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "struct P { a: i64, b: i64, c: i64, d: i64, e: i64, f: i64, g: i64, h: i64, i: i64, j: i64, k: i64, l: i64, m: i64, n: i64, o: i64, p: i64 }\n\
         atom first_last(p: P) -> i64 requires: true; ensures: true; body: { p.a + p.p };\n\
         atom main() -> i64 requires: true; ensures: true; body: { first_last(P { a: 1, b: 2, c: 3, d: 4, e: 5, f: 6, g: 7, h: 8, i: 9, j: 10, k: 11, l: 12, m: 13, n: 14, o: 15, p: 16 }) };",
    );

    let machine_tv = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&machine_tv);
    let machine_tv: serde_json::Value = serde_json::from_slice(&machine_tv.stdout).unwrap();
    assert_eq!(
        machine_tv["register_convention_assumed"],
        "sysv_x86_64_integer_args_rdi_rsi_rdx_rcx_r8_r9_result_rax_llvm_first_class_aggregates_split_into_registers_result_rax_rdx_rcx_xmm0_xmm1"
    );
    for function in ["first_last", "main"] {
        assert_eq!(
            machine_tv["functions"][function]["outcome"], "equivalent_within_profile",
            "{function}"
        );
    }
    assert!(machine_tv["checks_performed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check == "machine_code_composite_register_aggregate_tv"));
    assert!(machine_tv["checks_not_performed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check == "machine_code_tv_sysv_memory_class_aggregate_abi"));

    // DESIGN_OBJECT_ACCEPTANCE.md §4: `llvm_ir_object_equivalence` is
    // unchecked for a 16-leaf spilled aggregate signature — outside the
    // IR↔object comparison subset — so acceptance is inconclusive
    // (fail-closed; machine-TV equivalence alone no longer suffices).
    let accepted = accept_object(&source, &bundle, &id, true);
    assert_eq!(accepted.status.code(), Some(3));
    assert!(accepted.stdout.is_empty());
    assert!(String::from_utf8_lossy(&accepted.stderr).contains("llvm_ir_object_equivalence"));
}

#[test]
fn recheck_machine_tv_accepts_f64_equality_branch() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "atom eq_branch(x: f64, y: f64) -> i64 requires: true; ensures: true; body: { if x == y { 1 } else { 0 } };\n\
         atom main() -> i64 requires: true; ensures: true; body: { 0 };",
    );

    let output = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&output);
    let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        descriptor["functions"]["eq_branch"]["outcome"],
        "equivalent_within_profile"
    );
    assert_eq!(
        descriptor["functions"]["eq_branch"]["callee_saved_preservation"],
        "preserved"
    );
}

#[test]
fn module_bundle_rejects_a_nan_refutable_f64_ensures() {
    // `result == x + y` reads as `fp.eq(result, result)` under the
    // admitted IEEE encoding — false whenever `x + y` is NaN, so the
    // contract is refutable. The build must verify the source under the
    // same IEEE semantics the rechecks pin: the default exact-rational
    // `Real` model would prove it and ship a "verified" certificate the
    // source recheck contradicts.
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    // A second atom makes `build` take the module path
    // (`build_module`), where `emit_module` verifies `add` under the
    // admission-derived IEEE config — a single-atom source would bail
    // out on the `build_single` admission gate before any contract
    // check ran.
    fs::write(
        &source,
        "atom add(x: f64, y: f64) -> f64 requires: true; ensures: result == x + y; body: { x + y };\n\
         atom main() -> i64 requires: true; ensures: true; body: { 0 };",
    )
    .unwrap();
    let output = cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("add"),
        "expected the failed atom in the error output, got: {stderr}"
    );
}

#[test]
fn module_bundle_str_index_stays_outside_the_bundlable_fragment() {
    // Scope boundary: a `s[i]` byte read is inside the v20 composite
    // contract but outside the general verifier the bundle build runs
    // — it rejects the index as out-of-bounds, so the build fails
    // closed before emitting `gep i8`/`load i8` (a byte-cell model in
    // either layer is a separate slice).
    let root = tempfile::tempdir().unwrap();
    let literal = "a".repeat(100);
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        &source,
        format!(
            "atom big() -> u64 requires: true; ensures: result == 97; str_bound: 128; body: {{ let s = \"{literal}\"; s[99] }};\n\
             atom main() -> i64 requires: true; ensures: true; body: {{ 0 }};"
        ),
    )
    .unwrap();
    assert!(!cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ])
    .status
    .success());
}

#[test]
fn module_bundle_with_str_bound_builds_as_v23_and_rechecks() {
    // A declared `str_bound: N;` clause admits the longer literal
    // (`pure-i64-composite/v20`) and the record pins bundle v23.
    let root = tempfile::tempdir().unwrap();
    let literal = "a".repeat(100);
    let (source, bundle, id) = build_source(
        root.path(),
        &format!(
            "atom big() -> i64 requires: true; ensures: result == 100; str_bound: 128; body: {{ let s = \"{literal}\"; len(s) }};\n\
             atom main() -> i64 requires: true; ensures: true; body: {{ 0 }};"
        ),
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V23);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V20
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // A v23 record re-pinned as v22 is refused: v22 keeps the
    // `str_bound` clause out of the composite shape.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V22.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn multifile_bundle_with_str_bound_builds_as_v23() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    let literal = "a".repeat(80);
    fs::write(
        root.path().join("text.mm"),
        format!(
            "atom first() -> i64 requires: true; ensures: result == 80; str_bound: 96; body: {{ let s = \"{literal}\"; len(s) }};\n"
        ),
    )
    .unwrap();
    fs::write(
        &source,
        "import \"text\" as t;\n\natom main() -> i64 requires: true; ensures: true; body: { 0 };\n",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MULTIFILE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MULTIFILE_PROFILE_V23);
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // Under a v22 pin the member's `str_bound` clause stays refused.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MULTIFILE_PROFILE_V22.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn module_bundle_with_i128_leaves_builds_as_v24_and_rechecks() {
    // An `i128`/`u128` leaf rides the two-slot pairing
    // (`pure-i64-composite/v21`); the record pins bundle v24 and the
    // declared `leaf_widths` so a rechecker re-derives the slot split.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "atom add(x: i128, y: i128) -> i128 requires: true; ensures: result == x + y; body: { x + y };\n\
         atom lt(a: u128, b: u128) -> bool requires: true; ensures: result == (a < b); body: { a < b };\n\
         atom callee(x: i128) -> i128 requires: true; ensures: result == x + 1; body: { x + 1 };\n\
         atom caller(x: i128) -> i128 requires: true; ensures: result == callee(x); body: { callee(x) };\n\
         atom main() -> i64 requires: true; ensures: true; body: { 0 };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V24);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V21
    );
    // The pinned pairing names every atom's declared leaf widths.
    assert_eq!(record["leaf_widths"]["add"], serde_json::json!([128, 128]));
    assert_eq!(record["leaf_widths"]["lt"], serde_json::json!([128, 128]));
    assert_eq!(record["leaf_widths"]["callee"], serde_json::json!([128]));
    assert_eq!(record["leaf_widths"]["caller"], serde_json::json!([128]));
    assert_eq!(record["leaf_widths"]["main"], serde_json::json!([]));
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    let machine_tv = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&machine_tv);
    let machine_tv: serde_json::Value = serde_json::from_slice(&machine_tv.stdout).unwrap();
    assert_eq!(
        machine_tv["register_convention_assumed"],
        "sysv_x86_64_integer_args_rdi_rsi_rdx_rcx_r8_r9_128_bit_scalars_in_consecutive_gpr_pairs_low_first_result_rax_rdx"
    );
    for function in ["add", "lt", "callee", "caller"] {
        assert_eq!(
            machine_tv["functions"][function]["outcome"],
            "equivalent_within_profile"
        );
    }
    let accepted = accept_object(&source, &bundle, &id, true);
    assert_success(&accepted);
    let accepted: serde_json::Value = serde_json::from_slice(&accepted.stdout).unwrap();
    assert_eq!(accepted["accepted"], true);
    assert_eq!(
        accepted["register_convention_assumed"],
        machine_tv["register_convention_assumed"]
    );
    // Step 3e: scalar i128/u128 signatures join the direct LLVM-IR
    // comparison — `add`/`lt`/`callee` record `equivalent`, while
    // `caller`'s i128 call site stays outside the scalar-i64 call
    // subset and records its `NotLiftable` reason (inspection only,
    // no verdict).
    let checks = machine_tv["object_translation"]["correspondence"]["checks"]
        .as_array()
        .unwrap();
    let equivalence = checks
        .iter()
        .find(|check| check["item"] == "llvm_ir_object_equivalence")
        .unwrap();
    assert_eq!(equivalence["result"], "checked");
    let detail = equivalence["detail"].as_str().unwrap();
    for function in ["add", "lt", "callee"] {
        assert!(
            detail.contains(&format!("{function}: equivalent")),
            "{detail}"
        );
    }
    assert!(detail.contains("caller: unsupported_signature"), "{detail}");
    // A v24 record re-pinned as v23 is refused — v23 keeps the wide
    // leaf out of the composite shape.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V23.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn module_bundle_v24_rejects_a_tampered_leaf_widths_pin() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, _id) = build_source(
        root.path(),
        "atom add(x: i128, y: i128) -> i128 requires: true; ensures: result == x + y; body: { x + y };\n\
         atom main() -> i64 requires: true; ensures: true; body: { 0 };",
    );
    // Dropping the wide marker declares a single 64-bit leaf — the
    // re-derived pairing no longer matches.
    let mut record = module_record(&bundle);
    record["leaf_widths"]["add"] = serde_json::json!([64, 64]);
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
    // Rebuild so the next assertion starts from a valid record.
    let mut record = module_record(&bundle);
    record["leaf_widths"]["add"] = serde_json::json!([128, 128]);
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert_success(&check(&source, &bundle, &native_bundle::sha256(&bytes)));
}

#[test]
fn module_bundle_with_i128_variable_shifts_rechecks() {
    // A variable `i128`/`u128` shift lowers to `shld`/`shrd` +
    // `test $0x40,%cl`/`cmov` on the generic target — the machine-TV
    // lifter models the whole sequence. The `requires` bound satisfies
    // the source verifier's 0..64 shift check; the compared domain is
    // the wide definedness range 0..128, so the `count >= 64` cmov
    // fallback is exercised too.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "atom shl(x: i128, n: i128) -> i128 requires: n >= 0 && n < 64; ensures: true; body: { x << n };\n\
         atom lshr(x: u128, n: u128) -> u128 requires: n >= 0 && n < 64; ensures: true; body: { x >> n };\n\
         atom ashr(x: i128, n: i128) -> i128 requires: n >= 0 && n < 64; ensures: true; body: { x >> n };\n\
         atom main() -> i64 requires: true; ensures: true; body: { 0 };",
    );
    assert_success(&check(&source, &bundle, &id));
    let machine_tv = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&machine_tv);
    let machine_tv: serde_json::Value = serde_json::from_slice(&machine_tv.stdout).unwrap();
    for function in ["shl", "lshr", "ashr"] {
        assert_eq!(
            machine_tv["functions"][function]["outcome"], "equivalent_within_profile",
            "{function}"
        );
    }
    let accepted = accept_object(&source, &bundle, &id, true);
    assert_success(&accepted);
    let accepted: serde_json::Value = serde_json::from_slice(&accepted.stdout).unwrap();
    assert_eq!(accepted["accepted"], true);
}

#[test]
fn module_bundle_with_wide_enum_payload_builds_as_v25_and_rechecks() {
    // A uniform `i128`/`u128` payload position occupies two 64-bit
    // slots (`pure-i64-composite/v22`); the record pins bundle v25 and
    // the declared `leaf_widths` (tag excluded, one 128 per wide leaf).
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "enum F { A(i128), B(i128) };\n\
         atom pick(f: F) -> i128 requires: true; ensures: true; body: { match f { F::A(x) => x, _ => 0 } };\n\
         atom main() -> i64 requires: true; ensures: true; body: { 0 };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V25);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V22
    );
    assert_eq!(record["leaf_widths"]["pick"], serde_json::json!([64, 128]));
    assert_eq!(record["leaf_widths"]["main"], serde_json::json!([]));
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    // A v25 record re-pinned as v24 is refused — v24 keeps the wide
    // leaf out of the enum payload shape.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V24.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn module_bundle_with_wide_enum_literals_and_merges_rechecks() {
    // `Num` literal arms adapt to wide `if`/`match` merge results —
    // `pure-i64-composite/v22`. (Constructing or writing a
    // wide-payload enum in an atom body stays outside the admitted
    // boundary — source verification keeps the Int-tag shape for
    // non-scalar payloads.)
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "enum F { A(x: i128), B(y: i128) };\n\
         enum G { C(u128), D(u128) };\n\
         atom if_merge(b: bool, w: i128) -> i128 requires: w >= 0; ensures: result >= 0; body: { if b { 5 } else { w } };\n\
         atom match_lit(f: F) -> i128 requires: true; ensures: result >= 3; body: { match f { F::A(_) => 4, _ => 3 } };\n\
         atom match_wide(f: F) -> i128 requires: true; ensures: true; body: { match f { F::A(x) => x, _ => 3 } };\n\
         atom umerge(g: G) -> u128 requires: true; ensures: true; body: { match g { G::C(x) => x, _ => 7 } };\n\
         atom if_umerge(b: bool, w: u128) -> u128 requires: true; ensures: result == w || result == 0 - 1; body: { if b { w } else { 0 - 1 } };\n\
         atom main() -> i64 requires: true; ensures: true; body: { 0 };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V25);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V22
    );
    assert_eq!(
        record["leaf_widths"]["match_lit"],
        serde_json::json!([64, 128])
    );
    assert_eq!(
        record["leaf_widths"]["match_wide"],
        serde_json::json!([64, 128])
    );
    assert_eq!(
        record["leaf_widths"]["umerge"],
        serde_json::json!([64, 128])
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
}

#[test]
fn module_bundle_with_wide_enum_return_and_struct_field_rechecks() {
    // An enum result encodes leaves `[tag, lo, hi]` so `result == f`
    // holds, and a struct carrying a wide-payload enum flattens
    // per-64-bit-slot through a `-> S` return —
    // `pure-i64-composite/v22`.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "enum F { A(i128), B(i128) };\n\
         struct S { a: i64, f: F }\n\
         atom wrap(a: i64, f: F) -> S requires: true; ensures: result.a == a; body: { S { a: a, f: f } };\n\
         atom id(f: F) -> F requires: true; ensures: result == f; body: { f };\n\
         atom main() -> i64 requires: true; ensures: true; body: { 0 };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V25);
    assert_eq!(
        record["leaf_widths"]["wrap"],
        serde_json::json!([64, 64, 128])
    );
    assert_eq!(record["leaf_widths"]["id"], serde_json::json!([64, 128]));
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
}

#[test]
fn module_bundle_v25_pinned_by_declaration_without_wide_leaves_rechecks() {
    // A declared `i128` payload pins bundle v25 on the declaration
    // alone, so a module whose atoms never carry a wide leaf still
    // records `leaf_widths` with no 128s — a rechecker must not
    // demand a wide leaf that the declarations never used.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "enum F { A(i128), B(i128) };\n\
         atom main() -> i64 requires: true; ensures: true; body: { 0 };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V25);
    assert_eq!(record["leaf_widths"]["main"], serde_json::json!([]));
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
}

#[test]
fn module_bundle_with_mixed_width_struct_builds_and_rechecks() {
    // A struct mixing a 64-bit leaf with a 128-bit leaf flattens
    // leaf-first — `{a: i64, b: i128}` occupies leaves [a, b.lo,
    // b.hi], so `leaf_widths` reads `[64, 128]` and an `insertvalue`
    // writing `b` spans two ABI slots.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "struct S { a: i64, b: i128 }\n\
         atom mk(a: i64, b: i128) -> S requires: true; ensures: result.a == a; body: { S { a: a, b: b } };\n\
         atom rd(s: S) -> i64 requires: true; ensures: result == s.a; body: { s.a };\n\
         atom lit(b: i64) -> S requires: true; ensures: true; body: { S { a: 1, b: 2 } };\n\
         atom main() -> i64 requires: true; ensures: true; body: { 0 };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V24);
    assert_eq!(record["leaf_widths"]["mk"], serde_json::json!([64, 128]));
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
}

#[test]
fn multifile_bundle_with_i128_leaves_builds_as_v24() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        root.path().join("text.mm"),
        "atom double(x: i128) -> i128 requires: true; ensures: result == x + x; body: { x + x };\n",
    )
    .unwrap();
    fs::write(
        &source,
        "import \"text\" as t;\n\natom main() -> i64 requires: true; ensures: true; body: { 0 };\n",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MULTIFILE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MULTIFILE_PROFILE_V24);
    assert_eq!(record["leaf_widths"]["double"], serde_json::json!([128]));
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // Under a v23 pin the member's `i128` leaf stays refused.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MULTIFILE_PROFILE_V23.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn multifile_bundle_with_wide_enum_payload_builds_as_v25() {
    // Multifile admission rides the same rung as module builds:
    // `CompositeV22`, pinning `mumei.native-pure-i64-multifile/v25`.
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        root.path().join("flags.mm"),
        "enum F { A(i128), B(i128) };\n\
         atom pick(f: F) -> i128 requires: true; ensures: true; body: { match f { F::A(x) => x, _ => 0 } };\n",
    )
    .unwrap();
    fs::write(
        &source,
        "import \"flags\" as t;\n\natom main() -> i64 requires: true; ensures: true; body: { 0 };\n",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MULTIFILE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MULTIFILE_PROFILE_V25);
    assert_eq!(record["leaf_widths"]["pick"], serde_json::json!([64, 128]));
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
}

#[test]
fn multifile_bundle_with_f64_arithmetic_builds_as_v22() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        root.path().join("math.mm"),
        "atom add(x: f64, y: f64) -> f64 requires: true; ensures: true; body: { x + y };\n",
    )
    .unwrap();
    fs::write(
        &source,
        "import \"math\" as m;\n\natom main() -> i64 requires: true; ensures: true; body: { 0 };\n",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MULTIFILE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MULTIFILE_PROFILE_V22);
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // Under a v21 pin the member's `f64` arithmetic stays refused.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MULTIFILE_PROFILE_V21.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn multifile_bundle_with_nested_field_writes_builds_as_v18() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        root.path().join("shapes.mm"),
        "struct I { a: i64, b: i64 }\nstruct O { i: I, c: i64 }\n\
         atom bump(o: O, w: i64) -> O requires: true; ensures: result.i.a == w; body: { o.i.a = w; o };\n",
    )
    .unwrap();
    fs::write(
        &source,
        "import \"shapes\" as s;\n\natom main() -> i64 requires: true; ensures: true; body: { 0 };\n",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MULTIFILE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MULTIFILE_PROFILE_V18);
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // Under a v16 pin the member's nested write stays refused.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MULTIFILE_PROFILE_V16.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn module_bundle_with_typedef_and_single_main_takes_the_module_path() {
    // A module holding one `main` plus a refined scalar declaration is
    // still a module — dispatch on atom count alone would send it to
    // the single-atom path, which rejects `type` items outright.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "type Port = i64 where v >= 1 && v <= 65535;\n\
         atom main() -> i64 requires: true; ensures: result == 0; body: { 0 };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MODULE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V2);
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
}

#[test]
fn multifile_bundle_with_refined_member_builds_as_v2() {
    // A member carrying a `type` declaration merges only under the
    // refined admission — the member gate must not reject it before the
    // merged module's profile check ever runs.
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        root.path().join("helpers.mm"),
        "type Port = i64 where v >= 1 && v <= 65535;\n\
         atom listen(x: Port) -> Port requires: true; ensures: result == x; body: { x };\n",
    )
    .unwrap();
    fs::write(
        &source,
        "import \"helpers\" as h;\n\natom main() -> i64 requires: true; ensures: result == 80; body: { h::listen(80) };\n",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MULTIFILE_PROFILE_V2);
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // The consumer evidence identity record names the pinned v2
    // multifile profile.
    let json = consume("recheck-llvm --json", &source, &bundle, &id);
    assert_success(&json);
    let envelope: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(
        envelope["evidence"][0]["semantics"]["profile"],
        native_bundle::MULTIFILE_PROFILE_V2
    );
    // Under a v1 pin the same members stay refused at the member gate.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MULTIFILE_PROFILE.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn module_bundle_with_match_atom_builds_and_rechecks() {
    // Certificate generation annotates match bodies with a
    // manual-lemma reason in the translator IR; the binding pins the
    // extraction gate's IR identity, which never carries that
    // annotation. Comparing them unnormalized made every match module
    // fail `certificate binding mismatch` at build.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), "atom pick(x: i64) -> i64 requires: true; ensures: result >= 0; body: { match x { 0 => 0, _ => 1 } };\natom main() -> i64 requires: true; ensures: result >= 0; body: { pick(1) };");
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
}

#[test]
fn module_source_recheck_refutes_wrapped_i64_arithmetic() {
    // `verify` works over mathematical integers, but the source recheck
    // encodes the widened wrapping i64 profile: `id(i64::MAX) + 1`
    // wraps below zero, so the postcondition obligation is refutable
    // even though the call domain is trivially closed.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), WRAPPED);
    assert_success(&check(&source, &bundle, &id));
    let output = recheck(&source, &bundle, &id);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("refuted"));
}

#[test]
fn module_recheck_llvm_json_emits_per_atom_consumer_evidence() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), MODULE);
    let output = recheck_llvm_json(&source, &bundle, &id);
    assert_success(&output);
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["schema_version"],
        "mumei.native-module-consumer-evidence/v1"
    );
    assert_eq!(report["build_id"], id);
    assert_eq!(report["acceptance"], "not_evaluated");
    let evidence = report["evidence"].as_array().unwrap();
    // One identity record plus a postcondition and a translation record
    // per atom (`helper` and `main` here).
    assert_eq!(evidence.len(), 1 + 2 * 2);
    assert_eq!(
        evidence[0]["obligation_id"],
        "native_bundle_identity_consistency"
    );
    let obligations: Vec<&str> = evidence
        .iter()
        .map(|record| record["obligation_id"].as_str().unwrap())
        .collect();
    for atom in ["helper", "main"] {
        assert!(obligations.contains(&format!("{atom}_source_postcondition").as_str()));
        assert!(obligations.contains(&format!("{atom}_source_to_llvm_user_body").as_str()));
    }
    // The identity record names the module bundle profile and every
    // per-atom postcondition record names the module recheck profile —
    // never the single-atom family strings.
    assert_eq!(
        evidence[0]["semantics"]["profile"],
        native_bundle::MODULE_PROFILE
    );
    for record in evidence.iter().skip(1) {
        let obligation = record["obligation_id"].as_str().unwrap();
        if obligation.ends_with("_source_postcondition") {
            assert_eq!(
                record["semantics"]["profile"],
                native_bundle::MODULE_RECHECK_PROFILE
            );
        }
        assert_eq!(record["scope"]["kind"], "unbounded");
    }
}

#[test]
fn module_saved_evidence_roundtrips_and_rejects_tampering() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), MODULE);
    let saved = save_evidence(root.path(), &source, &bundle, &id);
    assert_success(
        &replay_command(&source, &bundle, &id, &saved)
            .output()
            .unwrap(),
    );
    // A tampered atom specification must never replay.
    let mut tampered: serde_json::Value =
        serde_json::from_slice(&fs::read(&saved).unwrap()).unwrap();
    let obligation = tampered["evidence"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|record| record["obligation_id"] == "helper_source_postcondition")
        .unwrap();
    obligation["specification"]["sha256"] = serde_json::Value::String("0".repeat(64));
    fs::write(&saved, serde_json::to_vec(&tampered).unwrap()).unwrap();
    let output = replay_rejected(&source, &bundle, &id, &saved);
    assert!(String::from_utf8_lossy(&output.stderr).contains("differs"));
}

#[test]
fn saved_evidence_family_mismatch_and_unknown_schemas_are_rejected() {
    let single_root = tempfile::tempdir().unwrap();
    let (single_source, single_bundle, single_id) = build(single_root.path());
    let module_root = tempfile::tempdir().unwrap();
    let (module_source, module_bundle, module_id) = build_source(module_root.path(), MODULE);
    let single_saved = save_evidence(
        single_root.path(),
        &single_source,
        &single_bundle,
        &single_id,
    );
    let module_saved = save_evidence(
        module_root.path(),
        &module_source,
        &module_bundle,
        &module_id,
    );
    // Single-atom evidence must not stand in for a module bundle, nor the
    // reverse — the families are dispatched on the saved file's schema.
    for (source, bundle, id, saved) in [
        (&module_source, &module_bundle, &module_id, &single_saved),
        (&single_source, &single_bundle, &single_id, &module_saved),
    ] {
        let output = replay_rejected(source, bundle, id, saved);
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("family"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    // An unrecognized schema_version is rejected before any comparison.
    let mut unknown: serde_json::Value =
        serde_json::from_slice(&fs::read(&module_saved).unwrap()).unwrap();
    unknown["schema_version"] = serde_json::Value::String("mumei.bogus/v9".into());
    fs::write(&module_saved, serde_json::to_vec(&unknown).unwrap()).unwrap();
    replay_rejected(&module_source, &module_bundle, &module_id, &module_saved);
}

#[test]
fn module_build_rejects_sources_without_a_single_trivially_gated_main() {
    for text in [
        // No zero-argument `main` atom among the siblings.
        "atom helper(x: i64) -> i64 requires: x > 0; ensures: result == x + 1; body: { x + 1 };\natom other(x: i64) -> i64 requires: true; ensures: result == x; body: { x };",
        // Two mains leave the entry point ambiguous.
        "atom main() -> i64 requires: true; ensures: result == 0; body: { 0 };\natom main() -> i64 requires: true; ensures: result == 0; body: { 0 };",
        // A gated main keeps the single-atom contract convention.
        "atom main() -> i64 requires: 1 > 0; ensures: result == 0; body: { 0 };\natom helper(x: i64) -> i64 requires: true; ensures: result == x; body: { x };",
        // The entry point keeps the `-> i64` signature the C wrapper
        // attests even though siblings may return other scalars.
        "atom main() -> bool requires: true; ensures: result; body: { 1 > 0 };\natom helper(x: i64) -> i64 requires: true; ensures: result == x; body: { x };",
    ] {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.mm");
        fs::write(&source, text).unwrap();
        let output = cli(&[
            "native-bundle",
            "build",
            source.to_str().unwrap(),
            "-o",
            root.path().join("bundle").to_str().unwrap(),
        ]);
        assert!(!output.status.success());
    }
}

fn recheck_artifact(source: &Path, bundle: &Path, id: &str) -> Output {
    consume("recheck-artifact", source, bundle, id)
}

fn repin_module(bundle: &Path) -> String {
    let mut record: native_bundle::ModuleNativeBundleRecord =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    for role in BundleFile::ALL {
        record.files.insert(
            role,
            native_bundle::ContentIdentity::of(&fs::read(bundle.join(role.file_name())).unwrap()),
        );
    }
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    native_bundle::sha256(&bytes)
}

#[test]
fn artifact_recheck_reproduces_pinned_object_and_executable() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let record: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    assert!(record["link_build"].get("runtime_source_sha256").is_none());
    let output = recheck_artifact(&source, &bundle, &id);
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("toolchain_identity_only"));
    assert!(stdout.contains("no_execution_or_runtime_semantics"));

    let mut runtime_mismatch = record;
    runtime_mismatch["settings"]["runtime"] = serde_json::Value::String(
        native_bundle::RUNTIME_SYSTEM_CRT_WITH_EMBEDDED_MUMEI_C.to_owned(),
    );
    let bytes = serde_json::to_vec(&runtime_mismatch).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    let mismatched_id = native_bundle::sha256(&bytes);
    let rejected = check(&source, &bundle, &mismatched_id);
    assert_eq!(rejected.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("unsupported schema, scope or settings")
    );
}

#[test]
fn loop_bundle_links_and_pins_runtime_but_keeps_runtime_out_of_claims() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), WHILE_PROBE);
    let record: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    let runtime_hash = native_bundle::sha256(include_str!("../runtime/mumei_runtime.c").as_bytes());
    assert_eq!(record["link_build"]["runtime_source_sha256"], runtime_hash);
    assert_eq!(
        record["settings"]["runtime"],
        native_bundle::RUNTIME_SYSTEM_CRT_WITH_EMBEDDED_MUMEI_C
    );
    assert!(!BundleFile::ALL
        .iter()
        .any(|file| file.file_name() == "mumei_runtime.c"));
    assert_success(&recheck_artifact(&source, &bundle, &id));

    // The loop bundle links the embedded runtime: its `__mumei_*`
    // helper and loader-import relocations resolve through the
    // assumed-external path, so the link correspondence is checkable
    // end to end instead of inconclusive.
    let object = recheck_object(&source, &bundle, &id, true);
    assert_success(&object);
    assert!(String::from_utf8_lossy(&object.stdout).contains("object_link_correspondence"));

    let machine_tv = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&machine_tv);
    let descriptor: serde_json::Value = serde_json::from_slice(&machine_tv.stdout).unwrap();
    assert_eq!(
        descriptor["functions"]["loop_count"],
        serde_json::json!({
            "outcome": "equivalent_within_profile",
            "callee_saved_preservation": "preserved"
        })
    );
    assert_eq!(
        descriptor["input_domain"],
        "signed_i64_inputs_with_signature_bounded_param_domains_where_division_and_shift_defined_not_restricted_by_requires_and_loops_exiting_within_bound_4"
    );
    let performed = descriptor["checks_performed"].as_array().unwrap();
    assert!(performed
        .iter()
        .any(|check| check == "machine_code_bounded_loop_function_tv"));
    assert!(performed
        .iter()
        .any(|check| check == "machine_code_callee_saved_register_preservation"));
    let not_performed = descriptor["checks_not_performed"].as_array().unwrap();
    assert!(not_performed
        .iter()
        .any(|check| check == "machine_code_tv_loop_executions_beyond_bound"));
    assert!(not_performed
        .iter()
        .any(|check| check == "runtime_cancellation_semantics"));
    assert!(!not_performed
        .iter()
        .any(|check| check == "object_link_correspondence"));
    assert!(!not_performed
        .iter()
        .any(|check| check == "machine_code_tv_cyclic_control_flow_functions"));

    // The `assumption` field is emitted on every relocation — the
    // `assumed_external_callee` marker belongs to loader imports only
    // (covered for real imports in the lib memcpy test); internal
    // targets carry null.
    for relocation in descriptor["object_translation"]["relocations"]
        .as_array()
        .unwrap()
    {
        // `.get()` distinguishes "emitted as null" from "key dropped" —
        // index access returns Null for a missing key and would hide
        // the field being removed from the JSON.
        let assumption = relocation
            .get("assumption")
            .expect("relocation is missing the assumption key");
        assert!(
            assumption.is_null() || *assumption == "assumed_external_callee",
            "unexpected assumption marker: {relocation}"
        );
    }

    let original_record = fs::read(bundle.join(RECORD_FILE)).unwrap();
    let mut runtime_setting_mismatch = record.clone();
    runtime_setting_mismatch["settings"]["runtime"] =
        serde_json::Value::String(native_bundle::RUNTIME_SYSTEM_CRT_ONLY.to_owned());
    let bytes = serde_json::to_vec(&runtime_setting_mismatch).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    let mismatched_id = native_bundle::sha256(&bytes);
    let rejected = check(&source, &bundle, &mismatched_id);
    assert_eq!(rejected.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("unsupported schema, scope or settings")
    );
    fs::write(bundle.join(RECORD_FILE), original_record).unwrap();

    let mut tampered = record;
    tampered["link_build"]["runtime_source_sha256"] = serde_json::Value::String("0".repeat(64));
    let bytes = serde_json::to_vec(&tampered).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    let tampered_id = native_bundle::sha256(&bytes);
    let mismatch = recheck_artifact(&source, &bundle, &tampered_id);
    assert_eq!(mismatch.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&mismatch.stderr).contains("runtime source differs"));
}

#[test]
fn module_artifact_recheck_reproduces_pinned_object_and_executable() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), MODULE);
    let output = recheck_artifact(&source, &bundle, &id);
    assert_success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("toolchain_identity_only"));
}

#[test]
fn artifact_recheck_rejects_a_repinned_foreign_object() {
    // A bundle repinned around a *valid but foreign* object — one this
    // toolchain did not emit from the pinned `program.ll` — fails the
    // object stage, not just the identity stage.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, _) = build(root.path());
    let other_root = tempfile::tempdir().unwrap();
    let other_source = other_root.path().join("other.mm");
    fs::write(
        &other_source,
        "atom main() -> i64 requires: true; ensures: result == 9; body: { 9 };",
    )
    .unwrap();
    let other_bundle = other_root.path().join("bundle");
    assert_success(&cli(&[
        "native-bundle",
        "build",
        other_source.to_str().unwrap(),
        "-o",
        other_bundle.to_str().unwrap(),
    ]));
    fs::copy(
        other_bundle.join(BundleFile::Object.file_name()),
        bundle.join(BundleFile::Object.file_name()),
    )
    .unwrap();
    let id = repin(&bundle, None);
    let output = recheck_artifact(&source, &bundle, &id);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("differs"));
}

#[test]
fn artifact_recheck_rejects_a_repinned_foreign_executable() {
    // The pinned object is genuine, so the object stage passes; the
    // relinked executable must still equal the pinned `program`.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, _) = build(root.path());
    fs::write(
        bundle.join(BundleFile::Executable.file_name()),
        b"repinned non-executable bytes",
    )
    .unwrap();
    let id = repin(&bundle, None);
    let output = recheck_artifact(&source, &bundle, &id);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("differs"));
}

#[test]
fn module_artifact_recheck_rejects_a_repinned_foreign_executable() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, _) = build_source(root.path(), MODULE);
    fs::write(
        bundle.join(BundleFile::Executable.file_name()),
        b"repinned non-executable bytes",
    )
    .unwrap();
    let id = repin_module(&bundle);
    let output = recheck_artifact(&source, &bundle, &id);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("differs"));
}

#[test]
fn accept_artifact_emits_descriptor_only_when_artifacts_reproduce() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    for action in ["accept-artifact", "accept-artifact --json"] {
        let output = consume(action, &source, &bundle, &id);
        assert_success(&output);
        if action.ends_with("--json") {
            let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                descriptor["schema_version"],
                "mumei.native-evidence-acceptance/v1"
            );
            assert_eq!(descriptor["acceptance"], "evaluated");
            assert_eq!(descriptor["accepted"], true);
            assert_eq!(
                descriptor["scope"],
                "native_bundle_source_llvm_and_toolchain_artifact_reproduction"
            );
            assert!(descriptor["checks_performed"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry == "toolchain_object_executable_reproduction"));
            assert!(descriptor["checks_not_performed"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry == "os_exit_status_observation"));
            assert!(!descriptor["checks_not_performed"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry == "os_exit_status"));
            // Artifact acceptance evaluates no record, so the binding
            // fingerprint key stays absent rather than emitted as null.
            assert!(descriptor.get("evidence_fingerprint").is_none());
            assert!(mumei_core::verification_evidence::decode(&output.stdout).is_err());
        } else {
            assert!(String::from_utf8_lossy(&output.stdout).contains("accepted"));
        }
    }
    // A repinned foreign executable is rejected with no descriptor.
    fs::write(
        bundle.join(BundleFile::Executable.file_name()),
        b"repinned non-executable bytes",
    )
    .unwrap();
    let bad_id = repin(&bundle, None);
    for action in ["accept-artifact", "accept-artifact --json"] {
        let output = consume(action, &source, &bundle, &bad_id);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn accept_artifact_accepts_module_bundles() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), MODULE);
    let output = consume("accept-artifact --json", &source, &bundle, &id);
    assert_success(&output);
    let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        descriptor["schema_version"],
        "mumei.native-evidence-acceptance/v1"
    );
    assert_eq!(descriptor["accepted"], true);
}

#[test]
fn accept_object_emits_bounded_descriptor_and_text_for_module_bundle() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), MODULE);
    let output = accept_object(&source, &bundle, &id, true);
    assert_success(&output);
    let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        descriptor["schema_version"],
        "mumei.native-evidence-acceptance/v1"
    );
    assert_eq!(descriptor["acceptance"], "evaluated");
    assert_eq!(descriptor["accepted"], true);
    assert!(descriptor.get("invocation").is_none());
    assert!(descriptor.get("host").is_none());
    assert!(descriptor.get("pin_envelope").is_none());
    assert_eq!(
        descriptor["scope"],
        "native_bundle_object_translation_correspondence"
    );
    assert_eq!(descriptor["build_id"], id);
    assert_eq!(
        descriptor["llvm_sha256"],
        native_bundle::sha256(&fs::read(bundle.join(BundleFile::LlvmIr.file_name())).unwrap())
    );
    assert_eq!(
        descriptor["object_sha256"],
        native_bundle::sha256(&fs::read(bundle.join(BundleFile::Object.file_name())).unwrap())
    );
    let record = module_record(&bundle);
    let canonical_pins = format!(
        r#"{{"build_id":"{id}","canonical_argv_hash":{},"emit_scope":{},"llvm_sha256":{},"object_sha256":{},"source_sha256":"{}"}}"#,
        record["link_build"]["canonical_arguments_sha256"],
        descriptor["scope"],
        descriptor["llvm_sha256"],
        descriptor["object_sha256"],
        native_bundle::sha256(&fs::read(&source).unwrap()),
    );
    assert_eq!(
        descriptor["evidence_fingerprint"],
        native_bundle::sha256(canonical_pins.as_bytes())
    );
    assert_eq!(
        descriptor["register_convention_assumed"],
        "sysv_x86_64_integer_args_rdi_rsi_rdx_rcx_r8_r9_result_rax"
    );
    assert_eq!(
        descriptor["functions"],
        serde_json::json!({
            "__mumei_user_main": {
                "outcome": "equivalent_within_profile",
                "callee_saved_preservation": "preserved"
            },
            "helper": {
                "outcome": "equivalent_within_profile",
                "callee_saved_preservation": "preserved"
            },
            "main": {"outcome": "equivalent_within_profile", "callee_saved_preservation": "preserved", "role": "entry_wrapper"},
        })
    );
    let performed = descriptor["checks_performed"].as_array().unwrap();
    assert!(performed
        .iter()
        .any(|check| check == "object_source_machine_code_correspondence"));
    assert!(performed
        .iter()
        .any(|check| check == "object_link_correspondence"));
    // Step 2 (DESIGN_OBJECT_ACCEPTANCE.md §6): the checks lists name
    // the correspondence items that gated the acceptance.
    for item in [
        "function_coverage",
        "llvm_ir_object_equivalence",
        "hir_llvm_model_correspondence",
        "object_data_correspondence",
        "object_symbol_attribute_correspondence",
    ] {
        assert!(performed.iter().any(|check| check == item), "{item}");
    }
    assert!(performed
        .iter()
        .any(|check| check == "machine_code_callee_saved_register_preservation"));
    assert!(!performed
        .iter()
        .any(|check| check == "pinned_member_merge_rederivation"));
    let not_performed = descriptor["checks_not_performed"].as_array().unwrap();
    assert!(not_performed
        .iter()
        .any(|check| check == "descriptor_authenticity"));
    assert!(!not_performed
        .iter()
        .any(|check| check == "object_entry_wrapper_semantics"));
    // The agent-owned `object_translation_correspondence` item name is
    // never emitted as a check name — the Core descriptor only claims its
    // own items (the new scope value legitimately contains the substring).
    for check in performed.iter().chain(not_performed.iter()) {
        assert_ne!(check, "object_translation_correspondence");
    }

    let text = accept_object(&source, &bundle, &id, false);
    assert_success(&text);
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(text.contains("Native object accepted within the machine-code TV profile:"));
    assert!(text.contains("2 source-atom functions equivalent"));
    assert!(text.contains("Entry wrapper is equivalent within the machine-code TV profile."));
    // Step 5: the acceptance text report also surfaces the aggregate
    // correspondence status.
    assert!(text.contains("Object translation correspondence: checked_correspondence"));
    // Design memo §7 Step 2: the within-profile gate is named too.
    assert!(
        text.contains("Source machine-TV within profile: checked_source_machine_tv_within_profile")
    );
    assert_eq!(text.lines().count(), 4);

    let wrong_id = "0".repeat(64);
    assert_ne!(wrong_id, id);
    let rejected = accept_object(&source, &bundle, &wrong_id, true);
    assert_eq!(rejected.status.code(), Some(1));
    assert!(rejected.stdout.is_empty());
}

#[test]
fn accept_object_signs_exact_descriptor_bytes_after_pinned_checks() {
    const FAKE_KEY: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const INVALID_KEY: &str = "FAKE_PRIVATE_KEY_SENTINEL";
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), MODULE);
    let key = root.path().join("key.hex");
    let signature_path = root.path().join("descriptor.sig.json");
    fs::write(&key, FAKE_KEY).unwrap();
    let signed_command = |pin: &str, format_args: &[&str]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
        command
            .args(["native-bundle", "accept-object"])
            .arg(&bundle)
            .arg(&source)
            .args(["--expected-build-id", pin, "--key-file"])
            .arg(&key)
            .args(["--producer-key-id", "test-producer", "--signature-output"])
            .arg(&signature_path)
            .args(format_args);
        command
    };
    let signed_acceptance =
        |pin: &str, format_args: &[&str]| signed_command(pin, format_args).output().unwrap();
    let unsigned = accept_object(&source, &bundle, &id, true);
    assert_success(&unsigned);
    let signed = signed_acceptance(&id, &["--json"]);
    assert_success(&signed);
    assert_eq!(signed.stdout, unsigned.stdout);
    assert!(signed.stdout.ends_with(b"\n"));
    assert!(signed.stderr.is_empty());
    let envelope_bytes = fs::read(&signature_path).unwrap();
    for bytes in [&signed.stdout, &signed.stderr, &envelope_bytes] {
        assert!(!String::from_utf8_lossy(bytes).contains(FAKE_KEY));
    }
    let envelope: serde_json::Value = serde_json::from_slice(&envelope_bytes).unwrap();
    assert!(producer_signature::envelope_match(&envelope));
    assert_eq!(envelope["producer_key_id"], "test-producer");
    let signature = envelope["signature"].as_str().unwrap();
    let public_key = producer_signature::decode_secret_key_bytes(FAKE_KEY.as_bytes().to_vec())
        .unwrap()
        .verifying_key()
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let digest = producer_signature::subject_digest(&signed.stdout);
    assert_eq!(envelope["subject_digest"], digest);
    assert!(producer_signature::signature_valid(
        &public_key,
        signature,
        digest.as_bytes()
    ));
    let mut changed: serde_json::Value = serde_json::from_slice(&signed.stdout).unwrap();
    for check in [
        "producer_authenticity",
        "descriptor_authenticity",
        "historical_execution",
    ] {
        assert!(changed["checks_not_performed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item == check));
    }
    changed["accepted"] = serde_json::json!(false);
    let mut changed = serde_json::to_vec_pretty(&changed).unwrap();
    changed.push(b'\n');
    for bytes in [&changed[..], &signed.stdout[..signed.stdout.len() - 1]] {
        assert!(!producer_signature::signature_valid(
            &public_key,
            signature,
            producer_signature::subject_digest(bytes).as_bytes()
        ));
    }

    let assert_rejected = |output: Output, exit| {
        assert_eq!(output.status.code(), Some(exit));
        assert!(output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(!error.contains(FAKE_KEY));
        assert!(!error.contains(INVALID_KEY));
    };
    fs::write(&signature_path, b"existing signature").unwrap();
    let no_overwrite = signed_acceptance(&id, &["--json"]);
    assert!(String::from_utf8_lossy(&no_overwrite.stderr)
        .contains("cannot publish native object descriptor signature"));
    assert_rejected(no_overwrite, 1);
    assert_eq!(fs::read(&signature_path).unwrap(), b"existing signature");
    fs::remove_file(&signature_path).unwrap();
    let entries = || {
        fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<BTreeSet<_>>()
    };
    let before = entries();
    let full = fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let failed_write = signed_command(&id, &["--json"])
        .stdout(Stdio::from(full))
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&failed_write.stderr)
        .contains("cannot write native object acceptance descriptor"));
    assert_rejected(failed_write, 1);
    assert!(!signature_path.exists());
    assert_eq!(entries(), before);
    assert_success(&signed_acceptance(&id, &["--json"]));
    assert_eq!(fs::read(&signature_path).unwrap(), envelope_bytes);
    fs::remove_file(&signature_path).unwrap();
    assert_rejected(signed_acceptance(&id, &[]), 2);
    assert!(!signature_path.exists());
    fs::write(&key, INVALID_KEY).unwrap();
    assert_rejected(signed_acceptance(&id, &["--json"]), 1);
    assert!(!signature_path.exists());
    fs::write(&key, FAKE_KEY).unwrap();
    let object_path = bundle.join(BundleFile::Object.file_name());
    let object_bytes = fs::read(&object_path).unwrap();
    fs::remove_file(&object_path).unwrap();
    assert_rejected(signed_acceptance(&id, &["--json"]), 1);
    assert!(!signature_path.exists());
    fs::write(&object_path, b"replaced object").unwrap();
    assert_rejected(signed_acceptance(&id, &["--json"]), 1);
    assert!(!signature_path.exists());
    fs::write(&object_path, object_bytes).unwrap();
    fs::write(&source, format!("{MODULE}\n")).unwrap();
    assert_rejected(signed_acceptance(&id, &["--json"]), 1);
    assert!(!signature_path.exists());
}

#[test]
fn accept_object_fingerprint_binds_multifile_record_source_digest() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_multifile(root.path());
    let record = module_record(&bundle);
    assert_eq!(record["schema"], "mumei.native-multifile-bundle/v1");
    let source_sha256 = record["source"]["source_sha256"].as_str().unwrap();
    assert_ne!(
        source_sha256,
        native_bundle::sha256(&fs::read(&source).unwrap())
    );

    let output = accept_object(&source, &bundle, &id, true);
    assert_success(&output);
    let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(descriptor["accepted"], true);
    let scope = "native_bundle_object_translation_correspondence";
    assert_eq!(descriptor["scope"], scope);
    let canonical_pins = format!(
        r#"{{"build_id":"{id}","canonical_argv_hash":{},"emit_scope":"{scope}","llvm_sha256":{},"object_sha256":{},"source_sha256":"{source_sha256}"}}"#,
        record["link_build"]["canonical_arguments_sha256"],
        record["files"]["llvm_ir"]["sha256"],
        record["files"]["object"]["sha256"],
    );
    assert_eq!(
        descriptor["evidence_fingerprint"],
        native_bundle::sha256(canonical_pins.as_bytes())
    );

    // Promote the real object-route acceptance descriptor into Common Evidence v1
    // and verify the projected record through `mumei evidence check`.
    let mut promote = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args([
            "evidence",
            "check-object-acceptance",
            "--stdin",
            "--emit-evidence",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    promote
        .stdin
        .take()
        .unwrap()
        .write_all(&output.stdout)
        .unwrap();
    let promoted = promote.wait_with_output().unwrap();
    assert_success(&promoted);

    let evidence: serde_json::Value = serde_json::from_slice(&promoted.stdout).unwrap();
    assert_eq!(evidence["schema_version"], "mumei.verification-evidence/v1");
    assert_eq!(evidence["subject"]["kind"], "native_artifact");
    assert_eq!(evidence["subject"]["sha256"], descriptor["object_sha256"]);
    assert_eq!(
        evidence["specification"]["sha256"],
        descriptor["llvm_sha256"]
    );
    assert_eq!(
        evidence["provenance"]["configuration_fingerprint"],
        descriptor["evidence_fingerprint"]
    );
    assert_eq!(evidence["method"], "bounded_model_checking");
    assert_eq!(evidence["scope"]["kind"], "bounded");
    assert_eq!(evidence["semantics"]["integer_model"], "wrapping_i64");

    let mut check = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["evidence", "check", "--stdin", "--json"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    check
        .stdin
        .take()
        .unwrap()
        .write_all(&promoted.stdout)
        .unwrap();
    let checked = check.wait_with_output().unwrap();
    assert_success(&checked);
    let check_json: serde_json::Value = serde_json::from_slice(&checked.stdout).unwrap();
    assert_eq!(check_json["representation"], "valid");
}

#[test]
fn accept_object_accepts_loop_bundle_with_object_link() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), WHILE_PROBE);
    let output = accept_object(&source, &bundle, &id, true);
    assert_success(&output);
    let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        descriptor["scope"],
        "native_bundle_object_translation_correspondence"
    );
    assert_eq!(
        descriptor["input_domain"],
        "signed_i64_inputs_with_signature_bounded_param_domains_where_division_and_shift_defined_not_restricted_by_requires_and_loops_exiting_within_bound_4"
    );
    assert_eq!(
        descriptor["functions"]["loop_count"]["callee_saved_preservation"],
        "preserved"
    );
    assert_eq!(
        descriptor["functions"]["main"]["outcome"],
        "equivalent_within_profile"
    );
    assert_eq!(descriptor["functions"]["main"]["role"], "entry_wrapper");
    let performed = descriptor["checks_performed"].as_array().unwrap();
    assert!(performed
        .iter()
        .any(|check| check == "object_source_machine_code_correspondence"));
    assert!(performed
        .iter()
        .any(|check| check == "machine_code_callee_saved_register_preservation"));
    assert!(performed
        .iter()
        .any(|check| check == "machine_code_entry_wrapper_tv"));
    assert!(performed
        .iter()
        .any(|check| check == "object_link_correspondence"));
    let not_performed = descriptor["checks_not_performed"].as_array().unwrap();
    assert!(!not_performed
        .iter()
        .any(|check| check == "object_link_correspondence"));
    assert!(not_performed
        .iter()
        .any(|check| check == "machine_code_tv_loop_executions_beyond_bound"));
    assert!(not_performed
        .iter()
        .any(|check| check == "runtime_cancellation_semantics"));
    // The gate covers source↔machine equivalence only; the link edge's
    // own status is named separately in the same record.
    assert_eq!(
        descriptor["source_machine_tv"]["status"],
        "checked_source_machine_tv_within_profile"
    );
    assert_eq!(
        descriptor["source_machine_tv"]["object_link_correspondence"],
        "checked_object_link_correspondence"
    );
    // The checked-link path emits the full record: functions enumerated,
    // relocations recorded, and the link edge reported as checked.
    let translation = &descriptor["object_translation"];
    let correspondence = &translation["correspondence"];
    let results: BTreeMap<String, &str> = correspondence["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|check| {
            (
                check["item"].as_str().unwrap().to_owned(),
                check["result"].as_str().unwrap(),
            )
        })
        .collect();
    let expected_status = if results.values().all(|result| *result == "checked") {
        "checked_correspondence"
    } else if results.values().any(|result| *result == "inconsistent") {
        "inconsistent_correspondence"
    } else {
        "unchecked_not_translation_validated"
    };
    // The aggregate statuses are derived from the item results, never fixed.
    assert_eq!(correspondence["status"], expected_status);
    let expected_translation_status = if expected_status == "checked_correspondence" {
        "checked_correspondence"
    } else {
        "metadata_only"
    };
    assert_eq!(translation["status"], expected_translation_status);
    let transformations = translation["transformations"].as_array().unwrap();
    assert_eq!(transformations.len(), 4);
    let link_edge = transformations
        .iter()
        .find(|edge| edge["relation"] == "static_link_relocation_placement")
        .unwrap();
    assert_eq!(
        link_edge["validation"],
        "checked_object_link_correspondence"
    );
    assert_eq!(link_edge["result"], "checked_correspondence");
    let functions = translation["functions"].as_array().unwrap();
    assert!(functions.len() >= 2);
    let names: Vec<&str> = functions
        .iter()
        .map(|function| function["ir_function"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"loop_count") && names.contains(&"main"));
    for function in functions {
        assert!(function["object_symbol"].is_string());
        assert!(function["object_section"].is_string());
        assert!(function["object_offset"].is_number());
    }
    for relocation in translation["relocations"].as_array().unwrap() {
        assert!(relocation["object_section"].is_string());
        assert!(relocation["object_offset"].is_number());
        assert!(relocation["target_symbol"].is_string());
    }
    for item in [
        "function_coverage",
        "function_count",
        "relocation_targets",
        "relocation_bounds",
    ] {
        assert_eq!(results.get(item).copied(), Some("checked"), "{item}");
    }
    // Unresolved relocation values may stay honestly unchecked on this path.
    assert!(matches!(
        results.get("relocation_resolved_values").copied(),
        Some("checked") | Some("unchecked")
    ));

    // The text path reports the same aggregate status — and one
    // diagnostic per inconsistent item — on the checked-link path too.
    let text = accept_object(&source, &bundle, &id, false);
    assert_success(&text);
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(text.contains(&format!(
        "Object translation correspondence: {expected_status}"
    )));
    let inconsistent_items = results
        .values()
        .filter(|result| **result == "inconsistent")
        .count();
    let inconsistent_lines = text
        .lines()
        .filter(|line| line.starts_with("  inconsistent: "))
        .count();
    assert_eq!(inconsistent_lines, inconsistent_items);
}

#[test]
fn accept_object_inconclusive_constant_division_emits_no_descriptor() {
    let root = tempfile::tempdir().unwrap();
    let source_text =
        "atom quotient_seven(x: i64) -> i64 requires: true; ensures: true; body: { x / 7 };\n\
atom main() -> i64 requires: true; ensures: true; body: { quotient_seven(3) };";
    let (source, bundle, id) = build_source(root.path(), source_text);
    let output = accept_object(&source, &bundle, &id, true);
    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("object not accepted"));
}

const MF_HELPERS: &str =
    "atom helper(x: i64) -> i64 requires: x > 0; ensures: result == x + 1; body: { x + 1 };\n";
const MF_ROOT: &str = "import \"helpers\" as h;\n\natom main() -> i64 requires: true; ensures: result == 6; body: { h::helper(5) };";

fn build_multifile(root: &Path) -> (std::path::PathBuf, std::path::PathBuf, String) {
    let source = root.join("app.mm");
    let bundle = root.join("bundle");
    fs::write(root.join("helpers.mm"), MF_HELPERS).unwrap();
    fs::write(&source, MF_ROOT).unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    (source, bundle, id)
}

#[test]
fn multifile_nested_closure_merges_and_cycles_fail_honestly() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        root.path().join("deep.mm"),
        "atom deep(x: i64) -> i64 requires: true; ensures: result == x + 1; body: { x + 1 };\n",
    )
    .unwrap();
    fs::write(
        root.path().join("helpers.mm"),
        "import \"deep\" as d;\n\natom helper(x: i64) -> i64 requires: x > 0; ensures: result == x + 2; body: { d::deep(x) + 1 };\n",
    )
    .unwrap();
    fs::write(
        &source,
        "import \"helpers\" as h;\n\natom main() -> i64 requires: true; ensures: result == 7; body: { h::helper(5) };",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    // The transitive member is pinned even though the root never names it.
    assert!(record["sources"]["deep.mm"]["sha256"].is_string());
    assert!(bundle.join("sources/deep.mm").is_file());
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    let json = consume("recheck-llvm --json", &source, &bundle, &id);
    assert_success(&json);
    let envelope: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(
        envelope["evidence"].as_array().unwrap().len(),
        2 + 2 * 3 // identity + merge correspondence + 3 atoms × 2
    );
    // Import cycles are rejected honestly at build time.
    fs::write(
        root.path().join("cyc1.mm"),
        "import \"cyc2\";\natom fa() -> i64 requires: true; ensures: result == 1; body: { 1 };",
    )
    .unwrap();
    fs::write(
        root.path().join("cyc2.mm"),
        "import \"cyc1\";\natom fb() -> i64 requires: true; ensures: result == 1; body: { 1 };",
    )
    .unwrap();
    let output = cli(&[
        "native-bundle",
        "build",
        root.path().join("cyc1.mm").to_str().unwrap(),
        "-o",
        root.path().join("cyc.bundle").to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
}

#[test]
fn multifile_subdirectory_members_merge_and_escape_paths_fail_honestly() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    let sub = root.path().join("sub");
    fs::create_dir(&sub).unwrap();
    // `sub/helpers` resolves file-relative: its own `import "deep"`
    // anchors at `sub/`, not the root's directory — exactly the
    // language resolver's base_dir semantics.
    fs::write(
        sub.join("deep.mm"),
        "atom deep(x: i64) -> i64 requires: true; ensures: result == x + 1; body: { x + 1 };\n",
    )
    .unwrap();
    fs::write(
        sub.join("helpers.mm"),
        "import \"deep\" as d;\n\natom helper(x: i64) -> i64 requires: x > 0; ensures: result == x + 2; body: { d::deep(x) + 1 };\n",
    )
    .unwrap();
    // Unaliased subdirectory import: the qualifier is the member's
    // last-segment stem (`deep`), not the directory prefix.
    fs::write(
        sub.join("extra.mm"),
        "atom extra(x: i64) -> i64 requires: true; ensures: result == x - 1; body: { x - 1 };\n",
    )
    .unwrap();
    fs::write(
        &source,
        "import \"sub/helpers\" as h;\nimport \"sub/extra\";\n\natom main() -> i64 requires: true; ensures: result == 12; body: { h::helper(5) + extra(6) };",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    // Nested members pin at their `/`-separated key and land under
    // `sources/` as real directories.
    assert!(record["sources"]["sub/deep.mm"]["sha256"].is_string());
    assert!(record["sources"]["sub/helpers.mm"]["sha256"].is_string());
    assert!(record["sources"]["sub/extra.mm"]["sha256"].is_string());
    assert!(bundle.join("sources/sub/deep.mm").is_file());
    assert!(bundle.join("sources/sub/extra.mm").is_file());
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    let json = consume("recheck-llvm --json", &source, &bundle, &id);
    assert_success(&json);
    let envelope: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(
        envelope["evidence"].as_array().unwrap().len(),
        2 + 2 * 4 // identity + merge correspondence + 4 atoms × 2
    );
    // The import graph pins the file-relative resolution.
    assert_eq!(
        record["source_files"]["import_graph_sha256"]
            .as_str()
            .unwrap(),
        native_bundle::import_graph_sha256(&BTreeMap::from([
            (
                "app.mm".to_string(),
                vec!["sub/extra.mm".to_string(), "sub/helpers.mm".to_string()]
            ),
            ("sub/deep.mm".to_string(), vec![]),
            ("sub/extra.mm".to_string(), vec![]),
            (
                "sub/helpers.mm".to_string(),
                vec!["sub/deep.mm".to_string()]
            ),
        ]))
    );
    // `..` cannot escape the importer's subtree — honest rejection.
    fs::write(
        root.path().join("esc.mm"),
        "import \"../outside\";\natom fa() -> i64 requires: true; ensures: result == 1; body: { 1 };",
    )
    .unwrap();
    let output = cli(&[
        "native-bundle",
        "build",
        root.path().join("esc.mm").to_str().unwrap(),
        "-o",
        root.path().join("esc.bundle").to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
}

const STD_DEP: &str =
    "atom std_helper(x: i64) -> i64 requires: true; ensures: result == x + 1; body: { x + 1 };\n";

#[test]
fn multifile_std_members_resolve_through_the_language_chain() {
    let root = tempfile::tempdir().unwrap();
    let std_dir = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    // The project carries no `std/` directory: `import "std/probe_dep"`
    // falls through importer-relative and the compiler-adjacent /
    // working-directory candidates to MUMEI_STD_PATH, exactly like the
    // language resolver. `probe_dep` is a name no shipped stdlib uses.
    fs::write(std_dir.path().join("probe_dep.mm"), STD_DEP).unwrap();
    fs::write(
        &source,
        "import \"std/probe_dep\";\n\natom main() -> i64 requires: true; ensures: result == 6; body: { probe_dep::std_helper(5) };",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args([
            "native-bundle",
            "build",
            source.to_str().unwrap(),
            "-o",
            bundle.to_str().unwrap(),
        ])
        .env("MUMEI_STD_PATH", std_dir.path())
        .output()
        .unwrap();
    assert_success(&output);
    let record = module_record(&bundle);
    // Std-supplied bytes pin under the importer-prefixed member key —
    // the consumer re-derives the merge from `sources/` alone.
    assert!(record["sources"]["std/probe_dep.mm"]["sha256"].is_string());
    assert_eq!(
        record["source_files"]["import_graph_sha256"]
            .as_str()
            .unwrap(),
        native_bundle::import_graph_sha256(&BTreeMap::from([
            ("app.mm".to_string(), vec!["std/probe_dep.mm".to_string()]),
            ("std/probe_dep.mm".to_string(), vec![]),
        ]))
    );
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));

    // Importer-relative precedence: a project `std/probe_dep.mm`
    // shadows the stdlib fallback — its bytes pin instead.
    let root2 = tempfile::tempdir().unwrap();
    let project_std = root2.path().join("std");
    fs::create_dir(&project_std).unwrap();
    fs::write(
        project_std.join("probe_dep.mm"),
        "atom std_helper(x: i64) -> i64 requires: true; ensures: result == x + 2; body: { x + 2 };\n",
    )
    .unwrap();
    let source2 = root2.path().join("app.mm");
    fs::write(
        &source2,
        "import \"std/probe_dep\";\n\natom main() -> i64 requires: true; ensures: result == 7; body: { probe_dep::std_helper(5) };",
    )
    .unwrap();
    let bundle2 = root2.path().join("bundle");
    let output2 = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args([
            "native-bundle",
            "build",
            source2.to_str().unwrap(),
            "-o",
            bundle2.to_str().unwrap(),
        ])
        .env("MUMEI_STD_PATH", std_dir.path())
        .output()
        .unwrap();
    assert_success(&output2);
    let record2 = module_record(&bundle2);
    assert_eq!(
        record2["sources"]["std/probe_dep.mm"]["sha256"]
            .as_str()
            .unwrap(),
        native_bundle::sha256(
            fs::read_to_string(project_std.join("probe_dep.mm"))
                .unwrap()
                .as_bytes()
        )
    );
}

#[test]
fn multifile_std_member_imports_resolve_beside_their_real_files() {
    let root = tempfile::tempdir().unwrap();
    let std_dir = tempfile::tempdir().unwrap();
    // A fallback member's own imports resolve beside the member's real
    // file — never beside the virtual project `std/` directory. The
    // decoy project `std/helper.mm` would shadow the real helper if
    // the producer resolved the std member's imports against the
    // bundle-key namespace instead of the resolved path's directory.
    fs::write(
        std_dir.path().join("helper.mm"),
        "atom h() -> i64 requires: true; ensures: result == 1; body: { 1 };\n",
    )
    .unwrap();
    fs::write(
        std_dir.path().join("probe_dep.mm"),
        "import \"helper\";\n\natom std_helper(x: i64) -> i64 requires: true; ensures: result == x + 1; body: { x + helper::h() };",
    )
    .unwrap();
    let project_std = root.path().join("std");
    fs::create_dir(&project_std).unwrap();
    fs::write(
        project_std.join("helper.mm"),
        "atom h() -> i64 requires: true; ensures: result == 99; body: { 99 };\n",
    )
    .unwrap();
    let source = root.path().join("app.mm");
    fs::write(
        &source,
        "import \"std/probe_dep\";\n\natom main() -> i64 requires: true; ensures: result == 6; body: { probe_dep::std_helper(5) };",
    )
    .unwrap();
    let bundle = root.path().join("bundle");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args([
            "native-bundle",
            "build",
            source.to_str().unwrap(),
            "-o",
            bundle.to_str().unwrap(),
        ])
        .env("MUMEI_STD_PATH", std_dir.path())
        .output()
        .unwrap();
    assert_success(&output);
    let record = module_record(&bundle);
    // The pinned `std/helper.mm` member carries the REAL stdlib
    // helper's bytes, not the project decoy's.
    assert_eq!(
        record["sources"]["std/helper.mm"]["sha256"]
            .as_str()
            .unwrap(),
        native_bundle::sha256(
            fs::read_to_string(std_dir.path().join("helper.mm"))
                .unwrap()
                .as_bytes()
        )
    );
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
}

#[test]
fn multifile_std_member_shared_by_two_importers_emits_its_atoms_once() {
    let root = tempfile::tempdir().unwrap();
    let std_dir = tempfile::tempdir().unwrap();
    fs::write(std_dir.path().join("probe_dep.mm"), STD_DEP).unwrap();
    // `sub/dep.mm` imports the same stdlib member the root does. Both
    // member keys pin the file (the consumer re-derives the merge from
    // `sources/` alone), but the merge emits the identical body once —
    // the language loads a resolved path once and its ModuleEnv folds
    // same-name atoms silently.
    let sub = root.path().join("sub");
    fs::create_dir(&sub).unwrap();
    fs::write(
        sub.join("dep.mm"),
        "import \"std/probe_dep\";\n\natom dep_add(x: i64) -> i64 requires: true; ensures: result == x + 1; body: { probe_dep::std_helper(x) };",
    )
    .unwrap();
    let source = root.path().join("app.mm");
    fs::write(
        &source,
        "import \"std/probe_dep\";\nimport \"sub/dep\";\n\natom main() -> i64 requires: true; ensures: result == 6; body: { dep::dep_add(5) };",
    )
    .unwrap();
    let bundle = root.path().join("bundle");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args([
            "native-bundle",
            "build",
            source.to_str().unwrap(),
            "-o",
            bundle.to_str().unwrap(),
        ])
        .env("MUMEI_STD_PATH", std_dir.path())
        .output()
        .unwrap();
    assert_success(&output);
    let record = module_record(&bundle);
    // Both member keys pin the stdlib bytes — the merge re-derivation
    // looks each up under its own importer-prefixed name.
    assert!(record["sources"]["std/probe_dep.mm"]["sha256"].is_string());
    assert!(record["sources"]["sub/std/probe_dep.mm"]["sha256"].is_string());
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
}

#[test]
fn multifile_member_key_collision_across_files_fails_honestly() {
    let root = tempfile::tempdir().unwrap();
    let std_dir = tempfile::tempdir().unwrap();
    // The fallback member's own `import "helper"` resolves beside its
    // real file (std_dir/helper.mm), while the root's
    // `import "std/helper"` resolves to the project's std/helper.mm.
    // Both land on the `std/helper.mm` member key but name DIFFERENT
    // files — the language binds them to different modules, which the
    // bundle member namespace cannot express, so the build must fail
    // honestly instead of bundling the wrong implementation.
    fs::write(
        std_dir.path().join("helper.mm"),
        "atom h() -> i64 requires: true; ensures: result == 1; body: { 1 };\n",
    )
    .unwrap();
    fs::write(
        std_dir.path().join("probe_dep.mm"),
        "import \"helper\";\n\natom std_helper(x: i64) -> i64 requires: true; ensures: result == x + 1; body: { x + helper::h() };",
    )
    .unwrap();
    let project_std = root.path().join("std");
    fs::create_dir(&project_std).unwrap();
    // The project helper returns the SAME value as the stdlib one, so
    // only the member-key collision gate — not a postcondition
    // mismatch — distinguishes this build from a valid one.
    fs::write(
        project_std.join("helper.mm"),
        "atom h() -> i64 requires: true; ensures: result == 1; body: { 1 };\n",
    )
    .unwrap();
    let source = root.path().join("app.mm");
    fs::write(
        &source,
        "import \"std/helper\";\nimport \"std/probe_dep\";\n\natom main() -> i64 requires: true; ensures: result == 7; body: { probe_dep::std_helper(5) + helper::h() };",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args([
            "native-bundle",
            "build",
            source.to_str().unwrap(),
            "-o",
            root.path().join("bundle").to_str().unwrap(),
        ])
        .env("MUMEI_STD_PATH", std_dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("collides"));
}

#[test]
fn multifile_bundle_builds_and_rechecks_with_honest_consumer_rejections() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_multifile(root.path());
    let record = module_record(&bundle);
    assert_eq!(record["schema"], "mumei.native-multifile-bundle/v1");
    assert_eq!(record["source_files"]["root"], "app.mm");
    assert!(record["sources"]["app.mm"]["sha256"].is_string());
    assert!(record["sources"]["helpers.mm"]["sha256"].is_string());
    assert!(bundle.join("sources/app.mm").is_file());
    assert!(bundle.join("sources/helpers.mm").is_file());

    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    let artifact = consume("recheck-artifact", &source, &bundle, &id);
    assert_success(&artifact);
    assert_success(&consume("recheck-exit-status", &source, &bundle, &id));

    // The multifile family emits the module-shaped consumer envelope
    // plus a merge-correspondence record.
    let json = consume("recheck-llvm --json", &source, &bundle, &id);
    assert_success(&json);
    let envelope: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(
        envelope["schema_version"],
        "mumei.native-multifile-consumer-evidence/v1"
    );
    assert!(envelope["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .any(|record| record["obligation_id"] == "multifile_merge_correspondence"));
    let evidence = root.path().join("evidence.json");
    fs::write(&evidence, &json.stdout).unwrap();
    for action in [
        "recheck-evidence",
        "accept-evidence",
        "accept-evidence --json",
    ] {
        let mut args = vec!["native-bundle"];
        args.extend(action.split_whitespace());
        args.extend([
            bundle.to_str().unwrap(),
            source.to_str().unwrap(),
            evidence.to_str().unwrap(),
            "--expected-build-id",
            &id,
        ]);
        let output = cli(&args);
        assert_success(&output);
    }
    let descriptor: serde_json::Value = serde_json::from_slice(
        &cli(&[
            "native-bundle",
            "accept-evidence",
            bundle.to_str().unwrap(),
            source.to_str().unwrap(),
            evidence.to_str().unwrap(),
            "--expected-build-id",
            &id,
            "--json",
        ])
        .stdout,
    )
    .unwrap();
    assert_eq!(
        descriptor["scope"],
        "native_multifile_bundle_identity_merge_source_llvm_and_saved_evidence_consistency"
    );
    // A single-family report can never stand in for this family.
    let single = tempfile::tempdir().unwrap();
    let (single_source, single_bundle, single_id) = build(single.path());
    let report = recheck_llvm_json(&single_source, &single_bundle, &single_id);
    assert_success(&report);
    fs::write(&evidence, &report.stdout).unwrap();
    let foreign = cli(&[
        "native-bundle",
        "accept-evidence",
        bundle.to_str().unwrap(),
        source.to_str().unwrap(),
        evidence.to_str().unwrap(),
        "--expected-build-id",
        &id,
    ]);
    assert_eq!(foreign.status.code(), Some(1));
    assert!(foreign.stdout.is_empty());
    let output = consume("accept-artifact", &source, &bundle, &id);
    assert_success(&output);

    // A pinned member that is not the root cannot pose as `--source`.
    let wrong = consume("check", &root.path().join("helpers.mm"), &bundle, &id);
    assert_eq!(wrong.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&wrong.stderr).contains("root source member"));

    // A member swapped inside the pinned sources/ directory is detected.
    fs::write(
        bundle.join("sources/helpers.mm"),
        MF_HELPERS.replace("x + 1", "x + 2"),
    )
    .unwrap();
    let tampered = consume("check", &source, &bundle, &id);
    assert_eq!(tampered.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&tampered.stderr).contains("source member"));
}

#[test]
fn module_bundle_with_enum_values_rechecks_machine_tv_and_accept_object() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "enum Opt { Some(v: i64), Nothing }\n\
         atom mk(x: i64) -> Opt requires: true; ensures: true; body: { if x > 0 { Opt::Some(x) } else { Opt::Nothing } };\n\
         atom get(o: Opt) -> i64 requires: true; ensures: true; body: { match o { Opt::Some(v) => v, _ => 0 } };\n\
         atom main() -> i64 requires: true; ensures: true; body: { get(mk(5)) };",
    );
    let record = module_record(&bundle);
    assert!(record["profile"].as_str().unwrap().contains("/v"));
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));

    let machine_tv = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&machine_tv);
    let machine_tv: serde_json::Value = serde_json::from_slice(&machine_tv.stdout).unwrap();
    for function in ["mk", "get", "main"] {
        assert_eq!(
            machine_tv["functions"][function]["outcome"],
            "equivalent_within_profile"
        );
    }
    assert_eq!(
        machine_tv["input_domain"],
        "signed_i64_inputs_with_signature_bounded_param_domains_where_division_and_shift_defined_not_restricted_by_requires"
    );
    assert!(machine_tv["checks_performed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check == "machine_code_composite_register_aggregate_tv"));

    // DESIGN_OBJECT_ACCEPTANCE.md §4: `main` calls `get`/`mk`, whose
    // `Opt` parameter/result are outside the HIR↔LLVM leaf subset, so
    // `hir_llvm_model_correspondence` is `unchecked` and acceptance
    // reports inconclusive with exit 3.
    let accepted = accept_object(&source, &bundle, &id, true);
    assert_eq!(accepted.status.code(), Some(3));
    assert!(accepted.stdout.is_empty());
    assert!(String::from_utf8_lossy(&accepted.stderr).contains("hir_llvm_model_correspondence"));
}

#[test]
fn module_bundle_with_enum_equality_builds_as_v17_and_rechecks() {
    // `e == e'`/`e != e'` on two values of one declared enum pins the
    // module at composite v14: the source model compares the whole
    // tagged-union layout — tag plus every payload slot — the same
    // positional `extractvalue`/`icmp` chain the emitter lowers it to.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "enum Chan { Tcp(p: i64), Idle }\n\
         atom is_idle(c: Chan) -> bool requires: true; ensures: true; body: { c == Chan::Idle };\n\
         atom differs(a: Chan, b: Chan) -> bool requires: true; ensures: true; body: { a != b };\n\
         atom main() -> i64 requires: true; ensures: result == 0; body: { 0 };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V17);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V14
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // A v17 record re-pinned as v16 is refused: v16 keeps enum
    // equality out of the vocabulary.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V16.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn multifile_bundle_with_enum_equality_builds_as_v17() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        root.path().join("chans.mm"),
        "enum Chan { Tcp(p: i64), Idle }\natom is_idle(c: Chan) -> bool requires: true; ensures: true; body: { c == Chan::Idle };\n",
    )
    .unwrap();
    fs::write(
        &source,
        "import \"chans\" as c;\n\natom main() -> i64 requires: true; ensures: true; body: { 0 };\n",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MULTIFILE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MULTIFILE_PROFILE_V17);
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // Under a v16 pin the member's enum equality stays refused.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MULTIFILE_PROFILE_V16.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn module_bundle_with_nested_enum_payload_builds_as_v26_and_rechecks() {
    // An enum payload position typed as another enum flattens into the
    // inner enum's own leaf layout — inner tag plus its payload leaves
    // (`pure-i64-composite/v23`); the record pins bundle v26 and every
    // ABI leaf is 64-bit.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "enum E { X(i64), Y(i64) };\n\
         enum F { A(E), B };\n\
         atom pick(f: F) -> i64 requires: true; ensures: true; body: { match f { F::A(e) => match e { E::X(v) => v, _ => 0 }, _ => 0 } };\n\
         atom wrap(e: E) -> F requires: true; ensures: true; body: { F::A(e) };\n\
         atom eqe(f: F, g: F) -> bool requires: true; ensures: true; body: { f == g };\n\
         atom main() -> i64 requires: true; ensures: true; body: { 0 };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V26);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V23
    );
    assert_eq!(
        record["leaf_widths"]["pick"],
        serde_json::json!([64, 64, 64])
    );
    assert_eq!(record["leaf_widths"]["main"], serde_json::json!([]));
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // A v26 record re-pinned as v25 is refused — v25 keeps enum
    // payloads out of the payload positions.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V25.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn module_bundle_with_inferred_i64_return_builds_as_v27_and_rechecks() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), INFERRED_I64_MODULE);
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V27);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V24
    );
    let binding_v24 = native_bundle::prepare_module_source(
        INFERRED_I64_MODULE,
        mumei_core::semantic_contract::RefinedAdmission::CompositeV24,
    )
    .unwrap()
    .0;
    let binding_v25 = native_bundle::prepare_module_source(
        INFERRED_I64_MODULE,
        mumei_core::semantic_contract::RefinedAdmission::CompositeV25,
    )
    .unwrap()
    .0;
    assert_eq!(
        serde_json::to_vec(&binding_v24).unwrap(),
        serde_json::to_vec(&binding_v25).unwrap()
    );
    assert_eq!(record["source"], serde_json::to_value(binding_v25).unwrap());
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    assert_success(&recheck_object(&source, &bundle, &id, false));
    assert_success(&recheck_machine_tv(&source, &bundle, &id, false));
    // Step 4 direct comparison: the omitted annotation leaves
    // `result_leaf_doubles` empty, but the inferred i64 result still
    // compares — `inc` records `equivalent`, not `skipped`.
    let output = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&output);
    let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let checks = descriptor["object_translation"]["correspondence"]["checks"]
        .as_array()
        .unwrap();
    let equivalence = checks
        .iter()
        .find(|check| check["item"] == "llvm_ir_object_equivalence")
        .unwrap();
    assert_eq!(equivalence["result"], "checked");
    let detail = equivalence["detail"].as_str().unwrap();
    assert!(detail.contains("inc: equivalent"), "{detail}");

    assert_success(&consume("recheck-exit-status", &source, &bundle, &id));
    assert_success(&accept_object(&source, &bundle, &id, false));

    let explicit_root = tempfile::tempdir().unwrap();
    let explicit_source =
        INFERRED_I64_MODULE.replace("atom inc(x: i64)", "atom inc(x: i64) -> i64");
    let (_, explicit_bundle, _) = build_source(explicit_root.path(), &explicit_source);
    let explicit_record = module_record(&explicit_bundle);
    assert_eq!(explicit_record["profile"], native_bundle::MODULE_PROFILE);
    assert_eq!(
        explicit_record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_WRAPPING_PROFILE
    );

    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V26.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    let tampered = recheck(&source, &bundle, &native_bundle::sha256(&bytes));
    assert!(!tampered.status.success());
}

#[test]
fn module_bundle_with_stateful_effects_builds_as_v28_and_rechecks() {
    let root = tempfile::tempdir().unwrap();
    mumei_core::semantic_contract::extract_pure_i64_module_contracts(
        STATEFUL_EFFECT_MODULE,
        "source.mm",
        None,
        mumei_core::semantic_contract::RefinedAdmission::CompositeV25,
    )
    .expect("stateful effect module must extract under CompositeV25");
    let (source, bundle, id) = build_source(root.path(), STATEFUL_EFFECT_MODULE);
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V28);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V25
    );
    assert_eq!(
        record["leaf_widths"]["wide_identity"],
        serde_json::json!([128])
    );
    assert_eq!(record["leaf_widths"]["main"], serde_json::json!([]));
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_eq!(
        Command::new(bundle.join("program"))
            .status()
            .unwrap()
            .code(),
        Some(7)
    );

    let mut downgraded = module_record(&bundle);
    downgraded["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V27.into());
    let bytes = serde_json::to_vec_pretty(&downgraded).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!recheck(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn single_atom_effect_module_routes_to_v28_and_rechecks() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), SINGLE_ATOM_EFFECT_MODULE);
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V28);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V25
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_eq!(
        Command::new(bundle.join("program"))
            .status()
            .unwrap()
            .code(),
        Some(0)
    );
}

#[test]
fn array_parameter_module_builds_as_v29_and_machine_tv_is_inconclusive() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), ARRAY_PARAM_MODULE);
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MODULE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V29);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V26
    );
    assert_eq!(record["leaf_widths"]["read_at"], serde_json::json!([64]));
    assert_eq!(record["leaf_widths"]["main"], serde_json::json!([]));
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));

    let llvm_ir = fs::read_to_string(bundle.join(BundleFile::LlvmIr.file_name())).unwrap();
    let report = mumei_emit_llvm::translation_validation::validate_native_module(
        ARRAY_PARAM_MODULE,
        &llvm_ir,
        Default::default(),
        mumei_core::semantic_contract::RefinedAdmission::CompositeV26,
    )
    .unwrap();
    assert_eq!(
        report
            .functions
            .iter()
            .map(|function| (function.function_name.as_str(), function.outcome))
            .collect::<Vec<_>>(),
        vec![
            (
                "read_at",
                mumei_emit_llvm::translation_validation::Outcome::EquivalentWithinProfile
            ),
            (
                "main",
                mumei_emit_llvm::translation_validation::Outcome::EquivalentWithinProfile
            ),
        ]
    );
    assert!(report
        .input_domain
        .ends_with("_with_read_only_i64_array_params_nonnegative_len"));
    assert!(!report.input_domain.contains("bool_params"));
    assert_eq!(
        Command::new(bundle.join("program"))
            .status()
            .unwrap()
            .code(),
        Some(0)
    );

    let machine_tv = recheck_machine_tv(&source, &bundle, &id, true);
    assert_success(&machine_tv);
    let machine_tv: serde_json::Value = serde_json::from_slice(&machine_tv.stdout).unwrap();
    assert_eq!(
        machine_tv["functions"]["read_at"]["outcome"],
        "outside_profile"
    );
    assert!(machine_tv.to_string().contains("unsupported_signature"));
    // Design memo §7 Step 2 gate: an out-of-profile atom keeps the
    // status honest and enumerates the function it could not evaluate.
    let source_machine_tv = &machine_tv["source_machine_tv"];
    assert_eq!(
        source_machine_tv["status"],
        "unchecked_not_translation_validated"
    );
    let non_equivalent = source_machine_tv["non_equivalent_functions"]
        .as_array()
        .unwrap();
    assert!(non_equivalent
        .iter()
        .any(|entry| { entry["name"] == "read_at" && entry["outcome"] == "outside_profile" }));
    assert_eq!(
        source_machine_tv["unchecked_domains"],
        machine_tv["checks_not_performed"]
    );
    let accepted = accept_object(&source, &bundle, &id, true);
    assert_eq!(accepted.status.code(), Some(3));
    assert!(accepted.stdout.is_empty());
    assert!(String::from_utf8_lossy(&accepted.stderr).contains("object not accepted"));

    let mut downgraded = module_record(&bundle);
    downgraded["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V28.into());
    let bytes = serde_json::to_vec_pretty(&downgraded).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!recheck(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

const ARRAY_RESULT_MODULE: &str = r#"
atom pair(x: i64) -> [i64]
requires: true;
ensures: len(result) == 2 && result[0] == x && result[1] == x;
body: [x, x];

atom use_pair(y: i64) -> i64
requires: y >= 0 && y <= 100;
ensures: result == y + y;
body: { let p = pair(y); p[0] + p[1] };

atom main() -> i64
requires: true;
ensures: result == 4;
body: use_pair(2);
"#;

#[test]
fn array_result_module_builds_as_v30_and_rechecks() {
    let root = tempfile::tempdir().unwrap();
    mumei_core::semantic_contract::extract_pure_i64_module_contracts(
        ARRAY_RESULT_MODULE,
        "source.mm",
        None,
        mumei_core::semantic_contract::RefinedAdmission::CompositeV29,
    )
    .expect("array-result module must extract under CompositeV29");
    let (source, bundle, id) = build_source(root.path(), ARRAY_RESULT_MODULE);
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MODULE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V30);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V29
    );
    // Members touching `[i64]` results are outside the machine-checked
    // profile, so the LLVM recheck stays inconclusive — identity and
    // source postconditions still check out.
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    let llvm_out = recheck_llvm(&source, &bundle, &id);
    assert!(!llvm_out.status.success());
    assert!(String::from_utf8_lossy(&llvm_out.stderr).contains("inconclusive"));
    assert_eq!(
        Command::new(bundle.join("program"))
            .status()
            .unwrap()
            .code(),
        Some(4)
    );

    // A caller proving against the callee's instantiated array ensures
    // fails closed when the body's own contract disagrees.
    let mut downgraded = module_record(&bundle);
    downgraded["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V29.into());
    let bytes = serde_json::to_vec_pretty(&downgraded).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!recheck(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn array_result_len_of_inline_call_rechecks() {
    // `len(make())` — an inline call to a `[i64]`-returning callee
    // reads the fat-pointer `len` slot, so the source recheck must
    // accept it rather than reject the module as out-of-profile.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        r#"atom make() -> [i64]
requires: true;
ensures: len(result) == 2;
body: [1, 2];

atom use_it() -> i64
requires: true;
ensures: result == 2;
body: len(make());

atom main() -> i64
requires: true;
ensures: result == 2;
body: use_it();
"#,
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    // `len(make())` must also survive codegen — the emitted program
    // returns the array's length.
    assert_eq!(
        Command::new(bundle.join("program"))
            .status()
            .unwrap()
            .code(),
        Some(2)
    );
}

#[test]
fn array_result_call_len_is_nonnegative_in_caller_ensures() {
    // The fat-pointer `arrlen` slot is nonnegative even when the
    // callee's contract does not state a length — `len(a) >= 0` must
    // hold in the caller's postcondition, not be refuted.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        r#"atom make() -> [i64]
requires: true;
body: [1, 2, 3];

atom use_it() -> i64
requires: true;
ensures: result >= 0;
body: { let a = make(); len(a) };

atom main() -> i64
requires: true;
ensures: result >= 0;
body: use_it();
"#,
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    // The contract only guarantees nonnegativity; the emitted
    // program still computes the concrete length.
    assert_eq!(
        Command::new(bundle.join("program"))
            .status()
            .unwrap()
            .code(),
        Some(3)
    );
}

#[test]
fn array_result_violating_ensures_is_refuted() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        &source,
        r#"atom pair(x: i64) -> [i64]
requires: true;
ensures: len(result) == 3;
body: [x, x];

atom main() -> i64
requires: true;
ensures: result == 0;
body: 0;
"#,
    )
    .unwrap();
    // `len(result) == 3` disagrees with the literal's length — the
    // contract layer must refute the build rather than emit a record.
    let output = cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]);
    assert!(!output.status.success());
}

#[test]
fn module_bundle_rejects_inferred_non_i64_return() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        &source,
        "atom predicate(x: i64) requires: true; ensures: true; body: { x > 0 };\n\
         atom main() -> i64 requires: true; ensures: result == 4; body: { 4 };",
    )
    .unwrap();
    let output = cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unsupported or invalid module source")
    );
}

#[test]
fn multifile_bundle_with_inferred_i64_return_builds_as_v27() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        root.path().join("helpers.mm"),
        "atom inc(x: i64) requires: x >= 0 && x <= 100; ensures: result == x + 1; body: { x + 1 };",
    )
    .unwrap();
    fs::write(
        &source,
        "import \"helpers\" as h;\n\
         atom main() -> i64 requires: true; ensures: result == 4; body: { h::inc(3) };",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MULTIFILE_PROFILE_V27);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V24
    );
    assert_success(&check(&source, &bundle, &id));
}

#[test]
fn multifile_bundle_imported_effect_declaration_builds_as_v28_and_rechecks() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    let imported_effect = r#"
effect Vault
    states: [Idle, Checked, Effected, Interacted];
    initial: Idle;
    transition check: Idle -> Checked;
    transition update: Checked -> Effected;
    transition interact: Effected -> Interacted;
"#;
    let root_source = r#"
import "effects" as Vault;

atom main() -> i64
    effects: [Vault];
    effect_pre: { Vault: Idle };
    effect_post: { Vault: Interacted };
    requires: true;
    ensures: result == 7;
    body: {
        perform Vault.check;
        perform Vault.update;
        perform Vault.interact;
        7
    }
"#;
    fs::write(root.path().join("effects.mm"), imported_effect).unwrap();
    fs::write(&source, root_source).unwrap();

    let prepared = native_bundle::prepare_multifile_source(
        "app.mm",
        &BTreeMap::from([
            ("app.mm".to_owned(), root_source.to_owned()),
            ("effects.mm".to_owned(), imported_effect.to_owned()),
        ]),
        mumei_core::semantic_contract::RefinedAdmission::CompositeV25,
    )
    .unwrap();
    assert!(prepared.merged.contains("effect Vault"));
    assert!(prepared.merged.contains("perform Vault.check;"));
    assert!(!prepared.merged.contains("Vault::"));
    assert!(native_bundle::prepare_multifile_source(
        "app.mm",
        &BTreeMap::from([
            ("app.mm".to_owned(), root_source.to_owned()),
            (
                "effects.mm".to_owned(),
                "effect Vault(path: i64);".to_owned(),
            ),
        ]),
        mumei_core::semantic_contract::RefinedAdmission::CompositeV25,
    )
    .is_err());

    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MULTIFILE_PROFILE_V28);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V25
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
}

#[test]
fn multifile_bundle_rechecks_an_imported_array_parameter_member_as_v29() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    let imported = r#"
atom size(a: [i64]) -> i64
    requires: true;
    ensures: true;
    body: len(a);
"#;
    let root_source = r#"
import "arrays" as arrays;
atom main() -> i64 requires: true; ensures: result == 0; body: 0;
"#;
    fs::write(root.path().join("arrays.mm"), imported).unwrap();
    fs::write(&source, root_source).unwrap();
    let prepared = native_bundle::prepare_multifile_source(
        "app.mm",
        &BTreeMap::from([
            ("app.mm".to_owned(), root_source.to_owned()),
            ("arrays.mm".to_owned(), imported.to_owned()),
        ]),
        mumei_core::semantic_contract::RefinedAdmission::CompositeV26,
    )
    .unwrap();
    let imported_atom = prepared
        .atoms
        .iter()
        .find(|atom| atom.name != "main")
        .expect("imported array atom must survive merge");
    let imported_name = imported_atom.name.clone();
    assert!(prepared.merged.contains("len("));

    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MULTIFILE_PROFILE_V29);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V26
    );
    assert_eq!(
        record["leaf_widths"][imported_name.as_str()],
        serde_json::json!([])
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
}

fn subdir(root: &Path, name: &str) -> std::path::PathBuf {
    let path = root.join(name);
    fs::create_dir(&path).unwrap();
    path
}

fn save_object_acceptance(
    root: &Path,
    source: &Path,
    bundle: &Path,
    id: &str,
) -> std::path::PathBuf {
    let output = accept_object(source, bundle, id, true);
    assert_success(&output);
    let path = root.join("object-acceptance.json");
    fs::write(&path, output.stdout).unwrap();
    path
}

fn recheck_object_acceptance(source: &Path, bundle: &Path, id: &str, descriptor: &Path) -> Output {
    cli(&[
        "native-bundle",
        "recheck-object-acceptance",
        bundle.to_str().unwrap(),
        source.to_str().unwrap(),
        descriptor.to_str().unwrap(),
        "--expected-build-id",
        id,
    ])
}

fn assert_rejected(output: &Output, reason: &str) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(reason), "{stderr}");
}

#[test]
fn recheck_object_acceptance_accepts_fresh_descriptor_without_authenticity() {
    let root = tempfile::tempdir().unwrap();
    for (name, (source, bundle, id)) in [
        (
            "module",
            build_source(&subdir(root.path(), "module"), MODULE),
        ),
        (
            "multifile",
            build_multifile(&subdir(root.path(), "multifile")),
        ),
    ] {
        let saved = save_object_acceptance(source.parent().unwrap(), &source, &bundle, &id);
        let output = recheck_object_acceptance(&source, &bundle, &id, &saved);
        assert_success(&output);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("Saved native object acceptance descriptor consistent with a fresh object acceptance"),
            "{name}: {stdout}"
        );
        assert!(stdout.contains("Scope: native_bundle_object_translation_correspondence\n"));
        assert!(stdout.contains(
            "Descriptor authenticity, producer authenticity and historical execution: not checked."
        ));
        assert!(!stdout.contains("artifact_verified"));
    }
}

#[test]
fn recheck_object_acceptance_checks_exact_descriptor_signature_when_supplied() {
    const FAKE_KEY: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const OTHER_KEY: &str = "2222222222222222222222222222222222222222222222222222222222222222";
    let public_key = |secret: &str| {
        producer_signature::decode_secret_key_bytes(secret.as_bytes().to_vec())
            .unwrap()
            .verifying_key()
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(&subdir(root.path(), "module"), MODULE);
    let key = root.path().join("producer.key");
    let signature = root.path().join("descriptor.sig.json");
    let descriptor = root.path().join("descriptor.json");
    fs::write(&key, FAKE_KEY).unwrap();
    let accepted = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["native-bundle", "accept-object"])
        .arg(&bundle)
        .arg(&source)
        .args(["--expected-build-id", &id, "--json", "--key-file"])
        .arg(&key)
        .args(["--producer-key-id", "object-producer", "--signature-output"])
        .arg(&signature)
        .args(["--produced-at", "2026-01-02T03:04:05Z"])
        .output()
        .unwrap();
    assert_success(&accepted);
    assert!(!String::from_utf8_lossy(&accepted.stdout).contains(FAKE_KEY));
    assert!(!String::from_utf8_lossy(&accepted.stderr).contains(FAKE_KEY));
    fs::write(&descriptor, &accepted.stdout).unwrap();
    let producer_key = public_key(FAKE_KEY);
    let other_producer_key = public_key(OTHER_KEY);

    let recheck = |extra: &[String]| {
        let mut args = vec![
            "native-bundle".to_owned(),
            "recheck-object-acceptance".to_owned(),
            bundle.to_string_lossy().into_owned(),
            source.to_string_lossy().into_owned(),
            descriptor.to_string_lossy().into_owned(),
            "--expected-build-id".to_owned(),
            id.clone(),
        ];
        args.extend(extra.iter().cloned());
        let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
        cli(&refs)
    };
    let signature_args = || {
        vec![
            "--signature".to_owned(),
            signature.to_string_lossy().into_owned(),
            "--producer-key".to_owned(),
            producer_key.clone(),
            "--expected-producer-key".to_owned(),
            "object-producer".to_owned(),
            "--reference-time".to_owned(),
            "2026-01-02T03:04:10Z".to_owned(),
            "--max-age-seconds".to_owned(),
            "5".to_owned(),
        ]
    };

    let root_key = root.path().join("root.key");
    fs::write(&root_key, OTHER_KEY).unwrap();
    let root_public = public_key(OTHER_KEY);
    let registry = root.path().join("registry.json");
    fs::write(
        &registry,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": "mumei.producer-key-registry/v1",
            "registry": {
                "object-producer": {
                    "principal": "fixture-principal",
                    "not_before": "2026-01-01T00:00:00Z",
                    "not_after": "2026-12-31T23:59:59Z",
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let revocation_list = root.path().join("revocation.json");
    fs::write(
        &revocation_list,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": "mumei.key-revocation-list/v1",
            "revoked_keys": [],
        }))
        .unwrap(),
    )
    .unwrap();
    let sign_json = |record: &Path, secret: &Path, output_path: &Path| {
        let signed = Command::new(env!("CARGO_BIN_EXE_mumei"))
            .args(["evidence", "sign"])
            .arg(record)
            .args(["--key-file"])
            .arg(secret)
            .args(["--producer-key-id", "root-key", "--json"])
            .output()
            .unwrap();
        assert_success(&signed);
        assert!(!String::from_utf8_lossy(&signed.stdout).contains(OTHER_KEY));
        assert!(!String::from_utf8_lossy(&signed.stderr).contains(OTHER_KEY));
        fs::write(output_path, signed.stdout).unwrap();
    };
    let registry_signature = root.path().join("registry.sig.json");
    sign_json(&registry, &root_key, &registry_signature);
    let revocation_signature = root.path().join("revocation.sig.json");
    sign_json(&revocation_list, &root_key, &revocation_signature);

    let output = recheck(&signature_args());
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Producer signature binding: checked for the exact saved descriptor bytes")
    );
    assert!(stdout.contains(
        "optional key-label, freshness, revocation and registry gates only as explicitly supplied"
    ));
    assert!(!stdout.contains("artifact_verified"));
    assert!(!stdout.contains(FAKE_KEY));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(FAKE_KEY));

    let mut json_args = signature_args();
    json_args.push("--json".to_owned());
    let json_output = recheck(&json_args);
    assert_success(&json_output);
    let document: serde_json::Value = serde_json::from_slice(&json_output.stdout).unwrap();
    assert_eq!(
        document["schema_version"],
        serde_json::json!("mumei.native-object-acceptance-recheck/v1")
    );
    assert_eq!(document["consistent"], serde_json::json!(true));
    assert_eq!(
        document["trust_inputs"]["producer_key"],
        serde_json::json!("supplied")
    );
    assert_eq!(
        document["trust_inputs"]["key_registry"],
        serde_json::json!("not_supplied")
    );
    assert_eq!(
        document["producer_signature"]["envelope_fresh"],
        serde_json::json!(true)
    );
    let not_performed = document["checks_not_performed"].as_array().unwrap();
    assert!(not_performed.contains(&serde_json::json!("record_freshness")));
    assert!(not_performed.contains(&serde_json::json!("producer_authenticity")));
    assert!(document["checks_performed"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("producer_signature_envelope_freshness")));
    assert!(!String::from_utf8_lossy(&json_output.stdout).contains(FAKE_KEY));
    assert!(!String::from_utf8_lossy(&json_output.stderr).contains(FAKE_KEY));

    let mut full_chain_args = signature_args();
    full_chain_args.extend([
        "--key-registry".to_owned(),
        registry.to_string_lossy().into_owned(),
        "--registry-signature".to_owned(),
        registry_signature.to_string_lossy().into_owned(),
        "--registry-key".to_owned(),
        root_public.clone(),
        "--revocation-list".to_owned(),
        revocation_list.to_string_lossy().into_owned(),
        "--revocation-signature".to_owned(),
        revocation_signature.to_string_lossy().into_owned(),
        "--revocation-key".to_owned(),
        root_public.clone(),
        "--json".to_owned(),
    ]);
    let full_chain_output = recheck(&full_chain_args);
    assert_success(&full_chain_output);
    let full_chain: serde_json::Value = serde_json::from_slice(&full_chain_output.stdout).unwrap();
    for key in [
        "key_registry",
        "registry_signature",
        "registry_key",
        "revocation_list",
        "revocation_signature",
        "revocation_key",
    ] {
        assert_eq!(
            full_chain["trust_inputs"][key],
            serde_json::json!("supplied"),
            "{key}"
        );
    }
    for field in [
        "key_authentic",
        "registry_authentic",
        "key_not_revoked",
        "revocation_list_authentic",
    ] {
        assert_eq!(
            full_chain["producer_signature"][field],
            serde_json::json!(true),
            "{field}"
        );
    }
    let full_chain_not_performed = full_chain["checks_not_performed"].as_array().unwrap();
    assert!(!full_chain_not_performed.contains(&serde_json::json!("producer_authenticity")));
    assert!(full_chain_not_performed.contains(&serde_json::json!("record_freshness")));
    assert!(!String::from_utf8_lossy(&full_chain_output.stdout).contains(FAKE_KEY));
    assert!(!String::from_utf8_lossy(&full_chain_output.stderr).contains(FAKE_KEY));

    let mut too_strict_args = signature_args();
    let max_age_index = too_strict_args.len() - 1;
    too_strict_args[max_age_index] = "4".to_owned();
    assert_rejected(
        &recheck(&too_strict_args),
        "producer signature binding failed",
    );

    let missing_bundle = root.path().join("missing-bundle");
    let mut ordered_args = vec![
        "native-bundle".to_owned(),
        "recheck-object-acceptance".to_owned(),
        missing_bundle.to_string_lossy().into_owned(),
        source.to_string_lossy().into_owned(),
        descriptor.to_string_lossy().into_owned(),
        "--expected-build-id".to_owned(),
        id.clone(),
    ];
    let mut wrong_key_ordered_args = signature_args();
    wrong_key_ordered_args[3] = other_producer_key.clone();
    ordered_args.extend(wrong_key_ordered_args);
    let ordered_refs = ordered_args.iter().map(String::as_str).collect::<Vec<_>>();
    assert_rejected(&cli(&ordered_refs), "producer signature binding failed");

    let mut wrong_key_args = signature_args();
    wrong_key_args[3] = other_producer_key;
    assert_rejected(
        &recheck(&wrong_key_args),
        "producer signature binding failed",
    );

    let mut wrong_label_args = signature_args();
    wrong_label_args[5] = "other-producer".to_owned();
    assert_rejected(
        &recheck(&wrong_label_args),
        "producer signature binding failed",
    );

    let original = fs::read(&descriptor).unwrap();
    fs::write(&descriptor, [&original[..], b" "].concat()).unwrap();
    assert_rejected(
        &recheck(&signature_args()),
        "producer signature binding failed",
    );
    fs::write(&descriptor, original).unwrap();

    let envelope_bytes = fs::read(&signature).unwrap();
    fs::write(&signature, b"{}").unwrap();
    assert_rejected(
        &recheck(&signature_args()),
        "producer signature binding failed",
    );
    fs::write(&signature, envelope_bytes).unwrap();
}

#[test]
fn recheck_object_acceptance_rejects_unbound_or_tampered_descriptors() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(&subdir(root.path(), "module"), MODULE);
    let saved = save_object_acceptance(root.path(), &source, &bundle, &id);
    let descriptor: serde_json::Value = serde_json::from_slice(&fs::read(&saved).unwrap()).unwrap();
    let tampered = root.path().join("tampered.json");
    let recheck_with = |value: &serde_json::Value| {
        fs::write(&tampered, serde_json::to_vec_pretty(value).unwrap()).unwrap();
        recheck_object_acceptance(&source, &bundle, &id, &tampered)
    };

    let mut forged = descriptor.clone();
    forged["evidence_fingerprint"] = native_bundle::sha256(b"other attempt").into();
    assert_rejected(
        &recheck_with(&forged),
        "not bound to the fresh attempt inputs",
    );

    for (pointer, value) in [
        ("/object_sha256", serde_json::json!("0".repeat(64))),
        ("/checks_not_performed", serde_json::json!([])),
        ("/functions", serde_json::json!({})),
        ("/register_convention_assumed", serde_json::json!(null)),
    ] {
        let mut edited = descriptor.clone();
        *edited.pointer_mut(pointer).unwrap() = value;
        assert_rejected(
            &recheck_with(&edited),
            "differs from the fresh object acceptance",
        );
    }
    let mut extended = descriptor.clone();
    extended["descriptor_authenticity"] = "checked".into();
    assert_rejected(
        &recheck_with(&extended),
        "differs from the fresh object acceptance",
    );
    let mut truncated = descriptor.clone();
    truncated
        .as_object_mut()
        .unwrap()
        .remove("source_machine_tv");
    assert_rejected(
        &recheck_with(&truncated),
        "differs from the fresh object acceptance",
    );

    // A descriptor from another attempt is stale for this bundle even when
    // every field is internally consistent.
    let (other_source, other_bundle, other_id) =
        build_source(&subdir(root.path(), "other"), WHILE_PROBE);
    let stale = save_object_acceptance(
        other_source.parent().unwrap(),
        &other_source,
        &other_bundle,
        &other_id,
    );
    assert_rejected(
        &recheck_object_acceptance(&source, &bundle, &id, &stale),
        "not bound to the fresh attempt inputs",
    );

    assert_rejected(
        &recheck_object_acceptance(&source, &bundle, &other_id, &saved),
        "",
    );
    assert_rejected(
        &recheck_object_acceptance(&source, &bundle, "not-a-digest", &saved),
        "invalid expected build ID",
    );
}

#[test]
fn recheck_object_acceptance_rejects_malformed_or_foreign_descriptors() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(&subdir(root.path(), "module"), MODULE);
    let saved = save_object_acceptance(root.path(), &source, &bundle, &id);
    let text = fs::read_to_string(&saved).unwrap();
    let descriptor: serde_json::Value = serde_json::from_str(&text).unwrap();
    let candidate = root.path().join("candidate.json");
    let recheck_bytes = |bytes: &[u8]| {
        fs::write(&candidate, bytes).unwrap();
        recheck_object_acceptance(&source, &bundle, &id, &candidate)
    };
    let malformed = "malformed native object acceptance descriptor";

    let duplicated = text.replacen(
        "\"accepted\": true,",
        "\"accepted\": false,\n  \"accepted\": true,",
        1,
    );
    assert_ne!(duplicated, text);
    assert_rejected(&recheck_bytes(duplicated.as_bytes()), malformed);
    assert_rejected(
        &recheck_bytes(&text.as_bytes()[..text.len() / 2]),
        malformed,
    );
    assert_rejected(&recheck_bytes(b""), malformed);
    for (key, value) in [
        ("accepted", serde_json::json!(false)),
        ("acceptance", serde_json::json!("not_evaluated")),
        (
            "schema_version",
            serde_json::json!("mumei.native-consumer-evidence/v1"),
        ),
        (
            "scope",
            serde_json::json!("native_bundle_identity_source_llvm_and_saved_evidence_consistency"),
        ),
        ("evidence_fingerprint", serde_json::json!("not-a-digest")),
    ] {
        let mut edited = descriptor.clone();
        edited[key] = value;
        assert_rejected(
            &recheck_bytes(&serde_json::to_vec(&edited).unwrap()),
            malformed,
        );
    }

    // A saved accept-evidence descriptor shares the schema but not the scope.
    let evidence = save_evidence(root.path(), &source, &bundle, &id);
    let accept_evidence = accept_command(&source, &bundle, &id, &evidence, true)
        .output()
        .unwrap();
    assert_success(&accept_evidence);
    assert_rejected(&recheck_bytes(&accept_evidence.stdout), malformed);
    assert_rejected(&recheck_bytes(&fs::read(&evidence).unwrap()), malformed);

    let link = root.path().join("link.json");
    std::os::unix::fs::symlink(&saved, &link).unwrap();
    let output = recheck_object_acceptance(&source, &bundle, &id, &link);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let oversized = vec![b' '; 1024 * 1024 + 1];
    let output = recheck_bytes(&oversized);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn recheck_object_acceptance_rejects_tampered_pinned_inputs() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(&subdir(root.path(), "module"), MODULE);
    let saved = save_object_acceptance(root.path(), &source, &bundle, &id);
    let original = fs::read(&source).unwrap();
    fs::write(&source, MODULE.replace("helper(5)", "helper(4)")).unwrap();
    let output = recheck_object_acceptance(&source, &bundle, &id, &saved);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    fs::write(&source, original).unwrap();
    let object = bundle.join(BundleFile::Object.file_name());
    let mut bytes = fs::read(&object).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    fs::write(&object, bytes).unwrap();
    let output = recheck_object_acceptance(&source, &bundle, &id, &saved);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn recheck_object_acceptance_inconclusive_fresh_acceptance_establishes_no_result() {
    let root = tempfile::tempdir().unwrap();
    let (accepted_source, accepted_bundle, accepted_id) =
        build_source(&subdir(root.path(), "accepted"), MODULE);
    let saved = save_object_acceptance(
        root.path(),
        &accepted_source,
        &accepted_bundle,
        &accepted_id,
    );
    let (source, bundle, id) = build_source(
        &subdir(root.path(), "division"),
        "atom quotient_seven(x: i64) -> i64 requires: true; ensures: true; body: { x / 7 };\n\
atom main() -> i64 requires: true; ensures: true; body: { quotient_seven(3) };",
    );
    let output = recheck_object_acceptance(&source, &bundle, &id, &saved);
    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("no object acceptance consistency result established"));
}

#[test]
fn recheck_object_acceptance_replays_mixed_register_return_pairs() {
    let root = tempfile::tempdir().unwrap();
    let mut saved = Vec::new();
    for (name, text) in [
        (
            "f64-i64",
            "struct B { v: f64, t: i64 }\n\
atom wrap(x: f64, t: i64) -> B requires: t >= 0; ensures: result.t == t; body: { B{v: x, t: t} };\n\
atom main() -> i64 requires: true; ensures: result == 1; body: { 1 };",
        ),
        (
            "i64-f64",
            "struct B { t: i64, v: f64 }\n\
atom wrap(t: i64, x: f64) -> B requires: t >= 0; ensures: result.t == t; body: { B{t: t, v: x} };\n\
atom main() -> i64 requires: true; ensures: result == 1; body: { 1 };",
        ),
    ] {
        let (source, bundle, id) = build_source(&subdir(root.path(), name), text);
        let descriptor = save_object_acceptance(source.parent().unwrap(), &source, &bundle, &id);
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(&descriptor).unwrap()).unwrap();
        assert_eq!(
            value["functions"]["wrap"]["outcome"], "equivalent_within_profile",
            "{name}: {value}"
        );
        assert_success(&recheck_object_acceptance(
            &source,
            &bundle,
            &id,
            &descriptor,
        ));

        let mut tampered = value.clone();
        tampered["functions"]["wrap"]["outcome"] = "outside_profile".into();
        let tampered_path = source.parent().unwrap().join("tampered.json");
        fs::write(&tampered_path, serde_json::to_vec(&tampered).unwrap()).unwrap();
        assert_rejected(
            &recheck_object_acceptance(&source, &bundle, &id, &tampered_path),
            "saved object acceptance descriptor differs from the fresh object acceptance",
        );
        saved.push((source, bundle, id, descriptor));
    }
    let (_, _, _, f64_first) = &saved[0];
    let (source, bundle, id, _) = &saved[1];
    assert_rejected(
        &recheck_object_acceptance(source, bundle, id, f64_first),
        "saved object acceptance descriptor is not bound to the fresh attempt inputs",
    );
}

#[test]
fn recheck_object_acceptance_json_marks_unsigned_authenticity_inputs_not_supplied() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(&subdir(root.path(), "module"), MODULE);
    let saved = save_object_acceptance(source.parent().unwrap(), &source, &bundle, &id);
    let output = cli(&[
        "native-bundle",
        "recheck-object-acceptance",
        bundle.to_str().unwrap(),
        source.to_str().unwrap(),
        saved.to_str().unwrap(),
        "--expected-build-id",
        &id,
        "--json",
    ]);
    assert_success(&output);
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        document["schema_version"],
        serde_json::json!("mumei.native-object-acceptance-recheck/v1")
    );
    assert!(document.get("producer_signature").is_none());
    for key in [
        "producer_key",
        "producer_key_id_pin",
        "reference_time",
        "max_age_seconds",
        "key_registry",
        "revocation_list",
    ] {
        assert_eq!(
            document["trust_inputs"][key],
            serde_json::json!("not_supplied"),
            "{key}"
        );
    }
    let not_performed = document["checks_not_performed"].as_array().unwrap();
    for check in [
        "descriptor_authenticity",
        "record_freshness",
        "producer_authenticity",
        "historical_execution",
    ] {
        assert!(not_performed.contains(&serde_json::json!(check)), "{check}");
    }
}
