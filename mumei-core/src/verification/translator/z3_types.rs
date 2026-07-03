#![allow(unused_imports)]
use super::super::support::*;
use super::super::*;
use super::*;
use crate::lowering::{lower, LoweredType};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArrayElementSort {
    Int,
    Real,
    Bool,
}

pub(crate) fn array_element_type_from_annotation(
    type_name: Option<&str>,
    module_env: &ModuleEnv,
) -> String {
    let Some(ty) = type_name else {
        return "i64".to_string();
    };
    if ty.starts_with('[') && ty.ends_with(']') {
        module_env.resolve_base_type(ty[1..ty.len() - 1].trim())
    } else {
        "i64".to_string()
    }
}

pub(crate) fn array_element_type_name(name: &str, vc: &VCtx<'_>) -> String {
    vc.current_atom
        .and_then(|atom| atom.params.iter().find(|param| param.name == name))
        .and_then(|param| param.type_name.as_deref())
        .map(|ty| array_element_type_from_annotation(Some(ty), vc.module_env))
        .unwrap_or_else(|| "i64".to_string())
}

pub(crate) fn array_element_sort_from_type(type_name: &str) -> ArrayElementSort {
    match lower(type_name) {
        LoweredType::F64 => ArrayElementSort::Real,
        LoweredType::Bool => ArrayElementSort::Bool,
        _ => ArrayElementSort::Int,
    }
}

pub(crate) fn array_element_sort(name: &str, vc: &VCtx<'_>) -> ArrayElementSort {
    array_element_sort_from_type(&array_element_type_name(name, vc))
}

/// Convert an `f64` literal to a Z3 `Real` (exact rational) value.
///
/// `f64` is currently verified under Z3 `Real` sort, not
/// IEEE 754 `Float` sort. The literal `0.1` is interpreted here as the rational
/// `1/10` — not as the binary64 approximation `0x3FB999999999999A`. Properties
/// depending on IEEE 754 semantics (rounding, subnormals, NaN/Infinity, the
/// fact that `0.1 + 0.2 != 0.3` in IEEE 754) are *not* modeled. When IEEE 754-
/// faithful verification is required, swap this for `Float::from_f64(ctx, value)`
/// and re-introduce the `Float` arithmetic branch in `expr_to_z3` (see also the
/// `param_z3_value` `f64` branch and `Expr::Float` lowering). See
/// `docs/ARCHITECTURE.md` § "`f64` Verification Sort: Real (not IEEE 754 Float)".
pub(crate) fn real_from_f64<'a>(ctx: &'a Context, value: f64) -> Real<'a> {
    let formatted = value.to_string();
    if let Some((num, frac)) = formatted.split_once('.') {
        let mut denominator = String::from("1");
        denominator.extend(std::iter::repeat_n('0', frac.len()));
        let numerator = format!("{}{}", num, frac);
        Real::from_real_str(ctx, &numerator, &denominator)
            .unwrap_or_else(|| Real::from_real(ctx, 0, 1))
    } else {
        Real::from_real_str(ctx, &formatted, "1").unwrap_or_else(|| Real::from_real(ctx, 0, 1))
    }
}

pub(crate) fn mark_string_constraints(vc: &VCtx<'_>) {
    if let Some(cell) = vc.has_string_constraints {
        cell.set(true);
    }
}

pub(crate) fn z3_array_for_sort<'a>(
    ctx: &'a Context,
    name: &str,
    sort: ArrayElementSort,
) -> Array<'a> {
    let int_sort = z3::Sort::int(ctx);
    match sort {
        ArrayElementSort::Int => Array::new_const(ctx, name, &int_sort, &int_sort),
        ArrayElementSort::Real => {
            let real_sort = z3::Sort::real(ctx);
            Array::new_const(ctx, name, &int_sort, &real_sort)
        }
        ArrayElementSort::Bool => {
            let bool_sort = z3::Sort::bool(ctx);
            Array::new_const(ctx, name, &int_sort, &bool_sort)
        }
    }
}

pub(crate) fn z3_array_for_name<'a>(vc: &VCtx<'a>, name: &str) -> Array<'a> {
    z3_array_for_sort(vc.ctx, name, array_element_sort(name, vc))
}

pub(crate) fn z3_dynamic_array<'a>(vc: &VCtx<'a>, name: &str, env: &Env<'a>) -> Array<'a> {
    let arr_key = format!("__z3_arr_{}", name);
    env.get(&arr_key)
        .and_then(|d| d.as_array())
        .unwrap_or_else(|| z3_array_for_name(vc, name))
}

pub(crate) fn coerce_array_store_value<'a>(
    vc: &VCtx<'a>,
    array: &str,
    value: Dynamic<'a>,
) -> DynResult<'a> {
    match array_element_sort(array, vc) {
        ArrayElementSort::Int => value
            .as_int()
            .map(Into::into)
            .ok_or_else(|| MumeiError::type_error("Array store value must be integer")),
        ArrayElementSort::Real => value
            .as_real()
            .or_else(|| value.as_int().map(|i| i.to_real()))
            .map(Into::into)
            .ok_or_else(|| MumeiError::type_error("Array store value must be real")),
        ArrayElementSort::Bool => value
            .as_bool()
            .map(Into::into)
            .ok_or_else(|| MumeiError::type_error("Array store value must be boolean")),
    }
}

pub(crate) fn param_z3_value<'a>(
    ctx: &'a Context,
    name: &str,
    type_name: Option<&str>,
    module_env: &ModuleEnv,
) -> Dynamic<'a> {
    let base = type_name
        .map(|t| module_env.resolve_base_type(t))
        .unwrap_or_else(|| "i64".to_string());
    if type_name.is_some_and(|ty| ty.starts_with('[') && ty.ends_with(']')) {
        z3_array_for_sort(
            ctx,
            name,
            array_element_sort_from_type(&array_element_type_from_annotation(
                type_name, module_env,
            )),
        )
        .into()
    } else {
        // TODO(strict-preservation): `lower()` unifies `Str`/`String` into
        // `LoweredType::Str`, so `"String"` now encodes as a Z3 string sort.
        // Pre-P1-b only `"Str"` did; `"String"` fell through to `Int`. This is
        // an intentional consistency fix (no `.mm` fixture declares `String`).
        // For exact legacy behavior, distinguish the spelling at the `lower()`
        // layer rather than re-adding a string match. Mirrors the note in
        // mumei-emit-llvm `resolve_param_type`.
        match lower(&base) {
            // `f64` params are encoded as Z3 `Real` (exact rationals), not IEEE 754.
            // See `real_from_f64` and
            // `docs/ARCHITECTURE.md` § "`f64` Verification Sort".
            LoweredType::F64 => Real::new_const(ctx, name).into(),
            LoweredType::Str => Z3String::new_const(ctx, name).into(),
            LoweredType::Bool => Bool::new_const(ctx, name).into(),
            _ => Int::new_const(ctx, name).into(),
        }
    }
}
