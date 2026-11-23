//! Plan 10 / P10-C: native Z3 `Datatype` sorts for finite enums and bounded
//! recursive enum groups (`Z3_DATATYPE_SORT`).
//!
//! Finite non-generic enums with supported payload leaves and bounded,
//! inhabited non-generic recursive enum groups are lowered to native Z3
//! datatypes. Other enums and recursion through structs or arrays keep the
//! Int-tag path. Native variants become constructors with `is-<Variant>`
//! testers and selectors of
//! the *correct* sort (a `Str` payload projects as a Z3 `String`, not an Int
//! tag). In default Int mode, width-narrowed integer leaves
//! (`u64`/`u32`/`i32`) land on `Int` — [`enum_value_leaf_domain`] bounds them
//! to their declared width at every introduction point.
//!
//! Unsupported recursive enums keep the Int-tag encoding and the
//! `inductive_data_type` fragment tag (Lean 4 delegation boundary).

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use z3::ast::{Ast, Bool, Datatype, Dynamic, Int, BV};
use z3::{Context, DatatypeAccessor, DatatypeSort, SatResult, Solver, Sort};

use crate::ast::TypeRef;
use crate::lowering::{lower, LoweredType};
use crate::parser::ast::{EnumDef, Expr, Stmt};
use crate::verification::module_env::ModuleEnv;
use crate::verification::translator::{VCtx, F64_EBITS, F64_SBITS};
use crate::verification::types::{Env, MumeiError, MumeiResult};

/// Deepest enum-in-payload nesting a finite ADT accepts, bounding flattening
/// of unsupported declaration cycles.
const MAX_ENUM_PAYLOAD_NEST: u8 = 8;

/// `true` when `enum_def` is finite + non-recursive and every payload
/// flattens to supported leaves
/// (a declared *struct* payload whose direct fields are all scalar also
/// counts: its leaves flatten into the constructor, mirroring the trusted
/// verifier's `pure-i64-composite/v18` layout; a payload field typed as
/// another finite enum counts too, carrying one accessor of the inner
/// enum's datatype sort — `pure-i64-composite/v23`). Direct arrays and
/// scalar-element arrays nested in structs contribute array and length
/// accessors; nested arrays and arrays containing non-scalar elements keep
/// the Int-tag path.
pub(crate) fn is_finite_adt(enum_def: &EnumDef, module_env: &ModuleEnv) -> bool {
    is_finite_adt_fueled(
        enum_def,
        module_env,
        MAX_ENUM_PAYLOAD_NEST,
        &mut FiniteFuelMemo::default(),
    )
}

const MAX_RECURSIVE_ENUM_GROUP_SIZE: usize = 4;

fn direct_enum_field_target<'a>(
    enum_def: &EnumDef,
    field_type: &TypeRef,
    module_env: &'a ModuleEnv,
) -> Option<&'a EnumDef> {
    if !field_type.type_args.is_empty() || field_type.name.starts_with('[') {
        return None;
    }
    let field_name = if field_type.name == "Self" {
        enum_def.name.as_str()
    } else {
        field_type.name.as_str()
    };
    if field_name.contains('<') {
        return None;
    }
    let resolved = module_env.resolve_base_type(field_name);
    let target = module_env.get_enum(&resolved)?;
    if !target.type_params.is_empty() || target.name.contains('<') {
        return None;
    }
    Some(target)
}

fn enum_graph_reaches(start: &str, target: &str, edges: &HashMap<String, Vec<String>>) -> bool {
    let mut pending = vec![start.to_string()];
    let mut visited = HashSet::new();
    while let Some(current) = pending.pop() {
        if current == target {
            return true;
        }
        if !visited.insert(current.clone()) {
            continue;
        }
        pending.extend(edges.get(&current).into_iter().flatten().cloned());
    }
    false
}

/// The strongly connected component containing `enum_def`, ordered by name.
/// Only bare, non-generic enum-valued fields participate in the graph.
pub(crate) fn recursive_enum_group<'a>(
    enum_def: &EnumDef,
    module_env: &'a ModuleEnv,
) -> Option<Vec<&'a EnumDef>> {
    if !enum_def
        .variants
        .iter()
        .flat_map(|variant| &variant.field_types)
        .any(|field| direct_enum_field_target(enum_def, field, module_env).is_some())
    {
        return None;
    }
    let mut enums: Vec<&EnumDef> = module_env
        .enums
        .values()
        .filter(|definition| definition.type_params.is_empty() && !definition.name.contains('<'))
        .collect();
    enums.sort_by(|left, right| left.name.cmp(&right.name));
    if !enums
        .iter()
        .any(|candidate| candidate.name == enum_def.name)
    {
        return None;
    }

    let mut edges = HashMap::new();
    for definition in &enums {
        let mut targets = Vec::new();
        for variant in &definition.variants {
            for field in &variant.field_types {
                if let Some(target) = direct_enum_field_target(definition, field, module_env) {
                    targets.push(target.name.clone());
                }
            }
        }
        targets.sort();
        targets.dedup();
        edges.insert(definition.name.clone(), targets);
    }

    let mut group: Vec<&EnumDef> = enums
        .into_iter()
        .filter(|candidate| {
            enum_graph_reaches(&enum_def.name, &candidate.name, &edges)
                && enum_graph_reaches(&candidate.name, &enum_def.name, &edges)
        })
        .collect();
    let is_self_recursive = group.len() == 1
        && edges
            .get(&enum_def.name)
            .is_some_and(|targets| targets.iter().any(|target| target == &enum_def.name));
    if group.len() == 1 && !is_self_recursive {
        return None;
    }
    group.sort_by(|left, right| left.name.cmp(&right.name));
    Some(group)
}

pub(crate) fn recursive_field_target<'a>(
    enum_def: &EnumDef,
    field_type: &TypeRef,
    module_env: &'a ModuleEnv,
) -> Option<&'a EnumDef> {
    let target = direct_enum_field_target(enum_def, field_type, module_env)?;
    let group = recursive_enum_group(enum_def, module_env)?;
    group
        .iter()
        .any(|member| member.name == target.name)
        .then_some(target)
}

