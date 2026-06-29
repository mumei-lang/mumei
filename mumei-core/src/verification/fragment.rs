use super::module_env::*;
use super::nlae_reporter::*;
use super::types::*;
use super::*;

pub fn detect_logic_fragment_tags(atom: &Atom, module_env: &ModuleEnv) -> Vec<String> {
    let mut tags = Vec::new();

    for param in &atom.params {
        if param
            .type_name
            .as_ref()
            .is_some_and(|type_name| module_env.enums.contains_key(type_name))
        {
            push_unique_tag(&mut tags, "inductive_data_type");
        }
    }

    let requires_expr = parse_expression(&atom.requires);
    let ensures_expr = parse_expression(&atom.ensures);
    let body_stmt = parse_body_expr(&atom.body_expr);
    let contract_text = atom_contract_text(atom);

    if expr_has_nonlinear_arithmetic(&requires_expr)
        || expr_has_nonlinear_arithmetic(&ensures_expr)
        || stmt_has_nonlinear_arithmetic(&body_stmt)
        || text_has_nonlinear_arithmetic_marker(&contract_text)
    {
        push_unique_tag(&mut tags, "nonlinear_arithmetic");
    }
    if expr_has_inductive_shape(&requires_expr)
        || expr_has_inductive_shape(&ensures_expr)
        || stmt_has_inductive_shape(&body_stmt)
    {
        push_unique_tag(&mut tags, "inductive_data_type");
    }

    if let Some(invariant) = &atom.invariant {
        push_unique_tag(&mut tags, "recursive_invariant");
        let invariant_expr = parse_expression(invariant);
        if expr_has_nonlinear_arithmetic(&invariant_expr)
            || text_has_nonlinear_arithmetic_marker(invariant)
        {
            push_unique_tag(&mut tags, "nonlinear_arithmetic");
        }
    }

    if stmt_has_while(&body_stmt) {
        push_unique_tag(&mut tags, "recursive_invariant");
    }
    if atom_has_unbounded_array_access(atom, &requires_expr, &ensures_expr, &body_stmt) {
        push_unique_tag(&mut tags, "array_without_bounds");
    }

    let has_forall = atom
        .forall_constraints
        .iter()
        .any(|q| q.q_type == QuantifierType::ForAll)
        || atom.requires.contains("forall(")
        || atom.ensures.contains("forall(");
    let has_exists = atom
        .forall_constraints
        .iter()
        .any(|q| q.q_type == QuantifierType::Exists)
        || atom.requires.contains("exists(")
        || atom.ensures.contains("exists(");
    if has_forall && has_exists {
        push_unique_tag(&mut tags, "quantifier_alternation");
    }
    if atom.forall_constraints.iter().any(|q| {
        q.condition.contains('[')
            || q.condition.contains("forall(")
            || q.condition.contains("exists(")
    }) || atom.requires.contains("forall(")
        || atom.ensures.contains("forall(")
    {
        push_unique_tag(&mut tags, "trigger_sensitive_quantifier");
    }
    if atom_uses_complex_temporal_effect(atom, module_env) {
        push_unique_tag(&mut tags, "complex_temporal_effect");
    }
    if atom_has_nested_mutable_aliasing(atom, &body_stmt) {
        push_unique_tag(&mut tags, "nested_aliasing");
    }
    if atom_has_regex_semantics(atom, &requires_expr, &ensures_expr, &body_stmt) {
        push_unique_tag(&mut tags, "regex_semantics");
    }
    if atom_uses_finite_field_semantics(atom) {
        push_unique_tag(&mut tags, "finite_field");
    }

    tags
}

pub fn detect_logic_fragment(atom: &Atom, module_env: &ModuleEnv) -> Vec<LogicFragment> {
    let requires_expr = parse_expression(&atom.requires);
    let ensures_expr = parse_expression(&atom.ensures);
    let body_stmt = parse_body_expr(&atom.body_expr);
    let contract_text = atom_contract_text(atom);
    let mut fragments = Vec::new();

    if expr_has_nonlinear_arithmetic(&requires_expr)
        || expr_has_nonlinear_arithmetic(&ensures_expr)
        || stmt_has_nonlinear_arithmetic(&body_stmt)
        || text_has_nonlinear_arithmetic_marker(&contract_text)
    {
        push_unique_fragment(&mut fragments, LogicFragment::NonlinearArithmetic);
    } else if expr_has_linear_arithmetic(&requires_expr)
        || expr_has_linear_arithmetic(&ensures_expr)
        || stmt_has_linear_arithmetic(&body_stmt)
        || atom.forall_constraints.iter().any(|q| {
            expression_text_has_linear_arithmetic(&q.start)
                || expression_text_has_linear_arithmetic(&q.end)
                || expr_has_linear_arithmetic(&parse_expression(&q.condition))
        })
    {
        push_unique_fragment(&mut fragments, LogicFragment::LinearArithmetic);
    }

    if expr_has_array_access(&requires_expr)
        || expr_has_array_access(&ensures_expr)
        || stmt_has_array_access(&body_stmt)
        || atom.forall_constraints.iter().any(|q| {
            expr_has_array_access(&parse_expression(&q.condition)) || q.condition.contains('[')
        })
    {
        push_unique_fragment(&mut fragments, LogicFragment::ArrayAccess);
    }

    if atom_has_quantifier_alternation(atom) {
        push_unique_fragment(&mut fragments, LogicFragment::QuantifierAlternation);
    }

    if atom_uses_temporal_effect(atom, module_env) {
        push_unique_fragment(&mut fragments, LogicFragment::TemporalState);
    }
    if atom_uses_finite_field_semantics(atom) {
        push_unique_fragment(&mut fragments, LogicFragment::FiniteField);
    }

    if fragments.is_empty() {
        push_unique_fragment(&mut fragments, LogicFragment::LinearArithmetic);
    }

    fragments
}

