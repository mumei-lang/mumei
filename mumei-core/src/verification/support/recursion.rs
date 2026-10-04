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
// Eligibility is deliberately narrow: scalar Int/Bool signatures only, no
// effects, no borrows/consumes, no type parameters, default trust level, and a
// call-free measure over the atom's own parameters. Atoms on a cycle that fail
// eligibility keep the legacy unconstrained-call treatment and get an
// advisory hint instead (see `recursive_contract_hint_diagnostic`).

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
        Stmt::While { cond, body, .. } => {
            collect_variables_expr(cond, out);
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

/// Check that `atom`'s `decreases` measure is call-free and only references
/// the atom's own params (plus literals/arithmetic).
fn measure_is_valid(atom: &Atom) -> Result<(), String> {
    let measures = measure_components(atom.decreases.as_deref().unwrap_or(""));
    let params: HashSet<&str> = atom.params.iter().map(|p| p.name.as_str()).collect();
    for measure in measures {
        if measure.is_empty() {
            return Err(format!(
                "atom '{}' has an empty decreases clause",
                atom.name
            ));
        }
        let ast = parse_expression(&measure);
        if !collect_call_edges_expr(&ast).is_empty() {
            return Err(format!(
                "decreases measure of atom '{}' must not contain calls",
                atom.name
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
    if let Err(reason) = measure_is_valid(member) {
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
    if !is_scalar_int_or_bool(module_env, member.return_type.as_deref()) {
        return Some(format!(
            "atom '{}' returns a non-scalar type '{}'",
            member.name,
            member.return_type.as_deref().unwrap_or("")
        ));
    }
    if let Some(param) = member
        .params
        .iter()
        .find(|p| !is_scalar_int_or_bool(module_env, p.type_name.as_deref()))
    {
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
    use super::{has_tuple_comma, measure_components};

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
