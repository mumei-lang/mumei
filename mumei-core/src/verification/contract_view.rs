use crate::parser::{Atom, ClauseKind, ClauseTrustMode, QuantifierType};
use crate::verification::spec_validation::{split_top_level_conjunctions, strip_wrapping_parens};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractView {
    BodyRequires,
    CallerRequires,
    BodyEnsures,
    CallerEnsures,
}

pub fn contract_view(atom: &Atom, view: ContractView) -> String {
    contract_view_with_dropped_conjuncts(atom, view).0
}

pub fn dropped_conjuncts(atom: &Atom, view: ContractView) -> Vec<String> {
    contract_view_with_dropped_conjuncts(atom, view).1
}

/// The full requires obligation a caller must satisfy: the caller-view
/// requires plus every top-level quantified conjunct extracted into
/// `forall_constraints` (nested quantifiers stayed in `atom.requires`
/// and are already part of the view).
pub fn caller_requires_obligation(atom: &Atom) -> String {
    let base = contract_view(atom, ContractView::CallerRequires);
    if atom.forall_constraints.is_empty() {
        return base;
    }
    let mut parts: Vec<String> = Vec::new();
    if !(base.trim().is_empty() || base.trim() == "true") {
        parts.push(base);
    }
    for q in &atom.forall_constraints {
        let keyword = match q.q_type {
            QuantifierType::ForAll => "forall",
            QuantifierType::Exists => "exists",
        };
        parts.push(format!(
            "{keyword}({}, {}, {}, {})",
            q.var, q.start, q.end, q.condition
        ));
    }
    parts
        .iter()
        .map(|part| format!("({part})"))
        .collect::<Vec<_>>()
        .join(" && ")
}

fn contract_view_with_dropped_conjuncts(atom: &Atom, view: ContractView) -> (String, Vec<String>) {
    let (kind, source, dropped_mode) = match view {
        ContractView::BodyRequires => {
            (ClauseKind::Requires, &atom.requires, ClauseTrustMode::Check)
        }
        ContractView::CallerRequires => (
            ClauseKind::Requires,
            &atom.requires,
            ClauseTrustMode::Assume,
        ),
        ContractView::BodyEnsures => (ClauseKind::Ensures, &atom.ensures, ClauseTrustMode::Assume),
        ContractView::CallerEnsures => (ClauseKind::Ensures, &atom.ensures, ClauseTrustMode::Check),
    };
    if atom.clause_modes.is_empty() {
        return (source.clone(), Vec::new());
    }

    let mut excluded = HashMap::<String, usize>::new();
    for mode in atom
        .clause_modes
        .iter()
        .filter(|mode| mode.kind == kind && mode.mode == dropped_mode)
    {
        for conjunct in flatten_conjuncts(&mode.clause) {
            *excluded.entry(conjunct).or_default() += 1;
        }
    }
    if excluded.is_empty() {
        return (source.clone(), Vec::new());
    }

    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    for conjunct in flatten_conjuncts(source) {
        if let Some(count) = excluded.get_mut(&conjunct) {
            if *count > 0 {
                *count -= 1;
                dropped.push(conjunct);
                continue;
            }
        }
        kept.push(conjunct);
    }

    if dropped.is_empty() {
        return (source.clone(), dropped);
    }

    let retained: HashSet<&str> = kept.iter().map(String::as_str).collect();
    dropped.retain(|conjunct| !retained.contains(conjunct.as_str()));
    let contract = if kept.is_empty() {
        "true".to_string()
    } else {
        kept.iter()
            .map(|conjunct| format!("({conjunct})"))
            .collect::<Vec<_>>()
            .join(" && ")
    };
    (contract, dropped)
}

fn flatten_conjuncts(source: &str) -> Vec<String> {
    let mut flattened = Vec::new();
    for conjunct in split_top_level_conjunctions(source) {
        let conjunct = normalize_conjunct(&conjunct);
        if split_top_level_conjunctions(&conjunct).len() > 1 {
            flattened.extend(flatten_conjuncts(&conjunct));
        } else {
            flattened.push(conjunct);
        }
    }
    flattened
}