/// Returns true if the atom's logic fragment tags indicate it is outside
/// the Z3-decidable fragment and should trigger a warning.
pub fn is_outside_decidable_fragment(tags: &[String]) -> bool {
    const OUTSIDE_TAGS: &[&str] = &[
        "nonlinear_arithmetic",
        "quantifier_alternation",
        "array_without_bounds",
        "inductive_data_type",
        "finite_field",
    ];
    tags.iter().any(|tag| OUTSIDE_TAGS.contains(&tag.as_str()))
}

pub fn outside_decidable_fragment_diagnostic(
    atom: &Atom,
    module_env: &ModuleEnv,
) -> Option<Diagnostic> {
    let tags = detect_logic_fragment_tags(atom, module_env);
    if !is_outside_decidable_fragment(&tags) {
        None
    } else {
        Some(Diagnostic {
            code: "outside_decidable_fragment".to_string(),
            severity: "warning".to_string(),
            atom: atom.name.clone(),
            message: outside_decidable_fragment_message(atom, &tags),
            tags,
            escalation_reason: Some("outside_decidable_fragment".to_string()),
        })
    }
}

pub fn outside_decidable_fragment_warning(atom: &Atom, module_env: &ModuleEnv) -> Option<String> {
    outside_decidable_fragment_diagnostic(atom, module_env).map(|diagnostic| {
        format!(
            "{}: {} [{}]",
            diagnostic.code,
            diagnostic.message,
            diagnostic.tags.join(", ")
        )
    })
}

pub fn collect_decidable_fragment_metrics(module_env: &ModuleEnv) -> DecidableFragmentMetrics {
    let mut metrics = DecidableFragmentMetrics::default();

    for atom in module_env.atoms.values() {
        metrics.total_atoms_checked += 1;
        let tags = detect_logic_fragment_tags(atom, module_env);
        if !is_outside_decidable_fragment(&tags) {
            continue;
        }
        metrics.atoms_with_warnings += 1;
        for tag in tags {
            *metrics.warning_counts.entry(tag).or_insert(0) += 1;
        }
    }

    metrics
}

pub(crate) fn push_unique_tag(tags: &mut Vec<String>, tag: &str) {
    if !tags.iter().any(|existing| existing == tag) {
        tags.push(tag.to_string());
    }
}

pub(crate) fn push_unique_fragment(fragments: &mut Vec<LogicFragment>, fragment: LogicFragment) {
    if !fragments.contains(&fragment) {
        fragments.push(fragment);
    }
}

pub(crate) fn atom_contract_text(atom: &Atom) -> String {
    let mut parts = vec![
        atom.requires.as_str(),
        atom.ensures.as_str(),
        atom.body_expr.as_str(),
    ];
    if let Some(invariant) = &atom.invariant {
        parts.push(invariant.as_str());
    }
    for q in &atom.forall_constraints {
        parts.push(q.start.as_str());
        parts.push(q.end.as_str());
        parts.push(q.condition.as_str());
    }
    parts.join(" ")
}

