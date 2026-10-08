//! Whole-array `==`/`!=` lowering in the Z3 translator.
//!
//! An array value is its (contents, len) pair: the Z3 `Array` sort carries
//! only contents, while the length is a separate `len_*` symbol in the
//! active `i64` sort. `a == b` therefore lowers to `len_a == len_b` AND
//! `forall k in [0, len_a): a[k] == b[k]` — contents beyond the length are
//! arbitrary (literals store over an unconstrained base const) and must not
//! count, so bare extensional `=` is rejected as a lowering.
//!
//! Mirrors tests/test_recursive_contract_decreases.rs: each case is
//! verified in a fresh temp dir with `--json`, the verdict is read from
//! diagnostics / report.json, and crashes are asserted away.

use serde_json::Value;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

// `a == b` usable in every contract position.
const EQ_POSITIONS: &str = r#"
atom eq_body(a: [i64], b: [i64]) -> bool
    requires: len(a) == 2 && len(b) == 2;
    ensures: result == (a == b);
    body: a == b;

atom eq_requires(a: [i64], b: [i64]) -> i64
    requires: len(a) == 2 && a == b;
    ensures: result == b[1];
    body: a[1];

atom eq_ensures(a: [i64], b: [i64]) -> bool
    requires: len(a) == 2 && len(b) == 2 && forall(i, 0, 2, a[i] == b[i]);
    ensures: a == b;
    body: true;
"#;

// Equal arrays compare equal; same prefix with different len is `!=`;
// different contents are `!=`.
const EQ_VALUES: &str = r#"
atom same_literal() -> bool
    requires: true;
    ensures: result == true;
    body: [1, 2, 3] == [1, 2, 3];

atom same_prefix_diff_len() -> bool
    requires: true;
    ensures: result == true;
    body: [1, 2] != [1, 2, 0];

atom diff_contents() -> bool
    requires: true;
    ensures: result == true;
    body: [1, 2] != [1, 9];

atom neq_implies_len_gap(a: [i64], b: [i64]) -> bool
    requires: len(a) == 1 && a != b && b[0] == a[0];
    ensures: len(b) != 1;
    body: true;
"#;

// `let b = a` moves `a` (arrays are Move), so the alias is exercised
// through `b`; `a = [1, 2]` reassignment rewrites the tracked chain.
const EQ_ALIAS_REASSIGN: &str = r#"
atom alias_eq(a: [i64]) -> bool
    requires: len(a) == 2 && a[0] == 5 && a[1] == 7;
    ensures: result == true;
    body: {
        let b = a;
        b == [5, 7]
    };

atom reassign_eq(a: [i64]) -> bool
    requires: len(a) == 2;
    ensures: result == true;
    body: {
        let b = a;
        a = [1, 2];
        a == [1, 2]
    };

atom store_then_eq(a: [i64]) -> bool
    requires: len(a) == 2;
    ensures: result == true;
    body: {
        a[0] = 5;
        a[1] = 7;
        a == [5, 7]
    };

atom store_breaks_eq(a: [i64]) -> bool
    requires: len(a) == 2 && a[0] == 0 && a[1] == 0;
    ensures: result == true;
    body: {
        let b = [0, 0];
        a[0] = 42;
        a != b
    };
"#;

// The left operand is evaluated before the right: a shadowing `let a`
// inside the right operand must not replace the left's value — outer
// `[0, 0]` vs inner `[0, 1]` is false (contents), outer `len == 2` vs
// inner `len == 3` is false (length), and outer `[0, 0]` vs inner
// `[0, 0]` is true.
const EQ_SHADOWED_OPERAND: &str = r#"
atom shadow_rhs_contents(a: [i64]) -> bool
    requires: len(a) == 2 && a[0] == 0 && a[1] == 0;
    ensures: result == false;
    body: a == ({ let a = [0, 1]; a });

atom shadow_rhs_len(a: [i64]) -> bool
    requires: len(a) == 2;
    ensures: result == false;
    body: a == ({ let a = [1, 2, 3]; a });

atom shadow_rhs_true(a: [i64]) -> bool
    requires: len(a) == 2 && a[0] == 0 && a[1] == 0;
    ensures: result == true;
    body: a == ({ let a = [0, 0]; a });
"#;

// Conditional arrays: `if`/`else` branches with different literal lengths
// resolve through `tail_len_expr`'s `ite` — `select(ite(..), k)` is not a
// legal Z3 pattern, so the quantifier must carry no explicit trigger.
const EQ_CONDITIONAL: &str = r#"
atom if_bound_eq(n: i64) -> bool
    requires: n > 0;
    ensures: result == true;
    body: {
        let a = if n > 0 { [1, 2] } else { [1, 2, 3] };
        a == [1, 2]
    };

