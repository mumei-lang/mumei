use crate::hir::{HirAtom, HirExpr, HirStmt};
use crate::parser::{EnumDef, ImplDef, ImportDecl, Op, StructDef, TraitDef};

/// 型名をベース型に解決する（transpiler ローカル版）
fn resolve_base_type(name: &str) -> String {
    name.to_string()
}

/// import 宣言から TypeScript のモジュールヘッダーを生成する
/// 例: import { add } from "./lib/math";
pub fn transpile_module_header_ts(imports: &[ImportDecl]) -> String {
    let mut lines = Vec::new();
    for import in imports {
        let module_path = import.path.trim_end_matches(".mm");
        if let Some(alias) = &import.alias {
            lines.push(format!("import * as {} from \"{}\";", alias, module_path));
        } else {
            // エイリアスなしの場合、ワイルドカードインポート（モジュール名を推定）
            let mod_name = import
                .path
                .rsplit('/')
                .next()
                .unwrap_or(&import.path)
                .trim_end_matches(".mm");
            lines.push(format!(
                "import * as {} from \"{}\";",
                mod_name, module_path
            ));
        }
    }
    if !lines.is_empty() {
        lines.push(String::new()); // 空行で区切り
    }
    lines.join("\n")
}

fn map_type_ts(type_name: Option<&str>) -> String {
    match type_name {
        Some(name) => {
            // 関数型パラメータ: atom_ref(T1, T2) -> R → (arg0: number, arg1: number) => number
            if name.starts_with("atom_ref(") {
                let tr = crate::parser::parse_type_ref(name);
                if tr.is_fn_type() {
                    let params: Vec<String> = tr.type_args[..tr.type_args.len() - 1]
                        .iter()
                        .enumerate()
                        .map(|(i, _a)| format!("arg{}: number", i))
                        .collect();
                    return format!("({}) => number", params.join(", "));
                }
            }
            let base = resolve_base_type(name);
            match base.as_str() {
                "f64" | "i64" | "u64" => "number".to_string(),
                _ => "number".to_string(),
            }
        }
        None => "number".to_string(),
    }
}

/// Enum 定義を TypeScript の const enum + discriminated union に変換する（Generics 対応）
pub fn transpile_enum_ts(enum_def: &EnumDef) -> String {
    let mut lines = Vec::new();
    lines.push(format!("/** Verified Enum: {} */", enum_def.name));
    // Generics: 型パラメータがある場合は discriminated union の型に <T> を付与
    let type_params_str = if enum_def.type_params.is_empty() {
        String::new()
    } else {
        format!("<{}>", enum_def.type_params.join(", "))
    };
    lines.push(format!("export const enum {}Tag {{", enum_def.name));
    for variant in &enum_def.variants {
        lines.push(format!("    {},", variant.name));
    }
    lines.push("}".to_string());

    // Discriminated union 型も生成
    let mut union_members = Vec::new();
    for (i, variant) in enum_def.variants.iter().enumerate() {
        if variant.fields.is_empty() {
            union_members.push(format!("{{ tag: {}Tag.{} }}", enum_def.name, variant.name));
        } else {
            let field_types: Vec<String> = variant
                .fields
                .iter()
                .enumerate()
                .map(|(fi, f)| format!("field_{}: {}", fi, map_type_ts(Some(f.as_str()))))
                .collect();
            union_members.push(format!(
                "{{ tag: {}Tag.{}; {} }}",
                enum_def.name,
                variant.name,
                field_types.join("; ")
            ));
        }
        let _ = i;
    }
    lines.push(format!(
        "export type {}{} = {};",
        enum_def.name,
        type_params_str,
        union_members.join(" | ")
    ));
    lines.join("\n")
}