fn atom_uses_finite_field_semantics(atom: &Atom) -> bool {
    let text = atom_contract_text(atom);
    [
        "ff_eq(",
        "ff_zero(",
        "ff_one(",
        "ff_add(",
        "ff_mul(",
        "ff_in_field(",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

pub(crate) fn outside_decidable_fragment_message(atom: &Atom, tags: &[String]) -> String {
    if tags.iter().any(|tag| tag == "nonlinear_arithmetic") {
        if let Some(pattern) = first_nonlinear_arithmetic_pattern(atom) {
            format!(
                "atom `{}` uses nonlinear arithmetic ({}), consider Lean escalation",
                atom.name, pattern
            )
        } else {
            format!(
                "atom `{}` uses nonlinear arithmetic, consider Lean escalation",
                atom.name
            )
        }
    } else if tags.iter().any(|tag| tag == "quantifier_alternation") {
        let pattern = quantifier_alternation_pattern(atom);
        format!(
            "atom `{}` uses quantifier alternation ({}), escalation recommended",
            atom.name, pattern
        )
    } else if tags.iter().any(|tag| tag == "array_without_bounds") {
        if let Some(pattern) = first_array_access_pattern(atom) {
            format!(
                "atom `{}` accesses array without explicit bounds ({}), add 0 <= i && i < len(a)",
                atom.name, pattern
            )
        } else {
            format!(
                "atom `{}` accesses array without explicit bounds, add 0 <= i && i < len(a)",
                atom.name
            )
        }
    } else if tags.iter().any(|tag| tag == "finite_field") {
        format!(
            "atom `{}` uses finite-field helper semantics, route through Lean escalation",
            atom.name
        )
    } else {
        format!(
            "atom `{}` uses fragments outside Z3-stable range: [{}]",
            atom.name,
            tags.join(", ")
        )
    }
}

pub(crate) fn text_has_nonlinear_arithmetic_marker(text: &str) -> bool {
    text.contains('%')
        || text.contains("**")
        || text.contains("pow(")
        || text.contains("mod(")
        || text.contains("exp(")
}

pub(crate) fn first_nonlinear_arithmetic_pattern(atom: &Atom) -> Option<String> {
    let contract_text = atom_contract_text(atom);
    let symbolic_op =
        regex::Regex::new(r"([A-Za-z_][A-Za-z0-9_]*)\s*(\*\*|\*|/|%)\s*([A-Za-z_][A-Za-z0-9_]*)")
            .ok()?;
    if let Some(captures) = symbolic_op.captures(&contract_text) {
        return Some(format!(
            "{} {} {}",
            captures.get(1)?.as_str(),
            captures.get(2)?.as_str(),
            captures.get(3)?.as_str()
        ));
    }
    let call_op = regex::Regex::new(r"\b(pow|mod|exp)\s*\(([^)]*)\)").ok()?;
    call_op
        .captures(&contract_text)
        .and_then(|captures| captures.get(0).map(|matched| matched.as_str().to_string()))
}

pub(crate) fn first_array_access_pattern(atom: &Atom) -> Option<String> {
    let contract_text = atom_contract_text(atom);
    let re = regex::Regex::new(r"([A-Za-z_][A-Za-z0-9_]*)\s*\[\s*([^]]+?)\s*\]").ok()?;
    re.captures(&contract_text).and_then(|captures| {
        Some(format!(
            "{}[{}]",
            captures.get(1)?.as_str(),
            captures.get(2)?.as_str().trim()
        ))
    })
}

pub(crate) fn quantifier_alternation_pattern(atom: &Atom) -> &'static str {
    let text = atom_contract_text(atom).to_ascii_lowercase();
    let forall_pos = text.find("forall");
    let exists_pos = text.find("exists");
    match (forall_pos, exists_pos) {
        (Some(forall), Some(exists)) if forall < exists => "forall exists",
        (Some(_), Some(_)) => "exists forall",
        _ if atom
            .forall_constraints
            .iter()
            .any(|q| q.q_type == QuantifierType::ForAll)
            && atom
                .forall_constraints
                .iter()
                .any(|q| q.q_type == QuantifierType::Exists) =>
        {
            "forall exists"
        }
        _ => "mixed quantifiers",
    }
}

pub(crate) fn atom_has_quantifier_alternation(atom: &Atom) -> bool {
    let has_forall = atom
        .forall_constraints
        .iter()
        .any(|q| q.q_type == QuantifierType::ForAll)
        || atom.requires.contains("forall(")
        || atom.ensures.contains("forall(");
    let has_exists = atom
        .forall_constraints
        .iter()
        .any(|q| q.q_type == QuantifierType::Exists)
        || atom.requires.contains("exists(")
        || atom.ensures.contains("exists(");
    has_forall && has_exists
}

pub(crate) fn expression_text_has_linear_arithmetic(text: &str) -> bool {
    expr_has_linear_arithmetic(&parse_expression(text))
}

pub(crate) fn atom_has_nested_mutable_aliasing(atom: &Atom, body_stmt: &Stmt) -> bool {
    let mutable_refs = atom.params.iter().filter(|param| param.is_ref_mut).count();
    mutable_refs > 1 || stmt_has_nested_mutable_scope(body_stmt, 0)
}

pub(crate) fn stmt_has_nested_mutable_scope(stmt: &Stmt, depth: usize) -> bool {
    match stmt {
        Stmt::Acquire { body, .. } => depth > 0 || stmt_has_nested_mutable_scope(body, depth + 1),
        Stmt::Block(stmts, _) => stmts
            .iter()
            .any(|stmt| stmt_has_nested_mutable_scope(stmt, depth)),
        Stmt::While { body, .. } | Stmt::Task { body, .. } => {
            stmt_has_nested_mutable_scope(body, depth)
        }
        Stmt::TaskGroup { children, .. } => children
            .iter()
            .any(|child| stmt_has_nested_mutable_scope(child, depth)),
        Stmt::Let { .. }
        | Stmt::Assign { .. }
        | Stmt::Expr(_, _)
        | Stmt::ArrayStore { .. }
        | Stmt::Cancel { .. } => false,
    }
}

pub(crate) fn atom_has_regex_semantics(
    atom: &Atom,
    requires_expr: &Expr,
    ensures_expr: &Expr,
    body_stmt: &Stmt,
) -> bool {
    expr_has_regex_semantics(requires_expr)
        || expr_has_regex_semantics(ensures_expr)
        || stmt_has_regex_semantics(body_stmt)
        || text_has_regex_semantics(&atom_contract_text(atom))
}

pub(crate) fn text_has_regex_semantics(text: &str) -> bool {
    let normalized = text.to_ascii_lowercase();
    normalized.contains("regex")
        || normalized.contains("regexp")
        || normalized.contains("match_regex(")
        || normalized.contains("re_match(")
}

pub(crate) fn atom_has_unbounded_array_access(
    atom: &Atom,
    requires_expr: &Expr,
    ensures_expr: &Expr,
    body_stmt: &Stmt,
) -> bool {
    let mut indexes = Vec::new();
    collect_array_index_names_from_expr(requires_expr, &mut indexes);
    collect_array_index_names_from_expr(ensures_expr, &mut indexes);
    collect_array_index_names_from_stmt(body_stmt, &mut indexes);

    for q in &atom.forall_constraints {
        let condition_expr = parse_expression(&q.condition);
        collect_array_index_names_from_expr(&condition_expr, &mut indexes);
        let normalized_start = normalize_logical_text(&q.start);
        if normalized_start == "0" {
            indexes.retain(|idx| idx != &q.var);
        }
    }

    let contract_text = normalize_logical_text(&atom_contract_text(atom));
    indexes
        .iter()
        .any(|index| !index_has_explicit_bounds(index, &contract_text))
}

pub(crate) fn collect_array_index_names_from_stmt(stmt: &Stmt, indexes: &mut Vec<String>) {
    match stmt {
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            collect_array_index_names_from_expr(value, indexes);
        }
        Stmt::Expr(value, _) => collect_array_index_names_from_expr(value, indexes),
        Stmt::ArrayStore { index, value, .. } => {
            collect_array_index_name(index, indexes);
            collect_array_index_names_from_expr(value, indexes);
        }
        Stmt::Block(stmts, _) => {
            for stmt in stmts {
                collect_array_index_names_from_stmt(stmt, indexes);
            }
        }
        Stmt::While {
            cond,
            invariant,
            body,
            ..
        } => {
            collect_array_index_names_from_expr(cond, indexes);
            collect_array_index_names_from_expr(invariant, indexes);
            collect_array_index_names_from_stmt(body, indexes);
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
            collect_array_index_names_from_stmt(body, indexes);
        }
        Stmt::TaskGroup { children, .. } => {
            for child in children {
                collect_array_index_names_from_stmt(child, indexes);
            }
        }
        Stmt::Cancel { .. } => {}
    }
}

