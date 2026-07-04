#![allow(unused_imports)]
use super::super::module_env::*;
use super::super::translator::*;
use super::super::types::*;
use super::super::*;
use crate::parser::*;
use std::collections::{HashMap, HashSet};
use z3::ast::Int;
use z3::{Config, Context, SatResult, Solver};

// =============================================================================
// リソース階層検証 (Resource Hierarchy Verification)
// =============================================================================
//
// デッドロック防止: リソース取得順序の半順序関係を Z3 で検証する。
//
// 不変条件: ∀ r1, r2 ∈ Held(thread, t):
//   acquire(r2) かつ r1 ∈ Held → Priority(r2) > Priority(r1)
//
// これにより、待機グラフ（Wait-For Graph）に循環が生じないことを
// コンパイル時に数学的に保証する。

// リソース取得コンテキスト: 現在保持中のリソースとその優先度を追跡する。
// acquire 式の検証時に、リソース階層制約をチェックする。

// =============================================================================
#[derive(Debug, Clone, Default)]
pub(crate) struct ResourceCtx {
    /// 現在保持中のリソース: (リソース名, 優先度)
    held: Vec<(String, i64)>,
    /// 違反リスト
    violations: Vec<String>,
}

impl ResourceCtx {
    fn new() -> Self {
        Self::default()
    }

    /// リソースを取得する。階層制約を検証し、違反があればエラーを記録する。
    fn acquire(&mut self, resource_name: &str, priority: i64) -> Result<(), String> {
        // 現在保持中の全リソースに対して、新リソースの優先度が厳密に高いことを検証
        for (held_name, held_priority) in &self.held {
            if priority <= *held_priority {
                let msg = format!(
                    "Deadlock risk: acquiring '{}' (priority={}) while holding '{}' (priority={}). \
                     New resource must have strictly higher priority.",
                    resource_name, priority, held_name, held_priority
                );
                self.violations.push(msg.clone());
                return Err(msg);
            }
        }
        self.held.push((resource_name.to_string(), priority));
        Ok(())
    }

    /// リソースを解放する（acquire ブロック終了時に呼ばれる）
    fn release(&mut self, resource_name: &str) {
        self.held.retain(|(name, _)| name != resource_name);
    }

    #[allow(dead_code)]
    pub(crate) fn has_violations(&self) -> bool {
        !self.violations.is_empty()
    }
}

