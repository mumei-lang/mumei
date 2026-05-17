use super::*;

/// - release_borrow: 借用を解放。
///
/// NOTE: Primary move analysis is now handled by MIR MoveAnalysis (Plan 19).
/// This struct is kept for Z3-level borrow/consume tracking during symbolic
/// execution. See mumei-core/src/mir_analysis.rs for the MIR-based replacement.
#[derive(Debug, Clone, Default)]
pub struct LinearityCtx {
    /// 変数名 → 生存状態（true = alive, false = consumed）
    alive: HashMap<String, bool>,
    /// 変数名 → 借用カウント（0 = 借用なし、1+ = 借用中）
    borrow_count: HashMap<String, usize>,
    /// 変数名 → 借用元の変数名リスト（誰がこの変数を借用しているか）
    borrowers: HashMap<String, Vec<String>>,
    /// 消費済み変数のアクセス違反リスト
    violations: Vec<String>,
}

impl LinearityCtx {
    pub fn new() -> Self {
        Self::default()
    }

    /// 変数を生存状態で登録する
    pub fn register(&mut self, name: &str) {
        self.alive.insert(name.to_string(), true);
        self.borrow_count.insert(name.to_string(), 0);
    }

    /// 変数を消費済みとしてマークする（所有権の移動）
    /// 既に消費済みの場合は二重解放エラーを記録する。
    /// 借用中の場合は消費を拒否する。
    pub fn consume(&mut self, name: &str) -> Result<(), String> {
        // 借用中チェック: 借用されている変数は消費できない
        if let Some(&count) = self.borrow_count.get(name) {
            if count > 0 {
                let borrower_names = self
                    .borrowers
                    .get(name)
                    .map(|v| v.join(", "))
                    .unwrap_or_else(|| "unknown".to_string());
                let msg = format!(
                    "Cannot consume '{}': currently borrowed by [{}] ({} active borrow(s))",
                    name, borrower_names, count
                );
                self.violations.push(msg.clone());
                return Err(msg);
            }
        }

        match self.alive.get(name) {
            Some(true) => {
                self.alive.insert(name.to_string(), false);
                Ok(())
            }
            Some(false) => {
                let msg = format!("Double-free detected: '{}' has already been consumed", name);
                self.violations.push(msg.clone());
                Err(msg)
            }
            None => {
                // 追跡対象外の変数は無視（通常の値型）
                Ok(())
            }
        }
    }

    /// 変数を借用する（読み取り専用の参照）
    /// 借用中は所有者が consume/free できなくなる。
    /// borrower_name: 借用する側の変数名（ライフタイム追跡用）
    pub fn borrow(&mut self, owner_name: &str, borrower_name: &str) -> Result<(), String> {
        // 生存チェック: 消費済み変数は借用できない
        if let Some(false) = self.alive.get(owner_name) {
            let msg = format!(
                "Cannot borrow '{}': it has already been consumed (use-after-free)",
                owner_name
            );
            self.violations.push(msg.clone());
            return Err(msg);
        }

        let count = self.borrow_count.entry(owner_name.to_string()).or_insert(0);
        *count += 1;
        self.borrowers
            .entry(owner_name.to_string())
            .or_default()
            .push(borrower_name.to_string());
        Ok(())
    }

    /// 借用を解放する
    pub fn release_borrow(&mut self, owner_name: &str, borrower_name: &str) {
        if let Some(count) = self.borrow_count.get_mut(owner_name) {
            if *count > 0 {
                *count -= 1;
            }
        }
        if let Some(borrowers) = self.borrowers.get_mut(owner_name) {
            borrowers.retain(|b| b != borrower_name);
        }
    }

    /// 変数が生存しているかチェックする
    /// 消費済み変数へのアクセスはエラーを記録する
    pub fn check_alive(&mut self, name: &str) -> Result<(), String> {
        if let Some(false) = self.alive.get(name) {
            let msg = format!(
                "Use-after-free detected: '{}' has been consumed and is no longer valid",
                name
            );
            self.violations.push(msg.clone());
            return Err(msg);
        }
        Ok(())
    }

    /// 変数が借用中かどうかを確認する
    /// Available for future borrow-conflict diagnostic passes.
    #[allow(dead_code)]
    pub fn is_borrowed(&self, name: &str) -> bool {
        self.borrow_count.get(name).is_some_and(|&c| c > 0)
    }

    /// 蓄積された違反リストを返す
    pub fn get_violations(&self) -> &[String] {
        &self.violations
    }