pub(crate) fn is_self_recursive_adt(enum_def: &EnumDef, module_env: &ModuleEnv) -> bool {
    let Some(group) = recursive_enum_group(enum_def, module_env) else {
        return false;
    };
    if group.len() > MAX_RECURSIVE_ENUM_GROUP_SIZE
        || group.iter().any(|member| {
            !member.type_params.is_empty()
                || member.name.contains('<')
                || member.variants.is_empty()
        })
    {
        return false;
    }

    let group_names: HashSet<&str> = group.iter().map(|member| member.name.as_str()).collect();
    let mut memo = FiniteFuelMemo::default();
    for member in &group {
        for variant in &member.variants {
            for field in &variant.field_types {
                if direct_enum_field_target(member, field, module_env)
                    .is_some_and(|target| group_names.contains(target.name.as_str()))
                {
                    continue;
                }
                let field_type = payload_field_type_name(field);
                if struct_array_payload_element_type(&field_type, module_env).is_some()
                    || !field_is_finite_payload(
                        &field_type,
                        module_env,
                        MAX_ENUM_PAYLOAD_NEST,
                        &mut memo,
                    )
                {
                    return false;
                }
            }
        }
    }

    let mut inhabited = HashSet::new();
    loop {
        let mut changed = false;
        for member in &group {
            if inhabited.contains(member.name.as_str()) {
                continue;
            }
            let has_inhabited_variant = member.variants.iter().any(|variant| {
                variant.field_types.iter().all(|field| {
                    direct_enum_field_target(member, field, module_env)
                        .filter(|target| group_names.contains(target.name.as_str()))
                        .is_none_or(|target| inhabited.contains(target.name.as_str()))
                })
            });
            if has_inhabited_variant {
                inhabited.insert(member.name.as_str());
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    inhabited.len() == group.len()
}

pub(crate) fn uses_datatype_sort(enum_def: &EnumDef, module_env: &ModuleEnv) -> bool {
    is_finite_adt(enum_def, module_env) || is_self_recursive_adt(enum_def, module_env)
}

/// Memoized `is_finite_adt_fueled` answers keyed by `(enum_name, fuel)`.
/// A shared inner enum on every field of every level used to re-walk its
/// subtree per field — `16 fields × 8 levels` was ~4 billion leaf walks.
/// `fuel` strictly decreases on each enum edge, so a `(name, fuel)` key
/// cannot re-enter while in progress and no cycle marker is needed.
type FiniteFuelMemo = HashMap<(String, u8), bool>;

fn is_finite_adt_fueled(
    enum_def: &EnumDef,
    module_env: &ModuleEnv,
    fuel: u8,
    memo: &mut FiniteFuelMemo,
) -> bool {
    let key = (enum_def.name.clone(), fuel);
    if let Some(&finite) = memo.get(&key) {
        return finite;
    }
    // Empty enums cannot form a Z3 datatype (`DatatypeBuilder::finish`
    // asserts at least one variant) and never have a constructible value, so
    // they keep the Int-tag encoding — the vacuous `all()` below would
    // otherwise claim them.
    let finite = recursive_enum_group(enum_def, module_env).is_none()
        && !enum_def.is_recursive
        && enum_def.type_params.is_empty()
        && !enum_def.variants.is_empty()
        && enum_def.variants.iter().all(|variant| {
            variant.field_types.iter().all(|field| {
                field_is_finite_payload(&payload_field_type_name(field), module_env, fuel, memo)
            })
        });
    memo.insert(key, finite);
    finite
}

fn field_is_finite_payload(
    field_type: &str,
    module_env: &ModuleEnv,
    fuel: u8,
    memo: &mut FiniteFuelMemo,
) -> bool {
    field_leaf_types_fueled(field_type, module_env, fuel, memo).is_some()
}

/// The `(leaf_name, leaf_base_type)` pairs a payload field contributes to
/// the Z3 constructor: a scalar field is a single unnamed leaf; a declared
/// struct expands to its scalar leaves, with a struct-typed field recursing
/// under a `{field}_{leaf}` name (`b.inner.x` → leaf `inner_x`, matching the
/// `<binding>_<path>` env-key convention). A scalar-element `[T]` field at
/// any struct path contributes an array leaf and its sibling `#len` leaf.
/// Nested arrays and arrays of non-scalar elements, a recursive struct on the
/// flattening path, a leaf-name collision
/// (`inner_x` scalar beside `inner: {x}`), or more than
/// `MAX_PAYLOAD_LEAVES` leaves return `None` and keep the enum on the
/// Int-tag encoding.
pub(crate) fn field_leaf_types(
    field_type: &str,
    module_env: &ModuleEnv,
) -> Option<Vec<(String, String)>> {
    field_leaf_types_fueled(
        field_type,
        module_env,
        MAX_ENUM_PAYLOAD_NEST,
        &mut FiniteFuelMemo::default(),
    )
}

pub(crate) fn payload_field_type_name(field: &crate::ast::TypeRef) -> String {
    if field.name == "[]" {
        field.display_name()
    } else {
        field.name.clone()
    }
}

fn field_leaf_types_fueled(
    field_type: &str,
    module_env: &ModuleEnv,
    fuel: u8,
    memo: &mut FiniteFuelMemo,
) -> Option<Vec<(String, String)>> {
    let mut leaves = Vec::new();
    let mut visiting = std::collections::HashSet::new();
    let mut seen = std::collections::HashSet::new();
    let mut work = 0usize;
    if collect_field_leaves(
        field_type,
        module_env,
        String::new(),
        &mut leaves,
        &mut visiting,
        &mut seen,
        &mut work,
        fuel,
        memo,
    )
    .is_none()
    {
        // A bounded rejection is silent to callers (`None` falls back to
        // the scalar path), so surface *which* budget tripped — otherwise
        // an exponential shared-subtree walk is indistinguishable from a
        // structural reject (recursion, enum/array leaf, name collision).
        if work > MAX_LEAF_FIELD_VISITS {
            eprintln!(
                "warning: enum payload flattening exceeded the leaf-walk visit budget ({MAX_LEAF_FIELD_VISITS}); the payload binds as a scalar projection"
            );
        } else if leaves.len() >= MAX_PAYLOAD_LEAVES {
            eprintln!(
                "warning: enum payload exceeds {MAX_PAYLOAD_LEAVES} leaves; the payload binds as a scalar projection"
            );
        }
        return None;
    }
    Some(leaves)
}

pub(crate) fn array_payload_element_type(leaves: &[(String, String)]) -> Option<&str> {
    if leaves.len() == 2
        && leaves[0].0.is_empty()
        && leaves[1].0 == "#len"
        && leaves[1].1 == "array_len"
    {
        leaves[0].1.strip_prefix("array:")
    } else {
        None
    }
}

pub(crate) fn struct_array_payload_element_type(
    field_type: &str,
    module_env: &ModuleEnv,
) -> Option<String> {
    let resolved = module_env.resolve_base_type(field_type);
    let element_type = resolved.strip_prefix('[')?.strip_suffix(']')?.trim();
    let element_type = module_env.resolve_base_type(element_type);
    crate::verification::translator::z3_types::struct_array_leaf_types(&element_type, module_env)?;
    Some(element_type)
}

pub(crate) fn array_payload_length_index(
    leaves: &[(String, String)],
    array_index: usize,
) -> Option<usize> {
    let (name, base) = leaves.get(array_index)?;
    base.strip_prefix("array:")?;
    let array_name = name.split_once('#').map(|(root, _)| root).unwrap_or(name);
    let length_name = format!("{array_name}#len");
    leaves
        .iter()
        .position(|(candidate, base)| candidate == &length_name && base == "array_len")
}

/// Iterate the array/length sibling pairs in a flattened payload leaf list.
/// Each array leaf is paired with the shared length leaf for its root array;
/// direct `[T]` payloads use `""` / `"#len"`.
pub(crate) fn array_payload_leaf_pairs<'a>(
    leaves: &'a [(String, String)],
) -> impl Iterator<Item = (usize, &'a str)> + 'a {
    leaves.iter().enumerate().filter_map(|(index, (_, base))| {
        let element_type = base.strip_prefix("array:")?;
        array_payload_length_index(leaves, index).map(|_| (index, element_type))
    })
}

pub(crate) fn assert_payload_array_lengths<'a>(
    vc: &VCtx<'a>,
    solver: &Solver<'a>,
    gate: &Bool<'a>,
    leaf_types: &[(String, String)],
    leaf_values: &[Dynamic<'a>],
) {
    for (array_index, _) in array_payload_leaf_pairs(leaf_types) {
        let Some(length_index) = array_payload_length_index(leaf_types, array_index) else {
            continue;
        };
        let Some(length) = leaf_values.get(length_index) else {
            continue;
        };
        if let Some(nonnegative) =
            crate::verification::translator::z3_types::nonneg_constraint(vc.ctx, length)
        {
            solver.assert(&gate.implies(&nonnegative));
        }
    }
}

/// Widest leaf set one payload field may flatten to — the same budget the
/// composite fragment allows a bound struct. Exponential blowup from shared
/// nested declarations is rejected as a whole rather than truncated.
const MAX_PAYLOAD_LEAVES: usize = 16;

/// Total nodes `collect_field_leaves` may visit per call. The leaf cap
/// alone does not bound the walk: shared subtrees of empty structs produce
/// no leaves, so a deep DAG re-visits them exponentially. A legitimate
/// flattening visits one node per field on every path — 16 times the leaf
/// cap leaves headroom for long linear nestings while rejecting a 2^n
/// shared-subtree walk outright.
const MAX_LEAF_FIELD_VISITS: usize = 16 * MAX_PAYLOAD_LEAVES;

#[allow(clippy::too_many_arguments)]
fn collect_field_leaves(
    field_type: &str,
    module_env: &ModuleEnv,
    name: String,
    leaves: &mut Vec<(String, String)>,
    visiting: &mut std::collections::HashSet<String>,
    seen: &mut std::collections::HashSet<String>,
    work: &mut usize,
    fuel: u8,
    memo: &mut FiniteFuelMemo,
) -> Option<()> {
    *work += 1;
    if *work > MAX_LEAF_FIELD_VISITS {
        return None;
    }
    if field_is_scalar(field_type, module_env) {
        if !seen.insert(name.clone()) || leaves.len() >= MAX_PAYLOAD_LEAVES {
            return None;
        }
        leaves.push((name, module_env.resolve_base_type(field_type)));
        return Some(());
    }
    let base = module_env.resolve_base_type(field_type);
    if let Some(element_type) = base
        .strip_prefix('[')
        .and_then(|ty| ty.strip_suffix(']'))
        .map(str::trim)
    {
        if !field_is_scalar(element_type, module_env) {
            if name.is_empty() {
                let struct_leaves = struct_array_leaf_types_fueled(
                    element_type,
                    module_env,
                    visiting,
                    work,
                    fuel,
                    memo,
                )?;
                let len_name = format!("{name}#len");
                if leaves.len() + struct_leaves.len() + 1 > MAX_PAYLOAD_LEAVES
                    || !seen.insert(len_name.clone())
                {
                    return None;
                }
                for (path, leaf_type) in struct_leaves {
                    let leaf_name = format!("{name}#{path}");
                    if !seen.insert(leaf_name.clone()) {
                        return None;
                    }
                    leaves.push((leaf_name, format!("array:{leaf_type}")));
                }
                leaves.push((len_name, "array_len".to_string()));
                return Some(());
            }
            return None;
        }
        let len_name = format!("{name}#len");
        if leaves.len() + 2 > MAX_PAYLOAD_LEAVES
            || !seen.insert(name.clone())
            || !seen.insert(len_name.clone())
        {
            return None;
        }
        leaves.push((
            name,
            format!("array:{}", module_env.resolve_base_type(element_type)),
        ));
        leaves.push((len_name, "array_len".to_string()));
        return Some(());
    }
    // An enum-typed payload leaf contributes a single `enum:`-marked leaf —
    // it is ONE constructor accessor carrying the inner enum's own datatype
    // sort (`pure-i64-composite/v23` in the trusted fragment). The inner
    // enum must itself flatten finitely; `fuel` bounds the walk so a
    // `A(B)`/`B(A)` declaration cycle cannot diverge.
    if let Some(inner) = module_env.enums.get(&base) {
        let fuel = fuel.checked_sub(1)?;
        if !is_finite_adt_fueled(inner, module_env, fuel, memo) {
            return None;
        }
        if !seen.insert(name.clone()) || leaves.len() >= MAX_PAYLOAD_LEAVES {
            return None;
        }
        leaves.push((name, format!("enum:{base}")));
        return Some(());
    }
    let sdef = module_env.structs.get(&base)?;
    // A recursive struct declaration (`Node { n: Node }`) cannot flatten —
    // path-level guard, not a done-set: the same struct may still appear
    // under a sibling field.
    if !visiting.insert(base.clone()) {
        return None;
    }
    for leaf in &sdef.fields {
        let leaf_name = if name.is_empty() {
            leaf.name.clone()
        } else {
            format!("{name}_{}", leaf.name)
        };
        collect_field_leaves(
            &leaf.type_name,
            module_env,
            leaf_name,
            leaves,
            visiting,
            seen,
            work,
            fuel,
            memo,
        )?;
    }
    visiting.remove(&base);
    Some(())
}

fn struct_array_leaf_types_fueled(
    element_type: &str,
    module_env: &ModuleEnv,
    visiting: &mut std::collections::HashSet<String>,
    work: &mut usize,
    fuel: u8,
    memo: &mut FiniteFuelMemo,
) -> Option<Vec<(String, String)>> {
    let resolved = module_env.resolve_base_type(element_type);
    let base = type_name_base(&resolved);
    let sdef = module_env
        .get_struct(&resolved)
        .or_else(|| module_env.get_struct(base))?;
    if !visiting.insert(base.to_string()) {
        return None;
    }
    let mut leaves = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for field in &sdef.fields {
        collect_field_leaves(
            &field.type_name,
            module_env,
            field.name.clone(),
            &mut leaves,
            visiting,
            &mut seen,
            work,
            fuel,
            memo,
        )?;
    }
    visiting.remove(base);
    if leaves.is_empty()
        || leaves
            .iter()
            .any(|(_, leaf_type)| leaf_type.starts_with("array:") || leaf_type == "array_len")
    {
        return None;
    }
    Some(leaves)
}

/// The declared leaf values of a struct-typed payload value, in
/// `field_leaf_types` order. The value's flat env bindings
/// (`__struct_<handle>_<path>` entries, incl. deeper `<field>_<leaf>`
/// re-homes) are keyed exactly like the leaf names, so a match-bound
/// composite (`b` for `V(b: Box{inner: Point})` → `inner_x`) and a
/// `StructInit` literal answer identically.
pub(crate) fn flattened_value_leaves<'a>(
    vc: &VCtx<'a>,
    env: &mut crate::verification::types::Env<'a>,
    value: &Dynamic<'a>,
    leaf_types: &[(String, String)],
    solver_opt: Option<&Solver<'a>>,
) -> Option<Vec<Dynamic<'a>>> {
    if leaf_types.len() == 1 && leaf_types[0].0.is_empty() {
        return Some(vec![value.clone()]);
    }
    let flat: std::collections::HashMap<String, Dynamic> =
        crate::verification::translator::nested_field_bindings(env, value)
            .into_iter()
            .collect();
    let array_pairs: std::collections::HashMap<usize, &str> =
        array_payload_leaf_pairs(leaf_types).collect();
    let mut values = Vec::with_capacity(leaf_types.len());
    let mut index = 0;
    while index < leaf_types.len() {
        let (name, base) = &leaf_types[index];
        if array_pairs.contains_key(&index) {
            let array = flat.get(name)?.as_array()?;
            let len = crate::verification::translator::z3_types::array_len_for_value(
                vc, env, &array, solver_opt,
            );
            values.push(array.into());
            values.push(len);
            index += 2;
        } else if base == "array_len" {
            return None;
        } else {
            values.push(flat.get(name)?.clone());
            index += 1;
        }
    }
    Some(values)
}

pub(crate) fn struct_array_payload_leaves<'a>(
    vc: &VCtx<'a>,
    env: &mut crate::verification::types::Env<'a>,
    field_type: &str,
    expression: &Expr,
    value: &Dynamic<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> MumeiResult<Option<Vec<Dynamic<'a>>>> {
    let Some(element_type) = struct_array_payload_element_type(field_type, vc.module_env) else {
        return Ok(None);
    };
    let Some(snapshot) = crate::verification::translator::z3_types::snapshot_struct_array_leaves(
        vc, env, expression, value, None,
    )?
    else {
        return Ok(None);
    };
    if snapshot.element_type() != element_type {
        return Ok(None);
    }
    let Some(array) = value.as_array() else {
        return Ok(None);
    };
    let length = crate::verification::translator::z3_types::array_len_for_expr(
        vc,
        env,
        Some(expression),
        &value.clone(),
        solver_opt,
    );
    let Some(leaves) = field_leaf_types(field_type, vc.module_env) else {
        return Ok(None);
    };
    let mut values = Vec::with_capacity(leaves.len());
    for (name, base) in leaves {
        if base == "array_len" {
            values.push(length.clone());
        } else if base.starts_with("array:") {
            let Some(path) = name.strip_prefix('#') else {
                return Ok(None);
            };
            let Some(leaf) = snapshot.leaf_array(path) else {
                return Ok(None);
            };
            values.push(leaf.into());
        } else {
            return Ok(None);
        }
    }
    crate::verification::translator::z3_types::register_array_value_len(env, &array, length);
    Ok(Some(values))
}

