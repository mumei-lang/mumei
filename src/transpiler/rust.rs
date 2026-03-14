use crate::hir::{HirAtom, HirExpr, HirStmt};
use crate::parser::{EnumDef, ImplDef, ImportDecl, Op, StructDef, TraitDef};

/// 型名をベース型に解決する（transpiler ローカル版）
/// 精緻型の解決は ModuleEnv が担当するが、transpiler は単相化後の具体型名を受け取るため、
/// プリミティブ型のマッピングのみで十分。
fn resolve_base_type(name: &str) -> String {
    // プリミティブ型はそのまま返す。精緻型名は単相化後に具体型名に置換済み。
    name.to_string()
}

/// import 宣言から Rust のモジュールヘッダーを生成する
/// 例: mod math; use math::*;
pub fn transpile_module_header_rust(imports: &[ImportDecl], _module_name: &str) -> String {
    let mut lines = Vec::new();
    for import in imports {
        // パスからモジュール名を推定（例: "./lib/math.mm" → "math"）
        let mod_name = import.alias.as_deref().unwrap_or_else(|| {
            import
                .path
                .rsplit('/')
                .next()
                .unwrap_or(&import.path)
                .trim_end_matches(".mm")
        });
        lines.push(format!("mod {};", mod_name));
        lines.push(format!("use {}::*;", mod_name));
    }
    if !lines.is_empty() {
        lines.push(String::new()); // 空行で区切り
    }
    lines.join("\n")
}

/// Enum 定義を Rust の enum に変換する
pub fn transpile_enum_rust(enum_def: &EnumDef) -> String {
    let mut lines = Vec::new();
    lines.push(format!("/// Verified Enum: {}", enum_def.name));
    lines.push("#[derive(Debug, Clone, Copy, PartialEq)]".to_string());
    // Generics: 型パラメータがある場合は <T, U> を付与
    let type_params_str = if enum_def.type_params.is_empty() {
        String::new()
    } else {
        format!("<{}>", enum_def.type_params.join(", "))
    };
    lines.push(format!("pub enum {}{} {{", enum_def.name, type_params_str));
    for variant in &enum_def.variants {
        if variant.fields.is_empty() {
            lines.push(format!("    {},", variant.name));
        } else {
            let field_types: Vec<String> = variant
                .fields
                .iter()
                .map(|f| map_type_rust(Some(f.as_str())))
                .collect();
            lines.push(format!("    {}({}),", variant.name, field_types.join(", ")));
        }
    }
    lines.push("}".to_string());
    lines.join("\n")
}

/// Struct 定義を Rust の struct に変換する
pub fn transpile_struct_rust(struct_def: &StructDef) -> String {
    let mut lines = Vec::new();
    lines.push(format!("/// Verified Struct: {}", struct_def.name));
    lines.push("#[derive(Debug, Clone)]".to_string());
    // Generics: 型パラメータがある場合は <T, U> を付与
    let type_params_str = if struct_def.type_params.is_empty() {
        String::new()
    } else {
        format!("<{}>", struct_def.type_params.join(", "))
    };
    lines.push(format!(
        "pub struct {}{} {{",
        struct_def.name, type_params_str
    ));
    for field in &struct_def.fields {
        let rust_type = map_type_rust(Some(field.type_name.as_str()));
        if let Some(constraint) = &field.constraint {
            lines.push(format!("    /// where {}", constraint));
        }
        lines.push(format!("    pub {}: {},", field.name, rust_type));
    }
    lines.push("}".to_string());
    lines.join("\n")
}

/// Trait 定義を Rust の trait に変換する
pub fn transpile_trait_rust(trait_def: &TraitDef) -> String {
    let mut lines = Vec::new();
    // law をドキュメントコメントとして出力
    for (law_name, law_expr) in &trait_def.laws {
        lines.push(format!("/// Law {}: {}", law_name, law_expr));
    }
    lines.push(format!("pub trait {} {{", trait_def.name));
    for method in &trait_def.methods {
        let params: Vec<String> = method
            .param_types
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let param_name = if i == 0 {
                    "a"
                } else if i == 1 {
                    "b"
                } else {
                    "c"
                };
                let rust_type = if t == "Self" {
                    "Self"
                } else {
                    &map_type_rust(Some(t))
                };
                format!("{}: {}", param_name, rust_type)
            })
            .collect();
        let ret = if method.return_type == "bool" {
            "bool"
        } else {
            "Self"
        };
        lines.push(format!(
            "    fn {}({}) -> {};",
            method.name,
            params.join(", "),
            ret
        ));
    }
    lines.push("}".to_string());
    lines.join("\n")
}

