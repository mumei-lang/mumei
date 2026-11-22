use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const CLAMP: &str = include_str!("fixtures/semantic_contract_clamp.mm");
const REFERENCE_TIME: &str = "2024-02-02T00:00:00Z";
const PRODUCER_KEY_ID: &str = "producer-a";

struct Fixture {
    _dir: tempfile::TempDir,
    record: PathBuf,
    requirement: PathBuf,
    requirement_list: PathBuf,
    producer_secret: PathBuf,
    producer_public: String,
    root_secret: PathBuf,
    root_public: String,
    signature: PathBuf,
    registry: PathBuf,
    registry_signature: PathBuf,
    revocation_list: PathBuf,
    revocation_signature: PathBuf,
}

fn keygen(dir: &Path, name: &str) -> (PathBuf, String) {
    let secret_file = dir.join(format!("{name}.secret"));
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["evidence", "keygen", "--secret-file"])
        .arg(&secret_file)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let public_key = stdout
        .lines()
        .find_map(|line| line.strip_prefix("Public key: "))
        .unwrap()
        .to_owned();
    assert_eq!(public_key.len(), 64);
    assert!(public_key
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    (secret_file, public_key)
}

fn sign_file(record: &Path, secret_file: &Path, key_id: &str, produced_at: Option<&str>) -> Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command
        .args(["evidence", "sign"])
        .arg(record)
        .args(["--key-file"])
        .arg(secret_file)
        .args(["--producer-key-id", key_id]);
    if let Some(produced_at) = produced_at {
        command.args(["--produced-at", produced_at]);
    }
    let output = command.args(["--json"]).output().unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).unwrap()
}

