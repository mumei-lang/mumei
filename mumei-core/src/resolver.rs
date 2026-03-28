//! # Resolver モジュール
//!
//! import 宣言を再帰的に処理し、依存モジュールの型・構造体・atom を
//! ModuleEnv に登録する。循環参照の検出も行う。
//!
//! ## 設計方針
//! - Phase 1: ファイルベースの単純な import 解決
//! - Phase 2+: 完全修飾名（FQN）による名前空間分離、ModuleEnv ベースの管理
//!
//! ## 検証キャッシュ
//! インポートされたモジュールの atom は「検証済み」としてマークされ、
//! main.rs での body 再検証がスキップされる。呼び出し時は requires/ensures
//! の契約のみを信頼する（Compositional Verification）。
//!
//! キャッシュファイル (.mumei_cache) にはソースハッシュと検証結果を永続化し、
//! ソースが変更されていなければ再パース・再検証をスキップする。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::parser::{self, Item};
use crate::proof_cert;
use crate::verification::{ModuleEnv, MumeiError, MumeiResult};

/// 検証キャッシュのエントリ
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    /// ソースファイルの SHA-256 ハッシュ
    source_hash: String,
    /// 検証済み atom 名のリスト
    verified_atoms: Vec<String>,
    /// 型定義名のリスト
    type_names: Vec<String>,
    /// 構造体定義名のリスト
    struct_names: Vec<String>,
    /// Incremental Build: atom ごとの契約+body ハッシュ
    /// atom の requires/ensures/body_expr が変更されていなければ再検証をスキップする。
    /// キー: atom 名、値: SHA-256(name + requires + ensures + body_expr)
    #[serde(default)]
    atom_hashes: HashMap<String, String>,
}

/// キャッシュファイル全体
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct VerificationCache {
    /// ファイルパス → キャッシュエントリ
    entries: HashMap<String, CacheEntry>,
}
/// ロード済みモジュールのキャッシュ
struct ResolverContext {
    /// ロード中のモジュールパス集合（循環参照検出用）
    loading: HashSet<PathBuf>,
    /// 完全にロード済みのモジュール（キャッシュ）
    loaded: HashMap<PathBuf, Vec<Item>>,
    /// P5-C: When true, missing/invalid certificates cause hard errors instead of warnings
    strict_imports: bool,
}
impl ResolverContext {
    fn new() -> Self {
        Self {
            loading: HashSet::new(),
            loaded: HashMap::new(),
            strict_imports: false,
        }
    }
}
/// items 内の Import 宣言を処理し、依存モジュールの定義を ModuleEnv に登録する。
/// base_dir はインポート元ファイルの親ディレクトリ。
/// キャッシュファイルが存在し、ソースハッシュが一致する場合は再パースをスキップする。
pub fn resolve_imports(
    items: &[Item],
    base_dir: &Path,
    module_env: &mut ModuleEnv,
) -> MumeiResult<()> {
    resolve_imports_with_options(items, base_dir, module_env, false)
}

/// P5-C: resolve_imports with strict_imports option.
/// When strict_imports is true, missing or invalid certificates cause hard errors.
pub fn resolve_imports_with_options(
    items: &[Item],
    base_dir: &Path,
    module_env: &mut ModuleEnv,
    strict_imports: bool,
) -> MumeiResult<()> {
    let cache_path = base_dir.join(".mumei_cache");
    let mut cache = load_cache(&cache_path);
    let mut ctx = ResolverContext::new();
    ctx.strict_imports = strict_imports;
    resolve_imports_recursive(items, base_dir, &mut ctx, &mut cache, module_env)?;
    save_cache(&cache_path, &cache);
    Ok(())
}

/// std/prelude.mm を自動的にロードし、ModuleEnv に登録する。
/// ユーザーが `import "std/prelude"` を書かなくても、
/// Eq, Ord, Numeric, Option<T>, Result<T, E> 等が利用可能になる。
///
/// prelude の定義はトレイト・ADT のみを登録し、atom は検証済みとしてマークする。
/// prelude が見つからない場合はスキップする（組み込みトレイトがフォールバックとして機能）。
pub fn resolve_prelude(base_dir: &Path, module_env: &mut ModuleEnv) -> MumeiResult<()> {
    // prelude のパスを解決（見つからなければスキップ）
    let prelude_path = match resolve_path("std/prelude", base_dir) {
        Ok(path) => path,
        Err(_) => {
            // prelude が見つからない場合は静かにスキップ
            // （組み込みトレイト register_builtin_traits が代替として機能）
            return Ok(());
        }
    };

    // prelude を読み込み・パース
    let source = match fs::read_to_string(&prelude_path) {
        Ok(s) => s,
        Err(_) => return Ok(()), // 読み込み失敗もスキップ
    };

    let prelude_items = parser::parse_module(&source);

    // prelude 内の import を再帰的に解決（prelude 自身が他モジュールに依存する場合）
    let prelude_base_dir = prelude_path.parent().unwrap_or(Path::new("."));
    let cache_path = prelude_base_dir.join(".mumei_cache");
    let mut cache = load_cache(&cache_path);
    let mut ctx = ResolverContext::new();
    ctx.loading.insert(prelude_path.clone());
    resolve_imports_recursive(
        &prelude_items,
        prelude_base_dir,
        &mut ctx,
        &mut cache,
        module_env,
    )?;
    save_cache(&cache_path, &cache);

    // prelude の定義を ModuleEnv に登録（alias なし = グローバルスコープ）
    register_imported_items(&prelude_items, None, module_env);

    // prelude の atom を検証済みとしてマーク
    for item in &prelude_items {
        match item {
            Item::Atom(atom) => {
                module_env.mark_verified(&atom.name);
            }
            Item::ImplBlock(ib) => {
                for method in &ib.methods {
                    let qualified_name = format!("{}::{}", ib.struct_name, method.name);
                    module_env.mark_verified(&qualified_name);
                }
            }
            _ => {}
        }
    }

    Ok(())
}
/// P5-C: Check if an atom passed certificate verification for a given module.
/// Returns true if the atom is "proven" according to the certificate, false otherwise.
fn check_cert_for_atom(cert_results: &HashMap<String, String>, atom_name: &str) -> bool {
    cert_results
        .get(atom_name)
        .is_some_and(|status| status == "proven")
}

