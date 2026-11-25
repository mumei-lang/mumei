#![allow(unused_imports)]
use super::super::support::*;
use super::super::*;
use super::*;
use crate::lowering::{lower, LoweredType};
use crate::parser::expr::stmt_assigns_var;
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
    vc: &VCtx<'a>,
    env: &mut Env<'a>,
    binding: &str,
    return_type: Option<&str>,
) {
    let Some(components) = tuple_component_types(return_type) else {
        return;
    };
    env.insert(
        tuple_result_arity_key(binding),
        Int::from_i64(vc.ctx, components.len() as i64).into(),
    );
    for (index, component_type) in components.iter().enumerate() {
        let key = tuple_result_component_key(binding, index);
        env.insert(
            key.clone(),
            crate::verification::support::datatype::param_z3_value_for_vc(
                vc,
                &key,
                Some(component_type),
            ),
        );
    }
}

/// Bit width of the `BV` sort used for `i64` under the opt-in `--bitvec-i64`
/// mode. Machine `i64` is 64 bits wide and two's complement, so `BV(64)` with
/// the signed comparison / division operators models it exactly (including
/// wraparound on `+`, `-`, `*`).
pub(crate) const I64_BITS: u32 = 64;

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
pub(crate) fn raw_z3_context(ctx: &Context) -> z3_sys::Z3_context {
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
        .or_else(|| as_int_like(value).map(|i| i.to_real()))?;
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

/// Decode a 64-bit bitvector as an IEEE 754 binary64 `Float` — the inverse
/// of `ieee_bv_from_float`. The composite profile's `f64` leaf keeps its
/// bit-pattern carrier (`pure-i64-composite/v19`); arithmetic decodes it,
/// computes and re-encodes, so an i64/f64 slot confusion still compares
/// different bytes rather than silently widening.
pub(crate) fn float_from_ieee_bv<'a>(ctx: &'a Context, bits: &BV<'a>) -> Float<'a> {
    let raw = raw_z3_context(ctx);
    let sort = unsafe { z3_sys::Z3_mk_fpa_sort_double(raw) };
    let ast = unsafe { z3_sys::Z3_mk_fpa_to_fp_bv(raw, bits.get_z3_ast(), sort) };
    unsafe { Float::wrap(ctx, ast) }
}

/// The canonical quiet-NaN payload `ieee_bv_from_float` collapses every
/// NaN result to.
const CANONICAL_QUIET_NAN64: u64 = 0x7FF8_0000_0000_0000;

/// Encode an IEEE 754 binary64 `Float` back to its 64-bit bitvector.
/// `fp.to_ieee_bv` is partial on a NaN input — any NaN bit pattern
/// satisfies it — and the emitted `fadd`/`fsub`/`fmul`/`fdiv` likewise
/// produce an unspecified NaN payload, so both sides of the comparison
/// collapse a NaN to the canonical quiet-NaN pattern: the claim compares
/// NaN-ness, never the payload.
pub(crate) fn ieee_bv_from_float<'a>(ctx: &'a Context, value: &Float<'a>) -> BV<'a> {
    let raw = raw_z3_context(ctx);
    let ast = unsafe { z3_sys::Z3_mk_fpa_to_ieee_bv(raw, value.get_z3_ast()) };
    let bits = unsafe { BV::wrap(ctx, ast) };
    let nan = unsafe { z3_sys::Z3_mk_fpa_is_nan(raw, value.get_z3_ast()) };
    let nan = unsafe { Bool::wrap(ctx, nan) };
    nan.ite(&BV::from_u64(ctx, CANONICAL_QUIET_NAN64, 64), &bits)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArrayElementSort {
    Int,
    Real,
    /// IEEE 754 binary64 elements, selected for `[f64]` arrays only under
    /// the opt-in `--ieee754-f64` mode (default `f64` arrays use `Real`).
    Float,
    Bool,
    /// `Str` elements — encoded on Z3's `Seq`/`String` sort so `a[i] == "s"`
    /// and `a[i] = "s"` go through the same string theory as scalar `Str`.
    Str,
    /// A finite algebraic datatype. The actual range sort is resolved from
    /// the declared element type at array construction sites.
    Enum,
    /// A struct whose flattened leaves all have scalar sorts. The carrier
    /// remains `Int`-ranged; its fields live in per-leaf arrays.
    Struct,
    /// A struct with leaves that cannot be represented by scalar arrays.
    UnsupportedStruct,
    /// An enum that cannot be represented as a finite scalar ADT array
    /// element (recursive, unresolved generic, or array-bearing payload).
    UnsupportedEnum,
    /// `[[T]]` — the element is itself an array, which no scalar element sort
    /// can represent. Arrays carrying this tag are built with the genuine
    /// nested range sort (see `z3_element_sort`), so `a[i]` yields an `Array`
    /// value that scalar uses reject — never a phantom `Int`.
    Nested,
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
    if let Some(local_ty) = vc.local_array_elem_types.borrow().get(name) {
        return local_ty.clone();
    }
    vc.current_atom
        .and_then(|atom| atom.params.iter().find(|param| param.name == name))
        .and_then(|param| param.type_name.as_deref())
        .map(|ty| array_element_type_from_annotation(Some(ty), vc.module_env))
        .unwrap_or_else(|| "i64".to_string())
}

fn enum_array_leaf_shape_supported(
    module_env: &ModuleEnv,
    type_name: &str,
    visiting: &mut std::collections::HashSet<String>,
) -> bool {
    let resolved = module_env.resolve_base_type(type_name);
    let base = crate::verification::support::datatype::type_name_base(&resolved);
    let Some(enum_def) = module_env
        .get_enum(&resolved)
        .or_else(|| module_env.get_enum(base))
    else {
        return false;
    };
    if !enum_def.type_params.is_empty()
        || !visiting.insert(enum_def.name.clone())
        || !crate::verification::support::datatype::is_finite_adt(enum_def, module_env)
    {
        return false;
    }
    for variant in &enum_def.variants {
        for field in &variant.field_types {
            let field_type = crate::verification::support::datatype::payload_field_type_name(field);
            let Some(leaves) =
                crate::verification::support::datatype::field_leaf_types(&field_type, module_env)
            else {
                return false;
            };
            for (_, leaf) in leaves {
                if leaf == "array_len" || leaf.starts_with("array:") {
                    return false;
                }
                if let Some(inner) = leaf.strip_prefix("enum:") {
                    if !enum_array_leaf_shape_supported(module_env, inner, visiting) {
                        return false;
                    }
                }
            }
        }
    }
    visiting.remove(&enum_def.name);
    true
}

pub(crate) fn array_element_sort_from_type(
    type_name: &str,
    module_env: &ModuleEnv,
    ieee754_f64: bool,
) -> ArrayElementSort {
    let type_name = type_name.strip_prefix("enum:").unwrap_or(type_name);
    let resolved = module_env.resolve_base_type(type_name);
    let enum_base = crate::verification::support::datatype::type_name_base(&resolved);
    if let Some(enum_def) = module_env
        .get_enum(&resolved)
        .or_else(|| module_env.get_enum(enum_base))
    {
        return if enum_array_leaf_shape_supported(
            module_env,
            &enum_def.name,
            &mut Default::default(),
        ) {
            ArrayElementSort::Enum
        } else {
            ArrayElementSort::UnsupportedEnum
        };
    }
    let struct_base = crate::verification::support::datatype::type_name_base(&resolved);
    if module_env
        .get_struct(&resolved)
        .or_else(|| module_env.get_struct(struct_base))
        .is_some()
    {
        return if struct_array_leaf_types(&resolved, module_env).is_some() {
            ArrayElementSort::Struct
        } else {
            ArrayElementSort::UnsupportedStruct
        };
    }
    match lower(&resolved) {
        LoweredType::F64 if ieee754_f64 => ArrayElementSort::Float,
        LoweredType::F64 => ArrayElementSort::Real,
        LoweredType::Bool => ArrayElementSort::Bool,
        LoweredType::Str => ArrayElementSort::Str,
        // `[T]` elements — i.e. a `[[U]]` array — have no scalar sort.
        LoweredType::Array(_) => ArrayElementSort::Nested,
        _ => ArrayElementSort::Int,
    }
}

pub(crate) fn array_element_sort(name: &str, vc: &VCtx<'_>) -> ArrayElementSort {
    let type_name = array_element_type_name(name, vc);
    array_element_sort_from_type(&type_name, vc.module_env, vc.ieee754_f64)
}

pub(crate) fn struct_array_leaf_types(
    type_name: &str,
    module_env: &ModuleEnv,
) -> Option<Vec<(String, String)>> {
    let resolved = module_env.resolve_base_type(type_name);
    let base = crate::verification::support::datatype::type_name_base(&resolved);
    module_env
        .get_struct(&resolved)
        .or_else(|| module_env.get_struct(base))?;
    let leaves = crate::verification::support::datatype::field_leaf_types(&resolved, module_env)?;
    if leaves.is_empty()
        || leaves.iter().any(|(_, leaf)| {
            if let Some(enum_type) = leaf.strip_prefix("enum:") {
                !enum_array_leaf_shape_supported(module_env, enum_type, &mut Default::default())
            } else {
                !matches!(
                    lower(&module_env.resolve_base_type(leaf)),
                    LoweredType::I64
                        | LoweredType::I32
                        | LoweredType::U64
                        | LoweredType::U32
                        | LoweredType::F64
                        | LoweredType::Bool
                        | LoweredType::Str
                )
            }
        })
    {
        return None;
    }
    Some(leaves)
}

pub(crate) fn validate_array_element_type(
    vc: &VCtx<'_>,
    type_name: &str,
) -> Result<(), MumeiError> {
    validate_enum_array_element_type(vc, type_name)?;
    let sort = array_element_sort_from_type(type_name, vc.module_env, vc.ieee754_f64);
    if sort == ArrayElementSort::Struct {
        if let Some(leaves) = struct_array_leaf_types(type_name, vc.module_env) {
            for (_, leaf_type) in leaves {
                if let Some(enum_type) = leaf_type.strip_prefix("enum:") {
                    validate_enum_array_element_type(vc, enum_type)?;
                }
            }
        }
    }
    if sort == ArrayElementSort::UnsupportedStruct {
        return Err(MumeiError::type_error(format!(
            "array element struct '{type_name}' must have only scalar leaves (i64, fixed-width integers, f64, bool, or Str)"
        )));
    }
    if sort == ArrayElementSort::Nested {
        let resolved = vc.module_env.resolve_base_type(type_name);
        let inner = resolved
            .strip_prefix("[]<")
            .and_then(|inner| inner.strip_suffix('>'))
            .or_else(|| {
                resolved
                    .strip_prefix('[')
                    .and_then(|inner| inner.strip_suffix(']'))
            })
            .map(str::trim)
            .unwrap_or("");
        if matches!(
            array_element_sort_from_type(inner, vc.module_env, vc.ieee754_f64),
            ArrayElementSort::Struct | ArrayElementSort::UnsupportedStruct
        ) {
            return Err(MumeiError::type_error(
                "nested arrays of structs are not supported",
            ));
        }
    }
    Ok(())
}

pub(crate) fn struct_array_leaf_key(name: &str, leaf_path: &str) -> String {
    format!("__struct_arr#{name}#{leaf_path}")
}

pub(crate) fn struct_array_leaf_array<'a>(
    vc: &VCtx<'a>,
    name: &str,
    leaf_path: &str,
    leaf_type: &str,
    env: &mut Env<'a>,
) -> z3::ast::Array<'a> {
    let key = struct_array_leaf_key(name, leaf_path);
    if let Some(array) = env.get(&key).and_then(|value| value.as_array()) {
        return array;
    }
    let array = z3_fresh_array_for_range(vc.ctx, &key, &z3_element_sort(vc, leaf_type));
    env.insert(key, array.clone().into());
    array
}

pub(crate) fn seed_struct_array_param_leaves<'a>(
    vc: &VCtx<'a>,
    name: &str,
    type_name: Option<&str>,
    env: &mut Env<'a>,
) {
    let Some(type_name) = type_name.filter(|ty| ty.starts_with('[') && ty.ends_with(']')) else {
        return;
    };
    let element_type = array_element_type_from_annotation(Some(type_name), vc.module_env);
    if array_element_sort_from_type(&element_type, vc.module_env, vc.ieee754_f64)
        != ArrayElementSort::Struct
    {
        return;
    }
    let Some(leaves) = struct_array_leaf_types(&element_type, vc.module_env) else {
        return;
    };
    vc.local_array_elem_types
        .borrow_mut()
        .insert(name.to_string(), element_type.clone());
    array_len_value(vc.ctx, env, name, vc.bitvec_i64, None);
    for (path, leaf_type) in leaves {
        let key = struct_array_leaf_key(name, &path);
        env.entry(key.clone()).or_insert_with(|| {
            z3_fresh_array_for_range(vc.ctx, &key, &z3_element_sort(vc, &leaf_type)).into()
        });
    }
}

