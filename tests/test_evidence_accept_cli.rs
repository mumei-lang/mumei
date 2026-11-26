use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const CLAMP: &str = include_str!("fixtures/semantic_contract_clamp.mm");

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("FAKE_PRIVATE_SOURCE.mm");
    let record = dir.path().join("FAKE_PRIVATE_RECORD.json");
    let requirement = dir.path().join("FAKE_PRIVATE_REQUIREMENT.json");
    fs::write(&source, CLAMP).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["evidence", "verify-source"])
        .arg(&source)
        .arg("--json")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    fs::write(&record, &output.stdout).unwrap();
    let saved: Value = serde_json::from_slice(&output.stdout).unwrap();
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
    (dir, record, requirement, source)
}

fn command(record: &Path, requirement: &Path, json: bool) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mumei"));
    cmd.args(["evidence", "accept"])
        .arg(record)
        .arg("--requirement")
        .arg(requirement);
    if json {
        cmd.arg("--json");
    }
    cmd
}

fn run(record: &Path, requirement: &Path, json: bool) -> Output {
    command(record, requirement, json).output().unwrap()
}

fn rewrite_requirement(path: &Path, f: impl FnOnce(&mut Value)) {
    let mut doc: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    f(&mut doc);
    fs::write(path, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
}

fn rewrite_record(path: &Path, f: impl FnOnce(&mut Value)) {
    rewrite_requirement(path, f);
}

fn check<'a>(value: &'a Value, name: &str) -> &'a str {
    value["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == name)
        .unwrap_or_else(|| panic!("missing check {name}"))["outcome"]
        .as_str()
        .unwrap()
}

#[test]
fn accepted_record_emits_verdict_and_fingerprints_only() {
    let (_dir, record, requirement, _source) = fixture();
    for json in [false, true] {
        let output = run(&record, &requirement, json);
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(!text.contains("FAKE_PRIVATE"));
        assert!(!text.contains("clamp_nonnegative"));
        if json {
            let value: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(value["schema_version"], "mumei.evidence-acceptance/v1");
            assert_eq!(
                value["requirement_schema_version"],
                "mumei.evidence-requirement/v1"
            );
            assert_eq!(
                value["evidence_schema_version"],
                "mumei.verification-evidence/v1"
            );
            assert_eq!(value["acceptance"], "evaluated");
            assert_eq!(value["verdict"], "accepted");
            assert_eq!(
                value["evidence_fingerprint"],
                format!("{:x}", Sha256::digest(fs::read(&record).unwrap()))
            );
            assert_eq!(
                value["requirement_fingerprint"],
                format!("{:x}", Sha256::digest(fs::read(&requirement).unwrap()))
            );
            assert!(value["checks"]
                .as_array()
                .unwrap()
                .iter()
                .all(|entry| entry["outcome"] == "passed"));
            for unchecked in [
                "claim_truth",
                "producer_authenticity",
                "record_freshness",
                "independent_model_correspondence",
                "artifact_semantics",
                "tool_execution",
                "grade_promotion",
            ] {
                assert!(value["checks_not_performed"]
                    .as_array()
                    .unwrap()
                    .contains(&json!(unchecked)));
            }
        }
    }
}

#[test]
fn pin_mismatch_rejects_with_failed_check_and_exit_1() {
    let (_dir, record, requirement, _source) = fixture();
    rewrite_requirement(&requirement, |doc| {
        doc["obligation_id"] = json!("other-obligation");
    });
    let output = run(&record, &requirement, true);
    assert_eq!(output.status.code(), Some(1));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["verdict"], "rejected");
    assert_eq!(check(&value, "obligation_pin"), "failed");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("FAKE_PRIVATE"));
    assert!(!stderr.contains('/'));
}

#[test]
fn non_completed_execution_is_inconclusive_exit_3() {
    let (_dir, record, requirement, _source) = fixture();
    rewrite_record(&record, |doc| {
        doc["execution"] = json!("timeout");
        doc["result"] = json!("unknown");
        doc["correspondence"]["relation"] = json!("unchecked");
        doc["correspondence"]["status"] = json!("unverified");
    });
    let output = run(&record, &requirement, true);
    assert_eq!(output.status.code(), Some(3));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["verdict"], "inconclusive");
    assert_eq!(check(&value, "execution_completed"), "indeterminate");
}

#[test]
fn finite_execution_record_can_never_satisfy_proof_requirement() {
    let (_dir, record, requirement, _source) = fixture();
    rewrite_record(&record, |doc| {
        doc["method"] = json!("finite_execution");
        doc["scope"]["kind"] = json!("finite");
        doc["scope"]["bounds"] = json!(["input_count <= 4"]);
        doc["result"] = json!("observed_no_failure");
    });
    let output = run(&record, &requirement, true);
    assert_eq!(output.status.code(), Some(1));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(check(&value, "method_sufficient"), "failed");
    // The same record does satisfy an observation-floor requirement.
    rewrite_requirement(&requirement, |doc| {
        doc["minimum_assurance"] = json!("observation");
    });
    let output = run(&record, &requirement, true);
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["verdict"], "accepted");
}