/// Struct 定義を TypeScript の interface に変換する（Generics 対応）
pub fn transpile_struct_ts(struct_def: &StructDef) -> String {
    let mut lines = Vec::new();
    lines.push(format!("/** Verified Struct: {} */", struct_def.name));
    // Generics: 型パラメータがある場合は <T, U> を付与
    let type_params_str = if struct_def.type_params.is_empty() {
        String::new()
    } else {
        format!("<{}>", struct_def.type_params.join(", "))
    };
    lines.push(format!(
        "export interface {}{} {{",
        struct_def.name, type_params_str
    ));
    for field in &struct_def.fields {
        let ts_type = map_type_ts(Some(field.type_name.as_str()));
        if let Some(constraint) = &field.constraint {
            lines.push(format!("    /** where {} */", constraint));
        }
        lines.push(format!("    {}: {};", field.name, ts_type));
    }
    lines.push("}".to_string());
    lines.join("\n")
}

/// Trait 定義を TypeScript の interface に変換する
pub fn transpile_trait_ts(trait_def: &TraitDef) -> String {
    let mut lines = Vec::new();
    for (law_name, law_expr) in &trait_def.laws {
        lines.push(format!("/** Law {}: {} */", law_name, law_expr));
    }
    lines.push(format!("export interface {} {{", trait_def.name));
    for method in &trait_def.methods {
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
                format!("{}: number", name)
            })
            .collect();
        let ret = if method.return_type == "bool" {
            "boolean"
        } else {
            "number"
        };
        lines.push(format!(
            "    {}({}): {};",
            method.name,
            params.join(", "),
            ret
        ));
    }
    lines.push("}".to_string());
    lines.join("\n")
}

/// Impl 定義を TypeScript のクラスに変換する
pub fn transpile_impl_ts(impl_def: &ImplDef) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "/** impl {} for {} */",
        impl_def.trait_name, impl_def.target_type
    ));
    lines.push(format!(
        "export const {}{}: {} = {{",
        impl_def.target_type, impl_def.trait_name, impl_def.trait_name
    ));
    for (method_name, method_body) in &impl_def.method_bodies {
        lines.push(format!(
            "    {}: (a: number, b: number) => {},",
            method_name, method_body
        ));
    }
    lines.push("};".to_string());
    lines.join("\n")
}