/// `(accessor_name, sort)` pairs one declared payload field contributes to
/// the constructor — one entry per flattened leaf. A scalar field keeps the
/// historical `{variant}_{i}` accessor name; a struct payload names leaves
/// `{variant}_{i}_{leaf}`.
fn field_leaf_sorts<'a>(
    vc: &VCtx<'a>,
    variant_name: &str,
    field_idx: usize,
    field_type: &str,
) -> Option<Vec<(String, Sort<'a>)>> {
    let leaves = field_leaf_types(field_type, vc.module_env)?;
    let single = leaves.len() == 1 && leaves[0].0.is_empty();
    let mut out = Vec::with_capacity(leaves.len());
    for (leaf_name, base) in leaves {
        let name = if base.starts_with("array:") {
            if leaf_name.is_empty() {
                format!("{variant_name}_{field_idx}")
            } else {
                format!("{variant_name}_{field_idx}_{leaf_name}")
            }
        } else if base == "array_len" {
            if leaf_name == "#len" {
                format!("{variant_name}_{field_idx}#len")
            } else {
                format!("{variant_name}_{field_idx}_{leaf_name}")
            }
        } else if single {
            format!("{variant_name}_{field_idx}")
        } else {
            format!("{variant_name}_{field_idx}_{leaf_name}")
        };
        if let Some(element_type) = base.strip_prefix("array:") {
            let index = Sort::int(vc.ctx);
            let range = super::super::translator::z3_types::z3_element_sort(vc, element_type);
            out.push((name, Sort::array(vc.ctx, &index, &range)));
            continue;
        }
        if base == "array_len" {
            let sort = if vc.bitvec_i64 {
                Sort::bitvector(vc.ctx, 64)
            } else {
                Sort::int(vc.ctx)
            };
            out.push((name, sort));
            continue;
        }
        // `enum:`-marked leaves carry the inner enum's own datatype sort —
        // Z3 nests ADTs, so the inner sort object (cached per context) is
        // the accessor domain directly. A sort under construction recurses
        // here for its inner enum; `is_finite_adt` already bounded the nest.
        if let Some(inner_name) = base.strip_prefix("enum:") {
            let inner = vc.module_env.get_enum(inner_name)?;
            let inner_sort = enum_datatype_sort(vc, inner)?;
            out.push((name, inner_sort.sort.clone()));
            continue;
        }
        let sort = match lower(&base) {
            LoweredType::F64 if vc.ieee754_f64 => Sort::float(vc.ctx, F64_EBITS, F64_SBITS),
            LoweredType::F64 => Sort::real(vc.ctx),
            LoweredType::Str => Sort::string(vc.ctx),
            LoweredType::Bool => Sort::bool(vc.ctx),
            // `--bitvec-i64` encodes integer values as BV(64) — the
            // accessor sorts must match the args callers pass (a
            // struct param's fields are seeded BV too).
            _ if vc.bitvec_i64 => Sort::bitvector(vc.ctx, 64),
            _ => Sort::int(vc.ctx),
        };
        out.push((name, sort));
    }
    Some(out)
}

/// `(leaf_offset, leaf_width)` of declared field `field_idx` inside
/// variant `variant_idx`'s flattened constructor — the span
/// `variant_selector_apply` indexes when the field carries a composite
/// payload.
pub(crate) fn payload_leaf_range(
    enum_def: &EnumDef,
    module_env: &ModuleEnv,
    variant_idx: usize,
    field_idx: usize,
) -> Option<(usize, usize)> {
    let variant = enum_def.variants.get(variant_idx)?;
    let mut offset = 0;
    for (i, field_type) in variant.field_types.iter().enumerate() {
        if i == field_idx {
            if recursive_field_target(enum_def, field_type, module_env).is_some() {
                return Some((offset, 1));
            }
            let leaves = field_leaf_types(&payload_field_type_name(field_type), module_env)?;
            return Some((offset, leaves.len()));
        }
        if recursive_field_target(enum_def, field_type, module_env).is_some() {
            offset += 1;
        } else {
            offset += field_leaf_types(&payload_field_type_name(field_type), module_env)?.len();
        }
    }
    None
}

/// `lo <= v <= hi` for the widened integer leaf kinds `field_is_scalar`
/// admits beyond `i64`: `u64`, `u32` and `i32` (`usize`/`isize` already
/// resolved to `u64`/`i64`). `i64`/`f64`/`Str`/`bool` leaves return `None`
/// — `Int` is unbounded by design and the non-integer sorts carry their
/// own domain.
fn int_leaf_bound<'a>(ctx: &'a Context, base: &str, v: &Int<'a>) -> Option<Bool<'a>> {
    let (lo, hi) = match base {
        "u64" => (Int::from_u64(ctx, 0), Int::from_u64(ctx, u64::MAX)),
        "u32" => (
            Int::from_u64(ctx, 0),
            Int::from_u64(ctx, u64::from(u32::MAX)),
        ),
        "i32" => (
            Int::from_i64(ctx, i64::from(i32::MIN)),
            Int::from_i64(ctx, i64::from(i32::MAX)),
        ),
        _ => return None,
    };
    Some(Bool::and(ctx, &[&v.ge(&lo), &v.le(&hi)]))
}

fn bitvec_leaf_bound<'a>(ctx: &'a Context, base: &str, v: &BV<'a>) -> Option<Bool<'a>> {
    match base {
        "u32" => Some(v.bvule(&BV::from_u64(ctx, u64::from(u32::MAX), 64))),
        "i32" => {
            let lo = BV::from_i64(ctx, i64::from(i32::MIN), 64);
            let hi = BV::from_i64(ctx, i64::from(i32::MAX), 64);
            Some(Bool::and(ctx, &[&v.bvsge(&lo), &v.bvsle(&hi)]))
        }
        _ => None,
    }
}

pub(crate) fn scalar_int_leaf_domain<'a>(
    vc: &VCtx<'a>,
    type_name: &str,
    value: &Dynamic<'a>,
) -> Option<Bool<'a>> {
    let base = vc.module_env.resolve_base_type(type_name);
    if let Some(int) = value.as_int() {
        return int_leaf_bound(vc.ctx, &base, &int);
    }
    value
        .as_bv()
        .and_then(|bv| bitvec_leaf_bound(vc.ctx, &base, &bv))
}

pub(crate) fn array_int_leaf_domain<'a>(
    vc: &VCtx<'a>,
    array_value: &Dynamic<'a>,
    length_value: &Dynamic<'a>,
    element_type: &str,
) -> Option<Bool<'a>> {
    let array = array_value.as_array()?;
    let length = crate::verification::translator::z3_types::as_int_like(length_value)?;
    let index = Int::fresh_const(vc.ctx, "enum_payload_array_index");
    let lower_bound = index.ge(&Int::from_i64(vc.ctx, 0));
    let upper_bound = index.lt(&length);
    let in_bounds = Bool::and(vc.ctx, &[&lower_bound, &upper_bound]);
    let element_value = array.select(&index);
    let element_domain = scalar_int_leaf_domain(vc, element_type, &element_value)?;
    let in_range = in_bounds.implies(&element_domain);
    Some(z3::ast::forall_const(vc.ctx, &[&index], &[], &in_range))
}

pub(crate) fn enum_array_leaf_domain<'a>(
    vc: &VCtx<'a>,
    array_value: &Dynamic<'a>,
    length_value: &Dynamic<'a>,
) -> Option<Bool<'a>> {
    let array = array_value.as_array()?;
    let length = crate::verification::translator::z3_types::as_int_like(length_value)?;
    let index = z3::ast::Int::fresh_const(vc.ctx, "struct_array_enum_index");
    let in_bounds = Bool::and(
        vc.ctx,
        &[
            &index.ge(&z3::ast::Int::from_i64(vc.ctx, 0)),
            &index.lt(&length),
        ],
    );
    let selected: Dynamic = array.select(&index);
    let element_domain = enum_value_leaf_domain(vc, &selected)?;
    Some(z3::ast::forall_const(
        vc.ctx,
        &[&index],
        &[],
        &in_bounds.implies(&element_domain),
    ))
}

