use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

fn run_verify(fixture: &str, bitvec_i64: bool) -> Output {
    let dir = tempfile::tempdir().expect("create verification directory");
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(fixture);
    let copied = dir.path().join(fixture);
    std::fs::copy(source, &copied).expect("copy scalar-array payload fixture");

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
fn empty_scalar_array_payloads_are_equal_directly_and_nested() {
    for bitvec_i64 in [false, true] {
        let output = run_verify(
            "test_enum_scalar_array_payload_equality_positive.mm",
            bitvec_i64,
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "empty scalar-array payload equality must verify (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        let payload = verify_json(&output);
        assert_eq!(payload["status"], "success", "{payload:#}");
        assert!(
            payload["diagnostics"]
                .as_array()
                .is_some_and(|diagnostics| diagnostics.is_empty()),
            "unexpected diagnostics (bitvec={bitvec_i64}):\n{payload:#}"
        );
    }
}

#[test]
fn scalar_array_payload_contents_remain_extensional() {
    for bitvec_i64 in [false, true] {
        let output = run_verify(
            "test_enum_scalar_array_payload_equality_negative.mm",
            bitvec_i64,
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "different in-range contents must not compare equal (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        let payload = verify_json(&output);
        let diagnostic = payload["diagnostics"]
            .as_array()
            .and_then(|diagnostics| {
                diagnostics.iter().find(|entry| {
                    entry["atom"] == "scalar_array_payload_different_contents_reject_equality"
                })
            })
            .unwrap_or_else(|| {
                panic!("missing different-content equality diagnostic (bitvec={bitvec_i64}):\n{payload:#}")
            });
        assert_eq!(
            diagnostic["code"], "failed",
            "different-content equality must fail verification, not typecheck (bitvec={bitvec_i64}):\n{payload:#}"
        );
        assert_eq!(
            diagnostic["failure_type"], "postcondition_violated",
            "different-content equality must fail its postcondition (bitvec={bitvec_i64}):\n{payload:#}"
        );
        assert!(
            diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains("Postcondition (ensures) is not satisfied")),
            "different-content equality must report a postcondition failure, not a type error (bitvec={bitvec_i64}):\n{payload:#}"
        );
    }
}
