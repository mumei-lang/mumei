//! Units of measure (`type Usd = i64 unit USD;`).
//!
//! Units are compile-time tags checked on `+`, `-` and comparisons; they do
//! not change the Z3 encoding, so unit-annotated fixtures verify exactly like
//! their unitless counterparts and mismatches are rejected before any proof.

use std::path::Path;
use std::process::Command;

fn run_verify(fixture: &Path, tag: &str) -> std::process::Output {
    let dir = std::env::temp_dir().join(format!("mumei_units_{}_{}", std::process::id(), tag));
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

fn combined(out: &std::process::Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn matching_units_verify() {
    let out = run_verify(&fixture("test_units_ok.mm"), "ok");
    let text = combined(&out);
    assert!(
        out.status.success(),
        "unit-consistent fixture must verify\n{text}"
    );
    assert!(
        !text.contains("Unit mismatch"),
        "no unit error expected\n{text}"
    );
}

fn assert_unit_mismatch(name: &str, tag: &str, lhs: &str, rhs: &str) {
    let out = run_verify(&fixture(name), tag);
    let text = combined(&out);
    assert!(!out.status.success(), "{name} must be rejected\n{text}");
    assert!(
        text.contains("Unit mismatch"),
        "{name}: expected unit error\n{text}"
    );
    assert!(
        text.contains(&format!("'{lhs}' with '{rhs}'")),
        "{name}: expected units {lhs}/{rhs} in message\n{text}"
    );
}

#[test]
fn usd_plus_jpy_is_rejected() {
    assert_unit_mismatch("test_units_mismatch_add.mm", "add", "USD", "JPY");
}

#[test]
fn meter_compared_to_second_is_rejected() {
    assert_unit_mismatch("test_units_mismatch_cmp.mm", "cmp", "Meter", "Second");
}

#[test]
fn returning_usd_as_jpy_is_rejected() {
    assert_unit_mismatch("test_units_mismatch_return.mm", "ret", "JPY", "USD");
}

#[test]
fn passing_jpy_to_usd_parameter_is_rejected() {
    assert_unit_mismatch("test_units_mismatch_call.mm", "call", "USD", "JPY");
}

/// Backward compatibility: refinement-only aliases and unannotated atoms are
/// untouched by the unit checker.
#[test]
fn unannotated_fixture_still_verifies() {
    let out = run_verify(&fixture("test_call_with_contract.mm"), "compat");
    let text = combined(&out);
    assert!(
        out.status.success(),
        "legacy fixture must still verify\n{text}"
    );
    assert!(!text.contains("Unit mismatch"), "{text}");
}
