use super::*;
use core::arch::asm;
use mumei_core::semantic_contract::{encode_pure_i64_module, RefinedAdmission};
use std::mem::offset_of;
use z3::{Config, SatResult, Solver};

const GPRS: [usize; 6] = [0, 1, 2, 6, 7, 8]; // rax, rcx, rdx, rsi, rdi, r8
const CF: u64 = 1 << 0;
const PF: u64 = 1 << 2;
const ZF: u64 = 1 << 6;
const SF: u64 = 1 << 7;
const OF: u64 = 1 << 11;
const STATUS: u64 = CF | ZF | SF | OF;

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
struct Snapshot {
    gpr: [u64; 6],
    xmm: [[u64; 2]; 2],
    incoming: u64,
    local: u64,
    flags_before: u64,
    flags_after: u64,
    rsp_delta: i64,
}

pub(super) struct Probe<'ctx> {
    input: Snapshot,
    symbolic_registers: Option<Vec<BV<'ctx>>>,
    end: usize,
    local_at: Option<usize>,
    flags_before: Option<FlagState<'ctx>>,
    final_state: Option<MachineState<'ctx>>,
    local: Option<BV<'ctx>>,
}

impl<'ctx> Probe<'ctx> {
    pub(super) fn seed_registers(
        &self,
        registers: &mut [BV<'ctx>],
        xmm: &mut [BV<'ctx>],
        context: &'ctx Context,
    ) {
        if let Some(symbolic) = &self.symbolic_registers {
            for (register, value) in registers.iter_mut().zip(symbolic) {
                *register = value.clone();
            }
            return;
        }
        for (index, value) in GPRS.into_iter().zip(self.input.gpr) {
            registers[index] = BV::from_u64(context, value, 64);
        }
        for (register, [low, high]) in xmm.iter_mut().zip(self.input.xmm) {
            *register = BV::from_u64(context, high, 64).concat(&BV::from_u64(context, low, 64));
        }
    }

    fn symbolic(end: usize, context: &'ctx Context) -> Self {
        let symbolic_registers = (0..16)
            .map(|index| {
                BV::fresh_const(
                    context,
                    &format!("independent_initial_register_{index}"),
                    64,
                )
            })
            .collect();
        Self {
            input: Snapshot::default(),
            symbolic_registers: Some(symbolic_registers),
            end,
            local_at: None,
            flags_before: None,
            final_state: None,
            local: None,
        }
    }

    pub(super) fn seed_stack_arguments(
        &self,
        spilled: &[(usize, bool)],
        arguments: &mut [BV<'ctx>],
        context: &'ctx Context,
    ) {
        if let [(index, _)] = spilled {
            arguments[*index] = BV::from_u64(context, self.input.incoming, 64);
        } else {
            assert!(
                spilled.is_empty(),
                "fixture has more than one incoming slot"
            );
        }
    }

    pub(super) fn observe(&mut self, index: usize, state: &MachineState<'ctx>) {
        if index == 1 {
            assert!(self.flags_before.replace(state.flags.clone()).is_none());
        }
        if index == self.end {
            assert!(self.final_state.replace(state.clone()).is_none());
        }
        if Some(index) == self.local_at {
            let value = state.stack.iter().find(|(offset, _)| *offset == -8);
            self.local = value.map(|(_, value)| value.clone());
        }
    }
}

// Reserve space below Rust's red zone and restore RSP and MXCSR before
// returning to Rust. The snippets are compile-time strings, and the only
// memory they use is the private stack area and the two Snapshot pointers.
macro_rules! cpu {
    ($input:expr; $($line:expr),* $(; capture_then $pop:literal)? $(,)?) => {{
        let input: &Snapshot = $input;
        let mut output = Snapshot::default();
        // SAFETY: both pointers are live, all clobbers are declared, and the
        // test snippets use only the registers/stack slots set up here.
        unsafe {
            asm!(
                "leaq -160(%rsp), %rsp",
                "movq %rsp, %r11",
                "stmxcsr 16(%rsp)",
                "movl $0x1f80, 24(%rsp)",
                "ldmxcsr 24(%rsp)",
                "movq {incoming}({input}), %r10",
                "movq %r10, 8(%rsp)",
                "movq 0({input}), %rax",
                "movq 8({input}), %rcx",
                "movq 16({input}), %rdx",
                "movq 24({input}), %rsi",
                "movq 32({input}), %rdi",
                "movq 40({input}), %r8",
                "movups {xmm0}({input}), %xmm0",
                "movups {xmm1}({input}), %xmm1",
                "cmpq %rdi, %rcx",
                "pushfq",
                "popq %r10",
                "movq %r10, {flags_before}({output})",
                "movq $0, %r10",
                $($line,)*
                $(
                    "movq (%rsp), %r10",
                    $pop,
                )?
                "movq %r10, {local}({output})",
                "pushfq",
                "popq %r10",
                "movq %r10, {flags_after}({output})",
                "movq 8(%r11), %r10",
                "movq %r10, {incoming}({output})",
                "movq %rsp, %r10",
                "subq %r11, %r10",
                "movq %r10, {rsp_delta}({output})",
                "movq %rax, 0({output})",
                "movq %rcx, 8({output})",
                "movq %rdx, 16({output})",
                "movq %rsi, 24({output})",
                "movq %rdi, 32({output})",
                "movq %r8, 40({output})",
                "movups %xmm0, {xmm0}({output})",
                "movups %xmm1, {xmm1}({output})",
                "ldmxcsr 16(%r11)",
                "leaq 160(%r11), %rsp",
                input = in(reg) input,
                output = in(reg) &mut output,
                xmm0 = const offset_of!(Snapshot, xmm),
                xmm1 = const offset_of!(Snapshot, xmm) + 16,
                incoming = const offset_of!(Snapshot, incoming),
                local = const offset_of!(Snapshot, local),
                flags_before = const offset_of!(Snapshot, flags_before),
                flags_after = const offset_of!(Snapshot, flags_after),
                rsp_delta = const offset_of!(Snapshot, rsp_delta),
                out("rax") _, out("rcx") _, out("rdx") _, out("rsi") _,
                out("rdi") _, out("r8") _, out("r10") _, out("r11") _,
                out("xmm0") _, out("xmm1") _,
                options(att_syntax),
            );
        }
        output
    }};
}

#[derive(Clone, Copy)]
enum Flags {
    All,
    Float,
    Zero,
    Unmodeled,
    ShiftCl,
    ShiftImm(u8),
}

impl Flags {
    fn mask(self, input: &Snapshot) -> u64 {
        match self {
            Self::All => STATUS,
            Self::Float => STATUS | PF,
            Self::Zero => ZF,
            Self::Unmodeled => 0,
            Self::ShiftCl if input.gpr[1] & 63 == 0 => STATUS,
            Self::ShiftImm(count) if count & 63 == 0 => STATUS,
            Self::ShiftCl | Self::ShiftImm(_) => ZF,
        }
    }
}

fn flags_bits(flags: &FlagState<'_>, context: &Context) -> Option<(u64, u64)> {
    let condition = |suffix| {
        condition_for_flags(flags, suffix, context)
            .ok()?
            .simplify()
            .as_bool()
    };
    let bit = |value, mask| if value { mask } else { 0 };
    match flags {
        FlagState::Unknown => Some((0, 0)),
        FlagState::ZeroFlagResult { .. } => Some((ZF, bit(condition("e")?, ZF))),
        _ => {
            let sign = condition("s")?;
            // The production condition decoder exposes SF and SF xor OF.
            let overflow = sign != condition("l")?;
            let mut mask = STATUS;
            let mut bits = bit(condition("b")?, CF)
                | bit(condition("e")?, ZF)
                | bit(sign, SF)
                | bit(overflow, OF);
            if matches!(flags, FlagState::FloatCompare { .. }) {
                mask |= PF;
                bits |= bit(condition("p")?, PF);
            }
            Some((mask, bits))
        }
    }
}

fn instructions(code: &[&str]) -> Vec<Instruction> {
    std::iter::once("cmpq %rdi, %rcx")
        .chain(code.iter().copied())
        .chain(std::iter::once("retq"))
        .enumerate()
        .map(|(offset, asm)| {
            let (mnemonic, operands) = asm.split_once(' ').unwrap_or((asm, ""));
            Instruction {
                offset: offset as u64,
                mnemonic: mnemonic.into(),
                operands: operands.into(),
            }
        })
        .collect()
}

fn lift_case<'ctx>(
    model: &PureI64Body<'ctx>,
    instructions: &[Instruction],
    input: Snapshot,
    local: bool,
    context: &'ctx Context,
) -> Result<(Probe<'ctx>, LiftedPath<'ctx>), FunctionOutcome> {
    let end = instructions.len() - 1;
    let mut probe = Probe {
        input,
        symbolic_registers: None,
        end,
        local_at: local.then_some(end - 1),
        flags_before: None,
        final_state: None,
        local: None,
    };
    let mut lifted = lift_inner(
        instructions,
        instructions.len() as u64,
        model,
        &BTreeMap::new(),
        context,
        Some(&mut probe),
    )?;
    assert_eq!(lifted.paths.len(), 1, "fixture input is concrete");
    Ok((probe, lifted.paths.pop().unwrap()))
}

fn compare(
    probe: Probe<'_>,
    path: LiftedPath<'_>,
    actual: Snapshot,
    actual_return: u64,
    flags: Flags,
    local: bool,
    context: &Context,
) -> Result<(), String> {
    let value = |bv: &BV<'_>| bv.simplify().as_u64();
    let state = probe.final_state.ok_or("no modeled state reached retq")?;
    for (index, actual) in GPRS.into_iter().zip(actual.gpr) {
        let modeled = value(&state.registers[index]);
        if modeled != Some(actual) {
            return Err(format!("GPR {index}: model {modeled:#x?}, CPU {actual:#x}"));
        }
    }
    for (index, [low, high]) in actual.xmm.into_iter().enumerate() {
        let register = &state.xmm_registers[index];
        let modeled = [
            value(&register.extract(63, 0)),
            value(&register.extract(127, 64)),
        ];
        if modeled != [Some(low), Some(high)] {
            return Err(format!(
                "xmm{index}: model {modeled:#x?}, CPU [{low:#x}, {high:#x}]"
            ));
        }
    }
    let before = probe.flags_before.ok_or("no modeled initial flags")?;
    let after = state.flags;
    let pairs = [
        ("before", &before, STATUS, actual.flags_before),
        (
            "after",
            &after,
            flags.mask(&probe.input),
            actual.flags_after,
        ),
    ];
    for (when, flags, expected_mask, actual) in pairs {
        let modeled = flags_bits(flags, context);
        if modeled != Some((expected_mask, actual & expected_mask)) {
            return Err(format!(
                "flags {when}: model (mask, bits) {modeled:#x?}, CPU mask {expected_mask:#x} bits {:#x}",
                actual & expected_mask
            ));
        }
    }
    if !matches!(path.return_values.as_slice(), [result] if value(result) == Some(actual_return)) {
        let modeled: Vec<_> = path.return_values.iter().map(value).collect();
        return Err(format!(
            "completed return: model {modeled:#x?}, CPU {actual_return:#x}"
        ));
    }
    if path.final_rsp_offset != actual.rsp_delta {
        return Err(format!(
            "completed RSP delta: model {}, CPU {}",
            path.final_rsp_offset, actual.rsp_delta
        ));
    }
    if probe.input.incoming != actual.incoming {
        return Err(format!(
            "incoming slot changed: model {:#x}, CPU {:#x}",
            probe.input.incoming, actual.incoming
        ));
    }
    if local {
        let modeled = probe.local.as_ref().and_then(value);
        if modeled != Some(actual.local) {
            return Err(format!(
                "local slot: model {modeled:#x?}, CPU {:#x}",
                actual.local
            ));
        }
    }
    Ok(())
}

enum ReturnClass {
    I64,
    F64,
}

struct Suite<'ctx> {
    context: &'ctx Context,
    model: PureI64Body<'ctx>,
    return_class: ReturnClass,
    mismatches: Vec<String>,
}

impl<'ctx> Suite<'ctx> {
    fn new(context: &'ctx Context, return_class: ReturnClass) -> Self {
        let (return_type, parameters, admission, body) = match return_class {
            ReturnClass::I64 => (
                "i64",
                "a: i64, b: i64, c: i64, d: i64, e: i64, f: i64, g: i64",
                RefinedAdmission::Reject,
                "0",
            ),
            ReturnClass::F64 => (
                "f64",
                "a: f64, b: f64, c: f64, d: f64, e: f64, f: f64, g: f64, h: f64, i: f64",
                RefinedAdmission::CompositeV23,
                "a",
            ),
        };
        let source = format!(
            "atom fixture({parameters}) -> {return_type} requires: true; ensures: true; body: {{ {body} }};"
        );
        let model = encode_pure_i64_module(&source, context, true, admission)
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(
            model_result_is_double(&model),
            matches!(return_class, ReturnClass::F64),
            "model misclassified the fixture's declared {return_type} return"
        );
        Self {
            context,
            model,
            return_class,
            mismatches: Vec::new(),
        }
    }

