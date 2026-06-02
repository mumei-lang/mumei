use crate::parser::Span;
use crate::reconstruction_loss::ReconstructionLoss;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const STATUS_VERIFICATION_FAILED: &str = "verification_failed";
pub const STATUS_VERIFICATION_PASSED: &str = "verification_passed";

/// Source location for AI-consumable structured feedback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub file: String,
    pub line: usize,
}

impl Location {
    pub fn from_span(span: &Span) -> Option<Self> {
        if span.file.is_empty() && span.line == 0 {
            return None;
        }
        Some(Self {
            file: span.file.clone(),
            line: span.line,
        })
    }

    pub fn from_report(report: &Value) -> Option<Self> {
        let span = report.get("span")?.as_object()?;
        Some(Self {
            file: span
                .get("file")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            line: span.get("line").and_then(Value::as_u64).unwrap_or_default() as usize,
        })
    }
}

/// P9-E Loss Vector structured feedback emitted by verification.
///
/// JSON schema:
/// - `status`: `"verification_failed"` or `"verification_passed"`
/// - `error_type`: nullable verifier failure type
/// - `location`: nullable `{ "file": string, "line": number }`
/// - `reconstruction_loss`: nullable [`ReconstructionLoss`]
/// - `feedback_instruction`: actionable repair instruction for AI agents
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuredFeedback {
    pub status: String,
    pub error_type: Option<String>,
    pub location: Option<Location>,
    pub reconstruction_loss: Option<ReconstructionLoss>,
    pub feedback_instruction: String,
}

impl StructuredFeedback {
    pub fn verification_passed() -> Self {
        Self {
            status: STATUS_VERIFICATION_PASSED.to_string(),
            error_type: None,
            location: None,
            reconstruction_loss: None,
            feedback_instruction: "Verification passed; no fix is required.".to_string(),
        }
    }

    pub fn verification_failed(
        error_type: Option<String>,
        location: Option<Location>,
        reconstruction_loss: Option<ReconstructionLoss>,
        report: Option<&Value>,
    ) -> Self {
        let feedback_instruction = report
            .map(format_actionable_fix_hint)
            .filter(|hint| !hint.is_empty())
            .or_else(|| {
                error_type
                    .as_deref()
                    .map(|failure_type| feedback_instruction_for_error_type(failure_type, None))
            })
            .unwrap_or_else(|| {
                "Verification failed. Review the verifier report and repair the atom.".to_string()
            });

        Self {
            status: STATUS_VERIFICATION_FAILED.to_string(),
            error_type,
            location,
            reconstruction_loss,
            feedback_instruction,
        }
    }

    pub fn from_report(report: &Value) -> Self {
        let raw_status = report
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let passed = matches!(
            raw_status,
            "success" | "passed" | "verified" | "trusted" | "unverified"
        );
        if passed {
            return Self {
                location: Location::from_report(report),
                ..Self::verification_passed()
            };
        }

        let error_type = report
            .get("failure_type")
            .and_then(Value::as_str)
            .or_else(|| {
                report
                    .get("semantic_feedback")
                    .and_then(|feedback| feedback.get("failure_type"))
                    .and_then(Value::as_str)
            })
            .or_else(|| {
                let violation_type = report.get("violation_type").and_then(Value::as_str)?;
                if violation_type.starts_with("effect_") {
                    Some("effect_not_allowed")
                } else {
                    Some(violation_type)
                }
            })
            .map(str::to_string);
        let reconstruction_loss = report
            .get("semantic_feedback")
            .and_then(|feedback| feedback.get("reconstruction_loss"))
            .or_else(|| report.get("reconstruction_loss"))
            .and_then(|value| serde_json::from_value(value.clone()).ok());

        Self::verification_failed(
            error_type,
            Location::from_report(report),
            reconstruction_loss,
            Some(report),
        )
    }

    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    pub fn from_json(json: &str) -> serde_json::Result<Self> {
        serde_json::from_str(json)
    }

    pub fn json_schema() -> Value {
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "title": "MumeiStructuredFeedback",
            "description": "P9-E Loss Vector structured verification feedback for AI agents.",
            "type": "object",
            "required": [
                "status",
                "error_type",
                "location",
                "reconstruction_loss",
                "feedback_instruction"
            ],
            "properties": {
                "status": {
                    "type": "string",
                    "enum": [STATUS_VERIFICATION_FAILED, STATUS_VERIFICATION_PASSED]
                },
                "error_type": {
                    "type": ["string", "null"],
                    "description": "Verifier violation/failure type such as postcondition_violated or division_by_zero."
                },
                "location": {
                    "type": ["object", "null"],
                    "required": ["file", "line"],
                    "properties": {
                        "file": {"type": "string"},
                        "line": {"type": "integer", "minimum": 0}
                    }
                },
                "reconstruction_loss": {
                    "type": ["object", "null"],
                    "required": ["violated_property", "counter_example", "loss_vector"],
                    "properties": {
                        "violated_property": {"type": "string"},
                        "counter_example": {"type": "object"},
                        "loss_vector": {
                            "type": "array",
                            "items": {"type": "number"}
                        }
                    }
                },
                "feedback_instruction": {
                    "type": "string",
                    "description": "Human-readable, actionable instruction optimized for AI repair loops."
                }
            }
        })
    }
}

