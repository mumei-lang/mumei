//! P7-B: Binary compilation pipeline.
//!
//! Compiles multiple HirAtoms into a single LLVM Module with a C-compatible
//! main wrapper, then writes the merged .ll file for linking.

use inkwell::context::Context;
use mumei_core::hir::{HirAtom, HirExpr, HirStmt};
use mumei_core::parser::ExternBlock;
use mumei_core::verification::{ModuleEnv, MumeiError, MumeiResult};
use std::path::Path;

use crate::codegen::compile_atom_into_module;

/// Recursively rename function calls in a HirExpr from `from` to `to`.
fn rename_calls_in_hir_expr(expr: &mut HirExpr, from: &str, to: &str) {
    match expr {
        HirExpr::Call {
            ref mut name,
            ref mut args,
            ..
        } => {
            if name == from {
                *name = to.to_string();
            }
            for arg in args.iter_mut() {
                rename_calls_in_hir_expr(arg, from, to);
            }
        }
        HirExpr::BinaryOp(ref mut lhs, _, ref mut rhs) => {
            rename_calls_in_hir_expr(lhs, from, to);
            rename_calls_in_hir_expr(rhs, from, to);
        }
        HirExpr::IfThenElse {
            ref mut cond,
            ref mut then_branch,
            ref mut else_branch,
        } => {
            rename_calls_in_hir_expr(cond, from, to);
            rename_calls_in_hir_stmt(then_branch, from, to);
            rename_calls_in_hir_stmt(else_branch, from, to);
        }
        HirExpr::Match {
            ref mut target,
            ref mut arms,
        } => {
            rename_calls_in_hir_expr(target, from, to);
            for arm in arms.iter_mut() {
                rename_calls_in_hir_stmt(&mut arm.body, from, to);
                if let Some(ref mut guard) = arm.guard {
                    rename_calls_in_hir_expr(guard, from, to);
                }
            }
        }
        HirExpr::StructInit { ref mut fields, .. } => {
            for (_, ref mut val) in fields.iter_mut() {
                rename_calls_in_hir_expr(val, from, to);
            }
        }
        HirExpr::FieldAccess(ref mut inner, _) => {
            rename_calls_in_hir_expr(inner, from, to);
        }
        HirExpr::ArrayAccess(_, ref mut idx) => {
            rename_calls_in_hir_expr(idx, from, to);
        }
        HirExpr::CallRef {
            ref mut callee,
            ref mut args,
        } => {
            rename_calls_in_hir_expr(callee, from, to);
            for arg in args.iter_mut() {
                rename_calls_in_hir_expr(arg, from, to);
            }
        }
        HirExpr::Async { ref mut body } => {
            rename_calls_in_hir_stmt(body, from, to);
        }
        HirExpr::Await { ref mut expr } => {
            rename_calls_in_hir_expr(expr, from, to);
        }
        HirExpr::Perform { ref mut args, .. } => {
            for arg in args.iter_mut() {
                rename_calls_in_hir_expr(arg, from, to);
            }
        }
        HirExpr::VariantInit { ref mut fields, .. } => {
            for f in fields.iter_mut() {
                rename_calls_in_hir_expr(f, from, to);
            }
        }
        // Leaf nodes — no calls to rename
        HirExpr::Number(_)
        | HirExpr::Float(_)
        | HirExpr::StringLit(_)
        | HirExpr::Variable(_)
        | HirExpr::AtomRef { .. } => {}
        // NOTE: Task, TaskGroup, Lambda, ChanSend, ChanRecv are not expected in
        // binary compilation paths but handled defensively.
        #[allow(unreachable_patterns)]
        _ => {}
    }
}

