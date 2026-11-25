#![allow(unused_imports)]
use super::super::support::*;
use super::super::*;
use super::context::UnfoldCall;
use super::*;
use crate::lowering::{lower, LoweredType};
use crate::verification::raising::{
    mark_abstract_call_result_unraisable, references_call_result, resolve_source_base_type,
};
use crate::verification::translator::may_write::CalleeRef;
use serde_json::json;
use z3::ast::Ast as _;
use z3::{FuncDecl, Sort};

#[derive(Clone)]
struct MatchArrayResult<'a> {
    len: z3::ast::Dynamic<'a>,
    struct_element_type: Option<String>,
    struct_leaves: std::collections::HashMap<String, z3::ast::Array<'a>>,
}

#[inline(never)]
fn merge_match_array_results<'a>(
    arms: &[(z3::ast::Bool<'a>, Option<MatchArrayResult<'a>>)],
) -> Result<Option<MatchArrayResult<'a>>, MumeiError> {
    if arms.iter().any(|(_, arm)| arm.is_some()) && arms.iter().any(|(_, arm)| arm.is_none()) {
        return Err(MumeiError::type_error(
            "match branches have incompatible array results",
        ));
    }
    let mut result: Option<MatchArrayResult<'a>> = None;
    for (cond, arm) in arms.iter().rev() {
        let Some(arm) = arm else {
            if result.is_some() {
                return Err(MumeiError::type_error(
                    "match branches have incompatible array results",
                ));
            }
            continue;
        };
        result = Some(match result {
            Some(else_result) => {
                if arm.struct_element_type != else_result.struct_element_type
                    || arm
                        .struct_leaves
                        .keys()
                        .collect::<std::collections::HashSet<_>>()
                        != else_result
                            .struct_leaves
                            .keys()
                            .collect::<std::collections::HashSet<_>>()
                    || arm.len.get_sort() != else_result.len.get_sort()
                {
                    return Err(MumeiError::type_error(
                        "match branches have incompatible array results",
                    ));
                }
                let len = cond.ite(&arm.len, &else_result.len);
                let mut struct_leaves =
                    std::collections::HashMap::with_capacity(arm.struct_leaves.len());
                for (path, array) in &arm.struct_leaves {
                    let else_array = else_result.struct_leaves.get(path).ok_or_else(|| {
                        MumeiError::type_error(
                            "match branches have incompatible struct-array leaves",
                        )
                    })?;
                    let array_value: z3::ast::Dynamic<'a> = array.clone().into();
                    let else_value: z3::ast::Dynamic<'a> = else_array.clone().into();
                    let merged = cond.ite(&array_value, &else_value);
                    let merged = merged.as_array().ok_or_else(|| {
                        MumeiError::type_error(
                            "match branches have incompatible struct-array leaves",
                        )
                    })?;
                    struct_leaves.insert(path.clone(), merged);
                }
                MatchArrayResult {
                    len,
                    struct_element_type: arm.struct_element_type.clone(),
                    struct_leaves,
                }
            }
            None => arm.clone(),
        });
    }
    Ok(result)
}

#[inline(never)]
fn match_arm_array_result<'a>(
    vc: &VCtx<'a>,
    body: &Stmt,
    body_value: &Dynamic<'a>,
    env: &mut Env<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> MumeiResult<Option<MatchArrayResult<'a>>> {
    let Some(array) = body_value.as_array() else {
        return Ok(None);
    };
    let tail = array_value_tail_expr(body);
    let len = tail
        .map(|tail| {
            super::z3_types::array_len_for_expr(vc, env, Some(tail), body_value, solver_opt)
        })
        .unwrap_or_else(|| super::z3_types::array_len_for_value(vc, env, &array, solver_opt));
    let element_type = tail
        .and_then(|tail| super::z3_types::array_element_type_from_expr(vc, tail))
        .map(|element_type| vc.module_env.resolve_base_type(&element_type));
    let struct_element_type = element_type.filter(|element_type| {
        super::z3_types::array_element_sort_from_type(element_type, vc.module_env, vc.ieee754_f64)
            == ArrayElementSort::Struct
    });
    let struct_leaves = if let Some(element_type) = &struct_element_type {
        let tail = tail.ok_or_else(|| {
            MumeiError::type_error("match result struct array has no resolvable tail expression")
        })?;
        let snapshot =
            super::z3_types::snapshot_struct_array_leaves(vc, env, tail, body_value, None)?
                .ok_or_else(|| {
                    MumeiError::type_error(format!(
                        "match result struct array '{element_type}' has no leaf snapshot"
                    ))
                })?;
        let leaf_types = super::z3_types::struct_array_leaf_types(element_type, vc.module_env)
            .ok_or_else(|| {
                MumeiError::type_error(format!(
                    "array element struct '{element_type}' must have only scalar leaves"
                ))
            })?;
        let mut arrays = std::collections::HashMap::with_capacity(leaf_types.len());
        for (path, _) in leaf_types {
            let array = snapshot.leaf_array(&path).ok_or_else(|| {
                MumeiError::type_error(format!(
                    "match result struct array is missing leaf '{path}'"
                ))
            })?;
            arrays.insert(path, array);
        }
        arrays
    } else {
        std::collections::HashMap::new()
    };
    Ok(Some(MatchArrayResult {
        len,
        struct_element_type,
        struct_leaves,
    }))
}

#[inline(never)]
fn invoke_match_lowering<'a, F>(lowering: F) -> MumeiResult<Dynamic<'a>>
where
    F: FnOnce() -> MumeiResult<Dynamic<'a>>,
{
    lowering()
}

struct LocalArrayElementTypeGuard<'a> {
    types: &'a std::cell::RefCell<std::collections::HashMap<String, String>>,
    name: String,
    previous: Option<String>,
}

impl<'a> LocalArrayElementTypeGuard<'a> {
    fn for_call_result(vc: &'a VCtx<'_>, return_type: Option<&str>) -> Option<Self> {
        let resolved = vc.module_env.resolve_base_type(return_type?);
        if !resolved.starts_with('[') || !resolved.ends_with(']') {
            return None;
        }
        let element_type =
            super::z3_types::array_element_type_from_annotation(Some(&resolved), vc.module_env);
        let name = "result".to_string();
        let previous = vc
            .local_array_elem_types
            .borrow_mut()
            .insert(name.clone(), element_type);
        Some(Self {
            types: &vc.local_array_elem_types,
            name,
            previous,
        })
    }
}

impl Drop for LocalArrayElementTypeGuard<'_> {
    fn drop(&mut self) {
        let mut types = self.types.borrow_mut();
        if let Some(previous) = self.previous.take() {
            types.insert(self.name.clone(), previous);
        } else {
            types.remove(&self.name);
        }
    }
}

struct LocalIntTypeGuard<'a> {
    types: &'a std::cell::RefCell<Vec<(String, &'static str)>>,
}

impl<'a> LocalIntTypeGuard<'a> {
    fn for_refined_operand(
        vc: &'a VCtx<'_>,
        decl: &crate::parser::ast::RefinedType,
    ) -> Option<Self> {
        let base = vc.module_env.resolve_base_type(&decl._base_type);
        let int_type = fixed_width_int(&base).map(|(ty, _, _)| ty)?;
        vc.refined_operand_types
            .borrow_mut()
            .push((decl.operand.clone(), int_type));
        Some(Self {
            types: &vc.refined_operand_types,
        })
    }
}

impl Drop for LocalIntTypeGuard<'_> {
    fn drop(&mut self) {
        self.types.borrow_mut().pop();
    }
}

fn field_access_chain_text(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Variable(name) => Some(name.clone()),
        Expr::FieldAccess(inner, field) => {
            Some(format!("{}.{}", field_access_chain_text(inner)?, field))
        }
        _ => None,
    }
}

fn array_value_tail_expr(stmt: &Stmt) -> Option<&Expr> {
    match stmt {
        Stmt::Expr(expr, _) => Some(expr),
        Stmt::Block(stmts, _) => stmts.last().and_then(array_value_tail_expr),
        _ => None,
    }
}

fn collect_array_value_sources(expr: &Expr, sources: &mut std::collections::HashSet<String>) {
    match expr {
        Expr::Variable(name) => {
            sources.insert(name.clone());
        }
        Expr::FieldAccess(_, _) => {
            if let Some(path) = field_access_chain_text(expr) {
                sources.insert(path);
            }
        }
        Expr::Block(stmt) => {
            if let Some(tail) = array_value_tail_expr(stmt) {
                collect_array_value_sources(tail, sources);
            }
        }
        Expr::IfThenElse {
            then_branch,
            else_branch,
            ..
        } => {
            for branch in [then_branch, else_branch] {
                if let Some(tail) = array_value_tail_expr(branch) {
                    collect_array_value_sources(tail, sources);
                }
            }
        }
        Expr::Match { arms, .. } => {
            for arm in arms {
                if let Some(tail) = array_value_tail_expr(&arm.body) {
                    collect_array_value_sources(tail, sources);
                }
            }
        }
        Expr::Call(_, args) | Expr::CallRef { args, .. } => {
            for arg in args {
                collect_array_value_sources(arg, sources);
            }
        }
        Expr::Await { expr } => collect_array_value_sources(expr, sources),
        _ => {}
    }
}

fn array_argument_aliases<'a>(
    arg: &Expr,
    value: &Dynamic<'a>,
    env: &Env<'a>,
) -> std::collections::HashSet<String> {
    let mut sources = std::collections::HashSet::new();
    collect_array_value_sources(arg, &mut sources);

    let mut roots = std::collections::HashSet::new();
    if let Some(array) = value.as_array() {
        roots.insert(super::z3_types::array_root_ast(&array) as usize);
    }
    sources.retain(|name| {
        let tracked_key = format!("__z3_arr_{name}");
        let array = env
            .get(&tracked_key)
            .and_then(|tracked| tracked.as_array())
            .or_else(|| env.get(name).and_then(|tracked| tracked.as_array()));
        if let Some(array) = array {
            roots.insert(super::z3_types::array_root_ast(&array) as usize);
            true
        } else {
            false
        }
    });

    for (key, tracked) in env {
        if tracked.as_array().is_some_and(|array| {
            roots.contains(&(super::z3_types::array_root_ast(&array) as usize))
        }) {
            if let Some(name) = key.strip_prefix("__z3_arr_") {
                sources.insert(name.to_string());
            } else if !key.starts_with("__") {
                sources.insert(key.clone());
            }
        }
    }

    sources
}

fn array_source_is_declared_array(vc: &VCtx<'_>, name: &str, env: &Env<'_>) -> bool {
    let tracked_key = format!("__z3_arr_{name}");
    if env
        .get(&tracked_key)
        .and_then(|value| value.as_array())
        .is_some()
        || env.get(name).and_then(|value| value.as_array()).is_some()
        || env.contains_key(&super::z3_types::array_len_key(name))
        || vc.local_array_elem_types.borrow().contains_key(name)
        || vc.current_atom.is_some_and(|atom| {
            atom.params.iter().any(|param| {
                param.name == name
                    && param
                        .type_name
                        .as_deref()
                        .is_some_and(|type_name| type_name.starts_with('['))
            })
        })
    {
        return true;
    }
    let expression = parse_expression(name);
    crate::verification::support::datatype::declared_type_of_expr(vc, &expression)
        .is_some_and(|type_name| type_name.trim().starts_with('['))
}

fn mark_array_value_sources_shared<'a>(
    vc: &VCtx<'a>,
    env: &mut Env<'a>,
    expression: &Expr,
    value: &Dynamic<'a>,
) -> MumeiResult<()> {
    if value.as_array().is_none() {
        return Ok(());
    }
    let mut sources = std::collections::HashSet::new();
    collect_array_value_sources(expression, &mut sources);
    let unresolved = sources
        .iter()
        .filter(|name| {
            array_source_is_declared_array(vc, name, env)
                && env
                    .get(&format!("__z3_arr_{name}"))
                    .and_then(|value| value.as_array())
                    .or_else(|| env.get(*name).and_then(|value| value.as_array()))
                    .is_none()
        })
        .cloned()
        .collect::<Vec<_>>();
    if let Some(name) = unresolved.first() {
        return Err(MumeiError::type_error(format!(
            "array payload source '{name}' cannot be resolved for shared-buffer tracking"
        )));
    }

    let aliases = array_argument_aliases(expression, value, env);
    if sources
        .iter()
        .any(|name| array_source_is_declared_array(vc, name, env))
        && aliases.is_empty()
    {
        return Err(MumeiError::type_error(
            "array payload source cannot be resolved for shared-buffer tracking",
        ));
    }
    for name in aliases {
        super::z3_types::mark_array_shared(vc.ctx, env, &name);
    }
    Ok(())
}

fn reject_later_array_argument_mutation<'a>(
    vc: &VCtx<'a>,
    arg: &Expr,
    earlier_array_aliases: &[std::collections::HashSet<String>],
) -> Result<(), MumeiError> {
    for aliases in earlier_array_aliases {
        let mut names = aliases.iter().collect::<Vec<_>>();
        names.sort();
        if let Some(name) = names
            .into_iter()
            .find(|name| super::z3_types::expr_stores_to_var(vc.module_env, arg, name.as_str()))
        {
            return Err(MumeiError::type_error(format!(
                "later call argument may mutate array root '{name}' passed by an earlier argument"
            )));
        }
    }
    Ok(())
}

fn reject_shared_array_mutating_call_args<'a>(
    vc: &VCtx<'a>,
    callee: CalleeRef<'_>,
    args: &[Expr],
    arg_values: &[Dynamic<'a>],
    env: &Env<'a>,
) -> Result<(), MumeiError> {
    for name in super::may_write::callee_may_write_args(vc, callee, args, env) {
        if super::z3_types::is_array_shared(env, &name) {
            return Err(super::z3_types::shared_array_mutation_error(&name));
        }
    }
    for index in super::may_write::callee_may_write_arg_indices(vc, callee, args, env) {
        let (Some(arg), Some(value)) = (args.get(index), arg_values.get(index)) else {
            continue;
        };
        if value.as_array().is_some() && matches!(arg, Expr::FieldAccess(_, _)) {
            if let Some(name) = field_access_chain_text(arg) {
                return Err(super::z3_types::shared_array_mutation_error(&name));
            }
        }
    }
    Ok(())
}

/// Generations of constant-argument contract instances unfolded below a congruent call (§8 D7).
pub(crate) const CONSTANT_UNFOLD_DEPTH: usize = 32;
/// Upper bound on the total number of unfolded instances per top-level call.
pub(crate) const CONSTANT_UNFOLD_MAX_INSTANCES: usize = 256;

/// Z3 value of `perform <NonDeterministic>.<op>(args)`.
///
/// The source is modelled as an uninterpreted function of all its arguments: the
/// witness parameter (seed/timestamp/input) is universally quantified like any
/// other input, and two performs with equal arguments denote the same value.
/// That is exactly the replay guarantee — the result may be anything, but it
/// is a deterministic function of the explicit inputs. An argument-free
/// perform becomes a zero-arity function (a single unknown constant).
pub(crate) fn nondeterministic_perform_result<'a>(
    ctx: &'a Context,
    result_name: &str,
    args: &[Dynamic<'a>],
    bitvec_i64: bool,
) -> Dynamic<'a> {
    let domain_args: Vec<&dyn Ast<'a>> = args.iter().map(|arg| arg as &dyn Ast<'a>).collect();
    let range = if bitvec_i64 {
        Sort::bitvector(ctx, I64_BITS)
    } else {
        Sort::int(ctx)
    };
    let domain: Vec<Sort<'a>> = domain_args.iter().map(|arg| arg.get_sort()).collect();
    let domain_refs: Vec<&Sort<'a>> = domain.iter().collect();
    let decl = FuncDecl::new(ctx, format!("__nd_{}", result_name), &domain_refs, &range);
    decl.apply(&domain_args)
}

fn decimal_mul(lhs: &str, rhs: &str) -> String {
    if lhs == "0" || rhs == "0" {
        return "0".to_string();
    }

    let lhs_digits: Vec<u32> = lhs.bytes().rev().map(|b| (b - b'0') as u32).collect();
    let rhs_digits: Vec<u32> = rhs.bytes().rev().map(|b| (b - b'0') as u32).collect();
    let mut result = vec![0u32; lhs_digits.len() + rhs_digits.len() + 1];

    for (i, &lhs_digit) in lhs_digits.iter().enumerate() {
        let mut carry = 0u32;
        for (j, &rhs_digit) in rhs_digits.iter().enumerate() {
            let idx = i + j;
            let total = result[idx] + lhs_digit * rhs_digit + carry;
            result[idx] = total % 10;
            carry = total / 10;
        }
        let mut idx = i + rhs_digits.len();
        while carry > 0 {
            let total = result[idx] + carry;
            result[idx] = total % 10;
            carry = total / 10;
            idx += 1;
        }
    }

    while result.len() > 1 && result.last() == Some(&0) {
        result.pop();
    }

    result
        .into_iter()
        .rev()
        .map(|digit| char::from(b'0' + digit as u8))
        .collect()
}

fn pow_i64_to_decimal_string(base: i64, exponent: u64) -> String {
    if exponent == 0 {
        return "1".to_string();
    }

    let negative = base < 0 && exponent % 2 == 1;
    let mut factor = i128::from(base).abs().to_string();
    let mut power = exponent;
    let mut result = "1".to_string();

    while power > 0 {
        if power & 1 == 1 {
            result = decimal_mul(&result, &factor);
        }
        power >>= 1;
        if power > 0 {
            factor = decimal_mul(&factor, &factor);
        }
    }

    if negative {
        format!("-{result}")
    } else {
        result
    }
}

fn unsupported_exponentiation_error() -> MumeiError {
    MumeiError::verification(
        "Unsupported exponentiation: exponent must be a non-negative integer constant",
    )
}

/// Lower a binary operation whose operands are `i64` values encoded as
/// `BV(64)` (the opt-in `--bitvec-i64` mode).
///
/// Arithmetic uses the bit-vector operators, so `+`, `-`, `*` wrap in two's
/// complement exactly like machine `i64` instead of growing unboundedly as in
/// the default `Int` encoding. Comparisons and division are the *signed*
/// variants (`bvslt`, `bvsdiv`, …) because `i64` is signed, and `>>` is the
/// arithmetic shift `bvashr`.
///
/// Shift amounts must satisfy `0 <= n < 64`; that is a precondition the
/// contract has to establish in `requires` (Z3's `bvshl`/`bvashr` are total
/// but their out-of-range behavior is not the language's), so when a solver is
/// available and the amount can leave the range, verification fails with an
/// actionable error rather than proving something about saturating shifts.
fn bv_shift_out_of_range<'a>(ctx: &'a Context, amount: &BV<'a>) -> Bool<'a> {
    let zero = BV::from_i64(ctx, 0, I64_BITS);
    let width = BV::from_i64(ctx, I64_BITS as i64, I64_BITS);
    Bool::or(ctx, &[&amount.bvslt(&zero), &amount.bvsge(&width)])
}

fn out_of_range_shift_error() -> MumeiError {
    MumeiError::verification("Shift amount may be outside 0..64".to_string())
        .with_help("Add `n >= 0 && n < 64` for the shift amount to requires")
        .with_verdict(crate::verification::VerdictKind::Rejected)
}

fn undecided_shift_error() -> MumeiError {
    MumeiError::verification(
        "Z3 returned unknown while checking that the shift amount stays in 0..64".to_string(),
    )
    .with_help(
        "State `n >= 0 && n < 64` for the shift amount in requires, or raise --solver-timeout",
    )
    .with_verdict(crate::verification::VerdictKind::Inconclusive)
}

/// True when the callee's contract was proved in the same `i64` encoding the
/// caller is being verified in.
///
/// A callee's `ensures` may only be imported as a fact when both sides agree on
/// the encoding: `ensures: result > x` for `x + 1` holds under the unbounded
/// `Int` encoding, but is false at `i64::MAX` under two's complement wrapping,
/// so importing it into a bit-vector proof would assume something the callee
/// never proved. The mode of a callee is its own requirement (a bitwise
/// operator, `semantics: bitvec;`, or a bit-vector callee of its own), plus the
/// whole-run `--bitvec-i64` opt-in, under which every atom is a bit-vector
/// atom — including the callees of a caller that would have needed the encoding
/// anyway.
fn callee_semantics_match_caller(vc: &VCtx<'_>, callee: &Atom) -> bool {
    let callee_bv = vc.bitvec_i64_global
        || crate::verification::fragment::atom_requires_bitvector_semantics_in_module(
            callee,
            vc.module_env,
        );
    callee_bv == vc.bitvec_i64
}

fn user_defines_callee(vc: &VCtx<'_>, name: &str) -> bool {
    vc.local_lambdas.borrow().contains_key(name) || vc.module_env.get_atom(name).is_some()
}

/// The expected Z3 sort of a callee parameter under the caller's encoding.
fn param_sort_probe<'a>(vc: &VCtx<'a>, tag: &str, type_name: Option<&str>) -> Sort<'a> {
    datatype::param_z3_value_for_vc(vc, tag, type_name).get_sort()
}

/// Whether each argument lowers to the sort the callee parameter declares.
fn call_arg_sorts_match<'a>(vc: &VCtx<'a>, callee: &Atom, arg_vals: &[Dynamic<'a>]) -> bool {
    callee.params.len() == arg_vals.len()
        && callee
            .params
            .iter()
            .zip(arg_vals.iter())
            .all(|(param, val)| {
                val.get_sort() == param_sort_probe(vc, &param.name, param.type_name.as_deref())
            })
}

#[derive(Clone)]
enum RecFnParamKind {
    Scalar,
    Array,
    StructArray(Vec<String>),
    Struct(usize),
}

fn array_element_type_name(type_name: &str) -> Option<&str> {
    let trimmed = type_name.trim();
    trimmed
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .map(str::trim)
}

/// Expand source parameters into the domain arguments of a congruent
/// recursive function. Arrays carry their tracked length and structs are
/// represented by their fields rather than their opaque handles.
fn expand_rec_fn_args<'a, Array, Length, Field, ArrayLeaf>(
    kinds: &[RecFnParamKind],
    arg_vals: &[Dynamic<'a>],
    array_len_sort: &Sort<'a>,
    mut array_for: Array,
    mut length_for: Length,
    mut field_for: Field,
    mut array_leaf_for: ArrayLeaf,
) -> Option<Vec<Dynamic<'a>>>
where
    Array: FnMut(usize) -> Option<Dynamic<'a>>,
    Length: FnMut(usize) -> Option<Dynamic<'a>>,
    Field: FnMut(usize, usize) -> Option<Dynamic<'a>>,
    ArrayLeaf: FnMut(usize, &str) -> Option<Dynamic<'a>>,
{
    if kinds.len() != arg_vals.len() {
        return None;
    }
    let mut expanded = Vec::new();
    for (param_index, (kind, arg_val)) in kinds.iter().zip(arg_vals).enumerate() {
        match kind {
            RecFnParamKind::Scalar => expanded.push(arg_val.clone()),
            RecFnParamKind::Array => {
                let array = array_for(param_index)?;
                if array.get_sort() != arg_val.get_sort() {
                    return None;
                }
                expanded.push(array);
                let length = length_for(param_index)?;
                if length.get_sort() != array_len_sort.clone() {
                    return None;
                }
                expanded.push(length);
            }
            RecFnParamKind::StructArray(paths) => {
                let array = array_for(param_index)?;
                if array.get_sort() != arg_val.get_sort() {
                    return None;
                }
                expanded.push(array);
                let length = length_for(param_index)?;
                if length.get_sort() != array_len_sort.clone() {
                    return None;
                }
                expanded.push(length);
                for path in paths {
                    expanded.push(array_leaf_for(param_index, path)?);
                }
            }
            RecFnParamKind::Struct(field_count) => {
                for field_index in 0..*field_count {
                    expanded.push(field_for(param_index, field_index)?);
                }
            }
        }
    }
    Some(expanded)
}

fn rec_fn_args<'a>(
    vc: &VCtx<'a>,
    callee: &Atom,
    arg_vals: &[Dynamic<'a>],
    call_env: &Env<'a>,
) -> Option<Vec<Dynamic<'a>>> {
    if callee.params.len() != arg_vals.len() {
        return None;
    }
    let mut kinds = Vec::with_capacity(callee.params.len());
    for (param, arg_val) in callee.params.iter().zip(arg_vals) {
        if arg_val.get_sort() != param_sort_probe(vc, &param.name, param.type_name.as_deref()) {
            return None;
        }
        let type_name = param.type_name.as_deref().map(str::trim);
        if let Some(element_type) = type_name.and_then(array_element_type_name) {
            if let Some(leaves) =
                super::z3_types::struct_array_leaf_types(element_type, vc.module_env)
            {
                kinds.push(RecFnParamKind::StructArray(
                    leaves.into_iter().map(|(path, _)| path).collect(),
                ));
            } else {
                kinds.push(RecFnParamKind::Array);
            }
        } else if let Some(sdef) = type_name.and_then(|name| vc.module_env.get_struct(name)) {
            kinds.push(RecFnParamKind::Struct(sdef.fields.len()));
        } else {
            kinds.push(RecFnParamKind::Scalar);
        }
    }
    let array_len_sort = array_len_symbol(vc.ctx, "probe", vc.bitvec_i64).get_sort();
    expand_rec_fn_args(
        &kinds,
        arg_vals,
        &array_len_sort,
        |param_index| {
            call_env
                .get(&format!("__z3_arr_{}", callee.params[param_index].name))
                .cloned()
        },
        |param_index| {
            call_env
                .get(&array_len_key(&callee.params[param_index].name))
                .cloned()
        },
        |param_index, field_index| {
            let type_name = callee.params[param_index].type_name.as_deref()?.trim();
            let sdef = vc.module_env.get_struct(type_name)?;
            let field = sdef.fields.get(field_index)?;
            call_env
                .get(&struct_field_key(
                    &callee.params[param_index].name,
                    &field.name,
                ))
                .cloned()
        },
        |param_index, path| {
            call_env
                .get(&super::z3_types::struct_array_leaf_key(
                    &callee.params[param_index].name,
                    path,
                ))
                .cloned()
        },
    )
}

fn struct_field_values_for_call<'a>(
    vc: &VCtx<'a>,
    callee: &Atom,
    arg_vals: &[Dynamic<'a>],
    call_env: &Env<'a>,
    caller_env: &Env<'a>,
) -> Option<Vec<Dynamic<'a>>> {
    if callee.params.len() != arg_vals.len() {
        return None;
    }
    let mut field_vals = Vec::new();
    for (param, arg_val) in callee.params.iter().zip(arg_vals) {
        let Some(sdef) = param
            .type_name
            .as_deref()
            .and_then(|name| vc.module_env.get_struct(name))
        else {
            continue;
        };
        if let Some(fields) = struct_fields_of_value(caller_env, arg_val, sdef) {
            field_vals.extend(fields.into_iter().map(|(_, value)| value));
        } else {
            for field in &sdef.fields {
                field_vals.push(
                    call_env
                        .get(&struct_field_key(&param.name, &field.name))?
                        .clone(),
                );
            }
        }
    }
    Some(field_vals)
}

/// `rec_fn#<callee>#<suffix>` application over the callee's expanded
/// domain: the projection UF a non-scalar recursive result needs — `len`
/// for `[T]` results, `field.<name>` per struct field. Sharing the domain
/// makes equal argument tuples produce equal projections, which is what
/// makes a (contents, len) pair or a struct result congruent.
fn rec_fn_result_projection<'a>(
    vc: &VCtx<'a>,
    callee: &Atom,
    suffix: &str,
    domain: &[Sort<'a>],
    range: &Sort<'a>,
    args: &[Dynamic<'a>],
) -> Dynamic<'a> {
    let decl = vc.rec_fn_named(&format!("rec_fn#{}#{}", callee.name, suffix), domain, range);
    let app_args: Vec<&dyn Ast> = args.iter().map(|v| v as &dyn Ast).collect();
    decl.apply(&app_args)
}

const REC_FN_ARRAY_LEN_PROJECTION: &str = "len#result";

/// The result sort a `rec_fn#` application for `callee` must produce.
fn call_result_sort<'a>(vc: &VCtx<'a>, callee: &Atom) -> Sort<'a> {
    let has_float = callee.params.iter().any(|p| {
        p.type_name
            .as_deref()
            .map(|t| vc.module_env.resolve_base_type(t) == "f64")
            .unwrap_or(false)
    });
    let result_type = callee
        .return_type
        .as_deref()
        .or(if has_float { Some("f64") } else { None });
    param_sort_probe(
        vc,
        &format!("__recsort_result_{}", callee.name),
        result_type,
    )
}

fn constant_args<'a>(args: &[Dynamic<'a>]) -> Option<(Vec<Dynamic<'a>>, String)> {
    let mut simplified_args = Vec::with_capacity(args.len());
    let mut keys = Vec::with_capacity(args.len());
    for arg in args {
        let simplified = arg.simplify();
        let key = if simplified.as_int().is_some() {
            if simplified.kind() != z3::AstKind::Numeral {
                return None;
            }
            format!("int:{simplified}")
        } else if let Some(value) = simplified.as_bv() {
            let numeral = value
                .as_u64()
                .map(|value| format!("u{value}"))
                .or_else(|| value.as_i64().map(|value| format!("i{value}")))?;
            format!("bv{}:{numeral}", value.get_size())
        } else {
            let value = simplified.as_bool()?;
            format!("bool:{}", if value.as_bool()? { "true" } else { "false" })
        };
        simplified_args.push(simplified);
        keys.push(key);
    }
    Some((simplified_args, keys.join(",")))
}

fn constant_call_key(name: &str, args_key: &str) -> String {
    format!("{name}({args_key})")
}

fn caller_outside_scc(vc: &VCtx<'_>, callee: &Atom) -> bool {
    vc.recursion
        .current_scc
        .borrow()
        .as_ref()
        .map(|scc| !scc.contains(&callee.name))
        .unwrap_or(true)
}

struct UnfoldSinkGuard<'v, 'a> {
    vc: &'v VCtx<'a>,
}

impl<'v, 'a> UnfoldSinkGuard<'v, 'a> {
    fn new(vc: &'v VCtx<'a>) -> Self {
        *vc.recursion.unfold_sink.borrow_mut() = Some(Vec::new());
        Self { vc }
    }

    fn take_frontier(&self) -> Vec<UnfoldCall<'a>> {
        self.vc
            .recursion
            .unfold_sink
            .borrow_mut()
            .take()
            .unwrap_or_default()
    }
}

impl Drop for UnfoldSinkGuard<'_, '_> {
    fn drop(&mut self) {
        self.vc.recursion.unfold_sink.borrow_mut().take();
    }
}

fn parse_closed_contract_expression(raw: &str, description: &str) -> MumeiResult<Expr> {
    crate::parser::parse_expression_checked(raw).map_err(|failures| {
        MumeiError::verification(format!(
            "{} '{}' is not a closed expression: {}",
            description,
            raw,
            failures.join("; ")
        ))
    })
}

fn constant_call_antecedent<'a>(
    vc: &VCtx<'a>,
    atom: &Atom,
    args: &[Dynamic<'a>],
    call_env: &HashMap<String, Dynamic<'a>>,
) -> Option<Bool<'a>> {
    let requires = crate::verification::caller_requires_obligation(atom);
    let mut terms = Vec::new();
    if requires.trim() == "true" {
        terms.push(Bool::from_bool(vc.ctx, true));
    } else {
        let req_ast = crate::parser::parse_expression_checked(&requires).ok()?;
        let mut req_env = call_env.clone();
        let req_value = super::z3_types::with_array_shared_markers_restored(&mut req_env, |env| {
            expr_to_z3_with_domain_obligations(vc, &req_ast, env, None)
        })
        .ok()?;
        terms.push(req_value.as_bool()?);
    }

    for (param, arg) in atom.params.iter().zip(args) {
        let Some(type_name) = param.type_name.as_deref() else {
            continue;
        };
        if vc.module_env.get_type(type_name).is_none() {
            continue;
        };

        // Every conjunct of the declaration chain is part of the
        // antecedent — a chained alias like `Small = Pos where v < 10`
        // must carry `Pos`'s `v > 0` into the constant-call guard too.
        let chain = vc.module_env.refined_chain(type_name).ok()?;
        // `usize` folds to the `u64` leaf — `resolve_base_type` makes
        // the same fold — so a usize-based alias needs the unsigned
        // nonneg bound too.
        let leaf_base = match chain.last().map_or("", |decl| decl._base_type.as_str()) {
            "usize" => "u64",
            other => other,
        };
        if leaf_base == "u64" && arg.as_bv().is_none() {
            // `v >= 0` bounds an `Int` argument to the unsigned leaf; a
            // `BV(64)` argument is already a machine word, where the signed
            // `bv sge 0` reading would wrongly exclude values ≥ 2^63.
            terms.push(nonneg_constraint(vc.ctx, arg)?);
        }

        for decl in chain {
            let mut refinement_env = call_env.clone();
            refinement_env.insert(decl.operand.clone(), arg.clone());
            let predicate_ast = parse_expression(&decl.predicate_raw);
            let predicate_value =
                refined_predicate_to_z3(vc, decl, &predicate_ast, &mut refinement_env, None)
                    .ok()?;
            terms.push(predicate_value.as_bool()?);
        }
    }

    let term_refs: Vec<&Bool> = terms.iter().collect();
    Some(Bool::and(vc.ctx, &term_refs))
}

fn unfold_constant_calls<'a>(
    vc: &VCtx<'a>,
    env: &HashMap<String, Dynamic<'a>>,
    mut frontier: Vec<UnfoldCall<'a>>,
    mut seen: std::collections::HashSet<String>,
    solver: &Solver<'a>,
) {
    let mut asserted_instances = 0;
    for _ in 1..=CONSTANT_UNFOLD_DEPTH {
        if frontier.is_empty() || asserted_instances >= CONSTANT_UNFOLD_MAX_INSTANCES {
            break;
        }
        let mut next_frontier = Vec::new();
        for (name, args) in frontier {
            if asserted_instances >= CONSTANT_UNFOLD_MAX_INSTANCES {
                break;
            }
            let Some((args, args_key)) = constant_args(&args) else {
                continue;
            };
            if !seen.insert(constant_call_key(&name, &args_key)) {
                continue;
            }
            let Some(atom) = vc.module_env.get_atom(&name) else {
                continue;
            };
            if args.len() != atom.params.len() {
                continue;
            }
            if !vc.callee_congruent(atom)
                || !caller_outside_scc(vc, atom)
                || !callee_semantics_match_caller(vc, atom)
                || !call_arg_sorts_match(vc, atom, &args)
            {
                continue;
            }

            let mut call_env = (*env).clone();
            for (param, arg) in atom.params.iter().zip(args.iter()) {
                // An `Int` literal bound to a machine-word parameter must
                // enter the callee's contract under the same `BV(64)`
                // encoding `param_z3_value` gives it — otherwise the
                // clause's `a + 1 > a` is read as unbounded integer
                // arithmetic and the caller proves a value the compiled
                // code never produces.
                let encodes_as_bv = param.type_name.as_deref().is_some_and(|name| {
                    match lower(&vc.module_env.resolve_base_type(name.trim())) {
                        LoweredType::U64 => true,
                        LoweredType::I64 => vc.bitvec_i64,
                        _ => false,
                    }
                });
                let bound = if encodes_as_bv {
                    as_bv_i64(arg)
                        .map(|bv| bv.into())
                        .unwrap_or_else(|| arg.clone())
                } else {
                    arg.clone()
                };
                call_env.insert(param.name.clone(), bound);
            }

            let _contract_guard = vc.enter_contract_instantiation(atom);
            let Some(antecedent) = constant_call_antecedent(vc, atom, &args, &call_env) else {
                continue;
            };
            if antecedent.simplify().as_bool() == Some(false) {
                continue;
            }

            // Declare `rec_fn#<atom>` over the SAME expanded domain the
            // call sites use: `rec_fn_args` widens array params to
            // (contents, len) pairs and struct params to field
            // projections, so seed the `__z3_arr_`/`len_`/`__struct_`
            // slots it looks up — recoverable from the call-result maps
            // when the arg came from a call, otherwise the unfold skips
            // rather than colliding in the UF cache with a raw-domain
            // declaration for the same UF name.
            for (param, arg) in atom.params.iter().zip(args.iter()) {
                let type_name = param.type_name.as_deref().map(str::trim);
                if type_name.and_then(array_element_type_name).is_some() {
                    call_env.insert(format!("__z3_arr_{}", param.name), arg.clone());
                    if let Some(len) = arg.as_array().and_then(|arr| call_result_len(vc, &arr)) {
                        call_env.insert(array_len_key(&param.name), len);
                    } else {
                        // A caller-env slot of the same name must not
                        // leak in as this argument's length.
                        call_env.remove(&array_len_key(&param.name));
                    }
                } else if let Some(sdef) = type_name.and_then(|name| vc.module_env.get_struct(name))
                {
                    let fields = vc
                        .call_result_fields
                        .borrow()
                        .get(&(arg.get_z3_ast() as usize))
                        .map(|(fields, _)| fields.clone());
                    for field in sdef.fields.iter() {
                        let key = struct_field_key(&param.name, &field.name);
                        let value = fields.as_ref().and_then(|fs| {
                            fs.iter()
                                .find(|(n, _)| n == &field.name)
                                .map(|(_, v)| v.clone())
                        });
                        if let Some(value) = value {
                            call_env.insert(key, value);
                        } else {
                            call_env.remove(&key);
                        }
                    }
                }
            }
            let Some(rec_args) = rec_fn_args(vc, atom, &args, &call_env) else {
                continue;
            };
            let domain: Vec<Sort> = rec_args.iter().map(|arg| arg.get_sort()).collect();
            let struct_ret = atom
                .return_type
                .as_deref()
                .and_then(|t| vc.module_env.get_struct(t));
            let result = if let Some(sdef) = struct_ret {
                // Struct results are field-wise congruent: a fresh handle
                // plus a `#field.*` projection UF per field — the same
                // shape the call sites mint.
                static UNFOLD_RESULT_COUNTER: std::sync::atomic::AtomicUsize =
                    std::sync::atomic::AtomicUsize::new(0);
                let id = UNFOLD_RESULT_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let handle_name = format!("unfold_{}_{}", atom.name, id);
                let handle =
                    datatype::param_z3_value_for_vc(vc, &handle_name, atom.return_type.as_deref());
                let result_fields: Vec<(String, Dynamic)> = sdef
                    .fields
                    .iter()
                    .map(|field| {
                        let field_range = param_sort_probe(
                            vc,
                            &format!("__unfold_field_{}_{}", atom.name, field.name),
                            Some(field.type_name.trim()),
                        );
                        (
                            field.name.clone(),
                            rec_fn_result_projection(
                                vc,
                                atom,
                                &format!("field.{}", field.name),
                                &domain,
                                &field_range,
                                &rec_args,
                            ),
                        )
                    })
                    .collect();
                bind_struct_fields(&mut call_env, &handle_name, &result_fields);
                bind_struct_fields(&mut call_env, "result", &result_fields);
                register_call_result_fields(vc, &call_env, &handle, &result_fields);
                handle
            } else {
                let range = call_result_sort(vc, atom);
                let decl = vc.rec_fn(atom, &domain, &range);
                let app_args: Vec<&dyn Ast> = rec_args.iter().map(|arg| arg as &dyn Ast).collect();
                decl.apply(&app_args)
            };
            call_env.insert("result".to_string(), result.clone());
            // Array results: `len(result)` / `result[i]` inside the unfolded
            // ensures resolve to the same `#len` projection the call sites
            // mint.
            if result.as_array().is_some() {
                let len_sort = array_len_symbol(vc.ctx, "probe", vc.bitvec_i64).get_sort();
                let len_sym = rec_fn_result_projection(
                    vc,
                    atom,
                    REC_FN_ARRAY_LEN_PROJECTION,
                    &domain,
                    &len_sort,
                    &rec_args,
                );
                call_env.insert("__z3_arr_result".to_string(), result.clone());
                call_env.insert(array_len_key("result"), len_sym);
            }

            let ensures = crate::verification::contract_view(
                atom,
                crate::verification::ContractView::CallerEnsures,
            );
            if ensures.trim() == "true" {
                continue;
            }
            let ens_ast = parse_expression(&ensures);
            let sink_guard = UnfoldSinkGuard::new(vc);
            let Ok(ens_value) =
                super::z3_types::with_array_shared_markers_restored(&mut call_env, |env| {
                    expr_to_z3_with_domain_obligations(vc, &ens_ast, env, None)
                })
            else {
                continue;
            };
            let Some(ens_bool) = ens_value.as_bool() else {
                continue;
            };
            let nested_calls = sink_guard.take_frontier();
            solver.assert(&antecedent.implies(&ens_bool));
            asserted_instances += 1;
            next_frontier.extend(nested_calls);
        }
        frontier = next_frontier;
    }
}

/// `0 <= A1 ∧ … ∧ 0 <= Ak ∧ lex(B < A)` for Int measures, or the
/// signed bit-vector equivalent. A one-component measure builds exactly
/// `0 <= A ∧ B < A`. Returns None when the arities differ or a component
/// pair lowers to incompatible sorts.
pub(crate) fn termination_measure_obligation<'a>(
    ctx: &'a Context,
    measure_a: &[Dynamic<'a>],
    measure_b: &[Dynamic<'a>],
) -> Option<Bool<'a>> {
    if measure_a.is_empty() || measure_a.len() != measure_b.len() {
        return None;
    }
    if measure_a.len() == 1 {
        if let (Some(a), Some(b)) = (measure_a[0].as_int(), measure_b[0].as_int()) {
            let zero = Int::from_i64(ctx, 0);
            return Some(Bool::and(ctx, &[&zero.le(&a), &b.lt(&a)]));
        }
        if let (Some(a), Some(b)) = (measure_a[0].as_bv(), measure_b[0].as_bv()) {
            if a.get_size() != b.get_size() {
                return None;
            }
            let zero = BV::from_i64(ctx, 0, a.get_size());
            return Some(Bool::and(ctx, &[&zero.bvsle(&a), &b.bvslt(&a)]));
        }
    }

    let mut nonnegative = Vec::with_capacity(measure_a.len());
    let mut lexicographic = None;
    for (a, b) in measure_a.iter().zip(measure_b).rev() {
        let (nonnegative_component, less, equal) =
            if let (Some(a), Some(b)) = (a.as_int(), b.as_int()) {
                let zero = Int::from_i64(ctx, 0);
                (zero.le(&a), b.lt(&a), b._eq(&a))
            } else if let (Some(a), Some(b)) = (a.as_bv(), b.as_bv()) {
                if a.get_size() != b.get_size() {
                    return None;
                }
                let zero = BV::from_i64(ctx, 0, a.get_size());
                (zero.bvsle(&a), b.bvslt(&a), b._eq(&a))
            } else {
                return None;
            };
        nonnegative.push(nonnegative_component);
        lexicographic = Some(match lexicographic {
            Some(tail) => {
                let equal_and_tail = Bool::and(ctx, &[&equal, &tail]);
                Bool::or(ctx, &[&less, &equal_and_tail])
            }
            None => less,
        });
    }
    nonnegative.reverse();
    nonnegative.push(lexicographic?);
    Some(Bool::and(ctx, &nonnegative.iter().collect::<Vec<_>>()))
}

/// Counterexample object naming the current atom's parameter values from a
/// solver model, same shape as the call-site requires counterexample.
pub(crate) fn termination_counterexample<'a>(
    vc: &VCtx<'a>,
    solver: &Solver<'a>,
    env: &Env<'a>,
) -> Option<serde_json::Value> {
    let atom = vc.current_atom?;
    let model = solver.get_model()?;
    let mut ce_json = serde_json::Map::new();
    for param in &atom.params {
        if let Some(var) = env.get(&param.name) {
            if let Some(val) = model.eval(var, true) {
                ce_json.insert(param.name.clone(), json!(format!("{}", val)));
            }
        }
    }
    if ce_json.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(ce_json))
    }
}

/// Error for a failed termination check on an SCC-internal call.
pub(crate) fn termination_violation_error<'a>(
    vc: &VCtx<'a>,
    solver: &Solver<'a>,
    env: &Env<'a>,
    callee_name: &str,
    verdict: VerdictKind,
) -> MumeiError {
    let atom_name = vc.current_atom.map(|a| a.name.as_str()).unwrap_or("?");
    let measure_text = vc
        .current_atom
        .and_then(|a| a.decreases.as_deref())
        .unwrap_or("?");
    let ce = termination_counterexample(vc, solver, env);
    MumeiError::verification(format!(
        "Termination measure violation: call from '{}' to '{}' may not decrease measure '{}'",
        atom_name, callee_name, measure_text
    ))
    .with_counterexample(ce)
    .with_verdict(verdict)
}

/// Whether `callee` is a member of the current atom's SCC and the call sits
/// lexically in the atom's own body/contract (not inside an instantiated
/// callee contract).
fn scc_internal_call<'a>(vc: &VCtx<'a>, callee: &Atom) -> bool {
    vc.contracts_in_instantiation.borrow().is_empty()
        && vc
            .recursion
            .current_scc
            .borrow()
            .as_ref()
            .map(|scc| scc.contains(&callee.name))
            .unwrap_or(false)
}

/// Termination bookkeeping shared by the direct-call and `call(atom_ref(..))`
/// paths: when `callee` is a congruent SCC member and the call is lexically
/// in the current atom's own body/contract (not inside an instantiated callee
/// contract), record or check `0 <= M_A && M_B(args) < M_A`.
fn termination_obligation_at_call<'a>(
    vc: &VCtx<'a>,
    callee: &Atom,
    call_env: &mut Env<'a>,
    env: &Env<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> MumeiResult<()> {
    if !scc_internal_call(vc, callee) {
        return Ok(());
    }
    let Some(measure_a) = vc.recursion.current_measure.borrow().clone() else {
        return Ok(());
    };
    let Some(measure_text) = callee.decreases.clone() else {
        return Ok(());
    };
    let measure_b = measure_components(&measure_text)
        .iter()
        .map(|component| {
            let measure_ast = parse_expression(component);
            super::z3_types::with_array_shared_markers_restored(call_env, |env| {
                expr_to_z3(vc, &measure_ast, env, None)
            })
        })
        .collect::<MumeiResult<Vec<_>>>()?;
    let Some(obligation) = termination_measure_obligation(vc.ctx, &measure_a, &measure_b) else {
        // The measures lowered to incompatible sorts (Int vs BV, or BV of
        // different widths). Skipping the check while still asserting the
        // congruent ensures assumption would be unsound, so fail instead.
        let atom_name = vc.current_atom.map(|a| a.name.as_str()).unwrap_or("?");
        let measure_text = vc
            .current_atom
            .and_then(|a| a.decreases.as_deref())
            .unwrap_or("?");
        return Err(MumeiError::verification(format!(
            "Termination measure violation: measures of '{atom_name}' and '{}' have incompatible sorts; cannot check decrease of measure '{measure_text}'",
            callee.name
        )));
    };
    if let Some(solver) = solver_opt {
        // Check immediately — before the callee requires check and before any
        // ensures assumption is asserted (a contradictory assumption would
        // make a later check vacuous).
        solver.push();
        for cond in vc.path_cond_stack.borrow().iter() {
            solver.assert(cond);
        }
        solver.assert(&obligation.not());
        let check = solver.check();
        let err = match check {
            SatResult::Sat => Some(termination_violation_error(
                vc,
                solver,
                env,
                &callee.name,
                VerdictKind::Rejected,
            )),
            SatResult::Unknown => Some(termination_violation_error(
                vc,
                solver,
                env,
                &callee.name,
                VerdictKind::Inconclusive,
            )),
            SatResult::Unsat => None,
        };
        solver.pop(1);
        if let Some(err) = err {
            return Err(err);
        }
    } else {
        let ante = vc.path_cond_conj();
        vc.recursion
            .pending_termination
            .borrow_mut()
            .push((ante.implies(&obligation), callee.name.clone()));
    }
    Ok(())
}

/// `ante => ens` antecedent for a congruent recursive call's ensures
/// assumption: the callee's `CallerRequires` (or `true`), conjoined with the
/// path conditions for SCC-internal calls (the induction hypothesis only
/// holds where the decrease was proved).
fn congruent_ensures_antecedent<'a>(
    vc: &VCtx<'a>,
    callee: &Atom,
    caller_requires: &str,
    call_env: &mut Env<'a>,
    scc_internal: bool,
) -> MumeiResult<Bool<'a>> {
    let ctx = vc.ctx;
    let req = if caller_requires.trim() == "true" {
        Bool::from_bool(ctx, true)
    } else {
        let req_ast = parse_closed_contract_expression(caller_requires, "callee precondition")?;
        super::z3_types::with_array_shared_markers_restored(call_env, |env| {
            expr_to_z3_with_domain_obligations(vc, &req_ast, env, None)
        })?
        .as_bool()
        .unwrap_or_else(|| Bool::from_bool(ctx, true))
    };
    let domain_facts = param_domain_facts(vc, callee, call_env)?;
    let has_domain_facts = domain_facts.simplify().as_bool() != Some(true);
    if scc_internal {
        let mut conds: Vec<Bool<'a>> = vc.path_cond_stack.borrow().clone();
        conds.push(req);
        if has_domain_facts {
            conds.push(domain_facts);
        }
        let refs: Vec<&Bool<'a>> = conds.iter().collect();
        Ok(Bool::and(ctx, &refs))
    } else {
        if has_domain_facts {
            Ok(Bool::and(ctx, &[&req, &domain_facts]))
        } else {
            Ok(req)
        }
    }
}

fn param_domain_facts<'a>(
    vc: &VCtx<'a>,
    callee: &Atom,
    call_env: &Env<'a>,
) -> MumeiResult<Bool<'a>> {
    let mut facts = Vec::new();
    for param in &callee.params {
        let type_name = param.type_name.as_deref().map(str::trim);
        if type_name.and_then(array_element_type_name).is_some() {
            let length = call_env.get(&array_len_key(&param.name)).ok_or_else(|| {
                MumeiError::verification(format!(
                    "missing array length for recursive parameter '{}'",
                    param.name
                ))
            })?;
            let nonnegative = nonneg_constraint(vc.ctx, length).ok_or_else(|| {
                MumeiError::verification(format!(
                    "invalid array length sort for recursive parameter '{}'",
                    param.name
                ))
            })?;
            facts.push(nonnegative);
        } else if type_name
            .map(|t| vc.module_env.resolve_base_type(t) == "u64")
            .unwrap_or(false)
        {
            // `u64` parameters of the callee carry the full unsigned range.
            // A `BV(64)` argument is already total; an `Int` argument needs
            // `0 <= v <= u64::MAX` so `v >= 0` is decided the way the
            // unsigned semantics read it.
            if let Some(v) = call_env.get(&param.name).and_then(|d| d.as_int()) {
                let zero = Int::from_i64(vc.ctx, 0);
                let max = Int::from_str(vc.ctx, "18446744073709551615").expect("u64::MAX literal");
                facts.push(Bool::and(vc.ctx, &[&v.ge(&zero), &v.le(&max)]));
            }
        } else if let Some(sdef) = type_name.and_then(|name| vc.module_env.get_struct(name)) {
            let fields: Vec<(String, Dynamic<'a>)> = sdef
                .fields
                .iter()
                .map(|field| {
                    call_env
                        .get(&struct_field_key(&param.name, &field.name))
                        .cloned()
                        .map(|value| (field.name.clone(), value))
                        .ok_or_else(|| {
                            MumeiError::verification(format!(
                                "missing field '{}' for recursive struct parameter '{}'",
                                field.name, param.name
                            ))
                        })
                })
                .collect::<MumeiResult<_>>()?;
            lower_struct_contract(vc, sdef, &fields, call_env, None, |_, fact| {
                facts.push(fact);
                Ok(())
            })?;
        }
    }
    if facts.is_empty() {
        return Ok(Bool::from_bool(vc.ctx, true));
    }
    let refs: Vec<&Bool<'a>> = facts.iter().collect();
    Ok(Bool::and(vc.ctx, &refs))
}

/// Discharge the shift-range obligations collected while lowering clauses
/// without a solver (`requires`, `ensures`, invariants). Called once the
/// atom's solver holds the preconditions, so a shift amount that a `requires`
/// bounds is accepted while an unbounded one is rejected.
pub(crate) fn discharge_bv_shift_obligations<'a>(
    vc: &VCtx<'a>,
    solver: &Solver<'a>,
) -> MumeiResult<()> {
    match shift_range_status(vc, solver) {
        ShiftRangeStatus::Bounded => Ok(()),
        ShiftRangeStatus::OutOfRange => Err(out_of_range_shift_error()),
        ShiftRangeStatus::Undecided => Err(undecided_shift_error()),
    }
}

/// Outcome of checking the collected shift amounts against `0..64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShiftRangeStatus {
    /// Every amount is provably in range under the facts the solver holds.
    Bounded,
    /// An amount can leave the range on a reachable path.
    OutOfRange,
    /// The solver could not decide the range, so it is not guaranteed.
    Undecided,
}

/// Check the collected shift amounts against `0..64` under the facts the
/// solver already holds. Drains the obligations, like
/// `discharge_bv_shift_obligations`, for callers whose failures are not a
/// `MumeiError` (spec validation reports a `SpecContradiction`).
///
/// An `unknown` answer is not a guarantee: the shift range is a safety
/// condition, so it is reported as undecided rather than folded into the
/// in-range case.
pub(crate) fn shift_range_status<'a>(vc: &VCtx<'a>, solver: &Solver<'a>) -> ShiftRangeStatus {
    let obligations: Vec<(Bool<'a>, BV<'a>)> =
        vc.bv_shift_obligations.borrow_mut().drain(..).collect();
    let mut status = ShiftRangeStatus::Bounded;
    for (path_cond, amount) in obligations {
        solver.push();
        solver.assert(&path_cond);
        solver.assert(&bv_shift_out_of_range(vc.ctx, &amount));
        let result = crate::verification::phase_artifacts::check(solver);
        solver.pop(1);
        match result {
            SatResult::Sat => return ShiftRangeStatus::OutOfRange,
            SatResult::Unknown => status = ShiftRangeStatus::Undecided,
            SatResult::Unsat => {}
        }
    }
    status
}

/// Outcome of checking deferred `(dividend, divisor)` pairs for division
/// safety (`divisor != 0` and `!(i64::MIN / -1)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DivSafetyStatus {
    /// Every deferred division is provably safe under the solver's facts.
    Safe,
    /// A reachable path allows a zero divisor.
    DivisionByZero,
    /// A reachable path allows `i64::MIN / -1` (UB in the emitted `sdiv`).
    Overflow,
    /// The solver could not decide safety, so it is not guaranteed.
    Undecided,
}

/// Check deferred division obligations under the facts the solver already
/// holds, mirroring `shift_range_status`. An `unknown` answer is not a
/// guarantee: reported as `Undecided` rather than folded into `Safe`.
pub(crate) fn div_safety_status<'a>(vc: &VCtx<'a>, solver: &Solver<'a>) -> DivSafetyStatus {
    let obligations: Vec<(Bool<'a>, BV<'a>, BV<'a>, bool)> =
        vc.bv_div_obligations.borrow_mut().drain(..).collect();
    let ctx = vc.ctx;
    let zero = BV::from_i64(ctx, 0, I64_BITS);
    let min = BV::from_i64(ctx, i64::MIN, I64_BITS);
    let minus_one = BV::from_i64(ctx, -1, I64_BITS);
    let zero_divisors: Vec<(Bool<'a>, Bool<'a>)> =
        vc.div_zero_obligations.borrow_mut().drain(..).collect();
    let mut status = DivSafetyStatus::Safe;
    for (path_cond, divisor_is_zero) in zero_divisors {
        solver.push();
        solver.assert(&path_cond);
        solver.assert(&divisor_is_zero);
        let divides_by_zero = crate::verification::phase_artifacts::check(solver);
        solver.pop(1);
        match divides_by_zero {
            SatResult::Sat => return DivSafetyStatus::DivisionByZero,
            SatResult::Unknown => status = DivSafetyStatus::Undecided,
            SatResult::Unsat => {}
        }
    }
    for (path_cond, lb, rb, unsigned) in obligations {
        solver.push();
        solver.assert(&path_cond);
        solver.assert(&rb._eq(&zero));
        let divides_by_zero = crate::verification::phase_artifacts::check(solver);
        solver.pop(1);
        match divides_by_zero {
            SatResult::Sat => return DivSafetyStatus::DivisionByZero,
            SatResult::Unknown => status = DivSafetyStatus::Undecided,
            SatResult::Unsat => {}
        }
        // `udiv`/`urem` overflow nowhere: the `i64::MIN / -1` pair is an
        // ordinary result under unsigned division.
        if unsigned {
            continue;
        }
        solver.push();
        solver.assert(&path_cond);
        solver.assert(&Bool::and(ctx, &[&lb._eq(&min), &rb._eq(&minus_one)]));
        let overflows = crate::verification::phase_artifacts::check(solver);
        solver.pop(1);
        match overflows {
            SatResult::Sat => return DivSafetyStatus::Overflow,
            SatResult::Unknown => status = DivSafetyStatus::Undecided,
            SatResult::Unsat => {}
        }
    }
    status
}

/// Discharge the deferred division-safety obligations recorded while
/// lowering contract clauses without a solver.
/// Lengths of the deferred shift, bit-vector division and zero-divisor queues.
pub(crate) type ObligationMarks = (usize, usize, usize, usize);

/// Snapshot of deferred shift/division obligation queue lengths, used by
/// [`rebind_deferred_obligations`] to scope a rebind to one lowering.
pub(crate) fn obligation_marks(vc: &VCtx<'_>) -> ObligationMarks {
    (
        vc.bv_shift_obligations.borrow().len(),
        vc.bv_div_obligations.borrow().len(),
        vc.div_zero_obligations.borrow().len(),
        vc.i64_overflow_obligations.borrow().len(),
    )
}

/// True when at least one deferred obligation was recorded since `marks`.
/// Drop the deferred obligations queued since `marks`, for a clause that was
/// lowered and then skipped: its divisions and shifts are never evaluated.
pub(crate) fn discard_obligations_since(vc: &VCtx<'_>, marks: ObligationMarks) {
    vc.bv_shift_obligations.borrow_mut().truncate(marks.0);
    vc.bv_div_obligations.borrow_mut().truncate(marks.1);
    vc.div_zero_obligations.borrow_mut().truncate(marks.2);
    vc.i64_overflow_obligations.borrow_mut().truncate(marks.3);
}

pub(crate) fn has_new_obligations(vc: &VCtx<'_>, marks: ObligationMarks) -> bool {
    vc.bv_shift_obligations.borrow().len() > marks.0
        || vc.bv_div_obligations.borrow().len() > marks.1
        || vc.div_zero_obligations.borrow().len() > marks.2
        || vc.i64_overflow_obligations.borrow().len() > marks.3
}

/// Conjoin `extra` into the path condition of deferred shift/division
/// obligations recorded since `marks`. Expressions like a loop invariant or
/// an `ensures` clause are only ever evaluated in states where that clause
/// itself holds, so their deferred checks may assume it — otherwise
/// `r <= 62 && (n >> r) == v` could never discharge `n >> r`'s range
/// obligation, since `r`'s bound lives inside the same clause.
pub(crate) fn rebind_deferred_obligations<'a>(
    vc: &VCtx<'a>,
    marks: ObligationMarks,
    extra: &Bool<'a>,
) {
    for (pc, _) in vc.bv_shift_obligations.borrow_mut()[marks.0..].iter_mut() {
        *pc = Bool::and(vc.ctx, &[pc, extra]);
    }
    for (pc, _, _, _) in vc.bv_div_obligations.borrow_mut()[marks.1..].iter_mut() {
        *pc = Bool::and(vc.ctx, &[pc, extra]);
    }
    for (pc, _) in vc.div_zero_obligations.borrow_mut()[marks.2..].iter_mut() {
        *pc = Bool::and(vc.ctx, &[pc, extra]);
    }
    for (pc, _, _) in vc.i64_overflow_obligations.borrow_mut()[marks.3..].iter_mut() {
        *pc = Bool::and(vc.ctx, &[pc, extra]);
    }
}

pub(crate) fn discharge_bv_div_obligations<'a>(
    vc: &VCtx<'a>,
    solver: &Solver<'a>,
) -> MumeiResult<()> {
    match div_safety_status(vc, solver) {
        DivSafetyStatus::Safe => Ok(()),
        DivSafetyStatus::DivisionByZero => Err(MumeiError::verification(
            "Potential division by zero.".to_string(),
        )
        .with_help("Add a condition divisor != 0 to requires")
        .with_verdict(crate::verification::VerdictKind::Rejected)),
        DivSafetyStatus::Overflow => Err(MumeiError::verification(
            "Division may overflow (i64::MIN / -1).".to_string(),
        )
        .with_help(
            "Add a condition ruling out the pair, e.g. `divisor != 0 - 1` \
             or `dividend != 0 - 9223372036854775807 - 1`, to requires",
        )
        .with_verdict(crate::verification::VerdictKind::Rejected)),
        DivSafetyStatus::Undecided => Err(MumeiError::verification(
            "Z3 could not decide division safety (divisor != 0, no overflow).".to_string(),
        )
        .with_help("Strengthen requires so the divisor is provably nonzero")
        .with_verdict(crate::verification::VerdictKind::Inconclusive)),
    }
}

/// Name and range of a fixed-width integer type, or `None` for other types.
pub(crate) fn fixed_width_int(type_name: &str) -> Option<(&'static str, i128, i128)> {
    Some(match type_name {
        "i8" => ("i8", i8::MIN.into(), i8::MAX.into()),
        "i16" => ("i16", i16::MIN.into(), i16::MAX.into()),
        "i32" => ("i32", i32::MIN.into(), i32::MAX.into()),
        "i64" => ("i64", i64::MIN.into(), i64::MAX.into()),
        "u8" => ("u8", 0, u8::MAX.into()),
        "u16" => ("u16", 0, u16::MAX.into()),
        "u32" => ("u32", 0, u32::MAX.into()),
        "u64" => ("u64", 0, u64::MAX.into()),
        _ => return None,
    })
}

/// The declared fixed-width integer type of a name bound inside an atom's
/// contract vocabulary: `result` is the return value, every other name is
/// a parameter. `None` for anything else.
fn contract_name_int_type(
    vc: &VCtx,
    atom: &crate::parser::Atom,
    name: &str,
) -> Option<&'static str> {
    if name == "result" {
        let base = vc
            .module_env
            .resolve_base_type(atom.return_type.as_deref()?);
        return fixed_width_int(&base).map(|(ty, _, _)| ty);
    }
    let param = atom.params.iter().find(|p| p.name == name)?;
    let base = vc.module_env.resolve_base_type(param.type_name.as_deref()?);
    fixed_width_int(&base).map(|(ty, _, _)| ty)
}

/// The declared struct type a body expression evaluates to (`s` in `s.f`,
/// `s.point` in `s.point.x`), resolved through local and parameter
/// declarations — including the callee's signature while a contract is
/// instantiated. `None` for non-struct values.
fn expr_struct_type(vc: &VCtx, expr: &Expr) -> Option<String> {
    match expr {
        Expr::Variable(name) => {
            let instantiated = vc.contracts_in_instantiation.borrow().last().cloned();
            let atom = instantiated
                .as_deref()
                .and_then(|n| vc.module_env.get_atom(n))
                .or(vc.current_atom)?;
            // While a callee contract is instantiated its parameter names
            // bind before any same-named caller local (same rule as
            // `fixed_width_int_type`'s Variable arm).
            if instantiated.is_some() {
                if let Some(base) = atom
                    .params
                    .iter()
                    .find(|p| p.name == *name)
                    .and_then(|p| p.type_name.as_deref())
                    .map(|n| vc.module_env.resolve_base_type(n))
                {
                    if vc.module_env.get_struct(&base).is_some() {
                        return Some(base);
                    }
                }
            }
            if let Some(ty) = vc.local_struct_types.borrow().get(name) {
                return Some(ty.clone());
            }
            let param = atom.params.iter().find(|p| p.name == *name)?;
            let base = vc.module_env.resolve_base_type(param.type_name.as_deref()?);
            vc.module_env.get_struct(&base).map(|_| base)
        }
        Expr::FieldAccess(inner, field) => {
            let inner_ty = expr_struct_type(vc, inner)?;
            let sdef = vc.module_env.get_struct(&inner_ty)?;
            let f = sdef.fields.iter().find(|f| f.name == *field)?;
            let base = vc.module_env.resolve_base_type(f.type_name.trim());
            vc.module_env.get_struct(&base).map(|_| base)
        }
        Expr::ArrayAccess(name, _) => {
            let element_type = super::z3_types::array_element_type_name(name, vc);
            let resolved = vc.module_env.resolve_base_type(&element_type);
            vc.module_env
                .get_struct(datatype::type_name_base(&resolved))
                .map(|_| resolved)
        }
        _ => None,
    }
}

/// The declared fixed-width integer type of a body expression, taken from
/// the parameter or local it reads. `None` when nothing declares one
/// (literals, calls, lengths), which the overflow check treats as `i64`.
pub(crate) fn fixed_width_int_type(vc: &VCtx, expr: &Expr) -> Option<&'static str> {
    fixed_width_int_type_in_scope(
        vc,
        expr,
        &std::collections::HashMap::new(),
        &std::collections::HashMap::new(),
    )
}

fn fixed_width_int_type_in_scope(
    vc: &VCtx,
    expr: &Expr,
    local_types: &std::collections::HashMap<String, Option<&'static str>>,
    local_struct_types: &std::collections::HashMap<String, String>,
) -> Option<&'static str> {
    match expr {
        Expr::Variable(name) => {
            if let Some(ty) = local_types.get(name) {
                return *ty;
            }
            if let Some((_, ty)) = vc
                .refined_operand_types
                .borrow()
                .iter()
                .rev()
                .find(|(operand, _)| operand == name)
            {
                return Some(*ty);
            }
            // Inside an instantiated callee contract (`ensures`/`requires`
            // evaluated in `call_env`) names bind to the callee's signature,
            // not the caller's: `result` is the callee's return value and a
            // clause variable is a callee parameter — even when the caller
            // happens to hold a same-named local of a different type. The
            // innermost instantiation names that atom.
            let instantiated = vc.contracts_in_instantiation.borrow().last().cloned();
            let atom = instantiated
                .as_deref()
                .and_then(|name| vc.module_env.get_atom(name))
                .or(vc.current_atom)?;
            if instantiated.is_some() {
                if let Some(ty) = contract_name_int_type(vc, atom, name) {
                    return Some(ty);
                }
            }
            if let Some(ty) = vc.local_int_types.borrow().get(name) {
                return Some(ty);
            }
            contract_name_int_type(vc, atom, name)
        }
        Expr::Call(name, _) => {
            // A call's declared return type fixes its signedness: `f()`
            // returning `i64` in a `u64` operand context must still be
            // caught as a mixed pair rather than adapting.
            let atom = vc.module_env.get_atom(name)?;
            let base = vc
                .module_env
                .resolve_base_type(atom.return_type.as_deref()?);
            fixed_width_int(&base).map(|(ty, _, _)| ty)
        }
        Expr::BinaryOp(
            l,
            Op::Add
            | Op::Sub
            | Op::Mul
            | Op::Div
            | Op::Rem
            | Op::BitAnd
            | Op::BitOr
            | Op::BitXor
            | Op::Shl
            | Op::Shr,
            r,
        ) => fixed_width_int_type_in_scope(vc, l, local_types, local_struct_types)
            .or_else(|| fixed_width_int_type_in_scope(vc, r, local_types, local_struct_types)),
        Expr::IfThenElse {
            then_branch,
            else_branch,
            ..
        } => stmt_tail_int_type(vc, then_branch, local_types, local_struct_types)
            .or_else(|| stmt_tail_int_type(vc, else_branch, local_types, local_struct_types)),
        Expr::Block(stmt) => stmt_tail_int_type(vc, stmt, local_types, local_struct_types),
        Expr::FieldAccess(inner, field) => {
            // Preserve the declared width through nested structs and named
            // enum payloads, not only fields whose receiver is a struct.
            let scoped_declared = match inner.as_ref() {
                Expr::Variable(name) => local_struct_types.get(name).and_then(|type_name| {
                    let resolved = vc.module_env.resolve_base_type(type_name);
                    let sdef = vc.module_env.get_struct(&resolved).or_else(|| {
                        vc.module_env
                            .get_struct(datatype::type_name_base(&resolved))
                    })?;
                    sdef.fields
                        .iter()
                        .find(|candidate| candidate.name == *field)
                        .map(|candidate| candidate.type_name.clone())
                }),
                _ => None,
            };
            let declared = scoped_declared
                .or_else(|| datatype::declared_type_of_expr(vc, expr))
                .or_else(|| {
                    let ty = expr_struct_type(vc, inner)?;
                    let sdef = vc.module_env.get_struct(&ty)?;
                    sdef.fields
                        .iter()
                        .find(|candidate| candidate.name == *field)
                        .map(|candidate| candidate.type_name.clone())
                })?;
            let base = vc.module_env.resolve_base_type(&declared);
            fixed_width_int(&base).map(|(ty, _, _)| ty)
        }
        _ => None,
    }
}

fn stmt_tail_int_type(
    vc: &VCtx,
    stmt: &Stmt,
    local_types: &std::collections::HashMap<String, Option<&'static str>>,
    local_struct_types: &std::collections::HashMap<String, String>,
) -> Option<&'static str> {
    match stmt {
        Stmt::Expr(e, _) => fixed_width_int_type_in_scope(vc, e, local_types, local_struct_types),
        Stmt::Block(stmts, _) => {
            let (tail, preceding) = stmts.split_last()?;
            let mut scoped_types = local_types.clone();
            let mut scoped_struct_types = local_struct_types.clone();
            for stmt in preceding {
                if let Stmt::Let { var, value, .. } = stmt {
                    let ty = fixed_width_int_type_in_scope(
                        vc,
                        value,
                        &scoped_types,
                        &scoped_struct_types,
                    );
                    scoped_types.insert(var.clone(), ty);
                    let struct_type = match value.as_ref() {
                        Expr::StructInit { type_name, .. } => Some(type_name.clone()),
                        _ => expr_struct_type(vc, value),
                    };
                    if let Some(struct_type) = struct_type {
                        scoped_struct_types.insert(var.clone(), struct_type);
                    } else {
                        scoped_struct_types.remove(var);
                    }
                }
            }
            stmt_tail_int_type(vc, tail, &scoped_types, &scoped_struct_types)
        }
        _ => None,
    }
}

/// `--check-i64-overflow`: the operands of a body `+`, `-` or `*` fit in
/// their integer type `ty` but the unbounded result does not. The compiled
/// code wraps (or panics in a checked build) there, so a proof in the `Int`
/// encoding would not describe it.
fn i64_overflow_condition<'a>(
    vc: &VCtx<'a>,
    left: &Int<'a>,
    right: &Int<'a>,
    result: &Int<'a>,
    ty: &str,
) -> Bool<'a> {
    let ctx = vc.ctx;
    let (_, lo, hi) = fixed_width_int(ty).unwrap_or(("i64", i64::MIN.into(), i64::MAX.into()));
    let min = Int::from_str(ctx, &lo.to_string()).expect("integer literal");
    let max = Int::from_str(ctx, &hi.to_string()).expect("integer literal");
    let in_range = |v: &Int<'a>| Bool::and(ctx, &[&v.ge(&min), &v.le(&max)]);
    Bool::and(
        ctx,
        &[&in_range(left), &in_range(right), &in_range(result).not()],
    )
}

fn i64_overflow_error(outcome: SatResult, ty: &str) -> MumeiResult<()> {
    match outcome {
        SatResult::Unsat => Ok(()),
        SatResult::Sat => Err(MumeiError::verification(format!(
            "Integer arithmetic may overflow {ty}."
        ))
        .with_help(format!(
            "Bound the operands in requires so the result fits in {ty}"
        ))
        .with_verdict(crate::verification::VerdictKind::Rejected)),
        SatResult::Unknown => Err(MumeiError::verification(format!(
            "Z3 could not decide whether integer arithmetic stays within {ty}."
        ))
        .with_help(format!(
            "Bound the operands in requires so the result fits in {ty}"
        ))
        .with_verdict(crate::verification::VerdictKind::Inconclusive)),
    }
}

fn check_i64_range<'a>(
    vc: &VCtx<'a>,
    solver: &Solver<'a>,
    overflows: &Bool<'a>,
    ty: &str,
) -> MumeiResult<()> {
    solver.push();
    for cond in vc.path_cond_stack.borrow().iter() {
        solver.assert(cond);
    }
    solver.assert(overflows);
    let outcome = crate::verification::phase_artifacts::check(solver);
    solver.pop(1);
    i64_overflow_error(outcome, ty)
}

/// Check the `--check-i64-overflow` obligations recorded while lowering loop
/// conditions and match guards without a solver, each under its path
/// condition.
pub(crate) fn discharge_i64_overflow_obligations<'a>(
    vc: &VCtx<'a>,
    solver: &Solver<'a>,
) -> MumeiResult<()> {
    discharge_i64_overflow_obligations_from(vc, solver, 0)
}

/// Check the obligations recorded from index `start` on, and drop them.
fn discharge_i64_overflow_obligations_from<'a>(
    vc: &VCtx<'a>,
    solver: &Solver<'a>,
    start: usize,
) -> MumeiResult<()> {
    let obligations: Vec<(Bool<'a>, Bool<'a>, &'static str)> = vc
        .i64_overflow_obligations
        .borrow_mut()
        .drain(start..)
        .collect();
    let mut undecided = None;
    for (path_cond, overflows, ty) in obligations {
        solver.push();
        solver.assert(&path_cond);
        solver.assert(&overflows);
        let outcome = crate::verification::phase_artifacts::check(solver);
        solver.pop(1);
        match outcome {
            SatResult::Sat => return i64_overflow_error(outcome, ty),
            SatResult::Unknown => undecided = undecided.or(Some(ty)),
            SatResult::Unsat => {}
        }
    }
    match undecided {
        Some(ty) => i64_overflow_error(SatResult::Unknown, ty),
        None => Ok(()),
    }
}

fn bv_binary_op<'a>(
    vc: &VCtx<'a>,
    op: &Op,
    l: &Dynamic<'a>,
    r: &Dynamic<'a>,
    solver_opt: Option<&Solver<'a>>,
    unsigned: bool,
    ty: &'static str,
) -> MumeiResult<Dynamic<'a>> {
    let ctx = vc.ctx;
    let lb = as_bv_i64(l).ok_or_else(|| {
        MumeiError::type_error("Expected an i64 (BV(64)) operand under --bitvec-i64")
    })?;
    let rb = as_bv_i64(r).ok_or_else(|| {
        MumeiError::type_error("Expected an i64 (BV(64)) operand under --bitvec-i64")
    })?;

    if matches!(op, Op::Shl | Op::Shr) {
        match solver_opt {
            Some(solver) => {
                solver.push();
                for cond in vc.path_cond_stack.borrow().iter() {
                    solver.assert(cond);
                }
                solver.assert(&bv_shift_out_of_range(ctx, &rb));
                let result = crate::verification::phase_artifacts::check(solver);
                solver.pop(1);
                match result {
                    SatResult::Sat => return Err(out_of_range_shift_error()),
                    SatResult::Unknown => return Err(undecided_shift_error()),
                    SatResult::Unsat => {}
                }
            }
            // Contract clauses and invariants are lowered without a solver;
            // remember the amount so the range is still discharged once the
            // atom's solver carries the preconditions.
            None => vc
                .bv_shift_obligations
                .borrow_mut()
                .push((vc.path_cond_conj(), rb.clone())),
        }
    }

    // `--check-i64-overflow` under the BV encoding: `bvadd`/`bvsub`/`bvmul`
    // wrap by definition, so "may overflow" asks whether the machine-word
    // result can differ from the mathematical one.
    if matches!(op, Op::Add | Op::Sub | Op::Mul)
        && vc.check_i64_overflow.get()
        && vc.recursion.lambda_bind_depth.get() == 0
        && vc.contracts_in_instantiation.borrow().is_empty()
    {
        let no_wrap = match op {
            Op::Add => lb.bvadd_no_overflow(&rb, !unsigned),
            // Unsigned subtraction wraps downward (underflow); the signed
            // no-overflow predicate describes the i64 range.
            Op::Sub if unsigned => lb.bvsub_no_underflow(&rb, false),
            Op::Sub => lb.bvsub_no_overflow(&rb),
            _ => lb.bvmul_no_overflow(&rb, !unsigned),
        };
        let overflows = no_wrap.not();
        match solver_opt {
            Some(solver) => check_i64_range(vc, solver, &overflows, ty)?,
            None => {
                vc.i64_overflow_obligations
                    .borrow_mut()
                    .push((vc.path_cond_conj(), overflows, ty))
            }
        }
    }

    match op {
        Op::BitAnd => Ok(lb.bvand(&rb).into()),
        Op::BitOr => Ok(lb.bvor(&rb).into()),
        Op::BitXor => Ok(lb.bvxor(&rb).into()),
        Op::Shl => Ok(lb.bvshl(&rb).into()),
        // `>>` on a `u64` operand is the logical shift `lshr`; the
        // sign-propagating `ashr` only applies to signed operands.
        Op::Shr if unsigned => Ok(lb.bvlshr(&rb).into()),
        Op::Shr => Ok(lb.bvashr(&rb).into()),
        Op::Add => Ok(lb.bvadd(&rb).into()),
        Op::Sub => Ok(lb.bvsub(&rb).into()),
        Op::Mul => Ok(lb.bvmul(&rb).into()),
        Op::Div | Op::Rem => {
            if let Some(solver) = solver_opt {
                let zero = BV::from_i64(ctx, 0, I64_BITS);
                solver.push();
                for cond in vc.path_cond_stack.borrow().iter() {
                    solver.assert(cond);
                }
                solver.assert(&rb._eq(&zero));
                let divides_by_zero =
                    crate::verification::phase_artifacts::check(solver) == SatResult::Sat;
                solver.pop(1);
                if divides_by_zero {
                    return Err(MumeiError::verification(
                        "Potential division by zero.".to_string(),
                    )
                    .with_help("Add a condition divisor != 0 to requires")
                    .with_verdict(crate::verification::VerdictKind::Rejected));
                }
                // `bvsdiv` gives `i64::MIN / -1` the value `i64::MIN`, while
                // the emitted `sdiv` is undefined there, so a proof about that
                // pair would not describe the executable. `udiv`/`urem`
                // overflow nowhere — unsigned division skips this check.
                if !unsigned {
                    let min = BV::from_i64(ctx, i64::MIN, I64_BITS);
                    let minus_one = BV::from_i64(ctx, -1, I64_BITS);
                    solver.push();
                    for cond in vc.path_cond_stack.borrow().iter() {
                        solver.assert(cond);
                    }
                    solver.assert(&Bool::and(ctx, &[&lb._eq(&min), &rb._eq(&minus_one)]));
                    let overflows =
                        crate::verification::phase_artifacts::check(solver) == SatResult::Sat;
                    solver.pop(1);
                    if overflows {
                        return Err(MumeiError::verification(
                            "Division may overflow (i64::MIN / -1).".to_string(),
                        )
                        .with_help(
                            "Add a condition ruling out the pair, e.g. `divisor != 0 - 1` \
                             or `dividend != 0 - 9223372036854775807 - 1`, to requires",
                        )
                        .with_verdict(crate::verification::VerdictKind::Rejected));
                    }
                }
            } else {
                // Contract clauses are lowered without a solver; defer the
                // division-safety checks like the shift-range obligations
                // above so `divisor != 0` and the MIN/-1 pair are still
                // discharged under the atom's preconditions.
                vc.bv_div_obligations.borrow_mut().push((
                    vc.path_cond_conj(),
                    lb.clone(),
                    rb.clone(),
                    unsigned,
                ));
            }
            if unsigned {
                // `bvudiv`/`bvurem` like the emitted `udiv`/`urem`: the
                // only domain bound is a nonzero divisor — `i64::MIN / -1`
                // is an ordinary result unsigned, so that check is skipped.
                if matches!(op, Op::Rem) {
                    Ok(lb.bvurem(&rb).into())
                } else {
                    Ok(lb.bvudiv(&rb).into())
                }
            } else if matches!(op, Op::Rem) {
                // `bvsrem` truncates and takes the dividend's sign, like LLVM `srem`.
                Ok(lb.bvsrem(&rb).into())
            } else {
                Ok(lb.bvsdiv(&rb).into())
            }
        }
        Op::Gt if unsigned => Ok(lb.bvugt(&rb).into()),
        Op::Lt if unsigned => Ok(lb.bvult(&rb).into()),
        Op::Ge if unsigned => Ok(lb.bvuge(&rb).into()),
        Op::Le if unsigned => Ok(lb.bvule(&rb).into()),
        Op::Gt => Ok(lb.bvsgt(&rb).into()),
        Op::Lt => Ok(lb.bvslt(&rb).into()),
        Op::Ge => Ok(lb.bvsge(&rb).into()),
        Op::Le => Ok(lb.bvsle(&rb).into()),
        Op::Eq => Ok(lb._eq(&rb).into()),
        Op::Neq => Ok(lb._eq(&rb).not().into()),
        Op::Pow | Op::And | Op::Or | Op::Implies => Err(MumeiError::verification(format!(
            "Unsupported bit-vector operator {:?}",
            op
        ))),
    }
}

// Collect `let`-bound names anywhere inside a match arm body — including
// nested blocks, if/match branches, and loop/task bodies whose envs fold
// back into the arm env. The post-match merge treats them as arm-local
// shadows, so an arm-body `let x = …` never rewrites an outer `x`.
fn collect_arm_local_lets(stmt: &Stmt, local: &mut std::collections::HashSet<String>) {
    match stmt {
        Stmt::Let { var, value, .. } => {
            local.insert(var.clone());
            collect_expr_local_lets(value, local);
        }
        Stmt::Assign { value, .. } => collect_expr_local_lets(value, local),
        Stmt::Expr(e, _) => collect_expr_local_lets(e, local),
        Stmt::ArrayStore { index, value, .. } => {
            collect_expr_local_lets(index, local);
            collect_expr_local_lets(value, local);
        }
        Stmt::Block(stmts, _) => {
            for s in stmts {
                collect_arm_local_lets(s, local);
            }
        }
        Stmt::While {
            cond,
            invariant,
            decreases,
            ..
        } => {
            // The loop body runs on a havoced env clone — its `let`s
            // never reach the enclosing env.
            collect_expr_local_lets(cond, local);
            collect_expr_local_lets(invariant, local);
            if let Some(d) = decreases {
                collect_expr_local_lets(d, local);
            }
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
            collect_arm_local_lets(body, local);
        }
        Stmt::TaskGroup { children, .. } => {
            for c in children {
                collect_arm_local_lets(c, local);
            }
        }
        Stmt::Cancel { .. } => {}
    }
}

fn collect_expr_local_lets(expr: &Expr, local: &mut std::collections::HashSet<String>) {
    match expr {
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_expr_local_lets(cond, local);
            collect_arm_local_lets(then_branch, local);
            collect_arm_local_lets(else_branch, local);
        }
        Expr::Block(stmt) => collect_arm_local_lets(stmt, local),
        Expr::Match { target, arms } => {
            // Inner match arms have their own local set via the inner
            // merge — their `let`s never reach this arm's env, and
            // collecting them here would wrongly skip real assignments
            // this arm makes to a same-named outer variable.
            collect_expr_local_lets(target, local);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    collect_expr_local_lets(g, local);
                }
            }
        }
        Expr::Async { body } => collect_arm_local_lets(body, local),
        // Lambda bodies run in their own parameter scope — their `let`s
        // never reach the enclosing env.
        Expr::BinaryOp(l, _, r) => {
            collect_expr_local_lets(l, local);
            collect_expr_local_lets(r, local);
        }
        Expr::FieldAccess(base, _) | Expr::Await { expr: base } => {
            collect_expr_local_lets(base, local)
        }
        Expr::ArrayAccess(_, idx) => collect_expr_local_lets(idx, local),
        Expr::Call(_, args) | Expr::Perform { args, .. } => {
            for a in args {
                collect_expr_local_lets(a, local);
            }
        }
        Expr::CallRef { callee, args } => {
            collect_expr_local_lets(callee, local);
            for a in args {
                collect_expr_local_lets(a, local);
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, v) in fields {
                collect_expr_local_lets(v, local);
            }
        }
        Expr::ChanSend { channel, value } => {
            collect_expr_local_lets(channel, local);
            collect_expr_local_lets(value, local);
        }
        Expr::ChanRecv { channel } => collect_expr_local_lets(channel, local),
        _ => {}
    }
}

fn record_cover_obligation<'a>(vc: &VCtx<'a>, obligation: Bool<'a>) {
    let Some(obligations) = vc.cover_obligations.as_ref() else {
        return;
    };
    let path_conditions = vc.path_cond_stack.borrow();
    let obligation = if path_conditions.is_empty() {
        obligation
    } else {
        let path_condition = Bool::and(vc.ctx, &path_conditions.iter().collect::<Vec<_>>());
        path_condition.implies(&obligation)
    };
    obligations.borrow_mut().push(obligation);
}

#[inline(never)]
pub(crate) fn expr_to_z3_with_domain_obligations<'a>(
    vc: &VCtx<'a>,
    expr: &Expr,
    env: &mut Env<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> MumeiResult<Dynamic<'a>> {
    vc.begin_spec_domain_clause();
    let lowered = expr_to_z3(vc, expr, env, solver_opt);
    let obligations = vc.finish_spec_domain_clause();
    let value = lowered?;
    if obligations.is_empty() {
        return Ok(value);
    }
    let Some(clause) = value.as_bool() else {
        return Err(MumeiError::verification(
            "argument-domain obligations require a boolean specification clause",
        )
        .with_verdict(crate::verification::VerdictKind::Rejected));
    };
    let mut conjuncts = Vec::with_capacity(obligations.len() + 1);
    conjuncts.push(clause);
    conjuncts.extend(obligations);
    let conjuncts = conjuncts.iter().collect::<Vec<_>>();
    Ok(Bool::and(vc.ctx, &conjuncts).into())
}

#[inline(never)]
fn check_call_argument_domain<'a>(
    vc: &VCtx<'a>,
    solver_opt: Option<&Solver<'a>>,
    callee_name: &str,
    param_name: &str,
    type_name: &str,
    value: &Dynamic<'a>,
    call_env: &mut Env<'a>,
) -> MumeiResult<()> {
    let resolved_type = vc.module_env.resolve_base_type(type_name);
    let mut domains = Vec::new();

    if let Some(domain) = datatype::scalar_int_leaf_domain(vc, &resolved_type, value) {
        domains.push((resolved_type.clone(), domain));
    } else if let Some(element_type) = array_element_type_name(&resolved_type) {
        if let Some(leaves) = super::z3_types::struct_array_leaf_types(element_type, vc.module_env)
        {
            if let Some(length) = call_env.get(&array_len_key(param_name)) {
                for (path, leaf_type) in leaves {
                    let key = super::z3_types::struct_array_leaf_key(param_name, &path);
                    let Some(array) = call_env.get(&key).and_then(|value| value.as_array()) else {
                        continue;
                    };
                    let array: Dynamic = array.into();
                    let domain = if leaf_type.starts_with("enum:") {
                        datatype::enum_array_leaf_domain(vc, &array, length)
                    } else {
                        datatype::array_int_leaf_domain(vc, &array, length, &leaf_type)
                    };
                    if let Some(domain) = domain {
                        let domain_type = leaf_type
                            .strip_prefix("enum:")
                            .unwrap_or(&leaf_type)
                            .to_string();
                        domains.push((domain_type, domain));
                    }
                }
            }
        } else if let Some(length) = call_env.get(&array_len_key(param_name)) {
            if let Some(domain) = datatype::array_int_leaf_domain(vc, value, length, element_type) {
                domains.push((element_type.to_string(), domain));
            }
        } else if matches!(
            vc.module_env.resolve_base_type(element_type).as_str(),
            "u64" | "u32" | "i32"
        ) {
            return Err(MumeiError::verification(format!(
                "Call to '{callee_name}' argument '{param_name}' may be outside the '{}' domain",
                vc.module_env.resolve_base_type(element_type)
            ))
            .with_verdict(crate::verification::VerdictKind::Rejected));
        }
    } else if let Some(enum_def) = vc.module_env.get_enum(&resolved_type) {
        if let Some(domain) = datatype::enum_value_leaf_domain(vc, value) {
            domains.push((enum_def.name.clone(), domain));
        }
    } else if vc.module_env.get_struct(&resolved_type).is_some() {
        let values = super::struct_invariants::nested_field_bindings(call_env, value);
        for (_, leaf_value) in &values {
            if let Some(domain) = datatype::enum_value_leaf_domain(vc, leaf_value) {
                let domain_type = vc
                    .module_env
                    .enums
                    .values()
                    .find(|enum_def| {
                        datatype::enum_datatype_sort(vc, enum_def)
                            .is_some_and(|sort| sort.sort == leaf_value.get_sort())
                    })
                    .map(|enum_def| enum_def.name.clone())
                    .unwrap_or_else(|| resolved_type.clone());
                domains.push((domain_type, domain));
            }
        }
        if let Some(leaves) = datatype::field_leaf_types(&resolved_type, vc.module_env) {
            for (leaf_name, leaf_type) in leaves {
                if leaf_type == "array_len" {
                    continue;
                }
                let Some(leaf_value) = values
                    .iter()
                    .find(|(name, _)| name == &leaf_name)
                    .map(|(_, value)| value)
                else {
                    let domain_type = leaf_type
                        .strip_prefix("array:")
                        .or_else(|| leaf_type.strip_prefix("enum:"))
                        .unwrap_or(&leaf_type);
                    return Err(MumeiError::verification(format!(
                        "Call to '{callee_name}' argument '{param_name}' may be outside the '{domain_type}' domain"
                    ))
                    .with_verdict(crate::verification::VerdictKind::Rejected));
                };
                let domain = if let Some(element_type) = leaf_type.strip_prefix("array:") {
                    let Some(array) = leaf_value.as_array() else {
                        continue;
                    };
                    let length =
                        super::z3_types::array_len_for_value(vc, call_env, &array, solver_opt);
                    datatype::array_int_leaf_domain(vc, leaf_value, &length, element_type)
                        .map(|domain| (element_type.to_string(), domain))
                } else {
                    datatype::scalar_int_leaf_domain(vc, &leaf_type, leaf_value)
                        .map(|domain| (leaf_type.clone(), domain))
                };
                if let Some(domain) = domain {
                    domains.push(domain);
                }
            }
        }
    }

    for (domain_type, domain) in domains {
        if vc.cover_obligations.is_some() {
            record_cover_obligation(vc, domain);
            continue;
        }
        let Some(solver) = solver_opt else {
            if vc.record_spec_domain_obligation(vc.path_cond_conj().implies(&domain)) {
                continue;
            }
            return Err(MumeiError::verification(format!(
                "Call to '{callee_name}' argument '{param_name}' may be outside the '{domain_type}' domain"
            ))
            .with_verdict(crate::verification::VerdictKind::Rejected));
        };
        solver.push();
        solver.assert(&vc.path_cond_conj());
        solver.assert(&domain.not());
        let result = crate::verification::phase_artifacts::check(solver);
        solver.pop(1);
        if result != SatResult::Unsat {
            return Err(MumeiError::verification(format!(
                "Call to '{callee_name}' argument '{param_name}' may be outside the '{domain_type}' domain"
            ))
            .with_verdict(crate::verification::VerdictKind::Rejected));
        }
    }
    Ok(())
}

#[inline(never)]
fn check_call_argument_domains<'a>(
    vc: &VCtx<'a>,
    solver_opt: Option<&Solver<'a>>,
    callee_name: &str,
    params: &[crate::parser::Param],
    values: &[Dynamic<'a>],
    call_env: &mut Env<'a>,
) -> MumeiResult<()> {
    for (param, value) in params.iter().zip(values) {
        let Some(type_name) = param.type_name.as_deref() else {
            continue;
        };
        check_call_argument_domain(
            vc,
            solver_opt,
            callee_name,
            &param.name,
            type_name,
            value,
            call_env,
        )?;
    }
    Ok(())
}

/// The contents array for one `==`/`!=` operand. A `Variable` resolves
/// through `z3_dynamic_array` so the live `__z3_arr_<name>` chain (which
/// `a[i] = v` stores rewrite) is compared, not the stale base const still
/// in `env[name]`. Returns `None` for operands that did not lower to an
/// array — the caller turns that into a type error.
fn array_eq_operand<'a>(
    vc: &VCtx<'a>,
    env: &Env<'a>,
    operand: &Expr,
    val: &Dynamic<'a>,
    live: Option<Array<'a>>,
) -> Option<Array<'a>> {
    if let Expr::Variable(name) = operand {
        // Only a name that actually lowered to an array counts — a variable
        // rebound to a non-array must still hit the type error even when a
        // stale `__z3_arr_<name>` slot survives from before the rebind.
        return val
            .as_array()
            .map(|_| live.unwrap_or_else(|| z3_dynamic_array(vc, name, env)));
    }
    val.as_array()
}

/// The `len` of one `==`/`!=` operand. Named arrays use their tracked
/// `len_<name>` symbol (asserting `>= 0` for a freshly minted one, like
/// `len(x)`); anything else — literals, call results, `if`/`match`/`block`
/// tails — goes through `tail_len_expr`, which models them structurally.
fn array_eq_operand_len<'a>(
    vc: &VCtx<'a>,
    env: &mut Env<'a>,
    operand: &Expr,
    val: &Dynamic<'a>,
    solver_opt: Option<&Solver<'a>>,
    live_len: Option<Dynamic<'a>>,
) -> Dynamic<'a> {
    if let Expr::Variable(name) = operand {
        if let Some(len) = live_len {
            return len;
        }
        return array_len_value(vc.ctx, env, name, vc.bitvec_i64, solver_opt);
    }
    tail_len_expr(vc, env, None, operand, val, 0, solver_opt)
}

/// `l_sel == r_sel` on array elements. `fp.eq` keeps the IEEE reading for
/// `f64` elements (matching scalar `==`); every other element sort — `Int`,
/// `Bool`, `Real`, `BV`, `Str`, enum datatypes — uses Z3 `_eq`.
fn array_eq_element<'a>(ctx: &'a Context, l: &Dynamic<'a>, r: &Dynamic<'a>) -> Option<Bool<'a>> {
    if let (Some(lf), Some(rf)) = (l.as_float(), r.as_float()) {
        return Some(float_eq(ctx, &lf, &rf));
    }
    (l.get_sort() == r.get_sort()).then(|| l._eq(r))
}

/// `a == b` on `[T]` values.
///
/// An array value is its (contents, len) pair: contents live in the Z3
/// `Array(Int, Elem)` sort while the length is a separate `len_*` symbol in
/// the active `i64` sort (Int, or BV(64) under `--bitvec-i64`). Z3's
/// extensional `=` on the Array sort is *not* the right reading — it says
/// nothing about the separate length, and it also constrains `select` on
/// every out-of-range index, where array literals store over an
/// unconstrained base const and comparison is arbitrary. Two arrays are
/// therefore equal iff `len_a == len_b` and
/// `forall k in [0, len_a): a[k] == b[k]`; `!=` is its negation.
#[allow(clippy::too_many_arguments)]
fn array_value_equality<'a>(
    vc: &VCtx<'a>,
    left: &Expr,
    right: &Expr,
    l: &Dynamic<'a>,
    r: &Dynamic<'a>,
    env: &mut Env<'a>,
    solver_opt: Option<&Solver<'a>>,
    left_live: Option<(Array<'a>, Dynamic<'a>)>,
    left_struct_leaves: Option<super::z3_types::StructArrayLeafSnapshot<'a>>,
) -> MumeiResult<Bool<'a>> {
    let ctx = vc.ctx;
    let (left_arr_live, left_len_live) = left_live
        .map(|(arr, len)| (Some(arr), Some(len)))
        .unwrap_or((None, None));
    let (Some(l_arr), Some(r_arr)) = (
        array_eq_operand(vc, env, left, l, left_arr_live),
        array_eq_operand(vc, env, right, r, None),
    ) else {
        return Err(MumeiError::type_error(
            "Cannot compare an array to a non-array value with ==".to_string(),
        ));
    };
    if l_arr.get_sort() != r_arr.get_sort() {
        return Err(MumeiError::type_error(
            "Cannot compare arrays of different element types".to_string(),
        ));
    }
    if l_arr.get_sort().array_range().map(|s| s.kind()) == Some(z3::SortKind::Array) {
        return Err(MumeiError::type_error(
            "Cannot compare nested arrays with ==; `[[T]]` elements carry no length".to_string(),
        ));
    }
    let len_l = array_eq_operand_len(vc, env, left, l, solver_opt, left_len_live);
    let len_r = array_eq_operand_len(vc, env, right, r, solver_opt, None);
    let len_eq = if len_l.get_sort() == len_r.get_sort() {
        len_l._eq(&len_r)
    } else if let (Some(li), Some(ri)) = (as_int_like(&len_l), as_int_like(&len_r)) {
        li._eq(&ri)
    } else {
        return Err(MumeiError::verification(
            "Array lengths are not comparable i64 values".to_string(),
        ));
    };
    let right_struct_leaves =
        super::z3_types::snapshot_struct_array_leaves(vc, env, right, r, None)?;
    if left_struct_leaves.is_some() || right_struct_leaves.is_some() {
        let (Some(left_struct_leaves), Some(right_struct_leaves)) =
            (left_struct_leaves, right_struct_leaves)
        else {
            return Err(MumeiError::type_error(
                "Cannot compare arrays of different element types".to_string(),
            ));
        };
        let left_type = vc
            .module_env
            .resolve_base_type(left_struct_leaves.element_type());
        let right_type = vc
            .module_env
            .resolve_base_type(right_struct_leaves.element_type());
        if left_type != right_type {
            return Err(MumeiError::type_error(
                "Cannot compare arrays of different element types".to_string(),
            ));
        }
        let leaves = super::z3_types::struct_array_leaf_types(&left_type, vc.module_env)
            .ok_or_else(|| {
                MumeiError::type_error(format!(
                    "Array equality is not supported for `[{}]` elements",
                    left_type
                ))
            })?;
        let k = Int::fresh_const(ctx, "k");
        let len_l_int = as_int_like(&len_l).ok_or_else(|| {
            MumeiError::verification("Array length is not an i64 value".to_string())
        })?;
        let in_bounds = Bool::and(ctx, &[&k.ge(&Int::from_i64(ctx, 0)), &k.lt(&len_l_int)]);
        let mut leaf_equalities = Vec::with_capacity(leaves.len());
        for (path, _) in leaves {
            let left_leaf = left_struct_leaves
                .leaf_array(&path)
                .ok_or_else(|| MumeiError::type_error(format!("Missing array leaf '{path}'")))?;
            let right_leaf = right_struct_leaves
                .leaf_array(&path)
                .ok_or_else(|| MumeiError::type_error(format!("Missing array leaf '{path}'")))?;
            if left_leaf.get_sort() != right_leaf.get_sort() {
                return Err(MumeiError::type_error(format!(
                    "Cannot compare array leaves of different types at '{path}'"
                )));
            }
            let left_value = left_leaf.select(&k);
            let right_value = right_leaf.select(&k);
            leaf_equalities.push(array_eq_element(ctx, &left_value, &right_value).ok_or_else(
                || {
                    MumeiError::type_error(format!(
                        "Array equality is not supported for struct leaf '{path}'"
                    ))
                },
            )?);
        }
        let leaf_equalities: Vec<&Bool> = leaf_equalities.iter().collect();
        let all_leaves_equal = Bool::and(ctx, &leaf_equalities);
        let contents_eq =
            z3::ast::forall_const(ctx, &[&k], &[], &in_bounds.implies(&all_leaves_equal));
        return Ok(Bool::and(ctx, &[&len_eq, &contents_eq]));
    }
    // `forall k. 0 <= k < len_l => a[k] == b[k]`. The bound is built in
    // the `Int` theory: the select index is always `Int`, and for a
    // *bound* variable `int2bv(k) <bv len` (what `index_in_bounds` would
    // produce under `--bitvec-i64`) is not the same condition — `int2bv`
    // wraps mod 2^64, so e.g. `k = 2^64` would satisfy `int2bv(k) <bv 2`.
    // Signed `bv2int` on the length is exact (lengths are `>= 0`), and
    // keeps the quantifier inside one theory.
    let k = Int::fresh_const(ctx, "k");
    let len_l_int = as_int_like(&len_l)
        .ok_or_else(|| MumeiError::verification("Array length is not an i64 value".to_string()))?;
    let in_bounds = Bool::and(ctx, &[&k.ge(&Int::from_i64(ctx, 0)), &k.lt(&len_l_int)]);
    let sel_l = l_arr.select(&k);
    let sel_r = r_arr.select(&k);
    let elem_eq = array_eq_element(ctx, &sel_l, &sel_r).ok_or_else(|| {
        MumeiError::type_error(format!(
            "Array equality is not supported for `[{}]` elements",
            z3_range_type_name(&l_arr.get_sort())
        ))
    })?;
    let body = in_bounds.implies(&elem_eq);
    // No explicit pattern: a select over an `ite` array (e.g. comparing an
    // if-bound `[T]`) makes `Z3_mk_forall_const` reject the pattern and
    // return null — the same fallback the `forall` builtin uses when it has
    // no admissible trigger. Z3's default trigger selection picks up the
    // `select(a,k)`/`select(b,k)` terms itself.
    let contents_eq = z3::ast::forall_const(ctx, &[&k], &[], &body);
    Ok(Bool::and(ctx, &[&len_eq, &contents_eq]))
}

const MAX_ENUM_PAYLOAD_EQUALITY_WORK: usize = 4096;

#[derive(Default)]
struct EnumPayloadEqualityWork {
    work: usize,
    array_payload_types: std::collections::HashMap<String, bool>,
    visiting_types: std::collections::HashSet<String>,
}

impl EnumPayloadEqualityWork {
    fn spend(&mut self) -> MumeiResult<()> {
        self.work = self.work.saturating_add(1);
        if self.work > MAX_ENUM_PAYLOAD_EQUALITY_WORK {
            return Err(MumeiError::type_error(
                "Enum payload equality exceeds its work budget",
            ));
        }
        Ok(())
    }
}

fn enum_type_needs_leafwise_equality(
    vc: &VCtx<'_>,
    enum_name: &str,
    work: &mut EnumPayloadEqualityWork,
) -> MumeiResult<bool> {
    let resolved_name = vc.module_env.resolve_base_type(enum_name);
    if let Some(&needs_leafwise_equality) = work.array_payload_types.get(&resolved_name) {
        return Ok(needs_leafwise_equality);
    }
    if !work.visiting_types.insert(resolved_name.clone()) {
        return Err(MumeiError::type_error(
            "Recursive enum payload equality is not supported",
        ));
    }
    let Some(enum_def) = vc.module_env.enums.get(&resolved_name) else {
        work.visiting_types.remove(&resolved_name);
        return Ok(false);
    };
    let mut needs_leafwise_equality = false;
    for variant in &enum_def.variants {
        work.spend()?;
        for field in &variant.field_types {
            work.spend()?;
            if datatype::recursive_field_target(enum_def, field, vc.module_env).is_some() {
                continue;
            }
            let field_type = datatype::payload_field_type_name(field);
            let Some(leaves) = datatype::field_leaf_types(&field_type, vc.module_env) else {
                continue;
            };
            if leaves.iter().any(|(_, base)| base.starts_with("array:")) {
                needs_leafwise_equality = true;
                break;
            }
            for (_, base) in &leaves {
                if let Some(inner_name) = base.strip_prefix("enum:") {
                    if enum_type_needs_leafwise_equality(vc, inner_name, work)? {
                        needs_leafwise_equality = true;
                        break;
                    }
                }
            }
            if needs_leafwise_equality {
                break;
            }
        }
        if needs_leafwise_equality {
            break;
        }
    }
    work.visiting_types.remove(&resolved_name);
    work.array_payload_types
        .insert(resolved_name, needs_leafwise_equality);
    Ok(needs_leafwise_equality)
}

fn enum_payload_value_equality<'a>(
    vc: &VCtx<'a>,
    left: &Dynamic<'a>,
    right: &Dynamic<'a>,
) -> MumeiResult<Option<Bool<'a>>> {
    let Some(enum_def) = vc.module_env.enums.values().find(|enum_def| {
        datatype::enum_datatype_sort(vc, enum_def).is_some_and(|sort| sort.sort == left.get_sort())
    }) else {
        return Ok(None);
    };
    let mut work = EnumPayloadEqualityWork::default();
    if !enum_type_needs_leafwise_equality(vc, &enum_def.name, &mut work)? {
        return Ok(None);
    }
    let equality = enum_payload_value_equality_inner(vc, left, right, &mut work)?
        .ok_or_else(|| MumeiError::type_error("Unable to lower enum payload equality"))?;
    Ok(Some(equality))
}

fn enum_payload_value_equality_inner<'a>(
    vc: &VCtx<'a>,
    left: &Dynamic<'a>,
    right: &Dynamic<'a>,
    work: &mut EnumPayloadEqualityWork,
) -> MumeiResult<Option<Bool<'a>>> {
    let Some(enum_def) = vc.module_env.enums.values().find(|enum_def| {
        datatype::enum_datatype_sort(vc, enum_def).is_some_and(|sort| sort.sort == left.get_sort())
    }) else {
        return Ok(None);
    };
    let Some(sort) = datatype::enum_datatype_sort(vc, enum_def) else {
        return Ok(None);
    };
    let mut has_array_payload = false;
    let mut variant_equalities = Vec::with_capacity(enum_def.variants.len());
    for (variant_idx, variant) in enum_def.variants.iter().enumerate() {
        work.spend()?;
        let Some((_, left_tag, _, _)) = datatype::variant_tester_condition(vc, left, &variant.name)
        else {
            return Ok(None);
        };
        let Some((_, right_tag, _, _)) =
            datatype::variant_tester_condition(vc, right, &variant.name)
        else {
            return Ok(None);
        };
        let mut payload_equalities = Vec::new();
        for (field_idx, field) in variant.field_types.iter().enumerate() {
            work.spend()?;
            if datatype::recursive_field_target(enum_def, field, vc.module_env).is_some() {
                let Some((offset, width)) =
                    datatype::payload_leaf_range(enum_def, vc.module_env, variant_idx, field_idx)
                else {
                    return Ok(None);
                };
                if width != 1 {
                    return Err(MumeiError::type_error(
                        "Recursive enum payload has an invalid datatype accessor width",
                    ));
                }
                let Some(left_value) =
                    datatype::variant_selector_apply(&sort, variant_idx, offset, left)
                else {
                    return Ok(None);
                };
                let Some(right_value) =
                    datatype::variant_selector_apply(&sort, variant_idx, offset, right)
                else {
                    return Ok(None);
                };
                if left_value.get_sort() != right_value.get_sort() {
                    return Err(MumeiError::type_error(
                        "Recursive enum payload values have different field types",
                    ));
                }
                payload_equalities.push(left_value._eq(&right_value));
                continue;
            }
            let field_type = datatype::payload_field_type_name(field);
            let Some(leaf_types) = datatype::field_leaf_types(&field_type, vc.module_env) else {
                return Ok(None);
            };
            let Some((offset, _)) =
                datatype::payload_leaf_range(enum_def, vc.module_env, variant_idx, field_idx)
            else {
                return Ok(None);
            };
            let mut compared_lengths = std::collections::HashSet::new();
            for (leaf_idx, (name, base)) in leaf_types.iter().enumerate() {
                work.spend()?;
                if base == "array_len" {
                    continue;
                }
                let Some(array_type) = base.strip_prefix("array:") else {
                    let Some(left_value) = datatype::variant_selector_apply(
                        &sort,
                        variant_idx,
                        offset + leaf_idx,
                        left,
                    ) else {
                        return Ok(None);
                    };
                    let Some(right_value) = datatype::variant_selector_apply(
                        &sort,
                        variant_idx,
                        offset + leaf_idx,
                        right,
                    ) else {
                        return Ok(None);
                    };
                    if left_value.get_sort() != right_value.get_sort() {
                        return Err(MumeiError::type_error(
                            "Enum payload values have different field types",
                        ));
                    }
                    let nested_enum_eq = if let (Some(inner_name), true) = (
                        base.strip_prefix("enum:"),
                        left_value.as_datatype().is_some(),
                    ) {
                        if enum_type_needs_leafwise_equality(vc, inner_name, work)? {
                            Some(
                                enum_payload_value_equality_inner(
                                    vc,
                                    &left_value,
                                    &right_value,
                                    work,
                                )?
                                .ok_or_else(|| {
                                    MumeiError::type_error(
                                        "Unable to lower nested enum payload equality",
                                    )
                                })?,
                            )
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    if let Some(nested_enum_eq) = nested_enum_eq {
                        has_array_payload = true;
                        payload_equalities.push(nested_enum_eq);
                    } else {
                        payload_equalities.push(left_value._eq(&right_value));
                    }
                    continue;
                };
                has_array_payload = true;
                let Some(length_idx) = datatype::array_payload_length_index(&leaf_types, leaf_idx)
                else {
                    return Ok(None);
                };
                if compared_lengths.insert(length_idx) {
                    let Some(left_len) = datatype::variant_selector_apply(
                        &sort,
                        variant_idx,
                        offset + length_idx,
                        left,
                    ) else {
                        return Ok(None);
                    };
                    let Some(right_len) = datatype::variant_selector_apply(
                        &sort,
                        variant_idx,
                        offset + length_idx,
                        right,
                    ) else {
                        return Ok(None);
                    };
                    let len_eq = if left_len.get_sort() == right_len.get_sort() {
                        left_len._eq(&right_len)
                    } else if let (Some(l), Some(r)) =
                        (as_int_like(&left_len), as_int_like(&right_len))
                    {
                        l._eq(&r)
                    } else {
                        return Err(MumeiError::type_error(
                            "Enum payload array lengths are not comparable",
                        ));
                    };
                    payload_equalities.push(len_eq);
                }
                let Some(left_array) =
                    datatype::variant_selector_apply(&sort, variant_idx, offset + leaf_idx, left)
                        .and_then(|value| value.as_array())
                else {
                    return Ok(None);
                };
                let Some(right_array) =
                    datatype::variant_selector_apply(&sort, variant_idx, offset + leaf_idx, right)
                        .and_then(|value| value.as_array())
                else {
                    return Ok(None);
                };
                if left_array.get_sort() != right_array.get_sort() {
                    return Err(MumeiError::type_error(format!(
                        "Enum payload array leaf '{name}' has mismatched types"
                    )));
                }
                let Some(len) =
                    datatype::variant_selector_apply(&sort, variant_idx, offset + length_idx, left)
                else {
                    return Ok(None);
                };
                let Some(len) = as_int_like(&len) else {
                    return Err(MumeiError::type_error(
                        "Enum payload array length is not an integer",
                    ));
                };
                let index = Int::fresh_const(vc.ctx, "enum_payload_index");
                let in_bounds = Bool::and(
                    vc.ctx,
                    &[&index.ge(&Int::from_i64(vc.ctx, 0)), &index.lt(&len)],
                );
                let left_value = left_array.select(&index);
                let right_value = right_array.select(&index);
                let Some(element_eq) = array_eq_element(vc.ctx, &left_value, &right_value) else {
                    return Err(MumeiError::type_error(format!(
                        "Enum payload array leaf '{name}' cannot be compared"
                    )));
                };
                payload_equalities.push(z3::ast::forall_const(
                    vc.ctx,
                    &[&index],
                    &[],
                    &in_bounds.implies(&element_eq),
                ));
                let _ = array_type;
            }
        }
        let payload_refs: Vec<&Bool> = payload_equalities.iter().collect();
        let payload_eq = Bool::and(vc.ctx, &payload_refs);
        variant_equalities.push(Bool::and(vc.ctx, &[&left_tag, &right_tag, &payload_eq]));
    }
    if !has_array_payload {
        return Ok(None);
    }
    let variant_refs: Vec<&Bool> = variant_equalities.iter().collect();
    Ok(Some(Bool::or(vc.ctx, &variant_refs)))
}

fn enum_payload_field_read<'a>(
    vc: &VCtx<'a>,
    inner_expr: &Expr,
    field_name: &str,
    env: &mut Env<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> Result<Option<Dynamic<'a>>, MumeiError> {
    let mut fields = Vec::new();
    let mut base_expr = inner_expr;
    while let Expr::FieldAccess(inner, field) = base_expr {
        fields.push(field.clone());
        base_expr = inner;
    }
    fields.reverse();
    if !matches!(base_expr, Expr::Variable(_)) {
        return Ok(None);
    }
    if fields.iter().any(|field| field == "tag") {
        return Ok(None);
    }
    let mut field_path = fields.clone();
    field_path.push(field_name.to_string());

    let mut prefix = base_expr.clone();
    let mut enum_prefixes = Vec::new();
    if let Some(enum_name) = datatype::infer_expr_enum_name(vc, &prefix) {
        enum_prefixes.push((prefix.clone(), 0, enum_name));
    }
    for (idx, field) in fields.iter().enumerate() {
        prefix = Expr::FieldAccess(Box::new(prefix), field.clone());
        if let Some(enum_name) = datatype::infer_expr_enum_name(vc, &prefix) {
            enum_prefixes.push((prefix.clone(), idx + 1, enum_name));
        }
    }
    let Some((root, root_len, enum_name)) = enum_prefixes.last().cloned() else {
        return Ok(None);
    };
    let path: Vec<String> = field_path.iter().skip(root_len).cloned().collect();
    let Some(payload_name) = path.first() else {
        return Ok(None);
    };
    let enum_def = datatype::declared_type_of_expr(vc, &root)
        .map(|ty| vc.module_env.resolve_base_type(&ty))
        .and_then(|ty| {
            vc.module_env
                .get_enum(&ty)
                .or_else(|| vc.module_env.get_enum(datatype::type_name_base(&ty)))
        })
        .or_else(|| vc.module_env.get_enum(&enum_name));
    let Some(enum_def) = enum_def else {
        return Ok(None);
    };
    if !datatype::uses_datatype_sort(enum_def, vc.module_env)
        || datatype::enum_datatype_sort(vc, enum_def).is_none()
    {
        return Ok(None);
    }

    let mut owner = None;
    for (variant_idx, variant) in enum_def.variants.iter().enumerate() {
        let Some(field_idx) = variant.fields.iter().position(|payload| {
            payload
                .split_once(':')
                .is_some_and(|(name, _)| name.trim() == payload_name)
        }) else {
            continue;
        };
        if owner.is_some() {
            return Ok(None);
        }
        owner = Some((variant_idx, field_idx));
    }
    let Some((variant_idx, field_idx)) = owner else {
        return Ok(None);
    };
    let Some(field_type_ref) = enum_def.variants[variant_idx].field_types.get(field_idx) else {
        return Ok(None);
    };
    if let Some(target_enum) =
        datatype::recursive_field_target(enum_def, field_type_ref, vc.module_env)
    {
        let Some(target_sort) = datatype::enum_datatype_sort(vc, target_enum) else {
            return Ok(None);
        };
        let Some((payload_offset, _)) =
            datatype::payload_leaf_range(enum_def, vc.module_env, variant_idx, field_idx)
        else {
            return Ok(None);
        };
        let base = expr_to_z3(vc, &root, env, None)?;
        let Some((sort, is_variant, resolved_idx, resolved_enum)) =
            datatype::variant_tester_condition(vc, &base, &enum_def.variants[variant_idx].name)
        else {
            return Ok(None);
        };
        if resolved_idx != variant_idx || resolved_enum.name != enum_def.name {
            return Ok(None);
        }
        let Some(selector) =
            datatype::variant_selector_apply(&sort, variant_idx, payload_offset, &base)
        else {
            return Ok(None);
        };
        if selector.get_sort() != target_sort.sort {
            return Ok(None);
        }
        let Some(fresh) = super::z3_types::dynamic_fresh_const(
            vc.ctx,
            "__recursive_enum_payload_read",
            &selector,
        ) else {
            return Ok(None);
        };
        let Some(value) = super::z3_types::dynamic_ite(&is_variant, &selector, &fresh) else {
            return Ok(None);
        };
        if let (Some(solver), Some(domain)) =
            (solver_opt, datatype::enum_value_leaf_domain(vc, &value))
        {
            solver.assert(&is_variant.implies(&domain));
        }
        return Ok(Some(value));
    }
    let field_type = datatype::payload_field_type_name(field_type_ref);
    let Some((payload_offset, payload_width)) =
        datatype::payload_leaf_range(enum_def, vc.module_env, variant_idx, field_idx)
    else {
        return Ok(None);
    };
    let Some(field_leaves) = datatype::field_leaf_types(&field_type, vc.module_env) else {
        return Ok(None);
    };
    if field_leaves.len() != payload_width {
        return Ok(None);
    }
    let direct_struct_array_payload =
        datatype::struct_array_payload_element_type(&field_type, vc.module_env);
    let direct_array_payload = datatype::array_payload_element_type(&field_leaves).is_some()
        || direct_struct_array_payload.is_some();
    if path.len() == 1 && direct_struct_array_payload.is_some() {
        return Err(MumeiError::type_error(
            "direct reads of struct-array enum payloads are not supported; destructure the payload with match",
        ));
    }

    let mut terminal_type = field_type.clone();
    for segment in &path[1..] {
        let resolved = vc.module_env.resolve_base_type(&terminal_type);
        let Some(sdef) = vc.module_env.get_struct(&resolved).or_else(|| {
            vc.module_env
                .get_struct(datatype::type_name_base(&resolved))
        }) else {
            return Ok(None);
        };
        let Some(field) = sdef.fields.iter().find(|field| field.name == *segment) else {
            return Ok(None);
        };
        terminal_type = field.type_name.clone();
    }
    let resolved_terminal_type = vc.module_env.resolve_base_type(&terminal_type);
    let terminal_struct = vc
        .module_env
        .get_struct(&resolved_terminal_type)
        .or_else(|| {
            vc.module_env
                .get_struct(datatype::type_name_base(&resolved_terminal_type))
        });
    let (leaf_offset, struct_leaf_width, selected_leaf_types) = if terminal_struct.is_some() {
        let Some(struct_leaves) = datatype::field_leaf_types(&terminal_type, vc.module_env) else {
            return Ok(None);
        };
        if struct_leaves.is_empty() {
            return Ok(None);
        }
        let path_prefix = path[1..].join("_");
        let selected_leaf_names: Vec<String> = struct_leaves
            .iter()
            .map(|(name, _)| {
                if path_prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{path_prefix}_{name}")
                }
            })
            .collect();
        let Some(leaf_offset) =
            field_leaves
                .windows(selected_leaf_names.len())
                .position(|leaves| {
                    leaves
                        .iter()
                        .map(|(name, _)| name)
                        .eq(selected_leaf_names.iter())
                })
        else {
            return Ok(None);
        };
        (leaf_offset, selected_leaf_names.len(), Some(struct_leaves))
    } else {
        let leaf_offset = if path.len() == 1 {
            if direct_array_payload {
                0
            } else if payload_width != 1 || !field_leaves[0].0.is_empty() {
                return Ok(None);
            } else {
                0
            }
        } else {
            let leaf_name = path[1..].join("_");
            let Some(leaf_offset) = field_leaves.iter().position(|(name, _)| name == &leaf_name)
            else {
                return Ok(None);
            };
            leaf_offset
        };
        (leaf_offset, 0, None)
    };
    let array_leaf_pair = datatype::array_payload_leaf_pairs(&field_leaves)
        .find(|(array_index, _)| *array_index == leaf_offset);

    if terminal_struct.is_some() && solver_opt.is_none() {
        return Ok(None);
    }

    if solver_opt.is_none() {
        let mut tag_conditions = Vec::with_capacity(enum_prefixes.len());
        for (outer_root, consumed, outer_enum_name) in enum_prefixes
            .iter()
            .take(enum_prefixes.len().saturating_sub(1))
        {
            let outer_enum_def = datatype::declared_type_of_expr(vc, outer_root)
                .map(|ty| vc.module_env.resolve_base_type(&ty))
                .and_then(|ty| {
                    vc.module_env
                        .get_enum(&ty)
                        .or_else(|| vc.module_env.get_enum(datatype::type_name_base(&ty)))
                })
                .or_else(|| vc.module_env.get_enum(outer_enum_name));
            let Some(outer_enum_def) = outer_enum_def else {
                return Ok(None);
            };
            if !datatype::uses_datatype_sort(outer_enum_def, vc.module_env)
                || datatype::enum_datatype_sort(vc, outer_enum_def).is_none()
            {
                return Ok(None);
            }
            let Some(outer_payload_name) = field_path.get(*consumed) else {
                return Ok(None);
            };
            let mut outer_owner = None;
            for (variant_idx, variant) in outer_enum_def.variants.iter().enumerate() {
                let Some(field_idx) = variant.fields.iter().position(|payload| {
                    payload
                        .split_once(':')
                        .is_some_and(|(name, _)| name.trim() == outer_payload_name)
                }) else {
                    continue;
                };
                if outer_owner.is_some() {
                    return Ok(None);
                }
                outer_owner = Some((variant_idx, field_idx));
            }
            let Some((outer_variant_idx, outer_field_idx)) = outer_owner else {
                return Ok(None);
            };
            let Some(outer_field_type_ref) = outer_enum_def.variants[outer_variant_idx]
                .field_types
                .get(outer_field_idx)
            else {
                return Ok(None);
            };
            let outer_recursive_target = datatype::recursive_field_target(
                outer_enum_def,
                outer_field_type_ref,
                vc.module_env,
            );
            let outer_field_type = datatype::payload_field_type_name(outer_field_type_ref);
            let Some((_, width)) = datatype::payload_leaf_range(
                outer_enum_def,
                vc.module_env,
                outer_variant_idx,
                outer_field_idx,
            ) else {
                return Ok(None);
            };
            if width == 0 {
                return Ok(None);
            }
            if let Some(target_enum) = outer_recursive_target {
                if datatype::enum_datatype_sort(vc, target_enum).is_none() {
                    return Ok(None);
                }
            } else {
                let Some(leaves) = datatype::field_leaf_types(&outer_field_type, vc.module_env)
                else {
                    return Ok(None);
                };
                if leaves.len() != width {
                    return Ok(None);
                }
            }
            let outer_base = expr_to_z3(vc, outer_root, env, None)?;
            let Some((_, is_variant, resolved_idx, resolved_enum)) =
                datatype::variant_tester_condition(
                    vc,
                    &outer_base,
                    &outer_enum_def.variants[outer_variant_idx].name,
                )
            else {
                return Ok(None);
            };
            if resolved_idx != outer_variant_idx || resolved_enum.name != outer_enum_def.name {
                return Ok(None);
            }
            tag_conditions.push(is_variant);
        }
        let base = expr_to_z3(vc, &root, env, None)?;
        let Some((sort, is_variant, resolved_idx, resolved_enum)) =
            datatype::variant_tester_condition(vc, &base, &enum_def.variants[variant_idx].name)
        else {
            return Ok(None);
        };
        if resolved_idx != variant_idx || resolved_enum.name != enum_def.name {
            return Ok(None);
        }
        let Some(selector) = datatype::variant_selector_apply(
            &sort,
            variant_idx,
            payload_offset + leaf_offset,
            &base,
        ) else {
            return Ok(None);
        };
        tag_conditions.push(is_variant);
        let gate = Bool::and(vc.ctx, &tag_conditions.iter().collect::<Vec<_>>());
        let Some(fresh) =
            super::z3_types::dynamic_fresh_const(vc.ctx, "__enum_payload_read", &selector)
        else {
            return Ok(None);
        };
        let Some(value) = super::z3_types::dynamic_ite(&gate, &selector, &fresh) else {
            return Ok(None);
        };
        if let Some((array_index, _)) = array_leaf_pair {
            let Some(length_selector) = datatype::variant_selector_apply(
                &sort,
                variant_idx,
                payload_offset + array_index + 1,
                &base,
            ) else {
                return Ok(None);
            };
            let Some(length_fresh) = super::z3_types::dynamic_fresh_const(
                vc.ctx,
                "__enum_payload_len_read",
                &length_selector,
            ) else {
                return Ok(None);
            };
            let Some(length) = super::z3_types::dynamic_ite(&gate, &length_selector, &length_fresh)
            else {
                return Ok(None);
            };
            if let Some(array) = value.as_array() {
                super::z3_types::register_array_value_len(env, &array, length);
            }
        }
        return Ok(Some(value));
    }
    let Some(solver) = solver_opt else {
        return Ok(None);
    };
    for (outer_root, consumed, outer_enum_name) in enum_prefixes
        .iter()
        .take(enum_prefixes.len().saturating_sub(1))
    {
        let outer_enum_def = datatype::declared_type_of_expr(vc, outer_root)
            .map(|ty| vc.module_env.resolve_base_type(&ty))
            .and_then(|ty| {
                vc.module_env
                    .get_enum(&ty)
                    .or_else(|| vc.module_env.get_enum(datatype::type_name_base(&ty)))
            })
            .or_else(|| vc.module_env.get_enum(outer_enum_name));
        let Some(outer_enum_def) = outer_enum_def else {
            return Ok(None);
        };
        if !datatype::uses_datatype_sort(outer_enum_def, vc.module_env)
            || datatype::enum_datatype_sort(vc, outer_enum_def).is_none()
        {
            return Ok(None);
        }
        let Some(outer_payload_name) = field_path.get(*consumed) else {
            return Ok(None);
        };
        let mut outer_owner = None;
        for (variant_idx, variant) in outer_enum_def.variants.iter().enumerate() {
            let Some(field_idx) = variant.fields.iter().position(|payload| {
                payload
                    .split_once(':')
                    .is_some_and(|(name, _)| name.trim() == outer_payload_name)
            }) else {
                continue;
            };
            if outer_owner.is_some() {
                return Ok(None);
            }
            outer_owner = Some((variant_idx, field_idx));
        }
        let Some((outer_variant_idx, outer_field_idx)) = outer_owner else {
            return Ok(None);
        };
        let Some(outer_field_type_ref) = outer_enum_def.variants[outer_variant_idx]
            .field_types
            .get(outer_field_idx)
        else {
            return Ok(None);
        };
        let outer_recursive_target =
            datatype::recursive_field_target(outer_enum_def, outer_field_type_ref, vc.module_env);
        let outer_field_type = datatype::payload_field_type_name(outer_field_type_ref);
        let Some((_, width)) = datatype::payload_leaf_range(
            outer_enum_def,
            vc.module_env,
            outer_variant_idx,
            outer_field_idx,
        ) else {
            return Ok(None);
        };
        if width == 0 {
            return Ok(None);
        }
        if let Some(target_enum) = outer_recursive_target {
            if datatype::enum_datatype_sort(vc, target_enum).is_none() {
                return Ok(None);
            }
        } else {
            let Some(leaves) = datatype::field_leaf_types(&outer_field_type, vc.module_env) else {
                return Ok(None);
            };
            if leaves.len() != width {
                return Ok(None);
            }
        }
        let outer_base = expr_to_z3(vc, outer_root, env, Some(solver))?;
        let Some((_, is_variant, resolved_idx, resolved_enum)) = datatype::variant_tester_condition(
            vc,
            &outer_base,
            &outer_enum_def.variants[outer_variant_idx].name,
        ) else {
            return Ok(None);
        };
        if resolved_idx != outer_variant_idx || resolved_enum.name != outer_enum_def.name {
            return Ok(None);
        }
        solver.push();
        solver.assert(&vc.path_cond_conj());
        solver.assert(&is_variant.not());
        let tag_proven = crate::verification::phase_artifacts::check(solver) == SatResult::Unsat;
        solver.pop(1);
        if !tag_proven {
            return Ok(None);
        }
    }
    let base = expr_to_z3(vc, &root, env, Some(solver))?;
    let Some((sort, is_variant, resolved_idx, resolved_enum)) =
        datatype::variant_tester_condition(vc, &base, &enum_def.variants[variant_idx].name)
    else {
        return Ok(None);
    };
    if resolved_idx != variant_idx || resolved_enum.name != enum_def.name {
        return Ok(None);
    }
    solver.push();
    solver.assert(&vc.path_cond_conj());
    solver.assert(&is_variant.not());
    let tag_proven = crate::verification::phase_artifacts::check(solver) == SatResult::Unsat;
    solver.pop(1);
    if !tag_proven {
        return Ok(None);
    }
    if let Some(sdef) = terminal_struct {
        let mut leaves = Vec::with_capacity(struct_leaf_width);
        for offset in leaf_offset..leaf_offset + struct_leaf_width {
            let Some(leaf) = datatype::variant_selector_apply(
                &sort,
                variant_idx,
                payload_offset + offset,
                &base,
            ) else {
                return Ok(None);
            };
            leaves.push(leaf);
        }
        let Some(leaf_types) = selected_leaf_types.as_ref() else {
            return Ok(None);
        };
        datatype::assert_payload_array_lengths(vc, solver, &is_variant, leaf_types, &leaves);
        let env_keys_before: std::collections::HashSet<String> = env.keys().cloned().collect();
        let Some(direct) = crate::verification::translator::pattern::rebuild_payload_direct_fields(
            vc.ctx, vc, env, sdef, &leaves,
        ) else {
            let new_keys: Vec<String> = env
                .keys()
                .filter(|key| !env_keys_before.contains(*key))
                .cloned()
                .collect();
            for key in new_keys {
                env.remove(&key);
            }
            return Ok(None);
        };
        let handle = crate::verification::translator::fresh_struct_handle(vc.ctx, &sdef.name);
        let handle_name = handle.decl().name();
        crate::verification::translator::bind_struct_fields(env, &handle_name, &direct);
        return Ok(Some(handle));
    }
    let value =
        datatype::variant_selector_apply(&sort, variant_idx, payload_offset + leaf_offset, &base);
    if let Some((array_index, _)) = array_leaf_pair {
        let Some(length) = datatype::variant_selector_apply(
            &sort,
            variant_idx,
            payload_offset + array_index + 1,
            &base,
        ) else {
            return Ok(None);
        };
        let Some(nonnegative) = super::z3_types::nonneg_constraint(vc.ctx, &length) else {
            return Ok(None);
        };
        solver.assert(&is_variant.implies(&nonnegative));
        if let Some(array) = value.as_ref().and_then(|value| value.as_array()) {
            super::z3_types::register_array_value_len(env, &array, length);
        }
    }
    Ok(value)
}

pub(crate) fn struct_array_expr_type(vc: &VCtx<'_>, expr: &Expr) -> Option<String> {
    let type_name = match expr {
        Expr::StructInit { type_name, .. } => Some(type_name.clone()),
        _ => datatype::declared_type_of_expr(vc, expr),
    }?;
    let base = vc.module_env.resolve_base_type(&type_name);
    let struct_base = datatype::type_name_base(&base);
    vc.module_env
        .get_struct(&base)
        .or_else(|| vc.module_env.get_struct(struct_base))?;
    Some(struct_base.to_string())
}

pub(crate) fn struct_array_leaf_values_from_value<'a>(
    vc: &VCtx<'a>,
    env: &Env<'a>,
    value: &Dynamic<'a>,
    type_name: &str,
) -> Result<Vec<(String, String, Dynamic<'a>)>, MumeiError> {
    fn collect<'a>(
        vc: &VCtx<'a>,
        env: &Env<'a>,
        value: &Dynamic<'a>,
        type_name: &str,
        prefix: &str,
        leaves: &mut Vec<(String, String, Dynamic<'a>)>,
    ) -> Result<(), MumeiError> {
        let base = vc.module_env.resolve_base_type(type_name);
        let struct_base = datatype::type_name_base(&base);
        let sdef = vc
            .module_env
            .get_struct(&base)
            .or_else(|| vc.module_env.get_struct(struct_base))
            .ok_or_else(|| {
                MumeiError::type_error(format!("array element '{type_name}' is not a struct"))
            })?;
        let fields = super::struct_fields_of_value(env, value, sdef).ok_or_else(|| {
            MumeiError::type_error(format!(
                "cannot resolve scalar leaves for struct array element '{type_name}'"
            ))
        })?;
        for field in &sdef.fields {
            let field_value = fields
                .iter()
                .find(|(name, _)| name == &field.name)
                .map(|(_, value)| value)
                .ok_or_else(|| {
                    MumeiError::type_error(format!(
                        "struct array element is missing field '{}'",
                        field.name
                    ))
                })?;
            let path = if prefix.is_empty() {
                field.name.clone()
            } else {
                format!("{prefix}_{}", field.name)
            };
            let field_type = vc.module_env.resolve_base_type(&field.type_name);
            let nested_base = datatype::type_name_base(&field_type);
            if vc.module_env.get_struct(&field_type).is_some()
                || vc.module_env.get_struct(nested_base).is_some()
            {
                collect(vc, env, field_value, nested_base, &path, leaves)?;
            } else {
                leaves.push((path, field_type, field_value.clone()));
            }
        }
        Ok(())
    }

    let mut leaves = Vec::new();
    collect(vc, env, value, type_name, "", &mut leaves)?;
    Ok(leaves)
}

fn struct_array_access_path(expr: &Expr) -> Option<(String, Expr, Vec<String>)> {
    fn collect(expr: &Expr, fields: &mut Vec<String>) -> Option<(String, Expr)> {
        match expr {
            Expr::FieldAccess(inner, field) => {
                let base = collect(inner, fields)?;
                fields.push(field.clone());
                Some(base)
            }
            Expr::ArrayAccess(name, index) => Some((name.clone(), (**index).clone())),
            _ => None,
        }
    }
    let mut fields = Vec::new();
    let (name, index) = collect(expr, &mut fields)?;
    Some((name, index, fields))
}

fn bind_struct_array_fields<'a>(
    vc: &VCtx<'a>,
    binding: &str,
    type_name: &str,
    element: (&str, &z3::ast::Int<'a>),
    prefix: &str,
    env: &mut Env<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> Result<(), MumeiError> {
    let (array_name, index) = element;
    let base = vc.module_env.resolve_base_type(type_name);
    let struct_base = datatype::type_name_base(&base);
    let sdef = vc
        .module_env
        .get_struct(&base)
        .or_else(|| vc.module_env.get_struct(struct_base))
        .ok_or_else(|| {
            MumeiError::type_error(format!("array element '{type_name}' is not a struct"))
        })?;
    for field in &sdef.fields {
        let path = if prefix.is_empty() {
            field.name.clone()
        } else {
            format!("{prefix}_{}", field.name)
        };
        let field_type = vc.module_env.resolve_base_type(&field.type_name);
        let nested_base = datatype::type_name_base(&field_type);
        if vc.module_env.get_struct(&field_type).is_some()
            || vc.module_env.get_struct(nested_base).is_some()
        {
            let nested_handle = super::fresh_struct_handle(vc.ctx, nested_base);
            let field_key = super::struct_field_key(binding, &field.name);
            env.insert(field_key, nested_handle.clone());
            let nested_handle_name = nested_handle.decl().name().to_string();
            env.insert(format!("{binding}_{}", field.name), nested_handle);
            bind_struct_array_fields(
                vc,
                &nested_handle_name,
                nested_base,
                element,
                &path,
                env,
                solver_opt,
            )?;
        } else {
            let leaf_array =
                super::z3_types::struct_array_leaf_array(vc, array_name, &path, &field_type, env);
            let selected = leaf_array.select(index);
            let bound = datatype::enum_value_leaf_domain(vc, &selected)
                .or_else(|| datatype::scalar_int_leaf_domain(vc, &field_type, &selected));
            if let (Some(solver), Some(bound)) = (solver_opt, bound) {
                solver.assert(&bound);
            }
            env.insert(
                super::struct_field_key(binding, &field.name),
                selected.clone(),
            );
            env.insert(format!("{binding}_{}", field.name), selected);
        }
    }
    Ok(())
}

pub(crate) fn bind_struct_array_element<'a>(
    vc: &VCtx<'a>,
    _binding: &str,
    array_name: &str,
    index_expr: &Expr,
    env: &mut Env<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> Result<Option<Dynamic<'a>>, MumeiError> {
    let element_type = super::z3_types::array_element_type_name(array_name, vc);
    let sort =
        super::z3_types::array_element_sort_from_type(&element_type, vc.module_env, vc.ieee754_f64);
    if sort != super::z3_types::ArrayElementSort::Struct
        && sort != super::z3_types::ArrayElementSort::UnsupportedStruct
    {
        return Ok(None);
    }
    super::z3_types::validate_array_element_type(vc, &element_type)?;
    let index_value = expr_to_z3(vc, index_expr, env, solver_opt)?;
    let index = as_int_like(&index_value)
        .ok_or_else(|| MumeiError::type_error("Array index must be integer"))?;
    if let Some(solver) = solver_opt {
        let len = array_len_value(vc.ctx, env, array_name, vc.bitvec_i64, Some(solver));
        let safe = index_in_bounds(vc.ctx, &index_value, &len)
            .ok_or_else(|| MumeiError::type_error("Index must be integer"))?;
        solver.push();
        for cond in vc.path_cond_stack.borrow().iter() {
            solver.assert(cond);
        }
        solver.assert(&safe.not());
        if crate::verification::phase_artifacts::check(solver) == SatResult::Sat {
            solver.pop(1);
            return Err(MumeiError::verification(format!(
                "Potential Out-of-Bounds on '{}' (index may be < 0 or >= len_{})",
                array_name, array_name
            ))
            .with_help("requires にインデックスの範囲制約 (0 <= idx < len) を追加してください")
            .with_verdict(crate::verification::VerdictKind::Rejected));
        }
        solver.pop(1);
    }
    let handle = super::fresh_struct_handle(vc.ctx, &element_type);
    let handle_name = handle.decl().name();
    let leaves = super::z3_types::struct_array_leaf_types(&element_type, vc.module_env)
        .ok_or_else(|| {
            MumeiError::type_error(format!(
                "array element struct '{element_type}' must have only scalar leaves"
            ))
        })?;
    let _ = leaves;
    bind_struct_array_fields(
        vc,
        &handle_name,
        &element_type,
        (array_name, &index),
        "",
        env,
        solver_opt,
    )?;
    Ok(Some(handle))
}

fn read_struct_array_field<'a>(
    vc: &VCtx<'a>,
    array_name: &str,
    index_expr: &Expr,
    field_path: &[String],
    env: &mut Env<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> Result<Option<Dynamic<'a>>, MumeiError> {
    let element_type = super::z3_types::array_element_type_name(array_name, vc);
    let sort =
        super::z3_types::array_element_sort_from_type(&element_type, vc.module_env, vc.ieee754_f64);
    if sort != super::z3_types::ArrayElementSort::Struct
        && sort != super::z3_types::ArrayElementSort::UnsupportedStruct
    {
        return Ok(None);
    }
    super::z3_types::validate_array_element_type(vc, &element_type)?;
    let path = field_path.join("_");
    let Some((_, leaf_type)) =
        super::z3_types::struct_array_leaf_types(&element_type, vc.module_env)
            .and_then(|leaves| leaves.into_iter().find(|(leaf, _)| leaf == &path))
    else {
        return Err(MumeiError::type_error(format!(
            "field path '{path}' is not a scalar leaf of array element '{element_type}'"
        )));
    };
    let index_value = expr_to_z3(vc, index_expr, env, solver_opt)?;
    let index = as_int_like(&index_value)
        .ok_or_else(|| MumeiError::type_error("Array index must be integer"))?;
    if let Some(solver) = solver_opt {
        let len = array_len_value(vc.ctx, env, array_name, vc.bitvec_i64, Some(solver));
        let safe = index_in_bounds(vc.ctx, &index_value, &len)
            .ok_or_else(|| MumeiError::type_error("Index must be integer"))?;
        solver.push();
        for cond in vc.path_cond_stack.borrow().iter() {
            solver.assert(cond);
        }
        solver.assert(&safe.not());
        if crate::verification::phase_artifacts::check(solver) == SatResult::Sat {
            solver.pop(1);
            return Err(MumeiError::verification(format!(
                "Potential Out-of-Bounds on '{}' (index may be < 0 or >= len_{})",
                array_name, array_name
            ))
            .with_help("requires にインデックスの範囲制約 (0 <= idx < len) を追加してください")
            .with_verdict(crate::verification::VerdictKind::Rejected));
        }
        solver.pop(1);
    }
    let leaf_array =
        super::z3_types::struct_array_leaf_array(vc, array_name, &path, &leaf_type, env);
    let selected = leaf_array.select(&index);
    let bound = datatype::enum_value_leaf_domain(vc, &selected)
        .or_else(|| datatype::scalar_int_leaf_domain(vc, &leaf_type, &selected));
    if let (Some(solver), Some(bound)) = (solver_opt, bound) {
        solver.assert(&bound);
    }
    Ok(Some(selected))
}

fn refined_predicate_rebinds_operand(expr: &Expr, operand: &str) -> bool {
    expr.rebinds_name(operand) || predicate_contains_shadowing_binder(expr, operand)
}

fn predicate_pattern_binds_operand(pattern: &crate::parser::ast::Pattern, operand: &str) -> bool {
    match pattern {
        crate::parser::ast::Pattern::Variable(name) => name == operand,
        crate::parser::ast::Pattern::Variant { fields, .. } => fields
            .iter()
            .any(|field| predicate_pattern_binds_operand(field, operand)),
        crate::parser::ast::Pattern::Wildcard | crate::parser::ast::Pattern::Literal(_) => false,
    }
}

fn predicate_contains_shadowing_binder(expr: &Expr, operand: &str) -> bool {
    match expr {
        Expr::Paren(inner) => predicate_contains_shadowing_binder(inner, operand),
        Expr::Number(_)
        | Expr::Float(_)
        | Expr::StringLit(_)
        | Expr::Variable(_)
        | Expr::AtomRef { .. } => false,
        Expr::ArrayLit(elements) => elements
            .iter()
            .any(|element| predicate_contains_shadowing_binder(element, operand)),
        Expr::ArrayAccess(_, index) => predicate_contains_shadowing_binder(index, operand),
        Expr::BinaryOp(left, _, right) => {
            predicate_contains_shadowing_binder(left, operand)
                || predicate_contains_shadowing_binder(right, operand)
        }
        Expr::Block(stmt) => stmt_contains_shadowing_binder(stmt, operand),
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            predicate_contains_shadowing_binder(cond, operand)
                || stmt_contains_shadowing_binder(then_branch, operand)
                || stmt_contains_shadowing_binder(else_branch, operand)
        }
        Expr::Call(name, args) => {
            (matches!(name.as_str(), "forall" | "exists")
                && args
                    .first()
                    .is_some_and(|arg| matches!(arg, Expr::Variable(bound) if bound == operand)))
                || args
                    .iter()
                    .any(|arg| predicate_contains_shadowing_binder(arg, operand))
        }
        Expr::StructInit { fields, .. } => fields
            .iter()
            .any(|(_, value)| predicate_contains_shadowing_binder(value, operand)),
        Expr::FieldAccess(inner, _) => predicate_contains_shadowing_binder(inner, operand),
        Expr::Match { target, arms } => {
            predicate_contains_shadowing_binder(target, operand)
                || arms.iter().any(|arm| {
                    predicate_pattern_binds_operand(&arm.pattern, operand)
                        || arm.guard.as_ref().is_some_and(|guard| {
                            predicate_contains_shadowing_binder(guard, operand)
                        })
                        || stmt_contains_shadowing_binder(&arm.body, operand)
                })
        }
        Expr::Async { body } => stmt_contains_shadowing_binder(body, operand),
        Expr::Lambda { params, body, .. } => {
            params.iter().any(|param| param.name == operand)
                || stmt_contains_shadowing_binder(body, operand)
        }
        Expr::Await { expr } => predicate_contains_shadowing_binder(expr, operand),
        Expr::CallRef { callee, args } => {
            predicate_contains_shadowing_binder(callee, operand)
                || args
                    .iter()
                    .any(|arg| predicate_contains_shadowing_binder(arg, operand))
        }
        Expr::Perform { args, .. } => args
            .iter()
            .any(|arg| predicate_contains_shadowing_binder(arg, operand)),
        Expr::ChanSend { channel, value } => {
            predicate_contains_shadowing_binder(channel, operand)
                || predicate_contains_shadowing_binder(value, operand)
        }
        Expr::ChanRecv { channel } => predicate_contains_shadowing_binder(channel, operand),
    }
}

fn stmt_contains_shadowing_binder(stmt: &Stmt, operand: &str) -> bool {
    match stmt {
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            predicate_contains_shadowing_binder(value, operand)
        }
        Stmt::ArrayStore { index, value, .. } => {
            predicate_contains_shadowing_binder(index, operand)
                || predicate_contains_shadowing_binder(value, operand)
        }
        Stmt::Block(stmts, _) => stmts
            .iter()
            .any(|stmt| stmt_contains_shadowing_binder(stmt, operand)),
        Stmt::While {
            cond,
            invariant,
            decreases,
            body,
            ..
        } => {
            predicate_contains_shadowing_binder(cond, operand)
                || predicate_contains_shadowing_binder(invariant, operand)
                || decreases.as_ref().is_some_and(|decreases| {
                    predicate_contains_shadowing_binder(decreases, operand)
                })
                || stmt_contains_shadowing_binder(body, operand)
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
            stmt_contains_shadowing_binder(body, operand)
        }
        Stmt::TaskGroup { children, .. } => children
            .iter()
            .any(|child| stmt_contains_shadowing_binder(child, operand)),
        Stmt::Cancel { .. } => false,
        Stmt::Expr(expr, _) => predicate_contains_shadowing_binder(expr, operand),
    }
}

fn collect_refinement_free_identifiers(
    expr: &Expr,
    bound: &std::collections::HashSet<String>,
    enum_names: &std::collections::HashSet<String>,
    out: &mut Vec<String>,
) {
    fn add_identifier(
        name: &str,
        bound: &std::collections::HashSet<String>,
        out: &mut Vec<String>,
    ) {
        if name.contains("::") || matches!(name, "true" | "false") || bound.contains(name) {
            return;
        }
        if !out.iter().any(|existing| existing == name) {
            out.push(name.to_string());
        }
    }

    fn add_pattern_bindings(pattern: &Pattern, bound: &mut std::collections::HashSet<String>) {
        match pattern {
            Pattern::Variable(name) => {
                bound.insert(name.clone());
            }
            Pattern::Variant { fields, .. } => {
                for field in fields {
                    add_pattern_bindings(field, bound);
                }
            }
            Pattern::Wildcard | Pattern::Literal(_) => {}
        }
    }

    fn is_qualified_enum_path(expr: &Expr, enum_names: &std::collections::HashSet<String>) -> bool {
        let mut current = expr;
        let mut has_field_access = false;
        while let Expr::FieldAccess(inner, _) = current {
            has_field_access = true;
            current = inner;
        }
        has_field_access && matches!(current, Expr::Variable(name) if enum_names.contains(name))
    }

    fn visit_stmt(
        stmt: &Stmt,
        bound: &std::collections::HashSet<String>,
        enum_names: &std::collections::HashSet<String>,
        out: &mut Vec<String>,
    ) {
        match stmt {
            Stmt::Let { value, .. } => {
                collect_refinement_free_identifiers(value, bound, enum_names, out)
            }
            Stmt::Assign { var, value, .. } => {
                add_identifier(var.split('.').next().unwrap_or(var), bound, out);
                collect_refinement_free_identifiers(value, bound, enum_names, out);
            }
            Stmt::ArrayStore {
                array,
                index,
                value,
                ..
            } => {
                add_identifier(array.split('.').next().unwrap_or(array), bound, out);
                collect_refinement_free_identifiers(index, bound, enum_names, out);
                collect_refinement_free_identifiers(value, bound, enum_names, out);
            }
            Stmt::Block(stmts, _) => {
                let mut local = bound.clone();
                for stmt in stmts {
                    if let Stmt::Let { var, value, .. } = stmt {
                        collect_refinement_free_identifiers(value, &local, enum_names, out);
                        local.insert(var.clone());
                    } else {
                        visit_stmt(stmt, &local, enum_names, out);
                    }
                }
            }
            Stmt::While {
                cond,
                invariant,
                decreases,
                body,
                ..
            } => {
                collect_refinement_free_identifiers(cond, bound, enum_names, out);
                collect_refinement_free_identifiers(invariant, bound, enum_names, out);
                if let Some(decreases) = decreases {
                    collect_refinement_free_identifiers(decreases, bound, enum_names, out);
                }
                visit_stmt(body, bound, enum_names, out);
            }
            Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
                visit_stmt(body, bound, enum_names, out);
            }
            Stmt::TaskGroup { children, .. } => {
                for child in children {
                    visit_stmt(child, bound, enum_names, out);
                }
            }
            Stmt::Cancel { target, .. } => add_identifier(target, bound, out),
            Stmt::Expr(expr, _) => {
                collect_refinement_free_identifiers(expr, bound, enum_names, out)
            }
        }
    }

    match expr {
        Expr::Paren(inner) => collect_refinement_free_identifiers(inner, bound, enum_names, out),
        Expr::Number(_) | Expr::Float(_) | Expr::StringLit(_) | Expr::AtomRef { .. } => {}
        Expr::Variable(name) => add_identifier(name, bound, out),
        Expr::ArrayLit(elements) => {
            for element in elements {
                collect_refinement_free_identifiers(element, bound, enum_names, out);
            }
        }
        Expr::ArrayAccess(array, index) => {
            add_identifier(array.split('.').next().unwrap_or(array), bound, out);
            collect_refinement_free_identifiers(index, bound, enum_names, out);
        }
        Expr::BinaryOp(left, _, right) => {
            collect_refinement_free_identifiers(left, bound, enum_names, out);
            collect_refinement_free_identifiers(right, bound, enum_names, out);
        }
        Expr::Block(stmt) => visit_stmt(stmt, bound, enum_names, out),
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_refinement_free_identifiers(cond, bound, enum_names, out);
            visit_stmt(then_branch, bound, enum_names, out);
            visit_stmt(else_branch, bound, enum_names, out);
        }
        Expr::Call(name, args)
            if matches!(name.as_str(), "forall" | "exists") && args.len() == 4 =>
        {
            collect_refinement_free_identifiers(&args[1], bound, enum_names, out);
            collect_refinement_free_identifiers(&args[2], bound, enum_names, out);
            if let Expr::Variable(var) = &args[0] {
                let mut local = bound.clone();
                local.insert(var.clone());
                collect_refinement_free_identifiers(&args[3], &local, enum_names, out);
            } else {
                collect_refinement_free_identifiers(&args[0], bound, enum_names, out);
                collect_refinement_free_identifiers(&args[3], bound, enum_names, out);
            }
        }
        Expr::Call(name, args) => {
            let skip_callback = name == "call";
            for (index, arg) in args.iter().enumerate() {
                if skip_callback && index == 0 && matches!(arg, Expr::Variable(_)) {
                    continue;
                }
                collect_refinement_free_identifiers(arg, bound, enum_names, out);
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_refinement_free_identifiers(value, bound, enum_names, out);
            }
        }
        Expr::FieldAccess(inner, _) => {
            if !is_qualified_enum_path(expr, enum_names) {
                collect_refinement_free_identifiers(inner, bound, enum_names, out);
            }
        }
        Expr::Match { target, arms } => {
            collect_refinement_free_identifiers(target, bound, enum_names, out);
            for arm in arms {
                let mut local = bound.clone();
                add_pattern_bindings(&arm.pattern, &mut local);
                if let Some(guard) = &arm.guard {
                    collect_refinement_free_identifiers(guard, &local, enum_names, out);
                }
                visit_stmt(&arm.body, &local, enum_names, out);
            }
        }
        Expr::Async { body } => visit_stmt(body, bound, enum_names, out),
        Expr::Await { expr } => collect_refinement_free_identifiers(expr, bound, enum_names, out),
        Expr::CallRef { callee, args } => {
            if !matches!(callee.as_ref(), Expr::Variable(_) | Expr::AtomRef { .. }) {
                collect_refinement_free_identifiers(callee, bound, enum_names, out);
            }
            for arg in args {
                collect_refinement_free_identifiers(arg, bound, enum_names, out);
            }
        }
        Expr::Perform { args, .. } => {
            for arg in args {
                collect_refinement_free_identifiers(arg, bound, enum_names, out);
            }
        }
        Expr::Lambda { params, body, .. } => {
            let mut local = bound.clone();
            local.extend(params.iter().map(|param| param.name.clone()));
            visit_stmt(body, &local, enum_names, out);
        }
        Expr::ChanSend { channel, value } => {
            collect_refinement_free_identifiers(channel, bound, enum_names, out);
            collect_refinement_free_identifiers(value, bound, enum_names, out);
        }
        Expr::ChanRecv { channel } => {
            collect_refinement_free_identifiers(channel, bound, enum_names, out)
        }
    }
}

pub(crate) fn refined_predicate_to_z3<'a>(
    vc: &VCtx<'a>,
    decl: &crate::parser::ast::RefinedType,
    expr: &Expr,
    env: &mut Env<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> DynResult<'a> {
    let mut free_identifiers = Vec::new();
    let enum_names: std::collections::HashSet<String> =
        vc.module_env.enums.keys().cloned().collect();
    collect_refinement_free_identifiers(
        expr,
        &std::collections::HashSet::new(),
        &enum_names,
        &mut free_identifiers,
    );
    if let Some(identifier) = free_identifiers
        .into_iter()
        .find(|identifier| identifier != &decl.operand)
    {
        return Err(MumeiError::type_error(format!(
            "Refinement predicate of type '{}' references free identifier '{}'; only the operand '{}' may appear",
            decl.name, identifier, decl.operand
        )));
    }
    let base = vc.module_env.resolve_base_type(&decl._base_type);
    if fixed_width_int(&base).is_some_and(|ty| ty.0 == "u64")
        && refined_predicate_rebinds_operand(expr, &decl.operand)
    {
        return Err(MumeiError::type_error_at(
            format!(
                "Refinement predicate for '{}' rebinds its operand '{}'",
                decl.name, decl.operand
            ),
            decl.span.clone(),
        )
        .with_help("Rename the inner binding"));
    }
    let _guard = LocalIntTypeGuard::for_refined_operand(vc, decl);
    expr_to_z3(vc, expr, env, solver_opt)
}

#[cfg(test)]
mod refined_predicate_rebinding_tests {
    use super::{collect_refinement_free_identifiers, refined_predicate_rebinds_operand};
    use crate::parser::ast::{Expr, MatchArm, Pattern, Span, Stmt};
    use crate::parser::parse_expression;
    use std::collections::HashSet;

    fn free_identifiers(source: &str) -> Vec<String> {
        let expr = parse_expression(source);
        let mut free = Vec::new();
        let enum_names = HashSet::from(["Enum".to_string()]);
        collect_refinement_free_identifiers(&expr, &HashSet::new(), &enum_names, &mut free);
        free
    }

    #[test]
    fn free_identifier_collection_respects_quantifier_lambda_and_let_binders() {
        assert_eq!(free_identifiers("forall(i, 0, 1, i >= 0 && v > 0)"), ["v"]);
        assert!(free_identifiers("|i: i64| i >= 0").is_empty());
        assert_eq!(free_identifiers("{ let i = v; i >= 0 }"), ["v"]);
        assert_eq!(free_identifiers("len(v) <= 10"), ["v"]);
        assert!(free_identifiers("Enum::Variant() == Enum::Variant()").is_empty());
        assert!(free_identifiers("Enum::Variant == Enum::Variant").is_empty());
        assert_eq!(free_identifiers("Enum"), ["Enum"]);
    }

    #[test]
    fn finds_local_quantifier_and_match_bindings() {
        let local_binding = Expr::Block(Box::new(Stmt::Let {
            var: "v".to_string(),
            value: Box::new(Expr::Number(-1)),
            span: Span::default(),
        }));
        assert!(refined_predicate_rebinds_operand(&local_binding, "v"));

        let quantifier = Expr::Call(
            "forall".to_string(),
            vec![
                Expr::Variable("v".to_string()),
                Expr::Number(0),
                Expr::Number(1),
                Expr::Number(1),
            ],
        );
        assert!(refined_predicate_rebinds_operand(&quantifier, "v"));

        let match_binding = Expr::Match {
            target: Box::new(Expr::Number(0)),
            arms: vec![MatchArm {
                pattern: Pattern::Variant {
                    variant_name: "Some".to_string(),
                    fields: vec![Pattern::Variable("v".to_string())],
                },
                guard: None,
                body: Box::new(Stmt::Expr(Expr::Number(0), Span::default())),
            }],
        };
        assert!(refined_predicate_rebinds_operand(&match_binding, "v"));
    }
}

pub(crate) fn expr_to_z3<'a>(
    vc: &VCtx<'a>,
    expr: &Expr,
    env: &mut Env<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> DynResult<'a> {
    // Per-atom constraint budget: increment count and check limit.
    // This tracks the number of Z3 AST nodes generated, which correlates
    // with solver.assert() pressure and overall verification complexity.
    if solver_opt.is_some() {
        let atom_name = vc
            .current_atom
            .map(|a| a.name.as_str())
            .unwrap_or("<unknown>");
        check_constraint_budget(vc, atom_name)?;
    }

    let ctx = vc.ctx;
    match expr {
        Expr::Paren(inner) => expr_to_z3(vc, inner, env, solver_opt),
        Expr::Number(n) => Ok(Int::from_i64(ctx, *n).into()),
        Expr::Float(f) => {
            if vc.ieee754_f64 {
                // Opt-in IEEE 754: preserve the exact binary64 bit pattern.
                Ok(float_from_f64(ctx, *f).into())
            } else {
                // Default: exact-rational `Real` encoding.
                Ok(real_from_f64(ctx, *f).into())
            }
        }
        // Plan 9: String literal to Z3 String Sort
        Expr::StringLit(s) => Ok(Z3String::from_str(ctx, s).unwrap().into()),
        Expr::Variable(name) => {
            // Wire check_alive() into variable access: if the variable has been
            // consumed, report a use-after-consume error.
            if let Some(lctx_cell) = vc.linearity_ctx {
                if let Err(e) = lctx_cell.borrow_mut().check_alive(name) {
                    return Err(MumeiError::verification(format!(
                        "Linearity violation: {}",
                        e
                    )));
                }
            }
            // The parser represents `true` / `false` boolean literals as
            // Expr::Variable("true"|"false"). Without special handling they
            // would be resolved as Int consts via the unwrap_or_else fallback
            // below, which breaks `&&`/`||` operands. This also makes
            // strip_quantifiers' `forall(...)` → `true` substitution safe for
            // non-trusted atoms that mix forall with `&&`.
            if let Some(existing) = env.get(name) {
                return Ok(existing.clone());
            }
            if name == "result" && env.contains_key(&tuple_result_arity_key(name)) {
                return Err(MumeiError::verification(format!(
                    "{UNSUPPORTED_TUPLE_RESULT_INDEXING} bare result is unavailable for tuple return"
                )));
            }
            if name == "true" {
                return Ok(Bool::from_bool(ctx, true).into());
            }
            if name == "false" {
                return Ok(Bool::from_bool(ctx, false).into());
            }
            // P10-C: a bare uppercase identifier naming a nullary variant of a
            // finite ADT (e.g. `Red` for `enum Color { Red, Green, Blue }`)
            // constructs the datatype value.
            if name.chars().next().is_some_and(|c| c.is_uppercase()) {
                if let Some(value) = datatype::enum_ctor_apply(vc, env, name, &[], &[], solver_opt)?
                {
                    return Ok(value);
                }
            }
            Ok(Int::new_const(ctx, name.as_str()).into())
        }
        Expr::Call(name, args) => {
            if is_temporal_witness_predicate(name) {
                return temporal_witness_call_to_z3(vc, name, args, env, solver_opt);
            }
            match name.as_str() {
                // =============================================================
                // ensures / invariant 内の forall/exists 量化子サポート
                // =============================================================
                // forall(var, start, end, condition) → Z3 ∀ var ∈ [start, end). condition
                // exists(var, start, end, condition) → Z3 ∃ var ∈ [start, end). condition
                //
                // これにより ensures: forall(i, 0, result - 1, arr[i] <= arr[i+1])
                // のようなソート済み不変量を事後条件として記述・検証できる。
                "forall" | "exists" => {
                    if args.len() != 4 {
                        return Err(MumeiError::verification(format!(
                            "{}() requires exactly 4 arguments: (var, start, end, condition)",
                            name
                        )));
                    }
                    // 第1引数: 束縛変数名
                    let var_name = match &args[0] {
                        Expr::Variable(v) => v.clone(),
                        _ => {
                            return Err(MumeiError::verification(format!(
                                "{}(): first argument must be a variable name",
                                name
                            )))
                        }
                    };

                    // 第2引数: 範囲の開始
                    // Quantifier bounds stay `Int`-sorted under
                    // `--bitvec-i64`, bridging any `BV(64)` bound.
                    let start_z3 = as_int_like(&expr_to_z3(vc, &args[1], env, None)?).ok_or(
                        MumeiError::type_error(format!("{}(): start must be integer", name)),
                    )?;

                    // 第3引数: 範囲の終了
                    let end_z3 = as_int_like(&expr_to_z3(vc, &args[2], env, None)?).ok_or(
                        MumeiError::type_error(format!("{}(): end must be integer", name)),
                    )?;

                    // 束縛変数を一時的に env に追加して condition を評価
                    // Fresh const: a same-named outer variable must not be
                    // captured by the quantifier binder.
                    let bound_var = Int::fresh_const(ctx, var_name.as_str());
                    let old_val = env.insert(var_name.clone(), bound_var.clone().into());

                    let range_cond =
                        Bool::and(ctx, &[&bound_var.ge(&start_z3), &bound_var.lt(&end_z3)]);

                    vc.quantifier_binders
                        .borrow_mut()
                        .push((var_name.clone(), bound_var.clone().into()));
                    let marks = obligation_marks(vc);
                    let condition_res = expr_to_z3(vc, &args[3], env, None);
                    vc.quantifier_binders.borrow_mut().pop();
                    // Deferred checks inside the body only hold for binder
                    // values in range; outside it the binder is unconstrained.
                    rebind_deferred_obligations(vc, marks, &range_cond);

                    // 束縛変数を env から復元
                    if let Some(old) = old_val {
                        env.insert(var_name, old);
                    } else {
                        env.remove(&var_name);
                    }

                    let condition_z3 =
                        condition_res?
                            .as_bool()
                            .ok_or(MumeiError::type_error(format!(
                                "{}(): condition must be boolean",
                                name
                            )))?;

                    let quantifier_expr = if name == "forall" {
                        // ∀ var ∈ [start, end). condition
                        // Build E-matching patterns from `arr[idx]` accesses
                        // inside the condition. Mirrors the requires-side
                        // pattern extraction at L5950-5979 so that array-
                        // heavy `ensures` / loop invariants instantiate the
                        // same way as their requires counterparts. Each
                        // access uses `__z3_arr_<name>` when present so that
                        // patterns refer to the post-store array state seen
                        // by the body of the forall.
                        let mut scalar_pattern_asts: Vec<Dynamic> = Vec::new();
                        let mut pattern_groups: Vec<Vec<Dynamic>> = Vec::new();
                        // Re-bind the quantified variable while we lower
                        // index expressions so that they see `var_name`.
                        let var_re = match &args[0] {
                            Expr::Variable(v) => v.clone(),
                            _ => String::new(),
                        };
                        // A pattern must contain the bound variable; Z3 returns
                        // a null AST otherwise (e.g. `arr[i]` under an inner
                        // `forall j`), which the z3 crate turns into a panic.
                        let arr_accesses: Vec<(String, Expr)> = collect_array_accesses(&args[3])
                            .into_iter()
                            .filter(|(_, idx_expr)| expr_mentions_var(idx_expr, &var_re))
                            .collect();
                        let saved = if !var_re.is_empty() {
                            env.insert(var_re.clone(), bound_var.clone().into())
                        } else {
                            None
                        };
                        for (arr_name, idx_expr) in arr_accesses.iter() {
                            if let Ok(idx_z3) = expr_to_z3(vc, idx_expr, env, None) {
                                if let Some(idx_int) = idx_z3.as_int() {
                                    let element_type =
                                        super::z3_types::array_element_type_name(arr_name, vc);
                                    if let Some(leaves) = super::z3_types::struct_array_leaf_types(
                                        &element_type,
                                        vc.module_env,
                                    ) {
                                        for (path, leaf_type) in leaves {
                                            let leaf = super::z3_types::struct_array_leaf_array(
                                                vc, arr_name, &path, &leaf_type, env,
                                            );
                                            let term = leaf.select(&idx_int);
                                            if is_admissible_trigger(&term) {
                                                pattern_groups.push(vec![term]);
                                            }
                                        }
                                    } else {
                                        let term =
                                            z3_dynamic_array(vc, arr_name, env).select(&idx_int);
                                        if is_admissible_trigger(&term) {
                                            scalar_pattern_asts.push(term);
                                        }
                                    }
                                }
                            }
                        }
                        if !scalar_pattern_asts.is_empty() {
                            pattern_groups.push(scalar_pattern_asts);
                        }
                        // Restore env for var_re
                        if !var_re.is_empty() {
                            if let Some(old) = saved {
                                env.insert(var_re, old);
                            } else {
                                env.remove(&var_re);
                            }
                        }
                        let body = range_cond.implies(&condition_z3);
                        if pattern_groups.is_empty() {
                            z3::ast::forall_const(ctx, &[&bound_var], &[], &body)
                        } else {
                            let patterns: Vec<z3::Pattern> = pattern_groups
                                .iter()
                                .map(|group| {
                                    let pattern_refs: Vec<&dyn z3::ast::Ast> = group
                                        .iter()
                                        .map(|term| term as &dyn z3::ast::Ast)
                                        .collect();
                                    z3::Pattern::new(ctx, &pattern_refs)
                                })
                                .collect();
                            let pattern_refs: Vec<&z3::Pattern> = patterns.iter().collect();
                            z3::ast::forall_const(ctx, &[&bound_var], &pattern_refs, &body)
                        }
                    } else {
                        // ∃ var ∈ [start, end). condition
                        z3::ast::exists_const(
                            ctx,
                            &[&bound_var],
                            &[],
                            &Bool::and(ctx, &[&range_cond, &condition_z3]),
                        )
                    };

                    Ok(quantifier_expr.into())
                }
                "len" => {
                    if args.len() != 1 {
                        return Err(MumeiError::verification("len() takes exactly one argument"));
                    }
                    let arg = &args[0];
                    let val = expr_to_z3(vc, arg, env, solver_opt)?;
                    if val.as_string().is_some() {
                        // `len(s)` on a `Str` is the real sequence length —
                        // `len("abc") == 3` and `requires: len(s) == n` now
                        // constrain the actual content rather than a
                        // detached `len_s` symbol.
                        let ast = unsafe {
                            z3_sys::Z3_mk_seq_length(raw_z3_context(ctx), val.get_z3_ast())
                        };
                        return Ok(unsafe { Int::wrap(ctx, ast) }.into());
                    }
                    if let Expr::Variable(name) = arg {
                        // len(arr_name) → the tracked symbolic length,
                        // shared with requires clauses and bounds checks
                        // via `len_<name>`.
                        let tracked = env.contains_key(&format!("__z3_arr_{name}"))
                            || val.as_array().is_some()
                            || vc
                                .current_atom
                                .and_then(|atom| {
                                    atom.params
                                        .iter()
                                        .find(|p| p.name == *name)
                                        .and_then(|p| p.type_name.clone())
                                })
                                .map(|t| t.trim_start().starts_with('['))
                                .unwrap_or(false);
                        if tracked {
                            return Ok(array_len_value(ctx, env, name, vc.bitvec_i64, solver_opt));
                        }
                        return Err(MumeiError::type_error(format!(
                            "len() expects an array or string argument; `{name}` is neither"
                        )));
                    }
                    if val.as_array().is_some() {
                        // `len([1, 2, 3])` / `len(if c { … } else { … })` —
                        // structural length from the expression itself.
                        return Ok(tail_len_expr(vc, env, None, arg, &val, 0, solver_opt));
                    }
                    Err(MumeiError::type_error(
                        "len() expects an array or string argument",
                    ))
                }
                "sqrt" => {
                    // Z3 0.12 の Float/Real には sqrt メソッドがないため、
                    // シンボリック変数として扱い、sqrt(x) >= 0 の制約を付与。
                    // ソートは f64 エンコーディング（デフォルト Real、
                    // `--ieee754-f64` で Float）に合わせる。
                    let _val = expr_to_z3(vc, &args[0], env, solver_opt)?;
                    vc.reject_quantifier_dependent_call("sqrt", std::slice::from_ref(&_val))?;
                    // 呼び出しごとに一意な名前を使い、`sqrt(a) + sqrt(b)` の
                    // ような複数呼び出しが同一の Z3 変数に潰れないようにする。
                    static SQRT_COUNTER: std::sync::atomic::AtomicUsize =
                        std::sync::atomic::AtomicUsize::new(0);
                    let result_name = format!(
                        "sqrt_result_{}",
                        SQRT_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    );
                    if vc.ieee754_f64 {
                        let result = Float::new_const(ctx, result_name.as_str(), 11, 53);
                        if let Some(solver) = solver_opt {
                            let zero = Float::from_f64(ctx, 0.0);
                            solver.assert(&result.ge(&zero));
                        }
                        Ok(result.into())
                    } else {
                        let result = Real::new_const(ctx, result_name.as_str());
                        if let Some(solver) = solver_opt {
                            let zero = Real::from_real(ctx, 0, 1);
                            solver.assert(&result.ge(&zero));
                        }
                        Ok(result.into())
                    }
                }
                "cast_to_int" => {
                    // Z3 0.12 では Float->Int 直接変換がないため、シンボリック整数を返す
                    let _val = expr_to_z3(vc, &args[0], env, solver_opt)?;
                    vc.reject_quantifier_dependent_call(
                        "cast_to_int",
                        std::slice::from_ref(&_val),
                    )?;
                    Ok(Int::new_const(ctx, "cast_result").into())
                }
                // =============================================================
                // Built-in string constraint functions for requires/ensures
                // =============================================================
                // These are parsed as Expr::Call by the parser but need special
                // handling in Z3 to produce Bool constraints on Z3 String Sort.
                // =============================================================
                // P10-B: regex membership builtins → Z3 RegLan (str.in_re)
                // `matches(s, "pat")` / `match_regex` / `re_match` lower to
                // `s ∈ Σ*·re·Σ*` (Rust `is_match` substring semantics).
                // Patterns outside the supported fragment are a verification
                // error rather than a silently-dropped constraint: dropping a
                // requires clause only weakens the assumptions, but dropping
                // an ensures clause would report `verified` without checking
                // the postcondition (偽陰性).
                // =============================================================
                "matches" | "match_regex" | "re_match" => {
                    if args.len() != 2 {
                        return Err(MumeiError::verification(format!(
                            "{}() requires exactly 2 arguments: (string_var, \"regex\")",
                            name
                        )));
                    }
                    let str_val = expr_to_z3(vc, &args[0], env, solver_opt)?;
                    let Some(str_z3) = str_val.as_string() else {
                        // Non-string operand: mirror the permissive fallback of
                        // the other string builtins (type checking is out of
                        // scope here).
                        return Ok(Bool::from_bool(ctx, true).into());
                    };
                    let crate::parser::Expr::StringLit(pattern) = &args[1] else {
                        return Err(MumeiError::verification(format!(
                            "{}(): regex pattern must be a string literal",
                            name
                        )));
                    };
                    let Some(re) =
                        crate::verification::support::reglan::compile_search(ctx, pattern)
                    else {
                        return Err(MumeiError::verification(format!(
                            "{}(): regex pattern {:?} uses constructs outside the \
                             supported fragment (literals, '.', '*', '+', '?', \
                             '{{n,m}}', '[…]'/'[^…]', '|', '()' groups, outer '^'/'$' \
                             anchors, and \\d/\\w/\\s (ASCII-only), \\n/\\r/\\t/\
                             \\f/\\v/\\a, \\xHH escapes). Zero-width assertions, \
                             backreferences, lookarounds, Unicode escapes, and \
                             interior anchors must be delegated to Lean 4.",
                            name, pattern
                        )));
                    };
                    mark_string_constraints(vc);
                    Ok(str_z3.regex_matches(&re).into())
                }
                "starts_with" | "ends_with" | "contains" | "not_contains" => {
                    if args.len() != 2 {
                        return Err(MumeiError::verification(format!(
                            "{}() requires exactly 2 arguments: (string_var, \"pattern\")",
                            name
                        )));
                    }
                    let str_val = expr_to_z3(vc, &args[0], env, solver_opt)?;
                    let pattern_val = expr_to_z3(vc, &args[1], env, solver_opt)?;

                    // Both arguments must be Z3 String Sort
                    if let (Some(str_z3), Some(pat_z3)) =
                        (str_val.as_string(), pattern_val.as_string())
                    {
                        mark_string_constraints(vc);
                        let result: Bool = match name.as_str() {
                            "starts_with" => pat_z3.prefix(&str_z3),
                            "ends_with" => pat_z3.suffix(&str_z3),
                            "contains" => str_z3.contains(&pat_z3),
                            "not_contains" => str_z3.contains(&pat_z3).not(),
                            _ => unreachable!(),
                        };
                        Ok(result.into())
                    } else {
                        // Fallback: if operands are not strings, return true (no constraint).
                        // This handles cases where the variable hasn't been typed as Str
                        // (e.g., an i64 parameter). Str-typed parameters are correctly
                        // lowered to Z3 String Sort at parameter pre-registration, so this
                        // branch only fires for genuinely non-string variables.
                        //
                        // NOTE: This is a permissive fallback. If a user writes
                        // `not_contains(int_var, "..")` where int_var is i64, the constraint
                        // is silently dropped. This is acceptable because string constraint
                        // functions are only meaningful on Str-typed parameters, and using
                        // them on non-Str types is a user error that should ideally be
                        // caught by a type checker (not yet implemented for requires/ensures).
                        Ok(Bool::from_bool(ctx, true).into())
                    }
                }
                "is_empty" if !user_defines_callee(vc, name) => {
                    if args.len() != 1 {
                        return Err(MumeiError::verification(
                            "is_empty() requires exactly 1 argument: (string)",
                        ));
                    }
                    let val = expr_to_z3(vc, &args[0], env, solver_opt)?;
                    let str_z3 = val.as_string().ok_or_else(|| {
                        MumeiError::type_error("is_empty() expects a Str argument")
                    })?;
                    mark_string_constraints(vc);
                    let ast = unsafe {
                        z3_sys::Z3_mk_seq_length(raw_z3_context(ctx), str_z3.get_z3_ast())
                    };
                    let length = unsafe { Int::wrap(ctx, ast) };
                    Ok(length._eq(&Int::from_i64(ctx, 0)).into())
                }
                "index_of" if !user_defines_callee(vc, name) => {
                    if args.len() != 2 {
                        return Err(MumeiError::verification(
                            "index_of() requires exactly 2 arguments: (string, pattern)",
                        ));
                    }
                    let str_val = expr_to_z3(vc, &args[0], env, solver_opt)?;
                    let pattern_val = expr_to_z3(vc, &args[1], env, solver_opt)?;
                    let str_z3 = str_val
                        .as_string()
                        .ok_or_else(|| MumeiError::type_error("index_of() expects a Str string"))?;
                    let pattern_z3 = pattern_val.as_string().ok_or_else(|| {
                        MumeiError::type_error("index_of() expects a Str pattern")
                    })?;
                    mark_string_constraints(vc);
                    let offset = Int::from_i64(ctx, 0);
                    let ast = unsafe {
                        z3_sys::Z3_mk_seq_index(
                            raw_z3_context(ctx),
                            str_z3.get_z3_ast(),
                            pattern_z3.get_z3_ast(),
                            offset.get_z3_ast(),
                        )
                    };
                    Ok(unsafe { Int::wrap(ctx, ast) }.into())
                }
                "substr" if !user_defines_callee(vc, name) => {
                    if args.len() != 3 {
                        return Err(MumeiError::verification(
                            "substr() requires exactly 3 arguments: (string, start, count)",
                        ));
                    }
                    let str_val = expr_to_z3(vc, &args[0], env, solver_opt)?;
                    let start_val = expr_to_z3(vc, &args[1], env, solver_opt)?;
                    let count_val = expr_to_z3(vc, &args[2], env, solver_opt)?;
                    let str_z3 = str_val
                        .as_string()
                        .ok_or_else(|| MumeiError::type_error("substr() expects a Str string"))?;
                    let start = as_int_like(&start_val)
                        .ok_or_else(|| MumeiError::type_error("substr() expects an Int start"))?;
                    let count = as_int_like(&count_val)
                        .ok_or_else(|| MumeiError::type_error("substr() expects an Int count"))?;
                    mark_string_constraints(vc);
                    let ast = unsafe {
                        z3_sys::Z3_mk_seq_extract(
                            raw_z3_context(ctx),
                            str_z3.get_z3_ast(),
                            start.get_z3_ast(),
                            count.get_z3_ast(),
                        )
                    };
                    Ok(unsafe { Z3String::wrap(ctx, ast) }.into())
                }
                "char_at" if !user_defines_callee(vc, name) => {
                    if args.len() != 2 {
                        return Err(MumeiError::verification(
                            "char_at() requires exactly 2 arguments: (string, index)",
                        ));
                    }
                    let str_val = expr_to_z3(vc, &args[0], env, solver_opt)?;
                    let index_val = expr_to_z3(vc, &args[1], env, solver_opt)?;
                    let str_z3 = str_val
                        .as_string()
                        .ok_or_else(|| MumeiError::type_error("char_at() expects a Str string"))?;
                    let index = as_int_like(&index_val)
                        .ok_or_else(|| MumeiError::type_error("char_at() expects an Int index"))?;
                    mark_string_constraints(vc);
                    let ast = unsafe {
                        z3_sys::Z3_mk_seq_at(
                            raw_z3_context(ctx),
                            str_z3.get_z3_ast(),
                            index.get_z3_ast(),
                        )
                    };
                    Ok(unsafe { Z3String::wrap(ctx, ast) }.into())
                }
                _ => {
                    // `let f = |params| body; f(args)` — the callee name
                    // resolves to a `let`/`assign`-bound lambda: inline the
                    // body with each formal bound to the evaluated actual
                    // (call-by-value, a `let p_i = arg_i in body` step). A
                    // local binding shadows a same-named atom, matching how
                    // `Expr::Variable` shadows env entries.
                    let local_lambda = vc.local_lambdas.borrow().get(name).cloned();
                    if let Some(lambda) = local_lambda {
                        return apply_local_lambda(vc, name, &lambda, args, env, solver_opt);
                    }
                    // ユーザー定義関数呼び出し: 契約による検証（Compositional Verification）
                    // 呼び出し先の requires を現在のコンテキストで証明し、
                    // 成功すれば ensures を事実として追加する
                    //
                    // FQN dot-notation サポート:
                    // "math.add" → "math::add" として ModuleEnv から解決する。
                    // これにより `math.add(x, y)` と `math::add(x, y)` の両方が動作する。
                    let fqn_name = name.replace('.', "::");
                    let resolved_callee = vc
                        .module_env
                        .get_atom(name)
                        .cloned()
                        .or_else(|| vc.module_env.get_atom(&fqn_name).cloned());
                    if let Some(callee) = resolved_callee {
                        if callee.params.len() != args.len() {
                            return Err(MumeiError::verification(format!(
                                "Call to '{}': expected {} argument(s), got {}",
                                name,
                                callee.params.len(),
                                args.len()
                            )));
                        }
                        // 引数を評価
                        let mut arg_vals = Vec::new();
                        let mut arg_leaf_snapshots = Vec::new();
                        let mut earlier_array_aliases = Vec::new();
                        for (index, arg) in args.iter().enumerate() {
                            reject_later_array_argument_mutation(vc, arg, &earlier_array_aliases)?;
                            let value = expr_to_z3(vc, arg, env, solver_opt)?;
                            if value.as_array().is_some() {
                                let aliases = array_argument_aliases(arg, &value, env);
                                if !aliases.is_empty() {
                                    earlier_array_aliases.push(aliases);
                                }
                            }
                            let param_type = callee
                                .params
                                .get(index)
                                .and_then(|param| param.type_name.as_deref());
                            arg_leaf_snapshots.push(super::z3_types::snapshot_struct_array_leaves(
                                vc, env, arg, &value, param_type,
                            )?);
                            arg_vals.push(value);
                        }
                        let recursive_instance = vc.is_instantiating_contract(&callee.name);
                        // The unconstrained recursive result only weakens assumptions.
                        vc.reject_quantifier_dependent_call(name, &arg_vals)?;
                        reject_shared_array_mutating_call_args(
                            vc,
                            CalleeRef::Name(name.as_str()),
                            args,
                            &arg_vals,
                            env,
                        )?;
                        let cong = vc.callee_congruent(&callee);
                        let scc_internal = cong && scc_internal_call(vc, &callee);

                        // 仮引数名と実引数値の対応を構築
                        let mut call_env = env.clone();
                        for (i, param) in callee.params.iter().enumerate() {
                            if let Some(val) = arg_vals.get(i) {
                                call_env.insert(param.name.clone(), val.clone());
                                // 構造体引数: `<param>.<field>` が実引数のフィールドを指すようにする
                                alias_struct_fields(&mut call_env, &param.name, val);
                                // 配列引数: `__z3_arr_`/`len_`/要素型スロットを
                                // callee パラメータ名で配線（リテラルは len 実値）。
                                super::z3_types::wire_array_slots_with_struct_leaf_snapshot(
                                    vc,
                                    &param.name,
                                    args.get(i),
                                    val,
                                    &mut call_env,
                                    solver_opt,
                                    arg_leaf_snapshots.get(i).and_then(Option::as_ref),
                                )?;
                            }
                        }

                        // 呼び出し先の精緻型制約を call_env に適用
                        for (i, param) in callee.params.iter().enumerate() {
                            if let Some(type_name) = &param.type_name {
                                if let Some(refined) = vc.module_env.get_type(type_name) {
                                    // 実引数値を精緻型の述語変数に束縛して制約を検証
                                    if let Some(val) = arg_vals.get(i) {
                                        call_env.insert(refined.operand.clone(), val.clone());
                                    }
                                }
                            }
                        }

                        check_call_argument_domains(
                            vc,
                            solver_opt,
                            name,
                            &callee.params,
                            &arg_vals,
                            &mut call_env,
                        )?;

                        if cong {
                            if let Some(field_vals) =
                                struct_field_values_for_call(vc, &callee, &arg_vals, &call_env, env)
                            {
                                vc.reject_quantifier_dependent_call(name, &field_vals)?;
                            }
                        }

                        // Wire borrow()/consume() into call-site argument handling.
                        // For each callee parameter, if it is `ref`/`ref mut`, call borrow().
                        // If the callee has `consumed_params`, call consume() for the
                        // corresponding argument variable.
                        if let Some(lctx_cell) = vc.linearity_ctx {
                            // Handle ref/ref mut parameters → borrow
                            for (i, param) in callee.params.iter().enumerate() {
                                if param.is_ref || param.is_ref_mut {
                                    if let Some(Expr::Variable(arg_name)) = args.get(i) {
                                        let _ = lctx_cell.borrow_mut().borrow(arg_name, name);
                                    }
                                }
                            }
                            // Handle consumed params (either spelling) → consume
                            for consumed_name in &callee.consumed_names() {
                                if let Some(idx) =
                                    callee.params.iter().position(|p| p.name == *consumed_name)
                                {
                                    if let Some(Expr::Variable(arg_name)) = args.get(idx) {
                                        if let Err(e) = lctx_cell.borrow_mut().consume(arg_name) {
                                            return Err(MumeiError::verification(format!(
                                                "Linearity violation at call to '{}': {}",
                                                name, e
                                            )));
                                        }
                                    }
                                }
                            }
                        }

                        // Termination obligation for SCC-internal calls,
                        // checked before the callee requires check and before
                        // any ensures assumption is asserted.
                        if cong && !recursive_instance {
                            termination_obligation_at_call(
                                vc,
                                &callee,
                                &mut call_env,
                                env,
                                solver_opt,
                            )?;
                        }

                        // requires の検証: 呼び出し元のコンテキストで事前条件が満たされるか
                        let caller_requires =
                            crate::verification::caller_requires_obligation(&callee);
                        if caller_requires.trim() != "true" && !recursive_instance {
                            let _contract_guard = vc.enter_contract_instantiation(&callee);
                            if let Some(solver) = solver_opt {
                                let req_ast = parse_closed_contract_expression(
                                    &caller_requires,
                                    "callee precondition",
                                )?;
                                let req_z3 = super::z3_types::with_array_shared_markers_restored(
                                    &mut call_env,
                                    |env| {
                                        expr_to_z3_with_domain_obligations(vc, &req_ast, env, None)
                                    },
                                )?;
                                if let Some(req_bool) = req_z3.as_bool() {
                                    if vc.cover_obligations.is_some() {
                                        record_cover_obligation(vc, req_bool);
                                    } else {
                                        solver.push();
                                        // Branch guards are assumptions on this
                                        // path: a call under `if x >= 0` may rely
                                        // on `x >= 0` to satisfy the callee's
                                        // requires. Same pattern as the shift /
                                        // div-by-zero check sites.
                                        for cond in vc.path_cond_stack.borrow().iter() {
                                            solver.assert(cond);
                                        }
                                        solver.assert(&req_bool.not());
                                        if crate::verification::phase_artifacts::check(solver)
                                            == SatResult::Sat
                                        {
                                            // Extract counterexample: concrete argument values
                                            // that violate the callee's precondition.
                                            let raised_counterexample = if let Some(model) =
                                                solver.get_model()
                                            {
                                                let mut raised = RaisedCounterexample::default();
                                                let call_result_handles =
                                                    vc.call_result_symbols.borrow().clone();
                                                let source_bindings = callee
                                                    .params
                                                    .iter()
                                                    .map(|param| param.name.clone())
                                                    .collect::<HashSet<_>>();
                                                for (i, param) in callee.params.iter().enumerate() {
                                                    if let Some(arg_val) = arg_vals.get(i) {
                                                        let mut raised_value = raise_named_binding(
                                                            &model,
                                                            param.name.clone(),
                                                            &param.name,
                                                            arg_val,
                                                            param.type_name.as_deref(),
                                                            &call_env,
                                                            vc.module_env,
                                                        );
                                                        let references_abstract_result =
                                                            references_call_result(
                                                                arg_val,
                                                                &call_result_handles,
                                                            ) || param
                                                                .type_name
                                                                .as_deref()
                                                                .and_then(|type_name| {
                                                                    let base_type =
                                                                        resolve_source_base_type(
                                                                            type_name,
                                                                            vc.module_env,
                                                                        );
                                                                    vc.module_env
                                                                        .get_struct(&base_type)
                                                                })
                                                                .is_some_and(|struct_def| {
                                                                    struct_def.fields.iter().any(
                                                                        |field| {
                                                                            call_env
                                                                                .get(&struct_field_key(
                                                                                    &param.name,
                                                                                    &field.name,
                                                                                ))
                                                                                .is_some_and(
                                                                                    |field_value| {
                                                                                        references_call_result(
                                                                                            field_value,
                                                                                            &call_result_handles,
                                                                                        )
                                                                                    },
                                                                                )
                                                                        },
                                                                    )
                                                                });
                                                        if references_abstract_result {
                                                            let raw_rendering = model
                                                                .eval(arg_val, true)
                                                                .unwrap_or_else(|| arg_val.clone())
                                                                .to_string();
                                                            mark_abstract_call_result_unraisable(
                                                                &mut raised_value,
                                                                raw_rendering,
                                                            );
                                                        }
                                                        let argument_rendering =
                                                            arg_val.to_string();
                                                        let mut matching_names = env
                                                            .iter()
                                                            .filter_map(|(name, value)| {
                                                                (value.to_string()
                                                                    == argument_rendering)
                                                                    .then_some(name.as_str())
                                                            })
                                                            .collect::<Vec<_>>();
                                                        matching_names.sort_by_key(|name| {
                                                            (
                                                                !matches!(
                                                                    classify_solver_symbol(
                                                                        name,
                                                                        &source_bindings,
                                                                        vc.module_env,
                                                                    ),
                                                                    SolverSymbolClassification::Source(
                                                                        _
                                                                    )
                                                                ),
                                                                *name,
                                                            )
                                                        });
                                                        let solver_name = matching_names
                                                            .first()
                                                            .map(|name| (*name).to_string());
                                                        if solver_name
                                                            .as_deref()
                                                            .is_some_and(|name| name != param.name)
                                                        {
                                                            raised_value.solver_name = solver_name;
                                                        }
                                                        raised.values.push(raised_value);
                                                    }
                                                }
                                                raised.omitted_solver_symbols.extend(
                                                    env.keys()
                                                        .filter(|symbol| {
                                                            matches!(
                                                        classify_solver_symbol(
                                                            symbol,
                                                                &source_bindings,
                                                                vc.module_env,
                                                        ),
                                                        SolverSymbolClassification::Internal
                                                    )
                                                        })
                                                        .cloned(),
                                                );
                                                raised.omitted_solver_symbols.sort();
                                                raised.omitted_solver_symbols.dedup();
                                                Some(raised)
                                            } else {
                                                None
                                            };
                                            let ce_value = raised_counterexample
                                                .as_ref()
                                                .map(RaisedCounterexample::to_counterexample_json)
                                                .filter(|counterexample| {
                                                    !counterexample
                                                        .as_object()
                                                        .is_some_and(serde_json::Map::is_empty)
                                                });
                                            let provenance = raised_counterexample
                                                .as_ref()
                                                .map(RaisedCounterexample::provenance_json);
                                            solver.pop(1);
                                            return Err(MumeiError::verification(
                                                format!("Call to '{}': precondition (requires) not satisfied at call site", name)
                                            ).with_help("呼び出し元で事前条件を満たしていません。引数の制約を確認してください")
                                            .with_counterexample_provenance(ce_value, provenance)
                                            .with_verdict(crate::verification::VerdictKind::Rejected));
                                        }
                                        solver.pop(1);
                                    }
                                }
                            }
                        }

                        // Refined parameter types are part of the callee's
                        // contract: `need(x: Neg)` must reject `need(5)`. Each
                        // conjunct of the param's declaration chain is checked
                        // at the call site, the same shape as `requires`.
                        if !recursive_instance {
                            if let Some(solver) = solver_opt {
                                for param in &callee.params {
                                    let Some(type_name) = param.type_name.as_deref() else {
                                        continue;
                                    };
                                    if vc.module_env.get_type(type_name).is_none() {
                                        continue;
                                    }
                                    let chain = vc.module_env.refined_chain(type_name).map_err(
                                        |cycle| {
                                            MumeiError::type_error_at(cycle, callee.span.clone())
                                        },
                                    )?;
                                    let Some(arg_z3) = call_env.get(&param.name).cloned() else {
                                        continue;
                                    };
                                    for decl in chain {
                                        let mut local_env = call_env.clone();
                                        local_env.insert(decl.operand.clone(), arg_z3.clone());
                                        let pred_ast = parse_expression(&decl.predicate_raw);
                                        let Some(pred_bool) = refined_predicate_to_z3(
                                            vc,
                                            decl,
                                            &pred_ast,
                                            &mut local_env,
                                            None,
                                        )?
                                        .as_bool() else {
                                            continue;
                                        };
                                        if vc.cover_obligations.is_some() {
                                            record_cover_obligation(vc, pred_bool);
                                            continue;
                                        }
                                        solver.push();
                                        for cond in vc.path_cond_stack.borrow().iter() {
                                            solver.assert(cond);
                                        }
                                        solver.assert(&pred_bool.not());
                                        let refined_check =
                                            crate::verification::phase_artifacts::check(solver);
                                        solver.pop(1);
                                        if refined_check == SatResult::Sat {
                                            return Err(MumeiError::verification(
                                                format!("Call to '{}': refinement on parameter '{}' ({}) not satisfied at call site", name, param.name, decl.name)
                                            ).with_help("呼び出し元で引数の型制約を満たしていません。引数の値を確認してください")
                                            .with_verdict(crate::verification::VerdictKind::Rejected));
                                        }
                                        if refined_check == SatResult::Unknown {
                                            return Err(MumeiError::verification(
                                                format!("Call to '{}': refinement on parameter '{}' ({}) could not be decided", name, param.name, decl.name)
                                            ).with_verdict(crate::verification::VerdictKind::Inconclusive));
                                        }
                                    }
                                }
                            }
                        }

                        // Trait method param_constraints の検証:
                        // 呼び出し先がトレイトメソッドの実装である場合、param_constraints を
                        // 呼び出し元のコンテキストで検証する。
                        // get_traits_for_method で全候補を取得し、find_impl で callee の型に
                        // 対して実際にトレイトが impl されている候補のみ適用する。
                        if !recursive_instance {
                            if let Some(solver) = solver_opt {
                                let callee_type = callee
                                    .params
                                    .first()
                                    .and_then(|p| p.type_name.as_deref())
                                    .unwrap_or("i64");
                                let candidates = vc.module_env.get_traits_for_method(name);
                                // find_impl で正しいトレイトを絞り込む
                                let matched = candidates.iter().find(|(tn, _)| {
                                    vc.module_env.find_impl(tn, callee_type).is_some()
                                });
                                if let Some((_trait_name, trait_method)) = matched {
                                    for (i, constraint_opt) in
                                        trait_method.param_constraints.iter().enumerate()
                                    {
                                        if let Some(constraint) = constraint_opt {
                                            if let Some(arg_val) = arg_vals.get(i) {
                                                let param_name = callee
                                                    .params
                                                    .get(i)
                                                    .map(|p| p.name.as_str())
                                                    .unwrap_or("v");
                                                let concrete_constraint: String =
                                                    replace_constraint_placeholder(
                                                        constraint, param_name,
                                                    );
                                                let mut constraint_env: Env = env.clone();
                                                constraint_env.insert(
                                                    param_name.to_string(),
                                                    arg_val.clone(),
                                                );
                                                let constraint_ast =
                                                    parse_expression(&concrete_constraint);
                                                if let Ok(constraint_z3) = expr_to_z3(
                                                    vc,
                                                    &constraint_ast,
                                                    &mut constraint_env,
                                                    None,
                                                ) {
                                                    if let Some(constraint_bool) =
                                                        constraint_z3.as_bool()
                                                    {
                                                        if vc.cover_obligations.is_some() {
                                                            record_cover_obligation(
                                                                vc,
                                                                constraint_bool,
                                                            );
                                                        } else {
                                                            solver.push();
                                                            for cond in
                                                                vc.path_cond_stack.borrow().iter()
                                                            {
                                                                solver.assert(cond);
                                                            }
                                                            solver.assert(&constraint_bool.not());
                                                            if crate::verification::phase_artifacts::check(solver) == SatResult::Sat {
                                                            solver.pop(1);
                                                            return Err(MumeiError::verification(
                                                                format!(
                                                                    "Call to '{}': trait method parameter constraint '{}' not satisfied for argument {}",
                                                                    name, constraint, i
                                                                )
                                                            ).with_help(
                                                                "トレイトメソッドのパラメータ制約が満たされていません。引数の値を確認してください"
                                                            ).with_verdict(crate::verification::VerdictKind::Rejected));
                                                        }
                                                            solver.pop(1);
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // ensures からシンボリック結果を生成し、事後条件を事実として追加
                        static CALL_COUNTER: std::sync::atomic::AtomicUsize =
                            std::sync::atomic::AtomicUsize::new(0);
                        let call_id =
                            CALL_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        let result_name = format!("call_{}_{}", name, call_id);
                        vc.call_result_symbols
                            .borrow_mut()
                            .insert(result_name.clone());

                        // 戻り値型から Z3 ソートを選択する。明示的な `-> T` 注釈が
                        // あればそれを使い、なければ従来のヒューリスティック
                        // （f64 パラメータを含む呼び出し先は f64 を返す）に
                        // フォールバックする。ソートは呼び出し先の `result`
                        // エンコーディング（デフォルト Real、`--ieee754-f64` で
                        // Float）と一致させる。
                        let has_float = callee.params.iter().any(|p| {
                            p.type_name
                                .as_deref()
                                .map(|t| vc.module_env.resolve_base_type(t) == "f64")
                                .unwrap_or(false)
                        });
                        let result_type: Option<&str> = callee
                            .return_type
                            .as_deref()
                            .or(if has_float { Some("f64") } else { None });
                        let _result_array_type_guard =
                            LocalArrayElementTypeGuard::for_call_result(vc, result_type);
                        if let Some(type_name) = result_type {
                            super::z3_types::validate_enum_array_type(vc, type_name)?;
                        }
                        let rec_args = rec_fn_args(vc, &callee, &arg_vals, &call_env);
                        let use_rec_fn = cong
                            && callee_semantics_match_caller(vc, &callee)
                            && rec_args.is_some();
                        // Keep the expanded signature alive past the primary
                        // application: the projections a non-scalar result
                        // needs (`#len` for `[T]`, `#field.*` for structs)
                        // are declared over the same domain.
                        let rec_signature: Option<(Vec<Dynamic>, Vec<Sort>)> = if use_rec_fn {
                            let rec_args = rec_args.expect("recursive arguments checked above");
                            let domain = rec_args.iter().map(|v| v.get_sort()).collect();
                            Some((rec_args, domain))
                        } else {
                            None
                        };
                        let struct_return = result_type.and_then(|t| vc.module_env.get_struct(t));
                        let result_z3: Dynamic = if let Some((rec_args, domain)) = &rec_signature {
                            if struct_return.is_some() {
                                // A congruent struct result is carried
                                // field-wise: the opaque handle stays a
                                // per-call const (it is only ever observed
                                // through its fields) and each field is a
                                // projection UF bound below.
                                datatype::param_z3_value_for_vc(
                                    vc,
                                    result_name.as_str(),
                                    result_type,
                                )
                            } else {
                                let range = call_result_sort(vc, &callee);
                                let decl = vc.rec_fn(&callee, domain, &range);
                                let app_args: Vec<&dyn Ast> =
                                    rec_args.iter().map(|v| v as &dyn Ast).collect();
                                let application = decl.apply(&app_args);
                                if let Some(sink) = vc.recursion.unfold_sink.borrow_mut().as_mut() {
                                    sink.push((callee.name.clone(), arg_vals.clone()));
                                }
                                application
                            }
                        } else {
                            datatype::param_z3_value_for_vc(vc, result_name.as_str(), result_type)
                        };

                        // A widened-integer enum payload leaf
                        // (`u64`/`u32`/`i32`) keeps its declared width on a
                        // call result too — the result const is minted
                        // fresh here, so the param-time domain asserts
                        // cannot reach it.
                        if let (Some(solver), Some(bound)) =
                            (solver_opt, datatype::enum_value_leaf_domain(vc, &result_z3))
                        {
                            solver.assert(&bound);
                        }

                        // Array results: mint the result's `len` symbol up
                        // front and key it by the result array's ast — the
                        // callee's `ensures: len(result) == k` asserts on it,
                        // and `wire_array_slots`/`tail_len_expr` recover the
                        // same symbol for `let t = f(..)` so the guarantee
                        // reaches the caller instead of a fresh `len_t`.
                        let result_struct_arrays = if let Some(result_array) = result_z3.as_array()
                        {
                            super::z3_types::call_result_struct_arrays(
                                vc,
                                &result_array,
                                result_type,
                                &result_name,
                            )?
                        } else {
                            std::collections::HashMap::new()
                        };
                        let result_len: Option<Dynamic> = result_z3.as_array().map(|_| {
                            let len_sym: Dynamic = if let Some((rec_args, domain)) = &rec_signature
                            {
                                // Congruent call: an array value is its
                                // (contents, len) pair, so the result's
                                // length is a projection UF over the same
                                // domain — equal argument tuples yield
                                // equal contents AND equal lengths.
                                let len_sort =
                                    array_len_symbol(vc.ctx, "probe", vc.bitvec_i64).get_sort();
                                rec_fn_result_projection(
                                    vc,
                                    &callee,
                                    REC_FN_ARRAY_LEN_PROJECTION,
                                    domain,
                                    &len_sort,
                                    rec_args,
                                )
                            } else {
                                array_root_len_symbol(ctx, &result_name, vc.bitvec_i64)
                            };
                            // Array lengths are nonneg regardless of the
                            // callee's ensures — `array_len_value` asserts
                            // the same when a solver is in scope.
                            if let (Some(solver), Some(nn)) =
                                (solver_opt, nonneg_constraint(ctx, &len_sym))
                            {
                                solver.assert(&nn);
                            }
                            if rec_signature.is_none() {
                                env.insert(array_root_len_key(&result_name), len_sym.clone());
                            }
                            len_sym
                        });
                        if let Some(result_array) = result_z3.as_array() {
                            vc.call_result_lens.borrow_mut().insert(
                                super::z3_types::array_root_ast(&result_array) as usize,
                                super::context::CallResultArray {
                                    len: result_len.clone(),
                                    _keep: result_z3.clone(),
                                    struct_leaves: result_struct_arrays.clone(),
                                    struct_element_type: None,
                                },
                            );
                        }

                        // 構造体を返す呼び出し: 結果のフィールドをシンボル化し、
                        // 呼び出し先が保証する跨フィールド不変量を事実として仮定する
                        // （呼び出し先自身の検証で Invariant(result) は義務として課される）。
                        if let Some(sdef) = struct_return {
                            let result_fields: Vec<(String, Dynamic)> =
                                if let Some((rec_args, domain)) = &rec_signature {
                                    // Congruent call: each field is a projection
                                    // UF over the callee's expanded domain — the
                                    // result is compared field-wise, never by the
                                    // opaque handle.
                                    sdef.fields
                                        .iter()
                                        .map(|field| {
                                            let range = param_sort_probe(
                                                vc,
                                                &format!(
                                                    "__recfn_field_{}_{}",
                                                    callee.name, field.name
                                                ),
                                                Some(field.type_name.trim()),
                                            );
                                            (
                                                field.name.clone(),
                                                rec_fn_result_projection(
                                                    vc,
                                                    &callee,
                                                    &format!("field.{}", field.name),
                                                    domain,
                                                    &range,
                                                    rec_args,
                                                ),
                                            )
                                        })
                                        .collect()
                                } else {
                                    seed_struct_fields(vc, env, &result_name, sdef)
                                };
                            // The UF-path fields live under the per-call
                            // handle in `env` too, so `let t = f(..); t.x`
                            // projects to the shared congruent values.
                            if rec_signature.is_some() {
                                bind_struct_fields(env, &result_name, &result_fields);
                            }
                            bind_struct_fields(&mut call_env, &result_name, &result_fields);
                            bind_struct_fields(&mut call_env, "result", &result_fields);
                            // Nested result fields: `call_env` was cloned
                            // before `seed_struct_fields` wrote the deeper
                            // `__struct_<result>_<field>_<leaf>` keys to env,
                            // so rebind them under `result_<field>_*` —
                            // without this `result.inner.a` in the callee's
                            // ensures resolves to an unrelated fresh const
                            // and the caller never learns the guarantee.
                            // Deep leaves rebind too — a shallow bind
                            // leaves `result.inner.b.c` on stale keys.
                            for (name, value) in &result_fields {
                                let fields = nested_field_bindings(env, value);
                                if !fields.is_empty() {
                                    bind_struct_fields(
                                        &mut call_env,
                                        &format!("result_{name}"),
                                        &fields,
                                    );
                                }
                            }
                            // A direct read `f(1).x` cannot rebuild
                            // `call_<name>_<id>` — key the fields by the
                            // result ast so `FieldAccess` returns the
                            // projection instead of a shared `field_x`.
                            register_call_result_fields(vc, env, &result_z3, &result_fields);
                            if let (Some(solver), true) = (
                                solver_opt,
                                !recursive_instance && callee_semantics_match_caller(vc, &callee),
                            ) {
                                let _contract_guard = vc.enter_contract_instantiation(&callee);
                                if rec_signature.is_some() {
                                    // Result assumptions come from the same
                                    // builder the callee's body side uses
                                    // (`assume_struct_contract`, which lowers
                                    // field `where` constraints then
                                    // invariants) — never an ad-hoc subset.
                                    assume_struct_contract(
                                        vc,
                                        solver,
                                        sdef,
                                        &result_name,
                                        &result_fields,
                                        &call_env,
                                        None,
                                    )?;
                                } else {
                                    assume_struct_invariants(
                                        vc,
                                        solver,
                                        sdef,
                                        &result_name,
                                        &result_fields,
                                        &call_env,
                                        None,
                                    )?;
                                }
                            }
                        }

                        // Enum-typed fields inside a struct call result are
                        // fresh datatype consts — bound their widened
                        // integer leaves the same way params are bounded.
                        if let Some(solver) = solver_opt {
                            datatype::assert_enum_value_domains(vc, solver, env);
                        }

                        // `arr[i]` inside `ensures` means the POST-call
                        // element — havoc the call_env tracked slot of any
                        // `[T]` param the callee's body may store through,
                        // so a mutating callee's ensures can't smuggle in
                        // the caller's pre-call chain.
                        for param in callee.params.iter() {
                            if atom_stores_to_array(vc.module_env, &callee, &param.name) {
                                havoc_array_name(vc, &param.name, &mut call_env);
                            }
                        }

                        // ensures を事実として solver に追加（result を呼び出し結果に束縛）
                        //
                        // Equality Ensures Propagation:
                        // ensures 内に `result == expr` の形式の等式が含まれる場合、
                        // シンボリック result を具体的な式に直接束縛する。
                        // これにより `let x = increment(n);` で `x == n + 1` が
                        // 呼び出し元のコンテキストに伝播し、連鎖呼び出しの検証精度が向上する。
                        //
                        // 例: ensures: result == n + 1;
                        //   → call_env に result = call_increment_0 を挿入
                        //   → Z3 に call_increment_0 == n + 1 を assert
                        //   → 後続の `increment(x)` で x >= 1 だけでなく x == n + 1 が使える

                        // A refined return type is part of the callee's
                        // contract: the caller may assume every conjunct on
                        // the call's symbolic result. Same gates as the
                        // ensures assumption — caller/callee encodings must
                        // agree, and a congruent call assumes it only under
                        // the induction antecedent.
                        if !recursive_instance && callee_semantics_match_caller(vc, &callee) {
                            if let Some(return_type) = callee.return_type.as_deref() {
                                if vc.module_env.get_type(return_type).is_some() {
                                    let chain = vc.module_env.refined_chain(return_type).map_err(
                                        |cycle| {
                                            MumeiError::type_error_at(cycle, callee.span.clone())
                                        },
                                    )?;
                                    for decl in chain {
                                        let mut local_env = call_env.clone();
                                        local_env.insert(decl.operand.clone(), result_z3.clone());
                                        let pred_ast = parse_expression(&decl.predicate_raw);
                                        let Some(pred_bool) = refined_predicate_to_z3(
                                            vc,
                                            decl,
                                            &pred_ast,
                                            &mut local_env,
                                            None,
                                        )?
                                        .as_bool() else {
                                            continue;
                                        };
                                        if let Some(solver) = solver_opt {
                                            if use_rec_fn {
                                                let ante = congruent_ensures_antecedent(
                                                    vc,
                                                    &callee,
                                                    &caller_requires,
                                                    &mut call_env,
                                                    scc_internal,
                                                )?;
                                                solver.assert(&ante.implies(&pred_bool));
                                            } else {
                                                solver.assert(&pred_bool);
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        let caller_ensures = crate::verification::contract_view(
                            &callee,
                            crate::verification::ContractView::CallerEnsures,
                        );
                        if caller_ensures.trim() != "true"
                            && !recursive_instance
                            && callee_semantics_match_caller(vc, &callee)
                        {
                            let _contract_guard = vc.enter_contract_instantiation(&callee);
                            call_env.insert("result".to_string(), result_z3.clone());
                            for (path, leaf_array) in &result_struct_arrays {
                                call_env.insert(
                                    super::z3_types::struct_array_leaf_key("result", path),
                                    leaf_array.clone().into(),
                                );
                            }
                            if let Some(len_sym) = &result_len {
                                call_env.insert("__z3_arr_result".to_string(), result_z3.clone());
                                call_env.insert(array_len_key("result"), len_sym.clone());
                            }
                            // `ensures` reads post-write fields —
                            // freshen the callee's written composite
                            // params so its clauses don't assert on the
                            // caller's input-time slots. The recursion
                            // antecedent below must still see the input
                            // fields — keep a pre-havoc copy.
                            let mut pre_havoc_env = call_env.clone();
                            havoc_written_callee_fields(vc, &callee, &mut call_env);
                            let ens_ast = parse_expression(&caller_ensures);

                            if use_rec_fn {
                                let unfold_key = if solver_opt.is_some()
                                    && !recursive_instance
                                    && vc.recursion.unfold_sink.borrow().is_none()
                                    && caller_outside_scc(vc, &callee)
                                {
                                    constant_args(&arg_vals).map(|(_, key)| key)
                                } else {
                                    None
                                };
                                let unfold_guard =
                                    unfold_key.as_ref().map(|_| UnfoldSinkGuard::new(vc));
                                // Congruent recursive call: assume the callee's
                                // ensures as the induction hypothesis
                                // `CallerRequires => CallerEnsures`. SCC-internal
                                // calls additionally conjoin the path conditions
                                // under which the decrease was proved.
                                let ens_z3 = super::z3_types::with_array_shared_markers_restored(
                                    &mut call_env,
                                    |env| {
                                        expr_to_z3_with_domain_obligations(vc, &ens_ast, env, None)
                                    },
                                )?;
                                if let (Some(ens_bool), Some(solver)) =
                                    (ens_z3.as_bool(), solver_opt)
                                {
                                    let ante = congruent_ensures_antecedent(
                                        vc,
                                        &callee,
                                        &caller_requires,
                                        &mut pre_havoc_env,
                                        scc_internal,
                                    )?;
                                    solver.assert(&ante.implies(&ens_bool));
                                    if let (Some(unfold_key), Some(unfold_guard)) =
                                        (unfold_key.as_ref(), unfold_guard.as_ref())
                                    {
                                        let frontier = unfold_guard.take_frontier();
                                        let mut seen = std::collections::HashSet::new();
                                        seen.insert(constant_call_key(&callee.name, unfold_key));
                                        unfold_constant_calls(vc, env, frontier, seen, solver);
                                    }
                                }
                            } else {
                                // Equality ensures の特別処理:
                                // ensures が `result == expr` の形式の場合、
                                // expr を評価して result と等価であることを直接 assert する。
                                // これにより Z3 が等式を完全に活用できる。
                                let ens_z3 = super::z3_types::with_array_shared_markers_restored(
                                    &mut call_env,
                                    |env| {
                                        expr_to_z3_with_domain_obligations(vc, &ens_ast, env, None)
                                    },
                                )?;
                                if let Some(ens_bool) = ens_z3.as_bool() {
                                    if let Some(solver) = solver_opt {
                                        solver.assert(&ens_bool);
                                    }
                                }

                                // 追加: ensures 式が `result == expr` の形式かチェックし、
                                // 該当する場合は result のシンボリック値に対して
                                // 等式制約を明示的に追加する（Z3 の等式推論を強化）
                                if let Expr::BinaryOp(left, Op::Eq, right) = &ens_ast {
                                    if let Expr::Variable(ref var_name) = left.as_ref() {
                                        if var_name == "result" {
                                            // ensures: result == <expr> の場合
                                            // <expr> を call_env で評価し、result_z3 == eval(<expr>) を assert
                                            if let Ok(rhs_val) =
                                                super::z3_types::with_array_shared_markers_restored(
                                                    &mut call_env,
                                                    |env| expr_to_z3(vc, right, env, None),
                                                )
                                            {
                                                if let Some(solver) = solver_opt {
                                                    assert_result_equality(
                                                        vc, solver, &result_z3, &rhs_val,
                                                    );
                                                }
                                            }
                                        }
                                    }
                                    // ensures: <expr> == result の逆順もサポート
                                    if let Expr::Variable(ref var_name) = right.as_ref() {
                                        if var_name == "result" {
                                            if let Ok(lhs_val) =
                                                super::z3_types::with_array_shared_markers_restored(
                                                    &mut call_env,
                                                    |env| expr_to_z3(vc, left, env, None),
                                                )
                                            {
                                                if let Some(solver) = solver_opt {
                                                    assert_result_equality(
                                                        vc, solver, &result_z3, &lhs_val,
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }

                                // 複合 ensures（&& で結合された複数条件）内の等式も伝播
                                // ensures: result >= 0 && result == n + 1 のような場合
                                super::z3_types::with_array_shared_markers_restored(
                                    &mut call_env,
                                    |env| {
                                        propagate_equality_from_ensures(
                                            vc, &ens_ast, &result_z3, env, solver_opt,
                                        )
                                    },
                                )?;
                            }
                        }

                        if let Some(solver) = solver_opt {
                            assert_temporal_effect_transition(
                                vc,
                                solver,
                                &callee.name,
                                &arg_vals,
                                &result_z3,
                            )?;
                        }

                        // Release borrows after call returns.
                        // Once the callee has finished, ref/ref mut borrows are released
                        // so the owner can be consumed or re-borrowed.
                        if let Some(lctx_cell) = vc.linearity_ctx {
                            for (i, param) in callee.params.iter().enumerate() {
                                if param.is_ref || param.is_ref_mut {
                                    if let Some(Expr::Variable(arg_name)) = args.get(i) {
                                        lctx_cell.borrow_mut().release_borrow(arg_name, name);
                                    }
                                }
                            }
                        }

                        // =============================================================
                        // Subsumption Check: atom_ref argument vs contract ensures
                        // =============================================================
                        // When a callee parameter has either callback contract
                        // clause and the corresponding argument is
                        // atom_ref(concrete_name), verify both subsumption
                        // directions. Fail closed when either implication cannot
                        // be established.
                        if let Some(solver) = solver_opt {
                            for (i, param) in callee.params.iter().enumerate() {
                                if param.fn_contract_ensures.is_some()
                                    || param.fn_contract_requires.is_some()
                                {
                                    if let Some(Expr::AtomRef {
                                        name: ref concrete_name,
                                    }) = args.get(i)
                                    {
                                        if let Some(concrete_atom) =
                                            vc.module_env.get_atom(concrete_name).cloned()
                                        {
                                            check_contract_subsumption(
                                                vc,
                                                &concrete_atom,
                                                param
                                                    .fn_contract_ensures
                                                    .as_deref()
                                                    .unwrap_or("true"),
                                                param.fn_contract_requires.as_deref(),
                                                name,
                                                &param.name,
                                                solver,
                                                ctx,
                                            )?;
                                        }
                                    }
                                }
                            }
                        }

                        // Taint Analysis: 呼び出し先が unverified の場合、
                        // 戻り値を __tainted_ マーカーで汚染済みとしてマークする。
                        if callee.trust_level == TrustLevel::Unverified {
                            let taint_key = format!("__tainted_{}", result_name);
                            let taint_marker = Bool::from_bool(ctx, true);
                            env.insert(taint_key, taint_marker.into());
                        }

                        // The callee may have stored through a `[T]` arg's
                        // data pointer — havoc the caller-side tracked chain
                        // (and its aliases) for args whose callee param is
                        // mutated, so post-call reads can't claim pre-call
                        // elements.
                        havoc_array_args(vc, &callee, args, env);

                        Ok(result_z3)
                    } else {
                        // P10-C: `Enum::Variant(args)` / `Variant(args)`
                        // constructs a finite-ADT value through the Z3
                        // datatype constructor.
                        if name.contains("::") || vc.module_env.find_enum_by_variant(name).is_some()
                        {
                            let enum_ctor = datatype::resolve_enum_ctor(vc, name)?;
                            let mut ctor_args = Vec::with_capacity(args.len());
                            let mut payload_snapshots = Vec::with_capacity(args.len());
                            for (i, arg) in args.iter().enumerate() {
                                let arg_z3 = expr_to_z3(vc, arg, env, solver_opt)?;
                                if arg_z3.as_array().is_some() {
                                    mark_array_value_sources_shared(vc, env, arg, &arg_z3)?;
                                }
                                let payload_snapshot = enum_ctor
                                    .and_then(|(enum_def, idx)| enum_def.variants.get(idx))
                                    .and_then(|variant| variant.field_types.get(i))
                                    .and_then(|field_type| {
                                        let field_type =
                                            datatype::payload_field_type_name(field_type);
                                        let field_leaves =
                                            datatype::field_leaf_types(&field_type, vc.module_env)?;
                                        if datatype::array_payload_element_type(&field_leaves)
                                            .is_some()
                                        {
                                            let array = match arg {
                                                Expr::Variable(name) => env
                                                    .get(&format!("__z3_arr_{name}"))
                                                    .and_then(|value| value.as_array())
                                                    .map(Into::into)
                                                    .unwrap_or_else(|| arg_z3.clone()),
                                                _ => arg_z3.clone(),
                                            };
                                            let array_ast = array.as_array()?;
                                            let len = super::z3_types::array_len_for_expr(
                                                vc,
                                                env,
                                                Some(arg),
                                                &array,
                                                solver_opt,
                                            );
                                            Some(datatype::PayloadSnapshot {
                                                leaves: vec![array_ast.into(), len],
                                            })
                                        } else if datatype::struct_array_payload_element_type(
                                            &field_type,
                                            vc.module_env,
                                        )
                                        .is_some()
                                        {
                                            datatype::struct_array_payload_leaves(
                                                vc,
                                                env,
                                                &field_type,
                                                arg,
                                                &arg_z3,
                                                solver_opt,
                                            )
                                            .ok()?
                                            .map(|leaves| datatype::PayloadSnapshot { leaves })
                                        } else if vc.module_env.structs.contains_key(
                                            &vc.module_env.resolve_base_type(&field_type),
                                        ) {
                                            datatype::flattened_value_leaves(
                                                vc,
                                                env,
                                                &arg_z3,
                                                &field_leaves,
                                                solver_opt,
                                            )
                                            .map(|leaves| datatype::PayloadSnapshot { leaves })
                                        } else {
                                            None
                                        }
                                    });
                                ctor_args.push(arg_z3);
                                payload_snapshots.push(payload_snapshot);
                            }
                            // A payload argument must satisfy the declared
                            // field type — `Chan::Tcp(0)` with `p: Port`
                            // (`v in 1..=65535`) is a rejected construction,
                            // not a value a later match may assume the
                            // refined bounds on (the same contract a caller
                            // satisfies for a refined callee parameter).
                            if let (Some(solver), Some((enum_def, vidx))) = (solver_opt, enum_ctor)
                            {
                                let variant_def = &enum_def.variants[vidx];
                                for (i, arg_z3) in ctor_args.iter().enumerate() {
                                    let Some(field_ty) = variant_def.field_types.get(i) else {
                                        continue;
                                    };
                                    let Some(refined) =
                                        vc.module_env.get_type(field_ty.name.as_str())
                                    else {
                                        continue;
                                    };
                                    let chain = vc
                                        .module_env
                                        .refined_chain(&refined.name)
                                        .map_err(|cycle| {
                                            MumeiError::type_error_at(cycle, refined.span.clone())
                                        })?;
                                    for decl in chain {
                                        let mut local_env = env.clone();
                                        local_env.insert(decl.operand.clone(), arg_z3.clone());
                                        let predicate_ast = parse_expression(&decl.predicate_raw);
                                        let Some(pred_bool) = refined_predicate_to_z3(
                                            vc,
                                            decl,
                                            &predicate_ast,
                                            &mut local_env,
                                            None,
                                        )?
                                        .as_bool() else {
                                            continue;
                                        };
                                        if vc.cover_obligations.is_some() {
                                            record_cover_obligation(vc, pred_bool);
                                            continue;
                                        }
                                        solver.push();
                                        for cond in vc.path_cond_stack.borrow().iter() {
                                            solver.assert(cond);
                                        }
                                        solver.assert(&pred_bool.not());
                                        let refined_check =
                                            crate::verification::phase_artifacts::check(solver);
                                        solver.pop(1);
                                        if refined_check == SatResult::Sat {
                                            return Err(MumeiError::verification(
                                                format!("Construction of '{name}': refinement on field '{}' ({}) not satisfied", variant_def.fields.get(i).cloned().unwrap_or_else(|| i.to_string()), decl.name)
                                            ).with_help("enum を構築する引数が宣言されたフィールド型の制約を満たしていません。引数の値を確認してください")
                                            .with_verdict(crate::verification::VerdictKind::Rejected));
                                        }
                                        if refined_check == SatResult::Unknown {
                                            return Err(MumeiError::verification(
                                                format!("Construction of '{name}': refinement on field '{}' ({}) could not be decided", variant_def.fields.get(i).cloned().unwrap_or_else(|| i.to_string()), decl.name)
                                            ).with_verdict(crate::verification::VerdictKind::Inconclusive));
                                        }
                                    }
                                }
                            }
                            if let Some(value) = datatype::enum_ctor_apply(
                                vc,
                                env,
                                name,
                                &ctor_args,
                                &payload_snapshots,
                                solver_opt,
                            )? {
                                return Ok(value);
                            }
                            // A resolved enum+variant whose construction still
                            // failed (arity mismatch, or the enum is not a
                            // finite ADT) must not degrade to the
                            // clause-skipping "Unknown function" path — that
                            // would silently drop the contract clause.
                            // Resolve `E` directly for qualified `E::V` (the
                            // prelude's generic `Option`/`Result`/`List`
                            // variants can shadow a user enum in a
                            // name-only scan); bare `V` resolved unambiguously
                            // already inside `enum_ctor_apply`.
                            let owner = if let Some((qual, _)) = name.split_once("::") {
                                vc.module_env.get_enum(qual)
                            } else {
                                vc.module_env
                                    .resolve_variant_owner_by_hint(name, None)
                                    .ok()
                                    .flatten()
                            };
                            if let Some(ed) = owner {
                                let variant = name.rsplit("::").next().unwrap_or(name);
                                match ed.variants.iter().find(|v| v.name == variant) {
                                    Some(vdef) => {
                                        let arity = vdef.fields.len();
                                        if arity == args.len() {
                                            return Err(MumeiError::verification(format!(
                                                "Enum constructor '{name}' is not constructible in specs: enum '{}' is not a finite ADT (recursive, generic, empty, or non-scalar payload — it keeps the Int-tag encoding; use the tag literal or a `match` discriminant)",
                                                ed.name
                                            )));
                                        }
                                        return Err(MumeiError::verification(format!(
                                            "Enum constructor '{name}' takes {arity} payload arg(s), got {}",
                                            args.len()
                                        )));
                                    }
                                    None if name.contains("::") => {
                                        // `E` is a known enum but `V` is not
                                        // one of its variants — a typo must
                                        // not be silently dropped.
                                        return Err(MumeiError::verification(format!(
                                            "Enum '{}' has no variant named '{variant}'",
                                            ed.name
                                        )));
                                    }
                                    None => {}
                                }
                            }
                        }
                        Err(MumeiError::verification(format!(
                            "Unknown function: {}",
                            name
                        )))
                    }
                }
            }
        }
        Expr::ArrayLit(elements) => {
            let mut literal_enum_type: Option<String> = None;
            let mut literal_struct_type: Option<String> = None;
            let mut has_non_struct_element = false;
            for element in elements {
                if let Some(enum_name) = datatype::infer_expr_enum_name(vc, element) {
                    super::z3_types::validate_enum_array_element_type(vc, &enum_name)?;
                    if literal_enum_type
                        .as_ref()
                        .is_some_and(|known| known != &enum_name)
                    {
                        return Err(MumeiError::type_error(
                            "array literal elements must have the same enum datatype",
                        ));
                    }
                    literal_enum_type = Some(enum_name);
                }
                if let Some(struct_name) = struct_array_expr_type(vc, element) {
                    if literal_struct_type
                        .as_ref()
                        .is_some_and(|known| known != &struct_name)
                    {
                        return Err(MumeiError::type_error(
                            "array literal elements must have the same struct type",
                        ));
                    }
                    literal_struct_type = Some(struct_name);
                } else {
                    has_non_struct_element = true;
                }
            }
            if literal_struct_type.is_some() && has_non_struct_element {
                return Err(MumeiError::type_error(
                    "array literal cannot mix struct and non-struct elements",
                ));
            }
            // `[e0, e1, …]` lowers to an `Int -> Elem` Z3 array built by a
            // `store` chain over a fresh constant base. The literal has no
            // name of its own: the `let`/`assign` binding that receives it
            // (stmt.rs) records `__z3_arr_<var>`/`len_<var>` and the element
            // type so reads, stores, and `len()` resolve against this chain.
            static ARRAY_LIT_UID: std::sync::atomic::AtomicUsize =
                std::sync::atomic::AtomicUsize::new(0);
            let uid = ARRAY_LIT_UID.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let name = format!("__array_lit_{uid}");
            let mut elem_dynamics = Vec::with_capacity(elements.len());
            for element in elements {
                elem_dynamics.push(expr_to_z3(vc, element, env, solver_opt)?);
            }
            if let Some(struct_type) = literal_struct_type {
                super::z3_types::validate_array_element_type(vc, &struct_type)?;
                let leaves = super::z3_types::struct_array_leaf_types(&struct_type, vc.module_env)
                    .ok_or_else(|| {
                        MumeiError::type_error(format!(
                            "array element struct '{struct_type}' must have only scalar leaves"
                        ))
                    })?;
                let mut carrier =
                    super::z3_types::z3_array_for_sort(ctx, &name, ArrayElementSort::Struct)?;
                for (index, element) in elem_dynamics.iter().enumerate() {
                    let element = as_int_like(element).ok_or_else(|| {
                        MumeiError::type_error(format!(
                            "array literal element is not a struct value of type '{struct_type}'"
                        ))
                    })?;
                    carrier = carrier.store(&Int::from_i64(ctx, index as i64), &element);
                }
                let all_leaf_values = elem_dynamics
                    .iter()
                    .map(|element| {
                        struct_array_leaf_values_from_value(vc, env, element, &struct_type)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                for (path, leaf_type) in leaves {
                    let key = super::z3_types::struct_array_leaf_key(&name, &path);
                    let mut leaf_array = super::z3_types::z3_fresh_array_for_range(
                        ctx,
                        &key,
                        &super::z3_types::z3_element_sort(vc, &leaf_type),
                    );
                    let leaf_sort = super::z3_types::array_element_sort_from_type(
                        &leaf_type,
                        vc.module_env,
                        vc.ieee754_f64,
                    );
                    for (index, element_leaves) in all_leaf_values.iter().enumerate() {
                        let value = element_leaves
                            .iter()
                            .find(|(leaf_path, _, _)| leaf_path == &path)
                            .map(|(_, _, value)| value)
                            .ok_or_else(|| {
                                MumeiError::type_error(format!(
                                    "struct array element is missing leaf '{path}'"
                                ))
                            })?;
                        let value =
                            super::z3_types::coerce_to_array_elem_sort(vc, leaf_sort, value)?;
                        leaf_array = leaf_array.store(&Int::from_i64(ctx, index as i64), &value);
                    }
                    env.insert(key, leaf_array.into());
                }
                return Ok(carrier.into());
            }
            // Range sort = the widest element sort across the literal. Z3
            // numerals built from whole-valued floats (`1.0` → `Int`) make a
            // first-element-only probe choose the wrong sort, so scan them
            // all: Float > Real > Int; Bool and Str literals must each be
            // uniformly of their own sort. `Array` elements (`[[T]]`) stay
            // unsupported — they would need an `Array` range sort.
            let mut has_float = false;
            let mut has_real = false;
            let mut has_int = false;
            let mut has_bool = false;
            let mut has_str = false;
            let mut enum_sort = None;
            for elem in &elem_dynamics {
                match elem.get_sort().kind() {
                    z3::SortKind::Int | z3::SortKind::BV => has_int = true,
                    z3::SortKind::Real => has_real = true,
                    z3::SortKind::FloatingPoint => has_float = true,
                    z3::SortKind::Bool => has_bool = true,
                    z3::SortKind::Seq => has_str = true,
                    z3::SortKind::Datatype => {
                        let sort = elem.get_sort();
                        if enum_sort.as_ref().is_some_and(|known| known != &sort) {
                            return Err(MumeiError::type_error(
                                "array literal elements must have the same enum datatype",
                            ));
                        }
                        enum_sort = Some(sort);
                    }
                    other => {
                        return Err(MumeiError::type_error(format!(
                            "array literal element type {other:?} is unsupported \
                             (only i64/f64/bool/Str elements can be lowered; \
                             nested arrays are not supported)"
                        )))
                    }
                }
            }
            if let Some(enum_sort) = enum_sort {
                if has_float || has_real || has_int || has_bool || has_str {
                    return Err(MumeiError::type_error(
                        "array literal mixes enum and non-enum elements",
                    ));
                }
                super::z3_types::validate_enum_array_element_type(vc, &enum_sort.to_string())?;
                let mut arr = super::z3_types::z3_array_for_range(ctx, &name, &enum_sort);
                for (i, elem) in elem_dynamics.iter().enumerate() {
                    if elem.get_sort() != enum_sort {
                        return Err(MumeiError::type_error(
                            "array literal elements must have the same enum datatype",
                        ));
                    }
                    arr = arr.store(&Int::from_i64(ctx, i as i64), elem);
                }
                return Ok(arr.into());
            }
            if literal_enum_type.is_some() {
                return Err(MumeiError::type_error(
                    "enum array literal elements did not resolve to one datatype sort",
                ));
            }
            let range_sort = if has_str {
                if has_float || has_real || has_int || has_bool {
                    return Err(MumeiError::type_error(
                        "array literal mixes string and non-string elements",
                    ));
                }
                ArrayElementSort::Str
            } else if has_float {
                ArrayElementSort::Float
            } else if has_real {
                if has_bool {
                    return Err(MumeiError::type_error(
                        "array literal mixes bool and numeric elements",
                    ));
                }
                ArrayElementSort::Real
            } else if has_int {
                if has_bool {
                    return Err(MumeiError::type_error(
                        "array literal mixes bool and integer elements",
                    ));
                }
                ArrayElementSort::Int
            } else if has_bool {
                ArrayElementSort::Bool
            } else {
                unreachable!("non-empty array literal has an element sort")
            };
            let mut arr = z3_array_for_sort(ctx, &name, range_sort)?;
            for (i, elem) in elem_dynamics.iter().enumerate() {
                let idx = Int::from_i64(ctx, i as i64);
                let coerced = coerce_to_array_elem_sort(vc, range_sort, elem)?;
                arr = arr.store(&idx, &coerced);
            }
            Ok(arr.into())
        }
        Expr::ArrayAccess(name, index_expr) => {
            if name == "result" {
                if let Some(arity) = env
                    .get(&tuple_result_arity_key(name))
                    .and_then(|value| value.as_int())
                    .and_then(|value| value.as_i64())
                {
                    let Expr::Number(index) = &**index_expr else {
                        return Err(MumeiError::verification(format!(
                            "{UNSUPPORTED_TUPLE_RESULT_INDEXING} index must be a non-negative integer constant"
                        )));
                    };
                    if *index < 0 || *index >= arity {
                        return Err(MumeiError::verification(format!(
                            "{UNSUPPORTED_TUPLE_RESULT_INDEXING} index is out of range"
                        )));
                    }
                    let key = tuple_result_component_key(name, *index as usize);
                    return env.get(&key).cloned().ok_or_else(|| {
                        MumeiError::verification(format!(
                            "{UNSUPPORTED_TUPLE_RESULT_INDEXING} component is unavailable"
                        ))
                    });
                }
            }
            let idx_value = expr_to_z3(vc, index_expr, env, solver_opt)?;
            // Array indices stay `Int`-sorted even under `--bitvec-i64`, so an
            // `i64` index lowered to `BV(64)` crosses the documented signed
            // bit-vector/integer bridge here.
            let idx =
                as_int_like(&idx_value).ok_or(MumeiError::type_error("Index must be integer"))?;

            let element_type = super::z3_types::array_element_type_name(name, vc);
            super::z3_types::validate_array_element_type(vc, &element_type)?;
            if super::z3_types::array_element_sort_from_type(
                &element_type,
                vc.module_env,
                vc.ieee754_f64,
            ) == ArrayElementSort::Struct
            {
                return Err(MumeiError::type_error(
                    "bare struct array elements are only supported as a local binding or scalar field access",
                ));
            }
            let array = z3_dynamic_array(vc, name, env);

            // 配列名に紐づく長さシンボルを使った境界チェック
            if let Some(solver) = solver_opt {
                let len = array_len_value(ctx, env, name, vc.bitvec_i64, Some(solver));
                let safe = index_in_bounds(ctx, &idx_value, &len)
                    .ok_or(MumeiError::type_error("Index must be integer"))?;
                solver.push();
                for cond in vc.path_cond_stack.borrow().iter() {
                    solver.assert(cond);
                }
                solver.assert(&safe.not());
                if crate::verification::phase_artifacts::check(solver) == SatResult::Sat {
                    solver.pop(1);
                    return Err(MumeiError::verification(format!(
                        "Potential Out-of-Bounds on '{}' (index may be < 0 or >= len_{})",
                        name, name
                    ))
                    .with_help(
                        "requires にインデックスの範囲制約 (0 <= idx < len) を追加してください",
                    )
                    .with_verdict(crate::verification::VerdictKind::Rejected));
                }
                solver.pop(1);
            }
            let selected = array.select(&idx);
            if let Some(solver) = solver_opt {
                let bound = datatype::enum_value_leaf_domain(vc, &selected)
                    .or_else(|| datatype::scalar_int_leaf_domain(vc, &element_type, &selected));
                if let Some(bound) = bound {
                    solver.assert(&bound);
                }
            }
            Ok(selected)
        }
        Expr::BinaryOp(left, op, right) => {
            if matches!(op, Op::Pow) {
                if let (Expr::Number(base), Expr::Number(exponent)) = (&**left, &**right) {
                    if *exponent < 0 {
                        return Err(unsupported_exponentiation_error());
                    }
                    let folded = pow_i64_to_decimal_string(*base, *exponent as u64);
                    return Ok(Int::from_str(ctx, &folded)
                        .ok_or_else(|| {
                            MumeiError::verification(format!(
                                "Failed to build integer literal from {}",
                                folded
                            ))
                        })?
                        .into());
                }
                return Err(unsupported_exponentiation_error());
            }

            // Short-circuiting connectives: the right operand is only
            // evaluated when the left one allows it, so it is lowered under the
            // guard as a path condition. That makes partiality checks (division
            // by zero) respect guards like `b == 0 || a / b == c`, exactly as
            // the runtime semantics do.
            if matches!(op, Op::And | Op::Or | Op::Implies) {
                let l = expr_to_z3(vc, left, env, solver_opt)?;
                if let Some(lb) = l.as_bool() {
                    let guard = if matches!(op, Op::Or) {
                        lb.not()
                    } else {
                        lb.clone()
                    };
                    vc.path_cond_stack.borrow_mut().push(guard);
                    let r = expr_to_z3(vc, right, env, solver_opt);
                    vc.path_cond_stack.borrow_mut().pop();
                    let rb = r?
                        .as_bool()
                        .ok_or_else(|| MumeiError::type_error("Expected bool operand"))?;
                    return Ok(match op {
                        Op::And => Bool::and(ctx, &[&lb, &rb]).into(),
                        Op::Or => Bool::or(ctx, &[&lb, &rb]).into(),
                        _ => lb.implies(&rb).into(),
                    });
                }
            }

            let l = expr_to_z3(vc, left, env, solver_opt)?;

            // For `==`/`!=` on an array `Variable`, the operand's value is
            // the live `__z3_arr_<name>`/`len_<name>` slots (the base const
            // `l` never reflects `a[i] = v` stores) — and for the LEFT
            // operand those slots must be sampled before `right` is
            // lowered: a shadowing `let a = …` or a rebind inside the right
            // operand rewrites the same tracked slots, so reading them at
            // dispatch time would compare the post-right `a` on both sides.
            let array_eq_left_struct_leaves =
                if matches!(op, Op::Eq | Op::Neq) && l.as_array().is_some() {
                    super::z3_types::snapshot_struct_array_leaves(vc, env, left, &l, None)?
                } else {
                    None
                };

            let array_eq_left_live = if matches!(op, Op::Eq | Op::Neq) && l.as_array().is_some() {
                match &**left {
                    Expr::Variable(name) => Some((
                        z3_dynamic_array(vc, name, env),
                        array_len_value(vc.ctx, env, name, vc.bitvec_i64, solver_opt),
                    )),
                    _ => None,
                }
            } else {
                None
            };

            let r = expr_to_z3(vc, right, env, solver_opt)?;

            // Whole-array `==`/`!=`: an array value is its (contents, len)
            // pair. Checked before every numeric/String branch because a
            // mixed operand like `a == 0.0` would otherwise reach the Real
            // arm, which substitutes a zero literal for the array side and
            // silently proves `0.0 == 0.0`.
            if matches!(op, Op::Eq | Op::Neq) && (l.as_array().is_some() || r.as_array().is_some())
            {
                let eq = array_value_equality(
                    vc,
                    left,
                    right,
                    &l,
                    &r,
                    env,
                    solver_opt,
                    array_eq_left_live,
                    array_eq_left_struct_leaves,
                )?;
                return Ok(if matches!(op, Op::Neq) {
                    eq.not().into()
                } else {
                    eq.into()
                });
            }

            // Plan 9-8: String concatenation — if both operands are Z3 String Sort
            if l.get_sort() == z3::Sort::string(ctx) && r.get_sort() == z3::Sort::string(ctx) {
                mark_string_constraints(vc);
                let ls = l.as_string().ok_or("Expected string for Str op")?;
                let rs = r.as_string().ok_or("Expected string for Str op")?;
                return match op {
                    Op::Add => {
                        // Z3 string concatenation — return concat directly
                        let result = z3::ast::String::concat(ctx, &[&ls, &rs]);
                        Ok(result.into())
                    }
                    Op::Eq => Ok(ls._eq(&rs).into()),
                    Op::Neq => Ok(ls._eq(&rs).not().into()),
                    _ => Err(format!("Unsupported operator {:?} for Str type", op).into()),
                };
            }

            // Opt-in bit-vector `i64` path (`--bitvec-i64`): as soon as one
            // operand is a `BV(64)`, the whole operation is lowered into the
            // bit-vector theory. An `Int` operand (integer literal, array
            // element, result of an atom encoded in `Int` mode) is bridged with
            // `int2bv`, which is exactly the two's complement reading of the
            // literal. Placed before the `Real`/`Float` branches so a mixed
            // `Int`/`BV` contract never reaches `as_int()` with mismatched
            // sorts.
            // In bit-vector mode two `Int` literals are also lowered into the
            // bit-vector theory, so `12 & 10` and the wrapping of
            // `9223372036854775807 + 1` follow machine semantics too. Other
            // `Int`-sorted terms (array elements, quantifier bounds) keep the
            // `Int` encoding: bridging them with `int2bv` costs Z3 far more
            // than the wrapping is worth, and they are not machine values.
            // A mixed `i64`/`f64` operation keeps the numeric promotion of the
            // default mode instead: it is the `f64` branches below that widen
            // the bit-vector operand (through the signed bridge), because
            // `bvadd` has no meaning for a floating-point operand. Bitwise
            // operators on an `f64` stay rejected there.
            let has_fp_operand = [&l, &r]
                .iter()
                .any(|value| value.as_real().is_some() || value.as_float().is_some());
            let bitwise_op = matches!(op, Op::BitAnd | Op::BitOr | Op::BitXor | Op::Shl | Op::Shr);
            let both_literals = [&l, &r]
                .iter()
                .all(|value| value.as_int().and_then(|i| i.as_i64()).is_some());
            let (lt, rt) = (
                fixed_width_int_type(vc, left),
                fixed_width_int_type(vc, right),
            );
            if (lt == Some("u64") && rt.is_some_and(|t| t != "u64"))
                || (rt == Some("u64") && lt.is_some_and(|t| t != "u64"))
            {
                return Err(MumeiError::type_error(
                    "Cannot mix a u64 operand with a declared non-u64 integer operand",
                )
                .with_help("Keep both sides u64, or convert the signed operand explicitly"));
            }
            let unsigned = lt == Some("u64") || rt == Some("u64");
            if !has_fp_operand
                && (is_bv_i64(&l)
                    || is_bv_i64(&r)
                    || lt == Some("u64")
                    || rt == Some("u64")
                    || (vc.bitvec_i64 && (bitwise_op || both_literals)))
            {
                let ty = lt.or(rt).unwrap_or(if unsigned { "u64" } else { "i64" });
                return bv_binary_op(vc, op, &l, &r, solver_opt, unsigned, ty);
            }

            if l.as_real().is_some() || r.as_real().is_some() {
                let lr = l
                    .as_real()
                    .or_else(|| as_int_like(&l).map(|i| i.to_real()))
                    .unwrap_or_else(|| Real::from_real(ctx, 0, 1));
                let rr = r
                    .as_real()
                    .or_else(|| as_int_like(&r).map(|i| i.to_real()))
                    .unwrap_or_else(|| Real::from_real(ctx, 0, 1));
                match op {
                    Op::Gt => Ok(lr.gt(&rr).into()),
                    Op::Lt => Ok(lr.lt(&rr).into()),
                    Op::Ge => Ok(lr.ge(&rr).into()),
                    Op::Le => Ok(lr.le(&rr).into()),
                    Op::Eq => Ok(lr._eq(&rr).into()),
                    Op::Neq => Ok(lr._eq(&rr).not().into()),
                    Op::Add => Ok(Real::add(ctx, &[&lr, &rr]).into()),
                    Op::Sub => Ok(Real::sub(ctx, &[&lr, &rr]).into()),
                    Op::Mul => Ok(Real::mul(ctx, &[&lr, &rr]).into()),
                    Op::Div => {
                        // Z3 leaves `x / 0.0` unconstrained over the reals, so an
                        // unchecked zero divisor would let any `ensures` through.
                        if let Some(solver) = solver_opt {
                            solver.push();
                            for cond in vc.path_cond_stack.borrow().iter() {
                                solver.assert(cond);
                            }
                            solver.assert(&rr._eq(&Real::from_real(ctx, 0, 1)));
                            let zero_divisor = crate::verification::phase_artifacts::check(solver);
                            solver.pop(1);
                            match zero_divisor {
                                SatResult::Sat => {
                                    return Err(MumeiError::verification(
                                        "Potential division by zero.".to_string(),
                                    )
                                    .with_help("Add a condition divisor != 0.0 to requires")
                                    .with_verdict(crate::verification::VerdictKind::Rejected))
                                }
                                SatResult::Unknown => {
                                    return Err(MumeiError::verification(
                                        "Z3 could not decide whether the f64 divisor is nonzero."
                                            .to_string(),
                                    )
                                    .with_help(
                                        "Strengthen requires so the divisor is provably nonzero",
                                    )
                                    .with_verdict(crate::verification::VerdictKind::Inconclusive))
                                }
                                SatResult::Unsat => {}
                            }
                        } else {
                            vc.div_zero_obligations
                                .borrow_mut()
                                .push((vc.path_cond_conj(), rr._eq(&Real::from_real(ctx, 0, 1))));
                        }
                        Ok(lr.div(&rr).into())
                    }
                    _ => Err("Invalid real op".into()),
                }
            } else if l.as_float().is_some() || r.as_float().is_some() {
                // Opt-in IEEE 754 (`--ieee754-f64`) path: `f64` values are Z3
                // binary64 `Float`s. Lower arithmetic to the FP theory with the
                // round-nearest-ties-to-even mode (matching hardware `f64`) and
                // use `fp` comparisons / `fp.eq` — never an unconstrained fresh
                // constant, so the actual FP semantics are modeled.
                let rne = round_nearest_even(ctx);
                let lf = coerce_to_float(ctx, &l, &rne).ok_or("Expected float operand")?;
                let rf = coerce_to_float(ctx, &r, &rne).ok_or("Expected float operand")?;
                match op {
                    Op::Gt => Ok(lf.gt(&rf).into()),
                    Op::Lt => Ok(lf.lt(&rf).into()),
                    Op::Ge => Ok(lf.ge(&rf).into()),
                    Op::Le => Ok(lf.le(&rf).into()),
                    Op::Eq => Ok(float_eq(ctx, &lf, &rf).into()),
                    Op::Neq => Ok(float_eq(ctx, &lf, &rf).not().into()),
                    Op::Add => Ok(rne.add(&lf, &rf).into()),
                    Op::Sub => Ok(rne.sub(&lf, &rf).into()),
                    Op::Mul => Ok(rne.mul(&lf, &rf).into()),
                    Op::Div => Ok(rne.div(&lf, &rf).into()),
                    _ => Err("Invalid float op".into()),
                }
            } else {
                // Boolean 演算子は as_int() の前に処理する（オペランドが Bool のため）
                match op {
                    Op::And => {
                        let lb = l.as_bool().ok_or("Expected bool for &&")?;
                        let rb = r.as_bool().ok_or("Expected bool for &&")?;
                        return Ok(Bool::and(ctx, &[&lb, &rb]).into());
                    }
                    Op::Or => {
                        let lb = l.as_bool().ok_or("Expected bool for ||")?;
                        let rb = r.as_bool().ok_or("Expected bool for ||")?;
                        return Ok(Bool::or(ctx, &[&lb, &rb]).into());
                    }
                    Op::Implies => {
                        let lb = l.as_bool().ok_or("Expected bool for =>")?;
                        let rb = r.as_bool().ok_or("Expected bool for =>")?;
                        return Ok(lb.implies(&rb).into());
                    }
                    Op::Eq | Op::Neq if l.as_bool().is_some() || r.as_bool().is_some() => {
                        let lb = l.as_bool().ok_or("Expected bool for ==")?;
                        let rb = r.as_bool().ok_or("Expected bool for ==")?;
                        let eq = lb._eq(&rb);
                        return Ok(if matches!(op, Op::Neq) {
                            eq.not().into()
                        } else {
                            eq.into()
                        });
                    }
                    // P10-C: finite-ADT equality requires the *same* datatype
                    // sort — comparing two different enums is a hard error
                    // (next arm), not a sort mismatch the solver would
                    // interpret.
                    Op::Eq | Op::Neq
                        if l.as_datatype().is_some()
                            && r.as_datatype().is_some()
                            && l.get_sort() == r.get_sort() =>
                    {
                        let eq =
                            enum_payload_value_equality(vc, &l, &r)?.unwrap_or_else(|| l._eq(&r));
                        return Ok(if matches!(op, Op::Neq) {
                            eq.not().into()
                        } else {
                            eq.into()
                        });
                    }
                    Op::Eq | Op::Neq if l.as_datatype().is_some() && r.as_datatype().is_some() => {
                        return Err(MumeiError::verification(
                            "Cannot compare enum values of different types".to_string(),
                        ));
                    }
                    _ => {}
                }
                let li = l.as_int().ok_or("Expected int")?;
                let ri = r.as_int().ok_or("Expected int")?;
                match op {
                    Op::Add | Op::Sub | Op::Mul => {
                        let result = match op {
                            Op::Add => &li + &ri,
                            Op::Sub => &li - &ri,
                            _ => &li * &ri,
                        }
                        .simplify();
                        // The bind-time pass of a lambda body runs on unconstrained
                        // params; each call site re-lowers it with the real arguments.
                        // A callee's instantiated contract is math, like the
                        // caller's own contract.
                        if vc.check_i64_overflow.get()
                            && vc.recursion.lambda_bind_depth.get() == 0
                            && vc.contracts_in_instantiation.borrow().is_empty()
                        {
                            let ty = fixed_width_int_type(vc, left)
                                .or_else(|| fixed_width_int_type(vc, right))
                                .unwrap_or("i64");
                            let overflows = i64_overflow_condition(vc, &li, &ri, &result, ty);
                            match solver_opt {
                                Some(solver) => check_i64_range(vc, solver, &overflows, ty)?,
                                // Loop conditions and match guards are lowered
                                // without a solver: check them after the body.
                                None => vc.i64_overflow_obligations.borrow_mut().push((
                                    vc.path_cond_conj(),
                                    overflows,
                                    ty,
                                )),
                            }
                        }
                        Ok(result.into())
                    }
                    Op::Div | Op::Rem => {
                        if let Some(solver) = solver_opt {
                            solver.push();
                            for cond in vc.path_cond_stack.borrow().iter() {
                                solver.assert(cond);
                            }
                            solver.assert(&ri._eq(&Int::from_i64(ctx, 0)));
                            if crate::verification::phase_artifacts::check(solver) == SatResult::Sat
                            {
                                let (ce_hint, div_feedback, raised_counterexample) =
                                    if let Some(model) = solver.get_model() {
                                        let dividend = model.eval(&li, true);
                                        let divisor = model.eval(&ri, true);
                                        let mut raised = RaisedCounterexample::default();
                                        if let Some(value) = dividend.as_ref() {
                                            let value = Dynamic::from(value);
                                            raised.values.push(raise_named_model_value(
                                                &model,
                                                "dividend",
                                                &value,
                                                None,
                                                vc.module_env,
                                            ));
                                        }
                                        if let Some(value) = divisor.as_ref() {
                                            let value = Dynamic::from(value);
                                            raised.values.push(raise_named_model_value(
                                                &model,
                                                "divisor",
                                                &value,
                                                None,
                                                vc.module_env,
                                            ));
                                        }
                                        let dividend_val = raised
                                            .values
                                            .iter()
                                            .find(|value| value.source_name == "dividend")
                                            .map(|value| value.rendering.clone())
                                            .unwrap_or_else(|| "?".to_string());
                                        let divisor_val = raised
                                            .values
                                            .iter()
                                            .find(|value| value.source_name == "divisor")
                                            .map(|value| value.rendering.clone())
                                            .unwrap_or_else(|| "0".to_string());
                                        let hint = format!(
                                            " Counter-example: dividend = {}, divisor = {}",
                                            dividend_val, divisor_val
                                        );
                                        let fb = build_division_by_zero_feedback(
                                            &dividend_val,
                                            &divisor_val,
                                        );
                                        (hint, Some(fb), Some(raised))
                                    } else {
                                        (String::new(), None, None)
                                    };
                                // Attach structured feedback to error message for upstream reporting
                                let feedback_hint = div_feedback
                                    .as_ref()
                                    .map(|fb| format!(" [semantic_feedback: {}]", fb))
                                    .unwrap_or_default();
                                let ce_value = raised_counterexample
                                    .as_ref()
                                    .map(RaisedCounterexample::to_counterexample_json);
                                let provenance = raised_counterexample
                                    .as_ref()
                                    .map(RaisedCounterexample::provenance_json);
                                solver.pop(1);
                                return Err(MumeiError::verification(format!(
                                    "Potential division by zero.{}{}",
                                    ce_hint, feedback_hint
                                ))
                                .with_help("Add a condition divisor != 0 to requires")
                                .with_counterexample_provenance(ce_value, provenance)
                                .with_verdict(crate::verification::VerdictKind::Rejected));
                            }
                            solver.pop(1);
                        } else {
                            vc.div_zero_obligations
                                .borrow_mut()
                                .push((vc.path_cond_conj(), ri._eq(&Int::from_i64(ctx, 0))));
                        }
                        if matches!(op, Op::Rem) {
                            Ok(truncated_rem(ctx, &li, &ri).into())
                        } else {
                            // Truncate toward zero like `%` and the
                            // emitted `sdiv` — Z3's `div` is Euclidean
                            // and diverges from native semantics on
                            // negative dividends.
                            Ok(truncated_div(ctx, &li, &ri).into())
                        }
                    }
                    Op::Gt => Ok(li.gt(&ri).into()),
                    Op::Lt => Ok(li.lt(&ri).into()),
                    Op::Ge => Ok(li.ge(&ri).into()),
                    Op::Le => Ok(li.le(&ri).into()),
                    Op::Eq => Ok(li._eq(&ri).into()),
                    Op::Neq => Ok(li._eq(&ri).not().into()),
                    // Bitwise operators have no faithful meaning in the
                    // unbounded `Int` encoding, so they are rejected instead of
                    // being approximated.
                    Op::BitAnd | Op::BitOr | Op::BitXor | Op::Shl | Op::Shr => {
                        Err(MumeiError::verification(format!(
                            "Bitwise operator {:?} requires bit-vector semantics",
                            op
                        ))
                        .with_help(
                            "Verify with --bitvec-i64 so i64 is encoded as a 64-bit bit-vector",
                        ))
                    }
                    _ => Err(MumeiError::verification(format!(
                        "Unsupported int operator {:?}",
                        op
                    ))),
                }
            }
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            let c = expr_to_z3(vc, cond, env, solver_opt)?
                .as_bool()
                .ok_or(MumeiError::type_error("If condition must be boolean"))?;
            // Track the path condition for sub-statements that need it
            // (currently only the loop-invariant base case inside the
            // branch), without touching the solver's persistent assertion
            // stack. This lets, e.g., `if n <= 1 { n } else { let i = 1;
            // while i < n invariant: i >= 1 && i <= n … }` verify because
            // the inner invariant base case can rely on `n > 1` from the
            // surrounding else-guard, while still keeping `let left =
            // merge_sort(mid)` ensures-asserts (`left == mid`) live for the
            // outer postcondition check.
            // Binding types are scoped like the value env: each branch runs
            // on a fresh snapshot, and afterwards only entries the two
            // branches agree on survive (an if-local `let` must not leak its
            // inferred type onto an outer binding).
            let types_before = vc.local_enum_types.borrow().clone();
            let struct_types_before = vc.local_struct_types.borrow().clone();
            let lambdas_before = vc.local_lambdas.borrow().clone();
            let int_types_before = vc.local_int_types.borrow().clone();
            vc.path_cond_stack.borrow_mut().push(c.clone());
            let mut then_env = env.clone();
            let t = stmt_to_z3(vc, then_branch, &mut then_env, solver_opt);
            vc.path_cond_stack.borrow_mut().pop();
            let t = t?;
            let then_types = vc.local_enum_types.borrow().clone();
            let then_struct_types = vc.local_struct_types.borrow().clone();
            let then_lambdas = vc.local_lambdas.borrow().clone();
            *vc.local_enum_types.borrow_mut() = types_before.clone();
            *vc.local_struct_types.borrow_mut() = struct_types_before.clone();
            let then_int_types = std::mem::replace(
                &mut *vc.local_int_types.borrow_mut(),
                int_types_before.clone(),
            );
            *vc.local_lambdas.borrow_mut() = lambdas_before.clone();
            vc.path_cond_stack.borrow_mut().push(c.not());
            let mut else_env = env.clone();
            let e = stmt_to_z3(vc, else_branch, &mut else_env, solver_opt);
            vc.path_cond_stack.borrow_mut().pop();
            let else_types = vc.local_enum_types.borrow().clone();
            let else_struct_types = vc.local_struct_types.borrow().clone();
            let else_lambdas = vc.local_lambdas.borrow().clone();
            let mut merged = types_before;
            merged.retain(|k, v| then_types.get(k) == Some(v) && else_types.get(k) == Some(v));
            for (k, v) in then_types.iter() {
                if else_types.get(k) == Some(v) {
                    merged.insert(k.clone(), v.clone());
                }
            }
            *vc.local_enum_types.borrow_mut() = merged;
            // Same scope rule for struct bindings: shadowing a struct
            // param inside the `then` branch must not remove the type
            // information the `else` branch still needs for `q.f = v`.
            let mut merged_struct = struct_types_before;
            merged_struct.retain(|k, v| {
                then_struct_types.get(k) == Some(v) && else_struct_types.get(k) == Some(v)
            });
            for (k, v) in then_struct_types.iter() {
                if else_struct_types.get(k) == Some(v) {
                    merged_struct.insert(k.clone(), v.clone());
                }
            }
            *vc.local_struct_types.borrow_mut() = merged_struct;
            // A local keeps its integer type only when both branches agree;
            // otherwise the checker falls back to the declared parameter or i64.
            let else_int_types = vc.local_int_types.borrow().clone();
            let mut merged_int = int_types_before;
            merged_int.retain(|k, v| {
                then_int_types.get(k) == Some(v) && else_int_types.get(k) == Some(v)
            });
            for (k, v) in then_int_types.iter() {
                if else_int_types.get(k) == Some(v) {
                    merged_int.insert(k.clone(), v);
                }
            }
            *vc.local_int_types.borrow_mut() = merged_int;
            // Same merge rule as the value env: a binding only on one
            // branch keeps that side's (branch-local bindings leak, like
            // `merge_branch_envs`), while a name bound on both survives
            // only when both sides bound it to the *same* lambda — an
            // `ite` over two different closures has no Z3 encoding, so a
            // disagreeing rebind must drop the entry (a post-branch call
            // then fails closed as an unknown function rather than
            // picking one side's body).
            let mut merged_lambdas = then_lambdas;
            let mut conflicting = std::collections::HashSet::new();
            merged_lambdas.retain(|k, v| match else_lambdas.get(k) {
                Some(else_lambda) if same_local_lambda(v, else_lambda) => true,
                Some(_) => {
                    conflicting.insert(k.clone());
                    false
                }
                None => true,
            });
            for (k, v) in else_lambdas.iter() {
                if !merged_lambdas.contains_key(k) && !conflicting.contains(k) {
                    merged_lambdas.insert(k.clone(), v.clone());
                }
            }
            *vc.local_lambdas.borrow_mut() = merged_lambdas;
            let (t, e) = unify_branch_sorts(t, e?)?;
            // Branches ran on isolated env copies — merge their writes with
            // `ite(c, then, else)` per variable. Running both on the shared
            // env would apply every assignment unconditionally (the else
            // branch would even see the then-branch's writes), which is
            // unsound.
            merge_branch_envs(env, then_env, else_env, &c);
            Ok(c.ite(&t, &e))
        }
        Expr::Block(stmt) => stmt_to_z3(vc, stmt, env, solver_opt),

        Expr::StructInit { type_name, fields } => {
            // 構造体の各フィールドを検証し、env に登録
            // フィールドに精緻型制約がある場合は solver で検証する
            //
            // 構造体値そのものはハンドル（未解釈 Int 定数）で表し、各フィールドは
            // `__struct_<handle>_<field>` に平坦化して登録する。跨フィールド不変量は
            // 全フィールドの評価後にこの平坦化シンボルに対して検査する。
            let sdef = vc.module_env.get_struct(type_name);
            let handle = fresh_struct_handle(ctx, type_name);
            let handle_name = handle.decl().name();
            let mut field_values: Vec<(String, Dynamic)> = Vec::with_capacity(fields.len());
            for (field_name, field_expr) in fields {
                let mut val = expr_to_z3(vc, field_expr, env, solver_opt)?;
                let array_field = sdef
                    .and_then(|sdef| sdef.fields.iter().find(|field| field.name == *field_name))
                    .and_then(|field| datatype::field_leaf_types(&field.type_name, vc.module_env))
                    .is_some_and(|leaves| datatype::array_payload_element_type(&leaves).is_some());
                if array_field {
                    if let Expr::Variable(name) = field_expr {
                        super::z3_types::mark_array_shared(ctx, env, name);
                    }
                    let array = match field_expr {
                        Expr::Variable(name) => env
                            .get(&format!("__z3_arr_{name}"))
                            .and_then(|value| value.as_array())
                            .or_else(|| val.as_array()),
                        _ => val.as_array(),
                    };
                    if let Some(array) = array {
                        let array_value: Dynamic = array.clone().into();
                        let len = super::z3_types::array_len_for_expr(
                            vc,
                            env,
                            Some(field_expr),
                            &array_value,
                            solver_opt,
                        );
                        super::z3_types::register_array_value_len(env, &array, len);
                        val = array_value;
                    }
                }
                let qualified_name = format!("__struct_{}_{}", type_name, field_name);
                env.insert(qualified_name, val.clone());
                field_values.push((field_name.clone(), val.clone()));

                // フィールド制約の検証: 構造体定義から constraint を取得
                if let Some(sdef) = vc.module_env.get_struct(type_name) {
                    if let Some(sfield) = sdef.fields.iter().find(|f| f.name == *field_name) {
                        // A refined-alias field type is assumed for struct
                        // parameters, so every literal must establish it.
                        if let Some(solver) = solver_opt {
                            // Checked under the current path condition, and
                            // only `Unsat` proves the predicate: `Unknown`
                            // must not let an unproven domain be assumed.
                            for (decl, predicate) in field_type_predicates(vc, sfield, &val, env)? {
                                solver.push();
                                solver.assert(&vc.path_cond_conj());
                                solver.assert(&predicate.not());
                                let verdict = crate::verification::phase_artifacts::check(solver);
                                solver.pop(1);
                                if verdict != SatResult::Unsat {
                                    return Err(MumeiError::verification(format!(
                                        "Struct '{}' field '{}' type '{}' predicate violated",
                                        type_name, field_name, decl
                                    ))
                                    .with_verdict(crate::verification::VerdictKind::Rejected));
                                }
                            }
                        }
                        if let Some(constraint_raw) = &sfield.constraint {
                            // constraint 内の "v" をフィールド値に置き換えて検証
                            let mut local_env = env.clone();
                            local_env.insert("v".to_string(), val.clone());
                            let constraint_ast = parse_expression(constraint_raw);
                            let constraint_z3 =
                                expr_to_z3(vc, &constraint_ast, &mut local_env, None)?;
                            if let Some(constraint_bool) = constraint_z3.as_bool() {
                                if let Some(solver) = solver_opt {
                                    solver.push();
                                    // Hold under the current path
                                    // condition — without it, inputs on
                                    // not-taken branches falsify the
                                    // clause and a write guarded by `if`
                                    // rejects even though the constraint
                                    // holds on every reachable path.
                                    solver.assert(&vc.path_cond_conj());
                                    solver.assert(&constraint_bool.not());
                                    if crate::verification::phase_artifacts::check(solver)
                                        == SatResult::Sat
                                    {
                                        solver.pop(1);
                                        return Err(MumeiError::verification(format!(
                                            "Struct '{}' field '{}' constraint violated: {}",
                                            type_name, field_name, constraint_raw
                                        ))
                                        .with_verdict(crate::verification::VerdictKind::Rejected));
                                    }
                                    solver.pop(1);
                                }
                            }
                        }
                    }
                }
            }
            bind_struct_fields(env, &handle_name, &field_values);
            // Deep leaves: a struct-typed field's own fields are only
            // bound under ITS handle — re-home them under this one too
            // (`<handle>_mid_m_a`) or `result.mid.m.a`-style reads on a
            // nested literal resolve to unconstrained fresh symbols.
            for (name, value) in &field_values {
                let fields = nested_field_bindings(env, value);
                if !fields.is_empty() {
                    bind_struct_fields(env, &format!("{handle_name}_{name}"), &fields);
                }
            }
            if let (Some(sdef), Some(solver)) = (sdef, solver_opt) {
                check_struct_invariants(
                    vc,
                    solver,
                    sdef,
                    &format!("'{}' literal", type_name),
                    &field_values,
                    env,
                )?;
            }
            Ok(handle)
        }
        Expr::Match { target, arms } => invoke_match_lowering(|| {
            let target_z3 = expr_to_z3(vc, target, env, solver_opt)?;

            // Declared enum type of the scrutinee, when it's a bare
            // parameter/`result` variable or a field chain rooted at a
            // struct-typed binding (`h.r`, `h.a.b`, `result.r`, `f(x).r`) —
            // disambiguates Variant arms whose names collide across enums
            // (prelude List vs user `enum IntList`). The const-name lookup
            // inside `resolve_variant_owner` covers let-aliases; `result` is
            // rebound to the evaluated body value, so only the Expr knows.
            let decl_hint: Option<String> =
                crate::verification::support::datatype::declared_type_of_expr(vc, target).map(
                    |t| {
                        // A generic instance (`Wrap<i64>`) is itself the
                        // registered enum — keep it, since `type_name_base`
                        // would strip down to the unencodable generic def.
                        if vc.module_env.get_enum(&t).is_some() {
                            t
                        } else {
                            crate::verification::support::datatype::type_name_base(&t).to_string()
                        }
                    },
                );

            // Clause lowering (`requires`/`ensures`, struct invariants, match
            // guards) runs with `solver_opt = None` so arm-body side effects
            // never leak onto the ambient solver — but exhaustiveness is a
            // language-level obligation. Without a check, a non-exhaustive
            // clause match silently evaluates to its last arm's body on
            // uncovered inputs (e.g. a one-arm `requires: match h.r { V =>
            // e }` would lower to just `e`). Run the check on a scratch
            // solver seeded with the already-asserted clause context.
            let scratch_solver;
            let check_solver = match solver_opt {
                Some(solver) => solver,
                None => {
                    let scratch = Solver::new(ctx);
                    for clause in vc.clause_context.borrow().iter() {
                        scratch.assert(clause);
                    }
                    scratch_solver = scratch;
                    &scratch_solver
                }
            };

            // ========================================================
            // Enum ドメイン制約の自動注入
            // ========================================================
            // アームに Variant パターンが含まれる場合、対応する EnumDef を探し、
            // target の値域を 0..n_variants に制約する。
            // これにより Z3 が「これら以外のバリアントは存在しない」ことを知り、
            // 網羅性チェックの信頼性が 100% になる。
            if let Some(enum_def) =
                detect_enum_from_arms(arms, vc, &target_z3, decl_hint.as_deref())
            {
                let n = enum_def.variants.len() as i64;
                if let Some(tag_int) = target_z3.as_int() {
                    // tag ∈ [0, n_variants)
                    check_solver.assert(&tag_int.ge(&Int::from_i64(ctx, 0)));
                    check_solver.assert(&tag_int.lt(&Int::from_i64(ctx, n)));
                }
            }

            let array_elem_types_before_match = vc.local_array_elem_types.borrow().clone();
            // ========================================================
            // Z3 網羅性チェック (Exhaustiveness Check)
            // ========================================================
            // 各アームの条件 P_i を構築し、¬(P_1 ∨ P_2 ∨ ... ∨ P_n) が
            // Unsat であることを証明する。Sat なら網羅性欠如エラー。
            {
                let mut arm_conditions: Vec<Bool> = Vec::new();
                let mut coverage_negations: Vec<Bool> = Vec::new();
                let enum_types_before_coverage = vc.local_enum_types.borrow().clone();
                let array_elem_types_before_coverage = array_elem_types_before_match.clone();
                for arm in arms {
                    *vc.local_enum_types.borrow_mut() = enum_types_before_coverage.clone();
                    // This coverage pass binds payload names only to make
                    // guards resolvable — the registrations must not leak
                    // past the arm: a payload `x` shadowing an outer `i64`
                    // `x` would leave the outer var typed `u64` after the
                    // match. Snapshot/restore per arm like the arm pass
                    // below does.
                    let int_types_before_check = vc.local_int_types.borrow().clone();
                    let array_elem_types_before_check = vc.local_array_elem_types.borrow().clone();
                    let cond = pattern_to_z3_condition(
                        ctx,
                        &arm.pattern,
                        &target_z3,
                        env,
                        vc,
                        Some(check_solver),
                        decl_hint.as_deref(),
                    )?;
                    *vc.local_enum_types.borrow_mut() = enum_types_before_coverage.clone();
                    // ガード条件がある場合は AND で結合
                    let full_cond = if let Some(guard) = &arm.guard {
                        // The guard may name the arm's payload binders —
                        // evaluate it in an env where the pattern's
                        // bindings exist, like the arm pass below does.
                        let mut check_env = env.clone();
                        let mut check_bound: std::collections::HashSet<String> =
                            std::collections::HashSet::new();
                        let enum_types_before_guard = vc.local_enum_types.borrow().clone();
                        pattern_bind_variables(
                            &arm.pattern,
                            &target_z3,
                            &mut check_env,
                            vc,
                            None,
                            decl_hint.as_deref(),
                            &mut check_bound,
                        );
                        // The arm pass below checks the guard's arithmetic.
                        let check_i64 = vc.check_i64_overflow.replace(false);
                        {
                            let mut stack = vc.path_cond_stack.borrow_mut();
                            for negation in &coverage_negations {
                                stack.push(negation.clone());
                            }
                            stack.push(cond.clone());
                        }
                        let guard_z3 = expr_to_z3(vc, guard, &mut check_env, None);
                        {
                            let mut stack = vc.path_cond_stack.borrow_mut();
                            for _ in 0..=coverage_negations.len() {
                                stack.pop();
                            }
                        }
                        vc.check_i64_overflow.set(check_i64);
                        *vc.local_enum_types.borrow_mut() = enum_types_before_guard;
                        *vc.local_array_elem_types.borrow_mut() =
                            array_elem_types_before_check.clone();
                        let guard_z3 = guard_z3?
                            .as_bool()
                            .ok_or(MumeiError::type_error("Guard must be boolean"))?;
                        Bool::and(ctx, &[&cond, &guard_z3])
                    } else {
                        cond
                    };
                    *vc.local_int_types.borrow_mut() = int_types_before_check;
                    *vc.local_enum_types.borrow_mut() = enum_types_before_coverage.clone();
                    *vc.local_array_elem_types.borrow_mut() = array_elem_types_before_check;
                    coverage_negations.push(full_cond.not());
                    arm_conditions.push(full_cond);
                }
                *vc.local_enum_types.borrow_mut() = enum_types_before_coverage;
                *vc.local_array_elem_types.borrow_mut() = array_elem_types_before_coverage;

                // 網羅性: ¬(P_1 ∨ ... ∨ P_n) が Unsat か？
                let arm_refs: Vec<&Bool> = arm_conditions.iter().collect();
                let coverage = Bool::or(ctx, &arm_refs);
                check_solver.push();
                check_solver.assert(&coverage.not());
                let exhaustive =
                    crate::verification::phase_artifacts::check(check_solver) == SatResult::Unsat;
                check_solver.pop(1);

                if !exhaustive {
                    // 反例（Counter-example）の取得と表示
                    // solver はまだ Sat 状態なので、再度チェックして model を取得
                    check_solver.push();
                    check_solver.assert(&coverage.not());
                    if crate::verification::phase_artifacts::check(check_solver) == SatResult::Sat {
                        let (counterexample, counterexample_provenance) =
                            if let Some(model) = check_solver.get_model() {
                                // ターゲット変数の具体的な値を取得
                                format_counterexample(
                                    &model,
                                    &target_z3,
                                    arms,
                                    env,
                                    vc,
                                    decl_hint.as_deref(),
                                )
                            } else {
                                ("unknown value".to_string(), None)
                            };
                        check_solver.pop(1);
                        let ce_value = serde_json::json!({
                            "target": counterexample,
                        });
                        return Err(MumeiError::verification(
                            format!(
                                "Match is not exhaustive: the following value is not covered by any arm:\n  Counter-example: {}",
                                counterexample
                            )
                        ).with_counterexample_provenance(
                            Some(ce_value),
                            counterexample_provenance
                                .as_ref()
                                .map(RaisedCounterexample::provenance_json),
                        )
                        .with_verdict(crate::verification::VerdictKind::Rejected));
                    }
                    check_solver.pop(1);
                    return Err(MumeiError::verification(
                        "Match is not exhaustive: there exist values not covered by any arm.",
                    )
                    .with_verdict(crate::verification::VerdictKind::Inconclusive));
                }
            }

            // ========================================================
            // Match 式の値の構築（if-then-else チェーンとして Z3 式を構築）
            // ========================================================
            // Arms are evaluated in source order so `accumulated_negations`
            // holds the negations of the *preceding* arms — the window an
            // arm body actually runs under (all earlier arms failed to match
            // AND this arm's condition holds). The window is pushed onto the
            // path-condition stack during body evaluation so path-cond-aware
            // checks inside the body (refined ctor args, call-site
            // refinements, divisions) see only reachable inputs; the results
            // are then folded into an ite chain from the last arm outward.
            let mut accumulated_negations: Vec<Bool> = Vec::new();
            let mut arm_results: Vec<(Bool, Dynamic)> = Vec::with_capacity(arms.len());
            let mut arm_array_results: Vec<(Bool, Option<MatchArrayResult>)> =
                Vec::with_capacity(arms.len());
            // Per-arm writes to pre-existing vars fold back under first-match
            // precedence: collected forward, then merged last-to-first so an
            // earlier arm's state overrides a later arm's (the accumulated
            // `ite(cond, arm, acc)` otherwise lets a trailing `_` win).
            let mut arm_states: Vec<(Bool, Env, std::collections::HashSet<String>)> =
                Vec::with_capacity(arms.len());
            let mut merged_arm_env: Option<Env> = None;
            // Lambda `let`s inside an arm body stay arm-local — restore the
            // map before each arm and after the match, mirroring the
            // per-arm `env.clone()` scoping below.
            let lambdas_before = vc.local_lambdas.borrow().clone();
            let enum_types_before_arms = vc.local_enum_types.borrow().clone();

            for arm in arms.iter() {
                *vc.local_lambdas.borrow_mut() = lambdas_before.clone();
                *vc.local_enum_types.borrow_mut() = enum_types_before_arms.clone();
                let mut arm_env = env.clone();
                let int_types_before_arm = vc.local_int_types.borrow().clone();
                let array_elem_types_before_arm = vc.local_array_elem_types.borrow().clone();
                if let Pattern::Variable(name) = &arm.pattern {
                    let ty = fixed_width_int_type(vc, target).unwrap_or("i64");
                    vc.local_int_types.borrow_mut().insert(name.clone(), ty);
                }

                // Names bound by the arm's pattern — or by `let`s anywhere
                // in the arm body — stay arm-local even when they shadow an
                // outer binding; they must not be folded into the post-match
                // env like assignments.
                let mut arm_bound: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                crate::hir::collect_pattern_bindings(&arm.pattern.clone().into(), &mut arm_bound);
                collect_arm_local_lets(&arm.body, &mut arm_bound);

                // B. ネストパターンの再帰解体:
                //    pattern_bind_variables が再帰的にパターンを分解し、
                //    バインド変数を arm_env に登録する。
                pattern_bind_variables(
                    &arm.pattern,
                    &target_z3,
                    &mut arm_env,
                    vc,
                    solver_opt,
                    decl_hint.as_deref(),
                    &mut arm_bound,
                );

                let arm_cond = pattern_to_z3_condition(
                    ctx,
                    &arm.pattern,
                    &target_z3,
                    &mut arm_env,
                    vc,
                    solver_opt,
                    decl_hint.as_deref(),
                )?;
                // A payload field declared with a refined type carries the
                // declaration's conjuncts — asserted under the arm's hit
                // condition since the payload only exists when the tag
                // matches. Without this, `On(p: Port)` would bind `p` to an
                // unconstrained payload and reject valid atoms.
                if let (
                    Some(solver),
                    Pattern::Variant {
                        variant_name,
                        fields,
                    },
                ) = (solver_opt, &arm.pattern)
                {
                    if let Ok(Some(enum_def)) = datatype::resolve_variant_owner(
                        vc,
                        &target_z3,
                        variant_name,
                        decl_hint.as_deref(),
                    ) {
                        let variant_leaf = datatype::variant_leaf(variant_name);
                        if let Some(variant_def) =
                            enum_def.variants.iter().find(|v| v.name == variant_leaf)
                        {
                            for (i, field_pattern) in fields.iter().enumerate() {
                                let (Pattern::Variable(bound_name), Some(field_ty)) =
                                    (field_pattern, variant_def.field_types.get(i))
                                else {
                                    continue;
                                };
                                let Some(refined) = vc.module_env.get_type(field_ty.name.as_str())
                                else {
                                    continue;
                                };
                                let Some(bound) = arm_env.get(bound_name).cloned() else {
                                    continue;
                                };
                                let chain = vc.module_env.refined_chain(&refined.name).map_err(
                                    |cycle| MumeiError::type_error_at(cycle, refined.span.clone()),
                                )?;
                                for decl in chain {
                                    let mut local_env = arm_env.clone();
                                    local_env.insert(decl.operand.clone(), bound.clone());
                                    let predicate_ast = parse_expression(&decl.predicate_raw);
                                    let Some(predicate_z3) = refined_predicate_to_z3(
                                        vc,
                                        decl,
                                        &predicate_ast,
                                        &mut local_env,
                                        None,
                                    )?
                                    .as_bool() else {
                                        continue;
                                    };
                                    solver.assert(&arm_cond.implies(&predicate_z3));
                                }
                            }
                        }
                    }
                }
                let full_cond = if let Some(guard) = &arm.guard {
                    // The guard only runs when the arm's pattern matches.
                    vc.path_cond_stack.borrow_mut().push(arm_cond.clone());
                    let pending = vc.i64_overflow_obligations.borrow().len();
                    let guard_z3 = expr_to_z3(vc, guard, &mut arm_env, None);
                    vc.path_cond_stack.borrow_mut().pop();
                    // Check now, under the solver's assumptions (a loop body's
                    // invariant), rather than after the body without them.
                    if let Some(solver) = solver_opt {
                        discharge_i64_overflow_obligations_from(vc, solver, pending)?;
                    }
                    let guard_z3 = guard_z3?
                        .as_bool()
                        .ok_or(MumeiError::type_error("Guard must be boolean"))?;
                    Bool::and(ctx, &[&arm_cond, &guard_z3])
                } else {
                    arm_cond
                };

                // The arm body only runs on inputs that reach it — prior
                // arms did not match AND this arm's condition holds. Push
                // that window onto the path-condition stack while
                // translating the body so path-cond-aware checks inside it
                // (refined ctor args, call-site refinements, divisions) see
                // only reachable inputs — `match x { 1 => Chan::Tcp(x), .. }`
                // must not check the constructor against `x = 0`. The window
                // goes on the stack rather than the solver so checks that do
                // not consult path conditions are not perturbed.
                {
                    let mut stack = vc.path_cond_stack.borrow_mut();
                    for neg in accumulated_negations.iter() {
                        stack.push(neg.clone());
                    }
                    stack.push(full_cond.clone());
                }
                let body_val = stmt_to_z3(vc, &arm.body, &mut arm_env, solver_opt);
                {
                    let mut stack = vc.path_cond_stack.borrow_mut();
                    for _ in 0..=accumulated_negations.len() {
                        stack.pop();
                    }
                }
                let body_val = body_val?;
                let array_result =
                    match_arm_array_result(vc, &arm.body, &body_val, &mut arm_env, solver_opt)?;
                *vc.local_array_elem_types.borrow_mut() = array_elem_types_before_arm;
                *vc.local_enum_types.borrow_mut() = enum_types_before_arms.clone();
                *vc.local_int_types.borrow_mut() = int_types_before_arm.clone();
                arm_states.push((full_cond.clone(), arm_env, arm_bound));
                arm_array_results.push((full_cond.clone(), array_result));
                arm_results.push((full_cond.clone(), body_val));
                accumulated_negations.push(full_cond.not());
            }
            *vc.local_enum_types.borrow_mut() = enum_types_before_arms;
            *vc.local_array_elem_types.borrow_mut() = array_elem_types_before_match;

            // Fold the arm values into an ite chain from the last arm
            // outward: `ite(c_1, b_1, ite(c_2, b_2, ... b_n))`, and fold the
            // per-arm writes in the same order so the first matching arm
            // takes precedence for post-match state.
            let mut result: Option<Dynamic> = None;
            for (cond, val) in arm_results.into_iter().rev() {
                result = Some(match result {
                    Some(else_val) => {
                        let (val, else_val) = unify_branch_sorts(val, else_val)?;
                        cond.ite(&val, &else_val)
                    }
                    None => val,
                });
            }
            for (cond, arm_env, arm_bound) in arm_states.into_iter().rev() {
                merge_arm_env_into(&mut merged_arm_env, &arm_env, env, &cond, &arm_bound);
            }

            if let Some(merged_env) = merged_arm_env {
                *env = merged_env;
            }
            *vc.local_lambdas.borrow_mut() = lambdas_before;

            let result =
                result.ok_or_else(|| MumeiError::verification("Match expression has no arms"))?;
            let array_result = merge_match_array_results(&arm_array_results)?;
            if let Some(array) = result.as_array() {
                let array_result = array_result.ok_or_else(|| {
                    MumeiError::type_error("could not resolve array metadata for match result")
                })?;
                super::z3_types::register_array_result(
                    vc,
                    &array,
                    array_result.len,
                    array_result.struct_element_type,
                    array_result.struct_leaves,
                );
            } else if array_result.is_some() {
                return Err(MumeiError::type_error(
                    "match branches have incompatible array results",
                ));
            }
            Ok(result)
        }),

        // =================================================================
        // 非同期処理 + リソース管理の Z3 検証
        // =================================================================
        Expr::Perform {
            effect,
            operation,
            args: perform_args,
        } => {
            // Effect system: record effect usage and verify against allowed set
            // Record that this effect was used
            let used_name = format!("__effect_used_{}", effect);
            let used_bool = Bool::from_bool(ctx, true);
            env.insert(used_name.clone(), used_bool.into());

            // Wire EffectCtx: track the performed effect
            if let Some(ectx_cell) = vc.effect_ctx {
                let mut ectx = ectx_cell.borrow_mut();
                // Record usage; violations are warnings here (Z3 check below is authoritative)
                let _ = ectx.perform_effect(effect);
            }

            // Check SecurityPolicy parameter constraints if available.
            // Currently only checks constant arguments (Number-based path IDs);
            // symbolic arguments are validated via Z3 constraints in verify_effect_params.
            if let Some(ref policy) = vc.module_env.security_policy {
                if !policy.is_effect_allowed(effect) {
                    return Err(MumeiError::verification(format!(
                        "Security policy violation: effect '{}' is not permitted by the \
                         current security policy",
                        effect
                    )));
                }
            }

            // Check against allowed effects via Z3 environment
            let allowed_name = format!("__effect_allowed_{}", effect);
            if env.get(&allowed_name).is_none() {
                // Effect not in allowed set — immediate violation
                return Err(MumeiError::verification(format!(
                    "Effect violation: 'perform {}.{}' requires [{}] effect, \
                         but it is not declared in the current atom's effects set.",
                    effect, operation, effect
                ))
                .with_help(format!(
                    "Fix option 1: Add '{}' to the effects declaration: effects: [{}];\n\
                         Fix option 2: Remove the call to 'perform {}.{}'.",
                    effect, effect, effect, operation
                )));
            }

            // If solver is available, assert the Z3 containment constraint
            if let Some(solver) = solver_opt {
                let used_z3 = Bool::from_bool(ctx, true);
                let allowed_z3 = Bool::from_bool(ctx, true); // already proven allowed
                                                             // Assert: used → allowed (trivially true when allowed)
                solver.assert(&used_z3.implies(&allowed_z3));
            }

            // Process arguments and collect Z3 values
            let mut arg_z3_values: Vec<Dynamic> = Vec::new();
            for arg in perform_args {
                let val = expr_to_z3(vc, arg, env, solver_opt)?;
                arg_z3_values.push(val);
            }
            vc.reject_quantifier_dependent_call(
                &format!("perform {effect}.{operation}"),
                &arg_z3_values,
            )?;

            // Z3 String Sort: verify symbolic parameter constraints
            // Look up the EffectDef to get constraint and param definitions
            let effect_def = vc
                .module_env
                .effect_defs
                .get(effect.as_str())
                .or_else(|| vc.module_env.effects.get(effect.as_str()))
                .cloned();
            // capability 型パラメータの静的型から constraint を引く。
            // `perform cap.op(x)` はパーサで裏側のエフェクトに解決されているので、
            // 同じエフェクトを指す capability 宣言の制約をエフェクト制約と並べて検証する。
            let capability_constraints: Vec<String> = vc
                .current_atom
                .map(|current| {
                    current
                        .params
                        .iter()
                        .filter_map(|param| {
                            param
                                .type_ref
                                .as_ref()
                                .and_then(|ty| ty.capability.as_ref())
                        })
                        .filter(|capability| capability.effect == *effect)
                        .filter_map(|capability| capability.constraint.clone())
                        .collect()
                })
                .unwrap_or_default();

            if let Some(def) = effect_def {
                let constraints: Vec<&str> = def
                    .constraint
                    .as_deref()
                    .into_iter()
                    .chain(capability_constraints.iter().map(String::as_str))
                    .collect();
                for constraint in constraints {
                    // For each argument, check if it's a symbolic (non-constant) value
                    // that needs Z3 String constraint verification.
                    // NOTE: Currently def.constraint is a single string (e.g., "starts_with(path, \"/tmp/\")")
                    // that is applied to ALL non-constant args. This is correct for single-parameter
                    // effects (the only kind currently supported by the parser), but would incorrectly
                    // apply a path-specific constraint to unrelated parameters if multi-parameter
                    // effects like FileOp(path: Str, mode: Str) are added. When that happens,
                    // extract the parameter name from the constraint string, find its index in
                    // def.params, and only apply the constraint when `i` matches that index.
                    for (i, arg) in perform_args.iter().enumerate() {
                        // Number/Float literals are constants already checked
                        // by verify_effect_params (Phase 1g). Skip Z3 String here.
                        // Variables and other expressions need symbolic verification.
                        let is_constant = matches!(arg, Expr::Number(_) | Expr::Float(_));
                        if is_constant {
                            // Constant args are already checked by check_constant_constraint
                            // in verify_effect_params (Phase 1g). Skip Z3 String here.
                            continue;
                        }
                        // Symbolic argument: verify constraint using Z3 String Sort
                        if let Some(solver) = solver_opt {
                            let param_name =
                                def.params.get(i).map(|p| p.name.as_str()).unwrap_or("arg");
                            // Use a unique counter to distinguish different perform call sites.
                            // Without this, Z3 reuses the same constant for the same name,
                            // incorrectly merging constraints from distinct call sites.
                            static EFFECT_STR_COUNTER: std::sync::atomic::AtomicUsize =
                                std::sync::atomic::AtomicUsize::new(0);
                            let unique_id = EFFECT_STR_COUNTER
                                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            let z3_str_name = format!(
                                "__effect_str_{}_{}_{}_{}",
                                effect, operation, param_name, unique_id
                            );

                            // Plan 10: Use arg_z3_values[i] directly when it has Z3 String Sort.
                            // This enables dynamically constructed strings (e.g., "/tmp/" + var + "/file.txt")
                            // to be directly checked against constraints like starts_with(path, "/tmp/").
                            let param_z3_str = if i < arg_z3_values.len() {
                                if let Some(existing_str) = arg_z3_values[i].as_string() {
                                    // The argument was already evaluated to a Z3 String by expr_to_z3.
                                    // Use it directly — this preserves concat/variable relationships.
                                    existing_str
                                } else {
                                    // Non-string Z3 value: create a fresh Z3 String variable
                                    // and try to connect it to any known string variable in env.
                                    let fresh = Z3String::new_const(ctx, z3_str_name.as_str());
                                    if let Expr::Variable(var_name) = arg {
                                        let str_env_key = format!("__str_{}", var_name);
                                        let found_str = env
                                            .get(&str_env_key)
                                            .and_then(|v| v.as_string())
                                            .or_else(|| {
                                                env.get(var_name).and_then(|v| v.as_string())
                                            });
                                        if let Some(existing_s) = found_str {
                                            solver.assert(&fresh._eq(&existing_s));
                                        }
                                    }
                                    fresh
                                }
                            } else {
                                // Fallback: create a fresh Z3 String variable
                                let fresh = Z3String::new_const(ctx, z3_str_name.as_str());
                                if let Expr::Variable(var_name) = arg {
                                    let str_env_key = format!("__str_{}", var_name);
                                    let found_str = env
                                        .get(&str_env_key)
                                        .and_then(|v| v.as_string())
                                        .or_else(|| env.get(var_name).and_then(|v| v.as_string()));
                                    if let Some(existing_s) = found_str {
                                        solver.assert(&fresh._eq(&existing_s));
                                    }
                                }
                                fresh
                            };

                            // Parse the constraint and assert it
                            if let Some(constraint_bool) =
                                parse_constraint_to_z3_string(ctx, constraint, &param_z3_str)
                            {
                                // Set has_string_constraints flag for sort-aware timeout
                                if let Some(flag) = vc.has_string_constraints {
                                    flag.set(true);
                                }
                                // Check constraint budget
                                if let Some(count) = vc.constraint_count {
                                    let current = count.get();
                                    if current >= vc.constraint_budget {
                                        return Err(MumeiError::verification(format!(
                                            "Constraint budget exceeded for effect '{}' \
                                             string constraint: {} constraints (limit: {})",
                                            effect, current, vc.constraint_budget
                                        ))
                                        .with_verdict(
                                            crate::verification::VerdictKind::Inconclusive,
                                        ));
                                    }
                                    count.set(current + 1);
                                }
                                let track_label = format!(
                                    "track_effect_str_{}_{}_{}_{}",
                                    effect, operation, param_name, unique_id
                                );
                                let track_bool = Bool::new_const(ctx, track_label.as_str());
                                solver.assert_and_track(&constraint_bool, &track_bool);
                                profile_solver_assertion(vc, &track_label, None);
                            }

                            // Store the Z3 String variable in env for downstream use
                            env.insert(z3_str_name, param_z3_str.into());
                        }
                    }
                }
            }

            // Return a symbolic result value.
            // Use Z3 String Sort if the effect has Str-typed parameters,
            // since the operation may return a string (e.g., http_request_path).
            // Otherwise default to Int (status codes, handles, etc.).
            //
            // NOTE: This is a heuristic. Ideally, EffectDef would carry a
            // per-operation return type (e.g., `read -> Str`, `write -> i64`),
            // but the current parser does not record return types for effect
            // operations. Using "any param is Str → result is Str" is a
            // conservative approximation that prevents Z3 Sort mismatches when
            // the perform result is later used in string operations. When the
            // parser gains per-operation return type info, this heuristic
            // should be replaced with a direct lookup.
            //
            // IMPACT: This changes the return type for pre-existing effects
            // with Str params (e.g., HttpGet(url: Str), HttpPost(url: Str))
            // from Int to Z3String. No current code is broken because all
            // atoms discard the perform result (e.g., `perform X.op(url); 1`).
            // Future code that uses the perform result in an integer context
            // (e.g., `let x = perform HttpGet.request(url); x + 1`) would get
            // a Z3 Sort mismatch error.
            let result_name = format!("__perform_{}_{}", effect, operation);
            if nondeterministic_root(vc.module_env, effect).is_some() {
                let result = nondeterministic_perform_result(
                    ctx,
                    &result_name,
                    &arg_z3_values,
                    vc.bitvec_i64,
                );
                env.insert(result_name, result.clone());
                return Ok(result);
            }
            let has_str_params = vc
                .module_env
                .effect_defs
                .get(effect.as_str())
                .or_else(|| vc.module_env.effects.get(effect.as_str()))
                .map(|def| def.params.iter().any(|p| p.type_name == "Str"))
                .unwrap_or(false);
            if has_str_params {
                Ok(Z3String::new_const(ctx, result_name.as_str()).into())
            } else {
                Ok(Int::new_const(ctx, result_name.as_str()).into())
            }
        }

        Expr::Async { body } => {
            // async ブロック: body を非同期コンテキストとして検証する。
            // Z3 上では通常の式として扱い、結果をシンボリック値として返す。
            // await ポイントでの所有権検証は Await 式で行う。
            stmt_to_z3(vc, body, env, solver_opt)
        }
        Expr::Await { expr } => {
            // =============================================================
            // await 跨ぎの安全性検証 (Await Safety Verification)
            // =============================================================
            //
            // await ポイントはコルーチンの中断点であり、以下の安全性を検証する:
            //
            // 1. リソース保持検証 (Resource Held Across Await):
            //    acquire ブロック内で await を呼ぶと、リソースを保持したまま
            //    スレッドが中断される。これはデッドロックの典型パターン。
            //    env 内の __resource_held_* が true のリソースを検出してエラーにする。
            //
            // 2. 所有権一貫性検証 (Ownership Consistency):
            //    await 前に消費済み（__alive_ = false）の変数が、await 後に
            //    アクセスされないことを確認する。Z3 で __alive_ フラグをチェック。

            // --- 1. リソース保持検証 ---
            // env 内の __resource_held_* キーを走査し、Z3 で true かどうかを確認する。
            // acquire ブロック内で await を呼ぶパターンを検出する。
            if let Some(solver) = solver_opt {
                let held_resources: Vec<String> = env
                    .keys()
                    .filter(|k| k.starts_with("__resource_held_"))
                    .cloned()
                    .collect();

                for held_key in &held_resources {
                    let resource_name = held_key
                        .strip_prefix("__resource_held_")
                        .unwrap_or(held_key);
                    if let Some(held_val) = env.get(held_key) {
                        // Z3 で held_val == true が証明可能かチェック
                        // （acquire ブロック内なら held = true が assert されている）
                        if let Some(held_bool) = held_val.as_bool() {
                            solver.push();
                            // held が true であることを仮定し、矛盾がなければ保持中
                            solver.assert(&held_bool);
                            if crate::verification::phase_artifacts::check(solver)
                                != SatResult::Unsat
                            {
                                solver.pop(1);
                                return Err(MumeiError::verification(
                                    format!(
                                        "Unsafe await: resource '{}' is held across an await point. \
                                         This can cause deadlock because the resource lock is not released \
                                         during suspension. Move the await outside the acquire block, or \
                                         release the resource before awaiting.\n  \
                                         Hint: acquire {} {{ ... }}; let val = await expr; // OK\n  \
                                         Bad:  acquire {} {{ let val = await expr; ... }}  // deadlock risk",
                                        resource_name, resource_name, resource_name
                                    )
                                ));
                            }
                            solver.pop(1);
                        }
                    }
                }
            }

            // --- 2. 所有権一貫性検証 ---
            // await 前に消費済みの変数を検出し、Z3 で __alive_ = false を確認する。
            // 消費済み変数が await 後にアクセスされる可能性がある場合、警告する。
            if let Some(solver) = solver_opt {
                let consumed_vars: Vec<String> = env
                    .keys()
                    .filter(|k| k.starts_with("__alive_"))
                    .cloned()
                    .collect();

                for alive_key in &consumed_vars {
                    let var_name = alive_key.strip_prefix("__alive_").unwrap_or(alive_key);
                    if let Some(alive_val) = env.get(alive_key) {
                        if let Some(alive_bool) = alive_val.as_bool() {
                            // __alive_ が false（消費済み）であることを Z3 で確認
                            solver.push();
                            solver.assert(&alive_bool.not()); // alive = false を仮定
                            if crate::verification::phase_artifacts::check(solver) == SatResult::Sat
                            {
                                // 消費済み変数が存在する → await 後のアクセスは use-after-free
                                // await ポイントでの状態をマーク（後続の検証で参照）
                                let await_consumed_key = format!("__await_consumed_{}", var_name);
                                let marker = Bool::from_bool(vc.ctx, true);
                                env.insert(await_consumed_key, marker.into());
                            }
                            solver.pop(1);
                        }
                    }
                }
            }

            // 内側の式を評価してシンボリック結果を返す
            let inner_result = expr_to_z3(vc, expr, env, solver_opt)?;
            Ok(inner_result)
        }

        // =================================================================
        // Higher-Order Functions (Phase A): atom_ref + call
        // =================================================================
        Expr::AtomRef { name } => {
            // atom_ref(some_atom): ModuleEnv から atom 定義を取得し、
            // シンボリック値を生成する。呼び出し先の atom の契約情報は
            // CallRef 時に展開される。
            if vc.module_env.get_atom(name).is_none() {
                return Err(MumeiError::verification(format!(
                    "atom_ref: unknown atom '{}'",
                    name
                )));
            }
            // atom_ref はシンボリックな関数参照として Int 値を生成
            // （実行時は関数ポインタ、Z3 上はシンボリック識別子）
            let ref_name = format!("__atom_ref_{}", name);
            let ref_val = Int::new_const(ctx, ref_name.as_str());
            env.insert(ref_name, ref_val.clone().into());
            Ok(ref_val.into())
        }
        Expr::CallRef { callee, args } => {
            // call(callee_expr, arg1, arg2, ...):
            // callee が AtomRef の場合、参照先の atom の契約を展開して検証する。
            // - requires を呼び出し元のコンテキストで検証
            // - ensures を事実として solver に追加

            // `call(f, …)` where `f` is a `let`-bound lambda — or an inline
            // `call(|a| …, …)` — inlines the lambda body the same way a
            // direct `f(…)` does. A lambda-bound variable shadows the
            // atom_ref/param-contract resolution below.
            let lambda_callee = match callee.as_ref() {
                Expr::Variable(var) => vc.local_lambdas.borrow().get(var).cloned(),
                Expr::Lambda { .. } => Some(std::rc::Rc::new(LocalLambda::Closure {
                    expr: callee.as_ref().clone(),
                    captured: vc.local_lambdas.borrow().clone(),
                })),
                _ => None,
            };
            if let Some(lambda) = lambda_callee {
                return apply_local_lambda(vc, "call", &lambda, args, env, solver_opt);
            }

            // callee を評価
            let _callee_val = expr_to_z3(vc, callee, env, solver_opt)?;

            // callee が AtomRef の場合、参照先の atom 名を取得
            let atom_name = if let Expr::AtomRef { name } = callee.as_ref() {
                Some(name.clone())
            } else if let Expr::Variable(var_name) = callee.as_ref() {
                // 変数が atom_ref として束縛されている場合
                // env から __atom_ref_ プレフィックスで探す
                if env.contains_key(&format!("__atom_ref_{}", var_name)) {
                    Some(var_name.clone())
                } else {
                    None
                }
            } else {
                None
            };

            if let Some(ref callee_name) = atom_name {
                if let Some(callee_atom) = vc.module_env.get_atom(callee_name).cloned() {
                    if callee_atom.params.len() != args.len() {
                        return Err(MumeiError::verification(format!(
                            "Call to '{}': expected {} argument(s), got {}",
                            callee_name,
                            callee_atom.params.len(),
                            args.len()
                        )));
                    }
                    // 引数を Z3 で評価
                    let mut arg_vals = Vec::new();
                    let mut arg_leaf_snapshots = Vec::new();
                    let mut earlier_array_aliases = Vec::new();
                    for (index, arg) in args.iter().enumerate() {
                        reject_later_array_argument_mutation(vc, arg, &earlier_array_aliases)?;
                        let value = expr_to_z3(vc, arg, env, solver_opt)?;
                        if value.as_array().is_some() {
                            let aliases = array_argument_aliases(arg, &value, env);
                            if !aliases.is_empty() {
                                earlier_array_aliases.push(aliases);
                            }
                        }
                        let param_type = callee_atom
                            .params
                            .get(index)
                            .and_then(|param| param.type_name.as_deref());
                        arg_leaf_snapshots.push(super::z3_types::snapshot_struct_array_leaves(
                            vc, env, arg, &value, param_type,
                        )?);
                        arg_vals.push(value);
                    }
                    let recursive_instance = vc.is_instantiating_contract(&callee_atom.name);
                    vc.reject_quantifier_dependent_call(callee_name, &arg_vals)?;
                    reject_shared_array_mutating_call_args(
                        vc,
                        CalleeRef::Expr(callee.as_ref()),
                        args,
                        &arg_vals,
                        env,
                    )?;
                    let cong = vc.callee_congruent(&callee_atom);
                    let scc_internal = cong && scc_internal_call(vc, &callee_atom);

                    // 呼び出し先のパラメータ名に引数をマッピング
                    let mut call_env = env.clone();
                    for (i, param) in callee_atom.params.iter().enumerate() {
                        if let Some(arg_val) = arg_vals.get(i) {
                            call_env.insert(param.name.clone(), arg_val.clone());
                            alias_struct_fields(&mut call_env, &param.name, arg_val);
                            super::z3_types::wire_array_slots_with_struct_leaf_snapshot(
                                vc,
                                &param.name,
                                args.get(i),
                                arg_val,
                                &mut call_env,
                                solver_opt,
                                arg_leaf_snapshots.get(i).and_then(Option::as_ref),
                            )?;
                        }
                    }

                    check_call_argument_domains(
                        vc,
                        solver_opt,
                        callee_name,
                        &callee_atom.params,
                        &arg_vals,
                        &mut call_env,
                    )?;

                    if cong {
                        if let Some(field_vals) = struct_field_values_for_call(
                            vc,
                            &callee_atom,
                            &arg_vals,
                            &call_env,
                            env,
                        ) {
                            vc.reject_quantifier_dependent_call(callee_name, &field_vals)?;
                        }
                    }

                    if cong && !recursive_instance {
                        termination_obligation_at_call(
                            vc,
                            &callee_atom,
                            &mut call_env,
                            env,
                            solver_opt,
                        )?;
                    }

                    // requires を呼び出し元のコンテキストで検証
                    let caller_requires =
                        crate::verification::caller_requires_obligation(&callee_atom);
                    if caller_requires.trim() != "true" && !recursive_instance {
                        let _contract_guard = vc.enter_contract_instantiation(&callee_atom);
                        let req_ast = parse_closed_contract_expression(
                            &caller_requires,
                            "callee precondition",
                        )?;
                        let req_z3 = super::z3_types::with_array_shared_markers_restored(
                            &mut call_env,
                            |env| expr_to_z3_with_domain_obligations(vc, &req_ast, env, None),
                        )?;
                        if let Some(req_bool) = req_z3.as_bool() {
                            if let Some(solver) = solver_opt {
                                solver.push();
                                solver.assert(&req_bool.not());
                                if crate::verification::phase_artifacts::check(solver)
                                    == SatResult::Sat
                                {
                                    solver.pop(1);
                                    return Err(MumeiError::verification(format!(
                                        "call(atom_ref({})): precondition '{}' may not hold at call site",
                                        callee_name, caller_requires
                                    ))
                                    .with_help(
                                        "呼び出し元で事前条件を満たしていません。引数の制約を確認してください",
                                    )
                                    .with_verdict(crate::verification::VerdictKind::Rejected));
                                }
                                solver.pop(1);
                            }
                        }
                    }

                    // Refined parameter types are part of the callee's
                    // contract — the same check the direct-call arm runs:
                    // `atom_ref(need)(5)` for `need(x: Neg)` must not verify.
                    if !recursive_instance {
                        if let Some(solver) = solver_opt {
                            for param in &callee_atom.params {
                                let Some(type_name) = param.type_name.as_deref() else {
                                    continue;
                                };
                                if vc.module_env.get_type(type_name).is_none() {
                                    continue;
                                }
                                let chain =
                                    vc.module_env.refined_chain(type_name).map_err(|cycle| {
                                        MumeiError::type_error_at(cycle, callee_atom.span.clone())
                                    })?;
                                let Some(arg_z3) = call_env.get(&param.name).cloned() else {
                                    continue;
                                };
                                for decl in chain {
                                    let mut local_env = call_env.clone();
                                    local_env.insert(decl.operand.clone(), arg_z3.clone());
                                    let pred_ast = parse_expression(&decl.predicate_raw);
                                    let Some(pred_bool) = refined_predicate_to_z3(
                                        vc,
                                        decl,
                                        &pred_ast,
                                        &mut local_env,
                                        None,
                                    )?
                                    .as_bool() else {
                                        continue;
                                    };
                                    solver.push();
                                    for cond in vc.path_cond_stack.borrow().iter() {
                                        solver.assert(cond);
                                    }
                                    solver.assert(&pred_bool.not());
                                    let refined_check =
                                        crate::verification::phase_artifacts::check(solver);
                                    solver.pop(1);
                                    if refined_check == SatResult::Sat {
                                        return Err(MumeiError::verification(
                                            format!("call(atom_ref({})): refinement on parameter '{}' ({}) may not hold at call site", callee_name, param.name, decl.name)
                                        )
                                        .with_help("呼び出し元で引数の型制約を満たしていません。引数の値を確認してください")
                                        .with_verdict(crate::verification::VerdictKind::Rejected));
                                    }
                                    if refined_check == SatResult::Unknown {
                                        return Err(MumeiError::verification(
                                            format!("call(atom_ref({})): refinement on parameter '{}' ({}) could not be decided", callee_name, param.name, decl.name)
                                        )
                                        .with_verdict(crate::verification::VerdictKind::Inconclusive));
                                    }
                                }
                            }
                        }
                    }

                    // ensures を事実として solver に追加（Equality Ensures Propagation）
                    static CALL_REF_COUNTER: std::sync::atomic::AtomicUsize =
                        std::sync::atomic::AtomicUsize::new(0);
                    let call_id =
                        CALL_REF_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let result_name = format!("call_ref_{}_{}", callee_name, call_id);
                    let result_type = callee_atom.return_type.as_deref();
                    let _result_array_type_guard =
                        LocalArrayElementTypeGuard::for_call_result(vc, result_type);
                    if let Some(type_name) = result_type {
                        super::z3_types::validate_enum_array_type(vc, type_name)?;
                    }
                    let rec_args = rec_fn_args(vc, &callee_atom, &arg_vals, &call_env);
                    let use_rec_fn = cong
                        && callee_semantics_match_caller(vc, &callee_atom)
                        && rec_args.is_some();
                    // Same expanded-signature retention as the `Expr::Call`
                    // arm: the result's `#len`/`#field.*` projections share
                    // the callee's domain.
                    let rec_signature: Option<(Vec<Dynamic>, Vec<Sort>)> = if use_rec_fn {
                        let rec_args = rec_args.expect("recursive arguments checked above");
                        let domain = rec_args.iter().map(|v| v.get_sort()).collect();
                        Some((rec_args, domain))
                    } else {
                        None
                    };
                    let struct_return = result_type.and_then(|t| vc.module_env.get_struct(t));
                    let result_z3: Dynamic = if let Some((rec_args, domain)) = &rec_signature {
                        if struct_return.is_some() {
                            // Field-wise congruent struct result: the handle
                            // stays a per-call const; fields are projection
                            // UFs bound below.
                            datatype::param_z3_value_for_vc(vc, result_name.as_str(), result_type)
                        } else {
                            let range = call_result_sort(vc, &callee_atom);
                            let decl = vc.rec_fn(&callee_atom, domain, &range);
                            let app_args: Vec<&dyn Ast> =
                                rec_args.iter().map(|v| v as &dyn Ast).collect();
                            let application = decl.apply(&app_args);
                            if let Some(sink) = vc.recursion.unfold_sink.borrow_mut().as_mut() {
                                sink.push((callee_atom.name.clone(), arg_vals.clone()));
                            }
                            application
                        }
                    } else {
                        datatype::param_z3_value_for_vc(vc, result_name.as_str(), result_type)
                    };

                    // A widened-integer enum payload leaf
                    // (`u64`/`u32`/`i32`) keeps its declared width on a
                    // call result too — the result const is minted fresh
                    // here, so the param-time domain asserts cannot reach
                    // it.
                    if let (Some(solver), Some(bound)) =
                        (solver_opt, datatype::enum_value_leaf_domain(vc, &result_z3))
                    {
                        solver.assert(&bound);
                    }

                    // Array results on the congruent path mint their `len`
                    // through the `#len` projection UF (the (contents, len)
                    // pair). The non-congruent reference-call path keeps its
                    // historical shape: no tracked result length.
                    let result_struct_arrays = if let Some(result_array) = result_z3.as_array() {
                        super::z3_types::call_result_struct_arrays(
                            vc,
                            &result_array,
                            result_type,
                            &result_name,
                        )?
                    } else {
                        std::collections::HashMap::new()
                    };
                    let result_len: Option<Dynamic> =
                        if let Some((rec_args, domain)) = &rec_signature {
                            result_z3.as_array().map(|_| {
                                let len_sort =
                                    array_len_symbol(vc.ctx, "probe", vc.bitvec_i64).get_sort();
                                let len_sym = rec_fn_result_projection(
                                    vc,
                                    &callee_atom,
                                    REC_FN_ARRAY_LEN_PROJECTION,
                                    domain,
                                    &len_sort,
                                    rec_args,
                                );
                                if let (Some(solver), Some(nn)) =
                                    (solver_opt, nonneg_constraint(ctx, &len_sym))
                                {
                                    solver.assert(&nn);
                                }
                                len_sym
                            })
                        } else {
                            None
                        };
                    if let Some(result_array) = result_z3.as_array() {
                        vc.call_result_lens.borrow_mut().insert(
                            super::z3_types::array_root_ast(&result_array) as usize,
                            super::context::CallResultArray {
                                len: result_len.clone(),
                                _keep: result_z3.clone(),
                                struct_leaves: result_struct_arrays.clone(),
                                struct_element_type: None,
                            },
                        );
                    }

                    // 構造体を返す参照呼び出し: 通常呼び出しと同様に結果のフィールドを
                    // シンボル化し、呼び出し先が保証する不変量を仮定する。
                    if let Some(sdef) = struct_return {
                        let result_fields: Vec<(String, Dynamic)> =
                            if let Some((rec_args, domain)) = &rec_signature {
                                sdef.fields
                                    .iter()
                                    .map(|field| {
                                        let range = param_sort_probe(
                                            vc,
                                            &format!(
                                                "__recfn_field_{}_{}",
                                                callee_atom.name, field.name
                                            ),
                                            Some(field.type_name.trim()),
                                        );
                                        (
                                            field.name.clone(),
                                            rec_fn_result_projection(
                                                vc,
                                                &callee_atom,
                                                &format!("field.{}", field.name),
                                                domain,
                                                &range,
                                                rec_args,
                                            ),
                                        )
                                    })
                                    .collect()
                            } else {
                                seed_struct_fields(vc, env, &result_name, sdef)
                            };
                        if rec_signature.is_some() {
                            bind_struct_fields(env, &result_name, &result_fields);
                        }
                        bind_struct_fields(&mut call_env, &result_name, &result_fields);
                        bind_struct_fields(&mut call_env, "result", &result_fields);
                        // Same nested-leaf re-home as the other call arms:
                        // `call_env` predates the `seed_struct_fields` run
                        // on env, so `result.inner.a` in the callee's
                        // ensures would otherwise see a fresh const.
                        // Deep leaves rebind too — a shallow bind leaves
                        // `result.inner.b.c` reading pre-write symbols.
                        for (name, value) in &result_fields {
                            let fields = nested_field_bindings(env, value);
                            if !fields.is_empty() {
                                bind_struct_fields(
                                    &mut call_env,
                                    &format!("result_{name}"),
                                    &fields,
                                );
                            }
                        }
                        register_call_result_fields(vc, env, &result_z3, &result_fields);
                        if let (Some(solver), true) = (
                            solver_opt,
                            !recursive_instance && callee_semantics_match_caller(vc, &callee_atom),
                        ) {
                            let _contract_guard = vc.enter_contract_instantiation(&callee_atom);
                            if rec_signature.is_some() {
                                assume_struct_contract(
                                    vc,
                                    solver,
                                    sdef,
                                    &result_name,
                                    &result_fields,
                                    &call_env,
                                    None,
                                )?;
                            } else {
                                assume_struct_invariants(
                                    vc,
                                    solver,
                                    sdef,
                                    &result_name,
                                    &result_fields,
                                    &call_env,
                                    None,
                                )?;
                            }
                        }
                    }

                    // Enum-typed fields inside a struct call result are
                    // fresh datatype consts — bound their widened integer
                    // leaves the same way params are bounded.
                    if let Some(solver) = solver_opt {
                        datatype::assert_enum_value_domains(vc, solver, env);
                    }

                    // A refined return type is part of the callee's
                    // contract: the caller may assume every conjunct on the
                    // call's symbolic result — the same shape the
                    // direct-call arm applies. The encodings must agree:
                    // a predicate proved under the callee's Int arithmetic
                    // cannot be imported under caller bit-vector semantics.
                    if !recursive_instance && callee_semantics_match_caller(vc, &callee_atom) {
                        if let Some(return_type) = callee_atom.return_type.as_deref() {
                            if vc.module_env.get_type(return_type).is_some() {
                                let chain =
                                    vc.module_env.refined_chain(return_type).map_err(|cycle| {
                                        MumeiError::type_error_at(cycle, callee_atom.span.clone())
                                    })?;
                                for decl in chain {
                                    let mut local_env = call_env.clone();
                                    local_env.insert(decl.operand.clone(), result_z3.clone());
                                    let pred_ast = parse_expression(&decl.predicate_raw);
                                    let Some(pred_bool) = refined_predicate_to_z3(
                                        vc,
                                        decl,
                                        &pred_ast,
                                        &mut local_env,
                                        None,
                                    )?
                                    .as_bool() else {
                                        continue;
                                    };
                                    if let Some(solver) = solver_opt {
                                        if use_rec_fn {
                                            let ante = congruent_ensures_antecedent(
                                                vc,
                                                &callee_atom,
                                                &caller_requires,
                                                &mut call_env,
                                                scc_internal,
                                            )?;
                                            solver.assert(&ante.implies(&pred_bool));
                                        } else {
                                            solver.assert(&pred_bool);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    let caller_ensures = crate::verification::contract_view(
                        &callee_atom,
                        crate::verification::ContractView::CallerEnsures,
                    );
                    if caller_ensures.trim() != "true" && !recursive_instance {
                        let _contract_guard = vc.enter_contract_instantiation(&callee_atom);
                        call_env.insert("result".to_string(), result_z3.clone());
                        for (path, leaf_array) in &result_struct_arrays {
                            call_env.insert(
                                super::z3_types::struct_array_leaf_key("result", path),
                                leaf_array.clone().into(),
                            );
                        }
                        if let Some(len_sym) = &result_len {
                            call_env.insert("__z3_arr_result".to_string(), result_z3.clone());
                            call_env.insert(array_len_key("result"), len_sym.clone());
                        }
                        // `ensures` reads post-write fields — freshen
                        // the callee's written composite params so its
                        // clauses don't assert on the caller's
                        // input-time slots. The recursion antecedent
                        // below must still see the input fields — keep
                        // a pre-havoc copy.
                        let mut pre_havoc_env = call_env.clone();
                        havoc_written_callee_fields(vc, &callee_atom, &mut call_env);
                        let ens_ast = parse_expression(&caller_ensures);
                        if use_rec_fn {
                            let unfold_key = if solver_opt.is_some()
                                && !recursive_instance
                                && vc.recursion.unfold_sink.borrow().is_none()
                                && caller_outside_scc(vc, &callee_atom)
                            {
                                constant_args(&arg_vals).map(|(_, key)| key)
                            } else {
                                None
                            };
                            let unfold_guard =
                                unfold_key.as_ref().map(|_| UnfoldSinkGuard::new(vc));
                            let ens_z3 = super::z3_types::with_array_shared_markers_restored(
                                &mut call_env,
                                |env| expr_to_z3_with_domain_obligations(vc, &ens_ast, env, None),
                            )?;
                            if let (Some(ens_bool), Some(solver)) = (ens_z3.as_bool(), solver_opt) {
                                let ante = congruent_ensures_antecedent(
                                    vc,
                                    &callee_atom,
                                    &caller_requires,
                                    &mut pre_havoc_env,
                                    scc_internal,
                                )?;
                                solver.assert(&ante.implies(&ens_bool));
                                if let (Some(unfold_key), Some(unfold_guard)) =
                                    (unfold_key.as_ref(), unfold_guard.as_ref())
                                {
                                    let frontier = unfold_guard.take_frontier();
                                    let mut seen = std::collections::HashSet::new();
                                    seen.insert(constant_call_key(&callee_atom.name, unfold_key));
                                    unfold_constant_calls(vc, env, frontier, seen, solver);
                                }
                            }
                        } else {
                            let ens_z3 = super::z3_types::with_array_shared_markers_restored(
                                &mut call_env,
                                |env| expr_to_z3_with_domain_obligations(vc, &ens_ast, env, None),
                            )?;
                            if let Some(ens_bool) = ens_z3.as_bool() {
                                if let Some(solver) = solver_opt {
                                    solver.assert(&ens_bool);
                                }
                            }

                            // Equality ensures の特別処理
                            if let Expr::BinaryOp(left, Op::Eq, right) = &ens_ast {
                                if let Expr::Variable(ref var_name) = left.as_ref() {
                                    if var_name == "result" {
                                        if let Ok(rhs_val) =
                                            super::z3_types::with_array_shared_markers_restored(
                                                &mut call_env,
                                                |env| expr_to_z3(vc, right, env, None),
                                            )
                                        {
                                            if let Some(solver) = solver_opt {
                                                if let (Some(res_int), Some(rhs_int)) =
                                                    (result_z3.as_int(), rhs_val.as_int())
                                                {
                                                    solver.assert(&res_int._eq(&rhs_int));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // `[T]` args share the caller's data pointer — havoc
                    // the tracked chain so post-call reads can't claim
                    // pre-call elements the callee may have overwritten.
                    havoc_array_args(vc, &callee_atom, args, env);
                    return Ok(result_z3);
                }
            }

            // =============================================================
            // Phase B: call_with_contract — パラメトリック関数型の契約展開
            // =============================================================
            // callee が Variable で、current_atom のパラメータに contract(f) が
            // 宣言されている場合、その契約を使って結果を制約する。
            // これにより trusted マーカーなしで高階関数を検証できる。

            let mut arg_vals = Vec::new();
            for arg in args {
                arg_vals.push(expr_to_z3(vc, arg, env, solver_opt)?);
            }
            vc.reject_quantifier_dependent_call("call", &arg_vals)?;
            check_function_param_call_domains(vc, solver_opt, callee, args, &arg_vals, env)?;
            reject_shared_array_mutating_call_args(
                vc,
                CalleeRef::Expr(callee.as_ref()),
                args,
                &arg_vals,
                env,
            )?;

            // The callee didn't resolve to a known atom — a concrete
            // `atom_ref` target or contract callee may still store through
            // its params, and nothing above models that mutation. Havoc
            // the shared may-write set (every variable arg on this dynamic
            // path — fail closed); `havoc_array_name` no-ops on non-array
            // bindings.
            for name in
                super::may_write::callee_may_write_args(vc, CalleeRef::Expr(callee), args, env)
            {
                havoc_array_name(vc, &name, env);
            }

            static DYNAMIC_CALL_COUNTER: std::sync::atomic::AtomicUsize =
                std::sync::atomic::AtomicUsize::new(0);
            let id = DYNAMIC_CALL_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let result: Dynamic = Int::new_const(ctx, format!("call_ref_dynamic_{}", id)).into();

            // callee が Variable の場合、current_atom のパラメータから contract 情報を取得
            if let Expr::Variable(callee_var_name) = callee.as_ref() {
                if let Some(current_atom) = vc.current_atom {
                    if let Some(param) = current_atom
                        .params
                        .iter()
                        .find(|p| p.name == *callee_var_name)
                    {
                        // Check either clause when a callback contract is declared.
                        if param.fn_contract_ensures.is_some()
                            || param.fn_contract_requires.is_some()
                        {
                            let mut contract_env = env.clone();

                            // atom_ref のパラメータ型情報から引数名を生成
                            // atom_ref(i64) -> i64 の場合、arg0 として引数をマッピング
                            // atom_ref(i64, i64) -> i64 の場合、arg0, arg1 として引数をマッピング
                            for (i, arg_val) in arg_vals.iter().enumerate() {
                                contract_env.insert(format!("arg{}", i), arg_val.clone());
                            }

                            // 最初の引数を "x" としてもマッピング（よくある1引数パターン用）
                            if let Some(first_arg) = arg_vals.first() {
                                contract_env.insert("x".to_string(), first_arg.clone());
                            }
                            // 2引数の場合 "y" もマッピング
                            if let Some(second_arg) = arg_vals.get(1) {
                                contract_env.insert("y".to_string(), second_arg.clone());
                            }

                            // result をマッピング
                            contract_env.insert("result".to_string(), result.clone());

                            // requires の検証（宣言されている場合）
                            if let Some(ref fn_requires) = param.fn_contract_requires {
                                if fn_requires.trim() != "true" {
                                    let req_ast = parse_expression(fn_requires);
                                    let req_z3 =
                                        super::z3_types::with_array_shared_markers_restored(
                                            &mut contract_env,
                                            |env| {
                                                expr_to_z3_with_domain_obligations(
                                                    vc, &req_ast, env, None,
                                                )
                                            },
                                        )
                                        .map_err(|e| MumeiError::verification(format!(
                                            "call_with_contract({}): failed to evaluate requires '{}': {}",
                                            callee_var_name, fn_requires, e
                                        )))?;
                                    if let Some(req_bool) = req_z3.as_bool() {
                                        if let Some(solver) = solver_opt {
                                            solver.push();
                                            solver.assert(&req_bool.not());
                                            let sat_result =
                                                crate::verification::phase_artifacts::check(solver);
                                            solver.pop(1);
                                            if sat_result == SatResult::Sat {
                                                return Err(MumeiError::verification(format!(
                                                    "call_with_contract({}): precondition '{}' may not hold at call site",
                                                    callee_var_name, fn_requires
                                                ))
                                                .with_help(
                                                    "関数パラメータの事前条件を満たしていません。引数の制約を確認してください",
                                                )
                                                .with_verdict(crate::verification::VerdictKind::Rejected));
                                            }
                                            if sat_result == SatResult::Unknown {
                                                return Err(MumeiError::verification(format!(
                                                    "call_with_contract({}): precondition '{}' could not be decided (Z3 unknown)",
                                                    callee_var_name, fn_requires
                                                ))
                                                .with_verdict(crate::verification::VerdictKind::Inconclusive));
                                            }
                                        }
                                    }
                                }
                            }

                            // ensures を事実として solver に追加
                            if let Some(fn_ensures) = param.fn_contract_ensures.as_deref() {
                                if fn_ensures.trim() != "true" {
                                    let ens_ast = parse_expression(fn_ensures);
                                    let ens_z3 =
                                        super::z3_types::with_array_shared_markers_restored(
                                            &mut contract_env,
                                            |env| {
                                                expr_to_z3_with_domain_obligations(
                                                    vc, &ens_ast, env, None,
                                                )
                                            },
                                        )
                                    .map_err(|e| MumeiError::verification(format!(
                                        "call_with_contract({}): failed to evaluate ensures '{}': {}",
                                        callee_var_name, fn_ensures, e
                                    )))?;
                                    if let Some(ens_bool) = ens_z3.as_bool() {
                                        if let Some(solver) = solver_opt {
                                            solver.assert(&ens_bool);
                                            profile_solver_assertion(
                                                vc,
                                                &format!(
                                                    "call_with_contract_{}_ensures",
                                                    callee_var_name
                                                ),
                                                None,
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            Ok(result)
        }

        Expr::FieldAccess(inner_expr, field_name) => {
            let access = Expr::FieldAccess(inner_expr.clone(), field_name.clone());
            if let Some((array_name, index, field_path)) = struct_array_access_path(&access) {
                if let Some(value) =
                    read_struct_array_field(vc, &array_name, &index, &field_path, env, solver_opt)?
                {
                    return Ok(value);
                }
            }
            if let Expr::Variable(name) = inner_expr.as_ref() {
                if vc.module_env.resource_field(name, field_name).is_some() {
                    let key = format!("{name}.{field_name}");
                    return env.get(&key).cloned().ok_or_else(|| {
                        MumeiError::verification(format!(
                            "shared state '{key}' accessed outside 'acquire {name}'"
                        ))
                    });
                }
            }
            if let Expr::Variable(name) = inner_expr.as_ref() {
                if name == "result" && env.contains_key(&tuple_result_arity_key(name)) {
                    let Some(index) = field_name
                        .strip_prefix('_')
                        .and_then(|value| value.parse::<usize>().ok())
                    else {
                        return Err(MumeiError::verification(format!(
                            "{UNSUPPORTED_TUPLE_RESULT_INDEXING} field must be a non-negative integer constant"
                        )));
                    };
                    let arity = env
                        .get(&tuple_result_arity_key(name))
                        .and_then(|value| value.as_int())
                        .and_then(|value| value.as_i64())
                        .and_then(|value| usize::try_from(value).ok())
                        .unwrap_or(0);
                    if index >= arity {
                        return Err(MumeiError::verification(format!(
                            "{UNSUPPORTED_TUPLE_RESULT_INDEXING} index is out of range"
                        )));
                    }
                    let key = tuple_result_component_key(name, index);
                    return env.get(&key).cloned().ok_or_else(|| {
                        MumeiError::verification(format!(
                            "{UNSUPPORTED_TUPLE_RESULT_INDEXING} component is unavailable"
                        ))
                    });
                }
            }

            // P10-C: `Enum::Variant` (nullary) parses as
            // `FieldAccess(Variable("Enum"), "Variant")` — construct the
            // datatype value. A bound variable named `Enum` wins (shadowing),
            // and `.`-spelled accesses to an enum name are equivalent here.
            if let Expr::Variable(name) = inner_expr.as_ref() {
                if !env.contains_key(name)
                    && vc.module_env.enums.contains_key(name)
                    && datatype::uses_datatype_sort(
                        vc.module_env.get_enum(name).unwrap(),
                        vc.module_env,
                    )
                {
                    if let Some(value) = datatype::enum_ctor_apply(
                        vc,
                        env,
                        &format!("{}::{}", name, field_name),
                        &[],
                        &[],
                        solver_opt,
                    )? {
                        return Ok(value);
                    }
                }
                // Non-finite enums (recursive / generic / non-scalar payload /
                // empty) keep the Int-tag encoding: `Enum::Variant` on them
                // denotes the variant's tag index, so `l == IntList::Nil`
                // means `l == 0`. Previously this fell through to a fresh
                // unconstrained const, making the comparison vacuous.
                if !env.contains_key(name) {
                    if let Some(ed) = vc.module_env.get_enum(name) {
                        if let Some(idx) = ed.variants.iter().position(|v| v.name == *field_name) {
                            return Ok(Int::from_i64(ctx, idx as i64).into());
                        }
                        // `E` names a known enum but `V` is not one of its
                        // variants — a typo'd variant must not degrade to a
                        // fresh const (vacuous comparison).
                        return Err(MumeiError::verification(format!(
                            "Enum '{name}' has no variant named '{field_name}'"
                        )));
                    }
                }
            }

            // `.tag` on a value declared as an enum reads the variant index.
            // On the finite-ADT encoding it folds the variant testers into a
            // nested ite; on the Int-tag encoding (recursive / generic /
            // non-scalar payload / empty) the value already *is* the tag, so
            // the read returns it unchanged. Runs before the struct-field
            // path so a struct field literally named `tag` still wins on
            // struct-typed receivers (infer_expr_enum_name yields None).
            if field_name == "tag" {
                if let Some(enum_name) = datatype::infer_expr_enum_name(vc, inner_expr) {
                    // Prefer the instantiated declaration: a `Wrap<i64>`
                    // value carries the monomorphized enum's datatype sort,
                    // while the base name resolves to the generic def that
                    // never builds one — reading `base` as the Int-tag would
                    // hand a datatype term to an integer comparison.
                    let enum_def = datatype::declared_type_of_expr(vc, inner_expr)
                        .and_then(|t| vc.module_env.get_enum(&t))
                        .or_else(|| vc.module_env.get_enum(&enum_name));
                    if let Some(enum_def) = enum_def {
                        let base = expr_to_z3(vc, inner_expr, env, solver_opt)?;
                        let sort = datatype::enum_datatype_sort(vc, enum_def);
                        match (sort, base.as_datatype()) {
                            (None, None) => {
                                // Int-tag encoding: the value already is the tag.
                                return Ok(base);
                            }
                            (Some(sort), Some(dt)) => {
                                let n = enum_def.variants.len();
                                let mut tag = Int::from_i64(ctx, (n - 1) as i64);
                                for idx in (0..n - 1).rev() {
                                    let is_v = sort.variants[idx]
                                        .tester
                                        .apply(&[&dt])
                                        .as_bool()
                                        .ok_or_else(|| {
                                            MumeiError::verification(format!(
                                                "enum '{enum_name}' tester is not boolean"
                                            ))
                                        })?;
                                    tag = is_v.ite(&Int::from_i64(ctx, idx as i64), &tag);
                                }
                                return Ok(tag.into());
                            }
                            // Declared finite-ADT but the env value is not a
                            // datatype, or a datatype value whose def has no
                            // sort (generic base): fall through to the
                            // generic field resolution rather than returning
                            // a non-tag term.
                            _ => {}
                        }
                    }
                }
            }

            if field_name != "tag" {
                if let Some(value) =
                    enum_payload_field_read(vc, inner_expr, field_name, env, solver_opt)?
                {
                    return Ok(value);
                }
            }

            // ネスト構造体のフィールドアクセスを再帰的に解決する。
            //
            // 1段階: v.x → env["__struct_v_x"] or env["v_x"]
            // 2段階: v.point.x → まず v.point を解決し、その結果から .x を解決
            //
            // 解決戦略:
            // A. 内側の式が Variable の場合: 直接 env から探す
            // B. 内側の式が FieldAccess の場合: 再帰的に解決し、
            //    結果のパスを使って env から探す
            // C. どちらでもない場合: 式を評価してシンボリック変数を生成

            // フラットなパス文字列を構築するヘルパー
            // v.point.x → "v_point_x" のようなパスを生成
            fn build_field_path(expr: &Expr) -> Option<Vec<String>> {
                match expr {
                    Expr::Variable(name) => Some(vec![name.clone()]),
                    Expr::FieldAccess(inner, field) => {
                        let mut path = build_field_path(inner)?;
                        path.push(field.clone());
                        Some(path)
                    }
                    _ => None,
                }
            }

            // 完全なフィールドパスを構築（例: ["v", "point", "x"]）
            let full_path = {
                let mut path = build_field_path(inner_expr).unwrap_or_default();
                path.push(field_name.clone());
                path
            };

            if full_path.len() >= 2 {
                // パスの各プレフィックスで env を探索
                // 例: ["v", "point", "x"] → "v_point_x", "__struct_v_point_x"
                let underscore_path = full_path.join("_");
                let struct_path = format!("__struct_{}", underscore_path);

                // 直接パスで見つかればそれを返す
                if let Some(val) = env.get(&struct_path) {
                    return Ok(val.clone());
                }
                if let Some(val) = env.get(&underscore_path) {
                    return Ok(val.clone());
                }

                // 1段階ずつ解決を試みる
                // 例: v.point → env["__struct_v_point"] or env["v_point"]
                //     その結果が構造体型なら、.x のフィールドをさらに解決
                if full_path.len() == 2 {
                    // 単純な1段階アクセス: v.x
                    let var_name = &full_path[0];
                    let candidates = [
                        format!("__struct_{}_{}", var_name, field_name),
                        format!("{}_{}", var_name, field_name),
                    ];
                    for candidate in &candidates {
                        if let Some(val) = env.get(candidate) {
                            return Ok(val.clone());
                        }
                    }
                }

                // ネスト構造体の再帰解決:
                // 内側の式を先に Z3 で評価し、結果を env に登録してからフィールドを解決
                let base_val = expr_to_z3(vc, inner_expr, env, solver_opt)?;

                // 内側が構造体ハンドル（StructInit・パラメータ・let
                // エイリアスが残す `__mumei_struct_*` 定数、またはその
                // ite）の場合、そのハンドル名で平坦化された
                // `__struct_<handle>_<field>` エントリを射影する。
                // これが無いと `o.inner.a` は未拘束の fresh const に
                // 退化し、リテラル初期化のフィールド値を読めない。
                if let Some(fields) = project_struct_fields(env, &base_val) {
                    if let Some((_, value)) = fields.iter().find(|(name, _)| name == field_name) {
                        return Ok(value.clone());
                    }
                }

                // 内側の式の型を推定し、構造体定義からフィールドの型を取得
                // フィールドの精緻型制約も再帰的に適用する
                let nested_sym_name = format!(
                    "{}_{}",
                    underscore_path
                        .rsplit_once('_')
                        .map(|(prefix, _)| prefix)
                        .unwrap_or(&underscore_path),
                    field_name
                );
                let sym = if let Some(val) = env.get(&nested_sym_name) {
                    return Ok(val.clone());
                } else {
                    let s = Int::new_const(ctx, full_path.join("_").as_str());
                    env.insert(full_path.join("_"), s.clone().into());
                    s
                };
                Ok(sym.into())
            } else {
                // パスが構築できない場合: 式を評価してシンボリック変数を生成。
                // 構造体を返す呼び出し (`f(1).x`) なら、結果ハンドルの ast を
                // キーに `call_result_fields` を引く — さもなければ
                // `field_<x>` が全呼び出しで共有され、`f(1).x == f(2).x`
                // が偽の等式として通ってしまう。
                let base = expr_to_z3(vc, inner_expr, env, solver_opt)?;
                if let Some((fields, _keep)) = vc
                    .call_result_fields
                    .borrow()
                    .get(&(base.get_z3_ast() as usize))
                {
                    if let Some((_, value)) = fields.iter().find(|(name, _)| name == field_name) {
                        return Ok(value.clone());
                    }
                }
                // A bound handle's fields are in the env under
                // `__struct_<handle>_<field>` — project them so a deep
                // read like `f(1).inner.a` finds `a` instead of
                // falling to the fallback.
                if let Some(fields) = project_struct_fields(env, &base) {
                    if let Some((_, value)) = fields.iter().find(|(name, _)| name == field_name) {
                        return Ok(value.clone());
                    }
                }
                // Last resort: a constant keyed by the base value's own
                // ast — NOT a shared `field_<name>`, which would make
                // `f(1).inner.a` and `f(2).inner.a` resolve to one
                // symbol and prove them equal. Equal bases (the same
                // call result, or the same UF application) still share
                // the symbol.
                let sym = Int::new_const(
                    ctx,
                    format!("field_{}_{}", field_name, base.get_z3_ast() as usize),
                );
                Ok(sym.into())
            }
        }
        Expr::Lambda { params, body, .. } => lambda_expr_to_z3(vc, params, body, env, solver_opt),
        // Plan 8: Channel send — evaluate channel and value, return unit
        Expr::ChanSend { channel, value } => {
            let _ch = expr_to_z3(vc, channel, env, solver_opt)?;
            let _val = expr_to_z3(vc, value, env, solver_opt)?;
            Ok(Int::from_i64(ctx, 0).into())
        }
        // Plan 8: Channel recv — evaluate channel, return symbolic int
        Expr::ChanRecv { channel } => {
            let _ch = expr_to_z3(vc, channel, env, solver_opt)?;
            static RECV_COUNTER: std::sync::atomic::AtomicUsize =
                std::sync::atomic::AtomicUsize::new(0);
            let recv_id = RECV_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let recv_sym = Int::new_const(ctx, format!("__chan_recv_{}", recv_id).as_str());
            Ok(recv_sym.into())
        }
    }
}

#[inline(never)]
fn check_function_param_call_domains<'a>(
    vc: &VCtx<'a>,
    solver_opt: Option<&Solver<'a>>,
    callee: &Expr,
    args: &[Expr],
    arg_vals: &[Dynamic<'a>],
    env: &Env<'a>,
) -> MumeiResult<()> {
    let Expr::Variable(callee_var_name) = callee else {
        return Ok(());
    };
    let Some(param) = vc.current_atom.and_then(|atom| {
        atom.params
            .iter()
            .find(|param| param.name == *callee_var_name)
    }) else {
        return Ok(());
    };
    let function_type = param
        .type_ref
        .as_ref()
        .filter(|ty| ty.is_fn_type())
        .cloned()
        .or_else(|| {
            param.type_name.as_deref().and_then(|name| {
                let resolved = vc.module_env.resolve_base_type(name);
                let parsed = crate::parser::parse_type_ref(&resolved);
                parsed.is_fn_type().then_some(parsed)
            })
        });
    let Some(function_type) = function_type else {
        return Err(MumeiError::verification(format!(
            "Call through function parameter '{callee_var_name}' has no typed signature to check argument domains"
        ))
        .with_verdict(crate::verification::VerdictKind::Rejected));
    };
    let param_types = function_type.fn_param_types().unwrap_or_default();
    if param_types.len() != arg_vals.len() {
        return Err(MumeiError::verification(format!(
            "Call to function parameter '{callee_var_name}' expects {} argument(s), got {}",
            param_types.len(),
            arg_vals.len()
        ))
        .with_verdict(crate::verification::VerdictKind::Rejected));
    }
    let mut callback_env = env.clone();
    for (index, (param_type, value)) in param_types.iter().zip(arg_vals).enumerate() {
        let arg_name = format!("arg{index}");
        callback_env.insert(arg_name.clone(), value.clone());
        alias_struct_fields(&mut callback_env, &arg_name, value);
        if value.as_array().is_some() {
            let length = super::z3_types::array_len_for_expr(
                vc,
                &mut callback_env,
                args.get(index),
                value,
                solver_opt,
            );
            callback_env.insert(super::z3_types::array_len_key(&arg_name), length);
        }
        check_lambda_call_argument_domain(
            vc,
            solver_opt,
            callee_var_name,
            &arg_name,
            &param_type.display_name(),
            value,
            &mut callback_env,
        )?;
    }
    Ok(())
}

#[inline(never)]
fn lambda_expr_to_z3<'a>(
    vc: &VCtx<'a>,
    params: &[crate::parser::ast::LambdaParam],
    body: &crate::parser::ast::Stmt,
    env: &mut Env<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> MumeiResult<Dynamic<'a>> {
    let ctx = vc.ctx;
    static LAMBDA_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let lambda_id = LAMBDA_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let lambda_name = format!("__lambda_{}_{}", params.len(), lambda_id);
    let lambda_sym = Int::new_const(ctx, lambda_name.as_str());

    let mut lambda_env = env.clone();
    for p in params {
        if let Some(type_name) = p.type_ref.as_ref().map(|ty| ty.display_name()) {
            super::z3_types::validate_enum_array_type(vc, &type_name)?;
        }
    }
    let int_types_before = vc.local_int_types.borrow().clone();
    let array_elem_types_before = vc.local_array_elem_types.borrow().clone();
    let struct_types_before = vc.local_struct_types.borrow().clone();
    let enum_types_before = vc.local_enum_types.borrow().clone();
    for (param_index, p) in params.iter().enumerate() {
        super::z3_types::clear_array_shared(ctx, &mut lambda_env, &p.name);
        let type_name = p.type_ref.as_ref().map(|ty| ty.display_name());
        let lambda_param_name = format!("__mumei_lambda_param_{lambda_id}_{param_index}");
        let p_sym = datatype::param_z3_value_for_vc(vc, &lambda_param_name, type_name.as_deref());
        lambda_env.insert(p.name.clone(), p_sym.clone());
        lambda_env.remove(&format!("__z3_arr_{}", p.name));
        lambda_env.remove(&super::z3_types::array_len_key(&p.name));
        lambda_env.remove(&super::z3_types::array_root_len_key(&p.name));
        let struct_array_prefix = format!("__struct_arr#{}#", p.name);
        lambda_env.retain(|key, _| !key.starts_with(&struct_array_prefix));
        vc.local_int_types.borrow_mut().remove(&p.name);
        vc.local_array_elem_types.borrow_mut().remove(&p.name);
        vc.local_struct_types.borrow_mut().remove(&p.name);
        vc.local_enum_types.borrow_mut().remove(&p.name);
        let Some(type_ref) = p.type_ref.as_ref() else {
            continue;
        };
        let resolved_type = vc.module_env.resolve_base_type(&type_ref.display_name());
        if let Some((int_type, _, _)) =
            fixed_width_int(&vc.module_env.resolve_base_type(&type_ref.name))
        {
            vc.local_int_types
                .borrow_mut()
                .insert(p.name.clone(), int_type);
        }
        if let Some(element_type) = type_ref.array_element_type() {
            let length = super::z3_types::array_root_len_value(
                ctx,
                &mut lambda_env,
                &lambda_param_name,
                vc.bitvec_i64,
                None,
            );
            lambda_env.insert(super::z3_types::array_len_key(&p.name), length.clone());
            lambda_env.insert(super::z3_types::array_root_len_key(&p.name), length.clone());
            lambda_env.insert(format!("__z3_arr_{}", p.name), p_sym.clone());
            if let Some(array) = p_sym.as_array() {
                super::z3_types::register_array_value_len(&mut lambda_env, &array, length);
            }
            vc.local_array_elem_types
                .borrow_mut()
                .insert(p.name.clone(), element_type.display_name());
        }
        if let Some(struct_def) = vc.module_env.get_struct(&resolved_type) {
            let struct_field_prefix = format!("__struct_{}_", p.name);
            let stale_fields: Vec<String> = lambda_env
                .keys()
                .filter_map(|key| key.strip_prefix(&struct_field_prefix).map(str::to_string))
                .collect();
            for field in stale_fields {
                lambda_env.remove(&struct_field_key(&p.name, &field));
                lambda_env.remove(&format!("{}_{}", p.name, field));
            }
            super::struct_invariants::seed_struct_fields(
                vc,
                &mut lambda_env,
                &lambda_param_name,
                struct_def,
            );
            let fields = super::struct_invariants::nested_field_bindings(&lambda_env, &p_sym);
            super::struct_invariants::bind_struct_fields(&mut lambda_env, &p.name, &fields);
            vc.local_struct_types
                .borrow_mut()
                .insert(p.name.clone(), resolved_type.clone());
        }
        if vc.module_env.get_enum(&resolved_type).is_some() {
            vc.local_enum_types
                .borrow_mut()
                .insert(p.name.clone(), resolved_type);
        }
    }

    let saved_lambdas = vc.local_lambdas.borrow().clone();
    {
        let mut map = vc.local_lambdas.borrow_mut();
        for p in params {
            map.insert(p.name.clone(), std::rc::Rc::new(LocalLambda::Opaque));
        }
    }
    vc.recursion
        .lambda_bind_depth
        .set(vc.recursion.lambda_bind_depth.get() + 1);
    let body_result = if let Some(solver) = solver_opt {
        solver.push();
        solver.assert(&vc.path_cond_conj());
        for p in params {
            let (Some(type_ref), Some(value)) =
                (p.type_ref.as_ref(), lambda_env.get(&p.name).cloned())
            else {
                continue;
            };
            let type_name = type_ref.display_name();
            if type_ref.array_element_type().is_some() {
                if let Some(length) = lambda_env.get(&super::z3_types::array_len_key(&p.name)) {
                    if let Some(nonneg) = super::z3_types::nonneg_constraint(ctx, length) {
                        solver.assert(&nonneg);
                    }
                }
            }
            for (_, domain) in fixed_width_param_domains(
                vc,
                &p.name,
                &type_name,
                &value,
                &mut lambda_env,
                solver_opt,
            ) {
                solver.assert(&domain);
            }
        }
        let result = stmt_to_z3(vc, body, &mut lambda_env, solver_opt);
        solver.pop(1);
        result
    } else {
        vc.begin_spec_domain_clause();
        let result = stmt_to_z3(vc, body, &mut lambda_env, solver_opt);
        let obligations = vc.finish_spec_domain_clause();
        if obligations.is_empty() {
            result
        } else {
            Err(MumeiError::verification(
                "lambda body has argument-domain obligations that require a solver",
            )
            .with_verdict(crate::verification::VerdictKind::Rejected))
        }
    };
    vc.recursion
        .lambda_bind_depth
        .set(vc.recursion.lambda_bind_depth.get() - 1);
    *vc.local_lambdas.borrow_mut() = saved_lambdas;
    *vc.local_int_types.borrow_mut() = int_types_before;
    *vc.local_array_elem_types.borrow_mut() = array_elem_types_before;
    *vc.local_struct_types.borrow_mut() = struct_types_before;
    *vc.local_enum_types.borrow_mut() = enum_types_before;
    body_result?;

    Ok(lambda_sym.into())
}

/// Key a call result's fields — and each struct-typed field's own
/// fields, recursively — by their Z3 asts: a nested read
/// `f(1).inner.a` evaluates `f(1).inner` to the inner value and looks
/// it up by that value's ast, so registering only the outer result
/// leaves deep reads on the `field_*` fallback.
fn register_call_result_fields<'a>(
    vc: &VCtx<'a>,
    env: &Env<'a>,
    value: &Dynamic<'a>,
    fields: &[(String, Dynamic<'a>)],
) {
    vc.call_result_fields.borrow_mut().insert(
        value.get_z3_ast() as usize,
        (fields.to_vec(), value.clone()),
    );
    for (_, inner) in fields {
        if let Some(inner_fields) = project_struct_fields(env, inner) {
            register_call_result_fields(vc, env, inner, &inner_fields);
        }
    }
}

/// Binding identity for two `LocalLambda` entries — the `if`/`else`
/// merge keeps a name bound on both sides only when both left the *same*
/// binding in place (untouched, or re-aliased to the same `Rc` — e.g.
/// `let g = f` shares `f`'s allocation); an `ite` over two different
/// closures has no Z3 encoding, and textually identical lambdas written
/// in both branches are still distinct bindings, so they drop.
fn same_local_lambda<'a>(
    a: &std::rc::Rc<LocalLambda<'a>>,
    b: &std::rc::Rc<LocalLambda<'a>>,
) -> bool {
    std::rc::Rc::ptr_eq(a, b)
}

#[inline(never)]
fn fixed_width_param_domains<'a>(
    vc: &VCtx<'a>,
    param_name: &str,
    type_name: &str,
    value: &Dynamic<'a>,
    env: &mut Env<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> Vec<(String, Bool<'a>)> {
    let resolved_type = vc.module_env.resolve_base_type(type_name);
    let mut domains = Vec::new();

    if let Some(domain) = datatype::scalar_int_leaf_domain(vc, &resolved_type, value) {
        domains.push((resolved_type.clone(), domain));
    } else if let Some(element_type) = array_element_type_name(&resolved_type) {
        if value.as_array().is_some() {
            let length = super::z3_types::array_len_value(
                vc.ctx,
                env,
                param_name,
                vc.bitvec_i64,
                solver_opt,
            );
            if let Some(leaves) =
                super::z3_types::struct_array_leaf_types(element_type, vc.module_env)
            {
                for (path, leaf_type) in leaves {
                    let key = super::z3_types::struct_array_leaf_key(param_name, &path);
                    let Some(array) = env.get(&key).and_then(|value| value.as_array()) else {
                        continue;
                    };
                    let array: Dynamic = array.into();
                    let domain = if leaf_type.starts_with("enum:") {
                        datatype::enum_array_leaf_domain(vc, &array, &length)
                    } else {
                        datatype::array_int_leaf_domain(vc, &array, &length, &leaf_type)
                    };
                    if let Some(domain) = domain {
                        domains.push((
                            leaf_type
                                .strip_prefix("enum:")
                                .unwrap_or(&leaf_type)
                                .to_string(),
                            domain,
                        ));
                    }
                }
            } else if let Some(domain) =
                datatype::array_int_leaf_domain(vc, value, &length, element_type)
            {
                domains.push((element_type.to_string(), domain));
            }
        }
    } else if vc.module_env.get_enum(&resolved_type).is_some() {
        if let Some(domain) = datatype::enum_value_leaf_domain(vc, value) {
            domains.push((resolved_type.clone(), domain));
        }
    } else if vc.module_env.get_struct(&resolved_type).is_some() {
        let values = super::struct_invariants::nested_field_bindings(env, value);
        for (_, leaf_value) in &values {
            if let Some(domain) = datatype::enum_value_leaf_domain(vc, leaf_value) {
                let enum_name = vc
                    .module_env
                    .enums
                    .values()
                    .find(|enum_def| {
                        datatype::enum_datatype_sort(vc, enum_def)
                            .is_some_and(|sort| sort.sort == leaf_value.get_sort())
                    })
                    .map(|enum_def| enum_def.name.clone())
                    .unwrap_or_else(|| resolved_type.clone());
                domains.push((enum_name, domain));
            }
        }
        if let Some(leaves) = datatype::field_leaf_types(&resolved_type, vc.module_env) {
            for (leaf_name, leaf_type) in leaves {
                if leaf_type == "array_len" {
                    continue;
                }
                let Some(leaf_value) = values
                    .iter()
                    .find(|(name, _)| name == &leaf_name)
                    .map(|(_, value)| value)
                else {
                    continue;
                };
                if let Some(element_type) = leaf_type.strip_prefix("array:") {
                    let Some(array) = leaf_value.as_array() else {
                        continue;
                    };
                    let length = super::z3_types::array_len_for_value(vc, env, &array, solver_opt);
                    if let Some(domain) =
                        datatype::array_int_leaf_domain(vc, leaf_value, &length, element_type)
                    {
                        domains.push((element_type.to_string(), domain));
                    }
                } else if let Some(domain) =
                    datatype::scalar_int_leaf_domain(vc, &leaf_type, leaf_value)
                {
                    domains.push((vc.module_env.resolve_base_type(&leaf_type), domain));
                }
            }
        }
    }

    domains
}

#[inline(never)]
fn check_lambda_call_argument_domain<'a>(
    vc: &VCtx<'a>,
    solver_opt: Option<&Solver<'a>>,
    callee_name: &str,
    param_name: &str,
    type_name: &str,
    value: &Dynamic<'a>,
    call_env: &mut Env<'a>,
) -> MumeiResult<()> {
    check_call_argument_domain(
        vc,
        solver_opt,
        callee_name,
        param_name,
        type_name,
        value,
        call_env,
    )
}

/// Apply a `let`/`assign`-bound lambda to call-site arguments:
/// `f(args)` and `call(f, args)` evaluate the lambda's `body` with each
/// formal parameter bound to the evaluated actual — the call is the
/// let-in expression `let p_i = arg_i in body`.
///
/// Free lambda-variable names inside the body resolve against
/// `lambda.captured` — the scope live where the lambda was bound — not
/// the call-site `local_lambdas`, so a same-named later rebind cannot
/// redirect calls the body makes. Parameter names are removed from that
/// scope so a param like `|g| g(1)` shadows an outer lambda `g`.
/// `local_lambdas`, `local_enum_types`, and `local_array_elem_types`
/// are all restored afterwards: `let`s inside the body stay call-local
/// (they land in the cloned `call_env`, but the type/lambda side tables
/// are shared and would otherwise leak).
#[inline(never)]
fn apply_local_lambda<'a>(
    vc: &VCtx<'a>,
    call_name: &str,
    lambda: &LocalLambda<'a>,
    args: &[Expr],
    env: &mut Env<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> DynResult<'a> {
    let LocalLambda::Closure { expr, captured } = lambda else {
        match lambda {
            // `let h = if c { f } else { g }`: applying `h` inlines both
            // bodies behind the let-site condition.
            LocalLambda::Ite {
                cond,
                then_lam,
                else_lam,
            } => {
                let t = apply_local_lambda(vc, call_name, then_lam, args, env, solver_opt)?;
                let e = apply_local_lambda(vc, call_name, else_lam, args, env, solver_opt)?;
                return Ok(cond.ite(&t, &e));
            }
            // `Opaque` — a lambda param called inside its own body while the
            // body is being translated at binding time. The call's arity and
            // return type are unknowable until a real closure reaches the
            // param at a call site, so yield a fresh symbolic int and let the
            // check continue (its value is discarded anyway).
            LocalLambda::Opaque => {
                static OPAQUE_CALL_COUNTER: std::sync::atomic::AtomicUsize =
                    std::sync::atomic::AtomicUsize::new(0);
                let call_id = OPAQUE_CALL_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                return Ok(Int::new_const(
                    vc.ctx,
                    format!("__lam_param_call_{}_{}", call_name, call_id).as_str(),
                )
                .into());
            }
            LocalLambda::Closure { .. } => unreachable!(),
        }
    };
    let Expr::Lambda { params, body, .. } = expr else {
        return Err(MumeiError::verification(format!(
            "Call to '{call_name}': binding is not a lambda"
        )));
    };
    if params.len() != args.len() {
        return Err(MumeiError::verification(format!(
            "Call to lambda '{call_name}' takes {} argument(s), got {}",
            params.len(),
            args.len()
        )));
    }

    // Arguments evaluate in the caller's env (call-by-value) before the
    // param bindings overwrite anything.
    let mut arg_vals = Vec::with_capacity(args.len());
    let mut arg_leaf_snapshots = Vec::with_capacity(args.len());
    let mut earlier_array_aliases = Vec::new();
    for (index, arg) in args.iter().enumerate() {
        reject_later_array_argument_mutation(vc, arg, &earlier_array_aliases)?;
        let value = expr_to_z3(vc, arg, env, solver_opt)?;
        if value.as_array().is_some() {
            let aliases = array_argument_aliases(arg, &value, env);
            if !aliases.is_empty() {
                earlier_array_aliases.push(aliases);
            }
        }
        let param_type = params
            .get(index)
            .and_then(|param| param.type_ref.as_ref())
            .map(|type_ref| type_ref.display_name());
        arg_leaf_snapshots.push(super::z3_types::snapshot_struct_array_leaves(
            vc,
            env,
            arg,
            &value,
            param_type.as_deref(),
        )?);
        arg_vals.push(value);
    }
    reject_shared_array_mutating_call_args(vc, CalleeRef::Expr(expr), args, &arg_vals, env)?;

    let mut call_env = env.clone();
    for (i, param) in params.iter().enumerate() {
        if let Some(type_name) = param.type_ref.as_ref().map(|ty| ty.display_name()) {
            super::z3_types::validate_enum_array_type(vc, &type_name)?;
        }
        super::z3_types::clear_array_shared(vc.ctx, &mut call_env, &param.name);
        call_env.insert(param.name.clone(), arg_vals[i].clone());
        alias_struct_fields(&mut call_env, &param.name, &arg_vals[i]);
        super::z3_types::wire_array_slots_with_struct_leaf_snapshot(
            vc,
            &param.name,
            args.get(i),
            &arg_vals[i],
            &mut call_env,
            solver_opt,
            arg_leaf_snapshots.get(i).and_then(Option::as_ref),
        )?;
    }
    for (i, param) in params.iter().enumerate() {
        if let Some(type_name) = param.type_ref.as_ref().map(|ty| ty.display_name()) {
            check_lambda_call_argument_domain(
                vc,
                solver_opt,
                call_name,
                &param.name,
                &type_name,
                &arg_vals[i],
                &mut call_env,
            )?;
        }
    }

    let mut call_lambdas = captured.clone();
    {
        let caller_lambdas = vc.local_lambdas.borrow();
        for (i, param) in params.iter().enumerate() {
            // A param shadows any same-named outer lambda; when the actual
            // is itself a lambda (a bound name or a literal), the param
            // re-binds to it so `apply(f, x)`-style higher-order calls keep
            // a real body behind `f(…)` inside the callee.
            call_lambdas.remove(&param.name);
            let passed = match args.get(i) {
                Some(Expr::Variable(src)) => caller_lambdas.get(src).cloned(),
                Some(Expr::Lambda { .. }) => Some(std::rc::Rc::new(LocalLambda::Closure {
                    expr: args[i].clone(),
                    captured: caller_lambdas.clone(),
                })),
                _ => None,
            };
            if let Some(passed) = passed {
                call_lambdas.insert(param.name.clone(), passed);
            }
        }
    }
    let int_types_before = vc.local_int_types.borrow().clone();
    let param_int_types: Vec<&'static str> = params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            p.type_ref
                .as_ref()
                .and_then(|t| fixed_width_int(&vc.module_env.resolve_base_type(&t.name)))
                .map(|(ty, _, _)| ty)
                .or_else(|| args.get(i).and_then(|a| fixed_width_int_type(vc, a)))
                .unwrap_or("i64")
        })
        .collect();
    for (p, ty) in params.iter().zip(param_int_types) {
        vc.local_int_types.borrow_mut().insert(p.name.clone(), ty);
    }
    let saved_lambdas = std::mem::replace(&mut *vc.local_lambdas.borrow_mut(), call_lambdas);
    let enum_types_before = vc.local_enum_types.borrow().clone();
    let elem_types_before = vc.local_array_elem_types.borrow().clone();
    let result = stmt_to_z3(vc, body, &mut call_env, solver_opt);
    *vc.local_int_types.borrow_mut() = int_types_before;
    *vc.local_lambdas.borrow_mut() = saved_lambdas;
    *vc.local_enum_types.borrow_mut() = enum_types_before;
    *vc.local_array_elem_types.borrow_mut() = elem_types_before;
    // The body ran on a cloned `call_env`, so stores through a lambda
    // param (`x[i] = v`) never reached the caller's tracked slot — but at
    // real execution the `[T]` arg shares the caller's backing. Havoc the
    // caller's slot for each storing param so post-call reads can't claim
    // pre-call elements.
    // Captured variables likewise share the caller's bindings, so a store
    // to an outer name mutates caller state too (`havoc_array_name` no-ops
    // on non-array bindings).
    if result.is_ok() {
        for name in super::may_write::lambda_may_write_args(vc.module_env, lambda, args, env) {
            havoc_array_name(vc, &name, env);
        }
    }
    result
}

#[cfg(test)]
mod termination_measure_tests {
    use super::termination_measure_obligation;
    use z3::ast::{Bool, Dynamic, Int, BV};
    use z3::{Config, Context, SatResult, Solver};

    fn int_pair<'ctx>(ctx: &'ctx Context, first: i64, second: i64) -> [Dynamic<'ctx>; 2] {
        [
            Int::from_i64(ctx, first).into(),
            Int::from_i64(ctx, second).into(),
        ]
    }

    fn bitvector_pair<'ctx>(ctx: &'ctx Context, first: i64, second: i64) -> [Dynamic<'ctx>; 2] {
        [
            BV::from_i64(ctx, first, 64).into(),
            BV::from_i64(ctx, second, 64).into(),
        ]
    }

    fn assert_holds(ctx: &Context, obligation: &Bool) {
        let solver = Solver::new(ctx);
        solver.assert(&obligation.not());
        assert_eq!(solver.check(), SatResult::Unsat);
    }

    fn assert_never_holds(ctx: &Context, obligation: &Bool) {
        let solver = Solver::new(ctx);
        solver.assert(obligation);
        assert_eq!(solver.check(), SatResult::Unsat);
    }

    #[test]
    fn single_int_measure_matches_the_previous_obligation() {
        let ctx = Context::new(&Config::new());
        let a = Int::new_const(&ctx, "measure_a");
        let b = Int::new_const(&ctx, "measure_b");
        let expected = Bool::and(&ctx, &[&Int::from_i64(&ctx, 0).le(&a), &b.lt(&a)]);
        let actual = termination_measure_obligation(&ctx, &[a.into()], &[b.into()])
            .expect("compatible integer measures");

        assert_eq!(actual.to_string(), expected.to_string());
    }

    #[test]
    fn single_bitvector_measure_matches_the_previous_obligation() {
        let ctx = Context::new(&Config::new());
        let a = BV::new_const(&ctx, "measure_a", 64);
        let b = BV::new_const(&ctx, "measure_b", 64);
        let expected = Bool::and(&ctx, &[&BV::from_i64(&ctx, 0, 64).bvsle(&a), &b.bvslt(&a)]);
        let actual = termination_measure_obligation(&ctx, &[a.into()], &[b.into()])
            .expect("compatible bitvector measures");

        assert_eq!(actual.to_string(), expected.to_string());
    }

    #[test]
    fn lexicographic_int_measure_obligations_match_the_order() {
        let ctx = Context::new(&Config::new());
        let cases = [
            ((2, 0), (1, 100), true),
            ((1, 5), (1, 4), true),
            ((1, 4), (1, 4), false),
            ((1, 4), (2, 0), false),
            ((1, -1), (1, -2), false),
        ];

        for ((a1, a2), (b1, b2), should_hold) in cases {
            let measure_a = int_pair(&ctx, a1, a2);
            let measure_b = int_pair(&ctx, b1, b2);
            let obligation = termination_measure_obligation(&ctx, &measure_a, &measure_b)
                .expect("compatible integer measures");

            if should_hold {
                assert_holds(&ctx, &obligation);
            } else {
                assert_never_holds(&ctx, &obligation);
            }
        }
    }

    #[test]
    fn lexicographic_bitvector_measure_obligations_use_signed_order() {
        let ctx = Context::new(&Config::new());
        let measure_a = bitvector_pair(&ctx, 1, 5);
        let measure_b = bitvector_pair(&ctx, 1, 4);
        let obligation = termination_measure_obligation(&ctx, &measure_a, &measure_b)
            .expect("compatible bitvector measures");
        assert_holds(&ctx, &obligation);

        let measure_a = bitvector_pair(&ctx, 0, -1);
        let measure_b = bitvector_pair(&ctx, 0, -2);
        let obligation = termination_measure_obligation(&ctx, &measure_a, &measure_b)
            .expect("compatible bitvector measures");
        assert_never_holds(&ctx, &obligation);
    }

    #[test]
    fn incompatible_termination_measure_shapes_return_none() {
        let ctx = Context::new(&Config::new());
        let a1 = Int::new_const(&ctx, "a1");
        let a2 = Int::new_const(&ctx, "a2");
        let b1 = Int::new_const(&ctx, "b1");
        assert!(
            termination_measure_obligation(&ctx, &[a1.into(), a2.into()], &[b1.into()]).is_none()
        );
        assert!(termination_measure_obligation(&ctx, &[], &[]).is_none());

        let int_measure = Int::new_const(&ctx, "int_measure");
        let bitvector_measure = BV::new_const(&ctx, "bitvector_measure", 64);
        assert!(termination_measure_obligation(
            &ctx,
            &[int_measure.into()],
            &[bitvector_measure.into()]
        )
        .is_none());
    }
}

#[cfg(test)]
mod constant_args_tests {
    use super::constant_args;
    use z3::ast::{Array, Bool, Int};
    use z3::{Config, Context};

    #[test]
    fn accepts_int_numeral_beyond_i64_range() {
        let ctx = Context::new(&Config::new());
        let max = Int::from_i64(&ctx, i64::MAX);
        let three = Int::from_i64(&ctx, 3);
        let large = &max + &three;
        let result = constant_args(&[large.into()]);

        assert!(result.is_some(), "large Int numeral should be constant");
        assert_eq!(result.unwrap().1, "int:9223372036854775810");
    }

    #[test]
    fn formats_regular_int_numeral() {
        let ctx = Context::new(&Config::new());
        let args = [Int::from_i64(&ctx, 3).into()];

        assert_eq!(constant_args(&args).unwrap().1, "int:3");
    }

    #[test]
    fn rejects_non_numeral_int() {
        let ctx = Context::new(&Config::new());
        let args = [Int::new_const(&ctx, "x").into()];

        assert!(constant_args(&args).is_none());
    }

    #[test]
    fn formats_mixed_int_and_bool_args() {
        let ctx = Context::new(&Config::new());
        let args = [
            Int::from_i64(&ctx, 2).into(),
            Bool::from_bool(&ctx, true).into(),
        ];

        assert_eq!(constant_args(&args).unwrap().1, "int:2,bool:true");
    }

    #[test]
    fn rejects_array_arguments() {
        let ctx = Context::new(&Config::new());
        let int_sort = z3::Sort::int(&ctx);
        let array = Array::new_const(&ctx, "a", &int_sort, &int_sort);

        assert!(constant_args(&[array.into()]).is_none());
    }
}

#[cfg(test)]
mod rec_fn_args_tests {
    use super::{expand_rec_fn_args, RecFnParamKind};
    use z3::ast::{Array, Ast, Dynamic, Int, BV};
    use z3::{Config, Context, FuncDecl, SatResult, Sort};

    fn assert_length_is_part_of_the_domain(bitvec_i64: bool) {
        let ctx = Context::new(&Config::new());
        let int_sort = Sort::int(&ctx);
        let array = Array::new_const(&ctx, "a", &int_sort, &int_sort);
        let (length_sort, length_one, length_two): (Sort, Dynamic, Dynamic) = if bitvec_i64 {
            (
                Sort::bitvector(&ctx, 64),
                BV::new_const(&ctx, "l1", 64).into(),
                BV::new_const(&ctx, "l2", 64).into(),
            )
        } else {
            (
                int_sort.clone(),
                Int::new_const(&ctx, "l1").into(),
                Int::new_const(&ctx, "l2").into(),
            )
        };
        let kinds = [RecFnParamKind::Array];
        let expanded_one = expand_rec_fn_args(
            &kinds,
            &[array.clone().into()],
            &length_sort,
            |_| Some(array.clone().into()),
            |_| Some(length_one.clone()),
            |_, _| None,
            |_, _| None,
        )
        .expect("array length should expand");
        let expanded_two = expand_rec_fn_args(
            &kinds,
            &[array.clone().into()],
            &length_sort,
            |_| Some(array.clone().into()),
            |_| Some(length_two.clone()),
            |_, _| None,
            |_, _| None,
        )
        .expect("array length should expand");
        let domain: Vec<Sort> = expanded_one.iter().map(|value| value.get_sort()).collect();
        let domain_refs: Vec<&Sort> = domain.iter().collect();
        let decl = FuncDecl::new(&ctx, "rec_test", &domain_refs, &int_sort);
        let first_args: Vec<&dyn Ast> =
            expanded_one.iter().map(|value| value as &dyn Ast).collect();
        let second_args: Vec<&dyn Ast> =
            expanded_two.iter().map(|value| value as &dyn Ast).collect();
        let first = decl.apply(&first_args);
        let second = decl.apply(&second_args);

        let solver = z3::Solver::new(&ctx);
        solver.assert(&length_one._eq(&length_two).not());
        solver.assert(&first._eq(&second).not());
        assert_eq!(
            solver.check(),
            SatResult::Sat,
            "different lengths must remain distinguishable UF arguments"
        );

        let same = decl.apply(&first_args);
        let solver = z3::Solver::new(&ctx);
        solver.assert(&same._eq(&same).not());
        assert_eq!(
            solver.check(),
            SatResult::Unsat,
            "identical expanded arguments must produce identical UF applications"
        );
    }

    #[test]
    fn array_length_is_a_recursive_uf_argument_in_int_mode() {
        assert_length_is_part_of_the_domain(false);
    }

    #[test]
    fn array_length_is_a_recursive_uf_argument_in_bitvec_mode() {
        assert_length_is_part_of_the_domain(true);
    }
}

/// `%` with the truncating semantics of Rust/Go/C/TypeScript/Solidity and
/// LLVM `srem`: `l - r * trunc(l / r)`, so the result takes the dividend's
/// sign. Z3 `Int` division is Euclidean, which already truncates for a
/// non-negative dividend; a negative one is divided as `-(-l / r)`.
fn truncated_rem<'a>(ctx: &'a Context, l: &Int<'a>, r: &Int<'a>) -> Int<'a> {
    l - &(r * &truncated_div(ctx, l, r))
}

/// Integer `/` truncates toward zero — the same quotient
/// `truncated_rem` pairs with, matching `%`, `sdiv`/`srem` in the
/// emitted IR and every ported source language. Z3's `div` is
/// Euclidean (remainder always non-negative), so on a negative
/// dividend `a / b` must round the other way: `-((-a) div b)`.
fn truncated_div<'a>(ctx: &'a Context, l: &Int<'a>, r: &Int<'a>) -> Int<'a> {
    let zero = Int::from_i64(ctx, 0);
    l.ge(&zero).ite(&(l / r), &(-(&(-l) / r)))
}
