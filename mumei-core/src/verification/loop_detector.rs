use crate::parser::{parse_body_expr, Atom, Expr, Op, Stmt};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopInfo {
    pub line: usize,
    pub loop_type: LoopType,
    pub has_invariant: bool,
    pub variables: Vec<String>,
    pub needs_invariant: bool,
    pub context: LoopContext,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoopType {
    While,
    Recursive,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LoopContext {
    pub variables: Vec<String>,
    pub precondition: String,
    pub postcondition: String,
    pub condition: String,
    pub body: String,
}

/// Detect loops in an atom that may benefit from CEGIS-generated invariants.
pub fn detect_loops_needing_invariants(atom: &Atom) -> Vec<LoopInfo> {
    let parsed_body = parse_body_expr(&atom.body_expr);
    let mut loops = Vec::new();
    detect_loops_in_stmt(&parsed_body, &mut loops, atom);
    detect_recursive_loop(atom, &mut loops);
    loops
        .into_iter()
        .map(|mut loop_info| {
            loop_info.needs_invariant = should_require_invariant(&loop_info, atom);
            loop_info
        })
        .filter(|loop_info| loop_info.needs_invariant)
        .collect()
}

fn detect_loops_in_stmt(stmt: &Stmt, loops: &mut Vec<LoopInfo>, atom: &Atom) {
    match stmt {
        Stmt::While {
            cond,
            invariant,
            body,
            span,
            ..
        } => {
            let mut variables = sorted_variables(cond);
            for variable in mutated_variables(body) {
                if !variables.contains(&variable) {
                    variables.push(variable);
                }
            }
            variables.sort();
            let has_invariant =
                !matches!(invariant.as_ref(), Expr::Variable(value) if value == "true");
            loops.push(LoopInfo {
                line: span.line,
                loop_type: LoopType::While,
                has_invariant,
                variables: variables.clone(),
                needs_invariant: false,
                context: LoopContext {
                    variables,
                    precondition: atom.requires.clone(),
                    postcondition: atom.ensures.clone(),
                    condition: expr_to_source(cond),
                    body: stmt_to_source(body),
                },
            });
            detect_loops_in_stmt(body, loops, atom);
        }
        Stmt::Block(stmts, _) => {
            for child in stmts {
                detect_loops_in_stmt(child, loops, atom);
            }
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
            detect_loops_in_stmt(body, loops, atom);
        }
        Stmt::TaskGroup { children, .. } => {
            for child in children {
                detect_loops_in_stmt(child, loops, atom);
            }
        }
        Stmt::Expr(expr, _) => detect_loops_in_expr(expr, loops, atom),
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            detect_loops_in_expr(value, loops, atom);
        }
        Stmt::ArrayStore { index, value, .. } => {
            detect_loops_in_expr(index, loops, atom);
            detect_loops_in_expr(value, loops, atom);
        }
        Stmt::Cancel { .. } => {}
    }
}

fn detect_loops_in_expr(expr: &Expr, loops: &mut Vec<LoopInfo>, atom: &Atom) {
    match expr {
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            detect_loops_in_expr(cond, loops, atom);
            detect_loops_in_stmt(then_branch, loops, atom);
            detect_loops_in_stmt(else_branch, loops, atom);
        }
        Expr::BinaryOp(left, _, right) => {
            detect_loops_in_expr(left, loops, atom);
            detect_loops_in_expr(right, loops, atom);
        }
        Expr::Call(_, args) => {
            for arg in args {
                detect_loops_in_expr(arg, loops, atom);
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                detect_loops_in_expr(value, loops, atom);
            }
        }
        Expr::FieldAccess(inner, _) => detect_loops_in_expr(inner, loops, atom),
        Expr::Match { target, arms } => {
            detect_loops_in_expr(target, loops, atom);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    detect_loops_in_expr(guard, loops, atom);
                }
                detect_loops_in_stmt(&arm.body, loops, atom);
            }
        }
        Expr::Async { body } | Expr::Lambda { body, .. } => detect_loops_in_stmt(body, loops, atom),
        Expr::Await { expr } => detect_loops_in_expr(expr, loops, atom),
        Expr::CallRef { callee, args } => {
            detect_loops_in_expr(callee, loops, atom);
            for arg in args {
                detect_loops_in_expr(arg, loops, atom);
            }
        }
        Expr::Perform { args, .. } => {
            for arg in args {
                detect_loops_in_expr(arg, loops, atom);
            }
        }
        Expr::ChanSend { channel, value } => {
            detect_loops_in_expr(channel, loops, atom);
            detect_loops_in_expr(value, loops, atom);
        }
        Expr::ChanRecv { channel } => detect_loops_in_expr(channel, loops, atom),
        Expr::Number(_)
        | Expr::Float(_)
        | Expr::StringLit(_)
        | Expr::Variable(_)
        | Expr::ArrayAccess(_, _)
        | Expr::AtomRef { .. } => {}
    }
}

