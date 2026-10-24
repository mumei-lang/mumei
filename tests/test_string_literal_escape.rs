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
fn string_literals_with_escapes_round_trip() {
    let output = mumei_verify("tests/test_string_literal_escape.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "escaped string literals must survive the atom-body re-lex:\n{combined}"
    );
}

#[test]
fn backslash_n_does_not_collapse_to_newline() {
    let output = mumei_verify("tests/negative/string_literal_escape_collapse.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "`\"a\\\\nb\"` must not equal `\"a\\nb\"` — escaping must be preserved:\n{combined}"
    );
}