/// `value` is an enum datatype constant: the conjunction holding every
/// `u64`/`u32`/`i32` payload leaf inside its declared width, gated on the
/// variant that owns it (`is_V(value) => bounds_V`). The widened
/// `field_is_scalar` set lands these leaves on `Int` in default mode —
/// unbounded, they admit values the ABI cannot produce (a negative `u64`),
/// the same gap `u64` params needed an explicit domain assert for. In
/// `bitvec_i64` mode, `u32` and `i32` leaves are constrained to their
/// zero/sign-extended 32-bit domains. `None` when `value` is not a supported
/// enum datatype constant or every payload leaf is domain-exact.
pub(crate) fn enum_value_leaf_domain<'a>(vc: &VCtx<'a>, value: &Dynamic<'a>) -> Option<Bool<'a>> {
    let dt = value.as_datatype()?;
    let target_sort = dt.get_sort();
    // A datatype const's `decl()` names the constant, not its sort — pin
    // the enum by comparing sorts, the same probe
    // `variant_tester_condition` uses.
    let enum_def = vc.module_env.enums.values().find(|e| {
        uses_datatype_sort(e, vc.module_env)
            && enum_datatype_sort(vc, e)
                .map(|s| s.sort == target_sort)
                .unwrap_or(false)
    })?;
    let mut budget = MAX_ENUM_DOMAIN_NODES;
    enum_leaf_domain_for(vc, enum_def, value, MAX_ENUM_PAYLOAD_NEST, &mut budget)
}

/// Total conjunction nodes [`enum_leaf_domain_for`] may emit for one
/// enum value. The `enum:`-leaf walk multiplies variant counts level by
/// level (V outer × V inner × ...), so a declared nesting shape like 16
/// variants over 8 levels would otherwise mint ~4 billion conjuncts
/// before the solver ever sees a query. Past the cap the walk simply
/// stops descending — a dropped domain assumption only weakens what the
/// proof may assume, never what it may claim.
const MAX_ENUM_DOMAIN_NODES: usize = 1024;

/// The per-variant conjunction [`enum_value_leaf_domain`] builds for a
/// value already pinned to `enum_def`. An `enum:`-marked leaf is a single
/// inner-datatype accessor — `as_int()` cannot see it — so the walk
/// recurses into the inner enum's own leaf bounds, fueled by
/// `MAX_ENUM_PAYLOAD_NEST` like the admission-time flattening and
/// charged against `budget` per emitted node and per descent.
fn enum_leaf_domain_for<'a>(
    vc: &VCtx<'a>,
    enum_def: &crate::parser::EnumDef,
    value: &Dynamic<'a>,
    fuel: u8,
    budget: &mut usize,
) -> Option<Bool<'a>> {
    let dt = value.as_datatype()?;
    let sort = enum_datatype_sort(vc, enum_def)?;
    let mut conjuncts = Vec::new();
    for (vidx, variant) in enum_def.variants.iter().enumerate() {
        let mut accessor_idx = 0usize;
        let mut bounds: Vec<Bool> = Vec::new();
        for field in &variant.field_types {
            if recursive_field_target(enum_def, field, vc.module_env).is_some() {
                accessor_idx += 1;
                continue;
            }
            let leaves = field_leaf_types(&payload_field_type_name(field), vc.module_env)?;
            let mut leaf_idx = 0;
            while leaf_idx < leaves.len() {
                let base = &leaves[leaf_idx].1;
                if let Some(element_type) = base.strip_prefix("array:") {
                    let array = variant_selector_apply(&sort, vidx, accessor_idx, value)?;
                    let length_index = array_payload_length_index(&leaves, leaf_idx)?;
                    let length_accessor = accessor_idx + length_index.saturating_sub(leaf_idx);
                    let length = variant_selector_apply(&sort, vidx, length_accessor, value)?;
                    let nonnegative =
                        crate::verification::translator::nonneg_constraint(vc.ctx, &length)?;
                    bounds.push(nonnegative);
                    if *budget > 0 {
                        let domain = if let Some(enum_type) = element_type.strip_prefix("enum:") {
                            let _ = enum_type;
                            enum_array_leaf_domain(vc, &array, &length)
                        } else {
                            array_int_leaf_domain(vc, &array, &length, element_type)
                        };
                        if let Some(bound) = domain {
                            *budget = budget.saturating_sub(1);
                            bounds.push(bound);
                        }
                    }
                    accessor_idx += 1;
                    leaf_idx += 1;
                    continue;
                }
                if base == "array_len" {
                    accessor_idx += 1;
                    leaf_idx += 1;
                    continue;
                }
                let sel = variant_selector_apply(&sort, vidx, accessor_idx, value);
                if base == "array_len" {
                    let value = sel?;
                    bounds.push(crate::verification::translator::nonneg_constraint(
                        vc.ctx, &value,
                    )?);
                } else if let Some(inner_name) = base.strip_prefix("enum:") {
                    let inner = (fuel > 0 && *budget > 0)
                        .then(|| vc.module_env.enums.get(inner_name))
                        .flatten();
                    if let (Some(sel), Some(inner)) = (sel, inner) {
                        *budget -= 1;
                        if let Some(bound) = enum_leaf_domain_for(vc, inner, &sel, fuel - 1, budget)
                        {
                            *budget = budget.saturating_sub(1);
                            bounds.push(bound);
                        }
                    }
                } else if let Some(bound) =
                    sel.and_then(|sel| scalar_int_leaf_domain(vc, base.as_str(), &sel))
                {
                    *budget = budget.saturating_sub(1);
                    bounds.push(bound);
                }
                accessor_idx += 1;
                leaf_idx += 1;
            }
        }
        if bounds.is_empty() {
            continue;
        }
        let bound_refs: Vec<&Bool> = bounds.iter().collect();
        let is_v = sort.variants[vidx].tester.apply(&[&dt]).as_bool()?;
        *budget = budget.saturating_sub(1);
        conjuncts.push(is_v.implies(&Bool::and(vc.ctx, &bound_refs)));
    }
    if conjuncts.is_empty() {
        return None;
    }
    let conjunct_refs: Vec<&Bool> = conjuncts.iter().collect();
    Some(Bool::and(vc.ctx, &conjunct_refs))
}

/// Assert [`enum_value_leaf_domain`] for every enum datatype constant
/// seeded in `env` — call once after param/result seeding (params, bound
/// struct fields and the `result` handle all land there) so a widened
/// payload leaf cannot take a value its declared width excludes.
pub(crate) fn assert_enum_value_domains<'a>(vc: &VCtx<'a>, solver: &Solver<'a>, env: &Env<'a>) {
    for value in env.values() {
        if let Some(bound) = enum_value_leaf_domain(vc, value) {
            solver.assert(&bound);
        }
    }
    for (name, element_type) in vc.local_array_elem_types.borrow().iter() {
        let Some(leaves) = crate::verification::translator::z3_types::struct_array_leaf_types(
            element_type,
            vc.module_env,
        ) else {
            continue;
        };
        let Some(length) = env.get(&crate::verification::translator::z3_types::array_len_key(
            name,
        )) else {
            continue;
        };
        if let Some(nonnegative) =
            crate::verification::translator::z3_types::nonneg_constraint(vc.ctx, length)
        {
            solver.assert(&nonnegative);
        }
        for (path, leaf_type) in leaves {
            if !leaf_type.starts_with("enum:") {
                continue;
            }
            let key = crate::verification::translator::z3_types::struct_array_leaf_key(name, &path);
            let Some(array) = env.get(&key).and_then(|value| value.as_array()) else {
                continue;
            };
            let array: Dynamic = array.into();
            if let Some(bound) = enum_array_leaf_domain(vc, &array, length) {
                solver.assert(&bound);
            }
        }
    }
}

/// `true` when a variant field type lowers to a supported scalar Z3 sort.
/// `usize`/`isize` resolve to `u64`/`i64` inside `resolve_base_type`;
/// `i128`/`u128` and `f32` stay out (`LoweredType` has no 128-bit variant,
/// and `f32` has no sort mapping yet).
fn field_is_scalar(field_type: &str, module_env: &ModuleEnv) -> bool {
    let base = module_env.resolve_base_type(field_type);
    if module_env.enums.contains_key(&base)
        || module_env.structs.contains_key(&base)
        || base.starts_with('[')
    {
        return false;
    }
    matches!(
        lower(&base),
        LoweredType::I64
            | LoweredType::I32
            | LoweredType::U64
            | LoweredType::U32
            | LoweredType::F64
            | LoweredType::Str
            | LoweredType::Bool
    )
}

/// Builds (or reuses) the Z3 `DatatypeSort` for `enum_def` on `vc`'s context.
///
/// The sort is cached per context: redeclaring a datatype under the same name
/// would produce a *different* sort object, breaking `_eq`/`ite` between two
/// declarations of the same enum.
pub(crate) fn enum_datatype_sort<'a>(
    vc: &VCtx<'a>,
    enum_def: &EnumDef,
) -> Option<Rc<DatatypeSort<'a>>> {
    if let Some(sort) = vc.enum_sorts.borrow().get(&enum_def.name) {
        return Some(sort.clone());
    }
    if !uses_datatype_sort(enum_def, vc.module_env) {
        return None;
    }

    let group = recursive_enum_group(enum_def, vc.module_env).unwrap_or_else(|| vec![enum_def]);
    let builders = group
        .iter()
        .map(|member| enum_datatype_builder(vc, member))
        .collect::<Option<Vec<_>>>()?;
    let sorts = z3::datatype_builder::create_datatypes(builders);
    if sorts.len() != group.len() {
        return None;
    }
    let mut cache = vc.enum_sorts.borrow_mut();
    for (member, sort) in group.into_iter().zip(sorts) {
        cache.insert(member.name.clone(), Rc::new(sort));
    }
    cache.get(&enum_def.name).cloned()
}

fn enum_datatype_builder<'a>(vc: &VCtx<'a>, enum_def: &EnumDef) -> Option<z3::DatatypeBuilder<'a>> {
    let mut builder = z3::DatatypeBuilder::new(vc.ctx, enum_def.name.as_str());
    for variant in &enum_def.variants {
        let mut names = Vec::new();
        let mut accessors = Vec::new();
        for (i, field_type) in variant.field_types.iter().enumerate() {
            if let Some(target_enum) = recursive_field_target(enum_def, field_type, vc.module_env) {
                names.push(format!("{}_{}", variant.name, i));
                accessors.push(DatatypeAccessor::Datatype(target_enum.name.clone().into()));
            } else {
                for (name, sort) in
                    field_leaf_sorts(vc, &variant.name, i, &payload_field_type_name(field_type))?
                {
                    names.push(name);
                    accessors.push(DatatypeAccessor::Sort(sort));
                }
            }
        }
        let fields: Vec<(&str, DatatypeAccessor)> = names
            .iter()
            .zip(accessors)
            .map(|(n, accessor)| (n.as_str(), accessor))
            .collect();
        builder = builder.variant(variant.name.as_str(), fields);
    }
    Some(builder)
}

