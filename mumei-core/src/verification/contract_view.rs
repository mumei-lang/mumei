use crate::parser::{Atom, ClauseKind, ClauseTrustMode};
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
        for conjunct in split_top_level_conjunctions(&mode.clause) {
            let conjunct = normalize_conjunct(&conjunct);
            *excluded.entry(conjunct).or_default() += 1;
        }
    }
    if excluded.is_empty() {
        return (source.clone(), Vec::new());
    }

    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    for conjunct in split_top_level_conjunctions(source) {
        let conjunct = normalize_conjunct(&conjunct);
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
        kept.join(" && ")
    };
    (contract, dropped)
}

fn normalize_conjunct(conjunct: &str) -> String {
    strip_wrapping_parens(conjunct.trim()).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::{contract_view, dropped_conjuncts, ContractView};
    use crate::parser::item::parse_atom_from_source;

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
        assert_eq!(contract_view(&atom, ContractView::CallerRequires), "x > 0");
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
            "result > 0"
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
