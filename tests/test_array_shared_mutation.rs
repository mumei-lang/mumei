use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

const SHARED_ARRAY_DIAGNOSTIC: &str =
    "shares its buffer with a struct field or enum payload; mutating it is not supported — rebuild the field with a field write instead";

fn run_verify(fixture_name: &str, bitvec_i64: bool) -> Output {
    let dir = tempfile::tempdir().expect("create verification directory");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(fixture_name);
    let copied = dir.path().join(fixture_name);
    std::fs::copy(fixture, &copied).expect("copy shared-array fixture");
    assert!(!dir.path().join(".mumei_cache").exists());

    let mut invocation = Command::new(env!("CARGO_BIN_EXE_mumei"));
    invocation
        .arg("verify")
        .arg("--json")
        .arg("--disable-spurious-detection");
    if bitvec_i64 {
        invocation.arg("--bitvec-i64");
    }
    invocation
        .arg(&copied)
        .current_dir(dir.path())
        .output()
        .expect("run mumei verify")
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
fn shared_composite_array_mutations_are_rejected_in_both_integer_modes() {
    const EXPECTED_FAILURES: [(&str, &str); 18] = [
        ("struct_field_local_alias", "v"),
        ("struct_parameter_field_alias", "v"),
        ("enum_pattern_then_field_read", "ys"),
        ("enum_pattern_across_rematch", "ys"),
        ("enum_constructor_source_alias", "a"),
        ("struct_constructor_source_alias", "a"),
        ("spec_marker_does_not_hide_body_constructor", "xs"),
        ("shared_alias_chain", "w"),
        ("shared_array_mutating_lambda_argument", "v"),
        ("shared_mutating_call", "v"),
        ("shared_field_write_source", "ys"),
        ("branch_shared_marker_survives_merge", "v"),
        ("loop_shared_marker_survives_loop", "v"),
        ("shared_store_inside_while", "v"),
        ("mutating_field_access_call", "s.xs"),
        ("local_builtin_shadow_is_rejected", "s.xs"),
        ("lambda_alias_write_is_rejected", "xs"),
        ("atom_alias_write_is_rejected", "s.xs"),
    ];

    for bitvec_i64 in [false, true] {
        let output = run_verify("test_array_shared_mutation_negative.mm", bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "shared-array mutation cases must fail (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        let payload = verify_json(&output);
        let diagnostics = payload["diagnostics"]
            .as_array()
            .unwrap_or_else(|| panic!("verify JSON has no diagnostics array:\n{payload:#}"));

        for (atom, array_name) in EXPECTED_FAILURES {
            let diagnostic = diagnostics
                .iter()
                .find(|entry| entry["atom"].as_str() == Some(atom))
                .unwrap_or_else(|| {
                    panic!("missing shared-array failure for {atom} (bitvec={bitvec_i64}):\n{payload:#}")
                });
            let expected_message = format!("array '{array_name}' {SHARED_ARRAY_DIAGNOSTIC}");
            assert!(
                diagnostic["message"]
                    .as_str()
                    .is_some_and(|message| message.contains(&expected_message)),
                "unexpected diagnostic for {atom} (bitvec={bitvec_i64}):\n{diagnostic:#}"
            );
        }
    }
}

#[test]
fn ordinary_array_mutations_and_nonwriting_calls_remain_supported() {
    for bitvec_i64 in [false, true] {
        let output = run_verify("test_array_shared_mutation_positive.mm", bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "ordinary array operations must still verify (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        let payload = verify_json(&output);
        assert_eq!(
            payload["status"], "success",
            "positive shared-array fixture did not verify (bitvec={bitvec_i64}):\n{payload:#}"
        );
        assert!(
            payload["diagnostics"]
                .as_array()
                .is_some_and(|diagnostics| diagnostics.is_empty()),
            "positive shared-array fixture emitted diagnostics (bitvec={bitvec_i64}):\n{payload:#}"
        );
    }
}
