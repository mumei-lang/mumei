use super::super::module_env::*;
use super::super::types::Diagnostic;
use crate::lowering::{lower, LoweredType};
use crate::parser::*;
use std::collections::{HashMap, HashSet};

// =============================================================================
// Recursive contract support: SCC detection + eligibility
// =============================================================================
//
// An atom that declares `decreases: <expr>;` and sits on a cycle of the call
// graph (direct or mutual recursion) may be verified with congruent recursive
// contracts: each call to an SCC member produces `rec_fn#<callee>(args)` via an
// uninterpreted function, the callee's contract is assumed as
// `CallerRequires => CallerEnsures` (the induction hypothesis), and every
// recursive call carries a termination obligation
// `0 <= M_A && M_B(args) < M_A` where `M_A` is the caller's measure.
//
// Eligibility is deliberately narrow: scalar Int/Bool parameters and
// returns, one-dimensional arrays of scalar elements (as parameters or the
// return), structs with scalar fields (as parameters or the return), no
// effects, no borrows/consumes, no type parameters, default trust level, and
// a measure over the atom's own parameters with no calls except `len` of an
// array parameter. Array parameters additionally forbid stores through the
// parameter itself. Atoms on a cycle that fail eligibility keep the legacy
// unconstrained-call treatment and get an advisory hint instead (see
// `recursive_contract_hint_diagnostic`).

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecursiveSccEligibility {
    Eligible,
    /// SCC members (sorted) that do not declare a `decreases` clause.
    NeedsDecreases(Vec<String>),
    /// First non-decreases reason the SCC is not eligible.
    Unsupported(String),
}

#[derive(Debug, Clone)]
pub struct RecursiveScc {
    /// Sorted atom names of the cyclic SCC.
    pub members: Vec<String>,
    pub eligibility: RecursiveSccEligibility,
}

impl RecursiveScc {
    pub fn is_eligible(&self) -> bool {
        matches!(self.eligibility, RecursiveSccEligibility::Eligible)
    }

    pub fn contains(&self, atom_name: &str) -> bool {
        self.members.iter().any(|m| m == atom_name)
    }
}

/// Call edges of an expression: `f(..)` calls and `call(atom_ref(f), ..)`.
/// A bare `atom_ref(f)` without a call is not an edge, and `call(var, ..)`
/// through a non-atom_ref callee is not one either.
pub(crate) fn collect_call_edges_expr(expr: &Expr) -> Vec<String> {
    let mut edges = Vec::new();
    match expr {
        Expr::Call(name, args) => {
            edges.push(name.clone());
            for arg in args {
                edges.extend(collect_call_edges_expr(arg));
            }
        }
        Expr::CallRef { callee, args } => {
            if let Expr::AtomRef { name } = callee.as_ref() {
                edges.push(name.clone());
            }
            for arg in args {
                edges.extend(collect_call_edges_expr(arg));
            }
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            edges.extend(collect_call_edges_expr(cond));
            edges.extend(collect_call_edges_stmt(then_branch));
            edges.extend(collect_call_edges_stmt(else_branch));
        }
        Expr::Block(stmt) => edges.extend(collect_call_edges_stmt(stmt)),
        Expr::BinaryOp(l, _, r) => {
            edges.extend(collect_call_edges_expr(l));
            edges.extend(collect_call_edges_expr(r));
        }
        Expr::Async { body } => edges.extend(collect_call_edges_stmt(body)),
        Expr::Await { expr } => edges.extend(collect_call_edges_expr(expr)),
        Expr::Match { target, arms } => {
            edges.extend(collect_call_edges_expr(target));
            for arm in arms {
                edges.extend(collect_call_edges_stmt(&arm.body));
                if let Some(guard) = &arm.guard {
                    edges.extend(collect_call_edges_expr(guard));
                }
            }
        }
        Expr::Perform { args, .. } => {
            for arg in args {
                edges.extend(collect_call_edges_expr(arg));
            }
        }
        Expr::Lambda { body, .. } => edges.extend(collect_call_edges_stmt(body)),
        Expr::ChanSend { channel, value } => {
            edges.extend(collect_call_edges_expr(channel));
            edges.extend(collect_call_edges_expr(value));
        }
        Expr::ChanRecv { channel } => edges.extend(collect_call_edges_expr(channel)),
        Expr::FieldAccess(inner, _) => edges.extend(collect_call_edges_expr(inner)),
        Expr::ArrayAccess(_, idx) => edges.extend(collect_call_edges_expr(idx)),
        Expr::ArrayLit(elements) => {
            for e in elements {
                edges.extend(collect_call_edges_expr(e));
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, v) in fields {
                edges.extend(collect_call_edges_expr(v));
            }
        }
        _ => {}
    }
    edges
}

pub(crate) fn collect_call_edges_stmt(stmt: &Stmt) -> Vec<String> {
    let mut edges = Vec::new();
    match stmt {
        Stmt::Block(stmts, _) => {
            for s in stmts {
                edges.extend(collect_call_edges_stmt(s));
            }
        }
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            edges.extend(collect_call_edges_expr(value));
        }
        Stmt::ArrayStore { index, value, .. } => {
            edges.extend(collect_call_edges_expr(index));
            edges.extend(collect_call_edges_expr(value));
        }
        Stmt::While { cond, body, .. } => {
            edges.extend(collect_call_edges_expr(cond));
            edges.extend(collect_call_edges_stmt(body));
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
            edges.extend(collect_call_edges_stmt(body));
        }
        Stmt::TaskGroup { children, .. } => {
            for child in children {
                edges.extend(collect_call_edges_stmt(child));
            }
        }
        Stmt::Expr(e, _) => edges.extend(collect_call_edges_expr(e)),
        Stmt::Cancel { .. } => {}
    }
    edges
}

