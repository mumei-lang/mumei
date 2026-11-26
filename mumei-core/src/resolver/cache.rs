use crate::ast::TypeRef;
use crate::parser::{parse_body_expr, Atom, Expr, Pattern, Stmt};
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
/// Version 18 encodes `u64` leaves as machine words (`BV(64)`, unsigned
/// comparisons and `udiv`/`urem`/`lshr`) instead of unbounded `Int`s: a
/// `u64` atom verified under version 17 proved wrap-sensitive clauses like
/// `a + 1 > a` that the compiled code cannot satisfy, so cached `u64`
/// verdicts must be re-derived.
/// Version 19 makes the int-model `/` truncate toward zero (the `sdiv`
/// the emitted IR uses — #781) instead of Z3's Euclidean `div`: a
/// clause like `-7 / 2 == -4` that verified under version 18 is now
/// refuted, so every int-model verdict must be re-derived.
/// Version 20 models tag-gated direct enum payload reads (`e.port`,
/// `b.e.port`, `e.p.inner.x`) as datatype selectors instead of flattened-key
/// or unconstrained values, so cached verdicts must be re-derived.
/// Version 21 materializes struct-valued named enum payload reads (`e.p`,
/// `e.p.inner`) as struct handles over datatype selectors when each owning tag
/// is proven, so cached verdicts must be re-derived.
/// Version 22 havocs stale binding-keyed struct projections when rebinding to
/// a value whose enum payload tag is unproven, so cached verdicts must be
/// re-derived.
/// Version 23 registers nested struct projections under fresh loop-havoced
/// handles, allowing invariant-backed nested aliases to use post-loop fields,
/// so cached verdicts must be re-derived.
/// Version 24 keeps array types out of canonical struct binding detection,
/// preventing array element fields from becoming struct projection slots, so
/// cached verdicts must be re-derived.
/// Version 33 rejects calls where later argument evaluation may mutate an
/// array root passed by an earlier argument, matching runtime buffer aliasing.
/// Version 35 uses recursive datatype selectors for scalar payload access and
/// rejects writes that traverse a recursive self field. Version 36 admits
/// finite enum and scalar-struct payloads on direct-self-recursive enums and
/// re-derives cached verdicts after fixed-width refined-predicate comparisons
/// preserve unsigned u64 semantics.
/// Version 37 checks fixed-width payload domains when constructing finite and
/// direct-self-recursive enum values, so cached verdicts must be re-derived.
/// Version 38 binds refined predicates to their declared operand and rejects
/// free identifiers, so cached verdicts must be re-derived.
/// Version 40 compares struct-array equality through its leaf arrays and
/// includes the carrier in loop havoc, so cached verdicts must be re-derived.
/// Version 41 havocs every struct-array leaf after a potentially writing call,
/// so cached verdicts must be re-derived.
/// Version 42 represents struct-array enum payloads with leaf arrays and rejects
/// direct reads, so cached verdicts must be re-derived.
/// Version 43 admits bounded, inhabited mutual-recursive enum groups as Z3 datatypes.
/// Version 44 preserves explicit comparison grouping and rejects nested middle-chain comparisons.
pub const VERIFIER_POLICY_VERSION: u32 = 44;

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
    // str_bound も含める（composite admission の変更を検出）
    if let Some(bound) = atom.str_bound {
        hasher.update(b"|str_bound:");
        hasher.update(bound.to_string().as_bytes());
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
            hash_resource_definition(&mut hasher, resource);
        }
    }
    for resource_name in body_acquired_resource_names(atom) {
        if atom.resources.contains(&resource_name) {
            continue;
        }
        hasher.update(b"|acquired_resource:");
        hasher.update(resource_name.as_bytes());
        if let Some(resource) = module_env.get_resource(&resource_name) {
            hash_resource_definition(&mut hasher, resource);
        }
    }
    for resource_name in module_env.resources.keys().collect::<BTreeSet<_>>() {
        if let Some(resource) = module_env.get_resource(resource_name) {
            hasher.update(b"|module_resource:");
            hasher.update(resource_name.as_bytes());
            hash_resource_definition(&mut hasher, resource);
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
    // Enum declarations affect verdicts the atom text does not capture —
    // match exhaustiveness, variant ownership and, for generic enums,
    // which `E<...>` instances exist (constructor/pattern disambiguation
    // is sensitive to the instance set). Hash the whole enum environment
    // so an identical atom in a different enum context cannot replay a
    // stale verdict.
    let mut enum_names: Vec<&String> = module_env.enums.keys().collect();
    enum_names.sort();
    for name in enum_names {
        hash_enum_contract(&mut hasher, name, module_env);
    }
    let mut refined_type_names: Vec<&String> = module_env.types.keys().collect();
    refined_type_names.sort();
    for name in refined_type_names {
        if let Some(refined) = module_env.types.get(name) {
            hasher.update(b"|type_def:");
            hasher.update(name.as_bytes());
            hasher.update(b"|");
            hash_field(&mut hasher, "type_def.operand", &refined.operand);
            hash_field(&mut hasher, "type_def.base", &refined._base_type);
            hash_field(&mut hasher, "type_def.predicate", &refined.predicate_raw);
        }
    }

    // 3. Include callee signatures (transitive dependencies)
    let mut visited = HashSet::new();
    let mut stack = Vec::new();

    // Collect direct callees (sorted for deterministic hashing)
    let mut effect_names = atom_effect_names(atom);
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
            hash_trait_method_constraints(&mut hasher, callee_atom, module_env);
            hash_string_map(&mut hasher, "effect_pre", &callee_atom.effect_pre);
            hash_string_map(&mut hasher, "effect_post", &callee_atom.effect_post);
            effect_names.extend(atom_effect_names(callee_atom));
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

    for name in collect_effect_def_closure(effect_names, module_env) {
        if let Some(effect_def) = module_env.effect_defs.get(&name) {
            hash_effect_def(&mut hasher, effect_def);
        }
    }

    format!("{:x}", hasher.finalize())
}

fn atom_effect_names(atom: &Atom) -> BTreeSet<String> {
    atom.effects
        .iter()
        .map(|effect| effect.name.clone())
        .chain(atom.effect_pre.keys().cloned())
        .chain(atom.effect_post.keys().cloned())
        .collect()
}

fn collect_effect_def_closure(
    names: impl IntoIterator<Item = String>,
    module_env: &ModuleEnv,
) -> BTreeSet<String> {
    let mut effect_def_names = BTreeSet::new();
    let mut effect_def_stack: Vec<String> = names.into_iter().collect();
    while let Some(name) = effect_def_stack.pop() {
        if !effect_def_names.insert(name.clone()) {
            continue;
        }
        if let Some(effect_def) = module_env.effect_defs.get(&name) {
            effect_def_stack.extend(effect_def.includes.iter().cloned());
            effect_def_stack.extend(effect_def.parent.iter().cloned());
        }
    }
    effect_def_names
}

fn hash_effect_def(hasher: &mut Sha256, effect_def: &crate::parser::ast::EffectDef) {
    hasher.update(b"|effect_def:");
    hash_field(hasher, "effect_def.name", &effect_def.name);
    hash_field(
        hasher,
        "effect_def.params.len",
        &effect_def.params.len().to_string(),
    );
    for param in &effect_def.params {
        hash_field(hasher, "effect_def.param.name", &param.name);
        hash_field(hasher, "effect_def.param.type", &param.type_name);
    }
    hash_field(
        hasher,
        "effect_def.constraint.present",
        if effect_def.constraint.is_some() {
            "true"
        } else {
            "false"
        },
    );
    if let Some(constraint) = &effect_def.constraint {
        hash_field(hasher, "effect_def.constraint", constraint);
    }
    let mut includes = effect_def.includes.clone();
    includes.sort();
    hash_field(
        hasher,
        "effect_def.includes.len",
        &includes.len().to_string(),
    );
    for included in includes {
        hash_field(hasher, "effect_def.include", &included);
    }
    hash_field(
        hasher,
        "effect_def.refinement.present",
        if effect_def.refinement.is_some() {
            "true"
        } else {
            "false"
        },
    );
    if let Some(refinement) = &effect_def.refinement {
        hash_field(hasher, "effect_def.refinement", refinement);
    }
    let mut parents = effect_def.parent.clone();
    parents.sort();
    hash_field(hasher, "effect_def.parents.len", &parents.len().to_string());
    for parent in parents {
        hash_field(hasher, "effect_def.parent", &parent);
    }
    hash_field(
        hasher,
        "effect_def.states.len",
        &effect_def.states.len().to_string(),
    );
    for state in &effect_def.states {
        hash_field(hasher, "effect_def.state", state);
    }
    hash_field(
        hasher,
        "effect_def.transitions.len",
        &effect_def.transitions.len().to_string(),
    );
    for transition in &effect_def.transitions {
        hash_field(
            hasher,
            "effect_def.transition.operation",
            &transition.operation,
        );
        hash_field(hasher, "effect_def.transition.from", &transition.from_state);
        hash_field(hasher, "effect_def.transition.to", &transition.to_state);
    }
    hash_field(
        hasher,
        "effect_def.initial_state.present",
        if effect_def.initial_state.is_some() {
            "true"
        } else {
            "false"
        },
    );
    if let Some(initial_state) = &effect_def.initial_state {
        hash_field(hasher, "effect_def.initial_state", initial_state);
    }
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

fn hash_resource_definition(hasher: &mut Sha256, resource: &crate::parser::ResourceDef) {
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
        hasher.update(crate::verification::support::expr_to_source_string(invariant).as_bytes());
    }
}

fn hash_trait_method_constraints(hasher: &mut Sha256, callee: &Atom, module_env: &ModuleEnv) {
    let callee_type = callee
        .params
        .first()
        .and_then(|param| param.type_name.as_deref())
        .unwrap_or("i64");
    let Some((trait_name, trait_method)) = module_env
        .get_traits_for_method(&callee.name)
        .into_iter()
        .find(|(trait_name, _)| module_env.find_impl(trait_name, callee_type).is_some())
    else {
        return;
    };
    if !trait_method.param_constraints.iter().any(Option::is_some) {
        return;
    }

    hasher.update(b"|trait_method_constraint:");
    hash_field(hasher, "trait_method.callee", &callee.name);
    hash_field(hasher, "trait_method.trait", trait_name);
    for (index, constraint) in trait_method.param_constraints.iter().enumerate() {
        if let Some(constraint) = constraint {
            hash_field(hasher, "trait_method.constraint.index", &index.to_string());
            hash_field(hasher, "trait_method.constraint.value", constraint);
        }
    }
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

pub(crate) fn resource_dependency_hash(name: &str, module_env: &ModuleEnv) -> Option<String> {
    let resource = module_env.get_resource(name)?;
    let mut hasher = Sha256::new();
    hasher.update(b"|resource:");
    hasher.update(name.as_bytes());
    hash_resource_definition(&mut hasher, resource);
    Some(format!("{:x}", hasher.finalize()))
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
    let mut effect_names = if legacy_names {
        BTreeSet::new()
    } else {
        atom_effect_names(atom)
    };
    let atom_type_params = atom.type_params.iter().cloned().collect::<HashSet<_>>();
    let mut pending_type_names = Vec::new();
    let mut resource_names = BTreeSet::new();
    if !legacy_names {
        resource_names.extend(atom.resources.iter().cloned());
        collect_atom_signature_type_names(atom, &mut pending_type_names);
        let body = parse_body_expr(&atom.body_expr);
        collect_body_type_names_stmt(
            &body,
            module_env,
            &atom_type_params,
            &mut pending_type_names,
        );
        collect_acquired_resource_names_stmt(&body, &mut resource_names);
        let source_file = atom.spec_metadata.get("source_file");
        let imported_files =
            source_file.and_then(|source_file| module_env.import_closure.get(source_file));
        if let (Some(source_file), Some(imported_files)) = (source_file, imported_files) {
            for (name, origin) in &module_env.resource_origins {
                if origin
                    .as_deref()
                    .is_some_and(|origin| origin == source_file || imported_files.contains(origin))
                {
                    resource_names.insert(name.clone());
                }
            }
        } else {
            // Top-level atoms are source-attributed too; only imported files have tracked closures.
            resource_names.extend(module_env.resources.keys().cloned());
        }
    }
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
        if !legacy_names {
            effect_names.extend(atom_effect_names(callee_atom));
            collect_atom_signature_type_names(callee_atom, &mut pending_type_names);
        }
        pending.extend(
            dependency_callees(callee_atom, module_env, legacy_names)
                .into_iter()
                .filter(|name| !visited.contains(name)),
        );
    }

    if !legacy_names {
        for name in collect_effect_def_closure(effect_names, module_env) {
            if let Some(effect_def) = module_env.effect_defs.get(&name) {
                let mut hasher = Sha256::new();
                hash_effect_def(&mut hasher, effect_def);
                hashes.insert(format!("effect:{name}"), format!("{:x}", hasher.finalize()));
            }
        }
    }

    let mut type_names = BTreeSet::new();
    let mut struct_names = BTreeSet::new();
    let mut enum_names = BTreeSet::new();
    if legacy_names {
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
            if module_env.enums.contains_key(bare_name) {
                enum_names.insert(bare_name.to_string());
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
                let bare_name = type_ref
                    .name
                    .split('<')
                    .next()
                    .unwrap_or(&type_ref.name)
                    .trim();
                if module_env.enums.contains_key(bare_name) {
                    enum_names.insert(bare_name.to_string());
                }
            }
        }
    } else {
        // Same origin widening as resources: module-env-wide checks run for
        // every atom — every refined type's constraint is SAT-checked and every
        // struct's flattened projection paths are collision-checked — so an
        // unreferenced definition's change can flip this atom's verdict. Seed
        // the type-name walk with definitions whose origin is the atom's own
        // file or a transitively imported file; unattributed environments stay
        // conservative and seed every definition. The walk then pulls in the
        // definitions these reach transitively (e.g. enums nested in a
        // struct's fields).
        let source_file = atom.spec_metadata.get("source_file");
        let imported_files =
            source_file.and_then(|source_file| module_env.import_closure.get(source_file));
        // Monomorphized instance names ('Opt<i64>') never appear in a module's
        // raw items, so an importing environment cannot recompute their keys;
        // raw declarations never contain '<'.
        let is_monomorphized_instance = |name: &String| name.contains('<');
        if let (Some(source_file), Some(imported_files)) = (source_file, imported_files) {
            let within_closure = |origin: &Option<String>| {
                origin
                    .as_deref()
                    .is_some_and(|origin| origin == source_file || imported_files.contains(origin))
            };
            pending_type_names.extend(
                module_env
                    .type_origins
                    .iter()
                    .filter(|(name, origin)| {
                        within_closure(origin) && !is_monomorphized_instance(name)
                    })
                    .map(|(name, _)| name.clone()),
            );
            pending_type_names.extend(
                module_env
                    .struct_origins
                    .iter()
                    .filter(|(name, origin)| {
                        within_closure(origin) && !is_monomorphized_instance(name)
                    })
                    .map(|(name, _)| name.clone()),
            );
        } else {
            pending_type_names.extend(
                module_env
                    .types
                    .keys()
                    .filter(|name| !is_monomorphized_instance(name))
                    .cloned(),
            );
            pending_type_names.extend(
                module_env
                    .structs
                    .keys()
                    .filter(|name| !is_monomorphized_instance(name))
                    .cloned(),
            );
        }

        let empty_bound_params = HashSet::new();
        let mut visited_type_names = HashSet::new();
        while let Some(name) = pending_type_names.pop() {
            if name == "Self" || !visited_type_names.insert(name.clone()) {
                continue;
            }
            if let Some(refined) = module_env.get_type(&name) {
                type_names.insert(name.clone());
                let base_name = refined
                    ._base_type
                    .split('<')
                    .next()
                    .unwrap_or(&refined._base_type)
                    .trim();
                if module_env.get_struct(base_name).is_some()
                    || module_env.enums.contains_key(base_name)
                {
                    collect_signature_type_names(
                        &refined._base_type,
                        &mut pending_type_names,
                        &empty_bound_params,
                    );
                }
            }
            if let Some(struct_def) = module_env.get_struct(&name) {
                struct_names.insert(name.clone());
                let bound_params = struct_def
                    .type_params
                    .iter()
                    .cloned()
                    .collect::<HashSet<_>>();
                for field in &struct_def.fields {
                    collect_signature_type_names(
                        &field.type_name,
                        &mut pending_type_names,
                        &bound_params,
                    );
                    collect_signature_type_ref_names(
                        &field.type_ref,
                        &mut pending_type_names,
                        &bound_params,
                    );
                }
            }
            if let Some(enum_def) = module_env.enums.get(&name) {
                enum_names.insert(name.clone());
                let bound_params = enum_def.type_params.iter().cloned().collect::<HashSet<_>>();
                for variant in &enum_def.variants {
                    for field_type in &variant.field_types {
                        collect_signature_type_ref_names(
                            field_type,
                            &mut pending_type_names,
                            &bound_params,
                        );
                    }
                }
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
    if !legacy_names {
        for name in enum_names {
            let mut hasher = Sha256::new();
            hash_enum_contract(&mut hasher, &name, module_env);
            if let Some(enum_def) = module_env.enums.get(&name) {
                for variant in &enum_def.variants {
                    hasher.update(b"|fields:");
                    hasher.update(variant.name.as_bytes());
                    for field in &variant.fields {
                        hasher.update(b":");
                        hasher.update(field.as_bytes());
                    }
                }
            }
            if let Some(unit) = module_env.unit_of_type(&name) {
                hasher.update(b"|unit:");
                hasher.update(unit.as_bytes());
            }
            hashes.insert(format!("enum:{name}"), format!("{:x}", hasher.finalize()));
        }
        for name in resource_names {
            if let Some(hash) = resource_dependency_hash(&name, module_env) {
                hashes.insert(format!("resource:{name}"), hash);
            }
        }
    }

    hashes
}

fn collect_signature_type_names(
    type_name: &str,
    names: &mut Vec<String>,
    bound_params: &HashSet<String>,
) {
    let type_name = type_name.trim();
    if type_name.is_empty() {
        return;
    }

    let bare_name = type_name.split('<').next().unwrap_or(type_name).trim();
    if !bare_name.is_empty() && !bound_params.contains(bare_name) {
        names.push(bare_name.to_string());
    }

    let Some(open) = type_name.find('<') else {
        return;
    };
    let Some(close) = type_name.rfind('>') else {
        return;
    };
    if close <= open {
        return;
    }

    let arguments = &type_name[open + 1..close];
    let mut argument_start = 0;
    let mut nesting = 0usize;
    for (index, ch) in arguments.char_indices() {
        match ch {
            '<' => nesting += 1,
            '>' => nesting = nesting.saturating_sub(1),
            ',' if nesting == 0 => {
                collect_signature_type_names(
                    &arguments[argument_start..index],
                    names,
                    bound_params,
                );
                argument_start = index + 1;
            }
            _ => {}
        }
    }
    collect_signature_type_names(&arguments[argument_start..], names, bound_params);
}

fn collect_signature_type_ref_names(
    type_ref: &TypeRef,
    names: &mut Vec<String>,
    bound_params: &HashSet<String>,
) {
    collect_signature_type_names(&type_ref.name, names, bound_params);
    for type_arg in &type_ref.type_args {
        collect_signature_type_ref_names(type_arg, names, bound_params);
    }
}

fn collect_atom_signature_type_names(atom: &Atom, names: &mut Vec<String>) {
    let bound_params = atom.type_params.iter().cloned().collect::<HashSet<_>>();
    for type_name in atom
        .params
        .iter()
        .filter_map(|param| param.type_name.as_deref())
        .chain(atom.return_type.as_deref())
    {
        collect_signature_type_names(type_name, names, &bound_params);
    }
    for param in &atom.params {
        if let Some(type_ref) = &param.type_ref {
            collect_signature_type_ref_names(type_ref, names, &bound_params);
        }
    }
}

fn collect_body_type_names_stmt(
    stmt: &Stmt,
    module_env: &ModuleEnv,
    bound_params: &HashSet<String>,
    names: &mut Vec<String>,
) {
    match stmt {
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            collect_body_type_names_expr(value, module_env, bound_params, names);
        }
        Stmt::ArrayStore { index, value, .. } => {
            collect_body_type_names_expr(index, module_env, bound_params, names);
            collect_body_type_names_expr(value, module_env, bound_params, names);
        }
        Stmt::Block(stmts, _) => {
            for stmt in stmts {
                collect_body_type_names_stmt(stmt, module_env, bound_params, names);
            }
        }
        Stmt::While {
            cond,
            invariant,
            decreases,
            body,
            ..
        } => {
            collect_body_type_names_expr(cond, module_env, bound_params, names);
            collect_body_type_names_expr(invariant, module_env, bound_params, names);
            if let Some(decreases) = decreases {
                collect_body_type_names_expr(decreases, module_env, bound_params, names);
            }
            collect_body_type_names_stmt(body, module_env, bound_params, names);
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
            collect_body_type_names_stmt(body, module_env, bound_params, names);
        }
        Stmt::TaskGroup { children, .. } => {
            for child in children {
                collect_body_type_names_stmt(child, module_env, bound_params, names);
            }
        }
        Stmt::Cancel { .. } => {}
        Stmt::Expr(expr, _) => {
            collect_body_type_names_expr(expr, module_env, bound_params, names);
        }
    }
}

fn collect_body_type_names_expr(
    expr: &Expr,
    module_env: &ModuleEnv,
    bound_params: &HashSet<String>,
    names: &mut Vec<String>,
) {
    match expr {
        Expr::Paren(inner) => collect_body_type_names_expr(inner, module_env, bound_params, names),
        Expr::Number(_)
        | Expr::Float(_)
        | Expr::StringLit(_)
        | Expr::Variable(_)
        | Expr::AtomRef { .. } => {}
        Expr::ArrayLit(elements) => {
            for element in elements {
                collect_body_type_names_expr(element, module_env, bound_params, names);
            }
        }
        Expr::ArrayAccess(_, index) => {
            collect_body_type_names_expr(index, module_env, bound_params, names);
        }
        Expr::BinaryOp(left, _, right) => {
            collect_body_type_names_expr(left, module_env, bound_params, names);
            collect_body_type_names_expr(right, module_env, bound_params, names);
        }
        Expr::Block(stmt) => collect_body_type_names_stmt(stmt, module_env, bound_params, names),
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_body_type_names_expr(cond, module_env, bound_params, names);
            collect_body_type_names_stmt(then_branch, module_env, bound_params, names);
            collect_body_type_names_stmt(else_branch, module_env, bound_params, names);
        }
        Expr::Call(name, args) => {
            collect_enum_variant_owner_names(name, module_env, names);
            for arg in args {
                collect_body_type_names_expr(arg, module_env, bound_params, names);
            }
        }
        Expr::StructInit { type_name, fields } => {
            collect_signature_type_names(type_name, names, bound_params);
            for (_, value) in fields {
                collect_body_type_names_expr(value, module_env, bound_params, names);
            }
        }
        Expr::FieldAccess(inner, _) => {
            collect_body_type_names_expr(inner, module_env, bound_params, names);
        }
        Expr::Match { target, arms } => {
            collect_body_type_names_expr(target, module_env, bound_params, names);
            for arm in arms {
                collect_body_type_names_pattern(&arm.pattern, module_env, names);
                if let Some(guard) = &arm.guard {
                    collect_body_type_names_expr(guard, module_env, bound_params, names);
                }
                collect_body_type_names_stmt(&arm.body, module_env, bound_params, names);
            }
        }
        Expr::Async { body } => collect_body_type_names_stmt(body, module_env, bound_params, names),
        Expr::Await { expr } => {
            collect_body_type_names_expr(expr, module_env, bound_params, names);
        }
        Expr::CallRef { callee, args } => {
            collect_body_type_names_expr(callee, module_env, bound_params, names);
            for arg in args {
                collect_body_type_names_expr(arg, module_env, bound_params, names);
            }
        }
        Expr::Perform { args, .. } => {
            for arg in args {
                collect_body_type_names_expr(arg, module_env, bound_params, names);
            }
        }
        Expr::Lambda { body, .. } => {
            collect_body_type_names_stmt(body, module_env, bound_params, names);
        }
        Expr::ChanSend { channel, value } => {
            collect_body_type_names_expr(channel, module_env, bound_params, names);
            collect_body_type_names_expr(value, module_env, bound_params, names);
        }
        Expr::ChanRecv { channel } => {
            collect_body_type_names_expr(channel, module_env, bound_params, names);
        }
    }
}

fn body_acquired_resource_names(atom: &Atom) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    collect_acquired_resource_names_stmt(&parse_body_expr(&atom.body_expr), &mut names);
    names
}

