use crate::parser::ast::Atom;
use crate::verification::ModuleEnv;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::Path;

/// Bump when verifier semantics change so cached proofs are re-derived.
/// Version 3 enables checker-first MIR borrow checking.
/// Version 4 stops lowering calls whose arguments depend on a quantifier-bound
/// variable, so earlier "verified" results for such atoms must be re-derived.
/// Version 6 checks a callee's quantified requires at call sites and keeps
/// quantifiers nested under non-conjunctive operators in the requires text,
/// and stops quantifier binders from capturing same-named outer variables.
/// Version 7 enables recursive contracts: `decreases` termination obligations
/// and congruent recursive calls.
/// Version 8 accepts lexicographic `decreases: (m1, ..., mk);` measures.
/// Version 9 rejects calls whose argument count differs from the callee's
/// parameter count, so earlier "verified" results for such calls must be
/// re-derived.
/// Version 10 unfolds constant-argument calls into eligible recursive SCCs
/// up to a fixed depth, guarding each instance with the callee's requires
/// and parameter refinements.
/// Version 11 admits one-dimensional scalar arrays and scalar-field structs
/// as recursive-contract parameters, with length/field-valued UF domains.
/// Version 12 lowers whole-array `==`/`!=` to length equality plus a bounded
/// `forall` over `select`, so specs that compare arrays must be re-derived.
/// Version 13 admits `[i64]`/`[bool]` arrays and scalar-field structs as
/// recursive-contract RETURN values, modeling array results as (contents,
/// len) UF pairs and struct results field-wise.
/// Version 14 normalizes `Param.name` to the bare identifier and moves the
/// `consume` signature marker to `Param::consume`, so spec references such as
/// `len(xs)` resolve on `consume xs: [i64]` and prior verifications recorded
/// against prefixed names must be re-derived.
/// Version 15 declares `rec_fn#<atom>` in `unfold_constant_calls` over the
/// same expanded domain the call sites use, and catches UF
/// re-registration under a different signature.
/// Version 16 excludes auto-injected std/prelude atoms from the global
/// invariant conflict comparison — their `result` bounds are implicit
/// infrastructure, not part of the user's spec surface.
/// Version 17 unifies the two `consume` spellings: the signature marker
/// `consume x: T` now goes through the same linearity machinery as the
/// `consume x;` clause (arg consumption at call sites, is_alive tracking,
/// double-declaration rejected), so previously-verifying programs that
/// reuse an argument after calling a signature-`consume` callee may now
/// fail verification and cached results must be re-derived.
pub const VERIFIER_POLICY_VERSION: u32 = 17;

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
    if let Some(ref dec) = atom.decreases {
        hasher.update(b"|decreases:");
        hasher.update(dec.as_bytes());
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
        if p.consume {
            hasher.update(b"|consume_param:");
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
    if let Some(ref dec) = atom.decreases {
        hasher.update(b"|decreases:");
        hasher.update(dec.as_bytes());
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
        if p.consume {
            hasher.update(b"|consume_param:");
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
    hash_string_map(&mut hasher, "own_effect_pre", &atom.effect_pre);
    hash_string_map(&mut hasher, "own_effect_post", &atom.effect_post);
    for q in &atom.forall_constraints {
        let quantifier_type = match q.q_type {
            crate::parser::QuantifierType::ForAll => "forall",
            crate::parser::QuantifierType::Exists => "exists",
        };
        hasher.update(b"|own_forall:");
        hasher.update(quantifier_type.as_bytes());
        hasher.update(b"|");
        hasher.update(q.var.as_bytes());
        hasher.update(b"|");
        hasher.update(q.start.as_bytes());
        hasher.update(b"|");
        hasher.update(q.end.as_bytes());
        hasher.update(b"|");
        hasher.update(q.condition.as_bytes());
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
        hash_struct_contract(&mut hasher, name, module_env);
    }

    // 3. Include callee signatures (transitive dependencies)
    let mut visited = HashSet::new();
    let mut stack = Vec::new();

    // Collect direct callees (sorted for deterministic hashing)
    let mut effect_names: BTreeSet<String> = atom
        .effects
        .iter()
        .map(|effect| effect.name.clone())
        .collect();
    effect_names.extend(atom.effect_pre.keys().cloned());
    effect_names.extend(atom.effect_post.keys().cloned());
    let mut sorted_callees: Vec<String> = proof_hash_callee_names(atom, module_env)
        .into_iter()
        .collect();
    sorted_callees.sort();
    for callee in sorted_callees {
        stack.push(callee);
    }

    // Walk transitive callees (sort at each level for determinism)
    while let Some(callee_name) = stack.pop() {
        if !visited.insert(callee_name.clone()) {
            continue; // already visited, prevent infinite loops
        }
        if let Some(callee_atom) = module_env.get_atom(&callee_name) {
            hash_callee_contract(&mut hasher, callee_atom, module_env, true);
            hash_string_map(&mut hasher, "effect_pre", &callee_atom.effect_pre);
            hash_string_map(&mut hasher, "effect_post", &callee_atom.effect_post);
            effect_names.extend(callee_atom.effects.iter().map(|effect| effect.name.clone()));
            effect_names.extend(callee_atom.effect_pre.keys().cloned());
            effect_names.extend(callee_atom.effect_post.keys().cloned());
            let mut sorted_further: Vec<String> = proof_hash_callee_names(callee_atom, module_env)
                .into_iter()
                .collect();
            sorted_further.sort();
            for fc in sorted_further {
                if !visited.contains(&fc) {
                    stack.push(fc);
                }
            }
        }
    }

    let mut effect_def_names = BTreeSet::new();
    let mut effect_def_stack: Vec<String> = effect_names.into_iter().collect();
    while let Some(name) = effect_def_stack.pop() {
        if !effect_def_names.insert(name.clone()) {
            continue;
        }
        if let Some(effect_def) = module_env.effect_defs.get(&name) {
            effect_def_stack.extend(effect_def.includes.iter().cloned());
            effect_def_stack.extend(effect_def.parent.iter().cloned());
        }
    }
    for name in effect_def_names {
        if let Some(effect_def) = module_env.effect_defs.get(&name) {
            hasher.update(b"|effect_def:");
            hash_field(&mut hasher, "effect_def.name", &effect_def.name);
            hash_field(
                &mut hasher,
                "effect_def.params.len",
                &effect_def.params.len().to_string(),
            );
            for param in &effect_def.params {
                hash_field(&mut hasher, "effect_def.param.name", &param.name);
                hash_field(&mut hasher, "effect_def.param.type", &param.type_name);
            }
            hash_field(
                &mut hasher,
                "effect_def.constraint.present",
                if effect_def.constraint.is_some() {
                    "true"
                } else {
                    "false"
                },
            );
            if let Some(constraint) = &effect_def.constraint {
                hash_field(&mut hasher, "effect_def.constraint", constraint);
            }
            let mut includes = effect_def.includes.clone();
            includes.sort();
            hash_field(
                &mut hasher,
                "effect_def.includes.len",
                &includes.len().to_string(),
            );
            for included in includes {
                hash_field(&mut hasher, "effect_def.include", &included);
            }
            hash_field(
                &mut hasher,
                "effect_def.refinement.present",
                if effect_def.refinement.is_some() {
                    "true"
                } else {
                    "false"
                },
            );
            if let Some(refinement) = &effect_def.refinement {
                hash_field(&mut hasher, "effect_def.refinement", refinement);
            }
            let mut parents = effect_def.parent.clone();
            parents.sort();
            hash_field(
                &mut hasher,
                "effect_def.parents.len",
                &parents.len().to_string(),
            );
            for parent in parents {
                hash_field(&mut hasher, "effect_def.parent", &parent);
            }
            hash_field(
                &mut hasher,
                "effect_def.states.len",
                &effect_def.states.len().to_string(),
            );
            for state in &effect_def.states {
                hash_field(&mut hasher, "effect_def.state", state);
            }
            hash_field(
                &mut hasher,
                "effect_def.transitions.len",
                &effect_def.transitions.len().to_string(),
            );
            for transition in &effect_def.transitions {
                hash_field(
                    &mut hasher,
                    "effect_def.transition.operation",
                    &transition.operation,
                );
                hash_field(
                    &mut hasher,
                    "effect_def.transition.from",
                    &transition.from_state,
                );
                hash_field(
                    &mut hasher,
                    "effect_def.transition.to",
                    &transition.to_state,
                );
            }
            hash_field(
                &mut hasher,
                "effect_def.initial_state.present",
                if effect_def.initial_state.is_some() {
                    "true"
                } else {
                    "false"
                },
            );
            if let Some(initial_state) = &effect_def.initial_state {
                hash_field(&mut hasher, "effect_def.initial_state", initial_state);
            }
        }
    }

    format!("{:x}", hasher.finalize())
}

fn hash_callee_contract(
    hasher: &mut Sha256,
    callee_atom: &crate::parser::Atom,
    module_env: &ModuleEnv,
    include_trust: bool,
) {
    hasher.update(b"|dep:");
    hasher.update(callee_atom.name.as_bytes());
    hasher.update(b":");
    hasher.update(callee_atom.requires.as_bytes());
    hasher.update(b":");
    hasher.update(callee_atom.ensures.as_bytes());
    if let Some(ref dec) = callee_atom.decreases {
        hasher.update(b"|decreases:");
        hasher.update(dec.as_bytes());
    }
    for e in &callee_atom.effects {
        hasher.update(b",effect:");
        hasher.update(e.name.as_bytes());
        if e.negated {
            hasher.update(b",negated");
        }
        for p in &e.params {
            hasher.update(b",param:");
            hasher.update(p.value.as_bytes());
        }
    }
    if include_trust && callee_atom.trust_level != crate::parser::TrustLevel::Verified {
        hasher.update(b",trust:");
        hasher.update(
            match callee_atom.trust_level {
                crate::parser::TrustLevel::Trusted => "trusted",
                crate::parser::TrustLevel::Unverified => "unverified",
                crate::parser::TrustLevel::Verified => "verified",
            }
            .as_bytes(),
        );
    }
    if callee_atom.is_async {
        hasher.update(b",async");
    }
    if !callee_atom.type_params.is_empty() {
        hasher.update(b",type_params:");
        hasher.update(callee_atom.type_params.join(",").as_bytes());
    }
    if !callee_atom.forall_constraints.is_empty() {
        for q in &callee_atom.forall_constraints {
            let quantifier_type = match q.q_type {
                crate::parser::QuantifierType::ForAll => "forall",
                crate::parser::QuantifierType::Exists => "exists",
            };
            hasher.update(b",quantified_requires:");
            hasher.update(quantifier_type.as_bytes());
            hasher.update(b"|");
            hasher.update(q.var.as_bytes());
            hasher.update(b"|");
            hasher.update(q.start.as_bytes());
            hasher.update(b"|");
            hasher.update(q.end.as_bytes());
            hasher.update(b"|");
            hasher.update(q.condition.as_bytes());
        }
    }
    if !callee_atom.clause_modes.is_empty() {
        for mode in &callee_atom.clause_modes {
            hasher.update(b",clause_mode:");
            hasher.update(format!("{:?}|{:?}|{}", mode.kind, mode.mode, mode.clause).as_bytes());
        }
    }
    for p in &callee_atom.params {
        hasher.update(b",param_type:");
        hasher.update(p.type_name.as_deref().unwrap_or("").as_bytes());
        if let Some(refined) = p.type_name.as_deref().and_then(|t| module_env.get_type(t)) {
            hasher.update(b",param_refinement:");
            hasher.update(refined.operand.as_bytes());
            hasher.update(b"|");
            hasher.update(refined._base_type.as_bytes());
            hasher.update(b"|");
            hasher.update(refined.predicate_raw.as_bytes());
        }
        hasher.update(b",param_mode:");
        let mode = if p.is_ref_mut {
            "ref mut"
        } else if p.is_ref {
            "ref"
        } else if p.consume
            || callee_atom
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

fn hash_dependency_signature_units(
    hasher: &mut Sha256,
    atom: &crate::parser::Atom,
    module_env: &ModuleEnv,
) {
    for param in &atom.params {
        if let Some(unit) = param
            .type_name
            .as_deref()
            .and_then(|type_name| module_env.unit_of_type(type_name))
        {
            hasher.update(b"|param_unit:");
            hasher.update(param.name.as_bytes());
            hasher.update(b"=");
            hasher.update(unit.as_bytes());
        }
    }
    if let Some(unit) = atom
        .return_type
        .as_deref()
        .and_then(|type_name| module_env.unit_of_type(type_name))
    {
        hasher.update(b"|return_unit:");
        hasher.update(unit.as_bytes());
    }
}

pub fn compute_dependency_hashes(
    atom: &crate::parser::Atom,
    module_env: &ModuleEnv,
) -> BTreeMap<String, String> {
    compute_dependency_hashes_with_names(atom, module_env, false)
}

pub(crate) fn compute_legacy_dependency_hashes(
    atom: &crate::parser::Atom,
    module_env: &ModuleEnv,
) -> BTreeMap<String, String> {
    let mut hashes = compute_dependency_hashes_with_names(atom, module_env, true);
    hashes.retain(|key, _| !legacy_dependency_key_conflicts(atom, key, module_env));
    hashes
}

fn compute_dependency_hashes_with_names(
    atom: &crate::parser::Atom,
    module_env: &ModuleEnv,
    legacy_names: bool,
) -> BTreeMap<String, String> {
    let mut hashes = BTreeMap::new();
    let mut pending = dependency_callees(atom, module_env, legacy_names)
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut visited = HashSet::new();

    while let Some(callee_name) = pending.pop_first() {
        if !visited.insert(callee_name.clone()) {
            continue;
        }
        let Some(callee_atom) = module_env.get_atom(&callee_name) else {
            continue;
        };

        hashes.insert(
            format!("atom:{}", callee_atom.name),
            compute_dependency_hash_for_callee(callee_atom, module_env),
        );
        pending.extend(
            dependency_callees(callee_atom, module_env, legacy_names)
                .into_iter()
                .filter(|name| !visited.contains(name)),
        );
    }

    let mut type_names = BTreeSet::new();
    let mut struct_names = BTreeSet::new();
    for type_name in atom
        .params
        .iter()
        .filter_map(|param| param.type_name.as_deref())
        .chain(atom.return_type.as_deref())
    {
        let bare_name = type_name.split('<').next().unwrap_or(type_name).trim();
        if module_env.get_type(bare_name).is_some() {
            type_names.insert(bare_name.to_string());
        }
        if module_env.get_struct(bare_name).is_some() {
            struct_names.insert(bare_name.to_string());
        }
    }
    for param in &atom.params {
        if let Some(type_ref) = &param.type_ref {
            if module_env.get_type(&type_ref.name).is_some() {
                type_names.insert(type_ref.name.clone());
            }
            if module_env.get_struct(&type_ref.name).is_some() {
                struct_names.insert(type_ref.name.clone());
            }
        }
    }
    for name in type_names {
        let refined = module_env.get_type(&name).expect("registered refined type");
        let mut hasher = Sha256::new();
        hasher.update(refined.operand.as_bytes());
        hasher.update(b"|");
        hasher.update(refined._base_type.as_bytes());
        hasher.update(b"|");
        hasher.update(refined.predicate_raw.as_bytes());
        if let Some(unit) = module_env.unit_of_type(&name) {
            hasher.update(b"|unit:");
            hasher.update(unit.as_bytes());
        }
        hashes.insert(format!("type:{name}"), format!("{:x}", hasher.finalize()));
    }
    for name in struct_names {
        let mut hasher = Sha256::new();
        hash_struct_contract(&mut hasher, &name, module_env);
        hashes.insert(format!("struct:{name}"), format!("{:x}", hasher.finalize()));
    }

    hashes
}

fn compute_dependency_hash_for_callee(atom: &Atom, module_env: &ModuleEnv) -> String {
    let mut hasher = Sha256::new();
    hash_callee_contract(&mut hasher, atom, module_env, false);
    hash_dependency_signature_units(&mut hasher, atom, module_env);
    hash_string_map(&mut hasher, "effect_pre", &atom.effect_pre);
    hash_string_map(&mut hasher, "effect_post", &atom.effect_post);
    format!("{:x}", hasher.finalize())
}

fn legacy_dependency_key_conflicts(atom: &Atom, key: &str, module_env: &ModuleEnv) -> bool {
    let Some(bare_name) = key.strip_prefix("atom:") else {
        return false;
    };
    let Some(bare_atom) = module_env.get_atom(bare_name) else {
        return false;
    };

    collect_written_callees_from_atom(atom)
        .into_iter()
        .filter(|name| name.rsplit("::").next() == Some(bare_name))
        .any(|qualified_name| {
            if !qualified_name.contains("::") {
                return false;
            }
            let Some(qualified_atom) = module_env.get_atom(&qualified_name) else {
                return false;
            };
            let mut renamed = qualified_atom.clone();
            renamed.name = bare_name.to_string();
            compute_dependency_hash_for_callee(&renamed, module_env)
                != compute_dependency_hash_for_callee(bare_atom, module_env)
        })
}

fn dependency_callees(atom: &Atom, module_env: &ModuleEnv, legacy_names: bool) -> HashSet<String> {
    if legacy_names {
        collect_callees_from_atom(atom)
    } else {
        collect_written_callees_from_atom(atom)
            .iter()
            .map(|name| resolve_written_callee(name, module_env))
            .collect()
    }
}

fn resolve_written_callee(name: &str, module_env: &ModuleEnv) -> String {
    if module_env.get_atom(name).is_some() {
        name.to_string()
    } else {
        name.rsplit("::").next().unwrap_or(name).to_string()
    }
}

fn proof_hash_callee_names(atom: &Atom, module_env: &ModuleEnv) -> HashSet<String> {
    let mut names = module_env
        .dependency_graph
        .get(&atom.name)
        .cloned()
        .unwrap_or_default();
    let written_names = collect_written_callees_from_atom(atom);
    let unqualified_names = written_names
        .iter()
        .filter(|name| !name.contains("::"))
        .cloned()
        .collect::<HashSet<_>>();

    for qualified in written_names
        .into_iter()
        .filter(|name| name.contains("::") && module_env.get_atom(name).is_some())
    {
        let bare_name = qualified.rsplit("::").next().unwrap_or(&qualified);
        if !unqualified_names.contains(bare_name) {
            names.remove(bare_name);
        }
        names.insert(qualified);
    }
    names
}

fn hash_struct_contract(hasher: &mut Sha256, name: &str, module_env: &ModuleEnv) {
    hasher.update(b"|struct:");
    hasher.update(name.as_bytes());
    if let Some(struct_def) = module_env.structs.get(name) {
        for f in &struct_def.fields {
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
        for invariant in &struct_def.invariants {
            hasher.update(b"|struct_invariant:");
            hasher.update(name.as_bytes());
            hasher.update(b"=");
            hasher.update(invariant.as_bytes());
        }
    }
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
    if let Some(ref dec) = atom.decreases {
        hash_field(&mut hasher, "decreases", dec);
    }
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

/// Collect call names as written, preserving any `::` qualification.
pub fn collect_written_callees_from_body(body_expr: &str) -> HashSet<String> {
    let mut callees = HashSet::new();
    let chars: Vec<char> = body_expr.chars().collect();
    let len = chars.len();
    let mut i = 0;
    while i < len {
        if chars[i].is_ascii_alphabetic() || chars[i] == '_' {
            let start = i;
            while i < len && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let mut name: String = chars[start..i].iter().collect();
            let mut qualified = false;
            loop {
                let mut separator = i;
                while separator < len && chars[separator].is_ascii_whitespace() {
                    separator += 1;
                }
                let mut segment = if separator < len && chars[separator] == '.' {
                    separator + 1
                } else if separator + 1 < len
                    && chars[separator] == ':'
                    && chars[separator + 1] == ':'
                {
                    separator + 2
                } else {
                    break;
                };
                while segment < len && chars[segment].is_ascii_whitespace() {
                    segment += 1;
                }
                if segment == len
                    || !(chars[segment].is_ascii_alphabetic() || chars[segment] == '_')
                {
                    break;
                }
                qualified = true;
                name.push_str("::");
                i = segment;
                let segment_start = i;
                while i < len && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                name.extend(chars[segment_start..i].iter());
            }
            let mut next = i;
            while next < len && chars[next].is_ascii_whitespace() {
                next += 1;
            }
            if next < len && chars[next] == '(' {
                let keywords = [
                    "if", "else", "while", "let", "match", "true", "false", "return", "acquire",
                    "release", "perform", "async", "await", "call",
                ];
                if qualified || !keywords.contains(&name.as_str()) {
                    callees.insert(name);
                }
            }
            i = next;
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

/// Collect call names as written from every clause of an atom.
pub fn collect_written_callees_from_atom(atom: &Atom) -> HashSet<String> {
    let mut callees = collect_written_callees_from_body(&atom.body_expr);
    callees.extend(collect_written_callees_from_body(&atom.requires));
    callees.extend(collect_written_callees_from_body(&atom.ensures));
    if let Some(invariant) = atom.invariant.as_deref() {
        callees.extend(collect_written_callees_from_body(invariant));
    }
    for quantifier in &atom.forall_constraints {
        for clause in [&quantifier.start, &quantifier.end, &quantifier.condition] {
            callees.extend(collect_written_callees_from_body(clause));
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
    use crate::parser::ast::{EffectDef, EffectDefParam, EffectTransition, Span};
    use crate::parser::{parse_module, Item, TrustLevel};

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

    fn dependency_and_proof_hashes_for_trust(
        trust_level: TrustLevel,
    ) -> (BTreeMap<String, String>, String) {
        let items = parse_module(
            "atom f(x: i64) -> i64\n\
             requires: true;\n\
             ensures: result >= 0;\n\
             body: x;\n\
             atom g(x: i64) -> i64\n\
             requires: true;\n\
             ensures: result >= 0;\n\
             body: f(x);\n",
        );
        let mut module_env = ModuleEnv::default();
        for item in items {
            if let Item::Atom(atom) = item {
                module_env.atoms.insert(atom.name.clone(), atom);
            }
        }
        module_env
            .dependency_graph
            .entry("g".to_string())
            .or_default()
            .insert("f".to_string());
        module_env.atoms.get_mut("f").unwrap().trust_level = trust_level;
        let root = module_env.atoms.get("g").unwrap().clone();
        (
            compute_dependency_hashes(&root, &module_env),
            compute_proof_hash(&root, &module_env),
        )
    }

    fn dependency_hash_for(source: &str, root_name: &str, dependency_key: &str) -> String {
        let items = parse_module(source);
        let mut module_env = ModuleEnv::default();
        for item in items {
            match item {
                Item::Atom(atom) => {
                    module_env.atoms.insert(atom.name.clone(), atom);
                }
                Item::TypeDef(refined) => module_env.register_type(&refined),
                _ => {}
            }
        }
        let root = module_env.atoms.get(root_name).unwrap().clone();
        compute_dependency_hashes(&root, &module_env)
            .remove(dependency_key)
            .unwrap()
    }

    fn proof_hash_for(source: &str, root_name: &str) -> String {
        let items = parse_module(source);
        let mut module_env = ModuleEnv::default();
        for item in items {
            match item {
                Item::Atom(atom) => {
                    let name = atom.name.clone();
                    module_env
                        .dependency_graph
                        .insert(name, collect_callees_from_atom(&atom));
                    module_env.atoms.insert(atom.name.clone(), atom);
                }
                Item::EffectDef(effect_def) => module_env.register_effect(&effect_def),
                _ => {}
            }
        }
        let root = module_env.atoms.get(root_name).unwrap().clone();
        compute_proof_hash(&root, &module_env)
    }

    fn proof_hash_with_effect_def(effect_def: crate::parser::ast::EffectDef) -> String {
        let items = parse_module(
            "atom main(x: i64) -> i64\n\
             effects: [Work];\n\
             requires: true;\n\
             ensures: result == x;\n\
             body: x;\n",
        );
        let mut module_env = ModuleEnv::default();
        for item in items {
            if let Item::Atom(atom) = item {
                module_env.atoms.insert(atom.name.clone(), atom);
            }
        }
        module_env
            .effects
            .insert("Work".to_string(), effect_def.clone());
        module_env
            .effect_defs
            .insert("Work".to_string(), effect_def);
        let root = module_env.atoms.get("main").unwrap().clone();
        compute_proof_hash(&root, &module_env)
    }

    fn proof_hash_with_effect_defs(
        effect_defs: Vec<crate::parser::ast::EffectDef>,
        root_effect: &str,
    ) -> String {
        let source = format!(
            "atom main(x: i64) -> i64\n\
             effects: [{root_effect}];\n\
             requires: true;\n\
             ensures: result == x;\n\
             body: x;\n"
        );
        let items = parse_module(&source);
        let mut module_env = ModuleEnv::default();
        for item in items {
            if let Item::Atom(atom) = item {
                module_env.atoms.insert(atom.name.clone(), atom);
            }
        }
        for effect_def in effect_defs {
            module_env.register_effect(&effect_def);
        }
        let root = module_env.atoms.get("main").unwrap().clone();
        compute_proof_hash(&root, &module_env)
    }

    fn base_effect_def() -> EffectDef {
        EffectDef {
            name: "Work".to_string(),
            params: vec![EffectDefParam {
                name: "path".to_string(),
                type_name: "Str".to_string(),
            }],
            constraint: Some("path != \"\"".to_string()),
            includes: vec!["Leaf".to_string()],
            refinement: Some("path.starts_with(\"/\")".to_string()),
            parent: vec!["Parent".to_string()],
            span: Span::default(),
            states: vec!["Idle".to_string(), "Busy".to_string()],
            transitions: vec![EffectTransition {
                operation: "start".to_string(),
                from_state: "Idle".to_string(),
                to_state: "Busy".to_string(),
            }],
            initial_state: Some("Idle".to_string()),
        }
    }

    #[test]
    fn dependency_hashes_ignore_callee_trust_but_proof_hashes_include_it() {
        let (verified_dependencies, verified_proof) =
            dependency_and_proof_hashes_for_trust(TrustLevel::Verified);

        for trust_level in [TrustLevel::Unverified, TrustLevel::Trusted] {
            let (dependencies, proof) = dependency_and_proof_hashes_for_trust(trust_level);
            assert_eq!(dependencies, verified_dependencies);
            assert_ne!(proof, verified_proof);
        }
    }

    #[test]
    fn written_callee_collector_preserves_qualification_and_skips_keywords() {
        let collect = |source: &str| collect_written_callees_from_body(source);
        let names = |values: &[&str]| {
            values
                .iter()
                .map(|value| (*value).to_string())
                .collect::<HashSet<_>>()
        };

        assert_eq!(collect("lib::f(x)"), names(&["lib::f"]));
        assert_eq!(collect("a::b::f (x)"), names(&["a::b::f"]));
        assert_eq!(collect("f(x) + lib::f(y)"), names(&["f", "lib::f"]));
        assert_eq!(collect("Point::new(1)"), names(&["Point::new"]));
        assert_eq!(collect("lib :: f(x)"), names(&["lib::f"]));
        assert_eq!(collect("a :: b :: f (x)"), names(&["a::b::f"]));
        assert_eq!(collect("math.add(x)"), names(&["math::add"]));
        assert_eq!(collect("r.hi - r.lo"), names(&[]));
        assert_eq!(collect("a.b.c(1)"), names(&["a::b::c"]));
        assert_eq!(collect("a . b (x)"), names(&["a::b"]));
        assert_eq!(collect("lib::K + g(x)"), names(&["g"]));
        assert_eq!(collect("if(cond) { while(true) { g(x) } }"), names(&["g"]));

        let items = parse_module(
            "atom g(x: i64) -> i64\n\
             requires: true;\n\
             ensures: result >= 0;\n\
             body: lib::f(x);\n",
        );
        let Item::Atom(atom) = &items[0] else {
            panic!("expected g atom");
        };
        assert_eq!(collect_written_callees_from_atom(atom), names(&["lib::f"]));
    }

    #[test]
    fn written_callee_resolution_prefers_exact_names_and_falls_back_to_last_segment() {
        let mut module_env = ModuleEnv::default();
        let items = parse_module(
            "atom f(x: i64) -> i64\n\
             requires: true;\n\
             ensures: result >= 0;\n\
             body: x;\n",
        );
        let Item::Atom(atom) = &items[0] else {
            panic!("expected f atom");
        };
        module_env.register_atom(atom);
        let mut aliased = atom.clone();
        aliased.name = "lib::f".to_string();
        module_env.register_atom(&aliased);

        assert_eq!(resolve_written_callee("lib::f", &module_env), "lib::f");
        assert_eq!(resolve_written_callee("other::f", &module_env), "f");
        assert_eq!(resolve_written_callee("unknown", &module_env), "unknown");
    }

    #[test]
    fn dependency_hash_uses_qualified_aliased_callee_key() {
        let items = parse_module(
            "atom f(x: i64) -> i64\n\
             requires: true;\n\
             ensures: result >= 0;\n\
             body: x;\n\
             atom g(x: i64) -> i64\n\
             requires: true;\n\
             ensures: result >= 0;\n\
             body: lib::f(x);\n",
        );
        let mut module_env = ModuleEnv::default();
        for item in items {
            if let Item::Atom(atom) = item {
                module_env.register_atom(&atom);
                if atom.name == "f" {
                    let mut aliased = atom;
                    aliased.name = "lib::f".to_string();
                    module_env.register_atom(&aliased);
                }
            }
        }
        let root = module_env.get_atom("g").unwrap().clone();
        let hashes = compute_dependency_hashes(&root, &module_env);

        assert_eq!(
            hashes.keys().cloned().collect::<Vec<_>>(),
            vec!["atom:lib::f".to_string()]
        );
    }

    #[test]
    fn proof_hash_callee_names_match_the_graph_for_unqualified_calls() {
        let items = parse_module(
            "atom f(x: i64) -> i64\n\
             requires: true;\n\
             ensures: result >= 0;\n\
             body: x;\n\
             atom g(x: i64) -> i64\n\
             requires: true;\n\
             ensures: result >= 0;\n\
             body: f(x);\n",
        );
        let mut module_env = ModuleEnv::default();
        for item in items {
            if let Item::Atom(atom) = item {
                module_env.register_atom(&atom);
            }
        }
        module_env
            .dependency_graph
            .entry("g".to_string())
            .or_default()
            .insert("f".to_string());
        let root = module_env.get_atom("g").unwrap();
        let graph_names = module_env.dependency_graph.get("g").unwrap().clone();

        assert!(collect_written_callees_from_atom(root)
            .iter()
            .all(|name| !name.contains("::")));
        assert_eq!(proof_hash_callee_names(root, &module_env), graph_names);
    }

    #[test]
    fn dependency_hash_tracks_signature_type_units() {
        let usd = dependency_hash_for(
            "type Money = i64 unit USD;\n\
             atom g(x: Money) -> i64\n\
             requires: true;\n\
             ensures: result >= 0;\n\
             body: x;\n",
            "g",
            "type:Money",
        );
        let eur = dependency_hash_for(
            "type Money = i64 unit EUR;\n\
             atom g(x: Money) -> i64\n\
             requires: true;\n\
             ensures: result >= 0;\n\
             body: x;\n",
            "g",
            "type:Money",
        );
        assert_ne!(usd, eur);
    }

    #[test]
    fn dependency_hash_tracks_callee_param_type_units() {
        let usd = dependency_hash_for(
            "type Money = i64 unit USD;\n\
             atom f(x: Money) -> i64\n\
             requires: true;\n\
             ensures: result >= 0;\n\
             body: x;\n\
             atom g(x: i64) -> i64\n\
             requires: true;\n\
             ensures: result >= 0;\n\
             body: f(x);\n",
            "g",
            "atom:f",
        );
        let eur = dependency_hash_for(
            "type Money = i64 unit EUR;\n\
             atom f(x: Money) -> i64\n\
             requires: true;\n\
             ensures: result >= 0;\n\
             body: x;\n\
             atom g(x: i64) -> i64\n\
             requires: true;\n\
             ensures: result >= 0;\n\
             body: f(x);\n",
            "g",
            "atom:f",
        );
        assert_ne!(usd, eur);
    }

    #[test]
    fn dependency_hash_tracks_callee_effect_pre_and_effect_post() {
        let effect_pre_open = dependency_hash_for(
            "atom f(x: i64) -> i64\n\
             effect_pre: { File: Open };\n\
             effect_post: { File: Open };\n\
             requires: true;\n\
             ensures: result >= 0;\n\
             body: x;\n\
             atom g(x: i64) -> i64\n\
             requires: true;\n\
             ensures: result >= 0;\n\
             body: f(x);\n",
            "g",
            "atom:f",
        );
        let effect_pre_closed = dependency_hash_for(
            "atom f(x: i64) -> i64\n\
             effect_pre: { File: Closed };\n\
             effect_post: { File: Open };\n\
             requires: true;\n\
             ensures: result >= 0;\n\
             body: x;\n\
             atom g(x: i64) -> i64\n\
             requires: true;\n\
             ensures: result >= 0;\n\
             body: f(x);\n",
            "g",
            "atom:f",
        );
        assert_ne!(effect_pre_open, effect_pre_closed);

        let effect_post_open = dependency_hash_for(
            "atom f(x: i64) -> i64\n\
             effect_pre: { File: Open };\n\
             effect_post: { File: Open };\n\
             requires: true;\n\
             ensures: result >= 0;\n\
             body: x;\n\
             atom g(x: i64) -> i64\n\
             requires: true;\n\
             ensures: result >= 0;\n\
             body: f(x);\n",
            "g",
            "atom:f",
        );
        let effect_post_closed = dependency_hash_for(
            "atom f(x: i64) -> i64\n\
             effect_pre: { File: Open };\n\
             effect_post: { File: Closed };\n\
             requires: true;\n\
             ensures: result >= 0;\n\
             body: x;\n\
             atom g(x: i64) -> i64\n\
             requires: true;\n\
             ensures: result >= 0;\n\
             body: f(x);\n",
            "g",
            "atom:f",
        );
        assert_ne!(effect_post_open, effect_post_closed);
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
    fn callee_quantified_requires_change_invalidates_the_proof_hash() {
        let before = env_and_hash(
            "trusted atom getx(arr: [i64], n: i64) -> i64\n\
             requires: n >= 0 && forall(i, 0, n, arr[i] > 0);\n\
             ensures: result > 0;\n\
             body: arr[0];\n\
             trusted atom main(arr: [i64], n: i64) -> i64\n\
             requires: n >= 1 && forall(i, 0, n, arr[i] > 1);\n\
             ensures: result > 0;\n\
             body: getx(arr, n);\n",
        );
        let after = env_and_hash(
            "trusted atom getx(arr: [i64], n: i64) -> i64\n\
             requires: n >= 0 && forall(i, 0, n, arr[i] > 1);\n\
             ensures: result > 0;\n\
             body: arr[0];\n\
             trusted atom main(arr: [i64], n: i64) -> i64\n\
             requires: n >= 1 && forall(i, 0, n, arr[i] > 1);\n\
             ensures: result > 0;\n\
             body: getx(arr, n);\n",
        );
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
    fn callee_signature_consume_invalidates_the_proof_hash() {
        let before = env_and_hash(
            "trusted atom getx(x: i64) -> i64\n\
             requires: true;\n\
             ensures: true;\n\
             body: x;\n\
             trusted atom main(y: i64) -> i64\n\
             requires: true;\n\
             ensures: true;\n\
             body: getx(y);\n",
        );
        let after = env_and_hash(
            "trusted atom getx(consume x: i64) -> i64\n\
             requires: true;\n\
             ensures: true;\n\
             body: x;\n\
             trusted atom main(y: i64) -> i64\n\
             requires: true;\n\
             ensures: true;\n\
             body: getx(y);\n",
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

    #[test]
    fn proof_hash_tracks_callee_effect_pre_and_effect_post() {
        let source = "atom f(x: i64) -> i64\n\
             effect_pre: { File: Closed };\n\
             effect_post: { File: Open };\n\
             requires: true;\n\
             ensures: result == x;\n\
             body: x;\n\
             atom g(x: i64) -> i64\n\
             requires: true;\n\
             ensures: result == x;\n\
             body: f(x);\n";
        let baseline = proof_hash_for(source, "g");
        let changed_pre = proof_hash_for(
            &source.replace("effect_pre: { File: Closed }", "effect_pre: { File: Open }"),
            "g",
        );
        let changed_post = proof_hash_for(
            &source.replace(
                "effect_post: { File: Open }",
                "effect_post: { File: Closed }",
            ),
            "g",
        );

        assert_ne!(baseline, changed_pre);
        assert_ne!(baseline, changed_post);
    }

    #[test]
    fn proof_hash_tracks_transitive_callee_effect_states() {
        let source = "atom f(x: i64) -> i64\n\
             effect_pre: { File: Closed };\n\
             requires: true;\n\
             ensures: result == x;\n\
             body: x;\n\
             atom h(x: i64) -> i64\n\
             requires: true;\n\
             ensures: result == x;\n\
             body: f(x);\n\
             atom g(x: i64) -> i64\n\
             requires: true;\n\
             ensures: result == x;\n\
             body: h(x);\n";
        let baseline = proof_hash_for(source, "g");
        let changed = proof_hash_for(
            &source.replace("effect_pre: { File: Closed }", "effect_pre: { File: Open }"),
            "g",
        );

        assert_ne!(baseline, changed);
    }

    #[test]
    fn proof_hash_tracks_own_effect_states_and_quantifiers() {
        let own_effect_source = "atom main(x: i64) -> i64\n\
             effects: [File];\n\
             effect_pre: { File: Closed };\n\
             effect_post: { File: Open };\n\
             requires: true;\n\
             ensures: result == x;\n\
             body: x;\n";
        let baseline = proof_hash_for(own_effect_source, "main");
        let changed_pre = proof_hash_for(
            &own_effect_source.replace("File: Closed", "File: Open"),
            "main",
        );
        let changed_post = proof_hash_for(
            &own_effect_source.replace(
                "effect_post: { File: Open }",
                "effect_post: { File: Closed }",
            ),
            "main",
        );
        assert_ne!(baseline, changed_pre);
        assert_ne!(baseline, changed_post);

        let quantified_source = "atom main(arr: [i64], n: i64) -> i64\n\
             requires: n >= 1 && len(arr) >= n && forall(i, 0, n, arr[i] > 0);\n\
             ensures: result > 0;\n\
             body: arr[0];\n";
        let quantified = proof_hash_for(quantified_source, "main");
        let changed_quantifier = proof_hash_for(
            &quantified_source.replace("arr[i] > 0", "arr[i] >= 0"),
            "main",
        );
        assert_ne!(quantified, changed_quantifier);
    }

    #[test]
    fn proof_hash_tracks_effect_state_declaration_order() {
        let source = "effect File\n\
             states: [Closed, Open];\n\
             transition open: Closed -> Open;\n\
             transition close: Open -> Closed;\n\
             atom main(x: i64) -> i64\n\
             effects: [File];\n\
             requires: true;\n\
             ensures: result == x;\n\
             body: { perform File.open(x); x };\n";
        let baseline = proof_hash_for(source, "main");
        let reordered = proof_hash_for(
            &source.replace("states: [Closed, Open]", "states: [Open, Closed]"),
            "main",
        );

        assert_ne!(baseline, reordered);
    }

    #[test]
    fn proof_hash_tracks_transition_declaration_order() {
        let source = "effect File\n\
             states: [Closed, Open, Ready];\n\
             transition advance: Closed -> Open;\n\
             transition advance: Closed -> Ready;\n\
             atom main(x: i64) -> i64\n\
             effects: [File];\n\
             requires: true;\n\
             ensures: result == x;\n\
             body: x;\n";
        let baseline = proof_hash_for(source, "main");
        let reordered = proof_hash_for(
            &source.replace(
                "transition advance: Closed -> Open;\ntransition advance: Closed -> Ready;",
                "transition advance: Closed -> Ready;\ntransition advance: Closed -> Open;",
            ),
            "main",
        );

        assert_ne!(baseline, reordered);
    }

    #[test]
    fn proof_hash_tracks_definitions_named_only_by_effect_state_contracts() {
        let sources = [
            (
                "effect File\n\
                 states: [Closed, Open];\n\
                 atom main(x: i64) -> i64\n\
                 effect_pre: { File: Open };\n\
                 requires: true;\n\
                 ensures: result == x;\n\
                 body: x;\n",
                "main",
            ),
            (
                "effect File\n\
                 states: [Closed, Open];\n\
                 atom main(x: i64) -> i64\n\
                 effect_post: { File: Open };\n\
                 requires: true;\n\
                 ensures: result == x;\n\
                 body: x;\n",
                "main",
            ),
            (
                "effect File\n\
                 states: [Closed, Open];\n\
                 atom f(x: i64) -> i64\n\
                 effect_pre: { File: Open };\n\
                 requires: true;\n\
                 ensures: result == x;\n\
                 body: x;\n\
                 atom main(x: i64) -> i64\n\
                 requires: true;\n\
                 ensures: result == x;\n\
                 body: f(x);\n",
                "main",
            ),
            (
                "effect File\n\
                 states: [Closed, Open];\n\
                 atom f(x: i64) -> i64\n\
                 effect_post: { File: Open };\n\
                 requires: true;\n\
                 ensures: result == x;\n\
                 body: x;\n\
                 atom main(x: i64) -> i64\n\
                 requires: true;\n\
                 ensures: result == x;\n\
                 body: f(x);\n",
                "main",
            ),
        ];

        for (source, atom_name) in sources {
            let baseline = proof_hash_for(source, atom_name);
            let changed = proof_hash_for(
                &source.replace("states: [Closed, Open]", "states: [Closed, Ready]"),
                atom_name,
            );
            assert_ne!(baseline, changed, "effect-state reference was not hashed");
        }
    }

    #[test]
    fn proof_hash_tracks_every_effect_definition_field() {
        let base = base_effect_def();
        let baseline = proof_hash_with_effect_def(base.clone());
        let mut changed_defs = Vec::new();

        let mut changed = base.clone();
        changed.name = "Changed".to_string();
        changed_defs.push(("name", changed));

        let mut changed = base.clone();
        changed.params[0].name = "resource".to_string();
        changed_defs.push(("param name", changed));

        let mut changed = base.clone();
        changed.params[0].type_name = "Path".to_string();
        changed_defs.push(("param type", changed));

        let mut changed = base.clone();
        changed.constraint = Some("path == \"/tmp\"".to_string());
        changed_defs.push(("constraint", changed));

        let mut changed = base.clone();
        changed.includes = vec!["OtherLeaf".to_string()];
        changed_defs.push(("includes", changed));

        let mut changed = base.clone();
        changed.refinement = Some("path == \"/tmp\"".to_string());
        changed_defs.push(("refinement", changed));

        let mut changed = base.clone();
        changed.parent = vec!["OtherParent".to_string()];
        changed_defs.push(("parent", changed));

        let mut changed = base.clone();
        changed.states[0] = "Ready".to_string();
        changed_defs.push(("states", changed));

        let mut changed = base.clone();
        changed.transitions[0].operation = "finish".to_string();
        changed_defs.push(("transition operation", changed));

        let mut changed = base.clone();
        changed.transitions[0].from_state = "Busy".to_string();
        changed_defs.push(("transition from state", changed));

        let mut changed = base.clone();
        changed.transitions[0].to_state = "Idle".to_string();
        changed_defs.push(("transition to state", changed));

        let mut changed = base.clone();
        changed.initial_state = Some("Busy".to_string());
        changed_defs.push(("initial state", changed));

        for (field, changed) in changed_defs {
            assert_ne!(
                baseline,
                proof_hash_with_effect_def(changed),
                "changing effect definition {field} must change the proof hash"
            );
        }

        let mut changed = base.clone();
        changed.constraint = None;
        assert_ne!(baseline, proof_hash_with_effect_def(changed));
        let mut changed = base.clone();
        changed.refinement = None;
        assert_ne!(baseline, proof_hash_with_effect_def(changed));
        let mut changed = base.clone();
        changed.initial_state = None;
        assert_ne!(baseline, proof_hash_with_effect_def(changed));
    }

    #[test]
    fn proof_hash_tracks_effect_definition_include_and_parent_closure() {
        let mut work = base_effect_def();
        work.includes = vec!["Leaf".to_string()];
        work.parent.clear();

        let mut leaf = base_effect_def();
        leaf.name = "Leaf".to_string();
        leaf.includes.clear();
        leaf.parent = vec!["Parent".to_string()];

        let mut parent = base_effect_def();
        parent.name = "Parent".to_string();
        parent.includes.clear();
        parent.parent.clear();
        parent.states = vec!["Idle".to_string()];

        let baseline =
            proof_hash_with_effect_defs(vec![work.clone(), leaf.clone(), parent.clone()], "Work");
        parent.states = vec!["Ready".to_string()];
        let changed_parent = proof_hash_with_effect_defs(vec![work, leaf, parent], "Work");
        assert_ne!(baseline, changed_parent);
    }

    #[test]
    fn proof_hash_tracks_effect_definitions_of_transitive_callees() {
        let source = "effect Work\n\
                 states: [Idle, Busy];\n\
                 initial: Idle;\n\
                 transition start: Idle -> Busy;\n\
             atom f(x: i64) -> i64\n\
                 effects: [Work];\n\
                 requires: true;\n\
                 ensures: result == x;\n\
                 body: x;\n\
             atom h(x: i64) -> i64\n\
                 requires: true;\n\
                 ensures: result == x;\n\
                 body: f(x);\n\
             atom g(x: i64) -> i64\n\
                 requires: true;\n\
                 ensures: result == x;\n\
                 body: h(x);\n";
        let baseline = proof_hash_for(source, "g");
        let changed = proof_hash_for(
            &source.replace(
                "transition start: Idle -> Busy;",
                "transition start: Idle -> Idle;",
            ),
            "g",
        );
        assert_ne!(baseline, changed);
    }

    #[test]
    fn effect_free_proof_hash_matches_develop() {
        let hash = proof_hash_for(
            "atom main(x: i64) -> i64\n\
             requires: true;\n\
             ensures: result == x;\n\
             body: x;\n",
            "main",
        );

        assert_eq!(
            hash,
            "9e1ad8675ce5b94972092795e1f7583c37747cf2ace33fb3a601f459de1631b3"
        );
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
