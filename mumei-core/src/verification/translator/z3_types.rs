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
    bitvec_i64: bool,
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
            param_z3_value(
                ctx,
                &key,
                Some(component_type),
                module_env,
                ieee754_f64,
                bitvec_i64,
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

pub(crate) fn array_element_sort_from_type(type_name: &str, ieee754_f64: bool) -> ArrayElementSort {
    match lower(type_name) {
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
fn dynamic_ite<'a>(c: &Bool<'a>, t: &Dynamic<'a>, e: &Dynamic<'a>) -> Option<Dynamic<'a>> {
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
    let acc = merged.get_or_insert_with(|| base_env.clone());
    for (key, base_val) in base_env.iter() {
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
pub(crate) fn array_len_symbol<'a>(
    ctx: &'a Context,
    len_name: &str,
    bitvec_i64: bool,
) -> Dynamic<'a> {
    if bitvec_i64 {
        BV::new_const(ctx, len_name, I64_BITS).into()
    } else {
        Int::new_const(ctx, len_name).into()
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
    let len_name = format!("len_{}", array);
    if let Some(existing) = env.get(&len_name) {
        return existing.clone();
    }
    let len = array_len_symbol(ctx, &len_name, bitvec_i64);
    if let (Some(solver), Some(nonneg)) = (solver_opt, nonneg_constraint(ctx, &len)) {
        solver.assert(&nonneg);
    }
    env.insert(len_name, len.clone());
    len
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
fn z3_element_sort<'a>(ctx: &'a Context, type_name: &str, ieee754_f64: bool) -> z3::Sort<'a> {
    z3_sort_for_lowered(ctx, &lower(type_name), ieee754_f64)
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
        ArrayElementSort::Str => {
            let string_sort = z3::Sort::string(ctx);
            Array::new_const(ctx, name, &int_sort, &string_sort)
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
    }
}

pub(crate) fn z3_array_for_name<'a>(vc: &VCtx<'a>, name: &str) -> Array<'a> {
    let int_sort = z3::Sort::int(vc.ctx);
    let elem_sort = z3_element_sort(vc.ctx, &array_element_type_name(name, vc), vc.ieee754_f64);
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
) {
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
        if env.contains_key(&format!("__z3_arr_{name}")) || env.contains_key(&format!("len_{name}"))
        {
            let sort = vc
                .local_array_elem_types
                .borrow()
                .get(name)
                .map(|s| array_element_sort_from_type(s.as_str(), vc.ieee754_f64))
                .unwrap_or_else(|| array_element_sort(name, vc));
            env.insert(
                format!("__z3_arr_{name}"),
                z3_array_for_sort(vc.ctx, &format!("{name}#rebound"), sort).into(),
            );
            env.insert(
                format!("len_{name}"),
                array_len_symbol(vc.ctx, &format!("len_{name}#rebound"), vc.bitvec_i64),
            );
        }
        vc.local_array_elem_types.borrow_mut().remove(name);
        return;
    };
    let elem_ty = z3_range_type_name(&arr.get_sort());
    let cond_hint = arr.nth_child(0).and_then(|c| c.as_bool());
    let arr_dyn: Dynamic = arr.into();
    env.insert(format!("__z3_arr_{name}"), arr_dyn.clone());
    let len: Dynamic = match value {
        Some(Expr::ArrayLit(elements)) => concrete_len_value(vc.ctx, elements.len(), vc.bitvec_i64),
        Some(Expr::Variable(src)) => array_len_value(vc.ctx, env, src, vc.bitvec_i64, None),
        Some(Expr::IfThenElse {
            then_branch,
            else_branch,
            ..
        }) => {
            // `arr` is already `ite(c, then_arr, else_arr)` — merge the
            // branch lengths the same way so `len_<name>` is
            // `ite(c, len_t, len_e)` instead of an unconstrained symbol:
            // `let a = if c { [1,2] } else { [3,4] }; a[1]` is in bounds on
            // both branches. Nested `if`/`match` tails recurse through the
            // value node's own `ite` children.
            match cond_hint {
                Some(cond) => {
                    let len_t = match arr_dyn.nth_child(1) {
                        Some(v) => branch_tail_len(vc, env, name, "then", then_branch, &v, 0),
                        None => {
                            array_len_symbol(vc.ctx, &format!("len_{name}#then"), vc.bitvec_i64)
                        }
                    };
                    let len_e = match arr_dyn.nth_child(2) {
                        Some(v) => branch_tail_len(vc, env, name, "else", else_branch, &v, 0),
                        None => {
                            array_len_symbol(vc.ctx, &format!("len_{name}#else"), vc.bitvec_i64)
                        }
                    };
                    cond.ite(&len_t, &len_e)
                }
                None => array_len_symbol(vc.ctx, &format!("len_{name}#if"), vc.bitvec_i64),
            }
        }
        Some(Expr::Match { arms, .. }) => {
            // The Match eval folds arms in reverse, producing the chain
            // `ite(c_1, v_1, ite(c_2, v_2, …, v_n))` — the last arm's value
            // is the innermost else child and carries no condition of its
            // own (exhaustiveness implies it when all earlier conds fail).
            // Mirror the spine on lengths: `len_<name> = ite(c_1, len_1, …)`.
            match_arm_lens(vc, env, name, "m", arms, &arr_dyn, 0).unwrap_or_else(|| {
                array_len_symbol(vc.ctx, &format!("len_{name}#match"), vc.bitvec_i64)
            })
        }
        _ => array_len_value(vc.ctx, env, name, vc.bitvec_i64, None),
    };
    env.insert(format!("len_{name}"), len);
    vc.local_array_elem_types
        .borrow_mut()
        .insert(name.to_string(), elem_ty.to_string());
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

