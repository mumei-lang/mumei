use crate::hir::{HirAtom, HirExpr, HirStmt};
use crate::parser::{EnumDef, ImplDef, ImportDecl, Op, StructDef, TraitDef};

/// 型名をベース型に解決する（transpiler ローカル版）
fn resolve_base_type(name: &str) -> String {
    name.to_string()
}

/// import 宣言から Go のモジュールヘッダーを生成する
/// 例: package main\nimport "path/to/math"
pub fn transpile_module_header_go(imports: &[ImportDecl], module_name: &str) -> String {
    let mut lines = Vec::new();
    lines.push(format!("package {}", module_name));
    lines.push(String::new());

    // import ブロック
    let mut import_paths = Vec::new();
    for import in imports {
        let pkg_name = import.alias.as_deref().unwrap_or_else(|| {
            import
                .path
                .rsplit('/')
                .next()
                .unwrap_or(&import.path)
                .trim_end_matches(".mm")
        });
        import_paths.push(format!("\t\"{}\"", pkg_name));
    }
    if !import_paths.is_empty() {
        lines.push("import (".to_string());
        lines.extend(import_paths);
        lines.push(")".to_string());
        lines.push(String::new());
    }
    lines.join("\n")
}

/// Enum 定義を Go の const + type に変換する
pub fn transpile_enum_go(enum_def: &EnumDef) -> String {
    let mut lines = Vec::new();
    lines.push(format!("// Verified Enum: {}", enum_def.name));
    lines.push(format!("type {} int64", enum_def.name));
    lines.push(String::new());
    lines.push("const (".to_string());
    for (i, variant) in enum_def.variants.iter().enumerate() {
        if i == 0 {
            lines.push(format!("\t{} {} = iota", variant.name, enum_def.name));
        } else {
            lines.push(format!("\t{}", variant.name));
        }
    }
    lines.push(")".to_string());
    lines.join("\n")
}

/// Struct 定義を Go の struct に変換する（Go 1.18+ Generics 対応）
pub fn transpile_struct_go(struct_def: &StructDef) -> String {
    let mut lines = Vec::new();
    lines.push(format!("// Verified Struct: {}", struct_def.name));
    // Generics: 型パラメータがある場合は [T any, U any] を付与（Go 1.18+）
    let type_params_str = if struct_def.type_params.is_empty() {
        String::new()
    } else {
        let params: Vec<String> = struct_def
            .type_params
            .iter()
            .map(|p| format!("{} any", p))
            .collect();
        format!("[{}]", params.join(", "))
    };
    lines.push(format!(
        "type {}{} struct {{",
        struct_def.name, type_params_str
    ));
    for field in &struct_def.fields {
        let go_type = map_type_go(Some(field.type_name.as_str()));
        if let Some(constraint) = &field.constraint {
            lines.push(format!("\t// where {}", constraint));
        }
        // Go のフィールド名は大文字始まり（エクスポート）
        let capitalized = capitalize_first(&field.name);
        lines.push(format!("\t{} {}", capitalized, go_type));
    }
    lines.push("}".to_string());
    lines.join("\n")
}

fn capitalize_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

/// Trait 定義を Go の interface に変換する
pub fn transpile_trait_go(trait_def: &TraitDef) -> String {
    let mut lines = Vec::new();
    for (law_name, law_expr) in &trait_def.laws {
        lines.push(format!("// Law {}: {}", law_name, law_expr));
    }
    lines.push(format!("type {} interface {{", trait_def.name));
    for method in &trait_def.methods {
        let go_ret = if method.return_type == "bool" {
            "bool"
        } else {
            "int64"
        };
        let params: Vec<String> = method
            .param_types
            .iter()
            .enumerate()
            .map(|(i, _)| {
                let name = if i == 0 {
                    "a"
                } else if i == 1 {
                    "b"
                } else {
                    "c"
                };
                format!("{} int64", name)
            })
            .collect();
        lines.push(format!(
            "\t{}({}) {}",
            capitalize_first(&method.name),
            params.join(", "),
            go_ret
        ));
    }
    lines.push("}".to_string());
    lines.join("\n")
}

