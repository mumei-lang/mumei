// =============================================================================
// Top-level item parsing (token-based recursive descent -- no regex)
// =============================================================================

use crate::ast::TypeRef;
use crate::parser::{
    Atom, Effect, EffectDef, EffectDefParam, EffectParam, EnumDef, EnumVariant, ExternBlock,
    ExternFn, ImplBlock, ImplDef, ImportDecl, Item, Param, Quantifier, QuantifierType, RefinedType,
    ResourceDef, ResourceMode, Span, StructDef, StructField, TraitDef, TraitMethod, TrustLevel,
    TypeParamBound,
};

use super::token::{SpannedToken, Token};
use super::ParseContext;

// ---- Helper functions (preserved from old parser) ----

fn parse_type_params_from_str(input: &str) -> (Vec<String>, usize) {
    if !input.starts_with('<') {
        return (vec![], 0);
    }
    let mut depth = 0;
    let mut end = 0;
    for (i, c) in input.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    if end == 0 {
        return (vec![], 0);
    }
    let inner = &input[1..end];
    let params: Vec<String> = inner
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    (params, end + 1)
}

pub fn parse_type_ref(input: &str) -> TypeRef {
    let input = input.trim();

    if let Some(after_paren) = input.strip_prefix("atom_ref(") {
        let mut depth = 1;
        let mut close_pos = 0;
        for (i, c) in after_paren.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        close_pos = i;
                        break;
                    }
                }
                _ => {}
            }
        }
        let params_str = &after_paren[..close_pos];
        let param_types: Vec<TypeRef> = if params_str.trim().is_empty() {
            vec![]
        } else {
            split_type_args(params_str)
                .iter()
                .map(|a| parse_type_ref(a))
                .collect()
        };

        let rest = after_paren[close_pos + 1..].trim();
        let (return_type, effect_set) = if let Some(arrow_rest) = rest.strip_prefix("->") {
            let arrow_rest = arrow_rest.trim();
            if let Some(with_pos) = arrow_rest.find(" with ") {
                let return_type_str = &arrow_rest[..with_pos];
                let effect_part = arrow_rest[with_pos + 6..].trim();
                let effects = if effect_part.starts_with('[') && effect_part.ends_with(']') {
                    effect_part[1..effect_part.len() - 1]
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                } else {
                    vec![effect_part.to_string()]
                };
                (parse_type_ref(return_type_str.trim()), Some(effects))
            } else {
                (parse_type_ref(arrow_rest), None)
            }
        } else {
            (TypeRef::simple("i64"), None)
        };

        let mut type_ref = TypeRef::fn_type(param_types, return_type);
        type_ref.effect_set = effect_set;
        return type_ref;
    }

    if let Some(angle_pos) = input.find('<') {
        let name = input[..angle_pos].trim().to_string();
        let inner = if input.ends_with('>') {
            &input[angle_pos + 1..input.len() - 1]
        } else {
            &input[angle_pos + 1..]
        };
        let args = split_type_args(inner);
        let type_args: Vec<TypeRef> = args.iter().map(|a| parse_type_ref(a)).collect();
        TypeRef::generic(&name, type_args)
    } else {
        TypeRef::simple(input)
    }
}

pub fn parse_type_ref_from_ctx(ctx: &mut super::ParseContext) -> TypeRef {
    let mut type_str = String::new();
    match ctx.peek().clone() {
        Token::Ident(s) => {
            type_str.push_str(&s);
            ctx.advance();
        }
        ref tok => {
            let name = format!("{}", tok);
            if name.chars().next().is_some_and(|c| c.is_alphabetic()) {
                type_str.push_str(&name);
                ctx.advance();
            }
        }
    }
    if ctx.peek() == &Token::Lt {
        type_str.push('<');
        ctx.advance();
        let mut depth = 1;
        while depth > 0 && ctx.peek() != &Token::Eof {
            match ctx.peek().clone() {
                Token::Lt => {
                    depth += 1;
                    type_str.push('<');
                    ctx.advance();
                }
                Token::Gt => {
                    depth -= 1;
                    type_str.push('>');
                    ctx.advance();
                }
                Token::Comma => {
                    type_str.push_str(", ");
                    ctx.advance();
                }
                ref tok => {
                    type_str.push_str(&format!("{}", tok));
                    ctx.advance();
                }
            }
        }
    }
    parse_type_ref(&type_str)
}

fn split_type_args(input: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut angle_depth = 0;
    let mut paren_depth = 0;
    let mut current = String::new();
    for c in input.chars() {
        match c {
            '<' => {
                angle_depth += 1;
                current.push(c);
            }
            '>' => {
                angle_depth -= 1;
                current.push(c);
            }
            '(' => {
                paren_depth += 1;
                current.push(c);
            }
            ')' => {
                paren_depth -= 1;
                current.push(c);
            }
            ',' if angle_depth == 0 && paren_depth == 0 => {
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    result.push(trimmed);
                }
                current.clear();
            }
            _ => {
                current.push(c);
            }
        }
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        result.push(trimmed);
    }
    result
}

fn parse_type_params_with_bounds(input: &str) -> (Vec<String>, Vec<TypeParamBound>) {
    let (raw_params, _) = parse_type_params_from_str(input);
    let mut type_params = Vec::new();
    let mut bounds = Vec::new();
    for raw in &raw_params {
        if let Some((param, bound_str)) = raw.split_once(':') {
            let param = param.trim().to_string();
            let param_bounds: Vec<String> = bound_str
                .split('+')
                .map(|b| b.trim().to_string())
                .filter(|b| !b.is_empty())
                .collect();
            bounds.push(TypeParamBound {
                param: param.clone(),
                bounds: param_bounds,
            });
            type_params.push(param);
        } else {
            type_params.push(raw.trim().to_string());
        }
    }
    (type_params, bounds)
}

fn split_variants(input: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut depth = 0;
    let mut current = String::new();
    for c in input.chars() {
        match c {
            '(' => {
                depth += 1;
                current.push(c);
            }
            ')' => {
                depth -= 1;
                current.push(c);
            }
            ',' if depth == 0 => {
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    result.push(trimmed);
                }
                current.clear();
            }
            _ => {
                current.push(c);
            }
        }
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        result.push(trimmed);
    }
    result
}