    /// 違反があるかどうか
    pub fn has_violations(&self) -> bool {
        !self.violations.is_empty()
    }
}

// =============================================================================
// モジュール環境: グローバル static Mutex から構造体ベースの管理に移行
// =============================================================================

/// モジュール単位の環境。型定義・構造体定義・atom 定義・enum 定義を保持する。
/// グローバル static Mutex を廃止し、この構造体で一元管理する。
/// main.rs で構築し、verify() / codegen に参照渡しする。
#[derive(Debug, Clone, Default)]
pub struct ModuleEnv {
    /// 精緻型定義（FQN キー: 例 "math::Nat" or 自モジュールなら "Nat"）
    pub types: HashMap<String, RefinedType>,
    /// 構造体定義（FQN キー）
    pub structs: HashMap<String, StructDef>,
    /// Atom 定義（FQN キー）。契約による検証で requires/ensures のみ参照する。
    pub atoms: HashMap<String, Atom>,
    /// Enum 定義（FQN キー）
    pub enums: HashMap<String, EnumDef>,
    /// トレイト定義
    pub traits: HashMap<String, TraitDef>,
    /// トレイト実装: (トレイト名, 型名) → ImplDef
    pub impls: Vec<ImplDef>,
    /// 検証済み Atom 名のキャッシュ
    pub verified_cache: HashSet<String>,
    /// リソース定義（非同期安全性検証用）
    /// リソース名 → (優先度, アクセスモード)
    pub resources: HashMap<String, ResourceDef>,
    /// エフェクト定義（副作用検証用）
    /// エフェクト名 → EffectDef
    pub effects: HashMap<String, EffectDef>,
    /// エフェクト定義レジストリ（階層構造対応）
    /// Step 2a: EffectDef のパラメータ・制約・親を含む完全な定義
    pub effect_defs: HashMap<String, EffectDef>,
    /// Symbolic String ID: パス文字列 → 整数ID のマッピング（ハイブリッド・アプローチ）
    // NOTE: path_id_map/next_path_id/prefix_ranges are infrastructure for Z3 String Sort migration (see ROADMAP.md P4)
    #[allow(dead_code)]
    pub path_id_map: HashMap<String, i64>,
    #[allow(dead_code)]
    pub next_path_id: i64,
    /// パスプレフィックス → (range_start, range_end) のマッピング
    #[allow(dead_code)]
    pub prefix_ranges: HashMap<String, (i64, i64)>,

    // =========================================================================
    // Dependency Graph (Feature 2-a: Gradual Verification)
    // =========================================================================
    /// Forward dependency graph: atom_name → set of atoms it calls
    pub dependency_graph: HashMap<String, HashSet<String>>,
    /// Reverse dependency graph: atom_name → set of atoms that call it
    pub reverse_deps: HashMap<String, HashSet<String>>,
    /// Security policy for effect parameter constraint enforcement
    pub security_policy: Option<SecurityPolicy>,
    /// メソッド名 → Vec<(トレイト名, メソッドインデックス)> の逆引きインデックス。
    /// `register_trait()` 時に構築され、`get_traits_for_method()` で使用する。
    /// HashMap iteration の非決定性を排除し、同名メソッドを持つ複数トレイトを正しく解決する。
    pub method_trait_index: HashMap<String, Vec<(String, usize)>>,
}

impl ModuleEnv {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_type(&mut self, refined_type: &RefinedType) {
        self.types
            .insert(refined_type.name.clone(), refined_type.clone());
    }

    pub fn register_struct(&mut self, struct_def: &StructDef) {
        self.structs
            .insert(struct_def.name.clone(), struct_def.clone());
    }

    pub fn register_atom(&mut self, atom: &Atom) {
        self.atoms.insert(atom.name.clone(), atom.clone());
    }

    pub fn register_enum(&mut self, enum_def: &EnumDef) {
        self.enums.insert(enum_def.name.clone(), enum_def.clone());
    }

    pub fn get_type(&self, name: &str) -> Option<&RefinedType> {
        self.types.get(name)
    }

    pub fn get_struct(&self, name: &str) -> Option<&StructDef> {
        self.structs.get(name)
    }

    pub fn get_atom(&self, name: &str) -> Option<&Atom> {
        self.atoms.get(name)
    }

    #[allow(dead_code)]
    pub fn get_enum(&self, name: &str) -> Option<&EnumDef> {
        self.enums.get(name)
    }