/// Contract clause texts of an atom: `requires`/`ensures` already carry the
/// labelled and moded clause texts (the parser conjoins them). Top-level
/// quantified requires conjuncts are moved into `forall_constraints` by the
/// parser instead, so their `start`/`end`/`condition` texts are included
/// too — together they are the complete contract surface.
fn contract_clause_texts(atom: &Atom) -> Vec<&str> {
    let mut texts = vec![atom.requires.as_str(), atom.ensures.as_str()];
    for q in &atom.forall_constraints {
        texts.push(q.start.as_str());
        texts.push(q.end.as_str());
        texts.push(q.condition.as_str());
    }
    texts
}

/// Edges from the atom's contract clauses only (requires/ensures), resolved
/// to atom names in `module_env`.
fn contract_call_edges(module_env: &ModuleEnv, atom: &Atom) -> Vec<String> {
    let mut edges = Vec::new();
    for text in contract_clause_texts(atom) {
        if text.trim() == "true" {
            continue;
        }
        let ast = parse_expression(text);
        for name in collect_call_edges_expr(&ast) {
            if let Some(resolved) = resolve_edge_name(module_env, &name) {
                edges.push(resolved);
            }
        }
    }
    edges
}

/// All call edges of an atom: body plus every contract clause text.
fn all_call_edges(module_env: &ModuleEnv, atom: &Atom) -> Vec<String> {
    let mut edges = contract_call_edges(module_env, atom);
    let body = parse_body_expr(&atom.body_expr);
    for name in collect_call_edges_stmt(&body) {
        if let Some(resolved) = resolve_edge_name(module_env, &name) {
            edges.push(resolved);
        }
    }
    edges
}

/// Resolve an edge name the way call lowering does: `module_env.get_atom(name)`
/// first, then `name.replace('.', "::")`. Unresolved names (builtins) are ignored.
fn resolve_edge_name(module_env: &ModuleEnv, name: &str) -> Option<String> {
    if let Some(atom) = module_env.get_atom(name) {
        return Some(atom.name.clone());
    }
    let fqn = name.replace('.', "::");
    module_env.get_atom(&fqn).map(|a| a.name.clone())
}

fn has_tuple_comma(text: &str) -> bool {
    let mut delimiters = Vec::new();
    for ch in text.chars() {
        match ch {
            '(' | '[' | '{' => delimiters.push(ch),
            ')' | ']' | '}' => {
                delimiters.pop();
            }
            ',' if delimiters.is_empty() || delimiters.last() == Some(&'(') => return true,
            _ => {}
        }
    }
    false
}

/// Components of a `decreases` clause: the parts of an outer `(m1, ..., mk)`
/// tuple, or the whole text for a single measure.
pub(crate) fn measure_components(text: &str) -> Vec<String> {
    let text = text.trim();
    if text == "()" {
        return vec![String::new()];
    }
    if !text.starts_with('(') {
        return vec![text.to_string()];
    }

    let mut parens = 1usize;
    let mut brackets = 0usize;
    let mut braces = 0usize;
    let mut separators = Vec::new();
    let mut closing = None;
    for (index, ch) in text.char_indices().skip(1) {
        match ch {
            '(' => parens += 1,
            ')' => {
                if parens == 0 {
                    return vec![text.to_string()];
                }
                parens -= 1;
                if parens == 0 && brackets == 0 && braces == 0 {
                    closing = Some(index);
                    break;
                }
            }
            '[' => brackets += 1,
            ']' => brackets = brackets.saturating_sub(1),
            '{' => braces += 1,
            '}' => braces = braces.saturating_sub(1),
            ',' if parens == 1 && brackets == 0 && braces == 0 => separators.push(index),
            _ => {}
        }
    }
    if closing != Some(text.len() - 1) || separators.is_empty() {
        return vec![text.to_string()];
    }

    let inner = &text[1..text.len() - 1];
    let mut components = Vec::with_capacity(separators.len() + 1);
    let mut start = 0;
    for separator in separators {
        components.push(inner[start..separator - 1].trim().to_string());
        start = separator;
    }
    components.push(inner[start..].trim().to_string());
    components
}

/// Variable names referenced anywhere in `expr`.
fn collect_variables_expr(expr: &Expr, out: &mut HashSet<String>) {
    match expr {
        Expr::Variable(name) => {
            out.insert(name.clone());
        }
        Expr::Call(_, args) | Expr::Perform { args, .. } => {
            for a in args {
                collect_variables_expr(a, out);
            }
        }
        Expr::CallRef { callee, args } => {
            collect_variables_expr(callee, out);
            for a in args {
                collect_variables_expr(a, out);
            }
        }
        Expr::BinaryOp(l, _, r) => {
            collect_variables_expr(l, out);
            collect_variables_expr(r, out);
        }
        Expr::FieldAccess(e, _) => collect_variables_expr(e, out),
        Expr::ArrayAccess(_, idx) => collect_variables_expr(idx, out),
        Expr::ArrayLit(elements) => {
            for e in elements {
                collect_variables_expr(e, out);
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, v) in fields {
                collect_variables_expr(v, out);
            }
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_variables_expr(cond, out);
            collect_variables_stmt(then_branch, out);
            collect_variables_stmt(else_branch, out);
        }
        Expr::Block(stmt) => collect_variables_stmt(stmt, out),
        Expr::Match { target, arms } => {
            collect_variables_expr(target, out);
            for arm in arms {
                collect_variables_stmt(&arm.body, out);
                if let Some(guard) = &arm.guard {
                    collect_variables_expr(guard, out);
                }
            }
        }
        Expr::Lambda { body, .. } | Expr::Async { body } => collect_variables_stmt(body, out),
        Expr::Await { expr } => collect_variables_expr(expr, out),
        Expr::ChanSend { channel, value } => {
            collect_variables_expr(channel, out);
            collect_variables_expr(value, out);
        }
        Expr::ChanRecv { channel } => collect_variables_expr(channel, out),
        _ => {}
    }
}

