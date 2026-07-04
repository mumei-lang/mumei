use crate::pipeline::read_source_file;
use mumei_core::{
    parser, proof_cert,
    reconstruction_loss::ReconstructionLoss,
    structured_feedback::{Location, StructuredFeedback},
    verification,
};
use std::path::Path;

pub(crate) fn resolve_source_for_span(main_source: &str, span: &parser::Span) -> String {
    if span.file.is_empty() {
        main_source.to_string()
    } else {
        read_source_file(&span.file).unwrap_or_else(|_| main_source.to_string())
    }
}

pub(crate) fn structured_feedback_for_passed_atom(atom: &parser::Atom) -> StructuredFeedback {
    StructuredFeedback {
        location: Location::from_span(&atom.span),
        ..StructuredFeedback::verification_passed()
    }
}

pub(crate) fn structured_feedback_from_report_file(
    output_dir: &Path,
    atom: &parser::Atom,
    error_text: Option<&str>,
) -> StructuredFeedback {
    let report_path = output_dir.join("report.json");
    if let Ok(report_json) = std::fs::read_to_string(&report_path) {
        if let Ok(report) = serde_json::from_str::<serde_json::Value>(&report_json) {
            if report
                .get("atom")
                .and_then(serde_json::Value::as_str)
                .map(|name| name == atom.name)
                .unwrap_or(true)
            {
                return StructuredFeedback::from_report(&report);
            }
        }
    }

    let error_type = error_text.and_then(infer_failure_type_from_error_text);
    StructuredFeedback::verification_failed(
        error_type.map(str::to_string),
        Location::from_span(&atom.span),
        None,
        None,
    )
}

pub(crate) fn infer_failure_type_from_error_text(error_text: &str) -> Option<&'static str> {
    let lower = error_text.to_lowercase();
    if lower.contains("postcondition") || lower.contains("ensures") {
        Some(verification::FAILURE_POSTCONDITION_VIOLATED)
    } else if lower.contains("precondition") || lower.contains("requires") {
        Some(verification::FAILURE_PRECONDITION_VIOLATED)
    } else if lower.contains("linearity") || lower.contains("moved") {
        Some(verification::FAILURE_LINEARITY_VIOLATED)
    } else if lower.contains("contradiction") || lower.contains("invariant") {
        Some(verification::FAILURE_INVARIANT_VIOLATED)
    } else if lower.contains("effect") {
        Some(verification::FAILURE_EFFECT_NOT_ALLOWED)
    } else {
        None
    }
}

pub(crate) fn structured_feedback_payload(
    structured_feedbacks: &[StructuredFeedback],
    warnings: &[verification::Diagnostic],
    verified: usize,
    failed: usize,
    skipped: usize,
    escalated: usize,
) -> serde_json::Value {
    if structured_feedbacks.len() == 1 {
        let mut payload = serde_json::json!(structured_feedbacks[0]);
        if !warnings.is_empty() {
            payload["warnings"] = serde_json::json!(warnings);
        }
        return payload;
    }
    serde_json::json!({
        "structured_feedback": structured_feedbacks,
        "warnings": warnings,
        "summary": {
            "verified": verified,
            "failed": failed,
            "skipped": skipped,
            "escalation_candidates": escalated,
        }
    })
}

pub(crate) fn collect_decidable_fragment_diagnostic(
    atom: &parser::Atom,
    module_env: &verification::ModuleEnv,
    suppress_output: bool,
) -> Option<verification::Diagnostic> {
    let diagnostic = verification::outside_decidable_fragment_diagnostic(atom, module_env)?;
    if !suppress_output {
        let location = if atom.span.file.is_empty() {
            format!("<unknown>:{}", atom.span.line)
        } else {
            format!("{}:{}", atom.span.file, atom.span.line)
        };
        eprintln!("warning[{}]: {}", diagnostic.code, diagnostic.message);
        eprintln!("  --> {}", location);
        eprintln!(
            "  hint: simplify to linear arithmetic, or use `mumei verify --escalate-lean` to delegate to Lean 4"
        );
        eprintln!("  see: docs/SPEC_GUIDE.md#decidable-fragment");
    }
    Some(diagnostic)
}

pub(crate) fn collect_untyped_array_access_diagnostic(
    atom: &parser::Atom,
    strict: bool,
    suppress_output: bool,
) -> Option<verification::Diagnostic> {
    let diagnostic = verification::untyped_array_access_diagnostic(atom, strict)?;
    if !suppress_output {
        let location = if atom.span.file.is_empty() {
            format!("<unknown>:{}", atom.span.line)
        } else {
            format!("{}:{}", atom.span.file, atom.span.line)
        };
        eprintln!(
            "{}[{}]: {}",
            diagnostic.severity, diagnostic.code, diagnostic.message
        );
        eprintln!("  --> {}", location);
        eprintln!(
            "  hint: annotate the parameter as `[i64]`, `[f64]`, or `[bool]` to select the element sort"
        );
    }
    Some(diagnostic)
}