    /// Variant 名から所属する Enum 定義を逆引きする
    pub fn find_enum_by_variant(&self, variant_name: &str) -> Option<&EnumDef> {
        self.enums
            .values()
            .find(|e| e.variants.iter().any(|v| v.name == variant_name))
    }

    /// 精緻型名からベース型名を解決する（例: "Nat" -> "i64", "Pos" -> "f64"）
    pub fn resolve_base_type(&self, type_name: &str) -> String {
        // Plan 9: Str is a primitive type, return as-is
        if type_name == "Str" {
            return "Str".to_string();
        }
        if let Some(refined) = self.types.get(type_name) {
            return refined._base_type.clone();
        }
        type_name.to_string()
    }

    pub fn register_trait(&mut self, trait_def: &TraitDef) {
        // メソッド→トレイト逆引きインデックスを構築
        // 再登録時は旧エントリを除去してから追加（冪等性を維持）
        for entries in self.method_trait_index.values_mut() {
            entries.retain(|(tn, _)| tn != &trait_def.name);
        }
        for (idx, method) in trait_def.methods.iter().enumerate() {
            self.method_trait_index
                .entry(method.name.clone())
                .or_default()
                .push((trait_def.name.clone(), idx));
        }
        self.traits
            .insert(trait_def.name.clone(), trait_def.clone());
    }

    pub fn register_impl(&mut self, impl_def: &ImplDef) {
        self.impls.push(impl_def.clone());
    }

    pub fn get_trait(&self, name: &str) -> Option<&TraitDef> {
        self.traits.get(name)
    }

    /// 指定した型がトレイトを実装しているか確認する
    pub fn find_impl(&self, trait_name: &str, target_type: &str) -> Option<&ImplDef> {
        self.impls
            .iter()
            .find(|i| i.trait_name == trait_name && i.target_type == target_type)
    }

    /// メソッド名からトレイト定義とメソッドのparam_constraintsを取得する（全候補版）。
    /// Returns Vec<(trait_name, &TraitMethod)> for all traits defining this method.
    /// Uses the `method_trait_index` for deterministic, O(1) lookup.
    pub fn get_traits_for_method(&self, method_name: &str) -> Vec<(&str, &TraitMethod)> {
        let mut results = Vec::new();
        if let Some(entries) = self.method_trait_index.get(method_name) {
            for (trait_name, method_idx) in entries {
                if let Some(trait_def) = self.traits.get(trait_name) {
                    if let Some(method) = trait_def.methods.get(*method_idx) {
                        results.push((trait_name.as_str(), method));
                    }
                }
            }
        }
        results
    }

    /// 後方互換ラッパー: 候補が1つの場合は従来通りの動作を維持。
    /// 複数候補がある場合は最初の候補を返す（呼び出し元で find_impl による絞り込みを推奨）。
    #[allow(dead_code)]
    pub fn get_trait_for_method(&self, method_name: &str) -> Option<(&str, &TraitMethod)> {
        let candidates = self.get_traits_for_method(method_name);
        candidates.into_iter().next()
    }

    /// 指定した型がトレイト境界を全て満たしているか検証する
    pub fn check_trait_bounds(&self, type_name: &str, bounds: &[String]) -> Result<(), String> {
        for bound in bounds {
            if self.find_impl(bound, type_name).is_none() {
                return Err(format!(
                    "Type '{}' does not implement trait '{}'",
                    type_name, bound
                ));
            }
        }
        Ok(())
    }

    /// Atom を検証済みとしてマークする
    pub fn mark_verified(&mut self, atom_name: &str) {
        self.verified_cache.insert(atom_name.to_string());
    }

    /// Atom が検証済みかどうかを確認する
    pub fn is_verified(&self, atom_name: &str) -> bool {
        self.verified_cache.contains(atom_name)
    }

    /// P5-C: Set the trust level of a registered atom (for taint analysis on unverified imports).
    /// If the atom is not found by exact name, this is a no-op (the atom may not have been
    /// registered yet, or may be registered under a different key).
    pub fn set_trust_level(&mut self, atom_name: &str, level: TrustLevel) {
        if let Some(atom) = self.atoms.get_mut(atom_name) {
            atom.trust_level = level.clone();
        }
        // Also remove from verified_cache if setting to Unverified
        if matches!(level, TrustLevel::Unverified) {
            self.verified_cache.remove(atom_name);
        }
    }

    /// リソース定義を登録する
    pub fn register_resource(&mut self, resource_def: &ResourceDef) {
        self.resources
            .insert(resource_def.name.clone(), resource_def.clone());
    }

