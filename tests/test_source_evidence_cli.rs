use mumei_core::{source_evidence, verification_evidence};
use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

const CLAMP: &str = include_str!("fixtures/semantic_contract_clamp.mm");

fn command(path: &Path, json: bool) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command.args(["evidence", "verify-source"]).arg(path);
    if json {
        command.arg("--json");
    }
    command
}

fn assert_failure(output: &Output, code: i32) {
    assert_eq!(output.status.code(), Some(code));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(!error.is_empty());
    assert!(!error.contains("FAKE_PRIVATE"));
    assert!(!error.contains("atom "));
    assert!(!error.contains("/"));
}

fn assert_success(output: &Output, json: bool) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(!text.contains("FAKE_PRIVATE"));
    assert!(!text.contains("clamp_nonnegative"));
    assert!(!text.contains("requires:"));
    assert!(!text.contains("\"proved\""));
    if json {
        let record = verification_evidence::decode(&output.stdout).unwrap();
        let expected =
            match source_evidence::verify(CLAMP, source_evidence::Options::default()).unwrap() {
                source_evidence::Outcome::EstablishedWithinProfile(record) => record,
                other => panic!("unexpected result: {other:?}"),
            };
        assert_eq!(record, *expected);
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        for key in [
            "acceptance",
            "grade",
            "seal_grade",
            "seal",
            "claims",
            "certificate",
            "evidence",
        ] {
            assert!(value.get(key).is_none());
        }
    } else {
        assert!(text
            .contains("Source postcondition established within restricted C03 pure-i64 profile."));
        assert!(text.contains("Evidence acceptance: not evaluated"));
        assert!(text.contains("no LLVM, artifact or final-executable assurance"));
    }
}

#[test]
fn producer_emits_one_record_accepted_by_representation_gate_not_a_certificate() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("FAKE_PRIVATE_SOURCE.mm");
    fs::write(&source, CLAMP).unwrap();
    for json in [false, true] {
        let output = command(&source, json).output().unwrap();
        assert_success(&output, json);
        if json {
            let record = dir.path().join("source.json");
            fs::write(&record, output.stdout).unwrap();
            let check = Command::new(env!("CARGO_BIN_EXE_mumei"))
                .args(["evidence", "check"])
                .arg(&record)
                .arg("--json")
                .output()
                .unwrap();
            assert!(check.status.success());
            assert!(check.stderr.is_empty());
            let diagnostic: Value = serde_json::from_slice(&check.stdout).unwrap();
            assert_eq!(diagnostic["representation"], "valid");
            assert_eq!(diagnostic["acceptance"], "not_evaluated");
            assert_ne!(
                diagnostic["schema_version"],
                verification_evidence::SCHEMA_VERSION
            );
        }
    }
}

#[test]
fn fresh_false_postconditions_and_false_preconditions_never_reuse_a_previous_success() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("FAKE_PRIVATE_SOURCE.mm");
    fs::write(&path, CLAMP).unwrap();
    assert_success(&command(&path, true).output().unwrap(), true);
    for json in [false, true] {
        for source in [
            CLAMP.replace("result == 0", "result == 1"),
            CLAMP.replace("requires: true", "requires: false"),
            CLAMP.replace("requires: true", "requires: x >= 0"),
        ] {
            fs::write(&path, source).unwrap();
            assert_failure(&command(&path, json).output().unwrap(), 1);
        }
    }
    fs::write(&path, CLAMP).unwrap();
    assert_success(&command(&path, true).output().unwrap(), true);
}

#[test]
fn missing_empty_duplicate_and_comment_only_contracts_emit_no_record() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("FAKE_PRIVATE_SOURCE.mm");
    for source in [
        "atom FAKE_PRIVATE_NAME(x: i64) -> i64 body: { x };",
        "atom FAKE_PRIVATE_NAME(x: i64) -> i64 requires: true; body: { x };",
        "atom FAKE_PRIVATE_NAME(x: i64) -> i64 ensures: result == x; body: { x };",
        "// requires: true; ensures: true;\natom FAKE_PRIVATE_NAME() -> i64 { body: { 0 }; }",
        "atom FAKE_PRIVATE_NAME() -> i64 requires: true; ensures: // empty\n; body: { 0 };",
        "atom FAKE_PRIVATE_NAME() -> i64 requires: ; ensures: true; body: { 0 };",
        "atom FAKE_PRIVATE_NAME() -> i64 requires: true; requires: true; ensures: true; body: { 0 };",
        "atom FAKE_PRIVATE_NAME() -> i64 requires: true; ensures: true; ensures: true; body: { 0 };",
        "atom FAKE_PRIVATE_NAME() -> i64 requires: true; ensures: true; body: { 0 }; body: { 1 };",
    ] {
        fs::write(&path, source).unwrap();
        for json in [false, true] { assert_failure(&command(&path, json).output().unwrap(), 1); }
    }
}