fn fixture(produced_at: Option<&str>) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("FAKE_PRIVATE_SOURCE.mm");
    let record = dir.path().join("FAKE_PRIVATE_RECORD.json");
    let requirement = dir.path().join("FAKE_PRIVATE_REQUIREMENT.json");
    let requirement_list = dir.path().join("FAKE_PRIVATE_REQUIREMENT_LIST.json");
    fs::write(&source, CLAMP).unwrap();

    let source_evidence = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["evidence", "verify-source"])
        .arg(&source)
        .arg("--json")
        .output()
        .unwrap();
    assert!(source_evidence.status.success());
    assert!(source_evidence.stderr.is_empty());
    fs::write(&record, &source_evidence.stdout).unwrap();
    let saved: Value = serde_json::from_slice(&source_evidence.stdout).unwrap();
    let requirement_doc = json!({
        "schema_version": "mumei.evidence-requirement/v1",
        "subject_sha256": saved["subject"]["sha256"],
        "obligation_id": saved["obligation_id"],
        "semantics": {
            "profile": saved["semantics"]["profile"],
            "integer_model": saved["semantics"]["integer_model"],
            "target_triple": saved["semantics"]["target_triple"],
        },
        "minimum_assurance": "unbounded_proof",
        "correspondence_status": ["validated", "assumed"],
        "excluded_assumptions": [],
    });
    fs::write(
        &requirement,
        serde_json::to_vec_pretty(&requirement_doc).unwrap(),
    )
    .unwrap();
    fs::write(
        &requirement_list,
        serde_json::to_vec_pretty(&json!({
            "schema_version": "mumei.evidence-requirement-list/v1",
            "requirements": [requirement_doc],
        }))
        .unwrap(),
    )
    .unwrap();

    let (producer_secret, producer_public) = keygen(dir.path(), "producer");
    let (root_secret, root_public) = keygen(dir.path(), "root");
    let signature = dir.path().join("FAKE_PRIVATE_SIGNATURE.json");
    fs::write(
        &signature,
        serde_json::to_vec_pretty(&sign_file(
            &record,
            &producer_secret,
            PRODUCER_KEY_ID,
            produced_at,
        ))
        .unwrap(),
    )
    .unwrap();

    let registry = dir.path().join("FAKE_PRIVATE_REGISTRY.json");
    fs::write(
        &registry,
        serde_json::to_vec_pretty(&json!({
            "schema_version": "mumei.producer-key-registry/v1",
            "registry": {
                (PRODUCER_KEY_ID): {
                    "principal": "fixture-principal",
                    "not_before": "2024-01-01T00:00:00Z",
                    "not_after": "2024-12-31T23:59:59Z",
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let registry_signature = dir.path().join("FAKE_PRIVATE_REGISTRY_SIGNATURE.json");
    fs::write(
        &registry_signature,
        serde_json::to_vec_pretty(&sign_file(&registry, &root_secret, "root-key", None)).unwrap(),
    )
    .unwrap();

    let revocation_list = dir.path().join("FAKE_PRIVATE_REVOCATION.json");
    fs::write(
        &revocation_list,
        serde_json::to_vec_pretty(&json!({
            "schema_version": "mumei.key-revocation-list/v1",
            "revoked_keys": [],
        }))
        .unwrap(),
    )
    .unwrap();
    let revocation_signature = dir.path().join("FAKE_PRIVATE_REVOCATION_SIGNATURE.json");
    fs::write(
        &revocation_signature,
        serde_json::to_vec_pretty(&sign_file(&revocation_list, &root_secret, "root-key", None))
            .unwrap(),
    )
    .unwrap();

    Fixture {
        _dir: dir,
        record,
        requirement,
        requirement_list,
        producer_secret,
        producer_public,
        root_secret,
        root_public,
        signature,
        registry,
        registry_signature,
        revocation_list,
        revocation_signature,
    }
}

fn accept_command(fixture: &Fixture, json_output: bool) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command
        .args(["evidence", "accept"])
        .arg(&fixture.record)
        .args(["--requirement"])
        .arg(&fixture.requirement);
    if json_output {
        command.arg("--json");
    }
    command
}

fn signature_command(fixture: &Fixture, json_output: bool) -> Command {
    signature_command_with_key(fixture, &fixture.producer_public, json_output)
}

fn signature_command_with_key(
    fixture: &Fixture,
    producer_public: &str,
    json_output: bool,
) -> Command {
    let mut command = accept_command(fixture, json_output);
    command
        .args(["--signature"])
        .arg(&fixture.signature)
        .args(["--producer-key", producer_public]);
    command
}

fn full_chain_command(fixture: &Fixture, json_output: bool) -> Command {
    let mut command = signature_command(fixture, json_output);
    command
        .args(["--expected-producer-key", PRODUCER_KEY_ID])
        .args(["--reference-time", REFERENCE_TIME])
        .args(["--max-age-seconds", "172800"])
        .args(["--key-registry"])
        .arg(&fixture.registry)
        .args(["--registry-signature"])
        .arg(&fixture.registry_signature)
        .args(["--registry-key", &fixture.root_public])
        .args(["--revocation-list"])
        .arg(&fixture.revocation_list)
        .args(["--revocation-signature"])
        .arg(&fixture.revocation_signature)
        .args(["--revocation-key", &fixture.root_public]);
    command
}

fn run_command(mut command: Command) -> Output {
    command.output().unwrap()
}

fn write_v2_revocation_list(
    fixture: &Fixture,
    version: u64,
    issued_at: &str,
    not_after: Option<&str>,
) {
    let mut document = json!({
        "schema_version": "mumei.key-revocation-list/v2",
        "list_version": version,
        "issued_at": issued_at,
        "revoked_keys": [],
    });
    if let Some(not_after) = not_after {
        document["not_after"] = json!(not_after);
    }
    fs::write(
        &fixture.revocation_list,
        serde_json::to_vec_pretty(&document).unwrap(),
    )
    .unwrap();
    resign_revocation_list(fixture);
}

fn accepted_value(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

fn rewrite_json(path: &Path, edit: impl FnOnce(&mut Value)) {
    let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    edit(&mut value);
    fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
}

fn resign_registry(fixture: &Fixture) {
    fs::write(
        &fixture.registry_signature,
        serde_json::to_vec_pretty(&sign_file(
            &fixture.registry,
            &fixture.root_secret,
            "root-key",
            None,
        ))
        .unwrap(),
    )
    .unwrap();
}

fn resign_revocation_list(fixture: &Fixture) {
    fs::write(
        &fixture.revocation_signature,
        serde_json::to_vec_pretty(&sign_file(
            &fixture.revocation_list,
            &fixture.root_secret,
            "root-key",
            None,
        ))
        .unwrap(),
    )
    .unwrap();
}

fn assert_failure_without_descriptor(output: &Output) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(!error.is_empty());
    assert!(!error.contains("FAKE_PRIVATE"));
    assert!(!error.contains('/'));
}

#[test]
fn no_signature_flags_preserve_acceptance_descriptor_and_text() {
    let fixture = fixture(Some("2024-02-01T00:00:00Z"));
    let output = run_command(accept_command(&fixture, true));
    assert!(output.status.success());
    let descriptor = accepted_value(&output);
    assert_eq!(descriptor["verdict"], "accepted");
    assert!(descriptor.get("producer_signature").is_none());
    assert_eq!(
        descriptor["checks_performed"],
        json!(["representation_gate", "declared_requirement_normalization"])
    );
    assert_eq!(
        descriptor["checks_not_performed"],
        json!([
            "claim_truth",
            "producer_authenticity",
            "historical_execution",
            "record_freshness",
            "independent_model_correspondence",
            "artifact_semantics",
            "tool_execution",
            "grade_promotion"
        ])
    );

    let output = run_command(accept_command(&fixture, false));
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Evidence acceptance: accepted (0 check(s) failed, 0 indeterminate).\n\
Scope: declared-requirement normalization over the record's representation only; no claim truth, producer authenticity, freshness, independent correspondence, artifact semantics, tool execution or grade promotion checked.\n"
    );
}

#[test]
fn valid_full_chain_binds_signature_freshness_registry_and_revocation() {
    let fixture = fixture(Some("2024-02-01T00:00:00Z"));
    let output = run_command(full_chain_command(&fixture, true));
    assert!(output.status.success());
    let descriptor = accepted_value(&output);
    assert_eq!(descriptor["verdict"], "accepted");
    assert!(descriptor["checks_performed"]
        .as_array()
        .unwrap()
        .contains(&json!("producer_signature_binding")));
    for not_performed in descriptor["checks_not_performed"].as_array().unwrap() {
        assert_ne!(not_performed, "producer_authenticity");
        assert_ne!(not_performed, "record_freshness");
    }
    let binding = &descriptor["producer_signature"];
    for name in [
        "envelope_match",
        "signature_valid",
        "subject_bound",
        "key_pinned",
        "key_not_revoked",
        "key_authentic",
        "envelope_fresh",
        "registry_authentic",
        "revocation_list_authentic",
    ] {
        assert_eq!(binding[name], true, "{name}");
    }
    assert_eq!(
        binding["registry_digest"],
        format!("{:x}", Sha256::digest(fs::read(&fixture.registry).unwrap()))
    );
    assert_eq!(
        binding["revocation_digest"],
        format!(
            "{:x}",
            Sha256::digest(fs::read(&fixture.revocation_list).unwrap())
        )
    );
    assert_eq!(
        binding["checks_not_performed"],
        json!([
            "revocation_list_rollback",
            "producer_time_authenticity",
            "clock_authenticity"
        ])
    );
}

#[test]
fn signature_only_checks_subject_binding_but_leaves_authenticity_and_freshness_open() {
    let fixture = fixture(Some("2024-02-01T00:00:00Z"));
    let output = run_command(signature_command(&fixture, true));
    assert!(output.status.success());
    let descriptor = accepted_value(&output);
    assert_eq!(descriptor["verdict"], "accepted");
    let binding = &descriptor["producer_signature"];
    assert_eq!(binding["envelope_match"], true);
    assert_eq!(binding["signature_valid"], true);
    assert_eq!(binding["subject_bound"], true);
    assert_eq!(
        binding["checks_not_performed"],
        json!([
            "key_binding",
            "key_authenticity",
            "envelope_freshness",
            "key_revocation",
            "registry_authenticity",
            "revocation_list_authenticity",
            "revocation_list_rollback",
            "producer_time_authenticity",
            "clock_authenticity"
        ])
    );
    for omitted in [
        "key_pinned",
        "key_not_revoked",
        "key_authentic",
        "envelope_fresh",
        "registry_authentic",
        "revocation_list_authentic",
        "registry_digest",
        "revocation_digest",
    ] {
        assert!(binding.get(omitted).is_none(), "{omitted}");
    }
    for not_performed in ["producer_authenticity", "record_freshness"] {
        assert!(descriptor["checks_not_performed"]
            .as_array()
            .unwrap()
            .contains(&json!(not_performed)));
    }
}

#[test]
fn text_output_reports_bound_or_failed_signature_checks() {
    let fixture = fixture(Some("2024-02-01T00:00:00Z"));
    let output = run_command(signature_command(&fixture, false));
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Producer signature: bound\n"));
    assert!(
        text.contains("authenticity is checked only with the full key-registry/revocation chain")
    );
    assert!(text.contains("record freshness only with --max-age-seconds"));
    assert!(!text.contains(fixture.producer_public.as_str()));

    rewrite_json(&fixture.signature, |envelope| {
        envelope["unexpected"] = json!(true);
    });
    let output = run_command(signature_command(&fixture, false));
    assert_eq!(output.status.code(), Some(1));
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text
        .contains("Producer signature: failed (envelope_match, signature_valid, subject_bound)\n"));
}

#[test]
fn changed_record_is_rejected_by_subject_binding() {
    let fixture = fixture(Some("2024-02-01T00:00:00Z"));
    let mut bytes = fs::read(&fixture.record).unwrap();
    bytes.push(b'\n');
    fs::write(&fixture.record, bytes).unwrap();

    let output = run_command(full_chain_command(&fixture, true));
    assert_eq!(output.status.code(), Some(1));
    let descriptor = accepted_value(&output);
    assert_eq!(descriptor["verdict"], "rejected");
    assert_eq!(descriptor["producer_signature"]["signature_valid"], true);
    assert_eq!(descriptor["producer_signature"]["subject_bound"], false);
}

#[test]
fn wrong_producer_key_rejects_valid_envelope() {
    let fixture = fixture(Some("2024-02-01T00:00:00Z"));
    let (_wrong_secret, wrong_public) = keygen(fixture._dir.path(), "wrong-producer");
    let output = run_command(signature_command_with_key(&fixture, &wrong_public, true));
    assert_eq!(output.status.code(), Some(1));
    let descriptor = accepted_value(&output);
    assert_eq!(descriptor["verdict"], "rejected");
    assert_eq!(descriptor["producer_signature"]["signature_valid"], false);
    assert_eq!(descriptor["producer_signature"]["subject_bound"], true);
}

#[test]
fn stale_postdated_and_unstamped_envelopes_fail_closed() {
    let stale = fixture(Some("2024-01-01T00:00:00Z"));
    let mut command = signature_command(&stale, true);
    command
        .args(["--reference-time", REFERENCE_TIME])
        .args(["--max-age-seconds", "1"]);
    let output = run_command(command);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        accepted_value(&output)["producer_signature"]["envelope_fresh"],
        false
    );

    let postdated = fixture(Some("2024-03-01T00:00:00Z"));
    let mut command = signature_command(&postdated, true);
    command
        .args(["--reference-time", REFERENCE_TIME])
        .args(["--max-age-seconds", "172800"]);
    let output = run_command(command);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        accepted_value(&output)["producer_signature"]["envelope_fresh"],
        false
    );

    let unstamped = fixture(None);
    let mut command = signature_command(&unstamped, true);
    command
        .args(["--reference-time", REFERENCE_TIME])
        .args(["--max-age-seconds", "172800"]);
    let output = run_command(command);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        accepted_value(&output)["producer_signature"]["envelope_fresh"],
        false
    );
}

