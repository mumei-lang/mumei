//! Interactive proof-graph export (P26).
//!
//! [`build_proof_graph`] folds the artifacts an interactive viewer needs into a
//! single document: the atom dependency graph from
//! [`cross_spec::CrossSpecVerifier::build_dependency_graph`], each atom's
//! `requires`/`ensures`, the P23 trust-boundary classification from
//! [`crate::trust_boundary`], and the session protocol violations that anchor
//! on an atom.
//!
//! No new verdict vocabulary is introduced: node colouring reuses the health
//! classes `visualize_std_graph` already paints — `green` (proven), `yellow`
//! (a trust boundary carries the contract), `red` (verification failed).

use crate::cross_spec::{atom_source_file, CrossSpecResult};
use crate::trust_boundary::{classify_trust_boundaries, TrustBoundaryKind};
use crate::verification::ModuleEnv;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Schema version of `proof_graph.json`.
pub const PROOF_GRAPH_VERSION: &str = "1.0";

/// Fully proven atom, no trust boundary.
pub const HEALTH_GREEN: &str = "green";
/// Verified, but the contract rests on a trust boundary (proof hole).
pub const HEALTH_YELLOW: &str = "yellow";
/// Verification failed or was unverifiable.
pub const HEALTH_RED: &str = "red";

/// One trust boundary an atom sits on, with the rationale carried along so a
/// viewer does not have to re-derive the wording.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustBoundaryEntry {
    pub kind: String,
    pub rationale: String,
}

impl From<TrustBoundaryKind> for TrustBoundaryEntry {
    fn from(kind: TrustBoundaryKind) -> Self {
        Self {
            kind: kind.as_str().to_string(),
            rationale: kind.rationale().to_string(),
        }
    }
}

/// An atom as an interactive graph node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProofGraphNode {
    pub atom_name: String,
    pub source_file: String,
    pub requires: String,
    pub ensures: String,
    pub effects: Vec<String>,
    pub dependencies: Vec<String>,
    pub dependents: Vec<String>,
    pub trust_boundaries: Vec<TrustBoundaryEntry>,
    /// Per-atom verification status as recorded by `mumei verify`
    /// (`verified`, `failed`, `unverifiable`, `escalation_candidate`, ...).
    pub verification_status: Option<String>,
    /// `green` / `yellow` / `red` — see [`classify_health`].
    pub health: String,
    /// Indices into [`ProofGraph::session_protocol_violations`].
    pub session_protocol_violations: Vec<usize>,
}

/// A caller → callee dependency edge, carrying the contract consistency
/// verdict `cross_spec.json` recorded for the pair.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProofGraphEdge {
    /// Caller atom (the dependent).
    pub from: String,
    /// Callee atom (the dependency).
    pub to: String,
    pub is_consistent: bool,
    pub violations: Vec<String>,
    pub warnings: Vec<String>,
}

/// Aggregate counts, so a viewer can render headline metrics without a scan.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofGraphSummary {
    pub node_count: usize,
    pub edge_count: usize,
    pub green_count: usize,
    pub yellow_count: usize,
    pub red_count: usize,
    pub trust_boundary_count: usize,
    pub session_protocol_violation_count: usize,
    pub circular_dependency_count: usize,
}

/// The document written to `proof_graph.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProofGraph {
    pub version: String,
    pub nodes: Vec<ProofGraphNode>,
    pub edges: Vec<ProofGraphEdge>,
    pub session_protocol_violations:
        Vec<crate::cross_spec::session_types::SessionProtocolViolation>,
    pub circular_dependencies: Vec<Vec<String>>,
    pub summary: ProofGraphSummary,
}

/// Classify a node the same way `visualize_std_graph` classifies a std file.
///
/// A failed proof outranks a trust boundary: an atom whose proof did not go
/// through is red even when its contract is also assumed somewhere.
pub fn classify_health(
    verification_status: Option<&str>,
    trust_boundaries: &[TrustBoundaryKind],
) -> &'static str {
    if matches!(verification_status, Some("failed") | Some("unverifiable")) {
        return HEALTH_RED;
    }
    if trust_boundaries.is_empty() {
        HEALTH_GREEN
    } else {
        HEALTH_YELLOW
    }
}

