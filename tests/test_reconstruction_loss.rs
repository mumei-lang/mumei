use std::collections::HashMap;
use std::process::Command;

use mumei_core::parser::{Atom, Span, TrustLevel};
use mumei_core::proof_cert;
use mumei_core::reconstruction_loss::ReconstructionLoss;
use mumei_core::verification::ModuleEnv;
use serde_json::Value;

fn atom(name: &str) -> Atom {
    Atom {
        name: name.to_string(),
        type_params: Vec::new(),
        where_bounds: Vec::new(),
        params: Vec::new(),
        trace_id: None,
        spec_metadata: HashMap::new(),
        requires: "true".to_string(),
        forall_constraints: Vec::new(),
        ensures: "result > 10".to_string(),
        body_expr: "0".to_string(),
        consumed_params: Vec::new(),
        resources: Vec::new(),
        is_async: false,
        trust_level: TrustLevel::Verified,
        max_unroll: None,
        invariant: None,
        effects: Vec::new(),
        return_type: None,
        span: Span::default(),
        effect_pre: HashMap::new(),
        effect_post: HashMap::new(),
    }
}

#[test]
fn reconstruction_loss_from_counterexample_value_is_stable() {
    let counterexample = serde_json::json!({
        "x": "5",
        "result": 6,
    });

    let loss = ReconstructionLoss::from_counterexample_value("result > 10", &counterexample)
        .expect("object counterexample");

    assert_eq!(loss.violated_property, "result > 10");
    assert_eq!(
        loss.counter_example.get("x"),
        Some(&Value::String("5".to_string()))
    );
    assert_eq!(
        loss.counter_example.get("result"),
        Some(&Value::Number(6.into()))
    );
    assert_eq!(loss.loss_vector, vec![6.0, 5.0]);
    assert!(!loss.is_zero_loss());
}

#[test]
fn zero_loss_detection_handles_empty_and_zero_components() {
    let empty = ReconstructionLoss::from_counter_example("result == 0", HashMap::new());
    assert!(empty.is_zero_loss());

    let zero = ReconstructionLoss::from_counter_example(
        "result == 0",
        HashMap::from([("x".to_string(), Value::Number(0.into()))]),
    );
    assert!(zero.is_zero_loss());
}

#[test]
fn proof_certificate_includes_reconstruction_loss_metadata() {
    let module_env = ModuleEnv::new();
    let bad = atom("bad");
    let results = HashMap::from([("bad".to_string(), ("sat".to_string(), "failed".to_string()))]);
    let loss = ReconstructionLoss::from_counter_example(
        "result > 10",
        HashMap::from([("x".to_string(), Value::Number(5.into()))]),
    );
    let losses = HashMap::from([("bad".to_string(), loss.clone())]);

    let cert = proof_cert::generate_certificate_with_reconstruction_losses(
        "bad.mm",
        &[&bad],
        &results,
        &module_env,
        None,
        None,
        None,
        Some(&losses),
    );

    assert_eq!(cert.reconstruction_loss, Some(vec![loss.clone()]));
    assert_eq!(cert.atoms[0].reconstruction_loss, Some(loss));
}

#[test]
fn verify_emit_reconstruction_loss_writes_sat_counterexample_json() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let dir =
        std::env::temp_dir().join(format!("mumei_reconstruction_loss_{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale reconstruction loss dir");
    }
    std::fs::create_dir_all(&dir).expect("create reconstruction loss dir");
    let fixture = dir.join("main.mm");
    let output_path = dir.join("loss.json");
    let report_dir = dir.join("report");
    std::fs::write(
        &fixture,
        r#"
atom bad(x: i64) -> i64
requires: x == 5;
ensures: result > 10;
body: { x + 1 };
"#,
    )
    .expect("write reconstruction loss fixture");

    let output = Command::new(bin)
        .arg("verify")
        .arg("--disable-spurious-detection")
        .arg("--emit")
        .arg("reconstruction-loss")
        .arg("--output")
        .arg(&output_path)
        .arg("--report-dir")
        .arg(&report_dir)
        .arg(&fixture)
        .current_dir(manifest_dir)
        .output()
        .expect("run mumei verify --emit reconstruction-loss");

    assert!(
        !output.status.success(),
        "failing fixture should return nonzero\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output_path.exists(), "expected reconstruction loss JSON");

    let payload: Value =
        serde_json::from_str(&std::fs::read_to_string(&output_path).expect("read loss json"))
            .expect("parse loss json");
    assert_eq!(payload["reconstruction_loss_count"], 1);
    assert_eq!(payload["reconstruction_losses"][0]["atom"], "bad");
    assert_eq!(
        payload["reconstruction_losses"][0]["violated_property"],
        "result > 10"
    );
    assert!(payload["reconstruction_losses"][0]["counter_example"]["x"].is_string());
    assert!(payload["reconstruction_losses"][0]["loss_vector"].is_array());

    let report: Value =
        serde_json::from_str(&std::fs::read_to_string(report_dir.join("report.json")).unwrap())
            .expect("parse report json");
    assert_eq!(
        report["semantic_feedback"]["reconstruction_loss"]["violated_property"],
        "result > 10"
    );
}
