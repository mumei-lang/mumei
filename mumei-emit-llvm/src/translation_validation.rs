//! Opt-in equivalence checking of one C03 body and one supplied LLVM 17 SSA snapshot.
//! Not postcondition verification, pipeline attestation or final-binary verification.

use inkwell::{context::Context as LlvmContext, memory_buffer::MemoryBuffer, values::AsValueRef};
use llvm_sys::{core::*, prelude::*, LLVMIntPredicate, LLVMOpcode, LLVMTypeKind};
use mumei_core::semantic_contract::{encode_pure_i64_body, PURE_I64_WRAPPING_PROFILE};
use std::collections::HashMap;
use z3::{
    ast::{Ast, Bool, BV},
    Config, Context, SatResult, Solver,
};

pub const PROFILE: &str = "mumei.llvm17-pure-i64-ssa-equivalence/v1";
pub const NATIVE_PROFILE: &str = "mumei.llvm17-native-main-snapshot/v1";
pub const NATIVE_TARGET: &str = "x86_64-pc-linux-gnu";
pub const TARGET: &str = "x86_64-unknown-linux-gnu";
pub const DATA_LAYOUT: &str =
    "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128";
const MAX_BYTES: usize = 65_536;
const MAX_DEPTH: usize = 48;
const MAX_BLOCKS: usize = 64;
const MAX_INSTRUCTIONS: usize = 1024;
const MAX_STATES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    EquivalentWithinProfile,
    Different,
    /// The supplied IR reaches undefined behavior (division by zero, the
    /// i64::MIN / -1 pair or an out-of-range shift amount) on an input the
    /// source's own defined domain covers. Reported separately from
    /// `Different`: no produced value is wrong, the operation itself is.
    UndefinedBehavior,
    Inconclusive,
}

#[derive(Debug)]
pub struct ValidationReport {
    pub profile: &'static str,
    pub source_profile: &'static str,
    pub target: &'static str,
    pub data_layout: &'static str,
    pub checked_stage: &'static str,
    pub input_domain: &'static str,
    pub source_sha256: String,
    pub llvm_sha256: String,
    pub timeout_ms: u32,
    pub resource_limit: u32,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub timeout_ms: u32,
    pub resource_limit: u32,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            timeout_ms: 5_000,
            resource_limit: 1_000_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError(pub &'static str);

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for ValidationError {}

type Result<T> = std::result::Result<T, ValidationError>;

fn bounded(text: &str) -> Result<()> {
    if text.len() > MAX_BYTES {
        return Err(ValidationError("input size limit"));
    }
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    let mut comment = false;
    for byte in text.bytes() {
        if comment {
            if byte == b'\n' {
                comment = false;
            }
            continue;
        }
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
            continue;
        }
        match byte {
            b';' => comment = true,
            b'"' => quoted = true,
            b'(' | b'[' | b'{' | b'<' => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err(ValidationError("input nesting limit"));
                }
            }
            b')' | b']' | b'}' | b'>' => depth = depth.saturating_sub(1),
            _ => (),
        }
    }
    Ok(())
}

/// Compare all signed-i64 inputs, independently of requires/ensures. The caller
/// must pin target/layout on the supplied snapshot; this function never rewrites,
/// optimizes, executes or persists it. Unsupported inputs return only static gaps.
pub fn validate_pure_i64(
    source: &str,
    llvm_ir: &str,
    options: Options,
) -> Result<ValidationReport> {
    validate(source, llvm_ir, options, false)
}

/// Compare a zero-argument C03 main body with __mumei_user_main in the exact
/// supplied snapshot. Check only the call/trunc/ret shape of its C main wrapper;
/// no object, linking, OS exit-status or final-executable semantics are attested.
pub fn validate_native_main(
    source: &str,
    llvm_ir: &str,
    options: Options,
) -> Result<ValidationReport> {
    validate(source, llvm_ir, options, true)
}

