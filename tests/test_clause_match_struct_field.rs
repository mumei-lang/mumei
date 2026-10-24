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
fn clause_match_on_struct_field_resolves_enum() {
    let output = mumei_verify("tests/test_clause_match_struct_field.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "`match h.r` in requires/ensures must resolve the field's declared enum type (incl. nested chains, let-bound fields, bare variants, and i64 fields):\n{combined}"
    );
}

#[test]
fn clause_match_on_struct_field_fails_closed() {
    let output = mumei_verify("tests/test_clause_match_struct_field_negative.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "non-exhaustive or unsatisfied clause matches on a struct field must fail verification:\n{combined}"
    );
}
