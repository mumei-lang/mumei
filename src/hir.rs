// =============================================================================
// HIR (High-level Intermediate Representation)
// =============================================================================
// AST から lowering（変換）され、検証・コード生成に使用される中間表現。
// body_expr が String として保持されている現状を解消し、各所で毎回
// parse_expression() で再パースしている非効率を排除する。
//
// 将来的に型情報やエフェクト情報を付与するための拡張ポイントとなる。
// 初期実装では AST の Expr/Stmt からほぼ 1:1 で変換する。
// =============================================================================

// =============================================================================
// MIR (Mid-level IR) — CFG ベースの中間表現
// =============================================================================
// Phase 4b: MIR data structures and basic HIR → MIR lowering are now defined
// in src/mir.rs. The lowering covers:
//   - Let / Assign → MirStatement::Assign + StorageLive
//   - BinaryOp → flattened temporaries + Rvalue::BinaryOp
//   - IfThenElse → 3+ BasicBlocks with SwitchInt terminator
//   - While → loop header / body / after blocks with back-edge
//   - Call → Rvalue::Call
// Future phases will add:
//   - 借用の生存期間（Lifetime）解析
//   - Z3 への制約生成の最適化
//   - Drop の自動挿入
// 参照: docs/ROADMAP.md の "Multi-Stage IR Roadmap" セクション
// =============================================================================

// =============================================================================
// Effect System — エフェクト型の基盤: ✅ Complete
// =============================================================================
// HirEffectSet attached to HirAtom, HirExpr::Call, HirExpr::Perform.
// lower_atom_to_hir_with_env() populates effect info from ModuleEnv.
// 対応状況:
//   1. パーサーの regex 脱却 → ✅ 式パーサーは再帰下降に移行済み (PR #62)
//   2. 基本エフェクト定義 → ✅ effect FileWrite(path: Str); 構文サポート済み
//   3. atom にエフェクトアノテーション → ✅ effects: [...] + perform 構文サポート済み
//   4. エフェクト多相 → ✅ <E: Effect> + with E 構文 + 単相化ベースの解決
//   5. HIR エフェクト情報付与 → ✅ HirEffectSet, callee_effects, effect_usage
// TypeRef に effect_set フィールドを追加済み (PR #65)。
// =============================================================================

// =============================================================================
// Capability Security — Evaluation Complete
// =============================================================================
// See docs/CAPABILITY_SECURITY.md for the full evaluation.
// Current status: Option A (parameterized effects + Z3) is sufficient for
// current use cases. SecurityPolicy is wired into ModuleEnv for runtime
// enforcement. Object-based capability model documented as future alternative.
// =============================================================================

use std::collections::{BTreeSet, HashSet};

use crate::parser::{
    parse_body_expr, parse_expression, Atom, Expr, JoinSemantics, Op, Pattern, Stmt,
};

/// Effect set attached to HIR nodes.
#[derive(Debug, Clone, Default)]
pub struct HirEffectSet {
    /// Effect names that this node may produce (BTreeSet for deterministic iteration order)
    pub effects: BTreeSet<String>,
    /// Parameterized effect usages (consumed by future LSP/diagnostic passes)
    // NOTE: parameterized is populated during lowering and consumed by future LSP/diagnostic passes.
    #[allow(dead_code)]
    pub parameterized: Vec<HirEffectUsage>,
}

/// Structured effect usage information for parameterized effects.
/// Fields are populated during lowering and available for future LSP/diagnostic consumers.
#[derive(Debug, Clone)]
// NOTE: HirEffectUsage is populated during lowering. Not yet referenced in current pipeline.
#[allow(dead_code)]
pub struct HirEffectUsage {
    pub effect_name: String,
    pub operation: String,
    pub param_values: Vec<String>,
}

