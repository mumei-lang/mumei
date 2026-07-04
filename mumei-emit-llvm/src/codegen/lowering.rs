use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::types::{BasicMetadataTypeEnum, BasicTypeEnum};
use inkwell::values::BasicValueEnum;
use inkwell::AddressSpace;
use mumei_core::lowering::{lower, LoweredType};
use mumei_core::verification::{ModuleEnv, MumeiError};

pub(crate) fn array_struct_type(context: &Context) -> inkwell::types::StructType<'_> {
    let i64_type = context.i64_type();
    let ptr_type = context.ptr_type(AddressSpace::default());
    context.struct_type(&[i64_type.into(), ptr_type.into()], false)
}

fn basic_type_rank<'a>(ty: BasicTypeEnum<'a>) -> (u32, bool, bool) {
    match ty {
        BasicTypeEnum::FloatType(float_ty) => {
            let name = float_ty.print_to_string().to_string();
            let bits = match name.as_str() {
                "float" => 32,
                "double" => 64,
                "half" => 16,
                "fp128" | "quad" => 128,
                _ => 64,
            };
            (bits, true, false)
        }
        BasicTypeEnum::IntType(int_ty) => (int_ty.get_bit_width(), false, false),
        BasicTypeEnum::PointerType(_) => (64, false, true),
        _ => (64, false, false),
    }
}

fn float_type_bit_width<'a>(ty: inkwell::types::FloatType<'a>) -> u32 {
    match ty.print_to_string().to_string().as_str() {
        "float" => 32,
        "double" => 64,
        "half" => 16,
        "fp128" | "quad" => 128,
        _ => 64,
    }
}

fn union_slot_type<'a>(a: BasicTypeEnum<'a>, b: BasicTypeEnum<'a>) -> BasicTypeEnum<'a> {
    if a == b {
        return a;
    }

    let (a_bits, a_is_float, a_is_ptr) = basic_type_rank(a);
    let (b_bits, b_is_float, b_is_ptr) = basic_type_rank(b);

    if a_bits != b_bits {
        return if a_bits > b_bits { a } else { b };
    }

    if a_is_float != b_is_float {
        return if a_is_float { a } else { b };
    }

    if a_is_ptr != b_is_ptr {
        return if a_is_ptr { b } else { a };
    }

    a
}

/// Generate LLVM struct type for an enum (tagged union).
/// Layout: { i64 tag, payload_slot_0, payload_slot_1, ... }
/// Each payload slot uses the largest type needed by any variant at that
/// position, preferring floating-point types on equal bit-widths.
pub(crate) fn enum_llvm_type<'a>(
    context: &'a Context,
    enum_def: &mumei_core::parser::EnumDef,
    module_env: Option<&ModuleEnv>,
) -> inkwell::types::StructType<'a> {
    let max_fields = enum_def
        .variants
        .iter()
        .map(|v| v.fields.len())
        .max()
        .unwrap_or(0);
    let mut field_types: Vec<inkwell::types::BasicTypeEnum> = vec![context.i64_type().into()]; // tag
    for slot in 0..max_fields {
        // Plan 9: Resolve actual field types from EnumDef.variants[*].field_types
        // Collect the type names across all variants at this slot position
        let mut resolved_type: Option<inkwell::types::BasicTypeEnum> = None;
        for variant in &enum_def.variants {
            if slot < variant.field_types.len() {
                let type_name = &variant.field_types[slot].name;
                let slot_type = if let Some(menv) = module_env {
                    resolve_param_type(context, Some(type_name.as_str()), menv)
                } else {
                    match lower(type_name.as_str()) {
                        LoweredType::F64 => context.f64_type().into(),
                        LoweredType::Str => {
                            context.ptr_type(inkwell::AddressSpace::default()).into()
                        }
                        _ => context.i64_type().into(),
                    }
                };
                match resolved_type {
                    None => resolved_type = Some(slot_type),
                    Some(existing) => resolved_type = Some(union_slot_type(existing, slot_type)),
                }
            }
        }
        field_types.push(resolved_type.unwrap_or(context.i64_type().into()));
    }
    context.struct_type(&field_types, false)
}

