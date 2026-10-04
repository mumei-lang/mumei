use std::process::Command;

// Verify a copy in a fresh directory so `.mumei_cache` from an earlier run
// cannot turn the proofs into "skipped (unchanged)".
fn mumei_verify_fresh(file: &str) -> std::process::Output {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(file);
    let dir = std::env::temp_dir().join(format!(
        "mumei_nested_while_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let name = source.file_name().unwrap();
    std::fs::copy(&source, dir.join(name)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg(name)
        .current_dir(&dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify {file}: {err}"));
    let _ = std::fs::remove_dir_all(&dir);
    output
}

#[test]
fn nested_while_on_copy_locals_verifies_without_trusted() {
    let output = mumei_verify_fresh("tests/test_nested_while_no_trusted.mm");
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
