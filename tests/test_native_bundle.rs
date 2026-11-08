#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use mumei_core::native_bundle::{self, BundleFile, RECORD_FILE};
use std::{
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
        } else {
            assert!(stdout.contains(mumei_emit_llvm::translation_validation::NATIVE_PROFILE));
            assert!(stdout.contains("source_postcondition_identity_and_llvm_snapshot_only"));
            assert!(stdout.contains("OS exit-status/executable semantics: not checked"));
        }
        assert!(!stdout.contains("artifact_verified"));
        assert_eq!(fs::read_dir(&copied_bundle).unwrap().count(), 5);
        assert_eq!(fs::read_dir(&home).unwrap().count(), 0);
        assert_eq!(fs::read_dir(consumer.path()).unwrap().count(), 5);
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
        GOOD.replace("{ 7 }", "{ 7 + 0 }"),
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
}