/// The most recent `let <name> = <rhs>` before `stmt`'s tail — match-arm
/// `let`s stay arm-local and never reach the merged env, so resolving the
/// arm tail's variable here recovers e.g. `A => { let t = [1,2]; t }`.
fn stmt_let_rhs<'e>(stmt: &'e Stmt, name: &str) -> Option<&'e Expr> {
    match stmt {
        Stmt::Let { var, value, .. } if var == name => Some(value.as_ref()),
        Stmt::Block(stmts, _) => stmts.iter().rev().find_map(|s| stmt_let_rhs(s, name)),
        _ => None,
    }
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
    name: &str,
    side: &str,
    arms: &[MatchArm],
    value: &Dynamic<'a>,
    depth: u32,
) -> Option<Dynamic<'a>> {
    if depth >= TAIL_LEN_DEPTH {
        return None;
    }
    let (conds, vals) = match_ite_spine(arms, value)?;
    let mut lens: Vec<Dynamic> = arms
        .iter()
        .zip(vals.iter())
        .enumerate()
        .map(|(i, (arm, val))| {
            branch_tail_len(
                vc,
                env,
                name,
                &format!("{side}{i}"),
                &arm.body,
                val,
                depth + 1,
            )
        })
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
    name: &str,
    side: &str,
    scope: &Stmt,
    val_node: &Dynamic<'a>,
    depth: u32,
) -> Dynamic<'a> {
    match stmt_tail_expr(scope) {
        Some(tail) => tail_len_expr(vc, env, name, side, Some(scope), tail, val_node, depth),
        None => array_len_symbol(vc.ctx, &format!("len_{name}#{side}"), vc.bitvec_i64),
    }
}

/// Array length of a tail expression. `scope` is the enclosing block —
/// used to resolve `let`-bound tail variables whose slots never reached
/// the env (match-arm locals). `val_node` is the Z3 value the tail
/// evaluated to, letting nested `if`/`match` tails mirror their own
/// `ite` conditions on lengths. Anything unresolvable gets a fresh
/// per-side symbol (fail-closed: it only makes proofs harder, never
/// easier).
#[allow(clippy::too_many_arguments)]
pub(crate) fn tail_len_expr<'a>(
    vc: &VCtx<'a>,
    env: &mut Env<'a>,
    name: &str,
    side: &str,
    scope: Option<&Stmt>,
    tail: &Expr,
    val_node: &Dynamic<'a>,
    depth: u32,
) -> Dynamic<'a> {
    let fresh = || array_len_symbol(vc.ctx, &format!("len_{name}#{side}"), vc.bitvec_i64);
    if depth >= TAIL_LEN_DEPTH {
        return fresh();
    }
    match tail {
        Expr::ArrayLit(elements) => concrete_len_value(vc.ctx, elements.len(), vc.bitvec_i64),
        Expr::Variable(src) => match scope.and_then(|s| stmt_let_rhs(s, src)) {
            Some(rhs) => tail_len_expr(vc, env, name, side, scope, rhs, val_node, depth + 1),
            None => array_len_value(vc.ctx, env, src, vc.bitvec_i64, None),
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
                return fresh();
            };
            let len_t = branch_tail_len(
                vc,
                env,
                name,
                &format!("{side}t"),
                then_branch,
                &t_val,
                depth + 1,
            );
            let len_e = branch_tail_len(
                vc,
                env,
                name,
                &format!("{side}e"),
                else_branch,
                &e_val,
                depth + 1,
            );
            cond.ite(&len_t, &len_e)
        }
        Expr::Match { arms, .. } => match_arm_lens(
            vc,
            env,
            name,
            &format!("{side}m"),
            arms,
            val_node,
            depth + 1,
        )
        .unwrap_or_else(fresh),
        _ => fresh(),
    }
}

