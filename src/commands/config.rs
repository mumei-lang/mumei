use crate::commands::verify::{resolve_verify_profile, EXIT_USAGE_ERROR};
use crate::env_registry::{EnvVar, ENV_VARS};
use mumei_core::manifest;

struct EnvironmentEntry<'a> {
    variable: &'a EnvVar,
    value: Option<String>,
}

pub(crate) fn cmd_config(json: bool, all: bool) {
    let manifest_config = manifest::find_and_load();
    let manifest_path = manifest_config
        .as_ref()
        .map(|(directory, _)| directory.join("mumei.toml"));
    let manifest_profile = manifest_config.as_ref().and_then(|(directory, config)| {
        config
            .verify
            .profile
            .as_deref()
            .map(|profile| (directory.join("mumei.toml"), profile))
    });
    let profile = match resolve_verify_profile(
        None,
        manifest_profile
            .as_ref()
            .map(|(path, value)| (path.as_path(), *value)),
    ) {
        Ok(profile) => profile,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(EXIT_USAGE_ERROR);
        }
    };
    let environment = ENV_VARS
        .iter()
        .map(|variable| EnvironmentEntry {
            variable,
            value: std::env::var_os(variable.name)
                .map(|value| value.to_string_lossy().into_owned()),
        })
        .collect::<Vec<_>>();

    if json {
        let env = environment
            .iter()
            .filter(|entry| all || entry.value.is_some())
            .map(|entry| {
                let variable = entry.variable;
                let value = entry
                    .value
                    .as_deref()
                    .map(|value| redact_value(variable.name, value));
                serde_json::json!({
                    "name": variable.name,
                    "set": entry.value.is_some(),
                    "value": value,
                    "default": json_default(variable.default),
                    "description": variable.description,
                })
            })
            .collect::<Vec<_>>();
        let verify_profile = profile.map(|(profile, source)| {
            serde_json::json!({
                "name": profile.name(),
                "source": source.label(),
                "enables": profile.enabled_flags(),
            })
        });
        let payload = serde_json::json!({
            "manifest_path": manifest_path.map(|path| path.display().to_string()),
            "verify_profile": verify_profile,
            "env": env,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).expect("serialize config JSON")
        );
        return;
    }

    match &manifest_path {
        Some(path) => println!("mumei.toml: {}", path.display()),
        None => println!("mumei.toml: not found"),
    }
    match profile {
        Some((profile, source)) => {
            println!(
                "verify profile: {} (from {})",
                profile.name(),
                source.label()
            );
            println!("  enables: {}", profile.enabled_flags().join(", "));
        }
        None => println!(
            "verify profile: none (pass --profile to mumei verify, or set [verify] profile)"
        ),
    }
    let set_count = environment
        .iter()
        .filter(|entry| entry.value.is_some())
        .count();
    println!("environment ({set_count} set of {} known):", ENV_VARS.len());
    for entry in &environment {
        match entry.value.as_deref() {
            Some(value) => println!(
                "  {}={}  ({})",
                entry.variable.name,
                redact_value(entry.variable.name, value),
                entry.variable.category
            ),
            None if all => println!(
                "  {} (unset; default: {})",
                entry.variable.name,
                text_default(entry.variable.default)
            ),
            None => {}
        }
    }
}

fn redact_value<'a>(name: &str, value: &'a str) -> &'a str {
    let upper_name = name.to_ascii_uppercase();
    if ["TOKEN", "KEY", "SECRET", "PASSWORD"]
        .iter()
        .any(|needle| upper_name.contains(needle))
    {
        "***"
    } else {
        value
    }
}

fn text_default(default: &str) -> &str {
    if default.is_empty() {
        "-"
    } else {
        default
    }
}

fn json_default(default: &str) -> Option<&str> {
    if default.is_empty() || default == "unset" {
        None
    } else {
        Some(default)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_sensitive_names_case_insensitively() {
        assert_eq!(redact_value("mumei_registry_token", "secret-value"), "***");
        assert_eq!(redact_value("API_key", "secret-value"), "***");
        assert_eq!(redact_value("SERVICE_SECRET", "secret-value"), "***");
        assert_eq!(redact_value("ADMIN_PASSWORD", "secret-value"), "***");
        assert_eq!(redact_value("MUMEI_TASK_ID", "task-123"), "task-123");
    }
}
