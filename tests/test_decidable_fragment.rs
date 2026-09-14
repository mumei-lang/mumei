use std::{collections::HashMap, process::Command};

use mumei_core::parser::{
    Atom, Effect, EffectDef, EffectTransition, EnumDef, EnumVariant, Param, Quantifier,
    QuantifierType, Span, TrustLevel,
};
use mumei_core::verification::{
    collect_decidable_fragment_metrics, detect_logic_fragment, detect_logic_fragment_tags,
    is_outside_decidable_fragment, outside_decidable_fragment_warning, LogicFragment, ModuleEnv,
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

fn ref_mut_param(name: &str, type_name: &str) -> Param {
    Param {
        is_ref_mut: true,
        ..param(name, type_name)
    }
}

fn assert_detected_tag(atom: &Atom, module_env: &ModuleEnv, expected_tag: &str) -> Vec<String> {
    let tags = detect_logic_fragment_tags(atom, module_env);
    assert!(
        tags.iter().any(|tag| tag == expected_tag),
        "expected tag {expected_tag:?}, got {tags:?}"
    );
    tags
}

fn assert_outside_tag(atom: &Atom, module_env: &ModuleEnv, expected_tag: &str) {
    let tags = assert_detected_tag(atom, module_env, expected_tag);
    assert!(is_outside_decidable_fragment(&tags));
    let warning = outside_decidable_fragment_warning(atom, module_env)
        .expect("expected outside_decidable_fragment warning");
    assert!(warning.contains("outside_decidable_fragment"));
    assert!(warning.contains(expected_tag));
}

#[test]
fn diagnostic_struct_uses_outside_decidable_fragment_code() {
    let module_env = ModuleEnv::new();
    let mut atom = base_atom("diagnostic_nonlinear");
    atom.params = vec![param("x", "i64"), param("y", "i64")];
    atom.ensures = "result == x * y".to_string();

    let diagnostic =
        mumei_core::verification::outside_decidable_fragment_diagnostic(&atom, &module_env)
            .expect("expected diagnostic");

    assert_eq!(diagnostic.code, "outside_decidable_fragment");
    assert_eq!(diagnostic.severity, "warning");
    assert_eq!(diagnostic.atom, "diagnostic_nonlinear");
    assert!(diagnostic
        .tags
        .contains(&"nonlinear_arithmetic".to_string()));
}

#[test]
fn detects_nonlinear_arithmetic_in_symbolic_mul_and_div() {
    let module_env = ModuleEnv::new();

    let mut mul_atom = base_atom("symbolic_mul");
    mul_atom.params = vec![param("x", "i64"), param("y", "i64")];
    mul_atom.ensures = "result == x * y".to_string();
    assert_outside_tag(&mul_atom, &module_env, "nonlinear_arithmetic");

    let mut div_atom = base_atom("symbolic_div");
    div_atom.params = vec![param("x", "i64"), param("y", "i64")];
    div_atom.requires = "y != 0".to_string();
    div_atom.ensures = "result == x / y".to_string();
    assert_outside_tag(&div_atom, &module_env, "nonlinear_arithmetic");
}

#[test]
fn detect_logic_fragment_returns_requested_fragment_categories() {
    let mut module_env = ModuleEnv::new();
    module_env.register_effect(&EffectDef {
        name: "Protocol".to_string(),
        params: Vec::new(),
        constraint: None,
        includes: Vec::new(),
        refinement: None,
        parent: Vec::new(),
        span: Span::default(),
        states: vec!["Idle".to_string(), "Ready".to_string()],
        transitions: vec![EffectTransition {
            operation: "start".to_string(),
            from_state: "Idle".to_string(),
            to_state: "Ready".to_string(),
        }],
        initial_state: Some("Idle".to_string()),
    });

    let mut atom = base_atom("fragment_mix");
    atom.params = vec![param("i", "i64")];
    atom.requires = "i >= 0 && i < n".to_string();
    atom.ensures = "result == arr[i] && exists(j, 0, n, arr[j] == result)".to_string();
    atom.body_expr = "arr[i]".to_string();
    atom.forall_constraints = vec![Quantifier {
        q_type: QuantifierType::ForAll,
        var: "k".to_string(),
        start: "0".to_string(),
        end: "n".to_string(),
        condition: "arr[k] >= 0".to_string(),
    }];
    atom.effects = vec![Effect::simple("Protocol")];

    let fragments = detect_logic_fragment(&atom, &module_env);

    assert!(fragments.contains(&LogicFragment::LinearArithmetic));
    assert!(fragments.contains(&LogicFragment::ArrayAccess));
    assert!(fragments.contains(&LogicFragment::QuantifierAlternation));
    assert!(fragments.contains(&LogicFragment::TemporalState));
}

#[test]
fn detects_array_access_without_explicit_bounds() {
    let module_env = ModuleEnv::new();
    let mut atom = base_atom("unbounded_array_access");
    atom.params = vec![param("i", "i64")];
    atom.ensures = "result == arr[i]".to_string();

    assert_outside_tag(&atom, &module_env, "array_without_bounds");
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

    assert_outside_tag(&atom, &module_env, "quantifier_alternation");
}

#[test]
fn detects_finite_field_helpers_as_lean_bridge_fragment() {
    let module_env = ModuleEnv::new();
    let mut atom = base_atom("ff_zero_eq_zero");
    atom.params = vec![param("p", "i64")];
    atom.requires = "p > 0".to_string();
    atom.ensures = "ff_eq(result, 0, p)".to_string();
    atom.body_expr = "ff_zero(p)".to_string();

    assert_outside_tag(&atom, &module_env, "finite_field");
    let fragments = detect_logic_fragment(&atom, &module_env);
    assert!(fragments.contains(&LogicFragment::FiniteField));
}

#[test]
fn detects_nested_mutable_aliasing() {
    let module_env = ModuleEnv::new();
    let mut atom = base_atom("nested_aliases");
    atom.params = vec![ref_mut_param("left", "i64"), ref_mut_param("right", "i64")];

    let tags = assert_detected_tag(&atom, &module_env, "nested_aliasing");
    assert!(!is_outside_decidable_fragment(&tags));
    assert!(outside_decidable_fragment_warning(&atom, &module_env).is_none());
}

#[test]
fn detects_regex_semantics() {
    let module_env = ModuleEnv::new();
    let mut atom = base_atom("regex_check");
    atom.params = vec![param("s", "Str")];
    atom.requires = "regex_match(s, \"^[a-z]+$\")".to_string();

    let tags = assert_detected_tag(&atom, &module_env, "regex_semantics");
    assert!(!is_outside_decidable_fragment(&tags));
    assert!(outside_decidable_fragment_warning(&atom, &module_env).is_none());
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

    let tags = assert_detected_tag(&atom, &module_env, "trigger_sensitive_quantifier");
    assert!(!is_outside_decidable_fragment(&tags));
    assert!(outside_decidable_fragment_warning(&atom, &module_env).is_none());
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

    assert_outside_tag(&atom, &module_env, "inductive_data_type");
}

#[test]
fn detects_recursive_invariant_from_while_loop() {
    let module_env = ModuleEnv::new();
    let mut atom = base_atom("loop_sum");
    atom.body_expr = "{ let i = 0; while i < n invariant i >= 0 { i = i + 1; }; i }".to_string();

    let tags = assert_detected_tag(&atom, &module_env, "recursive_invariant");
    assert!(!is_outside_decidable_fragment(&tags));
    assert!(outside_decidable_fragment_warning(&atom, &module_env).is_none());
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

    let tags = assert_detected_tag(&atom, &module_env, "complex_temporal_effect");
    assert!(!is_outside_decidable_fragment(&tags));
    assert!(outside_decidable_fragment_warning(&atom, &module_env).is_none());
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

#[test]
fn verify_json_includes_outside_decidable_fragment_diagnostics() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let temp_dir = std::env::temp_dir().join(format!(
        "mumei_decidable_json_{}_{}",
        std::process::id(),
        "diagnostics"
    ));
    if temp_dir.exists() {
        std::fs::remove_dir_all(&temp_dir).expect("clean stale diagnostics temp dir");
    }
    std::fs::create_dir_all(&temp_dir).expect("create diagnostics temp dir");
    let fixture = temp_dir.join("nonlinear.mm");
    std::fs::write(
        &fixture,
        r#"
atom nonlinear(x: i64, y: i64) -> i64
requires: true;
ensures: result == x * y;
body: x * y;
"#,
    )
    .expect("write nonlinear fixture");

    let output = Command::new(bin)
        .arg("verify")
        .arg("--warn-fragment")
        .arg("--json")
        .arg("--report-dir")
        .arg(&temp_dir)
        .arg(&fixture)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify --json: {err}"));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let payload: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|err| panic!("{err}: {stdout}"));
    let diagnostics = payload["diagnostics"]
        .as_array()
        .expect("diagnostics should be an array");
    assert!(
        diagnostics.iter().any(
            |diagnostic| diagnostic["code"] == "outside_decidable_fragment"
                && diagnostic["tags"].as_array().is_some_and(|tags| tags
                    .iter()
                    .any(|tag| tag.as_str() == Some("nonlinear_arithmetic")))
        ),
        "expected outside_decidable_fragment nonlinear diagnostic in {payload}"
    );
    let warnings = payload["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(
        warnings
            .iter()
            .any(|warning| warning["code"] == "outside_decidable_fragment"
                && warning["tags"].as_array().is_some_and(|tags| tags
                    .iter()
                    .any(|tag| tag.as_str() == Some("nonlinear_arithmetic")))),
        "expected outside_decidable_fragment nonlinear warning in {payload}"
    );

    std::fs::remove_dir_all(temp_dir).expect("remove diagnostics temp dir");
}

#[test]
fn verify_stderr_prints_outside_decidable_fragment_warning_with_location_and_hint() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let temp_dir = std::env::temp_dir().join(format!(
        "mumei_decidable_warning_{}_{}",
        std::process::id(),
        "stderr"
    ));
    if temp_dir.exists() {
        std::fs::remove_dir_all(&temp_dir).expect("clean stale warning temp dir");
    }
    std::fs::create_dir_all(&temp_dir).expect("create warning temp dir");
    let fixture = temp_dir.join("vault.mm");
    std::fs::write(
        &fixture,
        "atom nonlinear(x: i64, y: i64) -> i64\nrequires: true;\nensures: result == x * y;\nbody: x * y;\n",
    )
    .expect("write nonlinear fixture");

    let output = Command::new(bin)
        .arg("verify")
        .arg("--warn-fragment")
        .arg("--report-dir")
        .arg(&temp_dir)
        .arg(&fixture)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify: {err}"));

    assert!(
        output.status.success(),
        "verify should succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(
            "warning[outside_decidable_fragment]: atom `nonlinear` uses nonlinear arithmetic (x * y), consider Lean escalation"
        ),
        "missing warning header in stderr:\n{stderr}"
    );
    assert!(
        stderr.contains(&format!("  --> {}:1", fixture.display())),
        "missing warning source location in stderr:\n{stderr}"
    );
    assert!(
        stderr.contains(
            "  hint: simplify to linear arithmetic, or use `mumei verify --escalate-lean` to delegate to Lean 4"
        ),
        "missing warning hint in stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("  see: docs/SPEC_GUIDE.md#decidable-fragment"),
        "missing warning guide link in stderr:\n{stderr}"
    );

    std::fs::remove_dir_all(temp_dir).expect("remove warning temp dir");
}

