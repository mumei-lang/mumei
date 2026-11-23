use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

fn run_verify(bitvec_i64: bool) -> Output {
    let dir = tempfile::tempdir().expect("create verification directory");
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("test_enum_match_tail_struct_array_payload.mm");
    let copied = dir
        .path()
        .join("test_enum_match_tail_struct_array_payload.mm");
    std::fs::copy(source, &copied).expect("copy match-tail payload fixture");

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
fn block_tail_match_struct_array_payload_resolves_typed_arm_result() {
    for bitvec_i64 in [false, true] {
        let output = run_verify(bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "block-tail match struct-array payload must verify (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
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
