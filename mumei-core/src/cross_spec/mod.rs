//! Cross-specification consistency verification.
//!
//! This module verifies consistency between multiple atoms, infers global
//! invariants, and detects dependency cycles.

pub mod drift_detector;
use crate::parser::Atom;
use crate::verification::ModuleEnv;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Dependency graph node representing an atom and its dependencies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DependencyNode {
    pub atom_name: String,
    pub dependencies: Vec<String>,
    pub dependents: Vec<String>,
}

/// Contract consistency check result between two atoms.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContractConsistencyResult {
    pub caller_atom: String,
    pub caller_file: String,
    pub callee_atom: String,
    pub callee_file: String,
    pub is_consistent: bool,
    pub violations: Vec<String>,
    pub warnings: Vec<String>,
}

/// Global invariant inferred from multiple atoms.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GlobalInvariant {
    pub invariant: String,
    pub source_atoms: Vec<String>,
    pub source_files: Vec<String>,
    pub confidence: f64,
}

/// Contradictory invariants found across atoms or specification files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GlobalInvariantConflict {
    pub left_invariant: String,
    pub right_invariant: String,
    pub left_source_atoms: Vec<String>,
    pub right_source_atoms: Vec<String>,
    pub left_source_files: Vec<String>,
    pub right_source_files: Vec<String>,
    pub message: String,
    pub suggested_fix: String,
}

/// Mapping from mumei cross-spec artifacts to mumei-agent alignment artifacts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentArtifactMapping {
    pub cross_spec_field: String,
    pub agent_field: String,
    pub contradiction_type: String,
    pub description: String,
}

/// Cross-specification verification result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrossSpecResult {
    pub contract_consistency: Vec<ContractConsistencyResult>,
    pub global_invariants: Vec<GlobalInvariant>,
    pub global_invariant_conflicts: Vec<GlobalInvariantConflict>,
    pub circular_dependencies: Vec<Vec<String>>,
    pub dependency_graph: Vec<DependencyNode>,
    pub agent_artifact_mapping: Vec<AgentArtifactMapping>,
    pub summary: CrossSpecSummary,
}

/// Summary of cross-specification verification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrossSpecSummary {
    pub total_atoms: usize,
    pub consistent_calls: usize,
    pub inconsistent_calls: usize,
    pub circular_dependency_count: usize,
    pub global_invariant_count: usize,
    pub global_invariant_conflict_count: usize,
}

/// Verifier for cross-specification consistency.
pub struct CrossSpecVerifier<'env> {
    module_env: &'env ModuleEnv,
}

impl<'env> CrossSpecVerifier<'env> {
    pub fn new(module_env: &'env ModuleEnv) -> Self {
        Self { module_env }
    }

    /// Verify contract consistency between all caller-callee pairs.
    pub fn verify_contract_consistency(&self) -> Vec<ContractConsistencyResult> {
        let mut results = Vec::new();
        let atoms = &self.module_env.atoms;

        let mut caller_names: Vec<&String> = atoms.keys().collect();
        caller_names.sort();

        for caller_name in caller_names {
            let Some(caller_atom) = atoms.get(caller_name) else {
                continue;
            };
            let callees = self.dependencies_for(caller_name, caller_atom);
            for callee_name in callees {
                if caller_name == &callee_name {
                    continue;
                }
                if let Some(callee_atom) = atoms.get(&callee_name) {
                    results.push(self.verify_pair_consistency(caller_atom, callee_atom));
                }
            }
        }

        results
    }

    /// Verify consistency between a specific caller-callee pair.
    fn verify_pair_consistency(&self, caller: &Atom, callee: &Atom) -> ContractConsistencyResult {
        let mut violations = Vec::new();
        let mut warnings = Vec::new();

        let caller_bounds = self.extract_numeric_bounds_from_contract(caller);
        if !caller_bounds.is_empty() {
            if let Some(callee_requires) = self.extract_numeric_bounds(&callee.requires) {
                for (param, caller_bound) in &caller_bounds {
                    if let Some(callee_bound) = callee_requires.get(param) {
                        if caller_bound < callee_bound {
                            violations.push(format!(
                                "Caller contract provides {param} >= {caller_bound} but callee requires {param} >= {callee_bound}"
                            ));
                        }
                    }
                }
            }
        }

        if !self.checks_effect_consistency(caller, callee) {
            warnings.push(format!(
                "Effect consistency warning between {} and {}",
                caller.name, callee.name
            ));
        }

        ContractConsistencyResult {
            caller_atom: caller.name.clone(),
            caller_file: atom_source_file(caller),
            callee_atom: callee.name.clone(),
            callee_file: atom_source_file(callee),
            is_consistent: violations.is_empty(),
            violations,
            warnings,
        }
    }

