//! Calls inside `forall` / `exists` whose arguments depend on the bound
//! variable lower to a single result constant shared by every instance of
//! the binder, which used to make unsound facts provable. Such calls now
//! fail closed: the atom is reported as unverifiable, and a cover clause as
//! unknown.
use std::path::PathBuf;
use std::process::{Command, Output};

const MARKER: &str =
    "calls whose arguments depend on a quantifier-bound variable are not supported yet";

const IDENT: &str = r#"
atom ident(x: i64)
    requires: true;
    ensures: result == x;
    body: x;
"#;

struct Run {
    dir: PathBuf,
    output: Output,
    text: String,
}

impl Run {
    fn code(&self) -> Option<i32> {
        self.output.status.code()
    }

    fn report(&self) -> serde_json::Value {
        serde_json::from_slice(
            &std::fs::read(self.dir.join("reports").join("report.json")).expect("read report.json"),
        )
        .expect("parse report.json")
    }

    /// The diagnostic shows up on stderr for atoms that fail outright and
    /// in `report.json` diagnostics for skipped clauses.
    fn mentions_marker(&self) -> bool {
        self.text.contains(MARKER) || self.report()["diagnostics"].to_string().contains(MARKER)
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Runs `mumei verify` on `IDENT` followed by `atom_src`, in a fresh
/// directory so no `.mumei_cache` from an earlier run can answer.
fn verify(name: &str, atom_src: &str) -> Run {
    let dir = std::env::temp_dir().join(format!(
        "mumei_quantifier_calls_{}_{}_{}",
        std::process::id(),
        name,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(dir.join("reports")).expect("create fixture directory");
    let fixture = dir.join("main.mm");
    std::fs::write(&fixture, format!("{IDENT}\n{atom_src}\n")).expect("write fixture");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--report-dir")
        .arg(dir.join("reports"))
        .arg(&fixture)
        .current_dir(&dir)
        .output()
        .expect("run mumei verify");
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Run { dir, output, text }
}

fn assert_unverifiable(run: &Run, atom: &str) {
    assert_eq!(
        run.code(),
        Some(3),
        "expected inconclusive exit 3:\n{}",
        run.text
    );
    assert!(
        run.text.contains(&format!("'{atom}': unverifiable")),
        "expected '{atom}' to be unverifiable:\n{}",
        run.text
    );
    assert!(
        run.mentions_marker(),
        "expected the quantifier-call diagnostic:\n{}",
        run.text
    );
}

#[test]
fn call_on_bound_variable_in_requires_does_not_verify() {
    let run = verify(
        "regression",
        r#"
atom all_equal_probe(arr: [i64], n: i64)
    requires: n >= 2 && len(arr) >= n && forall(i, 0, n, ident(arr[i]) == arr[i]);
    ensures: arr[0] == arr[1];
    body: n;
"#,
    );
    assert_unverifiable(&run, "all_equal_probe");
}

#[test]
fn call_independent_of_bound_variable_still_verifies() {
    // `ident(k)` does not mention `i`, so one shared result is exactly right:
    // every element equals the same value.
    let run = verify(
        "independent",
        r#"
atom same_call(arr: [i64], n: i64, k: i64)
    requires: n >= 2 && len(arr) >= n && forall(i, 0, n, arr[i] == ident(k));
    ensures: arr[0] == arr[1] && forall(j, 0, n, arr[j] == ident(k) || arr[j] == arr[0]);
    body: n;
"#,
    );
    assert_eq!(run.code(), Some(0), "{}", run.text);
    assert!(run.text.contains("'same_call': verified"), "{}", run.text);
    assert!(!run.mentions_marker(), "{}", run.text);
}

#[test]
fn nested_quantifier_call_on_inner_binder_does_not_verify() {
    let run = verify(
        "nested_inner",
        r#"
atom nested_inner(arr: [i64], n: i64)
    requires: n >= 2 && len(arr) >= n && forall(i, 0, n, forall(j, 0, n, ident(arr[j]) == arr[j]));
    ensures: arr[0] == arr[1];
    body: n;
"#,
    );
    assert_unverifiable(&run, "nested_inner");
}

#[test]
fn nested_quantifier_call_on_outer_binder_is_unverifiable() {
    let run = verify(
        "nested_outer",
        r#"
atom nested_outer(arr: [i64], n: i64)
    requires: n >= 2 && len(arr) >= n && forall(i, 0, n, exists(j, 0, n, ident(arr[i]) == arr[j]));
    ensures: arr[0] == arr[1];
    body: n;
"#,
    );
    assert_unverifiable(&run, "nested_outer");
}

#[test]
fn exists_call_on_bound_variable_is_unverifiable() {
    let ensures = verify(
        "exists_ensures",
        r#"
atom exists_ens(arr: [i64], n: i64)
    requires: n >= 1 && len(arr) >= n && arr[0] == 7;
    ensures: exists(i, 0, n, ident(arr[i]) == 7);
    body: n;
"#,
    );
    assert_unverifiable(&ensures, "exists_ens");
    assert_eq!(
        ensures.report()["status"],
        "unverifiable",
        "{}",
        ensures.text
    );

    let requires = verify(
        "exists_requires",
        r#"
atom exists_req(arr: [i64], n: i64)
    requires: n >= 2 && len(arr) >= n && arr[0] == 1 && exists(i, 0, n, ident(arr[i]) == 2);
    ensures: arr[1] == 2;
    body: n;
"#,
    );
    assert_unverifiable(&requires, "exists_req");
}

#[test]
fn quantifier_call_in_body_is_unverifiable() {
    let run = verify(
        "body",
        r#"
atom body_q(arr: [i64], n: i64)
    requires: n >= 2 && len(arr) >= n;
    ensures: result == 1;
    body: {
        let ok = forall(i, 0, n, ident(arr[i]) == arr[i]);
        if ok { 1 } else { 1 }
    };
"#,
    );
    assert_unverifiable(&run, "body_q");
}

#[test]
fn quantifier_call_in_loop_invariant_is_unverifiable() {
    let run = verify(
        "invariant",
        r#"
atom inv_q(arr: [i64], n: i64)
    requires: n >= 0 && len(arr) >= n;
    ensures: result == n;
    body: {
        let i = 0;
        while i < n
        invariant: i >= 0 && i <= n && forall(k, 0, i, ident(arr[k]) == arr[k])
        decreases: n - i
        {
            i = i + 1;
        };
        i
    };
"#,
    );
    assert_unverifiable(&run, "inv_q");
}

#[test]
fn quantifier_call_in_cover_is_unknown() {
    // Before the fix the shared result made this cover look unreachable.
    let run = verify(
        "cover",
        r#"
atom cover_q(arr: [i64], n: i64)
    requires: n >= 2 && len(arr) >= n;
    ensures: result == n;
    cover "all_equal": forall(i, 0, n, ident(arr[i]) == arr[i]) && arr[0] != arr[1];
    body: n;
"#,
    );
    assert!(!run.text.contains("is unreachable"), "{}", run.text);
    let report = run.report();
    assert_eq!(report["atom"], "cover_q", "{}", run.text);
    assert_eq!(
        report["cover_results"][0]["status"], "unknown",
        "{}",
        run.text
    );
    assert!(run.mentions_marker(), "{}", run.text);
}

#[test]
fn lambda_atom_ref_and_dynamic_calls_on_bound_variable_are_unverifiable() {
    let lambda = verify(
        "lambda",
        r#"
atom lambda_q(arr: [i64], n: i64)
    requires: n >= 2 && len(arr) >= n;
    ensures: result == 1;
    body: {
        let f = |x| ident(x);
        let ok = forall(i, 0, n, f(arr[i]) == arr[i]);
        if ok { 1 } else { 1 }
    };
"#,
    );
    assert_unverifiable(&lambda, "lambda_q");

    let atom_ref = verify(
        "atom_ref",
        r#"
atom ref_q(arr: [i64], n: i64)
    requires: n >= 2 && len(arr) >= n && forall(i, 0, n, call(atom_ref(ident), arr[i]) == arr[i]);
    ensures: arr[0] == arr[1];
    body: n;
"#,
    );
    assert_unverifiable(&atom_ref, "ref_q");

    let dynamic = verify(
        "dynamic",
        r#"
atom dyn_q(arr: [i64], n: i64, f: atom_ref(i64) -> i64)
    requires: n >= 2 && len(arr) >= n && forall(i, 0, n, call(f, arr[i]) == arr[i]);
    ensures: arr[0] == arr[1];
    contract(f): ensures: result >= 0;
    body: n;
"#,
    );
    assert_unverifiable(&dynamic, "dyn_q");
}
