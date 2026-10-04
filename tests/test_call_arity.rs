//! A call whose argument count does not match the callee's params must be
//! rejected with an arity error. Before the check, extra arguments were
//! silently dropped and missing arguments surfaced only as an unrelated
//! "precondition (requires) not satisfied" at the call site.
use std::process::Command;

fn mumei_verify(file: &str) -> std::process::Output {
    let bin = env!("CARGO_BIN_EXE_mumei");
    Command::new(bin)
        .arg("verify")
        .arg(file)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify {file}: {err}"))
}

fn combined(output: &std::process::Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn expect_arity_error(file: &str, expected: &str) -> String {
    let output = mumei_verify(file);
    let text = combined(&output);
    assert!(
        !output.status.success(),
        "{file} should fail verification\n{text}"
    );
    assert!(
        text.contains(expected),
        "{file} should report '{expected}'\n{text}"
    );
    text
}

#[test]
fn exact_arity_call_verifies() {
    let output = mumei_verify("tests/test_call_arity.mm");
    assert!(
        output.status.success(),
        "exact-arity call should verify\n{}",
        combined(&output)
    );
}

#[test]
fn too_few_args_reports_arity_not_requires() {
    let text = expect_arity_error(
        "tests/test_call_arity_too_few.mm",
        "expected 2 argument(s), got 1",
    );
    assert!(
        !text.contains("precondition (requires) not satisfied"),
        "missing-arg call must not surface as a requires failure\n{text}"
    );
}

#[test]
fn too_many_args_reports_arity() {
    expect_arity_error(
        "tests/test_call_arity_too_many.mm",
        "expected 2 argument(s), got 3",
    );
}

#[test]
fn wrong_arity_in_ensures_reports_arity() {
    expect_arity_error(
        "tests/test_call_arity_ensures.mm",
        "expected 2 argument(s), got 1",
    );
}

#[test]
fn wrong_arity_qualified_std_call_reports_arity() {
    expect_arity_error(
        "tests/test_call_arity_qualified.mm",
        "expected 2 argument(s), got 1",
    );
}
