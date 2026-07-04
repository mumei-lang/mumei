use crate::codegen::stmt_emit::compile_hir_stmt;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::values::{BasicValueEnum, FunctionValue};
use inkwell::AddressSpace;
use inkwell::IntPredicate;
use mumei_core::hir::{collect_free_variables_stmt, HirStmt};
use mumei_core::verification::{ModuleEnv, MumeiError, MumeiResult};
use std::collections::HashMap;

fn next_task_counter(module: &Module<'_>, atom_name: &str) -> u32 {
    let prefix = format!("__mumei_task_{}_", atom_name);
    let mut max_seen: i32 = -1;
    let mut f = module.get_first_function();
    while let Some(func) = f {
        if let Ok(name) = func.get_name().to_str() {
            if let Some(rest) = name.strip_prefix(&prefix) {
                if let Ok(n) = rest.parse::<i32>() {
                    if n > max_seen {
                        max_seen = n;
                    }
                }
            }
        }
        f = func.get_next_function();
    }
    (max_seen + 1) as u32
}

pub(crate) fn static_next_task_group_id() -> Option<u64> {
    static TASK_GROUP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let next = TASK_GROUP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    if next == u64::MAX {
        None
    } else {
        Some(next)
    }
}

/// Plan 21 — concurrency runtime: declare `pthread_create` /
/// `pthread_join` once per module and return their `FunctionValue`s.
/// Both have the standard glibc signatures (using `i8*` / `void*` for
/// opaque thread / arg / retval slots so we don't need `pthread_t`'s
/// platform-specific layout in IR).
fn declare_pthread_externs<'a>(
    context: &'a Context,
    module: &Module<'a>,
) -> (FunctionValue<'a>, FunctionValue<'a>) {
    let i32_type = context.i32_type();
    let ptr_type = context.ptr_type(AddressSpace::default());

    let create_fn = module.get_function("pthread_create").unwrap_or_else(|| {
        // int pthread_create(pthread_t *thread, const pthread_attr_t *attr,
        //                    void *(*start_routine)(void *), void *arg);
        let fn_type = i32_type.fn_type(
            &[
                ptr_type.into(),
                ptr_type.into(),
                ptr_type.into(),
                ptr_type.into(),
            ],
            false,
        );
        module.add_function(
            "pthread_create",
            fn_type,
            Some(inkwell::module::Linkage::External),
        )
    });
    let join_fn = module.get_function("pthread_join").unwrap_or_else(|| {
        // int pthread_join(pthread_t thread, void **retval);
        // pthread_t is `unsigned long` on glibc — model as i64.
        let fn_type = i32_type.fn_type(&[context.i64_type().into(), ptr_type.into()], false);
        module.add_function(
            "pthread_join",
            fn_type,
            Some(inkwell::module::Linkage::External),
        )
    });
    (create_fn, join_fn)
}