/// Resolves `type_name` to `(DatatypeSort, &EnumDef)` when it names an enum
/// with a native datatype sort — used by `param_z3_value` to declare enum
/// parameters as datatype constants.
pub(crate) fn datatype_sort_for_type<'a>(
    vc: &VCtx<'a>,
    type_name: &str,
) -> Option<(Rc<DatatypeSort<'a>>, &'a EnumDef)> {
    let base = vc.module_env.resolve_base_type(type_name);
    let enum_def = vc.module_env.get_enum(&base)?;
    let sort = enum_datatype_sort(vc, enum_def)?;
    Some((sort, enum_def))
}

/// Resolve `ctor_path` (`E::V` or bare `V`) to its declaring enum and the
/// variant's index — the resolution `enum_ctor_apply` performs, shared with
/// callers that need the declaration (e.g. to check payload field types).
pub(crate) fn resolve_enum_ctor<'a>(
    vc: &VCtx<'a>,
    ctor_path: &str,
) -> MumeiResult<Option<(&'a EnumDef, usize)>> {
    let variant_name = ctor_path.rsplit("::").next().unwrap_or(ctor_path);
    let enum_def = if let Some((qual, _)) = ctor_path.split_once("::") {
        // Qualified `E::V` resolves `E` directly — `find_enum_by_variant`
        // would non-deterministically pick a *different* enum declaring a
        // same-named variant (`enums` is a HashMap).
        let direct = vc.module_env.get_enum(qual);
        match direct {
            Some(def) if def.type_params.is_empty() => direct,
            _ => {
                // `E` names the generic declaration (or nothing):
                // monomorphization registers concrete instances under
                // `E<T...>`, so `Wrap::W` resolves through them. A sole
                // instance declaring the variant wins; several are
                // ambiguous — the caller can qualify `Wrap<i64>::W`.
                let instances = generic_instances(vc.module_env, qual, variant_name);
                match instances.len() {
                    0 => direct,
                    1 => Some(instances[0]),
                    _ => {
                        return Err(MumeiError::verification(format!(
                            "Ambiguous enum constructor '{ctor_path}': instances {} all declare '{variant_name}' — qualify the instance (e.g. '{qual}<...>::{variant_name}')",
                            instances
                                .iter()
                                .map(|e| e.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )));
                    }
                }
            }
        }
    } else {
        // Bare `Variant` is ambiguous when several enums declare it —
        // `enums` is a HashMap, so first-found order would make the choice
        // (and thus the datatype sort) non-deterministic across runs.
        let owners = variant_owners(vc.module_env, variant_name);
        if owners.len() > 1 {
            return Err(MumeiError::verification(format!(
                "Ambiguous enum variant '{variant_name}': declared by {} — qualify it (e.g. '{}::{variant_name}')",
                owners
                    .iter()
                    .map(|e| e.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                owners[0].name
            )));
        }
        owners.into_iter().next()
    };
    let Some(enum_def) = enum_def else {
        return Ok(None);
    };
    let Some(idx) = enum_def
        .variants
        .iter()
        .position(|v| v.name == variant_name)
    else {
        return Ok(None);
    };
    Ok(Some((enum_def, idx)))
}

/// Expand declared-arity ctor args to the constructor's flattened leaf
/// arity: scalar fields consume their arg unchanged; array and struct fields
/// consume per-argument leaf snapshots so later arguments cannot change them.
fn expand_ctor_args<'a>(
    vc: &VCtx<'a>,
    enum_def: &EnumDef,
    variant: &crate::parser::ast::EnumVariant,
    args: &[Dynamic<'a>],
    payload_snapshots: &[Option<PayloadSnapshot<'a>>],
) -> MumeiResult<Vec<Dynamic<'a>>> {
    let mut leaves: Vec<Dynamic> = Vec::new();
    for (i, arg) in args.iter().enumerate() {
        if variant
            .field_types
            .get(i)
            .is_some_and(|field| recursive_field_target(enum_def, field, vc.module_env).is_some())
        {
            leaves.push(arg.clone());
            continue;
        }
        let field_type = payload_field_type_name(&variant.field_types[i]);
        let base = vc.module_env.resolve_base_type(&field_type);
        let Some(field_leaves) = field_leaf_types(&field_type, vc.module_env) else {
            return Err(MumeiError::type_error(format!(
                "enum constructor payload '{field_type}' has no flattened leaves"
            )));
        };
        if array_payload_element_type(&field_leaves).is_some()
            || struct_array_payload_element_type(&field_type, vc.module_env).is_some()
            || vc.module_env.structs.contains_key(&base)
        {
            let Some(snapshot) = payload_snapshots.get(i).and_then(Option::as_ref) else {
                return Err(MumeiError::type_error(format!(
                    "enum constructor payload '{field_type}' has no value snapshot"
                )));
            };
            if (array_payload_element_type(&field_leaves).is_some()
                || struct_array_payload_element_type(&field_type, vc.module_env).is_some())
                && snapshot
                    .leaves
                    .first()
                    .and_then(|value| value.as_array())
                    .is_none()
            {
                return Err(MumeiError::type_error(format!(
                    "enum constructor payload '{field_type}' expects an array value"
                )));
            }
            if snapshot.leaves.len() != field_leaves.len() {
                return Err(MumeiError::type_error(format!(
                    "enum constructor payload '{field_type}' has {} leaves but its snapshot has {}",
                    field_leaves.len(),
                    snapshot.leaves.len()
                )));
            }
            leaves.extend(snapshot.leaves.iter().cloned());
        } else {
            leaves.push(arg.clone());
        }
    }
    Ok(leaves)
}

pub(crate) struct PayloadSnapshot<'a> {
    pub leaves: Vec<Dynamic<'a>>,
}

fn prove_enum_payload_domain<'a>(
    vc: &VCtx<'a>,
    solver: &Solver<'a>,
    enum_def: &EnumDef,
    variant: &crate::parser::ast::EnumVariant,
    payload_name: &str,
    type_name: &str,
    domain: &Bool<'a>,
) -> MumeiResult<()> {
    if let Some(obligations) = vc.cover_obligations.as_ref() {
        let obligation = vc.path_cond_conj().implies(domain);
        obligations.borrow_mut().push(obligation);
        return Ok(());
    }
    solver.push();
    solver.assert(&vc.path_cond_conj());
    solver.assert(&domain.not());
    let verdict = crate::verification::phase_artifacts::check(solver);
    solver.pop(1);
    if verdict != SatResult::Unsat {
        return Err(MumeiError::verification(format!(
            "Enum '{}' variant '{}' payload '{}' value may be outside the '{}' domain",
            enum_def.name, variant.name, payload_name, type_name
        ))
        .with_verdict(crate::verification::VerdictKind::Rejected));
    }
    Ok(())
}

pub(crate) fn check_enum_ctor_payload_domains<'a>(
    vc: &VCtx<'a>,
    solver: &Solver<'a>,
    enum_def: &EnumDef,
    variant: &crate::parser::ast::EnumVariant,
    leaf_args: &[Dynamic<'a>],
) -> MumeiResult<()> {
    check_enum_payload_domains(vc, solver, enum_def, variant, leaf_args, None)
}

pub(crate) fn check_enum_field_payload_domains<'a>(
    vc: &VCtx<'a>,
    solver: &Solver<'a>,
    enum_def: &EnumDef,
    variant: &crate::parser::ast::EnumVariant,
    field_idx: usize,
    leaf_args: &[Dynamic<'a>],
) -> MumeiResult<()> {
    check_enum_payload_domains(vc, solver, enum_def, variant, leaf_args, Some(field_idx))
}

fn check_enum_payload_domains<'a>(
    vc: &VCtx<'a>,
    solver: &Solver<'a>,
    enum_def: &EnumDef,
    variant: &crate::parser::ast::EnumVariant,
    leaf_args: &[Dynamic<'a>],
    only_field: Option<usize>,
) -> MumeiResult<()> {
    let mut offset = 0;
    for (field_idx, field_type_ref) in variant.field_types.iter().enumerate() {
        if only_field.is_some_and(|only| only != field_idx) {
            let width = if recursive_field_target(enum_def, field_type_ref, vc.module_env).is_some()
            {
                1
            } else {
                field_leaf_types(&payload_field_type_name(field_type_ref), vc.module_env)
                    .ok_or_else(|| {
                        MumeiError::type_error(format!(
                            "enum constructor payload '{}' has no flattened leaves",
                            payload_field_type_name(field_type_ref)
                        ))
                    })?
                    .len()
            };
            offset += width;
            continue;
        }
        let payload_name = variant
            .fields
            .get(field_idx)
            .map(|field| {
                field
                    .split_once(':')
                    .map(|(name, _)| name.trim())
                    .unwrap_or(field)
                    .to_owned()
            })
            .unwrap_or_else(|| field_idx.to_string());
        if let Some(target_enum) = recursive_field_target(enum_def, field_type_ref, vc.module_env) {
            let value = leaf_args.get(offset).ok_or_else(|| {
                MumeiError::type_error("enum constructor is missing a recursive payload leaf")
            })?;
            if let Some(domain) = enum_value_leaf_domain(vc, value) {
                prove_enum_payload_domain(
                    vc,
                    solver,
                    enum_def,
                    variant,
                    &payload_name,
                    &target_enum.name,
                    &domain,
                )?;
            }
            offset += 1;
            continue;
        }

        let field_type = payload_field_type_name(field_type_ref);
        let leaf_types = field_leaf_types(&field_type, vc.module_env).ok_or_else(|| {
            MumeiError::type_error(format!(
                "enum constructor payload '{field_type}' has no flattened leaves"
            ))
        })?;
        for (leaf_idx, (leaf_name, leaf_type)) in leaf_types.iter().enumerate() {
            if leaf_type == "array_len" {
                offset += 1;
                continue;
            }
            let value = leaf_args.get(offset).ok_or_else(|| {
                MumeiError::type_error("enum constructor is missing a payload leaf")
            })?;
            if let Some(element_type) = leaf_type.strip_prefix("array:") {
                let length_idx =
                    array_payload_length_index(&leaf_types, leaf_idx).ok_or_else(|| {
                        MumeiError::type_error("enum constructor is missing an array length leaf")
                    })?;
                let length = leaf_args
                    .get(offset + length_idx.saturating_sub(leaf_idx))
                    .ok_or_else(|| {
                        MumeiError::type_error("enum constructor is missing an array length leaf")
                    })?;
                let domain = if element_type.starts_with("enum:") {
                    enum_array_leaf_domain(vc, value, length)
                } else {
                    array_int_leaf_domain(vc, value, length, element_type)
                };
                if let Some(domain) = domain {
                    let leaf_payload_name = if leaf_name.is_empty() {
                        payload_name.clone()
                    } else {
                        format!("{payload_name}.{leaf_name}")
                    };
                    prove_enum_payload_domain(
                        vc,
                        solver,
                        enum_def,
                        variant,
                        &leaf_payload_name,
                        element_type,
                        &domain,
                    )?;
                }
                offset += 1;
                continue;
            }
            let (type_name, domain) = if let Some(enum_type) = leaf_type.strip_prefix("enum:") {
                (enum_type.to_string(), enum_value_leaf_domain(vc, value))
            } else {
                (
                    leaf_type.clone(),
                    scalar_int_leaf_domain(vc, leaf_type, value),
                )
            };
            if let Some(domain) = domain {
                let leaf_payload_name = if leaf_name.is_empty() {
                    payload_name.clone()
                } else {
                    format!("{payload_name}.{leaf_name}")
                };
                prove_enum_payload_domain(
                    vc,
                    solver,
                    enum_def,
                    variant,
                    &leaf_payload_name,
                    &type_name,
                    &domain,
                )?;
            }
            offset += 1;
        }
    }
    Ok(())
}

