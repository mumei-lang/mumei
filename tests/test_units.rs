//! Units of measure (`type Usd = i64 unit USD;`).
//!
//! Units are compile-time tags checked on `+`, `-` and comparisons; they do
//! not change the Z3 encoding, so unit-annotated fixtures verify exactly like
//! their unitless counterparts and mismatches are rejected before any proof.

use std::path::Path;
use std::process::Command;

fn run_verify(fixture: &Path, tag: &str) -> std::process::Output {
    let dir = std::env::temp_dir().join(format!("mumei_units_{}_{}", std::process::id(), tag));
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

fn combined(out: &std::process::Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn matching_units_verify() {
    let out = run_verify(&fixture("test_units_ok.mm"), "ok");
    let text = combined(&out);
    assert!(
        out.status.success(),
        "unit-consistent fixture must verify\n{text}"
    );
    assert!(
        !text.contains("Unit mismatch"),
        "no unit error expected\n{text}"
    );
}

fn assert_unit_mismatch(name: &str, tag: &str, lhs: &str, rhs: &str) {
    let out = run_verify(&fixture(name), tag);
    let text = combined(&out);
    assert!(!out.status.success(), "{name} must be rejected\n{text}");
    assert!(
        text.contains("Unit mismatch"),
        "{name}: expected unit error\n{text}"
    );
    assert!(
        text.contains(&format!("'{lhs}' with '{rhs}'")),
        "{name}: expected units {lhs}/{rhs} in message\n{text}"
    );
}

#[test]
fn usd_plus_jpy_is_rejected() {
    assert_unit_mismatch("test_units_mismatch_add.mm", "add", "USD", "JPY");
}

#[test]
fn meter_compared_to_second_is_rejected() {
    assert_unit_mismatch("test_units_mismatch_cmp.mm", "cmp", "Meter", "Second");
}

#[test]
fn returning_usd_as_jpy_is_rejected() {
    assert_unit_mismatch("test_units_mismatch_return.mm", "ret", "JPY", "USD");
}

#[test]
fn passing_jpy_to_usd_parameter_is_rejected() {
    assert_unit_mismatch("test_units_mismatch_call.mm", "call", "USD", "JPY");
}

/// Struct field units are tracked through `let`-bound structs, struct-returning
/// calls and nested field access, not only through parameters/`result`.
#[test]
fn struct_field_units_track_through_bindings_and_calls() {
    let out = run_verify(&fixture("test_units_struct_ok.mm"), "struct_ok");
    let text = combined(&out);
    assert!(out.status.success(), "{text}");
    assert!(!text.contains("Unit mismatch"), "{text}");
}

#[test]
fn let_bound_struct_field_mismatch_is_rejected() {
    assert_unit_mismatch(
        "test_units_struct_mismatch_let.mm",
        "struct_let",
        "JPY",
        "USD",
    );
}

#[test]
fn call_result_struct_field_mismatch_is_rejected() {
    assert_unit_mismatch(
        "test_units_struct_mismatch_call.mm",
        "struct_call",
        "JPY",
        "USD",
    );
}

#[test]
fn nested_struct_field_mismatch_is_rejected() {
    assert_unit_mismatch(
        "test_units_struct_mismatch_nested.mm",
        "struct_nested",
        "JPY",
        "USD",
    );
}

/// Branches yielding different struct types must not silently adopt the first
/// branch's type; both branch orders are rejected before any field is checked.
#[test]
fn mixed_struct_branches_are_rejected_in_either_order() {
    for (name, tag) in [
        ("test_units_struct_mismatch_branch_ab.mm", "branch_ab"),
        ("test_units_struct_mismatch_branch_ba.mm", "branch_ba"),
    ] {
        let out = run_verify(&fixture(name), tag);
        let text = combined(&out);
        assert!(!out.status.success(), "{name} must be rejected\n{text}");
        assert!(
            text.contains("Type mismatch") && text.contains("conditional branches"),
            "{name}: expected struct branch mismatch\n{text}"
        );
        assert!(
            text.contains("'A'") && text.contains("'B'"),
            "{name}: both struct names expected\n{text}"
        );
        assert!(
            text.contains("if c { A { amt: u } } else { B { amt: j } }")
                || text.contains("if c { B { amt: j } } else { A { amt: u } }"),
            "{name}: branch expression must be rendered as source\n{text}"
        );
    }
}

/// `type Money = Usd;` inherits `USD` through the alias chain.
#[test]
fn alias_chain_inherits_unit() {
    let out = run_verify(&fixture("test_units_alias_chain_ok.mm"), "alias_ok");
    let text = combined(&out);
    assert!(out.status.success(), "{text}");
    assert_unit_mismatch(
        "test_units_alias_chain_mismatch.mm",
        "alias_bad",
        "USD",
        "JPY",
    );
}

/// A unit error is reported before Z3 runs, so no report.json is written; a
/// stale report.json from an earlier run must not leak into `--json` output.
#[test]
fn stale_report_does_not_pollute_json_on_unit_error() {
    let dir = std::env::temp_dir().join(format!("mumei_units_{}_stale", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale temp dir");
    }
    std::fs::create_dir_all(&dir).expect("create temp dir");
    std::fs::write(
        dir.join("report.json"),
        r#"{"status":"success","atom":"stale_atom"}"#,
    )
    .expect("write stale report");
    let out = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--json")
        .arg("--report-dir")
        .arg(&dir)
        .arg(fixture("test_units_mismatch_add.mm"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run mumei verify");
    std::fs::remove_dir_all(&dir).ok();
    assert!(!out.status.success());
    let payload: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("--json must print valid JSON");
    assert_eq!(payload["status"], "failed", "{payload}");
    assert!(
        payload.get("atom").is_none(),
        "stale atom leaked: {payload}"
    );
    let tags = &payload["diagnostics"][0]["tags"];
    assert!(
        tags.as_array().is_some_and(
            |t| t.iter().any(|x| x == "z3_skipped") && !t.iter().any(|x| x == "z3_sat")
        ),
        "type error must not be tagged z3_sat: {payload}"
    );
}

/// A solver failure followed by a unit error: the first atom's report.json must
/// not be presented as the module's `--json` result.
#[test]
fn solver_failure_then_unit_error_json_reports_both_atoms() {
    let dir = std::env::temp_dir().join(format!("mumei_units_{}_two_failures", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale temp dir");
    }
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let out = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--json")
        .arg("--report-dir")
        .arg(&dir)
        .arg(fixture("test_units_solver_fail_then_mismatch.mm"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run mumei verify");
    std::fs::remove_dir_all(&dir).ok();
    assert!(!out.status.success());
    let payload: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("--json must print valid JSON");
    assert_eq!(payload["status"], "failed", "{payload}");
    assert_eq!(payload["failed"], 2, "{payload}");
    assert!(
        payload.get("atom").is_none(),
        "single-atom report leaked into module result: {payload}"
    );
    let atoms: Vec<&str> = payload["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .filter_map(|d| d["atom"].as_str())
        .collect();
    assert!(
        atoms.contains(&"wrong_inc") && atoms.contains(&"add_mixed"),
        "{payload}"
    );
}

/// A unit-only edit to an alias must invalidate the incremental cache, so the
/// second run reports the mismatch instead of reusing the cached success.
#[test]
fn unit_change_invalidates_verification_cache() {
    let dir = std::env::temp_dir().join(format!("mumei_units_{}_cache", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale temp dir");
    }
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let file = dir.join("cache.mm");
    let atom = "atom add(a: A, b: B)\n    requires: true;\n    ensures: result == a + b;\n    body: a + b;\n";
    let run = |src: &str| {
        std::fs::write(&file, src).expect("write fixture");
        let out = Command::new(env!("CARGO_BIN_EXE_mumei"))
            .arg("verify")
            .arg("--report-dir")
            .arg(&dir)
            .arg(&file)
            .current_dir(&dir)
            .output()
            .expect("failed to run mumei verify");
        (out.status.success(), combined(&out))
    };

    let (ok, text) = run(&format!(
        "type A = i64 unit USD;\ntype B = i64 unit USD;\n{atom}"
    ));
    assert!(ok, "same-unit program must verify\n{text}");
    let (ok, text) = run(&format!(
        "type A = i64 unit USD;\ntype B = i64 unit JPY;\n{atom}"
    ));
    assert!(!ok, "unit edit must not hit the cache\n{text}");
    assert!(text.contains("Unit mismatch"), "{text}");

    // Re-pointing an alias chain (B = A -> B = J) must also invalidate.
    let (ok, text) = run(&format!(
        "type A = i64 unit USD;\ntype J = i64 unit JPY;\ntype B = A;\n{atom}"
    ));
    assert!(ok, "alias-of-alias with same unit must verify\n{text}");
    let (ok, text) = run(&format!(
        "type A = i64 unit USD;\ntype J = i64 unit JPY;\ntype B = J;\n{atom}"
    ));
    std::fs::remove_dir_all(&dir).ok();
    assert!(!ok, "alias base edit must not hit the cache\n{text}");
    assert!(text.contains("Unit mismatch"), "{text}");
}

/// Backward compatibility: refinement-only aliases and unannotated atoms are
/// untouched by the unit checker.
#[test]
fn unannotated_fixture_still_verifies() {
    let out = run_verify(&fixture("test_call_with_contract.mm"), "compat");
    let text = combined(&out);
    assert!(
        out.status.success(),
        "legacy fixture must still verify\n{text}"
    );
    assert!(!text.contains("Unit mismatch"), "{text}");
}
