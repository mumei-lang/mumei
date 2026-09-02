//! A user alias of a Copy base type (`type Usd = i64 unit USD;`) must be Copy
//! in MIR move analysis: branching on such a parameter and returning it on one
//! path must not be reported as a move conflict.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn alias_of_copy_base_is_copy_in_move_analysis() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("test_alias_movability.mm");
    let dir = std::env::temp_dir().join(format!("mumei_alias_mv_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let out = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--report-dir")
        .arg(&dir)
        .arg(&fixture)
        .current_dir(&dir)
        .output()
        .expect("failed to run mumei verify");
    std::fs::remove_dir_all(&dir).ok();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{text}");
    assert!(!text.contains("conflicting ownership"), "{text}");
}