pub fn transpile_to_ts(hir_atom: &HirAtom) -> String {
    let atom = &hir_atom.atom;
    // TSでは number (f64/i64) または bigint (u64的な扱い) ですが、
    // 汎用性を考慮しすべて number として出力します。
    // ref パラメータは Readonly<T> コメントで論理的な読み取り専用を示す。
    // ref mut パラメータは @mutable JSDoc で可変参照を示す。
    // consume パラメータは @consume JSDoc で使用禁止を示す。
    let params: String = atom
        .params
        .iter()
        .map(|p| {
            if p.is_ref_mut {
                format!("/* &mut */ {}: number", p.name)
            } else if p.is_ref {
                format!("/* readonly */ {}: number", p.name)
            } else {
                format!("{}: number", p.name)
            }
        })
        .collect::<Vec<_>>()
        .join(", ");

    // トップレベルの body が純粋な式（HirStmt::Expr）の場合、return を補う。
    // Block の場合は内部で tail_expr に return が付与されるため不要。
    let body = {
        let raw = format_hir_stmt_ts(&hir_atom.body);
        if needs_return_prefix_ts(&raw) {
            format!("return {};", raw)
        } else {
            raw
        }
    };

    let async_keyword = if atom.is_async { "async " } else { "" };
    let return_type = if atom.is_async {
        "Promise<number>"
    } else {
        "number"
    };
    let effects_comment = if hir_atom.effect_set.effects.is_empty() {
        String::new()
    } else {
        format!(
            "\n * @effects [{}]",
            hir_atom
                .effect_set
                .effects
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    format!(
        "/**\n * Verified Atom: {}\n * Requires: {}\n * Ensures: {}{}\n */\n{}function {}({}): {} {{\n    {}\n}}",
        atom.name, atom.requires, atom.ensures, effects_comment, async_keyword, atom.name, params, return_type, body
    )
}

fn format_hir_expr_ts(expr: &HirExpr) -> String {
    match expr {
        HirExpr::Number(n) => n.to_string(),
        HirExpr::Float(f) => f.to_string(), // TypeScriptはそのままのリテラルでOK
        HirExpr::Variable(v) => v.clone(),
        HirExpr::ArrayAccess(name, idx) => format!("{}[{}]", name, format_hir_expr_ts(idx)),

        HirExpr::Call { name, args, .. } => {
            let args_str: Vec<String> = args.iter().map(format_hir_expr_ts).collect();
            match name.as_str() {
                "sqrt" => format!("Math.sqrt({})", args_str.join(", ")),
                "len" => format!("{}.length", args_str.join(", ")),
                _ => format!("{}({})", name, args_str.join(", ")),
            }
        }

        HirExpr::BinaryOp(l, op, r) => {
            let op_str = match op {
                Op::Add => "+",
                Op::Sub => "-",
                Op::Mul => "*",
                Op::Div => "/",
                Op::Eq => "===",
                Op::Neq => "!==",
                Op::Gt => ">",
                Op::Lt => "<",
                Op::Ge => ">=",
                Op::Le => "<=",
                Op::And => "&&",
                Op::Or => "||",
                Op::Implies => "/* implies: (!a || b) */",
            };
            format!(
                "({} {} {})",
                format_hir_expr_ts(l),
                op_str,
                format_hir_expr_ts(r)
            )
        }

        HirExpr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            format!(
                "if ({}) {{\n        {}\n    }} else {{\n        {}\n    }}",
                format_hir_expr_ts(cond),
                format_hir_stmt_ts(then_branch),
                format_hir_stmt_ts(else_branch)
            )
        }

        HirExpr::StructInit {
            type_name: _,
            fields,
        } => {
            let field_strs: Vec<String> = fields
                .iter()
                .map(|(name, expr)| format!("{}: {}", name, format_hir_expr_ts(expr)))
                .collect();
            format!("{{ {} }}", field_strs.join(", "))
        }

        HirExpr::FieldAccess(expr, field) => {
            format!("{}.{}", format_hir_expr_ts(expr), field)
        }

        HirExpr::Match { target, arms } => {
            // TypeScript では switch 文に変換
            let target_str = format_hir_expr_ts(target);
            let mut cases = Vec::new();
            for arm in arms {
                let body = format_hir_stmt_ts(&arm.body);
                match &arm.pattern {
                    crate::parser::Pattern::Literal(n) => {
                        cases.push(format!("case {}: return {};", n, body));
                    }
                    crate::parser::Pattern::Variant { variant_name, .. } => {
                        cases.push(format!("case /* {} */: return {};", variant_name, body));
                    }
                    crate::parser::Pattern::Wildcard | crate::parser::Pattern::Variable(_) => {
                        cases.push(format!("default: return {};", body));
                    }
                }
            }
            format!(
                "(() => {{ switch ({}) {{ {} }} }})()",
                target_str,
                cases.join(" ")
            )
        }

        HirExpr::Async { body } => {
            let body_str = format_hir_stmt_ts(body);
            format!("(async () => {{ {} }})()", body_str)
        }
        HirExpr::Await { expr } => {
            let expr_str = format_hir_expr_ts(expr);
            format!("await {}", expr_str)
        }
        HirExpr::Task { body, .. } => {
            let body_str = format_hir_stmt_ts(body);
            format!("(async () => {{ {} }})()", body_str)
        }
        HirExpr::TaskGroup { children, .. } => {
            let tasks: Vec<String> = children.iter().map(format_hir_stmt_ts).collect();
            format!("Promise.all([{}])", tasks.join(", "))
        }
        // Higher-order functions (Phase A): atom_ref + call
        HirExpr::AtomRef { name } => {
            // TypeScript では関数名がそのまま第一級値
            name.clone()
        }
        HirExpr::CallRef { callee, args } => {
            let callee_str = format_hir_expr_ts(callee);
            let args_str: Vec<String> = args.iter().map(format_hir_expr_ts).collect();
            format!("{}({})", callee_str, args_str.join(", "))
        }
        HirExpr::Perform {
            effect,
            operation,
            args,
            ..
        } => {
            // @effects perform Effect.operation(args) → function call
            let args_str: Vec<String> = args.iter().map(format_hir_expr_ts).collect();
            format!(
                "/* perform {}.{} */ {}{}({})",
                effect,
                operation,
                effect.to_lowercase(),
                operation,
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
                            "{}: {}",
                            p.name,
                            crate::transpiler::typescript::map_type_ts(Some(&tr.name))
                        )
                    } else {
                        p.name.clone()
                    }
                })
                .collect();
            let ret = return_type
                .as_ref()
                .map(|r| {
                    format!(
                        ": {}",
                        crate::transpiler::typescript::map_type_ts(Some(r.as_str()))
                    )
                })
                .unwrap_or_default();
            let body_str = format_hir_stmt_ts(body);
            let body_with_return = if needs_return_prefix_ts(&body_str) {
                format!("return {};", body_str)
            } else {
                body_str
            };
            format!(
                "(({}){} => {{ {} }})",
                params_str.join(", "),
                ret,
                body_with_return
            )
        }
    }
}