fn split_params(input: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut depth = 0;
    let mut current = String::new();
    for c in input.chars() {
        match c {
            '(' => {
                depth += 1;
                current.push(c);
            }
            ')' => {
                depth -= 1;
                current.push(c);
            }
            ',' if depth == 0 => {
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    result.push(trimmed);
                }
                current.clear();
            }
            _ => {
                current.push(c);
            }
        }
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        result.push(trimmed);
    }
    result
}

fn parse_effect_list(input: &str) -> Vec<Effect> {
    let mut effects = Vec::new();
    let input = input.trim();
    if input.is_empty() {
        return effects;
    }
    let mut depth = 0;
    let mut current_start = 0;
    for (byte_idx, ch) in input.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                let token = input[current_start..byte_idx].trim();
                if !token.is_empty() {
                    effects.push(parse_single_effect(token));
                }
                current_start = byte_idx + ch.len_utf8();
            }
            _ => {}
        }
    }
    let token = input[current_start..].trim();
    if !token.is_empty() {
        effects.push(parse_single_effect(token));
    }
    effects
}

fn parse_single_effect(input: &str) -> Effect {
    let input = input.trim();
    // Plan 6: Detect negated effects (e.g., `!IO`)
    let (negated, input) = if let Some(stripped) = input.strip_prefix('!') {
        (true, stripped.trim())
    } else {
        (false, input)
    };
    if let Some(paren_start) = input.find('(') {
        let name = input[..paren_start].trim().to_string();
        let paren_end = input.rfind(')').unwrap_or(input.len());
        let params_str = &input[paren_start + 1..paren_end];
        let params = parse_effect_params(params_str);
        Effect {
            name,
            params,
            span: Span::default(),
            negated,
        }
    } else {
        Effect {
            name: input.to_string(),
            params: vec![],
            span: Span::default(),
            negated,
        }
    }
}

fn parse_effect_params(input: &str) -> Vec<EffectParam> {
    let mut params = Vec::new();
    let mut in_quote = false;
    let mut current_start = 0;
    for (byte_idx, ch) in input.char_indices() {
        match ch {
            '"' => in_quote = !in_quote,
            ',' if !in_quote => {
                let part = input[current_start..byte_idx].trim();
                if !part.is_empty() {
                    params.push(parse_single_effect_param(part));
                }
                current_start = byte_idx + ch.len_utf8();
            }
            _ => {}
        }
    }
    let part = input[current_start..].trim();
    if !part.is_empty() {
        params.push(parse_single_effect_param(part));
    }
    params
}

fn parse_single_effect_param(part: &str) -> EffectParam {
    if (part.starts_with('"') && part.ends_with('"'))
        || (part.starts_with('\'') && part.ends_with('\''))
    {
        let value = part[1..part.len() - 1].to_string();
        EffectParam {
            value,
            refinement: None,
            is_constant: true,
        }
    } else {
        EffectParam {
            value: part.to_string(),
            refinement: None,
            is_constant: false,
        }
    }
}

// =============================================================================
// Token-based helpers
// =============================================================================

fn span_from_token(tok: &SpannedToken) -> Span {
    Span::new("", tok.line, tok.col, tok.len)
}

/// Append a token's text to a string with smart spacing:
/// no space before `)`, `,`, `;`, `:`, `.`, `]`
/// no space after `(`, `.`, `[`
fn append_token(text: &mut String, tok: &Token) {
    let no_leading_space = matches!(
        tok,
        Token::RParen
            | Token::Comma
            | Token::Semicolon
            | Token::Colon
            | Token::RBrace
            | Token::RBracket
            | Token::Dot
            | Token::LParen
            | Token::LBracket
    );
    let after_no_space = text.ends_with('(') || text.ends_with('.') || text.ends_with('[');
    if !text.is_empty() && !no_leading_space && !after_no_space {
        text.push(' ');
    }
    match tok {
        Token::StringLit(s) => {
            text.push('"');
            text.push_str(s);
            text.push('"');
        }
        other => text.push_str(&format!("{}", other)),
    }
}

fn collect_until_semicolon(ctx: &mut ParseContext) -> String {
    let mut text = String::new();
    let mut depth_brace = 0i32;
    let mut depth_paren = 0i32;
    let mut depth_bracket = 0i32;
    loop {
        match ctx.peek() {
            Token::Eof => break,
            Token::Semicolon if depth_brace == 0 && depth_paren == 0 && depth_bracket == 0 => break,
            _ => {}
        }
        let tok = ctx.advance().clone();
        match &tok.token {
            Token::LBrace => depth_brace += 1,
            Token::RBrace => depth_brace -= 1,
            Token::LParen => depth_paren += 1,
            Token::RParen => depth_paren -= 1,
            Token::LBracket => depth_bracket += 1,
            Token::RBracket => depth_bracket -= 1,
            _ => {}
        }
        append_token(&mut text, &tok.token);
    }
    text
}

fn collect_brace_body(ctx: &mut ParseContext) -> String {
    let mut text = String::new();
    let mut depth = 0i32;
    loop {
        match ctx.peek() {
            Token::Eof => break,
            Token::RBrace if depth == 0 => break,
            _ => {}
        }
        let tok = ctx.advance().clone();
        match &tok.token {
            Token::LBrace => depth += 1,
            Token::RBrace => depth -= 1,
            _ => {}
        }
        append_token(&mut text, &tok.token);
    }
    text
}

fn collect_braced_block(ctx: &mut ParseContext) -> String {
    if ctx.peek() == &Token::LBrace {
        ctx.advance();
    }
    let body = collect_brace_body(ctx);
    if ctx.peek() == &Token::RBrace {
        ctx.advance();
    }
    body
}

fn parse_type_params_from_ctx(ctx: &mut ParseContext) -> Vec<String> {
    if ctx.peek() != &Token::Lt {
        return vec![];
    }
    let text = collect_angle_brackets(ctx);
    let (params, _) = parse_type_params_from_str(&text);
    params
}

