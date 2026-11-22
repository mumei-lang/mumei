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
        .expect("run lambda-domain fixture");
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.success(), text)
}

#[test]
fn typed_lambda_domains_are_scoped_and_checked_at_invocation() {
    for bitvec_i64 in [false, true] {
        for fixture in [
            "tests/positive/test_lambda_domain_f5.mm",
            "tests/positive/test_lambda_array_domain.mm",
            "tests/positive/test_lambda_struct_domain.mm",
            "tests/positive/test_lambda_struct_shadow.mm",
            "tests/positive/test_lambda_hof_domain.mm",
            "tests/positive/test_lambda_hof_guarded_array_store.mm",
        ] {
            let (verified, output) = verify_fixture(fixture, bitvec_i64);
            assert!(
                verified,
                "{fixture} should verify (bitvec_i64={bitvec_i64}):\n{output}"
            );
        }

        for (fixture, diagnostic) in [
            (
                "tests/negative/test_lambda_domain_f6.mm",
                "Call to 'h' argument 'x' may be outside the 'u32' domain",
            ),
            (
                "tests/negative/test_lambda_array_oob.mm",
                "Potential Out-of-Bounds store on 'ys'",
            ),
            (
                "tests/negative/test_lambda_hof_array_store_oob.mm",
                "Potential Out-of-Bounds store on 'ys'",
            ),
            (
                "tests/negative/test_lambda_hof_array_read_oob.mm",
                "Potential Out-of-Bounds on 'ys'",
            ),
            (
                "tests/negative/test_lambda_callref_domain.mm",
                "Call to 'call' argument 'x' may be outside the 'u32' domain",
            ),
            (
                "tests/negative/test_lambda_shadow_domain.mm",
                "precondition (requires) not satisfied at call site",
            ),
            (
                "tests/negative/test_lambda_hof_domain.mm",
                "Call to 'f' argument 'arg0' may be outside the 'u32' domain",
            ),
            (
                "tests/negative/test_lambda_hof_spec_domain.mm",
                "Call to 'f' argument 'arg0' may be outside the 'u32' domain",
            ),
        ] {
            let (verified, output) = verify_fixture(fixture, bitvec_i64);
            assert!(
                !verified && output.contains(diagnostic),
                "{fixture} should reject with {diagnostic:?} (bitvec_i64={bitvec_i64}):\n{output}"
            );
        }
    }
}
