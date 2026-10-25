use crate::charter;
use crate::hir::{lower_atom_to_hir_with_env, HirAtom};
use crate::parser::{Atom, Item, Param, TrustLevel};
use crate::verification::{self, ModuleEnv, VerdictKind, VerificationConfig};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

const SCHEMA: &str = "mumei.observation-check/v1";
const HARVEST_SCHEMA: &str = "mumei.test-harvest/v1";
const VERDICTS: [&str; 7] = [
    "consistent",
    "contradicts_ensures",
    "outside_requires",
    "inconclusive",
    "rejection_explained",
    "requires_admits_rejected",
    "not_checked",
];

#[derive(Debug, Serialize)]
pub struct ObserveReport {
    pub schema: &'static str,
    pub file: String,
    pub harvest: String,
    pub results: Vec<ObservationResult>,
    pub totals: BTreeMap<String, usize>,
}

#[derive(Debug, Serialize)]
pub struct ObservationResult {
    pub kind: String,
    pub target: String,
    pub test: String,
    pub file: String,
    pub line: usize,
    pub args: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub predicate: Option<String>,
    pub verdict: &'static str,
    pub detail: String,
}

impl ObserveReport {
    pub fn json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn human(&self) -> String {
        let mut lines = self
            .results
            .iter()
            .map(|result| {
                format!(
                    "{} {} in {}:{}: {} ({})",
                    result.verdict,
                    result.target,
                    result.file,
                    result.line,
                    result.test,
                    result.detail
                )
            })
            .collect::<Vec<_>>();
        lines.push(format!(
            "Totals: {} observations; {}",
            self.results.len(),
            VERDICTS
                .iter()
                .filter_map(|verdict| {
                    let count = self.totals.get(*verdict).copied().unwrap_or_default();
                    (count > 0).then(|| format!("{verdict} {count}"))
                })
                .collect::<Vec<_>>()
                .join(", ")
        ));
        lines.join("\n")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckVerdict {
    Ok,
    Rejected,
    Inconclusive,
}

pub fn analyze(
    input: &str,
    harvest_path: &str,
    harvest_json: &str,
    implementation: &[Item],
    generic_implementation: &[Item],
    module_env: &ModuleEnv,
    atom_filter: Option<&str>,
) -> Result<ObserveReport, String> {
    let document: Value = serde_json::from_str(harvest_json)
        .map_err(|error| format!("invalid harvest JSON: {error}"))?;
    if document.get("schema").and_then(Value::as_str) != Some(HARVEST_SCHEMA) {
        return Err(format!("harvest schema must be '{HARVEST_SCHEMA}'"));
    }
    let observations = document
        .get("observations")
        .and_then(Value::as_array)
        .ok_or_else(|| "harvest JSON is missing an observations array".to_string())?;

    let output_dir = tempfile::tempdir().map_err(|error| error.to_string())?;
    let config = VerificationConfig::default();
    let mut results = Vec::new();
    for (index, observation) in observations.iter().enumerate() {
        let object = observation
            .as_object()
            .ok_or_else(|| format!("observation {} must be an object", index + 1))?;
        let kind = required_string(object, "kind", index)?;
        let target = required_string(object, "target", index)?;
        if atom_filter.is_some_and(|filter| filter != target) {
            continue;
        }
        let test = required_string(object, "test", index)?;
        let file = required_string(object, "file", index)?;
        let line = object
            .get("line")
            .and_then(Value::as_u64)
            .and_then(|line| usize::try_from(line).ok())
            .ok_or_else(|| format!("observation {} has an invalid line", index + 1))?;
        let args = object.get("args").cloned().unwrap_or(Value::Null);
        let result = object.get("result").cloned();
        let predicate = object
            .get("predicate")
            .and_then(Value::as_str)
            .map(str::to_string);

        let Some(atom) = find_atom(&target, implementation, generic_implementation, module_env)
        else {
            results.push(result_record(
                kind,
                target,
                test,
                file,
                line,
                args,
                result,
                predicate,
                "not_checked",
                "no_atom",
            ));
            continue;
        };
        if let Some(detail) = unsupported_atom_detail(atom) {
            results.push(result_record(
                kind,
                target,
                test,
                file,
                line,
                args,
                result,
                predicate,
                "not_checked",
                detail,
            ));
            continue;
        }

        let mapped_args = map_args(atom, &args, module_env);
        match mapped_args {
            Err(detail) => results.push(result_record(
                kind,
                target,
                test,
                file,
                line,
                args,
                result,
                predicate,
                "not_checked",
                &detail,
            )),
            Ok(values) => {
                let pin = pin_expression(atom, &values);
                let admissible_ensures = format!("({})", clause_or_true(&atom.requires));
                let admissible = run_check(
                    atom,
                    format!("__observe_{index}_admissible"),
                    &pin,
                    &admissible_ensures,
                    "0",
                    None,
                    module_env,
                    output_dir.path(),
                    &config,
                );
                let verdict = classify_observation(
                    &kind,
                    admissible,
                    || {
                        let observed = result
                            .as_ref()
                            .ok_or_else(|| "example observation is missing result".to_string())?;
                        let body = render_for_type(
                            observed,
                            atom.return_type.as_deref().unwrap_or("i64"),
                            module_env,
                        )?;
                        let ensures = format!(
                            "({}) => ({})",
                            clause_or_true(&atom.requires),
                            clause_or_true(&atom.ensures)
                        );
                        Ok(run_check(
                            atom,
                            format!("__observe_{index}_example"),
                            &pin,
                            &ensures,
                            &body,
                            atom.return_type.as_deref(),
                            module_env,
                            output_dir.path(),
                            &config,
                        ))
                    },
                    || {
                        let predicate = predicate.as_deref().ok_or_else(|| {
                            "relation observation is missing predicate".to_string()
                        })?;
                        let predicate = replace_result_identifier(predicate);
                        let ensures = format!(
                            "(({}) && ({})) => !({})",
                            clause_or_true(&atom.requires),
                            clause_or_true(&atom.ensures),
                            predicate
                        );
                        let mut relation_atom = atom.clone();
                        relation_atom
                            .params
                            .push(observation_result_param(atom.return_type.as_deref()));
                        Ok(run_check(
                            &relation_atom,
                            format!("__observe_{index}_relation"),
                            &pin,
                            &ensures,
                            "__observe_result",
                            atom.return_type.as_deref(),
                            module_env,
                            output_dir.path(),
                            &config,
                        ))
                    },
                );
                let (verdict, detail) = verdict.unwrap_or_else(|detail| ("not_checked", detail));
                results.push(result_record(
                    kind, target, test, file, line, args, result, predicate, verdict, &detail,
                ));
            }
        }
    }

    let mut totals = VERDICTS
        .iter()
        .map(|verdict| (verdict.to_string(), 0))
        .collect::<BTreeMap<_, _>>();
    for result in &results {
        *totals.entry(result.verdict.to_string()).or_default() += 1;
    }
    Ok(ObserveReport {
        schema: SCHEMA,
        file: input.to_string(),
        harvest: harvest_path.to_string(),
        results,
        totals,
    })
}

fn required_string(
    object: &serde_json::Map<String, Value>,
    field: &str,
    index: usize,
) -> Result<String, String> {
    object
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            format!(
                "observation {} is missing string field '{field}'",
                index + 1
            )
        })
}

