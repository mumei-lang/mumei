use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(name)
}

fn copy_fixture(name: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("create struct-array test directory");
    let source = fixture(name);
    let copied = dir.path().join(name);
    std::fs::copy(source, &copied).expect("copy struct-array fixture");
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
        .expect("run struct-array verifier fixture")
}

fn verify_json(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json_start = stdout
        .find('{')
        .unwrap_or_else(|| panic!("verify --json emitted no JSON:\n{stdout}\n{stderr}"));
    serde_json::from_str(&stdout[json_start..])
        .unwrap_or_else(|err| panic!("{err}: invalid verify JSON:\n{stdout}\n{stderr}"))
}

#[test]
fn scalar_leaf_struct_arrays_verify_in_both_integer_modes() {
    for bitvec_i64 in [false, true] {
        let output = run_verify("test_struct_array_positive.mm", bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "struct-array positive cases must verify (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        let payload = verify_json(&output);
        assert_eq!(
            payload["status"], "success",
            "struct-array fixture did not verify (bitvec={bitvec_i64}):\n{payload:#}"
        );
        assert!(
            payload["diagnostics"]
                .as_array()
                .is_some_and(|diagnostics| diagnostics.is_empty()),
            "struct-array fixture emitted diagnostics (bitvec={bitvec_i64}):\n{payload:#}"
        );
    }
}

#[test]
fn unsupported_or_unjustified_struct_array_uses_fail_closed_errors() {
    for bitvec_i64 in [false, true] {
        let output = run_verify("test_struct_array_negative.mm", bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "struct-array negative cases must fail (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        assert!(
            !stdout.contains("panicked") && !stderr.contains("panicked"),
            "struct-array negatives must fail without panicking:\n{stdout}\n{stderr}"
        );
        let payload = verify_json(&output);
        let diagnostics = payload["diagnostics"]
            .as_array()
            .unwrap_or_else(|| panic!("verify JSON has no diagnostics array:\n{payload:#}"));
        for atom in ["stale_read", "unconstrained_field", "independent_fields"] {
            assert!(
                diagnostics.iter().any(|entry| {
                    entry["atom"] == atom
                        && entry["message"]
                            .as_str()
                            .is_some_and(|message| message.contains("Postcondition"))
                }),
                "missing expected verification failure for {atom} (bitvec={bitvec_i64}):\n{payload:#}"
            );
        }
        for atom in [
            "bare_element_equality",
            "mismatched_struct_store",
            "enum_leaf",
            "array_leaf",
            "unresolved_generic_leaf",
            "nested_struct_array",
        ] {
            assert!(
                diagnostics.iter().any(|entry| {
                    entry["atom"] == atom
                        && entry["message"]
                            .as_str()
                            .is_some_and(|message| message.contains("Type Error"))
                }),
                "missing expected type error for {atom} (bitvec={bitvec_i64}):\n{payload:#}"
            );
        }
        assert!(
            diagnostics.iter().any(|entry| {
                entry["atom"] == "shared_field_array"
                    && entry["message"]
                        .as_str()
                        .is_some_and(|message| message.contains("shares its buffer"))
            }),
            "shared struct-field array mutation must be rejected:\n{payload:#}"
        );
    }
}

#[test]
fn struct_array_reads_use_the_struct_llvm_type_and_native_values() {
    let (dir, copied) = copy_fixture("test_struct_array_codegen.mm");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("build")
        .arg(&copied)
        .arg("--emit")
        .arg("llvm-ir")
        .current_dir(dir.path())
        .output()
        .expect("build struct-array LLVM fixture");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "struct-array LLVM build failed:\n{stdout}\n{stderr}"
    );
    let ir_path = std::fs::read_dir(dir.path())
        .expect("list struct-array output")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|extension| extension == "ll"))
        .expect("find emitted struct-array LLVM");
    let ir = std::fs::read_to_string(ir_path).expect("read emitted struct-array LLVM");
    assert!(
        ir.contains("getelementptr { i64, i64 }")
            && ir.contains("load { i64, i64 }")
            && ir.contains("extractvalue { i64, i64 }"),
        "struct array reads must use the aggregate element type:\n{ir}"
    );

    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("run")
        .arg(copied)
        .current_dir(dir.path())
        .output()
        .expect("run native struct-array fixture");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(16),
        "native struct-array program should return 16:\n{stdout}\n{stderr}"
    );
}

#[test]
fn unsupported_struct_array_layout_fails_codegen_cleanly() {
    let (dir, copied) = copy_fixture("test_struct_array_codegen_unsupported.mm");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("build")
        .arg(copied)
        .arg("--emit")
        .arg("llvm-ir")
        .current_dir(dir.path())
        .output()
        .expect("build unsupported struct-array LLVM fixture");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "unsupported struct-array layout should fail codegen:\n{stdout}\n{stderr}"
    );
    assert!(
        stderr.contains("array element struct 'Bad' must have only scalar leaves")
            && !stderr.contains("panicked")
            && !stdout.contains("panicked"),
        "unsupported struct-array layout must produce a clean codegen error:\n{stdout}\n{stderr}"
    );
}
