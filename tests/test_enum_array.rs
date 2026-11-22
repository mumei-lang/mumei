use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(name)
}

fn copy_fixture(name: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("create enum-array test directory");
    let source = fixture(name);
    let copied = dir.path().join(name);
    std::fs::copy(source, &copied).expect("copy enum-array fixture");
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
        .expect("run enum-array verifier fixture")
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
fn finite_enum_arrays_verify_in_both_integer_modes() {
    for bitvec_i64 in [false, true] {
        let output = run_verify("test_enum_array_positive.mm", bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "enum-array positive cases must verify (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        let payload = verify_json(&output);
        assert_eq!(
            payload["status"], "success",
            "enum-array fixture did not verify (bitvec={bitvec_i64}):\n{payload:#}"
        );
        assert!(
            payload["diagnostics"]
                .as_array()
                .is_some_and(|diagnostics| diagnostics.is_empty()),
            "enum-array fixture emitted diagnostics (bitvec={bitvec_i64}):\n{payload:#}"
        );
    }
}

#[test]
fn false_enum_claims_fail_and_unsupported_element_enums_are_type_errors() {
    const VERIFICATION_FAILURES: [&str; 2] = ["false_tag_claim", "stale_same_index_read"];
    const ENUM_TYPE_ERRORS: [&str; 2] = ["mixed_enum_literal", "mismatched_enum_store"];
    const TYPE_ERRORS: [&str; 4] = [
        "array_payload_enum",
        "struct_array_payload_enum",
        "recursive_enum_array",
        "unresolved_generic_enum_array",
    ];
    for bitvec_i64 in [false, true] {
        let output = run_verify("test_enum_array_negative.mm", bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "enum-array negative cases must fail (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        assert!(
            !stdout.contains("panicked") && !stderr.contains("panicked"),
            "enum-array negatives must fail without panicking:\n{stdout}\n{stderr}"
        );
        let payload = verify_json(&output);
        let diagnostics = payload["diagnostics"]
            .as_array()
            .unwrap_or_else(|| panic!("verify JSON has no diagnostics array:\n{payload:#}"));
        for atom in VERIFICATION_FAILURES {
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
        for atom in ENUM_TYPE_ERRORS {
            assert!(
                diagnostics.iter().any(|entry| {
                    entry["atom"] == atom
                        && entry["message"]
                            .as_str()
                            .is_some_and(|message| message.contains("Type Error"))
                }),
                "missing enum-sort type error for {atom} (bitvec={bitvec_i64}):\n{payload:#}"
            );
        }
        for atom in TYPE_ERRORS {
            assert!(
                diagnostics.iter().any(|entry| {
                    entry["atom"] == atom
                        && entry["message"]
                            .as_str()
                            .is_some_and(|message| message.contains("array element enum"))
                }),
                "missing enum-array type error for {atom} (bitvec={bitvec_i64}):\n{payload:#}"
            );
        }
    }
}

#[test]
fn enum_array_payload_ranges_are_enforced_without_overconstraining_i64() {
    let positive = run_verify("test_enum_array_ranges_positive.mm", false);
    let positive_stdout = String::from_utf8_lossy(&positive.stdout);
    let positive_stderr = String::from_utf8_lossy(&positive.stderr);
    assert!(
        positive.status.success(),
        "u32 enum-array payload should satisfy nonnegativity in Int mode:\n{positive_stdout}\n{positive_stderr}"
    );
    assert_eq!(
        verify_json(&positive)["status"],
        "success",
        "u32 enum-array payload range was not applied"
    );

    for bitvec_i64 in [false, true] {
        let negative = run_verify("test_enum_array_ranges_negative.mm", bitvec_i64);
        let negative_stdout = String::from_utf8_lossy(&negative.stdout);
        let negative_stderr = String::from_utf8_lossy(&negative.stderr);
        assert!(
            !negative.status.success(),
            "i64 enum-array payload must remain unconstrained (bitvec={bitvec_i64}):\n{negative_stdout}\n{negative_stderr}"
        );
        let payload = verify_json(&negative);
        assert!(
            payload["diagnostics"]
                .as_array()
                .is_some_and(|diagnostics| diagnostics.iter().any(|entry| {
                    entry["atom"] == "unbounded_i64_payload"
                        && entry["message"]
                            .as_str()
                            .is_some_and(|message| message.contains("Postcondition"))
                })),
            "expected unconstrained i64 payload failure (bitvec={bitvec_i64}):\n{payload:#}"
        );
    }
}

#[test]
fn enum_array_reads_load_the_aggregate_llvm_element_type() {
    let (dir, copied) = copy_fixture("test_enum_array_codegen.mm");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("build")
        .arg(copied)
        .arg("--emit")
        .arg("llvm-ir")
        .current_dir(dir.path())
        .output()
        .expect("build enum-array LLVM fixture");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "enum-array LLVM build failed:\n{stdout}\n{stderr}"
    );
    let ir = std::fs::read_to_string(dir.path().join("katana_array_match.ll"))
        .expect("read emitted enum-array LLVM");
    assert!(
        ir.contains("getelementptr { i64, i64 }")
            && ir.contains("load { i64, i64 }")
            && ir.contains("extractvalue { i64, i64 }"),
        "enum array reads and matches must use the tagged aggregate type:\n{ir}"
    );
}

#[test]
fn unsupported_enum_array_layout_fails_codegen_cleanly() {
    let (dir, copied) = copy_fixture("test_enum_array_codegen_unsupported.mm");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("build")
        .arg(copied)
        .arg("--emit")
        .arg("llvm-ir")
        .current_dir(dir.path())
        .output()
        .expect("build unsupported enum-array LLVM fixture");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "unsupported enum-array layout should fail codegen:\n{stdout}\n{stderr}"
    );
    assert!(
        stderr.contains("mixed Str/f64 payload slot")
            && !stderr.contains("panicked")
            && !stdout.contains("panicked"),
        "unsupported enum-array layout must produce a clean codegen error:\n{stdout}\n{stderr}"
    );
}

#[test]
fn native_enum_array_literal_store_and_payload_calls_return_12() {
    let (dir, copied) = copy_fixture("test_enum_array_codegen.mm");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("run")
        .arg(copied)
        .current_dir(dir.path())
        .output()
        .expect("run native enum-array fixture");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(12),
        "native enum-array program should return 12:\n{stdout}\n{stderr}"
    );
}

#[test]
fn native_enum_array_equality_uses_array_element_type_inference() {
    let (dir, copied) = copy_fixture("test_enum_array_equality.mm");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("run")
        .arg(copied)
        .current_dir(dir.path())
        .output()
        .expect("run enum-array equality fixture");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(12),
        "native enum-array equality program should return 12:\n{stdout}\n{stderr}"
    );
}
