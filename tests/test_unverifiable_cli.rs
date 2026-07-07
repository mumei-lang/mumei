use std::process::Command;

fn write_fixture(name: &str, source: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mumei_unverifiable_cli_{}_{}",
        name,
        std::process::id()
    ));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale unverifiable fixture dir");
    }
    std::fs::create_dir_all(&dir).expect("create unverifiable fixture dir");
    let path = dir.join("main.mm");
    std::fs::write(&path, source).expect("write unverifiable fixture");
    path
}

#[test]
fn verify_distinguishes_unverifiable_then_failed_atoms() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture = write_fixture(
        "mixed",
        r#"
atom symbolic_pow(x: i64, y: i64) -> i64
  requires: x >= 0;
  ensures: result == x**y && result == x;
  body: x;

atom false_postcondition(x: i64) -> i64
  requires: x >= 0;
  ensures: result == x + 1;
  body: x;
"#,
    );

    let output = Command::new(bin)
        .arg("verify")
        .arg(&fixture)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify: {err}"));

    assert!(
        !output.status.success(),
        "mixed unverifiable+failed fixture should not verify\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("'symbolic_pow': unverifiable ⚠️"),
        "expected symbolic_pow to be unverifiable, got:\n{combined}"
    );
    assert!(
        combined.contains("Phase 5: ensures verification (failed)")
            || combined.contains("'false_postcondition':"),
        "expected false_postcondition failure details, got:\n{combined}"
    );
    assert!(
        combined.contains("❌ Verification: 0 passed, 1 failed, 1 unverifiable")
            || combined.contains("⚠️  Verification: 0 passed, 1 unverifiable")
            || combined.contains("1 failed, 1 unverifiable"),
        "expected mixed failed/unverifiable summary, got:\n{combined}"
    );

    std::fs::remove_dir_all(fixture.parent().unwrap()).expect("remove unverifiable fixture dir");
}

#[test]
fn verify_enforces_all_requires_conjuncts_fresh() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture = write_fixture(
        "multi_requires",
        r#"
atom safe_add_with_bound(x: i64, y: i64) -> i64
  requires: x >= 0 && y >= 0 && x + y <= 2**64 - 1;
  ensures: result == x + y && 0 <= result <= 2**64 - 1;
  body: x + y;
"#,
    );

    let output = Command::new(bin)
        .arg("verify")
        .arg(&fixture)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify: {err}"));

    assert!(
        output.status.success(),
        "multi-requires fixture should verify fresh\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("'safe_add_with_bound': verified ✅"),
        "expected safe_add_with_bound to verify, got:\n{combined}"
    );

    std::fs::remove_dir_all(fixture.parent().unwrap()).expect("remove multi-requires fixture dir");
}

#[test]
fn verify_treats_trivial_ensures_conjunct_as_noop() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture = write_fixture(
        "trivial_ensures",
        r#"
atom trivial_ensures(x: i64) -> i64
  requires: x >= 0;
  ensures: result == x && true;
  body: x;
"#,
    );

    let output = Command::new(bin)
        .arg("verify")
        .arg(&fixture)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify: {err}"));

    assert!(
        output.status.success(),
        "trivial ensures fixture should verify\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("'trivial_ensures': verified ✅"),
        "expected trivial_ensures to verify, got:\n{combined}"
    );

    std::fs::remove_dir_all(fixture.parent().unwrap()).expect("remove trivial ensures fixture dir");
}
