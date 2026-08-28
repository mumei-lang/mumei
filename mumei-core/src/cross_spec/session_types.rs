//! Session-type style protocol checking across specification files.
//!
//! Atoms declare `effect_pre` / `effect_post` contracts for stateful effects
//! (see `docs/ARCHITECTURE.md`, Modular Verification). When the atoms of one
//! stateful effect are spread over several `.mm` files, those contracts encode
//! a communication protocol: the atom that drives the effect into state `S`
//! is the sender, and the atom that requires `S` as its pre-state is the dual
//! receiver.
//!
//! This module checks that protocol by abstract interpretation on the Rust
//! side only — no Z3 — following the Temporal Effect Verifier's approach:
//!
//! * **duality** — every post-state that the effect declaration can still
//!   leave has a receiving atom,
//! * **reachability** — every required pre-state is actually produced,
//! * **progress** — the reachable protocol graph can quiesce; a reachable
//!   region that only cycles is a deadlock (circular wait).
//!
//! Explosion is bounded the same way `EffectStateMachine` bounds itself: the
//! protocol graph is skipped when it exceeds [`MAX_PROTOCOL_NODES`] states or
//! [`MAX_PROTOCOL_ROLES`] roles, and every graph traversal is capped by
//! [`MAX_PROTOCOL_ITERATIONS`].

use crate::parser::{Atom, EffectDef};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Maximum number of protocol states analysed per effect.
pub const MAX_PROTOCOL_NODES: usize = 32;
/// Maximum number of participating atoms analysed per effect.
pub const MAX_PROTOCOL_ROLES: usize = 64;
/// Maximum number of graph traversal steps per effect.
pub const MAX_PROTOCOL_ITERATIONS: usize = 512;

/// Classification of a session protocol violation.
pub const KIND_DUALITY_MISMATCH: &str = "duality_mismatch";
/// A required pre-state is never produced by any role.
pub const KIND_UNREACHABLE_RECEIVE: &str = "unreachable_receive";
/// The reachable protocol graph can never quiesce.
pub const KIND_DEADLOCK_NO_PROGRESS: &str = "deadlock_no_progress";

/// A protocol inconsistency between the atoms of one stateful effect.
///
/// The `caller_*` / `callee_*` granularity mirrors `ContractConsistencyResult`
/// so downstream consumers can treat both arrays the same way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionProtocolViolation {
    /// Stateful effect that carries the protocol.
    pub effect: String,
    /// One of [`KIND_DUALITY_MISMATCH`], [`KIND_UNREACHABLE_RECEIVE`],
    /// [`KIND_DEADLOCK_NO_PROGRESS`].
    pub kind: String,
    pub caller_atom: String,
    pub caller_file: String,
    /// Dual atom when one could be identified.
    pub callee_atom: Option<String>,
    pub callee_file: Option<String>,
    /// Protocol state the violation is anchored at.
    pub protocol_state: String,
    /// Protocol states involved, in traversal order.
    pub protocol_path: Vec<String>,
    pub message: String,
    pub suggested_fix: String,
}

/// One atom's role in the protocol of a single stateful effect.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProtocolRole {
    atom_name: String,
    file: String,
    pre_state: String,
    post_state: String,
}

impl ProtocolRole {
    /// A role that advances the protocol is a sender; a role that only
    /// observes its pre-state is a receiver-side guard.
    fn is_send(&self) -> bool {
        self.pre_state != self.post_state
    }
}

/// Check the protocols of every stateful effect shared by at least two files.
///
/// Single-file protocols are left to the Temporal Effect Verifier
/// (`mir_analysis::temporal_effects`), which sees the actual `perform`
/// sequence and therefore reports strictly more precise diagnostics.
pub fn detect_session_protocol_violations(
    atoms: &BTreeMap<String, &Atom>,
    effect_defs: &BTreeMap<String, &EffectDef>,
) -> Vec<SessionProtocolViolation> {
    let mut violations = Vec::new();

    for (effect_name, effect_def) in effect_defs {
        if effect_def.states.is_empty() || effect_def.states.len() > MAX_PROTOCOL_NODES {
            continue;
        }
        let roles = collect_roles(effect_name, effect_def, atoms);
        if roles.len() > MAX_PROTOCOL_ROLES {
            continue;
        }
        let files: BTreeSet<&str> = roles.iter().map(|role| role.file.as_str()).collect();
        if roles.len() < 2 || files.len() < 2 {
            continue;
        }
        violations.extend(check_duality(effect_name, effect_def, &roles));
        violations.extend(check_reachable_receives(effect_name, effect_def, &roles));
        violations.extend(check_progress(effect_name, effect_def, &roles));
    }

    violations.sort_by(|left, right| {
        (&left.effect, &left.kind, &left.message).cmp(&(&right.effect, &right.kind, &right.message))
    });
    violations
}

