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
            if enum_def.name != qe.name {
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
/// `E::V` → `V`; bare `V` → `V`. `EnumVariant.name` stores the leaf segment
/// only, so qualified pattern names (`Mine::Cons`) compare by the leaf.
pub(crate) fn variant_leaf(variant_name: &str) -> &str {
    variant_name.rsplit("::").next().unwrap_or(variant_name)
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
                        .and_then(|v| v.fields.get(i))
                    {
                        let resolved = if *ft == enum_name {
                            enum_name.to_string()
                        } else {
                            vc.module_env.resolve_base_type(ft)
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
        let Some(param) = atom.params.iter().find(|p| p.name == binding) else {
            continue;
        };
        let Some(struct_name) = param.type_name.as_deref().map(type_name_base) else {
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
            let declared = decl_hint
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
                );
            for decl in declared {
                if let Some(decl_enum) = vc.module_env.get_enum(&decl) {
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
        Expr::Variable(v) => vc
            .local_enum_types
            .borrow()
            .get(v.as_str())
            .cloned()
            .or_else(|| {
                vc.current_atom
                    .and_then(|atom| {
                        atom.params
                            .iter()
                            .find(|p| p.name == *v)
                            .and_then(|p| p.type_name.as_deref())
                    })
                    .map(|t| type_name_base(t).to_string())
                    .filter(|t| vc.module_env.get_enum(t).is_some())
            }),
        Expr::Call(name, _) => {
            if let Some((enum_name, variant)) = name.split_once("::") {
                vc.module_env
                    .get_enum(enum_name)
                    .filter(|e| e.variants.iter().any(|v| v.name == variant))
                    .map(|e| e.name.clone())
            } else {
                vc.module_env
                    .get_atom(name)
                    .and_then(|a| a.return_type.as_deref())
                    .map(|t| type_name_base(t).to_string())
                    .filter(|t| vc.module_env.get_enum(t).is_some())
            }
        }
        Expr::FieldAccess(inner, field) => match inner.as_ref() {
            // `E.V` unit constructor or `value.field` — only the qualified
            // unit-constructor form names an enum.
            Expr::Variable(base) => vc
                .module_env
                .get_enum(base)
                .filter(|e| e.variants.iter().any(|v| v.name == *field))
                .map(|e| e.name.clone()),
            _ => None,
        },
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
    if let Some((qual, leaf)) = variant_name.rsplit_once("::") {
        if let Some(qe) = module_env.get_enum(qual) {
            return qe.variants.iter().any(|v| v.name == leaf).then_some(qe);
        }
        return resolve_variant_owner_deterministic(module_env, leaf);
    }
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
