use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const CLAMP: &str = include_str!("fixtures/semantic_contract_clamp.mm");
const PRODUCER_KEY_ID: &str = "producer-a";
const RECORD_TIME: &str = "2024-02-01T00:00:00Z";
const LIST_TIME: &str = "2024-02-01T12:00:00Z";
const REFERENCE_TIME: &str = "2024-02-02T00:00:00Z";

struct Fixture {
    _dir: tempfile::TempDir,
    source: PathBuf,
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
    let public_key = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("Public key: "))
        .unwrap()
        .to_owned();
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

fn write_signature(fixture: &Fixture, subject: &Path, produced_at: Option<&str>) {
    fs::write(
        &fixture.signature,
        serde_json::to_vec_pretty(&sign_file(
            subject,
            &fixture.producer_secret,
            PRODUCER_KEY_ID,
            produced_at,
        ))
        .unwrap(),
    )
    .unwrap();
}

fn fixture(record_time: Option<&str>) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("FAKE_PRIVATE_SOURCE.mm");
    let record = dir.path().join("FAKE_PRIVATE_RECORD.json");
    let requirement = dir.path().join("FAKE_PRIVATE_REQUIREMENT.json");
    let requirement_list = dir.path().join("FAKE_PRIVATE_REQUIREMENT_LIST.json");
    fs::write(&source, CLAMP).unwrap();

    let mut verify = Command::new(env!("CARGO_BIN_EXE_mumei"));
    verify
        .args(["evidence", "verify-source"])
        .arg(&source)
        .args(["--json"]);
    if let Some(record_time) = record_time {
        verify.args(["--produced-at", record_time]);
    }
    let source_evidence = verify.output().unwrap();
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

    let fixture = Fixture {
        _dir: dir,
        source,
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
    };
    write_signature(&fixture, &fixture.record, record_time);
    fixture
}

fn list_file(fixture: &Fixture) -> PathBuf {
    let list = fixture._dir.path().join("FAKE_PRIVATE_LIST.json");
    let record: Value = serde_json::from_slice(&fs::read(&fixture.record).unwrap()).unwrap();
    fs::write(
        &list,
        serde_json::to_vec_pretty(&json!({
            "schema_version": "mumei.verification-evidence-list/v1",
            "evidence": [record],
        }))
        .unwrap(),
    )
    .unwrap();
    list
}

fn full_chain(mut command: Command, fixture: &Fixture, producer_key: &str) -> Command {
    command
        .args(["--signature"])
        .arg(&fixture.signature)
        .args(["--producer-key", producer_key])
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

fn accept_list_command(list: &Path, requirement: &Path, json_output: bool) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command
        .args(["evidence", "accept-list"])
        .arg(list)
        .args(["--requirement"])
        .arg(requirement);
    if json_output {
        command.arg("--json");
    }
    command
}

fn accept_list_requirement_list_command(
    list: &Path,
    requirement_list: &Path,
    json_output: bool,
) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command
        .args(["evidence", "accept-list"])
        .arg(list)
        .args(["--requirement-list"])
        .arg(requirement_list);
    if json_output {
        command.arg("--json");
    }
    command
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

fn accept_source_command(
    fixture: &Fixture,
    json_output: bool,
    produced_at: Option<&str>,
) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command
        .args(["evidence", "accept-source"])
        .arg(&fixture.record)
        .args(["--source"])
        .arg(&fixture.source);
    if json_output {
        command.arg("--json");
    }
    if let Some(produced_at) = produced_at {
        command.args(["--produced-at", produced_at]);
    }
    command
}

fn run(mut command: Command) -> Output {
    command.output().unwrap()
}

fn assert_usage_error(output: &Output) {
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error:"));
    assert!(!stderr.contains("FAKE_PRIVATE"));
    assert!(!stderr.contains('/'));
}

fn contains_check(checks: &Value, name: &str) -> bool {
    checks.as_array().unwrap().iter().any(|check| check == name)
}