fn collect_acquired_resource_names_stmt(stmt: &Stmt, names: &mut BTreeSet<String>) {
    match stmt {
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            collect_acquired_resource_names_expr(value, names);
        }
        Stmt::ArrayStore { index, value, .. } => {
            collect_acquired_resource_names_expr(index, names);
            collect_acquired_resource_names_expr(value, names);
        }
        Stmt::Block(stmts, _) => {
            for stmt in stmts {
                collect_acquired_resource_names_stmt(stmt, names);
            }
        }
        Stmt::While {
            cond,
            invariant,
            decreases,
            body,
            ..
        } => {
            collect_acquired_resource_names_expr(cond, names);
            collect_acquired_resource_names_expr(invariant, names);
            if let Some(decreases) = decreases {
                collect_acquired_resource_names_expr(decreases, names);
            }
            collect_acquired_resource_names_stmt(body, names);
        }
        Stmt::Acquire { resource, body, .. } => {
            names.insert(resource.clone());
            collect_acquired_resource_names_stmt(body, names);
        }
        Stmt::Task { body, .. } => collect_acquired_resource_names_stmt(body, names),
        Stmt::TaskGroup { children, .. } => {
            for child in children {
                collect_acquired_resource_names_stmt(child, names);
            }
        }
        Stmt::Cancel { .. } => {}
        Stmt::Expr(expr, _) => collect_acquired_resource_names_expr(expr, names),
    }
}

