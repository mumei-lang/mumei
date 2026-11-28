use inkwell::targets::{InitializationConfig, Target};
use llvm_sys::disassembler::{
    LLVMCreateDisasm, LLVMDisasmContextRef, LLVMDisasmDispose, LLVMDisasmInstruction,
};
use mumei_core::semantic_contract::{CallSite, CallValueShape, PureI64Body, LOOP_BOUND};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{CStr, CString},
    os::raw::c_char,
    ptr,
    sync::Once,
};
use z3::{
    ast::{Array, Ast, Bool, BV},
    Context, Params, SatResult, Solver, Sort, Tactic,
};

pub const TIMEOUT_MS: u32 = 5_000;
pub const RESOURCE_LIMIT: u32 = 1_000_000;
pub const MAX_ARGUMENT_COUNT: usize = 16;

static INITIALIZE_X86: Once = Once::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instruction {
    pub offset: u64,
    pub mnemonic: String,
    pub operands: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisassemblyError;

impl From<()> for DisassemblyError {
    fn from(_: ()) -> Self {
        Self
    }
}

impl std::fmt::Display for DisassemblyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("x86-64 disassembly failed")
    }
}

impl std::error::Error for DisassemblyError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionOutcome {
    Equivalent,
    Different,
    Unknown,
    OutsideProfile(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalleeSavedPreservation {
    Preserved,
    Violated,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResultWidth {
    I64,
    I32,
    F64,
    I128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallTarget {
    pub name: String,
    pub relocation_kind: CallRelocationKind,
    pub relocation_size: u8,
    pub addend: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallRelocationKind {
    ElfX86_64Plt32,
    ElfX86_64Pc32,
    Other,
}

impl CallTarget {
    fn is_entry_target_relocation(&self) -> bool {
        self.relocation_size == 32
            && self.addend == -4
            && matches!(
                self.relocation_kind,
                CallRelocationKind::ElfX86_64Plt32 | CallRelocationKind::ElfX86_64Pc32
            )
    }
}

struct Disassembler(LLVMDisasmContextRef);

impl Drop for Disassembler {
    fn drop(&mut self) {
        unsafe { LLVMDisasmDispose(self.0) };
    }
}

fn create_disassembler() -> Result<Disassembler, ()> {
    INITIALIZE_X86.call_once(|| {
        Target::initialize_x86(&InitializationConfig::default());
    });
    let triple = CString::new("x86_64-unknown-linux-gnu").map_err(|_| ())?;
    let context = unsafe { LLVMCreateDisasm(triple.as_ptr(), ptr::null_mut(), 0, None, None) };
    if context.is_null() {
        Err(())
    } else {
        Ok(Disassembler(context))
    }
}

pub fn disassemble(bytes: &[u8], address: u64) -> Result<Vec<Instruction>, DisassemblyError> {
    let disassembler = create_disassembler()?;
    let mut code = bytes.to_vec();
    let mut offset = 0usize;
    let mut instructions = Vec::new();
    while offset < code.len() {
        let mut output = [0 as c_char; 256];
        let pc = address
            .checked_add(u64::try_from(offset).map_err(|_| ())?)
            .ok_or(())?;
        let consumed = unsafe {
            LLVMDisasmInstruction(
                disassembler.0,
                code[offset..].as_mut_ptr(),
                u64::try_from(code.len() - offset).map_err(|_| ())?,
                pc,
                output.as_mut_ptr(),
                output.len(),
            )
        };
        if consumed == 0 {
            return Err(DisassemblyError);
        }
        let text = unsafe { CStr::from_ptr(output.as_ptr()) }
            .to_str()
            .map_err(|_| ())?
            .trim();
        let (mnemonic, operands) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
        instructions.push(Instruction {
            offset: u64::try_from(offset).map_err(|_| ())?,
            mnemonic: mnemonic.to_owned(),
            operands: operands.trim().to_owned(),
        });
        offset = offset.checked_add(consumed).ok_or(())?;
    }
    Ok(instructions)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchShape {
    None,
    Forward,
    AcyclicBackward,
}

pub fn branch_shape(
    bytes: &[u8],
    address: u64,
    call_targets: &BTreeMap<u64, CallTarget>,
) -> BranchShape {
    let Ok(instructions) = disassemble(bytes, address) else {
        return BranchShape::None;
    };
    if instructions.is_empty() {
        return BranchShape::None;
    }
    let function_size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    let Ok(branch_targets) = resolve_branch_targets(&instructions, function_size, call_targets)
    else {
        return BranchShape::None;
    };
    let successors = control_flow_successors(&instructions, &branch_targets, call_targets);
    let reachable = reachable_instructions(&successors);
    if reachable_control_flow_has_cycle(&successors, &reachable) {
        return BranchShape::None;
    }
    let mut has_branch = false;
    let mut has_backward_edge = false;
    for (index, is_reachable) in reachable.iter().enumerate() {
        if *is_reachable {
            if let Some(target) = branch_targets.get(&index) {
                has_branch = true;
                has_backward_edge |= *target < index;
            }
        }
    }
    if !has_branch {
        BranchShape::None
    } else if has_backward_edge {
        BranchShape::AcyclicBackward
    } else {
        BranchShape::Forward
    }
}

pub fn has_conditional_branch_and_call(
    bytes: &[u8],
    address: u64,
    call_targets: &BTreeMap<u64, CallTarget>,
) -> bool {
    let Ok(instructions) = disassemble(bytes, address) else {
        return false;
    };
    instructions
        .iter()
        .any(|instruction| is_conditional_branch(&instruction.mnemonic))
        && instructions.iter().any(|instruction| {
            instruction.mnemonic == "callq" || is_tail_call(instruction, call_targets)
        })
}

pub fn check_function<'ctx>(
    bytes: &[u8],
    address: u64,
    model: &PureI64Body<'ctx>,
    call_targets: &BTreeMap<u64, CallTarget>,
    context: &'ctx Context,
) -> FunctionOutcome {
    check_function_with_preservation(bytes, address, model, call_targets, context).0
}

/// Check return-value equivalence and, when equivalent, prove callee-saved
/// preservation. Calls assume callees preserve these registers; this is
/// compositional for source callees only when each is independently proved
/// preserving, while external and cancellation-poll callees remain assumptions.
pub fn check_function_with_preservation<'ctx>(
    bytes: &[u8],
    address: u64,
    model: &PureI64Body<'ctx>,
    call_targets: &BTreeMap<u64, CallTarget>,
    context: &'ctx Context,
) -> (FunctionOutcome, Option<CalleeSavedPreservation>) {
    let (outcome, preservation, _) =
        check_function_with_frame_memory(bytes, address, model, call_targets, context);
    (outcome, preservation)
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct FrameMemoryUse {
    pub red_zone: bool,
    pub allocated: bool,
}

/// Borrowed LLVM-IR encoding used as the expected value in the
/// scalar-i64 machine-TV comparison (Step 3a): `(path_condition,
/// return_value)` edges plus `(path_condition, defined)` definedness
/// obligations over the shared `model.inputs`.
type ExpectedIr<'a, 'ctx> = (&'a [(Bool<'ctx>, BV<'ctx>)], &'a [(Bool<'ctx>, Bool<'ctx>)]);

/// Verdict of the direct LLVM-IR ↔ object-function comparison used by
/// the `llvm_ir_object_equivalence` correspondence item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrObjectEquivalence {
    Equivalent,
    Different,
    /// Outside the comparison's narrower subset, or inconclusive — the
    /// caller records the function as not liftable (fail-closed).
    NotLiftable(&'static str),
}

/// Whether a source model's signature is inside the direct
/// IR ↔ object comparison's subset (design memo §7 Step 3
/// slices): scalar parameters (one 64-bit leaf, integer or f64
/// bit-pattern), scalar or all-integer aggregate results
/// (register-resident or `sret` memory-class), flat mixed
/// `i64`/`double` register pairs, scalar-i64 call sites, and
/// bounded memory — no array params, FP arithmetic, or loops.
/// Admission is semantic, not only inspection: a signature inside this
/// subset makes `evaluate_machine_tv` compare the lifted machine code
/// against the IR encoding instead of the source model, and the
/// equivalence outcome feeds the same object-acceptance record.
/// Callers use this to record `skipped` without paying for an IR
/// encode; `check_function_against_ir` re-checks it internally.
pub fn ir_object_signature_liftable(model: &PureI64Body) -> bool {
    // A scalar result's leaf budget is its value width: `i64` is one
    // leaf, a scalar `i128`/`u128` (`pure-i64-composite/v21`) carries
    // two 64-bit leaves and comes back in `rdx:rax`.
    let result_leaf_count = model
        .result_shape
        .unwrap_or((model.value.get_size() / 64) as usize);
    model.inputs.len() <= MAX_ARGUMENT_COUNT
        && model.inputs.iter().all(|input| input.get_size() == 64)
        // Integer-leaf aggregate results enter the subset: up to the
        // register-resident budget (rax/rdx/rcx) directly, and beyond
        // it through `sret` — the callee stores leaf `i` at byte
        // offset `8 * i` through the hidden result pointer, which the
        // lifter already records in `sret_stores`.
        && model.result_shape.is_none_or(|count| {
            count <= INTEGER_RESULT_REGISTERS.len()
                || sret_result_leaf_count(model) == Some(count)
        })
        // A 128-bit aggregate field stays indistinguishable here —
        // `{i128}` and `{i64, i64}` both flatten to two `[false]`
        // slots — so the encode side checks exact field types and
        // enforces flatness for mixed register pairs. A scalar result
        // admits either leaf kind: f64 compares its IEEE-754 bit pattern
        // (pass-through/select only — FP arithmetic stays gated out
        // by `f64_arithmetic: false` on the encode call), and an
        // i128 result (`[false, false]` on a `None` shape, only
        // produced when the module enables wide scalars) compares as
        // its full 128-bit edge. An omitted return annotation leaves
        // the list empty — an inferred-i64 body still compares; a
        // genuinely void atom is already excluded by the source
        // signature check upstream.
        && match model.result_shape {
            Some(count) => {
                model.result_leaf_doubles.iter().all(|leaf| !*leaf)
                    || (count == 2
                        && matches!(model.result_leaf_doubles.as_slice(), [false, true] | [true, false]))
            }
            None => true,
        }
        && model.result_leaf_doubles.len() <= result_leaf_count
        && (model.result_shape.is_some()
            || model.result_leaf_doubles.len() <= 1
            || (model.has_wide_scalar_features
                && model.result_leaf_doubles.len() == 2
                && model.value.get_size() == 128))
        // Every parameter a scalar leaf: one 64-bit leaf of either
        // class — integer (`[false]`) or f64 bit-pattern (`[true]`,
        // bound to an xmm register on the machine side and pinned to
        // a `double` IR parameter by the shared leaf-kind check) —
        // or the two 64-bit leaves an `i128`/`u128` scalar flattens
        // to (`pure-i64-composite/v21`, `lo` then `hi`). Composite
        // parameters stay out of the subset.
        && model.param_leaf_doubles.iter().all(|leaves| {
            leaves.len() == 1
                || (model.has_wide_scalar_features && leaves.as_slice() == [false, false])
        })
        && model.param_shapes.iter().all(Option::is_none)
        && model.array_params.iter().all(Option::is_none)
        && model.call_sites.iter().all(scalar_i64_call_site)
        && model.loop_bound_exits.is_none()
    // Bounded memory (local `let a = [...]` cells) stays inside:
    // the shared IR encoder models alloca/GEP/store/load over Z3
    // arrays with in-bounds definedness obligations, and the
    // frame-memory comparison asserts the same return-value edges
    // (Step 3c of the expansion order).
}

/// Whether a source call site stays inside the direct comparison's
/// scalar-i64 subset: every argument and the result are single
/// non-double 64-bit leaves, with no composite shape or callback
/// parameter. Both sides encode such a call as the same
/// `call_decl(callee, arity)` uninterpreted application over
/// `model.inputs`, and the lifter's `call_argument_obligations` check
/// the machine's actual arguments against the source ones — the
/// memo's arg-match obligation (design memo §3.1).
fn scalar_i64_call_site(site: &CallSite) -> bool {
    site.callback_arguments.is_empty()
        && !site.has_composite_arguments_or_result
        && matches!(site.result_shape, CallValueShape::Scalar { leaf_count: 1 })
        && site.result.get_size() == 64
        && !site.result_is_double
        && site.result_leaf_doubles.as_slice() == [false]
        && site.arg_leaf_doubles.len() == site.args.len()
        && site.arg_leaf_doubles.iter().all(|is_double| !*is_double)
        && site
            .argument_shapes
            .iter()
            .all(|shape| matches!(shape, CallValueShape::Scalar { leaf_count: 1 }))
        && site.argument_shapes.len() == site.args.len()
}

/// Compare an object function's lifted paths directly against its
/// LLVM-IR encoding over shared `model.inputs` (design memo §7 Step 3
/// slice). `ir_returns` is the encoder's `(path condition, return
/// value)` edge list and `ir_definedness` its `(path condition,
/// defined)` domain obligations, both produced by
/// `translation_validation::encode_ir_function_for_comparison` on the
/// same Z3 context.
///
/// Scoped to 64-bit scalar signatures (integer and f64
/// bit-pattern leaves — f64 compares pass-through/select only, FP
/// arithmetic is gated out by `f64_arithmetic: false` on the encode
/// call; plus scalar `i128`/`u128` on wide-scalar modules, compared
/// as one 128-bit `rdx:rax` edge) and all-`i64` aggregate results
/// (register-resident or `sret` memory-class), plus flat mixed
/// `i64`/`double` register pairs: other aggregate f64 results,
/// composite or wider-than-128 inputs, non-scalar call sites,
/// and loops return `NotLiftable`. For
/// every lifted path the machine return must equal the IR return of
/// some covering edge whose definedness obligations hold and whose
/// call-argument obligations all hold; an uncovered path, a diverging
/// edge, or a failed obligation each compare `Different`.
pub fn check_function_against_ir<'ctx>(
    bytes: &[u8],
    address: u64,
    model: &PureI64Body<'ctx>,
    ir_returns: &[(Bool<'ctx>, BV<'ctx>)],
    ir_definedness: &[(Bool<'ctx>, Bool<'ctx>)],
    call_targets: &BTreeMap<u64, CallTarget>,
    context: &'ctx Context,
) -> IrObjectEquivalence {
    if !ir_object_signature_liftable(model) {
        return IrObjectEquivalence::NotLiftable("unsupported_signature");
    }
    let result_leaf_count = model.result_shape.unwrap_or(1);
    // A scalar i128 returns as one `rdx:rax` bit-vector edge, not a
    // 64-bit leaf count — size the IR edge off the declared shape or
    // the scalar value width.
    let result_bits = model
        .result_shape
        .map_or_else(|| model.value.get_size(), |count| 64 * count as u32);
    if ir_returns.is_empty()
        || ir_returns
            .iter()
            .any(|(_, value)| value.get_size() != result_bits)
    {
        return IrObjectEquivalence::NotLiftable("unsupported_ir");
    }
    let instructions = match disassemble(bytes, address) {
        Ok(instructions) => instructions,
        Err(_) => return IrObjectEquivalence::NotLiftable("unsupported_instruction"),
    };
    let function_size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    let lifted = match lift(&instructions, function_size, model, call_targets, context) {
        Ok(lifted) => lifted,
        Err(FunctionOutcome::Unknown) => return IrObjectEquivalence::NotLiftable("solver_unknown"),
        Err(FunctionOutcome::OutsideProfile(reason)) => {
            return IrObjectEquivalence::NotLiftable(reason);
        }
        Err(_) => return IrObjectEquivalence::NotLiftable("lift_failed"),
    };
    if lifted.paths.is_empty() {
        return IrObjectEquivalence::NotLiftable("no_liftable_path");
    }
    let substitutions = input_substitutions(model, &lifted.arguments);
    for path in &lifted.paths {
        if path.return_values.len() != result_leaf_count {
            return IrObjectEquivalence::NotLiftable("unsupported_signature");
        }
        let machine_return = aggregate_return_value(&path.return_values).substitute(&substitutions);
        let coverage: Vec<Bool> = ir_returns
            .iter()
            .map(|(condition, _)| condition.substitute(&substitutions))
            .collect();
        let coverage_refs = coverage.iter().collect::<Vec<_>>();
        let mut mismatches = vec![Bool::or(context, &coverage_refs).not()];
        for (condition, value) in ir_returns {
            let condition = condition.substitute(&substitutions);
            let value = value.substitute(&substitutions);
            mismatches.push(Bool::and(
                context,
                &[&condition, &machine_return._eq(&value).not()],
            ));
        }
        for (condition, defined) in ir_definedness {
            let condition = condition.substitute(&substitutions);
            let defined = defined.substitute(&substitutions);
            mismatches.push(Bool::and(context, &[&condition, &defined.not()]));
        }
        // A machine call whose actual arguments do not match the
        // source call site's expected ones is a mismatch — the
        // arg-match obligation (design memo §3.1). Scalar-i64 paths
        // carry no sret or exact-f64 obligations, but they are
        // asserted for completeness.
        mismatches.extend(
            path.call_argument_obligations
                .iter()
                .chain(&path.exact_f64_call_argument_obligations)
                .map(|obligation| obligation.substitute(&substitutions).not()),
        );
        if let Some(obligation) = &path.sret_pointer_obligation {
            mismatches.push(obligation.substitute(&substitutions).not());
        }
        let mismatches_refs = mismatches.iter().collect::<Vec<_>>();
        let result = check_query_with_simplify_fallback(context, |solver| {
            assert_path_assumptions(solver, model, &lifted.arguments, path);
            solver.assert(&Bool::or(context, &mismatches_refs));
        });
        match result {
            SatResult::Unsat => {}
            SatResult::Sat => return IrObjectEquivalence::Different,
            SatResult::Unknown => return IrObjectEquivalence::NotLiftable("solver_unknown"),
        }
    }
    IrObjectEquivalence::Equivalent
}

pub fn check_function_with_frame_memory<'ctx>(
    bytes: &[u8],
    address: u64,
    model: &PureI64Body<'ctx>,
    call_targets: &BTreeMap<u64, CallTarget>,
    context: &'ctx Context,
) -> (
    FunctionOutcome,
    Option<CalleeSavedPreservation>,
    FrameMemoryUse,
) {
    check_function_with_result_width(
        bytes,
        address,
        model,
        call_targets,
        context,
        ResultWidth::I64,
        None,
    )
}

/// Like `check_function_with_frame_memory` but the expected value is
/// the LLVM-IR encoding — `ir_returns` edges and `ir_definedness`
/// obligations over the shared `model.inputs` from
/// `translation_validation::encode_ir_function_for_comparison` —
/// instead of the source model's `model.value` (design memo §7 Step
/// 3a). Scoped to 64-bit scalar signatures (plus scalar `i128`/`u128`
/// on wide-scalar modules, compared as one 128-bit `rdx:rax` edge)
/// and all-`i64` aggregate results (register-resident or `sret`
/// memory-class), plus flat mixed `i64`/`double` register pairs:
/// any other shape reports `OutsideProfile`.
pub fn check_function_with_frame_memory_against_ir<'ctx>(
    bytes: &[u8],
    address: u64,
    model: &PureI64Body<'ctx>,
    ir_returns: &[(Bool<'ctx>, BV<'ctx>)],
    ir_definedness: &[(Bool<'ctx>, Bool<'ctx>)],
    call_targets: &BTreeMap<u64, CallTarget>,
    context: &'ctx Context,
) -> (
    FunctionOutcome,
    Option<CalleeSavedPreservation>,
    FrameMemoryUse,
) {
    if !ir_object_signature_liftable(model) {
        return (
            FunctionOutcome::OutsideProfile("unsupported_signature"),
            None,
            FrameMemoryUse::default(),
        );
    }
    let result_bits = model
        .result_shape
        .map_or_else(|| model.value.get_size(), |count| 64 * count as u32);
    if ir_returns.is_empty()
        || ir_returns
            .iter()
            .any(|(_, value)| value.get_size() != result_bits)
    {
        return (
            FunctionOutcome::OutsideProfile("unsupported_ir"),
            None,
            FrameMemoryUse::default(),
        );
    }
    check_function_with_result_width(
        bytes,
        address,
        model,
        call_targets,
        context,
        ResultWidth::I64,
        Some((ir_returns, ir_definedness)),
    )
}

/// Check the C entry wrapper's low-32-bit result. Its call model assumes
/// callees preserve these registers; external and cancellation-poll callees
/// remain assumptions.
pub fn check_entry_wrapper_with_preservation(
    bytes: &[u8],
    address: u64,
    call_targets: &BTreeMap<u64, CallTarget>,
    context: &Context,
) -> (FunctionOutcome, Option<CalleeSavedPreservation>) {
    let value = BV::fresh_const(context, "entry_wrapper_user_main_result", 64);
    let mut model = PureI64Body::synthetic_scalar("main", value.clone());
    model.result_leaf_doubles = vec![false];
    model.call_sites.push(CallSite {
        callee: "main".to_owned(),
        args: Vec::new(),
        callback_arguments: Vec::new(),
        arg_leaf_doubles: Vec::new(),
        argument_shapes: Vec::new(),
        has_composite_arguments_or_result: false,
        result: value,
        result_is_double: false,
        result_store: None,
        result_shape: CallValueShape::Scalar { leaf_count: 1 },
        result_leaf_doubles: vec![false],
        guard: Bool::from_bool(context, true),
    });
    let (outcome, preservation, _) = check_function_with_result_width(
        bytes,
        address,
        &model,
        call_targets,
        context,
        ResultWidth::I32,
        None,
    );
    (outcome, preservation)
}

fn check_function_with_result_width<'ctx>(
    bytes: &[u8],
    address: u64,
    model: &PureI64Body<'ctx>,
    call_targets: &BTreeMap<u64, CallTarget>,
    context: &'ctx Context,
    result_width: ResultWidth,
    expected_ir: Option<ExpectedIr<'_, 'ctx>>,
) -> (
    FunctionOutcome,
    Option<CalleeSavedPreservation>,
    FrameMemoryUse,
) {
    if model.inputs.len() > MAX_ARGUMENT_COUNT {
        return (
            FunctionOutcome::OutsideProfile("argument_count"),
            None,
            FrameMemoryUse::default(),
        );
    }
    if model
        .call_sites
        .iter()
        .any(call_site_has_unsupported_composite_shape)
    {
        return (
            FunctionOutcome::OutsideProfile("call_arguments"),
            None,
            FrameMemoryUse::default(),
        );
    }
    if model
        .call_sites
        .iter()
        .any(|site| site.args.len() > MAX_ARGUMENT_COUNT)
    {
        return (
            FunctionOutcome::OutsideProfile("argument_count"),
            None,
            FrameMemoryUse::default(),
        );
    }
    let result_width = if result_width == ResultWidth::I64 && model_result_is_double(model) {
        ResultWidth::F64
    } else if result_width == ResultWidth::I64
        && model.result_shape.is_none()
        && model.value.get_size() == 128
    {
        ResultWidth::I128
    } else {
        result_width
    };
    if !supports_signature(model) {
        if let Ok(instructions) = disassemble(bytes, address) {
            let function_size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
            if function_has_reachable_cycle(&instructions, function_size, call_targets) {
                return (
                    FunctionOutcome::OutsideProfile("cyclic_control_flow"),
                    None,
                    FrameMemoryUse::default(),
                );
            }
        }
        return (
            FunctionOutcome::OutsideProfile("unsupported_signature"),
            None,
            FrameMemoryUse::default(),
        );
    }
    let instructions = match disassemble(bytes, address) {
        Ok(instructions) if !instructions.is_empty() => instructions,
        _ => {
            return (
                FunctionOutcome::OutsideProfile("unsupported_instruction"),
                None,
                FrameMemoryUse::default(),
            )
        }
    };
    let lifted = match lift(
        &instructions,
        u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        model,
        call_targets,
        context,
    ) {
        Ok(lifted) => lifted,
        Err(outcome) => return (outcome, None, FrameMemoryUse::default()),
    };
    // The IR-edge mode covers scalar-i64 and register-resident integer
    // aggregate signatures — enforced by
    // `check_function_with_frame_memory_against_ir`, the only caller
    // that passes `expected_ir` — with every return leaf modelled.
    let mut unknown_path = false;
    let mut unsupported_call_arguments = false;
    for path in &lifted.paths {
        if expected_ir.is_some() && path.return_values.len() != model.result_shape.unwrap_or(1) {
            return (
                FunctionOutcome::OutsideProfile("unsupported_signature"),
                None,
                FrameMemoryUse::default(),
            );
        }
        let result = check_query_with_simplify_fallback(context, |solver| {
            assert_path_equivalence_query(
                solver,
                model,
                &lifted.arguments,
                path,
                result_width,
                expected_ir,
                context,
            );
        });
        match result {
            SatResult::Unsat => {
                if !path.exact_f64_call_argument_obligations.is_empty() {
                    let exact_arguments = check_query_with_simplify_fallback(context, |solver| {
                        assert_path_exact_f64_call_arguments_query(
                            solver,
                            model,
                            &lifted.arguments,
                            path,
                            context,
                        );
                    });
                    if matches!(exact_arguments, SatResult::Sat | SatResult::Unknown) {
                        unsupported_call_arguments = true;
                    }
                }
            }
            SatResult::Sat => return (FunctionOutcome::Different, None, FrameMemoryUse::default()),
            SatResult::Unknown => unknown_path = true,
        }
    }
    if unsupported_call_arguments {
        return (
            FunctionOutcome::OutsideProfile("call_arguments"),
            None,
            FrameMemoryUse::default(),
        );
    }
    if unknown_path {
        return (FunctionOutcome::Unknown, None, FrameMemoryUse::default());
    }

    let mut preservation_violated = false;
    let mut preservation_unknown = false;
    for path in &lifted.paths {
        let result = check_query_with_simplify_fallback(context, |solver| {
            assert_path_callee_saved_preservation_query(
                solver,
                model,
                &lifted.arguments,
                &lifted.callee_saved_initial,
                path,
                context,
            );
        });
        match result {
            SatResult::Unsat => {}
            SatResult::Sat => preservation_violated = true,
            SatResult::Unknown => preservation_unknown = true,
        }
    }
    let preservation = if preservation_violated {
        CalleeSavedPreservation::Violated
    } else if preservation_unknown {
        CalleeSavedPreservation::Unknown
    } else {
        CalleeSavedPreservation::Preserved
    };
    (
        FunctionOutcome::Equivalent,
        Some(preservation),
        lifted.frame_memory_use,
    )
}

pub fn function_has_reachable_cycle(
    instructions: &[Instruction],
    function_size: u64,
    call_targets: &BTreeMap<u64, CallTarget>,
) -> bool {
    let Ok(branch_targets) = resolve_branch_targets(instructions, function_size, call_targets)
    else {
        return false;
    };
    let successors = control_flow_successors(instructions, &branch_targets, call_targets);
    let reachable = reachable_instructions(&successors);
    reachable_control_flow_has_cycle(&successors, &reachable)
}

pub fn supports_signature(model: &PureI64Body<'_>) -> bool {
    if model.array_params.iter().any(Option::is_some) || model.array_result.is_some() {
        return false;
    }
    let parameter_leaf_count = model
        .param_shapes
        .iter()
        .zip(&model.param_leaf_doubles)
        .fold(0usize, |count, (shape, leaves)| {
            count.saturating_add(shape.unwrap_or(leaves.len()))
        });
    let parameter_shapes_supported = model.param_shapes.len() == model.param_leaf_doubles.len()
        && model
            .param_shapes
            .iter()
            .zip(&model.param_leaf_doubles)
            .all(|(shape, leaves)| match shape {
                None => {
                    leaves.len() == 1
                        || (model.has_wide_scalar_features
                            && leaves.len() == 2
                            && leaves.iter().all(|is_double| !*is_double))
                }
                Some(count) => {
                    (1..=MAX_AGGREGATE_ARGUMENT_LEAF_COUNT).contains(count)
                        && leaves.len() == *count
                }
            });
    let result_shape_supported = match model.result_shape {
        None => {
            (model.result_leaf_doubles.len() <= 1 && model.value.get_size() == 64)
                || (model.has_wide_scalar_features
                    && model.result_leaf_doubles.len() == 2
                    && model
                        .result_leaf_doubles
                        .iter()
                        .all(|is_double| !*is_double)
                    && model.value.get_size() == 128)
        }
        Some(count) => {
            sret_result_leaf_count(model).is_some()
                || ((1..=MAX_SRET_RESULT_LEAF_COUNT).contains(&count)
                    && model.result_leaf_doubles.len() == count
                    && aggregate_result_register_classes_supported(&model.result_leaf_doubles)
                    && model.value.get_size() == (64 * count) as u32)
        }
    };
    model.inputs.len() == parameter_leaf_count
        && model.inputs.iter().all(|input| input.get_size() == 64)
        && parameter_shapes_supported
        && result_shape_supported
        && model.call_sites.iter().all(|site| {
            (site.result.get_size() == 64
                || site.has_composite_arguments_or_result
                || (model.has_wide_scalar_features
                    && site.result.get_size() == 128
                    && site.result_shape == (CallValueShape::Scalar { leaf_count: 2 })))
                && call_site_shapes_are_consistent(site)
        })
}

const MAX_AGGREGATE_LEAF_COUNT: usize = 4;
// Struct arguments (parameters and direct-call arguments) may carry up to
// sixteen leaves: LLVM aggregate lowering flattens them leaf-by-leaf into the
// scalar argument sequence regardless of size, INTEGER leaves past `rdi`-`r9`
// spilling onto the argument stack like scalars. Sixteen keeps each leaf a
// bounded solver term without admitting the unbounded leaf counts of truly
// huge aggregates; beyond it signatures stay fail-closed.
const MAX_AGGREGATE_ARGUMENT_LEAF_COUNT: usize = 16;

// Result aggregates past the register-resident three INTEGER leaves return
// through a hidden `sret` pointer — SysV's memory-class convention — carried
// in `rdi` while INTEGER arguments shift one register right. The callee
// stores each result leaf through a register that still holds the entry `rdi`
// value and must leave that same pointer in `rax`. The bound mirrors the
// argument one: a bounded solver term per leaf.
const MAX_SRET_RESULT_LEAF_COUNT: usize = MAX_AGGREGATE_ARGUMENT_LEAF_COUNT;

/// Whether a call site's result comes back through an `sret` buffer the
/// caller allocated: a struct of four to sixteen leaves whose INTEGER
/// leaves overflow the three INTEGER result registers — the same
/// admission as the callee side (`sret_result_leaf_count`), keyed on
/// the call site's `CallValueShape` instead of the function's own
/// result shape. Enum results share the path — their tag and payload
/// leaves flatten into the same all-INTEGER buffer writes. Register-
/// resident results (three INTEGER and two SSE slots) stay on their
/// own path; results LLVM reaches for the x87 register stay
/// fail-closed.
fn sret_call_result_leaf_count(site: &CallSite<'_>) -> Option<usize> {
    let leaf_count = match site.result_shape {
        CallValueShape::Struct {
            leaf_count,
            has_enum: false,
            ..
        }
        | CallValueShape::Enum { leaf_count } => leaf_count,
        _ => return None,
    };
    (MAX_AGGREGATE_LEAF_COUNT..=MAX_SRET_RESULT_LEAF_COUNT)
        .contains(&leaf_count)
        .then_some(())
        .filter(|_| {
            site.result_leaf_doubles.len() == leaf_count
                && leaf_doubles_need_sret(&site.result_leaf_doubles)
                && site.result.get_size() == (64 * leaf_count) as u32
        })
        .map(|_| leaf_count)
}

/// Whether this function's result comes back through a hidden `sret`
/// pointer: an aggregate whose INTEGER leaves exceed the three
/// register-resident INTEGER slots — real LLVM lowers a four-`i64`
/// result this way, and a mixed-class aggregate then writes every leaf
/// to memory (`f64` leaves as `movsd` stores), no matter how few SSE
/// leaves it carries. When INTEGER leaves still fit, LLVM puts the
/// third-plus SSE leaf in the x87 register instead — that path stays
/// fail-closed.
fn sret_result_leaf_count(model: &PureI64Body<'_>) -> Option<usize> {
    let leaf_count = model.result_shape?;
    (MAX_AGGREGATE_LEAF_COUNT..=MAX_SRET_RESULT_LEAF_COUNT)
        .contains(&leaf_count)
        .then_some(())
        .filter(|_| {
            model.result_leaf_doubles.len() == leaf_count
                && leaf_doubles_need_sret(&model.result_leaf_doubles)
                && model.value.get_size() == (64 * leaf_count) as u32
        })
        .map(|_| leaf_count)
}
const INTEGER_RESULT_REGISTERS: [usize; 3] = [0, 2, 1];
const FLOAT_RESULT_REGISTER_COUNT: usize = 2;

// LLVM hands aggregate results back on a per-class register budget:
// three INTEGER (`rax`/`rdx`/`rcx`) and two SSE (`xmm0`/`xmm1`) slots,
// so a five-leaf `{i64,f64,i64,f64,i64}` still returns in registers.
// Past the INTEGER budget the whole result goes memory-class (`sret`);
// past the SSE budget with room in INTEGER slots LLVM reaches for the
// x87 result register, which this model does not cover.
fn aggregate_result_register_classes_supported(result_leaf_doubles: &[bool]) -> bool {
    let integer_leaf_count = result_leaf_doubles
        .iter()
        .filter(|is_double| !**is_double)
        .count();
    let float_leaf_count = result_leaf_doubles
        .iter()
        .filter(|is_double| **is_double)
        .count();
    !result_leaf_doubles.is_empty()
        && integer_leaf_count <= INTEGER_RESULT_REGISTERS.len()
        && float_leaf_count <= FLOAT_RESULT_REGISTER_COUNT
}

/// Whether `result_leaf_doubles` describes a shape LLVM lowers through
/// the `sret` convention: more INTEGER leaves than the three INTEGER
/// result registers carry — the whole aggregate then goes to memory,
/// `f64` leaves included (`movsd` stores), no matter the SSE count.
fn leaf_doubles_need_sret(result_leaf_doubles: &[bool]) -> bool {
    result_leaf_doubles
        .iter()
        .filter(|is_double| !**is_double)
        .count()
        > INTEGER_RESULT_REGISTERS.len()
}

fn call_site_shapes_are_consistent(site: &CallSite<'_>) -> bool {
    let mut argument_leaf_count = 0usize;
    for shape in &site.argument_shapes {
        let Some(end) = argument_leaf_count.checked_add(shape.leaf_count()) else {
            return false;
        };
        let Some(leaf_doubles) = site.arg_leaf_doubles.get(argument_leaf_count..end) else {
            return false;
        };
        if let CallValueShape::Struct {
            leaf_count,
            has_enum,
            has_f64,
        } = shape
        {
            if *has_enum
                || !(1..=MAX_AGGREGATE_ARGUMENT_LEAF_COUNT).contains(leaf_count)
                || *has_f64 != leaf_doubles.iter().any(|is_double| *is_double)
            {
                return false;
            }
        } else if let CallValueShape::Enum { leaf_count } = shape {
            // Enum arguments flatten leaf-by-leaf into the scalar argument
            // sequence exactly like struct arguments — the shared bound is
            // sixteen leaves, matching the parameter side.
            if !(1..=MAX_AGGREGATE_ARGUMENT_LEAF_COUNT).contains(leaf_count)
                || leaf_doubles.first().copied().unwrap_or(true)
            {
                return false;
            }
        } else if matches!(shape, CallValueShape::Scalar { leaf_count: 2 })
            && leaf_doubles.iter().any(|is_double| *is_double)
        {
            return false;
        }
        argument_leaf_count = end;
    }
    let Some(result_bit_width) = site.result_shape.leaf_count().checked_mul(64) else {
        return false;
    };
    if site.result_leaf_doubles.len() != site.result_shape.leaf_count() {
        return false;
    }
    if matches!(
        site.result_shape,
        CallValueShape::Struct { .. } | CallValueShape::Enum { .. }
    ) && !aggregate_result_register_classes_supported(&site.result_leaf_doubles)
        && sret_call_result_leaf_count(site).is_none()
    {
        return false;
    }
    match site.result_shape {
        CallValueShape::Scalar { leaf_count: 1 }
            if site.result_is_double != site.result_leaf_doubles[0] =>
        {
            return false;
        }
        CallValueShape::Scalar { leaf_count: 2 }
            if site.result_is_double
                || site.result_leaf_doubles.iter().any(|is_double| *is_double) =>
        {
            return false;
        }
        CallValueShape::Struct { has_f64, .. }
            if has_f64 != site.result_leaf_doubles.iter().any(|is_double| *is_double) =>
        {
            return false;
        }
        CallValueShape::Enum { .. }
            if site.result_leaf_doubles.first().copied().unwrap_or(true) =>
        {
            return false;
        }
        _ => {}
    }
    let has_composite_shape = site.argument_shapes.iter().any(|shape| {
        matches!(
            shape,
            CallValueShape::Struct { .. } | CallValueShape::Enum { .. }
        )
    }) || matches!(
        site.result_shape,
        CallValueShape::Struct { .. } | CallValueShape::Enum { .. }
    );
    argument_leaf_count == site.args.len()
        && site.args.iter().all(|argument| argument.get_size() == 64)
        && site.arg_leaf_doubles.len() == site.args.len()
        && result_bit_width == site.result.get_size() as usize
        && has_composite_shape == site.has_composite_arguments_or_result
}

fn model_result_is_double(model: &PureI64Body<'_>) -> bool {
    model.result_leaf_doubles.first().copied().unwrap_or(false)
}

fn model_has_f64_leaves(model: &PureI64Body<'_>) -> bool {
    model
        .param_leaf_doubles
        .iter()
        .flatten()
        .any(|is_double| *is_double)
        || model.result_leaf_doubles.iter().any(|is_double| *is_double)
        || model.call_sites.iter().any(|site| {
            site.arg_leaf_doubles.iter().any(|is_double| *is_double)
                || site.result_leaf_doubles.iter().any(|is_double| *is_double)
        })
}

fn f64_values_match<'ctx>(context: &'ctx Context, left: &BV<'ctx>, right: &BV<'ctx>) -> Bool<'ctx> {
    let both_nan = Bool::and(
        context,
        &[
            &mumei_core::semantic_contract::f64_is_nan(context, left),
            &mumei_core::semantic_contract::f64_is_nan(context, right),
        ],
    );
    Bool::or(context, &[&left._eq(right), &both_nan])
}

fn x86_sse_scalar_f64<'ctx>(
    context: &'ctx Context,
    a: &BV<'ctx>,
    b: &BV<'ctx>,
    operation: mumei_core::semantic_contract::F64Arith,
) -> BV<'ctx> {
    use mumei_core::semantic_contract::{f64_arith, f64_is_nan};

    let rounded = f64_arith(context, a, b, operation);
    let quiet_mask = BV::from_u64(context, 1 << 51, 64);
    let quiet_a = a.bvor(&quiet_mask);
    let quiet_b = b.bvor(&quiet_mask);
    let invalid_result = BV::from_u64(context, 0xFFF8_0000_0000_0000, 64);
    let non_nan_result = f64_is_nan(context, &rounded).ite(&invalid_result, &rounded);
    let b_nan_result = f64_is_nan(context, b).ite(&quiet_b, &non_nan_result);
    f64_is_nan(context, a).ite(&quiet_a, &b_nan_result)
}

fn set_solver_budget(solver: &Solver, context: &Context) {
    let mut params = Params::new(context);
    params.set_u32("timeout", TIMEOUT_MS);
    params.set_u32("rlimit", RESOURCE_LIMIT);
    solver.set_params(&params);
}

fn aggregate_leaf<'ctx>(value: &BV<'ctx>, leaf_count: usize, leaf_index: usize) -> BV<'ctx> {
    let high = (64 * (leaf_count - leaf_index) - 1) as u32;
    let low = (64 * (leaf_count - leaf_index - 1)) as u32;
    value.extract(high, low)
}

/// A lifted path's per-leaf return values folded the way the IR
/// encoder lays out an aggregate `ret`: leaf 0 in the high bits
/// (`semantic_contract::aggregate_value`'s layout), so both sides
/// compare one bit-vector per return edge.
fn aggregate_return_value<'ctx>(values: &[BV<'ctx>]) -> BV<'ctx> {
    let (first, rest) = values
        .split_first()
        .expect("lifted paths carry at least one return value");
    rest.iter()
        .fold(first.clone(), |acc, slot| acc.concat(slot))
}

fn assert_path_equivalence_query<'ctx>(
    solver: &Solver<'ctx>,
    model: &PureI64Body<'ctx>,
    arguments: &[BV<'ctx>],
    path: &LiftedPath<'ctx>,
    result_width: ResultWidth,
    expected_ir: Option<ExpectedIr<'_, 'ctx>>,
    context: &'ctx Context,
) {
    let substitutions = input_substitutions(model, arguments);
    assert_path_assumptions(solver, model, arguments, path);
    // Lifted machine terms can themselves contain the model's input
    // constants — e.g. a call's return value is the callee's
    // uninterpreted-function application over the *source* argument
    // terms — so fold them onto the same variables as the model side.
    let substituted_returns: Vec<BV> = path
        .return_values
        .iter()
        .map(|value| value.substitute(&substitutions))
        .collect();
    let model_value = model.value.substitute(&substitutions);
    let output_matches = if let Some((ir_returns, ir_definedness)) = expected_ir {
        // The expected value is the IR encoding: some edge must cover
        // this machine path with a matching return and discharged
        // definedness obligations — the same mismatch disjunction as
        // `check_function_against_ir`, negated into an equality claim.
        let machine_return = aggregate_return_value(&substituted_returns);
        let coverage: Vec<Bool> = ir_returns
            .iter()
            .map(|(condition, _)| condition.substitute(&substitutions))
            .collect();
        let coverage_refs = coverage.iter().collect::<Vec<_>>();
        let mut mismatches = vec![Bool::or(context, &coverage_refs).not()];
        for (condition, value) in ir_returns {
            let condition = condition.substitute(&substitutions);
            let value = value.substitute(&substitutions);
            mismatches.push(Bool::and(
                context,
                &[&condition, &machine_return._eq(&value).not()],
            ));
        }
        for (condition, defined) in ir_definedness {
            let condition = condition.substitute(&substitutions);
            let defined = defined.substitute(&substitutions);
            mismatches.push(Bool::and(context, &[&condition, &defined.not()]));
        }
        let mismatches_refs = mismatches.iter().collect::<Vec<_>>();
        Bool::or(context, &mismatches_refs).not()
    } else if let Some(leaf_count) = model.result_shape {
        let matches = (0..leaf_count)
            .map(|leaf| {
                let source_leaf = aggregate_leaf(&model_value, leaf_count, leaf);
                let machine_leaf = &substituted_returns[leaf];
                if model.result_leaf_doubles[leaf] {
                    f64_values_match(context, &source_leaf, machine_leaf)
                } else {
                    source_leaf._eq(machine_leaf)
                }
            })
            .collect::<Vec<_>>();
        Bool::and(context, &matches.iter().collect::<Vec<_>>())
    } else {
        let return_value = &substituted_returns[0];
        match result_width {
            ResultWidth::I64 => model_value._eq(return_value),
            ResultWidth::I32 => model_value.extract(31, 0)._eq(&return_value.extract(31, 0)),
            ResultWidth::F64 => f64_values_match(context, &model_value, return_value),
            ResultWidth::I128 => model_value._eq(return_value),
        }
    };
    // Call-site obligations reference the model's input constants
    // (e.g. a call passing the caller's own argument through), so fold
    // them with the same substitutions as the assumptions.
    let mut equivalent_terms: Vec<Bool> = path
        .call_argument_obligations
        .iter()
        .map(|obligation| obligation.substitute(&substitutions))
        .collect();
    equivalent_terms.push(output_matches);
    if let Some(obligation) = &path.sret_pointer_obligation {
        equivalent_terms.push(obligation.substitute(&substitutions));
    }
    solver.assert(&Bool::and(context, &equivalent_terms.iter().collect::<Vec<_>>()).not());
}

fn assert_path_exact_f64_call_arguments_query<'ctx>(
    solver: &Solver<'ctx>,
    model: &PureI64Body<'ctx>,
    arguments: &[BV<'ctx>],
    path: &LiftedPath<'ctx>,
    context: &'ctx Context,
) {
    let substitutions = input_substitutions(model, arguments);
    assert_path_assumptions(solver, model, arguments, path);
    let exact_arguments: Vec<Bool> = path
        .exact_f64_call_argument_obligations
        .iter()
        .map(|obligation| obligation.substitute(&substitutions))
        .collect();
    solver.assert(&Bool::and(context, &exact_arguments.iter().collect::<Vec<_>>()).not());
}

fn input_substitutions<'ctx, 'a>(
    model: &'a PureI64Body<'ctx>,
    arguments: &'a [BV<'ctx>],
) -> Vec<(&'a BV<'ctx>, &'a BV<'ctx>)> {
    model.inputs.iter().zip(arguments.iter()).collect()
}

fn assert_path_assumptions<'ctx>(
    solver: &Solver<'ctx>,
    model: &PureI64Body<'ctx>,
    arguments: &[BV<'ctx>],
    path: &LiftedPath<'ctx>,
) {
    // Fold the model's input constants into its obligations directly
    // instead of asserting `input_i == register_i` bridging equalities.
    // Separate input and register variables make Z3 duplicate the
    // bit-blasted input constraints on both sides of multiply/divide
    // pairs (e.g. magic-multiply division), which pushes those queries
    // over the rlimit cliff; substitution shares one term across both.
    let substitutions = input_substitutions(model, arguments);
    for assumption in model
        .param_domain
        .iter()
        .chain(&model.definedness)
        .chain(&model.call_result_bounds)
        .chain(model.loop_bound_exits.iter())
    {
        solver.assert(&assumption.substitute(&substitutions));
    }
    for condition in &path.path_conditions {
        solver.assert(&condition.substitute(&substitutions));
    }
}

fn assert_path_callee_saved_preservation_query<'ctx>(
    solver: &Solver<'ctx>,
    model: &PureI64Body<'ctx>,
    arguments: &[BV<'ctx>],
    initial_callee_saved: &[BV<'ctx>],
    path: &LiftedPath<'ctx>,
    context: &'ctx Context,
) {
    let substitutions = input_substitutions(model, arguments);
    assert_path_assumptions(solver, model, arguments, path);
    let mut changed_registers = initial_callee_saved
        .iter()
        .zip(&path.callee_saved_registers)
        .map(|(initial, final_value)| {
            final_value
                .substitute(&substitutions)
                ._eq(&initial.substitute(&substitutions))
                .not()
        })
        .collect::<Vec<_>>();
    changed_registers.push(Bool::from_bool(context, path.final_rsp_offset != 0));
    let changed_registers = changed_registers.iter().collect::<Vec<_>>();
    solver.assert(&Bool::or(context, &changed_registers));
}

fn check_query_with_simplify_fallback<'ctx>(
    context: &'ctx Context,
    assert_query: impl Fn(&Solver<'ctx>),
) -> SatResult {
    let default_result = {
        let solver = Solver::new(context);
        set_solver_budget(&solver, context);
        assert_query(&solver);
        solver.check()
    };
    if default_result == SatResult::Unknown {
        let simplify = Tactic::new(context, "simplify");
        let smt = Tactic::new(context, "smt");
        let fallback_solver = simplify.and_then(&smt).solver();
        set_solver_budget(&fallback_solver, context);
        assert_query(&fallback_solver);
        fallback_solver.check()
    } else {
        default_result
    }
}

const ARGUMENT_REGISTERS: [usize; 6] = [7, 6, 2, 1, 8, 9];
const FLOAT_ARGUMENT_REGISTER_COUNT: usize = 8;
const CALLEE_SAVED_REGISTERS: [usize; 6] = [3, 5, 12, 13, 14, 15];

struct LiftedFunction<'ctx> {
    arguments: Vec<BV<'ctx>>,
    callee_saved_initial: Vec<BV<'ctx>>,
    paths: Vec<LiftedPath<'ctx>>,
    frame_memory_use: FrameMemoryUse,
}

struct LiftedPath<'ctx> {
    return_values: Vec<BV<'ctx>>,
    /// `rax` must hand the sret result pointer back to the caller —
    /// present only on memory-class result signatures.
    sret_pointer_obligation: Option<Bool<'ctx>>,
    callee_saved_registers: Vec<BV<'ctx>>,
    final_rsp_offset: i64,
    path_conditions: Vec<Bool<'ctx>>,
    call_argument_obligations: Vec<Bool<'ctx>>,
    exact_f64_call_argument_obligations: Vec<Bool<'ctx>>,
    used_call_targets: BTreeSet<u64>,
    used_frame_memory: bool,
    used_allocated_frame_memory: bool,
}

#[derive(Clone)]
struct CallState<'ctx> {
    call_index: usize,
    used_call_targets: BTreeSet<u64>,
    argument_obligations: Vec<Bool<'ctx>>,
    exact_f64_argument_obligations: Vec<Bool<'ctx>>,
}

#[derive(Clone)]
enum FlagState<'ctx> {
    Unknown,
    Compare {
        left: BV<'ctx>,
        right: BV<'ctx>,
        width: u32,
    },
    Arithmetic {
        value: BV<'ctx>,
        carry: Bool<'ctx>,
        overflow: Bool<'ctx>,
        width: u32,
    },
    FloatCompare {
        left: BV<'ctx>,
        right: BV<'ctx>,
    },
    Result {
        value: BV<'ctx>,
        width: u32,
    },
    ZeroFlagResult {
        value: BV<'ctx>,
        width: u32,
    },
    IncDecResult {
        value: BV<'ctx>,
        overflow: Bool<'ctx>,
        width: u32,
    },
}

#[derive(Clone)]
struct MachineState<'ctx> {
    instruction_index: usize,
    instruction_visits: Vec<usize>,
    registers: Vec<BV<'ctx>>,
    xmm_registers: Vec<BV<'ctx>>,
    known_upper_32_zero: [bool; 16],
    stack: Vec<(i64, BV<'ctx>)>,
    frame_memory: Array<'ctx>,
    used_frame_memory: bool,
    used_allocated_frame_memory: bool,
    frame_region: Option<(i64, i64)>,
    frame_region_closed: bool,
    outgoing_cleanup_slots: BTreeSet<i64>,
    rsp_offset: i64,
    rsp_is_known: bool,
    flags: FlagState<'ctx>,
    call_state: CallState<'ctx>,
    path_conditions: Vec<Bool<'ctx>>,
    /// The hidden `sret` result-buffer pointer a memory-class result
    /// arrives with — the entry `rdi` BV itself, so a `mov %rdi,%rax`
    /// keeps registers carrying that same AST recognized as the
    /// pointer while overwriting `rdi` drops the recognition.
    sret_pointer: Option<BV<'ctx>>,
    /// `movq` stores this path recorded through the sret pointer, as
    /// `(byte offset inside the result buffer, value)` — the last
    /// store to an offset wins.
    sret_stores: Vec<(i64, BV<'ctx>)>,
    /// Caller-side `sret` result buffers recorded on this path: one entry
    /// per call whose result shape needs the memory-class convention,
    /// holding the pointer BV the call was made with and the buffer's
    /// entry-relative base offset so reads through a register still
    /// carrying the pointer (`(%rax)` right after the call) resolve to
    /// the recorded leaf slots.
    sret_call_buffers: Vec<SretCallBuffer<'ctx>>,
}

/// One caller-allocated `sret` result buffer: `pointer` is the BV the
/// call left in `rdi` (later also in `rax`); `base_offset` is the
/// entry-relative stack address leaf 0 occupies, so buffer leaf `i`
/// lives at `base_offset + 8 * i` in the `stack` slot map.
#[derive(Clone)]
struct SretCallBuffer<'ctx> {
    pointer: BV<'ctx>,
    base_offset: i64,
    leaf_count: usize,
}

const MAX_COMPLETED_PATHS: usize = 64;

#[cfg(all(test, target_arch = "x86_64"))]
#[path = "machine_tv_cpu_differential.rs"]
mod cpu_differential;

fn lift<'ctx>(
    instructions: &[Instruction],
    function_size: u64,
    model: &PureI64Body<'ctx>,
    call_targets: &BTreeMap<u64, CallTarget>,
    context: &'ctx Context,
) -> Result<LiftedFunction<'ctx>, FunctionOutcome> {
    lift_inner(
        instructions,
        function_size,
        model,
        call_targets,
        context,
        #[cfg(all(test, target_arch = "x86_64"))]
        None,
    )
}

fn lift_inner<'ctx>(
    instructions: &[Instruction],
    function_size: u64,
    model: &PureI64Body<'ctx>,
    call_targets: &BTreeMap<u64, CallTarget>,
    context: &'ctx Context,
    #[cfg(all(test, target_arch = "x86_64"))] mut probe: Option<&mut cpu_differential::Probe<'ctx>>,
) -> Result<LiftedFunction<'ctx>, FunctionOutcome> {
    let branch_targets = resolve_branch_targets(instructions, function_size, call_targets)?;
    let function_has_calls = instructions.iter().any(|instruction| {
        instruction.mnemonic == "callq" || is_tail_call(instruction, call_targets)
    });
    let function_adjusts_stack = instructions.iter().any(|instruction| {
        instruction.mnemonic.starts_with("push")
            || instruction.mnemonic.starts_with("pop")
            || split_operands(&instruction.operands)
                .last()
                .is_some_and(|destination| {
                    matches!(destination.trim(), "%rsp" | "%esp" | "%sp" | "%spl")
                })
    });
    let function_indexes_frame = instructions
        .iter()
        .any(|instruction| instruction.operands.contains("(%rsp,%"));
    let allocated_frame_memory_mode = function_indexes_frame && model.bounded_memory;
    let memory_sort = Sort::bitvector(context, 64);
    let frame_memory = Array::fresh_const(
        context,
        "machine_red_zone_frame_memory",
        &memory_sort,
        &memory_sort,
    );
    let registers = (0..16)
        .map(|_| BV::fresh_const(context, "machine_initial_register", 64))
        .collect::<Vec<_>>();
    let xmm_registers = (0..16)
        .map(|_| BV::fresh_const(context, "machine_initial_xmm_register", 128))
        .collect::<Vec<_>>();
    #[cfg(all(test, target_arch = "x86_64"))]
    let (registers, xmm_registers) = {
        let (mut registers, mut xmm_registers) = (registers, xmm_registers);
        if let Some(probe) = probe.as_deref() {
            probe.seed_registers(&mut registers, &mut xmm_registers, context);
        }
        (registers, xmm_registers)
    };
    let callee_saved_initial = CALLEE_SAVED_REGISTERS
        .iter()
        .map(|register_index| registers[*register_index].clone())
        .collect();
    // `(input_index, is_double)` for every argument leaf that spilled
    // onto the incoming argument stack, in declaration order — SysV
    // lays overflow leaves of both classes out in that order, so the
    // stack slot ordinal maps straight into this list.
    let mut stack_argument_leaves: Vec<(usize, bool)> = Vec::new();
    // A memory-class result hands the callee a hidden `sret` pointer in
    // `rdi`, so INTEGER argument leaves begin one register later at
    // `rsi`. The pointer is the entry `rdi` BV itself: a `mov
    // %rdi,%rax` leaves `rax` carrying the same AST, which the sret
    // store path recognizes as the result-buffer base.
    let sret_leaf_count = sret_result_leaf_count(model);
    let sret_pointer = sret_leaf_count.map(|_| registers[7].clone());
    let mut integer_argument_index = usize::from(sret_pointer.is_some());
    let mut float_argument_index = 0usize;
    let mut arguments = Vec::with_capacity(model.inputs.len());
    let mut input_index = 0usize;
    for leaves in model.param_leaf_doubles.iter() {
        for is_double in leaves {
            if *is_double {
                // xmm0-xmm7 carry `f64` arguments; the ninth and later
                // `f64` leaves spill to the argument stack like INTEGER
                // leaves do past `r9`.
                arguments.push(if float_argument_index < FLOAT_ARGUMENT_REGISTER_COUNT {
                    xmm_registers[float_argument_index].extract(63, 0)
                } else {
                    stack_argument_leaves.push((input_index, true));
                    BV::fresh_const(context, "machine_initial_stack_argument", 64)
                });
                float_argument_index += 1;
            } else {
                arguments.push(ARGUMENT_REGISTERS.get(integer_argument_index).map_or_else(
                    || {
                        stack_argument_leaves.push((input_index, false));
                        BV::fresh_const(context, "machine_initial_stack_argument", 64)
                    },
                    |register_index| registers[*register_index].clone(),
                ));
                integer_argument_index += 1;
            }
            input_index += 1;
        }
    }
    #[cfg(all(test, target_arch = "x86_64"))]
    if let Some(probe) = probe.as_deref() {
        probe.seed_stack_arguments(&stack_argument_leaves, &mut arguments, context);
    }
    let mut pending = vec![MachineState {
        instruction_index: 0,
        instruction_visits: vec![0; instructions.len()],
        registers,
        xmm_registers,
        known_upper_32_zero: [false; 16],
        stack: Vec::new(),
        frame_memory,
        used_frame_memory: false,
        used_allocated_frame_memory: false,
        frame_region: None,
        frame_region_closed: false,
        outgoing_cleanup_slots: BTreeSet::new(),
        rsp_offset: 0,
        rsp_is_known: true,
        flags: FlagState::Unknown,
        call_state: CallState {
            call_index: 0,
            used_call_targets: BTreeSet::new(),
            argument_obligations: Vec::new(),
            exact_f64_argument_obligations: Vec::new(),
        },
        path_conditions: Vec::new(),
        sret_pointer,
        sret_stores: Vec::new(),
        sret_call_buffers: Vec::new(),
    }];
    let mut paths = Vec::new();
    while let Some(mut state) = pending.pop() {
        loop {
            let index = state.instruction_index;
            let Some(instruction) = instructions.get(index) else {
                return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
            };
            #[cfg(all(test, target_arch = "x86_64"))]
            if let Some(probe) = probe.as_deref_mut() {
                probe.observe(index, &state);
            }
            if state.instruction_visits[index] > LOOP_BOUND {
                match path_feasibility_status(model, &state.path_conditions, &arguments, context) {
                    SatResult::Unsat => break,
                    SatResult::Sat | SatResult::Unknown => {
                        return Err(FunctionOutcome::OutsideProfile("loop_bound"));
                    }
                }
            }
            state.instruction_visits[index] += 1;
            if is_conditional_branch(&instruction.mnemonic) {
                let suffix = instruction
                    .mnemonic
                    .strip_prefix('j')
                    .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
                let condition = condition_for_flags(&state.flags, suffix, context)?;
                let target = *branch_targets
                    .get(&index)
                    .ok_or(FunctionOutcome::OutsideProfile("branch_target"))?;
                let mut taken = state.clone();
                taken.path_conditions.push(condition.clone());
                taken.instruction_index = target;
                let taken_is_feasible =
                    path_feasibility_status(model, &taken.path_conditions, &arguments, context)
                        != SatResult::Unsat;
                state.path_conditions.push(condition.not());
                state.instruction_index = index + 1;
                if taken_is_feasible {
                    pending.push(taken);
                }
                if path_feasibility_status(model, &state.path_conditions, &arguments, context)
                    != SatResult::Unsat
                {
                    continue;
                }
                break;
            }
            if is_unconditional_jump(&instruction.mnemonic) {
                if is_tail_call(instruction, call_targets) {
                    if index + 1 != instructions.len()
                        || state.rsp_offset != 0
                        || !state.stack.is_empty()
                        || !state.outgoing_cleanup_slots.is_empty()
                    {
                        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
                    }
                    lift_call(
                        instruction,
                        model,
                        call_targets,
                        &mut state.call_state,
                        &mut state.registers,
                        &mut state.xmm_registers,
                        &mut state.stack,
                        state.rsp_offset,
                        &mut state.outgoing_cleanup_slots,
                        &mut state.sret_call_buffers,
                        state.frame_region,
                        state.rsp_is_known,
                        &state.path_conditions,
                        &arguments,
                        true,
                        context,
                    )?;
                    state.flags = FlagState::Unknown;
                    paths.push(finish_path(state, model, &arguments, context)?);
                    if paths.len() > MAX_COMPLETED_PATHS {
                        return Err(FunctionOutcome::OutsideProfile("path_bound"));
                    }
                    break;
                }
                state.instruction_index = *branch_targets
                    .get(&index)
                    .ok_or(FunctionOutcome::OutsideProfile("branch_target"))?;
                continue;
            }
            if instruction.mnemonic == "retq" {
                if !instruction.operands.is_empty()
                    || state.rsp_offset != 0
                    || !state.stack.is_empty()
                    || !state.outgoing_cleanup_slots.is_empty()
                {
                    return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
                }
                paths.push(finish_path(state, model, &arguments, context)?);
                if paths.len() > MAX_COMPLETED_PATHS {
                    return Err(FunctionOutcome::OutsideProfile("path_bound"));
                }
                break;
            }
            if instruction.mnemonic.starts_with("nop") {
                state.instruction_index += 1;
                continue;
            }
            let move_source_upper_32_zero = instruction.mnemonic == "movl"
                && split_operands(&instruction.operands)
                    .first()
                    .and_then(|source| parse_register(source))
                    .is_some_and(|(index, width)| width == 32 && state.known_upper_32_zero[index]);
            if instruction.mnemonic == "cqto" {
                lift_cqto(instruction, &mut state.registers, context)?;
            } else if instruction.mnemonic == "cltd" {
                lift_cltd(instruction, &mut state.registers, context)?;
            } else if matches!(instruction.mnemonic.as_str(), "idivq" | "divq" | "divl") {
                lift_division(instruction, &mut state, model, &arguments, context)?;
                state.flags = FlagState::Unknown;
            } else if instruction.mnemonic == "imulq"
                && split_operands(&instruction.operands).len() == 1
            {
                lift_imul_high(instruction, &mut state.registers)?;
                state.flags = FlagState::Unknown;
            } else if instruction.mnemonic == "mulq" {
                if model.has_wide_scalar_features {
                    return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
                }
                lift_mul_high(instruction, &mut state.registers)?;
                state.flags = FlagState::Unknown;
            } else if instruction.mnemonic == "callq" {
                lift_call(
                    instruction,
                    model,
                    call_targets,
                    &mut state.call_state,
                    &mut state.registers,
                    &mut state.xmm_registers,
                    &mut state.stack,
                    state.rsp_offset,
                    &mut state.outgoing_cleanup_slots,
                    &mut state.sret_call_buffers,
                    state.frame_region,
                    state.rsp_is_known,
                    &state.path_conditions,
                    &arguments,
                    false,
                    context,
                )?;
                state.flags = FlagState::Unknown;
            } else if matches!(
                instruction.mnemonic.as_str(),
                "incl" | "incq" | "decl" | "decq"
            ) {
                let (value, width, overflow) = lift_inc_dec(instruction, &mut state, context)?;
                state.flags = FlagState::IncDecResult {
                    value,
                    overflow,
                    width,
                };
            } else if matches!(instruction.mnemonic.as_str(), "negl" | "negq") {
                let width = instruction_width(&instruction.mnemonic)?;
                let operands = split_operands(&instruction.operands);
                let [destination] = operands.as_slice() else {
                    return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
                };
                let right = register_value(destination, &state.registers, width)
                    .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
                if !lift_neg(&operands, &mut state.registers, width) {
                    return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
                }
                state.flags = arithmetic_flags(
                    &BV::from_u64(context, 0, width),
                    &right,
                    None,
                    true,
                    context,
                );
            } else if instruction.mnemonic == "pushq" {
                if !state.rsp_is_known || state.rsp_offset % 8 != 0 {
                    return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
                }
                let operands = split_operands(&instruction.operands);
                let [source] = operands.as_slice() else {
                    return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
                };
                // `pushq` accepts a register, an immediate, or a memory
                // source — LLVM forwards an incoming stack argument to a
                // callee's overflow area with `pushq N(%rsp)` directly.
                // x86-64 `push imm` sign-extends an imm8/imm32 to 64
                // bits, matching the sign extension convention used for
                // `movq $imm, mem` stores.
                let value = if let Some(immediate) = source.strip_prefix('$') {
                    let immediate = parse_integer(immediate)
                        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
                    BV::from_i64(context, i64::from(immediate as u32 as i32), 64)
                } else if source.contains('(') {
                    stack_memory_source(source, &state, &stack_argument_leaves, &arguments, false)?
                } else {
                    register_value(source, &state.registers, 64)
                        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?
                };
                if !adjust_rsp(
                    &mut state.registers,
                    &mut state.stack,
                    &mut state.rsp_offset,
                    -8,
                    context,
                ) {
                    return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
                }
                state.stack.push((state.rsp_offset, value));
            } else if instruction.mnemonic == "popq" {
                if !state.rsp_is_known || state.rsp_offset % 8 != 0 {
                    return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
                }
                let operands = split_operands(&instruction.operands);
                let [destination] = operands.as_slice() else {
                    return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
                };
                let Some((register_index, width)) = parse_register(destination) else {
                    return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
                };
                if width != 64 || register_index == 4 {
                    return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
                }
                if let Some(stack_index) = state
                    .stack
                    .iter()
                    .rposition(|(stack_slot, _)| *stack_slot == state.rsp_offset)
                {
                    let (_, value) = state.stack.remove(stack_index);
                    if !adjust_rsp(
                        &mut state.registers,
                        &mut state.stack,
                        &mut state.rsp_offset,
                        8,
                        context,
                    ) || !write_register(&mut state.registers, register_index, value, 64)
                    {
                        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
                    }
                } else if state.outgoing_cleanup_slots.remove(&state.rsp_offset) {
                    if !adjust_rsp(
                        &mut state.registers,
                        &mut state.stack,
                        &mut state.rsp_offset,
                        8,
                        context,
                    ) || !write_register(
                        &mut state.registers,
                        register_index,
                        BV::fresh_const(context, "machine_outgoing_stack_cleanup", 64),
                        64,
                    ) {
                        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
                    }
                } else {
                    return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
                }
            } else if instruction.mnemonic == "movq"
                && split_operands(&instruction.operands)
                    .get(1)
                    .is_some_and(|destination| destination.contains('('))
            {
                let operands = split_operands(&instruction.operands);
                let destination = operands
                    .get(1)
                    .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
                if sret_memory_offset(destination, &state).is_some_and(|offset| {
                    sret_leaf_count.is_some_and(|leaf_count| offset < 8 * leaf_count as i64)
                }) {
                    lift_sret_store(instruction, &mut state, context)?;
                } else if sret_call_memory_offset(destination, &state).is_some() {
                    lift_sret_call_store(instruction, &mut state, context)?;
                } else if is_incoming_stack_argument_operand(destination, state.rsp_offset) {
                    lift_stack_store(instruction, &mut state, context)?;
                } else if should_use_allocated_frame_memory(
                    destination,
                    &state,
                    allocated_frame_memory_mode,
                )? {
                    lift_allocated_frame_memory_store(
                        instruction,
                        &mut state,
                        model,
                        &arguments,
                        context,
                    )?;
                } else if state.rsp_offset != 0 {
                    lift_stack_store(instruction, &mut state, context)?;
                } else {
                    lift_frame_memory_store(
                        instruction,
                        &mut state,
                        model,
                        &arguments,
                        function_has_calls,
                        function_adjusts_stack,
                        context,
                    )?;
                }
            } else if instruction.mnemonic == "movq"
                && split_operands(&instruction.operands)
                    .first()
                    .is_some_and(|source| source.contains('('))
            {
                let operands = split_operands(&instruction.operands);
                let source = operands
                    .first()
                    .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
                if sret_memory_offset(source, &state).is_some_and(|offset| {
                    sret_leaf_count.is_some_and(|leaf_count| offset < 8 * leaf_count as i64)
                }) {
                    lift_sret_load(instruction, &mut state, context)?;
                } else if sret_call_memory_offset(source, &state).is_some() {
                    lift_sret_call_load(instruction, &mut state, context)?;
                } else if is_incoming_stack_argument_operand(source, state.rsp_offset) {
                    lift_stack_argument_move(
                        instruction,
                        &mut state,
                        &stack_argument_leaves,
                        &arguments,
                    )?;
                } else if should_use_allocated_frame_memory(
                    source,
                    &state,
                    allocated_frame_memory_mode,
                )? {
                    lift_allocated_frame_memory_load(
                        instruction,
                        &mut state,
                        model,
                        &arguments,
                        context,
                    )?;
                } else if state.rsp_offset != 0 {
                    lift_stack_argument_move(
                        instruction,
                        &mut state,
                        &stack_argument_leaves,
                        &arguments,
                    )?;
                } else {
                    lift_frame_memory_load(
                        instruction,
                        &mut state,
                        model,
                        &arguments,
                        function_has_calls,
                        function_adjusts_stack,
                        context,
                    )?;
                }
            } else if stack_argument_binary_form(instruction).is_some() {
                lift_stack_argument_binary(
                    instruction,
                    &mut state,
                    &stack_argument_leaves,
                    &arguments,
                    context,
                )?;
            } else if matches!(
                instruction.mnemonic.as_str(),
                "movsd" | "addsd" | "subsd" | "mulsd" | "divsd" | "ucomisd" | "comisd"
            ) && model_has_f64_leaves(model)
                && split_operands(&instruction.operands)
                    .first()
                    .is_some_and(|source| source.contains('('))
            {
                let operands = split_operands(&instruction.operands);
                let source = operands
                    .first()
                    .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
                if instruction.mnemonic == "movsd"
                    && sret_memory_offset(source, &state).is_some_and(|offset| {
                        sret_leaf_count.is_some_and(|leaf_count| offset < 8 * leaf_count as i64)
                    })
                {
                    lift_sret_load(instruction, &mut state, context)?;
                } else if instruction.mnemonic == "movsd"
                    && sret_call_memory_offset(source, &state).is_some()
                {
                    lift_sret_call_load(instruction, &mut state, context)?;
                } else {
                    lift_stack_argument_sse(
                        instruction,
                        &mut state,
                        &stack_argument_leaves,
                        &arguments,
                        context,
                    )?;
                }
            } else if instruction.mnemonic.as_str() == "movsd"
                && model_has_f64_leaves(model)
                && split_operands(&instruction.operands)
                    .get(1)
                    .is_some_and(|destination| destination.contains('('))
            {
                let operands = split_operands(&instruction.operands);
                let destination = operands
                    .get(1)
                    .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
                if sret_memory_offset(destination, &state).is_some_and(|offset| {
                    sret_leaf_count.is_some_and(|leaf_count| offset < 8 * leaf_count as i64)
                }) {
                    lift_sret_store(instruction, &mut state, context)?;
                } else if sret_call_memory_offset(destination, &state).is_some() {
                    lift_sret_call_store(instruction, &mut state, context)?;
                } else if should_use_allocated_frame_memory(
                    destination,
                    &state,
                    allocated_frame_memory_mode,
                )? {
                    // An SSE scalar store into the allocated frame
                    // region has no modeled path: writing only `stack`
                    // while integer loads read `frame_memory` would
                    // silently drop the write, so fail closed.
                    return Err(FunctionOutcome::OutsideProfile("frame_memory_bounds"));
                } else {
                    lift_stack_store_sse(instruction, &mut state)?;
                }
            } else if matches!(
                instruction.mnemonic.as_str(),
                "movaps" | "movups" | "movdqa" | "movdqu"
            ) && split_operands(&instruction.operands)
                .iter()
                .any(|operand| operand.contains('('))
            {
                let operands = split_operands(&instruction.operands);
                let memory_is_destination =
                    operands.get(1).is_some_and(|operand| operand.contains('('));
                let memory = if memory_is_destination {
                    operands.get(1).copied()
                } else {
                    operands.first().copied()
                }
                .unwrap_or("");
                if memory.contains("%rsp") {
                    lift_stack_wide_move(
                        instruction,
                        &mut state,
                        &stack_argument_leaves,
                        &arguments,
                        allocated_frame_memory_mode,
                    )?;
                } else if memory_is_destination {
                    // A 16-byte store through a register-held `sret`
                    // pointer — LLVM vectorizes buffer-to-buffer
                    // forwarding of memory-class results this way. Both
                    // halves must land inside the result buffer, like
                    // the scalar `movq` bound above.
                    if sret_memory_offset(memory, &state).is_some_and(|offset| {
                        sret_leaf_count.is_some_and(|leaf_count| offset + 8 < 8 * leaf_count as i64)
                    }) {
                        lift_sret_wide_store(instruction, &mut state)?;
                    } else if sret_call_memory_offset(memory, &state).is_some() {
                        lift_sret_call_wide_store(instruction, &mut state)?;
                    } else {
                        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
                    }
                } else if sret_memory_offset(memory, &state).is_some_and(|offset| {
                    sret_leaf_count.is_some_and(|leaf_count| offset + 8 < 8 * leaf_count as i64)
                }) {
                    lift_sret_wide_load(instruction, &mut state, context)?;
                } else if sret_call_memory_offset(memory, &state).is_some() {
                    lift_sret_call_wide_load(instruction, &mut state, context)?;
                } else {
                    return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
                }
            } else if matches!(instruction.mnemonic.as_str(), "addq" | "subq")
                && split_operands(&instruction.operands)
                    .get(1)
                    .is_some_and(|destination| *destination == "%rsp")
            {
                lift_rsp_adjustment(
                    instruction,
                    &mut state,
                    allocated_frame_memory_mode,
                    context,
                )?;
            } else if matches!(instruction.mnemonic.as_str(), "cmpb" | "cmpl" | "cmpq") {
                let width = instruction_width(&instruction.mnemonic)?;
                state.flags =
                    compare_flags(&instruction.operands, &state.registers, context, width)?;
            } else if matches!(instruction.mnemonic.as_str(), "ucomisd" | "comisd")
                && model_has_f64_leaves(model)
            {
                state.flags = float_compare_flags(&instruction.operands, &state.xmm_registers)?;
            } else if matches!(instruction.mnemonic.as_str(), "testb" | "testl" | "testq") {
                let width = instruction_width(&instruction.mnemonic)?;
                state.flags = test_flags(&instruction.operands, &state.registers, context, width)?;
            } else if matches!(
                instruction.mnemonic.as_str(),
                "addl" | "addq" | "subl" | "subq"
            ) {
                let width = instruction_width(&instruction.mnemonic)?;
                let (left, right) =
                    binary_operands(&instruction.operands, &state.registers, context, width)?;
                let is_subtraction = instruction.mnemonic.starts_with("sub");
                let operation = if is_subtraction {
                    BinaryOp::Sub
                } else {
                    BinaryOp::Add
                };
                if !lift_binary(
                    &split_operands(&instruction.operands),
                    &mut state.registers,
                    context,
                    width,
                    operation,
                ) {
                    return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
                }
                state.flags = arithmetic_flags(&left, &right, None, is_subtraction, context);
            } else if matches!(instruction.mnemonic.as_str(), "adcq" | "sbbq") {
                let carry_in = match &state.flags {
                    FlagState::Compare { left, right, .. } => left.bvult(right),
                    FlagState::Arithmetic { carry, .. } => carry.clone(),
                    _ => return Err(FunctionOutcome::OutsideProfile("unsupported_flags")),
                };
                let width = instruction_width(&instruction.mnemonic)?;
                let (left, right) =
                    binary_operands(&instruction.operands, &state.registers, context, width)?;
                let is_subtraction = instruction.mnemonic == "sbbq";
                let operands = split_operands(&instruction.operands);
                let [_, destination] = operands.as_slice() else {
                    return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
                };
                let (index, destination_width) = parse_register(destination)
                    .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
                if destination_width != width || index == 4 {
                    return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
                }
                let carry_value = carry_in.ite(
                    &BV::from_u64(context, 1, width),
                    &BV::from_u64(context, 0, width),
                );
                let value = if is_subtraction {
                    left.bvsub(&right).bvsub(&carry_value)
                } else {
                    left.bvadd(&right).bvadd(&carry_value)
                };
                if !write_register(&mut state.registers, index, value.clone(), width) {
                    return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
                }
                state.flags =
                    arithmetic_flags(&left, &right, Some(&carry_in), is_subtraction, context);
            } else if matches!(instruction.mnemonic.as_str(), "xorl" | "xorq") {
                let width = instruction_width(&instruction.mnemonic)?;
                state.flags = lift_bitwise_flags(
                    &instruction.operands,
                    &mut state.registers,
                    context,
                    width,
                    BitwiseOp::Xor,
                )?;
            } else if matches!(instruction.mnemonic.as_str(), "andl" | "andq") {
                let width = instruction_width(&instruction.mnemonic)?;
                state.flags = lift_bitwise_flags(
                    &instruction.operands,
                    &mut state.registers,
                    context,
                    width,
                    BitwiseOp::And,
                )?;
            } else if matches!(instruction.mnemonic.as_str(), "orl" | "orq") {
                let width = instruction_width(&instruction.mnemonic)?;
                state.flags = lift_bitwise_flags(
                    &instruction.operands,
                    &mut state.registers,
                    context,
                    width,
                    BitwiseOp::Or,
                )?;
            } else if is_double_shift_mnemonic(&instruction.mnemonic) {
                let previous_flags = state.flags.clone();
                state.flags = lift_double_shift(
                    instruction,
                    &mut state.registers,
                    &previous_flags,
                    model,
                    &state.path_conditions,
                    &arguments,
                    context,
                )
                .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
            } else if is_shift_mnemonic(&instruction.mnemonic) {
                let previous_flags = state.flags.clone();
                state.flags = lift_shift(
                    instruction,
                    &mut state.registers,
                    &previous_flags,
                    model,
                    &state.path_conditions,
                    &arguments,
                    context,
                )
                .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
            } else if instruction.mnemonic.starts_with("set") {
                lift_setcc(instruction, &mut state, context)?;
            } else if instruction.mnemonic.starts_with("cmov") {
                lift_cmov(instruction, &mut state, context)?;
            } else {
                let leaves_flags_unchanged = matches!(
                    instruction.mnemonic.as_str(),
                    "movb"
                        | "movl"
                        | "movq"
                        | "movabsq"
                        | "movzbl"
                        | "movzbq"
                        | "movsbl"
                        | "movsbq"
                        | "leaq"
                        | "notl"
                        | "notq"
                        | "movsd"
                        | "movapd"
                        | "movaps"
                        | "addsd"
                        | "subsd"
                        | "mulsd"
                        | "divsd"
                        | "cmpeqsd"
                        | "xorps"
                        | "xorpd"
                );
                if !lift_instruction(
                    instruction,
                    &mut state.registers,
                    &mut state.xmm_registers,
                    move_source_upper_32_zero,
                    model_has_f64_leaves(model),
                    context,
                ) {
                    return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
                }
                if !leaves_flags_unchanged {
                    state.flags = FlagState::Unknown;
                }
            }
            state.known_upper_32_zero = [false; 16];
            match instruction.mnemonic.as_str() {
                "divl" => {
                    state.known_upper_32_zero[0] = true;
                    state.known_upper_32_zero[2] = true;
                }
                "movl" => {
                    if let Some((index, 32)) = split_operands(&instruction.operands)
                        .get(1)
                        .and_then(|destination| parse_register(destination))
                    {
                        state.known_upper_32_zero[index] = true;
                    }
                }
                "incl" | "decl" => {
                    if let Some((index, 32)) = split_operands(&instruction.operands)
                        .first()
                        .and_then(|destination| parse_register(destination))
                    {
                        state.known_upper_32_zero[index] = true;
                    }
                }
                _ => (),
            }
            state
                .outgoing_cleanup_slots
                .retain(|stack_slot| *stack_slot >= state.rsp_offset);
            if state
                .frame_region
                .is_some_and(|(lo, _)| state.rsp_offset > lo)
            {
                state.frame_region = None;
                state.frame_region_closed = true;
            }
            state.instruction_index += 1;
        }
    }
    if paths.is_empty() {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let used_call_targets = paths
        .iter()
        .flat_map(|path| path.used_call_targets.iter().copied())
        .collect::<BTreeSet<_>>();
    if used_call_targets.len() != call_targets.len() {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let frame_memory_use = FrameMemoryUse {
        red_zone: paths.iter().any(|path| path.used_frame_memory),
        allocated: paths.iter().any(|path| path.used_allocated_frame_memory),
    };
    Ok(LiftedFunction {
        arguments,
        callee_saved_initial,
        paths,
        frame_memory_use,
    })
}

fn is_unconditional_jump(mnemonic: &str) -> bool {
    matches!(mnemonic, "jmp" | "jmpq")
}

fn is_conditional_branch(mnemonic: &str) -> bool {
    mnemonic.starts_with('j') && !is_unconditional_jump(mnemonic)
}

fn is_tail_call(instruction: &Instruction, call_targets: &BTreeMap<u64, CallTarget>) -> bool {
    is_unconditional_jump(&instruction.mnemonic)
        && instruction
            .offset
            .checked_add(1)
            .is_some_and(|offset| call_targets.contains_key(&offset))
}

fn control_flow_successors(
    instructions: &[Instruction],
    branch_targets: &BTreeMap<usize, usize>,
    call_targets: &BTreeMap<u64, CallTarget>,
) -> Vec<Vec<usize>> {
    let mut successors = vec![Vec::new(); instructions.len()];
    for (index, instruction) in instructions.iter().enumerate() {
        if is_conditional_branch(&instruction.mnemonic) {
            if let Some(target) = branch_targets.get(&index) {
                successors[index].push(*target);
            }
            if index + 1 < instructions.len() {
                successors[index].push(index + 1);
            }
        } else if is_unconditional_jump(&instruction.mnemonic) {
            if !is_tail_call(instruction, call_targets) {
                if let Some(target) = branch_targets.get(&index) {
                    successors[index].push(*target);
                }
            }
        } else if instruction.mnemonic != "retq" && index + 1 < instructions.len() {
            successors[index].push(index + 1);
        }
    }
    successors
}

fn reachable_instructions(successors: &[Vec<usize>]) -> Vec<bool> {
    let mut reachable = vec![false; successors.len()];
    if successors.is_empty() {
        return reachable;
    }
    let mut pending = vec![0];
    while let Some(index) = pending.pop() {
        if reachable[index] {
            continue;
        }
        reachable[index] = true;
        pending.extend(successors[index].iter().copied());
    }
    reachable
}

fn reachable_control_flow_has_cycle(successors: &[Vec<usize>], reachable: &[bool]) -> bool {
    if successors.is_empty() || !reachable[0] {
        return false;
    }
    let mut states = vec![0u8; successors.len()];
    states[0] = 1;
    let mut pending = vec![(0usize, 0usize)];
    while let Some(&(index, edge_index)) = pending.last() {
        if edge_index == successors[index].len() {
            states[index] = 2;
            pending.pop();
            continue;
        }
        pending.last_mut().unwrap().1 += 1;
        let target = successors[index][edge_index];
        if !reachable[target] {
            continue;
        }
        match states[target] {
            1 => return true,
            0 => {
                states[target] = 1;
                pending.push((target, 0));
            }
            _ => {}
        }
    }
    false
}

fn resolve_branch_targets(
    instructions: &[Instruction],
    function_size: u64,
    call_targets: &BTreeMap<u64, CallTarget>,
) -> Result<BTreeMap<usize, usize>, FunctionOutcome> {
    let instruction_indices = instructions
        .iter()
        .enumerate()
        .map(|(index, instruction)| (instruction.offset, index))
        .collect::<BTreeMap<_, _>>();
    let mut targets = BTreeMap::new();
    for (index, instruction) in instructions.iter().enumerate() {
        let conditional = is_conditional_branch(&instruction.mnemonic);
        if !conditional && !is_unconditional_jump(&instruction.mnemonic) {
            continue;
        }
        if is_tail_call(instruction, call_targets) {
            continue;
        }
        if conditional
            && !instruction
                .mnemonic
                .strip_prefix('j')
                .is_some_and(is_supported_condition)
        {
            return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
        }
        let displacement = branch_target_displacement(instruction)
            .ok_or(FunctionOutcome::OutsideProfile("branch_target"))?;
        let next_offset = instructions
            .get(index + 1)
            .map_or(function_size, |next| next.offset);
        let target_offset = i128::from(next_offset) + i128::from(displacement);
        if target_offset < 0 || target_offset >= i128::from(function_size) {
            return Err(FunctionOutcome::OutsideProfile("branch_target"));
        }
        let target_offset = u64::try_from(target_offset)
            .map_err(|_| FunctionOutcome::OutsideProfile("branch_target"))?;
        let Some(target_index) = instruction_indices.get(&target_offset) else {
            return Err(FunctionOutcome::OutsideProfile("branch_target"));
        };
        targets.insert(index, *target_index);
    }
    Ok(targets)
}

fn branch_target_displacement(instruction: &Instruction) -> Option<i64> {
    let operands = split_operands(&instruction.operands);
    let [operand] = operands.as_slice() else {
        return None;
    };
    let operand = operand.split_whitespace().next()?;
    let displacement = parse_integer(operand)?;
    if operand.starts_with('-') {
        Some(displacement as i64)
    } else {
        i64::try_from(displacement).ok()
    }
}

fn is_supported_condition(suffix: &str) -> bool {
    matches!(
        suffix,
        "e" | "z"
            | "ne"
            | "nz"
            | "l"
            | "nge"
            | "le"
            | "ng"
            | "g"
            | "nle"
            | "ge"
            | "nl"
            | "b"
            | "c"
            | "nae"
            | "be"
            | "na"
            | "a"
            | "nbe"
            | "ae"
            | "nb"
            | "nc"
            | "s"
            | "ns"
            | "p"
            | "pe"
            | "np"
            | "po"
    )
}

fn condition_for_flags<'ctx>(
    flags: &FlagState<'ctx>,
    suffix: &str,
    context: &'ctx Context,
) -> Result<Bool<'ctx>, FunctionOutcome> {
    if !is_supported_condition(suffix) {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let is_parity_condition = matches!(suffix, "p" | "pe" | "np" | "po");
    if is_parity_condition && !matches!(flags, FlagState::FloatCompare { .. }) {
        return Err(FunctionOutcome::OutsideProfile("unsupported_flags"));
    }
    let (zero, sign, signed_less, unsigned_less) = match flags {
        FlagState::Unknown => {
            return Err(FunctionOutcome::OutsideProfile("unsupported_flags"));
        }
        FlagState::FloatCompare { left, right } => {
            let unordered = Bool::or(
                context,
                &[
                    &mumei_core::semantic_contract::f64_is_nan(context, left),
                    &mumei_core::semantic_contract::f64_is_nan(context, right),
                ],
            );
            let equal = mumei_core::semantic_contract::f64_eq(context, left, right);
            let less = mumei_core::semantic_contract::f64_lt(context, left, right);
            let zero = Bool::or(context, &[&unordered, &equal]);
            let carry = Bool::or(context, &[&unordered, &less]);
            let not_zero = zero.not();
            let not_carry = carry.not();
            return Ok(match suffix {
                "e" | "z" => zero,
                "ne" | "nz" => not_zero.clone(),
                "b" | "c" | "nae" => carry.clone(),
                "ae" | "nb" | "nc" => not_carry.clone(),
                "be" | "na" => Bool::or(context, &[&carry, &zero]),
                "a" | "nbe" => Bool::and(context, &[&not_carry, &not_zero]),
                "p" | "pe" => unordered,
                "np" | "po" => unordered.not(),
                "s" | "l" | "nge" => Bool::from_bool(context, false),
                "ns" | "ge" | "nl" => Bool::from_bool(context, true),
                "le" | "ng" => zero,
                "g" | "nle" => not_zero,
                _ => return Err(FunctionOutcome::OutsideProfile("unsupported_instruction")),
            });
        }
        FlagState::Compare { left, right, width } => {
            let difference = left.bvsub(right);
            (
                left._eq(right),
                most_significant_bit(&difference, *width, context),
                left.bvslt(right),
                left.bvult(right),
            )
        }
        FlagState::Arithmetic {
            value,
            carry,
            overflow,
            width,
        } => {
            let zero = value._eq(&BV::from_u64(context, 0, *width));
            let sign = most_significant_bit(value, *width, context);
            let signed_less = sign._eq(overflow).not();
            (zero, sign, signed_less, carry.clone())
        }
        FlagState::Result { value, width } => {
            let zero = value._eq(&BV::from_u64(context, 0, *width));
            let sign = most_significant_bit(value, *width, context);
            (zero, sign.clone(), sign, Bool::from_bool(context, false))
        }
        FlagState::ZeroFlagResult { value, width } => {
            let zero = value._eq(&BV::from_u64(context, 0, *width));
            return match suffix {
                "e" | "z" => Ok(zero),
                "ne" | "nz" => Ok(zero.not()),
                _ => Err(FunctionOutcome::OutsideProfile("unsupported_flags")),
            };
        }
        FlagState::IncDecResult {
            value,
            overflow,
            width,
        } => {
            // inc/dec set SF/ZF/OF exactly but never update CF, so every
            // carry-consuming condition stays fail-closed rather than
            // borrowing a fabricated carry.
            let zero = value._eq(&BV::from_u64(context, 0, *width));
            let sign = most_significant_bit(value, *width, context);
            let signed_less = sign._eq(overflow).not();
            return Ok(match suffix {
                "e" | "z" => zero.clone(),
                "ne" | "nz" => zero.not(),
                "s" => sign.clone(),
                "ns" => sign.not(),
                "l" | "nge" => signed_less.clone(),
                "le" | "ng" => Bool::or(context, &[&signed_less, &zero]),
                "g" | "nle" => Bool::and(context, &[&signed_less.not(), &zero.not()]),
                "ge" | "nl" => signed_less.not(),
                _ => return Err(FunctionOutcome::OutsideProfile("unsupported_flags")),
            });
        }
    };
    let not_zero = zero.not();
    let not_signed_less = signed_less.not();
    let not_unsigned_less = unsigned_less.not();
    Ok(match suffix {
        "e" | "z" => zero,
        "ne" | "nz" => not_zero,
        "l" | "nge" => signed_less,
        "le" | "ng" => Bool::or(context, &[&signed_less, &zero]),
        "g" | "nle" => Bool::and(context, &[&not_signed_less, &not_zero]),
        "ge" | "nl" => not_signed_less,
        "b" | "c" | "nae" => unsigned_less,
        "be" | "na" => Bool::or(context, &[&unsigned_less, &zero]),
        "a" | "nbe" => Bool::and(context, &[&not_unsigned_less, &not_zero]),
        "ae" | "nb" | "nc" => not_unsigned_less,
        "s" => sign,
        "ns" => sign.not(),
        _ => return Err(FunctionOutcome::OutsideProfile("unsupported_instruction")),
    })
}

fn arithmetic_flags<'ctx>(
    left: &BV<'ctx>,
    right: &BV<'ctx>,
    carry_in: Option<&Bool<'ctx>>,
    is_subtraction: bool,
    context: &'ctx Context,
) -> FlagState<'ctx> {
    let width = left.get_size();
    let carry_wide = carry_in
        .map(|carry| {
            carry.ite(
                &BV::from_u64(context, 1, width + 1),
                &BV::from_u64(context, 0, width + 1),
            )
        })
        .unwrap_or_else(|| BV::from_u64(context, 0, width + 1));
    let carry_narrow = carry_in
        .map(|carry| {
            carry.ite(
                &BV::from_u64(context, 1, width),
                &BV::from_u64(context, 0, width),
            )
        })
        .unwrap_or_else(|| BV::from_u64(context, 0, width));
    let value = match (is_subtraction, carry_in) {
        (true, Some(_)) => left.bvsub(right).bvsub(&carry_narrow),
        (false, Some(_)) => left.bvadd(right).bvadd(&carry_narrow),
        (true, None) => left.bvsub(right),
        (false, None) => left.bvadd(right),
    };
    let unsigned = if is_subtraction {
        if let Some(carry) = carry_in {
            Bool::or(
                context,
                &[
                    &left.bvult(right),
                    &Bool::and(context, &[carry, &left._eq(right)]),
                ],
            )
        } else {
            left.bvult(right)
        }
    } else {
        let full = left
            .zero_ext(1)
            .bvadd(&right.zero_ext(1))
            .bvadd(&carry_wide);
        full.extract(width, width)._eq(&BV::from_u64(context, 1, 1))
    };
    let signed = match (is_subtraction, carry_in) {
        (true, Some(_)) => left
            .sign_ext(1)
            .bvsub(&right.sign_ext(1))
            .bvsub(&carry_wide),
        (false, Some(_)) => left
            .sign_ext(1)
            .bvadd(&right.sign_ext(1))
            .bvadd(&carry_wide),
        (true, None) => left.sign_ext(1).bvsub(&right.sign_ext(1)),
        (false, None) => left.sign_ext(1).bvadd(&right.sign_ext(1)),
    };
    let overflow = signed._eq(&value.sign_ext(1)).not();
    FlagState::Arithmetic {
        value,
        carry: unsigned,
        overflow,
        width,
    }
}

fn most_significant_bit<'ctx>(value: &BV<'ctx>, width: u32, context: &'ctx Context) -> Bool<'ctx> {
    value
        .extract(width - 1, width - 1)
        ._eq(&BV::from_u64(context, 1, 1))
}

fn finish_path<'ctx>(
    state: MachineState<'ctx>,
    model: &PureI64Body<'ctx>,
    arguments: &[BV<'ctx>],
    context: &'ctx Context,
) -> Result<LiftedPath<'ctx>, FunctionOutcome> {
    if !state.rsp_is_known {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    for call_site in model.call_sites.iter().skip(state.call_state.call_index) {
        if call_guard_status(
            model,
            &call_site.guard,
            &state.path_conditions,
            arguments,
            context,
        ) != Some(false)
        {
            return Err(FunctionOutcome::OutsideProfile("call_order"));
        }
    }
    let return_values = if state.sret_pointer.is_some() {
        // A memory-class result comes back as the leaves the callee
        // stored through the sret pointer, leaf `i` at byte offset
        // `8 * i`. A slot the path never stored still holds whatever
        // the caller left — a fresh constant — so an unwritten leaf
        // compares `Different` against the determined source result.
        let leaf_count = model.result_shape.unwrap_or_default();
        (0..leaf_count)
            .map(|leaf| {
                state
                    .sret_stores
                    .iter()
                    .rev()
                    .find(|(offset, _)| *offset == 8 * leaf as i64)
                    .map(|(_, value)| value.clone())
                    .unwrap_or_else(|| {
                        BV::fresh_const(context, "machine_sret_unwritten_result_leaf", 64)
                    })
            })
            .collect()
    } else if let Some(leaf_count) = model.result_shape {
        let mut integer_result_index = 0usize;
        let mut float_result_index = 0usize;
        model
            .result_leaf_doubles
            .iter()
            .take(leaf_count)
            .map(|is_double| {
                if *is_double {
                    let result = state.xmm_registers[float_result_index].extract(63, 0);
                    float_result_index += 1;
                    result
                } else {
                    let register_index = INTEGER_RESULT_REGISTERS[integer_result_index];
                    integer_result_index += 1;
                    state.registers[register_index].clone()
                }
            })
            .collect()
    } else if model.value.get_size() == 128 {
        vec![state.registers[2]
            .clone()
            .concat(&state.registers[0].clone())]
    } else {
        vec![if model_result_is_double(model) {
            state.xmm_registers[0].extract(63, 0)
        } else {
            state.registers[0].clone()
        }]
    };
    let callee_saved_registers = CALLEE_SAVED_REGISTERS
        .iter()
        .map(|register_index| state.registers[*register_index].clone())
        .collect();
    // SysV's sret convention also requires the callee to hand the
    // result pointer back in `rax` — a callee that stores every leaf
    // but returns a different `rax` breaks the ABI.
    let sret_pointer_obligation = state
        .sret_pointer
        .as_ref()
        .map(|pointer| state.registers[0]._eq(pointer));
    Ok(LiftedPath {
        return_values,
        sret_pointer_obligation,
        callee_saved_registers,
        final_rsp_offset: state.rsp_offset,
        path_conditions: state.path_conditions,
        call_argument_obligations: state.call_state.argument_obligations,
        exact_f64_call_argument_obligations: state.call_state.exact_f64_argument_obligations,
        used_call_targets: state.call_state.used_call_targets,
        used_frame_memory: state.used_frame_memory,
        used_allocated_frame_memory: state.used_allocated_frame_memory,
    })
}

fn instruction_width(mnemonic: &str) -> Result<u32, FunctionOutcome> {
    if mnemonic.ends_with('b') {
        Ok(8)
    } else if mnemonic.ends_with('l') {
        Ok(32)
    } else if mnemonic.ends_with('q') {
        Ok(64)
    } else {
        Err(FunctionOutcome::OutsideProfile("unsupported_instruction"))
    }
}

fn binary_operands<'ctx>(
    operands: &str,
    registers: &[BV<'ctx>],
    context: &'ctx Context,
    width: u32,
) -> Result<(BV<'ctx>, BV<'ctx>), FunctionOutcome> {
    let operands = split_operands(operands);
    let [source, destination] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    let left = register_value(destination, registers, width)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    let right = operand_value(source, registers, context, width)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    Ok((left, right))
}

fn compare_flags<'ctx>(
    operands: &str,
    registers: &[BV<'ctx>],
    context: &'ctx Context,
    width: u32,
) -> Result<FlagState<'ctx>, FunctionOutcome> {
    let (left, right) = binary_operands(operands, registers, context, width)?;
    Ok(FlagState::Compare { left, right, width })
}

fn float_compare_flags<'ctx>(
    operands: &str,
    xmm_registers: &[BV<'ctx>],
) -> Result<FlagState<'ctx>, FunctionOutcome> {
    let operands = split_operands(operands);
    let [source, destination] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    let (Some(right_index), Some(left_index)) =
        (parse_xmm_register(source), parse_xmm_register(destination))
    else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    Ok(FlagState::FloatCompare {
        left: xmm_registers[left_index].extract(63, 0),
        right: xmm_registers[right_index].extract(63, 0),
    })
}

fn test_flags<'ctx>(
    operands: &str,
    registers: &[BV<'ctx>],
    context: &'ctx Context,
    width: u32,
) -> Result<FlagState<'ctx>, FunctionOutcome> {
    let (left, right) = binary_operands(operands, registers, context, width)?;
    Ok(FlagState::Result {
        value: left.bvand(&right),
        width,
    })
}

#[derive(Clone, Copy)]
enum BitwiseOp {
    And,
    Or,
    Xor,
}

fn lift_bitwise_flags<'ctx>(
    operands: &str,
    registers: &mut [BV<'ctx>],
    context: &'ctx Context,
    width: u32,
    operation: BitwiseOp,
) -> Result<FlagState<'ctx>, FunctionOutcome> {
    let operands = split_operands(operands);
    let [source, destination] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    let (index, destination_width) = parse_register(destination)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    if destination_width != width || index == 4 {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let left = register_value(destination, registers, width)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    let right = operand_value(source, registers, context, width)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    let value = match operation {
        BitwiseOp::And => left.bvand(&right),
        BitwiseOp::Or => left.bvor(&right),
        BitwiseOp::Xor => left.bvxor(&right),
    };
    if !write_register(registers, index, value.clone(), width) {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    Ok(FlagState::Result { value, width })
}

fn lift_setcc<'ctx>(
    instruction: &Instruction,
    state: &mut MachineState<'ctx>,
    context: &'ctx Context,
) -> Result<(), FunctionOutcome> {
    let suffix = instruction
        .mnemonic
        .strip_prefix("set")
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    let condition = condition_for_flags(&state.flags, suffix, context)?;
    let operands = split_operands(&instruction.operands);
    let [destination] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    let (index, width) = parse_register(destination)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    if width != 8 {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let value = condition
        .ite(&BV::from_u64(context, 1, 1), &BV::from_u64(context, 0, 1))
        .zero_ext(7);
    if !write_register(&mut state.registers, index, value, 8) {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    if index == 4 {
        state.rsp_is_known = false;
    }
    Ok(())
}

fn is_shift_mnemonic(mnemonic: &str) -> bool {
    matches!(
        mnemonic,
        "shlq" | "shll" | "shrq" | "shrl" | "sarq" | "sarl"
    )
}

#[derive(Clone, Copy)]
enum ShiftOp {
    Left,
    LogicalRight,
    ArithmeticRight,
}

fn lift_shift<'ctx>(
    instruction: &Instruction,
    registers: &mut [BV<'ctx>],
    previous_flags: &FlagState<'ctx>,
    model: &PureI64Body<'ctx>,
    path_conditions: &[Bool<'ctx>],
    arguments: &[BV<'ctx>],
    context: &'ctx Context,
) -> Option<FlagState<'ctx>> {
    let (width, operation) = match instruction.mnemonic.as_str() {
        "shlq" => (64, ShiftOp::Left),
        "shll" => (32, ShiftOp::Left),
        "shrq" => (64, ShiftOp::LogicalRight),
        "shrl" => (32, ShiftOp::LogicalRight),
        "sarq" => (64, ShiftOp::ArithmeticRight),
        "sarl" => (32, ShiftOp::ArithmeticRight),
        _ => return None,
    };
    let operands = split_operands(&instruction.operands);
    let (immediate_count, destination) = match operands.as_slice() {
        [destination] => (Some(1), *destination),
        [count, destination] => match count.strip_prefix('$').and_then(parse_integer) {
            Some(count) => (Some(count), *destination),
            None if *count == "%cl" => (None, *destination),
            None => return None,
        },
        _ => return None,
    };
    let (index, destination_width) = parse_register(destination)?;
    if destination_width != width || index == 4 {
        return None;
    }
    let count_mask = if width == 64 { 0x3f } else { 0x1f };
    let masked_immediate = immediate_count.map(|count| count & count_mask);
    let shift_count = if let Some(count) = masked_immediate {
        BV::from_u64(context, count, width)
    } else {
        registers[1]
            .extract(7, 0)
            .bvand(&BV::from_u64(context, count_mask, 8))
            .zero_ext(width - 8)
    };
    let value = if width == 64 {
        registers[index].clone()
    } else {
        registers[index].extract(width - 1, 0)
    };
    let shifted = match operation {
        ShiftOp::Left => value.bvshl(&shift_count),
        ShiftOp::LogicalRight => value.bvlshr(&shift_count),
        ShiftOp::ArithmeticRight => value.bvashr(&shift_count),
    };
    let flags = match masked_immediate {
        Some(0) => previous_flags.clone(),
        Some(_) => FlagState::ZeroFlagResult {
            value: shifted.clone(),
            width,
        },
        None => {
            let count_is_zero = shift_count._eq(&BV::from_u64(context, 0, width));
            match call_guard_status(model, &count_is_zero, path_conditions, arguments, context) {
                Some(true) => previous_flags.clone(),
                Some(false) => FlagState::ZeroFlagResult {
                    value: shifted.clone(),
                    width,
                },
                None => FlagState::Unknown,
            }
        }
    };
    write_register(registers, index, shifted, width).then_some(flags)
}

fn is_double_shift_mnemonic(mnemonic: &str) -> bool {
    matches!(mnemonic, "shld" | "shldq" | "shrd" | "shrdq")
}

/// `shld`/`shrd`, the double-precision shift pair LLVM emits for a
/// variable `i128` shift (`shl`/`lshr`/`ashr` on a two-word value —
/// the surrounding `test %cl,0x40`/`cmov` fallback sequence is already
/// modeled). `shldq c, src, dest` reads the 128-bit `dest:src`
/// concatenation shifted left and keeps the high word;
/// `shrdq c, src, dest` reads `src:dest` shifted right and keeps the
/// low word. The count is masked to 6 bits by the hardware; count 0
/// leaves the destination and the flags untouched. Only the 64-bit
/// forms are lifted — the `i128` surface never needs narrower pairs.
fn lift_double_shift<'ctx>(
    instruction: &Instruction,
    registers: &mut [BV<'ctx>],
    previous_flags: &FlagState<'ctx>,
    model: &PureI64Body<'ctx>,
    path_conditions: &[Bool<'ctx>],
    arguments: &[BV<'ctx>],
    context: &'ctx Context,
) -> Option<FlagState<'ctx>> {
    let left = matches!(instruction.mnemonic.as_str(), "shld" | "shldq");
    let operands = split_operands(&instruction.operands);
    let (immediate_count, source, destination) = match operands.as_slice() {
        [count, source, destination] => match count.strip_prefix('$').and_then(parse_integer) {
            Some(count) => (Some(count), *source, *destination),
            None if *count == "%cl" => (None, *source, *destination),
            None => return None,
        },
        _ => return None,
    };
    let (destination_index, destination_width) = parse_register(destination)?;
    let (source_index, source_width) = parse_register(source)?;
    if destination_width != 64 || source_width != 64 || destination_index == 4 {
        return None;
    }
    let masked_immediate = immediate_count.map(|count| count & 0x3f);
    let shift_count = if let Some(count) = masked_immediate {
        BV::from_u64(context, count, 128)
    } else {
        registers[1]
            .extract(7, 0)
            .bvand(&BV::from_u64(context, 0x3f, 8))
            .zero_ext(120)
    };
    let pair = if left {
        registers[destination_index].concat(&registers[source_index])
    } else {
        registers[source_index].concat(&registers[destination_index])
    };
    let shifted = if left {
        pair.bvshl(&shift_count)
    } else {
        pair.bvlshr(&shift_count)
    };
    let value = if left {
        shifted.extract(127, 64)
    } else {
        shifted.extract(63, 0)
    };
    let flags = match masked_immediate {
        Some(0) => previous_flags.clone(),
        Some(_) => FlagState::ZeroFlagResult {
            value: value.clone(),
            width: 64,
        },
        None => {
            let count_is_zero = shift_count._eq(&BV::from_u64(context, 0, 128));
            match call_guard_status(model, &count_is_zero, path_conditions, arguments, context) {
                Some(true) => previous_flags.clone(),
                Some(false) => FlagState::ZeroFlagResult {
                    value: value.clone(),
                    width: 64,
                },
                None => FlagState::Unknown,
            }
        }
    };
    write_register(registers, destination_index, value, 64).then_some(flags)
}

fn lift_cmov<'ctx>(
    instruction: &Instruction,
    state: &mut MachineState<'ctx>,
    context: &'ctx Context,
) -> Result<(), FunctionOutcome> {
    let (suffix, width) = instruction
        .mnemonic
        .strip_prefix("cmov")
        .and_then(|rest| {
            rest.strip_suffix('l')
                .map(|suffix| (suffix, 32))
                .or_else(|| rest.strip_suffix('q').map(|suffix| (suffix, 64)))
        })
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    let condition = condition_for_flags(&state.flags, suffix, context)?;
    let operands = split_operands(&instruction.operands);
    let [source, destination] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    let source = register_value(source, &state.registers, width)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    let (index, destination_width) = parse_register(destination)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    if destination_width != width || index == 4 {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let previous = if width == 64 {
        state.registers[index].clone()
    } else {
        state.registers[index].extract(width - 1, 0)
    };
    if !write_register(
        &mut state.registers,
        index,
        condition.ite(&source, &previous),
        width,
    ) {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    Ok(())
}

fn lift_rsp_adjustment<'ctx>(
    instruction: &Instruction,
    state: &mut MachineState<'ctx>,
    allocated_frame_memory_mode: bool,
    context: &'ctx Context,
) -> Result<(), FunctionOutcome> {
    let operands = split_operands(&instruction.operands);
    let [immediate, destination] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    if *destination != "%rsp" {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    if !state.rsp_is_known || state.rsp_offset % 8 != 0 {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let amount = immediate
        .strip_prefix('$')
        .and_then(parse_integer)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    let amount_signed = amount as i64;
    let delta = if instruction.mnemonic == "addq" {
        amount_signed
    } else {
        amount_signed
            .checked_neg()
            .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?
    };
    let previous_rsp = state.registers[4].clone();
    if !adjust_rsp(
        &mut state.registers,
        &mut state.stack,
        &mut state.rsp_offset,
        delta,
        context,
    ) {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    if allocated_frame_memory_mode
        && instruction.mnemonic == "subq"
        && amount_signed > 0
        && amount_signed % 8 == 0
        && state.frame_region.is_none()
        && !state.frame_region_closed
    {
        let hi = state
            .rsp_offset
            .checked_add(amount_signed)
            .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
        state.frame_region = Some((state.rsp_offset, hi));
    }
    state.flags = arithmetic_flags(
        &previous_rsp,
        &BV::from_u64(context, amount, 64),
        None,
        instruction.mnemonic == "subq",
        context,
    );
    Ok(())
}

fn parse_rsp_memory_displacement(operand: &str) -> Option<i64> {
    let open = operand.find('(')?;
    let close = operand.rfind(')')?;
    if close + 1 != operand.len() || operand[open + 1..close].trim() != "%rsp" {
        return None;
    }
    let displacement = operand[..open].trim();
    if displacement.is_empty() {
        return Some(0);
    }
    let value = parse_integer(displacement)?;
    if displacement.starts_with('-') {
        Some(value as i64)
    } else {
        i64::try_from(value).ok()
    }
}

fn rsp_memory_entry_offset(operand: &str, rsp_offset: i64) -> Result<i64, FunctionOutcome> {
    let displacement = parse_rsp_memory_displacement(operand)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    rsp_offset
        .checked_add(displacement)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))
}

/// Resolves an `N(%reg)` operand whose base register still holds the
/// entry `sret` pointer — the identical BV AST, so a `mov %rdi,%rax`
/// keeps the recognition while overwriting the register drops it — to
/// the byte offset inside the caller-owned result buffer it addresses.
/// Anything else (`%rsp` bases, displaced bases, offsets outside the
/// buffer) does not qualify and falls through to the ordinary
/// stack/frame paths.
fn sret_memory_offset<'ctx>(operand: &str, state: &MachineState<'ctx>) -> Option<i64> {
    let sret_pointer = state.sret_pointer.as_ref()?;
    let open = operand.find('(')?;
    let close = operand.rfind(')')?;
    if close + 1 != operand.len() {
        return None;
    }
    let (register_index, width) = parse_register(operand[open + 1..close].trim())?;
    if width != 64 || register_index == 4 {
        return None;
    }
    if state.registers[register_index].get_z3_ast() != sret_pointer.get_z3_ast() {
        return None;
    }
    let displacement = operand[..open].trim();
    let offset = if displacement.is_empty() {
        0
    } else {
        parse_integer(displacement)? as i64
    };
    (offset % 8 == 0 && offset >= 0).then_some(offset)
}

/// Records a `movq %r, N(%reg)` store through the sret pointer into the
/// result-buffer slot map — the callee's only way to write its
/// memory-class result. `movsd %xmm, N(%reg)` writes an `f64` leaf of a
/// mixed-class buffer: once INTEGER leaves overflow the whole
/// aggregate goes to memory, SSE leaves landing as bit-pattern stores.
fn lift_sret_store<'ctx>(
    instruction: &Instruction,
    state: &mut MachineState<'ctx>,
    context: &'ctx Context,
) -> Result<(), FunctionOutcome> {
    let operands = split_operands(&instruction.operands);
    let [source, destination] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    let offset = sret_memory_offset(destination, state)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    let value = if instruction.mnemonic == "movsd" {
        parse_xmm_register(source)
            .map(|index| state.xmm_registers[index].extract(63, 0))
            .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?
    } else {
        operand_value(source, &state.registers, context, 64)
            .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?
    };
    if let Some((_, stored)) = state
        .sret_stores
        .iter_mut()
        .find(|(slot, _)| *slot == offset)
    {
        *stored = value;
    } else {
        state.sret_stores.push((offset, value));
    }
    Ok(())
}

/// Loads a `movq N(%reg), %r` read through the sret pointer: a value
/// this path recorded, or a fresh constant for a slot the callee has
/// not written yet — that byte still holds whatever the caller left in
/// its buffer.
fn lift_sret_load<'ctx>(
    instruction: &Instruction,
    state: &mut MachineState<'ctx>,
    context: &'ctx Context,
) -> Result<(), FunctionOutcome> {
    let operands = split_operands(&instruction.operands);
    let [source, destination] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    let offset = sret_memory_offset(source, state)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    let value = state
        .sret_stores
        .iter()
        .find(|(slot, _)| *slot == offset)
        .map(|(_, value)| value.clone())
        .unwrap_or_else(|| BV::fresh_const(context, "machine_sret_unwritten_slot", 64));
    if instruction.mnemonic == "movsd" {
        let Some(destination_index) = parse_xmm_register(destination) else {
            return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
        };
        state.xmm_registers[destination_index] = BV::from_u64(context, 0, 64).concat(&value);
        return Ok(());
    }
    let (register_index, width) = parse_register(destination)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    if !write_register(&mut state.registers, register_index, value, width) {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    Ok(())
}

/// Resolves an `N(%reg)` operand whose base register still carries the
/// pointer an `sret` call was made with — the identical BV AST, so
/// `mov %rsp,%rdi` before the call and `rax` right after it both
/// qualify — to `(buffer index, byte offset inside the buffer)`. The
/// buffer's leaf slots live in the `stack` map at
/// `base_offset + offset`.
fn sret_call_memory_offset<'ctx>(
    operand: &str,
    state: &MachineState<'ctx>,
) -> Option<(usize, i64)> {
    if state.sret_call_buffers.is_empty() {
        return None;
    }
    let open = operand.find('(')?;
    let close = operand.rfind(')')?;
    if close + 1 != operand.len() {
        return None;
    }
    let (register_index, width) = parse_register(operand[open + 1..close].trim())?;
    if width != 64 || register_index == 4 {
        return None;
    }
    let displacement = operand[..open].trim();
    let offset = if displacement.is_empty() {
        0
    } else {
        parse_integer(displacement)? as i64
    };
    if offset % 8 != 0 || offset < 0 {
        return None;
    }
    let ast = state.registers[register_index].get_z3_ast();
    // Newest record first: LLVM reuses one frame slot for consecutive
    // `sret` buffers, and only the latest call's extent is live.
    state
        .sret_call_buffers
        .iter()
        .rposition(|buffer| buffer.pointer.get_z3_ast() == ast)
        .map(|index| (index, offset))
}

/// A `movq %r, N(%reg)` store through a caller-side `sret` buffer
/// pointer: writes into the recorded `stack` slot so both pointer- and
/// `rsp`-relative reads agree — a caller scribbling on its own buffer
/// overwrites the leaf the callee left.
fn lift_sret_call_store<'ctx>(
    instruction: &Instruction,
    state: &mut MachineState<'ctx>,
    context: &'ctx Context,
) -> Result<(), FunctionOutcome> {
    let operands = split_operands(&instruction.operands);
    let [source, destination] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    let (buffer_index, offset) = sret_call_memory_offset(destination, state)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    let buffer = &state.sret_call_buffers[buffer_index];
    if offset >= 8 * buffer.leaf_count as i64 {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let entry_offset = buffer.base_offset + offset;
    let value = if instruction.mnemonic == "movsd" {
        parse_xmm_register(source)
            .map(|index| state.xmm_registers[index].extract(63, 0))
            .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?
    } else {
        operand_value(source, &state.registers, context, 64)
            .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?
    };
    if let Some((_, stored)) = state
        .stack
        .iter_mut()
        .find(|(stack_slot, _)| *stack_slot == entry_offset)
    {
        *stored = value;
    } else {
        state.stack.push((entry_offset, value));
    }
    Ok(())
}

/// Loads a `movq N(%reg), %r` read through a caller-side `sret` buffer
/// pointer: the recorded `stack` slot — a leaf the callee wrote, or a
/// caller overwrite — else a fresh constant when the frame slot is
/// already unreachable (e.g. after `rsp` was restored).
fn lift_sret_call_load<'ctx>(
    instruction: &Instruction,
    state: &mut MachineState<'ctx>,
    context: &'ctx Context,
) -> Result<(), FunctionOutcome> {
    let operands = split_operands(&instruction.operands);
    let [source, destination] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    let (buffer_index, offset) = sret_call_memory_offset(source, state)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    let buffer = &state.sret_call_buffers[buffer_index];
    if offset >= 8 * buffer.leaf_count as i64 {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let entry_offset = buffer.base_offset + offset;
    let value = state
        .stack
        .iter()
        .find(|(stack_slot, _)| *stack_slot == entry_offset)
        .map(|(_, value)| value.clone())
        .unwrap_or_else(|| BV::fresh_const(context, "machine_sret_call_unwritten_slot", 64));
    if instruction.mnemonic == "movsd" {
        let Some(destination_index) = parse_xmm_register(destination) else {
            return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
        };
        state.xmm_registers[destination_index] = BV::from_u64(context, 0, 64).concat(&value);
        return Ok(());
    }
    let (register_index, width) = parse_register(destination)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    if !write_register(&mut state.registers, register_index, value, width) {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    Ok(())
}

/// A 128-bit `movups`/`movaps`/`movdqa`/`movdqu` store through the sret
/// pointer — LLVM's vectorized copy of two adjacent buffer leaves when
/// it forwards a memory-class result (an enum call result re-returned,
/// or any wide aggregate the callee copies). Each half records like a
/// scalar `movq` store; a packed copy moves raw bits.
fn lift_sret_wide_store<'ctx>(
    instruction: &Instruction,
    state: &mut MachineState<'ctx>,
) -> Result<(), FunctionOutcome> {
    let operands = split_operands(&instruction.operands);
    let [source, destination] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    let offset = sret_memory_offset(destination, state)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    let Some(register_index) = parse_xmm_register(source) else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    let value = state.xmm_registers[register_index].clone();
    for (slot, half) in [
        (offset, value.extract(63, 0)),
        (offset + 8, value.extract(127, 64)),
    ] {
        if let Some((_, stored)) = state
            .sret_stores
            .iter_mut()
            .find(|(stored_slot, _)| *stored_slot == slot)
        {
            *stored = half;
        } else {
            state.sret_stores.push((slot, half));
        }
    }
    Ok(())
}

/// A 128-bit `movups`/`movaps`/`movdqa`/`movdqu` load through the sret
/// pointer: two adjacent buffer leaves concat into the xmm register,
/// recorded stores first, fresh constants for unwritten slots.
fn lift_sret_wide_load<'ctx>(
    instruction: &Instruction,
    state: &mut MachineState<'ctx>,
    context: &'ctx Context,
) -> Result<(), FunctionOutcome> {
    let operands = split_operands(&instruction.operands);
    let [source, destination] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    let offset = sret_memory_offset(source, state)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    let Some(destination_index) = parse_xmm_register(destination) else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    let leaf = |slot: i64| {
        state
            .sret_stores
            .iter()
            .find(|(stored_slot, _)| *stored_slot == slot)
            .map(|(_, value)| value.clone())
            .unwrap_or_else(|| BV::fresh_const(context, "machine_sret_unwritten_slot", 64))
    };
    state.xmm_registers[destination_index] = leaf(offset + 8).concat(&leaf(offset));
    Ok(())
}

/// A 128-bit `movups`/`movaps`/`movdqa`/`movdqu` store through a
/// caller-side `sret` buffer pointer: writes both halves into the
/// recorded `stack` slots so pointer- and `rsp`-relative reads agree.
fn lift_sret_call_wide_store<'ctx>(
    instruction: &Instruction,
    state: &mut MachineState<'ctx>,
) -> Result<(), FunctionOutcome> {
    let operands = split_operands(&instruction.operands);
    let [source, destination] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    let (buffer_index, offset) = sret_call_memory_offset(destination, state)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    let buffer = &state.sret_call_buffers[buffer_index];
    if offset + 8 >= 8 * buffer.leaf_count as i64 {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let base_offset = buffer.base_offset;
    let Some(register_index) = parse_xmm_register(source) else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    let value = state.xmm_registers[register_index].clone();
    for (slot, half) in [
        (base_offset + offset, value.extract(63, 0)),
        (base_offset + offset + 8, value.extract(127, 64)),
    ] {
        if let Some((_, stored)) = state
            .stack
            .iter_mut()
            .find(|(stack_slot, _)| *stack_slot == slot)
        {
            *stored = half;
        } else {
            state.stack.push((slot, half));
        }
    }
    Ok(())
}

/// A 128-bit `movups`/`movaps`/`movdqa`/`movdqu` load through a
/// caller-side `sret` buffer pointer: two adjacent `stack` leaves
/// concat into the xmm register, fresh constants where unreachable.
fn lift_sret_call_wide_load<'ctx>(
    instruction: &Instruction,
    state: &mut MachineState<'ctx>,
    context: &'ctx Context,
) -> Result<(), FunctionOutcome> {
    let operands = split_operands(&instruction.operands);
    let [source, destination] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    let (buffer_index, offset) = sret_call_memory_offset(source, state)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    let buffer = &state.sret_call_buffers[buffer_index];
    if offset + 8 >= 8 * buffer.leaf_count as i64 {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let base_offset = buffer.base_offset;
    let Some(destination_index) = parse_xmm_register(destination) else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    let leaf = |slot: i64| {
        state
            .stack
            .iter()
            .find(|(stack_slot, _)| *stack_slot == slot)
            .map(|(_, value)| value.clone())
            .unwrap_or_else(|| BV::fresh_const(context, "machine_sret_call_unwritten_slot", 64))
    };
    state.xmm_registers[destination_index] =
        leaf(base_offset + offset + 8).concat(&leaf(base_offset + offset));
    Ok(())
}

/// Resolves an `N(%rsp)` operand above the current frame to the input
/// BV of the N-th spilled argument leaf. `want_double` is the class of
/// the instruction doing the access — an INTEGER load of a spilled
/// `f64` leaf (or an SSE load of a spilled integer leaf) binds the
/// wrong input shape and stays closed.
fn incoming_stack_argument<'ctx>(
    operand: &str,
    rsp_offset: i64,
    stack_argument_leaves: &[(usize, bool)],
    arguments: &[BV<'ctx>],
    want_double: bool,
) -> Result<BV<'ctx>, FunctionOutcome> {
    if rsp_offset % 8 != 0 {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let entry_offset = rsp_memory_entry_offset(operand, rsp_offset)?;
    if entry_offset <= 0 || entry_offset % 8 != 0 {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let slot_index = usize::try_from(entry_offset / 8)
        .map_err(|_| FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    if slot_index == 0 || slot_index > stack_argument_leaves.len() {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let (input_index, is_double) = stack_argument_leaves[slot_index - 1];
    if is_double != want_double {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    arguments
        .get(input_index)
        .cloned()
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))
}

fn lift_stack_store<'ctx>(
    instruction: &Instruction,
    state: &mut MachineState<'ctx>,
    context: &'ctx Context,
) -> Result<(), FunctionOutcome> {
    let operands = split_operands(&instruction.operands);
    let [source, destination] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    if !state.rsp_is_known {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let entry_offset = rsp_memory_entry_offset(destination, state.rsp_offset)?;
    if state.rsp_offset % 8 != 0
        || entry_offset >= 0
        || entry_offset < state.rsp_offset
        || entry_offset % 8 != 0
    {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let value = operand_value(source, &state.registers, context, 64)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    if let Some((_, stack_value)) = state
        .stack
        .iter_mut()
        .find(|(stack_slot, _)| *stack_slot == entry_offset)
    {
        *stack_value = value;
    } else {
        state.stack.push((entry_offset, value));
    }
    state.outgoing_cleanup_slots.remove(&entry_offset);
    Ok(())
}

/// Stores the low 64 bits of an XMM register to `N(%rsp)` — the form
/// LLVM uses to write a spilled outgoing `f64` call argument (and to
/// spill a live `f64` across a call). Same slot bounds as
/// `lift_stack_store`: the destination must sit inside the current
/// frame between `rsp` and the caller's argument area.
fn lift_stack_store_sse<'ctx>(
    instruction: &Instruction,
    state: &mut MachineState<'ctx>,
) -> Result<(), FunctionOutcome> {
    let operands = split_operands(&instruction.operands);
    let [source, destination] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    let Some(source_index) = parse_xmm_register(source) else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    if !state.rsp_is_known {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let entry_offset = rsp_memory_entry_offset(destination, state.rsp_offset)?;
    if state.rsp_offset % 8 != 0
        || entry_offset >= 0
        || entry_offset < state.rsp_offset
        || entry_offset % 8 != 0
    {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let value = state.xmm_registers[source_index].extract(63, 0);
    if let Some((_, stack_value)) = state
        .stack
        .iter_mut()
        .find(|(stack_slot, _)| *stack_slot == entry_offset)
    {
        *stack_value = value;
    } else {
        state.stack.push((entry_offset, value));
    }
    state.outgoing_cleanup_slots.remove(&entry_offset);
    Ok(())
}

/// A 128-bit `movaps`/`movups`/`movdqa`/`movdqu` `N(%rsp)`↔xmm transfer:
/// the vectorized form LLVM uses to copy two adjacent qword slots — e.g.
/// forwarding a pair of spilled arguments into a callee's outgoing area
/// in one load/store pair. Each half resolves like a scalar slot: an
/// incoming argument leaf of either class or a recorded frame store on
/// load (a packed copy moves raw bits, so no class check applies), and
/// frame slots inside `[rsp, 0)` on store.
fn lift_stack_wide_move<'ctx>(
    instruction: &Instruction,
    state: &mut MachineState<'ctx>,
    stack_argument_leaves: &[(usize, bool)],
    arguments: &[BV<'ctx>],
    allocated_frame_memory_mode: bool,
) -> Result<(), FunctionOutcome> {
    let operands = split_operands(&instruction.operands);
    let [source, destination] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    let memory_is_source = source.contains('(');
    let (memory, register) = if memory_is_source {
        (source, destination)
    } else {
        (destination, source)
    };
    let Some(register_index) = parse_xmm_register(register) else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    if !state.rsp_is_known || state.rsp_offset % 8 != 0 {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    if should_use_allocated_frame_memory(memory, state, allocated_frame_memory_mode)? {
        // No modeled path for a 16-byte transfer inside the allocated
        // frame region — the region's array model is qword-granular.
        return Err(FunctionOutcome::OutsideProfile("frame_memory_bounds"));
    }
    let entry_offset = rsp_memory_entry_offset(memory, state.rsp_offset)?;
    let high_offset = entry_offset
        .checked_add(8)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    if memory_is_source {
        let low = stack_qword_at(entry_offset, state, stack_argument_leaves, arguments)?;
        let high = stack_qword_at(high_offset, state, stack_argument_leaves, arguments)?;
        state.xmm_registers[register_index] = high.concat(&low);
        return Ok(());
    }
    if high_offset >= 0 || entry_offset < state.rsp_offset || entry_offset % 8 != 0 {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let value = state.xmm_registers[register_index].clone();
    for (offset, half) in [
        (entry_offset, value.extract(63, 0)),
        (high_offset, value.extract(127, 64)),
    ] {
        if let Some((_, stack_value)) = state
            .stack
            .iter_mut()
            .find(|(stack_slot, _)| *stack_slot == offset)
        {
            *stack_value = half;
        } else {
            state.stack.push((offset, half));
        }
        state.outgoing_cleanup_slots.remove(&offset);
    }
    Ok(())
}

fn is_incoming_stack_argument_operand(operand: &str, rsp_offset: i64) -> bool {
    rsp_memory_entry_offset(operand, rsp_offset).is_ok_and(|entry_offset| entry_offset > 0)
}

fn should_use_allocated_frame_memory<'ctx>(
    operand: &str,
    state: &MachineState<'ctx>,
    allocated_frame_memory_mode: bool,
) -> Result<bool, FunctionOutcome> {
    if !allocated_frame_memory_mode {
        return Ok(false);
    }
    if state.frame_region_closed {
        return Err(FunctionOutcome::OutsideProfile("frame_memory_bounds"));
    }
    let indexed = operand.contains("(%rsp,");
    let Some((lo, hi)) = state.frame_region else {
        return if indexed && state.rsp_offset != 0 {
            Err(FunctionOutcome::OutsideProfile("frame_memory_bounds"))
        } else {
            Ok(false)
        };
    };
    if indexed {
        return Ok(true);
    }
    let Some((displacement, None)) = parse_frame_memory_operand(operand) else {
        return Ok(false);
    };
    let entry_offset = state
        .rsp_offset
        .checked_add(displacement)
        .ok_or(FunctionOutcome::OutsideProfile("frame_memory_bounds"))?;
    Ok(entry_offset >= lo && entry_offset < hi)
}

fn parse_frame_memory_operand(operand: &str) -> Option<(i64, Option<usize>)> {
    let open = operand.find('(')?;
    let close = operand.rfind(')')?;
    if close + 1 != operand.len() {
        return None;
    }
    let displacement = operand[..open].trim();
    let displacement = if displacement.is_empty() {
        0
    } else {
        let parsed = parse_integer(displacement)?;
        if operand[..open].trim().starts_with('-') {
            parsed as i64
        } else {
            i64::try_from(parsed).ok()?
        }
    };
    let address_parts = operand[open + 1..close]
        .split(',')
        .map(str::trim)
        .collect::<Vec<_>>();
    match address_parts.as_slice() {
        ["%rsp"] => Some((displacement, None)),
        ["%rsp", index, "8"] => {
            let (register, width) = parse_register(index)?;
            (width == 64 && register != 4).then_some((displacement, Some(register)))
        }
        _ => None,
    }
}

fn frame_memory_offset<'ctx>(
    operand: &str,
    state: &MachineState<'ctx>,
    model: &PureI64Body<'ctx>,
    arguments: &[BV<'ctx>],
    function_has_calls: bool,
    function_adjusts_stack: bool,
    context: &'ctx Context,
) -> Result<BV<'ctx>, FunctionOutcome> {
    if function_has_calls {
        return Err(FunctionOutcome::OutsideProfile("frame_memory_call"));
    }
    if function_adjusts_stack {
        return Err(FunctionOutcome::OutsideProfile("frame_memory_stack_adjust"));
    }
    let (displacement, index) = parse_frame_memory_operand(operand)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    if !state.rsp_is_known || state.rsp_offset != 0 || displacement % 8 != 0 {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let displacement = BV::from_i64(context, displacement, 64);
    let offset = index.map_or_else(
        || displacement.clone(),
        |index| {
            state.registers[index]
                .bvshl(&BV::from_u64(context, 3, 64))
                .bvadd(&displacement)
        },
    );
    let lower_bound = offset.bvsge(&BV::from_i64(context, -128, 64));
    let upper_bound = offset.bvsle(&BV::from_i64(context, -8, 64));
    let in_red_zone = Bool::and(context, &[&lower_bound, &upper_bound]);
    if call_guard_status(
        model,
        &in_red_zone,
        &state.path_conditions,
        arguments,
        context,
    ) != Some(true)
    {
        return Err(FunctionOutcome::OutsideProfile("frame_memory_bounds"));
    }
    Ok(offset)
}

fn allocated_frame_memory_offset<'ctx>(
    operand: &str,
    state: &MachineState<'ctx>,
    model: &PureI64Body<'ctx>,
    arguments: &[BV<'ctx>],
    context: &'ctx Context,
) -> Result<BV<'ctx>, FunctionOutcome> {
    let (displacement, index) = parse_frame_memory_operand(operand)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    let (lo, hi) = state
        .frame_region
        .ok_or(FunctionOutcome::OutsideProfile("frame_memory_bounds"))?;
    if state.frame_region_closed
        || !state.rsp_is_known
        || state.rsp_offset % 8 != 0
        || displacement % 8 != 0
    {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let base = state
        .rsp_offset
        .checked_add(displacement)
        .ok_or(FunctionOutcome::OutsideProfile("frame_memory_bounds"))?;
    let base = BV::from_i64(context, base, 64);
    let offset = index.map_or_else(
        || base.clone(),
        |index| {
            state.registers[index]
                .bvshl(&BV::from_u64(context, 3, 64))
                .bvadd(&base)
        },
    );
    let last_qword = hi
        .checked_sub(8)
        .ok_or(FunctionOutcome::OutsideProfile("frame_memory_bounds"))?;
    let lower_bound = offset.bvsge(&BV::from_i64(context, lo, 64));
    let upper_bound = offset.bvsle(&BV::from_i64(context, last_qword, 64));
    let in_region = Bool::and(context, &[&lower_bound, &upper_bound]);
    if call_guard_status(
        model,
        &in_region,
        &state.path_conditions,
        arguments,
        context,
    ) != Some(true)
    {
        return Err(FunctionOutcome::OutsideProfile("frame_memory_bounds"));
    }
    Ok(offset)
}

fn lift_frame_memory_store<'ctx>(
    instruction: &Instruction,
    state: &mut MachineState<'ctx>,
    model: &PureI64Body<'ctx>,
    arguments: &[BV<'ctx>],
    function_has_calls: bool,
    function_adjusts_stack: bool,
    context: &'ctx Context,
) -> Result<(), FunctionOutcome> {
    let operands = split_operands(&instruction.operands);
    let [source, destination] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    let offset = frame_memory_offset(
        destination,
        state,
        model,
        arguments,
        function_has_calls,
        function_adjusts_stack,
        context,
    )?;
    let value = if let Some(immediate) = source.strip_prefix('$') {
        let immediate = parse_integer(immediate)
            .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
        BV::from_i64(context, i64::from(immediate as u32 as i32), 64)
    } else {
        register_value(source, &state.registers, 64)
            .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?
    };
    state.frame_memory = state.frame_memory.store(&offset, &value);
    state.used_frame_memory = true;
    Ok(())
}

fn lift_allocated_frame_memory_store<'ctx>(
    instruction: &Instruction,
    state: &mut MachineState<'ctx>,
    model: &PureI64Body<'ctx>,
    arguments: &[BV<'ctx>],
    context: &'ctx Context,
) -> Result<(), FunctionOutcome> {
    let operands = split_operands(&instruction.operands);
    let [source, destination] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    let offset = allocated_frame_memory_offset(destination, state, model, arguments, context)?;
    let value = if let Some(immediate) = source.strip_prefix('$') {
        let immediate = parse_integer(immediate)
            .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
        BV::from_i64(context, i64::from(immediate as u32 as i32), 64)
    } else {
        register_value(source, &state.registers, 64)
            .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?
    };
    state.frame_memory = state.frame_memory.store(&offset, &value);
    state.used_allocated_frame_memory = true;
    Ok(())
}

fn lift_frame_memory_load<'ctx>(
    instruction: &Instruction,
    state: &mut MachineState<'ctx>,
    model: &PureI64Body<'ctx>,
    arguments: &[BV<'ctx>],
    function_has_calls: bool,
    function_adjusts_stack: bool,
    context: &'ctx Context,
) -> Result<(), FunctionOutcome> {
    let operands = split_operands(&instruction.operands);
    let [source, destination] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    let offset = frame_memory_offset(
        source,
        state,
        model,
        arguments,
        function_has_calls,
        function_adjusts_stack,
        context,
    )?;
    let (register, width) = parse_register(destination)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    let value = state
        .frame_memory
        .select(&offset)
        .as_bv()
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    if width != 64 || register == 4 || !write_register(&mut state.registers, register, value, 64) {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    state.used_frame_memory = true;
    Ok(())
}

fn lift_allocated_frame_memory_load<'ctx>(
    instruction: &Instruction,
    state: &mut MachineState<'ctx>,
    model: &PureI64Body<'ctx>,
    arguments: &[BV<'ctx>],
    context: &'ctx Context,
) -> Result<(), FunctionOutcome> {
    let operands = split_operands(&instruction.operands);
    let [source, destination] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    let offset = allocated_frame_memory_offset(source, state, model, arguments, context)?;
    let (register, width) = parse_register(destination)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    let value = state
        .frame_memory
        .select(&offset)
        .as_bv()
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    if width != 64 || register == 4 || !write_register(&mut state.registers, register, value, 64) {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    state.used_allocated_frame_memory = true;
    Ok(())
}

/// A `N(%rsp)` load inside the current frame (entry offset `<= 0`):
/// the value the function itself stored at that slot — a register or
/// XMM spill written by `movq`/`movsd`/`pushq`. Reads the recorded
/// `stack` entry only; an unrecorded offset stays closed, and offsets
/// the outgoing-argument cleanup dropped after a call are already
/// absent from `stack` so post-call reads keep failing.
fn frame_stack_slot<'ctx>(
    operand: &str,
    state: &MachineState<'ctx>,
) -> Result<Option<BV<'ctx>>, FunctionOutcome> {
    if state.rsp_offset % 8 != 0 {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let entry_offset = rsp_memory_entry_offset(operand, state.rsp_offset)?;
    if entry_offset >= 0 || entry_offset % 8 != 0 || entry_offset < state.rsp_offset {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    Ok(state
        .stack
        .iter()
        .find(|(stack_slot, _)| *stack_slot == entry_offset)
        .map(|(_, value)| value.clone()))
}

/// The value an `N(%rsp)` memory source carries: an incoming
/// stack-argument leaf above the frame, else a value the function
/// stored inside the frame this path recorded.
fn stack_memory_source<'ctx>(
    operand: &str,
    state: &MachineState<'ctx>,
    stack_argument_leaves: &[(usize, bool)],
    arguments: &[BV<'ctx>],
    want_double: bool,
) -> Result<BV<'ctx>, FunctionOutcome> {
    match incoming_stack_argument(
        operand,
        state.rsp_offset,
        stack_argument_leaves,
        arguments,
        want_double,
    ) {
        Ok(value) => Ok(value),
        Err(outcome) => frame_stack_slot(operand, state)?.ok_or(outcome),
    }
}

/// Resolves a qword at an explicit entry-relative offset: an incoming
/// stack-argument leaf of either class when positive — a packed vector
/// copy moves raw bits, so the source class check does not apply — else
/// the value recorded into `stack` inside the frame this path stored.
fn stack_qword_at<'ctx>(
    entry_offset: i64,
    state: &MachineState<'ctx>,
    stack_argument_leaves: &[(usize, bool)],
    arguments: &[BV<'ctx>],
) -> Result<BV<'ctx>, FunctionOutcome> {
    if entry_offset % 8 != 0 {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    if entry_offset > 0 {
        let slot_index = usize::try_from(entry_offset / 8)
            .map_err(|_| FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
        if slot_index > stack_argument_leaves.len() {
            return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
        }
        let (input_index, _) = stack_argument_leaves[slot_index - 1];
        return arguments
            .get(input_index)
            .cloned()
            .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    if entry_offset < state.rsp_offset {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    state
        .stack
        .iter()
        .find(|(stack_slot, _)| *stack_slot == entry_offset)
        .map(|(_, value)| value.clone())
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))
}

fn lift_stack_argument_move<'ctx>(
    instruction: &Instruction,
    state: &mut MachineState<'ctx>,
    stack_argument_leaves: &[(usize, bool)],
    arguments: &[BV<'ctx>],
) -> Result<(), FunctionOutcome> {
    let operands = split_operands(&instruction.operands);
    let [source, destination] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    if !state.rsp_is_known {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let value = stack_memory_source(source, state, stack_argument_leaves, arguments, false)?;
    let (register_index, width) = parse_register(destination)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    if width != 64
        || register_index == 4
        || !write_register(&mut state.registers, register_index, value, 64)
    {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    Ok(())
}

/// An SSE scalar `f64` instruction reading a memory source: only an
/// incoming stack-argument slot holding a spilled `f64` leaf is
/// modeled — frame-memory or computed-address sources stay closed.
/// `movsd` loads the low half and zeroes bits 127:64; `*sd`
/// arithmetic keeps the destination's upper half;
/// `ucomisd`/`comisd` only set the float-compare flags.
fn lift_stack_argument_sse<'ctx>(
    instruction: &Instruction,
    state: &mut MachineState<'ctx>,
    stack_argument_leaves: &[(usize, bool)],
    arguments: &[BV<'ctx>],
    context: &'ctx Context,
) -> Result<(), FunctionOutcome> {
    let operands = split_operands(&instruction.operands);
    let [source, destination] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    if !state.rsp_is_known {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let value = stack_memory_source(source, state, stack_argument_leaves, arguments, true)?;
    let Some(destination_index) = parse_xmm_register(destination) else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    match instruction.mnemonic.as_str() {
        "movsd" => {
            state.xmm_registers[destination_index] = BV::from_u64(context, 0, 64).concat(&value);
        }
        "addsd" | "subsd" | "mulsd" | "divsd" => {
            let operation = match instruction.mnemonic.as_str() {
                "addsd" => mumei_core::semantic_contract::F64Arith::Add,
                "subsd" => mumei_core::semantic_contract::F64Arith::Sub,
                "mulsd" => mumei_core::semantic_contract::F64Arith::Mul,
                "divsd" => mumei_core::semantic_contract::F64Arith::Div,
                _ => unreachable!(),
            };
            let a = state.xmm_registers[destination_index].extract(63, 0);
            let low = x86_sse_scalar_f64(context, &a, &value, operation);
            let upper = state.xmm_registers[destination_index].extract(127, 64);
            state.xmm_registers[destination_index] = upper.concat(&low);
        }
        "ucomisd" | "comisd" => {
            state.flags = FlagState::FloatCompare {
                left: state.xmm_registers[destination_index].extract(63, 0),
                right: value,
            };
        }
        _ => return Err(FunctionOutcome::OutsideProfile("unsupported_instruction")),
    }
    Ok(())
}

/// Which operand index carries a memory operand for an instruction the
/// stack-argument binary lifter handles: `Some(0)` for write-back ops
/// (the incoming area is read-only, so the slot must be the source) and
/// `Some(0) | Some(1)` for the flag-only `cmp`/`test` forms.
fn stack_argument_binary_form(instruction: &Instruction) -> Option<usize> {
    let index = split_operands(&instruction.operands)
        .iter()
        .position(|operand| operand.contains('('))?;
    match (instruction.mnemonic.as_str(), index) {
        ("addq" | "subq" | "imulq" | "andq" | "orq" | "xorq", 0) => Some(0),
        ("cmpq" | "testq", 0 | 1) => Some(index),
        _ => None,
    }
}

/// An INTEGER binary or flag instruction whose memory operand is an
/// incoming stack-argument slot as one operand: write-back forms
/// (`add`/`sub`/`imul`/`and`/`or`/`xor`) only admit `opq mem, %reg`
/// (the incoming area is read-only) and update the destination register
/// with the same flag model their register forms carry; `cmp` and
/// `test` are read-only, so the slot may be either operand —
/// `cmpq %reg, mem` and `testq %reg, mem` are the forms LLVM emits
/// when it leaves the spilled leaf in place. Other widths stay
/// closed — spilled leaves are qword slots.
fn lift_stack_argument_binary<'ctx>(
    instruction: &Instruction,
    state: &mut MachineState<'ctx>,
    stack_argument_leaves: &[(usize, bool)],
    arguments: &[BV<'ctx>],
    context: &'ctx Context,
) -> Result<(), FunctionOutcome> {
    let operands = split_operands(&instruction.operands);
    let [source, destination] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    if !state.rsp_is_known {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let memory_is_source = source.contains('(');
    let slot_operand = if memory_is_source {
        source
    } else {
        destination
    };
    let slot = stack_memory_source(slot_operand, state, stack_argument_leaves, arguments, false)?;
    match instruction.mnemonic.as_str() {
        "cmpq" | "testq" => {
            let other_operand = if memory_is_source {
                destination
            } else {
                source
            };
            let other = operand_value(other_operand, &state.registers, context, 64)
                .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
            let (left, right) = if memory_is_source {
                (other, slot)
            } else {
                (slot, other)
            };
            state.flags = match instruction.mnemonic.as_str() {
                "cmpq" => FlagState::Compare {
                    left,
                    right,
                    width: 64,
                },
                _ => FlagState::Result {
                    value: left.bvand(&right),
                    width: 64,
                },
            };
            return Ok(());
        }
        _ => (),
    }
    if !memory_is_source {
        // `opq %reg, mem` would write into the incoming argument area —
        // outside the read-only convention.
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let right = slot;
    let (register_index, width) = parse_register(destination)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    if width != 64 || register_index == 4 {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let left = state.registers[register_index].clone();
    let (value, flags) = match instruction.mnemonic.as_str() {
        "addq" => (
            left.bvadd(&right),
            arithmetic_flags(&left, &right, None, false, context),
        ),
        "subq" => (
            left.bvsub(&right),
            arithmetic_flags(&left, &right, None, true, context),
        ),
        "imulq" => (left.bvmul(&right), FlagState::Unknown),
        "andq" => (
            left.bvand(&right),
            FlagState::Result {
                value: left.bvand(&right),
                width: 64,
            },
        ),
        "orq" => (
            left.bvor(&right),
            FlagState::Result {
                value: left.bvor(&right),
                width: 64,
            },
        ),
        "xorq" => (
            left.bvxor(&right),
            FlagState::Result {
                value: left.bvxor(&right),
                width: 64,
            },
        ),
        _ => return Err(FunctionOutcome::OutsideProfile("unsupported_instruction")),
    };
    if !write_register(&mut state.registers, register_index, value, 64) {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    state.flags = flags;
    Ok(())
}

fn call_value_shape_supported_with_leaf_bound(
    shape: CallValueShape,
    aggregate_leaf_bound: usize,
) -> bool {
    match shape {
        CallValueShape::Scalar { leaf_count } => matches!(leaf_count, 1 | 2),
        CallValueShape::Struct {
            leaf_count,
            has_enum: false,
            ..
        } => (1..=aggregate_leaf_bound).contains(&leaf_count),
        CallValueShape::Enum { leaf_count, .. } => (1..=aggregate_leaf_bound).contains(&leaf_count),
        CallValueShape::Struct { .. } => false,
        // Machine TV does not yet model a `[i64]` call result's
        // `(len, store)` pair — outside the checked profile.
        CallValueShape::Array => false,
    }
}

fn call_site_has_unsupported_composite_shape(site: &CallSite<'_>) -> bool {
    site.argument_shapes.iter().copied().any(|shape| {
        matches!(
            shape,
            CallValueShape::Struct { .. } | CallValueShape::Enum { .. }
        ) && !call_value_shape_supported_with_leaf_bound(shape, MAX_AGGREGATE_ARGUMENT_LEAF_COUNT)
    }) || (matches!(site.result_shape, CallValueShape::Struct { .. })
        && !aggregate_result_register_classes_supported(&site.result_leaf_doubles)
        && sret_call_result_leaf_count(site).is_none())
        || (matches!(site.result_shape, CallValueShape::Enum { .. })
            && !aggregate_result_register_classes_supported(&site.result_leaf_doubles)
            && sret_call_result_leaf_count(site).is_none())
}

// Resolves a call-argument leaf of either class that spilled to the
// outgoing argument stack: the caller must have written the leaf at
// `rsp_offset + 8 * stack_argument_index`, where the ordinal counts
// overflowing leaves of both classes in declaration order (the same
// layout the incoming side models). The slot is recorded in
// `outgoing_slots` so a non-tail call can discard it afterwards.
// Scalar and aggregate leaves share this path because `expand`
// lowering flattens both into the same argument sequence.
fn spilled_call_argument<'ctx>(
    stack_argument_index: usize,
    rsp_is_known: bool,
    rsp_offset: i64,
    stack: &[(i64, BV<'ctx>)],
    outgoing_slots: &mut BTreeSet<i64>,
) -> Result<BV<'ctx>, FunctionOutcome> {
    if !rsp_is_known || rsp_offset % 8 != 0 {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let word_index = i64::try_from(stack_argument_index)
        .map_err(|_| FunctionOutcome::OutsideProfile("call_arguments"))?;
    let Some(stack_slot) = word_index
        .checked_mul(8)
        .and_then(|offset| rsp_offset.checked_add(offset))
    else {
        return Err(FunctionOutcome::OutsideProfile("call_arguments"));
    };
    if stack_slot % 8 != 0 {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let Some((_, stack_argument)) = stack.iter().find(|(candidate, _)| *candidate == stack_slot)
    else {
        return Err(FunctionOutcome::OutsideProfile("call_arguments"));
    };
    outgoing_slots.insert(stack_slot);
    Ok(stack_argument.clone())
}

#[allow(clippy::too_many_arguments)]
fn lift_call<'ctx>(
    instruction: &Instruction,
    model: &PureI64Body<'ctx>,
    call_targets: &BTreeMap<u64, CallTarget>,
    call_state: &mut CallState<'ctx>,
    registers: &mut [BV<'ctx>],
    xmm_registers: &mut [BV<'ctx>],
    stack: &mut Vec<(i64, BV<'ctx>)>,
    rsp_offset: i64,
    outgoing_cleanup_slots: &mut BTreeSet<i64>,
    sret_call_buffers: &mut Vec<SretCallBuffer<'ctx>>,
    frame_region: Option<(i64, i64)>,
    rsp_is_known: bool,
    path_conditions: &[Bool<'ctx>],
    arguments: &[BV<'ctx>],
    tail_call: bool,
    context: &'ctx Context,
) -> Result<(), FunctionOutcome> {
    let operands = split_operands(&instruction.operands);
    if operands.len() != 1 || operands[0].starts_with('*') {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let field_offset = instruction
        .offset
        .checked_add(1)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    let target = call_targets
        .get(&field_offset)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    if !target.is_entry_target_relocation() {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    if target.name == crate::translation_validation::CANCEL_POLL_INTRINSIC {
        if tail_call {
            return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
        }
        apply_call_result_and_clobbers(
            registers,
            xmm_registers,
            BV::from_i64(context, 0, 64),
            false,
            CallValueShape::Scalar { leaf_count: 1 },
            &[false],
            context,
        )?;
        call_state.used_call_targets.insert(field_offset);
        return Ok(());
    }
    // A callee named `__*ti3` that a remaining source call site
    // resolves to is a user function, not compiler-rt — name matching
    // alone cannot tell them apart, so the normal call path wins.
    let target_is_source_callee = model
        .call_sites
        .iter()
        .skip(call_state.call_index)
        .any(|site| {
            crate::translation_validation::native_function_name(&site.callee) == target.name
        });
    if let Some(operation) =
        ti3_libcall_operation(&target.name).filter(|_| !target_is_source_callee)
    {
        // compiler-rt `__*ti3` helpers implement an i128/u128
        // operation that the source model keeps as an expression, so
        // no call site is consumed: apply the SysV pair convention
        // (a in rsi:rdi, b in rcx:rdx, result in rdx:rax) directly.
        if tail_call || !model.has_wide_scalar_features {
            return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
        }
        let dividend = registers[6].concat(&registers[7]);
        let divisor = registers[1].concat(&registers[2]);
        if !matches!(operation, Ti3Operation::Multiply) {
            // A zero divisor traps in the helper just as `divq` does,
            // so the same nonzero proof obligation applies.
            let zero = BV::from_u64(context, 0, 128);
            prove_division_safe(
                model,
                path_conditions,
                arguments,
                &divisor._eq(&zero).not(),
                context,
            )?;
        }
        let result = match operation {
            Ti3Operation::SignedDivide => dividend.bvsdiv(&divisor),
            Ti3Operation::SignedRemainder => dividend.bvsrem(&divisor),
            Ti3Operation::UnsignedDivide => dividend.bvudiv(&divisor),
            Ti3Operation::UnsignedRemainder => dividend.bvurem(&divisor),
            Ti3Operation::Multiply => dividend.bvmul(&divisor),
        };
        apply_call_result_and_clobbers(
            registers,
            xmm_registers,
            result,
            false,
            CallValueShape::Scalar { leaf_count: 2 },
            &[false, false],
            context,
        )?;
        call_state.used_call_targets.insert(field_offset);
        return Ok(());
    }
    let mut skipped_false_guard = false;
    let call_site = loop {
        let Some(call_site) = model.call_sites.get(call_state.call_index) else {
            return Err(FunctionOutcome::OutsideProfile(if skipped_false_guard {
                "conditional_call"
            } else {
                "call_order"
            }));
        };
        match call_guard_status(model, &call_site.guard, path_conditions, arguments, context) {
            Some(false) => {
                call_state.call_index += 1;
                skipped_false_guard = true;
            }
            Some(true) => break call_site,
            None => return Err(FunctionOutcome::OutsideProfile("conditional_call")),
        }
    };
    if target.name != crate::translation_validation::native_function_name(&call_site.callee) {
        return Err(FunctionOutcome::OutsideProfile("call_order"));
    }
    let has_composite_shape = call_site.argument_shapes.iter().any(|shape| {
        matches!(
            shape,
            CallValueShape::Struct { .. } | CallValueShape::Enum { .. }
        )
    }) || matches!(
        call_site.result_shape,
        CallValueShape::Struct { .. } | CallValueShape::Enum { .. }
    );
    if call_site.has_composite_arguments_or_result != has_composite_shape
        || call_site.argument_shapes.iter().copied().any(|shape| {
            !call_value_shape_supported_with_leaf_bound(shape, MAX_AGGREGATE_ARGUMENT_LEAF_COUNT)
        })
        || (matches!(call_site.result_shape, CallValueShape::Struct { .. })
            && !aggregate_result_register_classes_supported(&call_site.result_leaf_doubles)
            && sret_call_result_leaf_count(call_site).is_none())
        || (matches!(call_site.result_shape, CallValueShape::Enum { .. })
            && !aggregate_result_register_classes_supported(&call_site.result_leaf_doubles)
            && sret_call_result_leaf_count(call_site).is_none())
    {
        return Err(FunctionOutcome::OutsideProfile("call_arguments"));
    }
    if tail_call && has_composite_shape {
        return Err(FunctionOutcome::OutsideProfile("call_arguments"));
    }
    if call_site.args.len() > MAX_ARGUMENT_COUNT {
        return Err(FunctionOutcome::OutsideProfile("argument_count"));
    }
    let float_argument_count = call_site
        .arg_leaf_doubles
        .iter()
        .filter(|is_double| **is_double)
        .count();
    if tail_call && float_argument_count > 0 {
        return Err(FunctionOutcome::OutsideProfile("call_arguments"));
    }
    let integer_argument_count = call_site.args.len() - float_argument_count;
    if tail_call && integer_argument_count > ARGUMENT_REGISTERS.len() {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    // A call whose all-INTEGER result — struct or enum tag/payload
    // leaves alike — needs the `sret` convention carries the
    // caller-allocated result-buffer pointer in `rdi`, so declared
    // INTEGER arguments begin one register right at `rsi`.
    let sret_call_leaf_count = sret_call_result_leaf_count(call_site);
    let mut outgoing_slots = BTreeSet::new();
    let mut integer_argument_index = usize::from(sret_call_leaf_count.is_some());
    let mut float_argument_index = 0usize;
    let mut stack_argument_index = 0usize;
    let mut leaf_index = 0usize;
    for shape in &call_site.argument_shapes {
        let leaf_count = shape.leaf_count();
        match *shape {
            // No call site can carry an array argument — `check_call`
            // rejects them — but keep the match total fail-closed.
            CallValueShape::Array => {
                return Err(FunctionOutcome::OutsideProfile("call_arguments"));
            }
            CallValueShape::Struct { .. } | CallValueShape::Enum { .. } => {
                let leaf_end = leaf_index
                    .checked_add(leaf_count)
                    .ok_or(FunctionOutcome::OutsideProfile("call_arguments"))?;
                let Some(leaf_doubles) = call_site.arg_leaf_doubles.get(leaf_index..leaf_end)
                else {
                    return Err(FunctionOutcome::OutsideProfile("call_arguments"));
                };
                for (leaf_offset, is_double) in leaf_doubles.iter().enumerate() {
                    let argument = &call_site.args[leaf_index + leaf_offset];
                    if *is_double {
                        if float_argument_index < FLOAT_ARGUMENT_REGISTER_COUNT {
                            let register = xmm_registers[float_argument_index].extract(63, 0);
                            call_state
                                .argument_obligations
                                .push(f64_values_match(context, argument, &register));
                            call_state
                                .exact_f64_argument_obligations
                                .push(argument._eq(&register));
                            float_argument_index += 1;
                        } else {
                            let stack_argument = spilled_call_argument(
                                stack_argument_index,
                                rsp_is_known,
                                rsp_offset,
                                stack,
                                &mut outgoing_slots,
                            )?;
                            call_state.argument_obligations.push(f64_values_match(
                                context,
                                argument,
                                &stack_argument,
                            ));
                            call_state
                                .exact_f64_argument_obligations
                                .push(argument._eq(&stack_argument));
                            stack_argument_index += 1;
                        }
                    } else if let Some(register_index) =
                        ARGUMENT_REGISTERS.get(integer_argument_index)
                    {
                        call_state
                            .argument_obligations
                            .push(argument._eq(&registers[*register_index]));
                        integer_argument_index += 1;
                    } else {
                        let stack_argument = spilled_call_argument(
                            stack_argument_index,
                            rsp_is_known,
                            rsp_offset,
                            stack,
                            &mut outgoing_slots,
                        )?;
                        call_state
                            .argument_obligations
                            .push(argument._eq(&stack_argument));
                        integer_argument_index += 1;
                        stack_argument_index += 1;
                    }
                }
            }
            CallValueShape::Scalar { leaf_count: 2 } => {
                let leaf_end = leaf_index
                    .checked_add(2)
                    .ok_or(FunctionOutcome::OutsideProfile("call_arguments"))?;
                let Some(leaf_doubles) = call_site.arg_leaf_doubles.get(leaf_index..leaf_end)
                else {
                    return Err(FunctionOutcome::OutsideProfile("call_arguments"));
                };
                if leaf_doubles.iter().any(|is_double| *is_double) {
                    return Err(FunctionOutcome::OutsideProfile("argument_count"));
                }
                // `expand` lowering flattens a 128-bit argument into two
                // independent INTEGER scalars: when the register budget
                // runs out mid-pair the low half keeps its register and
                // only the high half spills (real codegen puts `y.lo` in
                // `r9` and `y.hi` at `(%rsp)`).
                for offset in 0..2 {
                    let argument = &call_site.args[leaf_index + offset];
                    if let Some(register_index) = ARGUMENT_REGISTERS.get(integer_argument_index) {
                        call_state
                            .argument_obligations
                            .push(argument._eq(&registers[*register_index]));
                    } else {
                        let stack_argument = spilled_call_argument(
                            stack_argument_index,
                            rsp_is_known,
                            rsp_offset,
                            stack,
                            &mut outgoing_slots,
                        )?;
                        call_state
                            .argument_obligations
                            .push(argument._eq(&stack_argument));
                        stack_argument_index += 1;
                    }
                    integer_argument_index += 1;
                }
            }
            CallValueShape::Scalar { .. } => {
                let argument = &call_site.args[leaf_index];
                if call_site.arg_leaf_doubles[leaf_index] {
                    if float_argument_index < FLOAT_ARGUMENT_REGISTER_COUNT {
                        let register = xmm_registers[float_argument_index].extract(63, 0);
                        call_state
                            .argument_obligations
                            .push(f64_values_match(context, argument, &register));
                        call_state
                            .exact_f64_argument_obligations
                            .push(argument._eq(&register));
                        float_argument_index += 1;
                    } else {
                        let stack_argument = spilled_call_argument(
                            stack_argument_index,
                            rsp_is_known,
                            rsp_offset,
                            stack,
                            &mut outgoing_slots,
                        )?;
                        call_state.argument_obligations.push(f64_values_match(
                            context,
                            argument,
                            &stack_argument,
                        ));
                        call_state
                            .exact_f64_argument_obligations
                            .push(argument._eq(&stack_argument));
                        stack_argument_index += 1;
                    }
                } else if let Some(register_index) = ARGUMENT_REGISTERS.get(integer_argument_index)
                {
                    call_state
                        .argument_obligations
                        .push(argument._eq(&registers[*register_index]));
                    integer_argument_index += 1;
                } else {
                    let stack_argument = spilled_call_argument(
                        stack_argument_index,
                        rsp_is_known,
                        rsp_offset,
                        stack,
                        &mut outgoing_slots,
                    )?;
                    call_state
                        .argument_obligations
                        .push(argument._eq(&stack_argument));
                    integer_argument_index += 1;
                    stack_argument_index += 1;
                }
            }
        }
        leaf_index += leaf_count;
    }
    if !tail_call && !outgoing_slots.is_empty() {
        stack.retain(|(stack_slot, _)| !outgoing_slots.contains(stack_slot));
        outgoing_cleanup_slots.extend(outgoing_slots.iter().copied());
    }
    if let Some(leaf_count) = sret_call_leaf_count {
        apply_sret_call_result_and_clobbers(
            registers,
            xmm_registers,
            stack,
            rsp_offset,
            rsp_is_known,
            leaf_count,
            &call_site.result,
            sret_call_buffers,
            &outgoing_slots,
            frame_region,
            context,
        )?;
    } else {
        apply_call_result_and_clobbers(
            registers,
            xmm_registers,
            call_site.result.clone(),
            call_site.result_is_double,
            call_site.result_shape,
            &call_site.result_leaf_doubles,
            context,
        )?;
    }
    call_state.used_call_targets.insert(field_offset);
    call_state.call_index += 1;
    Ok(())
}

enum Ti3Operation {
    SignedDivide,
    SignedRemainder,
    UnsignedDivide,
    UnsignedRemainder,
    Multiply,
}

/// Whether `name` is a compiler-rt 128-bit helper the lifter models
/// (`__divti3`, `__modti3`, `__udivti3`, `__umodti3`, `__multi3`). LLVM
/// injects these symbols at codegen time, so they never appear in the
/// IR's declared set.
pub fn is_ti3_libcall(name: &str) -> bool {
    ti3_libcall_operation(name).is_some()
}

fn ti3_libcall_operation(name: &str) -> Option<Ti3Operation> {
    match name {
        "__divti3" => Some(Ti3Operation::SignedDivide),
        "__modti3" => Some(Ti3Operation::SignedRemainder),
        "__udivti3" => Some(Ti3Operation::UnsignedDivide),
        "__umodti3" => Some(Ti3Operation::UnsignedRemainder),
        "__multi3" => Some(Ti3Operation::Multiply),
        _ => None,
    }
}

fn apply_call_result_and_clobbers<'ctx>(
    registers: &mut [BV<'ctx>],
    xmm_registers: &mut [BV<'ctx>],
    result: BV<'ctx>,
    result_is_double: bool,
    result_shape: CallValueShape,
    result_leaf_doubles: &[bool],
    context: &'ctx Context,
) -> Result<(), FunctionOutcome> {
    for xmm_register in xmm_registers.iter_mut() {
        *xmm_register = BV::fresh_const(context, "machine_call_xmm_clobber", 128);
    }
    for register_index in [0, 1, 2, 6, 7, 8, 9, 10, 11] {
        registers[register_index] = BV::fresh_const(context, "machine_call_clobber", 64);
    }
    match result_shape {
        CallValueShape::Scalar { leaf_count: 1 } if result_is_double => {
            let upper = BV::fresh_const(context, "machine_call_xmm_result_upper", 64);
            xmm_registers[0] = upper.concat(&result);
        }
        CallValueShape::Scalar { leaf_count: 1 } => registers[0] = result,
        CallValueShape::Scalar { leaf_count: 2 } if result.get_size() == 128 => {
            registers[0] = result.extract(63, 0);
            registers[2] = result.extract(127, 64);
        }
        CallValueShape::Struct {
            leaf_count,
            has_enum: false,
            has_f64,
        } if leaf_count <= INTEGER_RESULT_REGISTERS.len() + FLOAT_RESULT_REGISTER_COUNT => {
            if result_leaf_doubles.len() != leaf_count
                || has_f64 != result_leaf_doubles.iter().any(|is_double| *is_double)
                || !aggregate_result_register_classes_supported(result_leaf_doubles)
            {
                return Err(FunctionOutcome::OutsideProfile("call_arguments"));
            }
            let mut integer_result_index = 0usize;
            let mut float_result_index = 0usize;
            for (leaf_index, is_double) in result_leaf_doubles.iter().enumerate() {
                let leaf = aggregate_leaf(&result, leaf_count, leaf_index);
                if *is_double {
                    let upper = BV::fresh_const(context, "machine_call_xmm_result_upper", 64);
                    xmm_registers[float_result_index] = upper.concat(&leaf);
                    float_result_index += 1;
                } else {
                    registers[INTEGER_RESULT_REGISTERS[integer_result_index]] = leaf;
                    integer_result_index += 1;
                }
            }
        }
        // The class budgets (≤3 INTEGER + ≤2 SSE leaves) admit a
        // five-leaf enum — e.g. `{tag, a, b, x: f64, y: f64}` — as
        // register-resident, so the cap matches the Struct arm.
        CallValueShape::Enum { leaf_count }
            if leaf_count <= INTEGER_RESULT_REGISTERS.len() + FLOAT_RESULT_REGISTER_COUNT =>
        {
            if result_leaf_doubles.len() != leaf_count
                || result_leaf_doubles.first().copied().unwrap_or(true)
                || !aggregate_result_register_classes_supported(result_leaf_doubles)
            {
                return Err(FunctionOutcome::OutsideProfile("call_arguments"));
            }
            let mut integer_result_index = 0usize;
            let mut float_result_index = 0usize;
            for (leaf_index, is_double) in result_leaf_doubles.iter().enumerate() {
                let leaf = aggregate_leaf(&result, leaf_count, leaf_index);
                if *is_double {
                    let upper = BV::fresh_const(context, "machine_call_xmm_result_upper", 64);
                    xmm_registers[float_result_index] = upper.concat(&leaf);
                    float_result_index += 1;
                } else {
                    registers[INTEGER_RESULT_REGISTERS[integer_result_index]] = leaf;
                    integer_result_index += 1;
                }
            }
        }
        _ => return Err(FunctionOutcome::OutsideProfile("call_arguments")),
    }
    Ok(())
}

/// Applies an `sret` call result: the callee wrote each result leaf
/// into the caller's buffer through the pointer it received in `rdi`
/// and returned that same pointer in `rax`. The buffer's entry-relative
/// base is recovered by subtracting the current `rsp` value — real LLVM
/// materializes the pointer with `mov %rsp,%rdi` or `lea N(%rsp),%rdi`,
/// so the difference simplifies to a constant — and every leaf slot
/// goes into the `stack` map, so later `N(%rsp)` reads resolve through
/// the ordinary slot lookup and `add $0x28,%rsp`-style restores prune
/// the buffer with the rest of the frame. A pointer that does not
/// reduce to an `rsp`-relative constant is not a stack buffer the model
/// can place and stays fail-closed.
#[allow(clippy::too_many_arguments)]
fn apply_sret_call_result_and_clobbers<'ctx>(
    registers: &mut [BV<'ctx>],
    xmm_registers: &mut [BV<'ctx>],
    stack: &mut Vec<(i64, BV<'ctx>)>,
    rsp_offset: i64,
    rsp_is_known: bool,
    leaf_count: usize,
    result: &BV<'ctx>,
    sret_call_buffers: &mut Vec<SretCallBuffer<'ctx>>,
    outgoing_slots: &BTreeSet<i64>,
    frame_region: Option<(i64, i64)>,
    context: &'ctx Context,
) -> Result<(), FunctionOutcome> {
    if !rsp_is_known {
        return Err(FunctionOutcome::OutsideProfile("call_arguments"));
    }
    let pointer = registers[7].clone();
    let Some(buffer_delta) = pointer.bvsub(&registers[4]).simplify().as_i64() else {
        return Err(FunctionOutcome::OutsideProfile("call_arguments"));
    };
    if buffer_delta < 0 || buffer_delta % 8 != 0 {
        return Err(FunctionOutcome::OutsideProfile("call_arguments"));
    }
    let Some(base_offset) = rsp_offset.checked_add(buffer_delta) else {
        return Err(FunctionOutcome::OutsideProfile("call_arguments"));
    };
    let Some(buffer_end) = leaf_count
        .checked_mul(8)
        .and_then(|bytes| i64::try_from(bytes).ok())
        .and_then(|bytes| base_offset.checked_add(bytes))
    else {
        return Err(FunctionOutcome::OutsideProfile("call_arguments"));
    };
    // A buffer overlapping this call's outgoing stack-argument slots
    // would let the callee's result stores overwrite bytes its own
    // argument reads still resolve to — stay fail-closed.
    if outgoing_slots
        .range(base_offset..buffer_end)
        .next()
        .is_some()
    {
        return Err(FunctionOutcome::OutsideProfile("call_arguments"));
    }
    // A buffer overlapping the allocated frame region would leave leaf
    // writes visible through `stack` while indexed array reads still
    // resolve through `frame_memory` — the two views diverge.
    if let Some((lo, hi)) = frame_region {
        if base_offset < hi && buffer_end > lo {
            return Err(FunctionOutcome::OutsideProfile("frame_memory_bounds"));
        }
    }
    for xmm_register in xmm_registers.iter_mut() {
        *xmm_register = BV::fresh_const(context, "machine_call_xmm_clobber", 128);
    }
    for register_index in [0, 1, 2, 6, 7, 8, 9, 10, 11] {
        registers[register_index] = BV::fresh_const(context, "machine_call_clobber", 64);
    }
    // `rax` returns the buffer pointer — `mov (%rax),...` and
    // `mov (%rsp),...` reads both resolve to the leaf slots.
    registers[0] = pointer.clone();
    for leaf_index in 0..leaf_count {
        let Ok(slot_index) = i64::try_from(leaf_index) else {
            return Err(FunctionOutcome::OutsideProfile("call_arguments"));
        };
        let Some(entry_offset) = slot_index
            .checked_mul(8)
            .and_then(|offset| base_offset.checked_add(offset))
        else {
            return Err(FunctionOutcome::OutsideProfile("call_arguments"));
        };
        let leaf = aggregate_leaf(result, leaf_count, leaf_index);
        if let Some((_, stored)) = stack
            .iter_mut()
            .find(|(stack_slot, _)| *stack_slot == entry_offset)
        {
            *stored = leaf;
        } else {
            stack.push((entry_offset, leaf));
        }
    }
    sret_call_buffers.push(SretCallBuffer {
        pointer,
        base_offset,
        leaf_count,
    });
    Ok(())
}

fn path_solver<'ctx>(
    model: &PureI64Body<'ctx>,
    path_conditions: &[Bool<'ctx>],
    arguments: &[BV<'ctx>],
    context: &'ctx Context,
) -> Solver<'ctx> {
    let solver = Solver::new(context);
    set_solver_budget(&solver, context);
    let substitutions = input_substitutions(model, arguments);
    for assumption in model
        .param_domain
        .iter()
        .chain(&model.definedness)
        .chain(&model.call_result_bounds)
        .chain(model.loop_bound_exits.iter())
    {
        solver.assert(&assumption.substitute(&substitutions));
    }
    for path_condition in path_conditions {
        solver.assert(&path_condition.substitute(&substitutions));
    }
    solver
}

fn path_feasibility_status<'ctx>(
    model: &PureI64Body<'ctx>,
    path_conditions: &[Bool<'ctx>],
    arguments: &[BV<'ctx>],
    context: &'ctx Context,
) -> SatResult {
    path_solver(model, path_conditions, arguments, context).check()
}

fn call_guard_status<'ctx>(
    model: &PureI64Body<'ctx>,
    guard: &Bool<'ctx>,
    path_conditions: &[Bool<'ctx>],
    arguments: &[BV<'ctx>],
    context: &'ctx Context,
) -> Option<bool> {
    let solver = path_solver(model, path_conditions, arguments, context);
    if solver.check() != SatResult::Sat {
        return None;
    }
    // The guard is a model-side term — fold its input constants onto the
    // same variables as the machine terms before asserting it.
    let substitutions = input_substitutions(model, arguments);
    let guard = guard.substitute(&substitutions);
    solver.push();
    solver.assert(&guard.not());
    let guard_true = solver.check();
    solver.pop(1);
    match guard_true {
        SatResult::Unsat => return Some(true),
        SatResult::Unknown => return None,
        SatResult::Sat => {}
    }
    solver.push();
    solver.assert(&guard);
    let guard_false = solver.check();
    solver.pop(1);
    match guard_false {
        SatResult::Unsat => Some(false),
        SatResult::Sat | SatResult::Unknown => None,
    }
}

fn division_safety_outcome(counterexample_status: SatResult) -> Result<(), FunctionOutcome> {
    match counterexample_status {
        SatResult::Unsat => Ok(()),
        SatResult::Sat => Err(FunctionOutcome::OutsideProfile("division_trap")),
        SatResult::Unknown => Err(FunctionOutcome::Unknown),
    }
}

fn prove_division_safe<'ctx>(
    model: &PureI64Body<'ctx>,
    path_conditions: &[Bool<'ctx>],
    arguments: &[BV<'ctx>],
    safety: &Bool<'ctx>,
    context: &'ctx Context,
) -> Result<(), FunctionOutcome> {
    let solver = path_solver(model, path_conditions, arguments, context);
    let substitutions = input_substitutions(model, arguments);
    solver.assert(&safety.substitute(&substitutions).not());
    division_safety_outcome(solver.check())
}

fn lift_division<'ctx>(
    instruction: &Instruction,
    state: &mut MachineState<'ctx>,
    model: &PureI64Body<'ctx>,
    arguments: &[BV<'ctx>],
    context: &'ctx Context,
) -> Result<(), FunctionOutcome> {
    let operands = split_operands(&instruction.operands);
    let [operand] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };

    match instruction.mnemonic.as_str() {
        "idivq" => {
            let divisor = register_value(operand, &state.registers, 64)
                .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
            let sign_filled_rax = most_significant_bit(&state.registers[0], 64, context).ite(
                &BV::from_i64(context, -1, 64),
                &BV::from_i64(context, 0, 64),
            );
            let dividend_is_sign_extended_i64 = call_guard_status(
                model,
                &state.registers[2]._eq(&sign_filled_rax),
                &state.path_conditions,
                arguments,
                context,
            ) == Some(true);
            let nonzero = divisor._eq(&BV::from_i64(context, 0, 64)).not();
            if dividend_is_sign_extended_i64 {
                let min = BV::from_i64(context, i64::MIN, 64);
                let minus_one = BV::from_i64(context, -1, 64);
                let overflow = Bool::and(
                    context,
                    &[&state.registers[0]._eq(&min), &divisor._eq(&minus_one)],
                )
                .not();
                prove_division_safe(
                    model,
                    &state.path_conditions,
                    arguments,
                    &Bool::and(context, &[&nonzero, &overflow]),
                    context,
                )?;
                let dividend = state.registers[0].clone();
                state.registers[0] = signed_division_64(&dividend, &divisor, false);
                state.registers[2] = signed_division_64(&dividend, &divisor, true);
            } else {
                let dividend = state.registers[2].concat(&state.registers[0]);
                let quotient = signed_division_128(&dividend, &divisor, false);
                let remainder = signed_division_128(&dividend, &divisor, true);
                let quotient_low = quotient.extract(63, 0);
                let fits = quotient._eq(&quotient_low.sign_ext(64));
                prove_division_safe(
                    model,
                    &state.path_conditions,
                    arguments,
                    &Bool::and(context, &[&nonzero, &fits]),
                    context,
                )?;
                state.registers[0] = quotient_low;
                state.registers[2] = remainder.extract(63, 0);
            }
        }
        "divq" => {
            let divisor = register_value(operand, &state.registers, 64)
                .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
            let dividend = state.registers[2].concat(&state.registers[0]);
            let wide_divisor = divisor.zero_ext(64);
            let quotient = dividend.bvudiv(&wide_divisor);
            let remainder = dividend.bvurem(&wide_divisor);
            let fits = quotient.extract(127, 64)._eq(&BV::from_u64(context, 0, 64));
            let nonzero = divisor._eq(&BV::from_u64(context, 0, 64)).not();
            let high_half_zero = call_guard_status(
                model,
                &state.registers[2]._eq(&BV::from_u64(context, 0, 64)),
                &state.path_conditions,
                arguments,
                context,
            ) == Some(true);
            if high_half_zero {
                prove_division_safe(model, &state.path_conditions, arguments, &nonzero, context)?;
                let dividend = state.registers[0].clone();
                state.registers[0] = dividend.bvudiv(&divisor);
                state.registers[2] = dividend.bvurem(&divisor);
                return Ok(());
            }
            let safety = Bool::and(context, &[&nonzero, &fits]);
            prove_division_safe(model, &state.path_conditions, arguments, &safety, context)?;
            state.registers[0] = quotient.extract(63, 0);
            state.registers[2] = remainder.extract(63, 0);
        }
        "divl" => {
            let Some((divisor_index, 32)) = parse_register(operand) else {
                return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
            };
            let divisor = register_value(operand, &state.registers, 32)
                .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
            let divisor_i64 = state.registers[divisor_index].clone();
            let dividend = state.registers[2]
                .extract(31, 0)
                .concat(&state.registers[0].extract(31, 0));
            let wide_divisor = divisor.zero_ext(32);
            let quotient = dividend.bvudiv(&wide_divisor);
            let remainder = dividend.bvurem(&wide_divisor);
            let fits_by_small_dividend =
                dividend.extract(63, 32)._eq(&BV::from_u64(context, 0, 32));
            let fits_by_quotient = quotient.extract(63, 32)._eq(&BV::from_u64(context, 0, 32));
            let nonzero = divisor._eq(&BV::from_u64(context, 0, 32)).not();
            prove_division_safe(
                model,
                &state.path_conditions,
                arguments,
                &Bool::and(
                    context,
                    &[
                        &nonzero,
                        &Bool::or(context, &[&fits_by_small_dividend, &fits_by_quotient]),
                    ],
                ),
                context,
            )?;
            let zero_extended_32_bit_operands = Bool::and(
                context,
                &[
                    &state.registers[0]
                        .extract(63, 32)
                        ._eq(&BV::from_u64(context, 0, 32)),
                    &state.registers[2]
                        .extract(31, 0)
                        ._eq(&BV::from_u64(context, 0, 32)),
                    &divisor_i64
                        .extract(63, 32)
                        ._eq(&BV::from_u64(context, 0, 32)),
                ],
            );
            let zero_extended_32_bit_status = call_guard_status(
                model,
                &zero_extended_32_bit_operands,
                &state.path_conditions,
                arguments,
                context,
            );
            if zero_extended_32_bit_status == Some(true) {
                let dividend = state.registers[0].clone();
                state.registers[0] = signed_division_64(&dividend, &divisor_i64, false);
                state.registers[2] = signed_division_64(&dividend, &divisor_i64, true);
            } else if !write_register(&mut state.registers, 0, quotient.extract(31, 0), 32)
                || !write_register(&mut state.registers, 2, remainder.extract(31, 0), 32)
            {
                return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
            }
        }
        _ => return Err(FunctionOutcome::OutsideProfile("unsupported_instruction")),
    }
    Ok(())
}

fn bv_ite_parts<'ctx>(value: &BV<'ctx>) -> Option<(Bool<'ctx>, BV<'ctx>, BV<'ctx>)> {
    let simplified = value.simplify();
    let declaration = simplified.decl().name();
    if declaration == "extract" {
        let extracted = simplified.children().into_iter().next()?.as_bv()?;
        if extracted.get_size() == simplified.get_size() {
            return bv_ite_parts(&extracted);
        }
        return None;
    }
    if declaration != "if" {
        return None;
    }
    let mut children = simplified.children().into_iter();
    Some((
        children.next()?.as_bool()?,
        children.next()?.as_bv()?,
        children.next()?.as_bv()?,
    ))
}

fn neg_one_divisor_ite<'ctx>(
    condition: &Bool<'ctx>,
    then_divisor: &BV<'ctx>,
    else_divisor: &BV<'ctx>,
) -> Option<BV<'ctx>> {
    if then_divisor.as_u64() == Some(1)
        && is_neg_one_comparison(condition, else_divisor) == Some(true)
    {
        return Some(else_divisor.clone());
    }
    if else_divisor.as_u64() == Some(1)
        && is_neg_one_comparison(condition, then_divisor) == Some(false)
    {
        return Some(then_divisor.clone());
    }
    None
}

fn is_neg_one_comparison(condition: &Bool<'_>, divisor: &BV<'_>) -> Option<bool> {
    let simplified = condition.simplify();
    let (equality, positive) = if simplified.decl().name() == "not" {
        (simplified.children().into_iter().next()?.as_bool()?, false)
    } else {
        (simplified, true)
    };
    if equality.decl().name() != "=" {
        return None;
    }
    let mut children = equality.children().into_iter();
    let left = children.next()?.as_bv()?;
    let right = children.next()?.as_bv()?;
    let compares_divisor_to_minus_one = (left == divisor.clone()
        && right.as_u64() == Some(u64::MAX))
        || (right == divisor.clone() && left.as_u64() == Some(u64::MAX));
    compares_divisor_to_minus_one.then_some(positive)
}

fn signed_division_64<'ctx>(numerator: &BV<'ctx>, divisor: &BV<'ctx>, remainder: bool) -> BV<'ctx> {
    if let Some((condition, then_divisor, else_divisor)) = bv_ite_parts(divisor) {
        if remainder {
            if let Some(original_divisor) =
                neg_one_divisor_ite(&condition, &then_divisor, &else_divisor)
            {
                return numerator.bvsrem(&original_divisor);
            }
        }
        let then_result = if remainder {
            numerator.bvsrem(&then_divisor)
        } else {
            numerator.bvsdiv(&then_divisor)
        };
        let else_result = if remainder {
            numerator.bvsrem(&else_divisor)
        } else {
            numerator.bvsdiv(&else_divisor)
        };
        condition.ite(&then_result, &else_result).simplify()
    } else if remainder {
        numerator.bvsrem(divisor)
    } else {
        numerator.bvsdiv(divisor)
    }
}

fn signed_division_128<'ctx>(
    numerator: &BV<'ctx>,
    divisor: &BV<'ctx>,
    remainder: bool,
) -> BV<'ctx> {
    if let Some((condition, then_divisor, else_divisor)) = bv_ite_parts(divisor) {
        if remainder {
            if let Some(original_divisor) =
                neg_one_divisor_ite(&condition, &then_divisor, &else_divisor)
            {
                return numerator.bvsrem(&original_divisor.sign_ext(64));
            }
        }
        let then_divisor = then_divisor.sign_ext(64);
        let else_divisor = else_divisor.sign_ext(64);
        let then_result = if remainder {
            numerator.bvsrem(&then_divisor)
        } else {
            numerator.bvsdiv(&then_divisor)
        };
        let else_result = if remainder {
            numerator.bvsrem(&else_divisor)
        } else {
            numerator.bvsdiv(&else_divisor)
        };
        condition.ite(&then_result, &else_result).simplify()
    } else {
        let wide_divisor = divisor.sign_ext(64);
        if remainder {
            numerator.bvsrem(&wide_divisor)
        } else {
            numerator.bvsdiv(&wide_divisor)
        }
    }
}

fn lift_cqto<'ctx>(
    instruction: &Instruction,
    registers: &mut [BV<'ctx>],
    context: &'ctx Context,
) -> Result<(), FunctionOutcome> {
    if !instruction.operands.is_empty() {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let sign = most_significant_bit(&registers[0], 64, context);
    registers[2] = sign.ite(
        &BV::from_i64(context, -1, 64),
        &BV::from_i64(context, 0, 64),
    );
    Ok(())
}

fn lift_cltd<'ctx>(
    instruction: &Instruction,
    registers: &mut [BV<'ctx>],
    context: &'ctx Context,
) -> Result<(), FunctionOutcome> {
    if !instruction.operands.is_empty() {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let eax = registers[0].extract(31, 0);
    let sign = most_significant_bit(&eax, 32, context);
    registers[2] = sign.ite(
        &BV::from_u64(context, u64::from(u32::MAX), 64),
        &BV::from_u64(context, 0, 64),
    );
    Ok(())
}

fn lift_imul_high<'ctx>(
    instruction: &Instruction,
    registers: &mut [BV<'ctx>],
) -> Result<(), FunctionOutcome> {
    let operands = split_operands(&instruction.operands);
    let [operand] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    let source = register_value(operand, registers, 64)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    let product = registers[0].sign_ext(64).bvmul(&source.sign_ext(64));
    registers[0] = product.extract(63, 0);
    registers[2] = product.extract(127, 64);
    Ok(())
}

fn lift_mul_high<'ctx>(
    instruction: &Instruction,
    registers: &mut [BV<'ctx>],
) -> Result<(), FunctionOutcome> {
    let operands = split_operands(&instruction.operands);
    let [operand] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    let source = register_value(operand, registers, 64)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    let product = registers[0].zero_ext(64).bvmul(&source.zero_ext(64));
    registers[0] = product.extract(63, 0);
    registers[2] = product.extract(127, 64);
    Ok(())
}

fn adjust_rsp<'ctx>(
    registers: &mut [BV<'ctx>],
    stack: &mut Vec<(i64, BV<'ctx>)>,
    rsp_offset: &mut i64,
    delta: i64,
    context: &'ctx Context,
) -> bool {
    let Some(next_offset) = rsp_offset.checked_add(delta) else {
        return false;
    };
    if *rsp_offset % 8 != 0 || next_offset > 0 || next_offset % 8 != 0 {
        return false;
    }
    registers[4] = registers[4].bvadd(&BV::from_u64(context, delta as u64, 64));
    *rsp_offset = next_offset;
    if delta > 0 {
        stack.retain(|(stack_slot, _)| *stack_slot >= next_offset);
    }
    true
}

fn lift_instruction<'ctx>(
    instruction: &Instruction,
    registers: &mut [BV<'ctx>],
    xmm_registers: &mut [BV<'ctx>],
    source_upper_32_zero: bool,
    allow_f64: bool,
    context: &'ctx Context,
) -> bool {
    if allow_f64 && lift_sse_scalar_instruction(instruction, registers, xmm_registers, context) {
        return true;
    }
    let operands = split_operands(&instruction.operands);
    match instruction.mnemonic.as_str() {
        "movb" => lift_move(&operands, registers, context, 8, false),
        "movl" => lift_move(&operands, registers, context, 32, source_upper_32_zero),
        "movq" => lift_move(&operands, registers, context, 64, false),
        "movabsq" => lift_move(&operands, registers, context, 64, false),
        "movzbl" => lift_move_extend(&operands, registers, false, 32),
        "movzbq" => lift_move_extend(&operands, registers, false, 64),
        "movsbl" => lift_move_extend(&operands, registers, true, 32),
        "movsbq" => lift_move_extend(&operands, registers, true, 64),
        "leaq" => lift_lea(&operands, registers, context),
        "addl" => lift_binary(&operands, registers, context, 32, BinaryOp::Add),
        "addq" => lift_binary(&operands, registers, context, 64, BinaryOp::Add),
        "subl" => lift_binary(&operands, registers, context, 32, BinaryOp::Sub),
        "subq" => lift_binary(&operands, registers, context, 64, BinaryOp::Sub),
        "imull" => lift_imul(&operands, registers, context, 32),
        "imulq" => lift_imul(&operands, registers, context, 64),
        "notl" => lift_not(&operands, registers, 32),
        "notq" => lift_not(&operands, registers, 64),
        "xorl" => lift_xor(&operands, registers, 32),
        "xorq" => lift_xor(&operands, registers, 64),
        _ => false,
    }
}

fn lift_sse_scalar_instruction<'ctx>(
    instruction: &Instruction,
    registers: &mut [BV<'ctx>],
    xmm_registers: &mut [BV<'ctx>],
    context: &'ctx Context,
) -> bool {
    let operands = split_operands(&instruction.operands);
    match instruction.mnemonic.as_str() {
        "movsd" => {
            let [source, destination] = operands.as_slice() else {
                return false;
            };
            let (Some(source_index), Some(destination_index)) =
                (parse_xmm_register(source), parse_xmm_register(destination))
            else {
                return false;
            };
            let low = xmm_registers[source_index].extract(63, 0);
            let upper = xmm_registers[destination_index].extract(127, 64);
            xmm_registers[destination_index] = upper.concat(&low);
            true
        }
        "movapd" | "movaps" => {
            let [source, destination] = operands.as_slice() else {
                return false;
            };
            let (Some(source_index), Some(destination_index)) =
                (parse_xmm_register(source), parse_xmm_register(destination))
            else {
                return false;
            };
            xmm_registers[destination_index] = xmm_registers[source_index].clone();
            true
        }
        "addsd" | "subsd" | "mulsd" | "divsd" => {
            let [source, destination] = operands.as_slice() else {
                return false;
            };
            let (Some(source_index), Some(destination_index)) =
                (parse_xmm_register(source), parse_xmm_register(destination))
            else {
                return false;
            };
            let operation = match instruction.mnemonic.as_str() {
                "addsd" => mumei_core::semantic_contract::F64Arith::Add,
                "subsd" => mumei_core::semantic_contract::F64Arith::Sub,
                "mulsd" => mumei_core::semantic_contract::F64Arith::Mul,
                "divsd" => mumei_core::semantic_contract::F64Arith::Div,
                _ => unreachable!(),
            };
            let a = xmm_registers[destination_index].extract(63, 0);
            let b = xmm_registers[source_index].extract(63, 0);
            let low = x86_sse_scalar_f64(context, &a, &b, operation);
            let upper = xmm_registers[destination_index].extract(127, 64);
            xmm_registers[destination_index] = upper.concat(&low);
            true
        }
        "cmpeqsd" => {
            let [source, destination] = operands.as_slice() else {
                return false;
            };
            let (Some(source_index), Some(destination_index)) =
                (parse_xmm_register(source), parse_xmm_register(destination))
            else {
                return false;
            };
            let a = xmm_registers[destination_index].extract(63, 0);
            let b = xmm_registers[source_index].extract(63, 0);
            let low = mumei_core::semantic_contract::f64_eq(context, &a, &b).ite(
                &BV::from_u64(context, u64::MAX, 64),
                &BV::from_u64(context, 0, 64),
            );
            let upper = xmm_registers[destination_index].extract(127, 64);
            xmm_registers[destination_index] = upper.concat(&low);
            true
        }
        "movq" => {
            let [source, destination] = operands.as_slice() else {
                return false;
            };
            if let (Some(source_index), Some((destination_index, 64))) =
                (parse_xmm_register(source), parse_register(destination))
            {
                destination_index != 4
                    && write_register(
                        registers,
                        destination_index,
                        xmm_registers[source_index].extract(63, 0),
                        64,
                    )
            } else if let (Some((source_index, 64)), Some(destination_index)) =
                (parse_register(source), parse_xmm_register(destination))
            {
                if source_index == 4 {
                    return false;
                }
                xmm_registers[destination_index] = registers[source_index].zero_ext(64);
                true
            } else {
                false
            }
        }
        "xorps" | "xorpd" => {
            let [source, destination] = operands.as_slice() else {
                return false;
            };
            let (Some(source_index), Some(destination_index)) =
                (parse_xmm_register(source), parse_xmm_register(destination))
            else {
                return false;
            };
            if source_index != destination_index {
                return false;
            }
            xmm_registers[destination_index] = BV::from_u64(context, 0, 128);
            true
        }
        _ => false,
    }
}

fn lift_move_extend(
    operands: &[&str],
    registers: &mut [BV<'_>],
    sign_extend: bool,
    destination_width: u32,
) -> bool {
    let [source, destination] = operands else {
        return false;
    };
    let Some(source) = register_value(source, registers, 8) else {
        return false;
    };
    let Some((index, width)) = parse_register(destination) else {
        return false;
    };
    if width != destination_width || index == 4 {
        return false;
    }
    let extension = destination_width - 8;
    let value = if sign_extend {
        source.sign_ext(extension)
    } else {
        source.zero_ext(extension)
    };
    write_register(registers, index, value, destination_width)
}

#[derive(Clone, Copy)]
enum BinaryOp {
    Add,
    Sub,
}

fn lift_move<'ctx>(
    operands: &[&str],
    registers: &mut [BV<'ctx>],
    context: &'ctx Context,
    width: u32,
    source_upper_32_zero: bool,
) -> bool {
    let [source, destination] = operands else {
        return false;
    };
    let Some((index, destination_width)) = parse_register(destination) else {
        return false;
    };
    if destination_width != width || index == 4 {
        return false;
    }
    if width == 32 && source_upper_32_zero {
        if let Some((source_index, 32)) = parse_register(source) {
            registers[index] = registers[source_index].clone();
            return true;
        }
    }
    let Some(source) = operand_value(source, registers, context, width) else {
        return false;
    };
    write_register(registers, index, source, width)
}

fn lift_lea<'ctx>(operands: &[&str], registers: &mut [BV<'ctx>], context: &'ctx Context) -> bool {
    let [source, destination] = operands else {
        return false;
    };
    let Some((index, width)) = parse_register(destination) else {
        return false;
    };
    if width != 64 || index == 4 {
        return false;
    }
    let Some(value) = lea_value(source, registers, context) else {
        return false;
    };
    write_register(registers, index, value, 64)
}

fn lift_binary<'ctx>(
    operands: &[&str],
    registers: &mut [BV<'ctx>],
    context: &'ctx Context,
    width: u32,
    operation: BinaryOp,
) -> bool {
    let [source, destination] = operands else {
        return false;
    };
    let Some((index, destination_width)) = parse_register(destination) else {
        return false;
    };
    if destination_width != width || index == 4 {
        return false;
    }
    let Some(source) = operand_value(source, registers, context, width) else {
        return false;
    };
    let destination = if width == 64 {
        registers[index].clone()
    } else {
        registers[index].extract(width - 1, 0)
    };
    let value = match operation {
        BinaryOp::Add => destination.bvadd(&source),
        BinaryOp::Sub => destination.bvsub(&source),
    };
    write_register(registers, index, value, width)
}

fn lift_imul<'ctx>(
    operands: &[&str],
    registers: &mut [BV<'ctx>],
    context: &'ctx Context,
    width: u32,
) -> bool {
    match operands {
        [source, destination] => {
            let Some((index, destination_width)) = parse_register(destination) else {
                return false;
            };
            if destination_width != width || index == 4 {
                return false;
            }
            let Some(source) = operand_value(source, registers, context, width) else {
                return false;
            };
            let destination = if width == 64 {
                registers[index].clone()
            } else {
                registers[index].extract(width - 1, 0)
            };
            write_register(registers, index, destination.bvmul(&source), width)
        }
        [immediate, source, destination] if width == 64 => {
            if !immediate.starts_with('$') {
                return false;
            }
            let Some(source) = register_value(source, registers, 64) else {
                return false;
            };
            let Some((index, destination_width)) = parse_register(destination) else {
                return false;
            };
            if destination_width != 64 || index == 4 {
                return false;
            }
            let Some(immediate) = operand_value(immediate, registers, context, 64) else {
                return false;
            };
            write_register(registers, index, immediate.bvmul(&source), 64)
        }
        _ => false,
    }
}

fn lift_neg<'ctx>(operands: &[&str], registers: &mut [BV<'ctx>], width: u32) -> bool {
    let [destination] = operands else {
        return false;
    };
    let Some((index, destination_width)) = parse_register(destination) else {
        return false;
    };
    if destination_width != width || index == 4 {
        return false;
    }
    let destination = registers[index].extract(width - 1, 0);
    write_register(registers, index, destination.bvneg(), width)
}

fn lift_inc_dec<'ctx>(
    instruction: &Instruction,
    state: &mut MachineState<'ctx>,
    context: &'ctx Context,
) -> Result<(BV<'ctx>, u32, Bool<'ctx>), FunctionOutcome> {
    let width = instruction_width(&instruction.mnemonic)?;
    let operands = split_operands(&instruction.operands);
    let [destination] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    let (index, destination_width) = parse_register(destination)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    if destination_width != width || (index == 4 && width != 64) {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let previous = if width == 64 {
        state.registers[index].clone()
    } else {
        state.registers[index].extract(width - 1, 0)
    };
    let is_decrement = matches!(instruction.mnemonic.as_str(), "decl" | "decq");
    let one = BV::from_u64(context, 1, width);
    let value = if is_decrement {
        previous.bvsub(&one)
    } else {
        previous.bvadd(&one)
    };
    // inc overflows only at signed max, dec only at signed min; both are
    // exact constant comparisons because the step is exactly one. CF is
    // unaffected by inc/dec in hardware and stays unmodeled here.
    let boundary = if is_decrement {
        BV::from_u64(context, 1u64 << (width - 1), width)
    } else {
        BV::from_u64(context, (1u64 << (width - 1)) - 1, width)
    };
    let overflow = previous._eq(&boundary);
    if index == 4 {
        if !state.rsp_is_known || state.rsp_offset % 8 != 0 {
            return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
        }
        let delta = if is_decrement { -1 } else { 1 };
        if !adjust_rsp(
            &mut state.registers,
            &mut state.stack,
            &mut state.rsp_offset,
            delta,
            context,
        ) {
            return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
        }
    } else if !write_register(&mut state.registers, index, value.clone(), width) {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    Ok((value, width, overflow))
}

fn lift_not<'ctx>(operands: &[&str], registers: &mut [BV<'ctx>], width: u32) -> bool {
    let [destination] = operands else {
        return false;
    };
    let Some((index, destination_width)) = parse_register(destination) else {
        return false;
    };
    if destination_width != width || index == 4 {
        return false;
    }
    let value = registers[index].extract(width - 1, 0).bvnot();
    write_register(registers, index, value, width)
}

fn lift_xor<'ctx>(operands: &[&str], registers: &mut [BV<'ctx>], width: u32) -> bool {
    let [source, destination] = operands else {
        return false;
    };
    let Some(source) = register_value(source, registers, width) else {
        return false;
    };
    let Some((index, destination_width)) = parse_register(destination) else {
        return false;
    };
    if destination_width != width || index == 4 {
        return false;
    }
    let destination = registers[index].extract(width - 1, 0);
    write_register(registers, index, destination.bvxor(&source), width)
}

fn operand_value<'ctx>(
    operand: &str,
    registers: &[BV<'ctx>],
    context: &'ctx Context,
    width: u32,
) -> Option<BV<'ctx>> {
    if let Some(immediate) = operand.strip_prefix('$') {
        Some(BV::from_u64(context, parse_integer(immediate)?, width))
    } else {
        register_value(operand, registers, width)
    }
}

fn register_value<'ctx>(operand: &str, registers: &[BV<'ctx>], width: u32) -> Option<BV<'ctx>> {
    let (index, register_width) = parse_register(operand)?;
    // Registers always hold the semantic 64-bit value, so a full-width
    // `extract(63, 0)` is the identity — skip the redundant node.
    (register_width == width).then(|| {
        if width == 64 {
            registers[index].clone()
        } else {
            registers[index].extract(width - 1, 0)
        }
    })
}

fn write_register<'ctx>(
    registers: &mut [BV<'ctx>],
    index: usize,
    value: BV<'ctx>,
    width: u32,
) -> bool {
    if index >= registers.len() || value.get_size() != width {
        return false;
    }
    let updated = match width {
        8 => registers[index].extract(63, 8).concat(&value),
        32 => value.zero_ext(32),
        64 => value,
        _ => return false,
    };
    registers[index] = updated;
    true
}

fn lea_value<'ctx>(
    operand: &str,
    registers: &[BV<'ctx>],
    context: &'ctx Context,
) -> Option<BV<'ctx>> {
    let open = operand.find('(')?;
    let close = operand.rfind(')')?;
    if close + 1 != operand.len() {
        return None;
    }
    let displacement = operand[..open].trim();
    let components = operand[open + 1..close]
        .split(',')
        .map(str::trim)
        .collect::<Vec<_>>();
    let displacement = if displacement.is_empty() {
        0
    } else {
        parse_integer(displacement)?
    };
    let mut value = BV::from_u64(context, displacement, 64);
    match components.as_slice() {
        [base] if !base.is_empty() => {
            value = value.bvadd(&register_value(base, registers, 64)?);
        }
        [base, index] if !index.is_empty() => {
            if !base.is_empty() {
                value = value.bvadd(&register_value(base, registers, 64)?);
            }
            value = value.bvadd(&register_value(index, registers, 64)?);
        }
        [base, index, scale] if !index.is_empty() && !scale.is_empty() => {
            if !base.is_empty() {
                value = value.bvadd(&register_value(base, registers, 64)?);
            }
            let scale = scale.parse::<u64>().ok()?;
            if !matches!(scale, 1 | 2 | 4 | 8) {
                return None;
            }
            let index = register_value(index, registers, 64)?;
            value = value.bvadd(&index.bvmul(&BV::from_u64(context, scale, 64)));
        }
        _ => {
            return None;
        }
    }
    Some(value)
}

fn split_operands(operands: &str) -> Vec<&str> {
    let operands = operands.trim();
    if operands.is_empty() {
        return Vec::new();
    }
    let mut result = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, character) in operands.char_indices() {
        match character {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                result.push(operands[start..index].trim());
                start = index + 1;
            }
            _ => (),
        }
    }
    result.push(operands[start..].trim());
    result
}

fn parse_xmm_register(register: &str) -> Option<usize> {
    let index = register.strip_prefix("%xmm")?.parse::<usize>().ok()?;
    (index < 16).then_some(index)
}

fn parse_register(register: &str) -> Option<(usize, u32)> {
    Some(match register.strip_prefix('%')? {
        "al" => (0, 8),
        "cl" => (1, 8),
        "dl" => (2, 8),
        "bl" => (3, 8),
        "spl" => (4, 8),
        "bpl" => (5, 8),
        "sil" => (6, 8),
        "dil" => (7, 8),
        "r8b" => (8, 8),
        "r9b" => (9, 8),
        "r10b" => (10, 8),
        "r11b" => (11, 8),
        "r12b" => (12, 8),
        "r13b" => (13, 8),
        "r14b" => (14, 8),
        "r15b" => (15, 8),
        "rax" => (0, 64),
        "eax" => (0, 32),
        "rcx" => (1, 64),
        "ecx" => (1, 32),
        "rdx" => (2, 64),
        "edx" => (2, 32),
        "rbx" => (3, 64),
        "ebx" => (3, 32),
        "rsp" => (4, 64),
        "esp" => (4, 32),
        "rbp" => (5, 64),
        "ebp" => (5, 32),
        "rsi" => (6, 64),
        "esi" => (6, 32),
        "rdi" => (7, 64),
        "edi" => (7, 32),
        "r8" => (8, 64),
        "r8d" => (8, 32),
        "r9" => (9, 64),
        "r9d" => (9, 32),
        "r10" => (10, 64),
        "r10d" => (10, 32),
        "r11" => (11, 64),
        "r11d" => (11, 32),
        "r12" => (12, 64),
        "r12d" => (12, 32),
        "r13" => (13, 64),
        "r13d" => (13, 32),
        "r14" => (14, 64),
        "r14d" => (14, 32),
        "r15" => (15, 64),
        "r15d" => (15, 32),
        _ => return None,
    })
}

fn parse_integer(value: &str) -> Option<u64> {
    let (negative, magnitude) = match value.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, value.strip_prefix('+').unwrap_or(value)),
    };
    let magnitude = if let Some(hex) = magnitude.strip_prefix("0x") {
        u64::from_str_radix(hex, 16).ok()?
    } else if let Some(hex) = magnitude.strip_prefix("0X") {
        u64::from_str_radix(hex, 16).ok()?
    } else {
        magnitude.parse::<u64>().ok()?
    };
    Some(if negative {
        0u64.wrapping_sub(magnitude)
    } else {
        magnitude
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mumei_core::semantic_contract::{encode_pure_i64_module, RefinedAdmission};
    use object::{Object, ObjectSection, ObjectSymbol, RelocationTarget, SymbolKind};
    use std::fs;
    use z3::Config;

    fn source_model<'ctx>(source: &str, context: &'ctx Context) -> PureI64Body<'ctx> {
        source_model_with_admission(source, context, RefinedAdmission::Reject)
    }

    fn source_model_with_admission<'ctx>(
        source: &str,
        context: &'ctx Context,
        admission: RefinedAdmission,
    ) -> PureI64Body<'ctx> {
        encode_pure_i64_module(source, context, true, admission)
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
    }

    fn source_model_named<'ctx>(
        source: &str,
        name: &str,
        context: &'ctx Context,
    ) -> PureI64Body<'ctx> {
        source_model_named_with_admission(source, name, context, RefinedAdmission::Reject)
    }

    fn source_model_named_with_admission<'ctx>(
        source: &str,
        name: &str,
        context: &'ctx Context,
        admission: RefinedAdmission,
    ) -> PureI64Body<'ctx> {
        encode_pure_i64_module(source, context, true, admission)
            .unwrap()
            .into_iter()
            .find(|model| model.function_name == name)
            .unwrap()
    }

    #[test]
    fn array_parameter_signatures_are_outside_machine_tv() {
        let config = Config::new();
        let context = Context::new(&config);
        let model = source_model_with_admission(
            "atom read(a: [i64]) -> i64 requires: true; ensures: result >= 0; body: len(a);",
            &context,
            RefinedAdmission::CompositeV26,
        );
        assert!(model.array_params[0].is_some());
        assert!(!supports_signature(&model));
    }

    fn helper_call_targets(offsets: &[u64]) -> BTreeMap<u64, CallTarget> {
        offsets
            .iter()
            .map(|offset| {
                (
                    *offset,
                    CallTarget {
                        name: "helper".to_owned(),
                        relocation_kind: CallRelocationKind::ElfX86_64Plt32,
                        relocation_size: 32,
                        addend: -4,
                    },
                )
            })
            .collect()
    }

    fn compiled_helper() -> (u64, Vec<u8>) {
        compiled_function(
            r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @helper(i64 %x) {
entry:
  %result = add i64 %x, 1
  ret i64 %result
}
"#,
        )
    }

    fn compiled_function(ir: &str) -> (u64, Vec<u8>) {
        let (address, bytes, _) = compiled_function_with_targets(ir, "helper");
        (address, bytes)
    }

    fn compiled_function_with_targets(
        ir: &str,
        function_name: &str,
    ) -> (u64, Vec<u8>, BTreeMap<u64, CallTarget>) {
        let directory = tempfile::tempdir().unwrap();
        let ir_path = directory.path().join("helper.ll");
        let object_path = directory.path().join("helper.o");
        fs::write(&ir_path, ir).unwrap();
        crate::codegen::compile_llvm_ir_to_object_recorded(&ir_path, &object_path).unwrap();
        let bytes = fs::read(object_path).unwrap();
        let file = object::File::parse(bytes.as_slice()).unwrap();
        let symbol = file
            .symbols()
            .find(|symbol| {
                symbol.name().ok() == Some(function_name)
                    && symbol.is_definition()
                    && symbol.kind() == SymbolKind::Text
            })
            .unwrap();
        let address = symbol.address();
        let size = symbol.size();
        let section_index = symbol.section_index().unwrap();
        let section = file.section_by_index(section_index).unwrap();
        let function_bytes = section.data_range(address, size).unwrap().unwrap().to_vec();
        let mut call_targets = BTreeMap::new();
        for (offset, relocation) in section.relocations() {
            if address <= offset && offset < address + size {
                let RelocationTarget::Symbol(target_index) = relocation.target() else {
                    panic!("compiled direct call has a non-symbol relocation target");
                };
                let target = file
                    .symbol_by_index(target_index)
                    .unwrap()
                    .name()
                    .unwrap()
                    .to_owned();
                let relocation_kind = match relocation.flags() {
                    object::RelocationFlags::Elf { r_type }
                        if r_type == object::elf::R_X86_64_PLT32
                            && relocation.kind() == object::RelocationKind::PltRelative =>
                    {
                        CallRelocationKind::ElfX86_64Plt32
                    }
                    object::RelocationFlags::Elf { r_type }
                        if r_type == object::elf::R_X86_64_PC32
                            && relocation.kind() == object::RelocationKind::Relative =>
                    {
                        CallRelocationKind::ElfX86_64Pc32
                    }
                    _ => CallRelocationKind::Other,
                };
                call_targets.insert(
                    offset - address,
                    CallTarget {
                        name: target,
                        relocation_kind,
                        relocation_size: relocation.size(),
                        addend: relocation.addend(),
                    },
                );
            }
        }
        (address, function_bytes, call_targets)
    }

    #[test]
    fn llvm17_x86_disassembler_is_available() {
        let instructions = disassemble(&[0xb8, 7, 0, 0, 0, 0xc3], 0).unwrap();
        assert_eq!(instructions.len(), 2);
        assert_eq!(instructions[0].offset, 0);
        assert_eq!(instructions[0].mnemonic, "movl");
        assert_eq!(instructions[0].operands, "$7, %eax");
        assert_eq!(instructions[1].offset, 5);
        assert_eq!(instructions[1].mnemonic, "retq");
    }

    #[test]
    fn ir_object_equivalence_compares_select_arithmetic() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        // icmp + select through both encoders over the shared inputs.
        let model = source_model(
            "atom mx(x: i64, y: i64) -> i64 requires: true; ensures: result >= x; body: { if x > y { x } else { y } };",
            &context,
        );
        let ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @helper(i64 %x, i64 %y) {
entry:
  %c = icmp sgt i64 %x, %y
  %r = select i1 %c, i64 %x, i64 %y
  ret i64 %r
}
"#;
        let (address, bytes) = compiled_function(ir);
        let (returns, defined) = unsafe {
            crate::translation_validation::encode_ir_function_for_comparison(
                ir.as_bytes(),
                "helper",
                &model,
                false,
                &context,
            )
        }
        .unwrap();
        assert_eq!(
            check_function_against_ir(
                &bytes,
                address,
                &model,
                &returns,
                &defined,
                &BTreeMap::new(),
                &context,
            ),
            IrObjectEquivalence::Equivalent
        );
    }

    #[test]
    fn ir_object_equivalence_reports_divergent_return() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model(
            "atom add(x: i64, y: i64) -> i64 requires: true; ensures: result == x + y; body: { x + y };",
            &context,
        );
        let ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @helper(i64 %x, i64 %y) {
entry:
  %r = add i64 %x, %y
  ret i64 %r
}
"#;
        // The IR text claims subtraction while the object function is
        // `x + y`: the direct comparison must report a divergence.
        let divergent_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @helper(i64 %x, i64 %y) {
entry:
  %r = sub i64 %x, %y
  ret i64 %r
}
"#;
        let (address, bytes) = compiled_function(ir);
        let (returns, defined) = unsafe {
            crate::translation_validation::encode_ir_function_for_comparison(
                divergent_ir.as_bytes(),
                "helper",
                &model,
                false,
                &context,
            )
        }
        .unwrap();
        assert_eq!(
            check_function_against_ir(
                &bytes,
                address,
                &model,
                &returns,
                &defined,
                &BTreeMap::new(),
                &context,
            ),
            IrObjectEquivalence::Different
        );
    }

    #[test]
    fn ir_object_equivalence_skips_unsupported_signature() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        // A call site with a multi-leaf (`i128`) argument is outside
        // the scalar-i64 call subset.
        let caller = source_model_named_with_admission(
            "atom helper(x: i128) -> i64 requires: true; ensures: true; body: { 0 };\n\
             atom caller(x: i128) -> i64 requires: true; ensures: true; body: { helper(x) };",
            "caller",
            &context,
            RefinedAdmission::CompositeV21,
        );
        assert!(!ir_object_signature_liftable(&caller));
        assert_eq!(
            check_function_against_ir(&[], 0, &caller, &[], &[], &BTreeMap::new(), &context,),
            IrObjectEquivalence::NotLiftable("unsupported_signature")
        );

        // A composite-argument call site inside an otherwise scalar
        // caller is rejected by the call-site gate itself — the
        // caller's own parameters stay inside the scalar-i64 subset.
        let struct_caller = source_model_named_with_admission(
            "struct P { x: i64, y: i64 }\n\
             atom helper(p: P) -> i64 requires: true; ensures: true; body: { p.x };\n\
             atom caller(x: i64) -> i64 requires: true; ensures: true; body: { helper(P { x: x, y: 0 }) };",
            "caller",
            &context,
            RefinedAdmission::CompositeV21,
        );
        assert!(!ir_object_signature_liftable(&struct_caller));
        assert_eq!(
            check_function_against_ir(&[], 0, &struct_caller, &[], &[], &BTreeMap::new(), &context,),
            IrObjectEquivalence::NotLiftable("unsupported_signature")
        );
    }

    #[test]
    fn ir_object_equivalence_admits_wide_inputs() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        // An i128 parameter flattens into two 64-bit input leaves
        // (`[false, false]`); the encoder binds them `hi.concat(lo)`
        // and the machine reads the adjacent argument registers —
        // the signature gate admits the pair when the module enables
        // wide scalars.
        let model = source_model_with_admission(
            "atom wide(x: i128, y: i64) -> i64 requires: true; ensures: result == y; body: { y };",
            &context,
            RefinedAdmission::CompositeV21,
        );
        assert!(ir_object_signature_liftable(&model));
        // An empty IR edge set reaches the `unsupported_ir` check
        // rather than bouncing off the signature gate.
        assert_eq!(
            check_function_against_ir(&[], 0, &model, &[], &[], &BTreeMap::new(), &context,),
            IrObjectEquivalence::NotLiftable("unsupported_ir")
        );
    }

    #[test]
    fn frame_memory_check_uses_ir_edges_as_expected_value() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        // Step 3a: with `check_function_with_frame_memory_against_ir`
        // the expected return comes from the IR edges, not
        // `model.value` — an IR that disagrees with the machine code
        // reports `Different` even though source and machine agree.
        let model = source_model(
            "atom add(x: i64, y: i64) -> i64 requires: true; ensures: result == x + y; body: { x + y };",
            &context,
        );
        let add_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @helper(i64 %x, i64 %y) {
entry:
  %r = add i64 %x, %y
  ret i64 %r
}
"#;
        let divergent_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @helper(i64 %x, i64 %y) {
entry:
  %r = sub i64 %x, %y
  ret i64 %r
}
"#;
        let (address, bytes) = compiled_function(add_ir);
        for (ir, expected) in [
            (add_ir, FunctionOutcome::Equivalent),
            (divergent_ir, FunctionOutcome::Different),
        ] {
            let (ir_returns, ir_definedness) = unsafe {
                crate::translation_validation::encode_ir_function_for_comparison(
                    ir.as_bytes(),
                    "helper",
                    &model,
                    false,
                    &context,
                )
            }
            .unwrap();
            assert_eq!(
                check_function_with_frame_memory_against_ir(
                    &bytes,
                    address,
                    &model,
                    &ir_returns,
                    &ir_definedness,
                    &BTreeMap::new(),
                    &context,
                )
                .0,
                expected
            );
        }
    }

    #[test]
    fn frame_memory_check_compares_scalar_call_results_against_ir() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        // Step 3b: scalar-i64 call sites compare as the same
        // `call_decl` uninterpreted applications on both sides. An IR
        // that passes a different actual argument encodes a different
        // application and reports `Different` — the arg-match
        // obligation is verified, not assumed.
        let source = "atom callee(x: i64) -> i64 requires: true; ensures: true; body: { x };\n\
            atom caller(x: i64) -> i64 requires: true; ensures: true; body: { callee(x) };";
        let model = source_model_named(source, "caller", &context);
        assert_eq!(model.call_sites.len(), 1);
        let matching_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @callee(i64 %x) {
entry:
  ret i64 %x
}
define i64 @caller(i64 %x) {
entry:
  %result = call i64 @callee(i64 %x)
  ret i64 %result
}
"#;
        let divergent_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @callee(i64 %x) {
entry:
  ret i64 %x
}
define i64 @caller(i64 %x) {
entry:
  %result = call i64 @callee(i64 0)
  ret i64 %result
}
"#;
        let (address, bytes, call_targets) = compiled_function_with_targets(matching_ir, "caller");
        for (ir, expected) in [
            (matching_ir, FunctionOutcome::Equivalent),
            (divergent_ir, FunctionOutcome::Different),
        ] {
            let (ir_returns, ir_definedness) = unsafe {
                crate::translation_validation::encode_ir_function_for_comparison(
                    ir.as_bytes(),
                    "caller",
                    &model,
                    false,
                    &context,
                )
            }
            .unwrap();
            assert_eq!(
                check_function_with_frame_memory_against_ir(
                    &bytes,
                    address,
                    &model,
                    &ir_returns,
                    &ir_definedness,
                    &call_targets,
                    &context,
                )
                .0,
                expected
            );
            assert_eq!(
                check_function_against_ir(
                    &bytes,
                    address,
                    &model,
                    &ir_returns,
                    &ir_definedness,
                    &call_targets,
                    &context,
                ),
                match expected {
                    FunctionOutcome::Equivalent => IrObjectEquivalence::Equivalent,
                    _ => IrObjectEquivalence::Different,
                }
            );
        }
    }

    #[test]
    fn encode_ir_for_comparison_renames_native_main_callee() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        // The binary emitter renames the user `main` to
        // `__mumei_user_main`, while the source model's call site
        // binds `call_decl("main", ...)`. Without the native rename
        // map the encoded term is a different uninterpreted function
        // and every call to `main` compares `Different`.
        let source = "atom main(x: i64) -> i64 requires: true; ensures: true; body: { main(x) };";
        let model = source_model_named(source, "main", &context);
        let ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @__mumei_user_main(i64 %x) {
entry:
  %r = call i64 @__mumei_user_main(i64 %x)
  ret i64 %r
}
"#;
        let (returns, _defined) = unsafe {
            crate::translation_validation::encode_ir_function_for_comparison(
                ir.as_bytes(),
                "__mumei_user_main",
                &model,
                false,
                &context,
            )
        }
        .unwrap();
        let expected = mumei_core::semantic_contract::call_decl(&context, "main", 1)
            .apply(&[&model.inputs[0]])
            .as_bv()
            .unwrap();
        assert!(
            returns.iter().any(|(_condition, value)| {
                value._eq(&expected).simplify().as_bool() == Some(true)
            }),
            "call term does not use the source callee name `main`"
        );
    }

    #[test]
    fn frame_memory_check_compares_bounded_memory_against_ir() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        // Step 3c: bounded-memory atoms compare the same alloca-array
        // encoding on both sides — a divergent stored value reports
        // `Different`.
        let model = source_model_named_with_admission(
            "atom pick(a0: i64, i: i64) -> i64 requires: 0 <= i && i < 3; ensures: result == 7; body: { let a = [a0, 20, 30]; a[i] = 7; a[i] };",
            "pick",
            &context,
            RefinedAdmission::CompositeV11,
        );
        assert!(model.bounded_memory);
        let matching_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @pick(i64 %a0, i64 %i) {
entry:
  %a = alloca [3 x i64], align 8
  %p0 = getelementptr [3 x i64], ptr %a, i64 0, i64 0
  store i64 %a0, ptr %p0, align 8
  %p1 = getelementptr [3 x i64], ptr %a, i64 0, i64 1
  store i64 20, ptr %p1, align 8
  %p2 = getelementptr [3 x i64], ptr %a, i64 0, i64 2
  store i64 30, ptr %p2, align 8
  %hi = icmp slt i64 %i, 3
  %lo = icmp sge i64 %i, 0
  %safe = and i1 %hi, %lo
  br i1 %safe, label %store, label %loadcheck
store:
  %pi = getelementptr [3 x i64], ptr %a, i64 0, i64 %i
  store i64 7, ptr %pi, align 8
  br label %loadcheck
loadcheck:
  %hi2 = icmp slt i64 %i, 3
  %lo2 = icmp sge i64 %i, 0
  %safe2 = and i1 %hi2, %lo2
  br i1 %safe2, label %load, label %oob
load:
  %pj = getelementptr [3 x i64], ptr %a, i64 0, i64 %i
  %v = load i64, ptr %pj, align 8
  br label %merge
oob:
  br label %merge
merge:
  %result = phi i64 [ %v, %load ], [ 0, %oob ]
  ret i64 %result
}
"#;
        let divergent_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @pick(i64 %a0, i64 %i) {
entry:
  %a = alloca [3 x i64], align 8
  %p0 = getelementptr [3 x i64], ptr %a, i64 0, i64 0
  store i64 %a0, ptr %p0, align 8
  %p1 = getelementptr [3 x i64], ptr %a, i64 0, i64 1
  store i64 20, ptr %p1, align 8
  %p2 = getelementptr [3 x i64], ptr %a, i64 0, i64 2
  store i64 30, ptr %p2, align 8
  %hi = icmp slt i64 %i, 3
  %lo = icmp sge i64 %i, 0
  %safe = and i1 %hi, %lo
  br i1 %safe, label %store, label %loadcheck
store:
  %pi = getelementptr [3 x i64], ptr %a, i64 0, i64 %i
  store i64 8, ptr %pi, align 8
  br label %loadcheck
loadcheck:
  %hi2 = icmp slt i64 %i, 3
  %lo2 = icmp sge i64 %i, 0
  %safe2 = and i1 %hi2, %lo2
  br i1 %safe2, label %load, label %oob
load:
  %pj = getelementptr [3 x i64], ptr %a, i64 0, i64 %i
  %v = load i64, ptr %pj, align 8
  br label %merge
oob:
  br label %merge
merge:
  %result = phi i64 [ %v, %load ], [ 0, %oob ]
  ret i64 %result
}
"#;
        let (address, bytes, call_targets) = compiled_function_with_targets(matching_ir, "pick");
        for (ir, expected) in [
            (matching_ir, FunctionOutcome::Equivalent),
            (divergent_ir, FunctionOutcome::Different),
        ] {
            let (ir_returns, ir_definedness) = unsafe {
                crate::translation_validation::encode_ir_function_for_comparison(
                    ir.as_bytes(),
                    "pick",
                    &model,
                    false,
                    &context,
                )
            }
            .unwrap();
            assert_eq!(
                check_function_with_frame_memory_against_ir(
                    &bytes,
                    address,
                    &model,
                    &ir_returns,
                    &ir_definedness,
                    &call_targets,
                    &context,
                )
                .0,
                expected
            );
            assert_eq!(
                check_function_against_ir(
                    &bytes,
                    address,
                    &model,
                    &ir_returns,
                    &ir_definedness,
                    &call_targets,
                    &context,
                ),
                match expected {
                    FunctionOutcome::Equivalent => IrObjectEquivalence::Equivalent,
                    _ => IrObjectEquivalence::Different,
                }
            );
        }
    }

    #[test]
    fn frame_memory_check_compares_aggregate_result_against_ir() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        // A register-resident all-integer aggregate result (two `i64`
        // leaves returning in rax/rdx) joins the comparison subset:
        // the IR `ret {i64, i64}` encodes as its aggregate slots —
        // leaf 0 high — and the machine side folds the same leaves,
        // so a divergent leaf compares `Different`.
        let model = source_model_named_with_admission(
            "struct P { x: i64, y: i64 }\n\
             atom make(a: i64, b: i64) -> P requires: true; ensures: true; body: { P { x: a, y: b + 1 } };",
            "make",
            &context,
            RefinedAdmission::CompositeV6,
        );
        assert_eq!(model.result_shape, Some(2));
        let matching_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define { i64, i64 } @make(i64 %a, i64 %b) {
entry:
  %t = add i64 %b, 1
  %p0 = insertvalue { i64, i64 } undef, i64 %a, 0
  %p1 = insertvalue { i64, i64 } %p0, i64 %t, 1
  ret { i64, i64 } %p1
}
"#;
        let divergent_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define { i64, i64 } @make(i64 %a, i64 %b) {
entry:
  %t = add i64 %b, 2
  %p0 = insertvalue { i64, i64 } undef, i64 %a, 0
  %p1 = insertvalue { i64, i64 } %p0, i64 %t, 1
  ret { i64, i64 } %p1
}
"#;
        let (address, bytes, call_targets) = compiled_function_with_targets(matching_ir, "make");
        for (ir, expected) in [
            (matching_ir, FunctionOutcome::Equivalent),
            (divergent_ir, FunctionOutcome::Different),
        ] {
            let (ir_returns, ir_definedness) = unsafe {
                crate::translation_validation::encode_ir_function_for_comparison(
                    ir.as_bytes(),
                    "make",
                    &model,
                    false,
                    &context,
                )
            }
            .unwrap();
            assert_eq!(
                check_function_with_frame_memory_against_ir(
                    &bytes,
                    address,
                    &model,
                    &ir_returns,
                    &ir_definedness,
                    &call_targets,
                    &context,
                )
                .0,
                expected
            );
            assert_eq!(
                check_function_against_ir(
                    &bytes,
                    address,
                    &model,
                    &ir_returns,
                    &ir_definedness,
                    &call_targets,
                    &context,
                ),
                match expected {
                    FunctionOutcome::Equivalent => IrObjectEquivalence::Equivalent,
                    _ => IrObjectEquivalence::Different,
                }
            );
        }
    }

    #[test]
    fn frame_memory_check_compares_sret_aggregate_result_against_ir() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        // A four-`i64` aggregate result exceeds the three INTEGER
        // result registers, so LLVM lowers it memory-class: the callee
        // stores leaf `i` at byte offset `8 * i` through the hidden
        // `sret` pointer in rdi and hands the pointer back in rax.
        // The machine side reads `sret_stores`; the IR side still
        // encodes `ret {i64, i64, i64, i64}` as its aggregate slots —
        // the same leaf-0-high fold on both sides.
        let model = source_model_named_with_admission(
            "struct Q { a: i64, b: i64, c: i64, d: i64 }\n\
             atom make(a: i64, b: i64, c: i64, d: i64) -> Q requires: true; ensures: true; body: { Q { a: a + 1, b: b, c: c, d: d } };",
            "make",
            &context,
            RefinedAdmission::CompositeV6,
        );
        assert_eq!(model.result_shape, Some(4));
        let matching_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define { i64, i64, i64, i64 } @make(i64 %a, i64 %b, i64 %c, i64 %d) {
entry:
  %t = add i64 %a, 1
  %s0 = insertvalue { i64, i64, i64, i64 } undef, i64 %t, 0
  %s1 = insertvalue { i64, i64, i64, i64 } %s0, i64 %b, 1
  %s2 = insertvalue { i64, i64, i64, i64 } %s1, i64 %c, 2
  %s3 = insertvalue { i64, i64, i64, i64 } %s2, i64 %d, 3
  ret { i64, i64, i64, i64 } %s3
}
"#;
        let divergent_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define { i64, i64, i64, i64 } @make(i64 %a, i64 %b, i64 %c, i64 %d) {
entry:
  %t = add i64 %a, 2
  %s0 = insertvalue { i64, i64, i64, i64 } undef, i64 %t, 0
  %s1 = insertvalue { i64, i64, i64, i64 } %s0, i64 %b, 1
  %s2 = insertvalue { i64, i64, i64, i64 } %s1, i64 %c, 2
  %s3 = insertvalue { i64, i64, i64, i64 } %s2, i64 %d, 3
  ret { i64, i64, i64, i64 } %s3
}
"#;
        let (address, bytes, call_targets) = compiled_function_with_targets(matching_ir, "make");
        for (ir, expected) in [
            (matching_ir, FunctionOutcome::Equivalent),
            (divergent_ir, FunctionOutcome::Different),
        ] {
            let (ir_returns, ir_definedness) = unsafe {
                crate::translation_validation::encode_ir_function_for_comparison(
                    ir.as_bytes(),
                    "make",
                    &model,
                    false,
                    &context,
                )
            }
            .unwrap();
            assert_eq!(
                check_function_with_frame_memory_against_ir(
                    &bytes,
                    address,
                    &model,
                    &ir_returns,
                    &ir_definedness,
                    &call_targets,
                    &context,
                )
                .0,
                expected
            );
            assert_eq!(
                check_function_against_ir(
                    &bytes,
                    address,
                    &model,
                    &ir_returns,
                    &ir_definedness,
                    &call_targets,
                    &context,
                ),
                match expected {
                    FunctionOutcome::Equivalent => IrObjectEquivalence::Equivalent,
                    _ => IrObjectEquivalence::Different,
                }
            );
        }
    }

    #[test]
    fn frame_memory_check_compares_scalar_i128_against_ir() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        // A scalar i128 carries two 64-bit leaves (`lo` in the lower
        // argument register, `hi` in the next) and returns in
        // `rdx:rax`. Both sides then compare one full 128-bit
        // bit-vector edge — the IR encoder binds the two input slots
        // into a native `i128`, the machine side concatenates the
        // result registers `hi:lo`.
        let model = source_model_named_with_admission(
            "atom add(x: i128, y: i128) -> i128 requires: true; ensures: result == x + y; body: { x + y };",
            "add",
            &context,
            RefinedAdmission::CompositeV21,
        );
        assert!(model.result_shape.is_none());
        assert_eq!(model.value.get_size(), 128);
        let matching_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i128 @add(i128 %x, i128 %y) {
entry:
  %r = add i128 %x, %y
  ret i128 %r
}
"#;
        let divergent_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i128 @add(i128 %x, i128 %y) {
entry:
  %r = sub i128 %x, %y
  ret i128 %r
}
"#;
        // A divergence confined to the high 64 bits proves both APIs
        // compare the whole 128-bit edge, not just `rax`.
        let high_bit_divergent_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i128 @add(i128 %x, i128 %y) {
entry:
  %r = add i128 %x, %y
  %h = add i128 %r, 18446744073709551616
  ret i128 %h
}
"#;
        let (address, bytes, call_targets) = compiled_function_with_targets(matching_ir, "add");
        for (ir, expected) in [
            (matching_ir, FunctionOutcome::Equivalent),
            (divergent_ir, FunctionOutcome::Different),
            (high_bit_divergent_ir, FunctionOutcome::Different),
        ] {
            let (ir_returns, ir_definedness) = unsafe {
                crate::translation_validation::encode_ir_function_for_comparison(
                    ir.as_bytes(),
                    "add",
                    &model,
                    false,
                    &context,
                )
            }
            .unwrap();
            assert_eq!(
                check_function_with_frame_memory_against_ir(
                    &bytes,
                    address,
                    &model,
                    &ir_returns,
                    &ir_definedness,
                    &call_targets,
                    &context,
                )
                .0,
                expected
            );
            assert_eq!(
                check_function_against_ir(
                    &bytes,
                    address,
                    &model,
                    &ir_returns,
                    &ir_definedness,
                    &call_targets,
                    &context,
                ),
                match expected {
                    FunctionOutcome::Equivalent => IrObjectEquivalence::Equivalent,
                    _ => IrObjectEquivalence::Different,
                }
            );
        }
    }

    #[test]
    fn ir_object_equivalence_compares_flat_mixed_register_pairs() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        for (fields, ir_type, int_index, double_index) in [
            ("x: i64, y: f64", "{ i64, double }", 0, 1),
            ("y: f64, x: i64", "{ double, i64 }", 1, 0),
        ] {
            let source = format!(
                "struct P {{ {fields} }}\n\
                 atom make(x: i64, y: f64, z: f64) -> P requires: true; ensures: true; body: {{ P {{ x: x, y: y }} }};"
            );
            let model = source_model_named_with_admission(
                &source,
                "make",
                &context,
                RefinedAdmission::CompositeV23,
            );
            assert!(ir_object_signature_liftable(&model));
            let ir = |double| {
                format!(
                    r#"target triple = "x86_64-unknown-linux-gnu"
define {ir_type} @make(i64 %x, double %y, double %z) {{
  %first = insertvalue {ir_type} undef, i64 %x, {int_index}
  %result = insertvalue {ir_type} %first, double {double}, {double_index}
  ret {ir_type} %result
}}
"#
                )
            };
            let matching_ir = ir("%y");
            let (ir_returns, ir_definedness) = unsafe {
                crate::translation_validation::encode_ir_function_for_comparison(
                    matching_ir.as_bytes(),
                    "make",
                    &model,
                    false,
                    &context,
                )
            }
            .unwrap();
            for (lowering, expected, expected_with_frame) in [
                (
                    matching_ir,
                    IrObjectEquivalence::Equivalent,
                    FunctionOutcome::Equivalent,
                ),
                (
                    ir("%z"),
                    IrObjectEquivalence::Different,
                    FunctionOutcome::Different,
                ),
            ] {
                let (address, bytes, targets) = compiled_function_with_targets(&lowering, "make");
                assert_eq!(
                    check_function_against_ir(
                        &bytes,
                        address,
                        &model,
                        &ir_returns,
                        &ir_definedness,
                        &targets,
                        &context,
                    ),
                    expected,
                    "{ir_type}",
                );
                assert_eq!(
                    check_function_with_frame_memory_against_ir(
                        &bytes,
                        address,
                        &model,
                        &ir_returns,
                        &ir_definedness,
                        &targets,
                        &context,
                    )
                    .0,
                    expected_with_frame,
                    "{ir_type}",
                );
            }
        }
    }

    #[test]
    fn ir_object_equivalence_compares_mixed_register_pair_nan_payloads() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model_named_with_admission(
            "struct P { x: i64, y: f64 }\n\
             atom make(x: i64, y: f64) -> P requires: true; ensures: true; body: { P { x: x, y: y } };",
            "make",
            &context,
            RefinedAdmission::CompositeV23,
        );
        let bytes = [
            0x48, 0xb8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf8,
            0x7f, // movabsq $0x7FF8000000000000, %rax
            0x66, 0x48, 0x0f, 0x6e, 0xc0, // movq %rax, %xmm0
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0xc3, // retq
        ];
        let targets = BTreeMap::new();
        for (nan, expected, expected_with_frame) in [
            (
                "0x7FF8000000000000",
                IrObjectEquivalence::Equivalent,
                FunctionOutcome::Equivalent,
            ),
            (
                "0x7FF8000000000001",
                IrObjectEquivalence::Different,
                FunctionOutcome::Different,
            ),
        ] {
            let ir = format!(
                r#"target triple = "x86_64-unknown-linux-gnu"
define {{ i64, double }} @make(i64 %x, double %y) {{
  %first = insertvalue {{ i64, double }} undef, i64 %x, 0
  %result = insertvalue {{ i64, double }} %first, double {nan}, 1
  ret {{ i64, double }} %result
}}
"#
            );
            let (ir_returns, ir_definedness) = unsafe {
                crate::translation_validation::encode_ir_function_for_comparison(
                    ir.as_bytes(),
                    "make",
                    &model,
                    false,
                    &context,
                )
            }
            .unwrap();
            assert_eq!(
                check_function_against_ir(
                    &bytes,
                    0,
                    &model,
                    &ir_returns,
                    &ir_definedness,
                    &targets,
                    &context,
                ),
                expected,
                "{nan}",
            );
            assert_eq!(
                check_function_with_frame_memory_against_ir(
                    &bytes,
                    0,
                    &model,
                    &ir_returns,
                    &ir_definedness,
                    &targets,
                    &context,
                )
                .0,
                expected_with_frame,
                "{nan}",
            );
        }
    }

    #[test]
    fn ir_object_equivalence_rejects_nested_mixed_register_pair() {
        let context = Context::new(&Config::new());
        let model = source_model_named_with_admission(
            "struct P { x: i64, y: f64 }\n\
             atom make(x: i64, y: f64) -> P requires: true; ensures: true; body: { P { x: x, y: y } };",
            "make",
            &context,
            RefinedAdmission::CompositeV23,
        );
        assert!(ir_object_signature_liftable(&model));
        let nested_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define { { i64 }, double } @make(i64 %x, double %y) {
  %inner = insertvalue { i64 } undef, i64 %x, 0
  %first = insertvalue { { i64 }, double } undef, { i64 } %inner, 0
  %result = insertvalue { { i64 }, double } %first, double %y, 1
  ret { { i64 }, double } %result
}
"#;
        let error = unsafe {
            crate::translation_validation::encode_ir_function_for_comparison(
                nested_ir.as_bytes(),
                "make",
                &model,
                false,
                &context,
            )
        }
        .unwrap_err();
        assert_eq!(error.0, "LLVM return outside comparison subset");
    }

    #[test]
    fn ir_object_equivalence_rejects_wide_aggregate_result() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        // `{i128}` and `{i64, i64}` flatten to the same signature —
        // two non-`double` 64-bit slots — so a count+kinds match
        // cannot keep a 128-bit leaf out of the subset; the
        // encode-side `aggregate_all_i64_leaves` gate fails it
        // closed instead.
        let model = source_model_named_with_admission(
            "struct P { x: i64, y: i64 }\n\
             atom make(a: i64, b: i64) -> P requires: true; ensures: true; body: { P { x: a, y: b } };",
            "make",
            &context,
            RefinedAdmission::CompositeV6,
        );
        assert_eq!(model.result_shape, Some(2));
        // `ret undef` keeps the body inside the shared gate, so the
        // comparison subset's own return check is what rejects: the
        // `{i128}` signature matches the model's field count and leaf
        // kinds, and only `aggregate_all_i64_leaves` fails it closed.
        let wide_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define { i128 } @make(i64 %a, i64 %b) {
entry:
  ret { i128 } undef
}
"#;
        let error = unsafe {
            crate::translation_validation::encode_ir_function_for_comparison(
                wide_ir.as_bytes(),
                "make",
                &model,
                false,
                &context,
            )
        }
        .unwrap_err();
        assert_eq!(error.0, "LLVM return outside comparison subset");
        // A `double` leaf is rejected too — earlier, at the shared
        // signature check whose leaf kinds no longer match the model's.
        let f64_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define { i64, double } @make(i64 %a, i64 %b) {
entry:
  ret { i64, double } undef
}
"#;
        assert!(unsafe {
            crate::translation_validation::encode_ir_function_for_comparison(
                f64_ir.as_bytes(),
                "make",
                &model,
                false,
                &context,
            )
        }
        .is_err());
    }

    #[test]
    fn frame_memory_check_compares_scalar_f64_against_ir() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        // A scalar `f64` signature compares its IEEE-754 bit
        // pattern: xmm-bound parameters and the xmm0 result are one
        // 64-bit leaf each, pinned to `double` IR parameters by the
        // shared leaf-kind check. Pass-through and `select` only —
        // FP arithmetic stays gated out by `f64_arithmetic: false`.
        let model = source_model_named_with_admission(
            "atom pick(a: f64, b: f64) -> f64 requires: true; ensures: result == a; body: { a };",
            "pick",
            &context,
            RefinedAdmission::CompositeV19,
        );
        assert!(ir_object_signature_liftable(&model));
        let matching_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define double @pick(double %a, double %b) {
entry:
  ret double %a
}
"#;
        let divergent_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define double @pick(double %a, double %b) {
entry:
  ret double %b
}
"#;
        // NaN payloads compare bit-for-bit, not under the source
        // model's `f64_values_match` NaN equivalence: a machine
        // return carrying the canonical quiet NaN matches the
        // canonical-NaN IR edge but reports `Different` against a
        // payload-1 NaN edge even though both are NaN.
        let quiet_nan_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define double @pick(double %a, double %b) {
entry:
  ret double 0x7FF8000000000000
}
"#;
        let payload_nan_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define double @pick(double %a, double %b) {
entry:
  ret double 0x7FF8000000000001
}
"#;
        // `ret double <const>` compiles to a rip-relative `movsd`
        // the lifter does not model — spell the constant return in
        // GPR form the lifter does cover.
        let nan_address = 0u64;
        let nan_bytes = [
            0x48, 0xb8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf8,
            0x7f, // movabsq $0x7FF8000000000000, %rax
            0x66, 0x48, 0x0f, 0x6e, 0xc0, // movq %rax, %xmm0
            0xc3, // retq
        ];
        let nan_targets = BTreeMap::new();
        for (ir, expected) in [
            (quiet_nan_ir, FunctionOutcome::Equivalent),
            (payload_nan_ir, FunctionOutcome::Different),
        ] {
            let (ir_returns, ir_definedness) = unsafe {
                crate::translation_validation::encode_ir_function_for_comparison(
                    ir.as_bytes(),
                    "pick",
                    &model,
                    false,
                    &context,
                )
            }
            .unwrap();
            assert_eq!(
                check_function_with_frame_memory_against_ir(
                    &nan_bytes,
                    nan_address,
                    &model,
                    &ir_returns,
                    &ir_definedness,
                    &nan_targets,
                    &context,
                )
                .0,
                expected
            );
            assert_eq!(
                check_function_against_ir(
                    &nan_bytes,
                    nan_address,
                    &model,
                    &ir_returns,
                    &ir_definedness,
                    &nan_targets,
                    &context,
                ),
                match expected {
                    FunctionOutcome::Equivalent => IrObjectEquivalence::Equivalent,
                    _ => IrObjectEquivalence::Different,
                }
            );
        }
        let (address, bytes, call_targets) = compiled_function_with_targets(matching_ir, "pick");
        for (ir, expected) in [
            (matching_ir, FunctionOutcome::Equivalent),
            (divergent_ir, FunctionOutcome::Different),
        ] {
            let (ir_returns, ir_definedness) = unsafe {
                crate::translation_validation::encode_ir_function_for_comparison(
                    ir.as_bytes(),
                    "pick",
                    &model,
                    false,
                    &context,
                )
            }
            .unwrap();
            assert_eq!(
                check_function_with_frame_memory_against_ir(
                    &bytes,
                    address,
                    &model,
                    &ir_returns,
                    &ir_definedness,
                    &call_targets,
                    &context,
                )
                .0,
                expected
            );
            assert_eq!(
                check_function_against_ir(
                    &bytes,
                    address,
                    &model,
                    &ir_returns,
                    &ir_definedness,
                    &call_targets,
                    &context,
                ),
                match expected {
                    FunctionOutcome::Equivalent => IrObjectEquivalence::Equivalent,
                    _ => IrObjectEquivalence::Different,
                }
            );
        }
    }

    #[test]
    fn ir_object_equivalence_rejects_ir_arity_mismatch() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        // The direct-comparison encoder reuses the validation header
        // gate: an IR function whose parameter count differs from the
        // source signature fails to encode instead of binding inputs
        // to the wrong positions.
        let model = source_model(
            "atom add(x: i64, y: i64) -> i64 requires: true; ensures: result == x + y; body: { x + y };",
            &context,
        );
        for arity_ir in [
            r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @helper(i64 %x) {
entry:
  ret i64 %x
}
"#,
            r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @helper(i64 %x, i64 %y, i64 %z) {
entry:
  %r = add i64 %x, %y
  ret i64 %r
}
"#,
        ] {
            assert!(unsafe {
                crate::translation_validation::encode_ir_function_for_comparison(
                    arity_ir.as_bytes(),
                    "helper",
                    &model,
                    false,
                    &context,
                )
            }
            .is_err());
        }
        // A direct call whose result and arguments are scalar 64-bit
        // leaves stays inside the comparison subset (Step 3b).
        assert!(unsafe {
            crate::translation_validation::encode_ir_function_for_comparison(
                r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @callee(i64 %x) {
entry:
  ret i64 %x
}
define i64 @helper(i64 %x, i64 %y) {
entry:
  %r = call i64 @callee(i64 %x)
  ret i64 %r
}
"#
                .as_bytes(),
                "helper",
                &model,
                false,
                &context,
            )
        }
        .is_ok());
        // The shared gate admits shapes the scalar-i64 comparison
        // subset excludes: an `i128` return (the gate allows 64|128
        // bits for a scalar result) and a call whose result is not a
        // 64-bit scalar both encode unless the comparison path
        // rejects them itself. Assert the error names the
        // comparison-only gate: the shared validation gate admits
        // both shapes, so an `is_err` alone could hide an earlier
        // parse or header failure.
        for (outside_subset_ir, expected) in [
            (
                r#"target triple = "x86_64-unknown-linux-gnu"
define i128 @helper(i64 %x, i64 %y) {
entry:
  %a = zext i64 %x to i128
  %b = zext i64 %y to i128
  %r = add i128 %a, %b
  ret i128 %r
}
"#,
                "return outside comparison subset",
            ),
            (
                r#"target triple = "x86_64-unknown-linux-gnu"
define i128 @callee(i64 %x) {
entry:
  %r = zext i64 %x to i128
  ret i128 %r
}
define i64 @helper(i64 %x, i64 %y) {
entry:
  %w = call i128 @callee(i64 %x)
  ret i64 %x
}
"#,
                "call outside comparison subset",
            ),
        ] {
            let error = unsafe {
                crate::translation_validation::encode_ir_function_for_comparison(
                    outside_subset_ir.as_bytes(),
                    "helper",
                    &model,
                    false,
                    &context,
                )
            }
            .unwrap_err();
            assert!(
                format!("{error:?}").contains(expected),
                "unexpected error: {error:?}"
            );
        }
    }

    #[test]
    fn integer_struct_pairs_match_real_llvm_codegen_and_preserve_leaf_order() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let source = "struct P { x: i64, y: i64 }\n\
            atom sum(p: P) -> i64 requires: true; ensures: true; body: { p.x + p.y };\n\
            atom mk(a: i64, b: i64) -> P requires: true; ensures: true; body: { P { x: a, y: b } };\n\
            atom mk_swapped(a: i64, b: i64) -> P requires: true; ensures: true; body: { P { x: b, y: a } };";
        let sum_model = source_model_named_with_admission(
            source,
            "sum",
            &context,
            RefinedAdmission::CompositeV23,
        );
        let mk_model = source_model_named_with_admission(
            source,
            "mk",
            &context,
            RefinedAdmission::CompositeV23,
        );
        let swapped_model = source_model_named_with_admission(
            source,
            "mk_swapped",
            &context,
            RefinedAdmission::CompositeV23,
        );
        assert!(sum_model.has_composite_or_enum_features);
        assert!(!sum_model.has_composite_features_beyond_struct_values);
        assert_eq!(sum_model.param_shapes, vec![Some(2)]);
        assert_eq!(mk_model.result_shape, Some(2));
        assert!(supports_signature(&sum_model));
        assert!(supports_signature(&mk_model));

        let sum_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @sum({ i64, i64 } %pair) {
entry:
  %x = extractvalue { i64, i64 } %pair, 0
  %y = extractvalue { i64, i64 } %pair, 1
  %result = add i64 %x, %y
  ret i64 %result
}
"#;
        let (sum_address, sum_bytes, sum_targets) = compiled_function_with_targets(sum_ir, "sum");
        let sum_disassembly = disassemble(&sum_bytes, sum_address)
            .unwrap()
            .iter()
            .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
            .collect::<Vec<_>>()
            .join("\n");
        println!("sum struct disassembly:\n{sum_disassembly}");
        assert_eq!(
            check_function(&sum_bytes, sum_address, &sum_model, &sum_targets, &context),
            FunctionOutcome::Equivalent
        );

        let mk_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define { i64, i64 } @mk(i64 %a, i64 %b) {
entry:
  %first = insertvalue { i64, i64 } undef, i64 %a, 0
  %result = insertvalue { i64, i64 } %first, i64 %b, 1
  ret { i64, i64 } %result
}
"#;
        let (mk_address, mk_bytes, mk_targets) = compiled_function_with_targets(mk_ir, "mk");
        let mk_disassembly = disassemble(&mk_bytes, mk_address)
            .unwrap()
            .iter()
            .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
            .collect::<Vec<_>>()
            .join("\n");
        println!("mk struct disassembly:\n{mk_disassembly}");
        assert_eq!(
            check_function(&mk_bytes, mk_address, &mk_model, &mk_targets, &context),
            FunctionOutcome::Equivalent
        );
        assert_eq!(
            check_function(&mk_bytes, mk_address, &swapped_model, &mk_targets, &context),
            FunctionOutcome::Different
        );
    }

    #[test]
    fn struct_writes_and_merged_values_match_real_llvm_codegen() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let source = "struct P { x: i64, y: i64 }\n\
            struct I { a: i64, b: i64 }\n\
            struct O { i: I, c: i64 }\n\
            atom setx(p: P, v: i64) -> P requires: true; ensures: true; body: { p.x = v; p };\n\
            atom bump(o: O, w: i64) -> O requires: true; ensures: true; body: { o.i.a = w; o };\n\
            atom merged(c: i64, a: i64, b: i64, d: i64, e: i64) -> P requires: true; ensures: true; body: { if c > 0 { P { x: a, y: b } } else { P { x: d, y: e } } };";
        let model = |name| {
            source_model_named_with_admission(
                source,
                name,
                &context,
                RefinedAdmission::CompositeV23,
            )
        };
        let setx_model = model("setx");
        let bump_model = model("bump");
        let merged_model = model("merged");
        assert!(setx_model.has_composite_features_beyond_struct_values);
        assert!(bump_model.has_composite_features_beyond_struct_values);
        assert!(merged_model.has_composite_features_beyond_struct_values);
        assert!(!setx_model.has_enum_composite_features);
        assert!(!bump_model.has_enum_composite_features);
        assert!(!merged_model.has_enum_composite_features);

        let setx_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define { i64, i64 } @setx({ i64, i64 } %p, i64 %v) {
entry:
  %p.x = insertvalue { i64, i64 } %p, i64 %v, 0
  ret { i64, i64 } %p.x
}
"#;
        let (setx_address, setx_bytes, setx_targets) =
            compiled_function_with_targets(setx_ir, "setx");
        assert_eq!(
            check_function(
                &setx_bytes,
                setx_address,
                &setx_model,
                &setx_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );

        let wrong_field_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define { i64, i64 } @setx({ i64, i64 } %p, i64 %v) {
entry:
  %p.y = insertvalue { i64, i64 } %p, i64 %v, 1
  ret { i64, i64 } %p.y
}
"#;
        let (wrong_address, wrong_bytes, wrong_targets) =
            compiled_function_with_targets(wrong_field_ir, "setx");
        assert_eq!(
            check_function(
                &wrong_bytes,
                wrong_address,
                &setx_model,
                &wrong_targets,
                &context
            ),
            FunctionOutcome::Different
        );

        let bump_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define { { i64, i64 }, i64 } @bump({ { i64, i64 }, i64 } %o, i64 %w) {
entry:
  %inner = extractvalue { { i64, i64 }, i64 } %o, 0
  %inner.a = insertvalue { i64, i64 } %inner, i64 %w, 0
  %result = insertvalue { { i64, i64 }, i64 } %o, { i64, i64 } %inner.a, 0
  ret { { i64, i64 }, i64 } %result
}
"#;
        let (bump_address, bump_bytes, bump_targets) =
            compiled_function_with_targets(bump_ir, "bump");
        assert_eq!(
            check_function(
                &bump_bytes,
                bump_address,
                &bump_model,
                &bump_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );

        let merged_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define { i64, i64 } @merged(i64 %c, i64 %a, i64 %b, i64 %d, i64 %e) {
entry:
  %condition = icmp sgt i64 %c, 0
  br i1 %condition, label %then, label %else
then:
  %then.x = insertvalue { i64, i64 } poison, i64 %a, 0
  %then.result = insertvalue { i64, i64 } %then.x, i64 %b, 1
  br label %join
else:
  %else.x = insertvalue { i64, i64 } poison, i64 %d, 0
  %else.result = insertvalue { i64, i64 } %else.x, i64 %e, 1
  br label %join
join:
  %result = phi { i64, i64 } [ %then.result, %then ], [ %else.result, %else ]
  ret { i64, i64 } %result
}
"#;
        let (merged_address, merged_bytes, merged_targets) =
            compiled_function_with_targets(merged_ir, "merged");
        assert_eq!(
            check_function(
                &merged_bytes,
                merged_address,
                &merged_model,
                &merged_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );

        let match_source = "struct P { x: i64, y: i64 }\n\
            atom pick(c: i64, a: i64, b: i64) -> P requires: true; ensures: true; body: { match c { 0 => P { x: a, y: b }, _ => P { x: b, y: a } } };";
        let match_model = source_model_named_with_admission(
            match_source,
            "pick",
            &context,
            RefinedAdmission::CompositeV23,
        );
        assert!(match_model.has_composite_features_beyond_struct_values);
        assert!(!match_model.has_enum_composite_features);

        let match_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define { i64, i64 } @pick(i64 %0, i64 %1, i64 %2) {
entry:
  br label %match.try_0
match.merge:
  %match_result = phi { i64, i64 } [ %struct_y, %match.body_0 ], [ %struct_y2, %match.body_1 ], [ zeroinitializer, %match.unreachable ]
  ret { i64, i64 } %match_result
match.unreachable:
  br label %match.merge
match.try_0:
  %pat_lit_eq = icmp eq i64 %0, 0
  br i1 %pat_lit_eq, label %match.body_0, label %match.try_1
match.try_1:
  br i1 true, label %match.body_1, label %match.unreachable
match.body_0:
  %struct_x = insertvalue { i64, i64 } undef, i64 %1, 0
  %struct_y = insertvalue { i64, i64 } %struct_x, i64 %2, 1
  br label %match.merge
match.body_1:
  %struct_x1 = insertvalue { i64, i64 } undef, i64 %2, 0
  %struct_y2 = insertvalue { i64, i64 } %struct_x1, i64 %1, 1
  br label %match.merge
}
"#;
        let (match_address, match_bytes, match_targets) =
            compiled_function_with_targets(match_ir, "pick");
        assert_eq!(
            check_function(
                &match_bytes,
                match_address,
                &match_model,
                &match_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );
    }

    #[test]
    fn enum_signatures_and_values_match_real_llvm_codegen() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = |source: &str, name| {
            source_model_named_with_admission(
                source,
                name,
                &context,
                RefinedAdmission::CompositeV23,
            )
        };
        let opt_source = "enum Opt { Some(v: i64), Nothing }\n\
            atom mk(x: i64) -> Opt requires: true; ensures: true; body: { if x > 0 { Opt::Some(x) } else { Opt::Nothing } };\n\
            atom pick(o: Opt) -> i64 requires: true; ensures: true; body: { match o { Opt::Some(v) => v, _ => 0 } };\n\
            atom get(o: Opt) -> i64 requires: true; ensures: true; body: { match o { Opt::Some(v) => v, _ => 0 } };\n\
            atom main() -> i64 requires: true; ensures: true; body: { get(mk(5)) };";
        let c_source = "enum C { T(p: i64), U }\n\
            atom bump(w: i64) -> C requires: true; ensures: result == C::T(w); body: { let t = C::T(0); t.p = w; t };";
        let chan_source = "enum Chan { Tcp(p: i64), Idle }\n\
            atom is_idle(c: Chan) -> bool requires: true; ensures: true; body: { c == Chan::Idle };\n\
            atom differs(a: Chan, b: Chan) -> bool requires: true; ensures: true; body: { a != b };";
        let e_source = "enum E { A(x: i64, y: i64), B }\n\
            atom make3(x: i64, y: i64, c: i64) -> E requires: true; ensures: true; body: { if c > 0 { E::A(x, y) } else { E::B } };";

        let mk_model = model(opt_source, "mk");
        assert!(mk_model.has_enum_composite_features);
        assert!(mk_model.has_enum_signature);
        assert!(supports_signature(&mk_model));
        let mk_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define { i64, i64 } @mk(i64 %0) {
entry:
  %cmp_tmp = icmp sgt i64 %0, 0
  %bool_tmp = zext i1 %cmp_tmp to i64
  %if_cond = icmp ne i64 %bool_tmp, 0
  br i1 %if_cond, label %then, label %else
then:
  %payload_0 = insertvalue { i64, i64 } zeroinitializer, i64 %0, 1
  br label %merge
else:
  br label %merge
merge:
  %if_result = phi { i64, i64 } [ %payload_0, %then ], [ { i64 1, i64 0 }, %else ]
  ret { i64, i64 } %if_result
}
"#;
        let (mk_address, mk_bytes, mk_targets) = compiled_function_with_targets(mk_ir, "mk");
        assert_eq!(
            check_function(&mk_bytes, mk_address, &mk_model, &mk_targets, &context),
            FunctionOutcome::Equivalent
        );

        let tag_immediate = mk_bytes
            .windows(5)
            .position(|window| window[0] == 0xb8 && window[1..] == [0x01, 0x00, 0x00, 0x00])
            .unwrap();
        let mut wrong_tag_bytes = mk_bytes.clone();
        wrong_tag_bytes[tag_immediate + 1] = 0;
        assert_eq!(
            check_function(
                &wrong_tag_bytes,
                mk_address,
                &mk_model,
                &mk_targets,
                &context
            ),
            FunctionOutcome::Different
        );

        let swapped_result_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define { i64, i64 } @mk(i64 %0) {
entry:
  %cmp_tmp = icmp sgt i64 %0, 0
  br i1 %cmp_tmp, label %then, label %else
then:
  %swapped = insertvalue { i64, i64 } zeroinitializer, i64 %0, 0
  br label %merge
else:
  br label %merge
merge:
  %if_result = phi { i64, i64 } [ %swapped, %then ], [ { i64 0, i64 1 }, %else ]
  ret { i64, i64 } %if_result
}
"#;
        let (swapped_address, swapped_bytes, swapped_targets) =
            compiled_function_with_targets(swapped_result_ir, "mk");
        assert_eq!(
            check_function(
                &swapped_bytes,
                swapped_address,
                &mk_model,
                &swapped_targets,
                &context
            ),
            FunctionOutcome::Different
        );

        let pick_model = model(opt_source, "pick");
        assert!(pick_model.has_enum_signature);
        assert!(!pick_model.param_domain.is_empty());
        assert!(supports_signature(&pick_model));
        let pick_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @pick({ i64, i64 } %0) {
entry:
  %pat_tag = extractvalue { i64, i64 } %0, 0
  %pat_tag_eq = icmp eq i64 %pat_tag, 0
  br i1 %pat_tag_eq, label %some, label %other
some:
  %variant_payload_0 = extractvalue { i64, i64 } %0, 1
  br label %merge
other:
  br label %merge
merge:
  %result = phi i64 [ %variant_payload_0, %some ], [ 0, %other ]
  ret i64 %result
}
"#;
        let (pick_address, pick_bytes, pick_targets) =
            compiled_function_with_targets(pick_ir, "pick");
        assert_eq!(
            check_function(
                &pick_bytes,
                pick_address,
                &pick_model,
                &pick_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );

        let main_model = model(opt_source, "main");
        assert!(matches!(
            main_model.call_sites[0].result_shape,
            CallValueShape::Enum { leaf_count: 2 }
        ));
        assert!(matches!(
            main_model.call_sites[1].argument_shapes.as_slice(),
            [CallValueShape::Enum { leaf_count: 2 }]
        ));
        let main_ir = r#"target triple = "x86_64-unknown-linux-gnu"
declare { i64, i64 } @mk(i64)
declare i64 @get({ i64, i64 })
define i64 @__mumei_user_main() {
entry:
  %call_mk = call { i64, i64 } @mk(i64 5)
  %call_get = call i64 @get({ i64, i64 } %call_mk)
  ret i64 %call_get
}
"#;
        let (main_address, main_bytes, main_targets) =
            compiled_function_with_targets(main_ir, "__mumei_user_main");
        assert_eq!(
            check_function(
                &main_bytes,
                main_address,
                &main_model,
                &main_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );

        let bump_model = model(c_source, "bump");
        let bump_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define { i64, i64 } @bump(i64 %0) {
entry:
  %t.p = insertvalue { i64, i64 } zeroinitializer, i64 %0, 1
  ret { i64, i64 } %t.p
}
"#;
        let (bump_address, bump_bytes, bump_targets) =
            compiled_function_with_targets(bump_ir, "bump");
        assert_eq!(
            check_function(
                &bump_bytes,
                bump_address,
                &bump_model,
                &bump_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );

        let is_idle_model = model(chan_source, "is_idle");
        let is_idle_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @is_idle({ i64, i64 } %0) {
entry:
  %enum_eq_l = extractvalue { i64, i64 } %0, 0
  %enum_eq_int = icmp eq i64 %enum_eq_l, 1
  %enum_eq_and = and i1 true, %enum_eq_int
  %enum_eq_l1 = extractvalue { i64, i64 } %0, 1
  %enum_eq_int2 = icmp eq i64 %enum_eq_l1, 0
  %enum_eq_and3 = and i1 %enum_eq_and, %enum_eq_int2
  %enum_eq_ext = zext i1 %enum_eq_and3 to i64
  ret i64 %enum_eq_ext
}
"#;
        let (is_idle_address, is_idle_bytes, is_idle_targets) =
            compiled_function_with_targets(is_idle_ir, "is_idle");
        assert_eq!(
            check_function(
                &is_idle_bytes,
                is_idle_address,
                &is_idle_model,
                &is_idle_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );

        let differs_model = model(chan_source, "differs");
        let differs_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @differs({ i64, i64 } %0, { i64, i64 } %1) {
entry:
  %enum_eq_l = extractvalue { i64, i64 } %0, 0
  %enum_eq_r = extractvalue { i64, i64 } %1, 0
  %enum_eq_int = icmp eq i64 %enum_eq_l, %enum_eq_r
  %enum_eq_and = and i1 true, %enum_eq_int
  %enum_eq_l1 = extractvalue { i64, i64 } %0, 1
  %enum_eq_r2 = extractvalue { i64, i64 } %1, 1
  %enum_eq_int3 = icmp eq i64 %enum_eq_l1, %enum_eq_r2
  %enum_eq_and4 = and i1 %enum_eq_and, %enum_eq_int3
  %enum_neq_not = xor i1 %enum_eq_and4, true
  %enum_eq_ext = zext i1 %enum_neq_not to i64
  ret i64 %enum_eq_ext
}
"#;
        let (differs_address, differs_bytes, differs_targets) =
            compiled_function_with_targets(differs_ir, "differs");
        assert_eq!(
            check_function(
                &differs_bytes,
                differs_address,
                &differs_model,
                &differs_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );

        let make3_model = model(e_source, "make3");
        assert_eq!(make3_model.result_shape, Some(3));
        let make3_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define { i64, i64, i64 } @make3(i64 %0, i64 %1, i64 %2) {
entry:
  %cmp_tmp = icmp sgt i64 %2, 0
  %bool_tmp = zext i1 %cmp_tmp to i64
  %if_cond = icmp ne i64 %bool_tmp, 0
  br i1 %if_cond, label %then, label %else
then:
  %payload_0 = insertvalue { i64, i64, i64 } zeroinitializer, i64 %0, 1
  %payload_1 = insertvalue { i64, i64, i64 } %payload_0, i64 %1, 2
  br label %merge
else:
  br label %merge
merge:
  %if_result = phi { i64, i64, i64 } [ %payload_1, %then ], [ { i64 1, i64 0, i64 0 }, %else ]
  ret { i64, i64, i64 } %if_result
}
"#;
        let (make3_address, make3_bytes, make3_targets) =
            compiled_function_with_targets(make3_ir, "make3");
        assert_eq!(
            check_function(
                &make3_bytes,
                make3_address,
                &make3_model,
                &make3_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );

        let five_leaf_source = "enum Five { A(a: i64, b: i64, c: i64, d: i64), B }\n\
            atom make() -> Five requires: true; ensures: true; body: { Five::A(1, 2, 3, 4) };";
        let five_leaf_model = source_model_named_with_admission(
            five_leaf_source,
            "make",
            &context,
            RefinedAdmission::CompositeV23,
        );
        // A five-leaf all-INTEGER enum result rides the callee-side `sret`
        // machinery like a struct result, so the signature is admitted and a
        // bare `ret` fails only on the leaf obligations.
        assert_eq!(
            check_function(&[0xc3], 0, &five_leaf_model, &BTreeMap::new(), &context),
            FunctionOutcome::Different
        );

        let wide_enum_source = "enum Wide { A(x: i128), B }\n\
            atom make() -> Wide requires: true; ensures: true; body: { Wide::A(1) };";
        let wide_enum_model = source_model_named_with_admission(
            wide_enum_source,
            "make",
            &context,
            RefinedAdmission::CompositeV23,
        );
        // The i128 payload flattens into two INTEGER leaves, so the three-leaf
        // result is register-resident and admitted: a bare `ret` fails only on
        // the return obligations.
        assert_eq!(
            check_function(&[0xc3], 0, &wide_enum_model, &BTreeMap::new(), &context),
            FunctionOutcome::Different
        );
    }

    #[test]
    fn enum_call_arguments_match_codegen_past_four_leaves() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        // Seven leaves — the tag plus six `i64` payloads — overflow the six
        // argument registers, so the last leaf spills to the argument stack
        // exactly like a struct of the same width.
        let source = "enum F { A(a: i64, b: i64, c: i64, d: i64, e: i64, g: i64), B }\n\
            atom take(f: F) -> i64 requires: true; ensures: true; body: { match f { F::A(a, b, c, d, e, g) => a + g, _ => 0 } };\n\
            atom main(x: i64) -> i64 requires: true; ensures: true; body: { take(F::A(x, x, x, x, x, x)) };";
        let main_model = source_model_named_with_admission(
            source,
            "main",
            &context,
            RefinedAdmission::CompositeV23,
        );
        assert!(matches!(
            main_model.call_sites[0].argument_shapes.as_slice(),
            [CallValueShape::Enum { leaf_count: 7 }]
        ));
        let main_ir = r#"target triple = "x86_64-unknown-linux-gnu"
declare i64 @take({ i64, i64, i64, i64, i64, i64, i64 })
define i64 @__mumei_user_main(i64 %0) {
entry:
  %e = insertvalue { i64, i64, i64, i64, i64, i64, i64 } zeroinitializer, i64 %0, 1
  %e1 = insertvalue { i64, i64, i64, i64, i64, i64, i64 } %e, i64 %0, 2
  %e2 = insertvalue { i64, i64, i64, i64, i64, i64, i64 } %e1, i64 %0, 3
  %e3 = insertvalue { i64, i64, i64, i64, i64, i64, i64 } %e2, i64 %0, 4
  %e4 = insertvalue { i64, i64, i64, i64, i64, i64, i64 } %e3, i64 %0, 5
  %e5 = insertvalue { i64, i64, i64, i64, i64, i64, i64 } %e4, i64 %0, 6
  %r = call i64 @take({ i64, i64, i64, i64, i64, i64, i64 } %e5)
  ret i64 %r
}
"#;
        let (main_address, main_bytes, main_targets) =
            compiled_function_with_targets(main_ir, "__mumei_user_main");
        assert_eq!(
            check_function(
                &main_bytes,
                main_address,
                &main_model,
                &main_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );
    }

    #[test]
    fn bounded_array_red_zone_lowering_is_modeled_and_bounded() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let source = "atom sum() -> i64 requires: true; ensures: result == 13; body: { let a = [1, 2, 3]; a[1] = 9; a[0] + a[1] + a[2] };\n\
            atom pick(a0: i64, i: i64) -> i64 requires: 0 <= i && i < 3; ensures: result == 7; body: { let a = [a0, 20, 30]; a[i] = 7; a[i] };\n\
            atom get(a0: i64, a1: i64, i: i64) -> i64 requires: 0 <= i && i < 2; ensures: true; body: { let a = [a0, a1]; a[i] };";
        let model = |name| {
            source_model_named_with_admission(
                source,
                name,
                &context,
                RefinedAdmission::CompositeV11,
            )
        };
        let sum_model = model("sum");
        let pick_model = model("pick");
        let get_model = model("get");

        let sum_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @sum() {
entry:
  %a = alloca [3 x i64], align 8
  %p0 = getelementptr [3 x i64], ptr %a, i64 0, i64 0
  store i64 1, ptr %p0, align 8
  %p1 = getelementptr [3 x i64], ptr %a, i64 0, i64 1
  store i64 9, ptr %p1, align 8
  %p2 = getelementptr [3 x i64], ptr %a, i64 0, i64 2
  store i64 3, ptr %p2, align 8
  %v0 = load i64, ptr %p0, align 8
  %v1 = load i64, ptr %p1, align 8
  %v2 = load i64, ptr %p2, align 8
  %s0 = add i64 %v0, %v1
  %s1 = add i64 %s0, %v2
  ret i64 %s1
}
"#;
        let (sum_address, sum_bytes, sum_targets) = compiled_function_with_targets(sum_ir, "sum");
        assert_eq!(
            check_function(&sum_bytes, sum_address, &sum_model, &sum_targets, &context),
            FunctionOutcome::Equivalent
        );

        let pick_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @pick(i64 %a0, i64 %i) {
entry:
  %a = alloca [3 x i64], align 8
  %p0 = getelementptr [3 x i64], ptr %a, i64 0, i64 0
  store i64 %a0, ptr %p0, align 8
  %p1 = getelementptr [3 x i64], ptr %a, i64 0, i64 1
  store i64 20, ptr %p1, align 8
  %p2 = getelementptr [3 x i64], ptr %a, i64 0, i64 2
  store i64 30, ptr %p2, align 8
  %hi = icmp slt i64 %i, 3
  %lo = icmp sge i64 %i, 0
  %safe = and i1 %hi, %lo
  br i1 %safe, label %store, label %loadcheck
store:
  %pi = getelementptr [3 x i64], ptr %a, i64 0, i64 %i
  store i64 7, ptr %pi, align 8
  br label %loadcheck
loadcheck:
  %hi2 = icmp slt i64 %i, 3
  %lo2 = icmp sge i64 %i, 0
  %safe2 = and i1 %hi2, %lo2
  br i1 %safe2, label %load, label %oob
load:
  %pj = getelementptr [3 x i64], ptr %a, i64 0, i64 %i
  %v = load i64, ptr %pj, align 8
  br label %merge
oob:
  br label %merge
merge:
  %result = phi i64 [ %v, %load ], [ 0, %oob ]
  ret i64 %result
}
"#;
        let (pick_address, pick_bytes, pick_targets) =
            compiled_function_with_targets(pick_ir, "pick");
        assert_eq!(
            check_function(
                &pick_bytes,
                pick_address,
                &pick_model,
                &pick_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );
        let mut patched_pick = pick_bytes.clone();
        let store = disassemble(&pick_bytes, pick_address)
            .unwrap()
            .into_iter()
            .find(|instruction| {
                instruction.mnemonic == "movq" && instruction.operands.contains("$7")
            })
            .unwrap();
        patched_pick[store.offset as usize + 4] = 0xe0;
        assert_eq!(
            check_function(
                &patched_pick,
                pick_address,
                &pick_model,
                &pick_targets,
                &context
            ),
            FunctionOutcome::Different
        );

        let get_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @get(i64 %a0, i64 %a1, i64 %i) {
entry:
  %a = alloca [2 x i64], align 8
  %p0 = getelementptr [2 x i64], ptr %a, i64 0, i64 0
  store i64 %a0, ptr %p0, align 8
  %p1 = getelementptr [2 x i64], ptr %a, i64 0, i64 1
  store i64 %a1, ptr %p1, align 8
  %hi = icmp slt i64 %i, 2
  %lo = icmp sge i64 %i, 0
  %safe = and i1 %hi, %lo
  br i1 %safe, label %load, label %oob
load:
  %pi = getelementptr [2 x i64], ptr %a, i64 0, i64 %i
  %v = load i64, ptr %pi, align 8
  br label %merge
oob:
  br label %merge
merge:
  %result = phi i64 [ %v, %load ], [ 0, %oob ]
  ret i64 %result
}
"#;
        let (get_address, get_bytes, get_targets) = compiled_function_with_targets(get_ir, "get");
        assert_eq!(
            check_function(&get_bytes, get_address, &get_model, &get_targets, &context),
            FunctionOutcome::Equivalent
        );
        let out_of_bounds = [
            0x48, 0xc7, 0x84, 0x24, 0x78, 0xff, 0xff, 0xff, 1, 0, 0, 0, 0xc3,
        ];
        assert_eq!(
            check_function(&out_of_bounds, 0, &sum_model, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("frame_memory_bounds")
        );
        let memory_and_call = [
            0x48, 0xc7, 0x44, 0x24, 0xf8, 1, 0, 0, 0, 0xe8, 0, 0, 0, 0, 0xc3,
        ];
        assert_eq!(
            check_function(&memory_and_call, 0, &sum_model, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("frame_memory_call")
        );
        let store_push_pop_load = [
            0x48, 0xc7, 0x44, 0x24, 0xf8, 5, 0, 0, 0, 0x50, 0x58, 0x48, 0x8b, 0x44, 0x24, 0xf8,
            0xc3,
        ];
        let instructions = disassemble(&store_push_pop_load, 0).unwrap();
        assert_eq!(
            instructions
                .iter()
                .map(|instruction| instruction.mnemonic.as_str())
                .collect::<Vec<_>>(),
            ["movq", "pushq", "popq", "movq", "retq"]
        );
        assert_eq!(
            check_function(
                &store_push_pop_load,
                0,
                &sum_model,
                &BTreeMap::new(),
                &context
            ),
            FunctionOutcome::OutsideProfile("frame_memory_stack_adjust")
        );
        let non_qword_store = [0xc7, 0x44, 0x24, 0xf8, 1, 0, 0, 0, 0xc3];
        assert_eq!(
            check_function(&non_qword_store, 0, &sum_model, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );
    }

    #[test]
    fn allocated_frame_memory_matches_codegen_and_fails_closed_at_region_edges() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = |source: &str, name: &str| {
            source_model_named_with_admission(
                source,
                name,
                &context,
                RefinedAdmission::CompositeV11,
            )
        };
        let usea_source = "atom inc(x: i64) -> i64 requires: true; ensures: result == x + 1; body: { x + 1 };\n\
            atom usea(a0: i64, i: i64) -> i64 requires: 0 <= i && i < 3 && a0 >= 0 && a0 < 1000; ensures: if i == 0 { result == 2 * (a0 + 1) } else { result == 2 * a0 + 1 }; body: { let a = [a0, 20, 30]; a[i] = inc(a0); a[0] + a[i] };";
        let sum_source = "atom sum() -> i64 requires: true; ensures: result == 13; body: { let a = [1, 2, 3]; a[1] = 9; a[0] + a[1] + a[2] };";
        let get_source = "atom get(a0: i64, a1: i64, i: i64) -> i64 requires: 0 <= i && i < 2; ensures: true; body: { let a = [a0, a1]; a[i] };";
        let calls_seven_source = "atom seven_args(a0: i64, a1: i64, a2: i64, a3: i64, a4: i64, a5: i64, a6: i64) -> i64 requires: true; ensures: true; body: { a0 };\n\
            atom calls_seven(x: i64) -> i64 requires: true; ensures: true; body: { let a = [x, 2]; a[0] + seven_args(x, 2, 3, 4, 5, 6, 7) };";
        let usea_model = model(usea_source, "usea");
        let sum_model = model(sum_source, "sum");
        let get_model = model(get_source, "get");
        let calls_seven_model = model(calls_seven_source, "calls_seven");
        assert!(usea_model.bounded_memory);
        assert!(calls_seven_model.bounded_memory);
        let usea_ir = r#"target triple = "x86_64-unknown-linux-gnu"
declare i64 @inc(i64)
define i64 @usea(i64 %a0, i64 %i) {
entry:
  %a = alloca [3 x i64], align 8
  %p0 = getelementptr [3 x i64], ptr %a, i64 0, i64 0
  store i64 %a0, ptr %p0, align 8
  %p1 = getelementptr [3 x i64], ptr %a, i64 0, i64 1
  store i64 20, ptr %p1, align 8
  %p2 = getelementptr [3 x i64], ptr %a, i64 0, i64 2
  store i64 30, ptr %p2, align 8
  %call = call i64 @inc(i64 %a0)
  %hi = icmp slt i64 %i, 3
  %lo = icmp sge i64 %i, 0
  %safe = and i1 %hi, %lo
  br i1 %safe, label %store, label %loadcheck
store:
  %pi = getelementptr [3 x i64], ptr %a, i64 0, i64 %i
  store i64 %call, ptr %pi, align 8
  br label %loadcheck
loadcheck:
  %v0 = load i64, ptr %p0, align 8
  %hi2 = icmp slt i64 %i, 3
  %lo2 = icmp sge i64 %i, 0
  %safe2 = and i1 %hi2, %lo2
  br i1 %safe2, label %load, label %oob
load:
  %pj = getelementptr [3 x i64], ptr %a, i64 0, i64 %i
  %v = load i64, ptr %pj, align 8
  br label %merge
oob:
  br label %merge
merge:
  %selected = phi i64 [ %v, %load ], [ 0, %oob ]
  %result = add i64 %v0, %selected
  ret i64 %result
}
"#;
        let (usea_address, usea_bytes, usea_targets) =
            compiled_function_with_targets(usea_ir, "usea");
        let (outcome, _, frame_memory_use) = check_function_with_frame_memory(
            &usea_bytes,
            usea_address,
            &usea_model,
            &usea_targets,
            &context,
        );
        assert_eq!(outcome, FunctionOutcome::Equivalent);
        assert_eq!(
            frame_memory_use,
            FrameMemoryUse {
                red_zone: false,
                allocated: true
            }
        );

        let concrete_load = disassemble(&usea_bytes, usea_address)
            .unwrap()
            .into_iter()
            .find(|instruction| {
                instruction.mnemonic == "movq"
                    && instruction.operands.contains("(%rsp)")
                    && instruction.operands.ends_with("%rax")
            })
            .unwrap();
        let mut wrong_concrete_load = usea_bytes.clone();
        wrong_concrete_load[concrete_load.offset as usize + 4] = 0x10;
        assert_eq!(
            check_function(
                &wrong_concrete_load,
                usea_address,
                &usea_model,
                &usea_targets,
                &context
            ),
            FunctionOutcome::Different
        );

        let closed_region = [
            0x48, 0x83, 0xec, 0x10, // subq $0x10, %rsp
            0x48, 0xc7, 0x04, 0x24, 5, 0, 0, 0, // movq $5, (%rsp)
            0x48, 0x83, 0xc4, 0x10, // addq $0x10, %rsp
            0x50, 0x58, // pushq %rax; popq %rax
            0x48, 0x83, 0xec, 0x10, // subq $0x10, %rsp
            0x48, 0x8b, 0x04, 0xd4, // movq (%rsp,%rdx,8), %rax
            0x48, 0x83, 0xc4, 0x10, 0xc3,
        ];
        assert_eq!(
            check_function(&closed_region, 0, &sum_model, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("frame_memory_bounds")
        );

        let below_region = [
            0x48, 0x83, 0xec, 0x10, // subq $0x10, %rsp
            0x31, 0xd2, // xorl %edx, %edx
            0x48, 0x8b, 0x44, 0xd4, 0xe0, // movq -0x20(%rsp,%rdx,8), %rax
            0x48, 0x83, 0xc4, 0x10, 0xc3,
        ];
        assert_eq!(
            check_function(&below_region, 0, &get_model, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("frame_memory_bounds")
        );

        let call_with_array_stack_argument = [
            0x48, 0x83, 0xec, 0x10, // subq $0x10, %rsp
            0x48, 0xc7, 0x04, 0x24, 7, 0, 0, 0, // movq $7, (%rsp)
            0xbe, 2, 0, 0, 0, // movl $2, %esi
            0xba, 3, 0, 0, 0, // movl $3, %edx
            0xb9, 4, 0, 0, 0, // movl $4, %ecx
            0x41, 0xb8, 5, 0, 0, 0, // movl $5, %r8d
            0x41, 0xb9, 6, 0, 0, 0, // movl $6, %r9d
            0xe8, 0, 0, 0, 0, // callq seven_args
            0x48, 0x83, 0xc4, 0x10, // addq $0x10, %rsp
            0x48, 0x8b, 0x04, 0xfc, // movq (%rsp,%rdi,8), %rax
            0xc3,
        ];
        let call_targets = BTreeMap::from([(
            40,
            CallTarget {
                name: "seven_args".to_owned(),
                relocation_kind: CallRelocationKind::ElfX86_64Plt32,
                relocation_size: 32,
                addend: -4,
            },
        )]);
        assert_eq!(
            check_function(
                &call_with_array_stack_argument,
                0,
                &calls_seven_model,
                &call_targets,
                &context
            ),
            FunctionOutcome::OutsideProfile("call_arguments")
        );
    }

    #[test]
    fn mixed_class_struct_signatures_match_real_llvm_codegen() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let q_source = "struct Q { a: i64, b: f64 }\n\
            atom mkq(a: i64, b: f64) -> Q requires: true; ensures: true; body: { Q { a: a, b: b } };\n\
            atom getb(q: Q) -> f64 requires: true; ensures: true; body: { q.b };";
        let mkq_model = source_model_named_with_admission(
            q_source,
            "mkq",
            &context,
            RefinedAdmission::CompositeV23,
        );
        let getb_model = source_model_named_with_admission(
            q_source,
            "getb",
            &context,
            RefinedAdmission::CompositeV23,
        );
        assert_eq!(mkq_model.result_leaf_doubles, vec![false, true]);
        assert_eq!(getb_model.param_leaf_doubles, vec![vec![false, true]]);
        assert!(supports_signature(&mkq_model));
        assert!(supports_signature(&getb_model));

        let q_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define { i64, double } @mkq(i64 %a, double %b) {
entry:
  %first = insertvalue { i64, double } undef, i64 %a, 0
  %result = insertvalue { i64, double } %first, double %b, 1
  ret { i64, double } %result
}
define double @getb({ i64, double } %q) {
entry:
  %b = extractvalue { i64, double } %q, 1
  ret double %b
}
"#;
        let (mkq_address, mkq_bytes, mkq_targets) = compiled_function_with_targets(q_ir, "mkq");
        let mkq_disassembly = disassemble(&mkq_bytes, mkq_address)
            .unwrap()
            .iter()
            .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
            .collect::<Vec<_>>()
            .join("\n");
        println!("mixed {{i64, f64}} constructor disassembly:\n{mkq_disassembly}");
        assert!(
            mkq_disassembly.contains("movq %rdi, %rax"),
            "{mkq_disassembly}"
        );
        assert!(
            mkq_disassembly.trim_end().ends_with("retq"),
            "{mkq_disassembly}"
        );
        assert_eq!(
            check_function(&mkq_bytes, mkq_address, &mkq_model, &mkq_targets, &context),
            FunctionOutcome::Equivalent
        );
        let (getb_address, getb_bytes, getb_targets) = compiled_function_with_targets(q_ir, "getb");
        assert_eq!(
            check_function(
                &getb_bytes,
                getb_address,
                &getb_model,
                &getb_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );

        let r_source = "struct R { x: f64, y: f64 }\n\
            atom mkr(x: f64, y: f64) -> R requires: true; ensures: true; body: { R { x: x, y: y } };";
        let mkr_model = source_model_named_with_admission(
            r_source,
            "mkr",
            &context,
            RefinedAdmission::CompositeV23,
        );
        assert_eq!(mkr_model.result_leaf_doubles, vec![true, true]);
        assert!(supports_signature(&mkr_model));
        let r_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define { double, double } @mkr(double %x, double %y) {
entry:
  %first = insertvalue { double, double } undef, double %x, 0
  %result = insertvalue { double, double } %first, double %y, 1
  ret { double, double } %result
}
define { double, double } @mkr_swapped(double %x, double %y) {
entry:
  %first = insertvalue { double, double } undef, double %y, 0
  %result = insertvalue { double, double } %first, double %x, 1
  ret { double, double } %result
}
"#;
        let (mkr_address, mkr_bytes, mkr_targets) = compiled_function_with_targets(r_ir, "mkr");
        let mkr_disassembly = disassemble(&mkr_bytes, mkr_address)
            .unwrap()
            .iter()
            .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
            .collect::<Vec<_>>()
            .join("\n");
        println!("two-f64 struct constructor disassembly:\n{mkr_disassembly}");
        assert_eq!(mkr_disassembly.trim(), "retq");
        assert_eq!(
            check_function(&mkr_bytes, mkr_address, &mkr_model, &mkr_targets, &context),
            FunctionOutcome::Equivalent
        );
        let (swapped_address, swapped_bytes, swapped_targets) =
            compiled_function_with_targets(r_ir, "mkr_swapped");
        assert_eq!(
            check_function(
                &swapped_bytes,
                swapped_address,
                &mkr_model,
                &swapped_targets,
                &context,
            ),
            FunctionOutcome::Different
        );
    }

    #[test]
    fn mixed_class_struct_call_results_and_arguments_match_real_codegen() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let source = "struct Q { a: i64, b: f64 }\n\
            atom mkq(a: i64, b: f64) -> Q requires: true; ensures: true; body: { Q { a: a, b: b } };\n\
            atom getb(q: Q) -> f64 requires: true; ensures: true; body: { q.b };\n\
            atom use_q(x: f64) -> f64 requires: true; ensures: true; body: { getb(mkq(1, x)) };";
        let model = source_model_named_with_admission(
            source,
            "use_q",
            &context,
            RefinedAdmission::CompositeV23,
        );
        assert!(model.call_sites.iter().any(|site| matches!(
            site.result_shape,
            CallValueShape::Struct {
                leaf_count: 2,
                has_f64: true,
                ..
            }
        )));
        let mkq_call = model
            .call_sites
            .iter()
            .find(|site| matches!(site.result_shape, CallValueShape::Struct { .. }))
            .unwrap();
        assert_eq!(mkq_call.arg_leaf_doubles, vec![false, true]);
        assert_eq!(mkq_call.result_leaf_doubles, vec![false, true]);
        let getb_call = model
            .call_sites
            .iter()
            .find(|site| {
                site.argument_shapes.iter().any(|shape| {
                    matches!(
                        shape,
                        CallValueShape::Struct {
                            leaf_count: 2,
                            has_f64: true,
                            ..
                        }
                    )
                })
            })
            .unwrap();
        assert_eq!(getb_call.arg_leaf_doubles, vec![false, true]);
        assert_eq!(getb_call.result_leaf_doubles, vec![true]);
        let ir = r#"target triple = "x86_64-unknown-linux-gnu"
declare { i64, double } @mkq(i64, double)
declare double @getb({ i64, double })
define double @use_q(double %x) {
entry:
  %q = call { i64, double } @mkq(i64 1, double %x)
  %result = call double @getb({ i64, double } %q)
  ret double %result
}
"#;
        let (address, bytes, targets) = compiled_function_with_targets(ir, "use_q");
        let disassembly = disassemble(&bytes, address)
            .unwrap()
            .iter()
            .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
            .collect::<Vec<_>>()
            .join("\n");
        println!("mixed struct call disassembly:\n{disassembly}");
        assert_eq!(
            check_function(&bytes, address, &model, &targets, &context),
            FunctionOutcome::Equivalent
        );
    }

    #[test]
    fn llvm_first_class_aggregate_return_limits_match_codegen() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);

        let three_integer_source = "struct T { a: i64, b: i64, c: i64 }\n\
            atom mk3(a: i64, b: i64, c: i64) -> T requires: true; ensures: true; body: { T { a: a, b: b, c: c } };\n\
            atom sum3(p: T) -> i64 requires: true; ensures: true; body: { p.a + p.b + p.c };\n\
            atom main() -> i64 requires: true; ensures: true; body: { sum3(mk3(1, 2, 3)) };";
        let mk3_model = source_model_named_with_admission(
            three_integer_source,
            "mk3",
            &context,
            RefinedAdmission::CompositeV23,
        );
        let sum3_model = source_model_named_with_admission(
            three_integer_source,
            "sum3",
            &context,
            RefinedAdmission::CompositeV23,
        );
        let main_model = source_model_named_with_admission(
            three_integer_source,
            "main",
            &context,
            RefinedAdmission::CompositeV23,
        );
        let three_integer_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define { i64, i64, i64 } @mk3(i64 %a, i64 %b, i64 %c) {
entry:
  %p0 = insertvalue { i64, i64, i64 } undef, i64 %a, 0
  %p1 = insertvalue { i64, i64, i64 } %p0, i64 %b, 1
  %p2 = insertvalue { i64, i64, i64 } %p1, i64 %c, 2
  ret { i64, i64, i64 } %p2
}
define { i64, i64, i64 } @mk3_swapped(i64 %a, i64 %b, i64 %c) {
entry:
  %p0 = insertvalue { i64, i64, i64 } undef, i64 %a, 0
  %p1 = insertvalue { i64, i64, i64 } %p0, i64 %c, 1
  %p2 = insertvalue { i64, i64, i64 } %p1, i64 %b, 2
  ret { i64, i64, i64 } %p2
}
define i64 @sum3({ i64, i64, i64 } %p) {
entry:
  %a = extractvalue { i64, i64, i64 } %p, 0
  %b = extractvalue { i64, i64, i64 } %p, 1
  %c = extractvalue { i64, i64, i64 } %p, 2
  %ab = add i64 %a, %b
  %result = add i64 %ab, %c
  ret i64 %result
}
define i64 @main() {
entry:
  %p = call { i64, i64, i64 } @mk3(i64 1, i64 2, i64 3)
  %result = call i64 @sum3({ i64, i64, i64 } %p)
  ret i64 %result
}
"#;
        let (mk3_address, mk3_bytes, mk3_targets) =
            compiled_function_with_targets(three_integer_ir, "mk3");
        let mk3_disassembly = disassemble(&mk3_bytes, mk3_address)
            .unwrap()
            .iter()
            .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
            .collect::<Vec<_>>()
            .join("\n");
        println!("three-i64 aggregate return disassembly:\n{mk3_disassembly}");
        assert_eq!(
            check_function(&mk3_bytes, mk3_address, &mk3_model, &mk3_targets, &context),
            FunctionOutcome::Equivalent
        );
        let (swapped_address, swapped_bytes, swapped_targets) =
            compiled_function_with_targets(three_integer_ir, "mk3_swapped");
        assert_eq!(
            check_function(
                &swapped_bytes,
                swapped_address,
                &mk3_model,
                &swapped_targets,
                &context
            ),
            FunctionOutcome::Different
        );
        let (sum3_address, sum3_bytes, sum3_targets) =
            compiled_function_with_targets(three_integer_ir, "sum3");
        assert_eq!(
            check_function(
                &sum3_bytes,
                sum3_address,
                &sum3_model,
                &sum3_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );
        let (main_address, main_bytes, main_targets) =
            compiled_function_with_targets(three_integer_ir, "main");
        let main_disassembly = disassemble(&main_bytes, main_address)
            .unwrap()
            .iter()
            .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
            .collect::<Vec<_>>()
            .join("\n");
        println!("three-i64 aggregate call disassembly:\n{main_disassembly}");
        assert_eq!(
            check_function(
                &main_bytes,
                main_address,
                &main_model,
                &main_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );

        let four_integer_source = "struct I4 { a: i64, b: i64, c: i64, d: i64 }\n\
            atom mk4(a: i64, b: i64, c: i64, d: i64) -> I4 requires: true; ensures: true; body: { I4 { a: a, b: b, c: c, d: d } };\n\
            atom sum4(x: i64, y: i64, p: I4) -> i64 requires: true; ensures: true; body: { x + y + p.a + p.b + p.c + p.d };\n\
            atom main4() -> i64 requires: true; ensures: true; body: { sum4(10, 20, I4 { a: 1, b: 2, c: 3, d: 4 }) };";
        let mk4_model = source_model_named_with_admission(
            four_integer_source,
            "mk4",
            &context,
            RefinedAdmission::CompositeV23,
        );
        let sum4_model = source_model_named_with_admission(
            four_integer_source,
            "sum4",
            &context,
            RefinedAdmission::CompositeV23,
        );
        let main4_model = source_model_named_with_admission(
            four_integer_source,
            "main4",
            &context,
            RefinedAdmission::CompositeV23,
        );
        let four_integer_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define { i64, i64, i64, i64 } @mk4(i64 %a, i64 %b, i64 %c, i64 %d) {
entry:
  %p0 = insertvalue { i64, i64, i64, i64 } undef, i64 %a, 0
  %p1 = insertvalue { i64, i64, i64, i64 } %p0, i64 %b, 1
  %p2 = insertvalue { i64, i64, i64, i64 } %p1, i64 %c, 2
  %p3 = insertvalue { i64, i64, i64, i64 } %p2, i64 %d, 3
  ret { i64, i64, i64, i64 } %p3
}
define i64 @sum4(i64 %x, i64 %y, { i64, i64, i64, i64 } %p) {
entry:
  %a = extractvalue { i64, i64, i64, i64 } %p, 0
  %b = extractvalue { i64, i64, i64, i64 } %p, 1
  %c = extractvalue { i64, i64, i64, i64 } %p, 2
  %d = extractvalue { i64, i64, i64, i64 } %p, 3
  %xy = add i64 %x, %y
  %ab = add i64 %xy, %a
  %abc = add i64 %ab, %b
  %abcd = add i64 %abc, %c
  %result = add i64 %abcd, %d
  ret i64 %result
}
define i64 @main4() {
entry:
  %p0 = insertvalue { i64, i64, i64, i64 } undef, i64 1, 0
  %p1 = insertvalue { i64, i64, i64, i64 } %p0, i64 2, 1
  %p2 = insertvalue { i64, i64, i64, i64 } %p1, i64 3, 2
  %p3 = insertvalue { i64, i64, i64, i64 } %p2, i64 4, 3
  %result = call i64 @sum4(i64 10, i64 20, { i64, i64, i64, i64 } %p3)
  ret i64 %result
}
"#;
        let (mk4_address, mk4_bytes, mk4_targets) =
            compiled_function_with_targets(four_integer_ir, "mk4");
        let mk4_disassembly = disassemble(&mk4_bytes, mk4_address)
            .unwrap()
            .iter()
            .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
            .collect::<Vec<_>>()
            .join("\n");
        println!("four-i64 aggregate return disassembly:\n{mk4_disassembly}");
        // The four-`i64` result comes back through the sret pointer —
        // `rdi` carries it, the four arguments shift to `rsi`-`r8` —
        // and the recorded `movq` stores match the result leaves.
        assert_eq!(
            check_function(&mk4_bytes, mk4_address, &mk4_model, &mk4_targets, &context),
            FunctionOutcome::Equivalent
        );
        let (sum4_address, sum4_bytes, sum4_targets) =
            compiled_function_with_targets(four_integer_ir, "sum4");
        assert_eq!(
            check_function(
                &sum4_bytes,
                sum4_address,
                &sum4_model,
                &sum4_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );
        let (main4_address, main4_bytes, main4_targets) =
            compiled_function_with_targets(four_integer_ir, "main4");
        assert_eq!(
            check_function(
                &main4_bytes,
                main4_address,
                &main4_model,
                &main4_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );

        let mixed_source = "struct M { a: i64, b: f64, c: i64, d: f64 }\n\
            atom mixed(a: i64, b: f64, c: i64, d: f64) -> M requires: true; ensures: true; body: { M { a: a, b: b, c: c, d: d } };";
        let mixed_model = source_model_named_with_admission(
            mixed_source,
            "mixed",
            &context,
            RefinedAdmission::CompositeV23,
        );
        let mixed_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define { i64, double, i64, double } @mixed(i64 %a, double %b, i64 %c, double %d) {
entry:
  %p0 = insertvalue { i64, double, i64, double } undef, i64 %a, 0
  %p1 = insertvalue { i64, double, i64, double } %p0, double %b, 1
  %p2 = insertvalue { i64, double, i64, double } %p1, i64 %c, 2
  %p3 = insertvalue { i64, double, i64, double } %p2, double %d, 3
  ret { i64, double, i64, double } %p3
}
"#;
        let (mixed_address, mixed_bytes, mixed_targets) =
            compiled_function_with_targets(mixed_ir, "mixed");
        let mixed_disassembly = disassemble(&mixed_bytes, mixed_address)
            .unwrap()
            .iter()
            .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
            .collect::<Vec<_>>()
            .join("\n");
        println!("mixed four-leaf aggregate return disassembly:\n{mixed_disassembly}");
        assert_eq!(
            check_function(
                &mixed_bytes,
                mixed_address,
                &mixed_model,
                &mixed_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );

        let three_float_source = "struct F3 { a: f64, b: f64, c: f64 }\n\
            atom mkf3(a: f64, b: f64, c: f64) -> F3 requires: true; ensures: true; body: { F3 { a: a, b: b, c: c } };";
        let mkf3_model = source_model_named_with_admission(
            three_float_source,
            "mkf3",
            &context,
            RefinedAdmission::CompositeV23,
        );
        let three_float_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define { double, double, double } @mkf3(double %a, double %b, double %c) {
entry:
  %p0 = insertvalue { double, double, double } undef, double %a, 0
  %p1 = insertvalue { double, double, double } %p0, double %b, 1
  %p2 = insertvalue { double, double, double } %p1, double %c, 2
  ret { double, double, double } %p2
}
"#;
        let (mkf3_address, mkf3_bytes, mkf3_targets) =
            compiled_function_with_targets(three_float_ir, "mkf3");
        let mkf3_disassembly = disassemble(&mkf3_bytes, mkf3_address)
            .unwrap()
            .iter()
            .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
            .collect::<Vec<_>>()
            .join("\n");
        println!("three-f64 aggregate return disassembly:\n{mkf3_disassembly}");
        assert_eq!(
            check_function(
                &mkf3_bytes,
                mkf3_address,
                &mkf3_model,
                &mkf3_targets,
                &context
            ),
            FunctionOutcome::OutsideProfile("unsupported_signature")
        );
    }

    #[test]
    fn f64_only_struct_after_integer_stack_arguments_uses_xmm_registers() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let source = "struct R { x: f64, y: f64 }\n\
            atom getx(a: i64, b: i64, c: i64, d: i64, e: i64, f: i64, g: i64, r: R) -> f64 \
            requires: true; ensures: true; body: { r.x };\n\
            atom caller(a: i64, b: i64, c: i64, d: i64, e: i64, f: i64, g: i64, x: f64, y: f64) \
            -> f64 requires: true; ensures: true; body: { getx(a, b, c, d, e, f, g, R { x: x, y: y }) };";
        let getx_model = source_model_named_with_admission(
            source,
            "getx",
            &context,
            RefinedAdmission::CompositeV23,
        );
        let caller_model = source_model_named_with_admission(
            source,
            "caller",
            &context,
            RefinedAdmission::CompositeV23,
        );
        assert!(supports_signature(&getx_model));
        assert!(supports_signature(&caller_model));
        let call_site = caller_model.call_sites.first().unwrap();
        assert_eq!(
            call_site.arg_leaf_doubles,
            [vec![false; 7], vec![true, true]].concat()
        );
        assert!(call_site.argument_shapes.iter().any(|shape| matches!(
            shape,
            CallValueShape::Struct {
                leaf_count: 2,
                has_f64: true,
                ..
            }
        )));

        let getx_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define double @getx(i64 %a, i64 %b, i64 %c, i64 %d, i64 %e, i64 %f, i64 %g, { double, double } %r) {
entry:
  %x = extractvalue { double, double } %r, 0
  ret double %x
}
"#;
        let (getx_address, getx_bytes, getx_targets) =
            compiled_function_with_targets(getx_ir, "getx");
        let getx_disassembly = disassemble(&getx_bytes, getx_address)
            .unwrap()
            .iter()
            .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
            .collect::<Vec<_>>()
            .join("\n");
        println!("f64 struct after integer stack arguments disassembly:\n{getx_disassembly}");
        assert_eq!(getx_disassembly.trim(), "retq");
        assert_eq!(
            check_function(
                &getx_bytes,
                getx_address,
                &getx_model,
                &getx_targets,
                &context,
            ),
            FunctionOutcome::Equivalent
        );

        let caller_ir = r#"target triple = "x86_64-unknown-linux-gnu"
declare double @getx(i64, i64, i64, i64, i64, i64, i64, { double, double })
define double @caller(i64 %a, i64 %b, i64 %c, i64 %d, i64 %e, i64 %f, i64 %g, double %x, double %y) {
entry:
  %first = insertvalue { double, double } undef, double %x, 0
  %r = insertvalue { double, double } %first, double %y, 1
  %result = call double @getx(i64 %a, i64 %b, i64 %c, i64 %d, i64 %e, i64 %f, i64 %g, { double, double } %r)
  ret double %result
}
"#;
        let (caller_address, caller_bytes, caller_targets) =
            compiled_function_with_targets(caller_ir, "caller");
        let caller_disassembly = disassemble(&caller_bytes, caller_address)
            .unwrap()
            .iter()
            .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
            .collect::<Vec<_>>()
            .join("\n");
        println!(
            "f64 struct call after integer stack arguments disassembly:\n{caller_disassembly}"
        );
        assert_eq!(
            check_function(
                &caller_bytes,
                caller_address,
                &caller_model,
                &caller_targets,
                &context,
            ),
            FunctionOutcome::Equivalent
        );
    }

    #[test]
    fn f64_argument_beyond_xmm7_spills_to_the_argument_stack() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let source =
            "atom mul_ai(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64, g: f64, h: f64, i: f64) \
            -> f64 requires: true; ensures: true; body: { a * i };\n\
            atom sq_i(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64, g: f64, h: f64, i: f64) \
            -> f64 requires: true; ensures: true; body: { i * i };\n\
            atom ret_i(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64, g: f64, h: f64, i: f64) \
            -> f64 requires: true; ensures: true; body: { i };\n\
            atom ret_h(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64, g: f64, h: f64, i: f64) \
            -> f64 requires: true; ensures: true; body: { h };";
        let models =
            encode_pure_i64_module(source, &context, true, RefinedAdmission::CompositeV23).unwrap();
        let mul_ai = models.iter().find(|m| m.function_name == "mul_ai").unwrap();
        let sq_i = models.iter().find(|m| m.function_name == "sq_i").unwrap();
        let ret_h = models.iter().find(|m| m.function_name == "ret_h").unwrap();
        assert!(supports_signature(mul_ai));
        assert!(supports_signature(sq_i));

        // LLVM lowers the ninth `f64` argument to an SSE memory
        // operand reading the incoming argument stack directly.
        let mul_ai_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define double @mul_ai(double %a, double %b, double %c, double %d, double %e, double %f, double %g, double %h, double %i) {
entry:
  %r = fmul double %a, %i
  ret double %r
}
"#;
        let (mul_ai_address, mul_ai_bytes, mul_ai_targets) =
            compiled_function_with_targets(mul_ai_ir, "mul_ai");
        let mul_ai_disassembly = disassemble(&mul_ai_bytes, mul_ai_address)
            .unwrap()
            .iter()
            .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
            .collect::<Vec<_>>()
            .join("\n");
        println!("ninth f64 argument disassembly:\n{mul_ai_disassembly}");
        assert!(
            mul_ai_disassembly.contains("(%rsp)"),
            "expected a stack-operand SSE instruction:\n{mul_ai_disassembly}"
        );
        assert_eq!(
            check_function(
                &mul_ai_bytes,
                mul_ai_address,
                mul_ai,
                &mul_ai_targets,
                &context,
            ),
            FunctionOutcome::Equivalent
        );

        // Using the same spilled argument twice makes LLVM load it
        // through `movsd` first — the move form of the stack-argument
        // access.
        let sq_i_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define double @sq_i(double %a, double %b, double %c, double %d, double %e, double %f, double %g, double %h, double %i) {
entry:
  %r = fmul double %i, %i
  ret double %r
}
"#;
        let (sq_i_address, sq_i_bytes, sq_i_targets) =
            compiled_function_with_targets(sq_i_ir, "sq_i");
        let sq_i_disassembly = disassemble(&sq_i_bytes, sq_i_address)
            .unwrap()
            .iter()
            .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
            .collect::<Vec<_>>()
            .join("\n");
        println!("spilled f64 argument used twice disassembly:\n{sq_i_disassembly}");
        assert!(
            sq_i_disassembly.contains("movsd"),
            "expected a movsd stack load:\n{sq_i_disassembly}"
        );
        assert_eq!(
            check_function(&sq_i_bytes, sq_i_address, sq_i, &sq_i_targets, &context,),
            FunctionOutcome::Equivalent
        );

        // A `movsd` stack load bound to a model that returns a
        // register argument reads the wrong leaf — the stack slot is
        // not interchangeable with a register argument.
        let ret_i_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define double @ret_i(double %a, double %b, double %c, double %d, double %e, double %f, double %g, double %h, double %i) {
entry:
  ret double %i
}
"#;
        let (ret_i_address, ret_i_bytes, ret_i_targets) =
            compiled_function_with_targets(ret_i_ir, "ret_i");
        let ret_i_disassembly = disassemble(&ret_i_bytes, ret_i_address)
            .unwrap()
            .iter()
            .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
            .collect::<Vec<_>>()
            .join("\n");
        println!("return spilled f64 argument disassembly:\n{ret_i_disassembly}");
        assert!(
            ret_i_disassembly.contains("(%rsp)"),
            "expected a stack-operand load:\n{ret_i_disassembly}"
        );
        assert_eq!(
            check_function(&ret_i_bytes, ret_i_address, ret_h, &ret_i_targets, &context,),
            FunctionOutcome::Different
        );
    }

    #[test]
    fn mixed_class_stack_arguments_follow_declaration_order() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);

        // Seven `i64` then nine `f64`: the seventh integer overflows
        // into stack slot 0 and the ninth `f64` into slot 1.
        let source_a = "atom ret_g_a(a: i64, b: i64, c: i64, d: i64, e: i64, f: i64, g: i64, \
            h: f64, i: f64, j: f64, k: f64, l: f64, m: f64, n: f64, o: f64, p: f64) \
            -> i64 requires: true; ensures: true; body: { g };\n\
            atom ret_p_a(a: i64, b: i64, c: i64, d: i64, e: i64, f: i64, g: i64, \
            h: f64, i: f64, j: f64, k: f64, l: f64, m: f64, n: f64, o: f64, p: f64) \
            -> f64 requires: true; ensures: true; body: { p };";
        let models_a =
            encode_pure_i64_module(source_a, &context, true, RefinedAdmission::CompositeV23)
                .unwrap();
        let ret_g_a = models_a
            .iter()
            .find(|m| m.function_name == "ret_g_a")
            .unwrap();
        let ret_p_a = models_a
            .iter()
            .find(|m| m.function_name == "ret_p_a")
            .unwrap();
        assert!(supports_signature(ret_g_a));
        assert!(supports_signature(ret_p_a));

        // Nine `f64` then seven `i64` — the reversed overflow order:
        // the ninth `f64` takes slot 0 and the seventh integer slot 1.
        let source_b = "atom ret_i_b(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64, g: f64, \
            h: f64, i: f64, j: i64, k: i64, l: i64, m: i64, n: i64, o: i64, p: i64) \
            -> f64 requires: true; ensures: true; body: { i };\n\
            atom ret_p_b(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64, g: f64, \
            h: f64, i: f64, j: i64, k: i64, l: i64, m: i64, n: i64, o: i64, p: i64) \
            -> i64 requires: true; ensures: true; body: { p };";
        let models_b =
            encode_pure_i64_module(source_b, &context, true, RefinedAdmission::CompositeV23)
                .unwrap();
        let ret_i_b = models_b
            .iter()
            .find(|m| m.function_name == "ret_i_b")
            .unwrap();
        let ret_p_b = models_b
            .iter()
            .find(|m| m.function_name == "ret_p_b")
            .unwrap();
        assert!(supports_signature(ret_i_b));
        assert!(supports_signature(ret_p_b));

        let check = |ir: &str, name: &str, model: &PureI64Body<'_>| {
            let (address, bytes, targets) = compiled_function_with_targets(ir, name);
            let disassembly = disassemble(&bytes, address)
                .unwrap()
                .iter()
                .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
                .collect::<Vec<_>>()
                .join("\n");
            println!("{name} disassembly:\n{disassembly}");
            assert!(
                disassembly.contains("(%rsp)"),
                "expected a stack-operand access:\n{disassembly}"
            );
            check_function(&bytes, address, model, &targets, &context)
        };

        // Both overflow orders verify against their own models —
        // slot order follows declaration order, not per-class order.
        assert_eq!(
            check(
                r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @ret_g_a(i64 %a, i64 %b, i64 %c, i64 %d, i64 %e, i64 %f, i64 %g, double %h, double %i, double %j, double %k, double %l, double %m, double %n, double %o, double %p) {
entry:
  ret i64 %g
}
"#,
                "ret_g_a",
                ret_g_a,
            ),
            FunctionOutcome::Equivalent
        );
        assert_eq!(
            check(
                r#"target triple = "x86_64-unknown-linux-gnu"
define double @ret_p_a(i64 %a, i64 %b, i64 %c, i64 %d, i64 %e, i64 %f, i64 %g, double %h, double %i, double %j, double %k, double %l, double %m, double %n, double %o, double %p) {
entry:
  ret double %p
}
"#,
                "ret_p_a",
                ret_p_a,
            ),
            FunctionOutcome::Equivalent
        );
        assert_eq!(
            check(
                r#"target triple = "x86_64-unknown-linux-gnu"
define double @ret_i_b(double %a, double %b, double %c, double %d, double %e, double %f, double %g, double %h, double %i, i64 %j, i64 %k, i64 %l, i64 %m, i64 %n, i64 %o, i64 %p) {
entry:
  ret double %i
}
"#,
                "ret_i_b",
                ret_i_b,
            ),
            FunctionOutcome::Equivalent
        );
        assert_eq!(
            check(
                r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @ret_p_b(double %a, double %b, double %c, double %d, double %e, double %f, double %g, double %h, double %i, i64 %j, i64 %k, i64 %l, i64 %m, i64 %n, i64 %o, i64 %p) {
entry:
  ret i64 %p
}
"#,
                "ret_p_b",
                ret_p_b,
            ),
            FunctionOutcome::Equivalent
        );

        // Cross-class slot reads fail closed: the INTEGER `movq` load
        // of slot 0 is checked against the order-B model whose slot 0
        // holds the spilled `f64` leaf `i` — class mismatch, not a
        // bound to a wrong input.
        assert_eq!(
            check(
                r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @ret_g_a(i64 %a, i64 %b, i64 %c, i64 %d, i64 %e, i64 %f, i64 %g, double %h, double %i, double %j, double %k, double %l, double %m, double %n, double %o, double %p) {
entry:
  ret i64 %g
}
"#,
                "ret_g_a",
                ret_i_b,
            ),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );
        // And an SSE `movsd` load of slot 0 against the order-A model
        // whose slot 0 holds the spilled integer `g`.
        assert_eq!(
            check(
                r#"target triple = "x86_64-unknown-linux-gnu"
define double @ret_i_b(double %a, double %b, double %c, double %d, double %e, double %f, double %g, double %h, double %i, i64 %j, i64 %k, i64 %l, i64 %m, i64 %n, i64 %o, i64 %p) {
entry:
  ret double %i
}
"#,
                "ret_i_b",
                ret_g_a,
            ),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );
    }

    #[test]
    fn f64_call_argument_beyond_xmm7_spills_to_the_argument_stack() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let source = "atom callee9(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64, g: f64, \
            h: f64, i: f64) -> f64 requires: true; ensures: true; body: { i };\n\
            atom caller9(x: f64) -> f64 requires: true; ensures: true; \
            body: { callee9(x, x, x, x, x, x, x, x, x) };\n\
            atom callee16(a: i64, b: i64, c: i64, d: i64, e: i64, f: i64, g: i64, \
            h: f64, i: f64, j: f64, k: f64, l: f64, m: f64, n: f64, o: f64, p: f64) \
            -> f64 requires: true; ensures: true; body: { p };\n\
            atom caller16(x: f64, y: i64) -> f64 requires: true; ensures: true; \
            body: { callee16(y, y, y, y, y, y, y, x, x, x, x, x, x, x, x, x) };";
        let models =
            encode_pure_i64_module(source, &context, true, RefinedAdmission::CompositeV23).unwrap();
        let caller9 = models
            .iter()
            .find(|m| m.function_name == "caller9")
            .unwrap();
        let caller16 = models
            .iter()
            .find(|m| m.function_name == "caller16")
            .unwrap();
        assert!(supports_signature(caller9));
        assert!(supports_signature(caller16));

        // LLVM writes the ninth `f64` call argument to the outgoing
        // argument stack with `movsd %xmm, N(%rsp)`.
        let caller9_ir = r#"target triple = "x86_64-unknown-linux-gnu"
declare double @callee9(double, double, double, double, double, double, double, double, double)
define double @caller9(double %x) {
entry:
  %r = call double @callee9(double %x, double %x, double %x, double %x, double %x, double %x, double %x, double %x, double %x)
  ret double %r
}
"#;
        let (caller9_address, caller9_bytes, caller9_targets) =
            compiled_function_with_targets(caller9_ir, "caller9");
        let caller9_disassembly = disassemble(&caller9_bytes, caller9_address)
            .unwrap()
            .iter()
            .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
            .collect::<Vec<_>>()
            .join("\n");
        println!("caller9 disassembly:\n{caller9_disassembly}");
        assert!(
            caller9_disassembly.contains("movsd"),
            "expected a movsd outgoing stack store:\n{caller9_disassembly}"
        );
        assert_eq!(
            check_function(
                &caller9_bytes,
                caller9_address,
                caller9,
                &caller9_targets,
                &context,
            ),
            FunctionOutcome::Equivalent
        );

        // A mixed call puts the seventh integer and the ninth `f64`
        // into adjacent stack slots in declaration order — an integer
        // `mov` store followed by the `movsd` store.
        let caller16_ir = r#"target triple = "x86_64-unknown-linux-gnu"
declare double @callee16(i64, i64, i64, i64, i64, i64, i64, double, double, double, double, double, double, double, double, double)
define double @caller16(double %x, i64 %y) {
entry:
  %r = call double @callee16(i64 %y, i64 %y, i64 %y, i64 %y, i64 %y, i64 %y, i64 %y, double %x, double %x, double %x, double %x, double %x, double %x, double %x, double %x, double %x)
  ret double %r
}
"#;
        let (caller16_address, caller16_bytes, caller16_targets) =
            compiled_function_with_targets(caller16_ir, "caller16");
        let caller16_disassembly = disassemble(&caller16_bytes, caller16_address)
            .unwrap()
            .iter()
            .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
            .collect::<Vec<_>>()
            .join("\n");
        println!("caller16 disassembly:\n{caller16_disassembly}");
        assert!(
            caller16_disassembly.contains("movsd"),
            "expected a movsd outgoing stack store:\n{caller16_disassembly}"
        );
        assert_eq!(
            check_function(
                &caller16_bytes,
                caller16_address,
                caller16,
                &caller16_targets,
                &context,
            ),
            FunctionOutcome::Equivalent
        );

        // The same bytes checked against a model that expects the
        // spilled `f64` argument to differ are refuted, not merely
        // unmatched.
        let wrong = "atom caller9(x: f64) -> f64 requires: true; ensures: true; \
            body: { callee9(x, x, x, x, x, x, x, x, x + x) };\n\
            atom callee9(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64, g: f64, \
            h: f64, i: f64) -> f64 requires: true; ensures: true; body: { i };";
        let wrong_models =
            encode_pure_i64_module(wrong, &context, true, RefinedAdmission::CompositeV23).unwrap();
        let wrong_caller = wrong_models
            .iter()
            .find(|m| m.function_name == "caller9")
            .unwrap();
        assert_eq!(
            check_function(
                &caller9_bytes,
                caller9_address,
                wrong_caller,
                &caller9_targets,
                &context,
            ),
            FunctionOutcome::Different
        );
    }

    #[test]
    fn struct_signature_limits_fail_closed() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let one_leaf_source = "struct One { x: i64 }\n\
            atom identity(value: One) -> One requires: true; ensures: true; body: { value };";
        let one_leaf = source_model_named_with_admission(
            one_leaf_source,
            "identity",
            &context,
            RefinedAdmission::CompositeV23,
        );
        assert_eq!(one_leaf.param_shapes, vec![Some(1)]);
        assert_eq!(one_leaf.result_shape, Some(1));
        assert!(supports_signature(&one_leaf));

        let three_leaf_source = "struct T { x: i64, y: i64, z: i64 }\n\
            atom three(p: T) -> i64 requires: true; ensures: true; body: { p.x + p.y + p.z };";
        let three_leaf = source_model_named_with_admission(
            three_leaf_source,
            "three",
            &context,
            RefinedAdmission::CompositeV23,
        );
        assert!(supports_signature(&three_leaf));
        assert_eq!(
            check_function(&[0xc3], 0, &three_leaf, &BTreeMap::new(), &context),
            FunctionOutcome::Different
        );

        // A five-leaf INTEGER result is memory-class and admitted
        // under the `sret` convention: a bare `ret` writes no buffer
        // leaves and does not hand the pointer back in `rax`, so it
        // differs from `identity`.
        let five_leaf_source = "struct Five { a: i64, b: i64, c: i64, d: i64, e: i64 }\n\
            atom identity(value: Five) -> Five requires: true; ensures: true; body: { value };";
        let five_leaf = source_model_named_with_admission(
            five_leaf_source,
            "identity",
            &context,
            RefinedAdmission::CompositeV23,
        );
        assert!(supports_signature(&five_leaf));
        assert_eq!(
            check_function(&[0xc3], 0, &five_leaf, &BTreeMap::new(), &context),
            FunctionOutcome::Different
        );

        let f64_struct_source = "struct F { value: f64 }\n\
            atom project(value: F) -> f64 requires: true; ensures: true; body: { value.value };";
        let f64_struct = source_model_named_with_admission(
            f64_struct_source,
            "project",
            &context,
            RefinedAdmission::CompositeV23,
        );
        assert!(supports_signature(&f64_struct));
        assert_eq!(
            check_function(&[0xc3], 0, &f64_struct, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );

        let f64_struct_after_eight_source = "struct F { value: f64 }\n\
            atom after_eight(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64, g: f64, h: f64, p: F) \
            -> f64 requires: true; ensures: true; body: { p.value };";
        let f64_struct_after_eight = source_model_named_with_admission(
            f64_struct_after_eight_source,
            "after_eight",
            &context,
            RefinedAdmission::CompositeV23,
        );
        // The aggregate's ninth `f64` leaf spills to the argument
        // stack the same way a scalar one does — `retq` alone returns
        // the first argument, not the spilled field.
        assert_eq!(
            check_function(
                &[0xc3],
                0,
                &f64_struct_after_eight,
                &BTreeMap::new(),
                &context,
            ),
            FunctionOutcome::Different
        );
        let after_eight_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define double @after_eight(double %a, double %b, double %c, double %d, double %e, double %f, double %g, double %h, double %p) {
entry:
  ret double %p
}
"#;
        let (after_eight_address, after_eight_bytes, after_eight_targets) =
            compiled_function_with_targets(after_eight_ir, "after_eight");
        assert_eq!(
            check_function(
                &after_eight_bytes,
                after_eight_address,
                &f64_struct_after_eight,
                &after_eight_targets,
                &context,
            ),
            FunctionOutcome::Equivalent
        );

        let after_five_source = "struct P { x: i64, y: i64 }\n\
            atom after_five(a: i64, b: i64, c: i64, d: i64, e: i64, p: P) -> i64 \
            requires: true; ensures: true; body: { p.x + p.y };";
        let after_five = source_model_named_with_admission(
            after_five_source,
            "after_five",
            &context,
            RefinedAdmission::CompositeV23,
        );
        // p.y spills to the argument stack like a scalar argument, so the
        // signature stays inside the profile.
        assert_eq!(
            check_function(&[0xc3], 0, &after_five, &BTreeMap::new(), &context),
            FunctionOutcome::Different
        );
    }

    #[test]
    fn movsd_store_into_allocated_frame_memory_fails_closed() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let source = "atom idf(x: f64) -> f64 requires: true; ensures: true; body: { x };";
        let mut model = source_model_named_with_admission(
            source,
            "idf",
            &context,
            RefinedAdmission::CompositeV23,
        );
        model.bounded_memory = true;
        // subq $0x10,%rsp; movsd %xmm0,0x8(%rsp); movq (%rsp,%rcx,8),%rax;
        // retq — the SSE scalar store lands inside the allocated frame
        // region, which has no modeled path (the integer load reads
        // `frame_memory`, not the outgoing-argument `stack`), so the
        // function must fail closed rather than silently drop the write.
        let bytes = [
            0x48, 0x83, 0xec, 0x10, 0xf2, 0x0f, 0x11, 0x44, 0x24, 0x08, 0x48, 0x8b, 0x04, 0xcc,
            0xc3,
        ];
        assert_eq!(
            check_function(&bytes, 0, &model, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("frame_memory_bounds")
        );
    }

    #[test]
    fn wide_struct_parameters_and_call_arguments_match_codegen() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let source = "struct P5 { a: i64, b: i64, c: i64, d: i64, e: i64 }\n\
            atom sum5(p: P5) -> i64 requires: true; ensures: true; body: { p.a + p.b + p.c + p.d + p.e };\n\
            atom main5() -> i64 requires: true; ensures: true; body: { sum5(P5 { a: 1, b: 2, c: 3, d: 4, e: 5 }) };";
        let sum5_model = source_model_named_with_admission(
            source,
            "sum5",
            &context,
            RefinedAdmission::CompositeV23,
        );
        let main5_model = source_model_named_with_admission(
            source,
            "main5",
            &context,
            RefinedAdmission::CompositeV23,
        );
        assert_eq!(sum5_model.param_shapes, vec![Some(5)]);
        assert!(supports_signature(&sum5_model));
        let five_integer_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @sum5({ i64, i64, i64, i64, i64 } %p) {
entry:
  %a = extractvalue { i64, i64, i64, i64, i64 } %p, 0
  %b = extractvalue { i64, i64, i64, i64, i64 } %p, 1
  %c = extractvalue { i64, i64, i64, i64, i64 } %p, 2
  %d = extractvalue { i64, i64, i64, i64, i64 } %p, 3
  %e = extractvalue { i64, i64, i64, i64, i64 } %p, 4
  %ab = add i64 %a, %b
  %abc = add i64 %ab, %c
  %abcd = add i64 %abc, %d
  %result = add i64 %abcd, %e
  ret i64 %result
}
define i64 @sum5_swapped({ i64, i64, i64, i64, i64 } %p) {
entry:
  %a = extractvalue { i64, i64, i64, i64, i64 } %p, 0
  %b = extractvalue { i64, i64, i64, i64, i64 } %p, 1
  %c = extractvalue { i64, i64, i64, i64, i64 } %p, 2
  %d = extractvalue { i64, i64, i64, i64, i64 } %p, 3
  %e = extractvalue { i64, i64, i64, i64, i64 } %p, 4
  %ab = add i64 %a, %e
  %abc = add i64 %ab, %c
  %abcd = add i64 %abc, %d
  %result = add i64 %abcd, %e
  ret i64 %result
}
define i64 @main5() {
entry:
  %p0 = insertvalue { i64, i64, i64, i64, i64 } undef, i64 1, 0
  %p1 = insertvalue { i64, i64, i64, i64, i64 } %p0, i64 2, 1
  %p2 = insertvalue { i64, i64, i64, i64, i64 } %p1, i64 3, 2
  %p3 = insertvalue { i64, i64, i64, i64, i64 } %p2, i64 4, 3
  %p4 = insertvalue { i64, i64, i64, i64, i64 } %p3, i64 5, 4
  %result = call i64 @sum5({ i64, i64, i64, i64, i64 } %p4)
  ret i64 %result
}
"#;
        let (sum5_address, sum5_bytes, sum5_targets) =
            compiled_function_with_targets(five_integer_ir, "sum5");
        let sum5_disassembly = disassemble(&sum5_bytes, sum5_address)
            .unwrap()
            .iter()
            .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
            .collect::<Vec<_>>()
            .join("\n");
        println!("five-i64 aggregate parameter disassembly:\n{sum5_disassembly}");
        assert_eq!(
            check_function(
                &sum5_bytes,
                sum5_address,
                &sum5_model,
                &sum5_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );
        let (swapped_address, swapped_bytes, swapped_targets) =
            compiled_function_with_targets(five_integer_ir, "sum5_swapped");
        assert_eq!(
            check_function(
                &swapped_bytes,
                swapped_address,
                &sum5_model,
                &swapped_targets,
                &context
            ),
            FunctionOutcome::Different
        );
        let (main5_address, main5_bytes, main5_targets) =
            compiled_function_with_targets(five_integer_ir, "main5");
        let main5_disassembly = disassemble(&main5_bytes, main5_address)
            .unwrap()
            .iter()
            .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
            .collect::<Vec<_>>()
            .join("\n");
        println!("five-i64 aggregate call disassembly:\n{main5_disassembly}");
        assert_eq!(
            check_function(
                &main5_bytes,
                main5_address,
                &main5_model,
                &main5_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );

        let six_f64_source = "struct F6 { a: f64, b: f64, c: f64, d: f64, e: f64, f: f64 }\n\
            atom sumf6(p: F6) -> f64 requires: true; ensures: true; body: { p.a + p.b + p.c + p.d + p.e + p.f };\n\
            atom lastf6(p: F6) -> f64 requires: true; ensures: true; body: { p.f };";
        let sumf6_model = source_model_named_with_admission(
            six_f64_source,
            "sumf6",
            &context,
            RefinedAdmission::CompositeV23,
        );
        // The six-leaf fadd chain sits at the solver budget edge and returns
        // Unknown (fail-closed), so the gate admission is asserted only.
        assert!(supports_signature(&sumf6_model));
        let lastf6_model = source_model_named_with_admission(
            six_f64_source,
            "lastf6",
            &context,
            RefinedAdmission::CompositeV23,
        );
        let six_f64_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define double @lastf6({ double, double, double, double, double, double } %p) {
entry:
  %f = extractvalue { double, double, double, double, double, double } %p, 5
  ret double %f
}
"#;
        let (lastf6_address, lastf6_bytes, lastf6_targets) =
            compiled_function_with_targets(six_f64_ir, "lastf6");
        let lastf6_disassembly = disassemble(&lastf6_bytes, lastf6_address)
            .unwrap()
            .iter()
            .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
            .collect::<Vec<_>>()
            .join("\n");
        println!("six-f64 aggregate tail-leaf disassembly:\n{lastf6_disassembly}");
        assert_eq!(
            check_function(
                &lastf6_bytes,
                lastf6_address,
                &lastf6_model,
                &lastf6_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );

        let mixed_eight_source =
            "struct M8 { a: i64, x: f64, b: i64, y: f64, c: i64, z: f64, d: i64, w: f64 }\n\
            atom lasti8(p: M8) -> i64 requires: true; ensures: true; body: { p.d };\n\
            atom lastf8(p: M8) -> f64 requires: true; ensures: true; body: { p.w };\n\
            atom fwd8(p: M8) -> i64 requires: true; ensures: true; body: { lasti8(p) };";
        let lasti8_model = source_model_named_with_admission(
            mixed_eight_source,
            "lasti8",
            &context,
            RefinedAdmission::CompositeV23,
        );
        let lastf8_model = source_model_named_with_admission(
            mixed_eight_source,
            "lastf8",
            &context,
            RefinedAdmission::CompositeV23,
        );
        let fwd8_model = source_model_named_with_admission(
            mixed_eight_source,
            "fwd8",
            &context,
            RefinedAdmission::CompositeV23,
        );
        let mixed_eight_ir = r#"target triple = "x86_64-unknown-linux-gnu"
declare i64 @lasti8({ i64, double, i64, double, i64, double, i64, double })
define i64 @lasti8_body({ i64, double, i64, double, i64, double, i64, double } %p) {
entry:
  %d = extractvalue { i64, double, i64, double, i64, double, i64, double } %p, 6
  ret i64 %d
}
define double @lastf8({ i64, double, i64, double, i64, double, i64, double } %p) {
entry:
  %w = extractvalue { i64, double, i64, double, i64, double, i64, double } %p, 7
  ret double %w
}
define i64 @fwd8({ i64, double, i64, double, i64, double, i64, double } %p) {
entry:
  %r = call i64 @lasti8({ i64, double, i64, double, i64, double, i64, double } %p)
  ret i64 %r
}
"#;
        for (name, model) in [
            ("lasti8_body", &lasti8_model),
            ("lastf8", &lastf8_model),
            ("fwd8", &fwd8_model),
        ] {
            let (address, bytes, targets) = compiled_function_with_targets(mixed_eight_ir, name);
            let disassembly = disassemble(&bytes, address)
                .unwrap()
                .iter()
                .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
                .collect::<Vec<_>>()
                .join("\n");
            println!("mixed eight-leaf {name} disassembly:\n{disassembly}");
            assert_eq!(
                check_function(&bytes, address, model, &targets, &context),
                FunctionOutcome::Equivalent,
                "{name}"
            );
        }

        let seven_integer_source =
            "struct P7 { a: i64, b: i64, c: i64, d: i64, e: i64, f: i64, g: i64 }\n\
            atom sum7(p: P7) -> i64 requires: true; ensures: true; body: { p.a + p.b };";
        let sum7_model = source_model_named_with_admission(
            seven_integer_source,
            "sum7",
            &context,
            RefinedAdmission::CompositeV23,
        );
        // INTEGER leaves beyond rdi-r9 spill to the argument stack like
        // scalars, so a seven-i64-leaf struct still fits the profile.
        assert!(supports_signature(&sum7_model));
        assert_eq!(
            check_function(&[0xc3], 0, &sum7_model, &BTreeMap::new(), &context),
            FunctionOutcome::Different
        );

        let eight_integer_source = "struct P8 { a: i64, b: i64, c: i64, d: i64, e: i64, f: i64, g: i64, h: i64 }\n\
            atom sum8(p: P8) -> i64 requires: true; ensures: true; body: { p.a + p.b + p.c + p.d + p.e + p.f + p.g + p.h };\n\
            atom last8(p: P8) -> i64 requires: true; ensures: true; body: { p.h };\n\
            atom main8() -> i64 requires: true; ensures: true; body: { sum8(P8 { a: 1, b: 2, c: 3, d: 4, e: 5, f: 6, g: 7, h: 8 }) };";
        let eight_models = |name| {
            source_model_named_with_admission(
                eight_integer_source,
                name,
                &context,
                RefinedAdmission::CompositeV23,
            )
        };
        let eight_integer_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @sum8({ i64, i64, i64, i64, i64, i64, i64, i64 } %p) {
entry:
  %a = extractvalue { i64, i64, i64, i64, i64, i64, i64, i64 } %p, 0
  %b = extractvalue { i64, i64, i64, i64, i64, i64, i64, i64 } %p, 1
  %c = extractvalue { i64, i64, i64, i64, i64, i64, i64, i64 } %p, 2
  %d = extractvalue { i64, i64, i64, i64, i64, i64, i64, i64 } %p, 3
  %e = extractvalue { i64, i64, i64, i64, i64, i64, i64, i64 } %p, 4
  %f = extractvalue { i64, i64, i64, i64, i64, i64, i64, i64 } %p, 5
  %g = extractvalue { i64, i64, i64, i64, i64, i64, i64, i64 } %p, 6
  %h = extractvalue { i64, i64, i64, i64, i64, i64, i64, i64 } %p, 7
  %ab = add i64 %a, %b
  %abc = add i64 %ab, %c
  %abcd = add i64 %abc, %d
  %abcde = add i64 %abcd, %e
  %abcdef = add i64 %abcde, %f
  %abcdefg = add i64 %abcdef, %g
  %result = add i64 %abcdefg, %h
  ret i64 %result
}
define i64 @last8({ i64, i64, i64, i64, i64, i64, i64, i64 } %p) {
entry:
  %h = extractvalue { i64, i64, i64, i64, i64, i64, i64, i64 } %p, 7
  ret i64 %h
}
define i64 @main8() {
entry:
  %p0 = insertvalue { i64, i64, i64, i64, i64, i64, i64, i64 } undef, i64 1, 0
  %p1 = insertvalue { i64, i64, i64, i64, i64, i64, i64, i64 } %p0, i64 2, 1
  %p2 = insertvalue { i64, i64, i64, i64, i64, i64, i64, i64 } %p1, i64 3, 2
  %p3 = insertvalue { i64, i64, i64, i64, i64, i64, i64, i64 } %p2, i64 4, 3
  %p4 = insertvalue { i64, i64, i64, i64, i64, i64, i64, i64 } %p3, i64 5, 4
  %p5 = insertvalue { i64, i64, i64, i64, i64, i64, i64, i64 } %p4, i64 6, 5
  %p6 = insertvalue { i64, i64, i64, i64, i64, i64, i64, i64 } %p5, i64 7, 6
  %p7 = insertvalue { i64, i64, i64, i64, i64, i64, i64, i64 } %p6, i64 8, 7
  %result = call i64 @sum8({ i64, i64, i64, i64, i64, i64, i64, i64 } %p7)
  ret i64 %result
}
"#;
        for name in ["sum8", "last8", "main8"] {
            let model = eight_models(name);
            let (address, bytes, targets) = compiled_function_with_targets(eight_integer_ir, name);
            let disassembly = disassemble(&bytes, address)
                .unwrap()
                .iter()
                .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
                .collect::<Vec<_>>()
                .join("\n");
            println!("eight-i64 aggregate {name} disassembly:\n{disassembly}");
            assert_eq!(
                check_function(&bytes, address, &model, &targets, &context),
                FunctionOutcome::Equivalent,
                "{name}"
            );
        }

        let nine_leaf_source = "struct P9 { a: i64, b: i64, c: i64, d: i64, e: i64, f: i64, g: i64, h: i64, i: i64 }\n\
            atom sum9(p: P9) -> i64 requires: true; ensures: true; body: { p.a + p.i };";
        let sum9_model = source_model_named_with_admission(
            nine_leaf_source,
            "sum9",
            &context,
            RefinedAdmission::CompositeV23,
        );
        // Nine INTEGER leaves fit inside the sixteen-leaf bound now that
        // spilled leaves share the scalar stack-argument path.
        assert!(supports_signature(&sum9_model));
        assert_eq!(
            check_function(&[0xc3], 0, &sum9_model, &BTreeMap::new(), &context),
            FunctionOutcome::Different
        );

        let sixteen_leaf_source = "struct P16 { a: i64, b: i64, c: i64, d: i64, e: i64, f: i64, g: i64, h: i64, i: i64, j: i64, k: i64, l: i64, m: i64, n: i64, o: i64, p: i64 }\n\
            atom first_last(p: P16) -> i64 requires: true; ensures: true; body: { p.a + p.p };\n\
            atom main16() -> i64 requires: true; ensures: true; body: { first_last(P16 { a: 1, b: 2, c: 3, d: 4, e: 5, f: 6, g: 7, h: 8, i: 9, j: 10, k: 11, l: 12, m: 13, n: 14, o: 15, p: 16 }) };";
        let sixteen_models = |name| {
            source_model_named_with_admission(
                sixteen_leaf_source,
                name,
                &context,
                RefinedAdmission::CompositeV23,
            )
        };
        let sixteen_leaf_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @first_last({ i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64 } %p) {
entry:
  %a = extractvalue { i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64 } %p, 0
  %q = extractvalue { i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64 } %p, 15
  %r = add i64 %a, %q
  ret i64 %r
}
define i64 @main16() {
entry:
  %p0 = insertvalue { i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64 } undef, i64 1, 0
  %p1 = insertvalue { i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64 } %p0, i64 2, 1
  %p2 = insertvalue { i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64 } %p1, i64 3, 2
  %p3 = insertvalue { i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64 } %p2, i64 4, 3
  %p4 = insertvalue { i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64 } %p3, i64 5, 4
  %p5 = insertvalue { i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64 } %p4, i64 6, 5
  %p6 = insertvalue { i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64 } %p5, i64 7, 6
  %p7 = insertvalue { i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64 } %p6, i64 8, 7
  %p8 = insertvalue { i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64 } %p7, i64 9, 8
  %p9 = insertvalue { i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64 } %p8, i64 10, 9
  %p10 = insertvalue { i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64 } %p9, i64 11, 10
  %p11 = insertvalue { i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64 } %p10, i64 12, 11
  %p12 = insertvalue { i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64 } %p11, i64 13, 12
  %p13 = insertvalue { i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64 } %p12, i64 14, 13
  %p14 = insertvalue { i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64 } %p13, i64 15, 14
  %p15 = insertvalue { i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64 } %p14, i64 16, 15
  %r = call i64 @first_last({ i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64 } %p15)
  ret i64 %r
}
"#;
        for name in ["first_last", "main16"] {
            let model = sixteen_models(name);
            assert!(supports_signature(&model), "{name}");
            let (address, bytes, targets) = compiled_function_with_targets(sixteen_leaf_ir, name);
            assert_eq!(
                check_function(&bytes, address, &model, &targets, &context),
                FunctionOutcome::Equivalent,
                "{name}"
            );
        }

        // Past the bound — and past the admission fragment's own 16-leaf
        // flattening limit — aggregates stay fail-closed.
        assert!(!call_value_shape_supported_with_leaf_bound(
            CallValueShape::Struct {
                leaf_count: MAX_AGGREGATE_ARGUMENT_LEAF_COUNT + 1,
                has_enum: false,
                has_f64: false,
            },
            MAX_AGGREGATE_ARGUMENT_LEAF_COUNT,
        ));
    }

    #[test]
    fn struct_valued_direct_calls_match_arguments_and_results() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let caller_source = "struct P { x: i64, y: i64 }\n\
            atom sum(p: P) -> i64 requires: true; ensures: true; body: { p.x + p.y };\n\
            atom mk(a: i64, b: i64) -> P requires: true; ensures: true; body: { P { x: a, y: b } };\n\
            atom main() -> i64 requires: true; ensures: true; body: { sum(mk(1, 2)) };";
        let fwd_source = "struct P { x: i64, y: i64 }\n\
            atom sum(p: P) -> i64 requires: true; ensures: true; body: { p.x + p.y };\n\
            atom fwd(p: P) -> i64 requires: true; ensures: true; body: { sum(p) };";
        let diff_source = "struct P { x: i64, y: i64 }\n\
            atom mk(a: i64, b: i64) -> P requires: true; ensures: true; body: { P { x: a, y: b } };\n\
            atom diff() -> i64 requires: true; ensures: true; body: { let p = mk(1, 2); p.x - p.y };";
        let after_five_source = "struct P { x: i64, y: i64 }\n\
            atom callee5(a: i64, b: i64, c: i64, d: i64, e: i64, p: P) -> i64 \
            requires: true; ensures: true; body: { p.x + p.y };\n\
            atom after_five(a: i64, b: i64, c: i64, d: i64, e: i64) -> i64 \
            requires: true; ensures: true; body: { callee5(a, b, c, d, e, P { x: 1, y: 2 }) };";
        let model = |source, name| {
            source_model_named_with_admission(
                source,
                name,
                &context,
                RefinedAdmission::CompositeV23,
            )
        };
        let caller = model(caller_source, "main");
        let fwd = model(fwd_source, "fwd");
        let diff = model(diff_source, "diff");
        let after_five = model(after_five_source, "after_five");
        assert!(caller.call_sites.iter().any(|call_site| {
            matches!(
                call_site.result_shape,
                CallValueShape::Struct { leaf_count: 2, .. }
            )
        }));
        assert!(caller.call_sites.iter().any(|call_site| {
            call_site
                .argument_shapes
                .iter()
                .any(|shape| matches!(shape, CallValueShape::Struct { leaf_count: 2, .. }))
        }));

        let ir = r#"target triple = "x86_64-unknown-linux-gnu"
declare { i64, i64 } @mk(i64, i64)
declare i64 @sum({ i64, i64 })
define i64 @main() {
entry:
  %pair = call { i64, i64 } @mk(i64 1, i64 2)
  %result = call i64 @sum({ i64, i64 } %pair)
  ret i64 %result
}
define i64 @fwd({ i64, i64 } %pair) {
entry:
  %result = call i64 @sum({ i64, i64 } %pair)
  ret i64 %result
}
define i64 @diff() {
entry:
  %pair = call { i64, i64 } @mk(i64 1, i64 2)
  %x = extractvalue { i64, i64 } %pair, 0
  %y = extractvalue { i64, i64 } %pair, 1
  %result = sub i64 %x, %y
  ret i64 %result
}
"#;
        let (caller_address, caller_bytes, caller_targets) =
            compiled_function_with_targets(ir, "main");
        let caller_disassembly = disassemble(&caller_bytes, caller_address)
            .unwrap()
            .iter()
            .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
            .collect::<Vec<_>>()
            .join("\n");
        println!("struct-valued call disassembly:\n{caller_disassembly}");
        assert_eq!(
            check_function(
                &caller_bytes,
                caller_address,
                &caller,
                &caller_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );

        let (fwd_address, fwd_bytes, fwd_targets) = compiled_function_with_targets(ir, "fwd");
        assert_eq!(
            check_function(&fwd_bytes, fwd_address, &fwd, &fwd_targets, &context),
            FunctionOutcome::Equivalent
        );

        let (diff_address, diff_bytes, diff_targets) = compiled_function_with_targets(ir, "diff");
        assert_eq!(
            check_function(&diff_bytes, diff_address, &diff, &diff_targets, &context),
            FunctionOutcome::Equivalent
        );
        let mk_call_arguments = [0xbf, 1, 0, 0, 0, 0xbe, 2, 0, 0, 0, 0xe8, 0, 0, 0, 0];
        let mut reversed_result_targets = helper_call_targets(&[11]);
        reversed_result_targets.get_mut(&11).unwrap().name = "mk".to_owned();
        assert_eq!(
            check_function(
                &[
                    mk_call_arguments.as_slice(),
                    &[0x48, 0x29, 0xc2, 0x48, 0x89, 0xd0, 0xc3],
                ]
                .concat(),
                0,
                &diff,
                &reversed_result_targets,
                &context,
            ),
            FunctionOutcome::Different
        );
        let mut correct_result_targets = helper_call_targets(&[11]);
        correct_result_targets.get_mut(&11).unwrap().name = "mk".to_owned();
        assert_eq!(
            check_function(
                &[mk_call_arguments.as_slice(), &[0x48, 0x29, 0xd0, 0xc3],].concat(),
                0,
                &diff,
                &correct_result_targets,
                &context,
            ),
            FunctionOutcome::Equivalent
        );

        let fwd_with_swapped_registers = [
            0x48, 0x89, 0xf8, 0x48, 0x89, 0xf7, 0x48, 0x89, 0xc6, 0xe8, 0, 0, 0, 0, 0xc3,
        ];
        let mut swapped_argument_targets = helper_call_targets(&[10]);
        swapped_argument_targets.get_mut(&10).unwrap().name = "sum".to_owned();
        assert_eq!(
            check_function(
                &fwd_with_swapped_registers,
                0,
                &fwd,
                &swapped_argument_targets,
                &context,
            ),
            FunctionOutcome::Different
        );

        let mut after_five_targets = helper_call_targets(&[1]);
        after_five_targets.get_mut(&1).unwrap().name = "callee5".to_owned();
        // The seventh INTEGER leaf spills to the argument stack; a caller that
        // never stores it fails as call_arguments.
        assert_eq!(
            check_function(
                &[0xe8, 0, 0, 0, 0, 0xc3],
                0,
                &after_five,
                &after_five_targets,
                &context,
            ),
            FunctionOutcome::OutsideProfile("call_arguments")
        );
    }

    #[test]
    fn unsupported_composite_call_shapes_fail_closed() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = |source: &str, name: &str| {
            source_model_named_with_admission(
                source,
                name,
                &context,
                RefinedAdmission::CompositeV23,
            )
        };
        let regular_bytes = [0xe8, 0, 0, 0, 0, 0xc3];
        let wide_source = "struct T { x: i64, y: i64, z: i64 }\n\
            atom too_wide(p: T) -> i64 requires: true; ensures: true; body: { p.x + p.y + p.z };\n\
            atom wide_arg() -> i64 requires: true; ensures: true; body: { too_wide(T { x: 1, y: 2, z: 3 }) };";
        let mut targets = helper_call_targets(&[1]);

        let mut wide_arg_targets = targets.clone();
        wide_arg_targets.get_mut(&1).unwrap().name = "too_wide".to_owned();
        assert_eq!(
            check_function(
                &regular_bytes,
                0,
                &model(wide_source, "wide_arg"),
                &wide_arg_targets,
                &context,
            ),
            FunctionOutcome::Different
        );

        let float_source = "struct F { value: f64 }\n\
            atom float_struct(p: F) -> i64 requires: true; ensures: true; body: { 0 };\n\
            atom float_arg(x: f64) -> i64 requires: true; ensures: true; body: { float_struct(F { value: x }) };";
        let mut float_arg_targets = targets.clone();
        float_arg_targets.get_mut(&1).unwrap().name = "float_struct".to_owned();
        let float_arg_model = model(float_source, "float_arg");
        assert!(float_arg_model.call_sites.iter().any(|call_site| {
            call_site
                .argument_shapes
                .iter()
                .any(|shape| matches!(shape, CallValueShape::Struct { has_f64: true, .. }))
        }));
        assert_eq!(
            check_function(
                &regular_bytes,
                0,
                &float_arg_model,
                &float_arg_targets,
                &context,
            ),
            FunctionOutcome::Equivalent
        );

        let enum_float_source = "enum F { A(x: f64), B }\n\
            atom make(v: f64) -> F requires: true; ensures: true; body: { F::A(v) };\n\
            atom discard(v: f64) -> i64 requires: true; ensures: true; body: { let value = make(v); 0 };";
        let enum_float_model = model(enum_float_source, "discard");
        assert!(enum_float_model.has_f64_features);
        // The tag leaf is INTEGER and the `f64` payload leaf reports
        // its SSE class — the two-leaf result fits the per-class
        // register budgets, so the call is admitted: `Different`
        // against this toy `call`/`ret` fixture, not fail-closed.
        assert_eq!(
            enum_float_model.call_sites[0].result_leaf_doubles,
            vec![false, true]
        );
        let mut enum_float_targets = targets.clone();
        enum_float_targets.get_mut(&1).unwrap().name = "make".to_owned();
        assert_eq!(
            check_function(
                &regular_bytes,
                0,
                &enum_float_model,
                &enum_float_targets,
                &context,
            ),
            FunctionOutcome::Different
        );

        let result_source = "struct T { x: i64, y: i64, z: i64 }\n\
            atom wide_result() -> T requires: true; ensures: true; body: { T { x: 1, y: 2, z: 3 } };\n\
            atom wide_result_call() -> i64 requires: true; ensures: true; body: { let p = wide_result(); p.x + p.y + p.z };";
        let mut wide_result_targets = targets.clone();
        wide_result_targets.get_mut(&1).unwrap().name = "wide_result".to_owned();
        assert_eq!(
            check_function(
                &regular_bytes,
                0,
                &model(result_source, "wide_result_call"),
                &wide_result_targets,
                &context,
            ),
            FunctionOutcome::Different
        );

        let three_float_result_source = "struct F3 { a: f64, b: f64, c: f64 }\n\
            atom make_f3(a: f64, b: f64, c: f64) -> F3 requires: true; ensures: true; body: { F3 { a: a, b: b, c: c } };\n\
            atom use_f3(a: f64, b: f64, c: f64) -> f64 requires: true; ensures: true; body: { let p = make_f3(a, b, c); p.c };";
        let mut three_float_result_targets = targets.clone();
        three_float_result_targets.get_mut(&1).unwrap().name = "make_f3".to_owned();
        assert_eq!(
            check_function(
                &regular_bytes,
                0,
                &model(three_float_result_source, "use_f3"),
                &three_float_result_targets,
                &context,
            ),
            FunctionOutcome::OutsideProfile("call_arguments")
        );

        let four_integer_result_source = "struct I4 { a: i64, b: i64, c: i64, d: i64 }\n\
            atom make_i4() -> I4 requires: true; ensures: true; body: { I4 { a: 1, b: 2, c: 3, d: 4 } };\n\
            atom use_i4() -> i64 requires: true; ensures: true; body: { let p = make_i4(); p.d };";
        let mut four_integer_result_targets = targets.clone();
        four_integer_result_targets.get_mut(&1).unwrap().name = "make_i4".to_owned();
        assert_eq!(
            check_function(
                &regular_bytes,
                0,
                &model(four_integer_result_source, "use_i4"),
                &four_integer_result_targets,
                &context,
            ),
            FunctionOutcome::OutsideProfile("call_arguments")
        );

        let five_leaf_result_source = "struct Five { a: i64, b: i64, c: i64, d: i64, e: i64 }\n\
            atom make_five() -> Five requires: true; ensures: true; body: { Five { a: 1, b: 2, c: 3, d: 4, e: 5 } };\n\
            atom use_five() -> i64 requires: true; ensures: true; body: { let p = make_five(); p.e };";
        let mut five_leaf_result_targets = targets.clone();
        five_leaf_result_targets.get_mut(&1).unwrap().name = "make_five".to_owned();
        assert_eq!(
            check_function(
                &regular_bytes,
                0,
                &model(five_leaf_result_source, "use_five"),
                &five_leaf_result_targets,
                &context,
            ),
            FunctionOutcome::OutsideProfile("call_arguments")
        );

        let tail_source = "struct P { x: i64, y: i64 }\n\
            atom sum(p: P) -> i64 requires: true; ensures: true; body: { p.x + p.y };\n\
            atom tail_struct(p: P) -> i64 requires: true; ensures: true; body: { sum(p) };";
        targets.get_mut(&1).unwrap().name = "sum".to_owned();
        assert_eq!(
            check_function(
                &[0xe9, 0, 0, 0, 0],
                0,
                &model(tail_source, "tail_struct"),
                &targets,
                &context,
            ),
            FunctionOutcome::OutsideProfile("call_arguments")
        );

        let after_four_source = "struct T { a: i64, b: i64, c: i64 }\n\
            atom target(a: i64, b: i64, c: i64, d: i64, p: T) -> i64 requires: true; ensures: true; body: { p.a + p.b + p.c };\n\
            atom after_four(a: i64, b: i64, c: i64, d: i64) -> i64 requires: true; ensures: true; body: { target(a, b, c, d, T { a: 1, b: 2, c: 3 }) };";
        let mut after_four_targets = targets;
        after_four_targets.get_mut(&1).unwrap().name = "target".to_owned();
        // The struct's last leaf spills to the argument stack; a caller that
        // never stores it fails as call_arguments.
        assert_eq!(
            check_function(
                &regular_bytes,
                0,
                &model(after_four_source, "after_four"),
                &after_four_targets,
                &context,
            ),
            FunctionOutcome::OutsideProfile("call_arguments")
        );
    }

    #[test]
    fn compiled_leaf_helper_matches_its_pure_i64_model() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model(
            "atom helper(x: i64) -> i64 requires: true; ensures: result == x + 1; body: { x + 1 };",
            &context,
        );
        let (address, bytes) = compiled_helper();
        assert_eq!(
            check_function(&bytes, address, &model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );
    }

    #[test]
    fn refined_parameter_domain_is_used_for_machine_tv_equivalence() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let source = "type Pos = i64 where v > 0;\n\
            atom inc(x: Pos) -> i64 requires: true; ensures: true; body: { x + 1 };";
        let model = source_model_with_admission(source, &context, RefinedAdmission::V1);
        assert!(model.refined_signature);
        assert_eq!(model.param_domain.len(), 1);

        let mut unbounded_model =
            source_model_with_admission(source, &context, RefinedAdmission::V1);
        unbounded_model.param_domain.clear();
        let bytes = [
            0x48, 0x85, 0xff, // testq %rdi, %rdi
            0x7e, 0x05, // jle to the zero-return path
            0x48, 0x8d, 0x47, 0x01, // leaq 1(%rdi), %rax
            0xc3, // retq
            0x31, 0xc0, // xorl %eax, %eax
            0xc3, // retq
        ];
        assert_eq!(
            check_function_with_preservation(&bytes, 0, &model, &BTreeMap::new(), &context),
            (
                FunctionOutcome::Equivalent,
                Some(CalleeSavedPreservation::Preserved)
            )
        );
        assert_eq!(
            check_function(&bytes, 0, &unbounded_model, &BTreeMap::new(), &context),
            FunctionOutcome::Different
        );
    }

    #[test]
    fn unsupported_memory_and_other_feature_flags_fail_closed() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let source = "atom identity(x: i64) -> i64 requires: true; ensures: true; body: { x };";
        let bytes = [0x48, 0x89, 0xf8, 0xc3];

        let mut memory_model = source_model(source, &context);
        memory_model.bounded_memory = true;
        assert_eq!(
            check_function(&bytes, 0, &memory_model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );

        let mut floating_model = source_model(source, &context);
        floating_model.has_f64_features = true;
        assert_eq!(
            check_function(&bytes, 0, &floating_model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );

        let mut non_i64_model = source_model(source, &context);
        non_i64_model.has_non_i64_scalar_features = true;
        assert_eq!(
            check_function(&bytes, 0, &non_i64_model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );
    }

    #[test]
    fn u64_machine_tv_uses_full_width_unsigned_bitvector_semantics() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let source = "atom add(a: u64, b: u64) -> u64 requires: true; ensures: true; body: { a + b };\n\
            atom udiv(a: u64, b: u64) -> u64 requires: b != 0; ensures: true; body: { a / b };\n\
            atom urem(a: u64, b: u64) -> u64 requires: b != 0; ensures: true; body: { a % b };\n\
            atom ult(a: u64, b: u64) -> bool requires: true; ensures: true; body: { a < b };\n\
            atom ugt(a: u64, b: u64) -> bool requires: true; ensures: true; body: { a > b };\n\
            atom shr(a: u64, n: u64) -> u64 requires: true; ensures: true; body: { a >> n };\n\
            atom umax(a: u64, b: u64) -> u64 requires: true; ensures: true; body: { if a > b { a } else { b } };";
        let model = |name| {
            source_model_named_with_admission(
                source,
                name,
                &context,
                RefinedAdmission::CompositeV10,
            )
        };
        let add_model = model("add");
        assert!(add_model.has_non_i64_scalar_features);
        assert!(!add_model.has_wide_scalar_features);
        assert!(add_model.inputs.iter().all(|input| input.get_size() == 64));
        assert!(add_model.param_domain.is_empty());
        assert!(supports_signature(&add_model));

        let ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @add(i64 %a, i64 %b) {
entry:
  %r = add i64 %a, %b
  ret i64 %r
}
define i64 @udiv(i64 %a, i64 %b) {
entry:
  %r = udiv i64 %a, %b
  ret i64 %r
}
define i64 @urem(i64 %a, i64 %b) {
entry:
  %r = urem i64 %a, %b
  ret i64 %r
}
define i64 @ult(i64 %a, i64 %b) {
entry:
  %c = icmp ult i64 %a, %b
  %r = zext i1 %c to i64
  ret i64 %r
}
define i64 @ugt(i64 %a, i64 %b) {
entry:
  %c = icmp ugt i64 %a, %b
  %r = zext i1 %c to i64
  ret i64 %r
}
define i64 @shr(i64 %a, i64 %n) {
entry:
  %r = lshr i64 %a, %n
  ret i64 %r
}
define { i64, i64 } @pass({ i64, i64 } %p) {
entry:
  ret { i64, i64 } %p
}
define i64 @callee(i64 %x) {
entry:
  %r = add i64 %x, 1
  ret i64 %r
}
define i64 @caller(i64 %x) {
entry:
  %r = call i64 @callee(i64 %x)
  ret i64 %r
}
define i64 @umax(i64 %a, i64 %b) {
entry:
  %c = icmp ugt i64 %a, %b
  %r = select i1 %c, i64 %a, i64 %b
  ret i64 %r
}
"#;
        for name in ["add", "udiv", "urem", "ult", "ugt", "shr", "umax"] {
            let model = model(name);
            let (address, bytes, targets) = compiled_function_with_targets(ir, name);
            if matches!(name, "udiv" | "urem" | "ult" | "ugt" | "shr" | "umax") {
                let disassembly = disassemble(&bytes, address)
                    .unwrap()
                    .iter()
                    .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
                    .collect::<Vec<_>>()
                    .join("\n");
                println!("u64 {name} disassembly:\n{disassembly}");
            }
            assert_eq!(
                check_function(&bytes, address, &model, &targets, &context),
                FunctionOutcome::Equivalent,
                "{name}"
            );
        }

        let aggregate_source = "struct Pair { a: u64, b: i64 }\n\
            atom pass(p: Pair) -> Pair requires: true; ensures: true; body: { p };\n\
            atom callee(x: u64) -> u64 requires: true; ensures: true; body: { x + 1 };\n\
            atom caller(x: u64) -> u64 requires: true; ensures: true; body: { callee(x) };";
        let aggregate_model = |name| {
            source_model_named_with_admission(
                aggregate_source,
                name,
                &context,
                RefinedAdmission::CompositeV10,
            )
        };

        for name in ["pass", "callee", "caller"] {
            let model = aggregate_model(name);
            let (address, bytes, targets) = compiled_function_with_targets(ir, name);
            assert_eq!(
                check_function(&bytes, address, &model, &targets, &context),
                FunctionOutcome::Equivalent,
                "{name}"
            );
        }

        let branch_source = "atom gt_branch(a: u64, b: u64) -> u64 requires: true; ensures: true; body: { if a > b { 1 } else { 0 } };\n\
            atom lt_branch(a: u64, b: u64) -> u64 requires: true; ensures: true; body: { if a < b { 1 } else { 0 } };";
        let branch_model = |name| {
            source_model_named_with_admission(
                branch_source,
                name,
                &context,
                RefinedAdmission::CompositeV10,
            )
        };
        let gt_model = branch_model("gt_branch");
        let ja_bytes = [
            0x48, 0x39, 0xf7, // cmpq %rsi, %rdi
            0x0f, 0x87, 0x03, 0x00, 0x00, 0x00, // ja true
            0x31, 0xc0, // xorl %eax, %eax
            0xc3, // retq
            0xb8, 0x01, 0x00, 0x00, 0x00, // movl $1, %eax
            0xc3, // retq
        ];
        assert_eq!(
            check_function(&ja_bytes, 0, &gt_model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );
        let mut jg_bytes = ja_bytes;
        jg_bytes[4] = 0x8f;
        assert_eq!(
            check_function(&jg_bytes, 0, &gt_model, &BTreeMap::new(), &context),
            FunctionOutcome::Different
        );

        let lt_model = branch_model("lt_branch");
        let jb_bytes = [
            0x48, 0x39, 0xf7, // cmpq %rsi, %rdi
            0x0f, 0x82, 0x03, 0x00, 0x00, 0x00, // jb true
            0x31, 0xc0, // xorl %eax, %eax
            0xc3, // retq
            0xb8, 0x01, 0x00, 0x00, 0x00, // movl $1, %eax
            0xc3, // retq
        ];
        assert_eq!(
            check_function(&jb_bytes, 0, &lt_model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );

        let cvtsi2sdq = [0xf2, 0x48, 0x0f, 0x2a, 0xc7, 0xc3];
        assert_eq!(
            check_function(&cvtsi2sdq, 0, &add_model, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );

        let wide_source = "atom wide(x: i128) -> i128 requires: true; ensures: true; body: { x };";
        let wide_model = source_model_named_with_admission(
            wide_source,
            "wide",
            &context,
            RefinedAdmission::CompositeV21,
        );
        assert!(wide_model.has_wide_scalar_features);
        assert_eq!(wide_model.inputs.len(), 2);
        assert!(wide_model.inputs.iter().all(|input| input.get_size() == 64));
        assert!(wide_model.param_domain.is_empty());
        assert_eq!(wide_model.value.get_size(), 128);
        assert!(supports_signature(&wide_model));
    }

    #[test]
    fn wide_alias_leaf_doubles_match_codegen() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        // An alias-typed 128-bit leaf must lay out the same two
        // 64-bit ABI slots the literal leaf takes: a literal-name
        // check would record one slot and desync the signature.
        let alias_source = "type Big = u128 where v <= 10;\n\
            atom wide(a: Big) -> Big requires: true; ensures: true; body: { a };";
        let alias_model = source_model_named_with_admission(
            alias_source,
            "wide",
            &context,
            RefinedAdmission::CompositeV21,
        );
        assert_eq!(alias_model.param_leaf_doubles, vec![vec![false, false]]);
        assert_eq!(alias_model.result_leaf_doubles, vec![false, false]);
        assert_eq!(alias_model.inputs.len(), 2);
        assert!(supports_signature(&alias_model));
        // The same two-slot shape inside an enum payload slot: tag
        // plus two non-double leaves (`pure-i64-composite/v22`).
        let enum_source = "type Big = u128 where v <= 10;\n\
            enum F { A(Big), B(Big) }\n\
            atom pick(f: F) -> u128 requires: true; ensures: true; body: { match f { F::A(x) => x, _ => 0 } };";
        let enum_model = source_model_named_with_admission(
            enum_source,
            "pick",
            &context,
            RefinedAdmission::CompositeV22,
        );
        assert_eq!(
            enum_model.param_leaf_doubles,
            vec![vec![false, false, false]]
        );
        assert_eq!(enum_model.param_shapes, vec![Some(3)]);
        // An enum-typed struct field with an alias wide payload also
        // expands: tag + two slots, then the `i64` sibling leaf.
        let nested_source = "type Big = u128 where v <= 10;\n\
            enum F { A(Big), B(Big) }\n\
            struct S { f: F, k: i64 }\n\
            atom get(s: S) -> i64 requires: true; ensures: true; body: { s.k };";
        let nested_model = source_model_named_with_admission(
            nested_source,
            "get",
            &context,
            RefinedAdmission::CompositeV22,
        );
        assert_eq!(
            nested_model.param_leaf_doubles,
            vec![vec![false, false, false, false]]
        );
        assert_eq!(nested_model.param_shapes, vec![Some(4)]);
        assert_eq!(nested_model.inputs.len(), 4);
    }

    #[test]
    fn wide_aggregate_arguments_match_codegen() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        // A 128-bit leaf inside an aggregate parameter flattens into
        // the same two-INTEGER-slot sequence LLVM emits for it.
        let source = "struct P { a: i64, b: i128 }\n\
            atom leaf(p: P) -> i128 requires: true; ensures: result == p.b; body: { p.b };\n\
            atom caller(x: i128) -> i128 requires: true; ensures: result == x; body: { leaf(P { a: 0, b: x }) };";
        let leaf_model = source_model_named_with_admission(
            source,
            "leaf",
            &context,
            RefinedAdmission::CompositeV21,
        );
        assert_eq!(leaf_model.param_shapes, vec![Some(3)]);
        assert_eq!(
            leaf_model.param_leaf_doubles,
            vec![vec![false, false, false]]
        );
        assert!(supports_signature(&leaf_model));
        let caller_model = source_model_named_with_admission(
            source,
            "caller",
            &context,
            RefinedAdmission::CompositeV21,
        );
        assert!(supports_signature(&caller_model));
        let ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i128 @leaf({ i64, i128 } %p) {
entry:
  %b = extractvalue { i64, i128 } %p, 1
  ret i128 %b
}
define i128 @leaf_wrong({ i64, i128 } %p) {
entry:
  %a = extractvalue { i64, i128 } %p, 0
  %w = zext i64 %a to i128
  ret i128 %w
}
define i128 @caller(i128 %x) {
entry:
  %p0 = insertvalue { i64, i128 } undef, i64 0, 0
  %p1 = insertvalue { i64, i128 } %p0, i128 %x, 1
  %r = call i128 @leaf({ i64, i128 } %p1)
  ret i128 %r
}
"#;
        let (leaf_address, leaf_bytes, leaf_targets) = compiled_function_with_targets(ir, "leaf");
        assert_eq!(
            check_function(
                &leaf_bytes,
                leaf_address,
                &leaf_model,
                &leaf_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );
        // Returning the wrong field is a real counterexample, not a
        // vacuous pass.
        let (wrong_address, wrong_bytes, wrong_targets) =
            compiled_function_with_targets(ir, "leaf_wrong");
        assert_eq!(
            check_function(
                &wrong_bytes,
                wrong_address,
                &leaf_model,
                &wrong_targets,
                &context
            ),
            FunctionOutcome::Different
        );
        // The call-site composite argument carrying a wide leaf is
        // checked leaf-wise as well.
        let (caller_address, caller_bytes, caller_targets) =
            compiled_function_with_targets(ir, "caller");
        assert_eq!(
            check_function(
                &caller_bytes,
                caller_address,
                &caller_model,
                &caller_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );
        // A wide leaf inside an aggregate *result* is admitted the
        // same way: LLVM emits the result leaf-by-leaf into
        // `rax`/`rdx`/`rcx`, and a 128-bit leaf contributes its low
        // half before its high half — exactly the `[lo, hi]` slot
        // order the source model uses.
        let mk_model = source_model_named_with_admission(
            "struct P { a: i64, b: i128 }\n\
             atom mk(x: i128) -> P requires: true; ensures: true; body: { P { a: 0, b: x } };",
            "mk",
            &context,
            RefinedAdmission::CompositeV21,
        );
        assert_eq!(mk_model.result_shape, Some(3));
        assert_eq!(mk_model.result_leaf_doubles, vec![false, false, false]);
        assert!(supports_signature(&mk_model));
        let ir = r#"target triple = "x86_64-unknown-linux-gnu"
define { i64, i128 } @mk(i128 %x) {
entry:
  %p0 = insertvalue { i64, i128 } undef, i64 0, 0
  %p1 = insertvalue { i64, i128 } %p0, i128 %x, 1
  ret { i64, i128 } %p1
}
define { i64, i128 } @mk_wrong(i128 %x) {
entry:
  %lo = trunc i128 %x to i64
  %shr = lshr i128 %x, 64
  %hi = trunc i128 %shr to i64
  %sw0 = zext i64 %lo to i128
  %sw = shl i128 %sw0, 64
  %hz = zext i64 %hi to i128
  %b = or i128 %sw, %hz
  %p0 = insertvalue { i64, i128 } undef, i64 0, 0
  %p1 = insertvalue { i64, i128 } %p0, i128 %b, 1
  ret { i64, i128 } %p1
}
"#;
        let (mk_address, mk_bytes, mk_targets) = compiled_function_with_targets(ir, "mk");
        assert_eq!(
            check_function(&mk_bytes, mk_address, &mk_model, &mk_targets, &context),
            FunctionOutcome::Equivalent
        );
        // Returning the wide leaf's halves swapped is a real
        // counterexample, not a vacuous pass.
        let (wrong_address, wrong_bytes, wrong_targets) =
            compiled_function_with_targets(ir, "mk_wrong");
        assert_eq!(
            check_function(
                &wrong_bytes,
                wrong_address,
                &mk_model,
                &wrong_targets,
                &context
            ),
            FunctionOutcome::Different
        );
    }

    #[test]
    fn i128_u128_machine_tv_models_two_gpr_scalar_values() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = |name| {
            let source = match name {
                "add" => {
                    "atom add(x: i128, y: i128) -> i128 requires: true; ensures: true; body: { x + y };"
                }
                "sub" => {
                    "atom sub(x: i128, y: i128) -> i128 requires: true; ensures: true; body: { x - y };"
                }
                "neg" => "atom neg(x: i128) -> i128 requires: true; ensures: true; body: { -x };",
                "mul" => {
                    "atom mul(x: i128, y: i128) -> i128 requires: true; ensures: true; body: { x * y };"
                }
                "slt" => {
                    "atom slt(x: i128, y: i128) -> bool requires: true; ensures: true; body: { x < y };"
                }
                "ult" => {
                    "atom ult(x: u128, y: u128) -> bool requires: true; ensures: true; body: { x < y };"
                }
                "copy" => "atom copy(x: i128) -> i128 requires: true; ensures: true; body: { x };",
                "callee" => {
                    "atom callee(x: i128) -> i128 requires: true; ensures: true; body: { x + 1 };"
                }
                "caller" => {
                    "atom callee(x: i128) -> i128 requires: true; ensures: true; body: { x + 1 };\n\
                     atom caller(x: i128) -> i128 requires: true; ensures: true; body: { callee(x) };"
                }
                "divide" => {
                    "atom divide(x: i128, y: i128) -> i128 requires: y != 0; ensures: true; body: { x / y };"
                }
                "udivide" => {
                    "atom udivide(x: u128, y: u128) -> u128 requires: y != 0; ensures: true; body: { x / y };"
                }
                "after_five" => {
                    "atom after_five(a: i64, b: i64, c: i64, d: i64, e: i64, x: i128) -> i64 requires: true; ensures: true; body: { 0 };"
                }
                "ret_hi_wide" => {
                    "atom ret_hi_wide(a: i64, b: i64, c: i64, d: i64, e: i64, x: u128) -> u128 requires: true; ensures: true; body: { x >> 64 };"
                }
                "wide_call" => {
                    "atom wide_callee(x: u128, a: i64, b: i64, c: i64, y: u128) -> i64 requires: true; ensures: result == a + b + c; body: { a + b + c };\n\
                     atom wide_call(z: u128) -> i64 requires: true; ensures: true; body: { wide_callee(z, 1, 2, 3, z) };"
                }
                _ => unreachable!(),
            };
            source_model_named_with_admission(
                source,
                name,
                &context,
                RefinedAdmission::CompositeV21,
            )
        };
        let add_model = model("add");
        assert!(add_model.has_wide_scalar_features);
        assert_eq!(add_model.inputs.len(), 4);
        assert!(add_model.inputs.iter().all(|input| input.get_size() == 64));
        assert!(add_model.param_domain.is_empty());
        assert_eq!(add_model.value.get_size(), 128);
        assert!(supports_signature(&add_model));

        let ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i128 @add(i128 %x, i128 %y) {
  %r = add i128 %x, %y
  ret i128 %r
}
define i128 @sub(i128 %x, i128 %y) {
  %r = sub i128 %x, %y
  ret i128 %r
}
define i128 @neg(i128 %x) {
  %r = sub i128 0, %x
  ret i128 %r
}
define i128 @mul(i128 %x, i128 %y) {
  %r = mul i128 %x, %y
  ret i128 %r
}
define i64 @slt(i128 %x, i128 %y) {
  %c = icmp slt i128 %x, %y
  %r = zext i1 %c to i64
  ret i64 %r
}
define i64 @ult(i128 %x, i128 %y) {
  %c = icmp ult i128 %x, %y
  %r = zext i1 %c to i64
  ret i64 %r
}
define i128 @copy(i128 %x) {
  ret i128 %x
}
define i128 @callee(i128 %x) {
  %r = add i128 %x, 1
  ret i128 %r
}
define i128 @caller(i128 %x) {
  %r = call i128 @callee(i128 %x)
  ret i128 %r
}
define i128 @divide(i128 %x, i128 %y) {
  %r = sdiv i128 %x, %y
  ret i128 %r
}
define i128 @udivide(i128 %x, i128 %y) {
  %r = udiv i128 %x, %y
  ret i128 %r
}
define i64 @after_five(i64 %a, i64 %b, i64 %c, i64 %d, i64 %e, i128 %x) {
  ret i64 0
}
"#;
        for name in [
            "add", "sub", "neg", "slt", "ult", "copy", "callee", "caller",
        ] {
            let (address, bytes, targets) = compiled_function_with_targets(ir, name);
            assert_eq!(
                check_function(&bytes, address, &model(name), &targets, &context),
                FunctionOutcome::Equivalent,
                "{name}"
            );
        }
        let (address, bytes, targets) = compiled_function_with_targets(ir, "mul");
        assert_eq!(
            check_function(&bytes, address, &model("mul"), &targets, &context),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );

        let (address, mut add_bytes, targets) = compiled_function_with_targets(ir, "add");
        let adc_offset = add_bytes
            .windows(3)
            .position(|bytes| bytes == [0x48, 0x11, 0xce])
            .unwrap();
        add_bytes[adc_offset + 1] = 0x01;
        assert_eq!(
            check_function(&add_bytes, address, &model("add"), &targets, &context),
            FunctionOutcome::Different
        );

        let (address, _bytes, targets) = compiled_function_with_targets(ir, "copy");
        let swapped_result = [0x48, 0x89, 0xf0, 0x48, 0x89, 0xfa, 0xc3];
        assert_eq!(
            check_function(&swapped_result, address, &model("copy"), &targets, &context,),
            FunctionOutcome::Different
        );
        // The backend lowers `sdiv`/`udiv i128` to `__divti3`/`__udivti3`
        // calls, which the lifter models by their SysV pair convention —
        // no source call site is consumed.
        let (address, bytes, targets) = compiled_function_with_targets(ir, "divide");
        assert_eq!(
            check_function(&bytes, address, &model("divide"), &targets, &context),
            FunctionOutcome::Equivalent
        );
        let (address, bytes, targets) = compiled_function_with_targets(ir, "udivide");
        assert_eq!(
            check_function(&bytes, address, &model("udivide"), &targets, &context),
            FunctionOutcome::Equivalent
        );
        let (address, bytes, targets) = compiled_function_with_targets(ir, "after_five");
        // Seven flattened INTEGER leaves: `x.lo` stays in `r9` and only
        // `x.hi` spills to the argument stack, so the signature is inside
        // the profile now. The machine ignores `x`, which is equivalent to
        // the model's `ret i64 0`.
        assert_eq!(
            check_function(&bytes, address, &model("after_five"), &targets, &context),
            FunctionOutcome::Equivalent
        );
        // Reading the spilled `x.hi` slot binds it to the high half of the
        // wide argument, not to `x.lo` or a register, so returning it does
        // not satisfy `ret i64 0`.
        let read_spilled_hi = [0x48, 0x8b, 0x44, 0x24, 0x08, 0xc3];
        assert_eq!(
            check_function(
                &read_spilled_hi,
                address,
                &model("after_five"),
                &targets,
                &context
            ),
            FunctionOutcome::Different
        );
        // `x / 2^64` reads `x.hi` back out of the spilled slot: returning it
        // (low half in `rax`, zeroed high half in `rdx`) is Equivalent, and
        // returning the registered `x.lo` from `r9` instead is Different —
        // together these pin the slot to `x.hi` specifically.
        let ret_spilled_hi = [
            0x48, 0x8b, 0x44, 0x24, 0x08, // movq 8(%rsp), %rax
            0x31, 0xd2, // xorl %edx, %edx
            0xc3,
        ];
        assert_eq!(
            check_function(
                &ret_spilled_hi,
                address,
                &model("ret_hi_wide"),
                &targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );
        let ret_registered_lo = [
            0x41, 0x51, // pushq %r9   -- x.lo is the registered half
            0x58, // popq %rax
            0x31, 0xd2, // xorl %edx, %edx
            0xc3,
        ];
        assert_eq!(
            check_function(
                &ret_registered_lo,
                address,
                &model("ret_hi_wide"),
                &targets,
                &context
            ),
            FunctionOutcome::Different
        );

        // Outgoing side, matching real LLVM codegen for `wide_call`:
        // `y.lo` keeps `r9` while only `y.hi` is stored into the outgoing
        // stack slot.
        let wide_call_bytes = [
            0x50, // pushq %rax
            0x48, 0x89, 0x34, 0x24, // movq %rsi, (%rsp)   -- y.hi = z.hi
            0xba, 0x01, 0x00, 0x00, 0x00, // movl $1, %edx
            0xb9, 0x02, 0x00, 0x00, 0x00, // movl $2, %ecx
            0x41, 0xb8, 0x03, 0x00, 0x00, 0x00, // movl $3, %r8d
            0x49, 0x89, 0xf9, // movq %rdi, %r9          -- y.lo = z.lo
            0xe8, 0x00, 0x00, 0x00, 0x00, // callq wide_callee
            0x59, // popq %rcx
            0xc3,
        ];
        let wide_call_targets: BTreeMap<u64, CallTarget> = [(
            25u64,
            CallTarget {
                name: "wide_callee".to_owned(),
                relocation_kind: CallRelocationKind::ElfX86_64Plt32,
                relocation_size: 32,
                addend: -4,
            },
        )]
        .into_iter()
        .collect();
        assert_eq!(
            check_function(
                &wide_call_bytes,
                address,
                &model("wide_call"),
                &wide_call_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );
        // Storing the registered low half into the slot feeds `y.hi` the
        // wrong half of `z`.
        let mut wrong_half = wide_call_bytes;
        wrong_half[3] = 0x3c; // movq %rdi, (%rsp)
        assert_eq!(
            check_function(
                &wrong_half,
                address,
                &model("wide_call"),
                &wide_call_targets,
                &context
            ),
            FunctionOutcome::Different
        );
    }

    #[test]
    fn i128_variable_shifts_lift_shld_shrd() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = |name| {
            let source = match name {
                "shl128" => {
                    "atom shl128(x: i128, n: i128) -> i128 requires: n >= 0 && n < 64; ensures: true; body: { x << n };"
                }
                "lshr128" => {
                    "atom lshr128(x: u128, n: u128) -> u128 requires: n >= 0 && n < 64; ensures: true; body: { x >> n };"
                }
                "ashr128" => {
                    "atom ashr128(x: i128, n: i128) -> i128 requires: n >= 0 && n < 64; ensures: true; body: { x >> n };"
                }
                _ => unreachable!(),
            };
            source_model_named_with_admission(
                source,
                name,
                &context,
                RefinedAdmission::CompositeV21,
            )
        };
        // Real `llc -mcpu=generic` codegen for a variable i128 shift:
        // `shld`/`shrd` move the cross-word bits while `test $0x40,%cl`
        // + `cmov` select the `count >= 64` fallback — both halves of
        // the pattern are lifted, so the whole sequence is equivalent
        // over the source's defined wide-shift domain (`n` in 0..128;
        // the `requires` bound does not enter the compared domain).
        let ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i128 @shl128(i128 %x, i128 %n) {
  %r = shl i128 %x, %n
  ret i128 %r
}
define i128 @lshr128(i128 %x, i128 %n) {
  %r = lshr i128 %x, %n
  ret i128 %r
}
define i128 @ashr128(i128 %x, i128 %n) {
  %r = ashr i128 %x, %n
  ret i128 %r
}
"#;
        for name in ["shl128", "lshr128", "ashr128"] {
            let (address, bytes, targets) = compiled_function_with_targets(ir, name);
            assert_eq!(
                check_function(&bytes, address, &model(name), &targets, &context),
                FunctionOutcome::Equivalent,
                "{name}"
            );
        }
        // The direction of the double shift is semantic, not cosmetic:
        // running the `shl128` code against the `lshr128` model must
        // produce a counterexample, not a vacuous pass.
        let (address, bytes, targets) = compiled_function_with_targets(ir, "shl128");
        assert_eq!(
            check_function(&bytes, address, &model("lshr128"), &targets, &context),
            FunctionOutcome::Different
        );
    }

    #[test]
    fn scalar_f64_sse2_machine_tv_models_results_and_mixed_abi_arguments() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let source = "atom fadd(x: f64, y: f64) -> f64 requires: true; ensures: true; body: { x * y + x };\n\
            atom add(x: f64, y: f64) -> f64 requires: true; ensures: true; body: { x + y };\n\
            atom feq(x: f64, y: f64) -> bool requires: true; ensures: true; body: { x == y };\n\
            atom mixed(x: i64, y: f64) -> f64 requires: true; ensures: true; body: { y };\n\
            atom stack_mixed(a: i64, b: i64, c: i64, d: i64, e: i64, f: i64, p: f64, g: i64) -> i64 requires: true; ensures: true; body: { g };\n\
            atom callee(x: f64) -> f64 requires: true; ensures: true; body: { x };\n\
            atom caller(x: f64) -> f64 requires: true; ensures: true; body: { callee(x) };";

        let fadd = source_model_named_with_admission(
            source,
            "fadd",
            &context,
            RefinedAdmission::CompositeV19,
        );
        let fadd_bytes = [
            0xf2, 0x0f, 0x59, 0xc8, // mulsd %xmm0, %xmm1
            0xf2, 0x0f, 0x58, 0xc1, // addsd %xmm1, %xmm0
            0xc3, // retq
        ];
        assert_eq!(
            check_function_with_preservation(&fadd_bytes, 0, &fadd, &BTreeMap::new(), &context,),
            (
                FunctionOutcome::Equivalent,
                Some(CalleeSavedPreservation::Preserved)
            )
        );

        let add = source_model_named_with_admission(
            source,
            "add",
            &context,
            RefinedAdmission::CompositeV19,
        );
        let swapped_operands = [
            0xf2, 0x0f, 0x58, 0xc8, // addsd %xmm0, %xmm1
            0x66, 0x0f, 0x28, 0xc1, // movapd %xmm1, %xmm0
            0xc3,
        ];
        // Reversed NaN payload priority is covered by the NaN-aware result
        // obligation; for non-NaNs, floating-point addition is commutative.
        assert_eq!(
            check_function(&swapped_operands, 0, &add, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );
        let subtraction = [
            0xf2, 0x0f, 0x5c, 0xc1, // subsd %xmm1, %xmm0
            0xc3,
        ];
        assert_eq!(
            check_function(&subtraction, 0, &add, &BTreeMap::new(), &context),
            FunctionOutcome::Different
        );
        let memory_operand = [
            0xf2, 0x0f, 0x58, 0x00, // addsd (%rax), %xmm0
            0xc3,
        ];
        assert_eq!(
            check_function(&memory_operand, 0, &add, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );
        let unsupported_compare = [
            0x66, 0x0f, 0x2e, 0xc1, // ucomisd %xmm1, %xmm0
            0xc3,
        ];
        assert_eq!(
            check_function(&unsupported_compare, 0, &add, &BTreeMap::new(), &context,),
            FunctionOutcome::Different
        );
        let memory_compare = [
            0x66, 0x0f, 0x2e, 0x00, // ucomisd (%rax), %xmm0
            0xc3,
        ];
        assert_eq!(
            check_function(&memory_compare, 0, &add, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );

        let feq = source_model_named_with_admission(
            source,
            "feq",
            &context,
            RefinedAdmission::CompositeV19,
        );
        let equality = [
            0xf2, 0x0f, 0xc2, 0xc1, 0x00, // cmpeqsd %xmm1, %xmm0
            0x66, 0x48, 0x0f, 0x7e, 0xc0, // movq %xmm0, %rax
            0x83, 0xe0, 0x01, // andl $1, %eax
            0xc3,
        ];
        assert_eq!(
            check_function(&equality, 0, &feq, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );
        let nan_bits_as_integer = [
            0x66, 0x48, 0x0f, 0x7e, 0xc0, // movq %xmm0, %rax
            0xc3,
        ];
        assert_eq!(
            check_function(&nan_bits_as_integer, 0, &feq, &BTreeMap::new(), &context),
            FunctionOutcome::Different
        );

        let mixed = source_model_named_with_admission(
            source,
            "mixed",
            &context,
            RefinedAdmission::CompositeV19,
        );
        assert_eq!(
            check_function(&[0xc3], 0, &mixed, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );
        let stack_mixed = source_model_named_with_admission(
            source,
            "stack_mixed",
            &context,
            RefinedAdmission::CompositeV19,
        );
        let stack_argument = [
            0x48, 0x8b, 0x44, 0x24, 0x08, // movq 8(%rsp), %rax
            0xc3,
        ];
        assert_eq!(
            check_function(&stack_argument, 0, &stack_mixed, &BTreeMap::new(), &context,),
            FunctionOutcome::Equivalent
        );

        // Nine `f64` arguments are no longer out of profile — the
        // ninth spills to the argument stack like an INTEGER leaf past
        // `r9`. A bare `retq` returning `xmm0` still matches this
        // model, which returns the first argument.
        let too_many_f64 = source_model_with_admission(
            "atom too_many(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64, g: f64, h: f64, i: f64) -> f64 requires: true; ensures: true; body: { a };",
            &context,
            RefinedAdmission::CompositeV19,
        );
        assert_eq!(
            check_function(&[0xc3], 0, &too_many_f64, &BTreeMap::new(), &context,),
            FunctionOutcome::Equivalent
        );

        let caller = source_model_named_with_admission(
            source,
            "caller",
            &context,
            RefinedAdmission::CompositeV19,
        );
        assert_eq!(caller.call_sites[0].arg_leaf_doubles, vec![true]);
        let caller_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define double @callee(double %x) {
entry:
  ret double %x
}
define double @caller(double %x) {
entry:
  %result = call double @callee(double %x)
  ret double %result
}
"#;
        let (address, bytes, call_targets) = compiled_function_with_targets(caller_ir, "caller");
        assert_eq!(
            check_function(&bytes, address, &caller, &call_targets, &context),
            FunctionOutcome::Equivalent
        );

        let float_result = BV::fresh_const(&context, "f64_call_result", 64);
        let mut float_result_caller =
            PureI64Body::synthetic_scalar("float_result_caller", float_result.clone());
        float_result_caller.result_leaf_doubles = vec![true];
        float_result_caller.call_sites.push(CallSite {
            callee: "helper".to_owned(),
            args: Vec::new(),
            callback_arguments: Vec::new(),
            arg_leaf_doubles: Vec::new(),
            argument_shapes: Vec::new(),
            has_composite_arguments_or_result: false,
            result: float_result,
            result_is_double: true,
            result_store: None,
            result_shape: CallValueShape::Scalar { leaf_count: 1 },
            result_leaf_doubles: vec![true],
            guard: Bool::from_bool(&context, true),
        });
        assert_eq!(
            check_function(
                &[0xe8, 0, 0, 0, 0, 0xc3],
                0,
                &float_result_caller,
                &helper_call_targets(&[1]),
                &context,
            ),
            FunctionOutcome::Equivalent
        );
    }

    #[test]
    fn f64_call_arguments_require_exact_bits_after_nan_aware_equivalence() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let source = "atom callee(x: f64) -> f64 requires: true; ensures: true; body: { x };\n\
            atom caller(x: f64) -> f64 requires: true; ensures: true; body: { callee(x) };\n\
            atom computed(x: f64, y: f64) -> f64 requires: true; ensures: true; body: { callee(x + y) };\n\
            atom mixed_callee(i: i64, x: f64) -> f64 requires: true; ensures: true; body: { x };\n\
            atom mixed_caller(i: i64, x: f64) -> f64 requires: true; ensures: true; body: { mixed_callee(i, x) };\n\
            atom twice(x: f64) -> f64 requires: true; ensures: true; body: { callee(x); callee(x) };";
        let admission = RefinedAdmission::CompositeV19;
        let caller = source_model_named_with_admission(source, "caller", &context, admission);
        let computed = source_model_named_with_admission(source, "computed", &context, admission);
        let mixed_caller =
            source_model_named_with_admission(source, "mixed_caller", &context, admission);
        let twice = source_model_named_with_admission(source, "twice", &context, admission);

        let pass_through_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define double @callee(double %x) {
entry:
  ret double %x
}
define double @caller(double %x) {
entry:
  %result = call double @callee(double %x)
  ret double %result
}
"#;
        let (address, bytes, call_targets) =
            compiled_function_with_targets(pass_through_ir, "caller");
        let instructions = disassemble(&bytes, address).unwrap();
        assert!(
            instructions
                .iter()
                .any(|instruction| instruction.mnemonic == "callq"),
            "{instructions:?}"
        );
        assert_eq!(
            check_function(&bytes, address, &caller, &call_targets, &context),
            FunctionOutcome::Equivalent
        );

        let computed_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define double @callee(double %x) {
entry:
  ret double %x
}
define double @computed(double %x, double %y) {
entry:
  %sum = fadd double %x, %y
  %result = call double @callee(double %sum)
  ret double %result
}
"#;
        let (address, bytes, call_targets) =
            compiled_function_with_targets(computed_ir, "computed");
        let instructions = disassemble(&bytes, address).unwrap();
        assert!(
            instructions
                .iter()
                .any(|instruction| instruction.mnemonic == "addsd"),
            "{instructions:?}"
        );
        assert!(
            instructions
                .iter()
                .any(|instruction| instruction.mnemonic == "callq"),
            "{instructions:?}"
        );
        assert_eq!(
            check_function(&bytes, address, &computed, &call_targets, &context),
            FunctionOutcome::OutsideProfile("call_arguments")
        );

        let wrong_xmm_argument = [
            0x66, 0x0f, 0x28, 0xc1, // movapd %xmm1, %xmm0
            0xe8, 0x00, 0x00, 0x00, 0x00, // callq callee
            0xc3,
        ];
        let mut caller_targets = helper_call_targets(&[5]);
        caller_targets.get_mut(&5).unwrap().name = "callee".to_owned();
        assert_eq!(
            check_function(&wrong_xmm_argument, 0, &caller, &caller_targets, &context,),
            FunctionOutcome::Different
        );

        let mixed_call = [0xe8, 0x00, 0x00, 0x00, 0x00, 0xc3];
        let mut mixed_targets = helper_call_targets(&[1]);
        mixed_targets.get_mut(&1).unwrap().name = "mixed_callee".to_owned();
        assert_eq!(
            check_function(&mixed_call, 0, &mixed_caller, &mixed_targets, &context,),
            FunctionOutcome::Equivalent
        );

        let nan_payload_caller = twice;
        assert_eq!(nan_payload_caller.call_sites.len(), 2);
        assert_eq!(
            nan_payload_caller.call_sites[0].arg_leaf_doubles,
            vec![true]
        );
        assert_eq!(
            nan_payload_caller.call_sites[1].arg_leaf_doubles,
            vec![true]
        );
        let quiets_signaling_nan = [
            0x41, 0x54, // pushq %r12
            0x66, 0x49, 0x0f, 0x7e, 0xc4, // movq %xmm0, %r12
            0xe8, 0x00, 0x00, 0x00, 0x00, // callq callee
            0x4c, 0x89, 0xe0, // movq %r12, %rax
            0x66, 0x48, 0x0f, 0x6e, 0xc0, // movq %rax, %xmm0
            0x48, 0xb8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x80, // movabsq $-0x8000000000000000, %rax
            0x66, 0x48, 0x0f, 0x6e, 0xc8, // movq %rax, %xmm1
            0xf2, 0x0f, 0x58, 0xc1, // addsd %xmm1, %xmm0
            0xe8, 0x00, 0x00, 0x00, 0x00, // callq callee
            0x41, 0x5c, // popq %r12
            0xc3,
        ];
        let exact_plain_x = [
            0x41, 0x54, // pushq %r12
            0x66, 0x49, 0x0f, 0x7e, 0xc4, // movq %xmm0, %r12
            0xe8, 0x00, 0x00, 0x00, 0x00, // callq callee
            0x4c, 0x89, 0xe0, // movq %r12, %rax
            0x66, 0x48, 0x0f, 0x6e, 0xc0, // movq %rax, %xmm0
            0xe8, 0x00, 0x00, 0x00, 0x00, // callq callee
            0x41, 0x5c, // popq %r12
            0xc3,
        ];
        let mut twice_targets = helper_call_targets(&[8, 40]);
        for target in twice_targets.values_mut() {
            target.name = "callee".to_owned();
        }
        assert_eq!(
            check_function(
                &quiets_signaling_nan,
                0,
                &nan_payload_caller,
                &twice_targets,
                &context,
            ),
            FunctionOutcome::OutsideProfile("call_arguments")
        );
        let mut plain_x_targets = helper_call_targets(&[8, 21]);
        for target in plain_x_targets.values_mut() {
            target.name = "callee".to_owned();
        }
        assert_eq!(
            check_function(
                &exact_plain_x,
                0,
                &nan_payload_caller,
                &plain_x_targets,
                &context,
            ),
            FunctionOutcome::Equivalent
        );

        let tail_call = [0xe9, 0x00, 0x00, 0x00, 0x00]; // jmp callee
        let mut tail_targets = helper_call_targets(&[1]);
        tail_targets.get_mut(&1).unwrap().name = "callee".to_owned();
        assert_eq!(
            check_function(&tail_call, 0, &caller, &tail_targets, &context),
            FunctionOutcome::OutsideProfile("call_arguments")
        );

        let result = BV::fresh_const(&context, "many_f64_result", 64);
        let mut too_many_f64 =
            PureI64Body::synthetic_scalar("too_many_f64_call_args", result.clone());
        too_many_f64.result_leaf_doubles = vec![true];
        too_many_f64.call_sites.push(CallSite {
            callee: "helper".to_owned(),
            args: (0..9)
                .map(|index| BV::fresh_const(&context, &format!("f64_arg_{index}"), 64))
                .collect(),
            callback_arguments: Vec::new(),
            arg_leaf_doubles: vec![true; 9],
            argument_shapes: vec![CallValueShape::Scalar { leaf_count: 1 }; 9],
            has_composite_arguments_or_result: false,
            result,
            result_is_double: true,
            result_store: None,
            result_shape: CallValueShape::Scalar { leaf_count: 1 },
            result_leaf_doubles: vec![true],
            guard: Bool::from_bool(&context, true),
        });
        // The ninth `f64` call argument now spills to the stack; a
        // machine that never wrote the slot cannot be equivalent, so
        // the honest failure is `call_arguments` rather than the old
        // `argument_count` admission bound.
        assert_eq!(
            check_function(
                &mixed_call,
                0,
                &too_many_f64,
                &helper_call_targets(&[1]),
                &context,
            ),
            FunctionOutcome::OutsideProfile("call_arguments")
        );
    }

    #[test]
    fn ucomisd_and_comisd_flags_match_ieee_conditions() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let source = "atom eq_branch(x: f64, y: f64) -> i64 requires: true; ensures: true; body: { if x == y { 1 } else { 0 } };\n\
            atom feq(x: f64, y: f64) -> bool requires: true; ensures: true; body: { x == y };\n\
            atom feq_self(x: f64) -> bool requires: true; ensures: true; body: { x == x };\n\
            atom identity(x: i64) -> i64 requires: true; ensures: true; body: { x };";
        let branch_model = source_model_named_with_admission(
            source,
            "eq_branch",
            &context,
            RefinedAdmission::CompositeV19,
        );
        let branch_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @eq_branch(double %x, double %y) {
entry:
  %equal = fcmp oeq double %x, %y
  br i1 %equal, label %yes, label %no
yes:
  ret i64 1
no:
  ret i64 0
}
"#;
        let (address, compiled_bytes, call_targets) =
            compiled_function_with_targets(branch_ir, "eq_branch");
        let instructions = disassemble(&compiled_bytes, address).unwrap();
        let mnemonics = instructions
            .iter()
            .map(|instruction| instruction.mnemonic.as_str())
            .collect::<Vec<_>>();
        assert!(
            mnemonics.contains(&"ucomisd") || mnemonics.contains(&"comisd"),
            "{mnemonics:?}"
        );
        let has_branch_flags = (mnemonics.contains(&"jp") || mnemonics.contains(&"jpe"))
            && (mnemonics.contains(&"jne")
                || mnemonics.contains(&"je")
                || mnemonics.contains(&"jz")
                || mnemonics.contains(&"jnz"));
        let has_set_flags = (mnemonics.contains(&"setnp") || mnemonics.contains(&"setpo"))
            && mnemonics.contains(&"sete")
            && (mnemonics.contains(&"andl") || mnemonics.contains(&"andq"));
        assert!(has_branch_flags || has_set_flags, "{mnemonics:?}");
        assert_eq!(
            check_function(
                &compiled_bytes,
                address,
                &branch_model,
                &call_targets,
                &context,
            ),
            FunctionOutcome::Equivalent,
            "{mnemonics:?}"
        );

        let branch_bytes = [
            0x66, 0x0f, 0x2e, 0xc1, // ucomisd %xmm1, %xmm0
            0x7a, 0x08, // jp to false
            0x75, 0x06, // jne to false
            0xb8, 0x01, 0x00, 0x00, 0x00, // movl $1, %eax
            0xc3, // retq
            0x31, 0xc0, // false: xorl %eax, %eax
            0xc3, // retq
        ];
        assert_eq!(
            check_function(&branch_bytes, 0, &branch_model, &BTreeMap::new(), &context,),
            FunctionOutcome::Equivalent
        );
        let comisd_bytes = [
            0x66, 0x0f, 0x2f, 0xc1, // comisd %xmm1, %xmm0
            0x7a, 0x08, // jp to false
            0x75, 0x06, // jne to false
            0xb8, 0x01, 0x00, 0x00, 0x00, // movl $1, %eax
            0xc3, // retq
            0x31, 0xc0, // false: xorl %eax, %eax
            0xc3, // retq
        ];
        assert_eq!(
            check_function(&comisd_bytes, 0, &branch_model, &BTreeMap::new(), &context,),
            FunctionOutcome::Equivalent
        );
        let mut inverted_jp = branch_bytes;
        inverted_jp[4] = 0x7b; // jnp
        assert_eq!(
            check_function(&inverted_jp, 0, &branch_model, &BTreeMap::new(), &context,),
            FunctionOutcome::Different
        );

        let equality_model = source_model_named_with_admission(
            source,
            "feq",
            &context,
            RefinedAdmission::CompositeV19,
        );
        let set_equal_bytes = [
            0x66, 0x0f, 0x2e, 0xc1, // ucomisd %xmm1, %xmm0
            0x0f, 0x94, 0xc0, // sete %al
            0x0f, 0x9b, 0xc2, // setnp %dl
            0x0f, 0xb6, 0xc0, // movzbl %al, %eax
            0x0f, 0xb6, 0xd2, // movzbl %dl, %edx
            0x21, 0xd0, // andl %edx, %eax
            0xc3,
        ];
        assert_eq!(
            check_function(
                &set_equal_bytes,
                0,
                &equality_model,
                &BTreeMap::new(),
                &context,
            ),
            FunctionOutcome::Equivalent
        );
        let mut inverted_setnp = set_equal_bytes;
        inverted_setnp[8] = 0x9a; // setp
        assert_eq!(
            check_function(
                &inverted_setnp,
                0,
                &equality_model,
                &BTreeMap::new(),
                &context,
            ),
            FunctionOutcome::Different
        );
        let seta_bytes = [
            0x66, 0x0f, 0x2e, 0xc1, // ucomisd %xmm1, %xmm0
            0x0f, 0x97, 0xc0, // seta %al
            0x0f, 0xb6, 0xc0, // movzbl %al, %eax
            0xc3,
        ];
        assert_eq!(
            check_function(&seta_bytes, 0, &equality_model, &BTreeMap::new(), &context,),
            FunctionOutcome::Different
        );

        let self_equality_model = source_model_named_with_admission(
            source,
            "feq_self",
            &context,
            RefinedAdmission::CompositeV19,
        );
        let cmovp_bytes = [
            0x66, 0x0f, 0x2e, 0xc0, // ucomisd %xmm0, %xmm0
            0xb8, 0x01, 0x00, 0x00, 0x00, // movl $1, %eax
            0xb9, 0x00, 0x00, 0x00, 0x00, // movl $0, %ecx
            0x48, 0x0f, 0x4a, 0xc1, // cmovpq %rcx, %rax
            0xc3,
        ];
        assert_eq!(
            check_function(
                &cmovp_bytes,
                0,
                &self_equality_model,
                &BTreeMap::new(),
                &context,
            ),
            FunctionOutcome::Equivalent
        );

        let identity_model = source_model_named_with_admission(
            source,
            "identity",
            &context,
            RefinedAdmission::CompositeV19,
        );
        let integer_compare_parity = [
            0x48, 0x39, 0xf7, // cmpq %rsi, %rdi
            0x0f, 0x9a, 0xc0, // setp %al
            0x0f, 0xb6, 0xc0, // movzbl %al, %eax
            0xc3,
        ];
        assert_eq!(
            check_function(
                &integer_compare_parity,
                0,
                &identity_model,
                &BTreeMap::new(),
                &context,
            ),
            FunctionOutcome::OutsideProfile("unsupported_flags")
        );
    }

    #[test]
    fn compiled_direct_calls_match_uninterpreted_source_sites() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let source = "atom callee(x: i64) -> i64 requires: true; ensures: true; body: { x };\n\
            atom caller(x: i64) -> i64 requires: true; ensures: true; body: { callee(x) };";
        let model = source_model_named(source, "caller", &context);
        assert_eq!(model.call_sites.len(), 1);
        let ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @callee(i64 %x) {
entry:
  ret i64 %x
}
define i64 @caller(i64 %x) {
entry:
  %result = call i64 @callee(i64 %x)
  ret i64 %result
}
"#;
        let (address, bytes, call_targets) = compiled_function_with_targets(ir, "caller");
        assert_eq!(call_targets.len(), 1);
        assert_eq!(
            call_targets
                .values()
                .next()
                .map(|target| target.name.as_str()),
            Some("callee")
        );
        assert_eq!(
            check_function(&bytes, address, &model, &call_targets, &context),
            FunctionOutcome::Equivalent
        );
        let mut unsupported_addend = call_targets.clone();
        unsupported_addend.values_mut().next().unwrap().addend = -3;
        assert_eq!(
            check_function(&bytes, address, &model, &unsupported_addend, &context),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );

        let add_source = "atom callee(x: i64) -> i64 requires: true; ensures: true; body: { x };\n\
            atom caller(x: i64) -> i64 requires: true; ensures: true; body: { callee(x) + 2 };";
        let add_model = source_model_named(add_source, "caller", &context);
        let add_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @callee(i64 %x) {
entry:
  ret i64 %x
}
define i64 @caller(i64 %x) {
entry:
  %result = call i64 @callee(i64 %x)
  %sum = add i64 %result, 2
  ret i64 %sum
}
"#;
        let (address, bytes, call_targets) = compiled_function_with_targets(add_ir, "caller");
        assert_eq!(
            check_function(&bytes, address, &add_model, &call_targets, &context),
            FunctionOutcome::Equivalent
        );

        let changed_argument_source =
            "atom callee(x: i64) -> i64 requires: true; ensures: true; body: { x };\n\
             atom caller(x: i64) -> i64 requires: true; ensures: true; body: { callee(x + 1) };";
        let changed_argument_model =
            source_model_named(changed_argument_source, "caller", &context);
        let (address, bytes, call_targets) = compiled_function_with_targets(ir, "caller");
        assert_eq!(
            check_function(
                &bytes,
                address,
                &changed_argument_model,
                &call_targets,
                &context
            ),
            FunctionOutcome::Different
        );
    }

    #[test]
    fn abstract_stack_models_balanced_push_pop_and_rsp_adjustments() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model(
            "atom helper(x: i64) -> i64 requires: true; ensures: true; body: { x + 1 };",
            &context,
        );
        let bytes = [
            0x48, 0x83, 0xec, 0x08, // subq $8, %rsp
            0x53, // pushq %rbx
            0x48, 0x89, 0xfb, // movq %rdi, %rbx
            0x5b, // popq %rbx
            0x48, 0x83, 0xc4, 0x08, // addq $8, %rsp
            0x48, 0x8d, 0x47, 0x01, // leaq 1(%rdi), %rax
            0xc3,
        ];
        assert_eq!(
            check_function_with_preservation(&bytes, 0, &model, &BTreeMap::new(), &context),
            (
                FunctionOutcome::Equivalent,
                Some(CalleeSavedPreservation::Preserved)
            )
        );
        assert_eq!(
            check_function(&[0x5b, 0xc3], 0, &model, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );
    }

    #[test]
    fn callee_saved_register_clobbers_are_reported_separately_from_equivalence() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model(
            "atom identity(x: i64) -> i64 requires: true; ensures: true; body: { x };",
            &context,
        );
        let clobbers_rbx = [
            0x48, 0x89, 0xfb, // movq %rdi, %rbx
            0x48, 0x89, 0xd8, // movq %rbx, %rax
            0xc3,
        ];
        assert_eq!(
            check_function_with_preservation(&clobbers_rbx, 0, &model, &BTreeMap::new(), &context,),
            (
                FunctionOutcome::Equivalent,
                Some(CalleeSavedPreservation::Violated)
            )
        );

        let pops_saved_rbx_into_rcx = [
            0x53, // pushq %rbx
            0x48, 0x89, 0xfb, // movq %rdi, %rbx
            0x59, // popq %rcx
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0xc3,
        ];
        assert_eq!(
            check_function_with_preservation(
                &pops_saved_rbx_into_rcx,
                0,
                &model,
                &BTreeMap::new(),
                &context,
            ),
            (
                FunctionOutcome::Equivalent,
                Some(CalleeSavedPreservation::Violated)
            )
        );
    }

    #[test]
    fn abstract_stack_rejects_slots_invalidated_by_rsp_increase() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model(
            "atom helper(x: i64) -> i64 requires: true; ensures: true; body: { x };",
            &context,
        );
        let bytes = [
            0x48, 0x83, 0xec, 0x08, // subq $8, %rsp
            0x57, // pushq %rdi
            0x48, 0x83, 0xc4, 0x10, // addq $16, %rsp
            0x48, 0x83, 0xec, 0x10, // subq $16, %rsp
            0x58, // popq %rax
            0xc3,
        ];
        assert_eq!(
            check_function(&bytes, 0, &model, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );
    }

    #[test]
    fn abstract_stack_rejects_rsp_above_function_entry() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model(
            "atom helper(x: i64) -> i64 requires: true; ensures: true; body: { x };",
            &context,
        );
        let bytes = [
            0x48, 0x83, 0xc4, 0x08, // addq $8, %rsp
            0x57, // pushq %rdi
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0xc3,
        ];
        assert_eq!(
            check_function(&bytes, 0, &model, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );
    }

    #[test]
    fn unaligned_rsp_adjustments_reject_overlapping_stack_words() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model(
            "atom helper(x: i64, y: i64) -> i64 requires: true; ensures: true; body: { x };",
            &context,
        );
        let overlapping_words = [
            0x48, 0x83, 0xec, 0x04, // subq $4, %rsp
            0x57, // pushq %rdi; slot starts at entry-relative -12
            0x48, 0x89, 0x74, 0x24, 0x04, // movq %rsi, 4(%rsp); overlaps that slot at -8
            0x58, // popq %rax
            0x48, 0x83, 0xc4, 0x04, // addq $4, %rsp
            0xc3, // retq
        ];
        assert_eq!(
            check_function(&overlapping_words, 0, &model, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );

        assert_eq!(
            check_function(
                &[0x48, 0xff, 0xcc, 0xc3], // decq %rsp; retq
                0,
                &model,
                &BTreeMap::new(),
                &context
            ),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );
    }

    #[test]
    fn bool_identity_uses_its_signature_bounded_input_domain() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model(
            "atom helper(b: bool) -> bool requires: true; ensures: true; body: { b };",
            &context,
        );
        assert_eq!(model.param_domain.len(), 1);
        let (address, bytes) = compiled_function(
            r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @helper(i64 %b) {
entry:
  ret i64 %b
}
"#,
        );
        assert_eq!(
            check_function(&bytes, address, &model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );
    }

    #[test]
    fn loop_model_can_refute_a_non_looping_machine_function() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model(
            "atom helper(x: i64) -> i64 requires: true; ensures: true; body: { let n = x; while n > 0 invariant: n >= 0 decreases: n { n = n - 1 }; n };",
            &context,
        );
        assert!(model.loop_bound_exits.is_some());
        let (address, bytes) = compiled_function(
            r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @helper(i64 %x) {
entry:
  ret i64 %x
}
"#,
        );
        assert_eq!(
            check_function(&bytes, address, &model, &BTreeMap::new(), &context),
            FunctionOutcome::Different
        );
    }

    #[test]
    fn unsupported_signature_cycles_remain_classified_as_cyclic_control_flow() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let mut model = source_model(
            "atom helper() -> i64 requires: true; ensures: true; body: { 7 };",
            &context,
        );
        model.param_shapes = vec![Some(MAX_AGGREGATE_LEAF_COUNT + 1)];
        assert_eq!(
            check_function(&[0x90, 0xeb, 0xfd], 0, &model, &BTreeMap::new(), &context,),
            FunctionOutcome::OutsideProfile("cyclic_control_flow")
        );
    }

    fn loop_count_machine_bytes() -> Vec<u8> {
        vec![
            0x41, 0x56, 0x53, 0x50, 0x49, 0x89, 0xfe, 0x31, 0xdb, 0x0f, 0x1f, 0x80, 0x00, 0x00,
            0x00, 0x00, 0xe8, 0x00, 0x00, 0x00, 0x00, 0x48, 0x85, 0xc0, 0x75, 0x0a, 0x4c, 0x39,
            0xf3, 0x7d, 0x05, 0x48, 0xff, 0xc3, 0xeb, 0xec, 0x48, 0x89, 0xd8, 0x48, 0x83, 0xc4,
            0x08, 0x5b, 0x41, 0x5e, 0xc3,
        ]
    }

    fn poll_call_targets() -> BTreeMap<u64, CallTarget> {
        poll_call_targets_at(0x11)
    }

    fn poll_call_targets_at(field_offset: u64) -> BTreeMap<u64, CallTarget> {
        BTreeMap::from([(
            field_offset,
            CallTarget {
                name: crate::translation_validation::CANCEL_POLL_INTRINSIC.to_owned(),
                relocation_kind: CallRelocationKind::ElfX86_64Plt32,
                relocation_size: 32,
                addend: -4,
            },
        )])
    }

    fn entry_wrapper_call_targets() -> BTreeMap<u64, CallTarget> {
        BTreeMap::from([(
            2,
            CallTarget {
                name: "__mumei_user_main".to_owned(),
                relocation_kind: CallRelocationKind::ElfX86_64Plt32,
                relocation_size: 32,
                addend: -4,
            },
        )])
    }

    #[test]
    fn entry_wrapper_compares_low_32_bits_and_proves_preservation() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let call_targets = entry_wrapper_call_targets();
        let real_wrapper = [0x50, 0xe8, 0, 0, 0, 0, 0x59, 0xc3];
        assert_eq!(
            check_entry_wrapper_with_preservation(&real_wrapper, 0, &call_targets, &context,),
            (
                FunctionOutcome::Equivalent,
                Some(CalleeSavedPreservation::Preserved)
            )
        );

        let increment_result = [0x50, 0xe8, 0, 0, 0, 0, 0x59, 0xff, 0xc0, 0xc3];
        assert_eq!(
            check_entry_wrapper_with_preservation(&increment_result, 0, &call_targets, &context,).0,
            FunctionOutcome::Different
        );

        let zero_extend_result = [0x50, 0xe8, 0, 0, 0, 0, 0x59, 0x89, 0xc0, 0xc3];
        assert_eq!(
            check_entry_wrapper_with_preservation(&zero_extend_result, 0, &call_targets, &context,)
                .0,
            FunctionOutcome::Equivalent
        );

        let clobber_rbx = [0x50, 0xe8, 0, 0, 0, 0, 0x59, 0x48, 0x89, 0xfb, 0xc3];
        assert_eq!(
            check_entry_wrapper_with_preservation(&clobber_rbx, 0, &call_targets, &context,),
            (
                FunctionOutcome::Equivalent,
                Some(CalleeSavedPreservation::Violated)
            )
        );
    }

    const WHILE_PROBE_SOURCE: &str =
        "atom loop_count(n: i64) -> i64 requires: n >= 0 && n <= 10; ensures: true; body: { let i = 0; while i < n invariant: i >= 0 && i <= n decreases: n - i { i = i + 1; }; i };\
         atom main() -> i64 requires: true; ensures: true; body: { loop_count(3) };";

    #[test]
    fn real_loop_count_object_bytes_are_equivalent_within_the_loop_domain() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model_named(WHILE_PROBE_SOURCE, "loop_count", &context);
        assert!(model.loop_bound_exits.is_some());
        assert_eq!(
            check_function_with_preservation(
                &loop_count_machine_bytes(),
                0,
                &model,
                &poll_call_targets(),
                &context,
            ),
            (
                FunctionOutcome::Equivalent,
                Some(CalleeSavedPreservation::Preserved)
            )
        );
    }

    #[test]
    fn extra_machine_loop_iteration_is_different_inside_the_source_domain() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model_named(WHILE_PROBE_SOURCE, "loop_count", &context);
        let mut bytes = loop_count_machine_bytes();
        bytes[0x1d] = 0x7f; // jge -> jg
        assert_eq!(
            check_function(&bytes, 0, &model, &poll_call_targets(), &context),
            FunctionOutcome::Different
        );
    }

    #[test]
    fn machine_loop_exceeding_the_bound_fails_closed() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model_named(WHILE_PROBE_SOURCE, "loop_count", &context);
        let mut bytes = loop_count_machine_bytes();
        // Only n=3 gets an n+2 threshold; all other inputs keep the original threshold and cannot differ first.
        bytes.splice(
            4..16,
            [
                0x4c, 0x8d, 0x77, 0x02, // leaq 2(%rdi), %r14
                0x48, 0x83, 0xff, 0x03, // cmpq $3, %rdi
                0x4c, 0x0f, 0x45, 0xf7, // cmovneq %rdi, %r14
                0x31, 0xdb, // xorl %ebx, %ebx
            ],
        );
        assert_eq!(
            check_function(&bytes, 0, &model, &poll_call_targets_at(0x13), &context),
            FunctionOutcome::OutsideProfile("loop_bound")
        );
    }

    #[test]
    fn non_intrinsic_external_call_without_a_source_site_fails_closed() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model(
            "atom helper(x: i64) -> i64 requires: true; ensures: true; body: { x };",
            &context,
        );
        let targets = BTreeMap::from([(
            1,
            CallTarget {
                name: "not_the_cancel_poll".to_owned(),
                relocation_kind: CallRelocationKind::ElfX86_64Plt32,
                relocation_size: 32,
                addend: -4,
            },
        )]);
        assert_eq!(
            check_function(&[0xe8, 0, 0, 0, 0, 0xc3], 0, &model, &targets, &context),
            FunctionOutcome::OutsideProfile("call_order")
        );
        let poll_tail = BTreeMap::from([(
            1,
            CallTarget {
                name: crate::translation_validation::CANCEL_POLL_INTRINSIC.to_owned(),
                relocation_kind: CallRelocationKind::ElfX86_64Plt32,
                relocation_size: 32,
                addend: -4,
            },
        )]);
        assert_eq!(
            check_function(&[0xe9, 0, 0, 0, 0], 0, &model, &poll_tail, &context),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );
    }

    #[test]
    fn different_leaf_model_is_refuted_by_compiled_helper() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model(
            "atom helper(x: i64) -> i64 requires: true; ensures: result == x + 2; body: { x + 2 };",
            &context,
        );
        let (address, bytes) = compiled_helper();
        assert_eq!(
            check_function(&bytes, address, &model, &BTreeMap::new(), &context),
            FunctionOutcome::Different
        );
    }

    #[test]
    fn unsupported_instruction_is_outside_profile() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model(
            "atom helper(x: i64) -> i64 requires: true; ensures: result == x + 1; body: { x + 1 };",
            &context,
        );
        assert_eq!(
            check_function(&[0x55, 0xc3], 0, &model, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );
    }

    #[test]
    fn probe_compiled_i32_clamp_join_bytes() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model_with_admission(
            "struct Parsed { ok: bool, value: i64 }\n\
             atom incr(current: i64, delta: i64) -> Parsed requires: current >= (0 - 2147483648) && current <= 2147483647 && delta >= (0 - 2147483648) && delta <= 2147483647; ensures: result.ok => (result.value >= (0 - 2147483648) && result.value <= 2147483647); body: { if current + delta > 2147483647 || current + delta < (0 - 2147483648) { Parsed { ok: false, value: 0 } } else { Parsed { ok: true, value: current + delta } } };",
            &context,
            RefinedAdmission::CompositeV26,
        );
        let bytes = [
            0x48, 0x01, 0xf7, // addq %rsi, %rdi
            0xb1, 0x01, // movb $1, %cl
            0x48, 0x81, 0xff, 0xff, 0xff, 0xff, 0x7f, // cmpq $2147483647, %rdi
            0x7f, 0x0a, // jg join
            0x48, 0x81, 0xff, 0x00, 0x00, 0x00, 0x80, // cmpq $-2147483648, %rdi
            0x0f, 0x9c, 0xc1, // setl %cl
            0x31, 0xc0, // xorl %eax, %eax
            0xba, 0x00, 0x00, 0x00, 0x00, // movl $0, %edx
            0x84, 0xc9, // testb %cl, %cl
            0x74, 0x01, // je return false
            0xc3, // retq
            0xb8, 0x01, 0x00, 0x00, 0x00, // movl $1, %eax
            0x48, 0x89, 0xfa, // movq %rdi, %rdx
            0xc3, // retq
        ];
        assert_eq!(
            check_function(&bytes, 0, &model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );

        let mut positive_overflow_not_rejected = bytes;
        positive_overflow_not_rejected[4] = 0;
        assert_eq!(
            check_function(
                &positive_overflow_not_rejected,
                0,
                &model,
                &BTreeMap::new(),
                &context,
            ),
            FunctionOutcome::Different
        );
    }

    #[test]
    fn movb_writes_only_the_low_byte_and_memory_movb_stays_closed() {
        let context = Context::new(&Config::new());
        let mut registers = (0..16)
            .map(|_| BV::fresh_const(&context, "movb_r", 64))
            .collect::<Vec<_>>();
        let mut xmm_registers = (0..16)
            .map(|_| BV::fresh_const(&context, "movb_xmm", 128))
            .collect::<Vec<_>>();
        let before = registers[1].clone();
        let instruction = Instruction {
            offset: 0,
            mnemonic: "movb".to_owned(),
            operands: "$0, %cl".to_owned(),
        };
        assert!(lift_instruction(
            &instruction,
            &mut registers,
            &mut xmm_registers,
            false,
            false,
            &context,
        ));
        let solver = Solver::new(&context);
        let semantics_preserved = Bool::and(
            &context,
            &[
                &registers[1].extract(63, 8)._eq(&before.extract(63, 8)),
                &registers[1]
                    .extract(7, 0)
                    ._eq(&BV::from_u64(&context, 0, 8)),
            ],
        );
        solver.assert(&semantics_preserved.not());
        assert_eq!(solver.check(), SatResult::Unsat);

        let model = source_model(
            "atom helper(x: i64) -> i64 requires: true; ensures: result == x; body: { x };",
            &context,
        );
        assert_eq!(
            check_function(
                &[0xc6, 0x00, 0x01, 0xc3], // movb $1, (%rax); retq
                0,
                &model,
                &BTreeMap::new(),
                &context,
            ),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );

        let clamp = source_model(
            "atom clamp(x: i64) -> i64 requires: true; ensures: result <= 10; body: { if x > 10 { 10 } else { x } };",
            &context,
        );
        let movb_preserves_flags = [
            0xb8, 0x0a, 0x00, 0x00, 0x00, // movl $10, %eax
            0x48, 0x83, 0xff, 0x0a, // cmpq $10, %rdi
            0x88, 0xca, // movb %cl, %dl
            0x7e, 0x01, // jle return x
            0xc3, // retq
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0xc3, // retq
        ];
        assert_eq!(
            check_function(&movb_preserves_flags, 0, &clamp, &BTreeMap::new(), &context,),
            FunctionOutcome::Equivalent
        );
    }

    #[test]
    fn probe_forward_branch_bytes_match_absd_clamp_and_pick() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let source = "atom absd(x: i64, y: i64) -> i64 requires: x >= 0 && x <= 1000 && y >= 0 && y <= 1000; ensures: result >= 0; body: { if x > y { x - y } else { y - x } };\n\
            atom clamp(x: i64) -> i64 requires: true; ensures: result <= 10; body: { if x > 10 { 10 } else { x } };\n\
            atom pick(b: bool, x: i64) -> i64 requires: true; ensures: true; body: { if b { x } else { 0 } };\n\
            atom main() -> i64 requires: true; ensures: result >= 0; body: { absd(7, 3) };";
        let absd = source_model_named(source, "absd", &context);
        let clamp = source_model_named(source, "clamp", &context);
        let pick = source_model_named(source, "pick", &context);
        let absd_bytes = [
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0x29, 0xf0, // subq %rsi, %rax
            0x7e, 0x01, // jle to the else arm
            0xc3, // retq
            0x48, 0x29, 0xfe, // subq %rdi, %rsi
            0x48, 0x89, 0xf0, // movq %rsi, %rax
            0xc3, // retq
        ];
        let clamp_bytes = [
            0xb8, 0x0a, 0x00, 0x00, 0x00, // movl $10, %eax
            0x48, 0x83, 0xff, 0x0a, // cmpq $10, %rdi
            0x7e, 0x01, // jle to the else arm
            0xc3, // retq
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0xc3, // retq
        ];
        let pick_bytes = [
            0x48, 0x89, 0xf0, // movq %rsi, %rax
            0x48, 0x85, 0xff, // testq %rdi, %rdi
            0x74, 0x01, // je to the false arm
            0xc3, // retq
            0x31, 0xc0, // xorl %eax, %eax
            0xc3, // retq
        ];
        let disassembled = disassemble(&absd_bytes, 0x1000).unwrap();
        assert_eq!(disassembled[2].operands, "1");
        for (bytes, model) in [
            (absd_bytes.as_slice(), &absd),
            (clamp_bytes.as_slice(), &clamp),
            (pick_bytes.as_slice(), &pick),
        ] {
            assert_eq!(
                check_function(bytes, 0, model, &BTreeMap::new(), &context),
                FunctionOutcome::Equivalent
            );
        }
        let mut inverted_absd = absd_bytes;
        inverted_absd[6] = 0x7f; // jle -> jg
        assert_eq!(
            check_function(&inverted_absd, 0, &absd, &BTreeMap::new(), &context),
            FunctionOutcome::Different
        );
    }

    #[test]
    fn setcc_byte_registers_and_extend_moves_match_boolean_models() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let lt_model = source_model(
            "atom lt(x: i64, y: i64) -> bool requires: true; ensures: true; body: { x < y };",
            &context,
        );
        let lt_bytes = [
            0x31, 0xc0, // xorl %eax, %eax
            0x48, 0x39, 0xf7, // cmpq %rsi, %rdi
            0x0f, 0x9c, 0xc0, // setl %al
            0xc3, // retq
        ];
        let disassembly = disassemble(&lt_bytes, 0).unwrap();
        assert_eq!(
            disassembly
                .iter()
                .map(|instruction| instruction.mnemonic.as_str())
                .collect::<Vec<_>>(),
            ["xorl", "cmpq", "setl", "retq"]
        );
        assert_eq!(
            check_function(&lt_bytes, 0, &lt_model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );
        let mut inverted = lt_bytes;
        inverted[6] = 0x9d; // setge %al
        assert_eq!(
            check_function(&inverted, 0, &lt_model, &BTreeMap::new(), &context),
            FunctionOutcome::Different
        );

        let upper_bits = [
            0xb8, 0x00, 0x01, 0x00, 0x00, // movl $0x100, %eax
            0x48, 0x39, 0xf7, // cmpq %rsi, %rdi
            0x0f, 0x9c, 0xc0, // setl %al
            0xc3,
        ];
        assert_eq!(
            check_function(&upper_bits, 0, &lt_model, &BTreeMap::new(), &context),
            FunctionOutcome::Different
        );
        let zero_extend_32 = [
            0x48, 0x39, 0xf7, // cmpq %rsi, %rdi
            0x0f, 0x9c, 0xc0, // setl %al
            0x0f, 0xb6, 0xc0, // movzbl %al, %eax
            0xc3,
        ];
        assert_eq!(
            check_function(&zero_extend_32, 0, &lt_model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );
        let zero_extend_64 = [
            0x48, 0x39, 0xf7, // cmpq %rsi, %rdi
            0x0f, 0x9c, 0xc0, // setl %al
            0x48, 0x0f, 0xb6, 0xc0, // movzbq %al, %rax
            0xc3,
        ];
        assert_eq!(
            check_function(&zero_extend_64, 0, &lt_model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );

        let one_model = source_model(
            "atom helper() -> i64 requires: true; ensures: true; body: { 1 };",
            &context,
        );
        let cmpb = [
            0xb8, 0xff, 0x00, 0x00, 0x00, // movl $255, %eax
            0xb9, 0x01, 0x00, 0x00, 0x00, // movl $1, %ecx
            0x38, 0xc1, // cmpb %al, %cl
            0x0f, 0x9f, 0xc2, // setg %dl
            0x0f, 0xb6, 0xc2, // movzbl %dl, %eax
            0xc3,
        ];
        assert_eq!(
            check_function(&cmpb, 0, &one_model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );
        let testb = [
            0xb8, 0xff, 0x00, 0x00, 0x00, // movl $255, %eax
            0x84, 0xc0, // testb %al, %al
            0x0f, 0x98, 0xc1, // sets %cl
            0x0f, 0xb6, 0xc1, // movzbl %cl, %eax
            0xc3,
        ];
        assert_eq!(
            check_function(&testb, 0, &one_model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );

        let positive_model = source_model(
            "atom helper() -> i64 requires: true; ensures: true; body: { 4294967295 };",
            &context,
        );
        let sign_extend_32 = [
            0xb8, 0xff, 0x00, 0x00, 0x00, // movl $255, %eax
            0x0f, 0xbe, 0xc0, // movsbl %al, %eax
            0xc3,
        ];
        assert_eq!(
            check_function(
                &sign_extend_32,
                0,
                &positive_model,
                &BTreeMap::new(),
                &context
            ),
            FunctionOutcome::Equivalent
        );
        let negative_model = source_model(
            "atom helper() -> i64 requires: true; ensures: true; body: { -1 };",
            &context,
        );
        let sign_extend_64 = [
            0xb8, 0xff, 0x00, 0x00, 0x00, // movl $255, %eax
            0x48, 0x0f, 0xbe, 0xc0, // movsbq %al, %rax
            0xc3,
        ];
        assert_eq!(
            check_function(
                &sign_extend_64,
                0,
                &negative_model,
                &BTreeMap::new(),
                &context
            ),
            FunctionOutcome::Equivalent
        );

        let high_byte = [0x0f, 0xb6, 0xc4, 0xc3]; // movzbl %ah, %eax; retq
        assert_eq!(
            check_function(&high_byte, 0, &one_model, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );
        let unknown_flags = [0x0f, 0x9c, 0xc0, 0xc3]; // setl %al; retq
        assert_eq!(
            check_function(&unknown_flags, 0, &lt_model, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("unsupported_flags")
        );
        let set_spl = [
            0x48, 0x39, 0xf7, // cmpq %rsi, %rdi
            0x40, 0x0f, 0x9c, 0xc4, // setl %spl
            0xc3,
        ];
        assert_eq!(
            check_function(&set_spl, 0, &lt_model, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );
    }

    #[test]
    fn immediate_and_implicit_shifts_match_and_validate_widths() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let half_model = source_model(
            "atom half(x: i64) -> i64 requires: x >= 0 && x <= 1000; ensures: result >= 0; body: { x / 2 };",
            &context,
        );
        let half_bytes = [
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0xc1, 0xe8, 0x3f, // shrq $0x3f, %rax
            0x48, 0x01, 0xf8, // addq %rdi, %rax
            0x48, 0xd1, 0xf8, // sarq %rax (implicit count 1)
            0xc3,
        ];
        let half_disassembly = disassemble(&half_bytes, 0).unwrap();
        let sar = half_disassembly
            .iter()
            .find(|instruction| instruction.mnemonic == "sarq")
            .unwrap();
        assert_eq!(sar.operands, "%rax");
        assert_eq!(
            check_function(&half_bytes, 0, &half_model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );
        let mut wrong_count = half_bytes;
        wrong_count[6] = 0x3e;
        assert_eq!(
            check_function(&wrong_count, 0, &half_model, &BTreeMap::new(), &context),
            FunctionOutcome::Different
        );

        let identity_model = source_model(
            "atom helper(x: i64) -> i64 requires: true; ensures: true; body: { x };",
            &context,
        );
        let masked_zero = [
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0xc1, 0xe0, 0x40, // shlq $0x40, %rax (masked count 0)
            0xc3,
        ];
        assert_eq!(
            check_function(&masked_zero, 0, &identity_model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );
        let variable_shift_model = source_model(
            "atom helper(x: i64, n: i64) -> i64 requires: true; ensures: true; body: { x << n };",
            &context,
        );
        let variable_count = [
            0x48, 0x89, 0xf1, // movq %rsi, %rcx
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0xd3, 0xe0, // shlq %cl, %rax
            0xc3,
        ];
        assert_eq!(
            check_function(
                &variable_count,
                0,
                &variable_shift_model,
                &BTreeMap::new(),
                &context
            ),
            FunctionOutcome::Equivalent
        );
        let branch_after_shift = [0x48, 0xc1, 0xe8, 0x01, 0x7f, 0x01, 0xc3, 0xc3];
        assert_eq!(
            check_function(
                &branch_after_shift,
                0,
                &identity_model,
                &BTreeMap::new(),
                &context
            ),
            FunctionOutcome::OutsideProfile("unsupported_flags")
        );

        let shifted_constant_model = source_model(
            "atom helper() -> i64 requires: true; ensures: true; body: { 2147483647 };",
            &context,
        );
        let shift_32 = [
            0xb8, 0xff, 0xff, 0xff, 0xff, // movl $-1, %eax
            0xc1, 0xe8, 0x01, // shrl $1, %eax
            0xc3,
        ];
        assert_eq!(
            check_function(
                &shift_32,
                0,
                &shifted_constant_model,
                &BTreeMap::new(),
                &context
            ),
            FunctionOutcome::Equivalent
        );
        let masked_zero_32_model = source_model(
            "atom helper() -> i64 requires: true; ensures: true; body: { 305419896 };",
            &context,
        );
        let masked_zero_32 = [
            0xb8, 0x78, 0x56, 0x34, 0x12, // movl $0x12345678, %eax
            0xc1, 0xe8, 0x20, // shrl $0x20, %eax (masked count 0)
            0xc3,
        ];
        assert_eq!(
            check_function(
                &masked_zero_32,
                0,
                &masked_zero_32_model,
                &BTreeMap::new(),
                &context
            ),
            FunctionOutcome::Equivalent
        );
        let shl_model = source_model(
            "atom helper() -> i64 requires: true; ensures: true; body: { 4 };",
            &context,
        );
        let shl_32 = [
            0xb8, 0x01, 0x00, 0x00, 0x00, // movl $1, %eax
            0xc1, 0xe0, 0x02, // shll $2, %eax
            0xc3,
        ];
        assert_eq!(
            check_function(&shl_32, 0, &shl_model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );
    }

    #[test]
    fn variable_count_shifts_match_probe_and_distinguish_signed_right_shift() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let shl_model = source_model(
            "atom shl_v(x: i64, n: i64) -> i64 requires: n >= 0 && n <= 62 && x >= 0 && x <= 1000; ensures: true; body: { x << n };",
            &context,
        );
        let shr_pos_model = source_model(
            "atom shr_pos(x: i64, n: i64) -> i64 requires: x >= 0 && x <= 1000 && n >= 0 && n <= 63; ensures: true; body: { x >> n };",
            &context,
        );
        let shr_signed_model = source_model(
            "atom shr_signed(x: i64, n: i64) -> i64 requires: x >= -1000 && x <= 1000 && n >= 0 && n <= 63; ensures: true; body: { x >> n };",
            &context,
        );
        let shl32_model = source_model(
            "atom shl32_v(x: i32, n: i32) -> i32 requires: x >= 0 && x <= 1000 && n >= 0 && n <= 30; ensures: true; body: { x << n };",
            &context,
        );
        let shl_bytes = [
            0x48, 0x89, 0xf1, // movq %rsi, %rcx
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0xd3, 0xe0, // shlq %cl, %rax
            0xc3,
        ];
        let sar_bytes = [
            0x48, 0x89, 0xf1, // movq %rsi, %rcx
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0xd3, 0xf8, // sarq %cl, %rax
            0xc3,
        ];
        for (model, bytes, name) in [
            (&shl_model, &shl_bytes[..], "shl_v"),
            (&shr_pos_model, &sar_bytes[..], "shr_pos"),
            (&shr_signed_model, &sar_bytes[..], "shr_signed"),
            (&shl32_model, &shl_bytes[..], "shl32_v"),
        ] {
            assert_eq!(
                check_function(bytes, 0, model, &BTreeMap::new(), &context),
                FunctionOutcome::Equivalent,
                "{name}"
            );
        }

        let mut logical_right = sar_bytes;
        logical_right[8] = 0xe8; // shrq %cl, %rax
        assert_eq!(
            check_function(
                &logical_right,
                0,
                &shr_signed_model,
                &BTreeMap::new(),
                &context
            ),
            FunctionOutcome::Different
        );

        let constant_32_model = source_model(
            "atom helper() -> i64 requires: true; ensures: true; body: { 4 };",
            &context,
        );
        let shll_bytes = [
            0xb8, 0x02, 0x00, 0x00, 0x00, // movl $2, %eax
            0xb9, 0x01, 0x00, 0x00, 0x00, // movl $1, %ecx
            0xd3, 0xe0, // shll %cl, %eax
            0xc3,
        ];
        assert_eq!(
            check_function(
                &shll_bytes,
                0,
                &constant_32_model,
                &BTreeMap::new(),
                &context
            ),
            FunctionOutcome::Equivalent
        );
    }

    #[test]
    fn variable_shift_flags_follow_count_feasibility() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let shift_result_model = source_model(
            "atom shift_result(x: i64) -> i64 requires: true; ensures: true; body: { if (x << 1) == 0 { 1 } else { 2 } };",
            &context,
        );
        let nonzero_count = [
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0x83, 0xff, 0x00, // cmpq $0, %rdi
            0xb9, 0x01, 0x00, 0x00, 0x00, // movl $1, %ecx
            0x48, 0xd3, 0xe0, // shlq %cl, %rax
            0x74, 0x06, // je to result 1
            0xb8, 0x02, 0x00, 0x00, 0x00, // movl $2, %eax
            0xc3, 0xb8, 0x01, 0x00, 0x00, 0x00, // movl $1, %eax
            0xc3,
        ];
        assert_eq!(
            check_function(
                &nonzero_count,
                0,
                &shift_result_model,
                &BTreeMap::new(),
                &context
            ),
            FunctionOutcome::Equivalent
        );

        let compare_model = source_model(
            "atom compare(x: i64) -> i64 requires: true; ensures: true; body: { if x < 0 { 1 } else { 2 } };",
            &context,
        );
        let zero_count = [
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0x83, 0xff, 0x00, // cmpq $0, %rdi
            0xb9, 0x00, 0x00, 0x00, 0x00, // movl $0, %ecx
            0x48, 0xd3, 0xe0, // shlq %cl, %rax
            0x7c, 0x06, // jl to result 1
            0xb8, 0x02, 0x00, 0x00, 0x00, // movl $2, %eax
            0xc3, 0xb8, 0x01, 0x00, 0x00, 0x00, // movl $1, %eax
            0xc3,
        ];
        assert_eq!(
            check_function(&zero_count, 0, &compare_model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );

        let undecided_model = source_model(
            "atom undecided(x: i64, n: i64) -> i64 requires: true; ensures: true; body: { if (x << n) == 0 { 1 } else { 2 } };",
            &context,
        );
        let undecided_count = [
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0x83, 0xff, 0x00, // cmpq $0, %rdi
            0x48, 0x89, 0xf1, // movq %rsi, %rcx
            0x48, 0xd3, 0xe0, // shlq %cl, %rax
            0x74, 0x06, // je to result 1
            0xb8, 0x02, 0x00, 0x00, 0x00, // movl $2, %eax
            0xc3, 0xb8, 0x01, 0x00, 0x00, 0x00, // movl $1, %eax
            0xc3,
        ];
        assert_eq!(
            check_function(
                &undecided_count,
                0,
                &undecided_model,
                &BTreeMap::new(),
                &context
            ),
            FunctionOutcome::OutsideProfile("unsupported_flags")
        );
    }

    #[test]
    fn or_flags_control_forward_branches_and_refute_inverted_branch() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model(
            "atom helper(x: i64, y: i64) -> i64 requires: true; ensures: true; body: { if x == 0 && y == 0 { 1 } else { 2 } };",
            &context,
        );
        let bytes = [
            0x48, 0x09, 0xf7, // orq %rsi, %rdi
            0x74, 0x06, // je to result 1
            0xb8, 0x02, 0x00, 0x00, 0x00, // movl $2, %eax
            0xc3, 0xb8, 0x01, 0x00, 0x00, 0x00, // movl $1, %eax
            0xc3,
        ];
        assert_eq!(
            check_function(&bytes, 0, &model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );
        let mut inverted = bytes;
        inverted[3] = 0x75; // jne
        assert_eq!(
            check_function(&inverted, 0, &model, &BTreeMap::new(), &context),
            FunctionOutcome::Different
        );
        let orl_model = source_model(
            "atom helper() -> i64 requires: true; ensures: true; body: { 3 };",
            &context,
        );
        let orl = [
            0xb8, 0x01, 0x00, 0x00, 0x00, // movl $1, %eax
            0x83, 0xc8, 0x02, // orl $2, %eax
            0xc3,
        ];
        assert_eq!(
            check_function(&orl, 0, &orl_model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );
    }

    #[test]
    fn forward_and_backward_jumps_are_classified() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model(
            "atom helper() -> i64 requires: true; ensures: true; body: { 7 };",
            &context,
        );
        let forward = [0xeb, 0x01, 0xc3, 0xb8, 0x07, 0x00, 0x00, 0x00, 0xc3];
        assert_eq!(
            check_function(&forward, 0, &model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );
        assert_eq!(
            check_function(&[0x90, 0xeb, 0xfd], 0, &model, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("loop_bound")
        );
        let jump_into_instruction = [0xeb, 0x01, 0x48, 0x89, 0xf8, 0xc3];
        assert_eq!(
            check_function(
                &jump_into_instruction,
                0,
                &model,
                &BTreeMap::new(),
                &context
            ),
            FunctionOutcome::OutsideProfile("branch_target")
        );
    }

    #[test]
    fn probe_acyclic_backward_branch_bytes_match_band() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model(
            "atom band(x: i64, y: i64) -> i64 requires: x >= 0 && x <= 100 && y >= 0 && y <= 100; ensures: true; body: { if x > 5 && y > 5 { x + y } else { 0 } };",
            &context,
        );
        let bytes = [
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0x83, 0xff, 0x06, // cmpq $6, %rdi
            0x7c, 0x0f, // jl to xorl %ecx, %ecx
            0x48, 0x83, 0xfe, 0x06, // cmpq $6, %rsi
            0x0f, 0x9d, 0xc1, // setge %cl
            0x84, 0xc9, // testb %cl, %cl
            0x74, 0x0a, // je to xorl %eax, %eax
            0x48, 0x01, 0xf0, // addq %rsi, %rax
            0xc3, // retq
            0x31, 0xc9, // xorl %ecx, %ecx
            0x84, 0xc9, // testb %cl, %cl
            0x75, 0xf6, // jne backward to addq
            0x31, 0xc0, // xorl %eax, %eax
            0xc3, // retq
        ];
        assert_eq!(
            branch_shape(&bytes, 0, &BTreeMap::new()),
            BranchShape::AcyclicBackward
        );
        assert_eq!(
            check_function(&bytes, 0, &model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );
        let mut inverted = bytes;
        inverted[28] = 0x74; // je to addq, keeping the backward edge
        assert_eq!(
            check_function(&inverted, 0, &model, &BTreeMap::new(), &context),
            FunctionOutcome::Different
        );
    }

    #[test]
    fn conditional_branch_after_unknown_flags_is_outside_profile() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model(
            "atom helper(x: i64) -> i64 requires: true; ensures: true; body: { x + 1 };",
            &context,
        );
        let bytes = [0x48, 0xf7, 0xe7, 0x74, 0x01, 0xc3, 0xc3];
        assert_eq!(
            check_function(&bytes, 0, &model, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("unsupported_flags")
        );
    }

    #[test]
    fn register_cmov_and_32_bit_compare_follow_instruction_width() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let max_model = source_model(
            "atom max(x: i64, y: i64) -> i64 requires: true; ensures: true; body: { if x < y { y } else { x } };",
            &context,
        );
        let max_bytes = [
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0x39, 0xf7, // cmpq %rsi, %rdi
            0x48, 0x0f, 0x4c, 0xc6, // cmovlq %rsi, %rax
            0xc3,
        ];
        assert_eq!(
            check_function(&max_bytes, 0, &max_model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );

        let signed_model = source_model(
            "atom helper(x: i64) -> i64 requires: true; ensures: true; body: { if x < 0 { 1 } else { 0 } };",
            &context,
        );
        let narrow_compare = [
            0xb8, 0x00, 0x00, 0x00, 0x00, // movl $0, %eax
            0x83, 0xff, 0x00, // cmpl $0, %edi
            0x7d, 0x05, // jge to retq
            0xb8, 0x01, 0x00, 0x00, 0x00, // movl $1, %eax
            0xc3,
        ];
        assert_eq!(
            check_function(
                &narrow_compare,
                0,
                &signed_model,
                &BTreeMap::new(),
                &context
            ),
            FunctionOutcome::Different
        );
    }

    #[test]
    fn movabsq_preserves_flags_for_conditional_move() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model(
            "atom min_five(x: i64) -> i64 requires: true; ensures: true; body: { if x < 5 { 5 } else { x } };",
            &context,
        );
        let bytes = [
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0x83, 0xff, 0x05, // cmpq $5, %rdi
            0x48, 0xb9, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // movabsq $5, %rcx
            0x48, 0x0f, 0x4c, 0xc1, // cmovlq %rcx, %rax
            0xc3, // retq
        ];
        assert_eq!(
            check_function(&bytes, 0, &model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );
    }

    #[test]
    fn symbolic_forward_paths_are_bounded() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model(
            "atom helper() -> i64 requires: true; ensures: true; body: { 0 };",
            &context,
        );
        let mut bytes = Vec::new();
        for (rex, modrm) in [
            (0x48, 0xff),
            (0x48, 0xf6),
            (0x48, 0xd2),
            (0x48, 0xc9),
            (0x48, 0xdb),
            (0x4d, 0xc0),
            (0x4d, 0xc9),
        ] {
            bytes.extend([rex, 0x85, modrm, 0x74, 0x01, 0x90]);
        }
        bytes.extend([0xb8, 0x00, 0x00, 0x00, 0x00, 0xc3]);
        assert_eq!(
            check_function(&bytes, 0, &model, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("path_bound")
        );
    }

    #[test]
    fn branching_call_probe_matches_real_bytes_and_rejects_inverted_call_guard() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let source = "atom helper(x: i64) -> i64 requires: x >= 0 && x <= 100; ensures: result >= 1; body: { x + 1 };\n\
            atom cc(x: i64) -> i64 requires: x >= 0 && x <= 100; ensures: true; body: { if x > 5 { helper(x) } else { 0 } };";
        let model = source_model_named(source, "cc", &context);
        let bytes = [
            0x48, 0x83, 0xff, 0x06, // cmpq $6, %rdi
            0x7c, 0x0b, // jl to xorl %eax, %eax
            0x50, // pushq %rax
            0xe8, 0x00, 0x00, 0x00, 0x00, // callq helper
            0x48, 0x83, 0xc4, 0x08, // addq $8, %rsp
            0xc3, // retq
            0x31, 0xc0, // xorl %eax, %eax
            0xc3, // retq
        ];
        let call_targets = helper_call_targets(&[8]);
        assert_eq!(
            check_function(&bytes, 0, &model, &call_targets, &context),
            FunctionOutcome::Equivalent
        );
        let mut inverted = bytes;
        inverted[4] = 0x7d; // jge: the machine call runs where the source guard is false
        assert_eq!(
            check_function(&inverted, 0, &model, &call_targets, &context),
            FunctionOutcome::OutsideProfile("conditional_call")
        );
    }

    #[test]
    fn survey_call_chain_bytes_match_and_many_live_is_equivalent() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);

        let chain_source =
            "atom helper(x: i64) -> i64 requires: true; ensures: true; body: { x + 1 };\n\
            atom chain_two(x: i64) -> i64 requires: true; ensures: true; body: { helper(x) + 1 };";
        let chain_model = source_model_named(chain_source, "chain_two", &context);
        let chain_two_bytes = [
            0x50, // pushq %rax
            0xe8, 0x00, 0x00, 0x00, 0x00, // callq helper
            0x48, 0xff, 0xc0, // incq %rax
            0x59, // popq %rcx
            0xc3, // retq
        ];
        assert_eq!(
            check_function(
                &chain_two_bytes,
                0,
                &chain_model,
                &helper_call_targets(&[2]),
                &context
            ),
            FunctionOutcome::Equivalent
        );

        let many_live_model = source_model(
            "atom many_live(a: i64, b: i64, c: i64, d: i64, e: i64, f: i64) -> i64 \
            requires: true; ensures: true; body: {\n\
                let v0 = a + b;\n\
                let v1 = b + c;\n\
                let v2 = c + d;\n\
                let v3 = d + e;\n\
                let v4 = e + f;\n\
                let v5 = a + f;\n\
                let v6 = a + c + e;\n\
                let v7 = b + d + f;\n\
                if a < b {\n\
                    v0 + v1 + v2 + v3 + v4 + v5 + v6 + v7\n\
                } else {\n\
                    v0 - v1 + v2 - v3 + v4 - v5 + v6 - v7\n\
                }\n\
            };",
            &context,
        );
        let many_live_bytes = [
            0x41, 0x56, // pushq %r14
            0x53, // pushq %rbx
            0x4c, 0x89, 0xc0, // movq %r8, %rax
            0x4c, 0x8d, 0x04, 0x37, // leaq (%rdi,%rsi,1), %r8
            0x4c, 0x8d, 0x34, 0x16, // leaq (%rsi,%rdx,1), %r14
            0x48, 0x8d, 0x1c, 0x01, // leaq (%rcx,%rax,1), %rbx
            0x4e, 0x8d, 0x1c, 0x0f, // leaq (%rdi,%r9,1), %r11
            0x4c, 0x8d, 0x14, 0x17, // leaq (%rdi,%rdx,1), %r10
            0x48, 0x01, 0xca, // addq %rcx, %rdx
            0x49, 0x01, 0xc2, // addq %rax, %r10
            0x4c, 0x01, 0xc8, // addq %r9, %rax
            0x48, 0x01, 0xf1, // addq %rsi, %rcx
            0x4c, 0x01, 0xc9, // addq %r9, %rcx
            0x48, 0x39, 0xf7, // cmpq %rsi, %rdi
            0x7d, 0x17, // jge to the else arm
            0x4d, 0x01, 0xf0, // addq %r14, %r8
            0x48, 0x01, 0xda, // addq %rbx, %rdx
            0x4c, 0x01, 0xc2, // addq %r8, %rdx
            0x4c, 0x01, 0xd8, // addq %r11, %rax
            0x4c, 0x01, 0xd0, // addq %r10, %rax
            0x48, 0x01, 0xd0, // addq %rdx, %rax
            0x48, 0x01, 0xc8, // addq %rcx, %rax
            0xeb, 0x18, // jmp to the join
            0x4d, 0x29, 0xf0, // subq %r14, %r8
            0x49, 0x01, 0xd0, // addq %rdx, %r8
            0x49, 0x29, 0xd8, // subq %rbx, %r8
            0x49, 0x01, 0xc0, // addq %rax, %r8
            0x4d, 0x29, 0xd8, // subq %r11, %r8
            0x4d, 0x01, 0xd0, // addq %r10, %r8
            0x49, 0x29, 0xc8, // subq %rcx, %r8
            0x4c, 0x89, 0xc0, // movq %r8, %rax
            0x5b, // popq %rbx
            0x41, 0x5e, // popq %r14
            0xc3, // retq
        ];
        let many_live_outcome = check_function(
            &many_live_bytes,
            0,
            &many_live_model,
            &BTreeMap::new(),
            &context,
        );
        assert_eq!(many_live_outcome, FunctionOutcome::Equivalent);
    }

    #[test]
    fn power_of_two_division_and_multiplication_are_machine_tv_controls() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);

        let div2_model = source_model(
            "atom div2(x: i64) -> i64 requires: true; ensures: true; body: { x / 2 };",
            &context,
        );
        let div2_bytes = [
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0xc1, 0xe8, 0x3f, // shrq $63, %rax
            0x48, 0x01, 0xf8, // addq %rdi, %rax
            0x48, 0xd1, 0xf8, // sarq %rax
            0xc3, // retq
        ];
        assert_eq!(
            check_function(&div2_bytes, 0, &div2_model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );

        let div4_model = source_model(
            "atom div4(x: i64) -> i64 requires: true; ensures: true; body: { x / 4 };",
            &context,
        );
        let div4_bytes = [
            0x48, 0x8d, 0x47, 0x03, // leaq 3(%rdi), %rax
            0x48, 0x85, 0xff, // testq %rdi, %rdi
            0x48, 0x0f, 0x49, 0xc7, // cmovnsq %rdi, %rax
            0x48, 0xc1, 0xf8, 0x02, // sarq $2, %rax
            0xc3, // retq
        ];
        assert_eq!(
            check_function(&div4_bytes, 0, &div4_model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );

        let multiply_model = source_model(
            "atom multiply(n: i64, pct: i64) -> i64 requires: true; ensures: true; body: { n * pct };",
            &context,
        );
        let multiply_bytes = [
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0x0f, 0xaf, 0xc6, // imulq %rsi, %rax
            0xc3, // retq
        ];
        assert_eq!(
            check_function(
                &multiply_bytes,
                0,
                &multiply_model,
                &BTreeMap::new(),
                &context
            ),
            FunctionOutcome::Equivalent
        );
    }

    #[test]
    fn non_power_of_two_magic_division_is_currently_solver_unknown() {
        // These pin current solver-budget behavior, not a semantic boundary: a
        // future sound canonicalization or solver improvement should move them
        // from Unknown to Equivalent.
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);

        let div3_model = source_model(
            "atom div3(x: i64) -> i64 requires: true; ensures: true; body: { x / 3 };",
            &context,
        );
        let div3_bytes = [
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0xb9, 0x56, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
            0x55, // movabsq magic3, %rcx
            0x48, 0xf7, 0xe9, // imulq %rcx
            0x48, 0x89, 0xd0, // movq %rdx, %rax
            0x48, 0xc1, 0xe8, 0x3f, // shrq $63, %rax
            0x48, 0x01, 0xd0, // addq %rdx, %rax
            0xc3, // retq
        ];
        assert_eq!(
            check_function(&div3_bytes, 0, &div3_model, &BTreeMap::new(), &context),
            FunctionOutcome::Unknown
        );

        // The older hand-written `/7` survey fixture is folded into this
        // characterization test so the magic-division Unknown cases stay in one place.
        let div7_model = source_model(
            "atom div7(x: i64) -> i64 requires: true; ensures: true; body: { x / 7 };",
            &context,
        );
        let div7_bytes = [
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0xb9, 0x25, 0x49, 0x92, 0x24, 0x49, 0x92, 0x24,
            0x49, // movabsq magic7, %rcx
            0x48, 0xf7, 0xe9, // imulq %rcx
            0x48, 0x89, 0xd0, // movq %rdx, %rax
            0x48, 0xc1, 0xe8, 0x3f, // shrq $63, %rax
            0x48, 0xd1, 0xfa, // sarq %rdx
            0x48, 0x01, 0xd0, // addq %rdx, %rax
            0xc3, // retq
        ];
        assert_eq!(
            check_function(&div7_bytes, 0, &div7_model, &BTreeMap::new(), &context),
            FunctionOutcome::Unknown
        );

        let div10_model = source_model(
            "atom div10(x: i64) -> i64 requires: true; ensures: true; body: { x / 10 };",
            &context,
        );
        let div10_bytes = [
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0xb9, 0x67, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
            0x66, // movabsq magic10, %rcx
            0x48, 0xf7, 0xe9, // imulq %rcx
            0x48, 0x89, 0xd0, // movq %rdx, %rax
            0x48, 0xc1, 0xe8, 0x3f, // shrq $63, %rax
            0x48, 0xc1, 0xfa, 0x02, // sarq $2, %rdx
            0x48, 0x01, 0xd0, // addq %rdx, %rax
            0xc3, // retq
        ];
        assert_eq!(
            check_function(&div10_bytes, 0, &div10_model, &BTreeMap::new(), &context),
            FunctionOutcome::Unknown
        );

        let div100_model = source_model(
            "atom div100(x: i64) -> i64 requires: true; ensures: true; body: { x / 100 };",
            &context,
        );
        let div100_bytes = [
            0x48, 0xb9, 0x0b, 0xd7, 0xa3, 0x70, 0x3d, 0x0a, 0xd7,
            0xa3, // movabsq magic100, %rcx
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0xf7, 0xe9, // imulq %rcx
            0x48, 0x8d, 0x04, 0x3a, // leaq (%rdx,%rdi), %rax
            0x48, 0x89, 0xc1, // movq %rax, %rcx
            0x48, 0xc1, 0xe9, 0x3f, // shrq $63, %rcx
            0x48, 0xc1, 0xf8, 0x06, // sarq $6, %rax
            0x48, 0x01, 0xc8, // addq %rcx, %rax
            0xc3, // retq
        ];
        assert_eq!(
            check_function(&div100_bytes, 0, &div100_model, &BTreeMap::new(), &context),
            FunctionOutcome::Unknown
        );

        let plus99_model = source_model(
            "atom plus99(x: i64) -> i64 requires: true; ensures: true; body: { (x + 99) / 100 - 1 };",
            &context,
        );
        let plus99_bytes = [
            0x48, 0x8d, 0x47, 0x63, // leaq 99(%rdi), %rax
            0x48, 0xb9, 0x0b, 0xd7, 0xa3, 0x70, 0x3d, 0x0a, 0xd7,
            0xa3, // movabsq magic100, %rcx
            0x48, 0xf7, 0xe9, // imulq %rcx
            0x48, 0x8d, 0x04, 0x3a, // leaq (%rdx,%rdi), %rax
            0x48, 0x83, 0xc0, 0x63, // addq $99, %rax
            0x48, 0x89, 0xc1, // movq %rax, %rcx
            0x48, 0xc1, 0xe9, 0x3f, // shrq $63, %rcx
            0x48, 0xc1, 0xf8, 0x06, // sarq $6, %rax
            0x48, 0x01, 0xc8, // addq %rcx, %rax
            0x48, 0xff, 0xc8, // decq %rax
            0xc3, // retq
        ];
        assert_eq!(
            check_function(&plus99_bytes, 0, &plus99_model, &BTreeMap::new(), &context),
            FunctionOutcome::Unknown
        );
    }

    #[test]
    fn mutated_magic_division_bytes_are_never_equivalent() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model(
            "atom div100(x: i64) -> i64 requires: true; ensures: true; body: { x / 100 };",
            &context,
        );
        let div100_bytes = [
            0x48, 0xb9, 0x0b, 0xd7, 0xa3, 0x70, 0x3d, 0x0a, 0xd7,
            0xa3, // movabsq magic100, %rcx
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0xf7, 0xe9, // imulq %rcx
            0x48, 0x8d, 0x04, 0x3a, // leaq (%rdx,%rdi), %rax
            0x48, 0x89, 0xc1, // movq %rax, %rcx
            0x48, 0xc1, 0xe9, 0x3f, // shrq $63, %rcx
            0x48, 0xc1, 0xf8, 0x06, // sarq $6, %rax
            0x48, 0x01, 0xc8, // addq %rcx, %rax
            0xc3, // retq
        ];

        let mut wrong_magic = div100_bytes;
        wrong_magic[9] ^= 0x01;
        assert_ne!(
            check_function(&wrong_magic, 0, &model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );

        let mut wrong_shift = div100_bytes;
        wrong_shift[30] = 0x05;
        assert_ne!(
            check_function(&wrong_shift, 0, &model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );
    }

    #[test]
    fn compiled_base_index_lea_matches_sum_model() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model(
            "atom lea_sum(x: i64, y: i64) -> i64 requires: true; ensures: true; body: { x + y };",
            &context,
        );
        let compiled_lea_bytes = [
            0x48, 0x8d, 0x04, 0x37, // leaq (%rdi,%rsi), %rax
            0xc3, // retq
        ];
        assert_eq!(
            check_function(&compiled_lea_bytes, 0, &model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );
    }

    #[test]
    fn inc_dec_flags_and_32_bit_results_are_modeled() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);

        let increment_zero_model = source_model(
            "atom increment_zero(x: i64) -> i64 requires: true; ensures: true; body: { if x == -1 { 1 } else { 2 } };",
            &context,
        );
        let increment_then_je = [
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0xff, 0xc0, // incq %rax
            0x74, 0x06, // je to result 1
            0xb8, 0x02, 0x00, 0x00, 0x00, // movl $2, %eax
            0xc3, 0xb8, 0x01, 0x00, 0x00, 0x00, // movl $1, %eax
            0xc3,
        ];
        assert_eq!(
            check_function(
                &increment_then_je,
                0,
                &increment_zero_model,
                &BTreeMap::new(),
                &context
            ),
            FunctionOutcome::Equivalent
        );

        let increment_then_jl = [
            0x48, 0xff, 0xc7, // incq %rdi
            0x7c, 0x01, // jl
            0xc3, 0xc3,
        ];
        assert_ne!(
            check_function(
                &increment_then_jl,
                0,
                &increment_zero_model,
                &BTreeMap::new(),
                &context
            ),
            FunctionOutcome::OutsideProfile("unsupported_flags")
        );

        let constant_32_model = source_model(
            "atom result() -> i64 requires: true; ensures: true; body: { 4294967295 };",
            &context,
        );
        let increment_32 = [
            0xb8, 0xfe, 0xff, 0xff, 0xff, // movl $0xfffffffe, %eax
            0xff, 0xc0, // incl %eax
            0xc3,
        ];
        let decrement_32 = [
            0xb8, 0x00, 0x00, 0x00, 0x00, // movl $0, %eax
            0xff, 0xc8, // decl %eax
            0xc3,
        ];
        for bytes in [&increment_32[..], &decrement_32[..]] {
            assert_eq!(
                check_function(bytes, 0, &constant_32_model, &BTreeMap::new(), &context),
                FunctionOutcome::Equivalent
            );
        }
    }

    #[test]
    fn decrement_then_jns_uses_exact_sign_and_overflow_flags() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);

        // The division-by-constant idiom: decrement, then branch on the
        // sign of the decremented value. Real hardware dec sets SF/ZF/OF
        // exactly, so the sign condition is liftable.
        let clamp_model = source_model(
            "atom clamp(x: i64) -> i64 requires: true; ensures: true; body: { if x - 1 < 0 { 0 } else { x - 1 } };",
            &context,
        );
        let decrement_then_jns = [
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0xff, 0xc8, // decq %rax
            0x79, 0x02, // jns past the zeroing
            0x31, 0xc0, // xorl %eax, %eax
            0xc3, 0xc3,
        ];
        assert_eq!(
            check_function(
                &decrement_then_jns,
                0,
                &clamp_model,
                &BTreeMap::new(),
                &context
            ),
            FunctionOutcome::Equivalent
        );
    }

    #[test]
    fn increment_then_js_and_jl_use_exact_sign_and_overflow_flags() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);

        // Bounded away from the signed-maximum overflow edge: below the
        // boundary the inc result cannot wrap, so the SF/OF-blended jl and
        // the SF-only js both agree with the source-level signed comparison.
        let sign_model = source_model(
            "atom signstep(x: i64) -> i64 requires: true; ensures: true; body: { if x + 1 < 0 { 1 } else { 0 } };",
            &context,
        );
        // js consumes only SF; the SF/OF blend must agree with the signed
        // comparison semantics across the inc overflow boundary.
        let increment_then_js = [
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0xff, 0xc0, // incq %rax
            0x78, 0x06, // js to result 1
            0xb8, 0x00, 0x00, 0x00, 0x00, // movl $0, %eax
            0xc3, 0xb8, 0x01, 0x00, 0x00, 0x00, // movl $1, %eax
            0xc3,
        ];
        let js_outcome = check_function(
            &increment_then_js,
            0,
            &sign_model,
            &BTreeMap::new(),
            &context,
        );
        assert_ne!(
            js_outcome,
            FunctionOutcome::OutsideProfile("unsupported_flags"),
            "js after inc must be lifted rather than rejected"
        );

        let increment_then_jl = [
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0xff, 0xc0, // incq %rax
            0x7c, 0x06, // jl to result 1
            0xb8, 0x00, 0x00, 0x00, 0x00, // movl $0, %eax
            0xc3, 0xb8, 0x01, 0x00, 0x00, 0x00, // movl $1, %eax
            0xc3,
        ];
        let jl_outcome = check_function(
            &increment_then_jl,
            0,
            &sign_model,
            &BTreeMap::new(),
            &context,
        );
        assert_ne!(
            jl_outcome,
            FunctionOutcome::OutsideProfile("unsupported_flags"),
            "jl after inc must be lifted rather than rejected"
        );
    }

    #[test]
    fn carry_consuming_conditions_after_inc_dec_remain_outside_profile() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model(
            "atom helper(x: i64) -> i64 requires: true; ensures: true; body: { x + 1 };",
            &context,
        );
        // inc/dec never update CF in hardware; a carry-consuming jb must
        // fail closed instead of borrowing a fabricated carry.
        for jump in [0x72u8, 0x73, 0x76, 0x77] {
            let bytes = [
                0x48, 0xff, 0xc8, // decq %rax
                jump, 0x01, // jb/jae/jbe/ja +1
                0xc3, 0xc3,
            ];
            assert_eq!(
                check_function(&bytes, 0, &model, &BTreeMap::new(), &context),
                FunctionOutcome::OutsideProfile("unsupported_flags"),
                "carry-consuming condition {jump:#x} after dec must stay outside the profile"
            );
        }
    }

    #[test]
    fn decrement_then_jns_32_bit_width_is_modeled() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model(
            "atom signstep32(x: i64) -> i64 requires: true; ensures: true; body: { if x - 1 < 0 { 1 } else { 0 } };",
            &context,
        );
        let decrement_32_then_js = [
            0x89, 0xf8, // movl %edi, %eax
            0xff, 0xc8, // decl %eax
            0x78, 0x06, // js to result 1
            0xb8, 0x00, 0x00, 0x00, 0x00, // movl $0, %eax
            0xc3, 0xb8, 0x01, 0x00, 0x00, 0x00, // movl $1, %eax
            0xc3,
        ];
        let outcome = check_function(&decrement_32_then_js, 0, &model, &BTreeMap::new(), &context);
        assert_ne!(
            outcome,
            FunctionOutcome::OutsideProfile("unsupported_flags"),
            "32-bit js after dec must be lifted rather than rejected"
        );
    }
    #[test]
    fn neg_condition_codes_match_subtract_from_zero_including_minimum() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        for (condition, body) in [
            ("b", "if x != 0 { 1 } else { 0 }"),
            ("a", "0"),
            ("l", "if x > 0 { 1 } else { 0 }"),
            ("g", "if x < 0 { 1 } else { 0 }"),
        ] {
            let source = format!(
                "atom neg_result(x: i64) -> i64 requires: true; ensures: true; body: {{ {body} }};"
            );
            let model = source_model(&source, &context);
            let setcc = match condition {
                "b" => 0x92,
                "a" => 0x97,
                "l" => 0x9c,
                "g" => 0x9f,
                _ => unreachable!(),
            };
            let bytes = [
                0x48, 0xf7, 0xdf, // negq %rdi
                0x0f, setcc, 0xc0, // set<cc> %al
                0x0f, 0xb6, 0xc0, // movzbl %al, %eax
                0xc3,
            ];
            assert_eq!(
                check_function(&bytes, 0, &model, &BTreeMap::new(), &context),
                FunctionOutcome::Equivalent,
                "set{condition}"
            );
        }
    }

    #[test]
    fn not_preserves_comparison_flags_and_unary_memory_forms_are_unsupported() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model(
            "atom not_after_compare(x: i64) -> i64 requires: true; ensures: true; body: { if x < 5 { 1 } else { 0 } };",
            &context,
        );
        let not_preserves_flags = [
            0x48, 0x83, 0xff, 0x05, // cmpq $5, %rdi
            0x48, 0xf7, 0xd7, // notq %rdi
            0x0f, 0x9c, 0xc0, // setl %al
            0x0f, 0xb6, 0xc0, // movzbl %al, %eax
            0xc3,
        ];
        assert_eq!(
            check_function(&not_preserves_flags, 0, &model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );

        let constant_model = source_model(
            "atom result() -> i64 requires: true; ensures: true; body: { 0 };",
            &context,
        );
        for bytes in [
            &[0x48, 0xff, 0x07, 0xc3][..], // incq (%rdi)
            &[0x48, 0xff, 0x0f, 0xc3][..], // decq (%rdi)
            &[0x48, 0xf7, 0x1f, 0xc3][..], // negq (%rdi)
            &[0x48, 0xf7, 0x17, 0xc3][..], // notq (%rdi)
        ] {
            assert_eq!(
                check_function(bytes, 0, &constant_model, &BTreeMap::new(), &context),
                FunctionOutcome::OutsideProfile("unsupported_instruction")
            );
        }
    }

    #[test]
    fn lea_accepts_implicit_scales_and_displaced_index_only_forms() {
        let config = Config::new();
        let context = Context::new(&config);
        let registers = (0..16)
            .map(|_| BV::fresh_const(&context, "lea_register", 64))
            .collect::<Vec<_>>();
        let cases = [
            ("(%rdi,%rsi)", registers[7].bvadd(&registers[6])),
            (
                "16(%rdi,%rsi)",
                BV::from_u64(&context, 16, 64)
                    .bvadd(&registers[7])
                    .bvadd(&registers[6]),
            ),
            ("(,%rsi)", registers[6].clone()),
            (
                "(,%rsi,4)",
                registers[6].bvmul(&BV::from_u64(&context, 4, 64)),
            ),
            (
                "8(,%rsi,2)",
                BV::from_u64(&context, 8, 64)
                    .bvadd(&registers[6].bvmul(&BV::from_u64(&context, 2, 64))),
            ),
        ];
        for (operand, expected) in cases {
            let value = lea_value(operand, &registers, &context).unwrap();
            let solver = Solver::new(&context);
            solver.assert(&value._eq(&expected).not());
            assert_eq!(solver.check(), SatResult::Unsat, "{operand}");
        }
        assert!(lea_value("(,%rsi,3)", &registers, &context).is_none());
    }

    #[test]
    fn calls_on_both_branch_arms_match_guarded_sites_and_union_relocations() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let source = "atom helper(x: i64) -> i64 requires: x >= 0 && x <= 100; ensures: result >= 1; body: { x + 1 };\n\
            atom cc_both(x: i64) -> i64 requires: x >= 0 && x <= 100; ensures: result >= 1; body: { if x > 5 { helper(x) } else { helper(5) } };";
        let model = source_model_named(source, "cc_both", &context);
        let bytes = [
            0x50, // pushq %rax
            0x48, 0x83, 0xff, 0x06, // cmpq $6, %rdi
            0x7d, 0x05, // jge to shared call with %rdi
            0xbf, 0x05, 0x00, 0x00, 0x00, // movl $5, %edi
            0xe8, 0x00, 0x00, 0x00, 0x00, // shared callq helper
            0x59, // popq %rcx
            0xc3, // retq
        ];
        assert_eq!(
            check_function(&bytes, 0, &model, &helper_call_targets(&[13]), &context),
            FunctionOutcome::Equivalent
        );

        let separate_calls = [
            0x48, 0x83, 0xff, 0x06, // cmpq $6, %rdi
            0x7c, 0x06, // jl to second call
            0xe8, 0x00, 0x00, 0x00, 0x00, // first callq helper
            0xc3, // retq
            0xbf, 0x05, 0x00, 0x00, 0x00, // movl $5, %edi
            0xe8, 0x00, 0x00, 0x00, 0x00, // second callq helper
            0xc3, // retq
        ];
        assert_eq!(
            check_function(
                &separate_calls,
                0,
                &model,
                &helper_call_targets(&[7, 18]),
                &context
            ),
            FunctionOutcome::Equivalent
        );
    }

    #[test]
    fn call_result_can_feed_a_later_branch() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let source = "atom helper(x: i64) -> i64 requires: x >= 0 && x <= 100; ensures: result >= 1; body: { x + 1 };\n\
            atom cc_result(x: i64) -> i64 requires: x >= 0 && x <= 100; ensures: true; body: { if helper(x) > 5 { 10 } else { 0 } };";
        let model = source_model_named(source, "cc_result", &context);
        let bytes = [
            0x50, // pushq %rax
            0xe8, 0x00, 0x00, 0x00, 0x00, // callq helper
            0x48, 0x89, 0xc1, // movq %rax, %rcx
            0xb8, 0x0a, 0x00, 0x00, 0x00, // movl $10, %eax
            0x48, 0x83, 0xf9, 0x05, // cmpq $5, %rcx
            0x7e, 0x02, // jle to zero result
            0x59, 0xc3, // popq %rcx; retq
            0x31, 0xc0, 0x59, 0xc3, // xorl %eax,%eax; popq %rcx; retq
        ];
        assert_eq!(
            check_function(&bytes, 0, &model, &helper_call_targets(&[2]), &context),
            FunctionOutcome::Equivalent
        );
    }

    #[test]
    fn branch_arm_tail_call_remains_supported_at_function_end() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let source = "atom helper(x: i64) -> i64 requires: x >= 0 && x <= 100; ensures: result >= 1; body: { x + 1 };\n\
            atom cc(x: i64) -> i64 requires: x >= 0 && x <= 100; ensures: true; body: { if x > 5 { helper(x) } else { 0 } };";
        let model = source_model_named(source, "cc", &context);
        let bytes = [
            0x48, 0x83, 0xff, 0x06, // cmpq $6, %rdi
            0x7d, 0x03, // jge to tail jmp helper
            0x31, 0xc0, 0xc3, // xorl %eax,%eax; retq
            0xe9, 0x00, 0x00, 0x00, 0x00, // tail jmp helper
        ];
        assert_eq!(
            check_function(&bytes, 0, &model, &helper_call_targets(&[10]), &context),
            FunctionOutcome::Equivalent
        );
    }

    #[test]
    fn survey_stack_argument_bytes_match_and_invalid_slots_fail_closed() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let seven_source =
            "atom seven_args(a: i64, b: i64, c: i64, d: i64, e: i64, f: i64, g: i64) -> i64 \
            requires: true; ensures: true; body: { a + b + c + d + e + f + g };";
        let seven_model = source_model_named(seven_source, "seven_args", &context);
        let seven_bytes = [
            0x48, 0x01, 0xfe, // addq %rdi, %rsi
            0x48, 0x8d, 0x04, 0x0a, // leaq (%rdx,%rcx), %rax
            0x48, 0x01, 0xf0, // addq %rsi, %rax
            0x4c, 0x01, 0xc0, // addq %r8, %rax
            0x4c, 0x01, 0xc8, // addq %r9, %rax
            0x48, 0x03, 0x44, 0x24, 0x08, // addq 8(%rsp), %rax
            0xc3, // retq
        ];
        assert_eq!(
            check_function(&seven_bytes, 0, &seven_model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );
        let seventh_only_source =
            "atom seventh_only(a: i64, b: i64, c: i64, d: i64, e: i64, f: i64, g: i64) -> i64 \
            requires: true; ensures: true; body: { g };";
        let seventh_only_model = source_model_named(seventh_only_source, "seventh_only", &context);
        assert_eq!(
            check_function(
                &[0x48, 0x8b, 0x44, 0x24, 0x08, 0xc3], // movq 8(%rsp), %rax; retq
                0,
                &seventh_only_model,
                &BTreeMap::new(),
                &context
            ),
            FunctionOutcome::Equivalent
        );

        let mut wrong_slot = seven_bytes;
        wrong_slot[20] = 0x10;
        assert_eq!(
            check_function(&wrong_slot, 0, &seven_model, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );
        assert_eq!(
            check_function(
                &[0x8b, 0x44, 0x24, 0x08, 0xc3], // movl 8(%rsp), %eax; retq
                0,
                &seventh_only_model,
                &BTreeMap::new(),
                &context
            ),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );
        assert_eq!(
            check_function(
                &[0x48, 0x89, 0x44, 0x24, 0x08, 0xc3], // movq %rax, 8(%rsp); retq
                0,
                &seven_model,
                &BTreeMap::new(),
                &context
            ),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );
        assert_eq!(
            check_function(
                &[0x48, 0x89, 0x7c, 0x24, 0xf8, 0xc3], // movq %rdi, -8(%rsp); retq
                0,
                &seventh_only_model,
                &BTreeMap::new(),
                &context
            ),
            FunctionOutcome::Different
        );

        let caller_source = format!(
            "{seven_source}\n\
            atom calls_seven(x: i64) -> i64 requires: true; ensures: true; \
            body: {{ seven_args(x, 2, 3, 4, 5, 6, 7) }};"
        );
        let caller_model = source_model_named(&caller_source, "calls_seven", &context);
        let caller_bytes = [
            0x50, // pushq %rax
            0x48, 0xc7, 0x04, 0x24, 0x07, 0x00, 0x00, 0x00, // movq $7, (%rsp)
            0xbe, 0x02, 0x00, 0x00, 0x00, // movl $2, %esi
            0xba, 0x03, 0x00, 0x00, 0x00, // movl $3, %edx
            0xb9, 0x04, 0x00, 0x00, 0x00, // movl $4, %ecx
            0x41, 0xb8, 0x05, 0x00, 0x00, 0x00, // movl $5, %r8d
            0x41, 0xb9, 0x06, 0x00, 0x00, 0x00, // movl $6, %r9d
            0xe8, 0x00, 0x00, 0x00, 0x00, // callq seven_args
            0x59, // popq %rcx
            0xc3, // retq
        ];
        let call_targets = |field_offset: u64| {
            BTreeMap::from([(
                field_offset,
                CallTarget {
                    name: "seven_args".into(),
                    relocation_kind: CallRelocationKind::ElfX86_64Plt32,
                    relocation_size: 32,
                    addend: -4,
                },
            )])
        };
        assert_eq!(
            check_function(&caller_bytes, 0, &caller_model, &call_targets(37), &context),
            FunctionOutcome::Equivalent
        );

        let mut wrong_outgoing_argument = caller_bytes;
        wrong_outgoing_argument[5] = 0x08;
        assert_eq!(
            check_function(
                &wrong_outgoing_argument,
                0,
                &caller_model,
                &call_targets(37),
                &context
            ),
            FunctionOutcome::Different
        );

        let mut missing_slot = vec![0x48, 0x83, 0xec, 0x08]; // subq $8, %rsp
        missing_slot.extend([0x90; 8]);
        missing_slot.extend_from_slice(&caller_bytes[9..]);
        assert_eq!(
            check_function(&missing_slot, 0, &caller_model, &call_targets(40), &context),
            FunctionOutcome::OutsideProfile("call_arguments")
        );

        let mut read_after_call = caller_bytes.to_vec();
        read_after_call
            .splice(41..41, [0x48, 0x8b, 0x04, 0x24])
            .for_each(drop); // movq (%rsp), %rax
        assert_eq!(
            check_function(
                &read_after_call,
                0,
                &caller_model,
                &call_targets(37),
                &context
            ),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );

        let tail_call = [0xe9, 0x00, 0x00, 0x00, 0x00]; // jmp seven_args
        assert_eq!(
            check_function(&tail_call, 0, &caller_model, &call_targets(1), &context),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );

        let mut too_many_model = source_model(
            "atom too_many() -> i64 requires: true; ensures: true; body: { 0 };",
            &context,
        );
        too_many_model.inputs.extend(
            (0..17)
                .map(|index| BV::fresh_const(&context, &format!("too_many_argument_{index}"), 64)),
        );
        assert_eq!(
            check_function(&[0xc3], 0, &too_many_model, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("argument_count")
        );
    }

    #[test]
    fn stack_argument_binary_memory_operands_match_and_validate() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        // Eight INTEGER arguments leave g and h on the argument stack
        // at slots 8(%rsp) and 0x10(%rsp); LLVM folds either one into
        // the source operand of a binary or flag instruction.
        let signature = |body: &str| {
            format!(
                "atom f(a: i64, b: i64, c: i64, d: i64, e: i64, f: i64, g: i64, h: i64) -> i64 \
                requires: true; ensures: true; body: {{ {body} }};"
            )
        };
        let check = |body: &str, bytes: &[u8]| {
            let model = source_model_named(&signature(body), "f", &context);
            check_function(bytes, 0, &model, &BTreeMap::new(), &context)
        };

        let mov_g = [0x48, 0x8b, 0x44, 0x24, 0x08]; // movq 8(%rsp), %rax
                                                    // { g - h }: subq 0x10(%rsp), %rax reads the h slot directly.
        let mut sub_bytes = mov_g.to_vec();
        sub_bytes.extend([0x48, 0x2b, 0x44, 0x24, 0x10, 0xc3]);
        assert_eq!(check("g - h", &sub_bytes), FunctionOutcome::Equivalent);
        // Reversed subtraction (h - g) must not satisfy the model.
        assert_eq!(check("h - g", &sub_bytes), FunctionOutcome::Different);

        // { g * h }: imulq folds the h slot the same way.
        let mut imul_bytes = mov_g.to_vec();
        imul_bytes.extend([0x48, 0x0f, 0xaf, 0x44, 0x24, 0x10, 0xc3]);
        assert_eq!(check("g * h", &imul_bytes), FunctionOutcome::Equivalent);

        // { g & h } and { g | h }: the bitwise forms carry
        // FlagState::Result like their register counterparts.
        let mut and_bytes = mov_g.to_vec();
        and_bytes.extend([0x48, 0x23, 0x44, 0x24, 0x10, 0xc3]);
        assert_eq!(check("g & h", &and_bytes), FunctionOutcome::Equivalent);
        let mut or_bytes = mov_g.to_vec();
        or_bytes.extend([0x48, 0x0b, 0x44, 0x24, 0x10, 0xc3]);
        assert_eq!(check("g | h", &or_bytes), FunctionOutcome::Equivalent);

        // { if g == h { 1 } else { 0 } }: cmpq 0x10(%rsp), %rcx leaves
        // compare flags the following jne consumes.
        let cmp_bytes = [
            0x48, 0x8b, 0x4c, 0x24, 0x08, // movq 8(%rsp), %rcx
            0xb8, 0x01, 0x00, 0x00, 0x00, // movl $1, %eax
            0x48, 0x3b, 0x4c, 0x24, 0x10, // cmpq 0x10(%rsp), %rcx
            0x75, 0x01, // jne +1
            0xc3, // retq — returns 1 when g == h
            0x31, 0xc0, // xorq %eax, %eax — jne lands here
            0xc3, // retq — returns 0 when g != h
        ];
        assert_eq!(
            check("if g == h { 1 } else { 0 }", &cmp_bytes),
            FunctionOutcome::Equivalent
        );

        // { if g == 7 { 1 } else { 0 } }: `cmpq $7, 8(%rsp)` leaves the
        // spilled g in place and compares it as the destination operand.
        let cmp_imm_bytes = [
            0xb8, 0x01, 0x00, 0x00, 0x00, // movl $1, %eax
            0x48, 0x83, 0x7c, 0x24, 0x08, 0x07, // cmpq $7, 8(%rsp)
            0x75, 0x01, // jne +1
            0xc3, // retq
            0x31, 0xc0, // xorq %eax, %eax
            0xc3, // retq
        ];
        assert_eq!(
            check("if g == 7 { 1 } else { 0 }", &cmp_imm_bytes),
            FunctionOutcome::Equivalent
        );

        // `testq %rcx, 8(%rsp)` reads the g slot as the destination
        // operand; setne consumes the result flags.
        let test_bytes = [
            0x48, 0x8b, 0x4c, 0x24, 0x10, // movq 0x10(%rsp), %rcx
            0x48, 0x85, 0x4c, 0x24, 0x08, // testq %rcx, 8(%rsp)
            0x0f, 0x95, 0xc0, // setne %al
            0x0f, 0xb6, 0xc0, // movzbl %al, %eax
            0xc3, // retq
        ];
        assert_eq!(
            check("if (g & h) != 0 { 1 } else { 0 }", &test_bytes),
            FunctionOutcome::Equivalent
        );

        // A write-back into the incoming argument area stays outside the
        // read-only convention.
        let write_incoming = [
            0x48, 0x01, 0x4c, 0x24, 0x08, // addq %rcx, 8(%rsp)
            0xc3, // retq
        ];
        assert_eq!(
            check("g", &write_incoming),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );
    }

    #[test]
    fn frame_stack_spill_slots_reload_recorded_values() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model_named(
            "atom f(a: i64, b: i64) -> i64 requires: true; ensures: true; body: { a + b };",
            "f",
            &context,
        );
        // subq $8,%rsp; movq %rdi,(%rsp); movq (%rsp),%rax;
        // addq %rsi,%rax; addq $8,%rsp; retq — a register spill and its
        // reload carry the recorded stack value through the frame slot.
        let spill_bytes = [
            0x48, 0x83, 0xec, 0x08, 0x48, 0x89, 0x3c, 0x24, 0x48, 0x8b, 0x04, 0x24, 0x48, 0x01,
            0xf0, 0x48, 0x83, 0xc4, 0x08, 0xc3,
        ];
        assert_eq!(
            check_function(&spill_bytes, 0, &model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );

        // A different model ({ a - b }) must not verify against the
        // same spill bytes.
        let sub_model = source_model_named(
            "atom f(a: i64, b: i64) -> i64 requires: true; ensures: true; body: { a - b };",
            "f",
            &context,
        );
        assert_eq!(
            check_function(&spill_bytes, 0, &sub_model, &BTreeMap::new(), &context),
            FunctionOutcome::Different
        );

        // An unrecorded frame slot stays closed.
        let unwritten = [
            0x48, 0x83, 0xec, 0x08, // subq $8, %rsp
            0x48, 0x8b, 0x04, 0x24, // movq (%rsp), %rax
            0x48, 0x83, 0xc4, 0x08, // addq $8, %rsp
            0xc3, // retq
        ];
        assert_eq!(
            check_function(&unwritten, 0, &model, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );
    }

    #[test]
    fn packed_stack_argument_forwarding_matches_and_validates() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        // Eight INTEGER arguments put s and t on the incoming argument
        // stack; LLVM forwards them to a callee's outgoing area with one
        // `movaps`/`movups` pair — the real tailer codegen.
        let source =
            "atom callee(a: i64, b: i64, c: i64, d: i64, e: i64, f: i64, g: i64, h: i64) -> i64 \
            requires: true; ensures: result == a + b + c + d + e + f + g + h; \
            body: { a + b + c + d + e + f + g + h };\n\
            atom tailer(x: i64, y: i64, z: i64, w: i64, v: i64, u: i64, s: i64, t: i64) -> i64 \
            requires: true; ensures: true; body: { callee(x, y, z, w, v, u, s, t) };\n";
        let call_targets = || {
            BTreeMap::from([(
                14,
                CallTarget {
                    name: "callee".into(),
                    relocation_kind: CallRelocationKind::ElfX86_64Plt32,
                    relocation_size: 32,
                    addend: -4,
                },
            )])
        };
        // subq $0x18,%rsp; movaps 0x20(%rsp),%xmm0; movups %xmm0,(%rsp);
        // callq callee; addq $0x18,%rsp; retq
        let forward_bytes = [
            0x48, 0x83, 0xec, 0x18, 0x0f, 0x28, 0x44, 0x24, 0x20, 0x0f, 0x11, 0x04, 0x24, 0xe8,
            0x00, 0x00, 0x00, 0x00, 0x48, 0x83, 0xc4, 0x18, 0xc3,
        ];
        let model = source_model_named(source, "tailer", &context);
        assert_eq!(
            check_function(&forward_bytes, 0, &model, &call_targets(), &context),
            FunctionOutcome::Equivalent
        );

        // Forwarding the spilled arguments swapped (the vector copy
        // preserves slot order, so callee(u8=t, u9=s) cannot match).
        let swapped_source =
            "atom callee(a: i64, b: i64, c: i64, d: i64, e: i64, f: i64, g: i64, h: i64) -> i64 \
            requires: true; ensures: true; body: { a + b + c + d + e + f + g + h };\n\
            atom tailer(x: i64, y: i64, z: i64, w: i64, v: i64, u: i64, s: i64, t: i64) -> i64 \
            requires: true; ensures: true; body: { callee(x, y, z, w, v, u, t, s) };\n";
        let swapped_model = source_model_named(swapped_source, "tailer", &context);
        assert_eq!(
            check_function(&forward_bytes, 0, &swapped_model, &call_targets(), &context),
            FunctionOutcome::Different
        );

        // A packed load whose high half is not a declared slot stays
        // closed: seven arguments spill only s at 8(%rsp), so a
        // movaps reading 8(%rsp)..0x18(%rsp) has nothing for its top
        // qword.
        let seven_source =
            "atom seven(x: i64, y: i64, z: i64, w: i64, v: i64, u: i64, s: i64) -> i64 \
            requires: true; ensures: true; body: { s };\n";
        let seven_model = source_model_named(seven_source, "seven", &context);
        let wide_load = [0x0f, 0x28, 0x44, 0x24, 0x08, 0xc3]; // movaps 8(%rsp),%xmm0; retq
        assert_eq!(
            check_function(&wide_load, 0, &seven_model, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );

        // A packed store straddling the return-address slot is not a
        // frame write.
        let straddle = [
            0x48, 0x83, 0xec, 0x10, // subq $0x10,%rsp
            0x0f, 0x11, 0x44, 0x24,
            0x08, // movups %xmm0,8(%rsp) — writes +0..+8 relative to entry
            0x48, 0x83, 0xc4, 0x10, // addq $0x10,%rsp
            0xc3,
        ];
        assert_eq!(
            check_function(&straddle, 0, &seven_model, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );
    }

    #[test]
    fn pushed_immediate_outgoing_stack_arguments_match_and_validate() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let eight_source = "atom eight_args(a: i64, b: i64, c: i64, d: i64, e: i64, f: i64, g: i64, h: i64) -> i64 requires: true; ensures: true; body: { a + b + c + d + e + f + g + h };";
        let caller_source = format!(
            "{eight_source}\n\
            atom calls_eight(x: i64) -> i64 requires: true; ensures: true; \
            body: {{ eight_args(x, 2, 3, 4, 5, 6, 7, 8) }};"
        );
        let caller_model = source_model_named(&caller_source, "calls_eight", &context);
        // Two outgoing stack arguments encoded as `pushq $imm`: LLVM picks
        // pushes over `movq $imm, disp(%rsp)` stores when more than one
        // outgoing word is live.
        let caller_bytes = [
            0x50, // pushq %rax
            0xbe, 0x02, 0x00, 0x00, 0x00, // movl $2, %esi
            0xba, 0x03, 0x00, 0x00, 0x00, // movl $3, %edx
            0xb9, 0x04, 0x00, 0x00, 0x00, // movl $4, %ecx
            0x41, 0xb8, 0x05, 0x00, 0x00, 0x00, // movl $5, %r8d
            0x41, 0xb9, 0x06, 0x00, 0x00, 0x00, // movl $6, %r9d
            0x6a, 0x08, // pushq $8
            0x6a, 0x07, // pushq $7
            0xe8, 0x00, 0x00, 0x00, 0x00, // callq eight_args
            0x48, 0x83, 0xc4, 0x10, // addq $16, %rsp
            0x59, // popq %rcx
            0xc3, // retq
        ];
        let call_targets = BTreeMap::from([(
            33u64,
            CallTarget {
                name: "eight_args".into(),
                relocation_kind: CallRelocationKind::ElfX86_64Plt32,
                relocation_size: 32,
                addend: -4,
            },
        )]);
        assert_eq!(
            check_function(&caller_bytes, 0, &caller_model, &call_targets, &context),
            FunctionOutcome::Equivalent
        );

        // A mismatched pushed immediate is a real divergence, not a profile
        // gap: the callee would read the wrong argument word.
        let mut wrong_outgoing_argument = caller_bytes;
        wrong_outgoing_argument[31] = 0x09;
        assert_eq!(
            check_function(
                &wrong_outgoing_argument,
                0,
                &caller_model,
                &call_targets,
                &context
            ),
            FunctionOutcome::Different
        );

        // x86-64 `push imm` sign-extends to 64 bits: a pushed $-1 is the
        // argument value -1, not 0xffffffff.
        let negative_source = format!(
            "{eight_source}\n\
            atom calls_eight_negative(x: i64) -> i64 requires: true; ensures: true; \
            body: {{ eight_args(x, 2, 3, 4, 5, 6, -1, -2) }};"
        );
        let negative_model = source_model_named(&negative_source, "calls_eight_negative", &context);
        let negative_bytes = [
            0x50, // pushq %rax
            0xbe, 0x02, 0x00, 0x00, 0x00, // movl $2, %esi
            0xba, 0x03, 0x00, 0x00, 0x00, // movl $3, %edx
            0xb9, 0x04, 0x00, 0x00, 0x00, // movl $4, %ecx
            0x41, 0xb8, 0x05, 0x00, 0x00, 0x00, // movl $5, %r8d
            0x41, 0xb9, 0x06, 0x00, 0x00, 0x00, // movl $6, %r9d
            0x6a, 0xfe, // pushq $-2
            0x6a, 0xff, // pushq $-1
            0xe8, 0x00, 0x00, 0x00, 0x00, // callq eight_args
            0x48, 0x83, 0xc4, 0x10, // addq $16, %rsp
            0x59, // popq %rcx
            0xc3, // retq
        ];
        assert_eq!(
            check_function(&negative_bytes, 0, &negative_model, &call_targets, &context),
            FunctionOutcome::Equivalent
        );

        // A memory-source push (`pushq disp(%rsp)`) is still outside the
        // profile.
        let memory_push = [
            0x50, // pushq %rax
            0xff, 0x34, 0x24, // pushq (%rsp)
            0xc3, // retq
        ];
        let no_arg_model = source_model_named(
            "atom no_args() -> i64 requires: true; ensures: true; body: { 0 };",
            "no_args",
            &context,
        );
        assert_eq!(
            check_function(&memory_push, 0, &no_arg_model, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );
    }

    #[test]
    fn division_probe_bytes_match_signed_and_unsigned_source_models() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let md_model = source_model(
            "atom md(x: i64, y: i64) -> i64 requires: x >= 0 && x <= 1000 && y >= 1 && y <= 100; ensures: true; body: { x / y };",
            &context,
        );
        let mr_model = source_model(
            "atom mr(x: i64, y: i64) -> i64 requires: x >= 0 && x <= 1000 && y >= 1 && y <= 100; ensures: true; body: { x % y };",
            &context,
        );
        let signed_md_model = source_model(
            "atom signed_md(x: i64, y: i64) -> i64 requires: x >= -1000 && x <= 1000 && ((y >= -100 && y <= -1) || (y >= 1 && y <= 100)); ensures: true; body: { x / y };",
            &context,
        );
        let signed_mr_model = source_model(
            "atom signed_mr(x: i64, y: i64) -> i64 requires: x >= -1000 && x <= 1000 && ((y >= -100 && y <= -1) || (y >= 1 && y <= 100)); ensures: true; body: { x % y };",
            &context,
        );
        let md_bytes = [
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0x89, 0xf9, // movq %rdi, %rcx
            0x48, 0x09, 0xf1, // orq %rsi, %rcx
            0x48, 0xc1, 0xe9, 0x20, // shrq $32, %rcx
            0x74, 0x06, // je to 32-bit fast path
            0x48, 0x99, // cqto
            0x48, 0xf7, 0xfe, // idivq %rsi
            0xc3, // retq
            0x31, 0xd2, // xorl %edx, %edx
            0xf7, 0xf6, // divl %esi
            0xc3, // retq
        ];
        let mr_bytes = [
            0x48, 0x83, 0xfe, 0xff, // cmpq $-1, %rsi
            0xb9, 0x01, 0x00, 0x00, 0x00, // movl $1, %ecx
            0x48, 0x0f, 0x45, 0xce, // cmovneq %rsi, %rcx
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0x89, 0xfa, // movq %rdi, %rdx
            0x48, 0x09, 0xca, // orq %rcx, %rdx
            0x48, 0xc1, 0xea, 0x20, // shrq $32, %rdx
            0x74, 0x09, // je to 32-bit fast path
            0x48, 0x99, // cqto
            0x48, 0xf7, 0xf9, // idivq %rcx
            0x48, 0x89, 0xd0, // movq %rdx, %rax
            0xc3, // retq
            0x31, 0xd2, // xorl %edx, %edx
            0xf7, 0xf1, // divl %ecx
            0x89, 0xd0, // movl %edx, %eax
            0xc3, // retq
        ];
        for (name, bytes, model) in [
            ("md", &md_bytes[..], &md_model),
            ("signed_md", &md_bytes[..], &signed_md_model),
            ("mr", &mr_bytes[..], &mr_model),
            ("signed_mr", &mr_bytes[..], &signed_mr_model),
        ] {
            assert_eq!(
                check_function(bytes, 0, model, &BTreeMap::new(), &context),
                FunctionOutcome::Equivalent,
                "{name}"
            );
        }

        let u32_md_model = source_model(
            "atom md_u32(x: u32, y: u32) -> u32 requires: true; ensures: true; body: { x / y };",
            &context,
        );
        let md_instructions = disassemble(&md_bytes, 0).unwrap();
        let u32_lifted = lift(
            &md_instructions,
            u64::try_from(md_bytes.len()).unwrap(),
            &u32_md_model,
            &BTreeMap::new(),
            &context,
        )
        .unwrap();
        assert_eq!(u32_lifted.paths.len(), 1);
        assert_eq!(
            check_function(&md_bytes, 0, &u32_md_model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );

        let unrestricted_md_model = source_model(
            "atom md_unrestricted(x: i64, y: i64) -> i64 requires: true; ensures: true; body: { x / y };",
            &context,
        );
        let unrestricted_lifted = lift(
            &md_instructions,
            u64::try_from(md_bytes.len()).unwrap(),
            &unrestricted_md_model,
            &BTreeMap::new(),
            &context,
        )
        .unwrap();
        assert_eq!(unrestricted_lifted.paths.len(), 2);
        assert_eq!(
            check_function(
                &md_bytes,
                0,
                &unrestricted_md_model,
                &BTreeMap::new(),
                &context
            ),
            FunctionOutcome::Equivalent
        );

        let constant_model = source_model(
            "atom constant_seven(x: i64) -> i64 requires: x >= -1000 && x <= 1000; ensures: true; body: { x / 7 };",
            &context,
        );
        // This legacy probe intentionally remains weaker than the
        // characterization fixture: it guards that the correct hand-written `/7`
        // sequence is never classified as a mismatch, while solver capability may
        // decide whether equivalence is proved. It is not an input-domain fixture.
        let constant_seven = [
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0xb9, 0x25, 0x49, 0x92, 0x24, 0x49, 0x92, 0x24, 0x49, // movabs magic, %rcx
            0x48, 0xf7, 0xe9, // imulq %rcx
            0x48, 0x89, 0xd0, // movq %rdx, %rax
            0x48, 0xc1, 0xe8, 0x3f, // shrq $63, %rax
            0x48, 0xd1, 0xfa, // sarq %rdx
            0x48, 0x01, 0xd0, // addq %rdx, %rax
            0xc3, // retq
        ];
        assert!(matches!(
            check_function(
                &constant_seven,
                0,
                &constant_model,
                &BTreeMap::new(),
                &context
            ),
            FunctionOutcome::Equivalent | FunctionOutcome::Unknown
        ));
    }

    #[test]
    fn one_operand_imulq_writes_both_product_halves() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model(
            "atom high(x: i64) -> i64 requires: true; ensures: true; body: { x };",
            &context,
        );
        let low_bytes = [
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0xb9, 0x25, 0x49, 0x92, 0x24, 0x49, 0x92, 0x24, 0x49, // movabs magic, %rcx
            0x48, 0xf7, 0xe9, // imulq %rcx
            0xc3, // retq
        ];
        let high_bytes = [
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0xb9, 0x25, 0x49, 0x92, 0x24, 0x49, 0x92, 0x24, 0x49, // movabs magic, %rcx
            0x48, 0xf7, 0xe9, // imulq %rcx
            0x48, 0x89, 0xd0, // movq %rdx, %rax
            0xc3, // retq
        ];
        let multiplier = BV::from_u64(&context, 0x4924_9249_2492_4925, 64);
        for (bytes, high_half) in [(&low_bytes[..], false), (&high_bytes[..], true)] {
            let instructions = disassemble(bytes, 0).unwrap();
            let lifted = lift(
                &instructions,
                u64::try_from(bytes.len()).unwrap(),
                &model,
                &BTreeMap::new(),
                &context,
            )
            .unwrap();
            assert_eq!(lifted.paths.len(), 1);
            let product = lifted.arguments[0]
                .sign_ext(64)
                .bvmul(&multiplier.sign_ext(64));
            let expected = if high_half {
                product.extract(127, 64)
            } else {
                product.extract(63, 0)
            };
            assert_eq!(
                lifted.paths[0].return_values[0]
                    ._eq(&expected)
                    .simplify()
                    .as_bool(),
                Some(true)
            );
        }
    }

    #[test]
    fn division_safety_solver_unknown_is_inconclusive_not_a_trap() {
        assert_eq!(division_safety_outcome(SatResult::Unsat), Ok(()));
        assert_eq!(
            division_safety_outcome(SatResult::Sat),
            Err(FunctionOutcome::OutsideProfile("division_trap"))
        );
        assert_eq!(
            division_safety_outcome(SatResult::Unknown),
            Err(FunctionOutcome::Unknown)
        );
    }

    #[test]
    fn division_remainder_swap_refutes_and_trapping_domains_fail_closed() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model(
            "atom helper(x: i64, y: i64) -> i64 requires: true; ensures: true; body: { x / y };",
            &context,
        );
        let swapped = [
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0x99, // cqto
            0x48, 0xf7, 0xfe, // idivq %rsi
            0x48, 0x89, 0xd0, // movq %rdx, %rax
            0xc3, // retq
        ];
        assert_eq!(
            check_function(&swapped, 0, &model, &BTreeMap::new(), &context),
            FunctionOutcome::Different
        );

        let mut zero_divisor_model = source_model(
            "atom helper(x: i64, y: i64) -> i64 requires: true; ensures: true; body: { x / y };",
            &context,
        );
        zero_divisor_model.definedness.clear();
        assert_eq!(
            check_function(
                &swapped[..8],
                0,
                &zero_divisor_model,
                &BTreeMap::new(),
                &context
            ),
            FunctionOutcome::OutsideProfile("division_trap")
        );

        let mut overflow_model = source_model(
            "atom helper(x: i64, y: i64) -> i64 requires: true; ensures: true; body: { x / y };",
            &context,
        );
        overflow_model.definedness.clear();
        overflow_model.param_domain = vec![
            overflow_model.inputs[0]._eq(&BV::from_i64(&context, i64::MIN, 64)),
            overflow_model.inputs[1]._eq(&BV::from_i64(&context, -1, 64)),
        ];
        assert_eq!(
            check_function(
                &swapped[..8],
                0,
                &overflow_model,
                &BTreeMap::new(),
                &context
            ),
            FunctionOutcome::OutsideProfile("division_trap")
        );
    }

    #[test]
    fn memory_class_struct_result_matches_real_llvm_sret_codegen() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let source = "struct Big { a: i64, b: i64, c: i64, d: i64, e: i64 }\n\
            atom mk_big(x: i64) -> Big requires: true; ensures: true; body: {\n\
              Big { a: x + 1, b: x + 2, c: x + 3, d: x + 4, e: x + 5 }\n\
            };";
        let mk_model = source_model_named_with_admission(
            source,
            "mk_big",
            &context,
            RefinedAdmission::CompositeV23,
        );
        assert_eq!(mk_model.result_shape, Some(5));
        assert!(supports_signature(&mk_model));

        // LLVM lowers the five-leaf result through the SysV `sret`
        // convention: `rdi` carries a hidden result-buffer pointer,
        // the integer argument shifts to `rsi`, each leaf lands via a
        // `movq` store through the pointer, and `rax` returns it.
        let mk_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define { i64, i64, i64, i64, i64 } @mk_big(i64 %x) {
entry:
  %a = add i64 %x, 1
  %b = add i64 %x, 2
  %c = add i64 %x, 3
  %d = add i64 %x, 4
  %e = add i64 %x, 5
  %s0 = insertvalue { i64, i64, i64, i64, i64 } undef, i64 %a, 0
  %s1 = insertvalue { i64, i64, i64, i64, i64 } %s0, i64 %b, 1
  %s2 = insertvalue { i64, i64, i64, i64, i64 } %s1, i64 %c, 2
  %s3 = insertvalue { i64, i64, i64, i64, i64 } %s2, i64 %d, 3
  %s4 = insertvalue { i64, i64, i64, i64, i64 } %s3, i64 %e, 4
  ret { i64, i64, i64, i64, i64 } %s4
}
"#;
        let (mk_address, mk_bytes, mk_targets) = compiled_function_with_targets(mk_ir, "mk_big");
        let mk_disassembly = disassemble(&mk_bytes, mk_address)
            .unwrap()
            .iter()
            .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
            .collect::<Vec<_>>()
            .join("\n");
        println!("mk_big sret disassembly:\n{mk_disassembly}");
        assert_eq!(
            check_function(&mk_bytes, mk_address, &mk_model, &mk_targets, &context),
            FunctionOutcome::Equivalent
        );

        // Four all-INTEGER leaves already cross the register-resident
        // bound — real LLVM lowers a four-`i64` result through sret
        // too.
        let four_source = "struct Four { a: i64, b: i64, c: i64, d: i64 }\n\
            atom mk_four(x: i64) -> Four requires: true; ensures: true; body: {\n\
              Four { a: x + 1, b: x + 2, c: x + 3, d: x + 4 }\n\
            };";
        let four_model = source_model_named_with_admission(
            four_source,
            "mk_four",
            &context,
            RefinedAdmission::CompositeV23,
        );
        assert!(supports_signature(&four_model));
        let four_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define { i64, i64, i64, i64 } @mk_four(i64 %x) {
entry:
  %a = add i64 %x, 1
  %b = add i64 %x, 2
  %c = add i64 %x, 3
  %d = add i64 %x, 4
  %s0 = insertvalue { i64, i64, i64, i64 } undef, i64 %a, 0
  %s1 = insertvalue { i64, i64, i64, i64 } %s0, i64 %b, 1
  %s2 = insertvalue { i64, i64, i64, i64 } %s1, i64 %c, 2
  %s3 = insertvalue { i64, i64, i64, i64 } %s2, i64 %d, 3
  ret { i64, i64, i64, i64 } %s3
}
"#;
        let (four_address, four_bytes, four_targets) =
            compiled_function_with_targets(four_ir, "mk_four");
        assert_eq!(
            check_function(
                &four_bytes,
                four_address,
                &four_model,
                &four_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );
    }

    #[test]
    fn memory_class_struct_result_rejects_wrong_leaves_and_dropped_pointer() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let source = "struct Big { a: i64, b: i64, c: i64, d: i64, e: i64 }\n\
            atom mk_big(x: i64) -> Big requires: true; ensures: true; body: {\n\
              Big { a: x + 1, b: x + 2, c: x + 3, d: x + 4, e: x + 5 }\n\
            };\n\
            atom mk_swapped(x: i64) -> Big requires: true; ensures: true; body: {\n\
              Big { a: x + 5, b: x + 4, c: x + 3, d: x + 2, e: x + 1 }\n\
            };";
        let mk_model = source_model_named_with_admission(
            source,
            "mk_big",
            &context,
            RefinedAdmission::CompositeV23,
        );
        let swapped_model = source_model_named_with_admission(
            source,
            "mk_swapped",
            &context,
            RefinedAdmission::CompositeV23,
        );
        // Correct sret shape with `a` and `e` stored in the wrong
        // buffer slots: the recorded leaves no longer line up.
        let wrong_slot_order = [
            0x48, 0x89, 0xf8, // movq %rdi,%rax
            0x48, 0x8d, 0x4e, 0x01, // leaq 0x1(%rsi),%rcx
            0x48, 0x8d, 0x56, 0x02, // leaq 0x2(%rsi),%rdx
            0x48, 0x8d, 0x7e, 0x03, // leaq 0x3(%rsi),%rdi
            0x4c, 0x8d, 0x46, 0x04, // leaq 0x4(%rsi),%r8
            0x48, 0x83, 0xc6, 0x05, // addq $0x5,%rsi
            0x48, 0x89, 0x70, 0x00, // movq %rsi,(%rax)      — e into slot 0
            0x4c, 0x89, 0x40, 0x18, // movq %r8,0x18(%rax)
            0x48, 0x89, 0x78, 0x10, // movq %rdi,0x10(%rax)
            0x48, 0x89, 0x50, 0x08, // movq %rdx,0x8(%rax)
            0x48, 0x89, 0x48, 0x20, // movq %rcx,0x20(%rax) — a into slot 4
            0xc3, // retq
        ];
        assert_eq!(
            check_function(&wrong_slot_order, 0, &mk_model, &BTreeMap::new(), &context),
            FunctionOutcome::Different
        );
        // A callee that returns the pointer — `movq %rdi,%rax` — but
        // drops the store of leaf `e` leaves buffer slot 4 holding
        // whatever the caller wrote there.
        let dropped_leaf_store = [
            0x48, 0x89, 0xf8, // movq %rdi,%rax
            0x48, 0x8d, 0x4e, 0x01, // leaq 0x1(%rsi),%rcx
            0x48, 0x8d, 0x56, 0x02, // leaq 0x2(%rsi),%rdx
            0x48, 0x8d, 0x7e, 0x03, // leaq 0x3(%rsi),%rdi
            0x4c, 0x8d, 0x46, 0x04, // leaq 0x4(%rsi),%r8
            0x4c, 0x89, 0x40, 0x18, // movq %r8,0x18(%rax)
            0x48, 0x89, 0x78, 0x10, // movq %rdi,0x10(%rax)
            0x48, 0x89, 0x50, 0x08, // movq %rdx,0x8(%rax)
            0x48, 0x89, 0x08, // movq %rcx,(%rax)
            0xc3, // retq
        ];
        assert_eq!(
            check_function(
                &dropped_leaf_store,
                0,
                &mk_model,
                &BTreeMap::new(),
                &context
            ),
            FunctionOutcome::Different
        );
        // Same stores through `r10` carrying the pointer, but `rax`
        // zeroed instead of returning it: the buffer is right, the
        // ABI is not.
        let dropped_pointer_return = [
            0x49, 0x89, 0xfa, // movq %rdi,%r10
            0x48, 0x8d, 0x4e, 0x01, // leaq 0x1(%rsi),%rcx
            0x48, 0x8d, 0x56, 0x02, // leaq 0x2(%rsi),%rdx
            0x48, 0x8d, 0x7e, 0x03, // leaq 0x3(%rsi),%rdi
            0x4c, 0x8d, 0x46, 0x04, // leaq 0x4(%rsi),%r8
            0x48, 0x83, 0xc6, 0x05, // addq $0x5,%rsi
            0x49, 0x89, 0x72, 0x20, // movq %rsi,0x20(%r10)
            0x4d, 0x89, 0x42, 0x18, // movq %r8,0x18(%r10)
            0x49, 0x89, 0x7a, 0x10, // movq %rdi,0x10(%r10)
            0x49, 0x89, 0x52, 0x08, // movq %rdx,0x8(%r10)
            0x49, 0x89, 0x0a, // movq %rcx,(%r10)
            0x48, 0x31, 0xc0, // xorq %rax,%rax
            0xc3, // retq
        ];
        assert_eq!(
            check_function(
                &dropped_pointer_return,
                0,
                &swapped_model,
                &BTreeMap::new(),
                &context
            ),
            FunctionOutcome::Different
        );
    }

    #[test]
    fn memory_class_struct_result_with_f64_leaf_stays_fail_closed() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        // A five-leaf result whose INTEGER leaves still fit the three
        // register slots makes LLVM return the third `f64` leaf
        // through the x87 register — that path stays outside the
        // profile.
        let source = "struct WideF { a: f64, b: f64, c: f64, d: i64, e: i64 }\n\
            atom mk_wide(x: i64, v: f64) -> WideF requires: true; ensures: true; body: {\n\
              WideF { a: v, b: v, c: v, d: x, e: x }\n\
            };";
        let model = source_model_named_with_admission(
            source,
            "mk_wide",
            &context,
            RefinedAdmission::CompositeV23,
        );
        assert_eq!(model.result_shape, Some(5));
        assert!(!supports_signature(&model));
    }

    #[test]
    fn memory_class_call_result_matches_real_llvm_sret_codegen() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let source = "struct Big { a: i64, b: i64, c: i64, d: i64, e: i64 }\n\
            atom mk_big(x: i64) -> Big requires: true; ensures: true; body: {\n\
              Big { a: x + 1, b: x + 2, c: x + 3, d: x + 4, e: x + 5 }\n\
            };\n\
            atom main() -> i64 requires: true; ensures: true; body: { mk_big(5).a };\n\
            atom main_b() -> i64 requires: true; ensures: true; body: { mk_big(5).b };";
        let main_model = source_model_named_with_admission(
            source,
            "main",
            &context,
            RefinedAdmission::CompositeV23,
        );
        let main_b_model = source_model_named_with_admission(
            source,
            "main_b",
            &context,
            RefinedAdmission::CompositeV23,
        );
        assert!(matches!(
            main_model.call_sites[0].result_shape,
            CallValueShape::Struct { leaf_count: 5, .. }
        ));
        // The caller allocates the result buffer on its own stack,
        // hands the pointer to `rdi`, reads the leaves back through
        // `(%rsp)` or `(%rax)`.
        let main_ir = r#"target triple = "x86_64-unknown-linux-gnu"
declare { i64, i64, i64, i64, i64 } @mk_big(i64)
define i64 @__mumei_user_main() {
entry:
  %call = call { i64, i64, i64, i64, i64 } @mk_big(i64 5)
  %leaf = extractvalue { i64, i64, i64, i64, i64 } %call, 0
  ret i64 %leaf
}
"#;
        let (main_address, main_bytes, main_targets) =
            compiled_function_with_targets(main_ir, "__mumei_user_main");
        let main_disassembly = disassemble(&main_bytes, main_address)
            .unwrap()
            .iter()
            .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
            .collect::<Vec<_>>()
            .join("\n");
        println!("sret caller disassembly:\n{main_disassembly}");
        assert_eq!(
            check_function(
                &main_bytes,
                main_address,
                &main_model,
                &main_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );
        // The same machine code returning leaf `a` cannot stand in for
        // a caller that needed leaf `b` — the buffer slot binds the
        // exact leaf.
        assert_eq!(
            check_function(
                &main_bytes,
                main_address,
                &main_b_model,
                &main_targets,
                &context
            ),
            FunctionOutcome::Different
        );
    }

    #[test]
    fn memory_class_call_result_with_f64_leaf_stays_fail_closed() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        // A call result whose third `f64` leaf needs the x87
        // register stays fail-closed — the `sret` model only covers
        // INTEGER overflow, not that convention.
        let source = "struct WideF { a: f64, b: f64, c: f64, d: i64, e: i64 }\n\
            atom mk_wide(x: i64, v: f64) -> WideF requires: true; ensures: true; body: {\n\
              WideF { a: v, b: v, c: v, d: x, e: x }\n\
            };\n\
            atom main(v: f64) -> i64 requires: true; ensures: true; body: { mk_wide(5, v).d };";
        let main_model = source_model_named_with_admission(
            source,
            "main",
            &context,
            RefinedAdmission::CompositeV23,
        );
        assert!(!supports_signature(&main_model));
    }

    #[test]
    fn memory_class_call_result_requires_stack_relative_pointer() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let source = "struct Big { a: i64, b: i64, c: i64, d: i64, e: i64 }\n\
            atom mk_big(x: i64) -> Big requires: true; ensures: true; body: {\n\
              Big { a: x + 1, b: x + 2, c: x + 3, d: x + 4, e: x + 5 }\n\
            };\n\
            atom main() -> i64 requires: true; ensures: true; body: { mk_big(5).a };";
        let main_model = source_model_named_with_admission(
            source,
            "main",
            &context,
            RefinedAdmission::CompositeV23,
        );
        // `rdi` pointing somewhere that does not reduce to a constant
        // `rsp` offset is not a stack buffer the model can place.
        let heap_pointer_caller = [
            0x48, 0xbf, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, // movabsq $0x1000,%rdi
            0xbe, 0x05, 0x00, 0x00, 0x00, // movl $5,%esi
            0xe8, 0x00, 0x00, 0x00, 0x00, // callq mk_big
        ];
        let call_targets = BTreeMap::from([(
            16u64,
            CallTarget {
                name: "mk_big".to_owned(),
                relocation_kind: CallRelocationKind::ElfX86_64Plt32,
                relocation_size: 32,
                addend: -4,
            },
        )]);
        assert_eq!(
            check_function(
                &heap_pointer_caller,
                0,
                &main_model,
                &call_targets,
                &context
            ),
            FunctionOutcome::OutsideProfile("call_arguments")
        );
    }

    #[test]
    fn mixed_class_sret_result_matches_real_llvm_sret_codegen() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        // Four INTEGER leaves overflow the register budget, so the
        // whole aggregate — `f64` leaf included — returns through the
        // `sret` buffer: real LLVM emits `movq` stores for the INTEGER
        // leaves and a `movsd` store for the `f64` leaf.
        let source = "struct BigF { a: i64, b: i64, c: i64, d: i64, e: f64 }\n\
            atom mk_mixed(x: i64, v: f64) -> BigF requires: true; ensures: true; body: {\n\
              BigF { a: x + 1, b: x + 2, c: x + 3, d: x + 4, e: v }\n\
            };\n\
            atom mk_mixed_b(x: i64, v: f64) -> BigF requires: true; ensures: true; body: {\n\
              BigF { a: x + 9, b: x + 2, c: x + 3, d: x + 4, e: v }\n\
            };";
        let model = source_model_named_with_admission(
            source,
            "mk_mixed",
            &context,
            RefinedAdmission::CompositeV23,
        );
        let model_b = source_model_named_with_admission(
            source,
            "mk_mixed_b",
            &context,
            RefinedAdmission::CompositeV23,
        );
        assert!(supports_signature(&model));
        let ir = r#"target triple = "x86_64-unknown-linux-gnu"
define { i64, i64, i64, i64, double } @__mumei_user_main(i64 %x, double %v) {
entry:
  %a = add i64 %x, 1
  %b = add i64 %x, 2
  %c = add i64 %x, 3
  %d = add i64 %x, 4
  %s0 = insertvalue { i64, i64, i64, i64, double } undef, i64 %a, 0
  %s1 = insertvalue { i64, i64, i64, i64, double } %s0, i64 %b, 1
  %s2 = insertvalue { i64, i64, i64, i64, double } %s1, i64 %c, 2
  %s3 = insertvalue { i64, i64, i64, i64, double } %s2, i64 %d, 3
  %s4 = insertvalue { i64, i64, i64, i64, double } %s3, double %v, 4
  ret { i64, i64, i64, i64, double } %s4
}
"#;
        let (address, bytes, targets) = compiled_function_with_targets(ir, "__mumei_user_main");
        let disassembly = disassemble(&bytes, address)
            .unwrap()
            .iter()
            .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
            .collect::<Vec<_>>()
            .join("\n");
        println!("mixed sret callee disassembly:\n{disassembly}");
        assert!(disassembly.contains("movsd"), "expected movsd store");
        assert_eq!(
            check_function(&bytes, address, &model, &targets, &context),
            FunctionOutcome::Equivalent
        );
        assert_eq!(
            check_function(&bytes, address, &model_b, &targets, &context),
            FunctionOutcome::Different
        );
    }

    #[test]
    fn register_class_five_leaf_result_matches_real_llvm_codegen() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        // Three INTEGER and two SSE leaves fit the per-class register
        // budgets — real LLVM returns the five-leaf aggregate in
        // `rax`/`rdx`/`rcx` + `xmm0`/`xmm1`, no `sret` pointer.
        let source = "struct Mix5 { a: i64, b: f64, c: i64, d: f64, e: i64 }\n\
            atom mk_reg(x: i64, v: f64) -> Mix5 requires: true; ensures: true; body: {\n\
              Mix5 { a: x + 1, b: v, c: x + 3, d: v, e: x + 5 }\n\
            };\n\
            atom main(x: i64, v: f64) -> i64 requires: true; ensures: true; body: {\n\
              mk_reg(x, v).e\n\
            };";
        let model = source_model_named_with_admission(
            source,
            "mk_reg",
            &context,
            RefinedAdmission::CompositeV23,
        );
        let main_model = source_model_named_with_admission(
            source,
            "main",
            &context,
            RefinedAdmission::CompositeV23,
        );
        assert!(supports_signature(&model));
        assert!(supports_signature(&main_model));
        let callee_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define { i64, double, i64, double, i64 } @__mumei_user_main(i64 %x, double %v) {
entry:
  %a = add i64 %x, 1
  %c = add i64 %x, 3
  %e = add i64 %x, 5
  %s0 = insertvalue { i64, double, i64, double, i64 } undef, i64 %a, 0
  %s1 = insertvalue { i64, double, i64, double, i64 } %s0, double %v, 1
  %s2 = insertvalue { i64, double, i64, double, i64 } %s1, i64 %c, 2
  %s3 = insertvalue { i64, double, i64, double, i64 } %s2, double %v, 3
  %s4 = insertvalue { i64, double, i64, double, i64 } %s3, i64 %e, 4
  ret { i64, double, i64, double, i64 } %s4
}
"#;
        let (address, bytes, targets) =
            compiled_function_with_targets(callee_ir, "__mumei_user_main");
        assert_eq!(
            check_function(&bytes, address, &model, &targets, &context),
            FunctionOutcome::Equivalent
        );

        // The caller binds all five result leaves back out of the
        // per-class registers — the fifth INTEGER leaf lands in `rcx`.
        let caller_ir = r#"target triple = "x86_64-unknown-linux-gnu"
declare { i64, double, i64, double, i64 } @mk_reg(i64, double)
define i64 @__mumei_user_main(i64 %x, double %v) {
entry:
  %call = call { i64, double, i64, double, i64 } @mk_reg(i64 %x, double %v)
  %leaf = extractvalue { i64, double, i64, double, i64 } %call, 4
  ret i64 %leaf
}
"#;
        let (caller_address, caller_bytes, caller_targets) =
            compiled_function_with_targets(caller_ir, "__mumei_user_main");
        let caller_disassembly = disassemble(&caller_bytes, caller_address)
            .unwrap()
            .iter()
            .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(caller_disassembly.contains("callq"), "{caller_disassembly}");
        assert_eq!(
            check_function(
                &caller_bytes,
                caller_address,
                &main_model,
                &caller_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );
    }

    #[test]
    fn mixed_class_call_result_matches_real_llvm_sret_codegen() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        // The caller reads the `f64` leaf back out of its own `sret`
        // buffer with `movsd N(%rsp),%xmm0`.
        let source = "struct BigF { a: i64, b: i64, c: i64, d: i64, e: f64 }\n\
            atom mk_mixed(x: i64, v: f64) -> BigF requires: true; ensures: true; body: {\n\
              BigF { a: x, b: x, c: x, d: x, e: v }\n\
            };\n\
            atom main(v: f64) -> f64 requires: true; ensures: true; body: { mk_mixed(5, v).e };\n\
            atom main_d(v: f64) -> i64 requires: true; ensures: true; body: { mk_mixed(5, v).d };";
        let main_model = source_model_named_with_admission(
            source,
            "main",
            &context,
            RefinedAdmission::CompositeV23,
        );
        let main_d_model = source_model_named_with_admission(
            source,
            "main_d",
            &context,
            RefinedAdmission::CompositeV23,
        );
        let main_ir = r#"target triple = "x86_64-unknown-linux-gnu"
declare { i64, i64, i64, i64, double } @mk_mixed(i64, double)
define double @__mumei_user_main(double %v) {
entry:
  %call = call { i64, i64, i64, i64, double } @mk_mixed(i64 5, double %v)
  %leaf = extractvalue { i64, i64, i64, i64, double } %call, 4
  ret double %leaf
}
"#;
        let (main_address, main_bytes, main_targets) =
            compiled_function_with_targets(main_ir, "__mumei_user_main");
        let main_disassembly = disassemble(&main_bytes, main_address)
            .unwrap()
            .iter()
            .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
            .collect::<Vec<_>>()
            .join("\n");
        println!("mixed sret caller disassembly:\n{main_disassembly}");
        assert_eq!(
            check_function(
                &main_bytes,
                main_address,
                &main_model,
                &main_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );
        assert_eq!(
            check_function(
                &main_bytes,
                main_address,
                &main_d_model,
                &main_targets,
                &context
            ),
            FunctionOutcome::Different
        );
    }

    #[test]
    fn enum_sret_call_result_matches_real_llvm_codegen() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        // A five-leaf enum (tag + four payload leaves) overflows the
        // INTEGER register budget, so both directions go through the
        // `sret` buffer — and LLVM vectorizes the buffer copies with
        // `movups`.
        let source = "enum Wide { V(a: i64, b: i64, c: i64, d: i64), N }\n\
            atom mk_wide(x: i64) -> Wide requires: true; ensures: true; body: {\n\
              Wide::V(x, x + 1, x + 2, x + 3)\n\
            };\n\
            atom wrap(x: i64) -> Wide requires: true; ensures: true; body: { mk_wide(x) };";
        let callee_model = source_model_named_with_admission(
            source,
            "mk_wide",
            &context,
            RefinedAdmission::CompositeV23,
        );
        let wrap_model = source_model_named_with_admission(
            source,
            "wrap",
            &context,
            RefinedAdmission::CompositeV23,
        );
        assert!(supports_signature(&callee_model));
        assert!(supports_signature(&wrap_model));

        // The callee writes tag + payload leaves into the `sret`
        // buffer — real LLVM emits two `movups` stores plus one
        // `movq` for the odd leaf.
        let callee_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define void @__mumei_user_main(ptr sret({ i64, i64, i64, i64, i64 }) %out, i64 %x) {
entry:
  %p1 = add i64 %x, 1
  %p2 = add i64 %x, 2
  %p3 = add i64 %x, 3
  %s0 = insertvalue { i64, i64, i64, i64, i64 } undef, i64 0, 0
  %s1 = insertvalue { i64, i64, i64, i64, i64 } %s0, i64 %x, 1
  %s2 = insertvalue { i64, i64, i64, i64, i64 } %s1, i64 %p1, 2
  %s3 = insertvalue { i64, i64, i64, i64, i64 } %s2, i64 %p2, 3
  %s4 = insertvalue { i64, i64, i64, i64, i64 } %s3, i64 %p3, 4
  store { i64, i64, i64, i64, i64 } %s4, ptr %out
  ret void
}
"#;
        let (address, bytes, targets) =
            compiled_function_with_targets(callee_ir, "__mumei_user_main");
        let disassembly = disassemble(&bytes, address)
            .unwrap()
            .iter()
            .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
            .collect::<Vec<_>>()
            .join("\n");
        println!("enum sret callee disassembly:\n{disassembly}");
        assert_eq!(
            check_function(&bytes, address, &callee_model, &targets, &context),
            FunctionOutcome::Equivalent
        );

        // The caller re-returns the call result: buffer into xmm pairs
        // via `movups` loads off `rsp`, then `movups` stores through the
        // callee's own `sret` pointer held in a register.
        let wrap_ir = r#"target triple = "x86_64-unknown-linux-gnu"
declare void @mk_wide(ptr sret({ i64, i64, i64, i64, i64 }), i64)
define void @__mumei_user_main(ptr sret({ i64, i64, i64, i64, i64 }) %out, i64 %x) {
entry:
  %buf = alloca { i64, i64, i64, i64, i64 }
  call void @mk_wide(ptr %buf, i64 %x)
  %v = load { i64, i64, i64, i64, i64 }, ptr %buf
  store { i64, i64, i64, i64, i64 } %v, ptr %out
  ret void
}
"#;
        let (wrap_address, wrap_bytes, wrap_targets) =
            compiled_function_with_targets(wrap_ir, "__mumei_user_main");
        let wrap_disassembly = disassemble(&wrap_bytes, wrap_address)
            .unwrap()
            .iter()
            .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
            .collect::<Vec<_>>()
            .join("\n");
        println!("enum sret forwarding disassembly:\n{wrap_disassembly}");
        assert!(wrap_disassembly.contains("callq"), "{wrap_disassembly}");
        assert_eq!(
            check_function(
                &wrap_bytes,
                wrap_address,
                &wrap_model,
                &wrap_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );
    }

    #[test]
    fn enum_sret_f64_leaf_matches_real_llvm_codegen() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        // Four INTEGER leaves overflow the budget even though only one
        // SSE leaf is present, so the mixed-class enum result goes
        // through `sret` — the `f64` payload as a `movsd` bit pattern.
        let source = "enum EW { V(a: i64, b: i64, c: i64, x: f64), N }\n\
            atom mk5(x: i64, v: f64) -> EW requires: true; ensures: true; body: {\n\
              EW::V(x, x + 1, x + 2, v)\n\
            };\n\
            atom wrap5(x: i64, v: f64) -> EW requires: true; ensures: true; body: { mk5(x, v) };";
        let callee_model = source_model_named_with_admission(
            source,
            "mk5",
            &context,
            RefinedAdmission::CompositeV23,
        );
        let wrap_model = source_model_named_with_admission(
            source,
            "wrap5",
            &context,
            RefinedAdmission::CompositeV23,
        );
        assert!(callee_model.has_f64_features && wrap_model.has_f64_features);
        assert!(supports_signature(&callee_model));
        assert!(supports_signature(&wrap_model));

        // The callee's buffer writes: three `movq`, one `movsd`, and
        // the `movq $0` tag — real LLVM's layout for the shape.
        let callee_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define void @__mumei_user_main(ptr sret({ i64, i64, i64, i64, double }) %out, i64 %x, double %v) {
entry:
  %p1 = add i64 %x, 1
  %p2 = add i64 %x, 2
  %s0 = insertvalue { i64, i64, i64, i64, double } undef, i64 0, 0
  %s1 = insertvalue { i64, i64, i64, i64, double } %s0, i64 %x, 1
  %s2 = insertvalue { i64, i64, i64, i64, double } %s1, i64 %p1, 2
  %s3 = insertvalue { i64, i64, i64, i64, double } %s2, i64 %p2, 3
  %s4 = insertvalue { i64, i64, i64, i64, double } %s3, double %v, 4
  store { i64, i64, i64, i64, double } %s4, ptr %out
  ret void
}
"#;
        let (address, bytes, targets) =
            compiled_function_with_targets(callee_ir, "__mumei_user_main");
        assert_eq!(
            check_function(&bytes, address, &callee_model, &targets, &context),
            FunctionOutcome::Equivalent
        );

        // The caller forwards via `movups` pairs plus `movsd` for the
        // `f64` leaf, through its register-held `sret` pointer.
        let wrap_ir = r#"target triple = "x86_64-unknown-linux-gnu"
declare void @mk5(ptr sret({ i64, i64, i64, i64, double }), i64, double)
define void @__mumei_user_main(ptr sret({ i64, i64, i64, i64, double }) %out, i64 %x, double %v) {
entry:
  %buf = alloca { i64, i64, i64, i64, double }
  call void @mk5(ptr %buf, i64 %x, double %v)
  %r = load { i64, i64, i64, i64, double }, ptr %buf
  store { i64, i64, i64, i64, double } %r, ptr %out
  ret void
}
"#;
        let (wrap_address, wrap_bytes, wrap_targets) =
            compiled_function_with_targets(wrap_ir, "__mumei_user_main");
        let wrap_disassembly = disassemble(&wrap_bytes, wrap_address)
            .unwrap()
            .iter()
            .map(|instruction| format!("{} {}", instruction.mnemonic, instruction.operands))
            .collect::<Vec<_>>()
            .join("\n");
        println!("enum f64 sret forwarding disassembly:\n{wrap_disassembly}");
        assert_eq!(
            check_function(
                &wrap_bytes,
                wrap_address,
                &wrap_model,
                &wrap_targets,
                &context
            ),
            FunctionOutcome::Equivalent
        );
    }

    #[test]
    fn ti3_libcall_results_match_their_wide_operator() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let source = "atom udiv128(a: u128, b: u128) -> u128 requires: b != 0; ensures: true; body: { a / b };\n\
            atom urem128(a: u128, b: u128) -> u128 requires: b != 0; ensures: true; body: { a % b };";
        let libcall = |name: &str| {
            BTreeMap::from([(
                1u64,
                CallTarget {
                    name: name.to_owned(),
                    relocation_kind: CallRelocationKind::ElfX86_64Plt32,
                    relocation_size: 32,
                    addend: -4,
                },
            )])
        };
        // `callq <target>` + `retq`: the lifted result must equal the
        // modeled 128-bit operator over the SysV register pairs. A
        // single helper pair is checked — each equivalence spends
        // solver budget on a 128-bit division query.
        let call_ret = [0xe8, 0x00, 0x00, 0x00, 0x00, 0xc3];
        let udiv_model = source_model_named_with_admission(
            source,
            "udiv128",
            &context,
            RefinedAdmission::CompositeV21,
        );
        assert!(udiv_model.has_wide_scalar_features);
        assert_eq!(
            check_function(&call_ret, 0, &udiv_model, &libcall("__udivti3"), &context),
            FunctionOutcome::Equivalent
        );
        // Without wide scalar features the helpers stay fail-closed.
        let narrow = source_model_named_with_admission(
            "atom id(x: i64) -> i64 requires: true; ensures: true; body: { x };",
            "id",
            &context,
            RefinedAdmission::CompositeV21,
        );
        assert!(!narrow.has_wide_scalar_features);
        assert_eq!(
            check_function(&call_ret, 0, &narrow, &libcall("__udivti3"), &context),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );
    }

    #[test]
    fn ti3_name_colliding_with_a_source_callee_uses_call_site_semantics() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let source = "atom __udivti3(a: i64, b: i64) -> i64 requires: true; ensures: true; body: { 0 };\n\
            atom caller(x: i64, y: i64) -> i64 requires: true; ensures: true; body: { __udivti3(x, y) };";
        let model = source_model_named(source, "caller", &context);
        assert_eq!(model.call_sites.len(), 1);
        // The object calls the user-defined `__udivti3`, not the
        // compiler-rt helper: the call must consume the source call
        // site instead of applying 128-bit division register-pair
        // semantics, otherwise the lifted state would diverge from
        // the site's result and reject a correct function.
        let call_ret = [0xe8, 0x00, 0x00, 0x00, 0x00, 0xc3];
        let target = BTreeMap::from([(
            1u64,
            CallTarget {
                name: "__udivti3".to_owned(),
                relocation_kind: CallRelocationKind::ElfX86_64Plt32,
                relocation_size: 32,
                addend: -4,
            },
        )]);
        assert_eq!(
            check_function(&call_ret, 0, &model, &target, &context),
            FunctionOutcome::Equivalent
        );
    }
}
