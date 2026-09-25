//! Replayability verification for non-deterministic effects.
//!
//! `Random`, `Clock` and `ExternalInput` (see `std/effects.mm`) mark atoms whose
//! behaviour depends on a source outside the program. Mumei isolates those
//! sources at the type level: an atom that declares such an effect must
//! receive every non-deterministic value through an explicit *witness*
//! parameter (`seed`, `timestamp`, `input`, ...) and must thread that witness
//! into each `perform` of the effect. Once the witness is an ordinary
//! parameter, verification treats it like any other universally-quantified
//! input, and re-running the atom with the same arguments replays the same
//! trace (the perform result is modelled as an uninterpreted function of its
//! arguments, see `nondeterministic_perform_result`).
//!
//! Pure atoms (no `effects:` clause) never reach this check: performing or
//! calling a non-deterministic source from a pure atom is already rejected by
//! effect containment.

use super::super::module_env::ModuleEnv;
use super::super::nlae_reporter::FAILURE_EFFECT_NOT_ALLOWED;
use super::super::types::{MumeiError, MumeiResult};
use super::call_graph::expr_mentions_var;
use crate::parser::{Atom, Expr, Stmt};
use serde_json::json;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// Built-in non-deterministic effects and the parameter names accepted as the
/// explicit witness for each. A parameter matches when its name equals a
/// witness name or starts with `<witness>_` (e.g. `seed_a`, `timestamp_ms`).
const NONDETERMINISTIC_EFFECTS: &[(&str, &[&str])] = &[
    ("Random", &["seed"]),
    ("Clock", &["timestamp", "now", "clock"]),
    ("ExternalInput", &["input", "external_input"]),
];

/// Witness parameter names for `effect_name`, or `None` if the effect is not a
/// built-in non-deterministic source.
pub(crate) fn nondeterministic_witnesses(effect_name: &str) -> Option<&'static [&'static str]> {
    NONDETERMINISTIC_EFFECTS
        .iter()
        .find(|(name, _)| *name == effect_name)
        .map(|(_, witnesses)| *witnesses)
}

/// Resolve `effect_name` (possibly a composite or a child via `parent:`) to the
/// built-in non-deterministic leaf it stands for, if any.
pub(crate) fn nondeterministic_root(
    module_env: &ModuleEnv,
    effect_name: &str,
) -> Option<&'static str> {
    NONDETERMINISTIC_EFFECTS
        .iter()
        .map(|(name, _)| *name)
        .find(|root| effect_name == *root || module_env.is_subeffect(effect_name, root))
}

fn param_is_witness(param_name: &str, witnesses: &[&str]) -> bool {
    witnesses.iter().any(|w| {
        param_name == *w
            || param_name
                .strip_prefix(w)
                .is_some_and(|rest| rest.starts_with('_'))
    })
}

/// Non-deterministic leaf effects declared (positively) by `atom`, in a
/// deterministic order.
pub(crate) fn declared_nondeterministic_effects(
    atom: &Atom,
    module_env: &ModuleEnv,
) -> Vec<&'static str> {
    let positive: Vec<crate::parser::Effect> = atom
        .effects
        .iter()
        .filter(|e| !e.negated)
        .cloned()
        .collect();
    let leaves = module_env.resolve_leaf_effects_from_effects(&positive);
    let mut roots = BTreeSet::new();
    for leaf in &leaves {
        if let Some(root) = nondeterministic_root(module_env, leaf) {
            roots.insert(root);
        }
    }
    roots.into_iter().collect()
}

/// Names of `atom`'s parameters that act as witnesses for `effect`.
pub(crate) fn witness_params<'a>(atom: &'a Atom, effect: &str) -> Vec<&'a str> {
    let Some(witnesses) = nondeterministic_witnesses(effect) else {
        return Vec::new();
    };
    atom.params
        .iter()
        .filter(|p| param_is_witness(&p.name, witnesses))
        .map(|p| p.name.as_str())
        .collect()
}

