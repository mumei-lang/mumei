//! Opt-in equivalence checking of one C03 body and one supplied LLVM 17 SSA snapshot.
//! Not postcondition verification, pipeline attestation or final-binary verification.

use inkwell::{
    context::Context as LlvmContext, memory_buffer::MemoryBuffer, module::Linkage,
    values::AsValueRef,
};
use llvm_sys::{core::*, prelude::*, LLVMIntPredicate, LLVMOpcode, LLVMTypeKind, LLVMUnnamedAddr};
use mumei_core::semantic_contract::{
    aggregate_value, call_field_decl, encode_pure_i64_body, encode_pure_i64_module,
    module_source_profile, CallValueShape, PureI64Body, RefinedAdmission, LOOP_BOUND,
    PURE_I64_WRAPPING_PROFILE,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use z3::{
    ast::{Array, Ast, Bool, BV},
    Config, Context, SatResult, Solver,
};

pub const PROFILE: &str = "mumei.llvm17-pure-i64-ssa-equivalence/v1";
pub const NATIVE_PROFILE: &str = "mumei.llvm17-native-main-snapshot/v1";
pub const NATIVE_MODULE_PROFILE: &str = "mumei.llvm17-native-module-snapshot/v1";
/// Bounded-loop variants: the source contains an admitted `while`, so
/// the claim covers only inputs where every loop's guard fails within
/// `LOOP_BOUND` unrolled iterations — on the LLVM side a path revisiting
/// a block past the same bound is dropped out of profile.
pub const BOUNDED_LOOPS_PROFILE: &str = "mumei.llvm17-pure-i64-ssa-equivalence-bounded-loops/v1";
pub const BOUNDED_LOOPS_NATIVE_PROFILE: &str = "mumei.llvm17-native-main-snapshot-bounded-loops/v1";
pub const BOUNDED_LOOPS_NATIVE_MODULE_PROFILE: &str =
    "mumei.llvm17-native-module-snapshot-bounded-loops/v1";
/// Bounded-memory variants: the source model holds admitted `let`-bound
/// arrays as Z3 arrays of i64 cells, and the IR whitelist admits the
/// emitter's `alloca`/`getelementptr`/`load`/`store` lowering of them —
/// pointer values still fail closed everywhere else.
pub const BOUNDED_MEMORY_PROFILE: &str = "mumei.llvm17-pure-i64-ssa-equivalence-bounded-memory/v1";
pub const BOUNDED_MEMORY_NATIVE_PROFILE: &str =
    "mumei.llvm17-native-main-snapshot-bounded-memory/v1";
pub const BOUNDED_MEMORY_NATIVE_MODULE_PROFILE: &str =
    "mumei.llvm17-native-module-snapshot-bounded-memory/v1";
/// A bounded-memory source may also contain an admitted `while`; the
/// combined variant names both relaxations.
pub const BOUNDED_MEMORY_LOOPS_PROFILE: &str =
    "mumei.llvm17-pure-i64-ssa-equivalence-bounded-loops-bounded-memory/v1";
pub const BOUNDED_MEMORY_LOOPS_NATIVE_PROFILE: &str =
    "mumei.llvm17-native-main-snapshot-bounded-loops-bounded-memory/v1";
pub const BOUNDED_MEMORY_LOOPS_NATIVE_MODULE_PROFILE: &str =
    "mumei.llvm17-native-module-snapshot-bounded-loops-bounded-memory/v1";
/// Post-optimization variants: the supplied IR is a `default<O2>`
/// pipeline output, pinned in the profile name. The whitelist
/// additionally admits the canonical forms that pipeline produces —
/// `tail` call markers, descriptive function-scope attributes,
/// `freeze`, and the `llvm.{u,s}{min,max}` / `llvm.abs` /
/// `llvm.*.with.overflow` i64 intrinsics — and the encoder gives each
/// its wrapping-source semantics.
pub const OPT_PROFILE: &str = "mumei.llvm17-pure-i64-ssa-equivalence-opt-o2/v1";
pub const OPT_BOUNDED_LOOPS_PROFILE: &str =
    "mumei.llvm17-pure-i64-ssa-equivalence-bounded-loops-opt-o2/v1";
pub const OPT_BOUNDED_MEMORY_PROFILE: &str =
    "mumei.llvm17-pure-i64-ssa-equivalence-bounded-memory-opt-o2/v1";
pub const OPT_BOUNDED_MEMORY_LOOPS_PROFILE: &str =
    "mumei.llvm17-pure-i64-ssa-equivalence-bounded-loops-bounded-memory-opt-o2/v1";
/// `default<O2>` canonical scalar intrinsics the `opt-o2` profiles
/// admit: `(name, parameter widths, result width)`. `llvm.abs.i64`
/// carries its `i1 immarg` poison flag as the second parameter.
const OPT_SCALAR_INTRINSICS: &[(&str, &[u32], u32)] = &[
    ("llvm.umin.i64", &[64, 64], 64),
    ("llvm.umax.i64", &[64, 64], 64),
    ("llvm.smin.i64", &[64, 64], 64),
    ("llvm.smax.i64", &[64, 64], 64),
    ("llvm.abs.i64", &[64, 1], 64),
];
/// The `*.with.overflow.i64` family returns `{i64, i1}` — the wrapped
/// result and the overflow flag.
const OPT_OVERFLOW_INTRINSICS: &[&str] = &[
    "llvm.sadd.with.overflow.i64",
    "llvm.ssub.with.overflow.i64",
    "llvm.smul.with.overflow.i64",
    "llvm.uadd.with.overflow.i64",
    "llvm.usub.with.overflow.i64",
    "llvm.umul.with.overflow.i64",
];

fn is_opt_intrinsic(name: &str) -> bool {
    OPT_SCALAR_INTRINSICS.iter().any(|(n, ..)| n == &name)
        || OPT_OVERFLOW_INTRINSICS.contains(&name)
}

/// The literal `{i64, i1}` a `*.with.overflow.i64` call returns — the
/// only aggregate shape the `opt-o2` profiles admit that the base
/// profiles' `aggregate_fields` does not.
unsafe fn overflow_pair(ty: LLVMTypeRef) -> bool {
    if LLVMGetTypeKind(ty) != LLVMTypeKind::LLVMStructTypeKind
        || LLVMIsPackedStruct(ty) != 0
        || LLVMIsLiteralStruct(ty) == 0
        || LLVMCountStructElementTypes(ty) != 2
    {
        return false;
    }
    let first = LLVMStructGetTypeAtIndex(ty, 0);
    let second = LLVMStructGetTypeAtIndex(ty, 1);
    LLVMGetTypeKind(first) == LLVMTypeKind::LLVMIntegerTypeKind
        && LLVMGetIntTypeWidth(first) == 64
        && LLVMGetTypeKind(second) == LLVMTypeKind::LLVMIntegerTypeKind
        && LLVMGetIntTypeWidth(second) == 1
}
/// The largest array an `alloca` may declare — bounds the SMT array's
/// concrete extent the same way `MAX_INSTRUCTIONS` bounds the CFG.
const MAX_ARRAY_SLOTS: u32 = 256;
pub const NATIVE_TARGET: &str = "x86_64-pc-linux-gnu";
pub const TARGET: &str = "x86_64-unknown-linux-gnu";
pub const DATA_LAYOUT: &str =
    "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128";
const MAX_BYTES: usize = 65_536;
const MAX_DEPTH: usize = 48;
const MAX_BLOCKS: usize = 64;
const MAX_INSTRUCTIONS: usize = 1024;
const MAX_STATES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IrFunctionSurface {
    pub defined: BTreeSet<String>,
    pub declared: BTreeSet<String>,
    pub calls: BTreeMap<String, BTreeSet<String>>,
    /// libc calls the backend synthesizes from `llvm.*` intrinsics, by
    /// caller — `llvm.memcpy.*`/`llvm.memmove.*`/`llvm.memset.*` lower to
    /// `memcpy`/`memmove`/`memset` edges that never appear in `calls` or
    /// `declared`.
    pub libc_calls: BTreeMap<String, BTreeSet<String>>,
    pub call_edges_checkable: bool,
}

/// The libc symbol a `llvm.*` memory intrinsic lowers to in the object
/// file, when the backend emits it as a call rather than inline code.
fn intrinsic_libc_name(callee: &str) -> Option<&'static str> {
    for (intrinsic, libc) in [
        ("llvm.memcpy.", "memcpy"),
        ("llvm.memmove.", "memmove"),
        ("llvm.memset.", "memset"),
    ] {
        if callee.starts_with(intrinsic) {
            return Some(libc);
        }
    }
    None
}

#[allow(clippy::result_unit_err)]
pub fn ir_function_surface(ir: &[u8]) -> std::result::Result<IrFunctionSurface, ()> {
    let context = LlvmContext::create();
    let module = context
        .create_module_from_ir(MemoryBuffer::create_from_memory_range_copy(
            ir,
            "function-surface.ll",
        ))
        .map_err(|_| ())?;
    let mut surface = IrFunctionSurface {
        call_edges_checkable: true,
        ..IrFunctionSurface::default()
    };
    for function in module.get_functions() {
        let name = function.get_name().to_str().map_err(|_| ())?.to_owned();
        if name.starts_with("llvm.") {
            continue;
        }
        if function.count_basic_blocks() > 0 {
            if matches!(function.get_linkage(), Linkage::Private | Linkage::Internal) {
                surface.call_edges_checkable = false;
                continue;
            }
            surface.defined.insert(name.clone());
            let mut callees = BTreeSet::new();
            unsafe {
                let mut block = LLVMGetFirstBasicBlock(function.as_value_ref());
                while !block.is_null() {
                    let mut instruction = LLVMGetFirstInstruction(block);
                    while !instruction.is_null() {
                        if LLVMGetInstructionOpcode(instruction) == LLVMOpcode::LLVMCall {
                            let callee = LLVMGetCalledValue(instruction);
                            if callee.is_null() || LLVMIsAFunction(callee).is_null() {
                                surface.call_edges_checkable = false;
                            } else if let Some(callee_name) = callee_name(callee) {
                                if callee_name.is_empty() {
                                    surface.call_edges_checkable = false;
                                } else if !callee_name.starts_with("llvm.") {
                                    callees.insert(callee_name);
                                } else if let Some(libc) = intrinsic_libc_name(&callee_name) {
                                    surface
                                        .libc_calls
                                        .entry(name.clone())
                                        .or_default()
                                        .insert(libc.to_owned());
                                }
                            } else {
                                surface.call_edges_checkable = false;
                            }
                        }
                        instruction = LLVMGetNextInstruction(instruction);
                    }
                    block = LLVMGetNextBasicBlock(block);
                }
            }
            surface.calls.insert(name, callees);
        } else {
            surface.declared.insert(name);
        }
    }
    Ok(surface)
}

/// Pinned-IR data globals with their derived initializer bytes, for
/// the object-side data-section correspondence check
/// (DESIGN_LLVM_OBJECT_CORRESPONDENCE.md Step 3). Every global that
/// carries an initializer maps to `Some` of its byte content, or
/// `None` when the initializer's shape is outside the derivable
/// subset — the caller must treat those as unverifiable, never as
/// zero-length data. Globals without an initializer (`extern`
/// declarations) emit no data and are omitted.
#[allow(clippy::result_unit_err)]
pub fn ir_data_globals(ir: &[u8]) -> std::result::Result<BTreeMap<String, Option<Vec<u8>>>, ()> {
    let context = LlvmContext::create();
    let module = context
        .create_module_from_ir(MemoryBuffer::create_from_memory_range_copy(
            ir,
            "data-globals.ll",
        ))
        .map_err(|_| ())?;
    let mut globals = BTreeMap::new();
    for global in module.get_globals() {
        let name = global.get_name().to_str().map_err(|_| ())?.to_owned();
        if name.starts_with("llvm.") {
            continue;
        }
        let Some(initializer) = global.get_initializer() else {
            continue;
        };
        globals.insert(name, unsafe {
            data_initializer_bytes(initializer.as_value_ref())
        });
    }
    Ok(globals)
}

/// ELF binding class an IR linkage maps to, or `None` when the
/// linkage has no byte-exact object-side counterpart
/// (`available_externally`/`appending` emit no object symbol; DLL and
/// ghost variants are platform-scoped). `common` tentatives are
/// STB_GLOBAL on ELF.
fn linkage_expected_binding(linkage: Linkage) -> Option<&'static str> {
    match linkage {
        Linkage::External | Linkage::Common => Some("global"),
        Linkage::Internal
        | Linkage::Private
        | Linkage::LinkerPrivate
        | Linkage::LinkerPrivateWeak => Some("local"),
        Linkage::WeakAny
        | Linkage::WeakODR
        | Linkage::ExternalWeak
        | Linkage::LinkOnceAny
        | Linkage::LinkOnceODR
        | Linkage::LinkOnceODRAutoHide => Some("weak"),
        _ => None,
    }
}

/// Defined named IR entities (functions and globals) with the
/// object-symbol binding class their linkage maps to, for the
/// object-side symbol-attribute correspondence check
/// (DESIGN_LLVM_OBJECT_CORRESPONDENCE.md Step 3'). A `None` value
/// marks a linkage with no derivable binding — the caller must treat
/// those as unverifiable. Entities carrying `unnamed_addr` are
/// excluded entirely: LLVM may merge or drop their symbols so no
/// binding claim survives the object step. For globals the bytes are
/// covered by the data-section correspondence check; `unnamed_addr`
/// functions simply carry no attribute claim.
#[derive(Clone, Default)]
pub struct IrSymbolAttributes {
    pub expected: BTreeMap<String, Option<&'static str>>,
    pub unnamed_addr_excluded: usize,
}

#[allow(clippy::result_unit_err)]
pub fn ir_symbol_attributes(ir: &[u8]) -> std::result::Result<IrSymbolAttributes, ()> {
    let context = LlvmContext::create();
    let module = context
        .create_module_from_ir(MemoryBuffer::create_from_memory_range_copy(
            ir,
            "symbol-attributes.ll",
        ))
        .map_err(|_| ())?;
    let mut attributes = IrSymbolAttributes::default();
    for function in module.get_functions() {
        if function.count_basic_blocks() == 0 {
            continue;
        }
        let name = function.get_name().to_str().map_err(|_| ())?.to_owned();
        if name.starts_with("llvm.") {
            continue;
        }
        if unsafe { LLVMGetUnnamedAddress(function.as_value_ref()) }
            != LLVMUnnamedAddr::LLVMNoUnnamedAddr
        {
            attributes.unnamed_addr_excluded += 1;
            continue;
        }
        attributes
            .expected
            .insert(name, linkage_expected_binding(function.get_linkage()));
    }
    for global in module.get_globals() {
        if global.get_initializer().is_none() {
            continue;
        }
        let name = global.get_name().to_str().map_err(|_| ())?.to_owned();
        if name.starts_with("llvm.") {
            continue;
        }
        if unsafe { LLVMGetUnnamedAddress(global.as_value_ref()) }
            != LLVMUnnamedAddr::LLVMNoUnnamedAddr
        {
            attributes.unnamed_addr_excluded += 1;
            continue;
        }
        let linkage: Linkage = unsafe { LLVMGetLinkage(global.as_value_ref()) }.into();
        attributes
            .expected
            .insert(name, linkage_expected_binding(linkage));
    }
    Ok(attributes)
}

/// Byte content of a constant initializer, or `None` when the shape
/// cannot be rendered byte-exactly without a target data layout
/// (pointers, structs, non-native floats, integer widths whose
/// byte-store size is not a power of two).
unsafe fn data_initializer_bytes(value: LLVMValueRef) -> Option<Vec<u8>> {
    if !LLVMIsAConstantDataSequential(value).is_null() {
        // `LLVMGetAsString` covers i8-element strings (the `str_lit`
        // globals); wider element kinds return null — fall back to
        // per-element derivation.
        let mut len = 0usize;
        let ptr = LLVMGetAsString(value, &mut len);
        if !ptr.is_null() {
            return Some(std::slice::from_raw_parts(ptr.cast::<u8>(), len).to_vec());
        }
        let count = LLVMGetArrayLength2(LLVMTypeOf(value));
        let mut out = Vec::new();
        for index in 0..count {
            let element = LLVMGetAggregateElement(value, index as u32);
            if element.is_null() {
                return None;
            }
            out.extend(data_initializer_bytes(element)?);
        }
        return Some(out);
    }
    if !LLVMIsAConstantInt(value).is_null() {
        let width = LLVMGetIntTypeWidth(LLVMTypeOf(value));
        if width == 0 || width > 64 {
            return None;
        }
        // LLVM pads an integer's object allocation up to the next
        // power-of-two alignment (`i24` stores 3 bytes but allocates
        // 4). Only widths whose store size is itself a power of two
        // have store size == alloc size, so they stay byte-exact
        // without the target data layout; `i24`/`i48`/... report
        // underivable rather than comparing against padded bytes.
        let bytes = width.div_ceil(8);
        if bytes > 8 || !bytes.is_power_of_two() {
            return None;
        }
        let bits = LLVMConstIntGetZExtValue(value);
        return Some(bits.to_le_bytes()[..bytes as usize].to_vec());
    }
    if !LLVMIsAConstantFP(value).is_null() {
        let mut loses_flag = 0;
        let real = LLVMConstRealGetDouble(value, &mut loses_flag);
        return Some(match LLVMGetTypeKind(LLVMTypeOf(value)) {
            LLVMTypeKind::LLVMDoubleTypeKind => real.to_le_bytes().to_vec(),
            LLVMTypeKind::LLVMFloatTypeKind => (real as f32).to_le_bytes().to_vec(),
            _ => return None,
        });
    }
    if !LLVMIsAConstantAggregateZero(value).is_null() || LLVMIsNull(value) != 0 {
        return Some(vec![0; type_store_bytes(LLVMTypeOf(value))?]);
    }
    if !LLVMIsAConstantArray(value).is_null() {
        // Uniform element types leave no padding to model; structs
        // would need the data layout, so they stay underivable.
        let count = LLVMGetNumOperands(value);
        let mut out = Vec::new();
        for index in 0..count {
            let element = LLVMGetOperand(value, index as u32);
            if element.is_null() {
                return None;
            }
            out.extend(data_initializer_bytes(element)?);
        }
        return Some(out);
    }
    None
}

/// Bytes a zero-initialized value of `ty` occupies in a data section.
/// Only shapes whose store size is derivable without a data layout.
unsafe fn type_store_bytes(ty: LLVMTypeRef) -> Option<usize> {
    match LLVMGetTypeKind(ty) {
        LLVMTypeKind::LLVMIntegerTypeKind => {
            let width = LLVMGetIntTypeWidth(ty);
            let bytes = width.div_ceil(8);
            (width != 0 && bytes <= 8 && bytes.is_power_of_two()).then_some(bytes as usize)
        }
        LLVMTypeKind::LLVMFloatTypeKind => Some(4),
        LLVMTypeKind::LLVMDoubleTypeKind => Some(8),
        LLVMTypeKind::LLVMArrayTypeKind => {
            Some(type_store_bytes(LLVMGetElementType(ty))? * LLVMGetArrayLength2(ty) as usize)
        }
        _ => None,
    }
}

#[cfg(test)]
mod ir_function_surface_tests {
    use super::*;

    #[test]
    fn function_surface_keeps_external_definitions_and_declarations_only() {
        let ir = br#"
define i32 @visible() {
entry:
  ret i32 0
}
declare i32 @external(i32)
define internal i32 @internal_fn() {
entry:
  ret i32 1
}
declare void @llvm.fake.intrinsic()
"#;
        let surface = ir_function_surface(ir).unwrap();
        assert_eq!(surface.defined, BTreeSet::from(["visible".to_owned()]));
        assert_eq!(surface.declared, BTreeSet::from(["external".to_owned()]));
        assert_eq!(
            surface.calls,
            BTreeMap::from([("visible".to_owned(), BTreeSet::new())])
        );
        assert!(!surface.call_edges_checkable);
    }

    #[test]
    fn independent_query_aggregation_keeps_scanning_after_unknown() {
        let context = Context::new(&Config::new());
        let terms = vec![
            Bool::from_bool(&context, true),
            Bool::from_bool(&context, false),
        ];
        for (results, expected) in [
            (vec![SatResult::Unknown, SatResult::Sat], SatResult::Sat),
            (
                vec![SatResult::Unknown, SatResult::Unsat],
                SatResult::Unknown,
            ),
            (vec![SatResult::Unsat, SatResult::Unsat], SatResult::Unsat),
        ] {
            let mut results = results.into_iter();
            assert_eq!(
                check_independent(&terms, |_| results.next().unwrap()),
                expected
            );
        }
    }
}

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
    pub input_domain: String,
    /// The bounded `while` unroll the equivalence claim covers —
    /// `Some(LOOP_BOUND)` when the source admits a loop, `None`
    /// otherwise, so consumers read the coverage limit without
    /// parsing `input_domain`.
    pub unroll_bound: Option<usize>,
    pub source_sha256: String,
    pub llvm_sha256: String,
    pub timeout_ms: u32,
    pub resource_limit: u32,
    pub outcome: Outcome,
}

