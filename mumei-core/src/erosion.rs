use crate::charter;
use crate::parser::{Atom, Effect, Item, TrustLevel};
use crate::verification::contract_view::{contract_view, ContractView};
use crate::verification::ModuleEnv;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};

pub const REPORT_SCHEMA: &str = "mumei.erosion-report/v1";

const ERODED_STATUSES: [&str; 7] = [
    "pre_weakened",
    "post_weakened",
    "missing",
    "signature_mismatch",
    "trust_added",
    "effects_widened",
    "inconclusive",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErosionAtomReport {
    pub name: String,
    pub status: String,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counterexample: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_candidate: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErosionTotals {
    pub checked: usize,
    pub eroded: usize,
    pub by_status: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErosionReport {
    pub schema: String,
    pub baseline: String,
    pub candidate: String,
    pub status: String,
    pub atoms: Vec<ErosionAtomReport>,
    pub added: Vec<String>,
    pub totals: ErosionTotals,
}

pub fn compare_contracts(
    baseline_path: &str,
    candidate_path: &str,
    baseline: &[Item],
    candidate: &[Item],
    generic_candidate: &[Item],
    module_env: &ModuleEnv,
) -> ErosionReport {
    let baseline_atoms = charter::implementation_atoms(baseline);
    let mut candidate_atoms = charter::implementation_atoms(candidate);
    let mut candidate_names: HashSet<String> = candidate_atoms
        .iter()
        .map(|atom| atom.name.clone())
        .collect();
    candidate_atoms.extend(
        charter::generic_implementation_atoms(generic_candidate)
            .into_iter()
            .filter(|atom| candidate_names.insert(atom.name.clone())),
    );

    let baseline_names: HashSet<&str> = baseline_atoms
        .iter()
        .map(|atom| atom.name.as_str())
        .collect();
    let added: Vec<String> = candidate_atoms
        .iter()
        .filter(|atom| !baseline_names.contains(atom.name.as_str()))
        .map(|atom| atom.name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let atoms: Vec<ErosionAtomReport> = baseline_atoms
        .iter()
        .map(|baseline_atom| compare_atom(baseline_atom, &candidate_atoms, module_env))
        .collect();
    let mut by_status = BTreeMap::new();
    for atom in &atoms {
        *by_status.entry(atom.status.clone()).or_insert(0) += 1;
    }
    let eroded = atoms
        .iter()
        .filter(|atom| ERODED_STATUSES.contains(&atom.status.as_str()))
        .count();

    ErosionReport {
        schema: REPORT_SCHEMA.to_string(),
        baseline: baseline_path.to_string(),
        candidate: candidate_path.to_string(),
        status: if eroded == 0 { "pass" } else { "fail" }.to_string(),
        atoms,
        added,
        totals: ErosionTotals {
            checked: baseline_atoms.len(),
            eroded,
            by_status,
        },
    }
}

fn compare_atom(
    baseline: &charter::NamedAtom<'_>,
    candidate_atoms: &[charter::NamedAtom<'_>],
    module_env: &ModuleEnv,
) -> ErosionAtomReport {
    let candidate = match charter::match_implementation_atom(&baseline.name, candidate_atoms) {
        Ok(Some(candidate)) => candidate,
        Ok(None) => {
            return atom_result(
                &baseline.name,
                "missing",
                "no matching implementation atom",
                None,
                None,
            )
        }
        Err(names) => {
            return atom_result(
                &baseline.name,
                "signature_mismatch",
                &format!("method name is ambiguous: {}", names.join(", ")),
                None,
                None,
            )
        }
    };
    let matched_candidate = Some(candidate.name.clone());

    if let Some(detail) = charter::signature_mismatch(baseline.atom, candidate.atom) {
        return atom_result(
            &baseline.name,
            "signature_mismatch",
            &detail,
            None,
            matched_candidate,
        );
    }

    if baseline.atom.trust_level != TrustLevel::Trusted
        && candidate.atom.trust_level == TrustLevel::Trusted
    {
        return atom_result(
            &baseline.name,
            "trust_added",
            "candidate marks a verified baseline atom as trusted",
            None,
            matched_candidate,
        );
    }

    if let Some(detail) = effects_widened(baseline.atom, candidate.atom, module_env) {
        return atom_result(
            &baseline.name,
            "effects_widened",
            detail,
            None,
            matched_candidate,
        );
    }

    if contract_text_unchanged(baseline.atom, candidate.atom) {
        return atom_result(
            &baseline.name,
            "preserved",
            "contract text unchanged",
            None,
            matched_candidate,
        );
    }

    if let Some(detail) = contract_field_difference(baseline.atom, candidate.atom) {
        return atom_result(
            &baseline.name,
            "inconclusive",
            detail,
            None,
            matched_candidate,
        );
    }

    if let Err((status, detail, counterexample)) =
        charter::check_refinement(baseline.atom, candidate.atom, module_env)
    {
        return atom_result(
            &baseline.name,
            &status,
            &detail,
            counterexample,
            matched_candidate,
        );
    }

    match charter::check_refinement(candidate.atom, baseline.atom, module_env) {
        Ok(()) => atom_result(
            &baseline.name,
            "preserved",
            "baseline and candidate contracts are equivalent",
            None,
            matched_candidate,
        ),
        Err((status, detail, counterexample)) if status == "inconclusive" => atom_result(
            &baseline.name,
            "refined",
            &format!("reverse refinement was inconclusive: {detail}"),
            counterexample,
            matched_candidate,
        ),
        Err((_, detail, counterexample)) => atom_result(
            &baseline.name,
            "strengthened",
            &format!("candidate contract is stronger than the baseline: {detail}"),
            counterexample,
            matched_candidate,
        ),
    }
}

fn atom_result(
    name: &str,
    status: &str,
    detail: &str,
    counterexample: Option<BTreeMap<String, String>>,
    matched_candidate: Option<String>,
) -> ErosionAtomReport {
    ErosionAtomReport {
        name: name.to_string(),
        status: status.to_string(),
        detail: detail.to_string(),
        counterexample,
        matched_candidate,
    }
}

fn contract_text_unchanged(baseline: &Atom, candidate: &Atom) -> bool {
    normalize_whitespace(&baseline.requires) == normalize_whitespace(&candidate.requires)
        && normalize_whitespace(&baseline.ensures) == normalize_whitespace(&candidate.ensures)
        && charter::normalized_forall_constraints(baseline)
            == charter::normalized_forall_constraints(candidate)
        && normalized_clause_modes(baseline) == normalized_clause_modes(candidate)
        && callback_contracts_unchanged(baseline, candidate)
        && effect_contracts_unchanged(baseline, candidate)
        && normalize_whitespace(&contract_view(baseline, ContractView::CallerEnsures))
            == normalize_whitespace(&contract_view(candidate, ContractView::CallerEnsures))
}

fn effects_widened(
    baseline: &Atom,
    candidate: &Atom,
    module_env: &ModuleEnv,
) -> Option<&'static str> {
    let baseline_negative: HashSet<String> = baseline
        .effects
        .iter()
        .filter(|effect| effect.negated)
        .map(normalized_effect)
        .collect();
    let candidate_negative: HashSet<String> = candidate
        .effects
        .iter()
        .filter(|effect| effect.negated)
        .map(normalized_effect)
        .collect();
    if baseline_negative
        .iter()
        .any(|effect| !candidate_negative.contains(effect))
    {
        return Some("candidate drops a forbidden effect");
    }

    let baseline_positive: Vec<&Effect> = baseline
        .effects
        .iter()
        .filter(|effect| !effect.negated)
        .collect();
    let candidate_widens = candidate
        .effects
        .iter()
        .filter(|effect| !effect.negated)
        .any(|candidate_effect| {
            !baseline_positive.iter().any(|baseline_effect| {
                effect_is_covered(candidate_effect, baseline_effect, module_env)
            })
        });
    candidate_widens.then_some("candidate effects include effects not present in the baseline")
}

fn effect_is_covered(candidate: &Effect, baseline: &Effect, module_env: &ModuleEnv) -> bool {
    if normalized_effect_parameters(candidate) != normalized_effect_parameters(baseline) {
        return false;
    }
    if candidate.name == baseline.name || module_env.is_subeffect(&candidate.name, &baseline.name) {
        return true;
    }

    let candidate_leaves = module_env.resolve_leaf_effects(std::slice::from_ref(&candidate.name));
    let baseline_leaves = module_env.resolve_leaf_effects(std::slice::from_ref(&baseline.name));
    candidate_leaves.is_subset(&baseline_leaves)
}

fn normalized_effect(effect: &Effect) -> String {
    normalize_whitespace(&format!("{effect:?}"))
}

fn normalized_effect_parameters(effect: &Effect) -> String {
    normalize_whitespace(&format!("{:?}", effect.params))
}

fn effect_contracts_unchanged(baseline: &Atom, candidate: &Atom) -> bool {
    normalized_effect_states(&baseline.effect_pre)
        == normalized_effect_states(&candidate.effect_pre)
        && normalized_effect_states(&baseline.effect_post)
            == normalized_effect_states(&candidate.effect_post)
}

fn contract_field_difference(baseline: &Atom, candidate: &Atom) -> Option<&'static str> {
    if normalized_clause_modes(baseline) != normalized_clause_modes(candidate) {
        return Some("clause modes differ");
    }
    if normalized_effect_states(&baseline.effect_pre)
        != normalized_effect_states(&candidate.effect_pre)
    {
        return Some("effect contracts differ: effect_pre");
    }
    if normalized_effect_states(&baseline.effect_post)
        != normalized_effect_states(&candidate.effect_post)
    {
        return Some("effect contracts differ: effect_post");
    }
    callback_contract_difference(baseline, candidate)
}

fn callback_contracts_unchanged(baseline: &Atom, candidate: &Atom) -> bool {
    baseline.params.len() == candidate.params.len()
        && baseline
            .params
            .iter()
            .zip(&candidate.params)
            .all(|(baseline, candidate)| {
                normalize_optional_contract(baseline.fn_contract_requires.as_deref())
                    == normalize_optional_contract(candidate.fn_contract_requires.as_deref())
                    && normalize_optional_contract(baseline.fn_contract_ensures.as_deref())
                        == normalize_optional_contract(candidate.fn_contract_ensures.as_deref())
            })
}

fn callback_contract_difference(baseline: &Atom, candidate: &Atom) -> Option<&'static str> {
    for (baseline, candidate) in baseline.params.iter().zip(&candidate.params) {
        if normalize_optional_contract(baseline.fn_contract_requires.as_deref())
            != normalize_optional_contract(candidate.fn_contract_requires.as_deref())
        {
            return Some("callback contract differs: fn_contract_requires");
        }
        if normalize_optional_contract(baseline.fn_contract_ensures.as_deref())
            != normalize_optional_contract(candidate.fn_contract_ensures.as_deref())
        {
            return Some("callback contract differs: fn_contract_ensures");
        }
    }
    None
}

fn normalize_optional_contract(value: Option<&str>) -> Option<String> {
    value.map(normalize_whitespace)
}

fn normalized_effect_states(
    states: &std::collections::HashMap<String, String>,
) -> BTreeMap<String, String> {
    states
        .iter()
        .map(|(effect, state)| (effect.clone(), normalize_whitespace(state)))
        .collect()
}

fn normalized_clause_modes(atom: &Atom) -> Vec<String> {
    atom.clause_modes
        .iter()
        .map(|mode| normalize_whitespace(&format!("{mode:?}")))
        .collect()
}

fn normalize_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::compare_contracts;
    use crate::parser;
    use crate::verification::ModuleEnv;

    fn parse(source: &str) -> Vec<parser::Item> {
        parser::parse_module_checked(source).expect("parse fixture")
    }

    fn compare(baseline: &str, candidate: &str) -> super::ErosionReport {
        let baseline = parse(baseline);
        let candidate = parse(candidate);
        let mut module_env = ModuleEnv::new();
        for item in &candidate {
            if let parser::Item::EffectDef(effect_def) = item {
                module_env.register_effect(effect_def);
            }
        }
        compare_contracts(
            "baseline.mm",
            "candidate.mm",
            &baseline,
            &candidate,
            &candidate,
            &module_env,
        )
    }

    fn atom_status<'a>(report: &'a super::ErosionReport, name: &str) -> &'a str {
        report
            .atoms
            .iter()
            .find(|atom| atom.name == name)
            .expect("atom report")
            .status
            .as_str()
    }

    #[test]
    fn identical_contract_text_skips_refinement_even_for_unsupported_clauses() {
        let report = compare(
            "trusted atom f(x: i64) -> i64 \
             requires: unknown_predicate(x); ensures: result > x; body: 0;",
            "atom f(x: i64) -> i64 \
             requires: unknown_predicate ( x ); ensures: result > x; body: x + 1;",
        );

        assert_eq!(report.status, "pass");
        assert_eq!(atom_status(&report, "f"), "preserved");
        assert_eq!(report.atoms[0].detail, "contract text unchanged");
    }

    #[test]
    fn trust_escalation_fails_but_trust_removal_is_allowed() {
        let trust_added = compare(
            "atom f(x: i64) -> i64 ensures: result == x; body: x;",
            "trusted atom f(x: i64) -> i64 ensures: result == x; body: x;",
        );
        assert_eq!(trust_added.status, "fail");
        assert_eq!(atom_status(&trust_added, "f"), "trust_added");
        assert_eq!(
            trust_added.atoms[0].detail,
            "candidate marks a verified baseline atom as trusted"
        );

        let trust_removed = compare(
            "trusted atom f(x: i64) -> i64 ensures: result == x; body: x;",
            "atom f(x: i64) -> i64 ensures: result == x; body: x;",
        );
        assert_eq!(trust_removed.status, "pass");
        assert_eq!(atom_status(&trust_removed, "f"), "preserved");
    }

    #[test]
    fn effects_may_be_removed_or_reordered_and_sub_effects_are_narrowing() {
        let widened = compare(
            "atom f(x: i64) -> i64 effects: [File]; ensures: result == x; body: x;",
            "atom f(x: i64) -> i64 effects: [File, Network]; ensures: result == x; body: x;",
        );
        assert_eq!(widened.status, "fail");
        assert_eq!(atom_status(&widened, "f"), "effects_widened");

        let removed = compare(
            "atom f(x: i64) -> i64 effects: [File, Network]; ensures: result == x; body: x;",
            "atom f(x: i64) -> i64 effects: [File]; ensures: result == x; body: x;",
        );
        assert_eq!(removed.status, "pass");
        assert_eq!(atom_status(&removed, "f"), "preserved");

        let reordered = compare(
            "atom f(x: i64) -> i64 effects: [File, Network]; ensures: result == x; body: x;",
            "atom f(x: i64) -> i64 effects: [Network, File]; ensures: result == x; body: x;",
        );
        assert_eq!(reordered.status, "pass");
        assert_eq!(atom_status(&reordered, "f"), "preserved");

        let sub_effect = compare(
            "effect IO;\n\
             effect FileRead parent: IO;\n\
             atom f(x: i64) -> i64 effects: [IO]; ensures: result == x; body: x;",
            "effect IO;\n\
             effect FileRead parent: IO;\n\
             atom f(x: i64) -> i64 effects: [FileRead]; ensures: result == x; body: x;",
        );
        assert_eq!(sub_effect.status, "pass");
        assert_eq!(atom_status(&sub_effect, "f"), "preserved");

        let super_effect = compare(
            "effect IO;\n\
             effect FileRead parent: IO;\n\
             atom f(x: i64) -> i64 effects: [FileRead]; ensures: result == x; body: x;",
            "effect IO;\n\
             effect FileRead parent: IO;\n\
             atom f(x: i64) -> i64 effects: [IO]; ensures: result == x; body: x;",
        );
        assert_eq!(super_effect.status, "fail");
        assert_eq!(atom_status(&super_effect, "f"), "effects_widened");
    }

    #[test]
    fn matched_methods_are_refined_without_rematching_unqualified_names() {
        let report = compare(
            "impl Point { trusted atom get(self: Point) -> i64 \
             ensures: result >= 0; body: 0; }\n\
             impl Other { trusted atom get(self: Other) -> i64 \
             ensures: result >= 0; body: 0; }\n\
             trusted atom get(x: i64) -> i64 ensures: result >= 0; body: 0;",
            "impl Point { atom get(self: Point) -> i64 body: 0; }\n\
             impl Other { atom get(self: Other) -> i64 \
             ensures: result >= 0; body: 0; }\n\
             atom get(x: i64) -> i64 ensures: result >= 0; body: 0;",
        );

        assert_eq!(atom_status(&report, "Point::get"), "post_weakened");
        assert_eq!(atom_status(&report, "Other::get"), "preserved");
        assert_eq!(atom_status(&report, "get"), "preserved");
        assert!(!report
            .atoms
            .iter()
            .any(|atom| atom.detail.contains("ambiguous")));
    }

    #[test]
    fn effect_parameters_are_part_of_the_effect_identity() {
        let report = compare(
            "atom f(x: i64, y: i64) -> i64 effects: [File(x)]; body: x;",
            "atom f(x: i64, y: i64) -> i64 effects: [File(y)]; body: x;",
        );

        assert_eq!(report.status, "fail");
        assert_eq!(atom_status(&report, "f"), "effects_widened");
    }

    #[test]
    fn adding_forbidden_effects_is_narrowing_but_dropping_one_is_not() {
        let added = compare(
            "atom f(x: i64) -> i64 effects: [!Network]; body: x;",
            "atom f(x: i64) -> i64 effects: [!Network, !FileRead]; body: x;",
        );
        assert_eq!(added.status, "pass");
        assert_eq!(atom_status(&added, "f"), "preserved");

        let removed = compare(
            "atom f(x: i64) -> i64 effects: [!Network]; body: x;",
            "atom f(x: i64) -> i64 body: x;",
        );
        assert_eq!(removed.status, "fail");
        assert_eq!(atom_status(&removed, "f"), "effects_widened");
        assert_eq!(
            removed.atoms[0].detail,
            "candidate drops a forbidden effect"
        );
    }

    #[test]
    fn effects_without_a_resolvable_hierarchy_require_exact_identity() {
        let replaced = compare(
            "atom f(x: i64) -> i64 effects: [UnknownBase]; body: x;",
            "atom f(x: i64) -> i64 effects: [UnknownCandidate]; body: x;",
        );

        assert_eq!(replaced.status, "fail");
        assert_eq!(atom_status(&replaced, "f"), "effects_widened");
    }

    #[test]
    fn changed_effect_contracts_are_inconclusive() {
        let effect_post = compare(
            "atom f(x: i64) -> i64 effect_pre: { File: Open }; \
             effect_post: { File: Closed }; body: x;",
            "atom f(x: i64) -> i64 effect_pre: { File: Open }; \
             effect_post: { File: Open }; body: x;",
        );
        assert_eq!(effect_post.status, "fail");
        assert_eq!(atom_status(&effect_post, "f"), "inconclusive");
        assert_eq!(
            effect_post.atoms[0].detail,
            "effect contracts differ: effect_post"
        );

        let effect_pre = compare(
            "atom f(x: i64) -> i64 effect_pre: { File: Open }; body: x;",
            "atom f(x: i64) -> i64 effect_pre: { File: Closed }; body: x;",
        );
        assert_eq!(effect_pre.status, "fail");
        assert_eq!(atom_status(&effect_pre, "f"), "inconclusive");
        assert_eq!(
            effect_pre.atoms[0].detail,
            "effect contracts differ: effect_pre"
        );
    }

    #[test]
    fn clause_mode_changes_are_inconclusive_in_both_directions() {
        for (baseline, candidate) in [
            (
                "atom f(x: i64) -> i64 requires assume: x > 0; body: x;",
                "atom f(x: i64) -> i64 requires: x > 0; body: x;",
            ),
            (
                "atom f(x: i64) -> i64 requires: x > 0; body: x;",
                "atom f(x: i64) -> i64 requires assume: x > 0; body: x;",
            ),
        ] {
            let report = compare(baseline, candidate);
            assert_eq!(report.status, "fail");
            assert_eq!(atom_status(&report, "f"), "inconclusive");
            assert_eq!(report.atoms[0].detail, "clause modes differ");
        }
    }

    #[test]
    fn callback_contract_changes_fail_closed_but_body_edits_shortcut() {
        let callback_contract = "contract(f): requires: x >= 0, ensures: result >= 0;";
        let baseline = format!(
            "atom apply(x: i64, f: atom_ref(i64) -> i64) -> i64 \
             requires: x >= 0; ensures: result >= 0; {callback_contract} body: x;"
        );
        let removed_callback = compare(
            &baseline,
            "atom apply(x: i64, f: atom_ref(i64) -> i64) -> i64 \
             requires: x >= 0; ensures: result >= 0; body: x;",
        );
        assert_eq!(removed_callback.status, "fail");
        assert_eq!(atom_status(&removed_callback, "apply"), "inconclusive");
        assert_eq!(
            removed_callback.atoms[0].detail,
            "callback contract differs: fn_contract_requires"
        );

        let body_only = compare(
            &baseline,
            &format!(
                "atom apply(x: i64, f: atom_ref(i64) -> i64) -> i64 \
                 requires: x >= 0; ensures: result >= 0; {callback_contract} body: x + 1;"
            ),
        );
        assert_eq!(body_only.status, "pass");
        assert_eq!(atom_status(&body_only, "apply"), "preserved");
        assert_eq!(body_only.atoms[0].detail, "contract text unchanged");
    }

    #[test]
    fn whitespace_normalization_preserves_token_boundaries() {
        let report = compare(
            "trusted atom f() -> Str ensures: result == \"a b\"; body: \"a b\";",
            "atom f() -> Str ensures: result == \"ab\"; body: \"ab\";",
        );

        assert_ne!(report.atoms[0].detail, "contract text unchanged");
    }

    #[test]
    fn unchanged_effect_contracts_keep_the_body_only_shortcut() {
        let report = compare(
            "atom f(x: i64) -> i64 effects: [File]; \
             effect_pre: { File: Open }; effect_post: { File: Closed }; body: x;",
            "atom f(x: i64) -> i64 effects: [File]; \
             effect_pre: { File: Open }; effect_post: { File: Closed }; body: x + 1;",
        );

        assert_eq!(report.status, "pass");
        assert_eq!(atom_status(&report, "f"), "preserved");
        assert_eq!(report.atoms[0].detail, "contract text unchanged");
    }

    #[test]
    fn dropped_ensures_is_post_weakened_with_counterexample() {
        let report = compare(
            "trusted atom f(x: i64) -> i64 ensures: result >= x + 1; body: 0;",
            "atom f(x: i64) -> i64 ensures: result >= x; body: x;",
        );

        assert_eq!(report.status, "fail");
        assert_eq!(atom_status(&report, "f"), "post_weakened");
        assert!(report.atoms[0].counterexample.is_some());
    }

    #[test]
    fn tightened_requires_is_pre_weakened_with_counterexample() {
        let report = compare(
            "trusted atom f(x: i64) -> i64 requires: x >= 0; body: 0;",
            "atom f(x: i64) -> i64 requires: x > 0; body: 0;",
        );

        assert_eq!(atom_status(&report, "f"), "pre_weakened");
        assert!(report.atoms[0].counterexample.is_some());
    }

    #[test]
    fn stronger_ensures_and_relaxed_requires_are_strengthened() {
        let stronger_ensures = compare(
            "trusted atom f(x: i64) -> i64 ensures: result >= x; body: 0;",
            "atom f(x: i64) -> i64 ensures: result > x; body: x + 1;",
        );
        let relaxed_requires = compare(
            "trusted atom f(x: i64) -> i64 requires: x > 0; body: 0;",
            "atom f(x: i64) -> i64 requires: x >= 0; body: 0;",
        );

        assert_eq!(stronger_ensures.status, "pass");
        assert_eq!(atom_status(&stronger_ensures, "f"), "strengthened");
        assert!(stronger_ensures.atoms[0].counterexample.is_some());
        assert_eq!(relaxed_requires.status, "pass");
        assert_eq!(atom_status(&relaxed_requires, "f"), "strengthened");
        assert!(relaxed_requires.atoms[0].counterexample.is_some());
    }

    #[test]
    fn equivalent_rewrite_is_preserved_by_both_directions() {
        let report = compare(
            "trusted atom f(x: i64, y: i64) -> i64 \
             requires: x <= y; ensures: result >= x; body: 0;",
            "atom f(x: i64, y: i64) -> i64 \
             requires: y >= x; ensures: result >= x; body: 0;",
        );

        assert_eq!(report.status, "pass");
        assert_eq!(atom_status(&report, "f"), "preserved");
        assert!(report.atoms[0].detail.contains("contracts are equivalent"));
    }

    #[test]
    fn removed_atoms_and_signature_changes_are_reported() {
        let removed = compare("trusted atom gone(x: i64) -> i64 body: 0;", "");
        assert_eq!(atom_status(&removed, "gone"), "missing");

        for (baseline, candidate) in [
            (
                "trusted atom f(x: i64) -> i64 body: 0;",
                "atom f(x: f64) -> i64 body: 0;",
            ),
            (
                "trusted atom f(x: i64) -> i64 body: 0;",
                "atom f(x: i64, y: i64) -> i64 body: 0;",
            ),
            (
                "trusted atom f(x: i64) -> i64 body: 0;",
                "atom f(x: i64) -> bool body: false;",
            ),
            (
                "trusted atom f(ref x: i64) -> i64 body: 0;",
                "atom f(consume x: i64) -> i64 body: 0;",
            ),
        ] {
            let report = compare(baseline, candidate);
            assert_eq!(atom_status(&report, "f"), "signature_mismatch");
        }
    }

    #[test]
    fn method_and_generic_atoms_are_compared() {
        let method = compare(
            "impl Point { trusted atom get(self: Point) -> i64 \
             ensures: result >= 0; body: 0; }",
            "impl Point { atom get(self: Point) -> i64 body: 0; }",
        );
        assert_eq!(atom_status(&method, "Point::get"), "post_weakened");

        let generic = compare(
            "trusted atom identity<T>(x: T) -> T \
             ensures: result == x; body: x;",
            "atom identity<T>(x: T) -> T body: x;",
        );
        assert_eq!(atom_status(&generic, "identity"), "post_weakened");
    }

    #[test]
    fn baseline_atoms_and_methods_keep_source_order() {
        let report = compare(
            "trusted atom first(x: i64) -> i64 body: 0;\n\
             impl Point { trusted atom get(self: Point) -> i64 body: 0; }\n\
             trusted atom last(x: i64) -> i64 body: 0;",
            "atom first(x: i64) -> i64 body: 0;\n\
             impl Point { atom get(self: Point) -> i64 body: 0; }\n\
             atom last(x: i64) -> i64 body: 0;",
        );

        let names: Vec<&str> = report.atoms.iter().map(|atom| atom.name.as_str()).collect();
        assert_eq!(names, ["first", "Point::get", "last"]);
        assert!(report.atoms.iter().all(|atom| atom.status == "preserved"));
    }

    #[test]
    fn differing_forall_contracts_are_inconclusive() {
        let report = compare(
            "trusted atom f(n: i64) -> i64 \
             requires: forall(i, 0, n, i >= 0); body: 0;",
            "atom f(n: i64) -> i64 \
             requires: forall(i, 0, n, i >= 0) && forall(j, 0, n, j >= 0); body: 0;",
        );

        assert_eq!(report.status, "fail");
        assert_eq!(atom_status(&report, "f"), "inconclusive");
        assert_eq!(report.atoms[0].detail, "quantified clauses differ");
    }

    #[test]
    fn ensures_check_mode_change_is_inconclusive() {
        let report = compare(
            "trusted atom f(x: i64) -> i64 ensures: result >= x + 1; body: 0;",
            "atom f(x: i64) -> i64 ensures check: result >= x + 1; body: 0;",
        );

        assert_eq!(atom_status(&report, "f"), "inconclusive");
        assert_eq!(report.atoms[0].detail, "clause modes differ");
    }

    #[test]
    fn added_atoms_are_informational_and_trusted_baselines_are_checked() {
        let added = compare(
            "trusted atom f(x: i64) -> i64 ensures: result >= x; body: 0;",
            "atom f(x: i64) -> i64 ensures: result >= x; body: x; \
             atom z() -> i64 body: 0; atom a() -> i64 body: 0;",
        );
        assert_eq!(added.status, "pass");
        assert_eq!(atom_status(&added, "f"), "preserved");
        assert_eq!(added.added, ["a", "z"]);

        let trusted_weakened = compare(
            "trusted atom f(x: i64) -> i64 ensures: result >= x + 1; body: 0;",
            "atom f(x: i64) -> i64 ensures: result >= x; body: x;",
        );
        assert_eq!(atom_status(&trusted_weakened, "f"), "post_weakened");
    }

    #[test]
    fn duplicate_candidate_atoms_keep_charter_ambiguity_behavior() {
        let report = compare(
            "trusted atom f(x: i64) -> i64 ensures: result >= x + 1; body: 0;",
            "atom f(x: i64) -> i64 ensures: result >= x + 1; body: x + 1; \
             atom f(x: i64) -> i64 ensures: result == x; body: x;",
        );

        assert_eq!(atom_status(&report, "f"), "signature_mismatch");
        assert!(report.atoms[0].detail.contains("ambiguous"));
        assert_eq!(report.atoms[0].matched_candidate, None);
    }
}