/// Impl 定義を Rust の impl に変換する
pub fn transpile_impl_rust(impl_def: &ImplDef) -> String {
    let mut lines = Vec::new();
    let rust_type = map_type_rust(Some(&impl_def.target_type));
    lines.push(format!("impl {} for {} {{", impl_def.trait_name, rust_type));
    for (method_name, method_body) in &impl_def.method_bodies {
        lines.push(format!(
            "    fn {name}(a: {t}, b: {t}) -> {t} {{ {body} }}",
            name = method_name,
            t = rust_type,
            body = method_body
        ));
    }
    lines.push("}".to_string());
    lines.join("\n")
}

pub fn transpile_to_rust(hir_atom: &HirAtom) -> String {
    let atom = &hir_atom.atom;
    // 引数の型を精緻型のベース型からマッピング (Type System 2.0)
    // ref パラメータは &T に、ref mut は &mut T に、consume はそのまま T（所有権移動）に変換
    let params: Vec<String> = atom
        .params
        .iter()
        .map(|p| {
            let rust_type = map_type_rust(p.type_name.as_deref());
            if p.is_ref_mut {
                format!("{}: &mut {}", p.name, rust_type)
            } else if p.is_ref {
                format!("{}: &{}", p.name, rust_type)
            } else {
                format!("{}: {}", p.name, rust_type)
            }
        })
        .collect();
    let params_str = params.join(", ");

    let body = format_hir_stmt_rust(&hir_atom.body);

    // 戻り値型の推論: ボディに f64 リテラルや f64 パラメータが含まれていれば f64
    let has_float_param = atom.params.iter().any(|p| {
        p.type_name
            .as_deref()
            .map(|t| resolve_base_type(t) == "f64")
            .unwrap_or(false)
    });
    let return_type = if has_float_param || hir_stmt_contains_float(&hir_atom.body) {
        "f64"
    } else {
        "i64"
    };

    let async_keyword = if atom.is_async { "async " } else { "" };
    let effects_comment = if hir_atom.effect_set.effects.is_empty() {
        String::new()
    } else {
        let effects_str: Vec<String> = hir_atom
            .effect_set
            .effects
            .iter()
            .map(|name| {
                if let Some(e) = atom.effects.iter().find(|e| &e.name == name) {
                    e.to_string()
                } else {
                    name.clone()
                }
            })
            .collect();
        format!("\n/// Effects: [{}]", effects_str.join(", "))
    };
    format!(
        "/// Verified Atom: {}\n/// Requires: {}\n/// Ensures: {}{}\npub {}fn {}({}) -> {} {{\n    {}\n}}",
        atom.name, atom.requires, atom.ensures, effects_comment, async_keyword, atom.name, params_str, return_type, body
    )
}

/// HIR 式に f64 リテラルが含まれるかを再帰的にチェック
fn hir_expr_contains_float(expr: &HirExpr) -> bool {
    match expr {
        HirExpr::Float(_) => true,
        HirExpr::BinaryOp(l, _, r) => hir_expr_contains_float(l) || hir_expr_contains_float(r),
        HirExpr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            hir_expr_contains_float(cond)
                || hir_stmt_contains_float(then_branch)
                || hir_stmt_contains_float(else_branch)
        }
        HirExpr::Call { args, .. } => args.iter().any(hir_expr_contains_float),
        HirExpr::Match { target, arms } => {
            hir_expr_contains_float(target) || arms.iter().any(|a| hir_stmt_contains_float(&a.body))
        }
        HirExpr::Async { body } | HirExpr::Task { body, .. } => hir_stmt_contains_float(body),
        HirExpr::Await { expr } => hir_expr_contains_float(expr),
        HirExpr::CallRef { callee, args } => {
            hir_expr_contains_float(callee) || args.iter().any(hir_expr_contains_float)
        }
        HirExpr::TaskGroup { children, .. } => children.iter().any(hir_stmt_contains_float),
        HirExpr::Lambda { body, .. } => hir_stmt_contains_float(body),
        _ => false,
    }
}

/// HIR 文に f64 リテラルが含まれるかを再帰的にチェック
fn hir_stmt_contains_float(stmt: &HirStmt) -> bool {
    match stmt {
        HirStmt::Let { value, .. } | HirStmt::Assign { value, .. } => {
            hir_expr_contains_float(value)
        }
        HirStmt::Block { stmts, tail_expr } => {
            stmts.iter().any(hir_stmt_contains_float)
                || tail_expr
                    .as_ref()
                    .is_some_and(|e| hir_expr_contains_float(e))
        }
        HirStmt::While { cond, body, .. } => {
            hir_expr_contains_float(cond) || hir_stmt_contains_float(body)
        }
        HirStmt::Acquire { body, .. } => hir_stmt_contains_float(body),
        HirStmt::Expr(expr) => hir_expr_contains_float(expr),
    }
}

