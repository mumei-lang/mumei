use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(name)
}

fn run_verify(name: &str, bitvec_i64: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command.arg("verify").arg("--json");
    if bitvec_i64 {
        command.arg("--bitvec-i64");
    }
    command
        .arg(fixture(name))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run mutual-recursive-ADT verification fixture")
}

fn run_build(name: &str) -> Output {
    let dir = tempfile::tempdir().expect("create mutual-recursive-ADT build directory");
    Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("build")
        .arg(fixture(name))
        .arg("-o")
        .arg(dir.path().join("out"))
        .arg("--emit")
        .arg("llvm-ir")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run mutual-recursive-ADT build fixture")
}

fn output_text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn diagnostics(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let start = stdout
        .find('{')
        .unwrap_or_else(|| panic!("verify --json emitted no JSON:\n{}", output_text(output)));
    serde_json::from_str(&stdout[start..])
        .unwrap_or_else(|error| panic!("{error}: invalid verify JSON:\n{stdout}"))
}

fn assert_normal_rejection(output: &Output, name: &str, expected: &str, bitvec_i64: bool) {
    let text = output_text(output);
    assert!(
        output.status.code().is_some_and(|code| code != 0),
        "{name} should fail normally (bitvec_i64={bitvec_i64}):\n{text}"
    );
    assert!(
        text.contains(expected),
        "{name} should report {expected:?} (bitvec_i64={bitvec_i64}):\n{text}"
    );
    assert!(
        !text.contains("panicked") && !text.contains("stack overflow"),
        "{name} must fail closed without crashing:\n{text}"
    );
}

#[test]
fn mutual_recursive_enums_verify_in_both_integer_modes() {
    for bitvec_i64 in [false, true] {
        let output = run_verify("test_mutual_recursive_adt_positive.mm", bitvec_i64);
        assert!(
            output.status.success(),
            "mutual recursive enums should verify (bitvec_i64={bitvec_i64}):\n{}",
            output_text(&output)
        );

        let output = run_verify("test_mutual_recursive_adt_false_claims.mm", bitvec_i64);
        assert!(
            output.status.code().is_some_and(|code| code != 0),
            "mutual recursive false claims must not verify (bitvec_i64={bitvec_i64}):\n{}",
            output_text(&output)
        );
        let text = output_text(&output);
        assert!(
            !text.contains("panicked") && !text.contains("stack overflow"),
            "mutual recursive false claims must fail closed:\n{text}"
        );
        let payload = diagnostics(&output);
        for atom in ["stale_value_claim", "wrong_variant_claim"] {
            assert!(
                payload["diagnostics"].as_array().is_some_and(|entries| {
                    entries
                        .iter()
                        .any(|entry| entry["atom"] == atom && entry["code"] == "failed")
                }),
                "{atom} must be rejected (bitvec_i64={bitvec_i64}):\n{payload:#}"
            );
        }

        let output = run_verify(
            "test_mutual_recursive_adt_write_domain_positive.mm",
            bitvec_i64,
        );
        assert!(
            output.status.success(),
            "in-range recursive enum field writes should verify (bitvec_i64={bitvec_i64}):\n{}",
            output_text(&output)
        );
        let output = run_verify(
            "test_mutual_recursive_adt_guarded_write_domain_positive.mm",
            bitvec_i64,
        );
        assert!(
            output.status.success(),
            "variant-guarded in-range recursive enum writes should verify (bitvec_i64={bitvec_i64}):\n{}",
            output_text(&output)
        );
        let output = run_verify(
            "test_mutual_recursive_adt_recursive_field_domain_positive.mm",
            bitvec_i64,
        );
        assert!(
            output.status.success(),
            "recursive enum field reads and bindings should preserve domains (bitvec_i64={bitvec_i64}):\n{}",
            output_text(&output)
        );
        for name in [
            "test_recursive_enum_write_domain_negative.mm",
            "test_mutual_recursive_adt_write_domain_negative.mm",
        ] {
            let output = run_verify(name, bitvec_i64);
            assert_normal_rejection(
                &output,
                name,
                "value may be outside the 'u32' domain",
                bitvec_i64,
            );
        }

        let output = run_verify("test_mutual_recursive_adt_out_of_range.mm", bitvec_i64);
        assert_normal_rejection(
            &output,
            "out-of-range u32 constructor",
            "value may be outside the 'u32' domain",
            bitvec_i64,
        );

        let output = run_verify("test_mutual_recursive_adt_closed_shapes.mm", bitvec_i64);
        assert_normal_rejection(
            &output,
            "unsupported recursive groups",
            "not constructible in specs",
            bitvec_i64,
        );
        let payload = diagnostics(&output);
        for atom in [
            "group_larger_than_four_is_closed",
            "uninhabited_group_is_closed",
            "recursion_through_struct_is_closed",
            "recursion_through_array_is_closed",
            "generic_recursive_group_is_closed",
            "struct_array_payload_group_is_closed",
            "non_group_recursive_enum_payload_stays_closed",
        ] {
            assert!(
                payload["diagnostics"]
                    .as_array()
                    .is_some_and(|entries| entries.iter().any(|entry| entry["atom"] == atom)),
                "missing clean rejection for {atom} (bitvec_i64={bitvec_i64}):\n{payload:#}"
            );
        }
    }
}

#[test]
fn mutual_recursive_enum_codegen_fails_cleanly() {
    let output = run_build("test_mutual_recursive_adt_positive.mm");
    let text = output_text(&output);
    assert!(
        output.status.code().is_some_and(|code| code != 0),
        "mutual recursive enum codegen should fail normally:\n{text}"
    );
    assert!(
        text.contains("recursive types are not supported in LLVM codegen"),
        "mutual recursive enum codegen should use the recursive-type error:\n{text}"
    );
    assert!(
        !["panicked", "stack overflow", "aborting", "core dumped"]
            .iter()
            .any(|message| text.contains(message)),
        "mutual recursive enum codegen must not crash:\n{text}"
    );
}
