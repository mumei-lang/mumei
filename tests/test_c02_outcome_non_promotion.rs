use mumei_core::proof_cert::{refresh_certificate_integrity, LeanResultMetadata, ProofCertificate};
use mumei_core::verification;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

const VERIFIED_SRC: &str = r#"
atom inc(x: Int) -> Int {
  ensures: result == x + 1;
  body: { x + 1 }
}
"#;

const UNKNOWN_SRC: &str = r#"
atom fermat3(x: i64, y: i64, z: i64) -> i64
requires: x > 0 && y > 0 && z > 0;
ensures: x * x * x + y * y * y != z * z * z;
body: { 0 };
"#;

const SKIPPED_ENSURES_SRC: &str = r#"
atom identity(x: i64) -> i64
requires: true;
ensures: is_hex_digit(result);
body: x;
"#;

const SKIPPED_REQUIRES_SRC: &str = r#"
atom identity(x: i64) -> i64
requires: is_hex_digit(x);
ensures: result == x;
body: x;
"#;

fn source_file(dir: &Path, source: &str) -> std::path::PathBuf {
    let path = dir.join("main.mm");
    fs::write(&path, source).expect("write Mumei source");
    path
}

fn certify(dir: &Path, source: &Path, cert: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--proof-cert")
        .arg("--output")
        .arg(cert)
        .args(args)
        .arg(source)
        .env_remove("MUMEI_VERIFICATION_TIMEOUT_MS")
        .current_dir(dir)
        .output()
        .expect("run mumei verify --proof-cert")
}

fn verify_cert(
    dir: &Path,
    cert: &Path,
    source: &Path,
    allow_lean_verified: bool,
    strict: bool,
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command
        .arg("verify-cert")
        .arg(cert)
        .arg(source)
        .env_remove("MUMEI_VERIFICATION_TIMEOUT_MS")
        .current_dir(dir);
    if allow_lean_verified {
        command.arg("--allow-lean-verified");
    }
    if strict {
        command.arg("--strict");
    }
    command.output().expect("run mumei verify-cert")
}

fn read_certificate(path: &Path) -> ProofCertificate {
    serde_json::from_slice(&fs::read(path).expect("read proof certificate"))
        .expect("parse proof certificate")
}

fn assert_exit(output: &Output, expected: i32) {
    assert_eq!(
        output.status.code(),
        Some(expected),
        "unexpected exit code\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_certificate_result(cert: &ProofCertificate, status: &str, z3_check_result: &str) {
    assert_eq!(cert.atoms.len(), 1, "expected one atom certificate");
    assert_eq!(cert.atoms[0].status, status);
    assert_eq!(cert.atoms[0].z3_check_result, z3_check_result);
}

fn assert_proven(output: &Output) {
    assert_exit(output, 0);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("1 proven"),
        "unexpected verify-cert output:\n{stdout}"
    );
    assert!(
        stdout.contains("All verified: true"),
        "unexpected verify-cert output:\n{stdout}"
    );
}

fn assert_unproven(output: &Output) {
    assert_exit(output, 1);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("1 unproven"),
        "unexpected verify-cert output:\n{stdout}"
    );
    assert!(
        stdout.contains("All verified: false"),
        "unexpected verify-cert output:\n{stdout}"
    );
}

fn verify_cert_both_modes(dir: &Path, cert: &Path, source: &Path, proven: bool) {
    for strict in [false, true] {
        let output = verify_cert(dir, cert, source, false, strict);
        if proven {
            assert_proven(&output);
        } else {
            assert_unproven(&output);
        }
    }
}

fn parse_json(output: &Output, context: &str) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "{context} did not produce JSON: {error}\n{}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

#[test]
fn verified_source_certificate_is_proven() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let source = source_file(dir.path(), VERIFIED_SRC);
    let cert_path = dir.path().join("main.proof-cert.json");

    let verify = certify(dir.path(), &source, &cert_path, &["--json"]);
    assert_exit(&verify, 0);
    assert_certificate_result(&read_certificate(&cert_path), "verified", "unsat");
    verify_cert_both_modes(dir.path(), &cert_path, &source, true);
}

#[test]
fn solver_unknown_certificate_remains_unproven() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let source = source_file(dir.path(), UNKNOWN_SRC);
    let cert_path = dir.path().join("main.proof-cert.json");

    let verify = certify(
        dir.path(),
        &source,
        &cert_path,
        &["--json", "--solver-timeout", "50"],
    );
    assert_exit(&verify, 3);

    let cert = read_certificate(&cert_path);
    assert_eq!(cert.atoms.len(), 1, "expected one atom certificate");
    assert_eq!(cert.atoms[0].status, "failed");
    assert!(
        ["unknown", "timeout"].contains(&cert.atoms[0].z3_check_result.as_str()),
        "unexpected solver result: {}",
        cert.atoms[0].z3_check_result
    );
    verify_cert_both_modes(dir.path(), &cert_path, &source, false);
}

