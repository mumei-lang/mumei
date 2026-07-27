//! Regression coverage for `mumei verify --json` diagnostics on paths that were
//! previously unreachable or crashed the CLI.

use std::path::PathBuf;
use std::process::Command;

fn write_fixture(name: &str, source: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mumei_json_diag_{}_{}_{}",
        std::process::id(),
        name,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).expect("create fixture dir");
    let path = dir.join(format!("{name}.mm"));
    std::fs::write(&path, source).expect("write fixture");
    path
}

fn verify_json(
    fixture: &PathBuf,
    extra_args: &[&str],
) -> (std::process::Output, serde_json::Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg(fixture)
        .arg("--json")
        .args(extra_args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run verify --json");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let json_start = stdout
        .find('{')
        .unwrap_or_else(|| panic!("no JSON in stdout:\n{stdout}"));
    let payload: serde_json::Value = serde_json::from_str(&stdout[json_start..])
        .unwrap_or_else(|err| panic!("{err}: verify --json must emit valid JSON:\n{stdout}"));
    (output, payload)
}

/// A pattern candidate that does not mention the quantified variable (here
/// `arr[j]`, bound by the inner `exists`) made Z3 return a null AST, which the
/// z3 crate turned into a panic.
#[test]
fn nested_forall_exists_over_arrays_does_not_crash() {
    let fixture = write_fixture(
        "nested_quantifiers",
        r#"
atom nested_q(arr: [i64], n: i64)
requires: n >= 0 && forall(i, 0, n, exists(j, 0, n, arr[i] * arr[j] >= 0));
ensures: result >= 0;
body: n;
"#,
    );
    let (output, _payload) = verify_json(&fixture, &[]);
    std::fs::remove_dir_all(fixture.parent().unwrap()).expect("remove fixture dir");
    assert_ne!(
        output.status.code(),
        Some(101),
        "nested quantifiers must not panic\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn escalation_candidate_diagnostic_carries_reason() {
    let fixture = write_fixture(
        "escalation_candidate",
        r#"
atom hard_nonlinear(a: i64, b: i64, c: i64, n: i64)
requires: a > 0 && b > 0 && c > 0 && n > 2 && n < 10;
ensures: result >= 0 && a * a * a + b * b * b * c != c * c * c * a * b + n * n * n * a;
body: a + b + c;
"#,
    );
    let (_output, payload) = verify_json(&fixture, &["--solver-timeout", "200", "--escalate-lean"]);
    std::fs::remove_dir_all(fixture.parent().unwrap()).expect("remove fixture dir");

    let diagnostics = payload["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    let candidate = diagnostics
        .iter()
        .find(|d| d["code"] == "escalation_candidate")
        .unwrap_or_else(|| panic!("no escalation_candidate diagnostic in:\n{payload:#}"));
    assert_eq!(candidate["atom"], "hard_nonlinear");
    assert_eq!(candidate["severity"], "warning");
    assert!(
        candidate["escalation_reason"].is_string(),
        "escalation candidates must carry a reason: {payload:#}"
    );
    assert!(
        candidate["tags"]
            .as_array()
            .is_some_and(|tags| tags.iter().any(|tag| tag == "z3_unknown")),
        "escalation candidate must be tagged with the z3 result: {payload:#}"
    );
}
