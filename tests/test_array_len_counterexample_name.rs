use std::process::Command;

#[test]
fn array_length_companion_does_not_overwrite_user_len_binding() {
    let dir = tempfile::tempdir().expect("create verification directory");
    let fixture = dir.path().join("array_length_name_collision.mm");
    std::fs::write(
        &fixture,
        r#"
atom counterexample_len_name_collision(xs: [i64], len_xs: i64) -> i64
    requires: len(xs) == 3 && len_xs == 9;
    ensures: result == 1;
    body: 0
"#,
    )
    .expect("write fixture");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--json")
        .arg("--disable-spurious-detection")
        .arg("--report-dir")
        .arg(dir.path())
        .arg(&fixture)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run mumei verify");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "the false ensures clause must fail"
    );
    let report: serde_json::Value = std::fs::read(dir.path().join("report.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| panic!("missing report.json:\n{stdout}\n{stderr}"));
    assert_eq!(report["counterexample"]["len_xs"], "9");
    assert!(report["counterexample_provenance"]["values"]["len(xs)"].is_object());
    let loss = &report["semantic_feedback"]["reconstruction_loss"]["counter_example"];
    assert_eq!(loss["len_xs"].as_i64(), Some(9));
    assert_eq!(loss["len(xs)"].as_i64(), Some(3));
}
