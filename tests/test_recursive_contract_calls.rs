use serde_json::Value;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const TRI: &str = r#"
atom tri(n: i64)
    requires: n >= 0;
    ensures: result >= 0 && (n == 0 || result == n + tri(n - 1));
    body: if n == 0 { 0 } else { n + tri(n - 1) };
"#;

const MUTUAL: &str = r#"
atom ev(n: i64)
    requires: n >= 0;
    ensures: result >= 0 && (n == 0 || result == od(n - 1));
    body: if n == 0 { 0 } else { od(n - 1) };

atom od(n: i64)
    requires: n >= 0;
    ensures: result >= 0 && (n == 0 || result == ev(n - 1));
    body: if n == 0 { 0 } else { ev(n - 1) };
"#;

const G: &str = r#"
atom g(n: i64)
    requires: n >= 0 && (n == 0 || g(n - 1) >= 0);
    ensures: result >= 0;
    body: n;
"#;

const HK: &str = r#"
atom h(n: i64)
    requires: n >= 0 && (n == 0 || k(n - 1) >= 0);
    ensures: result >= 0;
    body: n;

atom k(n: i64)
    requires: n >= 0;
    ensures: result >= 0 && (n == 0 || h(n - 1) >= 0);
    body: n;
"#;

const G2: &str = r#"
atom g2(n: i64)
    requires: n >= 0 && (n == 0 || call(atom_ref(g2), n - 1) >= 0);
    ensures: result >= 0;
    body: n;
"#;

const STRUCT_P: &str = r#"
struct P {
    x: i64,
    invariant: self.x >= 0 && mk(0).x >= 0
}
atom mk(n: i64) -> P
requires: n >= 0;
ensures: result.x == n;
body: P { x: n };
"#;

const QUANTIFIED_RECURSIVE_REQUIRES: &str = r#"
atom q(n: i64)
    requires: n >= 0 && forall(i, 0, n, q(i) >= 0);
    ensures: result >= 0;
    body: n;
"#;

struct CaseResult {
    name: &'static str,
    target: &'static str,
    output: Output,
    payload: Option<Value>,
    report: Option<Value>,
}

impl CaseResult {
    fn verdict(&self) -> Option<String> {
        if let Some(diagnostics) = self
            .payload
            .as_ref()
            .and_then(|payload| payload["diagnostics"].as_array())
        {
            if let Some(diagnostic) = diagnostics
                .iter()
                .rev()
                .find(|diagnostic| diagnostic["atom"].as_str() == Some(self.target))
            {
                let code = diagnostic["code"].as_str()?;
                return Some(
                    match code {
                        "failed" => "failed",
                        "unverifiable" | "unknown" => "unknown",
                        other => other,
                    }
                    .to_string(),
                );
            }
        }

        let report = self.report.as_ref()?;
        (report["atom"].as_str() == Some(self.target)
            && report["status"].as_str() == Some("success"))
        .then(|| "verified".to_string())
    }

    fn did_not_crash(&self) -> bool {
        // 0 verified, 1 rejected, 3 inconclusive (docs/CLI.md); anything else,
        // including signal termination, is not a verdict.
        let stderr = String::from_utf8_lossy(&self.output.stderr);
        matches!(self.output.status.code(), Some(0 | 1 | 3))
            && !stderr.contains("overflowed its stack")
            && !stderr.contains("panicked")
    }
}

fn parse_json_output(output: &Output) -> Option<Value> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json_start = stdout.find('{')?;
    serde_json::from_str(&stdout[json_start..]).ok()
}

fn verify(name: &'static str, source: &str, target: &'static str) -> CaseResult {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "mumei_recursive_contract_{name}_{}_{}",
        std::process::id(),
        nonce
    ));
    std::fs::create_dir_all(&dir).expect("create fixture directory");
    let fixture = dir.join(format!("{name}.mm"));
    std::fs::write(&fixture, source).expect("write fixture");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg(&fixture)
        .arg("--json")
        .arg("--report-dir")
        .arg(&dir)
        .current_dir(&dir)
        .output()
        .expect("run verify");
    let payload = parse_json_output(&output);
    let report = std::fs::read_to_string(dir.join("report.json"))
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok());
    let _ = std::fs::remove_dir_all(dir);
    CaseResult {
        name,
        target,
        output,
        payload,
        report,
    }
}

fn assert_verdict(case: &CaseResult, expected: &str) {
    assert_eq!(
        case.verdict().as_deref(),
        Some(expected),
        "{} should be {expected}; stdout:\n{}\nstderr:\n{}",
        case.name,
        String::from_utf8_lossy(&case.output.stdout),
        String::from_utf8_lossy(&case.output.stderr)
    );
}

fn assert_case(case: CaseResult, expected: &str) {
    assert!(
        case.did_not_crash(),
        "{} crashed; status={:?}\nstdout:\n{}\nstderr:\n{}",
        case.name,
        case.output.status,
        String::from_utf8_lossy(&case.output.stdout),
        String::from_utf8_lossy(&case.output.stderr)
    );
    assert_verdict(&case, expected);
}

