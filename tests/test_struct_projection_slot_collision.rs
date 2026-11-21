use std::path::Path;
use std::process::Command;

#[test]
fn user_locals_cannot_collide_with_struct_projection_slots() {
    let dir = tempfile::tempdir().expect("create verification directory");
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("test_struct_projection_slot_collision.mm");
    let copied = dir.path().join("test_struct_projection_slot_collision.mm");
    std::fs::copy(source, &copied).expect("copy projection-slot fixture");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--json")
        .arg("--disable-spurious-detection")
        .arg(&copied)
        .current_dir(dir.path())
        .output()
        .expect("run mumei verify");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "projection-slot collisions must be rejected:\n{stdout}\n{stderr}"
    );
    let json_start = stdout
        .find('{')
        .unwrap_or_else(|| panic!("verify --json emitted no JSON:\n{stdout}\n{stderr}"));
    let payload: serde_json::Value = serde_json::from_str(&stdout[json_start..])
        .unwrap_or_else(|err| panic!("{err}: invalid verify JSON:\n{stdout}\n{stderr}"));
    let diagnostics = payload["diagnostics"]
        .as_array()
        .unwrap_or_else(|| panic!("verify JSON has no diagnostics array:\n{payload:#}"));
    let expected = [
        "local_before_struct_slot_collision",
        "local_after_struct_slot_collision",
        "self_referential_projection_slot",
    ];
    assert_eq!(
        diagnostics.len(),
        expected.len(),
        "each collision must have exactly one diagnostic:\n{payload:#}"
    );
    assert!(
        !diagnostics.iter().any(|entry| {
            entry["atom"].as_str() == Some("array_element_field_name_is_not_a_projection_slot")
        }),
        "array element fields are not projection slots:\n{payload:#}"
    );
    for atom in expected {
        let errors: Vec<_> = diagnostics
            .iter()
            .filter(|entry| entry["atom"].as_str() == Some(atom) && entry["severity"] == "error")
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "collision diagnostic missing for '{atom}':\n{payload:#}"
        );
        assert!(
            errors[0]["message"]
                .as_str()
                .is_some_and(|message| message.contains("rename the local")),
            "collision '{atom}' failed for the wrong reason:\n{payload:#}"
        );
    }
}
