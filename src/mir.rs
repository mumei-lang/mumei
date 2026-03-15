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
// See also: docs/ROADMAP.md "Multi-Stage IR Roadmap" section.
// =============================================================================

use crate::hir::{HirAtom, HirExpr, HirStmt};
use crate::parser::Op;
use std::collections::HashMap;

// =============================================================================
// Core MIR types
// =============================================================================

/// A unique identifier for a local variable or temporary.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Local(pub usize);

/// A place in memory (variable or field access).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum Place {
    Local(Local),
    Field(Box<Place>, String),
    Index(Box<Place>, Local),
}

/// Right-hand side of an assignment.
#[derive(Debug, Clone)]
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
}

/// An operand: either a place (variable) or a constant.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum Operand {
    Place(Place),
    Constant(MirConstant),
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum MirConstant {
    Int(i64),
    Float(f64),
    Bool(bool),
}

/// A single MIR statement (three-address code).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum MirStatement {
    Assign(Place, Rvalue),
    StorageLive(Local),
    StorageDead(Local),
    Drop(Local),
    Nop,
}

/// Block terminator: how control flow leaves a basic block.
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
pub struct BasicBlock {
    pub id: BasicBlockId,
    pub statements: Vec<MirStatement>,
    pub terminator: Terminator,
}

/// Upper bound on MIR analysis complexity (block_count * local_count).
/// When exceeded, dataflow analyses are skipped to prevent explosion.
pub const MIR_ANALYSIS_COMPLEXITY_LIMIT: usize = 10_000;

/// A complete MIR body for one atom.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct MirBody {
    pub name: String,
    pub locals: Vec<LocalDecl>,
    pub blocks: Vec<BasicBlock>,
    pub entry_block: BasicBlockId,
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

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct LocalDecl {
    pub local: Local,
    pub name: Option<String>,
    pub ty: Option<String>,
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
    next_local: usize,
    next_block: usize,
}

impl LowerCtx {
    fn new() -> Self {
        Self {
            locals: Vec::new(),
            blocks: Vec::new(),
            current_stmts: Vec::new(),
            var_map: std::collections::HashMap::new(),
            next_local: 0,
            next_block: 0,
        }
    }

