use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const PACKAGE_MANIFEST: &str = "[package]\nname = \"config-fixture\"\nversion = \"0.1.0\"\n";

struct Scratch(PathBuf);

impl Scratch {
    fn new(manifest: Option<&str>) -> Self {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("mumei-config-cli-{}-{id}", std::process::id()));
        fs::create_dir_all(&directory).expect("create scratch project");
        if let Some(manifest) = manifest {
            fs::write(directory.join("mumei.toml"), manifest).expect("write manifest");
        }
        Self(directory)
    }

    fn root(&self) -> &Path {
        &self.0
    }

    fn run(&self, args: &[&str], envs: &[(&str, &str)]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("MUMEI_") {
                command.env_remove(name);
            }
        }
        command.args(args).current_dir(self.root());
        for (name, value) in envs {
            command.env(name, value);
        }
        command.output().expect("run mumei")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn verify_manifest(profile: &str) -> String {
    format!("{PACKAGE_MANIFEST}\n[verify]\nprofile = \"{profile}\"\n")
}

fn json_output(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid stdout JSON ({error}):\n{}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn no_manifest_reports_no_profile_and_environment_header() {
    let project = Scratch::new(None);
    let output = project.run(&["config"], &[]);
    let text = stdout(&output);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(text.starts_with("mumei.toml: not found\n"));
    assert!(text.contains(
        "verify profile: none (pass --profile to mumei verify, or set [verify] profile)"
    ));
    assert!(text.contains("environment ("));
    assert!(text.contains(" known):"));
}

#[test]
fn manifest_without_verify_section_reports_manifest_and_no_profile() {
    let project = Scratch::new(Some(PACKAGE_MANIFEST));
    let output = project.run(&["config"], &[]);
    let text = stdout(&output);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(text.starts_with(&format!(
        "mumei.toml: {}\n",
        project.root().join("mumei.toml").display()
    )));
    assert!(text.contains(
        "verify profile: none (pass --profile to mumei verify, or set [verify] profile)"
    ));
}

#[test]
fn malformed_manifest_exits_with_error_in_text_and_json_modes() {
    let project = Scratch::new(Some("[package\nname = \"broken\"\n"));
    for args in [&["config"][..], &["config", "--json"][..]] {
        let output = project.run(args, &[]);
        let message = stderr(&output);

        assert_eq!(output.status.code(), Some(2));
        assert!(message.contains("Failed to parse"));
        assert!(message.contains(&project.root().join("mumei.toml").display().to_string()));
        assert!(message.contains("could not be parsed"));
        assert!(stdout(&output).is_empty());
    }
}

#[test]
fn strict_manifest_profile_lists_exact_enabled_flags() {
    let project = Scratch::new(Some(&verify_manifest("strict")));
    let output = project.run(&["config"], &[]);
    let text = stdout(&output);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(text.contains("verify profile: strict (from mumei.toml)"));
    assert!(text.contains("  enables: --strict-imports, --strict-array-types, --warn-fragment\n"));
}

#[test]
fn audit_manifest_profile_includes_audit_flags() {
    let project = Scratch::new(Some(&verify_manifest("audit")));
    let output = project.run(&["config"], &[]);
    let text = stdout(&output);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(text.contains("verify profile: audit (from mumei.toml)"));
    assert!(text.contains(
        "  enables: --strict-imports, --strict-array-types, --warn-fragment, --warn-untyped-arrays, --enable-vacuity-check, --detect-loops\n"
    ));
}

#[test]
fn invalid_manifest_profile_returns_usage_error_with_verify_message() {
    let project = Scratch::new(Some(&verify_manifest("relaxed")));
    let output = project.run(&["config"], &[]);
    let message = stderr(&output);

    assert_eq!(output.status.code(), Some(2));
    assert!(message.contains(&project.root().join("mumei.toml").display().to_string()));
    assert!(message.contains("invalid verify profile 'relaxed'"));
    assert!(message.contains("allowed values: strict, audit"));
    assert!(stdout(&output).is_empty());
}

#[test]
fn text_lists_set_registered_variables_but_omits_unset_and_unknown_names() {
    let project = Scratch::new(None);
    let output = project.run(
        &["config"],
        &[
            ("MUMEI_TASK_ID", "task-from-test"),
            ("MUMEI_UNKNOWN_CONFIG", "not-registered"),
        ],
    );
    let text = stdout(&output);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(text.contains("  MUMEI_TASK_ID=task-from-test  (integration)"));
    assert!(!text.contains("MUMEI_SPEC_METADATA"));
    assert!(!text.contains("MUMEI_UNKNOWN_CONFIG"));
}

#[test]
fn registry_url_userinfo_is_redacted_in_text_and_json() {
    let project = Scratch::new(None);
    let url = "https://registry-user:registry-password@example.test/packages";
    let text_output = project.run(&["config"], &[("MUMEI_REGISTRY_URL", url)]);
    let text = stdout(&text_output);
    assert!(text_output.status.success(), "{}", stderr(&text_output));
    assert!(text.contains("MUMEI_REGISTRY_URL=https://example.test/packages  (user)"));
    assert!(!text.contains("registry-password"));
    assert!(!text.contains("***"));

    let json_result = project.run(&["config", "--json"], &[("MUMEI_REGISTRY_URL", url)]);
    let payload = json_output(&json_result);
    assert!(json_result.status.success(), "{}", stderr(&json_result));
    let registry_url = payload["env"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "MUMEI_REGISTRY_URL")
        .expect("registered registry URL");
    assert_eq!(registry_url["value"], "https://example.test/packages");
    assert_eq!(registry_url["sensitive"], false);
    assert!(!stdout(&json_result).contains("registry-password"));
    assert!(!stdout(&json_result).contains("***"));

    let all_output = project.run(&["config", "--all"], &[("MUMEI_REGISTRY_URL", url)]);
    let all_text = stdout(&all_output);
    assert!(all_output.status.success(), "{}", stderr(&all_output));
    assert!(all_text.contains("MUMEI_REGISTRY_URL=https://example.test/packages  (user)"));
    assert!(!all_text.contains("registry-password"));
    assert!(!all_text.contains("***"));
}

#[test]
fn all_text_includes_unset_registered_variables_and_their_defaults() {
    let project = Scratch::new(None);
    let output = project.run(&["config", "--all"], &[]);
    let text = stdout(&output);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(text.contains("  MUMEI_TASK_ID (unset; default: unset)"));
    assert!(text.contains("  MUMEI_UPDATE_REFERENCE_DOCS (unset; default: off)"));
}

#[test]
fn json_reports_manifest_profile_schema_and_set_registered_environment() {
    let project = Scratch::new(Some(&verify_manifest("strict")));
    let output = project.run(
        &["config", "--json"],
        &[
            ("MUMEI_TASK_ID", "json-task"),
            ("MUMEI_UNKNOWN_CONFIG", "not-registered"),
        ],
    );
    let payload = json_output(&output);

    assert!(output.status.success(), "{}", stderr(&output));
    let root_keys = payload
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        root_keys,
        BTreeSet::from(["env", "manifest_path", "verify_profile"])
    );
    assert_eq!(
        payload["manifest_path"],
        project.root().join("mumei.toml").display().to_string()
    );
    let profile = &payload["verify_profile"];
    assert_eq!(profile["name"], "strict");
    assert_eq!(profile["source"], "mumei.toml");
    assert_eq!(
        profile["enables"],
        serde_json::json!([
            "--strict-imports",
            "--strict-array-types",
            "--warn-fragment"
        ])
    );
    let profile_keys = profile
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert_eq!(profile_keys, BTreeSet::from(["enables", "name", "source"]));
    let env = payload["env"].as_array().expect("env array");
    let task_id = env
        .iter()
        .find(|entry| entry["name"] == "MUMEI_TASK_ID")
        .expect("registered task ID");
    assert!(task_id["set"].as_bool().expect("set boolean"));
    assert_eq!(task_id["value"], "json-task");
    assert_eq!(task_id["default"], serde_json::Value::Null);
    assert!(task_id["description"].is_string());
    let entry_keys = task_id
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        entry_keys,
        BTreeSet::from([
            "default",
            "description",
            "name",
            "sensitive",
            "set",
            "value"
        ])
    );
    assert!(env.iter().all(|entry| {
        entry["set"].as_bool().is_some()
            && entry["sensitive"].as_bool().is_some()
            && if entry["sensitive"] == true {
                entry.get("value").is_none()
            } else {
                entry["value"].is_string() || entry["value"].is_null()
            }
            && (entry["default"].is_string() || entry["default"].is_null())
            && entry["description"].is_string()
    }));
    assert!(env
        .iter()
        .all(|entry| entry["name"] != "MUMEI_UNKNOWN_CONFIG"));
    assert!(env
        .iter()
        .all(|entry| entry["name"] != "MUMEI_SPEC_METADATA"));
}