pub(crate) fn collect_array_index_names_from_expr(expr: &Expr, indexes: &mut Vec<String>) {
    match expr {
        Expr::ArrayAccess(_, index) => {
            collect_array_index_name(index, indexes);
            collect_array_index_names_from_expr(index, indexes);
        }
        Expr::BinaryOp(left, _, right) => {
            collect_array_index_names_from_expr(left, indexes);
            collect_array_index_names_from_expr(right, indexes);
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_array_index_names_from_expr(cond, indexes);
            collect_array_index_names_from_stmt(then_branch, indexes);
            collect_array_index_names_from_stmt(else_branch, indexes);
        }
        Expr::Call(_, args) => {
            for arg in args {
                collect_array_index_names_from_expr(arg, indexes);
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, field_expr) in fields {
                collect_array_index_names_from_expr(field_expr, indexes);
            }
        }
        Expr::FieldAccess(base, _) => collect_array_index_names_from_expr(base, indexes),
        Expr::Match { target, arms } => {
            collect_array_index_names_from_expr(target, indexes);
            for arm in arms {
                collect_array_index_names_from_stmt(&arm.body, indexes);
            }
        }
        Expr::Async { body } | Expr::Lambda { body, .. } => {
            collect_array_index_names_from_stmt(body, indexes);
        }
        Expr::Await { expr } => collect_array_index_names_from_expr(expr, indexes),
        Expr::CallRef { callee, args } => {
            collect_array_index_names_from_expr(callee, indexes);
            for arg in args {
                collect_array_index_names_from_expr(arg, indexes);
            }
        }
        Expr::Perform { args, .. } => {
            for arg in args {
                collect_array_index_names_from_expr(arg, indexes);
            }
        }
        Expr::ChanSend { channel, value } => {
            collect_array_index_names_from_expr(channel, indexes);
            collect_array_index_names_from_expr(value, indexes);
        }
        Expr::ChanRecv { channel } => collect_array_index_names_from_expr(channel, indexes),
        Expr::Number(_)
        | Expr::Float(_)
        | Expr::StringLit(_)
        | Expr::Variable(_)
        | Expr::AtomRef { .. } => {}
    }
}

pub(crate) fn collect_array_index_name(index: &Expr, indexes: &mut Vec<String>) {
    match index {
        Expr::Variable(name) if !indexes.iter().any(|existing| existing == name) => {
            indexes.push(name.clone());
        }
        Expr::BinaryOp(left, _, right) => {
            collect_array_index_name(left, indexes);
            collect_array_index_name(right, indexes);
        }
        _ => {}
    }
}

pub(crate) fn normalize_logical_text(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_whitespace()).collect()
}

pub(crate) fn index_has_explicit_bounds(index: &str, normalized_text: &str) -> bool {
    let has_lower_bound = normalized_text.contains(&format!("{index}>=0"))
        || normalized_text.contains(&format!("0<={index}"));
    let has_upper_bound = normalized_text.contains(&format!("{index}<"))
        || normalized_text.contains(&format!(">{index}"));
    has_lower_bound && has_upper_bound
}

pub(crate) fn atom_uses_complex_temporal_effect(atom: &Atom, module_env: &ModuleEnv) -> bool {
    atom.effects.iter().any(|effect| {
        module_env
            .effect_defs
            .get(&effect.name)
            .or_else(|| module_env.effects.get(&effect.name))
            .is_some_and(|def| def.states.len() > 4 || def.transitions.len() > 8)
    })
}

pub(crate) fn atom_uses_temporal_effect(atom: &Atom, module_env: &ModuleEnv) -> bool {
    atom.effects.iter().any(|effect| {
        module_env
            .effect_defs
            .get(&effect.name)
            .or_else(|| module_env.effects.get(&effect.name))
            .is_some_and(|def| !def.states.is_empty() || !def.transitions.is_empty())
    })
}

pub(crate) fn expr_has_array_access(expr: &Expr) -> bool {
    match expr {
        Expr::ArrayAccess(_, _) => true,
        Expr::BinaryOp(left, _, right) => {
            expr_has_array_access(left) || expr_has_array_access(right)
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            expr_has_array_access(cond)
                || stmt_has_array_access(then_branch)
                || stmt_has_array_access(else_branch)
        }
        Expr::Call(_, args) => args.iter().any(expr_has_array_access),
        Expr::StructInit { fields, .. } => fields
            .iter()
            .any(|(_, field_expr)| expr_has_array_access(field_expr)),
        Expr::FieldAccess(base, _) => expr_has_array_access(base),
        Expr::Match { target, arms } => {
            expr_has_array_access(target) || arms.iter().any(|arm| stmt_has_array_access(&arm.body))
        }
        Expr::Async { body } | Expr::Lambda { body, .. } => stmt_has_array_access(body),
        Expr::Await { expr } => expr_has_array_access(expr),
        Expr::CallRef { callee, args } => {
            expr_has_array_access(callee) || args.iter().any(expr_has_array_access)
        }
        Expr::Perform { args, .. } => args.iter().any(expr_has_array_access),
        Expr::ChanSend { channel, value } => {
            expr_has_array_access(channel) || expr_has_array_access(value)
        }
        Expr::ChanRecv { channel } => expr_has_array_access(channel),
        Expr::Number(_)
        | Expr::Float(_)
        | Expr::StringLit(_)
        | Expr::Variable(_)
        | Expr::AtomRef { .. } => false,
    }
}

pub(crate) fn stmt_has_array_access(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => expr_has_array_access(value),
        Stmt::Expr(value, _) => expr_has_array_access(value),
        Stmt::ArrayStore { .. } => true,
        Stmt::Block(stmts, _) => stmts.iter().any(stmt_has_array_access),
        Stmt::While {
            cond,
            invariant,
            body,
            ..
        } => {
            expr_has_array_access(cond)
                || expr_has_array_access(invariant)
                || stmt_has_array_access(body)
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => stmt_has_array_access(body),
        Stmt::TaskGroup { children, .. } => children.iter().any(stmt_has_array_access),
        Stmt::Cancel { .. } => false,
    }
}

