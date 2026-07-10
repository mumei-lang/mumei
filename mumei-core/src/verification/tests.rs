use super::executor::{compute_solver_config_fingerprint, verify, VerificationMetrics};
use super::fragment::*;
use super::module_env::*;
use super::nlae_reporter::*;
use super::profiler::{self, ConstraintProfile, IncrementalProfiler};
use super::support::*;
use super::translator::*;
use super::types::*;
use crate::hir::lower_atom_to_hir_with_env;
use crate::parser::{
    parse_module, Atom, Effect, EffectDef, EffectTransition, Expr, ImplDef, Item, Op, Param,
    Quantifier, QuantifierType, Span, TraitDef, TraitMethod, TrustLevel,
};
use crate::resolver::compute_contract_hash;
use crate::verification::{generate_contract_manifest, verify_contract_integrity};
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use z3::ast::{Array, Ast, Bool, Int, String as Z3String};
use z3::{Config, Context, SatResult, Solver};

fn test_atom(
    name: &str,
    params: Vec<Param>,
    requires: &str,
    ensures: &str,
    body_expr: &str,
    return_type: Option<&str>,
) -> Atom {
    Atom {
        name: name.to_string(),
        type_params: vec![],
        where_bounds: vec![],
        params,
        trace_id: None,
        spec_metadata: std::collections::HashMap::new(),
        requires: requires.to_string(),
        forall_constraints: vec![],
        ensures: ensures.to_string(),
        body_expr: body_expr.to_string(),
        consumed_params: vec![],
        resources: vec![],
        is_async: false,
        trust_level: TrustLevel::Verified,
        max_unroll: None,
        invariant: None,
        effects: vec![],
        return_type: return_type.map(str::to_string),
        span: Span::default(),
        effect_pre: HashMap::new(),
        effect_post: HashMap::new(),
    }
}

fn test_param(name: &str, type_name: Option<&str>) -> Param {
    Param {
        name: name.to_string(),
        type_name: type_name.map(str::to_string),
        type_ref: type_name.map(crate::parser::parse_type_ref),
        is_ref: false,
        is_ref_mut: false,
        fn_contract_requires: None,
        fn_contract_ensures: None,
    }
}

#[test]
fn test_contract_hash_computation_is_deterministic() {
    let mut atom = test_atom(
        "bounded_add",
        vec![test_param("a", Some("i64")), test_param("b", Some("i64"))],
        "a >= 0 && b >= 0",
        "result == a + b",
        "a + b",
        Some("i64"),
    );
    atom.effects.push(Effect::simple("Pure"));

    let hash1 = compute_contract_hash(&atom);
    let hash2 = compute_contract_hash(&atom);

    assert_eq!(hash1, hash2);
    assert_eq!(hash1.len(), 64);
}

#[test]
fn test_contract_hash_ignores_body_changes() {
    let atom = test_atom(
        "bounded_add",
        vec![test_param("a", Some("i64")), test_param("b", Some("i64"))],
        "a >= 0 && b >= 0",
        "result == a + b",
        "a + b",
        Some("i64"),
    );
    let mut changed_body = atom.clone();
    changed_body.body_expr = "b + a".to_string();

    assert_eq!(
        compute_contract_hash(&atom),
        compute_contract_hash(&changed_body)
    );
}

#[test]
fn test_contract_hash_detects_quantifier_changes() {
    let mut atom = test_atom(
        "array_non_negative",
        vec![test_param("n", Some("i64"))],
        "n >= 0 && true",
        "result == n",
        "n",
        Some("i64"),
    );
    atom.forall_constraints.push(Quantifier {
        q_type: QuantifierType::ForAll,
        var: "i".to_string(),
        start: "0".to_string(),
        end: "n".to_string(),
        condition: "arr[i] >= 0".to_string(),
    });

    let mut mutated = atom.clone();
    mutated.forall_constraints[0].condition = "arr[i] > 0".to_string();

    assert_ne!(
        compute_contract_hash(&atom),
        compute_contract_hash(&mutated)
    );
}

#[test]
fn test_contract_hash_detects_temporal_effect_contract_changes() {
    let mut atom = test_atom(
        "process_order",
        vec![],
        "true",
        "result >= 0",
        "0",
        Some("i64"),
    );
    atom.effects.push(Effect::simple("Order"));
    atom.effect_pre
        .insert("Order".to_string(), "Created".to_string());
    atom.effect_post
        .insert("Order".to_string(), "Shipped".to_string());

    let mut mutated = atom.clone();
    mutated
        .effect_post
        .insert("Order".to_string(), "Cancelled".to_string());

    assert_ne!(
        compute_contract_hash(&atom),
        compute_contract_hash(&mutated)
    );
}

#[test]
fn test_contract_hash_detects_negated_effect_changes() {
    let mut atom = test_atom("pure_step", vec![], "true", "result == 0", "0", Some("i64"));
    atom.effects.push(Effect {
        name: "IO".to_string(),
        params: vec![],
        span: Span::default(),
        negated: true,
    });

    let mut mutated = atom.clone();
    mutated.effects[0].negated = false;

    assert_ne!(
        compute_contract_hash(&atom),
        compute_contract_hash(&mutated)
    );
}

#[test]
fn test_contract_hash_avoids_field_boundary_collisions() {
    let left = test_atom("ab", vec![], "c", "d", "0", Some("i64"));
    let right = test_atom("a", vec![], "bc", "d", "0", Some("i64"));

    assert_ne!(compute_contract_hash(&left), compute_contract_hash(&right));
}

#[test]
fn test_contract_hash_covers_parsed_contract_clauses() {
    let source = r#"
atom apply(x: i64, f: atom_ref(i64) -> i64)
    requires: x >= 0;
    ensures: result >= 0;
    contract(f): requires: x >= 0, ensures: result >= 0;
    body: call(f, x);
"#;
    let items = parse_module(source);
    let atom = items
        .iter()
        .find_map(|item| match item {
            Item::Atom(atom) => Some(atom.clone()),
            _ => None,
        })
        .expect("expected atom");
    let mut mutated = atom.clone();
    mutated.params[1].fn_contract_ensures = Some("result > 0".to_string());

    assert_ne!(
        compute_contract_hash(&atom),
        compute_contract_hash(&mutated)
    );
}

#[test]
fn test_contract_mutation_detection() {
    let atom = test_atom(
        "bounded_add",
        vec![test_param("a", Some("i64")), test_param("b", Some("i64"))],
        "a >= 0 && b >= 0",
        "result == a + b",
        "a + b",
        Some("i64"),
    );
    let mut module_env = ModuleEnv::new();
    module_env.register_atom(&atom);
    let manifest = generate_contract_manifest(&module_env);

    let mut mutated = atom.clone();
    mutated.ensures = "result >= a".to_string();
    let err = verify_contract_integrity(&mutated, &manifest).unwrap_err();

    assert!(matches!(err, MumeiError::ContractMutation { .. }));
}

#[test]
fn test_contract_integrity_verification_allows_implementation_changes() {
    let atom = test_atom(
        "bounded_add",
        vec![test_param("a", Some("i64")), test_param("b", Some("i64"))],
        "a >= 0 && b >= 0",
        "result == a + b",
        "a + b",
        Some("i64"),
    );
    let mut module_env = ModuleEnv::new();
    module_env.register_atom(&atom);
    let manifest = generate_contract_manifest(&module_env);

    let mut changed_body = atom.clone();
    changed_body.body_expr = "(a + b)".to_string();

    assert!(verify_contract_integrity(&changed_body, &manifest).is_ok());
}

#[test]
fn test_decidable_fragment_warning_detects_unbounded_array_access() {
    let atom = test_atom(
        "read_unbounded",
        vec![
            test_param("arr", Some("[i64]")),
            test_param("i", Some("i64")),
        ],
        "i >= 0",
        "result == arr[i]",
        "arr[i]",
        Some("i64"),
    );
    let module_env = ModuleEnv::new();

    let tags = detect_logic_fragment_tags(&atom, &module_env);
    assert!(tags.iter().any(|tag| tag == "array_without_bounds"));
    assert!(outside_decidable_fragment_warning(&atom, &module_env)
        .is_some_and(|warning| warning.contains("outside_decidable_fragment")));
}

#[test]
fn test_decidable_fragment_warning_accepts_explicit_array_bounds() {
    let atom = test_atom(
        "read_bounded",
        vec![
            test_param("arr", Some("[i64]")),
            test_param("i", Some("i64")),
        ],
        "i >= 0 && i < len(arr)",
        "result == arr[i]",
        "arr[i]",
        Some("i64"),
    );
    let module_env = ModuleEnv::new();

    let tags = detect_logic_fragment_tags(&atom, &module_env);
    assert!(!tags.iter().any(|tag| tag == "array_without_bounds"));
}