/// One atom's outcome inside `validate_pure_i64_module`.
#[derive(Debug)]
pub struct FunctionOutcome {
    pub function_name: String,
    pub outcome: Outcome,
}

/// Whole-module report: one outcome per source atom in declaration order
/// plus the aggregate (`Different` > `UndefinedBehavior` > `Inconclusive`
/// > `EquivalentWithinProfile`).
#[derive(Debug)]
pub struct ModuleValidationReport {
    pub profile: &'static str,
    pub source_profile: &'static str,
    pub target: &'static str,
    pub data_layout: &'static str,
    pub checked_stage: &'static str,
    pub input_domain: String,
    /// The bounded `while` unroll the equivalence claim covers —
    /// `Some(LOOP_BOUND)` when any member admits a loop.
    pub unroll_bound: Option<usize>,
    pub source_sha256: String,
    pub llvm_sha256: String,
    pub timeout_ms: u32,
    pub resource_limit: u32,
    pub functions: Vec<FunctionOutcome>,
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
    validate(source, llvm_ir, options, false, false)
}

/// Compare a zero-argument C03 main body with __mumei_user_main in the exact
/// supplied snapshot. Check only the call/trunc/ret shape of its C main wrapper;
/// no object, linking, OS exit-status or final-executable semantics are attested.
pub fn validate_native_main(
    source: &str,
    llvm_ir: &str,
    options: Options,
) -> Result<ValidationReport> {
    validate(source, llvm_ir, options, true, false)
}

/// Compare the source against `llvm_ir` produced by the `default<O2>`
/// pipeline, pinning that pipeline in the profile name. The whitelist
/// additionally admits the canonical forms that pipeline produces and
/// the encoder gives each its wrapping-source semantics; everything
/// outside those forms still fails closed.
pub fn validate_pure_i64_optimized(
    source: &str,
    llvm_ir: &str,
    options: Options,
) -> Result<ValidationReport> {
    validate(source, llvm_ir, options, false, true)
}

/// Compare a `HirAtom`'s scalar leaf body with the same-named
/// function in `llvm_ir` — the `hir → llvm_ir` correspondence check.
/// Admission is the leaf subset only (`encode_hir_pure_i64_leaf`):
/// uniform-width scalar signatures (`i64`, `i128`/`u128` at their
/// two-slot carriers, or `f64` pass-through leaves), `let` bindings,
/// short-circuit `&&`/`||`/`=>`, tail `if` branches, i64-carrying
/// uninterpreted call sites (ordered operations over a call result
/// stay outside; `calls` carries the module's callee signatures so a
/// composite parameter or result flattens to its leaf slots the way
/// the IR `call` encoding splices them), and read-only
/// `[i64]` parameters inside the i64 domain (`a[i]` as the
/// bounds-checked read, `len(a)` as the stored count), and
/// register-resident aggregates whose fields are all `i64` leaves —
/// `structs` names them with their declaration-order
/// `(field name, field type)` pairs, so a `Pair` parameter binds one
/// input slot per leaf and a `Pair` result packs via
/// `aggregate_value`. An entry that is not a non-empty all-`i64`
/// record is dropped from the admission, so a signature or literal
/// naming it fails closed at the encoder's type gates. Anything wider fails closed as a
/// validation error.
///
/// The module sweep covers every member, not just the compared
/// function: an unrelated member carrying `{i64, ptr}` array
/// parameters fails the whole module — admission is per module, not
/// per member. The return is the bare outcome:
/// this is the stage check, not a profile attestation.
pub fn validate_hir_pure_i64_leaf(
    atom: &mumei_core::hir::HirAtom,
    structs: &std::collections::BTreeMap<String, Vec<(String, String)>>,
    calls: &std::collections::BTreeMap<String, mumei_core::semantic_contract::HirCallLeafSig>,
    llvm_ir: &str,
    // `true` only on the native-bundle recheck path: the bundle
    // emitter renames the user `main` member to `__mumei_user_main`,
    // so a source `main()` call must meet that member. Plain IR
    // comparisons keep source names as they are.
    native_main_rename: bool,
    options: Options,
) -> Result<Outcome> {
    bounded(llvm_ir)?;
    if options.timeout_ms == 0 || options.timeout_ms > 30_000 || options.resource_limit == 0 {
        return Err(ValidationError("invalid solver budget"));
    }
    let mut config = Config::new();
    config.set_timeout_msec(options.timeout_ms as u64);
    config.set_param_value("rlimit", &options.resource_limit.to_string());
    let context = Context::new(&config);
    let model =
        mumei_core::semantic_contract::encode_hir_pure_i64_leaf(atom, structs, calls, &context)
            .map_err(|_| ValidationError("hir atom outside the scalar-i64 leaf subset"))?;
    let llvm_context = LlvmContext::create();
    let module = llvm_context
        .create_module_from_ir(MemoryBuffer::create_from_memory_range_copy(
            llvm_ir.as_bytes(),
            "hir-comparison.ll",
        ))
        .map_err(|_| ValidationError("invalid LLVM IR"))?;
    module
        .verify()
        .map_err(|_| ValidationError("invalid LLVM module"))?;
    // A declared target or data layout must match the emitter's — a
    // foreign layout could truncate GEP indices the array model reads
    // as 64-bit. A module declaring neither gets LLVM's defaults, like
    // the pinned fixture modules the tests compare. The native
    // triple joins TARGET because the recheck compares bundle
    // modules emitted under it (`validate_native_module` admits the
    // same pair).
    let triple = module.get_triple();
    let layout = module.get_data_layout();
    if (!triple.as_str().is_empty()
        && !matches!(
            triple.as_str().to_bytes(),
            bytes if bytes == TARGET.as_bytes() || bytes == NATIVE_TARGET.as_bytes()
        ))
        || (!layout.as_str().is_empty() && layout.as_str().to_bytes() != DATA_LAYOUT.as_bytes())
    {
        return Err(ValidationError("unsupported target or data layout"));
    }
    let function = module
        .get_function(&model.function_name)
        .ok_or(ValidationError("missing source function"))?;
    // The module is parsed/verified and remains alive for every raw LLVM query.
    let (outcome, _) = unsafe {
        check_module(
            module.as_mut_ptr(),
            model.bounded_memory,
            false,
            false,
            false,
            &BTreeMap::from([(
                model.function_name.clone(),
                model.array_params.iter().map(Option::is_some).collect(),
            )]),
            model.array_params.iter().any(|a| a.is_some()),
        )?;
        compare_function(
            &context,
            function.as_value_ref(),
            &model,
            &if native_main_rename {
                native_call_renames(true)
            } else {
                BTreeMap::new()
            },
            false,
            false,
            model.array_params.iter().any(|a| a.is_some()),
        )?
    };
    Ok(outcome)
}

fn validate(
    source: &str,
    llvm_ir: &str,
    options: Options,
    native: bool,
    optimized: bool,
) -> Result<ValidationReport> {
    if native && optimized {
        return Err(ValidationError("optimized profile has no native variant"));
    }
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
    let (outcome, ir_definedness) = unsafe {
        let raw = function.as_value_ref();
        if native {
            check_native_module(module.as_mut_ptr(), raw)?;
        } else {
            check_module(
                module.as_mut_ptr(),
                model.bounded_memory,
                false,
                optimized,
                false,
                &BTreeMap::new(),
                false,
            )?;
        }
        compare_function(
            &context,
            raw,
            &model,
            &native_call_renames(native),
            false,
            optimized,
            false,
        )?
    };
    let loops = model.loop_bound_exits.is_some();
    Ok(ValidationReport {
        unroll_bound: loops.then_some(LOOP_BOUND),
        profile: match (loops, model.bounded_memory, native, optimized) {
            (false, false, false, false) => PROFILE,
            (true, false, false, false) => BOUNDED_LOOPS_PROFILE,
            (false, true, false, false) => BOUNDED_MEMORY_PROFILE,
            (true, true, false, false) => BOUNDED_MEMORY_LOOPS_PROFILE,
            (false, false, true, false) => NATIVE_PROFILE,
            (true, false, true, false) => BOUNDED_LOOPS_NATIVE_PROFILE,
            (false, true, true, false) => BOUNDED_MEMORY_NATIVE_PROFILE,
            (true, true, true, false) => BOUNDED_MEMORY_LOOPS_NATIVE_PROFILE,
            (false, false, false, true) => OPT_PROFILE,
            (true, false, false, true) => OPT_BOUNDED_LOOPS_PROFILE,
            (false, true, false, true) => OPT_BOUNDED_MEMORY_PROFILE,
            (true, true, false, true) => OPT_BOUNDED_MEMORY_LOOPS_PROFILE,
            (_, _, true, true) => unreachable!("native+optimized rejected above"),
        },
        source_profile: PURE_I64_WRAPPING_PROFILE,
        target,
        data_layout: DATA_LAYOUT,
        checked_stage: if native {
            "native_user_body_and_truncating_wrapper_snapshot_no_pipeline_attestation"
        } else if optimized {
            "supplied_llvm_ir_snapshot_post_default_O2"
        } else {
            "supplied_llvm_ir_snapshot_no_pipeline_attestation"
        },
        input_domain: loop_domain_label(
            &input_domain_label(native, &model, &ir_definedness),
            loops,
        ),
        source_sha256: model.source_sha256,
        llvm_sha256: mumei_core::native_bundle::sha256(llvm_ir.as_bytes()),
        timeout_ms: options.timeout_ms,
        resource_limit: options.resource_limit,
        outcome,
    })
}

/// Check every source atom against the same-named function in one supplied
/// module. Each atom is an independent obligation — a call is an
/// uninterpreted function site, so the callee's own body is compared as its
/// own entry, never inlined into the caller's. Every source atom must have
/// a same-named member or the whole check refuses (a callee's absence can
/// not be expressed as an outcome: the call-site comparison itself needs it).
/// Extra module members are screened by `check_module` but not compared.
/// The configured solver budget applies to each independent solver query;
/// a function may require several decomposed edge/UB queries.
pub fn validate_pure_i64_module(
    source: &str,
    llvm_ir: &str,
    options: Options,
) -> Result<ModuleValidationReport> {
    validate_module(source, llvm_ir, options, false)
}

/// The module-wide `opt-o2` variant of `validate_pure_i64_module`:
/// same per-member obligations, post-`default<O2>` whitelist.
pub fn validate_pure_i64_module_optimized(
    source: &str,
    llvm_ir: &str,
    options: Options,
) -> Result<ModuleValidationReport> {
    validate_module(source, llvm_ir, options, true)
}

fn validate_module(
    source: &str,
    llvm_ir: &str,
    options: Options,
    optimized: bool,
) -> Result<ModuleValidationReport> {
    bounded(llvm_ir)?;
    if options.timeout_ms == 0 || options.timeout_ms > 30_000 || options.resource_limit == 0 {
        return Err(ValidationError("invalid solver budget"));
    }
    let mut config = Config::new();
    config.set_timeout_msec(options.timeout_ms as u64);
    config.set_param_value("rlimit", &options.resource_limit.to_string());
    let context = Context::new(&config);
    let models = encode_pure_i64_module(source, &context, true, RefinedAdmission::Reject)
        .map_err(|_| ValidationError("unsupported C03 source"))?;
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
    if module.get_triple().as_str().to_bytes() != TARGET.as_bytes()
        || module.get_data_layout().as_str().to_bytes() != DATA_LAYOUT.as_bytes()
    {
        return Err(ValidationError("unsupported target or data layout"));
    }
    // Any atom admitting stack arrays switches the module pre-screen to
    // the bounded-memory whitelist — sibling members are potential call
    // targets and re-gate on their own model at comparison time.
    let any_memory = models.iter().any(|model| model.bounded_memory);
    let array_param_members: BTreeMap<String, Vec<bool>> = models
        .iter()
        .map(|model| {
            (
                model.function_name.clone(),
                model.array_params.iter().map(Option::is_some).collect(),
            )
        })
        .collect();
    let any_array_params = models
        .iter()
        .any(|model| model.array_params.iter().any(Option::is_some));
    unsafe {
        check_module(
            module.as_mut_ptr(),
            any_memory,
            false,
            optimized,
            false,
            &array_param_members,
            any_array_params,
        )?
    };
    let mut functions = Vec::new();
    let mut any_definedness = false;
    let mut any_param_domain = false;
    let mut any_subrange = false;
    let mut any_loops = false;
    let mut outcome = Outcome::EquivalentWithinProfile;
    for model in &models {
        let function = module
            .get_function(&model.function_name)
            .ok_or(ValidationError("missing source function"))?;
        let (member_outcome, ir_definedness) = unsafe {
            compare_function(
                &context,
                function.as_value_ref(),
                model,
                &BTreeMap::new(),
                false,
                optimized,
                false,
            )?
        };
        any_definedness |= !model.definedness.is_empty() || !ir_definedness.is_empty();
        any_param_domain |= !model.param_domain.is_empty();
        any_subrange |= model.subrange_params;
        any_loops |= model.loop_bound_exits.is_some();
        // Different outranks UndefinedBehavior for the aggregate: a wrong
        // produced value is a harder failure than a poisoned one, and both
        // outrank an unknown solver result.
        outcome = match (outcome, member_outcome) {
            (Outcome::Different, _) | (_, Outcome::Different) => Outcome::Different,
            (Outcome::UndefinedBehavior, _) | (_, Outcome::UndefinedBehavior) => {
                Outcome::UndefinedBehavior
            }
            (Outcome::Inconclusive, _) | (_, Outcome::Inconclusive) => Outcome::Inconclusive,
            _ => Outcome::EquivalentWithinProfile,
        };
        functions.push(FunctionOutcome {
            function_name: model.function_name.clone(),
            outcome: member_outcome,
        });
    }
    Ok(ModuleValidationReport {
        profile: match (any_loops, any_memory, optimized) {
            (false, false, false) => PROFILE,
            (true, false, false) => BOUNDED_LOOPS_PROFILE,
            (false, true, false) => BOUNDED_MEMORY_PROFILE,
            (true, true, false) => BOUNDED_MEMORY_LOOPS_PROFILE,
            (false, false, true) => OPT_PROFILE,
            (true, false, true) => OPT_BOUNDED_LOOPS_PROFILE,
            (false, true, true) => OPT_BOUNDED_MEMORY_PROFILE,
            (true, true, true) => OPT_BOUNDED_MEMORY_LOOPS_PROFILE,
        },
        source_profile: PURE_I64_WRAPPING_PROFILE,
        target: TARGET,
        data_layout: DATA_LAYOUT,
        checked_stage: if optimized {
            "supplied_llvm_ir_snapshot_post_default_O2"
        } else {
            "supplied_llvm_ir_snapshot_no_pipeline_attestation"
        },
        input_domain: loop_domain_label(
            if any_definedness {
                if any_subrange {
                    "signed_i64_inputs_with_signature_bounded_param_domains_where_division_and_shift_defined_not_restricted_by_requires"
                } else if any_param_domain {
                    "signed_i64_inputs_with_bool_params_restricted_to_0_or_1_where_division_and_shift_defined_not_restricted_by_requires"
                } else {
                    "signed_i64_inputs_where_division_and_shift_defined_not_restricted_by_requires"
                }
            } else if any_subrange {
                "signed_i64_inputs_with_signature_bounded_param_domains_not_restricted_by_requires"
            } else if any_param_domain {
                "signed_i64_inputs_with_bool_params_restricted_to_0_or_1_not_restricted_by_requires"
            } else {
                "all_signed_i64_inputs_not_restricted_by_requires"
            },
            any_loops,
        ),
        unroll_bound: any_loops.then_some(LOOP_BOUND),
        source_sha256: models
            .first()
            .map(|model| model.source_sha256.clone())
            .unwrap_or_default(),
        llvm_sha256: mumei_core::native_bundle::sha256(llvm_ir.as_bytes()),
        timeout_ms: options.timeout_ms,
        resource_limit: options.resource_limit,
        functions,
        outcome,
    })
}

