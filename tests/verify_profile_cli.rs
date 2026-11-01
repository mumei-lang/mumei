use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const LOOP_SOURCE: &str = r#"
atom add(x: i64, y: i64) -> i64
requires: true;
ensures: result == x + y;
body: x + y;

atom count_to_n(n: i64) -> i64
requires: n >= 0;
ensures: result == n;
body: {
    let i = 0;
    while i < n invariant: true {
        i = i + 1;
    };
    i
};
"#;

struct Scratch(PathBuf);

impl Scratch {
    fn new(source: &str, manifest: Option<&str>) -> Self {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("mumei-verify-profile-{}-{id}", std::process::id()));
        fs::create_dir_all(&directory).expect("create scratch project");
        fs::write(directory.join("main.mm"), source).expect("write input");
        if let Some(manifest) = manifest {
            fs::write(directory.join("mumei.toml"), manifest).expect("write manifest");
        }
        Self(directory)
    }

    fn root(&self) -> &Path {
        &self.0
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_mumei"))
            .args(args)
            .current_dir(self.root())
            .output()
            .expect("run mumei")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn manifest(profile: &str) -> String {
    format!(
        "[package]\nname = \"verify-profile-fixture\"\nversion = \"0.1.0\"\n\n[verify]\nprofile = \"{profile}\"\n"
    )
}

fn json_output(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid stdout JSON ({error}):\n{}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn default_and_cli_strict_profile_cover_untyped_array_access() {
    let project = Scratch::new(include_str!("test_untyped_array_access.mm"), None);

    let baseline = project.run(&["verify", "main.mm", "--json"]);
    assert_eq!(
        baseline.status.code(),
        Some(0),
        "{}\n{}",
        String::from_utf8_lossy(&baseline.stdout),
        stderr(&baseline)
    );
    let _baseline_json = json_output(&baseline);
    assert!(!stderr(&baseline).contains("verify profile:"));

    let strict = project.run(&["verify", "main.mm", "--profile", "strict", "--json"]);
    assert_eq!(
        strict.status.code(),
        Some(1),
        "{}\n{}",
        String::from_utf8_lossy(&strict.stdout),
        stderr(&strict)
    );
    let _strict_json = json_output(&strict);
    assert!(stderr(&strict).contains("verify profile: strict (from --profile)"));
}

#[test]
fn manifest_strict_profile_rejects_untyped_array_access() {
    let project = Scratch::new(
        include_str!("test_untyped_array_access.mm"),
        Some(&manifest("strict")),
    );

    let output = project.run(&["verify", "main.mm", "--json"]);

    assert_eq!(output.status.code(), Some(1));
    let _json = json_output(&output);
    assert!(stderr(&output).contains("verify profile: strict (from mumei.toml)"));
}

#[test]
fn audit_profile_enables_vacuity_checks_and_loop_suggestions() {
    let project = Scratch::new(LOOP_SOURCE, None);
    let output = project.run(&[
        "verify",
        "main.mm",
        "--profile",
        "audit",
        "--detect-loops",
        "--suggest-cegis",
        "--json",
    ]);
    let json = json_output(&output);
    let suggestions = json["cegis_suggestions"]
        .as_array()
        .expect("loop suggestions in JSON");

    assert!(!suggestions.is_empty(), "{}", json);
    assert!(stderr(&output).contains("Vacuity check passed for 'add'"));
    assert!(stderr(&output).contains("verify profile: audit (from --profile)"));
}

#[test]
fn audit_profile_detects_loops_without_explicit_loop_flags() {
    let project = Scratch::new(LOOP_SOURCE, None);
    let output = project.run(&["verify", "main.mm", "--profile", "audit"]);

    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("loop(s) may need invariant strengthening"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn explicit_loop_flags_remain_enabled_with_strict_profile() {
    let project = Scratch::new(LOOP_SOURCE, None);
    let output = project.run(&[
        "verify",
        "main.mm",
        "--profile",
        "strict",
        "--detect-loops",
        "--suggest-cegis",
        "--json",
    ]);
    let json = json_output(&output);

    assert!(json["cegis_suggestions"].as_array().is_some());
}

#[test]
fn cli_strict_overrides_manifest_audit_without_audit_outputs() {
    let source = format!(
        "{}\n{LOOP_SOURCE}",
        include_str!("test_untyped_array_access.mm")
    );
    let project = Scratch::new(&source, Some(&manifest("audit")));
    let output = project.run(&["verify", "main.mm", "--profile", "strict", "--json"]);
    let json = json_output(&output);

    assert_eq!(output.status.code(), Some(1));
    assert!(json.get("cegis_suggestions").is_none(), "{json}");
    assert!(!stderr(&output).contains("Vacuity check passed"));
    assert!(stderr(&output).contains("verify profile: strict (from --profile)"));
}

#[test]
fn invalid_manifest_profile_returns_usage_error_with_context() {
    let project = Scratch::new(
        "atom empty() -> i64 body: { 0 }\n",
        Some(&manifest("relaxed")),
    );
    let output = project.run(&["verify", "main.mm", "--json"]);
    let message = stderr(&output);

    assert_eq!(output.status.code(), Some(2));
    assert!(message.contains(&project.root().join("mumei.toml").display().to_string()));
    assert!(message.contains("relaxed"));
    assert!(message.contains("strict, audit"));
}
