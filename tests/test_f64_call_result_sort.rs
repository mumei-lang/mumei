//! Regression tests for call-site symbolic result sorts.
//!
//! The symbolic result of a callee is declared with the sort of its `-> T`
//! return type. Before this was fixed, the sort was inferred from a "callee
//! has any `f64` parameter" heuristic: an `f64`-returning callee without `f64`
//! parameters got an `Int` result, whose `ensures` equality (e.g.
//! `result == 0.5`) made the caller's solver context unsatisfiable and let
//! arbitrary caller postconditions verify vacuously.

use std::process::Command;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mumei_callsort_{}_{}", std::process::id(), tag));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale temp dir");
    }
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn run_verify(source: &str, tag: &str, extra: &[&str]) -> std::process::Output {
    let dir = temp_dir(tag);
    let fixture = dir.join(format!("{tag}.mm"));
    std::fs::write(&fixture, source).expect("write fixture");
    let bin = env!("CARGO_BIN_EXE_mumei");
    let mut cmd = Command::new(bin);
    cmd.arg("verify");
    for arg in extra {
        cmd.arg(arg);
    }
    cmd.arg("--report-dir")
        .arg(&dir)
        .arg(&fixture)
        .current_dir(env!("CARGO_MANIFEST_DIR"));
    let out = cmd.output().expect("failed to run mumei verify");
    std::fs::remove_dir_all(dir).ok();
    out
}

/// An `f64`-returning callee with no `f64` parameters must not poison the
/// caller's context: a false caller postcondition has to fail.
#[test]
fn f64_return_without_f64_params_does_not_verify_false_ensures() {
    let source = "atom half() -> f64\n\
        requires: true;\n\
        ensures: result == 0.5;\n\
        body: 0.5;\n\
        \n\
        atom caller_false(x: f64) -> f64\n\
        requires: true;\n\
        ensures: result >= 100.0;\n\
        body: half();\n";
    let out = run_verify(source, "false_ensures", &[]);
    assert!(
        !out.status.success(),
        "caller with false ensures must fail verification\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The propagated `ensures: result == 0.5` equality must be usable by the
/// caller under the default `Real` encoding and under `--ieee754-f64`.
#[test]
fn f64_return_equality_propagates_to_caller() {
    let source = "atom half() -> f64\n\
        requires: true;\n\
        ensures: result == 0.5;\n\
        body: 0.5;\n\
        \n\
        atom caller_ok(x: f64) -> f64\n\
        requires: true;\n\
        ensures: result >= 0.0;\n\
        body: half();\n";
    for mode in [&[][..], &["--ieee754-f64"][..]] {
        let out = run_verify(source, "propagates", mode);
        assert!(
            out.status.success(),
            "caller relying on callee ensures must verify (mode {mode:?})\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// A `bool`-returning callee gets a `Bool` result sort so its ensures can be
/// consumed by the caller without sort mismatches.
#[test]
fn bool_return_type_uses_bool_sort() {
    let source = "atom always_true() -> bool\n\
        requires: true;\n\
        ensures: result == true;\n\
        body: true;\n\
        \n\
        atom caller(x: i64) -> bool\n\
        requires: true;\n\
        ensures: result == true;\n\
        body: always_true();\n";
    let out = run_verify(source, "bool_ret", &[]);
    assert!(
        out.status.success(),
        "bool-returning call chain must verify\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
