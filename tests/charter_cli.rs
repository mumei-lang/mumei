use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct TestDir(PathBuf);

impl TestDir {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "mumei_charter_cli_{label}_{}_{}",
            std::process::id(),
            nonce
        ));
        std::fs::create_dir_all(&path).expect("create test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run mumei")
}

fn lock_charter(dir: &Path, charter: &str) {
    let output = run(dir, &["charter", "lock", "--charter", charter]);
    assert!(
        output.status.success(),
        "charter lock failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn certify(dir: &Path, source: &str) {
    let output = run(dir, &["verify", "--proof-cert", source]);
    assert!(
        output.status.success(),
        "verification failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(dir.join(source).with_extension("proof.json").exists());
}

fn check_with_cert(dir: &Path, args: &[&str]) -> (Output, Value) {
    let output = run(dir, args);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "parse charter report: {error}\n{}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    (output, report)
}

fn setup_explicit_files(dir: &Path) {
    std::fs::create_dir_all(dir.join("specs")).expect("create specs directory");
    std::fs::write(
        dir.join("specs/core.charter.mm"),
        "trusted atom increase(x: i64) -> i64\n\
         requires: x >= 0;\n\
         ensures: result >= x + 1;\n\
         body: 0;\n",
    )
    .expect("write charter source");
    std::fs::write(
        dir.join("impl.mm"),
        "atom increase(x: i64) -> i64\n\
         requires: x >= 0;\n\
         ensures: result >= x + 1;\n\
         body: 0;\n",
    )
    .expect("write implementation source");
}

#[test]
fn lock_check_json_and_gate_exit_codes() {
    let dir = TestDir::new("explicit");
    setup_explicit_files(dir.path());
    let lock = run(
        dir.path(),
        &[
            "charter",
            "lock",
            "--charter",
            "specs/core.charter.mm",
            "--lock",
            "charter.lock",
        ],
    );
    assert_eq!(
        lock.status.code(),
        Some(0),
        "lock failed:\n{}",
        String::from_utf8_lossy(&lock.stderr)
    );
    let first_lock = std::fs::read(dir.path().join("charter.lock")).expect("read lock");
    let repeated_lock = run(
        dir.path(),
        &[
            "charter",
            "lock",
            "--charter",
            "specs/core.charter.mm",
            "--lock",
            "charter.lock",
        ],
    );
    assert_eq!(repeated_lock.status.code(), Some(0));
    assert_eq!(
        first_lock,
        std::fs::read(dir.path().join("charter.lock")).expect("read repeated lock")
    );

    let check = run(
        dir.path(),
        &[
            "charter",
            "check",
            "impl.mm",
            "--charter",
            "specs/core.charter.mm",
            "--lock",
            "charter.lock",
            "--json",
        ],
    );
    assert_eq!(
        check.status.code(),
        Some(0),
        "check failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );
    let report: Value = serde_json::from_slice(&check.stdout).expect("parse stdout report");
    assert_eq!(report["schema"], "mumei.charter-report/v1");
    assert_eq!(report["status"], "pass");
    assert_eq!(report["lock"]["status"], "ok");
    assert_eq!(report["atoms"][0]["charter"], "specs/core.charter.mm");
    assert_eq!(report["atoms"][0]["name"], "increase");
    assert_eq!(report["atoms"][0]["status"], "ok");
    assert!(dir.path().join("charter_report.json").exists());

    std::fs::write(
        dir.path().join("specs/core.charter.mm"),
        "trusted atom increase(x: i64) -> i64\n\
         requires: x >= 0;\n\
         ensures: result >= x + 1;\n\
         body: 0;\n\n",
    )
    .expect("edit charter");
    let changed = run(
        dir.path(),
        &[
            "charter",
            "check",
            "impl.mm",
            "--charter",
            "specs/core.charter.mm",
            "--lock",
            "charter.lock",
            "--json",
        ],
    );
    assert_eq!(changed.status.code(), Some(1));
    let report: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("charter_report.json")).unwrap())
            .expect("parse failed report");
    assert_eq!(report["lock"]["status"], "mismatch");
    assert_eq!(report["status"], "fail");
    assert_eq!(report["atoms"][0]["status"], "unpinned");
    assert_eq!(
        report["atoms"][0]["detail"],
        "checked against an unpinned charter text"
    );

    let missing_input = run(
        dir.path(),
        &[
            "charter",
            "check",
            "missing.mm",
            "--charter",
            "specs/core.charter.mm",
            "--lock",
            "charter.lock",
        ],
    );
    assert_eq!(missing_input.status.code(), Some(4));
}

#[test]
fn manifest_files_and_default_lock_are_used() {
    let dir = TestDir::new("manifest");
    setup_explicit_files(dir.path());
    std::fs::write(
        dir.path().join("mumei.toml"),
        "[package]\nname = \"charter-test\"\nversion = \"0.1.0\"\n\n\
         [charter]\nfiles = [\"specs/core.charter.mm\"]\n",
    )
    .expect("write manifest");

    let lock = run(dir.path(), &["charter", "lock"]);
    assert_eq!(
        lock.status.code(),
        Some(0),
        "manifest lock failed:\n{}",
        String::from_utf8_lossy(&lock.stderr)
    );
    assert!(dir.path().join("charter.lock").exists());
    let check = run(dir.path(), &["charter", "check", "impl.mm"]);
    assert_eq!(
        check.status.code(),
        Some(0),
        "manifest check failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );
    let report: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("charter_report.json")).unwrap())
            .expect("parse manifest report");
    assert_eq!(report["atoms"][0]["status"], "ok");
}

