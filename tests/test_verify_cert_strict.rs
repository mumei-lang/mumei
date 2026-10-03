use mumei_core::proof_cert::{refresh_certificate_integrity, LeanResultMetadata, ProofCertificate};
use mumei_core::verification;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SOURCE: &str = r#"
atom clamp_low(x: i64) -> i64
  requires: x >= 0;
  ensures: result >= 0;
  body: x;
"#;

const DRIFTED_SOURCE: &str = r#"
atom clamp_low(x: i64) -> i64
  requires: x >= 0;
  ensures: result >= 1;
  body: x;
"#;

fn fixture_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mumei_verify_cert_strict_{}_{}",
        name,
        std::process::id()
    ));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale verify-cert fixture dir");
    }
    std::fs::create_dir_all(&dir).expect("create verify-cert fixture dir");
    dir
}

fn certify(source_path: &Path, cert_path: &Path) {
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--proof-cert")
        .arg("--output")
        .arg(cert_path)
        .arg(source_path)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run mumei verify --proof-cert");
    assert!(
        output.status.success(),
        "certificate generation failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn verify_cert(
    cert_path: &Path,
    source_path: &Path,
    allow_lean_verified: bool,
    strict: bool,
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command
        .arg("verify-cert")
        .arg(cert_path)
        .arg(source_path)
        .current_dir(env!("CARGO_MANIFEST_DIR"));
    if allow_lean_verified {
        command.arg("--allow-lean-verified");
    }
    if strict {
        command.arg("--strict");
    }
    command.output().expect("run mumei verify-cert")
}

fn patch_lean_verified(
    cert_path: &Path,
    axiom_audit: Option<&str>,
    kernel_axioms: Option<&[&str]>,
) {
    let raw = std::fs::read_to_string(cert_path).expect("read certificate");
    let mut cert: ProofCertificate = serde_json::from_str(&raw).expect("deserialize certificate");
    let atom = &mut cert.atoms[0];
    atom.z3_check_result = "lean_verified".to_string();
    atom.z3_result_class = "unknown".to_string();
    atom.translator_version = verification::LEAN_TRANSLATOR_VERSION.to_string();
    atom.bridge_lemma_hash = verification::LEAN_BRIDGE_LEMMA_HASH.to_string();
    atom.lean_result_metadata = Some(LeanResultMetadata {
        status: "lean_verified".to_string(),
        theorem_name: "clamp_low_spec".to_string(),
        translator_version: verification::LEAN_TRANSLATOR_VERSION.to_string(),
        bridge_lemma_hash: verification::LEAN_BRIDGE_LEMMA_HASH.to_string(),
        proof_path: "Generated/Test.lean".to_string(),
        diagnostics: vec![],
        kernel_axioms: kernel_axioms
            .map(|axioms| axioms.iter().map(|axiom| (*axiom).to_string()).collect()),
        axiom_audit: axiom_audit.map(str::to_string),
        ..Default::default()
    });
    refresh_certificate_integrity(&mut cert);
    std::fs::write(
        cert_path,
        serde_json::to_string_pretty(&cert).expect("serialize certificate"),
    )
    .expect("write patched certificate");
}

#[test]
fn strict_accepts_a_certificate_that_still_matches_its_source() {
    let dir = fixture_dir("match");
    let source = dir.join("main.mm");
    let cert = dir.join("main.proof.json");
    std::fs::write(&source, SOURCE).expect("write source");
    certify(&source, &cert);

    let output = verify_cert(&cert, &source, false, true);
    assert!(
        output.status.success(),
        "--strict should accept an up-to-date certificate\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn strict_rejects_a_certificate_whose_source_drifted() {
    let dir = fixture_dir("drift");
    let source = dir.join("main.mm");
    let cert = dir.join("main.proof.json");
    std::fs::write(&source, SOURCE).expect("write source");
    certify(&source, &cert);
    std::fs::write(&source, DRIFTED_SOURCE).expect("write drifted source");

    let lenient = verify_cert(&cert, &source, false, false);
    assert!(
        lenient.status.success(),
        "without --strict a drifted certificate stays a warning\nstdout:\n{}",
        String::from_utf8_lossy(&lenient.stdout)
    );

    let strict = verify_cert(&cert, &source, false, true);
    assert!(
        !strict.status.success(),
        "--strict should reject a drifted certificate\nstdout:\n{}",
        String::from_utf8_lossy(&strict.stdout)
    );
    let stderr = String::from_utf8_lossy(&strict.stderr);
    assert!(
        stderr.contains("--strict: certificate"),
        "expected a --strict rejection message, got:\n{stderr}"
    );
}

#[test]
fn strict_rejects_a_certificate_without_a_certificate_hash() {
    let dir = fixture_dir("nohash");
    let source = dir.join("main.mm");
    let cert = dir.join("main.proof.json");
    std::fs::write(&source, SOURCE).expect("write source");
    certify(&source, &cert);

    let raw = std::fs::read_to_string(&cert).expect("read certificate");
    let mut parsed: serde_json::Value = serde_json::from_str(&raw).expect("parse certificate");
    parsed["certificate_hash"] = serde_json::Value::String(String::new());
    std::fs::write(&cert, parsed.to_string()).expect("write hashless certificate");

    let lenient = verify_cert(&cert, &source, false, false);
    assert!(
        lenient.status.success(),
        "without --strict an absent certificate_hash stays a warning\nstdout:\n{}",
        String::from_utf8_lossy(&lenient.stdout)
    );

    let strict = verify_cert(&cert, &source, false, true);
    assert!(
        !strict.status.success(),
        "--strict should reject a certificate with no re-derivable hash\nstdout:\n{}",
        String::from_utf8_lossy(&strict.stdout)
    );
    assert!(
        String::from_utf8_lossy(&strict.stderr).contains("certificate_hash absent"),
        "expected the absent-hash reason in stderr"
    );
}

#[test]
fn verify_cert_reports_rejected_audits_and_strict_requires_audit_metadata() {
    let dir = fixture_dir("axiom_audit");
    let source = dir.join("main.mm");
    let cert = dir.join("main.proof.json");
    std::fs::write(&source, SOURCE).expect("write source");
    certify(&source, &cert);

    patch_lean_verified(&cert, Some("passed"), Some(&["sorryAx"]));
    let rejected = verify_cert(&cert, &source, true, false);
    assert!(
        !rejected.status.success(),
        "a disallowed kernel axiom must fail verification\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    let rejected_stdout = String::from_utf8_lossy(&rejected.stdout);
    assert!(rejected_stdout.contains("axiom_rejected"));
    assert!(rejected_stdout.contains("kernel_axioms: [\"sorryAx\"]"));
    assert!(rejected_stdout.contains("axiom_audit: rejected"));

    patch_lean_verified(&cert, None, None);
    let lenient = verify_cert(&cert, &source, true, false);
    assert!(
        lenient.status.success(),
        "legacy unaudited certificates remain accepted without --strict\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&lenient.stdout),
        String::from_utf8_lossy(&lenient.stderr)
    );
    assert!(String::from_utf8_lossy(&lenient.stdout).contains("axiom_audit: unaudited"));

    let strict = verify_cert(&cert, &source, true, true);
    assert!(
        !strict.status.success(),
        "--strict must reject accepted unaudited Lean certificates\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&strict.stdout),
        String::from_utf8_lossy(&strict.stderr)
    );
    assert!(String::from_utf8_lossy(&strict.stderr).contains("unaudited"));
}