    /// Extract numeric lower bounds from a constraint string.
    fn extract_numeric_bounds(&self, constraint: &str) -> Option<HashMap<String, i64>> {
        let mut bounds: HashMap<String, i64> = HashMap::new();

        for part in split_conjuncts(constraint) {
            if let Some((var, bound)) = parse_ge_constraint(part) {
                bounds
                    .entry(var)
                    .and_modify(|existing| *existing = (*existing).max(bound))
                    .or_insert(bound);
            }
        }

        if bounds.is_empty() {
            None
        } else {
            Some(bounds)
        }
    }

    fn extract_numeric_bounds_from_contract(&self, atom: &Atom) -> HashMap<String, i64> {
        let mut bounds = self
            .extract_numeric_bounds(&atom.requires)
            .unwrap_or_default();
        if let Some(ensures_bounds) = self.extract_numeric_bounds(&atom.ensures) {
            merge_bounds(&mut bounds, ensures_bounds);
        }
        bounds
    }

    /// Check if caller's effects cover callee's effects.
    fn checks_effect_consistency(&self, caller: &Atom, callee: &Atom) -> bool {
        let caller_effects = self
            .module_env
            .resolve_leaf_effects_from_effects(&caller.effects);
        let callee_effects = self
            .module_env
            .resolve_leaf_effects_from_effects(&callee.effects);

        callee_effects.iter().all(|callee_effect| {
            caller_effects.contains(callee_effect)
                || caller_effects
                    .iter()
                    .any(|caller_effect| self.module_env.is_subeffect(callee_effect, caller_effect))
        })
    }

    /// Infer global invariants from multiple atoms.
    pub fn infer_global_invariants(&self) -> Vec<GlobalInvariant> {
        let atoms = &self.module_env.atoms;
        if atoms.is_empty() {
            return Vec::new();
        }

        let mut ensures_counts: HashMap<String, usize> = HashMap::new();
        let mut source_atoms: HashMap<String, Vec<String>> = HashMap::new();
        let mut source_files: HashMap<String, Vec<String>> = HashMap::new();

        let mut atom_names: Vec<&String> = atoms.keys().collect();
        atom_names.sort();
        for atom_name in atom_names {
            let Some(atom) = atoms.get(atom_name) else {
                continue;
            };
            let mut seen_for_atom = HashSet::new();
            for ensures_part in split_conjuncts(&atom.ensures) {
                let normalized = ensures_part.trim();
                if normalized.is_empty() || normalized == "true" {
                    continue;
                }
                let invariant = normalized.to_string();
                if seen_for_atom.insert(invariant.clone()) {
                    *ensures_counts.entry(invariant.clone()).or_insert(0) += 1;
                    source_atoms
                        .entry(invariant)
                        .or_default()
                        .push(atom_name.clone());
                    source_files
                        .entry(normalized.to_string())
                        .or_default()
                        .push(atom_source_file(atom));
                }
            }
        }

        let mut invariants: Vec<GlobalInvariant> = ensures_counts
            .into_iter()
            .filter_map(|(invariant, count)| {
                if count >= 2 {
                    Some(GlobalInvariant {
                        invariant: invariant.clone(),
                        source_atoms: source_atoms.remove(&invariant).unwrap_or_default(),
                        source_files: unique_sorted(
                            source_files.remove(&invariant).unwrap_or_default(),
                        ),
                        confidence: count as f64 / atoms.len() as f64,
                    })
                } else {
                    None
                }
            })
            .collect();
        invariants.sort_by(|left, right| left.invariant.cmp(&right.invariant));
        invariants
    }