pub(crate) fn expr_has_linear_arithmetic(expr: &Expr) -> bool {
    match expr {
        Expr::BinaryOp(
            _,
            Op::Add | Op::Sub | Op::Eq | Op::Neq | Op::Gt | Op::Lt | Op::Ge | Op::Le,
            _,
        ) => true,
        Expr::BinaryOp(left, Op::Mul, right) => {
            expr_is_numeric_literal(left)
                || expr_is_numeric_literal(right)
                || expr_has_linear_arithmetic(left)
                || expr_has_linear_arithmetic(right)
        }
        Expr::BinaryOp(left, _, right) => {
            expr_has_linear_arithmetic(left) || expr_has_linear_arithmetic(right)
        }
        Expr::ArrayAccess(_, idx) => expr_has_linear_arithmetic(idx),
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            expr_has_linear_arithmetic(cond)
                || stmt_has_linear_arithmetic(then_branch)
                || stmt_has_linear_arithmetic(else_branch)
        }
        Expr::Call(_, args) => args.iter().any(expr_has_linear_arithmetic),
        Expr::StructInit { fields, .. } => fields
            .iter()
            .any(|(_, field_expr)| expr_has_linear_arithmetic(field_expr)),
        Expr::FieldAccess(base, _) => expr_has_linear_arithmetic(base),
        Expr::Match { target, arms } => {
            expr_has_linear_arithmetic(target)
                || arms.iter().any(|arm| stmt_has_linear_arithmetic(&arm.body))
        }
        Expr::Async { body } | Expr::Lambda { body, .. } => stmt_has_linear_arithmetic(body),
        Expr::Await { expr } => expr_has_linear_arithmetic(expr),
        Expr::CallRef { callee, args } => {
            expr_has_linear_arithmetic(callee) || args.iter().any(expr_has_linear_arithmetic)
        }
        Expr::Perform { args, .. } => args.iter().any(expr_has_linear_arithmetic),
        Expr::ChanSend { channel, value } => {
            expr_has_linear_arithmetic(channel) || expr_has_linear_arithmetic(value)
        }
        Expr::ChanRecv { channel } => expr_has_linear_arithmetic(channel),
        Expr::Number(_)
        | Expr::Float(_)
        | Expr::StringLit(_)
        | Expr::Variable(_)
        | Expr::AtomRef { .. } => false,
    }
}

pub(crate) fn stmt_has_linear_arithmetic(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => expr_has_linear_arithmetic(value),
        Stmt::Expr(value, _) => expr_has_linear_arithmetic(value),
        Stmt::ArrayStore { index, value, .. } => {
            expr_has_linear_arithmetic(index) || expr_has_linear_arithmetic(value)
        }
        Stmt::Block(stmts, _) => stmts.iter().any(stmt_has_linear_arithmetic),
        Stmt::While {
            cond,
            invariant,
            body,
            ..
        } => {
            expr_has_linear_arithmetic(cond)
                || expr_has_linear_arithmetic(invariant)
                || stmt_has_linear_arithmetic(body)
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => stmt_has_linear_arithmetic(body),
        Stmt::TaskGroup { children, .. } => children.iter().any(stmt_has_linear_arithmetic),
        Stmt::Cancel { .. } => false,
    }
}

pub(crate) fn expr_has_nonlinear_arithmetic(expr: &Expr) -> bool {
    match expr {
        Expr::BinaryOp(left, Op::Mul, right) => {
            (!expr_is_numeric_literal(left) && !expr_is_numeric_literal(right))
                || expr_has_nonlinear_arithmetic(left)
                || expr_has_nonlinear_arithmetic(right)
        }
        Expr::BinaryOp(left, Op::Div, right) => {
            !expr_is_numeric_literal(right)
                || expr_has_nonlinear_arithmetic(left)
                || expr_has_nonlinear_arithmetic(right)
        }
        Expr::BinaryOp(left, _, right) => {
            expr_has_nonlinear_arithmetic(left) || expr_has_nonlinear_arithmetic(right)
        }
        Expr::ArrayAccess(_, idx) => expr_has_nonlinear_arithmetic(idx),
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            expr_has_nonlinear_arithmetic(cond)
                || stmt_has_nonlinear_arithmetic(then_branch)
                || stmt_has_nonlinear_arithmetic(else_branch)
        }
        Expr::Call(_, args) => args.iter().any(expr_has_nonlinear_arithmetic),
        Expr::StructInit { fields, .. } => fields
            .iter()
            .any(|(_, field_expr)| expr_has_nonlinear_arithmetic(field_expr)),
        Expr::FieldAccess(base, _) => expr_has_nonlinear_arithmetic(base),
        Expr::Match { target, arms } => {
            expr_has_nonlinear_arithmetic(target)
                || arms
                    .iter()
                    .any(|arm| stmt_has_nonlinear_arithmetic(&arm.body))
        }
        Expr::Async { body } | Expr::Lambda { body, .. } => stmt_has_nonlinear_arithmetic(body),
        Expr::Await { expr } => expr_has_nonlinear_arithmetic(expr),
        Expr::CallRef { callee, args } => {
            expr_has_nonlinear_arithmetic(callee) || args.iter().any(expr_has_nonlinear_arithmetic)
        }
        Expr::Perform { args, .. } => args.iter().any(expr_has_nonlinear_arithmetic),
        Expr::ChanSend { channel, value } => {
            expr_has_nonlinear_arithmetic(channel) || expr_has_nonlinear_arithmetic(value)
        }
        Expr::ChanRecv { channel } => expr_has_nonlinear_arithmetic(channel),
        Expr::Number(_)
        | Expr::Float(_)
        | Expr::StringLit(_)
        | Expr::Variable(_)
        | Expr::AtomRef { .. } => false,
    }
}

pub(crate) fn stmt_has_nonlinear_arithmetic(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            expr_has_nonlinear_arithmetic(value)
        }
        Stmt::Expr(value, _) => expr_has_nonlinear_arithmetic(value),
        Stmt::ArrayStore { index, value, .. } => {
            expr_has_nonlinear_arithmetic(index) || expr_has_nonlinear_arithmetic(value)
        }
        Stmt::Block(stmts, _) => stmts.iter().any(stmt_has_nonlinear_arithmetic),
        Stmt::While {
            cond,
            invariant,
            body,
            ..
        } => {
            expr_has_nonlinear_arithmetic(cond)
                || expr_has_nonlinear_arithmetic(invariant)
                || stmt_has_nonlinear_arithmetic(body)
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => stmt_has_nonlinear_arithmetic(body),
        Stmt::TaskGroup { children, .. } => children.iter().any(stmt_has_nonlinear_arithmetic),
        Stmt::Cancel { .. } => false,
    }
}