/// Build the interactive proof graph.
///
/// `verification_status` maps an atom name to the status `mumei verify`
/// recorded for it; atoms missing from the map are reported with a `null`
/// status and classified from their trust boundaries alone.
pub fn build_proof_graph(
    module_env: &ModuleEnv,
    cross_spec: &CrossSpecResult,
    verification_status: &BTreeMap<String, String>,
) -> ProofGraph {
    let violations_by_atom = index_violations_by_atom(cross_spec);

    let mut nodes = Vec::with_capacity(cross_spec.dependency_graph.len());
    let mut summary = ProofGraphSummary {
        session_protocol_violation_count: cross_spec.session_protocol_violations.len(),
        circular_dependency_count: cross_spec.circular_dependencies.len(),
        ..ProofGraphSummary::default()
    };

    for dependency_node in &cross_spec.dependency_graph {
        let atom = module_env.atoms.get(&dependency_node.atom_name);
        let trust_boundaries = atom
            .map(|atom| classify_trust_boundaries(atom, &module_env.extern_blocks))
            .unwrap_or_default();
        let status = verification_status
            .get(&dependency_node.atom_name)
            .map(String::as_str);
        let health = classify_health(status, &trust_boundaries);
        match health {
            HEALTH_RED => summary.red_count += 1,
            HEALTH_YELLOW => summary.yellow_count += 1,
            _ => summary.green_count += 1,
        }
        summary.trust_boundary_count += trust_boundaries.len();

        nodes.push(ProofGraphNode {
            atom_name: dependency_node.atom_name.clone(),
            source_file: atom.map(atom_source_file).unwrap_or_else(unknown_file),
            requires: atom
                .map(|atom| atom.requires.clone())
                .unwrap_or_else(|| "true".to_string()),
            ensures: atom
                .map(|atom| atom.ensures.clone())
                .unwrap_or_else(|| "true".to_string()),
            effects: atom
                .map(|atom| {
                    atom.effects
                        .iter()
                        .map(|effect| effect.name.clone())
                        .collect()
                })
                .unwrap_or_default(),
            dependencies: dependency_node.dependencies.clone(),
            dependents: dependency_node.dependents.clone(),
            trust_boundaries: trust_boundaries
                .iter()
                .copied()
                .map(TrustBoundaryEntry::from)
                .collect(),
            verification_status: status.map(str::to_string),
            health: health.to_string(),
            session_protocol_violations: violations_by_atom
                .get(&dependency_node.atom_name)
                .cloned()
                .unwrap_or_default(),
        });
    }

    let edges = build_edges(cross_spec);
    summary.node_count = nodes.len();
    summary.edge_count = edges.len();

    ProofGraph {
        version: PROOF_GRAPH_VERSION.to_string(),
        nodes,
        edges,
        session_protocol_violations: cross_spec.session_protocol_violations.clone(),
        circular_dependencies: cross_spec.circular_dependencies.clone(),
        summary,
    }
}

/// Edges follow `dependency_graph[]`, so the interactive graph and the static
/// Mermaid rendering agree on which pairs exist; the consistency verdict is
/// looked up from `contract_consistency[]` when the pair was checked.
fn build_edges(cross_spec: &CrossSpecResult) -> Vec<ProofGraphEdge> {
    let mut consistency = BTreeMap::new();
    for result in &cross_spec.contract_consistency {
        consistency.insert(
            (result.caller_atom.as_str(), result.callee_atom.as_str()),
            result,
        );
    }

    let mut edges = Vec::new();
    for node in &cross_spec.dependency_graph {
        for callee in &node.dependencies {
            let checked = consistency.get(&(node.atom_name.as_str(), callee.as_str()));
            edges.push(ProofGraphEdge {
                from: node.atom_name.clone(),
                to: callee.clone(),
                is_consistent: checked.map(|result| result.is_consistent).unwrap_or(true),
                violations: checked
                    .map(|result| result.violations.clone())
                    .unwrap_or_default(),
                warnings: checked
                    .map(|result| result.warnings.clone())
                    .unwrap_or_default(),
            });
        }
    }
    edges
}

/// Map each atom named by a session protocol violation to the violation
/// indices it participates in, so node selection can surface them.
fn index_violations_by_atom(cross_spec: &CrossSpecResult) -> BTreeMap<String, Vec<usize>> {
    let mut index: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (position, violation) in cross_spec.session_protocol_violations.iter().enumerate() {
        let mut atoms = BTreeSet::new();
        atoms.insert(violation.caller_atom.clone());
        if let Some(callee) = &violation.callee_atom {
            atoms.insert(callee.clone());
        }
        for atom in atoms {
            index.entry(atom).or_default().push(position);
        }
    }
    index
}

