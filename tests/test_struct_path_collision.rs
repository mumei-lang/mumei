use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

const NEGATIVE_FIXTURES: [(&str, &str, &str, &str); 7] = [
    (
        "test_struct_path_collision_scalar.mm",
        "scalar_path_false_claim",
        "struct 'S'",
        "('inner.x' and 'inner_x' both map to 'inner_x')",
    ),
    (
        "test_struct_path_collision_array.mm",
        "array_path_false_claim",
        "struct 'ArrayS'",
        "('inner.values' and 'inner_values' both map to 'inner_values')",
    ),
    (
        "test_struct_path_collision_deep.mm",
        "deep_path_collision",
        "struct 'DeepA'",
        "('b.c' and 'b_c' both map to 'b_c')",
    ),
    (
        "test_struct_path_collision_nested.mm",
        "nested_path_collision",
        "struct 'Holder'",
        "('wrapped.child.x' and 'wrapped.child_x' both map to 'wrapped_child_x')",
    ),
    (
        "test_struct_path_collision_enum_payload.mm",
        "enum_payload_path_false_claim",
        "struct 'S'",
        "('inner.x' and 'inner_x' both map to 'inner_x')",
    ),
    (
        "test_struct_path_collision_recursive.mm",
        "recursive_path_false_claim",
        "struct 'R'",
        "('r.x' and 'r_x' both map to 'r_x')",
    ),
    (
        "test_struct_path_collision_mutual.mm",
        "mutual_recursive_path_collision",
        "struct 'A'",
        "('b.y' and 'b_y' both map to 'b_y')",
    ),
];

fn run_verify(fixture_name: &str, bitvec_i64: bool) -> Output {
    let dir = tempfile::tempdir().expect("create verification directory");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(fixture_name);
    let copied = dir.path().join(fixture_name);
    std::fs::copy(fixture, &copied).expect("copy struct-path fixture");

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
fn flattened_struct_path_collisions_are_rejected_in_both_integer_modes() {
    for bitvec_i64 in [false, true] {
        for (fixture, atom, expected_struct, expected_paths) in NEGATIVE_FIXTURES {
            let output = run_verify(fixture, bitvec_i64);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                !output.status.success(),
                "{fixture} must be rejected (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
            );
            assert!(
                !stderr.contains("panicked"),
                "{fixture} must fail without panicking (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
            );

            let payload = verify_json(&output);
            let diagnostics = payload["diagnostics"]
                .as_array()
                .unwrap_or_else(|| panic!("verify JSON has no diagnostics array:\n{payload:#}"));
            assert_eq!(
                diagnostics.len(),
                1,
                "{fixture} should emit one diagnostic (bitvec={bitvec_i64}):\n{payload:#}"
            );
            let diagnostic = &diagnostics[0];
            assert_eq!(
                diagnostic["atom"], atom,
                "{fixture} diagnostic is attributed to the wrong atom:\n{payload:#}"
            );
            let message = diagnostic["message"]
                .as_str()
                .unwrap_or_else(|| panic!("diagnostic has no message:\n{payload:#}"));
            assert!(
                message.contains(expected_struct)
                    && message.contains(expected_paths)
                    && message.contains("rename a field"),
                "{fixture} was rejected for the wrong reason (bitvec={bitvec_i64}):\n{payload:#}"
            );
        }
    }
}

#[test]
fn moved_deep_write_collisions_fail_closed_in_both_integer_modes() {
    const MOVED_ATOMS: [&str; 2] = [
        "colliding_deep_field_tail",
        "bad_colliding_deep_field_returns_sibling",
    ];
    for bitvec_i64 in [false, true] {
        let output = run_verify("test_struct_path_collision_deep_write.mm", bitvec_i64);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "colliding deep-write fixture must be rejected (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        assert!(
            !stderr.contains("panicked"),
            "colliding deep-write fixture must fail without panicking (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );

        let payload = verify_json(&output);
        let diagnostics = payload["diagnostics"]
            .as_array()
            .unwrap_or_else(|| panic!("verify JSON has no diagnostics array:\n{payload:#}"));
        assert_eq!(
            diagnostics.len(),
            MOVED_ATOMS.len(),
            "each moved atom should receive one collision diagnostic (bitvec={bitvec_i64}):\n{payload:#}"
        );
        for atom in MOVED_ATOMS {
            let atom_diagnostics: Vec<_> = diagnostics
                .iter()
                .filter(|entry| entry["atom"].as_str() == Some(atom))
                .collect();
            assert_eq!(
                atom_diagnostics.len(),
                1,
                "missing moved-atom diagnostic for {atom} (bitvec={bitvec_i64}):\n{payload:#}"
            );
            let message = atom_diagnostics[0]["message"]
                .as_str()
                .unwrap_or_else(|| panic!("diagnostic has no message:\n{payload:#}"));
            assert!(
                message.contains("struct 'O'")
                    && message.contains("('a.b_c' and 'a_b.c' both map to 'a_b_c')")
                    && message.contains("rename a field"),
                "moved atom {atom} was rejected for the wrong reason (bitvec={bitvec_i64}):\n{payload:#}"
            );
        }
    }
}

#[test]
fn noncolliding_and_recursive_struct_paths_remain_accepted_in_both_modes() {
    for bitvec_i64 in [false, true] {
        let started = Instant::now();
        let output = run_verify("test_struct_path_collision_positive.mm", bitvec_i64);
        let elapsed = started.elapsed();
        eprintln!("recursive positive verification (bitvec={bitvec_i64}): {elapsed:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "positive struct-path fixture must verify (bitvec={bitvec_i64}):\n{stdout}\n{stderr}"
        );
        assert!(
            elapsed < Duration::from_secs(10),
            "recursive positive took too long (bitvec={bitvec_i64}): {elapsed:?}"
        );
        let payload = verify_json(&output);
        assert_eq!(
            payload["status"], "success",
            "positive fixture did not verify (bitvec={bitvec_i64}):\n{payload:#}"
        );
        assert!(
            payload["diagnostics"]
                .as_array()
                .is_some_and(|diagnostics| diagnostics.is_empty()),
            "positive fixture emitted diagnostics (bitvec={bitvec_i64}):\n{payload:#}"
        );
    }
}
