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
