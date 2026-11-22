use crate::parser;
use crate::verification::{ModuleEnv, MumeiError, MumeiResult};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::cache::{load_cache, save_cache};
use super::imports::{
    mark_dependency_atoms_with_cert, record_import_closure, register_imported_alias_names,
    register_imported_items_with_source, resolve_imports_recursive, verify_import_certificate,
    ResolverContext,
};

struct DeferredCertificateCheck {
    dependency_name: String,
    package_alias: Option<String>,
    module_dir: PathBuf,
    entry_path: PathBuf,
    items: Vec<parser::Item>,
    load_message: Option<String>,
    spec: crate::manifest::Dependency,
}

struct LoadedDep {
    entry_path: PathBuf,
    module_dir: PathBuf,
    spec: crate::manifest::Dependency,
    dependency_name: String,
    package_name: Option<String>,
}

fn same_module_dir(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(a) == canon(b)
}

fn package_name_alias(
    package_dir: &Path,
    dependency_name: &str,
    manifest: &crate::manifest::Manifest,
) -> Option<String> {
    crate::manifest::load(&package_dir.join("mumei.toml"))
        .ok()
        .map(|manifest| manifest.package.name)
        .filter(|package_name| {
            package_name != dependency_name && !manifest.dependencies.contains_key(package_name)
        })
}

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
    resolve_manifest_dependencies_with_full_options(
        manifest,
        project_dir,
        module_env,
        strict_imports,
        false,
    )
}

/// PR 2: `resolve_manifest_dependencies` with both `strict_imports` and
/// `allow_lean_verified` opt-ins. See [`resolve_imports_with_full_options`].
///
/// Task 1-B: `strict_imports` is now propagated into the
/// `ResolverContext` used for sub-imports inside each dependency branch
/// (path / git / registry). Previously each branch only forwarded
/// `allow_lean_verified` and left `ctx.strict_imports` at its default
/// `false`, which silently weakened strict-mode semantics for transitive
/// imports. Direct dependency atoms were always subject to strict checks
/// via `mark_dependency_atoms_with_cert`; this restores the same
/// guarantee for indirect imports as well.
pub fn resolve_manifest_dependencies_with_full_options(
    manifest: &crate::manifest::Manifest,
    project_dir: &Path,
    module_env: &mut ModuleEnv,
    strict_imports: bool,
    allow_lean_verified: bool,
) -> MumeiResult<()> {
    resolve_manifest_dependencies_with_full_options_and_persistence(
        manifest,
        project_dir,
        module_env,
        strict_imports,
        allow_lean_verified,
        true,
    )
}

pub fn resolve_manifest_dependencies_with_full_options_and_persistence(
    manifest: &crate::manifest::Manifest,
    project_dir: &Path,
    module_env: &mut ModuleEnv,
    strict_imports: bool,
    allow_lean_verified: bool,
    persist_cache: bool,
) -> MumeiResult<()> {
    resolve_manifest_dependencies_with_full_options_persistence_and_source(
        manifest,
        project_dir,
        module_env,
        strict_imports,
        allow_lean_verified,
        persist_cache,
        None,
    )
}

