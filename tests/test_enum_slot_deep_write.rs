use std::path::Path;
use std::process::Command;

fn run_verify(fixture: &Path, tag: &str, extra: &[&str]) -> std::process::Output {
    let dir = std::env::temp_dir().join(format!(
        "mumei_enum_slot_deep_{}_{}",
        std::process::id(),
        tag
    ));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale temp dir");
    }
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let copied = dir.join(fixture.file_name().expect("fixture name"));
    std::fs::copy(fixture, &copied).expect("copy fixture");
    let out = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .args(extra)
        .arg("--report-dir")
        .arg(&dir)
        .arg(&copied)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run mumei verify");
    std::fs::remove_dir_all(dir).ok();
    out
}

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(name)
}

#[test]
fn enum_slot_deep_writes_verify_in_default_and_bitvec_modes() {
    for (tag, args) in [("positive", &[][..]), ("bitvec", &["--bitvec-i64"][..])] {
        let out = run_verify(&fixture("test_enum_slot_deep_write.mm"), tag, args);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success() && stdout.contains("Verification passed"),
            "enum-slot deep writes must verify ({tag}):\n{stdout}\n{stderr}"
        );
    }
}

#[test]
fn invalid_enum_slot_deep_writes_fail_for_their_own_diagnostics() {
    let out = run_verify(
        &fixture("test_enum_slot_deep_write_negative.mm"),
        "negative",
        &["--json", "--disable-spurious-detection"],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "invalid enum-slot deep writes must fail:\n{stdout}\n{stderr}"
    );
    let json_start = stdout
        .find('{')
        .unwrap_or_else(|| panic!("verify --json emitted no JSON:\n{stdout}\n{stderr}"));
    let payload: serde_json::Value = serde_json::from_str(&stdout[json_start..])
        .unwrap_or_else(|err| panic!("{err}: invalid verify JSON:\n{stdout}\n{stderr}"));
    let diagnostics = payload["diagnostics"]
        .as_array()
        .unwrap_or_else(|| panic!("verify JSON has no diagnostics array:\n{payload:#}"));
    let cases: [(&str, &[&str]); 16] = [
        (
            "bad_wrong_inner_tag",
            &["requires '__enum_slot_", "to hold variant 'Tcp'"],
        ),
        (
            "bad_unknown_inner_tag",
            &["requires '__enum_slot_", "to hold variant 'Tcp'"],
        ),
        (
            "bad_wrong_outer_tag",
            &["requires 'o' to hold variant 'Wrap'"],
        ),
        (
            "bad_unnarrowed_parameter",
            &["requires 'o' to hold variant 'Wrap'"],
        ),
        (
            "bad_unknown_field",
            &["unknown resource state 'b.e.missing'"],
        ),
        (
            "bad_positional_inner_payload",
            &["unknown resource state 'b.e.port'"],
        ),
        (
            "bad_ambiguous_inner_owner",
            &["field 'port' is a payload of more than one variant"],
        ),
        (
            "bad_scalar_instead_of_enum",
            &["unknown resource state 'b.e.port'"],
        ),
        ("bad_array_path", &["unknown resource state 'b.e.port'"]),
        (
            "bad_recursive_enum_path",
            &["unknown resource state 'b.e.next.next'"],
        ),
        (
            "bad_refined_inner_payload",
            &["type 'NonNegative' predicate violated"],
        ),
        (
            "bad_slot_rhs_double_eval",
            &["Postcondition (ensures) is not satisfied"],
        ),
        (
            "bad_slot_rebinds_base",
            &["field write 'b.e.port' value must not rebind 'b'"],
        ),
        (
            "bad_slot_false_ensures",
            &["Postcondition (ensures) is not satisfied"],
        ),
        (
            "bad_slot_direct_read_false_ensures",
            &["Postcondition (ensures) is not satisfied"],
        ),
        (
            "bad_outer_slot_direct_read_false_ensures",
            &["Postcondition (ensures) is not satisfied"],
        ),
    ];
    for (atom, expected_fragments) in cases {
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
            "expected exactly one error diagnostic for atom '{atom}':\n{payload:#}"
        );
        let message = atom_errors[0]["message"]
            .as_str()
            .unwrap_or_else(|| panic!("diagnostic has no message for '{atom}':\n{payload:#}"));
        for fragment in expected_fragments {
            assert!(
                message.contains(fragment),
                "atom '{atom}' failed for the wrong reason; expected '{fragment}' in '{message}'\n{payload:#}"
            );
        }
    }

    let atom = "bad_nominal_enum_slot_write";
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
        "expected exactly one error diagnostic for atom '{atom}':\n{payload:#}"
    );
    assert!(
        atom_errors[0]["message"].as_str().is_some_and(|message| {
            message.contains("field 'e.p' of 'b'")
                && message.contains("expects struct 'Inner' but got struct 'OtherInner'")
        }),
        "nominal enum-slot write failed for the wrong reason:\n{payload:#}"
    );
}

