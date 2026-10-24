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
fn loop_call_havoc_positive_claims_verify() {
    let output = mumei_verify("tests/test_loop_call_array_havoc.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "post-loop claims provable under a havoced array state must still verify:\n{combined}"
    );
}

#[test]
fn loop_call_havoc_rejects_stale_reads() {
    let output = mumei_verify("tests/test_loop_call_array_havoc_negative.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "post-loop reads must not see pre-loop (stale) array values:\n{combined}"
    );
}
