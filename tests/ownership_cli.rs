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
fn ownership_protocol_verifies() {
    let output = mumei_verify("tests/test_ownership.mm");
    assert!(
        output.status.success(),
        "ownership protocol should verify\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn ownership_hostile_takeover_fails_with_invalid_prestate() {
    let output = mumei_verify("tests/test_ownership_error.mm");
    assert!(
        !output.status.success(),
        "hostile takeover should fail verification"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stdout}\n{stderr}");
    assert!(
        combined.contains("InvalidPreState") || combined.contains("Temporal effect violation"),
        "expected InvalidPreState/temporal effect error, got:\n{combined}"
    );
}