#[test]
fn malformed_inputs_fail_statically_without_content() {
    let (_dir, record, requirement, _source) = fixture();
    for (target, f) in [
        (
            record.clone(),
            Box::new(|v: &mut Value| {
                v["subject"]["kind"] = json!("mystery");
            }) as Box<dyn FnOnce(&mut Value)>,
        ),
        (
            requirement.clone(),
            Box::new(|v: &mut Value| {
                v["minimum_assurance"] = json!("proof");
            }),
        ),
    ] {
        rewrite_record(&target, f);
        let output = run(&record, &requirement, true);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.is_empty());
        assert!(!stderr.contains("FAKE_PRIVATE"));
        assert!(!stderr.contains('/'));
    }
}

#[test]
fn stdin_record_matches_file_input() {
    let (_dir, record, requirement, _source) = fixture();
    let bytes = fs::read(&record).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mumei"));
    let output = cmd
        .args(["evidence", "accept", "--stdin"])
        .arg("--requirement")
        .arg(&requirement)
        .arg("--json")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(&bytes).unwrap();
            child.wait_with_output()
        })
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["verdict"], "accepted");
}

fn list_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let (dir, record, requirement, _source) = fixture();
    let saved: Value = serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
    let list = dir.path().join("FAKE_PRIVATE_LIST.json");
    fs::write(
        &list,
        serde_json::to_vec_pretty(&json!({
            "schema_version": "mumei.verification-evidence-list/v1",
            "evidence": [saved],
        }))
        .unwrap(),
    )
    .unwrap();
    (dir, list, requirement)
}

fn list_command(list: &Path, requirement: &Path, json: bool) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mumei"));
    cmd.args(["evidence", "accept-list"])
        .arg(list)
        .arg("--requirement")
        .arg(requirement);
    if json {
        cmd.arg("--json");
    }
    cmd
}

fn rewrite_list(path: &Path, f: impl FnOnce(&mut Value)) {
    rewrite_requirement(path, f);
}

#[test]
fn accepted_record_in_list_emits_aggregate_accepted() {
    let (_dir, list, requirement) = list_fixture();
    let output = list_command(&list, &requirement, true).output().unwrap();
    assert!(output.status.success());
    let doc: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(doc["schema_version"], "mumei.evidence-list-acceptance/v1");
    assert_eq!(doc["verdict"], "accepted");
    assert_eq!(doc["records_evaluated"], 1);
    assert_eq!(doc["records"][0]["verdict"], "accepted");
    assert_eq!(check(&doc["records"][0], "subject_pin"), "passed");
}

#[test]
fn mixed_list_still_accepts_on_one_accepted_record() {
    let (_dir, list, requirement) = list_fixture();
    rewrite_list(&list, |doc| {
        let mut bad = doc["evidence"][0].clone();
        bad["subject"]["sha256"] = json!("0".repeat(64));
        bad["subject"]["id"] = json!("other.mm");
        bad["obligation_id"] = json!("other-obligation");
        doc["evidence"].as_array_mut().unwrap().push(bad);
    });
    let output = list_command(&list, &requirement, true).output().unwrap();
    assert!(output.status.success());
    let doc: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(doc["verdict"], "accepted");
    assert_eq!(doc["records"][1]["verdict"], "rejected");
}

