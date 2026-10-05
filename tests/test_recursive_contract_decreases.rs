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

const ARRAY_COUNT: &str = r#"
atom count(a: [i64], i: i64) -> i64
    requires: 0 <= i && i <= len(a);
    ensures: result == len(a) - i;
    decreases: len(a) - i;
    body: if i == len(a) { 0 } else { 1 + count(a, i + 1) };

atom count_len_caller(a: [i64], b: [i64]) -> i64
    requires: len(a) == 3 && len(b) == 4;
    ensures: count(a, 0) == len(a) && count(b, 0) == len(b);
    body: 0;
"#;

const ARRAY_COUNT_EQUAL_LENGTHS: &str = r#"
atom count(a: [i64], i: i64) -> i64
    requires: 0 <= i && i <= len(a);
    ensures: result == len(a) - i;
    decreases: len(a) - i;
    body: if i == len(a) { 0 } else { 1 + count(a, i + 1) };

atom count_len_difference(a: [i64], b: [i64]) -> i64
    requires: len(a) == 3 && len(b) == 4;
    ensures: count(a, 0) == count(b, 0);
    body: 0;
"#;

const ARRAY_COUNT_NONDECREASING: &str = r#"
atom count(a: [i64], i: i64) -> i64
    requires: 0 <= i && i <= len(a);
    ensures: result >= 0;
    decreases: len(a) - i;
    body: if i == 0 { 0 } else { count(a, i - 1) };
"#;

const STRUCT_DOWN: &str = r#"
struct P {
    n: i64 where v >= 0
}

atom down(p: P) -> i64
    requires: true;
    ensures: result == p.n;
    decreases: p.n;
    body: if p.n == 0 { 0 } else { 1 + down(P { n: p.n - 1 }) };

atom down_caller() -> i64
    requires: true;
    ensures: down(P { n: 3 }) == 3;
    body: 0;
"#;

const STRUCT_DOWN_INVARIANT: &str = r#"
struct P {
    n: i64,
    invariant: self.n >= 0
}

atom down_inv(p: P) -> i64
    requires: true;
    ensures: result == p.n;
    decreases: p.n;
    body: if p.n == 0 { 0 } else { 1 + down_inv(P { n: p.n - 1 }) };

atom use_down_inv(p: P) -> i64
    requires: true;
    ensures: down_inv(p) >= 0;
    body: 0;
"#;

const UNSUPPORTED_REF_MUT_ARRAY: &str = r#"
atom bad(ref mut a: [i64]) -> i64
    requires: len(a) >= 0;
    ensures: result == bad(a) + 1;
    decreases: len(a);
    body: if len(a) == 0 { 0 } else { bad(a) };
"#;

const UNSUPPORTED_REF_MUT_STRUCT: &str = r#"
struct P { n: i64 }

atom bad(ref mut p: P) -> i64
    requires: true;
    ensures: result == bad(p) + 1;
    decreases: p.n;
    body: if p.n == 0 { 0 } else { bad(p) };
"#;

const UNSUPPORTED_CONSUME_ARRAY: &str = r#"
atom bad(a: [i64]) -> i64
    requires: bad(a) >= 0;
    ensures: result == 1;
    consume a;
    decreases: 0;
    body: 0;
"#;

const UNSUPPORTED_CONSUME_STRUCT: &str = r#"
struct P { n: i64 }

atom bad(p: P) -> i64
    requires: bad(p) >= 0;
    ensures: result == 1;
    consume p;
    decreases: 0;
    body: 0;
"#;

const UNSUPPORTED_STORED_ARRAY: &str = r#"
atom bad(a: [i64]) -> i64
    requires: len(a) >= 1;
    ensures: result == bad(a) + 1;
    decreases: len(a);
    body: {
        a[0] = 0;
        bad(a)
    };
"#;

const UNSUPPORTED_FLOAT_ARRAY: &str = r#"
atom bad(a: [f64]) -> i64
    requires: len(a) >= 0;
    ensures: result == bad(a) + 1;
    decreases: len(a);
    body: 0;
"#;

const UNSUPPORTED_STRUCT_ARRAY_FIELD: &str = r#"
struct P { a: [i64] }

atom bad(p: P) -> i64
    requires: true;
    ensures: result == bad(p) + 1;
    decreases: 0;
    body: bad(p);
"#;

const UNSUPPORTED_STRUCT_CALL_INVARIANT: &str = r#"
atom helper() -> i64
    requires: true;
    ensures: true;
    body: 0;

struct P {
    n: i64,
    invariant: helper() >= 0
}

atom bad(p: P) -> i64
    requires: true;
    ensures: result == bad(p) + 1;
    decreases: p.n;
    body: if p.n == 0 { 0 } else { bad(p) };
"#;

const UNSUPPORTED_ARRAY_RETURN: &str = r#"
atom bad() -> [i64]
    requires: true;
    ensures: len(bad()) == -1;
    decreases: 0;
    body: bad();
"#;

const UNSUPPORTED_STRUCT_RETURN: &str = r#"
struct P { n: i64 }

atom read_n(p: P) -> i64
    requires: true;
    ensures: result == p.n;
    body: p.n;

atom bad() -> P
    requires: true;
    ensures: result.n == -1 && read_n(bad()) >= 0;
    decreases: 0;
    body: P { n: 0 };
"#;

const UNSUPPORTED_ARRAY_MEASURE_ACCESS: &str = r#"
atom bad(a: [i64]) -> i64
    requires: len(a) >= 1;
    ensures: result == bad(a) + 1;
    decreases: a[0];
    body: bad(a);
"#;

const UNSUPPORTED_ARRAY_DIRECT_MEASURE: &str = r#"
atom bad(a: [i64]) -> i64
    requires: len(a) >= 0;
    ensures: result == bad(a) + 1;
    decreases: a;
    body: bad(a);
"#;

const UNSUPPORTED_STRUCT_DIRECT_MEASURE: &str = r#"
struct P { n: i64 }

atom bad(p: P) -> i64
    requires: true;
    ensures: result == bad(p) + 1;
    decreases: p;
    body: bad(p);
