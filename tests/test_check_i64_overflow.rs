//! Integration tests for `--check-i64-overflow`.
//!
//! The default `Int` encoding is unbounded, so `epoch + 1` never overflows in
//! a proof while the compiled `i64` wraps (or panics in a checked build). The
//! flag rejects a body `+`, `-` or `*` whose `i64` operands can produce a
//! result outside the `i64` range. Contracts keep unbounded arithmetic.

use std::process::Command;

fn verify(tag: &str, source: &str, extra: &[&str]) -> std::process::Output {
    let dir = std::env::temp_dir().join(format!(
        "mumei_check_i64_overflow_{}_{}",
        std::process::id(),
        tag
    ));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale temp dir");
    }
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let fixture = dir.join("overflow.mm");
    std::fs::write(&fixture, source).expect("write fixture");
    Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .args(extra)
        .arg("--report-dir")
        .arg(&dir)
        .arg(&fixture)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run mumei verify")
}

fn text(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

const UNBOUNDED_INC: &str = "atom next_epoch(epoch: i64) -> i64\n\
    requires: epoch >= 0;\n\
    ensures: result == epoch + 1;\n\
    body: { epoch + 1 };\n";

#[test]
fn unbounded_increment_verifies_by_default() {
    let output = verify("default", UNBOUNDED_INC, &[]);
    assert!(output.status.success(), "{}", text(&output));
}

#[test]
fn unbounded_increment_is_rejected_with_the_flag() {
    let output = verify("rejected", UNBOUNDED_INC, &["--check-i64-overflow"]);
    assert!(!output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("Integer arithmetic may overflow i64"),
        "{}",
        text(&output)
    );
}

#[test]
fn bounded_or_guarded_arithmetic_verifies_with_the_flag() {
    let source = "atom bounded_next(epoch: i64) -> i64\n\
        requires: epoch >= 0 && epoch < 1000000;\n\
        ensures: result == epoch + 1;\n\
        body: { epoch + 1 };\n\
        atom guarded_inc(x: i64) -> i64\n\
        requires: true;\n\
        ensures: result >= x;\n\
        body: { if x < 9223372036854775807 { x + 1 } else { x } };\n\
        atom diff(a: i64, b: i64) -> i64\n\
        requires: a >= 0 && b >= 0;\n\
        ensures: result + b == a;\n\
        body: { a - b };\n";
    let output = verify("bounded", source, &["--check-i64-overflow"]);
    assert!(output.status.success(), "{}", text(&output));
}

#[test]
fn product_of_unbounded_operands_is_rejected_with_the_flag() {
    let source = "atom area(w: i64, h: i64) -> i64\n\
        requires: w >= 0 && h >= 0;\n\
        ensures: result >= 0;\n\
        body: { w * h };\n";
    let output = verify("mul", source, &["--check-i64-overflow"]);
    assert!(!output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("Integer arithmetic may overflow i64"),
        "{}",
        text(&output)
    );
}

#[test]
fn contract_arithmetic_stays_unbounded_with_the_flag() {
    let source = "atom ident(x: i64) -> i64\n\
        requires: true;\n\
        ensures: result + 1 > result;\n\
        body: { x };\n";
    let output = verify("contract", source, &["--check-i64-overflow"]);
    assert!(output.status.success(), "{}", text(&output));
}
