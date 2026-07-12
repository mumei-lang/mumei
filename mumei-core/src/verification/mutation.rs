use crate::mir::{MirBody, MirConstant, MirStatement, Operand, Place, Rvalue, Terminator};
use crate::parser::Op;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MutationOperator {
    BinaryOpFlip(Op),
    ArrayIndexOffset(i64),
    ConditionFlip(Op),
    ConstantZero,
    ConstantOne,
}

#[derive(Debug, Clone)]
pub struct MutationResult {
    pub operator: MutationOperator,
    pub location: String,
    pub mutated_body: MirBody,
    pub verification_passed: bool,
}

pub fn apply_mutation(body: &MirBody, operator: &MutationOperator) -> MirBody {
    let mut mutated = body.clone();
    match operator {
        MutationOperator::BinaryOpFlip(original_op) => {
            mutate_binary_ops(&mut mutated, original_op, false)
        }
        MutationOperator::ArrayIndexOffset(offset) => mutate_array_indices(&mut mutated, *offset),
        MutationOperator::ConditionFlip(original_op) => {
            mutate_binary_ops(&mut mutated, original_op, true)
        }
        MutationOperator::ConstantZero => mutate_constants(&mut mutated, 0),
        MutationOperator::ConstantOne => mutate_constants(&mut mutated, 1),
    }
    mutated
}

fn flip_binary_op(op: &Op) -> Op {
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
    }
}

fn is_condition_op(op: &Op) -> bool {
    matches!(
        op,
        Op::Lt | Op::Le | Op::Gt | Op::Ge | Op::Eq | Op::Neq | Op::And | Op::Or | Op::Implies
    )
}

fn mutate_binary_ops(body: &mut MirBody, original_op: &Op, conditions_only: bool) {
    let flipped = flip_binary_op(original_op);
    for block in &mut body.blocks {
        for stmt in &mut block.statements {
            if let MirStatement::Assign(_, Rvalue::BinaryOp(op, _, _)) = stmt {
                if op == original_op && (!conditions_only || is_condition_op(op)) {
                    *op = flipped.clone();
                }
            }
        }
    }
}

fn collect_index_locals(place: &Place, out: &mut HashSet<usize>) {
    match place {
        Place::Local(_) => {}
        Place::Field(base, _) => collect_index_locals(base, out),
        Place::Index(base, local) => {
            out.insert(local.0);
            collect_index_locals(base, out);
        }
    }
}

fn mutate_array_indices(body: &mut MirBody, offset: i64) {
    if offset == 0 {
        return;
    }

    let mut index_locals = HashSet::new();
    for block in &body.blocks {
        for stmt in &block.statements {
            if let MirStatement::Assign(place, _) = stmt {
                collect_index_locals(place, &mut index_locals);
            }
        }
        match &block.terminator {
            Terminator::Return(Operand::Place(place))
            | Terminator::SwitchInt {
                discr: Operand::Place(place),
                ..
            } => {
                collect_index_locals(place, &mut index_locals);
            }
            _ => {}
        }
    }

    for block in &mut body.blocks {
        for stmt in &mut block.statements {
            if let MirStatement::Assign(Place::Local(local), rvalue) = stmt {
                if index_locals.contains(&local.0) {
                    let original = match rvalue {
                        Rvalue::Use(operand) => operand.clone(),
                        _ => Operand::Place(Place::Local(local.clone())),
                    };
                    let offset_operand = Operand::Constant(MirConstant::Int(offset.abs()));
                    let op = if offset > 0 { Op::Add } else { Op::Sub };
                    *rvalue = Rvalue::BinaryOp(op, original, offset_operand);
                }
            }
        }
    }
}

fn mutate_operand_constant(operand: &mut Operand, value: i64) {
    if let Operand::Constant(MirConstant::Int(current)) = operand {
        if *current != value {
            *operand = Operand::Constant(MirConstant::Int(value));
        }
    }
}