fn format_hir_stmt_ts(stmt: &HirStmt) -> String {
    match stmt {
        HirStmt::Let { var, value, .. } => {
            format!("let {} = {};", var, format_hir_expr_ts(value))
        }
        HirStmt::Assign { var, value } => {
            format!("{} = {};", var, format_hir_expr_ts(value))
        }
        HirStmt::While {
            cond,
            invariant,
            decreases: _,
            body,
        } => {
            format!(
                "// invariant: {}\n    while ({}) {{\n        {}\n    }}",
                format_hir_expr_ts(invariant),
                format_hir_expr_ts(cond),
                format_hir_stmt_ts(body)
            )
        }
        HirStmt::Block { stmts, tail_expr } => {
            let mut lines = Vec::new();
            for s in stmts {
                let code = format_hir_stmt_ts(s);
                // 文として出力
                if code.ends_with(';') || code.ends_with('}') || code.starts_with("//") {
                    lines.push(code);
                } else {
                    lines.push(format!("{};", code));
                }
            }
            if let Some(tail) = tail_expr {
                let tail_code = format_hir_expr_ts(tail);
                // TS では if/switch/while は文であり return の右辺にできない。
                // 旧コードの heuristic guard を再現する。
                if tail_code.starts_with("if")
                    || tail_code.starts_with("switch")
                    || tail_code.starts_with("while")
                {
                    lines.push(tail_code);
                } else {
                    lines.push(format!("return {};", tail_code));
                }
            }
            lines.join("\n    ")
        }
        HirStmt::Acquire { resource, body } => {
            // acquire を即時実行 async 関数で包むことで、外側の関数が async でなくても動作する。
            let body_str = format_hir_stmt_ts(body);
            format!("(async () => {{ await {r}.acquire(); try {{ return {body}; }} finally {{ {r}.release(); }} }})()", r = resource, body = body_str)
        }
        HirStmt::Expr(expr) => format_hir_expr_ts(expr),
    }
}

/// トップレベル body の出力に `return` プレフィックスが必要かを判定する。
/// 文（let, if, while, switch, コメント, return 済み）には不要。
fn needs_return_prefix_ts(code: &str) -> bool {
    !(code.starts_with("return ")
        || code.starts_with("if ")
        || code.starts_with("if(")
        || code.starts_with("while ")
        || code.starts_with("while(")
        || code.starts_with("switch ")
        || code.starts_with("let ")
        || code.starts_with("//")
        || code.contains(" = "))
}
