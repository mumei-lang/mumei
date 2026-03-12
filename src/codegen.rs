use crate::hir::{HirAtom, HirExpr, HirStmt};
use crate::parser::{Op, Pattern};
use crate::verification::{ModuleEnv, MumeiError, MumeiResult};
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::types::BasicMetadataTypeEnum;
use inkwell::values::{AnyValue, BasicMetadataValueEnum, BasicValueEnum, FunctionValue, PhiValue};
use inkwell::AddressSpace;
use inkwell::FloatPredicate;
use inkwell::IntPredicate;
use std::collections::HashMap;
use std::path::Path;

/// LLVM Builder の Result を簡潔にアンラップするマクロ
macro_rules! llvm {
    ($e:expr) => {
        $e.map_err(|e| MumeiError::codegen(e.to_string()))?
    };
}

/// Fat Pointer 配列の構造体型 { i64, i64* } を生成するヘルパー
fn array_struct_type(context: &Context) -> inkwell::types::StructType<'_> {
    let i64_type = context.i64_type();
    let ptr_type = context.ptr_type(AddressSpace::default());
    context.struct_type(&[i64_type.into(), ptr_type.into()], false)
}

/// パラメータの LLVM 型を解決する
fn resolve_param_type<'a>(
    context: &'a Context,
    type_name: Option<&str>,
    module_env: &ModuleEnv,
) -> inkwell::types::BasicTypeEnum<'a> {
    match type_name {
        Some(name) => {
            let base = module_env.resolve_base_type(name);
            match base.as_str() {
                "f64" => context.f64_type().into(),
                "u64" => context.i64_type().into(),
                "[i64]" => array_struct_type(context).into(),
                _ => context.i64_type().into(),
            }
        }
        None => context.i64_type().into(),
    }
}

