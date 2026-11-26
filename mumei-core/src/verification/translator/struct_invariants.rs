//! Cross-field struct invariants (`invariant: <expr>` in a struct body).
//!
//! A struct value has no Z3 sort of its own: it is a *handle* — an
//! uninterpreted `Int` constant — plus one flattened symbol per field, stored
//! in the env under `__struct_<binding>_<field>` (and `<binding>_<field>`).
//! The same encoding serves parameters, `StructInit` results, `let` aliases
//! and `result`, so an invariant is always lowered against plain scalar
//! symbols: it stays inside whatever fragment the field types already use
//! (QF_LIA for `i64` fields), never introducing a quantifier or a datatype.
//!
//! * struct parameter  → each invariant is *assumed* (tracked assertion)
//! * `StructInit`      → each invariant is *checked* on the new field values
//! * struct `result`   → each invariant is *checked* as an implicit `ensures`
#![allow(unused_imports)]
use super::super::support::*;
use super::super::*;
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(crate) const STRUCT_FIELD_PREFIX: &str = "__struct_";
const STRUCT_HANDLE_PREFIX: &str = "__mumei_struct_";

/// Env key of field `field` of the struct-valued binding `binding`.
pub(crate) fn struct_field_key(binding: &str, field: &str) -> String {
    format!("{STRUCT_FIELD_PREFIX}{binding}_{field}")
}

/// A fresh handle for a struct value produced by `StructInit`.
pub(crate) fn fresh_struct_handle<'a>(ctx: &'a Context, type_name: &str) -> Dynamic<'a> {
    static STRUCT_COUNTER: AtomicUsize = AtomicUsize::new(0);
    let id = STRUCT_COUNTER.fetch_add(1, Ordering::Relaxed);
    Int::new_const(
        ctx,
        format!("{STRUCT_HANDLE_PREFIX}{type_name}_{id}").as_str(),
    )
    .into()
}

/// Register `values` as the fields of `binding`.
pub(crate) fn bind_struct_fields<'a>(
    env: &mut Env<'a>,
    binding: &str,
    values: &[(String, Dynamic<'a>)],
) {
    for (field, value) in values {
        env.insert(struct_field_key(binding, field), value.clone());
        env.insert(format!("{binding}_{field}"), value.clone());
    }
}

/// Declare one fresh symbol per field of `sdef` for `binding` (sorts follow
/// `param_z3_value_for_vc`, i.e. the encoding of the run — an enum-typed
/// field seeds as the enum's Z3 datatype like an enum parameter does) and
/// register them. A struct-typed field seeds recursively — its leaves land
/// under `__struct_<binding>_<field>_<leaf>`, so a nested read
/// `<binding>.<field>.<leaf>` projects through the same keys a literal
/// init writes and two aliases of one inner value compare equal.
/// Depth is capped like the fragment's admission bound so a recursive
/// declaration degenerates to an unseeded const instead of looping.
pub(crate) fn seed_struct_fields<'a>(
    vc: &VCtx<'a>,
    env: &mut Env<'a>,
    binding: &str,
    sdef: &StructDef,
) -> Vec<(String, Dynamic<'a>)> {
    seed_struct_fields_depth(vc, env, binding, sdef, 0)
}

fn seed_struct_fields_depth<'a>(
    vc: &VCtx<'a>,
    env: &mut Env<'a>,
    binding: &str,
    sdef: &StructDef,
    depth: usize,
) -> Vec<(String, Dynamic<'a>)> {
    const MAX_NESTED_SEED_DEPTH: usize = 16;
    let values: Vec<(String, Dynamic<'a>)> = sdef
        .fields
        .iter()
        .map(|field| {
            let name = format!("{binding}_{}", field.name);
            let value = crate::verification::support::datatype::param_z3_value_for_vc(
                vc,
                &name,
                Some(&field.type_name),
            );
            if depth < MAX_NESTED_SEED_DEPTH {
                if let Some(inner) = vc.module_env.get_struct(&field.type_name) {
                    // Seed the inner leaves under this field's own
                    // binding name: `__struct_<binding>_<field>_<leaf>`.
                    seed_struct_fields_depth(vc, env, &name, inner, depth + 1);
                }
            }
            (field.name.clone(), value)
        })
        .collect();
    bind_struct_fields(env, binding, &values);
    values
}

/// Project the fields of a struct-valued Z3 term, in declaration order.
///
/// The term is either the handle of a binding whose fields are in the env
/// (a parameter, a `StructInit`, a `let` alias, a call result) or an `ite`
/// over such handles (`if c { a } else { b }`), which projects field-wise
/// into `ite(c, a.f, b.f)`.
pub(crate) fn struct_fields_of_value<'a>(
    env: &Env<'a>,
    value: &Dynamic<'a>,
    sdef: &StructDef,
) -> Option<Vec<(String, Dynamic<'a>)>> {
    let projected = project_struct_fields(env, value)?;
    sdef.fields
        .iter()
        .map(|field| {
            projected
                .iter()
                .find(|(name, _)| *name == field.name)
                .cloned()
        })
        .collect()
}

