//! Struct-typed enum payloads on the general verifier's Datatype path.
//!
//! A composite payload flattens into one accessor leaf per scalar field:
//! `Ev::Move(p)` binds `p` as a struct value over the selector leaves,
//! `Ev::Move(Point { .. })` and `Ev::Move(p)` expand constructor args to
//! leaves, and `e.p = <struct>` rebuilds the payload leaf-wise under the
//! proven tag gate.

use std::path::Path;
use std::process::Command;

fn run_verify(fixture: &Path, tag: &str) -> std::process::Output {
    run_verify_args(fixture, tag, &[])
}

fn run_verify_args(fixture: &Path, tag: &str, extra: &[&str]) -> std::process::Output {
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
        .args(extra)
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
fn composite_payloads_verify_on_datatype_path() {
    let out = run_verify(&fixture("test_enum_composite_payload_verify.mm"), "ok");
    assert!(
        out.status.success(),
        "composite payload atoms should verify:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn overwide_pattern_reports_a_diagnostic_not_a_panic() {
    let out = run_verify(&fixture("test_enum_overwide_pattern.mm"), "wide");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stderr.contains("panicked") && !stdout.contains("panicked"),
        "an over-wide variant pattern must not panic the verifier:\n{stdout}\n{stderr}"
    );
}

#[test]
fn payload_binding_shadows_outer_struct_without_polluting_it() {
    let out = run_verify(&fixture("test_enum_payload_shadow.mm"), "shadow");
    assert!(
        out.status.success(),
        "a match-bound 'p' is arm-local — the outer struct's fields must survive the merge:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn composite_payloads_verify_under_bitvec_i64() {
    let out = run_verify_args(
        &fixture("test_enum_composite_payload_bitvec.mm"),
        "bv",
        &["--bitvec-i64"],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && !stderr.contains("panicked"),
        "composite payloads must verify in bitvec mode without a sort mismatch:\n{}\n{stderr}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn composite_payload_write_rejects_wrong_tag() {
    let out = run_verify(
        &fixture("test_enum_composite_payload_verify_negative.mm"),
        "neg",
    );
    assert!(
        !out.status.success(),
        "a payload write on the wrong variant must reject"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stderr.contains("requires 'e' to hold variant 'Move'")
            || stdout.contains("requires 'e' to hold variant 'Move'"),
        "the tag-gate diagnostic should name the required variant:\n{stdout}\n{stderr}"
    );
}
