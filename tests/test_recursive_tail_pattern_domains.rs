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
        .expect("run recursive-tail pattern fixture")
}

#[test]
fn recursive_tail_pattern_bindings_preserve_payload_domains() {
    for bitvec_i64 in [false, true] {
        let output = verify_fixture("test_recursive_adt_tail_rewrap_positive.mm", bitvec_i64);
        assert!(
            output.status.success(),
            "recursive tail rewrap should verify (bitvec_i64={bitvec_i64}):\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
