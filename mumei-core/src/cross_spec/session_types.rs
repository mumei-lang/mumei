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

/// The effect declares more states than [`MAX_PROTOCOL_NODES`].
pub const SKIP_STATE_LIMIT: &str = "state_limit_exceeded";
/// The effect has more participating atoms than [`MAX_PROTOCOL_ROLES`].
pub const SKIP_ROLE_LIMIT: &str = "role_limit_exceeded";

/// An effect whose protocol was left unanalysed by the bounded analysis.
///
/// Skipping is fail-open, so it is reported explicitly: silence in
/// `session_protocol_violations[]` alone would be ambiguous.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionAnalysisSkip {
    /// Stateful effect that was not analysed.
    pub effect: String,
    /// One of [`SKIP_STATE_LIMIT`], [`SKIP_ROLE_LIMIT`].
    pub reason: String,
    /// States declared by the effect.
    pub state_count: usize,
    /// Atoms declaring a contract for the effect, when they were collected.
    pub role_count: Option<usize>,
    /// Bound that was exceeded.
    pub limit: usize,
    pub message: String,
}

/// Outcome of the bounded session protocol analysis.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionAnalysis {
    pub violations: Vec<SessionProtocolViolation>,
    /// Effects that exceeded an analysis bound and were therefore not checked.
    pub skipped: Vec<SessionAnalysisSkip>,
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
    analyze_session_protocols(atoms, effect_defs).violations
}

