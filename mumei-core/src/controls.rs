use crate::charter;
use crate::hir::{lower_atom_to_hir_with_env, lower_stmt_with_env, HirAtom};
use crate::parser::{Atom, Expr, Item, Op, Stmt, TrustLevel};
use crate::verification::{self, ModuleEnv, MumeiError, VerdictKind, VerificationConfig};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

const SCHEMA: &str = "mumei.controls-report/v1";

#[derive(Debug, Serialize)]
pub struct ControlsReport {
    pub schema: &'static str,
    pub input: String,
    pub status: &'static str,
    pub atoms: Vec<ControlsAtomReport>,
    pub totals: ControlsTotals,
}

#[derive(Debug, Serialize)]
pub struct ControlsAtomReport {
    pub name: String,
    pub status: &'static str,
    pub detail: String,
    pub tested: usize,
    pub killed: usize,
    pub survived: usize,
    pub unanalyzable: usize,
    pub mutants: Vec<MutantReport>,
}

#[derive(Debug, Serialize)]
pub struct MutantReport {
    pub id: String,
    pub operator: String,
    pub site: usize,
    pub outcome: &'static str,
    pub detail: String,
}

#[derive(Debug, Serialize)]
pub struct ControlsTotals {
    pub atoms: usize,
    pub by_status: BTreeMap<String, usize>,
    pub mutants: usize,
    pub killed: usize,
    pub survived: usize,
    pub unanalyzable: usize,
}

impl ControlsReport {
    pub fn json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn human(&self) -> String {
        let mut lines = self
            .atoms
            .iter()
            .map(|atom| {
                let survivors = atom
                    .mutants
                    .iter()
                    .filter(|mutant| mutant.outcome == "survived")
                    .map(|mutant| mutant.id.as_str())
                    .collect::<Vec<_>>();
                let mut line = format!(
                    "{}: {} (killed {}, survived {}, unanalyzable {})",
                    atom.name, atom.status, atom.killed, atom.survived, atom.unanalyzable
                );
                if !survivors.is_empty() {
                    line.push_str(&format!("; surviving mutants: {}", survivors.join(", ")));
                }
                line
            })
            .collect::<Vec<_>>();
        lines.push(format!(
            "Totals: {} atoms, {} mutants ({} killed, {} survived, {} unanalyzable); {}",
            self.totals.atoms,
            self.totals.mutants,
            self.totals.killed,
            self.totals.survived,
            self.totals.unanalyzable,
            self.status
        ));
        lines.join("\n")
    }
}

#[derive(Clone)]
enum MutationKind {
    BinaryOp(Op),
    ConditionFlip(Op),
    ArrayIndex,
    ConstantZero,
    ConstantOne,
}

#[derive(Clone)]
struct SiteMutant {
    id: String,
    operator: String,
    site: usize,
    kind: MutationKind,
}

pub fn analyze(
    input: &str,
    implementation: &[Item],
    generic_implementation: &[Item],
    module_env: &ModuleEnv,
    atom_filter: Option<&str>,
    max_mutants: usize,
) -> Result<ControlsReport, String> {
    let atoms = ordered_atoms(implementation, generic_implementation);
    if let Some(filter) = atom_filter {
        if !atoms.iter().any(|(name, _, _)| name == filter) {
            return Err(format!("unknown atom '{filter}'"));
        }
    }

    let output_dir = tempfile::tempdir().map_err(|error| error.to_string())?;
    let config = VerificationConfig::default();
    let selected = atoms
        .into_iter()
        .filter(|(name, _, _)| atom_filter.is_none_or(|filter| name == filter))
        .collect::<Vec<_>>();
    let mut reports = Vec::with_capacity(selected.len());
    for (name, atom, generic) in selected {
        reports.push(analyze_atom(
            name,
            atom,
            generic,
            module_env,
            output_dir.path(),
            max_mutants,
            &config,
        ));
    }

    let mut by_status = BTreeMap::new();
    let mut mutants = 0;
    let mut killed = 0;
    let mut survived = 0;
    let mut unanalyzable = 0;
    for atom in &reports {
        *by_status.entry(atom.status.to_string()).or_insert(0) += 1;
        mutants += atom.tested;
        killed += atom.killed;
        survived += atom.survived;
        unanalyzable += atom.unanalyzable;
    }
    let status = if reports.iter().any(|atom| atom.status == "weak") {
        "fail"
    } else {
        "pass"
    };
    Ok(ControlsReport {
        schema: SCHEMA,
        input: input.to_string(),
        status,
        totals: ControlsTotals {
            atoms: reports.len(),
            by_status,
            mutants,
            killed,
            survived,
            unanalyzable,
        },
        atoms: reports,
    })
}

