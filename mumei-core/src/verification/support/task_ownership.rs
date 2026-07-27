// =============================================================================
// Structured Concurrency Ownership Analysis
// =============================================================================
//
// Ownership/data-race checking for `task_group` bodies, complementing the
// MIR move analysis (Phase 1h) and the Z3 join constraints in
// `verification/translator/stmt.rs`.
//
// MIR lowering flattens a `task_group` into a *sequential* chain of child
// bodies, so it models neither the concurrent interleaving of siblings nor the
// cancellation of losing children in `task_group:any`. This pass works on the
// surface AST, where the group structure is still visible, and reports:
//
//   * ConcurrentDoubleMove   — two siblings move the same captured value.
//   * MoveWhileSiblingUses   — one sibling moves a value a sibling still reads.
//   * ConcurrentDataRace     — a captured value is written by one sibling and
//                              read or written by another.
//   * UseAfterConcurrentMove — the parent uses a value a child moved.
//   * CancelDependentRead    — the parent reads a value written by a
//                              `task_group:any` child, whose write may or may
//                              not have happened depending on cancellation.
//
// All violations are hard verification errors: they are decided syntactically,
// never produce a Z3 `unknown`, and therefore never reach the Lean escalation
// path (so a rejected atom can never be promoted to `lean_verified`).
// =============================================================================

use super::super::module_env::ModuleEnv;
use super::super::types::{MumeiError, MumeiResult};
use crate::mir::{movability_from_type, Movability};
use crate::parser::{Atom, Expr, JoinSemantics, Span, Stmt};
use std::collections::{HashMap, HashSet};

/// Kind of structured-concurrency ownership violation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskOwnershipViolationKind {
    ConcurrentDoubleMove,
    MoveWhileSiblingUses,
    ConcurrentDataRace,
    UseAfterConcurrentMove,
    CancelDependentRead,
}

/// A single violation, carrying the offending capture and a human message.
/// `kind` / `variable` are the machine-readable form used by tests and by
/// future structured-feedback consumers; the pipeline itself reports `message`.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct TaskOwnershipViolation {
    pub kind: TaskOwnershipViolationKind,
    pub variable: String,
    pub message: String,
}

/// Variables captured and used by one child task of a `task_group`.
#[derive(Debug, Clone, Default)]
struct ChildUsage {
    reads: HashSet<String>,
    writes: HashSet<String>,
    moves: HashSet<String>,
}

impl ChildUsage {
    fn touches(&self, var: &str) -> bool {
        self.reads.contains(var) || self.writes.contains(var) || self.moves.contains(var)
    }
}

/// Types of the variables visible at a program point (`None` = unknown type,
/// which `movability_from_type` conservatively treats as a Move type).
type TypeEnv = HashMap<String, Option<String>>;

fn is_move_typed(var: &str, types: &TypeEnv) -> bool {
    match types.get(var) {
        Some(ty) => movability_from_type(ty) == Movability::Move,
        // Unknown variables are not tracked as owned values.
        None => false,
    }
}

/// Best-effort literal type inference, mirroring `mir::infer_hir_ty` for the
/// cases that decide `Movability`.
fn infer_expr_ty(expr: &Expr, types: &TypeEnv) -> Option<String> {
    match expr {
        Expr::Number(_) => Some("i64".to_string()),
        Expr::Float(_) => Some("f64".to_string()),
        Expr::StringLit(_) => Some("Str".to_string()),
        Expr::Variable(v) => types.get(v).cloned().flatten(),
        Expr::StructInit { type_name, .. } => Some(type_name.clone()),
        Expr::BinaryOp(lhs, _, rhs) => {
            infer_expr_ty(lhs, types).or_else(|| infer_expr_ty(rhs, types))
        }
        _ => None,
    }
}

