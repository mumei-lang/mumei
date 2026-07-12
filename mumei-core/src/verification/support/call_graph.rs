#![allow(unused_imports)]
use super::super::module_env::*;
use super::super::translator::*;
use super::super::types::*;
use super::super::*;
use crate::hir::HirAtom;
use crate::parser::*;
use std::collections::{HashMap, HashSet};
use z3::ast::{Bool, Dynamic, Float, Int, Real, String as Z3String};
use z3::{Config, Context, SatResult, Solver};

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

/// atom レベルの invariant を帰納的に検証する。
pub(crate) fn verify_atom_invariant(
    atom: &Atom,
    body_stmt: &Stmt,
    invariant_raw: &str,
    module_env: &ModuleEnv,
    ieee754_f64: bool,
) -> MumeiResult<()> {
    let mut cfg = Config::new();
    cfg.set_timeout_msec(5000);
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);

    let vc = VCtx {
        ctx: &ctx,
        module_env,
        current_atom: Some(atom),
        linearity_ctx: None,
        effect_ctx: None,
        constraint_count: None,
        constraint_budget: DEFAULT_CONSTRAINT_BUDGET,
        has_string_constraints: None,
        path_cond_stack: std::cell::RefCell::new(Vec::new()),
        profiler: None,
        ieee754_f64,
    };

    let mut env: Env = HashMap::new();

    // パラメータをシンボリック変数として登録
    for param in &atom.params {
        let var = param_z3_value(
            &ctx,
            param.name.as_str(),
            param.type_name.as_deref(),
            module_env,
            ieee754_f64,
        );
        env.insert(param.name.clone(), var);

        // 精緻型制約も適用
        if let Some(type_name) = &param.type_name {
            if let Some(refined) = module_env.get_type(type_name) {
                apply_refinement_constraint(&vc, &solver, &param.name, refined, &mut env)?;
            }
        }
    }

    // invariant 式をパース
    let inv_ast = parse_expression(invariant_raw);
    let inv_z3 =
        expr_to_z3(&vc, &inv_ast, &mut env, None)?
            .as_bool()
            .ok_or(MumeiError::type_error_at(
                format!(
                    "Invariant for atom '{}' must be a boolean expression",
                    atom.name
                ),
                atom.span.clone(),
            ))?;

    // === Step 1: 導入 (Induction Base) ===
    // requires → invariant を証明する
    if atom.requires.trim() != "true" {
        let req_ast = parse_expression(&atom.requires);
        let req_z3 = expr_to_z3(&vc, &req_ast, &mut env, None)?;
        if let Some(req_bool) = req_z3.as_bool() {
            solver.push();
            // requires を仮定
            solver.assert(&req_bool);
            // invariant の否定を assert
            solver.assert(&inv_z3.not());
            // Unsat なら requires → invariant が証明された
            if solver.check() == SatResult::Sat {
                solver.pop(1);
                return Err(MumeiError::verification_at(
                    format!(
                        "Invariant induction base failed for atom '{}': \
                         requires does not imply invariant.\n  \
                         Invariant: {}\n  \
                         Requires: {}\n  \
                         The invariant must hold whenever the precondition is satisfied.",
                        atom.name, invariant_raw, atom.requires
                    ),
                    atom.span.clone(),
                ));
            }
            solver.pop(1);
        }
    } else {
        // requires が true の場合、invariant は無条件に成立する必要がある
        solver.push();
        solver.assert(&inv_z3.not());
        if solver.check() == SatResult::Sat {
            solver.pop(1);
            return Err(MumeiError::verification_at(
                format!(
                    "Invariant induction base failed for atom '{}': \
                     invariant '{}' is not universally true (no requires constraint).",
                    atom.name, invariant_raw
                ),
                atom.span.clone(),
            ));
        }
        solver.pop(1);
    }

    // === Step 2: 維持 (Preservation) ===
    // invariant ∧ requires のもとで body を実行した後も invariant が維持されることを証明
    {
        let env_snapshot = env.clone();
        solver.push();

        // invariant を仮定（帰納法の仮定）
        solver.assert(&inv_z3);

        // requires も仮定
        if atom.requires.trim() != "true" {
            let req_ast = parse_expression(&atom.requires);
            let req_z3 = expr_to_z3(&vc, &req_ast, &mut env, None)?;
            if let Some(req_bool) = req_z3.as_bool() {
                solver.assert(&req_bool);
            }
        }

        // body を実行
        let _body_result = stmt_to_z3(&vc, body_stmt, &mut env, Some(&solver))?;

        // body 実行後の invariant を再評価
        // （env が body の実行で更新されている可能性がある）
        let inv_after = expr_to_z3(&vc, &inv_ast, &mut env, None)?
            .as_bool()
            .ok_or(MumeiError::type_error("Invariant must be boolean"))?;

        // invariant の維持を検証: ¬inv_after が Unsat なら維持されている
        solver.assert(&inv_after.not());
        if solver.check() == SatResult::Sat {
            solver.pop(1);
            return Err(MumeiError::verification_at(
                format!(
                    "Invariant preservation failed for atom '{}': \
                     body execution may violate the invariant.\n  \
                     Invariant: {}\n  \
                     The invariant must be maintained after executing the body.",
                    atom.name, invariant_raw
                ),
                atom.span.clone(),
            ));
        }
        solver.pop(1);
        let _ = env_snapshot; // env_snapshot はスコープ終了で破棄
    }

    Ok(())
}