fn ordered_atoms<'a>(
    implementation: &'a [Item],
    generic_implementation: &'a [Item],
) -> Vec<(String, &'a Atom, bool)> {
    let prepared = charter::implementation_atoms(implementation)
        .into_iter()
        .map(|candidate| (candidate.name, candidate.atom))
        .collect::<HashMap<_, _>>();
    let generic_names = charter::generic_implementation_atoms(generic_implementation)
        .into_iter()
        .map(|candidate| candidate.name)
        .collect::<HashSet<_>>();
    charter::implementation_atoms(generic_implementation)
        .into_iter()
        .map(|candidate| {
            let generic = generic_names.contains(&candidate.name);
            let atom = if generic {
                candidate.atom
            } else {
                prepared
                    .get(&candidate.name)
                    .copied()
                    .unwrap_or(candidate.atom)
            };
            (candidate.name, atom, generic)
        })
        .collect()
}

fn analyze_atom(
    name: String,
    atom: &Atom,
    generic: bool,
    module_env: &ModuleEnv,
    output_dir: &Path,
    max_mutants: usize,
    config: &VerificationConfig,
) -> ControlsAtomReport {
    if atom.trust_level == TrustLevel::Trusted {
        return not_checked(name, "trusted atom");
    }
    if generic || !atom.type_params.is_empty() {
        return not_checked(name, "generic atom cannot be verified directly");
    }

    let hir = lower_atom_to_hir_with_env(atom, Some(module_env));
    if verify_hir(&hir, output_dir, module_env, config).is_err() {
        return not_checked(name, "original does not verify");
    }
    let mutants = site_mutants(&hir.body_stmt)
        .into_iter()
        .take(max_mutants)
        .collect::<Vec<_>>();
    if mutants.is_empty() {
        return ControlsAtomReport {
            name,
            status: "no_mutants",
            detail: "no supported one-site mutations".to_string(),
            tested: 0,
            killed: 0,
            survived: 0,
            unanalyzable: 0,
            mutants: Vec::new(),
        };
    }

    let mut reports = Vec::with_capacity(mutants.len());
    let mut killed = 0;
    let mut survived = 0;
    let mut unanalyzable = 0;
    for mutant in mutants {
        let mut body_stmt = hir.body_stmt.clone();
        let changed = mutate_at_site(&mut body_stmt, mutant.site, &mutant.kind);
        let (outcome, detail) = if !changed {
            (
                "unanalyzable",
                "mutant site could not be applied".to_string(),
            )
        } else {
            let mut mutated_hir = hir.clone();
            mutated_hir.body_stmt = body_stmt;
            mutated_hir.body = lower_stmt_with_env(&mutated_hir.body_stmt, Some(module_env));
            mutated_hir.atom.body_expr = stmt_source(&mutated_hir.body_stmt);
            match verify_hir(&mutated_hir, output_dir, module_env, config) {
                Ok(()) => ("survived", "mutation still verifies".to_string()),
                Err(error) if error.verdict() == Some(VerdictKind::Rejected) => {
                    ("killed", "confirmed verification rejection".to_string())
                }
                Err(error) => ("unanalyzable", error.to_string()),
            }
        };
        match outcome {
            "killed" => killed += 1,
            "survived" => survived += 1,
            _ => unanalyzable += 1,
        }
        reports.push(MutantReport {
            id: mutant.id,
            operator: mutant.operator,
            site: mutant.site,
            outcome,
            detail,
        });
    }

    let (status, detail) = if survived > 0 {
        (
            "weak",
            format!(
                "surviving mutants: {}",
                reports
                    .iter()
                    .filter(|mutant| mutant.outcome == "survived")
                    .map(|mutant| mutant.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
    } else if killed > 0 {
        (
            "discriminating",
            "at least one mutant was rejected".to_string(),
        )
    } else {
        (
            "inconclusive",
            "all tested mutants were unanalyzable".to_string(),
        )
    };
    ControlsAtomReport {
        name,
        status,
        detail,
        tested: reports.len(),
        killed,
        survived,
        unanalyzable,
        mutants: reports,
    }
}

fn stmt_source(stmt: &Stmt) -> String {
    match stmt {
        Stmt::Expr(expr, _) => crate::verification::support::expr_to_source_string(expr),
        Stmt::Let { var, value, .. } => format!(
            "let {var} = {};",
            crate::verification::support::expr_to_source_string(value)
        ),
        Stmt::Assign { var, value, .. } => format!(
            "{var} = {};",
            crate::verification::support::expr_to_source_string(value)
        ),
        Stmt::ArrayStore {
            array,
            index,
            value,
            ..
        } => format!(
            "{array}[{}] = {};",
            crate::verification::support::expr_to_source_string(index),
            crate::verification::support::expr_to_source_string(value)
        ),
        Stmt::Block(stmts, _) => format!(
            "{{ {} }}",
            stmts.iter().map(stmt_source).collect::<Vec<_>>().join(" ")
        ),
        Stmt::While {
            cond,
            invariant,
            body,
            ..
        } => format!(
            "while ({}) invariant {}; {}",
            crate::verification::support::expr_to_source_string(cond),
            crate::verification::support::expr_to_source_string(invariant),
            stmt_source(body)
        ),
        Stmt::Acquire { resource, body, .. } => {
            format!("acquire {resource} {}", stmt_source(body))
        }
        Stmt::Task { body, .. } => format!("task {}", stmt_source(body)),
        Stmt::TaskGroup { children, .. } => format!(
            "task_group {{ {} }}",
            children
                .iter()
                .map(stmt_source)
                .collect::<Vec<_>>()
                .join(" ")
        ),
        Stmt::Cancel { target, .. } => format!("cancel {target};"),
    }
}

fn not_checked(name: String, detail: &str) -> ControlsAtomReport {
    ControlsAtomReport {
        name,
        status: "not_checked",
        detail: detail.to_string(),
        tested: 0,
        killed: 0,
        survived: 0,
        unanalyzable: 0,
        mutants: Vec::new(),
    }
}

fn verify_hir(
    hir: &HirAtom,
    output_dir: &Path,
    module_env: &ModuleEnv,
    config: &VerificationConfig,
) -> Result<(), MumeiError> {
    verification::verify_with_config(
        hir,
        output_dir,
        module_env,
        config.timeout_ms,
        config.global_max_unroll,
    )
    .map(|_| ())
}

fn site_mutants(body: &Stmt) -> Vec<SiteMutant> {
    let mut mutants = Vec::new();
    let mut site = 0;
    collect_stmt(body, &mut site, &mut mutants);
    mutants
}

fn collect_stmt(stmt: &Stmt, site: &mut usize, mutants: &mut Vec<SiteMutant>) {
    match stmt {
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => collect_expr(value, site, mutants),
        Stmt::ArrayStore { index, value, .. } => {
            collect_expr(index, site, mutants);
            collect_expr(value, site, mutants);
        }
        Stmt::Block(stmts, _) => {
            for stmt in stmts {
                collect_stmt(stmt, site, mutants);
            }
        }
        Stmt::While { cond, body, .. } => {
            collect_expr(cond, site, mutants);
            collect_stmt(body, site, mutants);
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => collect_stmt(body, site, mutants),
        Stmt::TaskGroup { children, .. } => {
            for child in children {
                collect_stmt(child, site, mutants);
            }
        }
        Stmt::Cancel { .. } => {}
        Stmt::Expr(expr, _) => collect_expr(expr, site, mutants),
    }
}

fn collect_expr(expr: &Expr, site: &mut usize, mutants: &mut Vec<SiteMutant>) {
    match expr {
        Expr::Number(value) => {
            let current = *site;
            *site += 1;
            if *value != 0 {
                mutants.push(SiteMutant {
                    id: format!("constant:{value}->0@{current}"),
                    operator: "constant-to-zero".to_string(),
                    site: current,
                    kind: MutationKind::ConstantZero,
                });
            }
            if *value != 1 {
                mutants.push(SiteMutant {
                    id: format!("constant:{value}->1@{current}"),
                    operator: "constant-to-one".to_string(),
                    site: current,
                    kind: MutationKind::ConstantOne,
                });
            }
        }
        Expr::ArrayAccess(_, index) => {
            let current = *site;
            *site += 1;
            mutants.push(SiteMutant {
                id: format!("array-index:+1@{current}"),
                operator: "array-index-plus-one".to_string(),
                site: current,
                kind: MutationKind::ArrayIndex,
            });
            collect_expr(index, site, mutants);
        }
        Expr::BinaryOp(left, op, right) => {
            let current = *site;
            *site += 1;
            let flipped = crate::verification::vacuity::flip_binary_op(op);
            if flipped != *op {
                mutants.push(SiteMutant {
                    id: format!("op-flip:{op:?}->{flipped:?}@{current}"),
                    operator: "binary-op-flip".to_string(),
                    site: current,
                    kind: MutationKind::BinaryOp(op.clone()),
                });
            }
            if is_condition_op(op) && flipped != *op {
                mutants.push(SiteMutant {
                    id: format!("condition-flip:{op:?}->{flipped:?}@{current}"),
                    operator: "condition-flip".to_string(),
                    site: current,
                    kind: MutationKind::ConditionFlip(op.clone()),
                });
            }
            collect_expr(left, site, mutants);
            collect_expr(right, site, mutants);
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_expr(cond, site, mutants);
            collect_stmt(then_branch, site, mutants);
            collect_stmt(else_branch, site, mutants);
        }
        Expr::Block(stmt) => collect_stmt(stmt, site, mutants),
        Expr::Call(_, args) | Expr::Perform { args, .. } => {
            for arg in args {
                collect_expr(arg, site, mutants);
            }
        }
        Expr::CallRef { callee, args } => {
            collect_expr(callee, site, mutants);
            for arg in args {
                collect_expr(arg, site, mutants);
            }
        }
        Expr::ArrayLit(elements) => {
            for element in elements {
                collect_expr(element, site, mutants);
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, field) in fields {
                collect_expr(field, site, mutants);
            }
        }
        Expr::FieldAccess(base, _) | Expr::Await { expr: base } => {
            collect_expr(base, site, mutants)
        }
        Expr::Match { target, arms } => {
            collect_expr(target, site, mutants);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_expr(guard, site, mutants);
                }
                collect_stmt(&arm.body, site, mutants);
            }
        }
        Expr::Async { body } | Expr::Lambda { body, .. } => collect_stmt(body, site, mutants),
        Expr::ChanSend { channel, value } => {
            collect_expr(channel, site, mutants);
            collect_expr(value, site, mutants);
        }
        Expr::ChanRecv { channel } => collect_expr(channel, site, mutants),
        Expr::AtomRef { .. } | Expr::Float(_) | Expr::StringLit(_) | Expr::Variable(_) => {}
    }
}

fn is_condition_op(op: &Op) -> bool {
    matches!(
        op,
        Op::Lt | Op::Le | Op::Gt | Op::Ge | Op::Eq | Op::Neq | Op::And | Op::Or | Op::Implies
    )
}

fn mutate_at_site(stmt: &mut Stmt, target: usize, kind: &MutationKind) -> bool {
    let mut site = 0;
    let mut changed = false;
    mutate_stmt(stmt, target, kind, &mut site, &mut changed);
    changed
}

fn mutate_stmt(
    stmt: &mut Stmt,
    target: usize,
    kind: &MutationKind,
    site: &mut usize,
    changed: &mut bool,
) {
    if *changed {
        return;
    }
    match stmt {
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            mutate_expr(value, target, kind, site, changed)
        }
        Stmt::ArrayStore { index, value, .. } => {
            mutate_expr(index, target, kind, site, changed);
            mutate_expr(value, target, kind, site, changed);
        }
        Stmt::Block(stmts, _) => {
            for stmt in stmts {
                mutate_stmt(stmt, target, kind, site, changed);
            }
        }
        Stmt::While { cond, body, .. } => {
            mutate_expr(cond, target, kind, site, changed);
            mutate_stmt(body, target, kind, site, changed);
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
            mutate_stmt(body, target, kind, site, changed)
        }
        Stmt::TaskGroup { children, .. } => {
            for child in children {
                mutate_stmt(child, target, kind, site, changed);
            }
        }
        Stmt::Cancel { .. } => {}
        Stmt::Expr(expr, _) => mutate_expr(expr, target, kind, site, changed),
    }
}

fn mutate_expr(
    expr: &mut Expr,
    target: usize,
    kind: &MutationKind,
    site: &mut usize,
    changed: &mut bool,
) {
    if *changed {
        return;
    }
    match expr {
        Expr::Number(value) => {
            let current = *site;
            *site += 1;
            if current == target {
                match kind {
                    MutationKind::ConstantZero if *value != 0 => {
                        *value = 0;
                        *changed = true;
                    }
                    MutationKind::ConstantOne if *value != 1 => {
                        *value = 1;
                        *changed = true;
                    }
                    _ => {}
                }
            }
        }
        Expr::ArrayAccess(_, index) => {
            let current = *site;
            *site += 1;
            if current == target && matches!(kind, MutationKind::ArrayIndex) {
                let original = (**index).clone();
                **index = Expr::BinaryOp(Box::new(original), Op::Add, Box::new(Expr::Number(1)));
                *changed = true;
            } else {
                mutate_expr(index, target, kind, site, changed);
            }
        }
        Expr::BinaryOp(left, op, right) => {
            let current = *site;
            *site += 1;
            if current == target {
                match kind {
                    MutationKind::BinaryOp(original) if *op == *original => {
                        *op = crate::verification::vacuity::flip_binary_op(op);
                        *changed = true;
                    }
                    MutationKind::ConditionFlip(original)
                        if *op == *original && is_condition_op(op) =>
                    {
                        *op = crate::verification::vacuity::flip_binary_op(op);
                        *changed = true;
                    }
                    _ => {}
                }
            }
            mutate_expr(left, target, kind, site, changed);
            mutate_expr(right, target, kind, site, changed);
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            mutate_expr(cond, target, kind, site, changed);
            mutate_stmt(then_branch, target, kind, site, changed);
            mutate_stmt(else_branch, target, kind, site, changed);
        }
        Expr::Block(stmt) => mutate_stmt(stmt, target, kind, site, changed),
        Expr::Call(_, args) | Expr::Perform { args, .. } => {
            for arg in args {
                mutate_expr(arg, target, kind, site, changed);
            }
        }
        Expr::CallRef { callee, args } => {
            mutate_expr(callee, target, kind, site, changed);
            for arg in args {
                mutate_expr(arg, target, kind, site, changed);
            }
        }
        Expr::ArrayLit(elements) => {
            for element in elements {
                mutate_expr(element, target, kind, site, changed);
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, field) in fields {
                mutate_expr(field, target, kind, site, changed);
            }
        }
        Expr::FieldAccess(base, _) | Expr::Await { expr: base } => {
            mutate_expr(base, target, kind, site, changed)
        }
        Expr::Match {
            target: value,
            arms,
        } => {
            mutate_expr(value, target, kind, site, changed);
            for arm in arms {
                if let Some(guard) = &mut arm.guard {
                    mutate_expr(guard, target, kind, site, changed);
                }
                mutate_stmt(&mut arm.body, target, kind, site, changed);
            }
        }
        Expr::Async { body } | Expr::Lambda { body, .. } => {
            mutate_stmt(body, target, kind, site, changed)
        }
        Expr::ChanSend { channel, value } => {
            mutate_expr(channel, target, kind, site, changed);
            mutate_expr(value, target, kind, site, changed);
        }
        Expr::ChanRecv { channel } => mutate_expr(channel, target, kind, site, changed),
        Expr::AtomRef { .. } | Expr::Float(_) | Expr::StringLit(_) | Expr::Variable(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    #[test]
    fn skips_noop_pow_and_target_constants() {
        let items = parser::parse_module(
            "atom f(x: i64) -> i64\nrequires: true;\nensures: true;\nbody: x ** 0;\n",
        );
        let Item::Atom(atom) = &items[0] else {
            panic!("expected atom")
        };
        let mutants = site_mutants(&parser::parse_body_expr(&atom.body_expr));
        assert!(mutants
            .iter()
            .all(|mutant| !mutant.id.starts_with("op-flip:Pow")));
        assert!(mutants
            .iter()
            .all(|mutant| { !mutant.id.starts_with("constant:0->0") }));
    }

    #[test]
    fn per_site_ids_and_mutation_are_stable() {
        let stmt = parser::parse_body_expr("x + (y + 1)");
        let mutants = site_mutants(&stmt);
        let adds = mutants
            .iter()
            .filter(|mutant| mutant.id.starts_with("op-flip:Add"))
            .collect::<Vec<_>>();
        assert_eq!(adds.len(), 2);
        assert_ne!(adds[0].site, adds[1].site);
        let mut changed = stmt.clone();
        assert!(mutate_at_site(&mut changed, adds[0].site, &adds[0].kind));
        let mut changed_twice = stmt;
        assert!(mutate_at_site(
            &mut changed_twice,
            adds[1].site,
            &adds[1].kind
        ));
        assert_ne!(format!("{changed:?}"), format!("{changed_twice:?}"));
    }

    #[test]
    fn classifier_counts_only_structured_rejections() {
        let rejected = MumeiError::verification("rejected").with_verdict(VerdictKind::Rejected);
        let inconclusive =
            MumeiError::verification("unknown").with_verdict(VerdictKind::Inconclusive);
        assert_eq!(rejected.verdict(), Some(VerdictKind::Rejected));
        assert_eq!(inconclusive.verdict(), Some(VerdictKind::Inconclusive));
        assert_eq!(MumeiError::verification("unmarked").verdict(), None);
    }
}
