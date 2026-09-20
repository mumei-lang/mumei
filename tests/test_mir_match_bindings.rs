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
fn match_pattern_bindings_verify() {
    let output = mumei_verify("tests/test_mir_match_bindings.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success() && combined.contains("Verification passed"),
        "pattern-bound locals and merge-unused moves should verify:\n{combined}"
    );
}

#[test]
fn partial_move_observed_after_merge_still_fails() {
    let output = mumei_verify("tests/test_mir_match_bindings_negative.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success() && combined.contains("conflicting ownership"),
        "a partial move observed after the merge must stay a ConflictingMerge error:\n{combined}"
    );
}
