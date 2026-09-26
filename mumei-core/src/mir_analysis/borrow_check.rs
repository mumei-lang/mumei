use super::MoveAnalysisResult;
use crate::mir::{
    BasicBlockId, Local, LocalDecl, MirBody, MirParamMode, MirStatement, Movability, Operand,
    Place, Rvalue,
};
use crate::parser::{Expr, Stmt};
use crate::verification::ModuleEnv;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorrowViolationKind {
    MoveWhileBorrowed,
    WriteWhileShared,
    MutableAliasing,
    BorrowOfMoved,
    WriteThroughShared,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BorrowViolation {
    pub kind: BorrowViolationKind,
    pub place: String,
    pub borrower: String,
    pub block: BasicBlockId,
    pub statement: usize,
    pub message: String,
}

fn atom_has_borrowing_params(atom: &crate::parser::Atom) -> bool {
    atom.params
        .iter()
        .any(|param| param.is_ref || param.is_ref_mut)
        || atom
            .consumed_params
            .iter()
            .any(|name| atom.params.iter().any(|param| &param.name == name))
}

fn known_atom<'a>(module_env: &'a ModuleEnv, name: &str) -> Option<&'a crate::parser::Atom> {
    module_env
        .get_atom(name)
        .or_else(|| module_env.get_atom(&name.replace('.', "::")))
}