/// P5-C: Attempt to verify a proof certificate for an imported module directory.
/// Returns a map of atom_name -> status ("proven", "changed", "unproven", "missing").
/// Returns None if no certificate exists.
fn verify_import_certificate(
    module_dir: &Path,
    source_file: &Path,
    items: &[Item],
) -> Option<HashMap<String, String>> {
    // Look for .proof-cert.json or proof_certificate.json in the module directory
    let cert_candidates = [
        module_dir.join(".proof-cert.json"),
        module_dir.join("proof_certificate.json"),
    ];
    let cert_path = cert_candidates.iter().find(|p| p.exists())?;

    let cert = proof_cert::load_certificate(cert_path).ok()?;

    // Check if the certificate matches this source file
    let cert_file_path = Path::new(&cert.file);
    let source_matches = source_file.ends_with(cert_file_path)
        || cert_file_path.ends_with(source_file.file_name().unwrap_or_default());
    if !source_matches && !cert.file.is_empty() {
        // Certificate is for a different file — reject to prevent cross-file atom name collisions
        return None;
    }

    let mut atom_refs: Vec<&parser::Atom> = items
        .iter()
        .filter_map(|item| match item {
            Item::Atom(a) => Some(a),
            _ => None,
        })
        .collect();

    // Also collect ImplBlock methods with qualified names for cert verification
    let mut qualified_methods: Vec<parser::Atom> = Vec::new();
    for item in items {
        if let Item::ImplBlock(ib) = item {
            for method in &ib.methods {
                let mut qualified = method.clone();
                qualified.name = format!("{}::{}", ib.struct_name, method.name);
                qualified_methods.push(qualified);
            }
        }
    }
    for qm in &qualified_methods {
        atom_refs.push(qm);
    }

    let results = proof_cert::verify_certificate(&cert, &atom_refs);
    Some(results.into_iter().collect())
}

