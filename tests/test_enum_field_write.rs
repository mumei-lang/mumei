//! Enum payload writes `e.p = v` (`pure-i64-composite/v16`).
//!
//! A payload write is defined only where the tag provably carries the
//! single variant owning the named field — the verifier re-checks
//! `is-V(e)` under the current path and rebuilds the value as
//! `V(sel_0(e),…,v,…)`; the emitter lowers it to an `insertvalue` at
//! aggregate index `1 + i`.

use std::path::Path;
use std::process::Command;

fn run_verify(fixture: &Path, tag: &str) -> std::process::Output {
    let dir = std::env::temp_dir().join(format!("mumei_enum_write_{}_{}", std::process::id(), tag));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale temp dir");
    }
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let copied = dir.join(fixture.file_name().expect("fixture name"));
    std::fs::copy(fixture, &copied).expect("copy fixture");
    let bin = env!("CARGO_BIN_EXE_mumei");
    let out = Command::new(bin)
        .arg("verify")
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
fn enum_field_writes_verify() {
    let out = run_verify(&fixture("test_enum_field_write.mm"), "ok");
    assert!(
        out.status.success(),
        "enum field-write fixture must verify\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn rejected_enum_field_writes_fail() {
    let out = run_verify(&fixture("test_enum_field_write_negative.mm"), "neg");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "enum field-write violations must fail\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        stderr
    );
    for atom in [
        "bad_wrong_tag",
        "bad_unknown_tag",
        "bad_ambiguous",
        "bad_unknown_field",
        "bad_sort",
        "bad_refined_payload",
    ] {
        assert!(
            stderr.contains(atom),
            "expected a failure naming '{atom}'\nstderr:\n{stderr}"
        );
    }
}

/// Emit LLVM IR for a fixture file and return the module of `atom`.
fn emit_atom_ir(fixture: &Path, atom: &str, tag: &str) -> String {
    let dir = std::env::temp_dir().join(format!(
        "mumei_enum_write_ir_{}_{}_{}",
        std::process::id(),
        tag,
        atom
    ));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale emit dir");
    }
    std::fs::create_dir_all(&dir).expect("create emit dir");
    let copied = dir.join(fixture.file_name().expect("fixture name"));
    std::fs::copy(fixture, &copied).expect("copy fixture");
    let bin = env!("CARGO_BIN_EXE_mumei");
    let out = Command::new(bin)
        .arg("build")
        .arg(&copied)
        .arg("--emit")
        .arg("llvm-ir")
        .current_dir(&dir)
        .output()
        .expect("failed to run mumei build");
    assert!(
        out.status.success(),
        "build failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let ir = std::fs::read_to_string(dir.join(format!("katana_{atom}.ll")))
        .unwrap_or_else(|err| panic!("no emitted IR for atom {atom}: {err}"));
    std::fs::remove_dir_all(&dir).ok();
    ir
}

#[test]
fn enum_payload_write_lowers_to_insertvalue_past_tag() {
    let ir = emit_atom_ir(&fixture("test_enum_field_write.mm"), "write_slot", "ins");
    // `c.port = p` on `{ i64 tag, slot0 }`: payload slot 0 sits at
    // aggregate index 1 — a single `insertvalue` writing the new leaf.
    assert!(
        ir.contains("insertvalue { i64, i64 }") && ir.contains(", 1"),
        "payload write must lower to insertvalue at index 1\n{ir}"
    );
}

#[test]
fn enum_second_slot_write_lowers_to_index_two() {
    let ir = emit_atom_ir(
        &fixture("test_enum_field_write.mm"),
        "write_keeps_sibling",
        "ins2",
    );
    // `e.r = r` on `{ i64 tag, slot0, slot1 }`: slot 1 → index 2 —
    // the sibling slot (`l`) is carried over by the rebuild.
    assert!(
        ir.contains("insertvalue { i64, i64, i64 }") && ir.contains(", 2"),
        "second-slot write must lower to insertvalue at index 2\n{ir}"
    );
}

#[test]
fn enum_write_named_like_struct_field_uses_enum_slot() {
    let ir = emit_atom_ir(
        &fixture("test_enum_field_write.mm"),
        "write_shadowed_field",
        "ins",
    );
    // `e.l = l` on `{ i64 tag, slot0, slot1 }` must write index 1 —
    // not `Shadow.l`'s index 0, which would overwrite the tag.
    assert!(
        ir.contains("insertvalue { i64, i64, i64 }") && ir.contains(", 1"),
        "payload write must land on the enum slot, not a same-named struct field's index\n{ir}"
    );
}