fn collect_roles(
    effect_name: &str,
    effect_def: &EffectDef,
    atoms: &BTreeMap<String, &Atom>,
) -> Vec<ProtocolRole> {
    let initial = initial_state(effect_def);
    let mut roles = Vec::new();
    for (atom_name, atom) in atoms {
        let declared_pre = atom.effect_pre.get(effect_name);
        let declared_post = atom.effect_post.get(effect_name);
        let (pre_state, post_state) = match (declared_pre, declared_post) {
            (Some(pre), Some(post)) => (pre.clone(), post.clone()),
            (Some(pre), None) => (pre.clone(), pre.clone()),
            (None, Some(post)) => (initial.clone(), post.clone()),
            (None, None) => continue,
        };
        if !effect_def.states.contains(&pre_state) || !effect_def.states.contains(&post_state) {
            // Undeclared states are already reported by effect declaration
            // validation; re-reporting them here would duplicate diagnostics.
            continue;
        }
        roles.push(ProtocolRole {
            atom_name: atom_name.clone(),
            file: source_file(atom),
            pre_state,
            post_state,
        });
    }
    roles.sort_by(|left, right| left.atom_name.cmp(&right.atom_name));
    roles
}

fn check_duality(
    effect_name: &str,
    effect_def: &EffectDef,
    roles: &[ProtocolRole],
) -> Vec<SessionProtocolViolation> {
    let consumed: BTreeSet<&str> = roles.iter().map(|role| role.pre_state.as_str()).collect();
    let mut violations = Vec::new();
    let mut reported: BTreeSet<(&str, &str)> = BTreeSet::new();

    for role in roles.iter().filter(|role| role.is_send()) {
        if consumed.contains(role.post_state.as_str()) {
            continue;
        }
        if !has_outgoing_transition(effect_def, &role.post_state) {
            // The declaration itself ends here, so nothing is left to receive.
            continue;
        }
        if !reported.insert((role.atom_name.as_str(), role.post_state.as_str())) {
            continue;
        }
        let expected: Vec<String> = effect_def
            .transitions
            .iter()
            .filter(|transition| transition.from_state == role.post_state)
            .map(|transition| transition.operation.clone())
            .collect();
        violations.push(SessionProtocolViolation {
            effect: effect_name.to_string(),
            kind: KIND_DUALITY_MISMATCH.to_string(),
            caller_atom: role.atom_name.clone(),
            caller_file: role.file.clone(),
            callee_atom: None,
            callee_file: None,
            protocol_state: role.post_state.clone(),
            protocol_path: vec![role.pre_state.clone(), role.post_state.clone()],
            message: format!(
                "Atom '{}' in {} leaves effect '{}' in state '{}', but no atom declares \
                 'effect_pre: {{ {}: {} }}': the protocol has no dual receiver for that message.",
                role.atom_name,
                role.file,
                effect_name,
                role.post_state,
                effect_name,
                role.post_state
            ),
            suggested_fix: format!(
                "Add a receiving atom with 'effect_pre: {{ {}: {} }}' (performing one of: {}), \
                 or make '{}' end the protocol in a state without outgoing transitions.",
                effect_name,
                role.post_state,
                expected.join(", "),
                role.atom_name
            ),
        });
    }

    violations
}

