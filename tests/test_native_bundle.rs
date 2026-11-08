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

#[test]
fn build_binds_actual_native_output_and_source_certificate_without_proving_artifact() {
    let root = tempfile::tempdir().unwrap();
    let (source, bundle, id) = build(root.path());
    let result = check(&source, &bundle, &id);
    assert_success(&result);
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
