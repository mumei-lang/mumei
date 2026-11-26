//! Enum-typed struct fields `struct S { e: E }`
//! (`pure-i64-composite/v17`).
//!
//! An enum field flattens into the tag + payload leaves an enum
//! parameter carries, so `b.e` reads produce a whole enum value
//! consumable by `match`, equality and pass-through, and `b.e = e2`
//! rewrites the whole slot. Payload writes stay closed through the
//! field — v16 covers `e.p = v` on enum bindings only.

use std::path::Path;
use std::process::Command;

fn run_verify(fixture: &Path, tag: &str) -> std::process::Output {
    let dir =
        std::env::temp_dir().join(format!("mumei_enum_struct_{}_{}", std::process::id(), tag));
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
fn enum_struct_fields_verify() {
    let out = run_verify(&fixture("test_enum_struct_field.mm"), "ok");
    assert!(
        out.status.success(),
        "enum-struct-field fixture must verify\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn rejected_enum_struct_fields_fail() {
    let out = run_verify(&fixture("test_enum_struct_field_negative.mm"), "neg");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "enum-struct-field violations must fail\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        stderr
    );
    for atom in [
        "bad_payload_write",
        "bad_tag_write",
        "bad_type_mismatch",
        "bad_scalar_write",
        "bad_tail_write",
        "bad_wrong_tag",
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
        "mumei_enum_struct_ir_{}_{}",
        std::process::id(),
        tag
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
fn enum_field_lowers_as_nested_tagged_union() {
    // `Box` = `{ i64, { i64, i64, i64 } }`: the enum field is the
    // tagged-union aggregate `Color` lowers to (tag + max-arity
    // payload slots), nested as the struct's second slot.
    let ir = emit_atom_ir(&fixture("test_enum_struct_field.mm"), "pick", "ty");
    assert!(
        ir.contains("{ i64, { i64, i64, i64 } }"),
        "expected the enum field as a nested tagged union\n{ir}"
    );
}

#[test]
fn enum_field_match_extracts_tag() {
    let ir = emit_atom_ir(&fixture("test_enum_struct_field.mm"), "pick", "match");
    assert!(
        ir.contains("extractvalue { i64, { i64, i64, i64 } }")
            && ir.matches("extractvalue").count() >= 2,
        "expected extractvalue to reach the field and its tag\n{ir}"
    );
}

#[test]
fn whole_enum_write_lowers_to_insertvalue() {
    let ir = emit_atom_ir(&fixture("test_enum_struct_field.mm"), "repaint", "write");
    assert!(
        ir.lines()
            .any(|l| l.contains("insertvalue { i64, { i64, i64, i64 } }") && l.contains(", 1")),
        "expected the whole-enum write to insert the union aggregate\n{ir}"
    );
}