#[test]
fn precondition_and_postcondition_failures_exit_one_with_counterexamples() {
    let dir = TestDir::new("refinement");
    std::fs::write(
        dir.path().join("spec.charter.mm"),
        "trusted atom increase(x: i64) -> i64\n\
         requires: x >= 0;\n\
         ensures: result >= x + 1;\n\
         body: 0;\n",
    )
    .expect("write charter");
    let locked = run(
        dir.path(),
        &["charter", "lock", "--charter", "spec.charter.mm"],
    );
    assert_eq!(locked.status.code(), Some(0));

    std::fs::write(
        dir.path().join("impl.mm"),
        "atom increase(x: i64) -> i64\n\
         requires: x > 0;\n\
         ensures: result >= x;\n\
         body: 0;\n",
    )
    .expect("write implementation");
    let check = run(
        dir.path(),
        &[
            "charter",
            "check",
            "impl.mm",
            "--charter",
            "spec.charter.mm",
            "--json",
        ],
    );
    assert_eq!(check.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&check.stdout).expect("parse report");
    assert_eq!(report["atoms"][0]["status"], "pre_weakened");
    assert!(report["atoms"][0]["counterexample"]["x"].is_string());

    std::fs::write(
        dir.path().join("impl.mm"),
        "atom increase(x: i64) -> i64\n\
         requires: x >= 0;\n\
         ensures: result >= x;\n\
         body: 0;\n",
    )
    .expect("rewrite implementation");
    let check = run(
        dir.path(),
        &[
            "charter",
            "check",
            "impl.mm",
            "--charter",
            "spec.charter.mm",
            "--json",
        ],
    );
    assert_eq!(check.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&check.stdout).expect("parse report");
    assert_eq!(report["atoms"][0]["status"], "post_weakened");
    assert!(report["atoms"][0]["counterexample"]["x"].is_string());
}

#[test]
fn broken_manifest_is_an_input_error_with_explicit_charters() {
    let dir = TestDir::new("broken-manifest");
    setup_explicit_files(dir.path());
    std::fs::write(dir.path().join("mumei.toml"), "[package").expect("write broken manifest");

    let check = run(
        dir.path(),
        &[
            "charter",
            "check",
            "impl.mm",
            "--charter",
            "specs/core.charter.mm",
        ],
    );
    assert_eq!(check.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&check.stderr).contains("manifest"));
}

