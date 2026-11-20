#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use mumei_core::native_bundle::{self, BundleFile, RECORD_FILE};
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    process::{Command, Output},
};

const GOOD: &str = "atom main() -> i64 requires: true; ensures: result == 7; body: { 7 };";

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(args)
        .output()
        .unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn build(root: &Path) -> (std::path::PathBuf, std::path::PathBuf, String) {
    let source = root.join("source.mm");
    let bundle = root.join("bundle");
    fs::write(&source, GOOD).unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    (source, bundle, id)
}

fn check(source: &Path, bundle: &Path, id: &str) -> Output {
    cli(&[
        "native-bundle",
        "check",
        bundle.to_str().unwrap(),
        source.to_str().unwrap(),
        "--expected-build-id",
        id,
    ])
}

fn recheck(source: &Path, bundle: &Path, id: &str) -> Output {
    consume("recheck-source", source, bundle, id)
}

fn recheck_llvm(source: &Path, bundle: &Path, id: &str) -> Output {
    let text = consume("recheck-llvm", source, bundle, id);
    if !text.status.success() {
        let json = recheck_llvm_json(source, bundle, id);
        assert_eq!(json.status.code(), text.status.code());
        assert!(json.stdout.is_empty());
        assert_eq!(json.stderr, text.stderr);
    }
    text
}

fn recheck_llvm_json(source: &Path, bundle: &Path, id: &str) -> Output {
    consume("recheck-llvm --json", source, bundle, id)
}

fn consume(action: &str, source: &Path, bundle: &Path, id: &str) -> Output {
    let mut args = vec!["native-bundle"];
    args.extend(action.split_whitespace());
    args.extend([
        bundle.to_str().unwrap(),
        source.to_str().unwrap(),
        "--expected-build-id",
        id,
    ]);
    cli(&args)
}

fn repin(bundle: &Path, source: Option<&str>) -> String {
    let mut record: native_bundle::NativeBundleRecord =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    if let Some(source) = source {
        use mumei_core::{proof_cert, verification::ModuleEnv};
        let (binding, atom) = native_bundle::prepare_source(source).unwrap();
        let mut env = ModuleEnv::default();
        env.register_atom(&atom);
        let results = std::collections::HashMap::from([(
            "main".into(),
            (
                proof_cert::status::Z3_UNSAT.into(),
                proof_cert::status::VERIFIED.into(),
            ),
        )]);
        let cert = proof_cert::generate_certificate(
            native_bundle::SOURCE_LABEL,
            &[&atom],
            &results,
            &env,
            None,
            None,
            None,
        );
        fs::write(
            bundle.join("proof-cert.json"),
            serde_json::to_vec(&cert).unwrap(),
        )
        .unwrap();
        record.source = binding;
    }
    for role in BundleFile::ALL {
        record.files.insert(
            role,
            native_bundle::ContentIdentity::of(&fs::read(bundle.join(role.file_name())).unwrap()),
        );
    }
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    native_bundle::sha256(&bytes)
}

#[test]
fn clean_consumer_rechecks_copied_read_only_snapshots_without_producer_tools_or_execution() {
    use std::os::unix::fs::PermissionsExt;
    let producer = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(producer.path());
    let consumer = tempfile::tempdir().unwrap();
    let saved = save_evidence(consumer.path(), &source, &bundle, &id);
    fs::set_permissions(&saved, fs::Permissions::from_mode(0o400)).unwrap();
    let executable = consumer.path().join("mumei");
    fs::copy(env!("CARGO_BIN_EXE_mumei"), &executable).unwrap();
    let copied_source = consumer.path().join("source.mm");
    fs::copy(source, &copied_source).unwrap();
    let copied_bundle = consumer.path().join("bundle");
    fs::create_dir(&copied_bundle).unwrap();
    for name in
        std::iter::once(RECORD_FILE).chain(BundleFile::ALL.iter().map(|role| role.file_name()))
    {
        fs::copy(bundle.join(name), copied_bundle.join(name)).unwrap();
        fs::set_permissions(copied_bundle.join(name), fs::Permissions::from_mode(0o400)).unwrap();
    }
    fs::set_permissions(&copied_source, fs::Permissions::from_mode(0o400)).unwrap();
    fs::set_permissions(&copied_bundle, fs::Permissions::from_mode(0o500)).unwrap();
    producer.close().unwrap();
    fs::write(
        consumer.path().join("mumei.toml"),
        "not valid project configuration [",
    )
    .unwrap();
    let home = consumer.path().join("home");
    fs::create_dir(&home).unwrap();
    for (action, json) in [
        ("recheck-source", false),
        ("recheck-llvm", false),
        ("recheck-llvm", true),
        ("recheck-evidence", false),
    ] {
        let mut command = Command::new(&executable);
        command
            .env_clear()
            .current_dir(consumer.path())
            .env("HOME", &home)
            .env("PATH", "/nonexistent-consumer-tools");
        if let Some(path) = std::env::var_os("LD_LIBRARY_PATH") {
            command.env("LD_LIBRARY_PATH", path);
        }
        let result = command
            .args([
                "native-bundle",
                action,
                copied_bundle.to_str().unwrap(),
                copied_source.to_str().unwrap(),
                "--expected-build-id",
                &id,
            ])
            .args((action == "recheck-evidence").then_some(saved.to_str().unwrap()))
            .args(json.then_some("--json"))
            .output()
            .unwrap();
        assert_success(&result);
        let stdout = String::from_utf8_lossy(&result.stdout);
        if action == "recheck-source" {
            assert!(stdout.contains(native_bundle::RECHECK_SCOPE));
            assert!(stdout.contains(native_bundle::RECHECK_PROFILE));
        } else if json {
            let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
            assert_eq!(report["acceptance"], "not_evaluated");
            assert_eq!(report["evidence"].as_array().unwrap().len(), 3);
        } else if action == "recheck-evidence" {
            assert!(stdout.contains("Saved native Evidence consistent with fresh"));
            assert!(stdout.contains("Evidence acceptance: not evaluated"));
        } else {
            assert!(stdout.contains(mumei_emit_llvm::translation_validation::NATIVE_PROFILE));
            assert!(stdout.contains("source_postcondition_identity_and_llvm_snapshot_only"));
            assert!(stdout.contains("OS exit-status/executable semantics: not checked"));
        }
        assert!(!stdout.contains("artifact_verified"));
        assert_eq!(fs::read_dir(&copied_bundle).unwrap().count(), 5);
        assert_eq!(fs::read_dir(&home).unwrap().count(), 0);
        assert_eq!(fs::read_dir(consumer.path()).unwrap().count(), 6);
    }
    fs::set_permissions(&copied_bundle, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn self_consistent_forged_source_certificate_is_not_a_source_proof() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, _) = build(root.path());
    for text in [
        GOOD.replace("== 7", "== 8"),
        GOOD.replace("== 7", "== 1234567890123456"),
        GOOD.replace("result == 7", "false"),
    ] {
        fs::write(&source, &text).unwrap();
        let id = repin(&bundle, Some(&text));
        assert_success(&check(&source, &bundle, &id));
        for action in ["recheck-source", "recheck-llvm", "recheck-llvm --json"] {
            let result = consume(action, &source, &bundle, &id);
            assert_eq!(result.status.code(), Some(1));
            assert!(result.stdout.is_empty());
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert!(stderr.contains("source postcondition refuted"));
            assert!(!stderr.contains("1234567890123456"));
            assert!(!String::from_utf8_lossy(&result.stdout).contains("1234567890123456"));
        }
    }
}

#[test]
fn source_recheck_rejects_missing_tampered_and_unpinned_members_before_success() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    for name in
        std::iter::once(RECORD_FILE).chain(BundleFile::ALL.iter().map(|role| role.file_name()))
    {
        let path = bundle.join(name);
        let bytes = fs::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert!(!recheck(&source, &bundle, &id).status.success());
        assert!(!recheck_llvm(&source, &bundle, &id).status.success());
        fs::write(&path, b"tampered").unwrap();
        assert!(!recheck(&source, &bundle, &id).status.success());
        assert!(!recheck_llvm(&source, &bundle, &id).status.success());
        fs::write(path, bytes).unwrap();
    }
    assert!(!recheck(&source, &bundle, &"0".repeat(64)).status.success());
    assert!(!recheck_llvm(&source, &bundle, &"0".repeat(64))
        .status
        .success());
    fs::write(&source, format!("{GOOD}\n// stale source")).unwrap();
    assert!(!recheck(&source, &bundle, &id).status.success());
    assert!(!recheck_llvm(&source, &bundle, &id).status.success());
}

#[test]
fn authorized_non_executable_bytes_are_not_promoted_to_binary_assurance() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, old_id) = build(root.path());
    let marker = root.path().join("must-not-run");
    fs::write(
        bundle.join("program"),
        format!("#!/bin/sh\ntouch '{}'\nexit 99\n", marker.display()),
    )
    .unwrap();
    assert!(!recheck(&source, &bundle, &old_id).status.success());
    let id = repin(&bundle, None);
    let result = recheck(&source, &bundle, &id);
    assert_success(&result);
    assert!(String::from_utf8_lossy(&result.stdout).contains("executable semantics: not checked"));
    fs::write(bundle.join("program.o"), b"repinned non-object bytes").unwrap();
    let id = repin(&bundle, None);
    let result = recheck_llvm(&source, &bundle, &id);
    assert_success(&result);
    assert!(String::from_utf8_lossy(&result.stdout).contains("executable semantics: not checked"));
    assert!(!marker.exists());
}

