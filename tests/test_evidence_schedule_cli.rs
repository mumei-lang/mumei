use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Output, Stdio};

const FIXTURES: &[(&str, &str, &str, usize)] = &[
    (
        "attempt_schedule_selected.json",
        include_str!("fixtures/attempt_schedule_selected.json"),
        "selected",
        1,
    ),
    (
        "attempt_schedule_selected_reported.json",
        include_str!("fixtures/attempt_schedule_selected_reported.json"),
        "selected",
        1,
    ),
    (
        "attempt_schedule_selected_evidence_check.json",
        include_str!("fixtures/attempt_schedule_selected_evidence_check.json"),
        "selected",
        1,
    ),
    (
        "attempt_schedule_exhausted.json",
        include_str!("fixtures/attempt_schedule_exhausted.json"),
        "exhausted",
        2,
    ),
    (
        "attempt_schedule_not_schedulable.json",
        include_str!("fixtures/attempt_schedule_not_schedulable.json"),
        "not_schedulable",
        0,
    ),
    (
        "attempt_schedule_fallback_denied.json",
        include_str!("fixtures/attempt_schedule_fallback_denied.json"),
        "fallback_denied",
        1,
    ),
    (
        "attempt_schedule_cancelled.json",
        include_str!("fixtures/attempt_schedule_cancelled.json"),
        "cancelled",
        0,
    ),
];

const SELECTED_WITH_CHECK: &str =
    include_str!("fixtures/attempt_schedule_selected_evidence_check.json");

fn run(args: &[&str], stdin_bytes: Option<&[u8]>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if stdin_bytes.is_some() {
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }
    let mut child = command.spawn().expect("spawn mumei");
    if let Some(bytes) = stdin_bytes {
        child
            .stdin
            .take()
            .expect("piped stdin")
            .write_all(bytes)
            .expect("write stdin");
    }
    child.wait_with_output().expect("wait for mumei")
}

fn stdin_check(bytes: &[u8]) -> Output {
    run(
        &["evidence", "check-schedule", "--stdin", "--json"],
        Some(bytes),
    )
}

fn json_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("encode test schedule")
}

fn assert_invalid(label: &str, bytes: &[u8]) {
    let output = stdin_check(bytes);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "{label} was accepted: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        stderr.contains("invalid attempt-schedule representation"),
        "{label} did not use the static representation error: {stderr}"
    );
    assert!(
        !stderr.contains("FAKE_PRIVATE_SCHEDULE"),
        "{label} echoed input: {stderr}"
    );
}

#[test]
fn accepts_agent_fixtures_with_identical_file_and_stdin_diagnostics() {
    let directory = tempfile::tempdir().expect("temporary fixture directory");
    for (file_name, record, outcome, attempt_count) in FIXTURES {
        let path = directory.path().join(file_name);
        std::fs::write(&path, record).expect("write agent schedule fixture");
        let path = path.to_str().expect("UTF-8 temporary path");
        let file_output = run(&["evidence", "check-schedule", path, "--json"], None);
        let stdin_output = stdin_check(record.as_bytes());
        assert!(
            file_output.status.success(),
            "{file_name}: {}",
            String::from_utf8_lossy(&file_output.stderr)
        );
        assert!(
            stdin_output.status.success(),
            "{file_name}: {}",
            String::from_utf8_lossy(&stdin_output.stderr)
        );
        assert_eq!(file_output.stdout, stdin_output.stdout, "{file_name}");

        let diagnostic: Value =
            serde_json::from_slice(&file_output.stdout).expect("JSON diagnostic");
        assert_eq!(
            diagnostic,
            json!({
                "schema_version": "mumei.attempt-schedule-check/v1",
                "schedule_schema_version": "mumei.attempt-schedule/v1",
                "representation": "valid",
                "acceptance": "not_evaluated",
                "outcome": outcome,
                "attempt_count": attempt_count,
                "reported_outcome_meaning": "adapter_self_report",
                "checks_performed": ["representation_gate", "schedule_consistency"],
                "checks_not_performed": [
                    "adapter_execution",
                    "reported_outcome_truth",
                    "evidence_acceptance",
                    "embedded_acceptance_validation",
                    "claim_truth",
                    "producer_authenticity",
                    "freshness",
                    "resource_enforcement",
                    "grade_promotion",
                ],
            })
        );
        assert!(!String::from_utf8_lossy(&file_output.stdout).contains("alpha"));
        assert!(!String::from_utf8_lossy(&file_output.stdout)
            .contains("bb158efd9ea0b93f334d4d4aa5e8f616a4e210651868965bf2054d1cf03f48c0"));
    }
}

