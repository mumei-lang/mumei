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
fn builtin_calls_are_not_reported_as_uninterpreted() {
    let output = mumei_verify("tests/negative/builtin_not_spurious.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "unconstrained element read must falsify `result >= 0`:\n{combined}"
    );
    assert!(
        !combined.contains("Spurious counterexample"),
        "a genuine counterexample must not be labeled spurious:\n{combined}"
    );
    assert!(
        !combined.contains("len (uninterpreted_function)"),
        "builtin `len` must not be reported as uninterpreted:\n{combined}"
    );
}
