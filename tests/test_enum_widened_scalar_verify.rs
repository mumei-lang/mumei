//! Widened integer enum payloads (`u64`/`u32`/`i32`, with `usize`/`isize`
//! folding to `u64`/`i64`) on the general verifier's Datatype path.
//!
//! Each leaf lands on `Int` — `enum_value_leaf_domain` bounds it to the
//! declared width wherever an enum value is introduced (params, seeded
//! struct fields, `result`, call results), so a `u64` payload can never
//! model as a negative number while the bound stays exactly the declared
//! width (`x >= 1` under `W::A(x)` is refuted by `W::A(0)`).

use std::path::Path;
use std::process::Command;

fn run_verify(fixture: &Path, tag: &str) -> std::process::Output {
    let dir =
        std::env::temp_dir().join(format!("mumei_enum_widened_{}_{}", std::process::id(), tag));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale temp dir");
    }
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let copied = dir.join(fixture.file_name().expect("fixture name"));
    std::fs::copy(fixture, &copied).expect("copy fixture");
    let bin = env!("CARGO_BIN_EXE_mumei");
    let out = Command::new(bin)
        .arg("verify")
        .arg("--report-dir")
        .arg(&dir)
        .arg(&copied)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run mumei verify");
    std::fs::remove_dir_all(dir).ok();
    out
}

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(name)
}

#[test]
fn widened_scalar_payloads_verify_on_datatype_path() {
    let out = run_verify(&fixture("test_enum_widened_scalar_verify.mm"), "ok");
    assert!(
        out.status.success(),
        "u64/u32/i32/usize enum payload leaves should verify with declared-width domains:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn widened_leaf_domain_stops_at_declared_width() {
    let out = run_verify(
        &fixture("test_enum_widened_scalar_verify_negative.mm"),
        "neg",
    );
    assert!(
        !out.status.success(),
        "`x >= 1` under `W::A(x)` must fail — `W::A(0)` is in-domain"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("W::A(0)"),
        "the counterexample should name the in-domain boundary value:\n{stdout}"
    );
}

#[test]
fn u64_payload_doubling_respects_unsigned_wrap() {
    let out = run_verify(
        &fixture("test_enum_widened_scalar_wrap_negative.mm"),
        "wrap",
    );
    assert!(
        !out.status.success(),
        "`x + x >= x` does not hold for every u64 payload because addition wraps"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let payload = stdout
        .split("W::A(")
        .nth(1)
        .and_then(|witness| witness.split(')').next())
        .and_then(|witness| witness.parse::<u64>().ok())
        .unwrap_or_else(|| panic!("the counterexample should include a u64 payload:\n{stdout}"));
    assert!(
        payload >= (1_u64 << 63),
        "the counterexample should exercise unsigned wrap at 2^63:\n{stdout}"
    );
}

#[test]
fn widened_leaf_mixing_with_declared_i64_rejects() {
    let out = run_verify(&fixture("test_enum_widened_scalar_mix_negative.mm"), "mix");
    assert!(
        !out.status.success(),
        "`x + y` with `x: u64` and `y: i64` must reject — a match-bound \
         leaf carries its declared unsigned type into the mixing rule"
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("u64"),
        "the rejection should name the u64 side of the mixing rule:\n{combined}"
    );
}
