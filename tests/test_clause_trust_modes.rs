use mumei_core::parser::{self, ClauseKind, ClauseMode, ClauseTrustMode, Item};
use mumei_core::proof_cert;
use mumei_core::resolver;
use mumei_core::trust_boundary::{classify_trust_boundaries, TrustBoundaryKind};
use mumei_core::verification::ModuleEnv;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::{Command, Output};

fn verify(name: &str, source: &str) -> (PathBuf, Output, serde_json::Value) {
    let dir = std::env::temp_dir().join(format!(
        "mumei_clause_trust_modes_{}_{}_{}",
        std::process::id(),
        name,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).expect("create fixture directory");
    let fixture = dir.join(format!("{name}.mm"));
    std::fs::write(&fixture, source).expect("write fixture");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg(&fixture)
        .arg("--json")
        .arg("--report-dir")
        .arg(&dir)
        .arg("--disable-spurious-detection")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run mumei verify");
    let report = std::fs::read(dir.join("report.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(serde_json::Value::Null);
    (dir, output, report)
}

fn output_text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn remove_fixture(dir: PathBuf) {
    std::fs::remove_dir_all(dir).expect("remove fixture directory");
}

#[test]
fn assumed_ensures_are_reported_and_assumed_at_call_sites() {
    let (dir, output, report) = verify(
        "assumed_ensures",
        r#"
atom weak_post() -> i64
requires: true;
ensures assume: result > 100;
body: 0;
"#,
    );
    let text = output_text(&output);
    assert!(output.status.success(), "{text}");
    assert_eq!(report["ensures_outcomes"][0]["clause"], "result > 100");
    assert_eq!(report["ensures_outcomes"][0]["outcome"], "assumed");
    assert!(report["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|diagnostic| diagnostic
            .as_str()
            .is_some_and(|message| message.contains("result > 100"))));
    remove_fixture(dir);

    let (dir, caller, _) = verify(
        "assumed_ensures_caller",
        r#"
atom weak_post() -> i64
requires: true;
ensures assume: result > 100;
body: 0;

atom caller() -> i64
requires: true;
ensures: result > 100;
body: weak_post();
"#,
    );
    let text = output_text(&caller);
    assert!(caller.status.success(), "{text}");
    remove_fixture(dir);
}

#[test]
fn checked_ensures_are_proved_but_not_assumed_at_call_sites() {
    let (dir, output, _) = verify(
        "checked_ensures_caller",
        r#"
atom zero() -> i64
requires: true;
ensures check: result >= 0;
body: 0;

atom caller() -> i64
requires: true;
ensures: result >= 0;
body: zero();
"#,
    );
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &std::fs::read(dir.join("report.json")).expect("read report")
        )
        .unwrap()["failed_clause"],
        "result >= 0"
    );
    remove_fixture(dir);
}

#[test]
fn assumed_requires_allow_the_call_and_remain_available_to_the_body() {
    let (dir, output, _) = verify(
        "assumed_requires",
        r#"
atom positive(x: i64) -> i64
requires assume: x > 0;
ensures: result > 0;
body: x;

atom caller() -> i64
requires: true;
ensures: result > 0;
body: positive(-1);
"#,
    );
    let text = output_text(&output);
    assert!(output.status.success(), "{text}");
    remove_fixture(dir);
}

#[test]
fn checked_requires_are_neither_assumed_in_the_body_nor_skipped_at_calls() {
    let (dir, body_failure, report) = verify(
        "checked_requires_body",
        r#"
atom positive(x: i64) -> i64
requires check: x > 0;
ensures: result > 0;
body: x;
"#,
    );
    let text = output_text(&body_failure);
    assert!(!body_failure.status.success(), "{text}");
    assert_eq!(report["failed_clause"], "result > 0");
    remove_fixture(dir);

    let (dir, caller_failure, _) = verify(
        "checked_requires_call",
        r#"
atom guarded(x: i64) -> i64
requires check: x > 0;
ensures: true;
body: 0;

atom caller() -> i64
requires: true;
ensures: true;
body: guarded(-1);
"#,
    );
    let text = output_text(&caller_failure);
    assert!(!caller_failure.status.success(), "{text}");
    assert!(
        text.contains("precondition (requires) not satisfied at call site"),
        "{text}"
    );
    remove_fixture(dir);
}

#[test]
fn ordinary_and_assumed_duplicate_requires_keep_one_call_site_check() {
    let (dir, output, _) = verify(
        "duplicate_requires",
        r#"
atom guarded(x: i64) -> i64
requires: x > 0;
requires assume: x > 0;
ensures: true;
body: x;

atom caller() -> i64
requires: true;
ensures: true;
body: guarded(-1);
"#,
    );
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert!(
        text.contains("precondition (requires) not satisfied at call site"),
        "{text}"
    );
    remove_fixture(dir);
}

#[test]
fn multi_conjunct_assumed_ensures_report_each_conjunct() {
    let (dir, output, report) = verify(
        "multi_assumed_ensures",
        r#"
atom constant() -> i64
requires: true;
ensures assume: result > 1 && result < 3;
body: 0;
"#,
    );
    let text = output_text(&output);
    assert!(output.status.success(), "{text}");
    assert_eq!(report["ensures_outcomes"].as_array().unwrap().len(), 2);
    assert_eq!(report["ensures_outcomes"][0]["outcome"], "assumed");
    assert_eq!(report["ensures_outcomes"][1]["outcome"], "assumed");
    remove_fixture(dir);
}

#[test]
fn assumed_ensures_label_is_present_in_report_and_warning() {
    let (dir, output, report) = verify(
        "labeled_assumption",
        r#"
atom constant() -> i64
requires: true;
ensures assume "ffi result": result > 100;
body: 0;
"#,
    );
    let text = output_text(&output);
    assert!(output.status.success(), "{text}");
    assert_eq!(report["ensures_outcomes"][0]["label"], "ffi result");
    assert!(report["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(
            |diagnostic| diagnostic.as_str().is_some_and(
                |message| message.contains("ffi result") && message.contains("result > 100")
            )
        ));
    remove_fixture(dir);
}

#[test]
fn clause_mode_hashes_change_without_changing_develop_no_mode_content_hash() {
    let items = parser::parse_module(include_str!("fixtures/bitvec_backward_compat.mm"));
    let atom = items
        .iter()
        .find_map(|item| match item {
            Item::Atom(atom) if atom.name == "bc_add" => Some(atom),
            _ => None,
        })
        .expect("bc_add fixture atom");
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/bitvec_backward_compat.golden.json"))
            .expect("parse backward-compatibility golden");
    let expected_content_hash = golden["atoms"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "bc_add")
        .expect("bc_add golden")["content_hash"]
        .as_str()
        .expect("content hash");
    assert_eq!(
        proof_cert::compute_atom_content_hash_v2(atom),
        expected_content_hash
    );

    let mut with_mode = atom.clone();
    with_mode.clause_modes.push(ClauseMode {
        kind: ClauseKind::Requires,
        clause: atom.requires.clone(),
        mode: ClauseTrustMode::Assume,
    });
    assert_ne!(
        proof_cert::compute_atom_content_hash_v2(atom),
        proof_cert::compute_atom_content_hash_v2(&with_mode)
    );
    assert_ne!(
        resolver::compute_atom_hash(atom),
        resolver::compute_atom_hash(&with_mode)
    );
    let module_env = ModuleEnv::new();
    assert_ne!(
        resolver::compute_proof_hash(atom, &module_env),
        resolver::compute_proof_hash(&with_mode, &module_env)
    );
    assert_ne!(
        resolver::compute_contract_hash(atom),
        resolver::compute_contract_hash(&with_mode)
    );

    let callee = parser::item::parse_atom_from_source(
        r#"
atom callee() -> i64
requires: true;
ensures: result >= 0;
body: 0;
"#,
    );
    let caller = parser::item::parse_atom_from_source(
        r#"
atom caller() -> i64
requires: true;
ensures: true;
body: callee();
"#,
    );
    let mut plain_env = ModuleEnv::new();
    plain_env.register_atom(&callee);
    plain_env.register_dependencies("caller", HashSet::from(["callee".to_string()]));
    let plain_caller_hash = resolver::compute_proof_hash(&caller, &plain_env);
    let mut mode_callee = callee.clone();
    mode_callee.clause_modes.push(ClauseMode {
        kind: ClauseKind::Ensures,
        clause: "result >= 0".to_string(),
        mode: ClauseTrustMode::Check,
    });
    let mut mode_env = ModuleEnv::new();
    mode_env.register_atom(&mode_callee);
    mode_env.register_dependencies("caller", HashSet::from(["callee".to_string()]));
    assert_ne!(
        plain_caller_hash,
        resolver::compute_proof_hash(&caller, &mode_env)
    );
}

#[test]
fn certificate_records_assumed_clauses() {
    let atom = parser::item::parse_atom_from_source(
        r#"
atom trusted_post(x: i64) -> i64
requires assume: x > 0;
ensures assume: result > 100;
body: 0;
"#,
    );
    let atoms = [&atom];
    let results = HashMap::from([(
        atom.name.clone(),
        ("unsat".to_string(), "verified".to_string()),
    )]);
    let certificate = proof_cert::generate_certificate(
        "clause_trust_modes.mm",
        &atoms,
        &results,
        &ModuleEnv::new(),
        None,
        None,
        None,
    );

    assert_eq!(
        certificate.atoms[0].assumed_clauses,
        vec![
            "requires: x > 0".to_string(),
            "ensures: result > 100".to_string()
        ]
    );
    assert_eq!(
        classify_trust_boundaries(&atom, &[]),
        vec![TrustBoundaryKind::AssumedClause]
    );
}
