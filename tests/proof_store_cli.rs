use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "mumei-proof-store-{name}-{}-{id}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn project(&self, name: &str, source: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("main.mm"), source).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(project: &Path, store: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command
        .arg("verify")
        .args(args)
        .arg("main.mm")
        .current_dir(project)
        .env("MUMEI_PROOF_STORE", store);
    command.output().unwrap()
}

fn text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn basic_module(ensures: &str) -> String {
    format!(
        "atom add(a: i64, b: i64) -> i64\n    requires: true;\n    ensures: {ensures};\n    body: a + b;\n"
    )
}

fn assert_not_reused(name: &str, baseline: &str, changed: &str, extra: &[&str]) {
    let scratch = Scratch::new(name);
    let store = scratch.0.join("store");
    let first = scratch.project("first", baseline);
    let second = scratch.project("second", changed);
    let initial = run(&first, &store, &["--proof-store"]);
    assert!(initial.status.success(), "{}\n{}", name, text(&initial));
    let mut args = vec!["--proof-store"];
    args.extend_from_slice(extra);
    let candidate = run(&second, &store, &args);
    assert!(
        !text(&candidate).contains("reused from proof store"),
        "{name}\n{}",
        text(&candidate)
    );
}

fn json_files(root: &Path) -> Vec<PathBuf> {
    fn visit(path: &Path, result: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(&path, result);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                result.push(path);
            }
        }
    }
    let mut result = Vec::new();
    visit(root, &mut result);
    result
}

#[test]
fn identical_modules_reuse_proof_and_certificate_provenance() {
    let scratch = Scratch::new("reuse");
    let store = scratch.0.join("store");
    let source = basic_module("result == a + b");
    let first = scratch.project("first", &source);
    let second = scratch.project("second", &source);

    let verified = run(&first, &store, &["--proof-store", "--proof-cert"]);
    assert!(verified.status.success(), "{}", text(&verified));
    assert!(
        text(&verified).contains("'add': verified"),
        "{}",
        text(&verified)
    );

    let reused = run(&second, &store, &["--proof-store", "--proof-cert"]);
    assert!(reused.status.success(), "{}", text(&reused));
    assert!(
        text(&reused).contains("skipped (reused from proof store)"),
        "{}",
        text(&reused)
    );
    let cert: Value =
        serde_json::from_slice(&fs::read(second.join("main.proof.json")).unwrap()).unwrap();
    assert_eq!(cert["atoms"][0]["reused_from_store"], true);
    assert_eq!(cert["atoms"][0]["z3_check_result"], "unsat");
    assert_eq!(cert["atoms"][0]["status"], "verified");

    let project_hit = run(&second, &store, &["--proof-store"]);
    assert!(project_hit.status.success(), "{}", text(&project_hit));
    assert!(
        text(&project_hit).contains("skipped (unchanged, cached)"),
        "{}",
        text(&project_hit)
    );
    assert!(
        !text(&project_hit).contains("ignored:"),
        "{}",
        text(&project_hit)
    );
}

#[test]
fn the_environment_path_does_not_enable_the_store_by_default() {
    let scratch = Scratch::new("disabled");
    let store = scratch.0.join("store");
    let project = scratch.project("project", &basic_module("result == a + b"));
    let output = run(&project, &store, &[]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(!store.exists());
}

#[test]
fn manifest_enables_store_for_verify_and_build() {
    let scratch = Scratch::new("manifest");
    let store = scratch.0.join("store");
    let verify_project = scratch.project("verify", &basic_module("result == a + b"));
    fs::write(
        verify_project.join("mumei.toml"),
        "[package]\nname = \"store-test\"\nversion = \"0.1.0\"\n[proof]\nstore = true\n",
    )
    .unwrap();
    let verified = run(&verify_project, &store, &[]);
    assert!(verified.status.success(), "{}", text(&verified));
    assert!(!json_files(&store).is_empty());

    let build_project = scratch.project("build", &basic_module("result == a + b + 0"));
    fs::write(
        build_project.join("mumei.toml"),
        "[package]\nname = \"store-build-test\"\nversion = \"0.1.0\"\n[proof]\nstore = true\n",
    )
    .unwrap();
    let built = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["build", "main.mm", "--emit", "proof-cert"])
        .current_dir(&build_project)
        .env("MUMEI_PROOF_STORE", &store)
        .output()
        .unwrap();
    assert!(built.status.success(), "{}", text(&built));
    assert!(json_files(&store).len() >= 2);
}

