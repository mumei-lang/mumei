use crate::hir::{HirAtom, HirExpr, HirStmt};
use crate::mir::MirBody;
use crate::parser::{Atom, Expr, Op, Stmt};
use crate::verification::module_env::{LinearityCtx, ModuleEnv};
use crate::verification::mutation::{
    apply_mutation, generate_mutations, MutationOperator, MutationResult,
};
use crate::verification::support::EffectCtx;
use crate::verification::translator::{
    apply_refinement_constraint, expr_to_z3, param_z3_value, stmt_to_z3, VCtx,
    DEFAULT_CONSTRAINT_BUDGET,
};
use crate::verification::types::Env;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use z3::{Config, Context, SatResult, Solver};

#[derive(Debug, Clone)]
pub struct VacuityCheckResult {
    pub is_vacuous: bool,
    pub mutation_results: Vec<MutationResult>,
    pub vacuous_mutations: Vec<MutationOperator>,
    pub total_mutations_tested: usize,
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
    let mutations = generate_mutations(mir_body, max_mutations);
    let mut mutation_results = Vec::new();
    let mut vacuous_mutations = Vec::new();

    for mutation in &mutations {
        let mut mutated_stmt = hir_atom.body_stmt.clone();
        let stmt_changed = mutate_stmt(&mut mutated_stmt, mutation);
        let mut mutated_hir_body = hir_atom.body.clone();
        let hir_changed = mutate_hir_stmt(&mut mutated_hir_body, mutation);

        let verification_passed = if stmt_changed || hir_changed {
            verify_mutated_body(atom, &mutated_stmt, module_env, timeout_ms)
        } else {
            false
        };

        if verification_passed {
            vacuous_mutations.push(mutation.clone());
        }
        mutation_results.push(MutationResult {
            operator: mutation.clone(),
            location: format!("mir_body_{}", atom.name),
            mutated_body: apply_mutation(mir_body, mutation),
            verification_passed,
        });
    }

    finish_vacuity_check(atom, mutations.len(), mutation_results, vacuous_mutations)
}

fn finish_vacuity_check(
    atom: &Atom,
    total_mutations_tested: usize,
    mutation_results: Vec<MutationResult>,
    vacuous_mutations: Vec<MutationOperator>,
) -> Result<VacuityCheckResult, VacuityError> {
    if vacuous_mutations.is_empty() {
        Ok(VacuityCheckResult {
            is_vacuous: false,
            mutation_results,
            vacuous_mutations,
            total_mutations_tested,
        })
    } else {
        Err(VacuityError {
            atom_name: atom.name.clone(),
            reason: format!(
                "Specification is vacuous: {} out of {} mutated implementations still passed verification.",
                vacuous_mutations.len(),
                total_mutations_tested
            ),
            vacuous_mutations,
        })
    }
}

