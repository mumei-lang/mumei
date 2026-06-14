// =============================================================================
// P7-A: JIT Execution Engine for REPL
// =============================================================================
// Provides in-memory compilation and execution of verified mumei atoms
// using LLVM ORC LLJIT.

use inkwell::context::Context;
use inkwell::targets::{InitializationConfig, Target};
use llvm_sys::core::{LLVMSetDataLayout, LLVMSetTarget};
use llvm_sys::error::{
    LLVMConsumeError, LLVMDisposeErrorMessage, LLVMErrorRef, LLVMGetErrorMessage,
};
use llvm_sys::orc2::lljit::{
    LLVMOrcCreateLLJIT, LLVMOrcCreateLLJITBuilder, LLVMOrcDisposeLLJIT,
    LLVMOrcLLJITAddLLVMIRModule, LLVMOrcLLJITAddLLVMIRModuleWithRT, LLVMOrcLLJITGetDataLayoutStr,
    LLVMOrcLLJITGetGlobalPrefix, LLVMOrcLLJITGetMainJITDylib, LLVMOrcLLJITGetTripleString,
    LLVMOrcLLJITLookup, LLVMOrcLLJITMangleAndIntern, LLVMOrcLLJITRef,
};
use llvm_sys::orc2::{
    LLVMJITEvaluatedSymbol, LLVMJITSymbolFlags, LLVMJITSymbolGenericFlags, LLVMOrcAbsoluteSymbols,
    LLVMOrcCreateDynamicLibrarySearchGeneratorForProcess, LLVMOrcCreateNewThreadSafeContext,
    LLVMOrcCreateNewThreadSafeModule, LLVMOrcDisposeThreadSafeContext, LLVMOrcJITDylibAddGenerator,
    LLVMOrcJITDylibCreateResourceTracker, LLVMOrcJITDylibDefine, LLVMOrcJITDylibRef,
    LLVMOrcResourceTrackerRef, LLVMOrcResourceTrackerRemove, LLVMOrcThreadSafeContextGetContext,
};
use mumei_core::hir::HirAtom;
use mumei_core::verification::{ModuleEnv, MumeiError, MumeiResult};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::marker::PhantomData;
use std::mem::{self, MaybeUninit};
use std::ptr;

use crate::codegen;

