use crate::codegen::expr_emit::{compile_hir_expr, infer_struct_type_name};
use crate::codegen::task_runtime::declare_task_group_should_cancel_current_extern;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::values::{BasicValueEnum, FunctionValue, PhiValue};
use inkwell::AddressSpace;
use inkwell::IntPredicate;
use mumei_core::hir::HirStmt;
use mumei_core::verification::{ModuleEnv, MumeiError, MumeiResult};
use std::collections::HashMap;

#[allow(clippy::too_many_arguments)]
pub(crate) fn compile_hir_stmt<'a>(
    context: &'a Context,
    builder: &Builder<'a>,
    module: &Module<'a>,
    function: &FunctionValue<'a>,
    stmt: &HirStmt,
    variables: &mut HashMap<String, BasicValueEnum<'a>>,
    var_types: &mut HashMap<String, String>,
    array_ptrs: &HashMap<String, (BasicValueEnum<'a>, BasicValueEnum<'a>)>,
    module_env: &ModuleEnv,
) -> MumeiResult<BasicValueEnum<'a>> {
    match stmt {
        HirStmt::Let { var, ty, value } => {
            let val = compile_hir_expr(
                context, builder, module, function, value, variables, var_types, array_ptrs,
                module_env,
            )?;
            variables.insert(var.clone(), val);
            let inferred_ty = ty
                .as_deref()
                .map(|ty_name| module_env.resolve_base_type(ty_name))
                .filter(|base| module_env.get_struct(base).is_some())
                .or_else(|| infer_struct_type_name(value, var_types, module_env));
            if let Some(struct_ty) = inferred_ty {
                var_types.insert(var.clone(), struct_ty);
            } else {
                // Clear any stale type from a prior binding of this name so a
                // later field access cannot resolve against the wrong struct.
                var_types.remove(var);
            }
            Ok(val)
        }
        HirStmt::Assign { var, value } => {
            let val = compile_hir_expr(
                context, builder, module, function, value, variables, var_types, array_ptrs,
                module_env,
            )?;
            variables.insert(var.clone(), val);
            if let Some(struct_ty) = infer_struct_type_name(value, var_types, module_env) {
                var_types.insert(var.clone(), struct_ty);
            } else {
                // Reassignment to a value with no known struct type must not
                // leave a stale mapping from the previous binding.
                var_types.remove(var);
            }
            Ok(val)
        }
        HirStmt::ArrayStore {
            array,
            index,
            value,
        } => {
            let val = compile_hir_expr(
                context, builder, module, function, value, variables, var_types, array_ptrs,
                module_env,
            )?;
            let idx = compile_hir_expr(
                context, builder, module, function, index, variables, var_types, array_ptrs,
                module_env,
            )?
            .into_int_value();
            if let Some((len_val, data_ptr_val)) = array_ptrs.get(array.as_str()) {
                let data_ptr = data_ptr_val.into_pointer_value();
                let len_int = len_val.into_int_value();

                // Guard the GEP + store with an in-bounds check that mirrors
                // ArrayAccess read-side handling. Out-of-bounds writes are
                // skipped rather than crashing the process.
                let in_bounds = llvm!(builder.build_int_compare(
                    IntPredicate::SLT,
                    idx,
                    len_int,
                    "store_bounds_check"
                ));
                let non_neg = llvm!(builder.build_int_compare(
                    IntPredicate::SGE,
                    idx,
                    context.i64_type().const_int(0, false),
                    "store_non_neg_check"
                ));
                let safe = llvm!(builder.build_and(in_bounds, non_neg, "store_safe"));

                let safe_block = context.append_basic_block(*function, "arr.store.safe");
                let merge_block = context.append_basic_block(*function, "arr.store.merge");
                llvm!(builder.build_conditional_branch(safe, safe_block, merge_block));

                builder.position_at_end(safe_block);
                let elem_ptr = unsafe {
                    llvm!(builder.build_gep(context.i64_type(), data_ptr, &[idx], "store_elem_ptr"))
                };
                llvm!(builder.build_store(elem_ptr, val.into_int_value()));
                llvm!(builder.build_unconditional_branch(merge_block));

                builder.position_at_end(merge_block);
                Ok(val)
            } else {
                Err(MumeiError::codegen(format!(
                    "Array '{}' not found as fat pointer parameter",
                    array
                )))
            }
        }
        HirStmt::While {
            cond,
            invariant: _,
            decreases: _,
            body,
        } => {
            let header_block = context.append_basic_block(*function, "loop.header");
            let cond_block = context.append_basic_block(*function, "loop.cond");
            let body_block = context.append_basic_block(*function, "loop.body");
            let after_block = context.append_basic_block(*function, "loop.after");

            let pre_loop_vars = variables.clone();
            let entry_end_block = builder.get_insert_block().unwrap();

            llvm!(builder.build_unconditional_branch(header_block));

            builder.position_at_end(header_block);
            let mut phi_nodes: Vec<(String, PhiValue<'a>)> = Vec::new();
            for (name, pre_val) in &pre_loop_vars {
                let phi = llvm!(builder.build_phi(pre_val.get_type(), &format!("phi_{}", name)));
                phi.add_incoming(&[(pre_val, entry_end_block)]);
                phi_nodes.push((name.clone(), phi));
                variables.insert(name.clone(), phi.as_basic_value());
            }
            let should_cancel_fn = declare_task_group_should_cancel_current_extern(context, module);
            let cancel_flag =
                llvm!(builder.build_call(should_cancel_fn, &[], "task_group_cancel_check",))
                    .try_as_basic_value()
                    .left()
                    .ok_or_else(|| MumeiError::codegen("cancel check returned void".to_string()))?
                    .into_int_value();
            let is_cancelled = llvm!(builder.build_int_compare(
                IntPredicate::NE,
                cancel_flag,
                context.i64_type().const_int(0, false),
                "loop_cancelled",
            ));
            llvm!(builder.build_conditional_branch(is_cancelled, after_block, cond_block));

            builder.position_at_end(cond_block);
            let cond_val = compile_hir_expr(
                context, builder, module, function, cond, variables, var_types, array_ptrs,
                module_env,
            )?
            .into_int_value();
            let cond_bool = llvm!(builder.build_int_compare(
                IntPredicate::NE,
                cond_val,
                context.i64_type().const_int(0, false),
                "loop_cond"
            ));
            llvm!(builder.build_conditional_branch(cond_bool, body_block, after_block));

            builder.position_at_end(body_block);
            compile_hir_stmt(
                context, builder, module, function, body, variables, var_types, array_ptrs,
                module_env,
            )?;
            let body_end_block = builder.get_insert_block().unwrap();

            for (name, phi) in &phi_nodes {
                if let Some(body_val) = variables.get(name) {
                    phi.add_incoming(&[(body_val, body_end_block)]);
                }
            }

            llvm!(builder.build_unconditional_branch(header_block));

            builder.position_at_end(after_block);
            for (name, phi) in &phi_nodes {
                variables.insert(name.clone(), phi.as_basic_value());
            }
            Ok(context.i64_type().const_int(0, false).into())
        }
        HirStmt::Block { stmts, tail_expr } => {
            let mut last_val: BasicValueEnum = context.i64_type().const_int(0, false).into();
            for s in stmts {
                last_val = compile_hir_stmt(
                    context, builder, module, function, s, variables, var_types, array_ptrs,
                    module_env,
                )?;
            }
            if let Some(tail) = tail_expr {
                last_val = compile_hir_expr(
                    context, builder, module, function, tail, variables, var_types, array_ptrs,
                    module_env,
                )?;
            }
            Ok(last_val)
        }
        HirStmt::Acquire { resource, body } => {
            let ptr_type = context.ptr_type(AddressSpace::default());
            let i32_type = context.i32_type();

            let lock_fn = module
                .get_function("pthread_mutex_lock")
                .unwrap_or_else(|| {
                    let fn_type = i32_type.fn_type(&[ptr_type.into()], false);
                    module.add_function(
                        "pthread_mutex_lock",
                        fn_type,
                        Some(inkwell::module::Linkage::External),
                    )
                });
            let unlock_fn = module
                .get_function("pthread_mutex_unlock")
                .unwrap_or_else(|| {
                    let fn_type = i32_type.fn_type(&[ptr_type.into()], false);
                    module.add_function(
                        "pthread_mutex_unlock",
                        fn_type,
                        Some(inkwell::module::Linkage::External),
                    )
                });
            let get_resource_fn = module
                .get_function("__mumei_get_resource_mutex")
                .unwrap_or_else(|| {
                    let fn_type = ptr_type.fn_type(&[ptr_type.into()], false);
                    module.add_function(
                        "__mumei_get_resource_mutex",
                        fn_type,
                        Some(inkwell::module::Linkage::External),
                    )
                });
            let resource_name = builder
                .build_global_string_ptr(resource, &format!("resource_name_{}", resource))
                .map_err(|e| MumeiError::codegen(format!("Failed to build resource name: {e}")))?;
            let mutex_call = llvm!(builder.build_call(
                get_resource_fn,
                &[resource_name.as_pointer_value().into()],
                &format!("resource_{}", resource)
            ));
            let mutex_ptr = mutex_call
                .try_as_basic_value()
                .left()
                .ok_or_else(|| {
                    MumeiError::codegen("resource mutex helper returned void".to_string())
                })?
                .into_pointer_value();

            llvm!(builder.build_call(lock_fn, &[mutex_ptr.into()], &format!("lock_{}", resource)));

            let body_result = compile_hir_stmt(
                context, builder, module, function, body, variables, var_types, array_ptrs,
                module_env,
            )?;

            llvm!(builder.build_call(
                unlock_fn,
                &[mutex_ptr.into()],
                &format!("unlock_{}", resource)
            ));

            Ok(body_result)
        }
        HirStmt::Expr(expr) => compile_hir_expr(
            context, builder, module, function, expr, variables, var_types, array_ptrs, module_env,
        ),
    }
}