fn validate(
    source: &str,
    llvm_ir: &str,
    options: Options,
    native: bool,
) -> Result<ValidationReport> {
    bounded(llvm_ir)?;
    if options.timeout_ms == 0 || options.timeout_ms > 30_000 || options.resource_limit == 0 {
        return Err(ValidationError("invalid solver budget"));
    }
    let mut config = Config::new();
    config.set_timeout_msec(options.timeout_ms as u64);
    config.set_param_value("rlimit", &options.resource_limit.to_string());
    let context = Context::new(&config);
    let model = encode_pure_i64_body(source, &context, true)
        .map_err(|_| ValidationError("unsupported C03 source"))?;
    if native && (model.function_name != "main" || !model.inputs.is_empty()) {
        return Err(ValidationError(
            "native profile requires zero-argument main",
        ));
    }
    let llvm_context = LlvmContext::create();
    let module = llvm_context
        .create_module_from_ir(MemoryBuffer::create_from_memory_range_copy(
            llvm_ir.as_bytes(),
            "translation-validation.ll",
        ))
        .map_err(|_| ValidationError("invalid LLVM IR"))?;
    module
        .verify()
        .map_err(|_| ValidationError("invalid LLVM module"))?;
    let target = match module.get_triple().as_str().to_bytes() {
        bytes if bytes == TARGET.as_bytes() => TARGET,
        bytes if native && bytes == NATIVE_TARGET.as_bytes() => NATIVE_TARGET,
        _ => return Err(ValidationError("unsupported target or data layout")),
    };
    if module.get_data_layout().as_str().to_bytes() != DATA_LAYOUT.as_bytes() {
        return Err(ValidationError("unsupported target or data layout"));
    }
    let function = module
        .get_function(if native {
            "__mumei_user_main"
        } else {
            &model.function_name
        })
        .ok_or(ValidationError("missing source function"))?;
    // The module is parsed/verified and remains alive for every raw LLVM query.
    let returns = unsafe {
        let raw = function.as_value_ref();
        if native {
            check_native_module(module.as_mut_ptr(), raw)?;
        } else {
            check_module(module.as_mut_ptr(), raw)?;
        }
        check_function(raw, model.inputs.len())?;
        let mut encoder = IrEncoder::new(&context);
        let returns = encoder.encode(raw, &model.inputs)?;
        (returns, encoder.definedness)
    };
    let (returns, ir_definedness) = returns;
    // Division, remainder and shifts bind both sides only where the operands
    // are defined — the same safety domain `--bitvec-i64` verification
    // requires. Inputs outside the source domain are don't-care; inputs the
    // source binds but the IR poisons on are a distinct failure.
    let src_dom = Bool::and(&context, &model.definedness.iter().collect::<Vec<_>>());
    let ir_terms: Vec<Bool> = ir_definedness
        .iter()
        .map(|(path, defined)| path.implies(defined))
        .collect();
    let ir_dom = Bool::and(&context, &ir_terms.iter().collect::<Vec<_>>());
    let mismatches: Vec<_> = returns
        .iter()
        .map(|(guard, value)| {
            Bool::and(
                &context,
                &[guard, &src_dom, &ir_dom, &value._eq(&model.value).not()],
            )
        })
        .collect();
    let solver = Solver::new(&context);
    let outcome = 'check: {
        // A source whose guarded operators have no defined input at all is a
        // spec `--bitvec-i64` verification would already reject; equivalence
        // over an empty domain would be vacuous.
        if !model.definedness.is_empty() {
            solver.assert(&src_dom);
            match solver.check() {
                SatResult::Sat => {}
                SatResult::Unsat => {
                    return Err(ValidationError(
                        "source division or shift defined on no inputs",
                    ))
                }
                SatResult::Unknown => break 'check Outcome::Inconclusive,
            }
            solver.reset();
        }
        solver.assert(&Bool::or(&context, &mismatches.iter().collect::<Vec<_>>()));
        match solver.check() {
            SatResult::Sat => break 'check Outcome::Different,
            SatResult::Unknown => break 'check Outcome::Inconclusive,
            SatResult::Unsat => {}
        }
        // Even with matching values, an instruction may still produce poison
        // on an input inside the source's defined domain.
        let ub: Vec<Bool> = ir_definedness
            .iter()
            .map(|(path, defined)| Bool::and(&context, &[path, &defined.not(), &src_dom]))
            .collect();
        solver.reset();
        solver.assert(&Bool::or(&context, &ub.iter().collect::<Vec<_>>()));
        match solver.check() {
            SatResult::Sat => Outcome::UndefinedBehavior,
            SatResult::Unknown => Outcome::Inconclusive,
            SatResult::Unsat => Outcome::EquivalentWithinProfile,
        }
    };
    Ok(ValidationReport {
        profile: if native { NATIVE_PROFILE } else { PROFILE },
        source_profile: PURE_I64_WRAPPING_PROFILE,
        target,
        data_layout: DATA_LAYOUT,
        checked_stage: if native {
            "native_user_body_and_truncating_wrapper_snapshot_no_pipeline_attestation"
        } else {
            "supplied_llvm_ir_snapshot_no_pipeline_attestation"
        },
        input_domain: if !model.definedness.is_empty() || !ir_definedness.is_empty() {
            if native {
                "zero_argument_main_i64_body_where_division_and_shift_defined_not_process_exit_status"
            } else {
                "signed_i64_inputs_where_division_and_shift_defined_not_restricted_by_requires"
            }
        } else if native {
            "zero_argument_main_i64_body_not_process_exit_status"
        } else {
            "all_signed_i64_inputs_not_restricted_by_requires"
        },
        source_sha256: model.source_sha256,
        llvm_sha256: mumei_core::native_bundle::sha256(llvm_ir.as_bytes()),
        timeout_ms: options.timeout_ms,
        resource_limit: options.resource_limit,
        outcome,
    })
}

