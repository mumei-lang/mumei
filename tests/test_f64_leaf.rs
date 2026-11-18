//! `f64` leaf params/fields/results (`pure-i64-composite/v11`).
//!
//! Covers pass-through copies, call results, struct fields, `let`
//! rebinding, branch merges and enum payloads — and rejection of every
//! arithmetic or comparison read of the bit-pattern leaf.
//!
//! The general verifier gives `f64` Real semantics, so a read like
//! `a < a` verifies there; the pass-through contract is enforced only
//! by fragment admission, which `native-bundle build` drives. Per-atom
//! granularity of the rejection is covered by
//! `composite_v12_rejects_f64_reads` in `semantic_contract`'s tests.

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

fn run_bundle_build(fixture: &Path, tag: &str) -> std::process::Output {
    let dir = std::env::temp_dir().join(format!("mumei_f64_nb_{}_{}", std::process::id(), tag));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale temp dir");
    }
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let copied = dir.join(fixture.file_name().expect("fixture name"));
    std::fs::copy(fixture, &copied).expect("copy fixture");
    let bin = env!("CARGO_BIN_EXE_mumei");
    // `native-bundle build` writes its output beside the working
    // directory — the temp dir keeps parallel runs from colliding.
    let out = Command::new(bin)
        .arg("native-bundle")
        .arg("build")
        .arg(&copied)
        .current_dir(&dir)
        .output()
        .expect("failed to run mumei native-bundle build");
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
fn f64_leaf_bundle_builds() {
    let out = run_bundle_build(&fixture("test_f64_leaf.mm"), "ok");
    assert!(
        out.status.success(),
        "f64-leaf fixture must build a native bundle\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn f64_leaf_reads_fail() {
    // A module whose atoms read f64 leaves (comparison, arithmetic,
    // requires/ensures expressions) is refused by fragment admission.
    let out = run_bundle_build(&fixture("test_f64_leaf_negative.mm"), "neg");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !out.status.success(),
        "f64 leaf reads must fail native-bundle build\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("reject") || stdout.contains("reject"),
        "expected a rejection report\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}
