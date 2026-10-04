//! Atom-level `decreases:` clauses and congruent recursive contracts.
//!
//! Mirrors tests/test_recursive_contract_calls.rs: each case is verified in a
//! fresh temp dir with `--json`, the verdict is read from diagnostics /
//! report.json, and crashes are asserted away.

use serde_json::Value;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const TRI_DEC: &str = r#"
atom tri(n: i64)
    requires: n >= 0;
    ensures: result >= 0 && (n == 0 || result == n + tri(n - 1));
    decreases: n;
    body: if n == 0 { 0 } else { n + tri(n - 1) };
"#;

const TRI_NODEC: &str = r#"
atom tri(n: i64)
    requires: n >= 0;
    ensures: result >= 0 && (n == 0 || result == n + tri(n - 1));
    body: if n == 0 { 0 } else { n + tri(n - 1) };
"#;

const TRI_WRONG: &str = r#"
atom tri(n: i64)
    requires: n >= 0;
    ensures: result >= 0 && (n == 0 || result == n + tri(n - 1) + 1);
    decreases: n;
    body: if n == 0 { 0 } else { n + tri(n - 1) };
"#;

const BAD_SELF: &str = r#"
atom bad(x: i64)
    decreases: x;
    ensures: result == bad(x) + 1;
    body: x;
"#;

const NO_DECREASE_CALL: &str = r#"
atom f(n: i64)
    requires: n >= 0;
    decreases: n;
    ensures: result >= 0;
    body: if n == 0 { 0 } else { f(n) };
"#;

const GROWING_CALL: &str = r#"
atom f(n: i64)
    requires: n >= 0;
    decreases: n;
    ensures: result >= 0;
    body: if n == 0 { 0 } else { f(n + 1) };
"#;

const NEGATIVE_MEASURE: &str = r#"
atom neg(n: i64)
    requires: n <= 100;
    decreases: n;
    ensures: result >= 0;
    body: if n == 0 { 0 } else { neg(n - 1) };
"#;

const MUTUAL_DEC: &str = r#"
atom is_even(n: i64) -> bool
    requires: n >= 0;
    decreases: n;
    ensures: (n == 0 && result) || (n > 0 && result == is_odd(n - 1));
    body: if n == 0 { true } else { is_odd(n - 1) };

atom is_odd(n: i64) -> bool
    requires: n >= 0;
    decreases: n;
    ensures: (n == 0 && !result) || (n > 0 && result == is_even(n - 1));
    body: if n == 0 { false } else { is_even(n - 1) };
"#;

const MUTUAL_DEC_WRONG: &str = r#"
atom is_even(n: i64) -> bool
    requires: n >= 0;
    decreases: n;
    ensures: (n == 0 && result) || (n > 0 && result == is_odd(n - 1));
    body: if n == 0 { true } else { is_odd(n - 1) };

atom is_odd(n: i64) -> bool
    requires: n >= 0;
    decreases: n;
    ensures: (n == 0 && result) || (n > 0 && result == is_even(n - 1));
    body: if n == 0 { false } else { is_even(n - 1) };
"#;

const USE_TRI: &str = r#"
atom tri(n: i64)
    requires: n >= 0;
    ensures: result >= 0 && (n == 0 || result == n + tri(n - 1));
    decreases: n;
    body: if n == 0 { 0 } else { n + tri(n - 1) };

atom use_tri(n: i64)
    requires: n >= 1;
    ensures: result == n + tri(n - 1);
    body: tri(n);
"#;

const TRI_CONST: &str = r#"
atom tri(n: i64)
    requires: n >= 0;
    ensures: result >= 0 && (n == 0 || result == n + tri(n - 1));
    decreases: n;
    body: if n == 0 { 0 } else { n + tri(n - 1) };

atom tri3()
    ensures: result == 6;
    body: tri(3);
"#;

const EFFECT_REC: &str = r#"
effect Tick;

atom tick(n: i64)
    requires: n >= 0;
    effects: [Tick];
    decreases: n;
    ensures: result >= 0 && (n == 0 || result == tick(n - 1));
    body: if n == 0 { perform Tick.tick(); 0 } else { perform Tick.tick(); tick(n - 1) };
"#;

const EFFECT_REC_NODEC: &str = r#"
effect Tick;

atom tick(n: i64)
    requires: n >= 0;
    effects: [Tick];
    ensures: result >= 0 && (n == 0 || result == tick(n - 1));
    body: if n == 0 { perform Tick.tick(); 0 } else { perform Tick.tick(); tick(n - 1) };
"#;

struct CaseResult {
    name: &'static str,
    target: &'static str,
    output: Output,
    payload: Option<Value>,
    report: Option<Value>,
}

/// Codes that carry the actual verdict. Advisory diagnostics (e.g. the
/// `recursive_contract_*` hints) precede the failure entry, so the scan must
/// skip any code that is not a verdict code.
fn is_verdict_code(code: &str) -> bool {
    matches!(code, "failed" | "unverifiable" | "unknown")
}

