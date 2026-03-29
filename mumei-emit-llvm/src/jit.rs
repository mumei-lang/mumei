// =============================================================================
// P7-A: JIT Execution Engine for REPL
// =============================================================================
// Provides in-memory compilation and execution of verified mumei atoms
// using inkwell's ExecutionEngine (LLVM MCJIT).

use inkwell::context::Context;
use inkwell::execution_engine::ExecutionEngine;
use inkwell::module::Module;
use inkwell::OptimizationLevel;
use mumei_core::hir::HirAtom;
use mumei_core::verification::{ModuleEnv, MumeiError, MumeiResult};

use crate::codegen;

/// JIT execution engine that owns an LLVM Context, Module, and ExecutionEngine.
/// Atoms are compiled into the module and can be executed immediately.
pub struct JitEngine<'ctx> {
    context: &'ctx Context,
    module: Module<'ctx>,
    execution_engine: ExecutionEngine<'ctx>,
}

/// Result of JIT execution — the return value of the executed atom.
#[derive(Debug, Clone)]
pub enum JitValue {
    I64(i64),
    F64(f64),
}

impl std::fmt::Display for JitValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JitValue::I64(v) => write!(f, "{}", v),
            JitValue::F64(v) => write!(f, "{}", v),
        }
    }
}

impl<'ctx> JitEngine<'ctx> {
    /// Create a new JIT engine from an externally-owned Context.
    pub fn new(context: &'ctx Context) -> MumeiResult<Self> {
        let module = context.create_module("mumei_jit");

        let execution_engine = module
            .create_jit_execution_engine(OptimizationLevel::Default)
            .map_err(|e| MumeiError::codegen(format!("Failed to create JIT engine: {}", e)))?;

        Ok(JitEngine {
            context,
            module,
            execution_engine,
        })
    }

    /// Compile an atom into the JIT module so it can be called.
    pub fn compile_atom(
        &self,
        hir_atom: &HirAtom,
        module_env: &ModuleEnv,
        extern_blocks: &[mumei_core::parser::ExternBlock],
    ) -> MumeiResult<()> {
        codegen::compile_atom_into_module(
            self.context,
            &self.module,
            hir_atom,
            module_env,
            extern_blocks,
        )
    }

    /// Execute a no-argument atom that returns i64.
    pub fn execute_i64(&self, func_name: &str) -> MumeiResult<i64> {
        unsafe {
            let func = self
                .execution_engine
                .get_function::<unsafe extern "C" fn() -> i64>(func_name)
                .map_err(|e| {
                    MumeiError::codegen(format!("JIT function '{}' not found: {}", func_name, e))
                })?;
            Ok(func.call())
        }
    }

    /// Execute a no-argument atom that returns f64.
    pub fn execute_f64(&self, func_name: &str) -> MumeiResult<f64> {
        unsafe {
            let func = self
                .execution_engine
                .get_function::<unsafe extern "C" fn() -> f64>(func_name)
                .map_err(|e| {
                    MumeiError::codegen(format!("JIT function '{}' not found: {}", func_name, e))
                })?;
            Ok(func.call())
        }
    }

    /// Remove a function from the module (used for temporary __repl_eval atoms).
    pub fn remove_function(&self, func_name: &str) {
        if let Some(func) = self.module.get_function(func_name) {
            unsafe {
                func.delete();
            }
        }
    }