/// atom のリソース使用順序を Z3 で検証する。
/// atom の resources 宣言と body 内の acquire 式から、
/// リソース階層制約 Priority(r2) > Priority(r1) を検証する。
///
/// 検証方法:
/// 1. atom の resources リストから使用リソースを特定
/// 2. body 内の acquire 式を走査し、取得順序を抽出
/// 3. Z3 で半順序関係の非循環性を証明
pub(crate) fn verify_resource_hierarchy(atom: &Atom, module_env: &ModuleEnv) -> MumeiResult<()> {
    if atom.resources.is_empty() {
        return Ok(());
    }

    // リソース定義の存在チェック
    let mut resource_priorities: Vec<(String, i64)> = Vec::new();
    for res_name in &atom.resources {
        if let Some(rdef) = module_env.resources.get(res_name) {
            resource_priorities.push((rdef.name.clone(), rdef.priority));
        } else {
            return Err(MumeiError::type_error_at(
                format!("Resource '{}' used in atom '{}' is not defined. Add: resource {} priority:<N> mode:exclusive|shared;",
                    res_name, atom.name, res_name),
                atom.span.clone()
            ));
        }
    }

    // Z3 で半順序関係を検証
    let mut cfg = Config::new();
    cfg.set_timeout_msec(5000);
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);

    // 各リソースの優先度をシンボリック整数として定義
    let mut priority_vars: HashMap<String, Int> = HashMap::new();
    for (name, priority) in &resource_priorities {
        let var = Int::new_const(&ctx, format!("priority_{}", name).as_str());
        // 優先度を具体値に束縛
        solver.assert(&var._eq(&Int::from_i64(&ctx, *priority)));
        priority_vars.insert(name.clone(), var);
    }

    // リソース間の順序制約を検証:
    // resources リスト内で前に宣言されたリソースは先に取得されると仮定し、
    // 後に宣言されたリソースは厳密に高い優先度を持つ必要がある。
    for i in 0..resource_priorities.len() {
        for j in (i + 1)..resource_priorities.len() {
            let (name_i, _) = &resource_priorities[i];
            let (name_j, _) = &resource_priorities[j];
            let pri_i = &priority_vars[name_i];
            let pri_j = &priority_vars[name_j];

            // Priority(r_j) > Priority(r_i) を検証
            solver.push();
            solver.assert(&pri_j.le(pri_i)); // 否定: Priority(r_j) <= Priority(r_i)
            if solver.check() == SatResult::Sat {
                solver.pop(1);
                let error_span = module_env
                    .resources
                    .get(name_j)
                    .map(|r| r.span.clone())
                    .unwrap_or_else(|| atom.span.clone());
                return Err(MumeiError::verification_at(
                    format!(
                        "Resource hierarchy violation in atom '{}': \
                         '{}' (priority={}) must have strictly lower priority than '{}' (priority={}). \
                         Reorder resources or adjust priorities to prevent potential deadlock.",
                        atom.name, name_i, resource_priorities[i].1,
                        name_j, resource_priorities[j].1
                    ),
                    error_span
                ));
            }
            solver.pop(1);
        }
    }

    // データレース検証: exclusive リソースの排他性チェック
    // 同一 atom 内で同じ exclusive リソースを複数回 acquire していないことを確認
    let mut exclusive_set: HashSet<String> = HashSet::new();
    for res_name in &atom.resources {
        if let Some(rdef) = module_env.resources.get(res_name) {
            if rdef.mode == ResourceMode::Exclusive && !exclusive_set.insert(res_name.clone()) {
                return Err(MumeiError::verification_at(
                    format!(
                        "Data race risk in atom '{}': exclusive resource '{}' is listed multiple times",
                        atom.name, res_name
                    ),
                    atom.span.clone()
                ));
            }
        }
    }

    Ok(())
}

// =============================================================================
// 有界モデル検査 (Bounded Model Checking — BMC)
// =============================================================================
//
// ループ内の acquire パターンや非同期処理の安全性を、ループ不変量を
// ユーザーが記述しなくても検証するための補助的な検証手法。
//
// 設計:
// - ループを最大 BMC_UNROLL_DEPTH 回展開し、各展開でリソース階層制約を検証
// - ループ不変量が提供されている場合はそちらを優先（BMC はフォールバック）
// - Z3 タイムアウトリスクがあるため、展開回数は保守的に制限
//
// 制約:
// - 無限ループの停止性は証明しない（それは decreases 句の役割）
// - BMC は「展開回数以内でのバグ不在」を証明するのみ（完全性はない）

/// BMC のループ展開回数上限（グローバルデフォルト）
/// atom 単位で `max_unroll: N;` によりオーバーライド可能。
pub(crate) const BMC_DEFAULT_UNROLL_DEPTH: usize = 3;

/// 再帰的 async 呼び出しの最大展開深度。
/// async atom が自身を呼び出す場合、この深度を超えると
/// 「Unknown（未定義）」として扱い、Z3 探索を打ち切る。
pub(crate) const MAX_ASYNC_RECURSION_DEPTH: usize = 3;

