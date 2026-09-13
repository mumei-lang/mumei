//! Exit-code contract of `mumei verify` (see docs/CLI.md "Exit codes").
//!
//! `0` / `1` are verdicts; `3` / `4` / `5` mean the verifier reached no
//! verdict (inconclusive solver, unreadable input, internal failure) and must
//! be distinguishable from a caught counterexample without parsing the summary.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const EXIT_VERIFIED: i32 = 0;
const EXIT_REJECTED: i32 = 1;
const EXIT_INCONCLUSIVE: i32 = 3;
const EXIT_INPUT_ERROR: i32 = 4;

const VERIFIED_SRC: &str = r#"
atom inc(x: Int) -> Int {
  ensures: result == x + 1;
  body: { x + 1 }
}
"#;

const REJECTED_SRC: &str = r#"
atom inc(x: Int) -> Int {
  ensures: result == x + 2;
  body: { x + 1 }
}
"#;

const UNKNOWN_SRC: &str = r#"
atom fermat3(x: i64, y: i64, z: i64) -> i64
requires: x > 0 && y > 0 && z > 0;
ensures: x * x * x + y * y * y != z * z * z;
body: { 0 };
"#;

fn temp_dir(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "mumei_exit_codes_{name}_{}_{}",
        std::process::id(),
        nonce
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn write(dir: &Path, name: &str, source: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, source).expect("write fixture");
    path
}

fn verify(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--solver-timeout")
        .arg("50")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify: {err}"))
}

fn combined_output(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_exit(output: &Output, expected: i32) {
    assert_eq!(
        output.status.code(),
        Some(expected),
        "unexpected exit code\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn verified_program_exits_zero() {
    let dir = temp_dir("verified");
    write(&dir, "ok.mm", VERIFIED_SRC);
    assert_exit(&verify(&dir, &["ok.mm"]), EXIT_VERIFIED);
}

#[test]
fn counterexample_rejection_keeps_exit_code_one() {
    let dir = temp_dir("rejected");
    write(&dir, "bad.mm", REJECTED_SRC);
    let output = verify(&dir, &["bad.mm"]);
    assert_exit(&output, EXIT_REJECTED);
    let combined = combined_output(&output);
    assert!(
        combined.contains("1 failed"),
        "rejection summary should still report the failed obligation:\n{combined}"
    );
}

#[test]
fn solver_unknown_without_counterexample_is_inconclusive() {
    let dir = temp_dir("inconclusive");
    write(&dir, "unknown.mm", UNKNOWN_SRC);
    let output = verify(&dir, &["unknown.mm"]);
    assert_exit(&output, EXIT_INCONCLUSIVE);
    let combined = combined_output(&output);
    assert!(
        combined.contains("1 failed"),
        "summary counts stay unchanged; only the exit code differs:\n{combined}"
    );
}

#[test]
fn unreadable_input_is_an_input_error() {
    let dir = temp_dir("missing");
    assert_exit(&verify(&dir, &["does_not_exist.mm"]), EXIT_INPUT_ERROR);
}

#[test]
fn empty_directory_is_an_input_error() {
    let dir = temp_dir("empty_dir");
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).expect("create empty dir");
    assert_exit(&verify(&dir, &["empty"]), EXIT_INPUT_ERROR);
}

#[test]
fn directory_run_reports_the_most_severe_outcome() {
    let dir = temp_dir("directory");

    let verified_only = dir.join("verified");
    std::fs::create_dir_all(&verified_only).expect("create dir");
    write(&verified_only, "ok.mm", VERIFIED_SRC);
    assert_exit(&verify(&dir, &["verified"]), EXIT_VERIFIED);

    let inconclusive = dir.join("inconclusive");
    std::fs::create_dir_all(&inconclusive).expect("create dir");
    write(&inconclusive, "ok.mm", VERIFIED_SRC);
    write(&inconclusive, "unknown.mm", UNKNOWN_SRC);
    assert_exit(&verify(&dir, &["inconclusive"]), EXIT_INCONCLUSIVE);

    // A rejection dominates an inconclusive sibling: the directory *is* wrong.
    let rejected = dir.join("rejected");
    std::fs::create_dir_all(&rejected).expect("create dir");
    write(&rejected, "bad.mm", REJECTED_SRC);
    write(&rejected, "unknown.mm", UNKNOWN_SRC);
    assert_exit(&verify(&dir, &["rejected"]), EXIT_REJECTED);
}
