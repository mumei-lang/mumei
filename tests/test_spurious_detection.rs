use std::collections::HashMap;

use mumei_core::parser::{Atom, Param, Span, TrustLevel};
use mumei_core::verification::{
    detect_uninterpreted_symbols, detect_unused_hypotheses, validate_counterexample, ModuleEnv,
};

fn base_atom(name: &str) -> Atom {
    Atom {
        name: name.to_string(),
        type_params: Vec::new(),
        where_bounds: Vec::new(),
        params: Vec::new(),
        trace_id: None,
        spec_metadata: std::collections::HashMap::new(),
        requires: "true".to_string(),
        forall_constraints: Vec::new(),
        ensures: "result >= 0".to_string(),
        body_expr: "0".to_string(),
        consumed_params: Vec::new(),
        resources: Vec::new(),
        is_async: false,
        trust_level: TrustLevel::Verified,
        max_unroll: None,
        invariant: None,
        effects: Vec::new(),
        return_type: None,
        span: Span::default(),
        effect_pre: HashMap::new(),
        effect_post: HashMap::new(),
    }
}

fn param(name: &str, type_name: &str) -> Param {
    Param {
        name: name.to_string(),
        type_name: Some(type_name.to_string()),
        type_ref: None,
        is_ref: false,
        is_ref_mut: false,
        fn_contract_requires: None,
        fn_contract_ensures: None,
    }
}

#[test]
fn test_validated_counterexample() {
    let module_env = ModuleEnv::new();
    let mut atom = base_atom("validated_counterexample");
    atom.params = vec![param("x", "i64")];
    atom.requires = "x >= 0".to_string();
    atom.ensures = "result > 10".to_string();
    atom.body_expr = "x + 1".to_string();

    let model = HashMap::from([("x".to_string(), 0)]);
    let validation = validate_counterexample(&atom, &model, &module_env);

    assert!(validation.is_valid);
    assert_eq!(validation.validation_status, "validated");
    assert_eq!(validation.failed_constraints, vec!["ensures: result > 10"]);
    assert!(validation.symbol_provenance.is_empty());
}

#[test]
fn test_spurious_candidate_uninterpreted_symbol() {
    let module_env = ModuleEnv::new();
    let mut atom = base_atom("spurious_candidate");
    atom.params = vec![param("x", "i64")];
    atom.ensures = "mystery(x) > 0".to_string();

    let model = HashMap::from([("x".to_string(), 0)]);
    let validation = validate_counterexample(&atom, &model, &module_env);
    let symbols = detect_uninterpreted_symbols(&atom, &model, &module_env);

    assert!(!validation.is_valid);
    assert_eq!(validation.validation_status, "spurious_candidate");
    assert!(symbols.iter().any(|symbol| {
        symbol.symbol_name == "mystery" && symbol.source == "uninterpreted_function"
    }));
}

#[test]
fn test_unused_hypothesis_detection() {
    let module_env = ModuleEnv::new();
    let mut atom = base_atom("bounded");
    atom.requires = "x > 0".to_string();
    atom.invariant = Some("x < 10".to_string());
    atom.effect_pre
        .insert("Account".to_string(), "Open".to_string());

    let report = detect_unused_hypotheses(&atom, &["requires:bounded".to_string()], &module_env);

    assert!(report.unused_requires.is_empty());
    assert_eq!(report.unused_invariants, vec!["x < 10"]);
    assert_eq!(report.unused_effect_constraints, vec!["Account=Open"]);
}

#[test]
fn test_minimal_constraint_set() {
    let module_env = ModuleEnv::new();
    let atom = base_atom("minimal_core");
    let core = vec![
        "requires:minimal_core".to_string(),
        "ensures:minimal_core".to_string(),
    ];

    let report = detect_unused_hypotheses(&atom, &core, &module_env);

    assert_eq!(report.minimal_constraint_set, core);
}

#[test]
fn test_cli_exposes_spurious_detection_flag() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let output = std::process::Command::new(bin)
        .arg("verify")
        .arg("--help")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run mumei verify --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--enable-spurious-detection"));
    assert!(stdout.contains("--disable-spurious-detection"));
}

#[test]
fn test_cli_spurious_detection_flags_conflict() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let output = std::process::Command::new(bin)
        .arg("verify")
        .arg("--enable-spurious-detection")
        .arg("--disable-spurious-detection")
        .arg("tests/test_contracts.mm")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run mumei verify with conflicting flags");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot be used with") || stderr.contains("conflict"));
}