pub(crate) fn bitpreserve_cast<'a>(
    builder: &Builder<'a>,
    value: BasicValueEnum<'a>,
    target_ty: inkwell::types::BasicTypeEnum<'a>,
) -> mumei_core::verification::MumeiResult<BasicValueEnum<'a>> {
    let source_ty = value.get_type();
    if source_ty == target_ty {
        return Ok(value);
    }

    match (value, target_ty) {
        (BasicValueEnum::IntValue(int_val), inkwell::types::BasicTypeEnum::IntType(target_int)) => {
            let source_bits = int_val.get_type().get_bit_width();
            let target_bits = target_int.get_bit_width();
            if source_bits < target_bits {
                Ok(llvm!(builder.build_int_z_extend(int_val, target_int, "payload_zext")).into())
            } else if source_bits > target_bits {
                Ok(llvm!(builder.build_int_truncate(int_val, target_int, "payload_trunc")).into())
            } else {
                Ok(llvm!(builder.build_bit_cast(
                    int_val,
                    target_int,
                    "payload_cast"
                )))
            }
        }
        (
            BasicValueEnum::IntValue(int_val),
            inkwell::types::BasicTypeEnum::FloatType(target_float),
        ) => {
            if int_val.get_type().get_bit_width() == float_type_bit_width(target_float) {
                Ok(llvm!(builder.build_bit_cast(
                    int_val,
                    target_float,
                    "payload_cast"
                )))
            } else {
                Err(mumei_core::verification::MumeiError::codegen(
                    "unsupported int-to-float payload cast with mismatched width".to_string(),
                ))
            }
        }
        (
            BasicValueEnum::FloatValue(float_val),
            inkwell::types::BasicTypeEnum::IntType(target_int),
        ) => {
            if float_type_bit_width(float_val.get_type()) == target_int.get_bit_width() {
                Ok(llvm!(builder.build_bit_cast(
                    float_val,
                    target_int,
                    "payload_cast"
                )))
            } else {
                Err(mumei_core::verification::MumeiError::codegen(
                    "unsupported float-to-int payload cast with mismatched width".to_string(),
                ))
            }
        }
        (
            BasicValueEnum::PointerValue(ptr_val),
            inkwell::types::BasicTypeEnum::IntType(target_int),
        ) => Ok(llvm!(builder.build_ptr_to_int(ptr_val, target_int, "payload_ptr_to_int")).into()),
        (
            BasicValueEnum::IntValue(int_val),
            inkwell::types::BasicTypeEnum::PointerType(target_ptr),
        ) => Ok(llvm!(builder.build_int_to_ptr(int_val, target_ptr, "payload_int_to_ptr")).into()),
        (
            BasicValueEnum::PointerValue(ptr_val),
            inkwell::types::BasicTypeEnum::PointerType(target_ptr),
        ) => Ok(llvm!(builder.build_bit_cast(
            ptr_val,
            target_ptr,
            "payload_ptr_cast"
        ))),
        (value, target_ty) => Err(mumei_core::verification::MumeiError::codegen(format!(
            "unsupported payload cast from {:?} to {:?}",
            value.get_type(),
            target_ty
        ))),
    }
}

/// パラメータの LLVM 型を解決する
pub(crate) fn resolve_param_type<'a>(
    context: &'a Context,
    type_name: Option<&str>,
    module_env: &ModuleEnv,
) -> inkwell::types::BasicTypeEnum<'a> {
    match type_name {
        Some(name) => {
            let base = module_env.resolve_base_type(name);
            // TODO(strict-preservation): `lower()` unifies `Str`/`String` into
            // `LoweredType::Str`, so `"String"` now maps to a pointer here.
            // Pre-P1-b the arm matched only `"Str"`, so `"String"` fell through
            // to `i64`. This unification is intentional (consistency with the
            // FFI emitters; no `.mm` fixture declares a `String` type today).
            // If exact legacy behavior is ever required, distinguish the Str
            // spelling from String at the `lower()` layer (e.g. a dedicated
            // variant) rather than re-adding a string match here. Same note
            // applies to `param_z3_value` in verification/translator.rs.
            match lower(&base) {
                LoweredType::F64 => context.f64_type().into(),
                LoweredType::Str => context.ptr_type(inkwell::AddressSpace::default()).into(),
                LoweredType::Array(inner) if matches!(inner.as_ref(), LoweredType::I64) => {
                    array_struct_type(context).into()
                }
                _ => {
                    // Plan 14: Check if type is an enum
                    if let Some(enum_def) = module_env.get_enum(name) {
                        return enum_llvm_type(context, enum_def, Some(module_env)).into();
                    }
                    context.i64_type().into()
                }
            }
        }
        None => context.i64_type().into(),
    }
}

/// Emit LLVM `declare` for each function in the extern blocks so that
/// call-site codegen can resolve them via `module.get_function(name)`.
pub fn declare_extern_functions<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    extern_blocks: &[mumei_core::parser::ExternBlock],
    module_env: &ModuleEnv,
) {
    for eb in extern_blocks {
        for ext_fn in &eb.functions {
            // Skip if already declared (e.g. by a previous block)
            if module.get_function(&ext_fn.name).is_some() {
                continue;
            }
            let param_types: Vec<BasicMetadataTypeEnum> = ext_fn
                .param_types
                .iter()
                .map(|ty| resolve_param_type(context, Some(ty.as_str()), module_env).into())
                .collect();

            let ret_base = module_env.resolve_base_type(&ext_fn.return_type);
            let fn_type = match lower(&ret_base) {
                LoweredType::F64 => context.f64_type().fn_type(&param_types, false),
                // Plan 9: Str return type as pointer
                LoweredType::Str => context
                    .ptr_type(inkwell::AddressSpace::default())
                    .fn_type(&param_types, false),
                _ => context.i64_type().fn_type(&param_types, false),
            };
            // Both "C" and "Rust" FFI use the C calling convention
            module.add_function(
                &ext_fn.name,
                fn_type,
                Some(inkwell::module::Linkage::External),
            );
        }
    }
}

pub(crate) fn resolve_return_type<'a>(
    context: &'a Context,
    atom: &mumei_core::parser::Atom,
    module_env: &ModuleEnv,
) -> inkwell::types::BasicTypeEnum<'a> {
    if let Some(ref ret_type) = atom.return_type {
        let base = module_env.resolve_base_type(ret_type);
        match lower(&base) {
            LoweredType::F64 => context.f64_type().into(),
            LoweredType::Str => context.ptr_type(AddressSpace::default()).into(),
            LoweredType::Array(inner) if matches!(inner.as_ref(), LoweredType::I64) => {
                array_struct_type(context).into()
            }
            _ => {
                if let Some(enum_def) = module_env.get_enum(&base) {
                    return enum_llvm_type(context, enum_def, Some(module_env)).into();
                }
                context.i64_type().into()
            }
        }
    } else {
        // Infer the return type from the body when possible; otherwise fall
        // back to a conservative i64 default.
        if let Some(ret_type) = mumei_core::mir::infer_atom_return_type(atom) {
            resolve_param_type(context, Some(&ret_type), module_env)
        } else {
            context.i64_type().into()
        }
    }
}
