// =============================================================================
// Expression and Statement parsing (Pratt parser + statement parser)
// =============================================================================

use super::pattern::parse_pattern;
use super::token::Token;
use super::ParseContext;
use crate::ast::TypeRef;
use crate::parser::{Expr, JoinSemantics, LambdaParam, MatchArm, Op, Span, Stmt};

// This must exceed FatArrow's left binding power so guards and arm bodies
// stop before `=>`.
const ABOVE_FAT_ARROW_BP: u8 = 5;

/// Pratt parser binding power for binary operators.
/// Returns (left_bp, right_bp). Left-associative: left < right.
/// Right-associative: left > right.
fn binding_power(tok: &Token) -> Option<(u8, u8)> {
    match tok {
        Token::Pipe => Some((1, 2)),
        Token::FatArrow => Some((3, 4)), // left-assoc => (matches old parser's while-loop behavior)
        Token::Or => Some((5, 6)),
        Token::And => Some((7, 8)),
        Token::Eq | Token::Neq | Token::Gt | Token::Lt | Token::Ge | Token::Le => Some((9, 10)),
        // Bitwise operators bind tighter than comparison and looser than
        // arithmetic, following the C/Rust ordering `| < ^ < & < shift`.
        // `Token::Bar` is only a bitwise OR in infix position; in prefix
        // position it still opens a lambda parameter list (`|x| x + 1`).
        Token::Bar => Some((11, 12)),
        Token::Caret => Some((13, 14)),
        Token::Amp => Some((15, 16)),
        Token::Shl | Token::Shr => Some((17, 18)),
        Token::Plus | Token::Minus => Some((19, 20)),
        Token::Star | Token::Slash => Some((21, 22)),
        Token::StarStar => Some((23, 22)),
        Token::Dot | Token::ColonColon => Some((25, 26)),
        _ => None,
    }
}

fn token_to_op(tok: &Token) -> Option<Op> {
    match tok {
        Token::Plus => Some(Op::Add),
        Token::Minus => Some(Op::Sub),
        Token::Star => Some(Op::Mul),
        Token::StarStar => Some(Op::Pow),
        Token::Slash => Some(Op::Div),
        Token::Eq => Some(Op::Eq),
        Token::Neq => Some(Op::Neq),
        Token::Gt => Some(Op::Gt),
        Token::Lt => Some(Op::Lt),
        Token::Ge => Some(Op::Ge),
        Token::Le => Some(Op::Le),
        Token::And => Some(Op::And),
        Token::Or => Some(Op::Or),
        Token::Amp => Some(Op::BitAnd),
        Token::Bar => Some(Op::BitOr),
        Token::Caret => Some(Op::BitXor),
        Token::Shl => Some(Op::Shl),
        Token::Shr => Some(Op::Shr),
        Token::FatArrow => Some(Op::Implies),
        _ => None,
    }
}

fn is_relational_op(op: &Op) -> bool {
    matches!(op, Op::Eq | Op::Neq | Op::Gt | Op::Lt | Op::Ge | Op::Le)
}

fn collect_relational_chain(expr: &Expr, operands: &mut Vec<Expr>, ops: &mut Vec<Op>) {
    if let Expr::BinaryOp(left, op, right) = expr {
        if is_relational_op(op) {
            collect_relational_chain(left, operands, ops);
            ops.push(op.clone());
            operands.push((**right).clone());
            return;
        }
    }
    operands.push(expr.clone());
}

fn normalize_expr(expr: Expr) -> Expr {
    match expr {
        Expr::BinaryOp(left, op, right) if is_relational_op(&op) => {
            let raw = Expr::BinaryOp(left, op, right);
            let mut operands = Vec::new();
            let mut ops = Vec::new();
            collect_relational_chain(&raw, &mut operands, &mut ops);
            if ops.len() <= 1 {
                let left = normalize_expr(operands.remove(0));
                let right = normalize_expr(operands.remove(0));
                return Expr::BinaryOp(Box::new(left), ops.remove(0), Box::new(right));
            }
            let mut comparisons = Vec::with_capacity(ops.len());
            for i in 0..ops.len() {
                comparisons.push(Expr::BinaryOp(
                    Box::new(normalize_expr(operands[i].clone())),
                    ops[i].clone(),
                    Box::new(normalize_expr(operands[i + 1].clone())),
                ));
            }
            let mut iter = comparisons.into_iter();
            let mut combined = iter.next().unwrap();
            for next in iter {
                combined = Expr::BinaryOp(Box::new(combined), Op::And, Box::new(next));
            }
            combined
        }
        Expr::BinaryOp(left, op, right) => Expr::BinaryOp(
            Box::new(normalize_expr(*left)),
            op,
            Box::new(normalize_expr(*right)),
        ),
        Expr::Call(name, args) => Expr::Call(name, args.into_iter().map(normalize_expr).collect()),
        Expr::ArrayAccess(name, index) => Expr::ArrayAccess(name, Box::new(normalize_expr(*index))),
        Expr::StructInit { type_name, fields } => Expr::StructInit {
            type_name,
            fields: fields
                .into_iter()
                .map(|(name, value)| (name, normalize_expr(value)))
                .collect(),
        },
        Expr::FieldAccess(base, field) => Expr::FieldAccess(Box::new(normalize_expr(*base)), field),
        Expr::Match { target, arms } => Expr::Match {
            target: Box::new(normalize_expr(*target)),
            arms: arms
                .into_iter()
                .map(|arm| MatchArm {
                    pattern: arm.pattern,
                    guard: arm.guard.map(|guard| Box::new(normalize_expr(*guard))),
                    body: arm.body,
                })
                .collect(),
        },
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => Expr::IfThenElse {
            cond: Box::new(normalize_expr(*cond)),
            then_branch,
            else_branch,
        },
        Expr::Block(stmt) => Expr::Block(stmt),
        Expr::CallRef { callee, args } => Expr::CallRef {
            callee: Box::new(normalize_expr(*callee)),
            args: args.into_iter().map(normalize_expr).collect(),
        },
        Expr::Perform {
            effect,
            operation,
            args,
        } => Expr::Perform {
            effect,
            operation,
            args: args.into_iter().map(normalize_expr).collect(),
        },
        Expr::ChanSend { channel, value } => Expr::ChanSend {
            channel: Box::new(normalize_expr(*channel)),
            value: Box::new(normalize_expr(*value)),
        },
        Expr::ChanRecv { channel } => Expr::ChanRecv {
            channel: Box::new(normalize_expr(*channel)),
        },
        Expr::Await { expr } => Expr::Await {
            expr: Box::new(normalize_expr(*expr)),
        },
        Expr::Async { body } => Expr::Async { body },
        Expr::Lambda {
            params,
            return_type,
            body,
        } => Expr::Lambda {
            params,
            return_type,
            body,
        },
        other => other,
    }
}