/// Find a first-class `atom_ref` value whose target has borrowing or consuming
/// parameters. Direct `CallRef` callees are intentionally exempt because they
/// are lowered into the same MIR call representation as direct calls.
pub fn find_dynamic_borrowing_atom_ref(stmt: &Stmt, module_env: &ModuleEnv) -> Option<String> {
    fn walk_expr(node: &Expr, module_env: &ModuleEnv) -> Option<String> {
        match node {
            Expr::AtomRef { name } => known_atom(module_env, name)
                .filter(|atom| atom_has_borrowing_params(atom))
                .map(|_| name.clone()),
            Expr::CallRef { callee, args } => {
                let found = if matches!(callee.as_ref(), Expr::AtomRef { .. }) {
                    None
                } else {
                    walk_expr(callee, module_env)
                };
                found.or_else(|| args.iter().find_map(|arg| walk_expr(arg, module_env)))
            }
            Expr::ArrayLit(elements) => elements
                .iter()
                .find_map(|element| walk_expr(element, module_env)),
            Expr::ArrayAccess(_, index) => walk_expr(index, module_env),
            Expr::BinaryOp(lhs, _, rhs) => {
                walk_expr(lhs, module_env).or_else(|| walk_expr(rhs, module_env))
            }
            Expr::IfThenElse {
                cond,
                then_branch,
                else_branch,
            } => walk_expr(cond, module_env)
                .or_else(|| walk_stmt(then_branch, module_env))
                .or_else(|| walk_stmt(else_branch, module_env)),
            Expr::Call(_, args) | Expr::Perform { args, .. } => {
                args.iter().find_map(|arg| walk_expr(arg, module_env))
            }
            Expr::StructInit { fields, .. } => fields
                .iter()
                .find_map(|(_, value)| walk_expr(value, module_env)),
            Expr::FieldAccess(base, _) => walk_expr(base, module_env),
            Expr::Match { target, arms } => walk_expr(target, module_env).or_else(|| {
                arms.iter().find_map(|arm| {
                    arm.guard
                        .as_deref()
                        .and_then(|guard| walk_expr(guard, module_env))
                        .or_else(|| walk_stmt(&arm.body, module_env))
                })
            }),
            Expr::Async { body } | Expr::Lambda { body, .. } => walk_stmt(body, module_env),
            Expr::Await { expr: inner } => walk_expr(inner, module_env),
            Expr::ChanSend { channel, value } => {
                walk_expr(channel, module_env).or_else(|| walk_expr(value, module_env))
            }
            Expr::ChanRecv { channel } => walk_expr(channel, module_env),
            Expr::Number(_) | Expr::Float(_) | Expr::StringLit(_) | Expr::Variable(_) => None,
        }
    }

    fn walk_stmt(node: &Stmt, module_env: &ModuleEnv) -> Option<String> {
        match node {
            Stmt::Let { value, .. } | Stmt::Assign { value, .. } => walk_expr(value, module_env),
            Stmt::ArrayStore { index, value, .. } => {
                walk_expr(index, module_env).or_else(|| walk_expr(value, module_env))
            }
            Stmt::Block(stmts, _)
            | Stmt::TaskGroup {
                children: stmts, ..
            } => stmts.iter().find_map(|child| walk_stmt(child, module_env)),
            Stmt::While {
                cond,
                invariant,
                decreases,
                body,
                ..
            } => walk_expr(cond, module_env)
                .or_else(|| walk_expr(invariant, module_env))
                .or_else(|| decreases.as_deref().and_then(|d| walk_expr(d, module_env)))
                .or_else(|| walk_stmt(body, module_env)),
            Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => walk_stmt(body, module_env),
            Stmt::Expr(expr, _) => walk_expr(expr, module_env),
            Stmt::Cancel { .. } => None,
        }
    }

    walk_stmt(stmt, module_env)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoanKind {
    Shared,
    Mut,
}

#[derive(Debug, Clone, PartialEq)]
struct Loan {
    holder: Local,
    place: Place,
    kind: LoanKind,
}

fn lookup_movability(local: &Local, locals: &[LocalDecl]) -> Movability {
    locals
        .iter()
        .find(|decl| decl.local == *local)
        .map(|decl| decl.movability)
        .unwrap_or(Movability::Move)
}

fn param_move_mode(
    body: &MirBody,
    place: &Place,
    param_modes: &HashMap<Local, MirParamMode>,
) -> Option<MirParamMode> {
    let Place::Local(local) = place else {
        return None;
    };
    (lookup_movability(local, &body.locals) != Movability::Copy)
        .then(|| param_modes.get(local).copied())
        .flatten()
        .filter(|mode| matches!(mode, MirParamMode::Shared | MirParamMode::Mut))
}

fn moved_at_entry(
    body: &MirBody,
    move_analysis: &MoveAnalysisResult,
    block: BasicBlockId,
) -> HashSet<Local> {
    move_analysis
        .entry_states
        .get(&block)
        .into_iter()
        .flat_map(|state| {
            state
                .status
                .iter()
                .filter_map(|(local, alive)| (!*alive).then_some(local.clone()))
        })
        .filter(|local| {
            body.locals
                .iter()
                .find(|decl| decl.local == *local)
                .is_some()
        })
        .collect()
}

fn add_loan(loans: &mut Vec<Loan>, loan: Loan) {
    if !loans.iter().any(|existing| existing == &loan) {
        loans.push(loan);
    }
}

fn root_local(place: &Place) -> &Local {
    match place {
        Place::Local(local) => local,
        Place::Field(base, _) | Place::Index(base, _) => root_local(base),
    }
}

fn overlaps(lhs: &Place, rhs: &Place) -> bool {
    root_local(lhs) == root_local(rhs)
}

fn place_name(body: &MirBody, place: &Place) -> String {
    match place {
        Place::Local(local) => body
            .locals
            .iter()
            .find(|decl| decl.local == *local)
            .and_then(|decl| decl.name.clone())
            .unwrap_or_else(|| format!("_{}", local.0)),
        Place::Field(base, field) => format!("{}.{}", place_name(body, base), field),
        Place::Index(base, index) => format!(
            "{}[{}]",
            place_name(body, base),
            body.locals
                .iter()
                .find(|decl| decl.local == *index)
                .and_then(|decl| decl.name.clone())
                .unwrap_or_else(|| format!("_{}", index.0))
        ),
    }
}

fn operand_place(op: &Operand) -> Option<&Place> {
    match op {
        Operand::Place(place) | Operand::Move(place) => Some(place),
        Operand::Constant(_) => None,
    }
}

fn violation(
    kind: BorrowViolationKind,
    body: &MirBody,
    place: &Place,
    borrower: impl Into<String>,
    block: BasicBlockId,
    statement: usize,
    message: String,
) -> BorrowViolation {
    BorrowViolation {
        kind,
        place: place_name(body, place),
        borrower: borrower.into(),
        block,
        statement,
        message,
    }
}

fn check_move(
    body: &MirBody,
    place: &Place,
    loans: &[Loan],
    block: BasicBlockId,
    statement: usize,
    violations: &mut Vec<BorrowViolation>,
) {
    for loan in loans {
        if overlaps(place, &loan.place) {
            let borrowed = place_name(body, place);
            let borrower = place_name(body, &Place::Local(loan.holder.clone()));
            violations.push(violation(
                BorrowViolationKind::MoveWhileBorrowed,
                body,
                place,
                borrower.clone(),
                block,
                statement,
                format!("cannot move '{borrowed}' while it is borrowed by '{borrower}'"),
            ));
            break;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn check_borrow(
    body: &MirBody,
    place: &Place,
    kind: LoanKind,
    loans: &[Loan],
    moved: &HashSet<Local>,
    block: BasicBlockId,
    statement: usize,
    violations: &mut Vec<BorrowViolation>,
) {
    if moved.contains(root_local(place)) {
        let name = place_name(body, place);
        violations.push(violation(
            BorrowViolationKind::BorrowOfMoved,
            body,
            place,
            "caller",
            block,
            statement,
            format!("cannot borrow '{name}' after it was moved"),
        ));
        return;
    }
    let conflicts = loans.iter().any(|loan| {
        overlaps(place, &loan.place) && (kind == LoanKind::Mut || loan.kind == LoanKind::Mut)
    });
    if conflicts {
        let name = place_name(body, place);
        violations.push(violation(
            BorrowViolationKind::MutableAliasing,
            body,
            place,
            "caller",
            block,
            statement,
            format!("cannot borrow '{name}' mutably while another borrow is live"),
        ));
    }
}

/// Check MIR loans and callee-side parameter access rules.
fn check_block(
    body: &MirBody,
    block: &crate::mir::BasicBlock,
    entry_loans: &[Loan],
    moved: &mut HashSet<Local>,
    param_modes: &HashMap<Local, MirParamMode>,
    callee_modes: &HashMap<String, Vec<MirParamMode>>,
    violations: &mut Vec<BorrowViolation>,
) -> Vec<Loan> {
    let mut loans = entry_loans.to_vec();
    for (statement_index, statement) in block.statements.iter().enumerate() {
        match statement {
            MirStatement::StorageDead(local) | MirStatement::Drop(local) => {
                loans.retain(|loan| loan.holder != *local);
            }
            MirStatement::Assign(place, Rvalue::Ref(borrowed))
            | MirStatement::Assign(place, Rvalue::RefMut(borrowed)) => {
                let kind = if matches!(statement, MirStatement::Assign(_, Rvalue::RefMut(_))) {
                    LoanKind::Mut
                } else {
                    LoanKind::Shared
                };
                if kind == LoanKind::Mut
                    && matches!(
                        param_modes.get(root_local(borrowed)),
                        Some(MirParamMode::Shared)
                    )
                {
                    let name = place_name(body, borrowed);
                    violations.push(violation(
                        BorrowViolationKind::WriteThroughShared,
                        body,
                        borrowed,
                        "caller",
                        block.id,
                        statement_index,
                        format!("cannot write through shared parameter '{name}'"),
                    ));
                }
                check_borrow(
                    body,
                    borrowed,
                    kind,
                    &loans,
                    moved,
                    block.id,
                    statement_index,
                    violations,
                );
                let Place::Local(holder) = place else {
                    continue;
                };
                loans.retain(|loan| loan.holder != *holder);
                add_loan(
                    &mut loans,
                    Loan {
                        holder: holder.clone(),
                        place: borrowed.clone(),
                        kind,
                    },
                );
            }
            MirStatement::Assign(place, rvalue) => {
                let local = root_local(place);
                if matches!(param_modes.get(local), Some(MirParamMode::Shared)) {
                    let name = place_name(body, place);
                    violations.push(violation(
                        BorrowViolationKind::WriteThroughShared,
                        body,
                        place,
                        "caller",
                        block.id,
                        statement_index,
                        format!("cannot write through shared parameter '{name}'"),
                    ));
                }
                check_rvalue(
                    body,
                    rvalue,
                    &loans,
                    moved,
                    param_modes,
                    callee_modes,
                    block.id,
                    statement_index,
                    violations,
                );
                if loans
                    .iter()
                    .any(|loan| loan.kind == LoanKind::Shared && overlaps(place, &loan.place))
                {
                    let name = place_name(body, place);
                    violations.push(violation(
                        BorrowViolationKind::WriteWhileShared,
                        body,
                        place,
                        "caller",
                        block.id,
                        statement_index,
                        format!("cannot write '{name}' while it is shared-borrowed"),
                    ));
                }
                if let Place::Local(local) = place {
                    if !matches!(rvalue, Rvalue::Ref(_) | Rvalue::RefMut(_)) {
                        moved.remove(local);
                    }
                }
            }
            _ => {}
        }
    }
    loans
}

/// Check MIR loans and callee-side parameter access rules.
pub fn check_borrows(body: &MirBody, move_analysis: &MoveAnalysisResult) -> Vec<BorrowViolation> {
    check_borrows_with_callees(body, move_analysis, &HashMap::new())
}

pub fn check_borrows_with_callees(
    body: &MirBody,
    move_analysis: &MoveAnalysisResult,
    callee_modes: &HashMap<String, Vec<MirParamMode>>,
) -> Vec<BorrowViolation> {
    let mut param_modes = HashMap::new();
    for (local, mode) in &body.param_modes {
        param_modes.insert(local.clone(), *mode);
    }

    let mut entry_loans: HashMap<BasicBlockId, Vec<Loan>> = body
        .blocks
        .iter()
        .map(|block| (block.id, Vec::new()))
        .collect();
    let mut exit_loans = entry_loans.clone();
    let predecessors = body.predecessors();
    loop {
        let mut changed = false;
        for block in &body.blocks {
            let mut incoming = Vec::new();
            if let Some(preds) = predecessors.get(&block.id) {
                for pred in preds {
                    for loan in exit_loans.get(pred).into_iter().flatten() {
                        add_loan(&mut incoming, loan.clone());
                    }
                }
            }
            if incoming != entry_loans[&block.id] {
                entry_loans.insert(block.id, incoming.clone());
                changed = true;
            } else {
                incoming = entry_loans[&block.id].clone();
            }
            let mut ignored = Vec::new();
            let mut moved = moved_at_entry(body, move_analysis, block.id);
            let exit = check_block(
                body,
                block,
                &incoming,
                &mut moved,
                &param_modes,
                callee_modes,
                &mut ignored,
            );
            if exit != exit_loans[&block.id] {
                exit_loans.insert(block.id, exit);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let mut violations = Vec::new();
    for block in &body.blocks {
        let mut moved = moved_at_entry(body, move_analysis, block.id);
        check_block(
            body,
            block,
            &entry_loans[&block.id],
            &mut moved,
            &param_modes,
            callee_modes,
            &mut violations,
        );
    }
    violations
}

#[allow(clippy::too_many_arguments)]
fn check_rvalue(
    body: &MirBody,
    rvalue: &Rvalue,
    loans: &[Loan],
    moved: &mut HashSet<Local>,
    param_modes: &HashMap<Local, MirParamMode>,
    callee_modes: &HashMap<String, Vec<MirParamMode>>,
    block: BasicBlockId,
    statement: usize,
    violations: &mut Vec<BorrowViolation>,
) {
    let mut check_operand =
        |op: &Operand, ordinary_param_move: bool, allow_implicit_local_move: bool| {
            let Some(place) = operand_place(op) else {
                return;
            };
            let whole_local_move = allow_implicit_local_move
                && matches!(op, Operand::Place(Place::Local(local))
                if lookup_movability(local, &body.locals) != Movability::Copy);
            let explicit_move = matches!(op, Operand::Move(_));
            if ordinary_param_move || whole_local_move || explicit_move {
                let local = root_local(place);
                match param_modes.get(local) {
                    Some(MirParamMode::Shared) => {
                        let name = place_name(body, place);
                        violations.push(violation(
                            BorrowViolationKind::WriteThroughShared,
                            body,
                            place,
                            "caller",
                            block,
                            statement,
                            if ordinary_param_move {
                                format!("cannot move out of shared parameter '{name}'")
                            } else {
                                format!("cannot write through shared parameter '{name}'")
                            },
                        ));
                    }
                    Some(MirParamMode::Mut) => {
                        let name = place_name(body, place);
                        violations.push(violation(
                            BorrowViolationKind::MoveWhileBorrowed,
                            body,
                            place,
                            "caller",
                            block,
                            statement,
                            format!("cannot move '{name}' while it is borrowed by 'caller'"),
                        ));
                    }
                    _ => {}
                }
                check_move(body, place, loans, block, statement, violations);
                moved.insert(root_local(place).clone());
            }
        };
    match rvalue {
        Rvalue::Use(op) => {
            check_operand(
                op,
                operand_place(op)
                    .is_some_and(|place| param_move_mode(body, place, param_modes).is_some()),
                true,
            );
        }
        Rvalue::Call { func, args } => {
            for (index, op) in args.iter().enumerate() {
                let ordinary_param_move = matches!(op, Operand::Place(Place::Local(_)))
                    && matches!(
                        callee_modes.get(func).and_then(|modes| modes.get(index)),
                        Some(MirParamMode::Owned)
                    )
                    && operand_place(op)
                        .is_some_and(|place| param_move_mode(body, place, param_modes).is_some());
                check_operand(op, ordinary_param_move, false);
            }
        }
        Rvalue::BinaryOp(_, _, _) => {}
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::{BasicBlock, LocalDecl, MirConstant, Movability, Terminator};

    fn body(statements: Vec<MirStatement>, param_modes: HashMap<Local, MirParamMode>) -> MirBody {
        let locals = (0..4)
            .map(|index| LocalDecl {
                local: Local(index),
                name: Some(match index {
                    0 => "x".to_string(),
                    1 => "shared".to_string(),
                    2 => "mut".to_string(),
                    _ => format!("t{index}"),
                }),
                ty: Some("i64".to_string()),
                movability: Movability::Copy,
                capability: None,
            })
            .collect();
        MirBody {
            name: "borrow_test".to_string(),
            locals,
            blocks: vec![BasicBlock {
                id: 0,
                statements,
                terminator: Terminator::Return(Operand::Constant(MirConstant::Int(0))),
            }],
            entry_block: 0,
            unbound_names: vec![],
            param_modes,
        }
    }

    #[test]
    fn reports_move_while_borrowed() {
        let body = body(
            vec![
                MirStatement::Assign(Place::Local(Local(1)), Rvalue::Ref(Place::Local(Local(0)))),
                MirStatement::Assign(
                    Place::Local(Local(2)),
                    Rvalue::Call {
                        func: "f".to_string(),
                        args: vec![Operand::Move(Place::Local(Local(0)))],
                    },
                ),
            ],
            HashMap::new(),
        );
        let moves = super::super::analyze_moves(&body);
        assert!(check_borrows(&body, &moves)
            .iter()
            .any(|v| v.kind == BorrowViolationKind::MoveWhileBorrowed));
    }

    #[test]
    fn reports_write_while_shared() {
        let body = body(
            vec![
                MirStatement::Assign(Place::Local(Local(1)), Rvalue::Ref(Place::Local(Local(0)))),
                MirStatement::Assign(
                    Place::Local(Local(0)),
                    Rvalue::Use(Operand::Constant(MirConstant::Int(1))),
                ),
            ],
            HashMap::new(),
        );
        let moves = super::super::analyze_moves(&body);
        assert!(check_borrows(&body, &moves)
            .iter()
            .any(|v| v.kind == BorrowViolationKind::WriteWhileShared));
    }

    #[test]
    fn reports_mutable_aliasing() {
        let body = body(
            vec![
                MirStatement::Assign(Place::Local(Local(1)), Rvalue::Ref(Place::Local(Local(0)))),
                MirStatement::Assign(
                    Place::Local(Local(2)),
                    Rvalue::RefMut(Place::Local(Local(0))),
                ),
            ],
            HashMap::new(),
        );
        let moves = super::super::analyze_moves(&body);
        assert!(check_borrows(&body, &moves)
            .iter()
            .any(|v| v.kind == BorrowViolationKind::MutableAliasing));
    }

    #[test]
    fn reports_borrow_of_moved() {
        let body = body(
            vec![
                MirStatement::Assign(
                    Place::Local(Local(1)),
                    Rvalue::Call {
                        func: "f".to_string(),
                        args: vec![Operand::Move(Place::Local(Local(0)))],
                    },
                ),
                MirStatement::Assign(Place::Local(Local(2)), Rvalue::Ref(Place::Local(Local(0)))),
            ],
            HashMap::new(),
        );
        let moves = super::super::analyze_moves(&body);
        assert!(check_borrows(&body, &moves)
            .iter()
            .any(|v| v.kind == BorrowViolationKind::BorrowOfMoved));
    }

    #[test]
    fn reports_write_through_shared_parameter() {
        let mut modes = HashMap::new();
        modes.insert(Local(0), MirParamMode::Shared);
        let body = body(
            vec![MirStatement::Assign(
                Place::Local(Local(0)),
                Rvalue::Use(Operand::Constant(MirConstant::Int(1))),
            )],
            modes,
        );
        let moves = super::super::analyze_moves(&body);
        assert!(check_borrows(&body, &moves)
            .iter()
            .any(|v| v.kind == BorrowViolationKind::WriteThroughShared));
    }

    #[test]
    fn sequential_loan_is_allowed_after_storage_dead() {
        let body = body(
            vec![
                MirStatement::Assign(
                    Place::Local(Local(1)),
                    Rvalue::RefMut(Place::Local(Local(0))),
                ),
                MirStatement::StorageDead(Local(1)),
                MirStatement::Assign(Place::Local(Local(2)), Rvalue::Ref(Place::Local(Local(0)))),
            ],
            HashMap::new(),
        );
        let moves = super::super::analyze_moves(&body);
        assert!(check_borrows(&body, &moves).is_empty());
    }
}