    fn check(
        &mut self,
        shape: &str,
        code: &[&str],
        flags: Flags,
        local: bool,
        inputs: &[Snapshot],
        run_cpu: impl Fn(&Snapshot) -> Snapshot,
    ) {
        let instructions = instructions(code);
        let mut failures = 0;
        for input in inputs {
            let actual = run_cpu(input);
            let actual_return = match self.return_class {
                ReturnClass::I64 => actual.gpr[0],
                ReturnClass::F64 => actual.xmm[0][0],
            };
            let result = lift_case(&self.model, &instructions, *input, local, self.context)
                .map_err(|error| format!("model rejected fixture: {error:?}"))
                .and_then(|(probe, path)| {
                    compare(
                        probe,
                        path,
                        actual,
                        actual_return,
                        flags,
                        local,
                        self.context,
                    )
                });
            if let Err(error) = result {
                if failures < 2 {
                    self.mismatches
                        .push(format!("({code:?}, {shape}, input={input:#x?}): {error}"));
                }
                failures += 1;
            }
        }
        eprintln!(
            "{shape:<20} {code:?}: {} cases, {failures} mismatches",
            inputs.len()
        );
    }

    fn finish(self) {
        assert!(self.mismatches.is_empty(), "{}", self.mismatches.join("\n"));
    }
}

macro_rules! form {
    ($suite:ident, $inputs:expr, $shape:literal, $flags:expr, $($asm:expr),+ $(,)?) => {
        $suite.check($shape, &[$($asm),+], $flags, false, $inputs, |input| cpu!(input; $($asm),+));
    };
}

fn random_word(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn concrete_inputs() -> Vec<Snapshot> {
    let mut seed = 0xb42d_b35f_ae39_1de1;
    let mut next = || random_word(&mut seed);
    let mut inputs: Vec<_> = (0..16)
        .map(|index| {
            let mut input = Snapshot {
                gpr: std::array::from_fn(|_| next()),
                xmm: std::array::from_fn(|_| [next(), next()]),
                incoming: next(),
                ..Snapshot::default()
            };
            // Force both incoming carry values, independent of random chance.
            input.gpr[4] = if index % 2 == 0 { 0 } else { u64::MAX };
            input
        })
        .collect();
    let sign32 = 1 << 31;
    let sign64 = 1 << 63;
    for (index, (left, right)) in [
        (0, 0),
        (1, 1),
        (0, 1),
        (1, 0),
        (u64::MAX, 1),
        (u64::MAX, u64::MAX),
        (sign64 - 1, 1),
        (sign64, 1),
        (sign64, u64::MAX),
        (sign64, sign64),
        (sign32 - 1, 1),
        (sign32, 1),
        (sign32, sign32),
        (0xffff_ffff, 1),
        (0x7f, 1),
        (0x80, 1),
    ]
    .into_iter()
    .enumerate()
    {
        let mut input = inputs[index];
        input.gpr[0] = left;
        input.gpr[3] = right;
        input.gpr[1] = if index % 2 == 0 { 0 } else { sign64 };
        input.gpr[4] = if index % 2 == 0 { 0 } else { 1 };
        input.incoming = right;
        inputs.push(input);
    }
    inputs
}

#[test]
fn integer() {
    let context = Context::new(&Config::new());
    let mut suite = Suite::new(&context, ReturnClass::I64);
    let inputs = concrete_inputs();
    macro_rules! binary {
        ($flags:expr, $source:literal, $dest:literal; $($mnemonic:literal),+) => {
            $(
                form!(suite, &inputs, "reg/reg", $flags, concat!($mnemonic, " ", $source, ", ", $dest));
                form!(suite, &inputs, "imm/reg", $flags, concat!($mnemonic, " $-1, ", $dest));
                form!(suite, &inputs, "imm/reg", $flags, concat!($mnemonic, " $127, ", $dest));
            )+
        };
    }
    binary!(Flags::All, "%rsi", "%rax"; "adcq", "addq", "andq", "cmpq", "movq", "orq", "sbbq", "subq", "testq", "xorq");
    binary!(Flags::All, "%esi", "%eax"; "andl", "cmpl", "movl", "orl", "testl", "xorl");
    binary!(Flags::All, "%sil", "%al"; "cmpb", "testb");
    binary!(Flags::Unmodeled, "%rsi", "%rax"; "imulq");
    form!(
        suite,
        &inputs,
        "imm/reg/reg",
        Flags::Unmodeled,
        "imulq $-128, %rsi, %rax"
    );
    form!(
        suite,
        &inputs,
        "imm/reg/reg",
        Flags::Unmodeled,
        "imulq $2147483647, %rsi, %rax"
    );
    form!(suite, &inputs, "reg", Flags::Unmodeled, "imulq %rsi");
    form!(suite, &inputs, "reg", Flags::Unmodeled, "mulq %rsi");
    form!(suite, &inputs, "reg", Flags::All, "negq %rax");
    form!(suite, &inputs, "reg", Flags::All, "negl %eax");
    form!(suite, &inputs, "reg", Flags::Zero, "decq %rax");
    form!(suite, &inputs, "reg", Flags::Zero, "decl %eax");
    form!(suite, &inputs, "implicit", Flags::All, "cltd");
    form!(suite, &inputs, "implicit", Flags::All, "cqto");
    form!(
        suite,
        &inputs,
        "implicit/reg",
        Flags::ShiftImm(1),
        "sarq %rax"
    );
    macro_rules! immediate_shift {
        ($($count:literal),+) => {
            $(
                form!(suite, &inputs, "imm/reg", Flags::ShiftImm($count),
                    concat!("sarq $", stringify!($count), ", %rax"));
                form!(suite, &inputs, "imm/reg/reg", Flags::ShiftImm($count),
                    concat!("shldq $", stringify!($count), ", %rsi, %rax"));
            )+
        };
    }
    immediate_shift!(0, 1, 63, 64, 65, 255);
    let shift_inputs: Vec<_> = inputs
        .iter()
        .copied()
        .enumerate()
        .map(|(index, mut input)| {
            if index >= 16 {
                input.gpr[1] = [0, 1, 63, 64, 65, 127, 128, 255][index % 8];
            }
            input
        })
        .collect();
    form!(
        suite,
        &shift_inputs,
        "cl/reg",
        Flags::ShiftCl,
        "sarq %cl, %rax"
    );
    form!(
        suite,
        &shift_inputs,
        "cl/reg/reg",
        Flags::ShiftCl,
        "shldq %cl, %rsi, %rax"
    );

    // LLVM can also spell the same 64-bit SHLD without the width suffix.
    suite.check(
        "cl/reg/reg (alias)",
        &["shld %cl, %rsi, %rax"],
        Flags::ShiftCl,
        false,
        &shift_inputs,
        |input| cpu!(input; "shldq %cl, %rsi, %rax"),
    );
    suite.finish();
}

#[test]
fn stack_memory() {
    let context = Context::new(&Config::new());
    let mut suite = Suite::new(&context, ReturnClass::I64);
    let inputs = concrete_inputs();
    macro_rules! load_op {
        ($($mnemonic:literal),+) => {
            $(
                form!(suite, &inputs, "mem/reg", Flags::All, concat!($mnemonic, " 8(%rsp), %rax"));
            )+
        };
    }
    load_op!("addq", "andq", "cmpq", "movq", "orq", "subq", "testq", "xorq");
    form!(
        suite,
        &inputs,
        "mem/reg",
        Flags::Unmodeled,
        "imulq 8(%rsp), %rax"
    );
    form!(suite, &inputs, "reg/mem", Flags::All, "cmpq %rax, 8(%rsp)");
    form!(suite, &inputs, "reg/mem", Flags::All, "testq %rax, 8(%rsp)");
    form!(suite, &inputs, "imm/mem", Flags::All, "cmpq $-1, 8(%rsp)");
    form!(suite, &inputs, "imm/mem", Flags::All, "testq $-1, 8(%rsp)");
    macro_rules! stack_roundtrip {
        ($shape:literal, $($asm:literal),+ $(,)?) => {
            suite.check($shape, &[$($asm),+, "popq %rdx"], Flags::All, true, &inputs,
                |input| cpu!(input; $($asm),+; capture_then "popq %rdx"));
        };
    }
    stack_roundtrip!("reg", "pushq %rsi");
    stack_roundtrip!("imm", "pushq $-128");
    stack_roundtrip!("imm", "pushq $-2147483648");
    stack_roundtrip!("mem", "pushq 8(%rsp)");
    stack_roundtrip!("reg/mem", "pushq %rsi", "movq %rax, (%rsp)");
    stack_roundtrip!("imm/mem", "pushq %rsi", "movq $-1, (%rsp)");
    stack_roundtrip!("local mem/reg", "pushq %rsi", "movq (%rsp), %rax");
    check_retq(&mut suite, &inputs);
    suite.finish();
}

fn check_retq(suite: &mut Suite<'_>, inputs: &[Snapshot]) {
    // The CPU uses a local call only to supply a real return address.
    // Machine-TV's call abstraction models source calls, not this trampoline.
    suite.check(
        "implicit retq",
        &[],
        Flags::All,
        false,
        inputs,
        |input| cpu!(input; "callq 2f", "jmp 3f", "2:", "retq", "3:"),
    );
}

fn float_inputs() -> Vec<Snapshot> {
    let mut inputs = concrete_inputs();
    let snan = 0x7ff0_0000_0000_0123;
    let qnan = 0xfff8_0000_0000_0456;
    let pairs = [
        (0, (-0.0f64).to_bits()),
        ((-0.0f64).to_bits(), (-0.0f64).to_bits()),
        (1.0f64.to_bits(), 1.0f64.to_bits()),
        (1.0f64.to_bits(), 2.0f64.to_bits()),
        (2.0f64.to_bits(), 1.0f64.to_bits()),
        (1.0f64.to_bits(), (-1.0f64).to_bits()),
        (1.0f64.to_bits(), f64::EPSILON.to_bits() - (1 << 52)),
        (f64::MIN_POSITIVE.to_bits(), 1),
        (1, 1),
        (f64::MAX.to_bits(), f64::MAX.to_bits()),
        (f64::INFINITY.to_bits(), f64::NEG_INFINITY.to_bits()),
        (f64::INFINITY.to_bits(), f64::INFINITY.to_bits()),
        (0, f64::INFINITY.to_bits()),
        (1.0f64.to_bits(), (-0.0f64).to_bits()),
        (qnan, 1.0f64.to_bits()),
        (1.0f64.to_bits(), snan),
        (qnan, snan),
        (snan, qnan),
    ];
    for (index, (left, right)) in pairs.into_iter().enumerate() {
        let mut input = inputs[index];
        input.xmm[0][0] = left;
        input.xmm[1][0] = right;
        inputs.push(input);
    }
    for input in &mut inputs {
        input.incoming = input.xmm[1][0];
    }
    inputs
}

#[test]
fn scalar_sse() {
    let context = Context::new(&Config::new());
    let mut suite = Suite::new(&context, ReturnClass::F64);
    let inputs = float_inputs();
    macro_rules! sse {
        ($flags:expr; $($mnemonic:literal),+) => {
            $(
                form!(suite, &inputs, "xmm/xmm", $flags, concat!($mnemonic, " %xmm1, %xmm0"));
                form!(suite, &inputs, "mem/xmm", $flags, concat!($mnemonic, " 8(%rsp), %xmm0"));
            )+
        };
    }
    sse!(Flags::All; "addsd", "subsd", "mulsd", "divsd", "movsd");
    sse!(Flags::Float; "comisd", "ucomisd");
    form!(suite, &inputs, "xmm/reg", Flags::All, "movq %xmm1, %rax");
    form!(suite, &inputs, "reg/xmm", Flags::All, "movq %rax, %xmm1");
    suite.check(
        "xmm/mem",
        &["pushq %rsi", "movsd %xmm0, (%rsp)", "popq %rdx"],
        Flags::All,
        true,
        &inputs,
        |input| cpu!(input; "pushq %rsi", "movsd %xmm0, (%rsp)"; capture_then "popq %rdx"),
    );
    check_retq(&mut suite, &inputs);
    suite.finish();
}

#[test]
fn conditions() {
    let context = Context::new(&Config::new());
    let mut integers = Suite::new(&context, ReturnClass::I64);
    let mut floats = Suite::new(&context, ReturnClass::F64);
    let integer_inputs = concrete_inputs();
    let float_inputs = float_inputs();
    macro_rules! both_forms {
        ($suite:ident, $inputs:ident, $compare:literal, $flags:expr, $suffix:literal) => {
            form!($suite, &$inputs, "reg8", $flags, $compare, concat!("set", $suffix, " %r8b"));
            $suite.check("relative branch", &[$compare, "movl $1, %r8d", concat!("j", $suffix, " 1"), "movl $0, %r8d"],
                $flags, false, &$inputs, |input| cpu!(input;
                    $compare, "movl $1, %r8d", concat!("j", $suffix, " 2f"), "movl $0, %r8d", "2:"));
        };
    }
    macro_rules! conditions {
        ($($suffix:literal),+) => {$(
            both_forms!(integers, integer_inputs, "cmpq %rsi, %rax", Flags::All, $suffix);
            both_forms!(floats, float_inputs, "ucomisd %xmm1, %xmm0", Flags::Float, $suffix);
        )+};
    }
    conditions!(
        "e", "z", "ne", "nz", "l", "nge", "le", "ng", "g", "nle", "ge", "nl", "b", "c", "nae",
        "be", "na", "a", "nbe", "ae", "nb", "nc", "s", "ns"
    );
    both_forms!(
        floats,
        float_inputs,
        "ucomisd %xmm1, %xmm0",
        Flags::Float,
        "p"
    );
    both_forms!(
        floats,
        float_inputs,
        "ucomisd %xmm1, %xmm0",
        Flags::Float,
        "pe"
    );
    both_forms!(
        floats,
        float_inputs,
        "ucomisd %xmm1, %xmm0",
        Flags::Float,
        "np"
    );
    both_forms!(
        floats,
        float_inputs,
        "ucomisd %xmm1, %xmm0",
        Flags::Float,
        "po"
    );
    integers.finish();
    floats.finish();
}

#[test]
fn division() {
    let context = Context::new(&Config::new());
    let mut suite = Suite::new(&context, ReturnClass::I64);
    let inputs = concrete_inputs();
    let unsigned = |narrow: bool| {
        inputs
            .iter()
            .copied()
            .enumerate()
            .map(|(index, mut input)| {
                let divisor = if narrow {
                    input.gpr[3] & 0xffff_ffff
                } else {
                    input.gpr[3]
                };
                let divisor = divisor.max(1);
                // A high half smaller than the divisor guarantees the quotient fits.
                let high = if index % 2 == 0 {
                    0
                } else {
                    input.gpr[2] % divisor
                };
                input.gpr[3] = if narrow {
                    divisor | (input.gpr[3] & !0xffff_ffff)
                } else {
                    divisor
                };
                input.gpr[2] = if narrow {
                    high | (input.gpr[2] & !0xffff_ffff)
                } else {
                    high
                };
                input
            })
            .collect::<Vec<_>>()
    };
    let signed: Vec<_> = inputs
        .iter()
        .copied()
        .enumerate()
        .map(|(index, mut input)| {
            let mut divisor = input.gpr[3] as i64;
            if divisor == 0 || (input.gpr[0] == i64::MIN as u64 && divisor == -1) {
                divisor = 1;
            }
            input.gpr[3] = divisor as u64;
            let dividend = i128::from(input.gpr[0] as i64);
            // Alternate sign-extended 64-bit and full 128-bit dividends whose
            // exact quotient is an i64; neither can raise a hardware #DE.
            let dividend = if index % 2 == 0 {
                dividend
            } else {
                dividend * i128::from(divisor)
            };
            input.gpr[0] = dividend as u64;
            input.gpr[2] = (dividend >> 64) as u64;
            input
        })
        .collect();
    form!(
        suite,
        &unsigned(true),
        "reg32",
        Flags::Unmodeled,
        "divl %esi"
    );
    form!(
        suite,
        &unsigned(false),
        "reg64",
        Flags::Unmodeled,
        "divq %rsi"
    );
    form!(suite, &signed, "reg64", Flags::Unmodeled, "idivq %rsi");

    // Check traps only in Machine-TV; never feed them to inline assembly.
    for (op, overflow) in [
        ("divl %esi", [0, 1, 1]),
        ("divq %rsi", [0, 1, 1]),
        ("idivq %rsi", [i64::MIN as u64, u64::MAX, u64::MAX]),
    ] {
        for (trap, [low, high, divisor]) in
            [("zero divisor", [0; 3]), ("quotient overflow", overflow)]
        {
            let mut input = Snapshot::default();
            [input.gpr[0], input.gpr[2], input.gpr[3]] = [low, high, divisor];
            let result = lift_case(&suite.model, &instructions(&[op]), input, false, &context);
            assert_eq!(
                result.err(),
                Some(FunctionOutcome::OutsideProfile("division_trap")),
                "({op}, register, {trap}, input={input:#x?})"
            );
        }
    }
    suite.finish();
}

/// Formal pilot for one instruction against a separately written reference
/// transition. Unlike the CPU differential tests above, this quantifies over
/// every 64-bit input bit-vector in Z3 instead of sampling a CPU run.
///
/// Reference scope for `addq %rsi, %rdi`: the 64-bit destination result, CF
/// and OF. AF, PF, memory effects, control flow, instruction bytes and all
/// other registers/flags are outside this pilot and are not treated as proved.
#[test]
fn addq_register_form_matches_independent_reference_semantics() {
    let context = Context::new(&Config::new());
    let suite = Suite::new(&context, ReturnClass::I64);
    let instructions = instructions(&["addq %rsi, %rdi"]);
    let end = instructions.len() - 1;
    let mut probe = Probe::symbolic(end, &context);
    let mut lifted = lift_inner(
        &instructions,
        instructions.len() as u64,
        &suite.model,
        &BTreeMap::new(),
        &context,
        Some(&mut probe),
    )
    .expect("the narrow addq pilot stays inside the lifter profile");
    assert_eq!(lifted.paths.len(), 1);
    let path = lifted.paths.pop().unwrap();
    assert!(path.path_conditions.is_empty());
    let state = probe.final_state.expect("addq reaches retq");
    let initial = probe.symbolic_registers.expect("symbolic pilot seeds");
    let left = &initial[7]; // rdi
    let right = &initial[6]; // rsi
    let expected = left.bvadd(right);

    let solver = Solver::new(&context);
    solver.assert(&state.registers[7]._eq(&expected).not());
    assert_eq!(
        solver.check(),
        SatResult::Unsat,
        "destination result differs from the independent addq reference"
    );

    let FlagState::Arithmetic {
        value,
        carry,
        overflow,
        width: 64,
    } = &state.flags
    else {
        panic!("addq did not produce arithmetic flags");
    };

    let solver = Solver::new(&context);
    solver.assert(&value._eq(&expected).not());
    assert_eq!(
        solver.check(),
        SatResult::Unsat,
        "flag value differs from the independent addq reference"
    );

    // Independent reference: unsigned addition carried exactly when the
    // wrapped result is below the original destination operand.
    let expected_carry = expected.bvult(left);
    let solver = Solver::new(&context);
    solver.assert(&carry._eq(&expected_carry).not());
    assert_eq!(
        solver.check(),
        SatResult::Unsat,
        "CF differs from the independent addq reference"
    );

    let one = BV::from_u64(&context, 1, 1);
    let left_sign = left.extract(63, 63)._eq(&one);
    let right_sign = right.extract(63, 63)._eq(&one);
    let result_sign = expected.extract(63, 63)._eq(&one);
    let same_sign = left_sign._eq(&right_sign);
    let result_sign_changed = result_sign._eq(&left_sign).not();
    let expected_overflow = Bool::and(&context, &[&same_sign, &result_sign_changed]);
    let solver = Solver::new(&context);
    solver.assert(&overflow._eq(&expected_overflow).not());
    assert_eq!(
        solver.check(),
        SatResult::Unsat,
        "OF differs from the independent addq reference"
    );
}
