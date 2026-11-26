use std::path::Path;
use std::process::Command;

#[test]
fn recursive_struct_slot_discovery_finishes_within_budget() {
    let dir = tempfile::tempdir().expect("create verification directory");
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("test_recursive_struct_slot_walk.mm");
    let copied = dir.path().join("test_recursive_struct_slot_walk.mm");
    std::fs::copy(source, &copied).expect("copy recursive struct fixture");
    let output = Command::new("timeout")
        .arg("60s")
        .arg(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--disable-spurious-detection")
        .arg(&copied)
        .current_dir(dir.path())
        .output()
        .expect("run mumei verify");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("Verification passed"),
        "recursive struct binding did not verify or timed out:\n{stdout}\n{stderr}"
    );
}
