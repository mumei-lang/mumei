use serde_json::Value;
use std::process::Command;

#[test]
fn cross_spec_verify_writes_report_by_default() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let report_dir = std::env::temp_dir().join(format!("mumei_cross_spec_{}", std::process::id()));
    if report_dir.exists() {
        std::fs::remove_dir_all(&report_dir).expect("clean stale report dir");
    }

    let output = Command::new(bin)
        .arg("verify")
        .arg("--report-dir")
        .arg(&report_dir)
        .arg("tests/test_cross_spec.mm")
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify: {err}"));

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

#[test]
fn cross_spec_verify_merges_multiple_files() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let report_dir =
        std::env::temp_dir().join(format!("mumei_cross_spec_multi_{}", std::process::id()));
    if report_dir.exists() {
        std::fs::remove_dir_all(&report_dir).expect("clean stale report dir");
    }

    let output = Command::new(bin)
        .arg("verify")
        .arg("--report-dir")
        .arg(&report_dir)
        .arg("--cross-spec-files")
        .arg("tests/test_cross_spec_multi_file_dep.mm")
        .arg("tests/test_cross_spec_multi_file.mm")
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify: {err}"));

    assert!(
        output.status.success(),
        "multi-file cross-spec fixture should verify\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let report_path = report_dir.join("cross_spec.json");
    let report = std::fs::read_to_string(&report_path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", report_path.display()));
    let report: Value = serde_json::from_str(&report).expect("valid cross_spec.json");

    assert!(
        report["summary"]["total_atoms"]
            .as_u64()
            .expect("total_atoms")
            >= 4
    );
    assert_eq!(report["summary"]["inconsistent_calls"], 1);
    assert_eq!(
        report["agent_artifact_mapping"][0]["cross_spec_field"],
        "contract_consistency[]"
    );
    assert_eq!(
        report["agent_artifact_mapping"][0]["agent_field"],
        "missing_constraints[]"
    );
    assert!(
        report["summary"]["global_invariant_conflict_count"]
            .as_u64()
            .expect("global_invariant_conflict_count")
            >= 1
    );

    let contracts = report["contract_consistency"]
        .as_array()
        .expect("contract consistency array");
    let cross_file_call = contracts
        .iter()
        .find(|entry| entry["caller_atom"] == "cross_file_caller")
        .expect("cross-file caller entry");
    assert_eq!(cross_file_call["callee_atom"], "cross_file_callee");
    assert_eq!(cross_file_call["is_consistent"], false);
    assert!(cross_file_call["caller_file"]
        .as_str()
        .expect("caller_file")
        .ends_with("test_cross_spec_multi_file.mm"));
    assert!(cross_file_call["callee_file"]
        .as_str()
        .expect("callee_file")
        .ends_with("test_cross_spec_multi_file_dep.mm"));

    let conflicts = report["global_invariant_conflicts"]
        .as_array()
        .expect("global invariant conflicts array");
    assert!(conflicts.iter().any(|conflict| {
        conflict["left_source_files"]
            .as_array()
            .expect("left_source_files")
            .iter()
            .chain(
                conflict["right_source_files"]
                    .as_array()
                    .expect("right_source_files")
                    .iter(),
            )
            .filter_map(Value::as_str)
            .any(|file| file.ends_with("test_cross_spec_multi_file.mm"))
            && conflict["message"]
                .as_str()
                .expect("conflict message")
                .contains("Global invariant conflict")
    }));

    std::fs::remove_dir_all(report_dir).expect("remove report dir");
}