/// Impl 定義を Go のメソッドレシーバに変換する
pub fn transpile_impl_go(impl_def: &ImplDef) -> String {
    let mut lines = Vec::new();
    let go_type = map_type_go(Some(&impl_def.target_type));
    lines.push(format!("// impl {} for {}", impl_def.trait_name, go_type));
    for (method_name, method_body) in &impl_def.method_bodies {
        lines.push(format!(
            "func {}{}(a, b {}) {} {{ return {} }}",
            go_type,
            capitalize_first(method_name),
            go_type,
            if method_body.contains("==") || method_body.contains("<=") {
                "bool"
            } else {
                &go_type
            },
            method_body
        ));
    }
    lines.join("\n")
}

pub fn transpile_to_go(hir_atom: &HirAtom) -> String {
    let atom = &hir_atom.atom;
    // パラメータの型を精緻型名からマッピング
    // ref mut はポインタ型 *T、ref は値渡し（Go は暗黙的に参照渡し）
    let params: Vec<String> = atom
        .params
        .iter()
        .map(|p| {
            let go_type = map_type_go(p.type_name.as_deref());
            if p.is_ref_mut {
                format!("{} *{}", p.name, go_type)
            } else {
                format!("{} {}", p.name, go_type)
            }
        })
        .collect();
    let params_str = params.join(", ");

    // ボディの変換（HIR から直接）
    // トップレベルの body が純粋な式（HirStmt::Expr）の場合、return を補う。
    // Block の場合は内部で tail_expr に return が付与されるため不要。
    let body = {
        let raw = format_hir_stmt_go(&hir_atom.body);
        if needs_return_prefix_go(&raw) {
            format!("return {}", raw)
        } else {
            raw
        }
    };

    // mathパッケージが必要な関数(sqrt等)があるか簡易チェック
    let imports = if atom.body_expr.contains("sqrt") {
        "import \"math\"\n\n"
    } else {
        ""
    };

    let async_comment = if atom.is_async {
        "// NOTE: This function is async (use goroutine for concurrent execution)\n"
    } else {
        ""
    };
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
        format!("// Effects: [{}]\n", effects_str.join(", "))
    };
    format!(
        "{}{}{}// {} is a verified Atom.\n// Requires: {}\n// Ensures: {}\nfunc {}({}) int64 {{\n    {}\n}}",
        imports, async_comment, effects_comment, atom.name, atom.requires, atom.ensures, atom.name, params_str, body
    )
}

fn map_type_go(type_name: Option<&str>) -> String {
    match type_name {
        Some(name) => {
            // 関数型パラメータ: atom_ref(T1, T2) -> R → func(T1, T2) R
            if name.starts_with("atom_ref(") {
                let tr = crate::parser::parse_type_ref(name);
                if tr.is_fn_type() {
                    let params: Vec<String> = tr.type_args[..tr.type_args.len() - 1]
                        .iter()
                        .map(|a| map_type_go(Some(&a.name)))
                        .collect();
                    let ret = map_type_go(Some(&tr.type_args.last().unwrap().name));
                    return format!("func({}) {}", params.join(", "), ret);
                }
            }
            let base = resolve_base_type(name);
            match base.as_str() {
                "f64" => "float64".to_string(),
                "u64" => "uint64".to_string(),
                _ => "int64".to_string(),
            }
        }
        None => "int64".to_string(), // デフォルト
    }
}