pub(crate) fn normalize_comparison_chains(expr: Expr) -> Expr {
    normalize_expr(expr)
}

/// Parse an expression using Pratt parsing with minimum binding power.
pub fn parse_expr(ctx: &mut ParseContext, min_bp: u8) -> Expr {
    let mut lhs = parse_prefix(ctx);

    loop {
        let tok = ctx.peek().clone();
        // Dot/`::` are handled specially for field access and qualified calls.
        if tok == Token::Dot || tok == Token::ColonColon {
            if let Some((l_bp, _)) = binding_power(&tok) {
                if l_bp < min_bp {
                    break;
                }
                let separator = if tok == Token::Dot { "." } else { "::" };
                ctx.advance();
                if let Token::Ident(field) = ctx.peek().clone() {
                    ctx.advance();
                    let access = Expr::FieldAccess(Box::new(lhs), field);
                    if ctx.peek() == &Token::LParen {
                        if let Some(name) = expr_to_call_path(&access, separator) {
                            let args = parse_call_args(ctx);
                            lhs = Expr::Call(name, args);
                            continue;
                        }
                    }
                    lhs = access;
                    continue;
                } else {
                    // Accept keyword tokens as field names (e.g., obj.mode, obj.priority)
                    let field_name = format!("{}", ctx.peek());
                    if field_name.chars().next().is_some_and(|c| c.is_alphabetic()) {
                        ctx.advance();
                        let access = Expr::FieldAccess(Box::new(lhs), field_name);
                        if ctx.peek() == &Token::LParen {
                            if let Some(name) = expr_to_call_path(&access, separator) {
                                let args = parse_call_args(ctx);
                                lhs = Expr::Call(name, args);
                                continue;
                            }
                        }
                        lhs = access;
                        continue;
                    }
                }
            }
            break;
        }

        if let Some((l_bp, r_bp)) = binding_power(&tok) {
            if l_bp < min_bp {
                break;
            }
            if tok == Token::Pipe {
                ctx.advance();
                let rhs_starts_bare_lambda = ctx.peek() == &Token::Bar;
                let rhs = parse_expr(ctx, r_bp);
                if rhs_starts_bare_lambda {
                    ctx.syntax_failure(
                        "pipeline lambda must be parenthesized: x |> (|y| ...)".to_string(),
                    );
                    continue;
                }
                lhs = match rhs {
                    Expr::Variable(name) => Expr::Call(name, vec![lhs]),
                    Expr::Call(name, mut args) => {
                        args.push(lhs);
                        Expr::Call(name, args)
                    }
                    Expr::CallRef { callee, mut args } => {
                        args.push(lhs);
                        Expr::CallRef { callee, args }
                    }
                    Expr::Lambda { .. } => Expr::CallRef {
                        callee: Box::new(rhs),
                        args: vec![lhs],
                    },
                    _ => {
                        ctx.syntax_failure(
                            "pipeline right-hand side must be a function name, call, or lambda"
                                .to_string(),
                        );
                        lhs
                    }
                };
                continue;
            }
            let op = match token_to_op(&tok) {
                Some(op) => op,
                None => break,
            };
            ctx.advance(); // consume operator
            let rhs = parse_expr(ctx, r_bp);
            lhs = Expr::BinaryOp(Box::new(lhs), op, Box::new(rhs));
        } else {
            break;
        }
    }

    // `a[i][j]` / `f()[0]` cannot be expressed: `ArrayAccess`/`ArrayStore`
    // only accept a bare identifier in the array position. Left unparsed,
    // the trailing `[…]` re-lexes as a stray array-literal statement, so the
    // intended index silently evaluates a different program. Flag it.
    if ctx.peek() == &Token::LBracket {
        ctx.syntax_failure(
            "indexing into an expression (e.g. `a[i][j]` or `f()[0]`) is not supported; \
             bind the value to a variable first"
                .to_string(),
        );
    }

    lhs
}

fn parse_call_args(ctx: &mut ParseContext) -> Vec<Expr> {
    ctx.expect(Token::LParen);
    let mut args = Vec::new();
    while ctx.peek() != &Token::RParen && ctx.peek() != &Token::Eof {
        args.push(parse_expr(ctx, 0));
        if ctx.peek() == &Token::Comma {
            ctx.advance();
        }
    }
    ctx.expect(Token::RParen);
    args
}

