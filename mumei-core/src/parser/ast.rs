// =============================================================================
// AST type definitions
// =============================================================================
//
// All AST types are defined directly in this file and re-exported from mod.rs.
// This file contains all the core data structures used by the parser.

use crate::ast::TypeRef;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// --- 0. Source position information (Span) ---

/// Source position information. Attached to all AST nodes for diagnostic accuracy.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Span {
    pub file: String,
    pub line: usize,
    pub col: usize,
    pub len: usize,
}

impl Span {
    pub fn new(file: impl Into<String>, line: usize, col: usize, len: usize) -> Self {
        Span {
            file: file.into(),
            line,
            col,
            len,
        }
    }
}

impl std::fmt::Display for Span {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.file.is_empty() {
            write!(f, "<unknown>:{}:{}", self.line, self.col)
        } else {
            write!(f, "{}:{}:{}", self.file, self.line, self.col)
        }
    }
}

// --- 1. Expression AST ---

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Pow,
    Div,
    /// Remainder (`%`), truncated toward zero like Rust/Go/C/TypeScript/Solidity
    /// (the result takes the sign of the dividend); not Python's floored `%`.
    Rem,
    Eq,
    Neq,
    Gt,
    Lt,
    Ge,
    Le,
    And,
    Or,
    Implies,
    /// Bitwise AND (`&`). Verified with real bit semantics only under
    /// `--bitvec-i64`; see `docs/SPEC_GUIDE.md`.
    BitAnd,
    /// Bitwise OR (`|`).
    BitOr,
    /// Bitwise XOR (`^`).
    BitXor,
    /// Left shift (`<<`). Requires `0 <= n < 64`.
    Shl,
    /// Arithmetic (sign-propagating) right shift (`>>`). Requires `0 <= n < 64`.
    Shr,
}

// =============================================================================
// Async + Resource Management
// =============================================================================

