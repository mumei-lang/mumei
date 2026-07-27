use std::process::Command;

fn write_fixture(name: &str, source: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mumei_unverifiable_cli_{}_{}",
        name,
        std::process::id()
    ));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale unverifiable fixture dir");
    }
    std::fs::create_dir_all(&dir).expect("create unverifiable fixture dir");
    let path = dir.join("main.mm");
    std::fs::write(&path, source).expect("write unverifiable fixture");
    path
}

#[test]
fn verify_distinguishes_unverifiable_then_failed_atoms() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture = write_fixture(
        "mixed",
        r#"
atom symbolic_pow(x: i64, y: i64) -> i64
  requires: x >= 0;
  ensures: result == x**y && result == x;
  body: x;

atom false_postcondition(x: i64) -> i64
  requires: x >= 0;
  ensures: result == x + 1;
  body: x;
"#,
    );

    let output = Command::new(bin)
        .arg("verify")
        .arg(&fixture)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify: {err}"));

    assert!(
        !output.status.success(),
        "mixed unverifiable+failed fixture should not verify\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("'symbolic_pow': unverifiable ⚠️"),
        "expected symbolic_pow to be unverifiable, got:\n{combined}"
    );
    assert!(
        combined.contains("Phase 5: ensures verification (failed)")
            || combined.contains("'false_postcondition':"),
        "expected false_postcondition failure details, got:\n{combined}"
    );
    assert!(
        combined.contains("❌ Verification: 0 passed, 1 failed, 1 unverifiable")
            || combined.contains("⚠️  Verification: 0 passed, 1 unverifiable")
            || combined.contains("1 failed, 1 unverifiable"),
        "expected mixed failed/unverifiable summary, got:\n{combined}"
    );

    std::fs::remove_dir_all(fixture.parent().unwrap()).expect("remove unverifiable fixture dir");
}

#[test]
fn verify_enforces_all_requires_conjuncts_fresh() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture = write_fixture(
        "multi_requires",
        r#"
atom safe_add_with_bound(x: i64, y: i64) -> i64
  requires: x >= 0 && y >= 0 && x + y <= 2**64 - 1;
  ensures: result == x + y && 0 <= result <= 2**64 - 1;
  body: x + y;
"#,
    );

    let output = Command::new(bin)
        .arg("verify")
        .arg(&fixture)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify: {err}"));

    assert!(
        output.status.success(),
        "multi-requires fixture should verify fresh\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("'safe_add_with_bound': verified ✅"),
        "expected safe_add_with_bound to verify, got:\n{combined}"
    );

    std::fs::remove_dir_all(fixture.parent().unwrap()).expect("remove multi-requires fixture dir");
}

#[test]
fn verify_encodes_tuple_result_components_without_skipping() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let dir = std::env::temp_dir().join(format!(
        "mumei_tuple_result_indexing_{}",
        std::process::id()
    ));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale tuple result fixture dir");
    }
    std::fs::create_dir_all(&dir).expect("create tuple result fixture dir");
    let fixture = dir.join("main.mm");
    std::fs::copy(
        std::path::Path::new(manifest_dir).join("tests/test_tuple_result_indexing.mm"),
        &fixture,
    )
    .expect("copy tuple result fixture");

    let output = Command::new(bin)
        .arg("verify")
        .arg(&fixture)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify: {err}"));

    assert!(
        output.status.success(),
        "tuple result fixture should verify\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("'SafeAdd': verified ✅") || combined.contains("'SafeAdd': trusted ✅"),
        "expected tuple result atom to verify without skipping, got:\n{combined}"
    );
    assert!(
        !combined.contains("Skipped unsupported Z3 clause")
            && !combined.contains("satisfiable_with_skips")
            && !combined.contains("unverifiable"),
        "tuple result indexing must not be skipped, got:\n{combined}"
    );
    std::fs::remove_dir_all(dir).expect("remove tuple result fixture dir");
}

#[test]
fn verify_treats_trivial_ensures_conjunct_as_noop() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture = write_fixture(
        "trivial_ensures",
        r#"
atom trivial_ensures(x: i64) -> i64
  requires: x >= 0;
  ensures: result == x && true;
  body: x;
"#,
    );

    let output = Command::new(bin)
        .arg("verify")
        .arg(&fixture)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify: {err}"));

    assert!(
        output.status.success(),
        "trivial ensures fixture should verify\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("'trivial_ensures': verified ✅"),
        "expected trivial_ensures to verify, got:\n{combined}"
    );

    std::fs::remove_dir_all(fixture.parent().unwrap()).expect("remove trivial ensures fixture dir");
}

