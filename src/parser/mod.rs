// =============================================================================
// Parser module: recursive descent parser with proper lexer
// =============================================================================
//
// Module structure:
//   token.rs   — Token enum and SpannedToken
//   lexer.rs   — Lexer: source string → Vec<SpannedToken>
//   ast.rs     — All AST type definitions
//   expr.rs    — Expression and statement parsing (Pratt parser)
//   item.rs    — Top-level item parsing (replaces regex)
//   pattern.rs — Pattern parsing

pub mod ast;
pub mod expr;
pub mod item;
pub mod lexer;
pub mod pattern;
pub mod token;

// Re-export all AST types for backward compatibility
// (callers use `use crate::parser::*`)
pub use ast::*;

// Re-export item helpers that other modules need
pub use item::parse_type_ref;

use token::{SpannedToken, Token};

// =============================================================================
// ParseContext: shared parsing state for expression/statement/pattern parsing
// =============================================================================

/// Shared parsing context used by expr.rs, pattern.rs, etc.
pub struct ParseContext {
    tokens: Vec<SpannedToken>,
    pos: usize,
}

impl ParseContext {
    pub fn new(tokens: Vec<SpannedToken>) -> Self {
        ParseContext { tokens, pos: 0 }
    }

    pub fn peek(&self) -> &Token {
        self.tokens
            .get(self.pos)
            .map(|t| &t.token)
            .unwrap_or(&Token::Eof)
    }

    pub fn peek_at(&self, offset: usize) -> Option<&Token> {
        self.tokens.get(self.pos + offset).map(|t| &t.token)
    }

    pub fn advance(&mut self) -> &SpannedToken {
        let tok = &self.tokens[self.pos];
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        tok
    }

    pub fn expect(&mut self, expected: Token) {
        if self.peek() == &expected {
            self.advance();
        }
        // Silently skip if not matching (backward compat with old parser behavior)
    }

    pub fn expect_ident(&mut self) -> String {
        match self.peek().clone() {
            Token::Ident(s) => {
                self.advance();
                s
            }
            // Many keywords can appear as identifiers in various contexts
            ref tok => {
                let name = format!("{}", tok);
                if name.chars().next().map_or(false, |c| c.is_alphabetic()) {
                    self.advance();
                    name
                } else {
                    self.advance();
                    "unknown".to_string()
                }
            }
        }
    }
}

// =============================================================================
// Public API — same signatures as the old parser.rs
// =============================================================================

/// Parse a full module source into a list of Items.
pub fn parse_module(source: &str) -> Vec<Item> {
    item::parse_module_from_source(source)
}

/// Parse a pure expression (for requires/ensures/conditions).
pub fn parse_expression(input: &str) -> Expr {
    let mut lexer = lexer::Lexer::new(input);
    let tokens = lexer.tokenize();
    let mut ctx = ParseContext::new(tokens);
    expr::parse_expr(&mut ctx, 0)
}

/// Parse a body expression (blocks and statements).
pub fn parse_body_expr(input: &str) -> Stmt {
    let mut lexer = lexer::Lexer::new(input);
    let tokens = lexer.tokenize();
    let mut ctx = ParseContext::new(tokens);
    expr::parse_block_or_stmt(&mut ctx)
}

/// Parse a single atom definition from source text.
// NOTE: parse_atom is a public API preserved for backward compatibility and used in tests (e.g., test_parse_task_group)
#[allow(dead_code)]
pub fn parse_atom(source: &str) -> Atom {
    item::parse_atom_from_source(source)
}