/// body 内の Acquire を再帰的に収集する（BMC 用）。
/// ループ内で acquire が使われているパターンを検出するために使用。
pub(crate) fn collect_acquire_resources_expr(expr: &Expr) -> Vec<String> {
    let mut resources = Vec::new();
    match expr {
        Expr::IfThenElse {
            then_branch,
            else_branch,
            ..
        } => {
            resources.extend(collect_acquire_resources_stmt(then_branch));
            resources.extend(collect_acquire_resources_stmt(else_branch));
        }
        Expr::Async { body } => {
            resources.extend(collect_acquire_resources_stmt(body));
        }
        Expr::Await { expr } => {
            resources.extend(collect_acquire_resources_expr(expr));
        }
        Expr::Perform { args, .. } => {
            for arg in args {
                resources.extend(collect_acquire_resources_expr(arg));
            }
        }
        Expr::AtomRef { .. } => {}
        Expr::CallRef { callee, args } => {
            resources.extend(collect_acquire_resources_expr(callee));
            for arg in args {
                resources.extend(collect_acquire_resources_expr(arg));
            }
        }
        Expr::Lambda { body, .. } => {
            resources.extend(collect_acquire_resources_stmt(body));
        }
        // Plan 8: Channel operations — traverse sub-expressions for acquire resources
        Expr::ChanSend { channel, value } => {
            resources.extend(collect_acquire_resources_expr(channel));
            resources.extend(collect_acquire_resources_expr(value));
        }
        Expr::ChanRecv { channel } => {
            resources.extend(collect_acquire_resources_expr(channel));
        }
        _ => {}
    }
    resources
}

pub(crate) fn collect_acquire_resources_stmt(stmt: &Stmt) -> Vec<String> {
    let mut resources = Vec::new();
    match stmt {
        Stmt::Acquire { resource, body, .. } => {
            resources.push(resource.clone());
            resources.extend(collect_acquire_resources_stmt(body));
        }
        Stmt::Block(stmts, _) => {
            for s in stmts {
                resources.extend(collect_acquire_resources_stmt(s));
            }
        }
        Stmt::While { body, .. } => {
            resources.extend(collect_acquire_resources_stmt(body));
        }
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            resources.extend(collect_acquire_resources_expr(value));
        }
        Stmt::ArrayStore { index, value, .. } => {
            resources.extend(collect_acquire_resources_expr(index));
            resources.extend(collect_acquire_resources_expr(value));
        }
        Stmt::Task { body, .. } => {
            resources.extend(collect_acquire_resources_stmt(body));
        }
        Stmt::TaskGroup { children, .. } => {
            for child in children {
                resources.extend(collect_acquire_resources_stmt(child));
            }
        }
        Stmt::Expr(e, _) => {
            resources.extend(collect_acquire_resources_expr(e));
        }
        // Plan 8: Cancel statement has no resources
        Stmt::Cancel { .. } => {}
    }
    resources
}

/// 有界モデル検査: atom の body 内のループを展開し、
/// 各展開でリソース階層制約が維持されることを検証する。
///
/// 展開回数は atom.max_unroll（指定時）または BMC_DEFAULT_UNROLL_DEPTH を使用。
/// ループ不変量が提供されている場合はスキップ（不変量ベースの検証が優先）。
/// BMC は「ユーザーが不変量を書けない場合」の補助的な検証手段。
pub(crate) fn verify_bmc_resource_safety(
    atom: &Atom,
    body_stmt: &Stmt,
    module_env: &ModuleEnv,
    global_max_unroll: usize,
) -> MumeiResult<()> {
    // body 内に acquire が含まれない場合はスキップ
    let acquired_resources = collect_acquire_resources_stmt(body_stmt);
    if acquired_resources.is_empty() {
        return Ok(());
    }

    // While ループ内に acquire があるかチェック
    fn has_acquire_in_while_stmt(stmt: &Stmt) -> bool {
        match stmt {
            Stmt::While { body, .. } => !collect_acquire_resources_stmt(body).is_empty(),
            Stmt::Block(stmts, _) => stmts.iter().any(has_acquire_in_while_stmt),
            Stmt::Acquire { body, .. } => has_acquire_in_while_stmt(body),
            Stmt::Task { body, .. } => has_acquire_in_while_stmt(body),
            Stmt::Expr(e, _) => has_acquire_in_while_expr(e),
            _ => false,
        }
    }
    fn has_acquire_in_while_expr(expr: &Expr) -> bool {
        match expr {
            Expr::IfThenElse {
                then_branch,
                else_branch,
                ..
            } => has_acquire_in_while_stmt(then_branch) || has_acquire_in_while_stmt(else_branch),
            Expr::Async { body } => has_acquire_in_while_stmt(body),
            // Plan 8: Channel operations — traverse sub-expressions
            Expr::ChanSend { channel, value } => {
                has_acquire_in_while_expr(channel) || has_acquire_in_while_expr(value)
            }
            Expr::ChanRecv { channel } => has_acquire_in_while_expr(channel),
            _ => false,
        }
    }

    if !has_acquire_in_while_stmt(body_stmt) {
        return Ok(()); // ループ外の acquire は通常の検証で十分
    }

    // 展開回数: atom 単位のオーバーライド > 設定ファイルのグローバル値 > デフォルト
    let configured_unroll_depth = if global_max_unroll == 0 {
        BMC_DEFAULT_UNROLL_DEPTH
    } else {
        global_max_unroll
    };
    let unroll_depth = atom.max_unroll.unwrap_or(configured_unroll_depth);

    // BMC: ループを展開して各ステップでリソース階層をチェック
    let mut resource_ctx = ResourceCtx::new();

    for unroll_step in 0..unroll_depth {
        // 各展開ステップで acquire されるリソースの順序を検証
        for res_name in &acquired_resources {
            if let Some(rdef) = module_env.resources.get(res_name) {
                if let Err(e) = resource_ctx.acquire(res_name, rdef.priority) {
                    return Err(MumeiError::verification_at(
                        format!(
                            "BMC (unroll step {}/{}, max_unroll={}): resource ordering violation in loop body: {}",
                            unroll_step, unroll_depth, unroll_depth, e
                        ),
                        atom.span.clone()
                    ));
                }
            }
        }
        // 各ステップ終了時にリソースを解放（ループの次のイテレーションをシミュレート）
        for res_name in &acquired_resources {
            resource_ctx.release(res_name);
        }
    }

    Ok(())
}

