use crate::parser::{Expr, Op, Stmt};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferredInvariant {
    pub line: usize,
    pub column: usize,
    pub candidates_tried: usize,
    pub adopted: Vec<String>,
}

pub(crate) fn generate_candidates(
    cond: &Expr,
    body: &Stmt,
    modified: &BTreeSet<String>,
    init_prefix: &str,
) -> Vec<Expr> {
    let mut candidates = Vec::new();
    let modified_names: BTreeSet<String> = modified
        .iter()
        .map(|name| name.strip_prefix("__z3_arr_").unwrap_or(name).to_string())
        .collect();

    // Bounded counters are deliberately emitted first: they are generally
    // the cheapest formulas for Z3 to discharge.
    for (var, op, bound, swapped) in condition_comparisons(cond) {
        let stable_length_bound = is_stable_array_length_bound(&bound, body);
        if !modified_names.contains(&var)
            || (mentions_any(&bound, &modified_names) && !stable_length_bound)
        {
            continue;
        }
        if stable_length_bound {
            candidates.push(comparison(
                Expr::Variable(var.clone()),
                Op::Ge,
                Expr::Number(0),
            ));
        }
        let (candidate_op, rhs) = match (op, swapped) {
            (Op::Lt, false) => (Op::Le, bound),
            (Op::Le, false) => (
                Op::Le,
                Expr::BinaryOp(Box::new(bound), Op::Add, Box::new(Expr::Number(1))),
            ),
            (Op::Gt, false) => (Op::Ge, bound),
            (Op::Ge, false) => (
                Op::Ge,
                Expr::BinaryOp(Box::new(bound), Op::Sub, Box::new(Expr::Number(1))),
            ),
            (Op::Neq, false) => {
                candidates.push(comparison(
                    Expr::Variable(var.clone()),
                    Op::Le,
                    bound.clone(),
                ));
                (Op::Ge, bound)
            }
            (Op::Lt, true) => (Op::Ge, bound),
            (Op::Le, true) => (Op::Ge, bound),
            (Op::Gt, true) => (Op::Le, bound),
            (Op::Ge, true) => (Op::Le, bound),
            (Op::Neq, true) => {
                candidates.push(comparison(
                    Expr::Variable(var.clone()),
                    Op::Ge,
                    bound.clone(),
                ));
                (Op::Le, bound)
            }
            _ => continue,
        };
        candidates.push(comparison(Expr::Variable(var), candidate_op, rhs));
    }

    let steps = collect_steps(body);
    for var in &modified_names {
        if let Some(step) = steps.get(var) {
            let snapshot = Expr::Variable(format!("{init_prefix}{var}"));
            let op = if step.sign > 0 { Op::Ge } else { Op::Le };
            candidates.push(comparison(Expr::Variable(var.clone()), op, snapshot));
        } else {
            candidates.push(comparison(
                Expr::Variable(var.clone()),
                Op::Ge,
                Expr::Variable(format!("{init_prefix}{var}")),
            ));
            candidates.push(comparison(
                Expr::Variable(var.clone()),
                Op::Le,
                Expr::Variable(format!("{init_prefix}{var}")),
            ));
        }
    }

    for var in &modified_names {
        if condition_mentions(cond, var) {
            continue;
        }
        candidates.push(comparison(
            Expr::Variable(var.clone()),
            Op::Ge,
            Expr::Number(0),
        ));
        candidates.push(comparison(
            Expr::Variable(var.clone()),
            Op::Ge,
            Expr::Variable(format!("{init_prefix}{var}")),
        ));
        candidates.push(comparison(
            Expr::Variable(var.clone()),
            Op::Le,
            Expr::Variable(format!("{init_prefix}{var}")),
        ));
        if let Some(acc_step) = steps.get(var) {
            if acc_step.sign > 0 {
                for (counter, counter_step) in &steps {
                    if counter == var || counter_step.amount != 1 || counter_step.sign <= 0 {
                        continue;
                    }
                    let lhs = Expr::BinaryOp(
                        Box::new(Expr::Variable(var.clone())),
                        Op::Sub,
                        Box::new(Expr::Variable(format!("{init_prefix}{var}"))),
                    );
                    let rhs = Expr::BinaryOp(
                        Box::new(Expr::Number(acc_step.amount)),
                        Op::Mul,
                        Box::new(Expr::BinaryOp(
                            Box::new(Expr::Variable(counter.clone())),
                            Op::Sub,
                            Box::new(Expr::Variable(format!("{init_prefix}{counter}"))),
                        )),
                    );
                    candidates.push(comparison(lhs, Op::Eq, rhs));
                }
            }
        }
    }

    // Keep the array template conservative. The snapshot is represented by a
    // normal array name, with its tracked __z3_arr_ slot installed by the
    // translator before candidates are checked.
    for (array, index) in array_writes(body) {
        let Some(counter) = index_variable(&index) else {
            continue;
        };
        let Some(step) = steps.get(&counter) else {
            continue;
        };
        if step.sign != 1 || step.amount != 1 || !modified_names.contains(&counter) {
            continue;
        }
        candidates.push(Expr::Call(
            "forall".to_string(),
            vec![
                Expr::Variable("j".to_string()),
                Expr::Variable(counter.clone()),
                Expr::Variable(format!("len_{array}")),
                comparison(
                    Expr::ArrayAccess(array.clone(), Box::new(Expr::Variable("j".to_string()))),
                    Op::Eq,
                    Expr::ArrayAccess(
                        format!("{init_prefix}{array}"),
                        Box::new(Expr::Variable("j".to_string())),
                    ),
                ),
            ],
        ));
    }

    let mut seen = BTreeSet::new();
    candidates.retain(|candidate| seen.insert(expr_to_string(candidate)));
    candidates
}