unsafe fn width(ty: LLVMTypeRef) -> Result<u32> {
    if LLVMGetTypeKind(ty) != LLVMTypeKind::LLVMIntegerTypeKind {
        return Err(ValidationError("non-integer LLVM value"));
    }
    match LLVMGetIntTypeWidth(ty) {
        n @ (1 | 64) => Ok(n),
        _ => Err(ValidationError("unsupported LLVM width")),
    }
}

unsafe fn check_module_environment(module: LLVMModuleRef) -> Result<()> {
    let mut asm_size = 0;
    LLVMGetModuleInlineAsm(module, &mut asm_size);
    if asm_size != 0
        || !LLVMGetFirstNamedMetadata(module).is_null()
        || !LLVMGetFirstGlobal(module).is_null()
        || !LLVMGetFirstGlobalAlias(module).is_null()
        || !LLVMGetFirstGlobalIFunc(module).is_null()
    {
        return Err(ValidationError("module outside bounded snapshot profile"));
    }
    Ok(())
}

unsafe fn check_module(module: LLVMModuleRef, function: LLVMValueRef) -> Result<()> {
    check_module_environment(module)?;
    if LLVMGetFirstFunction(module) != function || !LLVMGetNextFunction(function).is_null() {
        return Err(ValidationError("module outside single-function profile"));
    }
    Ok(())
}

unsafe fn check_function_header(function: LLVMValueRef, params: usize) -> Result<()> {
    let mut metadata_count = 0;
    let metadata = LLVMGlobalCopyAllMetadata(function, &mut metadata_count);
    if !metadata.is_null() {
        LLVMDisposeValueMetadataEntries(metadata);
    }
    if metadata_count != 0
        || LLVMGetFunctionCallConv(function) != 0
        || LLVMHasPersonalityFn(function) != 0
        || !LLVMGetGC(function).is_null()
        || LLVMGetPointerAddressSpace(LLVMTypeOf(function)) != 0
        || LLVMCountParams(function) as usize != params
        || LLVMIsFunctionVarArg(LLVMGlobalGetValueType(function)) != 0
    {
        return Err(ValidationError("unsupported LLVM signature"));
    }
    for index in (0..=params as u32).chain(std::iter::once(u32::MAX)) {
        if LLVMGetAttributeCountAtIndex(function, index) != 0 {
            return Err(ValidationError("LLVM attributes unsupported"));
        }
    }
    Ok(())
}

