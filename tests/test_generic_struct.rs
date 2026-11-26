//! Generic struct declarations (`pure-i64-composite/v9`).
//!
//! Covers instantiated parameter/result types (`Pair<i64>`),
//! explicitly instantiated literals (`Pair<i64>{a: x, b: y}`),
//! instantiations nested inside another generic (`Box<Pair<i64>>`)
//! and refutation of wrong postconditions over instantiated fields.

use std::path::Path;
use std::process::Command;

fn run_verify(fixture: &Path, tag: &str) -> std::process::Output {
    let dir = std::env::temp_dir().join(format!("mumei_gen_{}_{}", std::process::id(), tag));
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
fn generic_struct_fields_verify() {
    let out = run_verify(&fixture("test_generic_struct.mm"), "ok");
    assert!(
        out.status.success(),
        "generic-struct fixture must verify\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn contradicted_generic_postconditions_fail() {
    let out = run_verify(&fixture("test_generic_struct_negative.mm"), "neg");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "generic postcondition violations must fail\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        stderr
    );
    for atom in ["bad_sum", "bad_swap", "bad_mk"] {
        assert!(
            stderr.contains(atom),
            "expected {atom} reported in stderr:\n{stderr}"
        );
    }
}
