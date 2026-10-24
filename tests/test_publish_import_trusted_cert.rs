use mumei_core::proof_cert::{status, ProofCertificate};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn fresh_dir() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "mumei_publish_trusted_{}_{}_{}",
        std::process::id(),
        nonce,
        DIR_COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("create test directory");
    dir
}

fn run_in(dir: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(args)
        .current_dir(dir)
        .env("HOME", home)
        .output()
        .expect("run mumei")
}

#[test]
fn published_trusted_package_is_accepted_as_a_strict_dependency() {
    let dir = fresh_dir();
    let home = dir.join("home");
    let package = dir.join("trusted_package");
    std::fs::create_dir_all(package.join("src")).expect("create package source directory");
    std::fs::create_dir_all(&home).expect("create isolated home");
    std::fs::write(
        package.join("mumei.toml"),
        "[package]\nname = \"trusted_package\"\nversion = \"0.1.0\"\n",
    )
    .expect("write package manifest");
    std::fs::write(
        package.join("src/main.mm"),
        "trusted atom trusted_value() -> i64\n  requires: true;\n  ensures: result == 7;\n  body: 7;\n",
    )
    .expect("write trusted package source");

    let publish = run_in(&package, &home, &["publish"]);
    assert!(
        publish.status.success(),
        "publish failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&publish.stdout),
        String::from_utf8_lossy(&publish.stderr)
    );

    let published = home.join(".mumei/packages/trusted_package/0.1.0");
    let certificate: ProofCertificate =
        serde_json::from_slice(&std::fs::read(published.join("proof_certificate.json")).unwrap())
            .expect("parse published certificate");
    let trusted = certificate
        .atoms
        .iter()
        .find(|atom| atom.name == "trusted_value")
        .expect("trusted certificate entry");
    assert_eq!(trusted.z3_check_result, status::Z3_SKIPPED);
    assert_eq!(trusted.status, status::TRUSTED);

    let consumer = dir.join("consumer");
    std::fs::create_dir_all(consumer.join("src")).expect("create consumer source directory");
    std::fs::write(
        consumer.join("mumei.toml"),
        format!(
            "[package]\nname = \"consumer\"\nversion = \"0.1.0\"\n\n[dependencies]\ntrusted_package = {{ path = \"{}\" }}\n",
            published.display()
        ),
    )
    .expect("write consumer manifest");
    std::fs::write(
        consumer.join("src/main.mm"),
        "atom consumer() -> i64\n  requires: true;\n  ensures: result == 7;\n  body: trusted_package::trusted_value();\n",
    )
    .expect("write consumer source");

    let verify = run_in(
        &consumer,
        &home,
        &["verify", "--strict-imports", "src/main.mm"],
    );
    assert!(
        verify.status.success(),
        "strict dependency verification failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&verify.stdout),
        String::from_utf8_lossy(&verify.stderr)
    );
}
