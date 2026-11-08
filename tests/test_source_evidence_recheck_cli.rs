use mumei_core::{semantic_contract, source_evidence, verification_evidence};
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
    cmd.args(["evidence", "recheck-source"])
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
fn successful_diagnostics_are_static_not_evidence_or_acceptance_receipts() {
    let (_dir, source, saved) = fixture();
    for json in [false, true] {
        let first = command(&source, &saved, json).output().unwrap();
        assert!(first.status.success());
        assert!(first.stderr.is_empty());
        let text = String::from_utf8_lossy(&first.stdout);
        assert!(!text.contains("FAKE_PRIVATE"));
        assert!(!text.contains("clamp_nonnegative"));
        if json {
            let value: Value = serde_json::from_slice(&first.stdout).unwrap();
            assert_eq!(value["schema_version"], "mumei.source-evidence-recheck/v1");
            assert_eq!(value["recheck"], "consistent");
            assert_eq!(value["acceptance"], "not_evaluated");
            assert!(verification_evidence::decode(&first.stdout).is_err());
        } else {
            assert!(text.contains("Evidence acceptance: not evaluated"));
        }
        let different =
            "atom FAKE_PRIVATE_BINDER() -> i64 requires: true; ensures: true; body: { 0 };";
        let source_record = match source_evidence::verify(
            different,
            source_evidence::Options::default(),
        )
        .unwrap()
        {
            source_evidence::Outcome::EstablishedWithinProfile(record) => record,
            _ => panic!("expected fixture proof"),
        };
        fs::write(&source, different).unwrap();
        fs::write(&saved, serde_json::to_vec_pretty(&source_record).unwrap()).unwrap();
        let second = command(&source, &saved, json).output().unwrap();
        assert!(second.status.success());
        assert!(second.stderr.is_empty());
        assert_eq!(first.stdout, second.stdout);
        fs::write(&saved, &first.stdout).unwrap();
        failure(&command(&source, &saved, json).output().unwrap(), 1);
        fs::write(&source, CLAMP).unwrap();
        let record =
            match source_evidence::verify(CLAMP, source_evidence::Options::default()).unwrap() {
                source_evidence::Outcome::EstablishedWithinProfile(record) => record,
                _ => unreachable!(),
            };
        fs::write(&saved, serde_json::to_vec(&record).unwrap()).unwrap();
    }
}

#[test]
fn changed_metadata_digests_semantics_trust_and_provenance_never_consume_as_success() {
    let (_dir, source, saved) = fixture();
    let original: Value = serde_json::from_slice(&fs::read(&saved).unwrap()).unwrap();
    for pointer in [
        "/subject/id",
        "/subject/kind",
        "/subject/sha256",
        "/specification/id",
        "/specification/sha256",
        "/obligation_id",
        "/semantics/profile",
        "/semantics/integer_model",
        "/semantics/target_triple",
        "/method",
        "/execution",
        "/result",
        "/scope/kind",
        "/correspondence/relation",
        "/correspondence/status",
        "/correspondence/notes/0",
        "/assumptions/0",
        "/exclusions/0",
        "/provenance/adapter/id",
        "/provenance/adapter/version",
        "/provenance/adapter/protocol_version",
        "/provenance/tool/name",
        "/provenance/tool/version",
        "/provenance/configuration_fingerprint",
        "/provenance/translator_version",
        "/provenance/bridge_lemma_hash",
        "/artifact_paths",
    ] {
        let mut changed = original.clone();
        let replacement = if pointer.ends_with("sha256")
            || pointer.ends_with("fingerprint")
            || pointer.ends_with("lemma_hash")
        {
            Value::String("f".repeat(64))
        } else if pointer == "/artifact_paths" {
            serde_json::json!(["FAKE_PRIVATE_NEVER_READ"])
        } else {
            Value::String("FAKE_PRIVATE_METADATA".into())
        };
        if let Some(field) = changed.pointer_mut(pointer) {
            *field = replacement;
        } else {
            let (parent, key) = pointer.rsplit_once('/').unwrap();
            changed
                .pointer_mut(parent)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert(key.into(), replacement);
        }
        fs::write(&saved, serde_json::to_vec(&changed).unwrap()).unwrap();
        for json in [false, true] {
            failure(&command(&source, &saved, json).output().unwrap(), 1);
        }
    }
    fs::write(&saved, serde_json::to_vec_pretty(&original).unwrap()).unwrap();
    assert!(command(&source, &saved, true)
        .output()
        .unwrap()
        .status
        .success());
}

