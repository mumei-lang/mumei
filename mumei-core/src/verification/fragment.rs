use super::module_env::*;
use super::nlae_reporter::*;
use super::types::*;
use super::*;

pub fn detect_logic_fragment_tags(atom: &Atom, module_env: &ModuleEnv) -> Vec<String> {
    let mut tags = Vec::new();

    for param in &atom.params {
        // P10-C: finite, non-recursive, scalar-payload enums lower to native
        // Z3 datatypes and stay decidable — only enums outside that fragment
        // (recursive or generic) keep the Int-tag encoding and escalate to
        // `inductive_data_type` for Lean 4.
        if param.type_name.as_ref().is_some_and(|type_name| {
            module_env.enums.get(type_name).is_some_and(|e| {
                !crate::verification::support::datatype::is_finite_adt(e, module_env)
            })
        }) {
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
    // Declared enum types visible to the classifier: parameters seed the
    // environment and `let` bindings extend it sequentially, so a bare
    // `match r { Ok(v) => … }` on a `Res2` can disambiguate `Ok` from a
    // same-named variant of another enum (e.g. prelude `Result`) — the same
    // declared-type rule the verifier uses. Without the hint the owner is
    // ambiguous and the atom was tagged `inductive_data_type` even though
    // the finite-ADT path verifies natively.
    let mut enum_names: std::collections::HashMap<String, String> = atom
        .params
        .iter()
        .filter_map(|p| {
            let base =
                crate::verification::support::datatype::type_name_base(p.type_name.as_deref()?);
            module_env
                .get_enum(base)
                .map(|e| (p.name.clone(), e.name.clone()))
        })
        .collect();

    if expr_has_inductive_shape(&requires_expr, module_env, &enum_names)
        || expr_has_inductive_shape(&ensures_expr, module_env, &enum_names)
        || stmt_has_inductive_shape(&body_stmt, module_env, &mut enum_names)
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
    let mut struct_invariant_nonlinear = false;
    for invariant in atom_struct_invariants(atom, module_env) {
        if expr_has_nonlinear_arithmetic(&parse_expression(&invariant))
            || text_has_nonlinear_arithmetic_marker(&invariant)
        {
            push_unique_tag(&mut tags, "nonlinear_arithmetic");
            struct_invariant_nonlinear = true;
        }
    }
    if tags.iter().any(|tag| tag == "nonlinear_arithmetic")
        && atom.invariant.is_none()
        && !struct_invariant_nonlinear
        && atom.forall_constraints.is_empty()
        && bounded_low_degree_nonlinear_profile(atom).is_some()
    {
        push_unique_tag(&mut tags, BOUNDED_NONLINEAR_TAG);
    }

    if stmt_has_while(&body_stmt) {
        push_unique_tag(&mut tags, "recursive_invariant");
    }
    if atom_has_unbounded_array_access(atom, &requires_expr, &ensures_expr, &body_stmt) {
        push_unique_tag(&mut tags, "array_without_bounds");
    }

    let has_forall = atom.forall_constraints.iter().any(|q| {
        q.q_type == QuantifierType::ForAll
            || q.condition.contains("forall(")
            || q.start.contains("forall(")
            || q.end.contains("forall(")
    }) || atom.requires.contains("forall(")
        || atom.ensures.contains("forall(");
    let has_exists = atom.forall_constraints.iter().any(|q| {
        q.q_type == QuantifierType::Exists
            || q.condition.contains("exists(")
            || q.start.contains("exists(")
            || q.end.contains("exists(")
    }) || atom.requires.contains("exists(")
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
    if atom_uses_bitvector_semantics(atom, &requires_expr, &ensures_expr, &body_stmt)
        || atom_struct_invariants(atom, module_env)
            .iter()
            .any(|invariant| expr_has_bitwise_op(&parse_expression(invariant)))
    {
        push_unique_tag(&mut tags, "bitvector_semantics");
    }

    tags
}

/// The values `semantics:` accepts. `bitvec` is the only semantic mode an atom
/// can opt into today; the default (no `semantics:` clause) is the unbounded
/// `Int` encoding.
pub const SUPPORTED_SEMANTICS: [&str; 1] = ["bitvec"];

/// The `semantics:` value of this atom when it is not one the verifier knows.
///
/// An unrecognized value must not be ignored: silently verifying `semantics:
/// bitvector;` in the default mode would certify unbounded arithmetic for an
/// atom whose author asked for wrapping semantics, so a typo is an error
/// instead of a downgrade.
pub fn unsupported_semantics_value(atom: &Atom) -> Option<&str> {
    atom.spec_metadata
        .get("semantics")
        .map(String::as_str)
        .filter(|value| !SUPPORTED_SEMANTICS.contains(value))
}

/// True when this atom has to be verified with the bit-vector encoding
/// (`i64` as `BV(64)`), i.e. the default unbounded `Int` encoding cannot give
/// its contract its intended meaning.
///
/// That is the case when the atom uses a bitwise operator — the `Int` encoding
/// rejects `&`/`|`/`^`/`<<`/`>>` rather than approximating them — or when it
/// opts in explicitly with `semantics: bitvec;`, for contracts that depend on
/// two's complement wrapping without naming a bitwise operator. Every other
/// atom keeps the default `Int` encoding, so its proof certificate is
/// unchanged.
pub fn atom_requires_bitvector_semantics(atom: &Atom) -> bool {
    if atom
        .spec_metadata
        .get("semantics")
        .is_some_and(|value| value == "bitvec")
    {
        return true;
    }
    atom_uses_bitvector_semantics(
        atom,
        &parse_expression(&atom.requires),
        &parse_expression(&atom.ensures),
        &parse_body_expr(&atom.body_expr),
    )
}

/// True when this atom, or one of the atoms it transitively calls, has to be
/// verified with the bit-vector encoding.
///
/// A caller imports its callees' `ensures` as facts, so the callee's semantic
/// mode has to hold in the caller's proof: an atom that calls a bit-vector atom
/// is verified in bit-vector mode too, rather than lowering a bitwise `ensures`
/// in the `Int` encoding that cannot express it.
pub fn atom_requires_bitvector_semantics_in_module(atom: &Atom, module_env: &ModuleEnv) -> bool {
    let mut pending = vec![atom.name.clone()];
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    if atom_requires_bitvector_semantics(atom) {
        return true;
    }

    while let Some(current) = pending.pop() {
        if !seen.insert(current.clone()) {
            continue;
        }
        let Some(current_atom) = module_env.atoms.get(&current) else {
            continue;
        };
        if current != atom.name && atom_requires_bitvector_semantics(current_atom) {
            return true;
        }
        if atom_struct_invariants(current_atom, module_env)
            .iter()
            .any(|invariant| expr_has_bitwise_op(&parse_expression(invariant)))
        {
            return true;
        }
        pending.extend(atom_callees(current_atom));
    }

    false
}

/// Every cross-field struct invariant this atom's verification context lowers:
/// the invariants of struct-typed parameters (assumed), of the struct return
/// type (imposed on `result`) and of every struct literal in the body
/// (checked). Like the atom's own clauses they are lowered with the atom's
/// sorts, so they take part in fragment classification and mode detection.
pub(crate) fn atom_struct_invariants(atom: &Atom, module_env: &ModuleEnv) -> Vec<String> {
    let mut struct_names: Vec<&str> = atom
        .params
        .iter()
        .filter_map(|param| param.type_name.as_deref())
        .chain(atom.return_type.as_deref())
        .filter(|type_name| module_env.structs.contains_key(*type_name))
        .collect();
    for name in module_env.structs.keys() {
        if body_has_struct_literal(&atom.body_expr, name) {
            struct_names.push(name.as_str());
        }
    }
    struct_names.sort_unstable();
    struct_names.dedup();
    struct_names
        .into_iter()
        .filter_map(|name| module_env.structs.get(name))
        .flat_map(|sdef| sdef.invariants.iter().cloned())
        .collect()
}

fn body_has_struct_literal(body: &str, struct_name: &str) -> bool {
    body.match_indices(struct_name).any(|(start, _)| {
        let preceded_by_ident = body[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        let rest = body[start + struct_name.len()..].trim_start();
        !preceded_by_ident && rest.starts_with('{')
    })
}

/// Every atom called from a clause that the atom's verification context
/// lowers: `requires`, `ensures`, the body, the atom invariant and the bounds
/// and condition of an extracted quantifier. All of those are lowered with the
/// same sorts, so a call to a bit-vector atom in any of them decides the
/// encoding.
fn atom_callees(atom: &Atom) -> Vec<String> {
    let mut callees = super::support::collect_callees_stmt(&parse_body_expr(&atom.body_expr));
    for clause in [&atom.requires, &atom.ensures] {
        callees.extend(super::support::collect_callees_expr(&parse_expression(
            clause,
        )));
    }
    if let Some(invariant) = atom.invariant.as_ref() {
        callees.extend(super::support::collect_callees_expr(&parse_expression(
            invariant,
        )));
    }
    for quantifier in &atom.forall_constraints {
        for clause in [&quantifier.start, &quantifier.end, &quantifier.condition] {
            callees.extend(super::support::collect_callees_expr(&parse_expression(
                clause,
            )));
        }
    }
    callees
}

/// True when a lowered expression source uses a bitwise operator, so it has to
/// be verified with the bit-vector encoding. Used for trait laws, whose text
/// only exists after the method bodies are substituted into them.
pub fn expression_requires_bitvector_semantics(source: &str) -> bool {
    expr_has_bitwise_op(&parse_expression(source))
}

/// True when any atom of the module needs the bit-vector encoding.
pub fn module_uses_bitvector_semantics(module_env: &ModuleEnv) -> bool {
    module_env
        .atoms
        .values()
        .any(atom_requires_bitvector_semantics)
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
///
/// `bitvector_semantics` is deliberately absent from `OUTSIDE_TAGS`: QF_BV is
/// decidable, so a bitwise obligation that is closed under bit-vectors stays a
/// Z3 goal. Only obligations that leave that fragment (e.g. also tagged
/// `nonlinear_arithmetic` for ring/polynomial overflow reasoning) become Lean
/// escalation candidates.
pub fn is_outside_decidable_fragment(tags: &[String]) -> bool {
    const OUTSIDE_TAGS: &[&str] = &[
        "nonlinear_arithmetic",
        "quantifier_alternation",
        "array_without_bounds",
        "inductive_data_type",
        "finite_field",
    ];
    let nlsat_first = is_nlsat_first_candidate(tags);
    tags.iter().any(|tag| {
        OUTSIDE_TAGS.contains(&tag.as_str()) && !(nlsat_first && tag == "nonlinear_arithmetic")
    })
}

/// Secondary tag recorded next to `nonlinear_arithmetic` when the obligation
/// is a bounded, low-degree polynomial (P10-D / C-2). Such atoms are tried by
/// Z3 with `nlsat` first and only demoted to Lean on `unknown` / timeout.
pub const BOUNDED_NONLINEAR_TAG: &str = "bounded_nonlinear_arithmetic";

/// Maximum total degree of any product term (`x * y` is 2, `x * x * x` is 3).
pub const NLSAT_FIRST_MAX_DEGREE: usize = 2;
/// Maximum number of distinct variables occurring in nonlinear terms.
pub const NLSAT_FIRST_MAX_VARIABLES: usize = 3;

/// Summary of the nonlinear terms of an atom that qualify for nlsat-first.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BoundedNonlinearProfile {
    pub max_degree: usize,
    pub variables: Vec<String>,
}

/// True when the tags mark a bounded low-degree nonlinear obligation that is
/// tried by Z3 (`nlsat`) before any Lean escalation.
pub fn is_nlsat_first_candidate(tags: &[String]) -> bool {
    tags.iter().any(|tag| tag == BOUNDED_NONLINEAR_TAG)
        && tags.iter().any(|tag| tag == "nonlinear_arithmetic")
        && !tags.iter().any(|tag| tag == "finite_field")
}

#[derive(Default, Clone, Copy)]
struct VarBounds {
    lower: Option<i64>,
    upper: Option<i64>,
}

impl VarBounds {
    fn is_closed(&self) -> bool {
        self.lower.is_some() && self.upper.is_some()
    }

    fn excludes_zero(&self) -> bool {
        self.is_closed() && (self.lower.unwrap_or(0) > 0 || self.upper.unwrap_or(0) < 0)
    }
}

fn collect_requires_bounds(expr: &Expr, bounds: &mut HashMap<String, VarBounds>) {
    match expr {
        Expr::BinaryOp(left, Op::And, right) => {
            collect_requires_bounds(left, bounds);
            collect_requires_bounds(right, bounds);
        }
        Expr::BinaryOp(left, op, right) => {
            let (var, literal, flipped) = match (left.as_ref(), right.as_ref()) {
                (Expr::Variable(v), Expr::Number(n)) => (v, *n, false),
                (Expr::Number(n), Expr::Variable(v)) => (v, *n, true),
                _ => return,
            };
            let entry = bounds.entry(var.clone()).or_default();
            // Normalize to `var <op> literal`.
            let op = if flipped {
                match op {
                    Op::Gt => Op::Lt,
                    Op::Lt => Op::Gt,
                    Op::Ge => Op::Le,
                    Op::Le => Op::Ge,
                    other => other.clone(),
                }
            } else {
                op.clone()
            };
            match op {
                Op::Ge => entry.lower = Some(entry.lower.map_or(literal, |l| l.max(literal))),
                Op::Gt => {
                    let lit = literal.saturating_add(1);
                    entry.lower = Some(entry.lower.map_or(lit, |l| l.max(lit)));
                }
                Op::Le => entry.upper = Some(entry.upper.map_or(literal, |u| u.min(literal))),
                Op::Lt => {
                    let lit = literal.saturating_sub(1);
                    entry.upper = Some(entry.upper.map_or(lit, |u| u.min(lit)));
                }
                Op::Eq => {
                    entry.lower = Some(literal);
                    entry.upper = Some(literal);
                }
                _ => {}
            }
        }
        _ => {}
    }
}

#[derive(Default)]
struct PolyShape {
    degree: usize,
    variables: Vec<String>,
    divisors: Vec<String>,
}

fn poly_shape(expr: &Expr) -> Option<PolyShape> {
    match expr {
        Expr::Number(_) | Expr::Float(_) => Some(PolyShape::default()),
        Expr::Variable(name) => Some(PolyShape {
            degree: 1,
            variables: vec![name.clone()],
            divisors: Vec::new(),
        }),
        Expr::BinaryOp(left, Op::Add, right) | Expr::BinaryOp(left, Op::Sub, right) => {
            let (l, r) = (poly_shape(left)?, poly_shape(right)?);
            Some(merge_shapes(l, r, false))
        }
        Expr::BinaryOp(left, Op::Mul, right) => {
            let (l, r) = (poly_shape(left)?, poly_shape(right)?);
            Some(merge_shapes(l, r, true))
        }
        Expr::BinaryOp(left, Op::Div, right) => {
            let l = poly_shape(left)?;
            match right.as_ref() {
                Expr::Number(_) => Some(l),
                Expr::Variable(name) => {
                    let mut shape = merge_shapes(
                        l,
                        PolyShape {
                            degree: 1,
                            variables: vec![name.clone()],
                            divisors: Vec::new(),
                        },
                        true,
                    );
                    shape.divisors.push(name.clone());
                    Some(shape)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn merge_shapes(l: PolyShape, r: PolyShape, product: bool) -> PolyShape {
    let mut variables = l.variables;
    for v in r.variables {
        if !variables.contains(&v) {
            variables.push(v);
        }
    }
    let mut divisors = l.divisors;
    divisors.extend(r.divisors);
    PolyShape {
        degree: if product {
            l.degree + r.degree
        } else {
            l.degree.max(r.degree)
        },
        variables,
        divisors,
    }
}

/// Collect every nonlinear product/division term. Returns `false` when a
/// term is not a plain polynomial over variables and literals.
fn collect_nonlinear_terms_expr(expr: &Expr, terms: &mut Vec<PolyShape>) -> bool {
    match expr {
        Expr::BinaryOp(_, Op::Pow, _) => false,
        Expr::BinaryOp(_, Op::Mul, _) | Expr::BinaryOp(_, Op::Div, _) => match poly_shape(expr) {
            Some(shape) => {
                if shape.degree >= 2 || !shape.divisors.is_empty() {
                    terms.push(shape);
                }
                true
            }
            None => false,
        },
        Expr::BinaryOp(left, _, right) => {
            collect_nonlinear_terms_expr(left, terms) && collect_nonlinear_terms_expr(right, terms)
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_nonlinear_terms_expr(cond, terms)
                && collect_nonlinear_terms_stmt(then_branch, terms)
                && collect_nonlinear_terms_stmt(else_branch, terms)
        }
        Expr::Call(_, args) => args
            .iter()
            .all(|arg| collect_nonlinear_terms_expr(arg, terms)),
        Expr::Number(_) | Expr::Float(_) | Expr::StringLit(_) | Expr::Variable(_) => true,
        other => !expr_has_nonlinear_arithmetic(other),
    }
}

fn collect_nonlinear_terms_stmt(stmt: &Stmt, terms: &mut Vec<PolyShape>) -> bool {
    match stmt {
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            collect_nonlinear_terms_expr(value, terms)
        }
        Stmt::Expr(value, _) => collect_nonlinear_terms_expr(value, terms),
        Stmt::Block(stmts, _) => stmts
            .iter()
            .all(|stmt| collect_nonlinear_terms_stmt(stmt, terms)),
        other => !stmt_has_nonlinear_arithmetic(other),
    }
}

/// Decide whether an atom's nonlinear arithmetic is a *bounded low-degree*
/// obligation that Z3 should try first with `nlsat` (P10-D step 1).
///
/// Conservative acceptance criteria — every one must hold:
/// - only `*` and `/` over variables/literals (no `**`, `%`, `pow`/`mod`/`exp`,
///   no finite-field helpers, no loop invariant);
/// - every product term has total degree `<= NLSAT_FIRST_MAX_DEGREE`;
/// - at most `NLSAT_FIRST_MAX_VARIABLES` distinct variables occur in nonlinear terms;
/// - each such variable has both a literal lower and upper bound in `requires`
///   (`v >= c && v <= d`, `c < v`, `v == c`, ...), and every symbolic divisor's
///   bounds exclude zero.
///
/// Returns `None` when the atom is not nonlinear at all or fails any criterion;
/// those atoms keep the unconditional Lean escalation path.
pub fn bounded_low_degree_nonlinear_profile(atom: &Atom) -> Option<BoundedNonlinearProfile> {
    let contract_text = atom_contract_text(atom);
    if text_has_nonlinear_arithmetic_marker(&contract_text)
        || atom_uses_finite_field_semantics(atom)
    {
        return None;
    }
    let requires_expr = parse_expression(&atom.requires);
    let ensures_expr = parse_expression(&atom.ensures);
    let body_stmt = parse_body_expr(&atom.body_expr);

    let mut terms = Vec::new();
    if !collect_nonlinear_terms_expr(&requires_expr, &mut terms)
        || !collect_nonlinear_terms_expr(&ensures_expr, &mut terms)
        || !collect_nonlinear_terms_stmt(&body_stmt, &mut terms)
    {
        return None;
    }
    if terms.is_empty() {
        return None;
    }

    let mut bounds = HashMap::new();
    collect_requires_bounds(&requires_expr, &mut bounds);

    let mut profile = BoundedNonlinearProfile::default();
    for term in &terms {
        if term.degree > NLSAT_FIRST_MAX_DEGREE {
            return None;
        }
        profile.max_degree = profile.max_degree.max(term.degree);
        for var in &term.variables {
            if !profile.variables.contains(var) {
                profile.variables.push(var.clone());
            }
        }
        for divisor in &term.divisors {
            if !bounds.get(divisor).is_some_and(VarBounds::excludes_zero) {
                return None;
            }
        }
    }
    if profile.variables.len() > NLSAT_FIRST_MAX_VARIABLES {
        return None;
    }
    if profile
        .variables
        .iter()
        .any(|var| !bounds.get(var).is_some_and(VarBounds::is_closed))
    {
        return None;
    }
    profile.variables.sort();
    Some(profile)
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

/// Collect the distinct names of arrays accessed as `name[...]` in `atom`
/// whose element type is *not* annotated (i.e. there is no `[T]` parameter
/// annotation for `name`), and which therefore fall back to the default `i64`
/// element sort during Z3 verification. Names are returned sorted and unique.
pub fn detect_untyped_array_accesses(atom: &Atom) -> Vec<String> {
    let mut names = Vec::new();
    collect_array_access_names_in_expr(&parse_expression(&atom.requires), &mut names);
    collect_array_access_names_in_expr(&parse_expression(&atom.ensures), &mut names);
    collect_array_access_names_in_stmt(&parse_body_expr(&atom.body_expr), &mut names);
    if let Some(invariant) = &atom.invariant {
        collect_array_access_names_in_expr(&parse_expression(invariant), &mut names);
    }
    for constraint in &atom.forall_constraints {
        collect_array_access_names_in_expr(&parse_expression(&constraint.condition), &mut names);
    }

    let mut untyped: Vec<String> = names
        .into_iter()
        .filter(|name| is_untyped_array_param(atom, name))
        .collect();
    untyped.sort();
    untyped.dedup();
    untyped
}

/// A `name[...]` access is untyped when `name` is either not a parameter, or a
/// parameter whose annotation is not a `[T]` array type. Both cases lower to
/// the `i64` element-sort fallback.
fn is_untyped_array_param(atom: &Atom, name: &str) -> bool {
    match atom.params.iter().find(|param| param.name == name) {
        Some(param) => !param
            .type_name
            .as_deref()
            .map(is_array_annotation)
            .unwrap_or(false),
        None => true,
    }
}

fn is_array_annotation(type_name: &str) -> bool {
    let trimmed = type_name.trim();
    trimmed.starts_with('[') && trimmed.ends_with(']')
}

fn collect_array_access_names_in_expr(expr: &Expr, names: &mut Vec<String>) {
    match expr {
        Expr::ArrayAccess(name, index) => {
            names.push(name.clone());
            collect_array_access_names_in_expr(index, names);
        }
        Expr::ArrayLit(elements) => {
            for element in elements {
                collect_array_access_names_in_expr(element, names);
            }
        }
        Expr::BinaryOp(left, _, right) => {
            collect_array_access_names_in_expr(left, names);
            collect_array_access_names_in_expr(right, names);
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_array_access_names_in_expr(cond, names);
            collect_array_access_names_in_stmt(then_branch, names);
            collect_array_access_names_in_stmt(else_branch, names);
        }
        Expr::Call(_, args) | Expr::Perform { args, .. } => {
            for arg in args {
                collect_array_access_names_in_expr(arg, names);
            }
        }
        Expr::CallRef { callee, args } => {
            collect_array_access_names_in_expr(callee, names);
            for arg in args {
                collect_array_access_names_in_expr(arg, names);
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, field_expr) in fields {
                collect_array_access_names_in_expr(field_expr, names);
            }
        }
        Expr::FieldAccess(base, _) => collect_array_access_names_in_expr(base, names),
        Expr::Match { target, arms } => {
            collect_array_access_names_in_expr(target, names);
            for arm in arms {
                collect_array_access_names_in_stmt(&arm.body, names);
            }
        }
        Expr::Async { body } | Expr::Lambda { body, .. } => {
            collect_array_access_names_in_stmt(body, names)
        }
        Expr::Await { expr } => collect_array_access_names_in_expr(expr, names),
        Expr::ChanSend { channel, value } => {
            collect_array_access_names_in_expr(channel, names);
            collect_array_access_names_in_expr(value, names);
        }
        Expr::ChanRecv { channel } => collect_array_access_names_in_expr(channel, names),
        Expr::Number(_)
        | Expr::Float(_)
        | Expr::StringLit(_)
        | Expr::Variable(_)
        | Expr::AtomRef { .. } => {}
    }
}

fn collect_array_access_names_in_stmt(stmt: &Stmt, names: &mut Vec<String>) {
    match stmt {
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            collect_array_access_names_in_expr(value, names)
        }
        Stmt::Expr(value, _) => collect_array_access_names_in_expr(value, names),
        Stmt::ArrayStore {
            array,
            index,
            value,
            ..
        } => {
            names.push(array.clone());
            collect_array_access_names_in_expr(index, names);
            collect_array_access_names_in_expr(value, names);
        }
        Stmt::Block(stmts, _) => {
            for stmt in stmts {
                collect_array_access_names_in_stmt(stmt, names);
            }
        }
        Stmt::While {
            cond,
            invariant,
            body,
            ..
        } => {
            collect_array_access_names_in_expr(cond, names);
            collect_array_access_names_in_expr(invariant, names);
            collect_array_access_names_in_stmt(body, names);
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
            collect_array_access_names_in_stmt(body, names)
        }
        Stmt::TaskGroup { children, .. } => {
            for child in children {
                collect_array_access_names_in_stmt(child, names);
            }
        }
        Stmt::Cancel { .. } => {}
    }
}

/// Build a diagnostic for untyped array accesses in `atom`, or `None` when the
/// atom has no untyped accesses. `strict` selects the severity: `"error"` under
/// the opt-in strict mode, `"warning"` otherwise. This never changes the
/// verification encoding — untyped arrays still lower to the `i64` element sort.
pub fn untyped_array_access_diagnostic(atom: &Atom, strict: bool) -> Option<Diagnostic> {
    let names = detect_untyped_array_accesses(atom);
    if names.is_empty() {
        return None;
    }
    let rendered = names
        .iter()
        .map(|name| format!("`{}`", name))
        .collect::<Vec<_>>()
        .join(", ");
    let plural = if names.len() == 1 { "" } else { "s" };
    Some(Diagnostic {
        code: "untyped_array_access".to_string(),
        severity: if strict { "error" } else { "warning" }.to_string(),
        atom: atom.name.clone(),
        message: format!(
            "unannotated array access{plural} {rendered} default{} to element type `i64`; \
             add an explicit `[i64]`/`[f64]`/`[bool]` annotation to select the element sort",
            if names.len() == 1 { "s" } else { "" }
        ),
        tags: vec!["untyped_array_access".to_string()],
        escalation_reason: None,
    })
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

/// True when the atom's contract or body uses a bitwise operator, i.e. its
/// obligation only has a faithful meaning in the bit-vector theory
/// (`--bitvec-i64`).
///
/// This is a *decidable* fragment (QF_BV), so the tag alone does not make the
/// atom an escalation candidate; it is recorded so reports and the Lean bridge
/// can tell BV obligations apart from unbounded-integer ones. An obligation
/// that is not closed under BV — e.g. one that also needs nonlinear/ring
/// reasoning — still carries `nonlinear_arithmetic` and stays outside the
/// fragment.
/// A bitwise operator anywhere in the contract — including the loop invariant
/// and the bounds and condition of an extracted quantifier — decides the
/// encoding, since all of those clauses are lowered with the same sorts.
fn atom_uses_bitvector_semantics(
    atom: &Atom,
    requires: &Expr,
    ensures: &Expr,
    body: &Stmt,
) -> bool {
    expr_has_bitwise_op(requires)
        || expr_has_bitwise_op(ensures)
        || stmt_has_bitwise_op(body)
        || atom
            .invariant
            .as_ref()
            .is_some_and(|inv| expr_has_bitwise_op(&parse_expression(inv)))
        || atom.forall_constraints.iter().any(|q| {
            [&q.start, &q.end, &q.condition]
                .iter()
                .any(|clause| expr_has_bitwise_op(&parse_expression(clause)))
        })
}

fn expr_has_bitwise_op(expr: &Expr) -> bool {
    match expr {
        Expr::BinaryOp(left, op, right) => {
            matches!(op, Op::BitAnd | Op::BitOr | Op::BitXor | Op::Shl | Op::Shr)
                || expr_has_bitwise_op(left)
                || expr_has_bitwise_op(right)
        }
        Expr::ArrayAccess(_, index) => expr_has_bitwise_op(index),
        Expr::Call(_, args) => args.iter().any(expr_has_bitwise_op),
        Expr::FieldAccess(inner, _) => expr_has_bitwise_op(inner),
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            expr_has_bitwise_op(cond)
                || stmt_has_bitwise_op(then_branch)
                || stmt_has_bitwise_op(else_branch)
        }
        Expr::Match { target, arms } => {
            expr_has_bitwise_op(target) || arms.iter().any(|arm| stmt_has_bitwise_op(&arm.body))
        }
        Expr::Async { body } | Expr::Lambda { body, .. } => stmt_has_bitwise_op(body),
        Expr::Await { expr } => expr_has_bitwise_op(expr),
        Expr::CallRef { args, .. } | Expr::Perform { args, .. } => {
            args.iter().any(expr_has_bitwise_op)
        }
        Expr::StructInit { fields, .. } => {
            fields.iter().any(|(_, value)| expr_has_bitwise_op(value))
        }
        _ => false,
    }
}

fn stmt_has_bitwise_op(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Expr(expr, _) => expr_has_bitwise_op(expr),
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => expr_has_bitwise_op(value),
        Stmt::ArrayStore { index, value, .. } => {
            expr_has_bitwise_op(index) || expr_has_bitwise_op(value)
        }
        Stmt::Block(stmts, _) => stmts.iter().any(stmt_has_bitwise_op),
        Stmt::While {
            cond,
            invariant,
            body,
            ..
        } => {
            expr_has_bitwise_op(cond) || expr_has_bitwise_op(invariant) || stmt_has_bitwise_op(body)
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => stmt_has_bitwise_op(body),
        Stmt::TaskGroup { children, .. } => children.iter().any(stmt_has_bitwise_op),
        Stmt::Cancel { .. } => false,
    }
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
        // Nested alternation: the inner quantifier lives inside the
        // outer constraint's `condition` (nested matches are not
        // extracted as separate constraints).
        _ if atom.forall_constraints.iter().any(|q| {
            q.q_type == QuantifierType::ForAll
                && (q.condition.contains("exists(")
                    || q.start.contains("exists(")
                    || q.end.contains("exists("))
        }) =>
        {
            "forall exists"
        }
        _ if atom.forall_constraints.iter().any(|q| {
            q.q_type == QuantifierType::Exists
                && (q.condition.contains("forall(")
                    || q.start.contains("forall(")
                    || q.end.contains("forall("))
        }) =>
        {
            "exists forall"
        }
        _ => "mixed quantifiers",
    }
}

pub(crate) fn atom_has_quantifier_alternation(atom: &Atom) -> bool {
    let has_forall = atom.forall_constraints.iter().any(|q| {
        q.q_type == QuantifierType::ForAll
            || q.condition.contains("forall(")
            || q.start.contains("forall(")
            || q.end.contains("forall(")
    }) || atom.requires.contains("forall(")
        || atom.ensures.contains("forall(");
    let has_exists = atom.forall_constraints.iter().any(|q| {
        q.q_type == QuantifierType::Exists
            || q.condition.contains("exists(")
            || q.start.contains("exists(")
            || q.end.contains("exists(")
    }) || atom.requires.contains("exists(")
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
    // P10-B: `matches(` / `match_regex(` / `re_match(` calls whose literal
    // pattern compiles to Z3 RegLan stay decidable — mask each such call so
    // the word triggers below do not tag it ("match_regex" itself contains
    // "regex"). Unparseable or uncompilable patterns keep the tag.
    let mut masked = normalized.clone().into_bytes();
    for name in ["matches(", "match_regex(", "re_match("] {
        let mut search_from = 0usize;
        while let Some(rel) = normalized[search_from..].find(name) {
            let start = search_from + rel;
            // Word boundary: `more_match(` must not be read as `re_match(`.
            if start > 0
                && (normalized.as_bytes()[start - 1].is_ascii_alphanumeric()
                    || normalized.as_bytes()[start - 1] == b'_')
            {
                search_from = start + name.len();
                continue;
            }
            let open = start + name.len() - 1; // byte index of '('
            match regex_call_span(&normalized, open) {
                Some((close, Some(pattern)))
                    if crate::verification::support::reglan::supported(&pattern) =>
                {
                    for b in &mut masked[start..=close] {
                        *b = b' ';
                    }
                    search_from = close + 1;
                }
                _ => return true,
            }
        }
    }
    let masked = String::from_utf8_lossy(&masked);
    masked.contains("regex") || masked.contains("regexp")
}

/// `matches` / `match_regex` / `re_match` lower to Z3 RegLan when their
/// pattern is a supported literal.
fn is_regex_builtin_name(name: &str) -> bool {
    matches!(name, "matches" | "match_regex" | "re_match")
}

/// Byte offsets of a `name(...)` call's closing `)` plus its last quoted
/// string literal (the regex pattern), tracking paren depth and skipping
/// quoted spans. `open` is the index of the call's `(`.
fn regex_call_span(text: &str, open: usize) -> Option<(usize, Option<String>)> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                // Skip the quoted span (honoring \" escapes).
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' => i += 1,
                        b'"' => break,
                        _ => {}
                    }
                    i += 1;
                }
            }
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    let inner = &text[open + 1..i];
                    // The pattern is the last quoted literal inside the call.
                    if let Some(endq) = inner.rfind('"') {
                        if let Some(startq) = inner[..endq].rfind('"') {
                            return Some((i, Some(inner[startq + 1..endq].to_string())));
                        }
                    }
                    return Some((i, None));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
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
        Expr::ArrayLit(elements) => {
            for element in elements {
                collect_array_index_names_from_expr(element, indexes);
            }
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
        Expr::ArrayLit(elements) => elements.iter().any(expr_has_array_access),
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
        Expr::BinaryOp(_, Op::Pow, _) => false,
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
        Expr::ArrayLit(elements) => elements.iter().any(expr_has_linear_arithmetic),
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
        Expr::BinaryOp(_, Op::Pow, _) => true,
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
        Expr::ArrayLit(elements) => elements.iter().any(expr_has_nonlinear_arithmetic),
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
            // P10-B: a regex builtin call is decidable when its pattern is a
            // literal inside the supported RegLan fragment; anything else
            // keeps the `regex_semantics` tag for Lean delegation.
            let own_call = if is_regex_builtin_name(name) {
                match args.get(1) {
                    Some(Expr::StringLit(pattern)) => {
                        !crate::verification::support::reglan::supported(pattern)
                    }
                    _ => true,
                }
            } else {
                text_has_regex_semantics(name)
            };
            own_call || args.iter().any(expr_has_regex_semantics)
        }
        Expr::ArrayLit(elements) => elements.iter().any(expr_has_regex_semantics),
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

/// The enum type an expression evaluates to, when statically inferable for
/// fragment classification: variables resolve through the `names` env
/// (params + enclosing `let` bindings), `E::V(..)`/`E::V` constructors name
/// their enum, calls to atoms declared to return an enum resolve via the
/// return type, and `if`/`match` resolve when every branch agrees.
fn expr_enum_name(
    expr: &Expr,
    names: &std::collections::HashMap<String, String>,
    module_env: &ModuleEnv,
) -> Option<String> {
    match expr {
        Expr::Variable(v) => names.get(v.as_str()).cloned(),
        Expr::Call(name, _) => {
            if let Some((enum_name, variant)) = name.split_once("::") {
                module_env
                    .get_enum(enum_name)
                    .filter(|e| e.variants.iter().any(|v| v.name == variant))
                    .map(|e| e.name.clone())
            } else {
                module_env
                    .get_atom(name)
                    .and_then(|a| a.return_type.as_deref())
                    .map(|t| crate::verification::support::datatype::type_name_base(t).to_string())
                    .filter(|t| module_env.get_enum(t).is_some())
            }
        }
        Expr::FieldAccess(inner, field) => match inner.as_ref() {
            // `E.V` unit constructor (or `value.field` — only the qualified
            // unit-constructor form names an enum).
            Expr::Variable(base) => module_env
                .get_enum(base)
                .filter(|e| e.variants.iter().any(|v| v.name == *field))
                .map(|e| e.name.clone()),
            _ => None,
        },
        Expr::IfThenElse {
            then_branch,
            else_branch,
            ..
        } => {
            let t = stmt_enum_name(then_branch, names, module_env);
            let e = stmt_enum_name(else_branch, names, module_env);
            if t.is_some() && t == e {
                t
            } else {
                None
            }
        }
        Expr::Match { arms, .. } => {
            let mut results = arms
                .iter()
                .filter_map(|arm| stmt_enum_name(&arm.body, names, module_env));
            let first = results.next()?;
            if results.all(|n| n == first) {
                Some(first)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// The enum type a statement position evaluates to (expression statement or
/// block tail), mirroring `expr_enum_name`.
fn stmt_enum_name(
    stmt: &Stmt,
    names: &std::collections::HashMap<String, String>,
    module_env: &ModuleEnv,
) -> Option<String> {
    match stmt {
        Stmt::Expr(e, _) => expr_enum_name(e, names, module_env),
        Stmt::Block(stmts, _) => stmts
            .last()
            .and_then(|s| stmt_enum_name(s, names, module_env)),
        _ => None,
    }
}

pub(crate) fn expr_has_inductive_shape(
    expr: &Expr,
    module_env: &ModuleEnv,
    names: &std::collections::HashMap<String, String>,
) -> bool {
    match expr {
        // P10-C: a `match` whose arms resolve to a finite, non-recursive enum
        // is verified natively on the Z3 datatype encoding — it is inductive
        // only when a nested subexpression is. Scalar `match`es and recursive
        // enums keep the historic `inductive_data_type` tag. The owner enum
        // is resolved with the scrutinee's declared type as the hint (the
        // verifier's rule): a bare `Ok` colliding with a same-named variant
        // of another enum resolves to the declared enum instead of staying
        // ambiguous; without a resolvable owner the match fails closed at
        // verify time and keeps the tag here too.
        Expr::Match { target, arms } => {
            let hint = expr_enum_name(target, names, module_env);
            let on_finite_adt = arms.iter().any(|arm| {
                matches!(&arm.pattern, crate::parser::ast::Pattern::Variant { variant_name, .. }
                if module_env
                    .resolve_variant_owner_by_hint(variant_name, hint.as_deref())
                    .ok()
                    .flatten()
                    .is_some_and(|e| {
                        crate::verification::support::datatype::is_finite_adt(e, module_env)
                    }))
            });
            !on_finite_adt
                || expr_has_inductive_shape(target, module_env, names)
                || arms.iter().any(|arm| {
                    // Arm bodies are a fresh scope for `let`-bound enum names.
                    let mut arm_names = names.clone();
                    arm.guard
                        .as_ref()
                        .is_some_and(|g| expr_has_inductive_shape(g, module_env, names))
                        || stmt_has_inductive_shape(&arm.body, module_env, &mut arm_names)
                })
        }
        Expr::BinaryOp(left, _, right) => {
            expr_has_inductive_shape(left, module_env, names)
                || expr_has_inductive_shape(right, module_env, names)
        }
        Expr::ArrayAccess(_, idx) => expr_has_inductive_shape(idx, module_env, names),
        Expr::ArrayLit(elements) => elements
            .iter()
            .any(|e| expr_has_inductive_shape(e, module_env, names)),
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            let mut then_names = names.clone();
            let mut else_names = names.clone();
            expr_has_inductive_shape(cond, module_env, names)
                || stmt_has_inductive_shape(then_branch, module_env, &mut then_names)
                || stmt_has_inductive_shape(else_branch, module_env, &mut else_names)
        }
        Expr::Call(_, args) => args
            .iter()
            .any(|arg| expr_has_inductive_shape(arg, module_env, names)),
        Expr::StructInit { fields, .. } => fields
            .iter()
            .any(|(_, field_expr)| expr_has_inductive_shape(field_expr, module_env, names)),
        Expr::FieldAccess(base, _) => expr_has_inductive_shape(base, module_env, names),
        Expr::Async { body } | Expr::Lambda { body, .. } => {
            let mut body_names = names.clone();
            stmt_has_inductive_shape(body, module_env, &mut body_names)
        }
        Expr::Await { expr } => expr_has_inductive_shape(expr, module_env, names),
        Expr::CallRef { callee, args } => {
            expr_has_inductive_shape(callee, module_env, names)
                || args
                    .iter()
                    .any(|arg| expr_has_inductive_shape(arg, module_env, names))
        }
        Expr::Perform { args, .. } => args
            .iter()
            .any(|arg| expr_has_inductive_shape(arg, module_env, names)),
        Expr::ChanSend { channel, value } => {
            expr_has_inductive_shape(channel, module_env, names)
                || expr_has_inductive_shape(value, module_env, names)
        }
        Expr::ChanRecv { channel } => expr_has_inductive_shape(channel, module_env, names),
        Expr::Number(_)
        | Expr::Float(_)
        | Expr::StringLit(_)
        | Expr::Variable(_)
        | Expr::AtomRef { .. } => false,
    }
}

pub(crate) fn stmt_has_inductive_shape(
    stmt: &Stmt,
    module_env: &ModuleEnv,
    names: &mut std::collections::HashMap<String, String>,
) -> bool {
    match stmt {
        Stmt::Let { var, value, .. } | Stmt::Assign { var, value, .. } => {
            let inductive = expr_has_inductive_shape(value, module_env, names);
            // Record (or clear) the binding's enum type so later siblings
            // resolve `match <var>` against the declared type.
            match expr_enum_name(value, names, module_env) {
                Some(enum_name) => {
                    names.insert(var.clone(), enum_name);
                }
                None => {
                    names.remove(var);
                }
            }
            inductive
        }
        Stmt::Expr(value, _) => expr_has_inductive_shape(value, module_env, names),
        Stmt::ArrayStore { index, value, .. } => {
            expr_has_inductive_shape(index, module_env, names)
                || expr_has_inductive_shape(value, module_env, names)
        }
        Stmt::Block(stmts, _) => {
            // `let` bindings are block-scoped: visible to later siblings,
            // invisible past the block.
            let mut block_names = names.clone();
            let mut inductive = false;
            for s in stmts {
                inductive |= stmt_has_inductive_shape(s, module_env, &mut block_names);
            }
            inductive
        }
        Stmt::While {
            cond,
            invariant,
            body,
            ..
        } => {
            let mut body_names = names.clone();
            expr_has_inductive_shape(cond, module_env, names)
                || expr_has_inductive_shape(invariant, module_env, names)
                || stmt_has_inductive_shape(body, module_env, &mut body_names)
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
            let mut body_names = names.clone();
            stmt_has_inductive_shape(body, module_env, &mut body_names)
        }
        Stmt::TaskGroup { children, .. } => {
            let mut inductive = false;
            for child in children {
                let mut child_names = names.clone();
                inductive |= stmt_has_inductive_shape(child, module_env, &mut child_names);
            }
            inductive
        }
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
        Expr::ArrayLit(elements) => elements.iter().any(expr_contains_while),
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
