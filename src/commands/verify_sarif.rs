use super::verify::VerifyOutcome;
use mumei_core::parser::{Atom, ClauseKind, ClauseTrustMode, Span};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::Path;

pub(crate) struct AtomFindings<'a> {
    pub(crate) atom: &'a Atom,
    pub(crate) name: &'a str,
    pub(crate) report: Option<&'a Value>,
    pub(crate) cert_result: Option<&'a (String, String)>,
    pub(crate) failure_diagnostics: &'a [mumei_core::verification::Diagnostic],
    pub(crate) diagnostics: &'a [mumei_core::verification::Diagnostic],
    pub(crate) imported_contract_trusted: bool,
}

pub(crate) struct SarifCollector {
    pub(crate) results: Vec<Value>,
    pub(crate) notifications: Vec<Value>,
    pub(crate) prior_outcome: VerifyOutcome,
    pub(crate) written: bool,
}

impl SarifCollector {
    pub(crate) fn new() -> Self {
        Self {
            results: Vec::new(),
            notifications: Vec::new(),
            prior_outcome: VerifyOutcome::Verified,
            written: false,
        }
    }

    pub(crate) fn collect_atom(&mut self, findings: AtomFindings<'_>) {
        collect_atom(&mut self.results, findings);
    }

    pub(crate) fn push_axiom_rejected(
        &mut self,
        atom_name: &str,
        disallowed: Option<&[String]>,
        span: Option<&Span>,
    ) {
        let message = match disallowed {
            Some(axioms) => format!(
                "Lean proof for atom `{atom_name}` was rejected because it uses disallowed axioms: {}",
                axioms.join(", ")
            ),
            None => format!(
                "Lean proof for atom `{atom_name}` was rejected because the kernel axiom audit errored"
            ),
        };
        let mut properties = base_properties(atom_name, "lean_proof");
        properties.insert("failure_type".to_string(), json!("axiom_rejected"));
        properties.insert("outcome".to_string(), json!("axiom_rejected"));
        if let Some(axioms) = disallowed {
            properties.insert("disallowed_axioms".to_string(), json!(axioms));
        }
        self.results.push(result(
            "axiom_rejected",
            "error",
            "fail",
            message,
            span,
            properties,
        ));
    }

    pub(crate) fn push_panic_notification(&mut self, file: &str) {
        self.notifications.push(json!({
            "level": "error",
            "message": {"text": format!("internal error (panic) while verifying {file}")}
        }));
    }

    pub(crate) fn remove_lean_verified(&mut self, start: usize, atom_names: &[String]) {
        let mut index = 0;
        self.results.retain(|result| {
            let keep = index < start
                || !result["properties"]["atom"]
                    .as_str()
                    .is_some_and(|name| atom_names.iter().any(|verified| verified == name));
            index += 1;
            keep
        });
    }

    pub(crate) fn document(&self, outcome: VerifyOutcome) -> Value {
        let exit_code = self.prior_outcome.combine(outcome).exit_code();
        let rules = collect_rules(&self.results);
        let rule_indices: HashMap<&str, usize> = rules
            .iter()
            .enumerate()
            .filter_map(|(index, rule)| rule["id"].as_str().map(|id| (id, index)))
            .collect();
        let results = self
            .results
            .iter()
            .map(|result| {
                let mut result = result.clone();
                if let Some(rule_id) = result["ruleId"].as_str() {
                    if let Some(index) = rule_indices.get(rule_id) {
                        result["ruleIndex"] = json!(index);
                    }
                }
                result
            })
            .collect::<Vec<_>>();
        json!({
            "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
            "version": "2.1.0",
            "runs": [{
                "tool": {
                    "driver": {
                        "name": "mumei",
                        "version": env!("CARGO_PKG_VERSION"),
                        "informationUri": "https://github.com/mumei-lang/mumei",
                        "rules": rules,
                    }
                },
                "invocations": [{
                    "exitCode": exit_code,
                    "executionSuccessful": matches!(exit_code, 0 | 1 | 3),
                    "toolExecutionNotifications": self.notifications.clone(),
                }],
                "results": results,
            }]
        })
    }
}

