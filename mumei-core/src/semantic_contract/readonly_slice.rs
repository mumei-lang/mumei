//! Source-model admission and fresh Z3 checks for shared read-only slices.

use super::*;
use crate::parser::token::{SpannedToken, Token};
use crate::source_evidence::{MAX_RESOURCE_LIMIT, MAX_TIMEOUT_MS, SOURCE_LABEL};
use z3::{ast::Int, Config, SatResult, Solver};

pub const PROFILE: &str = "mumei.readonly-i64-region-slice/v1";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("source or expression failed checked parsing")]
    Parse,
    #[error("unsupported by the read-only i64 region-slice profile: {0}")]
    Unsupported(&'static str),
    #[error("cannot encode the read-only i64 region-slice model")]
    Encoding,
}

impl From<SemanticContractError> for Error {
    fn from(error: SemanticContractError) -> Self {
        match error {
            SemanticContractError::Parse => Self::Parse,
            SemanticContractError::Unsupported(reason) => Self::Unsupported(reason),
            SemanticContractError::Encoding => Self::Encoding,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    EstablishedWithinProfile,
    Refuted,
    EmptyInputDomain,
    Inconclusive,
}

#[derive(Debug, Serialize)]
pub struct Diagnostic {
    schema_version: &'static str,
    profile: &'static str,
    atom: String,
    source_span: Span,
    source_sha256: String,
    specification_sha256: String,
    implementation_sha256: String,
    binders: Vec<TypedBinder>,
    normalization: &'static str,
    assumptions: [&'static str; 3],
    model: &'static str,
    lean: &'static str,
    llvm: &'static str,
    correspondence: &'static str,
    evidence_acceptance: &'static str,
    pub status: Status,
    checks: Vec<QueryResult>,
}

#[derive(Debug, Serialize)]
struct QueryResult {
    obligation: &'static str,
    z3: &'static str,
}

pub(super) struct SliceView<'ctx> {
    region: Int<'ctx>,
    start: Int<'ctx>,
    pub(super) len: BV<'ctx>,
}

pub(super) struct SliceModel<'ctx> {
    pub(super) views: BTreeMap<String, SliceView<'ctx>>,
    extent: FuncDecl<'ctx>,
    cells: FuncDecl<'ctx>,
}

impl<'ctx> SliceModel<'ctx> {
    fn new(ctx: &'ctx Context, names: impl Iterator<Item = String>) -> Self {
        let int = Sort::int(ctx);
        Self {
            views: names
                .enumerate()
                .map(|(i, name)| {
                    (
                        name,
                        SliceView {
                            region: Int::new_const(ctx, format!("slice_{i}_region")),
                            start: Int::new_const(ctx, format!("slice_{i}_start")),
                            len: BV::new_const(ctx, format!("slice_{i}_len"), 64),
                        },
                    )
                })
                .collect(),
            extent: FuncDecl::new(ctx, "slice_region_extent", &[&int], &int),
            cells: FuncDecl::new(
                ctx,
                "slice_region_cell",
                &[&int, &int],
                &Sort::bitvector(ctx, 64),
            ),
        }
    }

