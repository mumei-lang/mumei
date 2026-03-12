// =============================================================================
// Expression and Statement parsing (Pratt parser + statement parser)
// =============================================================================

use super::pattern::parse_pattern;
use super::token::Token;
use super::ParseContext;
use crate::parser::{Expr, JoinSemantics, LambdaParam, MatchArm, Op, Stmt};

/// Pratt parser binding power for binary operators.
/// Returns (left_bp, right_bp). Left-associative: left < right.
/// Right-associative: left > right.
fn binding_power(tok: &Token) -> Option<(u8, u8)> {
    match tok {
        Token::FatArrow => Some((1, 2)), // left-assoc => (matches old parser's while-loop behavior)
        Token::Or => Some((3, 4)),
        Token::And => Some((5, 6)),
        Token::Eq | Token::Neq | Token::Gt | Token::Lt | Token::Ge | Token::Le => Some((7, 8)),
        Token::Plus | Token::Minus => Some((9, 10)),
        Token::Star | Token::Slash => Some((11, 12)),
        Token::Dot => Some((15, 16)),
        _ => None,
    }
}

fn token_to_op(tok: &Token) -> Option<Op> {
    match tok {
        Token::Plus => Some(Op::Add),
        Token::Minus => Some(Op::Sub),
        Token::Star => Some(Op::Mul),
        Token::Slash => Some(Op::Div),
        Token::Eq => Some(Op::Eq),
        Token::Neq => Some(Op::Neq),
        Token::Gt => Some(Op::Gt),
        Token::Lt => Some(Op::Lt),
        Token::Ge => Some(Op::Ge),
        Token::Le => Some(Op::Le),
        Token::And => Some(Op::And),
        Token::Or => Some(Op::Or),
        Token::FatArrow => Some(Op::Implies),
        _ => None,
    }
}

/// Parse an expression using Pratt parsing with minimum binding power.
pub fn parse_expr(ctx: &mut ParseContext, min_bp: u8) -> Expr {
    let mut lhs = parse_prefix(ctx);

    loop {
        let tok = ctx.peek().clone();
        // Dot is handled specially for field access
        if tok == Token::Dot {
            if let Some((l_bp, _)) = binding_power(&tok) {
                if l_bp < min_bp {
                    break;
                }
                ctx.advance(); // consume .
                if let Token::Ident(field) = ctx.peek().clone() {
                    ctx.advance();
                    lhs = Expr::FieldAccess(Box::new(lhs), field);
                    continue;
                } else {
                    // Accept keyword tokens as field names (e.g., obj.mode, obj.priority)
                    let field_name = format!("{}", ctx.peek());
                    if field_name
                        .chars()
                        .next()
                        .map_or(false, |c| c.is_alphabetic())
                    {
                        ctx.advance();
                        lhs = Expr::FieldAccess(Box::new(lhs), field_name);
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

    lhs
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
                panic!("Mumei requires an 'else' branch.");
            }
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
                    // Parse guard at binding power above => (l_bp=1, so min_bp=3 excludes it)
                    Some(Box::new(parse_expr(ctx, 3)))
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

        Token::True => {
            ctx.advance();
            Expr::Variable("true".to_string())
        }

        Token::False => {
            ctx.advance();
            Expr::Variable("false".to_string())
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
            // Optional return type: -> Type
            let return_type = if ctx.peek() == &Token::Arrow {
                ctx.advance(); // skip ->
                let type_name = ctx.expect_ident();
                Some(type_name)
            } else {
                None
            };
            // Parse body: either { stmts } or a single expression
            let body = if ctx.peek() == &Token::LBrace {
                parse_block_or_stmt(ctx)
            } else {
                Stmt::Expr(parse_expr(ctx, 0))
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
            if name.chars().next().map_or(false, |c| c.is_alphabetic()) {
                ctx.advance();
                parse_ident_continuation(ctx, name)
            } else {
                ctx.advance();
                Expr::Number(0)
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
        Token::LBrace if name.chars().next().map_or(false, |c| c.is_uppercase()) => {
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

        _ => Expr::Variable(name),
    }
}

/// Parse match arm body.
/// Uses parse_expr with binding power above => to avoid consuming => as implies.
fn parse_match_arm_body(ctx: &mut ParseContext) -> Stmt {
    if ctx.peek() == &Token::LBrace {
        parse_block_or_stmt(ctx)
    } else if ctx.peek() == &Token::Match || ctx.peek() == &Token::If {
        Stmt::Expr(parse_expr(ctx, 0))
    } else {
        // Parse at binding power 3, above => (l_bp=1) so => is not consumed
        Stmt::Expr(parse_expr(ctx, 3))
    }
}

/// Parse a block or single statement.
pub fn parse_block_or_stmt(ctx: &mut ParseContext) -> Stmt {
    if ctx.peek() == &Token::LBrace {
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
        Stmt::Block(stmts)
    } else {
        parse_statement(ctx)
    }
}

/// Parse a single statement.
pub fn parse_statement(ctx: &mut ParseContext) -> Stmt {
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
                let inv = parse_expr(ctx, 0);
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
                }
            } else {
                panic!("Mumei loops require an 'invariant'.");
            }
        }

        Token::Acquire => {
            ctx.advance();
            let resource = ctx.expect_ident();
            let body = parse_block_or_stmt(ctx);
            Stmt::Acquire {
                resource,
                body: Box::new(body),
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
                        panic!(
                            "Unknown task_group join semantics '{}'. Expected 'all' or 'any'.",
                            tok
                        );
                    }
                }
            } else {
                JoinSemantics::All
            };
            let body = parse_block_or_stmt(ctx);
            let children = if let Stmt::Block(stmts) = body {
                stmts
            } else {
                vec![body]
            };
            Stmt::TaskGroup {
                children,
                join_semantics,
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
            }
        }

        // Check for assignment: ident = expr
        Token::Ident(ref name) => {
            let name_clone = name.clone();
            // Peek ahead for assignment
            if ctx.peek_at(1).map_or(false, |t| *t == Token::Assign)
                && ctx
                    .peek_at(2)
                    .map_or(true, |t| *t != Token::Assign && *t != Token::Gt)
            {
                // It's an assignment: var = expr (but not == or =>)
                ctx.advance(); // consume ident
                ctx.advance(); // consume =
                let value = parse_expr(ctx, 0);
                Stmt::Assign {
                    var: name_clone,
                    value: Box::new(value),
                }
            } else {
                Stmt::Expr(parse_expr(ctx, 0))
            }
        }

        // Check for assignment with keyword-named variables (e.g., mode = expr)
        // The lexer converts keywords like "mode", "priority" to typed tokens,
        // so they don't match Token::Ident above. This mirrors the old parser's
        // behavior which checked is_alphabetic() on any token string.
        ref tok => {
            let name = format!("{}", tok);
            if name.chars().next().map_or(false, |c| c.is_alphabetic())
                && ctx.peek_at(1).map_or(false, |t| *t == Token::Assign)
                && ctx
                    .peek_at(2)
                    .map_or(true, |t| *t != Token::Assign && *t != Token::Gt)
            {
                ctx.advance(); // consume keyword token
                ctx.advance(); // consume =
                let value = parse_expr(ctx, 0);
                Stmt::Assign {
                    var: name,
                    value: Box::new(value),
                }
            } else {
                Stmt::Expr(parse_expr(ctx, 0))
            }
        }
    }
}