fn collect_performs_expr<'a>(expr: &'a Expr, out: &mut Vec<(&'a str, &'a str, &'a [Expr])>) {
    match expr {
        Expr::Perform {
            effect,
            operation,
            args,
        } => {
            out.push((effect.as_str(), operation.as_str(), args.as_slice()));
            for arg in args {
                collect_performs_expr(arg, out);
            }
        }
        Expr::Call(_, args) => {
            for arg in args {
                collect_performs_expr(arg, out);
            }
        }
        Expr::CallRef { callee, args } => {
            collect_performs_expr(callee, out);
            for arg in args {
                collect_performs_expr(arg, out);
            }
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_performs_expr(cond, out);
            collect_performs_stmt(then_branch, out);
            collect_performs_stmt(else_branch, out);
        }
        Expr::BinaryOp(l, _, r) => {
            collect_performs_expr(l, out);
            collect_performs_expr(r, out);
        }
        Expr::Async { body } | Expr::Lambda { body, .. } => collect_performs_stmt(body, out),
        Expr::Await { expr } => collect_performs_expr(expr, out),
        Expr::Match { target, arms } => {
            collect_performs_expr(target, out);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_performs_expr(guard, out);
                }
                collect_performs_stmt(&arm.body, out);
            }
        }
        Expr::ChanSend { channel, value } => {
            collect_performs_expr(channel, out);
            collect_performs_expr(value, out);
        }
        Expr::ChanRecv { channel } => collect_performs_expr(channel, out),
        _ => {}
    }
}

fn collect_performs_stmt<'a>(stmt: &'a Stmt, out: &mut Vec<(&'a str, &'a str, &'a [Expr])>) {
    match stmt {
        Stmt::Block(stmts, _) => {
            for s in stmts {
                collect_performs_stmt(s, out);
            }
        }
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => collect_performs_expr(value, out),
        Stmt::ArrayStore { index, value, .. } => {
            collect_performs_expr(index, out);
            collect_performs_expr(value, out);
        }
        Stmt::While { cond, body, .. } => {
            collect_performs_expr(cond, out);
            collect_performs_stmt(body, out);
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => collect_performs_stmt(body, out),
        Stmt::TaskGroup { children, .. } => {
            for child in children {
                collect_performs_stmt(child, out);
            }
        }
        Stmt::Expr(e, _) => collect_performs_expr(e, out),
        Stmt::Cancel { .. } => {}
    }
}

/// Structured description of a replayability violation, saved to `report.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayViolation {
    pub atom: String,
    pub effect: &'static str,
    pub accepted_witnesses: Vec<&'static str>,
    /// `None` when the signature lacks a witness parameter; `Some(op)` when the
    /// `perform <effect>.<op>` call site does not thread the witness through.
    pub perform_operation: Option<String>,
}

impl ReplayViolation {
    fn suggested_signature(&self) -> String {
        format!("{}: i64", self.accepted_witnesses[0])
    }
}

/// Verify that every non-deterministic effect declared by `atom` is fed through
/// an explicit witness parameter (signature) and that each `perform` of the
/// effect mentions that witness (body).
pub(crate) fn check_replayability(
    atom: &Atom,
    body_stmt: &Stmt,
    module_env: &ModuleEnv,
) -> Result<(), ReplayViolation> {
    let roots = declared_nondeterministic_effects(atom, module_env);
    if roots.is_empty() {
        return Ok(());
    }

    let mut performs = Vec::new();
    collect_performs_stmt(body_stmt, &mut performs);

    for root in roots {
        let accepted = nondeterministic_witnesses(root).unwrap_or(&[]).to_vec();
        let witnesses = witness_params(atom, root);
        if witnesses.is_empty() {
            return Err(ReplayViolation {
                atom: atom.name.clone(),
                effect: root,
                accepted_witnesses: accepted,
                perform_operation: None,
            });
        }
        for (effect, operation, args) in &performs {
            if nondeterministic_root(module_env, effect) != Some(root) {
                continue;
            }
            let threaded = args
                .iter()
                .any(|arg| witnesses.iter().any(|w| expr_mentions_var(arg, w)));
            if !threaded {
                return Err(ReplayViolation {
                    atom: atom.name.clone(),
                    effect: root,
                    accepted_witnesses: accepted,
                    perform_operation: Some((*operation).to_string()),
                });
            }
        }
    }
    Ok(())
}

