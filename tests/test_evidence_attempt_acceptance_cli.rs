use mumei_core::evidence_check_acceptance::MAX_INPUT_BYTES;
use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Output, Stdio};

const FIXTURES: &[(&str, &str, &str, &str)] = &[
    (
        "evidence_check_acceptance_valid.json",
        include_str!("fixtures/evidence_check_acceptance_valid.json"),
        "valid",
        "absent",
    ),
    (
        "evidence_check_acceptance_failed.json",
        include_str!("fixtures/evidence_check_acceptance_failed.json"),
        "failed",
        "absent",
    ),
    (
        "evidence_check_acceptance_mismatch.json",
        include_str!("fixtures/evidence_check_acceptance_mismatch.json"),
        "valid",
        "absent",
    ),
    (
        "evidence_check_acceptance_requirement.json",
        include_str!("fixtures/evidence_check_acceptance_requirement.json"),
        "accepted",
        "declared",
    ),
];

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
        &["evidence", "check-attempt-acceptance", "--stdin", "--json"],
        Some(bytes),
    )
}

fn json_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("encode acceptance")
}

fn base_acceptance() -> Value {
    serde_json::from_str(FIXTURES[0].1).expect("agent acceptance fixture")
}

fn assert_invalid(label: &str, bytes: &[u8]) {
    let output = stdin_check(bytes);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{label} was accepted");
    assert!(
        stderr.contains("invalid evidence-check acceptance representation"),
        "{label} did not use the static representation error: {stderr}"
    );
    assert!(
        !stderr.contains("FAKE_PRIVATE_ACCEPTANCE"),
        "{label} echoed input: {stderr}"
    );
}

#[test]
fn accepts_agent_fixtures_with_identical_file_and_stdin_diagnostics() {
    let directory = tempfile::tempdir().expect("temporary fixture directory");
    for (file_name, fixture, outcome, requirement_fingerprint) in FIXTURES {
        let path = directory.path().join(file_name);
        std::fs::write(&path, fixture).expect("write agent acceptance fixture");
        let path = path.to_str().expect("UTF-8 temporary path");
        let file_output = run(
            &["evidence", "check-attempt-acceptance", path, "--json"],
            None,
        );
        let stdin_output = stdin_check(fixture.as_bytes());
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
                "schema_version": "mumei.evidence-check-acceptance-check/v1",
                "acceptance_schema_version": "mumei.evidence-check-acceptance/v1",
                "representation": "valid",
                "acceptance": "not_evaluated",
                "declared_outcome": outcome,
                "requirement_fingerprint": requirement_fingerprint,
                "declared_field_meaning": {
                    "outcome": "declared_core_check_outcome",
                    "candidate_match": "producer_computed_not_rechecked",
                    "accepted": "producer_representation_binding_not_core_acceptance",
                },
                "checks_performed": ["representation_gate", "acceptance_consistency"],
                "checks_not_performed": [
                    "candidate_match_recheck",
                    "core_check_rerun",
                    "evidence_acceptance",
                    "claim_truth",
                    "producer_authenticity",
                    "freshness",
                    "grade_promotion",
                ],
            })
        );
    }
}

#[test]
fn text_diagnostic_is_one_static_line() {
    let output = run(
        &["evidence", "check-attempt-acceptance", "--stdin"],
        Some(FIXTURES[0].1.as_bytes()),
    );
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).expect("UTF-8 text"),
        "Evidence-check acceptance v1 representation valid; acceptance not evaluated.\n"
    );
}

#[test]
fn input_fingerprints_are_never_echoed() {
    let sentinel = "ab".repeat(32);
    let mut acceptance = base_acceptance();
    acceptance["request_fingerprint"] = json!(sentinel);
    let output = stdin_check(&json_bytes(&acceptance));
    assert!(output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains(&sentinel));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(&sentinel));
}

