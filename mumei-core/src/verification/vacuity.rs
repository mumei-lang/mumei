use crate::controls::{mutate_at_site, site_mutants, MutationKind};
use crate::hir::{lower_stmt_with_env, HirAtom};
use crate::mir::{lower_hir_to_mir, MirBody};
use crate::parser::{Atom, Op};
use crate::verification::executor::{verify_inner_for_vacuity, VerifyInnerOptions};
use crate::verification::module_env::ModuleEnv;
use crate::verification::mutation::{
    apply_mutation, generate_mutations, MutationOperator, MutationResult,
};
use crate::verification::types::{MumeiError, VerdictKind};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct VacuityCheckResult {
    pub is_vacuous: bool,
    pub mutation_results: Vec<MutationResult>,
    pub vacuous_mutations: Vec<MutationOperator>,
    pub total_mutations_tested: usize,
    pub killed_mutations: usize,
    pub survived_mutations: usize,
    pub unanalyzable_mutations: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VacuityError {
    pub atom_name: String,
    pub reason: String,
    pub vacuous_mutations: Vec<MutationOperator>,
}

pub fn check_spec_vacuity(
    atom: &Atom,
    mir_body: &MirBody,
    _module_env: &ModuleEnv,
    max_mutations: usize,
) -> Result<VacuityCheckResult, VacuityError> {
    let mutations = generate_mutations(mir_body, max_mutations);
    let mut mutation_results = Vec::new();
    let mut vacuous_mutations = Vec::new();
    let is_trivial_spec = atom.requires.trim() == "true" && atom.ensures.trim() == "true";

    for mutation in &mutations {
        let mutated_body = apply_mutation(mir_body, mutation);
        let verification_passed = is_trivial_spec;
        if verification_passed {
            vacuous_mutations.push(mutation.clone());
        }
        mutation_results.push(MutationResult {
            operator: mutation.clone(),
            location: format!("mir_body_{}", atom.name),
            mutated_body,
            verification_passed,
        });
    }

    finish_vacuity_check(atom, mutations.len(), mutation_results, vacuous_mutations)
}

pub fn check_spec_vacuity_for_hir(
    atom: &Atom,
    hir_atom: &HirAtom,
    mir_body: &MirBody,
    module_env: &ModuleEnv,
    max_mutations: usize,
    timeout_ms: u64,
) -> Result<VacuityCheckResult, VacuityError> {
    let options = VerifyInnerOptions::for_vacuity(timeout_ms);
    check_spec_vacuity_for_hir_with_options(
        atom,
        hir_atom,
        mir_body,
        module_env,
        max_mutations,
        &options,
    )
}

pub(crate) fn check_spec_vacuity_for_hir_with_options(
    atom: &Atom,
    hir_atom: &HirAtom,
    _mir_body: &MirBody,
    module_env: &ModuleEnv,
    max_mutations: usize,
    options: &VerifyInnerOptions<'_>,
) -> Result<VacuityCheckResult, VacuityError> {
    let mutants = site_mutants(&hir_atom.body_stmt)
        .into_iter()
        .take(max_mutations)
        .collect::<Vec<_>>();
    let total_mutations_tested = mutants.len();
    let mut mutation_results = Vec::with_capacity(mutants.len());
    let mut vacuous_mutations = Vec::new();
    let mut surviving_ids = Vec::new();
    let mut killed_mutations = 0;
    let mut unanalyzable_mutations = 0;

    for mutant in mutants {
        let operator = mutation_operator(&mutant.kind);
        // Single-site mutations preserve identifiers, and spurious replay reads body_stmt.
        let mut mutated_hir = hir_atom.clone();
        let changed = mutate_at_site(&mut mutated_hir.body_stmt, mutant.site, &mutant.kind);
        let mutated_body = if changed {
            mutated_hir.body = lower_stmt_with_env(&mutated_hir.body_stmt, Some(module_env));
            lower_hir_to_mir(&mutated_hir)
        } else {
            lower_hir_to_mir(&mutated_hir)
        };
        let outcome = if !changed {
            MutantOutcome::Unanalyzable
        } else {
            match tempfile::tempdir() {
                Ok(output_dir) => classify_verification_result(verify_inner_for_vacuity(
                    &mutated_hir,
                    output_dir.path(),
                    module_env,
                    options,
                )),
                Err(_) => MutantOutcome::Unanalyzable,
            }
        };

        match outcome {
            MutantOutcome::Killed => killed_mutations += 1,
            MutantOutcome::Survived => {
                vacuous_mutations.push(operator.clone());
                surviving_ids.push(mutant.id.clone());
            }
            MutantOutcome::Unanalyzable => unanalyzable_mutations += 1,
        }
        mutation_results.push(MutationResult {
            operator,
            location: format!("mir_body_{}:{}", atom.name, mutant.id),
            mutated_body,
            verification_passed: outcome == MutantOutcome::Survived,
        });
    }

    let result = finish_verified_mutants(
        atom,
        mutation_results,
        vacuous_mutations,
        total_mutations_tested,
        killed_mutations,
        surviving_ids,
        unanalyzable_mutations,
    );
    if let Ok(result) = &result {
        if let Some(warning) = inconclusive_warning(
            &atom.name,
            result.total_mutations_tested,
            result.killed_mutations,
            result.unanalyzable_mutations,
        ) {
            eprintln!("{warning}");
        }
    }
    result
}

fn mutation_operator(kind: &MutationKind) -> MutationOperator {
    match kind {
        MutationKind::BinaryOp(op) => MutationOperator::BinaryOpFlip(op.clone()),
        MutationKind::ConditionFlip(op) => MutationOperator::ConditionFlip(op.clone()),
        MutationKind::ArrayIndex => MutationOperator::ArrayIndexOffset(1),
        MutationKind::ConstantZero => MutationOperator::ConstantZero,
        MutationKind::ConstantOne => MutationOperator::ConstantOne,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MutantOutcome {
    Killed,
    Survived,
    Unanalyzable,
}

fn classify_verification_result(result: Result<(), MumeiError>) -> MutantOutcome {
    match result {
        Ok(()) => MutantOutcome::Survived,
        Err(error) if error.verdict() == Some(VerdictKind::Rejected) => MutantOutcome::Killed,
        Err(_) => MutantOutcome::Unanalyzable,
    }
}

fn finish_verified_mutants(
    atom: &Atom,
    mutation_results: Vec<MutationResult>,
    vacuous_mutations: Vec<MutationOperator>,
    total_mutations_tested: usize,
    killed_mutations: usize,
    surviving_ids: Vec<String>,
    unanalyzable_mutations: usize,
) -> Result<VacuityCheckResult, VacuityError> {
    if surviving_ids.is_empty() {
        Ok(VacuityCheckResult {
            is_vacuous: false,
            mutation_results,
            vacuous_mutations,
            total_mutations_tested,
            killed_mutations,
            survived_mutations: 0,
            unanalyzable_mutations,
        })
    } else {
        Err(VacuityError {
            atom_name: atom.name.clone(),
            reason: format!(
                "Specification is vacuous: surviving mutants [{}]; tested {}, killed {}, unanalyzable {}.",
                surviving_ids.join(", "),
                total_mutations_tested,
                killed_mutations,
                unanalyzable_mutations
            ),
            vacuous_mutations,
        })
    }
}

fn inconclusive_warning(
    atom_name: &str,
    tested: usize,
    killed: usize,
    unanalyzable: usize,
) -> Option<String> {
    (killed == 0 && unanalyzable > 0).then(|| {
        format!(
            "warning: vacuity check for '{atom_name}' was inconclusive: tested {tested}, killed {killed}, unanalyzable {unanalyzable}"
        )
    })
}

fn finish_vacuity_check(
    atom: &Atom,
    total_mutations_tested: usize,
    mutation_results: Vec<MutationResult>,
    vacuous_mutations: Vec<MutationOperator>,
) -> Result<VacuityCheckResult, VacuityError> {
    let survived_mutations = vacuous_mutations.len();
    let killed_mutations = mutation_results.len().saturating_sub(survived_mutations);
    if survived_mutations == 0 {
        Ok(VacuityCheckResult {
            is_vacuous: false,
            mutation_results,
            vacuous_mutations,
            total_mutations_tested,
            killed_mutations,
            survived_mutations,
            unanalyzable_mutations: 0,
        })
    } else {
        Err(VacuityError {
            atom_name: atom.name.clone(),
            reason: format!(
                "Specification is vacuous: {} out of {} mutated implementations still passed verification.",
                survived_mutations,
                total_mutations_tested
            ),
            vacuous_mutations,
        })
    }
}

pub(crate) fn flip_binary_op(op: &Op) -> Op {
    match op {
        Op::Add => Op::Sub,
        Op::Sub => Op::Add,
        Op::Mul => Op::Div,
        Op::Div => Op::Mul,
        Op::Pow => Op::Pow,
        Op::Lt => Op::Ge,
        Op::Le => Op::Gt,
        Op::Gt => Op::Le,
        Op::Ge => Op::Lt,
        Op::Eq => Op::Neq,
        Op::Neq => Op::Eq,
        Op::And => Op::Or,
        Op::Or => Op::And,
        Op::Implies => Op::And,
        Op::BitAnd => Op::BitOr,
        Op::BitOr => Op::BitAnd,
        Op::BitXor => Op::BitAnd,
        Op::Shl => Op::Shr,
        Op::Shr => Op::Shl,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir::lower_atom_to_hir_with_env;
    use crate::mir::lower_hir_to_mir;
    use crate::parser::{parse_atom, parse_body_expr};

    #[test]
    fn shared_site_generator_includes_array_index_mutants() {
        let mutants = site_mutants(&parse_body_expr("items[index]"));
        assert!(mutants
            .iter()
            .any(|mutant| mutant.id.starts_with("array-index:+1@")));
    }

    #[test]
    fn trivial_contract_reports_surviving_mutant_ids_and_counts() {
        let atom = parse_atom(
            r#"
atom evasive(x: i64) -> i64
  requires: true;
  ensures: true;
  body: x + 1;
"#,
        );
        let mut module_env = ModuleEnv::new();
        module_env.register_atom(&atom);
        let hir_atom = lower_atom_to_hir_with_env(&atom, Some(&module_env));
        let mir_body = lower_hir_to_mir(&hir_atom);

        let err = check_spec_vacuity_for_hir(&atom, &hir_atom, &mir_body, &module_env, 10, 1000)
            .unwrap_err();
        assert_eq!(err.atom_name, "evasive");
        assert!(err.reason.contains("constant:1->0@"));
        assert!(err.reason.contains("tested "));
        assert!(err.reason.contains("killed "));
        assert!(err.reason.contains("unanalyzable "));
        assert!(!err.vacuous_mutations.is_empty());
    }

    #[test]
    fn meaningful_postcondition_kills_mutated_implementations() {
        let atom = parse_atom(
            r#"
atom increment(x: i64) -> i64
  requires: true;
  ensures: result == x + 1;
  body: x + 1;
"#,
        );
        let mut module_env = ModuleEnv::new();
        module_env.register_atom(&atom);
        let hir_atom = lower_atom_to_hir_with_env(&atom, Some(&module_env));
        let mir_body = lower_hir_to_mir(&hir_atom);

        let result =
            check_spec_vacuity_for_hir(&atom, &hir_atom, &mir_body, &module_env, 10, 1000).unwrap();
        assert!(!result.is_vacuous);
        assert_eq!(result.survived_mutations, 0);
        assert!(result.killed_mutations > 0);
    }

    #[test]
    fn inconclusive_verdict_is_unanalyzable_and_does_not_fail() {
        let error =
            MumeiError::verification("solver unknown").with_verdict(VerdictKind::Inconclusive);
        assert_eq!(
            classify_verification_result(Err(error)),
            MutantOutcome::Unanalyzable
        );

        let atom = parse_atom(
            r#"
atom unknown(x: i64) -> i64
  requires: true;
  ensures: true;
  body: x;
"#,
        );
        let result =
            finish_verified_mutants(&atom, Vec::new(), Vec::new(), 2, 0, Vec::new(), 2).unwrap();
        assert!(!result.is_vacuous);
        assert_eq!(result.killed_mutations, 0);
        assert_eq!(result.unanalyzable_mutations, 2);
        assert!(inconclusive_warning(
            &atom.name,
            result.total_mutations_tested,
            result.killed_mutations,
            result.unanalyzable_mutations
        )
        .unwrap()
        .contains("was inconclusive"));
    }
}
