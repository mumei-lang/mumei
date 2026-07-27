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

fn verify_cert(cert_path: &Path, source_path: &Path, strict: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command
        .arg("verify-cert")
        .arg(cert_path)
        .arg(source_path)
        .current_dir(env!("CARGO_MANIFEST_DIR"));
    if strict {
        command.arg("--strict");
    }
    command.output().expect("run mumei verify-cert")
}

#[test]
fn strict_accepts_a_certificate_that_still_matches_its_source() {
    let dir = fixture_dir("match");
    let source = dir.join("main.mm");
    let cert = dir.join("main.proof.json");
    std::fs::write(&source, SOURCE).expect("write source");
    certify(&source, &cert);

    let output = verify_cert(&cert, &source, true);
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

    let lenient = verify_cert(&cert, &source, false);
    assert!(
        lenient.status.success(),
        "without --strict a drifted certificate stays a warning\nstdout:\n{}",
        String::from_utf8_lossy(&lenient.stdout)
    );

    let strict = verify_cert(&cert, &source, true);
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

    let lenient = verify_cert(&cert, &source, false);
    assert!(
        lenient.status.success(),
        "without --strict an absent certificate_hash stays a warning\nstdout:\n{}",
        String::from_utf8_lossy(&lenient.stdout)
    );

    let strict = verify_cert(&cert, &source, true);
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
