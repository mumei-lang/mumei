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
fn str_array_elements_verify() {
    let output = mumei_verify("tests/test_array_str.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "`[Str]` arrays must verify: params, literals, reads, stores, `==`/`!=`, `len`, forall, call args, `-> [Str]` returns:\n{combined}"
    );
}

#[test]
fn str_array_failures_are_rejected() {
    let output = mumei_verify("tests/test_array_str_negative.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "wrong `[Str]` claims, non-string stores, and mixed literals must fail:\n{combined}"
    );
}

#[test]
fn nested_arrays_fail_closed() {
    let output = mumei_verify("tests/test_array_nested.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "`[[T]]` signatures and nested literals must be rejected, never silently mis-encoded:\n{combined}"
    );
}

#[test]
fn nested_indexing_is_a_syntax_error() {
    let output = mumei_verify("tests/test_array_nested_index.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "`a[i][j]` must be a syntax error, not a silently-dropped index:\n{combined}"
    );
}