/// The accessor domain sorts of `enum_def.variants[idx]`'s constructor,
/// recomputed from `field_leaf_sorts` (the same source the datatype was
/// built with). Returned in leaf order so callers can bridge arg sorts —
/// under `--bitvec-i64` an integer literal stays `Int` while the accessor
/// expects `BV(64)`, and applying it raw panics inside Z3.
fn ctor_leaf_sorts<'a>(vc: &VCtx<'a>, enum_def: &EnumDef, idx: usize) -> Option<Vec<Sort<'a>>> {
    let variant = enum_def.variants.get(idx)?;
    let mut sorts = Vec::new();
    for (i, ft) in variant.field_types.iter().enumerate() {
        if let Some(target_enum) = recursive_field_target(enum_def, ft, vc.module_env) {
            sorts.push(enum_datatype_sort(vc, target_enum)?.sort.clone());
        } else {
            sorts.extend(
                field_leaf_sorts(vc, &variant.name, i, &payload_field_type_name(ft))?
                    .into_iter()
                    .map(|(_, s)| s),
            );
        }
    }
    Some(sorts)
}

/// Bridge `arg` to the accessor domain sort `want`: `Int` ↔ `BV(64)`
/// through the standard conversion points; any other mismatch fails.
fn coerce_ctor_leaf<'a>(arg: &Dynamic<'a>, want: &Sort<'a>) -> Option<Dynamic<'a>> {
    if arg.get_sort() == *want {
        return Some(arg.clone());
    }
    match want.kind() {
        z3::SortKind::BV => crate::verification::translator::as_bv_i64(arg).map(Into::into),
        z3::SortKind::Int => crate::verification::translator::as_int_like(arg).map(Into::into),
        _ => None,
    }
}

/// Applies the constructor of `enum_def.variants[idx]` to `leaf_args`,
/// already at the flattened leaf arity (one entry per accessor).
pub(crate) fn enum_ctor_apply_leaves<'a>(
    vc: &VCtx<'a>,
    enum_def: &EnumDef,
    idx: usize,
    leaf_args: &[Dynamic<'a>],
) -> Option<Dynamic<'a>> {
    let sort = enum_datatype_sort(vc, enum_def)?;
    if sort.variants[idx].accessors.len() != leaf_args.len() {
        return None;
    }
    let want = ctor_leaf_sorts(vc, enum_def, idx)?;
    if want.len() != leaf_args.len() {
        return None;
    }
    let coerced: Vec<Dynamic> = leaf_args
        .iter()
        .zip(&want)
        .map(|(arg, s)| coerce_ctor_leaf(arg, s))
        .collect::<Option<Vec<_>>>()?;
    let arg_refs: Vec<&dyn Ast> = coerced.iter().map(|d| d as &dyn Ast).collect();
    Some(sort.variants[idx].constructor.apply(&arg_refs))
}

/// `Enum::Variant(args)` / `Variant(args)` construction — applies the
/// variant's Z3 constructor to `args` (already lowered, at the declared
/// arity; composite payload fields expand to their leaves). Returns `None`
/// when the name does not resolve to a native-datatype variant or the arity is
/// wrong — callers then keep the previous behaviour (unknown/atom call).
pub(crate) fn enum_ctor_apply<'a>(
    vc: &VCtx<'a>,
    _env: &mut crate::verification::types::Env<'a>,
    ctor_path: &str,
    args: &[Dynamic<'a>],
    payload_snapshots: &[Option<PayloadSnapshot<'a>>],
    solver_opt: Option<&Solver<'a>>,
) -> MumeiResult<Option<Dynamic<'a>>> {
    let Some((enum_def, idx)) = resolve_enum_ctor(vc, ctor_path)? else {
        return Ok(None);
    };
    if enum_datatype_sort(vc, enum_def).is_none() {
        return Ok(None);
    };
    if enum_def.variants[idx].fields.len() != args.len() {
        return Ok(None);
    }
    let leaf_args = expand_ctor_args(
        vc,
        enum_def,
        &enum_def.variants[idx],
        args,
        payload_snapshots,
    )?;
    if let Some(solver) = solver_opt {
        check_enum_ctor_payload_domains(vc, solver, enum_def, &enum_def.variants[idx], &leaf_args)?;
    }
    Ok(enum_ctor_apply_leaves(vc, enum_def, idx, &leaf_args))
}

/// Tester predicate `is-<Variant>(target)` for a `Datatype`-sorted match
/// target. Returns `(sort, tester_bool, variant_idx, enum_def)`, or `None` when no
/// supported ADT owns `variant_name` on `target`'s sort. When several enums
/// share the variant name, the one whose datatype sort matches `target`'s
/// wins — the name-only `find_enum_by_variant` scan would pick the wrong
/// enum for colliding names.
pub(crate) fn variant_tester_condition<'a>(
    vc: &VCtx<'a>,
    target: &Dynamic<'a>,
    variant_name: &str,
) -> Option<(Rc<DatatypeSort<'a>>, Bool<'a>, usize, &'a EnumDef)> {
    let dt = target.as_datatype()?;
    let target_sort = dt.get_sort();
    // Qualified `E::V` pins the enum by its qualifier — a qualifier that
    // names a different enum yields no tester and falls through to
    // `resolve_variant_owner`, which reports the mismatch. A qualifier that
    // is not a known enum name resolves by the leaf, like a bare name.
    let (qual, variant_name) = match variant_name.rsplit_once("::") {
        Some((q, leaf)) => (Some(q), leaf),
        None => (None, variant_name),
    };
    let qual_enum = qual.and_then(|q| vc.module_env.get_enum(q));
    for enum_def in vc.module_env.enums.values() {
        if let Some(qe) = qual_enum {
            // A qualifier naming a generic declaration (`Wrap`) admits any
            // `Wrap<...>` instance — the target sort comparison below pins
            // the concrete one (the generic def has no sort anyway).
            let matches = if qe.type_params.is_empty() {
                enum_def.name == qe.name
            } else {
                enum_def.name.starts_with(&format!("{}<", qe.name))
            };
            if !matches {
                continue;
            }
        }
        let Some(idx) = enum_def
            .variants
            .iter()
            .position(|v| v.name == variant_name)
        else {
            continue;
        };
        let Some(sort) = enum_datatype_sort(vc, enum_def) else {
            continue;
        };
        if sort.sort != target_sort {
            continue;
        }
        let is_v = sort.variants[idx].tester.apply(&[&dt]).as_bool()?;
        return Some((sort, is_v, idx, enum_def));
    }
    None
}

/// Applies the `field_idx`-th selector of variant `variant_idx` to `target`:
/// `<Variant>_<field_idx>(target)`.
pub(crate) fn variant_selector_apply<'a>(
    sort: &DatatypeSort<'a>,
    variant_idx: usize,
    field_idx: usize,
    target: &Dynamic<'a>,
) -> Option<Dynamic<'a>> {
    let dt = target.as_datatype()?;
    // `field_idx` comes from the user's pattern, which may name more payload
    // slots than the variant declares — index safely (caller falls back to a
    // fresh projector const).
    let accessor = sort.variants[variant_idx].accessors.get(field_idx)?;
    Some(accessor.apply(&[&dt]))
}

/// All enums declaring `variant_name`, sorted by name — `module_env.enums`
/// is a `HashMap`, so iteration order is not stable between processes.
/// `E::V` → `V`; bare `V` → `V`. `EnumVariant.name` stores the leaf segment
/// only, so qualified pattern names (`Mine::Cons`) compare by the leaf.
pub(crate) fn variant_leaf(variant_name: &str) -> &str {
    variant_name.rsplit("::").next().unwrap_or(variant_name)
}

/// Registered monomorphized instances `qual<T...>` that declare variant
/// `leaf` — the concrete enums `Wrap::W` can mean when `Wrap` names a
/// generic declaration. Sorted by name for deterministic ambiguity errors.
pub(crate) fn generic_instances<'m>(
    module_env: &'m ModuleEnv,
    qual: &str,
    leaf: &str,
) -> Vec<&'m EnumDef> {
    let prefix = format!("{qual}<");
    let mut owners: Vec<_> = module_env
        .enums
        .values()
        .filter(|e| {
            e.name.starts_with(prefix.as_str()) && e.variants.iter().any(|v| v.name == leaf)
        })
        .collect();
    owners.sort_by(|a, b| a.name.cmp(&b.name));
    owners
}