#[derive(Clone, Copy)]
struct Step {
    sign: i64,
    amount: i64,
}

fn comparison(left: Expr, op: Op, right: Expr) -> Expr {
    Expr::BinaryOp(Box::new(left), op, Box::new(right))
}

fn condition_comparisons(expr: &Expr) -> Vec<(String, Op, Expr, bool)> {
    let mut out = Vec::new();
    match expr {
        Expr::BinaryOp(left, Op::And, right) | Expr::BinaryOp(left, Op::Or, right) => {
            out.extend(condition_comparisons(left));
            out.extend(condition_comparisons(right));
        }
        Expr::BinaryOp(left, op, right)
            if matches!(op, Op::Eq | Op::Neq | Op::Gt | Op::Lt | Op::Ge | Op::Le) =>
        {
            if let Expr::Variable(var) = left.as_ref() {
                out.push((var.clone(), op.clone(), right.as_ref().clone(), false));
            } else if let Expr::Variable(var) = right.as_ref() {
                out.push((var.clone(), op.clone(), left.as_ref().clone(), true));
            }
        }
        _ => {}
    }
    out
}

fn collect_steps(stmt: &Stmt) -> std::collections::BTreeMap<String, Step> {
    let mut out = std::collections::BTreeMap::new();
    collect_steps_inner(stmt, &mut out);
    out
}

fn collect_steps_inner(stmt: &Stmt, out: &mut std::collections::BTreeMap<String, Step>) {
    match stmt {
        Stmt::Assign { var, value, .. } => {
            if let Some(step) = assignment_step(var, value) {
                out.insert(var.clone(), step);
            }
        }
        Stmt::Block(stmts, _) => {
            for stmt in stmts {
                collect_steps_inner(stmt, out);
            }
        }
        Stmt::While { body, .. } | Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
            collect_steps_inner(body, out)
        }
        Stmt::TaskGroup { children, .. } => {
            for child in children {
                collect_steps_inner(child, out);
            }
        }
        Stmt::Expr(_, _) | Stmt::Let { .. } | Stmt::ArrayStore { .. } | Stmt::Cancel { .. } => {}
    }
}

fn assignment_step(var: &str, value: &Expr) -> Option<Step> {
    let Expr::BinaryOp(left, op, right) = value else {
        return None;
    };
    let Expr::Variable(name) = left.as_ref() else {
        return None;
    };
    if name != var {
        return None;
    }
    let Expr::Number(amount) = right.as_ref() else {
        return None;
    };
    if *amount <= 0 || !matches!(op, Op::Add | Op::Sub) {
        return None;
    }
    Some(Step {
        sign: if *op == Op::Add { 1 } else { -1 },
        amount: *amount,
    })
}

fn array_writes(stmt: &Stmt) -> Vec<(String, Expr)> {
    let mut out = Vec::new();
    array_writes_inner(stmt, &mut out);
    out
}

fn array_writes_inner(stmt: &Stmt, out: &mut Vec<(String, Expr)>) {
    match stmt {
        Stmt::ArrayStore { array, index, .. } => out.push((array.clone(), index.as_ref().clone())),
        Stmt::Block(stmts, _) => {
            for stmt in stmts {
                array_writes_inner(stmt, out);
            }
        }
        Stmt::While { body, .. } | Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
            array_writes_inner(body, out)
        }
        Stmt::TaskGroup { children, .. } => {
            for child in children {
                array_writes_inner(child, out);
            }
        }
        Stmt::Let { .. } | Stmt::Assign { .. } | Stmt::Expr(_, _) | Stmt::Cancel { .. } => {}
    }
}

fn index_variable(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Variable(name) => Some(name.clone()),
        _ => None,
    }
}

fn condition_mentions(expr: &Expr, name: &str) -> bool {
    let mut vars = BTreeSet::new();
    collect_variables(expr, &mut vars);
    vars.contains(name)
}