fn collect_atom(results: &mut Vec<Value>, findings: AtomFindings<'_>) {
    let AtomFindings {
        atom,
        name,
        report,
        cert_result,
        failure_diagnostics,
        diagnostics,
        imported_contract_trusted,
    } = findings;
    let report_span = report.and_then(span_from_report);
    let span = report_span.as_ref().unwrap_or(&atom.span);
    let matching_failure_diagnostics = failure_diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.atom == name)
        .collect::<Vec<_>>();
    let matching_error_diagnostics = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.atom == name && diagnostic.severity == "error")
        .collect::<Vec<_>>();

    let mut emitted_skipped_outcome = false;
    let report_is_current = report.is_some_and(|report| report["atom"].as_str() == Some(name));
    if report_is_current {
        let report = report.expect("checked report");
        let status = report["status"].as_str().unwrap_or_default();
        if status == "failed" {
            let rule_id = nonempty(report["failure_type"].as_str()).unwrap_or("failed");
            let failed_clause = report["failed_clause"].as_str();
            let label = report["failed_clause_label"].as_str();
            let mut properties = base_properties(
                name,
                if failed_clause.is_some() {
                    "ensures"
                } else {
                    "atom"
                },
            );
            properties.insert("failure_type".to_string(), json!(rule_id));
            properties.insert("outcome".to_string(), json!(status));
            if let Some(clause) = failed_clause {
                properties.insert("clause".to_string(), json!(clause));
                if let Some(label) = label {
                    properties.insert("label".to_string(), json!(label));
                }
                if let Some(outcome) = report["ensures_outcomes"]
                    .as_array()
                    .and_then(|entries| {
                        entries.iter().find(|entry| {
                            entry["clause"].as_str() == Some(clause)
                                && matches!(
                                    entry["outcome"].as_str(),
                                    Some("always_false" | "fails_on_some_inputs" | "fails")
                                )
                        })
                    })
                    .and_then(|entry| entry["outcome"].as_str())
                {
                    properties.insert("outcome".to_string(), json!(outcome));
                }
                append_clause_label(&mut properties, label);
            }
            for field in ["counterexample", "counterexample_fidelity"] {
                if let Some(value) = report.get(field) {
                    properties.insert(field.to_string(), value.clone());
                }
            }
            if let Some(z3_result) = report["z3_result"].as_str() {
                properties.insert("z3_result".to_string(), json!(z3_result));
            }
            let description = report["reason"].as_str().unwrap_or("Verification failed");
            let message = format_failure_message(description, name, label, failed_clause);
            results.push(result(
                rule_id,
                "error",
                "fail",
                message,
                Some(span),
                properties,
            ));
        }

        if let Some(entries) = report["ensures_outcomes"].as_array() {
            for entry in entries {
                let Some(outcome) = entry["outcome"].as_str() else {
                    continue;
                };
                if !matches!(outcome, "vacuous" | "unknown" | "skipped") {
                    continue;
                }
                if entry["clause"].as_str() == report["failed_clause"].as_str() {
                    continue;
                }
                let Some(clause) = entry["clause"].as_str() else {
                    continue;
                };
                if outcome == "skipped" {
                    emitted_skipped_outcome = true;
                }
                let label = mumei_core::verification::clause_label_for_atom(
                    atom,
                    ClauseKind::Ensures,
                    clause,
                );
                let mut properties = base_properties(name, "ensures");
                properties.insert("clause".to_string(), json!(clause));
                properties.insert("outcome".to_string(), json!(outcome));
                if let Some(label) = label {
                    properties.insert("label".to_string(), json!(label));
                }
                if let Some(context) = report.get("context_reachability") {
                    properties.insert("context_reachability".to_string(), context.clone());
                }
                let label_clause = format_labeled_clause("ensures", label, clause);
                results.push(result(
                    outcome,
                    "warning",
                    "review",
                    format!("{label_clause} is {outcome} in atom `{name}`; it was not proved"),
                    Some(span),
                    properties,
                ));
            }
        }

        if status == "unverifiable" && !emitted_skipped_outcome {
            let mut properties = base_properties(name, "atom");
            properties.insert("outcome".to_string(), json!(status));
            results.push(result(
                "unverifiable",
                "warning",
                "review",
                format!("Atom `{name}` is unverifiable"),
                Some(span),
                properties,
            ));
        }

        if let Some(cover_results) = report["cover_results"].as_array() {
            for cover in cover_results {
                let Some(status) = cover["status"].as_str() else {
                    continue;
                };
                let Some(clause) = cover["clause"].as_str() else {
                    continue;
                };
                let mut properties = base_properties(name, "cover");
                properties.insert("clause".to_string(), json!(clause));
                properties.insert("outcome".to_string(), json!(status));
                if let Some(label) = cover["label"].as_str() {
                    properties.insert("label".to_string(), json!(label));
                }
                if status == "covered" {
                    if let Some(witness) = cover.get("witness") {
                        properties.insert("witness".to_string(), witness.clone());
                    }
                    results.push(result(
                        "covered",
                        "none",
                        "pass",
                        format!(
                            "Cover clause {} has a witness in atom `{name}`",
                            format_labeled_clause("cover", cover["label"].as_str(), clause)
                        ),
                        Some(span),
                        properties,
                    ));
                } else if status == "unknown" {
                    results.push(result(
                        "unknown",
                        "warning",
                        "review",
                        format!(
                            "Cover clause {} could not be decided in atom `{name}`",
                            format_labeled_clause("cover", cover["label"].as_str(), clause)
                        ),
                        Some(span),
                        properties,
                    ));
                }
            }
        }
    }

    if !imported_contract_trusted {
        for mode in atom
            .clause_modes
            .iter()
            .filter(|mode| mode.mode == ClauseTrustMode::Assume)
        {
            let obligation = match &mode.kind {
                ClauseKind::Requires => "requires",
                ClauseKind::Ensures => "ensures",
            };
            let label = mumei_core::verification::clause_label_for_atom(
                atom,
                mode.kind.clone(),
                &mode.clause,
            );
            let mut properties = base_properties(name, obligation);
            properties.insert("clause".to_string(), json!(mode.clause));
            properties.insert("outcome".to_string(), json!("assumed"));
            if let Some(label) = label {
                properties.insert("label".to_string(), json!(label));
            }
            results.push(result(
                "assumed_clause",
                "note",
                "review",
                format!(
                    "{} is trusted, not proved, in atom `{name}`",
                    format_labeled_clause(obligation, label, &mode.clause)
                ),
                Some(span),
                properties,
            ));
        }
    }

    if !report_is_current {
        let cert_z3_result = cert_result.map(|(z3_result, _)| z3_result.as_str());
        let cert_status = cert_result.map(|(_, status)| status.as_str());
        if cert_status.is_some_and(|status| status != "verified")
            || !matching_failure_diagnostics.is_empty()
            || !matching_error_diagnostics.is_empty()
        {
            if let Some(diagnostic) = matching_error_diagnostics.first() {
                let rule_id = if diagnostic.code.is_empty() {
                    "failed"
                } else {
                    diagnostic.code.as_str()
                };
                let mut properties = base_properties(name, "atom");
                properties.insert("failure_type".to_string(), json!(rule_id));
                properties.insert(
                    "outcome".to_string(),
                    json!(cert_status.unwrap_or("failed")),
                );
                results.push(result(
                    rule_id,
                    "error",
                    "fail",
                    format!("{} (atom `{name}`)", first_line(&diagnostic.message)),
                    Some(span),
                    properties,
                ));
            } else {
                let z3_result = cert_z3_result.unwrap_or_default();
                let status = cert_status.unwrap_or_default();
                let rule_id = if !z3_result.is_empty() && !matches!(z3_result, "sat" | "skipped") {
                    z3_result
                } else if !status.is_empty() {
                    status
                } else {
                    "failed"
                };
                let warning = matches!(z3_result, "unknown" | "timeout" | "resource_limit")
                    || matches!(status, "unverifiable" | "escalation_candidate" | "unknown");
                let message = matching_failure_diagnostics
                    .first()
                    .map(|diagnostic| first_line(&diagnostic.message))
                    .unwrap_or("Verification did not prove this atom");
                let mut properties = base_properties(name, "atom");
                properties.insert(
                    "outcome".to_string(),
                    json!(if status.is_empty() { rule_id } else { status }),
                );
                if !z3_result.is_empty() {
                    properties.insert("z3_result".to_string(), json!(z3_result));
                }
                if !rule_id.is_empty() {
                    properties.insert("failure_type".to_string(), json!(rule_id));
                }
                results.push(result(
                    rule_id,
                    if warning { "warning" } else { "error" },
                    if warning { "review" } else { "fail" },
                    format!("{message} (atom `{name}`)"),
                    Some(span),
                    properties,
                ));
            }
        }
    }
}

