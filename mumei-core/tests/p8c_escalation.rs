use std::collections::HashMap;

use mumei_core::parser::{Atom, Param, Span, TrustLevel};
use mumei_core::proof_cert::{self, EscalationReason, LogicFragment};
use mumei_core::verification::{
    classify_atom_for_lean_escalation, primary_logic_fragment_tag, ModuleEnv,
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
        return_type: None,
        span: Span::default(),
        effect_pre: HashMap::new(),
        effect_post: HashMap::new(),
    }
}

fn param(name: &str) -> Param {
    Param {
        name: name.to_string(),
        type_name: Some("i64".to_string()),
        type_ref: None,
        is_ref: false,
        is_ref_mut: false,
        fn_contract_requires: None,
        fn_contract_ensures: None,
    }
}

#[test]
fn unknown_nonlinear_obligation_gets_typed_p8c_metadata() {
    let module_env = ModuleEnv::new();
    let mut atom = base_atom("nonlinear_bound");
    atom.params = vec![param("x"), param("y")];
    atom.ensures = "result >= x * y".to_string();

    let classification = classify_atom_for_lean_escalation(&atom, &module_env, "unknown", "failed");

    assert!(classification.should_escalate);
    assert_eq!(
        classification.escalation_reason,
        Some(EscalationReason::NonlinearArithmetic)
    );
    assert_eq!(
        classification.logic_fragment_tag,
        Some(LogicFragment::NonlinearArithmetic)
    );
}

#[test]
fn proof_certificate_serializes_escalation_reason_fragment_and_lean_result() {
    let module_env = ModuleEnv::new();
    let mut atom = base_atom("nonlinear_bound");
    atom.params = vec![param("x"), param("y")];
    atom.ensures = "result >= x * y".to_string();
    let results = HashMap::from([(
        atom.name.clone(),
        ("unknown".to_string(), "escalation_candidate".to_string()),
    )]);
    let cert = proof_cert::generate_certificate(
        "complex_proof.mm",
        &[&atom],
        &results,
        &module_env,
        None,
        None,
        None,
    );
    let bundle = proof_cert::generate_escalation_bundle(&cert);
    let payload = serde_json::to_value(bundle).expect("serialize escalation bundle");

    assert_eq!(
        payload["candidates"][0]["escalation_reason"],
        "nonlinear_arithmetic"
    );
    assert_eq!(
        payload["candidates"][0]["logic_fragment_tag"],
        "nonlinear_arithmetic"
    );
    assert_eq!(
        primary_logic_fragment_tag(&["array_without_bounds".to_string()]),
        LogicFragment::ArrayAccess
    );
}