#[test]
fn explicit_charter_does_not_skip_other_pinned_charters() {
    let dir = TestDir::new("all-pinned");
    std::fs::write(
        dir.path().join("first.charter.mm"),
        "trusted atom first(x: i64) -> i64 ensures: result >= x; body: 0;",
    )
    .expect("write first charter");
    std::fs::write(
        dir.path().join("second.charter.mm"),
        "trusted atom second(x: i64) -> i64 ensures: result >= x + 1; body: 0;",
    )
    .expect("write second charter");
    std::fs::write(
        dir.path().join("impl.mm"),
        "atom first(x: i64) -> i64 ensures: result >= x; body: 0;\n\
         atom second(x: i64) -> i64 ensures: result >= x; body: 0;",
    )
    .expect("write implementation");
    let lock = run(
        dir.path(),
        &[
            "charter",
            "lock",
            "--charter",
            "first.charter.mm",
            "--charter",
            "second.charter.mm",
        ],
    );
    assert_eq!(lock.status.code(), Some(0));

    let check = run(
        dir.path(),
        &[
            "charter",
            "check",
            "impl.mm",
            "--charter",
            "first.charter.mm",
            "--json",
        ],
    );
    assert_eq!(check.status.code(), Some(1));
    let report: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("charter_report.json")).unwrap())
            .expect("parse report");
    assert_eq!(report["atoms"].as_array().unwrap().len(), 2);
    assert_eq!(report["atoms"][1]["name"], "second");
    assert_eq!(report["atoms"][0]["charter"], "first.charter.mm");
    assert_eq!(report["atoms"][1]["charter"], "second.charter.mm");
    assert_eq!(report["atoms"][1]["status"], "post_weakened");
}

#[test]
fn generic_charter_atom_matches_pre_monomorphized_implementation() {
    let dir = TestDir::new("generic");
    std::fs::write(
        dir.path().join("generic.charter.mm"),
        "trusted atom identity<T>(x: T) -> T body: 0;",
    )
    .expect("write generic charter");
    std::fs::write(
        dir.path().join("impl.mm"),
        "atom identity<T>(x: T) -> T body: 0;",
    )
    .expect("write generic implementation");
    let lock = run(
        dir.path(),
        &["charter", "lock", "--charter", "generic.charter.mm"],
    );
    assert_eq!(lock.status.code(), Some(0));

    let check = run(
        dir.path(),
        &[
            "charter",
            "check",
            "impl.mm",
            "--charter",
            "generic.charter.mm",
            "--json",
        ],
    );
    assert!(matches!(check.status.code(), Some(0 | 1)));
    let report: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("charter_report.json")).unwrap())
            .expect("parse generic report");
    assert_ne!(report["atoms"][0]["status"], "missing");
    assert!(report["atoms"][0].get("effective_grade").is_none());
    assert!(report["atoms"][0].get("effective_grade_complete").is_none());
}

#[test]
fn generic_method_candidate_is_not_ambiguous() {
    let dir = TestDir::new("generic-method");
    std::fs::write(
        dir.path().join("generic.charter.mm"),
        "trusted atom get<T>(self: Box, value: T) -> T body: 0;",
    )
    .expect("write generic charter");
    std::fs::write(
        dir.path().join("impl.mm"),
        "struct Box { value: i64 }\n\
         impl Box { atom get<T>(self: Box, value: T) -> T body: value; }",
    )
    .expect("write generic implementation");
    let lock = run(
        dir.path(),
        &["charter", "lock", "--charter", "generic.charter.mm"],
    );
    assert_eq!(lock.status.code(), Some(0));

    let check = run(
        dir.path(),
        &[
            "charter",
            "check",
            "impl.mm",
            "--charter",
            "generic.charter.mm",
            "--json",
        ],
    );
    let report: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("charter_report.json")).unwrap())
            .expect("parse generic method report");
    assert!(matches!(check.status.code(), Some(0 | 1)));
    assert!(matches!(
        report["atoms"][0]["status"].as_str(),
        Some("ok" | "inconclusive")
    ));
    assert!(!report["atoms"][0]["detail"]
        .as_str()
        .unwrap_or_default()
        .contains("ambiguous"));
}