fn base_properties(atom: &str, obligation: &str) -> Map<String, Value> {
    let mut properties = Map::new();
    properties.insert("atom".to_string(), json!(atom));
    properties.insert("obligation".to_string(), json!(obligation));
    properties
}

fn result(
    rule_id: &str,
    level: &str,
    kind: &str,
    message: String,
    span: Option<&Span>,
    properties: Map<String, Value>,
) -> Value {
    let mut value = json!({
        "ruleId": rule_id,
        "level": level,
        "kind": kind,
        "message": {"text": message},
        "properties": properties,
    });
    if let Some(span) = span {
        let mut physical_location = json!({
            "artifactLocation": {"uri": sarif_uri(&span.file)}
        });
        if span.line > 0 {
            let mut region = json!({"startLine": span.line});
            if span.col > 0 {
                region["startColumn"] = json!(span.col);
                if span.len > 0 {
                    region["endColumn"] = json!(span.col + span.len);
                }
            }
            physical_location["region"] = region;
        }
        value["locations"] = json!([{"physicalLocation": physical_location}]);
    }
    value
}

fn collect_rules(results: &[Value]) -> Vec<Value> {
    let mut rules = Vec::new();
    let mut seen = HashMap::new();
    for result in results {
        let Some(id) = result["ruleId"].as_str() else {
            continue;
        };
        if seen.contains_key(id) {
            continue;
        }
        seen.insert(id.to_string(), rules.len());
        rules.push(json!({
            "id": id,
            "shortDescription": {"text": rule_description(id)},
            "defaultConfiguration": {"level": result["level"]},
        }));
    }
    rules
}