#[test]
fn registry_window_and_edited_after_signing_registry_fail_closed() {
    let outside_window = fixture(Some("2024-02-01T00:00:00Z"));
    rewrite_json(&outside_window.registry, |registry| {
        registry["registry"][PRODUCER_KEY_ID]["not_before"] = json!("2024-03-01T00:00:00Z");
        registry["registry"][PRODUCER_KEY_ID]["not_after"] = json!("2024-12-31T23:59:59Z");
    });
    resign_registry(&outside_window);
    let output = run_command(full_chain_command(&outside_window, true));
    assert_eq!(output.status.code(), Some(1));
    let descriptor = accepted_value(&output);
    let binding = &descriptor["producer_signature"];
    assert_eq!(binding["registry_authentic"], true);
    assert_eq!(binding["key_authentic"], false);

    let edited = fixture(Some("2024-02-01T00:00:00Z"));
    rewrite_json(&edited.registry, |registry| {
        registry["registry"][PRODUCER_KEY_ID]["principal"] = json!("edited-principal");
    });
    let output = run_command(full_chain_command(&edited, true));
    assert_eq!(output.status.code(), Some(1));
    let descriptor = accepted_value(&output);
    assert_eq!(
        descriptor["producer_signature"]["registry_authentic"],
        false
    );
    assert_eq!(descriptor["producer_signature"]["key_authentic"], false);
}

