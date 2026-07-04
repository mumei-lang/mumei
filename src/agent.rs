use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

pub(crate) struct AgentReport {
    pub(crate) success: bool,
    pub(crate) spec_health_issues: Vec<Value>,
    pub(crate) verification_violations: Vec<Value>,
    pub(crate) cross_validation_gaps: Vec<Value>,
    pub(crate) next_steps: Vec<Value>,
}

fn json_array(value: &Value, key: &str) -> Vec<Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn json_values(value: &Value, keys: &[&str]) -> Vec<Value> {
    keys.iter().flat_map(|key| json_array(value, key)).collect()
}

fn agent_report(value: &Value, success_hint: bool, is_spec: bool) -> AgentReport {
    let mut spec_health_issues = json_array(value, "spec_health_issues");
    if spec_health_issues.is_empty() && is_spec {
        spec_health_issues = json_values(
            value,
            &[
                "contradictions",
                "ambiguities",
                "overconstraints",
                "completeness_warnings",
                "vacuity_warnings",
                "errors",
            ],
        );
    }

    let mut verification_violations = json_array(value, "verification_violations");
    if verification_violations.is_empty() && !is_spec {
        verification_violations = json_values(value, &["issues", "errors"]);
    }

    let mut cross_validation_gaps = json_array(value, "cross_validation_gaps");
    if cross_validation_gaps.is_empty() {
        cross_validation_gaps = json_values(
            value,
            &[
                "missing_constraints",
                "divergences",
                "drift_issues",
                "missing_constraint_issues",
            ],
        );
    }

    let next_steps = json_array(value, "next_steps");
    let success = value
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(success_hint)
        && spec_health_issues.is_empty()
        && verification_violations.is_empty()
        && cross_validation_gaps.is_empty();

    AgentReport {
        success,
        spec_health_issues,
        verification_violations,
        cross_validation_gaps,
        next_steps,
    }
}

fn parse_agent_json(output: &Output) -> Result<Value, String> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    let candidate = if trimmed.starts_with('{') {
        trimmed.to_string()
    } else if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
        trimmed[start..=end].to_string()
    } else {
        String::new()
    };

    if candidate.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "mumei-agent did not emit JSON (exit: {}). stderr: {}",
            output.status,
            stderr.trim()
        ));
    }

    serde_json::from_str(&candidate).map_err(|err| {
        let stderr = String::from_utf8_lossy(&output.stderr);
        format!(
            "failed to parse mumei-agent JSON: {}. stderr: {}",
            err,
            stderr.trim()
        )
    })
}

fn run_agent(mut process: Command, is_spec: bool) -> Result<AgentReport, String> {
    let output = process.output().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            "mumei-agent not found in PATH; install mumei-agent or add it to PATH".to_string()
        } else {
            format!("failed to run mumei-agent: {}", err)
        }
    })?;
    let value = parse_agent_json(&output)?;
    Ok(agent_report(&value, output.status.success(), is_spec))
}

pub(crate) fn validate_spec(input: &Path) -> Result<AgentReport, String> {
    let mut process = Command::new("mumei-agent");
    process
        .arg("validate-spec")
        .arg("--input")
        .arg(input)
        .arg("--format")
        .arg("json");
    run_agent(process, true)
}

pub(crate) fn validate_code(input: &Path, language: &str) -> Result<AgentReport, String> {
    let mut process = Command::new("mumei-agent");
    process
        .arg("validate-code")
        .arg("--input")
        .arg(input)
        .arg("--language")
        .arg(language);
    run_agent(process, false)
}

pub(crate) fn infer_code_language(path: &Path) -> Result<String, String> {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("py") => Ok("python".to_string()),
        Some("rs") => Ok("rust".to_string()),
        Some("go") => Ok("go".to_string()),
        Some("ts") | Some("tsx") => Ok("typescript".to_string()),
        _ => Err(format!(
            "unsupported code file extension for '{}'; expected .py, .rs, .ts, .tsx, or .go",
            path.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn infer_code_language_python() {
        assert_eq!(infer_code_language(Path::new("app.py")).unwrap(), "python");
    }

    #[test]
    fn infer_code_language_rust() {
        assert_eq!(infer_code_language(Path::new("lib.rs")).unwrap(), "rust");
    }

    #[test]
    fn infer_code_language_go() {
        assert_eq!(infer_code_language(Path::new("main.go")).unwrap(), "go");
    }

    #[test]
    fn infer_code_language_typescript() {
        assert_eq!(
            infer_code_language(Path::new("service.ts")).unwrap(),
            "typescript"
        );
    }

    #[test]
    fn infer_code_language_tsx() {
        assert_eq!(
            infer_code_language(Path::new("component.tsx")).unwrap(),
            "typescript"
        );
    }

    #[test]
    fn infer_code_language_unsupported() {
        let err = infer_code_language(Path::new("data.json")).unwrap_err();
        assert!(err.contains("unsupported code file extension"));
        assert!(err.contains(".ts, .tsx"));
    }
}