fn expr_to_call_path(expr: &Expr, separator: &str) -> Option<String> {
    match expr {
        Expr::Variable(name) => Some(name.clone()),
        Expr::FieldAccess(inner, field) => {
            let base = expr_to_call_path(inner, separator)?;
            Some(format!("{base}{separator}{field}"))
        }
        _ => None,
    }
}

/// Parse a prefix / primary expression.
fn parse_prefix(ctx: &mut ParseContext) -> Expr {
    match ctx.peek().clone() {
        Token::AtomRef => {
            ctx.advance();
            ctx.expect(Token::LParen);
            let name = ctx.expect_ident();
            ctx.expect(Token::RParen);
            Expr::AtomRef { name }
        }

        Token::Call => {
            ctx.advance();
            ctx.expect(Token::LParen);
            let callee = parse_expr(ctx, 0);
            let mut args = Vec::new();
            while ctx.peek() == &Token::Comma {
                ctx.advance();
                args.push(parse_expr(ctx, 0));
            }
            ctx.expect(Token::RParen);
            Expr::CallRef {
                callee: Box::new(callee),
                args,
            }
        }

        Token::Perform => {
            ctx.advance();
            let effect = ctx.expect_ident();
            ctx.expect(Token::Dot);
            let operation = ctx.expect_ident();
            let mut args = Vec::new();
            if ctx.peek() == &Token::LParen {
                ctx.advance();
                while ctx.peek() != &Token::RParen && ctx.peek() != &Token::Eof {
                    args.push(parse_expr(ctx, 0));
                    if ctx.peek() == &Token::Comma {
                        ctx.advance();
                    }
                }
                ctx.expect(Token::RParen);
            }
            Expr::Perform {
                effect,
                operation,
                args,
            }
        }

        Token::Async => {
            ctx.advance();
            let body = parse_block_or_stmt(ctx);
            Expr::Async {
                body: Box::new(body),
            }
        }

        Token::Await => {
            ctx.advance();
            let expr = parse_prefix(ctx);
            Expr::Await {
                expr: Box::new(expr),
            }
        }

        // Plan 8: Channel send — `send(ch, value)`
        Token::Send => {
            ctx.advance();
            ctx.expect(Token::LParen);
            let channel = parse_expr(ctx, 0);
            ctx.expect(Token::Comma);
            let value = parse_expr(ctx, 0);
            ctx.expect(Token::RParen);
            Expr::ChanSend {
                channel: Box::new(channel),
                value: Box::new(value),
            }
        }

        // Plan 8: Channel receive — `recv(ch)`
        Token::Recv => {
            ctx.advance();
            ctx.expect(Token::LParen);
            let channel = parse_expr(ctx, 0);
            ctx.expect(Token::RParen);
            Expr::ChanRecv {
                channel: Box::new(channel),
            }
        }

        Token::If => {
            ctx.advance();
            let cond = parse_expr(ctx, 0);
            let then_branch = parse_block_or_stmt(ctx);
            if ctx.peek() == &Token::Else {
                ctx.advance();
                let else_branch = parse_block_or_stmt(ctx);
                Expr::IfThenElse {
                    cond: Box::new(cond),
                    then_branch: Box::new(then_branch),
                    else_branch: Box::new(else_branch),
                }
            } else {
                // `if` requires an `else` branch (it is an expression). Recover
                // with an unbound marker as the else value instead of
                // panicking: the recorded syntax failure fails checked callers
                // closed, and if an unchecked caller keeps the AST, MIR
                // lowering still rejects the unresolved marker name.
                let found = format!("{}", ctx.peek());
                let (line, col) = ctx
                    .tokens_ref()
                    .get(ctx.pos())
                    .map(|t| (t.line, t.col))
                    .unwrap_or((0, 0));
                ctx.syntax_failure(format!(
                    "if expression requires an 'else' branch, found {found} at {line}:{col}"
                ));
                let else_span = ctx.current_span();
                Expr::IfThenElse {
                    cond: Box::new(cond),
                    then_branch: Box::new(then_branch),
                    else_branch: Box::new(Stmt::Expr(
                        Expr::Variable("__mumei_missing_else_branch".to_string()),
                        else_span,
                    )),
                }
            }
        }

        Token::LBrace => {
            let block = parse_block_or_stmt(ctx);
            Expr::Block(Box::new(block))
        }

        Token::Match => {
            ctx.advance();
            let target = parse_expr(ctx, 0);
            ctx.expect(Token::LBrace);
            let mut arms = Vec::new();
            while ctx.peek() != &Token::RBrace && ctx.peek() != &Token::Eof {
                let pattern = parse_pattern(ctx);
                // Optional guard: if cond
                let guard = if ctx.peek() == &Token::If {
                    ctx.advance();
                    Some(Box::new(parse_expr(ctx, ABOVE_FAT_ARROW_BP)))
                } else {
                    None
                };
                // Expect =>
                if ctx.peek() == &Token::FatArrow {
                    ctx.advance();
                } else if ctx.peek() == &Token::Assign {
                    ctx.advance();
                    if ctx.peek() == &Token::Gt {
                        ctx.advance();
                    }
                }
                let body = parse_match_arm_body(ctx);
                arms.push(MatchArm {
                    pattern,
                    guard,
                    body: Box::new(body),
                });
                if ctx.peek() == &Token::Comma {
                    ctx.advance();
                }
            }
            ctx.expect(Token::RBrace);
            Expr::Match {
                target: Box::new(target),
                arms,
            }
        }

        Token::LParen => {
            ctx.advance();
            let expr = parse_expr(ctx, 0);
            ctx.expect(Token::RParen);
            expr
        }

        Token::LBracket => {
            // Array literal `[e0, e1, …]` at expression position (a `[` after an
            // identifier is parsed as `Expr::ArrayAccess` by
            // `parse_ident_continuation`, so this arm only sees genuine literals).
            ctx.advance();
            let mut elements = Vec::new();
            while ctx.peek() != &Token::RBracket && ctx.peek() != &Token::Eof {
                elements.push(parse_expr(ctx, 0));
                if ctx.peek() == &Token::Comma {
                    ctx.advance();
                } else {
                    break;
                }
            }
            ctx.expect(Token::RBracket);
            if elements.is_empty() {
                // `[]` cannot infer an element type — record a checked
                // syntax failure (the marker variable fails closed if a
                // caller ignores the diagnostics).
                ctx.syntax_failure(
                    "empty array literal `[]` needs an element type — annotate \
                     via a non-empty literal or a typed let binding"
                        .to_string(),
                );
                return Expr::Variable("__mumei_empty_array_literal".to_string());
            }
            Expr::ArrayLit(elements)
        }

        Token::IntLit(n) => {
            let val = n;
            ctx.advance();
            Expr::Number(val)
        }

        Token::FloatLit(f) => {
            let val = f;
            ctx.advance();
            Expr::Float(val)
        }

        // Plan 9: First-class Str type — parse string literals
        Token::StringLit(s) => {
            let val = s.clone();
            ctx.advance();
            Expr::StringLit(val)
        }

        Token::True => {
            ctx.advance();
            Expr::Variable("true".to_string())
        }

        Token::False => {
            ctx.advance();
            Expr::Variable("false".to_string())
        }

        Token::Bang => {
            // Unary logical not — desugared to `if e { false } else { true }`.
            // (A relational `e == false` desugar is not safe: the
            // comparison-chain normalizer would flatten `!(a < b)` into
            // `a < b && b == false`.) Verify requires a Bool condition; codegen
            // emits the 0/1 branch, matching `if e { 0 } else { 1 }`.
            ctx.advance();
            let operand = parse_prefix(ctx);
            Expr::IfThenElse {
                cond: Box::new(operand),
                then_branch: Box::new(Stmt::Expr(
                    Expr::Variable("false".to_string()),
                    Span::default(),
                )),
                else_branch: Box::new(Stmt::Expr(
                    Expr::Variable("true".to_string()),
                    Span::default(),
                )),
            }
        }

        Token::Minus => {
            // Unary minus: only for standalone negative numbers in expression context
            // Check if next token is a number
            ctx.advance();
            match ctx.peek().clone() {
                Token::IntLit(n) => {
                    ctx.advance();
                    Expr::Number(-n)
                }
                Token::FloatLit(f) => {
                    ctx.advance();
                    Expr::Float(-f)
                }
                _ => {
                    // Treat as 0 - expr
                    let rhs = parse_prefix(ctx);
                    Expr::BinaryOp(Box::new(Expr::Number(0)), Op::Sub, Box::new(rhs))
                }
            }
        }

        Token::Ident(name) => {
            let name = name.clone();
            ctx.advance();
            parse_ident_continuation(ctx, name)
        }

        // Keywords that may appear as identifiers in expression context
        Token::Forall => {
            ctx.advance();
            parse_ident_continuation(ctx, "forall".to_string())
        }
        Token::Exists => {
            ctx.advance();
            parse_ident_continuation(ctx, "exists".to_string())
        }

        // Lambda: |params| body or |params| -> RetType { body }
        Token::Bar => {
            ctx.advance(); // consume opening |
            let mut params = Vec::new();
            while ctx.peek() != &Token::Bar && ctx.peek() != &Token::Eof {
                let param_name = ctx.expect_ident();
                let type_ref = if ctx.peek() == &Token::Colon {
                    ctx.advance(); // skip :
                    Some(crate::parser::item::parse_type_ref_from_ctx(ctx))
                } else {
                    None
                };
                params.push(LambdaParam {
                    name: param_name,
                    type_ref,
                });
                if ctx.peek() == &Token::Comma {
                    ctx.advance();
                }
            }
            if ctx.peek() == &Token::Bar {
                ctx.advance(); // consume closing |
            }
            // Optional return type: -> Type (use parse_type_ref_from_ctx to handle generics)
            let return_type = if ctx.peek() == &Token::Arrow {
                ctx.advance(); // skip ->
                let tr = crate::parser::item::parse_type_ref_from_ctx(ctx);
                Some(tr.display_name())
            } else {
                None
            };
            // Parse body: either { stmts } or a single expression
            let body = if ctx.peek() == &Token::LBrace {
                parse_block_or_stmt(ctx)
            } else {
                let body_span = ctx.current_span();
                Stmt::Expr(parse_expr(ctx, 0), body_span)
            };
            Expr::Lambda {
                params,
                return_type,
                body: Box::new(body),
            }
        }

        // Any other keyword used as identifier in expression context
        ref tok => {
            // For backward compatibility: some keywords can appear as variable names
            // or function names in expression contexts (e.g., in requires/ensures clauses)
            let name = format!("{}", tok);
            if name.chars().next().is_some_and(|c| c.is_alphabetic()) {
                ctx.advance();
                parse_ident_continuation(ctx, name)
            } else {
                let found = format!("{tok}");
                let (line, col) = ctx
                    .tokens_ref()
                    .get(ctx.pos())
                    .map(|t| (t.line, t.col))
                    .unwrap_or((0, 0));
                ctx.syntax_failure(format!(
                    "unexpected token {found} in expression at {line}:{col}"
                ));
                ctx.advance();
                Expr::Variable("__mumei_unexpected_token".to_string())
            }
        }
    }
}