#[derive(Debug, Clone)]
pub struct ResourceDef {
    pub name: String,
    pub state: Vec<ResourceField>,
    pub priority: i64,
    pub mode: ResourceMode,
    pub invariant: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ResourceField {
    pub name: String,
    pub ty: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResourceMode {
    Exclusive,
    Shared,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Effect {
    pub name: String,
    pub params: Vec<EffectParam>,
    pub span: Span,
    /// Plan 6: Negative effects — when true, this effect is negated (e.g., `!IO`)
    pub negated: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EffectParam {
    pub value: String,
    pub refinement: Option<String>,
    pub is_constant: bool,
}

impl Effect {
    // NOTE: Effect::simple is infrastructure for future effect construction in tests and MCP tools
    #[allow(dead_code)]
    pub fn simple(name: &str) -> Self {
        Effect {
            name: name.to_string(),
            params: vec![],
            span: Span::default(),
            negated: false,
        }
    }
}

// NOTE: EffectDef fields are read during effect hierarchy resolution and MCP get_inferred_effects
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct EffectDef {
    pub name: String,
    pub params: Vec<EffectDefParam>,
    pub constraint: Option<String>,
    pub includes: Vec<String>,
    pub refinement: Option<String>,
    /// Plan 6: Multi-parent support — `parent: [Network, Encrypted]`
    pub parent: Vec<String>,
    pub span: Span,
    // Stateful effect fields (Task 3: Temporal Effect Verification)
    pub states: Vec<String>,
    pub transitions: Vec<EffectTransition>,
    pub initial_state: Option<String>,
}

/// capability 宣言: `type FileCap = capability SafeFileRead(path: Str) where C;`
///
/// 新しいエフェクトを定義せず、既存 `EffectDef` (`effect_name`) に別名と
/// constraint を与える view として扱う。
#[derive(Debug, Clone)]
pub struct CapabilityDef {
    /// capability 型名（例: "FileCap"）
    pub name: String,
    /// 裏側の既存エフェクト名（例: "SafeFileRead"）
    pub effect_name: String,
    pub params: Vec<EffectDefParam>,
    pub constraint: Option<String>,
    pub span: Span,
}

/// A state transition rule for a stateful effect.
/// Represents: `transition operation: FromState -> ToState;`
#[derive(Debug, Clone)]
pub struct EffectTransition {
    pub operation: String,
    pub from_state: String,
    pub to_state: String,
}

// NOTE: EffectDefParam fields are read during effect constraint resolution (future Z3 String Sort integration)
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct EffectDefParam {
    pub name: String,
    pub type_name: String,
}

#[derive(Debug, Clone)]
pub struct LambdaParam {
    pub name: String,
    pub type_ref: Option<TypeRef>,
}

#[derive(Debug, Clone)]
pub enum Expr {
    Number(i64),
    Float(f64),
    /// Plan 9: First-class string literal
    StringLit(String),
    Variable(String),
    /// Array literal: `[e0, e1, …]` — element type is inferred from the first
    /// element; an empty literal is a parse error (no annotation syntax).
    ArrayLit(Vec<Expr>),
    ArrayAccess(String, Box<Expr>),
    BinaryOp(Box<Expr>, Op, Box<Expr>),
    /// Parser-internal explicit grouping marker. Expression parsing strips it
    /// while normalizing comparison chains; it is not a runtime expression.
    Paren(Box<Expr>),
    Block(Box<Stmt>),
    IfThenElse {
        cond: Box<Expr>,
        then_branch: Box<Stmt>,
        else_branch: Box<Stmt>,
    },
    Call(String, Vec<Expr>),
    StructInit {
        type_name: String,
        fields: Vec<(String, Expr)>,
    },
    FieldAccess(Box<Expr>, String),
    Match {
        target: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    Async {
        body: Box<Stmt>,
    },
    Await {
        expr: Box<Expr>,
    },
    AtomRef {
        name: String,
    },
    CallRef {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    Perform {
        effect: String,
        operation: String,
        args: Vec<Expr>,
    },
    /// Lambda 式: |params| body or |params| -> RetType { body }
    Lambda {
        params: Vec<LambdaParam>,
        return_type: Option<String>,
        body: Box<Stmt>,
    },
    // Plan 8: Channel send expression — `send(ch, value)`
    // NOTE: ChanSend is constructed by the parser when `send` keyword is encountered.
    #[allow(dead_code)]
    ChanSend {
        channel: Box<Expr>,
        value: Box<Expr>,
    },
    // Plan 8: Channel receive expression — `recv(ch)`
    // NOTE: ChanRecv is constructed by the parser when `recv` keyword is encountered.
    #[allow(dead_code)]
    ChanRecv {
        channel: Box<Expr>,
    },
}

impl Expr {
    /// True when evaluating this expression can bind or rebind `name`
    /// through an embedded statement (`{ let name = …; }`,
    /// `{ name = …; }`, `{ name.f = …; }`, including under a branch,
    /// loop, or a lambda the evaluation may call). Conservative —
    /// meant for fail-closed admission gates.
    pub fn binds_name(&self, name: &str) -> bool {
        match self {
            Expr::Number(_)
            | Expr::Float(_)
            | Expr::StringLit(_)
            | Expr::Variable(_)
            | Expr::AtomRef { .. } => false,
            Expr::ArrayLit(elements) => elements.iter().any(|e| e.binds_name(name)),
            Expr::ArrayAccess(_, index) => index.binds_name(name),
            Expr::BinaryOp(left, _, right) => left.binds_name(name) || right.binds_name(name),
            Expr::Paren(inner) => inner.binds_name(name),
            Expr::Block(stmt) => stmt.binds_name(name),
            Expr::IfThenElse {
                cond,
                then_branch,
                else_branch,
            } => {
                cond.binds_name(name)
                    || then_branch.binds_name(name)
                    || else_branch.binds_name(name)
            }
            Expr::Call(_, args) => args.iter().any(|a| a.binds_name(name)),
            Expr::StructInit { fields, .. } => fields.iter().any(|(_, e)| e.binds_name(name)),
            Expr::FieldAccess(inner, _) => inner.binds_name(name),
            Expr::Match { target, arms } => {
                target.binds_name(name)
                    || arms.iter().any(|arm| {
                        arm.guard.as_ref().is_some_and(|g| g.binds_name(name))
                            || arm.body.binds_name(name)
                    })
            }
            Expr::Async { body } | Expr::Lambda { body, .. } => body.binds_name(name),
            Expr::Await { expr } => expr.binds_name(name),
            Expr::CallRef { callee, args } => {
                callee.binds_name(name) || args.iter().any(|a| a.binds_name(name))
            }
            Expr::Perform { args, .. } => args.iter().any(|a| a.binds_name(name)),
            Expr::ChanSend { channel, value } => channel.binds_name(name) || value.binds_name(name),
            Expr::ChanRecv { channel } => channel.binds_name(name),
        }
    }

    /// Collect the field names written on `base` by `base.f = …`
    /// statements embedded in this expression (blocks, branches, loops,
    /// lambdas). Conservative — meant for fail-closed gates.
    pub fn written_fields(&self, base: &str, out: &mut std::collections::HashSet<String>) {
        match self {
            Expr::Number(_)
            | Expr::Float(_)
            | Expr::StringLit(_)
            | Expr::Variable(_)
            | Expr::AtomRef { .. } => {}
            Expr::ArrayLit(elements) => {
                for e in elements {
                    e.written_fields(base, out);
                }
            }
            Expr::ArrayAccess(_, index) => index.written_fields(base, out),
            Expr::BinaryOp(left, _, right) => {
                left.written_fields(base, out);
                right.written_fields(base, out);
            }
            Expr::Paren(inner) => inner.written_fields(base, out),
            Expr::Block(stmt) => stmt.written_fields(base, out),
            Expr::IfThenElse {
                cond,
                then_branch,
                else_branch,
            } => {
                cond.written_fields(base, out);
                then_branch.written_fields(base, out);
                else_branch.written_fields(base, out);
            }
            Expr::Call(_, args) => {
                for a in args {
                    a.written_fields(base, out);
                }
            }
            Expr::StructInit { fields, .. } => {
                for (_, e) in fields {
                    e.written_fields(base, out);
                }
            }
            Expr::FieldAccess(inner, _) => inner.written_fields(base, out),
            Expr::Match { target, arms } => {
                target.written_fields(base, out);
                for arm in arms {
                    if let Some(g) = &arm.guard {
                        g.written_fields(base, out);
                    }
                    arm.body.written_fields(base, out);
                }
            }
            Expr::Async { body } | Expr::Lambda { body, .. } => body.written_fields(base, out),
            Expr::Await { expr } => expr.written_fields(base, out),
            Expr::CallRef { callee, args } => {
                callee.written_fields(base, out);
                for a in args {
                    a.written_fields(base, out);
                }
            }
            Expr::Perform { args, .. } => {
                for a in args {
                    a.written_fields(base, out);
                }
            }
            Expr::ChanSend { channel, value } => {
                channel.written_fields(base, out);
                value.written_fields(base, out);
            }
            Expr::ChanRecv { channel } => channel.written_fields(base, out),
        }
    }

    /// True when evaluating this expression can rebind `name` as a whole
    /// through an embedded `name = …` statement (a `let name` shadows
    /// locally and does not count). Conservative — for fail-closed gates.
    pub fn rebinds_name(&self, name: &str) -> bool {
        match self {
            Expr::Number(_)
            | Expr::Float(_)
            | Expr::StringLit(_)
            | Expr::Variable(_)
            | Expr::AtomRef { .. } => false,
            Expr::ArrayLit(elements) => elements.iter().any(|e| e.rebinds_name(name)),
            Expr::ArrayAccess(_, index) => index.rebinds_name(name),
            Expr::BinaryOp(left, _, right) => left.rebinds_name(name) || right.rebinds_name(name),
            Expr::Paren(inner) => inner.rebinds_name(name),
            Expr::Block(stmt) => stmt.rebinds_name(name),
            Expr::IfThenElse {
                cond,
                then_branch,
                else_branch,
            } => {
                cond.rebinds_name(name)
                    || then_branch.rebinds_name(name)
                    || else_branch.rebinds_name(name)
            }
            Expr::Call(_, args) => args.iter().any(|a| a.rebinds_name(name)),
            Expr::StructInit { fields, .. } => fields.iter().any(|(_, e)| e.rebinds_name(name)),
            Expr::FieldAccess(inner, _) => inner.rebinds_name(name),
            Expr::Match { target, arms } => {
                target.rebinds_name(name)
                    || arms.iter().any(|arm| {
                        arm.guard.as_ref().is_some_and(|g| g.rebinds_name(name))
                            || arm.body.rebinds_name(name)
                    })
            }
            Expr::Async { body } | Expr::Lambda { body, .. } => body.rebinds_name(name),
            Expr::Await { expr } => expr.rebinds_name(name),
            Expr::CallRef { callee, args } => {
                callee.rebinds_name(name) || args.iter().any(|a| a.rebinds_name(name))
            }
            Expr::Perform { args, .. } => args.iter().any(|a| a.rebinds_name(name)),
            Expr::ChanSend { channel, value } => {
                channel.rebinds_name(name) || value.rebinds_name(name)
            }
            Expr::ChanRecv { channel } => channel.rebinds_name(name),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Let {
        var: String,
        value: Box<Expr>,
        span: Span,
    },
    Assign {
        var: String,
        value: Box<Expr>,
        span: Span,
    },
    /// Array element or struct-field store: `name[idx][.field] = value`.
    /// Semantics: Z3 `Array::store(arr, idx, value)` — produces a new logical
    /// array where position `idx` now holds `value`.
    ArrayStore {
        array: String,
        index: Box<Expr>,
        field_path: Option<Vec<String>>,
        value: Box<Expr>,
        span: Span,
    },
    Block(Vec<Stmt>, Span),
    While {
        cond: Box<Expr>,
        invariant: Box<Expr>,
        decreases: Option<Box<Expr>>,
        body: Box<Stmt>,
        span: Span,
    },
    Acquire {
        resource: String,
        body: Box<Stmt>,
        span: Span,
    },
    Task {
        body: Box<Stmt>,
        group: Option<String>,
        span: Span,
    },
    TaskGroup {
        children: Vec<Stmt>,
        join_semantics: JoinSemantics,
        span: Span,
    },
    // Plan 8: Cancel statement — `cancel task_group_name;`
    // NOTE: Cancel is constructed by the parser when `cancel` keyword is encountered
    // but not yet used in verification/codegen pipelines.
    #[allow(dead_code)]
    Cancel {
        target: String,
        span: Span,
    },
    Expr(Expr, Span),
}

#[derive(Debug, Clone)]
pub(crate) struct ArrayStoreTarget {
    pub(crate) field_path: Option<Vec<String>>,
    pub(crate) index: Expr,
    pub(crate) may_repeat: bool,
}

impl Stmt {
    /// True when executing this statement can bind or rebind `name`: a
    /// `name`/`name.field` assignment, an array store through `name`, a
    /// `let name` shadow, or any of those inside a nested block, branch,
    /// loop or lambda body (a
    /// lambda's writes only fire if the lambda is later called, so this
    /// is conservative — it is meant for fail-closed admission gates).
    pub fn binds_name(&self, name: &str) -> bool {
        match self {
            Stmt::Let { var, value, .. } => var == name || value.binds_name(name),
            Stmt::Assign { var, value, .. } => {
                var == name || var.starts_with(&format!("{name}.")) || value.binds_name(name)
            }
            Stmt::ArrayStore {
                array,
                index,
                value,
                ..
            } => array == name || index.binds_name(name) || value.binds_name(name),
            Stmt::Block(stmts, _) => stmts.iter().any(|s| s.binds_name(name)),
            Stmt::While {
                cond,
                invariant,
                decreases,
                body,
                ..
            } => {
                cond.binds_name(name)
                    || invariant.binds_name(name)
                    || decreases.as_ref().is_some_and(|d| d.binds_name(name))
                    || body.binds_name(name)
            }
            Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => body.binds_name(name),
            Stmt::TaskGroup { children, .. } => children.iter().any(|c| c.binds_name(name)),
            Stmt::Cancel { .. } => false,
            Stmt::Expr(expr, _) => expr.binds_name(name),
        }
    }

    /// Collect fields written on `base` by field and struct-array stores
    /// under this statement (nested blocks, branches, loops, lambda bodies).
    pub fn written_fields(&self, base: &str, out: &mut std::collections::HashSet<String>) {
        match self {
            Stmt::Let { value, .. } => value.written_fields(base, out),
            Stmt::Assign { var, value, .. } => {
                if let Some((b, field)) = var.split_once('.') {
                    if b == base {
                        out.insert(field.to_string());
                    }
                }
                value.written_fields(base, out);
            }
            Stmt::ArrayStore {
                array,
                index,
                field_path,
                value,
                ..
            } => {
                if array == base {
                    out.insert(
                        field_path
                            .as_ref()
                            .map_or_else(|| "*".to_string(), |path| path.join("_")),
                    );
                }
                index.written_fields(base, out);
                value.written_fields(base, out);
            }
            Stmt::Block(stmts, _) => {
                for s in stmts {
                    s.written_fields(base, out);
                }
            }
            Stmt::While {
                cond,
                invariant,
                decreases,
                body,
                ..
            } => {
                cond.written_fields(base, out);
                invariant.written_fields(base, out);
                if let Some(d) = decreases {
                    d.written_fields(base, out);
                }
                body.written_fields(base, out);
            }
            Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => body.written_fields(base, out),
            Stmt::TaskGroup { children, .. } => {
                for c in children {
                    c.written_fields(base, out);
                }
            }
            Stmt::Cancel { .. } => {}
            Stmt::Expr(expr, _) => expr.written_fields(base, out),
        }
    }

    pub(crate) fn array_store_targets(&self, base: &str) -> Vec<ArrayStoreTarget> {
        let mut targets = Vec::new();
        collect_stmt_array_store_targets(self, base, false, &mut targets);
        targets
    }

    /// True when executing this statement can rebind `name` as a whole
    /// (`name = …`, including inside nested control flow). A `let name`
    /// shadows locally and does not count, nor does `name.f = …`.
    pub fn rebinds_name(&self, name: &str) -> bool {
        match self {
            // A `let` re-binding `name` replaces the binding wholesale —
            // the admission gate only allows it for a same-type struct
            // re-init, which is a whole-aggregate rebind for
            // spec-validation purposes.
            Stmt::Let { var, value, .. } => var == name || value.rebinds_name(name),
            Stmt::Assign { var, value, .. } => var == name || value.rebinds_name(name),
            Stmt::ArrayStore { index, value, .. } => {
                index.rebinds_name(name) || value.rebinds_name(name)
            }
            Stmt::Block(stmts, _) => stmts.iter().any(|s| s.rebinds_name(name)),
            Stmt::While {
                cond,
                invariant,
                decreases,
                body,
                ..
            } => {
                cond.rebinds_name(name)
                    || invariant.rebinds_name(name)
                    || decreases.as_ref().is_some_and(|d| d.rebinds_name(name))
                    || body.rebinds_name(name)
            }
            Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => body.rebinds_name(name),
            Stmt::TaskGroup { children, .. } => children.iter().any(|c| c.rebinds_name(name)),
            Stmt::Cancel { .. } => false,
            Stmt::Expr(expr, _) => expr.rebinds_name(name),
        }
    }

    /// Get the Span associated with this statement.
    // NOTE: span() is infrastructure for future diagnostic passes that need statement-level source locations.
    #[allow(dead_code)]
    pub fn span(&self) -> &Span {
        match self {
            Stmt::Let { span, .. }
            | Stmt::Assign { span, .. }
            | Stmt::ArrayStore { span, .. }
            | Stmt::While { span, .. }
            | Stmt::Acquire { span, .. }
            | Stmt::Task { span, .. }
            | Stmt::TaskGroup { span, .. }
            | Stmt::Cancel { span, .. } => span,
            Stmt::Block(_, span) | Stmt::Expr(_, span) => span,
        }
    }
}

fn collect_expr_array_store_targets(
    expr: &Expr,
    base: &str,
    may_repeat: bool,
    targets: &mut Vec<ArrayStoreTarget>,
) {
    match expr {
        Expr::Number(_)
        | Expr::Float(_)
        | Expr::StringLit(_)
        | Expr::Variable(_)
        | Expr::AtomRef { .. } => {}
        Expr::ArrayLit(elements) => {
            for element in elements {
                collect_expr_array_store_targets(element, base, may_repeat, targets);
            }
        }
        Expr::ArrayAccess(_, index) => {
            collect_expr_array_store_targets(index, base, may_repeat, targets);
        }
        Expr::BinaryOp(left, _, right) => {
            collect_expr_array_store_targets(left, base, may_repeat, targets);
            collect_expr_array_store_targets(right, base, may_repeat, targets);
        }
        Expr::Paren(inner) => collect_expr_array_store_targets(inner, base, may_repeat, targets),
        Expr::Block(stmt) => {
            collect_stmt_array_store_targets(stmt, base, may_repeat, targets);
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_expr_array_store_targets(cond, base, may_repeat, targets);
            collect_stmt_array_store_targets(then_branch, base, may_repeat, targets);
            collect_stmt_array_store_targets(else_branch, base, may_repeat, targets);
        }
        Expr::Call(_, args) => {
            for arg in args {
                collect_expr_array_store_targets(arg, base, may_repeat, targets);
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_expr_array_store_targets(value, base, may_repeat, targets);
            }
        }
        Expr::FieldAccess(inner, _) => {
            collect_expr_array_store_targets(inner, base, may_repeat, targets);
        }
        Expr::Match { target, arms } => {
            collect_expr_array_store_targets(target, base, may_repeat, targets);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_expr_array_store_targets(guard, base, may_repeat, targets);
                }
                collect_stmt_array_store_targets(&arm.body, base, may_repeat, targets);
            }
        }
        Expr::Async { body } | Expr::Lambda { body, .. } => {
            collect_stmt_array_store_targets(body, base, true, targets);
        }
        Expr::Await { expr } => {
            collect_expr_array_store_targets(expr, base, may_repeat, targets);
        }
        Expr::CallRef { callee, args } => {
            collect_expr_array_store_targets(callee, base, may_repeat, targets);
            for arg in args {
                collect_expr_array_store_targets(arg, base, may_repeat, targets);
            }
        }
        Expr::Perform { args, .. } => {
            for arg in args {
                collect_expr_array_store_targets(arg, base, may_repeat, targets);
            }
        }
        Expr::ChanSend { channel, value } => {
            collect_expr_array_store_targets(channel, base, may_repeat, targets);
            collect_expr_array_store_targets(value, base, may_repeat, targets);
        }
        Expr::ChanRecv { channel } => {
            collect_expr_array_store_targets(channel, base, may_repeat, targets);
        }
    }
}

fn collect_stmt_array_store_targets(
    stmt: &Stmt,
    base: &str,
    may_repeat: bool,
    targets: &mut Vec<ArrayStoreTarget>,
) {
    match stmt {
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            collect_expr_array_store_targets(value, base, may_repeat, targets);
        }
        Stmt::ArrayStore {
            array,
            index,
            field_path,
            value,
            ..
        } => {
            collect_expr_array_store_targets(value, base, may_repeat, targets);
            collect_expr_array_store_targets(index, base, may_repeat, targets);
            if array == base {
                targets.push(ArrayStoreTarget {
                    field_path: field_path.clone(),
                    index: (**index).clone(),
                    may_repeat,
                });
            }
        }
        Stmt::Block(stmts, _) => {
            for stmt in stmts {
                collect_stmt_array_store_targets(stmt, base, may_repeat, targets);
            }
        }
        Stmt::While {
            cond,
            invariant,
            decreases,
            body,
            ..
        } => {
            collect_expr_array_store_targets(cond, base, true, targets);
            collect_expr_array_store_targets(invariant, base, true, targets);
            if let Some(decreases) = decreases {
                collect_expr_array_store_targets(decreases, base, true, targets);
            }
            collect_stmt_array_store_targets(body, base, true, targets);
        }
        Stmt::Acquire { body, .. } => {
            collect_stmt_array_store_targets(body, base, may_repeat, targets);
        }
        Stmt::Task { body, .. } => {
            collect_stmt_array_store_targets(body, base, true, targets);
        }
        Stmt::TaskGroup { children, .. } => {
            for child in children {
                collect_stmt_array_store_targets(child, base, true, targets);
            }
        }
        Stmt::Cancel { .. } => {}
        Stmt::Expr(expr, _) => {
            collect_expr_array_store_targets(expr, base, may_repeat, targets);
        }
    }
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<Box<Expr>>,
    pub body: Box<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum JoinSemantics {
    All,
    Any,
}

impl JoinSemantics {
    pub fn completes_after_first_child(&self) -> bool {
        matches!(self, JoinSemantics::Any)
    }

    pub fn cancels_remaining_children(&self) -> bool {
        matches!(self, JoinSemantics::Any)
    }
}

#[derive(Debug, Clone)]
pub enum Pattern {
    Wildcard,
    Literal(i64),
    Variable(String),
    Variant {
        variant_name: String,
        fields: Vec<Pattern>,
    },
}

#[derive(Debug, Clone)]
pub struct EnumVariant {
    pub name: String,
    pub fields: Vec<String>,
    pub field_types: Vec<TypeRef>,
    // NOTE: is_recursive is set by parser for recursive ADT detection; read by future codegen for box/pointer decisions
    #[allow(dead_code)]
    pub is_recursive: bool,
}

#[derive(Debug, Clone)]
pub struct EnumDef {
    pub name: String,
    pub type_params: Vec<String>,
    pub variants: Vec<EnumVariant>,
    // NOTE: is_recursive is set by parser for recursive ADT detection; read by future codegen for box/pointer decisions
    #[allow(dead_code)]
    pub is_recursive: bool,
    pub span: Span,
}

// --- 2. Quantifiers, refined types, and Item ---

#[derive(Debug, Clone, PartialEq)]
pub enum QuantifierType {
    ForAll,
    Exists,
}

#[derive(Debug, Clone)]
pub struct Quantifier {
    pub q_type: QuantifierType,
    pub var: String,
    pub start: String,
    pub end: String,
    pub condition: String,
}

/// `type Name = <base> [unit <Unit>] [where <predicate>];`
///
/// A refined type may carry a unit-of-measure tag (`unit USD`). The unit is a
/// pure type-level label: it never changes the value's Z3 sort or runtime
/// representation, and is only consulted by the unit-consistency check on
/// `+`, `-` and comparisons (see `verification::support::units`).
#[derive(Debug, Clone)]
pub struct RefinedType {
    pub name: String,
    pub _base_type: String,
    pub operand: String,
    pub predicate_raw: String,
    /// Unit-of-measure tag (e.g. `USD`, `Meter`). `None` for unitless types.
    pub unit: Option<String>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Param {
    /// Bare parameter name with ownership markers (`consume `/`ref `/`ref mut `)
    /// stripped. Consumption is recorded in `consume`, borrows in `is_ref` /
    /// `is_ref_mut`.
    pub name: String,
    pub type_name: Option<String>,
    pub type_ref: Option<TypeRef>,
    /// `consume` ownership marker in the `atom`/`fn` signature
    /// (e.g. `atom f(consume xs: [i64])`).
    pub consume: bool,
    pub is_ref: bool,
    pub is_ref_mut: bool,
    /// Higher-Order Function contract: requires clause for function parameter.
    /// Syntax: `contract(f): requires: <expr>, ensures: <expr>;`
    pub fn_contract_requires: Option<String>,
    /// Higher-Order Function contract: ensures clause for function parameter.
    /// Used by call_with_contract to constrain the symbolic result of `call(f, args...)` in Z3.
    pub fn_contract_ensures: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClauseKind {
    Requires,
    Ensures,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClauseLabel {
    pub kind: ClauseKind,
    pub clause: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClauseTrustMode {
    Assume,
    Check,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClauseMode {
    pub kind: ClauseKind,
    pub clause: String,
    pub mode: ClauseTrustMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverClause {
    pub clause: String,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Atom {
    pub name: String,
    pub type_params: Vec<String>,
    /// トレイト境界: 型パラメータに課す制約（例: [TypeParamBound { param: "T", bounds: ["Comparable"] }]）
    /// 単相化時のトレイト境界バリデーションで使用
    pub where_bounds: Vec<TypeParamBound>,
    pub params: Vec<Param>,
    pub trace_id: Option<String>,
    pub spec_metadata: HashMap<String, String>,
    pub requires: String,
    pub clause_labels: Vec<ClauseLabel>,
    pub clause_modes: Vec<ClauseMode>,
    pub covers: Vec<CoverClause>,
    pub forall_constraints: Vec<Quantifier>,
    pub ensures: String,
    pub body_expr: String,
    pub consumed_params: Vec<String>,
    pub resources: Vec<String>,
    pub is_async: bool,
    pub trust_level: TrustLevel,
    pub max_unroll: Option<usize>,
    /// Per-atom `Str` literal bound (`str_bound: N;`) — the maximum
    /// byte length a `let`-bound string literal may declare under the
    /// composite profiles. `None` keeps the default `STR_BOUND`.
    pub str_bound: Option<usize>,
    pub invariant: Option<String>,
    pub effects: Vec<Effect>,
    /// Plan 18: Explicit return type annotation (e.g., `-> Str`). Defaults to None (i64).
    pub return_type: Option<String>,
    /// Termination measure for recursive contracts (`decreases: <expr>;`).
    pub decreases: Option<String>,
    pub span: Span,
    /// Modular Verification: required effect pre-states
    /// e.g., { "File" => "Open" } means File must be in Open state before this atom
    pub effect_pre: HashMap<String, String>,
    /// Modular Verification: guaranteed effect post-states
    /// e.g., { "File" => "Closed" } means File will be in Closed state after this atom
    pub effect_post: HashMap<String, String>,
}

impl Atom {
    /// Canonical view of the parameters this atom consumes. Mumei has two
    /// spellings for the same concept — the `consume` marker on a signature
    /// parameter (`atom f(consume x: T)`) and the `consume x;` clause — and
    /// both mean "the atom takes ownership of this parameter and it may not
    /// be used afterwards". Consumers deciding whether a parameter is
    /// consumed (linearity tracking, move lowering, hashing) must go through
    /// this instead of reading `consumed_params` or `Param::consume` alone.
    ///
    /// Clause targets come first (preserving declaration order), then
    /// signature-`consume` params not already covered. Names are
    /// deduplicated so a name declared through both spellings appears once.
    pub fn consumed_names(&self) -> Vec<String> {
        let mut names = self.consumed_params.clone();
        for param in &self.params {
            if param.consume && !names.iter().any(|name| name == &param.name) {
                names.push(param.name.clone());
            }
        }
        names
    }

    /// Whether this atom consumes the named parameter via either spelling.
    pub fn consumes_param(&self, name: &str) -> bool {
        self.consumed_params.iter().any(|p| p == name)
            || self.params.iter().any(|p| p.name == name && p.consume)
    }

    /// Whether the atom declares any consumed parameter at all.
    pub fn has_consumed_params(&self) -> bool {
        !self.consumed_params.is_empty() || self.params.iter().any(|p| p.consume)
    }
}

// =============================================================================
// Trust Boundary
// =============================================================================

#[derive(Debug, Clone, PartialEq)]
pub enum TrustLevel {
    Verified,
    Trusted,
    Unverified,
}

#[derive(Debug, Clone)]
pub struct StructField {
    pub name: String,
    pub type_name: String,
    pub type_ref: TypeRef,
    pub constraint: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StructDef {
    pub name: String,
    pub type_params: Vec<String>,
    pub fields: Vec<StructField>,
    /// Cross-field invariants declared as `invariant: <expr>` clauses in the
    /// struct body. Each expression refers to fields through the `self`
    /// binder (`self.active_tasks <= self.max_tasks`) or by bare field name.
    /// They are assumed for struct-typed parameters, checked at every
    /// `StructInit`, and imposed on `result` of atoms returning this struct.
    pub invariants: Vec<String>,
    pub method_names: Vec<String>,
    /// Methods defined in `impl StructName { atom ... }` blocks
    pub methods: Vec<Atom>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ImportDecl {
    pub span: Span,
    pub path: String,
    pub alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeParamBound {
    pub param: String,
    pub bounds: Vec<String>,
}

#[derive(Debug, Clone)]
// NOTE: TraitMethod is constructed by the parser and accessed via TraitDef.methods
// in law verification and trait impl expansion. Struct-level allow(dead_code)
// suppresses warnings on fields not yet consumed by all downstream passes.
#[allow(dead_code)]
pub struct TraitMethod {
    pub name: String,
    pub param_types: Vec<String>,
    pub return_type: String,
    // NOTE: param_constraints holds per-parameter refinement types (e.g., "v != 0") for future Z3 constraint generation
    #[allow(dead_code)]
    pub param_constraints: Vec<Option<String>>,
}

#[derive(Debug, Clone)]
pub struct TraitDef {
    pub name: String,
    pub methods: Vec<TraitMethod>,
    pub laws: Vec<(String, String)>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ImplDef {
    pub trait_name: String,
    pub target_type: String,
    pub method_bodies: Vec<(String, String)>,
    pub span: Span,
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum Item {
    Atom(Atom),
    TypeDef(RefinedType),
    StructDef(StructDef),
    EnumDef(EnumDef),
    Import(ImportDecl),
    TraitDef(TraitDef),
    ImplDef(ImplDef),
    ResourceDef(ResourceDef),
    ExternBlock(ExternBlock),
    EffectDef(EffectDef),
    /// `type FileCap = capability SafeFileRead(path: Str) where C;`
    CapabilityDef(CapabilityDef),
    /// `impl StructName { atom method(...) ... }` block
    ImplBlock(ImplBlock),
}

// =============================================================================
// FFI Bridge (extern blocks)
// =============================================================================

#[derive(Debug, Clone)]
pub struct ExternFn {
    pub name: String,
    pub param_names: Vec<String>,
    pub param_types: Vec<String>,
    pub return_type: String,
    /// Optional requires contract for verified FFI
    pub requires: Option<String>,
    /// Optional ensures contract for verified FFI
    pub ensures: Option<String>,
    pub span: Span,
}

#[derive(Debug, Clone)]
// NOTE: ExternBlock is constructed by the parser and consumed in load_and_prepare/register_imported_items.
// The `language` and `span` fields trigger dead_code warnings as they are only accessed via pattern matching.
#[allow(dead_code)]
pub struct ExternBlock {
    pub language: String,
    pub functions: Vec<ExternFn>,
    pub span: Span,
}

/// `impl StructName { atom method1(...) ... atom method2(...) ... }` block
#[derive(Debug, Clone)]
// NOTE: ImplBlock is constructed by the parser and matched in load_and_prepare, cmd_build, verify_source_for_lsp.
// The `span` field triggers dead_code warning as it is only used indirectly.
#[allow(dead_code)]
pub struct ImplBlock {
    pub struct_name: String,
    pub methods: Vec<Atom>,
    pub span: Span,
}

/// Effect Display implementation
impl std::fmt::Display for Effect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.params.is_empty() {
            write!(f, "{}", self.name)
        } else {
            let params_str: Vec<String> = self
                .params
                .iter()
                .map(|p| {
                    if p.is_constant {
                        format!("\"{}\"", p.value)
                    } else {
                        p.value.clone()
                    }
                })
                .collect();
            write!(f, "{}({})", self.name, params_str.join(", "))
        }
    }
}