/// Parameter positions of `callee` that are declared `consume`.
fn consumed_positions(callee: &str, module_env: &ModuleEnv) -> Vec<usize> {
    let atom = module_env.atoms.get(callee).or_else(|| {
        module_env
            .atoms
            .iter()
            .find(|(fqn, _)| {
                fqn.rsplit("::").next() == Some(callee) || fqn.rsplit('.').next() == Some(callee)
            })
            .map(|(_, a)| a)
    });
    let Some(atom) = atom else {
        return Vec::new();
    };
    atom.params
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            // `consume x;` clause form, or the `consume x: T` parameter-prefix
            // form (which the parser keeps in the parameter name).
            atom.consumed_params.contains(&p.name) || p.name.starts_with("consume ")
        })
        .map(|(i, _)| i)
        .collect()
}

// -----------------------------------------------------------------------------
// Usage collection
// -----------------------------------------------------------------------------

struct UsageCollector<'a> {
    module_env: &'a ModuleEnv,
    /// Variables declared inside the analysed task body — not captures.
    locals: HashSet<String>,
    types: TypeEnv,
    usage: ChildUsage,
}

impl<'a> UsageCollector<'a> {
    fn new(module_env: &'a ModuleEnv, types: &TypeEnv) -> Self {
        UsageCollector {
            module_env,
            locals: HashSet::new(),
            types: types.clone(),
            usage: ChildUsage::default(),
        }
    }

    fn record_read(&mut self, var: &str) {
        if !self.locals.contains(var) {
            self.usage.reads.insert(var.to_string());
        }
    }

    fn record_write(&mut self, var: &str) {
        if !self.locals.contains(var) {
            self.usage.writes.insert(var.to_string());
        }
    }

    fn record_move(&mut self, var: &str) {
        if !self.locals.contains(var) && is_move_typed(var, &self.types) {
            self.usage.moves.insert(var.to_string());
        }
    }

    fn expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Number(_) | Expr::Float(_) | Expr::StringLit(_) | Expr::AtomRef { .. } => {}
            Expr::Variable(v) => self.record_read(v),
            Expr::ArrayAccess(name, index) => {
                self.record_read(name);
                self.expr(index);
            }
            Expr::BinaryOp(lhs, _, rhs) => {
                self.expr(lhs);
                self.expr(rhs);
            }
            Expr::IfThenElse {
                cond,
                then_branch,
                else_branch,
            } => {
                self.expr(cond);
                self.stmt(then_branch);
                self.stmt(else_branch);
            }
            Expr::Call(name, args) => {
                let consumed = consumed_positions(name, self.module_env);
                for (i, arg) in args.iter().enumerate() {
                    if consumed.contains(&i) {
                        if let Expr::Variable(v) = arg {
                            self.record_read(v);
                            self.record_move(v);
                            continue;
                        }
                    }
                    self.expr(arg);
                }
            }
            Expr::StructInit { fields, .. } => {
                for (_, value) in fields {
                    self.expr(value);
                }
            }
            Expr::FieldAccess(base, _) => self.expr(base),
            Expr::Match { target, arms } => {
                self.expr(target);
                for arm in arms {
                    self.stmt(&arm.body);
                }
            }
            Expr::Async { body } | Expr::Lambda { body, .. } => self.stmt(body),
            Expr::Await { expr } => self.expr(expr),
            Expr::CallRef { callee, args } => {
                self.expr(callee);
                for arg in args {
                    self.expr(arg);
                }
            }
            Expr::Perform { args, .. } => {
                for arg in args {
                    self.expr(arg);
                }
            }
            Expr::ChanSend { channel, value } => {
                self.expr(channel);
                // Sending an owned value transfers it to the receiving task.
                if let Expr::Variable(v) = value.as_ref() {
                    self.record_read(v);
                    self.record_move(v);
                } else {
                    self.expr(value);
                }
            }
            Expr::ChanRecv { channel } => self.expr(channel),
        }
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let { var, value, .. } => {
                // `let y = x;` moves `x` when `x` is a Move-typed value.
                if let Expr::Variable(src) = value.as_ref() {
                    self.record_read(src);
                    self.record_move(src);
                } else {
                    self.expr(value);
                }
                let ty = infer_expr_ty(value, &self.types);
                self.types.insert(var.clone(), ty);
                self.locals.insert(var.clone());
            }
            Stmt::Assign { var, value, .. } => {
                self.expr(value);
                self.record_write(var);
            }
            Stmt::ArrayStore {
                array,
                index,
                value,
                ..
            } => {
                self.expr(index);
                self.expr(value);
                self.record_write(array);
            }
            Stmt::Block(stmts, _) => {
                for s in stmts {
                    self.stmt(s);
                }
            }
            Stmt::While {
                cond,
                invariant,
                decreases,
                body,
                ..
            } => {
                self.expr(cond);
                self.expr(invariant);
                if let Some(d) = decreases {
                    self.expr(d);
                }
                self.stmt(body);
            }
            Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => self.stmt(body),
            Stmt::TaskGroup { children, .. } => {
                for child in children {
                    self.stmt(child);
                }
            }
            Stmt::Cancel { .. } => {}
            Stmt::Expr(e, _) => self.expr(e),
        }
    }
}