pub(crate) fn expr_has_regex_semantics(expr: &Expr) -> bool {
    match expr {
        Expr::Call(name, args) => {
            text_has_regex_semantics(name) || args.iter().any(expr_has_regex_semantics)
        }
        Expr::BinaryOp(left, _, right) => {
            expr_has_regex_semantics(left) || expr_has_regex_semantics(right)
        }
        Expr::ArrayAccess(_, idx) => expr_has_regex_semantics(idx),
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            expr_has_regex_semantics(cond)
                || stmt_has_regex_semantics(then_branch)
                || stmt_has_regex_semantics(else_branch)
        }
        Expr::StructInit { fields, .. } => fields
            .iter()
            .any(|(_, field_expr)| expr_has_regex_semantics(field_expr)),
        Expr::FieldAccess(base, field) => {
            text_has_regex_semantics(field) || expr_has_regex_semantics(base)
        }
        Expr::Match { target, arms } => {
            expr_has_regex_semantics(target)
                || arms.iter().any(|arm| stmt_has_regex_semantics(&arm.body))
        }
        Expr::Async { body } | Expr::Lambda { body, .. } => stmt_has_regex_semantics(body),
        Expr::Await { expr } => expr_has_regex_semantics(expr),
        Expr::CallRef { callee, args } => {
            expr_has_regex_semantics(callee) || args.iter().any(expr_has_regex_semantics)
        }
        Expr::Perform { args, .. } => args.iter().any(expr_has_regex_semantics),
        Expr::ChanSend { channel, value } => {
            expr_has_regex_semantics(channel) || expr_has_regex_semantics(value)
        }
        Expr::ChanRecv { channel } => expr_has_regex_semantics(channel),
        Expr::StringLit(value) => text_has_regex_semantics(value),
        Expr::Number(_) | Expr::Float(_) | Expr::Variable(_) | Expr::AtomRef { .. } => false,
    }
}

pub(crate) fn stmt_has_regex_semantics(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => expr_has_regex_semantics(value),
        Stmt::Expr(value, _) => expr_has_regex_semantics(value),
        Stmt::ArrayStore { index, value, .. } => {
            expr_has_regex_semantics(index) || expr_has_regex_semantics(value)
        }
        Stmt::Block(stmts, _) => stmts.iter().any(stmt_has_regex_semantics),
        Stmt::While {
            cond,
            invariant,
            body,
            ..
        } => {
            expr_has_regex_semantics(cond)
                || expr_has_regex_semantics(invariant)
                || stmt_has_regex_semantics(body)
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => stmt_has_regex_semantics(body),
        Stmt::TaskGroup { children, .. } => children.iter().any(stmt_has_regex_semantics),
        Stmt::Cancel { .. } => false,
    }
}

pub(crate) fn expr_has_inductive_shape(expr: &Expr) -> bool {
    match expr {
        Expr::Match { .. } => true,
        Expr::BinaryOp(left, _, right) => {
            expr_has_inductive_shape(left) || expr_has_inductive_shape(right)
        }
        Expr::ArrayAccess(_, idx) => expr_has_inductive_shape(idx),
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            expr_has_inductive_shape(cond)
                || stmt_has_inductive_shape(then_branch)
                || stmt_has_inductive_shape(else_branch)
        }
        Expr::Call(_, args) => args.iter().any(expr_has_inductive_shape),
        Expr::StructInit { fields, .. } => fields
            .iter()
            .any(|(_, field_expr)| expr_has_inductive_shape(field_expr)),
        Expr::FieldAccess(base, _) => expr_has_inductive_shape(base),
        Expr::Async { body } | Expr::Lambda { body, .. } => stmt_has_inductive_shape(body),
        Expr::Await { expr } => expr_has_inductive_shape(expr),
        Expr::CallRef { callee, args } => {
            expr_has_inductive_shape(callee) || args.iter().any(expr_has_inductive_shape)
        }
        Expr::Perform { args, .. } => args.iter().any(expr_has_inductive_shape),
        Expr::ChanSend { channel, value } => {
            expr_has_inductive_shape(channel) || expr_has_inductive_shape(value)
        }
        Expr::ChanRecv { channel } => expr_has_inductive_shape(channel),
        Expr::Number(_)
        | Expr::Float(_)
        | Expr::StringLit(_)
        | Expr::Variable(_)
        | Expr::AtomRef { .. } => false,
    }
}

pub(crate) fn stmt_has_inductive_shape(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => expr_has_inductive_shape(value),
        Stmt::Expr(value, _) => expr_has_inductive_shape(value),
        Stmt::ArrayStore { index, value, .. } => {
            expr_has_inductive_shape(index) || expr_has_inductive_shape(value)
        }
        Stmt::Block(stmts, _) => stmts.iter().any(stmt_has_inductive_shape),
        Stmt::While {
            cond,
            invariant,
            body,
            ..
        } => {
            expr_has_inductive_shape(cond)
                || expr_has_inductive_shape(invariant)
                || stmt_has_inductive_shape(body)
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => stmt_has_inductive_shape(body),
        Stmt::TaskGroup { children, .. } => children.iter().any(stmt_has_inductive_shape),
        Stmt::Cancel { .. } => false,
    }
}

pub(crate) fn stmt_has_while(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::While { .. } => true,
        Stmt::Block(stmts, _) => stmts.iter().any(stmt_has_while),
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => stmt_has_while(body),
        Stmt::TaskGroup { children, .. } => children.iter().any(stmt_has_while),
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => expr_contains_while(value),
        Stmt::Expr(value, _) => expr_contains_while(value),
        Stmt::ArrayStore { index, value, .. } => {
            expr_contains_while(index) || expr_contains_while(value)
        }
        Stmt::Cancel { .. } => false,
    }
}