/// Recursively rename function calls in a HirStmt from `from` to `to`.
fn rename_calls_in_hir_stmt(stmt: &mut HirStmt, from: &str, to: &str) {
    match stmt {
        HirStmt::Let { ref mut value, .. } => {
            rename_calls_in_hir_expr(value, from, to);
        }
        HirStmt::Assign { ref mut value, .. } => {
            rename_calls_in_hir_expr(value, from, to);
        }
        HirStmt::While {
            ref mut cond,
            ref mut invariant,
            ref mut decreases,
            ref mut body,
        } => {
            rename_calls_in_hir_expr(cond, from, to);
            rename_calls_in_hir_expr(invariant, from, to);
            if let Some(ref mut d) = decreases {
                rename_calls_in_hir_expr(d, from, to);
            }
            rename_calls_in_hir_stmt(body, from, to);
        }
        HirStmt::Block {
            ref mut stmts,
            ref mut tail_expr,
        } => {
            for s in stmts.iter_mut() {
                rename_calls_in_hir_stmt(s, from, to);
            }
            if let Some(ref mut tail) = tail_expr {
                rename_calls_in_hir_expr(tail, from, to);
            }
        }
        HirStmt::Acquire { ref mut body, .. } => {
            rename_calls_in_hir_stmt(body, from, to);
        }
        HirStmt::Expr(ref mut e) => {
            rename_calls_in_hir_expr(e, from, to);
        }
    }
}

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

    // Check that a "main" atom exists and takes no parameters
    let main_atom = hir_atoms.iter().find(|h| h.atom.name == "main");
    match main_atom {
        None => {
            return Err(MumeiError::codegen(
                "No `atom main()` found. A main atom is required for binary compilation."
                    .to_string(),
            ));
        }
        Some(m) if !m.atom.params.is_empty() => {
            return Err(MumeiError::codegen(format!(
                "atom main() must take no parameters for binary compilation, but found {} parameter(s). \
                 Define main as: atom main() requires: ...; ensures: ...; body: {{ ... }}",
                m.atom.params.len()
            )));
        }
        _ => {}
    }

    // Compile all atoms into the merged module.
    // For the "main" atom, rename it to "__mumei_user_main" before compilation
    // so it doesn't conflict with the C main wrapper we'll generate.
    for hir_atom in hir_atoms {
        if hir_atom.atom.name == "main" {
            // Clone and rename to avoid C main conflict
            let mut renamed = hir_atom.clone();
            renamed.atom.name = "__mumei_user_main".to_string();
            // Rename self-recursive calls inside the body so they target
            // __mumei_user_main instead of the C wrapper main.
            rename_calls_in_hir_stmt(&mut renamed.body, "main", "__mumei_user_main");
            renamed.atom.body_expr = renamed
                .atom
                .body_expr
                .replace("main(", "__mumei_user_main(");
            compile_atom_into_module(
                &context,
                &merged_module,
                &renamed,
                module_env,
                extern_blocks,
            )?;
        } else {
            // Rename any calls to "main" in non-main atoms so they target
            // __mumei_user_main instead of the C wrapper main(argc, argv).
            let mut patched = hir_atom.clone();
            rename_calls_in_hir_stmt(&mut patched.body, "main", "__mumei_user_main");
            patched.atom.body_expr = patched
                .atom
                .body_expr
                .replace("main(", "__mumei_user_main(");
            compile_atom_into_module(
                &context,
                &merged_module,
                &patched,
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

    // Convert result to i32 for C-compatible process exit code.
    // Handle both integer and float return types from __mumei_user_main.
    let exit_code = if result.is_int_value() {
        builder
            .build_int_truncate(result.into_int_value(), i32_type, "exit_code")
            .map_err(|e| MumeiError::codegen(format!("Failed to build truncate: {}", e)))?
    } else if result.is_float_value() {
        builder
            .build_float_to_signed_int(result.into_float_value(), i32_type, "exit_code")
            .map_err(|e| MumeiError::codegen(format!("Failed to build float to int: {}", e)))?
    } else {
        return Err(MumeiError::codegen(
            "main() must return an integer or float type for binary compilation".to_string(),
        ));
    };
    builder
        .build_return(Some(&exit_code))
        .map_err(|e| MumeiError::codegen(format!("Failed to build return: {}", e)))?;

    // Write merged module to .ll file
    merged_module
        .print_to_file(output_ll_path)
        .map_err(|e| MumeiError::codegen(format!("Failed to write LLVM IR: {}", e)))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mumei_core::hir::{HirEffectSet, HirExpr, HirStmt};
    use mumei_core::parser::{Atom, Span};

    /// Helper to build a minimal HirAtom for testing
    fn make_test_hir_atom(name: &str, body: HirStmt, body_expr: &str) -> HirAtom {
        HirAtom {
            body,
            requires_hir: HirExpr::Number(1),
            ensures_hir: HirExpr::Number(1),
            atom: Atom {
                name: name.to_string(),
                type_params: vec![],
                where_bounds: vec![],
                params: vec![],
                requires: "true".to_string(),
                forall_constraints: vec![],
                ensures: "true".to_string(),
                body_expr: body_expr.to_string(),
                consumed_params: vec![],
                resources: vec![],
                is_async: false,
                trust_level: mumei_core::parser::TrustLevel::Verified,
                max_unroll: None,
                invariant: None,
                effects: vec![],
                return_type: None,
                span: Span::default(),
                effect_pre: std::collections::HashMap::new(),
                effect_post: std::collections::HashMap::new(),
            },
            body_stmt: mumei_core::parser::Stmt::Expr(
                mumei_core::parser::Expr::Number(0),
                Span::default(),
            ),
            effect_set: HirEffectSet::default(),
        }
    }

    #[test]
    fn test_rename_calls_in_hir_expr_simple() {
        let mut expr = HirExpr::Call {
            name: "main".to_string(),
            args: vec![HirExpr::Number(42)],
            callee_effects: None,
        };
        rename_calls_in_hir_expr(&mut expr, "main", "__mumei_user_main");
        match &expr {
            HirExpr::Call { name, .. } => {
                assert_eq!(name, "__mumei_user_main");
            }
            _ => panic!("Expected Call"),
        }
    }

    #[test]
    fn test_rename_calls_in_hir_expr_nested() {
        // main(main(1) + 2) — both calls should be renamed
        let inner_call = HirExpr::Call {
            name: "main".to_string(),
            args: vec![HirExpr::Number(1)],
            callee_effects: None,
        };
        let bin = HirExpr::BinaryOp(
            Box::new(inner_call),
            mumei_core::parser::Op::Add,
            Box::new(HirExpr::Number(2)),
        );
        let mut outer = HirExpr::Call {
            name: "main".to_string(),
            args: vec![bin],
            callee_effects: None,
        };
        rename_calls_in_hir_expr(&mut outer, "main", "__mumei_user_main");
        match &outer {
            HirExpr::Call { name, args, .. } => {
                assert_eq!(name, "__mumei_user_main");
                match &args[0] {
                    HirExpr::BinaryOp(lhs, _, _) => match lhs.as_ref() {
                        HirExpr::Call { name, .. } => {
                            assert_eq!(name, "__mumei_user_main");
                        }
                        _ => panic!("Expected inner Call"),
                    },
                    _ => panic!("Expected BinaryOp"),
                }
            }
            _ => panic!("Expected Call"),
        }
    }

    #[test]
    fn test_rename_calls_in_hir_stmt_block() {
        let call = HirExpr::Call {
            name: "main".to_string(),
            args: vec![],
            callee_effects: None,
        };
        let mut stmt = HirStmt::Block {
            stmts: vec![HirStmt::Let {
                var: "x".to_string(),
                ty: None,
                value: Box::new(call),
            }],
            tail_expr: Some(Box::new(HirExpr::Variable("x".to_string()))),
        };
        rename_calls_in_hir_stmt(&mut stmt, "main", "__mumei_user_main");
        match &stmt {
            HirStmt::Block { stmts, .. } => match &stmts[0] {
                HirStmt::Let { value, .. } => match value.as_ref() {
                    HirExpr::Call { name, .. } => {
                        assert_eq!(name, "__mumei_user_main");
                    }
                    _ => panic!("Expected Call"),
                },
                _ => panic!("Expected Let"),
            },
            _ => panic!("Expected Block"),
        }
    }

    #[test]
    fn test_rename_does_not_affect_other_names() {
        let mut expr = HirExpr::Call {
            name: "other_function".to_string(),
            args: vec![],
            callee_effects: None,
        };
        rename_calls_in_hir_expr(&mut expr, "main", "__mumei_user_main");
        match &expr {
            HirExpr::Call { name, .. } => {
                assert_eq!(name, "other_function");
            }
            _ => panic!("Expected Call"),
        }
    }

    #[test]
    fn test_impl_block_method_hir_atom_construction() {
        // Verify that a HirAtom with a qualified name (Struct::method) can be constructed
        let body = HirStmt::Expr(HirExpr::Number(42));
        let atom = make_test_hir_atom("MyStruct::get_value", body, "42");
        assert_eq!(atom.atom.name, "MyStruct::get_value");
    }
}