fn collect_usage(stmt: &Stmt, module_env: &ModuleEnv, types: &TypeEnv) -> ChildUsage {
    let mut collector = UsageCollector::new(module_env, types);
    collector.stmt(stmt);
    collector.usage
}

// -----------------------------------------------------------------------------
// Group-level checks
// -----------------------------------------------------------------------------

fn check_group(
    children: &[Stmt],
    join_semantics: &JoinSemantics,
    module_env: &ModuleEnv,
    types: &TypeEnv,
    violations: &mut Vec<TaskOwnershipViolation>,
) -> ChildUsage {
    let usages: Vec<ChildUsage> = children
        .iter()
        .map(|child| collect_usage(child, module_env, types))
        .collect();

    for i in 0..usages.len() {
        for j in (i + 1)..usages.len() {
            let (a, b) = (&usages[i], &usages[j]);

            for var in a.moves.intersection(&b.moves) {
                violations.push(TaskOwnershipViolation {
                    kind: TaskOwnershipViolationKind::ConcurrentDoubleMove,
                    variable: var.clone(),
                    message: format!(
                        "captured value `{}` is moved by concurrent sibling tasks {} and {} \
                         of the same task_group (concurrent double move)",
                        var, i, j
                    ),
                });
            }

            for (mover, other, mover_idx, other_idx) in [(a, b, i, j), (b, a, j, i)] {
                for var in &mover.moves {
                    if other.moves.contains(var) {
                        continue; // already reported as a double move
                    }
                    if other.touches(var) {
                        violations.push(TaskOwnershipViolation {
                            kind: TaskOwnershipViolationKind::MoveWhileSiblingUses,
                            variable: var.clone(),
                            message: format!(
                                "captured value `{}` is moved by task {} while concurrent sibling \
                                 task {} still uses it",
                                var, mover_idx, other_idx
                            ),
                        });
                    }
                }
            }

            for (writer, other, writer_idx, other_idx) in [(a, b, i, j), (b, a, j, i)] {
                for var in &writer.writes {
                    if other.writes.contains(var) && writer_idx > other_idx {
                        continue; // report write/write once
                    }
                    if other.reads.contains(var) || other.writes.contains(var) {
                        violations.push(TaskOwnershipViolation {
                            kind: TaskOwnershipViolationKind::ConcurrentDataRace,
                            variable: var.clone(),
                            message: format!(
                                "captured variable `{}` is written by task {} and concurrently \
                                 accessed by sibling task {} without synchronisation (data race)",
                                var, writer_idx, other_idx
                            ),
                        });
                    }
                }
            }
        }
    }

    // Recurse into nested groups inside each child.
    for child in children {
        check_stmt(child, module_env, &mut types.clone(), violations);
    }

    let mut merged = ChildUsage::default();
    for usage in &usages {
        merged.reads.extend(usage.reads.iter().cloned());
        merged.writes.extend(usage.writes.iter().cloned());
        merged.moves.extend(usage.moves.iter().cloned());
    }
    if *join_semantics == JoinSemantics::Any {
        // Losing children are cancelled, so their writes are not guaranteed to
        // have happened; mark them so the parent cannot depend on them.
        merged.reads.extend(merged.writes.iter().cloned());
    }
    merged
}

