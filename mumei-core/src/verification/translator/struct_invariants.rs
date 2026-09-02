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
use crate::parser::expr::normalize_comparison_chains;
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
/// `param_z3_value`, i.e. the encoding of the run) and register them.
pub(crate) fn seed_struct_fields<'a>(
    ctx: &'a Context,
    env: &mut Env<'a>,
    binding: &str,
    sdef: &StructDef,
    module_env: &ModuleEnv,
    ieee754_f64: bool,
    bitvec_i64: bool,
) -> Vec<(String, Dynamic<'a>)> {
    let values: Vec<(String, Dynamic<'a>)> = sdef
        .fields
        .iter()
        .map(|field| {
            let name = format!("{binding}_{}", field.name);
            let value = param_z3_value(
                ctx,
                &name,
                Some(&field.type_name),
                module_env,
                ieee754_f64,
                bitvec_i64,
            );
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
/// `ite` for a conditional over handles.
fn project_struct_fields<'a>(
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
    let ast = normalize_comparison_chains(parse_expression(invariant_raw));
    let lowered = expr_to_z3(vc, &ast, &mut local_env, solver_opt)?;
    lowered.as_bool().ok_or_else(|| {
        MumeiError::type_error(format!(
            "Struct '{}' invariant must be a boolean expression: {}",
            sdef.name, invariant_raw
        ))
    })
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
    for (field, (_, field_z3)) in sdef.fields.iter().zip(fields) {
        let Some(constraint_raw) = &field.constraint else {
            continue;
        };
        let mut local_env = env.clone();
        local_env.insert("v".to_string(), field_z3.clone());
        let ast = normalize_comparison_chains(parse_expression(constraint_raw));
        let constraint = expr_to_z3(vc, &ast, &mut local_env, None)?;
        if let Some(constraint) = constraint.as_bool() {
            let track_label = format!("track_struct_field_{}::{}", binding, field.name);
            let track_bool = Bool::new_const(vc.ctx, track_label.as_str());
            solver.assert_and_track(&constraint, &track_bool);
            profile_solver_assertion(vc, &track_label, span.clone());
        }
    }
    assume_struct_invariants(vc, solver, sdef, binding, fields, env, span)
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
        let verdict = solver.check();
        profile_solver_check(vc, checkpoint);
        solver.pop(1);
        match verdict {
            SatResult::Unsat => {}
            SatResult::Sat => {
                return Err(MumeiError::verification(format!(
                    "Struct '{}' invariant violated for {}: {}",
                    sdef.name, subject, invariant_raw
                )));
            }
            SatResult::Unknown => {
                return Err(MumeiError::verification(format!(
                    "Struct '{}' invariant could not be decided for {}: {}",
                    sdef.name, subject, invariant_raw
                )));
            }
        }
    }
    Ok(())
}
