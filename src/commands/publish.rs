use crate::feedback::*;
use crate::pipeline::*;
use mumei_core::hir::lower_atom_to_hir_with_env;
use mumei_core::parser::Item;
use mumei_core::{manifest, parser, proof_cert, registry, verification};
use std::fs;
use std::path::Path;

pub(crate) fn cmd_publish(proof_only: bool, allow_lean_verified: bool) {
    println!("📦 Mumei publish: publishing to local registry...");

    // 1. mumei.toml を読み込み
    let manifest_path = Path::new("mumei.toml");
    if !manifest_path.exists() {
        eprintln!("❌ Error: mumei.toml not found. Run `mumei init` first.");
        std::process::exit(1);
    }
    let m = match manifest::load(manifest_path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("❌ Error: {}", e);
            std::process::exit(1);
        }
    };

    let pkg_name = &m.package.name;
    let pkg_version = &m.package.version;
    println!("  📄 Package: {} v{}", pkg_name, pkg_version);

    // 2. エントリファイルを探す
    let entry_candidates = ["src/main.mm", "main.mm"];
    let entry_path = entry_candidates.iter().find(|p| Path::new(p).exists());
    let entry = match entry_path {
        Some(p) => *p,
        None => {
            eprintln!("❌ Error: No entry file found (src/main.mm or main.mm).");
            std::process::exit(1);
        }
    };

    // 3. 全 atom を Z3 で検証（未検証パッケージの公開を禁止）
    println!("  🔍 Verifying all atoms before publish...");
    let (items, mut module_env, _imports, source) =
        load_and_prepare_with_full_options(entry, false, allow_lean_verified);

    let output_dir = Path::new(".");
    let mut atom_count = 0;
    let mut failed = 0;
    let mut verification_results: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();

    for item in &items {
        match item {
            Item::Atom(atom) => {
                if module_env.is_verified(&atom.name) {
                    atom_count += 1;
                    verification_results.insert(
                        atom.name.clone(),
                        ("unsat".to_string(), "verified".to_string()),
                    );
                    continue;
                }
                let hir_atom = lower_atom_to_hir_with_env(atom, Some(&module_env));
                match verification::verify(&hir_atom, output_dir, &module_env) {
                    Ok(_) => {
                        println!("  ⚖️  '{}': verified ✅", atom.name);
                        module_env.mark_verified(&atom.name);
                        atom_count += 1;
                        verification_results.insert(
                            atom.name.clone(),
                            ("unsat".to_string(), "verified".to_string()),
                        );
                    }
                    Err(e) => {
                        let resolved = resolve_source_for_span(&source, &atom.span);
                        let e = e.with_source(&resolved, &atom.span);
                        eprintln!("{:?}", miette::Report::new(e));
                        verification_results
                            .insert(atom.name.clone(), ("sat".to_string(), "failed".to_string()));
                        failed += 1;
                    }
                }
            }
            Item::ImplBlock(impl_block) => {
                for method in &impl_block.methods {
                    let qualified_name = format!("{}::{}", impl_block.struct_name, method.name);
                    if module_env.is_verified(&qualified_name) {
                        atom_count += 1;
                        verification_results.insert(
                            qualified_name.clone(),
                            ("unsat".to_string(), "verified".to_string()),
                        );
                        continue;
                    }
                    let mut qualified_method = method.clone();
                    qualified_method.name = qualified_name.clone();
                    let hir_atom = lower_atom_to_hir_with_env(&qualified_method, Some(&module_env));
                    match verification::verify(&hir_atom, output_dir, &module_env) {
                        Ok(_) => {
                            println!("  ⚖️  '{}': verified ✅", qualified_name);
                            module_env.mark_verified(&qualified_name);
                            atom_count += 1;
                            verification_results.insert(
                                qualified_name.clone(),
                                ("unsat".to_string(), "verified".to_string()),
                            );
                        }
                        Err(e) => {
                            let resolved = resolve_source_for_span(&source, &method.span);
                            let e = e.with_source(&resolved, &method.span);
                            eprintln!("{:?}", miette::Report::new(e));
                            verification_results.insert(
                                qualified_name.clone(),
                                ("sat".to_string(), "failed".to_string()),
                            );
                            failed += 1;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if failed > 0 {
        eprintln!(
            "❌ Publish aborted: {} atom(s) failed verification. Fix errors and retry.",
            failed
        );
        std::process::exit(1);
    }

    println!("  ✅ All {} atom(s) verified.", atom_count);

    // 4. ~/.mumei/packages/<name>/<version>/ にコピー
    let packages_dir = manifest::mumei_home().join("packages");
    let pkg_dir = packages_dir.join(pkg_name).join(pkg_version);

    if pkg_dir.exists() {
        println!("  ⚠️  Overwriting existing version {}", pkg_version);
        let _ = fs::remove_dir_all(&pkg_dir);
    }
    fs::create_dir_all(&pkg_dir).unwrap_or_else(|e| {
        eprintln!("❌ Error: Failed to create {}: {}", pkg_dir.display(), e);
        std::process::exit(1);
    });

    // mumei.toml をコピー
    let _ = fs::copy("mumei.toml", pkg_dir.join("mumei.toml"));

    // ビルドキャッシュをコピー（proof artifact）
    let base_dir = Path::new(entry).parent().unwrap_or(Path::new("."));
    let cache_src = base_dir.join(".mumei_build_cache");
    if cache_src.exists() {
        let _ = fs::copy(&cache_src, pkg_dir.join(".mumei_build_cache"));
    }

    if !proof_only {
        // src/ ディレクトリを再帰コピー
        if Path::new("src").exists() {
            copy_dir_recursive(Path::new("src"), &pkg_dir.join("src"));
        }
        // ルートの .mm ファイルもコピー
        if let Ok(entries) = fs::read_dir(".") {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "mm") {
                    let _ = fs::copy(&path, pkg_dir.join(path.file_name().unwrap()));
                }
            }
        }
        println!("  📁 Copied source + proof cache to {}", pkg_dir.display());
    } else {
        println!("  📁 Copied proof cache only to {}", pkg_dir.display());
    }

    // 5. Generate proof certificate for the published package
    {
        let all_atoms: Vec<&parser::Atom> = items
            .iter()
            .filter_map(|item| match item {
                Item::Atom(atom) => Some(atom),
                _ => None,
            })
            .collect();
        // Also collect impl block methods as atoms for the certificate
        let impl_atoms: Vec<parser::Atom> = items
            .iter()
            .filter_map(|item| match item {
                Item::ImplBlock(ib) => Some(ib.methods.iter().map(|m| {
                    let mut qualified = m.clone();
                    qualified.name = format!("{}::{}", ib.struct_name, m.name);
                    qualified
                })),
                _ => None,
            })
            .flatten()
            .collect();
        let mut cert_atoms: Vec<&parser::Atom> = all_atoms;
        let impl_refs: Vec<&parser::Atom> = impl_atoms.iter().collect();
        cert_atoms.extend(impl_refs);

        let cert = proof_cert::generate_certificate(
            entry,
            &cert_atoms,
            &verification_results,
            &module_env,
            Some(pkg_name),
            Some(pkg_version),
            Some(proof_cert::HarnessCertificateMetadata {
                harness_contract: proof_cert::harness_contract_from_env(),
                intent_fidelity: proof_cert::intent_fidelity_from_env(),
                artifact_paths: proof_cert::artifact_paths_from_env(),
                budget_policy_fingerprint: proof_cert::budget_policy_fingerprint_from_env(),
            }),
        );
        let cert_path = pkg_dir.join("proof_certificate.json");
        match proof_cert::save_certificate(&cert, &cert_path) {
            Ok(()) => {
                println!(
                    "  📜 Proof certificate saved ({} atoms): {}",
                    cert.atoms.len(),
                    cert_path.display()
                );
            }
            Err(e) => {
                eprintln!("  ⚠️  Failed to save proof certificate: {}", e);
            }
        }
    }

    // 6. registry.json に登録 (P5-B: with cert metadata)
    let cert_file = pkg_dir.join("proof_certificate.json");
    let (reg_cert_path, reg_cert_hash) = if cert_file.exists() {
        let cert_path_str = cert_file.to_string_lossy().to_string();
        let cert_hash_str = fs::read_to_string(&cert_file)
            .ok()
            .map(|data| proof_cert::compute_sha256(&data))
            .unwrap_or_default();
        (Some(cert_path_str), Some(cert_hash_str))
    } else {
        (None, None)
    };
    if let Err(e) = registry::register_with_cert(
        pkg_name,
        pkg_version,
        &pkg_dir,
        atom_count,
        true,
        reg_cert_path,
        reg_cert_hash,
    ) {
        eprintln!("  ⚠️  Registry update warning: {}", e);
    }

    println!();
    println!(
        "🎉 Published {} v{} to local registry",
        pkg_name, pkg_version
    );
    println!(
        "   Other projects can now use: {} = \"{}\"",
        pkg_name, pkg_version
    );
}

// =============================================================================
// mumei list — List available packages in local registry
// =============================================================================