#[test]
fn revoked_key_rejects_even_when_revocation_list_signature_is_valid() {
    let fixture = fixture(Some("2024-02-01T00:00:00Z"));
    rewrite_json(&fixture.revocation_list, |revocation| {
        revocation["revoked_keys"] = json!([PRODUCER_KEY_ID]);
    });
    resign_revocation_list(&fixture);

    let output = run_command(full_chain_command(&fixture, true));
    assert_eq!(output.status.code(), Some(1));
    let descriptor = accepted_value(&output);
    assert_eq!(descriptor["producer_signature"]["key_not_revoked"], false);
    assert_eq!(
        descriptor["producer_signature"]["revocation_list_authentic"],
        true
    );
}

#[test]
fn relabeled_revoked_key_requires_and_fails_the_producer_key_id_pin() {
    let fixture = fixture(Some("2024-02-01T00:00:00Z"));
    rewrite_json(&fixture.revocation_list, |revocation| {
        revocation["revoked_keys"] = json!([PRODUCER_KEY_ID]);
    });
    resign_revocation_list(&fixture);
    rewrite_json(&fixture.signature, |envelope| {
        envelope["producer_key_id"] = json!("relabeled-key");
    });

    let mut unpinned = signature_command(&fixture, true);
    unpinned
        .args(["--revocation-list"])
        .arg(&fixture.revocation_list);
    let output = run_command(unpinned);
    assert_failure_without_descriptor(&output);
    assert!(String::from_utf8_lossy(&output.stderr).contains("--expected-producer-key"));

    let mut pinned = signature_command(&fixture, true);
    pinned
        .args(["--expected-producer-key", PRODUCER_KEY_ID])
        .args(["--revocation-list"])
        .arg(&fixture.revocation_list);
    let output = run_command(pinned);
    assert_eq!(output.status.code(), Some(1));
    let descriptor = accepted_value(&output);
    assert_eq!(descriptor["verdict"], "rejected");
    assert_eq!(descriptor["producer_signature"]["signature_valid"], true);
    assert_eq!(descriptor["producer_signature"]["key_pinned"], false);
}