#[test]
fn canonical_link_arguments_are_pinned_and_reproduced() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let mut record: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    // New records pin the canonical argument vector — reproducible by a
    // recheck on fresh staging paths, unlike the raw argv.
    let canonical = record["link_build"]["canonical_arguments_sha256"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(canonical.len(), 64);
    assert!(canonical.chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(
        canonical,
        record["link_build"]["arguments_sha256"].as_str().unwrap()
    );
    let artifact = |source: &Path, bundle: &Path, id: &str| {
        Command::new(env!("CARGO_BIN_EXE_mumei"))
            .args([
                "native-bundle",
                "recheck-artifact",
                bundle.to_str().unwrap(),
                source.to_str().unwrap(),
                "--expected-build-id",
                id,
            ])
            .output()
            .unwrap()
    };
    assert_success(&artifact(&source, &bundle, &id));
    // …and the acceptance descriptor claims the comparison that ran.
    let output = consume("accept-artifact --json", &source, &bundle, &id);
    assert_success(&output);
    let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(descriptor["checks_performed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry == "canonical_link_argument_vector_reproduction"));

    // A record pinned before the field existed carries no pin — the
    // recheck skips the comparison honestly rather than failing, and the
    // acceptance descriptor names the check among the ones not performed
    // rather than claiming it.
    record["link_build"]
        .as_object_mut()
        .unwrap()
        .remove("canonical_arguments_sha256");
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    let unpinned_id = native_bundle::sha256(&bytes);
    assert_success(&artifact(&source, &bundle, &unpinned_id));
    let output = consume("accept-artifact --json", &source, &bundle, &unpinned_id);
    assert_success(&output);
    let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(!descriptor["checks_performed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry == "canonical_link_argument_vector_reproduction"));
    assert!(descriptor["checks_not_performed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry == "canonical_link_argument_vector_reproduction"));

    // A wrong pin fails the reproduction check.
    record["link_build"]["canonical_arguments_sha256"] = serde_json::json!("0".repeat(64));
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!artifact(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn exit_status_is_pinned_and_reproduced() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let mut record: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    // New records pin the executable's observed exit status — `main`
    // returns 7 here — so a recheck that executes the byte-identical
    // reproduction compares against the pin.
    assert_eq!(record["link_build"]["expected_exit_status"], 7);
    let exit_status = |source: &Path, bundle: &Path, id: &str| {
        Command::new(env!("CARGO_BIN_EXE_mumei"))
            .args([
                "native-bundle",
                "recheck-exit-status",
                bundle.to_str().unwrap(),
                source.to_str().unwrap(),
                "--expected-build-id",
                id,
            ])
            .output()
            .unwrap()
    };
    assert_success(&exit_status(&source, &bundle, &id));

    // A record pinned before the field existed carries no pin — the
    // recheck cannot evaluate the comparison and reports inconclusive.
    record["link_build"]
        .as_object_mut()
        .unwrap()
        .remove("expected_exit_status");
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert_eq!(
        exit_status(&source, &bundle, &native_bundle::sha256(&bytes))
            .status
            .code(),
        Some(3)
    );

    // A wrong pin fails the reproduction check.
    record["link_build"]["expected_exit_status"] = serde_json::json!(0);
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(
        !exit_status(&source, &bundle, &native_bundle::sha256(&bytes))
            .status
            .success()
    );
}

#[test]
fn exit_status_acceptance_emits_the_observation_descriptor() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let accept = cli(&[
        "native-bundle",
        "accept-exit-status",
        bundle.to_str().unwrap(),
        source.to_str().unwrap(),
        "--expected-build-id",
        &id,
        "--json",
    ]);
    assert_success(&accept);
    let descriptor: serde_json::Value = serde_json::from_slice(&accept.stdout).unwrap();
    assert_eq!(descriptor["accepted"], true);
    assert!(
        descriptor["checks_performed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "os_exit_status_observation"),
        "exit-status acceptance must claim the OS observation"
    );
    assert!(
        descriptor["checks_not_performed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "historical_execution"),
        "one run stays an observation, never a history"
    );
    assert!(
        !descriptor["checks_performed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "abi_runtime_semantics"),
        "no ABI/runtime claim"
    );

    // A pinless record cannot be accepted this way — inconclusive.
    let mut record: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    record["link_build"]
        .as_object_mut()
        .unwrap()
        .remove("expected_exit_status");
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    let pinless = cli(&[
        "native-bundle",
        "accept-exit-status",
        bundle.to_str().unwrap(),
        source.to_str().unwrap(),
        "--expected-build-id",
        &native_bundle::sha256(&bytes),
    ]);
    assert_eq!(pinless.status.code(), Some(3));
}

#[test]
fn stdout_is_pinned_and_reproduced() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let mut record: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    // New records pin the executable's observed stdout digest — the
    // profile's `main` is silent, so the pin is the empty-stream digest.
    assert_eq!(
        record["link_build"]["expected_stdout_sha256"],
        native_bundle::sha256(b"")
    );
    let stdout_recheck = |source: &Path, bundle: &Path, id: &str| {
        Command::new(env!("CARGO_BIN_EXE_mumei"))
            .args([
                "native-bundle",
                "recheck-stdout",
                bundle.to_str().unwrap(),
                source.to_str().unwrap(),
                "--expected-build-id",
                id,
            ])
            .output()
            .unwrap()
    };
    assert_success(&stdout_recheck(&source, &bundle, &id));

    // A record pinned before the field existed carries no pin — the
    // recheck cannot evaluate the comparison and reports inconclusive.
    record["link_build"]
        .as_object_mut()
        .unwrap()
        .remove("expected_stdout_sha256");
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert_eq!(
        stdout_recheck(&source, &bundle, &native_bundle::sha256(&bytes))
            .status
            .code(),
        Some(3)
    );

    // A wrong pin fails the reproduction check.
    record["link_build"]["expected_stdout_sha256"] = serde_json::json!("0".repeat(64));
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(
        !stdout_recheck(&source, &bundle, &native_bundle::sha256(&bytes))
            .status
            .success()
    );
}

#[test]
fn stdout_acceptance_emits_the_observation_descriptor() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let accept = cli(&[
        "native-bundle",
        "accept-stdout",
        bundle.to_str().unwrap(),
        source.to_str().unwrap(),
        "--expected-build-id",
        &id,
        "--json",
    ]);
    assert_success(&accept);
    let descriptor: serde_json::Value = serde_json::from_slice(&accept.stdout).unwrap();
    assert_eq!(descriptor["accepted"], true);
    assert!(
        descriptor["checks_performed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "os_stdout_observation"),
        "stdout acceptance must claim the OS observation"
    );
    assert!(
        descriptor["checks_not_performed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "os_exit_status"),
        "a stdout run does not compare the exit status"
    );
    assert!(
        !descriptor["checks_performed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "abi_runtime_semantics"),
        "no ABI/runtime claim"
    );

    // A pinless record cannot be accepted this way — inconclusive.
    let mut record: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    record["link_build"]
        .as_object_mut()
        .unwrap()
        .remove("expected_stdout_sha256");
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    let pinless = cli(&[
        "native-bundle",
        "accept-stdout",
        bundle.to_str().unwrap(),
        source.to_str().unwrap(),
        "--expected-build-id",
        &native_bundle::sha256(&bytes),
    ]);
    assert_eq!(pinless.status.code(), Some(3));
}

#[test]
fn stderr_is_pinned_and_reproduced() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let mut record: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    // New records pin the executable's observed stderr digest — the
    // profile's `main` writes nothing to either stream, so the pin is
    // the empty-stream digest.
    assert_eq!(
        record["link_build"]["expected_stderr_sha256"],
        native_bundle::sha256(b"")
    );
    let stderr_recheck = |source: &Path, bundle: &Path, id: &str| {
        Command::new(env!("CARGO_BIN_EXE_mumei"))
            .args([
                "native-bundle",
                "recheck-stderr",
                bundle.to_str().unwrap(),
                source.to_str().unwrap(),
                "--expected-build-id",
                id,
            ])
            .output()
            .unwrap()
    };
    assert_success(&stderr_recheck(&source, &bundle, &id));

    // A record pinned before the field existed carries no pin — the
    // recheck cannot evaluate the comparison and reports inconclusive.
    record["link_build"]
        .as_object_mut()
        .unwrap()
        .remove("expected_stderr_sha256");
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert_eq!(
        stderr_recheck(&source, &bundle, &native_bundle::sha256(&bytes))
            .status
            .code(),
        Some(3)
    );

    // A wrong pin fails the reproduction check.
    record["link_build"]["expected_stderr_sha256"] = serde_json::json!("0".repeat(64));
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(
        !stderr_recheck(&source, &bundle, &native_bundle::sha256(&bytes))
            .status
            .success()
    );
}

#[test]
fn stderr_acceptance_emits_the_observation_descriptor() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let accept = cli(&[
        "native-bundle",
        "accept-stderr",
        bundle.to_str().unwrap(),
        source.to_str().unwrap(),
        "--expected-build-id",
        &id,
        "--json",
    ]);
    assert_success(&accept);
    let descriptor: serde_json::Value = serde_json::from_slice(&accept.stdout).unwrap();
    assert_eq!(descriptor["accepted"], true);
    assert!(
        descriptor["checks_performed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "os_stderr_observation"),
        "stderr acceptance must claim the OS observation"
    );
    for unclaimed in ["os_exit_status", "os_stdout_observation"] {
        assert!(
            descriptor["checks_not_performed"]
                .as_array()
                .unwrap()
                .iter()
                .any(|check| check == unclaimed),
            "a stderr run does not claim {unclaimed}"
        );
    }
    assert!(
        !descriptor["checks_performed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "abi_runtime_semantics"),
        "no ABI/runtime claim"
    );

    // A pinless record cannot be accepted this way — inconclusive.
    let mut record: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    record["link_build"]
        .as_object_mut()
        .unwrap()
        .remove("expected_stderr_sha256");
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    let pinless = cli(&[
        "native-bundle",
        "accept-stderr",
        bundle.to_str().unwrap(),
        source.to_str().unwrap(),
        "--expected-build-id",
        &native_bundle::sha256(&bytes),
    ]);
    assert_eq!(pinless.status.code(), Some(3));
}

#[test]
fn bounded_source_and_symlinks_cannot_bypass_the_consumer() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    for (text, diagnostic) in [
        ("x".repeat(65_537), "unsupported component type or size"),
        (
            GOOD.replace(
                "{ 7 }",
                &format!("{{ {}7{} }}", "(".repeat(49), ")".repeat(49)),
            ),
            "source outside bounded C03 recheck profile",
        ),
        (
            GOOD.replace(
                "result == 7",
                &std::iter::repeat_n("result == 7", 100)
                    .collect::<Vec<_>>()
                    .join(" || "),
            ),
            "source outside bounded C03 recheck profile",
        ),
    ] {
        fs::write(&source, text).unwrap();
        let result = recheck(&source, &bundle, &id);
        assert_eq!(result.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&result.stderr).contains(diagnostic));
        assert_eq!(recheck_llvm(&source, &bundle, &id).status.code(), Some(1));
    }
    fs::write(&source, GOOD).unwrap();
    let object = bundle.join("program.o");
    fs::remove_file(&object).unwrap();
    std::os::unix::fs::symlink(&source, &object).unwrap();
    assert!(!recheck(&source, &bundle, &id).status.success());
    assert!(!recheck_llvm(&source, &bundle, &id).status.success());
}

#[test]
fn vacuous_preconditions_and_unsupported_sources_cannot_recheck_successfully() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    for text in [
        GOOD.replace("requires: true", "requires: false")
            .replace("result == 7", "false"),
        GOOD.replace("main()", "main(x: i64)"),
        GOOD.replace("main()", "other()"),
    ] {
        fs::write(&source, text).unwrap();
        let result = recheck(&source, &bundle, &id);
        assert_eq!(result.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&result.stderr)
            .contains("zero-argument main with literal true precondition"));
        assert_eq!(recheck_llvm(&source, &bundle, &id).status.code(), Some(1));
    }
    let fake = "FAKE_C10_SOURCE_DIAGNOSTIC_ONLY";
    for text in [
        GOOD.replace("{ 7 }", "{ 7 / 1 }"),
        GOOD.replace("{ 7 }", "{ 7.0 }"),
        GOOD.replace("{ 7 }", &format!("{{ {fake} }}")),
    ] {
        fs::write(&source, text).unwrap();
        let result = recheck(&source, &bundle, &id);
        assert_eq!(result.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&result.stderr)
            .contains("source outside bounded C03 recheck profile"));
        assert!(!String::from_utf8_lossy(&result.stderr).contains(fake));
        assert!(!String::from_utf8_lossy(&result.stdout).contains(fake));
        let result = recheck_llvm(&source, &bundle, &id);
        assert_eq!(result.status.code(), Some(1));
        assert!(!String::from_utf8_lossy(&result.stderr).contains(fake));
        assert!(!String::from_utf8_lossy(&result.stdout).contains(fake));
    }
}

#[test]
fn ambient_orchestration_settings_and_fake_secrets_do_not_enter_the_recheck() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, _) = build(root.path());
    let fake = "FAKE_C10_SECRET_ONLY";
    let text = format!("{GOOD}\n// {fake}");
    fs::write(&source, &text).unwrap();
    let id = repin(&bundle, Some(&text));
    for (action, json) in [
        ("recheck-source", false),
        ("recheck-llvm", false),
        ("recheck-llvm", true),
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_mumei"))
            .args([
                "native-bundle",
                action,
                bundle.to_str().unwrap(),
                source.to_str().unwrap(),
                "--expected-build-id",
                &id,
            ])
            .args(json.then_some("--json"))
            .env("MUMEI_VERIFICATION_TIMEOUT_MS", "1")
            .env("MUMEI_TASK_ID", fake)
            .env("MUMEI_CANCEL_REASON", fake)
            .env("MUMEI_SOLVER_CONFIG_FINGERPRINT", fake)
            .env("MUMEI_HARNESS_CONTRACT", fake)
            .env("MUMEI_ARTIFACT_PATHS", fake)
            .env("OPENAI_API_KEY", fake)
            .output()
            .unwrap();
        assert_success(&result);
        assert!(!String::from_utf8_lossy(&result.stdout).contains(fake));
        assert!(!String::from_utf8_lossy(&result.stderr).contains(fake));
    }
}

#[test]
fn build_binds_actual_native_output_and_source_certificate_without_proving_artifact() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let result = check(&source, &bundle, &id);
    assert_success(&result);
    assert_success(&recheck_llvm(&source, &bundle, &id));
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("identity consistent"));
    assert!(stdout.contains(native_bundle::SCOPE));
    let record: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    // Single-atom profile: main cannot call, so the dependency map is the
    // one-entry `{"main": []}` — same canonical-JSON pin convention as the
    // module family.
    assert_eq!(
        record["source"]["source_dependencies_sha256"],
        native_bundle::sha256(&serde_json::to_vec(&serde_json::json!({"main": []})).unwrap())
    );
    assert!(record["object_build"]["target_triple"]
        .as_str()
        .unwrap()
        .starts_with("x86_64-"));
    let ir = fs::read_to_string(bundle.join(BundleFile::LlvmIr.file_name())).unwrap();
    assert!(ir.contains("target triple ="));
    assert!(ir.contains("target datalayout ="));
    assert_eq!(
        Command::new(bundle.join("program"))
            .status()
            .unwrap()
            .code(),
        Some(7)
    );
    assert_success(&cli(&[
        "verify-cert",
        bundle.join("proof-cert.json").to_str().unwrap(),
        source.to_str().unwrap(),
    ]));
    assert_eq!(fs::read_dir(&bundle).unwrap().count(), 5);
    fs::write(
        &source,
        GOOD.replace("{ 7 }", "{ if 1 < 2 { 7 } else { 8 } }"),
    )
    .unwrap();
    let conditional = root.path().join("conditional");
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        conditional.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(conditional.join(RECORD_FILE)).unwrap());
    assert_success(&check(&source, &conditional, &id));
    assert_success(&recheck_llvm(&source, &conditional, &id));
}

#[test]
fn consumer_rejects_missing_replaced_stale_source_settings_and_unpinned_records() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    for role in BundleFile::ALL {
        let path = bundle.join(role.file_name());
        let original = fs::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert!(!check(&source, &bundle, &id).status.success());
        fs::write(&path, b"replaced").unwrap();
        assert!(!check(&source, &bundle, &id).status.success());
        fs::write(path, original).unwrap();
    }
    for source_text in [
        GOOD.replace("{ 7 }", "{ 8 }"),
        GOOD.replace("== 7", "== 8"),
        format!("{GOOD}\n// changed source"),
    ] {
        fs::write(&source, source_text).unwrap();
        assert!(!check(&source, &bundle, &id).status.success());
    }
    fs::write(&source, GOOD).unwrap();
    assert!(!check(&source, &bundle, &"0".repeat(64)).status.success());
    let record_path = bundle.join(RECORD_FILE);
    let original = fs::read(&record_path).unwrap();
    for pointer in [
        "/settings/relocation",
        "/object_build/target_triple",
        "/source/compiler_version",
        "/link_build/driver_version",
        "/link_build/canonical_arguments_sha256",
        "/scope",
    ] {
        let mut record: serde_json::Value = serde_json::from_slice(&original).unwrap();
        *record.pointer_mut(pointer).unwrap() = serde_json::json!("changed");
        fs::write(&record_path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(!check(&source, &bundle, &id).status.success());
        let new_id = native_bundle::sha256(&fs::read(&record_path).unwrap());
        assert!(!check(&source, &bundle, &new_id).status.success());
    }
    fs::write(record_path, original).unwrap();
    assert_success(&check(&source, &bundle, &id));
}

#[test]
fn producer_refuses_unsupported_or_false_source_and_never_clobbers_outputs() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.mm");
    let bundle = root.path().join("bundle");
    for text in [
        GOOD.replace("== 7", "== 8"),
        GOOD.replace("true;", "false;"),
        GOOD.replace("{ 7 }", "{ 7 + 1 }"),
        GOOD.replace("main()", "main(x: i64)"),
        format!("trusted {GOOD}"),
        GOOD.replace("-> i64", "-> f64"),
    ] {
        fs::write(&source, text).unwrap();
        assert!(!cli(&[
            "native-bundle",
            "build",
            source.to_str().unwrap(),
            "-o",
            bundle.to_str().unwrap()
        ])
        .status
        .success());
        assert!(!bundle.exists());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
    let (source, bundle, id) = build(root.path());
    assert!(!cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap()
    ])
    .status
    .success());
    assert_success(&check(&source, &bundle, &id));
}

#[test]
fn untrusted_metadata_and_symlinks_do_not_select_files_or_echo_secrets() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let path = bundle.join(RECORD_FILE);
    let original = fs::read(&path).unwrap();
    let fake = "FAKE_SECRET_NATIVE_BUNDLE_ONLY";
    let mut record: serde_json::Value = serde_json::from_slice(&original).unwrap();
    record[fake] = serde_json::json!("../../private");
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(&path, &bytes).unwrap();
    let output = check(&source, &bundle, &native_bundle::sha256(&bytes));
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).contains(fake));
    fs::write(&path, original).unwrap();
    let object = bundle.join("program.o");
    fs::remove_file(&object).unwrap();
    std::os::unix::fs::symlink(&source, &object).unwrap();
    assert!(!check(&source, &bundle, &id).status.success());
    fs::remove_file(&object).unwrap();
    std::os::unix::fs::symlink(&source, &object).unwrap();
    assert!(!check(&source, &bundle, fake).status.success());
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    let other = root.path().join("other");
    fs::write(&source, format!("{GOOD}\n// {fake}")).unwrap();
    let output = command
        .args([
            "native-bundle",
            "build",
            source.to_str().unwrap(),
            "-o",
            other.to_str().unwrap(),
        ])
        .env("MUMEI_HARNESS_CONTRACT", fake)
        .env("MUMEI_ARTIFACT_PATHS", fake)
        .env("OPENAI_API_KEY", fake)
        .output()
        .unwrap();
    assert_success(&output);
    assert!(!String::from_utf8_lossy(&output.stdout).contains(fake));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(fake));
    for role in BundleFile::ALL {
        assert!(!fs::read(other.join(role.file_name()))
            .unwrap()
            .windows(fake.len())
            .any(|bytes| bytes == fake.as_bytes()));
    }
    assert!(!fs::read_to_string(other.join("proof-cert.json"))
        .unwrap()
        .contains(fake));
}

#[test]
fn a_rebound_record_is_not_evidence_of_semantic_correspondence() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let other = root.path().join("wrong-bundle");
    let wrong_source = root.path().join("wrong.mm");
    fs::write(&wrong_source, GOOD.replace('7', "8")).unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        wrong_source.to_str().unwrap(),
        "-o",
        other.to_str().unwrap(),
    ]));
    let wrong_artifact = fs::read(other.join("program")).unwrap();
    fs::write(bundle.join("program"), &wrong_artifact).unwrap();
    assert!(!check(&source, &bundle, &id).status.success());
    let mut record: native_bundle::NativeBundleRecord =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    record.files.insert(
        BundleFile::Executable,
        native_bundle::ContentIdentity::of(&wrong_artifact),
    );
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &id).status.success());
    // Deliberately authorizing a new digest only establishes new byte identities.
    let repinned = check(&source, &bundle, &native_bundle::sha256(&bytes));
    assert_success(&repinned);
    assert!(String::from_utf8_lossy(&repinned.stdout).contains(native_bundle::SCOPE));
    assert_eq!(
        Command::new(bundle.join("program"))
            .status()
            .unwrap()
            .code(),
        Some(8)
    );
}

