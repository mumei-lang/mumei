use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};

// Obviously fake sentinel key material (AGENTS.md: never a real-looking secret).
const SECRET: &str = "1111111111111111111111111111111111111111111111111111111111111111";

fn sign_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command.args(["evidence", "sign"]);
    command
}

fn run(mut output: Command) -> Output {
    output.output().unwrap()
}

fn assert_failure(output: &Output, code: i32) {
    assert_eq!(output.status.code(), Some(code));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(!error.is_empty());
    assert!(!error.contains(SECRET));
    assert!(!error.contains("FAKE_PRIVATE"));
    assert!(!error.contains("/"));
}

fn assert_signed_subject(output: &Output, subject: &[u8]) -> Value {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains(SECRET));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(SECRET));
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        envelope["subject_digest"],
        format!("{:x}", Sha256::digest(subject))
    );
    envelope
}

fn write(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
}

#[test]
fn signed_envelope_binds_record_bytes_and_declared_key_id() {
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("key.hex");
    let record = dir.path().join("record.json");
    write(&key, SECRET.as_bytes());
    write(&record, b"{\"claim\":1}");

    let first = run({
        let mut command = sign_command();
        command.arg(&record).args(["--key-file"]).arg(&key).args([
            "--producer-key-id",
            "producer-a",
            "--json",
        ]);
        command
    });
    assert!(first.status.success(), "{:?}", first.stderr);
    assert!(first.stderr.is_empty());
    let text = String::from_utf8_lossy(&first.stdout);
    assert!(!text.contains(SECRET));
    let envelope: Value = serde_json::from_slice(&first.stdout).unwrap();
    let mut keys: Vec<_> = envelope.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "algorithm",
            "producer_key_id",
            "schema_version",
            "signature",
            "subject_digest"
        ]
    );
    assert_eq!(envelope["schema_version"], "mumei.producer-signature/v1");
    assert_eq!(envelope["algorithm"], "ed25519");
    assert_eq!(envelope["producer_key_id"], "producer-a");
    assert_eq!(
        envelope["subject_digest"],
        format!("{:x}", Sha256::digest(b"{\"claim\":1}"))
    );
    let signature = envelope["signature"].as_str().unwrap();
    assert_eq!(signature.len(), 128);
    assert!(signature
        .chars()
        .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));

    // ed25519 is deterministic and the key id is a declared label: a
    // different label changes only the label field, not the signature.
    let relabeled = run({
        let mut command = sign_command();
        command.arg(&record).args(["--key-file"]).arg(&key).args([
            "--producer-key-id",
            "producer-b",
            "--json",
        ]);
        command
    });
    assert!(relabeled.status.success());
    let other: Value = serde_json::from_slice(&relabeled.stdout).unwrap();
    assert_eq!(other["signature"], envelope["signature"]);
    assert_eq!(other["subject_digest"], envelope["subject_digest"]);
    assert_eq!(other["producer_key_id"], "producer-b");

    // The digest covers the raw bytes: any byte change re-signs.
    write(&record, b"{\"claim\": 1}");
    let modified = run({
        let mut command = sign_command();
        command.arg(&record).args(["--key-file"]).arg(&key).args([
            "--producer-key-id",
            "producer-a",
            "--json",
        ]);
        command
    });
    let changed: Value = serde_json::from_slice(&modified.stdout).unwrap();
    assert_ne!(changed["subject_digest"], envelope["subject_digest"]);
    assert_ne!(changed["signature"], envelope["signature"]);

    // The record is never parsed: non-UTF8 bytes sign the same way.
    write(&record, b"\x00\xff not utf8 \xfe");
    let opaque = run({
        let mut command = sign_command();
        command.arg(&record).args(["--key-file"]).arg(&key).args([
            "--producer-key-id",
            "producer-a",
            "--json",
        ]);
        command
    });
    assert!(opaque.status.success());
    let opaque: Value = serde_json::from_slice(&opaque.stdout).unwrap();
    assert_eq!(
        opaque["subject_digest"],
        format!("{:x}", Sha256::digest(b"\x00\xff not utf8 \xfe"))
    );
}

#[test]
fn non_json_output_is_a_static_scope_summary_without_key_material() {
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("key.hex");
    let record = dir.path().join("record.bin");
    write(&key, SECRET.as_bytes());
    write(&record, b"data");

    let output = run({
        let mut command = sign_command();
        command
            .arg(&record)
            .args(["--key-file"])
            .arg(&key)
            .args(["--producer-key-id", "producer-a"]);
        command
    });
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains(&format!("{:x}", Sha256::digest(b"data"))));
    assert!(text.contains("producer key id"));
    assert!(!text.contains(SECRET));
    assert!(!text.contains("signature\":"));
}