fn mentions_any(expr: &Expr, names: &BTreeSet<String>) -> bool {
    let mut vars = BTreeSet::new();
    collect_variables(expr, &mut vars);
    vars.iter().any(|name| names.contains(name))
}

fn is_stable_array_length_bound(expr: &Expr, body: &Stmt) -> bool {
    let Expr::Call(name, args) = expr else {
        return false;
    };
    let [Expr::Variable(array)] = args.as_slice() else {
        return false;
    };
    if name != "len"
        || !array_writes(body)
            .iter()
            .any(|(written, _)| written == array)
    {
        return false;
    }
    !assigns_variable(body, array)
}

fn assigns_variable(stmt: &Stmt, name: &str) -> bool {
    match stmt {
        Stmt::Assign { var, .. } => var == name,
        Stmt::Block(stmts, _) => stmts.iter().any(|stmt| assigns_variable(stmt, name)),
        Stmt::While { body, .. } | Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
            assigns_variable(body, name)
        }
        Stmt::TaskGroup { children, .. } => {
            children.iter().any(|child| assigns_variable(child, name))
        }
        _ => false,
    }
}

fn collect_variables(expr: &Expr, out: &mut BTreeSet<String>) {
    match expr {
        Expr::Variable(name) => {
            out.insert(name.clone());
        }
        Expr::ArrayAccess(_, index) => collect_variables(index, out),
        Expr::BinaryOp(left, _, right) => {
            collect_variables(left, out);
            collect_variables(right, out);
        }
        Expr::Call(_, args) => {
            for arg in args {
                collect_variables(arg, out);
            }
        }
        Expr::FieldAccess(inner, _) => collect_variables(inner, out),
        Expr::ArrayLit(elements) => {
            for element in elements {
                collect_variables(element, out);
            }
        }
        _ => {}
    }
}

pub(crate) fn expr_to_string(expr: &Expr) -> String {
    match expr {
        Expr::Number(value) => value.to_string(),
        Expr::Float(value) => value.to_string(),
        Expr::StringLit(value) => format!("\"{}\"", value),
        Expr::Variable(name) => name.clone(),
        Expr::ArrayAccess(array, index) => format!("{array}[{}]", expr_to_string(index)),
        Expr::BinaryOp(left, op, right) => format!(
            "({} {} {})",
            expr_to_string(left),
            match op {
                Op::Add => "+",
                Op::Sub => "-",
                Op::Mul => "*",
                Op::Div => "/",
                Op::Eq => "==",
                Op::Neq => "!=",
                Op::Gt => ">",
                Op::Lt => "<",
                Op::Ge => ">=",
                Op::Le => "<=",
                Op::And => "&&",
                Op::Or => "||",
                Op::Implies => "==>",
                Op::Pow => "**",
                Op::BitAnd => "&",
                Op::BitOr => "|",
                Op::BitXor => "^",
                Op::Shl => "<<",
                Op::Shr => ">>",
            },
            expr_to_string(right)
        ),
        Expr::Call(name, args) => format!(
            "{name}({})",
            args.iter()
                .map(expr_to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Expr::FieldAccess(inner, field) => format!("{}.{}", expr_to_string(inner), field),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Span;

    #[test]
    fn counter_candidates_are_ordered_before_progress_and_bounds() {
        let cond = comparison(
            Expr::Variable("i".to_string()),
            Op::Lt,
            Expr::Variable("n".to_string()),
        );
        let body = Stmt::Block(
            vec![
                Stmt::Assign {
                    var: "sum".to_string(),
                    value: Box::new(Expr::BinaryOp(
                        Box::new(Expr::Variable("sum".to_string())),
                        Op::Add,
                        Box::new(Expr::Number(1)),
                    )),
                    span: Span::default(),
                },
                Stmt::Assign {
                    var: "i".to_string(),
                    value: Box::new(Expr::BinaryOp(
                        Box::new(Expr::Variable("i".to_string())),
                        Op::Add,
                        Box::new(Expr::Number(1)),
                    )),
                    span: Span::default(),
                },
            ],
            Span::default(),
        );
        let candidates = generate_candidates(
            &cond,
            &body,
            &BTreeSet::from(["i".to_string(), "sum".to_string()]),
            "__loop_init_",
        );
        assert_eq!(expr_to_string(&candidates[0]), "(i <= n)");
        assert!(candidates
            .iter()
            .any(|candidate| { expr_to_string(candidate) == "(i >= __loop_init_i)" }));
        assert!(candidates.iter().any(|candidate| {
            expr_to_string(candidate) == "((sum - __loop_init_sum) == (1 * (i - __loop_init_i)))"
        }));
        assert_eq!(
            candidates
                .iter()
                .filter(|candidate| expr_to_string(candidate) == "(sum >= __loop_init_sum)")
                .count(),
            1
        );
    }
}