#[test]
fn rejects_malformed_acceptance_fields_and_consistency() {
    let mut cases = Vec::new();

    let mut value = base_acceptance();
    value["schema_version"] = json!("mumei.other/v1");
    cases.push(("wrong schema", value));

    let mut value = base_acceptance();
    value["extra"] = json!("not permitted");
    cases.push(("extra key", value));

    let mut value = base_acceptance();
    value
        .as_object_mut()
        .expect("acceptance object")
        .remove("accepted");
    cases.push(("missing key", value));

    let mut value = base_acceptance();
    value["request_fingerprint"] = json!("A".repeat(64));
    cases.push(("uppercase fingerprint", value));

    let mut value = base_acceptance();
    value["output_fingerprint"] = json!("a".repeat(63));
    cases.push(("short fingerprint", value));

    let mut value = base_acceptance();
    value["attempt_number"] = json!(0);
    cases.push(("zero attempt number", value));

    let mut value = base_acceptance();
    value["attempt_number"] = json!(true);
    cases.push(("boolean attempt number", value));

    let mut value = base_acceptance();
    value["attempt_number"] = json!(1u64 << 63);
    cases.push(("attempt number above MAX_COUNTER", value));

    let mut value = base_acceptance();
    value["attempt_number"] = json!(1.0);
    cases.push(("floating-point attempt number", value));

    let mut value = base_acceptance();
    value["outcome"] = json!("unknown");
    cases.push(("unknown outcome", value));

    let mut value = base_acceptance();
    value["candidate_match"] = json!(false);
    cases.push(("accepted without candidate match", value));

    let mut value = base_acceptance();
    value["outcome"] = json!("failed");
    cases.push(("failed outcome marked accepted", value));

    let mut value = base_acceptance();
    value["accepted"] = json!(false);
    cases.push(("current valid check not accepted", value));

    let mut value = base_acceptance();
    value["candidate_match"] = json!("true");
    cases.push(("non-boolean candidate match", value));

    let mut value = base_acceptance();
    value["accepted"] = json!(1);
    cases.push(("non-boolean accepted field", value));

    let mut value = base_acceptance();
    value["checks_not_performed"] = json!([
        "file_identity",
        "claim_truth",
        "freshness",
        "producer_authenticity",
        "semantic_correspondence",
        "artifact_semantics",
    ]);
    cases.push(("reordered checks_not_performed", value));

    let mut value = base_acceptance();
    value["checks_not_performed"] = json!([
        "claim_truth",
        "file_identity",
        "freshness",
        "producer_authenticity",
        "semantic_correspondence",
        "artifact_semantics",
        "extra",
    ]);
    cases.push(("wrong checks_not_performed", value));

    let mut value = base_acceptance();
    value["requirement_fingerprint"] = json!("A".repeat(64));
    cases.push(("uppercase requirement fingerprint", value));

    let mut value = base_acceptance();
    value["requirement_fingerprint"] = json!("a".repeat(63));
    cases.push(("short requirement fingerprint", value));

    let mut value = base_acceptance();
    value["requirement_fingerprint"] = Value::Null;
    cases.push(("null requirement fingerprint", value));

    for (label, value) in cases {
        assert_invalid(label, &json_bytes(&value));
    }
}

#[test]
fn rejects_duplicate_keys_invalid_bytes_non_objects_and_oversized_input() {
    assert_invalid(
        "duplicate key",
        br#"{"schema_version":"mumei.evidence-check-acceptance/v1","schema_version":"mumei.evidence-check-acceptance/v1"}"#,
    );
    assert_invalid("non-object", b"[]");
    assert_invalid("invalid UTF-8", &[0xff, 0xfe]);

    let oversized = vec![b' '; MAX_INPUT_BYTES + 1];
    let output = stdin_check(&oversized);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(stderr
        .contains("Evidence-check acceptance representation check failed: component too large"));
    assert!(!stderr.contains("FAKE_PRIVATE_ACCEPTANCE"));
}

#[test]
fn rejects_missing_or_conflicting_inputs() {
    let missing = run(&["evidence", "check-attempt-acceptance"], None);
    assert!(!missing.status.success());

    let conflicting = run(
        &[
            "evidence",
            "check-attempt-acceptance",
            "record.json",
            "--stdin",
        ],
        Some(FIXTURES[0].1.as_bytes()),
    );
    assert!(!conflicting.status.success());
}