/// Like [`detect_session_protocol_violations`], but also reports the effects
/// the bounded analysis skipped, so fail-open silence is never ambiguous.
pub fn analyze_session_protocols(
    atoms: &BTreeMap<String, &Atom>,
    effect_defs: &BTreeMap<String, &EffectDef>,
) -> SessionAnalysis {
    let mut violations = Vec::new();
    let mut skipped: Vec<SessionAnalysisSkip> = Vec::new();
    // The same effect is visible both unqualified and under its import alias,
    // so skips are reported once per declaration.
    let mut skipped_effects: BTreeSet<&str> = BTreeSet::new();

    for (effect_name, effect_def) in effect_defs {
        if effect_def.states.is_empty() {
            continue;
        }
        if effect_def.states.len() > MAX_PROTOCOL_NODES {
            if !skipped_effects.insert(unqualified(effect_name)) {
                continue;
            }
            skipped.push(SessionAnalysisSkip {
                effect: effect_name.clone(),
                reason: SKIP_STATE_LIMIT.to_string(),
                state_count: effect_def.states.len(),
                role_count: None,
                limit: MAX_PROTOCOL_NODES,
                message: format!(
                    "Effect '{}' declares {} states (limit {}), so its cross-file protocol was \
                     not checked: session protocol violations in this effect are not reported.",
                    effect_name,
                    effect_def.states.len(),
                    MAX_PROTOCOL_NODES
                ),
            });
            continue;
        }
        let roles = collect_roles(effect_name, effect_def, atoms);
        if roles.len() > MAX_PROTOCOL_ROLES {
            if !skipped_effects.insert(unqualified(effect_name)) {
                continue;
            }
            skipped.push(SessionAnalysisSkip {
                effect: effect_name.clone(),
                reason: SKIP_ROLE_LIMIT.to_string(),
                state_count: effect_def.states.len(),
                role_count: Some(roles.len()),
                limit: MAX_PROTOCOL_ROLES,
                message: format!(
                    "Effect '{}' has {} participating atoms (limit {}), so its cross-file \
                     protocol was not checked: session protocol violations in this effect are \
                     not reported.",
                    effect_name,
                    roles.len(),
                    MAX_PROTOCOL_ROLES
                ),
            });
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
    skipped
        .sort_by(|left, right| (&left.effect, &left.reason).cmp(&(&right.effect, &right.reason)));
    SessionAnalysis {
        violations,
        skipped,
    }
}

/// Strip an import alias prefix (`protocol::Channel` -> `Channel`).
fn unqualified(effect_name: &str) -> &str {
    effect_name.rsplit("::").next().unwrap_or(effect_name)
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
        // Without file attribution the role cannot be placed on either side of
        // a cross-file protocol, and duality would be judged against unknown
        // peers.
        let Some(file) = source_file(atom) else {
            continue;
        };
        roles.push(ProtocolRole {
            atom_name: atom_name.clone(),
            file,
            pre_state,
            post_state,
        });
    }
    roles.sort_by(|left, right| {
        // Import aliases register the same atom twice (`x` and `alias::x`); keep
        // the unqualified name so a role is named the way it is declared.
        let left_key = (left.atom_name.matches("::").count(), &left.atom_name);
        let right_key = (right.atom_name.matches("::").count(), &right.atom_name);
        left_key.cmp(&right_key)
    });

    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    roles.retain(|role| seen.insert((unqualified(&role.atom_name).to_string(), role.file.clone())));

    roles.sort_by(|left, right| left.atom_name.cmp(&right.atom_name));
    roles
}

fn check_duality(
    effect_name: &str,
    effect_def: &EffectDef,
    roles: &[ProtocolRole],
) -> Vec<SessionProtocolViolation> {
    let mut violations = Vec::new();
    let mut reported: BTreeSet<(&str, &str)> = BTreeSet::new();

    for role in roles.iter().filter(|role| role.is_send()) {
        // The dual receiver has to be a different atom: an atom whose own
        // 'effect_pre' happens to match its 'effect_post' would otherwise
        // receive its own message. Peers in the sender's file do count — a
        // module may host both ends of a protocol it also exposes to others.
        let has_receiver = roles.iter().any(|peer| {
            peer.pre_state == role.post_state
                && unqualified(&peer.atom_name) != unqualified(&role.atom_name)
        });
        if has_receiver {
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
                "Atom '{}' in {} leaves effect '{}' in state '{}', but no other atom declares \
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
    // Reachability is decided from the initial state, not from "any role
    // produces this state": roles that only produce each other's pre-states
    // form an island the protocol never enters.
    let reachable = reachable_states(&initial, roles);
    let mut violations = Vec::new();

    for role in roles {
        if reachable.contains(&role.pre_state) {
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

/// Role edges of one protocol, keyed by the state the role consumes.
fn role_edges(roles: &[ProtocolRole]) -> BTreeMap<&str, Vec<&ProtocolRole>> {
    let mut edges: BTreeMap<&str, Vec<&ProtocolRole>> = BTreeMap::new();
    for role in roles.iter().filter(|role| role.is_send()) {
        edges.entry(role.pre_state.as_str()).or_default().push(role);
    }
    edges
}

/// Breadth-first visit order of the role graph starting at the initial state.
/// `None` when the traversal exceeds [`MAX_PROTOCOL_ITERATIONS`].
fn reachable_order<'a>(
    initial: &'a str,
    edges: &BTreeMap<&'a str, Vec<&'a ProtocolRole>>,
) -> Option<Vec<&'a str>> {
    let mut visited: BTreeSet<&str> = BTreeSet::from([initial]);
    let mut order: Vec<&str> = Vec::new();
    let mut queue: VecDeque<&str> = VecDeque::from([initial]);
    let mut steps = 0usize;
    while let Some(state) = queue.pop_front() {
        steps += 1;
        if steps > MAX_PROTOCOL_ITERATIONS {
            return None;
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
    Some(order)
}

/// States the protocol can actually enter by following the declared roles from
/// the initial state.
fn reachable_states(initial: &str, roles: &[ProtocolRole]) -> BTreeSet<String> {
    let edges = role_edges(roles);
    reachable_order(initial, &edges)
        .unwrap_or_else(|| {
            // Budget exhausted: fall back to every declared state so the
            // bounded analysis stays conservative and reports nothing.
            roles
                .iter()
                .flat_map(|role| [role.pre_state.as_str(), role.post_state.as_str()])
                .collect()
        })
        .into_iter()
        .map(str::to_string)
        .collect()
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
    let edges = role_edges(roles);
    let Some(order) = reachable_order(initial.as_str(), &edges) else {
        return Vec::new();
    };

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

fn source_file(atom: &Atom) -> Option<String> {
    atom.spec_metadata
        .get("source_file")
        .filter(|value| !value.is_empty())
        .cloned()
        .or_else(|| (!atom.span.file.is_empty()).then(|| atom.span.file.clone()))
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
        analyze(effect_def, atoms).violations
    }

    fn analyze(effect_def: &EffectDef, atoms: &[Atom]) -> SessionAnalysis {
        let atom_map: BTreeMap<String, &Atom> =
            atoms.iter().map(|atom| (atom.name.clone(), atom)).collect();
        let effect_map = BTreeMap::from([("Channel".to_string(), effect_def)]);
        analyze_session_protocols(&atom_map, &effect_map)
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
    fn roles_without_file_attribution_are_ignored() {
        let effect_def = effect(
            &["Idle", "Sent", "Answered"],
            &[("send", "Idle", "Sent"), ("answer", "Sent", "Answered")],
        );
        let mut unattributed = role_atom("client_send", "client.mm", Some("Idle"), Some("Sent"));
        unattributed.spec_metadata.clear();
        let atoms = vec![
            unattributed,
            role_atom("server_answer", "server.mm", Some("Sent"), Some("Answered")),
        ];
        assert!(run(&effect_def, &atoms).is_empty());
    }

    #[test]
    fn a_peer_in_the_senders_own_file_satisfies_duality() {
        let effect_def = effect(
            &["Idle", "Sent", "Answered", "Done"],
            &[
                ("send", "Idle", "Sent"),
                ("answer", "Sent", "Answered"),
                ("close", "Answered", "Done"),
            ],
        );
        // 'client_answer' consumes 'Sent' from the sender's own file, which is
        // how a verified library hosting both ends of a protocol looks.
        let atoms = vec![
            role_atom("client_send", "client.mm", Some("Idle"), Some("Sent")),
            role_atom("client_answer", "client.mm", Some("Sent"), Some("Answered")),
            role_atom("server_close", "server.mm", Some("Answered"), Some("Done")),
        ];
        assert!(run(&effect_def, &atoms).is_empty());
    }

    #[test]
    fn import_aliases_of_the_same_atom_collapse_into_one_role() {
        let effect_def = effect(
            &["Idle", "Sent", "Answered"],
            &[("send", "Idle", "Sent"), ("answer", "Sent", "Answered")],
        );
        // 'dep::client_send' is the same atom registered under an import alias,
        // so it must neither double-count nor stand in for the missing receiver.
        let atoms = vec![
            role_atom("client_send", "client.mm", Some("Idle"), Some("Sent")),
            role_atom("dep::client_send", "client.mm", Some("Idle"), Some("Sent")),
            role_atom("server_idle", "server.mm", Some("Idle"), None),
        ];
        let violations: Vec<SessionProtocolViolation> = run(&effect_def, &atoms)
            .into_iter()
            .filter(|violation| violation.kind == KIND_DUALITY_MISMATCH)
            .collect();
        assert_eq!(
            violations.len(),
            1,
            "expected one mismatch in {violations:#?}"
        );
        assert_eq!(violations[0].caller_atom, "client_send");
        assert_eq!(violations[0].protocol_state, "Sent");
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
    fn roles_that_only_produce_each_other_are_unreachable() {
        let effect_def = effect(
            &["Idle", "Sent", "Island", "Peer"],
            &[
                ("send", "Idle", "Sent"),
                ("hop", "Island", "Peer"),
                ("back", "Peer", "Island"),
            ],
        );
        let atoms = vec![
            role_atom("client_hop", "client.mm", Some("Island"), Some("Peer")),
            role_atom("server_back", "server.mm", Some("Peer"), Some("Island")),
        ];
        let states: Vec<String> = run(&effect_def, &atoms)
            .into_iter()
            .filter(|violation| violation.kind == KIND_UNREACHABLE_RECEIVE)
            .map(|violation| violation.protocol_state)
            .collect();
        assert_eq!(states, vec!["Island".to_string(), "Peer".to_string()]);
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

    #[test]
    fn skipped_oversized_protocols_are_reported() {
        let states: Vec<String> = (0..=MAX_PROTOCOL_NODES)
            .map(|index| format!("S{index}"))
            .collect();
        let state_refs: Vec<&str> = states.iter().map(String::as_str).collect();
        let effect_def = effect(&state_refs, &[("step", "S0", "S1")]);
        let atoms = vec![
            role_atom("client_send", "client.mm", Some("S0"), Some("S1")),
            role_atom("server_wait", "server.mm", Some("S5"), None),
        ];

        let analysis = analyze(&effect_def, &atoms);
        assert!(analysis.violations.is_empty());
        assert_eq!(analysis.skipped.len(), 1);
        let skip = &analysis.skipped[0];
        assert_eq!(skip.effect, "Channel");
        assert_eq!(skip.reason, SKIP_STATE_LIMIT);
        assert_eq!(skip.state_count, MAX_PROTOCOL_NODES + 1);
        assert_eq!(skip.role_count, None);
        assert_eq!(skip.limit, MAX_PROTOCOL_NODES);
        assert!(skip.message.contains("Channel"));
    }

    #[test]
    fn skipped_role_heavy_protocols_are_reported() {
        let effect_def = effect(&["Idle", "Sent"], &[("send", "Idle", "Sent")]);
        let mut atoms = Vec::new();
        for index in 0..=MAX_PROTOCOL_ROLES {
            atoms.push(role_atom(
                &format!("sender_{index}"),
                "client.mm",
                Some("Idle"),
                Some("Sent"),
            ));
        }

        let analysis = analyze(&effect_def, &atoms);
        assert!(analysis.violations.is_empty());
        assert_eq!(analysis.skipped.len(), 1);
        let skip = &analysis.skipped[0];
        assert_eq!(skip.reason, SKIP_ROLE_LIMIT);
        assert_eq!(skip.role_count, Some(MAX_PROTOCOL_ROLES + 1));
        assert_eq!(skip.limit, MAX_PROTOCOL_ROLES);
    }

    #[test]
    fn aliased_effect_declarations_are_reported_once() {
        let states: Vec<String> = (0..=MAX_PROTOCOL_NODES)
            .map(|index| format!("S{index}"))
            .collect();
        let state_refs: Vec<&str> = states.iter().map(String::as_str).collect();
        let effect_def = effect(&state_refs, &[("step", "S0", "S1")]);
        let effect_map = BTreeMap::from([
            ("Channel".to_string(), &effect_def),
            ("protocol::Channel".to_string(), &effect_def),
        ]);

        let analysis = analyze_session_protocols(&BTreeMap::new(), &effect_map);
        assert_eq!(analysis.skipped.len(), 1);
        assert_eq!(analysis.skipped[0].effect, "Channel");
    }

    #[test]
    fn analysed_protocols_report_no_skips() {
        let effect_def = effect(&["Idle", "Sent"], &[("send", "Idle", "Sent")]);
        let atoms = vec![
            role_atom("client_send", "client.mm", Some("Idle"), Some("Sent")),
            role_atom("server_recv", "server.mm", Some("Sent"), None),
        ];
        assert!(analyze(&effect_def, &atoms).skipped.is_empty());
    }
}
