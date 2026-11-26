//! Regression coverage for quantified `requires` handling: top-level
//! `forall`/`exists` conjuncts are checked at call sites (direct calls and
//! `call(atom_ref(..))`), while quantifiers nested under `||`, `!`, `if`,
//! comparisons, etc. stay in the requires text and are lowered in place.

use std::path::PathBuf;
use std::process::{Command, Output};

fn write_fixture(name: &str, source: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "mumei_quantified_requires_{}_{}_{}",
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

fn verify_json(name: &str, source: &str) -> (PathBuf, Output, serde_json::Value) {
    let (dir, fixture) = write_fixture(name, source);
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg(&fixture)
        .arg("--json")
        .arg("--report-dir")
        .arg(&dir)
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

fn output_text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The `diagnostics` entry for one atom of a multi-atom file — the stdout
/// payload holds per-atom outcomes only via diagnostics (report.json
/// describes just the last atom processed).
fn atom_diagnostic<'a>(
    payload: &'a serde_json::Value,
    atom: &str,
) -> Option<&'a serde_json::Value> {
    payload["diagnostics"]
        .as_array()
        .and_then(|diags| diags.iter().find(|d| d["atom"] == atom))
}

/// report.json reflects only the last atom; read failure_type from it when
/// it names the atom we are asserting on, else fall back to the diagnostic
/// message.
fn atom_failure_type(dir: &std::path::Path, payload: &serde_json::Value, atom: &str) -> String {
    if let Ok(content) = std::fs::read_to_string(dir.join("report.json")) {
        if let Ok(report) = serde_json::from_str::<serde_json::Value>(&content) {
            if report["atom"].as_str() == Some(atom) {
                if let Some(ft) = report["failure_type"].as_str() {
                    return ft.to_string();
                }
            }
        }
    }
    atom_diagnostic(payload, atom)
        .and_then(|d| d["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

fn assert_failed_with(
    payload: &serde_json::Value,
    atom: &str,
    dir: &std::path::Path,
    needle: &str,
) {
    let diag = atom_diagnostic(payload, atom).unwrap_or_else(|| {
        panic!(
            "no diagnostic for atom {atom}:\n{}",
            serde_json::to_string_pretty(payload).unwrap()
        )
    });
    assert_eq!(diag["code"], "failed", "{diag:#}");
    let detail = atom_failure_type(dir, payload, atom);
    assert!(
        detail.contains(needle),
        "expected failure detail containing '{needle}' for {atom}, got '{detail}'"
    );
}

fn assert_verified(output: &Output, payload: &serde_json::Value) {
    assert!(output.status.success(), "{}", output_text(output));
    if let Some(diags) = payload["diagnostics"].as_array() {
        for d in diags {
            assert_ne!(d["code"], "failed", "{d:#}");
        }
    }
}

const NEEDS_POS: &str = r#"
atom needs_pos(arr: [i64], n: i64) -> i64
requires: n >= 1 && len(arr) >= n && forall(i, 0, n, arr[i] > 0);
ensures: result > 0;
body: arr[0];
"#;

// A1: a caller that never established the forall is rejected.
#[test]
fn caller_missing_forall_precondition_is_rejected() {
    let (dir, output, payload) = verify_json(
        "caller_bad",
        &format!(
            r#"{NEEDS_POS}
atom caller_bad(arr: [i64], n: i64) -> i64
requires: n >= 1 && len(arr) >= n;
ensures: result > 0;
body: needs_pos(arr, n);
"#
        ),
    );
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert_failed_with(&payload, "caller_bad", &dir, "precondition_violated");
    std::fs::remove_dir_all(dir).ok();
}

// A2: a caller carrying the same forall conjunct verifies.
#[test]
fn caller_with_same_forall_precondition_verifies() {
    let (dir, output, payload) = verify_json(
        "caller_same",
        &format!(
            r#"{NEEDS_POS}
atom caller_same(arr: [i64], n: i64) -> i64
requires: n >= 1 && len(arr) >= n && forall(i, 0, n, arr[i] > 0);
ensures: result > 0;
body: needs_pos(arr, n);
"#
        ),
    );
    assert_verified(&output, &payload);
    std::fs::remove_dir_all(dir).ok();
}

// A3: a strictly stronger forall conjunct also satisfies the call site.
#[test]
fn caller_with_stronger_forall_precondition_verifies() {
    let (dir, output, payload) = verify_json(
        "caller_stronger",
        &format!(
            r#"{NEEDS_POS}
atom caller_stronger(arr: [i64], n: i64) -> i64
requires: n >= 1 && len(arr) >= n && forall(i, 0, n, arr[i] > 1);
ensures: result > 0;
body: needs_pos(arr, n);
"#
        ),
    );
    assert_verified(&output, &payload);
    std::fs::remove_dir_all(dir).ok();
}

// A4: an `exists` requires is also a call-site obligation.
#[test]
fn caller_missing_exists_precondition_is_rejected() {
    let (dir, output, payload) = verify_json(
        "caller_bad_exists",
        r#"
atom needs_zero(arr: [i64], n: i64) -> i64
requires: n >= 1 && len(arr) >= n && exists(i, 0, n, arr[i] == 0);
ensures: result == 0;
body: 0;

atom caller_bad_exists(arr: [i64], n: i64) -> i64
requires: n >= 1 && len(arr) >= n;
ensures: result == 0;
body: needs_zero(arr, n);
"#,
    );
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert_failed_with(&payload, "caller_bad_exists", &dir, "precondition_violated");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn caller_with_exists_witness_verifies() {
    let (dir, output, payload) = verify_json(
        "caller_ok_exists",
        r#"
atom needs_zero(arr: [i64], n: i64) -> i64
requires: n >= 1 && len(arr) >= n && exists(i, 0, n, arr[i] == 0);
ensures: result == 0;
body: 0;

atom caller_ok_exists(arr: [i64], n: i64) -> i64
requires: n >= 1 && len(arr) >= n && arr[0] == 0;
ensures: result == 0;
body: needs_zero(arr, n);
"#,
    );
    assert_verified(&output, &payload);
    std::fs::remove_dir_all(dir).ok();
}

// A5: the `call(atom_ref(..))` path applies the same obligation.
#[test]
fn atom_ref_call_missing_forall_precondition_is_rejected() {
    let (dir, output, payload) = verify_json(
        "caller_bad_ref",
        &format!(
            r#"{NEEDS_POS}
atom caller_bad_ref(arr: [i64], n: i64) -> i64
requires: n >= 1 && len(arr) >= n;
ensures: result > 0;
body: call(atom_ref(needs_pos), arr, n);
"#
        ),
    );
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    let diag = atom_diagnostic(&payload, "caller_bad_ref")
        .unwrap_or_else(|| panic!("no diagnostic for caller_bad_ref:\n{payload:#}"));
    assert_eq!(diag["code"], "failed", "{diag:#}");
    let detail = atom_failure_type(&dir, &payload, "caller_bad_ref");
    assert!(
        detail.contains("precondition_violated") || detail.contains("precondition"),
        "expected a precondition failure for caller_bad_ref, got '{detail}'"
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn atom_ref_call_with_forall_precondition_verifies() {
    let (dir, output, payload) = verify_json(
        "caller_ok_ref",
        &format!(
            r#"{NEEDS_POS}
atom caller_ok_ref(arr: [i64], n: i64) -> i64
requires: n >= 1 && len(arr) >= n && forall(i, 0, n, arr[i] > 0);
ensures: result > 0;
body: call(atom_ref(needs_pos), arr, n);
"#
        ),
    );
    assert_verified(&output, &payload);
    std::fs::remove_dir_all(dir).ok();
}

// A6: a call inside a cover clause records the callee's quantified
// requires as a cover obligation — without it the cover would report a
// violating witness.
#[test]
fn cover_call_records_quantified_requires_obligation() {
    let (dir, output, payload) = verify_json(
        "cover_quantified",
        &format!(
            r#"{NEEDS_POS}
atom cov(arr: [i64], n: i64) -> i64
requires: n >= 1 && len(arr) >= n;
ensures: true;
cover "pos call": needs_pos(arr, n) >= 1 && arr[0] <= 0;
body: 0;
"#
        ),
    );
    let text = output_text(&output);
    // The forall makes arr[0] <= 0 unreachable. An unreachable cover
    // aborts the atom with failure_type "cover_unreachable" (the only
    // per-clause statuses the cover machinery emits are "covered" and
    // "unknown"; unreachable produces no cover_results entry).
    assert_eq!(
        atom_failure_type(&dir, &payload, "cov"),
        "cover_unreachable",
        "{text}"
    );
    // If a per-clause status entry was emitted, it must not be a covered
    // witness.
    let cover_status = std::fs::read_to_string(dir.join("report.json"))
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
        .filter(|r| r["atom"] == "cov")
        .or_else(|| Some(payload.clone()))
        .and_then(|r| {
            r["cover_results"].as_array().and_then(|results| {
                results
                    .iter()
                    .find(|entry| entry["label"] == "pos call")
                    .map(|entry| entry["status"].as_str().unwrap_or_default().to_string())
            })
        });
    if let Some(status) = cover_status {
        assert_ne!(status, "covered", "{text}");
        assert!(
            status == "unknown",
            "unexpected cover status {status}: {text}"
        );
    }
    std::fs::remove_dir_all(dir).ok();
}

// A8: binder capture — a caller parameter sharing the callee's bound name
// must not satisfy the quantified obligation for free.
#[test]
fn caller_param_named_like_bound_var_does_not_capture() {
    let (dir, output, payload) = verify_json(
        "caller_i",
        &format!(
            r#"{NEEDS_POS}
atom caller_i(arr: [i64], i: i64) -> i64
requires: i >= 1 && len(arr) >= i;
ensures: result > 0;
body: needs_pos(arr, i);
"#
        ),
    );
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert_failed_with(&payload, "caller_i", &dir, "precondition_violated");
    std::fs::remove_dir_all(dir).ok();
}

// A9: the same caller verifies when it establishes the obligation with
// its own quantifier over a differently named binder.
#[test]
fn caller_param_shares_bound_var_but_establishes_obligation() {
    let (dir, output, payload) = verify_json(
        "caller_k",
        &format!(
            r#"{NEEDS_POS}
atom caller_k(arr: [i64], i: i64) -> i64
requires: i >= 1 && len(arr) >= i && forall(k, 0, i, arr[k] > 0);
ensures: result > 0;
body: needs_pos(arr, i);
"#
        ),
    );
    assert_verified(&output, &payload);
    std::fs::remove_dir_all(dir).ok();
}

// A10: an ensures quantifier must not capture a same-named parameter —
// this atom claims `arr[i] > 0` over a range it never established.
#[test]
fn ensures_forall_does_not_capture_same_named_param() {
    let (dir, output, payload) = verify_json(
        "ens_cap",
        r#"
atom ens_cap(arr: [i64], i: i64) -> i64
requires: i >= 2 && len(arr) >= i;
ensures: forall(i, 0, i, arr[i] > 0);
body: 0;
"#,
    );
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert_eq!(
        atom_failure_type(&dir, &payload, "ens_cap"),
        "postcondition_violated"
    );
    std::fs::remove_dir_all(dir).ok();
}

// A11: the requires-side binder over a same-named parameter still binds
// the quantified variable, not the parameter.
#[test]
fn requires_forall_over_same_named_param_verifies() {
    let (dir, output, payload) = verify_json(
        "req_cap",
        r#"
atom req_cap(arr: [i64], i: i64) -> i64
requires: i >= 2 && len(arr) >= i && forall(i, 0, i, arr[i] > 0);
ensures: arr[0] > 0;
body: 0;
"#,
    );
    assert_verified(&output, &payload);
    std::fs::remove_dir_all(dir).ok();
}

const IDENT: &str = r#"
atom ident(x: i64)
    requires: true;
    ensures: result == x;
    body: x;
"#;

/// Runs `mumei verify` without `--json` (unverifiable exits may not emit a
/// JSON payload) and returns the fixture dir plus the process output.
fn verify_plain(name: &str, source: &str) -> (PathBuf, Output) {
    let (dir, fixture) = write_fixture(name, source);
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg(&fixture)
        .arg("--report-dir")
        .arg(&dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run verify");
    (dir, output)
}

// P1: a quantified callee obligation that calls an atom on the bound
// variable fails closed at the call site — the caller is unverifiable,
// never silently verified.
#[test]
fn caller_of_forall_call_obligation_is_unverifiable() {
    let (dir, output) = verify_plain(
        "caller_ident",
        &format!(
            r#"{IDENT}
atom needs_ident(arr: [i64], n: i64) -> i64
requires: n >= 1 && len(arr) >= n && forall(i, 0, n, ident(arr[i]) == arr[i]);
ensures: result > 0;
body: arr[0];
atom caller_ident(arr: [i64], n: i64) -> i64
requires: n >= 1 && len(arr) >= n;
ensures: result > 0;
body: needs_ident(arr, n);
"#
        ),
    );
    let text = output_text(&output);
    assert_eq!(
        output.status.code(),
        Some(3),
        "expected inconclusive exit 3:\n{text}"
    );
    assert!(
        text.contains("'caller_ident': unverifiable"),
        "expected caller_ident unverifiable:\n{text}"
    );
    std::fs::remove_dir_all(dir).ok();
}

// P3: a forall in the atom's own requires whose condition calls an atom on
// the bound variable fails closed the same way.
#[test]
fn own_forall_call_requires_is_unverifiable() {
    let (dir, output) = verify_plain(
        "probe_forall_call",
        &format!(
            r#"{IDENT}
atom probe_forall_call(arr: [i64], n: i64) -> i64
requires: n >= 2 && len(arr) >= n && forall(i, 0, n, ident(arr[i]) == arr[i]);
ensures: arr[0] == arr[n - 1];
body: n;
"#
        ),
    );
    let text = output_text(&output);
    assert_eq!(
        output.status.code(),
        Some(3),
        "expected inconclusive exit 3:\n{text}"
    );
    assert!(
        text.contains("'probe_forall_call': unverifiable"),
        "expected probe_forall_call unverifiable:\n{text}"
    );
    std::fs::remove_dir_all(dir).ok();
}

// A7: repeated `requires:` clauses keep the quantifier extractable.
#[test]
fn repeated_requires_clause_forall_is_enforced_at_call_sites() {
    let (dir, output, payload) = verify_json(
        "caller_bad_repeated",
        r#"
atom needs_pos_rep(arr: [i64], n: i64) -> i64
requires: n >= 1 && len(arr) >= n;
requires: forall(i, 0, n, arr[i] > 0);
ensures: result > 0;
body: arr[0];

atom caller_bad_rep(arr: [i64], n: i64) -> i64
requires: n >= 1 && len(arr) >= n;
ensures: result > 0;
body: needs_pos_rep(arr, n);
"#,
    );
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert_failed_with(&payload, "caller_bad_rep", &dir, "precondition_violated");
    std::fs::remove_dir_all(dir).ok();
}

// B1: a forall nested under `||` stays in the requires text; it no longer
// makes the precondition vacuous, so the body must actually prove the
// postcondition.
#[test]
fn forall_under_disjunction_is_not_a_free_fact() {
    let (dir, output, payload) = verify_json(
        "disj",
        r#"
atom disj(arr: [i64], n: i64) -> i64
requires: n >= 1 && len(arr) >= n && (n == 5 || forall(i, 0, n, arr[i] > 0));
ensures: result > 0;
body: arr[0];
"#,
    );
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    let diag = atom_diagnostic(&payload, "disj")
        .unwrap_or_else(|| panic!("no diagnostic for disj:\n{payload:#}"));
    assert_eq!(diag["code"], "failed", "{diag:#}");
    assert_eq!(
        atom_failure_type(&dir, &payload, "disj"),
        "postcondition_violated"
    );
    assert!(
        !text.contains("requires clause is unsatisfiable"),
        "disj must not report an unsatisfiable requires: {text}"
    );
    std::fs::remove_dir_all(dir).ok();
}

// B2: a negated forall also stays in place — the requires is satisfiable
// and the failure is a real postcondition failure.
#[test]
fn negated_forall_is_not_unsatisfiable_requires() {
    let (dir, output, payload) = verify_json(
        "neg",
        r#"
atom neg(arr: [i64], n: i64) -> i64
requires: n >= 1 && len(arr) >= n && !forall(i, 0, n, arr[i] > 0);
ensures: result > 0;
body: arr[0];
"#,
    );
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    let diag = atom_diagnostic(&payload, "neg")
        .unwrap_or_else(|| panic!("no diagnostic for neg:\n{payload:#}"));
    assert_eq!(diag["code"], "failed", "{diag:#}");
    assert_eq!(
        atom_failure_type(&dir, &payload, "neg"),
        "postcondition_violated"
    );
    assert!(
        !text.contains("requires clause is unsatisfiable"),
        "neg must not report an unsatisfiable requires: {text}"
    );
    std::fs::remove_dir_all(dir).ok();
}

// B3: a body that discharges the disjunct either way verifies.
#[test]
fn forall_under_disjunction_still_constrains_the_requires() {
    let (dir, output, payload) = verify_json(
        "disj_ok",
        r#"
atom disj_ok(arr: [i64], n: i64) -> i64
requires: n >= 1 && len(arr) >= n && (n == 5 || forall(i, 0, n, arr[i] > 0));
ensures: result > 0;
body: { if n != 5 { arr[0] } else { 1 } };
"#,
    );
    assert_verified(&output, &payload);
    std::fs::remove_dir_all(dir).ok();
}

// R1: the classic top-level forall keeps verifying end to end.
#[test]
fn top_level_forall_requires_still_verifies() {
    let (dir, output, payload) = verify_json(
        "top_level",
        r#"
atom top_level(arr: [i64], n: i64) -> i64
requires: n >= 1 && len(arr) >= n && forall(i, 0, n, arr[i] >= 0);
ensures: result >= 0;
body: arr[0];
"#,
    );
    assert_verified(&output, &payload);
    std::fs::remove_dir_all(dir).ok();
}

// C1: comparison-chain normalization applies inside a quantifier body.
// The parenthesized equality is a boolean operand, while the independent
// flat ordering chain is normalized locally.
#[test]
fn quantifier_condition_normalizes_comparison_chains_with_parentheses() {
    let (dir, output, payload) = verify_json(
        "quantified_chain",
        r#"
atom quantified_chain(arr: [i64], n: i64) -> i64
requires: n >= 1 && len(arr) >= n && forall(i, 0, n, (arr[i] == arr[i]) == true && 0 <= arr[i] < 10);
ensures: result >= 0 && result < 10;
body: arr[0];
"#,
    );
    assert_verified(&output, &payload);
    std::fs::remove_dir_all(dir).ok();
}

// C2: a nested comparison in a duplicated middle-chain operand follows the
// same fail-closed policy as an ordinary contract clause. It must not be
// silently reinterpreted as a different left-associative proposition.
#[test]
fn quantifier_condition_rejects_nested_comparison_chain_operand() {
    let (dir, output, _payload) = verify_json(
        "quantified_nested_chain",
        r#"
atom quantified_nested_chain(arr: [i64], n: i64, x: i64, y: i64) -> i64
requires: n >= 1 && len(arr) >= n && forall(i, 0, n, arr[i] == x < y == arr[i]);
ensures: result >= 0;
body: arr[0];
"#,
    );
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert!(
        text.contains("nested comparison inside a comparison-chain operand"),
        "expected fail-closed nested-chain diagnostic:\n{text}"
    );
    std::fs::remove_dir_all(dir).ok();
}

// C3: checked parsing also applies to quantifier bounds, so trailing input
// is not silently dropped into a weaker range expression.
#[test]
fn quantifier_end_rejects_trailing_input() {
    let (dir, output, _payload) = verify_json(
        "quantified_bad_end",
        r#"
atom quantified_bad_end(arr: [i64], n: i64) -> i64
requires: n >= 1 && len(arr) >= n && forall(i, 0, n - 1 trailing, arr[i] >= 0);
ensures: result >= 0;
body: arr[0];
"#,
    );
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert!(
        text.contains("unexpected trailing input"),
        "expected fail-closed trailing-input diagnostic:\n{text}"
    );
    std::fs::remove_dir_all(dir).ok();
}

// C4: one trailing comma in the quantifier argument list is a separator,
// not part of the condition expression.
#[test]
fn quantifier_condition_accepts_one_trailing_argument_comma() {
    let (dir, output, payload) = verify_json(
        "quantified_trailing_comma",
        r#"
atom quantified_trailing_comma(arr: [i64], n: i64) -> i64
requires: n >= 1 && len(arr) >= n && forall(i, 0, n, arr[i] >= 0,);
ensures: result >= 0;
body: arr[0];
"#,
    );
    assert_verified(&output, &payload);
    std::fs::remove_dir_all(dir).ok();
}

// C5: even for a trusted callee, a direct caller must not reinterpret a
// nested middle-chain comparison through the reconstructed precondition.
#[test]
fn direct_call_rejects_nested_chain_in_reconstructed_quantified_precondition() {
    let (dir, output, _payload) = verify_json(
        "direct_nested_quantified",
        r#"
trusted atom nested_quantified_req(arr: [i64], n: i64, x: i64, y: i64) -> i64
requires: forall(i, 0, n, arr[i] == x < y == arr[i]);
ensures: result == 0;
body: 0;

atom direct_nested_quantified(arr: [i64], n: i64, x: i64, y: i64) -> i64
requires: n >= 1 && len(arr) >= n;
ensures: result == 0;
body: nested_quantified_req(arr, n, x, y);
"#,
    );
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert!(
        text.contains("nested comparison inside a comparison-chain operand"),
        "expected fail-closed nested-chain diagnostic:\n{text}"
    );
    std::fs::remove_dir_all(dir).ok();
}

// C6: `call(atom_ref(...))` follows the same checked-precondition policy as
// a direct call.
#[test]
fn atom_ref_call_rejects_nested_chain_in_reconstructed_quantified_precondition() {
    let (dir, output, _payload) = verify_json(
        "ref_nested_quantified",
        r#"
trusted atom nested_quantified_req(arr: [i64], n: i64, x: i64, y: i64) -> i64
requires: forall(i, 0, n, arr[i] == x < y == arr[i]);
ensures: result == 0;
body: 0;

atom ref_nested_quantified(arr: [i64], n: i64, x: i64, y: i64) -> i64
requires: n >= 1 && len(arr) >= n;
ensures: result == 0;
body: call(atom_ref(nested_quantified_req), arr, n, x, y);
"#,
    );
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert!(
        text.contains("nested comparison inside a comparison-chain operand"),
        "expected fail-closed nested-chain diagnostic:\n{text}"
    );
    std::fs::remove_dir_all(dir).ok();
}

const TRAILING_COMMA_CALLEE: &str = r#"
atom trailing_comma_callee(arr: [i64], n: i64) -> i64
requires: n >= 1 && len(arr) >= n && forall(i, 0, n, arr[i] > 0,);
ensures: result > 0;
body: arr[0];
"#;

// C7: a direct caller accepts the reconstructed quantified precondition of
// a callee whose source used one trailing argument comma.
#[test]
fn direct_call_accepts_reconstructed_precondition_from_trailing_comma_callee() {
    let (dir, output, payload) = verify_json(
        "direct_trailing_comma",
        &format!(
            r#"{TRAILING_COMMA_CALLEE}
atom direct_trailing_comma(arr: [i64], n: i64) -> i64
requires: n >= 1 && len(arr) >= n && forall(i, 0, n, arr[i] > 0);
ensures: result > 0;
body: trailing_comma_callee(arr, n);
"#
        ),
    );
    assert_verified(&output, &payload);
    std::fs::remove_dir_all(dir).ok();
}

// C8: `call(atom_ref(...))` accepts the same reconstructed quantified
// precondition through its separate call path.
#[test]
fn atom_ref_call_accepts_reconstructed_precondition_from_trailing_comma_callee() {
    let (dir, output, payload) = verify_json(
        "ref_trailing_comma",
        &format!(
            r#"{TRAILING_COMMA_CALLEE}
atom ref_trailing_comma(arr: [i64], n: i64) -> i64
requires: n >= 1 && len(arr) >= n && forall(i, 0, n, arr[i] > 0);
ensures: result > 0;
body: call(atom_ref(trailing_comma_callee), arr, n);
"#
        ),
    );
    assert_verified(&output, &payload);
    std::fs::remove_dir_all(dir).ok();
}
