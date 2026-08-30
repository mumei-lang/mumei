use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

fn report_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mumei_proof_graph_{tag}_{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale report dir");
    }
    dir
}

/// Run `verify --emit proof-graph` over a multi-file project; `files[0]` is the
/// entry point and the rest are passed via `--cross-spec-files`.
fn emit_proof_graph(tag: &str, files: &[&str]) -> (bool, String, Value) {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let dir = report_dir(tag);

    let mut command = Command::new(bin);
    command
        .arg("verify")
        .arg("--report-dir")
        .arg(&dir)
        .arg("--emit")
        .arg("proof-graph")
        .arg("--cross-spec-files");
    for file in &files[1..] {
        command.arg(file);
    }
    let output = command
        .arg(files[0])
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify: {err}"));

    let combined = format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let graph_path = dir.join("proof_graph.json");
    let graph = std::fs::read_to_string(&graph_path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}\n{combined}", graph_path.display()));
    let graph: Value = serde_json::from_str(&graph).expect("valid proof_graph.json");
    (output.status.success(), combined, graph)
}

fn node<'a>(graph: &'a Value, atom_name: &str) -> &'a Value {
    graph["nodes"]
        .as_array()
        .expect("nodes array")
        .iter()
        .find(|node| node["atom_name"] == atom_name)
        .unwrap_or_else(|| panic!("missing node '{atom_name}' in {graph:#}"))
}

#[test]
fn proof_graph_export_carries_contracts_edges_and_trust_boundaries() {
    let (success, log, graph) = emit_proof_graph(
        "multi_file",
        &[
            "tests/test_cross_spec_multi_file.mm",
            "tests/test_cross_spec_multi_file_dep.mm",
        ],
    );

    assert!(success, "multi-file fixture should verify\n{log}");
    assert_eq!(graph["version"], "1.0");
    assert!(
        log.contains("Proof graph written to:"),
        "the emit target must report its path\n{log}"
    );

    let caller = node(&graph, "cross_file_caller");
    assert_eq!(caller["requires"], "x >= 0");
    assert!(caller["source_file"]
        .as_str()
        .expect("source_file")
        .ends_with("test_cross_spec_multi_file.mm"));
    assert_eq!(
        caller["dependencies"]
            .as_array()
            .expect("dependencies")
            .as_slice(),
        [Value::from("cross_file_callee")]
    );

    let callee = node(&graph, "cross_file_callee");
    assert_eq!(
        callee["dependents"].as_array().expect("dependents").len(),
        1
    );
    assert!(callee["source_file"]
        .as_str()
        .expect("source_file")
        .ends_with("test_cross_spec_multi_file_dep.mm"));

    // The inconsistent cross-file call is the edge an interactive viewer needs
    // to highlight, and it must come with the cross-spec violation text.
    let edges = graph["edges"].as_array().expect("edges array");
    let edge = edges
        .iter()
        .find(|edge| edge["from"] == "cross_file_caller" && edge["to"] == "cross_file_callee")
        .unwrap_or_else(|| panic!("missing cross-file edge in {graph:#}"));
    assert_eq!(edge["is_consistent"], false);
    assert!(!edge["violations"]
        .as_array()
        .expect("violations")
        .is_empty());

    // Both fixture atoms are `trusted`, so P23 classification puts them on the
    // yellow proof-hole boundary rather than fully-proven green.
    for name in ["cross_file_caller", "cross_file_callee"] {
        let entry = node(&graph, name);
        assert_eq!(entry["health"], "yellow");
        assert_eq!(entry["verification_status"], "verified");
        assert_eq!(
            entry["trust_boundaries"]
                .as_array()
                .expect("trust boundaries")
                .iter()
                .map(|boundary| boundary["kind"].as_str().expect("kind"))
                .collect::<Vec<_>>(),
            ["trusted_atom"]
        );
    }

    assert_eq!(
        graph["summary"]["node_count"],
        graph["nodes"].as_array().expect("nodes").len()
    );
    assert_eq!(graph["summary"]["edge_count"], edges.len());
    assert!(
        graph["summary"]["yellow_count"].as_u64().expect("yellow") >= 2,
        "trusted atoms must be counted as yellow in {graph:#}"
    );
}