/// Field values of a struct-valued term without knowing its struct type:
/// the `__struct_<handle>_<field>` entries of a handle, or their field-wise
/// `ite` for a conditional over handles. Nested field reads use it to
/// resolve `o.inner.a` once `o.inner` evaluates to an inner handle.
pub(crate) fn project_struct_fields<'a>(
    env: &Env<'a>,
    value: &Dynamic<'a>,
) -> Option<Vec<(String, Dynamic<'a>)>> {
    if value.kind() != z3::AstKind::App {
        return None;
    }
    let decl = value.decl();
    if decl.kind() == z3::DeclKind::ITE {
        let children = value.children();
        let cond = children.first()?.as_bool()?;
        let then_fields = project_struct_fields(env, children.get(1)?)?;
        let else_fields = project_struct_fields(env, children.get(2)?)?;
        return then_fields
            .into_iter()
            .map(|(name, t)| {
                let (_, e) = else_fields.iter().find(|(other, _)| *other == name)?;
                let (t, e) = unify_branch_sorts(t, e.clone()).ok()?;
                Some((name, cond.ite(&t, &e)))
            })
            .collect();
    }
    if decl.kind() != z3::DeclKind::UNINTERPRETED || decl.arity() != 0 {
        return None;
    }
    let prefix = format!("{STRUCT_FIELD_PREFIX}{}_", decl.name());
    let mut fields: Vec<(String, Dynamic<'a>)> = env
        .iter()
        .filter_map(|(key, val)| {
            key.strip_prefix(prefix.as_str())
                .map(|field| (field.to_string(), val.clone()))
        })
        .collect();
    if fields.is_empty() {
        return None;
    }
    fields.sort_by(|(a, _), (b, _)| a.cmp(b));
    Some(fields)
}

/// The full flat field list of `value`: its direct fields plus every
/// deeper leaf composed under `<field>_<leaf>` (`b` and `b_c` for
/// `Inner { b: Sub { c } }`), so re-homing the binding covers
/// `q.inner.b.c`-style paths and a shallow bind never leaves deep keys
/// on their pre-write symbols.
pub(crate) fn nested_field_bindings<'a>(
    env: &Env<'a>,
    value: &Dynamic<'a>,
) -> Vec<(String, Dynamic<'a>)> {
    fn collect<'a>(
        env: &Env<'a>,
        value: &Dynamic<'a>,
        visiting: &mut std::collections::HashSet<usize>,
    ) -> Vec<(String, Dynamic<'a>)> {
        let ast = value.get_z3_ast() as usize;
        if !visiting.insert(ast) {
            return Vec::new();
        }
        let Some(fields) = project_struct_fields(env, value) else {
            visiting.remove(&ast);
            return Vec::new();
        };
        let mut out = Vec::with_capacity(fields.len());
        for (name, inner) in fields {
            for (deep, v) in collect(env, &inner, visiting) {
                out.push((format!("{name}_{deep}"), v));
            }
            out.push((name, inner));
        }
        visiting.remove(&ast);
        out
    }

    collect(env, value, &mut std::collections::HashSet::new())
}