fn collect_variables_stmt(stmt: &Stmt, out: &mut HashSet<String>) {
    match stmt {
        Stmt::Block(stmts, _) => {
            for s in stmts {
                collect_variables_stmt(s, out);
            }
        }
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            collect_variables_expr(value, out);
        }
        Stmt::ArrayStore { index, value, .. } => {
            collect_variables_expr(index, out);
            collect_variables_expr(value, out);
        }
        Stmt::While {
            cond,
            invariant,
            decreases,
            body,
            ..
        } => {
            collect_variables_expr(cond, out);
            collect_variables_expr(invariant, out);
            if let Some(decreases) = decreases {
                collect_variables_expr(decreases, out);
            }
            collect_variables_stmt(body, out);
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
            collect_variables_stmt(body, out);
        }
        Stmt::TaskGroup { children, .. } => {
            for child in children {
                collect_variables_stmt(child, out);
            }
        }
        Stmt::Expr(e, _) => collect_variables_expr(e, out),
        Stmt::Cancel { .. } => {}
    }
}

fn measure_contains_array_access(expr: &Expr) -> bool {
    match expr {
        Expr::ArrayAccess(_, _) => true,
        Expr::Call(_, args) | Expr::Perform { args, .. } | Expr::ArrayLit(args) => {
            args.iter().any(measure_contains_array_access)
        }
        Expr::CallRef { callee, args } => {
            measure_contains_array_access(callee) || args.iter().any(measure_contains_array_access)
        }
        Expr::BinaryOp(left, _, right) => {
            measure_contains_array_access(left) || measure_contains_array_access(right)
        }
        Expr::FieldAccess(inner, _) | Expr::Await { expr: inner } => {
            measure_contains_array_access(inner)
        }
        Expr::StructInit { fields, .. } => fields
            .iter()
            .any(|(_, value)| measure_contains_array_access(value)),
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            measure_contains_array_access(cond)
                || measure_contains_array_access_stmt(then_branch)
                || measure_contains_array_access_stmt(else_branch)
        }
        Expr::Block(stmt) | Expr::Async { body: stmt } | Expr::Lambda { body: stmt, .. } => {
            measure_contains_array_access_stmt(stmt)
        }
        Expr::Match { target, arms } => {
            measure_contains_array_access(target)
                || arms.iter().any(|arm| {
                    measure_contains_array_access_stmt(&arm.body)
                        || arm
                            .guard
                            .as_ref()
                            .is_some_and(|guard| measure_contains_array_access(guard))
                })
        }
        Expr::ChanSend { channel, value } => {
            measure_contains_array_access(channel) || measure_contains_array_access(value)
        }
        Expr::ChanRecv { channel } => measure_contains_array_access(channel),
        _ => false,
    }
}

fn measure_contains_array_access_stmt(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Block(stmts, _) => stmts.iter().any(measure_contains_array_access_stmt),
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            measure_contains_array_access(value)
        }
        Stmt::ArrayStore { index, value, .. } => {
            measure_contains_array_access(index) || measure_contains_array_access(value)
        }
        Stmt::While {
            cond,
            invariant,
            decreases,
            body,
            ..
        } => {
            measure_contains_array_access(cond)
                || measure_contains_array_access(invariant)
                || decreases
                    .as_ref()
                    .is_some_and(|measure| measure_contains_array_access(measure))
                || measure_contains_array_access_stmt(body)
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
            measure_contains_array_access_stmt(body)
        }
        Stmt::TaskGroup { children, .. } => children.iter().any(measure_contains_array_access_stmt),
        Stmt::Expr(expr, _) => measure_contains_array_access(expr),
        Stmt::Cancel { .. } => false,
    }
}

