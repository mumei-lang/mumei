//! `usize`/`isize` leaf params/fields/results: `usize` folds to `u64`
//! and `isize` to `i64` on the 64-bit target (`pure-i64-composite/v10`).
//!
//! Covers unsigned comparison and division on `usize`, wrap
//! arithmetic, signed `isize` ops, width-typed struct fields, and
//! refutation of unsigned-impossible requires / wrong postconditions.

use std::path::Path;
use std::process::Command;

fn run_verify(fixture: &Path, tag: &str) -> std::process::Output {
    let dir = std::env::temp_dir().join(format!("mumei_usize_{}_{}", std::process::id(), tag));
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
fn usize_isize_leaves_verify() {
    let out = run_verify(&fixture("test_usize_leaf.mm"), "ok");
    assert!(
        out.status.success(),
        "usize/isize-leaf fixture must verify\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn usize_wrong_facts_fail() {
    let out = run_verify(&fixture("test_usize_leaf_negative.mm"), "neg");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "usize/isize violations must fail\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        stderr
    );
    for atom in ["vacuous", "bad_add", "underflow"] {
        assert!(
            stderr.contains(atom),
            "expected {atom} reported in stderr:\n{stderr}"
        );
    }
}
