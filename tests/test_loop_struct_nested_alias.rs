use std::path::Path;
use std::process::{Command, Output};

fn run_verify(fixture_name: &str) -> (Output, serde_json::Value, String) {
    let dir = tempfile::tempdir().expect("create verification directory");
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(fixture_name);
    let copied = dir.path().join(fixture_name);
    std::fs::copy(source, &copied).expect("copy loop struct fixture");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--disable-spurious-detection")
        .arg("--report-dir")
        .arg(dir.path())
        .arg(&copied)
        .current_dir(dir.path())
        .output()
        .expect("run mumei verify");
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = serde_json::from_slice(
        &std::fs::read(dir.path().join("report.json")).expect("read verification report"),
    )
    .unwrap_or_else(|err| panic!("{err}: invalid report.json:\n{text}"));
    (output, report, text)
}

#[test]
fn loop_havoc_preserves_nested_alias_precision_without_proving_false_claims() {
    let (positive_output, positive_report, positive_text) =
        run_verify("test_loop_struct_nested_alias_positive.mm");
    assert!(
        positive_output.status.success(),
        "the invariant-backed nested alias must verify:\n{positive_text}\n{positive_report:#}"
    );

    let (negative_output, negative_report, negative_text) =
        run_verify("test_loop_struct_nested_alias_negative.mm");
    assert!(
        !negative_output.status.success(),
        "the false postcondition must fail:\n{negative_text}\n{negative_report:#}"
    );
    assert_eq!(negative_report["failure_type"], "postcondition_violated");
    assert_eq!(negative_report["failed_clause"], "result == 0");
}
