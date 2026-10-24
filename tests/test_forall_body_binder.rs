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
fn forall_exists_body_binder_is_scoped() {
    let output = mumei_verify("tests/test_forall_body_binder.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "forall/exists binders in a body let must not be unbound:\n{combined}"
    );
}

#[test]
fn forall_body_other_names_still_fail_closed() {
    let output = mumei_verify("tests/negative/forall_body_binder_unbound.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success() && combined.contains("unresolved variable"),
        "non-binder names inside the quantifier body must stay fail-closed:\n{combined}"
    );
}
