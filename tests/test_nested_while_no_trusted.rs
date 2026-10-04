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
fn nested_while_on_copy_locals_verifies_without_trusted() {
    let output = mumei_verify("tests/test_nested_while_no_trusted.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success() && combined.contains("3 item(s) verified"),
        "nested while loops over i64 locals must verify without `trusted`:\n{combined}"
    );
    assert!(!combined.contains("UseAfterMove"), "{combined}");
}
