use crate::codegen::*;
use crate::linker;
use crate::pipeline::*;
use mumei_core::hir::lower_atom_to_hir_with_env;
use mumei_core::parser::Item;
use mumei_core::{resolver, verification};
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) fn cmd_run(
    input: &str,
    emit: &str,
    output: Option<&str>,
    allow_lean_verified: bool,
    args: &[String],
) {
    use std::process::Command;

    let tmp_dir = std::env::temp_dir().join(format!("mumei_run_{}", std::process::id()));
    if let Err(e) = fs::create_dir_all(&tmp_dir) {
        eprintln!("❌ Failed to create temp directory: {}", e);
        std::process::exit(1);
    }

    let output_path = output.map(PathBuf::from);
    let binary_path = output_path
        .clone()
        .unwrap_or_else(|| tmp_dir.join("mumei_output"));
    if let Some(parent) = binary_path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            eprintln!(
                "❌ Failed to create output directory '{}': {}",
                parent.display(),
                e
            );
            let _ = fs::remove_dir_all(&tmp_dir);
            std::process::exit(1);
        }
    }

    check_z3_available();
    println!("🗡️  Mumei Run: verify → codegen → link → execute");

    let (items, mut module_env, _imports, _source) =
        load_and_prepare_with_full_options(input, false, allow_lean_verified);
    let extern_blocks = collect_extern_blocks(&module_env);

    // Check that a main atom exists and takes no parameters
    let main_atom = items
        .iter()
        .find(|item| matches!(item, Item::Atom(atom) if atom.name == "main"));
    match main_atom {
        None => {
            eprintln!(
                "❌ Error: No `atom main()` found in '{}'. A main atom is required for `mumei run`.",
                input
            );
            let _ = fs::remove_dir_all(&tmp_dir);
            std::process::exit(1);
        }
        Some(Item::Atom(atom)) if !atom.params.is_empty() => {
            eprintln!(
                "❌ Error: atom main() must take no parameters for `mumei run`, but found {} parameter(s).",
                atom.params.len()
            );
            let _ = fs::remove_dir_all(&tmp_dir);
            std::process::exit(1);
        }
        _ => {}
    }

    // Check for extern "Rust" blocks and warn
    for item in &items {
        if let Item::ExternBlock(eb) = item {
            eprintln!(
                "  ⚠️  Warning: extern \"{}\" block detected. FFI functions require the mumei runtime library and may not link.",
                eb.language
            );
        }
    }

    // Register dependencies for all atoms
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

    // Verify and collect all atoms
    for item in &items {
        match item {
            Item::Atom(atom) => {
                let hir_atom = lower_atom_to_hir_with_env(atom, Some(&module_env));
                match verification::verify(&hir_atom, Path::new("."), &module_env) {
                    Ok(()) => {
                        module_env.mark_verified(&atom.name);
                        println!("  ✅ Verified: {}", atom.name);
                    }
                    Err(e) => {
                        eprintln!("  ❌ Verification failed for '{}': {}", atom.name, e);
                        let _ = fs::remove_dir_all(&tmp_dir);
                        std::process::exit(1);
                    }
                }
            }
            Item::ImplBlock(impl_block) => {
                for method in &impl_block.methods {
                    let mut qualified = method.clone();
                    qualified.name = format!("{}::{}", impl_block.struct_name, method.name);
                    let hir_atom = lower_atom_to_hir_with_env(&qualified, Some(&module_env));
                    match verification::verify(&hir_atom, Path::new("."), &module_env) {
                        Ok(()) => {
                            module_env.mark_verified(&qualified.name);
                            println!("  ✅ Verified: {}", qualified.name);
                        }
                        Err(e) => {
                            eprintln!("  ❌ Verification failed for '{}': {}", qualified.name, e);
                            let _ = fs::remove_dir_all(&tmp_dir);
                            std::process::exit(1);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let hir_atoms = collect_binary_hir_atoms(&items, &module_env);

    let runtime_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/mumei_runtime.c");
    if !runtime_path.exists() {
        eprintln!("❌ Runtime library not found: {}", runtime_path.display());
        let _ = fs::remove_dir_all(&tmp_dir);
        std::process::exit(1);
    }
    let runtime_stubs_path = tmp_dir.join("mumei_runtime_stubs.c");
    if let Err(e) = write_effect_and_resource_runtime_stubs(&module_env, &runtime_stubs_path) {
        eprintln!("❌ Runtime stub generation failed: {}", e);
        let _ = fs::remove_dir_all(&tmp_dir);
        std::process::exit(1);
    }

    let link_input = if emit == "llvm-ir" {
        let ll_path = output_path
            .as_ref()
            .map(|path| path.with_extension("ll"))
            .unwrap_or_else(|| tmp_dir.join("merged.ll"));
        if let Err(e) = mumei_emit_llvm::binary::compile_atoms_to_binary_ll(
            &hir_atoms,
            &module_env,
            &extern_blocks,
            &ll_path,
        ) {
            eprintln!("❌ Codegen failed: {}", e);
            let _ = fs::remove_dir_all(&tmp_dir);
            std::process::exit(1);
        }
        let object_path = tmp_dir.join("mumei_output.o");
        if let Err(e) = mumei_emit_llvm::codegen::compile_llvm_ir_to_object(&ll_path, &object_path)
        {
            eprintln!("❌ Codegen failed: {}", e);
            let _ = fs::remove_dir_all(&tmp_dir);
            std::process::exit(1);
        }
        object_path
    } else {
        let object_path = tmp_dir.join("mumei_output.o");
        if let Err(e) = mumei_emit_llvm::binary::compile_atoms_to_binary_object(
            &hir_atoms,
            &module_env,
            &extern_blocks,
            &object_path,
        ) {
            eprintln!("❌ Codegen failed: {}", e);
            let _ = fs::remove_dir_all(&tmp_dir);
            std::process::exit(1);
        }
        object_path
    };

    println!(
        "  🔗 Linking {} atom(s) to native binary...",
        hir_atoms.len()
    );
    let mut link_inputs = vec![link_input.clone(), runtime_stubs_path.clone()];
    let rust_ffi_lib = if uses_rust_ffi(&extern_blocks) {
        match generate_rust_ffi_staticlib(Path::new(env!("CARGO_MANIFEST_DIR")), &tmp_dir) {
            Ok(path) => Some(path),
            Err(e) => {
                eprintln!("❌ Rust FFI runtime build failed: {}", e);
                let _ = fs::remove_dir_all(&tmp_dir);
                std::process::exit(1);
            }
        }
    } else {
        None
    };
    if let Some(path) = &rust_ffi_lib {
        link_inputs.push(path.clone());
    }
    if let Err(e) = linker::link_to_binary(&link_inputs, &binary_path, Some(&runtime_path)) {
        eprintln!("❌ Linking failed: {}", e);
        let _ = fs::remove_dir_all(&tmp_dir);
        std::process::exit(1);
    }

    println!("  🚀 Running {}...\n", binary_path.display());

    // Execute the binary
    let status = Command::new(&binary_path)
        .args(args)
        .status()
        .unwrap_or_else(|e| {
            eprintln!("❌ Failed to execute binary: {}", e);
            let _ = fs::remove_dir_all(&tmp_dir);
            std::process::exit(1);
        });

    if output_path.is_some() {
        println!("\n  ✅ Binary written to: {}", binary_path.display());
    }
    let _ = fs::remove_dir_all(&tmp_dir);

    // Exit with the child's exit code
    std::process::exit(status.code().unwrap_or(1));
}

// =============================================================================
// mumei add — add dependency to mumei.toml
// =============================================================================
