use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

#[test]
fn recursive_struct_slot_discovery_finishes_within_budget() {
    let dir = tempfile::tempdir().expect("create verification directory");
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("test_recursive_struct_slot_walk.mm");
    let copied = dir.path().join("test_recursive_struct_slot_walk.mm");
    std::fs::copy(source, &copied).expect("copy recursive struct fixture");
    let started = Instant::now();
    let output = Command::new("timeout")
        .arg("8s")
        .arg(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--disable-spurious-detection")
        .arg(&copied)
        .current_dir(dir.path())
        .output()
        .expect("run mumei verify");
    let elapsed = started.elapsed();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("Verification passed"),
        "recursive struct binding did not verify:\n{stdout}\n{stderr}"
    );
    assert!(
        elapsed < Duration::from_secs(8),
        "recursive struct binding took {elapsed:?} (budget: 8 seconds)"
    );
}
