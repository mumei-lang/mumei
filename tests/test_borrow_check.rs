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
fn borrow_after_branch_move_is_rejected() {
    let output = verify("tests/negative/borrow_after_move_branch.mm");
    assert!(!output.status.success());
}
