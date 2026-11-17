//! Struct field assignment `q.f = v` (`pure-i64-composite/v5`).
//!
//! Covers the post-state contract: written fields read their new
//! value at the atom boundary and at call sites, unwritten fields keep
//! input-time facts, and `let` shadowing of a struct binding is
//! rejected unless the new value re-initialises the same struct type.

use std::path::Path;
use std::process::Command;

fn run_verify(fixture: &Path, tag: &str) -> std::process::Output {
    let dir =
        std::env::temp_dir().join(format!("mumei_field_write_{}_{}", std::process::id(), tag));
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
fn field_writes_verify() {
    let out = run_verify(&fixture("test_struct_field_write.mm"), "ok");
    assert!(
        out.status.success(),
        "field-write fixture must verify\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn contradicted_postconditions_fail() {
    let out = run_verify(&fixture("test_struct_field_write_negative.mm"), "neg");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "caller_bad and write_wrong must fail verification\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        stderr
    );
    for atom in ["caller_bad", "write_wrong"] {
        assert!(
            stderr.contains(atom),
            "expected a failure naming '{atom}'\nstderr:\n{stderr}"
        );
    }
}

#[test]
fn struct_shadow_lets_are_rejected() {
    let out = run_verify(&fixture("test_struct_field_write_shadow.mm"), "shadow");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "scalar/different-struct shadow lets must be rejected\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        stderr
    );
    assert!(
        stderr.contains("shadows a struct binding"),
        "expected the shadow rejection message\nstderr:\n{stderr}"
    );
}