#[test]
fn cache_disabled_build_warns_and_does_not_access_store() {
    let scratch = Scratch::new("build-cache-off");
    let store = scratch.0.join("store");
    let project = scratch.project("project", &basic_module("result == a + b"));
    fs::write(
        project.join("mumei.toml"),
        "[package]\nname = \"store-test\"\nversion = \"0.1.0\"\n[proof]\ncache = false\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["build", "main.mm", "--emit", "proof-cert", "--proof-store"])
        .current_dir(&project)
        .env("MUMEI_PROOF_STORE", &store)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output));
    assert!(text(&output).contains("proof cache is disabled"));
    assert!(!store.exists());
}

#[test]
fn changed_contract_does_not_reuse_another_projects_proof() {
    let scratch = Scratch::new("changed-contract");
    let store = scratch.0.join("store");
    let first = scratch.project("first", &basic_module("result == a + b"));
    let second = scratch.project("second", &basic_module("result == a + b + 0"));
    let initial = run(&first, &store, &["--proof-store"]);
    assert!(initial.status.success(), "{}", text(&initial));
    let changed = run(&second, &store, &["--proof-store"]);
    assert!(changed.status.success(), "{}", text(&changed));
    assert!(
        !text(&changed).contains("reused from proof store"),
        "{}",
        text(&changed)
    );
}

#[test]
fn callee_contract_refined_type_effect_and_bitvector_changes_do_not_reuse() {
    let callee = "atom f(a: i64) -> i64\n    requires: true;\n    ensures: result == a;\n    body: a;\natom g(a: i64) -> i64\n    requires: true;\n    ensures: result == a;\n    body: f(a);\n";
    let changed_callee = callee.replacen("ensures: result == a;", "ensures: result == a + 0;", 1);
    assert_not_reused("callee-ensures", callee, &changed_callee, &[]);

    let refined = "type Positive = i64 where v >= 0;\natom positive(x: Positive) -> Positive\n    requires: true;\n    ensures: result >= 0;\n    body: x;\n";
    let changed_refinement = refined.replace("v >= 0", "v > 0");
    assert_not_reused("refined-type", refined, &changed_refinement, &[]);

    let source = basic_module("result == a + b");
    assert_not_reused("bitvector", &source, &source, &["--bitvec-i64"]);

    let effects = r#"effect File
    states: [Closed, Open];
    initial: Closed;
    transition open: Closed -> Open;
    transition close: Open -> Closed;
atom f(x: i64) -> i64
    effects: [File];
    effect_pre: { File: Closed };
    effect_post: { File: Open };
    requires: true;
    ensures: result == x;
    body: { perform File.open(x); x };
atom g(x: i64) -> i64
    effects: [File];
    effect_pre: { File: Closed };
    effect_post: { File: Open };
    requires: true;
    ensures: result == x;
    body: { f(x); x };
"#;
    let changed_effect = effects.replace(
        "atom f(x: i64) -> i64\n    effects: [File];\n    effect_pre: { File: Closed };\n    effect_post: { File: Open };",
        "atom f(x: i64) -> i64\n    effects: [File];\n    effect_pre: { File: Closed };\n    effect_post: { File: Closed };",
    );
    let scratch = Scratch::new("effect-post");
    let store = scratch.0.join("store");
    let first = scratch.project("first", effects);
    let second = scratch.project("second", &changed_effect);
    let initial = run(&first, &store, &["--proof-store"]);
    assert!(initial.status.success(), "{}", text(&initial));
    let changed = run(&second, &store, &["--proof-store"]);
    assert!(
        !text(&changed).contains("'g': skipped (reused from proof store)"),
        "{}",
        text(&changed)
    );
}