pub(crate) fn validate_enum_array_element_type(
    vc: &VCtx<'_>,
    type_name: &str,
) -> Result<(), MumeiError> {
    fn validate(
        vc: &VCtx<'_>,
        type_name: &str,
        visiting: &mut std::collections::HashSet<String>,
    ) -> Result<(), MumeiError> {
        let resolved = vc.module_env.resolve_base_type(type_name);
        let base = crate::verification::support::datatype::type_name_base(&resolved);
        let Some(enum_def) = vc
            .module_env
            .get_enum(&resolved)
            .or_else(|| vc.module_env.get_enum(base))
        else {
            return Ok(());
        };
        if !enum_def.type_params.is_empty()
            || !visiting.insert(enum_def.name.clone())
            || !crate::verification::support::datatype::is_finite_adt(enum_def, vc.module_env)
            || crate::verification::support::datatype::datatype_sort_for_type(vc, type_name)
                .is_none()
        {
            return Err(MumeiError::type_error(format!(
                "array element enum '{type_name}' is unsupported; enum arrays require a resolved finite ADT"
            )));
        }
        for variant in &enum_def.variants {
            for field in &variant.field_types {
                let field_type =
                    crate::verification::support::datatype::payload_field_type_name(field);
                let leaves = crate::verification::support::datatype::field_leaf_types(
                    &field_type,
                    vc.module_env,
                )
                .ok_or_else(|| {
                    MumeiError::type_error(format!(
                        "array element enum '{type_name}' has an unsupported payload"
                    ))
                })?;
                for (_, leaf) in leaves {
                    if leaf == "array_len" || leaf.starts_with("array:") {
                        return Err(MumeiError::type_error(format!(
                            "array element enum '{type_name}' has an array-bearing payload, which is unsupported"
                        )));
                    }
                    if let Some(inner) = leaf.strip_prefix("enum:") {
                        validate(vc, inner, visiting)?;
                    }
                }
            }
        }
        visiting.remove(&enum_def.name);
        Ok(())
    }

    validate(vc, type_name, &mut std::collections::HashSet::new())
}

pub(crate) fn validate_enum_array_type(vc: &VCtx<'_>, type_name: &str) -> Result<(), MumeiError> {
    fn validate(
        vc: &VCtx<'_>,
        type_name: &str,
        visiting_structs: &mut std::collections::HashSet<String>,
    ) -> Result<(), MumeiError> {
        let resolved = vc.module_env.resolve_base_type(type_name);
        if resolved.trim_start().starts_with('[') || resolved.trim_end().ends_with("[]") {
            let element_type = array_element_type_from_annotation(Some(type_name), vc.module_env);
            let element_sort =
                array_element_sort_from_type(&element_type, vc.module_env, vc.ieee754_f64);
            if visiting_structs.is_empty()
                && matches!(
                    element_sort,
                    ArrayElementSort::Struct | ArrayElementSort::UnsupportedStruct
                )
            {
                validate_array_element_type(vc, &element_type)?;
            } else {
                validate_enum_array_element_type(vc, &element_type)?;
            }
        } else if let Some(struct_def) = vc.module_env.get_struct(&resolved) {
            if visiting_structs.insert(struct_def.name.clone()) {
                for field in &struct_def.fields {
                    validate(vc, &field.type_name, visiting_structs)?;
                }
                visiting_structs.remove(&struct_def.name);
            }
        }
        Ok(())
    }
    validate(vc, type_name, &mut std::collections::HashSet::new())
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

/// Bridge a `Dynamic` to the `Int` encoding (`bv2int`, signed).
///
/// Under `--bitvec-i64` an `i64` value is a `BV(64)`, but several encodings
/// stay on the `Int` sort regardless of the mode — array indices, `[i64]`
/// elements, and the `Real`/`Float` coercions for `f64`. This is the
/// single BV → Int conversion point: it applies Z3's signed `bv2int` so the
/// two's complement value is preserved.
pub(crate) fn as_int_like<'a>(value: &Dynamic<'a>) -> Option<Int<'a>> {
    value
        .as_int()
        .or_else(|| value.as_bv().map(|bv| bv_to_int_signed(&bv)))
}

/// Signed `bv2int`, without an `ite`.
///
/// Z3's signed `bv2int` branches on the sign bit, and a term holding an `ite`
/// is rejected as an E-matching trigger — which loses quantifier
/// instantiation on every array contract bridged this way. Flipping the sign
/// bit and reading the result as unsigned yields the signed value biased by
/// `2^63`, so subtracting the bias gives the same integer out of a term Z3 can
/// still match on.
pub(crate) fn bv_to_int_signed<'a>(bv: &BV<'a>) -> Int<'a> {
    let ctx = bv.get_ctx();
    let sign_bit = BV::from_u64(ctx, 1u64 << (I64_BITS - 1), I64_BITS);
    let bias = Int::from_u64(ctx, 1u64 << (I64_BITS - 1));
    Int::sub(ctx, &[&bv.bvxor(&sign_bit).to_int(false), &bias])
}

/// Bridge a `Dynamic` to the `BV(64)` encoding (`int2bv`).
///
/// This is the Int → BV conversion point, used when a contract mixes an `Int`
/// term (an integer literal, an array element, a call result of an atom
/// verified in `Int` mode) with a `BV(64)` term. Wider/narrower bit-vectors
/// are rejected rather than silently resized.
pub(crate) fn as_bv_i64<'a>(value: &Dynamic<'a>) -> Option<BV<'a>> {
    if let Some(bv) = value.as_bv() {
        return (bv.get_size() == I64_BITS).then_some(bv);
    }
    let int = value.as_int()?;
    // A literal becomes a bit-vector numeral directly: Z3's `int2bv` of a
    // non-numeral term is expensive (it bit-blasts the integer encoding), and
    // an `int2bv` wrapper around a constant is enough to push a trivial goal
    // like `(x & 1) == 1` into `unknown`.
    match int.as_i64() {
        Some(literal) => Some(BV::from_i64(int.get_ctx(), literal, I64_BITS)),
        None => Some(BV::from_int(&int, I64_BITS)),
    }
}

/// `ite(c, t, e)` over two `Dynamic`s of the same sort. Returns `None` when
/// the sorts differ or are not a sort an `ite` can merge here.
pub(crate) fn dynamic_ite<'a>(
    c: &Bool<'a>,
    t: &Dynamic<'a>,
    e: &Dynamic<'a>,
) -> Option<Dynamic<'a>> {
    if t.get_sort() != e.get_sort() {
        return None;
    }
    match t.get_sort().kind() {
        z3::SortKind::Int => Some(c.ite(&t.as_int()?, &e.as_int()?).into()),
        z3::SortKind::Bool => Some(c.ite(&t.as_bool()?, &e.as_bool()?).into()),
        z3::SortKind::Real => Some(c.ite(&t.as_real()?, &e.as_real()?).into()),
        z3::SortKind::BV => Some(c.ite(&t.as_bv()?, &e.as_bv()?).into()),
        z3::SortKind::Array => Some(c.ite(&t.as_array()?, &e.as_array()?).into()),
        z3::SortKind::FloatingPoint => Some(c.ite(&t.as_float()?, &e.as_float()?).into()),
        z3::SortKind::Datatype => Some(c.ite(&t.as_datatype()?, &e.as_datatype()?).into()),
        z3::SortKind::Seq => Some(c.ite(&t.as_string()?, &e.as_string()?).into()),
        _ => None,
    }
}

pub(crate) fn dynamic_fresh_const<'a>(
    ctx: &'a Context,
    prefix: &str,
    template: &Dynamic<'a>,
) -> Option<Dynamic<'a>> {
    let sort = template.get_sort();
    match sort.kind() {
        z3::SortKind::Bool => Some(z3::ast::Bool::fresh_const(ctx, prefix).into()),
        z3::SortKind::Int => Some(z3::ast::Int::fresh_const(ctx, prefix).into()),
        z3::SortKind::Real => Some(z3::ast::Real::fresh_const(ctx, prefix).into()),
        z3::SortKind::BV => {
            Some(z3::ast::BV::fresh_const(ctx, prefix, template.as_bv()?.get_size()).into())
        }
        z3::SortKind::FloatingPoint => Some(
            z3::ast::Float::fresh_const(
                ctx,
                prefix,
                sort.float_exponent_size()?,
                sort.float_significand_size()?,
            )
            .into(),
        ),
        z3::SortKind::Datatype => Some(z3::ast::Datatype::fresh_const(ctx, prefix, &sort).into()),
        z3::SortKind::Seq => Some(z3::ast::String::fresh_const(ctx, prefix).into()),
        _ => None,
    }
}

/// Merge the variable environments produced by running an `if`'s then/else
/// branches in isolation: every variable becomes `ite(cond, then_val,
/// else_val)`. Keys only present on one side (declared inside one branch)
/// keep that side's value, matching the previous env-leak behavior. Values of
/// sorts `dynamic_ite` cannot merge keep the else-branch value — the same
/// last-write-wins order the shared-env execution had.
pub(crate) fn merge_branch_envs<'a>(
    env: &mut Env<'a>,
    then_env: Env<'a>,
    else_env: Env<'a>,
    c: &Bool<'a>,
) {
    let mut merged: Env<'a> = HashMap::new();
    for (key, then_val) in then_env.iter() {
        if key.starts_with(ARRAY_SHARED_PREFIX) {
            let else_val = else_env.get(key);
            let value = if is_shared_marker_value(then_val) {
                then_val
            } else if let Some(else_val) = else_val.filter(|value| is_shared_marker_value(value)) {
                else_val
            } else {
                then_val
            };
            merged.insert(key.clone(), value.clone());
            continue;
        }
        let merged_val = match else_env.get(key) {
            Some(else_val) => {
                dynamic_ite(c, then_val, else_val).unwrap_or_else(|| else_val.clone())
            }
            None => then_val.clone(),
        };
        merged.insert(key.clone(), merged_val);
    }
    for (key, else_val) in else_env.iter() {
        if !then_env.contains_key(key) {
            merged.insert(key.clone(), else_val.clone());
        }
    }
    *env = merged;
}

/// Fold one match arm's writes to pre-existing variables into the
/// accumulated post-match env: `acc[k] = ite(cond, arm[k], acc[k])`.
/// Bindings introduced by the pattern — including ones that shadow a
/// pre-existing outer name (`match p { P(x, ..) => .. }` while `x` is
/// already bound) — stay arm-local and are not merged; names in `bound`
/// are skipped. Values `dynamic_ite` cannot merge keep the accumulated
/// value — the previous behavior of dropping arm-side writes.
pub(crate) fn merge_arm_env_into<'a>(
    merged: &mut Option<Env<'a>>,
    arm_env: &Env<'a>,
    base_env: &Env<'a>,
    cond: &Bool<'a>,
    bound: &std::collections::HashSet<String>,
) {
    let first_arm = merged.is_none();
    let acc = merged.get_or_insert_with(|| base_env.clone());
    for (key, base_val) in base_env.iter() {
        if key.starts_with(ARRAY_SHARED_PREFIX) {
            continue;
        }
        if bound.contains(key) {
            continue;
        }
        let arm_val = arm_env
            .get(key)
            .cloned()
            .unwrap_or_else(|| base_val.clone());
        if let Some(acc_val) = acc.get(key) {
            if let Some(m) = dynamic_ite(cond, &arm_val, acc_val) {
                acc.insert(key.clone(), m);
            }
        }
    }
    let marker_keys: std::collections::HashSet<String> = base_env
        .keys()
        .chain(arm_env.keys())
        .filter(|key| key.starts_with(ARRAY_SHARED_PREFIX))
        .cloned()
        .collect();
    for key in marker_keys {
        if key
            .strip_prefix(ARRAY_SHARED_PREFIX)
            .is_some_and(|name| bound.contains(name))
        {
            if let Some(value) = base_env.get(&key) {
                acc.insert(key, value.clone());
            } else {
                acc.remove(&key);
            }
            continue;
        }
        let arm_val = arm_env.get(&key).or_else(|| base_env.get(&key));
        if first_arm {
            if let Some(value) = arm_val {
                acc.insert(key, value.clone());
            } else {
                acc.remove(&key);
            }
            continue;
        }
        let acc_val = acc.get(&key);
        let value = match (acc_val, arm_val) {
            (Some(value), _) if is_shared_marker_value(value) => Some(value.clone()),
            (_, Some(value)) if is_shared_marker_value(value) => Some(value.clone()),
            (Some(value), _) => Some(value.clone()),
            (_, Some(value)) => Some(value.clone()),
            (None, None) => None,
        };
        if let Some(value) = value {
            acc.insert(key, value);
        } else {
            acc.remove(&key);
        }
    }
    // Arm-local `__struct_<handle>_<field>` slots (each arm's
    // StructInit registers its fresh handle's fields) must survive the
    // merge: `project_struct_fields` reads them back through the folded
    // `ite` over handles, so dropping them leaves a match-produced
    // struct with unconstrained fields. They take their arm-local value
    // verbatim — the `ite` selects the right handle per path.
    for (key, arm_val) in arm_env.iter() {
        if key.starts_with(ARRAY_SHARED_PREFIX) {
            continue;
        }
        if !(key.starts_with(STRUCT_FIELD_PREFIX) || key.starts_with(ARRAY_VALUE_LEN_PREFIX))
            || base_env.contains_key(key)
            || bound.contains(key)
        {
            continue;
        }
        if !acc.contains_key(key) {
            acc.insert(key.clone(), arm_val.clone());
        }
    }
}

/// Put the two branches of an `ite` into a common sort.
///
/// A branch may be `BV(64)` while the other one is an `Int` (a literal, an
/// array element, the result of an atom verified in `Int` mode); Z3 rejects an
/// `ite` over mixed sorts, so the `Int` side is bridged to `BV(64)`. Branches
/// that stay in unrelated sorts are reported as a type error, since Z3 answers
/// such an `ite` with a null AST.
pub(crate) fn unify_branch_sorts<'a>(
    then_value: Dynamic<'a>,
    else_value: Dynamic<'a>,
) -> MumeiResult<(Dynamic<'a>, Dynamic<'a>)> {
    let unified = match (is_bv_i64(&then_value), is_bv_i64(&else_value)) {
        (true, false) => match as_bv_i64(&else_value) {
            Some(bv) => (then_value, bv.into()),
            None => (then_value, else_value),
        },
        (false, true) => match as_bv_i64(&then_value) {
            Some(bv) => (bv.into(), else_value),
            None => (then_value, else_value),
        },
        _ => (then_value, else_value),
    };
    let unified = unify_numeric_branch_sorts(unified);
    if unified.0.get_sort() != unified.1.get_sort() {
        return Err(MumeiError::type_error(format!(
            "branches of a conditional must have the same type, got {} and {}",
            unified.0.get_sort(),
            unified.1.get_sort()
        )));
    }
    Ok(unified)
}

