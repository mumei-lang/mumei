use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

#[test]
fn match_bound_nested_struct_field_lookup_finishes_within_budget() {
    let dir = tempfile::tempdir().expect("create verification directory");
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("test_nominal_field_lookup_depth.mm");
    let copied = dir.path().join("test_nominal_field_lookup_depth.mm");
    std::fs::copy(source, &copied).expect("copy nested-field fixture");

    let started = Instant::now();
    let output = Command::new("timeout")
        .arg("12s")
        .arg(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--disable-spurious-detection")
        .arg(&copied)
        .current_dir(dir.path())
        .env("RAYON_NUM_THREADS", "1")
        .output()
        .expect("run mumei verify");
    let elapsed = started.elapsed();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success() && stdout.contains("Verification passed"),
        "deep match-bound field chain did not verify:\n{stdout}\n{stderr}"
    );
    assert!(
        elapsed < Duration::from_secs(8),
        "deep match-bound field chain took {elapsed:?} (budget: 8 seconds)"
    );
}
