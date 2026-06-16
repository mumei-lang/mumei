use mumei_core::{manifest, proof_cert, registry};
use std::fs;
use std::path::Path;

pub(crate) fn cmd_add(dep: &str, version: Option<&str>) {
    // mumei.toml を探す
    let manifest_path = Path::new("mumei.toml");
    if !manifest_path.exists() {
        eprintln!("❌ Error: mumei.toml not found in current directory.");
        eprintln!("   Run `mumei init <project>` first, or cd into a Mumei project.");
        std::process::exit(1);
    }

    // 現在の mumei.toml を読み込み
    let content = fs::read_to_string(manifest_path).unwrap_or_else(|e| {
        eprintln!("❌ Error: Cannot read mumei.toml: {}", e);
        std::process::exit(1);
    });

    // パース確認
    if let Err(e) = manifest::load(manifest_path) {
        eprintln!("❌ Error: mumei.toml parse error: {}", e);
        std::process::exit(1);
    }

    // 依存の種類を判定
    let dep_entry = if dep.starts_with("./") || dep.starts_with("../") || dep.starts_with('/') {
        // ローカルパス依存
        let dep_path = Path::new(dep);
        if !dep_path.exists() {
            eprintln!("❌ Error: Path '{}' does not exist.", dep);
            std::process::exit(1);
        }
        // パッケージ名はディレクトリ名から推定
        let pkg_name = dep_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .replace('-', "_");
        let toml_line = format!("{} = {{ path = \"{}\" }}", pkg_name, dep);
        println!("📦 Adding local dependency: {} → {}", pkg_name, dep);
        (pkg_name, toml_line)
    } else if dep.contains("github.com") || dep.contains("gitlab.com") || dep.ends_with(".git") {
        // Git URL 依存 — clone to ~/.mumei/packages/<name>/
        let pkg_name = dep
            .split('/')
            .next_back()
            .unwrap_or("unknown")
            .trim_end_matches(".git")
            .replace('-', "_");
        let toml_line = format!("{} = {{ git = \"{}\" }}", pkg_name, dep);
        println!("📦 Adding git dependency: {} → {}", pkg_name, dep);

        // Pre-clone the repository so it's available for build
        let packages_dir = manifest::mumei_home().join("packages");
        let clone_dir = packages_dir.join(&pkg_name);
        if !clone_dir.exists() {
            let _ = fs::create_dir_all(&packages_dir);
            println!("   Cloning {}...", dep);
            let status = std::process::Command::new("git")
                .args(["clone", "--depth", "1", dep, &clone_dir.to_string_lossy()])
                .status();
            match status {
                Ok(s) if s.success() => {
                    println!("   ✅ Cloned to {}", clone_dir.display());
                }
                _ => {
                    eprintln!(
                        "  ⚠️  Warning: git clone failed. The dependency will be cloned at build time."
                    );
                }
            }
        } else {
            println!("   Using cached clone at {}", clone_dir.display());
        }

        (pkg_name, toml_line)
    } else {
        // パッケージ名のみ（レジストリ依存）
        // ~/.mumei/registry.json から検索
        let reg = registry::load();
        if let Some(pkg_entry) = reg.packages.get(dep) {
            // P5-B: Use --version if specified, otherwise use latest
            let resolved_version = match version {
                Some(v) => {
                    // Verify the specified version exists
                    if !pkg_entry.versions.contains_key(v) {
                        let available: Vec<&String> = pkg_entry.versions.keys().collect();
                        eprintln!(
                            "❌ Error: Version '{}' not found for package '{}'. Available versions: {:?}",
                            v, dep, available
                        );
                        std::process::exit(1);
                    }
                    v.to_string()
                }
                None => pkg_entry.latest.clone(),
            };
            let toml_line = format!("{} = \"{}\"", dep, resolved_version);
            println!(
                "📦 Adding registry dependency: {} v{}",
                dep, resolved_version
            );

            // Show available versions
            if pkg_entry.versions.len() > 1 {
                let versions: Vec<&String> = pkg_entry.versions.keys().collect();
                println!(
                    "   Available versions: {}",
                    versions
                        .iter()
                        .map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }

            // Verify the package path exists
            if let Some(ver_entry) = pkg_entry.versions.get(resolved_version.as_str()) {
                if !Path::new(&ver_entry.path).exists() {
                    eprintln!(
                        "  ⚠️  Warning: Package directory '{}' does not exist. It may have been removed.",
                        ver_entry.path
                    );
                }
                if ver_entry.verified {
                    println!("   ✅ Package is verified ({} atoms)", ver_entry.atom_count);
                }

                // P5-B: Verify proof certificate if cert_path exists
                if let Some(ref cp) = ver_entry.cert_path {
                    let cert_path = Path::new(cp);
                    if cert_path.exists() {
                        // Verify cert hash integrity
                        let mut cert_ok = true;
                        if let Some(ref expected_hash) = ver_entry.cert_hash {
                            if let Ok(data) = fs::read_to_string(cert_path) {
                                let actual_hash = proof_cert::compute_sha256(&data);
                                if &actual_hash != expected_hash {
                                    eprintln!(
                                        "  ⚠️  Certificate hash mismatch! Expected: {}, Got: {}",
                                        expected_hash, actual_hash
                                    );
                                    cert_ok = false;
                                }
                            }
                        }
                        if cert_ok {
                            println!("   🔒 Proof certificate verified");
                        }
                    } else {
                        eprintln!("  ⚠️  Certificate file not found: {}", cp);
                    }
                }
            }

            (dep.to_string(), toml_line)
        } else {
            // Not found in registry — add with wildcard version
            let toml_line = format!("{} = \"*\"", dep);
            eprintln!(
                "⚠️  Package '{}' not found in local registry (~/.mumei/registry.json).",
                dep
            );
            eprintln!("   The dependency will be added with version \"*\".");
            eprintln!("   To publish a package: cd <package-dir> && mumei publish");
            (dep.to_string(), toml_line)
        }
    };

    // 重複チェック: [dependencies] セクション内に同じ依存名が既に存在する場合は警告して終了
    {
        let dep_name = &dep_entry.0;
        let mut in_deps_section = false;
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed == "[dependencies]" {
                in_deps_section = true;
                continue;
            }
            if in_deps_section && trimmed.starts_with('[') {
                break; // reached next section
            }
            if in_deps_section
                && (trimmed.starts_with(&format!("{} ", dep_name))
                    || trimmed.starts_with(&format!("{}=", dep_name))
                    || trimmed.starts_with(&format!("{} =", dep_name)))
            {
                eprintln!(
                    "⚠️  Dependency '{}' already exists in mumei.toml. Remove the existing entry first or edit it manually.",
                    dep_name
                );
                std::process::exit(1);
            }
        }
    }

    // mumei.toml に追記
    let new_content = if content.contains("[dependencies]") {
        // [dependencies] セクションが既にある場合、その直後に追記
        content.replace(
            "[dependencies]",
            &format!("[dependencies]\n{}", dep_entry.1),
        )
    } else {
        // [dependencies] セクションがない場合、末尾に追加
        format!("{}\n[dependencies]\n{}\n", content.trim_end(), dep_entry.1)
    };

    fs::write(manifest_path, new_content).unwrap_or_else(|e| {
        eprintln!("❌ Error: Cannot write mumei.toml: {}", e);
        std::process::exit(1);
    });

    println!("✅ Added '{}' to mumei.toml", dep_entry.0);
}

// =============================================================================
// mumei publish — publish to local registry
// =============================================================================
