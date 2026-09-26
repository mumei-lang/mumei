use std::process::Command;

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

#[test]
fn block_expression_fixture_verifies() {
    let output = verify("tests/positive/block_expr_arg.mm");
    let text = combined(&output);
    assert!(output.status.success(), "{text}");
}

#[test]
fn block_expression_lambda_fixture_verifies() {
    let output = verify("tests/positive/block_expr_lambda.mm");
    let text = combined(&output);
    assert!(output.status.success(), "{text}");
}

#[test]
fn block_expression_reuse_fixture_verifies() {
    let output = verify("tests/positive/block_expr_reuse.mm");
    let text = combined(&output);
    assert!(output.status.success(), "{text}");
}

#[test]
fn unexpected_expression_token_fails_closed() {
    let output = verify("tests/negative/unexpected_token_expr.mm");
    let text = combined(&output);
    assert!(!output.status.success(), "{text}");
    assert!(text.contains("unexpected token"), "{text}");
}