unsafe fn check_function(function: LLVMValueRef, params: usize) -> Result<()> {
    check_function_header(function, params)?;
    if width(LLVMGetReturnType(LLVMGlobalGetValueType(function)))? != 64 {
        return Err(ValidationError("unsupported LLVM signature"));
    }
    if LLVMCountBasicBlocks(function) == 0 || LLVMCountBasicBlocks(function) as usize > MAX_BLOCKS {
        return Err(ValidationError("LLVM block limit"));
    }
    let mut block = LLVMGetFirstBasicBlock(function);
    let mut count = 0;
    let mut colors = HashMap::new();
    while !block.is_null() {
        acyclic(block, &mut colors)?;
        let mut inst = LLVMGetFirstInstruction(block);
        while !inst.is_null() {
            count += 1;
            if count > MAX_INSTRUCTIONS {
                return Err(ValidationError("LLVM instruction limit"));
            }
            if LLVMHasMetadata(inst) != 0 {
                return Err(ValidationError("LLVM metadata unsupported"));
            }
            for i in 0..LLVMGetNumOperands(inst) {
                let value = LLVMGetOperand(inst, i as u32);
                if LLVMIsUndef(value) != 0 || LLVMIsPoison(value) != 0 {
                    return Err(ValidationError("undef or poison unsupported"));
                }
                if LLVMValueIsBasicBlock(value) == 0 {
                    width(LLVMTypeOf(value))?;
                    if LLVMIsConstant(value) != 0 && LLVMIsAConstantInt(value).is_null() {
                        return Err(ValidationError("unsupported LLVM constant"));
                    }
                }
            }
            match LLVMGetInstructionOpcode(inst) {
                LLVMOpcode::LLVMRet | LLVMOpcode::LLVMBr => (),
                LLVMOpcode::LLVMAdd | LLVMOpcode::LLVMSub | LLVMOpcode::LLVMMul => {
                    unflagged_arithmetic(inst)?;
                    width(LLVMTypeOf(inst))?;
                }
                // Division, remainder and shifts are encoded against the
                // i64 source semantics; the i1 forms this whitelist also
                // admits for `add`/`sub`/`mul` have no counterpart there.
                LLVMOpcode::LLVMSDiv
                | LLVMOpcode::LLVMSRem
                | LLVMOpcode::LLVMShl
                | LLVMOpcode::LLVMAShr => {
                    unflagged_arithmetic(inst)?;
                    if width(LLVMTypeOf(inst))? != 64 {
                        return Err(ValidationError("non-i64 division or shift"));
                    }
                }
                LLVMOpcode::LLVMICmp
                | LLVMOpcode::LLVMAnd
                | LLVMOpcode::LLVMOr
                | LLVMOpcode::LLVMXor
                | LLVMOpcode::LLVMZExt
                | LLVMOpcode::LLVMTrunc
                | LLVMOpcode::LLVMSelect
                | LLVMOpcode::LLVMPHI => {
                    width(LLVMTypeOf(inst))?;
                }
                _ => return Err(ValidationError("unsupported LLVM instruction")),
            }
            inst = LLVMGetNextInstruction(inst);
        }
        block = LLVMGetNextBasicBlock(block);
    }
    Ok(())
}

/// `add`/`sub`/`mul` wrap mod 2^64 on both sides of the comparison — the
/// Mumei emitter never sets wrap flags. The nsw/nuw flags instead make
/// overflow produce poison, which this profile rejects wholesale, so a
/// flagged arithmetic instruction stays unsupported. `sdiv`/`shl`/`ashr`
/// can carry nsw/nuw (`shl`) or `exact`, which likewise strengthen the
/// poison domain, so they are rejected by the same whole-token check.
unsafe fn unflagged_arithmetic(inst: LLVMValueRef) -> Result<()> {
    let printed = LLVMPrintValueToString(inst);
    let text = std::ffi::CStr::from_ptr(printed)
        .to_string_lossy()
        .into_owned();
    LLVMDisposeMessage(printed);
    // Wrap flags print as whole tokens after the mnemonic (`add nsw i64`);
    // a substring test would also trip on SSA names like `%answer`.
    if text
        .split_whitespace()
        .any(|token| matches!(token, "nsw" | "nuw" | "exact"))
    {
        return Err(ValidationError("flagged LLVM arithmetic unsupported"));
    }
    Ok(())
}