/// Register the fields of the struct value `value` under the new binding
/// `alias` (`let alias = value;` / `alias = value;`), so `alias.f` resolves.
pub(crate) fn alias_struct_fields<'a>(env: &mut Env<'a>, alias: &str, value: &Dynamic<'a>) {
    if value.kind() == z3::AstKind::App
        && value.decl().kind() == z3::DeclKind::UNINTERPRETED
        && value.decl().name() == alias
    {
        return;
    }
    let fields = project_struct_fields(env, value);
    let stale_prefix = format!("{STRUCT_FIELD_PREFIX}{alias}_");
    let stale: Vec<String> = env
        .keys()
        .filter_map(|key| key.strip_prefix(stale_prefix.as_str()).map(str::to_string))
        .collect();
    for field in stale {
        env.remove(&struct_field_key(alias, &field));
        env.remove(&format!("{alias}_{field}"));
    }
    if let Some(fields) = fields {
        bind_struct_fields(env, alias, &fields);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use z3::ast::Int;
    use z3::{Config, Context};

    #[test]
    fn nested_field_bindings_stop_at_cyclic_handles() {
        let context = Context::new(&Config::new());
        let root: Dynamic<'_> = Int::new_const(&context, "root").into();
        let next: Dynamic<'_> = Int::new_const(&context, "next").into();
        let mut env: Env<'_> = std::collections::HashMap::new();
        bind_struct_fields(&mut env, "root", &[("next".to_string(), next.clone())]);
        bind_struct_fields(&mut env, "next", &[("self".to_string(), next)]);

        let fields = nested_field_bindings(&env, &root);
        assert_eq!(
            fields
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            vec!["next_self", "next"]
        );
    }
}

/// Lower a struct invariant against the given field values.
///
/// Inside the expression `self.<field>` (and a bare `<field>`) denotes the
/// field value; every other name resolves through `env`. Calls inside the
/// invariant are lowered against `solver_opt` so their contracts are usable.
pub(crate) fn lower_struct_invariant<'a>(
    vc: &VCtx<'a>,
    sdef: &StructDef,
    invariant_raw: &str,
    fields: &[(String, Dynamic<'a>)],
    env: &Env<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> MumeiResult<Bool<'a>> {
    let mut local_env = env.clone();
    bind_struct_fields(&mut local_env, "self", fields);
    for (field, value) in fields {
        local_env.insert(field.clone(), value.clone());
    }
    let ast = parse_expression(invariant_raw);
    let lowered = expr_to_z3(vc, &ast, &mut local_env, solver_opt)?;
    lowered.as_bool().ok_or_else(|| {
        MumeiError::type_error(format!(
            "Struct '{}' invariant must be a boolean expression: {}",
            sdef.name, invariant_raw
        ))
    })
}

/// The predicates a field's declared refined-alias type imposes on its
/// value, named by declaration: the alias's whole chain, each declaration
/// binding its own operand to the field value — exactly what
/// `apply_refinement_constraint` asserts for a scalar parameter of that
/// type. Empty for a field of any other type; a cyclic chain is a type
/// error.
pub(crate) fn field_type_predicates<'a>(
    vc: &VCtx<'a>,
    field: &crate::parser::ast::StructField,
    field_z3: &Dynamic<'a>,
    env: &Env<'a>,
) -> MumeiResult<Vec<(String, Bool<'a>)>> {
    if !vc.module_env.types.contains_key(&field.type_name) {
        return Ok(Vec::new());
    }
    let chain = vc
        .module_env
        .refined_chain(&field.type_name)
        .map_err(MumeiError::type_error)?;
    let mut predicates = Vec::with_capacity(chain.len());
    for decl in chain {
        let mut local_env = env.clone();
        local_env.insert(decl.operand.clone(), field_z3.clone());
        let ast = parse_expression(&decl.predicate_raw);
        let predicate = refined_predicate_to_z3(vc, decl, &ast, &mut local_env, None)?
            .as_bool()
            .ok_or_else(|| {
                MumeiError::type_error(format!("Predicate for {} must be boolean", decl.name))
            })?;
        predicates.push((decl.name.clone(), predicate));
    }
    Ok(predicates)
}

