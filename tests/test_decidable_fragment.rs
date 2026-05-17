use std::collections::HashMap;

use mumei_core::parser::{
    Atom, Effect, EffectDef, EffectTransition, EnumDef, EnumVariant, Param, Quantifier,
    QuantifierType, Span, TrustLevel,
};
use mumei_core::verification::{
    collect_decidable_fragment_metrics, detect_logic_fragment_tags,
    outside_decidable_fragment_warning, ModuleEnv,
};

fn base_atom(name: &str) -> Atom {
    Atom {
        name: name.to_string(),
        type_params: Vec::new(),
        where_bounds: Vec::new(),
        params: Vec::new(),
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

fn assert_tag(atom: &Atom, module_env: &ModuleEnv, expected_tag: &str) {
    let tags = detect_logic_fragment_tags(atom, module_env);
    assert!(
        tags.iter().any(|tag| tag == expected_tag),
        "expected tag {expected_tag:?}, got {tags:?}"
    );
    let warning = outside_decidable_fragment_warning(atom, module_env)
        .expect("expected outside_decidable_fragment warning");
    assert!(warning.contains("outside_decidable_fragment"));
    assert!(warning.contains(expected_tag));
}

#[test]
fn detects_nonlinear_arithmetic_in_symbolic_mul_and_div() {
    let module_env = ModuleEnv::new();

    let mut mul_atom = base_atom("symbolic_mul");
    mul_atom.params = vec![param("x", "i64"), param("y", "i64")];
    mul_atom.ensures = "result == x * y".to_string();
    assert_tag(&mul_atom, &module_env, "nonlinear_arithmetic");

    let mut div_atom = base_atom("symbolic_div");
    div_atom.params = vec![param("x", "i64"), param("y", "i64")];
    div_atom.requires = "y != 0".to_string();
    div_atom.ensures = "result == x / y".to_string();
    assert_tag(&div_atom, &module_env, "nonlinear_arithmetic");
}

#[test]
fn detects_array_access_without_explicit_bounds() {
    let module_env = ModuleEnv::new();
    let mut atom = base_atom("unbounded_array_access");
    atom.params = vec![param("i", "i64")];
    atom.ensures = "result == arr[i]".to_string();

    assert_tag(&atom, &module_env, "array_without_bounds");
}

#[test]
fn detects_quantifier_alternation() {
    let module_env = ModuleEnv::new();
    let mut atom = base_atom("alternating_quantifiers");
    atom.forall_constraints = vec![
        Quantifier {
            q_type: QuantifierType::ForAll,
            var: "i".to_string(),
            start: "0".to_string(),
            end: "n".to_string(),
            condition: "arr[i] >= 0".to_string(),
        },
        Quantifier {
            q_type: QuantifierType::Exists,
            var: "j".to_string(),
            start: "0".to_string(),
            end: "n".to_string(),
            condition: "arr[j] == result".to_string(),
        },
    ];

    assert_tag(&atom, &module_env, "quantifier_alternation");
}

#[test]
fn detects_trigger_sensitive_quantifier_with_array_access() {
    let module_env = ModuleEnv::new();
    let mut atom = base_atom("array_quantifier");
    atom.forall_constraints = vec![Quantifier {
        q_type: QuantifierType::ForAll,
        var: "i".to_string(),
        start: "0".to_string(),
        end: "n".to_string(),
        condition: "arr[i] >= 0".to_string(),
    }];

    assert_tag(&atom, &module_env, "trigger_sensitive_quantifier");
}

#[test]
fn detects_inductive_data_type_from_recursive_enum_parameter() {
    let mut module_env = ModuleEnv::new();
    module_env.register_enum(&EnumDef {
        name: "List".to_string(),
        type_params: Vec::new(),
        variants: vec![
            EnumVariant {
                name: "Nil".to_string(),
                fields: Vec::new(),
                field_types: Vec::new(),
                is_recursive: false,
            },
            EnumVariant {
                name: "Cons".to_string(),
                fields: vec!["head".to_string(), "tail".to_string()],
                field_types: Vec::new(),
                is_recursive: true,
            },
        ],
        is_recursive: true,
        span: Span::default(),
    });
    let mut atom = base_atom("list_len");
    atom.params = vec![param("xs", "List")];

    assert_tag(&atom, &module_env, "inductive_data_type");
}

#[test]
fn detects_recursive_invariant_from_while_loop() {
    let module_env = ModuleEnv::new();
    let mut atom = base_atom("loop_sum");
    atom.body_expr = "{ let i = 0; while i < n invariant i >= 0 { i = i + 1; }; i }".to_string();

    assert_tag(&atom, &module_env, "recursive_invariant");
}

#[test]
fn detects_complex_temporal_effect_with_many_states() {
    let mut module_env = ModuleEnv::new();
    module_env.effect_defs.insert(
        "Protocol".to_string(),
        EffectDef {
            name: "Protocol".to_string(),
            params: Vec::new(),
            constraint: None,
            includes: Vec::new(),
            refinement: None,
            parent: Vec::new(),
            span: Span::default(),
            states: vec![
                "Created".to_string(),
                "Pending".to_string(),
                "Approved".to_string(),
                "Committed".to_string(),
                "Archived".to_string(),
            ],
            transitions: vec![EffectTransition {
                operation: "advance".to_string(),
                from_state: "Created".to_string(),
                to_state: "Pending".to_string(),
            }],
            initial_state: Some("Created".to_string()),
        },
    );
    let mut atom = base_atom("protocol_step");
    atom.effects = vec![Effect::simple("Protocol")];

    assert_tag(&atom, &module_env, "complex_temporal_effect");
}

#[test]
fn collects_decidable_fragment_metrics_by_warning_tag() {
    let mut module_env = ModuleEnv::new();
    let mut atom = base_atom("nonlinear_metric");
    atom.params = vec![param("x", "i64"), param("y", "i64")];
    atom.ensures = "result == x * y".to_string();
    module_env.register_atom(&atom);

    let clean_atom = base_atom("linear_metric");
    module_env.register_atom(&clean_atom);

    let metrics = collect_decidable_fragment_metrics(&module_env);
    assert_eq!(metrics.total_atoms_checked, 2);
    assert_eq!(metrics.atoms_with_warnings, 1);
    assert_eq!(metrics.warning_counts.get("nonlinear_arithmetic"), Some(&1));
}