#[test]
fn test_decidable_fragment_warning_detects_complex_temporal_effect() {
    let mut atom = test_atom("temporal", vec![], "true", "result == 0", "0", Some("i64"));
    atom.effects.push(Effect::simple("Protocol"));

    let mut module_env = ModuleEnv::new();
    module_env.register_effect(&EffectDef {
        name: "Protocol".to_string(),
        params: vec![],
        constraint: None,
        includes: vec![],
        refinement: None,
        parent: vec![],
        span: Span::default(),
        states: vec!["S0", "S1", "S2", "S3", "S4"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        transitions: (0..9)
            .map(|idx| EffectTransition {
                operation: format!("op{idx}"),
                from_state: "S0".to_string(),
                to_state: "S1".to_string(),
            })
            .collect(),
        initial_state: Some("S0".to_string()),
    });

    let tags = detect_logic_fragment_tags(&atom, &module_env);
    assert!(tags.iter().any(|tag| tag == "complex_temporal_effect"));
}

#[test]
fn test_array_store_promotes_int_literal_for_real_arrays() {
    let atom = test_atom(
        "store_real_array",
        vec![test_param("arr", Some("[f64]"))],
        "len(arr) >= 1",
        "result == 42.0",
        "{ arr[0] = 42; arr[0] }",
        Some("f64"),
    );
    let mut module_env = ModuleEnv::new();
    register_builtin_traits(&mut module_env);
    module_env.register_atom(&atom);
    let hir = lower_atom_to_hir_with_env(&atom, Some(&module_env));

    verify(&hir, Path::new("."), &module_env).unwrap();
}

#[test]
fn test_array_store_updates_subsequent_selects_for_ffi_buffers() {
    let atom = test_atom(
        "ffi_buffer_store_roundtrip",
        vec![test_param("buf", Some("[i64]"))],
        "len(buf) >= 1",
        "result == 7",
        "{ buf[0] = 7; buf[0] }",
        Some("i64"),
    );
    let mut module_env = ModuleEnv::new();
    register_builtin_traits(&mut module_env);
    module_env.register_atom(&atom);
    let hir = lower_atom_to_hir_with_env(&atom, Some(&module_env));

    verify(&hir, Path::new("."), &module_env).unwrap();
}

#[test]
fn test_array_store_rejects_out_of_bounds_ffi_buffers() {
    let atom = test_atom(
        "ffi_buffer_store_oob",
        vec![test_param("buf", Some("[i64]"))],
        "len(buf) >= 1",
        "result == 7",
        "{ buf[1] = 7; 7 }",
        Some("i64"),
    );
    let mut module_env = ModuleEnv::new();
    register_builtin_traits(&mut module_env);
    module_env.register_atom(&atom);
    let hir = lower_atom_to_hir_with_env(&atom, Some(&module_env));

    let err = verify(&hir, Path::new("."), &module_env).unwrap_err();
    assert!(err.to_string().contains("Potential Out-of-Bounds store"));
}

#[test]
fn test_executor_marks_symbolic_exponent_ensures_unverifiable() {
    let atom = test_atom(
        "pow_unverifiable",
        vec![test_param("x", Some("i64")), test_param("y", Some("i64"))],
        "x >= 0",
        "result == x**y && result == x",
        "x",
        Some("i64"),
    );
    let mut module_env = ModuleEnv::new();
    register_builtin_traits(&mut module_env);
    module_env.register_atom(&atom);
    let hir = lower_atom_to_hir_with_env(&atom, Some(&module_env));

    let output_dir = std::env::temp_dir().join(format!(
        "mumei-unverifiable-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&output_dir).unwrap();

    let err = verify(&hir, &output_dir, &module_env).unwrap_err();
    assert!(err.to_string().contains("Unverifiable"));

    let report = std::fs::read_to_string(output_dir.join("report.json")).unwrap();
    let report_json: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(report_json["status"], "unverifiable");
    let diagnostics = report_json["diagnostics"].as_array().unwrap();
    assert!(diagnostics.iter().any(|diag| {
        diag.as_str()
            .map(|diag| {
                diag.starts_with("Skipped unsupported Z3 clause: ensures clause 'result == x")
                    && diag.contains("Unsupported exponentiation")
            })
            .unwrap_or(false)
    }));
}

// ---- constraint_to_natural_language tests ----

#[test]
fn test_constraint_to_natural_language_range() {
    let result =
        constraint_to_natural_language("age", "BoundedAge", "age >= 0 && age <= 120", "150");
    assert!(result.contains("age"));
    assert!(result.contains("150"));
}

#[test]
fn test_constraint_to_natural_language_modulo() {
    let result = constraint_to_natural_language("n", "EvenInt", "n % 2 == 0", "3");
    assert!(result.contains("multiple") || result.contains("倍数"));
    assert!(result.contains("3"));
}

#[test]
fn test_constraint_to_natural_language_enum() {
    let result = constraint_to_natural_language(
        "status",
        "StatusCode",
        "status == 1 || status == 2 || status == 3",
        "5",
    );
    assert!(result.contains("one of") || result.contains("のいずれか"));
    assert!(result.contains("5"));
}

#[test]
fn test_constraint_to_natural_language_negation() {
    let result = constraint_to_natural_language("x", "NonZero", "x != 0", "0");
    assert!(result.contains("not") || result.contains("ありません"));
    assert!(result.contains("0"));
}

#[test]
fn test_constraint_to_natural_language_string_constraint() {
    let result = constraint_to_natural_language(
        "path",
        "SafePath",
        "starts_with(path, \"/tmp/\")",
        "/etc/passwd",
    );
    assert!(result.contains("starts_with") || result.contains("start"));
}

#[test]
fn test_constraint_to_natural_language_comparison() {
    let result = constraint_to_natural_language("x", "Positive", "x > 0", "-1");
    assert!(result.contains("greater than") || result.contains("より大きい"));
}

#[test]
fn test_constraint_to_natural_language_fallback() {
    let result = constraint_to_natural_language("x", "Custom", "some_complex_pred(x)", "42");
    assert!(result.contains("x"));
    assert!(result.contains("42"));
}

// ---- suggestion_for_failure_type tests ----

#[test]
fn test_suggestion_for_failure_type_division() {
    let suggestion = suggestion_for_failure_type(FAILURE_DIVISION_BY_ZERO);
    assert!(suggestion.contains("divisor") || suggestion.contains("0"));
}

#[test]
fn test_suggestion_for_failure_type_linearity() {
    let suggestion = suggestion_for_failure_type(FAILURE_LINEARITY_VIOLATED);
    assert!(
        suggestion.contains("Clone")
            || suggestion.contains("clone")
            || suggestion.contains("クローン")
    );
}

#[test]
fn test_suggestion_for_failure_type_effect() {
    let suggestion = suggestion_for_failure_type("effect_not_allowed");
    assert!(suggestion.contains("effect") || suggestion.contains("エフェクト"));
}

#[test]
fn test_suggestion_for_failure_type_postcondition() {
    let suggestion = suggestion_for_failure_type(FAILURE_POSTCONDITION_VIOLATED);
    assert!(!suggestion.is_empty());
}

#[test]
fn test_suggestion_for_failure_type_precondition() {
    let suggestion = suggestion_for_failure_type(FAILURE_PRECONDITION_VIOLATED);
    assert!(!suggestion.is_empty());
}

// ---- build_division_by_zero_feedback tests ----

#[test]
fn test_build_division_by_zero_feedback() {
    let feedback = build_division_by_zero_feedback("10", "0");
    assert_eq!(feedback["failure_type"], FAILURE_DIVISION_BY_ZERO);
    assert!(feedback["counter_example"]["dividend"].as_str().is_some());
    assert!(feedback["counter_example"]["divisor"].as_str().is_some());
}

// ---- build_linearity_feedback tests ----

#[test]
fn test_build_linearity_feedback() {
    let violations = vec!["Variable 'x' used after being consumed".to_string()];
    let span = Span {
        file: "test.mm".to_string(),
        line: 10,
        col: 1,
        len: 5,
    };
    let feedback = build_linearity_feedback("test_atom", &violations, &span);
    assert_eq!(feedback["failure_type"], FAILURE_LINEARITY_VIOLATED);
    assert!(feedback["violations"].is_array());
    assert_eq!(feedback["atom"], "test_atom");
}

// ---- build_effect_feedback tests ----

#[test]
fn test_build_effect_feedback() {
    let allowed = vec!["FileRead".to_string()];
    let missing = vec!["FileWrite".to_string()];
    let feedback = build_effect_feedback("test_atom", "FileWrite", &allowed, &missing);
    assert_eq!(feedback["failure_type"], "effect_not_allowed");
    assert_eq!(feedback["attempted_effect"], "FileWrite");
    assert!(feedback["allowed_effects"].is_array());
    assert!(feedback["missing_effects"].is_array());
}

// ---- try_match_comparison tests ----

#[test]
fn test_try_match_comparison() {
    let result = try_match_comparison("x > 10", "x", "Bounded", "5");
    assert!(result.is_some());
    let msg = result.unwrap();
    assert!(msg.contains("greater than") || msg.contains("より大きい"));
    assert!(msg.contains("5"));
}

#[test]
fn test_try_match_comparison_lte() {
    let result = try_match_comparison("x <= 100", "x", "Capped", "150");
    assert!(result.is_some());
    let msg = result.unwrap();
    assert!(msg.contains("at most") || msg.contains("以下"));
}

// ---- SecurityPolicy tests ----

#[test]
fn test_security_policy_new() {
    let policy = SecurityPolicy::new();
    assert!(policy.allowed_effects.is_empty());
}

#[test]
fn test_security_policy_allow_and_check() {
    let mut policy = SecurityPolicy::new();
    policy.allow_effect(
        "FileRead",
        vec![(
            "path".to_string(),
            "starts_with(path, \"/tmp/\")".to_string(),
        )],
    );
    assert!(policy.is_effect_allowed("FileRead"));
    assert!(!policy.is_effect_allowed("FileWrite"));
}

#[test]
fn test_security_policy_get_constraints() {
    let mut policy = SecurityPolicy::new();
    policy.allow_effect(
        "HttpGet",
        vec![(
            "url".to_string(),
            "starts_with(url, \"https://\")".to_string(),
        )],
    );
    let constraints = policy.get_constraints("HttpGet");
    assert_eq!(constraints.len(), 1);
    assert_eq!(constraints[0].0, "url");
}

#[test]
fn test_security_policy_check_param_constraint() {
    let mut policy = SecurityPolicy::new();
    policy.allow_effect(
        "FileRead",
        vec![(
            "path".to_string(),
            "starts_with(path, \"/tmp/\")".to_string(),
        )],
    );
    assert!(policy
        .check_param_constraint("FileRead", "path", Some("/tmp/data.txt"))
        .is_ok());
    assert!(policy
        .check_param_constraint("FileRead", "path", Some("/etc/passwd"))
        .is_err());
}

#[test]
fn test_substitute_method_calls_uses_dynamic_nested_passes() {
    let mut method_bodies = HashMap::new();
    method_bodies.insert("wrap".to_string(), "inner(x)".to_string());
    method_bodies.insert("inner".to_string(), "x + 1".to_string());

    let mut method_params = HashMap::new();
    method_params.insert("wrap".to_string(), vec!["x".to_string()]);
    method_params.insert("inner".to_string(), vec!["x".to_string()]);

    let expanded = substitute_method_calls("wrap(a) == inner(b)", &method_bodies, &method_params);
    assert!(expanded.contains("((a) + 1)"));
    assert!(expanded.contains("((b) + 1)"));
}

#[test]
fn test_contains_method_call_rejects_partial_names() {
    assert!(contains_method_call("add(a, b)", "add"));
    assert!(!contains_method_call("safe_add(a, b)", "add"));
    assert!(!contains_method_call("adder(a, b)", "add"));
}

// ---- evaluate_string_constraint tests ----

#[test]
fn test_evaluate_string_constraint_starts_with() {
    assert!(evaluate_string_constraint(
        "starts_with(path, \"/tmp/\")",
        "path",
        "/tmp/data.txt"
    ));
    assert!(!evaluate_string_constraint(
        "starts_with(path, \"/tmp/\")",
        "path",
        "/etc/passwd"
    ));
}

#[test]
fn test_evaluate_string_constraint_ends_with() {
    assert!(evaluate_string_constraint(
        "ends_with(file, \".mm\")",
        "file",
        "test.mm"
    ));
    assert!(!evaluate_string_constraint(
        "ends_with(file, \".mm\")",
        "file",
        "test.rs"
    ));
}

#[test]
fn test_evaluate_string_constraint_contains() {
    assert!(evaluate_string_constraint(
        "contains(url, \"api\")",
        "url",
        "https://api.example.com"
    ));
    assert!(!evaluate_string_constraint(
        "contains(url, \"api\")",
        "url",
        "https://example.com"
    ));
}

// ---- parse_tracking_label tests ----

#[test]
fn test_parse_tracking_label_requires() {
    let result = parse_tracking_label("track_requires");
    assert!(result.is_some());
    let sl = result.unwrap();
    assert_eq!(sl.constraint_type, "requires");
    assert!(sl.param.is_none());
    assert!(sl.type_name.is_none());
    assert!(sl.field.is_none());
    assert!(sl.description.contains("requires"));
    assert!(sl.description.contains("前提条件"));
}

#[test]
fn test_parse_tracking_label_refined_type() {
    let result = parse_tracking_label("track_refined_type_n::Nat");
    assert!(result.is_some());
    let sl = result.unwrap();
    assert_eq!(sl.constraint_type, "refined_type");
    assert_eq!(sl.param.as_deref(), Some("n"));
    assert_eq!(sl.type_name.as_deref(), Some("Nat"));
    assert!(sl.field.is_none());
    assert!(sl.description.contains("n"));
    assert!(sl.description.contains("Nat"));
    assert!(sl.description.contains("精緻型"));
}

#[test]
fn test_parse_tracking_label_refined_type_underscore_var() {
    let result = parse_tracking_label("track_refined_type_my_var::Pos");
    assert!(result.is_some());
    let sl = result.unwrap();
    assert_eq!(sl.constraint_type, "refined_type");
    assert_eq!(sl.param.as_deref(), Some("my_var"));
    assert_eq!(sl.type_name.as_deref(), Some("Pos"));
    assert!(sl.description.contains("my_var"));
    assert!(sl.description.contains("Pos"));
    assert!(sl.description.contains("精緻型"));
}

#[test]
fn test_parse_tracking_label_struct_field() {
    let result = parse_tracking_label("track_struct_field_p::age");
    assert!(result.is_some());
    let sl = result.unwrap();
    assert_eq!(sl.constraint_type, "struct_field");
    assert_eq!(sl.param.as_deref(), Some("p"));
    assert_eq!(sl.field.as_deref(), Some("age"));
    assert!(sl.type_name.is_none());
    assert!(sl.description.contains("p"));
    assert!(sl.description.contains("age"));
    assert!(sl.description.contains("構造体フィールド"));
}

#[test]
fn test_parse_tracking_label_struct_field_underscore() {
    let result = parse_tracking_label("track_struct_field_my_obj::my_field");
    assert!(result.is_some());
    let sl = result.unwrap();
    assert_eq!(sl.constraint_type, "struct_field");
    assert_eq!(sl.param.as_deref(), Some("my_obj"));
    assert_eq!(sl.field.as_deref(), Some("my_field"));
    assert!(sl.description.contains("my_obj"));
    assert!(sl.description.contains("my_field"));
    assert!(sl.description.contains("構造体フィールド"));
}

#[test]
fn test_parse_tracking_label_quantifier() {
    let result = parse_tracking_label("track_quantifier_0");
    assert!(result.is_some());
    let sl = result.unwrap();
    assert_eq!(sl.constraint_type, "quantifier");
    assert!(sl.param.is_none());
    assert!(sl.description.contains("#0"));
    assert!(sl.description.contains("量子化"));
}

#[test]
fn test_parse_tracking_label_u64_nonneg() {
    let result = parse_tracking_label("track_u64_nonneg_x");
    assert!(result.is_some());
    let sl = result.unwrap();
    assert_eq!(sl.constraint_type, "u64_nonneg");
    assert_eq!(sl.param.as_deref(), Some("x"));
    assert!(sl.description.contains("x"));
    assert!(sl.description.contains("u64"));
}

#[test]
fn test_parse_tracking_label_unknown() {
    assert!(parse_tracking_label("__alive_x").is_none());
    assert!(parse_tracking_label("__borrowed_y").is_none());
    assert!(parse_tracking_label("random_label").is_none());
}

// ---- build_contradiction_feedback tests ----

#[test]
fn test_build_contradiction_feedback_with_constraints() {
    let constraints = vec![
        "Precondition (requires)".to_string(),
        "Refined type constraint: n (Nat)".to_string(),
    ];
    let raw = vec![
        "track_requires".to_string(),
        "track_refined_type_n::Nat".to_string(),
    ];
    let structured: Vec<StructuredLabel> = raw
        .iter()
        .filter_map(|label| parse_tracking_label(label))
        .collect();
    let feedback = build_contradiction_feedback("test_atom", &constraints, &raw, &structured, None);
    assert_eq!(feedback["failure_type"], FAILURE_INVARIANT_VIOLATED);
    assert_eq!(feedback["atom"], "test_atom");
    assert!(feedback["conflicting_constraints"].is_array());
    assert_eq!(
        feedback["conflicting_constraints"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(feedback["raw_unsat_core"].is_array());
    assert!(feedback["structured_unsat_core"].is_array());
    let suc = feedback["structured_unsat_core"].as_array().unwrap();
    assert_eq!(suc.len(), 2);
    assert_eq!(suc[0]["constraint_type"], "requires");
    assert_eq!(suc[1]["constraint_type"], "refined_type");
    assert_eq!(suc[1]["param"], "n");
    assert_eq!(suc[1]["type_name"], "Nat");
    assert!(feedback["explanation"]
        .as_str()
        .unwrap()
        .contains("contradictory"));
}

#[test]
fn test_build_contradiction_feedback_empty() {
    let feedback = build_contradiction_feedback("test_atom", &[], &[], &[], None);
    assert_eq!(feedback["atom"], "test_atom");
    assert!(feedback["conflicting_constraints"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(feedback["structured_unsat_core"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(feedback["explanation"]
        .as_str()
        .unwrap()
        .contains("could not be determined"));
}

#[test]
fn test_extract_minimal_unsat_core_simple() {
    let cfg = Config::new();
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);
    let a = Bool::new_const(&ctx, "A");
    let b = Bool::new_const(&ctx, "B");
    let track_a = Bool::new_const(&ctx, "track_a");
    let track_not_a = Bool::new_const(&ctx, "track_not_a");
    let track_b = Bool::new_const(&ctx, "track_b");

    solver.assert_and_track(&a, &track_a);
    solver.assert_and_track(&a.not(), &track_not_a);
    solver.assert_and_track(&b, &track_b);

    assert_eq!(solver.check(), SatResult::Unsat);

    let labels = vec![
        "track_a".to_string(),
        "track_not_a".to_string(),
        "track_b".to_string(),
    ];
    let minimal = extract_minimal_unsat_core(&solver, &labels, &ctx);

    assert_eq!(minimal.len(), 2);
    assert!(minimal.contains(&"track_a".to_string()));
    assert!(minimal.contains(&"track_not_a".to_string()));
    assert!(!minimal.contains(&"track_b".to_string()));
}

#[test]
fn test_extract_minimal_unsat_core_empty() {
    let cfg = Config::new();
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);

    let minimal = extract_minimal_unsat_core(&solver, &[], &ctx);
    assert!(minimal.is_empty());
}

#[test]
fn test_extract_minimal_unsat_core_single() {
    let cfg = Config::new();
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);

    let labels = vec!["track_a".to_string()];
    let minimal = extract_minimal_unsat_core(&solver, &labels, &ctx);

    assert_eq!(minimal, labels);
}

#[test]
fn test_extract_minimal_unsat_core_linear_matches_simple_case() {
    let cfg = Config::new();
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);
    let a = Bool::new_const(&ctx, "A");
    let b = Bool::new_const(&ctx, "B");
    let track_a = Bool::new_const(&ctx, "track_a");
    let track_not_a = Bool::new_const(&ctx, "track_not_a");
    let track_b = Bool::new_const(&ctx, "track_b");

    solver.assert_and_track(&a, &track_a);
    solver.assert_and_track(&a.not(), &track_not_a);
    solver.assert_and_track(&b, &track_b);

    let labels = vec![
        "track_a".to_string(),
        "track_not_a".to_string(),
        "track_b".to_string(),
    ];
    let minimal = extract_minimal_unsat_core_linear(&solver, &labels, &ctx);

    assert_eq!(minimal.len(), 2);
    assert!(minimal.contains(&"track_a".to_string()));
    assert!(minimal.contains(&"track_not_a".to_string()));
    assert!(!minimal.contains(&"track_b".to_string()));
}

#[test]
fn test_profiler_basic() {
    let cfg = Config::new();
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);

    let mut profiler = IncrementalProfiler::new(&solver, &ctx);

    let x = Int::new_const(&ctx, "profiler_x");
    let y = Int::new_const(&ctx, "profiler_y");

    solver.assert(&x._eq(&y));
    profiler.profile_assertion("constraint_1", Some("test.mm:10".to_string()));

    solver.assert(&x.ge(&Int::from_i64(&ctx, 0)));
    profiler.profile_assertion("constraint_2", Some("test.mm:11".to_string()));

    let heatmap = profiler.build_heatmap("test_atom", "test_timeout");

    assert_eq!(heatmap.atom_name, "test_atom");
    assert_eq!(heatmap.timeout_reason, "test_timeout");
    assert_eq!(heatmap.constraints.len(), 2);
    assert_eq!(heatmap.constraints[0].constraint_id, "constraint_1");
    assert_eq!(
        heatmap.constraints[1].source_location,
        Some("test.mm:11".to_string())
    );
}

#[test]
fn test_top_consumers_summary_orders_by_rlimit() {
    let constraints = vec![
        ConstraintProfile {
            constraint_id: "low".to_string(),
            rlimit_consumed: 1,
            time_ms: 0,
            source_location: None,
        },
        ConstraintProfile {
            constraint_id: "high".to_string(),
            rlimit_consumed: 10,
            time_ms: 0,
            source_location: None,
        },
    ];

    assert_eq!(
        profiler::top_consumers_summary(&constraints, 1),
        "high (10 rlimit)"
    );
}

#[test]
fn test_profiler_attributes_check_rlimit_to_recent_constraints() {
    let cfg = Config::new();
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);
    let mut profiler = IncrementalProfiler::new(&solver, &ctx);

    let x = Int::new_const(&ctx, "profiler_check_x");
    let y = Int::new_const(&ctx, "profiler_check_y");
    solver.assert(&x.gt(&Int::from_i64(&ctx, 0)));
    profiler.profile_assertion("constraint_1", None);
    solver.assert(&y._eq(&(x.clone() + 1_i64)));
    profiler.profile_assertion("constraint_2", None);

    let checkpoint = profiler.begin_check();
    assert_eq!(solver.check(), SatResult::Sat);
    profiler.end_check(checkpoint);

    let heatmap = profiler.build_heatmap("test_atom", "test_timeout");
    assert!(heatmap
        .constraints
        .iter()
        .all(|c| c.constraint_id != "solver_check"));
    assert!(
        heatmap
            .constraints
            .iter()
            .map(|constraint| constraint.rlimit_consumed)
            .sum::<u64>()
            <= heatmap.total_rlimit
    );
}

#[test]
fn test_build_contradiction_feedback_with_minimal_core() {
    let constraints = vec![
        "Precondition (requires)".to_string(),
        "Refined type constraint: n (Nat)".to_string(),
    ];
    let raw = vec![
        "track_requires".to_string(),
        "track_refined_type_n::Nat".to_string(),
        "track_quantifier_0".to_string(),
    ];
    let minimal = vec![
        "track_requires".to_string(),
        "track_refined_type_n::Nat".to_string(),
    ];
    let structured: Vec<StructuredLabel> = raw
        .iter()
        .filter_map(|label| parse_tracking_label(label))
        .collect();

    let feedback = build_contradiction_feedback_with_minimal_core(
        "test_atom",
        &constraints,
        &raw,
        &structured,
        &minimal,
    );

    assert_eq!(feedback["minimal_unsat_core"], json!(minimal));
    assert_eq!(feedback["minimal_core_size"], 2);
    assert_eq!(feedback["total_core_size"], 3);
    assert_eq!(feedback["reduction_ratio"], json!(2.0 / 3.0));
    assert!(feedback["suggestion"]
        .as_str()
        .unwrap()
        .contains("Minimal conflicting constraints"));
}

#[test]
fn test_build_contradiction_feedback_accepts_optional_minimal_core() {
    let raw = vec![
        "track_requires".to_string(),
        "track_refined_type_n::Nat".to_string(),
        "track_quantifier_0".to_string(),
    ];
    let minimal = vec![
        "track_requires".to_string(),
        "track_refined_type_n::Nat".to_string(),
    ];
    let structured: Vec<StructuredLabel> = raw
        .iter()
        .filter_map(|label| parse_tracking_label(label))
        .collect();
    let constraints: Vec<String> = structured
        .iter()
        .map(|label| label.description.clone())
        .collect();

    let feedback =
        build_contradiction_feedback("test_atom", &constraints, &raw, &structured, Some(&minimal));

    assert_eq!(feedback["minimal_unsat_core"], json!(minimal));
    assert_eq!(feedback["minimal_core_size"], 2);
    assert_eq!(feedback["total_core_size"], 3);
    assert_eq!(feedback["reduction_ratio"], json!(2.0 / 3.0));
}

// =========================================================================
// Task 0: Explosion Prevention Infrastructure tests
// =========================================================================

#[test]
fn test_constraint_budget_exceeded() {
    // Simulate constraint budget exceeded by setting a very low budget
    let ctx = z3::Context::new(&z3::Config::new());
    let count_cell = std::cell::Cell::new(0usize);
    let has_string_cell = std::cell::Cell::new(false);
    let module_env = ModuleEnv::new();

    let vc = VCtx {
        ctx: &ctx,
        module_env: &module_env,
        current_atom: None,
        linearity_ctx: None,
        effect_ctx: None,
        constraint_count: Some(&count_cell),
        constraint_budget: 5, // Very low budget
        has_string_constraints: Some(&has_string_cell),
        path_cond_stack: std::cell::RefCell::new(Vec::new()),
        profiler: None,
        ieee754_f64: false,
    };

    // Each call increments and checks
    for i in 0..5 {
        let result = check_constraint_budget(&vc, "test_atom");
        assert!(result.is_ok(), "Should succeed at count {}", i + 1);
    }

    // 6th call should exceed budget (count becomes 6 > 5)
    let result = check_constraint_budget(&vc, "test_atom");
    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("Constraint budget exceeded"));
    assert!(err_msg.contains("test_atom"));
    assert!(err_msg.contains("limit: 5"));
}

#[test]
fn test_constraint_budget_no_limit() {
    // When constraint_count is None, no budget checking occurs
    let ctx = z3::Context::new(&z3::Config::new());
    let module_env = ModuleEnv::new();

    let vc = VCtx {
        ctx: &ctx,
        module_env: &module_env,
        current_atom: None,
        linearity_ctx: None,
        effect_ctx: None,
        constraint_count: None,
        constraint_budget: DEFAULT_CONSTRAINT_BUDGET,
        has_string_constraints: None,
        path_cond_stack: std::cell::RefCell::new(Vec::new()),
        profiler: None,
        ieee754_f64: false,
    };

    // Should always succeed when no constraint tracking
    for _ in 0..100 {
        assert!(check_constraint_budget(&vc, "test_atom").is_ok());
    }
}

#[test]
fn test_verification_metrics_basic() {
    let mut metrics = VerificationMetrics::new("test_atom");

    assert_eq!(metrics.atom_name, "test_atom");
    assert!(metrics.phase_times.is_empty());
    assert_eq!(metrics.total_constraints, 0);
    assert_eq!(metrics.z3_check_time, std::time::Duration::ZERO);
    assert!(metrics.solver_config_fingerprint.is_empty());
    assert_eq!(metrics.task_id, None);
    assert_eq!(metrics.timeout_ms, 0);
    assert_eq!(metrics.cancel_reason, None);

    // Record some phases
    metrics.record_phase("Phase 1", std::time::Duration::from_millis(10));
    metrics.record_phase("Phase 2", std::time::Duration::from_millis(20));
    metrics.total_constraints = 42;
    metrics.z3_check_time = std::time::Duration::from_millis(5);

    assert_eq!(metrics.phase_times.len(), 2);
    assert_eq!(metrics.phase_times[0].0, "Phase 1");
    assert_eq!(
        metrics.phase_times[0].1,
        std::time::Duration::from_millis(10)
    );
    assert_eq!(metrics.phase_times[1].0, "Phase 2");
    assert_eq!(metrics.total_constraints, 42);
    assert_eq!(metrics.z3_check_time, std::time::Duration::from_millis(5));
}

#[test]
fn test_solver_config_fingerprint_changes_with_config() {
    let base = compute_solver_config_fingerprint(30000, true, false, false, true);
    assert_eq!(
        base,
        compute_solver_config_fingerprint(30000, true, false, false, true)
    );
    assert_ne!(
        base,
        compute_solver_config_fingerprint(10000, true, false, false, true)
    );
    assert_ne!(
        base,
        compute_solver_config_fingerprint(30000, false, false, false, true)
    );
    assert_ne!(
        base,
        compute_solver_config_fingerprint(30000, true, true, false, true)
    );
    assert_ne!(
        base,
        compute_solver_config_fingerprint(30000, true, false, true, true)
    );
    assert_ne!(
        base,
        compute_solver_config_fingerprint(30000, true, false, false, false)
    );
}

#[test]
fn test_has_string_constraints_flag() {
    // Test that the has_string_constraints flag can be set and read
    let cell = std::cell::Cell::new(false);
    assert!(!cell.get());
    cell.set(true);
    assert!(cell.get());
}

#[test]
fn test_default_constraint_budget_value() {
    assert_eq!(DEFAULT_CONSTRAINT_BUDGET, 1000);
}

// =========================================================================
// Task 4: Effect Parameter Z3 String Sort — Tests
// =========================================================================

#[test]
fn test_constant_path_ok() {
    // Constant path "/tmp/data.txt" should pass starts_with(path, "/tmp/") constraint
    assert!(check_constant_constraint(
        "/tmp/data.txt",
        "starts_with(path, \"/tmp/\")"
    ));
    assert!(check_constant_constraint(
        "/tmp/nested/file.log",
        "starts_with(path, \"/tmp/\")"
    ));
}

#[test]
fn test_constant_path_ng() {
    // Constant path "/etc/passwd" should fail starts_with(path, "/tmp/") constraint
    assert!(!check_constant_constraint(
        "/etc/passwd",
        "starts_with(path, \"/tmp/\")"
    ));
    assert!(!check_constant_constraint(
        "/var/log/syslog",
        "starts_with(path, \"/tmp/\")"
    ));
}

#[test]
fn test_z3_string_parse_constraint_starts_with() {
    // Test parse_constraint_to_z3_string with starts_with
    let cfg = z3::Config::new();
    let ctx = z3::Context::new(&cfg);
    let param = Z3String::new_const(&ctx, "path");

    // starts_with should produce a Bool constraint
    let result = parse_constraint_to_z3_string(&ctx, "starts_with(path, \"/tmp/\")", &param);
    assert!(result.is_some(), "starts_with constraint should parse");

    // ends_with should also work
    let result2 = parse_constraint_to_z3_string(&ctx, "ends_with(path, \".txt\")", &param);
    assert!(result2.is_some(), "ends_with constraint should parse");

    // contains should also work
    let result3 = parse_constraint_to_z3_string(&ctx, "contains(path, \"data\")", &param);
    assert!(result3.is_some(), "contains constraint should parse");

    // not_contains should also work
    let result4 = parse_constraint_to_z3_string(&ctx, "not_contains(path, \"..\")", &param);
    assert!(result4.is_some(), "not_contains constraint should parse");

    // Invalid constraint should return None
    let result5 = parse_constraint_to_z3_string(&ctx, "unknown_fn(path, \"x\")", &param);
    assert!(result5.is_none(), "unknown constraint should return None");
}

#[test]
fn test_z3_string_constraint_satisfiability() {
    // Test that Z3 String Sort constraints are satisfiable/unsatisfiable as expected
    let cfg = z3::Config::new();
    let ctx = z3::Context::new(&cfg);
    let solver = z3::Solver::new(&ctx);

    // Create a Z3 String variable
    let path = Z3String::new_const(&ctx, "path");

    // Assert: path starts with "/tmp/"
    let prefix = Z3String::from_str(&ctx, "/tmp/").unwrap();
    solver.assert(&prefix.prefix(&path));

    // Should be satisfiable (there exist strings starting with /tmp/)
    assert_eq!(solver.check(), z3::SatResult::Sat);

    // Now also assert: path starts with "/etc/"
    let prefix2 = Z3String::from_str(&ctx, "/etc/").unwrap();
    solver.assert(&prefix2.prefix(&path));

    // Should be unsatisfiable (can't start with both /tmp/ and /etc/)
    assert_eq!(solver.check(), z3::SatResult::Unsat);
}

#[test]
fn test_contains_constraint() {
    // Test contains constraint with Z3 String Sort
    let cfg = z3::Config::new();
    let ctx = z3::Context::new(&cfg);
    let solver = z3::Solver::new(&ctx);

    let path = Z3String::new_const(&ctx, "path");

    // Assert: path contains ".."
    let substr = Z3String::from_str(&ctx, "..").unwrap();
    let contains_dotdot = path.contains(&substr);
    // Assert NOT contains ".." (path traversal prevention)
    solver.assert(&contains_dotdot.not());

    // Also assert path starts with "/tmp/"
    let prefix = Z3String::from_str(&ctx, "/tmp/").unwrap();
    solver.assert(&prefix.prefix(&path));

    // Should be satisfiable: "/tmp/safe.txt" satisfies both
    assert_eq!(solver.check(), z3::SatResult::Sat);
}

#[test]
fn test_z3_string_performance() {
    // Test that String Sort constraint solving completes within reasonable time
    let start = std::time::Instant::now();

    let cfg = z3::Config::new();
    let ctx = z3::Context::new(&cfg);
    let solver = z3::Solver::new(&ctx);

    // Create multiple string variables with constraints
    for i in 0..10 {
        let name = format!("path_{}", i);
        let var = Z3String::new_const(&ctx, name.as_str());
        let prefix = Z3String::from_str(&ctx, "/tmp/").unwrap();
        solver.assert(&prefix.prefix(&var));
    }

    let result = solver.check();
    let elapsed = start.elapsed();

    assert_eq!(result, z3::SatResult::Sat);
    // Should solve within 500ms even with 10 string constraints
    assert!(
        elapsed.as_millis() < 500,
        "String Sort constraint solving took {}ms, expected < 500ms",
        elapsed.as_millis()
    );
}

// =========================================================================
// Compound && constraint tests
// =========================================================================

#[test]
fn test_check_constant_constraint_compound() {
    // Compound constraint: starts_with AND not_contains
    assert!(check_constant_constraint(
        "/tmp/data.txt",
        "starts_with(path, \"/tmp/\") && not_contains(path, \"..\")"
    ));
    // Path traversal should fail
    assert!(!check_constant_constraint(
        "/tmp/../etc/passwd",
        "starts_with(path, \"/tmp/\") && not_contains(path, \"..\")"
    ));
    // Wrong prefix should fail
    assert!(!check_constant_constraint(
        "/etc/passwd",
        "starts_with(path, \"/tmp/\") && not_contains(path, \"..\")"
    ));
}

#[test]
fn test_evaluate_string_constraint_compound() {
    assert!(evaluate_string_constraint(
        "starts_with(path, \"/tmp/\") && not_contains(path, \"..\")",
        "path",
        "/tmp/safe.txt"
    ));
    assert!(!evaluate_string_constraint(
        "starts_with(path, \"/tmp/\") && not_contains(path, \"..\")",
        "path",
        "/tmp/../etc/passwd"
    ));
}

#[test]
fn test_z3_compound_constraint_parse() {
    let cfg = z3::Config::new();
    let ctx = z3::Context::new(&cfg);
    let param = Z3String::new_const(&ctx, "path");

    // Compound constraint should parse successfully
    let result = parse_constraint_to_z3_string(
        &ctx,
        "starts_with(path, \"/tmp/\") && not_contains(path, \"..\")",
        &param,
    );
    assert!(result.is_some(), "compound constraint should parse");

    // Compound with unknown sub-constraint should fail (fail-closed)
    let result2 = parse_constraint_to_z3_string(
        &ctx,
        "starts_with(path, \"/tmp/\") && unknown_check(path, \"x\")",
        &param,
    );
    assert!(
        result2.is_none(),
        "compound with unknown sub-constraint should return None"
    );
}

// =========================================================================
// Plan 20: Temporal Effect Z3 Integration — Tests
// =========================================================================

#[test]
fn test_encode_effect_state_basic() {
    use crate::mir_analysis::EffectStateMachine;

    let sm = EffectStateMachine {
        effect_name: "FileIO".to_string(),
        states: vec![
            "Open".to_string(),
            "Reading".to_string(),
            "Closed".to_string(),
        ],
        transitions: std::collections::HashMap::new(),
        initial_state: "Closed".to_string(),
    };

    assert_eq!(encode_effect_state(&sm, "Open"), 0);
    assert_eq!(encode_effect_state(&sm, "Reading"), 1);
    assert_eq!(encode_effect_state(&sm, "Closed"), 2);
    assert_eq!(encode_effect_state(&sm, "Unknown"), -1);
}

#[test]
fn test_z3_conflicting_state_unsat() {
    // Two different states at the same merge point should be UNSAT.
    let cfg = z3::Config::new();
    let ctx = z3::Context::new(&cfg);
    let solver = z3::Solver::new(&ctx);

    let state_var = Int::new_const(&ctx, "__effect_state_FileIO_3");

    // Branch A says state = 0 (Open)
    solver.assert(&state_var._eq(&Int::from_i64(&ctx, 0)));
    // Branch B says state = 2 (Closed)
    solver.assert(&state_var._eq(&Int::from_i64(&ctx, 2)));
    // Valid range: 0..3
    solver.assert(&state_var.ge(&Int::from_i64(&ctx, 0)));
    solver.assert(&state_var.lt(&Int::from_i64(&ctx, 3)));

    assert_eq!(
        solver.check(),
        z3::SatResult::Unsat,
        "Different states at merge point should be UNSAT"
    );
}

#[test]
fn test_z3_conflicting_state_sat_same() {
    // Same state from both branches should be SAT.
    let cfg = z3::Config::new();
    let ctx = z3::Context::new(&cfg);
    let solver = z3::Solver::new(&ctx);

    let state_var = Int::new_const(&ctx, "__effect_state_FileIO_3");

    // Both branches say state = 1 (Reading)
    solver.assert(&state_var._eq(&Int::from_i64(&ctx, 1)));
    solver.assert(&state_var._eq(&Int::from_i64(&ctx, 1)));
    solver.assert(&state_var.ge(&Int::from_i64(&ctx, 0)));
    solver.assert(&state_var.lt(&Int::from_i64(&ctx, 3)));

    assert_eq!(
        solver.check(),
        z3::SatResult::Sat,
        "Same state from both branches should be SAT"
    );
}

// ---- split_compound_constraint tests ----

#[test]
fn test_split_compound_simple_and() {
    let parts = split_compound_constraint("a >= 0 && a <= 120");
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0], "a >= 0");
    assert_eq!(parts[1], "a <= 120");
}

