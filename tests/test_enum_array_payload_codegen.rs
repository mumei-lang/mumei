use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn copy_fixture(name: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("create enum-array codegen directory");
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(name);
    let copied = dir.path().join(name);
    std::fs::copy(source, &copied).expect("copy enum-array codegen fixture");
    (dir, copied)
}

fn run_verify(name: &str, bitvec_i64: bool) -> Output {
    let (dir, copied) = copy_fixture(name);
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command
        .arg("verify")
        .arg("--json")
        .arg("--disable-spurious-detection");
    if bitvec_i64 {
        command.arg("--bitvec-i64");
    }
    command
        .arg(copied)
        .current_dir(dir.path())
        .output()
        .expect("run enum-array codegen verifier fixture")
}

fn verify_payload(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json_start = stdout
        .find('{')
        .unwrap_or_else(|| panic!("verify --json emitted no JSON:\n{stdout}\n{stderr}"));
    serde_json::from_str(&stdout[json_start..])
        .unwrap_or_else(|err| panic!("{err}: invalid verify JSON:\n{stdout}\n{stderr}"))
}

fn assert_verifies(name: &str, bitvec_i64: bool) {
    let output = run_verify(name, bitvec_i64);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "{name} should verify (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
    );
    let payload = verify_payload(&output);
    assert_eq!(
        payload["status"], "success",
        "{name} did not verify (bitvec={bitvec_i64}):\n{payload:#}"
    );
    assert!(
        payload["diagnostics"]
            .as_array()
            .is_some_and(|diagnostics| diagnostics.is_empty()),
        "{name} emitted diagnostics (bitvec={bitvec_i64}):\n{payload:#}"
    );
}

fn assert_native_result(fixture: &str, expected: i32) {
    let (dir, copied) = copy_fixture(fixture);
    let binary = dir.path().join("enum_array_payload");
    let build = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("build")
        .arg(copied)
        .arg("--emit")
        .arg("binary")
        .arg("-o")
        .arg(&binary)
        .current_dir(dir.path())
        .output()
        .expect("build enum-array payload fixture");
    let build_stdout = String::from_utf8_lossy(&build.stdout);
    let build_stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        build.status.success(),
        "{fixture} build failed:\n{build_stdout}\n{build_stderr}"
    );

    let run = Command::new(binary)
        .output()
        .expect("run enum-array payload fixture");
    let run_stdout = String::from_utf8_lossy(&run.stdout);
    let run_stderr = String::from_utf8_lossy(&run.stderr);
    assert_eq!(
        run.status.code(),
        Some(expected),
        "{fixture} native result should equal its verified postcondition:\n{run_stdout}\n{run_stderr}"
    );
}

#[test]
fn scalar_array_enum_payloads_build_and_run_with_verified_results() {
    const FIXTURE: &str = "test_enum_array_payload_codegen_positive.mm";
    for bitvec_i64 in [false, true] {
        assert_verifies(FIXTURE, bitvec_i64);
    }

    let (dir, copied) = copy_fixture(FIXTURE);
    let binary = dir.path().join("enum_array_payload");
    let build = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("build")
        .arg(copied)
        .arg("--emit")
        .arg("binary")
        .arg("-o")
        .arg(&binary)
        .current_dir(dir.path())
        .output()
        .expect("build scalar enum-array payload fixture");
    let build_stdout = String::from_utf8_lossy(&build.stdout);
    let build_stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        build.status.success(),
        "scalar enum-array payload build failed:\n{build_stdout}\n{build_stderr}"
    );

    let run = Command::new(binary)
        .output()
        .expect("run scalar enum-array payload fixture");
    let run_stdout = String::from_utf8_lossy(&run.stdout);
    let run_stderr = String::from_utf8_lossy(&run.stderr);
    assert_eq!(
        run.status.code(),
        Some(24),
        "native result should equal the verified postcondition result:\n{run_stdout}\n{run_stderr}"
    );
}