fn verify_mutated_body(
    atom: &Atom,
    mutated_stmt: &Stmt,
    module_env: &ModuleEnv,
    timeout_ms: u64,
) -> bool {
    let mut cfg = Config::new();
    cfg.set_timeout_msec(timeout_ms);
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);
    let linearity_ctx_cell = std::cell::RefCell::new(LinearityCtx::new());
    let allowed_effects = module_env.resolve_effect_set_from_effects(&atom.effects);
    let effect_ctx_cell = std::cell::RefCell::new(EffectCtx::new(allowed_effects));
    let constraint_count_cell = std::cell::Cell::new(0usize);
    let vc = VCtx {
        ctx: &ctx,
        module_env,
        current_atom: Some(atom),
        linearity_ctx: Some(&linearity_ctx_cell),
        effect_ctx: Some(&effect_ctx_cell),
        constraint_count: Some(&constraint_count_cell),
        constraint_budget: DEFAULT_CONSTRAINT_BUDGET,
        has_string_constraints: None,
        path_cond_stack: std::cell::RefCell::new(Vec::new()),
        profiler: None,
    };
    let mut env: Env = HashMap::new();

    for param in &atom.params {
        env.insert(
            param.name.clone(),
            param_z3_value(&ctx, &param.name, param.type_name.as_deref(), module_env),
        );
    }

    for param in &atom.params {
        if let Some(type_name) = &param.type_name {
            if let Some(refined) = module_env.get_type(type_name) {
                if apply_refinement_constraint(&vc, &solver, &param.name, refined, &mut env)
                    .is_err()
                {
                    return false;
                }
            }
        }
    }

    if atom.requires.trim() != "true" {
        let req_ast = crate::parser::parse_expression(&atom.requires);
        let Ok(req_z3) = expr_to_z3(&vc, &req_ast, &mut env, None) else {
            return false;
        };
        let Some(req_bool) = req_z3.as_bool() else {
            return false;
        };
        solver.assert(&req_bool);
    }

    let Ok(body_result) = stmt_to_z3(&vc, mutated_stmt, &mut env, Some(&solver)) else {
        return false;
    };

    if atom.ensures.trim() == "true" {
        return true;
    }

    env.insert("result".to_string(), body_result);
    let ens_ast = crate::parser::parse_expression(&atom.ensures);
    let Ok(ens_z3) = expr_to_z3(&vc, &ens_ast, &mut env, None) else {
        return false;
    };
    let Some(ens_bool) = ens_z3.as_bool() else {
        return false;
    };
    solver.push();
    solver.assert(&ens_bool.not());
    let passed = solver.check() == SatResult::Unsat;
    solver.pop(1);
    passed
}

fn flip_binary_op(op: &Op) -> Op {
    match op {
        Op::Add => Op::Sub,
        Op::Sub => Op::Add,
        Op::Mul => Op::Div,
        Op::Div => Op::Mul,
        Op::Lt => Op::Ge,
        Op::Le => Op::Gt,
        Op::Gt => Op::Le,
        Op::Ge => Op::Lt,
        Op::Eq => Op::Neq,
        Op::Neq => Op::Eq,
        Op::And => Op::Or,
        Op::Or => Op::And,
        Op::Implies => Op::And,
    }
}

fn is_condition_op(op: &Op) -> bool {
    matches!(
        op,
        Op::Lt | Op::Le | Op::Gt | Op::Ge | Op::Eq | Op::Neq | Op::And | Op::Or | Op::Implies
    )
}