#[test]
fn proof_graph_export_attaches_session_protocol_violations_to_atoms() {
    let (success, log, graph) = emit_proof_graph(
        "session_deadlock",
        &[
            "tests/fixtures/session_types/payment_client.mm",
            "tests/fixtures/session_types/payment_server.mm",
        ],
    );

    assert!(
        !success,
        "the deadlocking protocol must still fail verification\n{log}"
    );

    let violations = graph["session_protocol_violations"]
        .as_array()
        .expect("session violation array");
    assert_eq!(violations.len(), 1, "expected one violation in {graph:#}");
    assert_eq!(violations[0]["kind"], "deadlock_no_progress");
    assert_eq!(graph["summary"]["session_protocol_violation_count"], 1);

    // Nodes reference violations by index so the payload is stored once.
    for name in ["payment_client_request", "payment_server_respond"] {
        assert_eq!(
            node(&graph, name)["session_protocol_violations"]
                .as_array()
                .expect("node violation indices")
                .as_slice(),
            [Value::from(0)],
            "'{name}' should point at the deadlock violation in {graph:#}"
        );
    }
    assert!(
        node(&graph, "payment_client_retry")["session_protocol_violations"]
            .as_array()
            .expect("node violation indices")
            .is_empty()
    );

    // `effect_pre` overrides are the P23 boundary these session atoms cross.
    let request = node(&graph, "payment_client_request");
    assert_eq!(request["health"], "yellow");
    assert_eq!(
        request["trust_boundaries"][0]["kind"],
        "effect_pre_override"
    );
    assert!(!request["trust_boundaries"][0]["rationale"]
        .as_str()
        .expect("rationale")
        .is_empty());
    assert_eq!(
        request["effects"].as_array().expect("effects").as_slice(),
        [Value::from("PaymentChannel")]
    );
}

#[test]
fn a_directory_input_yields_one_graph_covering_every_file() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let report = report_dir("directory_report");
    let project = report_dir("directory_project");
    std::fs::create_dir_all(&project).expect("create project dir");
    for name in [
        "test_cross_spec_multi_file.mm",
        "test_cross_spec_multi_file_dep.mm",
    ] {
        std::fs::copy(
            PathBuf::from(manifest_dir).join("tests").join(name),
            project.join(name),
        )
        .unwrap_or_else(|err| panic!("failed to stage {name}: {err}"));
    }

    let output = Command::new(bin)
        .arg("verify")
        .arg("--report-dir")
        .arg(&report)
        .arg("--emit")
        .arg("proof-graph")
        .arg(&project)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify: {err}"));
    let log = format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "directory verify failed\n{log}");

    let graph: Value = serde_json::from_str(
        &std::fs::read_to_string(report.join("proof_graph.json"))
            .unwrap_or_else(|err| panic!("failed to read directory proof graph: {err}\n{log}")),
    )
    .expect("valid proof_graph.json");

    // Both files land in the single graph, together with the cross-file edge
    // they form; a per-file graph would have kept only the last file's atoms.
    for name in [
        "cross_file_caller",
        "cross_file_callee",
        "high_global_result",
        "low_global_result",
    ] {
        node(&graph, name);
    }
    assert_eq!(
        node(&graph, "cross_file_caller")["dependencies"]
            .as_array()
            .expect("dependencies")
            .as_slice(),
        [Value::from("cross_file_callee")]
    );
    assert!(
        graph["edges"]
            .as_array()
            .expect("edges")
            .iter()
            .any(|edge| edge["from"] == "cross_file_caller" && edge["to"] == "cross_file_callee"),
        "the cross-file call must appear as an edge in {graph:#}"
    );
}

/// An atom rejected before it ever reaches the solver records no certificate
/// result, so the graph has to take the rejection diagnostic into account or the
/// atom would be painted as proven.
#[test]
fn an_atom_rejected_before_verification_is_red() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let dir = report_dir("strict_array");

    let output = Command::new(bin)
        .arg("verify")
        .arg("--report-dir")
        .arg(&dir)
        .arg("--emit")
        .arg("proof-graph")
        .arg("--strict-array-types")
        .arg("tests/test_untyped_array_access.mm")
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify: {err}"));

    let log = format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let graph: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("proof_graph.json"))
            .unwrap_or_else(|err| panic!("failed to read proof graph: {err}\n{log}")),
    )
    .expect("valid proof_graph.json");

    let rejected = node(&graph, "uses_untyped_array");
    assert_eq!(rejected["health"], "red", "in {graph:#}");
    assert_eq!(rejected["verification_status"], "failed");
    assert_eq!(node(&graph, "uses_typed_i64_array")["health"], "green");
}

#[test]
fn proof_graph_is_not_emitted_without_the_emit_target() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let dir = report_dir("no_emit");

    let output = Command::new(bin)
        .arg("verify")
        .arg("--report-dir")
        .arg(&dir)
        .arg("--cross-spec-verify")
        .arg("tests/test_cross_spec.mm")
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify: {err}"));

    assert!(output.status.success());
    assert!(dir.join("cross_spec.json").exists());
    assert!(
        !dir.join("proof_graph.json").exists(),
        "proof_graph.json must stay opt-in"
    );
}
