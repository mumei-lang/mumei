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
fn compliance_protocol_verifies() {
    let output = mumei_verify("tests/test_compliance.mm");
    assert!(
        output.status.success(),
        "compliance protocol should verify\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compliance_missing_pep_variant_fails_exhaustiveness() {
    let output = mumei_verify("tests/test_compliance_negative.mm");
    assert!(
        !output.status.success(),
        "non-exhaustive compliance classifier should fail verification"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stdout}\n{stderr}");
    assert!(
        combined.contains("Match is not exhaustive")
            && (combined.contains("CustomerType::PEP") || combined.contains("tag=3")),
        "expected PEP/tag=3 exhaustiveness counterexample, got:\n{combined}"
    );
}
