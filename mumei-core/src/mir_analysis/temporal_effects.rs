use crate::mir::{BasicBlock, BasicBlockId, MirBody, MirStatement, Rvalue};
use std::collections::{HashMap, HashSet, VecDeque};

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
        // Validate that initial_state is in the declared states list
        if !def.states.contains(&initial) {
            eprintln!(
                "Warning: effect '{}' has initial state '{}' which is not in states {:?}, \
                 skipping temporal verification",
                def.name, initial, def.states
            );
            return None;
        }
        let mut transitions = HashMap::new();
        for t in &def.transitions {
            // Validate that from_state and to_state are in the declared states list
            if !def.states.contains(&t.from_state) || !def.states.contains(&t.to_state) {
                eprintln!(
                    "Warning: effect '{}' transition '{}': state '{}' -> '{}' references \
                     undeclared state(s) (declared: {:?}), skipping temporal verification",
                    def.name, t.operation, t.from_state, t.to_state, def.states
                );
                return None;
            }
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
    /// Used by modular verification when effect_post contracts specify expected final states.
    /// NOTE: Not constructed in MIR analysis; checked in verification.rs via exit_states comparison.
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

/// Callee effect contract for cross-atom composition.
/// Maps effect name to (pre_state, post_state).
#[derive(Debug, Clone)]
pub struct AtomEffectContract {
    pub effect_pre: HashMap<String, String>,
    pub effect_post: HashMap<String, String>,
}

/// Describes a statement-level operation relevant to temporal analysis.
/// Either a direct `perform` or a `call` to an atom with effect contracts.
enum TemporalOp {
    Perform { effect: String, operation: String },
    Call { callee: String },
}

/// Extract Perform and Call operations from a block's statements in order.
fn extract_temporal_ops(block: &BasicBlock) -> Vec<TemporalOp> {
    let mut ops = Vec::new();
    for stmt in &block.statements {
        match stmt {
            MirStatement::Assign(
                _,
                Rvalue::Perform {
                    effect, operation, ..
                },
            ) => {
                ops.push(TemporalOp::Perform {
                    effect: effect.clone(),
                    operation: operation.clone(),
                });
            }
            MirStatement::Assign(_, Rvalue::Call { func, .. }) => {
                ops.push(TemporalOp::Call {
                    callee: func.clone(),
                });
            }
            _ => {}
        }
    }
    ops
}

/// Perform forward dataflow analysis for temporal effect verification.
///
/// Tracks effect state transitions through the MIR CFG:
/// 1. Initialize entry block with all effects' initial states
/// 2. Process each block: check perform operations against current state
/// 3. At merge points: detect conflicting states from different branches
/// 4. Cross-atom composition: when a Call to an atom with effect_pre/effect_post
///    is encountered, verify pre-state and apply post-state transition.
///
/// The `callee_contracts` parameter maps atom names to their effect contracts.
/// When None, only Perform operations are tracked (backward-compatible).
///
/// Iteration bound: block_count * max(state_machines.len(), 10)
#[allow(dead_code)]
pub fn analyze_temporal_effects(
    body: &MirBody,
    state_machines: &HashMap<String, EffectStateMachine>,
) -> TemporalEffectResult {
    analyze_temporal_effects_with_contracts(body, state_machines, None)
}

/// Extended version of analyze_temporal_effects that supports cross-atom
/// contract composition via callee_contracts.
pub fn analyze_temporal_effects_with_contracts(
    body: &MirBody,
    state_machines: &HashMap<String, EffectStateMachine>,
    callee_contracts: Option<&HashMap<String, AtomEffectContract>>,
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

        // Process perform and call operations in this block
        for op in extract_temporal_ops(block) {
            match op {
                TemporalOp::Perform { effect, operation } => {
                    if let Some(sm) = state_machines.get(&effect) {
                        if let Some(cur_state) = current.get(&effect).cloned() {
                            if let Some(next) = sm.next_state(&operation, &cur_state) {
                                current.insert(effect.clone(), next.clone());
                            } else {
                                // Invalid pre-state: operation not valid in current state
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
                                // Error recovery: assume the transition succeeded
                                if let Some((_, first_valid_from)) =
                                    sm.transitions.keys().find(|(op, _)| op == &operation)
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
                TemporalOp::Call { callee } => {
                    // Cross-atom contract composition: apply callee's effect contracts
                    if let Some(contracts) = callee_contracts {
                        if let Some(contract) = contracts.get(&callee) {
                            // Verify effect_pre: current state must match callee's requirements
                            for (effect_name, required_pre) in &contract.effect_pre {
                                if let Some(cur_state) = current.get(effect_name) {
                                    if cur_state != required_pre {
                                        violations.push(TemporalEffectViolation {
                                            block_id,
                                            effect: effect_name.clone(),
                                            operation: format!("call {}", callee),
                                            expected_state: required_pre.clone(),
                                            actual_state: cur_state.clone(),
                                            kind: TemporalViolationKind::InvalidPreState,
                                        });
                                    }
                                }
                            }
                            // Apply effect_post: update current state to callee's post-state
                            for (effect_name, post_state) in &contract.effect_post {
                                current.insert(effect_name.clone(), post_state.clone());
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
