//! Plan 10 / P10-C: native Z3 `Datatype` sorts for finite, non-recursive
//! enums (`Z3_DATATYPE_SORT`).
//!
//! A mumei `enum` that is not marked `is_recursive` by the parser (no `Self`
//! payload), is not generic (`type_params` is empty — `Option<T>` stays out
//! until a monomorphisation layer exists), and whose payload fields all
//! resolve to `i64`/`f64`/`Str`/`bool` through `resolve_base_type` (fields
//! that are other enums, structs, or arrays keep the enum on the Int-tag
//! path) is lowered to a real Z3 datatype: variants become constructors with
//! `is-<Variant>` testers and per-field selectors of the *correct* sort (a
//! `Str` payload projects as a Z3 `String`, not as an Int tag).
//!
//! Recursive enums keep the Int-tag encoding and the `inductive_data_type`
//! fragment tag (Lean 4 delegation boundary).

use std::collections::HashMap;
use std::rc::Rc;

use z3::ast::{Ast, Bool, Datatype, Dynamic};
use z3::{Context, DatatypeAccessor, DatatypeSort, Sort};

use crate::lowering::{lower, LoweredType};
use crate::parser::ast::{EnumDef, Expr, Stmt};
use crate::verification::module_env::ModuleEnv;
use crate::verification::translator::{VCtx, F64_EBITS, F64_SBITS};
use crate::verification::types::{MumeiError, MumeiResult};

/// `true` when `enum_def` is finite + non-recursive + scalar-payload only
/// (a declared *struct* payload whose direct fields are all scalar also
/// counts: its leaves flatten into the constructor, mirroring the trusted
/// verifier's `pure-i64-composite/v18` layout). Enum, array and nested
/// composite payloads keep the Int-tag path.
pub(crate) fn is_finite_adt(enum_def: &EnumDef, module_env: &ModuleEnv) -> bool {
    // Empty enums cannot form a Z3 datatype (`DatatypeBuilder::finish`
    // asserts at least one variant) and never have a constructible value, so
    // they keep the Int-tag encoding — the vacuous `all()` below would
    // otherwise claim them.
    if enum_def.is_recursive || !enum_def.type_params.is_empty() || enum_def.variants.is_empty() {
        return false;
    }
    enum_def.variants.iter().all(|variant| {
        variant
            .field_types
            .iter()
            .all(|field| field_leaf_types(field.name.as_str(), module_env).is_some())
    })
}

/// The `(leaf_name, leaf_base_type)` pairs a payload field contributes to
/// the Z3 constructor: a scalar field is a single unnamed leaf; a declared
/// struct expands to its scalar leaves, with a struct-typed field recursing
/// under a `{field}_{leaf}` name (`b.inner.x` → leaf `inner_x`, matching the
/// `<binding>_<path>` env-key convention). Enum and array leaves, a
/// recursive struct on the flattening path, a leaf-name collision
/// (`inner_x` scalar beside `inner: {x}`), or more than
/// `MAX_PAYLOAD_LEAVES` leaves return `None` and keep the enum on the
/// Int-tag encoding.
pub(crate) fn field_leaf_types(
    field_type: &str,
    module_env: &ModuleEnv,
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

fn collect_field_leaves(
    field_type: &str,
    module_env: &ModuleEnv,
    name: String,
    leaves: &mut Vec<(String, String)>,
    visiting: &mut std::collections::HashSet<String>,
    seen: &mut std::collections::HashSet<String>,
    work: &mut usize,
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
        )?;
    }
    visiting.remove(&base);
    Some(())
}

/// The declared leaf values of a struct-typed payload value, in
/// `field_leaf_types` order. The value's flat env bindings
/// (`__struct_<handle>_<path>` entries, incl. deeper `<field>_<leaf>`
/// re-homes) are keyed exactly like the leaf names, so a match-bound
/// composite (`b` for `V(b: Box{inner: Point})` → `inner_x`) and a
/// `StructInit` literal answer identically.
pub(crate) fn flattened_value_leaves<'a>(
    env: &crate::verification::types::Env<'a>,
    value: &Dynamic<'a>,
    leaf_types: &[(String, String)],
) -> Option<Vec<Dynamic<'a>>> {
    if leaf_types.len() == 1 && leaf_types[0].0.is_empty() {
        return Some(vec![value.clone()]);
    }
    let flat: std::collections::HashMap<String, Dynamic> =
        crate::verification::translator::nested_field_bindings(env, value)
            .into_iter()
            .collect();
    leaf_types
        .iter()
        .map(|(name, _)| flat.get(name).cloned())
        .collect()
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
    Some(
        leaves
            .into_iter()
            .map(|(leaf_name, base)| {
                let name = if single {
                    format!("{variant_name}_{field_idx}")
                } else {
                    format!("{variant_name}_{field_idx}_{leaf_name}")
                };
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
                (name, sort)
            })
            .collect(),
    )
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
        let leaves = field_leaf_types(&field_type.name, module_env)?;
        if i == field_idx {
            return Some((offset, leaves.len()));
        }
        offset += leaves.len();
    }
    None
}

