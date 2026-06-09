use crate::pipeline::*;
use mumei_core::hir::lower_atom_to_hir_with_env;
use mumei_core::parser::Item;
use mumei_core::{inspect, manifest, resolver, verification};
use std::fs;
use std::path::Path;

pub(crate) fn cmd_inspect() {
    use std::process::Command as Cmd;

    println!("🔍 Mumei Inspect: checking development environment...");
    println!();

    let mut ok_count = 0;
    let mut warn_count = 0;
    let mut fail_count = 0;

    // --- 1. Mumei compiler version ---
    println!("  Mumei compiler: v{}", env!("CARGO_PKG_VERSION"));
    ok_count += 1;

    // --- 2. Z3 solver ---
    match Cmd::new("z3").arg("--version").output() {
        Ok(output) => {
            let version = String::from_utf8_lossy(&output.stdout);
            let version = version.trim();
            if version.is_empty() {
                println!("  ⚠️  Z3: installed but version unknown");
                warn_count += 1;
            } else {
                println!("  ✅ Z3: {}", version);
                ok_count += 1;
            }
        }
        Err(_) => {
            println!("  ❌ Z3: not found");
            println!("     Install: brew install z3");
            fail_count += 1;
        }
    }

    // --- 3. LLVM ---
    let llvm_found = ["llc-17", "llc"]
        .iter()
        .any(|cmd| Cmd::new(cmd).arg("--version").output().is_ok());
    if llvm_found {
        // Try to get version
        let version_output = Cmd::new("llc-17")
            .arg("--version")
            .output()
            .or_else(|_| Cmd::new("llc").arg("--version").output());
        if let Ok(output) = version_output {
            let version = String::from_utf8_lossy(&output.stdout);
            let first_line = version.lines().next().unwrap_or("unknown");
            println!("  ✅ LLVM: {}", first_line.trim());
        } else {
            println!("  ✅ LLVM: installed");
        }
        ok_count += 1;
    } else {
        println!("  ❌ LLVM: not found");
        println!("     Install: brew install llvm@17");
        fail_count += 1;
    }

    // --- 4. std library ---
    // resolver と同じ探索順序: cwd → exe隣 → MUMEI_STD_PATH
    let std_modules = [
        "prelude.mm",
        "option.mm",
        "result.mm",
        "list.mm",
        "stack.mm",
        "alloc.mm",
        "container/bounded_array.mm",
    ];
    let mut std_base_dir: Option<std::path::PathBuf> = None;

    if Path::new("std/prelude.mm").exists() {
        std_base_dir = Some(std::path::PathBuf::from("std"));
    }
    if std_base_dir.is_none() {
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let candidate = exe_dir.join("std/prelude.mm");
                if candidate.exists() {
                    std_base_dir = Some(exe_dir.join("std"));
                }
            }
        }
    }
    if std_base_dir.is_none() {
        if let Ok(std_path) = std::env::var("MUMEI_STD_PATH") {
            let candidate = Path::new(&std_path).join("prelude.mm");
            if candidate.exists() {
                std_base_dir = Some(std::path::PathBuf::from(&std_path));
            }
        }
    }

    let mut std_found = 0;
    let mut std_missing = Vec::new();
    if let Some(ref base) = std_base_dir {
        for module in &std_modules {
            if base.join(module).exists() {
                std_found += 1;
            } else {
                std_missing.push(*module);
            }
        }
    } else {
        std_missing = std_modules.to_vec();
    }

    if std_missing.is_empty() {
        let location = std_base_dir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "?".to_string());
        println!(
            "  ✅ std library: {}/{} modules found ({})",
            std_found,
            std_modules.len(),
            location
        );
        ok_count += 1;
    } else {
        let hint = if std_base_dir.is_none() {
            " (set MUMEI_STD_PATH or place std/ next to mumei binary)"
        } else {
            ""
        };
        println!(
            "  ⚠️  std library: {}/{} modules found (missing: {}){}",
            std_found,
            std_modules.len(),
            std_missing.join(", "),
            hint
        );
        warn_count += 1;
    }

    // --- 8. mumei.toml (if in project directory) ---
    if Path::new("mumei.toml").exists() {
        // mumei.toml が見つかったらパースして内容を表示
        match manifest::load(Path::new("mumei.toml")) {
            Ok(m) => {
                println!("  ✅ mumei.toml: {} v{}", m.package.name, m.package.version);
                if !m.dependencies.is_empty() {
                    println!(
                        "     dependencies: {}",
                        m.dependencies
                            .keys()
                            .map(|k| k.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }

                ok_count += 1;
            }
            Err(e) => {
                println!("  ⚠️  mumei.toml: found but parse error: {}", e);
                warn_count += 1;
            }
        }
    } else {
        println!("  ℹ️  mumei.toml: not found (not in a Mumei project directory)");
    }

    // --- 9. ~/.mumei/ toolchain ---
    let mumei_home = manifest::mumei_home();
    let toolchains_dir = mumei_home.join("toolchains");
    if toolchains_dir.exists() {
        let mut tc_list = Vec::new();
        if let Ok(entries) = fs::read_dir(&toolchains_dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    if let Some(name) = entry.file_name().to_str() {
                        tc_list.push(name.to_string());
                    }
                }
            }
        }
        if tc_list.is_empty() {
            println!("  ℹ️  ~/.mumei/toolchains: empty (run `mumei setup`)");
        } else {
            tc_list.sort();
            println!("  ✅ ~/.mumei/toolchains: {}", tc_list.join(", "));
            ok_count += 1;
        }
    } else {
        println!("  ℹ️  ~/.mumei/toolchains: not found (run `mumei setup`)");
    }

    // --- Summary ---
    println!();
    if fail_count > 0 {
        println!(
            "❌ Inspect: {} ok, {} warnings, {} errors",
            ok_count, warn_count, fail_count
        );
        println!("   Fix the errors above to use Mumei.");
        std::process::exit(1);
    } else if warn_count > 0 {
        println!(
            "✅ Inspect: {} ok, {} warnings — Mumei is ready (optional tools missing)",
            ok_count, warn_count
        );
    } else {
        println!("✅ Inspect: {} ok — all tools available", ok_count);
    }
}

