use inkwell::targets::{InitializationConfig, Target};
use llvm_sys::disassembler::{
    LLVMCreateDisasm, LLVMDisasmContextRef, LLVMDisasmDispose, LLVMDisasmInstruction,
};
use mumei_core::semantic_contract::PureI64Body;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{CStr, CString},
    ptr,
    sync::Once,
};
use z3::{
    ast::{Ast, Bool, BV},
    Context, Params, SatResult, Solver,
};

pub const TIMEOUT_MS: u32 = 5_000;
pub const RESOURCE_LIMIT: u32 = 1_000_000;

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
    if !supports_signature(model) {
        return FunctionOutcome::OutsideProfile("unsupported_signature");
    }
    if model.inputs.len() > 6 {
        return FunctionOutcome::OutsideProfile("argument_count");
    }
    if model.call_sites.iter().any(|site| site.args.len() > 6) {
        return FunctionOutcome::OutsideProfile("argument_count");
    }
    let instructions = match disassemble(bytes, address) {
        Ok(instructions) if !instructions.is_empty() => instructions,
        _ => return FunctionOutcome::OutsideProfile("unsupported_instruction"),
    };
    let lifted = match lift(
        &instructions,
        u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        model,
        call_targets,
        context,
    ) {
        Ok(lifted) => lifted,
        Err(outcome) => return outcome,
    };
    let mut unknown_path = false;
    for path in &lifted.paths {
        let solver = Solver::new(context);
        set_solver_budget(&solver, context);
        for (input, register) in model.inputs.iter().zip(&lifted.arguments) {
            solver.assert(&input._eq(register));
        }
        for assumption in model
            .param_domain
            .iter()
            .chain(&model.definedness)
            .chain(&model.call_result_bounds)
        {
            solver.assert(assumption);
        }
        for condition in &path.path_conditions {
            solver.assert(condition);
        }
        let output_matches = model.value._eq(&path.return_value);
        let mut equivalent_terms: Vec<&Bool> = path.call_argument_obligations.iter().collect();
        equivalent_terms.push(&output_matches);
        solver.assert(&Bool::and(context, &equivalent_terms).not());
        match solver.check() {
            SatResult::Unsat => {}
            SatResult::Sat => return FunctionOutcome::Different,
            SatResult::Unknown => unknown_path = true,
        }
    }
    if unknown_path {
        FunctionOutcome::Unknown
    } else {
        FunctionOutcome::Equivalent
    }
}

pub fn supports_signature(model: &PureI64Body<'_>) -> bool {
    !model.composite_signature
        && model.result_shape.is_none()
        && !model
            .param_leaf_doubles
            .iter()
            .flatten()
            .any(|is_double| *is_double)
        && !model.result_leaf_doubles.iter().any(|is_double| *is_double)
        && model.loop_bound_exits.is_none()
        && !model.bounded_memory
}

fn set_solver_budget(solver: &Solver, context: &Context) {
    let mut params = Params::new(context);
    params.set_u32("timeout", TIMEOUT_MS);
    params.set_u32("rlimit", RESOURCE_LIMIT);
    solver.set_params(&params);
}

const ARGUMENT_REGISTERS: [usize; 6] = [7, 6, 2, 1, 8, 9];

struct LiftedFunction<'ctx> {
    arguments: Vec<BV<'ctx>>,
    paths: Vec<LiftedPath<'ctx>>,
}

struct LiftedPath<'ctx> {
    return_value: BV<'ctx>,
    path_conditions: Vec<Bool<'ctx>>,
    call_argument_obligations: Vec<Bool<'ctx>>,
    used_call_targets: BTreeSet<u64>,
}

#[derive(Clone)]
struct CallState<'ctx> {
    call_index: usize,
    used_call_targets: BTreeSet<u64>,
    argument_obligations: Vec<Bool<'ctx>>,
}

#[derive(Clone)]
enum FlagState<'ctx> {
    Unknown,
    Compare {
        left: BV<'ctx>,
        right: BV<'ctx>,
        width: u32,
    },
    Result {
        value: BV<'ctx>,
        width: u32,
    },
    ShiftResult {
        value: BV<'ctx>,
        width: u32,
    },
}

