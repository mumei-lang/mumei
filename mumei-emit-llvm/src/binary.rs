//! P7-B: Binary compilation pipeline.
//!
//! Compiles multiple HirAtoms into a single LLVM Module with a C-compatible
//! main wrapper, then writes the merged .ll file for linking.

use inkwell::context::Context;
use mumei_core::hir::HirAtom;
use mumei_core::parser::ExternBlock;
use mumei_core::verification::{ModuleEnv, MumeiError, MumeiResult};
use std::path::Path;

use crate::codegen::compile_atom_into_module;

/// Compile multiple HirAtoms into a single merged LLVM IR file with a
/// C-compatible `main` wrapper that calls the user's `main` atom.
///
/// The user's `main` atom is compiled as `__mumei_user_main` to avoid
/// conflict with the C `main` entry point. A wrapper `main(argc, argv)`
/// is generated that calls `__mumei_user_main` and returns its result
/// as the process exit code.
///
/// The merged .ll file is written to `output_ll_path`.
pub fn compile_atoms_to_binary_ll(
    hir_atoms: &[HirAtom],
    module_env: &ModuleEnv,
    extern_blocks: &[ExternBlock],
    output_ll_path: &Path,
) -> MumeiResult<()> {
    let context = Context::create();
    let merged_module = context.create_module("mumei_merged");

    // Check that a "main" atom exists before compiling
    let has_main = hir_atoms.iter().any(|h| h.atom.name == "main");
    if !has_main {
        return Err(MumeiError::codegen(
            "No `atom main()` found. A main atom is required for binary compilation.".to_string(),
        ));
    }

    // Compile all atoms into the merged module.
    // For the "main" atom, rename it to "__mumei_user_main" before compilation
    // so it doesn't conflict with the C main wrapper we'll generate.
    for hir_atom in hir_atoms {
        if hir_atom.atom.name == "main" {
            // Clone and rename to avoid C main conflict
            let mut renamed = hir_atom.clone();
            renamed.atom.name = "__mumei_user_main".to_string();
            compile_atom_into_module(
                &context,
                &merged_module,
                &renamed,
                module_env,
                extern_blocks,
            )?;
        } else {
            compile_atom_into_module(
                &context,
                &merged_module,
                hir_atom,
                module_env,
                extern_blocks,
            )?;
        }
    }

    // Generate C-compatible main wrapper
    let i32_type = context.i32_type();
    let ptr_type = context.ptr_type(inkwell::AddressSpace::default());

    // C standard requires main to return int (i32)
    let c_main_fn_type = i32_type.fn_type(&[i32_type.into(), ptr_type.into()], false);
    let real_main = merged_module.add_function("main", c_main_fn_type, None);
    let entry = context.append_basic_block(real_main, "entry");
    let builder = context.create_builder();
    builder.position_at_end(entry);

    let user_main = merged_module
        .get_function("__mumei_user_main")
        .ok_or_else(|| {
            MumeiError::codegen("Internal error: __mumei_user_main not found".to_string())
        })?;
    let call_result = builder
        .build_call(user_main, &[], "result")
        .map_err(|e| MumeiError::codegen(format!("Failed to build call to main: {}", e)))?;

    let result = call_result
        .try_as_basic_value()
        .left()
        .ok_or_else(|| MumeiError::codegen("main() must return a value".to_string()))?;

    // Truncate i64 result to i32 for C-compatible process exit code
    let exit_code = builder
        .build_int_truncate(result.into_int_value(), i32_type, "exit_code")
        .map_err(|e| MumeiError::codegen(format!("Failed to build truncate: {}", e)))?;
    builder
        .build_return(Some(&exit_code))
        .map_err(|e| MumeiError::codegen(format!("Failed to build return: {}", e)))?;

    // Write merged module to .ll file
    merged_module
        .print_to_file(output_ll_path)
        .map_err(|e| MumeiError::codegen(format!("Failed to write LLVM IR: {}", e)))?;

    Ok(())
}
