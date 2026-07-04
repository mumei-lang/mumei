use crate::codegen::lowering::{declare_extern_functions, resolve_param_type, resolve_return_type};
use crate::codegen::stmt_emit::compile_hir_stmt;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine,
};
use inkwell::types::BasicType;
use inkwell::values::BasicValueEnum;
use inkwell::OptimizationLevel;
use mumei_core::hir::HirAtom;
use mumei_core::verification::{ModuleEnv, MumeiError, MumeiResult};
use std::collections::HashMap;
use std::path::Path;

pub fn compile_atom_into_module<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    hir_atom: &HirAtom,
    module_env: &ModuleEnv,
    extern_blocks: &[mumei_core::parser::ExternBlock],
) -> MumeiResult<()> {
    let atom = &hir_atom.atom;
    let builder = context.create_builder();

    // Declare all extern functions before compiling the atom body
    declare_extern_functions(context, module, extern_blocks, module_env);

    // パラメータ型を精緻型から解決
    let param_types: Vec<inkwell::types::BasicMetadataTypeEnum> = atom
        .params
        .iter()
        .map(|p| resolve_param_type(context, p.type_name.as_deref(), module_env).into())
        .collect();
    // Plan 18: Use resolved return type instead of hardcoded i64
    let ret_type = resolve_return_type(context, atom, module_env);
    let fn_type = ret_type.fn_type(&param_types, false);
    let function = if let Some(existing) = module.get_function(&atom.name) {
        if existing.get_first_basic_block().is_some() {
            return Err(MumeiError::codegen(format!(
                "Duplicate function definition: {}",
                atom.name
            )));
        }
        existing
    } else {
        module.add_function(&atom.name, fn_type, None)
    };

    let entry_block = context.append_basic_block(function, "entry");
    builder.position_at_end(entry_block);

    let mut variables = HashMap::new();
    let mut var_types: HashMap<String, String> = HashMap::new();
    let mut array_ptrs: HashMap<String, (BasicValueEnum, BasicValueEnum)> = HashMap::new();

    for (i, param) in atom.params.iter().enumerate() {
        let val = function.get_nth_param(i as u32).unwrap();
        if let Some(type_name) = &param.type_name {
            let base = module_env.resolve_base_type(type_name);
            if module_env.get_struct(&base).is_some() {
                var_types.insert(param.name.clone(), base);
            }
        }
        // Fat Pointer 配列パラメータの場合、len と data_ptr を分解して保持
        if val.is_struct_value() {
            let struct_val = val.into_struct_value();
            let len_val =
                llvm!(builder.build_extract_value(struct_val, 0, &format!("{}_len", param.name)));
            let data_ptr =
                llvm!(builder.build_extract_value(struct_val, 1, &format!("{}_data", param.name)));
            array_ptrs.insert(param.name.clone(), (len_val, data_ptr));
            variables.insert(param.name.clone(), len_val);
        } else {
            variables.insert(param.name.clone(), val);
        }
    }

    let result_val = compile_hir_stmt(
        context,
        &builder,
        module,
        &function,
        &hir_atom.body,
        &mut variables,
        &mut var_types,
        &array_ptrs,
        module_env,
    )?;

    llvm!(builder.build_return(Some(&result_val)));

    Ok(())
}

/// P7-A: Compile an HirAtom into a standalone in-memory LLVM Module.
/// Creates its own Context and Module, compiles the atom, and returns
/// the LLVM IR as a string (detached from Context lifetime).
pub fn compile_to_module(
    hir_atom: &HirAtom,
    module_env: &ModuleEnv,
    extern_blocks: &[mumei_core::parser::ExternBlock],
) -> MumeiResult<String> {
    let atom = &hir_atom.atom;
    let context = Context::create();
    let module = context.create_module(&atom.name);

    compile_atom_into_module(&context, &module, hir_atom, module_env, extern_blocks)?;

    // Return the module IR as a string (detached from Context lifetime)
    Ok(module.print_to_string().to_string())
}