"#;

const UNSUPPORTED_LEN_NON_ARRAY_MEASURE: &str = r#"
atom bad(n: i64) -> i64
    requires: n >= 0;
    ensures: result == bad(n) + 1;
    decreases: len(n);
    body: bad(n);
"#;

const ACKERMANN: &str = r#"
atom ack(m: i64, n: i64) -> i64
    requires: m >= 0 && n >= 0;
    ensures: result >= n + 1;
    decreases: (m, n);
    body: if m == 0 { n + 1 } else { if n == 0 { ack(m - 1, 1) } else { ack(m - 1, ack(m, n - 1)) } };
"#;

const LEXDOWN: &str = r#"
atom lexdown(m: i64, n: i64) -> i64
    requires: m >= 0 && n >= 0;
    ensures: result == 0;
    decreases: (m, n);
    body: if m == 0 && n == 0 { 0 } else { if n == 0 { lexdown(m - 1, m) } else { lexdown(m, n - 1) } };
"#;

const MUTUAL_LEX: &str = r#"
atom f(m: i64, n: i64) -> i64
    requires: m >= 0 && n >= 0;
    ensures: result >= 0;
    decreases: (m, n);
    body: if n == 0 { 0 } else { g(m, n - 1) };

atom g(m: i64, n: i64) -> i64
    requires: m >= 0 && n >= 0;
    ensures: result >= 0;
    decreases: (m, n);
    body: if m == 0 { 0 } else { f(m - 1, n) };
"#;

const MIXED_ARITY: &str = r#"
atom f(n: i64) -> i64
    requires: n >= 0;
    ensures: n == 0 || result == g(n - 1);
    decreases: (n, 0);
    body: if n == 0 { 0 } else { g(n - 1) };

atom g(n: i64) -> i64
    requires: n >= 0;
    ensures: n == 0 || result == f(n - 1);
    decreases: n;
    body: if n == 0 { 0 } else { f(n - 1) };
"#;

const MIXED_ARITY_NODEC: &str = r#"
atom f(n: i64) -> i64
    requires: n >= 0;
    ensures: n == 0 || result == g(n - 1);
    body: if n == 0 { 0 } else { g(n - 1) };

atom g(n: i64) -> i64
    requires: n >= 0;
    ensures: n == 0 || result == f(n - 1);
    body: if n == 0 { 0 } else { f(n - 1) };
"#;

const TUPLE_CALL_MEASURE: &str = r#"
atom f(n: i64) -> i64
    requires: n >= 0;
    ensures: result >= 0 && (n == 0 || result == f(n - 1));
    decreases: (f(n), n);
    body: if n == 0 { 0 } else { f(n - 1) };
"#;

const NESTED_TUPLE_MEASURE: &str = r#"
atom f(n: i64) -> i64
    requires: n >= 0;
    ensures: result >= 0 && (n == 0 || result == f(n - 1));
    decreases: (n, (n, 0));
    body: if n == 0 { 0 } else { f(n - 1) };
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
    ensures: result >= 0 && ((n == 0 && result == 0) || (n > 0 && result == n + tri(n - 1)));
    decreases: n;
    body: if n == 0 { 0 } else { n + tri(n - 1) };

atom tri3()
    ensures: result == 6;
    body: tri(3);
"#;

const NAT_REFINEMENT_REPRO: &str = r#"
type Nat = i64 where v >= 0;

atom f(n: Nat) -> i64
    ensures: (n == 0 && result == 0) || (n > 0 && result == 1 + f(n - 1));
    decreases: n;
    body: if n == 0 { 0 } else { 1 + f(n - 1) };

atom bad() -> i64
    ensures: result == 0;
    body: f(0);
"#;

const NAT_TRI_CONST: &str = r#"
type Nat = i64 where v >= 0;

atom tri(n: Nat) -> i64
    ensures: (n == 0 && result == 0) || (n > 0 && result == n + tri(n - 1));
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

// An eligible recursive atom whose top-level requires carries a scalar
// quantified conjunct. `caller_requires_obligation` (policy 6) appends the
// conjunct to the call-site obligation, so a decreasing call still has to
// establish the quantified precondition.
const QR_QUANTIFIED_REQUIRES: &str = r#"
atom qr(n: i64)
    requires: n >= 0 && forall(i, 0, n, i * i >= 0);
    ensures: result >= 0;
    decreases: n;
    body: if n == 0 { 0 } else { 1 + qr(n - 1) };
"#;

// Failing twin: `qr2` itself is a clean eligible recursive atom — its
// recursive call `qr2(n - 1, k)` decreases the measure and satisfies the
// requires (the caller's quantified conjunct implies the callee's). The
// outside caller `use_qr2` passes `k = 7`, so the scalar conjuncts hold
// (`5 >= 0`, `7 >= 0`) but the quantified conjunct instantiated at `k = 7`
// — `forall(i, 0, 7, i < 5)` — is false. The call therefore fails on the
// quantified requires conjunct alone, and no termination obligation applies
// to a call from outside the SCC. The `atom_ref` call site is used because
// its diagnostic echoes the requires text.
const QR_QUANTIFIED_REQUIRES_FAIL: &str = r#"
atom qr2(n: i64, k: i64)
    requires: n >= 0 && forall(i, 0, k, i < 5);
    ensures: result >= 0;
    decreases: n;
    body: if n == 0 { 0 } else { 1 + qr2(n - 1, k) };

atom use_qr2()
    requires: true;
    ensures: result >= 0;
    body: call(atom_ref(qr2), 5, 7);
"#;

// SCC-internal failing case: `qr3(n - 1, k + 1)` decreases `n` but widens the
// quantifier range, so at `k = 5` the callee requires `i = 5 < 5`.
const QR3_WIDENING: &str = r#"
atom qr3(n: i64, k: i64) -> i64
    requires: n >= 0 && k >= 0 && forall(i, 0, k, i < 5);
    ensures: result >= 0;
    decreases: n;
    body: if n == 0 { 0 } else { qr3(n - 1, k + 1) };