pub fn compile(hir_atom: &HirAtom, output_path: &Path, module_env: &ModuleEnv) -> MumeiResult<()> {
    let atom = &hir_atom.atom;
    let context = Context::create();
    let module = context.create_module(&atom.name);
    let builder = context.create_builder();

    let i64_type = context.i64_type();

    // パラメータ型を精緻型から解決
    let param_types: Vec<inkwell::types::BasicMetadataTypeEnum> = atom
        .params
        .iter()
        .map(|p| resolve_param_type(&context, p.type_name.as_deref(), module_env).into())
        .collect();
    let fn_type = i64_type.fn_type(&param_types, false);
    let function = module.add_function(&atom.name, fn_type, None);

    let entry_block = context.append_basic_block(function, "entry");
    builder.position_at_end(entry_block);

    let mut variables = HashMap::new();
    let mut array_ptrs: HashMap<String, (BasicValueEnum, BasicValueEnum)> = HashMap::new(); // name -> (len, data_ptr)

    for (i, param) in atom.params.iter().enumerate() {
        let val = function.get_nth_param(i as u32).unwrap();
        // Fat Pointer 配列パラメータの場合、len と data_ptr を分解して保持
        if val.is_struct_value() {
            let struct_val = val.into_struct_value();
            let len_val =
                llvm!(builder.build_extract_value(struct_val, 0, &format!("{}_len", param.name)));
            let data_ptr =
                llvm!(builder.build_extract_value(struct_val, 1, &format!("{}_data", param.name)));
            array_ptrs.insert(param.name.clone(), (len_val, data_ptr));
            variables.insert(param.name.clone(), len_val); // デフォルトでは len を返す
        } else {
            variables.insert(param.name.clone(), val);
        }
    }

    // エフェクト情報を .ll ファイル先頭にコメントとして追記する（後処理）
    let effects_comment = if !atom.effects.is_empty() {
        let effects_str: Vec<String> = atom
            .effects
            .iter()
            .map(|e| {
                if e.params.is_empty() {
                    e.name.clone()
                } else {
                    let params: Vec<String> = e
                        .params
                        .iter()
                        .map(|p| format!("\"{}\"", p.value))
                        .collect();
                    format!("{}({})", e.name, params.join(", "))
                }
            })
            .collect();
        Some(format!("; effects: [{}]", effects_str.join(", ")))
    } else {
        None
    };

    let result_val = compile_hir_stmt(
        &context,
        &builder,
        &module,
        &function,
        &hir_atom.body,
        &mut variables,
        &array_ptrs,
        module_env,
    )?;

    llvm!(builder.build_return(Some(&result_val)));

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

#[allow(clippy::too_many_arguments)]
fn compile_hir_stmt<'a>(
    context: &'a Context,
    builder: &Builder<'a>,
    module: &Module<'a>,
    function: &FunctionValue<'a>,
    stmt: &HirStmt,
    variables: &mut HashMap<String, BasicValueEnum<'a>>,
    array_ptrs: &HashMap<String, (BasicValueEnum<'a>, BasicValueEnum<'a>)>,
    module_env: &ModuleEnv,
) -> MumeiResult<BasicValueEnum<'a>> {
    match stmt {
        HirStmt::Let { var, value, .. } => {
            let val = compile_hir_expr(
                context, builder, module, function, value, variables, array_ptrs, module_env,
            )?;
            variables.insert(var.clone(), val);
            Ok(val)
        }
        HirStmt::Assign { var, value } => {
            let val = compile_hir_expr(
                context, builder, module, function, value, variables, array_ptrs, module_env,
            )?;
            variables.insert(var.clone(), val);
            Ok(val)
        }
        HirStmt::While {
            cond,
            invariant: _,
            decreases: _,
            body,
        } => {
            let header_block = context.append_basic_block(*function, "loop.header");
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

            let cond_val = compile_hir_expr(
                context, builder, module, function, cond, variables, array_ptrs, module_env,
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
                context, builder, module, function, body, variables, array_ptrs, module_env,
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
                    context, builder, module, function, s, variables, array_ptrs, module_env,
                )?;
            }
            if let Some(tail) = tail_expr {
                last_val = compile_hir_expr(
                    context, builder, module, function, tail, variables, array_ptrs, module_env,
                )?;
            }
            Ok(last_val)
        }
        HirStmt::Acquire { resource, body } => {
            // --- mutex_lock/unlock の外部関数宣言 ---
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

            let global_name = format!("__mumei_resource_{}", resource);
            let mutex_global = module.get_global(&global_name).unwrap_or_else(|| {
                let i8_type = context.i8_type();
                let global =
                    module.add_global(i8_type, Some(AddressSpace::default()), &global_name);
                global.set_linkage(inkwell::module::Linkage::External);
                global
            });
            let mutex_ptr = mutex_global.as_pointer_value();

            llvm!(builder.build_call(lock_fn, &[mutex_ptr.into()], &format!("lock_{}", resource)));

            let body_result = compile_hir_stmt(
                context, builder, module, function, body, variables, array_ptrs, module_env,
            )?;

            llvm!(builder.build_call(
                unlock_fn,
                &[mutex_ptr.into()],
                &format!("unlock_{}", resource)
            ));

            Ok(body_result)
        }
        HirStmt::Expr(expr) => compile_hir_expr(
            context, builder, module, function, expr, variables, array_ptrs, module_env,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn compile_hir_expr<'a>(
    context: &'a Context,
    builder: &Builder<'a>,
    module: &Module<'a>,
    function: &FunctionValue<'a>,
    expr: &HirExpr,
    variables: &mut HashMap<String, BasicValueEnum<'a>>,
    array_ptrs: &HashMap<String, (BasicValueEnum<'a>, BasicValueEnum<'a>)>,
    module_env: &ModuleEnv,
) -> MumeiResult<BasicValueEnum<'a>> {
    match expr {
        HirExpr::Number(n) => Ok(context.i64_type().const_int(*n as u64, true).into()),

        HirExpr::Float(f) => Ok(context.f64_type().const_float(*f).into()),

        HirExpr::Variable(name) => variables
            .get(name.as_str())
            .cloned()
            .ok_or_else(|| MumeiError::codegen(format!("Undefined variable: {}", name))),

        HirExpr::Call { name, args } => match name.as_str() {
            "sqrt" => {
                let arg = compile_hir_expr(
                    context, builder, module, function, &args[0], variables, array_ptrs, module_env,
                )?;
                let sqrt_func = module.get_function("llvm.sqrt.f64").unwrap_or_else(|| {
                    let type_f64 = context.f64_type();
                    let fn_type = type_f64.fn_type(&[type_f64.into()], false);
                    module.add_function("llvm.sqrt.f64", fn_type, None)
                });
                let call = llvm!(builder.build_call(sqrt_func, &[arg.into()], "sqrt_tmp"));
                let result = call.as_any_value_enum();
                Ok(result.into_float_value().into())
            }
            "len" => {
                if !args.is_empty() {
                    if let HirExpr::Variable(arr_name) = &args[0] {
                        if let Some((len_val, _)) = array_ptrs.get(arr_name.as_str()) {
                            return Ok(*len_val);
                        }
                    }
                }
                Ok(context.i64_type().const_int(0, false).into())
            }
            "alloc_raw" => {
                let size_val = compile_hir_expr(
                    context, builder, module, function, &args[0], variables, array_ptrs, module_env,
                )?;
                let malloc_fn = module.get_function("malloc").unwrap_or_else(|| {
                    let ptr_type = context.ptr_type(AddressSpace::default());
                    let fn_type = ptr_type.fn_type(&[context.i64_type().into()], false);
                    module.add_function("malloc", fn_type, Some(inkwell::module::Linkage::External))
                });
                let byte_size = llvm!(builder.build_int_mul(
                    size_val.into_int_value(),
                    context.i64_type().const_int(8, false),
                    "byte_size"
                ));
                let ptr =
                    llvm!(builder.build_call(malloc_fn, &[byte_size.into()], "malloc_result"));
                let ptr_val = ptr.as_any_value_enum().into_pointer_value();
                Ok(
                    llvm!(builder.build_ptr_to_int(ptr_val, context.i64_type(), "ptr_as_int"))
                        .into(),
                )
            }
            "dealloc_raw" => {
                let ptr_int = compile_hir_expr(
                    context, builder, module, function, &args[0], variables, array_ptrs, module_env,
                )?;
                let free_fn = module.get_function("free").unwrap_or_else(|| {
                    let ptr_type = context.ptr_type(AddressSpace::default());
                    let fn_type = context.void_type().fn_type(&[ptr_type.into()], false);
                    module.add_function("free", fn_type, Some(inkwell::module::Linkage::External))
                });
                let ptr_val = llvm!(builder.build_int_to_ptr(
                    ptr_int.into_int_value(),
                    context.ptr_type(AddressSpace::default()),
                    "int_as_ptr"
                ));
                llvm!(builder.build_call(free_fn, &[ptr_val.into()], "free_call"));
                Ok(context.i64_type().const_int(0, false).into())
            }
            _ => {
                let fqn_name = name.replace('.', "::");
                let resolved_callee = module_env
                    .get_atom(name)
                    .or_else(|| module_env.get_atom(&fqn_name));
                if let Some(callee) = resolved_callee {
                    let callee_param_types: Vec<inkwell::types::BasicMetadataTypeEnum> = callee
                        .params
                        .iter()
                        .map(|p| {
                            resolve_param_type(context, p.type_name.as_deref(), module_env).into()
                        })
                        .collect();

                    let has_float = callee.params.iter().any(|p| {
                        p.type_name
                            .as_deref()
                            .map(|t| module_env.resolve_base_type(t) == "f64")
                            .unwrap_or(false)
                    });
                    let callee_fn = if has_float {
                        let fn_type = context.f64_type().fn_type(&callee_param_types, false);
                        module.get_function(name).unwrap_or_else(|| {
                            module.add_function(
                                name,
                                fn_type,
                                Some(inkwell::module::Linkage::External),
                            )
                        })
                    } else {
                        let fn_type = context.i64_type().fn_type(&callee_param_types, false);
                        module.get_function(name).unwrap_or_else(|| {
                            module.add_function(
                                name,
                                fn_type,
                                Some(inkwell::module::Linkage::External),
                            )
                        })
                    };

                    let mut arg_vals: Vec<inkwell::values::BasicMetadataValueEnum> = Vec::new();
                    for arg in args {
                        let val = compile_hir_expr(
                            context, builder, module, function, arg, variables, array_ptrs,
                            module_env,
                        )?;
                        arg_vals.push(val.into());
                    }

                    let call_result =
                        llvm!(builder.build_call(callee_fn, &arg_vals, &format!("call_{}", name)));
                    let result = call_result.as_any_value_enum();
                    if has_float {
                        Ok(result.into_float_value().into())
                    } else {
                        Ok(result.into_int_value().into())
                    }
                } else {
                    Err(MumeiError::codegen(format!("Unknown function {}", name)))
                }
            }
        },

        HirExpr::ArrayAccess(name, index_expr) => {
            let idx = compile_hir_expr(
                context, builder, module, function, index_expr, variables, array_ptrs, module_env,
            )?
            .into_int_value();
            if let Some((len_val, data_ptr_val)) = array_ptrs.get(name.as_str()) {
                let data_ptr = data_ptr_val.into_pointer_value();
                let len_int = len_val.into_int_value();
                let in_bounds = llvm!(builder.build_int_compare(
                    IntPredicate::SLT,
                    idx,
                    len_int,
                    "bounds_check"
                ));
                let non_neg = llvm!(builder.build_int_compare(
                    IntPredicate::SGE,
                    idx,
                    context.i64_type().const_int(0, false),
                    "non_neg_check"
                ));
                let safe = llvm!(builder.build_and(in_bounds, non_neg, "safe_access"));

                let safe_block = context.append_basic_block(*function, "arr.safe");
                let oob_block = context.append_basic_block(*function, "arr.oob");
                let merge_block = context.append_basic_block(*function, "arr.merge");

                llvm!(builder.build_conditional_branch(safe, safe_block, oob_block));

                builder.position_at_end(safe_block);
                let elem_ptr = unsafe {
                    llvm!(builder.build_gep(context.i64_type(), data_ptr, &[idx], "elem_ptr"))
                };
                let loaded = llvm!(builder.build_load(context.i64_type(), elem_ptr, "elem_val"));
                let safe_end = builder.get_insert_block().unwrap();
                llvm!(builder.build_unconditional_branch(merge_block));

                builder.position_at_end(oob_block);
                let zero_val = context.i64_type().const_int(0, false);
                let oob_end = builder.get_insert_block().unwrap();
                llvm!(builder.build_unconditional_branch(merge_block));

                builder.position_at_end(merge_block);
                let phi = llvm!(builder.build_phi(context.i64_type(), "arr_result"));
                phi.add_incoming(&[(&loaded, safe_end), (&zero_val, oob_end)]);
                Ok(phi.as_basic_value())
            } else {
                Err(MumeiError::codegen(format!(
                    "Array '{}' not found as fat pointer parameter",
                    name
                )))
            }
        }

        HirExpr::BinaryOp(left, op, right) => {
            let lhs = compile_hir_expr(
                context, builder, module, function, left, variables, array_ptrs, module_env,
            )?;
            let rhs = compile_hir_expr(
                context, builder, module, function, right, variables, array_ptrs, module_env,
            )?;

            if lhs.is_float_value() || rhs.is_float_value() {
                let l = if lhs.is_float_value() {
                    lhs.into_float_value()
                } else {
                    llvm!(builder.build_signed_int_to_float(
                        lhs.into_int_value(),
                        context.f64_type(),
                        "int_to_float_l"
                    ))
                };
                let r = if rhs.is_float_value() {
                    rhs.into_float_value()
                } else {
                    llvm!(builder.build_signed_int_to_float(
                        rhs.into_int_value(),
                        context.f64_type(),
                        "int_to_float_r"
                    ))
                };
                match op {
                    Op::Add => Ok(llvm!(builder.build_float_add(l, r, "fadd_tmp")).into()),
                    Op::Sub => Ok(llvm!(builder.build_float_sub(l, r, "fsub_tmp")).into()),
                    Op::Mul => Ok(llvm!(builder.build_float_mul(l, r, "fmul_tmp")).into()),
                    Op::Div => Ok(llvm!(builder.build_float_div(l, r, "fdiv_tmp")).into()),
                    Op::Eq => {
                        let cmp = llvm!(builder.build_float_compare(
                            FloatPredicate::OEQ,
                            l,
                            r,
                            "fcmp_tmp"
                        ));
                        Ok(
                            llvm!(builder.build_int_z_extend(cmp, context.i64_type(), "fbool_tmp"))
                                .into(),
                        )
                    }
                    _ => Err(MumeiError::codegen(format!(
                        "Unsupported float operator {:?}",
                        op
                    ))),
                }
            } else {
                let l = lhs.into_int_value();
                let r = rhs.into_int_value();
                match op {
                    Op::Add => Ok(llvm!(builder.build_int_add(l, r, "add_tmp")).into()),
                    Op::Sub => Ok(llvm!(builder.build_int_sub(l, r, "sub_tmp")).into()),
                    Op::Mul => Ok(llvm!(builder.build_int_mul(l, r, "mul_tmp")).into()),
                    Op::Div => Ok(llvm!(builder.build_int_signed_div(l, r, "div_tmp")).into()),
                    Op::Eq | Op::Neq | Op::Lt | Op::Gt | Op::Ge | Op::Le => {
                        let pred = match op {
                            Op::Eq => IntPredicate::EQ,
                            Op::Neq => IntPredicate::NE,
                            Op::Lt => IntPredicate::SLT,
                            Op::Gt => IntPredicate::SGT,
                            Op::Ge => IntPredicate::SGE,
                            Op::Le => IntPredicate::SLE,
                            _ => unreachable!(),
                        };
                        let cmp = llvm!(builder.build_int_compare(pred, l, r, "cmp_tmp"));
                        Ok(
                            llvm!(builder.build_int_z_extend(cmp, context.i64_type(), "bool_tmp"))
                                .into(),
                        )
                    }
                    _ => Err(MumeiError::codegen(format!(
                        "Unsupported int operator {:?}",
                        op
                    ))),
                }
            }
        }

        HirExpr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            let cond_val = compile_hir_expr(
                context, builder, module, function, cond, variables, array_ptrs, module_env,
            )?
            .into_int_value();
            let cond_bool = llvm!(builder.build_int_compare(
                IntPredicate::NE,
                cond_val,
                context.i64_type().const_int(0, false),
                "if_cond"
            ));

            let then_block = context.append_basic_block(*function, "then");
            let else_block = context.append_basic_block(*function, "else");
            let merge_block = context.append_basic_block(*function, "merge");

            llvm!(builder.build_conditional_branch(cond_bool, then_block, else_block));

            builder.position_at_end(then_block);
            let then_val = compile_hir_stmt(
                context,
                builder,
                module,
                function,
                then_branch,
                variables,
                array_ptrs,
                module_env,
            )?;
            let then_end_block = builder.get_insert_block().unwrap();
            llvm!(builder.build_unconditional_branch(merge_block));

            builder.position_at_end(else_block);
            let else_val = compile_hir_stmt(
                context,
                builder,
                module,
                function,
                else_branch,
                variables,
                array_ptrs,
                module_env,
            )?;
            let else_end_block = builder.get_insert_block().unwrap();
            llvm!(builder.build_unconditional_branch(merge_block));

            builder.position_at_end(merge_block);
            let phi = llvm!(builder.build_phi(then_val.get_type(), "if_result"));
            phi.add_incoming(&[(&then_val, then_end_block), (&else_val, else_end_block)]);
            Ok(phi.as_basic_value())
        }

        HirExpr::StructInit { type_name, fields } => {
            let mut last_val: BasicValueEnum = context.i64_type().const_int(0, false).into();
            if let Some(sdef) = module_env.get_struct(type_name) {
                for (field_name, field_expr) in fields {
                    let val = compile_hir_expr(
                        context, builder, module, function, field_expr, variables, array_ptrs,
                        module_env,
                    )?;
                    let qualified = format!("__struct_{}_{}", type_name, field_name);
                    variables.insert(qualified, val);
                }
                let field_types: Vec<inkwell::types::BasicTypeEnum> = sdef
                    .fields
                    .iter()
                    .map(|f| {
                        let base = module_env.resolve_base_type(&f.type_name);
                        match base.as_str() {
                            "f64" => context.f64_type().into(),
                            _ => context.i64_type().into(),
                        }
                    })
                    .collect();
                let struct_type = context.struct_type(&field_types.to_vec(), false);
                let mut struct_val = struct_type.get_undef();
                for (i, (field_name, _)) in fields.iter().enumerate() {
                    let qualified = format!("__struct_{}_{}", type_name, field_name);
                    if let Some(val) = variables.get(&qualified) {
                        struct_val = llvm!(builder.build_insert_value(
                            struct_val,
                            *val,
                            i as u32,
                            &format!("struct_{}", field_name)
                        ))
                        .into_struct_value();
                    }
                }
                last_val = struct_val.into();
            } else {
                for (field_name, field_expr) in fields {
                    let val = compile_hir_expr(
                        context, builder, module, function, field_expr, variables, array_ptrs,
                        module_env,
                    )?;
                    let qualified = format!("__struct_{}_{}", type_name, field_name);
                    variables.insert(qualified, val);
                    last_val = val;
                }
            }
            Ok(last_val)
        }

        HirExpr::Match { target, arms } => {
            let target_val = compile_hir_expr(
                context, builder, module, function, target, variables, array_ptrs, module_env,
            )?;

            let merge_block = context.append_basic_block(*function, "match.merge");
            let unreachable_block = context.append_basic_block(*function, "match.unreachable");

            let mut incoming: Vec<(BasicValueEnum<'a>, inkwell::basic_block::BasicBlock<'a>)> =
                Vec::new();

            let arm_count = arms.len();
            let mut try_blocks: Vec<inkwell::basic_block::BasicBlock<'a>> = Vec::new();
            for i in 0..arm_count {
                try_blocks.push(context.append_basic_block(*function, &format!("match.try_{}", i)));
            }

            llvm!(builder.build_unconditional_branch(try_blocks[0]));

            for (i, arm) in arms.iter().enumerate() {
                let try_block = try_blocks[i];
                let fail_block = if i + 1 < arm_count {
                    try_blocks[i + 1]
                } else {
                    unreachable_block
                };

                builder.position_at_end(try_block);

                let pattern_matches = compile_pattern_test(
                    context,
                    builder,
                    &arm.pattern,
                    target_val,
                    variables,
                    module_env,
                )?;

                let full_cond = if let Some(guard) = &arm.guard {
                    let mut guard_vars = variables.clone();
                    bind_pattern_variables(&arm.pattern, target_val, &mut guard_vars);
                    let guard_val = compile_hir_expr(
                        context,
                        builder,
                        module,
                        function,
                        guard,
                        &mut guard_vars,
                        array_ptrs,
                        module_env,
                    )?
                    .into_int_value();
                    let guard_bool = llvm!(builder.build_int_compare(
                        IntPredicate::NE,
                        guard_val,
                        context.i64_type().const_int(0, false),
                        "guard_cond"
                    ));
                    llvm!(builder.build_and(pattern_matches, guard_bool, "match_and_guard"))
                } else {
                    pattern_matches
                };

                let body_block =
                    context.append_basic_block(*function, &format!("match.body_{}", i));
                llvm!(builder.build_conditional_branch(full_cond, body_block, fail_block));

                builder.position_at_end(body_block);
                let mut arm_vars = variables.clone();
                bind_pattern_variables(&arm.pattern, target_val, &mut arm_vars);

                let body_val = compile_hir_stmt(
                    context,
                    builder,
                    module,
                    function,
                    &arm.body,
                    &mut arm_vars,
                    array_ptrs,
                    module_env,
                )?;
                let body_end = builder.get_insert_block().unwrap();
                llvm!(builder.build_unconditional_branch(merge_block));
                incoming.push((body_val, body_end));
            }

            builder.position_at_end(unreachable_block);
            let unreachable_val: BasicValueEnum = context.i64_type().const_int(0, false).into();
            llvm!(builder.build_unconditional_branch(merge_block));
            incoming.push((unreachable_val, unreachable_block));

            builder.position_at_end(merge_block);
            let phi = llvm!(builder.build_phi(context.i64_type(), "match_result"));
            for (val, block) in &incoming {
                phi.add_incoming(&[(val, *block)]);
            }

            Ok(phi.as_basic_value())
        }

        // =================================================================
        // 非同期処理の LLVM IR 生成
        // =================================================================
        HirExpr::Async { body } => compile_hir_stmt(
            context, builder, module, function, body, variables, array_ptrs, module_env,
        ),
        HirExpr::Await { expr: await_expr } => compile_hir_expr(
            context, builder, module, function, await_expr, variables, array_ptrs, module_env,
        ),

        HirExpr::Task { body, .. } => compile_hir_stmt(
            context, builder, module, function, body, variables, array_ptrs, module_env,
        ),
        HirExpr::TaskGroup { children, .. } => {
            let mut last_val: BasicValueEnum = context.i64_type().const_int(0, false).into();
            for child in children {
                last_val = compile_hir_stmt(
                    context, builder, module, function, child, variables, array_ptrs, module_env,
                )?;
            }
            Ok(last_val)
        }

        HirExpr::AtomRef { name } => {
            let func = if let Some(f) = module.get_function(name) {
                f
            } else if let Some(callee_atom) = module_env.get_atom(name) {
                let callee_param_types: Vec<inkwell::types::BasicMetadataTypeEnum> = callee_atom
                    .params
                    .iter()
                    .map(|p| resolve_param_type(context, p.type_name.as_deref(), module_env).into())
                    .collect();
                let fn_type = context.i64_type().fn_type(&callee_param_types, false);
                module.add_function(name, fn_type, Some(inkwell::module::Linkage::External))
            } else {
                return Err(MumeiError::codegen(format!(
                    "atom_ref: unknown function '{}'",
                    name
                )));
            };
            let fn_ptr = func.as_global_value().as_pointer_value();
            let ptr_int = llvm!(builder.build_ptr_to_int(
                fn_ptr,
                context.i64_type(),
                &format!("atom_ref_{}", name)
            ));
            Ok(ptr_int.into())
        }
        HirExpr::CallRef { callee, args } => {
            let callee_val = compile_hir_expr(
                context, builder, module, function, callee, variables, array_ptrs, module_env,
            )?;

            let mut arg_vals: Vec<BasicValueEnum> = Vec::new();
            for arg in args {
                let val = compile_hir_expr(
                    context, builder, module, function, arg, variables, array_ptrs, module_env,
                )?;
                arg_vals.push(val);
            }

            if let HirExpr::AtomRef { name } = callee.as_ref() {
                if let Some(callee_fn) = module.get_function(name) {
                    let args_meta: Vec<BasicMetadataValueEnum> =
                        arg_vals.iter().map(|v| (*v).into()).collect();
                    let call_result = llvm!(builder.build_call(
                        callee_fn,
                        &args_meta,
                        &format!("call_ref_{}", name)
                    ));
                    return Ok(call_result
                        .try_as_basic_value()
                        .left()
                        .unwrap_or(context.i64_type().const_int(0, false).into()));
                }
            }

            let callee_int = callee_val.into_int_value();
            let param_types: Vec<BasicMetadataTypeEnum> = arg_vals
                .iter()
                .map(|v| {
                    if v.is_float_value() {
                        context.f64_type().into()
                    } else {
                        context.i64_type().into()
                    }
                })
                .collect();
            let has_float_arg = arg_vals.iter().any(|v| v.is_float_value());
            let fn_type = if has_float_arg {
                context.f64_type().fn_type(&param_types, false)
            } else {
                context.i64_type().fn_type(&param_types, false)
            };
            let fn_ptr = llvm!(builder.build_int_to_ptr(
                callee_int,
                context.ptr_type(inkwell::AddressSpace::default()),
                "call_ref_fn_ptr"
            ));
            let args_meta: Vec<BasicMetadataValueEnum> =
                arg_vals.iter().map(|v| (*v).into()).collect();
            let call_result = llvm!(builder.build_indirect_call(
                fn_type,
                fn_ptr,
                &args_meta,
                "call_ref_indirect"
            ));
            Ok(call_result
                .try_as_basic_value()
                .left()
                .unwrap_or(context.i64_type().const_int(0, false).into()))
        }

        HirExpr::Perform {
            effect,
            operation,
            args: perform_args,
        } => {
            let fn_name = format!("__effect_{}_{}", effect, operation);

            let mut arg_vals = Vec::new();
            for arg in perform_args {
                let val = compile_hir_expr(
                    context, builder, module, function, arg, variables, array_ptrs, module_env,
                )?;
                arg_vals.push(val);
            }

            let param_types: Vec<BasicMetadataTypeEnum> = arg_vals
                .iter()
                .map(|v| {
                    if v.is_float_value() {
                        context.f64_type().into()
                    } else {
                        context.i64_type().into()
                    }
                })
                .collect();
            let fn_type = context.i64_type().fn_type(&param_types, false);
            let callee_fn = module.get_function(&fn_name).unwrap_or_else(|| {
                module.add_function(&fn_name, fn_type, Some(inkwell::module::Linkage::External))
            });

            let args_meta: Vec<BasicMetadataValueEnum> =
                arg_vals.iter().map(|v| (*v).into()).collect();
            let call_result = llvm!(builder.build_call(callee_fn, &args_meta, "perform_result"));
            Ok(call_result
                .try_as_basic_value()
                .left()
                .unwrap_or(context.i64_type().const_int(0, false).into()))
        }

        HirExpr::Lambda {
            params, captures, ..
        } => {
            // LLVM IR: Lambda as closure struct (function pointer + captured env)
            // For now, represent as an i64 tag (closure ID) — full closure conversion
            // will be added in a future phase with __Closure_N struct generation.
            let _param_count = params.len();
            let _capture_count = captures.len();
            // Return a symbolic closure ID
            Ok(context.i64_type().const_int(0, false).into())
        }

        HirExpr::FieldAccess(inner_expr, field_name) => {
            if let HirExpr::Variable(var_name) = inner_expr.as_ref() {
                let candidates = [
                    format!("__struct_{}_{}", var_name, field_name),
                    format!("{}_{}", var_name, field_name),
                ];
                for candidate in &candidates {
                    if let Some(val) = variables.get(candidate) {
                        return Ok(*val);
                    }
                }
                if let Some(struct_val) = variables.get(var_name.as_str()) {
                    if struct_val.is_struct_value() {
                        let sv = struct_val.into_struct_value();
                        if let Some(idx) = find_field_index(var_name, field_name, module_env) {
                            let extracted = llvm!(builder.build_extract_value(
                                sv,
                                idx,
                                &format!("{}.{}", var_name, field_name)
                            ));
                            return Ok(extracted);
                        }
                    }
                }
                Err(MumeiError::codegen(format!(
                    "Field '{}' not found on '{}'",
                    field_name, var_name
                )))
            } else {
                let base_val = compile_hir_expr(
                    context, builder, module, function, inner_expr, variables, array_ptrs,
                    module_env,
                )?;
                if base_val.is_struct_value() {
                    let sv = base_val.into_struct_value();
                    if let Some(idx) = find_field_index_by_name(field_name, module_env) {
                        let extracted = llvm!(builder.build_extract_value(
                            sv,
                            idx,
                            &format!("nested.{}", field_name)
                        ));
                        Ok(extracted)
                    } else {
                        let extracted = llvm!(builder.build_extract_value(
                            sv,
                            0,
                            &format!("field_{}", field_name)
                        ));
                        Ok(extracted)
                    }
                } else {
                    Err(MumeiError::codegen(format!(
                        "Cannot access field '{}' on non-struct value",
                        field_name
                    )))
                }
            }
        }
    }
}

