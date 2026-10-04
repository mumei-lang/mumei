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
    // The forall makes arr[0] <= 0 unreachable, so the cover must not be
    // reported covered with a witness. cover_results live on report.json
    // (cov is the last atom) or on the stdout payload.
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
    assert_ne!(
        cover_status.as_deref(),
        Some("covered"),
        "cover reported covered with a violating witness:\n{text}"
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
