use crate::mir::{
    BasicBlockId, Local, LocalDecl, MirBody, MirStatement, Movability, Operand, Place, Rvalue,
    Terminator,
};
use std::collections::{HashMap, HashSet, VecDeque};

use super::gen_kill::collect_place_locals;

// =============================================================================
// Move Analysis: Forward Dataflow (Ownership Tracking)
// =============================================================================
/// MIR-level ownership state. Tracks whether each Local is alive or consumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirLinearityState {
    /// Local → true (alive) / false (consumed)
    pub status: HashMap<Local, bool>,
}

/// Describes a conflict when merging two states at a branch join point.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct MergeConflict {
    pub local: Local,
    pub description: String,
}

impl MirLinearityState {
    #[allow(dead_code)]
    pub fn new() -> Self {
        MirLinearityState {
            status: HashMap::new(),
        }
    }

    /// Initialize all locals as alive.
    pub fn init_all_alive(locals: &[LocalDecl]) -> Self {
        let mut status = HashMap::new();
        for decl in locals {
            status.insert(decl.local.clone(), true);
        }
        MirLinearityState { status }
    }

    /// Mark a local as consumed (moved). Returns an error if already consumed.
    pub fn consume(&mut self, local: &Local) -> Result<(), String> {
        match self.status.get(local) {
            Some(true) => {
                self.status.insert(local.clone(), false);
                Ok(())
            }
            Some(false) => Err(format!(
                "Local({}) is already consumed (double move)",
                local.0
            )),
            None => {
                // Unknown local — treat as alive then consume
                self.status.insert(local.clone(), false);
                Ok(())
            }
        }
    }

    /// Check that a local is still alive (not consumed). Returns error if consumed.
    pub fn check_alive(&self, local: &Local) -> Result<(), String> {
        match self.status.get(local) {
            Some(true) | None => Ok(()),
            Some(false) => Err(format!("Local({}) was used after being moved", local.0)),
        }
    }

    /// Merge two states at a join point. Returns the merged state and any conflicts.
    /// A conflict occurs when one path has a local alive and the other has it consumed.
    pub fn merge(&self, other: &Self) -> (Self, Vec<MergeConflict>) {
        let mut merged = HashMap::new();
        let mut conflicts = Vec::new();

        // Collect all locals from both states
        let all_locals: HashSet<Local> = self
            .status
            .keys()
            .chain(other.status.keys())
            .cloned()
            .collect();

        for local in all_locals {
            let self_alive = self.status.get(&local).copied().unwrap_or(true);
            let other_alive = other.status.get(&local).copied().unwrap_or(true);

            if self_alive != other_alive {
                conflicts.push(MergeConflict {
                    local: local.clone(),
                    description: format!(
                        "Local({}) is alive on one path but consumed on another at merge point",
                        local.0
                    ),
                });
                // Conservative: mark as consumed at merge (prevent use-after-move)
                merged.insert(local, false);
            } else {
                merged.insert(local, self_alive);
            }
        }

        (MirLinearityState { status: merged }, conflicts)
    }
}

// =============================================================================
// Move Analysis Result
// =============================================================================

/// Result of forward dataflow move analysis.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct MoveAnalysisResult {
    pub entry_states: HashMap<BasicBlockId, MirLinearityState>,
    pub exit_states: HashMap<BasicBlockId, MirLinearityState>,
    pub violations: Vec<MoveViolation>,
}

/// A detected move violation.
#[derive(Debug, Clone)]
pub struct MoveViolation {
    pub block_id: BasicBlockId,
    pub local: Local,
    pub kind: MoveViolationKind,
}

/// Classification of move violations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveViolationKind {
    UseAfterMove,
    DoubleMove,
    ConflictingMerge,
}

impl std::fmt::Display for MoveViolationKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MoveViolationKind::UseAfterMove => write!(f, "UseAfterMove"),
            MoveViolationKind::DoubleMove => write!(f, "DoubleMove"),
            MoveViolationKind::ConflictingMerge => write!(f, "ConflictingMerge"),
        }
    }
}

/// Collect locals that are read (used) by an operand.
fn collect_operand_read_locals(op: &Operand) -> Vec<Local> {
    let mut locals = Vec::new();
    match op {
        Operand::Place(place) => {
            let mut set = HashSet::new();
            collect_place_locals(place, &mut set);
            locals.extend(set);
        }
        Operand::Constant(_) => {}
    }
    locals
}

/// Look up movability for a local from the locals list.
fn lookup_movability(local: &Local, locals: &[LocalDecl]) -> Movability {
    locals
        .iter()
        .find(|d| d.local == *local)
        .map(|d| d.movability)
        .unwrap_or(Movability::Move)
}

