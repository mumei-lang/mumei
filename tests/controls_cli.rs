use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture(PathBuf);

impl Fixture {
    fn new(source: &str) -> Self {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("mumei-controls-{id}"));
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("input.mm"), source).unwrap();
        Self(path)
    }

    fn input(&self) -> PathBuf {
        self.0.join("input.mm")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(input: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("controls")
        .arg(input)
        .args(args)
        .output()
        .unwrap()
}

fn parse(stdout: &[u8]) -> Value {
    serde_json::from_slice(stdout).unwrap_or_else(|error| {
        panic!(
            "invalid controls JSON: {error}\n{}",
            String::from_utf8_lossy(stdout)
        )
    })
}

#[test]
fn strong_contract_is_discriminating_and_weak_contract_fails() {
    let strong = Fixture::new(
        "atom add(x: i64, y: i64) -> i64\nrequires: true;\nensures: result == x + y;\nbody: x + y;\n",
    );
    let output = run(&strong.input(), &["--json"]);
    assert_eq!(output.status.code(), Some(0));
    let report = parse(&output.stdout);
    assert_eq!(report["schema"], "mumei.controls-report/v1");
    assert_eq!(report["atoms"][0]["status"], "discriminating");
    assert_eq!(report["atoms"][0]["survived"], 0);

    let weak = Fixture::new(
        "atom add(x: i64, y: i64) -> i64\nrequires: x >= 0 && y >= 0;\nensures: result >= 0;\nbody: x + y + 1;\n",
    );
    let output = run(&weak.input(), &["--json"]);
    assert_eq!(output.status.code(), Some(1));
    let report = parse(&output.stdout);
    assert_eq!(report["status"], "fail");
    assert_eq!(report["atoms"][0]["status"], "weak");
    assert!(report["atoms"][0]["detail"]
        .as_str()
        .unwrap()
        .contains("constant:1->0"));
}

#[test]
fn output_is_deterministic_and_cap_is_per_atom() {
    let fixture = Fixture::new(
        "atom add(x: i64, y: i64) -> i64\nrequires: true;\nensures: true;\nbody: x + y + 1;\n",
    );
    let first = run(&fixture.input(), &["--json", "--max-mutants", "1"]);
    let second = run(&fixture.input(), &["--json", "--max-mutants", "1"]);
    assert_eq!(first.stdout, second.stdout);
    assert_eq!(parse(&first.stdout)["atoms"][0]["tested"], 1);
    assert!(!fixture.0.join(".mumei_cache").exists());
}

#[test]
fn trivial_contract_is_weak_and_immutable_body_has_no_mutants() {
    let weak = Fixture::new(
        "atom identity(x: i64) -> i64\nrequires: true;\nensures: true;\nbody: x + 1;\n",
    );
    let output = run(&weak.input(), &["--json"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(parse(&output.stdout)["atoms"][0]["status"], "weak");

    let immutable = Fixture::new(
        "atom identity(x: i64) -> i64\nrequires: true;\nensures: result == x;\nbody: x;\n",
    );
    let output = run(&immutable.input(), &["--json"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(parse(&output.stdout)["atoms"][0]["status"], "no_mutants");
}

#[test]
fn input_and_atom_selection_errors_use_exit_four() {
    let fixture =
        Fixture::new("atom id(x: i64) -> i64\nrequires: true;\nensures: result == x;\nbody: x;\n");
    assert_eq!(
        run(&fixture.0.join("missing.mm"), &[]).status.code(),
        Some(4)
    );
    assert_eq!(
        run(&fixture.input(), &["--atom", "missing"]).status.code(),
        Some(4)
    );
    let bad = Fixture::new("atom broken(");
    assert_eq!(run(&bad.input(), &[]).status.code(), Some(4));
}

#[test]
fn verify_json_does_not_expose_internal_verdict_metadata() {
    let fixture =
        Fixture::new("atom bad(x: i64) -> i64\nrequires: true;\nensures: result > 0;\nbody: x;\n");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg(fixture.input())
        .arg("--json")
        .output()
        .unwrap();
    let payload = parse(&output.stdout);
    assert_eq!(payload["status"], "failed");
    assert!(payload.get("diagnostics").is_some());
    assert!(payload.to_string().find("\"verdict\"").is_none());
}
