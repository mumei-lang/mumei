use std::collections::HashMap;

use mumei_core::hir::lower_atom_to_hir_with_env;
use mumei_core::parser::{Atom, Param, Span, TrustLevel};
use mumei_core::verification::{build_data_flow_trace, ModuleEnv};

fn span(line: usize) -> Span {
    Span {
        file: "tests/test_data_flow_trace.mm".to_string(),
        line,
        col: 1,
        len: 1,
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
        return_type: None,
        span: span(5),
        effect_pre: HashMap::new(),
        effect_post: HashMap::new(),
    }
}

#[test]
fn test_data_flow_trace_simple_assignment() {
    let module_env = ModuleEnv::new();
    let mut atom = base_atom("withdraw_balance");
    atom.params = vec![param("balance", "i64"), param("amount", "i64")];
    atom.ensures = "balance >= 0".to_string();
    atom.body_expr = "{ balance = balance - amount; balance }".to_string();

    let hir_atom = lower_atom_to_hir_with_env(&atom, Some(&module_env));
    let model = HashMap::from([("balance".to_string(), 100), ("amount".to_string(), 150)]);
    let trace =
        build_data_flow_trace(&atom, &model, &module_env, &hir_atom).expect("expected trace");

    assert_eq!(trace.initial_state.len(), 2);
    assert!(trace
        .initial_state
        .iter()
        .any(|state| state.name == "balance" && state.value == "100"));
    assert!(trace
        .initial_state
        .iter()
        .any(|state| state.name == "amount" && state.value == "150"));
    assert!(trace.execution_path.iter().any(|step| {
        step.expression == "balance = (balance - amount)"
            && step.mutations.iter().any(|mutation| {
                mutation.name == "balance" && mutation.before == "100" && mutation.after == "-50"
            })
    }));
}

#[test]
fn test_data_flow_trace_violation_localization() {
    let module_env = ModuleEnv::new();
    let mut atom = base_atom("negative_result");
    atom.params = vec![param("x", "i64")];
    atom.ensures = "result >= 0".to_string();
    atom.body_expr = "{ let y = x - 3; y }".to_string();
    atom.span = span(15);

    let hir_atom = lower_atom_to_hir_with_env(&atom, Some(&module_env));
    let model = HashMap::from([("x".to_string(), 1)]);
    let trace =
        build_data_flow_trace(&atom, &model, &module_env, &hir_atom).expect("expected trace");

    assert_eq!(trace.violation.line, 15);
    assert_eq!(trace.violation.contract_type, "ensures");
    assert_eq!(trace.violation.expression, "result >= 0");
    assert_eq!(trace.violation.evaluated_as, "-2 >= 0 (FALSE)");
}
