use std::path::Path;
use std::process::Command;

#[test]
fn match_binding_shadows_outer_enum_payload_type() {
    let dir = tempfile::tempdir().expect("create verification directory");
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("test_nominal_payload_shadowing.mm");
    let copied = dir.path().join("test_nominal_payload_shadowing.mm");
    std::fs::copy(source, &copied).expect("copy nominal-shadow fixture");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--json")
        .arg("--disable-spurious-detection")
        .arg("--report-dir")
        .arg(dir.path())
        .arg(&copied)
        .current_dir(dir.path())
        .output()
        .expect("run mumei verify");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "the unshadowed A-versus-B mismatch must be rejected:\n{stdout}\n{stderr}"
    );
    let json_start = stdout
        .find('{')
        .unwrap_or_else(|| panic!("verify --json emitted no JSON:\n{stdout}\n{stderr}"));
    let payload: serde_json::Value = serde_json::from_str(&stdout[json_start..])
        .unwrap_or_else(|err| panic!("{err}: invalid verify JSON:\n{stdout}\n{stderr}"));
    let diagnostics = payload["diagnostics"]
        .as_array()
        .unwrap_or_else(|| panic!("verify JSON has no diagnostics array:\n{payload:#}"));
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|entry| entry["code"] == "failed" && entry["severity"] == "error")
        .collect();
    assert_eq!(
        errors.len(),
        1,
        "only the deliberate unshadowed mismatch should fail:\n{payload:#}"
    );
    assert_eq!(
        errors[0]["atom"], "unshadowed_payload_mismatch",
        "the shadowed call must remain accepted:\n{payload:#}"
    );
    assert!(
        errors[0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("expects struct 'A' but got struct 'B'")),
        "the unshadowed call failed for the wrong reason:\n{payload:#}"
    );
}