fn emit_atom_ir(fixture: &Path, atom: &str, tag: &str) -> String {
    let dir = std::env::temp_dir().join(format!(
        "mumei_enum_slot_deep_ir_{}_{}",
        std::process::id(),
        tag
    ));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale emit dir");
    }
    std::fs::create_dir_all(&dir).expect("create emit dir");
    let copied = dir.join(fixture.file_name().expect("fixture name"));
    std::fs::copy(fixture, &copied).expect("copy fixture");
    let out = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("build")
        .arg(&copied)
        .arg("--emit")
        .arg("llvm-ir")
        .current_dir(&dir)
        .output()
        .expect("failed to run mumei build");
    assert!(
        out.status.success(),
        "build failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let ir = std::fs::read_to_string(dir.join(format!("katana_{atom}.ll")))
        .unwrap_or_else(|err| panic!("no emitted IR for atom {atom}: {err}"));
    std::fs::remove_dir_all(dir).ok();
    ir
}

#[test]
fn enum_slot_deep_write_extracts_and_rebuilds_expected_indices() {
    let ir = emit_atom_ir(
        &fixture("test_enum_slot_deep_write.mm"),
        "enum_slot_ir",
        "enum-slot-ir",
    );
    let extract_indices: Vec<String> = ir
        .lines()
        .filter(|line| line.contains("extractvalue"))
        .map(|line| {
            line.rsplit_once(", ")
                .expect("extractvalue aggregate index")
                .1
                .to_string()
        })
        .collect();
    let insert_indices: Vec<String> = ir
        .lines()
        .filter(|line| line.contains("insertvalue"))
        .map(|line| {
            line.rsplit_once(", ")
                .expect("insertvalue aggregate index")
                .1
                .to_string()
        })
        .collect();
    assert!(
        extract_indices == ["1"] && insert_indices == ["1", "1"],
        "enum-slot write must extract and rebuild the outer and inner enum slots:\n{ir}"
    );
}

#[test]
fn struct_enum_slot_write_rebuilds_from_the_updated_struct_field() {
    let ir = emit_atom_ir(
        &fixture("test_enum_slot_deep_write.mm"),
        "struct_slot_ir",
        "struct-slot-ir",
    );
    let extract_indices: Vec<String> = ir
        .lines()
        .filter(|line| line.contains("extractvalue"))
        .map(|line| {
            line.rsplit_once(", ")
                .expect("extractvalue aggregate index")
                .1
                .to_string()
        })
        .collect();
    let insert_indices: Vec<String> = ir
        .lines()
        .filter(|line| line.contains("insertvalue"))
        .map(|line| {
            line.rsplit_once(", ")
                .expect("insertvalue aggregate index")
                .1
                .to_string()
        })
        .collect();
    assert!(
        extract_indices == ["0"] && insert_indices == ["1", "0"],
        "struct enum-slot write must extract and rebuild the enum field:\n{ir}"
    );
}

#[test]
fn enum_slot_reread_extracts_from_the_rebuilt_aggregate() {
    let ir = emit_atom_ir(
        &fixture("test_enum_slot_deep_write.mm"),
        "reread_struct_slot_preserves_siblings",
        "reread",
    );
    let lines: Vec<_> = ir.lines().collect();
    let (insert_index, root_insert) = lines
        .iter()
        .enumerate()
        .rfind(|(_, line)| line.contains("insertvalue"))
        .expect("rebuilt enum aggregate");
    let rebuilt = root_insert
        .split_once(" = insertvalue")
        .expect("insertvalue result")
        .0
        .trim();
    assert!(
        lines
            .iter()
            .skip(insert_index + 1)
            .any(|line| line.contains("extractvalue") && line.contains(&format!(" {rebuilt}, "))),
        "post-write match must extract from the rebuilt struct aggregate:\n{ir}"
    );
}

#[test]
fn enum_slot_direct_read_extracts_the_payload_not_the_tag() {
    let ir = emit_atom_ir(
        &fixture("test_enum_slot_deep_write.mm"),
        "struct_parameter_match_arm_slot",
        "direct-read",
    );
    assert!(
        ir.lines().any(|line| {
            line.contains("extractvalue { i64, i64, i64 }")
                && line
                    .rsplit_once(", ")
                    .is_some_and(|(_, index)| index == "1")
        }),
        "read after b.e.port write must extract Chan's named payload at index 1:\n{ir}"
    );
}

#[test]
fn generic_nested_struct_fields_keep_unsigned_ops() {
    let ir = emit_atom_ir(
        &fixture("test_enum_slot_deep_write.mm"),
        "generic_holder_unsigned_ops",
        "generic-u64",
    );
    assert!(
        ir.contains("icmp ugt"),
        "expected unsigned comparison:\n{ir}"
    );
    assert!(ir.contains("udiv"), "expected unsigned division:\n{ir}");
}

#[test]
fn enum_slot_deep_write_rejects_a_union_widened_slot() {
    let dir = std::env::temp_dir().join(format!(
        "mumei_enum_slot_deep_widened_{}",
        std::process::id()
    ));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale widened emit dir");
    }
    std::fs::create_dir_all(&dir).expect("create widened emit dir");
    let source = fixture("test_enum_slot_deep_write_widened.mm");
    let copied = dir.join(source.file_name().expect("fixture name"));
    std::fs::copy(source, &copied).expect("copy widened fixture");
    let out = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("build")
        .arg(&copied)
        .arg("--emit")
        .arg("llvm-ir")
        .current_dir(&dir)
        .output()
        .expect("failed to run mumei build");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::remove_dir_all(dir).ok();
    assert!(
        !out.status.success() && combined.contains("does not match its declared enum type"),
        "union-widened enum payload slots must fail closed:\n{combined}"
    );
}
