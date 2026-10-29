use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    reports: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "mumei_proof_hash_compat_{}_{}_{}",
            std::process::id(),
            nonce,
            DIR_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir(&root).expect("create exclusive test directory");
        let home = root.join("home");
        let reports = root.join("reports");
        std::fs::create_dir(&home).expect("create isolated home");
        std::fs::create_dir(&reports).expect("create reports directory");
        Self {
            root,
            home,
            reports,
        }
    }

    fn run_verify(&self, source: &Path, certificate: &Path) -> Output {
        let report = self.reports.join(format!(
            "report-{}",
            DIR_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir(&report).expect("create unique report directory");
        Command::new(env!("CARGO_BIN_EXE_mumei"))
            .args([
                "verify",
                "--proof-cert",
                "--output",
                certificate.to_str().expect("certificate path is UTF-8"),
                "--report-dir",
                report.to_str().expect("report path is UTF-8"),
            ])
            .arg(source)
            .current_dir(&self.root)
            .env("HOME", &self.home)
            .env_remove("MUMEI_PROOF_BUNDLE")
            .env_remove("CARGO_MANIFEST_DIR")
            .output()
            .expect("run mumei verify")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn output_text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// This golden pins proof-hash bytes for local transitive, imported, and trusted
/// callees, structs, refined types, and units. It was generated before the
/// dependency-freshness change; regenerate it only for an intended proof-hash
/// change.
#[test]
fn proof_hashes_match_pre_dependency_freshness_golden() {
    let fixture = Fixture::new();
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for name in ["proof_hash_compat.mm", "proof_hash_compat_lib.mm"] {
        std::fs::copy(fixture_dir.join(name), fixture.root.join(name))
            .expect("copy source fixture");
    }

    let certificate = fixture.root.join("cert.json");
    let output = fixture.run_verify(&fixture.root.join("proof_hash_compat.mm"), &certificate);
    assert!(
        output.status.success(),
        "certificate generation failed\n{}",
        output_text(&output)
    );

    let actual_certificate: Value =
        serde_json::from_slice(&std::fs::read(&certificate).expect("read certificate"))
            .expect("parse certificate");
    let actual_atoms = actual_certificate["atoms"]
        .as_array()
        .expect("certificate atoms array")
        .iter()
        .map(|atom| {
            json!({
                "name": atom["name"],
                "status": atom["status"],
                "content_hash": atom["content_hash"],
                "proof_hash": atom["proof_hash"],
            })
        })
        .collect::<Vec<_>>();

    let expected: Value = serde_json::from_slice(
        &std::fs::read(fixture_dir.join("proof_hash_compat.golden.json"))
            .expect("read committed proof-hash golden"),
    )
    .expect("parse committed proof-hash golden");
    let actual = json!({ "atoms": actual_atoms });
    assert_eq!(
        actual,
        expected,
        "proof-hash compatibility drift:\nactual:\n{}",
        serde_json::to_string_pretty(&actual).unwrap()
    );
}