pub fn feedback_instruction_for_error_type(error_type: &str, report: Option<&Value>) -> String {
    match error_type {
        "division_by_zero" => {
            let divisor = report
                .and_then(|value| value.get("semantic_feedback"))
                .and_then(|feedback| feedback.get("counter_example"))
                .and_then(|counter_example| counter_example.get("divisor"))
                .and_then(Value::as_str)
                .unwrap_or("the divisor");
            format!(
                "The divisor `{divisor}` can be zero. Add `requires: <divisor_param> != 0` to the atom's precondition."
            )
        }
        "linearity_violated" => {
            "A linear resource is used more than once. Clone before the second use or restructure the code so each linear value is consumed exactly once.".to_string()
        }
        "invariant_violated" => {
            let conflicts = report
                .and_then(|value| value.get("semantic_feedback"))
                .and_then(|feedback| feedback.get("conflicting_constraints"))
                .and_then(Value::as_array)
                .map(|constraints| {
                    constraints
                        .iter()
                        .filter_map(Value::as_str)
                        .take(4)
                        .map(|constraint| format!("`{constraint}`"))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if conflicts.is_empty() {
                "The constraints are contradictory. Relax one or more conflicting constraints.".to_string()
            } else {
                format!(
                    "The constraints {} are contradictory. Relax one or more constraints so they can be simultaneously satisfied.",
                    conflicts.join(", ")
                )
            }
        }
        "postcondition_violated" => {
            let counterexample = report
                .and_then(|value| value.get("counterexample"))
                .and_then(Value::as_object);
            if let Some(counterexample) = counterexample {
                let values = counterexample
                    .iter()
                    .map(|(key, value)| format!("{key}={value}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "The `ensures` clause is not satisfied for inputs: {values}. Fix the body to satisfy `ensures`, or adjust `ensures` to match actual behaviour."
                )
            } else {
                "The `ensures` clause is not satisfied by the function body's return value. Fix the body or adjust `ensures`.".to_string()
            }
        }
        "precondition_violated" => {
            "A `requires` clause is not satisfied. Strengthen the caller's arguments or relax the precondition only if the contract is too strict.".to_string()
        }
        "temporal_effect_violated" => {
            "The effect state transitions are in the wrong order. Reorder `perform` calls to follow the declared state machine.".to_string()
        }
        "effect_not_allowed" => {
            let attempted = report
                .and_then(|value| value.get("semantic_feedback"))
                .and_then(|feedback| feedback.get("attempted_effect"))
                .and_then(Value::as_str)
                .unwrap_or("the required effect");
            format!(
                "Effect `{attempted}` is used in the body but not declared or propagated. Add the effect to the caller's effects list or remove the operation."
            )
        }
        "trait_law_violated" => {
            "A trait law does not hold for this implementation. Update the method body or trait law so the implementation satisfies the declared algebraic law.".to_string()
        }
        "exhaustiveness_failed" => {
            "A match or enum analysis is non-exhaustive. Add missing cases so every constructor/path is covered.".to_string()
        }
        "resource_conflict" => {
            "Resource acquisition can conflict or deadlock. Reorder acquisitions according to the resource hierarchy or split the critical section.".to_string()
        }
        _ => report
            .and_then(|value| value.get("suggestion"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| "Verification failed. Review the verifier report and repair the atom.".to_string()),
    }
}

pub fn format_actionable_fix_hint(report: &Value) -> String {
    let failure_type = report
        .get("failure_type")
        .and_then(Value::as_str)
        .or_else(|| {
            report
                .get("semantic_feedback")
                .and_then(|feedback| feedback.get("failure_type"))
                .and_then(Value::as_str)
        })
        .unwrap_or_default();
    let violation_type = report
        .get("violation_type")
        .and_then(Value::as_str)
        .unwrap_or_default();

    let mut instruction = if failure_type.is_empty() && violation_type == "effect_mismatch" {
        effect_mismatch_instruction(report)
    } else if failure_type.is_empty() && violation_type == "effect_propagation" {
        effect_propagation_instruction(report)
    } else {
        feedback_instruction_for_error_type(failure_type, Some(report))
    };

    if let Some(suggestion) = report.get("suggestion").and_then(Value::as_str) {
        if is_contextual_suggestion(suggestion) && !instruction.contains(suggestion) {
            instruction.push_str("\nVerifier suggestion (contextual): ");
            instruction.push_str(suggestion);
        } else if instruction.starts_with("Verification failed.") {
            instruction = format!("Verifier suggestion: {suggestion}");
        }
    }

    instruction
}

fn effect_mismatch_instruction(report: &Value) -> String {
    let effect_violation = report.get("effect_violation");
    let required = effect_violation
        .and_then(|violation| violation.get("required_effect"))
        .and_then(Value::as_str)
        .unwrap_or("the required effect");
    format!(
        "Effect `{required}` is used in the body but not declared in the effects list. Add `{required}` to the effects list."
    )
}

fn effect_propagation_instruction(report: &Value) -> String {
    let effect_violation = report.get("effect_violation");
    let caller = effect_violation
        .and_then(|violation| violation.get("caller"))
        .and_then(Value::as_str)
        .unwrap_or("the caller");
    let callee = effect_violation
        .and_then(|violation| violation.get("callee"))
        .and_then(Value::as_str)
        .unwrap_or("the callee");
    let missing = effect_violation
        .and_then(|violation| violation.get("missing_effects"))
        .cloned()
        .unwrap_or(Value::Array(Vec::new()));
    format!(
        "Caller `{caller}` calls `{callee}` but does not propagate required effects {missing}. Add the missing effects to `{caller}`'s effects list."
    )
}

fn is_contextual_suggestion(suggestion: &str) -> bool {
    const MARKERS: &[&str] = &[
        "counterexample",
        "counter-example",
        "e.g.",
        "for example",
        "value",
        "when ",
        "because ",
        "specific",
        " = ",
    ];
    let lower = suggestion.to_lowercase();
    MARKERS.iter().any(|marker| lower.contains(marker))
}