/// Widen an integer branch to the `f64` sort of the other branch.
///
/// An atom returning `f64` may still yield an `i64` in one branch (`if c { x }
/// else { 1.5 }`), which is the same widening applied to a mixed `f64`
/// subexpression.
fn unify_numeric_branch_sorts<'a>(
    (then_value, else_value): (Dynamic<'a>, Dynamic<'a>),
) -> (Dynamic<'a>, Dynamic<'a>) {
    let ctx = then_value.get_ctx();
    let widen = |value: &Dynamic<'a>, target: &Dynamic<'a>| -> Option<Dynamic<'a>> {
        if target.as_float().is_some() {
            let rne = round_nearest_even(ctx);
            return coerce_to_float(ctx, value, &rne).map(Into::into);
        }
        if target.as_real().is_some() {
            return as_int_like(value).map(|i| i.to_real().into());
        }
        None
    };
    let then_is_int = as_int_like(&then_value).is_some() && then_value.as_real().is_none();
    let else_is_int = as_int_like(&else_value).is_some() && else_value.as_real().is_none();
    match (then_is_int, else_is_int) {
        (true, false) => match widen(&then_value, &else_value) {
            Some(widened) => (widened, else_value),
            None => (then_value, else_value),
        },
        (false, true) => match widen(&else_value, &then_value) {
            Some(widened) => (then_value, widened),
            None => (then_value, else_value),
        },
        _ => (then_value, else_value),
    }
}

/// True when a term is usable as an E-matching trigger.
///
/// Z3 refuses a pattern containing an `ite` — which is what the signed
/// `bv2int` bridge of an `i64` index or element expands into — and answers with
/// a null AST that the `z3` crate turns into a panic.
pub(crate) fn is_admissible_trigger(term: &Dynamic<'_>) -> bool {
    if term.kind() == z3::AstKind::App && term.decl().kind() == z3::DeclKind::ITE {
        return false;
    }
    term.children().iter().all(is_admissible_trigger)
}

/// True when the value is encoded as a 64-bit bit-vector.
pub(crate) fn is_bv_i64(value: &Dynamic<'_>) -> bool {
    value.as_bv().is_some_and(|bv| bv.get_size() == I64_BITS)
}

/// Symbolic length of an array, in the sort of the active `i64` encoding.
///
/// Lengths follow the mode so that index comparisons (`i < len(arr)`) stay in
/// one theory: mixing `bv2int` on the index with `int2bv` on the length makes
/// Z3 answer `unknown` on otherwise trivial array contracts.
pub(crate) fn array_len_symbol<'a>(ctx: &'a Context, name: &str, bitvec_i64: bool) -> Dynamic<'a> {
    let len_name = array_len_key(name);
    if bitvec_i64 {
        BV::new_const(ctx, len_name.as_str(), I64_BITS).into()
    } else {
        Int::new_const(ctx, len_name.as_str()).into()
    }
}

pub(crate) fn array_len_key(name: &str) -> String {
    format!("len#{name}")
}

pub(crate) const ARRAY_SHARED_PREFIX: &str = "__array_shared#";

pub(crate) fn array_shared_key(name: &str) -> String {
    format!("{ARRAY_SHARED_PREFIX}{name}")
}

pub(crate) fn is_read_only_array_builtin(name: &str) -> bool {
    matches!(
        name,
        "len"
            | "matches"
            | "match_regex"
            | "re_match"
            | "starts_with"
            | "ends_with"
            | "contains"
            | "not_contains"
            | "is_empty"
            | "index_of"
            | "substr"
            | "char_at"
    )
}

pub(crate) fn is_array_shared(env: &Env<'_>, name: &str) -> bool {
    env.get(&array_shared_key(name))
        .is_some_and(is_shared_marker_value)
}

pub(crate) fn with_array_shared_markers_restored<'a, T>(
    env: &mut Env<'a>,
    f: impl FnOnce(&mut Env<'a>) -> T,
) -> T {
    let saved = env
        .iter()
        .filter(|(key, _)| key.starts_with(ARRAY_SHARED_PREFIX))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Vec<_>>();
    let result = f(env);
    env.retain(|key, _| !key.starts_with(ARRAY_SHARED_PREFIX));
    env.extend(saved);
    result
}

pub(crate) fn mark_array_shared<'a>(ctx: &'a Context, env: &mut Env<'a>, name: &str) {
    env.insert(array_shared_key(name), Bool::from_bool(ctx, true).into());
}

pub(crate) fn clear_array_shared<'a>(ctx: &'a Context, env: &mut Env<'a>, name: &str) {
    let key = array_shared_key(name);
    if env.contains_key(&key) {
        env.insert(key, Bool::from_bool(ctx, false).into());
    }
}

fn is_shared_marker_value(value: &Dynamic<'_>) -> bool {
    value
        .as_bool()
        .and_then(|marker| marker.as_bool())
        .unwrap_or(false)
}

pub(crate) fn shared_array_mutation_error(name: &str) -> MumeiError {
    MumeiError::type_error(format!(
        "array '{name}' shares its buffer with a struct field or enum payload; mutating it is not supported — rebuild the field with a field write instead"
    ))
}

pub(crate) fn array_root_len_key(name: &str) -> String {
    format!("len#root#{name}")
}

pub(crate) fn array_root_len_symbol<'a>(
    ctx: &'a Context,
    name: &str,
    bitvec_i64: bool,
) -> Dynamic<'a> {
    let len_name = array_root_len_key(name);
    if bitvec_i64 {
        BV::new_const(ctx, len_name.as_str(), I64_BITS).into()
    } else {
        Int::new_const(ctx, len_name.as_str()).into()
    }
}

/// `value >= 0`, signed in the bit-vector encoding.
pub(crate) fn nonneg_constraint<'a>(ctx: &'a Context, value: &Dynamic<'a>) -> Option<Bool<'a>> {
    if let Some(bv) = value.as_bv() {
        let zero = BV::from_i64(ctx, 0, bv.get_size());
        return Some(bv.bvsge(&zero));
    }
    value.as_int().map(|i| i.ge(&Int::from_i64(ctx, 0)))
}

/// `0 <= index < len`, compared in the bit-vector encoding when the length is
/// a `BV(64)` and in `Int` otherwise.
pub(crate) fn index_in_bounds<'a>(
    ctx: &'a Context,
    index: &Dynamic<'a>,
    len: &Dynamic<'a>,
) -> Option<Bool<'a>> {
    if is_bv_i64(len) {
        let (idx, len) = (as_bv_i64(index)?, as_bv_i64(len)?);
        let zero = BV::from_i64(ctx, 0, I64_BITS);
        return Some(Bool::and(ctx, &[&idx.bvsge(&zero), &idx.bvslt(&len)]));
    }
    let (idx, len) = (as_int_like(index)?, as_int_like(len)?);
    Some(Bool::and(
        ctx,
        &[&idx.ge(&Int::from_i64(ctx, 0)), &idx.lt(&len)],
    ))
}

/// Look up (or create and constrain) the `len_<array>` symbol in `env`.
pub(crate) fn array_len_value<'a>(
    ctx: &'a Context,
    env: &mut Env<'a>,
    array: &str,
    bitvec_i64: bool,
    solver_opt: Option<&Solver<'a>>,
) -> Dynamic<'a> {
    let len_name = array_len_key(array);
    if let Some(existing) = env.get(&len_name) {
        return existing.clone();
    }
    let len = array_len_symbol(ctx, array, bitvec_i64);
    if let (Some(solver), Some(nonneg)) = (solver_opt, nonneg_constraint(ctx, &len)) {
        solver.assert(&nonneg);
    }
    env.insert(len_name, len.clone());
    len
}

pub(crate) fn array_root_len_value<'a>(
    ctx: &'a Context,
    env: &mut Env<'a>,
    root: &str,
    bitvec_i64: bool,
    solver_opt: Option<&Solver<'a>>,
) -> Dynamic<'a> {
    let len_name = array_root_len_key(root);
    if let Some(existing) = env.get(&len_name) {
        return existing.clone();
    }
    let len = array_root_len_symbol(ctx, root, bitvec_i64);
    if let (Some(solver), Some(nonneg)) = (solver_opt, nonneg_constraint(ctx, &len)) {
        solver.assert(&nonneg);
    }
    env.insert(len_name, len.clone());
    len
}

fn fresh_array_len<'a>(vc: &VCtx<'a>, solver_opt: Option<&Solver<'a>>) -> Dynamic<'a> {
    let len: Dynamic = if vc.bitvec_i64 {
        BV::fresh_const(vc.ctx, &array_len_key("fresh"), I64_BITS).into()
    } else {
        Int::fresh_const(vc.ctx, &array_len_key("fresh")).into()
    };
    if let (Some(solver), Some(nonneg)) = (solver_opt, nonneg_constraint(vc.ctx, &len)) {
        solver.assert(&nonneg);
    }
    len
}

pub(crate) fn array_len_for_value<'a>(
    vc: &VCtx<'a>,
    env: &mut Env<'a>,
    arr: &Array<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> Dynamic<'a> {
    array_len_for_value_at_depth(vc, env, arr, solver_opt, 0)
}

fn array_len_for_value_at_depth<'a>(
    vc: &VCtx<'a>,
    env: &mut Env<'a>,
    arr: &Array<'a>,
    solver_opt: Option<&Solver<'a>>,
    depth: u32,
) -> Dynamic<'a> {
    if let Some(len) = call_result_len(vc, arr) {
        return len;
    }
    let root = array_root_value(arr);
    if root.kind() == z3::AstKind::App
        && root.decl().kind() == z3::DeclKind::UNINTERPRETED
        && root.num_children() == 0
    {
        return array_root_len_value(
            vc.ctx,
            env,
            &root.decl().name().to_string(),
            vc.bitvec_i64,
            solver_opt,
        );
    }
    if let Some(len) = env.get(&array_value_len_key(&root)) {
        return len.clone();
    }
    if depth < ARRAY_VALUE_LEN_ITE_DEPTH
        && root.kind() == z3::AstKind::App
        && root.decl().kind() == z3::DeclKind::ITE
    {
        let (Some(cond), Some(then_arr), Some(else_arr)) = (
            root.nth_child(0).and_then(|child| child.as_bool()),
            root.nth_child(1).and_then(|child| child.as_array()),
            root.nth_child(2).and_then(|child| child.as_array()),
        ) else {
            return fresh_array_len(vc, solver_opt);
        };
        let then_len = array_len_for_value_at_depth(vc, env, &then_arr, solver_opt, depth + 1);
        let else_len = array_len_for_value_at_depth(vc, env, &else_arr, solver_opt, depth + 1);
        let len = cond.ite(&then_len, &else_len);
        env.insert(array_value_len_key(&root), len.clone());
        return len;
    }
    fresh_array_len(vc, solver_opt)
}

const ARRAY_VALUE_LEN_ITE_DEPTH: u32 = 16;

pub(crate) fn array_len_for_expr<'a>(
    vc: &VCtx<'a>,
    env: &mut Env<'a>,
    value_expr: Option<&crate::parser::ast::Expr>,
    value: &Dynamic<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> Dynamic<'a> {
    match value_expr {
        Some(crate::parser::ast::Expr::Variable(name)) => {
            array_len_value(vc.ctx, env, name, vc.bitvec_i64, solver_opt)
        }
        Some(crate::parser::ast::Expr::ArrayLit(elements)) => {
            concrete_len_value(vc.ctx, elements.len(), vc.bitvec_i64)
        }
        Some(crate::parser::ast::Expr::IfThenElse {
            then_branch,
            else_branch,
            ..
        }) => {
            let Some(cond) = value.nth_child(0).and_then(|c| c.as_bool()) else {
                return fresh_array_len(vc, solver_opt);
            };
            let len_t = value
                .nth_child(1)
                .map(|v| branch_tail_len(vc, env, then_branch, &v, 0, solver_opt))
                .unwrap_or_else(|| fresh_array_len(vc, solver_opt));
            let len_e = value
                .nth_child(2)
                .map(|v| branch_tail_len(vc, env, else_branch, &v, 0, solver_opt))
                .unwrap_or_else(|| fresh_array_len(vc, solver_opt));
            cond.ite(&len_t, &len_e)
        }
        Some(crate::parser::ast::Expr::Block(stmt)) => {
            branch_tail_len(vc, env, stmt, value, 0, solver_opt)
        }
        Some(crate::parser::ast::Expr::Match { arms, .. }) => value
            .as_array()
            .and_then(|array| call_result_len(vc, &array))
            .or_else(|| match_arm_lens(vc, env, arms, value, 0, solver_opt))
            .unwrap_or_else(|| fresh_array_len(vc, solver_opt)),
        _ => value
            .as_array()
            .map(|arr| array_len_for_value(vc, env, &arr, solver_opt))
            .unwrap_or_else(|| fresh_array_len(vc, solver_opt)),
    }
}

pub(crate) fn register_array_value_len<'a>(
    env: &mut Env<'a>,
    arr: &Array<'a>,
    len: Dynamic<'a>,
) -> String {
    let root = array_root_value(arr);
    let key = array_value_len_key(&root);
    env.insert(key.clone(), len.clone());
    if root.kind() == z3::AstKind::App
        && root.decl().kind() == z3::DeclKind::UNINTERPRETED
        && root.num_children() == 0
    {
        let root_name = root.decl().name().to_string();
        env.entry(array_root_len_key(&root_name)).or_insert(len);
    }
    key
}

pub(crate) const ARRAY_VALUE_LEN_PREFIX: &str = "__array_len#";

fn array_value_len_key(root: &Dynamic<'_>) -> String {
    format!("{ARRAY_VALUE_LEN_PREFIX}{root}")
}