/// Same as [`resolve_manifest_dependencies_with_full_options_and_persistence`]
/// but records each dependency entry file in `source_file`'s
/// [`ModuleEnv::import_closure`] entry, so atoms in the root file treat
/// manifest dependencies as part of their tracked import closure — the same
/// way `import` statements do. Without it, manifest-loaded definitions would
/// fall outside the root closure and be dropped from dependency keys even
/// though module-wide checks see them.
pub fn resolve_manifest_dependencies_with_full_options_persistence_and_source(
    manifest: &crate::manifest::Manifest,
    project_dir: &Path,
    module_env: &mut ModuleEnv,
    strict_imports: bool,
    allow_lean_verified: bool,
    persist_cache: bool,
    source_file: Option<&str>,
) -> MumeiResult<()> {
    let mut deferred_certificates = Vec::new();

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
                ctx.allow_lean_verified = allow_lean_verified;
                ctx.strict_imports = strict_imports;
                resolve_imports_recursive(
                    &items,
                    dep_base_dir,
                    &mut ctx,
                    &mut cache,
                    module_env,
                    entry_path.to_str(),
                )?;
                // Manifest dependencies load into the root file's module
                // environment just like `import` statements do, so record the
                // entry edge in the root's import closure — otherwise the
                // dep's definitions fall outside the tracked closure and are
                // dropped from the root atoms' dependency keys.
                record_import_closure(module_env, source_file, entry_path.to_str());
                if persist_cache {
                    save_cache(&cache_path, &cache);
                }
                let package_alias = package_name_alias(&abs_path, dep_name, manifest);
                register_imported_items_with_source(
                    &items,
                    Some(dep_name),
                    module_env,
                    entry_path.to_str(),
                );
                deferred_certificates.push(DeferredCertificateCheck {
                    dependency_name: dep_name.clone(),
                    package_alias,
                    module_dir: abs_path,
                    entry_path: entry_path.to_path_buf(),
                    items,
                    spec: dep.clone(),
                    load_message: Some(format!(
                        "  📦 Dependency '{}': loaded from {}",
                        dep_name,
                        entry_path.display()
                    )),
                });
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
                ctx.allow_lean_verified = allow_lean_verified;
                ctx.strict_imports = strict_imports;
                resolve_imports_recursive(
                    &items,
                    dep_base_dir,
                    &mut ctx,
                    &mut cache,
                    module_env,
                    entry_path.to_str(),
                )?;
                // Manifest dependencies load into the root file's module
                // environment just like `import` statements do, so record the
                // entry edge in the root's import closure — otherwise the
                // dep's definitions fall outside the tracked closure and are
                // dropped from the root atoms' dependency keys.
                record_import_closure(module_env, source_file, entry_path.to_str());
                if persist_cache {
                    save_cache(&cache_path, &cache);
                }
                let package_alias = package_name_alias(&clone_dir, dep_name, manifest);
                register_imported_items_with_source(
                    &items,
                    Some(dep_name),
                    module_env,
                    entry_path.to_str(),
                );
                deferred_certificates.push(DeferredCertificateCheck {
                    dependency_name: dep_name.clone(),
                    package_alias,
                    module_dir: clone_dir,
                    entry_path: entry_path.to_path_buf(),
                    items,
                    spec: dep.clone(),
                    load_message: None,
                });
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
            let local = crate::registry::resolve(dep_name, version);
            let resolved = match local {
                Some(dir) => Some(dir),
                // P24: fall back to the configured remote registry. Opt-in —
                // without `[registry] url` / `MUMEI_REGISTRY_URL` this is a no-op.
                None => crate::registry::remote::resolve(
                    &manifest.registry,
                    dep_name,
                    version,
                    strict_imports,
                )
                .map_err(|e| {
                    MumeiError::verification(format!("Dependency '{}': {}", dep_name, e))
                })?,
            };
            if let Some(pkg_dir) = resolved {
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
                    ctx.allow_lean_verified = allow_lean_verified;
                    ctx.strict_imports = strict_imports;
                    resolve_imports_recursive(
                        &items,
                        dep_base_dir,
                        &mut ctx,
                        &mut cache,
                        module_env,
                        entry_path.to_str(),
                    )?;
                    // Manifest dependencies load into the root file's module
                    // environment just like `import` statements do, so record
                    // the entry edge in the root's import closure — otherwise
                    // the dep's definitions fall outside the tracked closure
                    // and are dropped from the root atoms' dependency keys.
                    record_import_closure(module_env, source_file, entry_path.to_str());
                    if persist_cache {
                        save_cache(&cache_path, &cache);
                    }
                    let package_alias = package_name_alias(&pkg_dir, dep_name, manifest);
                    register_imported_items_with_source(
                        &items,
                        Some(dep_name),
                        module_env,
                        entry_path.to_str(),
                    );
                    deferred_certificates.push(DeferredCertificateCheck {
                        dependency_name: dep_name.clone(),
                        package_alias,
                        module_dir: pkg_dir.clone(),
                        entry_path: entry_path.to_path_buf(),
                        items,
                        spec: dep.clone(),
                        load_message: Some(format!(
                            "  📦 Dependency '{}': loaded from registry ({})",
                            dep_name,
                            pkg_dir.display()
                        )),
                    });
                } else {
                    eprintln!(
                        "  ⚠️  Dependency '{}': found in registry but no entry file in '{}'",
                        dep_name,
                        pkg_dir.display()
                    );
                }
            } else {
                if manifest.registry.effective_url().is_some() {
                    eprintln!("  ⚠️  Dependency '{}': not found in the local registry or the configured remote registry.", dep_name);
                } else {
                    eprintln!("  ⚠️  Dependency '{}': not found in local registry. Run `mumei publish` in the dependency project first.", dep_name);
                }
            }
        }
    }

    // A dependency can declare its own manifest dependencies: a package
    // certified standalone keys the definitions its manifest deps provide, so
    // validating that certificate here needs the same edges in the dep
    // entry's import closure — not just the root's. Wire every loaded dep
    // entry to the entries its manifest names. Matching is by resolved
    // dependency spec, never by bare name: a dependency key is only an alias
    // in the consumer's manifest, so a same-named but different package must
    // not leak into the dep's tracked closure. Iterate to a fixpoint so
    // dep-of-dep chains merge transitively; cycles terminate because each
    // round adds at least one previously absent edge.
    let loaded_deps: Vec<LoadedDep> = deferred_certificates
        .iter()
        .map(|deferred| {
            let package_name = crate::manifest::load(&deferred.module_dir.join("mumei.toml"))
                .ok()
                .map(|manifest| manifest.package.name);
            LoadedDep {
                entry_path: deferred.entry_path.clone(),
                module_dir: deferred.module_dir.clone(),
                spec: deferred.spec.clone(),
                dependency_name: deferred.dependency_name.clone(),
                package_name,
            }
        })
        .collect();
    let resolved_target =
        |dep_name: &str, spec: &crate::manifest::Dependency, base: &Path| -> Option<PathBuf> {
            loaded_deps.iter().find_map(|loaded| {
                let matched = if let Some(rel_path) = spec.as_path() {
                    same_module_dir(&base.join(rel_path), &loaded.module_dir)
                } else if spec.as_git().is_some() {
                    loaded.spec.as_git() == spec.as_git()
                } else {
                    // Registry specs resolve by name, so the manifest key is
                    // itself the package name; use it as the fallback when the
                    // fetched package carries no manifest (remote fetch does
                    // not require one).
                    let registry_name = loaded.package_name.as_deref().or_else(|| {
                        loaded
                            .spec
                            .version()
                            .map(|_| loaded.dependency_name.as_str())
                    });
                    registry_name == Some(dep_name)
                };
                matched.then(|| loaded.entry_path.clone())
            })
        };
    loop {
        let mut changed = false;
        for deferred in &deferred_certificates {
            let Ok(dep_manifest) = crate::manifest::load(&deferred.module_dir.join("mumei.toml"))
            else {
                continue;
            };
            for (dep_name, spec) in &dep_manifest.dependencies {
                let Some(target) = resolved_target(dep_name, spec, &deferred.module_dir) else {
                    continue;
                };
                let (Some(entry), Some(target)) = (deferred.entry_path.to_str(), target.to_str())
                else {
                    continue;
                };
                let already_wired = module_env
                    .import_closure
                    .get(entry)
                    .is_some_and(|imports| imports.contains(target));
                if !already_wired {
                    record_import_closure(module_env, Some(entry), Some(target));
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    deferred_certificates.sort_by(|a, b| a.dependency_name.cmp(&b.dependency_name));
    let mut package_alias_claims = HashMap::new();
    for deferred in &deferred_certificates {
        if let Some(package_alias) = &deferred.package_alias {
            *package_alias_claims
                .entry(package_alias.clone())
                .or_insert(0usize) += 1;
        }
    }
    for deferred in &mut deferred_certificates {
        if deferred
            .package_alias
            .as_ref()
            .is_some_and(|alias| package_alias_claims.get(alias) != Some(&1))
        {
            deferred.package_alias = None;
        }
    }
    for deferred in &deferred_certificates {
        if let Some(package_alias) = &deferred.package_alias {
            register_imported_alias_names(
                &deferred.items,
                package_alias,
                module_env,
                deferred.entry_path.to_str(),
            );
        }
    }
    let verified_certificates = deferred_certificates
        .into_iter()
        .map(|deferred| {
            let cert_results = verify_import_certificate(
                &deferred.module_dir,
                &deferred.entry_path,
                &deferred.items,
                allow_lean_verified,
                module_env,
                strict_imports,
            );
            (deferred, cert_results)
        })
        .collect::<Vec<_>>();
    for (deferred, cert_results) in verified_certificates {
        let package_aliases = deferred
            .package_alias
            .as_deref()
            .into_iter()
            .collect::<Vec<_>>();
        mark_dependency_atoms_with_cert(
            &deferred.items,
            &deferred.dependency_name,
            &package_aliases,
            &cert_results,
            module_env,
            strict_imports,
        )?;
        if let Some(message) = deferred.load_message {
            println!("{message}");
        }
    }

    Ok(())
}
