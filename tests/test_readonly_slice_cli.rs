use mumei_core::semantic_contract::readonly_slice::PROFILE;
use serde_json::Value;
use std::{
    fs,
    process::{Command, Output},
};

const GET: &str = "atom get(ref a: [i64], i: i64) -> i64
requires: i >= 0 && i < len(a);
ensures: result == a[i];
body: { a[i] }";

fn check(source: &str) -> Output {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("input.mm");
    fs::write(&path, source).unwrap();
    Command::new(env!("CARGO_BIN_EXE_mumei"))
        .current_dir(directory.path())
        .args(["evidence", "check-readonly-slice"])
        .arg(path)
        .output()
        .unwrap()
}

fn diagnostic(source: &str, code: i32) -> Value {
    let output = check(source);
    assert_eq!(
        output.status.code(),
        Some(code),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn last_check(diagnostic: &Value) -> (&str, &str) {
    let last = diagnostic["checks"].as_array().unwrap().last().unwrap();
    (
        last["obligation"].as_str().unwrap(),
        last["z3"].as_str().unwrap(),
    )
}

#[test]
fn proves_guarded_loads_and_reports_source_only_scope() {
    let result = diagnostic(GET, 0);
    assert_eq!(result["profile"], PROFILE);
    assert_eq!(result["status"], "established_within_profile");
    assert_eq!(result["evidence_acceptance"], "not_evaluated");
    assert_eq!(result["correspondence"], "unchecked");
    assert_eq!(result["lean"], "unsupported");
    assert_eq!(result["llvm"], "unsupported");
    assert_eq!(result["binders"][0]["scalar_type"], "ref [i64]");
    assert_eq!(last_check(&result), ("postcondition", "unsat"));

    let conditional = "atom first(ref a: [i64]) -> i64
requires: len(a) == 0 || a[0] == a[0];
ensures: len(a) == 0 || result == a[0];
body: { if len(a) > 0 { a[0] } else { 0 } }";
    assert_eq!(
        diagnostic(conditional, 0)["status"],
        "established_within_profile"
    );
}

#[test]
fn refutes_unguarded_loads_in_each_clause_and_a_false_postcondition() {
    for (source, failure) in [
        (
            GET.replace("i >= 0 && i < len(a)", "a[i] == a[i]"),
            "requires_bounds",
        ),
        (GET.replace("i >= 0 && i < len(a)", "true"), "body_bounds"),
        (
            GET.replace("result == a[i]", "result == a[len(a)]"),
            "ensures_bounds",
        ),
        (
            GET.replace("result == a[i]", "result != a[i]"),
            "postcondition",
        ),
    ] {
        let result = diagnostic(&source, 1);
        assert_eq!(result["status"], "refuted");
        assert_eq!(last_check(&result), (failure, "sat"));
    }
}

#[test]
fn refuses_vacuous_and_fixed_alias_assumptions() {
    let empty = diagnostic(&GET.replace("i >= 0 && i < len(a)", "false"), 3);
    assert_eq!(empty["status"], "empty_input_domain");
    assert_eq!(last_check(&empty), ("input_domain", "unsat"));

    let cross_formal = "atom cross(ref a: [i64], ref b: [i64]) -> i64
requires: len(a) > 0 && len(b) > 0;
ensures: result == b[0];
body: { a[0] }";
    assert_eq!(
        last_check(&diagnostic(cross_formal, 1)),
        ("postcondition", "sat")
    );
}

#[test]
fn rejects_unsupported_syntax_without_a_diagnostic_record() {
    for source in [
        GET.replace("ref a", "ref mut a"),
        GET.replace("ref a", "a"),
        GET.replace("[i64]", "[f64]"),
        GET.replace("{ a[i] }", "{ a[0] = 0; a[0] }"),
        GET.replace("{ a[i] }", "{ slice(a, i) }"),
        GET.replace("{ a[i] }", "{ let other = a; other[i] }"),
        GET.replace("{ a[i] }", "{ a }"),
        GET.replace("{ a[i] }", "{ a[i] + 1 }"),
        GET.replace("{ a[i] }", "{ a[i]"),
        GET.replace(
            "requires:",
            "contract missing: { requires: false; } requires:",
        ),
        GET.replace("requires:", "stray requires:"),
        GET.replace("{ a[i] }", "{ a[i] } stray"),
        GET.replace(", i:", ",, i:"),
        GET.replace("ensures: result == a[i];", ""),
        GET.replace("requires: i >= 0 && i < len(a);", "requires: forall(i);"),
        GET.replace(
            "requires: i >= 0 && i < len(a);",
            "requires: i < (0 < len(a)) < 2;",
        ),
        GET.replace("body: { a[i] }", "body: { a[i]. }"),
        GET.replace("body: { a[i] }", "body: { a[i].foo }"),
    ] {
        let output = check(&source);
        assert_eq!(output.status.code(), Some(1), "{source}");
        assert!(output.stdout.is_empty(), "{source}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("Read-only slice check failed"),
            "{source}"
        );
    }
}

#[test]
fn braced_and_unbraced_conditionals_normalize_comparison_chains_the_same_way() {
    let unbraced = "atom pick(ref a: [i64], i: i64) -> i64
requires: 0 <= i && i < len(a);
ensures: result == 1;
body: if 0 <= i < len(a) { 1 } else { 0 }";
    let braced = unbraced
        .replace("body: if", "body: { if")
        .replace("else { 0 }", "else { 0 } }");
    assert_eq!(
        diagnostic(unbraced, 0)["status"],
        diagnostic(&braced, 0)["status"]
    );
}
