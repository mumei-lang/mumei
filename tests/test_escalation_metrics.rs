use std::{collections::HashMap, process::Command};

use mumei_core::proof_cert::{AtomCertificate, EscalationCandidate, LeanResultMetadata};
use mumei_core::resolver::LeanEscalationMetrics;
use mumei_core::verification::TranslatorIRMetadata;

fn candidate(
    name: &str,
    reason: &str,
    tags: &[&str],
    lean_status: Option<&str>,
    manual_lemma_reason: Option<&str>,
) -> EscalationCandidate {
    EscalationCandidate {
        name: name.to_string(),
        z3_check_result: "unknown".to_string(),
        z3_result_class: "unknown".to_string(),
        status: "lean_escalation_candidate".to_string(),
        content_hash: format!("{name}-content"),
        proof_hash: format!("{name}-proof"),
        dependencies: Vec::new(),
        effects: Vec::new(),
        requires: "true".to_string(),
        ensures: "result >= 0".to_string(),
        escalation_reason: reason.to_string(),
        logic_fragment_tags: tags.iter().map(|tag| tag.to_string()).collect(),
        translator_version: "mumei-lean-translator-ir-v1".to_string(),
        binder_mapping: HashMap::new(),
        bridge_lemma_hash: "bridge".to_string(),
        manual_lemma_reason: manual_lemma_reason.map(str::to_string),
        translator_ir: TranslatorIRMetadata::default(),
        lean_metadata: lean_status.map(|status| LeanResultMetadata {
            status: status.to_string(),
            ..LeanResultMetadata::default()
        }),
    }
}

fn lean_verified_atom(name: &str, reason: Option<&str>, tags: &[&str]) -> AtomCertificate {
    AtomCertificate {
        name: name.to_string(),
        z3_check_result: "lean_verified".to_string(),
        content_hash: format!("{name}-content"),
        status: "verified".to_string(),
        spec_validation_result: None,
        proof_hash: format!("{name}-proof"),
        dependencies: Vec::new(),
        effects: Vec::new(),
        requires: "true".to_string(),
        ensures: "result >= 0".to_string(),
        z3_result_class: "unknown".to_string(),
        escalation_reason: reason.map(str::to_string),
        logic_fragment_tags: tags.iter().map(|tag| tag.to_string()).collect(),
        translator_version: "mumei-lean-translator-ir-v1".to_string(),
        binder_mapping: HashMap::new(),
        bridge_lemma_hash: "bridge".to_string(),
        manual_lemma_reason: None,
        translator_ir: TranslatorIRMetadata::default(),
        lean_metadata: Some(LeanResultMetadata {
            status: "lean_verified".to_string(),
            ..LeanResultMetadata::default()
        }),
        counterexample_validation: None,
        reconstruction_loss: None,
        self_correction_metadata: None,
        symbol_provenance: Vec::new(),
        unused_hypotheses: None,
        solver_process_metadata: None,
        retry_policy_fingerprint: None,
        attempt_summary: None,
        cost_success_metrics: None,
    }
}

#[test]
fn test_escalation_metrics_collection() {
    let mut metrics = LeanEscalationMetrics::default();
    metrics.record_candidate(&candidate(
        "proved",
        "z3_unknown_complex_fragment",
        &["nonlinear_arithmetic"],
        Some("lean_verified"),
        None,
    ));
    metrics.record_candidate(&candidate(
        "partial",
        "z3_unknown_complex_fragment",
        &["nonlinear_arithmetic"],
        Some("partial_translation"),
        None,
    ));
    metrics.record_candidate(&candidate(
        "manual",
        "trusted_atom_human_review",
        &["inductive_data_type"],
        None,
        Some("manual lemma needed"),
    ));

    assert_eq!(metrics.escalation_attempts, 3);
    assert_eq!(metrics.lean_successes, 1);
    assert_eq!(
        metrics
            .successes_by_failure_reason
            .get("z3_unknown_complex_fragment"),
        Some(&1)
    );
    assert_eq!(metrics.partial_translation, 1);
    assert_eq!(metrics.manual_required, 1);
    assert_eq!(
        metrics.by_failure_reason.get("z3_unknown_complex_fragment"),
        Some(&2)
    );
    assert_eq!(
        metrics.by_logic_fragment.get("nonlinear_arithmetic"),
        Some(&2)
    );
    assert_eq!(
        metrics.by_logic_fragment.get("inductive_data_type"),
        Some(&1)
    );
}

