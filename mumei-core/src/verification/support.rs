#![allow(unused_imports)]
mod call_graph;
mod dataflow_inference;
mod effects;
mod law_verification;
mod resource_safety;

pub(crate) use law_verification::{
    contains_method_call, replace_word, split_args, substitute_method_calls,
};
pub use law_verification::{verify_impl, verify_impl_with_options};

pub(crate) use effects::{
    check_constant_constraint, evaluate_string_constraint, parse_constraint_to_z3_string,
    save_effect_polymorphism_report, save_effect_propagation_report, save_effect_violation_report,
    verify_effect_consistency, verify_effect_containment, verify_effect_params, EffectCtx,
};
pub use effects::{AllowedEffect, SecurityPolicy};

pub(crate) use resource_safety::{
    collect_acquire_resources_expr, collect_acquire_resources_stmt, verify_async_recursion_depth,
    verify_bmc_resource_safety, verify_resource_hierarchy, ResourceCtx, BMC_DEFAULT_UNROLL_DEPTH,
    MAX_ASYNC_RECURSION_DEPTH,
};

pub(crate) use call_graph::{
    check_taint_propagation, collect_array_accesses, collect_array_accesses_in_stmt,
    collect_array_accesses_inner, collect_callees_expr, collect_callees_stmt,
    collect_callees_with_args_expr, collect_callees_with_args_stmt, detect_call_cycle,
    expr_to_source_string, verify_atom_invariant, verify_call_graph_cycles,
};

pub use dataflow_inference::{
    build_data_flow_trace, infer_contracts_json, infer_effects_json, DataFlowTrace, ExecutionStep,
    VariableMutation, VariableState, ViolationInfo,
};
pub(crate) use dataflow_inference::{
    collect_divisors_expr, collect_divisors_stmt, infer_effects, infer_ensures, infer_requires,
};
