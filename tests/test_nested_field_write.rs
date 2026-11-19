//! Nested field writes `q.inner.a = v` (`pure-i64-composite/v15`).
//!
//! A nested leaf write desugars to a whole-subtree write of the
//! innermost enclosing struct (`q.inner = Inner { a: v, .. }`),
//! re-checking field `where` clauses and struct invariants at every
//! enclosing level and re-homing the base on a fresh handle. The
//! emitter lowers it to an extractvalue-descend / insertvalue-ascend
//! chain.

use std::path::Path;
use std::process::Command;

fn run_verify(fixture: &Path, tag: &str) -> std::process::Output {
    let dir =
        std::env::temp_dir().join(format!("mumei_nested_write_{}_{}", std::process::id(), tag));
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
fn nested_field_writes_verify() {
    let out = run_verify(&fixture("test_nested_field_write.mm"), "ok");
    assert!(
        out.status.success(),
        "nested field-write fixture must verify\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn rejected_nested_field_writes_fail() {
    let out = run_verify(&fixture("test_nested_field_write_negative.mm"), "neg");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "nested field-write violations must fail\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        stderr
    );
    for atom in [
        "bad_keeps_old",
        "bad_mid_path_leaf",
        "bad_unknown_leaf",
        "bad_breaks_invariant",
        "bad_sibling_changed",
        "bad_callee_havoc",
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
        "mumei_nested_write_ir_{}_{}",
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
fn nested_leaf_write_lowers_to_extractvalue_insertvalue_chain() {
    let ir = emit_atom_ir(
        &fixture("test_nested_field_write.mm"),
        "write_leaf",
        "chain",
    );
    // `o.inner.a = v` on `Outer { inner: { a, b }, c }`: descend into
    // slot 0, rebuild the inner aggregate with the new leaf, then
    // rebuild the outer — three ops in extract/insert/insert order.
    let extract = ir
        .lines()
        .position(|l| l.contains("extractvalue { { i64, i64 }, i64 } %0, 0"))
        .expect("extractvalue of the inner aggregate");
    let inner_insert = ir
        .lines()
        .position(|l| l.contains("insertvalue { i64, i64 }") && l.contains(", 0"))
        .expect("insertvalue of the leaf into the inner aggregate");
    let outer_insert = ir
        .lines()
        .position(|l| l.contains("insertvalue { { i64, i64 }, i64 } %0"))
        .expect("insertvalue of the inner aggregate into the outer");
    assert!(
        extract < inner_insert && inner_insert < outer_insert,
        "expected extractvalue → inner insertvalue → outer insertvalue\n{ir}"
    );
}

#[test]
fn three_level_write_lowers_full_depth() {
    let ir = emit_atom_ir(&fixture("test_nested_field_write.mm"), "write_deep", "deep");
    // `tp.mid.m.a = v` descends two extractvalue levels and rebuilds
    // three insertvalue levels.
    assert_eq!(
        ir.matches("extractvalue").count(),
        2,
        "expected two extractvalue descents\n{ir}"
    );
    assert_eq!(
        ir.matches("insertvalue").count(),
        3,
        "expected three insertvalue rebuilds\n{ir}"
    );
}