pub(crate) fn emit_decidable_fragment_warning(
    atom: &parser::Atom,
    module_env: &verification::ModuleEnv,
    suppress_output: bool,
) {
    let _ = collect_decidable_fragment_diagnostic(atom, module_env, suppress_output);
}

pub(crate) fn enrich_verify_json_payload(
    mut payload: serde_json::Value,
    diagnostics: &[verification::Diagnostic],
    loop_suggestions: &[serde_json::Value],
) -> serde_json::Value {
    if let Some(object) = payload.as_object_mut() {
        object.insert("diagnostics".to_string(), serde_json::json!(diagnostics));
        object.insert("warnings".to_string(), serde_json::json!(diagnostics));
        if !loop_suggestions.is_empty() {
            object.insert(
                "cegis_suggestions".to_string(),
                serde_json::Value::Array(loop_suggestions.to_vec()),
            );
        }
    }
    payload
}

pub(crate) fn emit_spurious_counterexample_diagnostic(
    validation: &verification::CounterexampleValidationResult,
    atom_name: &str,
    suppress_output: bool,
) {
    if suppress_output {
        return;
    }
    match validation.validation_status.as_str() {
        "validated" => eprintln!("  ✓ Counterexample validated for atom '{}'", atom_name),
        "spurious_candidate" => {
            eprintln!(
                "  ⚠️  Spurious counterexample detected (candidate) for atom '{}'",
                atom_name
            );
            let symbols = validation
                .symbol_provenance
                .iter()
                .map(|symbol| format!("{} ({})", symbol.symbol_name, symbol.source))
                .collect::<Vec<_>>()
                .join(", ");
            eprintln!("    Depends on uninterpreted symbols: {symbols}");
        }
        "unvalidated" => eprintln!("  ? Counterexample unvalidated for atom '{}'", atom_name),
        _ => {}
    }
}

pub(crate) fn counterexample_model_from_error(
    error: &verification::MumeiError,
) -> std::collections::HashMap<String, verification::CexValue> {
    let mut model = std::collections::HashMap::new();
    if let verification::MumeiError::VerificationError {
        counterexample: Some(serde_json::Value::Object(values)),
        ..
    } = error
    {
        for (name, value) in values {
            let parsed = match value {
                serde_json::Value::Bool(flag) => Some(verification::CexValue::Bool(*flag)),
                serde_json::Value::Number(number) => number
                    .as_i64()
                    .map(verification::CexValue::Int)
                    .or_else(|| number.as_f64().map(verification::CexValue::Float)),
                serde_json::Value::String(text) => text
                    .parse::<i64>()
                    .ok()
                    .map(verification::CexValue::Int)
                    .or_else(|| {
                        verification::parse_z3_numeric_to_f64(text)
                            .map(verification::CexValue::Float)
                    }),
                _ => None,
            };
            if let Some(value) = parsed {
                model.insert(name.clone(), value);
            }
        }
    }
    model
}

pub(crate) fn reconstruction_loss_from_error(
    error: &verification::MumeiError,
    violated_property: &str,
) -> Option<ReconstructionLoss> {
    if let verification::MumeiError::VerificationError {
        counterexample: Some(counterexample),
        ..
    } = error
    {
        ReconstructionLoss::from_counterexample_value(violated_property.to_string(), counterexample)
    } else {
        None
    }
}

pub(crate) fn parse_artifact_paths(value: &str) -> Option<Vec<String>> {
    let paths: Vec<String> = value
        .split(',')
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect();
    if paths.is_empty() {
        None
    } else {
        Some(paths)
    }
}

pub(crate) fn resolve_harness_contract(cli_value: Option<String>) -> Option<String> {
    cli_value.or_else(proof_cert::harness_contract_from_env)
}

pub(crate) fn resolve_artifact_paths(cli_value: Option<String>) -> Option<Vec<String>> {
    cli_value
        .as_deref()
        .and_then(parse_artifact_paths)
        .or_else(proof_cert::artifact_paths_from_env)
}

pub(crate) fn parse_intent_fidelity_json(
    value: &str,
) -> Result<proof_cert::IntentFidelity, String> {
    serde_json::from_str(value).map_err(|err| format!("invalid --intent-fidelity JSON: {err}"))
}

pub(crate) fn resolve_intent_fidelity(
    cli_value: Option<String>,
) -> Option<proof_cert::IntentFidelity> {
    if let Some(value) = cli_value {
        match parse_intent_fidelity_json(&value) {
            Ok(intent_fidelity) => Some(intent_fidelity),
            Err(message) => {
                eprintln!("  ❌ {message}");
                std::process::exit(1);
            }
        }
    } else {
        proof_cert::intent_fidelity_from_env()
    }
}

pub(crate) fn resolve_budget_policy_fingerprint(cli_value: Option<String>) -> Option<String> {
    cli_value.or_else(proof_cert::budget_policy_fingerprint_from_env)
}