fn mutate_constants(body: &mut MirBody, value: i64) {
    for block in &mut body.blocks {
        for stmt in &mut block.statements {
            if let MirStatement::Assign(_, rvalue) = stmt {
                match rvalue {
                    Rvalue::Use(operand) => mutate_operand_constant(operand, value),
                    Rvalue::BinaryOp(_, lhs, rhs) => {
                        mutate_operand_constant(lhs, value);
                        mutate_operand_constant(rhs, value);
                    }
                    Rvalue::Call { args, .. } | Rvalue::Perform { args, .. } => {
                        for arg in args {
                            mutate_operand_constant(arg, value);
                        }
                    }
                    Rvalue::StructInit { fields, .. } => {
                        for (_, operand) in fields {
                            mutate_operand_constant(operand, value);
                        }
                    }
                    Rvalue::FieldAccess(operand, _) => mutate_operand_constant(operand, value),
                    Rvalue::Ref(_) | Rvalue::RefMut(_) => {}
                }
            }
        }
        match &mut block.terminator {
            Terminator::SwitchInt { discr, .. } | Terminator::Return(discr) => {
                mutate_operand_constant(discr, value);
            }
            Terminator::Goto(_) | Terminator::Unreachable => {}
        }
    }
}

fn has_array_index(body: &MirBody) -> bool {
    let mut indices = HashSet::new();
    for block in &body.blocks {
        for stmt in &block.statements {
            if let MirStatement::Assign(place, _) = stmt {
                collect_index_locals(place, &mut indices);
            }
        }
    }
    !indices.is_empty()
}

fn has_int_constant_other_than(body: &MirBody, value: i64) -> bool {
    fn operand_matches(operand: &Operand, value: i64) -> bool {
        matches!(operand, Operand::Constant(MirConstant::Int(current)) if *current != value)
    }

    for block in &body.blocks {
        for stmt in &block.statements {
            if let MirStatement::Assign(_, rvalue) = stmt {
                let found = match rvalue {
                    Rvalue::Use(operand) => operand_matches(operand, value),
                    Rvalue::BinaryOp(_, lhs, rhs) => {
                        operand_matches(lhs, value) || operand_matches(rhs, value)
                    }
                    Rvalue::Call { args, .. } | Rvalue::Perform { args, .. } => {
                        args.iter().any(|arg| operand_matches(arg, value))
                    }
                    Rvalue::StructInit { fields, .. } => fields
                        .iter()
                        .any(|(_, operand)| operand_matches(operand, value)),
                    Rvalue::FieldAccess(operand, _) => operand_matches(operand, value),
                    Rvalue::Ref(_) | Rvalue::RefMut(_) => false,
                };
                if found {
                    return true;
                }
            }
        }
        match &block.terminator {
            Terminator::SwitchInt { discr, .. } | Terminator::Return(discr) => {
                if operand_matches(discr, value) {
                    return true;
                }
            }
            Terminator::Goto(_) | Terminator::Unreachable => {}
        }
    }
    false
}

fn push_unique(
    mutations: &mut Vec<MutationOperator>,
    mutation: MutationOperator,
    max_mutations: usize,
) -> bool {
    if !mutations.contains(&mutation) {
        mutations.push(mutation);
    }
    mutations.len() >= max_mutations
}

pub fn generate_mutations(body: &MirBody, max_mutations: usize) -> Vec<MutationOperator> {
    if max_mutations == 0 {
        return Vec::new();
    }

    let mut mutations = Vec::new();
    for block in &body.blocks {
        for stmt in &block.statements {
            if let MirStatement::Assign(_, Rvalue::BinaryOp(op, _, _)) = stmt {
                if is_condition_op(op) {
                    if push_unique(
                        &mut mutations,
                        MutationOperator::ConditionFlip(op.clone()),
                        max_mutations,
                    ) {
                        return mutations;
                    }
                } else if push_unique(
                    &mut mutations,
                    MutationOperator::BinaryOpFlip(op.clone()),
                    max_mutations,
                ) {
                    return mutations;
                }
            }
        }
    }

    if has_array_index(body)
        && push_unique(
            &mut mutations,
            MutationOperator::ArrayIndexOffset(1),
            max_mutations,
        )
    {
        return mutations;
    }
    if has_int_constant_other_than(body, 0)
        && push_unique(
            &mut mutations,
            MutationOperator::ConstantZero,
            max_mutations,
        )
    {
        return mutations;
    }
    if has_int_constant_other_than(body, 1) {
        push_unique(&mut mutations, MutationOperator::ConstantOne, max_mutations);
    }
    mutations.truncate(max_mutations);
    mutations
}