/// P5-C: Mark dependency atoms as verified or unverified based on cert verification results.
/// Used by resolve_manifest_dependencies() for all dependency types.
/// When strict_imports is true and no cert exists or cert verification fails, returns an error.
fn mark_dependency_atoms_with_cert(
    items: &[Item],
    dep_name: &str,
    cert_results: &Option<HashMap<String, String>>,
    module_env: &mut ModuleEnv,
    strict_imports: bool,
) -> MumeiResult<()> {
    // P5-C: In strict mode, missing certificates are hard errors
    if strict_imports && cert_results.is_none() {
        return Err(MumeiError::verification(format!(
            "Strict imports: dependency '{}' has no proof certificate. \
             Provide a .proof-cert.json or proof_certificate.json in the package root.",
            dep_name
        )));
    }

    for item in items {
        match item {
            Item::Atom(atom) => {
                let should_verify = match cert_results {
                    Some(results) => check_cert_for_atom(results, &atom.name),
                    None => true, // No cert = legacy behavior (only reachable when !strict_imports)
                };
                if should_verify {
                    module_env.mark_verified(&atom.name);
                    let fqn = format!("{}::{}", dep_name, atom.name);
                    module_env.mark_verified(&fqn);
                } else {
                    if strict_imports {
                        return Err(MumeiError::verification(format!(
                            "Strict imports: dependency '{}': atom '{}' failed certificate verification",
                            dep_name, atom.name
                        )));
                    }
                    eprintln!(
                        "  ⚠️  Dependency '{}': atom '{}' failed certificate verification",
                        dep_name, atom.name
                    );
                    module_env.set_trust_level(&atom.name, crate::parser::TrustLevel::Unverified);
                    let fqn = format!("{}::{}", dep_name, atom.name);
                    module_env.set_trust_level(&fqn, crate::parser::TrustLevel::Unverified);
                }
            }
            Item::ImplBlock(ib) => {
                for method in &ib.methods {
                    let qualified = format!("{}::{}", ib.struct_name, method.name);
                    let should_verify = match cert_results {
                        Some(results) => check_cert_for_atom(results, &qualified),
                        None => true,
                    };
                    if should_verify {
                        module_env.mark_verified(&qualified);
                        let fqn = format!("{}::{}::{}", dep_name, ib.struct_name, method.name);
                        module_env.mark_verified(&fqn);
                    } else {
                        if strict_imports {
                            return Err(MumeiError::verification(format!(
                                "Strict imports: dependency '{}': method '{}' failed certificate verification",
                                dep_name, qualified
                            )));
                        }
                        eprintln!(
                            "  ⚠️  Dependency '{}': method '{}' failed certificate verification",
                            dep_name, qualified
                        );
                        module_env
                            .set_trust_level(&qualified, crate::parser::TrustLevel::Unverified);
                        let fqn = format!("{}::{}::{}", dep_name, ib.struct_name, method.name);
                        module_env.set_trust_level(&fqn, crate::parser::TrustLevel::Unverified);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// 再帰的にインポートを解決する内部関数
fn resolve_imports_recursive(
    items: &[Item],
    base_dir: &Path,
    ctx: &mut ResolverContext,
    cache: &mut VerificationCache,
    module_env: &mut ModuleEnv,
) -> MumeiResult<()> {
    for item in items {
        if let Item::Import(import_decl) = item {
            let resolved_path = resolve_path(&import_decl.path, base_dir)?;
            // 循環参照チェック
            if ctx.loading.contains(&resolved_path) {
                return Err(MumeiError::verification_at(
                    format!("Circular import detected: '{}'", resolved_path.display()),
                    import_decl.span.clone(),
                ));
            }
            // 既にロード済みならスキップ
            if ctx.loaded.contains_key(&resolved_path) {
                continue;
            }
            // ロード中としてマーク
            ctx.loading.insert(resolved_path.clone());
            // ファイルを読み込みパース
            let source = fs::read_to_string(&resolved_path).map_err(|e| {
                MumeiError::verification_at(
                    format!(
                        "Failed to read imported module '{}': {}",
                        import_decl.path, e
                    ),
                    import_decl.span.clone(),
                )
            })?;

            let path_key = resolved_path.to_string_lossy().to_string();
            let source_hash = compute_hash(&source);

            // キャッシュヒット判定: ソースハッシュが一致すれば再パース不要
            if let Some(entry) = cache.entries.get(&path_key) {
                if entry.source_hash == source_hash {
                    // キャッシュから atom を検証済みとしてマーク（body 再検証スキップ）
                    // ただし型・構造体・atom の登録は必要なので、パースは行う
                }
            }

            let imported_items = parser::parse_module(&source);
            let import_base_dir = resolved_path.parent().unwrap_or(Path::new("."));
            // 再帰的にインポートを解決（インポートされたモジュール内の import も処理）
            resolve_imports_recursive(&imported_items, import_base_dir, ctx, cache, module_env)?;
            // インポートされたモジュールの定義を ModuleEnv に登録
            let alias_prefix = import_decl.alias.as_deref();
            register_imported_items(&imported_items, alias_prefix, module_env);

            // P5-C: Check for proof certificate before marking atoms as verified
            let cert_results =
                verify_import_certificate(import_base_dir, &resolved_path, &imported_items);

            // P5-C: In strict mode, missing certificates are hard errors
            if ctx.strict_imports && cert_results.is_none() {
                return Err(MumeiError::verification(format!(
                    "Strict imports: imported module '{}' has no proof certificate. \
                     Provide a .proof-cert.json or proof_certificate.json in the module directory.",
                    resolved_path.display()
                )));
            }

            // インポートされた atom を検証済みとしてマーク
            // P5-C: Only mark as verified if cert check passes (or no cert exists = legacy behavior)
            let mut verified_atoms = Vec::new();
            let mut type_names = Vec::new();
            let mut struct_names = Vec::new();
            for imported_item in &imported_items {
                match imported_item {
                    Item::Atom(atom) => {
                        let should_verify = match &cert_results {
                            Some(results) => check_cert_for_atom(results, &atom.name),
                            None => true, // No cert = legacy behavior, trust imported atoms
                        };
                        if should_verify {
                            module_env.mark_verified(&atom.name);
                            verified_atoms.push(atom.name.clone());
                            if let Some(prefix) = alias_prefix {
                                let fqn = format!("{}::{}", prefix, atom.name);
                                module_env.mark_verified(&fqn);
                                verified_atoms.push(fqn);
                            }
                        } else {
                            if ctx.strict_imports {
                                return Err(MumeiError::verification(format!(
                                    "Strict imports: import '{}': atom '{}' failed certificate verification",
                                    resolved_path.display(),
                                    atom.name
                                )));
                            }
                            eprintln!(
                                "  ⚠️  Import '{}': atom '{}' failed certificate verification",
                                resolved_path.display(),
                                atom.name
                            );
                            // P5-C: Register as unverified for taint analysis
                            module_env
                                .set_trust_level(&atom.name, crate::parser::TrustLevel::Unverified);
                            if let Some(prefix) = alias_prefix {
                                let fqn = format!("{}::{}", prefix, atom.name);
                                module_env
                                    .set_trust_level(&fqn, crate::parser::TrustLevel::Unverified);
                            }
                        }
                    }
                    Item::TypeDef(t) => type_names.push(t.name.clone()),
                    Item::StructDef(s) => struct_names.push(s.name.clone()),
                    Item::EnumDef(_) => {}
                    Item::TraitDef(_) => {}
                    Item::ImplDef(_) => {}
                    Item::ResourceDef(_) => {}
                    Item::Import(_) => {}
                    Item::ExternBlock(_) => {}
                    Item::EffectDef(_) => {}
                    Item::ImplBlock(ib) => {
                        for method in &ib.methods {
                            let qualified_name = format!("{}::{}", ib.struct_name, method.name);
                            let should_verify = match &cert_results {
                                Some(results) => check_cert_for_atom(results, &qualified_name),
                                None => true,
                            };
                            if should_verify {
                                module_env.mark_verified(&qualified_name);
                                verified_atoms.push(qualified_name.clone());
                                if let Some(prefix) = alias_prefix {
                                    let fqn =
                                        format!("{}::{}::{}", prefix, ib.struct_name, method.name);
                                    module_env.mark_verified(&fqn);
                                    verified_atoms.push(fqn);
                                }
                            } else {
                                if ctx.strict_imports {
                                    return Err(MumeiError::verification(format!(
                                        "Strict imports: import '{}': method '{}' failed certificate verification",
                                        resolved_path.display(),
                                        qualified_name
                                    )));
                                }
                                eprintln!(
                                    "  ⚠️  Import '{}': method '{}' failed certificate verification",
                                    resolved_path.display(),
                                    qualified_name
                                );
                                module_env.set_trust_level(
                                    &qualified_name,
                                    crate::parser::TrustLevel::Unverified,
                                );
                                if let Some(prefix) = alias_prefix {
                                    let fqn =
                                        format!("{}::{}::{}", prefix, ib.struct_name, method.name);
                                    module_env.set_trust_level(
                                        &fqn,
                                        crate::parser::TrustLevel::Unverified,
                                    );
                                }
                            }
                        }
                    }
                }
            }

            // キャッシュを更新
            cache.entries.insert(
                path_key,
                CacheEntry {
                    source_hash,
                    verified_atoms,
                    type_names,
                    struct_names,
                    atom_hashes: HashMap::new(),
                },
            );

            // ロード完了
            ctx.loading.remove(&resolved_path);
            ctx.loaded.insert(resolved_path, imported_items);
        }
    }
    Ok(())
}
/// インポートされたモジュールの Item を ModuleEnv に登録する。
/// alias が指定されている場合、FQN（alias::name）でも登録する。
fn register_imported_items(items: &[Item], alias: Option<&str>, module_env: &mut ModuleEnv) {
    for item in items {
        match item {
            Item::TypeDef(refined_type) => {
                module_env.register_type(refined_type);
                if let Some(prefix) = alias {
                    let mut fqn_type = refined_type.clone();
                    fqn_type.name = format!("{}::{}", prefix, refined_type.name);
                    module_env.register_type(&fqn_type);
                }
            }
            Item::StructDef(struct_def) => {
                module_env.register_struct(struct_def);
                if let Some(prefix) = alias {
                    let mut fqn_struct = struct_def.clone();
                    fqn_struct.name = format!("{}::{}", prefix, struct_def.name);
                    module_env.register_struct(&fqn_struct);
                }
            }
            Item::Atom(atom) => {
                module_env.register_atom(atom);
                if let Some(prefix) = alias {
                    let mut fqn_atom = atom.clone();
                    fqn_atom.name = format!("{}::{}", prefix, atom.name);
                    module_env.register_atom(&fqn_atom);
                }
            }
            Item::EnumDef(enum_def) => {
                module_env.register_enum(enum_def);
                if let Some(prefix) = alias {
                    let mut fqn_enum = enum_def.clone();
                    fqn_enum.name = format!("{}::{}", prefix, enum_def.name);
                    module_env.register_enum(&fqn_enum);
                }
            }
            Item::TraitDef(trait_def) => {
                module_env.register_trait(trait_def);
                // トレイトは FQN 登録不要（トレイト名はグローバルに一意と仮定）
            }
            Item::ImplDef(impl_def) => {
                module_env.register_impl(impl_def);
            }
            Item::ResourceDef(resource_def) => {
                module_env.register_resource(resource_def);
                if let Some(prefix) = alias {
                    let mut fqn_resource = resource_def.clone();
                    fqn_resource.name = format!("{}::{}", prefix, resource_def.name);
                    module_env.register_resource(&fqn_resource);
                }
            }
            Item::Import(_) => {
                // 再帰的に処理済み
            }
            Item::EffectDef(effect_def) => {
                module_env.register_effect(effect_def);
                if let Some(prefix) = alias {
                    let mut fqn_effect = effect_def.clone();
                    fqn_effect.name = format!("{}::{}", prefix, effect_def.name);
                    module_env.register_effect(&fqn_effect);
                }
            }
            Item::ExternBlock(extern_block) => {
                // ExternBlock 内の関数を trusted atom として ModuleEnv に登録
                for ext_fn in &extern_block.functions {
                    let params: Vec<crate::parser::Param> = ext_fn
                        .param_types
                        .iter()
                        .enumerate()
                        .map(|(i, ty)| crate::parser::Param {
                            name: ext_fn
                                .param_names
                                .get(i)
                                .cloned()
                                .unwrap_or_else(|| format!("arg{}", i)),
                            type_name: Some(ty.clone()),
                            type_ref: Some(crate::parser::parse_type_ref(ty)),
                            is_ref: false,
                            is_ref_mut: false,
                            fn_contract_requires: None,
                            fn_contract_ensures: None,
                        })
                        .collect();

                    let atom = crate::parser::Atom {
                        name: ext_fn.name.clone(),
                        type_params: vec![],
                        where_bounds: vec![],
                        params,
                        requires: ext_fn
                            .requires
                            .clone()
                            .unwrap_or_else(|| "true".to_string()),
                        forall_constraints: vec![],
                        ensures: ext_fn.ensures.clone().unwrap_or_else(|| "true".to_string()),
                        body_expr: String::new(),
                        consumed_params: vec![],
                        resources: vec![],
                        is_async: false,
                        trust_level: crate::parser::TrustLevel::Trusted,
                        max_unroll: None,
                        invariant: None,
                        effects: vec![],
                        return_type: Some(ext_fn.return_type.clone()),
                        span: ext_fn.span.clone(),
                        effect_pre: std::collections::HashMap::new(),
                        effect_post: std::collections::HashMap::new(),
                    };
                    module_env.register_atom(&atom);
                    if let Some(prefix) = alias {
                        let mut fqn_atom = atom.clone();
                        fqn_atom.name = format!("{}::{}", prefix, ext_fn.name);
                        module_env.register_atom(&fqn_atom);
                    }
                }
            }
            Item::ImplBlock(impl_block) => {
                for method in &impl_block.methods {
                    let mut qualified = method.clone();
                    qualified.name = format!("{}::{}", impl_block.struct_name, method.name);
                    module_env.register_atom(&qualified);
                    if let Some(prefix) = alias {
                        let mut fqn_atom = method.clone();
                        fqn_atom.name =
                            format!("{}::{}::{}", prefix, impl_block.struct_name, method.name);
                        module_env.register_atom(&fqn_atom);
                    }
                }
            }
        }
    }
}
/// インポートパスを絶対パスに解決する。
/// 拡張子 .mm が省略されている場合は自動補完する。
///
/// 解決順序:
/// 1. base_dir（インポート元ファイルのディレクトリ）からの相対パス
/// 2. 標準ライブラリパス（コンパイラバイナリの隣の `std/`、または実行ディレクトリの `std/`）
/// 3. MUMEI_STD_PATH 環境変数で指定されたパス
///
/// これにより `import "std/option";` のようなインポートが、
/// プロジェクト内に `std/` ディレクトリがなくても解決できる。
fn resolve_path(import_path: &str, base_dir: &Path) -> MumeiResult<PathBuf> {
    let mut path = PathBuf::from(import_path);
    if path.extension().is_none() {
        path.set_extension("mm");
    }

    // 1. base_dir からの相対パス解決を試行
    if path.is_relative() {
        let candidate = base_dir.join(&path);
        if let Ok(canonical) = candidate.canonicalize() {
            return Ok(canonical);
        }
    } else {
        // 絶対パスの場合はそのまま解決
        if let Ok(canonical) = path.canonicalize() {
            return Ok(canonical);
        }
    }

    // 2. "std/" プレフィックスの場合、標準ライブラリディレクトリから解決
    let import_str = import_path.trim_start_matches("./");
    if import_str.starts_with("std/") || import_str.starts_with("std\\") {
        // 2a. コンパイラバイナリの隣の std/ を探す
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let std_candidate = exe_dir.join(&path);
                if let Ok(canonical) = std_candidate.canonicalize() {
                    return Ok(canonical);
                }
            }
        }

        // 2b. カレントディレクトリの std/ を探す
        if let Ok(cwd) = std::env::current_dir() {
            let std_candidate = cwd.join(&path);
            if let Ok(canonical) = std_candidate.canonicalize() {
                return Ok(canonical);
            }
        }

        // 2c. Cargo マニフェストディレクトリ（開発時用）
        if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
            let std_candidate = Path::new(&manifest_dir).join(&path);
            if let Ok(canonical) = std_candidate.canonicalize() {
                return Ok(canonical);
            }
        }
    }

    // 3. MUMEI_STD_PATH 環境変数からの解決
    if let Ok(std_path) = std::env::var("MUMEI_STD_PATH") {
        let std_base = Path::new(&std_path);
        // "std/option" → std_base/option.mm として解決
        let relative = import_str
            .strip_prefix("std/")
            .or_else(|| import_str.strip_prefix("std\\"))
            .unwrap_or(import_str);
        let mut rel_path = PathBuf::from(relative);
        if rel_path.extension().is_none() {
            rel_path.set_extension("mm");
        }
        let std_candidate = std_base.join(&rel_path);
        if let Ok(canonical) = std_candidate.canonicalize() {
            return Ok(canonical);
        }
    }

    // すべて失敗した場合はエラー
    Err(MumeiError::verification(
        format!(
            "Cannot resolve import path '{}'\n  Searched:\n    - {}\n    - compiler binary directory\n    - current working directory\n    - MUMEI_STD_PATH environment variable",
            import_path,
            base_dir.join(&path).display()
        )
    ))
}

// =============================================================================
// mumei.toml の [dependencies] 解決
// =============================================================================

/// mumei.toml の [dependencies] セクションを処理し、
/// パス依存・Git 依存のモジュールを ModuleEnv に登録する。
///
/// パス依存: `math = { path = "./libs/math" }` → path/src/main.mm を解決
/// Git 依存: `math = { git = "https://...", tag = "v1.0.0" }` → ~/.mumei/packages/ に clone
pub fn resolve_manifest_dependencies(
    manifest: &crate::manifest::Manifest,
    project_dir: &Path,
    module_env: &mut ModuleEnv,
) -> MumeiResult<()> {
    resolve_manifest_dependencies_with_options(manifest, project_dir, module_env, false)
}

/// P5-C: resolve_manifest_dependencies with strict_imports option.
pub fn resolve_manifest_dependencies_with_options(
    manifest: &crate::manifest::Manifest,
    project_dir: &Path,
    module_env: &mut ModuleEnv,
    strict_imports: bool,
) -> MumeiResult<()> {
    for (dep_name, dep) in &manifest.dependencies {
        // パス依存
        if let Some(dep_path) = dep.as_path() {
            let abs_path = project_dir.join(dep_path);
            let entry_candidates = [
                abs_path.join("src/main.mm"),
                abs_path.join("main.mm"),
                abs_path.join(format!("{}.mm", dep_name)),
            ];
            let entry = entry_candidates.iter().find(|p| p.exists());
            if let Some(entry_path) = entry {
                let source = fs::read_to_string(entry_path).map_err(|e| {
                    MumeiError::verification(format!(
                        "Failed to read dependency '{}' at '{}': {}",
                        dep_name,
                        entry_path.display(),
                        e
                    ))
                })?;
                let items = parser::parse_module(&source);
                let dep_base_dir = entry_path.parent().unwrap_or(Path::new("."));
                let cache_path = dep_base_dir.join(".mumei_cache");
                let mut cache = load_cache(&cache_path);
                let mut ctx = ResolverContext::new();
                resolve_imports_recursive(&items, dep_base_dir, &mut ctx, &mut cache, module_env)?;
                save_cache(&cache_path, &cache);
                register_imported_items(&items, Some(dep_name), module_env);
                // P5-C: Check for proof certificate before marking atoms as verified
                let cert_results = verify_import_certificate(&abs_path, entry_path, &items);
                mark_dependency_atoms_with_cert(
                    &items,
                    dep_name,
                    &cert_results,
                    module_env,
                    strict_imports,
                )?;
                println!(
                    "  📦 Dependency '{}': loaded from {}",
                    dep_name,
                    entry_path.display()
                );
            } else {
                eprintln!(
                    "  ⚠️  Dependency '{}': no entry file found in '{}'",
                    dep_name,
                    abs_path.display()
                );
            }
        }
        // Git 依存（git フィールドがある場合は registry より優先）
        else if let Some((url, tag, rev, branch)) = dep.as_git() {
            let packages_dir = crate::manifest::mumei_home().join("packages");
            let _ = fs::create_dir_all(&packages_dir);
            let clone_dir = packages_dir.join(dep_name);

            if !clone_dir.exists() {
                // git clone
                let ref_arg = if let Some(t) = tag {
                    vec![
                        "--branch".to_string(),
                        t.to_string(),
                        "--depth".to_string(),
                        "1".to_string(),
                    ]
                } else if let Some(b) = branch {
                    vec![
                        "--branch".to_string(),
                        b.to_string(),
                        "--depth".to_string(),
                        "1".to_string(),
                    ]
                } else {
                    vec!["--depth".to_string(), "1".to_string()]
                };

                let mut cmd_args = vec!["clone".to_string()];
                cmd_args.extend(ref_arg);
                cmd_args.push(url.to_string());
                cmd_args.push(clone_dir.to_string_lossy().to_string());

                let status = std::process::Command::new("git")
                    .args(&cmd_args)
                    .status()
                    .map_err(|e| {
                        MumeiError::verification(format!(
                            "git clone failed for '{}': {}",
                            dep_name, e
                        ))
                    })?;

                if !status.success() {
                    return Err(MumeiError::verification(format!(
                        "git clone failed for dependency '{}' ({})",
                        dep_name, url
                    )));
                }

                // 特定の rev にチェックアウト
                if let Some(r) = rev {
                    let _ = std::process::Command::new("git")
                        .args(["checkout", r])
                        .current_dir(&clone_dir)
                        .status();
                }

                println!("  📦 Dependency '{}': cloned from {}", dep_name, url);
            } else {
                println!("  📦 Dependency '{}': using cached clone", dep_name);
            }

            // クローンしたディレクトリからエントリファイルを解決
            let entry_candidates = [
                clone_dir.join("src/main.mm"),
                clone_dir.join("main.mm"),
                clone_dir.join(format!("{}.mm", dep_name)),
            ];
            if let Some(entry_path) = entry_candidates.iter().find(|p| p.exists()) {
                let source = fs::read_to_string(entry_path).map_err(|e| {
                    MumeiError::verification(format!(
                        "Failed to read dependency '{}' at '{}': {}",
                        dep_name,
                        entry_path.display(),
                        e
                    ))
                })?;
                let items = parser::parse_module(&source);
                let dep_base_dir = entry_path.parent().unwrap_or(Path::new("."));
                let cache_path = dep_base_dir.join(".mumei_cache");
                let mut cache = load_cache(&cache_path);
                let mut ctx = ResolverContext::new();
                resolve_imports_recursive(&items, dep_base_dir, &mut ctx, &mut cache, module_env)?;
                save_cache(&cache_path, &cache);
                register_imported_items(&items, Some(dep_name), module_env);
                // P5-C: Check for proof certificate before marking atoms as verified
                // Use clone_dir (package root) instead of dep_base_dir (entry file parent)
                // because cmd_publish saves certificates to the package root.
                let cert_results = verify_import_certificate(&clone_dir, entry_path, &items);
                mark_dependency_atoms_with_cert(
                    &items,
                    dep_name,
                    &cert_results,
                    module_env,
                    strict_imports,
                )?;
            } else {
                eprintln!(
                    "  ⚠️  Dependency '{}': no entry file found in cloned repo",
                    dep_name
                );
            }
        }
        // 名前依存（registry.json から解決 — path でも git でもない場合）
        else if dep.version().is_some() || matches!(dep, crate::manifest::Dependency::Version(_))
        {
            let version = dep.version();
            if let Some(pkg_dir) = crate::registry::resolve(dep_name, version) {
                let entry_candidates: Vec<PathBuf> = vec![
                    pkg_dir.join("src/main.mm"),
                    pkg_dir.join("main.mm"),
                    pkg_dir.join(format!("{}.mm", dep_name)),
                ];
                if let Some(entry_path) = entry_candidates.iter().find(|p| p.exists()) {
                    let source = fs::read_to_string(entry_path).map_err(|e| {
                        MumeiError::verification(format!(
                            "Failed to read dependency '{}' at '{}': {}",
                            dep_name,
                            entry_path.display(),
                            e
                        ))
                    })?;
                    let items = parser::parse_module(&source);
                    let dep_base_dir = entry_path.parent().unwrap_or(Path::new("."));
                    let cache_path = dep_base_dir.join(".mumei_cache");
                    let mut cache = load_cache(&cache_path);
                    let mut ctx = ResolverContext::new();
                    resolve_imports_recursive(
                        &items,
                        dep_base_dir,
                        &mut ctx,
                        &mut cache,
                        module_env,
                    )?;
                    save_cache(&cache_path, &cache);
                    register_imported_items(&items, Some(dep_name), module_env);
                    // P5-C: Check for proof certificate before marking atoms as verified
                    // Use pkg_dir (package root) instead of dep_base_dir (entry file parent)
                    // because cmd_publish saves certificates to the package root.
                    let cert_results = verify_import_certificate(&pkg_dir, entry_path, &items);
                    mark_dependency_atoms_with_cert(
                        &items,
                        dep_name,
                        &cert_results,
                        module_env,
                        strict_imports,
                    )?;
                    println!(
                        "  📦 Dependency '{}': loaded from registry ({})",
                        dep_name,
                        pkg_dir.display()
                    );
                } else {
                    eprintln!(
                        "  ⚠️  Dependency '{}': found in registry but no entry file in '{}'",
                        dep_name,
                        pkg_dir.display()
                    );
                }
            } else {
                eprintln!("  ⚠️  Dependency '{}': not found in local registry. Run `mumei publish` in the dependency project first.", dep_name);
            }
        }
    }
    Ok(())
}

// =============================================================================
// 検証キャッシュの永続化
// =============================================================================

/// ソースコードの SHA-256 ハッシュを計算する
fn compute_hash(source: &str) -> String {
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
}

/// Compute a proof hash that includes transitive dependency signatures and type predicates.
/// This extends compute_atom_hash with callee signatures and type predicate content.
pub fn compute_proof_hash(atom: &crate::parser::Atom, module_env: &ModuleEnv) -> String {
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
                    },
                );
            }
            save_verification_cache(base_dir, &new_cache);
        }
    }
    let _ = fs::remove_file(&old_path);
}