pub(crate) fn mark_string_constraints(vc: &VCtx<'_>) {
    if let Some(cell) = vc.has_string_constraints {
        cell.set(true);
    }
}

/// Z3 sort for a `lower()`ed type — recursive, so nested `[[T]]` element
/// types build genuine `Array(Int, …)` range sorts rather than the phantom
/// `Int` the flat `ArrayElementSort` tag would collapse them to.
fn z3_sort_for_lowered<'a>(
    ctx: &'a Context,
    ty: &crate::lowering::LoweredType,
    ieee754_f64: bool,
) -> z3::Sort<'a> {
    match ty {
        LoweredType::F64 if ieee754_f64 => z3::Sort::float(ctx, F64_EBITS, F64_SBITS),
        LoweredType::F64 => z3::Sort::real(ctx),
        LoweredType::Bool => z3::Sort::bool(ctx),
        LoweredType::Str => z3::Sort::string(ctx),
        LoweredType::Array(inner) => {
            let index = z3::Sort::int(ctx);
            let range = z3_sort_for_lowered(ctx, inner, ieee754_f64);
            z3::Sort::array(ctx, &index, &range)
        }
        _ => z3::Sort::int(ctx),
    }
}

/// Z3 element sort for a resolved element type *name* — the sort-valued
/// counterpart of `array_element_sort_from_type`, able to express the `Str`
/// and nested `[T]` elements the flat tag cannot.
pub(crate) fn z3_element_sort<'a>(vc: &VCtx<'a>, type_name: &str) -> z3::Sort<'a> {
    let type_name = type_name.strip_prefix("enum:").unwrap_or(type_name);
    if let Some((sort, _)) =
        crate::verification::support::datatype::datatype_sort_for_type(vc, type_name)
    {
        return sort.sort.clone();
    }
    z3_sort_for_lowered(
        vc.ctx,
        &lower(&vc.module_env.resolve_base_type(type_name)),
        vc.ieee754_f64,
    )
}

pub(crate) fn z3_array_for_sort<'a>(
    ctx: &'a Context,
    name: &str,
    sort: ArrayElementSort,
) -> Result<Array<'a>, MumeiError> {
    let int_sort = z3::Sort::int(ctx);
    Ok(match sort {
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
        ArrayElementSort::Str => {
            let string_sort = z3::Sort::string(ctx);
            Array::new_const(ctx, name, &int_sort, &string_sort)
        }
        ArrayElementSort::Struct => Array::new_const(ctx, name, &int_sort, &int_sort),
        ArrayElementSort::Enum | ArrayElementSort::UnsupportedEnum => {
            return Err(MumeiError::type_error(
                "enum array construction requires its resolved datatype sort",
            ));
        }
        ArrayElementSort::UnsupportedStruct => {
            return Err(MumeiError::type_error(
                "array element struct does not have supported scalar leaves",
            ));
        }
        // The flat tag cannot rebuild the precise inner sort of a `[[T]]` —
        // callers holding the element type (`param_z3_value`,
        // `z3_array_for_name`) go through `z3_element_sort` instead. This
        // generic `Int -> Int -> Int` shape only serves poison/havoc
        // placeholders, where any unconstrained nested array is sound.
        ArrayElementSort::Nested => {
            let inner = z3::Sort::array(ctx, &int_sort, &int_sort);
            Array::new_const(ctx, name, &int_sort, &inner)
        }
    })
}

pub(crate) fn z3_array_for_name<'a>(vc: &VCtx<'a>, name: &str) -> Array<'a> {
    let int_sort = z3::Sort::int(vc.ctx);
    let elem_sort = z3_element_sort(vc, &array_element_type_name(name, vc));
    Array::new_const(vc.ctx, name, &int_sort, &elem_sort)
}

pub(crate) fn z3_dynamic_array<'a>(vc: &VCtx<'a>, name: &str, env: &Env<'a>) -> Array<'a> {
    let arr_key = format!("__z3_arr_{}", name);
    env.get(&arr_key)
        .and_then(|d| d.as_array())
        // `env[name]` already holds the array const for `[T]` params and for
        // `result` in spec-validation — prefer it over a name-synthesized
        // const so the declared element sort (`Str`, `[T]`, …) is kept.
        .or_else(|| env.get(name).and_then(|d| d.as_array()))
        .unwrap_or_else(|| z3_array_for_name(vc, name))
}

pub(crate) struct StructArrayLeafSnapshot<'a> {
    element_type: String,
    arrays: HashMap<String, Array<'a>>,
}

impl<'a> StructArrayLeafSnapshot<'a> {
    pub(crate) fn element_type(&self) -> &str {
        &self.element_type
    }

    pub(crate) fn leaf_array(&self, path: &str) -> Option<Array<'a>> {
        self.arrays.get(path).cloned()
    }
}

pub(crate) fn snapshot_struct_array_leaves<'a>(
    vc: &VCtx<'a>,
    env: &mut Env<'a>,
    expression: &Expr,
    value: &Dynamic<'a>,
    declared_array_type: Option<&str>,
) -> MumeiResult<Option<StructArrayLeafSnapshot<'a>>> {
    let uses_unbound_result = is_unbound_array_result_expression(expression);
    let result_element_type = if uses_unbound_result {
        value
            .as_array()
            .and_then(|array| call_result_struct_array_element_type(vc, &array))
    } else {
        None
    };
    let element_type = declared_array_type
        .map(str::trim)
        .filter(|type_name| type_name.starts_with('[') && type_name.ends_with(']'))
        .map(|type_name| array_element_type_from_annotation(Some(type_name), vc.module_env))
        .or(result_element_type)
        .or_else(|| array_element_type_from_expr(vc, expression));
    let Some(element_type) = element_type else {
        return Ok(None);
    };
    if array_element_sort_from_type(&element_type, vc.module_env, vc.ieee754_f64)
        != ArrayElementSort::Struct
    {
        return Ok(None);
    }
    validate_array_element_type(vc, &element_type)?;
    if value.as_array().is_none() {
        return Ok(None);
    }
    let leaves = struct_array_leaf_types(&element_type, vc.module_env).ok_or_else(|| {
        MumeiError::type_error(format!(
            "array element struct '{element_type}' must have only scalar leaves"
        ))
    })?;
    static SNAPSHOT_UID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let snapshot_id = SNAPSHOT_UID.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let mut arrays = HashMap::with_capacity(leaves.len());
    for (path, leaf_type) in leaves {
        let range = z3_element_sort(vc, &leaf_type);
        let result_array = if uses_unbound_result {
            value
                .as_array()
                .and_then(|array| call_result_struct_array(vc, &array, &path))
        } else {
            None
        };
        let array = if let Some(array) = result_array {
            array
        } else if let Some(array) = struct_array_leaf_for_expr(vc, env, expression, value, &path, 0)
        {
            array
        } else {
            match expression {
                Expr::Variable(source) => {
                    struct_array_leaf_array(vc, source, &path, &leaf_type, env)
                }
                Expr::ArrayLit(_) => {
                    return Err(MumeiError::type_error(format!(
                        "struct array literal is missing leaf '{path}'"
                    )))
                }
                _ => z3_fresh_array_for_range(
                    vc.ctx,
                    &format!("__struct_arr_arg_snapshot#{snapshot_id}#{path}"),
                    &range,
                ),
            }
        };
        if array.get_sort().array_range() != Some(range) {
            return Err(MumeiError::type_error(format!(
                "struct array argument leaf '{path}' has an incompatible sort"
            )));
        }
        arrays.insert(path, array);
    }
    Ok(Some(StructArrayLeafSnapshot {
        element_type,
        arrays,
    }))
}

/// Wire a name-bound array value into the tracking slots reads and stores
/// consult: `__z3_arr_<name>` carries the Z3 array, `len_<name>` the length
/// (concrete for a `[e0, …]` literal, the source's `len` for a `var` alias, a
/// fresh tracked symbol otherwise), and `local_array_elem_types` records the
/// element type so `array_element_type_name` resolves it like a declared
/// `[T]` parameter.
pub(crate) fn wire_array_slots<'a>(
    vc: &VCtx<'a>,
    name: &str,
    value: Option<&Expr>,
    val: &Dynamic<'a>,
    env: &mut Env<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> MumeiResult<()> {
    wire_array_slots_with_struct_leaf_snapshot(vc, name, value, val, env, solver_opt, None)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn wire_array_slots_with_struct_leaf_snapshot<'a>(
    vc: &VCtx<'a>,
    name: &str,
    value: Option<&Expr>,
    val: &Dynamic<'a>,
    env: &mut Env<'a>,
    solver_opt: Option<&Solver<'a>>,
    leaf_snapshot: Option<&StructArrayLeafSnapshot<'a>>,
) -> MumeiResult<()> {
    // For `var` sources the live array is the tracked `__z3_arr_<src>` chain —
    // `env[src]` is only the base const, and `src[i] = v` stores never rewrite
    // it, so using `val` here would drop prior writes.
    let arr = match value {
        Some(Expr::Variable(src)) => env
            .get(&format!("__z3_arr_{src}"))
            .and_then(|d| d.as_array())
            .or_else(|| val.as_array()),
        _ => val.as_array(),
    };
    let Some(arr) = arr else {
        // Rebound to a non-array value — poison the tracked slots instead of
        // removing them: `z3_dynamic_array` falls back to
        // `Array::new_const(ctx, name)` when the slot is absent, and Z3
        // interns symbols by name, so a `[T]` parameter's rebound `arr[i]`
        // would revive the original param const along with its requires-side
        // assertions (`arr[0] == 4` staying provable after `arr = 5`).
        // A `#`-suffixed name can never collide with a source identifier.
        let len_key = array_len_key(name);
        if env.contains_key(&format!("__z3_arr_{name}")) || env.contains_key(&len_key) {
            let sort = array_element_sort(name, vc);
            static REBOUND_ARRAY_UID: std::sync::atomic::AtomicUsize =
                std::sync::atomic::AtomicUsize::new(0);
            let rebound_name = format!(
                "{name}#rebound#{}",
                REBOUND_ARRAY_UID.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            );
            let old_array = env
                .get(&format!("__z3_arr_{name}"))
                .and_then(|value| value.as_array());
            let rebound = if let Some(old_array) = old_array {
                let old_array: Dynamic = old_array.into();
                z3_fresh_const_like(vc.ctx, &rebound_name, &old_array)
                    .as_array()
                    .expect("fresh value preserves array sort")
            } else if sort == ArrayElementSort::Enum {
                let range = z3_element_sort(vc, &array_element_type_name(name, vc));
                z3_array_for_range(vc.ctx, &rebound_name, &range)
            } else {
                z3_array_for_sort(vc.ctx, &rebound_name, sort)?
            };
            let len = array_len_for_value(vc, env, &rebound, solver_opt);
            env.insert(format!("__z3_arr_{name}"), rebound.into());
            env.insert(len_key, len);
        }
        vc.local_array_elem_types.borrow_mut().remove(name);
        clear_struct_array_leaf_slots(env, name);
        return Ok(());
    };
    let elem_ty = z3_range_type_name(&arr.get_sort());
    let arr_dyn: Dynamic = arr.clone().into();
    env.insert(format!("__z3_arr_{name}"), arr_dyn.clone());
    let len = array_len_for_expr(vc, env, value, &arr_dyn, solver_opt);
    register_array_value_len(env, &arr, len.clone());
    env.insert(array_len_key(name), len);
    let elem_ty = leaf_snapshot
        .map(|snapshot| snapshot.element_type.clone())
        .or_else(|| call_result_struct_array_element_type(vc, &arr))
        .or_else(|| value.and_then(|value| array_element_type_from_expr(vc, value)))
        .unwrap_or(elem_ty);
    validate_array_element_type(vc, &elem_ty)?;
    vc.local_array_elem_types
        .borrow_mut()
        .insert(name.to_string(), elem_ty.to_string());
    if let Some(leaves) = struct_array_leaf_types(&elem_ty, vc.module_env) {
        for (path, leaf_type) in leaves {
            let key = struct_array_leaf_key(name, &path);
            let leaf_array = if let Some(snapshot) = leaf_snapshot {
                snapshot.arrays.get(&path).cloned().ok_or_else(|| {
                    MumeiError::type_error(format!(
                        "struct array argument is missing leaf snapshot '{path}'"
                    ))
                })?
            } else {
                match value {
                    Some(Expr::Variable(source)) => {
                        struct_array_leaf_array(vc, source, &path, &leaf_type, env)
                    }
                    Some(Expr::ArrayLit(_)) => {
                        let source = array_root_name(&arr).ok_or_else(|| {
                            MumeiError::type_error("struct array literal has no carrier root")
                        })?;
                        env.get(&struct_array_leaf_key(&source, &path))
                            .and_then(|value| value.as_array())
                            .ok_or_else(|| {
                                MumeiError::type_error(format!(
                                    "struct array literal is missing leaf '{path}'"
                                ))
                            })?
                    }
                    Some(expression) => {
                        struct_array_leaf_for_expr(vc, env, expression, &arr_dyn, &path, 0)
                            .unwrap_or_else(|| {
                                z3_fresh_array_for_range(
                                    vc.ctx,
                                    &key,
                                    &z3_element_sort(vc, &leaf_type),
                                )
                            })
                    }
                    None => {
                        z3_fresh_array_for_range(vc.ctx, &key, &z3_element_sort(vc, &leaf_type))
                    }
                }
            };
            env.insert(key, leaf_array.into());
        }
    } else {
        clear_struct_array_leaf_slots(env, name);
    }
    Ok(())
}

fn is_unbound_array_result_expression(expression: &Expr) -> bool {
    match expression {
        Expr::Call(_, _) | Expr::Match { .. } => true,
        Expr::Block(stmt) => stmt_tail_expr(stmt).is_some_and(is_unbound_array_result_expression),
        _ => false,
    }
}