fn collect_acquired_resource_names_expr(expr: &Expr, names: &mut BTreeSet<String>) {
    match expr {
        Expr::Paren(inner) => collect_acquired_resource_names_expr(inner, names),
        Expr::Number(_)
        | Expr::Float(_)
        | Expr::StringLit(_)
        | Expr::Variable(_)
        | Expr::AtomRef { .. } => {}
        Expr::ArrayLit(elements) => {
            for element in elements {
                collect_acquired_resource_names_expr(element, names);
            }
        }
        Expr::ArrayAccess(_, index) => collect_acquired_resource_names_expr(index, names),
        Expr::BinaryOp(left, _, right) => {
            collect_acquired_resource_names_expr(left, names);
            collect_acquired_resource_names_expr(right, names);
        }
        Expr::Block(stmt) => collect_acquired_resource_names_stmt(stmt, names),
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_acquired_resource_names_expr(cond, names);
            collect_acquired_resource_names_stmt(then_branch, names);
            collect_acquired_resource_names_stmt(else_branch, names);
        }
        Expr::Call(_, args) => {
            for arg in args {
                collect_acquired_resource_names_expr(arg, names);
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_acquired_resource_names_expr(value, names);
            }
        }
        Expr::FieldAccess(inner, _) => collect_acquired_resource_names_expr(inner, names),
        Expr::Match { target, arms } => {
            collect_acquired_resource_names_expr(target, names);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_acquired_resource_names_expr(guard, names);
                }
                collect_acquired_resource_names_stmt(&arm.body, names);
            }
        }
        Expr::Async { body } | Expr::Lambda { body, .. } => {
            collect_acquired_resource_names_stmt(body, names);
        }
        Expr::Await { expr } => collect_acquired_resource_names_expr(expr, names),
        Expr::CallRef { callee, args } => {
            collect_acquired_resource_names_expr(callee, names);
            for arg in args {
                collect_acquired_resource_names_expr(arg, names);
            }
        }
        Expr::Perform { args, .. } => {
            for arg in args {
                collect_acquired_resource_names_expr(arg, names);
            }
        }
        Expr::ChanSend { channel, value } => {
            collect_acquired_resource_names_expr(channel, names);
            collect_acquired_resource_names_expr(value, names);
        }
        Expr::ChanRecv { channel } => collect_acquired_resource_names_expr(channel, names),
    }
}

