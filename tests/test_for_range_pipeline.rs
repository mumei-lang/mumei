use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn mumei_verify_uncached(file: &str) -> (bool, String) {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(file);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("mumei_for_range_{}_{}", std::process::id(), nonce));
    fs::create_dir_all(&dir).expect("fixture dir");
    let dst = dir.join("main.mm");
    fs::copy(&src, &dst).expect("copy fixture");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg(&dst)
        .current_dir(&dir)
        .output()
        .expect("run mumei verify");
    let _ = fs::remove_dir_all(&dir);
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.success(), combined)
}

#[test]
fn for_range_and_pipeline_fixture_verifies() {
    let (ok, out) = mumei_verify_uncached("tests/test_for_range_sugar.mm");
    assert!(ok, "for-range and pipeline fixture must verify:\n{out}");
    for atom in [
        "count_range",
        "sum_range_nonneg",
        "inc",
        "dbl",
        "pipe_demo",
        "approval_level_fixture",
    ] {
        assert!(
            out.contains(&format!("'{atom}': verified")),
            "expected {atom} to verify:\n{out}"
        );
    }
}

#[test]
fn bad_for_invariant_is_rejected_non_vacuously() {
    let (ok, out) = mumei_verify_uncached("tests/negative/test_for_range_bad_invariant.mm");
    assert!(!ok, "invalid for invariant must fail verification:\n{out}");
    assert!(
        out.contains("bad_for_invariant"),
        "the loop fixture must be parsed and checked, not silently skipped:\n{out}"
    );
    assert!(
        !out.contains("syntax error"),
        "bad invariant should reach verification rather than fail parsing:\n{out}"
    );
}

#[test]
fn missing_for_range_separator_is_rejected() {
    let (ok, out) = mumei_verify_uncached("tests/negative/test_for_range_missing_bounds.mm");
    assert!(!ok, "missing `..` must fail:\n{out}");
    assert!(
        out.contains("for loop requires '..'"),
        "missing `..` must be rejected by the parser:\n{out}"
    );
}