fn detect_recursive_loop(atom: &Atom, loops: &mut Vec<LoopInfo>) {
    if !atom.body_expr.contains(&format!("{}(", atom.name)) {
        return;
    }
    let variables = atom.params.iter().map(|param| param.name.clone()).collect();
    loops.push(LoopInfo {
        line: atom.span.line,
        loop_type: LoopType::Recursive,
        has_invariant: atom.invariant.is_some(),
        variables,
        needs_invariant: false,
        context: LoopContext {
            variables: atom.params.iter().map(|param| param.name.clone()).collect(),
            precondition: atom.requires.clone(),
            postcondition: atom.ensures.clone(),
            condition: format!("recursive call to {}", atom.name),
            body: atom.body_expr.clone(),
        },
    });
}

fn should_require_invariant(loop_info: &LoopInfo, atom: &Atom) -> bool {
    if !loop_info.has_invariant {
        return true;
    }
    let postcondition_refs_loop_vars = loop_info
        .variables
        .iter()
        .any(|var| expression_references_identifier(&atom.ensures, var));
    let modifies_loop_state = !loop_info.variables.is_empty();
    let is_complex = loop_info.variables.len() > 2;
    (modifies_loop_state && postcondition_refs_loop_vars) || is_complex
}

fn sorted_variables(expr: &Expr) -> Vec<String> {
    let mut variables = HashSet::new();
    collect_expr_variables(expr, &mut variables);
    let mut variables: Vec<String> = variables.into_iter().collect();
    variables.sort();
    variables
}

fn mutated_variables(stmt: &Stmt) -> Vec<String> {
    let mut variables = HashSet::new();
    collect_mutated_variables(stmt, &mut variables);
    let mut variables: Vec<String> = variables.into_iter().collect();
    variables.sort();
    variables
}

fn collect_mutated_variables(stmt: &Stmt, variables: &mut HashSet<String>) {
    match stmt {
        Stmt::Let { var, .. } | Stmt::Assign { var, .. } => {
            variables.insert(var.clone());
        }
        Stmt::ArrayStore { array, .. } => {
            variables.insert(array.clone());
        }
        Stmt::Block(stmts, _) => {
            for stmt in stmts {
                collect_mutated_variables(stmt, variables);
            }
        }
        Stmt::While { body, .. } | Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
            collect_mutated_variables(body, variables);
        }
        Stmt::TaskGroup { children, .. } => {
            for child in children {
                collect_mutated_variables(child, variables);
            }
        }
        Stmt::Expr(_, _) | Stmt::Cancel { .. } => {}
    }
}

fn collect_expr_variables(expr: &Expr, variables: &mut HashSet<String>) {
    match expr {
        Expr::Variable(name) => {
            if !matches!(name.as_str(), "true" | "false") {
                variables.insert(name.clone());
            }
        }
        Expr::ArrayAccess(array, index) => {
            variables.insert(array.clone());
            collect_expr_variables(index, variables);
        }
        Expr::BinaryOp(left, _, right) => {
            collect_expr_variables(left, variables);
            collect_expr_variables(right, variables);
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_expr_variables(cond, variables);
            collect_stmt_variables(then_branch, variables);
            collect_stmt_variables(else_branch, variables);
        }
        Expr::Call(_, args) => {
            for arg in args {
                collect_expr_variables(arg, variables);
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_expr_variables(value, variables);
            }
        }
        Expr::FieldAccess(inner, _) => collect_expr_variables(inner, variables),
        Expr::Match { target, arms } => {
            collect_expr_variables(target, variables);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_expr_variables(guard, variables);
                }
                collect_stmt_variables(&arm.body, variables);
            }
        }
        Expr::Async { body } | Expr::Lambda { body, .. } => collect_stmt_variables(body, variables),
        Expr::Await { expr } => collect_expr_variables(expr, variables),
        Expr::CallRef { callee, args } => {
            collect_expr_variables(callee, variables);
            for arg in args {
                collect_expr_variables(arg, variables);
            }
        }
        Expr::Perform { args, .. } => {
            for arg in args {
                collect_expr_variables(arg, variables);
            }
        }
        Expr::ChanSend { channel, value } => {
            collect_expr_variables(channel, variables);
            collect_expr_variables(value, variables);
        }
        Expr::ChanRecv { channel } => collect_expr_variables(channel, variables),
        Expr::Number(_) | Expr::Float(_) | Expr::StringLit(_) | Expr::AtomRef { .. } => {}
    }
}