/// HIR 式: 純粋な式を表す
#[derive(Debug, Clone)]
pub enum HirExpr {
    Number(i64),
    Float(f64),
    /// Plan 9: First-class string literal
    StringLit(String),
    Variable(String),
    ArrayAccess(String, Box<HirExpr>),
    BinaryOp(Box<HirExpr>, Op, Box<HirExpr>),
    IfThenElse {
        cond: Box<HirExpr>,
        then_branch: Box<HirStmt>,
        else_branch: Box<HirStmt>,
    },
    Call {
        name: String,
        args: Vec<HirExpr>,
        /// Effects the callee may produce (populated during lowering when ModuleEnv is available)
        /// Consumed by future LSP hover and diagnostic passes.
        // NOTE: callee_effects is populated during lowering when ModuleEnv is available. Consumed by future LSP hover passes.
        #[allow(dead_code)]
        callee_effects: Option<HirEffectSet>,
    },
    StructInit {
        type_name: String,
        fields: Vec<(String, HirExpr)>,
    },
    FieldAccess(Box<HirExpr>, String),
    Match {
        target: Box<HirExpr>,
        arms: Vec<HirMatchArm>,
    },
    AtomRef {
        name: String,
    },
    CallRef {
        callee: Box<HirExpr>,
        args: Vec<HirExpr>,
    },
    Async {
        body: Box<HirStmt>,
    },
    Await {
        expr: Box<HirExpr>,
    },
    /// perform Effect.operation(args)
    Perform {
        effect: String,
        operation: String,
        args: Vec<HirExpr>,
        /// Structured effect usage info (consumed by future LSP/diagnostic passes)
        // NOTE: effect_usage is populated during lowering. Consumed by future LSP/diagnostic passes.
        #[allow(dead_code)]
        effect_usage: Option<HirEffectUsage>,
    },
    /// task { body }
    // NOTE: Task is constructed via lower_stmt(Stmt::Task) → HirStmt::Expr(HirExpr::Task),
    // but codegen matches on HirExpr::Task directly. The variant is reachable at runtime;
    // dead_code warning is a false positive from the compiler not tracing through HirStmt::Expr.
    #[allow(dead_code)]
    Task {
        body: Box<HirStmt>,
        group: Option<String>,
    },
    /// task_group { task { ... }; task { ... } }
    // NOTE: Same as Task — constructed via HirStmt::Expr(HirExpr::TaskGroup) in lower_stmt.
    #[allow(dead_code)]
    TaskGroup {
        children: Vec<HirStmt>,
        join_semantics: JoinSemantics,
    },
    /// Lambda 式（クロージャ変換前）
    Lambda {
        params: Vec<HirLambdaParam>,
        return_type: Option<String>,
        body: Box<HirStmt>,
        /// キャプチャされた外部変数（lower_expr 時に解析）
        captures: Vec<String>,
    },
    // Plan 8: Channel send expression — `send(ch, value)`
    // NOTE: ChanSend is constructed via HIR lowering from Expr::ChanSend
    #[allow(dead_code)]
    ChanSend {
        channel: Box<HirExpr>,
        value: Box<HirExpr>,
    },
    // Plan 8: Channel receive expression — `recv(ch)`
    // NOTE: ChanRecv is constructed via HIR lowering from Expr::ChanRecv
    #[allow(dead_code)]
    ChanRecv {
        channel: Box<HirExpr>,
    },
    /// Plan 14: Enum variant construction expression — `Some(42)`, `Ok(value)`
    /// Constructed during HIR lowering when a Call name resolves to a known enum variant.
    VariantInit {
        enum_name: String,
        variant_name: String,
        fields: Vec<HirExpr>,
    },
}

/// HIR 文: 副作用を持つ構文要素
#[derive(Debug, Clone)]
pub enum HirStmt {
    /// 変数束縛: let var = expr
    /// TODO: ty will be populated by type inference in future phases
    // NOTE: Let variant is always constructed by lower_stmt. The dead_code warning is
    // triggered by the `ty` field which is always None in Phase 1 (type inference not yet implemented).
    #[allow(dead_code)]
    Let {
        var: String,
        ty: Option<String>,
        value: Box<HirExpr>,
    },
    /// 代入: var = expr
    Assign { var: String, value: Box<HirExpr> },
    /// While ループ
    While {
        cond: Box<HirExpr>,
        invariant: Box<HirExpr>,
        decreases: Option<Box<HirExpr>>,
        body: Box<HirStmt>,
    },
    /// ブロック: { stmts; tail_expr }
    Block {
        stmts: Vec<HirStmt>,
        tail_expr: Option<Box<HirExpr>>,
    },
    /// リソース取得: acquire resource { body }
    Acquire {
        resource: String,
        body: Box<HirStmt>,
    },
    /// 式文
    Expr(HirExpr),
}

