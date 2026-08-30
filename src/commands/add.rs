use mumei_core::emitter;
use mumei_core::{manifest, proof_cert, registry};
use std::fs;
use std::path::{Path, PathBuf};

// =============================================================================
// mumei add --emitter — Emitter Plugin Architecture Phase 3 install path
// =============================================================================

/// Plugin names become directory and library file names, so keep them to a
/// conservative character set instead of letting arbitrary input reach the
/// filesystem.
fn is_valid_emitter_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Resolve the compiled cdylib to install for `name` from a user-supplied
/// `--path`, which may be the library itself, a directory containing it, or a
/// cargo project whose `target/{release,debug}` holds it.
fn resolve_plugin_source(name: &str, path: &Path) -> Result<PathBuf, String> {
    let lib_filename = emitter::external_emitter_library_filename(name);
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    if !path.is_dir() {
        return Err(format!("path '{}' does not exist", path.display()));
    }
    let candidates = [
        path.join(&lib_filename),
        path.join("target").join("release").join(&lib_filename),
        path.join("target").join("debug").join(&lib_filename),
    ];
    candidates
        .iter()
        .find(|candidate| candidate.is_file())
        .cloned()
        .ok_or_else(|| {
            format!(
                "no `{}` found under '{}' (looked in ./, target/release/, target/debug/).\n   \
                 Build the plugin with `cargo build --release` first, or pass the library directly with --path.",
                lib_filename,
                path.display()
            )
        })
}

/// Install an external emitter plugin into `~/.mumei/emitters/<name>/` and
/// validate it through the existing loader so the ABI-version check and the
/// panic-safety wrapper gate the install just like a build would.
pub(crate) fn cmd_add_emitter(name: &str, path: Option<&str>, force: bool) {
    if !is_valid_emitter_name(name) {
        eprintln!(
            "❌ Error: invalid emitter name '{}'. Use ASCII letters, digits, '_' or '-'.",
            name
        );
        std::process::exit(1);
    }

    let source_root = PathBuf::from(path.unwrap_or("."));
    let source = resolve_plugin_source(name, &source_root).unwrap_or_else(|e| {
        eprintln!("❌ Error: {}", e);
        std::process::exit(1);
    });

    let dest = emitter::external_emitter_library_path(name);
    let dest_dir = emitter::external_emitter_dir(name);
    if dest.exists() && !force {
        eprintln!(
            "❌ Error: emitter '{}' is already installed at {}.",
            name,
            dest.display()
        );
        eprintln!("   Re-run with --force to overwrite it.");
        std::process::exit(1);
    }

    fs::create_dir_all(&dest_dir).unwrap_or_else(|e| {
        eprintln!("❌ Error: cannot create {}: {}", dest_dir.display(), e);
        std::process::exit(1);
    });

    // Stage the candidate beside its final name so validation never touches an
    // existing install: the rename below is the only mutation of `dest`, and it
    // stays within one directory, hence one filesystem.
    let staged = dest_dir.join(format!(
        ".{}.incoming",
        emitter::external_emitter_library_filename(name)
    ));
    let _ = fs::remove_file(&staged);
    fs::copy(&source, &staged).unwrap_or_else(|e| {
        eprintln!(
            "❌ Error: cannot copy {} to {}: {}",
            source.display(),
            staged.display(),
            e
        );
        std::process::exit(1);
    });

    println!("🔌 Installing emitter plugin '{}'", name);
    println!(
        "   {} → {} (staged at {} until validation passes)",
        source.display(),
        dest.display(),
        staged.display()
    );

    // Reuse the build-time loader: it checks `EMITTER_ABI_VERSION`, the
    // `mumei_create_emitter` handle, and wraps the plugin in `PanicSafeEmitter`.
    match emitter::load_external_emitter_from_path(name, &staged) {
        Ok(_) => {
            if let Err(e) = fs::rename(&staged, &dest) {
                let _ = fs::remove_file(&staged);
                eprintln!(
                    "❌ Error: cannot install validated plugin at {}: {}",
                    dest.display(),
                    e
                );
                std::process::exit(1);
            }
            println!(
                "   ✅ ABI version {} verified",
                emitter::EMITTER_ABI_VERSION
            );
            println!("✅ Installed emitter '{name}'. Use it with `mumei build --emit {name} <input.mm>`.");
        }
        Err(e) => {
            eprintln!("❌ Error: emitter '{}' failed validation: {}", name, e);
            eprintln!(
                "   The plugin must export `mumei_emitter_abi_version` and `mumei_create_emitter`."
            );
            if let Err(remove_err) = fs::remove_file(&staged) {
                eprintln!(
                    "   ⚠️  Could not remove the staged candidate at {}: {}",
                    staged.display(),
                    remove_err
                );
            }
            if dest.exists() {
                eprintln!(
                    "   The previous install at {} is unchanged.",
                    dest.display()
                );
            }
            std::process::exit(1);
        }
    }
}

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
    let project_manifest = manifest::load(manifest_path).unwrap_or_else(|e| {
        eprintln!("❌ Error: mumei.toml parse error: {}", e);
        std::process::exit(1);
    });

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
        // ~/.mumei/registry.json から検索し、無ければ P24 のリモートレジストリへ
        let mut reg = registry::load();
        let local_version = |entry: &registry::PackageEntry| {
            registry::select_version(
                entry.versions.keys().map(String::as_str),
                &entry.latest,
                version,
            )
            .filter(|v| entry.versions.contains_key(v))
        };
        let missing_locally = match reg.packages.get(dep) {
            None => true,
            Some(entry) => local_version(entry).is_none(),
        };
        if missing_locally {
            match registry::remote::resolve(&project_manifest.registry, dep, version, false) {
                Ok(Some(pkg_dir)) => {
                    println!(
                        "   ⬇️  Fetched from remote registry → {}",
                        pkg_dir.display()
                    );
                    reg = registry::load();
                }
                Ok(None) => {}
                Err(e) => eprintln!("  ⚠️  {}", e),
            }
        }
        if let Some(pkg_entry) = reg.packages.get(dep) {
            // P5-B: Use --version if specified, otherwise use latest
            // P24: a range such as `^1.0.0` records the concrete version it selects.
            let resolved_version = match local_version(pkg_entry) {
                Some(v) => v,
                None => {
                    let available: Vec<&String> = pkg_entry.versions.keys().collect();
                    eprintln!(
                        "❌ Error: Version '{}' not found for package '{}'. Available versions: {:?}",
                        version.unwrap_or("*"),
                        dep,
                        available
                    );
                    std::process::exit(1);
                }
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
            eprintln!(
                "   To resolve it remotely: set [registry] url in mumei.toml or MUMEI_REGISTRY_URL"
            );
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