"#;

// Passing twin: the recursive call shrinks only `n`, so the callee's
// quantified requires is implied by the caller's.
const QR3_OK: &str = r#"
atom qr3_ok(n: i64, k: i64) -> i64
    requires: n >= 0 && k >= 0 && forall(i, 0, k, i < 5);
    ensures: result >= 0;
    decreases: n;
    body: if n == 0 { 0 } else { qr3_ok(n - 1, k) };
"#;

// Direct (non-atom_ref) call from outside the SCC into `qr2`: the scalar
// conjuncts hold (`5 >= 0`, `7 >= 0`), only the quantified conjunct fails.
const QR_DIRECT_CALLER: &str = r#"
atom qr2(n: i64, k: i64)
    requires: n >= 0 && forall(i, 0, k, i < 5);
    ensures: result >= 0;
    decreases: n;
    body: if n == 0 { 0 } else { 1 + qr2(n - 1, k) };

atom use_qr2_direct()
    requires: true;
    ensures: result >= 0;
    body: qr2(5, 7);
"#;

// A recursive call whose argument depends on a quantifier-bound variable is
// rejected before any `rec_fn#` application or termination obligation is
// built: the enclosing clause is unverifiable (exit 3).
const QF_CALL_UNDER_QUANTIFIER: &str = r#"
atom qf(n: i64)
    requires: n >= 0;
    ensures: forall(i, 0, n, qf(i) >= 0);
    decreases: n;
    body: 0;
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

    fn diagnostic_message_contains(&self, code: &str, text: &str) -> bool {
        self.payload
            .as_ref()
            .and_then(|payload| payload["diagnostics"].as_array())
            .is_some_and(|diagnostics| {
                diagnostics.iter().any(|diagnostic| {
                    diagnostic["code"].as_str() == Some(code)
                        && diagnostic["atom"].as_str() == Some(self.target)
                        && diagnostic["message"]
                            .as_str()
                            .is_some_and(|message| message.contains(text))
                })
            })
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
    verify_with_args(name, source, target, &[])
}

