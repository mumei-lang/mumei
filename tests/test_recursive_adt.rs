use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(name)
}

fn run_verify(name: &str, bitvec_i64: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command.arg("verify");
    if bitvec_i64 {
        command.arg("--bitvec-i64");
    }
    command
        .arg(fixture(name))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run recursive-ADT verification fixture")
}

fn output_text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_verified(output: &Output, name: &str, bitvec_i64: bool) {
    assert!(
        output.status.success(),
        "{name} should verify (bitvec_i64={bitvec_i64}):\n{}",
        output_text(output)
    );
}

fn assert_rejected(output: &Output, name: &str, expected: &str, bitvec_i64: bool) {
    let combined = output_text(output);
    assert!(
        output.status.code().is_some_and(|code| code != 0),
        "{name} should fail normally, not be signaled (bitvec_i64={bitvec_i64}):\n{combined}"
    );
    assert!(
        combined.contains(expected),
        "{name} should report {expected:?} (bitvec_i64={bitvec_i64}):\n{combined}"
    );
    assert!(
        !combined.contains("panicked") && !combined.contains("stack overflow"),
        "{name} must fail closed without crashing:\n{combined}"
    );
}

#[test]
fn bounded_recursive_datatypes_verify_in_both_integer_modes() {
    for bitvec_i64 in [false, true] {
        assert_verified(
            &run_verify("test_recursive_adt_positive.mm", bitvec_i64),
            "test_recursive_adt_positive.mm",
            bitvec_i64,
        );
        assert_verified(
            &run_verify("test_recursive_adt_no_base.mm", bitvec_i64),
            "test_recursive_adt_no_base.mm",
            bitvec_i64,
        );

        assert_rejected(
            &run_verify("test_recursive_adt_unconstrained_negative.mm", bitvec_i64),
            "test_recursive_adt_unconstrained_negative.mm",
            "Postcondition",
            bitvec_i64,
        );
        for name in [
            "test_recursive_adt_generic_negative.mm",
            "test_recursive_adt_mutual_negative.mm",
            "test_recursive_adt_struct_negative.mm",
        ] {
            assert_rejected(
                &run_verify(name, bitvec_i64),
                name,
                "not constructible in specs",
                bitvec_i64,
            );
        }
        assert_rejected(
            &run_verify("test_recursive_adt_array_negative.mm", bitvec_i64),
            "test_recursive_adt_array_negative.mm",
            "array element enum",
            bitvec_i64,
        );
    }
}

#[test]
fn recursive_enum_payload_field_updates_are_sound() {
    for bitvec_i64 in [false, true] {
        assert_verified(
            &run_verify("test_recursive_adt_payload_read_positive.mm", bitvec_i64),
            "test_recursive_adt_payload_read_positive.mm",
            bitvec_i64,
        );
        assert_rejected(
            &run_verify("test_recursive_adt_payload_update_negative.mm", bitvec_i64),
            "test_recursive_adt_payload_update_negative.mm",
            "Verification Error",
            bitvec_i64,
        );
        for name in [
            "test_recursive_adt_self_field_write_negative.mm",
            "test_recursive_adt_nested_self_write_negative.mm",
        ] {
            assert_rejected(
                &run_verify(name, bitvec_i64),
                name,
                "recursive self payload",
                bitvec_i64,
            );
        }
    }
}

#[test]
fn recursive_datatypes_support_finite_enum_and_struct_payloads() {
    for bitvec_i64 in [false, true] {
        assert_verified(
            &run_verify(
                "test_recursive_adt_composite_payload_positive.mm",
                bitvec_i64,
            ),
            "test_recursive_adt_composite_payload_positive.mm",
            bitvec_i64,
        );
        assert_rejected(
            &run_verify(
                "test_recursive_adt_composite_payload_false_claim.mm",
                bitvec_i64,
            ),
            "test_recursive_adt_composite_payload_false_claim.mm",
            "Postcondition",
            bitvec_i64,
        );
        assert_rejected(
            &run_verify(
                "test_recursive_adt_struct_payload_write_stale.mm",
                bitvec_i64,
            ),
            "test_recursive_adt_struct_payload_write_stale.mm",
            "Postcondition",
            bitvec_i64,
        );
        for name in [
            "test_recursive_adt_nested_array_payload_negative.mm",
            "test_recursive_adt_recursive_enum_payload_negative.mm",
        ] {
            assert_rejected(
                &run_verify(name, bitvec_i64),
                name,
                "not a finite ADT",
                bitvec_i64,
            );
        }
    }
}

