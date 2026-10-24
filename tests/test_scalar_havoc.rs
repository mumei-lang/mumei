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
fn scalar_havoc_keeps_sort_and_invariants() {
    let output = mumei_verify("tests/test_scalar_havoc.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "a `Str` havoced by a loop must still support len() and invariants:\n{combined}"
    );
}

#[test]
fn scalar_havoc_drops_stale_values() {
    let output = mumei_verify("tests/negative/scalar_havoc_stale.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "pre-loop `Str`/enum/f64 values must not survive loop havoc — \
         `s == \"x\"` after `s = \"y\"` must be unprovable:\n{combined}"
    );
}