#[derive(Clone)]
struct MachineState<'ctx> {
    instruction_index: usize,
    registers: Vec<BV<'ctx>>,
    known_upper_32_zero: [bool; 16],
    stack: Vec<(i64, BV<'ctx>)>,
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
    let successors = control_flow_successors(instructions, &branch_targets, call_targets);
    let reachable = reachable_instructions(&successors);
    if reachable_control_flow_has_cycle(&successors, &reachable) {
        return Err(FunctionOutcome::OutsideProfile("cyclic_control_flow"));
    }
    let registers = (0..16)
        .map(|_| BV::fresh_const(context, "machine_initial_register", 64))
        .collect::<Vec<_>>();
    let arguments: Vec<_> = ARGUMENT_REGISTERS
        .iter()
        .map(|index| registers[*index].extract(63, 0))
        .collect();
    let mut pending = vec![MachineState {
        instruction_index: 0,
        registers,
        known_upper_32_zero: [false; 16],
        stack: Vec::new(),
        rsp_offset: 0,
        rsp_is_known: true,
        flags: FlagState::Unknown,
        call_state: CallState {
            call_index: 0,
            used_call_targets: BTreeSet::new(),
            argument_obligations: Vec::new(),
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
                pending.push(taken);
                state.path_conditions.push(condition.not());
                state.instruction_index = index + 1;
                continue;
            }
            if is_unconditional_jump(&instruction.mnemonic) {
                if is_tail_call(instruction, call_targets) {
                    if index + 1 != instructions.len()
                        || state.rsp_offset != 0
                        || !state.stack.is_empty()
                    {
                        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
                    }
                    lift_call(
                        instruction,
                        model,
                        call_targets,
                        &mut state.call_state,
                        &mut state.registers,
                        &state.path_conditions,
                        &arguments,
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
            } else if instruction.mnemonic == "callq" {
                lift_call(
                    instruction,
                    model,
                    call_targets,
                    &mut state.call_state,
                    &mut state.registers,
                    &state.path_conditions,
                    &arguments,
                    context,
                )?;
                state.flags = FlagState::Unknown;
            } else if instruction.mnemonic == "pushq" {
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
                let operands = split_operands(&instruction.operands);
                let [destination] = operands.as_slice() else {
                    return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
                };
                let Some((register_index, width)) = parse_register(destination) else {
                    return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
                };
                let Some((stack_slot, _)) = state.stack.last() else {
                    return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
                };
                if width != 64 || register_index == 4 || *stack_slot != state.rsp_offset {
                    return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
                }
                let (_, value) = state.stack.pop().expect("stack was checked as non-empty");
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
            } else if matches!(instruction.mnemonic.as_str(), "addq" | "subq")
                && split_operands(&instruction.operands)
                    .get(1)
                    .is_some_and(|destination| *destination == "%rsp")
            {
                lift_rsp_adjustment(instruction, &mut state, context)?;
            } else if matches!(instruction.mnemonic.as_str(), "cmpb" | "cmpl" | "cmpq") {
                let width = instruction_width(&instruction.mnemonic)?;
                state.flags =
                    compare_flags(&instruction.operands, &state.registers, context, width)?;
            } else if matches!(instruction.mnemonic.as_str(), "testb" | "testl" | "testq") {
                let width = instruction_width(&instruction.mnemonic)?;
                state.flags = test_flags(&instruction.operands, &state.registers, context, width)?;
            } else if matches!(instruction.mnemonic.as_str(), "subl" | "subq") {
                let width = instruction_width(&instruction.mnemonic)?;
                let (left, right) =
                    binary_operands(&instruction.operands, &state.registers, context, width)?;
                if !lift_binary(
                    &split_operands(&instruction.operands),
                    &mut state.registers,
                    context,
                    width,
                    BinaryOp::Sub,
                ) {
                    return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
                }
                state.flags = FlagState::Compare { left, right, width };
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
                state.flags = lift_shift(instruction, &mut state.registers, context)
                    .ok_or(FunctionOutcome::OutsideProfile("unsupported_instruction"))?;
            } else if instruction.mnemonic.starts_with("set") {
                lift_setcc(instruction, &mut state, context)?;
            } else if instruction.mnemonic.starts_with("cmov") {
                lift_cmov(instruction, &mut state, context)?;
            } else {
                let leaves_flags_unchanged = matches!(
                    instruction.mnemonic.as_str(),
                    "movl" | "movq" | "movzbl" | "movzbq" | "movsbl" | "movsbq" | "leaq"
                );
                if !lift_instruction(
                    instruction,
                    &mut state.registers,
                    move_source_upper_32_zero,
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
                _ => (),
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
    Ok(LiftedFunction { arguments, paths })
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
    let (zero, sign, signed_less, unsigned_less) = match flags {
        FlagState::Unknown => {
            return Err(FunctionOutcome::OutsideProfile("unsupported_flags"));
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
        FlagState::Result { value, width } => {
            let zero = value._eq(&BV::from_u64(context, 0, *width));
            let sign = most_significant_bit(value, *width, context);
            (zero, sign.clone(), sign, Bool::from_bool(context, false))
        }
        FlagState::ShiftResult { value, width } => {
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
    Ok(LiftedPath {
        return_value: state.registers[0].extract(63, 0),
        path_conditions: state.path_conditions,
        call_argument_obligations: state.call_state.argument_obligations,
        used_call_targets: state.call_state.used_call_targets,
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
    let (count, destination) = match operands.as_slice() {
        [destination] => (1, *destination),
        [count, destination] => {
            let count = count.strip_prefix('$').and_then(parse_integer)?;
            (count, *destination)
        }
        _ => return None,
    };
    let (index, destination_width) = parse_register(destination)?;
    if destination_width != width || index == 4 {
        return None;
    }
    let count_mask = if width == 64 { 0x3f } else { 0x1f };
    let count = count & count_mask;
    let value = registers[index].extract(width - 1, 0);
    let shifted = if count == 0 {
        value
    } else {
        let count = BV::from_u64(context, count, width);
        match operation {
            ShiftOp::Left => value.bvshl(&count),
            ShiftOp::LogicalRight => value.bvlshr(&count),
            ShiftOp::ArithmeticRight => value.bvashr(&count),
        }
    };
    let flags = if count == 0 {
        FlagState::Unknown
    } else {
        FlagState::ShiftResult {
            value: shifted.clone(),
            width,
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
    context: &'ctx Context,
) -> Result<(), FunctionOutcome> {
    let operands = split_operands(&instruction.operands);
    let [immediate, destination] = operands.as_slice() else {
        return Err(FunctionOutcome::OutsideProfile("unsupported_instruction"));
    };
    if *destination != "%rsp" {
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
    state.flags = if instruction.mnemonic == "subq" {
        FlagState::Compare {
            left: previous_rsp,
            right: BV::from_u64(context, amount, 64),
            width: 64,
        }
    } else {
        FlagState::Unknown
    };
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn lift_call<'ctx>(
    instruction: &Instruction,
    model: &PureI64Body<'ctx>,
    call_targets: &BTreeMap<u64, CallTarget>,
    call_state: &mut CallState<'ctx>,
    registers: &mut [BV<'ctx>],
    path_conditions: &[Bool<'ctx>],
    arguments: &[BV<'ctx>],
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
    if call_site.args.len() > ARGUMENT_REGISTERS.len() {
        return Err(FunctionOutcome::OutsideProfile("argument_count"));
    }
    for (argument, register_index) in call_site.args.iter().zip(ARGUMENT_REGISTERS) {
        call_state
            .argument_obligations
            .push(argument._eq(&registers[register_index]));
    }
    registers[0] = call_site.result.clone();
    for register_index in [1, 2, 6, 7, 8, 9, 10, 11] {
        registers[register_index] = BV::fresh_const(context, "machine_call_clobber", 64);
    }
    call_state.used_call_targets.insert(field_offset);
    call_state.call_index += 1;
    Ok(())
}

fn call_guard_status<'ctx>(
    model: &PureI64Body<'ctx>,
    guard: &Bool<'ctx>,
    path_conditions: &[Bool<'ctx>],
    arguments: &[BV<'ctx>],
    context: &'ctx Context,
) -> Option<bool> {
    let solver = Solver::new(context);
    set_solver_budget(&solver, context);
    for assumption in model
        .param_domain
        .iter()
        .chain(&model.definedness)
        .chain(&model.call_result_bounds)
    {
        solver.assert(assumption);
    }
    for (input, argument) in model.inputs.iter().zip(arguments) {
        solver.assert(&input._eq(argument));
    }
    for path_condition in path_conditions {
        solver.assert(path_condition);
    }
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
            prove_division_safe(
                model,
                state,
                arguments,
                &Bool::and(context, &[&nonzero, &fits]),
                context,
            )?;
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
    if next_offset > 0 {
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
    source_upper_32_zero: bool,
    context: &'ctx Context,
) -> bool {
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
        "negl" => lift_neg(&operands, registers, 32),
        "negq" => lift_neg(&operands, registers, 64),
        "xorl" => lift_xor(&operands, registers, 32),
        "xorq" => lift_xor(&operands, registers, 64),
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
        [base, index, scale] if !index.is_empty() && !scale.is_empty() => {
            if base.is_empty() && displacement != 0 {
                return None;
            }
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
        encode_pure_i64_module(source, context, true, RefinedAdmission::Reject)
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
        encode_pure_i64_module(source, context, true, RefinedAdmission::Reject)
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
            check_function(&bytes, 0, &model, &BTreeMap::new(), &context),
            FunctionOutcome::Equivalent
        );
        assert_eq!(
            check_function(&[0x5b, 0xc3], 0, &model, &BTreeMap::new(), &context),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
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
    fn loop_signature_is_outside_profile() {
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
            FunctionOutcome::OutsideProfile("unsupported_signature")
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
        let variable_count = [0x48, 0xd3, 0xe0, 0xc3]; // shlq %cl, %rax; retq
        assert_eq!(
            check_function(
                &variable_count,
                0,
                &identity_model,
                &BTreeMap::new(),
                &context
            ),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
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
            FunctionOutcome::OutsideProfile("cyclic_control_flow")
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
        let bytes = [0x48, 0x83, 0xc7, 0x01, 0x74, 0x01, 0xc3, 0xc3];
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
        for _ in 0..7 {
            bytes.extend([0x48, 0x85, 0xff, 0x74, 0x01, 0x90]);
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
                lifted.paths[0]
                    .return_value
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