#[test]
fn enum_constructor_payload_domains_are_checked_in_both_integer_modes() {
    for bitvec_i64 in [false, true] {
        assert_verified(
            &run_verify(
                "test_recursive_adt_enum_payload_domain_positive.mm",
                bitvec_i64,
            ),
            "test_recursive_adt_enum_payload_domain_positive.mm",
            bitvec_i64,
        );
        for name in [
            "test_recursive_adt_enum_payload_domain_recursive_struct_negative.mm",
            "test_recursive_adt_enum_payload_domain_finite_struct_negative.mm",
            "test_recursive_adt_enum_payload_domain_recursive_scalar_negative.mm",
            "test_recursive_adt_enum_payload_domain_finite_scalar_negative.mm",
            "test_recursive_adt_enum_payload_array_domain_negative.mm",
        ] {
            assert_rejected(
                &run_verify(name, bitvec_i64),
                name,
                "value may be outside the 'u32' domain",
                bitvec_i64,
            );
        }
    }
}

#[test]
fn fixed_width_call_arguments_are_checked_in_both_integer_modes() {
    for bitvec_i64 in [false, true] {
        for name in [
            "test_call_domain_vacuous_negative.mm",
            "test_call_domain_narrow_eq_negative.mm",
            "test_call_domain_callref_negative.mm",
            "test_call_domain_array_negative.mm",
        ] {
            assert_rejected(
                &run_verify(name, bitvec_i64),
                name,
                if name == "test_call_domain_array_negative.mm" {
                    "Call to 'g' argument 'xs' may be outside the 'u32' domain"
                } else {
                    "Call to 'g' argument 'x' may be outside the 'u32' domain"
                },
                bitvec_i64,
            );
        }
        for name in [
            "test_call_domain_u32_passthrough_positive.mm",
            "test_call_domain_bounded_i64_positive.mm",
            "test_call_domain_u32_array_passthrough_positive.mm",
            "test_call_domain_u32_struct_passthrough_positive.mm",
            "test_call_domain_u32_struct_array_passthrough_positive.mm",
        ] {
            assert_verified(&run_verify(name, bitvec_i64), name, bitvec_i64);
        }
    }
}

#[test]
fn recursive_match_bindings_do_not_leak_enum_types_across_arms() {
    for bitvec_i64 in [false, true] {
        assert_verified(
            &run_verify("test_recursive_adt_arm_enum_shadow.mm", bitvec_i64),
            "test_recursive_adt_arm_enum_shadow.mm",
            bitvec_i64,
        );
    }
}

#[test]
fn recursive_datatype_codegen_keeps_the_clean_llvm_error() {
    let dir = tempfile::tempdir().expect("create recursive-ADT build directory");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("build")
        .arg(fixture("test_recursive_adt_positive.mm"))
        .arg("-o")
        .arg(dir.path().join("out"))
        .arg("--emit")
        .arg("llvm-ir")
        .current_dir(dir.path())
        .output()
        .expect("run recursive-ADT build");
    let combined = output_text(&output);
    assert!(
        output.status.code().is_some_and(|code| code != 0),
        "recursive LLVM codegen should exit normally with an error:\n{combined}"
    );
    assert!(
        combined.contains("recursive types are not supported in LLVM codegen"),
        "recursive LLVM codegen should use the #1318 diagnostic:\n{combined}"
    );
    assert!(
        !combined.contains("stack overflow") && !combined.contains("aborting"),
        "recursive LLVM codegen must not overflow:\n{combined}"
    );
}