    /// Build dependency graph from atoms.
    pub fn build_dependency_graph(&self) -> Vec<DependencyNode> {
        let atoms = &self.module_env.atoms;
        let mut graph = HashMap::new();

        let mut atom_names: Vec<&String> = atoms.keys().collect();
        atom_names.sort();
        for atom_name in &atom_names {
            graph.insert(
                (*atom_name).clone(),
                DependencyNode {
                    atom_name: (*atom_name).clone(),
                    dependencies: Vec::new(),
                    dependents: Vec::new(),
                },
            );
        }

        for caller_name in atom_names {
            let Some(caller_atom) = atoms.get(caller_name) else {
                continue;
            };
            for callee_name in self.dependencies_for(caller_name, caller_atom) {
                if !atoms.contains_key(&callee_name) || caller_name == &callee_name {
                    continue;
                }
                if let Some(node) = graph.get_mut(caller_name) {
                    node.dependencies.push(callee_name.clone());
                }
                if let Some(node) = graph.get_mut(&callee_name) {
                    node.dependents.push(caller_name.clone());
                }
            }
        }

        let mut nodes: Vec<DependencyNode> = graph.into_values().collect();
        for node in &mut nodes {
            node.dependencies.sort();
            node.dependencies.dedup();
            node.dependents.sort();
            node.dependents.dedup();
        }
        nodes.sort_by(|left, right| left.atom_name.cmp(&right.atom_name));
        nodes
    }

    /// Detect circular dependencies in the dependency graph.
    pub fn detect_circular_dependencies(&self) -> Vec<Vec<String>> {
        let graph = self.build_dependency_graph();
        let adjacency: HashMap<String, Vec<String>> = graph
            .iter()
            .map(|node| (node.atom_name.clone(), node.dependencies.clone()))
            .collect();

        let mut cycles = Vec::new();
        let mut cycle_keys = HashSet::new();
        let mut visited = HashSet::new();
        let mut rec_stack = HashSet::new();
        let mut path = Vec::new();

        let mut atom_names: Vec<&String> = adjacency.keys().collect();
        atom_names.sort();
        for atom_name in atom_names {
            if !visited.contains(atom_name) {
                self.dfs_cycle_detection(
                    atom_name,
                    &adjacency,
                    &mut visited,
                    &mut rec_stack,
                    &mut path,
                    &mut cycles,
                    &mut cycle_keys,
                );
            }
        }

        cycles.sort();
        cycles
    }

    /// Detect mutually contradictory invariants inferred from atom postconditions.
    pub fn detect_global_invariant_conflicts(&self) -> Vec<GlobalInvariantConflict> {
        let mut bounds = Vec::new();
        let mut atom_names: Vec<&String> = self.module_env.atoms.keys().collect();
        atom_names.sort();

        for atom_name in atom_names {
            let Some(atom) = self.module_env.atoms.get(atom_name) else {
                continue;
            };
            let source_file = atom_source_file(atom);
            if source_file == "<unknown>" {
                continue;
            }
            for invariant in split_conjuncts(&atom.ensures) {
                if let Some(bound) = parse_bound_constraint(invariant) {
                    bounds.push(InvariantBound {
                        atom_name: atom_name.clone(),
                        source_file: source_file.clone(),
                        invariant: invariant.to_string(),
                        bound,
                    });
                }
            }
        }

        let mut conflicts = Vec::new();
        let mut seen = HashSet::new();
        for left_index in 0..bounds.len() {
            for right in bounds.iter().skip(left_index + 1) {
                let left = &bounds[left_index];
                if left.atom_name == right.atom_name || left.bound.variable != right.bound.variable
                {
                    continue;
                }
                if !bounds_conflict(&left.bound, &right.bound) {
                    continue;
                }
                let key = canonical_conflict_key(left, right);
                if !seen.insert(key) {
                    continue;
                }
                let left_files = vec![left.source_file.clone()];
                let right_files = vec![right.source_file.clone()];
                conflicts.push(GlobalInvariantConflict {
                    left_invariant: left.invariant.clone(),
                    right_invariant: right.invariant.clone(),
                    left_source_atoms: vec![left.atom_name.clone()],
                    right_source_atoms: vec![right.atom_name.clone()],
                    left_source_files: left_files.clone(),
                    right_source_files: right_files.clone(),
                    message: format!(
                        "Global invariant conflict for '{}': '{}' from {} contradicts '{}' from {}",
                        left.bound.variable,
                        left.invariant,
                        left.source_file,
                        right.invariant,
                        right.source_file
                    ),
                    suggested_fix: format!(
                        "Align the '{}' bounds across {} and {}, or split the atoms into separate domains with explicit preconditions.",
                        left.bound.variable,
                        left.source_file,
                        right.source_file
                    ),
                });
            }
        }
        conflicts.sort_by(|left, right| left.message.cmp(&right.message));
        conflicts
    }