    /// リソース定義を取得する
    #[allow(dead_code)]
    pub fn get_resource(&self, name: &str) -> Option<&ResourceDef> {
        self.resources.get(name)
    }

    /// エフェクト定義を登録する（effects + effect_defs 両方に登録）
    pub fn register_effect(&mut self, effect_def: &EffectDef) {
        self.effects
            .insert(effect_def.name.clone(), effect_def.clone());
        self.effect_defs
            .insert(effect_def.name.clone(), effect_def.clone());
    }

    /// エフェクト定義を取得する
    #[allow(dead_code)]
    pub fn get_effect(&self, name: &str) -> Option<&EffectDef> {
        self.effects.get(name)
    }

    /// エフェクト名のリストを展開し、includes を再帰的に解決して
    /// 全てのリーフエフェクト名を返す。
    pub fn resolve_effect_set(&self, names: &[String]) -> HashSet<String> {
        let mut result = HashSet::new();
        let mut stack: Vec<String> = names.to_vec();
        while let Some(name) = stack.pop() {
            if result.contains(&name) {
                continue;
            }
            result.insert(name.clone());
            if let Some(def) = self.effects.get(&name) {
                for included in &def.includes {
                    if !result.contains(included) {
                        stack.push(included.clone());
                    }
                }
            }
        }
        result
    }

    /// Vec<Effect> からエフェクト名のリストを展開し、includes を再帰的に解決する。
    pub fn resolve_effect_set_from_effects(
        &self,
        effects: &[crate::parser::Effect],
    ) -> HashSet<String> {
        let names: Vec<String> = effects.iter().map(|e| e.name.clone()).collect();
        self.resolve_effect_set(&names)
    }

    /// Resolve an effect set to only its leaf effects (effects with no `includes`).
    /// This avoids false positives when comparing a caller declaring leaf effects
    /// against a callee declaring a composite effect (e.g., `[IO]` vs `[FileRead, FileWrite, Console]`).
    pub fn resolve_leaf_effects(&self, names: &[String]) -> HashSet<String> {
        let full = self.resolve_effect_set(names);
        full.into_iter()
            .filter(|name| {
                self.effects
                    .get(name)
                    .is_none_or(|def| def.includes.is_empty())
            })
            .collect()
    }

    /// Vec<Effect> からリーフエフェクトを解決する。
    pub fn resolve_leaf_effects_from_effects(
        &self,
        effects: &[crate::parser::Effect],
    ) -> HashSet<String> {
        let names: Vec<String> = effects.iter().map(|e| e.name.clone()).collect();
        self.resolve_leaf_effects(&names)
    }

    // =========================================================================
    // Step 2b: エフェクト階層の解決メソッド
    // =========================================================================

    /// エフェクト名からその祖先エフェクト（親→祖父→...）を全て返す。
    /// HttpRead → [Network] のように、包含関係を解決する。
    /// Plan 6: Multi-parent support — BFS over all parents.
    pub fn get_effect_ancestors(&self, effect_name: &str) -> Vec<String> {
        let mut ancestors = Vec::new();
        let mut visited = HashSet::new();
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(effect_name.to_string());
        visited.insert(effect_name.to_string());
        while let Some(current) = queue.pop_front() {
            // effect_defs を優先、なければ effects も参照
            let parents: Vec<String> = self
                .effect_defs
                .get(&current)
                .map(|def| def.parent.clone())
                .or_else(|| self.effects.get(&current).map(|def| def.parent.clone()))
                .unwrap_or_default();
            for parent in parents {
                if visited.insert(parent.clone()) {
                    ancestors.push(parent.clone());
                    queue.push_back(parent);
                }
            }
        }
        ancestors
    }

    /// effect_a が effect_b のサブタイプかを判定。
    /// HttpRead は Network のサブタイプ → is_subeffect("HttpRead", "Network") == true
    pub fn is_subeffect(&self, child: &str, parent: &str) -> bool {
        if child == parent {
            return true;
        }
        self.get_effect_ancestors(child)
            .contains(&parent.to_string())
    }

    // =========================================================================
    // Step 2c: Symbolic String ID 管理（ハイブリッド・アプローチ）
    // =========================================================================

    /// パス文字列を整数IDに変換して登録する。既に登録済みなら既存IDを返す。
    // NOTE: register_path_id is infrastructure for Z3 String Sort migration (see ROADMAP.md P4)
    #[allow(dead_code)]
    pub fn register_path_id(&mut self, path: &str) -> i64 {
        if let Some(&id) = self.path_id_map.get(path) {
            return id;
        }
        let id = self.next_path_id;
        self.next_path_id += 1;
        self.path_id_map.insert(path.to_string(), id);
        id
    }

