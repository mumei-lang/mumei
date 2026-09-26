use std::process::Command;

fn run_fixture(file: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("run")
        .arg(file)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap_or_else(|err| panic!("failed to run {file}: {err}"))
}

#[test]
fn if_branch_assignment_merges_before_following_expression() {
    let output = run_fixture("tests/positive/codegen_if_assign.mm");
    assert_eq!(
        output.status.code(),
        Some(8),
        "if branch assignment must produce 8\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn match_arm_assignment_merges_before_following_expression() {
    let output = run_fixture("tests/positive/codegen_match_assign.mm");
    assert_eq!(
        output.status.code(),
        Some(8),
        "match arm assignment must produce 8\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