/// After parsing an identifier, check for function call, struct init, array access, etc.
///
/// Known limitation: An uppercase identifier followed by `{` is always parsed as struct
/// initialization. This means `if SomeType { ... } else { ... }` would incorrectly parse
/// `SomeType { ... }` as a struct literal. In practice, mumei variables are conventionally
/// lowercase, so this ambiguity does not arise in real `.mm` files.
fn parse_ident_continuation(ctx: &mut ParseContext, name: String) -> Expr {
    match ctx.peek() {
        Token::LBrace if name.chars().next().is_some_and(|c| c.is_uppercase()) => {
            // Struct initialization: TypeName { field: expr, ... }
            ctx.advance(); // {
            let mut fields = Vec::new();
            while ctx.peek() != &Token::RBrace && ctx.peek() != &Token::Eof {
                let field_name = ctx.expect_ident();
                if ctx.peek() == &Token::Colon {
                    ctx.advance();
                }
                let value = parse_expr(ctx, 0);
                fields.push((field_name, value));
                if ctx.peek() == &Token::Comma {
                    ctx.advance();
                }
            }
            ctx.expect(Token::RBrace);
            Expr::StructInit {
                type_name: name,
                fields,
            }
        }

        Token::LParen => {
            // Function call: name(args)
            ctx.advance(); // (
            let mut args = Vec::new();
            while ctx.peek() != &Token::RParen && ctx.peek() != &Token::Eof {
                args.push(parse_expr(ctx, 0));
                if ctx.peek() == &Token::Comma {
                    ctx.advance();
                }
            }
            ctx.expect(Token::RParen);
            Expr::Call(name, args)
        }

        Token::LBracket => {
            // Array access: name[index]
            ctx.advance(); // [
            let index = parse_expr(ctx, 0);
            ctx.expect(Token::RBracket);
            Expr::ArrayAccess(name, Box::new(index))
        }

        Token::Lt => {
            // `name<T, …>(args)`: an explicit type-argument call. The `<`
            // is ambiguous with a comparison (`a < b`), so commit only
            // when the full `<` type-list `>` `(` shape matches; otherwise
            // rewind and let the Pratt loop parse `<` as `Op::Lt`.
            // The callee name carries the instantiation
            // (`apply<i64, Network>`), matching the monomorphizer's
            // `parse_type_ref(call_name)` convention and the atom
            // registry's `display_name()` keys.
            if let Some(type_args) = parse_explicit_type_args(ctx) {
                let callee = TypeRef::generic(&name, type_args).display_name();
                Expr::Call(callee, parse_call_args(ctx))
            } else {
                Expr::Variable(name)
            }
        }

        _ => Expr::Variable(name),
    }
}