#[test]
fn skipped_ensures_produces_unverifiable_unproven_certificate() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let source = source_file(dir.path(), SKIPPED_ENSURES_SRC);
    let cert_path = dir.path().join("main.proof-cert.json");

    let verify = certify(dir.path(), &source, &cert_path, &["--json"]);
    assert_exit(&verify, 3);
    let report = parse_json(&verify, "partial verify");
    assert_eq!(report["status"], "unverifiable");
    assert_eq!(report["partial"], true);
    assert_certificate_result(&read_certificate(&cert_path), "unverifiable", "skipped");
    verify_cert_both_modes(dir.path(), &cert_path, &source, false);
}

#[test]
fn skipped_requires_keeps_checked_proof_proven() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let source = source_file(dir.path(), SKIPPED_REQUIRES_SRC);
    let cert_path = dir.path().join("main.proof-cert.json");

    let verify = certify(dir.path(), &source, &cert_path, &["--json"]);
    assert_exit(&verify, 0);
    let report = parse_json(&verify, "partial-requires verify");
    assert_eq!(report["partial"], true);
    assert_certificate_result(&read_certificate(&cert_path), "verified", "unsat");
    verify_cert_both_modes(dir.path(), &cert_path, &source, true);
}

#[test]
fn finite_observation_does_not_change_unknown_certificate() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let source = source_file(dir.path(), UNKNOWN_SRC);
    let cert_path = dir.path().join("main.proof-cert.json");
    let verify = certify(
        dir.path(),
        &source,
        &cert_path,
        &["--json", "--solver-timeout", "50"],
    );
    assert_exit(&verify, 3);
    let cert_before = Sha256::digest(fs::read(&cert_path).expect("read certificate"));

    let harvest_path = dir.path().join("harvest.json");
    fs::write(
        &harvest_path,
        serde_json::to_vec(&json!({
            "schema": "mumei.test-harvest/v1",
            "observations": [{
                "kind": "example",
                "target": "fermat3",
                "test": "test_fermat3",
                "file": "tests/test_fermat3.py",
                "line": 1,
                "args": {"x": 1, "y": 1, "z": 1},
                "result": 0
            }]
        }))
        .expect("serialize harvest"),
    )
    .expect("write harvest");
    let observe = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("observe")
        .arg(&source)
        .arg(&harvest_path)
        .arg("--json")
        .env_remove("MUMEI_VERIFICATION_TIMEOUT_MS")
        .current_dir(dir.path())
        .output()
        .expect("run mumei observe");
    assert_exit(&observe, 0);
    let observation_report = parse_json(&observe, "observe");
    assert_eq!(observation_report["results"][0]["verdict"], "consistent");

    let cert_after = Sha256::digest(fs::read(&cert_path).expect("read certificate"));
    assert_eq!(
        cert_before, cert_after,
        "finite observation must leave certificate bytes unchanged"
    );
    verify_cert_both_modes(dir.path(), &cert_path, &source, false);
}

fn patch_lean_verified(cert_path: &Path) {
    let mut cert = read_certificate(cert_path);
    let atom = &mut cert.atoms[0];
    atom.z3_check_result = "lean_verified".to_string();
    atom.z3_result_class = "unknown".to_string();
    atom.translator_version = verification::LEAN_TRANSLATOR_VERSION.to_string();
    atom.bridge_lemma_hash = verification::LEAN_BRIDGE_LEMMA_HASH.to_string();
    atom.lean_result_metadata = Some(LeanResultMetadata {
        status: "lean_verified".to_string(),
        theorem_name: "inc_spec".to_string(),
        translator_version: verification::LEAN_TRANSLATOR_VERSION.to_string(),
        bridge_lemma_hash: verification::LEAN_BRIDGE_LEMMA_HASH.to_string(),
        proof_path: "Generated/Test.lean".to_string(),
        diagnostics: vec![],
        ..Default::default()
    });
    refresh_certificate_integrity(&mut cert);
    fs::write(
        cert_path,
        serde_json::to_string_pretty(&cert).expect("serialize patched certificate"),
    )
    .expect("write patched certificate");
}

#[test]
fn external_tool_success_requires_explicit_lean_promotion_flag() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let source = source_file(dir.path(), VERIFIED_SRC);
    let cert_path = dir.path().join("main.proof-cert.json");

    let verify = certify(dir.path(), &source, &cert_path, &["--json"]);
    assert_exit(&verify, 0);
    patch_lean_verified(&cert_path);

    let without_allow = verify_cert(dir.path(), &cert_path, &source, false, false);
    assert_unproven(&without_allow);
    let with_allow = verify_cert(dir.path(), &cert_path, &source, true, false);
    assert_proven(&with_allow);
}