atom if_expr_eq(n: i64) -> bool
    requires: n > 0;
    ensures: result == true;
    body: (if n > 0 { [1, 2] } else { [1, 2, 3] }) == [1, 2];

atom if_expr_neq(n: i64) -> bool
    requires: n > 0;
    ensures: result == true;
    body: (if n > 0 { [1, 2] } else { [1, 2, 3] }) != [1, 2, 3];
"#;

// `a == b` nested inside a user `forall` condition, and [bool] elements.
const EQ_NESTED_BOOL: &str = r#"
atom eq_in_forall(a: [i64], b: [i64], n: i64) -> bool
    requires: n == len(a) && a == b;
    ensures: forall(i, 0, n, a[i] == b[i]);
    body: true;

atom bool_eq(a: [bool], b: [bool]) -> bool
    requires: len(a) == 2 && len(b) == 2 && forall(i, 0, 2, a[i] == b[i]);
    ensures: a == b;
    body: true;
"#;

// Call results carry their callee-minted `len` — `mk(f) == f` works both
// through a `let` binding and directly in ensures. A refined param type
// (`Nat`) exercises the full param-assumption path at instantiation.
const EQ_CALL_RESULT: &str = r#"
type Nat = i64 where v >= 0;

atom nth(a: [i64], n: Nat) -> i64
    requires: len(a) == n + 1;
    ensures: result == a[n];
    body: a[n];

atom mk(a: [i64]) -> [i64]
    requires: len(a) == 2;
    ensures: result == a;
    body: a;

atom eq_call_let(f: [i64]) -> bool
    requires: len(f) == 2;
    ensures: result == true;
    body: {
        let t = mk(f);
        t == f
    };

atom eq_call_expr(f: [i64]) -> bool
    requires: len(f) == 2;
    ensures: mk(f) == f;
    body: true;

atom eq_refined_callee(a: [i64], b: [i64]) -> bool
    requires: len(a) == 2 && a == b;
    ensures: nth(a, 1) == b[1];
    body: true;
"#;

// Bit-vector mode (`--bitvec-i64`): `len_*` symbols are BV(64) while array
// elements stay `Int`; equality must keep both encodings consistent.
const EQ_BITVEC: &str = r#"
atom bv_eq(a: [i64], b: [i64]) -> bool
    requires: len(a) == 2 && len(b) == 2 && forall(i, 0, 2, a[i] == b[i]);
    ensures: a == b;
    body: true;

atom bv_lit_neq() -> bool
    requires: true;
    ensures: result == true;
    body: [1, 2] != [1, 2, 0];
"#;

// Negative cases: mismatched element sorts and array-vs-scalar are type
// errors, not silently dropped or mislowered.
const EQ_MISMATCHED_TYPES: &str = r#"
atom mixed(a: [i64], b: [bool]) -> bool
    requires: true;
    ensures: a == b;
    body: true;
"#;

const EQ_NON_ARRAY_OPERAND: &str = r#"
atom nonarr(a: [i64]) -> bool
    requires: true;
    ensures: a == 5;
    body: true;

atom nonarr_real(a: [i64]) -> bool
    requires: true;
    ensures: a == 0.0;
    body: true;

atom nonarr_f64(a: [i64], x: f64) -> bool
    requires: true;
    ensures: a == x;
    body: true;
"#;

// A false equality goal must fail — `[1, 2]` and `[1, 9]` are not equal.
const EQ_FALSE_GOAL: &str = r#"
atom bad_eq() -> bool
    requires: true;
    ensures: result == true;
    body: [1, 2] == [1, 9];
"#;

// IEEE `NaN` is not `fp.eq`-equal to itself, so `[NaN] == [NaN]` is false
// under `--ieee754-f64` (a `0.0/0.0` literal element is concrete `NaN`).
const EQ_NAN: &str = r#"
atom nan_ne() -> bool
    requires: true;
    ensures: result == false;
    body: [0.0 / 0.0] == [0.0 / 0.0];

atom nan_neq() -> bool
    requires: true;
    ensures: result == true;
    body: [0.0 / 0.0] != [0.0 / 0.0];
"#;

struct CaseResult {
    name: &'static str,
    target: &'static str,
    output: Output,
    payload: Option<Value>,
    report: Option<Value>,
}