pub(crate) fn expr_contains_while(expr: &Expr) -> bool {
    match expr {
        Expr::IfThenElse {
            then_branch,
            else_branch,
            ..
        } => stmt_has_while(then_branch) || stmt_has_while(else_branch),
        Expr::Match { arms, .. } => arms.iter().any(|arm| stmt_has_while(&arm.body)),
        Expr::Async { body } | Expr::Lambda { body, .. } => stmt_has_while(body),
        Expr::BinaryOp(left, _, right) => expr_contains_while(left) || expr_contains_while(right),
        Expr::ArrayAccess(_, idx) => expr_contains_while(idx),
        Expr::Call(_, args) => args.iter().any(expr_contains_while),
        Expr::StructInit { fields, .. } => fields
            .iter()
            .any(|(_, field_expr)| expr_contains_while(field_expr)),
        Expr::FieldAccess(base, _) => expr_contains_while(base),
        Expr::Await { expr } => expr_contains_while(expr),
        Expr::CallRef { callee, args } => {
            expr_contains_while(callee) || args.iter().any(expr_contains_while)
        }
        Expr::Perform { args, .. } => args.iter().any(expr_contains_while),
        Expr::ChanSend { channel, value } => {
            expr_contains_while(channel) || expr_contains_while(value)
        }
        Expr::ChanRecv { channel } => expr_contains_while(channel),
        Expr::Number(_)
        | Expr::Float(_)
        | Expr::StringLit(_)
        | Expr::Variable(_)
        | Expr::AtomRef { .. } => false,
    }
}

pub(crate) fn expr_is_numeric_literal(expr: &Expr) -> bool {
    matches!(expr, Expr::Number(_) | Expr::Float(_))
}

/// Structured representation of a Z3 tracking label.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StructuredLabel {
    pub constraint_type: String,
    pub param: Option<String>,
    pub type_name: Option<String>,
    pub field: Option<String>,
    pub description: String,
}

/// Parse a Z3 tracking label into a StructuredLabel.
/// Returns None for unrecognized labels (internal bookkeeping variables).
pub(crate) fn parse_tracking_label(label: &str) -> Option<StructuredLabel> {
    if label == "track_requires" {
        return Some(StructuredLabel {
            constraint_type: "requires".to_string(),
            param: None,
            type_name: None,
            field: None,
            description: "Precondition (requires) / 前提条件 (requires)".to_string(),
        });
    }
    if let Some(rest) = label.strip_prefix("track_refined_type_") {
        // format: track_refined_type_{var}::{type}
        if let Some(idx) = rest.find("::") {
            let var = &rest[..idx];
            let tn = &rest[idx + 2..];
            return Some(StructuredLabel {
                constraint_type: "refined_type".to_string(),
                param: Some(var.to_string()),
                type_name: Some(tn.to_string()),
                field: None,
                description: format!(
                    "Refined type constraint: {} ({}) / 精緻型制約: {} ({})",
                    var, tn, var, tn
                ),
            });
        }
    }
    if let Some(rest) = label.strip_prefix("track_struct_field_") {
        // format: track_struct_field_{param}::{field}
        if let Some(idx) = rest.find("::") {
            let param = &rest[..idx];
            let fld = &rest[idx + 2..];
            return Some(StructuredLabel {
                constraint_type: "struct_field".to_string(),
                param: Some(param.to_string()),
                type_name: None,
                field: Some(fld.to_string()),
                description: format!(
                    "Struct field constraint: {}.{} / 構造体フィールド制約: {}.{}",
                    param, fld, param, fld
                ),
            });
        }
    }
    if let Some(rest) = label.strip_prefix("track_quantifier_") {
        return Some(StructuredLabel {
            constraint_type: "quantifier".to_string(),
            param: None,
            type_name: None,
            field: None,
            description: format!("Quantifier constraint #{} / 量子化制約 #{}", rest, rest),
        });
    }
    if let Some(rest) = label.strip_prefix("track_u64_nonneg_") {
        return Some(StructuredLabel {
            constraint_type: "u64_nonneg".to_string(),
            param: Some(rest.to_string()),
            type_name: None,
            field: None,
            description: format!(
                "Non-negative constraint: {} (u64) / 非負制約: {} (u64)",
                rest, rest
            ),
        });
    }
    None
}

/// Encode an effect state name as an integer for Z3 Int Sort constraints.
/// Returns the index of the state in the state machine's states list, or -1 if not found.
pub(crate) fn encode_effect_state(
    state_machine: &crate::mir_analysis::EffectStateMachine,
    state_name: &str,
) -> i64 {
    state_machine
        .states
        .iter()
        .position(|s| s == state_name)
        .map(|i| i as i64)
        .unwrap_or(-1)
}

/// Build semantic feedback JSON for contradiction (unsat) detection with unsat core info.
pub fn build_contradiction_feedback(
    atom_name: &str,
    conflicting_constraints: &[String],
    raw_labels: &[String],
    structured_labels: &[StructuredLabel],
    minimal_core: Option<&[String]>,
) -> serde_json::Value {
    let explanation = if conflicting_constraints.is_empty() {
        "The constraints are mutually contradictory, but the specific conflicting set could not be determined. \
         (制約が相互に矛盾していますが、具体的な矛盾セットを特定できませんでした)".to_string()
    } else {
        format!(
            "The following constraints are mutually contradictory: {} \
             (以下の制約が相互に矛盾しています: {})",
            conflicting_constraints.join(", "),
            conflicting_constraints.join(", ")
        )
    };

    let mut feedback = json!({
        "failure_type": FAILURE_INVARIANT_VIOLATED,
        "atom": atom_name,
        "conflicting_constraints": conflicting_constraints,
        "raw_unsat_core": raw_labels,
        "structured_unsat_core": structured_labels,
        "explanation": explanation,
        "suggestion": suggestion_for_failure_type(FAILURE_INVARIANT_VIOLATED)
    });

    if let Some(minimal_core) = minimal_core {
        feedback["minimal_unsat_core"] = json!(minimal_core);
        feedback["minimal_core_size"] = json!(minimal_core.len());
        feedback["total_core_size"] = json!(raw_labels.len());
        feedback["reduction_ratio"] = json!(if raw_labels.is_empty() {
            0.0
        } else {
            minimal_core.len() as f64 / raw_labels.len() as f64
        });

        if minimal_core.is_empty() {
            feedback["suggestion"] = json!(suggestion_for_failure_type(FAILURE_INVARIANT_VIOLATED));
        } else if minimal_core.len() == 1 {
            feedback["suggestion"] = json!(format!(
                "Single constraint causing contradiction: '{}'. Consider relaxing or removing it.",
                minimal_core[0]
            ));
        } else {
            feedback["suggestion"] = json!(format!(
                "Minimal conflicting constraints: [{}]. Consider relaxing one of these.",
                minimal_core.join(", ")
            ));
        }
    }

    feedback
}

