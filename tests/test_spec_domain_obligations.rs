use std::process::Command;

fn verify_fixture(path: &str, bitvec_i64: bool) -> (bool, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command.arg("verify");
    if bitvec_i64 {
        command.arg("--bitvec-i64");
    }
    let output = command
        .arg(path)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run specification-domain fixture");
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.success(), text)
}

#[test]
fn spec_call_domains_are_conjoined_and_out_of_range_calls_fail() {
    for bitvec_i64 in [false, true] {
        let (verified, output) =
            verify_fixture("tests/positive/test_spec_callback_domain.mm", bitvec_i64);
        assert!(
            verified,
            "in-range spec callback should verify (bitvec_i64={bitvec_i64}):\n{output}"
        );

        let (verified, output) = verify_fixture(
            "tests/negative/test_spec_callback_domain_oob.mm",
            bitvec_i64,
        );
        assert!(
            !verified && output.contains("use_callback"),
            "out-of-range spec callback should be rejected (bitvec_i64={bitvec_i64}):\n{output}"
        );

        let (verified, output) =
            verify_fixture("tests/negative/test_spec_atom_call_domain.mm", bitvec_i64);
        assert!(
            !verified
                && output.contains("Call to 'g'")
                && output.contains("u32"),
            "out-of-range atom call in ensures should be rejected (bitvec_i64={bitvec_i64}):\n{output}"
        );

        let (verified, output) =
            verify_fixture("tests/negative/test_spec_guard_domain.mm", bitvec_i64);
        assert!(
            !verified && output.contains("Postcondition"),
            "guarded spec calls must not constrain paths that skip the call (bitvec_i64={bitvec_i64}):\n{output}"
        );
    }
}