/// Native multi-atom variant of `validate_pure_i64_module`: every source
/// atom is compared against its emitted member — the `main` atom against
/// `__mumei_user_main` (the emitter's rename of the user `main`), every
/// other atom against its same-named member. The `main` atom must be
/// zero-argument, matching the C wrapper's call shape, and the module
/// must contain exactly one `main` atom. The C `main` wrapper itself is
/// screened by the fixed call/trunc/ret shape (not compared — its i32
/// exit-status truncation is the attested artifact edge), and every
/// member keeps external linkage under the shared function profile.
/// `admission` runs the refined-scalar admission the pinned record
/// names: refined type declarations and signature members typed by
/// them join the encoded domain/postcondition, at exactly the record's
/// admission level. The configured solver budget applies to each
/// independent solver query; a function may require several decomposed
/// edge/UB queries.
pub fn validate_native_module(
    source: &str,
    llvm_ir: &str,
    options: Options,
    admission: RefinedAdmission,
) -> Result<ModuleValidationReport> {
    bounded(llvm_ir)?;
    if options.timeout_ms == 0 || options.timeout_ms > 30_000 || options.resource_limit == 0 {
        return Err(ValidationError("invalid solver budget"));
    }
    let mut config = Config::new();
    config.set_timeout_msec(options.timeout_ms as u64);
    config.set_param_value("rlimit", &options.resource_limit.to_string());
    let context = Context::new(&config);
    let models = encode_pure_i64_module(source, &context, true, admission)
        .map_err(|_| ValidationError("unsupported C03 source"))?;
    let mut mains = 0usize;
    for model in &models {
        // `__mumei_user_main` is the emitter-reserved name for the
        // renamed user `main`; a source atom literally named it would
        // collide with that member.
        if model.function_name == "__mumei_user_main" {
            return Err(ValidationError("source atom uses reserved member name"));
        }
        if model.function_name == "main" {
            mains += 1;
            if !model.param_shapes.is_empty() {
                return Err(ValidationError(
                    "native profile requires zero-argument main",
                ));
            }
        }
    }
    if mains != 1 {
        return Err(ValidationError(
            "native module profile requires one main atom",
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
        bytes if bytes == NATIVE_TARGET.as_bytes() => NATIVE_TARGET,
        _ => return Err(ValidationError("unsupported target or data layout")),
    };
    if module.get_data_layout().as_str().to_bytes() != DATA_LAYOUT.as_bytes() {
        return Err(ValidationError("unsupported target or data layout"));
    }
    let user_main = module
        .get_function("__mumei_user_main")
        .ok_or(ValidationError("missing source function"))?;
    let any_memory = models.iter().any(|model| model.bounded_memory);
    let array_param_members: BTreeMap<String, Vec<bool>> = models
        .iter()
        .map(|model| {
            let name = if model.function_name == "main" {
                "__mumei_user_main"
            } else {
                model.function_name.as_str()
            };
            (
                name.to_owned(),
                model.array_params.iter().map(Option::is_some).collect(),
            )
        })
        .collect();
    // Members touching an admitted `[i64]` result — returning one or
    // calling one — are outside the machine-checked profile: their
    // fat-pointer/malloc lowering never clears the structural whitelist,
    // and the per-atom comparison already fails closed on the same
    // models, so the structural scan skips them instead of erroring.
    let array_result_members: BTreeSet<String> = models
        .iter()
        .filter(|model| {
            model.array_result.is_some()
                || model
                    .call_sites
                    .iter()
                    .any(|site| site.result_shape == CallValueShape::Array)
        })
        .map(|model| {
            if model.function_name == "main" {
                "__mumei_user_main".to_string()
            } else {
                model.function_name.clone()
            }
        })
        .collect();
    unsafe {
        check_native_module_multi(
            module.as_mut_ptr(),
            user_main.as_value_ref(),
            any_memory,
            admission.admits_f64_arithmetic(),
            admission.admits_str_leaves(),
            &NativeModuleMembers {
                array_params: &array_param_members,
                array_params_admitted: admission.admits_array_params(),
                array_results: &array_result_members,
            },
        )?
    };
    let call_renames = native_call_renames(true);
    let mut functions = Vec::new();
    let mut any_definedness = false;
    let mut any_param_domain = false;
    let mut any_subrange = false;
    let mut any_loops = false;
    let mut any_refined = false;
    let mut any_array_params = false;
    let mut outcome = Outcome::EquivalentWithinProfile;
    // The report names the same source profile the record pins: the
    // profile the module gate itself resolves — refined v1 or v2 when
    // the admitted declarations need them — so this follows the
    // declared-members rule, not `any_refined`, which is per-signature.
    let source_profile = module_source_profile(source, admission)
        .map_err(|_| ValidationError("unsupported C03 source"))?;
    for model in &models {
        let member_name = if model.function_name == "main" {
            "__mumei_user_main"
        } else {
            model.function_name.as_str()
        };
        let function = module
            .get_function(member_name)
            .ok_or(ValidationError("missing source function"))?;
        let (member_outcome, ir_definedness) = if array_result_members.contains(member_name) {
            // `[i64]`-returning members and their callers are outside
            // the machine-checked profile — the fat-pointer lowering
            // never encodes — so the member reports inconclusive, the
            // same gate `supports_signature` fails closed on.
            (Outcome::Inconclusive, Vec::new())
        } else {
            unsafe {
                compare_function(
                    &context,
                    function.as_value_ref(),
                    model,
                    &call_renames,
                    admission.admits_f64_arithmetic(),
                    false,
                    admission.admits_array_params(),
                )?
            }
        };
        any_definedness |= !model.definedness.is_empty() || !ir_definedness.is_empty();
        any_param_domain |= !model.param_domain.is_empty();
        any_subrange |= model.subrange_params;
        any_loops |= model.loop_bound_exits.is_some();
        any_refined |= model.refined_signature;
        any_array_params |= model.array_params.iter().any(Option::is_some);
        outcome = match (outcome, member_outcome) {
            (Outcome::Different, _) | (_, Outcome::Different) => Outcome::Different,
            (Outcome::UndefinedBehavior, _) | (_, Outcome::UndefinedBehavior) => {
                Outcome::UndefinedBehavior
            }
            (Outcome::Inconclusive, _) | (_, Outcome::Inconclusive) => Outcome::Inconclusive,
            _ => Outcome::EquivalentWithinProfile,
        };
        functions.push(FunctionOutcome {
            function_name: model.function_name.clone(),
            outcome: member_outcome,
        });
    }
    let base_input_domain = if any_refined {
        "module_members_signed_i64_inputs_with_refined_scalar_signature_predicate_domains_not_process_exit_status"
    } else if any_definedness {
        if any_subrange {
            "module_members_signed_i64_inputs_with_signature_bounded_param_domains_where_division_and_shift_defined_not_process_exit_status"
        } else if any_param_domain {
            "module_members_signed_i64_inputs_with_bool_params_restricted_to_0_or_1_where_division_and_shift_defined_not_process_exit_status"
        } else {
            "module_members_signed_i64_inputs_where_division_and_shift_defined_not_process_exit_status"
        }
    } else if any_subrange {
        "module_members_signed_i64_inputs_with_signature_bounded_param_domains_not_process_exit_status"
    } else if any_param_domain {
        "module_members_signed_i64_inputs_with_bool_params_restricted_to_0_or_1_not_process_exit_status"
    } else {
        "module_members_all_signed_i64_inputs_not_process_exit_status"
    };
    let input_domain = if any_array_params {
        format!("{base_input_domain}_with_read_only_i64_array_params_nonnegative_len")
    } else {
        base_input_domain.to_owned()
    };
    Ok(ModuleValidationReport {
        profile: match (any_loops, any_memory) {
            (false, false) => NATIVE_MODULE_PROFILE,
            (true, false) => BOUNDED_LOOPS_NATIVE_MODULE_PROFILE,
            (false, true) => BOUNDED_MEMORY_NATIVE_MODULE_PROFILE,
            (true, true) => BOUNDED_MEMORY_LOOPS_NATIVE_MODULE_PROFILE,
        },
        source_profile,
        target,
        data_layout: DATA_LAYOUT,
        checked_stage:
            "native_module_user_bodies_and_truncating_wrapper_snapshot_no_pipeline_attestation",
        input_domain: loop_domain_label(&input_domain, any_loops),
        unroll_bound: any_loops.then_some(LOOP_BOUND),
        source_sha256: models
            .first()
            .map(|model| model.source_sha256.clone())
            .unwrap_or_default(),
        llvm_sha256: mumei_core::native_bundle::sha256(llvm_ir.as_bytes()),
        timeout_ms: options.timeout_ms,
        resource_limit: options.resource_limit,
        functions,
        outcome,
    })
}

/// Encode one supplied function and run the three solver queries against
/// its source model. Returns the outcome and the IR definedness entries,
/// which callers need for the report's input-domain label.
/// Callee-name mapping from an emitted member name to the source atom
/// name its call sites compare against: the binary emitter renames the
/// user `main` atom to `__mumei_user_main`, so a source call `main(...)`
/// (`mumei_call_main_0`) must meet `call @__mumei_user_main` under the
/// `main` name — without the mapping, a recursive or sibling call to
/// `main` compares Different on every native snapshot. Pure paths keep
/// emitted names, so their map is empty.
fn native_call_renames(native: bool) -> BTreeMap<String, String> {
    if native {
        [(native_function_name("main").to_owned(), "main".to_owned())]
            .into_iter()
            .collect()
    } else {
        BTreeMap::new()
    }
}

pub fn native_function_name(atom_name: &str) -> &str {
    if atom_name == "main" {
        "__mumei_user_main"
    } else {
        atom_name
    }
}

fn ir_definedness<'ctx>(encoder: &IrEncoder<'ctx, '_>) -> Vec<(Bool<'ctx>, Bool<'ctx>)> {
    encoder
        .definedness
        .iter()
        .map(|(path, defined, ..)| (path.clone(), defined.clone()))
        .collect()
}

/// `(path condition, return value)` edges plus `(path condition,
/// defined)` domain obligations an IR function encodes over shared
/// `model.inputs` for the direct object comparison.
pub type IrFunctionEncoding<'ctx> = (Vec<(Bool<'ctx>, BV<'ctx>)>, Vec<(Bool<'ctx>, Bool<'ctx>)>);

/// An LLVM IR module parsed once so every recorded function can be
/// encoded for the direct IR ↔ object comparison without reparsing.
/// Holds the inkwell context alive behind the raw module reference.
pub struct ParsedIrModule {
    _context: LlvmContext,
    raw: LLVMModuleRef,
}

impl ParsedIrModule {
    /// Look up a function by symbol name.
    ///
    /// # Safety
    /// The returned value is valid while this `ParsedIrModule` lives.
    pub unsafe fn function(&self, function_name: &str) -> Result<LLVMValueRef> {
        let name = std::ffi::CString::new(function_name)
            .map_err(|_| ValidationError("LLVM IR function name"))?;
        let raw = LLVMGetNamedFunction(self.raw, name.as_ptr());
        if raw.is_null() {
            return Err(ValidationError("function missing in LLVM IR"));
        }
        Ok(raw)
    }
}

/// Parse LLVM IR bytes once for repeated per-function encodes.
///
/// # Safety
/// Calls llvm-sys C APIs on the module parsed from `llvm_ir`; the
/// caller must pass well-formed LLVM IR bytes.
pub unsafe fn parse_ir_module_for_comparison(llvm_ir: &[u8]) -> Result<ParsedIrModule> {
    let context = LlvmContext::create();
    let module = context
        .create_module_from_ir(MemoryBuffer::create_from_memory_range_copy(
            llvm_ir,
            "ir-object-compare.ll",
        ))
        .map_err(|_| ValidationError("LLVM IR parse failed"))?;
    let raw = module.as_mut_ptr();
    // Skip the Rust wrapper's destructor: the C module intentionally
    // stays allocated for the struct's lifetime, and the held
    // `_context` keeps every lookup into it valid.
    std::mem::forget(module);
    Ok(ParsedIrModule {
        _context: context,
        raw,
    })
}

/// Encode one already-parsed IR function's return edges and
/// definedness obligations over shared `model.inputs`; `Err` outside
/// the encoder's IR subset — the caller records the function as not
/// liftable.
///
/// # Safety
/// `raw` must be a function inside a `ParsedIrModule` that still
/// lives, and `model.inputs` must be bound to `context`.
pub unsafe fn encode_parsed_function_for_comparison<'ctx>(
    raw: LLVMValueRef,
    model: &PureI64Body<'ctx>,
    optimized: bool,
    context: &'ctx Context,
) -> Result<IrFunctionEncoding<'ctx>> {
    // Reuse the validation gate so an arity or signature mismatch on
    // the IR side fails closed as a encode error instead of silently
    // comparing a mis-bound function (design memo §7 Step 3a). The
    // comparison subset admits scalar signatures and selected aggregate
    // results, with scalar-i64 calls and bounded memory admitted by
    // the same gates; the model-side pre-gate keeps wider models out,
    // and the same restriction is applied to the IR surface here.
    let array_param_mask: Vec<bool> = model.array_params.iter().map(Option::is_some).collect();
    check_function(
        raw,
        model.param_shapes.len(),
        model.result_shape,
        Some(&model.result_leaf_doubles),
        CheckGates {
            allow_cycles: model.loop_bound_exits.is_some(),
            bounded_memory: model.bounded_memory,
            f64_arithmetic: false,
            optimized,
            array_param_mask: Some(&array_param_mask),
            array_params_admitted: false,
        },
    )?;
    // The shared gate admits a 128-bit scalar return and a checked
    // direct call; a scalar `i128` return compares as its full 128-bit
    // edge — the machine side returns `rdx:rax` (`hi:lo`) and the IR
    // side encodes the `ret` operand at its native width, so both are
    // plain 128-bit bit-vectors. Register-resident and `sret` integer
    // aggregates and flat mixed register pairs are compared: the shared
    // gate already matched the aggregate's field count and leaf kinds against
    // `model.result_shape`/`result_leaf_doubles`, so an aggregate `ret`
    // encodes as its `aggregate_value` slots and compares leaf by leaf.
    //
    // A scalar return must match the model's own width exactly: the
    // shared gate admits 64|128 for either model, so a foreign IR
    // returning `i128` against an `i64` source (or `i64` against an
    // `i128` source) would otherwise encode and compare mismatched
    // widths — fail it closed here instead.
    //
    // Check the exact field types: a leaf count and the
    // double/integer kinds alone cannot pin the ABI — `{i128}` and
    // `{i64, i64}` both flatten to two non-`double` 64-bit slots, so an
    // `i128`/`u128` field would slip through a count+kinds check and a
    // machine leaf that is only half of the source's 128-bit value
    // would compare against the wrong field. Only flat, mixed
    // `i64`/`double` register pairs may have a non-`i64` leaf.
    // Aggregate parameters get the same all-`i64`-leaf gate the
    // result has: `param_shapes`/`param_leaf_doubles` pin the leaf
    // count and the double/integer kinds only, so a `{i128, i64}`
    // formal would slip a wider field into a slot the model binds at
    // 64 bits — count-and-kinds cannot pin widths, so a non-i64 leaf
    // fails closed instead of reaching the compare as a sort error.
    for index in 0..LLVMCountParams(raw) {
        let param_type = LLVMTypeOf(LLVMGetParam(raw, index));
        if aggregate_fields(param_type).is_some() && !aggregate_all_i64_leaves(param_type) {
            return Err(ValidationError("LLVM param outside comparison subset"));
        }
    }
    let ret = LLVMGetReturnType(LLVMGlobalGetValueType(raw));
    if aggregate_fields(ret).is_some() {
        if !aggregate_all_i64_leaves(ret) && !is_flat_mixed_register_pair(ret) {
            return Err(ValidationError("LLVM return outside comparison subset"));
        }
    } else if width(ret)? != model.value.get_size() {
        return Err(ValidationError("LLVM return outside comparison subset"));
    }
    // Scalar-i64 calls are compared: `check_call` already restricts
    // them to direct calls of the callee's declared type with no
    // operand bundles, tail markers, or call-site attributes, and the
    // encoder models them as `call_decl` uninterpreted applications.
    // A call returning or taking anything but scalar 64-bit leaves is
    // outside the subset.
    let mut block = LLVMGetFirstBasicBlock(raw);
    while !block.is_null() {
        let mut inst = LLVMGetFirstInstruction(block);
        while !inst.is_null() {
            if LLVMGetInstructionOpcode(inst) == LLVMOpcode::LLVMCall {
                let call_type = LLVMTypeOf(inst);
                let scalar_result = LLVMGetTypeKind(call_type) == LLVMTypeKind::LLVMIntegerTypeKind
                    && LLVMGetIntTypeWidth(call_type) == 64;
                let scalar_args = (0..LLVMGetNumArgOperands(inst)).all(|index| {
                    let arg_type = LLVMTypeOf(LLVMGetOperand(inst, index));
                    LLVMGetTypeKind(arg_type) == LLVMTypeKind::LLVMIntegerTypeKind
                        && LLVMGetIntTypeWidth(arg_type) == 64
                });
                // An unused call result carries its actual arguments
                // nowhere in the encoded return term, so the IR and
                // machine sides could pass different actuals and still
                // compare `Equivalent`. Reject it instead of asserting
                // arg obligations the subset cannot map to call sites.
                if !scalar_result || !scalar_args || LLVMGetFirstUse(inst).is_null() {
                    return Err(ValidationError("LLVM call outside comparison subset"));
                }
            }
            inst = LLVMGetNextInstruction(inst);
        }
        block = LLVMGetNextBasicBlock(block);
    }
    // The native emitter renames the user `main` atom to
    // `__mumei_user_main`; the source model's call sites name their
    // callee `main`, so the encoder needs the same rename map the
    // shared validation path uses or every call to `main` compares
    // `Different`.
    let call_renames = native_call_renames(true);
    let mut encoder = IrEncoder::new(context, &call_renames, false, optimized);
    let returns = encoder.encode(
        raw,
        &model.inputs,
        &model.param_shapes,
        &model.param_leaf_doubles,
        &model.array_params,
    )?;
    Ok((returns, ir_definedness(&encoder)))
}

/// Encode one LLVM IR function's return edges and definedness
/// obligations for the direct IR ↔ object comparison (design memo §7
/// Step 3 slice). Parses `llvm_ir` each call; callers comparing
/// several functions of one module should share a `ParsedIrModule`.
///
/// # Safety
/// Calls llvm-sys C APIs on a module it parses from `llvm_ir`; the
/// caller must pass well-formed LLVM IR bytes and a model whose
/// `inputs` are bound to `context`.
pub unsafe fn encode_ir_function_for_comparison<'ctx>(
    llvm_ir: &[u8],
    function_name: &str,
    model: &PureI64Body<'ctx>,
    optimized: bool,
    context: &'ctx Context,
) -> Result<IrFunctionEncoding<'ctx>> {
    let module = parse_ir_module_for_comparison(llvm_ir)?;
    let raw = module.function(function_name)?;
    encode_parsed_function_for_comparison(raw, model, optimized, context)
}

fn check_independent<'ctx>(
    terms: &[Bool<'ctx>],
    mut check: impl FnMut(&Bool<'ctx>) -> SatResult,
) -> SatResult {
    let mut unknown = false;
    for term in terms {
        match check(term) {
            SatResult::Sat => return SatResult::Sat,
            SatResult::Unknown => unknown = true,
            SatResult::Unsat => {}
        }
    }
    if unknown {
        SatResult::Unknown
    } else {
        SatResult::Unsat
    }
}

unsafe fn compare_function<'ctx>(
    context: &'ctx Context,
    raw: LLVMValueRef,
    model: &PureI64Body<'ctx>,
    call_renames: &BTreeMap<String, String>,
    f64_arithmetic: bool,
    optimized: bool,
    array_params_admitted: bool,
) -> Result<(Outcome, Vec<(Bool<'ctx>, Bool<'ctx>)>)> {
    // A source `while` relaxes the CFG gate only for that atom's member:
    // cycles elsewhere still reject as unsupported.
    let loops = model.loop_bound_exits.is_some();
    // A struct parameter is one aggregate formal on the IR side and its
    // flattened field slots on the source side, so the IR arity is the
    // declared parameter count, not the slot count.
    let array_param_mask: Vec<bool> = model.array_params.iter().map(Option::is_some).collect();
    check_function(
        raw,
        model.param_shapes.len(),
        model.result_shape,
        Some(&model.result_leaf_doubles),
        CheckGates {
            allow_cycles: loops,
            bounded_memory: model.bounded_memory,
            f64_arithmetic,
            optimized,
            array_param_mask: Some(&array_param_mask),
            array_params_admitted,
        },
    )?;
    let mut encoder = IrEncoder::new(context, call_renames, loops, optimized);
    let returns = encoder.encode(
        raw,
        &model.inputs,
        &model.param_shapes,
        &model.param_leaf_doubles,
        &model.array_params,
    )?;
    let ir_definedness = ir_definedness(&encoder);
    // Division, remainder and shifts bind both sides only where the operands
    // are defined — the same safety domain `--bitvec-i64` verification
    // requires. Inputs outside the source domain are don't-care; inputs the
    // source binds but the IR poisons on are a distinct failure.
    // A declared `bool` or subrange integer parameter lowers to i64, so
    // the signature itself bounds its input beside the operation
    // obligations.
    // The bounded-exit condition joins the source domain: equivalence is
    // only claimed on inputs where every `while` exits within the unroll
    // bound — paths the LLVM encoder dropped for exceeding the same
    // bound are covered by it on the source side.
    let exits = model
        .loop_bound_exits
        .clone()
        .unwrap_or_else(|| Bool::from_bool(context, true));
    let src_terms: Vec<Bool> = model
        .param_domain
        .iter()
        .chain(model.array_len_domain.iter())
        .chain(model.definedness.iter())
        .chain(model.call_result_bounds.iter())
        .cloned()
        .collect();
    let src_dom = Bool::and(context, &src_terms.iter().collect::<Vec<_>>());
    let ir_terms: Vec<Bool> = ir_definedness
        .iter()
        .map(|(path, defined)| path.implies(defined))
        .collect();
    let ir_dom = Bool::and(context, &ir_terms.iter().collect::<Vec<_>>());
    // A hostile module could return a width the source signature does
    // not declare — compare same-width values only, never let z3 panic
    // on a size mismatch.
    if returns
        .iter()
        .any(|(_, value)| value.get_size() != model.value.get_size())
    {
        return Err(ValidationError("LLVM return width mismatch"));
    }
    let mismatches: Vec<_> = returns
        .iter()
        .map(|(guard, value)| {
            Bool::and(
                context,
                &[
                    guard,
                    &src_dom,
                    &ir_dom,
                    &exits,
                    &value._eq(&model.value).not(),
                ],
            )
        })
        .collect();
    // A covered input every IR path out of was dropped past the bound
    // is neither proven divergent — truncation is not divergence — nor
    // safe to exclude silently: report it as inconclusive below rather
    // than as a difference or a vacuous equivalence.
    let covered_no_return = if loops {
        let covered = Bool::and(context, &[&src_dom, &ir_dom, &exits]);
        let guards: Vec<_> = returns.iter().map(|(guard, _)| guard).collect();
        let any_return = Bool::or(context, &guards);
        Some(Bool::and(context, &[&covered, &any_return.not()]))
    } else {
        None
    };
    let solver = Solver::new(context);
    let outcome = 'check: {
        // A source whose guarded operators have no defined input at all is a
        // spec `--bitvec-i64` verification would already reject; equivalence
        // over an empty domain would be vacuous. With bounded loops the
        // same check extends to the exit domain: a loop that cannot exit
        // within the bound on any defined input is unsupported, not
        // vacuously equivalent.
        if !model.definedness.is_empty() || model.loop_bound_exits.is_some() {
            solver.assert(&Bool::and(context, &[&src_dom, &exits]));
            match solver.check() {
                SatResult::Sat => {}
                SatResult::Unsat => {
                    return Err(ValidationError(
                        "source domain and loop exits empty on all inputs",
                    ))
                }
                SatResult::Unknown => break 'check Outcome::Inconclusive,
            }
            solver.reset();
        }
        // `or` over every edge makes one large bit-vector query. Checking
        // each edge violation separately is logically equivalent — one
        // `sat` witness still means `Different`, and every `unsat` still
        // means no mismatch — while keeping branch-local arithmetic small
        // enough for the solver. Any residual `unknown` stays inconclusive.
        match check_independent(&mismatches, |mismatch| {
            solver.reset();
            solver.assert(mismatch);
            solver.check()
        }) {
            SatResult::Sat => break 'check Outcome::Different,
            SatResult::Unknown => break 'check Outcome::Inconclusive,
            SatResult::Unsat => {}
        }
        // Even with matching values, an instruction may still produce poison
        // on an input inside the source's defined domain.
        let ub: Vec<Bool> = ir_definedness
            .iter()
            .map(|(path, defined)| Bool::and(context, &[path, &defined.not(), &src_dom, &exits]))
            .collect();
        // Same disjunction decomposition for independent UB obligations.
        match check_independent(&ub, |term| {
            solver.reset();
            solver.assert(term);
            solver.check()
        }) {
            SatResult::Sat => Outcome::UndefinedBehavior,
            SatResult::Unknown => Outcome::Inconclusive,
            SatResult::Unsat => match covered_no_return {
                Some(term) => {
                    solver.reset();
                    solver.assert(&term);
                    match solver.check() {
                        SatResult::Unsat => Outcome::EquivalentWithinProfile,
                        _ => Outcome::Inconclusive,
                    }
                }
                None => Outcome::EquivalentWithinProfile,
            },
        }
    };
    Ok((outcome, ir_definedness))
}