#[test]
fn unavailable_linker_leaves_no_partial_bundle() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.mm");
    let bundle = root.path().join("bundle");
    fs::write(&source, GOOD).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args([
            "native-bundle",
            "build",
            source.to_str().unwrap(),
            "-o",
            bundle.to_str().unwrap(),
        ])
        .env("PATH", "/nonexistent-native-bundle-test")
        .env("LLVM_SYS_170_PREFIX", "/nonexistent-native-bundle-test")
        .env("HOME", root.path())
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!bundle.exists());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn repinning_a_certificate_cannot_bypass_source_status_or_translator_gates() {
    use mumei_core::proof_cert;
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, _) = build(root.path());
    let record: native_bundle::NativeBundleRecord =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    let cert: proof_cert::ProofCertificate =
        serde_json::from_slice(&fs::read(bundle.join("proof-cert.json")).unwrap()).unwrap();
    for (status, result) in [
        ("trusted", "skipped"),
        ("verified", "lean_verified"),
        ("verified", "unknown"),
        ("verified", "sat"),
        ("verified", "unsat"),
    ] {
        let mut changed = cert.clone();
        changed.atoms[0].status = status.into();
        changed.atoms[0].z3_check_result = result.into();
        if result == "unsat" {
            changed.atoms[0].translator_version = "stale".into();
        }
        proof_cert::refresh_certificate_integrity(&mut changed);
        let bytes = serde_json::to_vec(&changed).unwrap();
        fs::write(bundle.join("proof-cert.json"), &bytes).unwrap();
        let mut changed_record = record.clone();
        changed_record.files.insert(
            BundleFile::Certificate,
            native_bundle::ContentIdentity::of(&bytes),
        );
        let record_bytes = serde_json::to_vec(&changed_record).unwrap();
        fs::write(bundle.join(RECORD_FILE), &record_bytes).unwrap();
        assert!(
            !check(&source, &bundle, &native_bundle::sha256(&record_bytes))
                .status
                .success()
        );
    }
}

#[test]
fn rehashed_certificate_targets_must_match_the_source_translator_identity() {
    use mumei_core::proof_cert;
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, _) = build(root.path());
    let record: native_bundle::NativeBundleRecord =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    let cert: proof_cert::ProofCertificate =
        serde_json::from_slice(&fs::read(bundle.join("proof-cert.json")).unwrap()).unwrap();
    for field in 0..7 {
        let mut changed = cert.clone();
        match field {
            0 => changed.atoms[0].translator_ir.theorem_goal = "False".into(),
            1 => changed.atoms[0].translator_ir.sort = "different".into(),
            2 => changed.atoms[0].body_expr = "{ 8 }".into(),
            3 => changed.atoms[0].ensures = "result == 8".into(),
            4 => changed.atoms[0].requires = "false".into(),
            5 => changed.atoms[0].assumed_clauses.push("ensures".into()),
            _ => changed.atoms[0].effects.push("IO".into()),
        }
        proof_cert::refresh_certificate_integrity(&mut changed);
        let bytes = serde_json::to_vec(&changed).unwrap();
        fs::write(bundle.join("proof-cert.json"), &bytes).unwrap();
        let mut changed_record = record.clone();
        changed_record.files.insert(
            BundleFile::Certificate,
            native_bundle::ContentIdentity::of(&bytes),
        );
        let record_bytes = serde_json::to_vec(&changed_record).unwrap();
        fs::write(bundle.join(RECORD_FILE), &record_bytes).unwrap();
        assert!(
            !check(&source, &bundle, &native_bundle::sha256(&record_bytes))
                .status
                .success(),
            "field {field}"
        );
    }
}

#[test]
fn repinned_wrong_llvm_is_not_a_translation_even_with_valid_source_and_certificate() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, _) = build(root.path());
    let path = bundle.join("program.ll");
    let original = fs::read_to_string(&path).unwrap();
    for value in ["8", "4294967303", "1234567890123456"] {
        let changed = original.replace("ret i64 7", &format!("ret i64 {value}"));
        assert_ne!(changed, original);
        fs::write(&path, changed).unwrap();
        let id = repin(&bundle, None);
        assert_success(&check(&source, &bundle, &id));
        assert_success(&recheck(&source, &bundle, &id));
        let result = recheck_llvm(&source, &bundle, &id);
        assert_eq!(result.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&result.stderr).contains("LLVM body differs from source"));
        assert!(!String::from_utf8_lossy(&result.stderr).contains(value));
    }
    fs::write(&path, original).unwrap();
    let changed = GOOD.replace("7", "8");
    fs::write(&source, &changed).unwrap();
    let id = repin(&bundle, Some(&changed));
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_eq!(recheck_llvm(&source, &bundle, &id).status.code(), Some(1));
}

#[test]
fn repinned_wrapper_mutations_and_private_malformed_ir_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, _) = build(root.path());
    let path = bundle.join("program.ll");
    let original = fs::read_to_string(&path).unwrap();
    let fake = "FAKE_PRIVATE_NATIVE_LLVM_ONLY";
    let mutations = [
        original.replace("ret i32 %exit_code", "ret i32 7"),
        original.replace("trunc i64 %result", "trunc i64 7"),
        original.replace("define i32 @main", "define i32 @other"),
        original.replace(
            "call i64 @__mumei_user_main()",
            "call i64 @__mumei_user_main() [ \"custom\"() ]",
        ),
        original.replace(
            "call i64 @__mumei_user_main()",
            "call i64 @__mumei_user_main() [ \"deopt\"() ]",
        ),
        format!("{original}\ndeclare void @other()"),
        format!("{original}\n@global = global i64 7"),
        original.replace("e-m:e-", "E-m:e-"),
        format!("{fake}\n{original}"),
        format!("{original};{}", "X".repeat(65_536)),
    ];
    for changed in mutations {
        assert_ne!(changed, original);
        fs::write(&path, changed).unwrap();
        let id = repin(&bundle, None);
        assert_success(&recheck(&source, &bundle, &id));
        let result = recheck_llvm(&source, &bundle, &id);
        assert_eq!(result.status.code(), Some(1));
        assert!(!String::from_utf8_lossy(&result.stdout).contains(fake));
        assert!(!String::from_utf8_lossy(&result.stderr).contains(fake));
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("outside bounded translation profile")
        );
    }
    fs::write(&path, [0xff, 0xfe]).unwrap();
    let id = repin(&bundle, None);
    assert_eq!(recheck_llvm(&source, &bundle, &id).status.code(), Some(1));
}

#[test]
fn native_ir_observations_must_match_the_snapshot_without_normalization() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, _) = build(root.path());
    let path = bundle.join(RECORD_FILE);
    let original: native_bundle::NativeBundleRecord =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for change_layout in [false, true] {
        let mut record = original.clone();
        if change_layout {
            record.object_build.data_layout = "e-i64:64".into();
        } else {
            record.object_build.target_triple =
                if record.object_build.target_triple == "x86_64-pc-linux-gnu" {
                    "x86_64-unknown-linux-gnu".into()
                } else {
                    "x86_64-pc-linux-gnu".into()
                };
        }
        let bytes = serde_json::to_vec(&record).unwrap();
        fs::write(&path, &bytes).unwrap();
        let id = native_bundle::sha256(&bytes);
        assert_success(&check(&source, &bundle, &id));
        assert_success(&recheck(&source, &bundle, &id));
        let result = recheck_llvm(&source, &bundle, &id);
        assert_eq!(result.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&result.stderr)
            .contains("snapshot/toolchain observation mismatch"));
    }
}

#[test]
fn full_native_build_rechecks_large_i64_results_without_promising_an_os_exit_code() {
    let root = tempfile::tempdir().unwrap();
    for value in ["0", "2147483648", "4294967296", "9223372036854775807"] {
        let source = root.path().join("source.mm");
        let bundle = root.path().join(format!("bundle-{value}"));
        fs::write(&source, format!("atom main() -> i64 requires: true; ensures: result == {value}; body: {{ {value} }};")).unwrap();
        assert_success(&cli(&[
            "native-bundle",
            "build",
            source.to_str().unwrap(),
            "-o",
            bundle.to_str().unwrap(),
        ]));
        let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
        let result = recheck_llvm(&source, &bundle, &id);
        assert_success(&result);
        let stdout = String::from_utf8_lossy(&result.stdout);
        assert!(stdout.contains("low 32 bits of the i64 body"));
        assert!(stdout.contains("OS exit-status/executable semantics: not checked"));
        assert!(!stdout.contains("artifact_verified"));
        let json = recheck_llvm_json(&source, &bundle, &id);
        assert_success(&json);
        let report: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
        assert_eq!(report["acceptance"], "not_evaluated");
    }
}

fn json_evidence(source: &Path, bundle: &Path, id: &str) -> serde_json::Value {
    let result = recheck_llvm_json(source, bundle, id);
    assert_success(&result);
    assert!(result.stderr.is_empty());
    serde_json::from_slice(&result.stdout).unwrap()
}

fn normalized_evidence(mut report: serde_json::Value) -> serde_json::Value {
    report["build_id"] = serde_json::json!("0".repeat(64));
    for evidence in report["evidence"].as_array_mut().unwrap() {
        for value in [
            "/subject/sha256",
            "/specification/sha256",
            "/provenance/configuration_fingerprint",
        ] {
            let digest = evidence.pointer_mut(value).unwrap();
            assert!(native_bundle::is_digest(digest.as_str().unwrap()));
            *digest = serde_json::json!("0".repeat(64));
        }
        if !evidence["semantics"]["target_triple"].is_null() {
            evidence["semantics"]["target_triple"] = serde_json::json!("runtime-target");
        }
        assert_eq!(
            evidence["provenance"]["adapter"]["version"],
            env!("CARGO_PKG_VERSION")
        );
        assert!(!evidence["provenance"]["tool"]["version"]
            .as_str()
            .unwrap()
            .is_empty());
        evidence["provenance"]["adapter"]["version"] = serde_json::json!("build-version");
        evidence["provenance"]["tool"]["version"] = serde_json::json!("runtime-version");
    }
    report
}

#[test]
fn json_evidence_separates_actual_identities_obligations_and_schema_checked_shape() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let before: Vec<_> = fs::read_dir(&bundle)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            (path.clone(), fs::read(path).unwrap())
        })
        .collect();
    let report = json_evidence(&source, &bundle, &id);
    assert_eq!(report, json_evidence(&source, &bundle, &id));
    assert_eq!(report["build_id"], id);
    let records = report["evidence"].as_array().unwrap();
    let manifest: native_bundle::NativeBundleRecord =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    assert_eq!(
        records[0]["subject"]["sha256"],
        manifest.files[&BundleFile::Executable].sha256
    );
    assert_eq!(records[0]["specification"]["sha256"], id);
    assert_eq!(
        records[1]["subject"]["sha256"],
        native_bundle::sha256(&fs::read(&source).unwrap())
    );
    assert_eq!(
        records[1]["specification"]["sha256"],
        manifest.source.specification_sha256
    );
    assert_eq!(
        records[2]["subject"]["sha256"],
        native_bundle::sha256(&fs::read(bundle.join("program.ll")).unwrap())
    );
    assert_eq!(
        records[2]["specification"]["sha256"],
        records[1]["subject"]["sha256"]
    );
    for record in &records[1..] {
        let version = mumei_core::verification::linked_z3_version();
        assert_eq!(record["provenance"]["tool"]["name"], "Z3");
        assert_eq!(
            record["provenance"]["tool"]["version"],
            format!("{}.{}.{}.{}", version.0, version.1, version.2, version.3)
        );
    }
    assert_eq!(
        records[0]["provenance"]["configuration_fingerprint"],
        records[2]["provenance"]["configuration_fingerprint"]
    );
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "fixtures/verification-evidence/native_consumer_shape.json"
    ))
    .unwrap();
    assert_eq!(normalized_evidence(report), fixture);
    for (path, bytes) in before {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    assert_eq!(fs::read_dir(&bundle).unwrap().count(), 5);
}

#[test]
fn json_llvm_obligation_binds_the_body_not_just_shared_postconditions() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.mm");
    let mut reports = Vec::new();
    for value in [7, 8] {
        fs::write(
            &source,
            format!("atom main() -> i64 requires: true; ensures: true; body: {{ {value} }};"),
        )
        .unwrap();
        let bundle = root.path().join(format!("bundle-{value}"));
        assert_success(&cli(&[
            "native-bundle",
            "build",
            source.to_str().unwrap(),
            "-o",
            bundle.to_str().unwrap(),
        ]));
        let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
        reports.push(json_evidence(&source, &bundle, &id));
    }
    assert_eq!(
        reports[0]["evidence"][1]["specification"],
        reports[1]["evidence"][1]["specification"]
    );
    assert_ne!(
        reports[0]["evidence"][2]["specification"],
        reports[1]["evidence"][2]["specification"]
    );
}

#[test]
fn json_failures_emit_no_partial_report_or_private_diagnostics() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let rejected = |pin: &str| {
        let result = recheck_llvm_json(&source, &bundle, pin);
        assert_eq!(result.status.code(), Some(1));
        assert!(result.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&result.stderr).contains(root.path().to_str().unwrap()));
    };
    for pin in ["", "untrusted", &"0".repeat(64), &format!("{id}\n")] {
        rejected(pin);
    }
    for role in BundleFile::ALL {
        let path = bundle.join(role.file_name());
        let bytes = fs::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        rejected(&id);
        fs::write(&path, b"FAKE_PRIVATE_REPORT_COMPONENT").unwrap();
        rejected(&id);
        fs::write(&path, bytes).unwrap();
    }
    for text in [
        GOOD.replace("7", "8"),
        format!("{GOOD}//FAKE_PRIVATE_STALE_SOURCE"),
        "FAKE_PRIVATE_MALFORMED_SOURCE".to_owned(),
    ] {
        fs::write(&source, text).unwrap();
        rejected(&id);
    }
    fs::write(&source, GOOD).unwrap();
    let ir_path = bundle.join("program.ll");
    let original = fs::read_to_string(&ir_path).unwrap();
    for changed in [
        original.replace("ret i64 7", "ret i64 4294967303"),
        original.replace("ret i32 %exit_code", "ret i32 7"),
        original.replace(
            "call i64 @__mumei_user_main()",
            "call i64 @__mumei_user_main() [ \"custom\"() ]",
        ),
        format!("{original}\ndeclare void @extra()"),
        format!("FAKE_PRIVATE_MALFORMED_IR{original}"),
    ] {
        fs::write(&ir_path, changed).unwrap();
        rejected(&repin(&bundle, None));
    }
}