fn rule_description(rule_id: &str) -> &'static str {
    match rule_id {
        "always_false" => "The postcondition is false for every input satisfying the precondition.",
        "fails_on_some_inputs" => "The postcondition fails for some inputs.",
        "fails" => "The postcondition could not be proved.",
        "postcondition_violated" => "A postcondition was violated.",
        "invariant_violated" => "An invariant was violated.",
        "cover_unreachable" => "The verification context for this cover clause is unreachable.",
        "vacuous" => "The verification context is unreachable, so the clause is vacuous.",
        "unknown" => "The solver could not decide the obligation.",
        "skipped" => "The obligation could not be lowered and was skipped.",
        "unverifiable" => "The atom contains an obligation the verifier cannot decide.",
        "covered" => "A witness satisfies the cover clause.",
        "assumed_clause" => "The clause is trusted rather than proved.",
        "axiom_rejected" => "Lean rejected the proof during kernel axiom audit.",
        "failed" => "The atom failed verification.",
        _ => "A verification finding reported by Mumei.",
    }
}

fn span_from_report(report: &Value) -> Option<Span> {
    let span = report.get("span")?;
    Some(Span {
        file: span["file"].as_str()?.to_string(),
        line: span["line"].as_u64().unwrap_or_default() as usize,
        col: span["col"].as_u64().unwrap_or_default() as usize,
        len: span["len"].as_u64().unwrap_or_default() as usize,
    })
}

fn format_failure_message(
    reason: &str,
    atom: &str,
    label: Option<&str>,
    clause: Option<&str>,
) -> String {
    match (label, clause) {
        (Some(label), Some(clause)) => {
            format!("ensures \"{label}\" `{clause}`: {reason} (atom `{atom}`)")
        }
        (None, Some(clause)) => format!("ensures `{clause}`: {reason} (atom `{atom}`)"),
        _ => format!("{reason} (atom `{atom}`)"),
    }
}

