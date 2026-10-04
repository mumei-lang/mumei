//! Without `--report-dir`, per-atom `report.json` is staged in a private temp
//! dir and published to the run's cwd on exit. Two concurrent `mumei verify`
//! runs in the same directory used to overwrite each other's report.json, so
//! a `--json` run could print the other run's atom.
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

static DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn fresh_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mumei_report_staging_{}_{}_{}_{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default(),
        DIR_COUNTER.fetch_add(1, Ordering::SeqCst),
    ));
    std::fs::create_dir_all(&dir).expect("create test dir");
    dir
}

fn verify_json(dir: &Path, file: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--json")
        .arg(file)
        .current_dir(dir)
        .output()
        .expect("run mumei verify --json")
}

fn spawn_verify_json(dir: &Path, file: &str) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--json")
        .arg(file)
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn mumei verify --json")
}

fn stdout_json(output: &Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json_start = stdout
        .find('{')
        .unwrap_or_else(|| panic!("no JSON in stdout:\n{stdout}"));
    serde_json::from_str(&stdout[json_start..]).expect("verify --json payload")
}

fn copy_fixture(dir: &Path, src: &str, dst_name: &str) -> PathBuf {
    let dst = dir.join(dst_name);
    std::fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join(src), &dst).expect("copy fixture");
    dst
}

#[test]
fn concurrent_runs_in_same_cwd_print_their_own_atom() {
    let dir = fresh_dir("concurrent");
    copy_fixture(
        &dir,
        "tests/positive/infer_invariant_counter.mm",
        "counter.mm",
    );
    copy_fixture(
        &dir,
        "tests/positive/infer_invariant_nested.mm",
        "nested.mm",
    );
    for iteration in 0..20 {
        let counter = spawn_verify_json(&dir, "counter.mm");
        let nested = spawn_verify_json(&dir, "nested.mm");
        let counter_out = counter.wait_with_output().expect("counter run");
        let nested_out = nested.wait_with_output().expect("nested run");
        assert_eq!(
            stdout_json(&counter_out)["atom"],
            serde_json::json!("infer_counter"),
            "iteration {iteration}: counter run printed the wrong atom"
        );
        assert_eq!(
            stdout_json(&nested_out)["atom"],
            serde_json::json!("infer_nested"),
            "iteration {iteration}: nested run printed the wrong atom"
        );
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn report_json_is_published_to_cwd() {
    let dir = fresh_dir("publish");
    copy_fixture(
        &dir,
        "tests/positive/infer_invariant_counter.mm",
        "counter.mm",
    );
    let output = verify_json(&dir, "counter.mm");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("report.json")).expect("report.json in cwd"),
    )
    .expect("parse report.json");
    assert_eq!(report["atom"], serde_json::json!("infer_counter"));
    let leftover_tmps = std::fs::read_dir(&dir)
        .expect("read cwd")
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .count();
    assert_eq!(leftover_tmps, 0, "staging tmp files must not leak into cwd");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn stale_report_json_is_removed_when_run_writes_none() {
    let dir = fresh_dir("stale");
    // A module with no atoms writes no report.json.
    std::fs::write(dir.join("empty.mm"), "// nothing to verify\n").expect("write fixture");
    std::fs::write(dir.join("report.json"), "{\"atom\":\"stale\"}").expect("seed report.json");
    let output = verify_json(&dir, "empty.mm");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !dir.join("report.json").exists(),
        "a run that writes no report.json must remove a stale one"
    );
    std::fs::remove_dir_all(&dir).ok();
}

const EXIT_INPUT_ERROR: i32 = 4;

fn seed_stale_report(dir: &Path) {
    std::fs::write(dir.join("report.json"), "{\"atom\":\"stale_atom\"}").expect("seed report.json");
}

fn verify(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run mumei verify")
}

fn assert_input_error_and_report_gone(output: &Output, report_dir: &Path) {
    assert_eq!(
        output.status.code(),
        Some(EXIT_INPUT_ERROR),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !report_dir.join("report.json").exists(),
        "an early exit must remove a stale report.json"
    );
}

#[test]
fn early_exit_on_missing_input_removes_stale_report_in_cwd() {
    let dir = fresh_dir("early_missing");
    seed_stale_report(&dir);
    let output = verify(&dir, &["does_not_exist.mm"]);
    assert_input_error_and_report_gone(&output, &dir);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn early_exit_on_missing_input_removes_stale_report_in_report_dir() {
    let dir = fresh_dir("early_missing_rd");
    let report_dir = fresh_dir("early_missing_rd_out");
    seed_stale_report(&report_dir);
    let output = verify(
        &dir,
        &[
            "--report-dir",
            report_dir.to_str().expect("utf8 report dir"),
            "does_not_exist.mm",
        ],
    );
    assert_input_error_and_report_gone(&output, &report_dir);
    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&report_dir).ok();
}

#[test]
fn early_exit_on_missing_cross_spec_removes_stale_report() {
    let dir = fresh_dir("early_xspec");
    copy_fixture(
        &dir,
        "tests/positive/infer_invariant_counter.mm",
        "counter.mm",
    );
    seed_stale_report(&dir);
    let output = verify(
        &dir,
        &["--cross-spec-files", "does_not_exist.mm", "counter.mm"],
    );
    assert_input_error_and_report_gone(&output, &dir);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn early_exit_on_empty_directory_removes_stale_report() {
    let dir = fresh_dir("early_emptydir");
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).expect("create empty dir");
    seed_stale_report(&dir);
    let output = verify(&dir, &["empty"]);
    assert_input_error_and_report_gone(&output, &dir);
    std::fs::remove_dir_all(&dir).ok();
}