#[test]
fn conflicting_list_never_accepts_silently() {
    // Same obligation_id, different subject sha256 — an accepted
    // verdict over a contradictory pin set must not pass.
    let (_dir, list, requirement) = list_fixture();
    rewrite_list(&list, |doc| {
        let mut bad = doc["evidence"][0].clone();
        bad["subject"]["sha256"] = json!("0".repeat(64));
        doc["evidence"].as_array_mut().unwrap().push(bad);
    });
    let output = list_command(&list, &requirement, true).output().unwrap();
    assert_eq!(output.status.code(), Some(3));
    let doc: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(doc["verdict"], "inconclusive");
    assert_eq!(doc["consistent"], false);
    assert!(doc["conflict_count"].as_u64().unwrap() >= 1);
    assert!(doc["checks_performed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c == "cross_record_pin_agreement"));
}

#[test]
fn only_rejected_records_reject_with_exit_1() {
    let (_dir, list, requirement) = list_fixture();
    rewrite_list(&list, |doc| {
        doc["evidence"][0]["subject"]["sha256"] = json!("0".repeat(64));
    });
    let output = list_command(&list, &requirement, true).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let doc: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(doc["verdict"], "rejected");
}

#[test]
fn empty_or_inconclusive_list_is_inconclusive_exit_3() {
    let (_dir, list, requirement) = list_fixture();
    rewrite_list(&list, |doc| {
        doc["evidence"] = json!([]);
    });
    let output = list_command(&list, &requirement, true).output().unwrap();
    assert_eq!(output.status.code(), Some(3));
    let doc: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(doc["verdict"], "inconclusive");
    assert_eq!(doc["records_evaluated"], 0);
}

#[test]
fn malformed_list_fails_statically_without_content() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("bad.json");
    fs::write(&bad, b"{\"evidence\": \"FAKE_PRIVATE_MALFORMED\"}").unwrap();
    let (_d2, _record, requirement, _s) = fixture();
    let output = list_command(&bad, &requirement, true).output().unwrap();
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("FAKE_PRIVATE"), "{stdout}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("FAKE_PRIVATE"), "{stderr}");
}

fn requirement_list_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let (dir, list, requirement) = list_fixture();
    let saved: Value = serde_json::from_slice(&fs::read(&requirement).unwrap()).unwrap();
    let requirement_list = dir.path().join("FAKE_PRIVATE_REQUIREMENT_LIST.json");
    fs::write(
        &requirement_list,
        serde_json::to_vec_pretty(&json!({
            "schema_version": "mumei.evidence-requirement-list/v1",
            "requirements": [saved],
        }))
        .unwrap(),
    )
    .unwrap();
    (dir, list, requirement_list)
}

fn requirement_list_command(list: &Path, requirement_list: &Path, json: bool) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mumei"));
    cmd.args(["evidence", "accept-list"])
        .arg(list)
        .arg("--requirement-list")
        .arg(requirement_list);
    if json {
        cmd.arg("--json");
    }
    cmd
}

#[test]
fn requirement_list_all_accepted_emits_aggregate_accepted() {
    let (_dir, list, requirement_list) = requirement_list_fixture();
    let output = requirement_list_command(&list, &requirement_list, true)
        .output()
        .unwrap();
    assert!(output.status.success());
    let doc: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        doc["schema_version"],
        "mumei.evidence-requirement-list-acceptance/v1"
    );
    assert_eq!(doc["verdict"], "accepted");
    assert_eq!(doc["requirements_evaluated"], 1);
    assert_eq!(doc["requirements"][0]["verdict"], "accepted");
    assert_eq!(doc["requirements"][0]["records"][0]["verdict"], "accepted");
    assert!(doc["checks_performed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c == "conjunctive_requirement_fold"));
    assert_eq!(
        doc["requirement_list_fingerprint"],
        json!(format!(
            "{:x}",
            Sha256::digest(fs::read(&requirement_list).unwrap())
        ))
    );
}

#[test]
fn requirement_list_one_rejected_requirement_rejects() {
    let (_dir, list, requirement_list) = requirement_list_fixture();
    rewrite_list(&requirement_list, |doc| {
        let mut bad = doc["requirements"][0].clone();
        bad["subject_sha256"] = json!("0".repeat(64));
        doc["requirements"].as_array_mut().unwrap().push(bad);
    });
    let output = requirement_list_command(&list, &requirement_list, true)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let doc: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(doc["verdict"], "rejected");
    assert_eq!(doc["requirements"][0]["verdict"], "accepted");
    assert_eq!(doc["requirements"][1]["verdict"], "rejected");
}

#[test]
fn requirement_list_one_inconclusive_stays_inconclusive() {
    let (_dir, list, requirement_list) = requirement_list_fixture();
    rewrite_list(&requirement_list, |doc| {
        let mut weak = doc["requirements"][0].clone();
        weak["correspondence_status"] = json!(["validated"]);
        doc["requirements"].as_array_mut().unwrap().push(weak);
    });
    let output = requirement_list_command(&list, &requirement_list, true)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let doc: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(doc["verdict"], "inconclusive");
    assert_eq!(doc["requirements"][1]["verdict"], "inconclusive");
}

#[test]
fn requirement_list_conflict_demotes_accepted_to_inconclusive() {
    let (_dir, list, requirement_list) = requirement_list_fixture();
    rewrite_list(&list, |doc| {
        let mut bad = doc["evidence"][0].clone();
        bad["subject"]["sha256"] = json!("0".repeat(64));
        doc["evidence"].as_array_mut().unwrap().push(bad);
    });
    let output = requirement_list_command(&list, &requirement_list, true)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let doc: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(doc["verdict"], "inconclusive");
    assert_eq!(doc["consistent"], false);
    assert_eq!(doc["requirements"][0]["verdict"], "inconclusive");
}

#[test]
fn requirement_list_empty_or_oversized_is_a_static_failure() {
    let (_dir, list, requirement_list) = requirement_list_fixture();
    let valid_item = serde_json::from_slice::<Value>(&fs::read(&requirement_list).unwrap())
        .unwrap()["requirements"][0]
        .clone();
    rewrite_list(&requirement_list, |doc| {
        doc["requirements"] = json!([]);
    });
    let output = requirement_list_command(&list, &requirement_list, true)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("FAKE_PRIVATE"), "{stderr}");
    rewrite_list(&requirement_list, |doc| {
        doc["requirements"] = json!(vec![valid_item.clone(); 16]);
    });
    let output = requirement_list_command(&list, &requirement_list, true)
        .output()
        .unwrap();
    assert!(output.status.success());
    rewrite_list(&requirement_list, |doc| {
        doc["requirements"] = json!(vec![valid_item; 17]);
    });
    let output = requirement_list_command(&list, &requirement_list, true)
        .output()
        .unwrap();
    assert!(!output.status.success());
}