    /// プレフィックスに対して整数範囲を割り当てる。
    /// 例: "/tmp/" → (1000, 1999) のように、"/tmp/" で始まるパスはこの範囲のIDを持つ。
    // NOTE: register_prefix_range is infrastructure for Z3 String Sort migration (see ROADMAP.md P4)
    #[allow(dead_code)]
    pub fn register_prefix_range(&mut self, prefix: &str, range_start: i64, range_end: i64) {
        self.prefix_ranges
            .insert(prefix.to_string(), (range_start, range_end));
    }

    /// パスIDが指定プレフィックスの範囲内にあるかチェックする。
    // NOTE: path_id_matches_prefix is infrastructure for Z3 String Sort migration (see ROADMAP.md P4)
    #[allow(dead_code)]
    pub fn path_id_matches_prefix(&self, path_id: i64, prefix: &str) -> bool {
        if let Some(&(start, end)) = self.prefix_ranges.get(prefix) {
            path_id >= start && path_id <= end
        } else {
            false
        }
    }

    /// エフェクト定義が存在するか確認する（トレイト境界 "Effect" の検証用）
    pub fn has_effect_def(&self, name: &str) -> bool {
        self.effect_defs.contains_key(name)
    }

    /// エフェクト定義を effect_defs レジストリに登録する。
    // NOTE: register_effect_def is used by future EffectDef import registration path
    #[allow(dead_code)]
    pub fn register_effect_def(&mut self, effect_def: &EffectDef) {
        self.effect_defs
            .insert(effect_def.name.clone(), effect_def.clone());
    }

    // =========================================================================
    // Dependency Graph Methods (Feature 2-a)
    // =========================================================================

    /// Register the set of atoms that `atom_name` calls.
    /// Populates both forward (`dependency_graph`) and reverse (`reverse_deps`) maps.
    pub fn register_dependencies(&mut self, atom_name: &str, callees: HashSet<String>) {
        for callee in &callees {
            self.reverse_deps
                .entry(callee.clone())
                .or_default()
                .insert(atom_name.to_string());
        }
        self.dependency_graph.insert(atom_name.to_string(), callees);
    }

    /// BFS traversal of `reverse_deps` to find all atoms transitively depending
    /// on the given atom.
    #[allow(dead_code)]
    pub fn get_transitive_dependents(&self, atom_name: &str) -> HashSet<String> {
        let mut result = HashSet::new();
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(atom_name.to_string());
        while let Some(current) = queue.pop_front() {
            if let Some(dependents) = self.reverse_deps.get(&current) {
                for dep in dependents {
                    if result.insert(dep.clone()) {
                        queue.push_back(dep.clone());
                    }
                }
            }
        }
        result
    }
}

// =============================================================================
// 組み込みトレイト (Built-in Traits)
// =============================================================================

