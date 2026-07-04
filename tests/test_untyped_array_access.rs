//! Integration tests for the opt-in `untyped_array_access` diagnostic (Item 3).
//!
//! Accessing an array as `arr[i]` without an explicit `[i64]`/`[f64]`/`[bool]`
//! element-type annotation silently falls back to the `i64` element sort. The
//! `--warn-untyped-arrays` flag surfaces this as a warning, and the opt-in
//! `--strict-array-types` flag escalates it to an error. Neither flag changes
//! the default `i64` fallback behavior, and explicitly annotated arrays never
//! trigger the diagnostic.

use std::process::Command;

fn manifest_fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(name)
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("mumei_untyped_arr_{}_{}", std::process::id(), tag));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale temp dir");
    }
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn write_fixture(dir: &std::path::Path, name: &str, source: &str) -> std::path::PathBuf {
    let fixture = dir.join(name);
    std::fs::write(&fixture, source).expect("write fixture");
    fixture
}

fn run_verify(
    fixture: &std::path::Path,
    dir: &std::path::Path,
    extra: &[&str],
) -> std::process::Output {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let mut cmd = Command::new(bin);
    cmd.arg("verify");
    for arg in extra {
        cmd.arg(arg);
    }
    cmd.arg("--report-dir")
        .arg(dir)
        .arg(fixture)
        .current_dir(env!("CARGO_MANIFEST_DIR"));
    cmd.output().expect("failed to run mumei verify")
}

/// Untyped `arr[i]` access is diagnosed under `--warn-untyped-arrays`, but the
/// verification still succeeds (warning-only; the `i64` fallback is preserved).
#[test]
fn warn_untyped_arrays_reports_unannotated_access() {
    let dir = temp_dir("warn");
    let fixture = manifest_fixture("test_untyped_array_access.mm");

    let out = run_verify(&fixture, &dir, &["--warn-untyped-arrays"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "warning-only mode must not fail verification\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        stderr
    );
    assert!(
        stderr.contains("untyped_array_access"),
        "expected an untyped_array_access warning\nstderr:\n{stderr}"
    );

    std::fs::remove_dir_all(dir).ok();
}

/// Without the flag, no `untyped_array_access` diagnostic is emitted (default
/// behavior is unchanged).
#[test]
fn default_mode_is_silent_about_untyped_arrays() {
    let dir = temp_dir("default");
    let fixture = manifest_fixture("test_untyped_array_access.mm");

    let out = run_verify(&fixture, &dir, &[]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("untyped_array_access"),
        "default mode must not emit untyped_array_access diagnostics\nstderr:\n{stderr}"
    );

    std::fs::remove_dir_all(dir).ok();
}

/// Arrays annotated with an explicit element type never trigger the diagnostic,
/// even with the flag on.
#[test]
fn annotated_arrays_do_not_warn() {
    let dir = temp_dir("annotated");
    let source = "atom typed_i64(arr: [i64], n: i64) -> i64\n\
        requires: n >= 1 && forall(i, 0, n, arr[i] >= 0);\n\
        ensures: result >= 0;\n\
        body: arr[0];\n\
        \n\
        atom typed_f64(arr: [f64], n: i64) -> f64\n\
        requires: n >= 1 && forall(i, 0, n, arr[i] >= 0.0);\n\
        ensures: result >= 0.0;\n\
        body: arr[0];\n\
        \n\
        atom typed_bool(arr: [bool], n: i64) -> bool\n\
        requires: n >= 1 && forall(i, 0, n, arr[i] == true);\n\
        ensures: result == true;\n\
        body: arr[0];\n";
    let fixture = write_fixture(&dir, "typed.mm", source);

    let out = run_verify(&fixture, &dir, &["--warn-untyped-arrays"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("untyped_array_access"),
        "explicitly annotated arrays must not warn\nstderr:\n{stderr}"
    );

    std::fs::remove_dir_all(dir).ok();
}

/// The opt-in strict mode escalates the untyped access to an error, failing the
/// run.
#[test]
fn strict_array_types_fails_on_untyped_access() {
    let dir = temp_dir("strict");
    let fixture = manifest_fixture("test_untyped_array_access.mm");

    let out = run_verify(&fixture, &dir, &["--strict-array-types"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "strict mode must fail on untyped array access\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        stderr
    );
    assert!(
        stderr.contains("untyped_array_access"),
        "strict mode should still report the diagnostic\nstderr:\n{stderr}"
    );

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn cli_exposes_untyped_array_flags() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let output = Command::new(bin)
        .arg("verify")
        .arg("--help")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run mumei verify --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--warn-untyped-arrays"),
        "verify --help should advertise --warn-untyped-arrays:\n{stdout}"
    );
    assert!(
        stdout.contains("--strict-array-types"),
        "verify --help should advertise --strict-array-types:\n{stdout}"
    );
}