#[test]
fn malformed_key_material_and_empty_key_id_fail_statically() {
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("key.hex");
    let record = dir.path().join("record.bin");
    write(&record, b"data");

    for material in [
        String::new(),
        "ab".repeat(31),                  // short
        "ab".repeat(33),                  // long
        "AB".repeat(32),                  // uppercase
        "zz".repeat(32),                  // non-hex
        format!("{}\n", "ab".repeat(32)), // trailing newline
        format!("0x{}", "ab".repeat(32)), // prefix
        "ab".repeat(64),                  // double length
    ] {
        write(&key, material.as_bytes());
        let output = run({
            let mut command = sign_command();
            command.arg(&record).args(["--key-file"]).arg(&key).args([
                "--producer-key-id",
                "producer-a",
                "--json",
            ]);
            command
        });
        assert_failure(&output, 1);
    }

    // Missing key file and non-regular key file.
    for candidate in [
        dir.path().join("FAKE_PRIVATE_MISSING"),
        dir.path().to_path_buf(),
    ] {
        let output = run({
            let mut command = sign_command();
            command
                .arg(&record)
                .args(["--key-file"])
                .arg(&candidate)
                .args(["--producer-key-id", "producer-a", "--json"]);
            command
        });
        assert_failure(&output, 1);
    }

    // Empty declared key id.
    write(&key, SECRET.as_bytes());
    let output = run({
        let mut command = sign_command();
        command.arg(&record).args(["--key-file"]).arg(&key).args([
            "--producer-key-id",
            "",
            "--json",
        ]);
        command
    });
    assert_failure(&output, 1);
}