#[test]
fn json_repinned_non_executable_artifacts_remain_identity_only_and_are_not_run() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, _) = build(root.path());
    let marker = root.path().join("MUST_NOT_EXIST");
    fs::write(bundle.join("program.o"), b"not an object").unwrap();
    fs::write(
        bundle.join("program"),
        format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
    )
    .unwrap();
    fs::set_permissions(bundle.join("program"), fs::Permissions::from_mode(0o700)).unwrap();
    let id = repin(&bundle, None);
    let report = json_evidence(&source, &bundle, &id);
    assert!(!marker.exists());
    assert_eq!(report["acceptance"], "not_evaluated");
    assert_eq!(report["evidence"][0]["method"], "identity_check");
    assert_eq!(
        report["evidence"][0]["correspondence"]["status"],
        "unverified"
    );
    assert_eq!(
        report["evidence"][0]["subject"]["sha256"],
        native_bundle::sha256(&fs::read(bundle.join("program")).unwrap())
    );
    assert!(report["evidence"][2]["exclusions"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!(
            "os_exit_status_and_final_executable_semantics"
        )));
}

#[test]
fn json_uses_local_provenance_and_non_secret_configuration_only() {
    let root = tempfile::Builder::new()
        .prefix("FAKE_PRIVATE_CALLER_PATH")
        .tempdir()
        .unwrap();
    let (source, bundle, _) = build(root.path());
    let fake = "FAKE_PRIVATE_EVIDENCE_ONLY";
    let text = format!("{GOOD}//{fake}");
    fs::write(&source, &text).unwrap();
    repin(&bundle, Some(&text));
    let path = bundle.join(RECORD_FILE);
    let mut record: native_bundle::NativeBundleRecord =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    record.object_build.llvm_version = "17.98765.43210".into();
    record.link_build.driver_version = "98765.43210".into();
    record.link_build.linker_version = "98765.43210".into();
    record.link_build.driver_target = format!("x86_64-{fake}-linux-gnu");
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(&path, &bytes).unwrap();
    let id = native_bundle::sha256(&bytes);
    let baseline = recheck_llvm_json(&source, &bundle, &id);
    assert_success(&baseline);
    let result = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args([
            "native-bundle",
            "recheck-llvm",
            bundle.to_str().unwrap(),
            source.to_str().unwrap(),
            "--expected-build-id",
            &id,
            "--json",
        ])
        .env("MUMEI_VERIFICATION_TIMEOUT_MS", "1")
        .env("MUMEI_SOLVER_CONFIG_FINGERPRINT", fake)
        .env("MUMEI_ARTIFACT_PATHS", fake)
        .env("MUMEI_TASK_ID", fake)
        .env("OPENAI_API_KEY", fake)
        .output()
        .unwrap();
    assert_success(&result);
    assert_eq!(result.stdout, baseline.stdout);
    assert!(result.stderr.is_empty());
    let stdout = String::from_utf8(result.stdout).unwrap();
    for private in [
        fake,
        "98765.43210",
        "FAKE_PRIVATE_CALLER_PATH",
        GOOD,
        root.path().to_str().unwrap(),
    ] {
        assert!(!stdout.contains(private));
    }
    let evidence: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        evidence["evidence"][2]["semantics"]["target_triple"],
        record.object_build.target_triple
    );
    let saved = save_evidence(root.path(), &source, &bundle, &id);
    let replay = replay_command(&source, &bundle, &id, &saved)
        .env("MUMEI_VERIFICATION_TIMEOUT_MS", "1")
        .env("MUMEI_SOLVER_CONFIG_FINGERPRINT", fake)
        .env("OPENAI_API_KEY", fake)
        .output()
        .unwrap();
    assert_success(&replay);
    assert!(!String::from_utf8_lossy(&replay.stdout).contains(fake));
    assert!(!String::from_utf8_lossy(&replay.stderr).contains(fake));
}

fn save_evidence(root: &Path, source: &Path, bundle: &Path, id: &str) -> std::path::PathBuf {
    let path = root.join("evidence.json");
    let output = recheck_llvm_json(source, bundle, id);
    assert_success(&output);
    fs::write(&path, output.stdout).unwrap();
    path
}

fn replay_command(source: &Path, bundle: &Path, id: &str, saved: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command.args([
        "native-bundle",
        "recheck-evidence",
        bundle.to_str().unwrap(),
        source.to_str().unwrap(),
        saved.to_str().unwrap(),
        "--expected-build-id",
        id,
    ]);
    command
}

fn accept_command(source: &Path, bundle: &Path, id: &str, saved: &Path, json: bool) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command.args([
        "native-bundle",
        "accept-evidence",
        bundle.to_str().unwrap(),
        source.to_str().unwrap(),
        saved.to_str().unwrap(),
        "--expected-build-id",
        id,
    ]);
    if json {
        command.arg("--json");
    }
    command
}

fn accept_rejected(source: &Path, bundle: &Path, id: &str, saved: &Path) -> Output {
    let output = accept_command(source, bundle, id, saved, false)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    output
}

#[test]
fn accept_evidence_emits_anonymous_acceptance_descriptor_only_on_acceptance() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let saved = save_evidence(root.path(), &source, &bundle, &id);
    for json in [false, true] {
        let output = accept_command(&source, &bundle, &id, &saved, json)
            .output()
            .unwrap();
        assert_success(&output);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!stdout.contains("FAKE_PRIVATE"));
        assert!(!stdout.contains("main"));
        if json {
            let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                descriptor["schema_version"],
                "mumei.native-evidence-acceptance/v1"
            );
            assert_eq!(descriptor["acceptance"], "evaluated");
            assert_eq!(descriptor["accepted"], true);
            // The binding fingerprint is the sha256 of the accepted
            // record's bytes — a holder can prove which record was
            // evaluated without the descriptor echoing any claim.
            assert_eq!(
                descriptor["evidence_fingerprint"],
                native_bundle::sha256(&fs::read(&saved).unwrap())
            );
            for check in [
                "bundle_identity_binding",
                "fresh_same_profile_source_recheck",
                "llvm_snapshot_translation_recheck",
                "saved_evidence_consistency",
            ] {
                assert!(descriptor["checks_performed"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|entry| entry == check));
            }
            assert!(mumei_core::verification_evidence::decode(&output.stdout).is_err());
        } else {
            assert!(stdout.contains("accepted"));
        }
    }
}

#[test]
fn accept_evidence_rejects_tampered_foreign_and_mispinned_reports() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let saved = save_evidence(root.path(), &source, &bundle, &id);
    let original = fs::read(&saved).unwrap();
    // Tampered record.
    let mut changed: serde_json::Value = serde_json::from_slice(&original).unwrap();
    changed["evidence"][1]["method"] = serde_json::json!("finite_observation");
    fs::write(&saved, serde_json::to_vec(&changed).unwrap()).unwrap();
    accept_rejected(&source, &bundle, &id, &saved);
    // Foreign-family / foreign-schema envelopes.
    for foreign in [
        serde_json::json!({"schema_version": "mumei.native-module-consumer-evidence/v1", "evidence": []}),
        serde_json::json!({"schema_version": "mumei.verification-evidence/v1"}),
    ] {
        fs::write(&saved, serde_json::to_vec(&foreign).unwrap()).unwrap();
        accept_rejected(&source, &bundle, &id, &saved);
    }
    fs::write(&saved, &original).unwrap();
    // Wrong expected build id and missing source agreement.
    accept_rejected(&source, &bundle, &"0".repeat(64), &saved);
    assert_success(
        &accept_command(&source, &bundle, &id, &saved, true)
            .output()
            .unwrap(),
    );
}

#[test]
fn accept_evidence_accepts_module_family_reports() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), MODULE);
    let saved = save_evidence(root.path(), &source, &bundle, &id);
    let output = accept_command(&source, &bundle, &id, &saved, true)
        .output()
        .unwrap();
    assert_success(&output);
    let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        descriptor["schema_version"],
        "mumei.native-evidence-acceptance/v1"
    );
    assert_eq!(descriptor["accepted"], true);
}

fn replay_rejected(source: &Path, bundle: &Path, id: &str, saved: &Path) -> Output {
    let output = replay_command(source, bundle, id, saved).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("FAKE_PRIVATE"));
    assert!(!stderr.contains(saved.to_str().unwrap()));
    output
}

#[test]
fn saved_evidence_roundtrips_without_changes_and_ignores_json_object_order() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let saved = save_evidence(root.path(), &source, &bundle, &id);
    let before = fs::read(&saved).unwrap();
    let output = replay_command(&source, &bundle, &id, &saved)
        .output()
        .unwrap();
    assert_success(&output);
    assert!(output.stderr.is_empty());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Evidence acceptance: not evaluated"));
    assert_eq!(fs::read(&saved).unwrap(), before);
    let reordered: serde_json::Value = serde_json::from_slice(&before).unwrap();
    fs::write(&saved, serde_json::to_vec(&reordered).unwrap()).unwrap();
    assert_success(
        &replay_command(&source, &bundle, &id, &saved)
            .output()
            .unwrap(),
    );
    assert_eq!(fs::read_dir(&bundle).unwrap().count(), 5);
}

#[test]
fn common_representation_is_not_native_profile_or_fresh_query_acceptance() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let saved = save_evidence(root.path(), &source, &bundle, &id);
    let original: serde_json::Value = serde_json::from_slice(&fs::read(&saved).unwrap()).unwrap();
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "fixtures/verification-evidence/synthetic_cases.json"
    ))
    .unwrap();
    for case in cases["cases"].as_array().unwrap() {
        let record = &case["evidence"];
        mumei_core::verification_evidence::decode(&serde_json::to_vec(record).unwrap()).unwrap();
        let mut report = original.clone();
        report["evidence"][1] = record.clone();
        fs::write(&saved, serde_json::to_vec(&report).unwrap()).unwrap();
        replay_rejected(&source, &bundle, &id, &saved);
    }
    for metadata in [
        serde_json::Value::Null,
        serde_json::json!("synthetic-translator/v1"),
    ] {
        let mut report = original.clone();
        for record in report["evidence"].as_array_mut().unwrap() {
            record["provenance"]["translator_version"] = metadata.clone();
            record["provenance"]["bridge_lemma_hash"] = serde_json::Value::Null;
        }
        fs::write(&saved, serde_json::to_vec(&report).unwrap()).unwrap();
        if metadata.is_null() {
            assert_success(
                &replay_command(&source, &bundle, &id, &saved)
                    .output()
                    .unwrap(),
            );
        } else {
            replay_rejected(&source, &bundle, &id, &saved);
        }
    }
    let mut missing = original.clone();
    missing["evidence"][1]["semantics"]
        .as_object_mut()
        .unwrap()
        .remove("target_triple");
    fs::write(&saved, serde_json::to_vec(&missing).unwrap()).unwrap();
    let rejected = replay_rejected(&source, &bundle, &id, &saved);
    assert!(String::from_utf8_lossy(&rejected.stderr)
        .contains("invalid native evidence representation"));
}

fn json_paths(
    value: &serde_json::Value,
    prefix: String,
    leaves: &mut Vec<String>,
    objects: &mut Vec<String>,
) {
    match value {
        serde_json::Value::Object(fields) => {
            objects.push(prefix.clone());
            for (key, value) in fields {
                json_paths(value, format!("{prefix}/{key}"), leaves, objects);
            }
        }
        serde_json::Value::Array(values) if !values.is_empty() => {
            for (index, value) in values.iter().enumerate() {
                json_paths(value, format!("{prefix}/{index}"), leaves, objects);
            }
        }
        _ => leaves.push(prefix),
    }
}

#[test]
fn replay_checks_every_field_and_rejects_unknown_fields_at_every_object_level() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let saved = save_evidence(root.path(), &source, &bundle, &id);
    let original: serde_json::Value = serde_json::from_slice(&fs::read(&saved).unwrap()).unwrap();
    let (mut leaves, mut objects) = (Vec::new(), Vec::new());
    json_paths(&original, String::new(), &mut leaves, &mut objects);
    assert!(leaves.len() > 100);
    for path in leaves {
        let mut changed = original.clone();
        *changed.pointer_mut(&path).unwrap() = serde_json::json!("FAKE_PRIVATE_FIELD_MUTATION");
        fs::write(&saved, serde_json::to_vec(&changed).unwrap()).unwrap();
        replay_rejected(&source, &bundle, &id, &saved);
    }
    for path in &objects {
        let mut changed = original.clone();
        changed
            .pointer_mut(path)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(
                "FAKE_PRIVATE_UNKNOWN_FIELD".into(),
                serde_json::json!("not_a_trust_override"),
            );
        fs::write(&saved, serde_json::to_vec(&changed).unwrap()).unwrap();
        replay_rejected(&source, &bundle, &id, &saved);
    }
    for path in objects {
        let fields = original.pointer(&path).unwrap().as_object().unwrap();
        for key in fields.keys() {
            let mut changed = original.clone();
            changed
                .pointer_mut(&path)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(key);
            fs::write(&saved, serde_json::to_vec(&changed).unwrap()).unwrap();
            replay_rejected(&source, &bundle, &id, &saved);
        }
    }
    let mut digests = vec!["/build_id".to_owned()];
    for index in 0..3 {
        for field in [
            "subject/sha256",
            "specification/sha256",
            "provenance/configuration_fingerprint",
        ] {
            digests.push(format!("/evidence/{index}/{field}"));
        }
    }
    for path in digests {
        let mut changed = original.clone();
        let value = changed.pointer_mut(&path).unwrap();
        assert_ne!(value.as_str().unwrap(), "0".repeat(64));
        *value = serde_json::json!("0".repeat(64));
        fs::write(&saved, serde_json::to_vec(&changed).unwrap()).unwrap();
        replay_rejected(&source, &bundle, &id, &saved);
    }
    for (path, value) in [
        ("/evidence/1/result", "inconclusive"),
        ("/evidence/1/execution", "timeout"),
        ("/evidence/2/method", "finite_execution"),
        (
            "/evidence/2/semantics/integer_model",
            "mathematical_integer",
        ),
        ("/evidence/2/correspondence/status", "assumed"),
        ("/evidence/1/provenance/tool/version", "0.0.0.0"),
        ("/evidence/0/provenance/adapter/version", "0.0.0"),
        ("/acceptance", "accepted"),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(path).unwrap() = serde_json::json!(value);
        fs::write(&saved, serde_json::to_vec(&changed).unwrap()).unwrap();
        replay_rejected(&source, &bundle, &id, &saved);
    }
    for count in [0, 1, 2, 4] {
        let mut changed = original.clone();
        changed["evidence"] = serde_json::json!(vec![original["evidence"][0].clone(); count]);
        fs::write(&saved, serde_json::to_vec(&changed).unwrap()).unwrap();
        replay_rejected(&source, &bundle, &id, &saved);
    }
    let mut reordered = original.clone();
    reordered["evidence"].as_array_mut().unwrap().swap(0, 2);
    fs::write(&saved, serde_json::to_vec(&reordered).unwrap()).unwrap();
    replay_rejected(&source, &bundle, &id, &saved);
    fs::write(&saved, serde_json::to_vec(&original).unwrap()).unwrap();
    assert_success(
        &replay_command(&source, &bundle, &id, &saved)
            .output()
            .unwrap(),
    );
}

