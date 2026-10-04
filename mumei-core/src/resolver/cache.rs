use crate::parser::ast::{Atom, Expr, Stmt};
use crate::verification::ModuleEnv;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::Path;

/// Bump when verifier semantics change so cached proofs are re-derived.
/// Version 3 enables checker-first MIR borrow checking.
pub const VERIFIER_POLICY_VERSION: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CacheEntry {
    /// ソースファイルの SHA-256 ハッシュ
    pub(crate) source_hash: String,
    /// 検証済み atom 名のリスト
    pub(crate) verified_atoms: Vec<String>,
    /// 型定義名のリスト
    pub(crate) type_names: Vec<String>,
    /// 構造体定義名のリスト
    pub(crate) struct_names: Vec<String>,
    /// Incremental Build: atom ごとの契約+body ハッシュ
    /// atom の requires/ensures/body_expr が変更されていなければ再検証をスキップする。
    /// キー: atom 名、値: SHA-256(name + requires + ensures + body_expr)
    #[serde(default)]
    pub(crate) atom_hashes: HashMap<String, String>,
}

/// キャッシュファイル全体
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct VerificationCache {
    /// ファイルパス → キャッシュエントリ
    pub(crate) entries: HashMap<String, CacheEntry>,
}

/// ソースコードの SHA-256 ハッシュを計算する
pub(crate) fn compute_hash(source: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Atom の契約+body+メタデータのハッシュを計算する（Incremental Build 用）
/// 以下のフィールドを結合してハッシュ化する:
/// - name, requires, ensures, covers, body_expr（基本契約）
/// - consumed_params, ref params（所有権制約）
/// - resources, async flag（並行性制約）
/// - invariant（帰納的不変量）
/// - trust_level, max_unroll（検証設定）
///
/// このハッシュが一致すれば、atom の検証結果は変わらないため再検証をスキップできる。
/// Call Graph サイクル検知・Taint Analysis の結果も暗黙的にキャッシュされる
/// （呼び出し先の atom が変更されればハッシュが変わり、呼び出し元も再検証される）。
#[allow(dead_code)]
pub fn compute_atom_hash(atom: &crate::parser::Atom) -> String {
    let mut hasher = Sha256::new();
    hasher.update(atom.name.as_bytes());
    hasher.update(b"|");
    hasher.update(atom.requires.as_bytes());
    hasher.update(b"|");
    hasher.update(atom.ensures.as_bytes());
    for cover in &atom.covers {
        hasher.update(b"|cover:");
        hasher.update(cover.clause.as_bytes());
        if let Some(label) = &cover.label {
            hasher.update(b"|label=");
            hasher.update(label.as_bytes());
        }
    }
    hasher.update(b"|");
    if !atom.clause_modes.is_empty() {
        for mode in &atom.clause_modes {
            hasher.update(b"|clause_mode:");
            hasher.update(format!("{:?}|{:?}|{}", mode.kind, mode.mode, mode.clause).as_bytes());
        }
    }
    hasher.update(atom.body_expr.as_bytes());
    // consumed_params も含める（所有権制約の変更を検出）
    for cp in &atom.consumed_params {
        hasher.update(b"|consume:");
        hasher.update(cp.as_bytes());
    }
    // ref / ref mut パラメータも含める
    for p in &atom.params {
        if p.is_ref {
            hasher.update(b"|ref:");
            hasher.update(p.name.as_bytes());
        }
        if p.is_ref_mut {
            hasher.update(b"|ref_mut:");
            hasher.update(p.name.as_bytes());
        }
        // fn_contract_requires / fn_contract_ensures も含める（契約変更を検出）
        if let Some(ref req) = p.fn_contract_requires {
            hasher.update(b"|fn_contract_req:");
            hasher.update(p.name.as_bytes());
            hasher.update(b"=");
            hasher.update(req.as_bytes());
        }
        if let Some(ref ens) = p.fn_contract_ensures {
            hasher.update(b"|fn_contract_ens:");
            hasher.update(p.name.as_bytes());
            hasher.update(b"=");
            hasher.update(ens.as_bytes());
        }
        // capability パラメータの静的型も含める。capability 宣言は atom の外にあるため、
        // 宣言側の constraint 変更をハッシュに反映しないと古い証明が再利用される。
        if let Some(capability) = p.type_ref.as_ref().and_then(|ty| ty.capability.as_ref()) {
            hasher.update(b"|capability:");
            hasher.update(p.name.as_bytes());
            hasher.update(b"=");
            hasher.update(capability.effect.as_bytes());
            if let Some(ref constraint) = capability.constraint {
                hasher.update(b" where ");
                hasher.update(constraint.as_bytes());
            }
        }
    }
    // resources も含める（リソース制約の変更を検出）
    for r in &atom.resources {
        hasher.update(b"|resource:");
        hasher.update(r.as_bytes());
    }
    // effects も含める（エフェクト制約の変更を検出）
    for e in &atom.effects {
        hasher.update(b"|effect:");
        hasher.update(e.name.as_bytes());
        for p in &e.params {
            hasher.update(b",param:");
            hasher.update(p.value.as_bytes());
        }
    }
    // async フラグも含める
    if atom.is_async {
        hasher.update(b"|async");
    }
    // invariant も含める
    if let Some(ref inv) = atom.invariant {
        hasher.update(b"|invariant:");
        hasher.update(inv.as_bytes());
    }
    // trust_level も含める（信頼レベルの変更を検出）
    let trust_str = match atom.trust_level {
        crate::parser::TrustLevel::Verified => "verified",
        crate::parser::TrustLevel::Trusted => "trusted",
        crate::parser::TrustLevel::Unverified => "unverified",
    };
    hasher.update(b"|trust:");
    hasher.update(trust_str.as_bytes());
    // max_unroll も含める（BMC 設定の変更を検出）
    if let Some(max) = atom.max_unroll {
        hasher.update(b"|max_unroll:");
        hasher.update(max.to_string().as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

/// Incremental Build 用: メインファイルのビルドキャッシュをロードする
#[allow(dead_code)]
pub fn load_build_cache(base_dir: &Path) -> HashMap<String, String> {
    let cache_path = base_dir.join(".mumei_build_cache");
    fs::read_to_string(&cache_path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

/// Incremental Build 用: メインファイルのビルドキャッシュを保存する
#[allow(dead_code)]
pub fn save_build_cache(base_dir: &Path, cache: &HashMap<String, String>) {
    let cache_path = base_dir.join(".mumei_build_cache");
    if let Ok(json) = serde_json::to_string_pretty(cache) {
        let _ = fs::write(cache_path, json);
    }
}

// =============================================================================
// Feature 2: Enhanced Verification Cache
// =============================================================================

/// Enhanced verification cache entry with dependency tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationCacheEntry {
    pub proof_hash: String,
    pub result: String, // "verified" or "failed"
    pub dependencies: Vec<String>,
    pub type_deps: Vec<String>,
    pub timestamp: String,
    #[serde(default)]
    pub skipped_clauses: usize,
    #[serde(default)]
    pub inferred_invariants: Vec<crate::verification::invariant_inference::InferredInvariant>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cover_results: Vec<serde_json::Value>,
}

/// Compute a proof hash that includes transitive dependency signatures and type predicates.
/// This extends compute_atom_hash with callee signatures and type predicate content.
pub fn compute_proof_hash(atom: &crate::parser::Atom, module_env: &ModuleEnv) -> String {
    compute_proof_hash_with_flags(atom, module_env, &[])
}

pub fn compute_proof_hash_with_flags(
    atom: &crate::parser::Atom,
    module_env: &ModuleEnv,
    flags: &[&str],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("|policy:{VERIFIER_POLICY_VERSION}").as_bytes());

    // 1. Include everything from the basic atom hash
    hasher.update(atom.name.as_bytes());
    hasher.update(b"|");
    hasher.update(atom.requires.as_bytes());
    hasher.update(b"|");
    hasher.update(atom.ensures.as_bytes());
    for cover in &atom.covers {
        hasher.update(b"|cover:");
        hasher.update(cover.clause.as_bytes());
        if let Some(label) = &cover.label {
            hasher.update(b"|label=");
            hasher.update(label.as_bytes());
        }
    }
    hasher.update(b"|");
    if !atom.clause_modes.is_empty() {
        for mode in &atom.clause_modes {
            hasher.update(b"|clause_mode:");
            hasher.update(format!("{:?}|{:?}|{}", mode.kind, mode.mode, mode.clause).as_bytes());
        }
    }
    hasher.update(atom.body_expr.as_bytes());
    for cp in &atom.consumed_params {
        hasher.update(b"|consume:");
        hasher.update(cp.as_bytes());
    }
    for p in &atom.params {
        if p.is_ref {
            hasher.update(b"|ref:");
            hasher.update(p.name.as_bytes());
        }
        if p.is_ref_mut {
            hasher.update(b"|ref_mut:");
            hasher.update(p.name.as_bytes());
        }
        // fn_contract_requires / fn_contract_ensures も含める（契約変更を検出）
        if let Some(ref req) = p.fn_contract_requires {
            hasher.update(b"|fn_contract_req:");
            hasher.update(p.name.as_bytes());
            hasher.update(b"=");
            hasher.update(req.as_bytes());
        }
        if let Some(ref ens) = p.fn_contract_ensures {
            hasher.update(b"|fn_contract_ens:");
            hasher.update(p.name.as_bytes());
            hasher.update(b"=");
            hasher.update(ens.as_bytes());
        }
        // capability パラメータの静的型も含める。capability 宣言は atom の外にあるため、
        // 宣言側の constraint 変更をハッシュに反映しないと古い証明が再利用される。
        if let Some(capability) = p.type_ref.as_ref().and_then(|ty| ty.capability.as_ref()) {
            hasher.update(b"|capability:");
            hasher.update(p.name.as_bytes());
            hasher.update(b"=");
            hasher.update(capability.effect.as_bytes());
            if let Some(ref constraint) = capability.constraint {
                hasher.update(b" where ");
                hasher.update(constraint.as_bytes());
            }
        }
    }
    for r in &atom.resources {
        hasher.update(b"|resource:");
        hasher.update(r.as_bytes());
        if let Some(resource) = module_env.get_resource(r) {
            hasher.update(b"|priority:");
            hasher.update(resource.priority.to_string().as_bytes());
            hasher.update(b"|mode:");
            hasher.update(format!("{:?}", resource.mode).as_bytes());
            for field in &resource.state {
                hasher.update(b"|state:");
                hasher.update(field.name.as_bytes());
                hasher.update(b":");
                hasher.update(field.ty.as_bytes());
            }
            if let Some(invariant) = &resource.invariant {
                hasher.update(b"|resource_invariant:");
                hasher.update(
                    crate::verification::support::expr_to_source_string(invariant).as_bytes(),
                );
            }
        }
    }
    for e in &atom.effects {
        hasher.update(b"|effect:");
        hasher.update(e.name.as_bytes());
        for p in &e.params {
            hasher.update(b",param:");
            hasher.update(p.value.as_bytes());
        }
    }
    // Replayability inputs: which non-deterministic roots the declared effects
    // resolve to (through the effect hierarchy) and which parameters act as
    // their witnesses. Atoms without such effects contribute nothing, so their
    // hashes are unchanged.
    for root in crate::verification::declared_nondeterministic_effects(atom, module_env) {
        hasher.update(b"|replay:");
        hasher.update(root.as_bytes());
        for witness in crate::verification::witness_params(atom, root) {
            hasher.update(b",witness:");
            hasher.update(witness.as_bytes());
        }
    }
    if atom.is_async {
        hasher.update(b"|async");
    }
    if let Some(ref inv) = atom.invariant {
        hasher.update(b"|invariant:");
        hasher.update(inv.as_bytes());
    }
    let trust_str = match atom.trust_level {
        crate::parser::TrustLevel::Verified => "verified",
        crate::parser::TrustLevel::Trusted => "trusted",
        crate::parser::TrustLevel::Unverified => "unverified",
    };
    hasher.update(b"|trust:");
    hasher.update(trust_str.as_bytes());
    if let Some(max) = atom.max_unroll {
        hasher.update(b"|max_unroll:");
        hasher.update(max.to_string().as_bytes());
    }
    if let Some(sem) = atom.spec_metadata.get("semantics") {
        hasher.update(b"|semantics:");
        hasher.update(sem.as_bytes());
    }
    for flag in flags {
        hasher.update(b"|verify_flag:");
        hasher.update(flag.as_bytes());
    }

    // 2. Include type predicate content for each param's refined type
    for p in &atom.params {
        if let Some(ref type_ref) = p.type_ref {
            if let Some(refined) = module_env.get_type(&type_ref.name) {
                hasher.update(b"|type_pred:");
                hasher.update(type_ref.name.as_bytes());
                hasher.update(b"=");
                hasher.update(refined.predicate_raw.as_bytes());
            }
        }
    }

    // 2b. Unit tag and base type of every alias in the module. The
    // unit-consistency check reads types through params, `-> T`, struct fields,
    // callee signatures and alias chains, so any unit or alias-base edit must
    // invalidate cached results.
    // Unitless aliases are left out so hashes of unit-free modules are unchanged.
    let mut unit_tags: Vec<(&String, &String, &String)> = module_env
        .types
        .iter()
        .filter_map(|(name, refined)| {
            module_env
                .unit_of_type(name)
                .map(|u| (name, &refined._base_type, u))
        })
        .collect();
    unit_tags.sort();
    for (name, base, unit) in unit_tags {
        hasher.update(b"|type_unit:");
        hasher.update(name.as_bytes());
        hasher.update(b":");
        hasher.update(base.as_bytes());
        hasher.update(b"=");
        hasher.update(unit.as_bytes());
    }

    // 2c. Nominal signature data read by the nominal struct check: the atom's own
    // parameter / return types, every struct's field types, and (below) callee
    // parameter / return types. Editing any of these must invalidate the cache.
    for p in &atom.params {
        hasher.update(b"|param_type:");
        hasher.update(p.name.as_bytes());
        hasher.update(b"=");
        hasher.update(p.type_name.as_deref().unwrap_or("").as_bytes());
    }
    hasher.update(b"|return_type:");
    hasher.update(atom.return_type.as_deref().unwrap_or("").as_bytes());
    let mut struct_names: Vec<&String> = module_env.structs.keys().collect();
    struct_names.sort();
    for name in struct_names {
        hasher.update(b"|struct:");
        hasher.update(name.as_bytes());
        for f in &module_env.structs[name].fields {
            hasher.update(b",");
            hasher.update(f.name.as_bytes());
            hasher.update(b":");
            hasher.update(f.type_name.as_bytes());
            if let Some(constraint) = &f.constraint {
                hasher.update(b"|field_constraint:");
                hasher.update(name.as_bytes());
                hasher.update(b".");
                hasher.update(f.name.as_bytes());
                hasher.update(b"=");
                hasher.update(constraint.as_bytes());
            }
        }
        for invariant in &module_env.structs[name].invariants {
            hasher.update(b"|struct_invariant:");
            hasher.update(name.as_bytes());
            hasher.update(b"=");
            hasher.update(invariant.as_bytes());
        }
    }

    // 3. Include callee signatures (transitive dependencies)
    let mut visited = HashSet::new();
    let mut stack = Vec::new();

    // Collect direct callees from dependency graph (sorted for deterministic hashing)
    if let Some(callees) = module_env.dependency_graph.get(&atom.name) {
        let mut sorted_callees: Vec<&String> = callees.iter().collect();
        sorted_callees.sort();
        for callee in sorted_callees {
            stack.push(callee.clone());
        }
    }

    // Walk transitive callees (sort at each level for determinism)
    while let Some(callee_name) = stack.pop() {
        if !visited.insert(callee_name.clone()) {
            continue; // already visited, prevent infinite loops
        }
        if let Some(callee_atom) = module_env.get_atom(&callee_name) {
            hasher.update(b"|dep:");
            hasher.update(callee_atom.name.as_bytes());
            hasher.update(b":");
            hasher.update(callee_atom.requires.as_bytes());
            hasher.update(b":");
            hasher.update(callee_atom.ensures.as_bytes());
            if !callee_atom.clause_modes.is_empty() {
                for mode in &callee_atom.clause_modes {
                    hasher.update(b",clause_mode:");
                    hasher.update(
                        format!("{:?}|{:?}|{}", mode.kind, mode.mode, mode.clause).as_bytes(),
                    );
                }
            }
            for p in &callee_atom.params {
                hasher.update(b",param_type:");
                hasher.update(p.type_name.as_deref().unwrap_or("").as_bytes());
                hasher.update(b",param_mode:");
                let mode = if p.is_ref_mut {
                    "ref mut"
                } else if p.is_ref {
                    "ref"
                } else if callee_atom
                    .consumed_params
                    .iter()
                    .any(|consumed| consumed == &p.name)
                {
                    "consume"
                } else {
                    "owned"
                };
                hasher.update(mode.as_bytes());
            }
            hasher.update(b",return_type:");
            hasher.update(callee_atom.return_type.as_deref().unwrap_or("").as_bytes());
            hasher.update(b",semantics:");
            hasher.update(
                if crate::verification::fragment::atom_requires_bitvector_semantics(callee_atom) {
                    b"bitvec".as_slice()
                } else {
                    b"default".as_slice()
                },
            );
        }
        // Walk further dependencies
        if let Some(further_callees) = module_env.dependency_graph.get(&callee_name) {
            let mut sorted_further: Vec<&String> = further_callees.iter().collect();
            sorted_further.sort();
            for fc in sorted_further {
                if !visited.contains(fc) {
                    stack.push(fc.clone());
                }
            }
        }
    }

    // 4. Fail-closed binder calls: a call whose lowered arguments contain a
    // `forall`/`exists` bound constant is now rejected as unverifiable instead
    // of being lowered with a result constant shared across every instance.
    // Any cached proof minted before this fix could have relied on the unsound
    // sharing, so atoms that carry the pattern get a marker in their hash.
    // The syntactic check below is a conservative superset: it marks every
    // non-builtin call / call_ref under a binder, even calls the verifier
    // accepts (e.g. ground arguments), because it cannot see the lowered
    // terms. Atoms with no call under a binder are byte-identical to before.
    if binder_call_marker_applies(atom, module_env, &visited) {
        hasher.update(b"|binder_call_fail_closed_v1");
    }

    format!("{:x}", hasher.finalize())
}

/// Whether the proof hash for `atom` carries the fail-closed binder-call
/// marker: true when the atom's own clauses/body — or the requires/ensures of
/// any transitive callee in `visited` — contain a non-builtin `Call` or any
/// `CallRef` inside a `forall`/`exists` condition. Conservative superset of
/// what the verifier rejects (see `expr_has_binder_scoped_call`).
fn binder_call_marker_applies(
    atom: &Atom,
    module_env: &ModuleEnv,
    visited: &HashSet<String>,
) -> bool {
    atom_has_binder_scoped_call(atom, true)
        || visited.iter().any(|callee_name| {
            module_env
                .get_atom(callee_name)
                .is_some_and(|callee_atom| atom_has_binder_scoped_call(callee_atom, false))
        })
}

/// Call names the expression translator handles as builtins — they never mint
/// a fresh `call_*` result constant, so a bound-variable argument is only a
/// concern for user-defined (or unresolved) callees.
fn is_builtin_call_name(name: &str) -> bool {
    matches!(
        name,
        "forall"
            | "exists"
            | "len"
            | "sqrt"
            | "cast_to_int"
            | "matches"
            | "match_regex"
            | "re_match"
            | "starts_with"
            | "ends_with"
            | "contains"
            | "not_contains"
            | "is_empty"
            | "index_of"
            | "substr"
            | "char_at"
            | "server_bound"
            | "server_listening"
            | "request_live"
    )
}

/// Syntactic walk: does `expr` contain a `Call(name, args)` to a non-builtin
/// callee — or any `CallRef` — anywhere inside a `forall`/`exists` condition?
/// `forall`/`exists` push their bound variable while walking the condition,
/// so the non-empty `binders` stack is effectively a quantifier-depth check.
///
/// This is a *conservative superset* of what the verifier rejects: the
/// translator rejects only calls whose lowered arguments contain the bound
/// constant, but a syntactic pass cannot see that, so it marks every call
/// under a binder — including ground-argument calls the verifier still
/// accepts.
fn expr_has_binder_scoped_call(expr: &Expr, binders: &mut Vec<String>) -> bool {
    match expr {
        Expr::Call(name, args) if (name == "forall" || name == "exists") && args.len() == 4 => {
            // (var, start, end, condition): bounds are evaluated outside the
            // binder's scope.
            if expr_has_binder_scoped_call(&args[1], binders)
                || expr_has_binder_scoped_call(&args[2], binders)
            {
                return true;
            }
            if let Expr::Variable(var) = &args[0] {
                binders.push(var.clone());
                let found = expr_has_binder_scoped_call(&args[3], binders);
                binders.pop();
                found
            } else {
                expr_has_binder_scoped_call(&args[3], binders)
            }
        }
        Expr::Call(name, args) => {
            if !is_builtin_call_name(name) && !binders.is_empty() {
                return true;
            }
            args.iter().any(|a| expr_has_binder_scoped_call(a, binders))
        }
        Expr::ArrayLit(items) => items
            .iter()
            .any(|e| expr_has_binder_scoped_call(e, binders)),
        Expr::ArrayAccess(_, idx) => expr_has_binder_scoped_call(idx, binders),
        Expr::BinaryOp(l, _, r) => {
            expr_has_binder_scoped_call(l, binders) || expr_has_binder_scoped_call(r, binders)
        }
        Expr::Block(stmt) | Expr::Async { body: stmt } | Expr::Lambda { body: stmt, .. } => {
            stmt_has_binder_scoped_call(stmt, binders)
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            expr_has_binder_scoped_call(cond, binders)
                || stmt_has_binder_scoped_call(then_branch, binders)
                || stmt_has_binder_scoped_call(else_branch, binders)
        }
        Expr::StructInit { fields, .. } => fields
            .iter()
            .any(|(_, e)| expr_has_binder_scoped_call(e, binders)),
        Expr::FieldAccess(e, _) => expr_has_binder_scoped_call(e, binders),
        Expr::Match { target, arms } => {
            expr_has_binder_scoped_call(target, binders)
                || arms.iter().any(|arm| {
                    arm.guard
                        .as_deref()
                        .is_some_and(|g| expr_has_binder_scoped_call(g, binders))
                        || stmt_has_binder_scoped_call(&arm.body, binders)
                })
        }
        Expr::Await { expr } => expr_has_binder_scoped_call(expr, binders),
        Expr::CallRef { callee, args } => {
            if !binders.is_empty() {
                return true;
            }
            expr_has_binder_scoped_call(callee, binders)
                || args.iter().any(|a| expr_has_binder_scoped_call(a, binders))
        }
        Expr::Perform { args, .. } => args.iter().any(|a| expr_has_binder_scoped_call(a, binders)),
        Expr::ChanSend { channel, value } => {
            expr_has_binder_scoped_call(channel, binders)
                || expr_has_binder_scoped_call(value, binders)
        }
        Expr::ChanRecv { channel } => expr_has_binder_scoped_call(channel, binders),
        _ => false,
    }
}

fn stmt_has_binder_scoped_call(stmt: &Stmt, binders: &mut Vec<String>) -> bool {
    match stmt {
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            expr_has_binder_scoped_call(value, binders)
        }
        Stmt::ArrayStore { index, value, .. } => {
            expr_has_binder_scoped_call(index, binders)
                || expr_has_binder_scoped_call(value, binders)
        }
        Stmt::Block(stmts, _)
        | Stmt::TaskGroup {
            children: stmts, ..
        } => stmts
            .iter()
            .any(|s| stmt_has_binder_scoped_call(s, binders)),
        Stmt::While {
            cond,
            invariant,
            decreases,
            body,
            ..
        } => {
            expr_has_binder_scoped_call(cond, binders)
                || expr_has_binder_scoped_call(invariant, binders)
                || decreases
                    .as_deref()
                    .is_some_and(|d| expr_has_binder_scoped_call(d, binders))
                || stmt_has_binder_scoped_call(body, binders)
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
            stmt_has_binder_scoped_call(body, binders)
        }
        Stmt::Expr(e, _) => expr_has_binder_scoped_call(e, binders),
        Stmt::Cancel { .. } => false,
    }
}

/// True when the atom's own spec (requires, extracted quantifier conditions,
/// ensures, covers, invariant — and, when `check_body`, the body including
/// loop invariants) contains a non-builtin `Call` or any `CallRef` inside a
/// `forall`/`exists` condition. Conservative superset of the verifier's
/// semantic check — see `expr_has_binder_scoped_call`.
fn atom_has_binder_scoped_call(atom: &Atom, check_body: bool) -> bool {
    let mut clause_exprs: Vec<Expr> = Vec::new();
    // `atom.requires` has its top-level quantifiers extracted into
    // `forall_constraints`; walk each condition with its bound variable on
    // the binder stack.
    for quantifier in &atom.forall_constraints {
        // Bounds are evaluated outside the binder scope, matching the
        // Call("forall") arm of `expr_has_binder_scoped_call`.
        for clause in [&quantifier.start, &quantifier.end] {
            if expr_has_binder_scoped_call(
                &crate::parser::parse_expression(clause),
                &mut Vec::new(),
            ) {
                return true;
            }
        }
        let mut binders = vec![quantifier.var.clone()];
        if expr_has_binder_scoped_call(
            &crate::parser::parse_expression(&quantifier.condition),
            &mut binders,
        ) {
            return true;
        }
    }
    clause_exprs.push(crate::parser::parse_expression(&atom.requires));
    clause_exprs.push(crate::parser::parse_expression(&atom.ensures));
    for cover in &atom.covers {
        clause_exprs.push(crate::parser::parse_expression(&cover.clause));
    }
    if let Some(invariant) = atom.invariant.as_deref() {
        clause_exprs.push(crate::parser::parse_expression(invariant));
    }
    for expr in &clause_exprs {
        if expr_has_binder_scoped_call(expr, &mut Vec::new()) {
            return true;
        }
    }
    check_body
        && stmt_has_binder_scoped_call(
            &crate::parser::parse_body_expr(&atom.body_expr),
            &mut Vec::new(),
        )
}

/// Compute a hash for the contract (specification) portion only.
/// This is used to detect unauthorized specification mutations by the agent.
pub fn compute_contract_hash(atom: &crate::parser::Atom) -> String {
    let mut hasher = Sha256::new();

    hash_field(&mut hasher, "name", &atom.name);
    hash_field(&mut hasher, "requires", &atom.requires);
    for q in &atom.forall_constraints {
        let quantifier_type = match q.q_type {
            crate::parser::QuantifierType::ForAll => "forall",
            crate::parser::QuantifierType::Exists => "exists",
        };
        hash_field(&mut hasher, "quantifier.type", quantifier_type);
        hash_field(&mut hasher, "quantifier.var", &q.var);
        hash_field(&mut hasher, "quantifier.start", &q.start);
        hash_field(&mut hasher, "quantifier.end", &q.end);
        hash_field(&mut hasher, "quantifier.condition", &q.condition);
    }
    hash_field(&mut hasher, "ensures", &atom.ensures);
    if !atom.clause_modes.is_empty() {
        for mode in &atom.clause_modes {
            hash_field(
                &mut hasher,
                "clause_mode",
                &format!("{:?}|{:?}|{}", mode.kind, mode.mode, mode.clause),
            );
        }
    }
    for cover in &atom.covers {
        let mut section = cover.clause.clone();
        if let Some(label) = &cover.label {
            section.push_str("|label=");
            section.push_str(label);
        }
        hash_field(&mut hasher, "cover", &section);
    }
    if let Some(ref inv) = atom.invariant {
        hash_field(&mut hasher, "invariant", inv);
    }
    for e in &atom.effects {
        hash_field(&mut hasher, "effect.name", &e.name);
        hash_field(
            &mut hasher,
            "effect.negated",
            if e.negated { "true" } else { "false" },
        );
        for p in &e.params {
            hash_field(&mut hasher, "effect.param.value", &p.value);
            hash_field(
                &mut hasher,
                "effect.param.is_constant",
                if p.is_constant { "true" } else { "false" },
            );
            if let Some(ref refinement) = p.refinement {
                hash_field(&mut hasher, "effect.param.refinement", refinement);
            }
        }
    }
    for p in &atom.params {
        if let Some(ref req) = p.fn_contract_requires {
            hash_field(&mut hasher, "fn_contract.param", &p.name);
            hash_field(&mut hasher, "fn_contract.requires", req);
        }
        if let Some(ref ens) = p.fn_contract_ensures {
            hash_field(&mut hasher, "fn_contract.param", &p.name);
            hash_field(&mut hasher, "fn_contract.ensures", ens);
        }
    }
    hash_string_map(&mut hasher, "effect_pre", &atom.effect_pre);
    hash_string_map(&mut hasher, "effect_post", &atom.effect_post);

    format!("{:x}", hasher.finalize())
}

pub(crate) fn hash_field(hasher: &mut Sha256, label: &str, value: &str) {
    hasher.update(label.as_bytes());
    hasher.update(b"#");
    hasher.update(value.len().to_string().as_bytes());
    hasher.update(b":");
    hasher.update(value.as_bytes());
    hasher.update(b";");
}

pub(crate) fn hash_string_map(hasher: &mut Sha256, label: &str, values: &HashMap<String, String>) {
    let sorted: BTreeMap<&String, &String> = values.iter().collect();
    for (key, value) in sorted {
        hash_field(hasher, label, key);
        hash_field(hasher, label, value);
    }
}

/// Collect callee names from an atom's body expression string.
/// This is a simple text-based extraction of function call names.
pub fn collect_callees_from_body(body_expr: &str) -> HashSet<String> {
    let mut callees = HashSet::new();
    // Match patterns like "func_name(" in the body expression
    let chars: Vec<char> = body_expr.chars().collect();
    let len = chars.len();
    let mut i = 0;
    while i < len {
        // Look for identifier followed by '('
        if chars[i].is_ascii_alphabetic() || chars[i] == '_' {
            let start = i;
            while i < len && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let ident: String = chars[start..i].iter().collect();
            // Skip whitespace
            while i < len && chars[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < len && chars[i] == '(' {
                // Skip known keywords
                let keywords = [
                    "if", "else", "while", "let", "match", "true", "false", "return", "acquire",
                    "release", "perform", "async", "await", "call",
                ];
                if !keywords.contains(&ident.as_str()) {
                    callees.insert(ident);
                }
            }
        } else {
            i += 1;
        }
    }
    callees
}

/// Collect callee names from every clause of an atom.
///
/// A callee is a cache dependency wherever it appears, not only in the body:
/// an atom whose `requires`/`ensures`/`invariant` or quantifier bounds call
/// another atom imports that atom's contract — and its semantic mode — into
/// its own proof, so a change there has to invalidate the cached proof.
pub fn collect_callees_from_atom(atom: &Atom) -> HashSet<String> {
    let mut callees = collect_callees_from_body(&atom.body_expr);
    callees.extend(collect_callees_from_body(&atom.requires));
    callees.extend(collect_callees_from_body(&atom.ensures));
    if let Some(invariant) = atom.invariant.as_deref() {
        callees.extend(collect_callees_from_body(invariant));
    }
    for quantifier in &atom.forall_constraints {
        for clause in [&quantifier.start, &quantifier.end, &quantifier.condition] {
            callees.extend(collect_callees_from_body(clause));
        }
    }
    callees
}

/// Load the enhanced verification cache from `.mumei/cache/verification_cache.json`.
pub fn load_verification_cache(base_dir: &Path) -> HashMap<String, VerificationCacheEntry> {
    let cache_path = base_dir
        .join(".mumei")
        .join("cache")
        .join("verification_cache.json");
    fs::read_to_string(&cache_path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

/// Save the enhanced verification cache to `.mumei/cache/verification_cache.json`.
pub fn save_verification_cache(base_dir: &Path, cache: &HashMap<String, VerificationCacheEntry>) {
    let cache_dir = base_dir.join(".mumei").join("cache");
    let _ = fs::create_dir_all(&cache_dir);
    let cache_path = cache_dir.join("verification_cache.json");
    if let Ok(json) = serde_json::to_string_pretty(cache) {
        let _ = fs::write(cache_path, json);
    }
}

/// Invalidate cache entries for all atoms that transitively depend on the changed atom.
// NOTE: invalidate_dependents is no longer called because compute_proof_hash already includes
// callee signatures (requires/ensures) in the hash. If a callee's contract changes, all callers
// will have different proof hashes and be re-verified automatically. Kept for potential future use.
#[allow(dead_code)]
pub fn invalidate_dependents(
    cache: &mut HashMap<String, VerificationCacheEntry>,
    changed_atom: &str,
    module_env: &ModuleEnv,
) {
    let dependents = module_env.get_transitive_dependents(changed_atom);
    for dep in &dependents {
        cache.remove(dep);
    }
}

/// Migrate old `.mumei_build_cache` to new `.mumei/cache/verification_cache.json`.
/// On successful migration, deletes the old cache file.
pub fn migrate_old_cache(base_dir: &Path) {
    let old_path = base_dir.join(".mumei_build_cache");
    if !old_path.exists() {
        return;
    }
    // Only migrate if new cache doesn't exist yet
    let new_cache_path = base_dir
        .join(".mumei")
        .join("cache")
        .join("verification_cache.json");
    if new_cache_path.exists() {
        // New cache already exists, just delete old
        let _ = fs::remove_file(&old_path);
        return;
    }
    // Load old cache
    if let Ok(content) = fs::read_to_string(&old_path) {
        if let Ok(old_cache) = serde_json::from_str::<HashMap<String, String>>(&content) {
            let mut new_cache: HashMap<String, VerificationCacheEntry> = HashMap::new();
            let timestamp = chrono_timestamp();
            for (name, hash) in old_cache {
                new_cache.insert(
                    name,
                    VerificationCacheEntry {
                        proof_hash: hash,
                        result: "verified".to_string(),
                        dependencies: Vec::new(),
                        type_deps: Vec::new(),
                        timestamp: timestamp.clone(),
                        skipped_clauses: 0,
                        inferred_invariants: Vec::new(),
                        cover_results: Vec::new(),
                    },
                );
            }
            save_verification_cache(base_dir, &new_cache);
        }
    }
    let _ = fs::remove_file(&old_path);
}

/// Simple timestamp string for cache entries.
pub(crate) fn chrono_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}s", duration.as_secs())
}

/// キャッシュファイルを読み込む。存在しない場合は空のキャッシュを返す。
pub(crate) fn load_cache(cache_path: &Path) -> VerificationCache {
    fs::read_to_string(cache_path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

/// キャッシュファイルに書き込む。書き込み失敗は無視する（キャッシュは最適化であり必須ではない）。
pub(crate) fn save_cache(cache_path: &Path, cache: &VerificationCache) {
    if let Ok(json) = serde_json::to_string_pretty(cache) {
        let _ = fs::write(cache_path, json);
    }
}

#[cfg(test)]
mod nominal_hash_tests {
    use super::*;
    use crate::parser::{parse_module, Item};

    fn env_and_hash(source: &str) -> String {
        let items = parse_module(source);
        let mut module_env = ModuleEnv::default();
        for item in &items {
            match item {
                Item::Atom(atom) => {
                    module_env.atoms.insert(atom.name.clone(), atom.clone());
                }
                Item::StructDef(s) => {
                    module_env.structs.insert(s.name.clone(), s.clone());
                }
                Item::ResourceDef(resource) => {
                    module_env.register_resource(resource);
                }
                _ => {}
            }
        }
        module_env
            .dependency_graph
            .entry("main".to_string())
            .or_default()
            .insert("getx".to_string());
        let main = module_env.atoms.get("main").unwrap().clone();
        compute_proof_hash(&main, &module_env)
    }

    const MAIN: &str = r#"
trusted atom main() -> i64
requires: true;
ensures: true;
body: { getx(Pair { a: 1, b: 2 }) };
"#;

    #[test]
    fn callee_param_type_change_invalidates_the_proof_hash() {
        let structs = "struct Point { x: i64, y: i64 }\nstruct Pair { a: i64, b: i64 }\n";
        let before = env_and_hash(&format!(
            "{structs}\ntrusted atom getx(p: Pair) -> i64\nrequires: true;\nensures: true;\nbody: {{ p.a }};\n{MAIN}"
        ));
        let after = env_and_hash(&format!(
            "{structs}\ntrusted atom getx(p: Point) -> i64\nrequires: true;\nensures: true;\nbody: {{ p.x }};\n{MAIN}"
        ));
        assert_ne!(before, after);
    }

    #[test]
    fn callee_param_mode_change_invalidates_the_proof_hash() {
        let before = env_and_hash(
            "trusted atom getx(ref p: i64) -> i64\n\
             requires: true;\n\
             ensures: true;\n\
             body: p;\n\
             trusted atom main(x: i64) -> i64\n\
             requires: true;\n\
             ensures: true;\n\
             body: getx(ref x);\n",
        );
        let after = env_and_hash(
            "trusted atom getx(ref mut p: i64) -> i64\n\
             requires: true;\n\
             ensures: true;\n\
             body: p;\n\
             trusted atom main(x: i64) -> i64\n\
             requires: true;\n\
             ensures: true;\n\
             body: getx(ref mut x);\n",
        );
        assert_ne!(before, after);
    }

    #[test]
    fn struct_field_type_change_invalidates_the_proof_hash() {
        let getx =
            "trusted atom getx(p: Pair) -> i64\nrequires: true;\nensures: true;\nbody: { p.a };\n";
        let before = env_and_hash(&format!("struct Pair {{ a: i64, b: i64 }}\n{getx}{MAIN}"));
        let after = env_and_hash(&format!("struct Pair {{ a: f64, b: i64 }}\n{getx}{MAIN}"));
        assert_ne!(before, after);
    }

    #[test]
    fn callee_semantics_change_invalidates_the_proof_hash() {
        let getx_default =
            "trusted atom getx(p: Pair) -> i64\nrequires: true;\nensures: true;\nbody: { p.a };\n";
        let getx_bitvec = "trusted atom getx(p: Pair) -> i64\n\
semantics: bitvec;\n\
requires: true;\n\
ensures: true;\n\
body: { p.a };\n";
        let parsed = parse_module(getx_bitvec);
        let callee = parsed
            .iter()
            .find_map(|item| match item {
                Item::Atom(atom) if atom.name == "getx" => Some(atom),
                _ => None,
            })
            .expect("parse bit-vector callee");
        assert_eq!(
            callee.spec_metadata.get("semantics").map(String::as_str),
            Some("bitvec")
        );

        let before = env_and_hash(&format!(
            "struct Pair {{ a: i64, b: i64 }}\n{getx_default}{MAIN}"
        ));
        let after = env_and_hash(&format!(
            "struct Pair {{ a: i64, b: i64 }}\n{getx_bitvec}{MAIN}"
        ));
        assert_ne!(before, after);
    }

    #[test]
    fn resource_invariant_change_invalidates_the_proof_hash() {
        let before = env_and_hash(
            "resource counter { value: i64 } priority: 1 mode: exclusive invariant: counter.value >= 0;\n\
             atom main() -> i64\nresources: [counter];\nrequires: true;\nensures: true;\nbody: { 0 }",
        );
        let after = env_and_hash(
            "resource counter { value: i64 } priority: 1 mode: exclusive invariant: counter.value >= 1;\n\
             atom main() -> i64\nresources: [counter];\nrequires: true;\nensures: true;\nbody: { 0 }",
        );
        assert_ne!(before, after);
    }
}

#[cfg(test)]
mod replayability_hash_tests {
    use super::*;
    use crate::parser::{parse_module, Item};

    fn hash(source: &str) -> String {
        let items = parse_module(source);
        let mut module_env = ModuleEnv::default();
        let mut atom = None;
        for item in items {
            match item {
                Item::EffectDef(def) => {
                    module_env.effect_defs.insert(def.name.clone(), def);
                }
                Item::Atom(a) => atom = Some(a),
                _ => {}
            }
        }
        compute_proof_hash(&atom.expect("atom"), &module_env)
    }

    #[test]
    fn nondeterministic_effect_resolution_participates_in_the_proof_hash() {
        let body = "body: { perform Dice.next(seed) };";
        let plain = hash(&format!(
            "effect Dice;\natom roll(seed: i64) -> i64\neffects: [Dice];\nensures: true;\n{body}"
        ));
        let child = hash(&format!(
            "effect Random;\neffect Dice parent: Random;\natom roll(seed: i64) -> i64\neffects: [Dice];\nensures: true;\n{body}"
        ));
        assert_ne!(
            plain, child,
            "resolving Dice to the Random root must change the hash"
        );
    }
}

#[cfg(test)]
mod binder_call_hash_tests {
    use super::*;
    use crate::parser::{parse_module, Item};

    fn env_and_atom(source: &str, atom_name: &str) -> (ModuleEnv, Atom) {
        let items = parse_module(source);
        let mut module_env = ModuleEnv::default();
        let mut target = None;
        for item in &items {
            if let Item::Atom(atom) = item {
                module_env.atoms.insert(atom.name.clone(), atom.clone());
                if atom.name == atom_name {
                    target = Some(atom.clone());
                }
            }
        }
        (module_env, target.expect("atom missing"))
    }

    fn hash(source: &str, atom_name: &str) -> String {
        let (module_env, atom) = env_and_atom(source, atom_name);
        compute_proof_hash(&atom, &module_env)
    }

    const IDENT: &str = r#"
atom ident(x: i64) -> i64
requires: true;
ensures: result == x;
body: x;
"#;

    #[test]
    fn bound_call_in_requires_adds_the_marker() {
        let (_, atom) = env_and_atom(
            &format!(
                "{IDENT}\natom all_equal_probe(arr: [i64], n: i64) -> i64\nrequires: n >= 2 && len(arr) >= n && forall(i, 0, n, ident(arr[i]) == arr[i]);\nensures: arr[0] == arr[1];\nbody: n;\n"
            ),
            "all_equal_probe",
        );
        assert!(atom_has_binder_scoped_call(&atom, true));
    }

    #[test]
    fn bound_call_via_nested_inner_quantifier_uses_outer_var() {
        let (_, atom) = env_and_atom(
            &format!(
                "{IDENT}\natom nested(arr: [i64], n: i64) -> i64\nrequires: n >= 2 && len(arr) >= n && forall(i, 0, n, forall(j, 0, n, ident(arr[i]) == arr[i]));\nensures: arr[0] == arr[1];\nbody: n;\n"
            ),
            "nested",
        );
        assert!(atom_has_binder_scoped_call(&atom, true));
    }

    #[test]
    fn bound_call_in_transitive_callee_ensures_marks_caller() {
        let source = format!(
            "{IDENT}\natom callee(arr: [i64], n: i64) -> i64\nrequires: n >= 1 && len(arr) >= n;\nensures: forall(i, 0, n, ident(arr[i]) == arr[i]);\nbody: n;\n\natom caller(arr: [i64], n: i64) -> i64\nrequires: n >= 1 && len(arr) >= n;\nensures: result == n;\nbody: {{ callee(arr, n) }};\n"
        );
        let (mut module_env, caller) = env_and_atom(&source, "caller");
        module_env
            .dependency_graph
            .entry("caller".to_string())
            .or_default()
            .insert("callee".to_string());
        // The caller's own spec has no bound call, but its callee's ensures
        // does — the hash must carry the marker.
        assert!(!atom_has_binder_scoped_call(&caller, true));
        let (_, callee) = env_and_atom(&source, "callee");
        assert!(atom_has_binder_scoped_call(&callee, false));
        let visited: HashSet<String> = ["callee".to_string()].into_iter().collect();
        assert!(binder_call_marker_applies(&caller, &module_env, &visited));

        // And a caller to a clean callee stays unmarked.
        let clean_source = format!(
            "{IDENT}\natom clean_callee(x: i64) -> i64\nrequires: true;\nensures: result == x;\nbody: x;\n\natom clean_caller(x: i64) -> i64\nrequires: true;\nensures: result == x;\nbody: {{ clean_callee(x) }};\n"
        );
        let (mut clean_env, clean_caller) = env_and_atom(&clean_source, "clean_caller");
        clean_env
            .dependency_graph
            .entry("clean_caller".to_string())
            .or_default()
            .insert("clean_callee".to_string());
        let clean_visited: HashSet<String> = ["clean_callee".to_string()].into_iter().collect();
        assert!(!binder_call_marker_applies(
            &clean_caller,
            &clean_env,
            &clean_visited
        ));
    }

    #[test]
    fn ground_call_under_binder_is_marked_conservatively() {
        // The verifier accepts this atom (the argument is ground), but the
        // syntactic marker is a superset: it cannot see that `ident(0)` is
        // ground, so it marks every non-builtin call under a binder.
        let (_, atom) = env_and_atom(
            &format!(
                "{IDENT}\natom ground(arr: [i64], n: i64) -> i64\nrequires: n >= 1 && len(arr) >= n && forall(i, 0, n, arr[i] >= ident(0));\nensures: arr[0] >= 0;\nbody: n;\n"
            ),
            "ground",
        );
        assert!(atom_has_binder_scoped_call(&atom, true));
    }

    #[test]
    fn builtin_reads_and_len_are_not_marked() {
        let source = r#"
atom sorted_probe(arr: [i64], n: i64) -> i64
requires: n >= 2 && len(arr) >= n && forall(i, 0, n - 1, arr[i] <= arr[i + 1]);
ensures: arr[0] <= arr[1];
body: n;
"#;
        let (_, atom) = env_and_atom(source, "sorted_probe");
        assert!(!atom_has_binder_scoped_call(&atom, true));
    }

    #[test]
    fn call_in_quantifier_bound_is_not_marked() {
        // `ident(n)` is the quantifier's end bound, which is evaluated
        // outside the binder scope, so the atom is not marked.
        let source = format!(
            "{IDENT}\natom bounded(arr: [i64], n: i64) -> i64\nrequires: n >= 1 && len(arr) >= n && forall(i, 0, ident(n), arr[i] >= 0);\nensures: result == n;\nbody: n;\n"
        );
        let (_, atom) = env_and_atom(&source, "bounded");
        assert!(!atom_has_binder_scoped_call(&atom, true));
    }

    #[test]
    fn call_outside_any_binder_is_not_marked() {
        let (_, atom) = env_and_atom(
            &format!(
                "{IDENT}\natom plain(x: i64) -> i64\nrequires: ident(x) == x;\nensures: result == x;\nbody: x;\n"
            ),
            "plain",
        );
        assert!(!atom_has_binder_scoped_call(&atom, true));
    }

    /// Golden hashes computed with the code from before the fail-closed
    /// binder-call fix: marker-free atoms must keep byte-identical proof
    /// hashes, so the marker never changes hashing for unmarked atoms.
    #[test]
    fn unmarked_atoms_keep_byte_identical_hashes() {
        let plain_source = format!(
            "{IDENT}\natom plain(x: i64) -> i64\nrequires: ident(x) == x;\nensures: result == x;\nbody: x;\n"
        );
        assert_eq!(
            hash(&plain_source, "plain"),
            "a3f9dc980fae70d7a9935a897fbaf537e1f24128680d8db24f02793e2dc9dcfb"
        );

        let sorted_source = "atom sorted_probe(arr: [i64], n: i64) -> i64\nrequires: n >= 2 && len(arr) >= n && forall(i, 0, n - 1, arr[i] <= arr[i + 1]);\nensures: arr[0] <= arr[1];\nbody: n;\n";
        assert_eq!(
            hash(sorted_source, "sorted_probe"),
            "10b939552687d80793422e3b173f7a1d18c53b9e3324798dcc87b88ff37c3974"
        );

        // `plain2` has a forall with only array reads — no call under any
        // binder — so it stays marker-free and byte-identical.
        let plain2_source = "atom plain2(arr: [i64], n: i64) -> i64\nrequires: n >= 1 && len(arr) >= n && forall(i, 0, n, arr[i] >= 0);\nensures: result == n;\nbody: n;\n";
        assert_eq!(
            hash(plain2_source, "plain2"),
            "8539cdef10921ab7704ff36a1f27ca31eefa383263b374c2c2657b64f8d4a952"
        );
    }
}

#[cfg(test)]
mod binder_call_nested_expr_tests {
    use super::*;
    use crate::parser::{parse_module, Item};

    fn has_binder_call(source: &str, atom_name: &str) -> bool {
        let items = parse_module(source);
        for item in &items {
            if let Item::Atom(atom) = item {
                if atom.name == atom_name {
                    return atom_has_binder_scoped_call(atom, true);
                }
            }
        }
        panic!("atom {atom_name} missing");
    }

    const IDENT: &str = r#"
atom ident(x: i64) -> i64
requires: true;
ensures: result == x;
body: x;
"#;

    #[test]
    fn bound_var_inside_if_argument_is_marked() {
        let source = format!(
            "{IDENT}\natom if_wrapped(arr: [i64], n: i64) -> i64\nrequires: n >= 2 && forall(i, 0, n, ident(if i >= 0 {{ i }} else {{ i }}) == i);\nensures: result == 0 - 1;\nbody: n;\n"
        );
        assert!(has_binder_call(&source, "if_wrapped"));
    }

    #[test]
    fn bound_var_inside_match_argument_is_marked() {
        let source = format!(
            "{IDENT}\natom match_wrapped(arr: [i64], n: i64) -> i64\nrequires: n >= 2 && forall(i, 0, n, ident(match i {{ _ => arr[i] }}) == arr[i]);\nensures: result == 0 - 1;\nbody: n;\n"
        );
        assert!(has_binder_call(&source, "match_wrapped"));
    }
}
