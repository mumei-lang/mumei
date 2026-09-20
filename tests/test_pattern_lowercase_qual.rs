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
fn lowercase_qualified_pattern_verifies() {
    // `mine::Cons(v)` previously bound `mine` as a variable and orphaned
    // `::Cons(v)` into the arm tail; now the qualifier folds into the
    // variant path and `v` binds the payload.
    let output = mumei_verify("tests/test_pattern_lowercase_qual.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success() && combined.contains("Verification passed"),
        "lowercase-qualified patterns should verify:\n{combined}"
    );
}