#[test]
fn accept_list_no_flags_and_full_chain_bind_the_raw_list_not_record_stamps() {
    let fixture = fixture(Some(RECORD_TIME));
    let list = list_file(&fixture);

    let output = run(accept_list_command(&list, &fixture.requirement, true));
    assert!(output.status.success());
    let descriptor: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(descriptor.get("producer_signature").is_none());
    assert_eq!(
        descriptor["checks_performed"],
        json!([
            "representation_gate",
            "declared_requirement_normalization",
            "cross_record_pin_agreement"
        ])
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

    write_signature(&fixture, &list, Some(LIST_TIME));
    let output = run(full_chain(
        accept_list_command(&list, &fixture.requirement, true),
        &fixture,
        &fixture.producer_public,
    ));
    assert!(output.status.success());
    let descriptor: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(descriptor["verdict"], "accepted");
    assert_eq!(
        descriptor["list_fingerprint"],
        format!("{:x}", Sha256::digest(fs::read(&list).unwrap()))
    );
    assert!(contains_check(
        &descriptor["checks_performed"],
        "producer_signature_binding"
    ));
    assert!(!contains_check(
        &descriptor["checks_not_performed"],
        "record_freshness"
    ));
    assert!(!contains_check(
        &descriptor["checks_not_performed"],
        "producer_authenticity"
    ));
    let binding = &descriptor["producer_signature"];
    assert_eq!(binding["signature_valid"], true);
    assert_eq!(binding["subject_bound"], true);
    assert_eq!(binding["key_pinned"], true);
    assert_eq!(binding["envelope_fresh"], true);
    assert!(binding.get("produced_at_signed").is_none());
    assert!(contains_check(
        &binding["checks_not_performed"],
        "producer_time_authenticity"
    ));
    assert!(contains_check(
        &binding["checks_not_performed"],
        "clock_authenticity"
    ));
}

#[test]
fn accept_list_rejects_member_record_signatures_and_binds_requirement_list_route() {
    let fixture = fixture(Some(RECORD_TIME));
    let list = list_file(&fixture);
    write_signature(&fixture, &fixture.record, Some(RECORD_TIME));

    let output = run(full_chain(
        accept_list_command(&list, &fixture.requirement, true),
        &fixture,
        &fixture.producer_public,
    ));
    assert_eq!(output.status.code(), Some(1));
    let descriptor: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(descriptor["verdict"], "rejected");
    assert_eq!(descriptor["producer_signature"]["signature_valid"], true);
    assert_eq!(descriptor["producer_signature"]["subject_bound"], false);

    write_signature(&fixture, &list, Some(LIST_TIME));
    let output = run(full_chain(
        accept_list_requirement_list_command(&list, &fixture.requirement_list, true),
        &fixture,
        &fixture.producer_public,
    ));
    assert!(output.status.success());
    let descriptor: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        descriptor["schema_version"],
        "mumei.evidence-requirement-list-acceptance/v1"
    );
    assert_eq!(descriptor["verdict"], "accepted");
    assert!(descriptor.get("producer_signature").is_some());
    assert!(contains_check(
        &descriptor["checks_performed"],
        "producer_signature_binding"
    ));
}

#[test]
fn accept_list_v2_revocation_minimum_reports_current_version() {
    let fixture = fixture(Some(RECORD_TIME));
    let list = list_file(&fixture);
    let mut revocation: Value =
        serde_json::from_slice(&fs::read(&fixture.revocation_list).unwrap()).unwrap();
    revocation["schema_version"] = json!("mumei.key-revocation-list/v2");
    revocation["list_version"] = json!(4);
    revocation["issued_at"] = json!("2024-02-01T00:00:00Z");
    revocation["not_after"] = json!("2024-03-01T00:00:00Z");
    fs::write(
        &fixture.revocation_list,
        serde_json::to_vec_pretty(&revocation).unwrap(),
    )
    .unwrap();
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
    write_signature(&fixture, &list, Some(LIST_TIME));
    let mut command = full_chain(
        accept_list_command(&list, &fixture.requirement, true),
        &fixture,
        &fixture.producer_public,
    );
    command.args(["--min-revocation-version", "4"]);
    let output = run(command);
    assert!(output.status.success());
    let descriptor: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        descriptor["producer_signature"]["revocation_list_current"],
        true
    );
}

#[test]
fn accept_list_text_reports_signature_and_dependencies_are_usage_errors() {
    let fixture = fixture(None);
    let list = list_file(&fixture);
    write_signature(&fixture, &list, None);

    let mut command = accept_list_command(&list, &fixture.requirement, false);
    command
        .args(["--signature"])
        .arg(&fixture.signature)
        .args(["--producer-key", &fixture.producer_public]);
    let output = run(command);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Producer signature: bound\n"));
    assert!(!text.contains(&fixture.producer_public));

    let mut command = accept_list_command(&list, &fixture.requirement, true);
    command
        .args(["--signature"])
        .arg(&fixture.signature)
        .args(["--producer-key", &fixture.producer_public]);
    let output = run(command);
    assert!(output.status.success());
    let descriptor: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(contains_check(
        &descriptor["checks_not_performed"],
        "record_freshness"
    ));
    assert!(contains_check(
        &descriptor["checks_not_performed"],
        "producer_authenticity"
    ));

    let mut command = accept_list_command(&list, &fixture.requirement, true);
    command
        .args(["--signature"])
        .arg(&fixture.signature)
        .args(["--producer-key", &fixture.producer_public])
        .args(["--max-age-seconds", "10"]);
    assert_usage_error(&run(command));
}

