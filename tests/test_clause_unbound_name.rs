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
fn unbound_clause_names_fail_closed_not_vacuous() {
    let output = mumei_verify("tests/negative/clause_unbound_name.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "clauses referencing undeclared names must fail closed:\n{combined}"
    );
    for clause in ["requires", "ensures", "forall condition"] {
        assert!(
            combined.contains(&format!("unresolved name(s) in {clause}")),
            "expected an unresolved-name error in {clause}:\n{combined}"
        );
    }
}
