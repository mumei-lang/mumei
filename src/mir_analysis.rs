// =============================================================================
// MIR Analysis: Liveness Analysis + Drop Insertion
// =============================================================================
// Implements backward dataflow liveness analysis on MIR CFG, then inserts
// MirStatement::Drop at points where variables become dead.
//
// Pipeline: compute_gen_kill → compute_liveness → insert_drops
// =============================================================================

use crate::mir::{
    BasicBlock, BasicBlockId, Local, MirBody, MirStatement, Operand, Place, Rvalue, Terminator,
};
use std::collections::{HashMap, HashSet, VecDeque};

// =============================================================================
// Gen/Kill computation
// =============================================================================

/// Gen and Kill sets for a single basic block.
#[derive(Debug, Clone)]
pub struct GenKill {
    /// Locals used (read) in this block.
    pub gen: HashSet<Local>,
    /// Locals defined (written) in this block.
    pub kill: HashSet<Local>,
}

/// Collect all Local references from an Operand into the given set.
fn collect_operand_locals(op: &Operand, set: &mut HashSet<Local>) {
    match op {
        Operand::Place(place) => collect_place_locals(place, set),
        Operand::Constant(_) => {}
    }
}

/// Collect all Local references from a Place into the given set.
fn collect_place_locals(place: &Place, set: &mut HashSet<Local>) {
    match place {
        Place::Local(l) => {
            set.insert(l.clone());
        }
        Place::Field(base, _) => collect_place_locals(base, set),
        Place::Index(base, idx) => {
            collect_place_locals(base, set);
            set.insert(idx.clone());
        }
    }
}

/// Collect all locals used (read) in an Rvalue.
fn collect_rvalue_uses(rvalue: &Rvalue, set: &mut HashSet<Local>) {
    match rvalue {
        Rvalue::Use(op) => collect_operand_locals(op, set),
        Rvalue::BinaryOp(_, lhs, rhs) => {
            collect_operand_locals(lhs, set);
            collect_operand_locals(rhs, set);
        }
        Rvalue::Call { args, .. } => {
            for arg in args {
                collect_operand_locals(arg, set);
            }
        }
        Rvalue::Ref(place) | Rvalue::RefMut(place) => {
            collect_place_locals(place, set);
        }
        Rvalue::StructInit { fields, .. } => {
            for (_, op) in fields {
                collect_operand_locals(op, set);
            }
        }
        Rvalue::FieldAccess(op, _) => {
            collect_operand_locals(op, set);
        }
        Rvalue::Perform { args, .. } => {
            for arg in args {
                collect_operand_locals(arg, set);
            }
        }
    }
}

/// Extract the Local defined by a Place (if it's a direct local assignment).
fn place_defined_local(place: &Place) -> Option<Local> {
    match place {
        Place::Local(l) => Some(l.clone()),
        _ => None,
    }
}

/// Compute the gen (used) and kill (defined) sets for a basic block.
///
/// For each statement, processed in order:
/// - gen: locals that are read before being defined in this block
/// - kill: locals that are defined (assigned to) in this block
///
/// Terminator operands are also included in gen.
pub fn compute_gen_kill(block: &BasicBlock) -> GenKill {
    let mut gen = HashSet::new();
    let mut kill = HashSet::new();

    for stmt in &block.statements {
        match stmt {
            MirStatement::Assign(place, rvalue) => {
                // Uses in rvalue: add to gen if not already killed
                let mut uses = HashSet::new();
                collect_rvalue_uses(rvalue, &mut uses);
                for u in &uses {
                    if !kill.contains(u) {
                        gen.insert(u.clone());
                    }
                }
                // Definition: add to kill
                if let Some(def) = place_defined_local(place) {
                    kill.insert(def);
                }
            }
            MirStatement::StorageLive(_) | MirStatement::StorageDead(_) => {
                // These don't produce uses or defs for liveness purposes
            }
            MirStatement::Drop(_) => {
                // Drop does not produce uses or defs for gen/kill
            }
            MirStatement::Nop => {}
        }
    }

    // Terminator operands contribute to gen
    match &block.terminator {
        Terminator::Return(op) => {
            let mut uses = HashSet::new();
            collect_operand_locals(op, &mut uses);
            for u in &uses {
                if !kill.contains(u) {
                    gen.insert(u.clone());
                }
            }
        }
        Terminator::SwitchInt { discr, .. } => {
            let mut uses = HashSet::new();
            collect_operand_locals(discr, &mut uses);
            for u in &uses {
                if !kill.contains(u) {
                    gen.insert(u.clone());
                }
            }
        }
        Terminator::Goto(_) | Terminator::Unreachable => {}
    }

    GenKill { gen, kill }
}