#[test]
fn accept_source_no_flags_full_chain_and_signed_time_mismatch() {
    let fixture = fixture(Some(RECORD_TIME));
    let output = run(accept_source_command(&fixture, true, Some(RECORD_TIME)));
    assert!(output.status.success());
    let descriptor: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(descriptor["accepted"], true);
    assert!(descriptor.get("producer_signature").is_none());
    assert_eq!(
        descriptor["checks_performed"],
        json!(["representation_gate", "fresh_same_profile_source_query"])
    );
    assert_eq!(
        descriptor["checks_not_performed"],
        json!([
            "producer_authenticity",
            "historical_execution",
            "record_freshness",
            "parser_encoder_solver_correctness",
            "independent_model_correspondence",
            "llvm_correspondence",
            "artifact_semantics",
            "grade_promotion",
            "lean_kernel_checks"
        ])
    );

    write_signature(&fixture, &fixture.record, Some(RECORD_TIME));
    let mut command = accept_source_command(&fixture, true, Some(RECORD_TIME));
    command
        .args(["--signature"])
        .arg(&fixture.signature)
        .args(["--producer-key", &fixture.producer_public]);
    let output = run(command);
    assert!(output.status.success());
    let descriptor: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(contains_check(
        &descriptor["checks_not_performed"],
        "record_freshness"
    ));
    assert!(contains_check(
        &descriptor["checks_not_performed"],
        "producer_authenticity"
    ));

    let mut command = accept_source_command(&fixture, false, Some(RECORD_TIME));
    command
        .args(["--signature"])
        .arg(&fixture.signature)
        .args(["--producer-key", &fixture.producer_public]);
    let output = run(command);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    let mut lines = text.lines();
    assert!(lines
        .next()
        .unwrap()
        .starts_with("Source Evidence accepted "));
    assert!(lines
        .next()
        .unwrap()
        .starts_with("Acceptance scope excludes "));
    assert_eq!(lines.next(), Some("Producer signature: bound"));

    let output = run(full_chain(
        accept_source_command(&fixture, true, Some(RECORD_TIME)),
        &fixture,
        &fixture.producer_public,
    ));
    assert!(output.status.success());
    let descriptor: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(descriptor["accepted"], true);
    assert_eq!(descriptor["producer_signature"]["produced_at_signed"], true);
    assert!(!contains_check(
        &descriptor["producer_signature"]["checks_not_performed"],
        "producer_time_authenticity"
    ));
    assert!(contains_check(
        &descriptor["producer_signature"]["checks_not_performed"],
        "clock_authenticity"
    ));
    assert!(!contains_check(
        &descriptor["checks_not_performed"],
        "record_freshness"
    ));
    assert!(!contains_check(
        &descriptor["checks_not_performed"],
        "producer_authenticity"
    ));

    write_signature(&fixture, &fixture.record, Some(LIST_TIME));
    let output = run(full_chain(
        accept_source_command(&fixture, true, Some(RECORD_TIME)),
        &fixture,
        &fixture.producer_public,
    ));
    assert_eq!(output.status.code(), Some(1));
    let descriptor: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(descriptor["accepted"], false);
    assert_eq!(
        descriptor["producer_signature"]["produced_at_signed"],
        false
    );
    assert_eq!(descriptor["producer_signature"]["signature_valid"], true);

    let output = run(full_chain(
        accept_source_command(&fixture, false, Some(RECORD_TIME)),
        &fixture,
        &fixture.producer_public,
    ));
    assert_eq!(output.status.code(), Some(1));
    let text = String::from_utf8(output.stdout).unwrap();
    let mut lines = text.lines();
    assert_eq!(
        lines.next(),
        Some("Source Evidence rejected: producer signature failed (produced_at_signed).")
    );
    assert!(lines
        .next()
        .unwrap()
        .starts_with("Acceptance scope excludes "));
}