fn normalize_conjunct(conjunct: &str) -> String {
    let normalized = strip_wrapping_parens(conjunct.trim());
    let chars: Vec<char> = normalized.chars().collect();
    let mut result = String::with_capacity(normalized.len());
    let mut index = 0;
    while index < chars.len() {
        let current = chars[index];
        result.push(current);
        index += 1;
        if current == '-' && index < chars.len() && chars[index].is_whitespace() {
            let previous = result[..result.len() - current.len_utf8()]
                .chars()
                .rev()
                .find(|character| !character.is_whitespace());
            if previous.is_none_or(|character| "<(=,+-*/!&|".contains(character)) {
                while index < chars.len() && chars[index].is_whitespace() {
                    index += 1;
                }
            }
        }
    }
    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::{caller_requires_obligation, contract_view, dropped_conjuncts, ContractView};
    use crate::parser::item::parse_atom_from_source;

    #[test]
    fn caller_requires_obligation_appends_quantified_conjuncts() {
        let plain = parse_atom_from_source(
            "atom plain(x: i64) -> i64 \
             requires: x > 0; \
             ensures: result > 0; \
             body: x;",
        );
        assert_eq!(
            caller_requires_obligation(&plain),
            contract_view(&plain, ContractView::CallerRequires)
        );

        let quantified = parse_atom_from_source(
            "atom quantified(arr: [i64], n: i64) -> i64 \
             requires: forall(i, 0, n, arr[i] > 0); \
             ensures: result > 0; \
             body: arr[0];",
        );
        assert_eq!(
            caller_requires_obligation(&quantified),
            "(forall(i, 0, n, arr[i] > 0))"
        );

        let mixed = parse_atom_from_source(
            "atom mixed(arr: [i64], n: i64) -> i64 \
             requires: n >= 1 && forall(i, 0, n, arr[i] > 0); \
             ensures: result > 0; \
             body: arr[0];",
        );
        assert_eq!(
            caller_requires_obligation(&mixed),
            "(n >= 1 && true) && (forall(i, 0, n, arr[i] > 0))"
        );
    }

    #[test]
    fn views_apply_clause_modes_without_losing_duplicate_conjuncts() {
        let atom = parse_atom_from_source(
            "atom duplicate(x: i64) -> i64 \
             requires: x > 0; \
             requires assume: x > 0; \
             ensures: result > 0; \
             body: x;",
        );

        assert_eq!(
            contract_view(&atom, ContractView::BodyRequires),
            "(x > 0) && (x > 0)"
        );
        assert_eq!(
            contract_view(&atom, ContractView::CallerRequires),
            "(x > 0)"
        );
    }

    #[test]
    fn dropped_conjuncts_omits_assumed_clauses_retained_by_a_duplicate() {
        let atom = parse_atom_from_source(
            "atom duplicate(x: i64) -> i64 \
             ensures: result > 0; \
             ensures assume: result > 0; \
             body: 1;",
        );

        assert_eq!(
            contract_view(&atom, ContractView::BodyEnsures),
            "(result > 0)"
        );
        assert!(dropped_conjuncts(&atom, ContractView::BodyEnsures).is_empty());
    }

    #[test]
    fn views_split_multi_conjunct_mode_clauses() {
        let atom = parse_atom_from_source(
            "atom assumed(x: i64) -> i64 \
             requires: true; \
             ensures assume: result > 1 && result < 3; \
             body: x;",
        );

        assert_eq!(contract_view(&atom, ContractView::BodyEnsures), "true");
        assert_eq!(
            contract_view(&atom, ContractView::CallerEnsures),
            "result > 1 && result < 3"
        );
    }

    #[test]
    fn views_match_middle_moded_clauses_for_all_contract_sides() {
        let body_requires = parse_atom_from_source(
            "atom body_requires(x: i64) -> i64 \
             requires: x > 0; \
             requires check: x > 1; \
             requires: x > 2; \
             body: x;",
        );
        assert_eq!(
            contract_view(&body_requires, ContractView::BodyRequires),
            "(x > 0) && (x > 2)"
        );

        let caller_requires = parse_atom_from_source(
            "atom caller_requires(x: i64) -> i64 \
             requires: x > 0; \
             requires assume: x > 1; \
             requires: x > 2; \
             body: x;",
        );
        assert_eq!(
            contract_view(&caller_requires, ContractView::CallerRequires),
            "(x > 0) && (x > 2)"
        );

        let body_ensures = parse_atom_from_source(
            "atom body_ensures(x: i64) -> i64 \
             ensures: result > 0; \
             ensures assume: result > 1; \
             ensures: result > 2; \
             body: x;",
        );
        assert_eq!(
            contract_view(&body_ensures, ContractView::BodyEnsures),
            "(result > 0) && (result > 2)"
        );

        let caller_ensures = parse_atom_from_source(
            "atom caller_ensures(x: i64) -> i64 \
             ensures: result > 0; \
             ensures check: result > 1; \
             ensures: result > 2; \
             body: x;",
        );
        assert_eq!(
            contract_view(&caller_ensures, ContractView::CallerEnsures),
            "(result > 0) && (result > 2)"
        );
    }

    #[test]
    fn views_flatten_compound_mode_clauses_next_to_other_clauses() {
        let atom = parse_atom_from_source(
            "atom compound(x: i64) -> i64 \
             ensures: result > 0; \
             ensures assume: result > 100 && result < 200; \
             ensures: result < 300; \
             body: x;",
        );

        assert_eq!(
            contract_view(&atom, ContractView::BodyEnsures),
            "(result > 0) && (result < 300)"
        );

        let checked_compound = parse_atom_from_source(
            "atom checked_compound(x: i64) -> i64 \
             ensures: result > 0; \
             ensures check: result > 100 && result < 200; \
             ensures: result < 300; \
             body: x;",
        );
        assert_eq!(
            contract_view(&checked_compound, ContractView::CallerEnsures),
            "(result > 0) && (result < 300)"
        );
    }

    #[test]
    fn caller_requires_parenthesizes_retained_disjunctions() {
        let atom = parse_atom_from_source(
            "atom disjunction(x: i64) -> i64 \
             requires: x > 10 || x < -10; \
             requires assume: x != 99; \
             requires: x < 50; \
             body: x;",
        );

        assert_eq!(
            contract_view(&atom, ContractView::CallerRequires),
            "(x > 10 || x < -10) && (x < 50)"
        );
    }

    #[test]
    fn views_preserve_full_contract_bytes_when_no_modes_exist() {
        let atom = parse_atom_from_source(
            "atom unchanged(x: i64) -> i64 \
             requires: (x > 0); \
             ensures: (result > 0); \
             body: x;",
        );

        assert_eq!(
            contract_view(&atom, ContractView::BodyRequires),
            atom.requires
        );
        assert_eq!(
            contract_view(&atom, ContractView::CallerEnsures),
            atom.ensures
        );
    }
}