#[allow(clippy::too_many_arguments)]
fn result_record(
    kind: String,
    target: String,
    test: String,
    file: String,
    line: usize,
    args: Value,
    result: Option<Value>,
    predicate: Option<String>,
    verdict: &'static str,
    detail: &str,
) -> ObservationResult {
    ObservationResult {
        kind,
        target,
        test,
        file,
        line,
        args,
        result,
        predicate,
        verdict,
        detail: detail.to_string(),
    }
}

fn find_atom<'a>(
    target: &str,
    implementation: &'a [Item],
    generic_implementation: &'a [Item],
    module_env: &'a ModuleEnv,
) -> Option<&'a Atom> {
    module_env.get_atom(target).or_else(|| {
        charter::implementation_atoms(implementation)
            .into_iter()
            .chain(charter::implementation_atoms(generic_implementation))
            .find(|candidate| candidate.name == target)
            .map(|candidate| candidate.atom)
    })
}

fn unsupported_atom_detail(atom: &Atom) -> Option<&'static str> {
    if !atom.type_params.is_empty() {
        Some("generic_atom")
    } else if !atom.forall_constraints.is_empty() {
        Some("unsupported_forall_constraints")
    } else {
        None
    }
}

fn map_args(atom: &Atom, args: &Value, module_env: &ModuleEnv) -> Result<Vec<String>, String> {
    let values = match args {
        Value::Object(object) => {
            if object.len() != atom.params.len()
                || atom
                    .params
                    .iter()
                    .any(|param| !object.contains_key(&param.name))
            {
                return Err("argument_names_mismatch".to_string());
            }
            atom.params
                .iter()
                .map(|param| object.get(&param.name).expect("checked argument"))
                .collect::<Vec<_>>()
        }
        Value::Array(values) if values.len() == atom.params.len() => values.iter().collect(),
        Value::Array(_) => return Err("argument_count_mismatch".to_string()),
        _ => return Err("unsupported_args_shape".to_string()),
    };
    atom.params
        .iter()
        .zip(values)
        .map(|(param, value)| {
            render_for_type(
                value,
                param.type_name.as_deref().unwrap_or("i64"),
                module_env,
            )
        })
        .collect()
}