#[test]
fn nonmatching_json_object_envelope_is_a_failed_binding_not_an_input_error() {
    let fixture = fixture(Some("2024-02-01T00:00:00Z"));
    write_v2_revocation_list(
        &fixture,
        1,
        "2024-02-01T00:00:00Z",
        Some("2024-03-01T00:00:00Z"),
    );
    rewrite_json(&fixture.signature, |envelope| {
        envelope["unexpected"] = json!(true);
    });
    let mut command = full_chain_command(&fixture, true);
    command.args(["--min-revocation-version", "1"]);
    let output = run_command(command);
    assert_eq!(output.status.code(), Some(1));
    let descriptor = accepted_value(&output);
    assert_eq!(descriptor["verdict"], "rejected");
    let binding = &descriptor["producer_signature"];
    assert_eq!(binding["envelope_match"], false);
    assert_eq!(binding["revocation_list_current"], true);
    assert_eq!(binding["signature_valid"], false);
    assert!(binding.get("subject_digest").is_none());
    assert_eq!(binding["subject_bound"], false);
    for name in [
        "key_pinned",
        "key_not_revoked",
        "key_authentic",
        "envelope_fresh",
        "registry_authentic",
    ] {
        assert_eq!(binding[name], false, "{name}");
    }
    assert_eq!(binding["revocation_list_authentic"], true);
}