fn measure_contains_disallowed_call(expr: &Expr, array_params: &HashSet<&str>) -> bool {
    match expr {
        Expr::Call(name, args) => {
            let allowed_len = name == "len"
                && args.len() == 1
                && matches!(&args[0], Expr::Variable(param) if array_params.contains(param.as_str()));
            !allowed_len
                || args
                    .iter()
                    .any(|arg| measure_contains_disallowed_call(arg, array_params))
        }
        Expr::CallRef { .. } => true,
        Expr::Perform { args, .. } | Expr::ArrayLit(args) => args
            .iter()
            .any(|arg| measure_contains_disallowed_call(arg, array_params)),
        Expr::BinaryOp(left, _, right) => {
            measure_contains_disallowed_call(left, array_params)
                || measure_contains_disallowed_call(right, array_params)
        }
        Expr::FieldAccess(inner, _) | Expr::Await { expr: inner } => {
            measure_contains_disallowed_call(inner, array_params)
        }
        Expr::StructInit { fields, .. } => fields
            .iter()
            .any(|(_, value)| measure_contains_disallowed_call(value, array_params)),
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            measure_contains_disallowed_call(cond, array_params)
                || measure_contains_disallowed_call_stmt(then_branch, array_params)
                || measure_contains_disallowed_call_stmt(else_branch, array_params)
        }
        Expr::Block(stmt) | Expr::Async { body: stmt } | Expr::Lambda { body: stmt, .. } => {
            measure_contains_disallowed_call_stmt(stmt, array_params)
        }
        Expr::Match { target, arms } => {
            measure_contains_disallowed_call(target, array_params)
                || arms.iter().any(|arm| {
                    measure_contains_disallowed_call_stmt(&arm.body, array_params)
                        || arm.guard.as_ref().is_some_and(|guard| {
                            measure_contains_disallowed_call(guard, array_params)
                        })
                })
        }
        Expr::ChanSend { channel, value } => {
            measure_contains_disallowed_call(channel, array_params)
                || measure_contains_disallowed_call(value, array_params)
        }
        Expr::ChanRecv { channel } => measure_contains_disallowed_call(channel, array_params),
        _ => false,
    }
}

fn measure_contains_disallowed_call_stmt(stmt: &Stmt, array_params: &HashSet<&str>) -> bool {
    match stmt {
        Stmt::Block(stmts, _) => stmts
            .iter()
            .any(|stmt| measure_contains_disallowed_call_stmt(stmt, array_params)),
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            measure_contains_disallowed_call(value, array_params)
        }
        Stmt::ArrayStore { index, value, .. } => {
            measure_contains_disallowed_call(index, array_params)
                || measure_contains_disallowed_call(value, array_params)
        }
        Stmt::While {
            cond,
            invariant,
            decreases,
            body,
            ..
        } => {
            measure_contains_disallowed_call(cond, array_params)
                || measure_contains_disallowed_call(invariant, array_params)
                || decreases
                    .as_ref()
                    .is_some_and(|measure| measure_contains_disallowed_call(measure, array_params))
                || measure_contains_disallowed_call_stmt(body, array_params)
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
            measure_contains_disallowed_call_stmt(body, array_params)
        }
        Stmt::TaskGroup { children, .. } => children
            .iter()
            .any(|child| measure_contains_disallowed_call_stmt(child, array_params)),
        Stmt::Expr(expr, _) => measure_contains_disallowed_call(expr, array_params),
        Stmt::Cancel { .. } => false,
    }
}

fn measure_non_scalar_param(
    expr: &Expr,
    arrays: &HashSet<&str>,
    structs: &HashSet<&str>,
) -> Option<String> {
    match expr {
        Expr::Variable(name)
            if arrays.contains(name.as_str()) || structs.contains(name.as_str()) =>
        {
            Some(name.clone())
        }
        Expr::Call(name, args) => {
            let allowed_len = name == "len"
                && args.len() == 1
                && matches!(&args[0], Expr::Variable(param) if arrays.contains(param.as_str()));
            args.iter()
                .filter(|_| !allowed_len)
                .find_map(|arg| measure_non_scalar_param(arg, arrays, structs))
        }
        Expr::CallRef { callee, args } => measure_non_scalar_param(callee, arrays, structs)
            .or_else(|| {
                args.iter()
                    .find_map(|arg| measure_non_scalar_param(arg, arrays, structs))
            }),
        Expr::FieldAccess(inner, _) => {
            if matches!(inner.as_ref(), Expr::Variable(name) if structs.contains(name.as_str())) {
                None
            } else {
                measure_non_scalar_param(inner, arrays, structs)
            }
        }
        Expr::ArrayAccess(_, index) => measure_non_scalar_param(index, arrays, structs),
        Expr::Perform { args, .. } | Expr::ArrayLit(args) => args
            .iter()
            .find_map(|arg| measure_non_scalar_param(arg, arrays, structs)),
        Expr::BinaryOp(left, _, right) => measure_non_scalar_param(left, arrays, structs)
            .or_else(|| measure_non_scalar_param(right, arrays, structs)),
        Expr::StructInit { fields, .. } => fields
            .iter()
            .find_map(|(_, value)| measure_non_scalar_param(value, arrays, structs)),
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => measure_non_scalar_param(cond, arrays, structs)
            .or_else(|| measure_non_scalar_param_stmt(then_branch, arrays, structs))
            .or_else(|| measure_non_scalar_param_stmt(else_branch, arrays, structs)),
        Expr::Block(stmt) | Expr::Async { body: stmt } | Expr::Lambda { body: stmt, .. } => {
            measure_non_scalar_param_stmt(stmt, arrays, structs)
        }
        Expr::Match { target, arms } => {
            measure_non_scalar_param(target, arrays, structs).or_else(|| {
                arms.iter().find_map(|arm| {
                    measure_non_scalar_param_stmt(&arm.body, arrays, structs).or_else(|| {
                        arm.guard
                            .as_ref()
                            .and_then(|guard| measure_non_scalar_param(guard, arrays, structs))
                    })
                })
            })
        }
        Expr::ChanSend { channel, value } => measure_non_scalar_param(channel, arrays, structs)
            .or_else(|| measure_non_scalar_param(value, arrays, structs)),
        Expr::ChanRecv { channel } => measure_non_scalar_param(channel, arrays, structs),
        _ => None,
    }
}