/// Append the bounded-loop domain restriction to a base input-domain
/// label so the report names the actual claim boundary.
fn loop_domain_label(domain: &str, loops: bool) -> String {
    if loops {
        format!("{domain}_and_loops_exiting_within_bound_{LOOP_BOUND}")
    } else {
        domain.to_owned()
    }
}

fn input_domain_label<'ctx>(
    native: bool,
    model: &PureI64Body<'ctx>,
    ir_definedness: &[(Bool<'ctx>, Bool<'ctx>)],
) -> String {
    // Bounded-memory models put each in-bounds obligation on the same
    // definedness footing as division and shift — name it so the
    // domain description does not overstate the claim.
    let defined_what = if model.bounded_memory {
        "division_and_shift_and_in_bounds_memory_accesses"
    } else {
        "division_and_shift"
    };
    if !model.definedness.is_empty() || !ir_definedness.is_empty() {
        if native {
            format!(
                "zero_argument_main_i64_body_where_{defined_what}_defined_not_process_exit_status"
            )
        } else if model.param_domain.is_empty() {
            format!("signed_i64_inputs_where_{defined_what}_defined_not_restricted_by_requires")
        } else if model.subrange_params {
            format!("signed_i64_inputs_with_signature_bounded_param_domains_where_{defined_what}_defined_not_restricted_by_requires")
        } else {
            format!("signed_i64_inputs_with_bool_params_restricted_to_0_or_1_where_{defined_what}_defined_not_restricted_by_requires")
        }
    } else if native {
        "zero_argument_main_i64_body_not_process_exit_status".to_owned()
    } else if model.param_domain.is_empty() {
        "all_signed_i64_inputs_not_restricted_by_requires".to_owned()
    } else if model.subrange_params {
        "signed_i64_inputs_with_signature_bounded_param_domains_not_restricted_by_requires"
            .to_owned()
    } else {
        "signed_i64_inputs_with_bool_params_restricted_to_0_or_1_not_restricted_by_requires"
            .to_owned()
    }
}

unsafe fn width(ty: LLVMTypeRef) -> Result<u32> {
    // A `double` is the emitter's lowering of a pass-through `f64`
    // leaf (`pure-i64-composite/v11`): one 64-bit slot carrying the
    // IEEE-754 bit pattern, compared bit-for-bit — the profile admits
    // no float instructions, so the leaf only ever moves.
    if LLVMGetTypeKind(ty) == LLVMTypeKind::LLVMDoubleTypeKind {
        return Ok(64);
    }
    if LLVMGetTypeKind(ty) != LLVMTypeKind::LLVMIntegerTypeKind {
        return Err(ValidationError("non-integer LLVM value"));
    }
    match LLVMGetIntTypeWidth(ty) {
        n @ (1 | 64 | 128) => Ok(n),
        _ => Err(ValidationError("unsupported LLVM width")),
    }
}

/// The leaf count of an admitted aggregate formal type: a literal,
/// non-packed struct whose elements are `i64` or admitted aggregates —
/// the emitter's lowering of a struct parameter whose fields are
/// `i64`/`bool` (a `bool` field is an i64 carrying 0/1) or another
/// admitted struct. Nested struct elements flatten leaf-first; the
/// total stays within the 16 leaf slots a flattened signature allows.
/// Packed, named, mixed-width and empty aggregates are `None`.
unsafe fn aggregate_fields(ty: LLVMTypeRef) -> Option<u32> {
    if LLVMGetTypeKind(ty) != LLVMTypeKind::LLVMStructTypeKind
        || LLVMIsPackedStruct(ty) != 0
        || LLVMIsLiteralStruct(ty) == 0
    {
        return None;
    }
    let count = LLVMCountStructElementTypes(ty);
    if count == 0 {
        return None;
    }
    let mut leaves = 0u32;
    for i in 0..count {
        let element = LLVMStructGetTypeAtIndex(ty, i);
        leaves = leaves.checked_add(element_leaf_span(element)?)?;
        // Leaf-first flattening bounds the shape the same way the
        // fragment's flattened signatures bound a composite param: at
        // most 16 leaf slots.
        if leaves > 16 {
            return None;
        }
    }
    Some(leaves)
}

/// The leaf span one struct element contributes — the same per-element
/// count `aggregate_fields` sums: a nested aggregate flattens to its
/// own leaf count, an `i128` element is one 128-bit leaf (two
/// consecutive 64-bit ABI slots, `pure-i64-composite/v21`), a
/// `double` or `i64` element one slot. `aggregate_leaf_range` walks
/// these spans so an `insertvalue` targets the same leaves the
/// signature flattens to.
unsafe fn element_leaf_span(element: LLVMTypeRef) -> Option<u32> {
    if let Some(inner) = aggregate_fields(element) {
        return Some(inner);
    }
    match LLVMGetTypeKind(element) {
        LLVMTypeKind::LLVMIntegerTypeKind => match LLVMGetIntTypeWidth(element) {
            64 => Some(1),
            128 => Some(2),
            _ => None,
        },
        LLVMTypeKind::LLVMDoubleTypeKind => Some(1),
        _ => None,
    }
}

/// The leaf range struct element `index` of an admitted aggregate
/// spans: `aggregate_fields` flattens `{inner: {a, b}, c}` to
/// `[a, b, c]`, so element 0 covers leaves 0..2, and `{a: i64, b: i128}`
/// puts element 1 at leaves 1..3. A non-admitted type or an
/// out-of-range index fails closed.
unsafe fn aggregate_leaf_range(ty: LLVMTypeRef, index: u32) -> Option<(usize, usize)> {
    // A `{i64, i1}` `with.overflow` result: leaf `index` spans itself.
    if overflow_pair(ty) {
        return (index <= 1).then_some((index as usize, 1));
    }
    if aggregate_fields(ty).is_none() || index >= LLVMCountStructElementTypes(ty) {
        return None;
    }
    let mut offset = 0usize;
    for i in 0..=index {
        let span = element_leaf_span(LLVMStructGetTypeAtIndex(ty, i))? as usize;
        if i == index {
            return Some((offset, span));
        }
        offset += span;
    }
    None
}

/// Whether every flattened leaf of an admitted aggregate is an `i64`.
/// `aggregate_fields`/`aggregate_leaf_doubles` cannot make this check:
/// an `i128` element flattens to two non-`double` slots exactly like
/// `{i64, i64}` (`pure-i64-composite/v21`), and a `double` leaf is one
/// slot like an `i64`.
unsafe fn aggregate_all_i64_leaves(ty: LLVMTypeRef) -> bool {
    if aggregate_fields(ty).is_none() {
        return false;
    }
    (0..LLVMCountStructElementTypes(ty)).all(|i| {
        let element = LLVMStructGetTypeAtIndex(ty, i);
        if aggregate_fields(element).is_some() {
            aggregate_all_i64_leaves(element)
        } else {
            LLVMGetTypeKind(element) == LLVMTypeKind::LLVMIntegerTypeKind
                && LLVMGetIntTypeWidth(element) == 64
        }
    })
}

unsafe fn is_flat_mixed_register_pair(ty: LLVMTypeRef) -> bool {
    if LLVMCountStructElementTypes(ty) != 2 {
        return false;
    }
    let field_kind = |index| {
        let element = LLVMStructGetTypeAtIndex(ty, index);
        match LLVMGetTypeKind(element) {
            LLVMTypeKind::LLVMDoubleTypeKind => Some(true),
            LLVMTypeKind::LLVMIntegerTypeKind if LLVMGetIntTypeWidth(element) == 64 => Some(false),
            _ => None,
        }
    };
    matches!(
        (field_kind(0), field_kind(1)),
        (Some(false), Some(true)) | (Some(true), Some(false))
    )
}

/// A constant `i128` as a `BV` — the C API's `LLVMConstIntGet*Value`
/// accessors truncate at 64 bits, so parse the printed `i128 N` token
/// (`pure-i64-composite/v21`).
unsafe fn const_i128<'ctx>(ctx: &'ctx Context, value: LLVMValueRef) -> Result<BV<'ctx>> {
    let printed = LLVMPrintValueToString(value);
    let text = std::ffi::CStr::from_ptr(printed)
        .to_string_lossy()
        .into_owned();
    LLVMDisposeMessage(printed);
    let parsed = text
        .split_whitespace()
        .next_back()
        .and_then(|token| token.parse::<i128>().ok())
        .ok_or(ValidationError("unsupported LLVM constant"))?;
    let lo = BV::from_u64(ctx, parsed as u64, 64);
    let hi = BV::from_u64(ctx, (parsed >> 64) as u64, 64);
    Ok(hi.concat(&lo))
}

/// `true` for a `double` leaf (the pass-through `f64` lowering),
/// `false` for an `i64` leaf, `None` for anything else — the type
/// check `width` alone cannot make, which an i64/f64 ABI confusion
/// needs (`pure-i64-composite/v11`).
unsafe fn leaf_is_double(ty: LLVMTypeRef) -> Option<bool> {
    if LLVMGetTypeKind(ty) == LLVMTypeKind::LLVMIntegerTypeKind && LLVMGetIntTypeWidth(ty) == 128 {
        return Some(false);
    }
    match LLVMGetTypeKind(ty) {
        LLVMTypeKind::LLVMDoubleTypeKind => Some(true),
        LLVMTypeKind::LLVMIntegerTypeKind if LLVMGetIntTypeWidth(ty) == 64 => Some(false),
        _ => None,
    }
}

/// The per-leaf element kinds of an admitted aggregate, flattened
/// leaf-first exactly like `aggregate_fields`: `true` for a `double`
/// leaf, `false` for an `i64` leaf. `None` unless `ty` is an admitted
/// aggregate. A leaf count alone cannot pin the ABI — `{i64, double}`
/// and `{double, i64}` both count two leaves — so the source model
/// pairs each leaf with its declared kind.
unsafe fn aggregate_leaf_doubles(ty: LLVMTypeRef) -> Option<Vec<bool>> {
    aggregate_fields(ty)?;
    let mut kinds = Vec::new();
    for i in 0..LLVMCountStructElementTypes(ty) {
        let element = LLVMStructGetTypeAtIndex(ty, i);
        if let Some(mut inner) = aggregate_leaf_doubles(element) {
            kinds.append(&mut inner);
        } else if LLVMGetTypeKind(element) == LLVMTypeKind::LLVMIntegerTypeKind
            && LLVMGetIntTypeWidth(element) == 128
        {
            // Two 64-bit ABI slots per 128-bit leaf — neither is a
            // `double`.
            kinds.push(false);
            kinds.push(false);
        } else {
            kinds.push(leaf_is_double(element)?);
        }
    }
    Some(kinds)
}

/// `true` when `inst` carries fast-math flags. LLVM 17's C API has no
/// FMF getter, but `LLVMPrintValueToString` renders them as tokens
/// right after the opcode (`select nnan i1 %c, double %a, double %b`,
/// `call fast double @f(...)`). The flag set is closed, and a flag can
/// never appear as a bare operand token (values print with a `%`, `@`
/// or literal prefix), so a known flag token in the instruction's
/// leading tokens means flags are set — the source model treats an
/// `f64` leaf as an opaque bit pattern, which any flag could
/// reinterpret.
unsafe fn has_fast_math(inst: LLVMValueRef) -> bool {
    let text = LLVMPrintValueToString(inst);
    if text.is_null() {
        // An unprintable instruction is not provably flag-free.
        return true;
    }
    let body = std::ffi::CStr::from_ptr(text).to_str().unwrap_or("");
    let body = body.split_once(" = ").map(|(_, rest)| rest).unwrap_or(body);
    let flagged = body.split_ascii_whitespace().take(8).any(|token| {
        matches!(
            token,
            "fast" | "nnan" | "ninf" | "nsz" | "arcp" | "contract" | "afn" | "reassoc"
        )
    });
    LLVMDisposeMessage(text);
    flagged
}

/// A constant of an admitted aggregate type whose slots the encoder can
/// read: whole `undef`/`poison` (every slot undefined),
/// `zeroinitializer`, or a literal struct of integer, `double` and
/// `undef` elements.
unsafe fn constant_aggregate(value: LLVMValueRef, optimized: bool) -> bool {
    let ty = LLVMTypeOf(value);
    if LLVMIsConstant(value) == 0
        || (aggregate_fields(ty).is_none() && !(optimized && overflow_pair(ty)))
    {
        return false;
    }
    if LLVMIsUndef(value) != 0 || !LLVMIsAConstantAggregateZero(value).is_null() {
        return true;
    }
    !LLVMIsAConstantStruct(value).is_null()
        && (0..LLVMGetNumOperands(value) as u32).all(|i| {
            let element = LLVMGetOperand(value, i);
            LLVMIsUndef(element) != 0
                || !LLVMIsAConstantInt(element).is_null()
                || !LLVMIsAConstantFP(element).is_null()
                || constant_aggregate(element, optimized)
        })
}

unsafe fn check_module_environment(module: LLVMModuleRef, str_leaves: bool) -> Result<()> {
    let mut asm_size = 0;
    LLVMGetModuleInlineAsm(module, &mut asm_size);
    if asm_size != 0
        || !LLVMGetFirstNamedMetadata(module).is_null()
        || !LLVMGetFirstGlobalAlias(module).is_null()
        || !LLVMGetFirstGlobalIFunc(module).is_null()
    {
        return Err(ValidationError("module outside bounded snapshot profile"));
    }
    // A `let s = "lit"` binding lowers to a private constant
    // byte-string global (`str_lit`) — the only global the bounded
    // shape may carry, and only when the admission covers `Str`
    // leaves. Anything else stays refused.
    let mut global = LLVMGetFirstGlobal(module);
    while !global.is_null() {
        if !(str_leaves && is_str_lit_global(global)) {
            return Err(ValidationError("module outside bounded snapshot profile"));
        }
        global = LLVMGetNextGlobal(global);
    }
    Ok(())
}

/// Whether `global` is exactly the emitter's string-literal shape: a
/// private/internal constant with a constant byte-array initializer
/// named `str_lit` (LLVM uniquifies duplicates as `str_lit.N`).
unsafe fn is_str_lit_global(global: LLVMValueRef) -> bool {
    let linkage = LLVMGetLinkage(global);
    if !matches!(
        linkage,
        llvm_sys::LLVMLinkage::LLVMPrivateLinkage | llvm_sys::LLVMLinkage::LLVMInternalLinkage
    ) || LLVMIsGlobalConstant(global) == 0
    {
        return false;
    }
    let init = LLVMGetInitializer(global);
    if init.is_null()
        || (LLVMIsAConstantDataArray(init).is_null() && LLVMIsAConstantArray(init).is_null())
    {
        return false;
    }
    let mut len = 0usize;
    let name = LLVMGetValueName2(global, &mut len);
    if name.is_null() {
        return false;
    }
    std::ffi::CStr::from_ptr(name)
        .to_bytes()
        .starts_with(b"str_lit")
}

unsafe fn check_module(
    module: LLVMModuleRef,
    bounded_memory: bool,
    f64_arithmetic: bool,
    optimized: bool,
    str_leaves: bool,
    array_param_members: &BTreeMap<String, Vec<bool>>,
    array_params_admitted: bool,
) -> Result<()> {
    check_module_environment(module, str_leaves)?;
    let mut member = LLVMGetFirstFunction(module);
    if member.is_null() {
        return Err(ValidationError("module outside bounded function profile"));
    }
    while !member.is_null() {
        // Every member is a potential call target, so each must satisfy
        // the same function profile against its own arity. The checked
        // function's arity against the source signature is asserted
        // separately at the call site. Cycles pass the pre-screen — the
        // per-atom comparison re-gates them against each model's loop
        // admission.
        if LLVMGetLinkage(member) != llvm_sys::LLVMLinkage::LLVMExternalLinkage {
            return Err(ValidationError("LLVM function linkage unsupported"));
        }
        // Declarations are call targets, not obligations: every use
        // is constrained by `check_call` — the cancellation poll plus,
        // under the opt-o2 profiles, the canonical optimizer intrinsics.
        if LLVMIsDeclaration(member) != 0
            && matches!(callee_name(member).as_deref(), Some(name)
                if name == CANCEL_POLL_INTRINSIC
                    || (optimized && is_opt_intrinsic(name)))
        {
            member = LLVMGetNextFunction(member);
            continue;
        }
        let member_name = callee_name(member);
        let array_param_mask = member_name
            .as_ref()
            .and_then(|name| array_param_members.get(name))
            .map(Vec::as_slice);
        check_function(
            member,
            LLVMCountParams(member) as usize,
            member_result_shape(member),
            None,
            CheckGates {
                allow_cycles: true,
                bounded_memory,
                f64_arithmetic,
                optimized,
                array_param_mask,
                array_params_admitted,
            },
        )?;
        member = LLVMGetNextFunction(member);
    }
    Ok(())
}

/// The emitter plants this poll in every loop header for cooperative
/// task-group cancellation. Cancellation is an effect the pure profile
/// does not model: the poll is admitted as a declaration-only runtime
/// intrinsic whose result binds to 0 — the claim covers executions
/// where no cancellation is requested.
pub(crate) const CANCEL_POLL_INTRINSIC: &str = "__mumei_task_group_should_cancel_current";

unsafe fn callee_name(value: LLVMValueRef) -> Option<String> {
    let mut len = 0;
    let raw = LLVMGetValueName2(value, &mut len);
    if raw.is_null() {
        return None;
    }
    Some(String::from_utf8_lossy(std::slice::from_raw_parts(raw as *const u8, len)).into_owned())
}

/// A `call` is admitted only as a direct, plain call to a defined
/// in-module function with an all-i64 signature — the encoder then
/// models it as an uninterpreted function of its arguments, and
/// equivalence holds when both sides call the same callee on equal
/// arguments. Whether the callee's own body matches its source is a
/// separate per-function validation obligation. The one declaration
/// admitted is `CANCEL_POLL_INTRINSIC`: its result binds to 0 under
/// the no-cancellation-requested assumption.
unsafe fn check_call(call: LLVMValueRef, optimized: bool) -> Result<()> {
    let callee = LLVMGetCalledValue(call);
    let decl_name = callee_name(callee);
    // `num_operands == args + 1` excludes operand bundles, which the
    // LLVM 17 C API cannot enumerate (the native wrapper prints instead
    // because its callee has no arguments at all).
    if callee.is_null()
        || LLVMGetValueKind(callee) != llvm_sys::LLVMValueKind::LLVMFunctionValueKind
        || LLVMGetCalledFunctionType(call) != LLVMGlobalGetValueType(callee)
        || (LLVMIsDeclaration(callee) != 0
            && decl_name.as_deref() != Some(CANCEL_POLL_INTRINSIC)
            && !(optimized && decl_name.as_deref().is_some_and(is_opt_intrinsic)))
        || LLVMGetNumOperands(call) != LLVMGetNumArgOperands(call) as i32 + 1
        || LLVMGetInstructionCallConv(call) != 0
        // `tail`/`musttail` are `default<O2>` canonical call markers —
        // they change codegen legality, not the call's value.
        || (!optimized && LLVMIsTailCall(call) != 0)
        // Function-scope call-site attributes (`nocallback`,
        // `memory(none)`, ...) are descriptive of the same pipeline's
        // proofs; per-argument attributes stay gated below.
        || (!optimized && LLVMGetCallSiteAttributeCount(call, u32::MAX) != 0)
    {
        return Err(ValidationError("unsupported LLVM call"));
    }
    // The operand count excludes empty operand bundles
    // (`call i64 @g(i64 %x) [ "deopt"() ]`), which the LLVM 17 C API
    // cannot enumerate at all. A bundle always prints as `[` directly
    // followed by a quoted tag, so the check keeps quoted SSA names
    // like `%"value [0]"` working while every bundle shape rejects.
    let printed = LLVMPrintValueToString(call);
    let text = std::ffi::CStr::from_ptr(printed)
        .to_string_lossy()
        .into_owned();
    LLVMDisposeMessage(printed);
    if text
        .split('[')
        .skip(1)
        .any(|rest| rest.trim_start().starts_with('"'))
    {
        return Err(ValidationError("unsupported LLVM call"));
    }
    // Call-site attribute index 0 is the return value, 1..=num_args are
    // the parameters: every index must carry no attribute, so `noundef`,
    // `signext`/`zeroext` and friends never slip in.
    let arg_count = LLVMGetNumArgOperands(call);
    for index in 0..=arg_count {
        if LLVMGetCallSiteAttributeCount(call, index) != 0 {
            return Err(ValidationError("unsupported LLVM call"));
        }
    }
    let signature = LLVMGetCalledFunctionType(call);
    let ret = LLVMGetReturnType(signature);
    if optimized
        && LLVMIsDeclaration(callee) != 0
        && decl_name.as_deref() != Some(CANCEL_POLL_INTRINSIC)
    {
        return check_intrinsic_signature(signature, decl_name.as_deref());
    }
    if (aggregate_fields(ret).is_none() && !matches!(width(ret)?, 64 | 128))
        || LLVMCountParamTypes(signature) != LLVMGetNumArgOperands(call)
        || LLVMIsFunctionVarArg(signature) != 0
    {
        return Err(ValidationError("unsupported LLVM call signature"));
    }
    let count = LLVMCountParamTypes(signature) as usize;
    let mut params = vec![std::ptr::null_mut(); count];
    if count != 0 {
        LLVMGetParamTypes(signature, params.as_mut_ptr());
    }
    // An admitted aggregate parameter passes a struct formal through;
    // the operand gate in `check_function` keeps its argument a formal.
    for param in params {
        if aggregate_fields(param).is_none() && !matches!(width(param)?, 64 | 128) {
            return Err(ValidationError("unsupported LLVM call signature"));
        }
    }
    Ok(())
}