#[test]
fn replay_rejects_duplicate_escaped_or_nested_fields_and_private_malformed_input() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let saved = save_evidence(root.path(), &source, &bundle, &id);
    let original = fs::read_to_string(&saved).unwrap();
    for (key, duplicate) in [
        ("\"acceptance\":", "\"acceptance\":\"FAKE_PRIVATE_DUPLICATE\",\"acceptance\":"),
        ("\"acceptance\":", "\"\\u0061cceptance\":\"not_evaluated\",\"acceptance\":"),
        ("\"configuration_fingerprint\":", "\"configuration_fingerprint\":\"FAKE_PRIVATE_DUPLICATE\",\"configuration_fingerprint\":"),
        ("\"obligation_id\":", "\"obligation_id\":\"FAKE_PRIVATE_DUPLICATE\",\"obligation_id\":"),
    ] {
        let duplicate = original.replacen(key, duplicate, 1);
        assert_ne!(duplicate, original);
        fs::write(&saved, duplicate).unwrap();
        replay_rejected(&source, &bundle, &id, &saved);
    }
    for malformed in [
        "".into(),
        "FAKE_PRIVATE_NOT_JSON".into(),
        "null".into(),
        "[]".into(),
        original[..original.len() / 2].to_owned(),
        format!("{original}{original}"),
        format!("{}0{}", "[".repeat(129), "]".repeat(129)),
    ] {
        fs::write(&saved, malformed).unwrap();
        replay_rejected(&source, &bundle, &id, &saved);
    }
    fs::write(&saved, [0xff, 0xfe]).unwrap();
    replay_rejected(&source, &bundle, &id, &saved);
}

#[test]
fn replay_file_and_pin_boundaries_are_fail_closed_without_following_report_paths() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let saved = save_evidence(root.path(), &source, &bundle, &id);
    let original = fs::read(&saved).unwrap();
    for pin in ["", "FAKE_PRIVATE_PIN", &"0".repeat(64), &format!("{id}\n")] {
        replay_rejected(&source, &bundle, pin, &saved);
    }
    let mut padded = original.clone();
    padded.resize(1024 * 1024, b' ');
    fs::write(&saved, &padded).unwrap();
    assert_success(
        &replay_command(&source, &bundle, &id, &saved)
            .output()
            .unwrap(),
    );
    padded.push(b' ');
    fs::write(&saved, padded).unwrap();
    replay_rejected(&source, &bundle, &id, &saved);
    fs::remove_file(&saved).unwrap();
    replay_rejected(&source, &bundle, &id, &saved);
    let target = root.path().join("FAKE_PRIVATE_TARGET.json");
    fs::write(&target, &original).unwrap();
    symlink(&target, &saved).unwrap();
    replay_rejected(&source, &bundle, &id, &saved);
    fs::remove_file(&saved).unwrap();
    fs::create_dir(&saved).unwrap();
    replay_rejected(&source, &bundle, &id, &saved);
    fs::remove_dir(&saved).unwrap();
    fs::write(&saved, original).unwrap();
    let snapshot = fs::read(bundle.join("program.ll")).unwrap();
    fs::remove_file(bundle.join("program.ll")).unwrap();
    replay_rejected(&source, &bundle, &id, &saved);
    fs::write(bundle.join("program.ll"), &snapshot).unwrap();
    fs::write(&source, format!("{GOOD}//FAKE_PRIVATE_STALE_SOURCE")).unwrap();
    replay_rejected(&source, &bundle, &id, &saved);
    fs::write(&source, GOOD).unwrap();
    let mut changed: serde_json::Value =
        serde_json::from_slice(&fs::read(&saved).unwrap()).unwrap();
    changed["evidence"][0]["artifact_paths"] =
        serde_json::json!(["FAKE_PRIVATE_PATH_MUST_NOT_BE_OPENED"]);
    fs::write(&saved, serde_json::to_vec(&changed).unwrap()).unwrap();
    replay_rejected(&source, &bundle, &id, &saved);
}

#[test]
fn saved_success_with_rebound_hashes_cannot_bypass_either_fresh_query() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, _) = build(root.path());
    for alter_source in [true, false] {
        fs::write(&source, GOOD).unwrap();
        let id = repin(&bundle, Some(GOOD));
        let saved = save_evidence(root.path(), &source, &bundle, &id);
        let mut report: serde_json::Value =
            serde_json::from_slice(&fs::read(&saved).unwrap()).unwrap();
        let original_ir = fs::read_to_string(bundle.join("program.ll")).unwrap();
        let text = if alter_source {
            GOOD.replace("== 7", "== 8")
        } else {
            GOOD.to_owned()
        };
        fs::write(&source, &text).unwrap();
        if !alter_source {
            fs::write(
                bundle.join("program.ll"),
                original_ir.replace("ret i64 7", "ret i64 4294967303"),
            )
            .unwrap();
        }
        let id = repin(&bundle, Some(&text));
        assert_success(&check(&source, &bundle, &id));
        let record: native_bundle::NativeBundleRecord =
            serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
        report["build_id"] = serde_json::json!(id);
        report["evidence"][0]["specification"]["sha256"] = serde_json::json!(id);
        report["evidence"][1]["subject"]["sha256"] = serde_json::json!(record.source.source_sha256);
        report["evidence"][1]["specification"]["sha256"] =
            serde_json::json!(record.source.specification_sha256);
        report["evidence"][2]["subject"]["sha256"] =
            serde_json::json!(record.files[&BundleFile::LlvmIr].sha256);
        report["evidence"][2]["specification"]["sha256"] =
            serde_json::json!(record.source.source_sha256);
        fs::write(&saved, serde_json::to_vec(&report).unwrap()).unwrap();
        let rejected = replay_rejected(&source, &bundle, &id, &saved);
        assert!(
            String::from_utf8_lossy(&rejected.stderr).contains(if alter_source {
                "source postcondition refuted"
            } else {
                "LLVM body differs from source"
            })
        );
        fs::write(bundle.join("program.ll"), original_ir).unwrap();
    }
}

#[test]
fn replay_of_rebound_garbage_artifacts_never_executes_or_promotes_binary_assurance() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let saved = save_evidence(root.path(), &source, &bundle, &id);
    let mut report: serde_json::Value = serde_json::from_slice(&fs::read(&saved).unwrap()).unwrap();
    let marker = root.path().join("MUST_NOT_RUN");
    fs::write(
        bundle.join("program"),
        format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
    )
    .unwrap();
    fs::set_permissions(bundle.join("program"), fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(bundle.join("program.o"), b"garbage object").unwrap();
    let id = repin(&bundle, None);
    replay_rejected(&source, &bundle, &id, &saved);
    report["build_id"] = serde_json::json!(id);
    report["evidence"][0]["specification"]["sha256"] = serde_json::json!(id);
    report["evidence"][0]["subject"]["sha256"] = serde_json::json!(native_bundle::sha256(
        &fs::read(bundle.join("program")).unwrap()
    ));
    fs::write(&saved, serde_json::to_vec(&report).unwrap()).unwrap();
    let output = replay_command(&source, &bundle, &id, &saved)
        .output()
        .unwrap();
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Object/linker/runtime/OS exit-status/executable semantics: not checked")
    );
    assert!(stdout.contains("Evidence acceptance: not evaluated"));
    assert!(!stdout.contains("artifact_verified"));
    assert!(!marker.exists());
}

const MODULE: &str = "atom helper(x: i64) -> i64 requires: x > 0; ensures: result == x + 1; body: { x + 1 };\natom main() -> i64 requires: true; ensures: result == 6; body: { helper(5) };";

const WRAPPED: &str = "atom id(x: i64) -> i64 requires: true; ensures: result == x; body: { x };\natom main() -> i64 requires: true; ensures: result > 0; body: { id(9223372036854775807) + 1 };";

fn build_source(root: &Path, text: &str) -> (std::path::PathBuf, std::path::PathBuf, String) {
    let source = root.join("source.mm");
    let bundle = root.join("bundle");
    fs::write(&source, text).unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    (source, bundle, id)
}

fn module_record(bundle: &Path) -> serde_json::Value {
    serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap()
}

#[test]
fn module_bundle_builds_checks_and_rechecks_within_its_own_profile() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), MODULE);
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MODULE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE);
    assert_eq!(record["source"]["atoms"].as_object().unwrap().len(), 2);
    // The dependency pin is the claimbook convention: sha256 of the
    // canonical JSON map {atom: [sorted direct callees]}.
    let dependencies = serde_json::json!({"helper": [], "main": ["helper"]});
    assert_eq!(
        record["source"]["source_dependencies_sha256"],
        native_bundle::sha256(&serde_json::to_vec(&dependencies).unwrap())
    );
    assert_success(&check(&source, &bundle, &id));
    let recheck = recheck(&source, &bundle, &id);
    assert_success(&recheck);
    let stdout = String::from_utf8_lossy(&recheck.stdout);
    assert!(stdout.contains(native_bundle::MODULE_RECHECK_PROFILE));
    let llvm = recheck_llvm(&source, &bundle, &id);
    assert_success(&llvm);
    let stdout = String::from_utf8_lossy(&llvm.stdout);
    assert!(stdout.contains(mumei_emit_llvm::translation_validation::NATIVE_MODULE_PROFILE));
}

#[test]
fn module_bundle_with_refined_scalar_members_builds_as_v2() {
    // A module declaring a refined scalar alias and using it in a
    // signature builds under the v2 profile: the record names the
    // refined admission, and check/recheck/recheck-llvm all re-derive
    // under that same gate.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "type Port = i64 where v >= 1 && v <= 65535;\n\
         atom listen(x: Port) -> Port requires: true; ensures: result == x; body: { x };\n\
         atom main() -> i64 requires: true; ensures: result == 80; body: { listen(80) };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MODULE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V2);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_REFINED_PROFILE
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // The emitted consumer evidence names the pinned v2 profile, not
    // the v1 constant — the identity record must say which admission
    // actually produced the bundle.
    let json = consume("recheck-llvm --json", &source, &bundle, &id);
    assert_success(&json);
    let envelope: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(
        envelope["evidence"][0]["semantics"]["profile"],
        native_bundle::MODULE_PROFILE_V2
    );
    // A v2 record re-pinned under the v1 name is not a v1 record —
    // the identity check refuses it rather than silently re-admitting
    // under the narrower gate.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

const COMPOSITE_MODULE: &str = "struct Point { x: i64, y: i64 }\n\
     atom gx(p: Point) -> i64 requires: p.x >= 0; ensures: result == p.x; body: { p.x };\n\
     atom fwd(p: Point) -> i64 requires: p.x >= 1; ensures: result >= 1; body: { gx(p) };\n\
     atom main() -> i64 requires: true; ensures: result == 0; body: { 0 };";

#[test]
fn module_bundle_with_struct_members_builds_as_v4_and_rechecks_its_source() {
    // Struct declarations, struct-typed parameters, field reads and a
    // same-struct pass-through call build under the composite profile;
    // check and the source recheck re-derive under that same gate.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), COMPOSITE_MODULE);
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MODULE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V4);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE
    );
    assert_eq!(record["source"]["atoms"].as_object().unwrap().len(), 3);
    assert_success(&check(&source, &bundle, &id));
    let output = recheck(&source, &bundle, &id);
    assert_success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains(native_bundle::MODULE_RECHECK_PROFILE));
    // The LLVM snapshot recheck pairs each `{ i64, i64 }` formal with
    // the struct parameter's flattened field slots.
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // A snapshot reading the fields in the wrong order is a different
    // member, even with every identity pin refreshed to match it.
    let ir = fs::read_to_string(bundle.join("program.ll")).unwrap();
    let swapped = ir
        .replace("%0, 0", "%0, 9")
        .replace("%0, 1", "%0, 0")
        .replace("%0, 9", "%0, 1");
    assert_ne!(swapped, ir);
    fs::write(bundle.join("program.ll"), swapped).unwrap();
    let swapped_id = repin_module(&bundle);
    assert_success(&check(&source, &bundle, &swapped_id));
    let llvm = recheck_llvm(&source, &bundle, &swapped_id);
    assert!(!llvm.status.success());
    fs::write(bundle.join("program.ll"), ir).unwrap();
    let id = repin_module(&bundle);
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // A v4 record re-pinned under any narrower name is refused: those
    // gates keep `struct` out of the item vocabulary.
    for narrower in [
        native_bundle::MODULE_PROFILE,
        native_bundle::MODULE_PROFILE_V2,
        native_bundle::MODULE_PROFILE_V3,
    ] {
        let mut record = module_record(&bundle);
        record["profile"] = serde_json::Value::String(narrower.into());
        let bytes = serde_json::to_vec_pretty(&record).unwrap();
        fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
        let id = native_bundle::sha256(&bytes);
        assert!(!check(&source, &bundle, &id).status.success());
        assert!(!recheck(&source, &bundle, &id).status.success());
    }
}

#[test]
fn module_bundle_with_alias_struct_fields_builds_as_v5_and_rechecks() {
    // `gx`'s postcondition holds only under `Pos`'s predicate on `p.x`:
    // the verifier now assumes it for struct fields, so the build and
    // every recheck bind the same field domain.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "type Pos = i64 where v > 0;\nstruct P { x: Pos, y: i64 }\n\
         atom gx(p: P) -> i64 requires: true; ensures: result >= 1; body: { p.x };\n\
         atom fwd(q: P) -> i64 requires: true; ensures: result >= 1; body: { gx(q) };\n\
         atom main() -> i64 requires: true; ensures: result == 0; body: { 0 };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V5);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V2
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // A v5 record re-pinned as v4 is refused: v4 keeps literal fields.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V4.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn literal_field_composite_modules_keep_pinning_v4() {
    let root = tempfile::tempdir().unwrap();
    let (_source, bundle, _id) = build_source(root.path(), COMPOSITE_MODULE);
    assert_eq!(
        module_record(&bundle)["profile"],
        native_bundle::MODULE_PROFILE_V4
    );
}

#[test]
fn module_bundle_without_structs_keeps_its_narrow_profile_under_the_composite_producer() {
    let root = tempfile::tempdir().unwrap();
    let (_source, bundle, _id) = build_source(root.path(), MODULE);
    assert_eq!(
        module_record(&bundle)["profile"],
        native_bundle::MODULE_PROFILE
    );
}

#[test]
fn module_bundle_with_struct_and_single_main_takes_the_module_path() {
    // A lone `main` beside a struct declaration is still a composite
    // module — the single-atom path rejects every non-atom item.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "struct Point { x: i64 }\n\
         atom main() -> i64 requires: true; ensures: result == 0; body: { 0 };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MODULE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V4);
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    // No member takes a struct parameter, so no aggregate formal exists
    // and the LLVM snapshot recheck still reaches its scalar profile.
    assert_success(&recheck_llvm(&source, &bundle, &id));
}

#[test]
fn composite_source_recheck_refutes_wrapped_field_arithmetic() {
    // `verify` reasons over mathematical integers; the source recheck
    // encodes each flattened field as a wrapping i64, so the sum of two
    // nonnegative fields can wrap below the first one.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "struct Point { x: i64, y: i64 }\n\
         atom sum(p: Point) -> i64 requires: p.x >= 0 && p.y >= 0; ensures: result >= p.x; body: { p.x + p.y };\n\
         atom main() -> i64 requires: true; ensures: result == 0; body: { 0 };",
    );
    assert_eq!(
        module_record(&bundle)["profile"],
        native_bundle::MODULE_PROFILE_V4
    );
    assert_success(&check(&source, &bundle, &id));
    let output = recheck(&source, &bundle, &id);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("refuted"));
}