#[derive(Debug, Clone)]
pub struct HirLambdaParam {
    pub name: String,
    pub type_ref: Option<crate::ast::TypeRef>,
}

/// Match 式のアーム
#[derive(Debug, Clone)]
pub struct HirMatchArm {
    pub pattern: Pattern,
    pub guard: Option<Box<HirExpr>>,
    pub body: Box<HirStmt>,
}

/// Atom の HIR 版: body が String ではなく構造化された HirStmt として保持される
#[derive(Debug, Clone)]
pub struct HirAtom {
    pub body: HirStmt,
    // NOTE: requires_hir/ensures_hir are not yet consumed by verification (which still re-parses
    // from atom.requires/ensures strings). They will be used once verification migrates to
    // HIR-based Z3 constraint generation (Phase 2).
    #[allow(dead_code)]
    pub requires_hir: HirExpr,
    #[allow(dead_code)]
    pub ensures_hir: HirExpr,
    /// 元の Atom（メタデータアクセス用）
    pub atom: Atom,
    /// パース済みの body ステートメント（verification.rs での再パースを避ける）
    pub body_stmt: Stmt,
    /// Effect set declared on this atom (resolved from atom.effects)
    pub effect_set: HirEffectSet,
}

// =============================================================================
// Lowering 関数: AST → HIR
// =============================================================================

/// AST の Atom を HirAtom に変換する（body_expr の String パースを含む）
/// parse_expression の呼び出しを 1 回に集約する。
pub fn lower_atom_to_hir(atom: &Atom) -> HirAtom {
    lower_atom_to_hir_with_env(atom, None)
}

/// AST の Atom を HirAtom に変換する。ModuleEnv があればエフェクト情報も付与する。
pub fn lower_atom_to_hir_with_env(
    atom: &Atom,
    module_env: Option<&crate::verification::ModuleEnv>,
) -> HirAtom {
    let body_stmt = parse_body_expr(&atom.body_expr);
    let body = lower_stmt_with_env(&body_stmt, module_env);

    let requires_expr = parse_expression(&atom.requires);
    let requires_hir = lower_expr_with_env(&requires_expr, module_env);

    let ensures_expr = parse_expression(&atom.ensures);
    let ensures_hir = lower_expr_with_env(&ensures_expr, module_env);

    // Build effect set from atom.effects
    let effect_set = HirEffectSet {
        effects: atom.effects.iter().map(|e| e.name.clone()).collect(),
        parameterized: atom
            .effects
            .iter()
            .filter(|e| !e.params.is_empty())
            .map(|e| HirEffectUsage {
                effect_name: e.name.clone(),
                operation: String::new(),
                param_values: e.params.iter().map(|p| p.value.clone()).collect(),
            })
            .collect(),
    };

    HirAtom {
        body,
        requires_hir,
        ensures_hir,
        atom: atom.clone(),
        body_stmt,
        effect_set,
    }
}

/// AST の Expr を HirExpr に変換する
pub fn lower_expr(expr: &Expr) -> HirExpr {
    lower_expr_with_env(expr, None)
}