pub(crate) fn array_element_type_from_expr(vc: &VCtx<'_>, value: &Expr) -> Option<String> {
    match value {
        Expr::Variable(source) => Some(array_element_type_name(source, vc)),
        Expr::Block(stmt) => {
            stmt_tail_expr(stmt).and_then(|tail| array_element_type_from_expr(vc, tail))
        }
        Expr::ArrayLit(elements) => elements.first().and_then(|element| {
            crate::verification::support::datatype::declared_type_of_expr(vc, element)
                .or_else(|| match element {
                    Expr::StructInit { type_name, .. } => Some(type_name.clone()),
                    _ => None,
                })
                .map(|type_name| vc.module_env.resolve_base_type(&type_name))
        }),
        Expr::Call(name, _) => vc
            .module_env
            .get_atom(name)
            .and_then(|atom| atom.return_type.as_deref())
            .map(|type_name| array_element_type_from_annotation(Some(type_name), vc.module_env)),
        _ => crate::verification::support::datatype::declared_type_of_expr(vc, value)
            .map(|type_name| array_element_type_from_annotation(Some(&type_name), vc.module_env)),
    }
}

fn clear_struct_array_leaf_slots(env: &mut Env<'_>, name: &str) {
    let prefix = format!("__struct_arr#{name}#");
    env.retain(|key, _| !key.starts_with(&prefix));
}

pub(crate) fn havoc_struct_array_leaf_arrays<'a>(
    vc: &VCtx<'a>,
    bindings: &[String],
    env: &mut Env<'a>,
) {
    static STRUCT_ARRAY_HAVOC_UID: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);
    let mut roots = std::collections::HashSet::new();
    for binding in bindings {
        let element_type = array_element_type_name(binding, vc);
        let Some(leaves) = struct_array_leaf_types(&element_type, vc.module_env) else {
            continue;
        };
        for (path, leaf_type) in leaves {
            let old = struct_array_leaf_array(vc, binding, &path, &leaf_type, env);
            let root = array_root_ast(&old);
            if !roots.insert(root as usize) {
                continue;
            }
            let uid = STRUCT_ARRAY_HAVOC_UID.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let old: Dynamic = old.into();
            let fresh = z3_fresh_const_like(vc.ctx, &format!("__havoc_struct_arr_{uid}"), &old);
            let aliases: Vec<String> = env
                .iter()
                .filter(|(_, value)| {
                    value
                        .as_array()
                        .is_some_and(|array| array_root_ast(&array) == root)
                })
                .map(|(key, _)| key.clone())
                .collect();
            for key in aliases {
                env.insert(key, fresh.clone());
            }
        }
    }
}

/// A concrete array length, sorted like `array_len_symbol` under
/// `--bitvec-i64` (BV(64)) so it can merge with tracked `len_` values.
pub(crate) fn concrete_len_value<'a>(ctx: &'a Context, n: usize, bitvec_i64: bool) -> Dynamic<'a> {
    if bitvec_i64 {
        z3::ast::BV::from_i64(ctx, n as i64, I64_BITS).into()
    } else {
        Int::from_i64(ctx, n as i64).into()
    }
}

/// Tail expression of a `Stmt`: bare expressions and the last statement of
/// a block produce the branch value.
fn stmt_tail_expr(stmt: &Stmt) -> Option<&Expr> {
    match stmt {
        Stmt::Expr(e, _) => Some(e),
        Stmt::Block(stmts, _) => stmts.last().and_then(stmt_tail_expr),
        _ => None,
    }
}

/// `let` scope for tail-length resolution. `stmts` are the statements a
/// variable may be resolved in (everything before the `let` currently
/// being followed); `root` is the whole block, used to tell whether a
/// variable that falls back to its env slot was reassigned after the
/// point it was read at (the env only reflects end-of-block state).
#[derive(Clone, Copy)]
pub(crate) struct LetScope<'e> {
    stmts: &'e [Stmt],
    root: &'e [Stmt],
}

impl<'e> LetScope<'e> {
    fn of(stmt: &'e Stmt) -> Self {
        let stmts = match stmt {
            Stmt::Block(stmts, _) => stmts.as_slice(),
            other => std::slice::from_ref(other),
        };
        LetScope { stmts, root: stmts }
    }

    fn before(self, stmts: &'e [Stmt]) -> Self {
        LetScope {
            stmts,
            root: self.root,
        }
    }

    /// Scope for a block nested somewhere inside this one.
    fn nested(self, stmt: &'e Stmt) -> Self {
        LetScope {
            stmts: LetScope::of(stmt).stmts,
            root: self.root,
        }
    }

    /// Whether `var`'s env slot is trustworthy for a read inside this
    /// scope: reads at the outermost block's tail see the final state,
    /// earlier reads only if nothing in that block reassigns `var`.
    fn env_is_current(self, var: &str) -> bool {
        std::ptr::eq(self.stmts, self.root) || !self.root.iter().any(|s| stmt_assigns_var(s, var))
    }
}

/// The most recent `let <name> = <rhs>` in `scope` — match-arm `let`s stay
/// arm-local and never reach the merged env, so resolving the arm tail's
/// variable here recovers e.g. `A => { let t = [1,2]; t }`. Returns the
/// rhs together with the statements preceding that `let`, which is the
/// only scope its own variables may be resolved in. Any reassignment of
/// `name` after the `let` invalidates the initializer as a length source,
/// so the caller falls back to the env slot (or a fresh symbol).
fn let_rhs_in<'e>(scope: &'e [Stmt], name: &str) -> Option<(&'e Expr, &'e [Stmt])> {
    for (i, s) in scope.iter().enumerate().rev() {
        if let Stmt::Let { var, value, .. } = s {
            if var == name {
                return Some((value.as_ref(), &scope[..i]));
            }
        }
        if stmt_assigns_var(s, name) {
            return None;
        }
    }
    None
}

/// Recursion bound for tail-length computation — deep let-chains or
/// `let x = x` cycles bail out to a fresh symbol rather than diverging.
const TAIL_LEN_DEPTH: u32 = 16;

/// Walk the `ite` spine of a folded `match` value, returning the arm
/// conditions and per-arm value nodes, or `None` when the shape doesn't
/// match `arms` (e.g. the fold was simplified). Only child-2 is
/// descended, so `ite`s inside arm values can't confuse the walk.
fn match_ite_spine<'a>(
    arms: &[MatchArm],
    value: &Dynamic<'a>,
) -> Option<(Vec<Bool<'a>>, Vec<Dynamic<'a>>)> {
    if arms.is_empty() {
        return None;
    }
    let mut conds: Vec<Bool> = Vec::with_capacity(arms.len() - 1);
    let mut vals: Vec<Dynamic> = Vec::with_capacity(arms.len());
    let mut node = value.clone();
    for _ in 0..arms.len() - 1 {
        conds.push(node.nth_child(0)?.as_bool()?);
        vals.push(node.nth_child(1)?);
        node = node.nth_child(2)?;
    }
    vals.push(node);
    Some((conds, vals))
}

/// Lengths merged across a `match`'s arms: `ite(c_1, len_1, …, len_n)`
/// mirroring the value spine, with each arm's length computed
/// recursively from its body and value node.
fn match_arm_lens<'a>(
    vc: &VCtx<'a>,
    env: &mut Env<'a>,
    arms: &[MatchArm],
    value: &Dynamic<'a>,
    depth: u32,
    solver_opt: Option<&Solver<'a>>,
) -> Option<Dynamic<'a>> {
    if depth >= TAIL_LEN_DEPTH {
        return None;
    }
    let (conds, vals) = match_ite_spine(arms, value)?;
    let mut lens: Vec<Dynamic> = arms
        .iter()
        .zip(vals.iter())
        .map(|(arm, val)| branch_tail_len(vc, env, &arm.body, val, depth + 1, solver_opt))
        .collect();
    let mut acc = lens.pop()?;
    for (cond, len_arm) in conds.iter().zip(lens.iter()).rev() {
        acc = cond.ite(len_arm, &acc);
    }
    Some(acc)
}

/// Length contributed by a branch/arm body: resolves the tail expr and
/// delegates to `tail_len_expr`, which tracks `val_node`'s structure.
fn branch_tail_len<'a>(
    vc: &VCtx<'a>,
    env: &mut Env<'a>,
    scope: &Stmt,
    val_node: &Dynamic<'a>,
    depth: u32,
    solver_opt: Option<&Solver<'a>>,
) -> Dynamic<'a> {
    match stmt_tail_expr(scope) {
        Some(tail) => tail_len_expr(
            vc,
            env,
            Some(LetScope::of(scope)),
            tail,
            val_node,
            depth,
            solver_opt,
        ),
        None => array_value_len_or_fresh(vc, env, val_node, solver_opt),
    }
}

fn branch_tail_struct_array_leaf<'a>(
    vc: &VCtx<'a>,
    env: &mut Env<'a>,
    scope: &Stmt,
    val_node: &Dynamic<'a>,
    leaf_path: &str,
    depth: u32,
) -> Option<z3::ast::Array<'a>> {
    let tail = stmt_tail_expr(scope)?;
    struct_array_leaf_for_expr(vc, env, tail, val_node, leaf_path, depth)
}

fn struct_array_leaf_for_expr<'a>(
    vc: &VCtx<'a>,
    env: &mut Env<'a>,
    expression: &Expr,
    val_node: &Dynamic<'a>,
    leaf_path: &str,
    depth: u32,
) -> Option<z3::ast::Array<'a>> {
    if depth >= TAIL_LEN_DEPTH {
        return None;
    }
    if matches!(expression, Expr::Match { .. }) {
        if let Some(array) = val_node
            .as_array()
            .and_then(|array| call_result_struct_array(vc, &array, leaf_path))
        {
            return Some(array);
        }
    }
    match expression {
        Expr::Variable(source) => env
            .get(&struct_array_leaf_key(source, leaf_path))
            .and_then(|value| value.as_array()),
        Expr::FieldAccess(_, _) => {
            let arr = val_node.as_array()?;
            let source = array_root_name(&arr)?;
            env.get(&struct_array_leaf_key(&source, leaf_path))
                .and_then(|value| value.as_array())
        }
        Expr::ArrayLit(_) => {
            let arr = val_node.as_array()?;
            let source = array_root_name(&arr)?;
            env.get(&struct_array_leaf_key(&source, leaf_path))
                .and_then(|value| value.as_array())
        }
        Expr::Call(_, _) => val_node
            .as_array()
            .and_then(|arr| call_result_struct_array(vc, &arr, leaf_path)),
        Expr::IfThenElse {
            then_branch,
            else_branch,
            ..
        } => {
            let cond = val_node.nth_child(0)?.as_bool()?;
            let then_value = val_node.nth_child(1)?;
            let else_value = val_node.nth_child(2)?;
            let then_leaf = branch_tail_struct_array_leaf(
                vc,
                env,
                then_branch,
                &then_value,
                leaf_path,
                depth + 1,
            )?;
            let else_leaf = branch_tail_struct_array_leaf(
                vc,
                env,
                else_branch,
                &else_value,
                leaf_path,
                depth + 1,
            )?;
            Some(cond.ite(&then_leaf, &else_leaf))
        }
        Expr::Block(stmt) => {
            let tail = stmt_tail_expr(stmt)?;
            struct_array_leaf_for_expr(vc, env, tail, val_node, leaf_path, depth + 1)
        }
        Expr::Match { arms, .. } => {
            let (conds, vals) = match_ite_spine(arms, val_node)?;
            let mut leaves: Vec<z3::ast::Array> = arms
                .iter()
                .zip(vals.iter())
                .map(|(arm, value)| {
                    branch_tail_struct_array_leaf(vc, env, &arm.body, value, leaf_path, depth + 1)
                })
                .collect::<Option<_>>()?;
            let mut acc = leaves.pop()?;
            for (cond, leaf) in conds.iter().zip(leaves.iter()).rev() {
                acc = cond.ite(leaf, &acc);
            }
            Some(acc)
        }
        _ => None,
    }
}

fn array_value_len_or_fresh<'a>(
    vc: &VCtx<'a>,
    env: &mut Env<'a>,
    val_node: &Dynamic<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> Dynamic<'a> {
    val_node
        .as_array()
        .map(|arr| array_len_for_value(vc, env, &arr, solver_opt))
        .unwrap_or_else(|| fresh_array_len(vc, solver_opt))
}