#[test]
fn accepts_optional_enforcement_and_rejects_mixed_presence() {
    let mut selected: Value =
        serde_json::from_str(include_str!("fixtures/attempt_schedule_selected.json"))
            .expect("agent schedule fixture");
    selected["attempts"][0]["enforcement"] = json!({
        "environment": "minimal",
        "network": "isolated",
        "filesystem_write": "permitted",
        "filesystem_read": "unenforced",
        "process_spawn": "isolated",
        "memory_bound_mb": null
    });
    let accepted = stdin_check(&json_bytes(&selected));
    assert!(
        accepted.status.success(),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );

    let mut missing_enforcement_key = selected.clone();
    missing_enforcement_key["attempts"][0]["enforcement"]
        .as_object_mut()
        .expect("enforcement object")
        .remove("filesystem_read");
    assert_invalid(
        "missing enforcement key",
        &json_bytes(&missing_enforcement_key),
    );

    let mut mixed: Value =
        serde_json::from_str(include_str!("fixtures/attempt_schedule_exhausted.json"))
            .expect("agent schedule fixture");
    mixed["attempts"][0]["enforcement"] = selected["attempts"][0]["enforcement"].clone();
    assert_invalid("mixed enforcement presence", &json_bytes(&mixed));
}

#[test]
fn text_diagnostic_is_one_static_line() {
    let output = run(
        &["evidence", "check-schedule", "--stdin"],
        Some(include_str!("fixtures/attempt_schedule_selected.json").as_bytes()),
    );
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).expect("UTF-8 text"),
        "Attempt-schedule representation valid: outcome selected; 1 attempt(s). Acceptance not evaluated; no adapter execution or grade promotion.\n"
    );
}

#[test]
fn accepts_only_the_declared_embedded_acceptance_schema_versions() {
    for schema_version in [
        "mumei.evidence-acceptance/v1",
        "mumei.evidence-requirement-list-acceptance/v1",
    ] {
        let mut schedule: Value =
            serde_json::from_str(SELECTED_WITH_CHECK).expect("agent schedule fixture");
        schedule["evidence_acceptance"]["schema_version"] = json!(schema_version);
        assert_invalid(
            "producer-never-emitted acceptance schema",
            &json_bytes(&schedule),
        );
    }
}

#[test]
fn embedded_acceptance_is_closed_and_consistent() {
    let mut schedule: Value =
        serde_json::from_str(SELECTED_WITH_CHECK).expect("agent schedule fixture");
    schedule["evidence_acceptance"]["extra"] = json!("not accepted");
    assert_invalid("extra embedded acceptance key", &json_bytes(&schedule));

    let mut schedule: Value =
        serde_json::from_str(SELECTED_WITH_CHECK).expect("agent schedule fixture");
    schedule["evidence_acceptance"]["candidate_match"] = json!(false);
    assert_invalid("accepted without a candidate match", &json_bytes(&schedule));
}

#[test]
fn embedded_acceptance_fields_must_match_the_evidence_check() {
    let mismatched = [
        ("request_fingerprint", json!("a".repeat(64))),
        ("attempt_number", json!(2)),
        ("output_fingerprint", json!("a".repeat(64))),
        ("outcome", json!("failed")),
    ];
    for (field, value) in mismatched {
        let mut schedule: Value =
            serde_json::from_str(SELECTED_WITH_CHECK).expect("agent schedule fixture");
        schedule["evidence_acceptance"][field] = value;
        assert_invalid("embedded acceptance field mismatch", &json_bytes(&schedule));
    }

    for field in [
        "request_fingerprint",
        "attempt_number",
        "output_fingerprint",
        "outcome",
    ] {
        let mut schedule: Value =
            serde_json::from_str(SELECTED_WITH_CHECK).expect("agent schedule fixture");
        schedule["evidence_acceptance"]
            .as_object_mut()
            .expect("acceptance object")
            .remove(field);
        assert_invalid(
            "missing embedded acceptance binding field",
            &json_bytes(&schedule),
        );
    }
}

#[test]
fn evidence_check_must_bind_to_the_selected_attempt_request() {
    let mut schedule: Value =
        serde_json::from_str(SELECTED_WITH_CHECK).expect("agent schedule fixture");
    schedule["attempts"][0]["request_fingerprint"] = json!("a".repeat(64));
    assert_invalid(
        "evidence check differs from selected attempt request",
        &json_bytes(&schedule),
    );

    let mut schedule: Value =
        serde_json::from_str(SELECTED_WITH_CHECK).expect("agent schedule fixture");
    schedule["attempts"][0]["request_fingerprint"] = Value::Null;
    assert_invalid(
        "evidence check has no selected attempt request",
        &json_bytes(&schedule),
    );
}

