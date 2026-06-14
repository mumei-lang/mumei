use mumei_core::parser::{parse_module, Atom, EffectDef, Item};
use mumei_core::verification::{
    detect_logic_fragment, outside_decidable_fragment_diagnostic, LogicFragment, ModuleEnv,
};

fn load_fixture(name: &str) -> (Atom, ModuleEnv) {
    let source = std::fs::read_to_string(format!(
        "{}/tests/fixtures/{}",
        env!("CARGO_MANIFEST_DIR"),
        name
    ))
    .expect("read fixture");
    let items = parse_module(&source);
    let mut env = ModuleEnv::new();
    let mut atom = None;

    for item in items {
        match item {
            Item::Atom(parsed_atom) => {
                env.register_atom(&parsed_atom);
                atom = Some(parsed_atom);
            }
            Item::EffectDef(effect_def) => register_effect(&mut env, &effect_def),
            _ => {}
        }
    }

    (atom.expect("fixture atom"), env)
}

fn register_effect(env: &mut ModuleEnv, effect_def: &EffectDef) {
    env.register_effect(effect_def);
}

#[test]
fn detects_linear_arithmetic_fixture() {
    let (atom, env) = load_fixture("fragment_linear.mm");
    let fragments = detect_logic_fragment(&atom, &env);

    assert!(fragments.contains(&LogicFragment::LinearArithmetic));
    assert!(!fragments.contains(&LogicFragment::NonlinearArithmetic));
    assert!(outside_decidable_fragment_diagnostic(&atom, &env).is_none());
}

#[test]
fn detects_nonlinear_arithmetic_fixture() {
    let (atom, env) = load_fixture("fragment_nonlinear.mm");
    let fragments = detect_logic_fragment(&atom, &env);
    let diagnostic = outside_decidable_fragment_diagnostic(&atom, &env).expect("diagnostic");

    assert!(fragments.contains(&LogicFragment::NonlinearArithmetic));
    assert_eq!(diagnostic.code, "outside_decidable_fragment");
    assert!(diagnostic.message.contains("a * b"));
}

#[test]
fn detects_array_access_fixture() {
    let (atom, env) = load_fixture("fragment_array.mm");
    let fragments = detect_logic_fragment(&atom, &env);

    assert!(fragments.contains(&LogicFragment::ArrayAccess));
    assert!(outside_decidable_fragment_diagnostic(&atom, &env).is_none());
}

#[test]
fn detects_quantifier_alternation_fixture() {
    let (atom, env) = load_fixture("fragment_quantifier.mm");
    let fragments = detect_logic_fragment(&atom, &env);
    let diagnostic = outside_decidable_fragment_diagnostic(&atom, &env).expect("diagnostic");

    assert!(fragments.contains(&LogicFragment::QuantifierAlternation));
    assert!(diagnostic.message.contains("forall exists"));
}

#[test]
fn detects_temporal_state_fixture() {
    let (atom, env) = load_fixture("fragment_temporal.mm");
    let fragments = detect_logic_fragment(&atom, &env);

    assert!(fragments.contains(&LogicFragment::TemporalState));
    assert!(outside_decidable_fragment_diagnostic(&atom, &env).is_none());
}
