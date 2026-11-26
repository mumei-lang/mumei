use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_dir(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "mumei_verify_json_failure_type_{name}_{}_{}",
        std::process::id(),
        nonce + DIR_COUNTER.fetch_add(1, Ordering::SeqCst) as u128
    ));
    std::fs::create_dir_all(&directory).expect("create fixture directory");
    directory
}

fn run_fixture(fixture_name: &str) -> Value {
    let root = temp_dir(fixture_name);
    let cwd = root.join("cwd");
    let home = root.join("home");
    let reports = root.join("reports");
    std::fs::create_dir_all(&cwd).expect("create fixture working directory");
    std::fs::create_dir_all(&home).expect("create isolated home");
    let input = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(fixture_name);
    let output: Output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg(&input)
        .arg("--json")
        .arg("--cache-scope")
        .arg("global")
        .arg("--report-dir")
        .arg(&reports)
        .current_dir(&cwd)
        .env("HOME", &home)
        .env_remove("MUMEI_PROOF_BUNDLE")
        .output()
        .expect("run mumei verify");
    assert_eq!(
        output.status.code(),
        Some(1),
        "expected verification rejection for {fixture_name}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "parse verify --json output for {fixture_name}: {error}\n{}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn diagnostic_for<'a>(payload: &'a Value, atom: &str) -> &'a Value {
    payload["diagnostics"]
        .as_array()
        .unwrap_or_else(|| panic!("missing diagnostics array: {payload:#}"))
        .iter()
        .find(|diagnostic| diagnostic["atom"].as_str() == Some(atom))
        .unwrap_or_else(|| panic!("missing diagnostic for {atom}: {payload:#}"))
}

#[test]
fn precondition_failure_type_is_present_on_summary_and_main_diagnostic() {
    let payload = run_fixture("precondition_fail.mm");
    let expected = "precondition_violated";

    assert_eq!(payload["failure_type"], expected);
    assert_eq!(diagnostic_for(&payload, "main")["failure_type"], expected);
}

#[test]
fn effect_mismatch_preserves_failure_and_violation_types() {
    let payload = run_fixture("effect_mismatch.mm");
    let expected_diagnostic = diagnostic_for(&payload, "write_log");

    assert_eq!(payload["failure_type"], "effect_not_allowed");
    assert_eq!(payload["violation_type"], "effect_mismatch");
    assert_eq!(expected_diagnostic["failure_type"], "effect_not_allowed");
    assert_eq!(expected_diagnostic["violation_type"], "effect_mismatch");
    assert_eq!(payload["effect_violation"]["required_effect"], "FileWrite");
    assert_eq!(
        payload["effect_violation"]["source_operation"],
        "FileWrite.write"
    );
}

#[test]
fn linearity_failure_type_is_inferred_for_summary_and_diagnostic() {
    let payload = run_fixture("lin.mm");
    let expected = "linearity_violated";

    assert_eq!(payload["failure_type"], expected);
    assert_eq!(
        diagnostic_for(&payload, "use_twice")["failure_type"],
        expected
    );
}

#[test]
fn temporal_effect_failure_type_is_inferred_for_summary_and_diagnostic() {
    let payload = run_fixture("tmp.mm");
    let expected = "temporal_effect_violated";

    assert_eq!(payload["failure_type"], expected);
    assert_eq!(
        diagnostic_for(&payload, "bad_file_usage")["failure_type"],
        expected
    );
}