fn unknown_file() -> String {
    "<unknown>".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cross_spec::session_types::{SessionProtocolViolation, KIND_DUALITY_MISMATCH};
    use crate::cross_spec::{
        ContractConsistencyResult, CrossSpecSummary, CrossSpecVerifier, DependencyNode,
    };
    use crate::parser::ast::Span;
    use crate::parser::{Atom, TrustLevel};
    use std::collections::HashMap;

    fn atom(name: &str, requires: &str, ensures: &str, file: &str) -> Atom {
        let mut spec_metadata = HashMap::new();
        spec_metadata.insert("source_file".to_string(), file.to_string());
        Atom {
            name: name.to_string(),
            type_params: vec![],
            where_bounds: vec![],
            params: vec![],
            trace_id: None,
            spec_metadata,
            requires: requires.to_string(),
            forall_constraints: vec![],
            ensures: ensures.to_string(),
            body_expr: String::new(),
            consumed_params: vec![],
            resources: vec![],
            is_async: false,
            trust_level: TrustLevel::Verified,
            max_unroll: None,
            invariant: None,
            effects: vec![],
            return_type: None,
            span: Span::default(),
            effect_pre: HashMap::new(),
            effect_post: HashMap::new(),
        }
    }

    fn module_env_with(atoms: Vec<Atom>) -> ModuleEnv {
        let mut env = ModuleEnv::default();
        for atom in atoms {
            env.atoms.insert(atom.name.clone(), atom);
        }
        env
    }

    fn cross_spec_of(env: &ModuleEnv) -> CrossSpecResult {
        CrossSpecVerifier::new(env).verify_all()
    }

    fn status(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(atom, status)| (atom.to_string(), status.to_string()))
            .collect()
    }

    fn node<'graph>(graph: &'graph ProofGraph, name: &str) -> &'graph ProofGraphNode {
        graph
            .nodes
            .iter()
            .find(|node| node.atom_name == name)
            .unwrap_or_else(|| panic!("node {name} missing from proof graph"))
    }

    #[test]
    fn nodes_carry_contracts_and_source_files() {
        let env = module_env_with(vec![
            atom("charge", "amount > 0", "result >= 0", "payment_app.mm"),
            atom("validate", "amount > 0", "result >= 0", "payment_client.mm"),
        ]);
        let graph = build_proof_graph(&env, &cross_spec_of(&env), &BTreeMap::new());

        assert_eq!(graph.version, PROOF_GRAPH_VERSION);
        assert_eq!(graph.summary.node_count, 2);
        let charge = node(&graph, "charge");
        assert_eq!(charge.requires, "amount > 0");
        assert_eq!(charge.ensures, "result >= 0");
        assert_eq!(charge.source_file, "payment_app.mm");
        assert_eq!(charge.verification_status, None);
    }

    #[test]
    fn dependency_edges_follow_the_cross_spec_dependency_graph() {
        let mut caller = atom("charge", "true", "true", "payment_app.mm");
        caller.body_expr = "validate(amount)".to_string();
        let env = module_env_with(vec![
            caller,
            atom("validate", "true", "true", "payment_client.mm"),
        ]);
        let graph = build_proof_graph(&env, &cross_spec_of(&env), &BTreeMap::new());

        assert_eq!(graph.summary.edge_count, 1);
        assert_eq!(graph.edges[0].from, "charge");
        assert_eq!(graph.edges[0].to, "validate");
        assert!(graph.edges[0].is_consistent);
        assert_eq!(node(&graph, "charge").dependencies, vec!["validate"]);
        assert_eq!(node(&graph, "validate").dependents, vec!["charge"]);
    }

    #[test]
    fn an_inconsistent_pair_marks_the_edge() {
        let cross_spec = CrossSpecResult {
            contract_consistency: vec![ContractConsistencyResult {
                caller_atom: "charge".to_string(),
                caller_file: "payment_app.mm".to_string(),
                callee_atom: "validate".to_string(),
                callee_file: "payment_client.mm".to_string(),
                is_consistent: false,
                violations: vec!["amount bound mismatch".to_string()],
                warnings: vec![],
            }],
            global_invariants: vec![],
            global_invariant_conflicts: vec![],
            circular_dependencies: vec![],
            session_protocol_violations: vec![],
            session_analysis_skips: vec![],
            dependency_graph: vec![
                DependencyNode {
                    atom_name: "charge".to_string(),
                    dependencies: vec!["validate".to_string()],
                    dependents: vec![],
                },
                DependencyNode {
                    atom_name: "validate".to_string(),
                    dependencies: vec![],
                    dependents: vec!["charge".to_string()],
                },
            ],
            agent_artifact_mapping: vec![],
            summary: CrossSpecSummary {
                total_atoms: 2,
                consistent_calls: 0,
                inconsistent_calls: 1,
                circular_dependency_count: 0,
                global_invariant_count: 0,
                global_invariant_conflict_count: 0,
                session_protocol_violation_count: 0,
                session_analysis_skipped_count: 0,
            },
        };
        let env = module_env_with(vec![]);
        let graph = build_proof_graph(&env, &cross_spec, &BTreeMap::new());

        assert!(!graph.edges[0].is_consistent);
        assert_eq!(graph.edges[0].violations, vec!["amount bound mismatch"]);
        // Atoms outside the module env still appear, with placeholder contracts.
        assert_eq!(node(&graph, "charge").requires, "true");
        assert_eq!(node(&graph, "charge").source_file, "<unknown>");
    }

    #[test]
    fn a_trusted_atom_is_yellow_and_carries_its_rationale() {
        let mut trusted = atom("read_clock", "true", "true", "std/time.mm");
        trusted.trust_level = TrustLevel::Trusted;
        let env = module_env_with(vec![trusted, atom("pure", "true", "true", "app.mm")]);
        let graph = build_proof_graph(
            &env,
            &cross_spec_of(&env),
            &status(&[("read_clock", "verified"), ("pure", "verified")]),
        );

        let read_clock = node(&graph, "read_clock");
        assert_eq!(read_clock.health, HEALTH_YELLOW);
        assert_eq!(read_clock.trust_boundaries.len(), 1);
        assert_eq!(read_clock.trust_boundaries[0].kind, "trusted_atom");
        assert!(!read_clock.trust_boundaries[0].rationale.is_empty());
        assert_eq!(node(&graph, "pure").health, HEALTH_GREEN);
        assert_eq!(graph.summary.yellow_count, 1);
        assert_eq!(graph.summary.green_count, 1);
        assert_eq!(graph.summary.trust_boundary_count, 1);
    }

    #[test]
    fn a_failed_proof_is_red_even_with_a_trust_boundary() {
        let mut trusted = atom("read_clock", "true", "true", "std/time.mm");
        trusted.trust_level = TrustLevel::Trusted;
        let env = module_env_with(vec![trusted]);
        let graph = build_proof_graph(
            &env,
            &cross_spec_of(&env),
            &status(&[("read_clock", "failed")]),
        );

        assert_eq!(node(&graph, "read_clock").health, HEALTH_RED);
        assert_eq!(graph.summary.red_count, 1);
    }

    #[test]
    fn an_effect_pre_override_is_a_yellow_boundary() {
        let mut send = atom("send_request", "true", "true", "order_client.mm");
        send.effect_pre
            .insert("OrderChannel".to_string(), "Idle".to_string());
        let env = module_env_with(vec![send]);
        let graph = build_proof_graph(&env, &cross_spec_of(&env), &BTreeMap::new());

        let send = node(&graph, "send_request");
        assert_eq!(send.health, HEALTH_YELLOW);
        assert_eq!(send.trust_boundaries[0].kind, "effect_pre_override");
    }

    #[test]
    fn session_violations_are_indexed_per_participating_atom() {
        let mut cross_spec = cross_spec_of(&module_env_with(vec![
            atom("send_request", "true", "true", "order_client.mm"),
            atom("recv_reply", "true", "true", "order_server.mm"),
        ]));
        cross_spec.session_protocol_violations = vec![SessionProtocolViolation {
            effect: "OrderChannel".to_string(),
            kind: KIND_DUALITY_MISMATCH.to_string(),
            caller_atom: "send_request".to_string(),
            caller_file: "order_client.mm".to_string(),
            callee_atom: Some("recv_reply".to_string()),
            callee_file: Some("order_server.mm".to_string()),
            protocol_state: "Idle".to_string(),
            protocol_path: vec!["Idle".to_string(), "Sent".to_string()],
            message: "no dual receive".to_string(),
            suggested_fix: "declare effect_post".to_string(),
        }];
        let env = module_env_with(vec![
            atom("send_request", "true", "true", "order_client.mm"),
            atom("recv_reply", "true", "true", "order_server.mm"),
        ]);

        let graph = build_proof_graph(&env, &cross_spec, &BTreeMap::new());

        assert_eq!(graph.summary.session_protocol_violation_count, 1);
        assert_eq!(
            node(&graph, "send_request").session_protocol_violations,
            [0]
        );
        assert_eq!(node(&graph, "recv_reply").session_protocol_violations, [0]);
        assert_eq!(
            graph.session_protocol_violations[0].effect,
            "OrderChannel".to_string()
        );
    }

    #[test]
    fn the_document_round_trips_through_json() {
        let env = module_env_with(vec![atom("charge", "amount > 0", "result >= 0", "app.mm")]);
        let graph = build_proof_graph(
            &env,
            &cross_spec_of(&env),
            &status(&[("charge", "verified")]),
        );
        let json = serde_json::to_string(&graph).expect("serialize proof graph");
        let restored: ProofGraph = serde_json::from_str(&json).expect("deserialize proof graph");
        assert_eq!(restored, graph);
    }
}
