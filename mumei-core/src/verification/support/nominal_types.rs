// =============================================================================
// Nominal struct type checking
// =============================================================================
//
// Structs are nominal types in Mumei, but everything downstream of the parser
// (Z3 translation, MIR, LLVM codegen) sees them structurally: two structs with
// the same field layout are indistinguishable there. This AST-level pass keeps
// the nominal distinction by rejecting an atom whose body passes a struct value
// of one declared type where a different struct type is expected:
//
//   * a call argument whose parameter is declared with another struct type;
//   * a `send` whose channel is declared `chan<S>` for another struct `S`;
//   * a `task_group:any` whose children yield different struct types (the
//     winner is restored through a single result type);
//   * an atom body whose result struct differs from the declared return type.
//
// Struct types are inferred where they are syntactically evident: struct
// literals, variables bound to one, calls returning one, `recv` from a typed
// channel, field reads of a known struct, and `if` / `match` whose branches
// all agree. Struct literal fields and reassignments are checked against the
// declared field / binding type; anything else is left to the structural
// pipeline.
// =============================================================================

use super::super::module_env::ModuleEnv;
use super::super::types::{MumeiError, MumeiResult};
use crate::lowering::chan_payload_type;
use crate::mir::resolve_alias_base;
use crate::parser::{Atom, Expr, JoinSemantics, Span, Stmt};
use std::collections::HashMap;

/// Declared type name of each variable in scope (`None` = unknown).
type TypeEnv = HashMap<String, Option<String>>;

struct NominalChecker<'a> {
    atom: &'a Atom,
    module_env: &'a ModuleEnv,
    types: TypeEnv,
}

impl<'a> NominalChecker<'a> {
    /// Canonical struct key for a declared type name, or `None` when the name
    /// is not a struct (primitives, refined scalars, channels, arrays, ...).
    fn struct_key(&self, type_name: &str) -> Option<String> {
        let base = type_name.split('<').next().unwrap_or(type_name).trim();
        let resolved = resolve_alias_base(self.module_env, base);
        if self.module_env.structs.contains_key(&resolved) {
            return Some(resolved);
        }
        self.module_env
            .structs
            .keys()
            .find(|fqn| {
                fqn.rsplit("::").next() == Some(resolved.as_str())
                    || fqn.rsplit('.').next() == Some(resolved.as_str())
            })
            .cloned()
    }