#[test]
fn input_selection_and_subject_cap_apply_to_file_and_stdin() {
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("key.hex");
    let record = dir.path().join("record.bin");
    write(&key, SECRET.as_bytes());
    write(&record, b"data");

    // input and --stdin are mutually exclusive.
    let output = run({
        let mut command = sign_command();
        command
            .arg(&record)
            .arg("--stdin")
            .args(["--key-file"])
            .arg(&key)
            .args(["--producer-key-id", "producer-a"]);
        command
    });
    assert_failure(&output, 2);

    let old_limit_plus_one = vec![0x5a; 65_537];
    let larger_than_old_limit = dir.path().join("larger-than-old-limit.bin");
    write(&larger_than_old_limit, &old_limit_plus_one);
    let output = run({
        let mut command = sign_command();
        command
            .arg(&larger_than_old_limit)
            .args(["--key-file"])
            .arg(&key)
            .args(["--producer-key-id", "producer-a", "--json"]);
        command
    });
    assert_signed_subject(&output, &old_limit_plus_one);

    let max_subject = vec![0x6b; 4_194_304];
    let max_subject_file = dir.path().join("max-subject.bin");
    write(&max_subject_file, &max_subject);
    let output = run({
        let mut command = sign_command();
        command
            .arg(&max_subject_file)
            .args(["--key-file"])
            .arg(&key)
            .args(["--producer-key-id", "producer-a", "--json"]);
        command
    });
    assert_signed_subject(&output, &max_subject);

    let oversized_subject = vec![0x7c; max_subject.len() + 1];
    let oversized_file = dir.path().join("oversized.bin");
    write(&oversized_file, &oversized_subject);
    let output = run({
        let mut command = sign_command();
        command
            .arg(&oversized_file)
            .args(["--key-file"])
            .arg(&key)
            .args(["--producer-key-id", "producer-a", "--json"]);
        command
    });
    assert_failure(&output, 1);

    // key files over 64 bytes are rejected.
    write(&key, &format!("{}\n", SECRET).into_bytes());
    let output = run({
        let mut command = sign_command();
        command
            .arg(&record)
            .args(["--key-file"])
            .arg(&key)
            .args(["--producer-key-id", "producer-a"]);
        command
    });
    assert_failure(&output, 1);
    write(&key, SECRET.as_bytes());

    // --stdin reads the record bytes.
    let mut command = sign_command();
    command
        .arg("--stdin")
        .args(["--key-file"])
        .arg(&key)
        .args(["--producer-key-id", "producer-a", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(&old_limit_plus_one)
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_signed_subject(&output, &old_limit_plus_one);

    let mut command = sign_command();
    command
        .arg("--stdin")
        .args(["--key-file"])
        .arg(&key)
        .args(["--producer-key-id", "producer-a", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(&oversized_subject)
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_failure(&output, 1);
}

fn keygen_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command.args(["evidence", "keygen"]);
    command
}

#[test]
fn keygen_writes_secret_file_and_prints_only_public_key() {
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("key.hex");
    let output = run({
        let mut command = keygen_command();
        command.args(["--secret-file"]).arg(&key);
        command
    });
    assert!(output.status.success(), "{:?}", output.stderr);

    // The file holds exactly the 64-char lowercase-hex secret --key-file
    // accepts; stdout carries the public key only (a public value).
    let secret = fs::read(&key).unwrap();
    assert_eq!(secret.len(), 64);
    assert!(secret
        .iter()
        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let public = stdout
        .lines()
        .find_map(|line| line.strip_prefix("Public key: "))
        .unwrap();
    assert_eq!(public.len(), 64);
    let secret_text = String::from_utf8_lossy(&secret);
    assert!(!stdout.contains(secret_text.as_ref()));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(secret_text.as_ref()));

    // The generated secret signs; the printed public key is its pair.
    let record = dir.path().join("record.json");
    write(&record, b"{\"claim\":3}");
    let output = run({
        let mut command = sign_command();
        command.arg(&record).args(["--key-file"]).arg(&key).args([
            "--producer-key-id",
            "producer-a",
            "--json",
        ]);
        command
    });
    assert!(output.status.success(), "{:?}", output.stderr);
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["signature"].as_str().unwrap().len(), 128);

    // The printed public key really is the generated secret's pair: it
    // verifies the envelope's signature over the subject digest, which
    // is what `signature-verify` checks consumer-side.
    let verifying_key =
        ed25519_dalek::VerifyingKey::from_bytes(&hex_bytes(public).try_into().unwrap()).unwrap();
    let signature = ed25519_dalek::Signature::from_bytes(
        &hex_bytes(envelope["signature"].as_str().unwrap())
            .try_into()
            .unwrap(),
    );
    use ed25519_dalek::Verifier;
    assert!(verifying_key
        .verify(
            envelope["subject_digest"].as_str().unwrap().as_bytes(),
            &signature,
        )
        .is_ok());

    // An existing file is never clobbered.
    let output = run({
        let mut command = keygen_command();
        command.args(["--secret-file"]).arg(&key);
        command
    });
    assert_failure(&output, 1);
    // Compare without printing the secret: `assert_eq!` would dump the
    // key bytes into the failure diagnostic.
    assert!(secret == fs::read(&key).unwrap());
}

fn hex_bytes(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).unwrap())
        .collect()
}

#[test]
fn produced_at_stamps_the_envelope_and_is_rejected_when_not_rfc3339() {
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("key.hex");
    let record = dir.path().join("record.json");
    write(&key, SECRET.as_bytes());
    write(&record, b"{\"claim\":1}");

    // `--produced-at` stamps a declared timestamp; the signature is
    // unaffected since the signed payload is the digest alone.
    let stamped = run({
        let mut command = sign_command();
        command.arg(&record).args(["--key-file"]).arg(&key).args([
            "--producer-key-id",
            "producer-a",
            "--produced-at",
            "2026-01-01T00:00:00Z",
            "--json",
        ]);
        command
    });
    assert!(stamped.status.success(), "{:?}", stamped.stderr);
    let envelope: Value = serde_json::from_slice(&stamped.stdout).unwrap();
    assert_eq!(envelope["produced_at"], "2026-01-01T00:00:00Z");

    let plain = run({
        let mut command = sign_command();
        command.arg(&record).args(["--key-file"]).arg(&key).args([
            "--producer-key-id",
            "producer-a",
            "--json",
        ]);
        command
    });
    let plain_env: Value = serde_json::from_slice(&plain.stdout).unwrap();
    assert_eq!(envelope["signature"], plain_env["signature"]);

    // A malformed timestamp is rejected before signing.
    let bad = run({
        let mut command = sign_command();
        command.arg(&record).args(["--key-file"]).arg(&key).args([
            "--producer-key-id",
            "producer-a",
            "--produced-at",
            "yesterday",
            "--json",
        ]);
        command
    });
    assert_failure(&bad, 1);
}