/// Simple timestamp string for cache entries.
fn chrono_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}s", duration.as_secs())
}

/// キャッシュファイルを読み込む。存在しない場合は空のキャッシュを返す。
fn load_cache(cache_path: &Path) -> VerificationCache {
    fs::read_to_string(cache_path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

/// キャッシュファイルに書き込む。書き込み失敗は無視する（キャッシュは最適化であり必須ではない）。
fn save_cache(cache_path: &Path, cache: &VerificationCache) {
    if let Ok(json) = serde_json::to_string_pretty(cache) {
        let _ = fs::write(cache_path, json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{self, TrustLevel};

    /// P1-A: ExternBlock → trusted atom 自動登録テスト
    #[test]
    fn test_register_extern_block_as_trusted_atoms() {
        let source = r#"
extern "Rust" {
    fn json_parse(input: String) -> String;
    fn json_stringify(obj: String) -> String;
}
"#;
        let items = parser::parse_module(source);
        let mut module_env = ModuleEnv::new();

        register_imported_items(&items, None, &mut module_env);

        // extern 関数が trusted atom として登録されていること
        let json_parse = module_env.get_atom("json_parse");
        assert!(
            json_parse.is_some(),
            "json_parse should be registered as atom"
        );
        let atom = json_parse.unwrap();
        assert_eq!(atom.trust_level, TrustLevel::Trusted);
        assert_eq!(atom.params.len(), 1);
        assert_eq!(atom.params[0].type_name, Some("String".to_string()));

        let json_stringify = module_env.get_atom("json_stringify");
        assert!(
            json_stringify.is_some(),
            "json_stringify should be registered as atom"
        );
        assert_eq!(json_stringify.unwrap().trust_level, TrustLevel::Trusted);
    }

    /// P1-A: ExternBlock with alias → FQN 登録テスト
    #[test]
    fn test_register_extern_block_with_alias() {
        let source = r#"
extern "Rust" {
    fn http_get(url: String) -> String;
}
"#;
        let items = parser::parse_module(source);
        let mut module_env = ModuleEnv::new();

        register_imported_items(&items, Some("http"), &mut module_env);

        // 基本名でも FQN でもアクセスできること
        assert!(
            module_env.get_atom("http_get").is_some(),
            "base name should be registered"
        );
        assert!(
            module_env.get_atom("http::http_get").is_some(),
            "FQN should be registered"
        );

        // FQN 版も trusted であること
        let fqn_atom = module_env.get_atom("http::http_get").unwrap();
        assert_eq!(fqn_atom.trust_level, TrustLevel::Trusted);
    }

    /// P1-A: ExternBlock の複数パラメータテスト
    #[test]
    fn test_extern_block_multi_param() {
        let source = r#"
extern "Rust" {
    fn http_post(url: String, body: String) -> String;
}
"#;
        let items = parser::parse_module(source);
        let mut module_env = ModuleEnv::new();

        register_imported_items(&items, None, &mut module_env);

        let atom = module_env.get_atom("http_post").unwrap();
        assert_eq!(atom.params.len(), 2);
        assert_eq!(atom.params[0].name, "url");
        assert_eq!(atom.params[0].type_name, Some("String".to_string()));
        assert_eq!(atom.params[1].name, "body");
        assert_eq!(atom.params[1].type_name, Some("String".to_string()));
        assert_eq!(atom.requires, "true");
        assert_eq!(atom.ensures, "true");
        assert!(atom.body_expr.is_empty());
    }

    /// P1-A: C 言語 ExternBlock テスト
    #[test]
    fn test_register_extern_block_c_language() {
        let source = r#"
extern "C" {
    fn printf(fmt: i64) -> i64;
}
"#;
        let items = parser::parse_module(source);
        let mut module_env = ModuleEnv::new();

        register_imported_items(&items, None, &mut module_env);

        let atom = module_env.get_atom("printf");
        assert!(atom.is_some(), "C extern function should be registered");
        assert_eq!(atom.unwrap().trust_level, TrustLevel::Trusted);
    }

    /// P1-A: 通常 atom + ExternBlock 混合テスト
    #[test]
    fn test_register_mixed_items_with_extern() {
        let source = r#"
atom add(x: i64, y: i64) -> i64
  requires: true;
  ensures: result == x + y;
  body: x + y;

extern "Rust" {
    fn ffi_helper(n: i64) -> i64;
}
"#;
        let items = parser::parse_module(source);
        let mut module_env = ModuleEnv::new();

        register_imported_items(&items, None, &mut module_env);

        // 通常 atom が登録されていること
        let add = module_env.get_atom("add");
        assert!(add.is_some(), "regular atom should be registered");
        assert_eq!(add.unwrap().trust_level, TrustLevel::Verified);

        // extern atom が trusted で登録されていること
        let ffi = module_env.get_atom("ffi_helper");
        assert!(ffi.is_some(), "extern atom should be registered");
        assert_eq!(ffi.unwrap().trust_level, TrustLevel::Trusted);
    }

    /// compute_hash のテスト
    #[test]
    fn test_compute_hash_deterministic() {
        let hash1 = compute_hash("hello world");
        let hash2 = compute_hash("hello world");
        assert_eq!(hash1, hash2, "same input should produce same hash");

        let hash3 = compute_hash("different");
        assert_ne!(
            hash1, hash3,
            "different input should produce different hash"
        );
    }

    // --- Feature 2: Dependency graph and proof hash tests ---

    /// Test collect_callees_from_body extracts function names
    #[test]
    fn test_collect_callees_from_body() {
        let body = "foo(x) + bar(y, z)";
        let callees = collect_callees_from_body(body);
        assert!(callees.contains("foo"), "should find foo");
        assert!(callees.contains("bar"), "should find bar");
        assert!(!callees.contains("x"), "should not include variable x");
    }

    /// Test collect_callees_from_body skips keywords
    #[test]
    fn test_collect_callees_skips_keywords() {
        let body = "if(cond) { while(true) { match(x) { foo(1) } } }";
        let callees = collect_callees_from_body(body);
        assert!(!callees.contains("if"), "should skip keyword 'if'");
        assert!(!callees.contains("while"), "should skip keyword 'while'");
        assert!(!callees.contains("match"), "should skip keyword 'match'");
        assert!(callees.contains("foo"), "should find foo");
    }

    /// Test compute_proof_hash is deterministic
    #[test]
    fn test_compute_proof_hash_deterministic() {
        let source = r#"
atom add(x: i64, y: i64) -> i64
  requires: x >= 0;
  ensures: result == x + y;
  body: x + y;
"#;
        let items = parser::parse_module(source);
        let module_env = ModuleEnv::new();

        for item in &items {
            if let parser::Item::Atom(atom) = item {
                let hash1 = compute_proof_hash(atom, &module_env);
                let hash2 = compute_proof_hash(atom, &module_env);
                assert_eq!(hash1, hash2, "same atom should produce same proof hash");
            }
        }
    }

    /// Test compute_proof_hash changes when callee signature changes
    #[test]
    fn test_proof_hash_includes_callee_signature() {
        let source = r#"
atom helper(x: i64) -> i64
  requires: x >= 0;
  ensures: result >= 0;
  body: x;

atom caller(n: i64) -> i64
  requires: n >= 0;
  ensures: result >= 0;
  body: helper(n);
"#;
        let items = parser::parse_module(source);
        let mut module_env = ModuleEnv::new();

        // Register atoms and dependencies
        for item in &items {
            if let parser::Item::Atom(atom) = item {
                module_env.register_atom(atom);
                let callees = collect_callees_from_body(&atom.body_expr);
                module_env.register_dependencies(&atom.name, callees);
            }
        }

        // Compute hash for caller
        let caller_atom = module_env.get_atom("caller").unwrap().clone();
        let hash1 = compute_proof_hash(&caller_atom, &module_env);

        // Now change helper's ensures and re-register
        let source2 = r#"
atom helper(x: i64) -> i64
  requires: x >= 0;
  ensures: result >= 1;
  body: x;

atom caller(n: i64) -> i64
  requires: n >= 0;
  ensures: result >= 0;
  body: helper(n);
"#;
        let items2 = parser::parse_module(source2);
        let mut module_env2 = ModuleEnv::new();
        for item in &items2 {
            if let parser::Item::Atom(atom) = item {
                module_env2.register_atom(atom);
                let callees = collect_callees_from_body(&atom.body_expr);
                module_env2.register_dependencies(&atom.name, callees);
            }
        }

        let caller_atom2 = module_env2.get_atom("caller").unwrap().clone();
        let hash2 = compute_proof_hash(&caller_atom2, &module_env2);

        assert_ne!(
            hash1, hash2,
            "proof hash should change when callee ensures changes"
        );
    }

    /// Test dependency graph transitive dependents
    #[test]
    fn test_dependency_graph_transitive_dependents() {
        let mut module_env = ModuleEnv::new();

        // A calls B, B calls C => changing C should affect both A and B
        let mut callees_a = std::collections::HashSet::new();
        callees_a.insert("B".to_string());
        module_env.register_dependencies("A", callees_a);

        let mut callees_b = std::collections::HashSet::new();
        callees_b.insert("C".to_string());
        module_env.register_dependencies("B", callees_b);

        let dependents_of_c = module_env.get_transitive_dependents("C");
        assert!(dependents_of_c.contains("B"), "B directly depends on C");
        assert!(
            dependents_of_c.contains("A"),
            "A transitively depends on C via B"
        );

        let dependents_of_b = module_env.get_transitive_dependents("B");
        assert!(dependents_of_b.contains("A"), "A directly depends on B");
        assert!(!dependents_of_b.contains("C"), "C does not depend on B");
    }

    /// Test verification cache load/save roundtrip
    #[test]
    fn test_verification_cache_roundtrip() {
        let base_dir =
            std::env::temp_dir().join(format!("mumei_test_cache_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&base_dir);
        let base_dir = base_dir.as_path();

        let mut cache = HashMap::new();
        cache.insert(
            "test_atom".to_string(),
            VerificationCacheEntry {
                proof_hash: "abc123".to_string(),
                result: "verified".to_string(),
                dependencies: vec!["dep1".to_string()],
                type_deps: vec!["Nat".to_string()],
                timestamp: "1234567890s".to_string(),
            },
        );

        save_verification_cache(base_dir, &cache);
        let loaded = load_verification_cache(base_dir);

        assert_eq!(loaded.len(), 1);
        assert!(loaded.contains_key("test_atom"));
        let entry = &loaded["test_atom"];
        assert_eq!(entry.proof_hash, "abc123");
        assert_eq!(entry.result, "verified");
        assert_eq!(entry.dependencies, vec!["dep1"]);
        assert_eq!(entry.type_deps, vec!["Nat"]);
        let _ = std::fs::remove_dir_all(base_dir);
    }

    /// Test invalidate_dependents removes transitive entries
    #[test]
    fn test_invalidate_dependents() {
        let mut module_env = ModuleEnv::new();

        // A calls B, B calls C
        let mut callees_a = std::collections::HashSet::new();
        callees_a.insert("B".to_string());
        module_env.register_dependencies("A", callees_a);

        let mut callees_b = std::collections::HashSet::new();
        callees_b.insert("C".to_string());
        module_env.register_dependencies("B", callees_b);

        let mut cache = HashMap::new();
        for name in &["A", "B", "C"] {
            cache.insert(
                name.to_string(),
                VerificationCacheEntry {
                    proof_hash: format!("hash_{}", name),
                    result: "verified".to_string(),
                    dependencies: vec![],
                    type_deps: vec![],
                    timestamp: "0s".to_string(),
                },
            );
        }

        // Invalidate dependents of C
        invalidate_dependents(&mut cache, "C", &module_env);

        // C itself should still be in cache (we only invalidate dependents)
        assert!(cache.contains_key("C"), "C itself should remain");
        // A and B depend on C, so they should be invalidated
        assert!(
            !cache.contains_key("B"),
            "B depends on C and should be invalidated"
        );
        assert!(
            !cache.contains_key("A"),
            "A transitively depends on C and should be invalidated"
        );
    }

    /// Test migrate_old_cache creates new cache directory
    #[test]
    fn test_migrate_old_cache() {
        let base_dir =
            std::env::temp_dir().join(format!("mumei_test_migrate_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&base_dir);
        let base_dir = base_dir.as_path();

        // Create old-style cache
        let old_cache_path = base_dir.join(".mumei_build_cache");
        let mut old_cache = HashMap::new();
        old_cache.insert("my_atom".to_string(), "oldhash123".to_string());
        let json = serde_json::to_string(&old_cache).unwrap();
        std::fs::write(&old_cache_path, json).unwrap();

        // Run migration
        migrate_old_cache(base_dir);

        // Old file should be deleted
        assert!(
            !old_cache_path.exists(),
            "old cache file should be deleted after migration"
        );

        // New cache should exist
        let new_cache = load_verification_cache(base_dir);
        assert!(
            new_cache.contains_key("my_atom"),
            "migrated atom should exist in new cache"
        );
        assert_eq!(new_cache["my_atom"].proof_hash, "oldhash123");
        assert_eq!(new_cache["my_atom"].result, "verified");
        let _ = std::fs::remove_dir_all(base_dir);
    }

    /// P5-C: check_cert_for_atom returns true for "proven" status
    #[test]
    fn test_check_cert_for_atom_proven() {
        let mut results = HashMap::new();
        results.insert("my_atom".to_string(), "proven".to_string());
        assert!(check_cert_for_atom(&results, "my_atom"));
    }

    /// P5-C: check_cert_for_atom returns false for "changed" status
    #[test]
    fn test_check_cert_for_atom_changed() {
        let mut results = HashMap::new();
        results.insert("my_atom".to_string(), "changed".to_string());
        assert!(!check_cert_for_atom(&results, "my_atom"));
    }

    /// P5-C: check_cert_for_atom returns false for "unproven" status
    #[test]
    fn test_check_cert_for_atom_unproven() {
        let mut results = HashMap::new();
        results.insert("my_atom".to_string(), "unproven".to_string());
        assert!(!check_cert_for_atom(&results, "my_atom"));
    }

    /// P5-C: check_cert_for_atom returns false for missing atom
    #[test]
    fn test_check_cert_for_atom_missing() {
        let results = HashMap::new();
        assert!(!check_cert_for_atom(&results, "nonexistent"));
    }

    /// P5-C: mark_dependency_atoms_with_cert verifies atoms with proven cert
    #[test]
    fn test_mark_dependency_atoms_with_cert_verified() {
        let source = r#"
atom add(x: i64) -> i64
  requires true
  ensures result >= 0
{
  x + 1
}
"#;
        let items = parser::parse_module(source);
        let mut module_env = ModuleEnv::new();
        register_imported_items(&items, Some("dep"), &mut module_env);

        let mut cert_results = HashMap::new();
        cert_results.insert("add".to_string(), "proven".to_string());

        mark_dependency_atoms_with_cert(&items, "dep", &Some(cert_results), &mut module_env, false)
            .unwrap();

        // The atom should be marked as verified
        assert!(module_env.is_verified("add"));
        assert!(module_env.is_verified("dep::add"));
    }

    /// P5-C: mark_dependency_atoms_with_cert marks atoms unverified on failed cert
    #[test]
    fn test_mark_dependency_atoms_with_cert_unverified() {
        let source = r#"
atom add(x: i64) -> i64
  requires true
  ensures result >= 0
{
  x + 1
}
"#;
        let items = parser::parse_module(source);
        let mut module_env = ModuleEnv::new();
        register_imported_items(&items, Some("dep"), &mut module_env);

        let mut cert_results = HashMap::new();
        cert_results.insert("add".to_string(), "changed".to_string());

        mark_dependency_atoms_with_cert(&items, "dep", &Some(cert_results), &mut module_env, false)
            .unwrap();

        // The atom should NOT be marked as verified
        assert!(!module_env.is_verified("add"));
    }

    /// P5-C: mark_dependency_atoms_with_cert with None cert (legacy) verifies all
    #[test]
    fn test_mark_dependency_atoms_with_cert_legacy() {
        let source = r#"
atom foo(x: i64) -> i64
  requires true
  ensures true
{
  x
}
"#;
        let items = parser::parse_module(source);
        let mut module_env = ModuleEnv::new();
        register_imported_items(&items, Some("legacy_dep"), &mut module_env);

        mark_dependency_atoms_with_cert(&items, "legacy_dep", &None, &mut module_env, false)
            .unwrap();

        // Legacy behavior: no cert = all verified
        assert!(module_env.is_verified("foo"));
        assert!(module_env.is_verified("legacy_dep::foo"));
    }

    /// P5-C: ResolverContext strict_imports defaults to false
    #[test]
    fn test_resolver_context_strict_imports_default() {
        let ctx = ResolverContext::new();
        assert!(!ctx.strict_imports);
    }

    /// Verify that verify_import_certificate logic collects ImplBlock methods
    /// with qualified names so they match cert entries like "Stack::push".
    #[test]
    fn test_verify_import_cert_collects_impl_block_methods() {
        use crate::proof_cert;

        // Source with an impl block containing a method
        let source = r#"
struct Stack { top: i64 }
impl Stack {
    atom push(self, val: i64) -> i64
      requires true
      ensures result >= 0
    {
      val
    }
}
"#;
        let items = parser::parse_module(source);

        // Replicate the collection logic from verify_import_certificate
        let mut atom_refs: Vec<&parser::Atom> = items
            .iter()
            .filter_map(|item| match item {
                Item::Atom(a) => Some(a),
                _ => None,
            })
            .collect();

        let mut qualified_methods: Vec<parser::Atom> = Vec::new();
        for item in &items {
            if let Item::ImplBlock(ib) = item {
                for method in &ib.methods {
                    let mut qualified = method.clone();
                    qualified.name = format!("{}::{}", ib.struct_name, method.name);
                    qualified_methods.push(qualified);
                }
            }
        }
        for qm in &qualified_methods {
            atom_refs.push(qm);
        }

        // The collected refs should contain "Stack::push"
        let names: Vec<&str> = atom_refs.iter().map(|a| a.name.as_str()).collect();
        assert!(
            names.contains(&"Stack::push"),
            "Expected 'Stack::push' in atom_refs, got: {:?}",
            names
        );

        // Generate a cert from these atoms, then verify it recognizes the method
        let mut cert_results = std::collections::HashMap::new();
        for a in &atom_refs {
            cert_results.insert(
                a.name.clone(),
                ("unsat".to_string(), "verified".to_string()),
            );
        }
        let module_env = crate::resolver::ModuleEnv::new();
        let cert = proof_cert::generate_certificate(
            "test_impl.mm",
            &atom_refs,
            &cert_results,
            &module_env,
            None,
            None,
        );

        // Verify that the cert contains the qualified method name
        let cert_names: Vec<&str> = cert.atoms.iter().map(|a| a.name.as_str()).collect();
        assert!(
            cert_names.contains(&"Stack::push"),
            "Expected 'Stack::push' in cert atoms, got: {:?}",
            cert_names
        );

        // Now verify_certificate should report "proven" for "Stack::push"
        let results = proof_cert::verify_certificate(&cert, &atom_refs);
        let result_map: std::collections::HashMap<String, String> = results.into_iter().collect();
        assert_eq!(
            result_map.get("Stack::push").map(|s| s.as_str()),
            Some("proven"),
            "Expected 'Stack::push' to be 'proven', got: {:?}",
            result_map.get("Stack::push")
        );
    }
}