/// AST の Expr を HirExpr に変換する。ModuleEnv があれば callee_effects / effect_usage を付与。
pub fn lower_expr_with_env(
    expr: &Expr,
    module_env: Option<&crate::verification::ModuleEnv>,
) -> HirExpr {
    let result = match expr {
        Expr::Number(n) => HirExpr::Number(*n),
        Expr::Float(f) => HirExpr::Float(*f),
        // Plan 9: String literal lowering
        Expr::StringLit(s) => HirExpr::StringLit(s.clone()),
        Expr::Variable(s) => HirExpr::Variable(s.clone()),
        Expr::ArrayAccess(name, idx) => {
            HirExpr::ArrayAccess(name.clone(), Box::new(lower_expr_with_env(idx, module_env)))
        }
        Expr::BinaryOp(l, op, r) => HirExpr::BinaryOp(
            Box::new(lower_expr_with_env(l, module_env)),
            op.clone(),
            Box::new(lower_expr_with_env(r, module_env)),
        ),
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => HirExpr::IfThenElse {
            cond: Box::new(lower_expr_with_env(cond, module_env)),
            then_branch: Box::new(lower_stmt_with_env(then_branch, module_env)),
            else_branch: Box::new(lower_stmt_with_env(else_branch, module_env)),
        },
        Expr::Call(name, args) => {
            let callee_effects = module_env.and_then(|env| {
                env.get_atom(name).map(|callee_atom| HirEffectSet {
                    effects: callee_atom.effects.iter().map(|e| e.name.clone()).collect(),
                    parameterized: callee_atom
                        .effects
                        .iter()
                        .filter(|e| !e.params.is_empty())
                        .map(|e| HirEffectUsage {
                            effect_name: e.name.clone(),
                            operation: String::new(),
                            param_values: e.params.iter().map(|p| p.value.clone()).collect(),
                        })
                        .collect(),
                })
            });
            HirExpr::Call {
                name: name.clone(),
                args: args
                    .iter()
                    .map(|a| lower_expr_with_env(a, module_env))
                    .collect(),
                callee_effects,
            }
        }
        Expr::StructInit { type_name, fields } => HirExpr::StructInit {
            type_name: type_name.clone(),
            fields: fields
                .iter()
                .map(|(name, expr)| (name.clone(), lower_expr_with_env(expr, module_env)))
                .collect(),
        },
        Expr::FieldAccess(expr, field) => HirExpr::FieldAccess(
            Box::new(lower_expr_with_env(expr, module_env)),
            field.clone(),
        ),
        Expr::Match { target, arms } => HirExpr::Match {
            target: Box::new(lower_expr_with_env(target, module_env)),
            arms: arms
                .iter()
                .map(|arm| HirMatchArm {
                    pattern: arm.pattern.clone(),
                    guard: arm
                        .guard
                        .as_ref()
                        .map(|g| Box::new(lower_expr_with_env(g, module_env))),
                    body: Box::new(lower_stmt_with_env(&arm.body, module_env)),
                })
                .collect(),
        },
        Expr::AtomRef { name } => HirExpr::AtomRef { name: name.clone() },
        Expr::CallRef { callee, args } => HirExpr::CallRef {
            callee: Box::new(lower_expr_with_env(callee, module_env)),
            args: args
                .iter()
                .map(|a| lower_expr_with_env(a, module_env))
                .collect(),
        },
        Expr::Async { body } => HirExpr::Async {
            body: Box::new(lower_stmt_with_env(body, module_env)),
        },
        Expr::Await { expr } => HirExpr::Await {
            expr: Box::new(lower_expr_with_env(expr, module_env)),
        },
        Expr::Perform {
            effect,
            operation,
            args,
        } => {
            let effect_usage = Some(HirEffectUsage {
                effect_name: effect.clone(),
                operation: operation.clone(),
                param_values: Vec::new(),
            });
            HirExpr::Perform {
                effect: effect.clone(),
                operation: operation.clone(),
                args: args
                    .iter()
                    .map(|a| lower_expr_with_env(a, module_env))
                    .collect(),
                effect_usage,
            }
        }
        Expr::Lambda {
            params,
            return_type,
            body,
        } => {
            let hir_body = lower_stmt_with_env(body, module_env);
            // Capture analysis: find free variables in body that are not in params
            let param_names: HashSet<String> = params.iter().map(|p| p.name.clone()).collect();
            let free_vars = collect_free_variables_stmt(&hir_body);
            let captures: Vec<String> = free_vars
                .into_iter()
                .filter(|v| !param_names.contains(v))
                .collect();

            HirExpr::Lambda {
                params: params
                    .iter()
                    .map(|p| HirLambdaParam {
                        name: p.name.clone(),
                        type_ref: p.type_ref.clone(),
                    })
                    .collect(),
                return_type: return_type.clone(),
                body: Box::new(hir_body),
                captures,
            }
        }
        // Plan 8: Channel send/recv lowering
        Expr::ChanSend { channel, value } => HirExpr::ChanSend {
            channel: Box::new(lower_expr_with_env(channel, module_env)),
            value: Box::new(lower_expr_with_env(value, module_env)),
        },
        Expr::ChanRecv { channel } => HirExpr::ChanRecv {
            channel: Box::new(lower_expr_with_env(channel, module_env)),
        },
    };

    // Plan 14: Check if Call expressions should be converted to VariantInit.
    // If the call name matches a known enum variant AND is NOT a known atom/function,
    // convert to VariantInit. This prevents namespace collisions where a function
    // named e.g. "Some" or "Ok" would be incorrectly treated as a variant constructor.
    if let HirExpr::Call {
        ref name, ref args, ..
    } = result
    {
        if let Some(env) = module_env {
            // Only convert if the name is NOT a known atom (functions take priority)
            if env.get_atom(name).is_none() {
                if let Some(enum_def) = env.find_enum_by_variant(name) {
                    return HirExpr::VariantInit {
                        enum_name: enum_def.name.clone(),
                        variant_name: name.clone(),
                        fields: args.clone(),
                    };
                }
            }
        }
    }

    result
}

