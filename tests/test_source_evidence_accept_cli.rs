use mumei_core::semantic_contract;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const CLAMP: &str = include_str!("fixtures/semantic_contract_clamp.mm");

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("FAKE_PRIVATE_SOURCE.mm");
    let saved = dir.path().join("FAKE_PRIVATE_EVIDENCE.json");
    fs::write(&source, CLAMP).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["evidence", "verify-source"])
        .arg(&source)
        .arg("--json")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    fs::write(&saved, output.stdout).unwrap();
    (dir, source, saved)
}

fn command(source: &Path, saved: &Path, json: bool) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mumei"));
    cmd.args(["evidence", "accept-source"])
        .arg(saved)
        .arg("--source")
        .arg(source);
    if json {
        cmd.arg("--json");
    }
    cmd
}

fn failure(output: &Output, code: i32) {
    assert_eq!(output.status.code(), Some(code));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(!error.is_empty());
    for forbidden in ["FAKE_PRIVATE", "atom ", "/"] {
        assert!(!error.contains(forbidden));
    }
}

#[test]
fn accepted_descriptor_is_static_anonymous_and_not_reusable_evidence() {
    let (_dir, source, saved) = fixture();
    for json in [false, true] {
        let output = command(&source, &saved, json).output().unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(!text.contains("FAKE_PRIVATE"));
        assert!(!text.contains("clamp_nonnegative"));
        if json {
            let value: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                value["schema_version"],
                "mumei.source-evidence-acceptance/v1"
            );
            assert_eq!(value["acceptance"], "evaluated");
            assert_eq!(value["accepted"], true);
            // The fingerprint binds the descriptor to the accepted
            // record's bytes — sha256 hex — without echoing the claim.
            let saved_bytes = std::fs::read(&saved).unwrap();
            use sha2::{Digest, Sha256};
            assert_eq!(
                value["evidence_fingerprint"],
                format!("{:x}", Sha256::digest(&saved_bytes))
            );
            assert_eq!(
                value["checks_performed"],
                serde_json::json!(["representation_gate", "fresh_same_profile_source_query"])
            );
            for unchecked in [
                "producer_authenticity",
                "historical_execution",
                "record_freshness",
                "parser_encoder_solver_correctness",
                "independent_model_correspondence",
                "llvm_correspondence",
                "artifact_semantics",
                "grade_promotion",
                "lean_kernel_checks",
            ] {
                assert!(value["checks_not_performed"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|entry| entry == unchecked));
            }
        } else {
            assert!(text.contains("accepted"));
        }
    }
    // The descriptor must not decode as reusable Evidence or a certificate.
    let accepted = command(&source, &saved, true).output().unwrap();
    let check = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["evidence", "check", "--stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.as_mut().unwrap().write_all(&accepted.stdout)?;
            child.wait_with_output()
        })
        .unwrap();
    assert_eq!(check.status.code(), Some(1));
}

#[test]
fn rejected_inconclusive_and_invalid_inputs_emit_no_descriptor() {
    let (_dir, source, saved) = fixture();
    let original = fs::read(&saved).unwrap();
    // Refuted: rebind the record's source digest to a refuted source.
    let wrong = CLAMP.replace("body: { if x < 0 { 0 } else { x } };", "body: { 1 };");
    let contract =
        semantic_contract::extract_pure_i64_contract(&wrong, "source.mm", None, false).unwrap();
    let mut forged: Value = serde_json::from_slice(&original).unwrap();
    forged["subject"]["sha256"] = Value::String(contract.source_sha256);
    fs::write(&saved, serde_json::to_vec(&forged).unwrap()).unwrap();
    fs::write(&source, wrong).unwrap();
    for json in [false, true] {
        failure(&command(&source, &saved, json).output().unwrap(), 1);
    }
    fs::write(&source, CLAMP).unwrap();
    fs::write(&saved, &original).unwrap();
    // Inconclusive.
    for json in [false, true] {
        failure(
            &command(&source, &saved, json)
                .args(["--solver-resource-limit", "1"])
                .output()
                .unwrap(),
            3,
        );
    }
    // Malformed record, tampered field, wrong source and bad budgets.
    for pointer in ["/subject/sha256", "/method", "/scope/kind"] {
        let mut changed: Value = serde_json::from_slice(&original).unwrap();
        *changed.pointer_mut(pointer).unwrap() = if pointer.ends_with("sha256") {
            Value::String("f".repeat(64))
        } else {
            Value::String("FAKE_PRIVATE_METADATA".into())
        };
        fs::write(&saved, serde_json::to_vec(&changed).unwrap()).unwrap();
        failure(&command(&source, &saved, true).output().unwrap(), 1);
    }
    fs::write(&saved, b"FAKE_PRIVATE_MALFORMED").unwrap();
    failure(&command(&source, &saved, true).output().unwrap(), 1);
    fs::write(&saved, &original).unwrap();
    for flag in ["--solver-timeout", "--solver-resource-limit"] {
        for value in ["0", "-1", "FAKE_PRIVATE_VALUE"] {
            failure(
                &command(&source, &saved, true)
                    .args([flag, value])
                    .output()
                    .unwrap(),
                1,
            );
        }
    }
    assert!(command(&source, &saved, true)
        .output()
        .unwrap()
        .status
        .success());
}

#[test]
fn help_requires_separate_source_and_describes_bounded_acceptance() {
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["evidence", "accept-source", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    for text in [
        "--source",
        "--solver-timeout",
        "--solver-resource-limit",
        "--json",
        "acceptance",
        "65,536",
    ] {
        assert!(help.contains(text), "missing help contract: {text}");
    }
    let missing = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["evidence", "accept-source", "FAKE_PRIVATE_RECORD"])
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(2));
    assert!(missing.stdout.is_empty());
}
