//! Integration tests for the opt-in `--ieee754-f64` verification mode.
//!
//! Default `f64` verification encodes values as Z3 `Real` (exact rationals).
//! With `--ieee754-f64`, `f64` is instead encoded as IEEE 754 binary64 `Float`
//! with round-nearest-even arithmetic, so rounding-sensitive properties such as
//! `0.1 + 0.2 != 0.3` become provable. These tests pin the differential and
//! confirm the flag does not regress existing `Real`-mode `f64` fixtures.

use std::process::Command;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mumei_ieee754_{}_{}", std::process::id(), tag));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale temp dir");
    }
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn write_fixture(dir: &std::path::Path, name: &str, source: &str) -> std::path::PathBuf {
    let fixture = dir.join(name);
    std::fs::write(&fixture, source).expect("write fixture");
    fixture
}

fn run_verify(
    fixture: &std::path::Path,
    dir: &std::path::Path,
    extra: &[&str],
) -> std::process::Output {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let mut cmd = Command::new(bin);
    cmd.arg("verify");
    for arg in extra {
        cmd.arg(arg);
    }
    cmd.arg("--report-dir")
        .arg(dir)
        .arg(fixture)
        .current_dir(env!("CARGO_MANIFEST_DIR"));
    cmd.output().expect("failed to run mumei verify")
}

/// The rounding property `0.1 + 0.2 != 0.3` holds under IEEE 754 binary64 but
/// not under exact-rational `Real`. It must verify only with `--ieee754-f64`.
#[test]
fn ieee754_flag_makes_rounding_property_hold() {
    let dir = temp_dir("rounding");
    let source = "atom rounding() -> i64\n\
        requires: true;\n\
        ensures: 0.1 + 0.2 != 0.3;\n\
        body: 0;\n";
    let fixture = write_fixture(&dir, "rounding.mm", source);

    let ieee = run_verify(&fixture, &dir, &["--ieee754-f64"]);
    assert!(
        ieee.status.success(),
        "expected --ieee754-f64 to prove `0.1 + 0.2 != 0.3`\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&ieee.stdout),
        String::from_utf8_lossy(&ieee.stderr)
    );

    let real = run_verify(&fixture, &dir, &[]);
    assert!(
        !real.status.success(),
        "default Real mode must NOT prove `0.1 + 0.2 != 0.3`\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&real.stdout),
        String::from_utf8_lossy(&real.stderr)
    );

    std::fs::remove_dir_all(dir).ok();
}

/// A contract that is valid under both encodings must keep verifying with the
/// flag on: `--ieee754-f64` lowers `f64` arithmetic to the FP theory (not to an
/// unconstrained fresh constant), so ordinary `f64` proofs still succeed.
#[test]
fn ieee754_flag_preserves_valid_f64_contracts() {
    let dir = temp_dir("regression");
    // `x >= 0.0` excludes NaN under fp comparison, so `result == x` satisfies
    // `result >= 0.0` in both Real and IEEE 754 encodings.
    let source = "atom nonneg(x: f64) -> f64\n\
        requires: x >= 0.0;\n\
        ensures: result >= 0.0;\n\
        body: x;\n";
    let fixture = write_fixture(&dir, "nonneg.mm", source);

    for mode in [&["--ieee754-f64"][..], &[][..]] {
        let out = run_verify(&fixture, &dir, mode);
        assert!(
            out.status.success(),
            "expected f64 contract to verify (mode {mode:?})\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn cli_exposes_ieee754_flag() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let output = Command::new(bin)
        .arg("verify")
        .arg("--help")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run mumei verify --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--ieee754-f64"),
        "verify --help should advertise --ieee754-f64:\n{stdout}"
    );
}
