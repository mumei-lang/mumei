//! `f64` leaf params/fields/results (`pure-i64-composite/v11`).
//!
//! Covers pass-through copies, call results, struct fields, `let`
//! rebinding, branch merges and enum payloads — and rejection of every
//! arithmetic or comparison read of the bit-pattern leaf.

use std::path::Path;
use std::process::Command;

fn run_verify(fixture: &Path, tag: &str) -> std::process::Output {
    let dir = std::env::temp_dir().join(format!("mumei_f64_{}_{}", std::process::id(), tag));
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
fn f64_leaves_verify() {
    let out = run_verify(&fixture("test_f64_leaf.mm"), "ok");
    assert!(
        out.status.success(),
        "f64-leaf fixture must verify\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn f64_leaf_reads_fail() {
    let out = run_verify(&fixture("test_f64_leaf_negative.mm"), "neg");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "f64 leaf reads must fail\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        stderr
    );
    for atom in ["read_cmp", "read_eq", "read_add", "mix", "read_req"] {
        assert!(
            stderr.contains(atom),
            "expected {atom} reported in stderr:\n{stderr}"
        );
    }
}
