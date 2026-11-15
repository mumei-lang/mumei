//! Opt-in equivalence checking of one C03 body and one supplied LLVM 17 SSA snapshot.
//! Not postcondition verification, pipeline attestation or final-binary verification.

use inkwell::{context::Context as LlvmContext, memory_buffer::MemoryBuffer, values::AsValueRef};
use llvm_sys::{core::*, prelude::*, LLVMIntPredicate, LLVMOpcode, LLVMTypeKind};
use mumei_core::semantic_contract::{
    encode_pure_i64_body, encode_pure_i64_module, PureI64Body, LOOP_BOUND,
    PURE_I64_WRAPPING_PROFILE,
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
            check_module(module.as_mut_ptr())?;
        }
        compare_function(&context, raw, &model, &native_call_renames(native))?
    };
    let loops = model.loop_bound_exits.is_some();
    Ok(ValidationReport {
        profile: if native {
            if loops {
                BOUNDED_LOOPS_NATIVE_PROFILE
            } else {
                NATIVE_PROFILE
            }
        } else if loops {
            BOUNDED_LOOPS_PROFILE
        } else {
            PROFILE
        },
        source_profile: PURE_I64_WRAPPING_PROFILE,
        target,
        data_layout: DATA_LAYOUT,
        checked_stage: if native {
            "native_user_body_and_truncating_wrapper_snapshot_no_pipeline_attestation"
        } else {
            "supplied_llvm_ir_snapshot_no_pipeline_attestation"
        },
        input_domain: loop_domain_label(input_domain_label(native, &model, &ir_definedness), loops),
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
    let models = encode_pure_i64_module(source, &context, true)
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
    unsafe { check_module(module.as_mut_ptr())? };
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
        profile: if any_loops {
            BOUNDED_LOOPS_PROFILE
        } else {
            PROFILE
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
/// The solver budget applies per function.
pub fn validate_native_module(
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
    let models = encode_pure_i64_module(source, &context, true)
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
    unsafe { check_native_module_multi(module.as_mut_ptr(), user_main.as_value_ref())? };
    let call_renames = native_call_renames(true);
    let mut functions = Vec::new();
    let mut any_definedness = false;
    let mut any_param_domain = false;
    let mut any_subrange = false;
    let mut any_loops = false;
    let mut outcome = Outcome::EquivalentWithinProfile;
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
        profile: if any_loops {
            BOUNDED_LOOPS_NATIVE_MODULE_PROFILE
        } else {
            NATIVE_MODULE_PROFILE
        },
        source_profile: PURE_I64_WRAPPING_PROFILE,
        target,
        data_layout: DATA_LAYOUT,
        checked_stage:
            "native_module_user_bodies_and_truncating_wrapper_snapshot_no_pipeline_attestation",
        input_domain: loop_domain_label(
            if any_definedness {
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
    check_function(raw, model.inputs.len(), loops)?;
    let mut encoder = IrEncoder::new(context, call_renames, loops);
    let returns = encoder.encode(raw, &model.inputs)?;
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
        .cloned()
        .collect();
    let src_dom = Bool::and(context, &src_terms.iter().collect::<Vec<_>>());
    let ir_terms: Vec<Bool> = ir_definedness
        .iter()
        .map(|(path, defined)| path.implies(defined))
        .collect();
    let ir_dom = Bool::and(context, &ir_terms.iter().collect::<Vec<_>>());
    let mut mismatches: Vec<_> = returns
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
    if loops {
        // A covered input the IR never returns on — every path out of
        // it was dropped for exceeding the bound — diverges where the
        // source converges: a difference, not an out-of-profile input.
        let covered = Bool::and(context, &[&src_dom, &ir_dom, &exits]);
        let guards: Vec<_> = returns.iter().map(|(guard, _)| guard).collect();
        let any_return = Bool::or(context, &guards);
        mismatches.push(Bool::and(context, &[&covered, &any_return.not()]));
    }
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
            SatResult::Unsat => Outcome::EquivalentWithinProfile,
        }
    };
    Ok((outcome, ir_definedness))
}

/// Append the bounded-loop domain restriction to a base input-domain
/// label so the report names the actual claim boundary.
fn loop_domain_label(domain: &'static str, loops: bool) -> String {
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
) -> &'static str {
    if !model.definedness.is_empty() || !ir_definedness.is_empty() {
        if native {
            "zero_argument_main_i64_body_where_division_and_shift_defined_not_process_exit_status"
        } else if model.param_domain.is_empty() {
            "signed_i64_inputs_where_division_and_shift_defined_not_restricted_by_requires"
        } else if model.subrange_params {
            "signed_i64_inputs_with_signature_bounded_param_domains_where_division_and_shift_defined_not_restricted_by_requires"
        } else {
            "signed_i64_inputs_with_bool_params_restricted_to_0_or_1_where_division_and_shift_defined_not_restricted_by_requires"
        }
    } else if native {
        "zero_argument_main_i64_body_not_process_exit_status"
    } else if model.param_domain.is_empty() {
        "all_signed_i64_inputs_not_restricted_by_requires"
    } else if model.subrange_params {
        "signed_i64_inputs_with_signature_bounded_param_domains_not_restricted_by_requires"
    } else {
        "signed_i64_inputs_with_bool_params_restricted_to_0_or_1_not_restricted_by_requires"
    }
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

unsafe fn check_module(module: LLVMModuleRef) -> Result<()> {
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
        check_function(member, LLVMCountParams(member) as usize, true)?;
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
    if width(LLVMGetReturnType(signature))? != 64
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
    for param in params {
        if width(param)? != 64 {
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

unsafe fn check_function(function: LLVMValueRef, params: usize, allow_cycles: bool) -> Result<()> {
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
                LLVMOpcode::LLVMCall => {
                    check_call(inst)?;
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
    check_native_wrapper(main, user_main)
}

/// Multi-atom variant of `check_native_module`: the C `main` wrapper
/// keeps its fixed call/trunc/ret shape against `__mumei_user_main`,
/// and every other member is screened by the shared function profile
/// against its own arity — each is a potential call target.
unsafe fn check_native_module_multi(module: LLVMModuleRef, user_main: LLVMValueRef) -> Result<()> {
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
            check_function(member, LLVMCountParams(member) as usize, true)?;
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
            // A parameter's LLVM width must match the source signature:
            // i64 for every parameter; a declared `bool` carries 0/1 and is
            // further bounded by `model.param_domain`.
            if width(LLVMTypeOf(param))? != input.get_size() {
                return Err(ValidationError("LLVM parameter width mismatch"));
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
        let mut inst = LLVMGetFirstInstruction(block);
        while !inst.is_null() {
            let op = LLVMGetInstructionOpcode(inst);
            if op == LLVMOpcode::LLVMRet {
                let value = self.value(LLVMGetOperand(inst, 0), &values)?;
                returns.push((guard, value));
                return Ok(());
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
            let value = if op == LLVMOpcode::LLVMCall {
                // Model the call as an uninterpreted function of its
                // arguments; `call_decl` gives it the same declaration
                // the source encoder builds for `Expr::Call` on the
                // same callee name and arity.
                let callee = LLVMGetCalledValue(inst);
                let mut name_len = 0;
                let raw_name = LLVMGetValueName2(callee, &mut name_len);
                if raw_name.is_null() {
                    return Err(ValidationError("unsupported LLVM call"));
                }
                let name = String::from_utf8_lossy(std::slice::from_raw_parts(
                    raw_name as *const u8,
                    name_len,
                ))
                .into_owned();
                if name == CANCEL_POLL_INTRINSIC {
                    // Cooperative cancellation is an effect outside the
                    // pure profile: the claim covers executions where
                    // the poll never fires, so its result binds to 0.
                    BV::from_i64(self.ctx, 0, 64)
                } else {
                    // An emitted member name maps back to the source atom
                    // name its call sites share with the source encoder
                    // (`__mumei_user_main` back to `main` on native paths).
                    let name = self
                        .call_renames
                        .get(name.as_str())
                        .map(String::as_str)
                        .unwrap_or(name.as_str());
                    let arity = LLVMGetNumArgOperands(inst) as usize;
                    let mut args = Vec::with_capacity(arity);
                    for index in 0..arity {
                        args.push(self.value(LLVMGetOperand(inst, index as u32), &values)?);
                    }
                    let refs: Vec<&dyn z3::ast::Ast> =
                        args.iter().map(|v| v as &dyn z3::ast::Ast).collect();
                    let applied = mumei_core::semantic_contract::call_decl(self.ctx, name, arity)
                        .apply(&refs);
                    applied
                        .as_bv()
                        .ok_or(ValidationError("unsupported LLVM call result"))?
                }
            } else if op == LLVMOpcode::LLVMPHI {
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