/// Signature gate for an admitted `opt-o2` intrinsic: the scalar
/// family checks `(widths...) -> i64` (abs takes its `i1 immarg`), the
/// `with.overflow` family `(i64, i64) -> {i64, i1}`.
unsafe fn check_intrinsic_signature(signature: LLVMTypeRef, name: Option<&str>) -> Result<()> {
    let bad = || ValidationError("unsupported LLVM intrinsic signature");
    if LLVMIsFunctionVarArg(signature) != 0 {
        return Err(bad());
    }
    let count = LLVMCountParamTypes(signature);
    let mut params = vec![std::ptr::null_mut(); count as usize];
    if count != 0 {
        LLVMGetParamTypes(signature, params.as_mut_ptr());
    }
    let ret = LLVMGetReturnType(signature);
    if let Some(&(_, expected_params, expected_ret)) = OPT_SCALAR_INTRINSICS
        .iter()
        .find(|(n, ..)| Some(*n) == name)
    {
        if width(ret)? != expected_ret || params.len() != expected_params.len() {
            return Err(bad());
        }
        for (param, expected) in params.iter().zip(expected_params) {
            if width(*param)? != *expected {
                return Err(bad());
            }
        }
        return Ok(());
    }
    if name.is_some_and(|n| OPT_OVERFLOW_INTRINSICS.contains(&n)) {
        if !overflow_pair(ret) || params.len() != 2 {
            return Err(bad());
        }
        for param in params {
            if width(param)? != 64 {
                return Err(bad());
            }
        }
        return Ok(());
    }
    Err(bad())
}

unsafe fn check_function_header(
    function: LLVMValueRef,
    params: usize,
    optimized: bool,
) -> Result<()> {
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
        let count = LLVMGetAttributeCountAtIndex(function, index);
        // Function-scope attributes describe facts the pipeline proved.
        // Parameter/return attributes (`noundef`, `signext`) constrain
        // the input domain and stay rejected in every profile.
        if count != 0 && !(optimized && index == u32::MAX) {
            return Err(ValidationError("LLVM attributes unsupported"));
        }
        if optimized && index == u32::MAX && count != 0 {
            check_opt_function_attributes(function, count)?;
        }
    }
    Ok(())
}

/// Function-scope attribute kinds `default<O2>` emits on a canonical
/// snapshot, resolved once by name: descriptive codegen facts the
/// comparison ignores. Semantic contracts — `noreturn` (UB on a
/// returning body), `returns_twice`, `null_pointer_is_valid`, legacy
/// memory kinds, `speculatable`, sanitizer/fp knobs and every string
/// attribute — stay rejected because the comparison cannot read what
/// they claim.
const OPT_FUNCTION_ATTRS: &[&str] = &[
    "alwaysinline",
    "cold",
    "convergent",
    "hot",
    "inlinehint",
    "minsize",
    "mustprogress",
    "nofree",
    "noinline",
    "norecurse",
    "nosync",
    "nounwind",
    "optforfuzzing",
    "optnone",
    "optsize",
    "willreturn",
];

unsafe fn check_opt_function_attributes(function: LLVMValueRef, count: u32) -> Result<()> {
    let allowed: std::collections::HashSet<u32> = OPT_FUNCTION_ATTRS
        .iter()
        .map(|name| LLVMGetEnumAttributeKindForName(name.as_ptr() as *const i8, name.len()))
        .filter(|kind| *kind != 0)
        .collect();
    let memory_kind = LLVMGetEnumAttributeKindForName(b"memory".as_ptr() as *const i8, 6);
    let mut attrs = vec![std::ptr::null_mut(); count as usize];
    LLVMGetAttributesAtIndex(function, u32::MAX, attrs.as_mut_ptr());
    for attr in attrs {
        // Only enum attributes are read: a string attribute's claim is
        // opaque to the whitelist, so it stays out.
        if LLVMIsEnumAttribute(attr) == 0 {
            return Err(ValidationError("LLVM attributes unsupported"));
        }
        let kind = LLVMGetEnumAttributeKind(attr);
        if allowed.contains(&kind) {
            continue;
        }
        // `memory(...)` claims an upper bound on observable effects.
        // Every load/store the IR profile admits goes through a
        // function-local `alloca` — externally unobservable — so a
        // claim is only inconsistent when a call's callee could
        // observably touch more memory than the claim allows.
        if kind == memory_kind
            && memory_claim_consistent(function, LLVMGetEnumAttributeValue(attr))?
        {
            continue;
        }
        return Err(ValidationError("LLVM attributes unsupported"));
    }
    Ok(())
}

/// A `memory` claim is consistent when the function cannot observably
/// touch more memory than `claimed`: admitted loads/stores go through
/// local `alloca`s (unobservable in LLVM's effect lattice), so the only
/// observable access is a call — the callee's own `memory` claim must
/// be a subset of `claimed`. A defined callee with no `memory`
/// attribute conservatively counts as full access.
unsafe fn memory_claim_consistent(function: LLVMValueRef, claimed: u64) -> Result<bool> {
    let mut block = LLVMGetFirstBasicBlock(function);
    while !block.is_null() {
        let mut inst = LLVMGetFirstInstruction(block);
        while !inst.is_null() {
            if LLVMGetInstructionOpcode(inst) == LLVMOpcode::LLVMCall {
                let callee = LLVMGetCalledValue(inst);
                if LLVMGetValueKind(callee) != llvm_sys::LLVMValueKind::LLVMFunctionValueKind {
                    return Ok(false);
                }
                if LLVMIsDeclaration(callee) == 0 && callee != function {
                    let callee_claim =
                        callee_memory_claim(callee).unwrap_or_else(unrestricted_memory_claim);
                    if callee_claim & !claimed != 0 {
                        return Ok(false);
                    }
                }
            }
            inst = LLVMGetNextInstruction(inst);
        }
        block = LLVMGetNextBasicBlock(block);
    }
    Ok(true)
}

/// The `memory` enum attribute's encoded value for `readwrite` —
/// what a callee with no `memory` attribute can do. Probing parses a
/// one-line declaration once because the encoding is an LLVM-internal
/// bit layout; an attribute-less callee maps to this, not `u64::MAX`
/// (which includes bits no valid claim ever sets and so rejects even
/// an unrestricted caller's `memory(readwrite)`).
fn unrestricted_memory_claim() -> u64 {
    static CLAIM: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *CLAIM.get_or_init(|| {
        let context = LlvmContext::create();
        let module = context
            .create_module_from_ir(MemoryBuffer::create_from_memory_range_copy(
                b"declare i64 @f() memory(readwrite)\n",
                "memory_probe",
            ))
            .expect("memory attribute probe");
        let function = module.get_function("f").expect("probe function");
        unsafe { callee_memory_claim(function.as_value_ref()) }
            .expect("memory(readwrite) encodes a claim")
    })
}

/// The callee's claimed whole-function memory bound: the `memory` enum
/// attribute's encoded effects value, `0` meaning `memory(none)`.
unsafe fn callee_memory_claim(callee: LLVMValueRef) -> Option<u64> {
    let count = LLVMGetAttributeCountAtIndex(callee, u32::MAX);
    let memory_kind = LLVMGetEnumAttributeKindForName(b"memory".as_ptr() as *const i8, 6);
    let mut attrs = vec![std::ptr::null_mut(); count as usize];
    LLVMGetAttributesAtIndex(callee, u32::MAX, attrs.as_mut_ptr());
    for attr in attrs {
        if LLVMIsEnumAttribute(attr) != 0 && LLVMGetEnumAttributeKind(attr) == memory_kind {
            return Some(LLVMGetEnumAttributeValue(attr));
        }
    }
    None
}

/// A member's own result shape for the module pre-screen: an admitted
/// aggregate return is checked as that many slots, anything else as
/// the scalar i64/i1 return.
unsafe fn member_result_shape(member: LLVMValueRef) -> Option<usize> {
    aggregate_fields(LLVMGetReturnType(LLVMGlobalGetValueType(member))).map(|n| n as usize)
}

unsafe fn array_param_formal_type(ty: LLVMTypeRef) -> bool {
    if LLVMGetTypeKind(ty) != LLVMTypeKind::LLVMStructTypeKind
        || LLVMIsPackedStruct(ty) != 0
        || LLVMIsLiteralStruct(ty) == 0
        || LLVMCountStructElementTypes(ty) != 2
    {
        return false;
    }
    let len = LLVMStructGetTypeAtIndex(ty, 0);
    let data = LLVMStructGetTypeAtIndex(ty, 1);
    LLVMGetTypeKind(len) == LLVMTypeKind::LLVMIntegerTypeKind
        && LLVMGetIntTypeWidth(len) == 64
        && LLVMGetTypeKind(data) == LLVMTypeKind::LLVMPointerTypeKind
        && LLVMGetPointerAddressSpace(data) == 0
}

unsafe fn array_param_argument_index(
    function: LLVMValueRef,
    value: LLVMValueRef,
    array_param_mask: &[bool],
) -> Option<usize> {
    if LLVMIsAArgument(value).is_null() {
        return None;
    }
    (0..LLVMCountParams(function) as usize)
        .find(|&index| LLVMGetParam(function, index as u32) == value)
        .filter(|&index| array_param_mask.get(index) == Some(&true))
}

unsafe fn is_array_param_pointer(
    function: LLVMValueRef,
    value: LLVMValueRef,
    array_param_mask: &[bool],
) -> bool {
    if LLVMGetTypeKind(LLVMTypeOf(value)) != LLVMTypeKind::LLVMPointerTypeKind
        || LLVMIsAInstruction(value).is_null()
    {
        return false;
    }
    match LLVMGetInstructionOpcode(value) {
        LLVMOpcode::LLVMExtractValue => {
            LLVMGetNumIndices(value) == 1
                && *LLVMGetIndices(value) == 1
                && array_param_argument_index(function, LLVMGetOperand(value, 0), array_param_mask)
                    .is_some()
        }
        LLVMOpcode::LLVMGetElementPtr => {
            is_array_param_pointer(function, LLVMGetOperand(value, 0), array_param_mask)
        }
        _ => false,
    }
}

/// `result_shape` is the source result layout: `None` for a scalar
/// result (an i64/i1 return), `Some(n)` for a struct result, whose
/// return must be an admitted aggregate of exactly `n` slots.
/// The admission gates a member check runs under: which relaxations
/// the active profile grants the emitted IR.
#[derive(Clone, Copy, Default)]
struct CheckGates<'a> {
    /// Cycles pass the CFG screen; the per-atom comparison re-gates
    /// them against each model's loop admission.
    allow_cycles: bool,
    /// Bounded-memory profile: `alloca`/`store`/`load`/`getelementptr`
    /// are admitted at their provenance-preserving positions.
    bounded_memory: bool,
    /// `f64` arithmetic (`fadd`/`fsub`/`fmul`/`fdiv` and `fcmp oeq`) is
    /// admitted, decoded and encoded through the shared
    /// `f64_arith`/`f64_eq` helpers.
    f64_arithmetic: bool,
    /// Post-`default<O2>` profile surface (the canonical optimizer
    /// intrinsics and instruction shapes).
    optimized: bool,
    /// Declared parameter positions backed by read-only `[i64]` models.
    array_param_mask: Option<&'a [bool]>,
    /// Only the V26 native-module admission enables array-parameter IR.
    array_params_admitted: bool,
}