#[test]
fn verify_json_preserves_skip_diagnostics_for_unverifiable_atoms() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture = write_fixture(
        "json_unverifiable",
        r#"
atom symbolic_pow(x: i64, y: i64) -> i64
  requires: x >= 0;
  ensures: result == x**y && result == x;
  body: x;
"#,
    );

    let output = Command::new(bin)
        .arg("verify")
        .arg("--json")
        .arg("--report-dir")
        .arg(fixture.parent().unwrap())
        .arg(&fixture)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify --json: {err}"));

    assert!(
        !output.status.success(),
        "--json unverifiable fixture should exit non-zero\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let payload: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|err| panic!("{err}: {stdout}"));
    let diagnostics = payload["diagnostics"]
        .as_array()
        .expect("diagnostics should be an array");
    assert!(
        !diagnostics.is_empty(),
        "expected diagnostics to be preserved in --json payload, got:\n{payload}"
    );
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic
                .as_str()
                .map(|s| s.contains("Skipped unsupported Z3 clause"))
                .unwrap_or(false)
                || diagnostic
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .map(|s| s.contains("Skipped unsupported Z3 clause"))
                    .unwrap_or(false)
        }),
        "expected skip diagnostic to be present in diagnostics, got:\n{payload}"
    );
    let warnings = payload["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(
        warnings.iter().all(|warning| {
            warning
                .as_str()
                .map(|s| !s.is_empty())
                .unwrap_or_else(|| warning.is_object())
        }),
        "expected warnings array to remain well-formed, got:\n{payload}"
    );

    std::fs::remove_dir_all(fixture.parent().unwrap())
        .expect("remove json unverifiable fixture dir");
}

#[test]
fn verify_json_preserves_skipped_clause_visibility_on_cache_hit() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture = write_fixture(
        "json_cached_partial",
        r#"
atom identity_with_unsupported_requires(x: i64) -> i64
  requires: is_hex_digit(x);
  ensures: result == x;
  body: { x };
"#,
    );
    let report_dir = fixture.parent().unwrap();

    let run_verify = || {
        Command::new(bin)
            .arg("verify")
            .arg("--json")
            .arg("--report-dir")
            .arg(report_dir)
            .arg(&fixture)
            .current_dir(manifest_dir)
            .output()
            .unwrap_or_else(|err| panic!("failed to run cached partial verify: {err}"))
    };

    let first = run_verify();
    assert!(
        first.status.success(),
        "fresh partial verification should succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    let first_payload: serde_json::Value =
        serde_json::from_slice(&first.stdout).expect("fresh output should be JSON");

    let second = run_verify();
    assert!(
        second.status.success(),
        "cached partial verification should succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr)
    );
    let second_payload: serde_json::Value =
        serde_json::from_slice(&second.stdout).expect("cached output should be JSON");

    assert_eq!(first_payload["status"], "success");
    assert_eq!(first_payload["skipped_clauses"], 1);
    assert_eq!(first_payload["partial"], true);
    assert_eq!(
        second_payload["skipped_clauses"],
        first_payload["skipped_clauses"]
    );
    assert_eq!(second_payload["partial"], true);

    std::fs::remove_dir_all(report_dir).expect("remove cached partial fixture dir");
}

#[test]
fn verify_json_cache_hit_preserves_multi_atom_aggregate() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    // A partial (skipped-requires) atom alongside a clean atom. The module
    // aggregate must report the single real skip both on the fresh run and on
    // the fully-cached re-run, exercising the cache-hit + aggregation join
    // rather than only the single-atom cache path.
    let fixture = write_fixture(
        "json_cached_multi_aggregate",
        r#"
atom identity_with_unsupported_requires(x: i64) -> i64
  requires: is_hex_digit(x);
  ensures: result == x;
  body: { x };

atom clean_identity(x: i64) -> i64
  requires: true;
  ensures: result == x;
  body: { x };
"#,
    );
    let report_dir = fixture.parent().unwrap();

    let run_verify = || {
        Command::new(bin)
            .arg("verify")
            .arg("--json")
            .arg("--report-dir")
            .arg(report_dir)
            .arg(&fixture)
            .current_dir(manifest_dir)
            .output()
            .unwrap_or_else(|err| panic!("failed to run cached multi-atom verify: {err}"))
    };

    let first = run_verify();
    assert!(
        first.status.success(),
        "fresh multi-atom partial verification should succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    let first_payload: serde_json::Value =
        serde_json::from_slice(&first.stdout).expect("fresh output should be JSON");

    let second = run_verify();
    assert!(
        second.status.success(),
        "cached multi-atom partial verification should succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr)
    );
    let second_payload: serde_json::Value =
        serde_json::from_slice(&second.stdout).expect("cached output should be JSON");

    assert_eq!(
        first_payload["status"], "success",
        "payload:\n{first_payload}"
    );
    assert_eq!(
        first_payload["skipped_clauses"], 1,
        "payload:\n{first_payload}"
    );
    assert_eq!(first_payload["partial"], true, "payload:\n{first_payload}");
    // The cache-hit re-run must surface the same module aggregate, not 0 (the
    // clean atom's per-report value) and not a doubled count.
    assert_eq!(
        second_payload["skipped_clauses"], first_payload["skipped_clauses"],
        "payload:\n{second_payload}"
    );
    assert_eq!(
        second_payload["partial"], true,
        "payload:\n{second_payload}"
    );

    std::fs::remove_dir_all(report_dir).expect("remove cached multi-atom fixture dir");
}