#[test]
fn accept_source_rejects_a_root_signed_revocation_list_below_the_minimum() {
    let fixture = fixture(Some(RECORD_TIME));
    let list_version = 1;
    write_signature(&fixture, &fixture.record, Some(RECORD_TIME));
    let mut revocation: Value =
        serde_json::from_slice(&fs::read(&fixture.revocation_list).unwrap()).unwrap();
    revocation["schema_version"] = json!("mumei.key-revocation-list/v2");
    revocation["list_version"] = json!(list_version);
    revocation["issued_at"] = json!("2024-02-01T00:00:00Z");
    revocation["not_after"] = json!("2024-03-01T00:00:00Z");
    fs::write(
        &fixture.revocation_list,
        serde_json::to_vec_pretty(&revocation).unwrap(),
    )
    .unwrap();
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

    let minimum_version = list_version.to_string();
    let mut current_command = full_chain(
        accept_source_command(&fixture, true, Some(RECORD_TIME)),
        &fixture,
        &fixture.producer_public,
    );
    current_command.args(["--min-revocation-version", &minimum_version]);
    let output = run(current_command);
    assert_eq!(output.status.code(), Some(0));
    let descriptor: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(descriptor["accepted"], true);
    assert_eq!(
        descriptor["producer_signature"]["revocation_list_current"],
        true
    );
    assert_eq!(
        descriptor["producer_signature"]["revocation_list_authentic"],
        true
    );
    assert!(contains_check(
        &descriptor["checks_performed"],
        "fresh_same_profile_source_query"
    ));

    let minimum_version = (list_version + 1).to_string();
    let mut command = full_chain(
        accept_source_command(&fixture, true, Some(RECORD_TIME)),
        &fixture,
        &fixture.producer_public,
    );
    command.args(["--min-revocation-version", &minimum_version]);
    let output = run(command);
    assert_eq!(output.status.code(), Some(1));
    let descriptor: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(descriptor["accepted"], false);
    assert_eq!(
        descriptor["producer_signature"]["revocation_list_current"],
        false
    );
    assert_eq!(
        descriptor["producer_signature"]["revocation_list_authentic"],
        true
    );
    assert!(contains_check(
        &descriptor["checks_performed"],
        "fresh_same_profile_source_query"
    ));
}

#[test]
fn accept_source_wrong_key_rejected_and_inconclusive_paths_do_not_emit_descriptors() {
    let fixture = fixture(Some(RECORD_TIME));
    write_signature(&fixture, &fixture.record, Some(RECORD_TIME));

    let output = run(full_chain(
        accept_source_command(&fixture, true, Some(RECORD_TIME)),
        &fixture,
        &fixture.root_public,
    ));
    assert_eq!(output.status.code(), Some(1));
    let descriptor: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(descriptor["accepted"], false);
    assert_eq!(descriptor["producer_signature"]["signature_valid"], false);

    let refuted_source = CLAMP.replace("body: { if x < 0 { 0 } else { x } };", "body: { 1 };");
    fs::write(&fixture.source, refuted_source).unwrap();
    let output = run(full_chain(
        accept_source_command(&fixture, true, Some(RECORD_TIME)),
        &fixture,
        &fixture.producer_public,
    ));
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());

    fs::write(&fixture.source, CLAMP).unwrap();
    let mut command = full_chain(
        accept_source_command(&fixture, true, Some(RECORD_TIME)),
        &fixture,
        &fixture.producer_public,
    );
    command.args(["--solver-resource-limit", "1"]);
    let output = run(command);
    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());
}

#[test]
fn accept_source_dependency_errors_and_accept_record_production_time_binding() {
    let stamped = fixture(Some(RECORD_TIME));
    let mut source_command = accept_source_command(&stamped, true, Some(RECORD_TIME));
    source_command
        .args(["--signature"])
        .arg(&stamped.signature)
        .args(["--producer-key", &stamped.producer_public])
        .args(["--max-age-seconds", "10"]);
    assert_usage_error(&run(source_command));

    let output = run({
        let mut command = accept_command(&stamped, true);
        command
            .args(["--signature"])
            .arg(&stamped.signature)
            .args(["--producer-key", &stamped.producer_public]);
        command
    });
    assert!(output.status.success());
    let descriptor: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(descriptor["producer_signature"]["produced_at_signed"], true);
    assert!(!contains_check(
        &descriptor["producer_signature"]["checks_not_performed"],
        "producer_time_authenticity"
    ));

    let unstamped = fixture(None);
    write_signature(&unstamped, &unstamped.record, Some(RECORD_TIME));
    let output = run({
        let mut command = accept_command(&unstamped, true);
        command
            .args(["--signature"])
            .arg(&unstamped.signature)
            .args(["--producer-key", &unstamped.producer_public]);
        command
    });
    assert!(output.status.success());
    let descriptor: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(descriptor["producer_signature"]
        .get("produced_at_signed")
        .is_none());
    assert!(contains_check(
        &descriptor["producer_signature"]["checks_not_performed"],
        "producer_time_authenticity"
    ));
}