#[test]
fn test_split_compound_with_nested_parens() {
    let parts = split_compound_constraint("(a > 0 && a < 10) && b > 0");
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0], "(a > 0 && a < 10)");
    assert_eq!(parts[1], "b > 0");
}

#[test]
fn test_split_compound_with_quoted_strings() {
    let parts =
        split_compound_constraint("starts_with(path, \"/tmp/\") && not_contains(path, \"..\")");
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0], "starts_with(path, \"/tmp/\")");
    assert_eq!(parts[1], "not_contains(path, \"..\")");
}

#[test]
fn test_split_compound_single() {
    let parts = split_compound_constraint("x > 0");
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0], "x > 0");
}

#[test]
fn test_split_compound_quoted_ampersand() {
    let parts = split_compound_constraint("contains(s, \"a && b\") && x > 0");
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0], "contains(s, \"a && b\")");
    assert_eq!(parts[1], "x > 0");
}

// ---- evaluate_sub_constraint tests ----

#[test]
fn test_evaluate_sub_constraint_starts_with_satisfied() {
    assert!(evaluate_sub_constraint(
        "starts_with(path, \"/tmp/\")",
        "/tmp/../etc/passwd"
    ));
}

#[test]
fn test_evaluate_sub_constraint_not_contains_violated() {
    assert!(!evaluate_sub_constraint(
        "not_contains(path, \"..\")",
        "/tmp/../etc/passwd"
    ));
}