fn collect_angle_brackets(ctx: &mut ParseContext) -> String {
    let mut text = String::new();
    let mut depth = 0;
    loop {
        match ctx.peek() {
            Token::Lt => {
                depth += 1;
                text.push('<');
                ctx.advance();
            }
            Token::Gt => {
                depth -= 1;
                text.push('>');
                ctx.advance();
                if depth == 0 {
                    break;
                }
            }
            Token::Eof => break,
            _ => {
                let tok = ctx.advance();
                text.push_str(&format!("{}", tok.token));
            }
        }
    }
    text
}

fn collect_balanced_parens(ctx: &mut ParseContext) -> String {
    if ctx.peek() != &Token::LParen {
        return String::new();
    }
    ctx.advance();
    let mut text = String::new();
    let mut depth = 1;
    loop {
        match ctx.peek() {
            Token::Eof => break,
            Token::LParen => {
                depth += 1;
                text.push('(');
                ctx.advance();
            }
            Token::RParen => {
                depth -= 1;
                if depth == 0 {
                    ctx.advance();
                    break;
                }
                text.push(')');
                ctx.advance();
            }
            _ => {
                let tok = ctx.advance();
                if !text.is_empty()
                    && !text.ends_with('(')
                    && !matches!(tok.token, Token::Comma | Token::RParen)
                {
                    text.push(' ');
                }
                match &tok.token {
                    Token::StringLit(s) => {
                        text.push('"');
                        text.push_str(s);
                        text.push('"');
                    }
                    other => text.push_str(&format!("{}", other)),
                }
            }
        }
    }
    text
}

fn skip_balanced(ctx: &mut ParseContext, open: Token, close: Token) {
    if ctx.peek() != &open {
        return;
    }
    ctx.advance();
    let mut depth = 1;
    while depth > 0 && ctx.peek() != &Token::Eof {
        if *ctx.peek() == open {
            depth += 1;
        } else if *ctx.peek() == close {
            depth -= 1;
        }
        ctx.advance();
    }
}

fn parse_bracket_list(ctx: &mut ParseContext) -> Vec<String> {
    let mut result = Vec::new();
    if ctx.peek() != &Token::LBracket {
        return result;
    }
    ctx.advance();
    while ctx.peek() != &Token::RBracket && ctx.peek() != &Token::Eof {
        let name = ctx.expect_ident();
        result.push(name);
        if ctx.peek() == &Token::Comma {
            ctx.advance();
        }
    }
    ctx.expect(Token::RBracket);
    result
}

fn parse_integer_literal(ctx: &mut ParseContext) -> i64 {
    let negative = if ctx.peek() == &Token::Minus {
        ctx.advance();
        true
    } else {
        false
    };
    if let Token::IntLit(n) = ctx.peek().clone() {
        ctx.advance();
        if negative {
            -n
        } else {
            n
        }
    } else {
        0
    }
}

fn collect_body(ctx: &mut ParseContext) -> String {
    if ctx.peek() == &Token::LBrace {
        let mut text = String::from("{");
        ctx.advance();
        let mut depth = 1;
        while depth > 0 && ctx.peek() != &Token::Eof {
            let tok = ctx.advance().clone();
            match &tok.token {
                Token::LBrace => {
                    depth += 1;
                    text.push_str(" {");
                }
                Token::RBrace => {
                    depth -= 1;
                    if depth == 0 {
                        text.push_str(" }");
                        break;
                    }
                    text.push_str(" }");
                }
                other => {
                    append_token(&mut text, other);
                }
            }
        }
        text
    } else {
        let mut text = String::new();
        let mut depth_brace = 0i32;
        let mut depth_paren = 0i32;
        loop {
            if depth_brace == 0 && depth_paren == 0 {
                match ctx.peek() {
                    Token::Eof
                    | Token::Atom
                    | Token::Struct
                    | Token::Enum
                    | Token::Trait
                    | Token::Impl
                    | Token::Import
                    | Token::Extern
                    | Token::Resource
                    | Token::Effect
                    | Token::Async
                    | Token::Trusted
                    | Token::Unverified
                    | Token::Semicolon
                    | Token::RBrace => break,
                    _ => {}
                }
            }
            let tok = ctx.advance().clone();
            match &tok.token {
                Token::LBrace => depth_brace += 1,
                Token::RBrace => depth_brace -= 1,
                Token::LParen => depth_paren += 1,
                Token::RParen => depth_paren -= 1,
                _ => {}
            }
            append_token(&mut text, &tok.token);
        }
        text
    }
}

// =============================================================================
// Token-based module parser
// =============================================================================

