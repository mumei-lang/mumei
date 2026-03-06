use crate::parser::{
    parse_expression, Atom, EnumDef, Expr, ImplDef, ImportDecl, Op, StructDef, TraitDef,
};

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

pub fn transpile_to_go(atom: &Atom) -> String {
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

    // ボディのパースと変換
    let body = format_expr_go(&parse_expression(&atom.body_expr));

    // mathパッケージが必要な関数(sqrt等)があるか簡易チェック（実用上はASTを走査すべきですが、ここでは含めます）
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
    format!(
        "{}{}// {} is a verified Atom.\n// Requires: {}\n// Ensures: {}\nfunc {}({}) int64 {{\n    {}\n}}",
        imports, async_comment, atom.name, atom.requires, atom.ensures, atom.name, params_str, body
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

fn format_expr_go(expr: &Expr) -> String {
    match expr {
        Expr::Number(n) => n.to_string(),
        Expr::Float(f) => format!("{:.15}", f), // Type System 2.0: 浮動小数点
        Expr::Variable(v) => v.clone(),
        Expr::ArrayAccess(name, idx) => format!("{}[{}]", name, format_expr_go(idx)),

        Expr::Call(name, args) => {
            // Standard Library 対応
            let args_str: Vec<String> = args.iter().map(format_expr_go).collect();
            match name.as_str() {
                "sqrt" => format!("math.Sqrt({})", args_str.join(", ")),
                "len" => format!("int64(len({}))", args_str.join(", ")),
                _ => format!("{}({})", name, args_str.join(", ")),
            }
        }

        Expr::BinaryOp(l, op, r) => {
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
            format!("({} {} {})", format_expr_go(l), op_str, format_expr_go(r))
        }

        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            format!(
                "if {} {{\n        {}\n    }} else {{\n        {}\n    }}",
                format_expr_go(cond),
                format_expr_go(then_branch),
                format_expr_go(else_branch)
            )
        }

        Expr::While {
            cond,
            invariant,
            decreases: _,
            body,
        } => {
            format!(
                "// invariant: {}\n    for {} {{\n        {}\n    }}",
                format_expr_go(invariant),
                format_expr_go(cond),
                format_expr_go(body)
            )
        }

        Expr::Let { var, value } => {
            match value.as_ref() {
                Expr::IfThenElse {
                    cond,
                    then_branch,
                    else_branch,
                } => {
                    format!(
                        "var {} int64\n    if {} {{\n        {} = {}\n    }} else {{\n        {} = {}\n    }}",
                        var, format_expr_go(cond), var, format_expr_go(then_branch), var, format_expr_go(else_branch)
                    )
                }
                _ => {
                    // 型推論を利用した定義
                    format!("{} := {}", var, format_expr_go(value))
                }
            }
        }

        Expr::Assign { var, value } => {
            format!("{} = {}", var, format_expr_go(value))
        }

        Expr::Block(stmts) => stmts
            .iter()
            .map(|s| {
                let code = format_expr_go(s);
                if code.starts_with("if")
                    || code.contains(":=")
                    || code.contains(" = ")
                    || code.starts_with("for")
                    || code.starts_with("//")
                    || code.starts_with("var")
                {
                    code
                } else {
                    format!("return {}", code)
                }
            })
            .collect::<Vec<_>>()
            .join("\n    "),

        Expr::StructInit { type_name, fields } => {
            let field_strs: Vec<String> = fields
                .iter()
                .map(|(name, expr)| format!("{}: {}", name, format_expr_go(expr)))
                .collect();
            format!("{}{{{}}}", type_name, field_strs.join(", "))
        }

        Expr::FieldAccess(expr, field) => {
            format!("{}.{}", format_expr_go(expr), field)
        }

        Expr::Match { target, arms } => {
            // Go には match がないため switch 文に変換
            let target_str = format_expr_go(target);
            let mut cases = Vec::new();
            for arm in arms {
                let body = format_expr_go(&arm.body);
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

        Expr::Acquire { resource, body } => {
            // Go: 即時実行関数リテラルでスコープを限定し、defer でブロック終了時に Unlock する。
            // defer は関数スコープなので、ネストやループ内でも正しくブロック終了時に解放される。
            let body_str = format_expr_go(body);
            format!("func() int64 {{\n        {r}.Lock()\n        defer {r}.Unlock()\n        return {body}\n    }}()", r = resource, body = body_str)
        }
        Expr::Async { body } => {
            // Go: goroutine + channel パターン
            let body_str = format_expr_go(body);
            format!("func() int64 {{\n        ch := make(chan int64, 1)\n        go func() {{ ch <- func() int64 {{ {} }}() }}()\n        return <-ch\n    }}()", body_str)
        }
        Expr::Await { expr } => {
            // Go: channel receive（goroutine の結果を待機）
            let expr_str = format_expr_go(expr);
            format!("<-{}", expr_str)
        }
        Expr::Task { body, .. } => {
            let body_str = format_expr_go(body);
            format!("func() int64 {{\n        ch := make(chan int64, 1)\n        go func() {{ ch <- func() int64 {{ {} }}() }}()\n        return <-ch\n    }}()", body_str)
        }
        Expr::TaskGroup { children, .. } => {
            let tasks: Vec<String> = children.iter().map(format_expr_go).collect();
            format!("/* task_group */ func() int64 {{ {} }}()", tasks.join("; "))
        }
        // Higher-order functions (Phase A): atom_ref + call
        Expr::AtomRef { name } => {
            // Go では関数名がそのまま第一級値として使える
            name.clone()
        }
        Expr::CallRef { callee, args } => {
            let callee_str = format_expr_go(callee);
            let args_str: Vec<String> = args.iter().map(format_expr_go).collect();
            format!("{}({})", callee_str, args_str.join(", "))
        }
    }
}
