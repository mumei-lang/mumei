use crate::verification::ModuleEnv;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::Path;

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
/// - name, requires, ensures, body_expr（基本契約）
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
    hasher.update(b"|");
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

    // 1. Include everything from the basic atom hash
    hasher.update(atom.name.as_bytes());
    hasher.update(b"|");
    hasher.update(atom.requires.as_bytes());
    hasher.update(b"|");
    hasher.update(atom.ensures.as_bytes());
    hasher.update(b"|");
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
    }
    for r in &atom.resources {
        hasher.update(b"|resource:");
        hasher.update(r.as_bytes());
    }
    for e in &atom.effects {
        hasher.update(b"|effect:");
        hasher.update(e.name.as_bytes());
        for p in &e.params {
            hasher.update(b",param:");
            hasher.update(p.value.as_bytes());
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

    format!("{:x}", hasher.finalize())
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