#[test]
fn malformed_envelope_json_bad_keys_and_bad_reference_time_fail_without_descriptor() {
    let malformed = fixture(Some("2024-02-01T00:00:00Z"));
    fs::write(&malformed.signature, b"[]").unwrap();
    assert_failure_without_descriptor(&run_command(signature_command(&malformed, true)));

    let invalid_key = fixture(Some("2024-02-01T00:00:00Z"));
    assert_failure_without_descriptor(&run_command(signature_command_with_key(
        &invalid_key,
        &"zz".repeat(32),
        true,
    )));

    let invalid_reference = fixture(Some("2024-02-01T00:00:00Z"));
    let mut command = signature_command(&invalid_reference, true);
    command
        .args(["--reference-time", "not-a-time"])
        .args(["--max-age-seconds", "10"]);
    assert_failure_without_descriptor(&run_command(command));
}

#[test]
fn dependency_violations_fail_without_a_descriptor() {
    let fixture = fixture(Some("2024-02-01T00:00:00Z"));

    let mut producer_key_without_signature = accept_command(&fixture, true);
    producer_key_without_signature.args(["--producer-key", &fixture.producer_public]);
    assert_failure_without_descriptor(&run_command(producer_key_without_signature));

    let mut max_age_without_reference = signature_command(&fixture, true);
    max_age_without_reference.args(["--max-age-seconds", "10"]);
    assert_failure_without_descriptor(&run_command(max_age_without_reference));
    let mut signature_without_producer_key = accept_command(&fixture, true);
    signature_without_producer_key
        .args(["--signature"])
        .arg(&fixture.signature);
    assert_failure_without_descriptor(&run_command(signature_without_producer_key));

    let mut registry_signature_without_registry = signature_command(&fixture, true);
    registry_signature_without_registry
        .args(["--registry-signature"])
        .arg(&fixture.registry_signature)
        .args(["--registry-key", &fixture.root_public]);
    assert_failure_without_descriptor(&run_command(registry_signature_without_registry));

    let mut reference_time_alone = signature_command(&fixture, true);
    reference_time_alone.args(["--reference-time", REFERENCE_TIME]);
    assert_failure_without_descriptor(&run_command(reference_time_alone));

    let mut reference_time_without_signature = accept_command(&fixture, true);
    reference_time_without_signature.args(["--reference-time", REFERENCE_TIME]);
    assert_failure_without_descriptor(&run_command(reference_time_without_signature));

    let mut registry_without_key_pin = signature_command(&fixture, true);
    registry_without_key_pin
        .args(["--reference-time", REFERENCE_TIME])
        .args(["--key-registry"])
        .arg(&fixture.registry);
    assert_failure_without_descriptor(&run_command(registry_without_key_pin));
}

#[test]
fn duplicate_registry_keys_are_malformed_anchors() {
    let fixture = fixture(Some("2024-02-01T00:00:00Z"));
    fs::write(
        &fixture.registry,
        br#"{"schema_version":"mumei.producer-key-registry/v1","registry":{"producer-a":{"principal":"one","principal":"two","not_before":"2024-01-01T00:00:00Z","not_after":"2024-12-31T23:59:59Z"}}}"#,
    )
    .unwrap();
    let mut command = signature_command(&fixture, true);
    command
        .args(["--expected-producer-key", PRODUCER_KEY_ID])
        .args(["--reference-time", REFERENCE_TIME])
        .args(["--key-registry"])
        .arg(&fixture.registry);
    let output = run_command(command);
    assert_failure_without_descriptor(&output);
}

