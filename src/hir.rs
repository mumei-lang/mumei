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
// TODO: MIR (Mid-level IR) — CFG ベースの中間表現
// =============================================================================
// 前提条件: 借用検査（Borrow Checking）の設計が確定してから導入する。
// MIR は制御フローグラフ（CFG）をグラフ構造として持ち、以下の解析に使用予定:
//   - 借用の生存期間（Lifetime）解析
//   - Z3 への制約生成の最適化
//   - Drop の自動挿入
// 参照: docs/ROADMAP.md の "Multi-Stage IR Roadmap" セクション
// =============================================================================

// =============================================================================
// TODO: Effect System — エフェクト型の基盤
// =============================================================================
// 前提条件（対応順序）:
//   1. パーサーの regex 脱却（再帰下降パーサーへの移行）
//   2. 基本エフェクト定義（effect FileWrite(path: Str); のような構文）
//   3. atom にエフェクトアノテーション追加
//   4. エフェクト多相の導入
// 現状 TypeRef にはエフェクトセットが含まれていない。
// HirExpr/HirStmt にエフェクト情報を付与する設計が必要。
// =============================================================================

// =============================================================================
// TODO: Capability Security
// =============================================================================
// 前提条件: エフェクト多相（上記 Phase 3）の成熟度評価が完了してから検討する。
// パラメータ付きエフェクト（FileWrite("/tmp/...") 等）を Z3 で検証する
// 現路線で不十分な場合に、FileCap のようなオブジェクトベースの権限モデルを導入する。
// =============================================================================

use crate::parser::{
    parse_body_expr, parse_expression, Atom, Expr, JoinSemantics, Op, Pattern, Stmt,
};

/// HIR 式: 純粋な式を表す
#[derive(Debug, Clone)]
pub enum HirExpr {
    Number(i64),
    Float(f64),
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
    },
    /// task { body }
    // NOTE: Task is constructed via lower_stmt(Stmt::Task) → HirStmt::Expr(HirExpr::Task),
    // but codegen/transpiler match on HirExpr::Task directly. The variant is reachable at runtime;
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
}

// =============================================================================
// Lowering 関数: AST → HIR
// =============================================================================

/// AST の Atom を HirAtom に変換する（body_expr の String パースを含む）
/// parse_expression の呼び出しを 1 回に集約する。
pub fn lower_atom_to_hir(atom: &Atom) -> HirAtom {
    let body_stmt = parse_body_expr(&atom.body_expr);
    let body = lower_stmt(&body_stmt);

    let requires_expr = parse_expression(&atom.requires);
    let requires_hir = lower_expr(&requires_expr);

    let ensures_expr = parse_expression(&atom.ensures);
    let ensures_hir = lower_expr(&ensures_expr);

    HirAtom {
        body,
        requires_hir,
        ensures_hir,
        atom: atom.clone(),
    }
}

/// AST の Expr を HirExpr に変換する
pub fn lower_expr(expr: &Expr) -> HirExpr {
    match expr {
        Expr::Number(n) => HirExpr::Number(*n),
        Expr::Float(f) => HirExpr::Float(*f),
        Expr::Variable(s) => HirExpr::Variable(s.clone()),
        Expr::ArrayAccess(name, idx) => {
            HirExpr::ArrayAccess(name.clone(), Box::new(lower_expr(idx)))
        }
        Expr::BinaryOp(l, op, r) => {
            HirExpr::BinaryOp(Box::new(lower_expr(l)), op.clone(), Box::new(lower_expr(r)))
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => HirExpr::IfThenElse {
            cond: Box::new(lower_expr(cond)),
            then_branch: Box::new(lower_stmt(then_branch)),
            else_branch: Box::new(lower_stmt(else_branch)),
        },
        Expr::Call(name, args) => HirExpr::Call {
            name: name.clone(),
            args: args.iter().map(lower_expr).collect(),
        },
        Expr::StructInit { type_name, fields } => HirExpr::StructInit {
            type_name: type_name.clone(),
            fields: fields
                .iter()
                .map(|(name, expr)| (name.clone(), lower_expr(expr)))
                .collect(),
        },
        Expr::FieldAccess(expr, field) => {
            HirExpr::FieldAccess(Box::new(lower_expr(expr)), field.clone())
        }
        Expr::Match { target, arms } => HirExpr::Match {
            target: Box::new(lower_expr(target)),
            arms: arms
                .iter()
                .map(|arm| HirMatchArm {
                    pattern: arm.pattern.clone(),
                    guard: arm.guard.as_ref().map(|g| Box::new(lower_expr(g))),
                    body: Box::new(lower_stmt(&arm.body)),
                })
                .collect(),
        },
        Expr::AtomRef { name } => HirExpr::AtomRef { name: name.clone() },
        Expr::CallRef { callee, args } => HirExpr::CallRef {
            callee: Box::new(lower_expr(callee)),
            args: args.iter().map(lower_expr).collect(),
        },
        Expr::Async { body } => HirExpr::Async {
            body: Box::new(lower_stmt(body)),
        },
        Expr::Await { expr } => HirExpr::Await {
            expr: Box::new(lower_expr(expr)),
        },
        Expr::Perform {
            effect,
            operation,
            args,
        } => HirExpr::Perform {
            effect: effect.clone(),
            operation: operation.clone(),
            args: args.iter().map(lower_expr).collect(),
        },
    }
}

/// AST の Stmt を HirStmt に変換する
pub fn lower_stmt(stmt: &Stmt) -> HirStmt {
    match stmt {
        Stmt::Let { var, value } => HirStmt::Let {
            var: var.clone(),
            ty: None, // TODO: type inference
            value: Box::new(lower_expr(value)),
        },
        Stmt::Assign { var, value } => HirStmt::Assign {
            var: var.clone(),
            value: Box::new(lower_expr(value)),
        },
        Stmt::Block(stmts) => {
            if stmts.is_empty() {
                HirStmt::Block {
                    stmts: vec![],
                    tail_expr: None,
                }
            } else {
                // 全 stmt を先に lower してから最後の要素を判定する。
                // これにより Stmt::Task/TaskGroup（lower 後に HirStmt::Expr になる）も
                // 正しく tail_expr として抽出される。
                let mut lowered: Vec<HirStmt> = stmts.iter().map(lower_stmt).collect();
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
        } => HirStmt::While {
            cond: Box::new(lower_expr(cond)),
            invariant: Box::new(lower_expr(invariant)),
            decreases: decreases.as_ref().map(|d| Box::new(lower_expr(d))),
            body: Box::new(lower_stmt(body)),
        },
        Stmt::Acquire { resource, body } => HirStmt::Acquire {
            resource: resource.clone(),
            body: Box::new(lower_stmt(body)),
        },
        Stmt::Task { body, group } => HirStmt::Expr(HirExpr::Task {
            body: Box::new(lower_stmt(body)),
            group: group.clone(),
        }),
        Stmt::TaskGroup {
            children,
            join_semantics,
        } => HirStmt::Expr(HirExpr::TaskGroup {
            children: children.iter().map(lower_stmt).collect(),
            join_semantics: join_semantics.clone(),
        }),
        Stmt::Expr(expr) => HirStmt::Expr(lower_expr(expr)),
    }
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
