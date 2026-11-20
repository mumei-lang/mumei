//! Composite-typed enum payloads `enum E { V(pos: Point) }`
//! (`pure-i64-composite/v18`).
//!
//! A payload slot may name an admitted struct: it carries the
//! struct's flattened leaves as one aggregate value, `match` binds it
//! as a composite readable by field, and call pass-through keeps the
//! whole payload. Constructor literals, bound-composite ctor args,
//! whole-payload writes and enum equality are covered by the
//! trusted-verifier unit tests in `mumei-core::semantic_contract`;
//! the general verifier still keeps composite payloads on the Int-tag
//! path, so the end-to-end fixture exercises the match side only.

use std::path::Path;
use std::process::Command;

fn run_verify(fixture: &Path, tag: &str) -> std::process::Output {
    let dir = std::env::temp_dir().join(format!(
        "mumei_enum_composite_{}_{}",
        std::process::id(),
        tag
    ));
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
fn enum_composite_payloads_verify() {
    let out = run_verify(&fixture("test_enum_composite_payload.mm"), "ok");
    assert!(
        out.status.success(),
        "enum-composite-payload fixture must verify\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn rejected_enum_composite_payloads_fail() {
    let out = run_verify(&fixture("test_enum_composite_payload_negative.mm"), "neg");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "enum-composite-payload violations must fail\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        stderr
    );
    for atom in [
        "bad_scalar_ctor",
        "bad_scalar_write",
        "bad_wrong_tag",
        "bad_unknown_tag",
        "bad_ambiguous",
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
        "mumei_enum_composite_ir_{}_{}",
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
fn composite_payload_lowers_as_aggregate_slot() {
    // `Event` = `{ i64, { i64, i64 } }`: tag plus one payload slot
    // holding the `Point` aggregate.
    let ir = emit_atom_ir(&fixture("test_enum_composite_payload.mm"), "step", "ty");
    assert!(
        ir.contains("{ i64, { i64, i64 } }"),
        "expected the composite payload as an aggregate slot\n{ir}"
    );
}

#[test]
fn composite_payload_leaf_reads_lower_to_extractvalue() {
    // `read_x` matches the `Move` arm and reads `p.x` — the bound
    // composite's first leaf lowers to `extractvalue` on the payload
    // aggregate nested inside the enum value.
    let ir = emit_atom_ir(&fixture("test_enum_composite_payload.mm"), "read_x", "read");
    assert!(
        ir.lines()
            .any(|l| l.contains("extractvalue") && l.contains("{ i64, i64 }")),
        "expected composite leaf reads as extractvalue\n{ir}"
    );
}

#[test]
fn composite_payload_u64_leaf_compares_unsigned() {
    // The match-bound `p` records `UPoint` in `var_types`, so `p.x > 0`
    // on a `u64` leaf lowers to an unsigned compare.
    let ir = emit_atom_ir(&fixture("test_enum_composite_payload.mm"), "uread", "u");
    assert!(
        ir.contains("icmp ugt"),
        "expected an unsigned compare on the u64 leaf\n{ir}"
    );
}
