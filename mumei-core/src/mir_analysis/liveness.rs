use crate::mir::{BasicBlock, BasicBlockId, Local, MirBody, MirStatement, Terminator};
use std::collections::{HashMap, HashSet, VecDeque};

use super::gen_kill::{collect_operand_locals, compute_gen_kill, GenKill};

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
/// **Pass 2 (Plan 4):** Variables used only by a SwitchInt discriminant
/// (in gen via terminator but not in live_out) are excluded from dropping in
/// the current block to prevent use-after-drop. Instead, compensating Drop
/// statements are inserted at the start of each successor block, preventing
/// resource leaks.
pub fn insert_drops(body: &mut MirBody, liveness: &LivenessResult) {
    // Pre-compute gen/kill for each block
    let gen_kill: HashMap<BasicBlockId, GenKill> = body
        .blocks
        .iter()
        .map(|b| (b.id, compute_gen_kill(b)))
        .collect();

    // Pass 1: standard drop insertion (locals dying within the block).
    // Collect SwitchInt discriminant locals that need successor drops (Pass 2).
    let mut successor_drops: Vec<(Vec<BasicBlockId>, Local)> = Vec::new();

    for block in &mut body.blocks {
        let block_id = block.id;
        let live_in = liveness.live_in.get(&block_id).cloned().unwrap_or_default();
        let live_out = liveness
            .live_out
            .get(&block_id)
            .cloned()
            .unwrap_or_default();
        let gk = gen_kill.get(&block_id).unwrap();

        // Collect locals used by the terminator (SwitchInt discriminant).
        let terminator_locals = terminator_used_locals(&block.terminator);

        // Locals that are live coming in but not live going out → they die in this block
        let mut drops: Vec<Local> = Vec::new();
        for l in &live_in {
            if !live_out.contains(l) && !gk.kill.contains(l) {
                if terminator_locals.contains(l) {
                    // This local is used by the terminator — cannot drop before it.
                    // Schedule compensating drop in successor blocks (Pass 2).
                    let succs = match &block.terminator {
                        Terminator::SwitchInt {
                            targets, otherwise, ..
                        } => {
                            let mut s: Vec<BasicBlockId> =
                                targets.iter().map(|(_, id)| *id).collect();
                            s.push(*otherwise);
                            s
                        }
                        _ => Vec::new(),
                    };
                    if !succs.is_empty() {
                        successor_drops.push((succs, l.clone()));
                    }
                } else {
                    drops.push(l.clone());
                }
            }
        }

        // Sort drops by local index for deterministic output
        drops.sort_by_key(|l| l.0);

        // Insert Drop statements before the terminator
        for l in drops {
            block.statements.push(MirStatement::Drop(l));
        }
    }

    // Pass 2 (Plan 4): Insert compensating drops at the start of successor blocks
    // for SwitchInt discriminant locals.
    for (succ_ids, local) in successor_drops {
        for succ_id in succ_ids {
            if let Some(succ_block) = body.blocks.iter_mut().find(|b| b.id == succ_id) {
                // Prevent double-drop: only insert if not already present.
                if !block_already_drops(succ_block, &local) {
                    succ_block
                        .statements
                        .insert(0, MirStatement::Drop(local.clone()));
                }
            }
        }
    }
}

/// Collect all locals read by a terminator.
fn terminator_used_locals(terminator: &Terminator) -> HashSet<Local> {
    let mut locals = HashSet::new();
    match terminator {
        Terminator::Return(op) => {
            collect_operand_locals(op, &mut locals);
        }
        Terminator::SwitchInt { discr, .. } => {
            collect_operand_locals(discr, &mut locals);
        }
        Terminator::Goto(_) | Terminator::Unreachable => {}
    }
    locals
}

/// Check whether a block already contains a Drop for the given local.
fn block_already_drops(block: &BasicBlock, local: &Local) -> bool {
    block.statements.iter().any(|stmt| match stmt {
        MirStatement::Drop(l) => l == local,
        _ => false,
    })
}