unsafe fn check_function(
    function: LLVMValueRef,
    params: usize,
    result_shape: Option<usize>,
    result_leaf_doubles: Option<&[bool]>,
    gates: CheckGates<'_>,
) -> Result<()> {
    check_function_header(function, params, gates.optimized)?;
    let array_param_mask = gates.array_param_mask.unwrap_or(&[]);
    if gates
        .array_param_mask
        .is_some_and(|mask| mask.len() != params)
    {
        return Err(ValidationError("unsupported LLVM signature"));
    }
    for index in 0..params {
        let param_type = LLVMTypeOf(LLVMGetParam(function, index as u32));
        let is_array_param = array_param_mask.get(index).copied().unwrap_or(false);
        if is_array_param {
            if !gates.array_params_admitted {
                return Err(ValidationError("array parameter outside profile"));
            }
            if !array_param_formal_type(param_type) {
                return Err(ValidationError("LLVM array parameter type mismatch"));
            }
        } else if array_param_formal_type(param_type) {
            return Err(ValidationError("unsupported use of array parameter memory"));
        }
    }
    let ret = LLVMGetReturnType(LLVMGlobalGetValueType(function));
    match (result_shape, result_leaf_doubles) {
        (None, leaf_kinds) => {
            // Scalar result: an `i64` (a `bool` carries 0/1) or a
            // `double` for a pass-through `f64` result — the declared
            // kind decides which, so an i64/f64 swap fails here.
            let expected = leaf_kinds.and_then(|k| k.first()).copied();
            if expected.is_some() && leaf_is_double(ret) != expected
                || !matches!(width(ret)?, 64 | 128)
            {
                return Err(ValidationError("unsupported LLVM signature"));
            }
        }
        (Some(slots), Some(leaf_kinds)) => {
            if aggregate_leaf_doubles(ret).as_deref() != Some(leaf_kinds)
                || aggregate_fields(ret) != Some(slots as u32)
            {
                return Err(ValidationError("unsupported LLVM signature"));
            }
        }
        (Some(slots), None) => {
            if aggregate_fields(ret) != Some(slots as u32) {
                return Err(ValidationError("unsupported LLVM signature"));
            }
        }
    }
    if LLVMCountBasicBlocks(function) == 0 || LLVMCountBasicBlocks(function) as usize > MAX_BLOCKS {
        return Err(ValidationError("LLVM block limit"));
    }
    let mut block = LLVMGetFirstBasicBlock(function);
    let mut count = 0;
    let mut colors = HashMap::new();
    while !block.is_null() {
        if !gates.allow_cycles {
            acyclic(block, &mut colors)?;
        }
        let mut inst = LLVMGetFirstInstruction(block);
        while !inst.is_null() {
            count += 1;
            if count > MAX_INSTRUCTIONS {
                return Err(ValidationError("LLVM instruction limit"));
            }
            if LLVMHasMetadata(inst) != 0 {
                return Err(ValidationError("LLVM metadata unsupported"));
            }
            let opcode = LLVMGetInstructionOpcode(inst);
            if has_fast_math(inst) {
                return Err(ValidationError("fast-math flags unsupported"));
            }
            for i in 0..LLVMGetNumOperands(inst) {
                let value = LLVMGetOperand(inst, i as u32);
                // A call's last operand is the callee itself — a
                // function reference, not an integer value.
                let is_callee = opcode == LLVMOpcode::LLVMCall && i == LLVMGetNumOperands(inst) - 1;
                // An aggregate is an admitted struct formal, an
                // `insertvalue` chain or a call's struct result. It is
                // read by `extractvalue`, extended by `insertvalue`,
                // passed to a call, returned or merged by
                // `phi` (the emitter lowers a branch-merged composite to
                // a `{i64, ...}` phi under composite v7) — never `select`
                // or stored. A constant aggregate (the
                // emitter's folded literal) holds integer or `undef`
                // elements only; an `undef` slot stays undefined and
                // fails closed wherever it is read.
                // A pointer operand is admitted only at its provenance-
                // preserving positions under the bounded-memory profile:
                // the base of a `getelementptr`, the address of a
                // `load`, the address of a `store`. Everywhere else a
                // pointer would escape tracking, so it fails closed.
                if array_param_argument_index(function, value, array_param_mask).is_some() {
                    let array_extract = opcode == LLVMOpcode::LLVMExtractValue
                        && i == 0
                        && LLVMGetNumIndices(inst) == 1
                        && *LLVMGetIndices(inst) <= 1;
                    if !array_extract {
                        return Err(ValidationError("unsupported use of array parameter memory"));
                    }
                    continue;
                }
                if is_array_param_pointer(function, value, array_param_mask) {
                    let pointer_ok = gates.array_params_admitted
                        && matches!(opcode, LLVMOpcode::LLVMGetElementPtr | LLVMOpcode::LLVMLoad)
                        && i == 0;
                    if !pointer_ok {
                        return Err(ValidationError("unsupported use of array parameter memory"));
                    }
                    continue;
                }
                if !is_callee
                    && LLVMGetTypeKind(LLVMTypeOf(value)) == LLVMTypeKind::LLVMPointerTypeKind
                {
                    let pointer_ok = gates.bounded_memory
                        && match opcode {
                            LLVMOpcode::LLVMGetElementPtr | LLVMOpcode::LLVMLoad => i == 0,
                            LLVMOpcode::LLVMStore => i == 1,
                            _ => false,
                        };
                    if !pointer_ok {
                        return Err(ValidationError("unsupported LLVM pointer operand"));
                    }
                    continue;
                }
                // An `alloca` may carry an element-count operand; only
                // the single-element form is admitted (the count is a
                // constant 1 when present).
                if opcode == LLVMOpcode::LLVMAlloca {
                    if i != 0
                        || LLVMIsAConstantInt(value).is_null()
                        || LLVMConstIntGetZExtValue(value) != 1
                    {
                        return Err(ValidationError("unsupported LLVM alloca"));
                    }
                    continue;
                }
                if !is_callee && aggregate_fields(LLVMTypeOf(value)).is_some() {
                    let use_ok = (matches!(
                        opcode,
                        LLVMOpcode::LLVMExtractValue | LLVMOpcode::LLVMInsertValue
                    ) && i == 0)
                        // A whole nested field write `o.inner = w` inserts an
                        // aggregate at operand 1 — a leaf `i64` insert is
                        // handled by the non-aggregate gate.
                        || (opcode == LLVMOpcode::LLVMInsertValue && i == 1)
                        || opcode == LLVMOpcode::LLVMCall
                        || opcode == LLVMOpcode::LLVMPHI
                        || opcode == LLVMOpcode::LLVMRet;
                    let source_ok = !LLVMIsAArgument(value).is_null()
                        || (!LLVMIsAInstruction(value).is_null()
                            && matches!(
                                LLVMGetInstructionOpcode(value),
                                LLVMOpcode::LLVMExtractValue
                                    | LLVMOpcode::LLVMInsertValue
                                    | LLVMOpcode::LLVMCall
                                    | LLVMOpcode::LLVMPHI
                            ))
                        || constant_aggregate(value, gates.optimized);
                    if !use_ok || !source_ok {
                        return Err(ValidationError("unsupported LLVM aggregate value"));
                    }
                    continue;
                }
                if LLVMIsUndef(value) != 0 || LLVMIsPoison(value) != 0 {
                    return Err(ValidationError("undef or poison unsupported"));
                }
                if !is_callee && LLVMValueIsBasicBlock(value) == 0 {
                    // The `*.with.overflow` `{i64, i1}` is read or
                    // extended whole under the opt-o2 profiles — its
                    // leaf gates run at `extractvalue`/`insertvalue`.
                    if !(gates.optimized
                        && matches!(
                            opcode,
                            LLVMOpcode::LLVMExtractValue | LLVMOpcode::LLVMInsertValue
                        )
                        && overflow_pair(LLVMTypeOf(value)))
                    {
                        width(LLVMTypeOf(value))?;
                        if LLVMIsConstant(value) != 0
                            && LLVMIsAConstantInt(value).is_null()
                            && LLVMIsAConstantFP(value).is_null()
                        {
                            return Err(ValidationError("unsupported LLVM constant"));
                        }
                    }
                }
            }
            match opcode {
                LLVMOpcode::LLVMRet | LLVMOpcode::LLVMBr | LLVMOpcode::LLVMUnreachable => (),
                // `let a = [x, ...]` lowers to an entry-block
                // `alloca [N x i64]` — N bounded so the SMT array's
                // extent stays concrete.
                LLVMOpcode::LLVMAlloca => {
                    let allocated = LLVMGetAllocatedType(inst);
                    let element = LLVMGetElementType(allocated);
                    let slots = LLVMGetArrayLength2(allocated);
                    if !gates.bounded_memory
                        || block != LLVMGetEntryBasicBlock(function)
                        || LLVMGetTypeKind(allocated) != LLVMTypeKind::LLVMArrayTypeKind
                        || LLVMGetTypeKind(element) != LLVMTypeKind::LLVMIntegerTypeKind
                        || LLVMGetIntTypeWidth(element) != 64
                        || slots == 0
                        || slots > u64::from(MAX_ARRAY_SLOTS)
                    {
                        return Err(ValidationError("unsupported LLVM alloca"));
                    }
                }
                // One-level indexing only: the emitter lowers `a[i]` to
                // `gep i64, ptr, i64 %i` or the literal-initialisation
                // `gep [N x i64], ptr, i64 0, i64 %i`. Every index is an
                // i64 (the operand gate above) and a two-index form's
                // outer index must be constant 0. The source element
                // type must match the form too: `i64` for the dynamic
                // access, an `i64` array for the initialisation — a
                // `gep i8` would index bytes, not cells, and break the
                // encoder's element-count offsets.
                LLVMOpcode::LLVMGetElementPtr => {
                    let operands = LLVMGetNumOperands(inst) as u32;
                    let outer_zero = operands != 3
                        || (!LLVMIsAConstantInt(LLVMGetOperand(inst, 1)).is_null()
                            && LLVMConstIntGetZExtValue(LLVMGetOperand(inst, 1)) == 0);
                    let source = LLVMGetGEPSourceElementType(inst);
                    let cell_indexed = match LLVMGetTypeKind(source) {
                        LLVMTypeKind::LLVMArrayTypeKind => {
                            operands == 3
                                && LLVMGetTypeKind(LLVMGetElementType(source))
                                    == LLVMTypeKind::LLVMIntegerTypeKind
                                && LLVMGetIntTypeWidth(LLVMGetElementType(source)) == 64
                        }
                        LLVMTypeKind::LLVMIntegerTypeKind => {
                            operands == 2 && LLVMGetIntTypeWidth(source) == 64
                        }
                        _ => false,
                    };
                    let array_param_pointer = gates.array_params_admitted
                        && is_array_param_pointer(
                            function,
                            LLVMGetOperand(inst, 0),
                            array_param_mask,
                        );
                    // The element index is the last operand and must be
                    // an i64: a wider or narrower index would reach the
                    // encoder's cell arithmetic at a different width —
                    // a sort mismatch, not a compare.
                    let index_i64 = LLVMGetTypeKind(LLVMTypeOf(LLVMGetOperand(inst, operands - 1)))
                        == LLVMTypeKind::LLVMIntegerTypeKind
                        && LLVMGetIntTypeWidth(LLVMTypeOf(LLVMGetOperand(inst, operands - 1)))
                            == 64;
                    if (!gates.bounded_memory && !array_param_pointer)
                        || !(2..=3).contains(&operands)
                        || !outer_zero
                        || !cell_indexed
                        || !index_i64
                    {
                        return Err(ValidationError("unsupported LLVM getelementptr"));
                    }
                }
                // Loads read an i64 cell — a pointer-typed load would
                // escape provenance tracking and is rejected by the
                // `width` check the same way an aggregate load is.
                // `s[i]` byte reads on `str_lit` globals emit
                // `gep i8`/`load i8`, which this 64-bit-cell gate
                // refuses. Such sources already fail closed earlier:
                // the bundle build's general verifier rejects `s[i]`
                // as out-of-bounds, so they never reach this gate —
                // byte-cell coverage in either model is a separate
                // slice (checked by the v23 bundle test).
                LLVMOpcode::LLVMLoad => {
                    let array_param_pointer = gates.array_params_admitted
                        && is_array_param_pointer(
                            function,
                            LLVMGetOperand(inst, 0),
                            array_param_mask,
                        );
                    // An alignment claim above the i64 cell's 8 bytes
                    // makes the load UB on a lawful input — the model
                    // assumes the element access, so over-aligned loads
                    // stay refused.
                    if (!gates.bounded_memory && !array_param_pointer)
                        || width(LLVMTypeOf(inst))? != 64
                        || LLVMGetAlignment(inst) > 8
                    {
                        return Err(ValidationError("unsupported LLVM load"));
                    }
                }
                // Stores write an i64 cell — storing a pointer would let
                // an array hold an untracked address.
                LLVMOpcode::LLVMStore => {
                    if !gates.bounded_memory || width(LLVMTypeOf(LLVMGetOperand(inst, 0)))? != 64 {
                        return Err(ValidationError("unsupported LLVM store"));
                    }
                }
                LLVMOpcode::LLVMAdd | LLVMOpcode::LLVMSub | LLVMOpcode::LLVMMul => {
                    unflagged_arithmetic(inst)?;
                    width(LLVMTypeOf(inst))?;
                }
                // Division, remainder and shifts are encoded against the
                // i64 source semantics; the i1 forms this whitelist also
                // admits for `add`/`sub`/`mul` have no counterpart there.
                LLVMOpcode::LLVMSDiv
                | LLVMOpcode::LLVMSRem
                | LLVMOpcode::LLVMUDiv
                | LLVMOpcode::LLVMURem
                | LLVMOpcode::LLVMShl
                | LLVMOpcode::LLVMLShr
                | LLVMOpcode::LLVMAShr => {
                    unflagged_arithmetic(inst)?;
                    if !matches!(width(LLVMTypeOf(inst))?, 64 | 128) {
                        return Err(ValidationError("non-i64/i128 division or shift"));
                    }
                }
                LLVMOpcode::LLVMCall => {
                    check_call(inst, gates.optimized)?;
                }
                // A one-index read of an admitted aggregate — the
                // result is an i64 leaf or an admitted aggregate
                // (extracting a nested struct field).
                LLVMOpcode::LLVMExtractValue => {
                    if array_param_argument_index(
                        function,
                        LLVMGetOperand(inst, 0),
                        array_param_mask,
                    )
                    .is_some()
                    {
                        let index = *LLVMGetIndices(inst);
                        let result = LLVMTypeOf(inst);
                        let supported = match index {
                            0 => {
                                LLVMGetTypeKind(result) == LLVMTypeKind::LLVMIntegerTypeKind
                                    && LLVMGetIntTypeWidth(result) == 64
                            }
                            1 => {
                                LLVMGetTypeKind(result) == LLVMTypeKind::LLVMPointerTypeKind
                                    && LLVMGetPointerAddressSpace(result) == 0
                            }
                            _ => false,
                        };
                        if LLVMGetNumIndices(inst) != 1 || !supported {
                            return Err(ValidationError(
                                "unsupported use of array parameter memory",
                            ));
                        }
                        inst = LLVMGetNextInstruction(inst);
                        continue;
                    }
                    if LLVMGetNumIndices(inst) != 1 {
                        return Err(ValidationError("unsupported LLVM aggregate value"));
                    }
                    let result = LLVMTypeOf(inst);
                    // The `i1` flag leaf of a `{i64, i1}`
                    // `with.overflow` result reads under opt-o2.
                    if aggregate_fields(result).is_none()
                        && !matches!(width(result)?, 64 | 128)
                        && !(gates.optimized
                            && width(result)? == 1
                            && overflow_pair(LLVMTypeOf(LLVMGetOperand(inst, 0))))
                    {
                        return Err(ValidationError("unsupported LLVM aggregate value"));
                    }
                }
                // One field of an admitted aggregate set from an i64
                // leaf or an admitted aggregate (writing a nested
                // struct field whole).
                LLVMOpcode::LLVMInsertValue => {
                    let inserted = LLVMTypeOf(LLVMGetOperand(inst, 1));
                    let inst_ty = LLVMTypeOf(inst);
                    let kinds = aggregate_leaf_doubles(inst_ty);
                    let range = (LLVMGetNumIndices(inst) == 1)
                        .then(|| aggregate_leaf_range(inst_ty, unsafe { *LLVMGetIndices(inst) }))
                        .flatten();
                    // The written slot's kind must match too: a
                    // `double` leaf inserted into an i64 field (or the
                    // reverse) would pass a count-only check with the
                    // wrong ABI.
                    let ok = match (kinds.as_deref(), range) {
                        (Some(kinds), Some((offset, span))) if offset + span <= kinds.len() => {
                            match leaf_is_double(inserted) {
                                // A 128-bit leaf inserts across its two
                                // ABI slots — both non-`double`
                                // (`pure-i64-composite/v21`).
                                Some(_)
                                    if LLVMGetTypeKind(inserted)
                                        == LLVMTypeKind::LLVMIntegerTypeKind
                                        && LLVMGetIntTypeWidth(inserted) == 128 =>
                                {
                                    span == 2 && kinds[offset..offset + span] == [false, false]
                                }
                                Some(d) => span == 1 && kinds[offset] == d,
                                None => aggregate_leaf_doubles(inserted)
                                    .map(|inner| inner == kinds[offset..offset + span])
                                    .unwrap_or(false),
                            }
                        }
                        _ => false,
                    };
                    if !ok {
                        // The `{i64, i1}` `with.overflow` result may be
                        // rebuilt whole under opt-o2: index 0 takes an
                        // i64, index 1 an i1.
                        let pair_ok = gates.optimized
                            && overflow_pair(inst_ty)
                            && LLVMGetNumIndices(inst) == 1
                            && match unsafe { *LLVMGetIndices(inst) } {
                                0 => leaf_is_double(inserted) == Some(false),
                                1 => {
                                    LLVMGetTypeKind(inserted) == LLVMTypeKind::LLVMIntegerTypeKind
                                        && LLVMGetIntTypeWidth(inserted) == 1
                                }
                                _ => false,
                            };
                        if !pair_ok {
                            return Err(ValidationError("unsupported LLVM aggregate value"));
                        }
                    }
                }
                LLVMOpcode::LLVMAnd | LLVMOpcode::LLVMOr | LLVMOpcode::LLVMXor => {
                    // `or disjoint` and friends are poison
                    // strengthenings — the same gate as `add nsw`.
                    unflagged_arithmetic(inst)?;
                    width(LLVMTypeOf(inst))?;
                }
                // `f64` arithmetic (`pure-i64-composite/v19`): the
                // emitter's `fadd`/`fsub`/`fmul`/`fdiv` on `double`
                // leaves — the operand gate above already rejects
                // narrower float types, and the universal fast-math
                // screen rejects flagged forms. `fdiv` is total in
                // IEEE (x/0.0 = ±inf), no definedness obligation.
                LLVMOpcode::LLVMFAdd
                | LLVMOpcode::LLVMFSub
                | LLVMOpcode::LLVMFMul
                | LLVMOpcode::LLVMFDiv
                    if gates.f64_arithmetic =>
                {
                    width(LLVMTypeOf(inst))?;
                }
                // `x == y` on `f64` lowers to `fcmp oeq` plus a `zext`
                // — the only predicate the emitter produces; every
                // other predicate stays unsupported.
                LLVMOpcode::LLVMFCmp
                    if gates.f64_arithmetic
                        && LLVMGetFCmpPredicate(inst)
                            == llvm_sys::LLVMRealPredicate::LLVMRealOEQ
                        && width(LLVMTypeOf(inst))? == 1 =>
                {
                    // operands are `double` — the operand gate's width
                    // check already proved it.
                }
                LLVMOpcode::LLVMICmp
                | LLVMOpcode::LLVMZExt
                | LLVMOpcode::LLVMSExt
                | LLVMOpcode::LLVMTrunc
                | LLVMOpcode::LLVMSelect
                | LLVMOpcode::LLVMPHI => {
                    if aggregate_fields(LLVMTypeOf(inst)).is_none() {
                        width(LLVMTypeOf(inst))?;
                    }
                }
                // `freeze` is a `default<O2>` canonical form: it pins a
                // poison operand to an arbitrary value — the operand
                // itself is the sound encoding without poison.
                LLVMOpcode::LLVMFreeze if gates.optimized => {
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
/// flagged arithmetic instruction stays unsupported. `sdiv`/`udiv`/`shl`/
/// `ashr`/`lshr` can carry nsw/nuw (`shl`) or `exact`, which likewise
/// strengthen the poison domain, so they are rejected by the same
/// whole-token check.
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
        .any(|token| matches!(token, "nsw" | "nuw" | "exact" | "disjoint"))
    {
        return Err(ValidationError("flagged LLVM arithmetic unsupported"));
    }
    Ok(())
}

unsafe fn check_native_module(module: LLVMModuleRef, user_main: LLVMValueRef) -> Result<()> {
    check_module_environment(module, false)?;
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
    check_native_wrapper(main, user_main)
}

/// Multi-atom variant of `check_native_module`: the C `main` wrapper
/// keeps its fixed call/trunc/ret shape against `__mumei_user_main`,
/// and every other member is screened by the shared function profile
/// against its own arity — each is a potential call target.
struct NativeModuleMembers<'a> {
    array_params: &'a BTreeMap<String, Vec<bool>>,
    array_params_admitted: bool,
    array_results: &'a BTreeSet<String>,
}

unsafe fn check_native_module_multi(
    module: LLVMModuleRef,
    user_main: LLVMValueRef,
    bounded_memory: bool,
    f64_arithmetic: bool,
    str_leaves: bool,
    members: &NativeModuleMembers,
) -> Result<()> {
    check_module_environment(module, str_leaves)?;
    let main = LLVMGetNamedFunction(module, c"main".as_ptr());
    if main.is_null() {
        return Err(ValidationError("missing native main wrapper"));
    }
    let mut member = LLVMGetFirstFunction(module);
    if member.is_null() {
        return Err(ValidationError("module outside bounded function profile"));
    }
    while !member.is_null() {
        if LLVMGetLinkage(member) != llvm_sys::LLVMLinkage::LLVMExternalLinkage {
            return Err(ValidationError("LLVM function linkage unsupported"));
        }
        // The wrapper's i32/pointer signature is outside the function
        // profile; its fixed shape is checked separately. Cycles pass
        // the pre-screen — the per-atom comparison re-gates them
        // against each model's loop admission. The cancellation-poll
        // declaration is a call target constrained by `check_call`,
        // not an obligation.
        if member != main
            && !(LLVMIsDeclaration(member) != 0
                && (callee_name(member).as_deref() == Some(CANCEL_POLL_INTRINSIC)
                    // `[i64]`-returning members lower through heap-backed
                    // fat pointers: the module's allocator/copy
                    // declarations only serve those skipped members, so
                    // they are admitted alongside them and nowhere else.
                    || (!members.array_results.is_empty()
                        && matches!(
                            callee_name(member).as_deref(),
                            Some("malloc") | Some("llvm.memcpy.p0.p0.i64")
                        ))))
        {
            let member_name = callee_name(member);
            if member_name
                .as_ref()
                .is_some_and(|name| members.array_results.contains(name))
            {
                member = LLVMGetNextFunction(member);
                continue;
            }
            let array_param_mask = member_name
                .as_ref()
                .and_then(|name| members.array_params.get(name))
                .map(Vec::as_slice);
            check_function(
                member,
                LLVMCountParams(member) as usize,
                member_result_shape(member),
                None,
                CheckGates {
                    allow_cycles: true,
                    bounded_memory,
                    f64_arithmetic,
                    array_param_mask,
                    array_params_admitted: members.array_params_admitted,
                    ..CheckGates::default()
                },
            )?;
        }
        member = LLVMGetNextFunction(member);
    }
    check_native_wrapper(main, user_main)
}

/// The C `main` wrapper's fixed shape: `call @__mumei_user_main()`,
/// `trunc i64 to i32`, `ret` — nothing else.
unsafe fn check_native_wrapper(main: LLVMValueRef, user_main: LLVMValueRef) -> Result<()> {
    check_function_header(main, 2, false)?;
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
            LLVMOpcode::LLVMBr | LLVMOpcode::LLVMRet | LLVMOpcode::LLVMUnreachable
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

/// What a `call` resolves to under the `opt-o2` profiles: a scalar
/// intrinsic's encoded BV, a `with.overflow` `{i64, i1}` leaf pair, or
/// an ordinary call (encoded as an uninterpreted application).
enum OptIntrinsic<'ctx> {
    Scalar(BV<'ctx>),
    Pair(Vec<Option<BV<'ctx>>>),
    Call,
}

#[derive(Clone)]
enum PointerProvenance<'ctx> {
    Alloca { allocation: usize, offset: BV<'ctx> },
    ArrayParameter { parameter: usize, offset: BV<'ctx> },
}

struct IrEncoder<'ctx, 'renames> {
    ctx: &'ctx Context,
    call_renames: &'renames BTreeMap<String, String>,
    states: usize,
    /// `(path condition, defined, producer, rescuable)` obligations
    /// for `sdiv`/`srem`/`shl`/`ashr`/`abs` and pointer bounds: the
    /// instruction's operands must satisfy the condition wherever the
    /// path executes it, or the IR misbehaves there. `producer` is the
    /// instruction the value came from (0 for sink-level obligations
    /// like `unreachable` and store bounds): a `freeze` that covers
    /// every use of a producer drops its `rescuable` obligations —
    /// only poison obligations (`shl`/`ashr`/`lshr`, `abs` on
    /// `i64::MIN`) are rescuable; a failed division or an
    /// out-of-bounds access is immediate UB no `freeze` can undo.
    definedness: Vec<(Bool<'ctx>, Bool<'ctx>, usize, bool)>,
    /// Whether the opt-o2 whitelist admits the canonical intrinsic and
    /// `{i64, i1}` constant shapes — gates `constant_aggregate`'s
    /// overflow-pair arm.
    optimized: bool,
    /// Bounded-cycle mode (set only when the source model contains an
    /// admitted `while`): `depths` counts each block's visits along the
    /// current path; a revisit past `LOOP_BOUND` drops the path without
    /// recording a return, mirroring the source's bounded unroll.
    loops: bool,
    depths: HashMap<usize, u32>,
    /// Each aggregate value's field slots in field declaration order: a
    /// formal's are the source slots its struct parameter flattens into,
    /// an `insertvalue` chain's the values written so far (`None` for a
    /// slot never written), a call result's the per-field uninterpreted
    /// applications. The walk is depth-first and restores this map at
    /// every conditional branch, so a value re-bound when a deeper path
    /// re-enters its block (a later loop iteration) never leaks into a
    /// sibling path.
    aggregates: HashMap<usize, Vec<Option<BV<'ctx>>>>,
    /// Bounded-memory state: each admitted `alloca`'s cells as a Z3
    /// array `BV64 -> BV64` over an unconstrained base — a cell read
    /// before any `store` yields an arbitrary value, so an
    /// initialisation store the IR dropped stays distinguishable from
    /// the source's definite 0. Restored at conditional branches like
    /// `aggregates`.
    arrays: HashMap<usize, z3::ast::Array<'ctx>>,
    /// Each admitted `alloca`'s element count: the bound every load's
    /// and store's offset must satisfy wherever the access executes —
    /// out-of-bounds is recorded as a definedness obligation, i.e. UB.
    /// Entries never change once written, so unlike `arrays` the map
    /// needs no restore at branches.
    alloca_lens: HashMap<usize, u64>,
    /// Read-only array parameter models, in declared parameter order.
    array_params: Vec<Option<(BV<'ctx>, Array<'ctx>)>>,
    array_param_args: HashMap<usize, usize>,
    /// Pointer provenance: local `alloca` or read-only array parameter
    /// plus an element offset (the whitelist keeps every index i64).
    ptrs: HashMap<usize, PointerProvenance<'ctx>>,
}
impl<'ctx, 'renames> IrEncoder<'ctx, 'renames> {
    fn new(
        ctx: &'ctx Context,
        call_renames: &'renames BTreeMap<String, String>,
        loops: bool,
        optimized: bool,
    ) -> Self {
        Self {
            ctx,
            call_renames,
            states: 0,
            definedness: Vec::new(),
            optimized,
            loops,
            depths: HashMap::new(),
            aggregates: HashMap::new(),
            arrays: HashMap::new(),
            alloca_lens: HashMap::new(),
            array_params: Vec::new(),
            array_param_args: HashMap::new(),
            ptrs: HashMap::new(),
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
            if bits == 128 {
                return const_i128(self.ctx, value);
            }
            return Ok(BV::from_u64(
                self.ctx,
                LLVMConstIntGetZExtValue(value),
                bits,
            ));
        }
        if !LLVMIsAConstantFP(value).is_null() {
            // A `double` literal is the pass-through `f64` leaf's bit
            // pattern — the model compares it as the raw 64 bits.
            let mut loses = 0;
            return Ok(BV::from_u64(
                self.ctx,
                LLVMConstRealGetDouble(value, &mut loses).to_bits(),
                bits,
            ));
        }
        values
            .get(&(value as usize))
            .cloned()
            .ok_or(ValidationError("unresolved LLVM value"))
    }

    /// Record that `offset` must lie inside `alloca`'s extent wherever
    /// the current path executes the access — the emitted IR bounds-
    /// checks every dynamic index, but a mutated or hostile module can
    /// read or write past the array, which is UB rather than a value
    /// difference.
    fn memory_in_bounds(
        &mut self,
        alloca: usize,
        offset: &BV<'ctx>,
        guard: &Bool<'ctx>,
    ) -> Result<()> {
        let slots = *self
            .alloca_lens
            .get(&alloca)
            .ok_or(ValidationError("untracked LLVM pointer"))?;
        let limit = BV::from_u64(self.ctx, slots, 64);
        self.memory_in_bounds_len(&limit, offset, guard)
    }

    fn memory_in_bounds_len(
        &mut self,
        len: &BV<'ctx>,
        offset: &BV<'ctx>,
        guard: &Bool<'ctx>,
    ) -> Result<()> {
        let zero = BV::from_u64(self.ctx, 0, 64);
        self.definedness.push((
            guard.clone(),
            Bool::and(self.ctx, &[&offset.bvsge(&zero), &offset.bvslt(len)]),
            0,
            false,
        ));
        Ok(())
    }

    /// Every slot of an aggregate value, failing closed on a slot no
    /// `insertvalue` wrote.
    unsafe fn aggregate_slots(&self, value: LLVMValueRef) -> Result<Vec<BV<'ctx>>> {
        self.aggregate(value)?
            .into_iter()
            .map(|slot| slot.ok_or(ValidationError("undefined LLVM aggregate slot")))
            .collect()
    }

    /// An aggregate value's slots: a constant's elements (`None` for an
    /// `undef` element) or the slots recorded for a formal,
    /// `insertvalue` or call result.
    unsafe fn aggregate(&self, value: LLVMValueRef) -> Result<Vec<Option<BV<'ctx>>>> {
        if constant_aggregate(value, self.optimized) {
            let mut leaves = Vec::new();
            self.constant_leaves(value, &mut leaves);
            return Ok(leaves);
        }
        self.aggregates
            .get(&(value as usize))
            .cloned()
            .ok_or(ValidationError("unsupported LLVM aggregate value"))
    }

    /// The leaf slots of a constant aggregate, flattened element-first:
    /// `{ {i64 3, i64 4}, i64 5 }` yields `[3, 4, 5]`; an `undef` element
    /// contributes `None` slots for its whole leaf range.
    unsafe fn constant_leaves(&self, value: LLVMValueRef, out: &mut Vec<Option<BV<'ctx>>>) {
        let leaf_count = |v: LLVMValueRef| {
            let ty = LLVMTypeOf(v);
            if overflow_pair(ty) {
                2
            } else {
                aggregate_fields(ty).unwrap_or(1) as usize
            }
        };
        if LLVMIsUndef(value) != 0 || LLVMIsPoison(value) != 0 {
            out.extend(std::iter::repeat_n(None, leaf_count(value)));
            return;
        }
        if !LLVMIsAConstantAggregateZero(value).is_null() {
            out.extend(std::iter::repeat_n(
                Some(BV::from_u64(self.ctx, 0, 64)),
                leaf_count(value),
            ));
            return;
        }
        for i in 0..LLVMGetNumOperands(value) as u32 {
            let element = LLVMGetOperand(value, i);
            if aggregate_fields(LLVMTypeOf(element)).is_some() {
                self.constant_leaves(element, out);
            } else if LLVMGetTypeKind(LLVMTypeOf(element)) == LLVMTypeKind::LLVMIntegerTypeKind
                && LLVMGetIntTypeWidth(LLVMTypeOf(element)) == 128
            {
                // An `i128` element flattens to its two ABI slots
                // (`lo`, `hi`); undef/poison and an unparsable form
                // stay undefined and fail closed on read
                // (`pure-i64-composite/v21`).
                if LLVMIsUndef(element) != 0 || LLVMIsPoison(element) != 0 {
                    out.extend([None, None]);
                } else {
                    match const_i128(self.ctx, element) {
                        Ok(v) => {
                            out.push(Some(v.extract(63, 0)));
                            out.push(Some(v.extract(127, 64)));
                        }
                        Err(_) => out.extend([None, None]),
                    }
                }
            } else if LLVMIsUndef(element) != 0 || LLVMIsPoison(element) != 0 {
                out.push(None);
            } else if !LLVMIsAConstantFP(element).is_null() {
                let mut loses = 0;
                out.push(Some(BV::from_u64(
                    self.ctx,
                    LLVMConstRealGetDouble(element, &mut loses).to_bits(),
                    64,
                )));
            } else {
                out.push(Some(BV::from_u64(
                    self.ctx,
                    LLVMConstIntGetZExtValue(element),
                    64,
                )));
            }
        }
    }

    /// `freeze` pins a poison operand to an arbitrary defined value.
    /// A producer obligation is rescued exactly when every use of that
    /// producer stays inside the operand's instruction cone or is the
    /// freeze itself — a use escaping unfrozen keeps its obligation.
    /// Rescued entries still shape the value: on an input where the
    /// obligation fails the result is arbitrary, so it encodes as a
    /// fresh unconstrained leaf instead of the operand's wrapped value.
    unsafe fn rescue_frozen(
        &mut self,
        inst: LLVMValueRef,
        operand: LLVMValueRef,
        a: BV<'ctx>,
    ) -> BV<'ctx> {
        // The freeze only pins the operand's own chain: a use of the
        // operand outside a `freeze` sees the poison unmasked, so
        // nothing is rescued unless every use of the operand is itself
        // a freeze. A non-instruction user (a constant expression) is
        // equally unfrozen.
        let mut use_ = LLVMGetFirstUse(operand);
        while !use_.is_null() {
            let user = LLVMGetUser(use_);
            if LLVMIsAInstruction(user).is_null()
                || LLVMGetInstructionOpcode(user) != LLVMOpcode::LLVMFreeze
            {
                return a;
            }
            use_ = LLVMGetNextUse(use_);
        }
        let mut cone = std::collections::HashSet::new();
        let mut stack = vec![operand];
        while let Some(value) = stack.pop() {
            if LLVMIsAInstruction(value).is_null() || !cone.insert(value as usize) {
                continue;
            }
            for i in 0..LLVMGetNumOperands(value) as u32 {
                stack.push(LLVMGetOperand(value, i));
            }
        }
        let rescued = |producer: usize| -> bool {
            if !cone.contains(&producer) {
                return false;
            }
            let mut use_ = LLVMGetFirstUse(producer as LLVMValueRef);
            while !use_.is_null() {
                let user = LLVMGetUser(use_);
                if user != inst && !cone.contains(&(user as usize)) {
                    return false;
                }
                use_ = LLVMGetNextUse(use_);
            }
            true
        };
        let mut violated: Vec<Bool<'ctx>> = Vec::new();
        self.definedness
            .retain(|(path, defined, producer, rescuable)| {
                if *rescuable && *producer != 0 && rescued(*producer) {
                    violated.push(Bool::and(self.ctx, &[path, &defined.not()]));
                    false
                } else {
                    true
                }
            });
        if violated.is_empty() {
            return a;
        }
        let poisoned = Bool::or(self.ctx, &violated.iter().collect::<Vec<_>>());
        poisoned.ite(&BV::fresh_const(self.ctx, "frz", a.get_size()), &a)
    }

    /// A call's source callee name and flattened arguments: a struct
    /// argument contributes its field slots, the same arguments in the
    /// same order the source call site applies. `None` for the
    /// cancellation-poll intrinsic.
    unsafe fn call_operands(
        &self,
        inst: LLVMValueRef,
        values: &HashMap<usize, BV<'ctx>>,
    ) -> Result<Option<(String, Vec<BV<'ctx>>)>> {
        let callee = LLVMGetCalledValue(inst);
        let mut name_len = 0;
        let raw_name = LLVMGetValueName2(callee, &mut name_len);
        if raw_name.is_null() {
            return Err(ValidationError("unsupported LLVM call"));
        }
        let name =
            String::from_utf8_lossy(std::slice::from_raw_parts(raw_name as *const u8, name_len))
                .into_owned();
        if name == CANCEL_POLL_INTRINSIC && LLVMIsDeclaration(callee) != 0 {
            return Ok(None);
        }
        // An emitted member name maps back to the source atom name its
        // call sites share with the source encoder (`__mumei_user_main`
        // back to `main` on native paths).
        let name = self
            .call_renames
            .get(name.as_str())
            .cloned()
            .unwrap_or(name);
        let operands = LLVMGetNumArgOperands(inst) as usize;
        let mut args = Vec::with_capacity(operands);
        for index in 0..operands {
            let arg = LLVMGetOperand(inst, index as u32);
            if aggregate_fields(LLVMTypeOf(arg)).is_some() {
                args.extend(self.aggregate_slots(arg)?);
            } else {
                let v = self.value(arg, values)?;
                if v.get_size() == 128 {
                    // A 128-bit argument applies as its two ABI slots
                    // (`lo`, `hi`) — the source encoder's `applied`
                    // expansion order (`pure-i64-composite/v21`).
                    args.push(v.extract(63, 0));
                    args.push(v.extract(127, 64));
                } else {
                    args.push(v);
                }
            }
        }
        Ok(Some((name, args)))
    }

    /// The `default<O2>` canonical intrinsics, encoded with their
    /// source semantics: `umin/umax` = unsigned, `smin/smax` = signed
    /// comparisons; `abs` = negate on negative (with `i64::MIN` poison
    /// when its immarg requires it); `*.with.overflow` = the wrapping
    /// op plus a flag bound 1 exactly where the op overflows — the
    /// no-overflow condition negated.
    unsafe fn opt_intrinsic(
        &mut self,
        inst: LLVMValueRef,
        values: &HashMap<usize, BV<'ctx>>,
        guard: &Bool<'ctx>,
    ) -> Result<OptIntrinsic<'ctx>> {
        let callee = LLVMGetCalledValue(inst);
        if LLVMIsDeclaration(callee) == 0 {
            return Ok(OptIntrinsic::Call);
        }
        let Some(name) = callee_name(callee) else {
            return Ok(OptIntrinsic::Call);
        };
        if OPT_OVERFLOW_INTRINSICS.contains(&name.as_str()) {
            let a = self.value(LLVMGetOperand(inst, 0), values)?;
            let b = self.value(LLVMGetOperand(inst, 1), values)?;
            let (res, overflow) = match name.as_str() {
                "llvm.sadd.with.overflow.i64" => (a.bvadd(&b), a.bvadd_no_overflow(&b, true).not()),
                "llvm.ssub.with.overflow.i64" => (a.bvsub(&b), a.bvsub_no_overflow(&b).not()),
                "llvm.smul.with.overflow.i64" => (a.bvmul(&b), a.bvmul_no_overflow(&b, true).not()),
                // Unsigned overflow is the carry/borrow direction the
                // signed `*_no_underflow` predicates do not describe.
                "llvm.uadd.with.overflow.i64" => {
                    (a.bvadd(&b), a.bvadd_no_overflow(&b, false).not())
                }
                "llvm.usub.with.overflow.i64" => {
                    (a.bvsub(&b), a.bvsub_no_underflow(&b, false).not())
                }
                "llvm.umul.with.overflow.i64" => {
                    (a.bvmul(&b), a.bvmul_no_overflow(&b, false).not())
                }
                _ => unreachable!("table member checked above"),
            };
            let one = BV::from_u64(self.ctx, 1, 64);
            let zero = BV::from_u64(self.ctx, 0, 64);
            return Ok(OptIntrinsic::Pair(vec![
                Some(res),
                Some(overflow.ite(&one, &zero)),
            ]));
        }
        if !OPT_SCALAR_INTRINSICS.iter().any(|(n, ..)| n == &name) {
            return Ok(OptIntrinsic::Call);
        }
        let a = self.value(LLVMGetOperand(inst, 0), values)?;
        let b = self.value(LLVMGetOperand(inst, 1), values)?;
        let value = match name.as_str() {
            "llvm.umin.i64" => a.bvult(&b).ite(&a, &b),
            "llvm.umax.i64" => a.bvugt(&b).ite(&a, &b),
            "llvm.smin.i64" => a.bvslt(&b).ite(&a, &b),
            "llvm.smax.i64" => a.bvsgt(&b).ite(&a, &b),
            "llvm.abs.i64" => {
                // `is_int_min_poison = true` (the canonical form) makes
                // `abs(i64::MIN)` poison — the same corner `sdiv`
                // records. A `false` or non-constant flag wraps.
                let flag = LLVMGetOperand(inst, 1);
                let poison =
                    LLVMIsAConstantInt(flag).is_null() || LLVMConstIntGetZExtValue(flag) != 0;
                if poison {
                    let min = BV::from_i64(self.ctx, i64::MIN, 64);
                    self.definedness
                        .push((guard.clone(), a._eq(&min).not(), inst as usize, true));
                }
                a.bvslt(&BV::from_i64(self.ctx, 0, 64)).ite(&a.bvneg(), &a)
            }
            _ => unreachable!("table member checked above"),
        };
        Ok(OptIntrinsic::Scalar(value))
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
        shapes: &[Option<usize>],
        leaf_doubles: &[Vec<bool>],
        array_params: &[Option<(BV<'ctx>, Array<'ctx>)>],
    ) -> Result<Vec<(Bool<'ctx>, BV<'ctx>)>> {
        if array_params.len() != shapes.len() || leaf_doubles.len() != shapes.len() {
            return Err(ValidationError("LLVM parameter shape mismatch"));
        }
        self.array_params = array_params.to_vec();
        let mut values = HashMap::new();
        let mut slots = inputs.iter();
        for (i, shape) in shapes.iter().enumerate() {
            let param = LLVMGetParam(function, i as u32);
            if array_params[i].is_some() {
                if shape.is_some() || !array_param_formal_type(LLVMTypeOf(param)) {
                    return Err(ValidationError("LLVM array parameter type mismatch"));
                }
                self.array_param_args.insert(param as usize, i);
                continue;
            }
            match shape {
                // A parameter's LLVM width must match the source signature:
                // i64 for every parameter; a declared `bool` carries 0/1 and
                // is further bounded by `model.param_domain`. A declared
                // `i128`/`u128` (`pure-i64-composite/v21`) carries a native
                // `i128` bound to two consecutive 64-bit source slots
                // (`lo`, then `hi` — the order `param_env` binds them).
                None => {
                    if width(LLVMTypeOf(param))? == 128 {
                        let lo = slots
                            .next()
                            .ok_or(ValidationError("LLVM parameter shape mismatch"))?;
                        let hi = slots
                            .next()
                            .ok_or(ValidationError("LLVM parameter shape mismatch"))?;
                        if leaf_doubles.get(i).map(Vec::as_slice) != Some(&[false, false]) {
                            return Err(ValidationError("LLVM parameter type mismatch"));
                        }
                        values.insert(param as usize, hi.concat(lo));
                        continue;
                    }
                    let input = slots
                        .next()
                        .ok_or(ValidationError("LLVM parameter shape mismatch"))?;
                    // The leaf's LLVM type must carry the declared kind
                    // too — an `i64` where the source declares `f64`
                    // (or a `double` where it declares `i64`) would
                    // pass a width-only check with the wrong ABI.
                    if leaf_is_double(LLVMTypeOf(param))
                        != leaf_doubles.get(i).and_then(|k| k.first()).copied()
                        || width(LLVMTypeOf(param))? != input.get_size()
                    {
                        return Err(ValidationError("LLVM parameter type mismatch"));
                    }
                    values.insert(param as usize, input.clone());
                }
                // A struct parameter's aggregate formal must have exactly
                // its field count; element i is the i-th flattened slot.
                Some(fields) => {
                    if aggregate_fields(LLVMTypeOf(param)) != Some(*fields as u32)
                        || aggregate_leaf_doubles(LLVMTypeOf(param)).as_deref()
                            != leaf_doubles.get(i).map(Vec::as_slice)
                    {
                        return Err(ValidationError("LLVM parameter shape mismatch"));
                    }
                    let fields: Vec<Option<BV<'ctx>>> =
                        slots.by_ref().take(*fields).cloned().map(Some).collect();
                    self.aggregates.insert(param as usize, fields);
                }
            }
        }
        if slots.next().is_some() {
            return Err(ValidationError("LLVM parameter shape mismatch"));
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
            // Every leaf under the whitelist is a `ret` or an
            // `unreachable`, so no return edges means every path ends in
            // `unreachable`. Its recorded obligations still classify the
            // function — reachable on a defined input it is
            // UndefinedBehavior rather than a missing-return error.
            if self.loops {
                // Bounded-loop mode also yields no returns when every
                // path stays inside a cycle past `LOOP_BOUND` — that is
                // out of profile, not a malformed function.
                return Err(ValidationError(
                    "no LLVM path exits a loop within the bound",
                ));
            }
            if self.definedness.is_empty() {
                return Err(ValidationError("missing LLVM return"));
            }
        }
        Ok(returns)
    }

    /// Path-state wrapper: counts each entered block (cycles permitted
    /// under `loops`), then delegates to `walk_block`. A block revisited
    /// past `LOOP_BOUND` drops the path silently — such a run exceeds
    /// the source side's unrolled bound too, so it stays outside the
    /// declared equivalence domain instead of comparing.
    unsafe fn block(
        &mut self,
        block: LLVMBasicBlockRef,
        previous: LLVMBasicBlockRef,
        values: HashMap<usize, BV<'ctx>>,
        guard: Bool<'ctx>,
        returns: &mut Vec<(Bool<'ctx>, BV<'ctx>)>,
    ) -> Result<()> {
        self.states += 1;
        if self.states > MAX_STATES {
            return Err(ValidationError("LLVM path expansion limit"));
        }
        if !self.loops {
            return self.walk_block(block, previous, values, guard, returns);
        }
        let depth = self.depths.entry(block as usize).or_insert(0);
        *depth += 1;
        if *depth as usize > mumei_core::semantic_contract::LOOP_BOUND {
            *depth -= 1;
            return Ok(());
        }
        let result = self.walk_block(block, previous, values, guard, returns);
        *self
            .depths
            .get_mut(&(block as usize))
            .unwrap_or_else(|| unreachable!("depth entry dropped")) -= 1;
        result
    }

    unsafe fn walk_block(
        &mut self,
        block: LLVMBasicBlockRef,
        previous: LLVMBasicBlockRef,
        mut values: HashMap<usize, BV<'ctx>>,
        guard: Bool<'ctx>,
        returns: &mut Vec<(Bool<'ctx>, BV<'ctx>)>,
    ) -> Result<()> {
        // Phi results commit together after the block's phi run: each
        // phi's incoming value is resolved against the environment the
        // block was entered with, so a phi that references a sibling
        // phi sees the previous iteration's value, not the freshly
        // bound one. Aggregate phis defer their `self.aggregates`
        // commit for the same reason. (LLVM requires phi nodes to
        // lead a block.)
        let mut pending_phis: Vec<(LLVMValueRef, BV<'ctx>)> = Vec::new();
        let mut pending_aggregates: Vec<(LLVMValueRef, Vec<Option<BV<'ctx>>>)> = Vec::new();
        let mut inst = LLVMGetFirstInstruction(block);
        while !inst.is_null() {
            let op = LLVMGetInstructionOpcode(inst);
            if op == LLVMOpcode::LLVMPHI {
                let i = (0..LLVMCountIncoming(inst))
                    .find(|&i| LLVMGetIncomingBlock(inst, i) == previous)
                    .ok_or(ValidationError("missing LLVM phi edge"))?;
                let incoming = LLVMGetIncomingValue(inst, i);
                if aggregate_fields(LLVMTypeOf(inst)).is_some() {
                    // A composite merged across a branch boundary is a
                    // `{i64, ...}` phi: its slots are the incoming
                    // aggregate's slots on this predecessor path — the
                    // same per-slot merge the source encoder performs.
                    // Slots are resolved eagerly (against the previous
                    // iteration's aggregates) but committed below,
                    // with the scalar phis.
                    pending_aggregates.push((inst, self.aggregate(incoming)?));
                } else {
                    pending_phis.push((inst, self.value(incoming, &values)?));
                }
                inst = LLVMGetNextInstruction(inst);
                continue;
            }
            for (phi, result) in pending_phis.drain(..) {
                values.insert(phi as usize, result);
            }
            for (phi, slots) in pending_aggregates.drain(..) {
                self.aggregates.insert(phi as usize, slots);
            }
            if op == LLVMOpcode::LLVMRet {
                // A returned aggregate compares as its slots laid out
                // the way the source lays out a struct result.
                let operand = LLVMGetOperand(inst, 0);
                let value = if aggregate_fields(LLVMTypeOf(operand)).is_some() {
                    aggregate_value(&self.aggregate_slots(operand)?)
                } else {
                    self.value(operand, &values)?
                };
                returns.push((guard, value));
                return Ok(());
            }
            if op == LLVMOpcode::LLVMInsertValue {
                let aggregate = LLVMGetOperand(inst, 0);
                let mut slots = self.aggregate(aggregate)?;
                let (offset, span) = aggregate_leaf_range(LLVMTypeOf(inst), *LLVMGetIndices(inst))
                    .filter(|(offset, span)| offset + span <= slots.len())
                    .ok_or(ValidationError("unsupported LLVM aggregate value"))?;
                if span > 1 {
                    let inserted_ty = LLVMTypeOf(LLVMGetOperand(inst, 1));
                    if LLVMGetTypeKind(inserted_ty) == LLVMTypeKind::LLVMIntegerTypeKind
                        && LLVMGetIntTypeWidth(inserted_ty) == 128
                    {
                        // Writing a 128-bit leaf splits into its two
                        // ABI slots (`lo`, `hi`).
                        let field = self.value(LLVMGetOperand(inst, 1), &values)?;
                        slots.splice(
                            offset..offset + span,
                            vec![Some(field.extract(63, 0)), Some(field.extract(127, 64))],
                        );
                    } else {
                        // An aggregate operand (writing a nested struct
                        // field whole) splices its leaf slots in.
                        let inserted = self.aggregate(LLVMGetOperand(inst, 1))?;
                        if inserted.len() != span {
                            return Err(ValidationError("unsupported LLVM aggregate value"));
                        }
                        slots.splice(offset..offset + span, inserted);
                    }
                } else {
                    let field = self.value(LLVMGetOperand(inst, 1), &values)?;
                    slots[offset] = Some(field);
                }
                self.aggregates.insert(inst as usize, slots);
                inst = LLVMGetNextInstruction(inst);
                continue;
            }
            if op == LLVMOpcode::LLVMCall {
                // A `*.with.overflow.i64` call produces the `{i64, i1}`
                // pair leaf-first into the aggregate table.
                if let OptIntrinsic::Pair(slots) = self.opt_intrinsic(inst, &values, &guard)? {
                    self.aggregates.insert(inst as usize, slots);
                    inst = LLVMGetNextInstruction(inst);
                    continue;
                }
                if let Some(count) = aggregate_fields(LLVMTypeOf(inst)) {
                    // A struct-returning call: one uninterpreted
                    // application per field slot, the declarations the
                    // source encoder applies at the same call site.
                    let (name, args) = self
                        .call_operands(inst, &values)?
                        .ok_or(ValidationError("unsupported LLVM call"))?;
                    let refs: Vec<&dyn z3::ast::Ast> =
                        args.iter().map(|v| v as &dyn z3::ast::Ast).collect();
                    let slots = (0..count as usize)
                        .map(|i| {
                            call_field_decl(self.ctx, &name, args.len(), i)
                                .apply(&refs)
                                .as_bv()
                                .map(Some)
                                .ok_or(ValidationError("unsupported LLVM call result"))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    self.aggregates.insert(inst as usize, slots);
                    inst = LLVMGetNextInstruction(inst);
                    continue;
                }
            }
            if op == LLVMOpcode::LLVMUnreachable {
                // `unreachable` executes LLVM UB: the path must never run
                // on a defined input — an always-unsatisfiable
                // definedness obligation reports it as UndefinedBehavior
                // and keeps it out of the value comparison.
                self.definedness
                    .push((guard.clone(), Bool::from_bool(self.ctx, false), 0, false));
                return Ok(());
            }
            if op == LLVMOpcode::LLVMBr {
                if LLVMGetNumSuccessors(inst) == 1 {
                    return self.block(LLVMGetSuccessor(inst, 0), block, values, guard, returns);
                }
                let cond = self.condition(&self.value(LLVMGetCondition(inst), &values)?)?;
                // Aggregate slots live in `self.aggregates`, not the
                // per-path `values`: the second successor must see them
                // as they were at this branch, not as the first
                // successor's subtree (a later loop iteration) rebound
                // them.
                let aggregates = self.aggregates.clone();
                let arrays = self.arrays.clone();
                let ptrs = self.ptrs.clone();
                self.block(
                    LLVMGetSuccessor(inst, 0),
                    block,
                    values.clone(),
                    Bool::and(self.ctx, &[&guard, &cond]),
                    returns,
                )?;
                self.aggregates = aggregates;
                self.arrays = arrays;
                self.ptrs = ptrs;
                return self.block(
                    LLVMGetSuccessor(inst, 1),
                    block,
                    values,
                    Bool::and(self.ctx, &[&guard, &cond.not()]),
                    returns,
                );
            }
            if op == LLVMOpcode::LLVMAlloca {
                // `let a = [x, ...]`'s entry-block `alloca` — a fresh
                // array over an unconstrained base. Zero-initialising
                // here instead would let an IR missing an
                // initialisation store still read the source's 0 and
                // mask the difference.
                let sort = z3::Sort::bitvector(self.ctx, 64);
                self.arrays.insert(
                    inst as usize,
                    z3::ast::Array::new_const(
                        self.ctx,
                        format!("array_base_{:x}", inst as usize),
                        &sort,
                        &sort,
                    ),
                );
                self.alloca_lens.insert(
                    inst as usize,
                    LLVMGetArrayLength2(LLVMGetAllocatedType(inst)),
                );
                self.ptrs.insert(
                    inst as usize,
                    PointerProvenance::Alloca {
                        allocation: inst as usize,
                        offset: BV::from_u64(self.ctx, 0, 64),
                    },
                );
                inst = LLVMGetNextInstruction(inst);
                continue;
            }
            if op == LLVMOpcode::LLVMGetElementPtr {
                let provenance = self
                    .ptrs
                    .get(&(LLVMGetOperand(inst, 0) as usize))
                    .cloned()
                    .ok_or(ValidationError("untracked LLVM pointer"))?;
                // Element offsets: the whitelist binds any leading
                // constant 0 of the two-index array form, so summing
                // every index is the addressed cell.
                let mut offset = match &provenance {
                    PointerProvenance::Alloca { offset, .. }
                    | PointerProvenance::ArrayParameter { offset, .. } => offset.clone(),
                };
                for index in 1..LLVMGetNumOperands(inst) {
                    offset =
                        offset.bvadd(&self.value(LLVMGetOperand(inst, index as u32), &values)?);
                }
                let provenance = match provenance {
                    PointerProvenance::Alloca { allocation, .. } => {
                        PointerProvenance::Alloca { allocation, offset }
                    }
                    PointerProvenance::ArrayParameter { parameter, .. } => {
                        PointerProvenance::ArrayParameter { parameter, offset }
                    }
                };
                self.ptrs.insert(inst as usize, provenance);
                inst = LLVMGetNextInstruction(inst);
                continue;
            }
            if op == LLVMOpcode::LLVMStore {
                let cell = self.value(LLVMGetOperand(inst, 0), &values)?;
                let provenance = self
                    .ptrs
                    .get(&(LLVMGetOperand(inst, 1) as usize))
                    .cloned()
                    .ok_or(ValidationError("untracked LLVM pointer"))?;
                let PointerProvenance::Alloca { allocation, offset } = provenance else {
                    return Err(ValidationError("unsupported use of array parameter memory"));
                };
                self.memory_in_bounds(allocation, &offset, &guard)?;
                let array = self
                    .arrays
                    .get(&allocation)
                    .cloned()
                    .ok_or(ValidationError("untracked LLVM pointer"))?;
                self.arrays.insert(allocation, array.store(&offset, &cell));
                inst = LLVMGetNextInstruction(inst);
                continue;
            }
            if op == LLVMOpcode::LLVMLoad {
                let provenance = self
                    .ptrs
                    .get(&(LLVMGetOperand(inst, 0) as usize))
                    .cloned()
                    .ok_or(ValidationError("untracked LLVM pointer"))?;
                let value = match provenance {
                    PointerProvenance::Alloca { allocation, offset } => {
                        self.memory_in_bounds(allocation, &offset, &guard)?;
                        self.arrays
                            .get(&allocation)
                            .cloned()
                            .ok_or(ValidationError("untracked LLVM pointer"))?
                            .select(&offset)
                            .as_bv()
                            .ok_or(ValidationError("unsupported LLVM load"))?
                    }
                    PointerProvenance::ArrayParameter { parameter, offset } => {
                        let (len, store) = self
                            .array_params
                            .get(parameter)
                            .and_then(Option::as_ref)
                            .map(|(len, store)| (len.clone(), store.clone()))
                            .ok_or(ValidationError("untracked array parameter"))?;
                        self.memory_in_bounds_len(&len, &offset, &guard)?;
                        store
                            .select(&offset)
                            .as_bv()
                            .ok_or(ValidationError("unsupported LLVM load"))?
                    }
                };
                values.insert(inst as usize, value);
                inst = LLVMGetNextInstruction(inst);
                continue;
            }
            if op == LLVMOpcode::LLVMExtractValue {
                let aggregate = LLVMGetOperand(inst, 0);
                if let Some(parameter) = self.array_param_args.get(&(aggregate as usize)).copied() {
                    if LLVMGetNumIndices(inst) != 1 {
                        return Err(ValidationError("unsupported use of array parameter memory"));
                    }
                    let (len, _) = self
                        .array_params
                        .get(parameter)
                        .and_then(Option::as_ref)
                        .ok_or(ValidationError("untracked array parameter"))?;
                    match *LLVMGetIndices(inst) {
                        0 => {
                            values.insert(inst as usize, len.clone());
                        }
                        1 => {
                            self.ptrs.insert(
                                inst as usize,
                                PointerProvenance::ArrayParameter {
                                    parameter,
                                    offset: BV::from_u64(self.ctx, 0, 64),
                                },
                            );
                        }
                        _ => {
                            return Err(ValidationError(
                                "unsupported use of array parameter memory",
                            ));
                        }
                    }
                    inst = LLVMGetNextInstruction(inst);
                    continue;
                }
            }
            let value = if op == LLVMOpcode::LLVMExtractValue {
                // `check_function` admitted only a one-index read of an
                // admitted aggregate; the index selects that element's
                // leaf range. An aggregate result (extracting a nested
                // struct) registers its leaf slice and continues; an
                // i64 result yields its single leaf.
                let aggregate = LLVMGetOperand(inst, 0);
                let mut fields = self.aggregate(aggregate)?;
                let index = *LLVMGetIndices(inst);
                let (offset, span) = aggregate_leaf_range(LLVMTypeOf(aggregate), index)
                    .filter(|(offset, span)| offset + span <= fields.len())
                    .ok_or(ValidationError("unsupported LLVM aggregate value"))?;
                let slots: Vec<Option<BV>> = fields.drain(offset..offset + span).collect();
                // A 128-bit leaf reassembles its two ABI slots
                // (`lo`, `hi` — the order `param_env` binds them) into
                // a scalar `i128` (`pure-i64-composite/v21`).
                if LLVMGetTypeKind(LLVMTypeOf(inst)) == LLVMTypeKind::LLVMIntegerTypeKind
                    && LLVMGetIntTypeWidth(LLVMTypeOf(inst)) == 128
                {
                    if slots.len() != 2 {
                        return Err(ValidationError("unsupported LLVM aggregate value"));
                    }
                    let lo = slots[0]
                        .clone()
                        .ok_or(ValidationError("undefined LLVM aggregate slot"))?;
                    let hi = slots[1]
                        .clone()
                        .ok_or(ValidationError("undefined LLVM aggregate slot"))?;
                    values.insert(inst as usize, hi.concat(&lo));
                    inst = LLVMGetNextInstruction(inst);
                    continue;
                }
                if span > 1 {
                    self.aggregates.insert(inst as usize, slots);
                    inst = LLVMGetNextInstruction(inst);
                    continue;
                }
                let leaf = slots
                    .into_iter()
                    .next()
                    .flatten()
                    .ok_or(ValidationError("undefined LLVM aggregate slot"))?;
                // The `i1` flag leaf of a `{i64, i1}` `with.overflow`
                // pair is stored in 64 bits — truncate to the result's
                // declared width so `icmp`/`br` sees an i1.
                if width(LLVMTypeOf(inst))? == 1 && leaf.get_size() == 64 {
                    leaf.extract(0, 0)
                } else {
                    leaf
                }
            } else if op == LLVMOpcode::LLVMCall {
                // A scalar opt intrinsic yields its encoding directly;
                // any other call is an uninterpreted function of its
                // arguments (`call_decl` gives it the same declaration
                // the source encoder builds for `Expr::Call` on the
                // same callee name and arity).
                if let OptIntrinsic::Scalar(value) = self.opt_intrinsic(inst, &values, &guard)? {
                    values.insert(inst as usize, value);
                    inst = LLVMGetNextInstruction(inst);
                    continue;
                }
                match self.call_operands(inst, &values)? {
                    // Cooperative cancellation is an effect outside the
                    // pure profile: the claim covers executions where
                    // the poll never fires, so its result binds to 0. A
                    // defined function that merely shares the name stays
                    // a normal call site — its body decides the result.
                    None => BV::from_i64(self.ctx, 0, 64),
                    Some((name, args)) => {
                        let refs: Vec<&dyn z3::ast::Ast> =
                            args.iter().map(|v| v as &dyn z3::ast::Ast).collect();
                        if width(LLVMTypeOf(inst))? == 128 {
                            // A 128-bit call result is two field slots —
                            // the same uninterpreted applications the
                            // source encoder makes for a wide result
                            // (`pure-i64-composite/v21`).
                            let slots = (0..2)
                                .map(|i| {
                                    call_field_decl(self.ctx, &name, args.len(), i)
                                        .apply(&refs)
                                        .as_bv()
                                        .ok_or(ValidationError("unsupported LLVM call result"))
                                })
                                .collect::<Result<Vec<_>>>()?;
                            aggregate_value(&slots)
                        } else {
                            mumei_core::semantic_contract::call_decl(self.ctx, &name, args.len())
                                .apply(&refs)
                                .as_bv()
                                .ok_or(ValidationError("unsupported LLVM call result"))?
                        }
                    }
                }
            } else {
                let a = self.value(LLVMGetOperand(inst, 0), &values)?;
                match op {
                    LLVMOpcode::LLVMZExt => {
                        let dest = width(LLVMTypeOf(inst))?;
                        // `i1 → i64` (comparisons and bools) and
                        // `i64 → i128` (`u128` widening,
                        // `pure-i64-composite/v21`).
                        if !matches!((a.get_size(), dest), (1, 64) | (64, 128)) {
                            return Err(ValidationError("unsupported LLVM cast"));
                        }
                        a.zero_ext(dest - a.get_size())
                    }
                    LLVMOpcode::LLVMSExt => {
                        let dest = width(LLVMTypeOf(inst))?;
                        // `i64 → i128` (`i128` widening — a `-1` literal
                        // stays `-1`).
                        if (a.get_size(), dest) != (64, 128) {
                            return Err(ValidationError("unsupported LLVM cast"));
                        }
                        a.sign_ext(64)
                    }
                    // `freeze` pins a poison operand to an arbitrary
                    // defined value — rescuing the operand's poison
                    // obligations only when every poisoned use flows
                    // here.
                    LLVMOpcode::LLVMFreeze => self.rescue_frozen(inst, LLVMGetOperand(inst, 0), a),
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
                            LLVMOpcode::LLVMUDiv | LLVMOpcode::LLVMURem => {
                                // `udiv`/`urem` by a zero divisor is
                                // immediate UB, not poison — `freeze`
                                // cannot rescue it.
                                let zero = BV::from_i64(self.ctx, 0, b.get_size());
                                self.definedness.push((
                                    guard.clone(),
                                    b._eq(&zero).not(),
                                    inst as usize,
                                    false,
                                ));
                                if op == LLVMOpcode::LLVMUDiv {
                                    a.bvudiv(&b)
                                } else {
                                    a.bvurem(&b)
                                }
                            }
                            LLVMOpcode::LLVMSDiv | LLVMOpcode::LLVMSRem => {
                                // `sdiv`/`srem` are immediate UB for a
                                // zero divisor and for i64::MIN / -1 —
                                // `freeze` cannot rescue the obligation; the
                                // source side records the same partial
                                // domain.
                                let zero = BV::from_i64(self.ctx, 0, b.get_size());
                                // `iN::MIN` = the single sign bit set,
                                // at whichever width the operands carry
                                // (`i64` or `i128` under composite v21).
                                let size = b.get_size();
                                let min = BV::from_u64(self.ctx, 1, size).bvshl(&BV::from_u64(
                                    self.ctx,
                                    (size - 1) as u64,
                                    size,
                                ));
                                let minus_one = BV::from_i64(self.ctx, -1, size);
                                let defined = Bool::and(
                                    self.ctx,
                                    &[
                                        &b._eq(&zero).not(),
                                        &Bool::and(self.ctx, &[&a._eq(&min), &b._eq(&minus_one)])
                                            .not(),
                                    ],
                                );
                                self.definedness.push((
                                    guard.clone(),
                                    defined,
                                    inst as usize,
                                    false,
                                ));
                                if op == LLVMOpcode::LLVMSDiv {
                                    a.bvsdiv(&b)
                                } else {
                                    a.bvsrem(&b)
                                }
                            }
                            LLVMOpcode::LLVMShl | LLVMOpcode::LLVMAShr | LLVMOpcode::LLVMLShr => {
                                // `shl`/`ashr`/`lshr` are poison for amounts
                                // at or over the operand's bit width
                                // (unsigned comparison covers negatives).
                                let size = b.get_size();
                                let max = BV::from_u64(self.ctx, size as u64, size);
                                self.definedness.push((
                                    guard.clone(),
                                    b.bvult(&max),
                                    inst as usize,
                                    true,
                                ));
                                if op == LLVMOpcode::LLVMShl {
                                    a.bvshl(&b)
                                } else if op == LLVMOpcode::LLVMAShr {
                                    a.bvashr(&b)
                                } else {
                                    a.bvlshr(&b)
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
                            // `f64` arithmetic (`pure-i64-composite/v19`):
                            // the leaf slots hold IEEE 754 binary64 bit
                            // patterns — decode, compute under RNE and
                            // re-encode through the same helper the
                            // source side uses.
                            LLVMOpcode::LLVMFAdd => mumei_core::semantic_contract::f64_arith(
                                self.ctx,
                                &a,
                                &b,
                                mumei_core::semantic_contract::F64Arith::Add,
                            ),
                            LLVMOpcode::LLVMFSub => mumei_core::semantic_contract::f64_arith(
                                self.ctx,
                                &a,
                                &b,
                                mumei_core::semantic_contract::F64Arith::Sub,
                            ),
                            LLVMOpcode::LLVMFMul => mumei_core::semantic_contract::f64_arith(
                                self.ctx,
                                &a,
                                &b,
                                mumei_core::semantic_contract::F64Arith::Mul,
                            ),
                            LLVMOpcode::LLVMFDiv => mumei_core::semantic_contract::f64_arith(
                                self.ctx,
                                &a,
                                &b,
                                mumei_core::semantic_contract::F64Arith::Div,
                            ),
                            // `fcmp oeq` — the sole float predicate the
                            // shape check admits — is `fp.eq` on the
                            // decoded operands, materialized to i1.
                            LLVMOpcode::LLVMFCmp => {
                                let eq = mumei_core::semantic_contract::f64_eq(self.ctx, &a, &b);
                                eq.ite(&BV::from_u64(self.ctx, 1, 1), &BV::from_u64(self.ctx, 0, 1))
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