#[test]
fn generic_implementation_claim_is_included_in_grade_checks() {
    let dir = TestDir::new("generic-grade");
    std::fs::write(
        dir.path().join("generic.charter.mm"),
        "trusted atom identity<T>(x: i64) -> i64 requires: true; ensures: result == x; body: 0;\n\
         trusted atom get<T>(self: Box, x: i64) -> i64 requires: true; ensures: result == x; body: 0;",
    )
    .expect("write generic charter");
    std::fs::write(
        dir.path().join("impl.mm"),
        "struct Box { value: i64 }\n\
         trusted atom identity<T>(x: i64) -> i64 requires: true; ensures: result == x; body: x;\n\
         impl Box { atom get<T>(self: Box, x: i64) -> i64 requires: true; ensures: result == x; body: x; }",
    )
    .expect("write generic implementation");
    lock_charter(dir.path(), "generic.charter.mm");
    certify(dir.path(), "impl.mm");

    let (g0_check, g0_report) = check_with_cert(
        dir.path(),
        &[
            "charter",
            "check",
            "impl.mm",
            "--charter",
            "generic.charter.mm",
            "--cert",
            "impl.proof.json",
            "--require-grade",
            "G0",
            "--json",
        ],
    );
    assert_eq!(g0_check.status.code(), Some(0));
    let g0_atoms = g0_report["atoms"]
        .as_array()
        .expect("generic charter atom results");
    assert_eq!(g0_atoms.len(), 2);
    for atom in g0_atoms {
        let grade = atom["effective_grade"]
            .as_str()
            .expect("generic implementation claim has an effective grade");
        assert_eq!(atom["status"], "ok");
        assert_ne!(atom["detail"], "no claim for implementation atom");
        assert!(atom["effective_grade_complete"].is_boolean());
        println!(
            "observed generic implementation effective grade for {}: {grade}",
            atom["name"]
        );
    }

    let (default_check, default_report) = check_with_cert(
        dir.path(),
        &[
            "charter",
            "check",
            "impl.mm",
            "--charter",
            "generic.charter.mm",
            "--cert",
            "impl.proof.json",
            "--json",
        ],
    );
    let default_atoms = default_report["atoms"]
        .as_array()
        .expect("generic charter atom results");
    let mut all_machine_sealed = true;
    for g0_atom in g0_atoms {
        let name = g0_atom["name"].as_str().expect("atom name");
        let grade = g0_atom["effective_grade"]
            .as_str()
            .expect("generic implementation grade");
        let default_atom = default_atoms
            .iter()
            .find(|atom| atom["name"] == name)
            .expect("matching default-grade atom result");
        assert_eq!(default_atom["effective_grade"], grade);
        if matches!(grade, "G4" | "G5") {
            assert_eq!(default_atom["status"], "ok");
        } else {
            all_machine_sealed = false;
            assert_eq!(default_atom["status"], "grade_below");
            assert_eq!(
                default_atom["detail"],
                format!("effective grade {grade} is below the required G4")
            );
        }
    }
    assert_eq!(
        default_check.status.code(),
        Some(if all_machine_sealed { 0 } else { 1 })
    );
}

