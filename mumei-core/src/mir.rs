// =============================================================================
// MIR (Mid-level Intermediate Representation)
// =============================================================================
// CFG-based representation for borrow checking and Z3 constraint optimization.
// Lowered from HIR; each atom body becomes a MirBody consisting of BasicBlocks.
//
// Design goals:
//   - Three-address code: every nested expression is flattened into temporaries.
//   - Explicit control flow: if/else and while become BasicBlock graphs.
//   - Suitable for lifetime/borrow analysis and drop insertion in future phases.
//
// Expression coverage (Plan 19 — Phase 4c complete):
//   All HirExpr forms are lowered: Number, Float, StringLit, Variable,
//   BinaryOp, Call, IfThenElse, StructInit, FieldAccess, Perform,
//   ArrayAccess, Match, AtomRef, CallRef, Lambda, Async, Await,
//   Task, TaskGroup, ChanSend, ChanRecv, VariantInit.
//
// See also: docs/ROADMAP.md "Multi-Stage IR Roadmap" section.
// =============================================================================

use crate::ast::CapabilityType;
use crate::hir::{HirAtom, HirExpr, HirStmt};
use crate::parser::Op;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// =============================================================================
// Core MIR types
// =============================================================================

/// A unique identifier for a local variable or temporary.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Local(pub usize);

/// A place in memory (variable or field access).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(dead_code)]
pub enum Place {
    Local(Local),
    Field(Box<Place>, String),
    Index(Box<Place>, Local),
}

/// Right-hand side of an assignment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(dead_code)]
pub enum Rvalue {
    Use(Operand),
    BinaryOp(Op, Operand, Operand),
    Call {
        func: String,
        args: Vec<Operand>,
    },
    Ref(Place),
    RefMut(Place),
    StructInit {
        type_name: String,
        fields: Vec<(String, Operand)>,
    },
    FieldAccess(Operand, String),
    Perform {
        effect: String,
        operation: String,
        args: Vec<Operand>,
    },
    /// Array literal `[e0, e1, …]` — the produced value is an array (Move type).
    ArrayLit(Vec<Operand>),
}

/// An operand: either a place (variable) or a constant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(dead_code)]
pub enum Operand {
    Place(Place),
    /// An explicit ownership-consuming use of a place.
    Move(Place),
    Constant(MirConstant),
}

/// Parameter mode recorded on a MIR body for callee-side borrow checking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MirParamMode {
    Owned,
    Shared,
    Mut,
    Consume,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(dead_code)]
pub enum MirConstant {
    Int(i64),
    Float(f64),
    Bool(bool),
    /// Plan 9: String constant
    Str(String),
    /// Function reference (atom_ref) — holds the function name.
    FuncRef(String),
}

/// A single MIR statement (three-address code).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(dead_code)]
pub enum MirStatement {
    Assign(Place, Rvalue),
    StorageLive(Local),
    StorageDead(Local),
    Drop(Local),
    Nop,
}

/// Block terminator: how control flow leaves a basic block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(dead_code)]
pub enum Terminator {
    Goto(BasicBlockId),
    SwitchInt {
        discr: Operand,
        targets: Vec<(i64, BasicBlockId)>,
        otherwise: BasicBlockId,
    },
    Return(Operand),
    Unreachable,
}

pub type BasicBlockId = usize;

/// A basic block: a sequence of statements followed by a terminator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BasicBlock {
    pub id: BasicBlockId,
    pub statements: Vec<MirStatement>,
    pub terminator: Terminator,
}

/// Upper bound on MIR analysis complexity (block_count * local_count).
/// When exceeded, dataflow analyses are skipped to prevent explosion.
pub const MIR_ANALYSIS_COMPLEXITY_LIMIT: usize = 10_000;

/// A complete MIR body for one atom.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct MirBody {
    pub name: String,
    pub locals: Vec<LocalDecl>,
    pub blocks: Vec<BasicBlock>,
    pub entry_block: BasicBlockId,
    /// Names referenced in the body with no binding at lowering time.
    /// The verifier fails closed on any entry — these are typos or
    /// mis-parsed keywords, not free variables.
    #[serde(default)]
    pub unbound_names: Vec<String>,
    /// Borrow mode for each atom parameter local.
    #[serde(default)]
    pub param_modes: HashMap<Local, MirParamMode>,
}

impl MirBody {
    /// Number of basic blocks in this MIR body.
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// Number of local variables in this MIR body.
    pub fn local_count(&self) -> usize {
        self.locals.len()
    }

    /// Approximate complexity metric for dataflow analyses.
    /// The product of blocks × locals bounds the size of the dataflow lattice.
    pub fn complexity(&self) -> usize {
        self.blocks.len() * self.locals.len()
    }

    /// Check whether this MIR body exceeds the analysis complexity budget.
    /// Returns an error message if the budget is exceeded, describing the
    /// overshoot so callers can decide whether to skip MIR analysis.
    pub fn check_analysis_budget(&self) -> Result<(), String> {
        let c = self.complexity();
        if c > MIR_ANALYSIS_COMPLEXITY_LIMIT {
            Err(format!(
                "MIR analysis budget exceeded for '{}': complexity {} (blocks={} * locals={}) > limit {}",
                self.name,
                c,
                self.block_count(),
                self.local_count(),
                MIR_ANALYSIS_COMPLEXITY_LIMIT
            ))
        } else {
            Ok(())
        }
    }

    /// Returns a map from each block ID to its successor block IDs.
    pub fn successors(&self) -> HashMap<BasicBlockId, Vec<BasicBlockId>> {
        let mut result = HashMap::new();
        for block in &self.blocks {
            let succs = match &block.terminator {
                Terminator::Goto(target) => vec![*target],
                Terminator::SwitchInt {
                    targets, otherwise, ..
                } => {
                    let mut s: Vec<BasicBlockId> = targets.iter().map(|(_, t)| *t).collect();
                    s.push(*otherwise);
                    s
                }
                Terminator::Return(_) => vec![],
                Terminator::Unreachable => vec![],
            };
            result.insert(block.id, succs);
        }
        result
    }

    /// Returns a map from each block ID to its predecessor block IDs.
    pub fn predecessors(&self) -> HashMap<BasicBlockId, Vec<BasicBlockId>> {
        let mut result: HashMap<BasicBlockId, Vec<BasicBlockId>> = HashMap::new();
        // Initialize all blocks with empty predecessor lists
        for block in &self.blocks {
            result.entry(block.id).or_default();
        }
        let succs = self.successors();
        for (block_id, successors) in &succs {
            for &succ in successors {
                result.entry(succ).or_default().push(*block_id);
            }
        }
        result
    }
}

/// Whether a local is Copy (bitwise-duplicable) or Move (ownership transfer).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Movability {
    /// Primitive types (i64, f64, bool) and refined type aliases (Nat, Pos, etc.) — assignment copies.
    Copy,
    /// Structs, enums, and other owned types — assignment moves.
    Move,
}

