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
use crate::parser::Atom;
use crate::proof_cert;
use crate::trust_boundary::{classify_trust_boundaries, TrustBoundaryKind};
use crate::verification::ModuleEnv;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

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
    /// "No contract mismatch was detected" rather than "the pair was checked":
    /// a call `contract_consistency[]` never examined is also reported as
    /// consistent.
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
/// through is red even when its contract is also assumed somewhere. An
/// outcome that is neither a proof nor a refutation (`unknown`, an atom awaiting
/// the Lean bridge) is a proof hole, so it is yellow rather than green; only an
/// atom whose proof went through with no boundary is green.
pub fn classify_health(
    verification_status: Option<&str>,
    trust_boundaries: &[TrustBoundaryKind],
) -> &'static str {
    match verification_status {
        Some("failed") | Some("unverifiable") => HEALTH_RED,
        Some("verified") | None if trust_boundaries.is_empty() => HEALTH_GREEN,
        _ => HEALTH_YELLOW,
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

/// Certificate files that may sit next to an atom's source file, in lookup
/// order: the per-file certificate `mumei verify --proof-cert` writes, then the
/// module-directory certificates `verify_import_certificate` accepts.
fn sibling_certificate_paths(source_file: &Path) -> Vec<PathBuf> {
    let mut paths = vec![
        source_file.with_extension("proof.json"),
        source_file.with_extension("proof-cert.json"),
    ];
    if let Some(dir) = source_file.parent() {
        paths.push(dir.join(".proof-cert.json"));
        paths.push(dir.join("proof_certificate.json"));
    }
    paths
}

/// Fill in `verification_status` for atoms the current run did not verify
/// (imported / prelude atoms) from a sibling proof certificate, when one
/// exists and passes the same freshness gate imports use
/// ([`proof_cert::verify_certificate`]): the certificate's `content_hash`
/// must equal the hash of the atom as currently loaded, and a
/// `lean_verified` atom is only accepted when its `translator_version` /
/// `bridge_lemma_hash` (atom and Lean result metadata) match the current
/// bridge. Only a fresh `proven` result is copied, as `verified`; every other
/// outcome — no certificate, stale hash, stale translator, unparseable file,
/// or a certificate that never decided the atom — leaves the status `null`.
///
/// Atoms already present in `verification_status` are never overwritten.
pub fn backfill_verification_status_from_sibling_certificates(
    module_env: &ModuleEnv,
    cross_spec: &CrossSpecResult,
    verification_status: &mut BTreeMap<String, String>,
) -> usize {
    let mut certificates: BTreeMap<PathBuf, Option<proof_cert::ProofCertificate>> = BTreeMap::new();
    let mut backfilled = 0;

    for dependency_node in &cross_spec.dependency_graph {
        if verification_status.contains_key(&dependency_node.atom_name) {
            continue;
        }
        let Some(atom) = module_env.atoms.get(&dependency_node.atom_name) else {
            continue;
        };
        let source_file = atom_source_file(atom);
        if source_file == unknown_file() {
            continue;
        }
        let source_file = PathBuf::from(source_file);
        // An `alias::name` node registered by an import is the same definition
        // as `name`; certificates record the unqualified name.
        let certified_atom = match atom.name.rsplit_once("::") {
            Some((_, unqualified)) if unqualified != atom.name => Atom {
                name: unqualified.to_string(),
                ..atom.clone()
            },
            _ => atom.clone(),
        };

        for cert_path in sibling_certificate_paths(&source_file) {
            let cert = certificates
                .entry(cert_path.clone())
                .or_insert_with(|| load_fresh_sibling_certificate(&cert_path));
            let Some(cert) = cert else {
                continue;
            };
            if !cert
                .atoms
                .iter()
                .any(|entry| entry.name == certified_atom.name)
            {
                continue;
            }
            let results = proof_cert::verify_certificate(cert, &[&certified_atom], true);
            let proven = results
                .iter()
                .any(|(name, status)| name == &certified_atom.name && status == "proven");
            if proven {
                verification_status
                    .insert(dependency_node.atom_name.clone(), "verified".to_string());
                backfilled += 1;
            }
            // The first certificate that lists the atom decides; a stale entry
            // is not papered over by another file further down the list.
            break;
        }
    }
    backfilled
}

/// Load a sibling certificate only if it exists, parses, and carries current
/// Lean translator metadata; anything else is treated as "no certificate".
fn load_fresh_sibling_certificate(path: &Path) -> Option<proof_cert::ProofCertificate> {
    if !path.is_file() {
        return None;
    }
    proof_cert::load_certificate(path).ok()
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
    use crate::parser::TrustLevel;
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
    fn an_unresolved_proof_is_yellow_rather_than_green() {
        let env = module_env_with(vec![
            atom("field_mul", "true", "true", "crypto.mm"),
            atom("outside_fragment", "true", "true", "crypto.mm"),
            atom("proved", "true", "true", "crypto.mm"),
        ]);
        let graph = build_proof_graph(
            &env,
            &cross_spec_of(&env),
            &status(&[
                ("field_mul", "unknown"),
                ("outside_fragment", "escalation_candidate"),
                ("proved", "verified"),
            ]),
        );

        assert_eq!(node(&graph, "field_mul").health, HEALTH_YELLOW);
        assert_eq!(node(&graph, "outside_fragment").health, HEALTH_YELLOW);
        assert_eq!(node(&graph, "proved").health, HEALTH_GREEN);
        assert_eq!(graph.summary.yellow_count, 2);
        assert_eq!(graph.summary.green_count, 1);
        // Neither atom crosses a trust boundary; the proof state alone is why
        // they are not green.
        assert!(node(&graph, "field_mul").trust_boundaries.is_empty());
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

    // ---- sibling-certificate backfill (R-3) ----

    fn sibling_fixture(prefix: &str, body: &str) -> (PathBuf, PathBuf, ModuleEnv) {
        let dir = std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create fixture dir");
        let source_file = dir.join("lib.mm");
        std::fs::write(&source_file, "").expect("write source");
        let mut imported = atom("lib_inc", "true", "true", source_file.to_str().unwrap());
        imported.body_expr = body.to_string();
        let env = module_env_with(vec![imported]);
        (dir, source_file, env)
    }

    fn sibling_certificate(
        source_file: &Path,
        env: &ModuleEnv,
        z3_check_result: &str,
        status: &str,
    ) -> proof_cert::ProofCertificate {
        let atom_refs: Vec<&Atom> = env.atoms.values().collect();
        let mut results = HashMap::new();
        results.insert(
            "lib_inc".to_string(),
            (z3_check_result.to_string(), status.to_string()),
        );
        proof_cert::generate_certificate(
            source_file.to_str().unwrap(),
            &atom_refs,
            &results,
            env,
            None,
            None,
            None,
        )
    }

    fn backfilled(env: &ModuleEnv) -> (usize, BTreeMap<String, String>) {
        let mut statuses = BTreeMap::new();
        let count = backfill_verification_status_from_sibling_certificates(
            env,
            &cross_spec_of(env),
            &mut statuses,
        );
        (count, statuses)
    }

    #[test]
    fn a_fresh_unsat_sibling_certificate_backfills_verified() {
        let (dir, source_file, env) = sibling_fixture("mumei-pg-fresh", "x + 1");
        let cert = sibling_certificate(&source_file, &env, "unsat", "verified");
        proof_cert::save_certificate(&cert, &source_file.with_extension("proof.json"))
            .expect("write cert");

        let (count, statuses) = backfilled(&env);
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(count, 1);
        assert_eq!(
            statuses.get("lib_inc").map(String::as_str),
            Some("verified")
        );
    }

    #[test]
    fn a_changed_atom_keeps_a_null_status() {
        let (dir, source_file, mut env) = sibling_fixture("mumei-pg-changed", "x + 1");
        let cert = sibling_certificate(&source_file, &env, "unsat", "verified");
        proof_cert::save_certificate(&cert, &source_file.with_extension("proof.json"))
            .expect("write cert");
        // The source moved on after the certificate was written.
        env.atoms.get_mut("lib_inc").unwrap().body_expr = "x + 2".to_string();

        let (count, statuses) = backfilled(&env);
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(count, 0);
        assert!(statuses.is_empty(), "{statuses:?}");
    }

    #[test]
    fn an_undecided_or_refuted_sibling_entry_is_not_promoted() {
        for (z3, status) in [("unknown", "escalation_candidate"), ("sat", "failed")] {
            let (dir, source_file, env) = sibling_fixture("mumei-pg-undecided", "x + 1");
            let cert = sibling_certificate(&source_file, &env, z3, status);
            proof_cert::save_certificate(&cert, &source_file.with_extension("proof.json"))
                .expect("write cert");

            let (count, statuses) = backfilled(&env);
            let _ = std::fs::remove_dir_all(&dir);

            assert_eq!(count, 0, "{z3}/{status}");
            assert!(statuses.is_empty(), "{z3}/{status}: {statuses:?}");
        }
    }

    #[test]
    fn lean_verified_needs_current_translator_and_bridge_metadata() {
        use crate::proof_cert::LeanResultMetadata;
        use crate::verification::{LEAN_BRIDGE_LEMMA_HASH, LEAN_TRANSLATOR_VERSION};

        let current = || LeanResultMetadata {
            status: "lean_verified".to_string(),
            theorem_name: "lib_inc_correct".to_string(),
            translator_version: LEAN_TRANSLATOR_VERSION.to_string(),
            bridge_lemma_hash: LEAN_BRIDGE_LEMMA_HASH.to_string(),
            proof_path: "Generated/Lib.lean".to_string(),
            diagnostics: vec![],
            ..Default::default()
        };

        // Fresh Lean result → verified.
        let (dir, source_file, env) = sibling_fixture("mumei-pg-lean-fresh", "x + 1");
        let mut cert = sibling_certificate(&source_file, &env, "lean_verified", "verified");
        cert.atoms[0].lean_result_metadata = Some(current());
        proof_cert::save_certificate(&cert, &source_file.with_extension("proof.json"))
            .expect("write cert");
        let (count, statuses) = backfilled(&env);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(count, 1);
        assert_eq!(
            statuses.get("lib_inc").map(String::as_str),
            Some("verified")
        );

        // Stale bridge lemma hash on the atom → the whole certificate is
        // rejected by the same translator gate imports use → null.
        let (dir, source_file, env) = sibling_fixture("mumei-pg-lean-stale-atom", "x + 1");
        let mut cert = sibling_certificate(&source_file, &env, "lean_verified", "verified");
        cert.atoms[0].lean_result_metadata = Some(current());
        cert.atoms[0].bridge_lemma_hash = "old-bridge-hash".to_string();
        proof_cert::save_certificate(&cert, &source_file.with_extension("proof.json"))
            .expect("write cert");
        let (count, statuses) = backfilled(&env);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(count, 0);
        assert!(statuses.is_empty(), "{statuses:?}");

        // Stale translator in the Lean result metadata → null.
        let (dir, source_file, env) = sibling_fixture("mumei-pg-lean-stale-meta", "x + 1");
        let mut cert = sibling_certificate(&source_file, &env, "lean_verified", "verified");
        let mut stale = current();
        stale.translator_version = "mumei-lean-translator-ir-v0".to_string();
        cert.atoms[0].lean_result_metadata = Some(stale);
        proof_cert::save_certificate(&cert, &source_file.with_extension("proof.json"))
            .expect("write cert");
        let (count, statuses) = backfilled(&env);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(count, 0);
        assert!(statuses.is_empty(), "{statuses:?}");

        // No Lean result metadata at all → null.
        let (dir, source_file, env) = sibling_fixture("mumei-pg-lean-no-meta", "x + 1");
        let cert = sibling_certificate(&source_file, &env, "lean_verified", "verified");
        proof_cert::save_certificate(&cert, &source_file.with_extension("proof.json"))
            .expect("write cert");
        let (count, statuses) = backfilled(&env);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(count, 0);
        assert!(statuses.is_empty(), "{statuses:?}");
    }

    #[test]
    fn the_current_run_is_never_overwritten_and_missing_files_stay_null() {
        let (dir, source_file, env) = sibling_fixture("mumei-pg-keep", "x + 1");
        let cert = sibling_certificate(&source_file, &env, "unsat", "verified");
        proof_cert::save_certificate(&cert, &source_file.with_extension("proof.json"))
            .expect("write cert");
        let mut statuses = status(&[("lib_inc", "failed")]);
        let count = backfill_verification_status_from_sibling_certificates(
            &env,
            &cross_spec_of(&env),
            &mut statuses,
        );
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(count, 0);
        assert_eq!(statuses.get("lib_inc").map(String::as_str), Some("failed"));

        // No certificate on disk, and a corrupt one, both leave null.
        let (dir, source_file, env) = sibling_fixture("mumei-pg-none", "x + 1");
        assert_eq!(backfilled(&env).0, 0);
        std::fs::write(source_file.with_extension("proof.json"), "{ not json").expect("write");
        let (count, statuses) = backfilled(&env);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(count, 0);
        assert!(statuses.is_empty());

        // An atom with no source attribution is skipped.
        let env = module_env_with(vec![atom("orphan", "true", "true", "")]);
        assert_eq!(backfilled(&env).0, 0);
    }
}
