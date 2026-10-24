//! Regression tests for issue #717: `Param.name` must hold the bare
//! identifier — ownership markers (`consume`, `ref`, `ref mut`) live on
//! `Param::consume` / `is_ref` / `is_ref_mut`. Before the fix, the parser kept
//! `"consume xs"` in `name`, so spec-side lookups that compare against the
//! written name (`len(xs)`, param equality) silently failed.

use std::process::{Command, Output};

fn verify_source(tag: &str, source: &str) -> (Output, String) {
    let dir = std::env::temp_dir().join(format!(
        "mumei_param_prefix_{}_{}_{}",
        std::process::id(),
        tag,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).expect("create fixture directory");
    let fixture = dir.join("fixture.mm");
    let report_dir = dir.join("reports");
    std::fs::create_dir_all(&report_dir).expect("create report directory");
    std::fs::write(&fixture, source).expect("write fixture");

    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--report-dir")
        .arg(&report_dir)
        .arg("--disable-spurious-detection")
        .arg(&fixture)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run mumei verify");
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::remove_dir_all(dir).expect("remove fixture directory");
    (output, text)
}

#[test]
fn consume_array_param_len_verifies() {
    let (output, text) = verify_source(
        "consume_array",
        r#"
atom first(consume xs: [i64])
    requires: len(xs) >= 1;
    ensures: result >= 0;
    body: 0;
"#,
    );
    assert!(
        output.status.success(),
        "consume xs: [i64] with len(xs) must verify, got:\n{text}"
    );
}

#[test]
fn ref_and_ref_mut_array_param_len_verifies() {
    let (output, text) = verify_source(
        "ref_array",
        r#"
atom read_it(ref xs: [i64])
    requires: len(xs) >= 1;
    ensures: result >= 0;
    body: 0;

atom read_mut(ref mut xs: [i64])
    requires: len(xs) >= 1;
    ensures: result >= 0;
    body: 0;
"#,
    );
    assert!(
        output.status.success(),
        "ref/ref mut xs: [i64] with len(xs) must verify, got:\n{text}"
    );
}

#[test]
fn consume_scalar_param_still_verifies() {
    let (output, text) = verify_source(
        "consume_scalar",
        r#"
atom bump(consume n: i64)
    requires: n >= 0;
    ensures: result == n + 1;
    body: n + 1;
"#,
    );
    assert!(
        output.status.success(),
        "consume n: i64 must keep verifying, got:\n{text}"
    );
}

#[test]
fn consume_param_ensures_references_verify() {
    // Spec lookups comparing `p.name` against the written identifier must
    // resolve on `consume` params too (ensures side of #717).
    let (output, text) = verify_source(
        "consume_ensures",
        r#"
atom head(consume xs: [i64])
    requires: len(xs) >= 1;
    ensures: result == xs[0];
    body: xs[0];
"#,
    );
    assert!(
        output.status.success(),
        "ensures referencing a consume param must verify, got:\n{text}"
    );
}
