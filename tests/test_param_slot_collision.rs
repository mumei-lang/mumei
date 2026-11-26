use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

fn run_verify(fixture_name: &str, bitvec_i64: bool) -> Output {
    let dir = tempfile::tempdir().expect("create verification directory");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(fixture_name);
    let copied = dir.path().join(fixture_name);
    std::fs::copy(fixture, &copied).expect("copy parameter-slot fixture");
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command
        .arg("verify")
        .arg("--json")
        .arg("--disable-spurious-detection");
    if bitvec_i64 {
        command.arg("--bitvec-i64");
    }
    command
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
fn parameter_projection_slot_collisions_are_rejected() {
    let expected = [
        (
            "overlapping_struct_slots",
            "parameter 'outer_inner' collides with a field projection slot of struct parameter 'outer'; rename the parameter",
        ),
        (
            "scalar_parameter_slot_false_claim",
            "parameter 'p_x' collides with a field projection slot of struct parameter 'p'; rename the parameter",
        ),
        (
            "nested_struct_slot_collision",
            "parameter 'p_a' collides with a field projection slot of struct parameter 'p'; rename the parameter",
        ),
        (
            "real_struct_slot_collision",
            "parameter 'item_x' collides with a field projection slot of struct parameter 'item'; rename the parameter",
        ),
        (
            "recursive_parameter_projection_overlap",
            "parameter 'p_r_a' collides with a field projection slot of struct parameter 'p'; rename the parameter",
        ),
    ];
    for bitvec_i64 in [false, true] {
        let output = run_verify("test_param_slot_collision_negative.mm", bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "parameter-slot collisions must be rejected (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        assert!(
            !stderr.contains("panicked"),
            "parameter-slot collision must fail without panicking (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );

        let payload = verify_json(&output);
        let diagnostics = payload["diagnostics"]
            .as_array()
            .unwrap_or_else(|| panic!("verify JSON has no diagnostics array:\n{payload:#}"));
        assert_eq!(
            diagnostics.len(),
            expected.len() + 1,
            "each parameter collision must have one diagnostic:\n{payload:#}"
        );
        for (atom, message) in expected {
            let errors: Vec<_> = diagnostics
                .iter()
                .filter(|entry| {
                    entry["atom"].as_str() == Some(atom) && entry["severity"] == "error"
                })
                .collect();
            assert_eq!(
                errors.len(),
                1,
                "missing diagnostic for {atom}:\n{payload:#}"
            );
            assert!(
                errors[0]["message"]
                    .as_str()
                    .is_some_and(|actual| actual.contains(message)),
                "wrong collision diagnostic for {atom}:\n{payload:#}"
            );
        }
        let let_error = diagnostics
            .iter()
            .find(|entry| entry["atom"].as_str() == Some("let_name_collides_parameter_slot"))
            .and_then(|entry| entry["message"].as_str())
            .unwrap_or_else(|| panic!("missing parameter-vs-let diagnostic:\n{payload:#}"));
        assert!(
            let_error.contains("local 'p_x' collides with a field projection slot")
                && let_error.contains("rename the local"),
            "parameter-vs-let collision failed for the wrong reason:\n{payload:#}"
        );
    }
}

#[test]
fn noncolliding_parameter_names_are_accepted() {
    for bitvec_i64 in [false, true] {
        let output = run_verify("test_param_slot_collision_positive.mm", bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "noncolliding parameter names must verify (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        let payload = verify_json(&output);
        assert_eq!(
            payload["status"], "success",
            "positive parameter fixture did not verify:\n{payload:#}"
        );
        assert!(
            payload["diagnostics"]
                .as_array()
                .is_some_and(|diagnostics| diagnostics.is_empty()),
            "positive parameter fixture emitted diagnostics:\n{payload:#}"
        );
    }
}