// =============================================================================
// Call Graph サイクル検知 (Call Graph Cycle Detection)
// =============================================================================
//
// 間接再帰（A→B→A）を含む呼び出しグラフのサイクルを検出する。
// 直接再帰は verify_async_recursion_depth で検出済みだが、
// 間接再帰はグラフ全体を走査する必要がある。
//
// アルゴリズム: DFS による強連結成分（SCC）の簡易検出。
// サイクルが検出された場合、invariant の記述を要求するか、
// BMC の深度制限を適用する。

/// Expr を簡易的にソース文字列に復元する（requires 置換用）。
pub(crate) fn expr_to_source_string(expr: &Expr) -> String {
    match expr {
        Expr::Number(n) => n.to_string(),
        Expr::Float(f) => format!("{}", f),
        Expr::StringLit(s) => format!("\"{}\"", s),
        Expr::Variable(v) => v.clone(),
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
                Op::Implies => "==>",
                Op::Pow => "**",
            };
            format!(
                "({} {} {})",
                expr_to_source_string(l),
                op_str,
                expr_to_source_string(r)
            )
        }
        Expr::Call(name, args) => {
            let args_str: Vec<String> = args.iter().map(expr_to_source_string).collect();
            format!("{}({})", name, args_str.join(", "))
        }
        Expr::FieldAccess(e, field) => format!("{}.{}", expr_to_source_string(e), field),
        Expr::ArrayAccess(name, idx) => format!("{}[{}]", name, expr_to_source_string(idx)),
        _ => format!("{:?}", expr),
    }
}

/// body 内の全 Call 式から呼び出し先の atom 名と引数を収集する。
pub(crate) fn collect_callees_with_args_expr(expr: &Expr) -> Vec<(String, Vec<Expr>)> {
    let mut callees = Vec::new();
    match expr {
        Expr::Call(name, args) => {
            callees.push((name.clone(), args.clone()));
            for arg in args {
                callees.extend(collect_callees_with_args_expr(arg));
            }
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            callees.extend(collect_callees_with_args_expr(cond));
            callees.extend(collect_callees_with_args_stmt(then_branch));
            callees.extend(collect_callees_with_args_stmt(else_branch));
        }
        Expr::BinaryOp(l, _, r) => {
            callees.extend(collect_callees_with_args_expr(l));
            callees.extend(collect_callees_with_args_expr(r));
        }
        Expr::Async { body } => {
            callees.extend(collect_callees_with_args_stmt(body));
        }
        Expr::Await { expr } => {
            callees.extend(collect_callees_with_args_expr(expr));
        }
        Expr::Match { target, arms } => {
            callees.extend(collect_callees_with_args_expr(target));
            for arm in arms {
                callees.extend(collect_callees_with_args_stmt(&arm.body));
                if let Some(guard) = &arm.guard {
                    callees.extend(collect_callees_with_args_expr(guard));
                }
            }
        }
        Expr::CallRef { callee, args } => {
            if let Expr::AtomRef { name } = callee.as_ref() {
                callees.push((name.clone(), args.clone()));
            }
            callees.extend(collect_callees_with_args_expr(callee));
            for arg in args {
                callees.extend(collect_callees_with_args_expr(arg));
            }
        }
        Expr::Perform { args, .. } => {
            for arg in args {
                callees.extend(collect_callees_with_args_expr(arg));
            }
        }
        Expr::Lambda { body, .. } => {
            callees.extend(collect_callees_with_args_stmt(body));
        }
        Expr::ChanSend { channel, value } => {
            callees.extend(collect_callees_with_args_expr(channel));
            callees.extend(collect_callees_with_args_expr(value));
        }
        Expr::ChanRecv { channel } => {
            callees.extend(collect_callees_with_args_expr(channel));
        }
        _ => {}
    }
    callees
}