#[test]
fn multifile_bundle_with_struct_member_builds_as_v4() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        root.path().join("shapes.mm"),
        "struct Point { x: i64, y: i64 }\n\
         atom gx(p: Point) -> i64 requires: true; ensures: result == p.x; body: { p.x };\n\
         atom one(k: i64) -> i64 requires: true; ensures: result == k; body: { k };\n",
    )
    .unwrap();
    fs::write(
        &source,
        "import \"shapes\" as s;\n\natom main() -> i64 requires: true; ensures: result == 3; body: { s::one(3) };\n",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MULTIFILE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MULTIFILE_PROFILE_V4);
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // Under a v3 pin the struct member is refused at the member gate.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MULTIFILE_PROFILE_V3.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn module_bundle_with_struct_results_builds_as_v6_and_rechecks() {
    // `make` returns a struct literal, `main` reads a field of its call
    // result: the build, the source recheck and the LLVM snapshot
    // recheck all pair the returned aggregate with its field slots.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "type Pos = i64 where v > 0;\nstruct P { x: Pos, y: i64 }\n\
         atom make(a: Pos) -> P requires: true; ensures: result.x == a; body: { P { x: a, y: 2 } };\n\
         atom uselet(a: Pos) -> i64 requires: true; ensures: result >= 1; body: { let q = make(a); q.x };\n\
         atom main() -> i64 requires: true; ensures: result == 5; body: { make(5).x };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V6);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V3
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // A v6 record re-pinned as v5 is refused: v5 keeps struct values out.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V5.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn multifile_bundle_with_struct_results_builds_as_v6() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        root.path().join("shapes.mm"),
        "struct Point { x: i64, y: i64 }\n\
         atom origin(k: i64) -> Point requires: true; ensures: result.y == k; body: { Point { x: 0, y: k } };\n",
    )
    .unwrap();
    fs::write(
        &source,
        "import \"shapes\" as s;\n\natom main() -> i64 requires: true; ensures: result == 3; body: { s::origin(3).y };\n",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MULTIFILE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MULTIFILE_PROFILE_V6);
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
}

#[test]
fn module_bundle_with_nested_struct_fields_builds_as_v11_and_rechecks() {
    // `set` writes a whole nested subtree (`o.i = w`) and `main` reads a
    // nested leaf of its call result: the build, the source recheck and
    // the LLVM snapshot recheck all pair the nested aggregate's leaf
    // slots under composite v8.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "struct I { a: i64, b: i64 }\nstruct O { i: I, c: i64 }\n\
         atom set(o: O, w: I) -> O requires: true; ensures: result.i.a == w.a; body: { o.i = w; o };\n\
         atom main() -> i64 requires: true; ensures: true; body: { set(O { i: I { a: 1, b: 2 }, c: 0 }, I { a: 8, b: 9 }).i.a };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V11);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V8
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // A v11 record re-pinned as v10 is refused: v10 keeps nested
    // struct fields out of the composite shape.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V10.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn module_bundle_rejects_colliding_flattened_field_names() {
    // `O { i: I, i_a: i64 }` flattens `i.a` and `i_a` onto the same
    // verifier symbol `i_a`: two independent fields would alias one
    // slot and could be proven equal, so admission must refuse it.
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        &source,
        "struct I { a: i64 }\nstruct O { i: I, i_a: i64 }\n\
         atom main() -> i64 requires: true; ensures: true; body: { 0 };",
    )
    .unwrap();
    assert!(!cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ])
    .status
    .success());
}

#[test]
fn module_bundle_rejects_exponential_struct_expansion() {
    // Every level names the same child type twice, so the flattened
    // leaf count doubles per level: the admission check must bail on
    // the bound during the count instead of materializing ~2^30 leaves
    // first (which would exhaust memory before it could reject).
    let mut decls = String::from("struct L0 { v: i64 }\n");
    for i in 1..30 {
        decls.push_str(&format!("struct L{i} {{ x: L{}, y: L{} }}\n", i - 1, i - 1));
    }
    decls.push_str("atom main() -> i64 requires: true; ensures: true; body: { 0 };");
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.mm");
    let bundle = root.path().join("bundle");
    fs::write(&source, &decls).unwrap();
    assert!(!cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ])
    .status
    .success());
}

#[test]
fn multifile_bundle_with_nested_struct_fields_builds_as_v11() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        root.path().join("shapes.mm"),
        "struct I { a: i64, b: i64 }\nstruct O { i: I, c: i64 }\n\
         atom set(o: O, w: I) -> O requires: true; ensures: result.i.a == w.a; body: { o.i = w; o };\n\
         atom get(o: O) -> i64 requires: true; ensures: result == o.i.a; body: { o.i.a };\n",
    )
    .unwrap();
    fs::write(
        &source,
        "import \"shapes\" as s;\n\natom main() -> i64 requires: true; ensures: true; body: { 0 };\n",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MULTIFILE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MULTIFILE_PROFILE_V11);
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // Under a v10 pin the same members stay refused at the member gate.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MULTIFILE_PROFILE_V10.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn module_bundle_with_generic_structs_builds_as_v12_and_rechecks() {
    // Generic `struct` declarations monomorphize to ordinary v8-shaped
    // declarations before verification: the build, the source recheck
    // and the LLVM snapshot recheck all run on the instantiated module.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "struct Pair<T> { a: T, b: T }\nstruct Box<T> { inner: T }\n\
         atom mk(a: i64) -> Pair<i64> requires: a >= 0; ensures: result.a == a && result.b == a; body: { Pair<i64>{a: a, b: a} };\n\
         atom boxed(b: Box<Pair<i64>>) -> i64 requires: true; ensures: result == b.inner.a; body: { b.inner.a };\n\
         atom main() -> i64 requires: true; ensures: result == 2; body: { let p = Pair<i64>{a: 2, b: 2}; p.a };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V12);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V9
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // A v12 record re-pinned as v11 is refused: v11 keeps generic
    // struct declarations out of the vocabulary.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V11.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn multifile_bundle_with_generic_structs_builds_as_v12() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        root.path().join("shapes.mm"),
        "struct Pair<T> { a: T, b: T }\n\
         atom sum(p: Pair<i64>) -> i64 requires: p.a >= 0 && p.b >= 0; ensures: result == p.a + p.b; body: { p.a + p.b };\n",
    )
    .unwrap();
    fs::write(
        &source,
        "import \"shapes\" as s;\n\natom main() -> i64 requires: true; ensures: true; body: { 0 };\n",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MULTIFILE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MULTIFILE_PROFILE_V12);
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // Under a v11 pin the member's generic declarations stay refused.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MULTIFILE_PROFILE_V11.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn module_bundle_with_u64_leaves_builds_as_v13_and_rechecks() {
    // `u64` leaf parameters/fields/results pin the module at composite
    // v10: unsigned comparisons and `udiv`/`urem`/`lshr` semantics on
    // both the source encoding and the emitted `icmp u*`/`udiv`/`lshr`.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "struct Bucket { total: u64, count: i64 }\n\
         atom add(b: Bucket, e: u64) -> Bucket requires: true; ensures: result.total == b.total + e && result.count == b.count; body: { Bucket{total: b.total + e, count: b.count} };\n\
         atom half(a: u64) -> u64 requires: a % 2 == 0; ensures: result * 2 == a; body: { a / 2 };\n\
         atom main() -> i64 requires: true; ensures: result == 1; body: { let b = Bucket{total: 4, count: 1}; if add(b, 2).total >= 6 { 1 } else { 0 } };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V13);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V10
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // A v13 record re-pinned as v12 is refused: v12 keeps u64 leaves
    // out of the vocabulary.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V12.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn module_bundle_with_bounded_arrays_builds_as_v14_and_rechecks() {
    // `let`-bound stack arrays pin the module at composite v11 /
    // bundle v14: the bounded-memory source encoding (SMT-array cells
    // with graceful out-of-bounds semantics) pairs with the emitter's
    // `alloca [N x i64]` + gep/load/store lowering.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "atom sum() -> i64 requires: true; ensures: result == 13; body: { let a = [1, 2, 3]; a[1] = 9; a[0] + a[1] + a[2] };\n\
         atom pick(a0: i64, i: i64) -> i64 requires: 0 <= i && i < 3; ensures: result == 7; body: { let a = [a0, 20, 30]; a[i] = 7; a[i] };\n\
         atom main() -> i64 requires: true; ensures: result == 20; body: { sum() + pick(1, 0) };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V14);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_MEMORY_PROFILE
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // A v14 record re-pinned as v13 is refused: v13 keeps arrays out of
    // the vocabulary.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V13.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn multifile_bundle_with_bounded_arrays_builds_as_v14() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        root.path().join("cell.mm"),
        "atom pick(i: i64) -> i64 requires: 0 <= i && i < 2; ensures: result == 5; body: { let a = [3, 5]; a[i] = 5; a[i] };\n",
    )
    .unwrap();
    fs::write(
        &source,
        "import \"cell\" as c;\n\natom main() -> i64 requires: true; ensures: true; body: { c::pick(1) };\n",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MULTIFILE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MULTIFILE_PROFILE_V14);
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
}

#[test]
fn multifile_bundle_with_u64_leaves_builds_as_v13() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        root.path().join("amounts.mm"),
        "atom add(a: u64, b: u64) -> u64 requires: true; ensures: result == a + b; body: { a + b };\n",
    )
    .unwrap();
    fs::write(
        &source,
        "import \"amounts\" as amt;\n\natom main() -> i64 requires: true; ensures: true; body: { 0 };\n",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MULTIFILE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MULTIFILE_PROFILE_V13);
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // Under a v12 pin the member's u64 leaf stays refused.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MULTIFILE_PROFILE_V12.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn module_bundle_with_f64_leaves_builds_as_v15_and_rechecks() {
    // `f64` leaf parameters/fields/results pin the module at composite
    // v11: one 64-bit `double` slot per leaf carrying the IEEE-754 bit
    // pattern — pass-through on both sides, never read by arithmetic
    // or comparison.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "struct B { v: f64, t: i64 }\n\
         atom wrap(x: f64, t: i64) -> B requires: t >= 0; ensures: result.t == t; body: { B{v: x, t: t} };\n\
         atom unwrap(b: B) -> f64 requires: b.t >= 0; ensures: true; body: { b.v };\n\
         atom pick(f: bool, x: f64, y: f64) -> f64 requires: true; ensures: true; body: { if f { x } else { y } };\n\
         atom main() -> i64 requires: true; ensures: result == 1; body: { 1 };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V15);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V11
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // A v15 record re-pinned as v14 is refused: v14 keeps f64 leaves
    // out of the vocabulary.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V14.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn multifile_bundle_with_f64_leaves_builds_as_v15() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        root.path().join("rates.mm"),
        "atom copy(a: f64) -> f64 requires: true; ensures: true; body: { a };\n",
    )
    .unwrap();
    fs::write(
        &source,
        "import \"rates\" as r;\n\natom main() -> i64 requires: true; ensures: true; body: { 0 };\n",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MULTIFILE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MULTIFILE_PROFILE_V15);
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // Under a v14 pin the member's f64 leaf stays refused.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MULTIFILE_PROFILE_V14.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn module_bundle_with_nested_field_writes_builds_as_v18_and_rechecks() {
    // `bump` writes a leaf two levels deep (`o.i.a = w`): the build,
    // the source recheck and the LLVM snapshot recheck all carry the
    // desugared subtree rebuild under composite v15.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "struct I { a: i64, b: i64 }\nstruct O { i: I, c: i64 }\n\
         atom bump(o: O, w: i64) -> O requires: true; ensures: result.i.a == w; body: { o.i.a = w; o };\n\
         atom main() -> i64 requires: true; ensures: true; body: { bump(O { i: I { a: 1, b: 2 }, c: 0 }, 8).i.a };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V18);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V15
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // A v18 record re-pinned as v16 is refused: v16 keeps nested
    // leaf writes out of the composite shape.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V16.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn module_bundle_with_enum_payload_writes_builds_as_v19_and_rechecks() {
    // `bump` writes a named payload slot (`t.p = w`): the build, the
    // source recheck and the LLVM snapshot recheck all carry the
    // tag-gated payload rebuild under composite v16.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "enum C { T(p: i64), U }\n\
         atom bump(w: i64) -> C requires: true; ensures: result == C::T(w); body: { let t = C::T(0); t.p = w; t };\n\
         atom main() -> i64 requires: true; ensures: true; body: { 0 };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V19);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V16
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // A v19 record re-pinned as v18 is refused: v18 keeps enum
    // payload writes out of the composite shape.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V18.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn multifile_bundle_with_nested_field_writes_builds_as_v18() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        root.path().join("shapes.mm"),
        "struct I { a: i64, b: i64 }\nstruct O { i: I, c: i64 }\n\
         atom bump(o: O, w: i64) -> O requires: true; ensures: result.i.a == w; body: { o.i.a = w; o };\n",
    )
    .unwrap();
    fs::write(
        &source,
        "import \"shapes\" as s;\n\natom main() -> i64 requires: true; ensures: true; body: { 0 };\n",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MULTIFILE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MULTIFILE_PROFILE_V18);
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // Under a v16 pin the member's nested write stays refused.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MULTIFILE_PROFILE_V16.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn module_bundle_with_typedef_and_single_main_takes_the_module_path() {
    // A module holding one `main` plus a refined scalar declaration is
    // still a module — dispatch on atom count alone would send it to
    // the single-atom path, which rejects `type` items outright.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "type Port = i64 where v >= 1 && v <= 65535;\n\
         atom main() -> i64 requires: true; ensures: result == 0; body: { 0 };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MODULE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V2);
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
}

#[test]
fn multifile_bundle_with_refined_member_builds_as_v2() {
    // A member carrying a `type` declaration merges only under the
    // refined admission — the member gate must not reject it before the
    // merged module's profile check ever runs.
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        root.path().join("helpers.mm"),
        "type Port = i64 where v >= 1 && v <= 65535;\n\
         atom listen(x: Port) -> Port requires: true; ensures: result == x; body: { x };\n",
    )
    .unwrap();
    fs::write(
        &source,
        "import \"helpers\" as h;\n\natom main() -> i64 requires: true; ensures: result == 80; body: { h::listen(80) };\n",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MULTIFILE_PROFILE_V2);
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // The consumer evidence identity record names the pinned v2
    // multifile profile.
    let json = consume("recheck-llvm --json", &source, &bundle, &id);
    assert_success(&json);
    let envelope: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(
        envelope["evidence"][0]["semantics"]["profile"],
        native_bundle::MULTIFILE_PROFILE_V2
    );
    // Under a v1 pin the same members stay refused at the member gate.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MULTIFILE_PROFILE.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn module_bundle_with_match_atom_builds_and_rechecks() {
    // Certificate generation annotates match bodies with a
    // manual-lemma reason in the translator IR; the binding pins the
    // extraction gate's IR identity, which never carries that
    // annotation. Comparing them unnormalized made every match module
    // fail `certificate binding mismatch` at build.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), "atom pick(x: i64) -> i64 requires: true; ensures: result >= 0; body: { match x { 0 => 0, _ => 1 } };\natom main() -> i64 requires: true; ensures: result >= 0; body: { pick(1) };");
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
}

#[test]
fn module_source_recheck_refutes_wrapped_i64_arithmetic() {
    // `verify` works over mathematical integers, but the source recheck
    // encodes the widened wrapping i64 profile: `id(i64::MAX) + 1`
    // wraps below zero, so the postcondition obligation is refutable
    // even though the call domain is trivially closed.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), WRAPPED);
    assert_success(&check(&source, &bundle, &id));
    let output = recheck(&source, &bundle, &id);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("refuted"));
}

