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
fn array_literals_bind_len_and_elements() {
    let output = mumei_verify("tests/test_array_literal.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "`let a = [e0, …]` must bind a concrete-length array across reads, stores, aliasing, tails, and call args:\n{combined}"
    );
}

#[test]
fn array_literal_oob_is_rejected() {
    let output = mumei_verify("tests/test_array_literal_negative.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "`a[3]` on a 3-element literal must fail verification:\n{combined}"
    );
}