/// Lower the complete struct contract against `fields`.
///
/// Field constraints use `v` as their field value and invariants use the same
/// `self.<field>`/bare-field bindings as `assume_struct_invariants`. Keeping
/// both forms in one helper ensures call-site domain facts cannot drift from
/// the assumptions made for a struct parameter's body.
pub(crate) fn lower_struct_contract<'a, F>(
    vc: &VCtx<'a>,
    sdef: &StructDef,
    fields: &[(String, Dynamic<'a>)],
    env: &Env<'a>,
    solver_opt: Option<&Solver<'a>>,
    mut on_fact: F,
) -> MumeiResult<()>
where
    F: FnMut(String, Bool<'a>) -> MumeiResult<()>,
{
    for (field, (_, field_z3)) in sdef.fields.iter().zip(fields) {
        for (decl, predicate) in field_type_predicates(vc, field, field_z3, env)? {
            on_fact(format!("field::{}::type::{}", field.name, decl), predicate)?;
        }
        let Some(constraint_raw) = &field.constraint else {
            continue;
        };
        let mut local_env = env.clone();
        local_env.insert("v".to_string(), field_z3.clone());
        let ast = parse_expression(constraint_raw);
        let constraint = expr_to_z3(vc, &ast, &mut local_env, None)?;
        if let Some(constraint) = constraint.as_bool() {
            on_fact(format!("field::{}", field.name), constraint)?;
        }
    }
    for (index, invariant_raw) in sdef.invariants.iter().enumerate() {
        let invariant = lower_struct_invariant(vc, sdef, invariant_raw, fields, env, solver_opt)?;
        on_fact(format!("invariant::{index}"), invariant)?;
    }
    Ok(())
}

/// Assume the whole struct contract of `sdef` for the field values of
/// `binding`: the per-field `where v ...` refinements, then every invariant.
pub(crate) fn assume_struct_contract<'a>(
    vc: &VCtx<'a>,
    solver: &Solver<'a>,
    sdef: &StructDef,
    binding: &str,
    fields: &[(String, Dynamic<'a>)],
    env: &Env<'a>,
    span: Option<String>,
) -> MumeiResult<()> {
    lower_struct_contract(vc, sdef, fields, env, Some(solver), |kind, fact| {
        let track_label = match kind.strip_prefix("field::") {
            Some(field) => format!("track_struct_field_{}::{}", binding, field),
            None => {
                let index = kind.strip_prefix("invariant::").unwrap_or("unknown");
                format!("track_struct_invariant_{}::{}", binding, index)
            }
        };
        let track_bool = Bool::new_const(vc.ctx, track_label.as_str());
        solver.assert_and_track(&fact, &track_bool);
        profile_solver_assertion(vc, &track_label, span.clone());
        Ok(())
    })
}

/// Assume every invariant of `sdef` for the field values of `binding`.
pub(crate) fn assume_struct_invariants<'a>(
    vc: &VCtx<'a>,
    solver: &Solver<'a>,
    sdef: &StructDef,
    binding: &str,
    fields: &[(String, Dynamic<'a>)],
    env: &Env<'a>,
    span: Option<String>,
) -> MumeiResult<()> {
    for (index, invariant_raw) in sdef.invariants.iter().enumerate() {
        let invariant = lower_struct_invariant(vc, sdef, invariant_raw, fields, env, Some(solver))?;
        let track_label = format!("track_struct_invariant_{}::{}", binding, index);
        let track_bool = Bool::new_const(vc.ctx, track_label.as_str());
        solver.assert_and_track(&invariant, &track_bool);
        profile_solver_assertion(vc, &track_label, span.clone());
    }
    Ok(())
}

/// Check every invariant of `sdef` against the field values of `subject`
/// under the current solver context; the first refutable one is an error.
pub(crate) fn check_struct_invariants<'a>(
    vc: &VCtx<'a>,
    solver: &Solver<'a>,
    sdef: &StructDef,
    subject: &str,
    fields: &[(String, Dynamic<'a>)],
    env: &Env<'a>,
) -> MumeiResult<()> {
    for invariant_raw in &sdef.invariants {
        let invariant = lower_struct_invariant(vc, sdef, invariant_raw, fields, env, Some(solver))?;
        solver.push();
        solver.assert(&vc.path_cond_conj());
        solver.assert(&invariant.not());
        let checkpoint = profiler_checkpoint(vc);
        let verdict = crate::verification::phase_artifacts::check(solver);
        profile_solver_check(vc, checkpoint);
        solver.pop(1);
        match verdict {
            SatResult::Unsat => {}
            SatResult::Sat => {
                return Err(MumeiError::verification(format!(
                    "Struct '{}' invariant violated for {}: {}",
                    sdef.name, subject, invariant_raw
                ))
                .with_verdict(crate::verification::VerdictKind::Rejected));
            }
            SatResult::Unknown => {
                return Err(MumeiError::verification(format!(
                    "Struct '{}' invariant could not be decided for {}: {}",
                    sdef.name, subject, invariant_raw
                ))
                .with_verdict(crate::verification::VerdictKind::Inconclusive));
            }
        }
    }
    Ok(())
}