/// AST の Stmt を HirStmt に変換する
pub fn lower_stmt(stmt: &Stmt) -> HirStmt {
    lower_stmt_with_env(stmt, None)
}

/// AST の Stmt を HirStmt に変換する。ModuleEnv があればエフェクト情報を付与。
pub fn lower_stmt_with_env(
    stmt: &Stmt,
    module_env: Option<&crate::verification::ModuleEnv>,
) -> HirStmt {
    match stmt {
        Stmt::Let { var, value, .. } => HirStmt::Let {
            var: var.clone(),
            ty: None, // TODO: type inference
            value: Box::new(lower_expr_with_env(value, module_env)),
        },
        Stmt::Assign { var, value, .. } => HirStmt::Assign {
            var: var.clone(),
            value: Box::new(lower_expr_with_env(value, module_env)),
        },
        Stmt::Block(stmts, _) => {
            if stmts.is_empty() {
                HirStmt::Block {
                    stmts: vec![],
                    tail_expr: None,
                }
            } else {
                let mut lowered: Vec<HirStmt> = stmts
                    .iter()
                    .map(|s| lower_stmt_with_env(s, module_env))
                    .collect();
                let last = lowered.pop().unwrap();
                if let HirStmt::Expr(expr) = last {
                    HirStmt::Block {
                        stmts: lowered,
                        tail_expr: Some(Box::new(expr)),
                    }
                } else {
                    lowered.push(last);
                    HirStmt::Block {
                        stmts: lowered,
                        tail_expr: None,
                    }
                }
            }
        }
        Stmt::While {
            cond,
            invariant,
            decreases,
            body,
            ..
        } => HirStmt::While {
            cond: Box::new(lower_expr_with_env(cond, module_env)),
            invariant: Box::new(lower_expr_with_env(invariant, module_env)),
            decreases: decreases
                .as_ref()
                .map(|d| Box::new(lower_expr_with_env(d, module_env))),
            body: Box::new(lower_stmt_with_env(body, module_env)),
        },
        Stmt::Acquire { resource, body, .. } => HirStmt::Acquire {
            resource: resource.clone(),
            body: Box::new(lower_stmt_with_env(body, module_env)),
        },
        Stmt::Task { body, group, .. } => HirStmt::Expr(HirExpr::Task {
            body: Box::new(lower_stmt_with_env(body, module_env)),
            group: group.clone(),
        }),
        Stmt::TaskGroup {
            children,
            join_semantics,
            ..
        } => HirStmt::Expr(HirExpr::TaskGroup {
            children: children
                .iter()
                .map(|s| lower_stmt_with_env(s, module_env))
                .collect(),
            join_semantics: join_semantics.clone(),
        }),
        // Plan 8: Cancel statement lowering
        Stmt::Cancel { target, .. } => HirStmt::Expr(HirExpr::Call {
            name: format!("__cancel_{}", target),
            args: vec![],
            callee_effects: None,
        }),
        Stmt::Expr(expr, _) => HirStmt::Expr(lower_expr_with_env(expr, module_env)),
    }
}