fn accept_requirement_list_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let (dir, record, requirement, _source) = fixture();
    let saved: Value = serde_json::from_slice(&fs::read(&requirement).unwrap()).unwrap();
    let requirement_list = dir.path().join("FAKE_PRIVATE_REQUIREMENT_LIST.json");
    fs::write(
        &requirement_list,
        serde_json::to_vec_pretty(&json!({
            "schema_version": "mumei.evidence-requirement-list/v1",
            "requirements": [saved],
        }))
        .unwrap(),
    )
    .unwrap();
    (dir, record, requirement_list)
}

fn accept_requirement_list_command(record: &Path, requirement_list: &Path, json: bool) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mumei"));
    cmd.args(["evidence", "accept"])
        .arg(record)
        .arg("--requirement-list")
        .arg(requirement_list);
    if json {
        cmd.arg("--json");
    }
    cmd
}

#[test]
fn accept_requirement_list_all_accepted_emits_accepted() {
    let (_dir, record, requirement_list) = accept_requirement_list_fixture();
    let output = accept_requirement_list_command(&record, &requirement_list, true)
        .output()
        .unwrap();
    assert!(output.status.success());
    let doc: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        doc["schema_version"],
        "mumei.evidence-requirement-list-acceptance/v1"
    );
    assert_eq!(doc["verdict"], "accepted");
    assert_eq!(doc["requirements_evaluated"], 1);
    assert_eq!(doc["records_evaluated"], 1);
    assert_eq!(doc["consistent"], true);
    assert_eq!(doc["conflict_count"], 0);
    // A single record pins as evidence_fingerprint with the record schema,
    // never as a list envelope.
    assert_eq!(
        doc["evidence_schema_version"],
        "mumei.verification-evidence/v1"
    );
    assert_eq!(
        doc["evidence_fingerprint"],
        json!(format!("{:x}", Sha256::digest(fs::read(&record).unwrap())))
    );
    assert!(doc["list_fingerprint"].is_null());
    assert!(doc["checks_performed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c == "conjunctive_requirement_fold"));
}

#[test]
fn accept_requirement_list_one_rejected_requirement_rejects() {
    let (_dir, record, requirement_list) = accept_requirement_list_fixture();
    rewrite_list(&requirement_list, |doc| {
        let mut bad = doc["requirements"][0].clone();
        bad["subject_sha256"] = json!("0".repeat(64));
        doc["requirements"].as_array_mut().unwrap().push(bad);
    });
    let output = accept_requirement_list_command(&record, &requirement_list, true)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let doc: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(doc["verdict"], "rejected");
}

#[test]
fn accept_requires_a_requirement_declaration() {
    let (_dir, record, _requirement, _source) = fixture();
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["evidence", "accept"])
        .arg(&record)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("FAKE_PRIVATE"), "{stderr}");
}

#[test]
fn accept_requirement_and_requirement_list_are_exclusive() {
    let (_dir, record, requirement, _source) = fixture();
    let requirement_list = _dir.path().join("FAKE_PRIVATE_REQUIREMENT_LIST.json");
    fs::write(
        &requirement_list,
        serde_json::to_vec_pretty(&json!({
            "schema_version": "mumei.evidence-requirement-list/v1",
            "requirements": [serde_json::from_slice::<Value>(&fs::read(&requirement).unwrap()).unwrap()],
        }))
        .unwrap(),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["evidence", "accept"])
        .arg(&record)
        .arg("--requirement")
        .arg(&requirement)
        .arg("--requirement-list")
        .arg(&requirement_list)
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[test]
fn requirement_and_requirement_list_are_exclusive() {
    let (_dir, list, requirement) = list_fixture();
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["evidence", "accept-list"])
        .arg(&list)
        .arg("--requirement")
        .arg(&requirement)
        .arg("--requirement-list")
        .arg(&requirement)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}
