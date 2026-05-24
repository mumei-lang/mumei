use std::collections::HashMap;
use std::process::Command;

use mumei_core::parser::{Atom, Param, RefinedType, Span, TrustLevel};
use mumei_core::verification::{
    check_generated_assignment, generator_from_refinement, run_property_based_test,
    shrink_counterexample, synthesize_input_generators, GeneratedValue, InputGenerator, ModuleEnv,
    PropertyBasedTestConfig,
};

fn base_atom(name: &str) -> Atom {
    Atom {
        name: name.to_string(),
        type_params: Vec::new(),
        where_bounds: Vec::new(),
        params: Vec::new(),
        trace_id: None,
        spec_metadata: HashMap::new(),
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
        return_type: Some("i64".to_string()),
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

fn refined(name: &str, base_type: &str, operand: &str, predicate: &str) -> RefinedType {
    RefinedType {
        name: name.to_string(),
        _base_type: base_type.to_string(),
        operand: operand.to_string(),
        predicate_raw: predicate.to_string(),
        span: Span::default(),
    }
}

#[test]
fn synthesizes_nat_generator_from_refinement_type() {
    let generator = generator_from_refinement(&refined("Nat", "i64", "v", "v >= 0"));

    assert_eq!(generator.min, 0);
    assert_eq!(generator.max, 100);
    assert!(generator
        .boundary_values()
        .contains(&GeneratedValue::Int(0)));
}

#[test]
fn synthesizes_bounded_generator_from_refinement_type() {
    let generator = generator_from_refinement(&refined("Small", "i64", "v", "v >= -3 && v <= 7"));

    assert_eq!(generator.min, -3);
    assert_eq!(generator.max, 7);
    assert!(generator
        .boundary_values()
        .contains(&GeneratedValue::Int(7)));
}

#[test]
fn synthesizes_array_generator_from_parameter_type() {
    let module_env = ModuleEnv::new();
    let mut atom = base_atom("array_input");
    atom.params = vec![param("xs", "[i64]")];

    let generators = synthesize_input_generators(&atom, &module_env);
    let assignments = generators.boundary_assignments();

    assert!(assignments.iter().any(|assignment| matches!(
        assignment.get("xs"),
        Some(GeneratedValue::Array(values)) if values.is_empty()
    )));
    assert!(assignments.iter().any(|assignment| matches!(
        assignment.get("xs"),
        Some(GeneratedValue::Array(values)) if !values.is_empty()
    )));
}

#[test]
fn property_based_validation_detects_and_shrinks_counterexample() {
    let module_env = ModuleEnv::new();
    let mut atom = base_atom("bad_abs");
    atom.params = vec![param("x", "i64")];
    atom.requires = "true".to_string();
    atom.ensures = "result >= 0".to_string();
    atom.body_expr = "x".to_string();

    let config = PropertyBasedTestConfig {
        test_count: 32,
        max_shrink_steps: 64,
        seed: 123,
        include_boundary_values: false,
    };
    let result = run_property_based_test(&atom, &module_env, &config);

    assert_eq!(result.status, "failed");
    assert!(result.shrink_steps > 0);
    let counterexample = result
        .shrunk_counterexample
        .expect("expected shrunk counterexample");
    assert!(matches!(
        counterexample.get("x"),
        Some(GeneratedValue::Int(value)) if *value < 0 && value.abs() <= 2
    ));
}

#[test]
fn shrinking_reduces_counterexample_size_by_at_least_ninety_percent() {
    let module_env = ModuleEnv::new();
    let mut atom = base_atom("bad_abs");
    atom.params = vec![param("x", "i64")];
    atom.ensures = "result >= 0".to_string();
    atom.body_expr = "x".to_string();
    let generators = synthesize_input_generators(&atom, &module_env);
    let config = PropertyBasedTestConfig::default();
    let mut initial = HashMap::new();
    initial.insert("x".to_string(), GeneratedValue::Int(-100));

    assert!(check_generated_assignment(&atom, &module_env, &initial).is_some());
    let (shrunk, _steps) = shrink_counterexample(&atom, &module_env, &generators, initial, &config);
    let GeneratedValue::Int(value) = shrunk.get("x").expect("missing x") else {
        panic!("expected integer counterexample");
    };

    assert!(
        value.abs() <= 10,
        "expected at least 90% shrink from 100, got {value}"
    );
}

#[test]
fn cli_exposes_property_based_validation_flags() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let output = Command::new(bin)
        .arg("verify")
        .arg("--help")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run mumei verify --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--property-based-test"));
    assert!(stdout.contains("--property-based-test-count"));
    assert!(stdout.contains("--property-based-test-seed"));
}
