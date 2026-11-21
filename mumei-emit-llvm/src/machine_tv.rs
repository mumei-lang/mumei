use inkwell::targets::{InitializationConfig, Target};
use llvm_sys::disassembler::{
    LLVMCreateDisasm, LLVMDisasmContextRef, LLVMDisasmDispose, LLVMDisasmInstruction,
};
use mumei_core::semantic_contract::{CallSite, CallValueShape, PureI64Body, LOOP_BOUND};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{CStr, CString},
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
        let mut output = [0i8; 256];
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
        arg_leaf_doubles: Vec::new(),
        argument_shapes: Vec::new(),
        has_composite_arguments_or_result: false,
        result: value,
        result_is_double: false,
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
) -> (
    FunctionOutcome,
    Option<CalleeSavedPreservation>,
    FrameMemoryUse,
) {
    if model.inputs.len() > MAX_ARGUMENT_COUNT
        || model
            .param_leaf_doubles
            .iter()
            .flatten()
            .filter(|is_double| **is_double)
            .count()
            > FLOAT_ARGUMENT_REGISTER_COUNT
    {
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
    let mut unknown_path = false;
    let mut unsupported_call_arguments = false;
    for path in &lifted.paths {
        let result = check_query_with_simplify_fallback(context, |solver| {
            assert_path_equivalence_query(
                solver,
                model,
                &lifted.arguments,
                path,
                result_width,
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
                    (1..=MAX_AGGREGATE_LEAF_COUNT).contains(count) && leaves.len() == *count
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
            (1..=MAX_AGGREGATE_LEAF_COUNT).contains(&count)
                && model.result_leaf_doubles.len() == count
                && aggregate_result_register_classes_supported(&model.result_leaf_doubles)
                && model.value.get_size() == (64 * count) as u32
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
        && !model.has_enum_composite_features
        && !model.has_enum_signature
        && !wide_composite_values(model)
}

fn wide_composite_values(model: &PureI64Body<'_>) -> bool {
    model.has_wide_scalar_features
        && (model.param_shapes.iter().any(Option::is_some)
            || model.result_shape.is_some()
            || model.call_sites.iter().any(|site| {
                site.argument_shapes
                    .iter()
                    .any(|shape| matches!(shape, CallValueShape::Struct { .. }))
                    || matches!(site.result_shape, CallValueShape::Struct { .. })
            }))
}

const MAX_AGGREGATE_LEAF_COUNT: usize = 4;
const INTEGER_RESULT_REGISTERS: [usize; 3] = [0, 2, 1];
const FLOAT_RESULT_REGISTER_COUNT: usize = 2;

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
        && result_leaf_doubles.len() <= MAX_AGGREGATE_LEAF_COUNT
        && integer_leaf_count <= INTEGER_RESULT_REGISTERS.len()
        && float_leaf_count <= FLOAT_RESULT_REGISTER_COUNT
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
                || !(1..=MAX_AGGREGATE_LEAF_COUNT).contains(leaf_count)
                || *has_f64 != leaf_doubles.iter().any(|is_double| *is_double)
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
    if matches!(site.result_shape, CallValueShape::Struct { .. })
        && !aggregate_result_register_classes_supported(&site.result_leaf_doubles)
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

fn assert_path_equivalence_query<'ctx>(
    solver: &Solver<'ctx>,
    model: &PureI64Body<'ctx>,
    arguments: &[BV<'ctx>],
    path: &LiftedPath<'ctx>,
    result_width: ResultWidth,
    context: &'ctx Context,
) {
    assert_path_assumptions(solver, model, arguments, path);
    let output_matches = if let Some(leaf_count) = model.result_shape {
        let matches = (0..leaf_count)
            .map(|leaf| {
                let source_leaf = aggregate_leaf(&model.value, leaf_count, leaf);
                if model.result_leaf_doubles[leaf] {
                    f64_values_match(context, &source_leaf, &path.return_values[leaf])
                } else {
                    source_leaf._eq(&path.return_values[leaf])
                }
            })
            .collect::<Vec<_>>();
        Bool::and(context, &matches.iter().collect::<Vec<_>>())
    } else {
        let return_value = &path.return_values[0];
        match result_width {
            ResultWidth::I64 => model.value._eq(return_value),
            ResultWidth::I32 => model.value.extract(31, 0)._eq(&return_value.extract(31, 0)),
            ResultWidth::F64 => f64_values_match(context, &model.value, return_value),
            ResultWidth::I128 => model.value._eq(return_value),
        }
    };
    let mut equivalent_terms: Vec<&Bool> = path.call_argument_obligations.iter().collect();
    equivalent_terms.push(&output_matches);
    solver.assert(&Bool::and(context, &equivalent_terms).not());
}

fn assert_path_exact_f64_call_arguments_query<'ctx>(
    solver: &Solver<'ctx>,
    model: &PureI64Body<'ctx>,
    arguments: &[BV<'ctx>],
    path: &LiftedPath<'ctx>,
    context: &'ctx Context,
) {
    assert_path_assumptions(solver, model, arguments, path);
    let exact_arguments = path
        .exact_f64_call_argument_obligations
        .iter()
        .collect::<Vec<_>>();
    solver.assert(&Bool::and(context, &exact_arguments).not());
}

fn assert_path_assumptions<'ctx>(
    solver: &Solver<'ctx>,
    model: &PureI64Body<'ctx>,
    arguments: &[BV<'ctx>],
    path: &LiftedPath<'ctx>,
) {
    for (input, register) in model.inputs.iter().zip(arguments) {
        solver.assert(&input._eq(register));
    }
    for assumption in model
        .param_domain
        .iter()
        .chain(&model.definedness)
        .chain(&model.call_result_bounds)
        .chain(model.loop_bound_exits.iter())
    {
        solver.assert(assumption);
    }
    for condition in &path.path_conditions {
        solver.assert(condition);
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
    assert_path_assumptions(solver, model, arguments, path);
    let mut changed_registers = initial_callee_saved
        .iter()
        .zip(&path.callee_saved_registers)
        .map(|(initial, final_value)| final_value._eq(initial).not())
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
}

const MAX_COMPLETED_PATHS: usize = 64;

fn lift<'ctx>(
    instructions: &[Instruction],
    function_size: u64,
    model: &PureI64Body<'ctx>,
    call_targets: &BTreeMap<u64, CallTarget>,
    context: &'ctx Context,
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
    let callee_saved_initial = CALLEE_SAVED_REGISTERS
        .iter()
        .map(|register_index| registers[*register_index].extract(63, 0))
        .collect();
    let mut integer_argument_indices = Vec::new();
    let mut integer_argument_index = 0usize;
    let mut float_argument_index = 0usize;
    let mut arguments = Vec::with_capacity(model.inputs.len());
    let mut input_index = 0usize;
    for (parameter_shape, leaves) in model.param_shapes.iter().zip(&model.param_leaf_doubles) {
        if parameter_shape.is_some() {
            let integer_leaf_count = leaves.iter().filter(|is_double| !**is_double).count();
            let float_leaf_count = leaves.iter().filter(|is_double| **is_double).count();
            if (integer_leaf_count > 0
                && integer_argument_index.saturating_add(integer_leaf_count)
                    > ARGUMENT_REGISTERS.len())
                || (float_leaf_count > 0
                    && float_argument_index.saturating_add(float_leaf_count)
                        > FLOAT_ARGUMENT_REGISTER_COUNT)
            {
                return Err(FunctionOutcome::OutsideProfile("argument_count"));
            }
        } else if leaves.len() == 2
            && integer_argument_index.saturating_add(2) > ARGUMENT_REGISTERS.len()
        {
            return Err(FunctionOutcome::OutsideProfile("argument_count"));
        }
        for is_double in leaves {
            if *is_double {
                let Some(register) = xmm_registers.get(float_argument_index) else {
                    return Err(FunctionOutcome::OutsideProfile("argument_count"));
                };
                arguments.push(register.extract(63, 0));
                float_argument_index += 1;
            } else {
                integer_argument_indices.push(input_index);
                arguments.push(ARGUMENT_REGISTERS.get(integer_argument_index).map_or_else(
                    || BV::fresh_const(context, "machine_initial_stack_argument", 64),
                    |register_index| registers[*register_index].extract(63, 0),
                ));
                integer_argument_index += 1;
            }
            input_index += 1;
        }
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
    }];
    let mut paths = Vec::new();
    while let Some(mut state) = pending.pop() {
        loop {
            let index = state.instruction_index;
            let Some(instruction) = instructions.get(index) else {
                return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
            };
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
                let (value, width) = lift_inc_dec(instruction, &mut state, context)?;
                state.flags = FlagState::ZeroFlagResult { value, width };
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
                let Some(value) = register_value(source, &state.registers, 64) else {
                    return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
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
                if is_incoming_stack_argument_operand(destination, state.rsp_offset) {
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
                if is_incoming_stack_argument_operand(source, state.rsp_offset) {
                    lift_stack_argument_move(
                        instruction,
                        &mut state,
                        &integer_argument_indices,
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
                        &integer_argument_indices,
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
            } else if instruction.mnemonic == "addq"
                && split_operands(&instruction.operands)
                    .first()
                    .is_some_and(|source| source.contains('('))
            {
                lift_stack_argument_add(
                    instruction,
                    &mut state,
                    &integer_argument_indices,
                    &arguments,
                )?;
                state.flags = FlagState::Unknown;
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
                    "movl"
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
    let return_values = if let Some(leaf_count) = model.result_shape {
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
                    state.registers[register_index].extract(63, 0)
                }
            })
            .collect()
    } else if model.value.get_size() == 128 {
        vec![state.registers[2]
            .extract(63, 0)
            .concat(&state.registers[0].extract(63, 0))]
    } else {
        vec![if model_result_is_double(model) {
            state.xmm_registers[0].extract(63, 0)
        } else {
            state.registers[0].extract(63, 0)
        }]
    };
    let callee_saved_registers = CALLEE_SAVED_REGISTERS
        .iter()
        .map(|register_index| state.registers[*register_index].extract(63, 0))
        .collect();
    Ok(LiftedPath {
        return_values,
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
    let right = match operation {
        BitwiseOp::And | BitwiseOp::Or => operand_value(source, registers, context, width),
        BitwiseOp::Xor => register_value(source, registers, width),
    }
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
    let value = registers[index].extract(width - 1, 0);
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
    let previous = state.registers[index].extract(width - 1, 0);
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

fn incoming_stack_argument<'ctx>(
    operand: &str,
    rsp_offset: i64,
    integer_argument_indices: &[usize],
    arguments: &[BV<'ctx>],
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
    let stack_argument_count = integer_argument_indices
        .len()
        .saturating_sub(ARGUMENT_REGISTERS.len());
    if slot_index == 0 || slot_index > stack_argument_count {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let input_index = *integer_argument_indices
        .get(ARGUMENT_REGISTERS.len() + slot_index - 1)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
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

fn lift_stack_argument_move<'ctx>(
    instruction: &Instruction,
    state: &mut MachineState<'ctx>,
    integer_argument_indices: &[usize],
    arguments: &[BV<'ctx>],
) -> Result<(), FunctionOutcome> {
    let operands = split_operands(&instruction.operands);
    let [source, destination] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    if !state.rsp_is_known {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let value = incoming_stack_argument(
        source,
        state.rsp_offset,
        integer_argument_indices,
        arguments,
    )?;
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

fn lift_stack_argument_add<'ctx>(
    instruction: &Instruction,
    state: &mut MachineState<'ctx>,
    integer_argument_indices: &[usize],
    arguments: &[BV<'ctx>],
) -> Result<(), FunctionOutcome> {
    let operands = split_operands(&instruction.operands);
    let [source, destination] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    if !state.rsp_is_known {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let right = incoming_stack_argument(
        source,
        state.rsp_offset,
        integer_argument_indices,
        arguments,
    )?;
    let (register_index, width) = parse_register(destination)
        .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
    if width != 64 || register_index == 4 {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let left = state.registers[register_index].clone();
    if !write_register(&mut state.registers, register_index, left.bvadd(&right), 64) {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    state.flags = FlagState::Unknown;
    Ok(())
}

fn call_value_shape_supported(shape: CallValueShape) -> bool {
    match shape {
        CallValueShape::Scalar { leaf_count } => matches!(leaf_count, 1 | 2),
        CallValueShape::Struct {
            leaf_count,
            has_enum: false,
            ..
        } => (1..=MAX_AGGREGATE_LEAF_COUNT).contains(&leaf_count),
        CallValueShape::Enum { .. } | CallValueShape::Struct { .. } => false,
    }
}

fn call_site_has_unsupported_composite_shape(site: &CallSite<'_>) -> bool {
    site.argument_shapes.iter().copied().any(|shape| {
        matches!(
            shape,
            CallValueShape::Struct { .. } | CallValueShape::Enum { .. }
        ) && !call_value_shape_supported(shape)
    }) || (matches!(
        site.result_shape,
        CallValueShape::Struct { .. } | CallValueShape::Enum { .. }
    ) && (!call_value_shape_supported(site.result_shape)
        || (matches!(site.result_shape, CallValueShape::Struct { .. })
            && !aggregate_result_register_classes_supported(&site.result_leaf_doubles))))
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
        || call_site
            .argument_shapes
            .iter()
            .copied()
            .any(|shape| !call_value_shape_supported(shape))
        || !call_value_shape_supported(call_site.result_shape)
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
    if float_argument_count > FLOAT_ARGUMENT_REGISTER_COUNT {
        return Err(FunctionOutcome::OutsideProfile("argument_count"));
    }
    let integer_argument_count = call_site.args.len() - float_argument_count;
    if tail_call && integer_argument_count > ARGUMENT_REGISTERS.len() {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    }
    let mut outgoing_slots = BTreeSet::new();
    let mut integer_argument_index = 0usize;
    let mut float_argument_index = 0usize;
    let mut leaf_index = 0usize;
    for shape in &call_site.argument_shapes {
        let leaf_count = shape.leaf_count();
        match *shape {
            CallValueShape::Struct { .. } => {
                let leaf_end = leaf_index
                    .checked_add(leaf_count)
                    .ok_or(FunctionOutcome::OutsideProfile("call_arguments"))?;
                let Some(leaf_doubles) = call_site.arg_leaf_doubles.get(leaf_index..leaf_end)
                else {
                    return Err(FunctionOutcome::OutsideProfile("call_arguments"));
                };
                let integer_leaf_count =
                    leaf_doubles.iter().filter(|is_double| !**is_double).count();
                let float_leaf_count = leaf_doubles.iter().filter(|is_double| **is_double).count();
                if (integer_leaf_count > 0
                    && integer_argument_index.saturating_add(integer_leaf_count)
                        > ARGUMENT_REGISTERS.len())
                    || (float_leaf_count > 0
                        && float_argument_index.saturating_add(float_leaf_count)
                            > FLOAT_ARGUMENT_REGISTER_COUNT)
                {
                    return Err(FunctionOutcome::OutsideProfile("argument_count"));
                }
                for (leaf_offset, is_double) in leaf_doubles.iter().enumerate() {
                    let argument = &call_site.args[leaf_index + leaf_offset];
                    if *is_double {
                        let register = xmm_registers[float_argument_index].extract(63, 0);
                        call_state
                            .argument_obligations
                            .push(f64_values_match(context, argument, &register));
                        call_state
                            .exact_f64_argument_obligations
                            .push(argument._eq(&register));
                        float_argument_index += 1;
                    } else {
                        let register_index = ARGUMENT_REGISTERS[integer_argument_index];
                        call_state
                            .argument_obligations
                            .push(argument._eq(&registers[register_index]));
                        integer_argument_index += 1;
                    }
                }
            }
            CallValueShape::Enum { .. } => {
                return Err(FunctionOutcome::OutsideProfile("call_arguments"));
            }
            CallValueShape::Scalar { leaf_count: 2 } => {
                let leaf_end = leaf_index
                    .checked_add(2)
                    .ok_or(FunctionOutcome::OutsideProfile("call_arguments"))?;
                let Some(leaf_doubles) = call_site.arg_leaf_doubles.get(leaf_index..leaf_end)
                else {
                    return Err(FunctionOutcome::OutsideProfile("call_arguments"));
                };
                if leaf_doubles.iter().any(|is_double| *is_double)
                    || integer_argument_index.saturating_add(2) > ARGUMENT_REGISTERS.len()
                {
                    return Err(FunctionOutcome::OutsideProfile("argument_count"));
                }
                for offset in 0..2 {
                    let register_index = ARGUMENT_REGISTERS[integer_argument_index];
                    call_state
                        .argument_obligations
                        .push(call_site.args[leaf_index + offset]._eq(&registers[register_index]));
                    integer_argument_index += 1;
                }
            }
            CallValueShape::Scalar { .. } => {
                let argument = &call_site.args[leaf_index];
                if call_site.arg_leaf_doubles[leaf_index] {
                    let Some(register) = xmm_registers.get(float_argument_index) else {
                        return Err(FunctionOutcome::OutsideProfile("argument_count"));
                    };
                    let register = register.extract(63, 0);
                    call_state
                        .argument_obligations
                        .push(f64_values_match(context, argument, &register));
                    call_state
                        .exact_f64_argument_obligations
                        .push(argument._eq(&register));
                    float_argument_index += 1;
                } else if let Some(register_index) = ARGUMENT_REGISTERS.get(integer_argument_index)
                {
                    call_state
                        .argument_obligations
                        .push(argument._eq(&registers[*register_index]));
                    integer_argument_index += 1;
                } else {
                    if !rsp_is_known || rsp_offset % 8 != 0 {
                        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
                    }
                    let stack_argument_index = integer_argument_index - ARGUMENT_REGISTERS.len();
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
                    let Some((_, stack_argument)) =
                        stack.iter().find(|(candidate, _)| *candidate == stack_slot)
                    else {
                        return Err(FunctionOutcome::OutsideProfile("call_arguments"));
                    };
                    call_state
                        .argument_obligations
                        .push(argument._eq(stack_argument));
                    outgoing_slots.insert(stack_slot);
                    integer_argument_index += 1;
                }
            }
        }
        leaf_index += leaf_count;
    }
    if !tail_call && !outgoing_slots.is_empty() {
        stack.retain(|(stack_slot, _)| !outgoing_slots.contains(stack_slot));
        outgoing_cleanup_slots.extend(outgoing_slots);
    }
    apply_call_result_and_clobbers(
        registers,
        xmm_registers,
        call_site.result.clone(),
        call_site.result_is_double,
        call_site.result_shape,
        &call_site.result_leaf_doubles,
        context,
    )?;
    call_state.used_call_targets.insert(field_offset);
    call_state.call_index += 1;
    Ok(())
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
        } if (1..=MAX_AGGREGATE_LEAF_COUNT).contains(&leaf_count) => {
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
        _ => return Err(FunctionOutcome::OutsideProfile("call_arguments")),
    }
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
    for assumption in model
        .param_domain
        .iter()
        .chain(&model.definedness)
        .chain(&model.call_result_bounds)
        .chain(model.loop_bound_exits.iter())
    {
        solver.assert(assumption);
    }
    for (input, argument) in model.inputs.iter().zip(arguments) {
        solver.assert(&input._eq(argument));
    }
    for path_condition in path_conditions {
        solver.assert(path_condition);
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
    solver.assert(guard);
    let guard_false = solver.check();
    solver.pop(1);
    match guard_false {
        SatResult::Unsat => Some(false),
        SatResult::Sat | SatResult::Unknown => None,
    }
}

fn prove_division_safe<'ctx>(
    model: &PureI64Body<'ctx>,
    state: &MachineState<'ctx>,
    arguments: &[BV<'ctx>],
    safety: &Bool<'ctx>,
    context: &'ctx Context,
) -> Result<(), FunctionOutcome> {
    let status = call_guard_status(model, safety, &state.path_conditions, arguments, context);
    if status == Some(true) {
        Ok(())
    } else {
        Err(FunctionOutcome::OutsideProfile("division_trap"))
    }
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
                    state,
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
                    state,
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
                prove_division_safe(model, state, arguments, &nonzero, context)?;
                let dividend = state.registers[0].clone();
                state.registers[0] = dividend.bvudiv(&divisor);
                state.registers[2] = dividend.bvurem(&divisor);
                return Ok(());
            }
            let safety = Bool::and(context, &[&nonzero, &fits]);
            prove_division_safe(model, state, arguments, &safety, context)?;
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
                state,
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
    let destination = registers[index].extract(width - 1, 0);
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
            let destination = registers[index].extract(width - 1, 0);
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
) -> Result<(BV<'ctx>, u32), FunctionOutcome> {
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
    let previous = state.registers[index].extract(width - 1, 0);
    let is_decrement = matches!(instruction.mnemonic.as_str(), "decl" | "decq");
    let one = BV::from_u64(context, 1, width);
    let value = if is_decrement {
        previous.bvsub(&one)
    } else {
        previous.bvadd(&one)
    };
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
    Ok((value, width))
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
    (register_width == width).then(|| registers[index].extract(width - 1, 0))
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
    fn enum_values_and_enum_matches_remain_outside_machine_tv() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let enum_source = "enum E { Off, Val(x: i64) }\n\
            atom make(x: i64) -> i64 requires: true; ensures: true; body: { let e = E::Val(x); match e { E::Val(y) => y, _ => 0 } };";
        let model = source_model_named_with_admission(
            enum_source,
            "make",
            &context,
            RefinedAdmission::CompositeV23,
        );
        assert!(model.has_enum_composite_features);
        let bytes = [0x48, 0x89, 0xf8, 0xc3];
        assert_eq!(
            check_function(&bytes, 0, &model, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("unsupported_signature")
        );

        let match_source = "enum E { Off, Val(x: i64) }\n\
            atom pick(e: E) -> i64 requires: true; ensures: true; body: { match e { E::Val(y) => y, _ => 0 } };";
        let match_model = source_model_named_with_admission(
            match_source,
            "pick",
            &context,
            RefinedAdmission::CompositeV23,
        );
        assert!(match_model.has_enum_signature);
        assert!(!match_model.has_enum_composite_features);
        assert_eq!(
            check_function(&bytes, 0, &match_model, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("unsupported_signature")
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
        assert_eq!(
            check_function(&mk4_bytes, mk4_address, &mk4_model, &mk4_targets, &context),
            FunctionOutcome::OutsideProfile("unsupported_signature")
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

        let five_leaf_source = "struct Five { a: i64, b: i64, c: i64, d: i64, e: i64 }\n\
            atom identity(value: Five) -> Five requires: true; ensures: true; body: { value };";
        let five_leaf = source_model_named_with_admission(
            five_leaf_source,
            "identity",
            &context,
            RefinedAdmission::CompositeV23,
        );
        assert_eq!(
            check_function(&[0xc3], 0, &five_leaf, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("unsupported_signature")
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
        assert_eq!(
            check_function(
                &[0xc3],
                0,
                &f64_struct_after_eight,
                &BTreeMap::new(),
                &context,
            ),
            FunctionOutcome::OutsideProfile("argument_count")
        );

        let enum_source = "enum Shade { Red, Blue }\n\
            atom color(value: Shade) -> i64 requires: true; ensures: true; body: { 0 };";
        let enum_model = source_model_named_with_admission(
            enum_source,
            "color",
            &context,
            RefinedAdmission::CompositeV23,
        );
        assert!(enum_model.has_enum_signature);
        assert_eq!(
            check_function(&[0xc3], 0, &enum_model, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("unsupported_signature")
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
        assert_eq!(
            check_function(&[0xc3], 0, &after_five, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("argument_count")
        );
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
        assert_eq!(
            check_function(
                &[0xe8, 0, 0, 0, 0, 0xc3],
                0,
                &after_five,
                &after_five_targets,
                &context,
            ),
            FunctionOutcome::OutsideProfile("argument_count")
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

        let enum_source = "enum Shade { Red, Blue }\n\
            atom color(p: Shade) -> i64 requires: true; ensures: true; body: { 0 };\n\
            atom enum_arg() -> i64 requires: true; ensures: true; body: { color(Shade::Red) };";
        let mut enum_targets = targets.clone();
        enum_targets.get_mut(&1).unwrap().name = "color".to_owned();
        assert_eq!(
            check_function(
                &regular_bytes,
                0,
                &model(enum_source, "enum_arg"),
                &enum_targets,
                &context,
            ),
            FunctionOutcome::OutsideProfile("call_arguments")
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
        assert_eq!(
            check_function(
                &regular_bytes,
                0,
                &model(after_four_source, "after_four"),
                &after_four_targets,
                &context,
            ),
            FunctionOutcome::OutsideProfile("argument_count")
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
    fn unsupported_memory_and_enum_features_fail_closed() {
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

        let mut composite_model = source_model(source, &context);
        composite_model.has_composite_or_enum_features = true;
        composite_model.has_composite_features_beyond_struct_values = true;
        composite_model.has_enum_composite_features = true;
        assert_eq!(
            check_function(&bytes, 0, &composite_model, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("unsupported_signature")
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
        let (address, bytes, targets) = compiled_function_with_targets(ir, "divide");
        assert_eq!(
            check_function(&bytes, address, &model("divide"), &targets, &context),
            FunctionOutcome::OutsideProfile("call_order")
        );
        let (address, bytes, targets) = compiled_function_with_targets(ir, "udivide");
        assert_eq!(
            check_function(&bytes, address, &model("udivide"), &targets, &context),
            FunctionOutcome::OutsideProfile("call_order")
        );
        let (address, bytes, targets) = compiled_function_with_targets(ir, "after_five");
        assert_eq!(
            check_function(&bytes, address, &model("after_five"), &targets, &context),
            FunctionOutcome::OutsideProfile("argument_count")
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

        let too_many_f64 = source_model_with_admission(
            "atom too_many(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64, g: f64, h: f64, i: f64) -> f64 requires: true; ensures: true; body: { a };",
            &context,
            RefinedAdmission::CompositeV19,
        );
        assert_eq!(
            check_function(&[0xc3], 0, &too_many_f64, &BTreeMap::new(), &context,),
            FunctionOutcome::OutsideProfile("argument_count")
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
            arg_leaf_doubles: Vec::new(),
            argument_shapes: Vec::new(),
            has_composite_arguments_or_result: false,
            result: float_result,
            result_is_double: true,
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
            arg_leaf_doubles: vec![true; 9],
            argument_shapes: vec![CallValueShape::Scalar { leaf_count: 1 }; 9],
            has_composite_arguments_or_result: false,
            result,
            result_is_double: true,
            result_shape: CallValueShape::Scalar { leaf_count: 1 },
            result_leaf_doubles: vec![true],
            guard: Bool::from_bool(&context, true),
        });
        assert_eq!(
            check_function(
                &mixed_call,
                0,
                &too_many_f64,
                &helper_call_targets(&[1]),
                &context,
            ),
            FunctionOutcome::OutsideProfile("argument_count")
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
        model.has_enum_signature = true;
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
    fn survey_quotient_seven_bytes_remain_inconclusive() {
        let mut config = Config::new();
        config.set_timeout_msec(u64::from(TIMEOUT_MS));
        config.set_param_value("rlimit", &RESOURCE_LIMIT.to_string());
        let context = Context::new(&config);
        let model = source_model(
            "atom quotient_seven(x: i64) -> i64 requires: true; ensures: true; body: { x / 7 };",
            &context,
        );
        let bytes = [
            0x48, 0x89, 0xf8, // movq %rdi, %rax
            0x48, 0xb9, 0x25, 0x49, 0x92, 0x24, 0x49, 0x92, 0x24, 0x49, // movabsq magic, %rcx
            0x48, 0xf7, 0xe9, // imulq %rcx
            0x48, 0x89, 0xd0, // movq %rdx, %rax
            0x48, 0xc1, 0xe8, 0x3f, // shrq $63, %rax
            0x48, 0xd1, 0xfa, // sarq %rdx
            0x48, 0x01, 0xd0, // addq %rdx, %rax
            0xc3, // retq
        ];
        assert_eq!(
            check_function(&bytes, 0, &model, &BTreeMap::new(), &context),
            FunctionOutcome::Unknown
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
        assert_eq!(
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
}
