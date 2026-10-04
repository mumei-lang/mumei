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

struct CaseResult {
    name: &'static str,
    target: &'static str,
    output: Output,
    report: Option<Value>,
}

impl CaseResult {
    fn verdict(&self) -> Option<&str> {
        let report = self.report.as_ref()?;
        if report["atom"].as_str() != Some(self.target) {
            return None;
        }
        match report["status"].as_str()? {
            "success" => Some("verified"),
            "unverifiable" => Some("unknown"),
            "failed" => Some("failed"),
            _ => None,
        }
    }

    fn did_not_crash(&self) -> bool {
        self.output.status.code().is_some_and(|code| code != 134)
            && !String::from_utf8_lossy(&self.output.stderr).contains("overflowed its stack")
    }
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
        .arg("--report-dir")
        .arg(&dir)
        .current_dir(&dir)
        .output()
        .expect("run verify");
    let report = std::fs::read_to_string(dir.join("report.json"))
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok());
    let _ = std::fs::remove_dir_all(dir);
    CaseResult {
        name,
        target,
        output,
        report,
    }
}

fn report_cases(cases: &[CaseResult]) {
    for case in cases {
        let stdout = String::from_utf8_lossy(&case.output.stdout);
        let stderr = String::from_utf8_lossy(&case.output.stderr);
        eprintln!(
            "{} target={} verdict={:?} status={:?} overflow_stdout={} overflow_stderr={} stdout={:?} stderr={:?}",
            case.name,
            case.target,
            case.verdict(),
            case.output.status,
            stdout.contains("overflowed its stack"),
            stderr.contains("overflowed its stack"),
            stdout.chars().take(250).collect::<String>(),
            stderr.chars().take(250).collect::<String>()
        );
    }
}

fn assert_verdict(case: &CaseResult, expected: &str) {
    assert_eq!(
        case.verdict(),
        Some(expected),
        "{} should be {expected}; stdout:\n{}\nstderr:\n{}",
        case.name,
        String::from_utf8_lossy(&case.output.stdout),
        String::from_utf8_lossy(&case.output.stderr)
    );
}

#[test]
fn recursive_contract_calls_are_finite_and_fail_closed() {
    let cases = vec![
        verify("tri", TRI, "tri"),
        verify(
            "use_ok",
            &format!(
                "{TRI}\natom use_ok(n: i64)\n    requires: n >= 1;\n    ensures: result >= 0;\n    body: tri(n);\n"
            ),
            "use_ok",
        ),
        verify(
            "use_bad",
            &format!(
                "{TRI}\natom use_bad()\n    requires: true;\n    ensures: result == 7;\n    body: tri(3);\n"
            ),
            "use_bad",
        ),
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
        verify(
            "use_ev",
            &format!(
                "{MUTUAL}\natom use_ev(n: i64)\n    requires: n >= 0;\n    ensures: result >= 0;\n    body: ev(n);\n"
            ),
            "use_ev",
        ),
        verify(
            "use_ev_bad",
            &format!(
                "{MUTUAL}\natom use_ev_bad()\n    requires: true;\n    ensures: result == 5;\n    body: ev(2);\n"
            ),
            "use_ev_bad",
        ),
        verify(
            "use_g",
            &format!(
                "{G}\natom use_g(n: i64)\n    requires: n == 0;\n    ensures: result >= 0;\n    body: g(n);\n"
            ),
            "use_g",
        ),
        verify(
            "use_g_bad",
            &format!(
                "{G}\natom use_g_bad()\n    requires: true;\n    ensures: result == 7;\n    body: g(0);\n"
            ),
            "use_g_bad",
        ),
        verify(
            "use_h",
            &format!(
                "{HK}\natom use_h(n: i64)\n    requires: n == 0;\n    ensures: result >= 0;\n    body: h(n);\n"
            ),
            "use_h",
        ),
        verify(
            "use_h_bad",
            &format!(
                "{HK}\natom use_h_bad()\n    requires: true;\n    ensures: result == 7;\n    body: h(0);\n"
            ),
            "use_h_bad",
        ),
        verify(
            "use_g2",
            &format!(
                "{G2}\natom use_g2(n: i64)\n    requires: n == 0;\n    ensures: result >= 0;\n    body: g2(n);\n"
            ),
            "use_g2",
        ),
        verify(
            "use_g2_bad",
            &format!(
                "{G2}\natom use_g2_bad()\n    requires: true;\n    ensures: result == 7;\n    body: g2(0);\n"
            ),
            "use_g2_bad",
        ),
    ];
    report_cases(&cases);

    let crashes: Vec<&str> = cases
        .iter()
        .filter(|case| !case.did_not_crash())
        .map(|case| case.name)
        .collect();
    assert!(crashes.is_empty(), "recursive cases crashed: {crashes:?}");

    assert_verdict(&cases[0], "failed");
    assert_verdict(&cases[1], "verified");
    assert_ne!(cases[2].verdict(), Some("verified"), "use_bad");
    assert_ne!(cases[3].verdict(), Some("verified"), "tri_bad");
    assert_verdict(&cases[4], "verified");
    assert_ne!(cases[5].verdict(), Some("verified"), "use_ev_bad");
    assert_verdict(&cases[6], "verified");
    assert_ne!(cases[7].verdict(), Some("verified"), "use_g_bad");
    assert_verdict(&cases[8], "verified");
    assert_ne!(cases[9].verdict(), Some("verified"), "use_h_bad");
    assert_verdict(&cases[10], "verified");
    assert_ne!(cases[11].verdict(), Some("verified"), "use_g2_bad");
}
