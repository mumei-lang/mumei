use std::path::PathBuf;
use std::process::{Command, Output};

fn verify_source(name: &str, source: &str) -> (PathBuf, Output) {
    let dir = std::env::temp_dir().join(format!(
        "mumei_refined_boundaries_{}_{}_{}",
        std::process::id(),
        name,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).expect("create fixture directory");
    let fixture = dir.join(format!("{name}.mm"));
    std::fs::write(&fixture, source).expect("write fixture");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--report-dir")
        .arg(&dir)
        .arg(&fixture)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run mumei verify");
    (dir, output)
}

fn output_text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn cleanup(dir: PathBuf) {
    std::fs::remove_dir_all(dir).expect("remove fixture directory");
}

#[test]
fn chained_alias_inherits_ancestor_predicate_as_postcondition() {
    // `Small = Pos where v < 10` must carry `Pos`'s `v > 0`: the body just
    // returns the parameter, so `result > 0` only verifies when the ancestor
    // conjunct is applied to the parameter.
    let (dir, output) = verify_source(
        "chain_post",
        r#"
type Pos = i64 where v > 0;
type Small = Pos where v < 10;
atom f(x: Small) -> i64
requires: true;
ensures: result > 0;
body: { x };
"#,
    );
    let text = output_text(&output);
    assert!(
        output.status.success(),
        "ancestor predicate must establish result > 0: {text}"
    );
    cleanup(dir);
}

#[test]
fn chained_alias_rejects_call_that_violates_ancestor_predicate() {
    // `-5` satisfies `Small`'s own `v < 10` but violates `Pos`'s `v > 0` —
    // the call site must reject it.
    let (dir, output) = verify_source(
        "chain_callsite",
        r#"
type Pos = i64 where v > 0;
type Small = Pos where v < 10;
atom callee(x: Small) -> i64
requires: true;
ensures: true;
body: { x };
atom caller() -> i64
requires: true;
ensures: true;
body: { callee(-5) };
"#,
    );
    let text = output_text(&output);
    assert!(
        !output.status.success(),
        "call violating an ancestor predicate must fail: {text}"
    );
    assert!(text.contains("refinement on parameter 'x'"), "{text}");
    cleanup(dir);
}

#[test]
fn refined_return_type_is_a_postcondition() {
    // A body returning `-5` for a `-> Pos` result must not verify.
    let (dir, output) = verify_source(
        "return_refinement",
        r#"
type Pos = i64 where v > 0;
atom bad() -> Pos
requires: true;
ensures: true;
body: { -5 };
"#,
    );
    let text = output_text(&output);
    assert!(
        !output.status.success(),
        "a result violating its refined return type must fail: {text}"
    );
    assert!(
        text.contains("Return type 'Pos' refinement is not satisfied"),
        "{text}"
    );
    cleanup(dir);
}

#[test]
fn refined_return_type_accepts_satisfying_result() {
    let (dir, output) = verify_source(
        "return_refinement_ok",
        r#"
type Pos = i64 where v > 0;
atom good() -> Pos
requires: true;
ensures: true;
body: { 5 };
"#,
    );
    let text = output_text(&output);
    assert!(
        output.status.success(),
        "a result satisfying its refined return type must verify: {text}"
    );
    cleanup(dir);
}

#[test]
fn refined_parameter_is_checked_at_call_site() {
    // A call `need(5)` for `need(x: Neg)` must fail even though `need`
    // has no explicit requires clause.
    let (dir, output) = verify_source(
        "param_callsite",
        r#"
type Neg = i64 where v < 0;
atom need(x: Neg) -> i64
requires: true;
ensures: true;
body: { x };
atom caller() -> i64
requires: true;
ensures: true;
body: { need(5) };
"#,
    );
    let text = output_text(&output);
    assert!(
        !output.status.success(),
        "a call violating a refined parameter must fail: {text}"
    );
    assert!(text.contains("refinement on parameter 'x' (Neg)"), "{text}");
    cleanup(dir);
}

#[test]
fn refined_parameter_accepts_satisfying_call() {
    let (dir, output) = verify_source(
        "param_callsite_ok",
        r#"
type Neg = i64 where v < 0;
atom need(x: Neg) -> i64
requires: true;
ensures: true;
body: { x };
atom caller() -> i64
requires: true;
ensures: true;
body: { need(-5) };
"#,
    );
    let text = output_text(&output);
    assert!(
        output.status.success(),
        "a call satisfying the refined parameter must verify: {text}"
    );
    cleanup(dir);
}

#[test]
fn bool_base_alias_binds_the_operand_as_bool() {
    // With the operand bound as Bool, `x == false` contradicts `v == true`
    // and must be refuted — not vacuously discharged.
    let (dir, output) = verify_source(
        "bool_base",
        r#"
type Flag = bool where v == true;
atom f(x: Flag) -> bool
requires: true;
ensures: result == false;
body: { x };
"#,
    );
    let text = output_text(&output);
    assert!(
        !output.status.success(),
        "a bool-refined operand must be refutable: {text}"
    );
    cleanup(dir);
}

#[test]
fn caller_gains_callees_refined_return_on_the_call_result() {
    // `need(-5)` returns a `Neg`: the caller may assume `v < 0` on the call
    // result, so `result < 0` verifies without an explicit ensures clause.
    let (dir, output) = verify_source(
        "caller_gains_return",
        r#"
type Neg = i64 where v < 0;
atom need(x: Neg) -> Neg
requires: true;
ensures: true;
body: { x };
atom caller() -> i64
requires: true;
ensures: result < 0;
body: { let y = need(-5); y };
"#,
    );
    let text = output_text(&output);
    assert!(
        output.status.success(),
        "the caller must gain the callee's refined return: {text}"
    );
    cleanup(dir);
}

#[test]
fn atom_ref_call_site_checks_refined_params() {
    // `call(atom_ref(need), 5)` must fail exactly like `need(5)` — an
    // indirect call cannot bypass the callee's declared refinement.
    let (dir, output) = verify_source(
        "atom_ref_callsite",
        r#"
type Neg = i64 where v < 0;
atom need(x: Neg) -> i64
requires: true;
ensures: true;
body: { x };
atom caller() -> i64
requires: true;
ensures: true;
body: { call(atom_ref(need), 5) };
"#,
    );
    let text = output_text(&output);
    assert!(
        !output.status.success(),
        "an atom_ref call violating a refined parameter must fail: {text}"
    );
    assert!(text.contains("refinement on parameter 'x'"), "{text}");
    cleanup(dir);
}

#[test]
fn atom_ref_call_site_respects_branch_conditions() {
    // `call(atom_ref(need), x)` under `x < 0` satisfies `Neg`: the check must
    // evaluate the argument under the enclosing branch's path condition, not
    // unconditionally.
    let (dir, output) = verify_source(
        "atom_ref_guarded",
        r#"
type Neg = i64 where v < 0;
atom need(x: Neg) -> i64
requires: true;
ensures: true;
body: { x };
atom caller(x: i64) -> i64
requires: true;
ensures: true;
body: { if x < 0 { call(atom_ref(need), x) } else { 0 } };
"#,
    );
    let text = output_text(&output);
    assert!(
        output.status.success(),
        "a branch-guarded atom_ref call satisfying the refinement must verify: {text}"
    );
    cleanup(dir);
}

#[test]
fn chained_bool_alias_binds_the_parameter_as_bool() {
    // `F2 = Flag = bool` must mint the parameter as a Bool — not an Int that
    // the refinement conjuncts cannot constrain. `x == true` input with
    // `result == false` must be refuted.
    let (dir, output) = verify_source(
        "bool_chain",
        r#"
type Flag = bool where v == true;
type F2 = Flag;
atom f(x: F2) -> bool
requires: true;
ensures: result == false;
body: { x };
"#,
    );
    let text = output_text(&output);
    assert!(
        !output.status.success(),
        "a chained bool-refined operand must be refutable: {text}"
    );
    cleanup(dir);
}

#[test]
fn chained_f64_alias_verifies_against_the_leaf_sort() {
    // `P2 = Pos = f64` must mint the parameter as Real so the leaf predicate
    // establishes `result > 0`.
    let (dir, output) = verify_source(
        "f64_chain",
        r#"
type Pos = f64 where v > 0.0;
type P2 = Pos;
atom keep(x: P2) -> f64
requires: true;
ensures: result > 0.0;
body: { x };
"#,
    );
    let text = output_text(&output);
    assert!(
        output.status.success(),
        "a chained f64 alias must verify against the leaf sort: {text}"
    );
    cleanup(dir);
}

#[test]
fn cyclic_alias_declarations_are_rejected() {
    // `type A = B; type B = A;` encodes no base and no constraints — a type
    // error, not a constraint-free alias.
    let (dir, output) = verify_source(
        "cycle",
        r#"
type A = B where v > 0;
type B = A where v < 0;
atom f(x: A) -> i64
requires: true;
ensures: result == x;
body: { x };
"#,
    );
    let text = output_text(&output);
    assert!(!output.status.success(), "cyclic aliases must fail: {text}");
    assert!(text.contains("cyclic refined type declaration"), "{text}");
    cleanup(dir);
}