fn format_labeled_clause(kind: &str, label: Option<&str>, clause: &str) -> String {
    match label {
        Some(label) => format!("{kind} \"{label}\" `{clause}`"),
        None => format!("{kind} `{clause}`"),
    }
}

fn first_line(message: &str) -> &str {
    message.lines().next().unwrap_or(message)
}

fn append_clause_label(properties: &mut Map<String, Value>, label: Option<&str>) {
    if let Some(label) = label {
        properties.insert("label".to_string(), json!(label));
    }
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

fn sarif_uri(file: &str) -> String {
    let normalized = file.replace('\\', "/");
    let uri = if Path::new(file).is_absolute() || normalized.starts_with('/') {
        format!("file://{normalized}")
    } else {
        normalized
    };
    uri.replace(' ', "%20")
}

#[cfg(test)]
mod tests {
    use super::{
        collect_atom as collect_atom_results, sarif_uri, AtomFindings, SarifCollector,
        VerifyOutcome,
    };
    use mumei_core::parser;
    use mumei_core::verification::Diagnostic;
    use serde_json::{json, Value};

    fn atom(source: &str) -> parser::Atom {
        parser::parse_module(source)
            .into_iter()
            .find_map(|item| match item {
                parser::Item::Atom(atom) => Some(atom),
                _ => None,
            })
            .unwrap()
    }

    fn source_atom() -> parser::Atom {
        atom(
            r#"
atom f(x: i64) -> i64
requires: x >= 0;
ensures: result >= 0;
body: { x + 1 };
"#,
        )
    }

    fn message(result: &Value) -> &str {
        result["message"]["text"].as_str().unwrap()
    }

    fn collect_atom(
        results: &mut Vec<Value>,
        atom: &parser::Atom,
        name: &str,
        report: Option<&Value>,
    ) {
        collect_atom_results(
            results,
            AtomFindings {
                atom,
                name,
                report,
                cert_result: None,
                failure_diagnostics: &[],
                diagnostics: &[],
                imported_contract_trusted: false,
            },
        );
    }

    #[test]
    fn maps_failed_ensures_with_label_and_counterexample() {
        let atom = source_atom();
        let report = json!({
            "atom": "f",
            "status": "failed",
            "failure_type": "postcondition_violated",
            "reason": "Postcondition was false",
            "failed_clause": "result >= 0",
            "failed_clause_label": "non-negative",
            "ensures_outcomes": [{"clause": "result >= 0", "outcome": "always_false"}],
            "counterexample": {"x": "-1"},
            "counterexample_fidelity": "exact",
        });
        let mut results = Vec::new();
        collect_atom(&mut results, &atom, "f", Some(&report));
        assert_eq!(results[0]["ruleId"], "postcondition_violated");
        assert_eq!(results[0]["level"], "error");
        assert_eq!(results[0]["properties"]["obligation"], "ensures");
        assert_eq!(results[0]["properties"]["outcome"], "always_false");
        assert_eq!(results[0]["properties"]["counterexample"]["x"], "-1");
        assert_eq!(results[0]["properties"]["counterexample_fidelity"], "exact");
        assert!(message(&results[0]).contains("\"non-negative\""));
    }

    #[test]
    fn maps_non_proved_ensures_outcomes() {
        let atom = source_atom();
        let report = json!({
            "atom": "f",
            "status": "success",
            "context_reachability": "reachable",
            "ensures_outcomes": [
                {"clause": "result >= 0", "outcome": "vacuous"},
                {"clause": "result > 0", "outcome": "unknown"},
                {"clause": "result < 0", "outcome": "skipped"},
                {"clause": "result == x", "outcome": "proved"}
            ]
        });
        let mut results = Vec::new();
        collect_atom(&mut results, &atom, "f", Some(&report));
        assert_eq!(results.len(), 3);
        assert_eq!(results[0]["ruleId"], "vacuous");
        assert_eq!(results[1]["ruleId"], "unknown");
        assert_eq!(results[2]["ruleId"], "skipped");
        assert_eq!(results[0]["level"], "warning");
        assert_eq!(results[0]["properties"]["outcome"], "vacuous");
        assert_eq!(
            results[0]["properties"]["context_reachability"],
            "reachable"
        );
    }

    #[test]
    fn maps_unverifiable_without_skipped_outcomes() {
        let atom = source_atom();
        let report = json!({"atom":"f","status":"unverifiable","ensures_outcomes":[]});
        let mut results = Vec::new();
        collect_atom(&mut results, &atom, "f", Some(&report));
        assert_eq!(results[0]["ruleId"], "unverifiable");
        assert_eq!(results[0]["level"], "warning");
        assert_eq!(results[0]["properties"]["outcome"], "unverifiable");
    }

    #[test]
    fn maps_cover_unknown_and_pass_witness() {
        let atom = source_atom();
        let report = json!({
            "atom": "f",
            "status": "success",
            "cover_results": [
                {"clause":"result > 100","status":"unknown"},
                {"clause":"result == 1","label":"one","status":"covered","witness":{"x":"0","result":"1"}}
            ]
        });
        let mut results = Vec::new();
        collect_atom(&mut results, &atom, "f", Some(&report));
        assert_eq!(results[0]["ruleId"], "unknown");
        assert_eq!(results[0]["level"], "warning");
        assert_eq!(results[0]["properties"]["outcome"], "unknown");
        assert_eq!(results[1]["ruleId"], "covered");
        assert_eq!(results[1]["level"], "none");
        assert_eq!(results[1]["kind"], "pass");
        assert_eq!(results[1]["properties"]["witness"]["x"], "0");
    }

    #[test]
    fn maps_assumed_clauses_and_skips_import_trusted_atoms() {
        let atom = atom(
            r#"
atom f(x: i64) -> i64
requires assume: x >= 0;
ensures assume "non-negative": result >= 0;
body: { x + 1 };
"#,
        );
        let mut results = Vec::new();
        collect_atom(&mut results, &atom, "f", None);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["ruleId"], "assumed_clause");
        assert_eq!(results[0]["level"], "note");
        assert!(message(&results[1]).contains("trusted, not proved"));
        let mut imported = Vec::new();
        collect_atom_results(
            &mut imported,
            AtomFindings {
                atom: &atom,
                name: "f",
                report: None,
                cert_result: None,
                failure_diagnostics: &[],
                diagnostics: &[],
                imported_contract_trusted: true,
            },
        );
        assert!(imported.is_empty());
    }

    #[test]
    fn maps_cert_status_and_failure_diagnostics_without_a_report() {
        let atom = source_atom();
        let cert = ("unknown".to_string(), "unknown".to_string());
        let diagnostic = Diagnostic {
            code: "unknown".to_string(),
            severity: "warning".to_string(),
            atom: "f".to_string(),
            message: "Z3 returned unknown\nextra solver details".to_string(),
            tags: Vec::new(),
            escalation_reason: None,
        };
        let mut results = Vec::new();
        collect_atom_results(
            &mut results,
            AtomFindings {
                atom: &atom,
                name: "f",
                report: None,
                cert_result: Some(&cert),
                failure_diagnostics: &[diagnostic],
                diagnostics: &[],
                imported_contract_trusted: false,
            },
        );
        assert_eq!(results[0]["ruleId"], "unknown");
        assert_eq!(results[0]["level"], "warning");
        assert_eq!(message(&results[0]), "Z3 returned unknown (atom `f`)");
        assert_eq!(results[0]["properties"]["outcome"], "unknown");
    }

    #[test]
    fn maps_only_the_first_error_diagnostic_without_a_report() {
        let atom = source_atom();
        let diagnostics = [
            Diagnostic {
                code: "strict_array_types".to_string(),
                severity: "error".to_string(),
                atom: "f".to_string(),
                message: "Array type mismatch\nadditional detail".to_string(),
                tags: Vec::new(),
                escalation_reason: None,
            },
            Diagnostic {
                code: "another_error".to_string(),
                severity: "error".to_string(),
                atom: "f".to_string(),
                message: "Another error".to_string(),
                tags: Vec::new(),
                escalation_reason: None,
            },
        ];
        let mut results = Vec::new();
        collect_atom_results(
            &mut results,
            AtomFindings {
                atom: &atom,
                name: "f",
                report: None,
                cert_result: None,
                failure_diagnostics: &[],
                diagnostics: &diagnostics,
                imported_contract_trusted: false,
            },
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["ruleId"], "strict_array_types");
        assert_eq!(results[0]["level"], "error");
        assert_eq!(message(&results[0]), "Array type mismatch (atom `f`)");
        assert_eq!(results[0]["properties"]["outcome"], "failed");
    }

    #[test]
    fn maps_axiom_rejection_and_rule_indices() {
        let atom = source_atom();
        let mut collector = SarifCollector::new();
        let report =
            json!({"atom":"f","status":"failed","failure_type":"failed","reason":"failed"});
        collector.collect_atom(AtomFindings {
            atom: &atom,
            name: "f",
            report: Some(&report),
            cert_result: None,
            failure_diagnostics: &[],
            diagnostics: &[],
            imported_contract_trusted: false,
        });
        collector.push_axiom_rejected("f", Some(&["propext".to_string()]), Some(&atom.span));
        let document = collector.document(VerifyOutcome::Verified);
        let run = &document["runs"][0];
        let rules = run["tool"]["driver"]["rules"].as_array().unwrap();
        let results = run["results"].as_array().unwrap();
        assert_eq!(results[1]["ruleId"], "axiom_rejected");
        assert_eq!(results[1]["properties"]["disallowed_axioms"][0], "propext");
        for result in results {
            let index = result["ruleIndex"].as_u64().unwrap_or_default() as usize;
            assert!(index < rules.len());
            assert_eq!(rules[index]["id"], result["ruleId"]);
        }
    }

    #[test]
    fn absolute_paths_become_file_uris_and_columns_are_one_based() {
        assert_eq!(sarif_uri("/tmp/a file.mm"), "file:///tmp/a%20file.mm");
        assert_eq!(sarif_uri("src/a file.mm"), "src/a%20file.mm");
        let atom = source_atom();
        let report = json!({
            "atom":"f",
            "status":"failed",
            "reason":"bad",
            "span":{"file":"/tmp/a file.mm","line":2,"col":4,"len":3}
        });
        let mut results = Vec::new();
        collect_atom(&mut results, &atom, "f", Some(&report));
        assert_eq!(
            results[0]["locations"][0]["physicalLocation"]["region"]["startColumn"],
            4
        );
        assert_eq!(
            results[0]["locations"][0]["physicalLocation"]["region"]["endColumn"],
            7
        );
    }

    #[test]
    fn schema_has_rules_in_first_use_order_and_consistent_indices() {
        let atom = source_atom();
        let mut collector = SarifCollector::new();
        let report = json!({
            "atom":"f",
            "status":"success",
            "ensures_outcomes":[
                {"clause":"a","outcome":"vacuous"},
                {"clause":"b","outcome":"unknown"}
            ]
        });
        collector.collect_atom(AtomFindings {
            atom: &atom,
            name: "f",
            report: Some(&report),
            cert_result: None,
            failure_diagnostics: &[],
            diagnostics: &[],
            imported_contract_trusted: false,
        });
        let document = collector.document(VerifyOutcome::Verified);
        let run = &document["runs"][0];
        assert_eq!(document["version"], "2.1.0");
        assert_eq!(
            document["$schema"],
            "https://json.schemastore.org/sarif-2.1.0.json"
        );
        assert_eq!(run["tool"]["driver"]["name"], "mumei");
        let rules = run["tool"]["driver"]["rules"].as_array().unwrap();
        assert_eq!(rules[0]["id"], "vacuous");
        assert_eq!(rules[1]["id"], "unknown");
        for result in run["results"].as_array().unwrap() {
            let index = result["ruleIndex"].as_u64().unwrap() as usize;
            assert_eq!(rules[index]["id"], result["ruleId"]);
        }
    }
}