/// The `qual<T...>` instance that owns `leaf`, chosen among the registered
/// instances of generic enum `qual`: a `declared` type name pins it, a
/// sole registered instance wins, several are ambiguous, zero is `None`.
fn generic_instance_owner<'a>(
    vc: &VCtx<'a>,
    qual: &str,
    leaf: &str,
    declared: &[String],
) -> MumeiResult<Option<&'a EnumDef>> {
    for decl in declared {
        if let Some(e) = vc.module_env.get_enum(decl.as_str()) {
            if e.name.starts_with(&format!("{qual}<")) && e.variants.iter().any(|v| v.name == leaf)
            {
                return Ok(Some(e));
            }
        }
    }
    let instances = generic_instances(vc.module_env, qual, leaf);
    match instances.len() {
        0 => Ok(None),
        1 => Ok(Some(instances[0])),
        _ => Err(MumeiError::verification(format!(
            "'{qual}::{leaf}' is ambiguous: instances {} all declare it — qualify the concrete type",
            instances
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

pub(crate) fn variant_owners<'m>(
    module_env: &'m ModuleEnv,
    variant_name: &str,
) -> Vec<&'m EnumDef> {
    let leaf = variant_leaf(variant_name);
    let mut owners: Vec<_> = module_env
        .enums
        .values()
        .filter(|e| e.variants.iter().any(|v| v.name == leaf))
        .collect();
    owners.sort_by(|a, b| a.name.cmp(&b.name));
    owners
}

/// `Option<i64>`/`[Color]` → base type name (`Option`, `Color`).
pub(crate) fn type_name_base(ty: &str) -> &str {
    let base = ty
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(ty);
    base.split('<').next().unwrap_or(base).trim()
}

/// The enum a `match` target was declared as, when the target is a bare
/// parameter or `result` constant — `match l` on `l: IntList` (or
/// `match result` on `-> IntList`) yields `Some("IntList")`.
fn target_param_enum_name(vc: &VCtx, target: &Dynamic) -> Option<String> {
    let sym = target.as_int()?.decl().name();
    // `__proj_{Enum}_{Variant}_{i}` — a bound field const's declared type
    // is that variant's i-th field type (`Self` → the enum itself), so
    // `match t` on a bound tail (`Cons(h, t)`) resolves the same enum the
    // outer arm used, while `match m` on a field of a *different* enum
    // type (`Outer::Wrap(m)`) resolves `Mine`, not `Outer`. Enum/variant
    // names may themselves contain `_`, so take the longest prefix that is
    // a known enum.
    if let Some(rest) = sym.strip_prefix("__proj_") {
        if let Some((head, idx)) = rest.rsplit_once('_') {
            let mut best: Option<usize> = None;
            for (pos, _) in head.match_indices('_') {
                if vc.module_env.get_enum(&head[..pos]).is_some() {
                    best = Some(pos);
                }
            }
            if let Some(pos) = best {
                let enum_name = &head[..pos];
                let variant_name = &head[pos + 1..];
                // The const is a bound field — its declared type is the
                // variant's i-th field type (`Self` → the enum itself), not
                // the outer enum: `match m` on `Outer::Wrap(m)`'s `m`
                // resolves `Wrap`'s field type, not `Outer`. A non-enum
                // field means the const is not enum-typed at all.
                if let (Some(edef), Ok(i)) =
                    (vc.module_env.get_enum(enum_name), idx.parse::<usize>())
                {
                    if let Some(ft) = edef
                        .variants
                        .iter()
                        .find(|v| v.name == variant_name)
                        .and_then(|v| v.field_types.get(i))
                    {
                        let resolved = if ft.name == enum_name {
                            enum_name.to_string()
                        } else {
                            vc.module_env.resolve_base_type(ft.name.as_str())
                        };
                        return vc.module_env.get_enum(&resolved).map(|e| e.name.clone());
                    }
                }
                return Some(enum_name.to_string());
            }
        }
    }
    let atom = vc.current_atom?;
    let declared = if sym == "result" {
        atom.return_type.as_deref()
    } else {
        atom.params
            .iter()
            .find(|p| p.name == sym)
            .and_then(|p| p.type_name.as_deref())
    };
    if let Some(ty) = declared {
        // A generic instance is itself a registered enum: keep the full
        // `Wrap<i64>` name so `match e` binds the instance, not the
        // generic def `type_name_base` would strip down to.
        if vc.module_env.get_enum(ty).is_some() {
            return Some(ty.to_string());
        }
        return Some(type_name_base(ty).to_string());
    }
    // `let`-bound enum values record their inferred declared type in
    // `local_enum_types` — `match e` after `let e = Mine::Cons(1)` resolves
    // the same owner a declared parameter type would.
    if let Some(local) = vc.local_enum_types.borrow().get(sym.as_str()) {
        return Some(local.clone());
    }
    // Struct-field projections are seeded as `<binding>_<field>` consts
    // (e.g. `match th.t` on `th: Thermo` yields `th_t`), so the match
    // target's declared enum type is recoverable by walking the struct
    // definition. Field names may themselves contain `_` (and fields may be
    // nested structs), so try each split position: the first prefix that is
    // a struct-typed parameter whose remaining path resolves wins.
    for (pos, _) in sym.match_indices('_') {
        let (binding, path) = sym.split_at(pos);
        let path = &path[1..];
        // The const prefix names the *binding whose field* is projected —
        // an actual argument's name in the caller, not a callee param. So
        // the caller's own parameters win over a callee's same-named param,
        // and among nested instantiation frames the outermost (oldest)
        // frame is the context that seeded the const.
        let param_ty = atom
            .params
            .iter()
            .find(|p| p.name == binding)
            .and_then(|p| p.type_name.clone())
            .or_else(|| {
                vc.instantiation_params
                    .borrow()
                    .iter()
                    .find(|(n, _)| n == binding)
                    .and_then(|(_, ty)| ty.clone())
            });
        let Some(struct_name) = param_ty.as_deref().map(type_name_base) else {
            continue;
        };
        if let Some(field_ty) = field_type_at_path(vc.module_env, struct_name, path) {
            return Some(type_name_base(&field_ty).to_string());
        }
    }
    None
}

/// Resolve a `_`-joined field path (`point_x` on `point: Point` → `Point`'s
/// field `x`) inside `struct_name`. Returns the terminal field's type name.
fn field_type_at_path(module_env: &ModuleEnv, struct_name: &str, path: &str) -> Option<String> {
    let sdef = module_env.get_struct(struct_name)?;
    for field in &sdef.fields {
        if path == field.name {
            return Some(field.type_name.clone());
        }
        if let Some(rest) = path.strip_prefix(&format!("{}_", field.name)) {
            if let Some(nested) =
                field_type_at_path(module_env, type_name_base(&field.type_name), rest)
            {
                return Some(nested);
            }
        }
    }
    None
}

/// Declared type of a bare binding: a parameter's `type_name`, `result`'s
/// return type, or the inferred enum type recorded for a `let`/`assign`
/// binding (`let e = Mine::Cons(1)` records `e: Mine`).
pub(crate) fn binding_declared_type(vc: &VCtx, name: &str) -> Option<String> {
    let atom = vc.current_atom;
    if name == "result" {
        return atom.and_then(|a| a.return_type.clone());
    }
    // Locals shadow params: a `let e = …` binding — or a callee param
    // seeded during contract instantiation — must win over a same-named
    // parameter of the current atom.
    vc.local_enum_types
        .borrow()
        .get(name)
        .cloned()
        .or_else(|| vc.local_struct_types.borrow().get(name).cloned())
        .or_else(|| {
            // A name a callee declares wins over a same-named parameter of
            // the current atom while that callee's contract is instantiated
            // (`current_atom.params` still belongs to the caller).
            vc.instantiation_params
                .borrow()
                .iter()
                .rev()
                .find(|(n, _)| n == name)
                .and_then(|(_, ty)| ty.clone())
        })
        .or_else(|| {
            atom.and_then(|a| {
                a.params
                    .iter()
                    .find(|p| p.name == name)
                    .and_then(|p| p.type_name.clone())
            })
        })
}

/// Declared type name of an expression, when statically recoverable:
/// parameters, `result`, `let`-bound enum values, calls to atoms with a
/// declared return type, and `FieldAccess` chains rooted at a struct-typed
/// expression or a uniquely named enum payload (`h.r`, `h.a.b`, `e.p.inner`,
/// `result.r`, `f(x).r`). Field names are matched exactly per segment, so
/// `_`-joined const-name ambiguity does not arise. The raw declared name is
/// returned; callers resolve aliases and use `get_enum` as needed.
pub(crate) fn declared_type_of_expr(vc: &VCtx, expr: &Expr) -> Option<String> {
    match expr {
        Expr::Variable(name) => binding_declared_type(vc, name),
        Expr::Call(name, _) => vc
            .module_env
            .get_atom(name)
            .and_then(|a| a.return_type.clone()),
        Expr::StructInit { type_name, .. } => Some(type_name.clone()),
        Expr::FieldAccess(..) => {
            let mut fields: Vec<&String> = Vec::new();
            let mut cur = expr;
            while let Expr::FieldAccess(inner, field) = cur {
                fields.push(field);
                cur = inner;
            }
            let mut ty = declared_type_of_expr(vc, cur)?;
            for field in fields.iter().rev() {
                ty = declared_field_type(vc, &ty, field)?;
            }
            Some(ty)
        }
        Expr::Block(stmt) => declared_type_of_stmt(vc, stmt),
        Expr::IfThenElse {
            then_branch,
            else_branch,
            ..
        } => {
            let then_ty = declared_type_of_stmt(vc, then_branch)?;
            let else_ty = declared_type_of_stmt(vc, else_branch)?;
            (then_ty == else_ty).then_some(then_ty)
        }
        Expr::Match { arms, .. } => {
            let mut iter = arms.iter();
            let first = declared_type_of_stmt(vc, iter.next()?.body.as_ref())?;
            iter.all(|arm| {
                declared_type_of_stmt(vc, arm.body.as_ref()).as_deref() == Some(first.as_str())
            })
            .then_some(first)
        }
        _ => None,
    }
}

fn declared_field_type(vc: &VCtx, parent_type: &str, field: &str) -> Option<String> {
    let resolved = vc.module_env.resolve_base_type(parent_type);
    let base = type_name_base(&resolved);
    if let Some(sdef) = vc.module_env.get_struct(base) {
        return sdef
            .fields
            .iter()
            .find(|f| f.name == field)
            .map(|f| f.type_name.clone());
    }
    let edef = vc
        .module_env
        .get_enum(&resolved)
        .or_else(|| vc.module_env.get_enum(base))?;
    if field == "tag" {
        return None;
    }
    let mut owner = None;
    for variant in &edef.variants {
        let Some(fidx) = variant.fields.iter().position(|payload| {
            payload
                .split_once(':')
                .is_some_and(|(name, _)| name.trim() == field)
        }) else {
            continue;
        };
        if owner.is_some() {
            return None;
        }
        owner = variant
            .field_types
            .get(fidx)
            .map(|payload| payload.name.clone());
    }
    owner
}

/// Declared type of a statement's tail value: a `Stmt::Expr` yields its
/// expression's type; a `Stmt::Block` recurses into its last element.
fn declared_type_of_stmt(vc: &VCtx, stmt: &Stmt) -> Option<String> {
    match stmt {
        Stmt::Expr(expr, _) => declared_type_of_expr(vc, expr),
        Stmt::Block(stmts, _) => stmts.last().and_then(|s| declared_type_of_stmt(vc, s)),
        _ => None,
    }
}

/// Signature a `Variant` pattern encodes on the Int-tag path: the variant's
/// tag index plus its payload field types (recursive `Self` fields project
/// as `i64` tags). When several enums declare the same variant name and the
/// signatures agree, the encoding does not depend on which enum is picked.
fn int_tag_sig(
    module_env: &ModuleEnv,
    enum_def: &EnumDef,
    variant_name: &str,
) -> Option<(usize, Vec<String>)> {
    enum_def
        .variants
        .iter()
        .position(|v| v.name == variant_leaf(variant_name))
        .map(|i| {
            (
                i,
                enum_def.variants[i]
                    .fields
                    .iter()
                    .map(|f| {
                        if *f == enum_def.name {
                            "i64".to_string()
                        } else {
                            module_env.resolve_base_type(f)
                        }
                    })
                    .collect::<Vec<_>>(),
            )
        })
}

/// Resolves the enum that owns `variant_name` for a `Variant` pattern on an
/// Int-tag `match` target.
///
/// `find_enum_by_variant` picks the first owner it meets in `enums` (a
/// `HashMap`) — when several enums declare the same variant name (the
/// prelude always contributes `Option`/`Result`/`List`), which owner it
/// returns, and therefore which tag index a `Cons` arm encodes, flips
/// between processes. Resolution order:
/// 1. the enum named by `decl_hint` (the scrutinee expression's declared
///    type — e.g. `match result` on `-> IntList`) or by the match target's
///    own declared parameter type (`match l` on `l: IntList`);
/// 2. the sole owner, or any owner when every owner assigns the variant
///    the same tag index and payload types;
/// 3. otherwise the match cannot be encoded soundly — an error.
pub(crate) fn resolve_variant_owner<'a>(
    vc: &VCtx<'a>,
    target: &Dynamic<'a>,
    variant_name: &str,
    decl_hint: Option<&str>,
) -> MumeiResult<Option<&'a EnumDef>> {
    // Qualified `E::V`: a qualifier naming a known enum pins the owner and
    // must agree with the target's declared type — `match e { Other::V }`
    // on `e: Mine` must not fall back to Mine's bare `V`. An unknown
    // qualifier (module path etc.) resolves by the leaf name, as before.
    if let Some((qual, leaf)) = variant_name.rsplit_once("::") {
        if let Some(qual_enum) = vc.module_env.get_enum(qual) {
            let declared: Vec<String> = decl_hint
                .map(str::to_string)
                .into_iter()
                .chain(target_param_enum_name(vc, target))
                // A `Datatype`-sorted target's declared enum is the enum
                // whose sort it carries — `target_param_enum_name` only
                // recovers Int-sorted const names.
                .chain(
                    target
                        .as_datatype()
                        .and_then(|dt| {
                            let tsort = dt.get_sort();
                            vc.module_env.enums.values().find(|e| {
                                enum_datatype_sort(vc, e).is_some_and(|s| s.sort == tsort)
                            })
                        })
                        .map(|e| e.name.clone()),
                )
                .collect();
            // The qualifier may name the generic declaration: `match e {
            // Wrap::W }` on `e: Wrap<i64>` binds the concrete instance,
            // since the generic def itself is not encodable.
            let qual_enum = if qual_enum.type_params.is_empty() {
                qual_enum
            } else {
                generic_instance_owner(vc, qual, leaf, &declared)?.unwrap_or(qual_enum)
            };
            for decl in &declared {
                if let Some(decl_enum) = vc.module_env.get_enum(decl) {
                    if decl_enum.name != qual_enum.name {
                        return Err(MumeiError::verification(format!(
                            "Match arm '{variant_name}' belongs to enum '{}' but the match target is declared as '{}'",
                            qual_enum.name, decl_enum.name
                        )));
                    }
                }
            }
            return if qual_enum.variants.iter().any(|v| v.name == leaf) {
                Ok(Some(qual_enum))
            } else {
                Err(MumeiError::verification(format!(
                    "Enum '{}' has no variant named '{leaf}'",
                    qual_enum.name
                )))
            };
        }
        return resolve_variant_owner(vc, target, leaf, decl_hint);
    }
    let owners = variant_owners(vc.module_env, variant_name);
    if owners.is_empty() {
        return Ok(None);
    }
    let declared = decl_hint
        .map(str::to_string)
        .into_iter()
        .chain(target_param_enum_name(vc, target));
    for decl in declared {
        if let Some(decl_enum) = vc.module_env.get_enum(&decl) {
            return if owners.iter().any(|o| o.name == decl_enum.name) {
                Ok(Some(decl_enum))
            } else {
                Err(MumeiError::verification(format!(
                    "Enum '{}' has no variant named '{variant_name}'",
                    decl_enum.name
                )))
            };
        }
    }
    if owners.len() == 1 {
        return Ok(Some(owners[0]));
    }
    let first = int_tag_sig(vc.module_env, owners[0], variant_name);
    if owners
        .iter()
        .all(|e| int_tag_sig(vc.module_env, e, variant_name) == first)
    {
        return Ok(Some(owners[0]));
    }
    Err(MumeiError::verification(format!(
        "Ambiguous enum variant '{variant_name}' in match: declared by {} with conflicting tags — rename the variants or give the match target a declared enum type",
        owners
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    )))
}

