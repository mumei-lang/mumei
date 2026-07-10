#![allow(unused_imports)]
use super::super::support::*;
use super::super::*;
use super::*;
use crate::lowering::{lower, LoweredType};
use crate::parser::parse_type_ref;
use z3::ast::Ast as _;

/// Prefix for tuple result bindings; it cannot collide with source identifiers.
pub(crate) const TUPLE_RESULT_PREFIX: &str = "__mumei_tuple_result_";
pub(crate) const UNSUPPORTED_TUPLE_RESULT_INDEXING: &str = "Unsupported tuple result indexing:";

pub(crate) fn tuple_component_types(return_type: Option<&str>) -> Option<Vec<String>> {
    parse_type_ref(return_type?.trim())
        .tuple_element_types()
        .map(|components| {
            components
                .iter()
                .map(|component| component.display_name())
                .collect()
        })
}

pub(crate) fn tuple_result_component_key(binding: &str, index: usize) -> String {
    format!("{TUPLE_RESULT_PREFIX}{binding}_{index}")
}

pub(crate) fn tuple_result_arity_key(binding: &str) -> String {
    format!("{TUPLE_RESULT_PREFIX}{binding}_arity")
}

pub(crate) fn seed_tuple_result_components<'a>(
    ctx: &'a Context,
    env: &mut Env<'a>,
    binding: &str,
    return_type: Option<&str>,
    module_env: &ModuleEnv,
    ieee754_f64: bool,
) {
    let Some(components) = tuple_component_types(return_type) else {
        return;
    };
    env.insert(
        tuple_result_arity_key(binding),
        Int::from_i64(ctx, components.len() as i64).into(),
    );
    for (index, component_type) in components.iter().enumerate() {
        let key = tuple_result_component_key(binding, index);
        env.insert(
            key.clone(),
            param_z3_value(ctx, &key, Some(component_type), module_env, ieee754_f64),
        );
    }
}

/// Number of exponent / significand bits for IEEE 754 binary64 (`f64`).
///
/// binary64 has an 11-bit exponent and a 53-bit significand (52 stored +
/// 1 implicit). `Sort::float(11, 53)` / `Float::from_f64` (which uses
/// `Sort::double`) both denote this sort.
pub(crate) const F64_EBITS: u32 = 11;
pub(crate) const F64_SBITS: u32 = 53;

/// Extract the raw `z3_sys::Z3_context` backing a `z3::Context`.
///
/// `z3` 0.12's `Context` is a single-field newtype over
/// `z3_sys::Z3_context` (`pub struct Context { z3_ctx: Z3_context }`), but
/// that field is private and the crate exposes no accessor. The IEEE 754
/// floating-point theory helpers below need the raw context to call the
/// `Z3_mk_fpa_*` builders that `z3` 0.12 does not wrap (notably the
/// round-nearest-ties-to-even rounding mode and real→float coercion). Reading
/// the pointer at offset 0 is sound for a single-field struct: the field lives
/// at the start of the struct and the pointer value is `Copy`.
fn raw_z3_context(ctx: &Context) -> z3_sys::Z3_context {
    // Catch a layout change (e.g. an added field) in a future `z3` upgrade at
    // compile time rather than as silent undefined behavior.
    const _: () =
        assert!(std::mem::size_of::<Context>() == std::mem::size_of::<z3_sys::Z3_context>());
    unsafe { *(ctx as *const Context as *const z3_sys::Z3_context) }
}

/// The IEEE 754 round-nearest-ties-to-even rounding mode.
///
/// This is the default rounding mode used by hardware `f64` arithmetic, so it
/// is the faithful choice for `--ieee754-f64` verification (e.g. it makes
/// `0.1 + 0.2 != 0.3` hold, which round-toward-zero would not).
///
/// The returned AST has Z3's `RoundingMode` sort, not a floating-point sort;
/// it is deliberately wrapped as `Float` because that is the receiver type the
/// `z3` crate's `Float::add`/`sub`/`mul`/`div` use for the rounding-mode
/// argument of `Z3_mk_fpa_add` etc. It must only be passed in that position,
/// never used as a floating-point operand.
pub(crate) fn round_nearest_even(ctx: &Context) -> Float<'_> {
    let raw = raw_z3_context(ctx);
    let rne = unsafe { z3_sys::Z3_mk_fpa_round_nearest_ties_to_even(raw) };
    unsafe { Float::wrap(ctx, rne) }
}