#[test]
fn failed_and_trusted_atoms_are_not_written() {
    let scratch = Scratch::new("ineligible");
    let store = scratch.0.join("store");
    let failed = scratch.project(
        "failed",
        "atom bad(a: i64) -> i64\n    requires: true;\n    ensures: result == 0;\n    body: 1;\n",
    );
    let failure = run(&failed, &store, &["--proof-store"]);
    assert!(!failure.status.success(), "{}", text(&failure));
    assert!(json_files(&store).is_empty());

    let trusted = scratch.project(
        "trusted",
        "trusted atom trusted_add(a: i64, b: i64) -> i64\n    requires: true;\n    ensures: result == a + b;\n    body: a + b;\n",
    );
    let trusted_result = run(&trusted, &store, &["--proof-store"]);
    assert!(trusted_result.status.success(), "{}", text(&trusted_result));
    assert!(json_files(&store).is_empty());
}

#[test]
fn invalid_records_warn_and_fall_back_to_z3() {
    type Mutation = (&'static str, fn(&mut Value));
    let mutations: [Mutation; 6] = [
        ("json", |_| {}),
        ("entry", |value| {
            value["entry"]["timestamp"] = "tampered".into()
        }),
        ("wrong-key", |value| value["key"] = "00".repeat(32).into()),
        ("z3-version", |value| {
            value["key_inputs"]["z3_version"] = "other-z3".into()
        }),
        ("policy", |value| {
            value["key_inputs"]["proof_hash"] = "other-policy".into()
        }),
        ("failed", |value| value["entry"]["result"] = "failed".into()),
    ];
    for (name, mutate) in mutations {
        let scratch = Scratch::new(name);
        let store = scratch.0.join("store");
        let source = basic_module("result == a + b");
        let first = scratch.project("first", &source);
        let second = scratch.project("second", &source);
        let seed = run(&first, &store, &["--proof-store"]);
        assert!(seed.status.success(), "{}", text(&seed));
        let entry = json_files(&store).pop().unwrap();
        let original = fs::read(&entry).unwrap();
        if name == "json" {
            fs::write(&entry, b"{").unwrap();
        } else {
            let mut value: Value = serde_json::from_slice(&original).unwrap();
            mutate(&mut value);
            fs::write(&entry, serde_json::to_vec(&value).unwrap()).unwrap();
        }
        let fallback = run(&second, &store, &["--proof-store"]);
        assert!(fallback.status.success(), "{}", text(&fallback));
        assert!(text(&fallback).contains("ignored:"), "{}", text(&fallback));
        assert!(
            text(&fallback).contains("'add': verified"),
            "{}",
            text(&fallback)
        );
        assert_eq!(json_files(&store).len(), 1);
    }
}

#[test]
fn proof_store_stats_prune_invalid_and_rejects_empty_selector() {
    let scratch = Scratch::new("stats-prune");
    let store = scratch.0.join("store");
    let project = scratch.project("project", &basic_module("result == a + b"));
    let verified = run(&project, &store, &["--proof-store"]);
    assert!(verified.status.success(), "{}", text(&verified));
    fs::create_dir_all(store.join("v1/aa")).unwrap();
    fs::write(store.join("v1/aa/invalid.json"), b"{").unwrap();

    let stats = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["proof-store", "stats", "--dir"])
        .arg(&store)
        .args(["--json"])
        .output()
        .unwrap();
    assert!(stats.status.success(), "{}", text(&stats));
    let json: Value = serde_json::from_slice(&stats.stdout).unwrap();
    assert_eq!(json["entries"], 2);
    assert!(json["total_bytes"].as_u64().unwrap() > 0);
    assert_eq!(json["invalid"], 1);

    let no_selector = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["proof-store", "prune", "--dir"])
        .arg(&store)
        .output()
        .unwrap();
    assert_eq!(no_selector.status.code(), Some(4));

    let pruned = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["proof-store", "prune", "--dir"])
        .arg(&store)
        .arg("--invalid")
        .output()
        .unwrap();
    assert!(pruned.status.success(), "{}", text(&pruned));
    assert_eq!(json_files(&store).len(), 1);

    let old = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["proof-store", "prune", "--dir"])
        .arg(&store)
        .args(["--older-than-days", "0"])
        .output()
        .unwrap();
    assert!(old.status.success(), "{}", text(&old));
    assert!(json_files(&store).is_empty());
}