fn verify_with_args(
    name: &'static str,
    source: &str,
    target: &'static str,
    extra_args: &[&str],
) -> CaseResult {
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
        .args(extra_args)
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

fn assert_case_ref(case: &CaseResult, expected: &str) {
    assert!(
        case.did_not_crash(),
        "{} crashed; stderr:\n{}",
        case.name,
        String::from_utf8_lossy(&case.output.stderr)
    );
    assert_verdict(case, expected);
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
fn test_parenthesized_single_measure_verifies() {
    let source = TRI_DEC.replace("decreases: n;", "decreases: (n);");
    let case = verify("tri_parenthesized", &source, "tri");
    assert_case(case, "verified");
}

#[test]
fn test_match_measures_verify() {
    let single_match = TRI_DEC.replace("decreases: n;", "decreases: match n { 0 => 0, _ => n };");
    assert_case(
        verify("tri_match_measure", &single_match, "tri"),
        "verified",
    );

    let tuple_match = LEXDOWN.replace(
        "decreases: (m, n);",
        "decreases: (match m { 0 => 0, _ => m }, n);",
    );
    assert_case(
        verify("lexdown_match_component", &tuple_match, "lexdown"),
        "verified",
    );
}

#[test]
fn test_ackermann_with_lexicographic_decreases_verifies() {
    let case = verify("ackermann", ACKERMANN, "ack");
    assert_case(case, "verified");
}

#[test]
fn test_lexdown_with_lexicographic_decreases_verifies() {
    let case = verify("lexdown", LEXDOWN, "lexdown");
    assert_case(case, "verified");
}

#[test]
fn test_lexdown_with_lexicographic_decreases_verifies_in_bitvec_mode() {
    let case = verify_with_args("lexdown_bv", LEXDOWN, "lexdown", &["--bitvec-i64"]);
    assert_case(case, "verified");
}

#[test]
fn test_mutual_recursion_with_lexicographic_decreases_verifies() {
    let case = verify("mutual_lex", MUTUAL_LEX, "g");
    assert!(
        case.did_not_crash(),
        "mutual_lex crashed; stderr:\n{}",
        String::from_utf8_lossy(&case.output.stderr)
    );
    assert_eq!(
        case.payload
            .as_ref()
            .and_then(|payload| payload["status"].as_str()),
        Some("success"),
        "mutual lexicographic SCC should verify; stdout:\n{}",
        String::from_utf8_lossy(&case.output.stdout)
    );
}

#[test]
fn test_mixed_arity_mutual_recursion_is_unsupported() {
    let dec = verify("mixed_arity", MIXED_ARITY, "f");
    let nodec = verify("mixed_arity_nodec", MIXED_ARITY_NODEC, "f");
    assert!(dec.did_not_crash() && nodec.did_not_crash());
    assert!(
        dec.has_diagnostic_code("recursive_contract_unsupported"),
        "expected recursive_contract_unsupported hint; stdout:\n{}",
        String::from_utf8_lossy(&dec.output.stdout)
    );
    assert!(
        dec.diagnostic_message_contains("recursive_contract_unsupported", "different arity"),
        "expected a different-arity explanation; stdout:\n{}",
        String::from_utf8_lossy(&dec.output.stdout)
    );
    assert_eq!(
        dec.verdict(),
        nodec.verdict(),
        "mixed-arity decreases must follow the ineligible SCC path"
    );
}

#[test]
fn test_tuple_call_and_nested_tuple_measures_are_unsupported() {
    let with_measure = |measure: &str| {
        NESTED_TUPLE_MEASURE.replace("decreases: (n, (n, 0));", &format!("decreases: {measure};"))
    };
    let cases = [
        (
            "tuple_call_measure",
            TUPLE_CALL_MEASURE.to_string(),
            "must not contain calls",
        ),
        (
            "nested_tuple_measure",
            NESTED_TUPLE_MEASURE.to_string(),
            "must not nest tuples",
        ),
        (
            "nested_tuple_with_double_parentheses",
            with_measure("(n, ((n, 0)))"),
            "must not nest tuples",
        ),
        (
            "nested_tuple_with_arithmetic",
            with_measure("(n, (n, 0) + 1)"),
            "must not nest tuples",
        ),
        (
            "single_measure_with_tuple_arithmetic",
            with_measure("(n, 0) + 1"),
            "must not nest tuples",
        ),
        (
            "tuple_call_with_comma",
            with_measure("(f(n, n))"),
            "must not contain calls",
        ),
    ];
    for (name, source, expected_message) in cases {
        let case = verify(name, &source, "f");
        assert!(case.did_not_crash(), "{name} crashed");
        assert!(
            case.has_diagnostic_code("recursive_contract_unsupported"),
            "expected unsupported hint; stdout:\n{}",
            String::from_utf8_lossy(&case.output.stdout)
        );
        assert!(
            case.diagnostic_message_contains("recursive_contract_unsupported", expected_message),
            "expected {expected_message:?}; stdout:\n{}",
            String::from_utf8_lossy(&case.output.stdout)
        );
    }
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
fn test_lexicographic_call_that_increases_first_measure_fails_termination() {
    let source = r#"
atom f(m: i64, n: i64) -> i64
    requires: m >= 0 && n >= 1;
    ensures: result >= 0;
    decreases: (m, n);
    body: if m == 0 { 0 } else { f(m + 1, n - 1) };
"#;
    let case = verify("lex_growing_first", source, "f");
    assert_termination(&case, "m");
}

#[test]
fn test_lexicographic_equal_measures_fail_termination() {
    let source = r#"
atom f(m: i64, n: i64) -> i64
    requires: m >= 0 && n >= 0;
    ensures: result >= 0;
    decreases: (m, n);
    body: if m == 0 { 0 } else { f(m, n) };
"#;
    let case = verify("lex_equal", source, "f");
    assert_termination(&case, "m");
}

#[test]
fn test_negative_lexicographic_component_fails_termination() {
    let source = r#"
atom f(m: i64, n: i64) -> i64
    requires: m >= 0;
    ensures: result >= 0;
    decreases: (m, n);
    body: if n != 0 { f(m, n - 1) } else { 0 };
"#;
    let case = verify("lex_negative_component", source, "f");
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
fn test_array_recursive_contract_carries_length_into_uf_domain() {
    let verified = verify("array_count", ARRAY_COUNT, "count_len_caller");
    assert_case(verified, "verified");

    let wrong_source = ARRAY_COUNT.replace(
        "count(a, 0) == len(a) && count(b, 0) == len(b)",
        "count(a, 0) == len(a) + 1 && count(b, 0) == len(b)",
    );
    let wrong = verify("array_count_wrong", &wrong_source, "count_len_caller");
    assert_case_ref(&wrong, "failed");
    assert_eq!(
        wrong.failure_type().as_deref(),
        Some("postcondition_violated"),
        "wrong array recursive postcondition should be an ordinary failure; report:\n{:?}",
        wrong.report
    );
}

#[test]
fn test_array_length_difference_caller_has_explicit_verified_twin() {
    let explicit_results = ARRAY_COUNT.replace(
        "count(a, 0) == len(a) && count(b, 0) == len(b)",
        "count(a, 0) == 3 && count(b, 0) == 4",
    );
    let verified = verify(
        "array_count_explicit_results",
        &explicit_results,
        "count_len_caller",
    );
    assert_case(verified, "verified");

    let wrong_source = explicit_results.replace(
        "count(a, 0) == 3 && count(b, 0) == 4",
        "count(a, 0) == count(b, 0)",
    );
    let wrong = verify(
        "array_count_explicit_wrong",
        &wrong_source,
        "count_len_caller",
    );
    assert_case_ref(&wrong, "failed");
    assert_eq!(
        wrong.failure_type().as_deref(),
        Some("postcondition_violated"),
        "length-only difference should produce an ordinary postcondition failure; report:\n{:?}",
        wrong.report
    );
}

#[test]
fn test_array_recursive_contract_distinguishes_lengths() {
    let case = verify(
        "array_count_length_difference",
        ARRAY_COUNT_EQUAL_LENGTHS,
        "count_len_difference",
    );
    assert_case_ref(&case, "failed");
    assert_eq!(
        case.failure_type().as_deref(),
        Some("postcondition_violated"),
        "different lengths must not become congruent recursive calls; report:\n{:?}",
        case.report
    );
}

#[test]
fn test_array_recursive_contract_verifies_in_bitvec_mode() {
    let verified = verify_with_args(
        "array_count_bv",
        ARRAY_COUNT,
        "count_len_caller",
        &["--bitvec-i64"],
    );
    assert_case(verified, "verified");

    let wrong_source = ARRAY_COUNT.replace(
        "count(a, 0) == len(a) && count(b, 0) == len(b)",
        "count(a, 0) == len(a) + 1 && count(b, 0) == len(b)",
    );
    let wrong = verify_with_args(
        "array_count_bv_wrong",
        &wrong_source,
        "count_len_caller",
        &["--bitvec-i64"],
    );
    assert_case_ref(&wrong, "failed");
    assert_eq!(
        wrong.failure_type().as_deref(),
        Some("postcondition_violated"),
        "wrong BV array recursive postcondition should fail normally; report:\n{:?}",
        wrong.report
    );
}

#[test]
fn test_nondecreasing_array_length_measure_fails_termination() {
    let case = verify(
        "array_count_nondecreasing",
        ARRAY_COUNT_NONDECREASING,
        "count",
    );
    assert_termination(&case, "i");
}

#[test]
fn test_struct_recursive_contract_verifies_and_wrong_twin_fails() {
    let verified = verify("struct_down", STRUCT_DOWN, "down_caller");
    assert_case(verified, "verified");

    let wrong_source = STRUCT_DOWN.replace("down(P { n: 3 }) == 3", "down(P { n: 3 }) == 4");
    let wrong = verify("struct_down_wrong", &wrong_source, "down_caller");
    assert_case_ref(&wrong, "failed");
    assert_eq!(
        wrong.failure_type().as_deref(),
        Some("postcondition_violated"),
        "wrong struct recursive postcondition should be an ordinary failure; report:\n{:?}",
        wrong.report
    );
}

#[test]
fn test_struct_calls_with_different_fields_are_not_congruent() {
    let source = STRUCT_DOWN.replace(
        "down(P { n: 3 }) == 3",
        "down(P { n: 3 }) == down(P { n: 4 })",
    );
    let case = verify("struct_down_different_fields", &source, "down_caller");
    assert_case_ref(&case, "failed");
    assert_eq!(
        case.failure_type().as_deref(),
        Some("postcondition_violated"),
        "different struct fields must be independent recursive UF arguments; report:\n{:?}",
        case.report
    );
}

#[test]
fn test_struct_recursive_contract_uses_invariant_domain_fact() {
    let verified = verify(
        "struct_down_invariant",
        STRUCT_DOWN_INVARIANT,
        "use_down_inv",
    );
    assert_case(verified, "verified");

    let wrong_source = STRUCT_DOWN_INVARIANT.replace("down_inv(p) >= 0", "down_inv(p) >= 1");
    let wrong = verify("struct_down_invariant_wrong", &wrong_source, "use_down_inv");
    assert_case_ref(&wrong, "failed");
    assert_eq!(
        wrong.failure_type().as_deref(),
        Some("postcondition_violated"),
        "wrong invariant-dependent postcondition should fail; report:\n{:?}",
        wrong.report
    );
}

#[test]
fn test_unsupported_array_and_struct_recursive_parameters_keep_fresh_results() {
    let cases = [
        (
            "unsupported_ref_mut_array",
            UNSUPPORTED_REF_MUT_ARRAY,
            "atom 'bad' has a `ref mut` parameter",
        ),
        (
            "unsupported_ref_mut_struct",
            UNSUPPORTED_REF_MUT_STRUCT,
            "atom 'bad' has a `ref mut` parameter",
        ),
        (
            "unsupported_consume_array",
            UNSUPPORTED_CONSUME_ARRAY,
            "atom 'bad' consumes a parameter",
        ),
        (
            "unsupported_consume_struct",
            UNSUPPORTED_CONSUME_STRUCT,
            "atom 'bad' consumes a parameter",
        ),
        (
            "unsupported_stored_array",
            UNSUPPORTED_STORED_ARRAY,
            "atom 'bad' may store into array parameter 'a'",
        ),
        (
            "unsupported_float_array",
            UNSUPPORTED_FLOAT_ARRAY,
            "atom 'bad' parameter 'a' has an unsupported array element type 'f64'",
        ),
        (
            "unsupported_struct_array_field",
            UNSUPPORTED_STRUCT_ARRAY_FIELD,
            "atom 'bad' parameter 'p' has struct type 'P' with non-scalar fields or call-containing constraints/invariants",
        ),
        (
            "unsupported_struct_call_invariant",
            UNSUPPORTED_STRUCT_CALL_INVARIANT,
            "atom 'bad' parameter 'p' has struct type 'P' with non-scalar fields or call-containing constraints/invariants",
        ),
        (
            "unsupported_array_return",
            UNSUPPORTED_ARRAY_RETURN,
            "atom 'bad' returns a non-scalar type",
        ),
        (
            "unsupported_struct_return",
            UNSUPPORTED_STRUCT_RETURN,
            "atom 'bad' returns a non-scalar type",
        ),
        (
            "unsupported_array_measure_access",
            UNSUPPORTED_ARRAY_MEASURE_ACCESS,
            "decreases measure of atom 'bad' must not read array elements",
        ),
        (
            "unsupported_array_direct_measure",
            UNSUPPORTED_ARRAY_DIRECT_MEASURE,
            "decreases measure of atom 'bad' uses non-scalar parameter 'a' directly",
        ),
        (
            "unsupported_struct_direct_measure",
            UNSUPPORTED_STRUCT_DIRECT_MEASURE,
            "decreases measure of atom 'bad' uses non-scalar parameter 'p' directly",
        ),
        (
            "unsupported_len_non_array_measure",
            UNSUPPORTED_LEN_NON_ARRAY_MEASURE,
            "decreases measure of atom 'bad' must not contain calls",
        ),
    ];

    for (name, source, reason) in cases {
        let case = verify(name, source, "bad");
        assert!(
            case.did_not_crash(),
            "{name} crashed; stdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&case.output.stdout),
            String::from_utf8_lossy(&case.output.stderr)
        );
        assert!(
            case.has_diagnostic_code("recursive_contract_unsupported"),
            "{name} should emit recursive_contract_unsupported; report:\n{:?}",
            case.report
        );
        assert!(
            case.diagnostic_message_contains("recursive_contract_unsupported", reason),
            "{name} should report exact reason {reason:?}; report:\n{:?}",
            case.report
        );
        assert_eq!(
            case.verdict().as_deref(),
            Some("failed"),
            "{name} should retain a fresh recursive-call result rather than prove the contract; report:\n{:?}",
            case.report
        );
        assert_eq!(
            case.failure_type().as_deref(),
            Some("postcondition_violated"),
            "{name} should fail through the postcondition, not termination or contradiction; report:\n{:?}\nstdout:\n{}\nstderr:\n{}",
            case.report,
            String::from_utf8_lossy(&case.output.stdout),
            String::from_utf8_lossy(&case.output.stderr)
        );
    }
}

#[test]
fn test_constant_argument_call_unfolds_and_verifies() {
    let case = verify("tri3", TRI_CONST, "tri3");
    assert_case(case, "verified");
}

#[test]
fn test_constant_argument_call_with_wrong_spec_fails() {
    let source = TRI_CONST.replace("result == 6;", "result == 7;");
    let case = verify("tri3_wrong", &source, "tri3");
    assert_case(case, "failed");
}

#[test]
fn test_refined_parameter_predicates_guard_constant_unfolding() {
    let valid = verify("refined_nat", NAT_REFINEMENT_REPRO, "bad");
    assert_case(valid, "verified");

    let wrong_source =
        NAT_REFINEMENT_REPRO.replace("ensures: result == 0;", "ensures: result == 999;");
    let wrong = verify("refined_nat_wrong", &wrong_source, "bad");
    assert!(
        !wrong.diagnostic_message_contains("failed", "Contradiction found"),
        "wrong refined postcondition must fail without a contradiction diagnostic: {:?}",
        wrong.verdict()
    );
    assert_case(wrong, "failed");
}

#[test]
fn test_constant_refined_nat_argument_unfolds_without_requires() {
    let case = verify("nat_tri3", NAT_TRI_CONST, "tri3");
    assert_case(case, "verified");
}

#[test]
fn test_constant_argument_unfolding_respects_depth_bound() {
    let tri32 = TRI_CONST
        .replace("atom tri3()", "atom tri32()")
        .replace("result == 6", "result == 528")
        .replace("tri(3);", "tri(32);");
    let case = verify("tri32", &tri32, "tri32");
    assert_case(case, "verified");

    let tri40 = TRI_CONST
        .replace("atom tri3()", "atom tri40()")
        .replace("result == 6", "result == 820")
        .replace("tri(3);", "tri(40);");
    let case = verify("tri40", &tri40, "tri40");
    assert_case(case, "failed");
}

#[test]
fn test_constant_argument_call_unfolds_in_bitvec_mode() {
    // Bare `3` lowers as Int, so it misses `tri`'s BV64 parameter sort.
    let tri_bitvec = TRI_CONST
        .replace("ensures: result >= 0 && ", "ensures: ")
        .replace("body: tri(3);", "body: tri(3 + 0);");
    let case = verify_with_args("tri3_bitvec", &tri_bitvec, "tri3", &["--bitvec-i64"]);
    assert_case(case, "verified");
}

#[test]
fn test_constant_argument_call_unfolds_mutual_recursion() {
    let source =
        format!("{MUTUAL_DEC}\natom odd3() -> bool\n    ensures: result;\n    body: is_odd(3);\n");
    let case = verify("odd3", &source, "odd3");
    assert_case(case, "verified");
}

#[test]
fn test_nonconstant_argument_call_does_not_unfold() {
    let tri_definition = TRI_CONST
        .split("\natom tri3()")
        .next()
        .expect("TRI_CONST has tri definition");
    let source = format!(
        "{tri_definition}\natom trin(n: i64)\n    requires: n >= 0;\n    ensures: result == n * (n + 1) / 2;\n    body: tri(n);\n"
    );
    let case = verify("trin", &source, "trin");
    assert_case(case, "failed");
}

#[test]
fn test_quantified_requires_conjunct_verified() {
    let case = verify("qr", QR_QUANTIFIED_REQUIRES, "qr");
    assert_case(case, "verified");
}

#[test]
fn test_quantified_requires_conjunct_violated_fails() {
    let case = verify("qr2", QR_QUANTIFIED_REQUIRES_FAIL, "use_qr2");
    assert!(case.did_not_crash());
    assert_verdict(&case, "failed");
    assert_eq!(
        case.failure_type().as_deref(),
        Some("precondition_violated"),
        "qr2 should report precondition_violated; report:\n{:?}",
        case.report
    );
    let message = format!("{:?}", case.report);
    assert!(
        message.contains("forall(i, 0, k, i < 5)"),
        "the diagnostic should name the violated quantified conjunct; report:\n{message}"
    );
}

#[test]
fn test_scc_internal_widened_quantifier_fails() {
    let case = verify("qr3", QR3_WIDENING, "qr3");
    assert!(case.did_not_crash());
    assert_verdict(&case, "failed");
    assert_eq!(
        case.failure_type().as_deref(),
        Some("precondition_violated"),
        "qr3 should report precondition_violated; report:\n{:?}",
        case.report
    );
    // The generic call-site message does not name the quantified conjunct;
    // that the forall is the cause is shown by the CallerRequires
    // counterfactual, not by the message text.
    let message = format!("{:?}", case.report);
    assert!(
        message.contains("Call to 'qr3'"),
        "the report should identify the failing call; report:\n{message}"
    );
}

#[test]
fn test_scc_internal_stable_quantifier_verifies() {
    let case = verify("qr3_ok", QR3_OK, "qr3_ok");
    assert_case(case, "verified");
}

#[test]
fn test_direct_call_quantified_requires_violated_fails() {
    let case = verify("use_qr2_direct", QR_DIRECT_CALLER, "use_qr2_direct");
    assert!(case.did_not_crash());
    assert_verdict(&case, "failed");
    assert_eq!(
        case.failure_type().as_deref(),
        Some("precondition_violated"),
        "use_qr2_direct should report precondition_violated; report:\n{:?}",
        case.report
    );
}

#[test]
fn test_call_under_quantifier_is_unverifiable() {
    let case = verify("qf", QF_CALL_UNDER_QUANTIFIER, "qf");
    assert!(case.did_not_crash());
    assert_eq!(
        case.report
            .as_ref()
            .and_then(|report| report["status"].as_str()),
        Some("unverifiable"),
        "qf should be unverifiable; report:\n{:?}",
        case.report
    );
    assert_eq!(
        case.output.status.code(),
        Some(3),
        "qf should exit 3; stdout:\n{}",
        String::from_utf8_lossy(&case.output.stdout)
    );
    let stderr = String::from_utf8_lossy(&case.output.stderr);
    let stdout = String::from_utf8_lossy(&case.output.stdout);
    let report = format!("{:?}", case.report);
    assert!(
        stdout.contains("Unsupported call under quantifier")
            || stderr.contains("Unsupported call under quantifier")
            || report.contains("Unsupported call under quantifier"),
        "expected the QUANTIFIER_DEPENDENT_CALL_UNSUPPORTED marker; stdout:\n{stdout}\nreport:\n{report}"
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

// A callee's eligibility inputs (trust level, effects, async, type params)
// feed the transitive-dep proof hash, so editing them must not be served by
// a stale cache entry.
const CACHE_BASE: &str = r#"
atom tri(n: i64) -> i64
    requires: n >= 0;
    ensures: result >= 0;
    decreases: n;
    body: if n == 0 { 0 } else { n + tri(n - 1) };

atom use2(n: i64) -> i64
    requires: n >= 0;
    ensures: result == 0;
    body: tri(n) - tri(n);
"#;

const CACHE_TRUSTED: &str = r#"
trusted atom tri(n: i64) -> i64
    requires: n >= 0;
    ensures: result >= 0;
    decreases: n;
    body: if n == 0 { 0 } else { n + tri(n - 1) };

atom use2(n: i64) -> i64
    requires: n >= 0;
    ensures: result == 0;
    body: tri(n) - tri(n);
"#;

const CACHE_EFFECTS: &str = r#"
effect Log;
atom tri(n: i64) -> i64
    requires: n >= 0;
    ensures: result >= 0;
    decreases: n;
    effects: [Log];
    body: if n == 0 { 0 } else { n + tri(n - 1) };

atom use2(n: i64) -> i64
    requires: n >= 0;
    ensures: result == 0;
    body: tri(n) - tri(n);
"#;

const CACHE_STRUCT_BASE: &str = r#"
struct P {
    n: i64,
    invariant: self.n >= 0
}

atom down_struct(p: P) -> i64
    requires: true;
    ensures: result == p.n;
    decreases: p.n;
    body: if p.n == 0 { 0 } else { 1 + down_struct(P { n: p.n - 1 }) };

atom use_struct(n: i64) -> i64
    requires: n >= 0;
    ensures: result >= 0;
    body: down_struct(P { n: n });
"#;

const CACHE_STRUCT_CHANGED: &str = r#"
struct P {
    n: i64,
    invariant: self.n >= 1
}

atom down_struct(p: P) -> i64
    requires: true;
    ensures: result == p.n;
    decreases: p.n;
    body: if p.n == 0 { 0 } else { 1 + down_struct(P { n: p.n - 1 }) };

atom use_struct(n: i64) -> i64
    requires: n >= 0;
    ensures: result >= 0;
    body: down_struct(P { n: n });
"#;

// Runs `verify` twice in the same directory under the same file name, so the
// second run exercises the on-disk cache written by the first.
fn verify_twice(
    name: &'static str,
    first: &str,
    second: &str,
    target: &'static str,
) -> (CaseResult, CaseResult) {
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
    let run = |source: &str| {
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
        CaseResult {
            name,
            target,
            output,
            payload,
            report,
        }
    };
    let first_result = run(first);
    let second_result = run(second);
    let _ = std::fs::remove_dir_all(dir);
    (first_result, second_result)
}

#[test]
fn test_stale_cache_on_callee_trust_level_change() {
    let (first, second) = verify_twice("cache_trust", CACHE_BASE, CACHE_TRUSTED, "use2");
    assert_case(first, "verified");
    let fresh = verify("cache_trust_fresh", CACHE_TRUSTED, "use2");
    assert!(
        second.did_not_crash(),
        "cached run crashed; stderr:\n{}",
        String::from_utf8_lossy(&second.output.stderr)
    );
    assert_ne!(
        second.verdict().as_deref(),
        Some("verified"),
        "use2 must not verify from a stale cache entry"
    );
    assert_eq!(
        second.output.status.code(),
        fresh.output.status.code(),
        "cached run must match a fresh run of the new source; fresh stdout:\n{}",
        String::from_utf8_lossy(&fresh.output.stdout)
    );
}

#[test]
fn test_stale_cache_on_callee_effects_change() {
    let (first, second) = verify_twice("cache_eff", CACHE_BASE, CACHE_EFFECTS, "use2");
    assert_case(first, "verified");
    let fresh = verify("cache_eff_fresh", CACHE_EFFECTS, "use2");
    assert!(
        second.did_not_crash(),
        "cached run crashed; stderr:\n{}",
        String::from_utf8_lossy(&second.output.stderr)
    );
    assert_ne!(
        second.verdict().as_deref(),
        Some("verified"),
        "use2 must not verify from a stale cache entry"
    );
    assert_eq!(
        second.output.status.code(),
        fresh.output.status.code(),
        "cached run must match a fresh run of the new source; fresh stdout:\n{}",
        String::from_utf8_lossy(&fresh.output.stdout)
    );
}

#[test]
fn test_stale_cache_on_callee_refinement_change() {
    let changed_refinement = NAT_REFINEMENT_REPRO.replace("v >= 0", "v >= -1");
    let (first, second) = verify_twice(
        "cache_refinement",
        NAT_REFINEMENT_REPRO,
        &changed_refinement,
        "bad",
    );
    assert_case(first, "verified");
    let fresh = verify("cache_refinement_fresh", &changed_refinement, "bad");
    assert_ne!(
        fresh.verdict().as_deref(),
        Some("verified"),
        "fresh verification of the changed refinement unexpectedly succeeded"
    );
    assert!(
        second.did_not_crash(),
        "cached run crashed; stderr:\n{}",
        String::from_utf8_lossy(&second.output.stderr)
    );
    assert_ne!(
        second.verdict().as_deref(),
        Some("verified"),
        "bad must not verify from a stale cache entry"
    );
    assert_eq!(
        second.output.status.code(),
        fresh.output.status.code(),
        "cached run must match a fresh run of the changed refinement; fresh stdout:\n{}",
        String::from_utf8_lossy(&fresh.output.stdout)
    );
}

#[test]
fn test_stale_cache_on_recursive_callee_struct_invariant_change() {
    let (first, second) = verify_twice(
        "cache_struct",
        CACHE_STRUCT_BASE,
        CACHE_STRUCT_CHANGED,
        "use_struct",
    );
    assert_case(first, "verified");
    let fresh = verify("cache_struct_fresh", CACHE_STRUCT_CHANGED, "use_struct");
    assert!(
        second.did_not_crash(),
        "cached run crashed; stderr:\n{}",
        String::from_utf8_lossy(&second.output.stderr)
    );
    assert_ne!(
        second.verdict().as_deref(),
        Some("verified"),
        "use_struct must not verify from a stale struct-definition cache entry"
    );
    assert_eq!(
        second.output.status.code(),
        fresh.output.status.code(),
        "cached run must match a fresh run of the changed struct source; fresh stdout:\n{}",
        String::from_utf8_lossy(&fresh.output.stdout)
    );
}

// The bind-time pass over a lambda body uses arbitrary param constants, so
// recursive calls there are not congruent; the real invocation through
// `apply_local_lambda` runs the full congruent path. Callees keep
// `requires: true` because a lambda body calling any atom with a non-trivial
// requires already fails at bind time.
const LAM_INVOKED: &str = r#"
atom lt(n: i64) -> i64
    requires: true;
    ensures: result >= 0;
    decreases: n;
    body: if n <= 0 { 0 } else { let g = |x| lt(x); g(n - 1) };
"#;

const LAM_NON_DECREASING: &str = r#"
atom lb(n: i64) -> i64
    requires: true;
    ensures: result >= 0;
    decreases: n;
    body: if n <= 0 { 0 } else { let g = |x| lb(x); g(n) };
"#;

const LAM_UNINVOKED: &str = r#"
atom lu(n: i64) -> i64
    requires: true;
    ensures: result >= 0;
    decreases: n;
    body: if n <= 0 { 0 } else { let g = |x| lu(x); lu(n - 1) };
"#;

#[test]
fn test_lambda_invoked_decreasing_call_verifies() {
    let case = verify("lt", LAM_INVOKED, "lt");
    assert_case(case, "verified");
}

#[test]
fn test_lambda_invoked_stable_call_fails_termination() {
    let case = verify("lb", LAM_NON_DECREASING, "lb");
    assert_verdict(&case, "failed");
    assert_eq!(
        case.failure_type().as_deref(),
        Some("termination_measure_violation"),
        "lb should report termination_measure_violation; report:\n{:?}",
        case.report
    );
}

#[test]
fn test_lambda_bound_but_never_invoked_verifies() {
    let case = verify("lu", LAM_UNINVOKED, "lu");
    assert_case(case, "verified");
}

// Calls inside top-level quantified requires conjuncts are SCC edges, so
// `qp` is a recursive call and needs a termination obligation.
const QP_QUANTIFIED_SELF_REQUIRES: &str = r#"
atom qp(n: i64) -> i64
    requires: n >= 0 && forall(i, 0, n, qp(n) >= 0);
    ensures: result >= 0;
    decreases: n;
    body: 0;
"#;

// A binder-dependent call is still rejected by
// `reject_quantifier_dependent_call` before any congruent lowering.
const QP2_BINDER_CALL: &str = r#"
atom qp2(n: i64) -> i64
    requires: n >= 0 && forall(i, 0, n, qp2(i) >= 0);
    ensures: result >= 0;
    decreases: n;
    body: 0;
"#;

const LOOP_DECREASING: &str = r#"
atom wl(n: i64) -> i64
    requires: n >= 0;
    ensures: result >= 0;
    decreases: n;
    body: {
        let s = 0;
        let i = 0;
        while i < n
        invariant: i >= 0 && i <= n && s >= 0
        decreases: n - i
        {
            s = s + wl(n - 1);
            i = i + 1;
        };
        s
    };
"#;

const LOOP_STABLE: &str = r#"
atom wb(n: i64) -> i64
    requires: n >= 0;
    ensures: result >= 0;
    decreases: n;
    body: {
        let s = 0;
        let i = 0;
        while i < n
        invariant: i >= 0 && i <= n && s >= 0
        decreases: n - i
        {
            s = s + wb(n);
            i = i + 1;
        };
        s
    };
"#;

// `s == 0` only holds through congruence: `wc(n - 1) - wc(n - 1)` is 0 only
// because the two identical calls share one `rec_fn#wc` result.
const LOOP_CONGRUENT: &str = r#"
atom wc(n: i64) -> i64
    requires: n >= 0;
    ensures: result >= 0;
    decreases: n;
    body: {
        let s = 0;
        let i = 0;
        while i < n
        invariant: i >= 0 && i <= n && s == 0
        decreases: n - i
        {
            s = s + wc(n - 1) - wc(n - 1);
            i = i + 1;
        };
        s
    };
"#;

#[test]
fn test_quantified_requires_self_call_fails_termination() {
    let case = verify("qp", QP_QUANTIFIED_SELF_REQUIRES, "qp");
    assert!(case.did_not_crash());
    assert_verdict(&case, "failed");
    assert_eq!(
        case.failure_type().as_deref(),
        Some("termination_measure_violation"),
        "qp should report termination_measure_violation; report:\n{:?}",
        case.report
    );
}

#[test]
fn test_quantified_requires_binder_call_stays_unverifiable() {
    let case = verify("qp2", QP2_BINDER_CALL, "qp2");
    assert!(case.did_not_crash());
    assert_eq!(
        case.output.status.code(),
        Some(3),
        "qp2 should exit 3; stdout:\n{}",
        String::from_utf8_lossy(&case.output.stdout)
    );
    let stdout = String::from_utf8_lossy(&case.output.stdout);
    assert!(
        stdout.contains("Unsupported call under quantifier"),
        "expected the QUANTIFIER_DEPENDENT_CALL_UNSUPPORTED marker; stdout:\n{stdout}"
    );
}

#[test]
fn test_recursive_call_in_loop_decreases_verifies() {
    let case = verify("wl", LOOP_DECREASING, "wl");
    assert_case(case, "verified");
}

#[test]
fn test_recursive_call_in_loop_stable_fails_termination() {
    let case = verify("wb", LOOP_STABLE, "wb");
    assert_verdict(&case, "failed");
    assert_eq!(
        case.failure_type().as_deref(),
        Some("termination_measure_violation"),
        "wb should report termination_measure_violation; report:\n{:?}",
        case.report
    );
}

#[test]
fn test_recursive_call_in_loop_congruent_invariant_verifies() {
    let case = verify("wc", LOOP_CONGRUENT, "wc");
    assert_case(case, "verified");
}