pub fn parse_module_from_tokens(ctx: &mut ParseContext) -> Vec<Item> {
    let mut items = Vec::new();

    loop {
        match ctx.peek().clone() {
            Token::Eof => break,

            Token::Import => {
                let start_tok = ctx.advance().clone();
                if let Token::StringLit(path) = ctx.peek().clone() {
                    ctx.advance();
                    let alias = if ctx.peek() == &Token::As {
                        ctx.advance();
                        Some(ctx.expect_ident())
                    } else {
                        None
                    };
                    ctx.expect(Token::Semicolon);
                    items.push(Item::Import(ImportDecl {
                        span: span_from_token(&start_tok),
                        path,
                        alias,
                    }));
                }
            }

            Token::Type => {
                let start_tok = ctx.advance().clone();
                let name = ctx.expect_ident();
                ctx.expect(Token::Assign);
                let base_type = ctx.expect_ident();
                ctx.expect(Token::Where);
                let predicate_raw = collect_until_semicolon(ctx);
                ctx.expect(Token::Semicolon);
                let tokens = super::lexer::legacy_tokenize(&predicate_raw);
                let operand = tokens.first().cloned().unwrap_or_else(|| "v".to_string());
                items.push(Item::TypeDef(RefinedType {
                    name,
                    _base_type: base_type,
                    operand,
                    predicate_raw,
                    span: span_from_token(&start_tok),
                }));
            }

            Token::Struct => {
                let start_tok = ctx.advance().clone();
                let name = ctx.expect_ident();
                let type_params = parse_type_params_from_ctx(ctx);
                let fields_raw = collect_braced_block(ctx);
                let fields: Vec<StructField> = fields_raw
                    .split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .map(|s| {
                        let (field_part, constraint) = if let Some(idx) = s.find("where") {
                            (s[..idx].trim(), Some(s[idx + 5..].trim().to_string()))
                        } else {
                            (s.trim(), None)
                        };
                        let parts: Vec<&str> = field_part.splitn(2, ':').collect();
                        let type_name_str = parts
                            .get(1)
                            .map(|t| t.trim().to_string())
                            .unwrap_or_else(|| "i64".to_string());
                        let type_ref = parse_type_ref(&type_name_str);
                        StructField {
                            name: parts[0].trim().to_string(),
                            type_name: type_name_str,
                            type_ref,
                            constraint,
                        }
                    })
                    .collect();
                items.push(Item::StructDef(StructDef {
                    name,
                    type_params,
                    fields,
                    method_names: vec![],
                    methods: vec![],
                    span: span_from_token(&start_tok),
                }));
            }

            Token::Enum => {
                let start_tok = ctx.advance().clone();
                let name = ctx.expect_ident();
                let type_params = parse_type_params_from_ctx(ctx);
                let variants_raw = collect_braced_block(ctx);
                let mut any_recursive = false;
                let variant_strs = split_variants(&variants_raw);
                let variants: Vec<EnumVariant> = variant_strs
                    .iter()
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .map(|s| {
                        if let Some(paren_start) = s.find('(') {
                            let variant_name = s[..paren_start].trim().to_string();
                            let fields_str = &s[paren_start + 1..s.rfind(')').unwrap_or(s.len())];
                            let fields: Vec<String> = fields_str
                                .split(',')
                                .map(|f| {
                                    let f = f.trim().to_string();
                                    if f == "Self" {
                                        name.clone()
                                    } else {
                                        f
                                    }
                                })
                                .filter(|f| !f.is_empty())
                                .collect();
                            let field_types: Vec<TypeRef> =
                                fields.iter().map(|f| parse_type_ref(f)).collect();
                            let is_recursive = fields.iter().any(|f| f == &name);
                            if is_recursive {
                                any_recursive = true;
                            }
                            EnumVariant {
                                name: variant_name,
                                fields,
                                field_types,
                                is_recursive,
                            }
                        } else {
                            EnumVariant {
                                name: s.to_string(),
                                fields: vec![],
                                field_types: vec![],
                                is_recursive: false,
                            }
                        }
                    })
                    .collect();
                items.push(Item::EnumDef(EnumDef {
                    name,
                    type_params,
                    variants,
                    is_recursive: any_recursive,
                    span: span_from_token(&start_tok),
                }));
            }

            Token::Trait => {
                let start_tok = ctx.advance().clone();
                let name = ctx.expect_ident();
                let body = collect_braced_block(ctx);
                let mut methods = Vec::new();
                let mut laws = Vec::new();
                // Token reconstruction produces single-line text; use flat parser
                // which scans for "fn " and "law " prefixes regardless of newlines.
                parse_trait_body_flat(&body, &mut methods, &mut laws);
                items.push(Item::TraitDef(TraitDef {
                    name,
                    methods,
                    laws,
                    span: span_from_token(&start_tok),
                }));
            }

            Token::Impl => {
                let start_tok = ctx.advance().clone();
                let first_name = ctx.expect_ident();

                // Distinguish: `impl Trait for Type { fn ... }` vs `impl StructName { atom ... }`
                if ctx.peek() == &Token::For {
                    // --- trait impl: `impl Trait for Type { fn ... }` ---
                    ctx.advance();
                    let target_type = ctx.expect_ident();
                    if ctx.peek() == &Token::LBrace {
                        ctx.advance();
                    }
                    let mut method_bodies = Vec::new();
                    while ctx.peek() != &Token::RBrace && ctx.peek() != &Token::Eof {
                        if ctx.peek() == &Token::Fn {
                            ctx.advance();
                            let method_name = ctx.expect_ident();
                            if ctx.peek() == &Token::LParen {
                                skip_balanced(ctx, Token::LParen, Token::RParen);
                            }
                            if ctx.peek() == &Token::Arrow {
                                ctx.advance();
                                ctx.expect_ident();
                            }
                            let method_body = collect_braced_block(ctx);
                            method_bodies.push((method_name, method_body));
                        } else {
                            ctx.advance();
                        }
                    }
                    if ctx.peek() == &Token::RBrace {
                        ctx.advance();
                    }
                    items.push(Item::ImplDef(ImplDef {
                        trait_name: first_name,
                        target_type,
                        method_bodies,
                        span: span_from_token(&start_tok),
                    }));
                } else {
                    // --- struct impl block: `impl StructName { atom method(...) ... }` ---
                    let struct_name = first_name;
                    if ctx.peek() == &Token::LBrace {
                        ctx.advance();
                    }
                    let mut methods = Vec::new();
                    while ctx.peek() != &Token::RBrace && ctx.peek() != &Token::Eof {
                        // Parse optional modifiers (async, trusted, unverified)
                        let mut is_async = false;
                        let mut trust_level = TrustLevel::Verified;
                        loop {
                            match ctx.peek() {
                                Token::Async => {
                                    is_async = true;
                                    ctx.advance();
                                }
                                Token::Trusted => {
                                    trust_level = TrustLevel::Trusted;
                                    ctx.advance();
                                }
                                Token::Unverified => {
                                    trust_level = TrustLevel::Unverified;
                                    ctx.advance();
                                }
                                _ => break,
                            }
                        }
                        if ctx.peek() == &Token::Atom {
                            let atom_tok = ctx.tokens_ref()[ctx.pos()].clone();
                            ctx.advance(); // consume `atom`
                            let mut atom = parse_atom_body(ctx, &atom_tok);
                            atom.is_async = is_async;
                            atom.trust_level = trust_level;
                            methods.push(atom);
                        } else {
                            ctx.advance();
                        }
                    }
                    if ctx.peek() == &Token::RBrace {
                        ctx.advance();
                    }
                    items.push(Item::ImplBlock(ImplBlock {
                        struct_name,
                        methods,
                        span: span_from_token(&start_tok),
                    }));
                }
            }

            Token::Resource => {
                let start_tok = ctx.advance().clone();
                let name = ctx.expect_ident();
                ctx.expect(Token::Priority);
                ctx.expect(Token::Colon);
                let priority = parse_integer_literal(ctx);
                ctx.expect(Token::Mode);
                ctx.expect(Token::Colon);
                let mode = match ctx.peek() {
                    Token::Exclusive => {
                        ctx.advance();
                        ResourceMode::Exclusive
                    }
                    Token::Shared => {
                        ctx.advance();
                        ResourceMode::Shared
                    }
                    _ => {
                        let ident = ctx.expect_ident();
                        if ident == "exclusive" {
                            ResourceMode::Exclusive
                        } else {
                            ResourceMode::Shared
                        }
                    }
                };
                ctx.expect(Token::Semicolon);
                items.push(Item::ResourceDef(ResourceDef {
                    name,
                    priority,
                    mode,
                    span: span_from_token(&start_tok),
                }));
            }

            Token::Effect => {
                let start_tok = ctx.advance().clone();
                let name = ctx.expect_ident();

                // Plan 6: Effect alias syntax — `effect IO = FileRead | FileWrite;`
                if ctx.peek() == &Token::Assign {
                    ctx.advance(); // consume `=`
                    let mut alias_names = Vec::new();
                    alias_names.push(ctx.expect_ident());
                    while ctx.peek() == &Token::Bar {
                        ctx.advance(); // consume `|`
                        alias_names.push(ctx.expect_ident());
                    }
                    ctx.expect(Token::Semicolon);
                    items.push(Item::EffectDef(EffectDef {
                        name,
                        params: vec![],
                        constraint: None,
                        includes: alias_names,
                        refinement: None,
                        parent: vec![],
                        span: span_from_token(&start_tok),
                        states: vec![],
                        transitions: vec![],
                        initial_state: None,
                    }));
                    continue;
                }

                let params: Vec<EffectDefParam> = if ctx.peek() == &Token::LParen {
                    ctx.advance();
                    let mut ps = Vec::new();
                    while ctx.peek() != &Token::RParen && ctx.peek() != &Token::Eof {
                        let pname = ctx.expect_ident();
                        let ptype = if ctx.peek() == &Token::Colon {
                            ctx.advance();
                            ctx.expect_ident()
                        } else {
                            "Str".to_string()
                        };
                        ps.push(EffectDefParam {
                            name: pname,
                            type_name: ptype,
                        });
                        if ctx.peek() == &Token::Comma {
                            ctx.advance();
                        }
                    }
                    ctx.expect(Token::RParen);
                    ps
                } else {
                    vec![]
                };
                // Plan 6: Multi-parent support — `parent: Name` or `parent: [A, B]`
                let parent: Vec<String> = if ctx.peek() == &Token::Parent {
                    ctx.advance();
                    ctx.expect(Token::Colon);
                    if ctx.peek() == &Token::LBracket {
                        // Multi-parent: parent: [Network, Encrypted]
                        parse_bracket_list(ctx)
                    } else {
                        // Single parent: parent: Network
                        vec![ctx.expect_ident()]
                    }
                } else {
                    vec![]
                };
                let includes: Vec<String> = if ctx.peek() == &Token::Includes {
                    ctx.advance();
                    ctx.expect(Token::Colon);
                    parse_bracket_list(ctx)
                } else {
                    vec![]
                };
                let refinement = if ctx.peek() == &Token::Where {
                    ctx.advance();
                    let text = collect_until_semicolon(ctx);
                    Some(text)
                } else {
                    None
                };

                // Parse stateful effect fields (states, initial, transition)
                // NOTE: Combining a `where` clause with stateful fields is not currently
                // supported. The `where` parser (collect_until_semicolon) leaves the
                // semicolon unconsumed, which causes the stateful-fields loop below to
                // break immediately. If this combination is needed in the future, the
                // parser must consume the `where` semicolon before entering this loop.
                let mut states: Vec<String> = vec![];
                let mut transitions: Vec<crate::parser::EffectTransition> = vec![];
                let mut initial_state: Option<String> = None;
                let mut is_stateful = false;

                loop {
                    match ctx.peek().clone() {
                        Token::Ident(ref s) if s == "states" => {
                            is_stateful = true;
                            ctx.advance();
                            ctx.expect(Token::Colon);
                            states = parse_bracket_list(ctx);
                            ctx.expect(Token::Semicolon);
                        }
                        Token::Ident(ref s) if s == "initial" => {
                            is_stateful = true;
                            ctx.advance();
                            ctx.expect(Token::Colon);
                            initial_state = Some(ctx.expect_ident());
                            ctx.expect(Token::Semicolon);
                        }
                        Token::Ident(ref s) if s == "transition" => {
                            is_stateful = true;
                            ctx.advance();
                            let operation = ctx.expect_ident();
                            ctx.expect(Token::Colon);
                            let from_state = ctx.expect_ident();
                            ctx.expect(Token::Arrow);
                            let to_state = ctx.expect_ident();
                            ctx.expect(Token::Semicolon);
                            transitions.push(crate::parser::EffectTransition {
                                operation,
                                from_state,
                                to_state,
                            });
                        }
                        _ => break,
                    }
                }

                if !is_stateful {
                    ctx.expect(Token::Semicolon);
                }

                let constraint = refinement.clone();
                items.push(Item::EffectDef(EffectDef {
                    name,
                    params,
                    constraint,
                    includes,
                    refinement,
                    parent,
                    span: span_from_token(&start_tok),
                    states,
                    transitions,
                    initial_state,
                }));
            }

            Token::Extern => {
                let start_tok = ctx.advance().clone();
                let language = if let Token::StringLit(s) = ctx.peek().clone() {
                    ctx.advance();
                    s
                } else {
                    ctx.expect_ident()
                };
                if ctx.peek() == &Token::LBrace {
                    ctx.advance();
                }
                let mut functions = Vec::new();
                while ctx.peek() != &Token::RBrace && ctx.peek() != &Token::Eof {
                    if ctx.peek() == &Token::Fn {
                        let fn_tok = ctx.advance().clone();
                        let fn_name = ctx.expect_ident();
                        ctx.expect(Token::LParen);
                        let mut param_names = Vec::new();
                        let mut param_types = Vec::new();
                        while ctx.peek() != &Token::RParen && ctx.peek() != &Token::Eof {
                            let first = ctx.expect_ident();
                            if ctx.peek() == &Token::Colon {
                                ctx.advance();
                                let type_name = ctx.expect_ident();
                                param_names.push(first);
                                param_types.push(type_name);
                            } else {
                                // No colon: treat as type-only (no name)
                                param_names.push(format!("arg{}", param_types.len()));
                                param_types.push(first);
                            }
                            if ctx.peek() == &Token::Comma {
                                ctx.advance();
                            }
                        }
                        ctx.expect(Token::RParen);
                        let return_type = if ctx.peek() == &Token::Arrow {
                            ctx.advance();
                            ctx.expect_ident()
                        } else {
                            "i64".to_string()
                        };
                        // Parse optional requires/ensures contracts for verified FFI
                        let mut ext_requires: Option<String> = None;
                        let mut ext_ensures: Option<String> = None;
                        loop {
                            match ctx.peek() {
                                Token::Requires => {
                                    ctx.advance();
                                    ctx.expect(Token::Colon);
                                    ext_requires = Some(collect_until_semicolon(ctx));
                                    ctx.expect(Token::Semicolon);
                                }
                                Token::Ensures => {
                                    ctx.advance();
                                    ctx.expect(Token::Colon);
                                    ext_ensures = Some(collect_until_semicolon(ctx));
                                    ctx.expect(Token::Semicolon);
                                }
                                _ => break,
                            }
                        }
                        // If no contracts were parsed, consume the trailing semicolon
                        if ext_requires.is_none() && ext_ensures.is_none() {
                            ctx.expect(Token::Semicolon);
                        }
                        functions.push(ExternFn {
                            name: fn_name,
                            param_names,
                            param_types,
                            return_type,
                            requires: ext_requires,
                            ensures: ext_ensures,
                            span: span_from_token(&fn_tok),
                        });
                    } else {
                        ctx.advance();
                    }
                }
                if ctx.peek() == &Token::RBrace {
                    ctx.advance();
                }
                items.push(Item::ExternBlock(ExternBlock {
                    language,
                    functions,
                    span: span_from_token(&start_tok),
                }));
            }

            Token::Atom | Token::Async | Token::Trusted | Token::Unverified => {
                let start_tok = ctx.tokens_ref()[ctx.pos()].clone();
                let mut is_async = false;
                let mut trust_level = TrustLevel::Verified;
                loop {
                    match ctx.peek() {
                        Token::Async => {
                            is_async = true;
                            ctx.advance();
                        }
                        Token::Trusted => {
                            trust_level = TrustLevel::Trusted;
                            ctx.advance();
                        }
                        Token::Unverified => {
                            trust_level = TrustLevel::Unverified;
                            ctx.advance();
                        }
                        _ => break,
                    }
                }
                if ctx.peek() != &Token::Atom {
                    ctx.advance();
                    continue;
                }
                ctx.advance();
                let mut atom = parse_atom_body(ctx, &start_tok);
                atom.is_async = is_async;
                atom.trust_level = trust_level;
                items.push(Item::Atom(atom));
            }

            _ => {
                ctx.advance();
            }
        }
    }

    items
}