#[test]
fn nested_required_unknown_and_duplicate_fields_use_the_strict_common_gate() {
    let (_dir, source, saved) = fixture();
    let bytes = fs::read(&saved).unwrap();
    let original: Value = serde_json::from_slice(&bytes).unwrap();
    for pointer in [
        "",
        "/subject",
        "/specification",
        "/semantics",
        "/scope",
        "/correspondence",
        "/provenance",
        "/provenance/adapter",
        "/provenance/tool",
    ] {
        for key in original
            .pointer(pointer)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
        {
            let mut changed = original.clone();
            changed
                .pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(key);
            fs::write(&saved, serde_json::to_vec(&changed).unwrap()).unwrap();
            failure(&command(&source, &saved, true).output().unwrap(), 1);
        }
        let mut changed = original.clone();
        changed
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("FAKE_PRIVATE_UNKNOWN".into(), Value::Null);
        fs::write(&saved, serde_json::to_vec(&changed).unwrap()).unwrap();
        failure(&command(&source, &saved, true).output().unwrap(), 1);
    }
    let text = std::str::from_utf8(&bytes).unwrap();
    for input in [
        text.replacen(
            '{',
            "{\"schema_versi\\u006fn\":\"FAKE_PRIVATE_DUPLICATE\",",
            1,
        ),
        text.replacen(
            "\"profile\":",
            "\"profile\":\"FAKE_PRIVATE_DUPLICATE\",\"profile\":",
            1,
        ),
        r#"{"schema_version":"mumei.native-consumer-evidence/v1","evidence":[]}"#.into(),
        r#"{"schema_version":"mumei.proof-cert/v1","verified":true}"#.into(),
    ] {
        fs::write(&saved, input).unwrap();
        for json in [false, true] {
            failure(&command(&source, &saved, json).output().unwrap(), 1);
        }
    }
}

#[test]
fn forged_rebound_success_does_not_replace_live_refutation_or_unknown() {
    let (_dir, source, saved) = fixture();
    let original = fs::read(&saved).unwrap();
    let wrong = CLAMP.replace("body: { if x < 0 { 0 } else { x } };", "body: { 1 };");
    let contract = semantic_contract::extract_pure_i64_contract(&wrong, "source.mm", None).unwrap();
    let mut forged: Value = serde_json::from_slice(&original).unwrap();
    forged["subject"]["sha256"] = Value::String(contract.source_sha256);
    fs::write(&saved, serde_json::to_vec(&forged).unwrap()).unwrap();
    fs::write(&source, wrong).unwrap();
    for json in [false, true] {
        let refuted = command(&source, &saved, json).output().unwrap();
        failure(&refuted, 1);
        assert!(String::from_utf8_lossy(&refuted.stderr).contains("refuted"));
    }
    fs::write(&source, CLAMP).unwrap();
    fs::write(&saved, original).unwrap();
    for json in [false, true] {
        let unknown = command(&source, &saved, json)
            .args(["--solver-resource-limit", "1"])
            .output()
            .unwrap();
        failure(&unknown, 3);
        assert!(String::from_utf8_lossy(&unknown.stderr).contains("inconclusive"));
    }
}

#[test]
fn budgets_are_external_explicit_and_never_reflected_in_errors() {
    let (_dir, source, saved) = fixture();
    failure(
        &command(&source, &saved, true)
            .args(["--solver-timeout", "9000"])
            .output()
            .unwrap(),
        1,
    );
    let producer = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["evidence", "verify-source"])
        .arg(&source)
        .args(["--json", "--solver-timeout", "9000"])
        .output()
        .unwrap();
    assert!(producer.status.success());
    fs::write(&saved, producer.stdout).unwrap();
    assert!(command(&source, &saved, true)
        .args(["--solver-timeout", "9000"])
        .output()
        .unwrap()
        .status
        .success());
    failure(&command(&source, &saved, true).output().unwrap(), 1);
    for flag in ["--solver-timeout", "--solver-resource-limit"] {
        for value in [
            "0",
            "4294967296",
            "-1",
            "",
            "FAKE_PRIVATE_VALUE",
            "-FAKE_PRIVATE_VALUE",
            "FAKE_PRIVATE\nVALUE",
        ] {
            failure(
                &command(&source, &saved, true)
                    .args([flag, value])
                    .output()
                    .unwrap(),
                1,
            );
        }
    }
}

#[test]
fn help_requires_separate_source_and_describes_non_acceptance() {
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["evidence", "recheck-source", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    for text in [
        "--source",
        "--solver-timeout",
        "--solver-resource-limit",
        "--json",
        "not proof acceptance",
        "linked Z3",
        "65,536",
    ] {
        assert!(help.contains(text), "missing help contract: {text}");
    }
    let missing = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["evidence", "recheck-source", "FAKE_PRIVATE_RECORD"])
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(2));
    assert!(missing.stdout.is_empty());
}