#[test]
fn verify_does_not_print_fragment_warning_without_flag() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let temp_dir = std::env::temp_dir().join(format!(
        "mumei_decidable_warning_{}_{}",
        std::process::id(),
        "default_off"
    ));
    if temp_dir.exists() {
        std::fs::remove_dir_all(&temp_dir).expect("clean stale warning temp dir");
    }
    std::fs::create_dir_all(&temp_dir).expect("create warning temp dir");
    let fixture = temp_dir.join("vault.mm");
    std::fs::write(
        &fixture,
        "atom nonlinear(x: i64, y: i64) -> i64\nrequires: true;\nensures: result == x * y;\nbody: x * y;\n",
    )
    .expect("write nonlinear fixture");

    let output = Command::new(bin)
        .arg("verify")
        .arg("--report-dir")
        .arg(&temp_dir)
        .arg(&fixture)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify: {err}"));

    assert!(
        output.status.success(),
        "verify should succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("warning[outside_decidable_fragment]"),
        "fragment warning should require --warn-fragment:\n{stderr}"
    );

    std::fs::remove_dir_all(temp_dir).expect("remove warning temp dir");
}

#[test]
fn escalate_lean_promotes_outside_fragment_to_candidate_bundle() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let temp_dir = std::env::temp_dir().join(format!(
        "mumei_decidable_escalation_{}_{}",
        std::process::id(),
        "bundle"
    ));
    if temp_dir.exists() {
        std::fs::remove_dir_all(&temp_dir).expect("clean stale escalation temp dir");
    }
    std::fs::create_dir_all(&temp_dir).expect("create escalation temp dir");
    let fixture = temp_dir.join("nonlinear.mm");
    let bundle_path = temp_dir.join("nonlinear.escalation-bundle.json");
    std::fs::write(
        &fixture,
        r#"
atom nonlinear(x: i64, y: i64) -> i64
requires: true;
ensures: result == x * y;
body: x * y;
"#,
    )
    .expect("write nonlinear fixture");

    let output = Command::new(bin)
        .arg("verify")
        .arg("--escalate-lean")
        .arg("--emit")
        .arg("escalation-bundle")
        .arg("--output")
        .arg(&bundle_path)
        .arg(&fixture)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify --escalate-lean: {err}"));

    assert!(
        output.status.success(),
        "verify --escalate-lean should succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let bundle: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&bundle_path).expect("read escalation bundle"),
    )
    .expect("parse escalation bundle");
    assert_eq!(bundle["summary"]["candidate_count"], 1);
    assert_eq!(bundle["candidates"][0]["name"], "nonlinear");
    assert_eq!(
        bundle["candidates"][0]["escalation_reason"],
        "nonlinear_arithmetic"
    );
    assert_eq!(
        bundle["candidates"][0]["logic_fragment_tag"],
        "nonlinear_arithmetic"
    );
    assert_eq!(bundle["candidates"][0]["status"], "escalation_candidate");

    std::fs::remove_dir_all(temp_dir).expect("remove escalation temp dir");
}