/// Collect variable names bound by a pattern (recursive for nested Variant patterns).
fn collect_pattern_bindings(pattern: &crate::parser::Pattern, bound: &mut HashSet<String>) {
    match pattern {
        crate::parser::Pattern::Variable(name) => {
            bound.insert(name.clone());
        }
        crate::parser::Pattern::Variant { fields, .. } => {
            for field_pattern in fields {
                collect_pattern_bindings(field_pattern, bound);
            }
        }
        crate::parser::Pattern::Wildcard | crate::parser::Pattern::Literal(_) => {}
    }
}

/// Collect free variables from a HirStmt (recursive traversal).
/// Returns all variable names referenced, excluding those bound by let statements.
fn collect_free_variables_stmt(stmt: &HirStmt) -> HashSet<String> {
    let mut vars = HashSet::new();
    match stmt {
        HirStmt::Let { value, .. } => {
            vars.extend(collect_free_variables_expr(value));
            // Note: the bound variable is tracked by the Block handler's `bound` set.
            // We do NOT remove `var` here because it may appear free in the value
            // expression (e.g., `let x = x + 1` where `x` on the right refers to
            // an outer binding that must be captured).
        }
        HirStmt::Assign { var, value } => {
            vars.insert(var.clone());
            vars.extend(collect_free_variables_expr(value));
        }
        HirStmt::While {
            cond,
            invariant,
            decreases,
            body,
        } => {
            vars.extend(collect_free_variables_expr(cond));
            vars.extend(collect_free_variables_expr(invariant));
            if let Some(dec) = decreases {
                vars.extend(collect_free_variables_expr(dec));
            }
            vars.extend(collect_free_variables_stmt(body));
        }
        HirStmt::Block { stmts, tail_expr } => {
            let mut bound = HashSet::new();
            for s in stmts {
                let s_vars = collect_free_variables_stmt(s);
                for v in s_vars {
                    if !bound.contains(&v) {
                        vars.insert(v);
                    }
                }
                if let HirStmt::Let { var, .. } = s {
                    bound.insert(var.clone());
                }
            }
            if let Some(tail) = tail_expr {
                let t_vars = collect_free_variables_expr(tail);
                for v in t_vars {
                    if !bound.contains(&v) {
                        vars.insert(v);
                    }
                }
            }
        }
        HirStmt::Acquire { body, .. } => {
            vars.extend(collect_free_variables_stmt(body));
        }
        HirStmt::Expr(expr) => {
            vars.extend(collect_free_variables_expr(expr));
        }
    }
    vars
}

