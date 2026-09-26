use super::MoveAnalysisResult;
use crate::mir::{
    BasicBlockId, Local, MirBody, MirParamMode, MirStatement, Operand, Place, Rvalue,
};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoanKind {
    Shared,
    Mut,
}

#[derive(Debug, Clone)]
struct Loan {
    holder: Local,
    place: Place,
    kind: LoanKind,
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
pub fn check_borrows(body: &MirBody, move_analysis: &MoveAnalysisResult) -> Vec<BorrowViolation> {
    let mut violations = Vec::new();
    let mut param_modes = HashMap::new();
    for (local, mode) in &body.param_modes {
        param_modes.insert(local.clone(), *mode);
    }

    for block in &body.blocks {
        let mut loans = Vec::<Loan>::new();
        let mut moved: HashSet<Local> = move_analysis
            .entry_states
            .get(&block.id)
            .into_iter()
            .flat_map(|state| {
                state
                    .status
                    .iter()
                    .filter_map(|(local, alive)| (!*alive).then_some(local.clone()))
            })
            .collect();
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
                        &moved,
                        block.id,
                        statement_index,
                        &mut violations,
                    );
                    loans.push(Loan {
                        holder: match place {
                            Place::Local(local) => local.clone(),
                            _ => continue,
                        },
                        place: borrowed.clone(),
                        kind,
                    });
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
                        &mut moved,
                        &param_modes,
                        block.id,
                        statement_index,
                        &mut violations,
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
                }
                _ => {}
            }
        }
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
    block: BasicBlockId,
    statement: usize,
    violations: &mut Vec<BorrowViolation>,
) {
    let operands = match rvalue {
        Rvalue::Call { args, .. } => args.as_slice(),
        Rvalue::Use(op) => std::slice::from_ref(op),
        _ => &[],
    };
    for op in operands {
        let Some(place) = operand_place(op) else {
            continue;
        };
        if matches!(op, Operand::Move(_)) {
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
                        format!("cannot write through shared parameter '{name}'"),
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
    }
    // A call's arguments form one region. Check all move/loan pairs together
    // so argument order cannot hide `ref`/`consume` conflicts.
    if let Rvalue::Call { args, .. } = rvalue {
        let call_loans: Vec<&Loan> = args
            .iter()
            .filter_map(|arg| match arg {
                Operand::Place(Place::Local(holder)) => {
                    loans.iter().find(|loan| loan.holder == *holder)
                }
                _ => None,
            })
            .collect();
        for arg in args {
            if let Operand::Move(place) = arg {
                for loan in &call_loans {
                    if overlaps(place, &loan.place) {
                        let name = place_name(body, place);
                        let borrower = place_name(body, &Place::Local(loan.holder.clone()));
                        violations.push(violation(
                            BorrowViolationKind::MoveWhileBorrowed,
                            body,
                            place,
                            borrower.clone(),
                            block,
                            statement,
                            format!("cannot move '{name}' while it is borrowed by '{borrower}'"),
                        ));
                    }
                }
            }
        }
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
