use crate::commands::verify::{resolve_verify_profile, EXIT_USAGE_ERROR};
use crate::env_registry::{EnvVar, ENV_VARS};
use mumei_core::manifest;

struct EnvironmentEntry<'a> {
    variable: &'a EnvVar,
    value: Option<String>,
}

fn json_environment_entry(entry: &EnvironmentEntry<'_>) -> serde_json::Value {
    let variable = entry.variable;
    let sensitive = is_sensitive_name(variable.name);
    let mut json_entry = serde_json::Map::new();
    json_entry.insert("name".to_owned(), serde_json::json!(variable.name));
    json_entry.insert("set".to_owned(), serde_json::json!(entry.value.is_some()));
    json_entry.insert(
        "default".to_owned(),
        serde_json::json!(json_default(variable.default)),
    );
    json_entry.insert(
        "description".to_owned(),
        serde_json::json!(variable.description),
    );
    json_entry.insert("sensitive".to_owned(), serde_json::json!(sensitive));
    if !sensitive {
        json_entry.insert(
            "value".to_owned(),
            serde_json::json!(entry.value.as_deref().map(redact_url_userinfo)),
        );
    }
    serde_json::Value::Object(json_entry)
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
            .map(json_environment_entry)
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
            Some(_) if is_sensitive_name(entry.variable.name) => println!(
                "  {}: set  ({})",
                entry.variable.name, entry.variable.category
            ),
            Some(value) => println!(
                "  {}={}  ({})",
                entry.variable.name,
                redact_url_userinfo(value),
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

fn is_sensitive_name(name: &str) -> bool {
    let upper_name = name.to_ascii_uppercase();
    ["TOKEN", "KEY", "SECRET", "PASSWORD"]
        .iter()
        .any(|needle| upper_name.contains(needle))
}

fn redact_url_userinfo(value: &str) -> String {
    let Some(scheme_end) = value.find("://") else {
        return value.to_owned();
    };
    let authority_start = scheme_end + 3;
    let authority_end = value[authority_start..]
        .find('/')
        .map(|offset| authority_start + offset)
        .unwrap_or(value.len());
    let Some(at_offset) = value[authority_start..authority_end].find('@') else {
        return value.to_owned();
    };
    let host_start = authority_start + at_offset + 1;
    format!("{}{}", &value[..authority_start], &value[host_start..])
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
    fn identifies_sensitive_names_case_insensitively() {
        assert!(is_sensitive_name("mumei_registry_token"));
        assert!(is_sensitive_name("API_key"));
        assert!(is_sensitive_name("SERVICE_SECRET"));
        assert!(is_sensitive_name("ADMIN_PASSWORD"));
        assert!(!is_sensitive_name("MUMEI_TASK_ID"));
    }

    #[test]
    fn json_omits_values_for_sensitive_variables() {
        let variable = EnvVar {
            name: "SERVICE_TOKEN",
            category: "integration",
            default: "unset",
            description: "test",
        };
        let entry = EnvironmentEntry {
            variable: &variable,
            value: Some("token-secret".to_owned()),
        };
        let json = json_environment_entry(&entry);

        assert_eq!(json["sensitive"], true);
        assert!(json.get("value").is_none());
        assert!(!json.to_string().contains("token-secret"));
    }

    #[test]
    fn strips_url_userinfo_without_changing_other_values() {
        assert_eq!(
            redact_url_userinfo("https://user:pass@host/path"),
            "https://host/path"
        );
        assert_eq!(redact_url_userinfo("https://user@host"), "https://host");
        assert_eq!(
            redact_url_userinfo("https://host/path"),
            "https://host/path"
        );
        assert_eq!(redact_url_userinfo("https://host/a@b"), "https://host/a@b");
        assert_eq!(redact_url_userinfo("not-a-url@host"), "not-a-url@host");
    }
}