pub(crate) fn declare_task_group_any_externs<'a>(
    context: &'a Context,
    module: &Module<'a>,
) -> (
    FunctionValue<'a>,
    FunctionValue<'a>,
    FunctionValue<'a>,
    FunctionValue<'a>,
    FunctionValue<'a>,
    FunctionValue<'a>,
    FunctionValue<'a>,
    FunctionValue<'a>,
) {
    let i64_type = context.i64_type();
    let ptr_type = context.ptr_type(AddressSpace::default());
    let void_type = context.void_type();

    let complete_fn = module
        .get_function("__mumei_task_group_complete")
        .unwrap_or_else(|| {
            let fn_type =
                i64_type.fn_type(&[i64_type.into(), i64_type.into(), ptr_type.into()], false);
            module.add_function(
                "__mumei_task_group_complete",
                fn_type,
                Some(inkwell::module::Linkage::External),
            )
        });
    let flag_fn = module
        .get_function("__mumei_task_group_any_flag")
        .unwrap_or_else(|| {
            let fn_type = i64_type.fn_type(&[i64_type.into()], false);
            module.add_function(
                "__mumei_task_group_any_flag",
                fn_type,
                Some(inkwell::module::Linkage::External),
            )
        });
    let reset_fn = module
        .get_function("__mumei_task_group_reset")
        .unwrap_or_else(|| {
            let fn_type = void_type.fn_type(&[i64_type.into()], false);
            module.add_function(
                "__mumei_task_group_reset",
                fn_type,
                Some(inkwell::module::Linkage::External),
            )
        });
    let cancel_fn = module
        .get_function("__mumei_task_cancel")
        .unwrap_or_else(|| {
            let fn_type = void_type.fn_type(&[i64_type.into()], false);
            module.add_function(
                "__mumei_task_cancel",
                fn_type,
                Some(inkwell::module::Linkage::External),
            )
        });
    let wait_fn = module
        .get_function("__mumei_task_group_any_wait")
        .unwrap_or_else(|| {
            let fn_type = void_type.fn_type(&[i64_type.into()], false);
            module.add_function(
                "__mumei_task_group_any_wait",
                fn_type,
                Some(inkwell::module::Linkage::External),
            )
        });
    let group_cancel_fn = module
        .get_function("__mumei_task_group_cancel")
        .unwrap_or_else(|| {
            let fn_type = void_type.fn_type(&[i64_type.into()], false);
            module.add_function(
                "__mumei_task_group_cancel",
                fn_type,
                Some(inkwell::module::Linkage::External),
            )
        });
    let group_enter_fn = module
        .get_function("__mumei_task_group_enter")
        .unwrap_or_else(|| {
            let fn_type = void_type.fn_type(&[i64_type.into()], false);
            module.add_function(
                "__mumei_task_group_enter",
                fn_type,
                Some(inkwell::module::Linkage::External),
            )
        });
    let group_leave_fn = module
        .get_function("__mumei_task_group_leave")
        .unwrap_or_else(|| {
            let fn_type = void_type.fn_type(&[], false);
            module.add_function(
                "__mumei_task_group_leave",
                fn_type,
                Some(inkwell::module::Linkage::External),
            )
        });

    (
        complete_fn,
        flag_fn,
        reset_fn,
        cancel_fn,
        wait_fn,
        group_cancel_fn,
        group_enter_fn,
        group_leave_fn,
    )
}

fn declare_task_group_should_cancel_extern<'a>(
    context: &'a Context,
    module: &Module<'a>,
) -> FunctionValue<'a> {
    let i64_type = context.i64_type();
    module
        .get_function("__mumei_task_group_should_cancel")
        .unwrap_or_else(|| {
            let fn_type = i64_type.fn_type(&[i64_type.into()], false);
            module.add_function(
                "__mumei_task_group_should_cancel",
                fn_type,
                Some(inkwell::module::Linkage::External),
            )
        })
}

pub(crate) fn declare_task_group_should_cancel_current_extern<'a>(
    context: &'a Context,
    module: &Module<'a>,
) -> FunctionValue<'a> {
    let i64_type = context.i64_type();
    module
        .get_function("__mumei_task_group_should_cancel_current")
        .unwrap_or_else(|| {
            let fn_type = i64_type.fn_type(&[], false);
            module.add_function(
                "__mumei_task_group_should_cancel_current",
                fn_type,
                Some(inkwell::module::Linkage::External),
            )
        })
}

/// Plan 21 — concurrency runtime: handle for a `task` that has been
/// spawned but not yet joined. `emit_task_spawn_only` returns one of
/// these; `emit_task_join_only` consumes it. Splitting spawn from join
/// is what makes `task_group:all` actually concurrent — see the
/// `HirExpr::TaskGroup` arm in `compile_hir_expr` for the full
/// spawn-all-then-join-all sequence.
pub(crate) struct PendingTask<'a> {
    args_struct_type: inkwell::types::StructType<'a>,
    args_ptr: inkwell::values::PointerValue<'a>,
    thread_ptr: inkwell::values::PointerValue<'a>,
    /// Field index of the trailing `i64 result` slot inside `args_struct_type`.
    result_idx: u32,
}

