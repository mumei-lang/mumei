// Capability Model Stage 1: capability declarations and capability-typed
// parameters must produce verdicts equivalent to the existing effect system.
use std::path::{Path, PathBuf};
use std::process::Command;

struct VerifyOutcome {
    success: bool,
    stdout: String,
    stderr: String,
}

fn verify(source_path: &Path, report_dir: &Path) -> VerifyOutcome {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let output = Command::new(bin)
        .arg("verify")
        .arg("--report-dir")
        .arg(report_dir)
        .arg(source_path)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify: {err}"));

    VerifyOutcome {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn scratch_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mumei_capability_{}_{}", tag, std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale scratch dir");
    }
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

fn write_source(dir: &Path, name: &str, source: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, source).expect("write .mm fixture");
    path
}

#[test]
fn capability_parameter_fixture_verifies() {
    let report_dir = scratch_dir("stage1_positive");
    let outcome = verify(Path::new("tests/test_capability_stage1.mm"), &report_dir);
    assert!(
        outcome.success,
        "capability Stage 1 fixture should verify\nstdout:\n{}\nstderr:\n{}",
        outcome.stdout, outcome.stderr
    );
    std::fs::remove_dir_all(report_dir).expect("remove scratch dir");
}

#[test]
fn capability_parameter_without_declared_effect_is_rejected() {
    let report_dir = scratch_dir("stage1_negative");
    let outcome = verify(
        Path::new("tests/test_capability_stage1_missing_effect.mm"),
        &report_dir,
    );
    let combined = format!("{}{}", outcome.stdout, outcome.stderr);
    assert!(
        combined.contains("Effect polymorphism violation"),
        "capability parameter without the underlying effect must be reported as an \
         effect polymorphism violation\nstdout:\n{}\nstderr:\n{}",
        outcome.stdout,
        outcome.stderr
    );
    assert!(
        combined.contains("accepts capability parameter 'cap'"),
        "violation must name the capability parameter\nstdout:\n{}\nstderr:\n{}",
        outcome.stdout,
        outcome.stderr
    );
    std::fs::remove_dir_all(report_dir).expect("remove scratch dir");
}

/// study §6 Stage 1 completion criterion: a capability-typed parameter yields
/// the same verdict as the equivalent effect-parameter formulation.
#[test]
fn capability_and_effect_versions_agree() {
    const CAPABILITY_PASS: &str = r#"
effect SafeFileRead(path: Str) where starts_with(path, "/tmp/");
type FileCap = capability SafeFileRead(path: Str) where starts_with(path, "/tmp/");

atom read_log(cap: FileCap, user_id: Str)
    effects: [SafeFileRead(path)]
    requires: not_contains(user_id, "..") && not_contains(user_id, "/");
    ensures: result >= 0;
    body: {
        let path = "/tmp/" + user_id + ".log";
        perform cap.read(path);
        1
    }
"#;
    const EFFECT_PASS: &str = r#"
effect SafeFileRead(path: Str) where starts_with(path, "/tmp/");

atom read_log(user_id: Str)
    effects: [SafeFileRead(path)]
    requires: not_contains(user_id, "..") && not_contains(user_id, "/");
    ensures: result >= 0;
    body: {
        let path = "/tmp/" + user_id + ".log";
        perform SafeFileRead.read(path);
        1
    }
"#;
    const CAPABILITY_FAIL: &str = r#"
effect SafeFileRead(path: Str) where starts_with(path, "/tmp/");
type FileCap = capability SafeFileRead(path: Str) where starts_with(path, "/tmp/");

atom read_log(cap: FileCap, user_id: Str)
    effects: [SafeFileRead(path)]
    requires: true;
    ensures: result >= 0;
    body: {
        let path = "/etc/" + user_id;
        perform cap.read(path);
        1
    }
"#;
    const EFFECT_FAIL: &str = r#"
effect SafeFileRead(path: Str) where starts_with(path, "/tmp/");

atom read_log(user_id: Str)
    effects: [SafeFileRead(path)]
    requires: true;
    ensures: result >= 0;
    body: {
        let path = "/etc/" + user_id;
        perform SafeFileRead.read(path);
        1
    }
"#;

    let dir = scratch_dir("stage1_equivalence");
    for (tag, capability_source, effect_source) in [
        ("pass", CAPABILITY_PASS, EFFECT_PASS),
        ("fail", CAPABILITY_FAIL, EFFECT_FAIL),
    ] {
        let capability_path =
            write_source(&dir, &format!("capability_{tag}.mm"), capability_source);
        let effect_path = write_source(&dir, &format!("effect_{tag}.mm"), effect_source);
        let report_dir = dir.join(format!("reports_{tag}"));

        let capability_outcome = verify(&capability_path, &report_dir);
        let effect_outcome = verify(&effect_path, &report_dir);

        assert_eq!(
            capability_outcome.success,
            effect_outcome.success,
            "capability and effect versions must agree ({tag})\n\
             capability stdout:\n{}\ncapability stderr:\n{}\n\
             effect stdout:\n{}\neffect stderr:\n{}",
            capability_outcome.stdout,
            capability_outcome.stderr,
            effect_outcome.stdout,
            effect_outcome.stderr
        );
    }
    std::fs::remove_dir_all(dir).expect("remove scratch dir");
}

/// The contextual `capability` keyword must not turn `capability` / `grant`
/// into reserved words elsewhere in a source file.
#[test]
fn capability_and_grant_remain_identifiers() {
    const SOURCE: &str = r#"
atom add_quota(capability: i64, grant: i64)
    requires: capability >= 0 && grant >= 0;
    ensures: result >= 0;
    body: {
        capability + grant
    }
"#;
    let dir = scratch_dir("stage1_identifiers");
    let source_path = write_source(&dir, "identifiers.mm", SOURCE);
    let outcome = verify(&source_path, &dir.join("reports"));
    assert!(
        outcome.success,
        "`capability` and `grant` must remain usable as identifiers\nstdout:\n{}\nstderr:\n{}",
        outcome.stdout, outcome.stderr
    );
    std::fs::remove_dir_all(dir).expect("remove scratch dir");
}
