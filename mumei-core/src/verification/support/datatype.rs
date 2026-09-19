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
use crate::parser::ast::EnumDef;
use crate::verification::module_env::ModuleEnv;
use crate::verification::translator::{VCtx, F64_EBITS, F64_SBITS};
use crate::verification::types::{MumeiError, MumeiResult};

/// `true` when `enum_def` is finite + non-recursive + scalar-payload only —
/// the fragment that maps onto a Z3 `DatatypeSort`.
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
            .fields
            .iter()
            .all(|field| field_is_scalar(field, module_env))
    })
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

fn field_sort<'a>(
    ctx: &'a Context,
    field_type: &str,
    module_env: &ModuleEnv,
    ieee754_f64: bool,
) -> Option<Sort<'a>> {
    if !field_is_scalar(field_type, module_env) {
        return None;
    }
    let base = module_env.resolve_base_type(field_type);
    Some(match lower(&base) {
        // Mirrors `param_z3_value`: `f64` is IEEE 754 Float under the
        // `--ieee754-f64` opt-in, exact `Real` otherwise.
        LoweredType::F64 if ieee754_f64 => Sort::float(ctx, F64_EBITS, F64_SBITS),
        LoweredType::F64 => Sort::real(ctx),
        LoweredType::Str => Sort::string(ctx),
        LoweredType::Bool => Sort::bool(ctx),
        _ => Sort::int(ctx),
    })
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
        for (i, field_type) in variant.fields.iter().enumerate() {
            names.push(format!("{}_{}", variant.name, i));
            sorts.push(field_sort(
                vc.ctx,
                field_type,
                vc.module_env,
                vc.ieee754_f64,
            )?);
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

/// `Enum::Variant(args)` / `Variant(args)` construction — applies the
/// variant's Z3 constructor to `args` (already lowered). Returns `None` when
/// the name does not resolve to a finite-ADT variant or the arity is wrong —
/// callers then keep the previous behaviour (unknown/atom call).
pub(crate) fn enum_ctor_apply<'a>(
    vc: &VCtx<'a>,
    ctor_path: &str,
    args: &[Dynamic<'a>],
) -> MumeiResult<Option<Dynamic<'a>>> {
    let variant_name = ctor_path.rsplit("::").next().unwrap_or(ctor_path);
    let enum_def = if let Some((qual, _)) = ctor_path.split_once("::") {
        // Qualified `E::V` resolves `E` directly — `find_enum_by_variant`
        // would non-deterministically pick a *different* enum declaring a
        // same-named variant (`enums` is a HashMap).
        vc.module_env.get_enum(qual)
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
    let Some(sort) = enum_datatype_sort(vc, enum_def) else {
        return Ok(None);
    };
    if enum_def.variants[idx].fields.len() != args.len() {
        return Ok(None);
    }
    let arg_refs: Vec<&dyn Ast> = args.iter().map(|d| d as &dyn Ast).collect();
    Ok(Some(sort.variants[idx].constructor.apply(&arg_refs)))
}

/// Tester predicate `is-<Variant>(target)` for a `Datatype`-sorted match
/// target. Returns `(sort, tester_bool, variant_idx)`, or `None` when no
/// finite ADT owns `variant_name` on `target`'s sort. When several enums
/// share the variant name, the one whose datatype sort matches `target`'s
/// wins — the name-only `find_enum_by_variant` scan would pick the wrong
/// enum for colliding names.
pub(crate) fn variant_tester_condition<'a>(
    vc: &VCtx<'a>,
    target: &Dynamic<'a>,
    variant_name: &str,
) -> Option<(Rc<DatatypeSort<'a>>, Bool<'a>, usize)> {
    let dt = target.as_datatype()?;
    let target_sort = dt.get_sort();
    for enum_def in vc.module_env.enums.values() {
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
        return Some((sort, is_v, idx));
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
pub(crate) fn variant_owners<'m>(
    module_env: &'m ModuleEnv,
    variant_name: &str,
) -> Vec<&'m EnumDef> {
    let mut owners: Vec<_> = module_env
        .enums
        .values()
        .filter(|e| e.variants.iter().any(|v| v.name == variant_name))
        .collect();
    owners.sort_by(|a, b| a.name.cmp(&b.name));
    owners
}

/// The enum a `match` target was declared as, when the target is a bare
/// parameter constant — `match l` on `l: IntList` yields `Some("IntList")`.
fn target_param_enum_name(vc: &VCtx, target: &Dynamic) -> Option<String> {
    let sym = target.as_int()?.decl().name();
    let atom = vc.current_atom?;
    atom.params
        .iter()
        .find(|p| p.name == sym)
        .and_then(|p| p.type_name.as_deref())
        .map(|ty| {
            // Strip generic arguments (`Option<i64>` → `Option`) and the
            // array wrapper (`[Color]` → `Color`).
            let base = ty
                .strip_prefix('[')
                .and_then(|s| s.strip_suffix(']'))
                .unwrap_or(ty);
            base.split('<').next().unwrap_or(base).trim().to_string()
        })
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
        .position(|v| v.name == variant_name)
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
/// 1. the enum named by the match target's declared parameter type;
/// 2. the sole owner, or any owner when every owner assigns the variant
///    the same tag index and payload types;
/// 3. otherwise the match cannot be encoded soundly — an error.
pub(crate) fn resolve_variant_owner<'a>(
    vc: &VCtx<'a>,
    target: &Dynamic<'a>,
    variant_name: &str,
) -> MumeiResult<Option<&'a EnumDef>> {
    let owners = variant_owners(vc.module_env, variant_name);
    if owners.is_empty() {
        return Ok(None);
    }
    if let Some(decl) = target_param_enum_name(vc, target) {
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

/// `resolve_variant_owner` minus the declared-parameter-type preference,
/// for callers with no verification context (fragment classification):
/// returns the deterministic owner — sole owner, or any owner when all
/// signatures agree — and `None` when the pick would be arbitrary (the
/// verifier may still resolve it via the match target's declared type, or
/// fail closed).
pub(crate) fn resolve_variant_owner_deterministic<'m>(
    module_env: &'m ModuleEnv,
    variant_name: &str,
) -> Option<&'m EnumDef> {
    let owners = variant_owners(module_env, variant_name);
    if owners.len() == 1 {
        return Some(owners[0]);
    }
    let first = int_tag_sig(module_env, *owners.first()?, variant_name);
    owners
        .iter()
        .all(|e| int_tag_sig(module_env, e, variant_name) == first)
        .then_some(owners[0])
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
