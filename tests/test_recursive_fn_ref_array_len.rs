use std::path::Path;
use std::process::Command;

#[test]
fn recursive_direct_and_reference_array_results_share_length_projection() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("test_recursive_fn_ref_array_len.mm");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--json")
        .arg("--disable-spurious-detection")
        .arg(fixture)
        .output()
        .expect("run mumei verify");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "recursive direct/reference array-length regression failed:\n{stdout}\n{stderr}"
    );
    let json_start = stdout
        .find('{')
        .unwrap_or_else(|| panic!("verify --json emitted no JSON:\n{stdout}\n{stderr}"));
    let report: serde_json::Value = serde_json::from_str(&stdout[json_start..])
        .unwrap_or_else(|error| panic!("{error}: invalid verify JSON:\n{stdout}\n{stderr}"));
    assert_eq!(report["status"], "success", "{report:#}");
}
