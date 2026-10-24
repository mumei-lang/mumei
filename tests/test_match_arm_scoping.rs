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
fn match_arm_scoping_verifies() {
    let output = mumei_verify("tests/test_match_arm_scoping.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success() && combined.contains("Verification passed"),
        "pattern/`let` bindings shadowing outer names must stay arm-local, \
         while real assignments still fold:\n{combined}"
    );
}

#[test]
fn arm_write_to_shadowed_name_fails_closed() {
    // Writing to a pattern-bound name inside an arm must NOT update the
    // outer binding — the post-match value keeps the outer value, so the
    // `result == 7` postcondition is a real spec failure.
    let output = mumei_verify("tests/test_match_arm_scoping_negative.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "arm-local binding writes must not leak to post-match env:\n{combined}"
    );
}