#[test]
fn block_tail_array_payload_builds_and_runs_with_verified_result() {
    let fixture = "test_enum_array_payload_codegen_block_tail.mm";
    for bitvec_i64 in [false, true] {
        assert_verifies(fixture, bitvec_i64);
    }
    assert_native_result(fixture, 9);
}

#[test]
fn branch_shadowed_array_payload_builds_and_runs_with_verified_result() {
    let fixture = "test_enum_array_payload_codegen_bshadow.mm";
    for bitvec_i64 in [false, true] {
        assert_verifies(fixture, bitvec_i64);
    }
    assert_native_result(fixture, 8);
}

#[test]
fn direct_payload_array_read_builds_and_runs_with_verified_result() {
    let fixture = "test_enum_array_payload_codegen_direct_read.mm";
    for bitvec_i64 in [false, true] {
        assert_verifies(fixture, bitvec_i64);
    }
    assert_native_result(fixture, 10);
}

#[test]
fn enum_tag_read_with_array_payload_named_tag_returns_discriminant() {
    let fixture = "test_enum_array_payload_codegen_tag_field.mm";
    for bitvec_i64 in [false, true] {
        assert_verifies(fixture, bitvec_i64);
    }
    assert_native_result(fixture, 1);
}

#[test]
fn unsupported_scalar_array_payload_shapes_fail_codegen_without_panicking() {
    for (fixture, diagnostic) in [
        (
            "test_enum_array_payload_codegen_escape_call.mm",
            "stack-backed array",
        ),
        (
            "test_enum_array_payload_codegen_escape_literal.mm",
            "stack-backed array",
        ),
        (
            "test_enum_array_payload_codegen_escape_let.mm",
            "stack-backed array",
        ),
        (
            "test_enum_array_payload_codegen_task_escape_direct.mm",
            "stack-backed array payload cannot escape through an array-bearing enum return",
        ),
        (
            "test_enum_array_payload_codegen_task_escape_let.mm",
            "stack-backed array payload cannot escape through an array-bearing enum return",
        ),
        (
            "test_enum_array_payload_codegen_block_tail_escape.mm",
            "stack-backed array payload cannot escape through an array-bearing enum return",
        ),
        (
            "test_enum_array_payload_codegen_conditional_shadow.mm",
            "cannot use if/match control flow",
        ),
        (
            "test_enum_array_payload_codegen_field_write_literal.mm",
            "stack-backed array payload cannot be written into enum field",
        ),
        (
            "test_enum_array_payload_codegen_field_write_alias.mm",
            "stack-backed array payload cannot be written into enum field",
        ),
        (
            "test_enum_array_payload_codegen_slot_conflict.mm",
            "array payload conflicts",
        ),
        (
            "test_enum_array_payload_codegen_equality.mm",
            "equality on enums with array payloads",
        ),
        (
            "test_enum_array_payload_codegen_struct_array.mm",
            "supported scalar element type",
        ),
        (
            "test_enum_array_payload_codegen_array_in_struct.mm",
            "unsupported payload cast",
        ),
    ] {
        for bitvec_i64 in [false, true] {
            assert_verifies(fixture, bitvec_i64);
        }

        let (dir, copied) = copy_fixture(fixture);
        let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
            .arg("build")
            .arg(copied)
            .arg("--emit")
            .arg("llvm-ir")
            .current_dir(dir.path())
            .output()
            .expect("build unsupported enum-array payload fixture");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "{fixture} should fail codegen:\n{stdout}\n{stderr}"
        );
        assert!(
            (stdout.contains("Codegen Error") || stderr.contains("Codegen Error"))
                && (stdout.contains(diagnostic) || stderr.contains(diagnostic))
                && !stdout.contains("panicked")
                && !stderr.contains("panicked"),
            "{fixture} should report a clean codegen error containing {diagnostic:?}:\n{stdout}\n{stderr}"
        );
    }
}