fn collect_stmt_variables(stmt: &Stmt, variables: &mut HashSet<String>) {
    match stmt {
        Stmt::Let { var, value, .. } | Stmt::Assign { var, value, .. } => {
            variables.insert(var.clone());
            collect_expr_variables(value, variables);
        }
        Stmt::ArrayStore {
            array,
            index,
            value,
            ..
        } => {
            variables.insert(array.clone());
            collect_expr_variables(index, variables);
            collect_expr_variables(value, variables);
        }
        Stmt::Block(stmts, _) => {
            for stmt in stmts {
                collect_stmt_variables(stmt, variables);
            }
        }
        Stmt::While {
            cond,
            invariant,
            body,
            ..
        } => {
            collect_expr_variables(cond, variables);
            collect_expr_variables(invariant, variables);
            collect_stmt_variables(body, variables);
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
            collect_stmt_variables(body, variables);
        }
        Stmt::TaskGroup { children, .. } => {
            for child in children {
                collect_stmt_variables(child, variables);
            }
        }
        Stmt::Expr(expr, _) => collect_expr_variables(expr, variables),
        Stmt::Cancel { .. } => {}
    }
}

fn expression_references_identifier(expression: &str, identifier: &str) -> bool {
    let mut current = String::new();
    for ch in expression.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            current.push(ch);
        } else {
            if current == identifier {
                return true;
            }
            current.clear();
        }
    }
    false
}

fn expr_to_source(expr: &Expr) -> String {
    match expr {
        Expr::Number(value) => value.to_string(),
        Expr::Float(value) => value.to_string(),
        Expr::StringLit(value) => format!("{value:?}"),
        Expr::Variable(name) => name.clone(),
        Expr::ArrayAccess(array, index) => format!("{}[{}]", array, expr_to_source(index)),
        Expr::BinaryOp(left, op, right) => {
            format!(
                "{} {} {}",
                expr_to_source(left),
                op_to_source(op),
                expr_to_source(right)
            )
        }
        Expr::Call(name, args) => format!(
            "{}({})",
            name,
            args.iter()
                .map(expr_to_source)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Expr::FieldAccess(inner, field) => format!("{}.{}", expr_to_source(inner), field),
        Expr::AtomRef { name } => format!("atom_ref({name})"),
        Expr::IfThenElse { .. }
        | Expr::StructInit { .. }
        | Expr::Match { .. }
        | Expr::Async { .. }
        | Expr::Await { .. }
        | Expr::CallRef { .. }
        | Expr::Perform { .. }
        | Expr::Lambda { .. }
        | Expr::ChanSend { .. }
        | Expr::ChanRecv { .. } => format!("{expr:?}"),
    }
}

fn stmt_to_source(stmt: &Stmt) -> String {
    match stmt {
        Stmt::Let { var, value, .. } => format!("let {} = {}", var, expr_to_source(value)),
        Stmt::Assign { var, value, .. } => format!("{} = {}", var, expr_to_source(value)),
        Stmt::ArrayStore {
            array,
            index,
            value,
            ..
        } => format!(
            "{}[{}] = {}",
            array,
            expr_to_source(index),
            expr_to_source(value)
        ),
        Stmt::Block(stmts, _) => stmts
            .iter()
            .map(stmt_to_source)
            .collect::<Vec<_>>()
            .join("; "),
        Stmt::While { cond, body, .. } => {
            format!(
                "while {} {{ {} }}",
                expr_to_source(cond),
                stmt_to_source(body)
            )
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => stmt_to_source(body),
        Stmt::TaskGroup { children, .. } => children
            .iter()
            .map(stmt_to_source)
            .collect::<Vec<_>>()
            .join("; "),
        Stmt::Cancel { target, .. } => format!("cancel {target}"),
        Stmt::Expr(expr, _) => expr_to_source(expr),
    }
}

fn op_to_source(op: &Op) -> &'static str {
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
        Op::Implies => "=>",
        Op::Pow => "**",
    }
}
