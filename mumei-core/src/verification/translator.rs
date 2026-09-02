#![allow(unused_imports)]
mod constraints;
mod context;
mod expr;
mod pattern;
mod stmt;
mod struct_invariants;
mod temporal;
mod z3_types;

pub(crate) use constraints::{
    apply_refinement_constraint, assert_result_equality, check_contract_subsumption,
    propagate_equality_from_ensures,
};
pub use context::DEFAULT_CONSTRAINT_BUDGET;
pub(crate) use context::{
    check_constraint_budget, profile_solver_assertion, profile_solver_check, profiler_checkpoint,
    VCtx,
};
pub(crate) use expr::{
    discharge_bv_shift_obligations, expr_to_z3, shift_range_status, ShiftRangeStatus,
};
pub(crate) use pattern::{
    detect_enum_from_arms, format_counterexample, pattern_bind_variables, pattern_to_z3_condition,
};
pub(crate) use stmt::stmt_to_z3;
pub(crate) use struct_invariants::{
    alias_struct_fields, assume_struct_contract, assume_struct_invariants, bind_struct_fields,
    check_struct_invariants, fresh_struct_handle, seed_struct_fields, struct_field_key,
    struct_fields_of_value,
};
pub(crate) use temporal::{
    assert_temporal_effect_transition, is_temporal_witness_predicate, temporal_witness_call_to_z3,
};
pub(crate) use z3_types::{
    array_element_sort, array_element_sort_from_type, array_element_type_from_annotation,
    array_element_type_name, array_len_value, as_bv_i64, as_int_like, coerce_array_store_value,
    coerce_to_float, float_eq, float_from_f64, index_in_bounds, is_admissible_trigger, is_bv_i64,
    mark_string_constraints, param_z3_value, real_from_f64, round_nearest_even,
    seed_tuple_result_components, tuple_component_types, tuple_result_arity_key,
    tuple_result_component_key, unify_branch_sorts, z3_array_for_name, z3_array_for_sort,
    z3_dynamic_array, ArrayElementSort, F64_EBITS, F64_SBITS, I64_BITS,
    UNSUPPORTED_TUPLE_RESULT_INDEXING,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_array_element_sort_from_type_uses_lowered_types() {
        assert_eq!(
            array_element_sort_from_type("f64", false),
            ArrayElementSort::Real
        );
        assert_eq!(
            array_element_sort_from_type("bool", false),
            ArrayElementSort::Bool
        );
        assert_eq!(
            array_element_sort_from_type("i64", false),
            ArrayElementSort::Int
        );
        assert_eq!(
            array_element_sort_from_type("[f64]", false),
            ArrayElementSort::Int
        );
    }

    #[test]
    fn test_array_element_sort_from_type_ieee754_uses_float() {
        assert_eq!(
            array_element_sort_from_type("f64", true),
            ArrayElementSort::Float
        );
        // Non-`f64` element types are unaffected by the IEEE 754 flag.
        assert_eq!(
            array_element_sort_from_type("i64", true),
            ArrayElementSort::Int
        );
        assert_eq!(
            array_element_sort_from_type("bool", true),
            ArrayElementSort::Bool
        );
    }
}
