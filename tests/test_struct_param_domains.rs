use std::path::Path;
use std::process::{Command, Output};

fn verify_fixture(fixture: &str, bitvec_i64: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command.arg("verify");
    if bitvec_i64 {
        command.arg("--bitvec-i64");
    }
    command
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join(fixture),
        )
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run struct-parameter domain fixture")
}

#[test]
fn fixed_width_struct_parameter_leaves_are_assumed_in_both_integer_modes() {
    for bitvec_i64 in [false, true] {
        let output = verify_fixture("test_struct_param_enum_payload_positive.mm", bitvec_i64);
        assert!(
            output.status.success(),
            "struct payload should verify (bitvec_i64={bitvec_i64}):\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