/// Run `check_replayability`, writing `report.json` and returning a
/// `MumeiError` on violation.
pub(crate) fn verify_replayability(
    atom: &Atom,
    body_stmt: &Stmt,
    module_env: &ModuleEnv,
    output_dir: &Path,
) -> MumeiResult<()> {
    check_replayability(atom, body_stmt, module_env).map_err(|violation| {
        save_replayability_report(output_dir, &violation);
        let accepted = violation.accepted_witnesses.join(" | ");
        match &violation.perform_operation {
            None => MumeiError::verification_at(
                format!(
                    "Replayability violation: atom '{}' declares non-deterministic effect '{}' \
                     but has no explicit witness parameter ({}). Non-deterministic sources must \
                     be received as parameters so the same inputs replay the same output.",
                    violation.atom, violation.effect, accepted
                ),
                atom.span.clone(),
            )
            .with_help(format!(
                "Add a parameter `{}` to atom '{}' and pass it to every `perform {}.<op>(...)`.",
                violation.suggested_signature(),
                violation.atom,
                violation.effect
            )),
            Some(op) => MumeiError::verification_at(
                format!(
                    "Replayability violation: atom '{}' performs '{}.{}' without threading its \
                     witness parameter ({}) through the call. The non-deterministic source must \
                     be derived from the explicit parameter.",
                    violation.atom, violation.effect, op, accepted
                ),
                atom.span.clone(),
            )
            .with_help(format!(
                "Pass the witness parameter as an argument: `perform {}.{}({})`.",
                violation.effect,
                op,
                witness_params(atom, violation.effect)
                    .first()
                    .copied()
                    .unwrap_or(violation.accepted_witnesses[0])
            )),
        }
    })
}

/// Save a replayability violation report to `report.json` for self-healing integration.
fn save_replayability_report(output_dir: &Path, violation: &ReplayViolation) {
    let witness = violation.suggested_signature();
    let (reason, fix) = match &violation.perform_operation {
        None => (
            format!(
                "Replayability violation: atom '{}' declares non-deterministic effect '{}' \
                 without a witness parameter",
                violation.atom, violation.effect
            ),
            format!("Add parameter `{}` to atom '{}'", witness, violation.atom),
        ),
        Some(op) => (
            format!(
                "Replayability violation: atom '{}' performs '{}.{}' without its witness parameter",
                violation.atom, violation.effect, op
            ),
            format!(
                "Pass the witness parameter to `perform {}.{}`",
                violation.effect, op
            ),
        ),
    };
    let mut report = json!({
        "status": "failed",
        "atom": violation.atom,
        "failure_type": FAILURE_EFFECT_NOT_ALLOWED,
        "violation_type": "replayability",
        "effect_violation": {
            "atom": violation.atom,
            "effect": violation.effect,
            "accepted_witnesses": violation.accepted_witnesses,
            "perform_operation": violation.perform_operation,
            "suggested_fixes": [fix],
            "resolution_paths": [
                {
                    "strategy": "witness_parameter",
                    "description": format!(
                        "Receive the '{}' source through an explicit parameter and thread it into every perform",
                        violation.effect
                    ),
                    "fix_type": "signature_change",
                    "target": violation.atom,
                    "change": witness,
                },
                {
                    "strategy": "isolation",
                    "description": format!("Remove '{}' from the effects declaration and drop the perform", violation.effect),
                    "fix_type": "body_change",
                    "target": violation.atom,
                }
            ]
        },
        "reason": reason,
    });
    report["structured_feedback"] = json!(
        crate::structured_feedback::StructuredFeedback::from_report(&report)
    );
    let _ = fs::create_dir_all(output_dir);
    let _ = fs::write(
        output_dir.join("report.json"),
        serde_json::to_string_pretty(&report).unwrap_or_else(|_| report.to_string()),
    );
}