/// Array length of a tail expression. `scope` is the enclosing block —
/// used to resolve `let`-bound tail variables whose slots never reached
/// the env (match-arm locals). `val_node` is the Z3 value the tail
/// evaluated to, letting nested `if`/`match` tails mirror their own
/// `ite` conditions on lengths. Anything unresolvable gets a fresh
/// symbol (fail-closed: it only makes proofs harder, never
/// easier).
#[allow(clippy::too_many_arguments)]
pub(crate) fn tail_len_expr<'a>(
    vc: &VCtx<'a>,
    env: &mut Env<'a>,
    scope: Option<LetScope<'_>>,
    tail: &Expr,
    val_node: &Dynamic<'a>,
    depth: u32,
    solver_opt: Option<&Solver<'a>>,
) -> Dynamic<'a> {
    if depth >= TAIL_LEN_DEPTH {
        return array_value_len_or_fresh(vc, env, val_node, solver_opt);
    }
    match tail {
        Expr::ArrayLit(elements) => concrete_len_value(vc.ctx, elements.len(), vc.bitvec_i64),
        Expr::Variable(src) => match scope {
            Some(sc) => match let_rhs_in(sc.stmts, src) {
                Some((rhs, before)) => tail_len_expr(
                    vc,
                    env,
                    Some(sc.before(before)),
                    rhs,
                    val_node,
                    depth + 1,
                    solver_opt,
                ),
                None if sc.env_is_current(src) => val_node
                    .as_array()
                    .and_then(|arr| {
                        let root = array_root_value(&arr);
                        env.get(&array_value_len_key(&root)).cloned()
                    })
                    .unwrap_or_else(|| {
                        array_len_value(vc.ctx, env, src, vc.bitvec_i64, solver_opt)
                    }),
                None => array_value_len_or_fresh(vc, env, val_node, solver_opt),
            },
            None => array_len_value(vc.ctx, env, src, vc.bitvec_i64, solver_opt),
        },
        Expr::IfThenElse {
            then_branch,
            else_branch,
            ..
        } => {
            let (Some(cond), Some(t_val), Some(e_val)) = (
                val_node.nth_child(0).and_then(|c| c.as_bool()),
                val_node.nth_child(1),
                val_node.nth_child(2),
            ) else {
                return array_value_len_or_fresh(vc, env, val_node, solver_opt);
            };
            let len_t = branch_tail_len(vc, env, then_branch, &t_val, depth + 1, solver_opt);
            let len_e = branch_tail_len(vc, env, else_branch, &e_val, depth + 1, solver_opt);
            cond.ite(&len_t, &len_e)
        }
        Expr::Block(stmt) => match stmt_tail_expr(stmt) {
            Some(tail) => tail_len_expr(
                vc,
                env,
                Some(scope.map_or_else(|| LetScope::of(stmt), |sc| sc.nested(stmt))),
                tail,
                val_node,
                depth + 1,
                solver_opt,
            ),
            None => array_value_len_or_fresh(vc, env, val_node, solver_opt),
        },
        Expr::Match { arms, .. } => match_arm_lens(vc, env, arms, val_node, depth + 1, solver_opt)
            .unwrap_or_else(|| array_value_len_or_fresh(vc, env, val_node, solver_opt)),
        // `len(f(..))` on a call result — or any other array value — picks
        // up the len symbol the Call arm registered for it.
        _ => array_value_len_or_fresh(vc, env, val_node, solver_opt),
    }
}

/// Walk a tracked array value to the innermost (root) array AST node — the
/// base const that `store(store(base, …), …)` chains share. Aliases created
/// by `let b = a` hold the same root even after either side stores.
pub(crate) fn array_root_ast(arr: &z3::ast::Array) -> z3_sys::Z3_ast {
    array_root_value(arr).get_z3_ast()
}

pub(crate) fn array_root_name(arr: &z3::ast::Array) -> Option<String> {
    let root = array_root_value(arr);
    (root.kind() == z3::AstKind::App && root.decl().kind() == z3::DeclKind::UNINTERPRETED)
        .then(|| root.decl().name().to_string())
}

fn array_root_value<'a>(arr: &z3::ast::Array<'a>) -> Dynamic<'a> {
    let mut cur: Dynamic = arr.clone().into();
    loop {
        if cur.kind() == z3::AstKind::App && cur.decl().kind() == z3::DeclKind::STORE {
            if let Some(child) = cur.nth_child(0) {
                if child.get_sort().kind() == z3::SortKind::Array {
                    cur = child;
                    continue;
                }
            }
        }
        return cur;
    }
}

/// The len symbol a `Call` arm minted for an array-typed result — keyed by
/// the result array's root ast, so `store` chains layered by caller writes
/// still find it. Returns `None` for arrays that didn't come from a call.
pub(crate) fn call_result_len<'a>(vc: &VCtx<'a>, arr: &z3::ast::Array<'a>) -> Option<Dynamic<'a>> {
    vc.call_result_lens
        .borrow()
        .get(&(array_root_ast(arr) as usize))
        .and_then(|result| result.len.clone())
}

pub(crate) fn call_result_struct_arrays<'a>(
    vc: &VCtx<'a>,
    arr: &z3::ast::Array<'a>,
    return_type: Option<&str>,
    result_name: &str,
) -> Result<std::collections::HashMap<String, z3::ast::Array<'a>>, MumeiError> {
    let element_type = array_element_type_from_annotation(return_type, vc.module_env);
    validate_array_element_type(vc, &element_type)?;
    if array_element_sort_from_type(&element_type, vc.module_env, vc.ieee754_f64)
        != ArrayElementSort::Struct
    {
        return Ok(std::collections::HashMap::new());
    }

    let root = array_root_ast(arr) as usize;
    if let Some(existing) = vc.call_result_lens.borrow().get(&root) {
        return Ok(existing.struct_leaves.clone());
    }

    let leaves = struct_array_leaf_types(&element_type, vc.module_env).ok_or_else(|| {
        MumeiError::type_error(format!(
            "array element struct '{element_type}' must have only scalar leaves"
        ))
    })?;
    Ok(leaves
        .into_iter()
        .map(|(path, leaf_type)| {
            let key = struct_array_leaf_key(result_name, &path);
            let array = z3_fresh_array_for_range(vc.ctx, &key, &z3_element_sort(vc, &leaf_type));
            (path, array)
        })
        .collect())
}

pub(crate) fn call_result_struct_array<'a>(
    vc: &VCtx<'a>,
    arr: &z3::ast::Array<'a>,
    leaf_path: &str,
) -> Option<z3::ast::Array<'a>> {
    vc.call_result_lens
        .borrow()
        .get(&(array_root_ast(arr) as usize))
        .and_then(|result| result.struct_leaves.get(leaf_path).cloned())
}

pub(crate) fn call_result_struct_array_element_type<'a>(
    vc: &VCtx<'a>,
    arr: &z3::ast::Array<'a>,
) -> Option<String> {
    vc.call_result_lens
        .borrow()
        .get(&(array_root_ast(arr) as usize))
        .and_then(|result| result.struct_element_type.clone())
}

pub(crate) fn register_array_result<'a>(
    vc: &VCtx<'a>,
    arr: &z3::ast::Array<'a>,
    len: Dynamic<'a>,
    struct_element_type: Option<String>,
    struct_leaves: HashMap<String, z3::ast::Array<'a>>,
) {
    let value: Dynamic<'a> = arr.clone().into();
    vc.call_result_lens.borrow_mut().insert(
        array_root_ast(arr) as usize,
        crate::verification::translator::context::CallResultArray {
            len: Some(len),
            _keep: value,
            struct_leaves,
            struct_element_type,
        },
    );
}

/// Replace `name`'s tracked array chain — and every `__z3_arr_*`/`env[x]`
/// slot rooting at the same backing chain (i.e. `let b = a` aliases) — with
/// a fresh unconstrained array of the same element sort. `len_<name>` is
/// kept: a callee receives the `{len, ptr}` fat pointer by value, so it can
/// write elements but cannot resize the caller's buffer.
pub(crate) fn havoc_array_name<'a>(vc: &VCtx<'a>, name: &str, env: &mut Env<'a>) {
    static POSTCALL_UID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    // `[T]` params bind `env[name]` to the array const eagerly and may not
    // yet have a `__z3_arr_` slot; bound locals carry it.
    let arr_key = format!("__z3_arr_{name}");
    let arg_arr = env
        .get(&arr_key)
        .and_then(|d| d.as_array())
        .or_else(|| env.get(name).and_then(|d| d.as_array()));
    let Some(arg_arr) = arg_arr else { return };
    let root = array_root_ast(&arg_arr);
    let uid = POSTCALL_UID.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let old_arr: Dynamic = arg_arr.clone().into();
    let fresh = z3_fresh_const_like(vc.ctx, &format!("__postcall_arr_{uid}"), &old_arr);
    // Every tracked slot rooting at the same backing chain aliases the
    // shared buffer — havoc them together.
    let keys: Vec<String> = env
        .iter()
        .filter(|(k, v)| {
            k.starts_with("__z3_arr_") && v.as_array().is_some_and(|a| array_root_ast(&a) == root)
        })
        .map(|(k, _)| k.clone())
        .collect();
    for key in &keys {
        env.insert(key.clone(), fresh.clone());
    }
    // `env[x]` itself holds the array const for `[T]` params, literals, and
    // `let` aliases — rebind every var bound to an array sharing this root
    // so aliased reads see the same havoc'd elements.
    let var_keys: Vec<String> = env
        .iter()
        .filter(|(k, v)| {
            !k.starts_with("__z3_arr_") && v.as_array().is_some_and(|a| array_root_ast(&a) == root)
        })
        .map(|(k, _)| k.clone())
        .collect();
    let mut struct_bindings = vec![name.to_string()];
    for var in &var_keys {
        struct_bindings.push(var.clone());
    }
    for var in var_keys {
        env.insert(var, fresh.clone());
    }
    // Ensure the tracked slot exists even when the arg only had a bare
    // `env[name]` const (param arrays before their first access).
    env.insert(arr_key, fresh);
    havoc_struct_array_leaf_arrays(vc, &struct_bindings, env);
}

/// `ensures` reads post-write composite fields: at a call site the
/// callee's written struct params must bind fresh slots, or the caller
/// evaluates `ensures q.f == 1` against the *input* field it just
/// constrained to `== 0` through `requires` — the assumption collapses
/// and every caller postcondition proves vacuously. Only written fields
/// are freshened; untouched fields keep the argument's projection so
/// `ensures result == q.g` still relates to the actual argument. A
/// whole-aggregate `q = …` rebind freshens every declared field.
pub(crate) fn havoc_written_callee_fields<'a>(
    vc: &VCtx<'a>,
    callee: &crate::parser::Atom,
    env: &mut Env<'a>,
) {
    static POSTFIELD_UID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let ctx = vc.ctx;
    let Ok(body) = crate::parser::parse_body_expr_checked(&callee.body_expr) else {
        return;
    };
    for param in &callee.params {
        let Some(sdef) = param
            .type_name
            .as_deref()
            .and_then(|name| vc.module_env.get_struct(name))
        else {
            continue;
        };
        let mut written = std::collections::HashSet::new();
        body.written_fields(&param.name, &mut written);
        if body.rebinds_name(&param.name) {
            written.extend(sdef.fields.iter().map(|f| f.name.clone()));
        }
        for field in &sdef.fields {
            // A nested write `q.inner.a = …` replaces the whole `inner`
            // subtree — count the direct field as written, then freshen
            // every leaf key under `<param>_<field>_…` too, not just the
            // `<param>_<field>` handle. Leaving `q_inner_a` bound to the
            // input projection lets the callee's `ensures q.inner.a == k`
            // constrain the caller's *input* slot — a contradiction that
            // proves the caller's postconditions vacuously.
            let nested_written = written
                .iter()
                .any(|w| w.starts_with(&format!("{}.", field.name)));
            if !written.contains(&field.name) && !nested_written {
                continue;
            }
            let prefixes = [
                format!("{}_{}", param.name, field.name),
                struct_field_key(&param.name, &field.name),
            ];
            let mut keys: Vec<String> = Vec::new();
            for prefix in &prefixes {
                if env.contains_key(prefix) {
                    keys.push(prefix.clone());
                }
                let leaf_prefix = format!("{prefix}_");
                keys.extend(
                    env.keys()
                        .filter(|key| key.starts_with(&leaf_prefix))
                        .cloned(),
                );
            }
            for key in keys {
                let Some(old) = env.get(&key) else {
                    continue;
                };
                let uid = POSTFIELD_UID.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let fresh: Dynamic = match old.get_sort().kind() {
                    z3::SortKind::Int => {
                        Int::fresh_const(ctx, &format!("__postfield_{key}_{uid}")).into()
                    }
                    z3::SortKind::Bool => {
                        Bool::fresh_const(ctx, &format!("__postfield_{key}_{uid}")).into()
                    }
                    z3::SortKind::BV => {
                        let width = old.as_bv().map(|b| b.get_size()).unwrap_or(64);
                        BV::fresh_const(ctx, &format!("__postfield_{key}_{uid}"), width).into()
                    }
                    _ => continue,
                };
                env.insert(key, fresh);
            }
        }
    }
}

/// Element type name carried by a Z3 array's range sort — the inverse of
/// `z3_sort_for_lowered`. Recursive so a nested `Array(Int, Int)` range comes
/// back as `"[i64]"` rather than collapsing to `"i64"`; `Seq` comes back as
/// `"Str"`.
pub(crate) fn z3_range_type_name(sort: &z3::Sort<'_>) -> String {
    match sort.array_range().map(|range| range.kind()) {
        Some(z3::SortKind::Real) | Some(z3::SortKind::FloatingPoint) => "f64".to_string(),
        Some(z3::SortKind::Bool) => "bool".to_string(),
        Some(z3::SortKind::Seq) => "Str".to_string(),
        Some(z3::SortKind::Datatype) => sort.array_range().unwrap().to_string(),
        Some(z3::SortKind::Array) => format!(
            "[{}]",
            sort.array_range()
                .map(|range| z3_range_type_name(&range))
                .unwrap_or_else(|| "i64".to_string())
        ),
        _ => "i64".to_string(),
    }
}

pub(crate) fn z3_array_for_range<'a>(
    ctx: &'a Context,
    name: &str,
    range: &z3::Sort<'a>,
) -> Array<'a> {
    Array::new_const(ctx, name, &z3::Sort::int(ctx), range)
}

pub(crate) fn z3_fresh_array_for_range<'a>(
    ctx: &'a Context,
    prefix: &str,
    range: &z3::Sort<'a>,
) -> Array<'a> {
    Array::fresh_const(ctx, prefix, &z3::Sort::int(ctx), range)
}

pub(crate) fn z3_fresh_const_like<'a>(
    ctx: &'a Context,
    name: &str,
    existing: &Dynamic<'a>,
) -> Dynamic<'a> {
    let raw_ctx = raw_z3_context(ctx);
    let c_name =
        std::ffi::CString::new(name).unwrap_or_else(|_| std::ffi::CString::new("__havoc").unwrap());
    let ast = unsafe {
        z3_sys::Z3_mk_fresh_const(
            raw_ctx,
            c_name.as_ptr(),
            z3_sys::Z3_get_sort(raw_ctx, existing.get_z3_ast()),
        )
    };
    unsafe { Dynamic::wrap(ctx, ast) }
}