/// Process a single statement within move analysis, updating state and recording violations.
/// `locals` is used to look up Copy/Move distinction: Copy locals are never consumed by
/// `Rvalue::Use`.
fn process_statement_for_moves(
    stmt: &MirStatement,
    state: &mut MirLinearityState,
    block_id: BasicBlockId,
    violations: &mut Vec<MoveViolation>,
    locals: &[LocalDecl],
) {
    match stmt {
        MirStatement::Assign(place, rvalue) => {
            // Check rvalue operands for use-after-move
            match rvalue {
                Rvalue::Use(op) => {
                    // Rvalue::Use(Operand::Place(...)) is a potential move
                    if let Operand::Place(Place::Local(src)) = op {
                        // Copy types are never consumed — only check alive
                        if lookup_movability(src, locals) == Movability::Copy {
                            if state.check_alive(src).is_err() {
                                violations.push(MoveViolation {
                                    block_id,
                                    local: src.clone(),
                                    kind: MoveViolationKind::UseAfterMove,
                                });
                            }
                        } else {
                            match state.consume(src) {
                                Ok(()) => {}
                                Err(_) => {
                                    // Already consumed → UseAfterMove or DoubleMove
                                    if state.status.get(src) == Some(&false) {
                                        violations.push(MoveViolation {
                                            block_id,
                                            local: src.clone(),
                                            kind: MoveViolationKind::DoubleMove,
                                        });
                                    }
                                }
                            }
                        }
                    } else {
                        // Non-place operand (constant) — no move
                        for l in collect_operand_read_locals(op) {
                            if state.check_alive(&l).is_err() {
                                violations.push(MoveViolation {
                                    block_id,
                                    local: l,
                                    kind: MoveViolationKind::UseAfterMove,
                                });
                            }
                        }
                    }
                }
                Rvalue::BinaryOp(_, lhs, rhs) => {
                    for op in [lhs, rhs] {
                        for l in collect_operand_read_locals(op) {
                            if state.check_alive(&l).is_err() {
                                violations.push(MoveViolation {
                                    block_id,
                                    local: l,
                                    kind: MoveViolationKind::UseAfterMove,
                                });
                            }
                        }
                    }
                }
                Rvalue::Call { args, .. } => {
                    for op in args {
                        for l in collect_operand_read_locals(op) {
                            if state.check_alive(&l).is_err() {
                                violations.push(MoveViolation {
                                    block_id,
                                    local: l,
                                    kind: MoveViolationKind::UseAfterMove,
                                });
                            }
                        }
                    }
                }
                Rvalue::Ref(place) | Rvalue::RefMut(place) => {
                    let mut set = HashSet::new();
                    collect_place_locals(place, &mut set);
                    for l in set {
                        if state.check_alive(&l).is_err() {
                            violations.push(MoveViolation {
                                block_id,
                                local: l,
                                kind: MoveViolationKind::UseAfterMove,
                            });
                        }
                    }
                }
                Rvalue::StructInit { fields, .. } => {
                    for (_, op) in fields {
                        for l in collect_operand_read_locals(op) {
                            if state.check_alive(&l).is_err() {
                                violations.push(MoveViolation {
                                    block_id,
                                    local: l,
                                    kind: MoveViolationKind::UseAfterMove,
                                });
                            }
                        }
                    }
                }
                Rvalue::FieldAccess(op, _) => {
                    for l in collect_operand_read_locals(op) {
                        if state.check_alive(&l).is_err() {
                            violations.push(MoveViolation {
                                block_id,
                                local: l,
                                kind: MoveViolationKind::UseAfterMove,
                            });
                        }
                    }
                }
                Rvalue::Perform { args, .. } => {
                    for op in args {
                        for l in collect_operand_read_locals(op) {
                            if state.check_alive(&l).is_err() {
                                violations.push(MoveViolation {
                                    block_id,
                                    local: l,
                                    kind: MoveViolationKind::UseAfterMove,
                                });
                            }
                        }
                    }
                }
            }

            // The destination local is defined (reborn) by assignment
            if let Place::Local(dst) = place {
                state.status.insert(dst.clone(), true);
            }
        }
        MirStatement::StorageLive(l) => {
            // New storage: local becomes alive
            state.status.insert(l.clone(), true);
        }
        MirStatement::StorageDead(l) => {
            // Storage ends: local becomes dead/consumed
            state.status.insert(l.clone(), false);
        }
        MirStatement::Drop(l) => {
            // Drop consumes the local
            state.status.insert(l.clone(), false);
        }
        MirStatement::Nop => {}
    }
}