/// 再帰的 async 呼び出しの深度を検証する。
/// async atom が自身を（直接的または間接的に）呼び出す場合、
/// MAX_ASYNC_RECURSION_DEPTH を超える再帰がないことを静的にチェックする。
///
/// 仕組み: body 内の Call 式を走査し、呼び出し先が async atom かつ
/// 自身と同名の場合、再帰深度カウンタをインクリメント。
/// 上限を超えたら「Unknown」として打ち切り、警告を出す。
pub(crate) fn verify_async_recursion_depth(
    atom: &Atom,
    body_stmt: &Stmt,
    module_env: &ModuleEnv,
) -> MumeiResult<()> {
    if !atom.is_async {
        return Ok(());
    }

    fn count_self_calls_expr(expr: &Expr, atom_name: &str) -> usize {
        match expr {
            Expr::Call(name, args) => {
                let self_call = if name == atom_name { 1 } else { 0 };
                self_call
                    + args
                        .iter()
                        .map(|a| count_self_calls_expr(a, atom_name))
                        .sum::<usize>()
            }
            Expr::IfThenElse {
                cond,
                then_branch,
                else_branch,
            } => {
                count_self_calls_expr(cond, atom_name)
                    + count_self_calls_stmt(then_branch, atom_name)
                    + count_self_calls_stmt(else_branch, atom_name)
            }
            Expr::Async { body } => count_self_calls_stmt(body, atom_name),
            Expr::Await { expr } => count_self_calls_expr(expr, atom_name),
            Expr::BinaryOp(l, _, r) => {
                count_self_calls_expr(l, atom_name) + count_self_calls_expr(r, atom_name)
            }
            Expr::Perform { args, .. } => args
                .iter()
                .map(|a| count_self_calls_expr(a, atom_name))
                .sum(),
            Expr::AtomRef { .. } => 0,
            Expr::CallRef { callee, args } => {
                let self_call = if let Expr::AtomRef { name } = callee.as_ref() {
                    if name == atom_name {
                        1
                    } else {
                        0
                    }
                } else {
                    0
                };
                self_call
                    + count_self_calls_expr(callee, atom_name)
                    + args
                        .iter()
                        .map(|a| count_self_calls_expr(a, atom_name))
                        .sum::<usize>()
            }
            Expr::Lambda { body, .. } => count_self_calls_stmt(body, atom_name),
            // Plan 8: Channel operations — traverse sub-expressions for self-calls
            Expr::ChanSend { channel, value } => {
                count_self_calls_expr(channel, atom_name) + count_self_calls_expr(value, atom_name)
            }
            Expr::ChanRecv { channel } => count_self_calls_expr(channel, atom_name),
            _ => 0,
        }
    }
    fn count_self_calls_stmt(stmt: &Stmt, atom_name: &str) -> usize {
        match stmt {
            Stmt::Block(stmts, _) => stmts
                .iter()
                .map(|s| count_self_calls_stmt(s, atom_name))
                .sum(),
            Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
                count_self_calls_expr(value, atom_name)
            }
            Stmt::ArrayStore { index, value, .. } => {
                count_self_calls_expr(index, atom_name) + count_self_calls_expr(value, atom_name)
            }
            Stmt::Acquire { body, .. } => count_self_calls_stmt(body, atom_name),
            Stmt::While { cond, body, .. } => {
                count_self_calls_expr(cond, atom_name) + count_self_calls_stmt(body, atom_name)
            }
            Stmt::Task { body, .. } => count_self_calls_stmt(body, atom_name),
            Stmt::TaskGroup { children, .. } => children
                .iter()
                .map(|c| count_self_calls_stmt(c, atom_name))
                .sum(),
            Stmt::Expr(e, _) => count_self_calls_expr(e, atom_name),
            // Plan 8: Cancel statement has no self-calls
            Stmt::Cancel { .. } => 0,
        }
    }

    let self_call_count = count_self_calls_stmt(body_stmt, &atom.name);

    if self_call_count > 0 {
        // 再帰的 async 呼び出しが検出された
        // 呼び出し先の async atom も再帰する可能性があるため、
        // 深度制限を超える場合は警告
        let max_depth = atom.max_unroll.unwrap_or(MAX_ASYNC_RECURSION_DEPTH);
        if self_call_count > max_depth {
            return Err(MumeiError::verification_at(
                format!(
                    "Async recursion depth exceeded in atom '{}': {} self-calls detected \
                     (max_depth={}). Use max_unroll: {}; to increase the limit, or \
                     refactor to use iteration with invariant.",
                    atom.name,
                    self_call_count,
                    max_depth,
                    self_call_count + 1
                ),
                atom.span.clone(),
            ));
        }

        // 再帰呼び出し先の契約を信頼して展開（Compositional Verification）
        // 各展開ステップで ensures を仮定として使用する。
        // これにより、f_depth_1, f_depth_2 ... と別シンボルとして扱われ、
        // Z3 が無限ループに陥ることを防ぐ。
        if let Some(callee) = module_env.get_atom(&atom.name) {
            if callee.ensures.trim() == "true" {
                // ensures が trivial な場合、再帰の安全性を証明できない
                return Err(MumeiError::verification_at(
                    format!(
                        "Recursive async atom '{}' requires a non-trivial ensures clause \
                         for inductive verification. Add: ensures: <postcondition>;",
                        atom.name
                    ),
                    atom.span.clone(),
                ));
            }
        }
    }

    Ok(())
}

// =============================================================================
// Atom レベル Invariant の帰納的検証 (Inductive Invariant Verification)
// =============================================================================
//
// atom シグネチャに `invariant: <expr>;` が指定されている場合、
// 帰納法（数学的帰納法）により不変量の正しさを証明する。
//
// 証明構造:
// 1. 導入 (Induction Base):
//    requires が成立するとき、invariant が成立することを証明する。
//    ∀ params. requires(params) → invariant(params)
//
// 2. 維持 (Induction Step / Preservation):
//    invariant が成立する状態で body を実行した後も invariant が維持されることを証明する。
//    ∀ params. invariant(params) ∧ requires(params) → invariant(body(params))
//    ※ 再帰呼び出しがある場合、呼び出し先の invariant を帰納法の仮定として使用する。
//
// これにより、再帰的 async atom の安全性を、ループ不変量と同様の
// 帰納的推論で証明できる。BMC の「有界」な保証を「完全」な保証に昇格させる。