    fn domain(&self, ctx: &'ctx Context) -> BodyResult<Bool<'ctx>> {
        let zero = Int::from_i64(ctx, 0);
        let mut well_formed = Vec::new();
        for view in self.views.values() {
            let extent = self
                .extent
                .apply(&[&view.region])
                .as_int()
                .ok_or(SemanticContractError::Encoding)?;
            let len = Int::from_bv(&view.len, true);
            well_formed.extend([
                view.start.ge(&zero),
                len.ge(&zero),
                Int::add(ctx, &[&view.start, &len]).le(&extent),
            ]);
        }
        Ok(conjunction(ctx, &well_formed))
    }

    pub(super) fn read(&self, view: &SliceView<'ctx>, index: &BV<'ctx>) -> BodyResult<BV<'ctx>> {
        let index = Int::from_bv(index, true);
        let offset = Int::add(index.get_ctx(), &[&view.start, &index]);
        self.cells
            .apply(&[&view.region, &offset])
            .as_bv()
            .ok_or(SemanticContractError::Encoding)
    }
}

type ParsedAtom = (Atom, Expr, Expr, Expr);

fn admit(source: &str) -> Result<ParsedAtom, Error> {
    let tokens = bounded_source_tokens(source)?;
    if !balanced_delimiters(&tokens) {
        return Err(Error::Parse);
    }
    if tokens.iter().any(|t| matches!(t.token, Token::FloatLit(_))) {
        return Err(Error::Unsupported("floating-point literals"));
    }
    if tokens
        .iter()
        .any(|t| matches!(t.token, Token::Forall | Token::Exists))
    {
        return Err(Error::Unsupported("quantifiers"));
    }
    if tokens
        .iter()
        .any(|t| matches!(t.token, Token::Dot | Token::ColonColon))
    {
        return Err(Error::Unsupported("field access and qualified calls"));
    }
    if !source_shape(&tokens) {
        return Err(Error::Unsupported(
            "expected one typed atom with explicit requires, ensures and body clauses",
        ));
    }
    let mut items = parse_module_checked(source).map_err(|_| Error::Parse)?;
    let Some(Item::Atom(atom)) = items.pop().filter(|_| items.is_empty()) else {
        return Err(Error::Unsupported("expected exactly one atom"));
    };
    validate_atom_annotations(&atom, false, false)?;
    if atom.name == "len" {
        return Err(Error::Unsupported(
            "len is reserved for the slice intrinsic",
        ));
    }
    if atom.params.len() > 16 {
        return Err(Error::Unsupported("parameter limit"));
    }
    if atom.return_type.as_deref() != Some("i64")
        || !atom.params.iter().any(|p| p.is_ref)
        || atom.params.iter().any(|p| {
            p.is_ref_mut
                || p.fn_contract_requires.is_some()
                || p.fn_contract_ensures.is_some()
                || !matches!(
                    (p.is_ref, p.type_name.as_deref()),
                    (true, Some("[i64]")) | (false, Some("i64"))
                )
        })
    {
        return Err(Error::Unsupported("expected shared ref [i64] inputs and an explicit i64 result; only owned i64 scalar inputs are also supported"));
    }
    let requires = parse_slice_expression(&atom.requires)?;
    let ensures = parse_slice_expression(&atom.ensures)?;
    let body = parse_slice_expression(&atom.body_expr)?;
    for expr in [&requires, &ensures, &body] {
        admit_expr(expr)?;
    }
    Ok((atom, requires, ensures, body))
}

fn parse_slice_expression(input: &str) -> Result<Expr, Error> {
    let tokens = crate::parser::lexer::Lexer::new(input)
        .tokenize_checked()
        .map_err(|_| Error::Parse)?;
    let mut paren_depth = 0usize;
    for token in &tokens {
        match token.token {
            Token::LParen => paren_depth += 1,
            Token::RParen => paren_depth = paren_depth.saturating_sub(1),
            Token::Eq | Token::Neq | Token::Gt | Token::Lt | Token::Ge | Token::Le
                if paren_depth > 0 =>
            {
                return Err(Error::Unsupported("parenthesized comparisons"));
            }
            _ => (),
        }
    }
    parse_expression_checked(input).map_err(|_| Error::Parse)
}

fn balanced_delimiters(tokens: &[SpannedToken]) -> bool {
    let mut closing = Vec::new();
    for token in tokens {
        match token.token {
            Token::LParen => closing.push(Token::RParen),
            Token::LBrace => closing.push(Token::RBrace),
            Token::LBracket => closing.push(Token::RBracket),
            ref close @ (Token::RParen | Token::RBrace | Token::RBracket)
                if closing.pop().as_ref() != Some(close) =>
            {
                return false;
            }
            _ => (),
        }
    }
    closing.is_empty()
}

fn source_shape(tokens: &[SpannedToken]) -> bool {
    use Token::*;

    let tokens = tokens.iter().map(|t| &t.token).collect::<Vec<_>>();
    let [Atom, Ident(_), LParen, rest @ .., Eof] = tokens.as_slice() else {
        return false;
    };
    let Some(close) = rest.iter().position(|t| **t == RParen) else {
        return false;
    };
    let (params, tail) = rest.split_at(close);
    let params = params.strip_suffix(&[&Comma]).unwrap_or(params);
    if !params.split(|t| **t == Comma).all(|param| match param {
        [Ref, Ident(_), Colon, LBracket, Ident(ty), RBracket] | [Ident(_), Colon, Ident(ty)] => {
            ty == "i64"
        }
        _ => false,
    }) {
        return false;
    }
    let [RParen, Arrow, Ident(result_ty), clauses @ ..] = tail else {
        return false;
    };
    if result_ty != "i64" {
        return false;
    }
    let clauses = match clauses {
        [LBrace, inner @ .., RBrace] => inner,
        _ => clauses,
    };
    let clauses = clauses.strip_suffix(&[&Semicolon]).unwrap_or(clauses);
    let mut depth = 0;
    let mut parts = clauses.split(|t| match t {
        LParen | LBrace | LBracket => {
            depth += 1;
            false
        }
        RParen | RBrace | RBracket => {
            depth -= 1;
            false
        }
        Semicolon => depth == 0,
        _ => false,
    });
    let (Some(first), Some(second), Some([Body, Colon, body @ ..]), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    matches!(
        (first, second),
        ([Requires, Colon, _, ..], [Ensures, Colon, _, ..])
            | ([Ensures, Colon, _, ..], [Requires, Colon, _, ..])
    ) && match body {
        [] => false,
        [LBrace, ..] => {
            let mut depth = 0;
            body.iter().position(|t| match t {
                LBrace => {
                    depth += 1;
                    false
                }
                RBrace => {
                    depth -= 1;
                    depth == 0
                }
                _ => false,
            }) == Some(body.len() - 1)
        }
        _ => true,
    }
}

fn admit_expr(expr: &Expr) -> Result<(), Error> {
    fn admit_stmt(stmt: &Stmt) -> Result<(), Error> {
        match stmt {
            Stmt::Expr(expr, _) => admit_expr(expr),
            Stmt::Block(stmts, _) if stmts.len() == 1 => admit_stmt(&stmts[0]),
            _ => Err(Error::Unsupported("only expression blocks are supported")),
        }
    }
    match expr {
        Expr::Number(_) | Expr::Variable(_) => Ok(()),
        Expr::ArrayAccess(_, index) => admit_expr(index),
        Expr::BinaryOp(
            left,
            Op::Eq | Op::Neq | Op::Gt | Op::Lt | Op::Ge | Op::Le | Op::And | Op::Or | Op::Implies,
            right,
        ) => {
            admit_expr(left)?;
            admit_expr(right)
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            admit_expr(cond)?;
            admit_stmt(then_branch)?;
            admit_stmt(else_branch)
        }
        Expr::Block(stmt) => admit_stmt(stmt),
        Expr::Call(name, args)
            if name == "len" && matches!(args.as_slice(), [Expr::Variable(_)]) =>
        {
            Ok(())
        }
        _ => Err(Error::Unsupported(
            "expression outside the comparison, conditional, len and indexed-read fragment",
        )),
    }
}

fn conjunction<'ctx>(ctx: &'ctx Context, terms: &[Bool<'ctx>]) -> Bool<'ctx> {
    Bool::and(ctx, &terms.iter().collect::<Vec<_>>())
}

fn encode<'ctx>(
    ctx: &'ctx Context,
    shapes: &CallShapes<'ctx>,
    expr: &Expr,
    env: &HashMap<String, SourceValue<'ctx>>,
    guard: &Bool<'ctx>,
) -> BodyResult<(SourceValue<'ctx>, Bool<'ctx>)> {
    let mut bounds = Vec::new();
    let value = source_expr(
        expr,
        ctx,
        env,
        guard,
        &mut bounds,
        0,
        &BTreeSet::new(),
        &mut HashMap::new(),
        &mut Vec::new(),
        &mut Vec::new(),
        shapes,
        &EnumEnv::new(),
    )?;
    Ok((value, conjunction(ctx, &bounds)))
}

/// Fresh source-model check. Does not run an emitter or emit evidence for acceptance.
pub fn check(source: &str) -> Result<Diagnostic, Error> {
    let (atom, requires, ensures, body) = admit(source)?;
    let mut config = Config::new();
    config.set_timeout_msec(u64::from(MAX_TIMEOUT_MS));
    config.set_param_value("rlimit", &MAX_RESOURCE_LIMIT.to_string());
    let ctx = &Context::new(&config);
    let model = SliceModel::new(
        ctx,
        atom.params
            .iter()
            .filter(|p| p.is_ref)
            .map(|p| p.name.clone()),
    );
    let entry = model.domain(ctx)?;
    let shapes = CallShapes {
        readonly_slices: Some(model),
        ..CallShapes::default()
    };
    let mut env = atom
        .params
        .iter()
        .filter(|p| !p.is_ref)
        .enumerate()
        .map(|(i, p)| {
            (
                p.name.clone(),
                SourceValue::Integer(BV::new_const(ctx, format!("slice_scalar_{i}"), 64)),
            )
        })
        .collect();
    let (requires, requires_bounds) =
        encode(ctx, &shapes, &requires, &env, &Bool::from_bool(ctx, true))?;
    let requires = requires.boolean()?;
    let (value, body_bounds) = encode(ctx, &shapes, &body, &env, &requires)?;
    env.insert("result".to_owned(), SourceValue::Integer(value.integer()?));
    let (ensures, ensures_bounds) = encode(ctx, &shapes, &ensures, &env, &requires)?;
    let ensures = ensures.boolean()?;
    let domain = Bool::and(ctx, &[&entry, &requires]);
    let queries = [
        (
            "requires_bounds",
            Bool::and(ctx, &[&entry, &requires_bounds.not()]),
            SatResult::Unsat,
        ),
        ("input_domain", domain, SatResult::Sat),
        (
            "body_bounds",
            Bool::and(ctx, &[&entry, &body_bounds.not()]),
            SatResult::Unsat,
        ),
        (
            "ensures_bounds",
            Bool::and(ctx, &[&entry, &ensures_bounds.not()]),
            SatResult::Unsat,
        ),
        (
            "postcondition",
            Bool::and(ctx, &[&entry, &requires, &ensures.not()]),
            SatResult::Unsat,
        ),
    ];
    let solver = Solver::new(ctx);
    let mut checks = Vec::new();
    let mut status = Status::EstablishedWithinProfile;
    for (obligation, query, expected) in queries {
        let result = solver.check_assumptions(&[query]);
        checks.push(QueryResult {
            obligation,
            z3: match result {
                SatResult::Sat => "sat",
                SatResult::Unsat => "unsat",
                SatResult::Unknown => "unknown",
            },
        });
        if result != expected {
            status = match result {
                SatResult::Sat => Status::Refuted,
                SatResult::Unsat => Status::EmptyInputDomain,
                SatResult::Unknown => Status::Inconclusive,
            };
            break;
        }
    }
    let source_span = Span {
        file: SOURCE_LABEL.to_owned(),
        ..atom.span.clone()
    };
    let binders = atom
        .params
        .iter()
        .map(|p| (&*p.name, p.is_ref, "parameter"))
        .chain(std::iter::once(("result", false, "result")))
        .map(|(name, is_ref, role)| TypedBinder {
            name: name.to_owned(),
            scalar_type: if is_ref { "ref [i64]" } else { "i64" },
            role,
            provenance: source_span.clone(),
            span_precision: "atom",
        })
        .collect();
    let specification_sha256 = json_digest(&serde_json::json!({
        "profile": PROFILE,
        "params": atom.params.iter().map(|p| (&p.name, p.is_ref, &p.type_name)).collect::<Vec<_>>(),
        "result_type": atom.return_type,
        "requires": atom.requires,
        "ensures": atom.ensures,
    }))?;
    Ok(Diagnostic {
        schema_version: "mumei.readonly-slice-diagnostic/v1",
        profile: PROFILE,
        atom: atom.name,
        source_span,
        source_sha256: digest(source.as_bytes()),
        specification_sha256,
        implementation_sha256: digest(atom.body_expr.as_bytes()),
        binders,
        normalization: "checked_parser_comparison_chains_to_conjunction",
        assumptions: [
            "live_initialized_regions_stable_for_the_call",
            "well_formed_shared_input_views",
            "unconstrained_region_equality_between_formals",
        ],
        model: "z3_signed_bv64_with_integer_element_offsets",
        lean: "unsupported",
        llvm: "unsupported",
        correspondence: "unchecked",
        evidence_acceptance: "not_evaluated",
        status,
        checks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_agree_only_when_both_region_and_absolute_offset_agree() {
        let ctx = &Context::new(&Config::new());
        let model = SliceModel::new(ctx, ["a".into(), "b".into()].into_iter());
        let a = &model.views["a"];
        let b = &model.views["b"];
        let solver = Solver::new(ctx);
        solver.assert(&model.domain(ctx).unwrap());
        for view in [a, b] {
            solver.assert(&view.len._eq(&BV::from_i64(ctx, 2, 64)));
        }
        let zero = BV::from_i64(ctx, 0, 64);
        let one = BV::from_i64(ctx, 1, 64);
        let same_region = a.region._eq(&b.region);
        let next_offset = Int::add(ctx, &[&a.start, &Int::from_i64(ctx, 1)])._eq(&b.start);
        let unequal_cells = model
            .read(a, &one)
            .unwrap()
            ._eq(&model.read(b, &zero).unwrap())
            .not();
        assert_eq!(
            solver.check_assumptions(&[same_region.clone(), next_offset.clone()]),
            SatResult::Sat
        );
        assert_eq!(
            solver.check_assumptions(&[
                same_region.clone(),
                next_offset.clone(),
                unequal_cells.clone()
            ]),
            SatResult::Unsat
        );
        assert_eq!(
            solver.check_assumptions(&[
                same_region.not(),
                next_offset.clone(),
                unequal_cells.clone()
            ]),
            SatResult::Sat
        );
        assert_eq!(
            solver.check_assumptions(&[same_region, next_offset.not(), unequal_cells]),
            SatResult::Sat
        );
    }
}