#[test]
fn test_low_success_category_identification() {
    let mut metrics = LeanEscalationMetrics::default();
    for index in 0..3 {
        metrics.record_candidate(&candidate(
            &format!("unknown_{index}"),
            "z3_unknown_complex_fragment",
            &["nonlinear_arithmetic"],
            if index == 0 {
                Some("lean_verified")
            } else {
                Some("partial_translation")
            },
            None,
        ));
    }
    for index in 0..2 {
        metrics.record_candidate(&candidate(
            &format!("trusted_{index}"),
            "trusted_atom_human_review",
            &["trusted_atom"],
            Some("lean_verified"),
            None,
        ));
    }

    assert_eq!(
        metrics.identify_low_success_categories(),
        vec!["z3_unknown_complex_fragment".to_string()]
    );
}

#[test]
fn test_metrics_json_output() {
    let mut metrics = LeanEscalationMetrics::default();
    metrics.record_candidate(&candidate(
        "proved",
        "z3_unknown_complex_fragment",
        &["nonlinear_arithmetic"],
        Some("lean_verified"),
        None,
    ));

    let json = metrics.to_summary_json();

    assert_eq!(json["escalation_attempts"], 1);
    assert_eq!(json["lean_successes"], 1);
    assert_eq!(json["partial_translation"], 0);
    assert_eq!(json["manual_required"], 0);
    assert_eq!(json["success_rate"], 1.0);
    assert_eq!(json["by_failure_reason"]["z3_unknown_complex_fragment"], 1);
    assert_eq!(
        json["successes_by_failure_reason"]["z3_unknown_complex_fragment"],
        1
    );
    assert_eq!(json["by_logic_fragment"]["nonlinear_arithmetic"], 1);
    assert_eq!(json["low_success_categories"].as_array().unwrap().len(), 0);
}

#[test]
fn test_lean_verified_acceptance_tracks_success_by_reason() {
    let mut metrics = LeanEscalationMetrics::default();
    let atom = lean_verified_atom(
        "accepted",
        Some("z3_unknown_complex_fragment"),
        &["nonlinear_arithmetic"],
    );

    metrics.record_atom_certificate(&atom);
    metrics.record_lean_verified_acceptance(&atom);

    assert_eq!(metrics.escalation_attempts, 1);
    assert_eq!(metrics.lean_successes, 1);
    assert_eq!(
        metrics
            .successes_by_failure_reason
            .get("z3_unknown_complex_fragment"),
        Some(&1)
    );
    assert_eq!(metrics.to_summary_json()["success_rate"], 1.0);
}

#[test]
fn test_lean_verified_acceptance_without_escalation_reason_has_fallback_bucket() {
    let mut metrics = LeanEscalationMetrics::default();
    let atom = lean_verified_atom("accepted_import", None, &["quantifier_alternation"]);

    metrics.record_lean_verified_acceptance(&atom);

    assert_eq!(metrics.escalation_attempts, 1);
    assert_eq!(metrics.lean_successes, 1);
    assert_eq!(
        metrics
            .by_failure_reason
            .get("lean_verified_import_acceptance"),
        Some(&1)
    );
    assert_eq!(
        metrics
            .successes_by_failure_reason
            .get("lean_verified_import_acceptance"),
        Some(&1)
    );
}

#[test]
fn test_no_emit_escalation_metrics_suppresses_metrics_file() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let output_dir =
        std::env::temp_dir().join(format!("mumei_escalation_metrics_{}", std::process::id()));
    if output_dir.exists() {
        std::fs::remove_dir_all(&output_dir).expect("clean stale metrics output dir");
    }
    std::fs::create_dir_all(&output_dir).expect("create metrics output dir");
    let cert_path = output_dir.join("fixture.proof.json");
    let metrics_path = cert_path.with_extension("escalation-metrics.json");

    let output = Command::new(bin)
        .arg("verify")
        .arg("--emit")
        .arg("escalation-metrics")
        .arg("--no-emit")
        .arg("escalation-metrics")
        .arg("--output")
        .arg(&cert_path)
        .arg("tests/test_cross_spec.mm")
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify: {err}"));

    assert!(
        output.status.success(),
        "verify should accept --no-emit escalation-metrics\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!metrics_path.exists());

    std::fs::remove_dir_all(output_dir).expect("remove metrics output dir");
}