// =============================================================================
// mumei inspect <file.mm> --ai — Structured JSON inspection report (Plan 11A)
// =============================================================================

pub(crate) fn cmd_inspect_file(input: &str, ai: bool, format: &str) {
    check_z3_available();
    let (items, mut module_env, _imports, _source) = load_and_prepare(input);

    // Run verification to get results
    let output_dir = Path::new(".");
    let mut verification_results: std::collections::HashMap<String, bool> =
        std::collections::HashMap::new();

    // Register dependencies
    for item in &items {
        match item {
            Item::Atom(atom) => {
                let callees = resolver::collect_callees_from_body(&atom.body_expr);
                module_env.register_dependencies(&atom.name, callees);
            }
            Item::ImplBlock(impl_block) => {
                for method in &impl_block.methods {
                    let qualified_name = format!("{}::{}", impl_block.struct_name, method.name);
                    let callees = resolver::collect_callees_from_body(&method.body_expr);
                    module_env.register_dependencies(&qualified_name, callees);
                }
            }
            _ => {}
        }
    }

    // Try Z3 verification for each atom
    for item in &items {
        match item {
            Item::Atom(atom) => {
                if module_env.is_verified(&atom.name) {
                    verification_results.insert(atom.name.clone(), true);
                    continue;
                }
                let hir_atom = lower_atom_to_hir_with_env(atom, Some(&module_env));
                match verification::verify(&hir_atom, output_dir, &module_env) {
                    Ok(_) => {
                        module_env.mark_verified(&atom.name);
                        verification_results.insert(atom.name.clone(), true);
                    }
                    Err(_) => {
                        verification_results.insert(atom.name.clone(), false);
                    }
                }
            }
            Item::ImplBlock(impl_block) => {
                for method in &impl_block.methods {
                    let qualified_name = format!("{}::{}", impl_block.struct_name, method.name);
                    if module_env.is_verified(&qualified_name) {
                        verification_results.insert(qualified_name, true);
                        continue;
                    }
                    let mut qualified_method = method.clone();
                    qualified_method.name = qualified_name.clone();
                    let hir_atom = lower_atom_to_hir_with_env(&qualified_method, Some(&module_env));
                    match verification::verify(&hir_atom, output_dir, &module_env) {
                        Ok(_) => {
                            module_env.mark_verified(&qualified_name);
                            verification_results.insert(qualified_name, true);
                        }
                        Err(_) => {
                            verification_results.insert(qualified_name, false);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let report = inspect::generate_report(input, &items, &module_env, &verification_results);

    if ai || format == "json" {
        // JSON output for AI agents
        match serde_json::to_string_pretty(&report) {
            Ok(json) => println!("{}", json),
            Err(e) => {
                eprintln!("Failed to serialize report: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        // Human-readable text output
        println!("🔍 Mumei Inspect: {}", input);
        println!("  Version: {}", report.version);
        println!();
        println!("  Atoms ({}):", report.atoms.len());
        for atom in &report.atoms {
            let status_icon = match atom.verification_status.as_str() {
                "verified" => "✅",
                "failed" => "❌",
                _ => "⏭️",
            };
            println!(
                "    {} {} ({})",
                status_icon, atom.name, atom.verification_status
            );
            if atom.requires != "true" {
                println!("      requires: {}", atom.requires);
            }
            if atom.ensures != "true" {
                println!("      ensures: {}", atom.ensures);
            }
            if !atom.effects.is_empty() {
                println!("      effects: {}", atom.effects.join(", "));
            }
        }
        if !report.enums.is_empty() {
            println!();
            println!("  Enums ({}):", report.enums.len());
            for e in &report.enums {
                println!(
                    "    {} (variants: {})",
                    e.name,
                    e.variants
                        .iter()
                        .map(|v| v.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
        if !report.structs.is_empty() {
            println!();
            println!("  Structs ({}):", report.structs.len());
            for s in &report.structs {
                println!(
                    "    {} (fields: {})",
                    s.name,
                    s.fields
                        .iter()
                        .map(|f| f.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
        println!();
        println!(
            "  Verification: {} total, {} verified, {} failed, {} skipped",
            report.verification.total_atoms,
            report.verification.verified,
            report.verification.failed,
            report.verification.skipped
        );
    }
}

// =============================================================================
// mumei verify-cert — Verify proof certificate against current source (Plan 11B)
// =============================================================================