/// Does `callee`'s body possibly store into the array bound to `param_name`?
/// Direct `name[i] = v` in any nested block/loop/task/if/match arm, or
/// passing `name` to a call whose callee may store to that parameter
/// (transitive, depth-capped; unparseable bodies and unknown callees are
/// assumed mutable — fail closed).
pub(crate) fn atom_stores_to_array(
    module_env: &ModuleEnv,
    callee: &crate::parser::Atom,
    param_name: &str,
) -> bool {
    let mut visited = std::collections::HashSet::new();
    atom_stores_to_array_depth(module_env, callee, param_name, &mut visited, 0)
}

fn bound_names_in_stmt(stmt: &Stmt) -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    collect_bound_names_stmt(stmt, &mut names);
    names
}

fn collect_bound_names_stmt(stmt: &Stmt, names: &mut std::collections::HashSet<String>) {
    match stmt {
        Stmt::Let { var, value, .. } | Stmt::Assign { var, value, .. } => {
            names.insert(var.clone());
            collect_bound_names_expr(value, names);
        }
        Stmt::ArrayStore { index, value, .. } => {
            collect_bound_names_expr(index, names);
            collect_bound_names_expr(value, names);
        }
        Stmt::Block(stmts, _) => {
            for stmt in stmts {
                collect_bound_names_stmt(stmt, names);
            }
        }
        Stmt::While {
            cond,
            invariant,
            decreases,
            body,
            ..
        } => {
            collect_bound_names_expr(cond, names);
            collect_bound_names_expr(invariant, names);
            if let Some(decreases) = decreases {
                collect_bound_names_expr(decreases, names);
            }
            collect_bound_names_stmt(body, names);
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
            collect_bound_names_stmt(body, names);
        }
        Stmt::TaskGroup { children, .. } => {
            for child in children {
                collect_bound_names_stmt(child, names);
            }
        }
        Stmt::Expr(expr, _) => collect_bound_names_expr(expr, names),
        Stmt::Cancel { .. } => {}
    }
}

fn collect_bound_names_expr(expr: &Expr, names: &mut std::collections::HashSet<String>) {
    match expr {
        Expr::Paren(inner) => collect_bound_names_expr(inner, names),
        Expr::ArrayLit(elements) => {
            for element in elements {
                collect_bound_names_expr(element, names);
            }
        }
        Expr::ArrayAccess(_, index) => collect_bound_names_expr(index, names),
        Expr::BinaryOp(left, _, right) => {
            collect_bound_names_expr(left, names);
            collect_bound_names_expr(right, names);
        }
        Expr::Block(stmt) => collect_bound_names_stmt(stmt, names),
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_bound_names_expr(cond, names);
            collect_bound_names_stmt(then_branch, names);
            collect_bound_names_stmt(else_branch, names);
        }
        Expr::Call(_, args) | Expr::Perform { args, .. } => {
            for arg in args {
                collect_bound_names_expr(arg, names);
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_bound_names_expr(value, names);
            }
        }
        Expr::FieldAccess(inner, _) => collect_bound_names_expr(inner, names),
        Expr::Match { target, arms } => {
            collect_bound_names_expr(target, names);
            for arm in arms {
                collect_pattern_bound_names(&arm.pattern, names);
                if let Some(guard) = &arm.guard {
                    collect_bound_names_expr(guard, names);
                }
                collect_bound_names_stmt(&arm.body, names);
            }
        }
        Expr::Async { body } => collect_bound_names_stmt(body, names),
        Expr::Lambda { params, body, .. } => {
            names.extend(params.iter().map(|param| param.name.clone()));
            collect_bound_names_stmt(body, names);
        }
        Expr::Await { expr } => collect_bound_names_expr(expr, names),
        Expr::CallRef { callee, args } => {
            collect_bound_names_expr(callee, names);
            for arg in args {
                collect_bound_names_expr(arg, names);
            }
        }
        Expr::ChanSend { channel, value } => {
            collect_bound_names_expr(channel, names);
            collect_bound_names_expr(value, names);
        }
        Expr::ChanRecv { channel } => collect_bound_names_expr(channel, names),
        Expr::Number(_)
        | Expr::Float(_)
        | Expr::StringLit(_)
        | Expr::Variable(_)
        | Expr::AtomRef { .. } => {}
    }
}

fn collect_pattern_bound_names(
    pattern: &crate::parser::Pattern,
    names: &mut std::collections::HashSet<String>,
) {
    match pattern {
        crate::parser::Pattern::Variable(name) => {
            names.insert(name.clone());
        }
        crate::parser::Pattern::Variant { fields, .. } => {
            for field in fields {
                collect_pattern_bound_names(field, names);
            }
        }
        crate::parser::Pattern::Wildcard | crate::parser::Pattern::Literal(_) => {}
    }
}

fn atom_stores_to_array_depth(
    module_env: &ModuleEnv,
    callee: &crate::parser::Atom,
    param_name: &str,
    visited: &mut std::collections::HashSet<String>,
    depth: u8,
) -> bool {
    if depth > 6 {
        return true;
    }
    if !visited.insert(format!("{}::{}", callee.name, param_name)) {
        return false;
    }
    let Ok(body) = crate::parser::parse_body_expr_checked(&callee.body_expr) else {
        return true;
    };
    let mut bound_names = bound_names_in_stmt(&body);
    bound_names.extend(callee.params.iter().map(|param| param.name.clone()));
    stmt_stores_to_array(module_env, &body, param_name, &bound_names, visited, depth)
}

/// `stmt` may store through the variable `name` — Stmt-level sibling of
/// `atom_stores_to_array` for bodies without an `Atom` wrapper (lambdas).
pub(crate) fn stmt_stores_to_var(
    module_env: &ModuleEnv,
    stmt: &Stmt,
    name: &str,
    lambda_params: &[String],
) -> bool {
    let mut bound_names = bound_names_in_stmt(stmt);
    bound_names.extend(lambda_params.iter().cloned());
    stmt_stores_to_array(
        module_env,
        stmt,
        name,
        &bound_names,
        &mut std::collections::HashSet::new(),
        0,
    )
}

pub(crate) fn expr_stores_to_var(module_env: &ModuleEnv, expr: &Expr, name: &str) -> bool {
    let mut aliases = std::collections::HashSet::new();
    aliases.insert(name.to_string());
    loop {
        let mut additions = std::collections::HashSet::new();
        collect_array_aliases_expr(expr, &aliases, &mut additions);
        let previous_len = aliases.len();
        aliases.extend(additions);
        if aliases.len() == previous_len {
            break;
        }
    }

    let mut bound_names = std::collections::HashSet::new();
    collect_bound_names_expr(expr, &mut bound_names);
    expr_stores_to_array(
        module_env,
        expr,
        &aliases,
        &bound_names,
        &mut std::collections::HashSet::new(),
        0,
    )
}

fn stmt_stores_to_array(
    module_env: &ModuleEnv,
    stmt: &Stmt,
    name: &str,
    bound_names: &std::collections::HashSet<String>,
    visited: &mut std::collections::HashSet<String>,
    depth: u8,
) -> bool {
    let aliases = array_aliases_in_stmt(stmt, name);
    stmt_stores_to_array_with_aliases(module_env, stmt, &aliases, bound_names, visited, depth)
}

pub(crate) fn array_aliases_in_stmt(stmt: &Stmt, name: &str) -> std::collections::HashSet<String> {
    let mut aliases = std::collections::HashSet::new();
    aliases.insert(name.to_string());
    loop {
        let mut additions = std::collections::HashSet::new();
        collect_array_aliases_stmt(stmt, &aliases, &mut additions);
        let previous_len = aliases.len();
        aliases.extend(additions);
        if aliases.len() == previous_len {
            return aliases;
        }
    }
}

fn collect_array_aliases_stmt(
    stmt: &Stmt,
    aliases: &std::collections::HashSet<String>,
    additions: &mut std::collections::HashSet<String>,
) {
    match stmt {
        Stmt::Let { var, value, .. } | Stmt::Assign { var, value, .. } => {
            if expr_mentions_alias(value, aliases) {
                additions.insert(var.clone());
            }
            collect_array_aliases_expr(value, aliases, additions);
        }
        Stmt::ArrayStore { index, value, .. } => {
            collect_array_aliases_expr(index, aliases, additions);
            collect_array_aliases_expr(value, aliases, additions);
        }
        Stmt::Block(stmts, _) => {
            for stmt in stmts {
                collect_array_aliases_stmt(stmt, aliases, additions);
            }
        }
        Stmt::While {
            cond,
            invariant,
            decreases,
            body,
            ..
        } => {
            collect_array_aliases_expr(cond, aliases, additions);
            collect_array_aliases_expr(invariant, aliases, additions);
            if let Some(decreases) = decreases {
                collect_array_aliases_expr(decreases, aliases, additions);
            }
            collect_array_aliases_stmt(body, aliases, additions);
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
            collect_array_aliases_stmt(body, aliases, additions);
        }
        Stmt::TaskGroup { children, .. } => {
            for child in children {
                collect_array_aliases_stmt(child, aliases, additions);
            }
        }
        Stmt::Expr(expr, _) => collect_array_aliases_expr(expr, aliases, additions),
        Stmt::Cancel { .. } => {}
    }
}

fn collect_array_aliases_expr(
    expr: &Expr,
    aliases: &std::collections::HashSet<String>,
    additions: &mut std::collections::HashSet<String>,
) {
    match expr {
        Expr::Paren(inner) => collect_array_aliases_expr(inner, aliases, additions),
        Expr::ArrayLit(elements) => {
            for element in elements {
                collect_array_aliases_expr(element, aliases, additions);
            }
        }
        Expr::ArrayAccess(_, index) => collect_array_aliases_expr(index, aliases, additions),
        Expr::BinaryOp(left, _, right) => {
            collect_array_aliases_expr(left, aliases, additions);
            collect_array_aliases_expr(right, aliases, additions);
        }
        Expr::Block(stmt) => collect_array_aliases_stmt(stmt, aliases, additions),
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_array_aliases_expr(cond, aliases, additions);
            collect_array_aliases_stmt(then_branch, aliases, additions);
            collect_array_aliases_stmt(else_branch, aliases, additions);
        }
        Expr::Call(_, args) | Expr::Perform { args, .. } => {
            for arg in args {
                collect_array_aliases_expr(arg, aliases, additions);
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_array_aliases_expr(value, aliases, additions);
            }
        }
        Expr::FieldAccess(inner, _) => collect_array_aliases_expr(inner, aliases, additions),
        Expr::Match { target, arms } => {
            if expr_mentions_alias(target, aliases) {
                for arm in arms {
                    collect_pattern_bound_names(&arm.pattern, additions);
                }
            }
            collect_array_aliases_expr(target, aliases, additions);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_array_aliases_expr(guard, aliases, additions);
                }
                collect_array_aliases_stmt(&arm.body, aliases, additions);
            }
        }
        Expr::Async { body } | Expr::Lambda { body, .. } => {
            collect_array_aliases_stmt(body, aliases, additions);
        }
        Expr::Await { expr } => collect_array_aliases_expr(expr, aliases, additions),
        Expr::CallRef { callee, args } => {
            collect_array_aliases_expr(callee, aliases, additions);
            for arg in args {
                collect_array_aliases_expr(arg, aliases, additions);
            }
        }
        Expr::ChanSend { channel, value } => {
            collect_array_aliases_expr(channel, aliases, additions);
            collect_array_aliases_expr(value, aliases, additions);
        }
        Expr::ChanRecv { channel } => collect_array_aliases_expr(channel, aliases, additions),
        Expr::Number(_)
        | Expr::Float(_)
        | Expr::StringLit(_)
        | Expr::Variable(_)
        | Expr::AtomRef { .. } => {}
    }
}

fn expr_mentions_alias(expr: &Expr, aliases: &std::collections::HashSet<String>) -> bool {
    field_access_path(expr).is_some_and(|path| aliases.contains(&path))
        || match expr {
            Expr::Paren(inner) => expr_mentions_alias(inner, aliases),
            Expr::Variable(name) => aliases.contains(name),
            Expr::ArrayLit(elements) => elements
                .iter()
                .any(|element| expr_mentions_alias(element, aliases)),
            Expr::ArrayAccess(array, index) => {
                aliases.contains(array) || expr_mentions_alias(index, aliases)
            }
            Expr::BinaryOp(left, _, right) => {
                expr_mentions_alias(left, aliases) || expr_mentions_alias(right, aliases)
            }
            Expr::Block(stmt) => stmt_mentions_alias(stmt, aliases),
            Expr::IfThenElse {
                cond,
                then_branch,
                else_branch,
            } => {
                expr_mentions_alias(cond, aliases)
                    || stmt_mentions_alias(then_branch, aliases)
                    || stmt_mentions_alias(else_branch, aliases)
            }
            Expr::Call(_, args) | Expr::Perform { args, .. } => {
                args.iter().any(|arg| expr_mentions_alias(arg, aliases))
            }
            Expr::StructInit { fields, .. } => fields
                .iter()
                .any(|(_, value)| expr_mentions_alias(value, aliases)),
            Expr::FieldAccess(inner, _) => expr_mentions_alias(inner, aliases),
            Expr::Match { target, arms } => {
                expr_mentions_alias(target, aliases)
                    || arms.iter().any(|arm| {
                        arm.guard
                            .as_deref()
                            .is_some_and(|guard| expr_mentions_alias(guard, aliases))
                            || stmt_mentions_alias(&arm.body, aliases)
                    })
            }
            Expr::Async { body } | Expr::Lambda { body, .. } => stmt_mentions_alias(body, aliases),
            Expr::Await { expr } => expr_mentions_alias(expr, aliases),
            Expr::CallRef { callee, args } => {
                expr_mentions_alias(callee, aliases)
                    || args.iter().any(|arg| expr_mentions_alias(arg, aliases))
            }
            Expr::ChanSend { channel, value } => {
                expr_mentions_alias(channel, aliases) || expr_mentions_alias(value, aliases)
            }
            Expr::ChanRecv { channel } => expr_mentions_alias(channel, aliases),
            Expr::Number(_) | Expr::Float(_) | Expr::StringLit(_) | Expr::AtomRef { .. } => false,
        }
}

