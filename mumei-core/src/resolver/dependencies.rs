use crate::parser;
use crate::verification::{ModuleEnv, MumeiError, MumeiResult};
use std::fs;
use std::path::{Path, PathBuf};

use super::cache::{load_cache, save_cache};
use super::imports::{
    mark_dependency_atoms_with_cert, register_imported_items, resolve_imports_recursive,
    verify_import_certificate, ResolverContext,
};

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
                resolve_imports_recursive(&items, dep_base_dir, &mut ctx, &mut cache, module_env)?;
                save_cache(&cache_path, &cache);
                register_imported_items(&items, Some(dep_name), module_env);
                // P5-C: Check for proof certificate before marking atoms as verified
                let cert_results =
                    verify_import_certificate(&abs_path, entry_path, &items, allow_lean_verified);
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
                ctx.allow_lean_verified = allow_lean_verified;
                ctx.strict_imports = strict_imports;
                resolve_imports_recursive(&items, dep_base_dir, &mut ctx, &mut cache, module_env)?;
                save_cache(&cache_path, &cache);
                register_imported_items(&items, Some(dep_name), module_env);
                // P5-C: Check for proof certificate before marking atoms as verified
                // Use clone_dir (package root) instead of dep_base_dir (entry file parent)
                // because cmd_publish saves certificates to the package root.
                let cert_results =
                    verify_import_certificate(&clone_dir, entry_path, &items, allow_lean_verified);
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
                    ctx.allow_lean_verified = allow_lean_verified;
                    ctx.strict_imports = strict_imports;
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
                    let cert_results = verify_import_certificate(
                        &pkg_dir,
                        entry_path,
                        &items,
                        allow_lean_verified,
                    );
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