#[test]
fn test_evaluate_sub_constraint_numeric_comparison() {
    assert!(evaluate_sub_constraint("v >= 0", "5"));
    assert!(!evaluate_sub_constraint("v >= 0", "-1"));
    assert!(evaluate_sub_constraint("v <= 120", "100"));
    assert!(!evaluate_sub_constraint("v <= 120", "150"));
}

// ---- compound constraint_to_natural_language tests ----

#[test]
fn test_constraint_to_natural_language_compound() {
    let result = constraint_to_natural_language(
        "path",
        "SafePath",
        "starts_with(path, \"/tmp/\") && not_contains(path, \"..\")",
        "/etc/passwd",
    );
    assert!(result.contains("[1/2]"));
    assert!(result.contains("[2/2]"));
    assert!(result.contains("AND"));
}

// ---- build_semantic_feedback with sub_constraints test ----

#[test]
fn test_build_semantic_feedback_sub_constraints() {
    use crate::parser::ast::Span;
    let mappings = vec![ConstraintMapping {
        param_name: "path".to_string(),
        type_name: Some("SafePath".to_string()),
        base_type: "Str".to_string(),
        predicate_raw: "starts_with(path, \"/tmp/\") && not_contains(path, \"..\")".to_string(),
        span: Span::default(),
    }];
    let ce = serde_json::json!({
        "path": "/tmp/../etc/passwd"
    });
    let dummy_atom = crate::parser::ast::Atom {
        name: "test_atom".to_string(),
        type_params: vec![],
        where_bounds: vec![],
        params: vec![],
        trace_id: None,
        spec_metadata: std::collections::HashMap::new(),
        requires: String::new(),
        forall_constraints: vec![],
        ensures: String::new(),
        body_expr: String::new(),
        consumed_params: vec![],
        resources: vec![],
        is_async: false,
        trust_level: crate::parser::ast::TrustLevel::Verified,
        max_unroll: None,
        invariant: None,
        effects: vec![],
        return_type: None,
        span: Span::default(),
        effect_pre: std::collections::HashMap::new(),
        effect_post: std::collections::HashMap::new(),
    };
    let feedback = build_semantic_feedback(
        &mappings,
        Some(&ce),
        &dummy_atom,
        "precondition_violated",
        None,
    )
    .unwrap();
    let vc_arr = feedback["violated_constraints"].as_array().unwrap();
    assert!(!vc_arr.is_empty());
    let vc = &vc_arr[0];
    assert!(vc.get("sub_constraints").is_some());
    let subs = vc["sub_constraints"].as_array().unwrap();
    assert_eq!(subs.len(), 2);
    assert_eq!(subs[0]["satisfied"], true);
    assert_eq!(subs[1]["satisfied"], false);
}