#[test]
fn module_recheck_llvm_json_emits_per_atom_consumer_evidence() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), MODULE);
    let output = recheck_llvm_json(&source, &bundle, &id);
    assert_success(&output);
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["schema_version"],
        "mumei.native-module-consumer-evidence/v1"
    );
    assert_eq!(report["build_id"], id);
    assert_eq!(report["acceptance"], "not_evaluated");
    let evidence = report["evidence"].as_array().unwrap();
    // One identity record plus a postcondition and a translation record
    // per atom (`helper` and `main` here).
    assert_eq!(evidence.len(), 1 + 2 * 2);
    assert_eq!(
        evidence[0]["obligation_id"],
        "native_bundle_identity_consistency"
    );
    let obligations: Vec<&str> = evidence
        .iter()
        .map(|record| record["obligation_id"].as_str().unwrap())
        .collect();
    for atom in ["helper", "main"] {
        assert!(obligations.contains(&format!("{atom}_source_postcondition").as_str()));
        assert!(obligations.contains(&format!("{atom}_source_to_llvm_user_body").as_str()));
    }
    // The identity record names the module bundle profile and every
    // per-atom postcondition record names the module recheck profile —
    // never the single-atom family strings.
    assert_eq!(
        evidence[0]["semantics"]["profile"],
        native_bundle::MODULE_PROFILE
    );
    for record in evidence.iter().skip(1) {
        let obligation = record["obligation_id"].as_str().unwrap();
        if obligation.ends_with("_source_postcondition") {
            assert_eq!(
                record["semantics"]["profile"],
                native_bundle::MODULE_RECHECK_PROFILE
            );
        }
        assert_eq!(record["scope"]["kind"], "unbounded");
    }
}

#[test]
fn module_saved_evidence_roundtrips_and_rejects_tampering() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), MODULE);
    let saved = save_evidence(root.path(), &source, &bundle, &id);
    assert_success(
        &replay_command(&source, &bundle, &id, &saved)
            .output()
            .unwrap(),
    );
    // A tampered atom specification must never replay.
    let mut tampered: serde_json::Value =
        serde_json::from_slice(&fs::read(&saved).unwrap()).unwrap();
    let obligation = tampered["evidence"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|record| record["obligation_id"] == "helper_source_postcondition")
        .unwrap();
    obligation["specification"]["sha256"] = serde_json::Value::String("0".repeat(64));
    fs::write(&saved, serde_json::to_vec(&tampered).unwrap()).unwrap();
    let output = replay_rejected(&source, &bundle, &id, &saved);
    assert!(String::from_utf8_lossy(&output.stderr).contains("differs"));
}

#[test]
fn saved_evidence_family_mismatch_and_unknown_schemas_are_rejected() {
    let single_root = tempfile::tempdir().unwrap();
    let (single_source, single_bundle, single_id) = build(single_root.path());
    let module_root = tempfile::tempdir().unwrap();
    let (module_source, module_bundle, module_id) = build_source(module_root.path(), MODULE);
    let single_saved = save_evidence(
        single_root.path(),
        &single_source,
        &single_bundle,
        &single_id,
    );
    let module_saved = save_evidence(
        module_root.path(),
        &module_source,
        &module_bundle,
        &module_id,
    );
    // Single-atom evidence must not stand in for a module bundle, nor the
    // reverse — the families are dispatched on the saved file's schema.
    for (source, bundle, id, saved) in [
        (&module_source, &module_bundle, &module_id, &single_saved),
        (&single_source, &single_bundle, &single_id, &module_saved),
    ] {
        let output = replay_rejected(source, bundle, id, saved);
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("family"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    // An unrecognized schema_version is rejected before any comparison.
    let mut unknown: serde_json::Value =
        serde_json::from_slice(&fs::read(&module_saved).unwrap()).unwrap();
    unknown["schema_version"] = serde_json::Value::String("mumei.bogus/v9".into());
    fs::write(&module_saved, serde_json::to_vec(&unknown).unwrap()).unwrap();
    replay_rejected(&module_source, &module_bundle, &module_id, &module_saved);
}

#[test]
fn module_build_rejects_sources_without_a_single_trivially_gated_main() {
    for text in [
        // No zero-argument `main` atom among the siblings.
        "atom helper(x: i64) -> i64 requires: x > 0; ensures: result == x + 1; body: { x + 1 };\natom other(x: i64) -> i64 requires: true; ensures: result == x; body: { x };",
        // Two mains leave the entry point ambiguous.
        "atom main() -> i64 requires: true; ensures: result == 0; body: { 0 };\natom main() -> i64 requires: true; ensures: result == 0; body: { 0 };",
        // A gated main keeps the single-atom contract convention.
        "atom main() -> i64 requires: 1 > 0; ensures: result == 0; body: { 0 };\natom helper(x: i64) -> i64 requires: true; ensures: result == x; body: { x };",
        // The entry point keeps the `-> i64` signature the C wrapper
        // attests even though siblings may return other scalars.
        "atom main() -> bool requires: true; ensures: result; body: { 1 > 0 };\natom helper(x: i64) -> i64 requires: true; ensures: result == x; body: { x };",
    ] {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.mm");
        fs::write(&source, text).unwrap();
        let output = cli(&[
            "native-bundle",
            "build",
            source.to_str().unwrap(),
            "-o",
            root.path().join("bundle").to_str().unwrap(),
        ]);
        assert!(!output.status.success());
    }
}

fn recheck_artifact(source: &Path, bundle: &Path, id: &str) -> Output {
    consume("recheck-artifact", source, bundle, id)
}

fn repin_module(bundle: &Path) -> String {
    let mut record: native_bundle::ModuleNativeBundleRecord =
        serde_json::from_slice(&fs::read(bundle.join(RECORD_FILE)).unwrap()).unwrap();
    for role in BundleFile::ALL {
        record.files.insert(
            role,
            native_bundle::ContentIdentity::of(&fs::read(bundle.join(role.file_name())).unwrap()),
        );
    }
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    native_bundle::sha256(&bytes)
}

#[test]
fn artifact_recheck_reproduces_pinned_object_and_executable() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let output = recheck_artifact(&source, &bundle, &id);
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("toolchain_identity_only"));
    assert!(stdout.contains("no_execution_or_runtime_semantics"));
}

#[test]
fn module_artifact_recheck_reproduces_pinned_object_and_executable() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), MODULE);
    let output = recheck_artifact(&source, &bundle, &id);
    assert_success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("toolchain_identity_only"));
}

#[test]
fn artifact_recheck_rejects_a_repinned_foreign_object() {
    // A bundle repinned around a *valid but foreign* object — one this
    // toolchain did not emit from the pinned `program.ll` — fails the
    // object stage, not just the identity stage.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, _) = build(root.path());
    let other_root = tempfile::tempdir().unwrap();
    let other_source = other_root.path().join("other.mm");
    fs::write(
        &other_source,
        "atom main() -> i64 requires: true; ensures: result == 9; body: { 9 };",
    )
    .unwrap();
    let other_bundle = other_root.path().join("bundle");
    assert_success(&cli(&[
        "native-bundle",
        "build",
        other_source.to_str().unwrap(),
        "-o",
        other_bundle.to_str().unwrap(),
    ]));
    fs::copy(
        other_bundle.join(BundleFile::Object.file_name()),
        bundle.join(BundleFile::Object.file_name()),
    )
    .unwrap();
    let id = repin(&bundle, None);
    let output = recheck_artifact(&source, &bundle, &id);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("differs"));
}

#[test]
fn artifact_recheck_rejects_a_repinned_foreign_executable() {
    // The pinned object is genuine, so the object stage passes; the
    // relinked executable must still equal the pinned `program`.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, _) = build(root.path());
    fs::write(
        bundle.join(BundleFile::Executable.file_name()),
        b"repinned non-executable bytes",
    )
    .unwrap();
    let id = repin(&bundle, None);
    let output = recheck_artifact(&source, &bundle, &id);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("differs"));
}

#[test]
fn module_artifact_recheck_rejects_a_repinned_foreign_executable() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, _) = build_source(root.path(), MODULE);
    fs::write(
        bundle.join(BundleFile::Executable.file_name()),
        b"repinned non-executable bytes",
    )
    .unwrap();
    let id = repin_module(&bundle);
    let output = recheck_artifact(&source, &bundle, &id);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("differs"));
}

#[test]
fn accept_artifact_emits_descriptor_only_when_artifacts_reproduce() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    for action in ["accept-artifact", "accept-artifact --json"] {
        let output = consume(action, &source, &bundle, &id);
        assert_success(&output);
        if action.ends_with("--json") {
            let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                descriptor["schema_version"],
                "mumei.native-evidence-acceptance/v1"
            );
            assert_eq!(descriptor["acceptance"], "evaluated");
            assert_eq!(descriptor["accepted"], true);
            assert_eq!(
                descriptor["scope"],
                "native_bundle_source_llvm_and_toolchain_artifact_reproduction"
            );
            assert!(descriptor["checks_performed"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry == "toolchain_object_executable_reproduction"));
            assert!(descriptor["checks_not_performed"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry == "os_exit_status"));
            // Artifact acceptance evaluates no record, so the binding
            // fingerprint key stays absent rather than emitted as null.
            assert!(descriptor.get("evidence_fingerprint").is_none());
            assert!(mumei_core::verification_evidence::decode(&output.stdout).is_err());
        } else {
            assert!(String::from_utf8_lossy(&output.stdout).contains("accepted"));
        }
    }
    // A repinned foreign executable is rejected with no descriptor.
    fs::write(
        bundle.join(BundleFile::Executable.file_name()),
        b"repinned non-executable bytes",
    )
    .unwrap();
    let bad_id = repin(&bundle, None);
    for action in ["accept-artifact", "accept-artifact --json"] {
        let output = consume(action, &source, &bundle, &bad_id);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn accept_artifact_accepts_module_bundles() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(root.path(), MODULE);
    let output = consume("accept-artifact --json", &source, &bundle, &id);
    assert_success(&output);
    let descriptor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        descriptor["schema_version"],
        "mumei.native-evidence-acceptance/v1"
    );
    assert_eq!(descriptor["accepted"], true);
}

const MF_HELPERS: &str =
    "atom helper(x: i64) -> i64 requires: x > 0; ensures: result == x + 1; body: { x + 1 };\n";
const MF_ROOT: &str = "import \"helpers\" as h;\n\natom main() -> i64 requires: true; ensures: result == 6; body: { h::helper(5) };";

fn build_multifile(root: &Path) -> (std::path::PathBuf, std::path::PathBuf, String) {
    let source = root.join("app.mm");
    let bundle = root.join("bundle");
    fs::write(root.join("helpers.mm"), MF_HELPERS).unwrap();
    fs::write(&source, MF_ROOT).unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    (source, bundle, id)
}

