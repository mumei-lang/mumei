use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

fn run_verify(fixture_name: &str, bitvec_i64: bool) -> Output {
    let dir = tempfile::tempdir().expect("create verification directory");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(fixture_name);
    let copied = dir.path().join(fixture_name);
    std::fs::copy(fixture, &copied).expect("copy struct-field array length fixture");
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
fn struct_field_array_lengths_are_root_keyed_in_both_integer_modes() {
    for bitvec_i64 in [false, true] {
        let output = run_verify("test_struct_field_array_len_positive.mm", bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "positive struct-field array length cases must verify (bitvec={bitvec_i64}):\n\
             {stdout}\n{stderr}"
        );
        let payload = verify_json(&output);
        assert_eq!(
            payload["status"], "success",
            "positive fixture did not verify (bitvec={bitvec_i64}):\n{payload:#}"
        );
        assert!(
            payload["diagnostics"]
                .as_array()
                .is_some_and(|diagnostics| diagnostics.is_empty()),
            "positive fixture emitted diagnostics (bitvec={bitvec_i64}):\n{payload:#}"
        );
    }
}

#[test]
fn distinct_struct_field_array_roots_cannot_share_lengths() {
    const EXPECTED_FAILURES: [&str; 9] = [
        "len_confusion",
        "len_confusion_eq",
        "len_nested_confusion",
        "len_if_branch_confusion",
        "len_match_branch_confusion",
        "len_field_length_stays_with_original_root",
        "len_field_conflicts_with_parameter_name",
        "len_plain_array_conflicts_with_parameter_name",
        "len_plain_array_false_claim",
    ];
    for bitvec_i64 in [false, true] {
        let output = run_verify("test_struct_field_array_len_negative.mm", bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "cross-root length claims must be rejected (bitvec={bitvec_i64}):\n\
             {stdout}\n{stderr}"
        );
        assert!(
            !stderr.contains("panicked"),
            "cross-root length fixture must fail without panicking:\n{stdout}\n{stderr}"
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
                "missing cross-root failure for {atom} (bitvec={bitvec_i64}):\n{payload:#}"
            );
        }
    }
}