fn render_for_type(
    value: &Value,
    type_name: &str,
    module_env: &ModuleEnv,
) -> Result<String, String> {
    let is_bool = is_bool_type(type_name, module_env);
    match value {
        Value::Bool(value) if is_bool => Ok(value.to_string()),
        Value::Number(number) if !is_bool => {
            let value = number
                .as_i64()
                .ok_or_else(|| "integer_out_of_i64_range".to_string())?;
            Ok(render_integer(value))
        }
        Value::Bool(_) => Err("bool_for_non_bool_type".to_string()),
        Value::Number(_) => Err("int_for_bool_type".to_string()),
        _ => Err("unsupported_value_type".to_string()),
    }
}

fn is_bool_type(type_name: &str, module_env: &ModuleEnv) -> bool {
    let unqualified = type_name.rsplit("::").next().unwrap_or(type_name);
    unqualified == "bool"
        || module_env
            .get_type(type_name)
            .is_some_and(|refined| refined._base_type == "bool")
}

fn render_integer(value: i64) -> String {
    if value == i64::MIN {
        "(0 - 9223372036854775807 - 1)".to_string()
    } else if value < 0 {
        format!("(0 - {})", value.unsigned_abs())
    } else {
        value.to_string()
    }
}

fn pin_expression(atom: &Atom, values: &[String]) -> String {
    if atom.params.is_empty() {
        return "true".to_string();
    }
    atom.params
        .iter()
        .zip(values)
        .map(|(param, value)| format!("{} == {}", param.name, value))
        .collect::<Vec<_>>()
        .join(" && ")
}

fn clause_or_true(clause: &str) -> &str {
    if clause.trim().is_empty() {
        "true"
    } else {
        clause
    }
}

fn replace_result_identifier(predicate: &str) -> String {
    regex::Regex::new(r"\bresult\b")
        .expect("static regex")
        .replace_all(predicate, "__observe_result")
        .into_owned()
}