fn measure_non_scalar_param_stmt(
    stmt: &Stmt,
    arrays: &HashSet<&str>,
    structs: &HashSet<&str>,
) -> Option<String> {
    match stmt {
        Stmt::Block(stmts, _) => stmts
            .iter()
            .find_map(|stmt| measure_non_scalar_param_stmt(stmt, arrays, structs)),
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            measure_non_scalar_param(value, arrays, structs)
        }
        Stmt::ArrayStore { index, value, .. } => measure_non_scalar_param(index, arrays, structs)
            .or_else(|| measure_non_scalar_param(value, arrays, structs)),
        Stmt::While {
            cond,
            invariant,
            decreases,
            body,
            ..
        } => measure_non_scalar_param(cond, arrays, structs)
            .or_else(|| measure_non_scalar_param(invariant, arrays, structs))
            .or_else(|| {
                decreases
                    .as_ref()
                    .and_then(|measure| measure_non_scalar_param(measure, arrays, structs))
            })
            .or_else(|| measure_non_scalar_param_stmt(body, arrays, structs)),
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
            measure_non_scalar_param_stmt(body, arrays, structs)
        }
        Stmt::TaskGroup { children, .. } => children
            .iter()
            .find_map(|child| measure_non_scalar_param_stmt(child, arrays, structs)),
        Stmt::Expr(expr, _) => measure_non_scalar_param(expr, arrays, structs),
        Stmt::Cancel { .. } => None,
    }
}

/// True when the type is a scalar Int or Bool sort (i64/u64/i32/…, bool, or a
/// refinement over those). A missing type name defaults to i64.
fn is_scalar_int_or_bool(module_env: &ModuleEnv, type_name: Option<&str>) -> bool {
    let Some(type_name) = type_name else {
        return true;
    };
    if module_env.get_struct(type_name).is_some() || module_env.get_enum(type_name).is_some() {
        return false;
    }
    let base = module_env.resolve_base_type(type_name);
    match lower(&base) {
        LoweredType::I64
        | LoweredType::I32
        | LoweredType::U64
        | LoweredType::U32
        | LoweredType::Bool => true,
        LoweredType::Other(name) => matches!(
            name.as_str(),
            "i8" | "i16" | "i128" | "u8" | "u16" | "u128" | "isize" | "usize" | "int"
        ),
        _ => false,
    }
}

fn array_element_type(type_name: &str) -> Option<&str> {
    let trimmed = type_name.trim();
    trimmed
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .map(str::trim)
}

/// True when `element_type` names a supported one-dimensional array element:
/// a scalar Int/Bool base type — no nested arrays, struct/enum elements, or
/// named (refined) element types. Shared by the parameter and return-type
/// checks so both sides accept the same shapes.
fn eligible_array_element(module_env: &ModuleEnv, element_type: &str) -> bool {
    !element_type.is_empty()
        && !element_type.starts_with('[')
        && module_env.get_struct(element_type).is_none()
        && module_env.get_enum(element_type).is_none()
        && module_env.get_type(element_type).is_none()
        && is_scalar_int_or_bool(module_env, Some(element_type))
}

/// True when `sdef` has only scalar Int/Bool fields and neither its field
/// `where` constraints nor its invariants contain calls. Shared by the
/// parameter and return-type checks.
fn eligible_struct_shape(module_env: &ModuleEnv, sdef: &StructDef) -> bool {
    sdef.fields
        .iter()
        .all(|field| is_scalar_int_or_bool(module_env, Some(field.type_name.trim())))
        && sdef.fields.iter().all(|field| {
            field
                .constraint
                .as_deref()
                .is_none_or(|text| collect_call_edges_expr(&parse_expression(text)).is_empty())
        })
        && sdef
            .invariants
            .iter()
            .all(|text| collect_call_edges_expr(&parse_expression(text)).is_empty())
}

/// The recursive-contract eligibility of `member`'s return type: scalar
/// Int/Bool as before, a `[T]` whose element satisfies the same rule as
/// array parameters, or a struct satisfying the same shape rule as struct
/// parameters. A missing return type defaults to i64.
fn rec_result_unsupported_reason(module_env: &ModuleEnv, member: &Atom) -> Option<String> {
    let type_name = member.return_type.as_deref().map(str::trim)?;
    if is_scalar_int_or_bool(module_env, Some(type_name)) {
        return None;
    }
    if let Some(element_type) = array_element_type(type_name) {
        return if eligible_array_element(module_env, element_type) {
            None
        } else {
            Some(format!(
                "atom '{}' returns an array with unsupported element type '{}'",
                member.name, element_type
            ))
        };
    }
    if let Some(sdef) = module_env.get_struct(type_name) {
        return if eligible_struct_shape(module_env, sdef) {
            None
        } else {
            Some(format!(
                "atom '{}' returns struct type '{}' with non-scalar fields or call-containing constraints/invariants",
                member.name, sdef.name
            ))
        };
    }
    Some(format!(
        "atom '{}' returns a non-scalar type '{}'",
        member.name, type_name
    ))
}