#[test]
fn multifile_nested_closure_merges_and_cycles_fail_honestly() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        root.path().join("deep.mm"),
        "atom deep(x: i64) -> i64 requires: true; ensures: result == x + 1; body: { x + 1 };\n",
    )
    .unwrap();
    fs::write(
        root.path().join("helpers.mm"),
        "import \"deep\" as d;\n\natom helper(x: i64) -> i64 requires: x > 0; ensures: result == x + 2; body: { d::deep(x) + 1 };\n",
    )
    .unwrap();
    fs::write(
        &source,
        "import \"helpers\" as h;\n\natom main() -> i64 requires: true; ensures: result == 7; body: { h::helper(5) };",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    // The transitive member is pinned even though the root never names it.
    assert!(record["sources"]["deep.mm"]["sha256"].is_string());
    assert!(bundle.join("sources/deep.mm").is_file());
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    let json = consume("recheck-llvm --json", &source, &bundle, &id);
    assert_success(&json);
    let envelope: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(
        envelope["evidence"].as_array().unwrap().len(),
        2 + 2 * 3 // identity + merge correspondence + 3 atoms × 2
    );
    // Import cycles are rejected honestly at build time.
    fs::write(
        root.path().join("cyc1.mm"),
        "import \"cyc2\";\natom fa() -> i64 requires: true; ensures: result == 1; body: { 1 };",
    )
    .unwrap();
    fs::write(
        root.path().join("cyc2.mm"),
        "import \"cyc1\";\natom fb() -> i64 requires: true; ensures: result == 1; body: { 1 };",
    )
    .unwrap();
    let output = cli(&[
        "native-bundle",
        "build",
        root.path().join("cyc1.mm").to_str().unwrap(),
        "-o",
        root.path().join("cyc.bundle").to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
}

#[test]
fn multifile_subdirectory_members_merge_and_escape_paths_fail_honestly() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    let sub = root.path().join("sub");
    fs::create_dir(&sub).unwrap();
    // `sub/helpers` resolves file-relative: its own `import "deep"`
    // anchors at `sub/`, not the root's directory — exactly the
    // language resolver's base_dir semantics.
    fs::write(
        sub.join("deep.mm"),
        "atom deep(x: i64) -> i64 requires: true; ensures: result == x + 1; body: { x + 1 };\n",
    )
    .unwrap();
    fs::write(
        sub.join("helpers.mm"),
        "import \"deep\" as d;\n\natom helper(x: i64) -> i64 requires: x > 0; ensures: result == x + 2; body: { d::deep(x) + 1 };\n",
    )
    .unwrap();
    // Unaliased subdirectory import: the qualifier is the member's
    // last-segment stem (`deep`), not the directory prefix.
    fs::write(
        sub.join("extra.mm"),
        "atom extra(x: i64) -> i64 requires: true; ensures: result == x - 1; body: { x - 1 };\n",
    )
    .unwrap();
    fs::write(
        &source,
        "import \"sub/helpers\" as h;\nimport \"sub/extra\";\n\natom main() -> i64 requires: true; ensures: result == 12; body: { h::helper(5) + extra(6) };",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    // Nested members pin at their `/`-separated key and land under
    // `sources/` as real directories.
    assert!(record["sources"]["sub/deep.mm"]["sha256"].is_string());
    assert!(record["sources"]["sub/helpers.mm"]["sha256"].is_string());
    assert!(record["sources"]["sub/extra.mm"]["sha256"].is_string());
    assert!(bundle.join("sources/sub/deep.mm").is_file());
    assert!(bundle.join("sources/sub/extra.mm").is_file());
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    let json = consume("recheck-llvm --json", &source, &bundle, &id);
    assert_success(&json);
    let envelope: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(
        envelope["evidence"].as_array().unwrap().len(),
        2 + 2 * 4 // identity + merge correspondence + 4 atoms × 2
    );
    // The import graph pins the file-relative resolution.
    assert_eq!(
        record["source_files"]["import_graph_sha256"]
            .as_str()
            .unwrap(),
        native_bundle::import_graph_sha256(&BTreeMap::from([
            (
                "app.mm".to_string(),
                vec!["sub/extra.mm".to_string(), "sub/helpers.mm".to_string()]
            ),
            ("sub/deep.mm".to_string(), vec![]),
            ("sub/extra.mm".to_string(), vec![]),
            (
                "sub/helpers.mm".to_string(),
                vec!["sub/deep.mm".to_string()]
            ),
        ]))
    );
    // `..` cannot escape the importer's subtree — honest rejection.
    fs::write(
        root.path().join("esc.mm"),
        "import \"../outside\";\natom fa() -> i64 requires: true; ensures: result == 1; body: { 1 };",
    )
    .unwrap();
    let output = cli(&[
        "native-bundle",
        "build",
        root.path().join("esc.mm").to_str().unwrap(),
        "-o",
        root.path().join("esc.bundle").to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
}

const STD_DEP: &str =
    "atom std_helper(x: i64) -> i64 requires: true; ensures: result == x + 1; body: { x + 1 };\n";

#[test]
fn multifile_std_members_resolve_through_the_language_chain() {
    let root = tempfile::tempdir().unwrap();
    let std_dir = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    // The project carries no `std/` directory: `import "std/probe_dep"`
    // falls through importer-relative and the compiler-adjacent /
    // working-directory candidates to MUMEI_STD_PATH, exactly like the
    // language resolver. `probe_dep` is a name no shipped stdlib uses.
    fs::write(std_dir.path().join("probe_dep.mm"), STD_DEP).unwrap();
    fs::write(
        &source,
        "import \"std/probe_dep\";\n\natom main() -> i64 requires: true; ensures: result == 6; body: { probe_dep::std_helper(5) };",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args([
            "native-bundle",
            "build",
            source.to_str().unwrap(),
            "-o",
            bundle.to_str().unwrap(),
        ])
        .env("MUMEI_STD_PATH", std_dir.path())
        .output()
        .unwrap();
    assert_success(&output);
    let record = module_record(&bundle);
    // Std-supplied bytes pin under the importer-prefixed member key —
    // the consumer re-derives the merge from `sources/` alone.
    assert!(record["sources"]["std/probe_dep.mm"]["sha256"].is_string());
    assert_eq!(
        record["source_files"]["import_graph_sha256"]
            .as_str()
            .unwrap(),
        native_bundle::import_graph_sha256(&BTreeMap::from([
            ("app.mm".to_string(), vec!["std/probe_dep.mm".to_string()]),
            ("std/probe_dep.mm".to_string(), vec![]),
        ]))
    );
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));

    // Importer-relative precedence: a project `std/probe_dep.mm`
    // shadows the stdlib fallback — its bytes pin instead.
    let root2 = tempfile::tempdir().unwrap();
    let project_std = root2.path().join("std");
    fs::create_dir(&project_std).unwrap();
    fs::write(
        project_std.join("probe_dep.mm"),
        "atom std_helper(x: i64) -> i64 requires: true; ensures: result == x + 2; body: { x + 2 };\n",
    )
    .unwrap();
    let source2 = root2.path().join("app.mm");
    fs::write(
        &source2,
        "import \"std/probe_dep\";\n\natom main() -> i64 requires: true; ensures: result == 7; body: { probe_dep::std_helper(5) };",
    )
    .unwrap();
    let bundle2 = root2.path().join("bundle");
    let output2 = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args([
            "native-bundle",
            "build",
            source2.to_str().unwrap(),
            "-o",
            bundle2.to_str().unwrap(),
        ])
        .env("MUMEI_STD_PATH", std_dir.path())
        .output()
        .unwrap();
    assert_success(&output2);
    let record2 = module_record(&bundle2);
    assert_eq!(
        record2["sources"]["std/probe_dep.mm"]["sha256"]
            .as_str()
            .unwrap(),
        native_bundle::sha256(
            fs::read_to_string(project_std.join("probe_dep.mm"))
                .unwrap()
                .as_bytes()
        )
    );
}

#[test]
fn multifile_std_member_imports_resolve_beside_their_real_files() {
    let root = tempfile::tempdir().unwrap();
    let std_dir = tempfile::tempdir().unwrap();
    // A fallback member's own imports resolve beside the member's real
    // file — never beside the virtual project `std/` directory. The
    // decoy project `std/helper.mm` would shadow the real helper if
    // the producer resolved the std member's imports against the
    // bundle-key namespace instead of the resolved path's directory.
    fs::write(
        std_dir.path().join("helper.mm"),
        "atom h() -> i64 requires: true; ensures: result == 1; body: { 1 };\n",
    )
    .unwrap();
    fs::write(
        std_dir.path().join("probe_dep.mm"),
        "import \"helper\";\n\natom std_helper(x: i64) -> i64 requires: true; ensures: result == x + 1; body: { x + helper::h() };",
    )
    .unwrap();
    let project_std = root.path().join("std");
    fs::create_dir(&project_std).unwrap();
    fs::write(
        project_std.join("helper.mm"),
        "atom h() -> i64 requires: true; ensures: result == 99; body: { 99 };\n",
    )
    .unwrap();
    let source = root.path().join("app.mm");
    fs::write(
        &source,
        "import \"std/probe_dep\";\n\natom main() -> i64 requires: true; ensures: result == 6; body: { probe_dep::std_helper(5) };",
    )
    .unwrap();
    let bundle = root.path().join("bundle");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args([
            "native-bundle",
            "build",
            source.to_str().unwrap(),
            "-o",
            bundle.to_str().unwrap(),
        ])
        .env("MUMEI_STD_PATH", std_dir.path())
        .output()
        .unwrap();
    assert_success(&output);
    let record = module_record(&bundle);
    // The pinned `std/helper.mm` member carries the REAL stdlib
    // helper's bytes, not the project decoy's.
    assert_eq!(
        record["sources"]["std/helper.mm"]["sha256"]
            .as_str()
            .unwrap(),
        native_bundle::sha256(
            fs::read_to_string(std_dir.path().join("helper.mm"))
                .unwrap()
                .as_bytes()
        )
    );
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
}

#[test]
fn multifile_std_member_shared_by_two_importers_emits_its_atoms_once() {
    let root = tempfile::tempdir().unwrap();
    let std_dir = tempfile::tempdir().unwrap();
    fs::write(std_dir.path().join("probe_dep.mm"), STD_DEP).unwrap();
    // `sub/dep.mm` imports the same stdlib member the root does. Both
    // member keys pin the file (the consumer re-derives the merge from
    // `sources/` alone), but the merge emits the identical body once —
    // the language loads a resolved path once and its ModuleEnv folds
    // same-name atoms silently.
    let sub = root.path().join("sub");
    fs::create_dir(&sub).unwrap();
    fs::write(
        sub.join("dep.mm"),
        "import \"std/probe_dep\";\n\natom dep_add(x: i64) -> i64 requires: true; ensures: result == x + 1; body: { probe_dep::std_helper(x) };",
    )
    .unwrap();
    let source = root.path().join("app.mm");
    fs::write(
        &source,
        "import \"std/probe_dep\";\nimport \"sub/dep\";\n\natom main() -> i64 requires: true; ensures: result == 6; body: { dep::dep_add(5) };",
    )
    .unwrap();
    let bundle = root.path().join("bundle");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args([
            "native-bundle",
            "build",
            source.to_str().unwrap(),
            "-o",
            bundle.to_str().unwrap(),
        ])
        .env("MUMEI_STD_PATH", std_dir.path())
        .output()
        .unwrap();
    assert_success(&output);
    let record = module_record(&bundle);
    // Both member keys pin the stdlib bytes — the merge re-derivation
    // looks each up under its own importer-prefixed name.
    assert!(record["sources"]["std/probe_dep.mm"]["sha256"].is_string());
    assert!(record["sources"]["sub/std/probe_dep.mm"]["sha256"].is_string());
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
}

#[test]
fn multifile_member_key_collision_across_files_fails_honestly() {
    let root = tempfile::tempdir().unwrap();
    let std_dir = tempfile::tempdir().unwrap();
    // The fallback member's own `import "helper"` resolves beside its
    // real file (std_dir/helper.mm), while the root's
    // `import "std/helper"` resolves to the project's std/helper.mm.
    // Both land on the `std/helper.mm` member key but name DIFFERENT
    // files — the language binds them to different modules, which the
    // bundle member namespace cannot express, so the build must fail
    // honestly instead of bundling the wrong implementation.
    fs::write(
        std_dir.path().join("helper.mm"),
        "atom h() -> i64 requires: true; ensures: result == 1; body: { 1 };\n",
    )
    .unwrap();
    fs::write(
        std_dir.path().join("probe_dep.mm"),
        "import \"helper\";\n\natom std_helper(x: i64) -> i64 requires: true; ensures: result == x + 1; body: { x + helper::h() };",
    )
    .unwrap();
    let project_std = root.path().join("std");
    fs::create_dir(&project_std).unwrap();
    // The project helper returns the SAME value as the stdlib one, so
    // only the member-key collision gate — not a postcondition
    // mismatch — distinguishes this build from a valid one.
    fs::write(
        project_std.join("helper.mm"),
        "atom h() -> i64 requires: true; ensures: result == 1; body: { 1 };\n",
    )
    .unwrap();
    let source = root.path().join("app.mm");
    fs::write(
        &source,
        "import \"std/helper\";\nimport \"std/probe_dep\";\n\natom main() -> i64 requires: true; ensures: result == 7; body: { probe_dep::std_helper(5) + helper::h() };",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args([
            "native-bundle",
            "build",
            source.to_str().unwrap(),
            "-o",
            root.path().join("bundle").to_str().unwrap(),
        ])
        .env("MUMEI_STD_PATH", std_dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("collides"));
}

#[test]
fn multifile_bundle_builds_and_rechecks_with_honest_consumer_rejections() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_multifile(root.path());
    let record = module_record(&bundle);
    assert_eq!(record["schema"], "mumei.native-multifile-bundle/v1");
    assert_eq!(record["source_files"]["root"], "app.mm");
    assert!(record["sources"]["app.mm"]["sha256"].is_string());
    assert!(record["sources"]["helpers.mm"]["sha256"].is_string());
    assert!(bundle.join("sources/app.mm").is_file());
    assert!(bundle.join("sources/helpers.mm").is_file());

    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    let artifact = consume("recheck-artifact", &source, &bundle, &id);
    assert_success(&artifact);
    assert_success(&consume("recheck-exit-status", &source, &bundle, &id));

    // The multifile family emits the module-shaped consumer envelope
    // plus a merge-correspondence record.
    let json = consume("recheck-llvm --json", &source, &bundle, &id);
    assert_success(&json);
    let envelope: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(
        envelope["schema_version"],
        "mumei.native-multifile-consumer-evidence/v1"
    );
    assert!(envelope["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .any(|record| record["obligation_id"] == "multifile_merge_correspondence"));
    let evidence = root.path().join("evidence.json");
    fs::write(&evidence, &json.stdout).unwrap();
    for action in [
        "recheck-evidence",
        "accept-evidence",
        "accept-evidence --json",
    ] {
        let mut args = vec!["native-bundle"];
        args.extend(action.split_whitespace());
        args.extend([
            bundle.to_str().unwrap(),
            source.to_str().unwrap(),
            evidence.to_str().unwrap(),
            "--expected-build-id",
            &id,
        ]);
        let output = cli(&args);
        assert_success(&output);
    }
    let descriptor: serde_json::Value = serde_json::from_slice(
        &cli(&[
            "native-bundle",
            "accept-evidence",
            bundle.to_str().unwrap(),
            source.to_str().unwrap(),
            evidence.to_str().unwrap(),
            "--expected-build-id",
            &id,
            "--json",
        ])
        .stdout,
    )
    .unwrap();
    assert_eq!(
        descriptor["scope"],
        "native_multifile_bundle_identity_merge_source_llvm_and_saved_evidence_consistency"
    );
    // A single-family report can never stand in for this family.
    let single = tempfile::tempdir().unwrap();
    let (single_source, single_bundle, single_id) = build(single.path());
    let report = recheck_llvm_json(&single_source, &single_bundle, &single_id);
    assert_success(&report);
    fs::write(&evidence, &report.stdout).unwrap();
    let foreign = cli(&[
        "native-bundle",
        "accept-evidence",
        bundle.to_str().unwrap(),
        source.to_str().unwrap(),
        evidence.to_str().unwrap(),
        "--expected-build-id",
        &id,
    ]);
    assert_eq!(foreign.status.code(), Some(1));
    assert!(foreign.stdout.is_empty());
    let output = consume("accept-artifact", &source, &bundle, &id);
    assert_success(&output);

    // A pinned member that is not the root cannot pose as `--source`.
    let wrong = consume("check", &root.path().join("helpers.mm"), &bundle, &id);
    assert_eq!(wrong.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&wrong.stderr).contains("root source member"));

    // A member swapped inside the pinned sources/ directory is detected.
    fs::write(
        bundle.join("sources/helpers.mm"),
        MF_HELPERS.replace("x + 1", "x + 2"),
    )
    .unwrap();
    let tampered = consume("check", &source, &bundle, &id);
    assert_eq!(tampered.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&tampered.stderr).contains("source member"));
}

#[test]
fn module_bundle_with_enum_equality_builds_as_v17_and_rechecks() {
    // `e == e'`/`e != e'` on two values of one declared enum pins the
    // module at composite v14: the source model compares the whole
    // tagged-union layout — tag plus every payload slot — the same
    // positional `extractvalue`/`icmp` chain the emitter lowers it to.
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build_source(
        root.path(),
        "enum Chan { Tcp(p: i64), Idle }\n\
         atom is_idle(c: Chan) -> bool requires: true; ensures: true; body: { c == Chan::Idle };\n\
         atom differs(a: Chan, b: Chan) -> bool requires: true; ensures: true; body: { a != b };\n\
         atom main() -> i64 requires: true; ensures: result == 0; body: { 0 };",
    );
    let record = module_record(&bundle);
    assert_eq!(record["profile"], native_bundle::MODULE_PROFILE_V17);
    assert_eq!(
        record["source"]["source_profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V14
    );
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // A v17 record re-pinned as v16 is refused: v16 keeps enum
    // equality out of the vocabulary.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MODULE_PROFILE_V16.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}

#[test]
fn multifile_bundle_with_enum_equality_builds_as_v17() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("app.mm");
    let bundle = root.path().join("bundle");
    fs::write(
        root.path().join("chans.mm"),
        "enum Chan { Tcp(p: i64), Idle }\natom is_idle(c: Chan) -> bool requires: true; ensures: true; body: { c == Chan::Idle };\n",
    )
    .unwrap();
    fs::write(
        &source,
        "import \"chans\" as c;\n\natom main() -> i64 requires: true; ensures: true; body: { 0 };\n",
    )
    .unwrap();
    assert_success(&cli(&[
        "native-bundle",
        "build",
        source.to_str().unwrap(),
        "-o",
        bundle.to_str().unwrap(),
    ]));
    let id = native_bundle::sha256(&fs::read(bundle.join(RECORD_FILE)).unwrap());
    let record = module_record(&bundle);
    assert_eq!(record["schema"], native_bundle::MULTIFILE_SCHEMA);
    assert_eq!(record["profile"], native_bundle::MULTIFILE_PROFILE_V17);
    assert_success(&check(&source, &bundle, &id));
    assert_success(&recheck(&source, &bundle, &id));
    assert_success(&recheck_llvm(&source, &bundle, &id));
    // Under a v16 pin the member's enum equality stays refused.
    let mut record = module_record(&bundle);
    record["profile"] = serde_json::Value::String(native_bundle::MULTIFILE_PROFILE_V16.into());
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(bundle.join(RECORD_FILE), &bytes).unwrap();
    assert!(!check(&source, &bundle, &native_bundle::sha256(&bytes))
        .status
        .success());
}