/// Usage of `tracked` variables by the statements following a `task_group`,
/// stopping at the point where a variable is *revived* — reassigned to a fresh
/// value that does not read the old one. This mirrors the MIR move analysis,
/// where an assignment to a place makes it live again.
fn usage_until_revived(
    rest: &[Stmt],
    tracked: &HashSet<String>,
    module_env: &ModuleEnv,
    types: &TypeEnv,
) -> ChildUsage {
    let mut live: HashSet<String> = tracked.clone();
    let mut acc = ChildUsage::default();
    for stmt in rest {
        if live.is_empty() {
            break;
        }
        let usage = collect_usage(stmt, module_env, types);
        for var in &live {
            if usage.reads.contains(var) {
                acc.reads.insert(var.clone());
            }
            if usage.moves.contains(var) {
                acc.moves.insert(var.clone());
            }
            if usage.writes.contains(var) {
                acc.writes.insert(var.clone());
            }
        }
        // A pure reassignment (`x = <expr not reading x>`) revives `x`.
        if let Stmt::Assign { var, .. } = stmt {
            if usage.writes.contains(var) && !usage.reads.contains(var) {
                live.remove(var);
            }
        }
    }
    acc
}

/// Walk a statement, checking every `task_group` it contains and the parent's
/// use of values the group consumed.
fn check_stmt(
    stmt: &Stmt,
    module_env: &ModuleEnv,
    types: &mut TypeEnv,
    violations: &mut Vec<TaskOwnershipViolation>,
) {
    match stmt {
        Stmt::Block(stmts, _) => {
            for (idx, s) in stmts.iter().enumerate() {
                if let Stmt::TaskGroup {
                    children,
                    join_semantics,
                    ..
                } = s
                {
                    let group =
                        check_group(children, join_semantics, module_env, types, violations);
                    let rest = &stmts[idx + 1..];
                    let after = usage_until_revived(rest, &group.moves, module_env, types);
                    for var in &group.moves {
                        if after.reads.contains(var) || after.moves.contains(var) {
                            violations.push(TaskOwnershipViolation {
                                kind: TaskOwnershipViolationKind::UseAfterConcurrentMove,
                                variable: var.clone(),
                                message: format!(
                                    "value `{}` is moved into a child task and used again after \
                                     the task_group (use after concurrent move)",
                                    var
                                ),
                            });
                        }
                    }
                    if *join_semantics == JoinSemantics::Any {
                        let after_writes =
                            usage_until_revived(rest, &group.writes, module_env, types);
                        for var in &group.writes {
                            if after_writes.reads.contains(var) {
                                violations.push(TaskOwnershipViolation {
                                    kind: TaskOwnershipViolationKind::CancelDependentRead,
                                    variable: var.clone(),
                                    message: format!(
                                        "variable `{}` is written by a task_group:any child and \
                                         used after the group: a cancelled child may never have \
                                         performed the write (cancellation-dependent value)",
                                        var
                                    ),
                                });
                            }
                        }
                    }
                    continue;
                }
                if let Stmt::Let { var, value, .. } = s {
                    let ty = infer_expr_ty(value, types);
                    types.insert(var.clone(), ty);
                }
                check_stmt(s, module_env, types, violations);
            }
        }
        Stmt::TaskGroup {
            children,
            join_semantics,
            ..
        } => {
            check_group(children, join_semantics, module_env, types, violations);
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } | Stmt::While { body, .. } => {
            check_stmt(body, module_env, &mut types.clone(), violations)
        }
        Stmt::Let { var, value, .. } => {
            check_expr(value, module_env, types, violations);
            let ty = infer_expr_ty(value, types);
            types.insert(var.clone(), ty);
        }
        Stmt::Assign { value, .. } => check_expr(value, module_env, types, violations),
        Stmt::ArrayStore { index, value, .. } => {
            check_expr(index, module_env, types, violations);
            check_expr(value, module_env, types, violations);
        }
        Stmt::Expr(e, _) => check_expr(e, module_env, types, violations),
        Stmt::Cancel { .. } => {}
    }
}

