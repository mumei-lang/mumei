use crate::parser::{self, Item};
use crate::proof_cert;
use crate::verification::{ModuleEnv, MumeiError, MumeiResult};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::cache::{compute_hash, load_cache, save_cache, CacheEntry, VerificationCache};
use super::metrics::LeanEscalationMetrics;

pub(crate) struct ResolverContext {
    /// ロード中のモジュールパス集合（循環参照検出用）
    loading: HashSet<PathBuf>,
    /// 完全にロード済みのモジュール（キャッシュ）
    loaded: HashMap<PathBuf, Vec<Item>>,
    /// P5-C: When true, missing/invalid certificates cause hard errors instead of warnings
    pub(crate) strict_imports: bool,
    /// PR 2: When true, certificates carrying `z3_check_result == "lean_verified"`
    /// (emitted by mumei-lean) are accepted as `"proven"`. Default `false`
    /// preserves the strict Z3-only contract.
    pub(crate) allow_lean_verified: bool,
}
impl ResolverContext {
    pub(crate) fn new() -> Self {
        Self {
            loading: HashSet::new(),
            loaded: HashMap::new(),
            strict_imports: false,
            allow_lean_verified: false,
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
    resolve_imports_with_full_options(items, base_dir, module_env, strict_imports, false)
}

/// PR 2: `resolve_imports` with both `strict_imports` and `allow_lean_verified`
/// opt-ins. `allow_lean_verified == true` widens the resolver so it accepts
/// mumei-lean-emitted certificates (`z3_check_result == "lean_verified"`) as
/// `"proven"`. Off by default.
pub fn resolve_imports_with_full_options(
    items: &[Item],
    base_dir: &Path,
    module_env: &mut ModuleEnv,
    strict_imports: bool,
    allow_lean_verified: bool,
) -> MumeiResult<()> {
    let cache_path = base_dir.join(".mumei_cache");
    let mut cache = load_cache(&cache_path);
    let mut ctx = ResolverContext::new();
    ctx.strict_imports = strict_imports;
    ctx.allow_lean_verified = allow_lean_verified;
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
pub(crate) fn check_cert_for_atom(cert_results: &HashMap<String, String>, atom_name: &str) -> bool {
    cert_results
        .get(atom_name)
        .is_some_and(|status| status == "proven")
}

pub(crate) fn unproven_cert_results(atom_refs: &[&parser::Atom]) -> HashMap<String, String> {
    atom_refs
        .iter()
        .map(|atom| (atom.name.clone(), "unproven".to_string()))
        .collect()
}

/// Collect atom references (including ImplBlock methods with qualified names)
/// from a parsed module. Shared between local certificate verification and
/// SI-5 Phase 3-C bundle-fallback verification so both paths recognise
/// methods like `Stack::push`.
pub(crate) fn collect_atom_refs_with_methods<'a>(
    items: &'a [Item],
    qualified_methods: &'a mut Vec<parser::Atom>,
) -> Vec<&'a parser::Atom> {
    let mut atom_refs: Vec<&parser::Atom> = items
        .iter()
        .filter_map(|item| match item {
            Item::Atom(a) => Some(a),
            _ => None,
        })
        .collect();

    for item in items {
        if let Item::ImplBlock(ib) = item {
            for method in &ib.methods {
                let mut qualified = method.clone();
                qualified.name = format!("{}::{}", ib.struct_name, method.name);
                qualified_methods.push(qualified);
            }
        }
    }
    for qm in qualified_methods.iter() {
        atom_refs.push(qm);
    }
    atom_refs
}

/// Task 1-B: Audit-log every atom that was accepted as `"proven"` solely
/// because of the `--allow-lean-verified` opt-in (i.e. its
/// `z3_check_result` is `"lean_verified"` rather than `"unsat"`).
///
/// This is a no-op when `allow_lean_verified == false`, when the
/// certificate has no `lean_verified` atoms, or when those atoms did not
/// end up `"proven"` (e.g. content_hash mismatch ⇒ `"changed"`).
pub(crate) fn log_lean_verified_acceptance(
    cert: &proof_cert::ProofCertificate,
    results: &[(String, String)],
    allow_lean_verified: bool,
) -> LeanEscalationMetrics {
    let mut metrics = LeanEscalationMetrics::default();
    if !allow_lean_verified {
        return metrics;
    }
    let proven: HashMap<&str, &str> = results
        .iter()
        .map(|(name, status)| (name.as_str(), status.as_str()))
        .collect();
    for atom in &cert.atoms {
        metrics.record_atom_certificate(atom);
        let lean_status = atom
            .lean_metadata
            .as_ref()
            .map(|metadata| metadata.status.as_str())
            .unwrap_or(atom.z3_check_result.as_str());
        if atom.z3_check_result == "lean_verified"
            && proven.get(atom.name.as_str()).copied() == Some("proven")
        {
            metrics.record_lean_verified_acceptance(atom);
            eprintln!(
                "  Lean-verified atom '{}' accepted as proven (--allow-lean-verified)",
                atom.name,
            );
        } else if lean_status == "partial_translation" {
            metrics.partial_translation += 1;
        } else if atom.escalation_reason.is_some() {
            metrics.manual_required += 1;
        }
    }
    if metrics.has_activity() {
        eprintln!(
            "  Lean escalation metrics: attempts={}, successes={}, partial_translation={}, manual_required={}",
            metrics.escalation_attempts,
            metrics.lean_successes,
            metrics.partial_translation,
            metrics.manual_required,
        );
    }
    metrics
}

/// P5-C: Attempt to verify a proof certificate for an imported module directory.
/// Returns a map of atom_name -> status ("proven", "changed", "unproven", "missing").
/// Returns None if no certificate exists.
///
/// Search order:
///   1. `module_dir/.proof-cert.json`
///   2. `module_dir/proof_certificate.json`
///   3. SI-5 Phase 3-C fallback: the bundle JSON referenced by the
///      `MUMEI_PROOF_BUNDLE` environment variable. The bundle is produced
///      by `scripts/bundle_std_certs.py` and distributed via
///      `homebrew-mumei`, so downstream projects can verify `std/*`
///      imports without shipping per-module `.proof-cert.json` files.
pub(crate) fn verify_import_certificate(
    module_dir: &Path,
    source_file: &Path,
    items: &[Item],
    allow_lean_verified: bool,
) -> Option<HashMap<String, String>> {
    let mut qualified_methods: Vec<parser::Atom> = Vec::new();
    let atom_refs = collect_atom_refs_with_methods(items, &mut qualified_methods);

    // 1. & 2. Local certificate in the module directory.
    let cert_candidates = [
        module_dir.join(".proof-cert.json"),
        module_dir.join("proof_certificate.json"),
    ];
    if let Some(cert_path) = cert_candidates.iter().find(|p| p.exists()) {
        match proof_cert::load_certificate_unvalidated(cert_path) {
            Ok(cert) => {
                // Check if the certificate matches this source file.
                let cert_file_path = Path::new(&cert.file);
                let source_matches = source_file.ends_with(cert_file_path)
                    || cert_file_path.ends_with(source_file.file_name().unwrap_or_default());
                if source_matches || cert.file.is_empty() {
                    if let Err(err) = proof_cert::validate_certificate_translator_versions(&cert) {
                        eprintln!(
                            "  ⚠️  Local certificate {} has invalid Lean translator metadata: {}",
                            cert_path.display(),
                            err,
                        );
                        return Some(unproven_cert_results(&atom_refs));
                    }
                    let results =
                        proof_cert::verify_certificate(&cert, &atom_refs, allow_lean_verified);
                    let mut metrics = LeanEscalationMetrics::default();
                    metrics.merge(log_lean_verified_acceptance(
                        &cert,
                        &results,
                        allow_lean_verified,
                    ));
                    return Some(results.into_iter().collect());
                }
                // Certificate is for a different file — fall through to the
                // bundle fallback rather than silently returning None.
            }
            Err(err) => {
                // Local cert exists but is corrupted/unparseable — do NOT
                // fall through to the bundle, because that would silently
                // paper over the problem (especially dangerous under
                // strict_imports).
                eprintln!(
                    "  ⚠️  Local certificate {} could not be parsed: {}",
                    cert_path.display(),
                    err,
                );
                return None;
            }
        }
    }

    // 3. SI-5 Phase 3-C: MUMEI_PROOF_BUNDLE fallback.
    if let Ok(bundle_path_str) = std::env::var("MUMEI_PROOF_BUNDLE") {
        let bundle_path = Path::new(&bundle_path_str);
        if bundle_path.exists() {
            match proof_cert::load_bundle(bundle_path) {
                Ok(bundle) => {
                    if let Some(cert) = proof_cert::lookup_bundle_certificate(&bundle, source_file)
                    {
                        if let Err(err) = proof_cert::validate_certificate_translator_versions(cert)
                        {
                            eprintln!(
                                "  ⚠️  MUMEI_PROOF_BUNDLE certificate for {} has invalid Lean translator metadata: {}",
                                source_file.display(),
                                err,
                            );
                            return Some(unproven_cert_results(&atom_refs));
                        }
                        let results =
                            proof_cert::verify_certificate(cert, &atom_refs, allow_lean_verified);
                        let mut metrics = LeanEscalationMetrics::default();
                        metrics.merge(log_lean_verified_acceptance(
                            cert,
                            &results,
                            allow_lean_verified,
                        ));
                        return Some(results.into_iter().collect());
                    }
                }
                Err(err) => {
                    eprintln!(
                        "  ⚠️  MUMEI_PROOF_BUNDLE at {} could not be parsed: {}",
                        bundle_path.display(),
                        err,
                    );
                }
            }
        } else {
            eprintln!(
                "  ⚠️  MUMEI_PROOF_BUNDLE points to missing file: {}",
                bundle_path.display(),
            );
        }
    }

    None
}

/// P5-C: Mark dependency atoms as verified or unverified based on cert verification results.
/// Used by resolve_manifest_dependencies() for all dependency types.
/// When strict_imports is true and no cert exists or cert verification fails, returns an error.
pub(crate) fn mark_dependency_atoms_with_cert(
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
pub(crate) fn resolve_imports_recursive(
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
            let cert_results = verify_import_certificate(
                import_base_dir,
                &resolved_path,
                &imported_items,
                ctx.allow_lean_verified,
            );

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
pub(crate) fn register_imported_items(
    items: &[Item],
    alias: Option<&str>,
    module_env: &mut ModuleEnv,
) {
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
                module_env.register_extern_block(extern_block);
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
                        trace_id: None,
                        spec_metadata: std::collections::HashMap::new(),
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
pub(crate) fn resolve_path(import_path: &str, base_dir: &Path) -> MumeiResult<PathBuf> {
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
