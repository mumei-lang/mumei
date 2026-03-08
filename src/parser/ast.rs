// =============================================================================
// AST type definitions
// =============================================================================
//
// All AST types are defined directly in this file and re-exported from mod.rs.
// This file contains all the core data structures used by the parser.

use crate::ast::TypeRef;

// --- 0. Source position information (Span) ---

/// Source position information. Attached to all AST nodes for diagnostic accuracy.
#[derive(Debug, Clone, PartialEq, Default)]
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

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    Neq,
    Gt,
    Lt,
    Ge,
    Le,
    And,
    Or,
    Implies,
}

// =============================================================================
// Async + Resource Management
// =============================================================================

#[derive(Debug, Clone)]
pub struct ResourceDef {
    pub name: String,
    pub priority: i64,
    pub mode: ResourceMode,
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
    pub parent: Option<String>,
    pub span: Span,
}

// NOTE: EffectDefParam fields are read during effect constraint resolution (future Z3 String Sort integration)
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct EffectDefParam {
    pub name: String,
    pub type_name: String,
}

#[derive(Debug, Clone)]
pub enum Expr {
    Number(i64),
    Float(f64),
    Variable(String),
    ArrayAccess(String, Box<Expr>),
    BinaryOp(Box<Expr>, Op, Box<Expr>),
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
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Let {
        var: String,
        value: Box<Expr>,
    },
    Assign {
        var: String,
        value: Box<Expr>,
    },
    Block(Vec<Stmt>),
    While {
        cond: Box<Expr>,
        invariant: Box<Expr>,
        decreases: Option<Box<Expr>>,
        body: Box<Stmt>,
    },
    Acquire {
        resource: String,
        body: Box<Stmt>,
    },
    Task {
        body: Box<Stmt>,
        group: Option<String>,
    },
    TaskGroup {
        children: Vec<Stmt>,
        join_semantics: JoinSemantics,
    },
    Expr(Expr),
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

#[derive(Debug, Clone)]
pub struct RefinedType {
    pub name: String,
    pub _base_type: String,
    pub operand: String,
    pub predicate_raw: String,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub type_name: Option<String>,
    pub type_ref: Option<TypeRef>,
    pub is_ref: bool,
    pub is_ref_mut: bool,
}

#[derive(Debug, Clone)]
pub struct Atom {
    pub name: String,
    pub type_params: Vec<String>,
    // NOTE: where_bounds is used for trait bound validation during future monomorphization
    #[allow(dead_code)]
    pub where_bounds: Vec<TypeParamBound>,
    pub params: Vec<Param>,
    pub requires: String,
    pub forall_constraints: Vec<Quantifier>,
    pub ensures: String,
    pub body_expr: String,
    pub consumed_params: Vec<String>,
    pub resources: Vec<String>,
    pub is_async: bool,
    pub trust_level: TrustLevel,
    pub max_unroll: Option<usize>,
    pub invariant: Option<String>,
    pub effects: Vec<Effect>,
    pub span: Span,
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
    // NOTE: method_names tracks struct-associated atoms (e.g., "Stack::push") for future method resolution
    #[allow(dead_code)]
    pub method_names: Vec<String>,
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
}

// =============================================================================
// FFI Bridge (extern blocks)
// =============================================================================

// NOTE: ExternFn fields will be used when auto-registering extern functions as trusted atoms in ModuleEnv
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ExternFn {
    pub name: String,
    pub param_types: Vec<String>,
    pub return_type: String,
    pub span: Span,
}

// NOTE: ExternBlock span will be used for error reporting in future extern function validation
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ExternBlock {
    pub language: String,
    pub functions: Vec<ExternFn>,
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
