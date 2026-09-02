//! Units of measure: a type-level unit tag on refined type aliases
//! (`type Usd = i64 unit USD;`) that is checked for consistency on `+`, `-`,
//! comparisons, assignments, call arguments and the returned value.
//!
//! The unit axis is orthogonal to refinement predicates and is purely static:
//! it never changes a value's Z3 sort (values are still passed to the solver as
//! `Int`/`Real`), the MIR, or generated code. The check runs on the AST before
//! any solver phase, so a mismatch is reported as a `TypeError` with no proof
//! attempt.
//!
//! Compatibility rules (minimal subset):
//! - two operands with *different* unit tags are a mismatch;
//! - a value without a unit tag (plain `i64`, literals, `*`/`/` results, calls
//!   into atoms without a unit-tagged return type) is compatible with any unit,
//!   so unit-free code is unaffected;
//! - `*`, `/`, `^` do not compose units: the result carries a unit only when
//!   exactly one operand does (scalar scaling), otherwise it is unit-free.

use super::call_graph::expr_to_source_string;
use crate::parser::{parse_expression, Atom, Expr, Op, Stmt};
use crate::verification::module_env::ModuleEnv;
use crate::verification::types::{MumeiError, MumeiResult};
use std::collections::HashMap;

/// Unit tag attached to a value, or `None` when the value is unit-free.
type Unit = Option<String>;

/// What the unit checker knows about a value: its unit tag and, for struct
/// values, the struct type so that field accesses can be resolved.
#[derive(Debug, Clone, Default, PartialEq)]
struct Ty {
    unit: Unit,
    struct_name: Option<String>,
}

impl Ty {
    fn unit(unit: Unit) -> Self {
        Ty {
            unit,
            struct_name: None,
        }
    }
}

struct UnitCtx<'a> {
    atom: &'a Atom,
    module_env: &'a ModuleEnv,
    /// Variable name -> known type information (unit tag and/or struct type).
    vars: HashMap<String, Ty>,
}

impl<'a> UnitCtx<'a> {
    fn unit_of_type(&self, type_name: &str) -> Unit {
        self.module_env
            .get_type(type_name)
            .and_then(|refined| refined.unit.clone())
    }

    /// Type information for a value declared with type `type_name`.
    fn ty_of_type(&self, type_name: &str) -> Ty {
        Ty {
            unit: self.unit_of_type(type_name),
            struct_name: self
                .module_env
                .get_struct(type_name)
                .map(|_| type_name.to_string()),
        }
    }

    fn field_ty(&self, struct_name: &str, field: &str) -> Ty {
        self.module_env
            .get_struct(struct_name)
            .and_then(|s| s.fields.iter().find(|f| f.name == field))
            .map(|f| self.ty_of_type(&f.type_name))
            .unwrap_or_default()
    }

    fn mismatch(&self, what: &str, lhs: &Unit, rhs: &Unit, expr_src: String) -> MumeiError {
        let show = |u: &Unit| u.clone().unwrap_or_else(|| "<unitless>".to_string());
        MumeiError::type_error_at(
            format!(
                "Unit mismatch in atom '{}': {} combines '{}' with '{}' in `{}`",
                self.atom.name,
                what,
                show(lhs),
                show(rhs),
                expr_src
            ),
            self.atom.span.clone(),
        )
        .with_help(
            "Values with different units of measure cannot be added, subtracted or compared. \
             Convert one side explicitly (through an atom returning the other unit) or fix the \
             type annotations."
                .to_string(),
        )
    }

    fn check_compatible(
        &self,
        what: &str,
        lhs: &Unit,
        rhs: &Unit,
        src: impl FnOnce() -> String,
    ) -> MumeiResult<()> {
        match (lhs, rhs) {
            (Some(l), Some(r)) if l != r => Err(self.mismatch(what, lhs, rhs, src())),
            _ => Ok(()),
        }
    }

    /// Merge the types of two branches that produce the same value (`if`/`else`,
    /// `match` arms): units must be compatible and known struct types must agree.
    fn join_branches(
        &self,
        what: &str,
        lhs: Ty,
        rhs: Ty,
        src: impl FnOnce() -> String,
    ) -> MumeiResult<Ty> {
        if let (Some(l), Some(r)) = (&lhs.unit, &rhs.unit) {
            if l != r {
                return Err(self.mismatch(what, &lhs.unit, &rhs.unit, src()));
            }
        }
        let struct_name = match (lhs.struct_name, rhs.struct_name) {
            (Some(l), Some(r)) if l != r => {
                return Err(MumeiError::type_error_at(
                    format!(
                        "Type mismatch in atom '{}': {} produce struct '{}' and struct '{}' in `{}`",
                        self.atom.name,
                        what,
                        l,
                        r,
                        src()
                    ),
                    self.atom.span.clone(),
                ));
            }
            (l, r) => l.or(r),
        };
        Ok(Ty {
            unit: lhs.unit.or(rhs.unit),
            struct_name,
        })
    }

