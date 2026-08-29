use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

fn report_dir(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("mumei_session_types_{tag}_{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale report dir");
    }
    dir
}

fn run_cross_spec(tag: &str, files: &[&str]) -> (bool, String, Value) {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let dir = report_dir(tag);

    let mut command = Command::new(bin);
    command
        .arg("verify")
        .arg("--report-dir")
        .arg(&dir)
        .arg("--cross-spec-files");
    for file in &files[1..] {
        command.arg(file);
    }
    let output = command
        .arg(files[0])
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify: {err}"));

    let combined = format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report_path = dir.join("cross_spec.json");
    let report = std::fs::read_to_string(&report_path).unwrap_or_else(|err| {
        panic!(
            "failed to read {}: {err}\n{combined}",
            report_path.display()
        )
    });
    let report: Value = serde_json::from_str(&report).expect("valid cross_spec.json");
    (output.status.success(), combined, report)
}

#[test]
fn dual_split_file_protocol_verifies() {
    let (success, log, report) = run_cross_spec(
        "ok",
        &[
            "tests/fixtures/session_types/order_client.mm",
            "tests/fixtures/session_types/order_server.mm",
        ],
    );

    assert!(success, "dual protocol should verify\n{log}");
    assert_eq!(report["summary"]["session_protocol_violation_count"], 0);
    assert!(report["session_protocol_violations"]
        .as_array()
        .expect("session violation array")
        .is_empty());
}

#[test]
fn oversized_protocol_is_reported_as_skipped() {
    let (success, log, report) = run_cross_spec(
        "skip",
        &[
            "tests/fixtures/session_types/bulk_client.mm",
            "tests/fixtures/session_types/bulk_server.mm",
        ],
    );

    assert!(
        success,
        "skipped protocol must not fail verification\n{log}"
    );
    assert_eq!(report["summary"]["session_protocol_violation_count"], 0);

    let skips = report["session_analysis_skips"]
        .as_array()
        .expect("session skip array");
    assert_eq!(skips.len(), 1, "expected one skip in {report:#}");
    assert_eq!(report["summary"]["session_analysis_skipped_count"], 1);

    let skip = &skips[0];
    assert_eq!(skip["effect"], "BulkChannel");
    assert_eq!(skip["reason"], "state_limit_exceeded");
    assert_eq!(skip["state_count"], 33);
    assert_eq!(skip["limit"], 32);
    assert!(skip["message"]
        .as_str()
        .expect("message")
        .contains("BulkChannel"));
    assert!(
        log.contains("session protocol not checked"),
        "skip must be surfaced on the CLI\n{log}"
    );
}

#[test]
fn deadlocking_split_file_protocol_is_a_hard_error() {
    let (success, log, report) = run_cross_spec(
        "deadlock",
        &[
            "tests/fixtures/session_types/payment_client.mm",
            "tests/fixtures/session_types/payment_server.mm",
        ],
    );

    assert!(
        !success,
        "deadlocking protocol must fail verification\n{log}"
    );

    let violations = report["session_protocol_violations"]
        .as_array()
        .expect("session violation array");
    assert_eq!(violations.len(), 1, "expected one violation in {report:#}");
    assert_eq!(
        report["summary"]["session_protocol_violation_count"],
        violations.len()
    );

    let violation = &violations[0];
    assert_eq!(violation["kind"], "deadlock_no_progress");
    assert_eq!(violation["effect"], "PaymentChannel");
    assert_eq!(
        violation["caller_file"],
        "tests/fixtures/session_types/payment_client.mm"
    );
    assert_eq!(
        violation["callee_file"],
        "tests/fixtures/session_types/payment_server.mm"
    );
    assert!(violation["message"].as_str().expect("message").len() > 20);
    assert!(violation["suggested_fix"]
        .as_str()
        .expect("suggested fix")
        .contains("effect_post"));
}
