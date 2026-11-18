//! Opt-in equivalence checking of one C03 body and one supplied LLVM 17 SSA snapshot.
//! Not postcondition verification, pipeline attestation or final-binary verification.

use inkwell::{context::Context as LlvmContext, memory_buffer::MemoryBuffer, values::AsValueRef};
use llvm_sys::{core::*, prelude::*, LLVMIntPredicate, LLVMOpcode, LLVMTypeKind};
use mumei_core::semantic_contract::{
    aggregate_value, call_field_decl, encode_pure_i64_body, encode_pure_i64_module,
    module_source_profile, PureI64Body, RefinedAdmission, LOOP_BOUND, PURE_I64_WRAPPING_PROFILE,
};
use std::collections::{BTreeMap, HashMap};
use z3::{
    ast::{Ast, Bool, BV},
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
    let (outcome, ir_definedness) = unsafe {
        let raw = function.as_value_ref();
        if native {
            check_native_module(module.as_mut_ptr(), raw)?;
        } else {
            check_module(module.as_mut_ptr(), model.bounded_memory)?;
        }
        compare_function(&context, raw, &model, &native_call_renames(native))?
    };
    let loops = model.loop_bound_exits.is_some();
    Ok(ValidationReport {
        profile: match (loops, model.bounded_memory, native) {
            (false, false, false) => PROFILE,
            (true, false, false) => BOUNDED_LOOPS_PROFILE,
            (false, true, false) => BOUNDED_MEMORY_PROFILE,
            (true, true, false) => BOUNDED_MEMORY_LOOPS_PROFILE,
            (false, false, true) => NATIVE_PROFILE,
            (true, false, true) => BOUNDED_LOOPS_NATIVE_PROFILE,
            (false, true, true) => BOUNDED_MEMORY_NATIVE_PROFILE,
            (true, true, true) => BOUNDED_MEMORY_LOOPS_NATIVE_PROFILE,
        },
        source_profile: PURE_I64_WRAPPING_PROFILE,
        target,
        data_layout: DATA_LAYOUT,
        checked_stage: if native {
            "native_user_body_and_truncating_wrapper_snapshot_no_pipeline_attestation"
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
/// The solver budget applies per function.
pub fn validate_pure_i64_module(
    source: &str,
    llvm_ir: &str,
    options: Options,
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
    unsafe { check_module(module.as_mut_ptr(), any_memory)? };
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
            compare_function(&context, function.as_value_ref(), model, &BTreeMap::new())?
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
        profile: match (any_loops, any_memory) {
            (false, false) => PROFILE,
            (true, false) => BOUNDED_LOOPS_PROFILE,
            (false, true) => BOUNDED_MEMORY_PROFILE,
            (true, true) => BOUNDED_MEMORY_LOOPS_PROFILE,
        },
        source_profile: PURE_I64_WRAPPING_PROFILE,
        target: TARGET,
        data_layout: DATA_LAYOUT,
        checked_stage: "supplied_llvm_ir_snapshot_no_pipeline_attestation",
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
/// admission level. The solver budget applies per function.
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
            if !model.inputs.is_empty() {
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
    unsafe {
        check_native_module_multi(module.as_mut_ptr(), user_main.as_value_ref(), any_memory)?
    };
    let call_renames = native_call_renames(true);
    let mut functions = Vec::new();
    let mut any_definedness = false;
    let mut any_param_domain = false;
    let mut any_subrange = false;
    let mut any_loops = false;
    let mut any_refined = false;
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
        let (member_outcome, ir_definedness) =
            unsafe { compare_function(&context, function.as_value_ref(), model, &call_renames)? };
        any_definedness |= !model.definedness.is_empty() || !ir_definedness.is_empty();
        any_param_domain |= !model.param_domain.is_empty();
        any_subrange |= model.subrange_params;
        any_loops |= model.loop_bound_exits.is_some();
        any_refined |= model.refined_signature;
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
        input_domain: loop_domain_label(
            if any_refined {
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
            },
            any_loops,
        ),
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
        [("__mumei_user_main".to_owned(), "main".to_owned())]
            .into_iter()
            .collect()
    } else {
        BTreeMap::new()
    }
}

unsafe fn compare_function<'ctx>(
    context: &'ctx Context,
    raw: LLVMValueRef,
    model: &PureI64Body<'ctx>,
    call_renames: &BTreeMap<String, String>,
) -> Result<(Outcome, Vec<(Bool<'ctx>, Bool<'ctx>)>)> {
    // A source `while` relaxes the CFG gate only for that atom's member:
    // cycles elsewhere still reject as unsupported.
    let loops = model.loop_bound_exits.is_some();
    // A struct parameter is one aggregate formal on the IR side and its
    // flattened field slots on the source side, so the IR arity is the
    // declared parameter count, not the slot count.
    check_function(
        raw,
        model.param_shapes.len(),
        model.result_shape,
        loops,
        model.bounded_memory,
    )?;
    let mut encoder = IrEncoder::new(context, call_renames, loops);
    let returns = encoder.encode(raw, &model.inputs, &model.param_shapes)?;
    let ir_definedness = encoder.definedness;
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
        solver.assert(&Bool::or(context, &mismatches.iter().collect::<Vec<_>>()));
        match solver.check() {
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
        solver.reset();
        solver.assert(&Bool::or(context, &ub.iter().collect::<Vec<_>>()));
        match solver.check() {
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
        n @ (1 | 64) => Ok(n),
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
        leaves = leaves.checked_add(match aggregate_fields(element) {
            Some(inner) => inner,
            None if LLVMGetTypeKind(element) == LLVMTypeKind::LLVMIntegerTypeKind
                && LLVMGetIntTypeWidth(element) == 64 =>
            {
                1
            }
            // A `double` element is one pass-through `f64` leaf slot
            // (`pure-i64-composite/v11`).
            None if LLVMGetTypeKind(element) == LLVMTypeKind::LLVMDoubleTypeKind => 1,
            None => return None,
        })?;
        // Leaf-first flattening bounds the shape the same way the
        // fragment's flattened signatures bound a composite param: at
        // most 16 leaf slots.
        if leaves > 16 {
            return None;
        }
    }
    Some(leaves)
}

/// The leaf range struct element `index` of an admitted aggregate
/// spans: `aggregate_fields` flattens `{inner: {a, b}, c}` to
/// `[a, b, c]`, so element 0 covers leaves 0..2. A non-admitted type
/// or an out-of-range index fails closed.
unsafe fn aggregate_leaf_range(ty: LLVMTypeRef, index: u32) -> Option<(usize, usize)> {
    if aggregate_fields(ty).is_none() || index >= LLVMCountStructElementTypes(ty) {
        return None;
    }
    let mut offset = 0usize;
    for i in 0..=index {
        let span = aggregate_fields(LLVMStructGetTypeAtIndex(ty, i)).unwrap_or(1) as usize;
        if i == index {
            return Some((offset, span));
        }
        offset += span;
    }
    None
}

/// A constant of an admitted aggregate type whose slots the encoder can
/// read: whole `undef`/`poison` (every slot undefined),
/// `zeroinitializer`, or a literal struct of integer and `undef`
/// elements.
unsafe fn constant_aggregate(value: LLVMValueRef) -> bool {
    if LLVMIsConstant(value) == 0 || aggregate_fields(LLVMTypeOf(value)).is_none() {
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
                || constant_aggregate(element)
        })
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

unsafe fn check_module(module: LLVMModuleRef, bounded_memory: bool) -> Result<()> {
    check_module_environment(module)?;
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
        // The cancellation-poll declaration is a call target, not an
        // obligation: every use is constrained by `check_call`.
        if LLVMIsDeclaration(member) != 0
            && callee_name(member).as_deref() == Some(CANCEL_POLL_INTRINSIC)
        {
            member = LLVMGetNextFunction(member);
            continue;
        }
        check_function(
            member,
            LLVMCountParams(member) as usize,
            member_result_shape(member),
            true,
            bounded_memory,
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
const CANCEL_POLL_INTRINSIC: &str = "__mumei_task_group_should_cancel_current";

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
unsafe fn check_call(call: LLVMValueRef) -> Result<()> {
    let callee = LLVMGetCalledValue(call);
    // `num_operands == args + 1` excludes operand bundles, which the
    // LLVM 17 C API cannot enumerate (the native wrapper prints instead
    // because its callee has no arguments at all).
    if callee.is_null()
        || LLVMGetValueKind(callee) != llvm_sys::LLVMValueKind::LLVMFunctionValueKind
        || LLVMGetCalledFunctionType(call) != LLVMGlobalGetValueType(callee)
        || (LLVMIsDeclaration(callee) != 0
            && callee_name(callee).as_deref() != Some(CANCEL_POLL_INTRINSIC))
        || LLVMGetNumOperands(call) != LLVMGetNumArgOperands(call) as i32 + 1
        || LLVMGetInstructionCallConv(call) != 0
        || LLVMIsTailCall(call) != 0
        || LLVMGetCallSiteAttributeCount(call, u32::MAX) != 0
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
    if (aggregate_fields(ret).is_none() && width(ret)? != 64)
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
        if aggregate_fields(param).is_none() && width(param)? != 64 {
            return Err(ValidationError("unsupported LLVM call signature"));
        }
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

/// A member's own result shape for the module pre-screen: an admitted
/// aggregate return is checked as that many slots, anything else as
/// the scalar i64/i1 return.
unsafe fn member_result_shape(member: LLVMValueRef) -> Option<usize> {
    aggregate_fields(LLVMGetReturnType(LLVMGlobalGetValueType(member))).map(|n| n as usize)
}

/// `result_shape` is the source result layout: `None` for a scalar
/// result (an i64/i1 return), `Some(n)` for a struct result, whose
/// return must be an admitted aggregate of exactly `n` slots.
unsafe fn check_function(
    function: LLVMValueRef,
    params: usize,
    result_shape: Option<usize>,
    allow_cycles: bool,
    bounded_memory: bool,
) -> Result<()> {
    check_function_header(function, params)?;
    let ret = LLVMGetReturnType(LLVMGlobalGetValueType(function));
    match result_shape {
        None => {
            if width(ret)? != 64 {
                return Err(ValidationError("unsupported LLVM signature"));
            }
        }
        Some(slots) => {
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
        if !allow_cycles {
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
                if !is_callee
                    && LLVMGetTypeKind(LLVMTypeOf(value)) == LLVMTypeKind::LLVMPointerTypeKind
                {
                    let pointer_ok = bounded_memory
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
                        || constant_aggregate(value);
                    if !use_ok || !source_ok {
                        return Err(ValidationError("unsupported LLVM aggregate value"));
                    }
                    continue;
                }
                if LLVMIsUndef(value) != 0 || LLVMIsPoison(value) != 0 {
                    return Err(ValidationError("undef or poison unsupported"));
                }
                if !is_callee && LLVMValueIsBasicBlock(value) == 0 {
                    width(LLVMTypeOf(value))?;
                    if LLVMIsConstant(value) != 0 && LLVMIsAConstantInt(value).is_null() {
                        return Err(ValidationError("unsupported LLVM constant"));
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
                    if !bounded_memory
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
                    if !bounded_memory
                        || !(2..=3).contains(&operands)
                        || !outer_zero
                        || !cell_indexed
                    {
                        return Err(ValidationError("unsupported LLVM getelementptr"));
                    }
                }
                // Loads read an i64 cell — a pointer-typed load would
                // escape provenance tracking and is rejected by the
                // `width` check the same way an aggregate load is.
                LLVMOpcode::LLVMLoad => {
                    if !bounded_memory || width(LLVMTypeOf(inst))? != 64 {
                        return Err(ValidationError("unsupported LLVM load"));
                    }
                }
                // Stores write an i64 cell — storing a pointer would let
                // an array hold an untracked address.
                LLVMOpcode::LLVMStore => {
                    if !bounded_memory || width(LLVMTypeOf(LLVMGetOperand(inst, 0)))? != 64 {
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
                    if width(LLVMTypeOf(inst))? != 64 {
                        return Err(ValidationError("non-i64 division or shift"));
                    }
                }
                LLVMOpcode::LLVMCall => {
                    check_call(inst)?;
                }
                // A one-index read of an admitted aggregate — the
                // result is an i64 leaf or an admitted aggregate
                // (extracting a nested struct field).
                LLVMOpcode::LLVMExtractValue => {
                    if LLVMGetNumIndices(inst) != 1 {
                        return Err(ValidationError("unsupported LLVM aggregate value"));
                    }
                    let result = LLVMTypeOf(inst);
                    if aggregate_fields(result).is_none() && width(result)? != 64 {
                        return Err(ValidationError("unsupported LLVM aggregate value"));
                    }
                }
                // One field of an admitted aggregate set from an i64
                // leaf or an admitted aggregate (writing a nested
                // struct field whole).
                LLVMOpcode::LLVMInsertValue => {
                    let inserted = LLVMTypeOf(LLVMGetOperand(inst, 1));
                    let leaf = width(inserted).map(|w| w == 64).unwrap_or(false);
                    if LLVMGetNumIndices(inst) != 1
                        || aggregate_fields(LLVMTypeOf(inst)).is_none()
                        || (!leaf && aggregate_fields(inserted).is_none())
                    {
                        return Err(ValidationError("unsupported LLVM aggregate value"));
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
                    if aggregate_fields(LLVMTypeOf(inst)).is_none() {
                        width(LLVMTypeOf(inst))?;
                    }
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
    check_native_wrapper(main, user_main)
}

/// Multi-atom variant of `check_native_module`: the C `main` wrapper
/// keeps its fixed call/trunc/ret shape against `__mumei_user_main`,
/// and every other member is screened by the shared function profile
/// against its own arity — each is a potential call target.
unsafe fn check_native_module_multi(
    module: LLVMModuleRef,
    user_main: LLVMValueRef,
    bounded_memory: bool,
) -> Result<()> {
    check_module_environment(module)?;
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
                && callee_name(member).as_deref() == Some(CANCEL_POLL_INTRINSIC))
        {
            check_function(
                member,
                LLVMCountParams(member) as usize,
                member_result_shape(member),
                true,
                bounded_memory,
            )?;
        }
        member = LLVMGetNextFunction(member);
    }
    check_native_wrapper(main, user_main)
}

/// The C `main` wrapper's fixed shape: `call @__mumei_user_main()`,
/// `trunc i64 to i32`, `ret` — nothing else.
unsafe fn check_native_wrapper(main: LLVMValueRef, user_main: LLVMValueRef) -> Result<()> {
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

struct IrEncoder<'ctx, 'renames> {
    ctx: &'ctx Context,
    call_renames: &'renames BTreeMap<String, String>,
    states: usize,
    /// `(path condition, defined)` obligations for `sdiv`/`srem`/`shl`/`ashr`:
    /// the instruction's operands must satisfy the condition wherever the
    /// path executes it, or the IR produces poison there.
    definedness: Vec<(Bool<'ctx>, Bool<'ctx>)>,
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
    /// Pointer provenance: `alloca` and `getelementptr` results map to
    /// their backing `alloca` plus an element offset (the whitelist
    /// keeps every index i64, so offsets stay element counts).
    ptrs: HashMap<usize, (usize, BV<'ctx>)>,
}
impl<'ctx, 'renames> IrEncoder<'ctx, 'renames> {
    fn new(
        ctx: &'ctx Context,
        call_renames: &'renames BTreeMap<String, String>,
        loops: bool,
    ) -> Self {
        Self {
            ctx,
            call_renames,
            states: 0,
            definedness: Vec::new(),
            loops,
            depths: HashMap::new(),
            aggregates: HashMap::new(),
            arrays: HashMap::new(),
            alloca_lens: HashMap::new(),
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
        let zero = BV::from_u64(self.ctx, 0, 64);
        let limit = BV::from_u64(self.ctx, slots, 64);
        self.definedness.push((
            guard.clone(),
            Bool::and(self.ctx, &[&offset.bvsge(&zero), &offset.bvslt(&limit)]),
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
        if constant_aggregate(value) {
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
        let leaf_count = |v: LLVMValueRef| aggregate_fields(LLVMTypeOf(v)).unwrap_or(1) as usize;
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
            } else if LLVMIsUndef(element) != 0 || LLVMIsPoison(element) != 0 {
                out.push(None);
            } else {
                out.push(Some(BV::from_u64(
                    self.ctx,
                    LLVMConstIntGetZExtValue(element),
                    64,
                )));
            }
        }
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
                args.push(self.value(arg, values)?);
            }
        }
        Ok(Some((name, args)))
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
    ) -> Result<Vec<(Bool<'ctx>, BV<'ctx>)>> {
        let mut values = HashMap::new();
        let mut slots = inputs.iter();
        for (i, shape) in shapes.iter().enumerate() {
            let param = LLVMGetParam(function, i as u32);
            match shape {
                // A parameter's LLVM width must match the source signature:
                // i64 for every parameter; a declared `bool` carries 0/1 and
                // is further bounded by `model.param_domain`.
                None => {
                    let input = slots
                        .next()
                        .ok_or(ValidationError("LLVM parameter shape mismatch"))?;
                    if width(LLVMTypeOf(param))? != input.get_size() {
                        return Err(ValidationError("LLVM parameter width mismatch"));
                    }
                    values.insert(param as usize, input.clone());
                }
                // A struct parameter's aggregate formal must have exactly
                // its field count; element i is the i-th flattened slot.
                Some(fields) => {
                    if aggregate_fields(LLVMTypeOf(param)) != Some(*fields as u32) {
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
                    // An aggregate operand (writing a nested struct
                    // field whole) splices its leaf slots in.
                    let inserted = self.aggregate(LLVMGetOperand(inst, 1))?;
                    if inserted.len() != span {
                        return Err(ValidationError("unsupported LLVM aggregate value"));
                    }
                    slots.splice(offset..offset + span, inserted);
                } else {
                    let field = self.value(LLVMGetOperand(inst, 1), &values)?;
                    slots[offset] = Some(field);
                }
                self.aggregates.insert(inst as usize, slots);
                inst = LLVMGetNextInstruction(inst);
                continue;
            }
            if op == LLVMOpcode::LLVMCall {
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
                    .push((guard.clone(), Bool::from_bool(self.ctx, false)));
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
                    (inst as usize, BV::from_u64(self.ctx, 0, 64)),
                );
                inst = LLVMGetNextInstruction(inst);
                continue;
            }
            if op == LLVMOpcode::LLVMGetElementPtr {
                let (alloca, base) = self
                    .ptrs
                    .get(&(LLVMGetOperand(inst, 0) as usize))
                    .cloned()
                    .ok_or(ValidationError("untracked LLVM pointer"))?;
                // Element offsets: the whitelist binds any leading
                // constant 0 of the two-index array form, so summing
                // every index is the addressed cell.
                let mut offset = base;
                for index in 1..LLVMGetNumOperands(inst) {
                    offset =
                        offset.bvadd(&self.value(LLVMGetOperand(inst, index as u32), &values)?);
                }
                self.ptrs.insert(inst as usize, (alloca, offset));
                inst = LLVMGetNextInstruction(inst);
                continue;
            }
            if op == LLVMOpcode::LLVMStore {
                let cell = self.value(LLVMGetOperand(inst, 0), &values)?;
                let (alloca, offset) = self
                    .ptrs
                    .get(&(LLVMGetOperand(inst, 1) as usize))
                    .cloned()
                    .ok_or(ValidationError("untracked LLVM pointer"))?;
                self.memory_in_bounds(alloca, &offset, &guard)?;
                let array = self
                    .arrays
                    .get(&alloca)
                    .cloned()
                    .ok_or(ValidationError("untracked LLVM pointer"))?;
                self.arrays.insert(alloca, array.store(&offset, &cell));
                inst = LLVMGetNextInstruction(inst);
                continue;
            }
            if op == LLVMOpcode::LLVMLoad {
                let (alloca, offset) = self
                    .ptrs
                    .get(&(LLVMGetOperand(inst, 0) as usize))
                    .cloned()
                    .ok_or(ValidationError("untracked LLVM pointer"))?;
                self.memory_in_bounds(alloca, &offset, &guard)?;
                let array = self
                    .arrays
                    .get(&alloca)
                    .cloned()
                    .ok_or(ValidationError("untracked LLVM pointer"))?;
                values.insert(
                    inst as usize,
                    array
                        .select(&offset)
                        .as_bv()
                        .ok_or(ValidationError("unsupported LLVM load"))?,
                );
                inst = LLVMGetNextInstruction(inst);
                continue;
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
                if span > 1 {
                    self.aggregates.insert(inst as usize, slots);
                    inst = LLVMGetNextInstruction(inst);
                    continue;
                }
                slots
                    .into_iter()
                    .next()
                    .flatten()
                    .ok_or(ValidationError("undefined LLVM aggregate slot"))?
            } else if op == LLVMOpcode::LLVMCall {
                // Model the call as an uninterpreted function of its
                // arguments; `call_decl` gives it the same declaration
                // the source encoder builds for `Expr::Call` on the
                // same callee name and arity.
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
                        mumei_core::semantic_contract::call_decl(self.ctx, &name, args.len())
                            .apply(&refs)
                            .as_bv()
                            .ok_or(ValidationError("unsupported LLVM call result"))?
                    }
                }
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
                            LLVMOpcode::LLVMUDiv | LLVMOpcode::LLVMURem => {
                                // `udiv`/`urem` are poison only for a zero
                                // divisor — there is no unsigned analogue
                                // of the `i64::MIN / -1` case.
                                let zero = BV::from_i64(self.ctx, 0, 64);
                                self.definedness.push((guard.clone(), b._eq(&zero).not()));
                                if op == LLVMOpcode::LLVMUDiv {
                                    a.bvudiv(&b)
                                } else {
                                    a.bvurem(&b)
                                }
                            }
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
                            LLVMOpcode::LLVMShl | LLVMOpcode::LLVMAShr | LLVMOpcode::LLVMLShr => {
                                // `shl`/`ashr`/`lshr` are poison for amounts >= 64
                                // (unsigned comparison covers negatives).
                                let max = BV::from_i64(self.ctx, 64, 64);
                                self.definedness.push((guard.clone(), b.bvult(&max)));
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