#[test]
fn json_without_all_hides_unset_registered_variables() {
    let project = Scratch::new(None);
    let output = project.run(&["config", "--json"], &[]);
    let payload = json_output(&output);

    assert!(output.status.success(), "{}", stderr(&output));
    let env = payload["env"].as_array().expect("env array");
    assert!(env.iter().all(|entry| entry["name"] != "MUMEI_TASK_ID"));
    assert_eq!(payload["manifest_path"], Value::Null);
    assert_eq!(payload["verify_profile"], Value::Null);
}

#[test]
fn json_all_includes_unset_variables_with_null_values() {
    let project = Scratch::new(None);
    let output = project.run(&["config", "--json", "--all"], &[]);
    let payload = json_output(&output);

    assert!(output.status.success(), "{}", stderr(&output));
    let env = payload["env"].as_array().expect("env array");
    let task_id = env
        .iter()
        .find(|entry| entry["name"] == "MUMEI_TASK_ID")
        .expect("unset registered task ID");
    assert!(!task_id["set"].as_bool().expect("set boolean"));
    assert_eq!(task_id["value"], Value::Null);
    assert_eq!(task_id["default"], Value::Null);
}

#[test]
fn config_long_help_explains_manifest_only_profile_source() {
    let project = Scratch::new(None);
    let output = project.run(&["config", "--help"], &[]);
    let help = stdout(&output);

    assert!(output.status.success());
    assert!(help.contains("per-invocation"));
    assert!(help.contains("only reports the manifest source"));
}
