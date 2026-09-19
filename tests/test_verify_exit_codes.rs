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
const EXIT_USAGE_ERROR: i32 = 2;
const EXIT_INCONCLUSIVE: i32 = 3;
const EXIT_INPUT_ERROR: i32 = 4;
const EXIT_INTERNAL_ERROR: i32 = 5;

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

#[test]
fn unsupported_emit_targets_are_usage_errors_not_verdicts() {
    let dir = temp_dir("bad_emit");
    write(&dir, "ok.mm", VERIFIED_SRC);
    assert_exit(
        &verify(&dir, &["--emit", "not-a-target", "ok.mm"]),
        EXIT_USAGE_ERROR,
    );
    assert_exit(
        &verify(&dir, &["--no-emit", "not-a-target", "ok.mm"]),
        EXIT_USAGE_ERROR,
    );
}

#[test]
fn unreadable_cross_spec_file_is_an_input_error() {
    let dir = temp_dir("cross_spec_missing");
    write(&dir, "ok.mm", VERIFIED_SRC);
    assert_exit(
        &verify(&dir, &["--cross-spec-files", "does_not_exist.mm", "ok.mm"]),
        EXIT_INPUT_ERROR,
    );
}

/// A stub `mumei-lean` bridge that echoes the bundle back unchanged, i.e.
/// discharges nothing. Mirrors the shape used in test_escalation_bundle_emit.
fn write_noop_bridge(dir: &Path) -> PathBuf {
    let bridge_repo = dir.join("mumei-lean");
    let scripts = bridge_repo.join("scripts");
    std::fs::create_dir_all(&scripts).expect("create stub bridge dir");
    std::fs::write(
        scripts.join("bridge.py"),
        r#"
import argparse, json
from pathlib import Path
p = argparse.ArgumentParser()
p.add_argument("--escalation-bundle", required=True)
p.add_argument("--lean-cert-out", required=True)
p.add_argument("--out-dir")
a = p.parse_args()
payload = json.loads(Path(a.escalation_bundle).read_text())
payload["lean_cert_schema_version"] = "1.0-lean"
Path(a.lean_cert_out).write_text(json.dumps(payload))
"#,
    )
    .expect("write stub bridge");
    bridge_repo
}

#[test]
fn undischarged_lean_escalation_candidate_is_inconclusive() {
    let dir = temp_dir("open_escalation");
    let bridge_repo = write_noop_bridge(&dir);
    write(&dir, "unknown.mm", UNKNOWN_SRC);
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--solver-timeout")
        .arg("50")
        .arg("--escalate-lean")
        .arg("--proof-cert")
        .arg("--output")
        .arg("unknown.proof.json")
        .arg("unknown.mm")
        .env("MUMEI_LEAN_PATH", &bridge_repo)
        .current_dir(&dir)
        .output()
        .expect("run mumei verify --escalate-lean");
    assert_exit(&output, EXIT_INCONCLUSIVE);
    let combined = combined_output(&output);
    assert!(
        combined.contains("still open"),
        "summary should say the Lean candidate is still open:\n{combined}"
    );
}

#[test]
fn artifact_write_failure_is_an_internal_error() {
    let dir = temp_dir("artifact_failure");
    write(&dir, "ok.mm", VERIFIED_SRC);
    // `--output` points at a path whose parent is a regular file, so the
    // certificate cannot be written even though the program verified.
    write(&dir, "blocker", "");
    assert_exit(
        &verify(
            &dir,
            &["--proof-cert", "--output", "blocker/cert.json", "ok.mm"],
        ),
        EXIT_INTERNAL_ERROR,
    );
}

const LAW_UNKNOWN_SRC: &str = r#"
trait Cubic {
    fn f(a: Self, b: Self, c: Self) -> bool;
    law fermat: f(x, y, z) == true;
}

impl Cubic for i64 {
    fn f(a: i64, b: i64, c: i64) -> bool { a <= 0 || b <= 0 || c <= 0 || a * a * a + b * b * b != c * c * c }
}

atom inc(x: Int) -> Int {
  ensures: result == x + 1;
  body: { x + 1 }
}
"#;

#[test]
fn trait_law_solver_unknown_is_inconclusive_not_verified() {
    let dir = temp_dir("law_unknown");
    write(&dir, "law.mm", LAW_UNKNOWN_SRC);
    let output = verify(&dir, &["law.mm"]);
    assert_exit(&output, EXIT_INCONCLUSIVE);
    assert!(
        combined_output(&output).contains("Z3 returned unknown"),
        "law failure must be reported as an unknown, not a counterexample"
    );
}

#[test]
fn missing_z3_cli_still_verifies_via_linked_libz3() {
    // Verification talks to the linked libz3, not a `z3` executable, so an
    // empty PATH must not be treated as "solver not found".
    let dir = temp_dir("missing_z3");
    write(&dir, "ok.mm", VERIFIED_SRC);
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("ok.mm")
        .env("PATH", dir.as_os_str())
        .current_dir(&dir)
        .output()
        .expect("run mumei verify without z3 on PATH");
    assert_exit(&output, EXIT_VERIFIED);
    assert!(!combined_output(&output).contains("Z3 solver not found"));
}

#[test]
fn json_payload_reports_the_exit_code_it_returns() {
    let dir = temp_dir("json_outcome");
    write(&dir, "law.mm", LAW_UNKNOWN_SRC);
    let output = verify(&dir, &["--json", "law.mm"]);
    assert_exit(&output, EXIT_INCONCLUSIVE);
    let payload: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("--json prints a JSON payload");
    assert_eq!(payload["exit_code"], serde_json::json!(EXIT_INCONCLUSIVE));
    assert_eq!(payload["status"], serde_json::json!("inconclusive"));

    write(&dir, "ok.mm", VERIFIED_SRC);
    write(&dir, "blocker", "");
    let output = verify(
        &dir,
        &[
            "--json",
            "--proof-cert",
            "--output",
            "blocker/cert.json",
            "ok.mm",
        ],
    );
    assert_exit(&output, EXIT_INTERNAL_ERROR);
    let payload: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("--json prints a JSON payload");
    assert_eq!(payload["status"], serde_json::json!("internal_error"));
    assert_eq!(payload["exit_code"], serde_json::json!(EXIT_INTERNAL_ERROR));
    assert_eq!(payload["infra_errors"], serde_json::json!(1));
}

#[test]
fn json_input_error_still_emits_a_summary_payload() {
    let dir = temp_dir("json_missing");
    let output = verify(&dir, &["--json", "does_not_exist.mm"]);
    assert_exit(&output, EXIT_INPUT_ERROR);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let payload: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is one JSON document");
    assert_eq!(payload["status"], serde_json::json!("input_error"));
    assert_eq!(payload["exit_code"], serde_json::json!(EXIT_INPUT_ERROR));
    assert_eq!(payload["verified"], serde_json::json!(0));
    assert!(payload["diagnostics"][0]["message"].is_string());
}