pub(crate) fn collect_callees_with_args_stmt(stmt: &Stmt) -> Vec<(String, Vec<Expr>)> {
    let mut callees = Vec::new();
    match stmt {
        Stmt::Block(stmts, _) => {
            for s in stmts {
                callees.extend(collect_callees_with_args_stmt(s));
            }
        }
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            callees.extend(collect_callees_with_args_expr(value));
        }
        Stmt::ArrayStore { index, value, .. } => {
            callees.extend(collect_callees_with_args_expr(index));
            callees.extend(collect_callees_with_args_expr(value));
        }
        Stmt::While { cond, body, .. } => {
            callees.extend(collect_callees_with_args_expr(cond));
            callees.extend(collect_callees_with_args_stmt(body));
        }
        Stmt::Acquire { body, .. } => {
            callees.extend(collect_callees_with_args_stmt(body));
        }
        Stmt::Task { body, .. } => {
            callees.extend(collect_callees_with_args_stmt(body));
        }
        Stmt::TaskGroup { children, .. } => {
            for child in children {
                callees.extend(collect_callees_with_args_stmt(child));
            }
        }
        Stmt::Expr(e, _) => {
            callees.extend(collect_callees_with_args_expr(e));
        }
        Stmt::Cancel { .. } => {}
    }
    callees
}

/// Collect `<name>[<idx_expr>]` sub-expressions (returns `(array_name, idx_expr)`
/// pairs) recursively from an expression tree. Used by the forall-constraint
/// handler to build explicit Z3 pattern hints and to derive per-array
/// `len_<name> > idx` bounds so that downstream `ArrayAccess` OOB checks can
/// be discharged for indices that the user's forall already certifies as
/// valid.
///
/// The array name is preserved because `expr_to_z3`'s `Expr::ArrayAccess`
/// branch looks up the companion length constant as `len_<name>`. If we
/// hard-coded a single identifier here, multi-array forall conditions (e.g.
/// referencing both `arr` and `aux`) would bind their bounds to the wrong
/// length variable.
pub(crate) fn collect_array_accesses(expr: &Expr) -> Vec<(String, Expr)> {
    let mut out = Vec::new();
    collect_array_accesses_inner(expr, &mut out);
    out
}

pub(crate) fn collect_array_accesses_inner(expr: &Expr, out: &mut Vec<(String, Expr)>) {
    match expr {
        Expr::ArrayAccess(name, idx) => {
            out.push((name.clone(), (**idx).clone()));
            collect_array_accesses_inner(idx, out);
        }
        Expr::BinaryOp(l, _, r) => {
            collect_array_accesses_inner(l, out);
            collect_array_accesses_inner(r, out);
        }
        Expr::Call(_, args) => {
            for a in args {
                collect_array_accesses_inner(a, out);
            }
        }
        Expr::FieldAccess(e, _) => collect_array_accesses_inner(e, out),
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_array_accesses_inner(cond, out);
            // then/else branches are `Box<Stmt>`; forall conditions virtually
            // never contain side-effectful branches, but when they do we still
            // want to scan any tail expression for `arr[...]` so the pattern
            // and bound synthesis stay consistent with `collect_callees_with_args_expr`.
            collect_array_accesses_in_stmt(then_branch, out);
            collect_array_accesses_in_stmt(else_branch, out);
        }
        _ => {}
    }
}