/// 組み込みトレイトを ModuleEnv に自動登録する。
/// Numeric（算術演算）、Ord（比較）、Eq（等価性）の3つを提供。
pub fn register_builtin_traits(module_env: &mut ModuleEnv) {
    use crate::parser::{ImplDef as ID, TraitDef as TD, TraitMethod};

    // --- trait Eq ---
    // fn eq(a: Self, b: Self) -> bool;
    // law reflexive: eq(x, x) == true;
    // law symmetric: eq(a, b) => eq(b, a);
    module_env.register_trait(&TD {
        name: "Eq".to_string(),
        methods: vec![TraitMethod {
            name: "eq".to_string(),
            param_types: vec!["Self".into(), "Self".into()],
            return_type: "bool".into(),
            param_constraints: vec![None, None],
        }],
        laws: vec![
            ("reflexive".into(), "eq(x, x) == true".into()),
            ("symmetric".into(), "eq(a, b) => eq(b, a)".into()),
        ],
        span: Span::default(),
    });

    // --- trait Ord (extends Eq implicitly) ---
    // fn leq(a: Self, b: Self) -> bool;
    // law reflexive: leq(x, x) == true;
    // law antisymmetric: leq(a, b) && leq(b, a) => eq(a, b);
    // law transitive: leq(a, b) && leq(b, c) => leq(a, c);
    module_env.register_trait(&TD {
        name: "Ord".to_string(),
        methods: vec![TraitMethod {
            name: "leq".to_string(),
            param_types: vec!["Self".into(), "Self".into()],
            return_type: "bool".into(),
            param_constraints: vec![None, None],
        }],
        laws: vec![
            ("reflexive".into(), "leq(x, x) == true".into()),
            (
                "transitive".into(),
                "leq(a, b) && leq(b, c) => leq(a, c)".into(),
            ),
        ],
        span: Span::default(),
    });

    // --- trait Numeric (extends Ord implicitly) ---
    // fn add(a: Self, b: Self) -> Self;
    // fn sub(a: Self, b: Self) -> Self;
    // fn mul(a: Self, b: Self) -> Self;
    // law additive_identity: add(a, 0) == a;
    // law commutative_add: add(a, b) == add(b, a);
    module_env.register_trait(&TD {
        name: "Numeric".to_string(),
        methods: vec![
            TraitMethod {
                name: "add".to_string(),
                param_types: vec!["Self".into(), "Self".into()],
                return_type: "Self".into(),
                param_constraints: vec![None, None],
            },
            TraitMethod {
                name: "sub".to_string(),
                param_types: vec!["Self".into(), "Self".into()],
                return_type: "Self".into(),
                param_constraints: vec![None, None],
            },
            TraitMethod {
                name: "mul".to_string(),
                param_types: vec!["Self".into(), "Self".into()],
                return_type: "Self".into(),
                param_constraints: vec![None, None],
            },
            TraitMethod {
                name: "div".to_string(),
                param_types: vec!["Self".into(), "Self".into()],
                return_type: "Self".into(),
                param_constraints: vec![None, Some("v != 0".to_string())],
            },
        ],
        laws: vec![("commutative_add".into(), "add(a, b) == add(b, a)".into())],
        span: Span::default(),
    });

    // --- 組み込み impl: i64, u64, f64 は Eq + Ord + Numeric を自動実装 ---
    for base_type in &["i64", "u64", "f64"] {
        module_env.register_impl(&ID {
            trait_name: "Eq".into(),
            target_type: base_type.to_string(),
            method_bodies: vec![("eq".into(), "a == b".into())],
            span: Span::default(),
        });
        module_env.register_impl(&ID {
            trait_name: "Ord".into(),
            target_type: base_type.to_string(),
            method_bodies: vec![("leq".into(), "a <= b".into())],
            span: Span::default(),
        });
        module_env.register_impl(&ID {
            trait_name: "Numeric".into(),
            target_type: base_type.to_string(),
            method_bodies: vec![
                ("add".into(), "a + b".into()),
                ("sub".into(), "a - b".into()),
                ("mul".into(), "a * b".into()),
                ("div".into(), "a / b".into()),
            ],
            span: Span::default(),
        });
    }
}

// =============================================================================
// 組み込みエフェクト (Built-in Effects)
// =============================================================================

/// 組み込みエフェクトを ModuleEnv に自動登録する。
/// FileRead, FileWrite, Network, Log, Console の基本エフェクトと、
/// IO (FileRead + FileWrite + Console), FullAccess (IO + Network + Log) の
/// 複合エフェクトを提供。
pub fn register_builtin_effects(module_env: &mut ModuleEnv) {
    use crate::parser::EffectDef;

    // --- 基本エフェクト ---
    for name in &["FileRead", "FileWrite", "Network", "Log", "Console"] {
        module_env.register_effect(&EffectDef {
            name: name.to_string(),
            params: vec![],
            constraint: None,
            includes: vec![],
            refinement: None,
            parent: vec![],
            span: Span::default(),
            states: vec![],
            transitions: vec![],
            initial_state: None,
        });
    }

    // --- 複合エフェクト ---
    // IO includes FileRead, FileWrite, Console
    module_env.register_effect(&EffectDef {
        name: "IO".to_string(),
        params: vec![],
        constraint: None,
        includes: vec![
            "FileRead".to_string(),
            "FileWrite".to_string(),
            "Console".to_string(),
        ],
        refinement: None,
        parent: vec![],
        span: Span::default(),
        states: vec![],
        transitions: vec![],
        initial_state: None,
    });

    // FullAccess includes IO, Network, Log
    module_env.register_effect(&EffectDef {
        name: "FullAccess".to_string(),
        params: vec![],
        constraint: None,
        includes: vec!["IO".to_string(), "Network".to_string(), "Log".to_string()],
        refinement: None,
        parent: vec![],
        span: Span::default(),
        states: vec![],
        transitions: vec![],
        initial_state: None,
    });
}
