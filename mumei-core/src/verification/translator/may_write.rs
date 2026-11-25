//! Single source of truth for the caller-side *may-write* set of a call:
//! which caller variables a `callee(args)` / `call(callee, args)` may store
//! through. Both the eval-time havoc (`havoc_array_args`,
//! `apply_local_lambda`) and the loop-invariant modified-set collection
//! (`collect_expr_assigned_vars`) derive their havoc targets from here, so
//! the two can no longer drift apart.
#![allow(unused_imports)]
use super::super::support::*;
use super::super::*;
use super::*;
use crate::verification::translator::z3_types::{
    atom_stores_to_array, is_read_only_array_builtin, stmt_stores_to_var,
};
use std::collections::HashSet;

/// The callee position of a call, as written at the call site.
#[derive(Clone, Copy)]
pub(crate) enum CalleeRef<'e> {
    /// `name(args)` — `Expr::Call`.
    Name(&'e str),
    /// `call(callee, args)` — the `callee` of `Expr::CallRef`.
    Expr(&'e Expr),
}

/// Resolve `callee` the way the evaluator does and return the raw names of
/// the caller variables the call may store through.
///
/// Resolution order (mirrors `expr_to_z3`):
/// * `Name`: a `local_lambdas` hit shadows the atom registry; then the atom
///   under `name` or its FQN spelling (`mod.f` → `mod::f`); an unresolvable
///   callee fails closed (every variable arg — it may bind later inside
///   the body and no body is available to inspect).
/// * `Expr`: `AtomRef{name}` names the atom under `name` only (no FQN
///   respelling on this path); `Variable` is a local lambda first, else an
///   atom only when the variable was bound through `atom_ref`
///   (`__atom_ref_<var>` in `env`); an inline `Lambda` literal
///   is applied as a closure over the current lambda scope; anything else
///   lands on the dynamic path where every variable arg is havoced.
pub(crate) fn callee_may_write_args(
    vc: &VCtx<'_>,
    callee: CalleeRef<'_>,
    args: &[Expr],
    env: &Env<'_>,
) -> HashSet<String> {
    let module_env = vc.module_env;
    match callee {
        CalleeRef::Name(name) => {
            if let Some(lambda) = vc.local_lambdas.borrow().get(name).cloned() {
                return lambda_may_write_args(module_env, &lambda, args, env);
            }
            match resolve_atom(module_env, name) {
                Some(atom) => atom_may_write_args(module_env, atom, args),
                None if is_read_only_array_builtin(name) => HashSet::new(),
                None => all_var_args(args),
            }
        }
        CalleeRef::Expr(Expr::AtomRef { name }) => match module_env.get_atom(name) {
            Some(atom) => atom_may_write_args(module_env, atom, args),
            None => all_var_args(args),
        },
        CalleeRef::Expr(Expr::Variable(var)) => {
            if let Some(lambda) = vc.local_lambdas.borrow().get(var).cloned() {
                return lambda_may_write_args(module_env, &lambda, args, env);
            }
            if env.contains_key(&format!("__atom_ref_{var}")) {
                if let Some(atom) = module_env.get_atom(var) {
                    return atom_may_write_args(module_env, atom, args);
                }
            }
            all_var_args(args)
        }
        CalleeRef::Expr(lambda_expr @ Expr::Lambda { .. }) => {
            let closure = LocalLambda::Closure {
                expr: lambda_expr.clone(),
                captured: vc.local_lambdas.borrow().clone(),
            };
            lambda_may_write_args(module_env, &closure, args, env)
        }
        CalleeRef::Expr(_) => all_var_args(args),
    }
}

pub(crate) fn callee_may_write_arg_indices(
    vc: &VCtx<'_>,
    callee: CalleeRef<'_>,
    args: &[Expr],
    env: &Env<'_>,
) -> HashSet<usize> {
    let module_env = vc.module_env;
    match callee {
        CalleeRef::Name(name) => {
            if let Some(lambda) = vc.local_lambdas.borrow().get(name).cloned() {
                return lambda_may_write_arg_indices(module_env, &lambda, args);
            }
            match resolve_atom(module_env, name) {
                Some(atom) => atom_may_write_arg_indices(module_env, atom, args),
                None if is_read_only_array_builtin(name) => HashSet::new(),
                None => all_arg_indices(args),
            }
        }
        CalleeRef::Expr(Expr::AtomRef { name }) => match module_env.get_atom(name) {
            Some(atom) => atom_may_write_arg_indices(module_env, atom, args),
            None => all_arg_indices(args),
        },
        CalleeRef::Expr(Expr::Variable(var)) => {
            if let Some(lambda) = vc.local_lambdas.borrow().get(var).cloned() {
                return lambda_may_write_arg_indices(module_env, &lambda, args);
            }
            if env.contains_key(&format!("__atom_ref_{var}")) {
                if let Some(atom) = module_env.get_atom(var) {
                    return atom_may_write_arg_indices(module_env, atom, args);
                }
            }
            all_arg_indices(args)
        }
        CalleeRef::Expr(lambda_expr @ Expr::Lambda { .. }) => {
            let closure = LocalLambda::Closure {
                expr: lambda_expr.clone(),
                captured: vc.local_lambdas.borrow().clone(),
            };
            lambda_may_write_arg_indices(module_env, &closure, args)
        }
        CalleeRef::Expr(_) => all_arg_indices(args),
    }
}

pub(crate) fn stmt_may_write_array(vc: &VCtx<'_>, stmt: &Stmt, name: &str, env: &Env<'_>) -> bool {
    let aliases = super::z3_types::array_aliases_in_stmt(stmt, name);
    stmt_has_may_write_call(vc, stmt, env, &aliases)
}

fn stmt_has_may_write_call(
    vc: &VCtx<'_>,
    stmt: &Stmt,
    env: &Env<'_>,
    aliases: &HashSet<String>,
) -> bool {
    match stmt {
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            expr_has_may_write_call(vc, value, env, aliases)
        }
        Stmt::ArrayStore { index, value, .. } => {
            expr_has_may_write_call(vc, index, env, aliases)
                || expr_has_may_write_call(vc, value, env, aliases)
        }
        Stmt::Block(stmts, _) => stmts
            .iter()
            .any(|stmt| stmt_has_may_write_call(vc, stmt, env, aliases)),
        Stmt::While {
            cond,
            invariant,
            decreases,
            body,
            ..
        } => {
            expr_has_may_write_call(vc, cond, env, aliases)
                || expr_has_may_write_call(vc, invariant, env, aliases)
                || decreases
                    .as_ref()
                    .is_some_and(|expr| expr_has_may_write_call(vc, expr, env, aliases))
                || stmt_has_may_write_call(vc, body, env, aliases)
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
            stmt_has_may_write_call(vc, body, env, aliases)
        }
        Stmt::TaskGroup { children, .. } => children
            .iter()
            .any(|child| stmt_has_may_write_call(vc, child, env, aliases)),
        Stmt::Cancel { .. } => false,
        Stmt::Expr(expr, _) => expr_has_may_write_call(vc, expr, env, aliases),
    }
}