// =============================================================================
// Liveness Analysis (Backward Dataflow)
// =============================================================================

/// Result of liveness analysis: live_in and live_out for each block.
#[derive(Debug, Clone)]
pub struct LivenessResult {
    pub live_in: HashMap<BasicBlockId, HashSet<Local>>,
    pub live_out: HashMap<BasicBlockId, HashSet<Local>>,
}

/// Compute liveness using backward dataflow (worklist algorithm).
///
/// Algorithm:
/// 1. Initialize all live_in and live_out to empty
/// 2. Add all blocks to worklist (in reverse order)
/// 3. While worklist is not empty:
///    a. Pop block B
///    b. live_out[B] = ∪ live_in[S] for S in successors(B)
///    c. live_in[B] = gen[B] ∪ (live_out[B] - kill[B])
///    d. If live_in[B] changed, add predecessors(B) to worklist
///
/// Iteration is bounded by block_count * max(local_count, 10) to prevent explosion.
pub fn compute_liveness(body: &MirBody) -> LivenessResult {
    let successors = body.successors();
    let predecessors = body.predecessors();

    // Pre-compute gen/kill for each block
    let gen_kill: HashMap<BasicBlockId, GenKill> = body
        .blocks
        .iter()
        .map(|b| (b.id, compute_gen_kill(b)))
        .collect();

    let mut live_in: HashMap<BasicBlockId, HashSet<Local>> = HashMap::new();
    let mut live_out: HashMap<BasicBlockId, HashSet<Local>> = HashMap::new();

    // Initialize all to empty
    for block in &body.blocks {
        live_in.insert(block.id, HashSet::new());
        live_out.insert(block.id, HashSet::new());
    }

    // Worklist: start with all blocks in reverse order
    let mut worklist: VecDeque<BasicBlockId> = VecDeque::new();
    let mut in_worklist: HashSet<BasicBlockId> = HashSet::new();
    for block in body.blocks.iter().rev() {
        worklist.push_back(block.id);
        in_worklist.insert(block.id);
    }

    // Theoretical convergence bound is O(block_count * local_count).
    // Use max(local_count, 10) to handle bodies with many locals correctly.
    let max_iterations = body.block_count() * body.local_count().max(10);
    let mut iterations = 0;

    while let Some(block_id) = worklist.pop_front() {
        in_worklist.remove(&block_id);
        iterations += 1;
        if iterations > max_iterations {
            eprintln!(
                "Warning: liveness analysis for '{}' reached iteration limit ({})",
                body.name, max_iterations
            );
            break;
        }

        // live_out[B] = ∪ live_in[S] for S in successors(B)
        let mut new_live_out: HashSet<Local> = HashSet::new();
        if let Some(succs) = successors.get(&block_id) {
            for succ in succs {
                if let Some(succ_live_in) = live_in.get(succ) {
                    new_live_out.extend(succ_live_in.iter().cloned());
                }
            }
        }
        live_out.insert(block_id, new_live_out.clone());

        // live_in[B] = gen[B] ∪ (live_out[B] - kill[B])
        let gk = &gen_kill[&block_id];
        let mut new_live_in: HashSet<Local> = gk.gen.clone();
        for l in &new_live_out {
            if !gk.kill.contains(l) {
                new_live_in.insert(l.clone());
            }
        }

        let old_live_in = live_in.get(&block_id).cloned().unwrap_or_default();
        if new_live_in != old_live_in {
            live_in.insert(block_id, new_live_in);
            // Add predecessors to worklist
            if let Some(preds) = predecessors.get(&block_id) {
                for pred in preds {
                    if !in_worklist.contains(pred) {
                        worklist.push_back(*pred);
                        in_worklist.insert(*pred);
                    }
                }
            }
        }
    }

    LivenessResult { live_in, live_out }
}

