//! Enum-typed enum payloads on the general verifier's Datatype path.
//!
//! An `A(E)` payload slot carries the inner enum's own datatype sort:
//! nested `match` binders, constructors over bound inner values, and
//! `==` over the deep leaf shape all verify; a non-finite inner enum
//! keeps the outer on the Int-tag encoding and the ctor rejects.

use std::path::Path;
use std::process::Command;

fn run_verify(fixture: &Path, tag: &str) -> std::process::Output {
    let dir =
        std::env::temp_dir().join(format!("mumei_nested_enum_{}_{}", std::process::id(), tag));
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
fn nested_enum_payloads_verify_on_datatype_path() {
    let out = run_verify(&fixture("test_enum_nested_enum_verify.mm"), "ok");
    assert!(
        out.status.success(),
        "nested enum payload atoms should verify:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn non_finite_inner_enum_keeps_int_tag_and_rejects_ctor() {
    let out = run_verify(&fixture("test_enum_nested_enum_verify_negative.mm"), "neg");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !out.status.success() && !stderr.contains("panicked") && !stdout.contains("panicked"),
        "a recursive inner enum payload must reject the ctor, not panic:\n{stdout}\n{stderr}"
    );
}