    #[allow(clippy::too_many_arguments)]
    fn dfs_cycle_detection(
        &self,
        current: &str,
        adjacency: &HashMap<String, Vec<String>>,
        visited: &mut HashSet<String>,
        rec_stack: &mut HashSet<String>,
        path: &mut Vec<String>,
        cycles: &mut Vec<Vec<String>>,
        cycle_keys: &mut HashSet<String>,
    ) {
        visited.insert(current.to_string());
        rec_stack.insert(current.to_string());
        path.push(current.to_string());

        if let Some(neighbors) = adjacency.get(current) {
            for neighbor in neighbors {
                if !visited.contains(neighbor) {
                    self.dfs_cycle_detection(
                        neighbor, adjacency, visited, rec_stack, path, cycles, cycle_keys,
                    );
                } else if rec_stack.contains(neighbor) {
                    if let Some(cycle_start) = path.iter().position(|atom| atom == neighbor) {
                        let cycle = path[cycle_start..].to_vec();
                        let key = canonical_cycle_key(&cycle);
                        if cycle_keys.insert(key) {
                            cycles.push(cycle);
                        }
                    }
                }
            }
        }

        path.pop();
        rec_stack.remove(current);
    }

    /// Run all cross-specification verifications.
    pub fn verify_all(&self) -> CrossSpecResult {
        let contract_consistency = self.verify_contract_consistency();
        let global_invariants = self.infer_global_invariants();
        let global_invariant_conflicts = self.detect_global_invariant_conflicts();
        let dependency_graph = self.build_dependency_graph();
        let circular_dependencies = self.detect_circular_dependencies();

        let consistent_calls = contract_consistency
            .iter()
            .filter(|result| result.is_consistent)
            .count();
        let inconsistent_calls = contract_consistency.len() - consistent_calls;

        let summary = CrossSpecSummary {
            total_atoms: self.module_env.atoms.len(),
            consistent_calls,
            inconsistent_calls,
            circular_dependency_count: circular_dependencies.len(),
            global_invariant_count: global_invariants.len(),
            global_invariant_conflict_count: global_invariant_conflicts.len(),
        };

        CrossSpecResult {
            contract_consistency,
            global_invariants,
            global_invariant_conflicts,
            circular_dependencies,
            dependency_graph,
            agent_artifact_mapping: agent_artifact_mapping(),
            summary,
        }
    }

    fn dependencies_for(&self, caller_name: &str, caller_atom: &Atom) -> Vec<String> {
        let mut dependencies: Vec<String> = self
            .module_env
            .dependency_graph
            .get(caller_name)
            .map(|callees| callees.iter().cloned().collect())
            .unwrap_or_else(|| {
                crate::resolver::collect_callees_from_body(&caller_atom.body_expr)
                    .into_iter()
                    .collect()
            });

        dependencies.retain(|name| self.module_env.atoms.contains_key(name));
        dependencies.sort();
        dependencies.dedup();
        dependencies
    }
}

fn agent_artifact_mapping() -> Vec<AgentArtifactMapping> {
    vec![
        AgentArtifactMapping {
            cross_spec_field: "contract_consistency[]".to_string(),
            agent_field: "missing_constraints[]".to_string(),
            contradiction_type: "spec_vs_code".to_string(),
            description: "Caller/callee contract violations correspond to agent-reported constraints that the implementation path does not enforce.".to_string(),
        },
        AgentArtifactMapping {
            cross_spec_field: "global_invariant_conflicts[]".to_string(),
            agent_field: "divergences[]".to_string(),
            contradiction_type: "spec_internal".to_string(),
            description: "Conflicting global invariants correspond to spec-level divergences before migration or repair.".to_string(),
        },
        AgentArtifactMapping {
            cross_spec_field: "circular_dependencies[]".to_string(),
            agent_field: "drift_issues[]".to_string(),
            contradiction_type: "spec_vs_code".to_string(),
            description: "Cycles that change dependency meaning are surfaced as drift issues for human and MCP review.".to_string(),
        },
    ]
}

