use crate::commands::verify::{resolve_verify_profile, EXIT_USAGE_ERROR};
use crate::env_registry::{EnvVar, ENV_VARS};
use mumei_core::manifest;

struct EnvironmentEntry<'a> {
    variable: &'a EnvVar,
    value: Option<String>,
}

fn json_environment_entry(entry: &EnvironmentEntry<'_>) -> serde_json::Value {
    let variable = entry.variable;
    let sensitive = variable.sensitive;
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
            serde_json::json!(entry.value.as_deref().map(sanitize_url_value)),
        );
    }
    serde_json::Value::Object(json_entry)
}

pub(crate) fn cmd_config(json: bool, all: bool) {
    let manifest_config = manifest::find_and_load();
    let manifest_path = manifest_config
        .as_ref()
        .map(|(directory, _)| directory.join("mumei.toml"))
        .or_else(manifest::find_manifest_path);
    if manifest_config.is_none() {
        if let Some(path) = &manifest_path {
            eprintln!("error: {} could not be parsed", path.display());
            std::process::exit(EXIT_USAGE_ERROR);
        }
    }
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
            Some(_) if entry.variable.sensitive => println!(
                "  {}: set  ({})",
                entry.variable.name, entry.variable.category
            ),
            Some(value) => println!(
                "  {}={}  ({})",
                entry.variable.name,
                sanitize_url_value(value),
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

fn sanitize_url_value(value: &str) -> String {
    let trimmed_value = value.trim();
    if !is_standalone_url(trimmed_value) {
        return value.to_owned();
    }
    let scheme_end = trimmed_value
        .find("://")
        .expect("standalone URL has a scheme separator");
    let authority_start = scheme_end + 3;
    let authority_end = trimmed_value[authority_start..]
        .find(['/', '?', '#'])
        .map(|offset| authority_start + offset)
        .unwrap_or(trimmed_value.len());
    let authority = &trimmed_value[authority_start..authority_end];
    let host = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);
    let suffix = &trimmed_value[authority_end..];
    let suffix_end = suffix.find(['?', '#']).unwrap_or(suffix.len());
    format!(
        "{}{}{}",
        &trimmed_value[..authority_start],
        host,
        &suffix[..suffix_end]
    )
}

fn is_standalone_url(value: &str) -> bool {
    let Some((scheme, _)) = value.split_once("://") else {
        return false;
    };
    let mut chars = scheme.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_alphabetic())
        && chars.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '.' | '-')
        })
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
    fn json_omits_values_for_sensitive_variables() {
        let variable = EnvVar {
            name: "SERVICE_TOKEN",
            category: "integration",
            default: "unset",
            description: "test",
            sensitive: true,
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
    fn sanitizes_url_userinfo_queries_and_fragments() {
        assert_eq!(
            sanitize_url_value("https://user:pass@host:8443/path?token=x#fragment"),
            "https://host:8443/path"
        );
        assert_eq!(sanitize_url_value("https://user@host"), "https://host");
        assert_eq!(sanitize_url_value("https://host/path"), "https://host/path");
        assert_eq!(sanitize_url_value("https://host/a@b"), "https://host/a@b");
        assert_eq!(
            sanitize_url_value("https://host/path?query=x#fragment"),
            "https://host/path"
        );
        assert_eq!(sanitize_url_value("not-a-url@host"), "not-a-url@host");
        assert_eq!(
            sanitize_url_value(r#"{"u":"https://h/x?y"}"#),
            r#"{"u":"https://h/x?y"}"#
        );
        assert_eq!(sanitize_url_value("key=https://u:p@h"), "key=https://u:p@h");
        assert_eq!(
            sanitize_url_value("https://u:p@h/path with-space"),
            "https://h/path with-space"
        );
        assert_eq!(sanitize_url_value("https://u:p@h/a b?t=1"), "https://h/a b");
        assert_eq!(sanitize_url_value(" https://u:p@h/x?t=1 "), "https://h/x");
        assert_eq!(
            sanitize_url_value("\t\nhttps://u:p@h/x?t=1\r\n\t"),
            "https://h/x"
        );
    }
}