fn format_hir_expr_go(expr: &HirExpr) -> String {
    match expr {
        HirExpr::Number(n) => n.to_string(),
        HirExpr::Float(f) => format!("{:.15}", f), // Type System 2.0: 浮動小数点
        HirExpr::Variable(v) => v.clone(),
        HirExpr::ArrayAccess(name, idx) => format!("{}[{}]", name, format_hir_expr_go(idx)),

        HirExpr::Call { name, args, .. } => {
            // Standard Library 対応
            let args_str: Vec<String> = args.iter().map(format_hir_expr_go).collect();
            match name.as_str() {
                "sqrt" => format!("math.Sqrt({})", args_str.join(", ")),
                "len" => format!("int64(len({}))", args_str.join(", ")),
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
                format_hir_expr_go(l),
                op_str,
                format_hir_expr_go(r)
            )
        }

        HirExpr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            format!(
                "if {} {{\n        {}\n    }} else {{\n        {}\n    }}",
                format_hir_expr_go(cond),
                format_hir_stmt_go(then_branch),
                format_hir_stmt_go(else_branch)
            )
        }

        HirExpr::StructInit { type_name, fields } => {
            let field_strs: Vec<String> = fields
                .iter()
                .map(|(name, expr)| format!("{}: {}", name, format_hir_expr_go(expr)))
                .collect();
            format!("{}{{{}}}", type_name, field_strs.join(", "))
        }

        HirExpr::FieldAccess(expr, field) => {
            format!("{}.{}", format_hir_expr_go(expr), field)
        }

        HirExpr::Match { target, arms } => {
            // Go には match がないため switch 文に変換
            let target_str = format_hir_expr_go(target);
            let mut cases = Vec::new();
            for arm in arms {
                let body = format_hir_stmt_go(&arm.body);
                match &arm.pattern {
                    crate::parser::Pattern::Literal(n) => {
                        cases.push(format!("case {}:\n        return {}", n, body));
                    }
                    crate::parser::Pattern::Variant { variant_name, .. } => {
                        cases.push(format!(
                            "// {}\n        case /* {} */:\n        return {}",
                            variant_name, variant_name, body
                        ));
                    }
                    crate::parser::Pattern::Wildcard | crate::parser::Pattern::Variable(_) => {
                        cases.push(format!("default:\n        return {}", body));
                    }
                }
            }
            format!(
                "switch {} {{\n    {}\n    }}",
                target_str,
                cases.join("\n    ")
            )
        }

        HirExpr::Async { body } => {
            // Go: goroutine + channel パターン
            let body_str = format_hir_stmt_go(body);
            format!("func() int64 {{\n        ch := make(chan int64, 1)\n        go func() {{ ch <- func() int64 {{ {} }}() }}()\n        return <-ch\n    }}()", body_str)
        }
        HirExpr::Await { expr } => {
            // Go: channel receive（goroutine の結果を待機）
            let expr_str = format_hir_expr_go(expr);
            format!("<-{}", expr_str)
        }
        HirExpr::Task { body, .. } => {
            let body_str = format_hir_stmt_go(body);
            format!("func() int64 {{\n        ch := make(chan int64, 1)\n        go func() {{ ch <- func() int64 {{ {} }}() }}()\n        return <-ch\n    }}()", body_str)
        }
        HirExpr::TaskGroup { children, .. } => {
            let tasks: Vec<String> = children.iter().map(format_hir_stmt_go).collect();
            format!("/* task_group */ func() int64 {{ {} }}()", tasks.join("; "))
        }
        // Higher-order functions (Phase A): atom_ref + call
        HirExpr::AtomRef { name } => {
            // Go では関数名がそのまま第一級値として使える
            name.clone()
        }
        HirExpr::CallRef { callee, args } => {
            let callee_str = format_hir_expr_go(callee);
            let args_str: Vec<String> = args.iter().map(format_hir_expr_go).collect();
            format!("{}({})", callee_str, args_str.join(", "))
        }
        HirExpr::Perform {
            effect,
            operation,
            args,
            ..
        } => {
            // Effects: perform Effect.operation(args) → function call
            let args_str: Vec<String> = args.iter().map(format_hir_expr_go).collect();
            format!(
                "/* perform {}.{} */ {}{}({})",
                effect,
                operation,
                effect,
                capitalize_first(operation),
                args_str.join(", ")
            )
        }
        HirExpr::Lambda {
            params,
            return_type,
            body,
            ..
        } => {
            let params_str: Vec<String> = params
                .iter()
                .map(|p| {
                    if let Some(ref tr) = p.type_ref {
                        format!(
                            "{} {}",
                            p.name,
                            crate::transpiler::golang::map_type_go(Some(&tr.name))
                        )
                    } else {
                        format!("{} int64", p.name)
                    }
                })
                .collect();
            let ret = return_type
                .as_ref()
                .map(|r| {
                    format!(
                        " {}",
                        crate::transpiler::golang::map_type_go(Some(r.as_str()))
                    )
                })
                .unwrap_or_else(|| " int64".to_string());
            let body_str = format_hir_stmt_go(body);
            let body_with_return = if needs_return_prefix_go(&body_str) {
                format!("return {}", body_str)
            } else {
                body_str
            };
            format!(
                "func({}){} {{ {} }}",
                params_str.join(", "),
                ret,
                body_with_return
            )
        }
        // Plan 8: Channel operations transpiled to Go native channels
        HirExpr::ChanSend { channel, value } => {
            format!(
                "{} <- {}",
                format_hir_expr_go(channel),
                format_hir_expr_go(value)
            )
        }
        HirExpr::ChanRecv { channel } => {
            format!("<-{}", format_hir_expr_go(channel))
        }
    }
}