unsafe fn check_native_module(module: LLVMModuleRef, user_main: LLVMValueRef) -> Result<()> {
    check_module_environment(module)?;
    let main = LLVMGetNamedFunction(module, c"main".as_ptr());
    if main.is_null() {
        return Err(ValidationError("missing native main wrapper"));
    }
    let mut function = LLVMGetFirstFunction(module);
    let mut count = 0;
    while !function.is_null() {
        if (function != user_main && function != main)
            || LLVMGetLinkage(function) != llvm_sys::LLVMLinkage::LLVMExternalLinkage
        {
            return Err(ValidationError(
                "module outside two-function native profile",
            ));
        }
        count += 1;
        function = LLVMGetNextFunction(function);
    }
    if count != 2 {
        return Err(ValidationError(
            "module outside two-function native profile",
        ));
    }
    check_function_header(main, 2)?;
    let is_i32 = |ty| {
        LLVMGetTypeKind(ty) == LLVMTypeKind::LLVMIntegerTypeKind && LLVMGetIntTypeWidth(ty) == 32
    };
    let argv_type = LLVMTypeOf(LLVMGetParam(main, 1));
    if !is_i32(LLVMGetReturnType(LLVMGlobalGetValueType(main)))
        || !is_i32(LLVMTypeOf(LLVMGetParam(main, 0)))
        || LLVMGetTypeKind(argv_type) != LLVMTypeKind::LLVMPointerTypeKind
        || LLVMGetPointerAddressSpace(argv_type) != 0
        || LLVMCountBasicBlocks(main) != 1
    {
        return Err(ValidationError("unsupported native wrapper signature"));
    }
    let call = LLVMGetFirstInstruction(LLVMGetEntryBasicBlock(main));
    if call.is_null() || LLVMGetInstructionOpcode(call) != LLVMOpcode::LLVMCall {
        return Err(ValidationError("unsupported native wrapper call"));
    }
    if LLVMGetCalledValue(call) != user_main
        || LLVMGetCalledFunctionType(call) != LLVMGlobalGetValueType(user_main)
        || LLVMGetNumArgOperands(call) != 0
        || LLVMGetNumOperands(call) != 1
        || LLVMGetInstructionCallConv(call) != 0
        || LLVMIsTailCall(call) != 0
        || LLVMGetCallSiteAttributeCount(call, 0) != 0
        || LLVMGetCallSiteAttributeCount(call, u32::MAX) != 0
    {
        return Err(ValidationError("unsupported native wrapper call"));
    }
    // LLVM 17's C API cannot enumerate bundles; empty bundles add no operands.
    // Its canonical plain-call suffix excludes them without constraining SSA names.
    let printed = LLVMPrintValueToString(call);
    let plain_call = std::ffi::CStr::from_ptr(printed)
        .to_bytes()
        .ends_with(b"call i64 @__mumei_user_main()");
    LLVMDisposeMessage(printed);
    if !plain_call {
        return Err(ValidationError("unsupported native wrapper call"));
    }
    let trunc = LLVMGetNextInstruction(call);
    if trunc.is_null()
        || LLVMGetInstructionOpcode(trunc) != LLVMOpcode::LLVMTrunc
        || LLVMGetOperand(trunc, 0) != call
        || !is_i32(LLVMTypeOf(trunc))
    {
        return Err(ValidationError("unsupported native wrapper truncation"));
    }
    let ret = LLVMGetNextInstruction(trunc);
    if ret.is_null()
        || LLVMGetInstructionOpcode(ret) != LLVMOpcode::LLVMRet
        || LLVMGetOperand(ret, 0) != trunc
        || !LLVMGetNextInstruction(ret).is_null()
        || [call, trunc, ret]
            .iter()
            .any(|&inst| LLVMHasMetadata(inst) != 0)
    {
        return Err(ValidationError("unsupported native wrapper return"));
    }
    Ok(())
}

unsafe fn acyclic(block: LLVMBasicBlockRef, colors: &mut HashMap<usize, u8>) -> Result<()> {
    match colors.get(&(block as usize)) {
        Some(1) => return Err(ValidationError("LLVM cycle unsupported")),
        Some(2) => return Ok(()),
        _ => (),
    }
    colors.insert(block as usize, 1);
    let term = LLVMGetBasicBlockTerminator(block);
    if term.is_null()
        || !matches!(
            LLVMGetInstructionOpcode(term),
            LLVMOpcode::LLVMBr | LLVMOpcode::LLVMRet
        )
    {
        return Err(ValidationError("unsupported LLVM terminator"));
    }
    for i in 0..LLVMGetNumSuccessors(term) {
        acyclic(LLVMGetSuccessor(term, i), colors)?;
    }
    colors.insert(block as usize, 2);
    Ok(())
}

