use inkwell::targets::{InitializationConfig, Target};
use llvm_sys::disassembler::{
    LLVMCreateDisasm, LLVMDisasmContextRef, LLVMDisasmDispose, LLVMDisasmInstruction,
};
use mumei_core::semantic_contract::PureI64Body;
use std::{
    ffi::{CStr, CString},
    ptr,
    sync::Once,
};
use z3::{
    ast::{Ast, BV},
    Context, Params, SatResult, Solver,
};

pub const TIMEOUT_MS: u32 = 5_000;
pub const RESOURCE_LIMIT: u32 = 1_000_000;

static INITIALIZE_X86: Once = Once::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instruction {
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
            mnemonic: mnemonic.to_owned(),
            operands: operands.trim().to_owned(),
        });
        offset = offset.checked_add(consumed).ok_or(())?;
    }
    Ok(instructions)
}

pub fn check_function<'ctx>(
    bytes: &[u8],
    address: u64,
    model: &PureI64Body<'ctx>,
    context: &'ctx Context,
) -> FunctionOutcome {
    if !model.call_sites.is_empty() {
        return FunctionOutcome::OutsideProfile("calls");
    }
    if model.inputs.len() > 6 {
        return FunctionOutcome::OutsideProfile("argument_count");
    }
    let instructions = match disassemble(bytes, address) {
        Ok(instructions) if !instructions.is_empty() => instructions,
        _ => return FunctionOutcome::OutsideProfile("unsupported_instruction"),
    };
    let Some(lifted) = lift(&instructions, context) else {
        return FunctionOutcome::OutsideProfile("unsupported_instruction");
    };
    let solver = Solver::new(context);
    let mut params = Params::new(context);
    params.set_u32("timeout", TIMEOUT_MS);
    params.set_u32("rlimit", RESOURCE_LIMIT);
    solver.set_params(&params);
    for (input, register) in model.inputs.iter().zip(&lifted.arguments) {
        solver.assert(&input._eq(register));
    }
    solver.assert(&model.value._eq(&lifted.return_value).not());
    match solver.check() {
        SatResult::Unsat => FunctionOutcome::Equivalent,
        SatResult::Sat => FunctionOutcome::Different,
        SatResult::Unknown => FunctionOutcome::Unknown,
    }
}

const ARGUMENT_REGISTERS: [usize; 6] = [7, 6, 2, 1, 8, 9];

struct LiftedFunction<'ctx> {
    return_value: BV<'ctx>,
    arguments: Vec<BV<'ctx>>,
}

fn lift<'ctx>(
    instructions: &[Instruction],
    context: &'ctx Context,
) -> Option<LiftedFunction<'ctx>> {
    let mut registers = (0..16)
        .map(|_| BV::fresh_const(context, "machine_initial_register", 64))
        .collect::<Vec<_>>();
    let arguments = ARGUMENT_REGISTERS
        .iter()
        .map(|index| registers[*index].extract(63, 0))
        .collect();
    let mut returned = false;
    for (index, instruction) in instructions.iter().enumerate() {
        if returned {
            return None;
        }
        if instruction.mnemonic == "retq" {
            if index + 1 != instructions.len() || !instruction.operands.is_empty() {
                return None;
            }
            returned = true;
        } else if instruction.mnemonic.starts_with("nop") {
            continue;
        } else if !lift_instruction(instruction, &mut registers, context) {
            return None;
        }
    }
    returned.then(|| LiftedFunction {
        return_value: registers[0].extract(63, 0),
        arguments,
    })
}

fn lift_instruction<'ctx>(
    instruction: &Instruction,
    registers: &mut [BV<'ctx>],
    context: &'ctx Context,
) -> bool {
    let operands = split_operands(&instruction.operands);
    match instruction.mnemonic.as_str() {
        "movl" => lift_move(&operands, registers, context, 32),
        "movq" => lift_move(&operands, registers, context, 64),
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
) -> bool {
    let [source, destination] = operands else {
        return false;
    };
    let Some((index, destination_width)) = parse_register(destination) else {
        return false;
    };
    if destination_width != width {
        return false;
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
    if width != 64 {
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
    if destination_width != width {
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
            if destination_width != width {
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
            if destination_width != 64 {
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
    if destination_width != width {
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
    if destination_width != width {
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
    if value.get_size() != width {
        return false;
    }
    registers[index] = if width == 32 {
        value.zero_ext(32)
    } else {
        value
    };
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
    use object::{Object, ObjectSection, ObjectSymbol, SymbolKind};
    use std::fs;
    use z3::Config;

    fn source_model<'ctx>(source: &str, context: &'ctx Context) -> PureI64Body<'ctx> {
        encode_pure_i64_module(source, context, true, RefinedAdmission::Reject)
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
    }

    fn compiled_helper() -> (u64, Vec<u8>) {
        let directory = tempfile::tempdir().unwrap();
        let ir_path = directory.path().join("helper.ll");
        let object_path = directory.path().join("helper.o");
        fs::write(
            &ir_path,
            r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @helper(i64 %x) {
entry:
  %result = add i64 %x, 1
  ret i64 %result
}
"#,
        )
        .unwrap();
        crate::codegen::compile_llvm_ir_to_object_recorded(&ir_path, &object_path).unwrap();
        let bytes = fs::read(object_path).unwrap();
        let file = object::File::parse(bytes.as_slice()).unwrap();
        let symbol = file
            .symbols()
            .find(|symbol| {
                symbol.name().ok() == Some("helper")
                    && symbol.is_definition()
                    && symbol.kind() == SymbolKind::Text
            })
            .unwrap();
        let section = file
            .section_by_index(symbol.section_index().unwrap())
            .unwrap();
        (
            symbol.address(),
            section
                .data_range(symbol.address(), symbol.size())
                .unwrap()
                .unwrap()
                .to_vec(),
        )
    }

    #[test]
    fn llvm17_x86_disassembler_is_available() {
        let instructions = disassemble(&[0xb8, 7, 0, 0, 0, 0xc3], 0).unwrap();
        assert_eq!(instructions.len(), 2);
        assert_eq!(instructions[0].mnemonic, "movl");
        assert_eq!(instructions[0].operands, "$7, %eax");
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
            check_function(&bytes, address, &model, &context),
            FunctionOutcome::Equivalent
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
            check_function(&bytes, address, &model, &context),
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
            check_function(&[0x55, 0xc3], 0, &model, &context),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );
    }
}