#[test]
fn test_build_loss_vector_returns_p9e_json_structure() {
    let mut atom = test_atom(
        "withdraw",
        vec![test_param("from", Some("i64"))],
        "true",
        "from_after == from - amount",
        "from + amount",
        Some("i64"),
    );
    atom.span = Span {
        file: "vault.mu".to_string(),
        line: 12,
        col: 1,
        len: 8,
    };
    let counterexample = json!({
        "from": 100,
        "to": 0,
        "amount": -50,
        "from_after": 150
    });

    let loss_vector = build_loss_vector(
        &atom,
        FAILURE_POSTCONDITION_VIOLATED,
        Some(&counterexample),
        &atom.span,
    );

    assert_eq!(loss_vector["status"], "verification_failed");
    assert_eq!(loss_vector["error_type"], FAILURE_POSTCONDITION_VIOLATED);
    assert_eq!(loss_vector["location"]["file"], "vault.mu");
    assert_eq!(loss_vector["location"]["line"], 12);
    assert_eq!(
        loss_vector["reconstruction_loss"]["violated_property"],
        "from_after == from - amount"
    );
    assert_eq!(
        loss_vector["reconstruction_loss"]["counter_example"]["amount"],
        -50
    );
    assert!(loss_vector["feedback_instruction"]
        .as_str()
        .unwrap()
        .contains("ensures"));
}

#[test]
fn test_build_loss_vector_enhances_feedback_for_self_correction() {
    let mut atom = test_atom(
        "withdraw",
        vec![test_param("amount", Some("i64"))],
        "true",
        "result >= 0",
        "amount",
        Some("i64"),
    );
    atom.span = Span {
        file: "vault.mu".to_string(),
        line: 12,
        col: 1,
        len: 8,
    };
    let counterexample = json!({"amount": -1});

    std::env::set_var(ENABLE_SELF_CORRECTION_ENV, "1");
    let loss_vector = build_loss_vector(
        &atom,
        FAILURE_POSTCONDITION_VIOLATED,
        Some(&counterexample),
        &atom.span,
    );
    std::env::remove_var(ENABLE_SELF_CORRECTION_ENV);

    let instruction = loss_vector["feedback_instruction"].as_str().unwrap();
    assert!(instruction.contains("Self-Correction Protocol"));
    assert!(instruction.contains("mumei verify --emit loss-vector"));
    assert!(instruction.contains("result >= 0"));
}

#[test]
fn test_is_reconstruction_loss_empty_detects_zero_loss_vector() {
    let empty = json!({
        "status": "verification_failed",
        "reconstruction_loss": {
            "violated_property": "result > 0",
            "counter_example": {}
        }
    });
    assert!(is_reconstruction_loss_empty(&empty));

    let non_empty = json!({
        "status": "verification_failed",
        "reconstruction_loss": {
            "violated_property": "result > 0",
            "counter_example": {"x": -1}
        }
    });
    assert!(!is_reconstruction_loss_empty(&non_empty));
}

#[test]
fn test_module_verification_report_serializes_loss_vector() {
    let loss_vector = json!({
        "status": "verification_failed",
        "error_type": FAILURE_POSTCONDITION_VIOLATED,
        "location": {"file": "main.mm", "line": 7},
        "reconstruction_loss": {
            "violated_property": "result > 10",
            "counter_example": {"x": 5}
        },
        "feedback_instruction": "Repair ensures."
    });
    let report = ModuleVerificationReport {
        cross_spec: None,
        decidable_fragment: None,
        loss_vector: Some(loss_vector.clone()),
    };

    let encoded = serde_json::to_value(report).expect("serialize module report");

    assert_eq!(encoded["loss_vector"], loss_vector);
}

// ---- build_contextual_suggestion tests ----

#[test]
fn test_contextual_suggestion_precondition_with_zero_counterexample() {
    let ce = json!({"b": "0"});
    let result = build_contextual_suggestion(FAILURE_PRECONDITION_VIOLATED, Some(&ce), None);
    assert!(
        result.contains("b != 0"),
        "should suggest b != 0 guard: {}",
        result
    );
    assert!(
        result.contains("requires"),
        "should mention requires: {}",
        result
    );
}

#[test]
fn test_contextual_suggestion_precondition_with_violated_constraint() {
    let ce = json!({"b": "0"});
    let unsat_core = json!([{"description": "b != 0"}]);
    let result =
        build_contextual_suggestion(FAILURE_PRECONDITION_VIOLATED, Some(&ce), Some(&unsat_core));
    assert!(
        result.contains("b != 0"),
        "should reference violated constraint: {}",
        result
    );
}

#[test]
fn test_contextual_suggestion_postcondition_with_counterexample() {
    let ce = json!({"x": "-1"});
    let result = build_contextual_suggestion(FAILURE_POSTCONDITION_VIOLATED, Some(&ce), None);
    assert!(
        result.contains("x = -1"),
        "should mention x = -1: {}",
        result
    );
    assert!(
        result.contains("ensures"),
        "should mention ensures: {}",
        result
    );
}

#[test]
fn test_contextual_suggestion_division_by_zero_with_counterexample() {
    let ce = json!({"divisor": "0"});
    let result = build_contextual_suggestion(FAILURE_DIVISION_BY_ZERO, Some(&ce), None);
    assert!(
        result.contains("divisor != 0"),
        "should suggest divisor != 0: {}",
        result
    );
}

#[test]
fn test_contextual_suggestion_invariant_with_unsat_core() {
    let unsat_core = json!([{"description": "i >= 0"}]);
    let result = build_contextual_suggestion(FAILURE_INVARIANT_VIOLATED, None, Some(&unsat_core));
    assert!(
        result.contains("i >= 0"),
        "should reference constraint: {}",
        result
    );
    assert!(
        result.contains("invariant") || result.contains("不変条件"),
        "should mention invariant: {}",
        result
    );
}

#[test]
fn test_contextual_suggestion_precondition_with_integer_counterexample() {
    // Regression test: JSON integer values (not strings) must be rendered correctly
    let ce = json!({"b": 0});
    let result = build_contextual_suggestion(FAILURE_PRECONDITION_VIOLATED, Some(&ce), None);
    assert!(
        result.contains("b != 0"),
        "should suggest b != 0 guard: {}",
        result
    );
}