// =============================================================================
// Pattern Matrix: パターン条件生成 + 変数バインド
// =============================================================================

/// パターンから LLVM の条件（i1 bool）を再帰的に生成する。
/// ネストパターン `Variant(Literal(42), x)` のような場合、
/// 各サブパターンの条件を AND 結合する。
///
/// - Wildcard / Variable → true (const 1)
/// - Literal(n) → target == n
/// - Variant { name, fields } → (target == tag) ∧ (fields の再帰条件)
fn compile_pattern_test<'a>(
    context: &'a Context,
    builder: &Builder<'a>,
    pattern: &Pattern,
    target: BasicValueEnum<'a>,
    _variables: &HashMap<String, BasicValueEnum<'a>>,
    module_env: &ModuleEnv,
) -> MumeiResult<inkwell::values::IntValue<'a>> {
    match pattern {
        Pattern::Wildcard | Pattern::Variable(_) => {
            // 常にマッチ
            Ok(context.bool_type().const_int(1, false))
        }
        Pattern::Literal(n) => {
            let target_int = target.into_int_value();
            let lit = context.i64_type().const_int(*n as u64, true);
            let cmp =
                llvm!(builder.build_int_compare(IntPredicate::EQ, target_int, lit, "pat_lit_eq"));
            Ok(cmp)
        }
        Pattern::Variant {
            variant_name,
            fields,
        } => {
            // Enum variant: tag 値で判定
            let target_int = target.into_int_value();
            let tag_val = if let Some(enum_def) = module_env.find_enum_by_variant(variant_name) {
                enum_def
                    .variants
                    .iter()
                    .position(|v| v.name == *variant_name)
                    .unwrap_or(0) as u64
            } else {
                // Enum 定義が見つからない場合はハッシュベースのフォールバック
                variant_name
                    .bytes()
                    .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64))
            };
            let tag_const = context.i64_type().const_int(tag_val, false);
            let tag_match = llvm!(builder.build_int_compare(
                IntPredicate::EQ,
                target_int,
                tag_const,
                "pat_tag_eq"
            ));

            // ネストパターンの再帰処理: 各フィールドの条件を AND 結合
            // 現在は tag のみで payload を持たないため、フィールドパターンが
            // Wildcard/Variable 以外の場合のみ再帰（将来の payload 対応の準備）
            let mut result = tag_match;
            for field_pat in fields.iter() {
                match field_pat {
                    Pattern::Wildcard | Pattern::Variable(_) => {
                        // 常にマッチ → AND しても変わらない
                    }
                    _ => {
                        // 将来: payload からフィールド値を取得して再帰
                        // 現在は payload がないため、ダミー値 0 で再帰テスト
                        let dummy_field: BasicValueEnum =
                            context.i64_type().const_int(0, false).into();
                        let field_test = compile_pattern_test(
                            context,
                            builder,
                            field_pat,
                            dummy_field,
                            _variables,
                            module_env,
                        )?;
                        result = llvm!(builder.build_and(result, field_test, "pat_nested_and"));
                    }
                }
            }
            Ok(result)
        }
    }
}