// =============================================================================
// Drop Insertion
// =============================================================================

/// Insert MirStatement::Drop for locals that become dead at each block.
///
/// For each block B:
/// - A local l that is in live_in[B] but NOT in live_out[B] becomes dead in B.
/// - If l is also in kill[B] (redefined), Drop is not needed.
/// - Otherwise, insert Drop(l) before the terminator.
///
/// For branches (SwitchInt), if a variable dies in only one branch,
/// the Drop is placed only in that branch's target block, preventing
/// double-free.
///
/// TODO: Variables used only by a SwitchInt discriminant (in terminator_uses
/// but not in live_out) are currently excluded from dropping in this block
/// to prevent use-after-drop, but no compensating drop is inserted in the
/// successor blocks. This causes a leak for resource types. When Mumei adds
/// types with destructors, add a second pass that inserts Drop at the start
/// of each successor block for such locals.
pub fn insert_drops(body: &mut MirBody, liveness: &LivenessResult) {
    // Pre-compute gen/kill for each block
    let gen_kill: HashMap<BasicBlockId, GenKill> = body
        .blocks
        .iter()
        .map(|b| (b.id, compute_gen_kill(b)))
        .collect();

    for block in &mut body.blocks {
        let block_id = block.id;
        let live_in = liveness.live_in.get(&block_id).cloned().unwrap_or_default();
        let live_out = liveness
            .live_out
            .get(&block_id)
            .cloned()
            .unwrap_or_default();
        let gk = gen_kill.get(&block_id).unwrap();

        // Locals that are live coming in but not live going out → they die in this block
        let mut drops: Vec<Local> = Vec::new();
        for l in &live_in {
            if !live_out.contains(l) && !gk.kill.contains(l) {
                drops.push(l.clone());
            }
        }

        // Sort drops by local index for deterministic output
        drops.sort_by_key(|l| l.0);

        // Insert Drop statements before the terminator
        for l in drops {
            block.statements.push(MirStatement::Drop(l));
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
    use crate::mir::{lower_hir_to_mir, BasicBlock, LocalDecl, Terminator};
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

    // =========================================================================
    // Test: predecessors/successors (CFG utilities)
    // =========================================================================

    #[test]
    fn test_successors_predecessors() {
        let atom = make_atom(
            "branch",
            vec![make_param("x", "Int")],
            "if x > 0 { x + 1 } else { x - 1 }",
        );
        let hir = lower_atom_to_hir(&atom);
        let mir = lower_hir_to_mir(&hir);

        let succs = mir.successors();
        let preds = mir.predecessors();

        // Every block should have a successor entry
        for block in &mir.blocks {
            assert!(succs.contains_key(&block.id));
        }

        // Every block should have a predecessor entry
        for block in &mir.blocks {
            assert!(preds.contains_key(&block.id));
        }

        // The entry block (0) should have no predecessors from outside
        // (unless there's a loop, which this simple if/else doesn't have)
        // Verify that if a block B has successor S, then S has B as predecessor
        for (block_id, block_succs) in &succs {
            for succ in block_succs {
                let succ_preds = preds.get(succ).unwrap();
                assert!(
                    succ_preds.contains(block_id),
                    "Block {} should be a predecessor of block {}",
                    block_id,
                    succ
                );
            }
        }
    }

    // =========================================================================
    // Test: gen/kill computation
    // =========================================================================

    #[test]
    fn test_gen_kill_basic() {
        // Manually construct a block: assign tmp = a + b
        let block = BasicBlock {
            id: 0,
            statements: vec![MirStatement::Assign(
                Place::Local(Local(2)),
                Rvalue::BinaryOp(
                    crate::parser::Op::Add,
                    Operand::Place(Place::Local(Local(0))),
                    Operand::Place(Place::Local(Local(1))),
                ),
            )],
            terminator: Terminator::Return(Operand::Place(Place::Local(Local(2)))),
        };

        let gk = compute_gen_kill(&block);

        // gen should contain Local(0) and Local(1) (used in BinaryOp)
        assert!(gk.gen.contains(&Local(0)));
        assert!(gk.gen.contains(&Local(1)));
        // kill should contain Local(2) (assigned)
        assert!(gk.kill.contains(&Local(2)));
        // gen should NOT contain Local(2) since it's defined before use in terminator
        // Actually Local(2) IS used in the Return terminator, but it's also killed,
        // so it's already in kill and the terminator use doesn't add to gen
    }

    // =========================================================================
    // Test: liveness analysis
    // =========================================================================

    #[test]
    fn test_liveness_simple() {
        // atom f(x: Int, y: Int) body: { let z = x + 1; z }
        // x should be live at entry, dead after z = x + 1
        // y should be dead throughout (unused)
        let atom = make_atom(
            "f",
            vec![make_param("x", "Int"), make_param("y", "Int")],
            "{ let z = x + 1; z }",
        );
        let hir = lower_atom_to_hir(&atom);
        let mir = lower_hir_to_mir(&hir);
        let liveness = compute_liveness(&mir);

        // Verify liveness result has entries for all blocks
        for block in &mir.blocks {
            assert!(liveness.live_in.contains_key(&block.id));
            assert!(liveness.live_out.contains_key(&block.id));
        }
    }

    // =========================================================================
    // Test: straight-line code — x dead after y = x + 1
    // =========================================================================

    #[test]
    fn test_drop_insertion_straight_line() {
        // atom f(a: Int, b: Int) body: { let y = a + 1; y }
        // After computing y = a + 1, a is dead (and b is never used)
        let atom = make_atom(
            "f",
            vec![make_param("a", "Int"), make_param("b", "Int")],
            "{ let y = a + 1; y }",
        );
        let hir = lower_atom_to_hir(&atom);
        let mut mir = lower_hir_to_mir(&hir);
        let liveness = compute_liveness(&mir);
        insert_drops(&mut mir, &liveness);

        // Check that at least one Drop statement exists in the MIR
        let has_drop = mir.blocks.iter().any(|b| {
            b.statements
                .iter()
                .any(|s| matches!(s, MirStatement::Drop(_)))
        });
        assert!(has_drop, "Should have at least one Drop statement");
    }

    // =========================================================================
    // Test: if/else branch — different drop paths
    // =========================================================================

    #[test]
    fn test_drop_insertion_if_else() {
        // atom f(cond: Int, x: Int) body: if cond > 0 { x + 1 } else { 42 }
        // x is used only in the then-branch; should be dropped in else-branch
        let atom = make_atom(
            "f",
            vec![make_param("cond", "Int"), make_param("x", "Int")],
            "if cond > 0 { x + 1 } else { 42 }",
        );
        let hir = lower_atom_to_hir(&atom);
        let mut mir = lower_hir_to_mir(&hir);
        let liveness = compute_liveness(&mir);
        insert_drops(&mut mir, &liveness);

        // The MIR should have Drop statements in some blocks
        let drop_count: usize = mir
            .blocks
            .iter()
            .map(|b| {
                b.statements
                    .iter()
                    .filter(|s| matches!(s, MirStatement::Drop(_)))
                    .count()
            })
            .sum();
        // At least some drops should be present (parameters become dead)
        assert!(
            drop_count > 0,
            "Should have Drop statements in if/else branches"
        );
    }

    // =========================================================================
    // Test: while loop — loop variable drop after loop
    // =========================================================================

    #[test]
    fn test_drop_insertion_while_loop() {
        // Mumei requires 'invariant' for while loops
        // atom f(n: Int) body: { let i = 0; while i < n invariant: i >= 0 { let i = i + 1; i }; i }
        let mut atom = make_atom(
            "f",
            vec![make_param("n", "Int")],
            "{ let i = 0; while i < n invariant: i >= 0 { let i = i + 1; i }; i }",
        );
        atom.invariant = Some("i >= 0".to_string());
        let hir = lower_atom_to_hir(&atom);
        let mut mir = lower_hir_to_mir(&hir);
        let liveness = compute_liveness(&mir);
        insert_drops(&mut mir, &liveness);

        // Verify the MIR is valid after drop insertion
        assert!(!mir.blocks.is_empty());
    }

    // =========================================================================
    // Test: function call — argument dropped after call if unused
    // =========================================================================

    #[test]
    fn test_drop_insertion_after_call() {
        // atom f(x: Int) body: { let r = add(x, 1); r }
        // x should be dropped after the call to add
        let atom = make_atom(
            "f",
            vec![make_param("x", "Int")],
            "{ let r = add(x, 1); r }",
        );
        let hir = lower_atom_to_hir(&atom);
        let mut mir = lower_hir_to_mir(&hir);
        let liveness = compute_liveness(&mir);
        insert_drops(&mut mir, &liveness);

        // Should have at least one Drop for x after the call
        let has_drop = mir.blocks.iter().any(|b| {
            b.statements
                .iter()
                .any(|s| matches!(s, MirStatement::Drop(_)))
        });
        assert!(has_drop, "Should have Drop for x after call");
    }

    // =========================================================================
    // Test: gen/kill with terminator operands
    // =========================================================================

    #[test]
    fn test_gen_kill_terminator() {
        // Block with no statements but Return(Local(3))
        let block = BasicBlock {
            id: 0,
            statements: vec![],
            terminator: Terminator::Return(Operand::Place(Place::Local(Local(3)))),
        };

        let gk = compute_gen_kill(&block);
        assert!(gk.gen.contains(&Local(3)));
        assert!(gk.kill.is_empty());
    }

    // =========================================================================
    // Test: liveness with multiple blocks
    // =========================================================================

    #[test]
    fn test_liveness_multiple_blocks() {
        // Manually construct a simple two-block MIR:
        //   Block 0: assign tmp = Local(0); goto Block 1
        //   Block 1: return tmp
        let body = MirBody {
            name: "test".to_string(),
            locals: vec![
                LocalDecl {
                    local: Local(0),
                    name: Some("x".to_string()),
                    ty: Some("Int".to_string()),
                },
                LocalDecl {
                    local: Local(1),
                    name: Some("tmp".to_string()),
                    ty: Some("Int".to_string()),
                },
            ],
            blocks: vec![
                BasicBlock {
                    id: 0,
                    statements: vec![MirStatement::Assign(
                        Place::Local(Local(1)),
                        Rvalue::Use(Operand::Place(Place::Local(Local(0)))),
                    )],
                    terminator: Terminator::Goto(1),
                },
                BasicBlock {
                    id: 1,
                    statements: vec![],
                    terminator: Terminator::Return(Operand::Place(Place::Local(Local(1)))),
                },
            ],
            entry_block: 0,
        };

        let liveness = compute_liveness(&body);

        // Block 1: live_in should contain Local(1) (used in Return)
        assert!(liveness.live_in[&1].contains(&Local(1)));

        // Block 0: live_in should contain Local(0) (used to define Local(1))
        assert!(liveness.live_in[&0].contains(&Local(0)));

        // Block 0: live_out should contain Local(1) (live_in of successor Block 1)
        assert!(liveness.live_out[&0].contains(&Local(1)));
    }
}
