//! A-1 relational verification: `ensures: result == calc_v1(x);` proves that
//! two verified atoms agree. Calls in a spec are fresh symbols constrained by
//! the callee's `ensures`, so a functional ensures on both atoms is required
//! and trusted callees cannot be related (see docs/SPEC_GUIDE.md).

use std::path::Path;
use std::process::Command;

fn run_verify(fixture: &Path, tag: &str) -> (std::process::Output, String) {
    let dir = std::env::temp_dir().join(format!("mumei_relational_{}_{}", std::process::id(), tag));
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
    let report = std::fs::read_to_string(dir.join("report.json")).unwrap_or_default();
    std::fs::remove_dir_all(dir).ok();
    (out, report)
}

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(name)
}

fn combined(out: &std::process::Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn equivalent_verified_atoms_are_proven_equal() {
    let (out, _) = run_verify(&fixture("test_relational_equiv_ok.mm"), "ok");
    let text = combined(&out);
    assert!(out.status.success(), "v1 == v2 must verify\n{text}");
    assert!(text.contains("'v2_matches_v1': verified"), "{text}");
    assert!(text.contains("'v2_matches_v1_via_let': verified"), "{text}");
}

#[test]
fn differing_atom_fails_with_counterexample() {
    let (out, report) = run_verify(&fixture("test_relational_equiv_mismatch.mm"), "mismatch");
    let text = combined(&out);
    assert!(!out.status.success(), "v1 != v2 must fail\n{text}");
    assert!(text.contains("'calc_v1': verified"), "{text}");
    assert!(text.contains("'calc_v2': verified"), "{text}");
    assert!(
        text.contains("Postcondition (ensures) is not satisfied"),
        "{text}"
    );
    assert!(
        report.contains("\"counterexample\":{\"x\":"),
        "expected a concrete counterexample for x in report.json\n{report}"
    );
}

#[test]
fn trusted_callee_gives_no_congruence() {
    let (out, _) = run_verify(&fixture("test_relational_equiv_trusted.mm"), "trusted");
    let text = combined(&out);
    assert!(
        !out.status.success(),
        "trusted oracle must not be provable\n{text}"
    );
    assert!(
        text.contains("uninterpreted symbols: oracle (trusted_atom)"),
        "{text}"
    );
}