/// Codes that carry the actual verdict. Advisory diagnostics precede the
/// failure entry, so the scan must skip any code that is not a verdict.
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

    fn diagnostic_message_contains(&self, text: &str) -> bool {
        self.payload
            .as_ref()
            .and_then(|payload| payload["diagnostics"].as_array())
            .is_some_and(|diagnostics| {
                diagnostics.iter().any(|diagnostic| {
                    diagnostic["message"]
                        .as_str()
                        .is_some_and(|message| message.contains(text))
                })
            })
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
        "mumei_array_equality_{name}_{}_{}",
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

/// Assert that EVERY atom in the fixture verified — `--json` reports the
/// last atom's outcome on success, so multi-atom fixtures check the run
/// `exit_code` (0 means no atom failed or came back unverifiable) plus the
/// last atom's own status.
fn assert_all_verified(case: &CaseResult) {
    assert!(
        case.did_not_crash(),
        "{} crashed; stderr:\n{}",
        case.name,
        String::from_utf8_lossy(&case.output.stderr)
    );
    let payload = case.payload.as_ref().unwrap_or_else(|| {
        panic!(
            "{} produced no JSON payload; stdout:\n{}\nstderr:\n{}",
            case.name,
            String::from_utf8_lossy(&case.output.stdout),
            String::from_utf8_lossy(&case.output.stderr)
        )
    });
    assert_eq!(
        payload["exit_code"].as_i64(),
        Some(0),
        "{} exited non-zero; stdout:\n{}",
        case.name,
        String::from_utf8_lossy(&case.output.stdout)
    );
    assert_eq!(
        payload["status"].as_str(),
        Some("success"),
        "{} last atom is not success; stdout:\n{}",
        case.name,
        String::from_utf8_lossy(&case.output.stdout)
    );
}

#[test]
fn array_eq_works_in_body_requires_and_ensures() {
    let case = verify("eq_positions", EQ_POSITIONS, "eq_ensures");
    assert_all_verified(&case);
}

#[test]
fn array_eq_values() {
    let case = verify("eq_values", EQ_VALUES, "neq_implies_len_gap");
    assert_all_verified(&case);
}

#[test]
fn array_eq_alias_reassign_and_stores() {
    let case = verify("eq_alias_reassign", EQ_ALIAS_REASSIGN, "store_breaks_eq");
    assert_all_verified(&case);
}

#[test]
fn array_eq_nested_in_forall_and_bool_elements() {
    let case = verify("eq_nested_bool", EQ_NESTED_BOOL, "bool_eq");
    assert_all_verified(&case);
}

#[test]
fn array_eq_on_call_results_and_refined_callee() {
    let case = verify("eq_call_result", EQ_CALL_RESULT, "eq_refined_callee");
    assert_all_verified(&case);
}

#[test]
fn array_eq_bitvec_mode() {
    let case = verify_with_args("eq_bitvec", EQ_BITVEC, "bv_lit_neq", &["--bitvec-i64"]);
    assert_all_verified(&case);
}

#[test]
fn array_eq_rejects_mismatched_element_types() {
    let case = verify("eq_mismatched_types", EQ_MISMATCHED_TYPES, "mixed");
    assert!(
        case.did_not_crash(),
        "crashed; stderr:\n{}",
        String::from_utf8_lossy(&case.output.stderr)
    );
    assert_verdict(&case, "failed");
    assert!(
        case.diagnostic_message_contains("different element types"),
        "expected a different-element-types diagnostic; stdout:\n{}",
        String::from_utf8_lossy(&case.output.stdout)
    );
}

#[test]
fn array_eq_shadowed_left_operand() {
    let case = verify(
        "eq_shadowed_operand",
        EQ_SHADOWED_OPERAND,
        "shadow_rhs_true",
    );
    assert_all_verified(&case);
}

#[test]
fn array_eq_conditional_arrays() {
    let case = verify("eq_conditional", EQ_CONDITIONAL, "if_expr_neq");
    assert_all_verified(&case);
}

#[test]
fn array_eq_ieee754_nan_is_not_equal() {
    let case = verify_with_args("eq_nan", EQ_NAN, "nan_neq", &["--ieee754-f64"]);
    assert_all_verified(&case);
}

#[test]
fn array_eq_rejects_non_array_operand() {
    let case = verify("eq_non_array_operand", EQ_NON_ARRAY_OPERAND, "nonarr_f64");
    assert!(
        case.did_not_crash(),
        "crashed; stderr:\n{}",
        String::from_utf8_lossy(&case.output.stderr)
    );
    assert_verdict(&case, "failed");
    assert!(
        case.diagnostic_message_contains("non-array"),
        "expected a non-array-operand diagnostic; stdout:\n{}",
        String::from_utf8_lossy(&case.output.stdout)
    );
}

#[test]
fn array_eq_false_goal_fails() {
    assert_case(verify("eq_false_goal", EQ_FALSE_GOAL, "bad_eq"), "failed");
}