fn collect_body_type_names_pattern(
    pattern: &Pattern,
    module_env: &ModuleEnv,
    names: &mut Vec<String>,
) {
    match pattern {
        Pattern::Variant {
            variant_name,
            fields,
        } => {
            collect_enum_variant_owner_names(variant_name, module_env, names);
            for field in fields {
                collect_body_type_names_pattern(field, module_env, names);
            }
        }
        Pattern::Wildcard | Pattern::Literal(_) | Pattern::Variable(_) => {}
    }
}

fn collect_enum_variant_owner_names(
    variant_name: &str,
    module_env: &ModuleEnv,
    names: &mut Vec<String>,
) {
    if let Some((enum_name, _)) = variant_name.rsplit_once("::") {
        let enum_name = enum_name.split('<').next().unwrap_or(enum_name).trim();
        if module_env.enums.contains_key(enum_name) {
            names.push(enum_name.to_string());
        }
        return;
    }

    let bare_variant = variant_name.rsplit("::").next().unwrap_or(variant_name);
    for (enum_name, enum_def) in &module_env.enums {
        if enum_def
            .variants
            .iter()
            .any(|variant| variant.name == bare_variant)
        {
            names.push(enum_name.clone());
        }
    }
}

fn compute_dependency_hash_for_callee(atom: &Atom, module_env: &ModuleEnv) -> String {
    let mut hasher = Sha256::new();
    hash_callee_contract(&mut hasher, atom, module_env, false);
    hash_trait_method_constraints(&mut hasher, atom, module_env);
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
            hasher.update(b":resolved:");
            hasher.update(module_env.resolve_base_type(&f.type_name).as_bytes());
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

fn hash_enum_contract(hasher: &mut Sha256, name: &str, module_env: &ModuleEnv) {
    hasher.update(b"|enum:");
    hasher.update(name.as_bytes());
    if let Some(enum_def) = module_env.enums.get(name) {
        for variant in &enum_def.variants {
            hasher.update(b"|variant:");
            hasher.update(variant.name.as_bytes());
            for field_type in &variant.field_types {
                hasher.update(b":");
                hasher.update(field_type.name.as_bytes());
            }
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
        dependency_hashes_for(source, root_name)
            .remove(dependency_key)
            .unwrap()
    }

    fn dependency_hashes_for(source: &str, root_name: &str) -> BTreeMap<String, String> {
        let items = parse_module(source);
        let mut module_env = ModuleEnv::default();
        for item in items {
            match item {
                Item::Atom(atom) => {
                    module_env.atoms.insert(atom.name.clone(), atom);
                }
                Item::TypeDef(refined) => module_env.register_type(&refined),
                Item::ResourceDef(resource) => module_env.register_resource(&resource),
                _ => {}
            }
        }
        let root = module_env.atoms.get(root_name).unwrap().clone();
        compute_dependency_hashes(&root, &module_env)
    }

    fn trait_dependency_and_proof_hashes(
        source: &str,
        root_name: &str,
    ) -> (String, BTreeMap<String, String>) {
        let items = parse_module(source);
        let mut module_env = ModuleEnv::default();
        for item in items {
            match item {
                Item::Atom(atom) => {
                    module_env
                        .dependency_graph
                        .insert(atom.name.clone(), collect_callees_from_atom(&atom));
                    module_env.register_atom(&atom);
                }
                Item::TraitDef(trait_def) => module_env.register_trait(&trait_def),
                Item::ImplDef(impl_def) => module_env.register_impl(&impl_def),
                _ => {}
            }
        }
        let root = module_env.get_atom(root_name).unwrap().clone();
        (
            compute_proof_hash(&root, &module_env),
            compute_dependency_hashes(&root, &module_env),
        )
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

    fn dependency_hashes_with_effect_defs(
        source: &str,
        root_name: &str,
        effect_defs: Vec<EffectDef>,
    ) -> BTreeMap<String, String> {
        let items = parse_module(source);
        let mut module_env = ModuleEnv::default();
        for item in items {
            match item {
                Item::Atom(atom) => module_env.register_atom(&atom),
                Item::TypeDef(refined) => module_env.register_type(&refined),
                Item::EffectDef(effect_def) => module_env.register_effect(&effect_def),
                _ => {}
            }
        }
        for effect_def in effect_defs {
            module_env.register_effect(&effect_def);
        }
        let root = module_env.get_atom(root_name).unwrap().clone();
        compute_dependency_hashes(&root, &module_env)
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

    fn bare_effect_def(name: &str) -> EffectDef {
        let mut effect_def = base_effect_def();
        effect_def.name = name.to_string();
        effect_def.params.clear();
        effect_def.constraint = None;
        effect_def.includes.clear();
        effect_def.refinement = None;
        effect_def.parent.clear();
        effect_def.states.clear();
        effect_def.transitions.clear();
        effect_def.initial_state = None;
        effect_def
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
    fn trait_method_parameter_constraints_participate_in_freshness_hashes() {
        let source = "trait Picker { fn pick(a: Self where v >= 0) -> Self; }\n\
                      impl Picker for i64 { fn pick(a: i64) -> i64 { a } }\n\
                      atom pick(a: i64) -> i64\n\
                      requires: true;\n\
                      ensures: true;\n\
                      body: a;\n\
                      atom g(x: i64) -> i64\n\
                      requires: x >= 0;\n\
                      ensures: true;\n\
                      body: pick(x);\n";
        let changed_source = source.replace("v >= 0", "v >= 5");
        let (proof_hash, dependency_hashes) = trait_dependency_and_proof_hashes(source, "g");
        let (changed_proof_hash, changed_dependency_hashes) =
            trait_dependency_and_proof_hashes(&changed_source, "g");

        assert_ne!(proof_hash, changed_proof_hash);
        assert_ne!(
            dependency_hashes["atom:pick"],
            changed_dependency_hashes["atom:pick"]
        );
    }

    #[test]
    fn dependency_hashes_include_caller_callee_and_effect_definition_closure() {
        let source = "atom callee(x: i64) -> i64\n\
             effects: [CalleeWork];\n\
             effect_pre: { CalleePre: Idle };\n\
             requires: true;\n\
             ensures: result == x;\n\
             body: x;\n\
             atom root(x: i64) -> i64\n\
             effects: [RootWork];\n\
             effect_pre: { CallerPre: Idle };\n\
             effect_post: { CallerPost: Done };\n\
             requires: true;\n\
             ensures: result == x;\n\
             body: callee(x);\n";
        let mut root_effect = bare_effect_def("RootWork");
        root_effect.includes = vec!["IncludeOne".to_string()];
        root_effect.parent = vec!["ParentOne".to_string()];
        let mut include_one = bare_effect_def("IncludeOne");
        include_one.includes = vec!["IncludeDeep".to_string()];
        let mut parent_one = bare_effect_def("ParentOne");
        parent_one.parent = vec!["ParentDeep".to_string()];
        let hashes = dependency_hashes_with_effect_defs(
            source,
            "root",
            vec![
                bare_effect_def("CalleeWork"),
                bare_effect_def("CalleePre"),
                bare_effect_def("CallerPre"),
                bare_effect_def("CallerPost"),
                root_effect,
                include_one,
                bare_effect_def("IncludeDeep"),
                parent_one,
                bare_effect_def("ParentDeep"),
            ],
        );

        let effect_keys = hashes
            .keys()
            .filter(|key| key.starts_with("effect:"))
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            effect_keys,
            vec![
                "effect:CalleePre",
                "effect:CalleeWork",
                "effect:CallerPost",
                "effect:CallerPre",
                "effect:IncludeDeep",
                "effect:IncludeOne",
                "effect:ParentDeep",
                "effect:ParentOne",
                "effect:RootWork",
            ]
        );
        assert!(hashes.contains_key("atom:callee"));
    }

    #[test]
    fn dependency_hashes_ignore_unreferenced_effect_definitions() {
        let source = "atom callee(x: i64) -> i64\n\
             requires: true;\n\
             ensures: result == x;\n\
             body: x;\n\
             atom root(x: i64) -> i64\n\
             requires: true;\n\
             ensures: result == x;\n\
             body: callee(x);\n";
        let baseline =
            dependency_hashes_with_effect_defs(source, "root", vec![bare_effect_def("Orphan")]);
        let mut changed_orphan = bare_effect_def("Orphan");
        changed_orphan.states.push("Changed".to_string());
        let changed = dependency_hashes_with_effect_defs(source, "root", vec![changed_orphan]);

        assert_eq!(baseline, changed);
        assert!(baseline.keys().all(|key| !key.starts_with("effect:")));
    }

    #[test]
    fn dependency_hash_tracks_every_hashed_effect_definition_field() {
        let source = "atom root(x: i64) -> i64\n\
             effects: [Work];\n\
             requires: true;\n\
             ensures: result == x;\n\
             body: x;\n";
        let base = base_effect_def();
        let baseline = dependency_hashes_with_effect_defs(source, "root", vec![base.clone()]);
        assert_eq!(
            baseline.keys().cloned().collect::<Vec<_>>(),
            vec!["effect:Work"]
        );

        let mut changed_defs = Vec::new();
        let mut changed = base.clone();
        changed.params[0].name = "resource".to_string();
        changed_defs.push(("param name", changed));
        let mut changed = base.clone();
        changed.params[0].type_name = "Path".to_string();
        changed_defs.push(("param type", changed));
        let mut changed = base.clone();
        changed.params.push(EffectDefParam {
            name: "mode".to_string(),
            type_name: "Mode".to_string(),
        });
        changed_defs.push(("param count", changed));
        let mut changed = base.clone();
        changed.constraint = Some("path == \"/tmp\"".to_string());
        changed_defs.push(("constraint", changed));
        let mut changed = base.clone();
        changed.constraint = None;
        changed_defs.push(("constraint presence", changed));
        let mut changed = base.clone();
        changed.includes = vec!["OtherLeaf".to_string()];
        changed_defs.push(("includes", changed));
        let mut changed = base.clone();
        changed.includes.push("OtherLeaf".to_string());
        changed_defs.push(("include count", changed));
        let mut changed = base.clone();
        changed.refinement = Some("path == \"/tmp\"".to_string());
        changed_defs.push(("refinement", changed));
        let mut changed = base.clone();
        changed.refinement = None;
        changed_defs.push(("refinement presence", changed));
        let mut changed = base.clone();
        changed.parent = vec!["OtherParent".to_string()];
        changed_defs.push(("parent", changed));
        let mut changed = base.clone();
        changed.parent.clear();
        changed_defs.push(("parent count", changed));
        let mut changed = base.clone();
        changed.states[0] = "Ready".to_string();
        changed_defs.push(("state", changed));
        let mut changed = base.clone();
        changed.states.push("Ready".to_string());
        changed_defs.push(("state count", changed));
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
        changed.transitions.push(EffectTransition {
            operation: "finish".to_string(),
            from_state: "Busy".to_string(),
            to_state: "Idle".to_string(),
        });
        changed_defs.push(("transition count", changed));
        let mut changed = base.clone();
        changed.initial_state = Some("Busy".to_string());
        changed_defs.push(("initial state", changed));
        let mut changed = base.clone();
        changed.initial_state = None;
        changed_defs.push(("initial state presence", changed));

        for (field, changed_def) in changed_defs {
            let changed = dependency_hashes_with_effect_defs(source, "root", vec![changed_def]);
            assert_eq!(
                baseline.keys().collect::<Vec<_>>(),
                changed.keys().collect::<Vec<_>>(),
                "changing effect definition {field} must keep the dependency keys stable"
            );
            assert_ne!(
                baseline["effect:Work"], changed["effect:Work"],
                "changing effect definition {field} must change only its effect hash"
            );
        }

        let mut renamed = base;
        renamed.name = "RenamedWork".to_string();
        let renamed_source = source.replace("[Work]", "[RenamedWork]");
        let renamed_hashes =
            dependency_hashes_with_effect_defs(&renamed_source, "root", vec![renamed]);
        assert_eq!(
            renamed_hashes.keys().cloned().collect::<Vec<_>>(),
            vec!["effect:RenamedWork"]
        );
        assert_ne!(
            baseline["effect:Work"],
            renamed_hashes["effect:RenamedWork"]
        );
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
    fn struct_field_alias_base_change_invalidates_the_proof_hash() {
        fn hash(source: &str) -> String {
            let items = parse_module(source);
            let mut module_env = ModuleEnv::default();
            let mut main = None;
            for item in items {
                match item {
                    Item::Atom(atom) => main = Some(atom),
                    Item::StructDef(sdef) => {
                        module_env.structs.insert(sdef.name.clone(), sdef);
                    }
                    Item::TypeDef(refined) => module_env.register_type(&refined),
                    _ => {}
                }
            }
            compute_proof_hash(&main.expect("main atom parsed"), &module_env)
        }

        let before = hash(
            "struct Inner { x: i64 }\n\
             type U = i64;\n\
             type T = U;\n\
             struct S { inner: T, inner_x: i64 }\n\
             atom main(s: S) -> i64\n\
             requires: true;\n\
             ensures: true;\n\
             body: 0;\n",
        );
        let after = hash(
            "struct Inner { x: i64 }\n\
             type U = Inner;\n\
             type T = U;\n\
             struct S { inner: T, inner_x: i64 }\n\
             atom main(s: S) -> i64\n\
             requires: true;\n\
             ensures: true;\n\
             body: 0;\n",
        );

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
    fn declared_resource_invariant_changes_its_dependency_hash() {
        let before = dependency_hash_for(
            "resource counter { value: i64 } priority: 1 mode: exclusive invariant: counter.value >= 0;\n\
             atom main() -> i64\n\
             resources: [counter];\n\
             requires: true;\n\
             ensures: true;\n\
             body: 0;",
            "main",
            "resource:counter",
        );
        let after = dependency_hash_for(
            "resource counter { value: i64 } priority: 1 mode: exclusive invariant: counter.value >= 5;\n\
             atom main() -> i64\n\
             resources: [counter];\n\
             requires: true;\n\
             ensures: true;\n\
             body: 0;",
            "main",
            "resource:counter",
        );
        assert_ne!(before, after);

        let acquired = dependency_hash_for(
            "resource counter { value: i64 } priority: 1 mode: exclusive invariant: counter.value >= 0;\n\
             atom main() -> i64\n\
             requires: true;\n\
             ensures: true;\n\
             body: { acquire counter { counter.value = 3; 0 } };",
            "main",
            "resource:counter",
        );
        assert_eq!(before, acquired);
    }

    #[test]
    fn module_resource_invariant_changes_an_unrelated_atom_hash() {
        let before_source = "resource counter { value: i64 } priority: 1 mode: exclusive invariant: counter.value >= 0;\n\
                             atom main(a: i64) -> i64\n\
                             requires: true;\n\
                             ensures: true;\n\
                             body: a;";
        let after_source = before_source.replace("counter.value >= 0", "counter.value >= 5");

        assert_ne!(env_and_hash(before_source), env_and_hash(&after_source));
        assert_ne!(
            dependency_hash_for(before_source, "main", "resource:counter"),
            dependency_hash_for(&after_source, "main", "resource:counter")
        );
    }

    #[test]
    fn resource_dependency_keys_follow_atom_source_import_closure() {
        let items = parse_module(
            "resource own { value: i64 } priority: 1 mode: exclusive invariant: own.value >= 0;\n\
             resource imported { value: i64 } priority: 1 mode: exclusive invariant: imported.value >= 0;\n\
             resource transitive { value: i64 } priority: 1 mode: exclusive invariant: transitive.value >= 0;\n\
             resource sibling { value: i64 } priority: 1 mode: exclusive invariant: sibling.value >= 0;\n\
             resource unknown { value: i64 } priority: 1 mode: exclusive invariant: unknown.value >= 0;",
        );
        let mut module_env = ModuleEnv::default();
        for item in items {
            if let Item::ResourceDef(resource) = item {
                let origin = match resource.name.as_str() {
                    "own" => "/c.mm",
                    "imported" => "/b.mm",
                    "transitive" => "/d.mm",
                    "sibling" => "/sibling.mm",
                    "unknown" => "/outside.mm",
                    _ => unreachable!("unexpected resource"),
                };
                module_env.register_resource_with_origin(&resource, Some(origin));
            }
        }
        module_env.import_closure.insert(
            "/c.mm".to_string(),
            ["/b.mm".to_string(), "/d.mm".to_string()]
                .into_iter()
                .collect(),
        );
        let mut atom = parse_module(
            "atom main() -> i64\n\
             requires: true;\n\
             ensures: true;\n\
             body: 0;",
        )
        .into_iter()
        .find_map(|item| match item {
            Item::Atom(atom) => Some(atom),
            _ => None,
        })
        .expect("parse main atom");
        atom.spec_metadata
            .insert("source_file".to_string(), "/c.mm".to_string());

        let hashes = compute_dependency_hashes(&atom, &module_env);
        assert!(hashes.contains_key("resource:own"));
        assert!(hashes.contains_key("resource:imported"));
        assert!(hashes.contains_key("resource:transitive"));
        assert!(!hashes.contains_key("resource:sibling"));
        assert!(!hashes.contains_key("resource:unknown"));

        let mut declared = atom.clone();
        declared.resources.push("sibling".to_string());
        assert!(compute_dependency_hashes(&declared, &module_env).contains_key("resource:sibling"));

        atom.spec_metadata.remove("source_file");
        let root_hashes = compute_dependency_hashes(&atom, &module_env);
        for name in ["own", "imported", "transitive", "sibling", "unknown"] {
            assert!(root_hashes.contains_key(&format!("resource:{name}")));
        }

        atom.spec_metadata
            .insert("source_file".to_string(), "/root.mm".to_string());
        let annotated_root_hashes = compute_dependency_hashes(&atom, &module_env);
        for name in ["own", "imported", "transitive", "sibling", "unknown"] {
            assert!(annotated_root_hashes.contains_key(&format!("resource:{name}")));
        }
    }

    #[test]
    fn body_acquired_resource_invariant_change_invalidates_the_proof_hash() {
        let before = env_and_hash(
            "resource counter { value: i64 } priority: 1 mode: exclusive invariant: counter.value >= 0;\n\
             atom main() -> i64\n\
             requires: true;\n\
             ensures: true;\n\
             body: { acquire counter { counter.value = 3; 0 } };",
        );
        let after = env_and_hash(
            "resource counter { value: i64 } priority: 1 mode: exclusive invariant: counter.value >= 5;\n\
             atom main() -> i64\n\
             requires: true;\n\
             ensures: true;\n\
             body: { acquire counter { counter.value = 3; 0 } };",
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
        assert_eq!(
            baseline,
            "7002005f369e9804b705fcccdb7d24d6ec1eb1151f4bd330c22b274f8a35fbdb"
        );
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
    fn effect_free_proof_hash_matches_policy_version_44() {
        let source = "atom main(x: i64) -> i64\n\
                      requires: true;\n\
                      ensures: result == x;\n\
                      body: x;\n";
        let hash = proof_hash_for(source, "main");

        assert_eq!(
            hash,
            "369e8579e33fa3311a5cba77ce270b739fcc099d2cf8d532c51970ee86dcc8fa"
        );
        assert!(!dependency_hashes_for(source, "main")
            .keys()
            .any(|key| key.starts_with("resource:")));
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
