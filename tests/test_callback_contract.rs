use std::process::Command;

fn verify_fixture(path: &str) -> (bool, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg(path)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run callback contract fixture");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.success(), text)
}

#[test]
fn callback_contract_violation_is_hard_error() {
    let (ok, output) = verify_fixture("tests/negative/test_callback_contract_violation.mm");
    assert!(!ok, "{output}");
    assert!(
        output.contains("Contract subsumption failed: atom_ref(neg_one) passed to apply_nonneg.f")
    );
}

#[test]
fn callback_contract_subsumption_accepts_valid_callback() {
    let (ok, output) = verify_fixture("tests/positive/test_callback_contract_ok.mm");
    assert!(ok, "{output}");
}