#[test]
fn rejects_invalid_shapes_types_labels_fingerprints_and_consistency() {
    let selected: Value =
        serde_json::from_str(include_str!("fixtures/attempt_schedule_selected.json"))
            .expect("agent schedule fixture");
    let exhausted: Value =
        serde_json::from_str(include_str!("fixtures/attempt_schedule_exhausted.json"))
            .expect("agent schedule fixture");
    let mut cases = Vec::new();

    let mut value = selected.clone();
    value["schema_version"] = json!("mumei.attempt-schedule/v2");
    cases.push(("wrong schedule schema", json_bytes(&value)));

    let mut value = selected.clone();
    value["outcome"] = json!("unknown");
    cases.push(("unknown schedule outcome", json_bytes(&value)));

    let mut value = selected.clone();
    value["unknown"] = json!(true);
    cases.push(("unknown top-level key", json_bytes(&value)));

    let mut value = selected.clone();
    value
        .as_object_mut()
        .expect("object")
        .remove("selected_adapter_id");
    cases.push(("missing top-level key", json_bytes(&value)));

    let mut value = selected.clone();
    value["max_attempts"] = json!(true);
    cases.push(("boolean max_attempts", json_bytes(&value)));

    let mut value = selected.clone();
    value["fallback_permitted"] = json!(1);
    cases.push(("non-boolean fallback_permitted", json_bytes(&value)));

    let mut value = selected.clone();
    value["attempts"] = json!(null);
    cases.push(("non-array attempts", json_bytes(&value)));

    for max_attempts in [0, 9] {
        let mut value = selected.clone();
        value["max_attempts"] = json!(max_attempts);
        cases.push(("out-of-range max_attempts", json_bytes(&value)));
    }

    let mut value = selected.clone();
    value["selected_adapter_id"] = json!(" alpha");
    value["attempts"][0]["adapter_id"] = json!(" alpha");
    cases.push(("bad adapter label", json_bytes(&value)));

    let mut value = selected.clone();
    value["attempts"][0]["adapter_id"] = json!("a".repeat(129));
    cases.push(("adapter label too long", json_bytes(&value)));

    for fingerprint in ["A".repeat(64), "a".repeat(63)] {
        let mut value = selected.clone();
        value["attempts"][0]["plan_fingerprint"] = json!(fingerprint);
        cases.push(("bad plan fingerprint", json_bytes(&value)));
    }

    let mut value = selected.clone();
    value["attempts"][0]["request_fingerprint"] = json!("A".repeat(64));
    cases.push(("bad request fingerprint", json_bytes(&value)));

    let mut value = selected.clone();
    value["attempts"][0]["ordinal"] = json!(2);
    cases.push(("ordinal gap", json_bytes(&value)));

    let mut value = selected.clone();
    value["attempts"][0]["ordinal"] = json!(true);
    cases.push(("boolean ordinal", json_bytes(&value)));

    let mut value = exhausted.clone();
    value["max_attempts"] = json!(1);
    cases.push(("attempt count exceeds max_attempts", json_bytes(&value)));

    let mut value = selected.clone();
    value["attempts"][0]["extra"] = json!(null);
    cases.push(("unknown attempt key", json_bytes(&value)));

    let mut value = selected.clone();
    value["attempts"][0]
        .as_object_mut()
        .expect("attempt object")
        .remove("exit_code");
    cases.push(("missing attempt key", json_bytes(&value)));

    let mut value = selected.clone();
    value["attempts"][0]["exit_code"] = json!(true);
    cases.push(("boolean exit code", json_bytes(&value)));

    let mut value = selected.clone();
    value["attempts"][0]["terminal_state"] = json!("unknown");
    cases.push(("unknown terminal state", json_bytes(&value)));

    let mut value = selected.clone();
    value["attempts"][0]["reported_outcome"] = json!("unknown");
    cases.push(("unknown reported outcome", json_bytes(&value)));

    let mut value = selected.clone();
    value["selected_adapter_id"] = Value::Null;
    cases.push(("selected without adapter", json_bytes(&value)));

    let mut value = selected.clone();
    value["selected_adapter_id"] = json!("beta");
    cases.push((
        "selected adapter differs from final attempt",
        json_bytes(&value),
    ));

    let mut value = selected.clone();
    value["attempts"][0]["terminal_state"] = json!("failed");
    value["attempts"][0]["exit_code"] = Value::Null;
    cases.push(("selected final attempt not exited", json_bytes(&value)));

    let mut value: Value = serde_json::from_str(SELECTED_WITH_CHECK).expect("agent fixture");
    value["outcome"] = json!("exhausted");
    value["selected_adapter_id"] = Value::Null;
    cases.push(("evidence check on unselected schedule", json_bytes(&value)));

    let mut value = selected.clone();
    value["outcome"] = json!("exhausted");
    value["selected_adapter_id"] = Value::Null;
    value["attempts"][0]["terminal_state"] = json!("failed");
    value["attempts"][0]["exit_code"] = Value::Null;
    value["attempts"][0]["reported_outcome"] = json!("valid");
    cases.push(("reported outcome on non-exited attempt", json_bytes(&value)));

    let mut value = selected.clone();
    value["outcome"] = json!("exhausted");
    value["selected_adapter_id"] = Value::Null;
    value["attempts"][0]["terminal_state"] = json!("failed");
    cases.push(("exit code on non-exited attempt", json_bytes(&value)));

    let mut value: Value = serde_json::from_str(SELECTED_WITH_CHECK).expect("agent fixture");
    value["evidence_check"]["outcome"] = json!("unexpected");
    cases.push(("bad evidence-check outcome", json_bytes(&value)));

    let mut value: Value = serde_json::from_str(SELECTED_WITH_CHECK).expect("agent fixture");
    value["evidence_check"]["attempt_number"] = json!(true);
    cases.push(("boolean evidence-check attempt number", json_bytes(&value)));

    let mut value: Value = serde_json::from_str(SELECTED_WITH_CHECK).expect("agent fixture");
    value["evidence_check"]["request_fingerprint"] = json!("A".repeat(64));
    cases.push(("bad evidence-check fingerprint", json_bytes(&value)));

    let mut value: Value = serde_json::from_str(SELECTED_WITH_CHECK).expect("agent fixture");
    value["evidence_check"]["schema_version"] = json!("mumei.unknown-check/v1");
    cases.push(("wrong evidence-check schema", json_bytes(&value)));

    let mut value: Value = serde_json::from_str(SELECTED_WITH_CHECK).expect("agent fixture");
    value["evidence_check"]["extra"] = json!(null);
    cases.push(("unknown evidence-check key", json_bytes(&value)));

    let mut value: Value = serde_json::from_str(SELECTED_WITH_CHECK).expect("agent fixture");
    value["evidence_acceptance"]["schema_version"] = json!("mumei.unknown-acceptance/v1");
    cases.push(("unknown embedded acceptance schema", json_bytes(&value)));

    for (label, bytes) in cases {
        assert_invalid(label, &bytes);
    }

    let duplicate_key =
        SELECTED_WITH_CHECK.replacen("{", "{\"schema_version\":\"mumei.attempt-schedule/v1\",", 1);
    assert_invalid("duplicate key", duplicate_key.as_bytes());
}