/// JIT execution engine backed by LLVM ORC LLJIT.
/// Atoms are compiled as independent modules and linked into one JITDylib.
pub struct JitEngine<'ctx> {
    lljit: LLVMOrcLLJITRef,
    main_jit_dylib: LLVMOrcJITDylibRef,
    compiled_functions: RefCell<HashSet<String>>,
    resource_trackers: RefCell<HashMap<String, LLVMOrcResourceTrackerRef>>,
    _marker: PhantomData<&'ctx Context>,
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
    /// Create a new LLJIT engine. The context parameter is kept for API compatibility.
    pub fn new(_context: &'ctx Context) -> MumeiResult<Self> {
        Target::initialize_native(&InitializationConfig::default()).map_err(|e| {
            MumeiError::codegen(format!("Failed to initialize native LLVM target: {e}"))
        })?;
        Self::promote_llvm_symbols_for_orc()?;

        let lljit = unsafe {
            let builder = LLVMOrcCreateLLJITBuilder();
            if builder.is_null() {
                return Err(MumeiError::codegen("Failed to create ORC JIT builder"));
            }

            let mut lljit = MaybeUninit::uninit();
            let err = LLVMOrcCreateLLJIT(lljit.as_mut_ptr(), builder);
            Self::take_error(err, "Failed to create ORC JIT")?;
            let lljit = lljit.assume_init();
            if lljit.is_null() {
                return Err(MumeiError::codegen("Failed to create ORC JIT"));
            }
            lljit
        };

        let main_jit_dylib = unsafe { LLVMOrcLLJITGetMainJITDylib(lljit) };
        if main_jit_dylib.is_null() {
            unsafe {
                Self::consume_error(LLVMOrcDisposeLLJIT(lljit));
            }
            return Err(MumeiError::codegen("Failed to get ORC main JITDylib"));
        }

        let engine = JitEngine {
            lljit,
            main_jit_dylib,
            compiled_functions: RefCell::new(HashSet::new()),
            resource_trackers: RefCell::new(HashMap::new()),
            _marker: PhantomData,
        };
        engine.register_process_symbols()?;
        engine.register_runtime_symbols()?;
        Ok(engine)
    }

    #[cfg(unix)]
    fn promote_llvm_symbols_for_orc() -> MumeiResult<()> {
        const CANDIDATES: &[&str] = &[
            "libLLVM-17.so.1",
            "libLLVM-17.so",
            "/usr/lib/llvm-17/lib/libLLVM-17.so.1",
            "/usr/lib/llvm-17/lib/libLLVM-17.so",
            "/usr/lib/x86_64-linux-gnu/libLLVM-17.so.1",
            "/usr/lib/x86_64-linux-gnu/libLLVM-17.so",
        ];

        let mut last_error = None;
        for candidate in CANDIDATES {
            let lib = CString::new(*candidate).map_err(|e| {
                MumeiError::codegen(format!("Invalid LLVM shared library path: {e}"))
            })?;
            let handle = unsafe { libc::dlopen(lib.as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL) };
            if !handle.is_null() {
                return Ok(());
            }
            let err = unsafe { libc::dlerror() };
            if !err.is_null() {
                last_error = Some(unsafe { CStr::from_ptr(err).to_string_lossy().into_owned() });
            }
        }

        Err(MumeiError::codegen(format!(
            "Failed to expose LLVM ORC process symbols{}",
            last_error.map(|err| format!(": {err}")).unwrap_or_default()
        )))
    }

    #[cfg(not(unix))]
    fn promote_llvm_symbols_for_orc() -> MumeiResult<()> {
        Ok(())
    }

    /// Compile an atom into the JIT module so it can be called.
    pub fn compile_atom(
        &self,
        hir_atom: &HirAtom,
        module_env: &ModuleEnv,
        extern_blocks: &[mumei_core::parser::ExternBlock],
    ) -> MumeiResult<()> {
        let atom_name = hir_atom.atom.name.clone();
        if self.resource_trackers.borrow().contains_key(&atom_name) {
            self.remove_function(&atom_name);
        } else if self.compiled_functions.borrow().contains(&atom_name) {
            return Ok(());
        }

        let thread_safe_context = unsafe { LLVMOrcCreateNewThreadSafeContext() };
        if thread_safe_context.is_null() {
            return Err(MumeiError::codegen(
                "Failed to create ORC thread-safe context",
            ));
        }

        let context_ref = unsafe { LLVMOrcThreadSafeContextGetContext(thread_safe_context) };
        if context_ref.is_null() {
            unsafe {
                LLVMOrcDisposeThreadSafeContext(thread_safe_context);
            }
            return Err(MumeiError::codegen("Failed to get LLVM context from ORC"));
        }

        let context = unsafe { Context::new(context_ref) };
        let module = context.create_module(&atom_name);
        unsafe {
            let triple = LLVMOrcLLJITGetTripleString(self.lljit);
            if !triple.is_null() {
                LLVMSetTarget(module.as_mut_ptr(), triple);
            }
            let data_layout = LLVMOrcLLJITGetDataLayoutStr(self.lljit);
            if !data_layout.is_null() {
                LLVMSetDataLayout(module.as_mut_ptr(), data_layout);
            }
        }

        if let Err(err) = codegen::compile_atom_into_module(
            &context,
            &module,
            hir_atom,
            module_env,
            extern_blocks,
        ) {
            drop(module);
            mem::forget(context);
            unsafe {
                LLVMOrcDisposeThreadSafeContext(thread_safe_context);
            }
            return Err(err);
        }

        let module_ref = module.as_mut_ptr();
        mem::forget(module);
        mem::forget(context);
        let thread_safe_module =
            unsafe { LLVMOrcCreateNewThreadSafeModule(module_ref, thread_safe_context) };
        if thread_safe_module.is_null() {
            unsafe {
                LLVMOrcDisposeThreadSafeContext(thread_safe_context);
            }
            return Err(MumeiError::codegen(
                "Failed to create ORC thread-safe module",
            ));
        }

        let resource_tracker = if atom_name == "__repl_eval" {
            let tracker = unsafe { LLVMOrcJITDylibCreateResourceTracker(self.main_jit_dylib) };
            if tracker.is_null() {
                return Err(MumeiError::codegen("Failed to create ORC resource tracker"));
            }
            Some(tracker)
        } else {
            None
        };

        let add_err = unsafe {
            match resource_tracker {
                Some(tracker) => {
                    LLVMOrcLLJITAddLLVMIRModuleWithRT(self.lljit, tracker, thread_safe_module)
                }
                None => {
                    LLVMOrcLLJITAddLLVMIRModule(self.lljit, self.main_jit_dylib, thread_safe_module)
                }
            }
        };
        if let Err(err) = Self::take_error(add_err, "Failed to add module to ORC JIT") {
            if let Some(tracker) = resource_tracker {
                unsafe {
                    Self::consume_error(LLVMOrcResourceTrackerRemove(tracker));
                }
            }
            return Err(err);
        }

        self.compiled_functions
            .borrow_mut()
            .insert(atom_name.clone());
        if let Some(tracker) = resource_tracker {
            self.resource_trackers
                .borrow_mut()
                .insert(atom_name, tracker);
        }
        Ok(())
    }

    fn take_error(err: LLVMErrorRef, context: &str) -> MumeiResult<()> {
        if err.is_null() {
            return Ok(());
        }

        let message = unsafe {
            let message_ptr = LLVMGetErrorMessage(err);
            if message_ptr.is_null() {
                return Err(MumeiError::codegen(context.to_string()));
            }
            let message = CStr::from_ptr(message_ptr).to_string_lossy().into_owned();
            LLVMDisposeErrorMessage(message_ptr);
            message
        };
        Err(MumeiError::codegen(format!("{context}: {message}")))
    }

    unsafe fn consume_error(err: LLVMErrorRef) {
        if !err.is_null() {
            LLVMConsumeError(err);
        }
    }

    fn register_process_symbols(&self) -> MumeiResult<()> {
        let mut generator = ptr::null_mut();
        let err = unsafe {
            LLVMOrcCreateDynamicLibrarySearchGeneratorForProcess(
                &mut generator,
                LLVMOrcLLJITGetGlobalPrefix(self.lljit),
                None,
                ptr::null_mut(),
            )
        };
        Self::take_error(err, "Failed to create ORC process symbol generator")?;
        if !generator.is_null() {
            unsafe {
                LLVMOrcJITDylibAddGenerator(self.main_jit_dylib, generator);
            }
        }
        Ok(())
    }

    fn register_runtime_symbols(&self) -> MumeiResult<()> {
        for (name, address) in Self::runtime_symbols() {
            let c_name = CString::new(name)
                .map_err(|e| MumeiError::codegen(format!("Invalid symbol name '{name}': {e}")))?;
            let symbol = unsafe { LLVMOrcLLJITMangleAndIntern(self.lljit, c_name.as_ptr()) };
            if symbol.is_null() {
                return Err(MumeiError::codegen(format!(
                    "Failed to intern runtime symbol '{name}'"
                )));
            }

            let mut pair = llvm_sys::orc2::LLVMOrcCSymbolMapPair {
                Name: symbol,
                Sym: LLVMJITEvaluatedSymbol {
                    Address: address as u64,
                    Flags: LLVMJITSymbolFlags {
                        GenericFlags: (LLVMJITSymbolGenericFlags::LLVMJITSymbolGenericFlagsExported
                            as u8)
                            | (LLVMJITSymbolGenericFlags::LLVMJITSymbolGenericFlagsCallable as u8),
                        TargetFlags: 0,
                    },
                },
            };

            let materialization_unit = unsafe { LLVMOrcAbsoluteSymbols(&mut pair, 1) };
            if materialization_unit.is_null() {
                return Err(MumeiError::codegen(format!(
                    "Failed to create ORC absolute symbol for '{name}'"
                )));
            }
            let err = unsafe { LLVMOrcJITDylibDefine(self.main_jit_dylib, materialization_unit) };
            Self::take_error(err, &format!("Failed to define runtime symbol '{name}'"))?;
        }
        Ok(())
    }

    fn runtime_symbols() -> Vec<(&'static str, usize)> {
        let mut symbols = Vec::new();
        macro_rules! map_symbol {
            ($name:literal, $path:path) => {
                symbols.push(($name, $path as *const () as usize));
            };
        }

        map_symbol!("file_read", mumei_core::ffi::file::file_read);
        map_symbol!("file_write", mumei_core::ffi::file::file_write);
        map_symbol!("file_exists", mumei_core::ffi::file::file_exists);
        map_symbol!("file_delete", mumei_core::ffi::file::file_delete);

        map_symbol!("json_parse", mumei_core::ffi::json::json_parse);
        map_symbol!("json_stringify", mumei_core::ffi::json::json_stringify);
        map_symbol!("json_get", mumei_core::ffi::json::json_get);
        map_symbol!("json_get_int", mumei_core::ffi::json::json_get_int);
        map_symbol!("json_get_str", mumei_core::ffi::json::json_get_str);
        map_symbol!("json_get_bool", mumei_core::ffi::json::json_get_bool);
        map_symbol!("json_array_len", mumei_core::ffi::json::json_array_len);
        map_symbol!("json_array_get", mumei_core::ffi::json::json_array_get);
        map_symbol!("json_is_null", mumei_core::ffi::json::json_is_null);
        map_symbol!("json_is_object", mumei_core::ffi::json::json_is_object);
        map_symbol!("json_is_array", mumei_core::ffi::json::json_is_array);
        map_symbol!("json_object_new", mumei_core::ffi::json::json_object_new);
        map_symbol!("json_object_set", mumei_core::ffi::json::json_object_set);
        map_symbol!("json_array_new", mumei_core::ffi::json::json_array_new);
        map_symbol!("json_array_push", mumei_core::ffi::json::json_array_push);
        map_symbol!("json_from_int", mumei_core::ffi::json::json_from_int);
        map_symbol!("json_from_str", mumei_core::ffi::json::json_from_str);
        map_symbol!("json_from_bool", mumei_core::ffi::json::json_from_bool);
        map_symbol!("mumei_str_concat", mumei_core::ffi::json::mumei_str_concat);
        map_symbol!("mumei_str_eq", mumei_core::ffi::json::mumei_str_eq);
        map_symbol!("json_free", mumei_core::ffi::json::json_free);
        map_symbol!("string_free", mumei_core::ffi::json::string_free);
        map_symbol!("mumei_str_alloc", mumei_core::ffi::json::mumei_str_alloc);
        map_symbol!("mumei_str_free", mumei_core::ffi::json::mumei_str_free);
        map_symbol!("mumei_str_get", mumei_core::ffi::json::mumei_str_get);

        map_symbol!("http_get", mumei_core::ffi::http::http_get);
        map_symbol!("http_post", mumei_core::ffi::http::http_post);
        map_symbol!("http_put", mumei_core::ffi::http::http_put);
        map_symbol!("http_delete", mumei_core::ffi::http::http_delete);
        map_symbol!("http_status", mumei_core::ffi::http::http_status);
        map_symbol!("http_body", mumei_core::ffi::http::http_body);
        map_symbol!("http_body_json", mumei_core::ffi::http::http_body_json);
        map_symbol!("http_header_get", mumei_core::ffi::http::http_header_get);
        map_symbol!("http_header_set", mumei_core::ffi::http::http_header_set);
        map_symbol!("http_is_ok", mumei_core::ffi::http::http_is_ok);
        map_symbol!("http_is_error", mumei_core::ffi::http::http_is_error);
        map_symbol!("http_free", mumei_core::ffi::http::http_free);

        map_symbol!(
            "http_server_bind",
            mumei_core::ffi::http_server::http_server_bind
        );
        map_symbol!(
            "http_server_listen",
            mumei_core::ffi::http_server::http_server_listen
        );
        map_symbol!(
            "http_server_accept",
            mumei_core::ffi::http_server::http_server_accept
        );
        map_symbol!(
            "http_request_path",
            mumei_core::ffi::http_server::http_request_path
        );
        map_symbol!(
            "http_request_method",
            mumei_core::ffi::http_server::http_request_method
        );
        map_symbol!(
            "http_server_respond",
            mumei_core::ffi::http_server::http_server_respond
        );
        map_symbol!(
            "http_server_free",
            mumei_core::ffi::http_server::http_server_free
        );
        map_symbol!(
            "http_request_free",
            mumei_core::ffi::http_server::http_request_free
        );

        map_symbol!("crypto_sha256", mumei_core::ffi::crypto::crypto_sha256);
        map_symbol!("crypto_hash_eq", mumei_core::ffi::crypto::crypto_hash_eq);
        map_symbol!(
            "crypto_hmac_sha256",
            mumei_core::ffi::crypto::crypto_hmac_sha256
        );
        map_symbol!(
            "crypto_verify_signature",
            mumei_core::ffi::crypto::crypto_verify_signature
        );

        symbols
    }

    /// Execute a no-argument atom that returns i64.
    pub fn execute_i64(&self, func_name: &str) -> MumeiResult<i64> {
        unsafe {
            let address = self.lookup_function(func_name)?;
            let func = mem::transmute::<u64, unsafe extern "C" fn() -> i64>(address);
            Ok(func())
        }
    }

    /// Execute a no-argument atom that returns f64.
    pub fn execute_f64(&self, func_name: &str) -> MumeiResult<f64> {
        unsafe {
            let address = self.lookup_function(func_name)?;
            let func = mem::transmute::<u64, unsafe extern "C" fn() -> f64>(address);
            Ok(func())
        }
    }

    unsafe fn lookup_function(&self, func_name: &str) -> MumeiResult<u64> {
        let c_name = CString::new(func_name).map_err(|e| {
            MumeiError::codegen(format!("Invalid JIT function name '{}': {}", func_name, e))
        })?;
        let mut address = MaybeUninit::uninit();
        let err = LLVMOrcLLJITLookup(self.lljit, address.as_mut_ptr(), c_name.as_ptr());
        Self::take_error(err, &format!("JIT function '{}' not found", func_name))?;
        Ok(address.assume_init())
    }

    /// Remove a function from the module (used for temporary __repl_eval atoms).
    pub fn remove_function(&self, func_name: &str) {
        if let Some(tracker) = self.resource_trackers.borrow_mut().remove(func_name) {
            unsafe {
                Self::consume_error(LLVMOrcResourceTrackerRemove(tracker));
            }
            self.compiled_functions.borrow_mut().remove(func_name);
        }
    }

    /// Check if a function exists in the module.
    pub fn has_function(&self, func_name: &str) -> bool {
        self.compiled_functions.borrow().contains(func_name)
    }
}

impl Drop for JitEngine<'_> {
    fn drop(&mut self) {
        for (_, tracker) in self.resource_trackers.borrow_mut().drain() {
            unsafe {
                Self::consume_error(LLVMOrcResourceTrackerRemove(tracker));
            }
        }
        unsafe {
            Self::consume_error(LLVMOrcDisposeLLJIT(self.lljit));
        }
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
                trace_id: None,
                spec_metadata: std::collections::HashMap::new(),
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