fn mutate_stmt(stmt: &mut Stmt, mutation: &MutationOperator) -> bool {
    match stmt {
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => mutate_expr(value, mutation),
        Stmt::ArrayStore { index, value, .. } => {
            let index_changed = mutate_array_index_expr(index, mutation);
            let value_changed = mutate_expr(value, mutation);
            index_changed || value_changed
        }
        Stmt::Block(stmts, _)
        | Stmt::TaskGroup {
            children: stmts, ..
        } => {
            let mut changed = false;
            for stmt in stmts {
                changed |= mutate_stmt(stmt, mutation);
            }
            changed
        }
        Stmt::While { cond, body, .. } => {
            let cond_changed = mutate_condition_expr(cond, mutation);
            let body_changed = mutate_stmt(body, mutation);
            cond_changed || body_changed
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => mutate_stmt(body, mutation),
        Stmt::Cancel { .. } => false,
        Stmt::Expr(expr, _) => mutate_expr(expr, mutation),
    }
}

fn mutate_expr(expr: &mut Expr, mutation: &MutationOperator) -> bool {
    match mutation {
        MutationOperator::BinaryOpFlip(original_op) => mutate_binary_expr(expr, original_op, false),
        MutationOperator::ConditionFlip(original_op) => mutate_binary_expr(expr, original_op, true),
        MutationOperator::ArrayIndexOffset(offset) => mutate_array_access_expr(expr, *offset),
        MutationOperator::ConstantZero => mutate_constant_expr(expr, 0),
        MutationOperator::ConstantOne => mutate_constant_expr(expr, 1),
    }
}

fn mutate_condition_expr(expr: &mut Expr, mutation: &MutationOperator) -> bool {
    match mutation {
        MutationOperator::ConditionFlip(original_op) => mutate_binary_expr(expr, original_op, true),
        _ => mutate_expr(expr, mutation),
    }
}

fn mutate_binary_expr(expr: &mut Expr, original_op: &Op, conditions_only: bool) -> bool {
    match expr {
        Expr::BinaryOp(lhs, op, rhs) => {
            let mut changed = mutate_binary_expr(lhs, original_op, conditions_only)
                | mutate_binary_expr(rhs, original_op, conditions_only);
            if op == original_op && (!conditions_only || is_condition_op(op)) {
                *op = flip_binary_op(op);
                changed = true;
            }
            changed
        }
        Expr::ArrayAccess(_, idx) | Expr::FieldAccess(idx, _) | Expr::Await { expr: idx } => {
            mutate_binary_expr(idx, original_op, conditions_only)
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            let mutation = if conditions_only {
                MutationOperator::ConditionFlip(original_op.clone())
            } else {
                MutationOperator::BinaryOpFlip(original_op.clone())
            };
            mutate_binary_expr(cond, original_op, conditions_only)
                | mutate_stmt(then_branch, &mutation)
                | mutate_stmt(else_branch, &mutation)
        }
        Expr::Call(_, args) | Expr::CallRef { args, .. } | Expr::Perform { args, .. } => args
            .iter_mut()
            .any(|arg| mutate_binary_expr(arg, original_op, conditions_only)),
        Expr::StructInit { fields, .. } => fields
            .iter_mut()
            .any(|(_, expr)| mutate_binary_expr(expr, original_op, conditions_only)),
        Expr::Match { target, arms } => {
            let mut changed = mutate_binary_expr(target, original_op, conditions_only);
            for arm in arms {
                if let Some(guard) = &mut arm.guard {
                    changed |= mutate_binary_expr(guard, original_op, conditions_only);
                }
                let mutation = if conditions_only {
                    MutationOperator::ConditionFlip(original_op.clone())
                } else {
                    MutationOperator::BinaryOpFlip(original_op.clone())
                };
                changed |= mutate_stmt(&mut arm.body, &mutation);
            }
            changed
        }
        Expr::Async { body } | Expr::Lambda { body, .. } => {
            let mutation = if conditions_only {
                MutationOperator::ConditionFlip(original_op.clone())
            } else {
                MutationOperator::BinaryOpFlip(original_op.clone())
            };
            mutate_stmt(body, &mutation)
        }
        Expr::AtomRef { .. }
        | Expr::ChanRecv { .. }
        | Expr::ChanSend { .. }
        | Expr::Float(_)
        | Expr::Number(_)
        | Expr::StringLit(_)
        | Expr::Variable(_) => false,
    }
}

fn mutate_array_index_expr(expr: &mut Expr, mutation: &MutationOperator) -> bool {
    if let MutationOperator::ArrayIndexOffset(offset) = mutation {
        mutate_array_access_expr(expr, *offset)
    } else {
        mutate_expr(expr, mutation)
    }
}

fn offset_expr(index: Expr, offset: i64) -> Expr {
    if offset >= 0 {
        Expr::BinaryOp(Box::new(index), Op::Add, Box::new(Expr::Number(offset)))
    } else {
        Expr::BinaryOp(
            Box::new(index),
            Op::Sub,
            Box::new(Expr::Number(offset.abs())),
        )
    }
}

fn mutate_array_access_expr(expr: &mut Expr, offset: i64) -> bool {
    if offset == 0 {
        return false;
    }
    match expr {
        Expr::ArrayAccess(_, idx) => {
            let original = (**idx).clone();
            **idx = offset_expr(original, offset);
            true
        }
        Expr::BinaryOp(lhs, _, rhs) => {
            mutate_array_access_expr(lhs, offset) | mutate_array_access_expr(rhs, offset)
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            mutate_array_access_expr(cond, offset)
                | mutate_stmt(then_branch, &MutationOperator::ArrayIndexOffset(offset))
                | mutate_stmt(else_branch, &MutationOperator::ArrayIndexOffset(offset))
        }
        Expr::Call(_, args) | Expr::CallRef { args, .. } | Expr::Perform { args, .. } => args
            .iter_mut()
            .any(|arg| mutate_array_access_expr(arg, offset)),
        Expr::StructInit { fields, .. } => fields
            .iter_mut()
            .any(|(_, expr)| mutate_array_access_expr(expr, offset)),
        Expr::FieldAccess(base, _) | Expr::Await { expr: base } => {
            mutate_array_access_expr(base, offset)
        }
        Expr::Match { target, arms } => {
            let mut changed = mutate_array_access_expr(target, offset);
            for arm in arms {
                if let Some(guard) = &mut arm.guard {
                    changed |= mutate_array_access_expr(guard, offset);
                }
                changed |= mutate_stmt(&mut arm.body, &MutationOperator::ArrayIndexOffset(offset));
            }
            changed
        }
        Expr::Async { body } | Expr::Lambda { body, .. } => {
            mutate_stmt(body, &MutationOperator::ArrayIndexOffset(offset))
        }
        Expr::AtomRef { .. }
        | Expr::ChanRecv { .. }
        | Expr::ChanSend { .. }
        | Expr::Float(_)
        | Expr::Number(_)
        | Expr::StringLit(_)
        | Expr::Variable(_) => false,
    }
}

fn mutate_constant_expr(expr: &mut Expr, value: i64) -> bool {
    match expr {
        Expr::Number(current) => {
            if *current != value {
                *current = value;
                true
            } else {
                false
            }
        }
        Expr::ArrayAccess(_, idx) | Expr::FieldAccess(idx, _) | Expr::Await { expr: idx } => {
            mutate_constant_expr(idx, value)
        }
        Expr::BinaryOp(lhs, _, rhs) => {
            mutate_constant_expr(lhs, value) | mutate_constant_expr(rhs, value)
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            let mutation = constant_mutation(value);
            mutate_constant_expr(cond, value)
                | mutate_stmt(then_branch, &mutation)
                | mutate_stmt(else_branch, &mutation)
        }
        Expr::Call(_, args) | Expr::CallRef { args, .. } | Expr::Perform { args, .. } => {
            args.iter_mut().any(|arg| mutate_constant_expr(arg, value))
        }
        Expr::StructInit { fields, .. } => fields
            .iter_mut()
            .any(|(_, expr)| mutate_constant_expr(expr, value)),
        Expr::Match { target, arms } => {
            let mut changed = mutate_constant_expr(target, value);
            for arm in arms {
                if let Some(guard) = &mut arm.guard {
                    changed |= mutate_constant_expr(guard, value);
                }
                let mutation = constant_mutation(value);
                changed |= mutate_stmt(&mut arm.body, &mutation);
            }
            changed
        }
        Expr::Async { body } | Expr::Lambda { body, .. } => {
            let mutation = constant_mutation(value);
            mutate_stmt(body, &mutation)
        }
        Expr::AtomRef { .. }
        | Expr::ChanRecv { .. }
        | Expr::ChanSend { .. }
        | Expr::Float(_)
        | Expr::StringLit(_)
        | Expr::Variable(_) => false,
    }
}

fn constant_mutation(value: i64) -> MutationOperator {
    if value == 0 {
        MutationOperator::ConstantZero
    } else {
        MutationOperator::ConstantOne
    }
}

fn mutate_hir_stmt(stmt: &mut HirStmt, mutation: &MutationOperator) -> bool {
    match stmt {
        HirStmt::Let { value, .. } | HirStmt::Assign { value, .. } => {
            mutate_hir_expr(value, mutation)
        }
        HirStmt::ArrayStore { index, value, .. } => {
            mutate_hir_expr(index, mutation) | mutate_hir_expr(value, mutation)
        }
        HirStmt::While { cond, body, .. } => {
            mutate_hir_expr(cond, mutation) | mutate_hir_stmt(body, mutation)
        }
        HirStmt::Block { stmts, tail_expr } => {
            let mut changed = false;
            for stmt in stmts {
                changed |= mutate_hir_stmt(stmt, mutation);
            }
            if let Some(tail) = tail_expr {
                changed |= mutate_hir_expr(tail, mutation);
            }
            changed
        }
        HirStmt::Acquire { body, .. } => mutate_hir_stmt(body, mutation),
        HirStmt::Expr(expr) => mutate_hir_expr(expr, mutation),
    }
}

fn mutate_hir_expr(expr: &mut HirExpr, mutation: &MutationOperator) -> bool {
    match expr {
        HirExpr::Number(current) => match mutation {
            MutationOperator::ConstantZero if *current != 0 => {
                *current = 0;
                true
            }
            MutationOperator::ConstantOne if *current != 1 => {
                *current = 1;
                true
            }
            _ => false,
        },
        HirExpr::ArrayAccess(_, idx) => {
            let changed = mutate_hir_expr(idx, mutation);
            if let MutationOperator::ArrayIndexOffset(offset) = mutation {
                let original = (**idx).clone();
                **idx = if *offset >= 0 {
                    HirExpr::BinaryOp(
                        Box::new(original),
                        Op::Add,
                        Box::new(HirExpr::Number(*offset)),
                    )
                } else {
                    HirExpr::BinaryOp(
                        Box::new(original),
                        Op::Sub,
                        Box::new(HirExpr::Number(offset.abs())),
                    )
                };
                true
            } else {
                changed
            }
        }
        HirExpr::BinaryOp(lhs, op, rhs) => {
            let mut changed = mutate_hir_expr(lhs, mutation) | mutate_hir_expr(rhs, mutation);
            match mutation {
                MutationOperator::BinaryOpFlip(original_op)
                | MutationOperator::ConditionFlip(original_op)
                    if op == original_op =>
                {
                    *op = flip_binary_op(op);
                    changed = true;
                }
                _ => {}
            }
            changed
        }
        HirExpr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            mutate_hir_expr(cond, mutation)
                | mutate_hir_stmt(then_branch, mutation)
                | mutate_hir_stmt(else_branch, mutation)
        }
        HirExpr::Call { args, .. }
        | HirExpr::CallRef { args, .. }
        | HirExpr::Perform { args, .. } => {
            args.iter_mut().any(|arg| mutate_hir_expr(arg, mutation))
        }
        HirExpr::StructInit { fields, .. } => fields
            .iter_mut()
            .any(|(_, expr)| mutate_hir_expr(expr, mutation)),
        HirExpr::FieldAccess(base, _) | HirExpr::Await { expr: base } => {
            mutate_hir_expr(base, mutation)
        }
        HirExpr::Match { target, arms } => {
            let mut changed = mutate_hir_expr(target, mutation);
            for arm in arms {
                if let Some(guard) = &mut arm.guard {
                    changed |= mutate_hir_expr(guard, mutation);
                }
                changed |= mutate_hir_stmt(&mut arm.body, mutation);
            }
            changed
        }
        HirExpr::Async { body } | HirExpr::Task { body, .. } | HirExpr::Lambda { body, .. } => {
            mutate_hir_stmt(body, mutation)
        }
        HirExpr::TaskGroup { children, .. } => children
            .iter_mut()
            .any(|child| mutate_hir_stmt(child, mutation)),
        HirExpr::ChanSend { channel, value } => {
            mutate_hir_expr(channel, mutation) | mutate_hir_expr(value, mutation)
        }
        HirExpr::ChanRecv { channel } => mutate_hir_expr(channel, mutation),
        HirExpr::VariantInit { fields, .. } => fields
            .iter_mut()
            .any(|field| mutate_hir_expr(field, mutation)),
        HirExpr::AtomRef { .. }
        | HirExpr::Float(_)
        | HirExpr::StringLit(_)
        | HirExpr::Variable(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir::lower_atom_to_hir_with_env;
    use crate::mir::lower_hir_to_mir;
    use crate::parser::parse_atom;

    #[test]
    fn trivial_contract_is_rejected_when_mutation_still_verifies() {
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
        assert!(!err.vacuous_mutations.is_empty());
    }

    #[test]
    fn meaningful_postcondition_rejects_mutated_implementations() {
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
        assert_eq!(result.vacuous_mutations.len(), 0);
    }
}
