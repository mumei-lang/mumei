use std::process::Command;

use mumei_core::reconstruction_loss::ReconstructionLoss;
use mumei_core::structured_feedback::{
    feedback_instruction_for_error_type, Location, StructuredFeedback, STATUS_VERIFICATION_FAILED,
    STATUS_VERIFICATION_PASSED,
};
use serde_json::Value;

#[test]
fn structured_feedback_json_roundtrip_preserves_schema() {
    let feedback = StructuredFeedback {
        status: STATUS_VERIFICATION_FAILED.to_string(),
        error_type: Some("postcondition_violated".to_string()),
        location: Some(Location {
            file: "main.mm".to_string(),
            line: 12,
        }),
        reconstruction_loss: Some(ReconstructionLoss::from_counter_example(
            "result > 10",
            std::collections::HashMap::from([("x".to_string(), Value::Number(5.into()))]),
        )),
        feedback_instruction: "Fix the body to satisfy `ensures`.".to_string(),
    };

    let encoded = feedback.to_json().expect("serialize structured feedback");
    let decoded = StructuredFeedback::from_json(&encoded).expect("parse structured feedback");

    assert_eq!(decoded.status, STATUS_VERIFICATION_FAILED);
    assert_eq!(
        decoded.error_type.as_deref(),
        Some("postcondition_violated")
    );
    assert_eq!(decoded.location.as_ref().map(|loc| loc.line), Some(12));
    assert!(decoded.reconstruction_loss.is_some());
    assert!(decoded.feedback_instruction.contains("ensures"));

    let schema = StructuredFeedback::json_schema();
    assert_eq!(schema["title"], "MumeiStructuredFeedback");
    assert!(schema["required"]
        .as_array()
        .unwrap()
        .contains(&Value::String("feedback_instruction".to_string())));
    assert!(schema["properties"]["status"]["enum"]
        .as_array()
        .unwrap()
        .contains(&Value::String(STATUS_VERIFICATION_PASSED.to_string())));
}

#[test]
fn structured_feedback_from_report_extracts_loss_and_instruction() {
    let report = serde_json::json!({
        "status": "failed",
        "failure_type": "postcondition_violated",
        "counterexample": {"x": "5"},
        "span": {"file": "main.mm", "line": 7},
        "semantic_feedback": {
            "reconstruction_loss": {
                "violated_property": "result > 10",
                "counter_example": {"x": "5"},
                "loss_vector": [5.0]
            }
        }
    });

    let feedback = StructuredFeedback::from_report(&report);

    assert_eq!(feedback.status, STATUS_VERIFICATION_FAILED);
    assert_eq!(
        feedback.error_type.as_deref(),
        Some("postcondition_violated")
    );
    assert_eq!(
        feedback.location.as_ref().map(|loc| loc.file.as_str()),
        Some("main.mm")
    );
    assert!(feedback.reconstruction_loss.is_some());
    assert!(feedback.feedback_instruction.contains("ensures"));
    assert!(feedback.feedback_instruction.contains("x"));
}

#[test]
fn violation_types_generate_actionable_instructions() {
    let cases = [
        ("division_by_zero", "divisor"),
        ("linearity_violated", "linear"),
        ("invariant_violated", "contradictory"),
        ("postcondition_violated", "ensures"),
        ("precondition_violated", "requires"),
        ("temporal_effect_violated", "state"),
        ("effect_not_allowed", "Effect"),
        ("trait_law_violated", "trait law"),
        ("exhaustiveness_failed", "missing cases"),
        ("resource_conflict", "Resource"),
    ];

    for (failure_type, expected) in cases {
        let instruction = feedback_instruction_for_error_type(failure_type, None);
        assert!(
            instruction.contains(expected),
            "{failure_type} instruction `{instruction}` did not contain `{expected}`"
        );
    }
}

#[test]
fn verify_emit_structured_feedback_writes_failure_json() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let dir =
        std::env::temp_dir().join(format!("mumei_structured_feedback_{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale structured feedback dir");
    }
    std::fs::create_dir_all(&dir).expect("create structured feedback dir");
    let fixture = dir.join("main.mm");
    let output_path = dir.join("structured_feedback.json");
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
    .expect("write structured feedback fixture");

    let output = Command::new(bin)
        .arg("verify")
        .arg("--disable-spurious-detection")
        .arg("--emit")
        .arg("structured-feedback")
        .arg("--output")
        .arg(&output_path)
        .arg("--report-dir")
        .arg(&report_dir)
        .arg(&fixture)
        .current_dir(manifest_dir)
        .output()
        .expect("run mumei verify --emit structured-feedback");

    assert!(
        !output.status.success(),
        "failing fixture should return nonzero\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output_path.exists(), "expected structured feedback JSON");

    let payload: Value =
        serde_json::from_str(&std::fs::read_to_string(&output_path).expect("read output json"))
            .expect("parse output json");
    assert_eq!(payload["status"], STATUS_VERIFICATION_FAILED);
    assert_eq!(payload["error_type"], "postcondition_violated");
    assert!(payload["reconstruction_loss"]["violated_property"].is_string());
    assert!(payload["feedback_instruction"]
        .as_str()
        .unwrap()
        .contains("ensures"));

    let report: Value =
        serde_json::from_str(&std::fs::read_to_string(report_dir.join("report.json")).unwrap())
            .expect("parse report json");
    assert_eq!(
        report["structured_feedback"]["status"],
        STATUS_VERIFICATION_FAILED
    );
}

#[test]
fn verify_emit_structured_feedback_stdout_for_success() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let dir = std::env::temp_dir().join(format!(
        "mumei_structured_feedback_success_{}",
        std::process::id()
    ));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale structured feedback success dir");
    }
    std::fs::create_dir_all(&dir).expect("create structured feedback success dir");
    let fixture = dir.join("main.mm");
    std::fs::write(
        &fixture,
        r#"
atom identity(x: i64) -> i64
requires: true;
ensures: result == x;
body: { x };
"#,
    )
    .expect("write passing structured feedback fixture");

    let output = Command::new(bin)
        .arg("verify")
        .arg("--emit")
        .arg("structured-feedback")
        .arg("--report-dir")
        .arg(dir.join("report"))
        .arg(&fixture)
        .current_dir(manifest_dir)
        .output()
        .expect("run mumei verify --emit structured-feedback");

    assert!(
        output.status.success(),
        "passing fixture should return zero\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout).expect("parse stdout JSON");
    assert_eq!(payload["status"], STATUS_VERIFICATION_PASSED);
    assert!(payload["error_type"].is_null());
}