#[test]
fn tri_does_not_crash_and_is_not_proved() {
    assert_case(verify("tri", TRI, "tri"), "failed");
}

#[test]
fn use_ok_verifies() {
    assert_case(
        verify(
            "use_ok",
            &format!(
                "{TRI}\natom use_ok(n: i64)\n    requires: n >= 1;\n    ensures: result >= 0;\n    body: tri(n);\n"
            ),
            "use_ok",
        ),
        "verified",
    );
}

#[test]
fn use_bad_fails() {
    assert_case(
        verify(
            "use_bad",
            &format!(
                "{TRI}\natom use_bad()\n    requires: true;\n    ensures: result == 7;\n    body: tri(3);\n"
            ),
            "use_bad",
        ),
        "failed",
    );
}

#[test]
fn tri_bad_fails() {
    assert_case(
        verify(
            "tri_bad",
            r#"
atom tri_bad(n: i64)
    requires: n >= 0;
    ensures: result >= 1 && (n == 0 || result == n + tri_bad(n - 1));
    body: if n == 0 { 0 } else { n + tri_bad(n - 1) };
"#,
            "tri_bad",
        ),
        "failed",
    );
}

#[test]
fn use_ev_verifies() {
    assert_case(
        verify(
            "use_ev",
            &format!(
                "{MUTUAL}\natom use_ev(n: i64)\n    requires: n >= 0;\n    ensures: result >= 0;\n    body: ev(n);\n"
            ),
            "use_ev",
        ),
        "verified",
    );
}

#[test]
fn use_ev_bad_fails() {
    assert_case(
        verify(
            "use_ev_bad",
            &format!(
                "{MUTUAL}\natom use_ev_bad()\n    requires: true;\n    ensures: result == 5;\n    body: ev(2);\n"
            ),
            "use_ev_bad",
        ),
        "failed",
    );
}

#[test]
fn use_g_verifies() {
    assert_case(
        verify(
            "use_g",
            &format!(
                "{G}\natom use_g(n: i64)\n    requires: n == 0;\n    ensures: result >= 0;\n    body: g(n);\n"
            ),
            "use_g",
        ),
        "verified",
    );
}

#[test]
fn use_g_bad_fails() {
    assert_case(
        verify(
            "use_g_bad",
            &format!(
                "{G}\natom use_g_bad()\n    requires: true;\n    ensures: result == 7;\n    body: g(0);\n"
            ),
            "use_g_bad",
        ),
        "failed",
    );
}

#[test]
fn use_h_verifies() {
    assert_case(
        verify(
            "use_h",
            &format!(
                "{HK}\natom use_h(n: i64)\n    requires: n == 0;\n    ensures: result >= 0;\n    body: h(n);\n"
            ),
            "use_h",
        ),
        "verified",
    );
}

#[test]
fn use_h_bad_fails() {
    assert_case(
        verify(
            "use_h_bad",
            &format!(
                "{HK}\natom use_h_bad()\n    requires: true;\n    ensures: result == 7;\n    body: h(0);\n"
            ),
            "use_h_bad",
        ),
        "failed",
    );
}

#[test]
fn use_g2_verifies() {
    assert_case(
        verify(
            "use_g2",
            &format!(
                "{G2}\natom use_g2(n: i64)\n    requires: n == 0;\n    ensures: result >= 0;\n    body: g2(n);\n"
            ),
            "use_g2",
        ),
        "verified",
    );
}

#[test]
fn use_g2_bad_fails() {
    assert_case(
        verify(
            "use_g2_bad",
            &format!(
                "{G2}\natom use_g2_bad()\n    requires: true;\n    ensures: result == 7;\n    body: g2(0);\n"
            ),
            "use_g2_bad",
        ),
        "failed",
    );
}

#[test]
fn mk_struct_result_verifies_without_crashing() {
    assert_case(verify("mk", STRUCT_P, "mk"), "verified");
}

#[test]
fn use_mk_verifies() {
    assert_case(
        verify(
            "use_mk",
            &format!(
                "{STRUCT_P}\natom use_mk(n: i64) -> P\nrequires: n >= 0;\nensures: result.x == n;\nbody: mk(n);\n"
            ),
            "use_mk",
        ),
        "verified",
    );
}

#[test]
fn use_mk_bad_fails() {
    assert_case(
        verify(
            "use_mk_bad",
            &format!(
                "{STRUCT_P}\natom use_mk_bad(n: i64) -> P\nrequires: n >= 0;\nensures: result.x == n + 1;\nbody: mk(n);\n"
            ),
            "use_mk_bad",
        ),
        "failed",
    );
}

#[test]
fn quantified_recursive_requires_and_caller_fail_closed_without_crashing() {
    let source = format!(
        "{QUANTIFIED_RECURSIVE_REQUIRES}\natom use_q(n: i64)\n    requires: n >= 0;\n    ensures: result >= 0;\n    body: q(n);\n"
    );
    assert_case(verify("q_quantified_requires", &source, "q"), "unknown");
    assert_case(
        verify("use_q_quantified_requires", &source, "use_q"),
        "unknown",
    );
}