fn expr_has_may_write_call(
    vc: &VCtx<'_>,
    expr: &Expr,
    env: &Env<'_>,
    aliases: &HashSet<String>,
) -> bool {
    match expr {
        Expr::Paren(inner) => expr_has_may_write_call(vc, inner, env, aliases),
        Expr::Call(callee, args) => {
            callee_may_write_args(vc, CalleeRef::Name(callee), args, env)
                .iter()
                .any(|name| aliases.contains(name))
                || args
                    .iter()
                    .any(|arg| expr_has_may_write_call(vc, arg, env, aliases))
        }
        Expr::Perform {
            operation, args, ..
        } => {
            callee_may_write_args(vc, CalleeRef::Name(operation), args, env)
                .iter()
                .any(|name| aliases.contains(name))
                || args
                    .iter()
                    .any(|arg| expr_has_may_write_call(vc, arg, env, aliases))
        }
        Expr::CallRef { callee, args } => {
            callee_may_write_args(vc, CalleeRef::Expr(callee), args, env)
                .iter()
                .any(|name| aliases.contains(name))
                || expr_has_may_write_call(vc, callee, env, aliases)
                || args
                    .iter()
                    .any(|arg| expr_has_may_write_call(vc, arg, env, aliases))
        }
        Expr::ArrayLit(elements) => elements
            .iter()
            .any(|element| expr_has_may_write_call(vc, element, env, aliases)),
        Expr::ArrayAccess(_, index) => expr_has_may_write_call(vc, index, env, aliases),
        Expr::BinaryOp(left, _, right) => {
            expr_has_may_write_call(vc, left, env, aliases)
                || expr_has_may_write_call(vc, right, env, aliases)
        }
        Expr::Block(stmt) => stmt_has_may_write_call(vc, stmt, env, aliases),
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            expr_has_may_write_call(vc, cond, env, aliases)
                || stmt_has_may_write_call(vc, then_branch, env, aliases)
                || stmt_has_may_write_call(vc, else_branch, env, aliases)
        }
        Expr::StructInit { fields, .. } => fields
            .iter()
            .any(|(_, value)| expr_has_may_write_call(vc, value, env, aliases)),
        Expr::FieldAccess(inner, _) | Expr::Await { expr: inner } => {
            expr_has_may_write_call(vc, inner, env, aliases)
        }
        Expr::Match { target, arms } => {
            expr_has_may_write_call(vc, target, env, aliases)
                || arms.iter().any(|arm| {
                    arm.guard
                        .as_deref()
                        .is_some_and(|guard| expr_has_may_write_call(vc, guard, env, aliases))
                        || stmt_has_may_write_call(vc, &arm.body, env, aliases)
                })
        }
        Expr::Async { body } | Expr::Lambda { body, .. } => {
            stmt_has_may_write_call(vc, body, env, aliases)
        }
        Expr::ChanSend { channel, value } => {
            expr_has_may_write_call(vc, channel, env, aliases)
                || expr_has_may_write_call(vc, value, env, aliases)
        }
        Expr::ChanRecv { channel } => expr_has_may_write_call(vc, channel, env, aliases),
        Expr::Number(_)
        | Expr::Float(_)
        | Expr::StringLit(_)
        | Expr::Variable(_)
        | Expr::AtomRef { .. } => false,
    }
}