fn check_reachable_receives(
    effect_name: &str,
    effect_def: &EffectDef,
    roles: &[ProtocolRole],
) -> Vec<SessionProtocolViolation> {
    let initial = initial_state(effect_def);
    let produced: BTreeSet<&str> = roles
        .iter()
        .filter(|role| role.is_send())
        .map(|role| role.post_state.as_str())
        .collect();
    let mut violations = Vec::new();

    for role in roles {
        if role.pre_state == initial || produced.contains(role.pre_state.as_str()) {
            continue;
        }
        let producers: Vec<String> = effect_def
            .transitions
            .iter()
            .filter(|transition| transition.to_state == role.pre_state)
            .map(|transition| transition.operation.clone())
            .collect();
        violations.push(SessionProtocolViolation {
            effect: effect_name.to_string(),
            kind: KIND_UNREACHABLE_RECEIVE.to_string(),
            caller_atom: role.atom_name.clone(),
            caller_file: role.file.clone(),
            callee_atom: None,
            callee_file: None,
            protocol_state: role.pre_state.clone(),
            protocol_path: vec![initial.clone(), role.pre_state.clone()],
            message: format!(
                "Atom '{}' in {} requires effect '{}' in state '{}', but no atom drives '{}' into \
                 that state: '{}' can never run.",
                role.atom_name,
                role.file,
                effect_name,
                role.pre_state,
                effect_name,
                role.atom_name
            ),
            suggested_fix: if producers.is_empty() {
                format!(
                    "Declare a transition into '{}' for effect '{}', or relax the 'effect_pre' of '{}'.",
                    role.pre_state, effect_name, role.atom_name
                )
            } else {
                format!(
                    "Add a sending atom with 'effect_post: {{ {}: {} }}' (performing one of: {}), \
                     or relax the 'effect_pre' of '{}' to a state the protocol reaches.",
                    effect_name,
                    role.pre_state,
                    producers.join(", "),
                    role.atom_name
                )
            },
        });
    }

    violations
}

/// Progress: from the initial state the protocol must be able to reach a state
/// with no outgoing role edge. If every reachable state keeps handing control
/// to another role, the participants wait on each other forever.
fn check_progress(
    effect_name: &str,
    effect_def: &EffectDef,
    roles: &[ProtocolRole],
) -> Vec<SessionProtocolViolation> {
    let initial = initial_state(effect_def);
    let mut edges: BTreeMap<&str, Vec<&ProtocolRole>> = BTreeMap::new();
    for role in roles.iter().filter(|role| role.is_send()) {
        edges.entry(role.pre_state.as_str()).or_default().push(role);
    }

    let mut visited: BTreeSet<&str> = BTreeSet::new();
    let mut order: Vec<&str> = Vec::new();
    let mut queue: VecDeque<&str> = VecDeque::new();
    queue.push_back(initial.as_str());
    visited.insert(initial.as_str());
    let mut steps = 0usize;
    while let Some(state) = queue.pop_front() {
        steps += 1;
        if steps > MAX_PROTOCOL_ITERATIONS {
            return Vec::new();
        }
        order.push(state);
        if let Some(outgoing) = edges.get(state) {
            for role in outgoing {
                if visited.insert(role.post_state.as_str()) {
                    queue.push_back(role.post_state.as_str());
                }
            }
        }
    }

    let quiescent = order.iter().any(|state| !edges.contains_key(state));
    if quiescent || order.len() < 2 {
        return Vec::new();
    }

    // Every reachable state has an outgoing role edge: the reachable region is
    // a cycle with no exit. Report the two roles that hand control to each
    // other first, in deterministic order.
    let waiting: Vec<&ProtocolRole> = order
        .iter()
        .filter_map(|state| edges.get(state).and_then(|roles| roles.first()).copied())
        .collect();
    let Some(first) = waiting.first() else {
        return Vec::new();
    };
    let second = waiting.iter().find(|role| role.file != first.file);
    let path: Vec<String> = order.iter().map(|state| (*state).to_string()).collect();
    let atoms: Vec<String> = waiting
        .iter()
        .map(|role| format!("{} ({})", role.atom_name, role.file))
        .collect();

    vec![SessionProtocolViolation {
        effect: effect_name.to_string(),
        kind: KIND_DEADLOCK_NO_PROGRESS.to_string(),
        caller_atom: first.atom_name.clone(),
        caller_file: first.file.clone(),
        callee_atom: second.map(|role| role.atom_name.clone()),
        callee_file: second.map(|role| role.file.clone()),
        protocol_state: first.post_state.clone(),
        protocol_path: path.clone(),
        message: format!(
            "Protocol '{}' never terminates: the reachable states {} all hand control to another \
             role ({}), so the participants wait on each other in a cycle.",
            effect_name,
            path.join(" -> "),
            atoms.join(", ")
        ),
        suggested_fix: format!(
            "Give one role an 'effect_post' that reaches a state without outgoing transitions \
             (for effect '{}': {}), so the protocol can complete.",
            effect_name,
            terminal_states(effect_def).join(", ")
        ),
    }]
}