#[test]
fn test_contextual_suggestion_postcondition_with_integer_counterexample() {
    // Regression test: JSON integer values in postcondition branch
    let ce = json!({"x": -1});
    let result = build_contextual_suggestion(FAILURE_POSTCONDITION_VIOLATED, Some(&ce), None);
    assert!(
        result.contains("x = -1"),
        "should mention x = -1: {}",
        result
    );
}

#[test]
fn test_contextual_suggestion_division_by_zero_with_integer_counterexample() {
    // Regression test: JSON integer values in division-by-zero branch
    let ce = json!({"divisor": 0});
    let result = build_contextual_suggestion(FAILURE_DIVISION_BY_ZERO, Some(&ce), None);
    assert!(
        result.contains("divisor != 0"),
        "should suggest divisor != 0: {}",
        result
    );
}

#[test]
fn test_contextual_suggestion_fallback_no_counterexample() {
    let result = build_contextual_suggestion(FAILURE_PRECONDITION_VIOLATED, None, None);
    // Should fall back to suggestion_for_failure_type
    let fallback = suggestion_for_failure_type(FAILURE_PRECONDITION_VIOLATED);
    assert_eq!(result, fallback);
}

#[test]
fn test_contextual_suggestion_unknown_failure_type() {
    let result = build_contextual_suggestion("unknown_type", Some(&json!({"x": "1"})), None);
    let fallback = suggestion_for_failure_type("unknown_type");
    assert_eq!(result, fallback);
}

// ---- Subsumption check unit tests ----

#[test]
fn test_subsumption_check_holds_with_requires() {
    // increment: requires x >= 0, ensures result == x + 1
    // contract: ensures result >= 0
    // With requires x >= 0: result == x + 1 ≥ 1 ≥ 0, so subsumption holds.
    let cfg = Config::new();
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);
    let module_env = ModuleEnv::new();
    let vc = VCtx {
        ctx: &ctx,
        module_env: &module_env,
        current_atom: None,
        linearity_ctx: None,
        effect_ctx: None,
        constraint_count: None,
        constraint_budget: DEFAULT_CONSTRAINT_BUDGET,
        has_string_constraints: None,
        path_cond_stack: std::cell::RefCell::new(Vec::new()),
        profiler: None,
        ieee754_f64: false,
    };
    let concrete = Atom {
        name: "increment".to_string(),
        type_params: vec![],
        where_bounds: vec![],
        params: vec![Param {
            name: "x".to_string(),
            type_name: Some("i64".to_string()),
            type_ref: None,
            is_ref: false,
            is_ref_mut: false,
            fn_contract_requires: None,
            fn_contract_ensures: None,
        }],
        trace_id: None,
        spec_metadata: std::collections::HashMap::new(),
        requires: "x >= 0".to_string(),
        forall_constraints: vec![],
        ensures: "result == x + 1".to_string(),
        body_expr: "x + 1".to_string(),
        consumed_params: vec![],
        resources: vec![],
        is_async: false,
        trust_level: TrustLevel::Verified,
        max_unroll: None,
        invariant: None,
        effects: vec![],
        return_type: None,
        span: Span::default(),
        effect_pre: std::collections::HashMap::new(),
        effect_post: std::collections::HashMap::new(),
    };
    let result = check_contract_subsumption(
        &vc,
        &concrete,
        "result >= 0",
        None,
        "apply",
        "f",
        &solver,
        &ctx,
    );
    assert!(
        result,
        "subsumption should hold: x >= 0 ∧ result == x + 1 ⇒ result >= 0"
    );
}

#[test]
fn test_subsumption_check_fails_without_requires() {
    // negate: requires x >= 0, ensures result == 0 - x
    // contract: ensures result >= 0
    // Even with requires x >= 0, result == -x ≤ 0 when x > 0, so subsumption fails.
    let cfg = Config::new();
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);
    let module_env = ModuleEnv::new();
    let vc = VCtx {
        ctx: &ctx,
        module_env: &module_env,
        current_atom: None,
        linearity_ctx: None,
        effect_ctx: None,
        constraint_count: None,
        constraint_budget: DEFAULT_CONSTRAINT_BUDGET,
        has_string_constraints: None,
        path_cond_stack: std::cell::RefCell::new(Vec::new()),
        profiler: None,
        ieee754_f64: false,
    };
    let concrete = Atom {
        name: "negate".to_string(),
        type_params: vec![],
        where_bounds: vec![],
        params: vec![Param {
            name: "x".to_string(),
            type_name: Some("i64".to_string()),
            type_ref: None,
            is_ref: false,
            is_ref_mut: false,
            fn_contract_requires: None,
            fn_contract_ensures: None,
        }],
        trace_id: None,
        spec_metadata: std::collections::HashMap::new(),
        requires: "x >= 0".to_string(),
        forall_constraints: vec![],
        ensures: "result == 0 - x".to_string(),
        body_expr: "0 - x".to_string(),
        consumed_params: vec![],
        resources: vec![],
        is_async: false,
        trust_level: TrustLevel::Verified,
        max_unroll: None,
        invariant: None,
        effects: vec![],
        return_type: None,
        span: Span::default(),
        effect_pre: std::collections::HashMap::new(),
        effect_post: std::collections::HashMap::new(),
    };
    let result = check_contract_subsumption(
        &vc,
        &concrete,
        "result >= 0",
        None,
        "apply",
        "f",
        &solver,
        &ctx,
    );
    assert!(
        !result,
        "subsumption should fail: x >= 0 ∧ result == -x does NOT imply result >= 0"
    );
}

#[test]
fn test_subsumption_check_crossed_param_names() {
    // Regression test: atom compute(y: i64, x: i64) with ensures: result == y / x
    // Contract: ensures result >= 0
    // Without the alias-collision fix, both "x" and "y" would map to the same
    // Z3 variable, making result == var/var == 1, trivially passing.
    // With the fix, y and x are independent, so y=-1, x=1 → result=-1 is a
    // valid counterexample and subsumption should fail.
    let cfg = Config::new();
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);
    let module_env = ModuleEnv::new();
    let vc = VCtx {
        ctx: &ctx,
        module_env: &module_env,
        current_atom: None,
        linearity_ctx: None,
        effect_ctx: None,
        constraint_count: None,
        constraint_budget: DEFAULT_CONSTRAINT_BUDGET,
        has_string_constraints: None,
        path_cond_stack: std::cell::RefCell::new(Vec::new()),
        profiler: None,
        ieee754_f64: false,
    };
    let concrete = Atom {
        name: "compute".to_string(),
        type_params: vec![],
        where_bounds: vec![],
        params: vec![
            Param {
                name: "y".to_string(),
                type_name: Some("i64".to_string()),
                type_ref: None,
                is_ref: false,
                is_ref_mut: false,
                fn_contract_requires: None,
                fn_contract_ensures: None,
            },
            Param {
                name: "x".to_string(),
                type_name: Some("i64".to_string()),
                type_ref: None,
                is_ref: false,
                is_ref_mut: false,
                fn_contract_requires: None,
                fn_contract_ensures: None,
            },
        ],
        trace_id: None,
        spec_metadata: std::collections::HashMap::new(),
        requires: "x != 0".to_string(),
        forall_constraints: vec![],
        ensures: "result == y / x".to_string(),
        body_expr: "y / x".to_string(),
        consumed_params: vec![],
        resources: vec![],
        is_async: false,
        trust_level: TrustLevel::Verified,
        max_unroll: None,
        invariant: None,
        effects: vec![],
        return_type: None,
        span: Span::default(),
        effect_pre: std::collections::HashMap::new(),
        effect_post: std::collections::HashMap::new(),
    };
    let result = check_contract_subsumption(
        &vc,
        &concrete,
        "result >= 0",
        None,
        "apply",
        "f",
        &solver,
        &ctx,
    );
    assert!(
        !result,
        "subsumption should fail: y/x can be negative (e.g. y=-1, x=1)"
    );
}

#[test]
fn test_subsumption_check_trivial_contract_ensures_skipped() {
    // If contract_ensures is "true", subsumption is trivially satisfied
    // (the contract requires nothing).
    let cfg = Config::new();
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);
    let module_env = ModuleEnv::new();
    let vc = VCtx {
        ctx: &ctx,
        module_env: &module_env,
        current_atom: None,
        linearity_ctx: None,
        effect_ctx: None,
        constraint_count: None,
        constraint_budget: DEFAULT_CONSTRAINT_BUDGET,
        has_string_constraints: None,
        path_cond_stack: std::cell::RefCell::new(Vec::new()),
        profiler: None,
        ieee754_f64: false,
    };
    let concrete = Atom {
        name: "something".to_string(),
        type_params: vec![],
        where_bounds: vec![],
        params: vec![Param {
            name: "x".to_string(),
            type_name: Some("i64".to_string()),
            type_ref: None,
            is_ref: false,
            is_ref_mut: false,
            fn_contract_requires: None,
            fn_contract_ensures: None,
        }],
        trace_id: None,
        spec_metadata: std::collections::HashMap::new(),
        requires: "true".to_string(),
        forall_constraints: vec![],
        ensures: "result == x".to_string(),
        body_expr: "x".to_string(),
        consumed_params: vec![],
        resources: vec![],
        is_async: false,
        trust_level: TrustLevel::Verified,
        max_unroll: None,
        invariant: None,
        effects: vec![],
        return_type: None,
        span: Span::default(),
        effect_pre: std::collections::HashMap::new(),
        effect_post: std::collections::HashMap::new(),
    };
    // contract ensures is "true" → trivially satisfied, skip check
    let result =
        check_contract_subsumption(&vc, &concrete, "true", None, "apply", "f", &solver, &ctx);
    assert!(
        result,
        "trivial contract ensures 'true' should be skipped (returns true)"
    );
}

#[test]
fn test_subsumption_check_concrete_true_ensures_warns() {
    // If concrete_atom.ensures is "true" but contract requires "result >= 0",
    // the concrete atom guarantees nothing → subsumption should FAIL (warn).
    let cfg = Config::new();
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);
    let module_env = ModuleEnv::new();
    let vc = VCtx {
        ctx: &ctx,
        module_env: &module_env,
        current_atom: None,
        linearity_ctx: None,
        effect_ctx: None,
        constraint_count: None,
        constraint_budget: DEFAULT_CONSTRAINT_BUDGET,
        has_string_constraints: None,
        path_cond_stack: std::cell::RefCell::new(Vec::new()),
        profiler: None,
        ieee754_f64: false,
    };
    let concrete = Atom {
        name: "no_guarantee".to_string(),
        type_params: vec![],
        where_bounds: vec![],
        params: vec![Param {
            name: "x".to_string(),
            type_name: Some("i64".to_string()),
            type_ref: None,
            is_ref: false,
            is_ref_mut: false,
            fn_contract_requires: None,
            fn_contract_ensures: None,
        }],
        trace_id: None,
        spec_metadata: std::collections::HashMap::new(),
        requires: "true".to_string(),
        forall_constraints: vec![],
        ensures: "true".to_string(),
        body_expr: "x".to_string(),
        consumed_params: vec![],
        resources: vec![],
        is_async: false,
        trust_level: TrustLevel::Verified,
        max_unroll: None,
        invariant: None,
        effects: vec![],
        return_type: None,
        span: Span::default(),
        effect_pre: std::collections::HashMap::new(),
        effect_post: std::collections::HashMap::new(),
    };
    // concrete ensures is "true" but contract requires "result >= 0"
    // → subsumption should fail because "true" does not imply "result >= 0"
    let result = check_contract_subsumption(
        &vc,
        &concrete,
        "result >= 0",
        None,
        "apply",
        "f",
        &solver,
        &ctx,
    );
    assert!(
        !result,
        "concrete ensures 'true' cannot imply 'result >= 0' — should warn (return false)"
    );
}

// ---- 3-a: replace_constraint_placeholder tests ----