/// Original compile function — calls compile_atom_into_module then writes .ll file.
/// Preserves existing behavior including effects comment insertion.
pub fn compile_atoms_into_module<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    hir_atoms: &[HirAtom],
    module_env: &ModuleEnv,
    extern_blocks: &[mumei_core::parser::ExternBlock],
) -> MumeiResult<()> {
    for hir_atom in hir_atoms {
        compile_atom_into_module(context, module, hir_atom, module_env, extern_blocks)?;
    }
    Ok(())
}

pub fn compile_llvm_ir_to_object(ir_path: &Path, object_path: &Path) -> MumeiResult<()> {
    Target::initialize_native(&InitializationConfig::default()).map_err(|e| {
        MumeiError::codegen(format!("Failed to initialize native LLVM target: {e}"))
    })?;

    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple)
        .map_err(|e| MumeiError::codegen(format!("Failed to create LLVM target: {e}")))?;
    let machine = target
        .create_target_machine(
            &triple,
            "generic",
            "",
            OptimizationLevel::Default,
            RelocMode::Default,
            CodeModel::Default,
        )
        .ok_or_else(|| MumeiError::codegen("Failed to create LLVM target machine".to_string()))?;

    let context = Context::create();
    let memory_buffer =
        inkwell::memory_buffer::MemoryBuffer::create_from_file(ir_path).map_err(|e| {
            MumeiError::codegen(format!(
                "Failed to read LLVM IR '{}': {e}",
                ir_path.display()
            ))
        })?;
    let module = context.create_module_from_ir(memory_buffer).map_err(|e| {
        MumeiError::codegen(format!(
            "Failed to parse LLVM IR '{}': {e}",
            ir_path.display()
        ))
    })?;

    machine
        .write_to_file(&module, FileType::Object, object_path)
        .map_err(|e| {
            MumeiError::codegen(format!(
                "Failed to write object '{}': {e}",
                object_path.display()
            ))
        })
}

pub fn compile(
    hir_atom: &HirAtom,
    output_path: &Path,
    module_env: &ModuleEnv,
    extern_blocks: &[mumei_core::parser::ExternBlock],
) -> MumeiResult<()> {
    let atom = &hir_atom.atom;
    let context = Context::create();
    let module = context.create_module(&atom.name);

    compile_atom_into_module(&context, &module, hir_atom, module_env, extern_blocks)?;

    // エフェクト情報を .ll ファイル先頭にコメントとして追記する（後処理）
    let effects_comment = if !hir_atom.effect_set.effects.is_empty() {
        let effects_str: Vec<String> = hir_atom
            .effect_set
            .effects
            .iter()
            .map(|name| {
                if let Some(e) = atom.effects.iter().find(|e| &e.name == name) {
                    if e.params.is_empty() {
                        name.clone()
                    } else {
                        let params: Vec<String> = e
                            .params
                            .iter()
                            .map(|p| format!("\"{}\"", p.value))
                            .collect();
                        format!("{}({})", name, params.join(", "))
                    }
                } else {
                    name.clone()
                }
            })
            .collect();
        Some(format!("; effects: [{}]", effects_str.join(", ")))
    } else {
        None
    };

    let path_with_ext = output_path.with_extension("ll");
    module
        .print_to_file(&path_with_ext)
        .map_err(|e| MumeiError::codegen(e.to_string()))?;

    // エフェクトコメントを .ll ファイル先頭に挿入（source_filename を破壊しない）
    if let Some(comment) = effects_comment {
        let ll_content = std::fs::read_to_string(&path_with_ext)
            .map_err(|e| MumeiError::codegen(e.to_string()))?;
        let with_comment = format!("{}\n{}", comment, ll_content);
        std::fs::write(&path_with_ext, with_comment)
            .map_err(|e| MumeiError::codegen(e.to_string()))?;
    }

    Ok(())
}
