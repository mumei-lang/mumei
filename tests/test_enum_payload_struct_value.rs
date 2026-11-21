use std::path::Path;
use std::process::{Command, Output};

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(name)
}

fn run_verify(name: &str, extra: &[&str]) -> Output {
    let dir = tempfile::tempdir().expect("create verification directory");
    let source = fixture(name);
    let copied = dir.path().join(source.file_name().expect("fixture name"));
    std::fs::copy(source, &copied).expect("copy fixture");
    Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .args(extra)
        .arg("--report-dir")
        .arg(dir.path())
        .arg(&copied)
        .current_dir(dir.path())
        .output()
        .expect("run mumei verify")
}

fn emit_atom_ir(name: &str, atom: &str) -> String {
    let dir = tempfile::tempdir().expect("create IR directory");
    let source = fixture(name);
    let copied = dir.path().join(source.file_name().expect("fixture name"));
    std::fs::copy(source, &copied).expect("copy fixture");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("build")
        .arg(&copied)
        .arg("--emit")
        .arg("llvm-ir")
        .current_dir(dir.path())
        .output()
        .expect("run mumei build");
    assert!(
        output.status.success(),
        "build failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::read_to_string(dir.path().join(format!("katana_{atom}.ll")))
        .unwrap_or_else(|err| panic!("no emitted IR for atom '{atom}': {err}"))
}

fn extract_indices(ir: &str) -> Vec<usize> {
    ir.lines()
        .filter(|line| line.contains("extractvalue"))
        .filter_map(|line| line.rsplit_once(", ")?.1.trim().parse().ok())
        .collect()
}

#[test]
fn struct_valued_enum_payload_reads_verify_in_default_and_bitvec_modes() {
    for args in [&[][..], &["--bitvec-i64"][..]] {
        let output = run_verify("test_enum_payload_struct_value.mm", args);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success() && stdout.contains("Verification passed"),
            "struct-valued enum payload reads must verify with {args:?}:\n{stdout}\n{stderr}"
        );
    }
}

