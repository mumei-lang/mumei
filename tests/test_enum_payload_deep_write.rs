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
        &[],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{stdout}\n{stderr}");
    assert!(
        !out.status.success(),
        "invalid deep enum payload writes must fail:\n{combined}"
    );
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
    ] {
        assert!(
            combined.contains(atom) && combined.contains(diagnostic),
            "expected {atom} to fail with '{diagnostic}':\n{combined}"
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