// =============================================================================
// Atom body parsing
// =============================================================================

fn parse_atom_body(ctx: &mut ParseContext, start_tok: &SpannedToken) -> Atom {
    let name = ctx.expect_ident();

    let (type_params, where_bounds) = if ctx.peek() == &Token::Lt {
        let type_params_str = collect_angle_brackets(ctx);
        parse_type_params_with_bounds(&type_params_str)
    } else {
        (vec![], vec![])
    };

    let params_str = if ctx.peek() == &Token::LParen {
        collect_balanced_parens(ctx)
    } else {
        String::new()
    };
    let params: Vec<Param> = split_params(&params_str)
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| {
            let (is_ref, is_ref_mut, s_stripped) = if let Some(rest) = s.strip_prefix("ref mut ") {
                (false, true, rest.trim())
            } else if let Some(rest) = s.strip_prefix("ref ") {
                (true, false, rest.trim())
            } else {
                (false, false, s)
            };
            if let Some((param_name, type_name)) = s_stripped.split_once(':') {
                let type_name_str = type_name.trim().to_string();
                let type_ref = parse_type_ref(&type_name_str);
                Param {
                    name: param_name.trim().to_string(),
                    type_name: Some(type_name_str),
                    type_ref: Some(type_ref),
                    is_ref,
                    is_ref_mut,
                    fn_contract_requires: None,
                    fn_contract_ensures: None,
                }
            } else {
                Param {
                    name: s_stripped.to_string(),
                    type_name: None,
                    type_ref: None,
                    is_ref,
                    is_ref_mut,
                    fn_contract_requires: None,
                    fn_contract_ensures: None,
                }
            }
        })
        .collect();

    // Plan 18: Parse optional return type annotation (e.g., `-> Str`)
    let return_type = if ctx.peek() == &Token::Arrow {
        ctx.advance();
        Some(ctx.expect_ident())
    } else {
        None
    };

    let mut requires_raw = "true".to_string();
    let mut ensures = "true".to_string();
    let mut body_raw = String::new();
    let mut consumed_params: Vec<String> = Vec::new();
    let mut resources: Vec<String> = Vec::new();
    let mut max_unroll: Option<usize> = None;
    let mut invariant: Option<String> = None;
    let mut effects: Vec<Effect> = Vec::new();
    let mut contracts: Vec<(String, Option<String>, Option<String>)> = Vec::new();
    let mut effect_pre: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut effect_post: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    loop {
        match ctx.peek().clone() {
            Token::Requires => {
                ctx.advance();
                ctx.expect(Token::Colon);
                requires_raw = collect_until_semicolon(ctx);
                ctx.expect(Token::Semicolon);
            }
            Token::Ensures => {
                ctx.advance();
                ctx.expect(Token::Colon);
                ensures = collect_until_semicolon(ctx);
                ctx.expect(Token::Semicolon);
            }
            Token::Body => {
                ctx.advance();
                ctx.expect(Token::Colon);
                body_raw = collect_body(ctx);
                break;
            }
            Token::Consume => {
                ctx.advance();
                let text = collect_until_semicolon(ctx);
                for p in text.split(',') {
                    let p = p.trim();
                    if !p.is_empty() {
                        consumed_params.push(p.to_string());
                    }
                }
                ctx.expect(Token::Semicolon);
            }
            Token::Resources => {
                ctx.advance();
                ctx.expect(Token::Colon);
                if ctx.peek() == &Token::LBracket {
                    resources = parse_bracket_list(ctx);
                } else {
                    let text = collect_until_semicolon(ctx);
                    for r in text.split(',') {
                        let r = r.trim();
                        if !r.is_empty() {
                            resources.push(r.to_string());
                        }
                    }
                }
                ctx.expect(Token::Semicolon);
            }
            Token::MaxUnroll => {
                ctx.advance();
                ctx.expect(Token::Colon);
                if let Token::IntLit(n) = ctx.peek().clone() {
                    ctx.advance();
                    max_unroll = Some(n as usize);
                }
                ctx.expect(Token::Semicolon);
            }
            Token::Invariant => {
                ctx.advance();
                ctx.expect(Token::Colon);
                invariant = Some(collect_until_semicolon(ctx));
                ctx.expect(Token::Semicolon);
            }
            Token::Effects => {
                ctx.advance();
                ctx.expect(Token::Colon);
                if ctx.peek() == &Token::LBracket {
                    ctx.advance();
                    let mut effect_text = String::new();
                    while ctx.peek() != &Token::RBracket && ctx.peek() != &Token::Eof {
                        if !effect_text.is_empty() {
                            effect_text.push(' ');
                        }
                        let tok = ctx.advance();
                        match &tok.token {
                            Token::StringLit(s) => {
                                effect_text.push('"');
                                effect_text.push_str(s);
                                effect_text.push('"');
                            }
                            other => effect_text.push_str(&format!("{}", other)),
                        }
                    }
                    ctx.expect(Token::RBracket);
                    effects = parse_effect_list(&effect_text);
                } else {
                    let text = collect_until_semicolon(ctx);
                    effects = parse_effect_list(&text);
                }
                ctx.expect(Token::Semicolon);
            }
            Token::Contract => {
                ctx.advance();
                ctx.expect(Token::LParen);
                let param_name = ctx.expect_ident();
                ctx.expect(Token::RParen);
                ctx.expect(Token::Colon);
                let clauses = collect_until_semicolon(ctx);
                ctx.expect(Token::Semicolon);
                let (fn_req, fn_ens) = parse_contract_clauses(&clauses);
                contracts.push((param_name, fn_req, fn_ens));
            }
            Token::Ident(ref s) if s == "effect_pre" || s == "effect_post" => {
                let is_pre = s == "effect_pre";
                ctx.advance();
                ctx.expect(Token::Colon);
                // Parse { EffectName: StateName, EffectName2: StateName2 }
                let map = parse_effect_state_map(ctx);
                ctx.expect(Token::Semicolon);
                if is_pre {
                    effect_pre = map;
                } else {
                    effect_post = map;
                }
            }
            Token::Atom
            | Token::Async
            | Token::Trusted
            | Token::Unverified
            | Token::Struct
            | Token::Enum
            | Token::Trait
            | Token::Impl
            | Token::Import
            | Token::Extern
            | Token::Resource
            | Token::Effect
            | Token::Eof => break,
            _ => {
                ctx.advance();
            }
        }
    }

    let mut params = params;
    for (param_name, fn_req, fn_ens) in &contracts {
        for p in params.iter_mut() {
            if p.name == *param_name {
                p.fn_contract_requires = fn_req.clone();
                p.fn_contract_ensures = fn_ens.clone();
            }
        }
    }

    let forall_constraints = extract_quantifiers(&requires_raw);
    let requires_cleaned = strip_quantifiers(&requires_raw);

    Atom {
        name,
        type_params,
        where_bounds,
        params,
        requires: requires_cleaned,
        forall_constraints,
        ensures,
        body_expr: body_raw,
        consumed_params,
        resources,
        is_async: false,
        trust_level: TrustLevel::Verified,
        max_unroll,
        invariant,
        effects,
        return_type,
        span: span_from_token(start_tok),
        effect_pre,
        effect_post,
    }
}