#[derive(Clone, Copy)]
pub(crate) struct TaskGroupAnyContext<'a> {
    pub(crate) group_id: u64,
    pub(crate) result_ptr: inkwell::values::PointerValue<'a>,
}

/// Plan 21 — concurrency runtime: emit the `__mumei_task_<atom>_<N>`
/// wrapper function for `body` and the parent-side `pthread_create`
/// call, returning a `PendingTask` handle that
/// `emit_task_join_only` can later turn into the joined i64 result.
///
/// The wrapper has signature `i8* (i8*)`. Its `arg` is a
/// stack-allocated args struct populated by the parent before
/// `pthread_create`; the struct layout is
/// `{ <captured i64s…>, [i64 group_id, i64* group_result], i64 result }`.
///
/// Captures are the intersection of the body's free variables and the
/// parent's currently-live i64 variables. Free variables that exist in
/// the parent's `variables` map but are *not* i64 (f64 / struct /
/// pointer / Str / array fat-pointers) are silently skipped today —
/// LLVM-IR-level closure conversion for those types is out-of-scope
/// for Plan 21 and tracked as a follow-up. We emit a `eprintln!`
/// diagnostic naming each dropped capture so users notice immediately
/// rather than only when their task body silently reads a zero. A
/// proper user-facing diagnostic (via `MumeiError`) is the right
/// long-term home for this warning, but at the time of writing the
/// codegen pass has no way to thread non-fatal diagnostics back to the
/// driver, so a stderr line is the best we can do without a larger
/// refactor.
///
/// Allocations (args struct, `pthread_t` slot) live in the parent's
/// *entry* block so subsequent basic blocks dominate them — this also
/// means a `task` inside an `if`/`while` doesn't hide the alloca from
/// later joins.
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_task_spawn_only<'a>(
    context: &'a Context,
    builder: &Builder<'a>,
    module: &Module<'a>,
    parent_function: &FunctionValue<'a>,
    body: &HirStmt,
    variables: &HashMap<String, BasicValueEnum<'a>>,
    var_types: &HashMap<String, String>,
    module_env: &ModuleEnv,
    task_group_any: Option<TaskGroupAnyContext<'a>>,
) -> MumeiResult<PendingTask<'a>> {
    let i64_type = context.i64_type();
    let ptr_type = context.ptr_type(AddressSpace::default());

    // 1. Determine captures: i64 free vars from `body` that are bound in `variables`.
    let free_vars = collect_free_variables_stmt(body);
    let mut captures: Vec<(String, BasicValueEnum<'a>)> = Vec::new();
    let mut dropped: Vec<String> = Vec::new();
    for name in &free_vars {
        if let Some(v) = variables.get(name.as_str()) {
            if v.is_int_value() && v.into_int_value().get_type() == i64_type {
                captures.push((name.clone(), *v));
            } else {
                dropped.push(name.clone());
            }
        }
    }
    captures.sort_by(|a, b| a.0.cmp(&b.0));
    if !dropped.is_empty() {
        dropped.sort();
        let parent_label = parent_function.get_name().to_str().unwrap_or("anon");
        eprintln!(
            "[mumei codegen] warning: in atom '{}', task body references non-i64 \
             free variable(s) {:?} which were silently dropped from the pthread \
             closure. The wrapper sees them as zero. (Plan 21 only marshals i64 \
             captures; floats/structs/pointers/Str/arrays are tracked as a \
             follow-up.)",
            parent_label, dropped
        );
    }

    // 2. Build args struct type: { i64 capture_0, …, [i64 group, i64* group_result], i64 result }.
    let mut field_types = captures
        .iter()
        .map(|_| i64_type.into())
        .collect::<Vec<inkwell::types::BasicTypeEnum>>();
    let group_fields = if task_group_any.is_some() {
        let group_id_idx = field_types.len() as u32;
        field_types.push(i64_type.into());
        let group_result_idx = field_types.len() as u32;
        field_types.push(ptr_type.into());
        Some((group_id_idx, group_result_idx))
    } else {
        None
    };
    let result_idx = field_types.len() as u32;
    field_types.push(i64_type.into());
    let args_struct_type = context.struct_type(&field_types, false);

    let parent_name = parent_function
        .get_name()
        .to_str()
        .unwrap_or("anon")
        .to_string();
    let counter = next_task_counter(module, &parent_name);
    let wrapper_name = format!("__mumei_task_{}_{}", parent_name, counter);

    // 3. Generate wrapper function: i8* (i8*).
    let wrapper_fn_type = ptr_type.fn_type(&[ptr_type.into()], false);
    let wrapper_fn = module.add_function(&wrapper_name, wrapper_fn_type, None);
    let wrapper_entry = context.append_basic_block(wrapper_fn, "entry");

    // Save parent insertion point so we can restore once wrapper is emitted.
    let parent_insert_block = builder.get_insert_block();

    builder.position_at_end(wrapper_entry);
    let arg_ptr = wrapper_fn.get_nth_param(0).unwrap().into_pointer_value();

    // Build inner variables map by loading each capture from args struct.
    let mut inner_vars: HashMap<String, BasicValueEnum> = HashMap::new();
    let mut inner_var_types: HashMap<String, String> = var_types.clone();
    for (i, (name, _val)) in captures.iter().enumerate() {
        let field_ptr = llvm!(builder.build_struct_gep(
            args_struct_type,
            arg_ptr,
            i as u32,
            &format!("task_capture_{}_ptr", name),
        ));
        let loaded =
            llvm!(builder.build_load(i64_type, field_ptr, &format!("task_capture_{}", name),));
        inner_vars.insert(name.clone(), loaded);
    }

    let task_group_runtime = if let Some((group_id_idx, group_result_idx)) = group_fields {
        let group_id_ptr = llvm!(builder.build_struct_gep(
            args_struct_type,
            arg_ptr,
            group_id_idx,
            "task_group_id_ptr",
        ));
        let group_id =
            llvm!(builder.build_load(i64_type, group_id_ptr, "task_group_id")).into_int_value();
        let group_result_ptr_ptr = llvm!(builder.build_struct_gep(
            args_struct_type,
            arg_ptr,
            group_result_idx,
            "task_group_result_ptr_ptr",
        ));
        let group_result_ptr =
            llvm!(builder.build_load(ptr_type, group_result_ptr_ptr, "task_group_result_ptr"))
                .into_pointer_value();
        let (_complete_fn, flag_fn, _reset_fn, _cancel_fn, _wait_fn, _, enter_fn, leave_fn) =
            declare_task_group_any_externs(context, module);
        llvm!(builder.build_call(enter_fn, &[group_id.into()], "task_group_enter_call",));
        let should_cancel_fn = declare_task_group_should_cancel_extern(context, module);
        let done_flag =
            llvm!(builder.build_call(flag_fn, &[group_id.into()], "task_group_start_done_check",))
                .try_as_basic_value()
                .left()
                .ok_or_else(|| {
                    MumeiError::codegen("task_group:any flag returned void".to_string())
                })?
                .into_int_value();
        let is_done = llvm!(builder.build_int_compare(
            IntPredicate::NE,
            done_flag,
            i64_type.const_int(0, false),
            "task_group_start_done",
        ));
        let cancel_flag = llvm!(builder.build_call(
            should_cancel_fn,
            &[group_id.into()],
            "task_group_start_cancel_check",
        ))
        .try_as_basic_value()
        .left()
        .ok_or_else(|| MumeiError::codegen("cancel check returned void".to_string()))?
        .into_int_value();
        let is_cancelled = llvm!(builder.build_int_compare(
            IntPredicate::NE,
            cancel_flag,
            i64_type.const_int(0, false),
            "task_group_start_cancelled",
        ));
        let should_exit = llvm!(builder.build_or(is_done, is_cancelled, "task_group_start_exit"));
        let cancelled_block = context.append_basic_block(wrapper_fn, "task_group_cancelled_entry");
        let body_block = context.append_basic_block(wrapper_fn, "task_group_body_entry");
        llvm!(builder.build_conditional_branch(should_exit, cancelled_block, body_block));

        builder.position_at_end(cancelled_block);
        llvm!(builder.build_call(leave_fn, &[], "task_group_leave_cancelled_call"));
        let null_ret = ptr_type.const_null();
        llvm!(builder.build_return(Some(&null_ret)));

        builder.position_at_end(body_block);
        Some((group_id, group_result_ptr))
    } else {
        None
    };

    // Compile the task body inside the wrapper. Note: `array_ptrs` is empty —
    // arrays in task bodies are not yet captured (follow-up work).
    let empty_array_ptrs: HashMap<String, (BasicValueEnum<'_>, BasicValueEnum<'_>)> =
        HashMap::new();
    let body_result = compile_hir_stmt(
        context,
        builder,
        module,
        &wrapper_fn,
        body,
        &mut inner_vars,
        &mut inner_var_types,
        &empty_array_ptrs,
        module_env,
    )?;

    // Coerce body result to i64 (most task bodies already produce i64;
    // f64/struct/pointer results are not yet plumbed through join).
    let body_i64 = if body_result.is_int_value() {
        body_result.into_int_value()
    } else {
        i64_type.const_int(0, false)
    };

    // Store result into the trailing slot of the args struct.
    let result_ptr =
        llvm!(builder.build_struct_gep(args_struct_type, arg_ptr, result_idx, "task_result_ptr",));
    llvm!(builder.build_store(result_ptr, body_i64));

    if let Some((group_id, group_result_ptr)) = task_group_runtime {
        let (complete_fn, _flag_fn, _reset_fn, _cancel_fn, _wait_fn, group_cancel_fn, _, leave_fn) =
            declare_task_group_any_externs(context, module);
        let completed = llvm!(builder.build_call(
            complete_fn,
            &[group_id.into(), body_i64.into(), group_result_ptr.into()],
            "task_group_complete_call",
        ))
        .try_as_basic_value()
        .left()
        .ok_or_else(|| MumeiError::codegen("task_group complete returned void".to_string()))?
        .into_int_value();
        let won_group = llvm!(builder.build_int_compare(
            IntPredicate::NE,
            completed,
            i64_type.const_int(0, false),
            "task_group_complete_won",
        ));
        let cancel_block = context.append_basic_block(wrapper_fn, "task_group_complete_cancel");
        let leave_block = context.append_basic_block(wrapper_fn, "task_group_complete_leave");
        llvm!(builder.build_conditional_branch(won_group, cancel_block, leave_block));

        builder.position_at_end(cancel_block);
        llvm!(builder.build_call(
            group_cancel_fn,
            &[group_id.into()],
            "task_group_winner_cancel_call",
        ));
        llvm!(builder.build_unconditional_branch(leave_block));

        builder.position_at_end(leave_block);
        llvm!(builder.build_call(leave_fn, &[], "task_group_leave_call"));
    }

    let null_ret = ptr_type.const_null();
    llvm!(builder.build_return(Some(&null_ret)));

    // 4. Restore builder to parent's insertion point.
    if let Some(block) = parent_insert_block {
        builder.position_at_end(block);
    }

    // 5. Allocate args struct + thread handle in the parent's *entry* block
    //    so they dominate any later use, then store captures and call
    //    `pthread_create`. Joining happens in `emit_task_join_only`.
    let parent_entry = parent_function
        .get_first_basic_block()
        .ok_or_else(|| MumeiError::codegen(String::from("parent function has no entry block")))?;
    let (args_ptr, thread_ptr) = {
        let saved = builder.get_insert_block();
        if let Some(first_inst) = parent_entry.get_first_instruction() {
            builder.position_before(&first_inst);
        } else {
            builder.position_at_end(parent_entry);
        }
        let alloca = llvm!(builder.build_alloca(args_struct_type, "task_args"));
        let thread_alloca = llvm!(builder.build_alloca(i64_type, "task_thread"));
        if let Some(block) = saved {
            builder.position_at_end(block);
        }
        (alloca, thread_alloca)
    };

    // Store captures.
    for (i, (name, val)) in captures.iter().enumerate() {
        let field_ptr = llvm!(builder.build_struct_gep(
            args_struct_type,
            args_ptr,
            i as u32,
            &format!("task_arg_{}_ptr", name),
        ));
        llvm!(builder.build_store(field_ptr, val.into_int_value()));
    }
    if let (Some(any_ctx), Some((group_id_idx, group_result_idx))) = (task_group_any, group_fields)
    {
        let group_id_ptr = llvm!(builder.build_struct_gep(
            args_struct_type,
            args_ptr,
            group_id_idx,
            "task_arg_group_id_ptr",
        ));
        llvm!(builder.build_store(group_id_ptr, i64_type.const_int(any_ctx.group_id, false)));
        let group_result_ptr_ptr = llvm!(builder.build_struct_gep(
            args_struct_type,
            args_ptr,
            group_result_idx,
            "task_arg_group_result_ptr_ptr",
        ));
        llvm!(builder.build_store(group_result_ptr_ptr, any_ctx.result_ptr));
    }
    // Initialize result slot to 0 so an early failure returns a defined value.
    let result_ptr_parent = llvm!(builder.build_struct_gep(
        args_struct_type,
        args_ptr,
        result_idx,
        "task_result_init_ptr",
    ));
    llvm!(builder.build_store(result_ptr_parent, i64_type.const_int(0, false)));

    let (create_fn, _join_fn) = declare_pthread_externs(context, module);
    let null_attr = ptr_type.const_null();
    let wrapper_ptr = wrapper_fn.as_global_value().as_pointer_value();
    llvm!(builder.build_call(
        create_fn,
        &[
            thread_ptr.into(),
            null_attr.into(),
            wrapper_ptr.into(),
            args_ptr.into(),
        ],
        "pthread_create_call",
    ));

    Ok(PendingTask {
        args_struct_type,
        args_ptr,
        thread_ptr,
        result_idx,
    })
}
pub(crate) fn emit_task_join_only<'a>(
    context: &'a Context,
    builder: &Builder<'a>,
    module: &Module<'a>,
    pending: &PendingTask<'a>,
) -> MumeiResult<BasicValueEnum<'a>> {
    let i64_type = context.i64_type();
    let ptr_type = context.ptr_type(AddressSpace::default());
    let (_create_fn, join_fn) = declare_pthread_externs(context, module);
    let thread_loaded = llvm!(builder.build_load(i64_type, pending.thread_ptr, "task_thread_val"));
    llvm!(builder.build_call(
        join_fn,
        &[thread_loaded.into(), ptr_type.const_null().into()],
        "pthread_join_call",
    ));
    let final_result_ptr = llvm!(builder.build_struct_gep(
        pending.args_struct_type,
        pending.args_ptr,
        pending.result_idx,
        "task_result_load_ptr",
    ));
    let result = llvm!(builder.build_load(i64_type, final_result_ptr, "task_result"));
    Ok(result)
}

/// Plan 21 — concurrency runtime: spawn + immediately join a single
/// `task { … }` body. Used by the `HirExpr::Task` arm. `task_group`
/// callers spawn each child via `emit_task_spawn_only` first and then
/// join them via `emit_task_join_only`, so children actually execute
/// in parallel.
#[allow(clippy::too_many_arguments)]
pub(crate) fn compile_task_spawn<'a>(
    context: &'a Context,
    builder: &Builder<'a>,
    module: &Module<'a>,
    parent_function: &FunctionValue<'a>,
    body: &HirStmt,
    variables: &HashMap<String, BasicValueEnum<'a>>,
    var_types: &HashMap<String, String>,
    module_env: &ModuleEnv,
) -> MumeiResult<BasicValueEnum<'a>> {
    let pending = emit_task_spawn_only(
        context,
        builder,
        module,
        parent_function,
        body,
        variables,
        var_types,
        module_env,
        None,
    )?;
    emit_task_join_only(context, builder, module, &pending)
}