#[test]
fn test_replace_constraint_placeholder_standalone_v() {
    // "v != 0" → "b != 0" (standalone v is replaced)
    let result = replace_constraint_placeholder("v != 0", "b");
    assert_eq!(result, "b != 0");
}

#[test]
fn test_replace_constraint_placeholder_does_not_corrupt_value() {
    // "value != 0" should NOT become "balue != 0"
    let result = replace_constraint_placeholder("value != 0", "b");
    assert_eq!(result, "value != 0");
}

#[test]
fn test_replace_constraint_placeholder_does_not_corrupt_divisor() {
    // "divisor > v" should replace only standalone v
    let result = replace_constraint_placeholder("divisor > v", "x");
    assert_eq!(result, "divisor > x");
}

#[test]
fn test_replace_constraint_placeholder_multiple_v() {
    // "v > 0 && v < 100" → "param > 0 && param < 100"
    let result = replace_constraint_placeholder("v > 0 && v < 100", "param");
    assert_eq!(result, "param > 0 && param < 100");
}

// ---- 3-b: get_traits_for_method / method_trait_index tests ----

#[test]
fn test_get_traits_for_method_single_trait() {
    let mut env = ModuleEnv::new();
    env.register_trait(&TraitDef {
        name: "Numeric".to_string(),
        methods: vec![TraitMethod {
            name: "div".to_string(),
            param_types: vec!["i64".to_string(), "i64".to_string()],
            return_type: "i64".to_string(),
            param_constraints: vec![None, Some("v != 0".to_string())],
        }],
        laws: vec![],
        span: Span::new("", 0, 0, 0),
    });
    let results = env.get_traits_for_method("div");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, "Numeric");
    assert_eq!(results[0].1.name, "div");
}

#[test]
fn test_get_traits_for_method_multiple_traits_same_method() {
    let mut env = ModuleEnv::new();
    env.register_trait(&TraitDef {
        name: "TraitA".to_string(),
        methods: vec![TraitMethod {
            name: "process".to_string(),
            param_types: vec!["i64".to_string()],
            return_type: "i64".to_string(),
            param_constraints: vec![Some("v > 0".to_string())],
        }],
        laws: vec![],
        span: Span::new("", 0, 0, 0),
    });
    env.register_trait(&TraitDef {
        name: "TraitB".to_string(),
        methods: vec![TraitMethod {
            name: "process".to_string(),
            param_types: vec!["i64".to_string()],
            return_type: "i64".to_string(),
            param_constraints: vec![None],
        }],
        laws: vec![],
        span: Span::new("", 0, 0, 0),
    });
    // Both traits should be returned
    let results = env.get_traits_for_method("process");
    assert_eq!(results.len(), 2);
    let trait_names: Vec<&str> = results.iter().map(|(tn, _)| *tn).collect();
    assert!(trait_names.contains(&"TraitA"));
    assert!(trait_names.contains(&"TraitB"));
}

#[test]
fn test_get_traits_for_method_selects_correct_with_find_impl() {
    let mut env = ModuleEnv::new();
    env.register_trait(&TraitDef {
        name: "TraitA".to_string(),
        methods: vec![TraitMethod {
            name: "process".to_string(),
            param_types: vec!["i64".to_string()],
            return_type: "i64".to_string(),
            param_constraints: vec![Some("v > 0".to_string())],
        }],
        laws: vec![],
        span: Span::new("", 0, 0, 0),
    });
    env.register_trait(&TraitDef {
        name: "TraitB".to_string(),
        methods: vec![TraitMethod {
            name: "process".to_string(),
            param_types: vec!["Str".to_string()],
            return_type: "Str".to_string(),
            param_constraints: vec![None],
        }],
        laws: vec![],
        span: Span::new("", 0, 0, 0),
    });
    // Only TraitA has an impl for i64
    env.register_impl(&ImplDef {
        trait_name: "TraitA".to_string(),
        target_type: "i64".to_string(),
        method_bodies: vec![],
        span: Span::new("", 0, 0, 0),
    });
    let candidates = env.get_traits_for_method("process");
    let matched = candidates
        .iter()
        .find(|(tn, _)| env.find_impl(tn, "i64").is_some());
    assert!(matched.is_some());
    assert_eq!(matched.unwrap().0, "TraitA");
}

// ---- 3-c: infer_requires callee argument substitution tests ----

#[test]
fn test_infer_requires_substitutes_callee_params() {
    use std::collections::HashMap;
    let mut env = ModuleEnv::new();
    // Register callee atom with requires: x > 0
    env.register_atom(&Atom {
        name: "callee".to_string(),
        type_params: vec![],
        where_bounds: vec![],
        params: vec![Param {
            name: "x".to_string(),
            type_name: Some("i64".to_string()),
            type_ref: None,
            is_ref: false,
            is_ref_mut: false,
            fn_contract_requires: None,
            fn_contract_ensures: None,
        }],
        trace_id: None,
        spec_metadata: std::collections::HashMap::new(),
        requires: "x > 0".to_string(),
        forall_constraints: vec![],
        ensures: "true".to_string(),
        body_expr: "x + 1".to_string(),
        consumed_params: vec![],
        resources: vec![],
        is_async: false,
        trust_level: TrustLevel::Verified,
        max_unroll: None,
        invariant: None,
        effects: vec![],
        return_type: None,
        span: Span::new("", 0, 0, 0),
        effect_pre: HashMap::new(),
        effect_post: HashMap::new(),
    });
    // Caller atom that calls callee(a + b)
    let caller = Atom {
        name: "caller".to_string(),
        type_params: vec![],
        where_bounds: vec![],
        params: vec![
            Param {
                name: "a".to_string(),
                type_name: Some("i64".to_string()),
                type_ref: None,
                is_ref: false,
                is_ref_mut: false,
                fn_contract_requires: None,
                fn_contract_ensures: None,
            },
            Param {
                name: "b".to_string(),
                type_name: Some("i64".to_string()),
                type_ref: None,
                is_ref: false,
                is_ref_mut: false,
                fn_contract_requires: None,
                fn_contract_ensures: None,
            },
        ],
        trace_id: None,
        spec_metadata: std::collections::HashMap::new(),
        requires: "true".to_string(),
        forall_constraints: vec![],
        ensures: "true".to_string(),
        body_expr: "callee(a + b)".to_string(),
        consumed_params: vec![],
        resources: vec![],
        is_async: false,
        trust_level: TrustLevel::Verified,
        max_unroll: None,
        invariant: None,
        effects: vec![],
        return_type: None,
        span: Span::new("", 0, 0, 0),
        effect_pre: HashMap::new(),
        effect_post: HashMap::new(),
    };
    let inferred = infer_requires(&caller, &env);
    // Should contain "(a + b) > 0" not "x > 0"
    assert!(
        inferred
            .iter()
            .any(|r| r.contains("a + b") && r.contains("> 0")),
        "Expected substituted requires with 'a + b', got: {:?}",
        inferred
    );
    assert!(
        !inferred.iter().any(|r| r == "x > 0"),
        "Should not contain raw callee param 'x > 0', got: {:?}",
        inferred
    );
}

// ---- expr_to_source_string tests ----

#[test]
fn test_expr_to_source_string_basic() {
    assert_eq!(expr_to_source_string(&Expr::Number(42)), "42");
    assert_eq!(expr_to_source_string(&Expr::Variable("x".to_string())), "x");
    assert_eq!(
        expr_to_source_string(&Expr::BinaryOp(
            Box::new(Expr::Variable("a".to_string())),
            Op::Add,
            Box::new(Expr::Variable("b".to_string())),
        )),
        "(a + b)"
    );
}

// ---- collect_array_accesses tests ----
//
// These tests pin the invariants used by the forall-constraint handler
// when it synthesises Z3 patterns and `len_arr > idx` bounds from the
// body of a `forall(...)` expression extracted out of a `requires` clause.

#[test]
fn test_collect_array_accesses_simple() {
    // forall(i, 0, n, arr[i] >= 0) — body is `arr[i] >= 0`
    let body = Expr::BinaryOp(
        Box::new(Expr::ArrayAccess(
            "arr".to_string(),
            Box::new(Expr::Variable("i".to_string())),
        )),
        Op::Ge,
        Box::new(Expr::Number(0)),
    );
    let accesses = collect_array_accesses(&body);
    assert_eq!(accesses.len(), 1);
    assert_eq!(accesses[0].0, "arr");
    match &accesses[0].1 {
        Expr::Variable(v) => assert_eq!(v, "i"),
        other => panic!("expected Variable(\"i\"), got {:?}", other),
    }
}

#[test]
fn test_collect_array_accesses_pair() {
    // forall(i, 0, n-1, arr[i] <= arr[i + 1])
    let body = Expr::BinaryOp(
        Box::new(Expr::ArrayAccess(
            "arr".to_string(),
            Box::new(Expr::Variable("i".to_string())),
        )),
        Op::Le,
        Box::new(Expr::ArrayAccess(
            "arr".to_string(),
            Box::new(Expr::BinaryOp(
                Box::new(Expr::Variable("i".to_string())),
                Op::Add,
                Box::new(Expr::Number(1)),
            )),
        )),
    );
    let accesses = collect_array_accesses(&body);
    assert_eq!(accesses.len(), 2);
    assert_eq!(accesses[0].0, "arr");
    assert_eq!(accesses[1].0, "arr");
    match &accesses[0].1 {
        Expr::Variable(v) => assert_eq!(v, "i"),
        other => panic!("expected Variable(\"i\"), got {:?}", other),
    }
    match &accesses[1].1 {
        Expr::BinaryOp(_, Op::Add, _) => {}
        other => panic!("expected i + 1 BinaryOp, got {:?}", other),
    }
}

#[test]
fn test_collect_array_accesses_multi_array_names() {
    // forall(i, 0, n, arr[i] == data[i]) — two distinct array names.
    let body = Expr::BinaryOp(
        Box::new(Expr::ArrayAccess(
            "arr".to_string(),
            Box::new(Expr::Variable("i".to_string())),
        )),
        Op::Eq,
        Box::new(Expr::ArrayAccess(
            "data".to_string(),
            Box::new(Expr::Variable("i".to_string())),
        )),
    );
    let accesses = collect_array_accesses(&body);
    assert_eq!(accesses.len(), 2);
    assert_eq!(accesses[0].0, "arr");
    assert_eq!(accesses[1].0, "data");
}

#[test]
fn test_collect_array_accesses_nested_index() {
    // arr[arr[i]] — the outer access must be captured, and the inner
    // nested ArrayAccess inside the index expression must also surface.
    let body = Expr::ArrayAccess(
        "arr".to_string(),
        Box::new(Expr::ArrayAccess(
            "arr".to_string(),
            Box::new(Expr::Variable("i".to_string())),
        )),
    );
    let accesses = collect_array_accesses(&body);
    assert_eq!(accesses.len(), 2);
    assert_eq!(accesses[0].0, "arr");
    assert_eq!(accesses[1].0, "arr");
}

#[test]
fn test_collect_array_accesses_recurses_into_while_body() {
    let while_stmt = crate::parser::Stmt::While {
        cond: Box::new(Expr::Variable("cond".to_string())),
        invariant: Box::new(Expr::Variable("invariant".to_string())),
        decreases: None,
        body: Box::new(crate::parser::Stmt::Block(
            vec![crate::parser::Stmt::Expr(
                Expr::ArrayAccess("arr".to_string(), Box::new(Expr::Variable("i".to_string()))),
                Span::default(),
            )],
            Span::default(),
        )),
        span: Span::default(),
    };

    let mut accesses = Vec::new();
    collect_array_accesses_in_stmt(&while_stmt, &mut accesses);

    assert_eq!(accesses.len(), 1);
    assert_eq!(accesses[0].0, "arr");
    match &accesses[0].1 {
        Expr::Variable(v) => assert_eq!(v, "i"),
        other => panic!("expected Variable(\"i\"), got {:?}", other),
    }
}

