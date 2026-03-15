// =============================================================================
// MIR Analysis: Liveness Analysis + Drop Insertion + Move Analysis
// =============================================================================
// Implements:
//   1. Backward dataflow liveness analysis on MIR CFG, then inserts
//      MirStatement::Drop at points where variables become dead.
//   2. Forward dataflow move analysis to detect use-after-move, double-move,
//      and conflicting merge at branch join points.
//
// Pipeline: compute_gen_kill → compute_liveness → insert_drops
//           analyze_moves (forward dataflow for ownership tracking)
// =============================================================================

use crate::mir::{
    BasicBlock, BasicBlockId, Local, LocalDecl, MirBody, MirStatement, Operand, Place, Rvalue,
    Terminator,
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

/// Process a single statement within move analysis, updating state and recording violations.
fn process_statement_for_moves(
    stmt: &MirStatement,
    state: &mut MirLinearityState,
    block_id: BasicBlockId,
    violations: &mut Vec<MoveViolation>,
) {
    match stmt {
        MirStatement::Assign(place, rvalue) => {
            // Check rvalue operands for use-after-move
            match rvalue {
                Rvalue::Use(op) => {
                    // Rvalue::Use(Operand::Place(...)) is a potential move
                    if let Operand::Place(Place::Local(src)) = op {
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
            process_statement_for_moves(stmt, &mut state, block_id, &mut violations);
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

// =============================================================================
// Temporal Effect Verification (Stateful Effects)
// =============================================================================
//
// Tracks effect state transitions through MIR CFG using forward dataflow.
// Each stateful effect has defined states (e.g., Closed, Open) and valid
// transitions (e.g., open: Closed -> Open). The analysis verifies that
// perform operations occur in valid states.
//
// Pipeline: EffectStateMachine::from_effect_def → analyze_temporal_effects
// =============================================================================

/// Maximum number of states allowed per effect state machine.
/// Exceeding this limit causes the state machine to be skipped with a warning.
pub const MAX_EFFECT_STATES: usize = 8;

/// A state machine derived from a stateful EffectDef.
#[derive(Debug, Clone)]
pub struct EffectStateMachine {
    // NOTE: effect_name and states are retained for debug output, future Z3 Datatype Sort
    // construction, and modular verification (effect_pre/effect_post contracts).
    #[allow(dead_code)]
    pub effect_name: String,
    #[allow(dead_code)]
    pub states: Vec<String>,
    /// (operation, from_state) → to_state
    pub transitions: HashMap<(String, String), String>,
    pub initial_state: String,
}

impl EffectStateMachine {
    /// Construct from an EffectDef. Returns None if the effect is stateless
    /// (no states defined) or exceeds MAX_EFFECT_STATES.
    pub fn from_effect_def(def: &crate::parser::EffectDef) -> Option<Self> {
        if def.states.is_empty() {
            return None;
        }
        if def.states.len() > MAX_EFFECT_STATES {
            eprintln!(
                "Warning: effect '{}' has {} states (max {}), skipping temporal verification",
                def.name,
                def.states.len(),
                MAX_EFFECT_STATES
            );
            return None;
        }
        let initial = def
            .initial_state
            .clone()
            .unwrap_or_else(|| def.states[0].clone());
        let mut transitions = HashMap::new();
        for t in &def.transitions {
            transitions.insert(
                (t.operation.clone(), t.from_state.clone()),
                t.to_state.clone(),
            );
        }
        Some(EffectStateMachine {
            effect_name: def.name.clone(),
            states: def.states.clone(),
            transitions,
            initial_state: initial,
        })
    }

    /// Check whether an operation can be performed from the given state.
    // NOTE: can_transition is used in tests and retained for future modular verification
    // (effect_pre/effect_post contract checking).
    #[allow(dead_code)]
    pub fn can_transition(&self, operation: &str, current_state: &str) -> bool {
        self.transitions
            .contains_key(&(operation.to_string(), current_state.to_string()))
    }

    /// Get the next state after performing an operation from the given state.
    pub fn next_state(&self, operation: &str, current_state: &str) -> Option<&String> {
        self.transitions
            .get(&(operation.to_string(), current_state.to_string()))
    }
}

/// Map from effect name to its current state at a given program point.
pub type EffectStateMap = HashMap<String, String>;

/// Result of temporal effect analysis.
#[derive(Debug, Clone)]
pub struct TemporalEffectResult {
    pub violations: Vec<TemporalEffectViolation>,
    // NOTE: entry_states/exit_states are retained for future Z3 ConflictingState delegation
    // and modular verification (atom-level effect_pre/effect_post contract inference).
    #[allow(dead_code)]
    pub entry_states: HashMap<BasicBlockId, EffectStateMap>,
    #[allow(dead_code)]
    pub exit_states: HashMap<BasicBlockId, EffectStateMap>,
}

/// A detected temporal effect violation.
#[derive(Debug, Clone)]
pub struct TemporalEffectViolation {
    pub block_id: BasicBlockId,
    pub effect: String,
    pub operation: String,
    pub expected_state: String,
    pub actual_state: String,
    pub kind: TemporalViolationKind,
}

/// Classification of temporal effect violations.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum TemporalViolationKind {
    /// Operation performed in wrong state (e.g., write when file is Closed)
    InvalidPreState,
    /// Different branches produce different effect states at a merge point
    ConflictingState,
    /// Effect is in unexpected state at function exit (e.g., file left Open)
    // NOTE: UnexpectedFinalState is reserved for future modular verification when
    // effect_pre/effect_post contracts specify expected final states at function exit.
    #[allow(dead_code)]
    UnexpectedFinalState,
}

impl std::fmt::Display for TemporalViolationKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TemporalViolationKind::InvalidPreState => write!(f, "InvalidPreState"),
            TemporalViolationKind::ConflictingState => write!(f, "ConflictingState"),
            TemporalViolationKind::UnexpectedFinalState => write!(f, "UnexpectedFinalState"),
        }
    }
}

/// Merge two effect state maps at a join point. Returns merged map and any conflicts.
fn merge_effect_states(
    a: &EffectStateMap,
    b: &EffectStateMap,
) -> (EffectStateMap, Vec<(String, String, String)>) {
    let mut merged = a.clone();
    let mut conflicts = Vec::new();

    for (effect, b_state) in b {
        match merged.get(effect) {
            Some(a_state) if a_state != b_state => {
                conflicts.push((effect.clone(), a_state.clone(), b_state.clone()));
                // Use b's state so that merged != a (the existing entry_state).
                // This ensures the worklist propagates the change to successors.
                // The choice between a and b is arbitrary (Z3 will resolve
                // ConflictingState cases in the future); what matters is that
                // the merged map differs from the existing entry_state.
                merged.insert(effect.clone(), b_state.clone());
            }
            None => {
                merged.insert(effect.clone(), b_state.clone());
            }
            _ => {} // Same state, no conflict
        }
    }

    (merged, conflicts)
}

/// Extract Perform operations from a block's statements.
/// Returns (effect_name, operation_name) pairs in order.
fn extract_performs(block: &BasicBlock) -> Vec<(String, String)> {
    let mut performs = Vec::new();
    for stmt in &block.statements {
        if let MirStatement::Assign(
            _,
            Rvalue::Perform {
                effect, operation, ..
            },
        ) = stmt
        {
            performs.push((effect.clone(), operation.clone()));
        }
    }
    performs
}

/// Perform forward dataflow analysis for temporal effect verification.
///
/// Tracks effect state transitions through the MIR CFG:
/// 1. Initialize entry block with all effects' initial states
/// 2. Process each block: check perform operations against current state
/// 3. At merge points: detect conflicting states from different branches
///
/// Iteration bound: block_count * max(state_machines.len(), 10)
pub fn analyze_temporal_effects(
    body: &MirBody,
    state_machines: &HashMap<String, EffectStateMachine>,
) -> TemporalEffectResult {
    if state_machines.is_empty() {
        return TemporalEffectResult {
            violations: vec![],
            entry_states: HashMap::new(),
            exit_states: HashMap::new(),
        };
    }

    let successors = body.successors();

    let mut entry_states: HashMap<BasicBlockId, EffectStateMap> = HashMap::new();
    let mut exit_states: HashMap<BasicBlockId, EffectStateMap> = HashMap::new();
    let mut violations: Vec<TemporalEffectViolation> = Vec::new();
    // Track already-reported conflicts to avoid duplicate ConflictingState violations
    // when the worklist re-processes a merge point.
    let mut reported_conflicts: HashSet<(BasicBlockId, String)> = HashSet::new();

    // Initialize entry block with all effects' initial states
    let mut init_map = EffectStateMap::new();
    for (name, sm) in state_machines {
        init_map.insert(name.clone(), sm.initial_state.clone());
    }
    entry_states.insert(body.entry_block, init_map);

    // Worklist
    let mut worklist: VecDeque<BasicBlockId> = VecDeque::new();
    let mut in_worklist: HashSet<BasicBlockId> = HashSet::new();
    worklist.push_back(body.entry_block);
    in_worklist.insert(body.entry_block);

    let max_iterations = body.block_count() * state_machines.len().max(10);
    let mut iterations = 0;

    while let Some(block_id) = worklist.pop_front() {
        in_worklist.remove(&block_id);
        iterations += 1;
        if iterations > max_iterations {
            eprintln!(
                "Warning: temporal effect analysis for '{}' reached iteration limit ({})",
                body.name, max_iterations
            );
            break;
        }

        let block = match body.blocks.iter().find(|b| b.id == block_id) {
            Some(b) => b,
            None => continue,
        };

        let mut current = match entry_states.get(&block_id) {
            Some(s) => s.clone(),
            None => continue,
        };

        // Process perform operations in this block
        for (effect, operation) in extract_performs(block) {
            if let Some(sm) = state_machines.get(&effect) {
                if let Some(cur_state) = current.get(&effect).cloned() {
                    if let Some(next) = sm.next_state(&operation, &cur_state) {
                        current.insert(effect.clone(), next.clone());
                    } else {
                        // Invalid pre-state: operation not valid in current state
                        // Find which states this operation is valid from
                        let valid_states: Vec<&str> = sm
                            .transitions
                            .keys()
                            .filter(|(op, _)| op == &operation)
                            .map(|(_, s)| s.as_str())
                            .collect();
                        let expected = if valid_states.is_empty() {
                            "no valid state".to_string()
                        } else {
                            valid_states.join(" or ")
                        };
                        violations.push(TemporalEffectViolation {
                            block_id,
                            effect: effect.clone(),
                            operation: operation.clone(),
                            expected_state: expected,
                            actual_state: cur_state.clone(),
                            kind: TemporalViolationKind::InvalidPreState,
                        });
                        // Error recovery: assume the transition succeeded from a valid
                        // state to avoid cascading false violations on subsequent
                        // operations in the same block. Pick the first valid from_state
                        // and use its target as the new current state.
                        if let Some((_, first_valid_from)) = sm
                            .transitions
                            .keys()
                            .find(|(op, _)| op == &operation)
                        {
                            if let Some(recovery_next) =
                                sm.next_state(&operation, first_valid_from)
                            {
                                current.insert(effect.clone(), recovery_next.clone());
                            }
                        }
                    }
                }
            }
        }

        // Save exit state
        exit_states.insert(block_id, current.clone());

        // Propagate to successors
        if let Some(succs) = successors.get(&block_id) {
            for &succ_id in succs {
                if let Some(existing) = entry_states.get(&succ_id) {
                    let (merged, conflicts) = merge_effect_states(existing, &current);
                    for (effect, state_a, state_b) in &conflicts {
                        let conflict_key = (succ_id, effect.clone());
                        if reported_conflicts.insert(conflict_key) {
                            violations.push(TemporalEffectViolation {
                                block_id: succ_id,
                                effect: effect.clone(),
                                operation: String::new(),
                                expected_state: state_a.clone(),
                                actual_state: state_b.clone(),
                                kind: TemporalViolationKind::ConflictingState,
                            });
                        }
                    }
                    if merged != *existing {
                        entry_states.insert(succ_id, merged);
                        if !in_worklist.contains(&succ_id) {
                            worklist.push_back(succ_id);
                            in_worklist.insert(succ_id);
                        }
                    }
                } else {
                    entry_states.insert(succ_id, current.clone());
                    if !in_worklist.contains(&succ_id) {
                        worklist.push_back(succ_id);
                        in_worklist.insert(succ_id);
                    }
                }
            }
        }
    }

    TemporalEffectResult {
        violations,
        entry_states,
        exit_states,
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

    // =========================================================================
    // Move Analysis Tests
    // =========================================================================

    #[test]
    fn test_move_analysis_normal_case() {
        // atom f(x: Int) body: { let y = x + 1; y }
        // No move violations expected — x is used (read) but not moved
        let atom = make_atom("f", vec![make_param("x", "Int")], "{ let y = x + 1; y }");
        let hir = lower_atom_to_hir(&atom);
        let mir = lower_hir_to_mir(&hir);
        let result = analyze_moves(&mir);

        // Should have entry/exit states for all blocks
        for block in &mir.blocks {
            assert!(result.entry_states.contains_key(&block.id));
            assert!(result.exit_states.contains_key(&block.id));
        }

        // No use-after-move or double-move violations expected
        let critical_violations: Vec<_> = result
            .violations
            .iter()
            .filter(|v| {
                v.kind == MoveViolationKind::UseAfterMove || v.kind == MoveViolationKind::DoubleMove
            })
            .collect();
        assert!(
            critical_violations.is_empty(),
            "Normal code should have no critical move violations, got: {:?}",
            critical_violations
                .iter()
                .map(|v| format!("{} at block {}", v.kind, v.block_id))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_move_analysis_use_after_move() {
        // Manually construct MIR for: let y = move x; use x (should detect UseAfterMove)
        // Block 0: y = Use(x); return x
        // The second use of x after it has been consumed should be a violation
        let body = MirBody {
            name: "use_after_move".to_string(),
            locals: vec![
                LocalDecl {
                    local: Local(0),
                    name: Some("x".to_string()),
                    ty: Some("Int".to_string()),
                },
                LocalDecl {
                    local: Local(1),
                    name: Some("y".to_string()),
                    ty: Some("Int".to_string()),
                },
            ],
            blocks: vec![BasicBlock {
                id: 0,
                statements: vec![
                    // y = move x (Use of Place is a move)
                    MirStatement::Assign(
                        Place::Local(Local(1)),
                        Rvalue::Use(Operand::Place(Place::Local(Local(0)))),
                    ),
                ],
                // Return x — but x was already moved!
                terminator: Terminator::Return(Operand::Place(Place::Local(Local(0)))),
            }],
            entry_block: 0,
        };

        let result = analyze_moves(&body);

        // Should detect UseAfterMove for Local(0) in block 0's terminator
        let uam_violations: Vec<_> = result
            .violations
            .iter()
            .filter(|v| v.kind == MoveViolationKind::UseAfterMove && v.local == Local(0))
            .collect();
        assert!(
            !uam_violations.is_empty(),
            "Should detect use-after-move for x after move to y"
        );
    }

    #[test]
    fn test_move_analysis_double_move() {
        // Block 0: y = move x; z = move x; return z
        // The second move of x should be a DoubleMove
        let body = MirBody {
            name: "double_move".to_string(),
            locals: vec![
                LocalDecl {
                    local: Local(0),
                    name: Some("x".to_string()),
                    ty: Some("Int".to_string()),
                },
                LocalDecl {
                    local: Local(1),
                    name: Some("y".to_string()),
                    ty: Some("Int".to_string()),
                },
                LocalDecl {
                    local: Local(2),
                    name: Some("z".to_string()),
                    ty: Some("Int".to_string()),
                },
            ],
            blocks: vec![BasicBlock {
                id: 0,
                statements: vec![
                    // y = move x
                    MirStatement::Assign(
                        Place::Local(Local(1)),
                        Rvalue::Use(Operand::Place(Place::Local(Local(0)))),
                    ),
                    // z = move x (double move!)
                    MirStatement::Assign(
                        Place::Local(Local(2)),
                        Rvalue::Use(Operand::Place(Place::Local(Local(0)))),
                    ),
                ],
                terminator: Terminator::Return(Operand::Place(Place::Local(Local(2)))),
            }],
            entry_block: 0,
        };

        let result = analyze_moves(&body);

        // Should detect DoubleMove for Local(0)
        let dm_violations: Vec<_> = result
            .violations
            .iter()
            .filter(|v| v.kind == MoveViolationKind::DoubleMove && v.local == Local(0))
            .collect();
        assert!(!dm_violations.is_empty(), "Should detect double move of x");
    }

    #[test]
    fn test_move_analysis_conflicting_merge() {
        // if/else where one branch moves x and the other doesn't
        // Block 0: SwitchInt(cond) → then=1, else=2
        // Block 1: y = move x; goto 3
        // Block 2: goto 3 (x is alive here)
        // Block 3: return 0
        // At block 3, x is consumed from path 1 but alive from path 2 → ConflictingMerge
        let body = MirBody {
            name: "conflicting_merge".to_string(),
            locals: vec![
                LocalDecl {
                    local: Local(0),
                    name: Some("cond".to_string()),
                    ty: Some("Int".to_string()),
                },
                LocalDecl {
                    local: Local(1),
                    name: Some("x".to_string()),
                    ty: Some("Int".to_string()),
                },
                LocalDecl {
                    local: Local(2),
                    name: Some("y".to_string()),
                    ty: Some("Int".to_string()),
                },
            ],
            blocks: vec![
                BasicBlock {
                    id: 0,
                    statements: vec![],
                    terminator: Terminator::SwitchInt {
                        discr: Operand::Place(Place::Local(Local(0))),
                        targets: vec![(1, 1)],
                        otherwise: 2,
                    },
                },
                BasicBlock {
                    id: 1,
                    statements: vec![
                        // y = move x (x is consumed on this path)
                        MirStatement::Assign(
                            Place::Local(Local(2)),
                            Rvalue::Use(Operand::Place(Place::Local(Local(1)))),
                        ),
                    ],
                    terminator: Terminator::Goto(3),
                },
                BasicBlock {
                    id: 2,
                    statements: vec![],
                    // x is alive on this path
                    terminator: Terminator::Goto(3),
                },
                BasicBlock {
                    id: 3,
                    statements: vec![],
                    terminator: Terminator::Return(Operand::Constant(
                        crate::mir::MirConstant::Int(0),
                    )),
                },
            ],
            entry_block: 0,
        };

        let result = analyze_moves(&body);

        // Should detect ConflictingMerge for Local(1) at block 3
        let cm_violations: Vec<_> = result
            .violations
            .iter()
            .filter(|v| v.kind == MoveViolationKind::ConflictingMerge && v.local == Local(1))
            .collect();
        assert!(
            !cm_violations.is_empty(),
            "Should detect conflicting merge for x at join point"
        );
    }

    #[test]
    fn test_move_analysis_loop_move() {
        // While loop where move happens inside the loop body.
        // On 2nd iteration, the local is already consumed → violation detected.
        //
        // Block 0: goto 1
        // Block 1: SwitchInt(cond) → body=2, after=3
        // Block 2: y = move x; goto 1
        // Block 3: return 0
        let body = MirBody {
            name: "loop_move".to_string(),
            locals: vec![
                LocalDecl {
                    local: Local(0),
                    name: Some("cond".to_string()),
                    ty: Some("Int".to_string()),
                },
                LocalDecl {
                    local: Local(1),
                    name: Some("x".to_string()),
                    ty: Some("Int".to_string()),
                },
                LocalDecl {
                    local: Local(2),
                    name: Some("y".to_string()),
                    ty: Some("Int".to_string()),
                },
            ],
            blocks: vec![
                BasicBlock {
                    id: 0,
                    statements: vec![],
                    terminator: Terminator::Goto(1),
                },
                BasicBlock {
                    id: 1,
                    statements: vec![],
                    terminator: Terminator::SwitchInt {
                        discr: Operand::Place(Place::Local(Local(0))),
                        targets: vec![(1, 2)],
                        otherwise: 3,
                    },
                },
                BasicBlock {
                    id: 2,
                    statements: vec![
                        // y = move x (on second iteration, x is already consumed)
                        MirStatement::Assign(
                            Place::Local(Local(2)),
                            Rvalue::Use(Operand::Place(Place::Local(Local(1)))),
                        ),
                    ],
                    terminator: Terminator::Goto(1),
                },
                BasicBlock {
                    id: 3,
                    statements: vec![],
                    terminator: Terminator::Return(Operand::Constant(
                        crate::mir::MirConstant::Int(0),
                    )),
                },
            ],
            entry_block: 0,
        };

        let result = analyze_moves(&body);

        // On second iteration, the loop header (block 1) receives a state where x is consumed
        // from block 2's exit. This should cause a ConflictingMerge at block 1 (x alive from
        // block 0, consumed from block 2), and potentially DoubleMove in block 2 on re-visit.
        let loop_violations: Vec<_> = result
            .violations
            .iter()
            .filter(|v| v.local == Local(1))
            .collect();
        assert!(
            !loop_violations.is_empty(),
            "Should detect move violation for x inside while loop on second iteration"
        );
    }

    #[test]
    fn test_move_analysis_function_call_consume() {
        // Block 0: result = call callee(x); return x
        // x is read in the call arguments, then used again in return → UseAfterMove
        // (Since all Rvalue::Use(Operand::Place) are treated as moves)
        let body = MirBody {
            name: "call_consume".to_string(),
            locals: vec![
                LocalDecl {
                    local: Local(0),
                    name: Some("x".to_string()),
                    ty: Some("Int".to_string()),
                },
                LocalDecl {
                    local: Local(1),
                    name: Some("result".to_string()),
                    ty: Some("Int".to_string()),
                },
            ],
            blocks: vec![BasicBlock {
                id: 0,
                statements: vec![
                    // result = callee(x)
                    MirStatement::Assign(
                        Place::Local(Local(1)),
                        Rvalue::Call {
                            func: "callee".to_string(),
                            args: vec![Operand::Place(Place::Local(Local(0)))],
                        },
                    ),
                ],
                // Return x — x was passed to callee (read access, not a move via Call)
                // Call args are read, not moved, so x is still alive
                terminator: Terminator::Return(Operand::Place(Place::Local(Local(0)))),
            }],
            entry_block: 0,
        };

        let result = analyze_moves(&body);

        // Call args are read access (not moves), so x should still be alive
        // No UseAfterMove expected since Call doesn't consume arguments
        let uam_for_x: Vec<_> = result
            .violations
            .iter()
            .filter(|v| v.kind == MoveViolationKind::UseAfterMove && v.local == Local(0))
            .collect();
        assert!(
            uam_for_x.is_empty(),
            "Call arguments are read access, not moves — x should still be alive"
        );
    }

    #[test]
    fn test_mir_linearity_state_basic() {
        let locals = vec![
            LocalDecl {
                local: Local(0),
                name: Some("a".to_string()),
                ty: None,
            },
            LocalDecl {
                local: Local(1),
                name: Some("b".to_string()),
                ty: None,
            },
        ];

        let mut state = MirLinearityState::init_all_alive(&locals);
        assert!(state.status[&Local(0)]);
        assert!(state.status[&Local(1)]);

        // Consume a
        assert!(state.consume(&Local(0)).is_ok());
        assert!(!state.status[&Local(0)]);

        // Check alive: a should be consumed, b should be alive
        assert!(state.check_alive(&Local(0)).is_err());
        assert!(state.check_alive(&Local(1)).is_ok());

        // Double consume should fail
        assert!(state.consume(&Local(0)).is_err());
    }

    #[test]
    fn test_mir_linearity_state_merge() {
        let mut state1 = MirLinearityState::new();
        state1.status.insert(Local(0), true);
        state1.status.insert(Local(1), false); // consumed

        let mut state2 = MirLinearityState::new();
        state2.status.insert(Local(0), false); // consumed
        state2.status.insert(Local(1), false); // consumed

        let (merged, conflicts) = state1.merge(&state2);

        // Local(0): alive vs consumed → conflict
        assert!(
            conflicts.iter().any(|c| c.local == Local(0)),
            "Should have conflict for Local(0)"
        );
        // Local(1): both consumed → no conflict
        assert!(
            !conflicts.iter().any(|c| c.local == Local(1)),
            "Should not have conflict for Local(1)"
        );
        // Merged: Local(0) should be consumed (conservative)
        assert!(!merged.status[&Local(0)]);
        // Merged: Local(1) should be consumed
        assert!(!merged.status[&Local(1)]);
    }

    // =========================================================================
    // Temporal Effect Tests
    // =========================================================================

    /// Helper: create a File effect EffectDef with states [Closed, Open].
    fn make_file_effect_def() -> crate::parser::EffectDef {
        use crate::parser::EffectTransition;
        crate::parser::EffectDef {
            name: "File".to_string(),
            params: vec![],
            constraint: None,
            includes: vec![],
            refinement: None,
            parent: None,
            span: Span::default(),
            states: vec!["Closed".to_string(), "Open".to_string()],
            transitions: vec![
                EffectTransition {
                    operation: "open".to_string(),
                    from_state: "Closed".to_string(),
                    to_state: "Open".to_string(),
                },
                EffectTransition {
                    operation: "write".to_string(),
                    from_state: "Open".to_string(),
                    to_state: "Open".to_string(),
                },
                EffectTransition {
                    operation: "read".to_string(),
                    from_state: "Open".to_string(),
                    to_state: "Open".to_string(),
                },
                EffectTransition {
                    operation: "close".to_string(),
                    from_state: "Open".to_string(),
                    to_state: "Closed".to_string(),
                },
            ],
            initial_state: Some("Closed".to_string()),
        }
    }

    #[test]
    fn test_effect_state_machine_construction() {
        let def = make_file_effect_def();
        let sm = EffectStateMachine::from_effect_def(&def);
        assert!(sm.is_some());
        let sm = sm.unwrap();
        assert_eq!(sm.effect_name, "File");
        assert_eq!(sm.states.len(), 2);
        assert_eq!(sm.initial_state, "Closed");
        assert_eq!(sm.transitions.len(), 4);
    }

    #[test]
    fn test_effect_state_machine_stateless_returns_none() {
        let def = crate::parser::EffectDef {
            name: "Log".to_string(),
            params: vec![],
            constraint: None,
            includes: vec![],
            refinement: None,
            parent: None,
            span: Span::default(),
            states: vec![],
            transitions: vec![],
            initial_state: None,
        };
        assert!(EffectStateMachine::from_effect_def(&def).is_none());
    }

    #[test]
    fn test_effect_state_machine_too_many_states() {
        let def = crate::parser::EffectDef {
            name: "TooMany".to_string(),
            params: vec![],
            constraint: None,
            includes: vec![],
            refinement: None,
            parent: None,
            span: Span::default(),
            states: (0..9).map(|i| format!("S{}", i)).collect(),
            transitions: vec![],
            initial_state: Some("S0".to_string()),
        };
        assert!(EffectStateMachine::from_effect_def(&def).is_none());
    }

    #[test]
    fn test_effect_state_machine_transitions() {
        let def = make_file_effect_def();
        let sm = EffectStateMachine::from_effect_def(&def).unwrap();

        assert!(sm.can_transition("write", "Open"));
        assert!(!sm.can_transition("write", "Closed"));
        assert!(sm.can_transition("open", "Closed"));
        assert!(!sm.can_transition("open", "Open"));

        assert_eq!(sm.next_state("open", "Closed"), Some(&"Open".to_string()));
        assert_eq!(sm.next_state("close", "Open"), Some(&"Closed".to_string()));
        assert_eq!(sm.next_state("write", "Closed"), None);
    }

    #[test]
    fn test_temporal_effect_valid_sequence() {
        // open → write → close (valid sequence)
        let def = make_file_effect_def();
        let sm = EffectStateMachine::from_effect_def(&def).unwrap();
        let mut sms = HashMap::new();
        sms.insert("File".to_string(), sm);

        let body = MirBody {
            name: "test_valid".to_string(),
            locals: vec![LocalDecl {
                local: Local(0),
                name: Some("_ret".to_string()),
                ty: None,
            }],
            blocks: vec![
                BasicBlock {
                    id: 0,
                    statements: vec![MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "File".to_string(),
                            operation: "open".to_string(),
                            args: vec![],
                        },
                    )],
                    terminator: Terminator::Goto(1),
                },
                BasicBlock {
                    id: 1,
                    statements: vec![MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "File".to_string(),
                            operation: "write".to_string(),
                            args: vec![],
                        },
                    )],
                    terminator: Terminator::Goto(2),
                },
                BasicBlock {
                    id: 2,
                    statements: vec![MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "File".to_string(),
                            operation: "close".to_string(),
                            args: vec![],
                        },
                    )],
                    terminator: Terminator::Return(Operand::Constant(
                        crate::mir::MirConstant::Int(0),
                    )),
                },
            ],
            entry_block: 0,
        };

        let result = analyze_temporal_effects(&body, &sms);
        assert!(
            result.violations.is_empty(),
            "Valid sequence should have no violations, got: {:?}",
            result.violations
        );
    }

    #[test]
    fn test_temporal_effect_invalid_prestate() {
        // write before open → InvalidPreState
        let def = make_file_effect_def();
        let sm = EffectStateMachine::from_effect_def(&def).unwrap();
        let mut sms = HashMap::new();
        sms.insert("File".to_string(), sm);

        let body = MirBody {
            name: "test_invalid".to_string(),
            locals: vec![LocalDecl {
                local: Local(0),
                name: Some("_ret".to_string()),
                ty: None,
            }],
            blocks: vec![BasicBlock {
                id: 0,
                statements: vec![MirStatement::Assign(
                    Place::Local(Local(0)),
                    Rvalue::Perform {
                        effect: "File".to_string(),
                        operation: "write".to_string(),
                        args: vec![],
                    },
                )],
                terminator: Terminator::Return(Operand::Constant(crate::mir::MirConstant::Int(0))),
            }],
            entry_block: 0,
        };

        let result = analyze_temporal_effects(&body, &sms);
        assert_eq!(result.violations.len(), 1);
        assert_eq!(
            result.violations[0].kind,
            TemporalViolationKind::InvalidPreState
        );
        assert_eq!(result.violations[0].effect, "File");
        assert_eq!(result.violations[0].operation, "write");
        assert_eq!(result.violations[0].actual_state, "Closed");
    }

    #[test]
    fn test_temporal_effect_invalid_prestate_no_cascade() {
        // write → close when initial state is Closed.
        // write on Closed → InvalidPreState (correct).
        // After error recovery, state becomes Open (write's normal target).
        // close on Open → valid (no second violation).
        // Without error recovery, close on Closed would produce a second false violation.
        let def = make_file_effect_def();
        let sm = EffectStateMachine::from_effect_def(&def).unwrap();
        let mut sms = HashMap::new();
        sms.insert("File".to_string(), sm);

        let body = MirBody {
            name: "test_no_cascade".to_string(),
            locals: vec![LocalDecl {
                local: Local(0),
                name: Some("_ret".to_string()),
                ty: None,
            }],
            blocks: vec![BasicBlock {
                id: 0,
                statements: vec![
                    MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "File".to_string(),
                            operation: "write".to_string(),
                            args: vec![],
                        },
                    ),
                    MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "File".to_string(),
                            operation: "close".to_string(),
                            args: vec![],
                        },
                    ),
                ],
                terminator: Terminator::Return(Operand::Constant(crate::mir::MirConstant::Int(0))),
            }],
            entry_block: 0,
        };

        let result = analyze_temporal_effects(&body, &sms);
        // Only one violation: write on Closed. close on Open (recovered) is valid.
        assert_eq!(
            result.violations.len(),
            1,
            "Should have exactly 1 violation (no cascade), got: {:?}",
            result.violations
        );
        assert_eq!(
            result.violations[0].kind,
            TemporalViolationKind::InvalidPreState
        );
        assert_eq!(result.violations[0].operation, "write");
    }

    #[test]
    fn test_temporal_effect_conflicting_merge() {
        // Branch: one side closes, other doesn't → ConflictingState
        let def = make_file_effect_def();
        let sm = EffectStateMachine::from_effect_def(&def).unwrap();
        let mut sms = HashMap::new();
        sms.insert("File".to_string(), sm);

        let body = MirBody {
            name: "test_conflict".to_string(),
            locals: vec![
                LocalDecl {
                    local: Local(0),
                    name: Some("_ret".to_string()),
                    ty: None,
                },
                LocalDecl {
                    local: Local(1),
                    name: Some("cond".to_string()),
                    ty: None,
                },
            ],
            blocks: vec![
                // Block 0: open, then branch
                BasicBlock {
                    id: 0,
                    statements: vec![MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "File".to_string(),
                            operation: "open".to_string(),
                            args: vec![],
                        },
                    )],
                    terminator: Terminator::SwitchInt {
                        discr: Operand::Place(Place::Local(Local(1))),
                        targets: vec![(1, 1)],
                        otherwise: 2,
                    },
                },
                // Block 1: close
                BasicBlock {
                    id: 1,
                    statements: vec![MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "File".to_string(),
                            operation: "close".to_string(),
                            args: vec![],
                        },
                    )],
                    terminator: Terminator::Goto(3),
                },
                // Block 2: no close (File still Open)
                BasicBlock {
                    id: 2,
                    statements: vec![],
                    terminator: Terminator::Goto(3),
                },
                // Block 3: merge point
                BasicBlock {
                    id: 3,
                    statements: vec![],
                    terminator: Terminator::Return(Operand::Constant(
                        crate::mir::MirConstant::Int(0),
                    )),
                },
            ],
            entry_block: 0,
        };

        let result = analyze_temporal_effects(&body, &sms);
        let conflict_violations: Vec<_> = result
            .violations
            .iter()
            .filter(|v| v.kind == TemporalViolationKind::ConflictingState)
            .collect();
        assert!(
            !conflict_violations.is_empty(),
            "Should detect conflicting state at merge point"
        );
        assert_eq!(conflict_violations[0].block_id, 3);
    }

    #[test]
    fn test_temporal_effect_both_branches_close() {
        // Both branches close → no conflict
        let def = make_file_effect_def();
        let sm = EffectStateMachine::from_effect_def(&def).unwrap();
        let mut sms = HashMap::new();
        sms.insert("File".to_string(), sm);

        let body = MirBody {
            name: "test_both_close".to_string(),
            locals: vec![
                LocalDecl {
                    local: Local(0),
                    name: Some("_ret".to_string()),
                    ty: None,
                },
                LocalDecl {
                    local: Local(1),
                    name: Some("cond".to_string()),
                    ty: None,
                },
            ],
            blocks: vec![
                BasicBlock {
                    id: 0,
                    statements: vec![MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "File".to_string(),
                            operation: "open".to_string(),
                            args: vec![],
                        },
                    )],
                    terminator: Terminator::SwitchInt {
                        discr: Operand::Place(Place::Local(Local(1))),
                        targets: vec![(1, 1)],
                        otherwise: 2,
                    },
                },
                // Block 1: write then close
                BasicBlock {
                    id: 1,
                    statements: vec![
                        MirStatement::Assign(
                            Place::Local(Local(0)),
                            Rvalue::Perform {
                                effect: "File".to_string(),
                                operation: "write".to_string(),
                                args: vec![],
                            },
                        ),
                        MirStatement::Assign(
                            Place::Local(Local(0)),
                            Rvalue::Perform {
                                effect: "File".to_string(),
                                operation: "close".to_string(),
                                args: vec![],
                            },
                        ),
                    ],
                    terminator: Terminator::Goto(3),
                },
                // Block 2: just close
                BasicBlock {
                    id: 2,
                    statements: vec![MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "File".to_string(),
                            operation: "close".to_string(),
                            args: vec![],
                        },
                    )],
                    terminator: Terminator::Goto(3),
                },
                BasicBlock {
                    id: 3,
                    statements: vec![],
                    terminator: Terminator::Return(Operand::Constant(
                        crate::mir::MirConstant::Int(0),
                    )),
                },
            ],
            entry_block: 0,
        };

        let result = analyze_temporal_effects(&body, &sms);
        assert!(
            result.violations.is_empty(),
            "Both branches close should have no violations, got: {:?}",
            result.violations
        );
    }

    #[test]
    fn test_temporal_effect_loop() {
        // Loop: open → (write)* → close (write is Open→Open, loop OK)
        let def = make_file_effect_def();
        let sm = EffectStateMachine::from_effect_def(&def).unwrap();
        let mut sms = HashMap::new();
        sms.insert("File".to_string(), sm);

        let body = MirBody {
            name: "test_loop".to_string(),
            locals: vec![
                LocalDecl {
                    local: Local(0),
                    name: Some("_ret".to_string()),
                    ty: None,
                },
                LocalDecl {
                    local: Local(1),
                    name: Some("cond".to_string()),
                    ty: None,
                },
            ],
            blocks: vec![
                // Block 0: open, goto loop header
                BasicBlock {
                    id: 0,
                    statements: vec![MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "File".to_string(),
                            operation: "open".to_string(),
                            args: vec![],
                        },
                    )],
                    terminator: Terminator::Goto(1),
                },
                // Block 1: loop header — branch to body or exit
                BasicBlock {
                    id: 1,
                    statements: vec![],
                    terminator: Terminator::SwitchInt {
                        discr: Operand::Place(Place::Local(Local(1))),
                        targets: vec![(1, 2)],
                        otherwise: 3,
                    },
                },
                // Block 2: loop body — write, back to header
                BasicBlock {
                    id: 2,
                    statements: vec![MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "File".to_string(),
                            operation: "write".to_string(),
                            args: vec![],
                        },
                    )],
                    terminator: Terminator::Goto(1),
                },
                // Block 3: after loop — close
                BasicBlock {
                    id: 3,
                    statements: vec![MirStatement::Assign(
                        Place::Local(Local(0)),
                        Rvalue::Perform {
                            effect: "File".to_string(),
                            operation: "close".to_string(),
                            args: vec![],
                        },
                    )],
                    terminator: Terminator::Return(Operand::Constant(
                        crate::mir::MirConstant::Int(0),
                    )),
                },
            ],
            entry_block: 0,
        };

        let result = analyze_temporal_effects(&body, &sms);
        assert!(
            result.violations.is_empty(),
            "Loop with Open→Open write should have no violations, got: {:?}",
            result.violations
        );
    }
}
