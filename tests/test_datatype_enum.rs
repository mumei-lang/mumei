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
fn datatype_enum_match_verifies() {
    let output = mumei_verify("tests/test_datatype_enum.mm");
    assert!(
        output.status.success(),
        "finite-ADT enum verification should succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("Verification passed") && !combined.contains('❌'),
        "every atom should verify on the datatype path:\n{combined}"
    );
}

#[test]
fn datatype_enum_missing_arm_fails_with_ctor_name() {
    let output = mumei_verify("tests/test_datatype_enum_negative.mm");
    assert!(
        !output.status.success(),
        "non-exhaustive enum match should fail verification"
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("Match is not exhaustive") && combined.contains("Blue"),
        "expected uncovered-constructor counterexample naming 'Blue', got:\n{combined}"
    );
}
