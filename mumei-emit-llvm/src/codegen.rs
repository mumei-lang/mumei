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
    use mumei_core::parser::ast::Span;
    use mumei_core::parser::{parse_type_ref, Atom, EnumDef, EnumVariant, Param};

    fn atom_with_return_type(return_type: Option<&str>) -> Atom {
        Atom {
            name: "test".to_string(),
            type_params: vec![],
            where_bounds: vec![],
            params: vec![],
            trace_id: None,
            spec_metadata: Default::default(),
            requires: "true".to_string(),
            forall_constraints: vec![],
            ensures: "true".to_string(),
            body_expr: "true".to_string(),
            consumed_params: vec![],
            resources: vec![],
            is_async: false,
            trust_level: mumei_core::parser::TrustLevel::Verified,
            max_unroll: None,
            invariant: None,
            effects: vec![],
            return_type: return_type.map(str::to_string),
            span: Span::default(),
            effect_pre: Default::default(),
            effect_post: Default::default(),
        }
    }

    fn make_param(name: &str, ty: &str) -> Param {
        Param {
            name: name.to_string(),
            type_name: Some(ty.to_string()),
            type_ref: Some(parse_type_ref(ty)),
            is_ref: false,
            is_ref_mut: false,
            fn_contract_requires: None,
            fn_contract_ensures: None,
        }
    }

    fn atom_with_body_and_params(body_expr: &str, params: Vec<Param>) -> Atom {
        Atom {
            params,
            body_expr: body_expr.to_string(),
            ..atom_with_return_type(None)
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

        let f64_atom = atom_with_return_type(Some("f64"));
        assert_eq!(
            resolve_return_type(&context, &f64_atom, &module_env),
            context.f64_type().into()
        );

        let str_atom = atom_with_return_type(Some("Str"));
        assert_eq!(
            resolve_return_type(&context, &str_atom, &module_env),
            context.ptr_type(AddressSpace::default()).into()
        );

        let array_atom = atom_with_return_type(Some("[i64]"));
        assert_eq!(
            resolve_return_type(&context, &array_atom, &module_env),
            array_struct_type(&context).into()
        );

        let string_atom = atom_with_return_type(Some("String"));
        assert_eq!(
            resolve_return_type(&context, &string_atom, &module_env),
            context.ptr_type(AddressSpace::default()).into()
        );
    }

    #[test]
    fn test_resolve_return_type_infers_bool_body_conservatively_defaults_to_i64() {
        let context = Context::create();
        let module_env = ModuleEnv::new();
        let atom = atom_with_body_and_params(
            "a < b",
            vec![make_param("a", "f64"), make_param("b", "f64")],
        );

        assert_eq!(
            resolve_return_type(&context, &atom, &module_env),
            context.i64_type().into()
        );
    }

    #[test]
    fn test_resolve_return_type_infers_f64_body() {
        let context = Context::create();
        let module_env = ModuleEnv::new();
        let atom = atom_with_body_and_params(
            "a + b",
            vec![make_param("a", "f64"), make_param("b", "f64")],
        );

        assert_eq!(
            resolve_return_type(&context, &atom, &module_env),
            context.f64_type().into()
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