    /// Infer the unit of `expr`, checking every `+`, `-`, comparison and call
    /// argument on the way.
    fn infer(&self, expr: &Expr) -> MumeiResult<Ty> {
        match expr {
            Expr::Number(_) | Expr::Float(_) | Expr::StringLit(_) => Ok(Ty::default()),
            Expr::Variable(name) => Ok(self.vars.get(name).cloned().unwrap_or_default()),
            Expr::ArrayAccess(_, idx) => {
                self.infer(idx)?;
                Ok(Ty::default())
            }
            Expr::BinaryOp(lhs, op, rhs) => {
                let l = self.infer(lhs)?.unit;
                let r = self.infer(rhs)?.unit;
                Ok(Ty::unit(match op {
                    Op::Add | Op::Sub => {
                        self.check_compatible(
                            if *op == Op::Add {
                                "addition"
                            } else {
                                "subtraction"
                            },
                            &l,
                            &r,
                            || expr_to_source_string(expr),
                        )?;
                        l.or(r)
                    }
                    Op::Eq | Op::Neq | Op::Gt | Op::Lt | Op::Ge | Op::Le => {
                        self.check_compatible("comparison", &l, &r, || {
                            expr_to_source_string(expr)
                        })?;
                        None
                    }
                    Op::Mul | Op::Div | Op::Pow => match (l, r) {
                        (Some(u), None) | (None, Some(u)) => Some(u),
                        _ => None,
                    },
                    Op::And
                    | Op::Or
                    | Op::Implies
                    | Op::BitAnd
                    | Op::BitOr
                    | Op::BitXor
                    | Op::Shl
                    | Op::Shr => None,
                }))
            }
            Expr::IfThenElse {
                cond,
                then_branch,
                else_branch,
            } => {
                self.infer(cond)?;
                let t = self.check_stmt(then_branch)?;
                let e = self.check_stmt(else_branch)?;
                self.join_branches("conditional branches", t, e, || expr_to_source_string(expr))
            }
            Expr::Call(name, args) => {
                let arg_units = args
                    .iter()
                    .map(|a| self.infer(a).map(|t| t.unit))
                    .collect::<MumeiResult<Vec<Unit>>>()?;
                let fqn_name = name.replace('.', "::");
                let Some(callee) = self
                    .module_env
                    .get_atom(name)
                    .or_else(|| self.module_env.get_atom(&fqn_name))
                else {
                    return Ok(Ty::default());
                };
                for (param, arg_unit) in callee.params.iter().zip(arg_units.iter()) {
                    let param_unit = param
                        .type_name
                        .as_deref()
                        .and_then(|t| self.unit_of_type(t));
                    self.check_compatible(
                        &format!("argument '{}' of call to '{}'", param.name, name),
                        &param_unit,
                        arg_unit,
                        || expr_to_source_string(expr),
                    )?;
                }
                Ok(callee
                    .return_type
                    .as_deref()
                    .map(|t| self.ty_of_type(t))
                    .unwrap_or_default())
            }
            Expr::StructInit { type_name, fields } => {
                let sdef = self.module_env.get_struct(type_name);
                for (field_name, value) in fields {
                    let value_unit = self.infer(value)?.unit;
                    let field_unit = sdef
                        .and_then(|s| s.fields.iter().find(|f| &f.name == field_name))
                        .and_then(|f| self.unit_of_type(&f.type_name));
                    self.check_compatible(
                        &format!("field '{}' of struct '{}'", field_name, type_name),
                        &field_unit,
                        &value_unit,
                        || expr_to_source_string(expr),
                    )?;
                }
                Ok(Ty {
                    unit: None,
                    struct_name: Some(type_name.clone()),
                })
            }
            Expr::FieldAccess(target, field) => {
                let target_ty = self.infer(target)?;
                Ok(target_ty
                    .struct_name
                    .map(|s| self.field_ty(&s, field))
                    .unwrap_or_default())
            }
            Expr::Match { target, arms } => {
                self.infer(target)?;
                let mut ty = Ty::default();
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        self.infer(guard)?;
                    }
                    let arm_ty = self.check_stmt(&arm.body)?;
                    ty = self
                        .join_branches("match arms", ty, arm_ty, || expr_to_source_string(expr))?;
                }
                Ok(ty)
            }
            Expr::Async { body } => self.check_stmt(body),
            Expr::Await { expr } => self.infer(expr),
            Expr::CallRef { callee, args } => {
                self.infer(callee)?;
                for a in args {
                    self.infer(a)?;
                }
                Ok(Ty::default())
            }
            Expr::Perform { args, .. } => {
                for a in args {
                    self.infer(a)?;
                }
                Ok(Ty::default())
            }
            Expr::Lambda { .. } | Expr::AtomRef { .. } => Ok(Ty::default()),
            Expr::ChanSend { channel, value } => {
                self.infer(channel)?;
                self.infer(value)?;
                Ok(Ty::default())
            }
            Expr::ChanRecv { channel } => {
                self.infer(channel)?;
                Ok(Ty::default())
            }
        }
    }

    /// Check a statement and return the unit of its value (the trailing
    /// expression of a block, or the value of a bare expression statement).
    /// `let` bindings are recorded so later uses carry the inferred unit; the
    /// binding is scoped to the enclosing block.
    fn check_stmt(&self, stmt: &Stmt) -> MumeiResult<Ty> {
        let mut scoped = UnitCtx {
            atom: self.atom,
            module_env: self.module_env,
            vars: self.vars.clone(),
        };
        scoped.check_stmt_in_scope(stmt)
    }

    fn check_stmt_in_scope(&mut self, stmt: &Stmt) -> MumeiResult<Ty> {
        match stmt {
            Stmt::Let { var, value, .. } => {
                let ty = self.infer(value)?;
                if ty == Ty::default() {
                    self.vars.remove(var);
                } else {
                    self.vars.insert(var.clone(), ty);
                }
                Ok(Ty::default())
            }
            Stmt::Assign { var, value, span } => {
                let value_unit = self.infer(value)?.unit;
                let var_unit = self.vars.get(var).and_then(|t| t.unit.clone());
                self.check_compatible(
                    &format!("assignment to '{}'", var),
                    &var_unit,
                    &value_unit,
                    || format!("{} = {} (at {})", var, expr_to_source_string(value), span),
                )?;
                Ok(Ty::default())
            }
            Stmt::ArrayStore { index, value, .. } => {
                self.infer(index)?;
                self.infer(value)?;
                Ok(Ty::default())
            }
            Stmt::Block(stmts, _) => {
                let mut last = Ty::default();
                for s in stmts {
                    last = self.check_stmt_in_scope(s)?;
                }
                Ok(last)
            }
            Stmt::While {
                cond,
                invariant,
                decreases,
                body,
                ..
            } => {
                self.infer(cond)?;
                self.infer(invariant)?;
                if let Some(d) = decreases {
                    self.infer(d)?;
                }
                self.check_stmt_in_scope(body)?;
                Ok(Ty::default())
            }
            Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
                self.check_stmt_in_scope(body)?;
                Ok(Ty::default())
            }
            Stmt::TaskGroup { children, .. } => {
                for c in children {
                    self.check_stmt_in_scope(c)?;
                }
                Ok(Ty::default())
            }
            Stmt::Cancel { .. } => Ok(Ty::default()),
            Stmt::Expr(expr, _) => self.infer(expr),
        }
    }
}

