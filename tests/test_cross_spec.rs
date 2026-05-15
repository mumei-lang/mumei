use serde_json::Value;
use std::process::Command;

#[test]
fn cross_spec_verify_writes_report() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let report_dir = std::env::temp_dir().join(format!("mumei_cross_spec_{}", std::process::id()));
    if report_dir.exists() {
        std::fs::remove_dir_all(&report_dir).expect("clean stale report dir");
    }

    let output = Command::new(bin)
        .arg("verify")
        .arg("--cross-spec-verify")
        .arg("--report-dir")
        .arg(&report_dir)
        .arg("tests/test_cross_spec.mm")
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify --cross-spec-verify: {err}"));

    assert!(
        output.status.success(),
        "cross-spec fixture should verify\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let report_path = report_dir.join("cross_spec.json");
    let report = std::fs::read_to_string(&report_path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", report_path.display()));
    let report: Value = serde_json::from_str(&report).expect("valid cross_spec.json");

    assert_eq!(report["summary"]["total_atoms"], 6);
    assert_eq!(report["summary"]["inconsistent_calls"], 0);
    assert_eq!(report["summary"]["global_invariant_count"], 2);

    let contracts = report["contract_consistency"]
        .as_array()
        .expect("contract consistency array");
    assert_eq!(contracts.len(), 1);
    assert_eq!(contracts[0]["caller_atom"], "transfer");
    assert_eq!(contracts[0]["callee_atom"], "validate_balance");
    assert_eq!(contracts[0]["is_consistent"], true);

    let invariants = report["global_invariants"]
        .as_array()
        .expect("global invariants array");
    assert!(invariants
        .iter()
        .any(|invariant| invariant["invariant"] == "result >= 0"));

    std::fs::remove_dir_all(report_dir).expect("remove report dir");
}