fn initial_state(effect_def: &EffectDef) -> String {
    effect_def
        .initial_state
        .clone()
        .unwrap_or_else(|| effect_def.states[0].clone())
}

fn has_outgoing_transition(effect_def: &EffectDef, state: &str) -> bool {
    effect_def
        .transitions
        .iter()
        .any(|transition| transition.from_state == state)
}

fn terminal_states(effect_def: &EffectDef) -> Vec<String> {
    let terminals: Vec<String> = effect_def
        .states
        .iter()
        .filter(|state| !has_outgoing_transition(effect_def, state))
        .cloned()
        .collect();
    if terminals.is_empty() {
        vec!["declare a terminal state".to_string()]
    } else {
        terminals
    }
}

fn source_file(atom: &Atom) -> String {
    atom.spec_metadata
        .get("source_file")
        .filter(|value| !value.is_empty())
        .cloned()
        .or_else(|| (!atom.span.file.is_empty()).then(|| atom.span.file.clone()))
        .unwrap_or_else(|| "<unknown>".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{EffectTransition, Span, TrustLevel};
    use std::collections::HashMap;

    fn effect(states: &[&str], transitions: &[(&str, &str, &str)]) -> EffectDef {
        EffectDef {
            name: "Channel".to_string(),
            params: vec![],
            constraint: None,
            includes: vec![],
            refinement: None,
            parent: vec![],
            span: Span::default(),
            states: states.iter().map(|state| state.to_string()).collect(),
            transitions: transitions
                .iter()
                .map(|(operation, from, to)| EffectTransition {
                    operation: operation.to_string(),
                    from_state: from.to_string(),
                    to_state: to.to_string(),
                })
                .collect(),
            initial_state: states.first().map(|state| state.to_string()),
        }
    }

    fn role_atom(name: &str, file: &str, pre: Option<&str>, post: Option<&str>) -> Atom {
        let mut spec_metadata = HashMap::new();
        spec_metadata.insert("source_file".to_string(), file.to_string());
        Atom {
            name: name.to_string(),
            type_params: vec![],
            where_bounds: vec![],
            params: vec![],
            trace_id: None,
            spec_metadata,
            requires: "true".to_string(),
            forall_constraints: vec![],
            ensures: "true".to_string(),
            body_expr: "0".to_string(),
            consumed_params: vec![],
            resources: vec![],
            is_async: false,
            trust_level: TrustLevel::Verified,
            max_unroll: None,
            invariant: None,
            effects: vec![],
            return_type: Some("i64".to_string()),
            span: Span::default(),
            effect_pre: pre
                .map(|state| HashMap::from([("Channel".to_string(), state.to_string())]))
                .unwrap_or_default(),
            effect_post: post
                .map(|state| HashMap::from([("Channel".to_string(), state.to_string())]))
                .unwrap_or_default(),
        }
    }

    fn run(effect_def: &EffectDef, atoms: &[Atom]) -> Vec<SessionProtocolViolation> {
        let atom_map: BTreeMap<String, &Atom> =
            atoms.iter().map(|atom| (atom.name.clone(), atom)).collect();
        let effect_map = BTreeMap::from([("Channel".to_string(), effect_def)]);
        detect_session_protocol_violations(&atom_map, &effect_map)
    }

    #[test]
    fn dual_protocol_across_files_is_accepted() {
        let effect_def = effect(
            &["Idle", "Sent", "Answered", "Done"],
            &[
                ("send", "Idle", "Sent"),
                ("answer", "Sent", "Answered"),
                ("close", "Answered", "Done"),
            ],
        );
        let atoms = vec![
            role_atom("client_send", "client.mm", Some("Idle"), Some("Sent")),
            role_atom("server_answer", "server.mm", Some("Sent"), Some("Answered")),
            role_atom("client_close", "client.mm", Some("Answered"), Some("Done")),
        ];
        assert!(run(&effect_def, &atoms).is_empty());
    }

    #[test]
    fn unmatched_send_is_a_duality_mismatch() {
        let effect_def = effect(
            &["Idle", "Sent", "Answered"],
            &[("send", "Idle", "Sent"), ("answer", "Sent", "Answered")],
        );
        let atoms = vec![
            role_atom("client_send", "client.mm", Some("Idle"), Some("Sent")),
            role_atom(
                "server_close",
                "server.mm",
                Some("Answered"),
                Some("Answered"),
            ),
        ];
        let kinds: Vec<String> = run(&effect_def, &atoms)
            .into_iter()
            .map(|violation| violation.kind)
            .collect();
        assert!(kinds.iter().any(|kind| kind == KIND_DUALITY_MISMATCH));
    }

    #[test]
    fn receive_on_a_never_produced_state_is_unreachable() {
        let effect_def = effect(
            &["Idle", "Sent", "Answered"],
            &[("send", "Idle", "Sent"), ("answer", "Sent", "Answered")],
        );
        let atoms = vec![
            role_atom("client_send", "client.mm", Some("Idle"), Some("Sent")),
            role_atom("server_read", "server.mm", Some("Answered"), None),
        ];
        let violation = run(&effect_def, &atoms)
            .into_iter()
            .find(|violation| violation.kind == KIND_UNREACHABLE_RECEIVE)
            .expect("unreachable receive reported");
        assert_eq!(violation.caller_atom, "server_read");
        assert_eq!(violation.protocol_state, "Answered");
    }

    #[test]
    fn cycle_without_exit_is_a_deadlock() {
        let effect_def = effect(
            &["Idle", "ServerWait", "ClientWait", "Done"],
            &[
                ("request", "Idle", "ServerWait"),
                ("respond", "ServerWait", "ClientWait"),
                ("retry", "ClientWait", "ServerWait"),
                ("finish", "ClientWait", "Done"),
            ],
        );
        let atoms = vec![
            role_atom(
                "client_request",
                "client.mm",
                Some("Idle"),
                Some("ServerWait"),
            ),
            role_atom(
                "server_respond",
                "server.mm",
                Some("ServerWait"),
                Some("ClientWait"),
            ),
            role_atom(
                "client_retry",
                "client.mm",
                Some("ClientWait"),
                Some("ServerWait"),
            ),
        ];
        let violations = run(&effect_def, &atoms);
        let deadlock = violations
            .iter()
            .find(|violation| violation.kind == KIND_DEADLOCK_NO_PROGRESS)
            .expect("deadlock reported");
        assert_eq!(deadlock.callee_file.as_deref(), Some("server.mm"));
        assert!(deadlock.protocol_path.contains(&"ClientWait".to_string()));
        assert_eq!(violations.len(), 1);
    }

    #[test]
    fn single_file_protocols_are_left_to_temporal_effect_verification() {
        let effect_def = effect(&["Idle", "Sent"], &[("send", "Idle", "Sent")]);
        let atoms = vec![
            role_atom("send_a", "only.mm", Some("Idle"), Some("Sent")),
            role_atom("send_b", "only.mm", Some("Idle"), Some("Sent")),
        ];
        assert!(run(&effect_def, &atoms).is_empty());
    }

    #[test]
    fn oversized_protocol_graphs_are_skipped() {
        let states: Vec<String> = (0..=MAX_PROTOCOL_NODES)
            .map(|index| format!("S{index}"))
            .collect();
        let state_refs: Vec<&str> = states.iter().map(String::as_str).collect();
        let effect_def = effect(&state_refs, &[("step", "S0", "S1")]);
        let atoms = vec![
            role_atom("client_send", "client.mm", Some("S0"), Some("S1")),
            role_atom("server_wait", "server.mm", Some("S5"), None),
        ];
        assert!(run(&effect_def, &atoms).is_empty());
    }
}
