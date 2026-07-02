use crate::mir::{BasicBlock, Local, MirStatement, Operand, Place, Rvalue, Terminator};
use std::collections::HashSet;

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
pub(crate) fn collect_operand_locals(op: &Operand, set: &mut HashSet<Local>) {
    match op {
        Operand::Place(place) => collect_place_locals(place, set),
        Operand::Constant(_) => {}
    }
}

/// Collect all Local references from a Place into the given set.
pub(crate) fn collect_place_locals(place: &Place, set: &mut HashSet<Local>) {
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
