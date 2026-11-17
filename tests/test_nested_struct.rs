//! Nested struct fields (`pure-i64-composite/v8`).
//!
//! Covers nested leaf reads (`o.inner.a`), whole-subtree writes
//! (`o.inner = w`), nested fields on `result` and on call results,
//! and refutation of wrong postconditions over nested paths.

use std::path::Path;
use std::process::Command;

fn run_verify(fixture: &Path, tag: &str) -> std::process::Output {
    let dir = std::env::temp_dir().join(format!("mumei_nested_{}_{}", std::process::id(), tag));
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
fn nested_struct_fields_verify() {
    let out = run_verify(&fixture("test_nested_struct.mm"), "ok");
    assert!(
        out.status.success(),
        "nested-struct fixture must verify\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn contradicted_nested_postconditions_fail() {
    let out = run_verify(&fixture("test_nested_struct_negative.mm"), "neg");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "nested postcondition violations must fail\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        stderr
    );
    for atom in ["bad_read", "bad_write", "bad_result"] {
        assert!(
            stderr.contains(atom),
            "expected a failure naming '{atom}'\nstderr:\n{stderr}"
        );
    }
}
