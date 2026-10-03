use std::process::{Command, Output};

fn verify_source(tag: &str, source: &str) -> (Output, serde_json::Value, String) {
    let dir = std::env::temp_dir().join(format!(
        "mumei_clause_labels_{}_{}_{}",
        std::process::id(),
        tag,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).expect("create fixture directory");
    let fixture = dir.join("fixture.mm");
    let report_dir = dir.join("reports");
    std::fs::create_dir_all(&report_dir).expect("create report directory");
    std::fs::write(&fixture, source).expect("write fixture");

    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--report-dir")
        .arg(&report_dir)
        .arg("--disable-spurious-detection")
        .arg(&fixture)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run mumei verify");
    let report: serde_json::Value = serde_json::from_slice(
        &std::fs::read(report_dir.join("report.json")).expect("read report.json"),
    )
    .expect("parse report.json");
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::remove_dir_all(dir).expect("remove fixture directory");
    (output, report, text)
}

#[test]
fn labeled_ensures_failure_reports_the_clause_and_label() {
    let (output, report, text) = verify_source(
        "labeled_failure",
        r#"
atom positive(x: i64) -> i64
requires: true;
ensures "must be positive": result > 0;
body: x;
"#,
    );

    assert!(!output.status.success(), "{text}");
    assert_eq!(report["failure_type"], "postcondition_violated");
    assert_eq!(report["failed_clause"], "result > 0");
    assert_eq!(report["failed_clause_label"], "must be positive");
    assert!(
        text.contains("violated ensures clause \"must be positive\": result > 0"),
        "{text}"
    );
}

#[test]
fn conjunction_label_matches_the_failing_conjunct() {
    let (output, report, text) = verify_source(
        "conjunct_failure",
        r#"
atom bounded(x: i64) -> i64
requires: x >= 0;
ensures "bounded result": result >= 0 && result <= 2;
body: x;
"#,
    );

    assert!(!output.status.success(), "{text}");
    assert_eq!(report["failed_clause"], "result <= 2");
    assert_eq!(report["failed_clause_label"], "bounded result");
}

#[test]
fn unlabeled_ensures_failure_omits_the_label_key() {
    let (output, report, text) = verify_source(
        "unlabeled_failure",
        r#"
atom positive(x: i64) -> i64
requires: true;
ensures: result > 0;
body: x;
"#,
    );

    assert!(!output.status.success(), "{text}");
    assert_eq!(report["failed_clause"], "result > 0");
    assert!(report.get("failed_clause_label").is_none());
}

#[test]
fn labeled_and_unlabeled_passing_atoms_verify_identically() {
    let labeled = verify_source(
        "labeled_success",
        r#"
atom increment(x: i64) -> i64
requires "nonnegative input": x >= 0;
ensures "increments input": result == x + 1;
body: x + 1;
"#,
    );
    let unlabeled = verify_source(
        "unlabeled_success",
        r#"
atom increment(x: i64) -> i64
requires: x >= 0;
ensures: result == x + 1;
body: x + 1;
"#,
    );

    assert!(labeled.0.status.success(), "{}", labeled.2);
    assert!(unlabeled.0.status.success(), "{}", unlabeled.2);
    for field in ["status", "reason", "failure_type", "skipped_clauses"] {
        assert_eq!(
            labeled.1[field], unlabeled.1[field],
            "different {field} for labeled and unlabeled atoms"
        );
    }
}