#[test]
fn certified_g4_atom_passes_and_reports_grade_fields() {
    let dir = TestDir::new("grade-g4");
    std::fs::write(
        dir.path().join("spec.charter.mm"),
        "trusted atom identity(x: i64) -> i64 requires: true; ensures: result == x; body: 0;",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("impl.mm"),
        "atom identity(x: i64) -> i64 requires: true; ensures: result == x; body: x;",
    )
    .unwrap();
    lock_charter(dir.path(), "spec.charter.mm");
    certify(dir.path(), "impl.mm");

    let (check, report) = check_with_cert(
        dir.path(),
        &[
            "charter",
            "check",
            "impl.mm",
            "--charter",
            "spec.charter.mm",
            "--cert",
            "impl.proof.json",
            "--json",
        ],
    );
    assert_eq!(check.status.code(), Some(0));
    assert_eq!(report["atoms"][0]["effective_grade"], "G4");
    assert_eq!(report["atoms"][0]["effective_grade_complete"], true);
}

#[test]
fn trusted_atom_fails_default_grade_but_passes_g1() {
    let dir = TestDir::new("grade-trusted");
    std::fs::write(
        dir.path().join("spec.charter.mm"),
        "trusted atom identity(x: i64) -> i64 requires: true; ensures: result == x; body: 0;",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("impl.mm"),
        "trusted atom identity(x: i64) -> i64 requires: true; ensures: result == x; body: x;",
    )
    .unwrap();
    lock_charter(dir.path(), "spec.charter.mm");
    certify(dir.path(), "impl.mm");

    let (default_check, default_report) = check_with_cert(
        dir.path(),
        &[
            "charter",
            "check",
            "impl.mm",
            "--charter",
            "spec.charter.mm",
            "--cert",
            "impl.proof.json",
            "--json",
        ],
    );
    assert_eq!(default_check.status.code(), Some(1));
    assert_eq!(default_report["atoms"][0]["status"], "grade_below");
    assert_eq!(default_report["atoms"][0]["effective_grade"], "G1");
    assert_eq!(
        default_report["atoms"][0]["detail"],
        "effective grade G1 is below the required G4"
    );

    let (g1_check, g1_report) = check_with_cert(
        dir.path(),
        &[
            "charter",
            "check",
            "impl.mm",
            "--charter",
            "spec.charter.mm",
            "--cert",
            "impl.proof.json",
            "--require-grade",
            "G1",
            "--json",
        ],
    );
    assert_eq!(g1_check.status.code(), Some(0));
    assert_eq!(g1_report["atoms"][0]["status"], "ok");
}

#[test]
fn trusted_helper_lowers_caller_effective_grade() {
    let dir = TestDir::new("grade-helper");
    std::fs::write(
        dir.path().join("spec.charter.mm"),
        "trusted atom trusted_caller() -> i64 requires: true; ensures: result == 7; body: 0;",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("impl.mm"),
        "trusted atom trusted_leaf() -> i64 requires: true; ensures: result == 7; body: 7;\n\
         atom trusted_caller() -> i64 requires: true; ensures: result == 7; body: trusted_leaf();",
    )
    .unwrap();
    lock_charter(dir.path(), "spec.charter.mm");
    certify(dir.path(), "impl.mm");

    let (check, report) = check_with_cert(
        dir.path(),
        &[
            "charter",
            "check",
            "impl.mm",
            "--charter",
            "spec.charter.mm",
            "--cert",
            "impl.proof.json",
            "--json",
        ],
    );
    assert_eq!(check.status.code(), Some(1));
    assert_eq!(report["atoms"][0]["effective_grade"], "G1");
    assert_eq!(report["atoms"][0]["status"], "grade_below");
}

#[test]
fn source_drift_lowers_effective_grade_and_fails_gate() {
    let dir = TestDir::new("grade-drift");
    std::fs::write(
        dir.path().join("spec.charter.mm"),
        "trusted atom identity(x: i64) -> i64 requires: true; ensures: result == x; body: 0;",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("impl.mm"),
        "atom identity(x: i64) -> i64 requires: true; ensures: result == x; body: x;",
    )
    .unwrap();
    lock_charter(dir.path(), "spec.charter.mm");
    certify(dir.path(), "impl.mm");
    std::fs::write(
        dir.path().join("impl.mm"),
        "atom identity(x: i64) -> i64 requires: true; ensures: result == x; body: x + 0;",
    )
    .unwrap();

    let (check, report) = check_with_cert(
        dir.path(),
        &[
            "charter",
            "check",
            "impl.mm",
            "--charter",
            "spec.charter.mm",
            "--cert",
            "impl.proof.json",
            "--json",
        ],
    );
    assert_eq!(check.status.code(), Some(1));
    assert_eq!(report["atoms"][0]["status"], "grade_below");
    assert_eq!(report["atoms"][0]["effective_grade"], "G0");
}

