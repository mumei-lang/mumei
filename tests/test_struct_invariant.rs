//! Cross-field struct invariants (`invariant: <expr>` inside a struct body).
//!
//! The invariant is assumed for struct-typed parameters, checked at every
//! struct literal, and imposed as an implicit postcondition on the `result`
//! of atoms returning the struct.

use std::path::Path;
use std::process::Command;

fn run_verify(fixture: &Path, tag: &str) -> std::process::Output {
    let dir = std::env::temp_dir().join(format!(
        "mumei_struct_invariant_{}_{}",
        std::process::id(),
        tag
    ));
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
fn invariant_preserving_transitions_verify() {
    let out = run_verify(&fixture("test_struct_invariant.mm"), "ok");
    assert!(
        out.status.success(),
        "invariant-preserving fixture must verify\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn invariant_violating_literal_fails() {
    let out = run_verify(&fixture("test_struct_invariant_violation.mm"), "violation");
    assert!(
        !out.status.success(),
        "active_tasks + 1 may exceed max_tasks: verification must fail\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("invariant violated"),
        "failure must be attributed to the struct invariant\nstderr:\n{stderr}"
    );
}

/// `requires` contradicting a parameter's struct invariant is a spec
/// contradiction, not a vacuous proof.
#[test]
fn requires_contradicting_invariant_is_unsat() {
    let out = run_verify(
        &fixture("test_struct_invariant_requires_unsat.mm"),
        "requires_unsat",
    );
    assert!(
        !out.status.success(),
        "contradictory requires must not verify"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("requires_unsat"),
        "failure must be reported as requires_unsat\nstderr:\n{stderr}"
    );
}

/// A bitwise struct invariant selects BV(64) for parameter assumptions,
/// literal checks and the implicit result postcondition.
#[test]
fn bitwise_invariant_selects_bitvector_semantics() {
    let out = run_verify(&fixture("test_struct_invariant_bitvec.mm"), "bitvec");
    assert!(
        out.status.success(),
        "bitwise invariant fixture must verify\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn parenthesized_comparison_invariant_keeps_grouping_semantics() {
    let out = run_verify(
        &fixture("test_struct_invariant_parenthesized.mm"),
        "parenthesized",
    );
    assert!(
        out.status.success(),
        "parenthesized invariant must not be re-normalized as a flat chain
stdout:
{}
stderr:
{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Commas inside an invariant (call arguments) do not split it into fields.
#[test]
fn invariant_with_call_arguments_verifies() {
    let out = run_verify(
        &fixture("test_struct_invariant_call_in_invariant.mm"),
        "call_in_invariant",
    );
    assert!(
        out.status.success(),
        "invariant with a call must verify\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Existing struct fixtures (per-field `where v ...` constraints, impl
/// blocks, field access) keep verifying unchanged.
#[test]
fn existing_struct_fixtures_are_unaffected() {
    let name = "test_impl_block.mm";
    let out = run_verify(&fixture(name), name.trim_end_matches(".mm"));
    assert!(
        out.status.success(),
        "{name} regressed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn refined_alias_field_predicates_are_assumed_for_struct_parameters() {
    // `type Pos = i64 where v > 0` on `x` lets `gx` prove `result >= 1`
    // from `p.x` alone — the same domain a scalar `Pos` parameter gets.
    let out = run_verify(&fixture("test_struct_alias_field.mm"), "alias_ok");
    assert!(
        out.status.success(),
        "alias field predicate must be assumed\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // The assumption is the predicate, not more: `result >= 2` fails.
    let out = run_verify(
        &fixture("test_struct_alias_field_violation.mm"),
        "alias_bad",
    );
    assert!(!out.status.success());
}

#[test]
fn struct_literals_must_establish_refined_alias_field_types() {
    // Struct parameters assume their alias field predicates, so a literal
    // is the point that has to prove them.
    let out = run_verify(
        &fixture("test_struct_alias_field_literal.mm"),
        "alias_lit_ok",
    );
    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = run_verify(
        &fixture("test_struct_alias_field_literal_violation.mm"),
        "alias_lit_bad",
    );
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("type 'Pos' predicate violated")
            || String::from_utf8_lossy(&out.stdout).contains("type 'Pos' predicate violated")
    );
}

#[test]
fn struct_results_bind_result_fields_and_literals_prove_alias_domains() {
    // `result.x` in `ensures` reads the returned field, and a returned
    // struct's field reads through a struct `let` see the callee's
    // postcondition.
    let out = run_verify(&fixture("test_struct_result.mm"), "result_ok");
    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // A returned literal still has to establish its alias field domain.
    let out = run_verify(&fixture("test_struct_result_violation.mm"), "result_bad");
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("type 'Pos' predicate violated")
            || String::from_utf8_lossy(&out.stdout).contains("type 'Pos' predicate violated")
    );
}