/// Declared enum type of an expression whose value is being bound
/// (`let e = …` / `e = …`), inferred statically from the AST: qualified
/// constructor calls `E::V(..)` and unit constructors `E::V`, variables
/// already recorded in `local_enum_types` (or declared enum parameters),
/// `if`/nested blocks whose arms agree, `match` whose arm bodies agree, and
/// calls to atoms declared to return an enum. Lets `match e` on a `let`-
/// bound enum resolve variant owners the same way `match m` on `m: Mine`
/// does — without this the owner set is ambiguous across prelude enums.
/// Enum-valued struct fields and uniquely named enum-payload fields are also
/// resolved from their declared types.
fn resolved_enum_name(vc: &VCtx, type_name: &str) -> Option<String> {
    let resolved = vc.module_env.resolve_base_type(type_name);
    vc.module_env
        .get_enum(&resolved)
        .or_else(|| vc.module_env.get_enum(type_name_base(&resolved)))
        .map(|e| e.name.clone())
}

pub(crate) fn infer_expr_enum_name(vc: &VCtx, expr: &Expr) -> Option<String> {
    match expr {
        Expr::Variable(v) => binding_declared_type(vc, v).and_then(|t| resolved_enum_name(vc, &t)),
        Expr::Call(name, _) => {
            if let Some((enum_name, variant)) = name.split_once("::") {
                let edef = vc
                    .module_env
                    .get_enum(enum_name)
                    .filter(|e| e.variants.iter().any(|v| v.name == variant))?;
                // A generic declaration names its instances at use sites:
                // record the sole registered instance so `e.tag` reads the
                // concrete tag domain instead of an unconstrained int.
                if edef.type_params.is_empty() {
                    Some(edef.name.clone())
                } else {
                    let inst = generic_instances(vc.module_env, enum_name, variant);
                    Some(if inst.len() == 1 {
                        inst[0].name.clone()
                    } else {
                        edef.name.clone()
                    })
                }
            } else {
                vc.module_env
                    .get_atom(name)
                    .and_then(|a| a.return_type.as_deref())
                    .and_then(|t| resolved_enum_name(vc, t))
            }
        }
        Expr::FieldAccess(inner, field) => {
            // `E.V` unit constructor: the base names the enum directly.
            if let Expr::Variable(base) = inner.as_ref() {
                if let Some(e) = vc
                    .module_env
                    .get_enum(base)
                    .filter(|e| e.variants.iter().any(|v| v.name == *field))
                {
                    return Some(e.name.clone());
                }
            }
            // `let s = h.r` — the declared type of a field chain rooted at a
            // struct- or enum-typed binding; `match s` then resolves like
            // `match h.r`.
            declared_type_of_expr(vc, expr).and_then(|t| resolved_enum_name(vc, &t))
        }
        Expr::IfThenElse {
            then_branch,
            else_branch,
            ..
        } => {
            let t = infer_stmt_enum_name(vc, then_branch);
            let e = infer_stmt_enum_name(vc, else_branch);
            if t.is_some() && t == e {
                t
            } else {
                None
            }
        }
        Expr::Block(stmt) => infer_stmt_enum_name(vc, stmt),
        Expr::Match { arms, .. } => {
            let mut names = arms
                .iter()
                .filter_map(|arm| infer_stmt_enum_name(vc, &arm.body));
            let first = names.next()?;
            names.all(|n| n == first).then_some(first)
        }
        _ => None,
    }
}

/// The enum type produced by a statement when it ends in a value expression
/// (`Stmt::Expr`) or a block whose tail does.
fn infer_stmt_enum_name(vc: &VCtx, stmt: &Stmt) -> Option<String> {
    match stmt {
        Stmt::Expr(e, _) => infer_expr_enum_name(vc, e),
        Stmt::Block(stmts, _) => stmts.last().and_then(|s| infer_stmt_enum_name(vc, s)),
        _ => None,
    }
}

/// `param_z3_value`, but enum-typed names lower to real `Datatype` constants
/// when the type has a native datatype sort. All scalar/array behaviour is delegated to
/// `param_z3_value` unchanged.
pub(crate) fn param_z3_value_for_vc<'a>(
    vc: &VCtx<'a>,
    name: &str,
    type_name: Option<&str>,
) -> Dynamic<'a> {
    if type_name.is_some_and(|ty| ty.starts_with('[') && ty.ends_with(']')) {
        let element_type = super::super::translator::z3_types::array_element_type_from_annotation(
            type_name,
            vc.module_env,
        );
        let range = super::super::translator::z3_types::z3_element_sort(vc, &element_type);
        return z3::ast::Array::new_const(vc.ctx, name, &Sort::int(vc.ctx), &range).into();
    }
    if let Some(type_name) = type_name {
        if let Some((sort, _)) = datatype_sort_for_type(vc, type_name) {
            return Datatype::new_const(vc.ctx, name, &sort.sort).into();
        }
    }
    crate::verification::translator::param_z3_value(
        vc.ctx,
        name,
        type_name,
        vc.module_env,
        vc.ieee754_f64,
        vc.bitvec_i64,
    )
}

/// `HashMap` alias used by `VCtx::enum_sorts`.
pub(crate) type EnumSortCache<'a> = HashMap<String, Rc<DatatypeSort<'a>>>;