/// `mod.f` resolves as `mod::f` too, matching the runtime `Call` path.
fn resolve_atom<'m>(module_env: &'m ModuleEnv, name: &str) -> Option<&'m crate::parser::Atom> {
    module_env
        .get_atom(name)
        .or_else(|| module_env.get_atom(&name.replace('.', "::")))
}

/// Variable args whose matching `callee` parameter the atom (transitively)
/// stores through.
pub(crate) fn atom_may_write_args(
    module_env: &ModuleEnv,
    callee: &crate::parser::Atom,
    args: &[Expr],
) -> HashSet<String> {
    let mut out = HashSet::new();
    for (i, arg) in args.iter().enumerate() {
        let (Expr::Variable(var), Some(param)) = (arg, callee.params.get(i)) else {
            continue;
        };
        if atom_stores_to_array(module_env, callee, &param.name) {
            out.insert(var.clone());
        }
    }
    out
}

fn atom_may_write_arg_indices(
    module_env: &ModuleEnv,
    callee: &crate::parser::Atom,
    args: &[Expr],
) -> HashSet<usize> {
    args.iter()
        .enumerate()
        .filter_map(|(i, _)| {
            callee
                .params
                .get(i)
                .filter(|param| atom_stores_to_array(module_env, callee, &param.name))
                .map(|_| i)
        })
        .collect()
}

/// Variable args whose lambda param the body stores through, plus every
/// caller-visible name the body stores to directly — the body runs on a
/// clone of the caller's env, so captured names share the caller's slots.
/// An `Ite` binding may apply either branch; `Opaque` runs no body.
pub(crate) fn lambda_may_write_args(
    module_env: &ModuleEnv,
    lambda: &LocalLambda<'_>,
    args: &[Expr],
    env: &Env<'_>,
) -> HashSet<String> {
    let mut out = HashSet::new();
    match lambda {
        LocalLambda::Closure { expr, .. } => {
            let Expr::Lambda { params, body, .. } = expr else {
                return out;
            };
            let lambda_params = params
                .iter()
                .map(|param| param.name.clone())
                .collect::<Vec<_>>();
            for (i, arg) in args.iter().enumerate() {
                let (Expr::Variable(var), Some(param)) = (arg, params.get(i)) else {
                    continue;
                };
                if stmt_stores_to_var(module_env, body, &param.name, &lambda_params) {
                    out.insert(var.clone());
                }
            }
            for name in env.keys().filter(|k| !k.starts_with("__")) {
                if params.iter().any(|p| p.name == *name) {
                    continue;
                }
                if stmt_stores_to_var(module_env, body, name, &lambda_params) {
                    out.insert(name.clone());
                }
            }
        }
        LocalLambda::Ite {
            then_lam, else_lam, ..
        } => {
            out.extend(lambda_may_write_args(module_env, then_lam, args, env));
            out.extend(lambda_may_write_args(module_env, else_lam, args, env));
        }
        LocalLambda::Opaque => {}
    }
    out
}

fn lambda_may_write_arg_indices(
    module_env: &ModuleEnv,
    lambda: &LocalLambda<'_>,
    args: &[Expr],
) -> HashSet<usize> {
    match lambda {
        LocalLambda::Closure { expr, .. } => {
            let Expr::Lambda { params, body, .. } = expr else {
                return HashSet::new();
            };
            let lambda_params = params
                .iter()
                .map(|param| param.name.clone())
                .collect::<Vec<_>>();
            args.iter()
                .enumerate()
                .filter_map(|(i, _)| {
                    params
                        .get(i)
                        .filter(|param| {
                            stmt_stores_to_var(module_env, body, &param.name, &lambda_params)
                        })
                        .map(|_| i)
                })
                .collect()
        }
        LocalLambda::Ite {
            then_lam, else_lam, ..
        } => {
            let mut indices = lambda_may_write_arg_indices(module_env, then_lam, args);
            indices.extend(lambda_may_write_arg_indices(module_env, else_lam, args));
            indices
        }
        LocalLambda::Opaque => HashSet::new(),
    }
}

/// Dynamic path (callee body unavailable): every variable arg may be
/// written — fail closed.
fn all_var_args(args: &[Expr]) -> HashSet<String> {
    args.iter()
        .filter_map(|a| match a {
            Expr::Variable(var) => Some(var.clone()),
            _ => None,
        })
        .collect()
}

fn all_arg_indices(args: &[Expr]) -> HashSet<usize> {
    (0..args.len()).collect()
}