    /// Allocate a new local (named or temporary).
    fn alloc_local(&mut self, name: Option<String>, ty: Option<String>) -> Local {
        let local = Local(self.next_local);
        self.next_local += 1;
        self.locals.push(LocalDecl {
            local: local.clone(),
            name: name.clone(),
            ty,
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

    /// Emit a statement into the current (in-progress) block.
    fn emit(&mut self, stmt: MirStatement) {
        self.current_stmts.push(stmt);
    }

    /// Look up a variable by name, returning its Place.
    fn lookup_var(&self, name: &str) -> Place {
        if let Some(local) = self.var_map.get(name) {
            Place::Local(local.clone())
        } else {
            // Variable not yet seen (e.g. atom parameter or free variable).
            // Return a placeholder local 0 — will be refined in later phases.
            Place::Local(Local(0))
        }
    }
}

/// Lower a HirAtom to MirBody.
/// Phase 4b: basic lowering that flattens nested expressions into three-address code.
pub fn lower_hir_to_mir(hir_atom: &HirAtom) -> MirBody {
    let mut ctx = LowerCtx::new();

    // Allocate locals for atom parameters.
    for param in &hir_atom.atom.params {
        let local = ctx.alloc_local(Some(param.name.clone()), param.type_name.clone());
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
    }
}

/// Lower a HirStmt, returning an optional Operand for the value it produces.
fn lower_stmt(ctx: &mut LowerCtx, stmt: &HirStmt) -> Option<Operand> {
    match stmt {
        HirStmt::Let { var, ty, value } => {
            let val_op = lower_expr(ctx, value);
            let local = ctx.alloc_local(Some(var.clone()), ty.clone());
            ctx.emit(MirStatement::StorageLive(local.clone()));
            ctx.emit(MirStatement::Assign(
                Place::Local(local),
                Rvalue::Use(val_op),
            ));
            None
        }
        HirStmt::Assign { var, value } => {
            let val_op = lower_expr(ctx, value);
            let place = ctx.lookup_var(var);
            ctx.emit(MirStatement::Assign(place, Rvalue::Use(val_op)));
            None
        }
        HirStmt::Block { stmts, tail_expr } => {
            for s in stmts {
                lower_stmt(ctx, s);
            }
            tail_expr.as_ref().map(|expr| lower_expr(ctx, expr))
        }
        HirStmt::While { cond, body, .. } => {
            // While loop:
            //   header_block: evaluate condition, SwitchInt -> body or after
            //   body_block: execute body, Goto -> header
            //   after_block: continue

            // Finish the current block with a Goto to the header.
            // finish_block() assigns id = next_block then increments, so the
            // header will be the *next* block after the pre-block.
            let header_id = ctx.next_block + 1;
            let _pre_block = ctx.finish_block(Terminator::Goto(header_id));

            // Header block: evaluate condition.
            let cond_op = lower_expr(ctx, cond);
            let body_id = header_id + 1;
            let after_id = header_id + 2;
            let _header = ctx.finish_block(Terminator::SwitchInt {
                discr: cond_op,
                targets: vec![(1, body_id)],
                otherwise: after_id,
            });

            // Body block.
            lower_stmt(ctx, body);
            let _body = ctx.finish_block(Terminator::Goto(header_id));

            // After block is the new current block (will be finished by the caller).
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
fn lower_expr(ctx: &mut LowerCtx, expr: &HirExpr) -> Operand {
    match expr {
        HirExpr::Number(n) => Operand::Constant(MirConstant::Int(*n)),
        HirExpr::Float(f) => Operand::Constant(MirConstant::Float(*f)),
        HirExpr::Variable(name) => {
            if name == "true" {
                Operand::Constant(MirConstant::Bool(true))
            } else if name == "false" {
                Operand::Constant(MirConstant::Bool(false))
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
            let arg_ops: Vec<Operand> = args.iter().map(|a| lower_expr(ctx, a)).collect();
            let tmp = ctx.alloc_temp();
            ctx.emit(MirStatement::StorageLive(tmp.clone()));
            ctx.emit(MirStatement::Assign(
                Place::Local(tmp.clone()),
                Rvalue::Call {
                    func: name.clone(),
                    args: arg_ops,
                },
            ));
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

            // Finish current block with SwitchInt.
            // TODO: Block ID pre-computation assumes each branch produces exactly
            // one block. Nested control flow (e.g., `if a { if b { 1 } else { 2 } }
            // else { 3 }`) creates additional blocks inside lower_stmt, causing
            // else_id and merge_id to point to wrong blocks. Fix by switching to a
            // forward-declaration / back-patching pattern for block IDs.
            let then_id = ctx.next_block + 1;
            let else_id = ctx.next_block + 2;
            let merge_id = ctx.next_block + 3;
            let _cond_block = ctx.finish_block(Terminator::SwitchInt {
                discr: cond_op,
                targets: vec![(1, then_id)],
                otherwise: else_id,
            });

            // Then block.
            let then_val =
                lower_stmt(ctx, then_branch).unwrap_or(Operand::Constant(MirConstant::Int(0)));
            ctx.emit(MirStatement::Assign(
                Place::Local(result_local.clone()),
                Rvalue::Use(then_val),
            ));
            let _then = ctx.finish_block(Terminator::Goto(merge_id));

            // Else block.
            let else_val =
                lower_stmt(ctx, else_branch).unwrap_or(Operand::Constant(MirConstant::Int(0)));
            ctx.emit(MirStatement::Assign(
                Place::Local(result_local.clone()),
                Rvalue::Use(else_val),
            ));
            let _else = ctx.finish_block(Terminator::Goto(merge_id));

            // Merge block is the new current block.
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
            let base_op = lower_expr(ctx, base);
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
        // For complex expressions not yet lowered, emit a placeholder constant.
        // These will be expanded in future phases.
        HirExpr::Match { .. }
        | HirExpr::AtomRef { .. }
        | HirExpr::CallRef { .. }
        | HirExpr::Async { .. }
        | HirExpr::Await { .. }
        | HirExpr::Task { .. }
        | HirExpr::TaskGroup { .. }
        | HirExpr::Lambda { .. } => {
            // TODO: Phase 4c — lower remaining expression forms
            Operand::Constant(MirConstant::Int(0))
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
    use crate::parser::{self, Atom, Param, Span, TrustLevel};

    /// Helper: create a minimal Atom with given body expression and params.
    fn make_atom(name: &str, params: Vec<Param>, body_expr: &str) -> Atom {
        Atom {
            name: name.to_string(),
            type_params: vec![],
            where_bounds: vec![],
            params,
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
            span: Span::new("", 1, 1, 0),
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