/// Speculatively parse an explicit type-argument list at `ident<`…: the
/// shape `<` type-ref (`,` type-ref)* `>` `(`. Returns the `TypeRef`s
/// with the cursor on the argument list's `(` when the shape matches,
/// else rewinds the stream — including any `>>` splits
/// `parse_type_ref_from_ctx` spliced in — so `<` stays a comparison.
fn parse_explicit_type_args(ctx: &mut ParseContext) -> Option<Vec<TypeRef>> {
    let snapshot = ctx.snapshot();
    ctx.advance(); // consume '<'
    let mut type_args = Vec::new();
    let closed = loop {
        let entry_start = ctx.pos();
        let type_ref = crate::parser::item::parse_type_ref_from_ctx(ctx);
        if ctx.pos() == entry_start {
            // No type-shaped token consumed: `a < 1`, `a < >`, `a < <b`.
            break false;
        }
        type_args.push(type_ref);
        match ctx.peek() {
            Token::Comma => {
                ctx.advance();
            }
            Token::Gt => {
                ctx.advance();
                break true;
            }
            _ => break false,
        }
    };
    // Only `>` directly followed by `(` makes this a call: `a < b > c`,
    // `f<T> { … }` and `a < b > (x)`-less shapes all stay comparisons.
    if !closed || ctx.peek() != &Token::LParen {
        ctx.rewind(snapshot);
        return None;
    }
    Some(type_args)
}

/// Parse match arm body.
/// Uses parse_expr with binding power above FatArrow to avoid consuming `=>`
/// as implies.
fn parse_match_arm_body(ctx: &mut ParseContext) -> Stmt {
    if ctx.peek() == &Token::LBrace {
        parse_block_or_stmt(ctx)
    } else {
        let arm_span = ctx.current_span();
        if ctx.peek() == &Token::Match || ctx.peek() == &Token::If {
            Stmt::Expr(parse_expr(ctx, 0), arm_span)
        } else {
            Stmt::Expr(parse_expr(ctx, ABOVE_FAT_ARROW_BP), arm_span)
        }
    }
}

/// Parse a block or single statement.
pub fn parse_block_or_stmt(ctx: &mut ParseContext) -> Stmt {
    if ctx.peek() == &Token::LBrace {
        let span = ctx.current_span();
        ctx.advance(); // {
        let mut stmts = Vec::new();
        while ctx.peek() != &Token::RBrace && ctx.peek() != &Token::Eof {
            stmts.push(parse_statement(ctx));
            if ctx.peek() == &Token::Semicolon {
                ctx.advance();
            }
        }
        if ctx.peek() == &Token::RBrace {
            ctx.advance();
        }
        Stmt::Block(stmts, span)
    } else {
        parse_statement(ctx)
    }
}