fn format_hir_stmt_go(stmt: &HirStmt) -> String {
    match stmt {
        HirStmt::Let { var, value, .. } => {
            match value.as_ref() {
                HirExpr::IfThenElse {
                    cond,
                    then_branch,
                    else_branch,
                } => {
                    format!(
                        "var {} int64\n    if {} {{\n        {} = {}\n    }} else {{\n        {} = {}\n    }}",
                        var, format_hir_expr_go(cond), var, format_hir_stmt_go(then_branch), var, format_hir_stmt_go(else_branch)
                    )
                }
                _ => {
                    // 型推論を利用した定義
                    format!("{} := {}", var, format_hir_expr_go(value))
                }
            }
        }
        HirStmt::Assign { var, value } => {
            format!("{} = {}", var, format_hir_expr_go(value))
        }
        HirStmt::While {
            cond,
            invariant,
            decreases: _,
            body,
        } => {
            format!(
                "// invariant: {}\n    for {} {{\n        {}\n    }}",
                format_hir_expr_go(invariant),
                format_hir_expr_go(cond),
                format_hir_stmt_go(body)
            )
        }
        HirStmt::Block { stmts, tail_expr } => {
            let mut lines: Vec<String> = stmts.iter().map(format_hir_stmt_go).collect();
            if let Some(tail) = tail_expr {
                let tail_code = format_hir_expr_go(tail);
                // Go では if/switch/for は文であり return の右辺にできない。
                // 旧コードの heuristic guard を再現する。
                if tail_code.starts_with("if")
                    || tail_code.starts_with("switch")
                    || tail_code.starts_with("for")
                {
                    lines.push(tail_code);
                } else {
                    lines.push(format!("return {}", tail_code));
                }
            }
            lines.join("\n    ")
        }
        HirStmt::Acquire { resource, body } => {
            // Go: 即時実行関数リテラルでスコープを限定し、defer でブロック終了時に Unlock する。
            let body_str = format_hir_stmt_go(body);
            format!("func() int64 {{\n        {r}.Lock()\n        defer {r}.Unlock()\n        return {body}\n    }}()", r = resource, body = body_str)
        }
        HirStmt::Expr(expr) => format_hir_expr_go(expr),
    }
}

/// トップレベル body の出力に `return` プレフィックスが必要かを判定する。
/// 文（let, if, for, switch, var, コメント, return 済み）には不要。
fn needs_return_prefix_go(code: &str) -> bool {
    // 既に return / 文キーワード / 代入 / コメントで始まっていれば不要
    !(code.starts_with("return ")
        || code.starts_with("if ")
        || code.starts_with("for ")
        || code.starts_with("switch ")
        || code.starts_with("var ")
        || code.starts_with("//")
        || code.contains(":=")
        || code.contains(" = "))
}