// ---------------------------------------------------------------------------
// P10-D / C-2: bounded low-degree nonlinear arithmetic is routed to nlsat-first
// instead of unconditional Lean escalation.
// ---------------------------------------------------------------------------

#[test]
fn bounded_low_degree_nonlinear_is_nlsat_first_and_not_outside_fragment() {
    use mumei_core::verification::{
        bounded_low_degree_nonlinear_profile, is_nlsat_first_candidate, BOUNDED_NONLINEAR_TAG,
        NLSAT_FIRST_MAX_DEGREE, NLSAT_FIRST_MAX_VARIABLES,
    };
    let module_env = ModuleEnv::new();

    let mut atom = base_atom("bounded_mul");
    atom.params = vec![param("a", "i64"), param("b", "i64")];
    atom.requires = "a >= 0 && a <= 1000 && b >= 0 && b <= 1000".to_string();
    atom.ensures = "result >= 0 && result <= 1000000".to_string();
    atom.body_expr = "a * b".to_string();

    let tags = assert_detected_tag(&atom, &module_env, "nonlinear_arithmetic");
    assert!(
        tags.iter().any(|tag| tag == BOUNDED_NONLINEAR_TAG),
        "{tags:?}"
    );
    assert!(is_nlsat_first_candidate(&tags));
    assert!(!is_outside_decidable_fragment(&tags));
    assert!(outside_decidable_fragment_warning(&atom, &module_env).is_none());

    let profile = bounded_low_degree_nonlinear_profile(&atom).expect("bounded profile");
    assert_eq!(profile.max_degree, 2);
    assert!(profile.max_degree <= NLSAT_FIRST_MAX_DEGREE);
    assert!(profile.variables.len() <= NLSAT_FIRST_MAX_VARIABLES);
    assert_eq!(profile.variables, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn unbounded_nonlinear_stays_on_lean_escalation_path() {
    use mumei_core::verification::{bounded_low_degree_nonlinear_profile, BOUNDED_NONLINEAR_TAG};
    let module_env = ModuleEnv::new();

    // One-sided bound only: not closed on both sides.
    let mut atom = base_atom("half_bounded_mul");
    atom.params = vec![param("a", "i64"), param("b", "i64")];
    atom.requires = "a >= 0 && b >= 0".to_string();
    atom.ensures = "result >= 0".to_string();
    atom.body_expr = "a * b".to_string();
    assert!(bounded_low_degree_nonlinear_profile(&atom).is_none());
    let tags = assert_detected_tag(&atom, &module_env, "nonlinear_arithmetic");
    assert!(!tags.iter().any(|tag| tag == BOUNDED_NONLINEAR_TAG));
    assert_outside_tag(&atom, &module_env, "nonlinear_arithmetic");
}

#[test]
fn high_degree_or_too_many_variables_stay_on_lean_escalation_path() {
    use mumei_core::verification::bounded_low_degree_nonlinear_profile;
    let module_env = ModuleEnv::new();

    // Degree 3 exceeds NLSAT_FIRST_MAX_DEGREE even though x is bounded.
    let mut cube = base_atom("cube");
    cube.params = vec![param("x", "i64")];
    cube.requires = "x >= 0 && x <= 20".to_string();
    cube.ensures = "result >= 0".to_string();
    cube.body_expr = "x * x * x".to_string();
    assert!(bounded_low_degree_nonlinear_profile(&cube).is_none());
    assert_outside_tag(&cube, &module_env, "nonlinear_arithmetic");

    // Four distinct bounded variables exceed NLSAT_FIRST_MAX_VARIABLES.
    let mut many = base_atom("many_vars");
    many.params = vec![
        param("a", "i64"),
        param("b", "i64"),
        param("c", "i64"),
        param("d", "i64"),
    ];
    many.requires =
        "a >= 0 && a <= 10 && b >= 0 && b <= 10 && c >= 0 && c <= 10 && d >= 0 && d <= 10"
            .to_string();
    many.ensures = "result >= 0".to_string();
    many.body_expr = "a * b + c * d".to_string();
    assert!(bounded_low_degree_nonlinear_profile(&many).is_none());
    assert_outside_tag(&many, &module_env, "nonlinear_arithmetic");
}

#[test]
fn symbolic_divisor_requires_zero_excluding_bounds_for_nlsat_first() {
    use mumei_core::verification::bounded_low_degree_nonlinear_profile;
    let module_env = ModuleEnv::new();

    let mut ok = base_atom("bounded_div");
    ok.params = vec![param("a", "i64"), param("b", "i64")];
    ok.requires = "a >= 0 && a <= 1000 && b >= 1 && b <= 1000".to_string();
    ok.ensures = "result >= 0".to_string();
    ok.body_expr = "a / b".to_string();
    assert!(bounded_low_degree_nonlinear_profile(&ok).is_some());
    assert!(!is_outside_decidable_fragment(&detect_logic_fragment_tags(
        &ok,
        &module_env
    )));

    // Divisor bounds include zero → stays on the Lean path.
    let mut zero = base_atom("div_zero_in_range");
    zero.params = vec![param("a", "i64"), param("b", "i64")];
    zero.requires = "a >= 0 && a <= 1000 && b >= 0 && b <= 1000 && b != 0".to_string();
    zero.ensures = "result >= 0".to_string();
    zero.body_expr = "a / b".to_string();
    assert!(bounded_low_degree_nonlinear_profile(&zero).is_none());
    assert_outside_tag(&zero, &module_env, "nonlinear_arithmetic");
}

#[test]
fn finite_field_and_modulo_are_never_nlsat_first() {
    use mumei_core::verification::{
        bounded_low_degree_nonlinear_profile, is_nlsat_first_candidate,
    };
    let module_env = ModuleEnv::new();

    let mut modulo = base_atom("bounded_mod");
    modulo.params = vec![param("a", "i64"), param("b", "i64")];
    modulo.requires = "a >= 0 && a <= 1000 && b >= 1 && b <= 1000".to_string();
    modulo.ensures = "result >= 0".to_string();
    modulo.body_expr = "a % b".to_string();
    assert!(bounded_low_degree_nonlinear_profile(&modulo).is_none());
    assert!(!is_nlsat_first_candidate(&detect_logic_fragment_tags(
        &modulo,
        &module_env
    )));

    let mut ff = base_atom("ff_bounded");
    ff.params = vec![param("a", "i64"), param("b", "i64")];
    ff.requires = "a >= 0 && a <= 6 && b >= 0 && b <= 6".to_string();
    ff.ensures = "ff_eq(result, ff_mul(a, b))".to_string();
    ff.body_expr = "a * b".to_string();
    assert!(bounded_low_degree_nonlinear_profile(&ff).is_none());
    let tags = detect_logic_fragment_tags(&ff, &module_env);
    assert!(!is_nlsat_first_candidate(&tags));
    assert!(is_outside_decidable_fragment(&tags));
}

fn run_escalation_bundle(fixture: &std::path::Path, tag: &str) -> serde_json::Value {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let temp_dir =
        std::env::temp_dir().join(format!("mumei_nlsat_first_{}_{}", std::process::id(), tag));
    if temp_dir.exists() {
        std::fs::remove_dir_all(&temp_dir).expect("clean stale nlsat temp dir");
    }
    std::fs::create_dir_all(&temp_dir).expect("create nlsat temp dir");
    let bundle_path = temp_dir.join("bundle.json");
    let output = Command::new(bin)
        .arg("verify")
        .arg("--escalate-lean")
        .arg("--emit")
        .arg("escalation-bundle")
        .arg("--output")
        .arg(&bundle_path)
        .arg(fixture)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify --escalate-lean: {err}"));
    assert!(
        output.status.success(),
        "verify --escalate-lean should succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let bundle: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&bundle_path).expect("read escalation bundle"),
    )
    .expect("parse escalation bundle");
    std::fs::remove_dir_all(temp_dir).expect("remove nlsat temp dir");
    bundle
}

#[test]
fn nlsat_first_fixture_is_decided_by_z3_and_not_escalated() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/nlsat_first/bounded_low_degree_decided.mm");
    let bundle = run_escalation_bundle(&fixture, "decided");
    assert_eq!(
        bundle["summary"]["candidate_count"], 0,
        "bounded low-degree nonlinear atoms must be decided by nlsat, got {bundle}"
    );
}

#[test]
fn nlsat_first_fixture_outside_window_keeps_lean_escalation() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/nlsat_first/nonlinear_lean_demotion.mm");
    let bundle = run_escalation_bundle(&fixture, "demotion");
    assert_eq!(bundle["summary"]["candidate_count"], 3, "{bundle}");
    for candidate in bundle["candidates"].as_array().expect("candidates array") {
        assert_eq!(candidate["escalation_reason"], "nonlinear_arithmetic");
        assert_eq!(candidate["logic_fragment_tag"], "nonlinear_arithmetic");
        assert_eq!(candidate["status"], "escalation_candidate");
    }
}