#[test]
fn u64_leaf_through_struct_valued_payload_uses_unsigned_comparison() {
    let output = run_verify("test_enum_payload_struct_value_u64.mm", &["--bitvec-i64"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("Verification passed"),
        "u64 comparison through a struct-valued payload must be unsigned:\n{stdout}\n{stderr}"
    );
}

#[test]
fn aliased_struct_payloads_preserve_unsigned_leaf_semantics() {
    let output = run_verify(
        "test_enum_payload_struct_value_alias_u64.mm",
        &["--bitvec-i64", "--json", "--disable-spurious-detection"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "false u64 comparison claims must be rejected:\n{stdout}\n{stderr}"
    );
    let json_start = stdout
        .find('{')
        .unwrap_or_else(|| panic!("verify --json emitted no JSON:\n{stdout}\n{stderr}"));
    let payload: serde_json::Value = serde_json::from_str(&stdout[json_start..])
        .unwrap_or_else(|err| panic!("{err}: invalid verify JSON:\n{stdout}\n{stderr}"));
    let diagnostics = payload["diagnostics"]
        .as_array()
        .unwrap_or_else(|| panic!("verify JSON has no diagnostics array:\n{payload:#}"));
    for atom in ["alias_positive_top", "alias_positive_nested"] {
        assert!(
            !diagnostics.iter().any(|entry| {
                entry["atom"].as_str() == Some(atom)
                    && entry["code"] == "failed"
                    && entry["severity"] == "error"
            }),
            "correct unsigned result must verify for '{atom}':\n{payload:#}"
        );
    }
    for atom in ["alias_false_claim_top", "alias_false_claim_nested"] {
        let errors: Vec<_> = diagnostics
            .iter()
            .filter(|entry| {
                entry["atom"].as_str() == Some(atom)
                    && entry["code"] == "failed"
                    && entry["severity"] == "error"
            })
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "false unsigned result claim must fail for '{atom}':\n{payload:#}"
        );
        assert!(
            errors[0]["message"]
                .as_str()
                .is_some_and(|message| message.contains("Postcondition (ensures) is not satisfied")),
            "false unsigned result claim failed for the wrong reason:\n{payload:#}"
        );
    }
}

#[test]
fn alias_typed_struct_payload_rebinds_use_canonical_nominal_types() {
    let output = run_verify(
        "test_enum_payload_struct_value_alias_rebind.mm",
        &["--json", "--disable-spurious-detection"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "a rebind to a different struct type must be rejected:\n{stdout}\n{stderr}"
    );
    let json_start = stdout
        .find('{')
        .unwrap_or_else(|| panic!("verify --json emitted no JSON:\n{stdout}\n{stderr}"));
    let payload: serde_json::Value = serde_json::from_str(&stdout[json_start..])
        .unwrap_or_else(|err| panic!("{err}: invalid verify JSON:\n{stdout}\n{stderr}"));
    let diagnostics = payload["diagnostics"]
        .as_array()
        .unwrap_or_else(|| panic!("verify JSON has no diagnostics array:\n{payload:#}"));
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|entry| entry["code"] == "failed" && entry["severity"] == "error")
        .collect();
    assert_eq!(
        errors.len(),
        1,
        "only the different-type rebind should fail:\n{payload:#}"
    );
    assert_eq!(
        errors[0]["atom"], "different_type_rebind_is_rejected",
        "the alias-equivalent rebind must be accepted:\n{payload:#}"
    );
    assert!(
        errors[0]["message"].as_str().is_some_and(
            |message| message.contains("shadows a struct binding with a different type")
        ),
        "the different-type rebind failed for the wrong reason:\n{payload:#}"
    );
}

#[test]
fn struct_valued_enum_payload_negatives_are_attributed_per_atom() {
    let output = run_verify(
        "test_enum_payload_struct_value_negative.mm",
        &["--json", "--disable-spurious-detection"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "invalid struct-valued enum payload reads must fail:\n{stdout}\n{stderr}"
    );
    let json_start = stdout
        .find('{')
        .unwrap_or_else(|| panic!("verify --json emitted no JSON:\n{stdout}\n{stderr}"));
    let payload: serde_json::Value = serde_json::from_str(&stdout[json_start..])
        .unwrap_or_else(|err| panic!("{err}: invalid verify JSON:\n{stdout}\n{stderr}"));
    let diagnostics = payload["diagnostics"]
        .as_array()
        .unwrap_or_else(|| panic!("verify JSON has no diagnostics array:\n{payload:#}"));
    let cases: [(&str, Option<&str>); 11] = [
        (
            "bad_wrong_leaf",
            Some("Postcondition (ensures) is not satisfied"),
        ),
        (
            "bad_unnarrowed_parameter",
            Some("Postcondition (ensures) is not satisfied"),
        ),
        (
            "bad_wrong_variant",
            Some("Postcondition (ensures) is not satisfied"),
        ),
        (
            "bad_wrong_outer_variant",
            Some("Postcondition (ensures) is not satisfied"),
        ),
        (
            "bad_nominal_mismatch",
            Some("expects struct 'P' but got struct 'Inner'"),
        ),
        (
            "bad_unknown_field",
            Some("Postcondition (ensures) is not satisfied"),
        ),
        (
            "bad_positional_payload",
            Some("Postcondition (ensures) is not satisfied"),
        ),
        (
            "bad_ambiguous_owner",
            Some("Postcondition (ensures) is not satisfied"),
        ),
        (
            "bad_both_unproven_rebind",
            Some("Postcondition (ensures) is not satisfied"),
        ),
        (
            "bad_both_unproven_rebind_assign",
            Some("Postcondition (ensures) is not satisfied"),
        ),
        (
            "bad_both_unproven_nested_rebind",
            Some("Postcondition (ensures) is not satisfied"),
        ),
    ];
    let expected_errors = cases.len();
    for (atom, expected_message) in cases {
        let atom_errors: Vec<_> = diagnostics
            .iter()
            .filter(|entry| {
                entry["atom"].as_str() == Some(atom)
                    && entry["code"] == "failed"
                    && entry["severity"] == "error"
            })
            .collect();
        assert_eq!(
            atom_errors.len(),
            1,
            "expected exactly one failure for atom '{atom}':\n{payload:#}"
        );
        if let Some(expected_message) = expected_message {
            assert!(
                atom_errors[0]["message"]
                    .as_str()
                    .is_some_and(|message| message.contains(expected_message)),
                "atom '{atom}' failed for the wrong reason:\n{payload:#}"
            );
        }
    }
    assert_eq!(
        diagnostics
            .iter()
            .filter(|entry| entry["code"] == "failed" && entry["severity"] == "error")
            .count(),
        expected_errors,
        "negative fixture has unrelated resolution errors:\n{payload:#}"
    );
}

#[test]
fn unsupported_array_and_recursive_payload_shapes_fail_closed() {
    for (fixture_name, atom) in [
        (
            "test_enum_payload_struct_value_array_negative.mm",
            "bad_array_payload",
        ),
        (
            "test_enum_payload_struct_value_recursive_negative.mm",
            "bad_recursive_payload",
        ),
    ] {
        let output = run_verify(fixture_name, &["--json", "--disable-spurious-detection"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "unsupported payload shape '{atom}' must remain rejected:\n{stdout}\n{stderr}"
        );
        let json_start = stdout.find('{').unwrap_or_else(|| {
            panic!("verify --json emitted no JSON for '{atom}':\n{stdout}\n{stderr}")
        });
        let payload: serde_json::Value = serde_json::from_str(&stdout[json_start..])
            .unwrap_or_else(|err| {
                panic!("{err}: invalid verify JSON for '{atom}':\n{stdout}\n{stderr}")
            });
        let diagnostics = payload["diagnostics"].as_array().unwrap_or_else(|| {
            panic!("verify JSON has no diagnostics array for '{atom}':\n{payload:#}")
        });
        let atom_errors = diagnostics
            .iter()
            .filter(|entry| {
                entry["atom"].as_str() == Some(atom)
                    && entry["code"] == "failed"
                    && entry["severity"] == "error"
            })
            .count();
        assert_eq!(
            atom_errors, 1,
            "expected exactly one failure for atom '{atom}':\n{payload:#}"
        );
    }
}

#[test]
fn struct_valued_enum_payload_reads_lower_expected_aggregate_indices() {
    for (atom, expected) in [
        ("whole_payload_let", &[1, 0, 0][..]),
        ("whole_inner_let", &[1, 0, 0][..]),
        ("nested_payload_subrange_after_sibling", &[1, 1, 0][..]),
    ] {
        let ir = emit_atom_ir("test_enum_payload_struct_value.mm", atom);
        let indices = extract_indices(&ir);
        assert!(
            indices
                .windows(expected.len())
                .any(|window| window == expected),
            "atom '{atom}' expected extractvalue sequence {expected:?}, got {indices:?}:\n{ir}"
        );
    }

    let ir = emit_atom_ir("test_enum_payload_struct_value.mm", "payload_param_pass");
    assert!(
        ir.lines().any(|line| {
            line.contains("extractvalue")
                && line
                    .rsplit_once(", ")
                    .is_some_and(|(_, index)| index.trim() == "1")
        }),
        "get_x(e.p) must extract the enum's P payload at index 1:\n{ir}"
    );
    assert!(
        ir.lines().any(|line| line.contains("call i64 @get_x")),
        "get_x(e.p) must lower as a call with the aggregate payload:\n{ir}"
    );
}