pub(crate) const MINIMAL_UNSAT_CORE_PROBE_TIMEOUT_MS: u32 = 1000;
pub(crate) const MAX_MINIMAL_UNSAT_CORE_PROBES: usize = 512;

pub(crate) struct MinimalUnsatCoreProbe<'ctx> {
    solver: Solver<'ctx>,
    context: &'ctx Context,
    probes_used: usize,
}

impl<'ctx> MinimalUnsatCoreProbe<'ctx> {
    fn new(source_solver: &Solver<'ctx>, context: &'ctx Context) -> Self {
        let solver = Solver::new(context);
        let mut params = z3::Params::new(context);
        params.set_u32("timeout", MINIMAL_UNSAT_CORE_PROBE_TIMEOUT_MS);
        solver.set_params(&params);

        for assertion in source_solver.get_assertions() {
            solver.assert(&assertion);
        }

        Self {
            solver,
            context,
            probes_used: 0,
        }
    }

    fn has_budget(&self) -> bool {
        self.probes_used < MAX_MINIMAL_UNSAT_CORE_PROBES
    }

    fn is_unsat_with_labels(&mut self, labels: &[String]) -> bool {
        if labels.is_empty() || !self.has_budget() {
            return false;
        }

        self.probes_used += 1;
        let assumptions: Vec<Bool> = labels
            .iter()
            .map(|label| Bool::new_const(self.context, normalize_tracking_label(label)))
            .collect();

        self.solver.check_assumptions(&assumptions) == SatResult::Unsat
    }
}

/// Extract a deletion-minimal unsat core from tracked Z3 constraint labels.
///
/// Given a set of constraints that are unsatisfiable together, find a subset
/// where removing any single remaining label makes that subset satisfiable.
/// This helps users understand which specific constraints are conflicting and
/// may need to be relaxed.
///
/// # Arguments
/// * `solver` - Z3 solver instance containing tracked assertions
/// * `all_labels` - Constraint labels to test
/// * `context` - Z3 context for creating label assumptions
///
/// # Returns
/// * `Vec<String>` - Minimal set of labels that cause unsatisfiability
pub fn extract_minimal_unsat_core<'ctx>(
    solver: &Solver<'ctx>,
    all_labels: &[String],
    context: &'ctx Context,
) -> Vec<String> {
    if all_labels.is_empty() {
        return vec![];
    }
    if all_labels.len() == 1 {
        return all_labels.to_vec();
    }

    let mut probe = MinimalUnsatCoreProbe::new(solver, context);
    let mut minimal = all_labels.to_vec();
    let mut chunk_size = minimal.len() / 2;

    while chunk_size > 0 && minimal.len() > 1 && probe.has_budget() {
        let mut removed_chunk = false;
        let mut start = 0;

        while start < minimal.len() && probe.has_budget() {
            let end = (start + chunk_size).min(minimal.len());
            let test_set: Vec<String> = minimal
                .iter()
                .enumerate()
                .filter(|(idx, _)| *idx < start || *idx >= end)
                .map(|(_, label)| label.clone())
                .collect();

            if !test_set.is_empty() && probe.is_unsat_with_labels(&test_set) {
                minimal = test_set;
                chunk_size = (minimal.len() / 2).max(1);
                removed_chunk = true;
                break;
            }

            start += chunk_size;
        }

        if !removed_chunk {
            chunk_size /= 2;
        }
    }

    extract_minimal_unsat_core_linear_with_probe(&mut probe, &minimal)
}

/// Extract a deletion-minimal unsat core using a linear greedy pass.
pub fn extract_minimal_unsat_core_linear<'ctx>(
    solver: &Solver<'ctx>,
    all_labels: &[String],
    context: &'ctx Context,
) -> Vec<String> {
    if all_labels.is_empty() {
        return vec![];
    }

    let mut probe = MinimalUnsatCoreProbe::new(solver, context);
    extract_minimal_unsat_core_linear_with_probe(&mut probe, all_labels)
}

pub(crate) fn extract_minimal_unsat_core_linear_with_probe<'ctx>(
    probe: &mut MinimalUnsatCoreProbe<'ctx>,
    all_labels: &[String],
) -> Vec<String> {
    let mut minimal = all_labels.to_vec();
    let mut i = 0;

    while i < minimal.len() && minimal.len() > 1 && probe.has_budget() {
        let test_set: Vec<String> = minimal
            .iter()
            .enumerate()
            .filter(|(idx, _)| *idx != i)
            .map(|(_, label)| label.clone())
            .collect();

        if probe.is_unsat_with_labels(&test_set) {
            minimal = test_set;
        } else {
            i += 1;
        }
    }

    minimal
}

pub(crate) fn normalize_tracking_label(label: &str) -> String {
    label
        .strip_prefix('|')
        .and_then(|without_prefix| without_prefix.strip_suffix('|'))
        .unwrap_or(label)
        .to_string()
}

/// Build contradiction feedback with minimal unsat core information.
pub fn build_contradiction_feedback_with_minimal_core(
    atom_name: &str,
    conflicting_constraints: &[String],
    raw_unsat_core: &[String],
    structured_labels: &[StructuredLabel],
    minimal_core: &[String],
) -> serde_json::Value {
    build_contradiction_feedback(
        atom_name,
        conflicting_constraints,
        raw_unsat_core,
        structured_labels,
        Some(minimal_core),
    )
}