/// Parse a { Key: Value, Key2: Value2 } map for effect_pre/effect_post.
fn parse_effect_state_map(ctx: &mut ParseContext) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    if ctx.peek() != &Token::LBrace {
        return map;
    }
    ctx.advance(); // consume '{'
    loop {
        if ctx.peek() == &Token::RBrace || ctx.peek() == &Token::Eof {
            break;
        }
        let key = ctx.expect_ident();
        ctx.expect(Token::Colon);
        let value = ctx.expect_ident();
        map.insert(key, value);
        if ctx.peek() == &Token::Comma {
            ctx.advance();
        } else {
            break;
        }
    }
    ctx.expect(Token::RBrace);
    map
}

// =============================================================================
// String-based helpers for trait/law/contract parsing
// =============================================================================

fn parse_contract_clauses(clauses: &str) -> (Option<String>, Option<String>) {
    let mut fn_req: Option<String> = None;
    let mut fn_ens: Option<String> = None;
    let req_markers = ["requires :", "requires:"];
    let ens_markers = ["ensures :", "ensures:"];
    for req_marker in &req_markers {
        if let Some(req_idx) = clauses.find(req_marker) {
            let after_req = &clauses[req_idx + req_marker.len()..];
            for ens_marker in &ens_markers {
                if let Some(ens_pos) = after_req.find(ens_marker) {
                    let req_val = after_req[..ens_pos].trim().trim_end_matches(',').trim();
                    fn_req = Some(req_val.to_string());
                    let ens_val = after_req[ens_pos + ens_marker.len()..].trim();
                    fn_ens = Some(ens_val.to_string());
                    return (fn_req, fn_ens);
                }
            }
            fn_req = Some(after_req.trim().to_string());
            return (fn_req, fn_ens);
        }
    }
    for ens_marker in &ens_markers {
        if let Some(ens_idx) = clauses.find(ens_marker) {
            let ens_val = clauses[ens_idx + ens_marker.len()..].trim();
            fn_ens = Some(ens_val.to_string());
            return (fn_req, fn_ens);
        }
    }
    (fn_req, fn_ens)
}