fn stmt_assigns_var(stmt: &Stmt, var: &str) -> bool {
    match stmt {
        Stmt::Let { value, .. } => expr_assigns_var(value, var),
        Stmt::Assign {
            var: assigned_var,
            value,
            ..
        } => assigned_var == var || expr_assigns_var(value, var),
        Stmt::ArrayStore { index, value, .. } => {
            expr_assigns_var(index, var) || expr_assigns_var(value, var)
        }
        Stmt::Block(stmts, _) => stmts.iter().any(|stmt| stmt_assigns_var(stmt, var)),
        Stmt::While {
            cond,
            invariant,
            decreases,
            body,
            ..
        } => {
            expr_assigns_var(cond, var)
                || expr_assigns_var(invariant, var)
                || decreases
                    .as_deref()
                    .is_some_and(|expr| expr_assigns_var(expr, var))
                || stmt_assigns_var(body, var)
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => stmt_assigns_var(body, var),
        Stmt::TaskGroup { children, .. } => children.iter().any(|stmt| stmt_assigns_var(stmt, var)),
        Stmt::Cancel { .. } => false,
        Stmt::Expr(expr, _) => expr_assigns_var(expr, var),
    }
}

fn expr_assigns_var(expr: &Expr, var: &str) -> bool {
    match expr {
        Expr::ArrayLit(items) => items.iter().any(|expr| expr_assigns_var(expr, var)),
        Expr::ArrayAccess(_, index) => expr_assigns_var(index, var),
        Expr::BinaryOp(left, _, right) => {
            expr_assigns_var(left, var) || expr_assigns_var(right, var)
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            expr_assigns_var(cond, var)
                || stmt_assigns_var(then_branch, var)
                || stmt_assigns_var(else_branch, var)
        }
        Expr::Block(stmt) => stmt_assigns_var(stmt, var),
        Expr::Call(_, args) => args.iter().any(|expr| expr_assigns_var(expr, var)),
        Expr::StructInit { fields, .. } => {
            fields.iter().any(|(_, expr)| expr_assigns_var(expr, var))
        }
        Expr::FieldAccess(base, _) => expr_assigns_var(base, var),
        Expr::Match { target, arms } => {
            expr_assigns_var(target, var)
                || arms.iter().any(|arm| {
                    arm.guard
                        .as_deref()
                        .is_some_and(|expr| expr_assigns_var(expr, var))
                        || stmt_assigns_var(&arm.body, var)
                })
        }
        Expr::Async { body } | Expr::Lambda { body, .. } => stmt_assigns_var(body, var),
        Expr::Await { expr } => expr_assigns_var(expr, var),
        Expr::CallRef { callee, args } => {
            expr_assigns_var(callee, var) || args.iter().any(|expr| expr_assigns_var(expr, var))
        }
        Expr::Perform { args, .. } => args.iter().any(|expr| expr_assigns_var(expr, var)),
        Expr::ChanSend { channel, value } => {
            expr_assigns_var(channel, var) || expr_assigns_var(value, var)
        }
        Expr::ChanRecv { channel } => expr_assigns_var(channel, var),
        Expr::Number(_)
        | Expr::Float(_)
        | Expr::StringLit(_)
        | Expr::Variable(_)
        | Expr::AtomRef { .. } => false,
    }
}

/// Parse a single statement.
pub fn parse_statement(ctx: &mut ParseContext) -> Stmt {
    let stmt_span = ctx.current_span();
    match ctx.peek().clone() {
        Token::Let => {
            ctx.advance();
            let var = ctx.expect_ident();
            if ctx.peek() == &Token::Assign {
                ctx.advance();
            }
            let value = parse_expr(ctx, 0);
            Stmt::Let {
                var,
                value: Box::new(value),
                span: stmt_span,
            }
        }

        Token::While => {
            ctx.advance();
            let cond = parse_expr(ctx, 0);
            if ctx.peek() == &Token::Invariant {
                ctx.advance();
                if ctx.peek() == &Token::Colon {
                    ctx.advance();
                }
                let inv = if matches!(ctx.peek(), Token::Ident(name) if name == "infer")
                    && matches!(ctx.peek_at(1), Some(Token::LBrace) | Some(Token::Decreases))
                {
                    ctx.advance();
                    Expr::Variable("__mumei_infer_invariant".to_string())
                } else {
                    parse_expr(ctx, 0)
                };
                let decreases = if ctx.peek() == &Token::Decreases {
                    ctx.advance();
                    if ctx.peek() == &Token::Colon {
                        ctx.advance();
                    }
                    Some(Box::new(parse_expr(ctx, 0)))
                } else {
                    None
                };
                let body = parse_block_or_stmt(ctx);
                Stmt::While {
                    cond: Box::new(cond),
                    invariant: Box::new(inv),
                    decreases,
                    body: Box::new(body),
                    span: stmt_span,
                }
            } else {
                // Same fail-closed recovery as a missing `else`: record the
                // failure and bind a poisoned invariant that can never verify.
                let found = format!("{}", ctx.peek());
                let (line, col) = ctx
                    .tokens_ref()
                    .get(ctx.pos())
                    .map(|t| (t.line, t.col))
                    .unwrap_or((0, 0));
                ctx.syntax_failure(format!(
                    "while loop requires an 'invariant' clause, found {found} at {line}:{col}"
                ));
                let decreases = if ctx.peek() == &Token::Decreases {
                    ctx.advance();
                    if ctx.peek() == &Token::Colon {
                        ctx.advance();
                    }
                    Some(Box::new(parse_expr(ctx, 0)))
                } else {
                    None
                };
                let body = parse_block_or_stmt(ctx);
                Stmt::While {
                    cond: Box::new(cond),
                    invariant: Box::new(Expr::Variable("__mumei_missing_invariant".to_string())),
                    decreases,
                    body: Box::new(body),
                    span: stmt_span,
                }
            }
        }

        Token::For => {
            ctx.advance();
            let var = ctx.expect_ident();
            match ctx.peek().clone() {
                Token::Ident(name) if name == "in" => {
                    ctx.advance();
                }
                found => {
                    let (line, col) = ctx
                        .tokens_ref()
                        .get(ctx.pos())
                        .map(|tok| (tok.line, tok.col))
                        .unwrap_or((0, 0));
                    ctx.syntax_failure(format!(
                        "for loop requires 'in' after loop variable, found {found} at {line}:{col}"
                    ));
                }
            }
            let lo = parse_expr(ctx, 0);
            if ctx.peek() != &Token::DotDot {
                let (line, col) = ctx
                    .tokens_ref()
                    .get(ctx.pos())
                    .map(|tok| (tok.line, tok.col))
                    .unwrap_or((0, 0));
                ctx.syntax_failure(format!(
                    "for loop requires '..' between bounds, found {} at {line}:{col}",
                    ctx.peek()
                ));
            }
            let hi = if ctx.peek() == &Token::DotDot {
                ctx.advance();
                parse_expr(ctx, 0)
            } else {
                Expr::Variable("__mumei_missing_range_bound".to_string())
            };
            let lo_var = format!("__for_lo_{var}");
            let hi_var = format!("__for_hi_{var}");
            let lo_ref = Expr::Variable(lo_var.clone());
            let hi_ref = Expr::Variable(hi_var.clone());
            let user_invariant = if ctx.peek() == &Token::Invariant {
                ctx.advance();
                if ctx.peek() == &Token::Colon {
                    ctx.advance();
                }
                Some(parse_expr(ctx, 0))
            } else {
                None
            };
            let user_decreases = if ctx.peek() == &Token::Decreases {
                ctx.advance();
                if ctx.peek() == &Token::Colon {
                    ctx.advance();
                }
                Some(parse_expr(ctx, 0))
            } else {
                None
            };
            let user_body = parse_block_or_stmt(ctx);
            if stmt_assigns_var(&user_body, &var) {
                ctx.syntax_failure(format!(
                    "for loop body must not assign to loop variable `{var}`"
                ));
            }
            let increment = Stmt::Assign {
                var: var.clone(),
                value: Box::new(Expr::BinaryOp(
                    Box::new(Expr::Variable(var.clone())),
                    Op::Add,
                    Box::new(Expr::Number(1)),
                )),
                span: stmt_span.clone(),
            };
            let loop_body = match user_body {
                Stmt::Block(mut stmts, _) => {
                    stmts.push(increment);
                    Stmt::Block(stmts, stmt_span.clone())
                }
                stmt => Stmt::Block(vec![stmt, increment], stmt_span.clone()),
            };
            let auto_invariant = Expr::BinaryOp(
                Box::new(Expr::BinaryOp(
                    Box::new(lo_ref.clone()),
                    Op::Le,
                    Box::new(Expr::Variable(var.clone())),
                )),
                Op::And,
                Box::new(Expr::BinaryOp(
                    Box::new(Expr::BinaryOp(
                        Box::new(Expr::Variable(var.clone())),
                        Op::Le,
                        Box::new(hi_ref.clone()),
                    )),
                    Op::Or,
                    Box::new(Expr::BinaryOp(
                        Box::new(Expr::Variable(var.clone())),
                        Op::Eq,
                        Box::new(lo_ref.clone()),
                    )),
                )),
            );
            let invariant = if let Some(user_invariant) = user_invariant {
                Expr::BinaryOp(Box::new(auto_invariant), Op::And, Box::new(user_invariant))
            } else {
                auto_invariant
            };
            let decreases = user_decreases.unwrap_or_else(|| {
                Expr::BinaryOp(
                    Box::new(hi_ref.clone()),
                    Op::Sub,
                    Box::new(Expr::Variable(var.clone())),
                )
            });
            Stmt::Block(
                vec![
                    Stmt::Let {
                        var: lo_var,
                        value: Box::new(lo),
                        span: stmt_span.clone(),
                    },
                    Stmt::Let {
                        var: hi_var,
                        value: Box::new(hi),
                        span: stmt_span.clone(),
                    },
                    Stmt::Let {
                        var: var.clone(),
                        value: Box::new(lo_ref),
                        span: stmt_span.clone(),
                    },
                    Stmt::While {
                        cond: Box::new(Expr::BinaryOp(
                            Box::new(Expr::Variable(var)),
                            Op::Lt,
                            Box::new(hi_ref),
                        )),
                        invariant: Box::new(invariant),
                        decreases: Some(Box::new(decreases)),
                        body: Box::new(loop_body),
                        span: stmt_span.clone(),
                    },
                ],
                stmt_span,
            )
        }

        Token::Acquire => {
            ctx.advance();
            let resource = ctx.expect_ident();
            let body = parse_block_or_stmt(ctx);
            Stmt::Acquire {
                resource,
                body: Box::new(body),
                span: stmt_span,
            }
        }

        Token::TaskGroup => {
            ctx.advance();
            let join_semantics = if ctx.peek() == &Token::Colon {
                ctx.advance();
                match ctx.peek().clone() {
                    Token::Ident(ref s) if s == "any" => {
                        ctx.advance();
                        JoinSemantics::Any
                    }
                    Token::Ident(ref s) if s == "all" => {
                        ctx.advance();
                        JoinSemantics::All
                    }
                    ref tok => {
                        ctx.syntax_failure(format!(
                            "unknown task_group join semantics '{tok}' — expected 'all' or 'any'"
                        ));
                        ctx.advance();
                        JoinSemantics::All
                    }
                }
            } else {
                JoinSemantics::All
            };
            let body = parse_block_or_stmt(ctx);
            let children = if let Stmt::Block(stmts, _) = body {
                stmts
            } else {
                vec![body]
            };
            Stmt::TaskGroup {
                children,
                join_semantics,
                span: stmt_span,
            }
        }

        Token::Task => {
            ctx.advance();
            let group = if ctx.peek() != &Token::LBrace && ctx.peek() != &Token::Eof {
                if let Token::Ident(name) = ctx.peek().clone() {
                    ctx.advance();
                    Some(name)
                } else {
                    None
                }
            } else {
                None
            };
            let body = parse_block_or_stmt(ctx);
            Stmt::Task {
                body: Box::new(body),
                group,
                span: stmt_span,
            }
        }

        // Plan 8: Cancel statement — `cancel group_name`
        Token::Cancel => {
            ctx.advance();
            let target = ctx.expect_ident();
            Stmt::Cancel {
                target,
                span: stmt_span,
            }
        }

        // Check for assignment: ident = expr (or array store: ident[idx] = expr)
        Token::Ident(ref name) => {
            let name_clone = name.clone();
            if ctx.peek_at(1) == Some(&Token::Dot)
                && matches!(ctx.peek_at(2), Some(Token::Ident(_)))
                && ctx.peek_at(3) == Some(&Token::Assign)
            {
                let field = match ctx.peek_at(2).cloned() {
                    Some(Token::Ident(field)) => field,
                    _ => unreachable!(),
                };
                ctx.advance(); // consume resource name
                ctx.advance(); // consume dot
                ctx.advance(); // consume field name
                ctx.advance(); // consume =
                let value = parse_expr(ctx, 0);
                Stmt::Assign {
                    var: format!("{name_clone}.{field}"),
                    value: Box::new(value),
                    span: stmt_span,
                }
            } else
            // Peek ahead for direct variable assignment
            if ctx.peek_at(1).is_some_and(|t| *t == Token::Assign)
                && ctx
                    .peek_at(2)
                    .is_none_or(|t| *t != Token::Assign && *t != Token::Gt)
            {
                // It's an assignment: var = expr (but not == or =>)
                ctx.advance(); // consume ident
                ctx.advance(); // consume =
                let value = parse_expr(ctx, 0);
                Stmt::Assign {
                    var: name_clone,
                    value: Box::new(value),
                    span: stmt_span,
                }
            } else {
                // Fall through to expression parsing, then detect array-store
                // pattern: an expression that is `arr[idx]` followed by `=` is
                // promoted from a statement-level expression to `Stmt::ArrayStore`.
                let expr = parse_expr(ctx, 0);
                try_promote_to_array_store(ctx, expr, stmt_span)
            }
        }

        // Check for assignment with keyword-named variables (e.g., mode = expr)
        // The lexer converts keywords like "mode", "priority" to typed tokens,
        // so they don't match Token::Ident above. This mirrors the old parser's
        // behavior which checked is_alphabetic() on any token string.
        ref tok => {
            let name = format!("{}", tok);
            if name.chars().next().is_some_and(|c| c.is_alphabetic())
                && ctx.peek_at(1).is_some_and(|t| *t == Token::Assign)
                && ctx
                    .peek_at(2)
                    .is_none_or(|t| *t != Token::Assign && *t != Token::Gt)
            {
                ctx.advance(); // consume keyword token
                ctx.advance(); // consume =
                let value = parse_expr(ctx, 0);
                Stmt::Assign {
                    var: name,
                    value: Box::new(value),
                    span: stmt_span,
                }
            } else {
                let expr = parse_expr(ctx, 0);
                try_promote_to_array_store(ctx, expr, stmt_span)
            }
        }
    }
}

/// If `expr` is an `Expr::ArrayAccess(arr, idx)` and the next token is `=`
/// (and not `==` / `=>`), consume the `=`, parse the RHS and produce
/// `Stmt::ArrayStore`. Otherwise wrap `expr` in `Stmt::Expr` unchanged.
fn try_promote_to_array_store(ctx: &mut ParseContext, expr: Expr, stmt_span: super::Span) -> Stmt {
    let is_store = matches!(expr, Expr::ArrayAccess(_, _))
        && ctx.peek() == &Token::Assign
        && ctx
            .peek_at(1)
            .is_none_or(|t| *t != Token::Assign && *t != Token::Gt);
    if is_store {
        if let Expr::ArrayAccess(array, index) = expr {
            ctx.advance(); // consume =
            let value = parse_expr(ctx, 0);
            return Stmt::ArrayStore {
                array,
                index,
                value: Box::new(value),
                span: stmt_span,
            };
        }
    }
    Stmt::Expr(expr, stmt_span)
}
