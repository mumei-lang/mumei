use std::path::Path;
use std::process::Command;

fn run_verify(fixture: &Path, tag: &str, extra: &[&str]) -> std::process::Output {
    let dir = std::env::temp_dir().join(format!(
        "mumei_enum_payload_deep_{}_{}",
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
fn deep_enum_payload_write_verifies() {
    for (tag, args) in [("positive", &[][..]), ("bitvec", &["--bitvec-i64"][..])] {
        let out = run_verify(&fixture("test_enum_payload_deep_write.mm"), tag, args);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success() && stdout.contains("Verification passed"),
            "deep enum payload write must verify ({tag}):\n{stdout}\n{stderr}"
        );
    }
}

#[test]
fn invalid_deep_enum_payload_writes_fail_closed() {
    let out = run_verify(
        &fixture("test_enum_payload_deep_write_negative.mm"),
        "negative",
        &["--json", "--disable-spurious-detection"],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "invalid deep enum payload writes must fail:\n{stdout}\n{stderr}"
    );
    let json_start = stdout
        .find('{')
        .unwrap_or_else(|| panic!("verify --json emitted no JSON:\n{stdout}\n{stderr}"));
    let payload: serde_json::Value = serde_json::from_str(&stdout[json_start..])
        .unwrap_or_else(|err| panic!("{err}: invalid verify JSON:\n{stdout}\n{stderr}"));
    let diagnostics = payload["diagnostics"]
        .as_array()
        .unwrap_or_else(|| panic!("verify JSON has no diagnostics array:\n{payload:#}"));
    for (atom, diagnostic) in [
        ("bad_wrong_tag", "requires 'e' to hold variant 'Move'"),
        ("bad_unknown_tag", "requires 'e' to hold variant 'Move'"),
        ("bad_nested_invariant", "Struct 'InBox' invariant violated"),
        (
            "bad_refined_alias",
            "Struct 'RefinedPoint' field 'x' type 'NonNegative' predicate violated",
        ),
        (
            "bad_unknown_field",
            "unknown resource state 'e.p.inner.missing'",
        ),
        ("bad_scalar_payload", "unknown resource state 'e.value.x'"),
        (
            "bad_positional_payload",
            "unknown resource state 'e.Outer.inner.x'",
        ),
        (
            "bad_ambiguous_owner",
            "field 'payload' is a payload of more than one variant",
        ),
        (
            "bad_false_ensures",
            "Postcondition (ensures) is not satisfied",
        ),
        (
            "bad_struct_rhs_double_eval",
            "Postcondition (ensures) is not satisfied",
        ),
        (
            "bad_enum_rhs_double_eval",
            "Postcondition (ensures) is not satisfied",
        ),
        (
            "bad_nominal_deep_payload_write",
            "expects struct 'Inner' but got struct 'OtherInner'",
        ),
        (
            "bad_reread_sibling_relation",
            "Postcondition (ensures) is not satisfied",
        ),
        (
            "bad_deep_write_rebinds_base",
            "field write 'o.inner.x' value must not rebind 'o'",
        ),
    ] {
        let atom_diagnostics: Vec<_> = diagnostics
            .iter()
            .filter(|entry| entry["atom"].as_str() == Some(atom))
            .collect();
        let matching = atom_diagnostics
            .iter()
            .filter(|entry| {
                entry["code"] == "failed"
                    && entry["severity"] == "error"
                    && entry["message"]
                        .as_str()
                        .is_some_and(|message| message.contains(diagnostic))
            })
            .count();
        assert!(
            matching == 1,
            "expected exactly one diagnostic for atom '{atom}' containing '{diagnostic}', found {matching} in:\n{payload:#}"
        );
    }
}

fn emit_atom_ir(fixture: &Path, atom: &str, tag: &str) -> String {
    let dir = std::env::temp_dir().join(format!(
        "mumei_enum_payload_deep_ir_{}_{}",
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
fn deep_enum_payload_write_rebuilds_nested_aggregate_indices() {
    let ir = emit_atom_ir(
        &fixture("test_enum_payload_deep_write.mm"),
        "deep_write",
        "ir",
    );
    let extracts: Vec<_> = ir
        .lines()
        .filter(|line| line.contains("extractvalue"))
        .collect();
    let inserts: Vec<_> = ir
        .lines()
        .filter(|line| line.contains("insertvalue"))
        .collect();
    let extract_indices: Vec<String> = extracts
        .iter()
        .map(|line| {
            line.rsplit_once(", ")
                .expect("extractvalue aggregate index")
                .1
                .to_string()
        })
        .collect();
    let insert_indices: Vec<String> = inserts
        .iter()
        .map(|line| {
            line.rsplit_once(", ")
                .expect("insertvalue aggregate index")
                .1
                .to_string()
        })
        .collect();
    assert!(
        extract_indices == ["1", "0"] && insert_indices == ["0", "0", "1"],
        "deep payload write must extract/rebuild the enum slot and nested struct fields:\n{ir}"
    );
}

#[test]
fn deep_enum_payload_write_re_reads_from_the_rebuilt_aggregate() {
    let ir = emit_atom_ir(
        &fixture("test_enum_payload_deep_write.mm"),
        "preserves_siblings",
        "reread",
    );
    let lines: Vec<_> = ir.lines().collect();
    let (insert_index, root_insert) = lines
        .iter()
        .enumerate()
        .rfind(|(_, line)| line.contains("insertvalue"))
        .expect("enum aggregate rebuild");
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
        "post-write match must extract from the rebuilt enum value:\n{ir}"
    );
}

#[test]
fn deep_enum_payload_write_rejects_union_widened_struct_slot() {
    let dir = std::env::temp_dir().join(format!(
        "mumei_enum_payload_deep_widened_{}",
        std::process::id()
    ));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale widened emit dir");
    }
    std::fs::create_dir_all(&dir).expect("create widened emit dir");
    let source = fixture("test_enum_payload_deep_write_widened.mm");
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
        !out.status.success() && combined.contains("does not match its declared struct type"),
        "widened payload slots must fail closed:\n{combined}"
    );
}
