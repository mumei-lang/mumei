#[cfg(test)]
use inkwell::context::Context;
#[cfg(test)]
use inkwell::AddressSpace;
#[cfg(test)]
use mumei_core::verification::ModuleEnv;

/// LLVM Builder の Result を簡潔にアンラップするマクロ
macro_rules! llvm {
    ($e:expr) => {
        $e.map_err(|e| MumeiError::codegen(e.to_string()))?
    };
}

mod driver;
mod expr_emit;
mod lowering;
mod pattern_emit;
mod stmt_emit;
mod task_runtime;

pub use driver::{
    compile, compile_atom_into_module, compile_atoms_into_module, compile_llvm_ir_to_object,
    compile_to_module,
};
pub use lowering::declare_extern_functions;

#[cfg(test)]
use lowering::{array_struct_type, resolve_param_type, resolve_return_type};

#[cfg(test)]
mod tests {
    use super::*;
    use mumei_core::hir::HirSignature;
    use mumei_core::parser::ast::Span;
    use mumei_core::parser::{parse_type_ref, EnumDef, EnumVariant};

    fn signature_with_return_type(
        return_type: Option<&str>,
        inferred_return_type: Option<&str>,
    ) -> HirSignature {
        HirSignature {
            name: "test".to_string(),
            params: vec![],
            effects: vec![],
            return_type: return_type.map(str::to_string),
            inferred_return_type: inferred_return_type.map(str::to_string),
            is_async: false,
        }
    }

    #[test]
    fn test_resolve_param_type_uses_lowered_types() {
        let context = Context::create();
        let module_env = ModuleEnv::new();

        assert_eq!(
            resolve_param_type(&context, Some("f64"), &module_env),
            context.f64_type().into()
        );
        assert_eq!(
            resolve_param_type(&context, Some("Str"), &module_env),
            context.ptr_type(AddressSpace::default()).into()
        );
        assert_eq!(
            resolve_param_type(&context, Some("[i64]"), &module_env),
            array_struct_type(&context).into()
        );
        assert_eq!(
            resolve_param_type(&context, Some("String"), &module_env),
            context.ptr_type(AddressSpace::default()).into()
        );
    }

    #[test]
    fn test_resolve_return_type_uses_lowered_types() {
        let context = Context::create();
        let module_env = ModuleEnv::new();

        let f64_sig = signature_with_return_type(Some("f64"), None);
        assert_eq!(
            resolve_return_type(&context, &f64_sig, &module_env),
            context.f64_type().into()
        );

        let str_sig = signature_with_return_type(Some("Str"), None);
        assert_eq!(
            resolve_return_type(&context, &str_sig, &module_env),
            context.ptr_type(AddressSpace::default()).into()
        );

        let array_sig = signature_with_return_type(Some("[i64]"), None);
        assert_eq!(
            resolve_return_type(&context, &array_sig, &module_env),
            array_struct_type(&context).into()
        );

        let string_sig = signature_with_return_type(Some("String"), None);
        assert_eq!(
            resolve_return_type(&context, &string_sig, &module_env),
            context.ptr_type(AddressSpace::default()).into()
        );
    }

    #[test]
    fn test_resolve_return_type_uses_inferred_type() {
        let context = Context::create();
        let module_env = ModuleEnv::new();

        let f64_sig = signature_with_return_type(None, Some("f64"));
        assert_eq!(
            resolve_return_type(&context, &f64_sig, &module_env),
            context.f64_type().into()
        );

        // An inferred `bool` resolves through the bool-as-i64 convention.
        let bool_sig = signature_with_return_type(None, Some("bool"));
        assert_eq!(
            resolve_return_type(&context, &bool_sig, &module_env),
            context.i64_type().into()
        );
    }

    #[test]
    fn test_resolve_return_type_defaults_to_i64_without_inference() {
        let context = Context::create();
        let module_env = ModuleEnv::new();
        let sig = signature_with_return_type(None, None);

        assert_eq!(
            resolve_return_type(&context, &sig, &module_env),
            context.i64_type().into()
        );
    }

    #[test]
    fn test_enum_llvm_type_uses_f64_union_slot() {
        let context = Context::create();
        let mut module_env = ModuleEnv::new();
        let enum_def = EnumDef {
            name: "Num".to_string(),
            type_params: vec![],
            variants: vec![
                EnumVariant {
                    name: "I".to_string(),
                    fields: vec!["value".to_string()],
                    field_types: vec![parse_type_ref("i64")],
                    is_recursive: false,
                },
                EnumVariant {
                    name: "F".to_string(),
                    fields: vec!["value".to_string()],
                    field_types: vec![parse_type_ref("f64")],
                    is_recursive: false,
                },
            ],
            is_recursive: false,
            span: Span::default(),
        };
        module_env.register_enum(&enum_def);

        let enum_ty = lowering::enum_llvm_type(&context, &enum_def, Some(&module_env));
        assert_eq!(
            enum_ty.get_field_type_at_index(1).unwrap(),
            context.f64_type().into()
        );
    }
}
