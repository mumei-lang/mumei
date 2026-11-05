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

#[test]
fn loop_conditions_and_match_guards_are_checked() {
    let looping = "atom loop_cond(x: i64) -> i64\n\
        requires: x >= 0;\n\
        ensures: result >= 0;\n\
        body: { let i = x; while i + 1 < 0 invariant i >= 0 { i = i + 0; }; i };\n";
    let guarded = "atom guard(x: i64) -> i64\n\
        requires: x >= 0;\n\
        ensures: result >= 0;\n\
        body: { match x { n if n + 1 > 0 => 1, _ => 0 } };\n";
    for (tag, source) in [("loop", looping), ("guard", guarded)] {
        assert!(verify(tag, source, &[]).status.success());
        let output = verify(tag, source, &["--check-i64-overflow"]);
        assert!(!output.status.success(), "{tag}: {}", text(&output));
        assert!(
            text(&output).contains("Integer arithmetic may overflow i64"),
            "{tag}: {}",
            text(&output)
        );
    }
}

#[test]
fn a_loop_condition_bounded_by_the_invariant_verifies_with_the_flag() {
    let source = "atom loop_ok(n: i64) -> i64\n\
        requires: n >= 0 && n < 1000;\n\
        ensures: result >= 0;\n\
        body: { let i = 0; while i + 1 <= n invariant i >= 0 && i <= n { i = i + 1; }; i };\n";
    let output = verify("loop_ok", source, &["--check-i64-overflow"]);
    assert!(output.status.success(), "{}", text(&output));
}

#[test]
fn lambda_bodies_are_checked_at_the_call_site() {
    let safe = "atom lam(x: i64) -> i64\n\
        requires: true;\n\
        ensures: result == 1;\n\
        body: { let f = |n| n + 1; f(0) };\n";
    let output = verify("lambda_safe", safe, &["--check-i64-overflow"]);
    assert!(output.status.success(), "{}", text(&output));
    let unbounded = "atom lam_bad(x: i64) -> i64\n\
        requires: true;\n\
        ensures: result == x + 1;\n\
        body: { let f = |n| n + 1; f(x) };\n";
    let output = verify("lambda_unbounded", unbounded, &["--check-i64-overflow"]);
    assert!(!output.status.success(), "{}", text(&output));
}

#[test]
fn a_callee_contract_stays_unbounded_at_the_call_site() {
    let source = "atom ident(x: i64) -> i64\n\
        requires: true;\n\
        ensures: result + 1 > result && result == x;\n\
        body: { x };\n\
        atom caller(x: i64) -> i64\n\
        requires: true;\n\
        ensures: result == x;\n\
        body: { ident(x) };\n";
    let output = verify("callee_contract", source, &["--check-i64-overflow"]);
    assert!(output.status.success(), "{}", text(&output));
}

#[test]
fn a_match_guard_is_checked_under_its_pattern() {
    let source = "atom pat(x: i64) -> i64\n\
        requires: true;\n\
        ensures: result >= 0;\n\
        body: { match x { 5 if x + 1 > 0 => 1, _ => 0 } };\n";
    let output = verify("guard_pattern", source, &["--check-i64-overflow"]);
    assert!(output.status.success(), "{}", text(&output));
}

#[test]
fn a_match_guard_in_a_loop_is_checked_under_the_invariant() {
    let source = "atom loop_guard(n: i64) -> i64\n\
        requires: n >= 0 && n < 1000;\n\
        ensures: result >= 0;\n\
        body: { let i = 0; while i < n invariant i >= 0 && i <= n { i = match i { k if k + 1 <= n => k + 1, _ => n }; }; i };\n";
    let output = verify("loop_guard", source, &["--check-i64-overflow"]);
    assert!(output.status.success(), "{}", text(&output));
}