/// Perform forward dataflow move analysis on a MIR body.
///
/// Algorithm:
/// 1. Initialize entry_block's entry_state with all locals alive
/// 2. Add entry block to worklist
/// 3. While worklist is not empty:
///    a. Pop block B
///    b. state = entry_states[B].clone()
///    c. Process each statement in B (track moves, check alive)
///    d. exit_states[B] = state
///    e. For each successor S:
///       - If entry_states[S] is unset, copy exit_states[B]
///       - If set, merge and record ConflictingMerge violations
///       - If entry_states[S] changed, add S to worklist
///
/// Iteration bound: block_count * 10
pub fn analyze_moves(body: &MirBody) -> MoveAnalysisResult {
    let successors = body.successors();

    let mut entry_states: HashMap<BasicBlockId, MirLinearityState> = HashMap::new();
    let mut exit_states: HashMap<BasicBlockId, MirLinearityState> = HashMap::new();
    let mut violations: Vec<MoveViolation> = Vec::new();

    // Initialize entry block with all locals alive
    let init_state = MirLinearityState::init_all_alive(&body.locals);
    entry_states.insert(body.entry_block, init_state);

    // Worklist
    let mut worklist: VecDeque<BasicBlockId> = VecDeque::new();
    let mut in_worklist: HashSet<BasicBlockId> = HashSet::new();
    worklist.push_back(body.entry_block);
    in_worklist.insert(body.entry_block);

    // Theoretical convergence bound is O(block_count * local_count).
    // Use max(local_count, 10) to handle bodies with many locals correctly.
    let max_iterations = body.block_count() * body.local_count().max(10);
    let mut iterations = 0;

    while let Some(block_id) = worklist.pop_front() {
        in_worklist.remove(&block_id);
        iterations += 1;
        if iterations > max_iterations {
            eprintln!(
                "Warning: move analysis for '{}' reached iteration limit ({})",
                body.name, max_iterations
            );
            break;
        }

        // Find the block
        let block = match body.blocks.iter().find(|b| b.id == block_id) {
            Some(b) => b,
            None => continue,
        };

        // Get entry state for this block
        let mut state = match entry_states.get(&block_id) {
            Some(s) => s.clone(),
            None => continue,
        };

        // Process each statement
        for stmt in &block.statements {
            process_statement_for_moves(stmt, &mut state, block_id, &mut violations, &body.locals);
        }

        // Process terminator operands (read access)
        match &block.terminator {
            Terminator::Return(op) => {
                for l in collect_operand_read_locals(op) {
                    if state.check_alive(&l).is_err() {
                        violations.push(MoveViolation {
                            block_id,
                            local: l,
                            kind: MoveViolationKind::UseAfterMove,
                        });
                    }
                }
            }
            Terminator::SwitchInt { discr, .. } => {
                for l in collect_operand_read_locals(discr) {
                    if state.check_alive(&l).is_err() {
                        violations.push(MoveViolation {
                            block_id,
                            local: l,
                            kind: MoveViolationKind::UseAfterMove,
                        });
                    }
                }
            }
            Terminator::Goto(_) | Terminator::Unreachable => {}
        }

        // Save exit state
        exit_states.insert(block_id, state.clone());

        // Propagate to successors
        if let Some(succs) = successors.get(&block_id) {
            for &succ_id in succs {
                if let Some(existing) = entry_states.get(&succ_id) {
                    // Merge with existing entry state
                    let (merged, conflicts) = existing.merge(&state);
                    for conflict in &conflicts {
                        // Copy-type locals cannot have ownership conflicts —
                        // skip ConflictingMerge for them.
                        if lookup_movability(&conflict.local, &body.locals) == Movability::Copy {
                            continue;
                        }
                        // Compiler-generated temporaries (no user-visible name) are
                        // lowering artefacts (e.g., match result locals). They may
                        // legitimately have different alive/consumed states across
                        // control-flow paths, so skip ConflictingMerge for them.
                        let is_named = body
                            .locals
                            .iter()
                            .find(|d| d.local == conflict.local)
                            .and_then(|d| d.name.as_ref())
                            .is_some();
                        if !is_named {
                            continue;
                        }
                        violations.push(MoveViolation {
                            block_id: succ_id,
                            local: conflict.local.clone(),
                            kind: MoveViolationKind::ConflictingMerge,
                        });
                    }
                    if merged != *existing {
                        entry_states.insert(succ_id, merged);
                        if !in_worklist.contains(&succ_id) {
                            worklist.push_back(succ_id);
                            in_worklist.insert(succ_id);
                        }
                    }
                } else {
                    // First visit: copy exit state
                    entry_states.insert(succ_id, state.clone());
                    if !in_worklist.contains(&succ_id) {
                        worklist.push_back(succ_id);
                        in_worklist.insert(succ_id);
                    }
                }
            }
        }
    }

    MoveAnalysisResult {
        entry_states,
        exit_states,
        violations,
    }
}
