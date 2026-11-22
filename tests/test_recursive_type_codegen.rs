use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(name)
}

fn copy_fixture(name: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("create recursive-type test directory");
    let source = fixture(name);
    let copied = dir.path().join(name);
    std::fs::copy(source, &copied).expect("copy recursive-type fixture");
    (dir, copied)
}

fn run_build(name: &str, emit: &str) -> (tempfile::TempDir, Output) {
    let (dir, copied) = copy_fixture(name);
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("build")
        .arg(copied)
        .arg("-o")
        .arg(dir.path().join("out"))
        .arg("--emit")
        .arg(emit)
        .current_dir(dir.path())
        .output()
        .expect("run recursive-type build fixture");
    (dir, output)
}

fn assert_clean_recursive_type_error(output: &Output, fixture: &str, emit: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");
    assert!(
        output.status.code().is_some_and(|code| code != 0),
        "{fixture} ({emit}) must exit normally with a nonzero code, not be signaled:\n{combined}"
    );
    assert!(
        combined.contains("recursive types are not supported in LLVM codegen"),
        "{fixture} ({emit}) must report a clean recursive-type codegen error:\n{combined}"
    );
    assert!(
        !["stack overflow", "aborting", "panicked", "core dumped"]
            .iter()
            .any(|message| combined.contains(message)),
        "{fixture} ({emit}) must not overflow or abort:\n{combined}"
    );
}

#[test]
fn recursive_enum_and_struct_cycles_fail_llvm_codegen_cleanly() {
    for name in [
        "test_recursive_enum_direct.mm",
        "test_recursive_enum_self.mm",
        "test_recursive_enum_mutual_struct.mm",
        "test_recursive_enum_struct_param.mm",
    ] {
        let (_dir, output) = run_build(name, "llvm-ir");
        assert_clean_recursive_type_error(&output, name, "llvm-ir");
    }
}

#[test]
fn recursive_enum_errors_are_clean_for_binary_build_and_run() {
    let fixture = "test_recursive_enum_codegen_minimal.mm";
    let (_dir, binary) = run_build(fixture, "binary");
    assert_clean_recursive_type_error(&binary, fixture, "binary");

    let (dir, copied) = copy_fixture(fixture);
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("run")
        .arg(copied)
        .current_dir(dir.path())
        .output()
        .expect("run recursive-type fixture");
    assert_clean_recursive_type_error(&output, fixture, "run");
}

#[test]
fn non_recursive_shared_enum_fields_still_build() {
    let (_dir, output) = run_build("test_recursive_type_dag.mm", "llvm-ir");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "an acyclic type DAG with shared enum fields must build:\n{stdout}\n{stderr}"
    );
}

#[test]
fn non_recursive_generic_struct_instantiation_still_builds() {
    let (_dir, output) = run_build("test_recursive_generic_struct_acyclic.mm", "llvm-ir");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "a concrete generic field must not be treated as a recursive type:\n{stdout}\n{stderr}"
    );
}

#[test]
fn recursive_generic_struct_instantiation_fails_cleanly() {
    let fixture = "test_recursive_generic_struct_cycle.mm";
    let (_dir, output) = run_build(fixture, "llvm-ir");
    assert_clean_recursive_type_error(&output, fixture, "llvm-ir");
}

#[test]
fn recursive_array_fields_fail_llvm_codegen_cleanly() {
    for fixture in [
        "test_recursive_enum_array_payload.mm",
        "test_recursive_struct_array_field.mm",
    ] {
        let (_dir, output) = run_build(fixture, "llvm-ir");
        assert_clean_recursive_type_error(&output, fixture, "llvm-ir");
    }
}

#[test]
fn mutual_struct_enum_cycle_remains_verifiable() {
    let (_dir, copied) = copy_fixture("test_recursive_enum_mutual_struct.mm");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg(copied)
        .output()
        .expect("verify mutual recursive-type fixture");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "codegen guard must not change verifier behavior:\n{stdout}\n{stderr}"
    );
}
