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
fn call_result_len_inherits_callee_ensures() {
    let output = mumei_verify("tests/test_call_result_len.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "`let t = f(..)` must inherit `ensures: len(result) == k` from the callee — \
         len(t), t[len(t)-1], len(f(..)), if-branch merges, and post-store len must all resolve:\n{combined}"
    );
}

#[test]
fn call_result_len_wrong_claim_is_rejected() {
    let output = mumei_verify("tests/negative/call_result_len.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "`len(t) == 4` for a callee guaranteeing `len(result) == 3` must fail verification:\n{combined}"
    );
}