fn is_trivial_clause(clause: &str) -> bool {
    let trimmed = clause.trim();
    trimmed.is_empty() || trimmed == "true"
}

/// Reject unit-of-measure mismatches in `atom`'s contract and body.
///
/// Parameters typed with a unit-tagged alias seed the environment; `result`
/// carries the unit of the declared return type. Returns a `TypeError` for the
/// first mismatch found, otherwise `Ok(())`. Atoms that mention no unit-tagged
/// type anywhere are unaffected.
pub fn verify_unit_consistency(
    atom: &Atom,
    body_stmt: &Stmt,
    module_env: &ModuleEnv,
) -> MumeiResult<()> {
    if module_env.types.values().all(|t| t.unit.is_none()) {
        return Ok(());
    }

    let mut ctx = UnitCtx {
        atom,
        module_env,
        vars: HashMap::new(),
    };
    for param in &atom.params {
        if let Some(ty) = param.type_name.as_deref().map(|t| ctx.ty_of_type(t)) {
            ctx.vars.insert(param.name.clone(), ty);
        }
    }
    let result_ty = atom
        .return_type
        .as_deref()
        .map(|t| ctx.ty_of_type(t))
        .unwrap_or_default();

    if !is_trivial_clause(&atom.requires) {
        ctx.infer(&parse_expression(&atom.requires))?;
    }

    let body_ty = ctx.check_stmt(body_stmt)?;
    ctx.check_compatible("returned value", &result_ty.unit, &body_ty.unit, || {
        format!(
            "body of '{}' (declared return type {})",
            atom.name,
            atom.return_type.as_deref().unwrap_or("i64")
        )
    })?;

    ctx.vars.insert("result".to_string(), result_ty);
    if !is_trivial_clause(&atom.ensures) {
        ctx.infer(&parse_expression(&atom.ensures))?;
    }
    Ok(())
}