fn field_access_path(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Variable(name) => Some(name.clone()),
        Expr::FieldAccess(inner, field) => Some(format!("{}.{}", field_access_path(inner)?, field)),
        _ => None,
    }
}

fn stmt_mentions_alias(stmt: &Stmt, aliases: &std::collections::HashSet<String>) -> bool {
    match stmt {
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => expr_mentions_alias(value, aliases),
        Stmt::ArrayStore {
            array,
            index,
            value,
            ..
        } => {
            aliases.contains(array)
                || expr_mentions_alias(index, aliases)
                || expr_mentions_alias(value, aliases)
        }
        Stmt::Block(stmts, _) => stmts.iter().any(|stmt| stmt_mentions_alias(stmt, aliases)),
        Stmt::While {
            cond,
            invariant,
            decreases,
            body,
            ..
        } => {
            expr_mentions_alias(cond, aliases)
                || expr_mentions_alias(invariant, aliases)
                || decreases
                    .as_deref()
                    .is_some_and(|decreases| expr_mentions_alias(decreases, aliases))
                || stmt_mentions_alias(body, aliases)
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => stmt_mentions_alias(body, aliases),
        Stmt::TaskGroup { children, .. } => children
            .iter()
            .any(|child| stmt_mentions_alias(child, aliases)),
        Stmt::Expr(expr, _) => expr_mentions_alias(expr, aliases),
        Stmt::Cancel { .. } => false,
    }
}

fn stmt_stores_to_array_with_aliases(
    module_env: &ModuleEnv,
    stmt: &Stmt,
    aliases: &std::collections::HashSet<String>,
    bound_names: &std::collections::HashSet<String>,
    visited: &mut std::collections::HashSet<String>,
    depth: u8,
) -> bool {
    match stmt {
        Stmt::ArrayStore { array, .. } => aliases.contains(array),
        Stmt::Block(stmts, _) => stmts.iter().any(|stmt| {
            stmt_stores_to_array_with_aliases(
                module_env,
                stmt,
                aliases,
                bound_names,
                visited,
                depth,
            )
        }),
        Stmt::While {
            cond,
            invariant,
            decreases,
            body,
            ..
        } => {
            expr_stores_to_array(module_env, cond, aliases, bound_names, visited, depth)
                || expr_stores_to_array(module_env, invariant, aliases, bound_names, visited, depth)
                || decreases.as_deref().is_some_and(|d| {
                    expr_stores_to_array(module_env, d, aliases, bound_names, visited, depth)
                })
                || stmt_stores_to_array_with_aliases(
                    module_env,
                    body,
                    aliases,
                    bound_names,
                    visited,
                    depth,
                )
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => stmt_stores_to_array_with_aliases(
            module_env,
            body,
            aliases,
            bound_names,
            visited,
            depth,
        ),
        Stmt::TaskGroup { children, .. } => children.iter().any(|stmt| {
            stmt_stores_to_array_with_aliases(
                module_env,
                stmt,
                aliases,
                bound_names,
                visited,
                depth,
            )
        }),
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            expr_stores_to_array(module_env, value, aliases, bound_names, visited, depth)
        }
        Stmt::Expr(expr, _) => {
            expr_stores_to_array(module_env, expr, aliases, bound_names, visited, depth)
        }
        Stmt::Cancel { .. } => false,
    }
}

/// `name` passed as a call argument: the callee receives a `{len, ptr}` fat
/// pointer and may store through it. Resolve the callee and check the
/// matching parameter transitively; unknown callees fail closed.
fn call_may_store_arg(
    module_env: &ModuleEnv,
    callee_name: &str,
    arg_index: usize,
    bound_names: &std::collections::HashSet<String>,
    visited: &mut std::collections::HashSet<String>,
    depth: u8,
) -> bool {
    if bound_names.contains(callee_name) {
        return true;
    }
    let fqn = callee_name.replace('.', "::");
    let callee = module_env
        .get_atom(callee_name)
        .or_else(|| module_env.get_atom(&fqn));
    if callee.is_none() && is_read_only_array_builtin(callee_name) {
        return false;
    }
    match callee {
        Some(callee) => callee.params.get(arg_index).is_none_or(|p| {
            atom_stores_to_array_depth(module_env, callee, &p.name, visited, depth + 1)
        }),
        None => true,
    }
}

fn expr_stores_to_array(
    module_env: &ModuleEnv,
    expr: &Expr,
    aliases: &std::collections::HashSet<String>,
    bound_names: &std::collections::HashSet<String>,
    visited: &mut std::collections::HashSet<String>,
    depth: u8,
) -> bool {
    match expr {
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            expr_stores_to_array(module_env, cond, aliases, bound_names, visited, depth)
                || stmt_stores_to_array_with_aliases(
                    module_env,
                    then_branch,
                    aliases,
                    bound_names,
                    visited,
                    depth,
                )
                || stmt_stores_to_array_with_aliases(
                    module_env,
                    else_branch,
                    aliases,
                    bound_names,
                    visited,
                    depth,
                )
        }
        Expr::Block(stmt) => stmt_stores_to_array_with_aliases(
            module_env,
            stmt,
            aliases,
            bound_names,
            visited,
            depth,
        ),
        Expr::Match { target, arms } => {
            expr_stores_to_array(module_env, target, aliases, bound_names, visited, depth)
                || arms.iter().any(|arm| {
                    arm.guard.as_deref().is_some_and(|g| {
                        expr_stores_to_array(module_env, g, aliases, bound_names, visited, depth)
                    }) || stmt_stores_to_array_with_aliases(
                        module_env,
                        &arm.body,
                        aliases,
                        bound_names,
                        visited,
                        depth,
                    )
                })
        }
        Expr::Async { body } | Expr::Lambda { body, .. } => stmt_stores_to_array_with_aliases(
            module_env,
            body,
            aliases,
            bound_names,
            visited,
            depth,
        ),
        Expr::Call(callee_name, args)
        | Expr::Perform {
            operation: callee_name,
            args,
            ..
        } => {
            args.iter().any(|arg| {
                expr_stores_to_array(module_env, arg, aliases, bound_names, visited, depth)
            }) || args.iter().enumerate().any(|(i, a)| {
                expr_mentions_alias(a, aliases)
                    && call_may_store_arg(module_env, callee_name, i, bound_names, visited, depth)
            })
        }
        Expr::CallRef { callee, args } => {
            expr_stores_to_array(module_env, callee, aliases, bound_names, visited, depth)
                || args.iter().any(|a| {
                    expr_stores_to_array(module_env, a, aliases, bound_names, visited, depth)
                })
                // indirect calls are opaque — passing the array is assumed mutating
                || args.iter().any(|arg| expr_mentions_alias(arg, aliases))
        }
        Expr::ArrayLit(elems) => elems.iter().any(|element| {
            expr_stores_to_array(module_env, element, aliases, bound_names, visited, depth)
        }),
        Expr::ArrayAccess(_, idx) => {
            expr_stores_to_array(module_env, idx, aliases, bound_names, visited, depth)
        }
        Expr::BinaryOp(l, _, r) => {
            expr_stores_to_array(module_env, l, aliases, bound_names, visited, depth)
                || expr_stores_to_array(module_env, r, aliases, bound_names, visited, depth)
        }
        Expr::StructInit { fields, .. } => fields.iter().any(|(_, value)| {
            expr_stores_to_array(module_env, value, aliases, bound_names, visited, depth)
        }),
        Expr::FieldAccess(e, _) => {
            expr_stores_to_array(module_env, e, aliases, bound_names, visited, depth)
        }
        Expr::Await { expr } => {
            expr_stores_to_array(module_env, expr, aliases, bound_names, visited, depth)
        }
        Expr::ChanSend { channel, value } => {
            expr_stores_to_array(module_env, channel, aliases, bound_names, visited, depth)
                || expr_stores_to_array(module_env, value, aliases, bound_names, visited, depth)
        }
        Expr::ChanRecv { channel } => {
            expr_stores_to_array(module_env, channel, aliases, bound_names, visited, depth)
        }
        _ => false,
    }
}

/// After `callee(args)` returns, havoc the caller-side tracked chain of each
/// `var` argument whose callee parameter may be stored through — the callee
/// receives the `{len, ptr}` fat pointer by value, so its `arr[i] = v`
/// writes land in the caller-visible buffer and post-call reads cannot
/// claim pre-call elements.
pub(crate) fn havoc_array_args<'a>(
    vc: &VCtx<'a>,
    callee: &crate::parser::Atom,
    args: &[Expr],
    env: &mut Env<'a>,
) {
    for name in super::may_write::atom_may_write_args(vc.module_env, callee, args) {
        havoc_array_name(vc, &name, env);
    }
}

pub(crate) fn coerce_array_store_value<'a>(
    vc: &VCtx<'a>,
    array: &str,
    value: Dynamic<'a>,
) -> DynResult<'a> {
    let element_type = array_element_type_name(array, vc);
    validate_array_element_type(vc, &element_type)?;
    let sort = array_element_sort(array, vc);
    if sort == ArrayElementSort::Enum {
        check_enum_array_store_type(vc, &element_type, &value)?;
    }
    coerce_to_array_elem_sort(vc, sort, &value)
}

pub(crate) fn check_enum_array_store_type(
    vc: &VCtx<'_>,
    element_type: &str,
    value: &Dynamic<'_>,
) -> Result<(), MumeiError> {
    let expected = z3_element_sort(vc, element_type);
    if value.get_sort() != expected {
        return Err(MumeiError::type_error(format!(
            "Array store value has enum sort {}, expected {}",
            value.get_sort(),
            expected
        )));
    }
    Ok(())
}

/// Coerce `value` to the given array element sort (the name-keyed version
/// above resolves the sort from the array's declared type; array literals
/// carry their sort alongside the value instead).
pub(crate) fn coerce_to_array_elem_sort<'a>(
    vc: &VCtx<'a>,
    sort: ArrayElementSort,
    value: &Dynamic<'a>,
) -> DynResult<'a> {
    match sort {
        ArrayElementSort::Int => as_int_like(value).map(Into::into).ok_or_else(|| {
            MumeiError::type_error(format!(
                "Array store value must be integer (got sort {:?})",
                value.get_sort()
            ))
        }),
        ArrayElementSort::Real => value
            .as_real()
            .or_else(|| as_int_like(value).map(|i| i.to_real()))
            .map(Into::into)
            .ok_or_else(|| MumeiError::type_error("Array store value must be real")),
        ArrayElementSort::Float => {
            let rne = round_nearest_even(vc.ctx);
            coerce_to_float(vc.ctx, value, &rne)
                .map(Into::into)
                .ok_or_else(|| MumeiError::type_error("Array store value must be float"))
        }
        ArrayElementSort::Bool => value
            .as_bool()
            .map(Into::into)
            .ok_or_else(|| MumeiError::type_error("Array store value must be boolean")),
        ArrayElementSort::Str => value
            .as_string()
            .map(Into::into)
            .ok_or_else(|| MumeiError::type_error("Array store value must be a string")),
        ArrayElementSort::Enum => value
            .as_datatype()
            .map(Into::into)
            .ok_or_else(|| MumeiError::type_error("Array store value must be an enum datatype")),
        ArrayElementSort::Struct => Err(MumeiError::type_error(
            "whole-struct array stores must write each scalar leaf",
        )),
        ArrayElementSort::UnsupportedStruct => Err(MumeiError::type_error(
            "array element struct does not have supported scalar leaves",
        )),
        ArrayElementSort::UnsupportedEnum => Err(MumeiError::type_error(
            "array element enum is not a supported finite ADT",
        )),
        ArrayElementSort::Nested => Err(MumeiError::type_error(
            "array-valued elements (`[[T]]`) are not supported for array stores",
        )),
    }
}

pub(crate) fn param_z3_value<'a>(
    ctx: &'a Context,
    name: &str,
    type_name: Option<&str>,
    module_env: &ModuleEnv,
    ieee754_f64: bool,
    bitvec_i64: bool,
) -> Dynamic<'a> {
    let base = type_name
        .map(|t| module_env.resolve_base_type(t))
        .unwrap_or_else(|| "i64".to_string());
    if type_name.is_some_and(|ty| ty.starts_with('[') && ty.ends_with(']')) {
        let int_sort = z3::Sort::int(ctx);
        let elem_type = array_element_type_from_annotation(type_name, module_env);
        let elem_sort = z3_sort_for_lowered(ctx, &lower(&elem_type), ieee754_f64);
        Array::new_const(ctx, name, &int_sort, &elem_sort).into()
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
            // Under the opt-in `--bitvec-i64` mode `i64` is declared as a
            // 64-bit bit-vector, so bitwise operators have real bit semantics
            // and `+`/`-`/`*` wrap like machine arithmetic. See
            // `docs/SPEC_GUIDE.md` § "Bit-Vector Mode".
            LoweredType::I64 if bitvec_i64 => BV::new_const(ctx, name, I64_BITS).into(),
            // A `u64` leaf is a machine word in every mode: the unbounded
            // `Int` encoding would prove `a + 1 > a` at `u64::MAX`, where the
            // compiled code wraps, so it always encodes as `BV(64)` — the
            // unsigned op selection reads `unsigned` from the declared type,
            // not from `--bitvec-i64` (`pure-i64-composite/v10`).
            LoweredType::U64 => BV::new_const(ctx, name, I64_BITS).into(),
            _ => Int::new_const(ctx, name).into(),
        }
    }
}
