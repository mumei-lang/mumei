use serde_json::Value;
use std::{fs, process::Command};

fn run(path: &str, json: bool) -> std::process::Output {
    let root = env!("CARGO_MANIFEST_DIR");
    let _ = fs::remove_dir_all(format!("{root}/tests/positive/.mumei"));
    let _ = fs::remove_dir_all(format!("{root}/tests/negative/.mumei"));
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command.arg("verify");
    if json {
        command.arg("--json");
    }
    command.arg(path);
    command
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run mumei verify")
}

#[test]
fn counter_inference_verifies_and_reports_human_output() {
    let output = run("tests/positive/infer_invariant_counter.mm", false);
    assert!(output.status.success(), "{:?}", output);
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(combined.contains("inferred loop invariant"));
}

#[test]
fn bound_inference_verifies() {
    let output = run("tests/positive/infer_invariant_bound.mm", false);
    assert!(output.status.success(), "{:?}", output);
}

#[test]
fn nested_inference_verifies() {
    let output = run("tests/positive/infer_invariant_nested.mm", false);
    assert!(output.status.success(), "{:?}", output);
}

#[test]
fn nested_inference_reports_each_loop_once() {
    let output = run("tests/positive/infer_invariant_nested.mm", true);
    assert!(output.status.success(), "{:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json_start = stdout
        .find("{\n  \"atom\"")
        .expect("JSON object in verify output");
    let payload: Value =
        serde_json::from_str(&stdout[json_start..]).expect("verify --json payload");
    let reports = payload["inferred_invariants"]
        .as_array()
        .expect("inferred invariant reports");
    assert_eq!(reports.len(), 1);
    let mut lines = reports
        .iter()
        .map(|report| report["line"].as_u64().expect("report line"))
        .collect::<Vec<_>>();
    lines.sort_unstable();
    lines.dedup();
    assert_eq!(lines.len(), 1);
}

#[test]
fn nested_inference_bad_postcondition_fails() {
    let output = run("tests/negative/infer_invariant_nested.mm", false);
    assert!(!output.status.success(), "{:?}", output);
}

#[test]
fn counter_inference_is_present_in_json() {
    let output = run("tests/positive/infer_invariant_counter.mm", true);
    assert!(output.status.success(), "{:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json_start = stdout
        .find("{\n  \"atom\"")
        .expect("JSON object in verify output");
    let payload: Value =
        serde_json::from_str(&stdout[json_start..]).expect("verify --json payload");
    assert!(payload["inferred_invariants"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));
}

#[test]
fn unprovable_inference_still_fails_closed() {
    let output = run("tests/negative/infer_invariant_unprovable.mm", false);
    assert!(!output.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!combined.contains("__mumei_infer_invariant"));
}

#[test]
fn missing_invariant_remains_a_parse_error() {
    let output = run("tests/negative/while_missing_invariant.mm", false);
    assert!(!output.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(combined.contains("requires an 'invariant' clause"));
}

#[test]
fn reserved_inference_snapshot_name_is_rejected() {
    let output = run("tests/negative/infer_invariant_reserved_name.mm", false);
    assert!(!output.status.success(), "{:?}", output);
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("identifier '__loop0_init_i' is reserved for loop invariant inference")
    );
}
