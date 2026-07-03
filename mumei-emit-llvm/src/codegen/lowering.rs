use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::types::BasicMetadataTypeEnum;
use inkwell::AddressSpace;
use mumei_core::lowering::{lower, LoweredType};
use mumei_core::verification::ModuleEnv;

pub(crate) fn array_struct_type(context: &Context) -> inkwell::types::StructType<'_> {
    let i64_type = context.i64_type();
    let ptr_type = context.ptr_type(AddressSpace::default());
    context.struct_type(&[i64_type.into(), ptr_type.into()], false)
}

/// Plan 14: Generate LLVM struct type for an enum (tagged union).
/// Layout: { i64 tag, i64 payload_0, i64 payload_1, ... }
/// The number of payload slots is the maximum field count across all variants.
///
/// Payload slots are resolved from `EnumDef` variant field types.
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
                    Some(existing) => {
                        // TODO(followup): when variants disagree on a slot's payload type, defaulting to i64 discards an f64's bit-width, corrupting heterogeneous payloads. Proper fix uses a union/largest-type slot layout. See PR #388 review.
                        // If types differ across variants, use the largest compatible type.
                        // ptr and i64 are same size on 64-bit; f64 needs its own slot.
                        if existing != slot_type {
                            // Default to i64 as the most general integer type
                            resolved_type = Some(context.i64_type().into());
                        }
                    }
                }
            }
        }
        field_types.push(resolved_type.unwrap_or(context.i64_type().into()));
    }
    context.struct_type(&field_types, false)
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
        // TODO(followup): inferring f64 return from f64 params is unsound for unannotated atoms that return a non-f64 (e.g. compare(f64,f64)->i64). A proper fix propagates an inferred return type onto Atom during an earlier phase, since the callee-resolution path only has parser::Atom (no typed body). See PR #388 review.
        // Fallback heuristic: if any parameter is f64, assume f64 return type.
        // This preserves backward compatibility for atoms without explicit -> Type.
        let has_float = atom.params.iter().any(|p| {
            p.type_name
                .as_deref()
                .map(|t| module_env.resolve_base_type(t) == "f64")
                .unwrap_or(false)
        });
        if has_float {
            context.f64_type().into()
        } else {
            context.i64_type().into()
        }
    }
}
