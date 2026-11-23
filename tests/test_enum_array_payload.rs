use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

fn run_verify(fixture: &str, bitvec_i64: bool) -> Output {
    let dir = tempfile::tempdir().expect("create verification directory");
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(fixture);
    let copied = dir.path().join(fixture);
    std::fs::copy(source, &copied).expect("copy enum-array fixture");
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command
        .arg("verify")
        .arg("--json")
        .arg("--disable-spurious-detection");
    if bitvec_i64 {
        command.arg("--bitvec-i64");
    }
    command
        .arg(&copied)
        .current_dir(dir.path())
        .output()
        .expect("run mumei verify")
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
fn direct_array_enum_payloads_verify_in_both_integer_modes() {
    for bitvec_i64 in [false, true] {
        let output = run_verify("test_enum_array_payload_positive.mm", bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "positive enum-array cases must verify (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        let payload = verify_json(&output);
        assert_eq!(
            payload["status"], "success",
            "positive enum-array fixture did not verify (bitvec={bitvec_i64}):\n{payload:#}"
        );
        assert!(
            payload["diagnostics"]
                .as_array()
                .is_some_and(|diagnostics| diagnostics.is_empty()),
            "positive enum-array fixture emitted diagnostics (bitvec={bitvec_i64}):\n{payload:#}"
        );
    }
}

#[test]
fn generic_enum_and_struct_payloads_verify_in_both_integer_modes() {
    for bitvec_i64 in [false, true] {
        for fixture in [
            "test_enum_generic.mm",
            "test_enum_generic_ctor.mm",
            "test_generic_struct.mm",
        ] {
            let output = run_verify(fixture, bitvec_i64);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                output.status.success(),
                "generic payload fixture {fixture} must verify (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
            );
            let payload = verify_json(&output);
            assert_eq!(
                payload["status"], "success",
                "generic payload fixture {fixture} did not verify (bitvec={bitvec_i64}):\n{payload:#}"
            );
        }
    }
}

#[test]
fn unsupported_or_unsafe_enum_array_uses_fail_closed_in_both_modes() {
    const EXPECTED_FAILURES: [&str; 10] = [
        "false_length_claim",
        "out_of_bounds_payload_read",
        "independent_payload_lengths",
        "different_direct_payload_roots_keep_lengths_separate",
        "stale_array_constructor_uses_updated_elements",
        "later_argument_mutation_does_not_change_array_payload",
        "stale_array_tagged_write_uses_updated_elements",
        "match_tail_uses_outer_shadowed_length",
        "nested_array_payload",
        "untagged_array_read",
    ];
    for bitvec_i64 in [false, true] {
        let output = run_verify("test_enum_array_payload_negative.mm", bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "negative enum-array cases must fail (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        assert!(
            !stderr.contains("panicked"),
            "negative enum-array cases must fail without panic (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
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
                "missing expected enum-array failure for {atom} (bitvec={bitvec_i64}):\n{payload:#}"
            );
        }
    }
}

#[test]
fn enum_array_write_without_a_tag_proof_is_rejected() {
    for bitvec_i64 in [false, true] {
        let output = run_verify("test_enum_array_payload_write_negative.mm", bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "untagged enum-array write must fail (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        let payload = verify_json(&output);
        assert!(
            payload["diagnostics"]
                .as_array()
                .is_some_and(|diagnostics| {
                    diagnostics
                        .iter()
                        .any(|entry| entry["atom"] == "untagged_array_write")
                }),
            "untagged enum-array write had no atom diagnostic (bitvec={bitvec_i64}):\n{payload:#}"
        );
    }
}

#[test]
fn struct_field_indexing_remains_unsupported_as_on_develop() {
    for bitvec_i64 in [false, true] {
        let output = run_verify("test_struct_field_indexing_unchanged.mm", bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            output.status.code(),
            Some(4),
            "struct-field indexing should remain a parser input error (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        let payload = verify_json(&output);
        assert_eq!(
            payload["status"], "input_error",
            "struct-field indexing outcome diverged from develop (bitvec={bitvec_i64}):\n{payload:#}"
        );
        assert!(
            payload["diagnostics"]
                .as_array()
                .is_some_and(|diagnostics| {
                    diagnostics.iter().any(|entry| {
                        entry["message"]
                            .as_str()
                            .is_some_and(|message| message.contains("indexing into an expression"))
                    })
                }),
            "struct-field indexing did not report the develop parser diagnostic (bitvec={bitvec_i64}):\n{payload:#}"
        );
    }
}
