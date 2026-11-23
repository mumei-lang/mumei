use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

fn run_command(command: &str, fixture_name: &str, bitvec_i64: bool) -> Output {
    let dir = tempfile::tempdir().expect("create verification directory");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(fixture_name);
    let copied = dir.path().join(fixture_name);
    std::fs::copy(fixture, &copied).expect("copy enum struct-array fixture");
    assert!(!dir.path().join(".mumei_cache").exists());

    let mut invocation = Command::new(env!("CARGO_BIN_EXE_mumei"));
    invocation.arg(command);
    if command == "verify" {
        invocation.arg("--json").arg("--disable-spurious-detection");
    }
    if bitvec_i64 {
        invocation.arg("--bitvec-i64");
    }
    invocation
        .arg(&copied)
        .current_dir(dir.path())
        .output()
        .expect("run mumei command")
}

fn verify_json(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json_start = stdout
        .find('{')
        .unwrap_or_else(|| panic!("verify --json emitted no JSON:\n{stdout}\n{stderr}"));
    serde_json::from_str(&stdout[json_start..])
        .unwrap_or_else(|err| panic!("{err}: invalid verify JSON:\n{stdout}\n{stderr}"))
}

#[test]
fn struct_array_payloads_verify_in_both_integer_modes() {
    for bitvec_i64 in [false, true] {
        let output = run_command(
            "verify",
            "test_enum_struct_array_payload_positive.mm",
            bitvec_i64,
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "positive struct-array payload cases must verify (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        let payload = verify_json(&output);
        assert_eq!(
            payload["status"], "success",
            "positive struct-array fixture did not verify (bitvec={bitvec_i64}):\n{payload:#}"
        );
        assert!(
            payload["diagnostics"]
                .as_array()
                .is_some_and(|diagnostics| diagnostics.is_empty()),
            "positive struct-array fixture emitted diagnostics (bitvec={bitvec_i64}):\n{payload:#}"
        );
    }
}

#[test]
fn unsafe_and_unsupported_struct_array_payloads_fail_closed_in_both_modes() {
    const EXPECTED_FAILURES: [&str; 29] = [
        "overclaimed_struct_array_length",
        "out_of_bounds_struct_array_index",
        "different_struct_fields_keep_lengths_separate",
        "different_enum_payloads_keep_lengths_separate",
        "later_argument_mutation_does_not_change_struct_array_payload",
        "whole_struct_array_slot_write_rejected",
        "untagged_struct_array_read",
        "untagged_struct_array_write",
        "nested_array_in_struct_payload",
        "struct_array_in_struct_payload",
        "direct_struct_array_payload_different_elements",
        "nested_enum_struct_array_equality_rejects_different_contents",
        "direct_struct_array_payload_stale_reconstruction",
        "call_result_array_leaf_cache_ignored_after_store",
        "match_result_array_leaf_cache_ignored_after_store",
        "match_result_shadowed_struct_array_does_not_reuse_outer",
        "block_tail_array_payload_shared_write_rejected",
        "alias_array_payload_shared_write_rejected",
        "branch_array_payload_shared_write_rejected",
        "tagged_direct_struct_array_payload_read_remains_closed",
        "direct_struct_array_u32_constructor_domain",
        "direct_struct_array_payload_field_write_rejected",
        "direct_struct_array_payload_whole_element_write_rejected",
        "direct_struct_array_payload_alias_write_rejected",
        "direct_struct_array_payload_callee_write_rejected",
        "direct_struct_array_nested_array_remains_closed",
        "direct_struct_array_recursive_enum_remains_closed",
        "direct_struct_array_generic_enum_remains_closed",
        "recursive_struct_enum_array_payload_fails_closed",
    ];
    for bitvec_i64 in [false, true] {
        let output = run_command(
            "verify",
            "test_enum_struct_array_payload_negative.mm",
            bitvec_i64,
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "negative struct-array payload cases must fail (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        assert!(
            !stdout.contains("panicked") && !stderr.contains("panicked"),
            "negative struct-array fixture must not panic (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        let payload = verify_json(&output);
        let diagnostics = payload["diagnostics"]
            .as_array()
            .unwrap_or_else(|| panic!("verify JSON has no diagnostics array:\n{payload:#}"));
        for atom in EXPECTED_FAILURES {
            assert!(
                diagnostics
                    .iter()
                    .any(|entry| entry["atom"].as_str() == Some(atom)),
                "missing expected struct-array failure for {atom} (bitvec={bitvec_i64}):\n{payload:#}"
            );
        }
        for atom in [
            "direct_struct_array_payload_different_elements",
            "nested_enum_struct_array_equality_rejects_different_contents",
        ] {
            let diagnostic = diagnostics
                .iter()
                .find(|entry| entry["atom"] == atom)
                .unwrap_or_else(|| {
                    panic!("missing equality diagnostic for {atom} (bitvec={bitvec_i64}):\n{payload:#}")
                });
            assert_eq!(
                diagnostic["code"], "failed",
                "different-content equality must fail verification, not typecheck, for {atom} (bitvec={bitvec_i64}):\n{payload:#}"
            );
            assert_eq!(
                diagnostic["failure_type"], "postcondition_violated",
                "different-content equality must fail its postcondition for {atom} (bitvec={bitvec_i64}):\n{payload:#}"
            );
            assert!(
                diagnostic["message"]
                    .as_str()
                    .is_some_and(|message| {
                        message.contains("Postcondition (ensures) is not satisfied")
                    }),
                "different-content equality must report a postcondition failure, not a type error, for {atom} (bitvec={bitvec_i64}):\n{payload:#}"
            );
        }
        let direct_read = diagnostics
            .iter()
            .find(|entry| entry["atom"] == "tagged_direct_struct_array_payload_read_remains_closed")
            .unwrap_or_else(|| {
                panic!("missing direct-read diagnostic (bitvec={bitvec_i64}):\n{payload:#}")
            });
        assert!(
            direct_read["message"]
                .as_str()
                .is_some_and(|message| message.contains("destructure the payload with match")),
            "direct struct-array reads must explain the supported match route (bitvec={bitvec_i64}):\n{payload:#}"
        );
    }
}

#[test]
fn struct_array_enum_equality_budget_fails_closed() {
    let output = run_command("verify", "test_enum_payload_equality_budget.mm", false);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "large nested struct-array enum equality must fail closed:\n{stdout}\n{stderr}"
    );
    let payload = verify_json(&output);
    let diagnostic = payload["diagnostics"]
        .as_array()
        .and_then(|diagnostics| {
            diagnostics
                .iter()
                .find(|entry| entry["atom"] == "struct_array_enum_equality_exceeds_budget")
        })
        .unwrap_or_else(|| panic!("missing equality budget diagnostic:\n{payload:#}"));
    assert!(
        diagnostic["message"]
            .as_str()
            .is_some_and(|message| message.contains("work budget")),
        "struct-array equality budget must return a type error:\n{payload:#}"
    );
}

#[test]
fn codegen_rejects_struct_array_enum_payload_cleanly() {
    let output = run_command(
        "build",
        "test_enum_struct_array_payload_codegen_unsupported.mm",
        false,
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "struct-array enum payload codegen should remain unsupported:\n{stdout}\n{stderr}"
    );
    assert!(
        stderr.contains("struct-element array payloads are not supported by LLVM codegen")
            && !stderr.contains("panicked")
            && !stdout.contains("panicked"),
        "unsupported struct-array enum payload codegen must fail cleanly:\n{stdout}\n{stderr}"
    );
}

#[test]
fn arrays_of_enums_inside_struct_payloads_remain_rejected() {
    for bitvec_i64 in [false, true] {
        let output = run_command(
            "verify",
            "test_enum_struct_array_payload_negative.mm",
            bitvec_i64,
        );
        let payload = verify_json(&output);
        assert!(
            payload["diagnostics"]
                .as_array()
                .is_some_and(|diagnostics| {
                    diagnostics
                        .iter()
                        .any(|entry| entry["atom"] == "enum_array_in_struct_payload")
                }),
            "array-of-enum payload was not rejected (bitvec={bitvec_i64}):\n{payload:#}"
        );
    }
}

#[test]
fn mumei_build_handles_struct_array_payload_without_panicking() {
    let output = run_command("build", "test_enum_struct_array_payload_build.mm", false);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "mumei build unexpectedly rejected a supported struct-array payload:\n{stdout}\n{stderr}"
    );
    assert!(
        !stdout.contains("panicked") && !stderr.contains("panicked"),
        "mumei build must fail cleanly without a panic:\n{stdout}\n{stderr}"
    );
}
