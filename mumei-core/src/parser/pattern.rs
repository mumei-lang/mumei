// =============================================================================
// Pattern parsing
// =============================================================================

use super::token::Token;
use super::ParseContext;
use crate::parser::Pattern;

/// Parse a pattern from the token stream.
pub fn parse_pattern(ctx: &mut ParseContext) -> Pattern {
    match ctx.peek().clone() {
        Token::Ident(ref name) if name == "_" => {
            ctx.advance();
            Pattern::Wildcard
        }

        Token::Minus => {
            // Negative literal: -N
            if let Token::IntLit(n) = ctx.peek_at(1).cloned().unwrap_or(Token::Eof) {
                ctx.advance(); // -
                ctx.advance(); // N
                Pattern::Literal(-n)
            } else {
                ctx.advance();
                Pattern::Wildcard
            }
        }

        Token::IntLit(n) => {
            ctx.advance();
            Pattern::Literal(n)
        }

        Token::Ident(ref name) => {
            let name = name.clone();
            ctx.advance();

            if name.chars().next().is_some_and(|c| c.is_uppercase()) {
                // Uppercase → Variant pattern
                if ctx.peek() == &Token::LParen {
                    ctx.advance(); // (
                    let mut fields = Vec::new();
                    while ctx.peek() != &Token::RParen && ctx.peek() != &Token::Eof {
                        fields.push(parse_pattern(ctx));
                        if ctx.peek() == &Token::Comma {
                            ctx.advance();
                        }
                    }
                    ctx.expect(Token::RParen);
                    Pattern::Variant {
                        variant_name: name,
                        fields,
                    }
                } else {
                    // Unit variant (no parens)
                    Pattern::Variant {
                        variant_name: name,
                        fields: vec![],
                    }
                }
            } else {
                // Lowercase → variable binding
                Pattern::Variable(name)
            }
        }

        // Some keywords may appear as pattern names (e.g. "true", "false")
        Token::True => {
            ctx.advance();
            Pattern::Variable("true".to_string())
        }
        Token::False => {
            ctx.advance();
            Pattern::Variable("false".to_string())
        }

        _ => {
            ctx.advance();
            Pattern::Wildcard
        }
    }
}