#[test]
fn valid_signature_chain_never_promotes_rejected_or_inconclusive_requirements() {
    let rejected = fixture(Some("2024-02-01T00:00:00Z"));
    rewrite_json(&rejected.requirement, |requirement| {
        requirement["obligation_id"] = json!("different-obligation");
    });
    let output = run_command(full_chain_command(&rejected, true));
    assert_eq!(output.status.code(), Some(1));
    let descriptor = accepted_value(&output);
    assert_eq!(descriptor["verdict"], "rejected");
    assert_eq!(descriptor["producer_signature"]["subject_bound"], true);
    assert_eq!(descriptor["producer_signature"]["signature_valid"], true);

    let inconclusive = fixture(Some("2024-02-01T00:00:00Z"));
    rewrite_json(&inconclusive.record, |record| {
        record["execution"] = json!("timeout");
        record["result"] = json!("unknown");
        record["correspondence"]["relation"] = json!("unchecked");
        record["correspondence"]["status"] = json!("unverified");
    });
    fs::write(
        &inconclusive.signature,
        serde_json::to_vec_pretty(&sign_file(
            &inconclusive.record,
            &inconclusive.producer_secret,
            PRODUCER_KEY_ID,
            Some("2024-02-01T00:00:00Z"),
        ))
        .unwrap(),
    )
    .unwrap();
    let output = run_command(full_chain_command(&inconclusive, true));
    assert_eq!(output.status.code(), Some(3));
    let descriptor = accepted_value(&output);
    assert_eq!(descriptor["verdict"], "inconclusive");
    assert_eq!(descriptor["producer_signature"]["subject_bound"], true);
    assert_eq!(descriptor["producer_signature"]["signature_valid"], true);
}

#[test]
fn requirement_list_accept_path_attaches_signature_binding() {
    let fixture = fixture(Some("2024-02-01T00:00:00Z"));
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command
        .args(["evidence", "accept"])
        .arg(&fixture.record)
        .args(["--requirement-list"])
        .arg(&fixture.requirement_list);
    command.arg("--json");
    command
        .args(["--signature"])
        .arg(&fixture.signature)
        .args(["--producer-key", &fixture.producer_public])
        .args(["--expected-producer-key", PRODUCER_KEY_ID])
        .args(["--reference-time", REFERENCE_TIME])
        .args(["--max-age-seconds", "172800"])
        .args(["--key-registry"])
        .arg(&fixture.registry)
        .args(["--registry-signature"])
        .arg(&fixture.registry_signature)
        .args(["--registry-key", &fixture.root_public])
        .args(["--revocation-list"])
        .arg(&fixture.revocation_list)
        .args(["--revocation-signature"])
        .arg(&fixture.revocation_signature)
        .args(["--revocation-key", &fixture.root_public]);
    let output = run_command(command);
    assert!(output.status.success());
    let descriptor = accepted_value(&output);
    assert_eq!(
        descriptor["schema_version"],
        "mumei.evidence-requirement-list-acceptance/v1"
    );
    assert_eq!(descriptor["verdict"], "accepted");
    assert_eq!(descriptor["producer_signature"]["envelope_match"], true);
    assert_eq!(descriptor["producer_signature"]["signature_valid"], true);
    assert_eq!(descriptor["producer_signature"]["key_authentic"], true);
    assert_eq!(descriptor["producer_signature"]["key_not_revoked"], true);
}