pub(crate) fn collect_array_accesses_in_stmt(stmt: &Stmt, out: &mut Vec<(String, Expr)>) {
    match stmt {
        Stmt::Expr(e, _) => collect_array_accesses_inner(e, out),
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            collect_array_accesses_inner(value, out);
        }
        Stmt::ArrayStore { index, value, .. } => {
            collect_array_accesses_inner(index, out);
            collect_array_accesses_inner(value, out);
        }
        Stmt::Block(stmts, _) => {
            for s in stmts {
                collect_array_accesses_in_stmt(s, out);
            }
        }
        Stmt::While {
            cond,
            invariant,
            body,
            ..
        } => {
            collect_array_accesses_inner(cond, out);
            collect_array_accesses_inner(invariant, out);
            collect_array_accesses_in_stmt(body, out);
        }
        _ => {}
    }
}

/// body 内の全 Call 式から呼び出し先の atom 名を収集する。
pub(crate) fn collect_callees_expr(expr: &Expr) -> Vec<String> {
    let mut callees = Vec::new();
    match expr {
        Expr::Call(name, args) => {
            callees.push(name.clone());
            for arg in args {
                callees.extend(collect_callees_expr(arg));
            }
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            callees.extend(collect_callees_expr(cond));
            callees.extend(collect_callees_stmt(then_branch));
            callees.extend(collect_callees_stmt(else_branch));
        }
        Expr::BinaryOp(l, _, r) => {
            callees.extend(collect_callees_expr(l));
            callees.extend(collect_callees_expr(r));
        }
        Expr::Async { body } => {
            callees.extend(collect_callees_stmt(body));
        }
        Expr::Await { expr } => {
            callees.extend(collect_callees_expr(expr));
        }
        Expr::Match { target, arms } => {
            callees.extend(collect_callees_expr(target));
            for arm in arms {
                callees.extend(collect_callees_stmt(&arm.body));
                if let Some(guard) = &arm.guard {
                    callees.extend(collect_callees_expr(guard));
                }
            }
        }
        Expr::AtomRef { name } => {
            callees.push(name.clone());
        }
        Expr::CallRef { callee, args } => {
            callees.extend(collect_callees_expr(callee));
            for arg in args {
                callees.extend(collect_callees_expr(arg));
            }
        }
        Expr::Perform { args, .. } => {
            for arg in args {
                callees.extend(collect_callees_expr(arg));
            }
        }
        Expr::Lambda { body, .. } => {
            callees.extend(collect_callees_stmt(body));
        }
        // Plan 8: Channel operations — traverse sub-expressions for callees
        Expr::ChanSend { channel, value } => {
            callees.extend(collect_callees_expr(channel));
            callees.extend(collect_callees_expr(value));
        }
        Expr::ChanRecv { channel } => {
            callees.extend(collect_callees_expr(channel));
        }
        _ => {}
    }
    callees
}

pub(crate) fn collect_callees_stmt(stmt: &Stmt) -> Vec<String> {
    let mut callees = Vec::new();
    match stmt {
        Stmt::Block(stmts, _) => {
            for s in stmts {
                callees.extend(collect_callees_stmt(s));
            }
        }
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            callees.extend(collect_callees_expr(value));
        }
        Stmt::ArrayStore { index, value, .. } => {
            callees.extend(collect_callees_expr(index));
            callees.extend(collect_callees_expr(value));
        }
        Stmt::While { cond, body, .. } => {
            callees.extend(collect_callees_expr(cond));
            callees.extend(collect_callees_stmt(body));
        }
        Stmt::Acquire { body, .. } => {
            callees.extend(collect_callees_stmt(body));
        }
        Stmt::Task { body, .. } => {
            callees.extend(collect_callees_stmt(body));
        }
        Stmt::TaskGroup { children, .. } => {
            for child in children {
                callees.extend(collect_callees_stmt(child));
            }
        }
        Stmt::Expr(e, _) => {
            callees.extend(collect_callees_expr(e));
        }
        // Plan 8: Cancel statement has no callees
        Stmt::Cancel { .. } => {}
    }
    callees
}