struct IrEncoder<'ctx> {
    ctx: &'ctx Context,
    states: usize,
    /// `(path condition, defined)` obligations for `sdiv`/`srem`/`shl`/`ashr`:
    /// the instruction's operands must satisfy the condition wherever the
    /// path executes it, or the IR produces poison there.
    definedness: Vec<(Bool<'ctx>, Bool<'ctx>)>,
}
impl<'ctx> IrEncoder<'ctx> {
    fn new(ctx: &'ctx Context) -> Self {
        Self {
            ctx,
            states: 0,
            definedness: Vec::new(),
        }
    }

    unsafe fn value(
        &self,
        value: LLVMValueRef,
        values: &HashMap<usize, BV<'ctx>>,
    ) -> Result<BV<'ctx>> {
        let bits = width(LLVMTypeOf(value))?;
        if LLVMIsUndef(value) != 0 || LLVMIsPoison(value) != 0 {
            return Err(ValidationError("undef or poison unsupported"));
        }
        if !LLVMIsAConstantInt(value).is_null() {
            return Ok(BV::from_u64(
                self.ctx,
                LLVMConstIntGetZExtValue(value),
                bits,
            ));
        }
        values
            .get(&(value as usize))
            .cloned()
            .ok_or(ValidationError("unresolved LLVM value"))
    }

    fn condition(&self, value: &BV<'ctx>) -> Result<Bool<'ctx>> {
        if value.get_size() != 1 {
            return Err(ValidationError("LLVM i1 condition expected"));
        }
        Ok(value._eq(&BV::from_u64(self.ctx, 1, 1)))
    }

    unsafe fn encode(
        &mut self,
        function: LLVMValueRef,
        inputs: &[BV<'ctx>],
    ) -> Result<Vec<(Bool<'ctx>, BV<'ctx>)>> {
        let mut values = HashMap::new();
        for (i, input) in inputs.iter().enumerate() {
            let param = LLVMGetParam(function, i as u32);
            if width(LLVMTypeOf(param))? != 64 {
                return Err(ValidationError("LLVM i64 parameter expected"));
            }
            values.insert(param as usize, input.clone());
        }
        let mut returns = Vec::new();
        self.block(
            LLVMGetEntryBasicBlock(function),
            std::ptr::null_mut(),
            values,
            Bool::from_bool(self.ctx, true),
            &mut returns,
        )?;
        if returns.is_empty() {
            return Err(ValidationError("missing LLVM return"));
        }
        Ok(returns)
    }

    unsafe fn block(
        &mut self,
        block: LLVMBasicBlockRef,
        previous: LLVMBasicBlockRef,
        mut values: HashMap<usize, BV<'ctx>>,
        guard: Bool<'ctx>,
        returns: &mut Vec<(Bool<'ctx>, BV<'ctx>)>,
    ) -> Result<()> {
        self.states += 1;
        if self.states > MAX_STATES {
            return Err(ValidationError("LLVM path expansion limit"));
        }
        let mut inst = LLVMGetFirstInstruction(block);
        while !inst.is_null() {
            let op = LLVMGetInstructionOpcode(inst);
            if op == LLVMOpcode::LLVMRet {
                let value = self.value(LLVMGetOperand(inst, 0), &values)?;
                returns.push((guard, value));
                return Ok(());
            }
            if op == LLVMOpcode::LLVMBr {
                if LLVMGetNumSuccessors(inst) == 1 {
                    return self.block(LLVMGetSuccessor(inst, 0), block, values, guard, returns);
                }
                let cond = self.condition(&self.value(LLVMGetCondition(inst), &values)?)?;
                self.block(
                    LLVMGetSuccessor(inst, 0),
                    block,
                    values.clone(),
                    Bool::and(self.ctx, &[&guard, &cond]),
                    returns,
                )?;
                return self.block(
                    LLVMGetSuccessor(inst, 1),
                    block,
                    values,
                    Bool::and(self.ctx, &[&guard, &cond.not()]),
                    returns,
                );
            }
            let value = if op == LLVMOpcode::LLVMPHI {
                let i = (0..LLVMCountIncoming(inst))
                    .find(|&i| LLVMGetIncomingBlock(inst, i) == previous)
                    .ok_or(ValidationError("missing LLVM phi edge"))?;
                self.value(LLVMGetIncomingValue(inst, i), &values)?
            } else {
                let a = self.value(LLVMGetOperand(inst, 0), &values)?;
                match op {
                    LLVMOpcode::LLVMZExt => {
                        if a.get_size() != 1 || width(LLVMTypeOf(inst))? != 64 {
                            return Err(ValidationError("unsupported LLVM cast"));
                        }
                        a.zero_ext(63)
                    }
                    LLVMOpcode::LLVMTrunc => {
                        if a.get_size() != 64 || width(LLVMTypeOf(inst))? != 1 {
                            return Err(ValidationError("unsupported LLVM cast"));
                        }
                        a.extract(0, 0)
                    }
                    LLVMOpcode::LLVMSelect => {
                        let b = self.value(LLVMGetOperand(inst, 1), &values)?;
                        let c = self.value(LLVMGetOperand(inst, 2), &values)?;
                        self.condition(&a)?.ite(&b, &c)
                    }
                    _ => {
                        let b = self.value(LLVMGetOperand(inst, 1), &values)?;
                        match op {
                            LLVMOpcode::LLVMAnd => a.bvand(&b),
                            LLVMOpcode::LLVMOr => a.bvor(&b),
                            LLVMOpcode::LLVMXor => a.bvxor(&b),
                            LLVMOpcode::LLVMAdd => a.bvadd(&b),
                            LLVMOpcode::LLVMSub => a.bvsub(&b),
                            LLVMOpcode::LLVMMul => a.bvmul(&b),
                            LLVMOpcode::LLVMSDiv | LLVMOpcode::LLVMSRem => {
                                // `sdiv`/`srem` are poison for a zero
                                // divisor and for i64::MIN / -1 — the same
                                // partial domain the source side records.
                                let zero = BV::from_i64(self.ctx, 0, 64);
                                let min = BV::from_i64(self.ctx, i64::MIN, 64);
                                let minus_one = BV::from_i64(self.ctx, -1, 64);
                                let defined = Bool::and(
                                    self.ctx,
                                    &[
                                        &b._eq(&zero).not(),
                                        &Bool::and(self.ctx, &[&a._eq(&min), &b._eq(&minus_one)])
                                            .not(),
                                    ],
                                );
                                self.definedness.push((guard.clone(), defined));
                                if op == LLVMOpcode::LLVMSDiv {
                                    a.bvsdiv(&b)
                                } else {
                                    a.bvsrem(&b)
                                }
                            }
                            LLVMOpcode::LLVMShl | LLVMOpcode::LLVMAShr => {
                                // `shl`/`ashr` are poison for amounts >= 64
                                // (unsigned comparison covers negatives).
                                let max = BV::from_i64(self.ctx, 64, 64);
                                self.definedness.push((guard.clone(), b.bvult(&max)));
                                if op == LLVMOpcode::LLVMShl {
                                    a.bvshl(&b)
                                } else {
                                    a.bvashr(&b)
                                }
                            }
                            LLVMOpcode::LLVMICmp => {
                                use LLVMIntPredicate::*;
                                let condition = match LLVMGetICmpPredicate(inst) {
                                    LLVMIntEQ => a._eq(&b),
                                    LLVMIntNE => a._eq(&b).not(),
                                    LLVMIntSGT => a.bvsgt(&b),
                                    LLVMIntSGE => a.bvsge(&b),
                                    LLVMIntSLT => a.bvslt(&b),
                                    LLVMIntSLE => a.bvsle(&b),
                                    LLVMIntUGT => a.bvugt(&b),
                                    LLVMIntUGE => a.bvuge(&b),
                                    LLVMIntULT => a.bvult(&b),
                                    LLVMIntULE => a.bvule(&b),
                                };
                                condition.ite(
                                    &BV::from_u64(self.ctx, 1, 1),
                                    &BV::from_u64(self.ctx, 0, 1),
                                )
                            }
                            _ => return Err(ValidationError("unsupported LLVM value instruction")),
                        }
                    }
                }
            };
            values.insert(inst as usize, value);
            inst = LLVMGetNextInstruction(inst);
        }
        Err(ValidationError("missing LLVM terminator"))
    }
}