fn check_expr(
    expr: &Expr,
    module_env: &ModuleEnv,
    types: &TypeEnv,
    violations: &mut Vec<TaskOwnershipViolation>,
) {
    match expr {
        Expr::IfThenElse {
            then_branch,
            else_branch,
            ..
        } => {
            check_stmt(then_branch, module_env, &mut types.clone(), violations);
            check_stmt(else_branch, module_env, &mut types.clone(), violations);
        }
        Expr::Async { body } | Expr::Lambda { body, .. } => {
            check_stmt(body, module_env, &mut types.clone(), violations)
        }
        Expr::Await { expr } => check_expr(expr, module_env, types, violations),
        Expr::Match { arms, .. } => {
            for arm in arms {
                check_stmt(&arm.body, module_env, &mut types.clone(), violations);
            }
        }
        Expr::BinaryOp(lhs, _, rhs) => {
            check_expr(lhs, module_env, types, violations);
            check_expr(rhs, module_env, types, violations);
        }
        _ => {}
    }
}

/// Analyse an atom body and return every structured-concurrency ownership
/// violation it contains.
pub(crate) fn analyze_task_ownership(
    atom: &Atom,
    body_stmt: &Stmt,
    module_env: &ModuleEnv,
) -> Vec<TaskOwnershipViolation> {
    let mut types: TypeEnv = HashMap::new();
    for param in &atom.params {
        // `ref` / `ref mut` parameters are borrowed, not owned, so they cannot
        // be moved into a task; they can still race, which the write checks
        // below cover.
        let ty = if param.is_ref || param.is_ref_mut {
            Some("i64".to_string())
        } else {
            param.type_name.clone()
        };
        types.insert(param.name.clone(), ty);
    }
    let mut violations = Vec::new();
    check_stmt(body_stmt, module_env, &mut types, &mut violations);
    violations
}