/// Walk a tracked array value to the innermost (root) array AST node — the
/// base const that `store(store(base, …), …)` chains share. Aliases created
/// by `let b = a` hold the same root even after either side stores.
pub(crate) fn array_root_ast(arr: &z3::ast::Array) -> z3_sys::Z3_ast {
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
        return cur.get_z3_ast();
    }
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
    let elem_sort = match arg_arr.get_sort().array_range().map(|s| s.kind()) {
        Some(z3::SortKind::Real) => ArrayElementSort::Real,
        Some(z3::SortKind::FloatingPoint) => ArrayElementSort::Float,
        Some(z3::SortKind::Bool) => ArrayElementSort::Bool,
        Some(z3::SortKind::Seq) => ArrayElementSort::Str,
        Some(z3::SortKind::Array) => ArrayElementSort::Nested,
        _ => ArrayElementSort::Int,
    };
    let uid = POSTCALL_UID.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let fresh: Dynamic =
        z3_array_for_sort(vc.ctx, &format!("__postcall_arr_{uid}"), elem_sort).into();
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
    for var in var_keys {
        env.insert(var, fresh.clone());
    }
    // Ensure the tracked slot exists even when the arg only had a bare
    // `env[name]` const (param arrays before their first access).
    env.insert(arr_key, fresh);
}

/// Element type name carried by a Z3 array's range sort — the inverse of
/// `z3_sort_for_lowered`. Recursive so a nested `Array(Int, Int)` range comes
/// back as `"[i64]"` rather than collapsing to `"i64"`; `Seq` comes back as
/// `"Str"`.
fn z3_range_type_name(sort: &z3::Sort<'_>) -> String {
    match sort.array_range().map(|range| range.kind()) {
        Some(z3::SortKind::Real) | Some(z3::SortKind::FloatingPoint) => "f64".to_string(),
        Some(z3::SortKind::Bool) => "bool".to_string(),
        Some(z3::SortKind::Seq) => "Str".to_string(),
        Some(z3::SortKind::Array) => format!(
            "[{}]",
            sort.array_range()
                .map(|range| z3_range_type_name(&range))
                .unwrap_or_else(|| "i64".to_string())
        ),
        _ => "i64".to_string(),
    }
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
    stmt_stores_to_array(module_env, &body, param_name, visited, depth)
}

fn stmt_stores_to_array(
    module_env: &ModuleEnv,
    stmt: &Stmt,
    name: &str,
    visited: &mut std::collections::HashSet<String>,
    depth: u8,
) -> bool {
    match stmt {
        Stmt::ArrayStore { array, .. } => array == name,
        Stmt::Block(stmts, _) => stmts
            .iter()
            .any(|s| stmt_stores_to_array(module_env, s, name, visited, depth)),
        Stmt::While {
            cond,
            invariant,
            decreases,
            body,
            ..
        } => {
            expr_stores_to_array(module_env, cond, name, visited, depth)
                || expr_stores_to_array(module_env, invariant, name, visited, depth)
                || decreases
                    .as_deref()
                    .is_some_and(|d| expr_stores_to_array(module_env, d, name, visited, depth))
                || stmt_stores_to_array(module_env, body, name, visited, depth)
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
            stmt_stores_to_array(module_env, body, name, visited, depth)
        }
        Stmt::TaskGroup { children, .. } => children
            .iter()
            .any(|s| stmt_stores_to_array(module_env, s, name, visited, depth)),
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            expr_stores_to_array(module_env, value, name, visited, depth)
        }
        Stmt::Expr(e, _) => expr_stores_to_array(module_env, e, name, visited, depth),
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
    visited: &mut std::collections::HashSet<String>,
    depth: u8,
) -> bool {
    let fqn = callee_name.replace('.', "::");
    match module_env
        .get_atom(callee_name)
        .or_else(|| module_env.get_atom(&fqn))
    {
        Some(callee) => callee.params.get(arg_index).is_none_or(|p| {
            atom_stores_to_array_depth(module_env, callee, &p.name, visited, depth + 1)
        }),
        None => true,
    }
}

