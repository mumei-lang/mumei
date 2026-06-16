use crate::pipeline::*;
use mumei_core::parser;
use mumei_core::parser::Item;
use std::path::Path;

pub(crate) fn cmd_check(input: &str) {
    let input_path = Path::new(input);
    if input_path.is_dir() {
        let mut files = collect_mm_files(input_path);
        files.sort();
        if files.is_empty() {
            eprintln!("❌ No .mm files found in '{}'", input);
            std::process::exit(1);
        }
        println!(
            "🗡️  Mumei check: checking {} file(s) in '{}'...",
            files.len(),
            input
        );
        let mut total_ok = 0usize;
        let mut total_fail = 0usize;
        for file in &files {
            let file_str = file.to_string_lossy().to_string();
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                try_load_and_prepare(&file_str)
            })) {
                Ok(Ok((items, ..))) => {
                    cmd_check_print_items(&file_str, &items);
                    total_ok += 1;
                }
                Ok(Err(e)) => {
                    eprintln!("  ❌ '{}': {}", file_str, e);
                    total_fail += 1;
                }
                Err(_) => {
                    eprintln!("  ❌ '{}': parse error (panic)", file_str);
                    total_fail += 1;
                }
            }
        }
        println!(
            "\n🗡️  Directory check summary: {} passed, {} failed",
            total_ok, total_fail
        );
        if total_fail > 0 {
            std::process::exit(1);
        }
    } else {
        println!("🗡️  Mumei check: parsing and resolving '{}'...", input);
        let (items, _module_env, _imports, _source) = load_and_prepare(input);
        cmd_check_print_items(input, &items);
    }
}

pub(crate) fn cmd_check_print_items(_input: &str, items: &[Item]) {
    let mut type_count = 0;
    let mut struct_count = 0;
    let mut enum_count = 0;
    let mut trait_count = 0;
    let mut atom_count = 0;
    for item in items {
        match item {
            Item::Import(decl) => {
                let alias_str = decl.alias.as_deref().unwrap_or("(none)");
                println!("  📦 Import: '{}' as '{}'", decl.path, alias_str);
            }
            Item::TypeDef(t) => {
                type_count += 1;
                println!("  ✨ Type: '{}' ({})", t.name, t._base_type);
            }
            Item::StructDef(s) => {
                struct_count += 1;
                println!("  🏗️  Struct: '{}'", s.name);
            }
            Item::EnumDef(e) => {
                enum_count += 1;
                println!("  🔷 Enum: '{}'", e.name);
            }
            Item::TraitDef(t) => {
                trait_count += 1;
                println!("  📜 Trait: '{}'", t.name);
            }
            Item::ImplDef(i) => {
                println!("  🔧 Impl: {} for {}", i.trait_name, i.target_type);
            }
            Item::Atom(a) => {
                atom_count += 1;
                let async_marker = if a.is_async { " (async)" } else { "" };
                let res_marker = if !a.resources.is_empty() {
                    format!(" [resources: {}]", a.resources.join(", "))
                } else {
                    String::new()
                };
                println!("  ✨ Atom: '{}'{}{}", a.name, async_marker, res_marker);
            }
            Item::ResourceDef(r) => {
                let mode_str = match r.mode {
                    parser::ResourceMode::Exclusive => "exclusive",
                    parser::ResourceMode::Shared => "shared",
                };
                println!(
                    "  🔒 Resource: '{}' (priority={}, mode={})",
                    r.name, r.priority, mode_str
                );
            }
            Item::ExternBlock(eb) => {
                println!(
                    "  🔗 Extern \"{}\" ({} function(s))",
                    eb.language,
                    eb.functions.len()
                );
            }
            Item::EffectDef(e) => {
                println!("  ⚡ Effect: '{}'", e.name);
            }
            Item::ImplBlock(ib) => {
                atom_count += ib.methods.len();
                println!(
                    "  🔧 ImplBlock: {} ({} method(s))",
                    ib.struct_name,
                    ib.methods.len()
                );
            }
        }
    }
    println!(
        "✅ Check passed: {} types, {} structs, {} enums, {} traits, {} atoms",
        type_count, struct_count, enum_count, trait_count, atom_count
    );
}

// =============================================================================
// mumei verify — Z3 verification only (no codegen)
// =============================================================================