/// Hard-error wrapper used by the verification pipeline.
pub(crate) fn verify_task_ownership(
    atom: &Atom,
    body_stmt: &Stmt,
    module_env: &ModuleEnv,
) -> MumeiResult<()> {
    let violations = analyze_task_ownership(atom, body_stmt, module_env);
    if let Some(v) = violations.first() {
        return Err(MumeiError::verification_at(
            format!(
                "Structured concurrency ownership violation in atom '{}': {}",
                atom.name, v.message
            ),
            atom.span.clone(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir::lower_atom_to_hir;
    use crate::parser::{parse_module, Item};

    fn analyze(source: &str) -> Vec<TaskOwnershipViolation> {
        let items = parse_module(source);
        let mut module_env = ModuleEnv::default();
        for item in &items {
            if let Item::Atom(atom) = item {
                module_env.atoms.insert(atom.name.clone(), atom.clone());
            }
        }
        let mut violations = Vec::new();
        for item in &items {
            if let Item::Atom(atom) = item {
                let hir = lower_atom_to_hir(atom);
                violations.extend(analyze_task_ownership(atom, &hir.body_stmt, &module_env));
            }
        }
        violations
    }

    const CONSUMER: &str = r#"
atom take_buffer(buf: [i64])
requires: len(buf) >= 0;
consume buf;
ensures: result >= 0;
body: len(buf);
"#;

    #[test]
    fn shared_reads_across_siblings_are_allowed() {
        let violations = analyze(
            r#"
atom read_only(a: i64, b: i64)
requires: a >= 0 && b >= 0;
ensures: result >= 0;
body: {
    task_group:all {
        task { a + b };
        task { a + b }
    }
};
"#,
        );
        assert!(violations.is_empty(), "{violations:?}");
    }

    #[test]
    fn concurrent_write_and_read_is_a_data_race() {
        let violations = analyze(
            r#"
atom racy(n: i64)
requires: n >= 0;
ensures: result >= 0;
body: {
    let counter = n;
    task_group:all {
        task { counter = counter + 1; counter };
        task { counter }
    }
};
"#,
        );
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert_eq!(
            violations[0].kind,
            TaskOwnershipViolationKind::ConcurrentDataRace
        );
        assert_eq!(violations[0].variable, "counter");
    }

    #[test]
    fn same_capture_consumed_by_two_children_is_a_double_move() {
        let violations = analyze(&format!(
            "{CONSUMER}
atom two_owners(buf: [i64])
requires: len(buf) >= 1;
ensures: result >= 0;
body: {{
    task_group:all {{
        task {{ take_buffer(buf) }};
        task {{ take_buffer(buf) }}
    }}
}};
"
        ));
        assert!(
            violations.iter().any(
                |v| v.kind == TaskOwnershipViolationKind::ConcurrentDoubleMove
                    && v.variable == "buf"
            ),
            "{violations:?}"
        );
    }

    #[test]
    fn parent_use_after_child_move_is_rejected() {
        let violations = analyze(&format!(
            "{CONSUMER}
atom use_after(buf: [i64])
requires: len(buf) >= 1;
ensures: result >= 0;
body: {{
    task_group:all {{
        task {{ take_buffer(buf) }};
        task {{ 0 }}
    }};
    len(buf)
}};
"
        ));
        assert!(
            violations
                .iter()
                .any(|v| v.kind == TaskOwnershipViolationKind::UseAfterConcurrentMove),
            "{violations:?}"
        );
    }

    #[test]
    fn any_group_write_read_back_by_parent_is_cancellation_dependent() {
        let violations = analyze(
            r#"
atom cancel_dependent(n: i64)
requires: n >= 0;
ensures: result >= 0;
body: {
    let total = n;
    task_group:any {
        task { total = total + 1; total };
        task { n }
    };
    total
};
"#,
        );
        assert!(
            violations.iter().any(
                |v| v.kind == TaskOwnershipViolationKind::CancelDependentRead
                    && v.variable == "total"
            ),
            "{violations:?}"
        );
    }

    #[test]
    fn copy_typed_capture_moved_into_one_child_is_allowed() {
        let violations = analyze(
            r#"
atom scalar_capture(n: i64)
requires: n >= 0;
ensures: result >= 0;
body: {
    task_group:all {
        task { let m = n; m };
        task { n }
    }
};
"#,
        );
        assert!(violations.is_empty(), "{violations:?}");
    }

    #[test]
    fn reassigning_a_moved_value_after_the_group_revives_it() {
        let violations = analyze(&format!(
            "{CONSUMER}
atom make_buffer(n: i64) -> [i64]
requires: n >= 0;
ensures: len(result) >= 0;
body: [n];

atom revive_after_move(buf: [i64], n: i64)
requires: len(buf) >= 1 && n >= 0;
ensures: result >= 0;
body: {{
    task_group:all {{
        task {{ take_buffer(buf) }};
        task {{ 0 }}
    }};
    buf = make_buffer(n);
    len(buf)
}};
"
        ));
        assert!(violations.is_empty(), "{violations:?}");
    }

    #[test]
    fn overwriting_a_cancellable_write_after_the_group_is_allowed() {
        let violations = analyze(
            r#"
atom overwrite_after_any(n: i64)
requires: n >= 0;
ensures: result >= 0;
body: {
    let total = n;
    task_group:any {
        task { total = total + 1; total };
        task { n }
    };
    total = n;
    total
};
"#,
        );
        assert!(violations.is_empty(), "{violations:?}");
    }

    #[test]
    fn nested_group_violations_are_reported() {
        let violations = analyze(
            r#"
atom nested(n: i64)
requires: n >= 0;
ensures: result >= 0;
body: {
    let counter = n;
    task_group:all {
        task { n };
        task {
            task_group:all {
                task { counter = counter + 1; counter };
                task { counter = counter + 2; counter }
            }
        }
    }
};
"#,
        );
        assert!(
            violations
                .iter()
                .any(|v| v.kind == TaskOwnershipViolationKind::ConcurrentDataRace),
            "{violations:?}"
        );
    }
}