#[test]
fn malformed_unsupported_and_bounded_inputs_never_emit_diagnostics_as_success() {
    let (_dir, source, saved) = fixture();
    let original = fs::read(&saved).unwrap();
    for input in [
        vec![],
        vec![0xff],
        b"{}".to_vec(),
        b"[]".to_vec(),
        [original.clone(), original.clone()].concat(),
        vec![b' '; 65_537],
        br#"{"schema_version":"FAKE_PRIVATE","schema_versi\u006fn":"FAKE_PRIVATE"}"#.to_vec(),
    ] {
        fs::write(&saved, input).unwrap();
        failure(&command(&source, &saved, true).output().unwrap(), 1);
    }
    fs::write(&saved, &original).unwrap();
    for input in [
        vec![],
        vec![0xff],
        b"atom FAKE_PRIVATE() -> i64 body: { 0 };".to_vec(),
        b"atom FAKE_PRIVATE() -> i64 requires: false; ensures: false; body: { 0 };".to_vec(),
        vec![b' '; 65_537],
    ] {
        fs::write(&source, input).unwrap();
        failure(&command(&source, &saved, true).output().unwrap(), 1);
    }
    let mut source_bytes = CLAMP.as_bytes().to_vec();
    source_bytes.resize(65_536, b' ');
    let padded = std::str::from_utf8(&source_bytes).unwrap();
    let record = match source_evidence::verify(padded, source_evidence::Options::default()).unwrap()
    {
        source_evidence::Outcome::EstablishedWithinProfile(record) => record,
        _ => unreachable!(),
    };
    let mut record_bytes = serde_json::to_vec(&record).unwrap();
    record_bytes.resize(65_536, b' ');
    fs::write(&source, source_bytes).unwrap();
    fs::write(&saved, record_bytes).unwrap();
    assert!(command(&source, &saved, true)
        .output()
        .unwrap()
        .status
        .success());
}

#[cfg(unix)]
#[test]
fn nonregular_inputs_and_saved_artifact_paths_are_not_followed_or_executed() {
    use std::os::unix::{
        fs::{symlink, PermissionsExt},
        net::UnixListener,
    };
    let (dir, source, saved) = fixture();
    let link = dir.path().join("FAKE_PRIVATE_LINK");
    symlink(&source, &link).unwrap();
    let fifo = dir.path().join("FAKE_PRIVATE_FIFO");
    assert!(Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    let socket = dir.path().join("FAKE_PRIVATE_SOCKET");
    let _listener = UnixListener::bind(&socket).unwrap();
    for input in [
        &link,
        &fifo,
        &socket,
        dir.path(),
        Path::new("/dev/null"),
        &dir.path().join("FAKE_PRIVATE_MISSING"),
    ] {
        failure(&command(input, &saved, true).output().unwrap(), 1);
        failure(&command(&source, input, true).output().unwrap(), 1);
    }
    let marker = dir.path().join("FAKE_PRIVATE_EXECUTED");
    let trap = dir.path().join("FAKE_PRIVATE_TRAP");
    fs::write(
        &trap,
        format!("#!/bin/sh\nprintf executed > '{}'\n", marker.display()),
    )
    .unwrap();
    fs::set_permissions(&trap, fs::Permissions::from_mode(0o700)).unwrap();
    for name in ["z3", "mumei", "clang", "cc", "lean", "lake"] {
        symlink(&trap, dir.path().join(name)).unwrap();
    }
    fs::write(dir.path().join("mumei.toml"), "FAKE_PRIVATE_INVALID_CONFIG").unwrap();
    fs::write(dir.path().join("claim-book.json"), "FAKE_PRIVATE_CLAIM").unwrap();
    for path in [&source, &saved] {
        let mut p = fs::metadata(path).unwrap().permissions();
        p.set_readonly(true);
        fs::set_permissions(path, p).unwrap();
    }
    let before_source = fs::read(&source).unwrap();
    let before_saved = fs::read(&saved).unwrap();
    let output = command(&source, &saved, true)
        .env("PATH", dir.path())
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let mut value: Value = serde_json::from_slice(&before_saved).unwrap();
    value["artifact_paths"] = serde_json::json!([trap.to_string_lossy(), fifo.to_string_lossy()]);
    let fake = dir.path().join("FAKE_PRIVATE_REPORT.json");
    fs::write(&fake, serde_json::to_vec(&value).unwrap()).unwrap();
    failure(
        &command(&source, &fake, true)
            .env("PATH", dir.path())
            .current_dir(dir.path())
            .output()
            .unwrap(),
        1,
    );
    assert!(!marker.exists());
    assert_eq!(fs::read(&source).unwrap(), before_source);
    assert_eq!(fs::read(&saved).unwrap(), before_saved);
}