/// Call Graph のサイクルを DFS で検出する。
/// atom_name から到達可能なサイクルがある場合、サイクルのパスを返す。
pub(crate) fn detect_call_cycle(atom_name: &str, module_env: &ModuleEnv) -> Option<Vec<String>> {
    let mut visited: HashSet<String> = HashSet::new();
    let mut path: Vec<String> = Vec::new();

    fn dfs(
        current: &str,
        target: &str,
        module_env: &ModuleEnv,
        visited: &mut HashSet<String>,
        path: &mut Vec<String>,
    ) -> bool {
        if current == target && !path.is_empty() {
            return true; // サイクル検出
        }
        if visited.contains(current) {
            return false;
        }
        visited.insert(current.to_string());
        path.push(current.to_string());

        if let Some(callee_atom) = module_env.get_atom(current) {
            let body_stmt = parse_body_expr(&callee_atom.body_expr);
            let callees = collect_callees_stmt(&body_stmt);
            for callee_name in &callees {
                if module_env.get_atom(callee_name).is_some() {
                    if callee_name == target && !path.is_empty() {
                        path.push(callee_name.clone());
                        return true;
                    }
                    if dfs(callee_name, target, module_env, visited, path) {
                        return true;
                    }
                }
            }
        }

        path.pop();
        false
    }

    // atom_name の呼び出し先から DFS 開始
    if let Some(atom) = module_env.get_atom(atom_name) {
        let body_stmt = parse_body_expr(&atom.body_expr);
        let callees = collect_callees_stmt(&body_stmt);
        for callee_name in &callees {
            if module_env.get_atom(callee_name).is_some() {
                visited.clear();
                path.clear();
                path.push(atom_name.to_string());
                if dfs(callee_name, atom_name, module_env, &mut visited, &mut path) {
                    return Some(path);
                }
            }
        }
    }
    None
}

/// Call Graph サイクル検知を実行し、サイクルが見つかった場合は
/// invariant の記述を要求するか、BMC 深度制限を適用する。
pub(crate) fn verify_call_graph_cycles(atom: &Atom, module_env: &ModuleEnv) -> MumeiResult<()> {
    if let Some(cycle_path) = detect_call_cycle(&atom.name, module_env) {
        let cycle_str = cycle_path.join(" → ");

        // invariant が指定されていれば帰納的検証で対応可能
        if atom.invariant.is_some() {
            // invariant が指定されている → 帰納的検証で安全性を保証
            // （verify_atom_invariant で検証済み）
            return Ok(());
        }

        // max_unroll が指定されていれば BMC で対応
        if atom.max_unroll.is_some() {
            // BMC 深度制限が明示されている → 有界検証で対応
            return Ok(());
        }

        // どちらもない場合は警告（エラーではなく警告にとどめる）
        eprintln!(
            "  ⚠️  Call graph cycle detected for atom '{}': {}\n     \
             Consider adding `invariant: <expr>;` for complete proof, or \
             `max_unroll: N;` for bounded verification.",
            atom.name, cycle_str
        );
    }
    Ok(())
}

// =============================================================================
// Taint Analysis (汚染解析)
// =============================================================================
//
// unverified な外部関数から戻ってきた値を「汚染済み（tainted）」としてマークし、
// tainted 値が安全性の証明に使われた場合に警告を出す。
//
// 仕組み:
// - expr_to_z3 の Call 処理で、呼び出し先が unverified の場合、
//   戻り値に __tainted_{call_id} マーカーを付与する。
// - ensures の検証時、env 内に __tainted_* が存在する場合、
//   「検証結果が未検証コードに依存している」旨の警告を出す。

/// unverified 関数の呼び出しを検出し、taint マーカーを env に追加する。
/// verify() の body 検証後に呼び出される。
pub(crate) fn check_taint_propagation(
    atom: &Atom,
    body_stmt: &Stmt,
    _env: &Env,
    module_env: &ModuleEnv,
) {
    // body 内で呼び出されている関数を収集
    let callees = collect_callees_stmt(body_stmt);

    let mut tainted_sources: Vec<String> = Vec::new();
    for callee_name in &callees {
        if let Some(callee) = module_env.get_atom(callee_name) {
            if callee.trust_level == TrustLevel::Unverified {
                tainted_sources.push(callee_name.clone());
            }
        }
    }

    if !tainted_sources.is_empty() {
        eprintln!(
            "  ⚠  Taint warning for atom '{}': verification depends on unverified function(s): [{}]. \
             Results may be unsound.",
            atom.name, tainted_sources.join(", ")
        );
    }
}
