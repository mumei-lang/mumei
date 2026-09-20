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

#[test]
fn bang_negation_verifies_in_body_and_specs() {
    let output = mumei_verify("tests/test_bang_not.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success() && combined.contains("Verification passed"),
        "prefix `!` should verify in bodies and requires/ensures clauses:\n{combined}"
    );
}

#[test]
fn bang_on_non_bool_operand_fails_closed() {
    let output = mumei_verify("tests/test_bang_not_negative.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success() && combined.contains("boolean"),
        "`!` on an i64 operand must be rejected, not silently dropped:\n{combined}"
    );
}