    fn lookup_atom(&self, callee: &str) -> Option<&'a Atom> {
        self.module_env.atoms.get(callee).or_else(|| {
            self.module_env
                .atoms
                .iter()
                .find(|(fqn, _)| {
                    fqn.rsplit("::").next() == Some(callee)
                        || fqn.rsplit('.').next() == Some(callee)
                })
                .map(|(_, a)| a)
        })
    }

    /// Declared type name of `field` on struct `key`.
    fn field_type(&self, key: &str, field: &str) -> Option<&'a str> {
        self.module_env
            .structs
            .get(key)?
            .fields
            .iter()
            .find(|f| f.name == field)
            .map(|f| f.type_name.as_str())
    }

    /// Struct type of `expr` when it is syntactically evident.
    fn struct_type_of(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::StructInit { type_name, .. } => self.struct_key(type_name),
            Expr::Variable(v) => self
                .types
                .get(v)
                .and_then(|t| t.as_deref())
                .and_then(|t| self.struct_key(t)),
            Expr::Call(name, _) => self
                .lookup_atom(name)
                .and_then(|a| a.return_type.as_deref())
                .and_then(|t| self.struct_key(t)),
            Expr::ChanRecv { channel } => self.chan_payload_struct(channel).map(|(_, key)| key),
            Expr::FieldAccess(base, field) => {
                let base_key = self.struct_type_of(base)?;
                let ty = self.field_type(&base_key, field)?;
                self.struct_key(ty)
            }
            Expr::IfThenElse {
                then_branch,
                else_branch,
                ..
            } => self.join_struct_types([then_branch.as_ref(), else_branch.as_ref()]),
            Expr::Match { arms, .. } => {
                self.join_struct_types(arms.iter().map(|arm| arm.body.as_ref()))
            }
            _ => None,
        }
    }

    /// Common struct type of every branch, or `None` when any branch is
    /// unknown or the branches disagree.
    fn join_struct_types<'s>(
        &self,
        branches: impl IntoIterator<Item = &'s Stmt>,
    ) -> Option<String> {
        let mut joined: Option<String> = None;
        for branch in branches {
            let ty = tail_expr(branch).and_then(|e| self.struct_type_of(e))?;
            match &joined {
                Some(first) if *first != ty => return None,
                Some(_) => {}
                None => joined = Some(ty),
            }
        }
        joined
    }

    /// `(declared payload type, struct key)` of a channel expression declared
    /// as `chan<S>` for some struct `S`.
    fn chan_payload_struct(&self, channel: &Expr) -> Option<(String, String)> {
        let Expr::Variable(ch) = channel else {
            return None;
        };
        let declared = self.types.get(ch).and_then(|t| t.as_deref())?;
        let payload = chan_payload_type(declared)?;
        let key = self.struct_key(&payload)?;
        Some((payload, key))
    }

    fn mismatch(&self, msg: String, span: &Span) -> MumeiError {
        MumeiError::type_error_at(
            format!(
                "Nominal struct type mismatch in atom '{}': {}",
                self.atom.name, msg
            ),
            span.clone(),
        )
    }

    fn expr(&mut self, expr: &Expr, span: &Span) -> MumeiResult<()> {
        match expr {
            Expr::Number(_)
            | Expr::Float(_)
            | Expr::StringLit(_)
            | Expr::Variable(_)
            | Expr::AtomRef { .. } => Ok(()),
            Expr::ArrayAccess(_, index) => self.expr(index, span),
            Expr::BinaryOp(lhs, _, rhs) => {
                self.expr(lhs, span)?;
                self.expr(rhs, span)
            }
            Expr::IfThenElse {
                cond,
                then_branch,
                else_branch,
            } => {
                self.expr(cond, span)?;
                self.branches_agree("if", [then_branch.as_ref(), else_branch.as_ref()], span)
            }
            Expr::Call(name, args) => {
                for arg in args {
                    self.expr(arg, span)?;
                }
                let Some(callee) = self.lookup_atom(name) else {
                    return Ok(());
                };
                for (i, (param, arg)) in callee.params.iter().zip(args).enumerate() {
                    let Some(expected) =
                        param.type_name.as_deref().and_then(|t| self.struct_key(t))
                    else {
                        continue;
                    };
                    if let Some(actual) = self.struct_type_of(arg) {
                        if actual != expected {
                            return Err(self.mismatch(
                                format!(
                                    "argument {} of '{}' expects struct '{}' but got struct '{}'",
                                    i + 1,
                                    name,
                                    expected,
                                    actual
                                ),
                                span,
                            ));
                        }
                    }
                }
                Ok(())
            }
            Expr::StructInit { type_name, fields } => {
                let key = self.struct_key(type_name);
                for (field, value) in fields {
                    self.expr(value, span)?;
                    let Some(expected) = key
                        .as_deref()
                        .and_then(|k| self.field_type(k, field))
                        .and_then(|t| self.struct_key(t))
                    else {
                        continue;
                    };
                    if let Some(actual) = self.struct_type_of(value) {
                        if actual != expected {
                            return Err(self.mismatch(
                                format!(
                                    "field '{}' of struct '{}' expects struct '{}' but got struct '{}'",
                                    field, type_name, expected, actual
                                ),
                                span,
                            ));
                        }
                    }
                }
                Ok(())
            }
            Expr::FieldAccess(base, _) => self.expr(base, span),
            Expr::Match { target, arms } => {
                self.expr(target, span)?;
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        self.expr(guard, span)?;
                    }
                }
                self.branches_agree("match", arms.iter().map(|arm| arm.body.as_ref()), span)
            }
            Expr::Async { body } | Expr::Lambda { body, .. } => self.scoped(|c| c.stmt(body)),
            Expr::Await { expr } => self.expr(expr, span),
            Expr::CallRef { callee, args } => {
                self.expr(callee, span)?;
                for arg in args {
                    self.expr(arg, span)?;
                }
                Ok(())
            }
            Expr::Perform { args, .. } => {
                for arg in args {
                    self.expr(arg, span)?;
                }
                Ok(())
            }
            Expr::ChanSend { channel, value } => {
                self.expr(value, span)?;
                let Some((payload, expected)) = self.chan_payload_struct(channel) else {
                    return Ok(());
                };
                match self.struct_type_of(value) {
                    Some(actual) if actual != expected => Err(self.mismatch(
                        format!(
                            "send on 'chan<{}>' expects struct '{}' but got struct '{}'",
                            payload, expected, actual
                        ),
                        span,
                    )),
                    _ => Ok(()),
                }
            }
            Expr::ChanRecv { channel } => self.expr(channel, span),
        }
    }

    /// Check every branch of a branching expression in its own scope and
    /// reject the expression when the branches yield different structs.
    fn branches_agree<'s>(
        &mut self,
        what: &str,
        branches: impl IntoIterator<Item = &'s Stmt>,
        span: &Span,
    ) -> MumeiResult<()> {
        let mut first: Option<String> = None;
        for branch in branches {
            let Some(ty) = self.stmt_result_type(branch)? else {
                continue;
            };
            match &first {
                Some(f) if *f != ty => {
                    return Err(self.mismatch(
                        format!("{} branches yield struct '{}' and struct '{}'", what, f, ty),
                        span,
                    ));
                }
                Some(_) => {}
                None => first = Some(ty),
            }
        }
        Ok(())
    }

    /// Run `f` in a nested lexical scope: bindings it introduces do not leak
    /// into the enclosing scope.
    fn scoped<T>(&mut self, f: impl FnOnce(&mut Self) -> MumeiResult<T>) -> MumeiResult<T> {
        let saved = self.types.clone();
        let result = f(self);
        self.types = saved;
        result
    }

    /// Check `stmt` in its own scope and return the struct type it evaluates
    /// to, seen from inside that scope (so local bindings are visible).
    fn stmt_result_type(&mut self, stmt: &Stmt) -> MumeiResult<Option<String>> {
        self.scoped(|c| {
            match stmt {
                Stmt::Block(stmts, _) => {
                    for s in stmts {
                        c.stmt(s)?;
                    }
                }
                other => c.stmt(other)?,
            }
            Ok(tail_expr(stmt).and_then(|e| c.struct_type_of(e)))
        })
    }

    fn stmt(&mut self, stmt: &Stmt) -> MumeiResult<()> {
        match stmt {
            Stmt::Let { var, value, span } => {
                self.expr(value, span)?;
                let ty = self.struct_type_of(value);
                self.types.insert(var.clone(), ty);
                Ok(())
            }
            Stmt::Assign { var, value, span } => {
                self.expr(value, span)?;
                let declared = self
                    .types
                    .get(var)
                    .and_then(|t| t.as_deref())
                    .and_then(|t| self.struct_key(t));
                let actual = self.struct_type_of(value);
                match (declared, actual) {
                    (Some(expected), Some(actual)) if expected != actual => Err(self.mismatch(
                        format!(
                            "'{}' is bound to struct '{}' but is assigned struct '{}'",
                            var, expected, actual
                        ),
                        span,
                    )),
                    (None, actual) => {
                        self.types.insert(var.clone(), actual);
                        Ok(())
                    }
                    _ => Ok(()),
                }
            }
            Stmt::ArrayStore {
                index, value, span, ..
            } => {
                self.expr(index, span)?;
                self.expr(value, span)
            }
            Stmt::Block(stmts, _) => self.scoped(|c| {
                for s in stmts {
                    c.stmt(s)?;
                }
                Ok(())
            }),
            Stmt::While {
                cond,
                invariant,
                decreases,
                body,
                span,
            } => {
                self.expr(cond, span)?;
                self.expr(invariant, span)?;
                if let Some(d) = decreases {
                    self.expr(d, span)?;
                }
                self.scoped(|c| c.stmt(body))
            }
            Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => self.scoped(|c| c.stmt(body)),
            Stmt::TaskGroup {
                children,
                join_semantics,
                span,
            } => {
                let mut winner: Option<String> = None;
                for child in children {
                    let ty = self.stmt_result_type(child)?;
                    if *join_semantics == JoinSemantics::Any {
                        let Some(ty) = ty else {
                            continue;
                        };
                        match &winner {
                            Some(first) if *first != ty => {
                                return Err(self.mismatch(
                                    format!(
                                        "task_group:any children yield struct '{}' and struct '{}'; \
                                         all children must share one result type",
                                        first, ty
                                    ),
                                    span,
                                ));
                            }
                            Some(_) => {}
                            None => winner = Some(ty),
                        }
                    }
                }
                Ok(())
            }
            Stmt::Cancel { .. } => Ok(()),
            Stmt::Expr(e, span) => self.expr(e, span),
        }
    }
}