/// Check that `atom`'s `decreases` measure uses only the permitted projections
/// of its own parameters (plus literals/arithmetic).
fn measure_is_valid(module_env: &ModuleEnv, atom: &Atom) -> Result<(), String> {
    let measures = measure_components(atom.decreases.as_deref().unwrap_or(""));
    let params: HashSet<&str> = atom.params.iter().map(|p| p.name.as_str()).collect();
    let array_params: HashSet<&str> = atom
        .params
        .iter()
        .filter(|p| {
            p.type_name
                .as_deref()
                .and_then(array_element_type)
                .is_some()
        })
        .map(|p| p.name.as_str())
        .collect();
    let struct_params: HashSet<&str> = atom
        .params
        .iter()
        .filter(|p| {
            p.type_name
                .as_deref()
                .is_some_and(|type_name| module_env.get_struct(type_name.trim()).is_some())
        })
        .map(|p| p.name.as_str())
        .collect();
    for measure in measures {
        if measure.is_empty() {
            return Err(format!(
                "atom '{}' has an empty decreases clause",
                atom.name
            ));
        }
        let ast = parse_expression(&measure);
        if measure_contains_array_access(&ast) {
            return Err(format!(
                "decreases measure of atom '{}' must not read array elements",
                atom.name
            ));
        }
        if measure_contains_disallowed_call(&ast, &array_params) {
            return Err(format!(
                "decreases measure of atom '{}' must not contain calls",
                atom.name
            ));
        }
        if let Some(param) = measure_non_scalar_param(&ast, &array_params, &struct_params) {
            return Err(format!(
                "decreases measure of atom '{}' uses non-scalar parameter '{}' directly",
                atom.name, param
            ));
        }
        if has_tuple_comma(&measure) {
            return Err(format!(
                "decreases measure of atom '{}' must not nest tuples",
                atom.name
            ));
        }
        let mut vars = HashSet::new();
        collect_variables_expr(&ast, &mut vars);
        if let Some(unknown) = vars.iter().find(|v| !params.contains(v.as_str())) {
            return Err(format!(
                "decreases measure of atom '{}' references '{}' which is not a parameter",
                atom.name, unknown
            ));
        }
    }
    Ok(())
}

/// First non-decreases eligibility failure for `member`, or None.
fn member_unsupported_reason(module_env: &ModuleEnv, member: &Atom) -> Option<String> {
    if let Err(reason) = measure_is_valid(module_env, member) {
        return Some(reason);
    }
    if !member.effects.is_empty() {
        return Some(format!("atom '{}' declares effects", member.name));
    }
    if member.params.iter().any(|p| p.is_ref_mut) {
        return Some(format!("atom '{}' has a `ref mut` parameter", member.name));
    }
    if !member.consumed_params.is_empty() {
        return Some(format!("atom '{}' consumes a parameter", member.name));
    }
    if member.is_async {
        return Some(format!("atom '{}' is async", member.name));
    }
    if !member.type_params.is_empty() {
        return Some(format!("atom '{}' has type parameters", member.name));
    }
    if member.trust_level != TrustLevel::Verified {
        return Some(format!(
            "atom '{}' is not at the default verified trust level",
            member.name
        ));
    }
    if member
        .clause_modes
        .iter()
        .any(|m| m.kind == ClauseKind::Ensures && m.mode == ClauseTrustMode::Assume)
    {
        return Some(format!(
            "atom '{}' has an assume-mode ensures clause",
            member.name
        ));
    }
    if let Some(reason) = rec_result_unsupported_reason(module_env, member) {
        return Some(reason);
    }
    for param in &member.params {
        let type_name = param.type_name.as_deref().map(str::trim);
        if is_scalar_int_or_bool(module_env, type_name) {
            continue;
        }

        if let Some(element_type) = type_name.and_then(array_element_type) {
            if crate::verification::translator::atom_stores_to_array(
                module_env,
                member,
                &param.name,
            ) {
                return Some(format!(
                    "atom '{}' may store into array parameter '{}'",
                    member.name, param.name
                ));
            }
            if !eligible_array_element(module_env, element_type) {
                return Some(format!(
                    "atom '{}' parameter '{}' has an unsupported array element type '{}'",
                    member.name, param.name, element_type
                ));
            }
            continue;
        }

        if let Some(sdef) = type_name.and_then(|name| module_env.get_struct(name)) {
            if !eligible_struct_shape(module_env, sdef) {
                return Some(format!(
                    "atom '{}' parameter '{}' has struct type '{}' with non-scalar fields or call-containing constraints/invariants",
                    member.name, param.name, sdef.name
                ));
            }
            continue;
        }

        return Some(format!(
            "atom '{}' parameter '{}' has a non-scalar type",
            member.name, param.name
        ));
    }
    None
}

