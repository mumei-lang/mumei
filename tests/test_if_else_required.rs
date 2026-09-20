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

#[test]
fn libc_wrapper_tests_verify_without_panic() {
    for file in ["tests/test_libc.mm", "tests/test_libc_contracts.mm"] {
        let output = mumei_verify(file);
        let out = combined(&output);
        assert!(
            output.status.success() && out.contains("Verification passed"),
            "{file} must verify cleanly (regression: parser panicked on these):\n{out}"
        );
    }
}

#[test]
fn if_branch_guard_satisfies_call_requires() {
    let output = mumei_verify("tests/test_if_guard_requires.mm");
    let out = combined(&output);
    assert!(
        output.status.success() && out.contains("Verification passed"),
        "a call under `if x >= 0` must see the guard when checking requires:\n{out}"
    );
}

#[test]
fn if_without_else_fails_closed_not_panic() {
    let output = mumei_verify("tests/test_if_missing_else_negative.mm");
    let out = combined(&output);
    assert!(
        !output.status.success() && out.contains("requires an 'else' branch"),
        "missing else must be a clean syntax error:\n{out}"
    );
    assert!(
        !out.contains("internal error") && !out.contains("panicked"),
        "missing else must not panic the binary:\n{out}"
    );
}

#[test]
fn while_without_invariant_fails_closed_not_panic() {
    let output = mumei_verify("tests/test_while_missing_invariant_negative.mm");
    let out = combined(&output);
    assert!(
        !output.status.success() && out.contains("requires an 'invariant'"),
        "missing invariant must be a clean syntax error:\n{out}"
    );
    assert!(
        !out.contains("internal error") && !out.contains("panicked"),
        "missing invariant must not panic the binary:\n{out}"
    );
}

#[test]
fn task_group_unknown_join_fails_closed_not_panic() {
    let output = mumei_verify("tests/test_task_group_bad_join_negative.mm");
    let out = combined(&output);
    assert!(
        !output.status.success() && out.contains("join semantics"),
        "unknown join semantics must be a clean syntax error:\n{out}"
    );
    assert!(
        !out.contains("internal error") && !out.contains("panicked"),
        "unknown join semantics must not panic the binary:\n{out}"
    );
}