    /// Check if a function exists in the module.
    pub fn has_function(&self, func_name: &str) -> bool {
        self.module.get_function(func_name).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mumei_core::hir::{HirEffectSet, HirExpr, HirStmt};
    use mumei_core::parser::ast::{Atom, Expr, Param, Span, Stmt, TrustLevel};

    /// Helper: build a minimal HirAtom for JIT testing.
    fn make_hir_atom(
        name: &str,
        params: Vec<Param>,
        body: HirStmt,
        return_type: Option<String>,
    ) -> HirAtom {
        let span = Span::new("", 0, 0, 0);
        HirAtom {
            body,
            requires_hir: HirExpr::Number(1),
            ensures_hir: HirExpr::Number(1),
            atom: Atom {
                name: name.to_string(),
                type_params: vec![],
                where_bounds: vec![],
                params,
                requires: "true".to_string(),
                forall_constraints: vec![],
                ensures: "true".to_string(),
                body_expr: "0".to_string(),
                consumed_params: vec![],
                resources: vec![],
                is_async: false,
                trust_level: TrustLevel::Verified,
                max_unroll: None,
                invariant: None,
                effects: vec![],
                return_type,
                span: span.clone(),
                effect_pre: std::collections::HashMap::new(),
                effect_post: std::collections::HashMap::new(),
            },
            body_stmt: Stmt::Expr(Expr::Number(0), span),
            effect_set: HirEffectSet::default(),
        }
    }

    #[test]
    fn test_jit_engine_creation() {
        let context = Context::create();
        let engine = JitEngine::new(&context);
        assert!(engine.is_ok(), "JIT engine should be created successfully");
    }

    #[test]
    fn test_jit_compile_and_execute_i64() {
        let context = Context::create();
        let engine = JitEngine::new(&context).unwrap();
        let module_env = ModuleEnv::new();

        // Create atom: atom answer() requires: true; ensures: true; body: 42;
        let hir = make_hir_atom(
            "answer",
            vec![],
            HirStmt::Expr(HirExpr::Number(42)),
            Some("i64".to_string()),
        );

        engine.compile_atom(&hir, &module_env, &[]).unwrap();
        let result = engine.execute_i64("answer").unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn test_jit_compile_and_execute_f64() {
        let context = Context::create();
        let engine = JitEngine::new(&context).unwrap();
        let module_env = ModuleEnv::new();

        // Create atom with f64 return: body: 3.14
        let hir = make_hir_atom(
            "pi_approx",
            vec![],
            HirStmt::Expr(HirExpr::Float(3.14)),
            Some("f64".to_string()),
        );

        engine.compile_atom(&hir, &module_env, &[]).unwrap();
        let result = engine.execute_f64("pi_approx").unwrap();
        assert!((result - 3.14).abs() < 1e-10);
    }

    #[test]
    fn test_jit_remove_function() {
        let context = Context::create();
        let engine = JitEngine::new(&context).unwrap();
        let module_env = ModuleEnv::new();

        let hir = make_hir_atom(
            "__repl_eval",
            vec![],
            HirStmt::Expr(HirExpr::Number(99)),
            Some("i64".to_string()),
        );

        engine.compile_atom(&hir, &module_env, &[]).unwrap();
        assert!(engine.has_function("__repl_eval"));

        engine.remove_function("__repl_eval");
        assert!(!engine.has_function("__repl_eval"));
    }

    #[test]
    fn test_jit_binary_op() {
        let context = Context::create();
        let engine = JitEngine::new(&context).unwrap();
        let module_env = ModuleEnv::new();

        // atom add_test() body: { let x = 5; let y = 10; x + y }
        let hir = make_hir_atom(
            "add_test",
            vec![],
            HirStmt::Block {
                stmts: vec![
                    HirStmt::Let {
                        var: "x".to_string(),
                        ty: None,
                        value: Box::new(HirExpr::Number(5)),
                    },
                    HirStmt::Let {
                        var: "y".to_string(),
                        ty: None,
                        value: Box::new(HirExpr::Number(10)),
                    },
                ],
                tail_expr: Some(Box::new(HirExpr::BinaryOp(
                    Box::new(HirExpr::Variable("x".to_string())),
                    mumei_core::parser::Op::Add,
                    Box::new(HirExpr::Variable("y".to_string())),
                ))),
            },
            Some("i64".to_string()),
        );

        engine.compile_atom(&hir, &module_env, &[]).unwrap();
        let result = engine.execute_i64("add_test").unwrap();
        assert_eq!(result, 15);
    }
}