/// `true` when a variant field type lowers to a scalar Z3 sort. Enum, struct,
/// and array payloads keep the enum on the Int-tag path.
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
        LoweredType::I64 | LoweredType::F64 | LoweredType::Str | LoweredType::Bool
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
    if !is_finite_adt(enum_def, vc.module_env) {
        return None;
    }
    if let Some(sort) = vc.enum_sorts.borrow().get(&enum_def.name) {
        return Some(sort.clone());
    }
    let mut builder = z3::DatatypeBuilder::new(vc.ctx, enum_def.name.as_str());
    for variant in &enum_def.variants {
        let mut names = Vec::new();
        let mut sorts = Vec::new();
        for (i, field_type) in variant.field_types.iter().enumerate() {
            for (name, sort) in field_leaf_sorts(vc, &variant.name, i, &field_type.name)? {
                names.push(name);
                sorts.push(sort);
            }
        }
        let fields: Vec<(&str, DatatypeAccessor)> = names
            .iter()
            .zip(sorts)
            .map(|(n, s)| (n.as_str(), DatatypeAccessor::Sort(s)))
            .collect();
        builder = builder.variant(variant.name.as_str(), fields);
    }
    let sort = Rc::new(builder.finish());
    vc.enum_sorts
        .borrow_mut()
        .insert(enum_def.name.clone(), sort.clone());
    Some(sort)
}

/// Resolves `type_name` to `(DatatypeSort, &EnumDef)` when it names a finite
/// ADT — used by `param_z3_value` to declare enum-typed parameters as real
/// datatype constants.
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
/// arity: scalar fields consume their arg unchanged; a struct-typed field
/// consumes the arg's projected leaves (`struct_fields_of_value`), so both
/// `Event::Move(Point { x, y })` and `Event::Move(p)` reach the leaves.
fn expand_ctor_args<'a>(
    vc: &VCtx<'a>,
    env: &crate::verification::types::Env<'a>,
    variant: &crate::parser::ast::EnumVariant,
    args: &[Dynamic<'a>],
) -> MumeiResult<Vec<Dynamic<'a>>> {
    let mut leaves: Vec<Dynamic> = Vec::new();
    for (i, arg) in args.iter().enumerate() {
        let field_type = &variant.field_types[i].name;
        let base = vc.module_env.resolve_base_type(field_type);
        if vc.module_env.structs.contains_key(&base) {
            let Some(leaf_types) = field_leaf_types(field_type, vc.module_env) else {
                return Err(MumeiError::type_error(format!(
                    "enum constructor payload '{field_type}' has no flattened leaves"
                )));
            };
            let Some(fields) = flattened_value_leaves(env, arg, &leaf_types) else {
                return Err(MumeiError::type_error(format!(
                    "enum constructor payload '{field_type}' expects a '{base}' struct value"
                )));
            };
            leaves.extend(fields);
        } else {
            leaves.push(arg.clone());
        }
    }
    Ok(leaves)
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
        sorts.extend(
            field_leaf_sorts(vc, &variant.name, i, &ft.name)?
                .into_iter()
                .map(|(_, s)| s),
        );
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
/// when the name does not resolve to a finite-ADT variant or the arity is
/// wrong — callers then keep the previous behaviour (unknown/atom call).
pub(crate) fn enum_ctor_apply<'a>(
    vc: &VCtx<'a>,
    env: &crate::verification::types::Env<'a>,
    ctor_path: &str,
    args: &[Dynamic<'a>],
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
    let leaf_args = expand_ctor_args(vc, env, &enum_def.variants[idx], args)?;
    Ok(enum_ctor_apply_leaves(vc, enum_def, idx, &leaf_args))
}

/// Tester predicate `is-<Variant>(target)` for a `Datatype`-sorted match
/// target. Returns `(sort, tester_bool, variant_idx, enum_def)`, or `None` when no
/// finite ADT owns `variant_name` on `target`'s sort. When several enums
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
/// expression (`h.r` on `h: H`, `h.a.b` through nested structs, `result.r`,
/// `f(x).r`). The chain walks `StructDef.fields` — field names are matched
/// exactly per segment, so `_`-joined const-name ambiguity does not arise.
/// The raw declared name is returned; callers apply `type_name_base` and
/// `get_enum` to use it (a non-enum name is simply ignored downstream).
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
                let sdef = vc.module_env.get_struct(type_name_base(&ty))?;
                ty = sdef
                    .fields
                    .iter()
                    .find(|f| f.name == **field)?
                    .type_name
                    .clone();
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
pub(crate) fn infer_expr_enum_name(vc: &VCtx, expr: &Expr) -> Option<String> {
    match expr {
        Expr::Variable(v) => binding_declared_type(vc, v)
            .map(|t| {
                if vc.module_env.get_enum(&t).is_some() {
                    t
                } else {
                    type_name_base(&t).to_string()
                }
            })
            .filter(|t| vc.module_env.get_enum(t).is_some()),
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
                    .map(|t| {
                        // Keep `Wrap<i64>` whole when that instance is
                        // registered; strip to the base otherwise.
                        if vc.module_env.get_enum(t).is_some() {
                            t.to_string()
                        } else {
                            type_name_base(t).to_string()
                        }
                    })
                    .filter(|t| vc.module_env.get_enum(t).is_some())
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
            // struct-typed binding; `match s` then resolves like `match h.r`.
            declared_type_of_expr(vc, expr)
                .map(|t| type_name_base(&t).to_string())
                .filter(|t| vc.module_env.get_enum(t).is_some())
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
/// when the type is a finite ADT. All scalar/array behaviour is delegated to
/// `param_z3_value` unchanged.
pub(crate) fn param_z3_value_for_vc<'a>(
    vc: &VCtx<'a>,
    name: &str,
    type_name: Option<&str>,
) -> Dynamic<'a> {
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
