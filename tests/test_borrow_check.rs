use std::process::Command;

fn verify(file: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg(file)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify {file}: {err}"))
}

fn combined(output: &std::process::Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn borrow_negative_fixtures_report_mir_rules() {
    let cases = [
        (
            "tests/negative/borrow_mut_shared_alias.mm",
            "cannot borrow 'x' mutably while another borrow is live",
        ),
        (
            "tests/negative/borrow_move_while_borrowed.mm",
            "cannot move 'x' while it is borrowed",
        ),
        (
            "tests/negative/borrow_after_move.mm",
            "cannot borrow 'x' after it was moved",
        ),
        (
            "tests/negative/borrow_write_through_shared.mm",
            "cannot write through shared parameter 'x'",
        ),
        (
            "tests/negative/borrow_move_out_of_ref_param.mm",
            "cannot move out of shared parameter 'x'",
        ),
        (
            "tests/negative/borrow_branch_loan_alias.mm",
            "cannot borrow 'x' mutably while another borrow is live",
        ),
        (
            "tests/negative/borrow_local_move_while_borrowed.mm",
            "cannot move 'x' while it is borrowed",
        ),
        (
            "tests/negative/borrow_field_alias.mm",
            "mutably while another borrow is live",
        ),
        (
            "tests/negative/borrow_callref_consume_use_after_move.mm",
            "x",
        ),
        (
            "tests/negative/borrow_callref_alias.mm",
            "cannot borrow 'x' mutably while another borrow is live",
        ),
        (
            "tests/negative/borrow_atom_ref_value_escape.mm",
            "cannot take 'atom_ref(reader)' as a value",
        ),
    ];
    for (file, message) in cases {
        let output = verify(file);
        let text = combined(&output);
        assert!(
            !output.status.success(),
            "{file} unexpectedly passed:\n{text}"
        );
        assert!(
            text.contains(message),
            "{file} missing {message:?}:\n{text}"
        );
    }
}

#[test]
fn borrow_positive_fixture_verifies() {
    let output = verify("tests/positive/borrow_ok.mm");
    let text = combined(&output);
    assert!(output.status.success(), "borrow_ok should verify:\n{text}");
}

#[test]
fn borrow_ref_param_reads_verify() {
    let output = verify("tests/positive/borrow_ref_param_reads.mm");
    let text = combined(&output);
    assert!(
        output.status.success(),
        "borrow_ref_param_reads should verify:\n{text}"
    );
}

#[test]
fn borrow_after_branch_move_is_rejected() {
    let output = verify("tests/negative/borrow_after_move_branch.mm");
    let text = combined(&output);
    assert!(!output.status.success(), "{text}");
    assert!(text.contains("x"), "{text}");
    assert!(
        text.contains("cannot borrow 'x' after it was moved")
            || text.contains("used after being moved"),
        "{text}"
    );
}

#[test]
fn borrow_reinit_after_move_verifies() {
    let output = verify("tests/positive/borrow_reinit_after_move.mm");
    let text = combined(&output);
    assert!(output.status.success(), "{text}");
}

#[test]
fn borrow_callref_positive_fixture_verifies() {
    let output = verify("tests/positive/borrow_callref_ok.mm");
    let text = combined(&output);
    assert!(
        output.status.success(),
        "borrow_callref_ok should verify:\n{text}"
    );
}
