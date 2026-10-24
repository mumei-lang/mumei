use std::process::Command;

fn run(file: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("run")
        .arg(file)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei run {file}: {err}"))
}

fn verify(file: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mumei"))
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

fn assert_run(file: &str, expected: i32) {
    let output = run(file);
    let text = combined(&output);
    assert_eq!(output.status.code(), Some(expected), "{file}\n{text}");
}

#[test]
fn array_reassignment_fixtures_run() {
    assert_run("tests/positive/array_reassign_straight.mm", 22);
    assert_run("tests/positive/array_reassign_branch.mm", 32);
    assert_run("tests/positive/array_reassign_match.mm", 22);
    assert_run("tests/positive/array_alias_branch.mm", 22);
    assert_run("tests/positive/array_branch_local_scope.mm", 12);
    assert_run("tests/positive/array_branch_store.mm", 50);
    assert_run("tests/positive/array_loop_shadow.mm", 7);
}

#[test]
fn array_reassignment_failures_stay_fail_closed() {
    let output = run("tests/negative/array_reassign_in_loop_codegen.mm");
    let text = combined(&output);
    assert!(!output.status.success(), "{text}");
    assert!(text.contains("reassigned inside a loop"), "{text}");

    let output = run("tests/negative/array_reassign_from_call_codegen.mm");
    let text = combined(&output);
    assert!(!output.status.success(), "{text}");
    assert!(
        text.contains("can only be reassigned from an array literal"),
        "{text}"
    );

    let output = run("tests/negative/array_reassign_in_loop_nested_codegen.mm");
    let text = combined(&output);
    assert!(!output.status.success(), "{text}");
    assert!(text.contains("reassigned inside a loop"), "{text}");
}

#[test]
fn array_reassignment_fixtures_verify_before_codegen() {
    for file in [
        "tests/positive/array_reassign_straight.mm",
        "tests/positive/array_reassign_branch.mm",
        "tests/positive/array_reassign_match.mm",
        "tests/positive/array_alias_branch.mm",
        "tests/positive/array_branch_local_scope.mm",
        "tests/positive/array_branch_store.mm",
        "tests/positive/array_loop_shadow.mm",
        "tests/negative/array_reassign_in_loop_codegen.mm",
        "tests/negative/array_reassign_in_loop_nested_codegen.mm",
        "tests/negative/array_reassign_from_call_codegen.mm",
    ] {
        let output = verify(file);
        let text = combined(&output);
        assert!(output.status.success(), "{file}\n{text}");
    }
}