#[test]
fn malformed_budget_values_are_not_echoed_by_the_argument_parser() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("FAKE_PRIVATE_SOURCE.mm");
    fs::write(&path, CLAMP).unwrap();
    for flag in ["--solver-timeout", "--solver-resource-limit"] {
        for value in [
            "",
            "FAKE_PRIVATE_BUDGET",
            "-FAKE_PRIVATE_BUDGET",
            "FAKE_PRIVATE\nBUDGET",
            "-1",
            "1.2",
            "4294967296",
        ] {
            for json in [false, true] {
                let output = command(&path, json).args([flag, value]).output().unwrap();
                assert_failure(&output, 1);
                assert_eq!(
                    output.stderr,
                    b"Source evidence check failed: invalid bounded source-query budget\n"
                );
            }
        }
    }
}

#[test]
fn saved_records_and_proof_reports_are_not_accepted_as_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.mm");
    fs::write(&path, CLAMP).unwrap();
    let generated = command(&path, true).output().unwrap();
    assert_success(&generated, true);
    for bytes in [
        generated.stdout,
        br#"{"schema_version":"mumei.native-consumer-evidence/v1","evidence":[],"result":"established"}"#.to_vec(),
        br#"{"schema_version":"mumei.proof-cert/v1","grade":"Gold","verified":true}"#.to_vec(),
    ] {
        fs::write(&path, bytes).unwrap();
        for json in [false, true] { assert_failure(&command(&path, json).output().unwrap(), 1); }
    }
}

#[test]
fn bounded_solver_unknown_has_distinct_exit_and_no_success_record() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("FAKE_PRIVATE_SOURCE.mm");
    fs::write(&path, CLAMP).unwrap();
    for json in [false, true] {
        let output = command(&path, json)
            .args(["--solver-resource-limit", "1"])
            .output()
            .unwrap();
        assert_failure(&output, 3);
        assert!(String::from_utf8_lossy(&output.stderr).contains("inconclusive"));
    }
}

#[test]
fn budgets_are_explicit_bounded_and_change_provenance_not_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("FAKE_PRIVATE_SOURCE.mm");
    fs::write(&path, CLAMP).unwrap();
    let baseline: Value =
        serde_json::from_slice(&command(&path, true).output().unwrap().stdout).unwrap();
    for (flag, value) in [
        ("--solver-timeout", "9000"),
        ("--solver-resource-limit", "900000"),
    ] {
        let output = command(&path, true).args([flag, value]).output().unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let evidence: Value = serde_json::from_slice(&output.stdout).unwrap();
        for key in [
            "subject",
            "specification",
            "obligation_id",
            "semantics",
            "method",
            "result",
            "scope",
            "correspondence",
        ] {
            assert_eq!(baseline[key], evidence[key]);
        }
        assert_ne!(
            baseline["provenance"]["configuration_fingerprint"],
            evidence["provenance"]["configuration_fingerprint"]
        );
    }
    for (flag, value) in [
        ("--solver-timeout", "0"),
        ("--solver-timeout", "10001"),
        ("--solver-resource-limit", "0"),
        ("--solver-resource-limit", "1000001"),
    ] {
        assert_failure(
            &command(&path, true).args([flag, value]).output().unwrap(),
            1,
        );
    }
}

#[test]
fn malformed_unsupported_multimodule_and_non_utf8_sources_have_private_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("FAKE_PRIVATE_SOURCE.mm");
    for source in [
        vec![],
        vec![0xff, 0xfe],
        b"FAKE_PRIVATE_BAD_SOURCE".to_vec(),
        CLAMP.replace("{ 0 }", "{ 0.5 }").into_bytes(),
        CLAMP.replace("{ x }", "{ x + 1 }").into_bytes(),
        CLAMP
            .replace("{ x }", "{ FAKE_PRIVATE_CALL(x) }")
            .into_bytes(),
        CLAMP.replace("x: i64", "x: u64").into_bytes(),
        format!("{CLAMP}\n{CLAMP}").into_bytes(),
        format!("import \"FAKE_PRIVATE_IMPORT\";\n{CLAMP}").into_bytes(),
        b"atom FAKE_PRIVATE_NAME(x: i64) -> i64 requires: true; ensures: result == x; body: {"
            .to_vec(),
    ] {
        fs::write(&path, source).unwrap();
        for json in [false, true] {
            assert_failure(&command(&path, json).output().unwrap(), 1);
        }
    }
}

#[test]
fn raw_private_source_names_are_not_exported_even_when_valid() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("FAKE_PRIVATE_SOURCE.mm");
    let source = CLAMP
        .replace("clamp_nonnegative", "FAKE_PRIVATE_FUNCTION")
        .replace('x', "FAKE_PRIVATE_PARAMETER");
    fs::write(&path, &source).unwrap();
    let output = command(&path, true).output().unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("FAKE_PRIVATE"));
    let record = verification_evidence::decode(&output.stdout).unwrap();
    assert_eq!(record.subject.id, "source.mm");
    assert_eq!(record.specification.id, "source.postcondition");
}

