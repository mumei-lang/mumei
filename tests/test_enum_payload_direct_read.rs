use std::path::Path;
use std::process::{Command, Output};

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(name)
}

fn run_verify(name: &str, extra: &[&str]) -> Output {
    let dir = tempfile::tempdir().expect("create fixture directory");
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
    let out = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("build")
        .arg(&copied)
        .arg("--emit")
        .arg("llvm-ir")
        .current_dir(dir.path())
        .output()
        .expect("run mumei build");
    assert!(
        out.status.success(),
        "build failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::read_to_string(dir.path().join(format!("katana_{atom}.ll")))
        .unwrap_or_else(|err| panic!("no emitted IR for atom '{atom}': {err}"))
}

#[test]
fn direct_enum_payload_reads_verify_in_default_and_bitvec_modes() {
    for args in [&[][..], &["--bitvec-i64"][..]] {
        let out = run_verify("test_enum_payload_direct_read.mm", args);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success() && stdout.contains("Verification passed"),
            "direct enum payload reads must verify with {args:?}:\n{stdout}\n{stderr}"
        );
    }
}

#[test]
fn block_root_enum_payload_read_evaluates_side_effect_once() {
    let out = run_verify("test_enum_payload_direct_read_side_effect.mm", &[]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stdout.contains("Verification passed"),
        "block-root payload read must evaluate its root once:\n{stdout}\n{stderr}"
    );
}

#[test]
fn bitvec_u64_direct_payload_reads_preserve_unsigned_comparisons() {
    let out = run_verify("test_enum_payload_direct_read_u64.mm", &["--bitvec-i64"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stdout.contains("Verification passed"),
        "u64 direct payload comparisons must use unsigned semantics:\n{stdout}\n{stderr}"
    );
}

#[test]
fn bitvec_u64_direct_payload_signed_claims_are_rejected_per_atom() {
    let out = run_verify(
        "test_enum_payload_direct_read_u64_negative.mm",
        &["--bitvec-i64", "--json", "--disable-spurious-detection"],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "false u64 direct-read comparisons must fail:\n{stdout}\n{stderr}"
    );
    let json_start = stdout
        .find('{')
        .unwrap_or_else(|| panic!("verify --json emitted no JSON:\n{stdout}\n{stderr}"));
    let payload: serde_json::Value = serde_json::from_str(&stdout[json_start..])
        .unwrap_or_else(|err| panic!("{err}: invalid verify JSON:\n{stdout}\n{stderr}"));
    let diagnostics = payload["diagnostics"]
        .as_array()
        .unwrap_or_else(|| panic!("verify JSON has no diagnostics array:\n{payload:#}"));
    for atom in [
        "bad_struct_payload_signed_compare",
        "bad_scalar_payload_signed_compare",
    ] {
        let failures: Vec<_> = diagnostics
            .iter()
            .filter(|entry| {
                entry["atom"].as_str() == Some(atom)
                    && entry["code"] == "failed"
                    && entry["severity"] == "error"
                    && entry["message"].as_str().is_some_and(|message| {
                        message.contains("Postcondition (ensures) is not satisfied")
                    })
            })
            .collect();
        assert_eq!(
            failures.len(),
            1,
            "expected one false-ensures failure for '{atom}':\n{payload:#}"
        );
    }
}

#[test]
fn direct_enum_payload_read_negatives_keep_their_own_diagnostics() {
    let out = run_verify(
        "test_enum_payload_direct_read_negative.mm",
        &["--json", "--disable-spurious-detection"],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "invalid direct enum payload reads must fail:\n{stdout}\n{stderr}"
    );
    let json_start = stdout
        .find('{')
        .unwrap_or_else(|| panic!("verify --json emitted no JSON:\n{stdout}\n{stderr}"));
    let payload: serde_json::Value = serde_json::from_str(&stdout[json_start..])
        .unwrap_or_else(|err| panic!("{err}: invalid verify JSON:\n{stdout}\n{stderr}"));
    let diagnostics = payload["diagnostics"]
        .as_array()
        .unwrap_or_else(|| panic!("verify JSON has no diagnostics array:\n{payload:#}"));
    for atom in [
        "bad_tag_read",
        "bad_sibling_read",
        "bad_unnarrowed_parameter",
        "bad_wrong_variant_path",
        "bad_requires_without_tag_does_not_narrow",
        "bad_wrong_variant_requires_does_not_bind_payload",
        "bad_outer_wrong_variant_path",
        "bad_unknown_field",
        "bad_positional_payload",
        "bad_ambiguous_owner",
    ] {
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
        assert!(
            atom_errors[0]["message"]
                .as_str()
                .is_some_and(|message| message.contains("Postcondition (ensures) is not satisfied")),
            "atom '{atom}' failed for the wrong reason:\n{payload:#}"
        );
    }
}

#[test]
fn direct_enum_payload_reads_extract_payload_slots_not_tags() {
    let cases: [(&str, &[usize]); 5] = [
        ("read_local_port", &[1]),
        ("read_struct_enum_port", &[0, 1]),
        ("read_outer_enum_port", &[1, 1]),
        ("read_enum_struct_leaf", &[1, 0]),
        ("read_enum_nested_struct_leaf", &[1, 0, 0]),
    ];
    for (atom, expected) in cases {
        let ir = emit_atom_ir("test_enum_payload_direct_read.mm", atom);
        let indices: Vec<usize> = ir
            .lines()
            .filter(|line| line.contains("extractvalue"))
            .filter_map(|line| line.rsplit_once(", ")?.1.trim().parse().ok())
            .collect();
        assert!(
            indices
                .windows(expected.len())
                .any(|window| window == expected),
            "atom '{atom}' expected extractvalue index sequence {expected:?}, got {indices:?}:\n{ir}"
        );
    }
}
