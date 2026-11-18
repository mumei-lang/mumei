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
fn input_selection_and_bound_match_the_evidence_family() {
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("key.hex");
    let record = dir.path().join("record.bin");
    let large = dir.path().join("large.bin");
    write(&key, SECRET.as_bytes());
    write(&record, b"data");
    write(&large, &vec![0u8; 65_537]);

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

    // records over the bound are rejected.
    let output = run({
        let mut command = sign_command();
        command
            .arg(&large)
            .args(["--key-file"])
            .arg(&key)
            .args(["--producer-key-id", "producer-a"]);
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
        .stdout(Stdio::piped());
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"stdin record")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        envelope["subject_digest"],
        format!("{:x}", Sha256::digest(b"stdin record"))
    );
}