impl CaseResult {
    fn verdict(&self) -> Option<String> {
        if let Some(diagnostics) = self
            .payload
            .as_ref()
            .and_then(|payload| payload["diagnostics"].as_array())
        {
            if let Some(diagnostic) = diagnostics.iter().rev().find(|diagnostic| {
                diagnostic["atom"].as_str() == Some(self.target)
                    && diagnostic["code"]
                        .as_str()
                        .map(is_verdict_code)
                        .unwrap_or(false)
            }) {
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
        self.output.status.code().is_some_and(|code| code != 134)
            && !String::from_utf8_lossy(&self.output.stderr).contains("overflowed its stack")
    }

    fn has_diagnostic_code(&self, code: &str) -> bool {
        self.payload
            .as_ref()
            .and_then(|payload| payload["diagnostics"].as_array())
            .map(|diagnostics| {
                diagnostics.iter().any(|diagnostic| {
                    diagnostic["code"].as_str() == Some(code)
                        && diagnostic["atom"].as_str() == Some(self.target)
                })
            })
            .unwrap_or(false)
    }

    fn failure_type(&self) -> Option<String> {
        self.report
            .as_ref()
            .and_then(|report| report["failure_type"].as_str().map(str::to_string))
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
        "mumei_recursive_decreases_{name}_{}_{}",
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
        "{} crashed; stderr:\n{}",
        case.name,
        String::from_utf8_lossy(&case.output.stderr)
    );
    assert_verdict(&case, expected);
}

fn assert_termination(case: &CaseResult, param: &str) {
    assert!(
        case.did_not_crash(),
        "{} crashed; stderr:\n{}",
        case.name,
        String::from_utf8_lossy(&case.output.stderr)
    );
    assert_verdict(case, "failed");
    assert_eq!(
        case.failure_type().as_deref(),
        Some("termination_measure_violation"),
        "{} should report termination_measure_violation; report:\n{:?}",
        case.name,
        case.report
    );
    assert_eq!(
        case.output.status.code(),
        Some(1),
        "{} should exit 1",
        case.name
    );
    let counterexample = case
        .report
        .as_ref()
        .and_then(|report| report["counterexample"].as_object());
    assert!(
        counterexample
            .map(|ce| ce.contains_key(param))
            .unwrap_or(false),
        "{} should report a counterexample binding '{param}'; report:\n{:?}",
        case.name,
        case.report
    );
}

#[test]
fn test_tri_with_decreases_verifies() {
    let case = verify("tri_dec", TRI_DEC, "tri");
    assert_case(case, "verified");
}

#[test]
fn test_tri_without_decreases_fails_with_hint() {
    let case = verify("tri_nodec", TRI_NODEC, "tri");
    assert!(case.did_not_crash());
    assert_verdict(&case, "failed");
    assert!(
        case.has_diagnostic_code("recursive_contract_needs_decreases"),
        "expected recursive_contract_needs_decreases hint; stdout:\n{}",
        String::from_utf8_lossy(&case.output.stdout)
    );
}

#[test]
fn test_wrong_recurrence_with_decreases_fails() {
    let case = verify("tri_wrong", TRI_WRONG, "tri");
    assert!(case.did_not_crash());
    assert_verdict(&case, "failed");
    assert_ne!(
        case.failure_type().as_deref(),
        Some("termination_measure_violation"),
        "wrong recurrence is a postcondition failure, not termination"
    );
}

#[test]
fn test_self_call_in_ensures_fails_termination() {
    let case = verify("bad_self", BAD_SELF, "bad");
    assert_termination(&case, "x");
}

#[test]
fn test_call_without_decreasing_argument_fails_termination() {
    let case = verify("no_decrease", NO_DECREASE_CALL, "f");
    assert_termination(&case, "n");
}

#[test]
fn test_growing_call_argument_fails_termination() {
    let case = verify("growing", GROWING_CALL, "f");
    assert_termination(&case, "n");
}

#[test]
fn test_negative_measure_fails_termination() {
    let case = verify("negative", NEGATIVE_MEASURE, "neg");
    assert_termination(&case, "n");
}

#[test]
fn test_mutual_recursion_with_decreases_verifies() {
    // report.json/diagnostics describe only the last atom, so assert the
    // whole-file result: a successful status means both members verified.
    let case = verify("mutual_dec", MUTUAL_DEC, "is_odd");
    assert!(
        case.did_not_crash(),
        "mutual_dec crashed; stderr:\n{}",
        String::from_utf8_lossy(&case.output.stderr)
    );
    let status = case
        .payload
        .as_ref()
        .and_then(|payload| payload["status"].as_str().map(str::to_string))
        .or_else(|| {
            case.report
                .as_ref()
                .and_then(|report| report["status"].as_str().map(str::to_string))
        });
    assert_eq!(
        status.as_deref(),
        Some("success"),
        "mutual SCC should verify; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&case.output.stdout),
        String::from_utf8_lossy(&case.output.stderr)
    );
}

#[test]
fn test_mutual_recursion_wrong_spec_fails() {
    let case = verify("mutual_wrong", MUTUAL_DEC_WRONG, "is_odd");
    assert!(case.did_not_crash());
    assert_verdict(&case, "failed");
}

#[test]
fn test_caller_of_decreased_atom_verifies() {
    let case = verify("use_tri", USE_TRI, "use_tri");
    assert_case(case, "verified");
}

#[test]
fn test_constant_unfolding_does_not_crash() {
    // Only one unfolding is instantiated for a concrete argument, so this is
    // expected to be unprovable; the meaningful assertion is no crash.
    let case = verify("tri3", TRI_CONST, "tri3");
    assert!(case.did_not_crash());
    eprintln!(
        "tri3 observed verdict: {:?}",
        case.verdict().as_deref().unwrap_or("<none>")
    );
}

#[test]
fn test_effectful_recursion_is_unsupported_hint() {
    let dec = verify("effect_dec", EFFECT_REC, "tick");
    let nodec = verify("effect_nodec", EFFECT_REC_NODEC, "tick");
    assert!(dec.did_not_crash() && nodec.did_not_crash());
    assert!(
        dec.has_diagnostic_code("recursive_contract_unsupported"),
        "expected recursive_contract_unsupported hint; stdout:\n{}",
        String::from_utf8_lossy(&dec.output.stdout)
    );
    assert_eq!(
        dec.verdict(),
        nodec.verdict(),
        "decreases on an unsupported SCC must not change the verdict"
    );
}