/// The expression a statement evaluates to, if it has one.
fn tail_expr(stmt: &Stmt) -> Option<&Expr> {
    match stmt {
        Stmt::Expr(e, _) => Some(e),
        Stmt::Block(stmts, _) => stmts.last().and_then(tail_expr),
        Stmt::Task { body, .. } => tail_expr(body),
        _ => None,
    }
}

/// Reject struct values used where a differently named struct is declared.
pub(crate) fn verify_nominal_struct_types(
    atom: &Atom,
    body_stmt: &Stmt,
    module_env: &ModuleEnv,
) -> MumeiResult<()> {
    let mut types: TypeEnv = HashMap::new();
    for param in &atom.params {
        types.insert(param.name.clone(), param.type_name.clone());
    }
    let mut checker = NominalChecker {
        atom,
        module_env,
        types,
    };
    let actual = checker.stmt_result_type(body_stmt)?;

    let declared = atom
        .return_type
        .as_deref()
        .and_then(|t| checker.struct_key(t));
    if let (Some(expected), Some(actual)) = (declared, actual) {
        if expected != actual {
            return Err(checker.mismatch(
                format!(
                    "body yields struct '{}' but the atom returns struct '{}'",
                    actual, expected
                ),
                &atom.span,
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir::lower_atom_to_hir;
    use crate::parser::{parse_module, Item};

    fn check(source: &str) -> Vec<String> {
        let items = parse_module(source);
        let mut module_env = ModuleEnv::default();
        for item in &items {
            match item {
                Item::Atom(atom) => {
                    module_env.atoms.insert(atom.name.clone(), atom.clone());
                }
                Item::StructDef(s) => {
                    module_env.structs.insert(s.name.clone(), s.clone());
                }
                _ => {}
            }
        }
        let mut errors = Vec::new();
        for item in &items {
            if let Item::Atom(atom) = item {
                let hir = lower_atom_to_hir(atom);
                if let Err(e) = verify_nominal_struct_types(atom, &hir.body_stmt, &module_env) {
                    errors.push(e.to_string());
                }
            }
        }
        errors
    }

    const STRUCTS: &str = r#"
struct Point { x: i64, y: i64 }
struct Pair { a: i64, b: i64 }
"#;

    #[test]
    fn same_layout_struct_argument_is_rejected() {
        let src = format!(
            "{STRUCTS}
trusted atom getx(p: Point) -> i64
requires: true;
ensures: true;
body: {{ p.x }};

trusted atom main() -> i64
requires: true;
ensures: true;
body: {{ getx(Pair {{ a: 1, b: 2 }}) }};
"
        );
        let errors = check(&src);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].contains("argument 1 of 'getx' expects struct 'Point' but got struct 'Pair'")
        );
    }

    #[test]
    fn matching_struct_argument_and_variable_are_accepted() {
        let src = format!(
            "{STRUCTS}
trusted atom getx(p: Point) -> i64
requires: true;
ensures: true;
body: {{ p.x }};

trusted atom main() -> i64
requires: true;
ensures: true;
body: {{ let p = Point {{ x: 1, y: 2 }}; getx(p) }};
"
        );
        assert!(check(&src).is_empty());
    }

    #[test]
    fn same_layout_struct_send_is_rejected() {
        let src = format!(
            "{STRUCTS}
trusted atom relay(ch: chan<Point>, q: Pair) -> Point
requires: true;
ensures: true;
body: {{ send(ch, q); recv(ch) }};
"
        );
        let errors = check(&src);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0]
            .contains("send on 'chan<Point>' expects struct 'Point' but got struct 'Pair'"));
    }

    #[test]
    fn same_layout_task_group_any_results_are_rejected() {
        let src = format!(
            "{STRUCTS}
trusted atom pick() -> Point
requires: true;
ensures: true;
body: {{
    task_group:any {{
        task {{ Point {{ x: 3, y: 7 }} }};
        task {{ Pair {{ a: 1, b: 2 }} }}
    }}
}};
"
        );
        let errors = check(&src);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].contains("task_group:any children yield struct 'Point' and struct 'Pair'")
        );
    }

    #[test]
    fn same_layout_return_value_is_rejected() {
        let src = format!(
            "{STRUCTS}
trusted atom mk() -> Point
requires: true;
ensures: true;
body: {{ Pair {{ a: 1, b: 2 }} }};
"
        );
        let errors = check(&src);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("body yields struct 'Pair' but the atom returns struct 'Point'"));
    }

    #[test]
    fn recv_from_typed_channel_carries_its_struct_type() {
        let src = format!(
            "{STRUCTS}
trusted atom usep(p: Pair) -> i64
requires: true;
ensures: true;
body: {{ p.a }};

trusted atom pull(ch: chan<Point>) -> i64
requires: true;
ensures: true;
body: {{ let v = recv(ch); usep(v) }};
"
        );
        let errors = check(&src);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("expects struct 'Pair' but got struct 'Point'"));
    }

    #[test]
    fn nested_struct_field_of_another_nominal_type_is_rejected() {
        let src = format!(
            "{STRUCTS}
struct Wrap {{ p: Point }}

trusted atom main() -> Wrap
requires: true;
ensures: true;
body: {{ Wrap {{ p: Pair {{ a: 1, b: 2 }} }} }};
"
        );
        let errors = check(&src);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0]
            .contains("field 'p' of struct 'Wrap' expects struct 'Point' but got struct 'Pair'"));
    }

    #[test]
    fn field_access_yields_the_declared_field_struct() {
        let src = format!(
            "{STRUCTS}
struct Wrap {{ q: Pair }}

trusted atom getx(p: Point) -> i64
requires: true;
ensures: true;
body: {{ p.x }};

trusted atom main(w: Wrap) -> i64
requires: true;
ensures: true;
body: {{ getx(w.q) }};
"
        );
        let errors = check(&src);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("expects struct 'Point' but got struct 'Pair'"));
    }

    #[test]
    fn reassigning_a_same_layout_struct_of_another_type_is_rejected() {
        let src = format!(
            "{STRUCTS}
trusted atom main() -> Point
requires: true;
ensures: true;
body: {{ let p = Point {{ x: 1, y: 2 }}; p = Pair {{ a: 3, b: 4 }}; p }};
"
        );
        let errors = check(&src);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("'p' is bound to struct 'Point' but is assigned struct 'Pair'"));
    }

    #[test]
    fn if_branches_with_different_structs_are_rejected() {
        let src = format!(
            "{STRUCTS}
trusted atom main(c: i64) -> Point
requires: true;
ensures: true;
body: {{ if c > 0 {{ Point {{ x: 1, y: 2 }} }} else {{ let q = Pair {{ a: 3, b: 4 }}; q }} }};
"
        );
        let errors = check(&src);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("if branches yield struct 'Point' and struct 'Pair'"));
    }

    #[test]
    fn local_bindings_in_a_branch_do_not_leak() {
        let src = format!(
            "{STRUCTS}
trusted atom getx(p: Point) -> i64
requires: true;
ensures: true;
body: {{ p.x }};

trusted atom main(c: i64, p: Point) -> i64
requires: true;
ensures: true;
body: {{ if c > 0 {{ let p = Pair {{ a: 1, b: 2 }}; p.a }} else {{ 0 }}; getx(p) }};
"
        );
        assert!(check(&src).is_empty(), "{:?}", check(&src));
    }

    #[test]
    fn locally_bound_struct_is_checked_against_the_return_type() {
        let src = format!(
            "{STRUCTS}
trusted atom main() -> Point
requires: true;
ensures: true;
body: {{ let q = Pair {{ a: 1, b: 2 }}; q }};
"
        );
        let errors = check(&src);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("body yields struct 'Pair' but the atom returns struct 'Point'"));
    }
}