/// Collect free variables from a HirExpr (recursive traversal).
fn collect_free_variables_expr(expr: &HirExpr) -> HashSet<String> {
    let mut vars = HashSet::new();
    match expr {
        HirExpr::Variable(name) => {
            // Exclude boolean literals
            if name != "true" && name != "false" {
                vars.insert(name.clone());
            }
        }
        HirExpr::Number(_) | HirExpr::Float(_) | HirExpr::StringLit(_) => {}
        HirExpr::ArrayAccess(name, idx) => {
            vars.insert(name.clone());
            vars.extend(collect_free_variables_expr(idx));
        }
        HirExpr::BinaryOp(l, _, r) => {
            vars.extend(collect_free_variables_expr(l));
            vars.extend(collect_free_variables_expr(r));
        }
        HirExpr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            vars.extend(collect_free_variables_expr(cond));
            vars.extend(collect_free_variables_stmt(then_branch));
            vars.extend(collect_free_variables_stmt(else_branch));
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                vars.extend(collect_free_variables_expr(arg));
            }
        }
        HirExpr::StructInit { fields, .. } => {
            for (_, expr) in fields {
                vars.extend(collect_free_variables_expr(expr));
            }
        }
        HirExpr::FieldAccess(expr, _) => {
            vars.extend(collect_free_variables_expr(expr));
        }
        HirExpr::Match { target, arms } => {
            vars.extend(collect_free_variables_expr(target));
            for arm in arms {
                // Collect variables bound by the pattern to exclude from free vars
                let mut pattern_bound = HashSet::new();
                collect_pattern_bindings(&arm.pattern, &mut pattern_bound);
                let arm_body_vars = collect_free_variables_stmt(&arm.body);
                for v in arm_body_vars {
                    if !pattern_bound.contains(&v) {
                        vars.insert(v);
                    }
                }
                if let Some(guard) = &arm.guard {
                    let guard_vars = collect_free_variables_expr(guard);
                    for v in guard_vars {
                        if !pattern_bound.contains(&v) {
                            vars.insert(v);
                        }
                    }
                }
            }
        }
        HirExpr::AtomRef { .. } => {}
        HirExpr::CallRef { callee, args } => {
            vars.extend(collect_free_variables_expr(callee));
            for arg in args {
                vars.extend(collect_free_variables_expr(arg));
            }
        }
        HirExpr::Async { body } => {
            vars.extend(collect_free_variables_stmt(body));
        }
        HirExpr::Await { expr } => {
            vars.extend(collect_free_variables_expr(expr));
        }
        HirExpr::Perform { args, .. } => {
            for arg in args {
                vars.extend(collect_free_variables_expr(arg));
            }
        }
        HirExpr::Task { body, .. } => {
            vars.extend(collect_free_variables_stmt(body));
        }
        HirExpr::TaskGroup { children, .. } => {
            for child in children {
                vars.extend(collect_free_variables_stmt(child));
            }
        }
        HirExpr::Lambda {
            params,
            body,
            captures,
            ..
        } => {
            // Lambda's own captures are free in the enclosing scope
            vars.extend(captures.iter().cloned());
            // Also collect from body, excluding lambda's own params
            let param_names: HashSet<String> = params.iter().map(|p| p.name.clone()).collect();
            let body_vars = collect_free_variables_stmt(body);
            for v in body_vars {
                if !param_names.contains(&v) {
                    vars.insert(v);
                }
            }
        }
        // Plan 8: Channel send/recv free variables
        HirExpr::ChanSend { channel, value } => {
            vars.extend(collect_free_variables_expr(channel));
            vars.extend(collect_free_variables_expr(value));
        }
        HirExpr::ChanRecv { channel } => {
            vars.extend(collect_free_variables_expr(channel));
        }
        // Plan 14: VariantInit free variables
        HirExpr::VariantInit { fields, .. } => {
            for field in fields {
                vars.extend(collect_free_variables_expr(field));
            }
        }
    }
    vars
}

/// 文字列から直接 HirExpr を生成するヘルパー（法則検証等のアドホックなパース用）
// NOTE: Utility for future phases (e.g., REPL HIR evaluation, test helpers). Not called in current pipeline.
#[allow(dead_code)]
pub fn lower_expr_from_str(input: &str) -> HirExpr {
    let expr = parse_expression(input);
    lower_expr(&expr)
}

/// 文字列から直接 HirStmt を生成するヘルパー（ボディ式のアドホックなパース用）
// NOTE: Utility for future phases (e.g., REPL HIR evaluation, test helpers). Not called in current pipeline.
#[allow(dead_code)]
pub fn lower_stmt_from_str(input: &str) -> HirStmt {
    let stmt = parse_body_expr(input);
    lower_stmt(&stmt)
}
