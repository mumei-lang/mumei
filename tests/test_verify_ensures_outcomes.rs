use std::path::PathBuf;
use std::process::{Command, Output};

fn write_fixture(name: &str, source: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "mumei_ensures_outcomes_{}_{}_{}",
        std::process::id(),
        name,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).expect("create fixture directory");
    let path = dir.join(format!("{name}.mm"));
    std::fs::write(&path, source).expect("write fixture");
    (dir, path)
}

fn verify_json(
    name: &str,
    source: &str,
    extra_args: &[&str],
) -> (PathBuf, Output, serde_json::Value) {
    let (dir, fixture) = write_fixture(name, source);
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg(&fixture)
        .arg("--json")
        .arg("--report-dir")
        .arg(&dir)
        .args(extra_args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run verify --json");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json_start = stdout
        .find('{')
        .unwrap_or_else(|| panic!("no JSON in stdout:\n{stdout}"));
    let payload = serde_json::from_str(&stdout[json_start..])
        .unwrap_or_else(|err| panic!("{err}: invalid verify JSON:\n{stdout}"));
    (dir, output, payload)
}

#[test]
fn proved_ensures_report_reachable_context_and_outcome() {
    let (dir, output, report) = verify_json(
        "proved",
        r#"
atom inc(x: i64) -> i64
requires: x < 100;
ensures: result > x;
body: { x + 1 };
"#,
        &[],
    );
    std::fs::remove_dir_all(dir).expect("remove fixture directory");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(report["status"], "success");
    assert_eq!(report["context_reachability"], "reachable");
    assert_eq!(report["ensures_outcomes"][0]["clause"], "result > x");
    assert_eq!(report["ensures_outcomes"][0]["outcome"], "proved");
}

#[test]
fn always_false_ensures_report_explains_the_failure() {
    let (dir, output, report) = verify_json(
        "always_false",
        r#"
atom inc(x: i64) -> i64
requires: x < 100;
ensures: result < x;
body: { x + 1 };
"#,
        &[],
    );
    std::fs::remove_dir_all(dir).expect("remove fixture directory");

    assert!(!output.status.success());
    assert_eq!(report["status"], "failed");
    assert_eq!(report["context_reachability"], "reachable");
    assert_eq!(report["ensures_outcomes"][0]["outcome"], "always_false");
    assert!(report["reason"]
        .as_str()
        .unwrap()
        .contains("false for every input that satisfies requires"));
}

#[test]
fn partially_failing_ensures_are_classified() {
    let (dir, output, report) = verify_json(
        "some_inputs",
        r#"
atom inc(x: i64) -> i64
requires: x >= 0;
ensures: result > 10;
body: { x + 1 };
"#,
        &[],
    );
    std::fs::remove_dir_all(dir).expect("remove fixture directory");

    assert!(!output.status.success());
    assert_eq!(report["status"], "failed");
    assert_eq!(
        report["ensures_outcomes"][0]["outcome"],
        "fails_on_some_inputs"
    );
    assert!(report["reason"]
        .as_str()
        .unwrap()
        .contains("holds for some inputs but not all"));
}

const VACUOUS_CONTEXT_SOURCE: &str = r#"
trusted atom conditional(x: i64) -> i64
requires: true;
ensures: (x != 5 || result == 0) && (x != 5 || result == 1);
body: 0;

atom caller(x: i64) -> i64
requires: x == 5;
ensures: result >= 0;
body: conditional(x);
"#;

#[test]
fn unreachable_context_warns_and_classifies_ensures_as_vacuous() {
    let (dir, output, report) = verify_json("vacuous_default", VACUOUS_CONTEXT_SOURCE, &[]);
    std::fs::remove_dir_all(dir).expect("remove fixture directory");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(report["status"], "success");
    assert_eq!(report["context_reachability"], "unreachable");
    assert_eq!(report["ensures_outcomes"][0]["outcome"], "vacuous");
    assert!(report["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|diagnostic| diagnostic["message"]
            .as_str()
            .is_some_and(|message| message.contains("vacuous verification context"))));
    assert!(report["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning["code"] == "vacuous_context" && warning["atom"] == "caller"));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("warning: vacuous verification context")
    );
}

#[test]
fn fail_on_vacuous_rejects_unreachable_context() {
    let (dir, output, report) = verify_json(
        "vacuous_failure",
        VACUOUS_CONTEXT_SOURCE,
        &["--fail-on-vacuous"],
    );
    let visualizer_report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("report.json")).expect("read visualizer report"),
    )
    .expect("parse visualizer report");
    std::fs::remove_dir_all(dir).expect("remove fixture directory");

    assert!(!output.status.success());
    assert_eq!(report["status"], "failed");
    assert_eq!(visualizer_report["failure_type"], "vacuous_context");
    assert_eq!(visualizer_report["context_reachability"], "unreachable");
    assert_eq!(
        visualizer_report["ensures_outcomes"][0]["outcome"],
        "vacuous"
    );
}