fn map_type_rust(type_name: Option<&str>) -> String {
    match type_name {
        Some(name) => {
            // 関数型パラメータ: atom_ref(T1, T2) -> R → fn(T1, T2) -> R
            if name.starts_with("atom_ref(") {
                let tr = crate::parser::parse_type_ref(name);
                if tr.is_fn_type() {
                    let params: Vec<String> = tr.type_args[..tr.type_args.len() - 1]
                        .iter()
                        .map(|a| map_type_rust(Some(&a.name)))
                        .collect();
                    let ret = map_type_rust(Some(&tr.type_args.last().unwrap().name));
                    return format!("fn({}) -> {}", params.join(", "), ret);
                }
            }
            let base = resolve_base_type(name);
            match base.as_str() {
                "f64" => "f64".to_string(),
                "u64" => "u64".to_string(),
                _ => "i64".to_string(),
            }
        }
        None => "i64".to_string(),
    }
}

/// 外側の括弧を除去するヘルパー（生成コードの不要な括弧 warning を防ぐ）
fn strip_parens(s: &str) -> &str {
    if s.starts_with('(') && s.ends_with(')') {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

fn format_hir_expr_rust(expr: &HirExpr) -> String {
    match expr {
        HirExpr::Number(n) => n.to_string(),
        HirExpr::Float(f) => {
            // Rustのリテラルとして明確にするため、.0を保証
            let s = f.to_string();
            if s.contains('.') {
                s
            } else {
                format!("{}.0", s)
            }
        }
        HirExpr::Variable(v) => v.clone(),
        HirExpr::ArrayAccess(name, idx) => {
            // インデックスは常に usize にキャスト
            format!("{}[{} as usize]", name, format_hir_expr_rust(idx))
        }

        HirExpr::Call { name, args, .. } => {
            let args_str: Vec<String> = args.iter().map(format_hir_expr_rust).collect();
            match name.as_str() {
                "sqrt" => {
                    // Rustでは f64 のメソッドとして呼び出す。整数ならキャストが必要。
                    format!("(({}) as f64).sqrt()", args_str.join(", "))
                }
                "len" => format!("{}.len() as i64", args_str.join(", ")),
                _ => format!("{}({})", name, args_str.join(", ")),
            }
        }

        HirExpr::BinaryOp(l, op, r) => {
            let op_str = match op {
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
                Op::Implies => "/* implies */",
            };
            format!(
                "({} {} {})",
                format_hir_expr_rust(l),
                op_str,
                format_hir_expr_rust(r)
            )
        }

        HirExpr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            format!(
                "if {} {{ {} }} else {{ {} }}",
                format_hir_expr_rust(cond),
                format_hir_stmt_rust(then_branch),
                format_hir_stmt_rust(else_branch)
            )
        }

        HirExpr::StructInit { type_name, fields } => {
            let field_strs: Vec<String> = fields
                .iter()
                .map(|(name, expr)| format!("{}: {}", name, format_hir_expr_rust(expr)))
                .collect();
            format!("{} {{ {} }}", type_name, field_strs.join(", "))
        }

        HirExpr::FieldAccess(expr, field) => {
            format!("{}.{}", format_hir_expr_rust(expr), field)
        }

        HirExpr::Match { target, arms } => {
            let target_str = format_hir_expr_rust(target);
            let arms_str: Vec<String> = arms
                .iter()
                .map(|arm| {
                    let pat = format_pattern_rust(&arm.pattern);
                    let guard = arm
                        .guard
                        .as_ref()
                        .map(|g| format!(" if {}", format_hir_expr_rust(g)))
                        .unwrap_or_default();
                    let body = format_hir_stmt_rust(&arm.body);
                    format!("{}{} => {}", pat, guard, body)
                })
                .collect();
            format!("match {} {{ {} }}", target_str, arms_str.join(", "))
        }

        HirExpr::Async { body } => {
            let body_str = format_hir_stmt_rust(body);
            format!("async {{ {} }}", body_str)
        }
        HirExpr::Await { expr } => {
            let expr_str = format_hir_expr_rust(expr);
            format!("{}.await", expr_str)
        }
        HirExpr::Task { body, .. } => {
            let body_str = format_hir_stmt_rust(body);
            format!("tokio::spawn(async {{ {} }})", body_str)
        }
        HirExpr::TaskGroup { children, .. } => {
            let tasks: Vec<String> = children.iter().map(format_hir_stmt_rust).collect();
            format!("tokio::join!({})", tasks.join(", "))
        }
        // Higher-order functions (Phase A): atom_ref + call
        HirExpr::AtomRef { name } => {
            // Rust では関数名がそのまま関数ポインタとして使える
            name.clone()
        }
        HirExpr::CallRef { callee, args } => {
            // Rust は関数ポインタの呼び出しが透過的
            let callee_str = format_hir_expr_rust(callee);
            let args_str: Vec<String> = args.iter().map(format_hir_expr_rust).collect();
            format!("{}({})", callee_str, args_str.join(", "))
        }
        HirExpr::Perform {
            effect,
            operation,
            args,
            ..
        } => {
            // Effects: perform Effect.operation(args) → function call
            let args_str: Vec<String> = args.iter().map(format_hir_expr_rust).collect();
            format!(
                "/* perform {}.{} */ {}_{}({})",
                effect,
                operation,
                effect.to_lowercase(),
                operation,
                args_str.join(", ")
            )
        }
        HirExpr::Lambda {
            params,
            body,
            captures,
            ..
        } => {
            let move_kw = if captures.is_empty() { "" } else { "move " };
            let params_str: Vec<String> = params
                .iter()
                .map(|p| {
                    if let Some(ref tr) = p.type_ref {
                        format!(
                            "{}: {}",
                            p.name,
                            crate::transpiler::rust::map_type_rust(Some(&tr.name))
                        )
                    } else {
                        p.name.clone()
                    }
                })
                .collect();
            let body_str = format_hir_stmt_rust(body);
            format!("{}|{}| {{ {} }}", move_kw, params_str.join(", "), body_str)
        }
    }
}

fn format_hir_stmt_rust(stmt: &HirStmt) -> String {
    match stmt {
        HirStmt::Let { var, value, .. } => {
            let val_str = format_hir_expr_rust(value);
            format!("let mut {} = {};", var, strip_parens(&val_str))
        }
        HirStmt::Assign { var, value } => {
            let val_str = format_hir_expr_rust(value);
            format!("{} = {};", var, strip_parens(&val_str))
        }
        HirStmt::While {
            cond,
            invariant,
            decreases,
            body,
        } => {
            let cond_str = format_hir_expr_rust(cond);
            let dec_comment = decreases
                .as_ref()
                .map(|d| format!(" decreases: {}", format_hir_expr_rust(d)))
                .unwrap_or_default();
            format!(
                "{{ // invariant: {}{}\n        while {} {{ {} }} \n    }}",
                format_hir_expr_rust(invariant),
                dec_comment,
                strip_parens(&cond_str),
                format_hir_stmt_rust(body)
            )
        }
        HirStmt::Block { stmts, tail_expr } => {
            let mut lines = Vec::new();
            for s in stmts {
                let code = format_hir_stmt_rust(s);
                if code.ends_with(';') || code.ends_with('}') {
                    lines.push(code);
                } else {
                    lines.push(format!("{};", code));
                }
            }
            if let Some(tail) = tail_expr {
                lines.push(strip_parens(&format_hir_expr_rust(tail)).to_string());
            }
            format!("{{\n        {}\n    }}", lines.join("\n        "))
        }
        HirStmt::Acquire { resource, body } => {
            // Rust: スコープガードパターン（MutexGuard の RAII）
            let body_str = format_hir_stmt_rust(body);
            format!(
                "{{\n        let _guard_{r} = {r}.lock().unwrap();\n        {body}\n    }}",
                r = resource,
                body = body_str
            )
        }
        HirStmt::Expr(expr) => format_hir_expr_rust(expr),
    }
}

fn format_pattern_rust(pattern: &crate::parser::Pattern) -> String {
    match pattern {
        crate::parser::Pattern::Wildcard => "_".to_string(),
        crate::parser::Pattern::Literal(n) => n.to_string(),
        crate::parser::Pattern::Variable(name) => name.clone(),
        crate::parser::Pattern::Variant {
            variant_name,
            fields,
        } => {
            if fields.is_empty() {
                variant_name.clone()
            } else {
                let field_strs: Vec<String> = fields.iter().map(format_pattern_rust).collect();
                format!("{}({})", variant_name, field_strs.join(", "))
            }
        }
    }
}