#[test]
fn verify_json_aggregate_counts_skips_from_unverifiable_atoms() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture = write_fixture(
        "json_mixed_aggregate",
        r#"
atom symbolic_pow(x: i64, y: i64) -> i64
  requires: x >= 0;
  ensures: result == x**y && result == x;
  body: { x };

atom identity_with_unsupported_requires(x: i64) -> i64
  requires: is_hex_digit(x);
  ensures: result == x;
  body: { x };
"#,
    );
    let report_dir = fixture.parent().unwrap();

    let output = Command::new(bin)
        .arg("verify")
        .arg("--json")
        .arg("--report-dir")
        .arg(report_dir)
        .arg(&fixture)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mixed aggregate verify: {err}"));

    let payload: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("mixed output should be JSON");

    assert_eq!(payload["status"], "unverifiable", "payload:\n{payload}");
    // The unverifiable atom's skipped clause must contribute to the module
    // aggregate alongside the success-with-skipped-requires atom.
    assert_eq!(payload["skipped_clauses"], 2, "payload:\n{payload}");
    assert_eq!(payload["partial"], true, "payload:\n{payload}");

    std::fs::remove_dir_all(report_dir).expect("remove mixed aggregate fixture dir");
}

#[test]
fn verify_json_aggregate_not_inflated_by_failing_atom() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    // A success-with-skipped-requires atom followed by a postcondition-failing
    // atom. All atoms share output_dir/report.json, so the module aggregate must
    // reflect only the real skip (1), not double-count the prior atom's report.
    let fixture = write_fixture(
        "json_failing_no_inflation",
        r#"
atom identity_with_unsupported_requires(x: i64) -> i64
  requires: is_hex_digit(x);
  ensures: result == x;
  body: { x };

atom bad_postcondition(x: i64) -> i64
  requires: x >= 0;
  ensures: result == x + 1;
  body: { x };
"#,
    );
    let report_dir = fixture.parent().unwrap();

    let output = Command::new(bin)
        .arg("verify")
        .arg("--json")
        .arg("--report-dir")
        .arg(report_dir)
        .arg(&fixture)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run failing-inflation verify: {err}"));

    let payload: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("mixed output should be JSON");

    assert_eq!(payload["status"], "failed", "payload:\n{payload}");
    assert_eq!(payload["skipped_clauses"], 1, "payload:\n{payload}");

    std::fs::remove_dir_all(report_dir).expect("remove failing-inflation fixture dir");
}

#[test]
fn verify_json_surfaces_aggregate_for_all_success_module() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture = write_fixture(
        "json_all_success_aggregate",
        r#"
atom identity_with_unsupported_requires(x: i64) -> i64
  requires: is_hex_digit(x);
  ensures: result == x;
  body: { x };

atom clean_identity(x: i64) -> i64
  requires: true;
  ensures: result == x;
  body: { x };
"#,
    );
    let report_dir = fixture.parent().unwrap();

    let output = Command::new(bin)
        .arg("verify")
        .arg("--json")
        .arg("--report-dir")
        .arg(report_dir)
        .arg(&fixture)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run all-success aggregate verify: {err}"));

    assert!(
        output.status.success(),
        "all-success module should verify\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("all-success output should be JSON");

    assert_eq!(payload["status"], "success", "payload:\n{payload}");
    // Even though every atom passes, the module-level skipped-clause aggregate
    // must be surfaced (not just the last atom's per-report value).
    assert_eq!(payload["skipped_clauses"], 1, "payload:\n{payload}");
    assert_eq!(payload["partial"], true, "payload:\n{payload}");

    std::fs::remove_dir_all(report_dir).expect("remove all-success aggregate fixture dir");
}