/// The cyclic SCC containing `atom_name`, or None when `atom_name` is not on
/// any cycle reachable from itself (including self-loops).
pub fn recursive_scc(module_env: &ModuleEnv, atom_name: &str) -> Option<RecursiveScc> {
    let start = resolve_edge_name(module_env, atom_name).unwrap_or_else(|| atom_name.to_string());
    module_env.get_atom(&start)?;

    // Reachable subgraph from `start`.
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    let mut stack = vec![start.clone()];
    let mut seen: HashSet<String> = HashSet::new();
    while let Some(name) = stack.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let Some(atom) = module_env.get_atom(&name) else {
            continue;
        };
        let mut out: Vec<String> = all_call_edges(module_env, atom);
        out.sort();
        out.dedup();
        for next in &out {
            if !seen.contains(next) {
                stack.push(next.clone());
            }
        }
        edges.insert(name, out);
    }

    // Tarjan SCC, restricted to the reachable subgraph.
    struct Tarjan<'g> {
        edges: &'g HashMap<String, Vec<String>>,
        index: usize,
        indices: HashMap<String, usize>,
        lowlink: HashMap<String, usize>,
        on_stack: HashSet<String>,
        stack: Vec<String>,
        sccs: Vec<Vec<String>>,
    }
    fn strong_connect(t: &mut Tarjan, v: &str) {
        t.indices.insert(v.to_string(), t.index);
        t.lowlink.insert(v.to_string(), t.index);
        t.index += 1;
        t.stack.push(v.to_string());
        t.on_stack.insert(v.to_string());
        if let Some(outs) = t.edges.get(v) {
            for w in outs.clone() {
                if !t.indices.contains_key(&w) {
                    strong_connect(t, &w);
                    let lw = t.lowlink[&w];
                    let lv = t.lowlink[v];
                    t.lowlink.insert(v.to_string(), lv.min(lw));
                } else if t.on_stack.contains(&w) {
                    let iw = t.indices[&w];
                    let lv = t.lowlink[v];
                    t.lowlink.insert(v.to_string(), lv.min(iw));
                }
            }
        }
        if t.lowlink[v] == t.indices[v] {
            let mut scc = Vec::new();
            while let Some(w) = t.stack.pop() {
                t.on_stack.remove(&w);
                scc.push(w.clone());
                if w == v {
                    break;
                }
            }
            scc.sort();
            t.sccs.push(scc);
        }
    }
    let mut tarjan = Tarjan {
        edges: &edges,
        index: 0,
        indices: HashMap::new(),
        lowlink: HashMap::new(),
        on_stack: HashSet::new(),
        stack: Vec::new(),
        sccs: Vec::new(),
    };
    strong_connect(&mut tarjan, &start);

    let scc = tarjan.sccs.into_iter().find(|s| s.contains(&start))?;
    let cyclic = scc.len() > 1
        || edges
            .get(&start)
            .map(|outs| outs.iter().any(|o| o == &start))
            .unwrap_or(false);
    if !cyclic {
        return None;
    }

    let mut missing: Vec<String> = scc
        .iter()
        .filter(|m| {
            module_env
                .get_atom(m)
                .map(|a| a.decreases.is_none())
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    missing.sort();
    let eligibility = if !missing.is_empty() {
        RecursiveSccEligibility::NeedsDecreases(missing)
    } else {
        let reason = scc.iter().find_map(|m| {
            let atom = module_env.get_atom(m)?;
            member_unsupported_reason(module_env, atom)
        });
        match reason {
            None => {
                let arities: Vec<(String, usize)> = scc
                    .iter()
                    .filter_map(|member| {
                        module_env.get_atom(member).map(|atom| {
                            (
                                member.clone(),
                                measure_components(atom.decreases.as_deref().unwrap_or("")).len(),
                            )
                        })
                    })
                    .collect();
                if arities
                    .first()
                    .is_some_and(|(_, first)| arities.iter().any(|(_, arity)| arity != first))
                {
                    let list = arities
                        .iter()
                        .map(|(member, arity)| format!("{member} has {arity}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    RecursiveSccEligibility::Unsupported(format!(
                        "SCC members declare `decreases` measures of different arity: {list}"
                    ))
                } else {
                    RecursiveSccEligibility::Eligible
                }
            }
            Some(reason) => RecursiveSccEligibility::Unsupported(reason),
        }
    };
    Some(RecursiveScc {
        members: scc,
        eligibility,
    })
}

/// Advisory hint for atoms whose contract clauses call into their own cyclic
/// SCC without the SCC being eligible for congruent recursive contracts. Pure
/// source analysis — no solver. Never affects the verdict.
pub fn recursive_contract_hint_diagnostic(
    atom: &Atom,
    module_env: &ModuleEnv,
) -> Option<Diagnostic> {
    let contract_edges = contract_call_edges(module_env, atom);
    if contract_edges.is_empty() {
        return None;
    }
    let scc = recursive_scc(module_env, &atom.name)?;
    if !contract_edges.iter().any(|e| scc.contains(e)) {
        return None;
    }
    let (code, detail) = match &scc.eligibility {
        RecursiveSccEligibility::Eligible => return None,
        RecursiveSccEligibility::NeedsDecreases(missing) => (
            "recursive_contract_needs_decreases",
            format!("SCC members missing `decreases`: {}", missing.join(", ")),
        ),
        RecursiveSccEligibility::Unsupported(reason) => {
            ("recursive_contract_unsupported", reason.clone())
        }
    };
    let message = format!(
        "atom '{}' is on a recursive call cycle ({}) but its contract cannot use congruent recursive calls: {}. The contract is checked with the call results left unconstrained.",
        atom.name,
        scc.members.join(", "),
        detail
    );
    Some(Diagnostic {
        code: code.to_string(),
        severity: "hint".to_string(),
        atom: atom.name.clone(),
        message,
        tags: vec![code.to_string()],
        escalation_reason: None,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        has_tuple_comma, measure_components, member_unsupported_reason,
        recursive_contract_hint_diagnostic,
    };
    use crate::parser::Item;
    use crate::verification::module_env::ModuleEnv;

    #[test]
    fn unsupported_float_array_parameter_has_hint_reason() {
        let items = crate::parser::parse_module(
            r#"
atom bad(a: [f64]) -> i64
    requires: len(a) >= 0;
    ensures: result == bad(a) + 1;
    decreases: len(a);
    body: 0;
"#,
        );
        let mut module_env = ModuleEnv::new();
        for item in items {
            match item {
                Item::Atom(atom) => module_env.register_atom(&atom),
                Item::StructDef(sdef) => module_env.register_struct(&sdef),
                Item::TypeDef(refined) => module_env.register_type(&refined),
                Item::EnumDef(edef) => module_env.register_enum(&edef),
                _ => {}
            }
        }
        let atom = module_env.get_atom("bad").unwrap();
        let reason = "atom 'bad' parameter 'a' has an unsupported array element type 'f64'";
        assert_eq!(
            member_unsupported_reason(&module_env, atom).as_deref(),
            Some(reason)
        );
        assert!(recursive_contract_hint_diagnostic(atom, &module_env).is_some());
    }

    fn env_with(items: Vec<Item>) -> ModuleEnv {
        let mut module_env = ModuleEnv::new();
        for item in items {
            match item {
                Item::Atom(atom) => module_env.register_atom(&atom),
                Item::StructDef(sdef) => module_env.register_struct(&sdef),
                Item::TypeDef(refined) => module_env.register_type(&refined),
                Item::EnumDef(edef) => module_env.register_enum(&edef),
                _ => {}
            }
        }
        module_env
    }

    #[test]
    fn unsupported_return_types_have_hint_reasons() {
        for (source, expected) in [
            (
                r#"atom bad() -> [f64]
    requires: true;
    ensures: len(bad()) == -1;
    decreases: 0;
    body: bad();
"#,
                Some("atom 'bad' returns an array with unsupported element type 'f64'"),
            ),
            (
                r#"atom bad() -> [[i64]]
    requires: true;
    ensures: len(bad()) == -1;
    decreases: 0;
    body: bad();
"#,
                Some("atom 'bad' returns an array with unsupported element type '[i64]'"),
            ),
            (
                r#"struct Wrap { a: [i64] }

atom read_a(w: Wrap) -> i64
    requires: true;
    ensures: true;
    body: 0;

atom bad() -> Wrap
    requires: true;
    ensures: read_a(bad()) == -1;
    decreases: 0;
    body: bad();
"#,
                Some(
                    "atom 'bad' returns struct type 'Wrap' with non-scalar fields or call-containing constraints/invariants",
                ),
            ),
            (
                r#"atom helper() -> i64
    requires: true;
    ensures: true;
    body: 0;

struct P {
    n: i64,
    invariant: helper() >= 0
}

atom read_n(p: P) -> i64
    requires: true;
    ensures: true;
    body: 0;

atom bad() -> P
    requires: true;
    ensures: read_n(bad()) == -1;
    decreases: 0;
    body: bad();
"#,
                Some(
                    "atom 'bad' returns struct type 'P' with non-scalar fields or call-containing constraints/invariants",
                ),
            ),
        ] {
            let module_env = env_with(crate::parser::parse_module(source));
            let atom = module_env.get_atom("bad").unwrap();
            assert_eq!(
                member_unsupported_reason(&module_env, atom).as_deref(),
                expected,
                "{source}"
            );
        }
    }

    #[test]
    fn array_and_scalar_struct_returns_are_eligible() {
        for source in [
            r#"atom ok(a: [i64], n: i64) -> [i64]
    requires: n >= 0;
    ensures: len(result) == len(a);
    decreases: n;
    body: if n == 0 { a } else { ok(a, n - 1) };
"#,
            r#"atom ok(a: [bool], n: i64) -> [bool]
    requires: n >= 0;
    ensures: len(result) == len(a);
    decreases: n;
    body: if n == 0 { a } else { ok(a, n - 1) };
"#,
            r#"struct Point { x: i64, y: i64 }

atom ok(p: Point, n: i64) -> Point
    requires: n >= 0;
    ensures: result.x == p.x;
    decreases: n;
    body: if n == 0 { p } else { ok(p, n - 1) };
"#,
        ] {
            let module_env = env_with(crate::parser::parse_module(source));
            let atom = module_env.get_atom("ok").unwrap();
            assert_eq!(
                member_unsupported_reason(&module_env, atom),
                None,
                "{source}"
            );
        }
    }

    #[test]
    fn splits_only_outer_decreases_tuples() {
        let cases = [
            ("n", vec!["n"]),
            ("(m, n)", vec!["m", "n"]),
            ("(m + 1, n, k)", vec!["m + 1", "n", "k"]),
            ("(n)", vec!["(n)"]),
            ("(a + b)", vec!["(a + b)"]),
            ("(a) + (b)", vec!["(a) + (b)"]),
            ("(f(a, b))", vec!["(f(a, b))"]),
            ("(m, )", vec!["m", ""]),
            ("()", vec![""]),
            ("(a, (b, c))", vec!["a", "(b, c)"]),
        ];

        for (text, expected) in cases {
            assert_eq!(
                measure_components(text),
                expected.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                "{text}"
            );
        }
    }

    #[test]
    fn detects_tuple_commas_by_innermost_delimiter() {
        let cases = [
            ("match n { 0 => 0, _ => n }", false),
            ("((m, k))", true),
            ("(b, a) + 1", true),
            ("n", false),
            ("match n { 0 => (a, b), _ => n }", true),
        ];

        for (text, expected) in cases {
            assert_eq!(has_tuple_comma(text), expected, "{text}");
        }
    }
}