fn expr_stores_to_array(
    module_env: &ModuleEnv,
    expr: &Expr,
    name: &str,
    visited: &mut std::collections::HashSet<String>,
    depth: u8,
) -> bool {
    match expr {
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            expr_stores_to_array(module_env, cond, name, visited, depth)
                || stmt_stores_to_array(module_env, then_branch, name, visited, depth)
                || stmt_stores_to_array(module_env, else_branch, name, visited, depth)
        }
        Expr::Match { target, arms } => {
            expr_stores_to_array(module_env, target, name, visited, depth)
                || arms.iter().any(|arm| {
                    arm.guard
                        .as_deref()
                        .is_some_and(|g| expr_stores_to_array(module_env, g, name, visited, depth))
                        || stmt_stores_to_array(module_env, &arm.body, name, visited, depth)
                })
        }
        Expr::Async { body } | Expr::Lambda { body, .. } => {
            stmt_stores_to_array(module_env, body, name, visited, depth)
        }
        Expr::Call(callee_name, args)
        | Expr::Perform {
            operation: callee_name,
            args,
            ..
        } => {
            args.iter()
                .any(|a| expr_stores_to_array(module_env, a, name, visited, depth))
                || args.iter().enumerate().any(|(i, a)| {
                    matches!(a, Expr::Variable(v) if v == name)
                        && call_may_store_arg(module_env, callee_name, i, visited, depth)
                })
        }
        Expr::CallRef { callee, args } => {
            expr_stores_to_array(module_env, callee, name, visited, depth)
                || args.iter().any(|a| expr_stores_to_array(module_env, a, name, visited, depth))
                // indirect calls are opaque — passing the array is assumed mutating
                || args.iter().any(|a| matches!(a, Expr::Variable(v) if v == name))
        }
        Expr::ArrayLit(elems) => elems
            .iter()
            .any(|e| expr_stores_to_array(module_env, e, name, visited, depth)),
        Expr::ArrayAccess(_, idx) => expr_stores_to_array(module_env, idx, name, visited, depth),
        Expr::BinaryOp(l, _, r) => {
            expr_stores_to_array(module_env, l, name, visited, depth)
                || expr_stores_to_array(module_env, r, name, visited, depth)
        }
        Expr::StructInit { fields, .. } => fields
            .iter()
            .any(|(_, e)| expr_stores_to_array(module_env, e, name, visited, depth)),
        Expr::FieldAccess(e, _) => expr_stores_to_array(module_env, e, name, visited, depth),
        Expr::Await { expr } => expr_stores_to_array(module_env, expr, name, visited, depth),
        Expr::ChanSend { channel, value } => {
            expr_stores_to_array(module_env, channel, name, visited, depth)
                || expr_stores_to_array(module_env, value, name, visited, depth)
        }
        Expr::ChanRecv { channel } => {
            expr_stores_to_array(module_env, channel, name, visited, depth)
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
    for (i, arg) in args.iter().enumerate() {
        let Expr::Variable(name) = arg else { continue };
        let Some(param) = callee.params.get(i) else {
            continue;
        };
        if atom_stores_to_array(vc.module_env, callee, &param.name) {
            havoc_array_name(vc, name, env);
        }
    }
}

pub(crate) fn coerce_array_store_value<'a>(
    vc: &VCtx<'a>,
    array: &str,
    value: Dynamic<'a>,
) -> DynResult<'a> {
    coerce_to_array_elem_sort(vc, array_element_sort(array, vc), &value)
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
        let elem_sort = z3_element_sort(
            ctx,
            &array_element_type_from_annotation(type_name, module_env),
            ieee754_f64,
        );
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
            _ => Int::new_const(ctx, name).into(),
        }
    }
}