fn parse_trait_method_from_str(line: &str) -> Option<TraitMethod> {
    let line = line.trim();
    if !line.starts_with("fn ") {
        return None;
    }
    let rest = &line[3..];
    let paren_start = rest.find('(')?;
    let method_name = rest[..paren_start].trim().to_string();
    let paren_end = rest.rfind(')')?;
    let params_str = &rest[paren_start + 1..paren_end];
    let arrow_pos = rest[paren_end..].find("->");
    let return_type = if let Some(ap) = arrow_pos {
        rest[paren_end + ap + 2..]
            .trim()
            .trim_end_matches(';')
            .trim()
            .to_string()
    } else {
        "i64".to_string()
    };
    let mut param_types = Vec::new();
    let mut param_constraints = Vec::new();
    for p in params_str.split(',') {
        let p = p.trim();
        if p.is_empty() {
            continue;
        }
        if let Some((before_where, constraint)) = p.split_once("where") {
            let type_str = if let Some((_, t)) = before_where.split_once(':') {
                t.trim().to_string()
            } else {
                before_where.trim().to_string()
            };
            param_types.push(type_str);
            param_constraints.push(Some(constraint.trim().to_string()));
        } else if let Some((_, t)) = p.split_once(':') {
            param_types.push(t.trim().to_string());
            param_constraints.push(None);
        } else {
            param_types.push(p.to_string());
            param_constraints.push(None);
        }
    }
    Some(TraitMethod {
        name: method_name,
        param_types,
        return_type,
        param_constraints,
    })
}