#[test]
fn test_collect_array_accesses_none() {
    // No array access at all → empty
    let body = Expr::BinaryOp(
        Box::new(Expr::Variable("n".to_string())),
        Op::Ge,
        Box::new(Expr::Number(0)),
    );
    let accesses = collect_array_accesses(&body);
    assert!(accesses.is_empty());
}

// ---- Variable("true") / Variable("false") as Z3 Bool ----
//
// strip_quantifiers replaces `forall(...)` with the literal token `true`
// when extracting forall constraints from a `requires` clause. Without
// Bool sort for Variable("true")/("false"), the remaining `... && true`
// fails to typecheck as a Z3 Bool and the atom can't be verified.

#[test]
fn test_expr_to_z3_true_false_are_bool() {
    use z3::{Config, Context, Solver};

    let cfg = Config::new();
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);
    let module_env = ModuleEnv::new();
    let linearity_ctx_cell = std::cell::RefCell::new(LinearityCtx::new());
    let effect_ctx_cell = std::cell::RefCell::new(EffectCtx::new(std::collections::HashSet::new()));
    let constraint_count_cell = std::cell::Cell::new(0usize);
    let has_string_constraints_cell = std::cell::Cell::new(false);
    let vc = VCtx {
        ctx: &ctx,
        module_env: &module_env,
        current_atom: None,
        linearity_ctx: Some(&linearity_ctx_cell),
        effect_ctx: Some(&effect_ctx_cell),
        constraint_count: Some(&constraint_count_cell),
        constraint_budget: DEFAULT_CONSTRAINT_BUDGET,
        has_string_constraints: Some(&has_string_constraints_cell),
        path_cond_stack: std::cell::RefCell::new(Vec::new()),
        profiler: None,
        ieee754_f64: false,
    };
    let mut env: Env = HashMap::new();

    // Variable("true") → Bool, And'able with another Bool
    let true_expr = Expr::Variable("true".to_string());
    let true_z3 = expr_to_z3(&vc, &true_expr, &mut env, Some(&solver)).unwrap();
    assert!(
        true_z3.as_bool().is_some(),
        "Variable(\"true\") must yield Bool sort"
    );

    let false_expr = Expr::Variable("false".to_string());
    let false_z3 = expr_to_z3(&vc, &false_expr, &mut env, Some(&solver)).unwrap();
    assert!(
        false_z3.as_bool().is_some(),
        "Variable(\"false\") must yield Bool sort"
    );

    // `x > 0 && true` — mirrors the shape of `strip_quantifiers`-processed requires.
    let composite = Expr::BinaryOp(
        Box::new(Expr::BinaryOp(
            Box::new(Expr::Variable("x".to_string())),
            Op::Gt,
            Box::new(Expr::Number(0)),
        )),
        Op::And,
        Box::new(Expr::Variable("true".to_string())),
    );
    let composite_z3 = expr_to_z3(&vc, &composite, &mut env, Some(&solver))
        .expect("composite `>  && true` must parse when Variable(true) is Bool");
    assert!(composite_z3.as_bool().is_some());
}

#[test]
fn test_expr_to_z3_pow_constant_folds_full_precision() {
    use crate::parser::parse_expression;
    use z3::{Config, Context, Solver};

    let cfg = Config::new();
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);
    let module_env = ModuleEnv::new();
    let linearity_ctx_cell = std::cell::RefCell::new(LinearityCtx::new());
    let effect_ctx_cell = std::cell::RefCell::new(EffectCtx::new(std::collections::HashSet::new()));
    let constraint_count_cell = std::cell::Cell::new(0usize);
    let has_string_constraints_cell = std::cell::Cell::new(false);
    let vc = VCtx {
        ctx: &ctx,
        module_env: &module_env,
        current_atom: None,
        linearity_ctx: Some(&linearity_ctx_cell),
        effect_ctx: Some(&effect_ctx_cell),
        constraint_count: Some(&constraint_count_cell),
        constraint_budget: DEFAULT_CONSTRAINT_BUDGET,
        has_string_constraints: Some(&has_string_constraints_cell),
        path_cond_stack: std::cell::RefCell::new(Vec::new()),
        profiler: None,
        ieee754_f64: false,
    };
    let mut env: Env = HashMap::new();

    let folded_64 = expr_to_z3(
        &vc,
        &parse_expression("2 ** 64 - 1"),
        &mut env,
        Some(&solver),
    )
    .expect("2**64-1 should fold");
    assert_eq!(folded_64.to_string(), "18446744073709551615");

    let folded_256 = expr_to_z3(
        &vc,
        &parse_expression("2 ** 256 - 1"),
        &mut env,
        Some(&solver),
    )
    .expect("2**256-1 should fold");
    assert_eq!(
        folded_256.to_string(),
        "115792089237316195423570985008687907853269984665640564039457584007913129639935"
    );
}

#[test]
fn test_tuple_result_indexing_uses_typed_components() {
    use crate::parser::parse_expression;
    use crate::verification::spec_validation::is_unsupported_clause_error;
    use z3::{Config, Context, Solver};

    let cfg = Config::new();
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);
    let module_env = ModuleEnv::new();
    let linearity_ctx_cell = std::cell::RefCell::new(LinearityCtx::new());
    let effect_ctx_cell = std::cell::RefCell::new(EffectCtx::new(std::collections::HashSet::new()));
    let constraint_count_cell = std::cell::Cell::new(0usize);
    let has_string_constraints_cell = std::cell::Cell::new(false);
    let vc = VCtx {
        ctx: &ctx,
        module_env: &module_env,
        current_atom: None,
        linearity_ctx: Some(&linearity_ctx_cell),
        effect_ctx: Some(&effect_ctx_cell),
        constraint_count: Some(&constraint_count_cell),
        constraint_budget: DEFAULT_CONSTRAINT_BUDGET,
        has_string_constraints: Some(&has_string_constraints_cell),
        path_cond_stack: std::cell::RefCell::new(Vec::new()),
        profiler: None,
        ieee754_f64: false,
    };
    let mut env: Env = HashMap::new();
    seed_tuple_result_components(
        &ctx,
        &mut env,
        "result",
        Some("(u64, bool)"),
        &module_env,
        false,
    );

    let bool_clause = expr_to_z3(
        &vc,
        &parse_expression("result[1] == false"),
        &mut env,
        Some(&solver),
    )
    .expect("bool tuple component should lower")
    .as_bool()
    .expect("tuple component comparison should be boolean");
    solver.assert(&bool_clause);
    assert_eq!(solver.check(), SatResult::Sat);

    let first = expr_to_z3(
        &vc,
        &parse_expression("result[0] == 7"),
        &mut env,
        Some(&solver),
    )
    .expect("integer tuple component should lower")
    .as_bool()
    .expect("tuple component comparison should be boolean");
    let conflicting = expr_to_z3(
        &vc,
        &parse_expression("result[0] == 8"),
        &mut env,
        Some(&solver),
    )
    .expect("integer tuple component should lower")
    .as_bool()
    .expect("tuple component comparison should be boolean");
    solver.assert(&first);
    solver.assert(&conflicting);
    assert_eq!(solver.check(), SatResult::Unsat);

    let out_of_range = expr_to_z3(&vc, &parse_expression("result[2] == 0"), &mut env, None)
        .expect_err("out-of-range tuple index should be unsupported");
    assert!(is_unsupported_clause_error(&out_of_range));

    let symbolic = expr_to_z3(&vc, &parse_expression("result[x] == 0"), &mut env, None)
        .expect_err("symbolic tuple index should be unsupported");
    assert!(is_unsupported_clause_error(&symbolic));

    let mut plain_env: Env = HashMap::new();
    let plain_array_access = expr_to_z3(
        &vc,
        &parse_expression("result[0] == 7"),
        &mut plain_env,
        None,
    )
    .expect("non-tuple result indexing should retain array semantics")
    .as_bool()
    .expect("generic array comparison should be boolean");
    assert!(plain_array_access.to_string().contains("result"));
}

#[test]
fn test_chained_comparison_normalizes_before_lowering() {
    use crate::parser::expr::normalize_comparison_chains;
    use crate::parser::parse_expression;
    use z3::{Config, Context, Solver};

    let cfg = Config::new();
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);
    let module_env = ModuleEnv::new();
    let linearity_ctx_cell = std::cell::RefCell::new(LinearityCtx::new());
    let effect_ctx_cell = std::cell::RefCell::new(EffectCtx::new(std::collections::HashSet::new()));
    let constraint_count_cell = std::cell::Cell::new(0usize);
    let has_string_constraints_cell = std::cell::Cell::new(false);
    let vc = VCtx {
        ctx: &ctx,
        module_env: &module_env,
        current_atom: None,
        linearity_ctx: Some(&linearity_ctx_cell),
        effect_ctx: Some(&effect_ctx_cell),
        constraint_count: Some(&constraint_count_cell),
        constraint_budget: DEFAULT_CONSTRAINT_BUDGET,
        has_string_constraints: Some(&has_string_constraints_cell),
        path_cond_stack: std::cell::RefCell::new(Vec::new()),
        profiler: None,
        ieee754_f64: false,
    };
    let mut env: Env = HashMap::new();

    let sat = expr_to_z3(
        &vc,
        &normalize_comparison_chains(parse_expression("0 <= 3 <= 5")),
        &mut env,
        Some(&solver),
    )
    .expect("satisfiable chain should lower")
    .as_bool()
    .expect("chain should lower to bool");
    solver.assert(&sat);
    assert_eq!(solver.check(), SatResult::Sat);

    let unsat = expr_to_z3(
        &vc,
        &normalize_comparison_chains(parse_expression("0 <= 6 <= 5")),
        &mut env,
        Some(&solver),
    )
    .expect("refutable chain should lower")
    .as_bool()
    .expect("chain should lower to bool");
    solver.assert(&unsat);
    assert_eq!(solver.check(), SatResult::Unsat);
}

// ---- forall-over-arr + E-matching pattern end-to-end ----
//
// Exercises the full forall_constraints path by asserting
// `forall i ∈ [0, n). arr[i] ≥ 0` and checking that Z3 can then discharge
// the same claim as an ensures — this used to fail prior to the
// pattern-based instantiation hint and `len_arr > idx` bound.

#[test]
fn test_forall_arr_transfers_between_requires_and_ensures() {
    use z3::ast::Ast;
    use z3::{Config, Context, SatResult, Solver};

    let cfg = Config::new();
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);
    let int_sort = z3::Sort::int(&ctx);
    let arr = Array::new_const(&ctx, "arr", &int_sort, &int_sort);

    let n = Int::new_const(&ctx, "n");
    let i = Int::new_const(&ctx, "i");
    let zero = Int::from_i64(&ctx, 0);

    // Requires forall with an explicit E-matching pattern on arr[i].
    let range_req = Bool::and(&ctx, &[&i.ge(&zero), &i.lt(&n)]);
    let ai = arr.select(&i).as_int().unwrap();
    let body_req = range_req.implies(&ai.ge(&zero));
    let pattern_ast = arr.select(&i);
    let pattern_refs: Vec<&dyn Ast> = vec![&pattern_ast as &dyn Ast];
    let pattern = z3::Pattern::new(&ctx, &pattern_refs);
    let req_forall = z3::ast::forall_const(&ctx, &[&i], &[&pattern], &body_req);
    solver.assert(&n.ge(&zero));
    solver.assert(&req_forall);

    // Ensures: same forall ⇒ must be provable (negation unsat).
    let body_ens = range_req.implies(&ai.ge(&zero));
    let ens_forall = z3::ast::forall_const(&ctx, &[&i], &[&pattern], &body_ens);
    solver.push();
    solver.assert(&ens_forall.not());
    assert_eq!(
        solver.check(),
        SatResult::Unsat,
        "requires forall should discharge equivalent ensures forall under same pattern"
    );
    solver.pop(1);
}