/// Determine movability from a type name string.
/// Primitive numeric types and bool are Copy; everything else is Move.
pub fn movability_from_type(ty: &Option<String>) -> Movability {
    match ty.as_deref() {
        Some(
            "i64" | "i32" | "i16" | "i8" | "u64" | "u32" | "u16" | "u8" | "f64" | "f32" | "Int"
            | "Nat" | "Pos" | "Float" | "bool" | "Bool"
            // Standard library refined types (all i64-based)
            | "RawPtr" | "NullablePtr" | "HumanAge"
            // `let f = |params| …` — a lambda binding is an immutable
            // closure reference, not an owned resource: reads copy, never
            // move, so `if c {f} else {g}` doesn't consume f/g.
            | "__lambda" | "resource",
        ) => Movability::Copy,
        _ => Movability::Move,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct LocalDecl {
    pub local: Local,
    pub name: Option<String>,
    pub ty: Option<String>,
    /// Whether this local is Copy or Move. Defaults to Move for unknown types.
    pub movability: Movability,
    /// capability 型ローカルの静的情報（capability 型以外は `None`）。
    pub capability: Option<CapabilityType>,
}

// =============================================================================
// HIR -> MIR lowering
// =============================================================================

/// Mutable context used during lowering to allocate locals and basic blocks.
struct LowerCtx {
    locals: Vec<LocalDecl>,
    blocks: Vec<BasicBlock>,
    /// Statements accumulated for the current basic block being built.
    current_stmts: Vec<MirStatement>,
    /// Mapping from variable name to Local index.
    var_map: std::collections::HashMap<String, Local>,
    /// User type aliases (`type Usd = i64 ...;`) resolved to their base type,
    /// so movability follows the underlying representation.
    alias_bases: std::collections::HashMap<String, String>,
    /// Enum definitions keyed by enum name — used to give match pattern
    /// bindings their declared field types so Copy fields stay Copy.
    enum_defs: std::collections::HashMap<String, crate::parser::EnumDef>,
    /// Module-level declaration names (enums, structs, atoms, type aliases,
    /// effects). Referencing one of these names — e.g. the `Shape` in
    /// `Shape::Point`, the effect name in `perform FileWrite.write(x)`, or an
    /// atom name inside `atom_ref(name)` / generic-call sugar — is a type or
    /// item reference, not an unbound variable.
    env_names: std::collections::BTreeSet<String>,
    /// Callee parameter modes used to materialize call-site loans.
    callee_modes: std::collections::HashMap<String, Vec<MirParamMode>>,
    /// Variable names that had no binding when they were referenced —
    /// surfaced to the caller as `MirBody::unbound_names` (fail-closed).
    unbound_names: std::collections::BTreeSet<String>,
    next_local: usize,
    next_block: usize,
}

impl LowerCtx {
    fn new() -> Self {
        Self::with_env(None)
    }

    fn with_env(module_env: Option<&crate::verification::ModuleEnv>) -> Self {
        let alias_bases = module_env
            .map(|env| {
                env.types
                    .keys()
                    .map(|name| (name.clone(), resolve_alias_base(env, name)))
                    .collect()
            })
            .unwrap_or_default();
        let enum_defs = module_env.map(|env| env.enums.clone()).unwrap_or_default();
        let env_names = module_env
            .map(|env| {
                env.enums
                    .keys()
                    .chain(env.structs.keys())
                    .chain(env.atoms.keys())
                    .chain(env.types.keys())
                    .chain(env.effects.keys())
                    .chain(env.effect_defs.keys())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let callee_modes = module_env
            .map(|env| {
                env.atoms
                    .iter()
                    .map(|(name, atom)| {
                        let modes = atom
                            .params
                            .iter()
                            .map(|param| {
                                if param.is_ref_mut {
                                    MirParamMode::Mut
                                } else if param.is_ref {
                                    MirParamMode::Shared
                                } else if atom.consumed_params.iter().any(|p| p == &param.name) {
                                    MirParamMode::Consume
                                } else {
                                    MirParamMode::Owned
                                }
                            })
                            .collect();
                        (name.clone(), modes)
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            locals: Vec::new(),
            blocks: Vec::new(),
            current_stmts: Vec::new(),
            var_map: std::collections::HashMap::new(),
            alias_bases,
            enum_defs,
            env_names,
            callee_modes,
            unbound_names: std::collections::BTreeSet::new(),
            next_local: 0,
            next_block: 0,
        }
    }

    /// Allocate a new local (named or temporary).
    fn alloc_local(&mut self, name: Option<String>, ty: Option<String>) -> Local {
        self.alloc_local_with_capability(name, ty, None)
    }

    /// Allocate a new local carrying capability type information.
    fn alloc_local_with_capability(
        &mut self,
        name: Option<String>,
        ty: Option<String>,
        capability: Option<CapabilityType>,
    ) -> Local {
        let local = Local(self.next_local);
        self.next_local += 1;
        let movability = movability_from_type(&ty.as_ref().map(|t| {
            self.alias_bases
                .get(t)
                .cloned()
                .unwrap_or_else(|| t.clone())
        }));
        self.locals.push(LocalDecl {
            local: local.clone(),
            name: name.clone(),
            ty,
            movability,
            capability,
        });
        if let Some(n) = name {
            self.var_map.insert(n, local.clone());
        }
        local
    }

    /// Allocate a new unnamed temporary.
    fn alloc_temp(&mut self) -> Local {
        self.alloc_local(None, None)
    }

    fn call_arg_mode(&self, func: &str, index: usize) -> MirParamMode {
        self.callee_modes
            .get(func)
            .or_else(|| self.callee_modes.get(&func.replace('.', "::")))
            .and_then(|modes| modes.get(index).copied())
            .unwrap_or(MirParamMode::Owned)
    }

    fn has_known_callee(&self, func: &str) -> bool {
        self.callee_modes.contains_key(func)
            || self.callee_modes.contains_key(&func.replace('.', "::"))
    }

    /// Finish the current basic block with the given terminator and return its id.
    fn finish_block(&mut self, terminator: Terminator) -> BasicBlockId {
        let id = self.next_block;
        self.next_block += 1;
        let stmts = std::mem::take(&mut self.current_stmts);
        self.blocks.push(BasicBlock {
            id,
            statements: stmts,
            terminator,
        });
        id
    }

    /// Patch the terminator of an already-finished block (back-patching pattern).
    /// Used by control-flow lowering to fill in block IDs that are not yet known
    /// at the time the block is first created.
    fn patch_terminator(&mut self, block_id: BasicBlockId, terminator: Terminator) {
        if let Some(block) = self.blocks.iter_mut().find(|b| b.id == block_id) {
            block.terminator = terminator;
        }
    }

    /// Emit a statement into the current (in-progress) block.
    fn emit(&mut self, stmt: MirStatement) {
        self.current_stmts.push(stmt);
    }

    /// Look up a variable by name, returning its Place.
    fn lookup_var(&mut self, name: &str) -> Place {
        if let Some(local) = self.var_map.get(name) {
            Place::Local(local.clone())
        } else {
            // Unbound name: allocate a dedicated local instead of silently
            // aliasing Local(0) — otherwise an unresolved name (typo, or a
            // mis-parsed keyword like a bare `task`) would consume/move the
            // atom's first local during ownership analysis.
            // `result` is exempt: it is the atom's implicit named return
            // binding (`result = x` in a body assigns the return value).
            if name != "result" && !self.env_names.contains(name) {
                self.unbound_names.insert(name.to_string());
            }
            let local = self.alloc_local(Some(format!("__unbound:{name}")), None);
            self.var_map.insert(name.to_string(), local.clone());
            Place::Local(local)
        }
    }

    fn lookup_resource(&mut self, name: &str) -> Place {
        if let Some(local) = self.var_map.get(name) {
            return Place::Local(local.clone());
        }
        Place::Local(self.alloc_local(Some(name.to_string()), Some("resource".to_string())))
    }

    /// A bare name that resolves to a nullary variant of a declared enum
    /// (e.g. `Red` for `enum Color { Red, .. }`) — not an unbound variable.
    fn is_nullary_enum_variant(&self, name: &str) -> bool {
        let leaf = name.rsplit_once("::").map(|(_, l)| l).unwrap_or(name);
        self.enum_defs.values().any(|def| {
            def.variants
                .iter()
                .any(|v| v.name == leaf && v.fields.is_empty())
        })
    }

    /// Look up the recorded type of a named variable, if any.
    fn lookup_var_ty(&self, name: &str) -> Option<String> {
        let local = self.var_map.get(name)?;
        self.locals
            .iter()
            .find(|d| d.local == *local)
            .and_then(|d| d.ty.clone())
    }

    /// The enum a match `Variant` pattern belongs to: a qualified `E::V` name
    /// pins `E`; otherwise the scrutinee's declared type supplies it.
    fn pattern_enum_name(&self, variant_name: &str, scrutinee_ty: Option<&str>) -> Option<String> {
        if let Some((qual, _)) = variant_name.rsplit_once("::") {
            if self.enum_defs.contains_key(qual) {
                return Some(qual.to_string());
            }
        }
        let ty = scrutinee_ty?;
        if self.enum_defs.contains_key(ty) {
            Some(ty.to_string())
        } else {
            None
        }
    }

    /// The declared type of variant field `idx`, with `Self` resolved to the
    /// owning enum. `variant_name` may be qualified (`E::V`) or a leaf (`V`).
    fn variant_field_ty(
        &self,
        enum_name: Option<&str>,
        variant_name: &str,
        idx: usize,
    ) -> Option<String> {
        let def = self.enum_defs.get(enum_name?)?;
        let leaf = variant_name
            .rsplit_once("::")
            .map(|(_, l)| l)
            .unwrap_or(variant_name);
        let variant = def.variants.iter().find(|v| v.name == leaf)?;
        let name = variant.field_types.get(idx)?.name.clone();
        Some(if name == "Self" {
            def.name.clone()
        } else {
            name
        })
    }

    /// Type of the variable a match pattern binds at position `name` — either
    /// the scrutinee's type (whole-scrutinee `Variable`) or a variant field's
    /// declared type. Used by `infer_hir_ty` on `Match` values, which runs
    /// before the arm's locals exist in `var_map`.
    fn pattern_binding_ty(
        &self,
        scrutinee_ty: Option<&str>,
        pattern: &crate::parser::Pattern,
        name: &str,
    ) -> Option<String> {
        match pattern {
            crate::parser::Pattern::Variable(bound) if bound == name => {
                scrutinee_ty.map(std::string::ToString::to_string)
            }
            crate::parser::Pattern::Variant {
                variant_name,
                fields,
            } => {
                let enum_name = self.pattern_enum_name(variant_name, scrutinee_ty);
                fields.iter().enumerate().find_map(|(idx, fp)| match fp {
                    crate::parser::Pattern::Variable(bound) if bound == name => {
                        self.variant_field_ty(enum_name.as_deref(), variant_name, idx)
                    }
                    crate::parser::Pattern::Variant { .. } => {
                        let nested_ty =
                            self.variant_field_ty(enum_name.as_deref(), variant_name, idx);
                        self.pattern_binding_ty(nested_ty.as_deref(), fp, name)
                    }
                    _ => None,
                })
            }
            _ => None,
        }
    }

    /// Best-effort type inference for a HIR expression. Used to populate
    /// `LocalDecl::ty` when the surface syntax does not annotate `let` /
    /// induction-variable types. Only the common cases that influence
    /// `Movability` (i.e. the Copy primitives) are recognised; anything
    /// unknown returns `None` and the local stays conservatively Move.
    fn infer_hir_ty(&self, expr: &HirExpr) -> Option<String> {
        self.infer_hir_ty_in(expr, &std::collections::HashMap::new())
    }

    fn infer_hir_ty_in(
        &self,
        expr: &HirExpr,
        locals: &std::collections::HashMap<String, String>,
    ) -> Option<String> {
        match expr {
            HirExpr::Number(_) => Some("i64".to_string()),
            HirExpr::Float(_) => Some("f64".to_string()),
            HirExpr::StringLit(_) => Some("Str".to_string()),
            // `|params| body` — mark the bound local with the internal
            // lambda type so `movability_from_type` makes it Copy: lambda
            // locals are immutable references, and reuse after a
            // conditional binding (`let h = if c {f} else {g}`) must not
            // trip move analysis.
            HirExpr::Lambda { .. } => Some("__lambda".to_string()),
            HirExpr::Variable(name) => {
                if name == "true" || name == "false" {
                    Some("bool".to_string())
                } else {
                    locals
                        .get(name)
                        .cloned()
                        .or_else(|| self.lookup_var_ty(name))
                }
            }
            HirExpr::BinaryOp(lhs, op, rhs) => match op {
                crate::parser::Op::Eq
                | crate::parser::Op::Neq
                | crate::parser::Op::Lt
                | crate::parser::Op::Le
                | crate::parser::Op::Gt
                | crate::parser::Op::Ge
                | crate::parser::Op::And
                | crate::parser::Op::Or
                | crate::parser::Op::Implies => Some("bool".to_string()),
                crate::parser::Op::Pow => {
                    if self.infer_hir_ty_in(lhs, locals).as_deref() == Some("f64")
                        || self.infer_hir_ty_in(rhs, locals).as_deref() == Some("f64")
                    {
                        Some("f64".to_string())
                    } else {
                        Some("i64".to_string())
                    }
                }
                _ => self
                    .infer_hir_ty_in(lhs, locals)
                    .or_else(|| self.infer_hir_ty_in(rhs, locals)),
            },
            // `[e0, …]` — element type from the first element; the binding is
            // an array (Move) so later `a[i]` reads and `let b = a` moves are
            // tracked correctly. Empty literals can't reach HIR (parse error).
            HirExpr::ArrayLit(elements) => elements
                .first()
                .and_then(|e| self.infer_hir_ty_in(e, locals))
                .map(|elem| format!("[{elem}]")),
            HirExpr::ArrayAccess(name, _) => locals
                .get(name)
                .cloned()
                .or_else(|| self.lookup_var_ty(name))
                .and_then(|ty| {
                    if ty.starts_with('[') && ty.ends_with(']') {
                        Some(ty[1..ty.len() - 1].to_string())
                    } else {
                        None
                    }
                })
                .or_else(|| Some("i64".to_string())),
            HirExpr::IfThenElse {
                then_branch,
                else_branch,
                ..
            } => self
                .infer_hir_branch_ty_in(then_branch, locals)
                .or_else(|| self.infer_hir_branch_ty_in(else_branch, locals)),
            HirExpr::Match { target, arms } => {
                let scrutinee_ty = self.infer_hir_ty_in(target, locals);
                arms.iter().find_map(|arm| {
                    hir_stmt_tail_expr(&arm.body)
                        .and_then(|e| match e {
                            HirExpr::Variable(name) => {
                                self.pattern_binding_ty(scrutinee_ty.as_deref(), &arm.pattern, name)
                            }
                            _ => None,
                        })
                        .or_else(|| self.infer_hir_branch_ty_in(&arm.body, locals))
                })
            }
            // P25: `recv(ch)` yields the channel's declared payload type.
            HirExpr::ChanRecv { channel } => match channel.as_ref() {
                HirExpr::Variable(name) => locals
                    .get(name)
                    .cloned()
                    .or_else(|| self.lookup_var_ty(name))
                    .and_then(|ty| crate::lowering::chan_payload_type(&ty)),
                _ => None,
            },
            _ => None,
        }
    }

    fn infer_hir_branch_ty(&self, stmt: &HirStmt) -> Option<String> {
        self.infer_hir_branch_ty_in(stmt, &std::collections::HashMap::new())
    }

    fn infer_hir_branch_ty_in(
        &self,
        stmt: &HirStmt,
        outer_locals: &std::collections::HashMap<String, String>,
    ) -> Option<String> {
        match stmt {
            HirStmt::Block { stmts, tail_expr } => {
                let mut locals = outer_locals.clone();
                for stmt in stmts {
                    if let HirStmt::Let { var, ty, value } = stmt {
                        // HIR `ty` can be inferred before parameter types are
                        // available, so prefer the param-aware expression
                        // inference and retain it only as a fallback.
                        let inferred = self.infer_hir_ty_in(value, &locals).or_else(|| ty.clone());
                        match inferred {
                            Some(ty) => {
                                locals.insert(var.clone(), ty);
                            }
                            None => {
                                locals.remove(var);
                            }
                        }
                    }
                }
                tail_expr
                    .as_deref()
                    .and_then(|expr| self.infer_hir_ty_in(expr, &locals))
            }
            _ => hir_stmt_tail_expr(stmt).and_then(|expr| self.infer_hir_ty_in(expr, outer_locals)),
        }
    }
}

/// Follow `type A = B; type B = i64;` chains down to the base type name.
pub fn resolve_alias_base(env: &crate::verification::ModuleEnv, name: &str) -> String {
    let mut current = name.to_string();
    let mut seen = std::collections::HashSet::new();
    while let Some(refined) = env.get_type(&current) {
        if !seen.insert(current.clone()) || refined._base_type == current {
            break;
        }
        current = refined._base_type.clone();
    }
    current
}

/// Lower a HirAtom to MirBody.
/// Phase 4b: basic lowering that flattens nested expressions into three-address code.
pub fn lower_hir_to_mir(hir_atom: &HirAtom) -> MirBody {
    lower_hir_to_mir_with_env(hir_atom, None)
}

/// Like [`lower_hir_to_mir`], but resolves user type aliases through the
/// `ModuleEnv` so an alias of a Copy base (`type Usd = i64 unit USD;`) is Copy.
pub fn lower_hir_to_mir_with_env(
    hir_atom: &HirAtom,
    module_env: Option<&crate::verification::ModuleEnv>,
) -> MirBody {
    let mut ctx = LowerCtx::with_env(module_env);
    let mut param_modes = HashMap::new();

    // Allocate locals for atom parameters.
    for param in &hir_atom.atom.params {
        let capability = param.type_ref.as_ref().and_then(|ty| ty.capability.clone());
        // `consume x` / `ref x` params keep the keyword in `name` (parser
        // quirk) — bind under the bare identifier so `x` resolves in the body.
        let pname = param.name.rsplit(' ').next().unwrap_or(param.name.as_str());
        let local = ctx.alloc_local_with_capability(
            Some(pname.to_string()),
            param.type_name.clone(),
            capability,
        );
        let mode = if param.is_ref_mut {
            MirParamMode::Mut
        } else if param.is_ref {
            MirParamMode::Shared
        } else if hir_atom
            .atom
            .consumed_params
            .iter()
            .any(|name| name == &param.name)
        {
            MirParamMode::Consume
        } else {
            MirParamMode::Owned
        };
        param_modes.insert(local.clone(), mode);
        ctx.emit(MirStatement::StorageLive(local));
    }

    // Lower the body.
    let result = lower_stmt(&mut ctx, &hir_atom.body);

    // Finish the last block with a Return terminator.
    let ret_operand = result.unwrap_or(Operand::Constant(MirConstant::Int(0)));
    let _entry = ctx.finish_block(Terminator::Return(ret_operand));

    // The entry block is always block 0.
    MirBody {
        name: hir_atom.atom.name.clone(),
        locals: ctx.locals,
        blocks: ctx.blocks,
        entry_block: 0,
        unbound_names: ctx.unbound_names.into_iter().collect(),
        param_modes,
    }
}

/// Infer an atom's return type from its lowered body expression.
pub fn infer_atom_return_type(atom: &crate::parser::Atom) -> Option<String> {
    let hir = crate::hir::lower_atom_to_hir(atom);
    let mut ctx = LowerCtx::new();
    for param in &hir.atom.params {
        let pname = param.name.rsplit(' ').next().unwrap_or(param.name.as_str());
        ctx.alloc_local(Some(pname.to_string()), param.type_name.clone());
    }
    match &hir.body {
        crate::hir::HirStmt::Expr(expr) => ctx.infer_hir_ty(expr),
        crate::hir::HirStmt::Block { .. } => ctx.infer_hir_branch_ty(&hir.body),
        _ => None,
    }
}

/// The value expression a statement position evaluates to, if any.
fn hir_stmt_tail_expr(stmt: &HirStmt) -> Option<&HirExpr> {
    match stmt {
        HirStmt::Expr(e) => Some(e),
        HirStmt::Block { tail_expr, .. } => tail_expr.as_deref(),
        _ => None,
    }
}

/// Emit MIR locals for the variables a match pattern binds.
///
/// `Variable` binds the whole scrutinee; `Variant` binds each field pattern to
/// a `FieldAccess` projection of the scrutinee (nested variants recurse through
/// a temporary). Wildcards and literals bind nothing. Bound locals carry the
/// field's declared type when it resolves (`enum_defs`), so `i64` payloads stay
/// Copy instead of defaulting to Move.
fn lower_pattern_bindings(
    ctx: &mut LowerCtx,
    pattern: &crate::parser::Pattern,
    discr: &Operand,
    scrutinee_ty: Option<&str>,
) {
    match pattern {
        crate::parser::Pattern::Variable(name) => {
            let local = ctx.alloc_local(
                Some(name.clone()),
                scrutinee_ty.map(std::string::ToString::to_string),
            );
            ctx.emit(MirStatement::StorageLive(local.clone()));
            ctx.emit(MirStatement::Assign(
                Place::Local(local),
                Rvalue::Use(discr.clone()),
            ));
        }
        crate::parser::Pattern::Variant {
            variant_name,
            fields,
        } => {
            let enum_name = ctx.pattern_enum_name(variant_name, scrutinee_ty);
            for (idx, field_pattern) in fields.iter().enumerate() {
                let field_ty = ctx.variant_field_ty(enum_name.as_deref(), variant_name, idx);
                lower_variant_field_binding(ctx, field_pattern, discr, idx, field_ty);
            }
        }
        crate::parser::Pattern::Wildcard | crate::parser::Pattern::Literal(_) => {}
    }
}

fn lower_variant_field_binding(
    ctx: &mut LowerCtx,
    pattern: &crate::parser::Pattern,
    discr: &Operand,
    idx: usize,
    field_ty: Option<String>,
) {
    match pattern {
        crate::parser::Pattern::Variable(name) => {
            let local = ctx.alloc_local(Some(name.clone()), field_ty);
            ctx.emit(MirStatement::StorageLive(local.clone()));
            ctx.emit(MirStatement::Assign(
                Place::Local(local),
                Rvalue::FieldAccess(discr.clone(), idx.to_string()),
            ));
        }
        crate::parser::Pattern::Variant {
            variant_name,
            fields,
        } => {
            // Nested variant pattern: bind a temporary to the intermediate
            // field value, then bind its sub-patterns off the temporary.
            let tmp = ctx.alloc_local(None, field_ty.clone());
            ctx.emit(MirStatement::StorageLive(tmp.clone()));
            ctx.emit(MirStatement::Assign(
                Place::Local(tmp.clone()),
                Rvalue::FieldAccess(discr.clone(), idx.to_string()),
            ));
            let tmp_op = Operand::Place(Place::Local(tmp));
            // A qualified nested pattern (`Mine::Yes(v)`) names its own enum;
            // otherwise the parent field's declared type supplies it.
            let enum_name = ctx.pattern_enum_name(variant_name, field_ty.as_deref());
            for (sub_idx, sub_pattern) in fields.iter().enumerate() {
                let sub_ty = ctx.variant_field_ty(enum_name.as_deref(), variant_name, sub_idx);
                lower_variant_field_binding(ctx, sub_pattern, &tmp_op, sub_idx, sub_ty);
            }
        }
        crate::parser::Pattern::Wildcard | crate::parser::Pattern::Literal(_) => {}
    }
}

/// Lower a HirStmt, returning an optional Operand for the value it produces.
fn lower_stmt(ctx: &mut LowerCtx, stmt: &HirStmt) -> Option<Operand> {
    match stmt {
        HirStmt::Let { var, ty, value } => {
            // Infer a concrete type from the initializer when the surface
            // syntax did not annotate one. This is critical for move
            // analysis: numeric literals make the binding `Copy`, which
            // prevents UseAfterMove false-positives on common idioms such
            // as `let i = 0; while i < n { arr[i] = …; i = i + 1 }`.
            let inferred_ty = ty.clone().or_else(|| ctx.infer_hir_ty(value));
            let val_op = lower_expr(ctx, value);
            let local = ctx.alloc_local(Some(var.clone()), inferred_ty);
            ctx.emit(MirStatement::StorageLive(local.clone()));
            ctx.emit(MirStatement::Assign(
                Place::Local(local),
                Rvalue::Use(val_op),
            ));
            None
        }
        HirStmt::Assign { var, value } => {
            let val_op = lower_expr(ctx, value);
            let place = if let Some((resource, field)) = var.split_once('.') {
                Place::Field(Box::new(ctx.lookup_resource(resource)), field.to_string())
            } else {
                ctx.lookup_var(var)
            };
            ctx.emit(MirStatement::Assign(place, Rvalue::Use(val_op)));
            None
        }
        HirStmt::ArrayStore {
            array,
            index,
            value,
        } => {
            // Evaluate value first (matches LLVM codegen / Z3 ordering).
            let val_op = lower_expr(ctx, value);

            // Materialize the index in a fresh local so it can be referenced
            // from a `Place::Index` in the target place.
            let idx_op = lower_expr(ctx, index);
            let idx_local = ctx.alloc_temp();
            ctx.emit(MirStatement::StorageLive(idx_local.clone()));
            ctx.emit(MirStatement::Assign(
                Place::Local(idx_local.clone()),
                Rvalue::Use(idx_op),
            ));

            let arr_place = ctx.lookup_var(array);
            let store_place = Place::Index(Box::new(arr_place), idx_local);
            ctx.emit(MirStatement::Assign(store_place, Rvalue::Use(val_op)));
            None
        }
        HirStmt::Block { stmts, tail_expr } => {
            for s in stmts {
                lower_stmt(ctx, s);
            }
            tail_expr.as_ref().map(|expr| lower_expr(ctx, expr))
        }
        HirStmt::While { cond, body, .. } => {
            // While loop (Plan 3: back-patching pattern):
            //   pre_block: Goto(header)
            //   header_block: evaluate condition, SwitchInt -> body or after
            //   body_block: execute body, Goto -> header
            //   after_block: continue

            // Finish pre-block with placeholder — we patch it to Goto(header).
            let pre_block = ctx.finish_block(Terminator::Unreachable);

            // Header block: evaluate condition.
            let header_id = ctx.next_block;
            let cond_op = lower_expr(ctx, cond);
            // Finish header with placeholder — patch after body is lowered.
            let header_exit = ctx.finish_block(Terminator::Unreachable);

            // Body block: lower body, then Goto back to header.
            let body_id = ctx.next_block;
            lower_stmt(ctx, body);
            let _body_exit = ctx.finish_block(Terminator::Goto(header_id));

            // After block is the next block to be created.
            let after_id = ctx.next_block;

            // Back-patch: pre → Goto(header)
            ctx.patch_terminator(pre_block, Terminator::Goto(header_id));
            // Back-patch: header → SwitchInt(body, otherwise=after)
            ctx.patch_terminator(
                header_exit,
                Terminator::SwitchInt {
                    discr: cond_op,
                    targets: vec![(1, body_id)],
                    otherwise: after_id,
                },
            );

            None
        }
        HirStmt::Acquire { body, .. } => {
            // For now, just lower the body (resource tracking is handled elsewhere).
            lower_stmt(ctx, body)
        }
        HirStmt::Expr(expr) => Some(lower_expr(ctx, expr)),
    }
}

/// Lower a HirExpr to an Operand, emitting MIR statements as needed.
fn lower_place(ctx: &mut LowerCtx, expr: &HirExpr) -> Option<Place> {
    match expr {
        HirExpr::Variable(name) => Some(ctx.lookup_var(name)),
        HirExpr::FieldAccess(base, field) => Some(Place::Field(
            Box::new(lower_place(ctx, base)?),
            field.clone(),
        )),
        _ => None,
    }
}

fn lower_call_args(ctx: &mut LowerCtx, name: &str, args: &[HirExpr]) -> (Vec<Operand>, Vec<Local>) {
    let binder = if matches!(
        name,
        "forall" | "exists" | "sum" | "all" | "any" | "prod" | "count"
    ) && args.len() >= 3
    {
        match &args[0] {
            HirExpr::Variable(v) => Some(v.clone()),
            _ => None,
        }
    } else {
        None
    };
    let saved_prior = binder.as_ref().and_then(|b| ctx.var_map.get(b).cloned());
    let binder_local = binder
        .as_ref()
        .map(|b| ctx.alloc_local(Some(b.clone()), Some("i64".to_string())));
    if let Some(b) = &binder {
        match &saved_prior {
            Some(l) => {
                ctx.var_map.insert(b.clone(), l.clone());
            }
            None => {
                ctx.var_map.remove(b);
            }
        }
    }

    let mut loan_holders = Vec::new();
    let arg_ops = args
        .iter()
        .enumerate()
        .map(|(i, arg_expr)| {
            if i == 0 {
                if let Some(local) = &binder_local {
                    return Operand::Place(Place::Local(local.clone()));
                }
            }
            if i == args.len() - 1 {
                if let (Some(b), Some(local)) = (&binder, &binder_local) {
                    let saved = ctx.var_map.insert(b.clone(), local.clone());
                    let op = lower_expr(ctx, arg_expr);
                    match saved {
                        Some(l) => {
                            ctx.var_map.insert(b.clone(), l);
                        }
                        None => {
                            ctx.var_map.remove(b);
                        }
                    }
                    return op;
                }
            }

            let mode = ctx.call_arg_mode(name, i);
            let arg = if matches!(mode, MirParamMode::Shared | MirParamMode::Mut) {
                lower_place(ctx, arg_expr)
                    .map(Operand::Place)
                    .unwrap_or_else(|| lower_expr(ctx, arg_expr))
            } else {
                lower_expr(ctx, arg_expr)
            };
            match mode {
                MirParamMode::Shared | MirParamMode::Mut => {
                    let place = match arg {
                        Operand::Place(place) | Operand::Move(place) => place,
                        Operand::Constant(_) => return arg,
                    };
                    let holder = ctx.alloc_temp();
                    ctx.emit(MirStatement::StorageLive(holder.clone()));
                    let rvalue = if mode == MirParamMode::Mut {
                        Rvalue::RefMut(place)
                    } else {
                        Rvalue::Ref(place)
                    };
                    ctx.emit(MirStatement::Assign(Place::Local(holder.clone()), rvalue));
                    loan_holders.push(holder.clone());
                    Operand::Place(Place::Local(holder))
                }
                MirParamMode::Consume => match arg {
                    Operand::Place(place) => Operand::Move(place),
                    other => other,
                },
                MirParamMode::Owned => arg,
            }
        })
        .collect();

    (arg_ops, loan_holders)
}

fn lower_expr(ctx: &mut LowerCtx, expr: &HirExpr) -> Operand {
    match expr {
        HirExpr::Number(n) => Operand::Constant(MirConstant::Int(*n)),
        HirExpr::Float(f) => Operand::Constant(MirConstant::Float(*f)),
        // Plan 9: String literal lowering to MIR
        HirExpr::StringLit(s) => Operand::Constant(MirConstant::Str(s.clone())),
        HirExpr::Variable(name) => {
            if name == "true" {
                Operand::Constant(MirConstant::Bool(true))
            } else if name == "false" {
                Operand::Constant(MirConstant::Bool(false))
            } else if ctx.is_nullary_enum_variant(name) {
                // Bare nullary enum variant (e.g. `Red`) — a fresh value,
                // not an unbound variable.
                Operand::Constant(MirConstant::Int(0))
            } else {
                Operand::Place(ctx.lookup_var(name))
            }
        }
        HirExpr::BinaryOp(lhs, op, rhs) => {
            let l = lower_expr(ctx, lhs);
            let r = lower_expr(ctx, rhs);
            let tmp = ctx.alloc_temp();
            ctx.emit(MirStatement::StorageLive(tmp.clone()));
            ctx.emit(MirStatement::Assign(
                Place::Local(tmp.clone()),
                Rvalue::BinaryOp(op.clone(), l, r),
            ));
            Operand::Place(Place::Local(tmp))
        }
        HirExpr::Call { name, args, .. } => {
            let call_name = name.replace('.', "::");
            let (arg_ops, loan_holders) = lower_call_args(ctx, name, args);
            let tmp = ctx.alloc_temp();
            ctx.emit(MirStatement::StorageLive(tmp.clone()));
            ctx.emit(MirStatement::Assign(
                Place::Local(tmp.clone()),
                Rvalue::Call {
                    func: call_name,
                    args: arg_ops,
                },
            ));
            for holder in loan_holders {
                ctx.emit(MirStatement::StorageDead(holder));
            }
            Operand::Place(Place::Local(tmp))
        }
        HirExpr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            let cond_op = lower_expr(ctx, cond);
            let result_local = ctx.alloc_temp();
            ctx.emit(MirStatement::StorageLive(result_local.clone()));

            // Plan 3: Use back-patching pattern for block IDs.
            // Finish cond block with a placeholder — we patch it after lowering branches.
            let cond_block = ctx.finish_block(Terminator::Unreachable);

            // Then block: lower branch, record first block ID.
            let then_id = ctx.next_block;
            let then_val =
                lower_stmt(ctx, then_branch).unwrap_or(Operand::Constant(MirConstant::Int(0)));
            ctx.emit(MirStatement::Assign(
                Place::Local(result_local.clone()),
                Rvalue::Use(then_val),
            ));
            let then_exit = ctx.finish_block(Terminator::Unreachable); // placeholder

            // Else block: lower branch, record first block ID.
            let else_id = ctx.next_block;
            let else_val =
                lower_stmt(ctx, else_branch).unwrap_or(Operand::Constant(MirConstant::Int(0)));
            ctx.emit(MirStatement::Assign(
                Place::Local(result_local.clone()),
                Rvalue::Use(else_val),
            ));
            let else_exit = ctx.finish_block(Terminator::Unreachable); // placeholder

            // Merge block is the next block to be created.
            let merge_id = ctx.next_block;

            // Back-patch: cond → SwitchInt(then, otherwise=else)
            ctx.patch_terminator(
                cond_block,
                Terminator::SwitchInt {
                    discr: cond_op,
                    targets: vec![(1, then_id)],
                    otherwise: else_id,
                },
            );
            // Back-patch: then exit → Goto(merge)
            ctx.patch_terminator(then_exit, Terminator::Goto(merge_id));
            // Back-patch: else exit → Goto(merge)
            ctx.patch_terminator(else_exit, Terminator::Goto(merge_id));

            Operand::Place(Place::Local(result_local))
        }
        HirExpr::StructInit { type_name, fields } => {
            let field_ops: Vec<(String, Operand)> = fields
                .iter()
                .map(|(name, expr)| (name.clone(), lower_expr(ctx, expr)))
                .collect();
            let tmp = ctx.alloc_temp();
            ctx.emit(MirStatement::StorageLive(tmp.clone()));
            ctx.emit(MirStatement::Assign(
                Place::Local(tmp.clone()),
                Rvalue::StructInit {
                    type_name: type_name.clone(),
                    fields: field_ops,
                },
            ));
            Operand::Place(Place::Local(tmp))
        }
        HirExpr::FieldAccess(base, field) => {
            let base_op = if let HirExpr::Variable(name) = base.as_ref() {
                if let Some(resource) = name.strip_prefix("__mumei_resource_") {
                    Operand::Place(ctx.lookup_resource(resource))
                } else {
                    lower_expr(ctx, base)
                }
            } else {
                lower_expr(ctx, base)
            };
            let tmp = ctx.alloc_temp();
            ctx.emit(MirStatement::StorageLive(tmp.clone()));
            ctx.emit(MirStatement::Assign(
                Place::Local(tmp.clone()),
                Rvalue::FieldAccess(base_op, field.clone()),
            ));
            Operand::Place(Place::Local(tmp))
        }
        HirExpr::Perform {
            effect,
            operation,
            args,
            ..
        } => {
            let arg_ops: Vec<Operand> = args.iter().map(|a| lower_expr(ctx, a)).collect();
            let tmp = ctx.alloc_temp();
            ctx.emit(MirStatement::StorageLive(tmp.clone()));
            ctx.emit(MirStatement::Assign(
                Place::Local(tmp.clone()),
                Rvalue::Perform {
                    effect: effect.clone(),
                    operation: operation.clone(),
                    args: arg_ops,
                },
            ));
            Operand::Place(Place::Local(tmp))
        }
        HirExpr::ArrayLit(elements) => {
            let elem_ops: Vec<Operand> = elements.iter().map(|e| lower_expr(ctx, e)).collect();
            let tmp = ctx.alloc_temp();
            ctx.emit(MirStatement::StorageLive(tmp.clone()));
            ctx.emit(MirStatement::Assign(
                Place::Local(tmp.clone()),
                Rvalue::ArrayLit(elem_ops),
            ));
            Operand::Place(Place::Local(tmp))
        }
        HirExpr::ArrayAccess(name, idx) => {
            let idx_op = lower_expr(ctx, idx);
            let base_local = ctx.lookup_var(name);
            match (base_local, idx_op) {
                (Place::Local(base), Operand::Place(Place::Local(idx_local))) => {
                    Operand::Place(Place::Index(Box::new(Place::Local(base)), idx_local))
                }
                (base_place, idx_operand) => {
                    // Fallback: store index into a temp, then build Index place.
                    let idx_tmp = ctx.alloc_temp();
                    ctx.emit(MirStatement::StorageLive(idx_tmp.clone()));
                    ctx.emit(MirStatement::Assign(
                        Place::Local(idx_tmp.clone()),
                        Rvalue::Use(idx_operand),
                    ));
                    Operand::Place(Place::Index(Box::new(base_place), idx_tmp))
                }
            }
        }
        // --- Plan 2: Remaining expression form lowering ---
        HirExpr::Match { target, arms } => {
            // Lower match target to a discriminant operand.
            let discr_op = lower_expr(ctx, target);
            let scrutinee_ty = ctx.infer_hir_ty(target);
            let result_local = ctx.alloc_temp();
            ctx.emit(MirStatement::StorageLive(result_local.clone()));

            // Build arm blocks: for each arm, create a block that evaluates the body.
            // Use SwitchInt for literal patterns; last arm (or wildcard) is otherwise.
            let mut arm_targets: Vec<(i64, BasicBlockId)> = Vec::new();
            let mut arm_block_ids: Vec<BasicBlockId> = Vec::new();

            // Reserve block IDs: we finish the current block, then create arm blocks.
            // First, finish the current block with a placeholder — we'll patch it.
            let switch_block_id = ctx.next_block;

            // Pre-scan arms to determine block layout.
            // Each arm gets one or more blocks; we record start IDs after lowering.
            // Strategy: finish current block with Unreachable, then lower arms,
            // then patch the SwitchInt terminator.
            let _switch_block = ctx.finish_block(Terminator::Unreachable);

            let mut otherwise_id: Option<BasicBlockId> = None;

            for arm in arms {
                let arm_start = ctx.next_block;
                arm_block_ids.push(arm_start);

                // Bind pattern variables to locals. Without this, names bound
                // by the pattern (e.g. `Mk(a, s, f)`) have no entry in var_map
                // and `lookup_var` falls back to Local(0) — the parameter —
                // producing spurious use-after-move violations on the scrutinee's
                // source binding.
                // Pattern bindings are scoped to the arm: a bound name that
                // shadows an outer variable must not leak past the arm, so the
                // var_map is restored after the arm body is lowered.
                let saved_var_map = ctx.var_map.clone();
                lower_pattern_bindings(ctx, &arm.pattern, &discr_op, scrutinee_ty.as_deref());

                // Lower the arm body.
                let arm_val =
                    lower_stmt(ctx, &arm.body).unwrap_or(Operand::Constant(MirConstant::Int(0)));
                ctx.emit(MirStatement::Assign(
                    Place::Local(result_local.clone()),
                    Rvalue::Use(arm_val),
                ));
                // Goto merge — will be patched after all arms.
                ctx.finish_block(Terminator::Unreachable); // placeholder

                // Map pattern to integer target.
                match &arm.pattern {
                    crate::parser::Pattern::Literal(n) => {
                        arm_targets.push((*n, arm_start));
                    }
                    crate::parser::Pattern::Wildcard => {
                        otherwise_id = Some(arm_start);
                    }
                    crate::parser::Pattern::Variable(_) => {
                        // Variable pattern binds the value — treat as otherwise.
                        otherwise_id = Some(arm_start);
                    }
                    crate::parser::Pattern::Variant { .. } => {
                        // Variant pattern — use variant index if available.
                        // For now, treat as otherwise fallback.
                        otherwise_id = Some(arm_start);
                    }
                }

                // Restore the pre-arm scope: pattern bindings (and any `let`s
                // inside the arm body) do not leak to sibling arms or to code
                // after the match.
                ctx.var_map = saved_var_map;
            }

            // Merge block.
            let merge_id = ctx.next_block;

            // Patch the SwitchInt terminator on the switch block.
            let otherwise_target = otherwise_id.unwrap_or(merge_id);
            if let Some(block) = ctx.blocks.get_mut(switch_block_id) {
                block.terminator = Terminator::SwitchInt {
                    discr: discr_op,
                    targets: arm_targets,
                    otherwise: otherwise_target,
                };
            }

            // Patch all arm exit blocks to Goto(merge_id).
            // Patch all Unreachable blocks after switch_block_id to Goto(merge_id).
            for block in ctx.blocks.iter_mut() {
                if block.id > switch_block_id && matches!(block.terminator, Terminator::Unreachable)
                {
                    block.terminator = Terminator::Goto(merge_id);
                }
            }

            Operand::Place(Place::Local(result_local))
        }

        HirExpr::AtomRef { name } => {
            // Atom reference — a first-class function pointer.
            Operand::Constant(MirConstant::FuncRef(name.clone()))
        }

        HirExpr::CallRef { callee, args } => {
            // Indirect call through a callee expression.
            let callee_op = lower_expr(ctx, callee);
            let tmp = ctx.alloc_temp();
            ctx.emit(MirStatement::StorageLive(tmp.clone()));

            // If callee is a FuncRef constant, extract the name for a direct call.
            let (func_name, arg_ops, loan_holders) = match &callee_op {
                Operand::Constant(MirConstant::FuncRef(name)) => {
                    if ctx.has_known_callee(name) {
                        let (arg_ops, loan_holders) = lower_call_args(ctx, name, args);
                        (name.replace('.', "::"), arg_ops, loan_holders)
                    } else {
                        (
                            name.replace('.', "::"),
                            args.iter().map(|arg| lower_expr(ctx, arg)).collect(),
                            Vec::new(),
                        )
                    }
                }
                _ => (
                    "__indirect_call".to_string(),
                    args.iter().map(|arg| lower_expr(ctx, arg)).collect(),
                    Vec::new(),
                ),
            };
            ctx.emit(MirStatement::Assign(
                Place::Local(tmp.clone()),
                Rvalue::Call {
                    func: func_name,
                    args: arg_ops,
                },
            ));
            for holder in loan_holders {
                ctx.emit(MirStatement::StorageDead(holder));
            }
            Operand::Place(Place::Local(tmp))
        }

        HirExpr::Lambda {
            params,
            body,
            captures,
            ..
        } => {
            // Lambda: lower captures and body inline.
            // Capture references are already in scope from the enclosing function.
            // Lower the body as statements and return the result.
            let _capture_refs: Vec<Place> = captures.iter().map(|c| ctx.lookup_var(c)).collect();
            // Lambda parameters are bound names inside the body — register them
            // so they don't fall into the unbound fallback (and so uses of a
            // same-named outer variable inside the body resolve to the param).
            let saved: Vec<(String, Option<Local>)> = params
                .iter()
                .map(|p| (p.name.clone(), ctx.var_map.get(&p.name).cloned()))
                .collect();
            for p in params {
                let ty = p.type_ref.as_ref().map(|t| t.to_string());
                let local = ctx.alloc_local(Some(p.name.clone()), ty);
                ctx.var_map.insert(p.name.clone(), local);
            }
            let result = lower_stmt(ctx, body).unwrap_or(Operand::Constant(MirConstant::Int(0)));
            for (name, prior) in saved {
                match prior {
                    Some(local) => ctx.var_map.insert(name, local),
                    None => ctx.var_map.remove(&name),
                };
            }
            result
        }

        HirExpr::Async { body } => {
            // Async block: lower body inline for MIR analysis purposes.
            // Full async lowering (coroutine transform) deferred to codegen.
            lower_stmt(ctx, body).unwrap_or(Operand::Constant(MirConstant::Int(0)))
        }

        HirExpr::Await { expr } => {
            // Await: lower the inner expression.
            // Full suspension point lowering deferred to codegen.
            lower_expr(ctx, expr)
        }

        HirExpr::Task { body, .. } => {
            // Task: lower body inline for ownership/move analysis.
            lower_stmt(ctx, body).unwrap_or(Operand::Constant(MirConstant::Int(0)))
        }

        HirExpr::TaskGroup { children, .. } => {
            // TaskGroup: lower each child task sequentially.
            // The result is the last child's value.
            let mut last_op = Operand::Constant(MirConstant::Int(0));
            for child in children {
                if let Some(op) = lower_stmt(ctx, child) {
                    last_op = op;
                }
            }
            last_op
        }

        // Plan 8: Channel send — lower value, emit as call-like operation
        HirExpr::ChanSend { channel, value } => {
            let _ch = lower_expr(ctx, channel);
            let _val = lower_expr(ctx, value);
            Operand::Constant(MirConstant::Int(0))
        }

        // Plan 8: Channel recv — lower channel, return placeholder
        HirExpr::ChanRecv { channel } => {
            let _ch = lower_expr(ctx, channel);
            Operand::Constant(MirConstant::Int(0))
        }

        // Plan 14: Enum variant construction — lower fields and emit as call-like
        HirExpr::VariantInit {
            enum_name,
            variant_name,
            fields,
        } => {
            let field_ops: Vec<Operand> = fields.iter().map(|f| lower_expr(ctx, f)).collect();
            let tmp = ctx.alloc_temp();
            ctx.emit(MirStatement::StorageLive(tmp.clone()));
            ctx.emit(MirStatement::Assign(
                Place::Local(tmp.clone()),
                Rvalue::Call {
                    func: format!("{}::{}", enum_name, variant_name),
                    args: field_ops,
                },
            ));
            Operand::Place(Place::Local(tmp))
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use crate::hir::lower_atom_to_hir;
    use crate::parser::{self, Atom, Expr, Param, Span, Stmt, TrustLevel};

    /// Helper: create a minimal Atom with given body expression and params.
    fn make_atom(name: &str, params: Vec<Param>, body_expr: &str) -> Atom {
        Atom {
            name: name.to_string(),
            type_params: vec![],
            where_bounds: vec![],
            params,
            trace_id: None,
            spec_metadata: std::collections::HashMap::new(),
            requires: "true".to_string(),
            forall_constraints: vec![],
            ensures: "true".to_string(),
            body_expr: body_expr.to_string(),
            consumed_params: vec![],
            resources: vec![],
            is_async: false,
            trust_level: TrustLevel::Verified,
            max_unroll: None,
            invariant: None,
            effects: vec![],
            return_type: None,
            span: Span::new("", 1, 1, 0),
            effect_pre: std::collections::HashMap::new(),
            effect_post: std::collections::HashMap::new(),
        }
    }

    fn make_param(name: &str, ty: &str) -> Param {
        Param {
            name: name.to_string(),
            type_name: Some(ty.to_string()),
            type_ref: Some(parser::parse_type_ref(ty)),
            is_ref: false,
            is_ref_mut: false,
            fn_contract_requires: None,
            fn_contract_ensures: None,
        }
    }

    fn infer_body_expr_type(atom: &Atom) -> Option<String> {
        infer_atom_return_type(atom)
    }

    #[test]
    fn test_infer_hir_ty_array_access_typed() {
        let atom = make_atom(
            "array_access_typed",
            vec![make_param("arr", "[i64]"), make_param("i", "i64")],
            "arr[i]",
        );
        assert_eq!(infer_body_expr_type(&atom), Some("i64".to_string()));
    }

    #[test]
    fn test_infer_hir_ty_array_access_f64() {
        let atom = make_atom(
            "array_access_f64",
            vec![make_param("arr", "[f64]"), make_param("i", "i64")],
            "arr[i]",
        );
        assert_eq!(infer_body_expr_type(&atom), Some("f64".to_string()));
    }

    #[test]
    fn test_infer_hir_ty_array_access_untyped_fallback() {
        let atom = make_atom(
            "array_access_untyped",
            vec![make_param("i", "i64")],
            "arr[i]",
        );
        assert_eq!(infer_body_expr_type(&atom), Some("i64".to_string()));
    }

    #[test]
    fn test_parse_body_block_with_let_tail_expr() {
        let stmt = parser::parse_body_expr("{ let x = a + b; x }");
        match stmt {
            Stmt::Block(stmts, _) => {
                assert_eq!(stmts.len(), 2);
                match &stmts[0] {
                    Stmt::Let { var, .. } => assert_eq!(var, "x"),
                    other => panic!("expected let statement, got {:?}", other),
                }
                match &stmts[1] {
                    Stmt::Expr(Expr::Variable(name), _) => assert_eq!(name, "x"),
                    other => panic!("expected tail expr `x`, got {:?}", other),
                }
            }
            other => panic!("expected block statement, got {:?}", other),
        }
    }

    #[test]
    fn test_infer_hir_ty_block_let_tail_f64() {
        let atom = make_atom(
            "block_let_tail_f64",
            vec![make_param("a", "f64"), make_param("b", "f64")],
            "{ let x = a + b; x }",
        );
        assert_eq!(infer_body_expr_type(&atom), Some("f64".to_string()));
    }

    #[test]
    fn test_infer_hir_ty_comparison_returns_bool() {
        let atom = make_atom(
            "comparison_returns_bool",
            vec![make_param("a", "f64"), make_param("b", "f64")],
            "a < b",
        );
        assert_eq!(infer_body_expr_type(&atom), Some("bool".to_string()));
    }

    #[test]
    fn test_lower_simple_addition() {
        // atom add(a: Int, b: Int) body: a + b
        let atom = make_atom(
            "add",
            vec![make_param("a", "Int"), make_param("b", "Int")],
            "a + b",
        );
        let hir = lower_atom_to_hir(&atom);
        let mir = lower_hir_to_mir(&hir);

        assert_eq!(mir.name, "add");
        assert_eq!(mir.entry_block, 0);
        // Should have at least 1 block.
        assert!(!mir.blocks.is_empty());
        // Should have locals for params (a, b) + temp for binary op result.
        assert!(mir.locals.len() >= 3);
        // The last block should have a Return terminator.
        let last_block = mir.blocks.last().unwrap();
        assert!(matches!(last_block.terminator, Terminator::Return(_)));
    }

    #[test]
    fn test_lower_if_then_else() {
        // atom max(x: Int, y: Int) body: if x > y { x } else { y }
        let atom = make_atom(
            "max",
            vec![make_param("x", "Int"), make_param("y", "Int")],
            "if x > y { x } else { y }",
        );
        let hir = lower_atom_to_hir(&atom);
        let mir = lower_hir_to_mir(&hir);

        assert_eq!(mir.name, "max");
        // if/else should produce: cond_block, then_block, else_block, merge_block (+ final return)
        // At minimum we expect 4+ blocks.
        assert!(
            mir.blocks.len() >= 4,
            "Expected >= 4 blocks for if/else, got {}",
            mir.blocks.len()
        );

        // Check that at least one block has a SwitchInt terminator.
        let has_switch = mir
            .blocks
            .iter()
            .any(|b| matches!(b.terminator, Terminator::SwitchInt { .. }));
        assert!(has_switch, "Expected a SwitchInt terminator for if/else");
    }

    #[test]
    fn test_lower_let_binding() {
        // atom double(n: Int) body: { let x = n + n; x }
        let atom = make_atom(
            "double",
            vec![make_param("n", "Int")],
            "{ let x = n + n; x }",
        );
        let hir = lower_atom_to_hir(&atom);
        let mir = lower_hir_to_mir(&hir);

        assert_eq!(mir.name, "double");
        // Should have locals: n (param) + temp (n+n) + x (let binding).
        assert!(
            mir.locals.len() >= 3,
            "Expected >= 3 locals, got {}",
            mir.locals.len()
        );

        // Check that there is a StorageLive for the let-bound variable.
        let has_storage_live = mir.blocks.iter().any(|b| {
            b.statements
                .iter()
                .any(|s| matches!(s, MirStatement::StorageLive(_)))
        });
        assert!(has_storage_live, "Expected StorageLive for let binding");
    }

    #[test]
    fn test_lower_function_call() {
        // atom caller(a: Int) body: callee(a)
        let atom = make_atom("caller", vec![make_param("a", "Int")], "callee(a)");
        let hir = lower_atom_to_hir(&atom);
        let mir = lower_hir_to_mir(&hir);

        // Should produce Rvalue::Call in the statements.
        let has_call = mir.blocks.iter().any(|b| {
            b.statements
                .iter()
                .any(|s| matches!(s, MirStatement::Assign(_, Rvalue::Call { .. })))
        });
        assert!(has_call, "Expected Rvalue::Call for function call");
    }

    #[test]
    fn test_lower_while_loop() {
        // atom countdown(n: Int) body: { let x = n; while x > 0 invariant x >= 0 { x = x - 1 }; x }
        let atom = make_atom(
            "countdown",
            vec![make_param("n", "Int")],
            "{ let x = n; while x > 0 invariant x >= 0 { x = x - 1 }; x }",
        );
        let hir = lower_atom_to_hir(&atom);
        let mir = lower_hir_to_mir(&hir);

        // While loop should produce: pre_block -> header -> body -> after
        // Plus the final return block.
        assert!(
            mir.blocks.len() >= 4,
            "Expected >= 4 blocks for while loop, got {}",
            mir.blocks.len()
        );

        // Check for SwitchInt (loop condition) and back-edge Goto.
        let has_switch = mir
            .blocks
            .iter()
            .any(|b| matches!(b.terminator, Terminator::SwitchInt { .. }));
        assert!(has_switch, "Expected SwitchInt for while condition");

        // Check for Goto back-edge (body -> header).
        let goto_count = mir
            .blocks
            .iter()
            .filter(|b| matches!(b.terminator, Terminator::Goto(_)))
            .count();
        assert!(
            goto_count >= 2,
            "Expected >= 2 Goto terminators (pre->header, body->header), got {}",
            goto_count
        );
    }

    #[test]
    fn test_lower_constants() {
        // atom const_test() body: 42
        let atom = make_atom("const_test", vec![], "42");
        let hir = lower_atom_to_hir(&atom);
        let mir = lower_hir_to_mir(&hir);

        // The return should use a constant operand.
        let last_block = mir.blocks.last().unwrap();
        match &last_block.terminator {
            Terminator::Return(Operand::Constant(MirConstant::Int(42))) => {}
            other => panic!("Expected Return(Constant(Int(42))), got {:?}", other),
        }
    }

    // =========================================================================
    // Task 0: MIR analysis budget tests
    // =========================================================================

    #[test]
    fn test_mir_body_metrics() {
        let atom = make_atom(
            "add",
            vec![make_param("a", "Int"), make_param("b", "Int")],
            "a + b",
        );
        let hir = lower_atom_to_hir(&atom);
        let mir = lower_hir_to_mir(&hir);

        assert!(mir.block_count() >= 1);
        assert!(mir.local_count() >= 2);
        assert_eq!(mir.complexity(), mir.block_count() * mir.local_count());
    }

    #[test]
    fn test_mir_analysis_budget_ok() {
        // A simple atom should be well within budget
        let atom = make_atom("simple", vec![make_param("x", "Int")], "x + 1");
        let hir = lower_atom_to_hir(&atom);
        let mir = lower_hir_to_mir(&hir);

        assert!(mir.check_analysis_budget().is_ok());
        assert!(mir.complexity() < MIR_ANALYSIS_COMPLEXITY_LIMIT);
    }

    #[test]
    fn test_mir_analysis_budget_exceeded() {
        // Manually construct a MirBody that exceeds the budget
        let mut locals = Vec::new();
        for i in 0..200 {
            locals.push(LocalDecl {
                local: Local(i),
                name: Some(format!("v{}", i)),
                ty: Some("Int".to_string()),
                movability: Movability::Copy,
                capability: None,
            });
        }
        let mut blocks = Vec::new();
        for i in 0..100 {
            blocks.push(BasicBlock {
                id: i,
                statements: vec![],
                terminator: Terminator::Unreachable,
            });
        }
        let body = MirBody {
            name: "huge".to_string(),
            locals,
            blocks,
            entry_block: 0,
            param_modes: HashMap::new(),
            unbound_names: Vec::new(),
        };

        assert_eq!(body.block_count(), 100);
        assert_eq!(body.local_count(), 200);
        assert_eq!(body.complexity(), 20_000);
        assert!(body.complexity() > MIR_ANALYSIS_COMPLEXITY_LIMIT);

        let err = body.check_analysis_budget();
        assert!(err.is_err());
        let msg = err.unwrap_err();
        assert!(msg.contains("MIR analysis budget exceeded"));
        assert!(msg.contains("huge"));
        assert!(msg.contains("20000"));
    }
}