fn parse_law_from_str(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if !line.starts_with("law ") {
        return None;
    }
    let rest = &line[4..];
    let colon_pos = rest.find(':')?;
    let law_name = rest[..colon_pos].trim().to_string();
    let law_expr = rest[colon_pos + 1..].trim().to_string();
    Some((law_name, law_expr))
}

fn parse_trait_body_flat(
    body: &str,
    methods: &mut Vec<TraitMethod>,
    laws: &mut Vec<(String, String)>,
) {
    let body = body.trim();
    if body.is_empty() {
        return;
    }
    let mut i = 0;
    let len = body.len();
    while i < len {
        while i < len
            && body
                .as_bytes()
                .get(i)
                .is_some_and(|b| b.is_ascii_whitespace())
        {
            i += 1;
        }
        if i >= len {
            break;
        }
        if i + 3 <= len && &body[i..i + 3] == "fn " {
            let start = i;
            let end = find_next_trait_item(body, i + 3);
            if let Some(method) = parse_trait_method_from_str(body[start..end].trim()) {
                methods.push(method);
            }
            i = end;
        } else if i + 4 <= len && &body[i..i + 4] == "law " {
            let start = i;
            let end = find_next_trait_item(body, i + 4);
            if let Some(law) = parse_law_from_str(body[start..end].trim()) {
                laws.push(law);
            }
            i = end;
        } else {
            i += 1;
        }
    }
}

fn find_next_trait_item(body: &str, start: usize) -> usize {
    let mut i = start;
    while i < body.len() {
        if i + 3 <= body.len() && &body[i..i + 3] == "fn " {
            return i;
        }
        if i + 4 <= body.len() && &body[i..i + 4] == "law " {
            return i;
        }
        i += 1;
    }
    body.len()
}

fn extract_quantifiers(requires: &str) -> Vec<Quantifier> {
    let mut quantifiers = Vec::new();
    for (prefix, q_type) in [
        ("forall(", QuantifierType::ForAll),
        ("exists(", QuantifierType::Exists),
    ] {
        let mut search_from = 0;
        while let Some(pos) = requires[search_from..].find(prefix) {
            let abs_pos = search_from + pos + prefix.len();
            let mut depth = 1;
            let mut end_pos = abs_pos;
            for (i, c) in requires[abs_pos..].char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end_pos = abs_pos + i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let inner = &requires[abs_pos..end_pos];
            let parts: Vec<&str> = inner.splitn(4, ',').collect();
            if parts.len() >= 4 {
                quantifiers.push(Quantifier {
                    q_type: q_type.clone(),
                    var: parts[0].trim().to_string(),
                    start: parts[1].trim().to_string(),
                    end: parts[2].trim().to_string(),
                    condition: parts[3].trim().to_string(),
                });
            }
            search_from = end_pos + 1;
        }
    }
    quantifiers
}

fn strip_quantifiers(requires: &str) -> String {
    let mut result = requires.to_string();
    for prefix in ["forall(", "exists("] {
        while let Some(pos) = result.find(prefix) {
            let after = pos + prefix.len();
            let mut depth = 1;
            let mut end = after;
            for (i, c) in result[after..].char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = after + i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            result = format!("{}true{}", &result[..pos], &result[end..]);
        }
    }
    result
}

// =============================================================================
// Backward-compatible public API
// =============================================================================

pub fn parse_module_from_source(source: &str) -> Vec<Item> {
    let mut lexer = super::lexer::Lexer::new(source);
    let tokens = lexer.tokenize();
    let mut ctx = ParseContext::new(tokens);
    parse_module_from_tokens(&mut ctx)
}

pub fn parse_atom_from_source(source: &str) -> Atom {
    let mut lexer = super::lexer::Lexer::new(source);
    let tokens = lexer.tokenize();
    let mut ctx = ParseContext::new(tokens);
    let start_tok = ctx.tokens_ref()[0].clone();
    let mut is_async = false;
    let mut trust_level = TrustLevel::Verified;
    loop {
        match ctx.peek() {
            Token::Async => {
                is_async = true;
                ctx.advance();
            }
            Token::Trusted => {
                trust_level = TrustLevel::Trusted;
                ctx.advance();
            }
            Token::Unverified => {
                trust_level = TrustLevel::Unverified;
                ctx.advance();
            }
            _ => break,
        }
    }
    if ctx.peek() == &Token::Atom {
        ctx.advance();
    }
    let mut atom = parse_atom_body(&mut ctx, &start_tok);
    atom.is_async = is_async;
    atom.trust_level = trust_level;
    atom
}

#[allow(dead_code)]
fn offset_to_line_col(source: &str, offset: usize) -> (usize, usize) {
    let mut line = 1;
    let mut col = 1;
    for (i, c) in source.char_indices() {
        if i >= offset {
            break;
        }
        if c == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

#[allow(dead_code)]
fn span_from_offset(source: &str, offset: usize, len: usize) -> Span {
    let (line, col) = offset_to_line_col(source, offset);
    Span::new("", line, col, len)
}