/// Convert an `f64` literal to a Z3 IEEE 754 binary64 `Float` numeral.
///
/// Unlike `real_from_f64`, this preserves the exact binary64 bit pattern of
/// the literal (e.g. `0.1` becomes `0x3FB999999999999A`, not the rational
/// ⅒), modeling true IEEE 754 semantics.
pub(crate) fn float_from_f64<'a>(ctx: &'a Context, value: f64) -> Float<'a> {
    Float::from_f64(ctx, value)
}

/// Coerce a `Dynamic` value to an IEEE 754 binary64 `Float`.
///
/// Already-`Float` values pass through unchanged. `Real` and `Int` operands
/// (e.g. a mixed `f64`/integer subexpression) are lowered into binary64 via
/// the FP theory's real→float conversion under the given rounding mode.
pub(crate) fn coerce_to_float<'a>(
    ctx: &'a Context,
    value: &Dynamic<'a>,
    rne: &Float<'a>,
) -> Option<Float<'a>> {
    if let Some(f) = value.as_float() {
        return Some(f);
    }
    let real = value
        .as_real()
        .or_else(|| value.as_int().map(|i| i.to_real()))?;
    let raw = raw_z3_context(ctx);
    let sort = unsafe { z3_sys::Z3_mk_fpa_sort_double(raw) };
    let ast =
        unsafe { z3_sys::Z3_mk_fpa_to_fp_real(raw, rne.get_z3_ast(), real.get_z3_ast(), sort) };
    Some(unsafe { Float::wrap(ctx, ast) })
}

/// IEEE 754 floating-point equality (`fp.eq`).
///
/// This differs from structural equality (`Z3_mk_eq`): `NaN != NaN` and
/// `+0.0 == -0.0` under `fp.eq`, matching runtime `f64` comparison.
pub(crate) fn float_eq<'a>(ctx: &'a Context, lhs: &Float<'a>, rhs: &Float<'a>) -> Bool<'a> {
    let raw = raw_z3_context(ctx);
    let ast = unsafe { z3_sys::Z3_mk_fpa_eq(raw, lhs.get_z3_ast(), rhs.get_z3_ast()) };
    unsafe { Bool::wrap(ctx, ast) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArrayElementSort {
    Int,
    Real,
    /// IEEE 754 binary64 elements, selected for `[f64]` arrays only under
    /// the opt-in `--ieee754-f64` mode (default `f64` arrays use `Real`).
    Float,
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

pub(crate) fn array_element_sort_from_type(type_name: &str, ieee754_f64: bool) -> ArrayElementSort {
    match lower(type_name) {
        LoweredType::F64 if ieee754_f64 => ArrayElementSort::Float,
        LoweredType::F64 => ArrayElementSort::Real,
        LoweredType::Bool => ArrayElementSort::Bool,
        _ => ArrayElementSort::Int,
    }
}

pub(crate) fn array_element_sort(name: &str, vc: &VCtx<'_>) -> ArrayElementSort {
    array_element_sort_from_type(&array_element_type_name(name, vc), vc.ieee754_f64)
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
        ArrayElementSort::Float => {
            let float_sort = z3::Sort::float(ctx, F64_EBITS, F64_SBITS);
            Array::new_const(ctx, name, &int_sort, &float_sort)
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
        ArrayElementSort::Float => {
            let rne = round_nearest_even(vc.ctx);
            coerce_to_float(vc.ctx, &value, &rne)
                .map(Into::into)
                .ok_or_else(|| MumeiError::type_error("Array store value must be float"))
        }
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
    ieee754_f64: bool,
) -> Dynamic<'a> {
    let base = type_name
        .map(|t| module_env.resolve_base_type(t))
        .unwrap_or_else(|| "i64".to_string());
    if type_name.is_some_and(|ty| ty.starts_with('[') && ty.ends_with(']')) {
        z3_array_for_sort(
            ctx,
            name,
            array_element_sort_from_type(
                &array_element_type_from_annotation(type_name, module_env),
                ieee754_f64,
            ),
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
            // `f64` params default to Z3 `Real` (exact rationals). Under the
            // opt-in `--ieee754-f64` mode they are instead declared as IEEE 754
            // binary64 `Float`. See `real_from_f64` / `float_from_f64` and
            // `docs/ARCHITECTURE.md` § "`f64` Verification Sort".
            LoweredType::F64 if ieee754_f64 => {
                Float::new_const(ctx, name, F64_EBITS, F64_SBITS).into()
            }
            LoweredType::F64 => Real::new_const(ctx, name).into(),
            LoweredType::Str => Z3String::new_const(ctx, name).into(),
            LoweredType::Bool => Bool::new_const(ctx, name).into(),
            _ => Int::new_const(ctx, name).into(),
        }
    }
}