fn observation_result_param(return_type: Option<&str>) -> Param {
    Param {
        name: "__observe_result".to_string(),
        type_name: Some(return_type.unwrap_or("i64").to_string()),
        type_ref: None,
        consume: false,
        is_ref: false,
        is_ref_mut: false,
        fn_contract_requires: None,
        fn_contract_ensures: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn run_check(
    target: &Atom,
    name: String,
    requires: &str,
    ensures: &str,
    body: &str,
    return_type: Option<&str>,
    module_env: &ModuleEnv,
    output_dir: &Path,
    config: &VerificationConfig,
) -> CheckVerdict {
    let mut wrapper = target.clone();
    wrapper.name = name;
    wrapper.type_params.clear();
    wrapper.where_bounds.clear();
    wrapper.trace_id = None;
    wrapper.requires = requires.to_string();
    wrapper.ensures = ensures.to_string();
    wrapper.clause_labels.clear();
    wrapper.clause_modes.clear();
    wrapper.body_expr = body.to_string();
    wrapper.return_type = return_type.map(str::to_string);
    wrapper.effects.clear();
    wrapper.effect_pre.clear();
    wrapper.effect_post.clear();
    wrapper.covers.clear();
    wrapper.invariant = None;
    wrapper.decreases = None;
    wrapper.resources.clear();
    wrapper.trust_level = TrustLevel::Verified;
    wrapper.is_async = false;

    let hir: HirAtom = lower_atom_to_hir_with_env(&wrapper, Some(module_env));
    match verification::verify_with_config(
        &hir,
        output_dir,
        module_env,
        config.timeout_ms,
        config.global_max_unroll,
    ) {
        Ok(_) => CheckVerdict::Ok,
        Err(error) => classify_verdict(error.verdict()),
    }
}

fn classify_verdict(verdict: Option<VerdictKind>) -> CheckVerdict {
    match verdict {
        None | Some(VerdictKind::Inconclusive) => CheckVerdict::Inconclusive,
        Some(VerdictKind::Rejected) => CheckVerdict::Rejected,
    }
}

fn classify_observation(
    kind: &str,
    admissible: CheckVerdict,
    example: impl FnOnce() -> Result<CheckVerdict, String>,
    relation: impl FnOnce() -> Result<CheckVerdict, String>,
) -> Result<(&'static str, String), String> {
    match admissible {
        CheckVerdict::Inconclusive => {
            return Ok(("inconclusive", "admissibility check inconclusive".into()));
        }
        CheckVerdict::Rejected => {
            return Ok(match kind {
                "rejects" => (
                    "rejection_explained",
                    "arguments are outside requires".into(),
                ),
                _ => ("outside_requires", "arguments are outside requires".into()),
            })
        }
        CheckVerdict::Ok => {}
    }
    match kind {
        "example" => match example()? {
            CheckVerdict::Ok => Ok(("consistent", "example satisfies the contract".into())),
            CheckVerdict::Rejected => Ok((
                "contradicts_ensures",
                "observed result contradicts ensures".into(),
            )),
            CheckVerdict::Inconclusive => Ok(("inconclusive", "example check inconclusive".into())),
        },
        "relation" => match relation()? {
            CheckVerdict::Ok => Ok((
                "contradicts_ensures",
                "contract requires a result violating the predicate".into(),
            )),
            CheckVerdict::Rejected => Ok(("consistent", "contract permits the predicate".into())),
            CheckVerdict::Inconclusive => {
                Ok(("inconclusive", "relation check inconclusive".into()))
            }
        },
        "rejects" => Ok((
            "requires_admits_rejected",
            "arguments satisfy requires but the test expects rejection".into(),
        )),
        _ => Ok(("not_checked", "unsupported observation kind".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;
    use serde_json::json;

    fn atom() -> Atom {
        parser::parse_module_checked(
            "atom check(value: i64, enabled: bool)\n\
             requires: true;\n\
             ensures: true;\n\
             body: 0;\n",
        )
        .expect("parse atom")
        .into_iter()
        .find_map(|item| match item {
            Item::Atom(atom) => Some(atom),
            _ => None,
        })
        .expect("atom")
    }

    #[test]
    fn renders_signed_literals_in_mumei_syntax() {
        assert_eq!(render_integer(0), "0");
        assert_eq!(render_integer(4), "4");
        assert_eq!(render_integer(-4), "(0 - 4)");
        assert_eq!(render_integer(i64::MIN), "(0 - 9223372036854775807 - 1)");
    }

    #[test]
    fn maps_object_and_positional_arguments_by_declared_params() {
        let atom = atom();
        let env = ModuleEnv::default();
        assert_eq!(
            map_args(&atom, &json!({"value": -3, "enabled": true}), &env).unwrap(),
            vec!["(0 - 3)", "true"]
        );
        assert_eq!(
            map_args(&atom, &json!([i64::MIN, false]), &env).unwrap(),
            vec!["(0 - 9223372036854775807 - 1)", "false"]
        );
        assert!(map_args(&atom, &json!({"value": 1}), &env).is_err());
        assert!(map_args(&atom, &json!([1]), &env).is_err());
        assert!(map_args(&atom, &json!([true, false]), &env).is_err());
    }

    #[test]
    fn classifies_missing_verdicts_as_inconclusive() {
        assert_eq!(classify_verdict(None), CheckVerdict::Inconclusive);
        assert_eq!(
            classify_verdict(Some(VerdictKind::Rejected)),
            CheckVerdict::Rejected
        );
        assert_eq!(
            classify_verdict(Some(VerdictKind::Inconclusive)),
            CheckVerdict::Inconclusive
        );
    }

    #[test]
    fn classifies_examples_relations_and_rejects() {
        let no_check = || Ok(CheckVerdict::Ok);
        assert_eq!(
            classify_observation("example", CheckVerdict::Ok, no_check, no_check)
                .unwrap()
                .0,
            "consistent"
        );
        assert_eq!(
            classify_observation(
                "example",
                CheckVerdict::Ok,
                || Ok(CheckVerdict::Rejected),
                no_check
            )
            .unwrap()
            .0,
            "contradicts_ensures"
        );
        assert_eq!(
            classify_observation("relation", CheckVerdict::Ok, no_check, || Ok(
                CheckVerdict::Rejected
            ))
            .unwrap()
            .0,
            "consistent"
        );
        assert_eq!(
            classify_observation("rejects", CheckVerdict::Ok, no_check, no_check)
                .unwrap()
                .0,
            "requires_admits_rejected"
        );
        assert_eq!(
            classify_observation("rejects", CheckVerdict::Rejected, no_check, no_check)
                .unwrap()
                .0,
            "rejection_explained"
        );
    }
}