#[test]
fn rejects_oversized_invalid_utf8_non_objects_and_missing_or_conflicting_inputs() {
    let oversized = vec![b'x'; 65_537];
    let output = run(
        &["evidence", "check-schedule", "--stdin", "--json"],
        Some(&oversized),
    );
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("FAKE_PRIVATE_SCHEDULE"));

    assert_invalid("invalid UTF-8", &[0xff, 0xfe]);
    assert_invalid("non-object JSON", b"[]");

    let missing = run(&["evidence", "check-schedule"], None);
    assert!(!missing.status.success());
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("FAKE_PRIVATE_SCHEDULE_INPUT.json");
    std::fs::write(&path, b"FAKE_PRIVATE_SCHEDULE_PAYLOAD").expect("write invalid input");
    let path = path.to_str().expect("UTF-8 temporary path");
    let conflict = run(
        &["evidence", "check-schedule", path, "--stdin"],
        Some(b"{}"),
    );
    assert!(!conflict.status.success());

    let invalid_file = run(&["evidence", "check-schedule", path], None);
    assert!(!invalid_file.status.success());
    let stderr = String::from_utf8_lossy(&invalid_file.stderr);
    assert!(stderr.contains("invalid attempt-schedule representation"));
    assert!(!stderr.contains(path));
    assert!(!stderr.contains("FAKE_PRIVATE_SCHEDULE"));
}