/// Legacy tokenize function for backward compatibility.
// NOTE: tokenize is a legacy public API preserved for backward compatibility and used in tests (e.g., test_legacy_tokenize_compat)
#[allow(dead_code)]
pub fn tokenize(input: &str) -> Vec<String> {
    lexer::legacy_tokenize(input)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_type_ref_simple() {
        let tr = parse_type_ref("i64");
        assert_eq!(tr.name, "i64");
        assert!(tr.type_args.is_empty());
    }

    #[test]
    fn test_parse_type_ref_generic() {
        let tr = parse_type_ref("Stack<i64>");
        assert_eq!(tr.name, "Stack");
        assert_eq!(tr.type_args.len(), 1);
        assert_eq!(tr.type_args[0].name, "i64");
    }

    #[test]
    fn test_parse_type_ref_nested() {
        let tr = parse_type_ref("Map<String, List<i64>>");
        assert_eq!(tr.name, "Map");
        assert_eq!(tr.type_args.len(), 2);
        assert_eq!(tr.type_args[0].name, "String");
        assert_eq!(tr.type_args[1].name, "List");
        assert_eq!(tr.type_args[1].type_args[0].name, "i64");
    }

    #[test]
    fn test_parse_type_ref_display() {
        let tr = parse_type_ref("Stack<i64>");
        assert_eq!(tr.display_name(), "Stack<i64>");

        let tr2 = parse_type_ref("Map<String, List<i64>>");
        assert_eq!(tr2.display_name(), "Map<String, List<i64>>");
    }

    #[test]
    fn test_parse_type_ref_fn() {
        let tr = parse_type_ref("atom_ref(i64) -> i64");
        assert!(tr.is_fn_type());
        assert_eq!(tr.type_args.len(), 2);
    }

    #[test]
    fn test_parse_type_ref_fn_multi_param() {
        let tr = parse_type_ref("atom_ref(i64, i64) -> i64");
        assert!(tr.is_fn_type());
        assert_eq!(tr.type_args.len(), 3);
    }

    #[test]
    fn test_parse_struct() {
        let source = "struct Point { x: f64, y: f64 }";
        let items = parse_module(source);
        let structs: Vec<_> = items
            .iter()
            .filter_map(|i| {
                if let Item::StructDef(s) = i {
                    Some(s)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(structs.len(), 1);
        assert_eq!(structs[0].name, "Point");
        assert_eq!(structs[0].fields.len(), 2);
    }

    #[test]
    fn test_parse_generic_struct() {
        let source = "struct Stack<T> { data: T, size: i64 }";
        let items = parse_module(source);
        let structs: Vec<_> = items
            .iter()
            .filter_map(|i| {
                if let Item::StructDef(s) = i {
                    Some(s)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(structs.len(), 1);
        assert_eq!(structs[0].name, "Stack");
        assert_eq!(structs[0].type_params, vec!["T"]);
    }

    #[test]
    fn test_parse_enum() {
        let source = "enum Option<T> { Some(T), None }";
        let items = parse_module(source);
        let enums: Vec<_> = items
            .iter()
            .filter_map(|i| {
                if let Item::EnumDef(e) = i {
                    Some(e)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(enums.len(), 1);
        assert_eq!(enums[0].name, "Option");
        assert_eq!(enums[0].type_params, vec!["T"]);
        assert_eq!(enums[0].variants.len(), 2);
    }

    #[test]
    fn test_parse_recursive_enum() {
        let source = "enum List { Cons(i64, List), Nil }";
        let items = parse_module(source);
        let enums: Vec<_> = items
            .iter()
            .filter_map(|i| {
                if let Item::EnumDef(e) = i {
                    Some(e)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(enums.len(), 1);
        assert!(enums[0].is_recursive);
        assert!(enums[0].variants[0].is_recursive);
    }

    #[test]
    fn test_parse_trait() {
        let source = r#"
trait Comparable {
    fn leq(a: Self, b: Self) -> bool;
    law reflexive: leq(x, x) == true;
}
"#;
        let items = parse_module(source);
        let traits: Vec<_> = items
            .iter()
            .filter_map(|i| {
                if let Item::TraitDef(t) = i {
                    Some(t)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(traits.len(), 1);
        assert_eq!(traits[0].name, "Comparable");
        assert_eq!(traits[0].methods.len(), 1);
        assert_eq!(traits[0].laws.len(), 1);
    }

    #[test]
    fn test_parse_trait_with_constraint() {
        let source = r#"
trait SafeDiv {
    fn div(a: Self, b: Self where v != 0) -> Self;
}
"#;
        let items = parse_module(source);
        let traits: Vec<_> = items
            .iter()
            .filter_map(|i| {
                if let Item::TraitDef(t) = i {
                    Some(t)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(traits.len(), 1);
        assert_eq!(traits[0].methods[0].param_constraints.len(), 2);
        assert_eq!(
            traits[0].methods[0].param_constraints[1],
            Some("v != 0".to_string())
        );
    }

    #[test]
    fn test_parse_impl() {
        let source = r#"
trait Comparable {
    fn leq(a: Self, b: Self) -> bool;
}

impl Comparable for i64 {
    fn leq(a: i64, b: i64) -> bool {
        a <= b
    }
}
"#;
        let items = parse_module(source);
        let impls: Vec<_> = items
            .iter()
            .filter_map(|i| {
                if let Item::ImplDef(im) = i {
                    Some(im)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(impls.len(), 1);
        assert_eq!(impls[0].trait_name, "Comparable");
        assert_eq!(impls[0].target_type, "i64");
    }

    #[test]
    fn test_parse_atom_basic() {
        let source = r#"
atom add(a: i64, b: i64)
    requires: a >= 0 && b >= 0;
    ensures: result == a + b;
    body: a + b;
"#;
        let items = parse_module(source);
        let atoms: Vec<_> = items
            .iter()
            .filter_map(|i| if let Item::Atom(a) = i { Some(a) } else { None })
            .collect();
        assert_eq!(atoms.len(), 1);
        assert_eq!(atoms[0].name, "add");
        assert_eq!(atoms[0].params.len(), 2);
    }

    #[test]
    fn test_parse_atom_with_trait_bounds() {
        let source = r#"
atom max_val<T: Comparable>(a: T, b: T)
    requires: true;
    ensures: result >= a && result >= b;
    body: if leq(a, b) { b } else { a };
"#;
        let items = parse_module(source);
        let atoms: Vec<_> = items
            .iter()
            .filter_map(|i| if let Item::Atom(a) = i { Some(a) } else { None })
            .collect();
        assert_eq!(atoms.len(), 1);
        assert_eq!(atoms[0].type_params, vec!["T"]);
    }

    #[test]
    fn test_parse_resource_def() {
        let source = "resource mutex_a priority: 1 mode: exclusive;";
        let items = parse_module(source);
        let resources: Vec<_> = items
            .iter()
            .filter_map(|i| {
                if let Item::ResourceDef(r) = i {
                    Some(r)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].name, "mutex_a");
        assert_eq!(resources[0].priority, 1);
        assert_eq!(resources[0].mode, ResourceMode::Exclusive);
    }

    #[test]
    fn test_parse_async_atom() {
        let source = r#"
async atom fetch(url: i64)
    requires: url >= 0;
    ensures: result >= 0;
    body: url;
"#;
        let items = parse_module(source);
        let atoms: Vec<_> = items
            .iter()
            .filter_map(|i| if let Item::Atom(a) = i { Some(a) } else { None })
            .collect();
        assert_eq!(atoms.len(), 1);
        assert!(atoms[0].is_async);
    }

    #[test]
    fn test_parse_trusted_atom() {
        let source = r#"
trusted atom ffi_read(fd: i64)
    requires: fd >= 0;
    ensures: result >= 0;
    body: fd;
"#;
        let items = parse_module(source);
        let atoms: Vec<_> = items
            .iter()
            .filter_map(|i| if let Item::Atom(a) = i { Some(a) } else { None })
            .collect();
        assert_eq!(atoms.len(), 1);
        assert_eq!(atoms[0].trust_level, TrustLevel::Trusted);
    }

    #[test]
    fn test_parse_ref_param() {
        let source = r#"
atom read_data(ref v: i64)
    requires: v >= 0;
    ensures: result == v;
    body: v;
"#;
        let items = parse_module(source);
        let atoms: Vec<_> = items
            .iter()
            .filter_map(|i| if let Item::Atom(a) = i { Some(a) } else { None })
            .collect();
        assert_eq!(atoms.len(), 1);
        assert!(atoms[0].params[0].is_ref);
        assert!(!atoms[0].params[0].is_ref_mut);
    }

    #[test]
    fn test_parse_ref_mut_param() {
        let source = r#"
atom write_data(ref mut v: i64)
    requires: v >= 0;
    ensures: result >= 0;
    body: v;
"#;
        let items = parse_module(source);
        let atoms: Vec<_> = items
            .iter()
            .filter_map(|i| if let Item::Atom(a) = i { Some(a) } else { None })
            .collect();
        assert_eq!(atoms.len(), 1);
        assert!(!atoms[0].params[0].is_ref);
        assert!(atoms[0].params[0].is_ref_mut);
    }

    #[test]
    fn test_parse_task_group() {
        let source = r#"
atom concurrent(x: i64)
    requires: x >= 0;
    ensures: result >= 0;
    body: {
        task_group {
            task { x }
            task { x + 1 }
        }
    };
"#;
        let atom = parse_atom(source);
        assert_eq!(atom.name, "concurrent");
    }

    #[test]
    fn test_parse_effect_def() {
        let source = "effect FileWrite;";
        let items = parse_module(source);
        let effects: Vec<_> = items
            .iter()
            .filter_map(|i| {
                if let Item::EffectDef(e) = i {
                    Some(e)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0].name, "FileWrite");
    }

    #[test]
    fn test_parse_effect_with_parent() {
        let source = "effect HttpRead parent: Network;";
        let items = parse_module(source);
        let effects: Vec<_> = items
            .iter()
            .filter_map(|i| {
                if let Item::EffectDef(e) = i {
                    Some(e)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0].parent, Some("Network".to_string()));
    }

    #[test]
    fn test_parse_effect_with_includes() {
        let source = "effect IO includes: [FileRead, FileWrite];";
        let items = parse_module(source);
        let effects: Vec<_> = items
            .iter()
            .filter_map(|i| {
                if let Item::EffectDef(e) = i {
                    Some(e)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0].includes, vec!["FileRead", "FileWrite"]);
    }

    #[test]
    fn test_parse_atom_with_effects() {
        let source = r#"
atom write_log(msg: i64)
    requires: msg >= 0;
    ensures: result >= 0;
    effects: [FileWrite, ConsoleOut];
    body: msg;
"#;
        let items = parse_module(source);
        let atoms: Vec<_> = items
            .iter()
            .filter_map(|i| if let Item::Atom(a) = i { Some(a) } else { None })
            .collect();
        assert_eq!(atoms.len(), 1);
        assert_eq!(atoms[0].effects.len(), 2);
    }

    #[test]
    fn test_parse_import() {
        let source = r#"import "std/math.mm" as math;"#;
        let items = parse_module(source);
        let imports: Vec<_> = items
            .iter()
            .filter_map(|i| {
                if let Item::Import(imp) = i {
                    Some(imp)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].path, "std/math.mm");
        assert_eq!(imports[0].alias.as_deref(), Some("math"));
    }

    #[test]
    fn test_parse_import_no_alias() {
        let source = r#"import "std/math.mm";"#;
        let items = parse_module(source);
        let imports: Vec<_> = items
            .iter()
            .filter_map(|i| {
                if let Item::Import(imp) = i {
                    Some(imp)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].path, "std/math.mm");
        assert!(imports[0].alias.is_none());
    }

    #[test]
    fn test_parse_type_def() {
        let source = "type Nat = i64 where v >= 0;";
        let items = parse_module(source);
        let types: Vec<_> = items
            .iter()
            .filter_map(|i| {
                if let Item::TypeDef(t) = i {
                    Some(t)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(types.len(), 1);
        assert_eq!(types[0].name, "Nat");
    }

    #[test]
    fn test_parse_atom_with_forall() {
        let source = r#"
atom sum_range(n: i64)
    requires: n >= 0 && forall(i, 0, n, i >= 0);
    ensures: result >= 0;
    body: 0;
"#;
        let items = parse_module(source);
        let atoms: Vec<_> = items
            .iter()
            .filter_map(|i| if let Item::Atom(a) = i { Some(a) } else { None })
            .collect();
        assert_eq!(atoms.len(), 1);
        assert!(!atoms[0].forall_constraints.is_empty());
        assert_eq!(
            atoms[0].forall_constraints[0].q_type,
            QuantifierType::ForAll
        );
    }

    #[test]
    fn test_parse_atom_with_exists() {
        let source = r#"
atom find_positive(n: i64)
    requires: n > 0 && exists(i, 0, n, i > 0);
    ensures: result >= 0;
    body: 1;
"#;
        let items = parse_module(source);
        let atoms: Vec<_> = items
            .iter()
            .filter_map(|i| if let Item::Atom(a) = i { Some(a) } else { None })
            .collect();
        assert_eq!(atoms.len(), 1);
        assert!(!atoms[0].forall_constraints.is_empty());
        assert_eq!(
            atoms[0].forall_constraints[0].q_type,
            QuantifierType::Exists
        );
    }

    #[test]
    fn test_parse_atom_with_consume() {
        let source = r#"
atom take(x: i64)
    requires: x >= 0;
    ensures: result >= 0;
    consume x;
    body: x;
"#;
        let items = parse_module(source);
        let atoms: Vec<_> = items
            .iter()
            .filter_map(|i| if let Item::Atom(a) = i { Some(a) } else { None })
            .collect();
        assert_eq!(atoms.len(), 1);
        assert_eq!(atoms[0].consumed_params, vec!["x"]);
    }

    #[test]
    fn test_parse_atom_with_resources() {
        let source = r#"
atom transfer(x: i64)
    requires: x >= 0;
    ensures: result >= 0;
    resources: [db, cache];
    body: x;
"#;
        let items = parse_module(source);
        let atoms: Vec<_> = items
            .iter()
            .filter_map(|i| if let Item::Atom(a) = i { Some(a) } else { None })
            .collect();
        assert_eq!(atoms.len(), 1);
        assert_eq!(atoms[0].resources, vec!["db", "cache"]);
    }

    #[test]
    fn test_parse_expression_basic() {
        let expr = parse_expression("a + b * c");
        // Should parse as a + (b * c) due to precedence
        match expr {
            Expr::BinaryOp(_, Op::Add, _) => {}
            _ => panic!("Expected Add at top level"),
        }
    }

    #[test]
    fn test_parse_expression_comparison() {
        let expr = parse_expression("x >= 0 && y <= 10");
        match expr {
            Expr::BinaryOp(_, Op::And, _) => {}
            _ => panic!("Expected And at top level"),
        }
    }

    #[test]
    fn test_parse_expression_implies() {
        let expr = parse_expression("a >= 0 => b >= 0");
        match expr {
            Expr::BinaryOp(_, Op::Implies, _) => {}
            _ => panic!("Expected Implies at top level"),
        }
    }

    #[test]
    fn test_parse_body_let() {
        let stmt = parse_body_expr("let x = 5");
        match stmt {
            Stmt::Let { var, .. } => assert_eq!(var, "x"),
            _ => panic!("Expected Let statement"),
        }
    }

    #[test]
    fn test_parse_body_block() {
        let stmt = parse_body_expr("{ let x = 1; x + 1 }");
        match stmt {
            Stmt::Block(stmts) => assert_eq!(stmts.len(), 2),
            _ => panic!("Expected Block"),
        }
    }

    #[test]
    fn test_parse_body_if() {
        let stmt = parse_body_expr("if x > 0 { x } else { 0 }");
        match stmt {
            Stmt::Expr(Expr::IfThenElse { .. }) => {}
            _ => panic!("Expected IfThenElse"),
        }
    }

    #[test]
    fn test_parse_body_match() {
        let stmt = parse_body_expr("match x { 0 => 1, _ => 2 }");
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }) => assert_eq!(arms.len(), 2),
            _ => panic!("Expected Match"),
        }
    }

    #[test]
    fn test_parse_body_while() {
        let stmt = parse_body_expr("{ while x > 0 invariant: x >= 0 { x = x - 1 } }");
        match stmt {
            Stmt::Block(stmts) => {
                assert!(matches!(stmts[0], Stmt::While { .. }));
            }
            _ => panic!("Expected Block with While"),
        }
    }

    #[test]
    fn test_parse_atom_ref_call() {
        let expr = parse_expression("call(f, x)");
        match expr {
            Expr::CallRef { args, .. } => assert_eq!(args.len(), 1),
            _ => panic!("Expected CallRef"),
        }
    }

    #[test]
    fn test_parse_atom_ref_expr() {
        let expr = parse_expression("atom_ref(my_func)");
        match expr {
            Expr::AtomRef { name } => assert_eq!(name, "my_func"),
            _ => panic!("Expected AtomRef"),
        }
    }

    #[test]
    fn test_parse_field_access() {
        let expr = parse_expression("point.x");
        match expr {
            Expr::FieldAccess(_, field) => assert_eq!(field, "x"),
            _ => panic!("Expected FieldAccess"),
        }
    }

    #[test]
    fn test_parse_atom_ref_param() {
        let source = r#"
atom apply(x: i64, y: i64, f: atom_ref(i64, i64) -> i64)
    requires: x >= 0;
    ensures: result >= 0;
    body: call(f, x, y);
"#;
        let items = parse_module(source);
        let atoms: Vec<_> = items
            .iter()
            .filter_map(|i| if let Item::Atom(a) = i { Some(a) } else { None })
            .collect();
        assert_eq!(atoms.len(), 1);
        assert_eq!(atoms[0].params.len(), 3);
        assert_eq!(atoms[0].params[2].name, "f");
        assert_eq!(
            atoms[0].params[2].type_name.as_deref(),
            Some("atom_ref(i64, i64) -> i64")
        );
        let type_ref = atoms[0].params[2].type_ref.as_ref().unwrap();
        assert!(type_ref.is_fn_type());
        assert_eq!(type_ref.type_args.len(), 3); // 2 params + 1 return
    }

    #[test]
    fn test_parse_extern_block_c() {
        let source = r#"
extern "C" {
    fn printf(fmt: i64) -> i64;
}
"#;
        let items = parse_module(source);
        let externs: Vec<_> = items
            .iter()
            .filter_map(|i| {
                if let Item::ExternBlock(e) = i {
                    Some(e)
                } else {
                    None
                }
            })
            .collect();

        assert_eq!(externs.len(), 1);
        assert_eq!(externs[0].language, "C");
        assert_eq!(externs[0].functions.len(), 1);
        assert_eq!(externs[0].functions[0].name, "printf");
    }

    #[test]
    fn test_parse_extern_block() {
        let source = r#"
extern "Rust" {
    fn sqrt(x: f64) -> f64;
    fn abs(x: i64) -> i64;
}
"#;
        let items = parse_module(source);
        let externs: Vec<_> = items
            .iter()
            .filter_map(|i| {
                if let Item::ExternBlock(e) = i {
                    Some(e)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(externs.len(), 1);
        assert_eq!(externs[0].language, "Rust");
        assert_eq!(externs[0].functions.len(), 2);
    }

    // --- New tests: Lexer and token-based parsing ---

    #[test]
    fn test_lexer_basic() {
        let mut lexer = lexer::Lexer::new("let x = 42;");
        let tokens = lexer.tokenize();
        assert!(tokens.len() >= 5); // let, x, =, 42, ;, Eof
        assert_eq!(tokens[0].token, token::Token::Let);
    }

    #[test]
    fn test_lexer_string_literal() {
        let mut lexer = lexer::Lexer::new(r#""hello world""#);
        let tokens = lexer.tokenize();
        assert_eq!(
            tokens[0].token,
            token::Token::StringLit("hello world".to_string())
        );
    }

    #[test]
    fn test_lexer_operators() {
        let mut lexer = lexer::Lexer::new("== != >= <= => && || |> ->");
        let tokens = lexer.tokenize();
        assert_eq!(tokens[0].token, token::Token::Eq);
        assert_eq!(tokens[1].token, token::Token::Neq);
        assert_eq!(tokens[2].token, token::Token::Ge);
        assert_eq!(tokens[3].token, token::Token::Le);
        assert_eq!(tokens[4].token, token::Token::FatArrow);
        assert_eq!(tokens[5].token, token::Token::And);
        assert_eq!(tokens[6].token, token::Token::Or);
        assert_eq!(tokens[7].token, token::Token::Pipe);
        assert_eq!(tokens[8].token, token::Token::Arrow);
    }

    #[test]
    fn test_lexer_comments() {
        let mut lexer = lexer::Lexer::new("x + y // this is a comment\nz");
        let tokens = lexer.tokenize();
        // Should have: x, +, y, z, Eof
        let non_eof: Vec<_> = tokens
            .iter()
            .filter(|t| t.token != token::Token::Eof)
            .collect();
        assert_eq!(non_eof.len(), 4);
    }

    #[test]
    fn test_lexer_span_tracking() {
        let mut lexer = lexer::Lexer::new("let x = 1;\nlet y = 2;");
        let tokens = lexer.tokenize();
        assert_eq!(tokens[0].line, 1); // "let" on line 1
                                       // Find "let" on line 2
        let second_let = tokens
            .iter()
            .find(|t| t.token == token::Token::Let && t.line == 2);
        assert!(second_let.is_some());
    }

    #[test]
    fn test_parse_pipeline_ready() {
        // The lexer can tokenize |> even though the parser doesn't use it yet
        let mut lexer = lexer::Lexer::new("x |> f |> g");
        let tokens = lexer.tokenize();
        let pipes: Vec<_> = tokens
            .iter()
            .filter(|t| t.token == token::Token::Pipe)
            .collect();
        assert_eq!(pipes.len(), 2);
    }

    #[test]
    fn test_legacy_tokenize_compat() {
        let tokens = tokenize("a + b * 3");
        assert_eq!(tokens, vec!["a", "+", "b", "*", "3"]);
    }

    #[test]
    fn test_parse_struct_init() {
        let expr = parse_expression("Point { x: 1, y: 2 }");
        match expr {
            Expr::StructInit { type_name, fields } => {
                assert_eq!(type_name, "Point");
                assert_eq!(fields.len(), 2);
            }
            _ => panic!("Expected StructInit"),
        }
    }

    #[test]
    fn test_parse_function_call() {
        let expr = parse_expression("add(1, 2)");
        match expr {
            Expr::Call(name, args) => {
                assert_eq!(name, "add");
                assert_eq!(args.len(), 2);
            }
            _ => panic!("Expected Call"),
        }
    }

    #[test]
    fn test_parse_array_access() {
        let expr = parse_expression("arr[0]");
        match expr {
            Expr::ArrayAccess(name, _) => assert_eq!(name, "arr"),
            _ => panic!("Expected ArrayAccess"),
        }
    }

    // --- Regression tests for keyword field access and function call fixes ---

    #[test]
    fn test_parse_keyword_field_access() {
        // Keywords like "mode", "priority" must work as field names after "."
        let expr = parse_expression("config.mode");
        match expr {
            Expr::FieldAccess(_, field) => assert_eq!(field, "mode"),
            _ => panic!("Expected FieldAccess for keyword field, got {:?}", expr),
        }
    }

    #[test]
    fn test_parse_keyword_field_access_priority() {
        let expr = parse_expression("resource.priority");
        match expr {
            Expr::FieldAccess(_, field) => assert_eq!(field, "priority"),
            _ => panic!("Expected FieldAccess for keyword field, got {:?}", expr),
        }
    }

    #[test]
    fn test_parse_keyword_field_access_exclusive() {
        let expr = parse_expression("lock.exclusive");
        match expr {
            Expr::FieldAccess(_, field) => assert_eq!(field, "exclusive"),
            _ => panic!("Expected FieldAccess for keyword field, got {:?}", expr),
        }
    }

    #[test]
    fn test_parse_keyword_as_function_call() {
        // Keywords used as function names in expression context
        let expr = parse_expression("shared(x)");
        match expr {
            Expr::Call(name, args) => {
                assert_eq!(name, "shared");
                assert_eq!(args.len(), 1);
            }
            _ => panic!("Expected Call for keyword function, got {:?}", expr),
        }
    }

    #[test]
    fn test_parse_implies_left_associative() {
        // a => b => c should parse as (a => b) => c (left-associative)
        let expr = parse_expression("a => b => c");
        match expr {
            Expr::BinaryOp(lhs, Op::Implies, _rhs) => {
                // lhs should be (a => b)
                match *lhs {
                    Expr::BinaryOp(_, Op::Implies, _) => {}
                    _ => panic!("Expected nested Implies on left side, got {:?}", lhs),
                }
            }
            _ => panic!("Expected Implies at top level, got {:?}", expr),
        }
    }

    #[test]
    fn test_parse_keyword_variable_assignment() {
        // Keywords used as variable names must support assignment
        let stmt = parse_body_expr("{ mode = mode - 1 }");
        match stmt {
            Stmt::Block(stmts) => {
                assert_eq!(stmts.len(), 1);
                match &stmts[0] {
                    Stmt::Assign { var, .. } => assert_eq!(var, "mode"),
                    other => panic!("Expected Assign for keyword variable, got {:?}", other),
                }
            }
            _ => panic!("Expected Block"),
        }
    }

    // =========================================================================
    // Lambda parsing tests (Part 3-9)
    // =========================================================================

    #[test]
    fn test_parse_lambda_simple() {
        // |x| x + 1
        let expr = parse_expression("|x| x + 1");
        match expr {
            Expr::Lambda {
                params,
                return_type,
                ..
            } => {
                assert_eq!(params.len(), 1);
                assert_eq!(params[0].name, "x");
                assert!(params[0].type_ref.is_none());
                assert!(return_type.is_none());
            }
            _ => panic!("Expected Lambda, got {:?}", expr),
        }
    }

    #[test]
    fn test_parse_lambda_typed_params() {
        // |x: i64, y: i64| -> i64 { x + y }
        let expr = parse_expression("|x: i64, y: i64| -> i64 { x + y }");
        match expr {
            Expr::Lambda {
                params,
                return_type,
                ..
            } => {
                assert_eq!(params.len(), 2);
                assert_eq!(params[0].name, "x");
                assert!(params[0].type_ref.is_some());
                assert_eq!(params[0].type_ref.as_ref().unwrap().name, "i64");
                assert_eq!(params[1].name, "y");
                assert!(params[1].type_ref.is_some());
                assert_eq!(return_type, Some("i64".to_string()));
            }
            _ => panic!("Expected Lambda, got {:?}", expr),
        }
    }

    #[test]
    fn test_parse_lambda_single_typed_param() {
        // |x: i64| x
        let expr = parse_expression("|x: i64| x");
        match expr {
            Expr::Lambda {
                params,
                return_type,
                ..
            } => {
                assert_eq!(params.len(), 1);
                assert_eq!(params[0].name, "x");
                assert!(params[0].type_ref.is_some());
                assert_eq!(params[0].type_ref.as_ref().unwrap().name, "i64");
                assert!(return_type.is_none());
            }
            _ => panic!("Expected Lambda, got {:?}", expr),
        }
    }

    #[test]
    fn test_parse_lambda_block_body() {
        // |x| { let y = x + 1; y }
        let expr = parse_expression("|x| { let y = x + 1; y }");
        match expr {
            Expr::Lambda { params, body, .. } => {
                assert_eq!(params.len(), 1);
                assert_eq!(params[0].name, "x");
                match *body {
                    Stmt::Block(_) => {}
                    _ => panic!("Expected Block body"),
                }
            }
            _ => panic!("Expected Lambda, got {:?}", expr),
        }
    }

    #[test]
    fn test_where_bounds_effect_bound() {
        // atom pipe<E: Effect>(...) のパースが成功し、
        // where_bounds に Effect 境界が記録されることを確認
        let source = r#"
effect FileWrite;

atom pipe<E: Effect>(x: i64)
    requires: true;
    ensures: true;
    body: x;
"#;
        let items = parse_module(source);
        let atoms: Vec<_> = items
            .iter()
            .filter_map(|i| if let Item::Atom(a) = i { Some(a) } else { None })
            .collect();
        assert_eq!(atoms[0].name, "pipe");
        assert_eq!(atoms[0].type_params, vec!["E"]);
        assert_eq!(atoms[0].where_bounds.len(), 1);
        assert_eq!(atoms[0].where_bounds[0].param, "E");
        assert_eq!(atoms[0].where_bounds[0].bounds, vec!["Effect"]);
    }

    #[test]
    fn test_where_bounds_trait_bound_check() {
        // ModuleEnv のトレイト境界チェックが正しく動作するか確認
        let source = r#"
trait Comparable {
    atom compare(a: i64, b: i64) -> i64
        requires: true;
        ensures: true;
        body: a;
}

impl Comparable for i64 {
    atom compare(a: i64, b: i64) -> i64
        requires: true;
        ensures: true;
        body: a;
}
"#;
        let items = parse_module(source);

        let mut module_env = crate::verification::ModuleEnv::new();
        crate::verification::register_builtin_traits(&mut module_env);
        for item in &items {
            match item {
                Item::TraitDef(t) => module_env.register_trait(t),
                Item::ImplDef(i) => module_env.register_impl(i),
                _ => {}
            }
        }

        // i64 は Comparable を実装しているのでチェック成功
        assert!(
            module_env
                .check_trait_bounds("i64", &["Comparable".to_string()])
                .is_ok(),
            "i64 should satisfy Comparable bound"
        );

        // f64 は Comparable を実装していないのでチェック失敗
        assert!(
            module_env
                .check_trait_bounds("f64", &["Comparable".to_string()])
                .is_err(),
            "f64 should NOT satisfy Comparable bound"
        );
    }

    #[test]
    fn test_has_effect_def() {
        // ModuleEnv.has_effect_def() が正しく動作するか確認
        let source = r#"
effect FileWrite;
effect Network;
"#;
        let items = parse_module(source);

        let mut module_env = crate::verification::ModuleEnv::new();
        for item in &items {
            if let Item::EffectDef(e) = item {
                module_env.register_effect(e);
            }
        }

        assert!(
            module_env.has_effect_def("FileWrite"),
            "FileWrite should be a known effect"
        );
        assert!(
            module_env.has_effect_def("Network"),
            "Network should be a known effect"
        );
        assert!(
            !module_env.has_effect_def("Console"),
            "Console should NOT be a known effect"
        );
    }

    #[test]
    fn test_parse_type_ref_with_effect() {
        let tr = parse_type_ref("atom_ref(i64) -> i64 with E");
        assert!(tr.is_fn_type());
        assert_eq!(tr.effect_set, Some(vec!["E".to_string()]));
    }

    #[test]
    fn test_parse_type_ref_with_multiple_effects() {
        let tr = parse_type_ref("atom_ref(i64) -> i64 with [FileWrite, Network]");
        assert!(tr.is_fn_type());
        assert_eq!(
            tr.effect_set,
            Some(vec!["FileWrite".to_string(), "Network".to_string()])
        );
    }

    #[test]
    fn test_parse_type_ref_without_effect() {
        let tr = parse_type_ref("atom_ref(i64) -> i64");
        assert!(tr.is_fn_type());
        assert_eq!(tr.effect_set, None);
    }

    #[test]
    fn test_parse_effect_polymorphic_atom() {
        let source = r#"
effect FileWrite;

atom pipe<E: Effect>(f: atom_ref(i64) -> i64 with E)
    effects: [E];
    requires: true;
    ensures: true;
    body: call(f, 42);
"#;
        let items = parse_module(source);
        let atoms: Vec<_> = items
            .iter()
            .filter_map(|i| if let Item::Atom(a) = i { Some(a) } else { None })
            .collect();

        assert_eq!(atoms[0].name, "pipe");
        assert_eq!(atoms[0].type_params, vec!["E"]);
        assert_eq!(atoms[0].where_bounds[0].bounds, vec!["Effect"]);
        assert_eq!(atoms[0].effects.len(), 1);
        assert_eq!(atoms[0].effects[0].name, "E");

        // Verify parameter f has effect_set
        let f_param = &atoms[0].params[0];
        assert_eq!(f_param.name, "f");
        if let Some(ref type_ref) = f_param.type_ref {
            assert!(type_ref.is_fn_type());
            assert_eq!(type_ref.effect_set, Some(vec!["E".to_string()]));
        } else {
            panic!("Expected type_ref for parameter f");
        }
    }

    #[test]
    fn test_display_name_with_effect() {
        let tr = parse_type_ref("atom_ref(i64) -> i64 with E");
        let display = tr.display_name();
        assert!(
            display.contains("with E"),
            "display_name should contain 'with E': {}",
            display
        );
    }
}