fn split_conjuncts(constraint: &str) -> impl Iterator<Item = &str> {
    constraint
        .split("&&")
        .map(str::trim)
        .filter(|part| !part.is_empty())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InvariantBound {
    atom_name: String,
    source_file: String,
    invariant: String,
    bound: NumericBound,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NumericBound {
    variable: String,
    kind: BoundKind,
    value: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundKind {
    LowerInclusive,
    LowerExclusive,
    UpperInclusive,
    UpperExclusive,
    Equal,
}

fn parse_bound_constraint(constraint: &str) -> Option<NumericBound> {
    for (operator, kind) in [
        (">=", BoundKind::LowerInclusive),
        (">", BoundKind::LowerExclusive),
        ("<=", BoundKind::UpperInclusive),
        ("<", BoundKind::UpperExclusive),
        ("==", BoundKind::Equal),
    ] {
        let Some((var, bound)) = constraint.split_once(operator) else {
            continue;
        };
        let var = var.trim();
        if !is_simple_identifier(var) {
            return None;
        }
        let value = bound.trim().parse::<i64>().ok()?;
        return Some(NumericBound {
            variable: var.to_string(),
            kind,
            value,
        });
    }
    None
}

fn bounds_conflict(left: &NumericBound, right: &NumericBound) -> bool {
    if let Some((value, inclusive)) = equality_bound(left) {
        return !bound_contains(right, value, inclusive);
    }
    if let Some((value, inclusive)) = equality_bound(right) {
        return !bound_contains(left, value, inclusive);
    }
    match (lower_bound(left), upper_bound(right)) {
        (Some(lower), Some(upper)) if lower_conflicts_with_upper(lower, upper) => return true,
        _ => {}
    }
    matches!(
        (lower_bound(right), upper_bound(left)),
        (Some(lower), Some(upper)) if lower_conflicts_with_upper(lower, upper)
    )
}

fn equality_bound(bound: &NumericBound) -> Option<(i64, bool)> {
    (bound.kind == BoundKind::Equal).then_some((bound.value, true))
}

fn lower_bound(bound: &NumericBound) -> Option<(i64, bool)> {
    match bound.kind {
        BoundKind::LowerInclusive => Some((bound.value, true)),
        BoundKind::LowerExclusive => Some((bound.value, false)),
        BoundKind::Equal => Some((bound.value, true)),
        BoundKind::UpperInclusive | BoundKind::UpperExclusive => None,
    }
}

fn upper_bound(bound: &NumericBound) -> Option<(i64, bool)> {
    match bound.kind {
        BoundKind::UpperInclusive => Some((bound.value, true)),
        BoundKind::UpperExclusive => Some((bound.value, false)),
        BoundKind::Equal => Some((bound.value, true)),
        BoundKind::LowerInclusive | BoundKind::LowerExclusive => None,
    }
}

fn lower_conflicts_with_upper(lower: (i64, bool), upper: (i64, bool)) -> bool {
    let (lower_value, lower_inclusive) = lower;
    let (upper_value, upper_inclusive) = upper;
    lower_value > upper_value
        || (lower_value == upper_value && !(lower_inclusive && upper_inclusive))
}

fn bound_contains(bound: &NumericBound, value: i64, _inclusive: bool) -> bool {
    match bound.kind {
        BoundKind::LowerInclusive => value >= bound.value,
        BoundKind::LowerExclusive => value > bound.value,
        BoundKind::UpperInclusive => value <= bound.value,
        BoundKind::UpperExclusive => value < bound.value,
        BoundKind::Equal => value == bound.value,
    }
}

fn atom_source_file(atom: &Atom) -> String {
    atom.spec_metadata
        .get("source_file")
        .filter(|value| !value.is_empty())
        .cloned()
        .or_else(|| (!atom.span.file.is_empty()).then(|| atom.span.file.clone()))
        .unwrap_or_else(|| "<unknown>".to_string())
}

fn unique_sorted(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values.dedup();
    values
}

fn canonical_conflict_key(left: &InvariantBound, right: &InvariantBound) -> String {
    let mut parts = [
        format!("{}:{}:{}", left.atom_name, left.source_file, left.invariant),
        format!(
            "{}:{}:{}",
            right.atom_name, right.source_file, right.invariant
        ),
    ];
    parts.sort();
    parts.join("|")
}

fn parse_ge_constraint(constraint: &str) -> Option<(String, i64)> {
    let (var, bound) = constraint.split_once(">=")?;
    let var = var.trim();
    if !is_simple_identifier(var) {
        return None;
    }
    let bound = bound.trim().parse::<i64>().ok()?;
    Some((var.to_string(), bound))
}

fn merge_bounds(target: &mut HashMap<String, i64>, source: HashMap<String, i64>) {
    for (var, bound) in source {
        target
            .entry(var)
            .and_modify(|existing| *existing = (*existing).max(bound))
            .or_insert(bound);
    }
}

fn is_simple_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn canonical_cycle_key(cycle: &[String]) -> String {
    if cycle.is_empty() {
        return String::new();
    }
    let min_index = cycle
        .iter()
        .enumerate()
        .min_by(|(_, left), (_, right)| left.cmp(right))
        .map(|(index, _)| index)
        .unwrap_or(0);
    cycle[min_index..]
        .iter()
        .chain(cycle[..min_index].iter())
        .cloned()
        .collect::<Vec<_>>()
        .join("->")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{Param, Span, TrustLevel};

    fn test_atom(name: &str, requires: &str, ensures: &str, body_expr: &str) -> Atom {
        Atom {
            name: name.to_string(),
            type_params: vec![],
            where_bounds: vec![],
            params: vec![Param {
                name: "x".to_string(),
                type_name: Some("i64".to_string()),
                type_ref: Some(crate::parser::parse_type_ref("i64")),
                is_ref: false,
                is_ref_mut: false,
                fn_contract_requires: None,
                fn_contract_ensures: None,
            }],
            trace_id: None,
            spec_metadata: std::collections::HashMap::new(),
            requires: requires.to_string(),
            forall_constraints: vec![],
            ensures: ensures.to_string(),
            body_expr: body_expr.to_string(),
            consumed_params: vec![],
            resources: vec![],
            is_async: false,
            trust_level: TrustLevel::Verified,
            max_unroll: None,
            invariant: None,
            effects: vec![],
            return_type: Some("i64".to_string()),
            span: Span::default(),
            effect_pre: HashMap::new(),
            effect_post: HashMap::new(),
        }
    }

    fn test_env(atoms: Vec<Atom>) -> ModuleEnv {
        let mut env = ModuleEnv::new();
        for atom in atoms {
            let callees = crate::resolver::collect_callees_from_body(&atom.body_expr);
            env.register_dependencies(&atom.name, callees);
            env.register_atom(&atom);
        }
        env
    }

    #[test]
    fn test_dependency_graph_building() {
        let env = test_env(vec![
            test_atom("transfer", "true", "balance >= 0", "validate_balance(x)"),
            test_atom("validate_balance", "x >= 0", "balance >= 0", "x"),
        ]);

        let graph = CrossSpecVerifier::new(&env).build_dependency_graph();
        let transfer = graph
            .iter()
            .find(|node| node.atom_name == "transfer")
            .expect("transfer node");
        let validate = graph
            .iter()
            .find(|node| node.atom_name == "validate_balance")
            .expect("validate_balance node");

        assert_eq!(transfer.dependencies, vec!["validate_balance"]);
        assert_eq!(validate.dependents, vec!["transfer"]);
    }

    #[test]
    fn test_circular_dependency_detection() {
        let env = test_env(vec![
            test_atom("a", "true", "x >= 0", "b(x)"),
            test_atom("b", "true", "x >= 0", "c(x)"),
            test_atom("c", "true", "x >= 0", "a(x)"),
        ]);

        let cycles = CrossSpecVerifier::new(&env).detect_circular_dependencies();

        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0], vec!["a", "b", "c"]);
    }

    #[test]
    fn test_contract_consistency() {
        let env = test_env(vec![
            test_atom("caller", "true", "x >= 0", "callee(x)"),
            test_atom("callee", "x >= 5", "x >= 5", "x"),
        ]);

        let results = CrossSpecVerifier::new(&env).verify_contract_consistency();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].caller_atom, "caller");
        assert_eq!(results[0].callee_atom, "callee");
        assert!(!results[0].is_consistent);
        assert_eq!(results[0].violations.len(), 1);
    }

    #[test]
    fn test_global_invariant_inference() {
        let env = test_env(vec![
            test_atom("transfer", "true", "balance >= 0 && total >= 0", "x"),
            test_atom("withdraw", "true", "balance >= 0", "x"),
            test_atom("deposit", "true", "total >= 0", "x"),
        ]);

        let invariants = CrossSpecVerifier::new(&env).infer_global_invariants();

        let invariant_names: HashSet<String> = invariants
            .iter()
            .map(|invariant| invariant.invariant.clone())
            .collect();
        assert!(invariant_names.contains("balance >= 0"));
        assert!(invariant_names.contains("total >= 0"));
    }
}
