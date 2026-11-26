use mumei_core::object_acceptance::MAX_INPUT_BYTES;
use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Output, Stdio};

const FIXTURE: &str =
    include_str!("fixtures/verification-evidence/native_object_acceptance_shape.json");

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
        let mut stdin = child.stdin.take().expect("stdin pipe");
        stdin.write_all(bytes).expect("write stdin");
    }
    child.wait_with_output().expect("wait mumei")
}

#[test]
fn check_object_acceptance_valid_text_and_json_from_file_and_stdin() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("descriptor.json");
    std::fs::write(&path, FIXTURE).unwrap();

    let text = run(
        &[
            "evidence",
            "check-object-acceptance",
            path.to_str().unwrap(),
        ],
        None,
    );
    assert!(
        text.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&text.stderr)
    );
    let stdout = String::from_utf8_lossy(&text.stdout);
    assert!(stdout.contains("Native object acceptance descriptor representation valid"));
    assert!(stdout.contains("no producer authenticity or grade promotion"));

    let diag = run(
        &["evidence", "check-object-acceptance", "--stdin", "--json"],
        Some(FIXTURE.as_bytes()),
    );
    assert!(
        diag.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&diag.stderr)
    );
    let value: Value = serde_json::from_slice(&diag.stdout).unwrap();
    assert_eq!(
        value["schema_version"],
        "mumei.native-object-acceptance-check/v1"
    );
    assert_eq!(
        value["descriptor_schema_version"],
        "mumei.native-evidence-acceptance/v1"
    );
    assert_eq!(value["representation"], "valid");
    assert_eq!(value["recheck"], "not_performed");
    assert_eq!(
        value["scope"],
        "native_bundle_object_translation_correspondence"
    );
    assert_eq!(value["checked_correspondence_items"], 10);
    assert_eq!(value["checked_functions"], 2);
}

#[test]
fn check_object_acceptance_emit_evidence_round_trips_through_evidence_check() {
    let emitted = run(
        &[
            "evidence",
            "check-object-acceptance",
            "--stdin",
            "--emit-evidence",
        ],
        Some(FIXTURE.as_bytes()),
    );
    assert!(
        emitted.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&emitted.stderr)
    );

    let evidence: Value = serde_json::from_slice(&emitted.stdout).unwrap();
    assert_eq!(evidence["schema_version"], "mumei.verification-evidence/v1");
    assert_eq!(evidence["subject"]["kind"], "native_artifact");
    assert_eq!(evidence["method"], "bounded_model_checking");
    assert_eq!(evidence["scope"]["kind"], "bounded");
    assert_eq!(evidence["semantics"]["integer_model"], "wrapping_i64");
    assert_eq!(
        evidence["provenance"]["configuration_fingerprint"],
        "0000000000000000000000000000000000000000000000000000000000000004"
    );

    let checked = run(
        &["evidence", "check", "--stdin", "--json"],
        Some(&emitted.stdout),
    );
    assert!(
        checked.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&checked.stderr)
    );
    let check_json: Value = serde_json::from_slice(&checked.stdout).unwrap();
    assert_eq!(check_json["representation"], "valid");
}

#[test]
fn check_object_acceptance_rejects_tampered_and_oversized_inputs() {
    let mut bad: Value = serde_json::from_str(FIXTURE).unwrap();
    bad["checks_performed"]
        .as_array_mut()
        .unwrap()
        .push(json!("object_translation_correspondence"));
    let bad_bytes = serde_json::to_vec(&bad).unwrap();
    let res = run(
        &[
            "evidence",
            "check-object-acceptance",
            "--stdin",
            "--emit-evidence",
        ],
        Some(&bad_bytes),
    );
    assert!(!res.status.success());
    assert!(res.stdout.is_empty());

    let mut bad_scope: Value = serde_json::from_str(FIXTURE).unwrap();
    bad_scope["source_machine_tv"]["object_link_correspondence"] =
        json!("unchecked_not_translation_validated");
    let bad_scope_bytes = serde_json::to_vec(&bad_scope).unwrap();
    let res2 = run(
        &["evidence", "check-object-acceptance", "--stdin", "--json"],
        Some(&bad_scope_bytes),
    );
    assert!(!res2.status.success());
    assert!(res2.stdout.is_empty());

    let oversized = vec![b' '; MAX_INPUT_BYTES + 1];
    let res3 = run(
        &["evidence", "check-object-acceptance", "--stdin"],
        Some(&oversized),
    );
    assert!(!res3.status.success());
    assert!(res3.stdout.is_empty());

    // A valid descriptor under the 256 KiB descriptor cap whose projected
    // Common Evidence v1 record exceeds 64 KiB must fail closed on --emit-evidence.
    let mut huge_fields: Value = serde_json::from_str(FIXTURE).unwrap();
    huge_fields["register_convention_assumed"] = json!("r".repeat(40_000));
    let huge_bytes = serde_json::to_vec(&huge_fields).unwrap();
    assert!(huge_bytes.len() <= MAX_INPUT_BYTES);
    let res4 = run(
        &[
            "evidence",
            "check-object-acceptance",
            "--stdin",
            "--emit-evidence",
        ],
        Some(&huge_bytes),
    );
    assert!(!res4.status.success());
    assert!(res4.stdout.is_empty());
}