#[test]
fn byte_limits_missing_and_nonregular_inputs_fail_without_output() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("FAKE_PRIVATE_SOURCE.mm");
    assert_failure(&command(&path, true).output().unwrap(), 1);
    assert_failure(&command(dir.path(), true).output().unwrap(), 1);
    let mut exact = CLAMP.to_owned();
    exact.push_str(&" ".repeat(65_536 - exact.len()));
    fs::write(&path, &exact).unwrap();
    let output = command(&path, true).output().unwrap();
    assert!(output.status.success());
    verification_evidence::decode(&output.stdout).unwrap();
    fs::write(&path, exact + " ").unwrap();
    assert_failure(&command(&path, true).output().unwrap(), 1);
    fs::File::create(&path)
        .unwrap()
        .set_len(1_000_000_000)
        .unwrap();
    assert_failure(&command(&path, true).output().unwrap(), 1);
}

#[cfg(unix)]
#[test]
fn symlinks_fifos_sockets_and_devices_are_rejected_promptly() {
    use std::{
        os::unix::{fs::symlink, net::UnixListener},
        process::Stdio,
        time::{Duration, Instant},
    };
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("FAKE_PRIVATE_SOURCE.mm");
    fs::write(&source, CLAMP).unwrap();
    let link = dir.path().join("FAKE_PRIVATE_LINK");
    symlink(&source, &link).unwrap();
    let broken = dir.path().join("FAKE_PRIVATE_BROKEN_LINK");
    symlink(dir.path().join("missing"), &broken).unwrap();
    let fifo = dir.path().join("FAKE_PRIVATE_FIFO");
    let cpath = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
    let socket = dir.path().join("FAKE_PRIVATE_SOCKET");
    let _listener = UnixListener::bind(&socket).unwrap();
    for path in [
        link.as_path(),
        broken.as_path(),
        fifo.as_path(),
        socket.as_path(),
        Path::new("/dev/null"),
    ] {
        let mut child = command(path, true)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if Instant::now() > deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("nonregular input did not fail promptly");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_failure(&child.wait_with_output().unwrap(), 1);
    }
}

#[cfg(unix)]
#[test]
fn read_only_source_uses_linked_solver_not_external_tools_policy_config_or_claim_files() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let tools = dir.path().join("tools");
    fs::create_dir(&tools).unwrap();
    let marker = dir.path().join("FAKE_PRIVATE_EXECUTION_MARKER");
    for name in [
        "z3",
        "llc",
        "llc-17",
        "cc",
        "clang",
        "lean",
        "lake",
        "mumei-agent",
    ] {
        let tool = tools.join(name);
        fs::write(
            &tool,
            format!("#!/bin/sh\nprintf ran > '{}'\nexit 99\n", marker.display()),
        )
        .unwrap();
        fs::set_permissions(tool, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let source = dir.path().join("FAKE_PRIVATE_SOURCE.mm");
    fs::write(&source, CLAMP).unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o444)).unwrap();
    for name in [
        "mumei.toml",
        "source.proof.json",
        "evidence.json",
        "claims.json",
    ] {
        fs::write(
            dir.path().join(name),
            "FAKE_PRIVATE_INVALID_OR_FORGED_SUCCESS",
        )
        .unwrap();
    }
    let before: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let bytes = if path.is_file() {
                Some(fs::read(&path).unwrap())
            } else {
                None
            };
            (path, bytes)
        })
        .collect();
    for json in [false, true] {
        let output = command(&source, json)
            .current_dir(dir.path())
            .env("PATH", &tools)
            .env("OPENAI_API_KEY", "FAKE_PRIVATE_SECRET")
            .env(
                "MUMEI_VERIFICATION_TIMEOUT_MS",
                "FAKE_PRIVATE_INVALID_TIMEOUT",
            )
            .env(
                "MUMEI_SOLVER_CONFIG_FINGERPRINT",
                "FAKE_PRIVATE_CONFIGURATION",
            )
            .env("MUMEI_SEAL_ALLOW_LEAN_VERIFIED", "true")
            .output()
            .unwrap();
        assert_success(&output, json);
        assert!(!marker.exists());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), before.len());
        for (path, bytes) in &before {
            if let Some(bytes) = bytes {
                assert_eq!(&fs::read(path).unwrap(), bytes);
            }
        }
    }
}

#[test]
fn cli_help_distinguishes_producer_from_acceptance_artifact_proof_and_representation_check() {
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["evidence", "verify-source", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for text in [
        "Freshly verify",
        "not LLVM/artifacts or proof acceptance",
        "16 parameters",
        "literal true",
        "--json",
        "fresh unsat only",
        "--solver-timeout",
        "--solver-resource-limit",
        "10000",
        "1000000",
        "zero/unlimited",
    ] {
        assert!(help.contains(text), "missing help contract: {text}");
    }
}
