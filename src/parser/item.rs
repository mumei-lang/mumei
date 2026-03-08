// =============================================================================
// Top-level item parsing (replaces all regex in parse_module / parse_atom)
// =============================================================================

use regex::Regex;

use crate::ast::TypeRef;
use crate::parser::{
    Atom, Effect, EffectDef, EffectDefParam, EffectParam, EnumDef, EnumVariant, ExternBlock,
    ExternFn, ImplDef, ImportDecl, Item, Param, Quantifier, QuantifierType, RefinedType,
    ResourceDef, ResourceMode, Span, StructDef, StructField, TraitDef, TraitMethod, TrustLevel,
    TypeParamBound,
};

// ---- Helper functions (moved from old parser.rs) ----

/// Parse type params from a string like "<T, U>"
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

/// Parse type reference string to TypeRef
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
        let return_type = if let Some(arrow_rest) = rest.strip_prefix("->") {
            parse_type_ref(arrow_rest.trim())
        } else {
            TypeRef::simple("i64")
        };

        return TypeRef::fn_type(param_types, return_type);
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

/// Split type args considering nested angle brackets
fn split_type_args(input: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut depth = 0;
    let mut current = String::new();
    for c in input.chars() {
        match c {
            '<' => {
                depth += 1;
                current.push(c);
            }
            '>' => {
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

/// Parse type params with trait bounds
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

/// Split params considering nested parentheses
/// Split enum variants considering nested parentheses.
/// e.g. "Cons(i64, List), Nil" -> ["Cons(i64, List)", "Nil"]
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

// ---- Effect parsing helpers ----

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
    if let Some(paren_start) = input.find('(') {
        let name = input[..paren_start].trim().to_string();
        let paren_end = input.rfind(')').unwrap_or(input.len());
        let params_str = &input[paren_start + 1..paren_end];
        let params = parse_effect_params(params_str);
        Effect {
            name,
            params,
            span: Span::default(),
        }
    } else {
        Effect {
            name: input.to_string(),
            params: vec![],
            span: Span::default(),
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

// ---- Main parse_module: regex-based (preserved for backward compat) ----

/// Parse a full module source into a list of Items.
/// This is the main entry point, kept backward-compatible.
pub fn parse_module_from_source(source: &str) -> Vec<Item> {
    let mut items = Vec::new();

    // Strip comments
    let comment_re = Regex::new(r"//[^\n]*").unwrap();
    let source = comment_re.replace_all(source, "").to_string();
    let source = source.as_str();

    // import
    let import_re = Regex::new(r#"(?m)^import\s+"([^"]+)"(?:\s+as\s+(\w+))?\s*;"#).unwrap();
    for cap in import_re.captures_iter(source) {
        let path = cap[1].to_string();
        let alias = cap.get(2).map(|m| m.as_str().to_string());
        let m = cap.get(0).unwrap();
        items.push(Item::Import(ImportDecl {
            span: span_from_offset(source, m.start(), m.end() - m.start()),
            path,
            alias,
        }));
    }

    // type
    let type_re = Regex::new(r"(?m)^type\s+(\w+)\s*=\s*(\w+)\s+where\s+([^;]+);").unwrap();
    for cap in type_re.captures_iter(source) {
        let full_predicate = cap[3].trim().to_string();
        let tokens = super::lexer::legacy_tokenize(&full_predicate);
        let operand = tokens.first().cloned().unwrap_or_else(|| "v".to_string());
        let m = cap.get(0).unwrap();
        items.push(Item::TypeDef(RefinedType {
            name: cap[1].to_string(),
            _base_type: cap[2].to_string(),
            operand,
            predicate_raw: full_predicate,
            span: span_from_offset(source, m.start(), m.end() - m.start()),
        }));
    }

    // struct
    let struct_re = Regex::new(r"(?m)^struct\s+(\w+)\s*(<[^>]*>)?\s*\{([^}]*)\}").unwrap();
    for cap in struct_re.captures_iter(source) {
        let name = cap[1].to_string();
        let type_params = cap
            .get(2)
            .map(|m| {
                let (params, _) = parse_type_params_from_str(m.as_str());
                params
            })
            .unwrap_or_default();
        let fields_raw = &cap[3];
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
        let m = cap.get(0).unwrap();
        items.push(Item::StructDef(StructDef {
            name,
            type_params,
            fields,
            method_names: vec![],
            span: span_from_offset(source, m.start(), m.end() - m.start()),
        }));
    }

    // enum
    let enum_re = Regex::new(r"(?m)^enum\s+(\w+)\s*(<[^>]*>)?\s*\{([^}]*)\}").unwrap();
    for cap in enum_re.captures_iter(source) {
        let name = cap[1].to_string();
        let type_params = cap
            .get(2)
            .map(|m| {
                let (params, _) = parse_type_params_from_str(m.as_str());
                params
            })
            .unwrap_or_default();
        let variants_raw = &cap[3];
        let mut any_recursive = false;
        // Use depth-aware split to handle commas inside parentheses
        // e.g. "Cons(i64, List), Nil" should split into ["Cons(i64, List)", "Nil"]
        let variant_strs = split_variants(variants_raw);
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
        let m = cap.get(0).unwrap();
        items.push(Item::EnumDef(EnumDef {
            name,
            type_params,
            variants,
            is_recursive: any_recursive,
            span: span_from_offset(source, m.start(), m.end() - m.start()),
        }));
    }

    // trait
    let trait_re = Regex::new(r"(?m)^trait\s+(\w+)\s*\{([^}]*)\}").unwrap();
    for cap in trait_re.captures_iter(source) {
        let name = cap[1].to_string();
        let body = &cap[2];
        let mut methods = Vec::new();
        let mut laws = Vec::new();

        for line in body.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            if line.starts_with("fn ") {
                let fn_re = Regex::new(r"fn\s+(\w+)\s*\(([^)]*)\)\s*->\s*(\w+)").unwrap();
                if let Some(fcap) = fn_re.captures(line) {
                    let method_name = fcap[1].to_string();
                    let params_str = &fcap[2];
                    let return_type = fcap[3].to_string();
                    let mut param_types: Vec<String> = Vec::new();
                    let mut param_constraints: Vec<Option<String>> = Vec::new();
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
                    methods.push(TraitMethod {
                        name: method_name,
                        param_types,
                        return_type,
                        param_constraints,
                    });
                }
            } else if line.starts_with("law ") {
                let law_re = Regex::new(r"law\s+(\w+)\s*:\s*([^;]+)").unwrap();
                if let Some(lcap) = law_re.captures(line) {
                    let law_name = lcap[1].to_string();
                    let law_expr = lcap[2].trim().to_string();
                    laws.push((law_name, law_expr));
                }
            }
        }
        let m = cap.get(0).unwrap();
        items.push(Item::TraitDef(TraitDef {
            name,
            methods,
            laws,
            span: span_from_offset(source, m.start(), m.end() - m.start()),
        }));
    }

    // impl
    let impl_header_re = Regex::new(r"(?m)^impl\s+(\w+)\s+for\s+(\w+)\s*\{").unwrap();
    for cap in impl_header_re.captures_iter(source) {
        let trait_name = cap[1].to_string();
        let target_type = cap[2].to_string();
        let block_start = cap.get(0).unwrap().end();
        let mut depth = 1;
        let mut block_end = block_start;
        for (i, c) in source[block_start..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        block_end = block_start + i;
                        break;
                    }
                }
                _ => {}
            }
        }
        let body = &source[block_start..block_end];
        let mut method_bodies = Vec::new();

        let fn_header_re = Regex::new(r"fn\s+(\w+)\s*\([^)]*\)\s*->\s*\w+\s*\{").unwrap();
        for fcap in fn_header_re.captures_iter(body) {
            let method_name = fcap[1].to_string();
            let fn_body_start = fcap.get(0).unwrap().end();
            let mut fn_depth = 1;
            let mut fn_body_end = fn_body_start;
            for (i, c) in body[fn_body_start..].char_indices() {
                match c {
                    '{' => fn_depth += 1,
                    '}' => {
                        fn_depth -= 1;
                        if fn_depth == 0 {
                            fn_body_end = fn_body_start + i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let method_body = body[fn_body_start..fn_body_end].trim().to_string();
            method_bodies.push((method_name, method_body));
        }
        let m = cap.get(0).unwrap();
        items.push(Item::ImplDef(ImplDef {
            trait_name,
            target_type,
            method_bodies,
            span: span_from_offset(source, m.start(), block_end + 1 - m.start()),
        }));
    }

    // resource
    let resource_re =
        Regex::new(r"(?m)^resource\s+(\w+)\s+priority:\s*(-?\d+)\s+mode:\s*(exclusive|shared)\s*;")
            .unwrap();
    for cap in resource_re.captures_iter(source) {
        let name = cap[1].to_string();
        let priority = cap[2].parse::<i64>().unwrap_or(0);
        let mode = match &cap[3] {
            "exclusive" => ResourceMode::Exclusive,
            _ => ResourceMode::Shared,
        };
        let m = cap.get(0).unwrap();
        items.push(Item::ResourceDef(ResourceDef {
            name,
            priority,
            mode,
            span: span_from_offset(source, m.start(), m.end() - m.start()),
        }));
    }

    // effect
    let effect_re = Regex::new(
        r"(?m)^effect\s+(\w+)(?:\(([^)]*)\))?(?:\s+parent:\s*(\w+))?(?:\s+includes:\s*\[([^\]]*)\])?(?:\s+where\s+([^;]+))?\s*;",
    )
    .unwrap();
    for cap in effect_re.captures_iter(source) {
        let name = cap[1].to_string();
        let params: Vec<EffectDefParam> = cap
            .get(2)
            .map(|m| {
                m.as_str()
                    .split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .map(|s| {
                        if let Some((pname, ptype)) = s.split_once(':') {
                            EffectDefParam {
                                name: pname.trim().to_string(),
                                type_name: ptype.trim().to_string(),
                            }
                        } else {
                            EffectDefParam {
                                name: s.to_string(),
                                type_name: "Str".to_string(),
                            }
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let parent = cap.get(3).map(|m| m.as_str().trim().to_string());
        let includes: Vec<String> = cap
            .get(4)
            .map(|m| {
                m.as_str()
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let refinement = cap.get(5).map(|m| m.as_str().trim().to_string());
        let constraint = refinement.clone();
        let m = cap.get(0).unwrap();
        items.push(Item::EffectDef(EffectDef {
            name,
            params,
            constraint,
            includes,
            refinement,
            parent,
            span: span_from_offset(source, m.start(), m.end() - m.start()),
        }));
    }

    // extern
    let extern_re = Regex::new(r#"(?m)^extern\s+"(\w+)"\s*\{([^}]*)\}"#).unwrap();
    for cap in extern_re.captures_iter(source) {
        let language = cap[1].to_string();
        let body = &cap[2];
        let body_offset = cap.get(2).unwrap().start();
        let mut functions = Vec::new();
        let fn_re = Regex::new(r"fn\s+(\w+)\s*\(([^)]*)\)\s*->\s*(\w+)").unwrap();
        for fcap in fn_re.captures_iter(body) {
            let name = fcap[1].to_string();
            let params_str = &fcap[2];
            let param_types: Vec<String> = params_str
                .split(',')
                .map(|p| p.trim())
                .filter(|p| !p.is_empty())
                .map(|p| {
                    if let Some((_, t)) = p.split_once(':') {
                        t.trim().to_string()
                    } else {
                        p.to_string()
                    }
                })
                .collect();
            let return_type = fcap[3].to_string();
            let fm = fcap.get(0).unwrap();
            functions.push(ExternFn {
                name,
                param_types,
                return_type,
                span: span_from_offset(source, body_offset + fm.start(), fm.end() - fm.start()),
            });
        }
        let m = cap.get(0).unwrap();
        items.push(Item::ExternBlock(ExternBlock {
            language,
            functions,
            span: span_from_offset(source, m.start(), m.end() - m.start()),
        }));
    }

    // atoms (modified and plain)
    let atom_re = Regex::new(r"atom\s+\w+").unwrap();
    let modified_atom_re = Regex::new(r"(?:(?:async|trusted|unverified)\s+)+atom\s+\w+").unwrap();
    let modified_atom_indices: Vec<_> = modified_atom_re.find_iter(source).collect();
    let mut modified_atom_starts: std::collections::HashSet<usize> =
        std::collections::HashSet::new();
    for mat in &modified_atom_indices {
        let start = mat.start();
        modified_atom_starts.insert(start);
        let atom_source = &source[start..];
        let mut is_async = false;
        let mut trust_level = TrustLevel::Verified;
        let mut remaining = atom_source;
        loop {
            remaining = remaining.trim_start();
            if remaining.starts_with("async")
                && remaining[5..].starts_with(|c: char| c.is_whitespace())
            {
                is_async = true;
                remaining = &remaining[5..];
            } else if remaining.starts_with("trusted")
                && remaining[7..].starts_with(|c: char| c.is_whitespace())
            {
                trust_level = TrustLevel::Trusted;
                remaining = &remaining[7..];
            } else if remaining.starts_with("unverified")
                && remaining[10..].starts_with(|c: char| c.is_whitespace())
            {
                trust_level = TrustLevel::Unverified;
                remaining = &remaining[10..];
            } else {
                break;
            }
        }
        let atom_start_in_remaining = remaining.find("atom").unwrap_or(0);
        let atom_text = &remaining[atom_start_in_remaining..];
        let next_atom_pos = atom_re
            .find(atom_text.get(5..).unwrap_or(""))
            .map(|m| m.start() + 5)
            .unwrap_or(atom_text.len());
        let atom_slice = &atom_text[..next_atom_pos];
        let mut atom = parse_atom_from_source(atom_slice);
        atom.is_async = is_async;
        atom.trust_level = trust_level;
        items.push(Item::Atom(atom));
    }

    let atom_indices: Vec<_> = atom_re.find_iter(source).map(|m| m.start()).collect();
    for i in 0..atom_indices.len() {
        let start = atom_indices[i];
        let skip = modified_atom_starts
            .iter()
            .any(|&ms| start > ms && start < ms + 30);
        if skip {
            continue;
        }
        let prefix = &source[start.saturating_sub(12)..start];
        if prefix.contains("async") || prefix.contains("trusted") || prefix.contains("unverified") {
            continue;
        }
        let end = if i + 1 < atom_indices.len() {
            atom_indices[i + 1]
        } else {
            source.len()
        };
        let atom_source = &source[start..end];
        items.push(Item::Atom(parse_atom_from_source(atom_source)));
    }

    items
}

/// Parse a single atom definition from source text.
pub fn parse_atom_from_source(source: &str) -> Atom {
    let name_re = Regex::new(r"atom\s+(\w+)\s*(<[^>]*>)?\s*\(").unwrap();
    let req_re = Regex::new(r"requires:\s*([^;]+);").unwrap();
    let ens_re = Regex::new(r"ensures:\s*([^;]+);").unwrap();

    let forall_re =
        Regex::new(r"forall\(\s*(\w+)\s*,\s*([^,]+)\s*,\s*([^,]+)\s*,\s*([^)]+)\)").unwrap();
    let exists_re =
        Regex::new(r"exists\(\s*(\w+)\s*,\s*([^,]+)\s*,\s*([^,]+)\s*,\s*([^)]+)\)").unwrap();

    let name_caps = name_re.captures(source).expect("Failed to parse atom name");
    let name = name_caps[1].to_string();
    let (type_params, where_bounds) = name_caps
        .get(2)
        .map(|m| parse_type_params_with_bounds(m.as_str()))
        .unwrap_or_default();

    let params_start = name_caps.get(0).unwrap().end();
    let after_open = &source[params_start..];
    let mut depth = 1;
    let mut params_end = 0;
    for (i, c) in after_open.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    params_end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    let params_str = &after_open[..params_end];

    let params: Vec<Param> = split_params(params_str)
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
                }
            } else {
                Param {
                    name: s_stripped.to_string(),
                    type_name: None,
                    type_ref: None,
                    is_ref,
                    is_ref_mut,
                }
            }
        })
        .collect();

    let requires_raw = req_re
        .captures(source)
        .map_or("true".to_string(), |c| c[1].trim().to_string());
    let ensures = ens_re
        .captures(source)
        .map_or("true".to_string(), |c| c[1].trim().to_string());

    let body_marker = "body:";
    let body_start_pos =
        source.find(body_marker).expect("Failed to find body:") + body_marker.len();
    let body_snippet = source[body_start_pos..].trim();

    let mut body_raw = String::new();
    if body_snippet.starts_with('{') {
        let mut brace_count = 0;
        for c in body_snippet.chars() {
            body_raw.push(c);
            if c == '{' {
                brace_count += 1;
            } else if c == '}' {
                brace_count -= 1;
                if brace_count == 0 {
                    break;
                }
            }
        }
    } else {
        body_raw = body_snippet.split(';').next().unwrap_or("").to_string();
    }

    let mut forall_constraints = Vec::new();
    for cap in forall_re.captures_iter(&requires_raw) {
        forall_constraints.push(Quantifier {
            q_type: QuantifierType::ForAll,
            var: cap[1].to_string(),
            start: cap[2].trim().to_string(),
            end: cap[3].trim().to_string(),
            condition: cap[4].trim().to_string(),
        });
    }
    for cap in exists_re.captures_iter(&requires_raw) {
        forall_constraints.push(Quantifier {
            q_type: QuantifierType::Exists,
            var: cap[1].to_string(),
            start: cap[2].trim().to_string(),
            end: cap[3].trim().to_string(),
            condition: cap[4].trim().to_string(),
        });
    }

    let consume_re = Regex::new(r"consume\s+([^;]+);").unwrap();
    let consumed_params: Vec<String> = consume_re
        .captures_iter(source)
        .flat_map(|cap| {
            cap[1]
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        })
        .collect();

    let resources_re = Regex::new(r"resources:\s*\[?([^\];]+)\]?\s*;").unwrap();
    let resources: Vec<String> = resources_re
        .captures_iter(source)
        .flat_map(|cap| {
            cap[1]
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        })
        .collect();

    let max_unroll_re = Regex::new(r"max_unroll:\s*(\d+)\s*;").unwrap();
    let max_unroll = max_unroll_re
        .captures(source)
        .and_then(|cap| cap[1].parse::<usize>().ok());

    let invariant_re = Regex::new(r"(?m)^invariant:\s*([^;]+);").unwrap();
    let invariant = invariant_re
        .captures(source)
        .map(|cap| cap[1].trim().to_string());

    let effects_re = Regex::new(r"effects:\s*\[([^\]]*)]\s*;").unwrap();
    let effects: Vec<Effect> = effects_re
        .captures(source)
        .map(|cap| parse_effect_list(&cap[1]))
        .unwrap_or_default();

    let atom_match = name_caps.get(0).unwrap();
    Atom {
        name,
        type_params,
        where_bounds,
        params,
        requires: forall_re
            .replace_all(&exists_re.replace_all(&requires_raw, "true"), "true")
            .to_string(),
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
        span: span_from_offset(source, atom_match.start(), source.len()),
    }
}

// ---- Span helper ----

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

fn span_from_offset(source: &str, offset: usize, len: usize) -> Span {
    let (line, col) = offset_to_line_col(source, offset);
    Span::new("", line, col, len)
}