/// パターンから変数バインドを variables に登録する（再帰的）。
/// - Variable(name) → target の値を name にバインド
/// - Variant の fields 内の Variable → 将来は payload から取得、現在はダミー
fn bind_pattern_variables<'a>(
    pattern: &Pattern,
    target: BasicValueEnum<'a>,
    variables: &mut HashMap<String, BasicValueEnum<'a>>,
) {
    match pattern {
        Pattern::Variable(name) => {
            variables.insert(name.clone(), target);
        }
        Pattern::Variant {
            variant_name: _,
            fields,
        } => {
            for field_pat in fields.iter() {
                match field_pat {
                    Pattern::Variable(fname) => {
                        // 将来: payload からフィールド値を GEP + load で取得
                        // 現在は tag のみなのでダミー値
                        let dummy: BasicValueEnum =
                            target.get_type().into_int_type().const_int(0, false).into();
                        variables.insert(fname.clone(), dummy);
                    }
                    Pattern::Variant { .. } => {
                        // ネストした Variant: 再帰的にバインド（将来の payload 対応）
                        let dummy: BasicValueEnum =
                            target.get_type().into_int_type().const_int(0, false).into();
                        bind_pattern_variables(field_pat, dummy, variables);
                    }
                    _ => {}
                }
            }
        }
        Pattern::Wildcard | Pattern::Literal(_) => {
            // バインドなし
        }
    }
}

/// フィールド名のみから全構造体定義を走査してインデックスを検索（ネスト構造体用）
fn find_field_index_by_name(field_name: &str, module_env: &ModuleEnv) -> Option<u32> {
    for sdef in module_env.structs.values() {
        if let Some(pos) = sdef.fields.iter().position(|f| f.name == field_name) {
            return Some(pos as u32);
        }
    }
    None
}

/// 構造体定義からフィールド名のインデックスを検索
fn find_field_index(
    type_or_var_name: &str,
    field_name: &str,
    module_env: &ModuleEnv,
) -> Option<u32> {
    // ModuleEnv に登録された構造体を探索
    // var_name が構造体型名と一致する場合、または型名を推定
    if let Some(sdef) = module_env.get_struct(type_or_var_name) {
        return sdef
            .fields
            .iter()
            .position(|f| f.name == field_name)
            .map(|i| i as u32);
    }
    // フォールバック: 全構造体定義を走査してフィールド名が一致するものを探す
    for sdef in module_env.structs.values() {
        if let Some(pos) = sdef.fields.iter().position(|f| f.name == field_name) {
            return Some(pos as u32);
        }
    }
    None
}