#[test]
fn method_claim_is_found_by_qualified_name() {
    let dir = TestDir::new("grade-method");
    std::fs::write(
        dir.path().join("spec.charter.mm"),
        "trusted atom get(self: Box) -> i64 requires: true; ensures: result == 0; body: 0;",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("impl.mm"),
        "struct Box { value: i64 }\n\
         impl Box { atom get(self: Box) -> i64 requires: true; ensures: result == 0; body: 0; }",
    )
    .unwrap();
    lock_charter(dir.path(), "spec.charter.mm");
    certify(dir.path(), "impl.mm");

    let (check, report) = check_with_cert(
        dir.path(),
        &[
            "charter",
            "check",
            "impl.mm",
            "--charter",
            "spec.charter.mm",
            "--cert",
            "impl.proof.json",
            "--json",
        ],
    );
    assert_eq!(check.status.code(), Some(0));
    assert_eq!(report["atoms"][0]["effective_grade"], "G4");
}

#[test]
fn grade_requires_a_certificate_and_missing_certificate_is_an_input_error() {
    let dir = TestDir::new("grade-input-errors");
    setup_explicit_files(dir.path());
    lock_charter(dir.path(), "specs/core.charter.mm");

    let without_cert = run(
        dir.path(),
        &[
            "charter",
            "check",
            "impl.mm",
            "--charter",
            "specs/core.charter.mm",
            "--require-grade",
            "G1",
        ],
    );
    assert_eq!(without_cert.status.code(), Some(4));

    let missing_cert = run(
        dir.path(),
        &[
            "charter",
            "check",
            "impl.mm",
            "--charter",
            "specs/core.charter.mm",
            "--cert",
            "missing.proof.json",
        ],
    );
    assert_eq!(missing_cert.status.code(), Some(4));
}

#[test]
fn duplicate_atom_names_are_attributed_to_charter_paths() {
    let dir = TestDir::new("duplicate-names");
    std::fs::write(
        dir.path().join("a.charter.mm"),
        "trusted atom f(x: i64) -> i64 ensures: result >= x; body: 0;",
    )
    .expect("write first charter");
    std::fs::write(
        dir.path().join("b.charter.mm"),
        "trusted atom f(x: i64) -> i64 ensures: result >= x + 1; body: 0;",
    )
    .expect("write second charter");
    std::fs::write(
        dir.path().join("impl.mm"),
        "atom f(x: i64) -> i64 ensures: result >= x; body: 0;",
    )
    .expect("write implementation");
    let lock = run(
        dir.path(),
        &[
            "charter",
            "lock",
            "--charter",
            "b.charter.mm",
            "--charter",
            "a.charter.mm",
        ],
    );
    assert_eq!(lock.status.code(), Some(0));

    let _ = run(
        dir.path(),
        &[
            "charter",
            "check",
            "impl.mm",
            "--charter",
            "b.charter.mm",
            "--charter",
            "a.charter.mm",
            "--json",
        ],
    );
    let report: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("charter_report.json")).unwrap())
            .expect("parse duplicate-name report");
    assert_eq!(report["atoms"].as_array().unwrap().len(), 2);
    assert_eq!(report["atoms"][0]["charter"], "a.charter.mm");
    assert_eq!(report["atoms"][1]["charter"], "b.charter.mm");
    assert_eq!(report["atoms"][0]["name"], "f");
    assert_eq!(report["atoms"][1]["name"], "f");
}
