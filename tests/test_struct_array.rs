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

fn run_verify_with_overflow_check(name: &str, bitvec_i64: bool) -> Output {
    let (dir, copied) = copy_fixture(name);
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command
        .arg("verify")
        .arg("--json")
        .arg("--disable-spurious-detection")
        .arg("--check-i64-overflow");
    if bitvec_i64 {
        command.arg("--bitvec-i64");
    }
    command
        .arg(copied)
        .current_dir(dir.path())
        .output()
        .expect("run fixed-width struct-array verifier fixture")
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
        for atom in [
            "stale_read",
            "stale_block_tail_after_store",
            "unconstrained_field",
            "independent_fields",
            "shadowed_fresh_array_bindings",
            "u64_array_wrap",
            "u64_array_binding_wrap",
        ] {
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
        for atom in [
            "call_block_param_shadow",
            "call_two_block_args",
            "call_if_param_shadow",
            "call_two_if_args",
            "call_match_param_shadow",
            "call_two_match_args",
        ] {
            assert!(
                diagnostics.iter().any(|entry| {
                    entry["atom"] == atom
                        && entry["message"].as_str().is_some_and(|message| {
                            message.contains("precondition (requires) not satisfied")
                        })
                }),
                "missing expected call-site precondition failure for {atom} (bitvec={bitvec_i64}):\n{payload:#}"
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
fn later_argument_array_mutations_are_rejected_in_both_integer_modes() {
    for bitvec_i64 in [false, true] {
        let output = run_verify("test_array_argument_order_negative.mm", bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "later arguments that mutate an earlier array root must fail (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        let payload = verify_json(&output);
        let diagnostics = payload["diagnostics"]
            .as_array()
            .unwrap_or_else(|| panic!("verify JSON has no diagnostics array:\n{payload:#}"));
        for atom in [
            "direct_struct_call",
            "atom_ref_struct_call",
            "lambda_struct_call",
            "scalar_call",
        ] {
            assert!(
                diagnostics.iter().any(|entry| {
                    entry["atom"] == atom
                        && entry["message"].as_str().is_some_and(|message| {
                            message.contains("Type Error")
                                && message.contains("later call argument may mutate array root")
                        })
                }),
                "missing fail-closed array argument-order error for {atom} (bitvec={bitvec_i64}):\n{payload:#}"
            );
        }
    }
}

#[test]
fn later_arguments_may_mutate_distinct_array_roots() {
    for bitvec_i64 in [false, true] {
        let output = run_verify("test_array_argument_order_positive.mm", bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "mutation of a distinct later-argument root must remain supported (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        let payload = verify_json(&output);
        assert_eq!(
            payload["status"], "success",
            "distinct-root array arguments did not verify (bitvec={bitvec_i64}):\n{payload:#}"
        );
    }
}

#[test]
fn fixed_width_struct_array_overflow_is_rejected() {
    for bitvec_i64 in [false, true] {
        let output =
            run_verify_with_overflow_check("test_struct_array_fixed_width_negative.mm", bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "fixed-width struct-array overflow claims must fail (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        let payload = verify_json(&output);
        let diagnostics = payload["diagnostics"]
            .as_array()
            .unwrap_or_else(|| panic!("verify JSON has no diagnostics array:\n{payload:#}"));
        for atom in ["i32_array_wrap", "u32_array_wrap"] {
            assert!(
                diagnostics.iter().any(|entry| {
                    entry["atom"] == atom
                        && entry["message"]
                            .as_str()
                            .is_some_and(|message| message.contains("overflow"))
                }),
                "missing fixed-width overflow rejection for {atom} (bitvec={bitvec_i64}):\n{payload:#}"
            );
        }
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

#[test]
fn finite_enum_struct_array_leaves_verify_in_both_integer_modes() {
    for bitvec_i64 in [false, true] {
        let output = run_verify("test_struct_array_enum_positive.mm", bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "finite-enum struct-array leaves must verify (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        let payload = verify_json(&output);
        assert_eq!(
            payload["status"], "success",
            "finite-enum struct-array fixture did not verify (bitvec={bitvec_i64}):\n{payload:#}"
        );
    }
}

#[test]
fn enum_struct_array_false_claims_and_unsupported_shapes_fail() {
    for bitvec_i64 in [false, true] {
        let output = run_verify("test_struct_array_enum_negative.mm", bitvec_i64);
        assert!(
            !output.status.success(),
            "enum struct-array false claims must fail (bitvec={bitvec_i64}):\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let payload = verify_json(&output);
        let diagnostics = payload["diagnostics"]
            .as_array()
            .expect("diagnostics array");
        for atom in [
            "stale_enum_leaf_read",
            "enum_leaf_call_overflow",
            "claim_enum_payload_overflow",
        ] {
            assert!(
                diagnostics.iter().any(|entry| entry["atom"] == atom),
                "missing enum struct-array rejection for {atom} (bitvec={bitvec_i64}):\n{payload:#}"
            );
        }
        assert!(
            diagnostics.iter().any(|entry| {
                entry["atom"] == "enum_leaf_call_overflow"
                    && entry["message"]
                        .as_str()
                        .is_some_and(|message| {
                            message.contains("Call to 'take_enum_leaf_array'")
                                || message.contains(
                                    "Enum 'C' variant 'G' payload 'v' value may be outside the 'u32' domain",
                                )
                        })
            }),
            "out-of-range enum payload passed as a call argument must be rejected:\n{payload:#}"
        );

        let unsupported = run_verify("test_struct_array_enum_unsupported.mm", bitvec_i64);
        assert!(
            !unsupported.status.success(),
            "unsupported enum struct-array shapes must fail (bitvec={bitvec_i64}):\n{}\n{}",
            String::from_utf8_lossy(&unsupported.stdout),
            String::from_utf8_lossy(&unsupported.stderr)
        );
        let payload = verify_json(&unsupported);
        let diagnostics = payload["diagnostics"]
            .as_array()
            .expect("diagnostics array");
        for atom in [
            "recursive_enum_leaf",
            "array_payload_enum_leaf",
            "nested_array_leaf",
            "struct_array_enum_payload_leaf",
            "generic_enum_leaf",
        ] {
            assert!(
                diagnostics.iter().any(|entry| {
                    entry["atom"] == atom
                        && entry["message"]
                            .as_str()
                            .is_some_and(|message| message.contains("Type Error"))
                }),
                "unsupported struct-array shape {atom} must be a type error (bitvec={bitvec_i64}):\n{payload:#}"
            );
        }
    }
}

#[test]
fn array_store_rhs_is_evaluated_before_index() {
    for bitvec_i64 in [false, true] {
        let output = run_verify("test_array_store_evaluation_order.mm", bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "array-store evaluation order must verify (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        let payload = verify_json(&output);
        assert_eq!(
            payload["ensures_outcomes"][0]["outcome"], "proved",
            "array-store evaluation-order result was not proved (bitvec={bitvec_i64}):\n{payload:#}"
        );
    }

    let (dir, copied) = copy_fixture("test_array_store_evaluation_order.mm");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("run")
        .arg(copied)
        .current_dir(dir.path())
        .output()
        .expect("run array-store evaluation-order fixture");
    assert_eq!(
        output.status.code(),
        Some(63),
        "native array-store order should return the verified result:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn struct_array_field_store_reuses_contract_validation() {
    for bitvec_i64 in [false, true] {
        let output = run_verify("test_struct_array_field_contract_negative.mm", bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "field store must reject a broken refined field contract (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        assert!(
            !stdout.contains("panicked") && !stderr.contains("panicked"),
            "field contract violation must fail without panicking:\n{stdout}\n{stderr}"
        );
        assert!(
            stdout.contains("predicate violated") || stderr.contains("predicate violated"),
            "field contract failure should identify the violated predicate:\n{stdout}\n{stderr}"
        );
    }
}

#[test]
fn struct_array_field_store_is_havoced_for_postconditions() {
    for bitvec_i64 in [false, true] {
        let output = run_verify("test_struct_array_field_havoc_positive.mm", bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "postcondition after enum field store should verify (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        let payload = verify_json(&output);
        assert_eq!(
            payload["status"], "success",
            "enum field-store postcondition did not verify (bitvec={bitvec_i64}):\n{payload:#}"
        );
    }
}

#[test]
fn struct_array_field_store_rejects_mismatched_enum_type() {
    for bitvec_i64 in [false, true] {
        let output = run_verify("test_struct_array_enum_mismatch_negative.mm", bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "field store with a mismatched enum type must fail (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        assert!(
            !stdout.contains("panicked") && !stderr.contains("panicked"),
            "mismatched enum type must be a normal diagnostic:\n{stdout}\n{stderr}"
        );
        assert!(
            stdout.contains("enum sort") && stdout.contains("expected")
                || stderr.contains("enum sort") && stderr.contains("expected"),
            "mismatched enum type should report the expected enum sort:\n{stdout}\n{stderr}"
        );
    }
}

#[test]
fn native_enum_struct_array_element_store_read_and_match_returns_12() {
    let (dir, copied) = copy_fixture("test_struct_array_enum_codegen.mm");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("build")
        .arg(&copied)
        .arg("--emit")
        .arg("llvm-ir")
        .current_dir(dir.path())
        .output()
        .expect("build enum struct-array LLVM fixture");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "enum struct-array LLVM build failed:\n{stdout}\n{stderr}"
    );
    let ir_path = std::fs::read_dir(dir.path())
        .expect("list enum struct-array output")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|extension| extension == "ll"))
        .expect("find emitted enum struct-array LLVM");
    let ir = std::fs::read_to_string(ir_path).expect("read emitted enum struct-array LLVM");
    assert!(
        ir.contains("getelementptr")
            && ir.contains("load")
            && ir.contains("store")
            && ir.contains("extractvalue"),
        "enum struct-array field stores, reads, and matches must use GEP/load/store and enum projection:\n{ir}"
    );

    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("run")
        .arg(copied)
        .current_dir(dir.path())
        .output()
        .expect("run native enum struct-array fixture");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(12),
        "native enum struct-array program should return 12:\n{stdout}\n{stderr}"
    );
}

#[test]
fn struct_array_equality_checks_mutated_leaves_in_both_integer_modes() {
    for bitvec_i64 in [false, true] {
        let output = run_verify("test_struct_array_equality_negative.mm", bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "mutated struct-array equality must fail (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        let payload = verify_json(&output);
        let diagnostics = payload["diagnostics"]
            .as_array()
            .unwrap_or_else(|| panic!("verify JSON has no diagnostics array:\n{payload:#}"));
        for atom in ["loop_rewrite_keeps_equality", "field_write_keeps_equality"] {
            assert!(
                diagnostics.iter().any(|entry| {
                    entry["atom"] == atom
                        && entry["message"]
                            .as_str()
                            .is_some_and(|message| message.contains("Postcondition"))
                }),
                "missing expected equality failure for {atom} (bitvec={bitvec_i64}):\n{payload:#}"
            );
        }
    }
}

#[test]
fn unmodified_struct_array_equality_verifies_in_both_integer_modes() {
    for bitvec_i64 in [false, true] {
        let output = run_verify("test_struct_array_equality_positive.mm", bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "unmodified struct-array equality must verify (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
    }
}

#[test]
fn struct_array_postcondition_havoc_preserves_untouched_indices() {
    for bitvec_i64 in [false, true] {
        let output = run_verify("test_struct_array_store_precision.mm", bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "stores to index zero must preserve facts about index one (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
    }
}

#[test]
fn mixed_struct_array_field_store_and_call_reject_stale_leaf_claims() {
    for bitvec_i64 in [false, true] {
        let output = run_verify("test_struct_array_mixed_call_havoc_negative.mm", bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "a writing call after a field store must havoc the other leaves (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        let payload = verify_json(&output);
        let diagnostics = payload["diagnostics"]
            .as_array()
            .expect("diagnostics array");
        for atom in [
            "mixed_field_store_and_writer_stale_read",
            "mixed_alias_field_store_and_writer_stale_read",
        ] {
            assert!(
                diagnostics.iter().any(|entry| {
                    entry["atom"] == atom
                        && entry["message"]
                            .as_str()
                            .is_some_and(|message| message.contains("Postcondition"))
                }),
                "expected the stale postcondition to fail for {atom} (bitvec={bitvec_i64}):\n{payload:#}"
            );
        }
        assert!(
            diagnostics.iter().any(|entry| {
                    entry["atom"] == "call_havoc_prevents_spec_ensures_unsat"
                        && entry["failure_type"] == "postcondition_violated"
                }),
            "call-written array facts must be havoced during spec validation (bitvec={bitvec_i64}):\n{payload:#}"
        );
    }
}

#[test]
fn struct_array_call_havoc_and_field_only_loop_preserve_expected_facts() {
    for bitvec_i64 in [false, true] {
        let output = run_verify("test_struct_array_call_havoc_positive.mm", bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "field-only loops must preserve unchanged leaf facts (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
    }
}

#[test]
fn whole_element_store_and_writing_call_reject_stale_leaf_claims() {
    for bitvec_i64 in [false, true] {
        let output = run_verify("test_struct_array_whole_element_call_havoc.mm", bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "whole-element stores followed by writing calls must not preserve stale leaves (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        let payload = verify_json(&output);
        assert!(
            payload["diagnostics"].as_array().is_some_and(|diagnostics| {
                diagnostics.iter().any(|entry| {
                    entry["atom"] == "whole_element_store_and_writer_stale_read"
                        && entry["message"]
                            .as_str()
                            .is_some_and(|message| message.contains("Postcondition"))
                })
            }),
            "expected the whole-element stale postcondition to fail (bitvec={bitvec_i64}):\n{payload:#}"
        );
    }
}
