#![allow(unused_imports)]
mod constraints;
mod context;
mod expr;
pub(crate) mod may_write;
mod pattern;
mod stmt;
mod struct_invariants;
mod temporal;
pub(crate) mod z3_types;

pub(crate) use constraints::{
    apply_refinement_constraint, assert_result_equality, check_contract_subsumption,
    propagate_equality_from_ensures,
};
pub use context::DEFAULT_CONSTRAINT_BUDGET;
pub(crate) use context::{
    check_constraint_budget, profile_solver_assertion, profile_solver_check, profiler_checkpoint,
    LocalLambda, VCtx,
};

pub(crate) use expr::termination_violation_error;
pub(crate) use expr::{
    discard_obligations_since, discharge_bv_div_obligations, discharge_bv_shift_obligations,
    discharge_i64_overflow_obligations, div_safety_status, expr_to_z3,
    expr_to_z3_with_domain_obligations, has_new_obligations, obligation_marks,
    rebind_deferred_obligations, refined_predicate_to_z3, shift_range_status, DivSafetyStatus,
    ShiftRangeStatus,
};
pub(crate) use pattern::{
    detect_enum_from_arms, format_counterexample, pattern_bind_variables, pattern_to_z3_condition,
};
pub(crate) use stmt::{
    reject_parameter_projection_slot_collisions, reject_struct_projection_path_collisions,
    stmt_to_z3,
};
pub(crate) use struct_invariants::{
    alias_struct_fields, assume_struct_contract, assume_struct_invariants, bind_struct_fields,
    check_struct_invariants, field_type_predicates, fresh_struct_handle, lower_struct_contract,
    nested_field_bindings, project_struct_fields, seed_struct_fields, struct_field_key,
    struct_fields_of_value, STRUCT_FIELD_PREFIX,
};
pub(crate) use temporal::{
    assert_temporal_effect_transition, is_temporal_witness_predicate, temporal_witness_call_to_z3,
};
pub(crate) use z3_types::{
    array_element_sort, array_element_sort_from_type, array_element_type_from_annotation,
    array_element_type_name, array_len_for_value, array_len_key, array_len_symbol, array_len_value,
    array_root_len_key, array_root_len_symbol, array_root_len_value, as_bv_i64, as_int_like,
    atom_stores_to_array, call_result_len, coerce_array_store_value, coerce_to_array_elem_sort,
    coerce_to_float, concrete_len_value, float_eq, float_from_f64, havoc_array_args,
    havoc_array_name, havoc_written_callee_fields, index_in_bounds, is_admissible_trigger,
    is_bv_i64, mark_string_constraints, merge_arm_env_into, merge_branch_envs, nonneg_constraint,
    param_z3_value, raw_z3_context, real_from_f64, round_nearest_even,
    seed_tuple_result_components, stmt_stores_to_var, tail_len_expr, tuple_component_types,
    tuple_result_arity_key, tuple_result_component_key, unify_branch_sorts, wire_array_slots,
    z3_array_for_name, z3_array_for_sort, z3_dynamic_array, z3_range_type_name, ArrayElementSort,
    F64_EBITS, F64_SBITS, I64_BITS, UNSUPPORTED_TUPLE_RESULT_INDEXING,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_array_element_sort_from_type_uses_lowered_types() {
        let module_env = crate::verification::ModuleEnv::new();
        assert_eq!(
            array_element_sort_from_type("f64", &module_env, false),
            ArrayElementSort::Real
        );
        assert_eq!(
            array_element_sort_from_type("bool", &module_env, false),
            ArrayElementSort::Bool
        );
        assert_eq!(
            array_element_sort_from_type("i64", &module_env, false),
            ArrayElementSort::Int
        );
        assert_eq!(
            array_element_sort_from_type("Str", &module_env, false),
            ArrayElementSort::Str
        );
        // `[[T]]` elements are arrays themselves — no scalar sort exists.
        assert_eq!(
            array_element_sort_from_type("[f64]", &module_env, false),
            ArrayElementSort::Nested
        );
        assert_eq!(
            array_element_sort_from_type("[i64]", &module_env, false),
            ArrayElementSort::Nested
        );
    }

    #[test]
    fn test_array_element_sort_from_type_ieee754_uses_float() {
        let module_env = crate::verification::ModuleEnv::new();
        assert_eq!(
            array_element_sort_from_type("f64", &module_env, true),
            ArrayElementSort::Float
        );
        // Non-`f64` element types are unaffected by the IEEE 754 flag.
        assert_eq!(
            array_element_sort_from_type("i64", &module_env, true),
            ArrayElementSort::Int
        );
        assert_eq!(
            array_element_sort_from_type("bool", &module_env, true),
            ArrayElementSort::Bool
        );
    }
}
