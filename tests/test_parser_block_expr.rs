use std::fs;
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

fn total_constraints(file: &str) -> usize {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(file);
    let unique = format!(
        "block_budget_{}_{}.mm",
        std::process::id(),
        file.as_bytes()
            .iter()
            .map(|byte| *byte as usize)
            .sum::<usize>()
    );
    let uncached = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(unique);
    let source_text = fs::read_to_string(&source)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", source.display()));
    let suffix = format!("_probe_{}", std::process::id());
    let source_text = source_text
        .replace(
            "atom budget_inline(",
            &format!("atom budget_inline{suffix}("),
        )
        .replace("atom budget(", &format!("atom budget{suffix}("));
    fs::write(
        &uncached,
        format!("{source_text}\n// budget probe {}\n", std::process::id()),
    )
    .unwrap_or_else(|err| panic!("failed to write {}: {err}", uncached.display()));
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args([
            "verify",
            "--json",
            uncached.to_str().expect("UTF-8 fixture path"),
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify --json {file}: {err}"));
    let _ = fs::remove_file(&uncached);
    let text = combined(&output);
    assert!(output.status.success(), "{text}");
    let marker = "total_constraints: ";
    let start = text
        .find(marker)
        .map(|index| index + marker.len())
        .expect("verify --json omitted total_constraints metrics");
    text[start..]
        .split_once(',')
        .and_then(|(count, _)| count.trim().parse().ok())
        .expect("invalid total_constraints metric")
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
fn block_expression_array_len_tail_fixture_verifies() {
    let output = verify("tests/positive/block_expr_array_len_tail.mm");
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

#[test]
fn block_expression_does_not_double_constraint_translation() {
    let block_constraints = total_constraints("tests/positive/block_expr_budget.mm");
    let inline_constraints = total_constraints("tests/positive/block_expr_budget_inline.mm");
    assert!(
        block_constraints.abs_diff(inline_constraints) <= 2,
        "block constraints {block_constraints} differ from inline {inline_constraints}"
    );
}