#[test]
fn v2_revocation_list_version_window_and_rollback_checks() {
    let current = fixture(Some("2024-02-01T00:00:00Z"));
    write_v2_revocation_list(
        &current,
        3,
        "2024-02-01T00:00:00Z",
        Some("2024-03-01T00:00:00Z"),
    );
    let mut command = full_chain_command(&current, true);
    command.args(["--min-revocation-version", "3"]);
    let output = run_command(command);
    assert!(output.status.success());
    let binding = &accepted_value(&output)["producer_signature"];
    assert_eq!(binding["revocation_list_current"], true);
    assert!(!binding["checks_not_performed"]
        .as_array()
        .unwrap()
        .contains(&json!("revocation_list_rollback")));

    for (version, issued_at, not_after) in [
        (2, "2024-02-01T00:00:00Z", Some("2024-03-01T00:00:00Z")),
        (3, "2024-02-03T00:00:00Z", Some("2024-03-01T00:00:00Z")),
        (3, "2024-01-01T00:00:00Z", Some("2024-02-01T00:00:00Z")),
    ] {
        let replay = fixture(Some("2024-02-01T00:00:00Z"));
        write_v2_revocation_list(&replay, version, issued_at, not_after);
        let mut command = full_chain_command(&replay, true);
        command.args(["--min-revocation-version", "3"]);
        let output = run_command(command);
        assert_eq!(output.status.code(), Some(1));
        let descriptor = accepted_value(&output);
        assert_eq!(descriptor["verdict"], "rejected");
        assert_eq!(
            descriptor["producer_signature"]["revocation_list_current"],
            false
        );
        assert_eq!(descriptor["producer_signature"]["key_not_revoked"], true);
    }

    let invalid_root_signature = fixture(Some("2024-02-01T00:00:00Z"));
    write_v2_revocation_list(
        &invalid_root_signature,
        3,
        "2024-02-01T00:00:00Z",
        Some("2024-03-01T00:00:00Z"),
    );
    rewrite_json(&invalid_root_signature.revocation_signature, |signature| {
        signature["signature"] = json!("0".repeat(128));
    });
    let mut command = full_chain_command(&invalid_root_signature, true);
    command.args(["--min-revocation-version", "3"]);
    let output = run_command(command);
    assert_eq!(output.status.code(), Some(1));
    let descriptor = accepted_value(&output);
    assert_eq!(descriptor["verdict"], "rejected");
    assert_eq!(
        descriptor["producer_signature"]["revocation_list_authentic"],
        false
    );
    assert_eq!(
        descriptor["producer_signature"]["revocation_list_current"],
        false
    );

    let no_minimum = fixture(Some("2024-02-01T00:00:00Z"));
    write_v2_revocation_list(
        &no_minimum,
        2,
        "2024-02-01T00:00:00Z",
        Some("2024-03-01T00:00:00Z"),
    );
    let output = run_command(full_chain_command(&no_minimum, true));
    assert!(output.status.success());
    let binding = &accepted_value(&output)["producer_signature"];
    assert!(binding.get("revocation_list_current").is_none());
    assert!(binding["checks_not_performed"]
        .as_array()
        .unwrap()
        .contains(&json!("revocation_list_rollback")));
}

#[test]
fn min_revocation_version_dependencies_and_v1_rejection() {
    let fixture = fixture(Some("2024-02-01T00:00:00Z"));
    let mut v1 = full_chain_command(&fixture, true);
    v1.args(["--min-revocation-version", "1"]);
    let output = run_command(v1);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("--min-revocation-version requires a mumei.key-revocation-list/v2 document"));

    let mut no_list = signature_command(&fixture, true);
    no_list.args(["--min-revocation-version", "1"]);
    assert_failure_without_descriptor(&run_command(no_list));
    let mut no_reference = signature_command(&fixture, true);
    no_reference
        .args(["--expected-producer-key", PRODUCER_KEY_ID])
        .args(["--revocation-list"])
        .arg(&fixture.revocation_list)
        .args(["--min-revocation-version", "1"]);
    assert_failure_without_descriptor(&run_command(no_reference));

    let mut unsigned_list = signature_command(&fixture, true);
    unsigned_list
        .args(["--expected-producer-key", PRODUCER_KEY_ID])
        .args(["--reference-time", REFERENCE_TIME])
        .args(["--revocation-list"])
        .arg(&fixture.revocation_list)
        .args(["--min-revocation-version", "1"]);
    assert_failure_without_descriptor(&run_command(unsigned_list));

    let mut zero = full_chain_command(&fixture, true);
    zero.args(["--min-revocation-version", "0"]);
    assert_failure_without_descriptor(&run_command(zero));
}
