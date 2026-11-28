use crate::{
    cli::{NativeBundleCommand, ObjectDescriptorOutputArgs},
    codegen::MUMEI_RUNTIME_C,
    linker,
};
use mumei_core::semantic_contract::RefinedAdmission;
use mumei_core::{
    hir, native_bundle::*, parser, producer_signature, proof_cert, resolver, semantic_contract,
    verification,
};
use mumei_emit_llvm::{
    machine_tv::{self, FunctionOutcome},
    translation_validation::{self as tv, Options, Outcome},
};
use object::{
    Architecture, BinaryFormat, Object, ObjectSection, ObjectSymbol, RelocationFlags,
    RelocationTarget, SectionKind, SymbolKind,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};

mod evidence;
mod object_acceptance;

enum LlvmRecheck {
    Rechecked(Box<evidence::NativeConsumerEvidence>),
    /// Module bundles carry the per-atom consumer evidence format; the
    /// payload is the translation profile plus that report.
    ModuleRechecked(&'static str, Box<evidence::NativeModuleConsumerEvidence>),
    /// Multifile bundles carry the module-shaped envelope plus a merge
    /// correspondence record; the payload is the recheck profile plus
    /// that report.
    MultifileRechecked(&'static str, Box<evidence::NativeMultifileConsumerEvidence>),
    Different,
    Inconclusive,
}

const MAX_COMPONENT_BYTES: u64 = 32 * 1024 * 1024;
/// The per-source byte bound the bounded recheck profile declares:
/// consumer commands re-read `--source` (and each pinned multifile
/// member) under this cap, not the loose component cap.
const SOURCE_BYTES: u64 = 65_536;
/// Bounded like the record, not the single-atom evidence size: a module
/// report carries `1 + 2·atoms` records.
const MAX_EVIDENCE_BYTES: u64 = 1024 * 1024;
const LLVM_RECHECK_SCOPE: &str =
    "source_postcondition_identity_and_llvm_snapshot_only/no_authenticity_or_binary_semantic_proof";
const ARTIFACT_RECHECK_SCOPE: &str =
    "source_postcondition_llvm_snapshot_and_toolchain_identity_only/no_execution_or_runtime_semantics";
const OBJECT_SYMBOL_CORRESPONDENCE_SCOPE: &str =
    "artifact_reproduction_object_symbols_call_edges_and_link_placement_only/no_instruction_abi_or_runtime_semantics";
const MACHINE_CODE_TV_SCOPE: &str =
    "artifact_reproduction_object_link_correspondence_and_acyclic_branch_and_conditional_call_function_machine_code_tv_only/no_abi_or_runtime_semantics";
const MACHINE_CODE_TV_WITHOUT_OBJECT_LINK_SCOPE: &str =
    "artifact_reproduction_object_symbol_and_call_edge_correspondence_and_machine_code_tv_only/no_object_link_correspondence_abi_or_runtime_semantics";
const MACHINE_CODE_REGISTER_CONVENTION: &str =
    "sysv_x86_64_integer_args_rdi_rsi_rdx_rcx_r8_r9_result_rax";
const MACHINE_CODE_F64_REGISTER_CONVENTION: &str =
    "sysv_x86_64_integer_args_rdi_rsi_rdx_rcx_r8_r9_f64_args_xmm0_7_result_rax_or_xmm0";
const MACHINE_CODE_REGISTER_CONVENTION_WITH_WIDE_SCALARS: &str =
    "sysv_x86_64_integer_args_rdi_rsi_rdx_rcx_r8_r9_128_bit_scalars_in_consecutive_gpr_pairs_low_first_result_rax_rdx";
const MACHINE_CODE_F64_REGISTER_CONVENTION_WITH_WIDE_SCALARS: &str =
    "sysv_x86_64_integer_args_rdi_rsi_rdx_rcx_r8_r9_f64_args_xmm0_7_result_rax_or_xmm0_128_bit_scalars_in_consecutive_gpr_pairs_low_first_result_rax_rdx";
const MACHINE_CODE_REGISTER_CONVENTION_WITH_INTEGER_AGGREGATES: &str =
    "sysv_x86_64_integer_args_rdi_rsi_rdx_rcx_r8_r9_result_rax_two_eightbyte_integer_aggregates_in_consecutive_gprs_result_rax_rdx";
const MACHINE_CODE_F64_REGISTER_CONVENTION_WITH_INTEGER_AGGREGATES: &str =
    "sysv_x86_64_integer_args_rdi_rsi_rdx_rcx_r8_r9_f64_args_xmm0_7_result_rax_or_xmm0_two_eightbyte_integer_aggregates_in_consecutive_gprs_result_rax_rdx";
const MACHINE_CODE_F64_REGISTER_CONVENTION_WITH_SSE_AGGREGATES: &str =
    "sysv_x86_64_integer_args_rdi_rsi_rdx_rcx_r8_r9_f64_args_xmm0_7_result_rax_or_xmm0_two_eightbyte_aggregates_classified_integer_gprs_sse_xmm_result_rax_rdx_xmm0_xmm1";
const MACHINE_CODE_REGISTER_CONVENTION_WITH_LLVM_AGGREGATES: &str =
    "sysv_x86_64_integer_args_rdi_rsi_rdx_rcx_r8_r9_result_rax_llvm_first_class_aggregates_split_into_registers_result_rax_rdx_rcx_xmm0_xmm1";
const MACHINE_CODE_F64_REGISTER_CONVENTION_WITH_LLVM_AGGREGATES: &str =
    "sysv_x86_64_integer_args_rdi_rsi_rdx_rcx_r8_r9_f64_args_xmm0_7_result_rax_or_xmm0_llvm_first_class_aggregates_split_into_registers_result_rax_rdx_rcx_xmm0_xmm1";
const MACHINE_CODE_TV_INPUT_DOMAIN: &str =
    "signed_i64_inputs_with_signature_bounded_param_domains_where_division_and_shift_defined_not_restricted_by_requires";
const MACHINE_CODE_TV_REFINED_INPUT_DOMAIN: &str =
    "module_members_signed_i64_inputs_with_refined_scalar_signature_predicate_domains_not_process_exit_status";
const EXIT_STATUS_RECHECK_SCOPE: &str =
    "artifact_reproduction_and_os_exit_status_observation_only/no_runtime_abi_or_semantic_proof";
const STDOUT_RECHECK_SCOPE: &str =
    "artifact_reproduction_and_os_stdout_observation_only/no_runtime_abi_or_semantic_proof";
const STDERR_RECHECK_SCOPE: &str =
    "artifact_reproduction_and_os_stderr_observation_only/no_runtime_abi_or_semantic_proof";
const RUNTIME_OBSERVATION_RECHECK_SCOPE: &str =
    "artifact_reproduction_and_runtime_observation_semantics_on_one_invocation_only/no_runtime_abi_or_semantic_proof";

#[cfg(test)]
fn machine_tv_register_convention_assumed(
    equivalent_f64_leaf_count: usize,
    equivalent_composite_aggregate_count: usize,
    equivalent_f64_composite_aggregate_count: usize,
    equivalent_wide_composite_aggregate_count: usize,
) -> &'static str {
    machine_tv_register_convention_assumed_with_wide_scalars(
        equivalent_f64_leaf_count,
        equivalent_composite_aggregate_count,
        equivalent_f64_composite_aggregate_count,
        equivalent_wide_composite_aggregate_count,
        0,
    )
}

fn machine_tv_register_convention_assumed_with_wide_scalars(
    equivalent_f64_leaf_count: usize,
    equivalent_composite_aggregate_count: usize,
    equivalent_f64_composite_aggregate_count: usize,
    equivalent_wide_composite_aggregate_count: usize,
    equivalent_wide_scalar_count: usize,
) -> &'static str {
    let has_f64_leaves =
        equivalent_f64_leaf_count > 0 || equivalent_f64_composite_aggregate_count > 0;
    if equivalent_wide_scalar_count > 0 {
        return if has_f64_leaves {
            MACHINE_CODE_F64_REGISTER_CONVENTION_WITH_WIDE_SCALARS
        } else {
            MACHINE_CODE_REGISTER_CONVENTION_WITH_WIDE_SCALARS
        };
    }
    if equivalent_wide_composite_aggregate_count > 0 {
        return if has_f64_leaves {
            MACHINE_CODE_F64_REGISTER_CONVENTION_WITH_LLVM_AGGREGATES
        } else {
            MACHINE_CODE_REGISTER_CONVENTION_WITH_LLVM_AGGREGATES
        };
    }
    match (
        has_f64_leaves,
        equivalent_composite_aggregate_count > 0,
        equivalent_f64_composite_aggregate_count > 0,
    ) {
        (true, true, true) => MACHINE_CODE_F64_REGISTER_CONVENTION_WITH_SSE_AGGREGATES,
        (true, true, false) => MACHINE_CODE_F64_REGISTER_CONVENTION_WITH_INTEGER_AGGREGATES,
        (true, false, _) => MACHINE_CODE_F64_REGISTER_CONVENTION,
        (false, true, true) => MACHINE_CODE_F64_REGISTER_CONVENTION_WITH_SSE_AGGREGATES,
        (false, true, false) => MACHINE_CODE_REGISTER_CONVENTION_WITH_INTEGER_AGGREGATES,
        (false, false, _) => MACHINE_CODE_REGISTER_CONVENTION,
    }
}

fn machine_tv_register_convention_assumed_with_frame_memory(
    equivalent_f64_leaf_count: usize,
    equivalent_composite_aggregate_count: usize,
    equivalent_f64_composite_aggregate_count: usize,
    equivalent_wide_composite_aggregate_count: usize,
    equivalent_wide_scalar_count: usize,
    equivalent_red_zone_frame_memory_count: usize,
    equivalent_allocated_frame_memory_count: usize,
) -> String {
    let mut convention = machine_tv_register_convention_assumed_with_wide_scalars(
        equivalent_f64_leaf_count,
        equivalent_composite_aggregate_count,
        equivalent_f64_composite_aggregate_count,
        equivalent_wide_composite_aggregate_count,
        equivalent_wide_scalar_count,
    )
    .to_owned();
    if equivalent_red_zone_frame_memory_count > 0 {
        convention.push_str("_leaf_red_zone_frame_memory_assumed_unclobbered");
    }
    if equivalent_allocated_frame_memory_count > 0 {
        convention.push_str("_allocated_frame_memory_assumed_callee_isolated");
    }
    convention
}

fn machine_tv_call_shapes_have_composite_aggregate(
    argument_shapes: &[semantic_contract::CallValueShape],
    result_shape: semantic_contract::CallValueShape,
) -> bool {
    argument_shapes.iter().any(|shape| {
        matches!(
            shape,
            semantic_contract::CallValueShape::Struct { .. }
                | semantic_contract::CallValueShape::Enum { .. }
        )
    }) || matches!(
        result_shape,
        semantic_contract::CallValueShape::Struct { .. }
            | semantic_contract::CallValueShape::Enum { .. }
    )
}

fn machine_tv_model_uses_composite_aggregate(model: &semantic_contract::PureI64Body<'_>) -> bool {
    model.param_shapes.iter().any(Option::is_some)
        || model.result_shape.is_some()
        || model.call_sites.iter().any(|call_site| {
            machine_tv_call_shapes_have_composite_aggregate(
                &call_site.argument_shapes,
                call_site.result_shape,
            )
        })
}

/// Whether any aggregate in the model's signatures or call sites exceeds
/// the two-leaf shape the narrow convention strings describe — the basis for
/// the `llvm_first_class_aggregates` convention name regardless of whether
/// the shape is modeled or assumed.
fn machine_tv_model_uses_wide_composite_aggregate_convention(
    model: &semantic_contract::PureI64Body<'_>,
) -> bool {
    model
        .param_shapes
        .iter()
        .flatten()
        .any(|leaf_count| *leaf_count > 2)
        || model.result_shape.is_some_and(|leaf_count| leaf_count > 2)
        || model.call_sites.iter().any(|call_site| {
            call_site.argument_shapes.iter().any(|shape| {
                matches!(
                    shape,
                    semantic_contract::CallValueShape::Struct { leaf_count, .. }
                        | semantic_contract::CallValueShape::Enum { leaf_count }
                        if *leaf_count > 2
                )
            }) || matches!(
                call_site.result_shape,
                semantic_contract::CallValueShape::Struct { leaf_count, .. }
                    | semantic_contract::CallValueShape::Enum { leaf_count }
                    if leaf_count > 2
            )
        })
}

fn machine_tv_model_uses_wide_composite_aggregate(
    model: &semantic_contract::PureI64Body<'_>,
) -> bool {
    model
        .param_shapes
        .iter()
        .flatten()
        .any(|leaf_count| *leaf_count > 2)
        || model.result_shape.is_some_and(|leaf_count| {
            leaf_count > 2
                // An aggregate result the lifter resolves — within the
                // per-class register budgets, or back through the
                // `sret` convention past the INTEGER budget — is
                // validated, not an assumed memory-class ABI.
                && !(model.result_leaf_doubles.len() == leaf_count
                    && (aggregate_result_leaf_doubles_modeled(
                        &model.result_leaf_doubles,
                    ) || ((4..=16).contains(&leaf_count)
                        && integer_result_leaf_count(&model.result_leaf_doubles) > 3)))
        })
        || model.call_sites.iter().any(|call_site| {
            call_site.argument_shapes.iter().any(|shape| {
                matches!(
                    shape,
                    semantic_contract::CallValueShape::Struct { leaf_count, .. }
                        | semantic_contract::CallValueShape::Enum { leaf_count }
                        if *leaf_count > 2
                )
            }) || matches!(
                call_site.result_shape,
                semantic_contract::CallValueShape::Struct {
                    leaf_count,
                    has_enum: false,
                    ..
                } if leaf_count > 2
                    // A call result the lifter resolves —
                    // register-resident within the per-class budgets,
                    // or back through the caller's `sret` buffer past
                    // the INTEGER budget — is modeled, not an assumed
                    // memory-class ABI.
                    && !(call_site.result_leaf_doubles.len() == leaf_count
                        && (aggregate_result_leaf_doubles_modeled(
                            &call_site.result_leaf_doubles,
                        ) || ((4..=16).contains(&leaf_count)
                            && integer_result_leaf_count(&call_site.result_leaf_doubles)
                                > 3)))
            ) || matches!(
                call_site.result_shape,
                semantic_contract::CallValueShape::Enum { leaf_count } if leaf_count > 2
                    // Enum call results follow the same per-class
                    // budgets and `sret` overflow as structs.
                    && !(call_site.result_leaf_doubles.len() == leaf_count
                        && (aggregate_result_leaf_doubles_modeled(
                            &call_site.result_leaf_doubles,
                        ) || ((4..=16).contains(&leaf_count)
                            && integer_result_leaf_count(&call_site.result_leaf_doubles)
                                > 3)))
            )
        })
}

fn integer_result_leaf_count(leaf_doubles: &[bool]) -> usize {
    leaf_doubles.iter().filter(|is_double| !**is_double).count()
}

fn aggregate_result_leaf_doubles_modeled(leaf_doubles: &[bool]) -> bool {
    !leaf_doubles.is_empty()
        && integer_result_leaf_count(leaf_doubles) <= 3
        && leaf_doubles.iter().filter(|is_double| **is_double).count() <= 2
}

fn machine_tv_model_uses_wide_scalar(model: &semantic_contract::PureI64Body<'_>) -> bool {
    model
        .param_shapes
        .iter()
        .zip(&model.param_leaf_doubles)
        .any(|(shape, leaves)| shape.is_none() && leaves.len() == 2)
        || (model.result_shape.is_none() && model.value.get_size() == 128)
        || model.call_sites.iter().any(|call_site| {
            call_site.argument_shapes.iter().any(|shape| {
                matches!(
                    shape,
                    semantic_contract::CallValueShape::Scalar { leaf_count: 2 }
                )
            }) || matches!(
                call_site.result_shape,
                semantic_contract::CallValueShape::Scalar { leaf_count: 2 }
            )
        })
}

fn machine_tv_model_uses_f64_composite_aggregate(
    model: &semantic_contract::PureI64Body<'_>,
) -> bool {
    model
        .param_shapes
        .iter()
        .zip(&model.param_leaf_doubles)
        .any(|(shape, leaf_doubles)| {
            shape.is_some() && leaf_doubles.iter().any(|is_double| *is_double)
        })
        || (model.result_shape.is_some()
            && model.result_leaf_doubles.iter().any(|is_double| *is_double))
        || model.call_sites.iter().any(|call_site| {
            let mut leaf_offset = 0usize;
            call_site.argument_shapes.iter().any(|shape| {
                let start = leaf_offset;
                leaf_offset = leaf_offset.saturating_add(shape.leaf_count());
                matches!(
                    shape,
                    semantic_contract::CallValueShape::Struct { has_f64: true, .. }
                ) || (matches!(shape, semantic_contract::CallValueShape::Enum { .. })
                    && call_site
                        .arg_leaf_doubles
                        .get(start..leaf_offset)
                        .is_some_and(|leaves| leaves.iter().any(|is_double| *is_double)))
            }) || matches!(
                call_site.result_shape,
                semantic_contract::CallValueShape::Struct { has_f64: true, .. }
            ) || (matches!(
                call_site.result_shape,
                semantic_contract::CallValueShape::Enum { .. }
            ) && call_site
                .result_leaf_doubles
                .iter()
                .any(|is_double| *is_double))
        })
}
const OBSERVATION_ENVIRONMENT: &str = "empty";
const OBSERVATION_WORKING_DIRECTORY: &str = "executable_directory";
/// The build/recheck single-run cap for observing an executable: a
/// bounded pure-i64 `main` is expected to return promptly, but the
/// observation must never hang a command.
const OBSERVATION_TIMEOUT: Duration = Duration::from_secs(30);
/// A channel observation's capture cap: a deterministic bounded program's
/// output fits well inside it, and an output cap means the observation
/// never stores or hashes an unbounded channel — overflow yields no pin.
const OBSERVED_CHANNEL_MAX_BYTES: usize = 65_536;
const EMBEDDED_RUNTIME_EXPORTS: &[&str] = &[
    "__mumei_chan_init",
    "__mumei_chan_recv",
    "__mumei_chan_send",
    "__mumei_chan_send_owned",
    "__mumei_effect_stub",
    "__mumei_get_resource_mutex",
    "__mumei_payload_box_alloc_failed",
    "__mumei_task_cancel",
    "__mumei_task_group_any_flag",
    "__mumei_task_group_any_wait",
    "__mumei_task_group_cancel",
    "__mumei_task_group_complete",
    "__mumei_task_group_enter",
    "__mumei_task_group_leave",
    "__mumei_task_group_reset",
    "__mumei_task_group_set_completed",
    "__mumei_task_group_should_cancel",
    "__mumei_task_group_should_cancel_current",
    "mumei_task_group_any_is_done",
    "mumei_task_group_any_signal",
    "mumei_task_group_any_wait",
];

/// Declare the inputs of the one invocation: an empty environment and the
/// executable's directory as the working directory.
fn observation_invocation_envelope() -> serde_json::Value {
    serde_json::json!({
        "argv": [],
        "stdin": "closed",
        "timeout_ms": OBSERVATION_TIMEOUT.as_millis(),
        "channel_cap_bytes": OBSERVED_CHANNEL_MAX_BYTES,
        "environment": OBSERVATION_ENVIRONMENT,
        "working_directory": OBSERVATION_WORKING_DIRECTORY,
    })
}

fn pinned_observation_envelope() -> ObservationEnvelope {
    ObservationEnvelope {
        environment: OBSERVATION_ENVIRONMENT.to_owned(),
        working_directory: OBSERVATION_WORKING_DIRECTORY.to_owned(),
    }
}

/// Record the running consumer binary's compile-time target as an observation
/// of where the invocation ran, never a claim. No host facts are pinned or
/// compared; libc, loader, kernel and CPU-feature details are not recorded.
fn observation_host() -> serde_json::Value {
    serde_json::json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "family": std::env::consts::FAMILY,
    })
}

/// Observe the effective solver and native toolchain linked or selected by this binary.
/// The observation records versions and target identities only; it does not attest the host,
/// artifact semantics, or producer authenticity and never enters the bundle manifest/build ID.
fn native_toolchain_observation_with_link(link: linker::LinkToolchain) -> serde_json::Value {
    let z3 = verification::linked_z3_version();
    let (llvm_version, target_triple) = mumei_emit_llvm::codegen::linked_llvm_toolchain();
    serde_json::json!({
        "schema_version": "mumei.native-toolchain-observation/v1",
        "scope": "effective_toolchain_identity_observation_only/no_environment_or_artifact_semantic_attestation",
        "host": observation_host(),
        "target_triple": target_triple,
        "solvers": {
            "z3": {
                "binding": "linked",
                "version": format!("{}.{}.{}.{}", z3.0, z3.1, z3.2, z3.3),
            }
        },
        "llvm": {
            "binding": "linked",
            "version": llvm_version,
        },
        "link": {
            "driver": link.driver,
            "driver_version": link.driver_version,
            "driver_target": link.driver_target,
            "linker": link.linker,
            "linker_version": link.linker_version,
        }
    })
}

fn native_toolchain_observation() -> Result<serde_json::Value, BundleError> {
    linker::observe_link_toolchain()
        .map(native_toolchain_observation_with_link)
        .map_err(|_| BundleError("native toolchain observation failed"))
}

fn write_native_toolchain_observation(json: bool) -> Result<(), BundleError> {
    let observation = native_toolchain_observation()?;
    if json {
        let mut bytes = serde_json::to_vec_pretty(&observation)
            .map_err(|_| BundleError("cannot encode native toolchain observation"))?;
        bytes.push(b'\n');
        std::io::stdout()
            .lock()
            .write_all(&bytes)
            .map_err(|_| BundleError("cannot write native toolchain observation"))
    } else {
        println!(
            "Native toolchain observation: z3 {} linked; LLVM {} linked for {}; link driver {} {} targeting {}; linker {} {}.\nScope: {}",
            observation["solvers"]["z3"]["version"].as_str().unwrap_or_default(),
            observation["llvm"]["version"].as_str().unwrap_or_default(),
            observation["target_triple"].as_str().unwrap_or_default(),
            observation["link"]["driver"].as_str().unwrap_or_default(),
            observation["link"]["driver_version"].as_str().unwrap_or_default(),
            observation["link"]["driver_target"].as_str().unwrap_or_default(),
            observation["link"]["linker"].as_str().unwrap_or_default(),
            observation["link"]["linker_version"].as_str().unwrap_or_default(),
            observation["scope"].as_str().unwrap_or_default(),
        );
        Ok(())
    }
}

/// Resolve `executable` to an absolute path when possible, then spawn it under
/// the pinned envelope: empty environment, cwd = its directory, no arguments.
fn observation_command(executable: &Path) -> std::process::Command {
    let resolved_executable =
        std::path::absolute(executable).unwrap_or_else(|_| executable.to_path_buf());
    let mut command = std::process::Command::new(&resolved_executable);
    command.env_clear();
    if let Some(dir) = resolved_executable
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
    {
        command.current_dir(dir);
    }
    command
}

/// Which standard output channel an observation drains and digests.
#[derive(Clone, Copy)]
enum ObservedChannel {
    Stdout,
    Stderr,
}

/// Run `executable` once with no arguments, an empty environment, cwd set to
/// its directory and closed stdio, returning its 0..=255 exit code. `None`
/// on spawn failure, timeout or signal termination — never a claim about
/// program semantics, only an OS exit-status observation.
fn observed_exit_status(executable: &Path) -> Option<u32> {
    let mut child = observation_command(executable)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + OBSERVATION_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.code().map(|code| code as u32),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Err(_) => return None,
        }
    }
}

/// Run `executable` once with no arguments, an empty environment, cwd set to
/// its directory, closed stdin and the other channel discarded, draining
/// `channel` into a buffer capped at
/// `OBSERVED_CHANNEL_MAX_BYTES` (the reader keeps draining past the cap
/// so the child never blocks on a full pipe), returning the sha256 hex
/// of the captured bytes. `None` on spawn failure, timeout, signal
/// termination or output over the cap — never a claim about program
/// semantics, only an OS-level channel observation.
/// Why a channel observation produced no digest — every failure is
/// fail-closed `None` to callers, but the reason is kept internally for
/// test diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObservedStreamFailure {
    SpawnFailed,
    PipeTaken,
    TimedOutOrKilled,
    ReaderFailed,
    OutputOverCap,
}

fn observed_stream(executable: &Path, channel: ObservedChannel) -> Option<String> {
    observed_stream_inner(executable, channel).ok()
}

fn observed_stream_inner(
    executable: &Path,
    channel: ObservedChannel,
) -> Result<String, ObservedStreamFailure> {
    let mut command = observation_command(executable);
    command.stdin(Stdio::null());
    match channel {
        ObservedChannel::Stdout => {
            command.stdout(Stdio::piped()).stderr(Stdio::null());
        }
        ObservedChannel::Stderr => {
            command.stdout(Stdio::null()).stderr(Stdio::piped());
        }
    }
    let mut child = command
        .spawn()
        .map_err(|_| ObservedStreamFailure::SpawnFailed)?;
    // ChildStdout/ChildStderr are distinct types; erase to the shared
    // Read interface for the draining reader thread.
    let piped: Box<dyn std::io::Read + Send> = match channel {
        ObservedChannel::Stdout => Box::new(
            child
                .stdout
                .take()
                .ok_or(ObservedStreamFailure::PipeTaken)?,
        ),
        ObservedChannel::Stderr => Box::new(
            child
                .stderr
                .take()
                .ok_or(ObservedStreamFailure::PipeTaken)?,
        ),
    };
    let reader = capped_reader(piped);
    let deadline = Instant::now() + OBSERVATION_TIMEOUT;
    let finished = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code().is_some(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
            Err(_) => {
                // Kill before joining the reader — a live child holding
                // the pipe open would leave join() blocked indefinitely.
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let (captured, overflow) = reader
        .join()
        .map_err(|_| ObservedStreamFailure::ReaderFailed)?;
    if !finished {
        return Err(ObservedStreamFailure::TimedOutOrKilled);
    }
    if overflow {
        return Err(ObservedStreamFailure::OutputOverCap);
    }
    Ok(sha256(&captured))
}

fn capped_reader<R>(mut piped: R) -> std::thread::JoinHandle<(Vec<u8>, bool)>
where
    R: std::io::Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut captured = Vec::new();
        let mut chunk = [0u8; 8192];
        let mut overflow = false;
        loop {
            match piped.read(&mut chunk) {
                Ok(0) => break,
                Err(_) => {
                    // A read error mid-stream means the captured bytes
                    // are truncated — fail closed, never pin a partial
                    // digest.
                    overflow = true;
                    break;
                }
                Ok(n) => {
                    let room = OBSERVED_CHANNEL_MAX_BYTES.saturating_sub(captured.len());
                    if n > room {
                        // Any byte past the cap makes the channel
                        // unobservable — the pin must cover the whole
                        // stream, never a truncation.
                        overflow = true;
                    }
                    captured.extend_from_slice(&chunk[..n.min(room)]);
                }
            }
        }
        (captured, overflow)
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InvocationObservation {
    exit_status: u32,
    stdout_sha256: String,
    stderr_sha256: String,
}

enum RuntimeObservationCheck {
    Matching(InvocationObservation),
    Different,
    Inconclusive(&'static str),
}

/// Run `executable` once with no arguments, an empty environment, cwd set to
/// its directory and closed stdin, observing both output channels.
fn observed_invocation(executable: &Path) -> Result<InvocationObservation, ObservedStreamFailure> {
    let mut child = observation_command(executable)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| ObservedStreamFailure::SpawnFailed)?;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(ObservedStreamFailure::PipeTaken);
    };
    let Some(stderr) = child.stderr.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(ObservedStreamFailure::PipeTaken);
    };
    let stdout_reader = capped_reader(stdout);
    let stderr_reader = capped_reader(stderr);

    let deadline = Instant::now() + OBSERVATION_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) | Err(_) => {
                // Kill before joining the readers — a live child holding
                // the pipes open would leave join() blocked indefinitely.
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(ObservedStreamFailure::TimedOutOrKilled);
            }
        }
    };
    let Some(exit_status) = status.code() else {
        let _ = stdout_reader.join();
        let _ = stderr_reader.join();
        return Err(ObservedStreamFailure::TimedOutOrKilled);
    };

    let stdout_result = stdout_reader.join();
    let stderr_result = stderr_reader.join();
    let (stdout, stdout_overflow) =
        stdout_result.map_err(|_| ObservedStreamFailure::ReaderFailed)?;
    let (stderr, stderr_overflow) =
        stderr_result.map_err(|_| ObservedStreamFailure::ReaderFailed)?;
    if stdout_overflow || stderr_overflow {
        return Err(ObservedStreamFailure::OutputOverCap);
    }
    Ok(InvocationObservation {
        exit_status: exit_status as u32,
        stdout_sha256: sha256(&stdout),
        stderr_sha256: sha256(&stderr),
    })
}

fn check_runtime_observations(
    exit: Option<u32>,
    stdout: Option<String>,
    stderr: Option<String>,
    stage: Option<tempfile::TempDir>,
) -> RuntimeObservationCheck {
    let (Some(expected_exit), Some(expected_stdout), Some(expected_stderr)) =
        (exit, stdout, stderr)
    else {
        drop(stage);
        return RuntimeObservationCheck::Inconclusive(
            "Record does not pin every runtime observation (exit status, stdout and stderr); inconclusive.",
        );
    };
    let Some(stage) = stage else {
        return RuntimeObservationCheck::Inconclusive(
            "Reproduced executable is unavailable for execution; inconclusive.",
        );
    };
    let observation = observed_invocation(&stage.path().join(BundleFile::Executable.file_name()));
    drop(stage);
    let observation = match observation {
        Ok(observation) => observation,
        Err(_) => {
            return RuntimeObservationCheck::Inconclusive(
                "Executable runtime observation inconclusive; spawn failed, timed out, terminated by signal or exceeded the capture cap.",
            );
        }
    };
    if observation.exit_status != expected_exit
        || observation.stdout_sha256 != expected_stdout
        || observation.stderr_sha256 != expected_stderr
    {
        RuntimeObservationCheck::Different
    } else {
        RuntimeObservationCheck::Matching(observation)
    }
}

/// The stdout channel observation — see `observed_stream`.
fn observed_stdout(executable: &Path) -> Option<String> {
    observed_stream(executable, ObservedChannel::Stdout)
}

/// The stderr channel observation — see `observed_stream`.
fn observed_stderr(executable: &Path) -> Option<String> {
    observed_stream(executable, ObservedChannel::Stderr)
}

enum ArtifactRecheck {
    Rechecked,
    Different,
    Inconclusive,
}

/// The pinned bundle state the recheck chain verified, kept in memory so
/// later stages compare against exactly the snapshot that passed — no
/// component is re-read from the bundle directory afterwards.
struct LoadedBundle {
    object_build: ObjectBuild,
    link_build: LinkBuild,
    files: BTreeMap<BundleFile, Vec<u8>>,
    source_text: Option<String>,
    record_profile: Option<String>,
}

pub(crate) fn run(command: NativeBundleCommand) {
    let result = match command {
        NativeBundleCommand::AcceptEvidence { bundle, source, evidence, expected_build_id, json } => {
            // The descriptor names the checks the chain ran: the
            // multifile variant adds the pinned-member merge to
            // `checks_performed`. The saved envelope's own schema picks
            // the descriptor: on an accepted outcome it is exactly the
            // family the fresh recheck ran (the compare fails closed
            // otherwise), so it names the checks the chain actually ran
            // even if the record file changed mid-command.
            match read_regular(Path::new(&evidence), MAX_EVIDENCE_BYTES)
                .and_then(|saved| {
                    let acceptance = match evidence::decode(&saved)? {
                        evidence::ConsumerEvidence::Multifile(_) => {
                            &MULTIFILE_EVIDENCE_ACCEPTANCE
                        }
                        _ => &EVIDENCE_ACCEPTANCE,
                    };
                    recheck_evidence_bytes(Path::new(&bundle), Path::new(&source), &saved, &expected_build_id)
                        .map(|outcome| (acceptance, outcome, saved))
                })
                .and_then(|(acceptance, outcome, saved)| write_native_acceptance(outcome, json, acceptance, Some(&saved), None, &mut std::io::stdout().lock()))
            {
                Ok(Outcome::EquivalentWithinProfile) => Ok(()),
                Ok(Outcome::Different) => Err(BundleError("native LLVM body differs from source")),
                Ok(Outcome::UndefinedBehavior) => Err(BundleError(
                    "native LLVM body reaches undefined behavior on an input the source defines",
                )),
                Ok(Outcome::Inconclusive) => {
                    eprintln!("Native source/LLVM recheck inconclusive; evidence not accepted.");
                    std::process::exit(3);
                }
                Err(error) => Err(error),
            }
        }
        NativeBundleCommand::AcceptArtifact { bundle, source, expected_build_id, json } => {
            // The descriptor names the checks the chain ran: the
            // multifile variant adds the pinned-member merge to
            // `checks_performed`, and the canonical link argument-vector
            // reproduction is claimed only when the recheck actually
            // compared a pin — a pre-field record carries none. Both
            // facts come back from the recheck itself so the descriptor
            // cannot claim a check a mid-command record swap left unrun.
            let recheck = recheck_artifact(Path::new(&bundle), Path::new(&source), &expected_build_id);
            match recheck
                .map(|recheck| (
                    match recheck.result {
                        ArtifactRecheck::Rechecked => Outcome::EquivalentWithinProfile,
                        ArtifactRecheck::Different => Outcome::Different,
                        ArtifactRecheck::Inconclusive => Outcome::Inconclusive,
                    },
                    recheck.family,
                    recheck.canonical_argv_compared,
                ))
                .and_then(|(outcome, family, argv_compared)| write_native_acceptance(outcome, json, match (family, argv_compared) {
                    (RecordFamily::Multifile, true) => &MULTIFILE_ARTIFACT_ACCEPTANCE,
                    (RecordFamily::Multifile, false) => &MULTIFILE_ARTIFACT_ACCEPTANCE_ARGV_UNPINNED,
                    (RecordFamily::Single | RecordFamily::Module, true) => &ARTIFACT_ACCEPTANCE,
                    (RecordFamily::Single | RecordFamily::Module, false) => &ARTIFACT_ACCEPTANCE_ARGV_UNPINNED,
                }, None, None, &mut std::io::stdout().lock()))
            {
                Ok(Outcome::EquivalentWithinProfile) => Ok(()),
                Ok(Outcome::Inconclusive) => {
                    eprintln!("Native source/LLVM recheck inconclusive; artifact not accepted.");
                    std::process::exit(3);
                }
                Ok(_) => Err(BundleError(
                    "pinned artifact differs from the toolchain's output for the pinned inputs",
                )),
                Err(error) => Err(error),
            }
        }
        NativeBundleCommand::RecheckEvidence { bundle, source, evidence, expected_build_id } => {
            match recheck_evidence(Path::new(&bundle), Path::new(&source), Path::new(&evidence), &expected_build_id) {
                Ok(Outcome::EquivalentWithinProfile) => {
                    println!("Saved native Evidence consistent with fresh pinned source/LLVM rechecks.\nScope: {LLVM_RECHECK_SCOPE}\nEvidence acceptance: not evaluated.\nObject/linker/runtime/OS exit-status/executable semantics: not checked.");
                    Ok(())
                }
                Ok(Outcome::Different) => Err(BundleError("native LLVM body differs from source")),
                Ok(Outcome::UndefinedBehavior) => Err(BundleError(
                    "native LLVM body reaches undefined behavior on an input the source defines",
                )),
                Ok(Outcome::Inconclusive) => {
                    eprintln!("Native source/LLVM recheck inconclusive; no evidence consistency result established.");
                    std::process::exit(3);
                }
                Err(error) => Err(error),
            }
        },
        NativeBundleCommand::RecheckLlvm { bundle, source, expected_build_id, json } => {
            match recheck_llvm(Path::new(&bundle), Path::new(&source), &expected_build_id)
                .and_then(|result| write_llvm_recheck(result, json, &mut std::io::stdout().lock())) {
                Ok(Outcome::EquivalentWithinProfile) => Ok(()),
                Ok(Outcome::Different) => Err(BundleError("native LLVM body differs from source")),
                Ok(Outcome::UndefinedBehavior) => Err(BundleError(
                    "native LLVM body reaches undefined behavior on an input the source defines",
                )),
                Ok(Outcome::Inconclusive) => {
                    eprintln!("Native source/LLVM recheck inconclusive; no combined result established.");
                    std::process::exit(3);
                }
                Err(error) => Err(error),
            }
        },
        NativeBundleCommand::RecheckSource { bundle, source, expected_build_id } => {
            match recheck(Path::new(&bundle), Path::new(&source), &expected_build_id) {
                Ok((SourceRecheck::RecheckedWithinProfile, profile)) => {
                    println!("Pinned snapshot identities consistent; source postcondition freshly rechecked within profile.\nProfile: {profile}\nScope: {RECHECK_SCOPE}\nLLVM/object/linker/runtime/executable semantics: not checked.");
                    Ok(())
                }
                Ok((SourceRecheck::Refuted, _)) => Err(BundleError("source postcondition refuted")),
                Ok((SourceRecheck::Inconclusive, _)) => {
                    eprintln!("Native source recheck inconclusive; no source proof established.");
                    std::process::exit(3);
                }
                Err(error) => Err(error),
            }
        },
        NativeBundleCommand::Build { input, output } => build(Path::new(&input), Path::new(&output)).map(|(id, scope)| {
            println!("Native bundle built with fixed profile (project config/cache not loaded).\nBuild ID: {id}\nScope: {scope}");
        }),
        NativeBundleCommand::ObserveToolchain { json } => write_native_toolchain_observation(json),
        NativeBundleCommand::Check { bundle, source, expected_build_id } => check(Path::new(&bundle), Path::new(&source), &expected_build_id).map(|scope| {
            println!("Native bundle identity consistent with pinned build ID.\nScope: {scope}");
        }),
        NativeBundleCommand::RecheckArtifact { bundle, source, expected_build_id } => {
            match recheck_artifact(Path::new(&bundle), Path::new(&source), &expected_build_id)
                .map(|recheck| recheck.result)
            {
                Ok(ArtifactRecheck::Rechecked) => {
                    println!("Pinned toolchain outputs reproduce the pinned object and executable.\nScope: {ARTIFACT_RECHECK_SCOPE}");
                    Ok(())
                }
                Ok(ArtifactRecheck::Different) => Err(BundleError(
                    "pinned artifact differs from the toolchain's output for the pinned inputs",
                )),
                Ok(ArtifactRecheck::Inconclusive) => {
                    eprintln!("Native source/LLVM recheck inconclusive; no artifact result established.");
                    std::process::exit(3);
                }
                Err(error) => Err(error),
            }
        }
        NativeBundleCommand::RecheckObject {
            bundle,
            source,
            expected_build_id,
            json,
        } => match recheck_object_pipeline(
            Path::new(&bundle),
            Path::new(&source),
            &expected_build_id,
            false,
        ) {
            Err(error) => Err(error),
            Ok(ObjectRecheckPipeline::Different(message)) => Err(BundleError(message)),
            Ok(ObjectRecheckPipeline::Inconclusive(message)) => {
                eprintln!("{message}");
                std::process::exit(3);
            }
            Ok(ObjectRecheckPipeline::Rechecked(data)) => {
                write_object_recheck(*data, &expected_build_id, json)
            }
        },
        NativeBundleCommand::RecheckMachineTv {
            bundle,
            source,
            expected_build_id,
            json,
        } => match recheck_object_pipeline(
            Path::new(&bundle),
            Path::new(&source),
            &expected_build_id,
            true,
        ) {
            Err(error) => Err(error),
            Ok(ObjectRecheckPipeline::Different(message)) => Err(BundleError(message)),
            Ok(ObjectRecheckPipeline::Inconclusive(message)) => {
                eprintln!("{message}");
                std::process::exit(3);
            }
            Ok(ObjectRecheckPipeline::Rechecked(data)) => {
                write_machine_tv(*data, &expected_build_id, json)
            }
        },
        NativeBundleCommand::AcceptObject {
            bundle,
            source,
            expected_build_id,
            output,
        } => match evaluate_object_acceptance(
            Path::new(&bundle),
            Path::new(&source),
            &expected_build_id,
        ) {
            Err(error) => Err(error),
            Ok(ObjectAcceptance::Inconclusive(message)) => {
                report_object_acceptance_inconclusive(&message);
            }
            Ok(ObjectAcceptance::Accepted(accepted)) => {
                write_object_machine_tv_acceptance(&accepted, &expected_build_id, &output)
            }
        },
        NativeBundleCommand::RecheckObjectAcceptance {
            bundle,
            source,
            descriptor,
            expected_build_id,
            signature,
            json,
        } => recheck_object_acceptance(
            Path::new(&bundle),
            Path::new(&source),
            Path::new(&descriptor),
            &expected_build_id,
            &signature,
            json,
        ),
        NativeBundleCommand::RecheckExitStatus { bundle, source, expected_build_id } => {
            match recheck_artifact(Path::new(&bundle), Path::new(&source), &expected_build_id) {
                Err(error) => Err(error),
                Ok(recheck) => match recheck {
                ArtifactRecheckResult {
                    result: ArtifactRecheck::Rechecked,
                    expected_exit_status: Some(expected),
                    stage: Some(stage),
                    ..
                } => {
                    // Run the byte-identical reproduced executable the
                    // recheck just verified, never the bundle's own copy.
                    match observed_exit_status(
                        &stage.path().join(BundleFile::Executable.file_name()),
                    ) {
                        Some(actual) if actual == expected => {
                            println!("Pinned toolchain outputs reproduce the pinned object and executable; the reproduced executable exits with the pinned status.\nScope: {EXIT_STATUS_RECHECK_SCOPE}");
                            Ok(())
                        }
                        Some(_) => Err(BundleError(
                            "reproduced executable's exit status differs from the pinned status",
                        )),
                        None => {
                            eprintln!("Executable exit-status observation inconclusive; spawn failed, timed out or terminated by signal.");
                            // `exit` skips destructors — clean the staged
                            // reproduction up explicitly first.
                            drop(stage);
                            std::process::exit(3);
                        }
                    }
                }
                ArtifactRecheckResult {
                    result: ArtifactRecheck::Rechecked,
                    expected_exit_status: None,
                    stage,
                    ..
                } => {
                    eprintln!("Record pins no expected exit status; inconclusive.");
                    drop(stage);
                    std::process::exit(3);
                }
                ArtifactRecheckResult {
                    result: ArtifactRecheck::Rechecked,
                    stage: None,
                    ..
                } => {
                    eprintln!("Reproduced executable is unavailable for execution; inconclusive.");
                    std::process::exit(3);
                }
                ArtifactRecheckResult {
                    result: ArtifactRecheck::Different,
                    ..
                } => Err(BundleError(
                    "pinned artifact differs from the toolchain's output for the pinned inputs",
                )),
                ArtifactRecheckResult {
                    result: ArtifactRecheck::Inconclusive,
                    ..
                } => {
                    eprintln!("Native source/LLVM recheck inconclusive; no artifact result established.");
                    std::process::exit(3);
                }
                },
            }
        }
        NativeBundleCommand::AcceptExitStatus { bundle, source, expected_build_id, json } => {
            // The acceptance vocabulary for the exit-status slice: the
            // descriptor claims the OS-level observation only when the
            // comparison actually ran, and the family/argv split mirrors
            // accept-artifact so the claim names the checks this record's
            // recheck performed.
            match recheck_artifact(Path::new(&bundle), Path::new(&source), &expected_build_id) {
                Err(error) => Err(error),
                Ok(recheck) => {
                    let ArtifactRecheckResult {
                        result,
                        family,
                        canonical_argv_compared,
                        expected_exit_status,
                        pin_envelope,
                        stage,
                        ..
                    } = recheck;
                    match (result, expected_exit_status, stage) {
                        (
                            ArtifactRecheck::Rechecked,
                            Some(expected),
                            Some(stage),
                        ) => {
                            match observed_exit_status(
                                &stage.path().join(BundleFile::Executable.file_name()),
                            ) {
                                Some(actual) if actual == expected => {
                                    let acceptance = match (family, canonical_argv_compared) {
                                        (RecordFamily::Multifile, true) => {
                                            &MULTIFILE_EXIT_STATUS_ACCEPTANCE
                                        }
                                        (RecordFamily::Multifile, false) => {
                                            &MULTIFILE_EXIT_STATUS_ACCEPTANCE_ARGV_UNPINNED
                                        }
                                        (RecordFamily::Single | RecordFamily::Module, true) => {
                                            &EXIT_STATUS_ACCEPTANCE
                                        }
                                        (RecordFamily::Single | RecordFamily::Module, false) => {
                                            &EXIT_STATUS_ACCEPTANCE_ARGV_UNPINNED
                                        }
                                    };
                                    write_native_acceptance(
                                        Outcome::EquivalentWithinProfile,
                                        json,
                                        acceptance,
                                        None,
                                        Some(pin_envelope),
                                        &mut std::io::stdout().lock(),
                                    )
                                    .map(|_| ())
                                }
                                Some(_) => Err(BundleError(
                                    "reproduced executable's exit status differs from the pinned status",
                                )),
                                None => {
                                    eprintln!("Executable exit-status observation inconclusive; spawn failed, timed out or terminated by signal.");
                                    drop(stage);
                                    std::process::exit(3);
                                }
                            }
                        }
                        (ArtifactRecheck::Rechecked, _, stage) => {
                            eprintln!("Record pins no expected exit status or the reproduced executable is unavailable; inconclusive.");
                            drop(stage);
                            std::process::exit(3);
                        }
                        (ArtifactRecheck::Different, ..) => Err(BundleError(
                            "pinned artifact differs from the toolchain's output for the pinned inputs",
                        )),
                        (ArtifactRecheck::Inconclusive, ..) => {
                            eprintln!("Native source/LLVM recheck inconclusive; artifact not accepted.");
                            std::process::exit(3);
                        }
                    }
                }
            }
        }
        NativeBundleCommand::RecheckStdout { bundle, source, expected_build_id } => {
            match recheck_artifact(Path::new(&bundle), Path::new(&source), &expected_build_id) {
                Err(error) => Err(error),
                Ok(recheck) => match recheck {
                ArtifactRecheckResult {
                    result: ArtifactRecheck::Rechecked,
                    expected_stdout_sha256: Some(expected),
                    stage: Some(stage),
                    ..
                } => {
                    // Run the byte-identical reproduced executable the
                    // recheck just verified, never the bundle's own copy.
                    match observed_stdout(
                        &stage.path().join(BundleFile::Executable.file_name()),
                    ) {
                        Some(actual) if actual == expected => {
                            println!("Pinned toolchain outputs reproduce the pinned object and executable; the reproduced executable's stdout matches the pinned digest.\nScope: {STDOUT_RECHECK_SCOPE}");
                            Ok(())
                        }
                        Some(_) => Err(BundleError(
                            "reproduced executable's stdout differs from the pinned digest",
                        )),
                        None => {
                            eprintln!("Executable stdout observation inconclusive; spawn failed, timed out, terminated by signal or exceeded the capture cap.");
                            // `exit` skips destructors — clean the staged
                            // reproduction up explicitly first.
                            drop(stage);
                            std::process::exit(3);
                        }
                    }
                }
                ArtifactRecheckResult {
                    result: ArtifactRecheck::Rechecked,
                    expected_stdout_sha256: None,
                    stage,
                    ..
                } => {
                    eprintln!("Record pins no expected stdout digest; inconclusive.");
                    drop(stage);
                    std::process::exit(3);
                }
                ArtifactRecheckResult {
                    result: ArtifactRecheck::Rechecked,
                    stage: None,
                    ..
                } => {
                    eprintln!("Reproduced executable is unavailable for execution; inconclusive.");
                    std::process::exit(3);
                }
                ArtifactRecheckResult {
                    result: ArtifactRecheck::Different,
                    ..
                } => Err(BundleError(
                    "pinned artifact differs from the toolchain's output for the pinned inputs",
                )),
                ArtifactRecheckResult {
                    result: ArtifactRecheck::Inconclusive,
                    ..
                } => {
                    eprintln!("Native source/LLVM recheck inconclusive; no artifact result established.");
                    std::process::exit(3);
                }
                },
            }
        }
        NativeBundleCommand::RecheckStderr { bundle, source, expected_build_id } => {
            match recheck_artifact(Path::new(&bundle), Path::new(&source), &expected_build_id) {
                Err(error) => Err(error),
                Ok(recheck) => match recheck {
                ArtifactRecheckResult {
                    result: ArtifactRecheck::Rechecked,
                    expected_stderr_sha256: Some(expected),
                    stage: Some(stage),
                    ..
                } => {
                    // Run the byte-identical reproduced executable the
                    // recheck just verified, never the bundle's own copy.
                    match observed_stderr(
                        &stage.path().join(BundleFile::Executable.file_name()),
                    ) {
                        Some(actual) if actual == expected => {
                            println!("Pinned toolchain outputs reproduce the pinned object and executable; the reproduced executable's stderr matches the pinned digest.\nScope: {STDERR_RECHECK_SCOPE}");
                            Ok(())
                        }
                        Some(_) => Err(BundleError(
                            "reproduced executable's stderr differs from the pinned digest",
                        )),
                        None => {
                            eprintln!("Executable stderr observation inconclusive; spawn failed, timed out, terminated by signal or exceeded the capture cap.");
                            // `exit` skips destructors — clean the staged
                            // reproduction up explicitly first.
                            drop(stage);
                            std::process::exit(3);
                        }
                    }
                }
                ArtifactRecheckResult {
                    result: ArtifactRecheck::Rechecked,
                    expected_stderr_sha256: None,
                    stage,
                    ..
                } => {
                    eprintln!("Record pins no expected stderr digest; inconclusive.");
                    drop(stage);
                    std::process::exit(3);
                }
                ArtifactRecheckResult {
                    result: ArtifactRecheck::Rechecked,
                    stage: None,
                    ..
                } => {
                    eprintln!("Reproduced executable is unavailable for execution; inconclusive.");
                    std::process::exit(3);
                }
                ArtifactRecheckResult {
                    result: ArtifactRecheck::Different,
                    ..
                } => Err(BundleError(
                    "pinned artifact differs from the toolchain's output for the pinned inputs",
                )),
                ArtifactRecheckResult {
                    result: ArtifactRecheck::Inconclusive,
                    ..
                } => {
                    eprintln!("Native source/LLVM recheck inconclusive; no artifact result established.");
                    std::process::exit(3);
                }
                },
            }
        }
        NativeBundleCommand::AcceptStdout { bundle, source, expected_build_id, json } => {
            // The acceptance vocabulary for the stdout slice: the
            // descriptor claims the OS-level observation only when the
            // comparison actually ran, and the family/argv split mirrors
            // accept-exit-status so the claim names the checks this
            // record's recheck performed.
            match recheck_artifact(Path::new(&bundle), Path::new(&source), &expected_build_id) {
                Err(error) => Err(error),
                Ok(recheck) => {
                    let ArtifactRecheckResult {
                        result,
                        family,
                        canonical_argv_compared,
                        expected_stdout_sha256,
                        pin_envelope,
                        stage,
                        ..
                    } = recheck;
                    match (result, expected_stdout_sha256, stage) {
                        (
                            ArtifactRecheck::Rechecked,
                            Some(expected),
                            Some(stage),
                        ) => {
                            match observed_stdout(
                                &stage.path().join(BundleFile::Executable.file_name()),
                            ) {
                                Some(actual) if actual == expected => {
                                    let acceptance = match (family, canonical_argv_compared) {
                                        (RecordFamily::Multifile, true) => {
                                            &MULTIFILE_STDOUT_ACCEPTANCE
                                        }
                                        (RecordFamily::Multifile, false) => {
                                            &MULTIFILE_STDOUT_ACCEPTANCE_ARGV_UNPINNED
                                        }
                                        (RecordFamily::Single | RecordFamily::Module, true) => {
                                            &STDOUT_ACCEPTANCE
                                        }
                                        (RecordFamily::Single | RecordFamily::Module, false) => {
                                            &STDOUT_ACCEPTANCE_ARGV_UNPINNED
                                        }
                                    };
                                    write_native_acceptance(
                                        Outcome::EquivalentWithinProfile,
                                        json,
                                        acceptance,
                                        None,
                                        Some(pin_envelope),
                                        &mut std::io::stdout().lock(),
                                    )
                                    .map(|_| ())
                                }
                                Some(_) => Err(BundleError(
                                    "reproduced executable's stdout differs from the pinned digest",
                                )),
                                None => {
                                    eprintln!("Executable stdout observation inconclusive; spawn failed, timed out, terminated by signal or exceeded the capture cap.");
                                    drop(stage);
                                    std::process::exit(3);
                                }
                            }
                        }
                        (ArtifactRecheck::Rechecked, _, stage) => {
                            eprintln!("Record pins no expected stdout digest or the reproduced executable is unavailable; inconclusive.");
                            drop(stage);
                            std::process::exit(3);
                        }
                        (ArtifactRecheck::Different, ..) => Err(BundleError(
                            "pinned artifact differs from the toolchain's output for the pinned inputs",
                        )),
                        (ArtifactRecheck::Inconclusive, ..) => {
                            eprintln!("Native source/LLVM recheck inconclusive; artifact not accepted.");
                            std::process::exit(3);
                        }
                    }
                }
            }
        }
        NativeBundleCommand::AcceptStderr { bundle, source, expected_build_id, json } => {
            match recheck_artifact(Path::new(&bundle), Path::new(&source), &expected_build_id) {
                Err(error) => Err(error),
                Ok(recheck) => {
                    let ArtifactRecheckResult {
                        result,
                        family,
                        canonical_argv_compared,
                        expected_stderr_sha256,
                        pin_envelope,
                        stage,
                        ..
                    } = recheck;
                    match (result, expected_stderr_sha256, stage) {
                        (
                            ArtifactRecheck::Rechecked,
                            Some(expected),
                            Some(stage),
                        ) => {
                            match observed_stderr(
                                &stage.path().join(BundleFile::Executable.file_name()),
                            ) {
                                Some(actual) if actual == expected => {
                                    let acceptance = match (family, canonical_argv_compared) {
                                        (RecordFamily::Multifile, true) => {
                                            &MULTIFILE_STDERR_ACCEPTANCE
                                        }
                                        (RecordFamily::Multifile, false) => {
                                            &MULTIFILE_STDERR_ACCEPTANCE_ARGV_UNPINNED
                                        }
                                        (RecordFamily::Single | RecordFamily::Module, true) => {
                                            &STDERR_ACCEPTANCE
                                        }
                                        (RecordFamily::Single | RecordFamily::Module, false) => {
                                            &STDERR_ACCEPTANCE_ARGV_UNPINNED
                                        }
                                    };
                                    write_native_acceptance(
                                        Outcome::EquivalentWithinProfile,
                                        json,
                                        acceptance,
                                        None,
                                        Some(pin_envelope),
                                        &mut std::io::stdout().lock(),
                                    )
                                    .map(|_| ())
                                }
                                Some(_) => Err(BundleError(
                                    "reproduced executable's stderr differs from the pinned digest",
                                )),
                                None => {
                                    eprintln!("Executable stderr observation inconclusive; spawn failed, timed out, terminated by signal or exceeded the capture cap.");
                                    drop(stage);
                                    std::process::exit(3);
                                }
                            }
                        }
                        (ArtifactRecheck::Rechecked, _, stage) => {
                            eprintln!("Record pins no expected stderr digest or the reproduced executable is unavailable; inconclusive.");
                            drop(stage);
                            std::process::exit(3);
                        }
                        (ArtifactRecheck::Different, ..) => Err(BundleError(
                            "pinned artifact differs from the toolchain's output for the pinned inputs",
                        )),
                        (ArtifactRecheck::Inconclusive, ..) => {
                            eprintln!("Native source/LLVM recheck inconclusive; artifact not accepted.");
                            std::process::exit(3);
                        }
                    }
                }
            }
        }
        NativeBundleCommand::RecheckObservations {
            bundle,
            source,
            expected_build_id,
            json,
        } => {
            match recheck_artifact(Path::new(&bundle), Path::new(&source), &expected_build_id) {
                Err(error) => Err(error),
                Ok(ArtifactRecheckResult {
                    result: ArtifactRecheck::Different,
                    ..
                }) => Err(BundleError(
                    "pinned artifact differs from the toolchain's output for the pinned inputs",
                )),
                Ok(ArtifactRecheckResult {
                    result: ArtifactRecheck::Inconclusive,
                    ..
                }) => {
                    eprintln!("Native source/LLVM recheck inconclusive; no artifact result established.");
                    std::process::exit(3);
                }
                Ok(ArtifactRecheckResult {
                    result: ArtifactRecheck::Rechecked,
                    expected_exit_status,
                    expected_stdout_sha256,
                    expected_stderr_sha256,
                    pin_envelope,
                    stage,
                    ..
                }) => match check_runtime_observations(
                    expected_exit_status,
                    expected_stdout_sha256,
                    expected_stderr_sha256,
                    stage,
                ) {
                    RuntimeObservationCheck::Matching(observation) => {
                        if json {
                            use std::io::Write;
                            let descriptor = serde_json::json!({
                                "schema_version": "mumei.runtime-observation/v1",
                                "verdict": "observed_matching",
                                "scope": RUNTIME_OBSERVATION_RECHECK_SCOPE,
                                "build_id": expected_build_id,
                                "invocation": observation_invocation_envelope(),
                                "host": observation_host(),
                                "pin_envelope": pin_envelope,
                                "observations": {
                                    "exit_status": observation.exit_status,
                                    "stdout_sha256": observation.stdout_sha256,
                                    "stderr_sha256": observation.stderr_sha256,
                                },
                                "checks_performed": [
                                    "os_exit_status_observation",
                                    "os_stdout_observation",
                                    "os_stderr_observation",
                                    "runtime_observation_semantics",
                                ],
                                "checks_not_performed": [
                                    "producer_authenticity",
                                    "historical_execution",
                                    "optimizer_semantics",
                                    "abi_runtime_semantics",
                                    "grade_promotion",
                                    "lean_kernel_checks",
                                ],
                            });
                            serde_json::to_vec_pretty(&descriptor)
                                .map_err(|_| {
                                    BundleError("runtime observation descriptor encoding failed")
                                })
                                .and_then(|mut bytes| {
                                    bytes.push(b'\n');
                                    std::io::stdout()
                                        .lock()
                                        .write_all(&bytes)
                                        .map_err(|_| {
                                            BundleError("cannot write runtime observation descriptor")
                                        })
                                })
                        } else {
                            println!("Pinned toolchain outputs reproduce the pinned object and executable; one invocation of the reproduced executable matches every pinned runtime observation (exit status, stdout and stderr).\nScope: {RUNTIME_OBSERVATION_RECHECK_SCOPE}");
                            Ok(())
                        }
                    }
                    RuntimeObservationCheck::Different => Err(BundleError(
                        "reproduced executable's runtime observations differ from the record pins",
                    )),
                    RuntimeObservationCheck::Inconclusive(message) => {
                        eprintln!("{message}");
                        std::process::exit(3);
                    }
                },
            }
        }
        NativeBundleCommand::AcceptObservations {
            bundle,
            source,
            expected_build_id,
            json,
        } => {
            match recheck_artifact(Path::new(&bundle), Path::new(&source), &expected_build_id) {
                Err(error) => Err(error),
                Ok(ArtifactRecheckResult {
                    result: ArtifactRecheck::Different,
                    ..
                }) => Err(BundleError(
                    "pinned artifact differs from the toolchain's output for the pinned inputs",
                )),
                Ok(ArtifactRecheckResult {
                    result: ArtifactRecheck::Inconclusive,
                    ..
                }) => {
                    eprintln!("Native source/LLVM recheck inconclusive; artifact not accepted.");
                    std::process::exit(3);
                }
                Ok(ArtifactRecheckResult {
                    result: ArtifactRecheck::Rechecked,
                    family,
                    canonical_argv_compared,
                    expected_exit_status,
                    expected_stdout_sha256,
                    expected_stderr_sha256,
                    pin_envelope,
                    stage,
                    ..
                }) => match check_runtime_observations(
                    expected_exit_status,
                    expected_stdout_sha256,
                    expected_stderr_sha256,
                    stage,
                ) {
                    RuntimeObservationCheck::Matching(_) => {
                        let acceptance = match (family, canonical_argv_compared) {
                            (RecordFamily::Multifile, true) => {
                                &MULTIFILE_RUNTIME_OBSERVATION_ACCEPTANCE
                            }
                            (RecordFamily::Multifile, false) => {
                                &MULTIFILE_RUNTIME_OBSERVATION_ACCEPTANCE_ARGV_UNPINNED
                            }
                            (RecordFamily::Single | RecordFamily::Module, true) => {
                                &RUNTIME_OBSERVATION_ACCEPTANCE
                            }
                            (RecordFamily::Single | RecordFamily::Module, false) => {
                                &RUNTIME_OBSERVATION_ACCEPTANCE_ARGV_UNPINNED
                            }
                        };
                        write_native_acceptance(
                            Outcome::EquivalentWithinProfile,
                            json,
                            acceptance,
                            None,
                            Some(pin_envelope),
                            &mut std::io::stdout().lock(),
                        )
                        .map(|_| ())
                    }
                    RuntimeObservationCheck::Different => Err(BundleError(
                        "reproduced executable's runtime observations differ from the record pins",
                    )),
                    RuntimeObservationCheck::Inconclusive(message) => {
                        eprintln!("{message}");
                        std::process::exit(3);
                    }
                },
            }
        }
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn write_llvm_recheck(
    result: LlvmRecheck,
    json: bool,
    output: &mut impl std::io::Write,
) -> Result<Outcome, BundleError> {
    match result {
        LlvmRecheck::Rechecked(report) => {
            let bytes = if json {
                let mut bytes = serde_json::to_vec_pretty(&report)
                    .map_err(|_| BundleError("cannot encode consumer evidence"))?;
                bytes.push(b'\n');
                bytes
            } else {
                format!("Pinned snapshot identities consistent; source postcondition and native LLVM body freshly rechecked.\nProfile: {}\nScope: {LLVM_RECHECK_SCOPE}\nWrapper: checked call/trunc/ret shape, returning low 32 bits of the i64 body.\nObject/linker/runtime/OS exit-status/executable semantics: not checked.\n", tv::NATIVE_PROFILE).into_bytes()
            };
            output
                .write_all(&bytes)
                .map_err(|_| BundleError("cannot write consumer evidence"))?;
            Ok(Outcome::EquivalentWithinProfile)
        }
        LlvmRecheck::ModuleRechecked(profile, report) => {
            let bytes = if json {
                let mut bytes = serde_json::to_vec_pretty(&report)
                    .map_err(|_| BundleError("cannot encode consumer evidence"))?;
                bytes.push(b'\n');
                bytes
            } else {
                format!("Pinned snapshot identities consistent; module source postconditions and native LLVM bodies freshly rechecked.\nProfile: {profile}\nScope: {LLVM_RECHECK_SCOPE}\nWrapper: checked call/trunc/ret shape, returning low 32 bits of the i64 body.\nObject/linker/runtime/OS exit-status/executable semantics: not checked.\n").into_bytes()
            };
            output
                .write_all(&bytes)
                .map_err(|_| BundleError("cannot write consumer evidence"))?;
            Ok(Outcome::EquivalentWithinProfile)
        }
        LlvmRecheck::MultifileRechecked(profile, report) => {
            let bytes = if json {
                let mut bytes = serde_json::to_vec_pretty(&report)
                    .map_err(|_| BundleError("cannot encode consumer evidence"))?;
                bytes.push(b'\n');
                bytes
            } else {
                format!("Pinned snapshot identities consistent; import closure merged from pinned members, module source postconditions and native LLVM bodies freshly rechecked.\nProfile: {profile}\nScope: {LLVM_RECHECK_SCOPE}\nWrapper: checked call/trunc/ret shape, returning low 32 bits of the i64 body.\nObject/linker/runtime/OS exit-status/executable semantics: not checked.\n").into_bytes()
            };
            output
                .write_all(&bytes)
                .map_err(|_| BundleError("cannot write consumer evidence"))?;
            Ok(Outcome::EquivalentWithinProfile)
        }
        LlvmRecheck::Different => Ok(Outcome::Different),
        LlvmRecheck::Inconclusive => Ok(Outcome::Inconclusive),
    }
}

/// Static content of one `mumei.native-evidence-acceptance/v1` descriptor:
/// what a successful evaluation checked, and the checks it did not run. One
/// scope per command keeps each descriptor's claim unambiguous.
struct NativeAcceptance {
    scope: &'static str,
    performed: &'static [&'static str],
    not_performed: &'static [&'static str],
    text: &'static str,
}

const OBSERVATION_ACCEPTANCE_CHECKS: &[&str] = &[
    "os_exit_status_observation",
    "os_stdout_observation",
    "os_stderr_observation",
    "runtime_observation_semantics",
];

const EVIDENCE_ACCEPTANCE: NativeAcceptance = NativeAcceptance {
    scope: "native_bundle_identity_source_llvm_and_saved_evidence_consistency",
    performed: &["bundle_identity_binding", "fresh_same_profile_source_recheck", "llvm_snapshot_translation_recheck", "saved_evidence_consistency"],
    not_performed: &["producer_authenticity", "historical_execution", "object_linker_artifact_recheck", "optimizer_semantics", "abi_runtime_semantics", "os_exit_status_observation", "os_stdout_observation", "os_stderr_observation", "grade_promotion", "lean_kernel_checks"],
    text: "Native Evidence accepted within the pinned bundle/source/LLVM profile: identities bound, source postcondition and LLVM snapshot freshly rechecked, saved report consistent.\nAcceptance scope excludes producer authenticity, historical execution, artifact/object/linker recheck, optimizer/ABI/runtime/OS semantics, grade and Lean promotion.\n",
};

const MULTIFILE_EVIDENCE_ACCEPTANCE: NativeAcceptance = NativeAcceptance {
    scope: "native_multifile_bundle_identity_merge_source_llvm_and_saved_evidence_consistency",
    performed: &["bundle_identity_binding", "pinned_member_merge_rederivation", "fresh_same_profile_source_recheck", "llvm_snapshot_translation_recheck", "saved_evidence_consistency"],
    not_performed: &["producer_authenticity", "historical_execution", "object_linker_artifact_recheck", "optimizer_semantics", "abi_runtime_semantics", "os_exit_status_observation", "os_stdout_observation", "os_stderr_observation", "grade_promotion", "lean_kernel_checks"],
    text: "Native multifile Evidence accepted within the pinned bundle/source/LLVM profile: identities bound, import closure re-merged from pinned members, source postcondition and LLVM snapshot freshly rechecked, saved report consistent.\nAcceptance scope excludes producer authenticity, historical execution, artifact/object/linker recheck, optimizer/ABI/runtime/OS semantics, grade and Lean promotion.\n",
};

const MULTIFILE_ARTIFACT_ACCEPTANCE: NativeAcceptance = NativeAcceptance {
    scope: "native_multifile_bundle_source_llvm_and_toolchain_artifact_reproduction",
    performed: &["bundle_identity_binding", "pinned_member_merge_rederivation", "fresh_same_profile_source_recheck", "llvm_snapshot_translation_recheck", "toolchain_object_executable_reproduction", "canonical_link_argument_vector_reproduction"],
    not_performed: &["producer_authenticity", "historical_execution", "raw_link_argument_vector_reproduction", "optimizer_semantics", "abi_runtime_semantics", "os_exit_status_observation", "os_stdout_observation", "os_stderr_observation", "grade_promotion", "lean_kernel_checks"],
    text: "Native multifile artifact accepted within the pinned bundle/toolchain profile: identities bound, import closure re-merged from pinned members, source postcondition and LLVM snapshot freshly rechecked, pinned object and executable reproduced byte-identically, canonical link argument vector re-derived.\nAcceptance scope excludes producer authenticity, historical execution, raw link argument-vector reproduction (absolute staging paths are unreproducible), optimizer/ABI/runtime/OS exit-status/stdout/stderr semantics, grade and Lean promotion.\n",
};

const MULTIFILE_ARTIFACT_ACCEPTANCE_ARGV_UNPINNED: NativeAcceptance = NativeAcceptance {
    scope: "native_multifile_bundle_source_llvm_and_toolchain_artifact_reproduction",
    performed: &["bundle_identity_binding", "pinned_member_merge_rederivation", "fresh_same_profile_source_recheck", "llvm_snapshot_translation_recheck", "toolchain_object_executable_reproduction"],
    not_performed: &["producer_authenticity", "historical_execution", "canonical_link_argument_vector_reproduction", "raw_link_argument_vector_reproduction", "optimizer_semantics", "abi_runtime_semantics", "os_exit_status_observation", "os_stdout_observation", "os_stderr_observation", "grade_promotion", "lean_kernel_checks"],
    text: "Native multifile artifact accepted within the pinned bundle/toolchain profile: identities bound, import closure re-merged from pinned members, source postcondition and LLVM snapshot freshly rechecked, pinned object and executable reproduced byte-identically.\nAcceptance scope excludes producer authenticity, historical execution, canonical link argument-vector reproduction (the record pins no canonical argv), raw link argument-vector reproduction (absolute staging paths are unreproducible), optimizer/ABI/runtime/OS exit-status/stdout/stderr semantics, grade and Lean promotion.\n",
};

const ARTIFACT_ACCEPTANCE: NativeAcceptance = NativeAcceptance {
    scope: "native_bundle_source_llvm_and_toolchain_artifact_reproduction",
    performed: &["bundle_identity_binding", "fresh_same_profile_source_recheck", "llvm_snapshot_translation_recheck", "toolchain_object_executable_reproduction", "canonical_link_argument_vector_reproduction"],
    not_performed: &["producer_authenticity", "historical_execution", "raw_link_argument_vector_reproduction", "optimizer_semantics", "abi_runtime_semantics", "os_exit_status_observation", "os_stdout_observation", "os_stderr_observation", "grade_promotion", "lean_kernel_checks"],
    text: "Native artifact accepted within the pinned bundle/toolchain profile: identities bound, source postcondition and LLVM snapshot freshly rechecked, pinned object and executable reproduced byte-identically, canonical link argument vector re-derived.\nAcceptance scope excludes producer authenticity, historical execution, raw link argument-vector reproduction (absolute staging paths are unreproducible), optimizer/ABI/runtime/OS exit-status/stdout/stderr semantics, grade and Lean promotion.\n",
};

const ARTIFACT_ACCEPTANCE_ARGV_UNPINNED: NativeAcceptance = NativeAcceptance {
    scope: "native_bundle_source_llvm_and_toolchain_artifact_reproduction",
    performed: &["bundle_identity_binding", "fresh_same_profile_source_recheck", "llvm_snapshot_translation_recheck", "toolchain_object_executable_reproduction"],
    not_performed: &["producer_authenticity", "historical_execution", "canonical_link_argument_vector_reproduction", "raw_link_argument_vector_reproduction", "optimizer_semantics", "abi_runtime_semantics", "os_exit_status_observation", "os_stdout_observation", "os_stderr_observation", "grade_promotion", "lean_kernel_checks"],
    text: "Native artifact accepted within the pinned bundle/toolchain profile: identities bound, source postcondition and LLVM snapshot freshly rechecked, pinned object and executable reproduced byte-identically.\nAcceptance scope excludes producer authenticity, historical execution, canonical link argument-vector reproduction (the record pins no canonical argv), raw link argument-vector reproduction (absolute staging paths are unreproducible), optimizer/ABI/runtime/OS exit-status/stdout/stderr semantics, grade and Lean promotion.\n",
};

// Exit-status acceptance descriptors: the artifact acceptance claim plus
// the OS exit-status *observation* moved into `checks_performed` — named
// as an observation, not a semantics proof, since one run under one
// environment is all it is. `historical_execution` and the semantic
// exclusions stay not performed.
const MULTIFILE_EXIT_STATUS_ACCEPTANCE: NativeAcceptance = NativeAcceptance {
    scope: "native_multifile_bundle_source_llvm_toolchain_reproduction_and_os_exit_status_observation",
    performed: &["bundle_identity_binding", "pinned_member_merge_rederivation", "fresh_same_profile_source_recheck", "llvm_snapshot_translation_recheck", "toolchain_object_executable_reproduction", "canonical_link_argument_vector_reproduction", "os_exit_status_observation"],
    not_performed: &["producer_authenticity", "historical_execution", "raw_link_argument_vector_reproduction", "optimizer_semantics", "abi_runtime_semantics", "os_stdout_observation", "os_stderr_observation", "grade_promotion", "lean_kernel_checks"],
    text: "Native multifile artifact accepted within the pinned bundle/toolchain profile: identities bound, import closure re-merged from pinned members, source postcondition and LLVM snapshot freshly rechecked, pinned object and executable reproduced byte-identically, canonical link argument vector re-derived, reproduced executable's OS exit status observed matching the record pin.\nAcceptance scope excludes producer authenticity, historical execution (one run is an observation, not a history), raw link argument-vector reproduction (absolute staging paths are unreproducible), stdout/stderr observation, optimizer/ABI/runtime/process semantics, grade and Lean promotion.\n",
};

const MULTIFILE_EXIT_STATUS_ACCEPTANCE_ARGV_UNPINNED: NativeAcceptance = NativeAcceptance {
    scope: "native_multifile_bundle_source_llvm_toolchain_reproduction_and_os_exit_status_observation",
    performed: &["bundle_identity_binding", "pinned_member_merge_rederivation", "fresh_same_profile_source_recheck", "llvm_snapshot_translation_recheck", "toolchain_object_executable_reproduction", "os_exit_status_observation"],
    not_performed: &["producer_authenticity", "historical_execution", "canonical_link_argument_vector_reproduction", "raw_link_argument_vector_reproduction", "optimizer_semantics", "abi_runtime_semantics", "os_stdout_observation", "os_stderr_observation", "grade_promotion", "lean_kernel_checks"],
    text: "Native multifile artifact accepted within the pinned bundle/toolchain profile: identities bound, import closure re-merged from pinned members, source postcondition and LLVM snapshot freshly rechecked, pinned object and executable reproduced byte-identically, reproduced executable's OS exit status observed matching the record pin.\nAcceptance scope excludes producer authenticity, historical execution (one run is an observation, not a history), canonical link argument-vector reproduction (the record pins no canonical argv), raw link argument-vector reproduction (absolute staging paths are unreproducible), stdout/stderr observation, optimizer/ABI/runtime/process semantics, grade and Lean promotion.\n",
};

const EXIT_STATUS_ACCEPTANCE: NativeAcceptance = NativeAcceptance {
    scope: "native_bundle_source_llvm_toolchain_reproduction_and_os_exit_status_observation",
    performed: &["bundle_identity_binding", "fresh_same_profile_source_recheck", "llvm_snapshot_translation_recheck", "toolchain_object_executable_reproduction", "canonical_link_argument_vector_reproduction", "os_exit_status_observation"],
    not_performed: &["producer_authenticity", "historical_execution", "raw_link_argument_vector_reproduction", "optimizer_semantics", "abi_runtime_semantics", "os_stdout_observation", "os_stderr_observation", "grade_promotion", "lean_kernel_checks"],
    text: "Native artifact accepted within the pinned bundle/toolchain profile: identities bound, source postcondition and LLVM snapshot freshly rechecked, pinned object and executable reproduced byte-identically, canonical link argument vector re-derived, reproduced executable's OS exit status observed matching the record pin.\nAcceptance scope excludes producer authenticity, historical execution (one run is an observation, not a history), raw link argument-vector reproduction (absolute staging paths are unreproducible), stdout/stderr observation, optimizer/ABI/runtime/process semantics, grade and Lean promotion.\n",
};

const EXIT_STATUS_ACCEPTANCE_ARGV_UNPINNED: NativeAcceptance = NativeAcceptance {
    scope: "native_bundle_source_llvm_toolchain_reproduction_and_os_exit_status_observation",
    performed: &["bundle_identity_binding", "fresh_same_profile_source_recheck", "llvm_snapshot_translation_recheck", "toolchain_object_executable_reproduction", "os_exit_status_observation"],
    not_performed: &["producer_authenticity", "historical_execution", "canonical_link_argument_vector_reproduction", "raw_link_argument_vector_reproduction", "optimizer_semantics", "abi_runtime_semantics", "os_stdout_observation", "os_stderr_observation", "grade_promotion", "lean_kernel_checks"],
    text: "Native artifact accepted within the pinned bundle/toolchain profile: identities bound, source postcondition and LLVM snapshot freshly rechecked, pinned object and executable reproduced byte-identically, reproduced executable's OS exit status observed matching the record pin.\nAcceptance scope excludes producer authenticity, historical execution (one run is an observation, not a history), canonical link argument-vector reproduction (the record pins no canonical argv), raw link argument-vector reproduction (absolute staging paths are unreproducible), stdout/stderr observation, optimizer/ABI/runtime/process semantics, grade and Lean promotion.\n",
};

// Stdout acceptance descriptors: the artifact acceptance claim plus the
// OS stdout-digest *observation* moved into `checks_performed` — named
// as an observation, not an output-semantics proof, since one run under
// one environment is all it is. Each channel is observed by its own
// command, so this one never captures stderr and names
// `os_stderr_observation` in the not-performed list; the paired
// `accept-stderr` command owns that channel.
const MULTIFILE_STDOUT_ACCEPTANCE: NativeAcceptance = NativeAcceptance {
    scope: "native_multifile_bundle_source_llvm_toolchain_reproduction_and_os_stdout_observation",
    performed: &["bundle_identity_binding", "pinned_member_merge_rederivation", "fresh_same_profile_source_recheck", "llvm_snapshot_translation_recheck", "toolchain_object_executable_reproduction", "canonical_link_argument_vector_reproduction", "os_stdout_observation"],
    not_performed: &["producer_authenticity", "historical_execution", "raw_link_argument_vector_reproduction", "optimizer_semantics", "abi_runtime_semantics", "os_exit_status_observation", "os_stderr_observation", "grade_promotion", "lean_kernel_checks"],
    text: "Native multifile artifact accepted within the pinned bundle/toolchain profile: identities bound, import closure re-merged from pinned members, source postcondition and LLVM snapshot freshly rechecked, pinned object and executable reproduced byte-identically, canonical link argument vector re-derived, reproduced executable's stdout digest observed matching the record pin.\nAcceptance scope excludes producer authenticity, historical execution (one run is an observation, not a history), OS exit-status observation, stderr observation, raw link argument-vector reproduction (absolute staging paths are unreproducible), optimizer/ABI/runtime/process semantics, grade and Lean promotion.\n",
};

const MULTIFILE_STDOUT_ACCEPTANCE_ARGV_UNPINNED: NativeAcceptance = NativeAcceptance {
    scope: "native_multifile_bundle_source_llvm_toolchain_reproduction_and_os_stdout_observation",
    performed: &["bundle_identity_binding", "pinned_member_merge_rederivation", "fresh_same_profile_source_recheck", "llvm_snapshot_translation_recheck", "toolchain_object_executable_reproduction", "os_stdout_observation"],
    not_performed: &["producer_authenticity", "historical_execution", "canonical_link_argument_vector_reproduction", "raw_link_argument_vector_reproduction", "optimizer_semantics", "abi_runtime_semantics", "os_exit_status_observation", "os_stderr_observation", "grade_promotion", "lean_kernel_checks"],
    text: "Native multifile artifact accepted within the pinned bundle/toolchain profile: identities bound, import closure re-merged from pinned members, source postcondition and LLVM snapshot freshly rechecked, pinned object and executable reproduced byte-identically, reproduced executable's stdout digest observed matching the record pin.\nAcceptance scope excludes producer authenticity, historical execution (one run is an observation, not a history), OS exit-status observation, stderr observation, canonical link argument-vector reproduction (the record pins no canonical argv), raw link argument-vector reproduction (absolute staging paths are unreproducible), optimizer/ABI/runtime/process semantics, grade and Lean promotion.\n",
};

const STDOUT_ACCEPTANCE: NativeAcceptance = NativeAcceptance {
    scope: "native_bundle_source_llvm_toolchain_reproduction_and_os_stdout_observation",
    performed: &["bundle_identity_binding", "fresh_same_profile_source_recheck", "llvm_snapshot_translation_recheck", "toolchain_object_executable_reproduction", "canonical_link_argument_vector_reproduction", "os_stdout_observation"],
    not_performed: &["producer_authenticity", "historical_execution", "raw_link_argument_vector_reproduction", "optimizer_semantics", "abi_runtime_semantics", "os_exit_status_observation", "os_stderr_observation", "grade_promotion", "lean_kernel_checks"],
    text: "Native artifact accepted within the pinned bundle/toolchain profile: identities bound, source postcondition and LLVM snapshot freshly rechecked, pinned object and executable reproduced byte-identically, canonical link argument vector re-derived, reproduced executable's stdout digest observed matching the record pin.\nAcceptance scope excludes producer authenticity, historical execution (one run is an observation, not a history), OS exit-status observation, stderr observation, raw link argument-vector reproduction (absolute staging paths are unreproducible), optimizer/ABI/runtime/process semantics, grade and Lean promotion.\n",
};

const STDOUT_ACCEPTANCE_ARGV_UNPINNED: NativeAcceptance = NativeAcceptance {
    scope: "native_bundle_source_llvm_toolchain_reproduction_and_os_stdout_observation",
    performed: &["bundle_identity_binding", "fresh_same_profile_source_recheck", "llvm_snapshot_translation_recheck", "toolchain_object_executable_reproduction", "os_stdout_observation"],
    not_performed: &["producer_authenticity", "historical_execution", "canonical_link_argument_vector_reproduction", "raw_link_argument_vector_reproduction", "optimizer_semantics", "abi_runtime_semantics", "os_exit_status_observation", "os_stderr_observation", "grade_promotion", "lean_kernel_checks"],
    text: "Native artifact accepted within the pinned bundle/toolchain profile: identities bound, source postcondition and LLVM snapshot freshly rechecked, pinned object and executable reproduced byte-identically, reproduced executable's stdout digest observed matching the record pin.\nAcceptance scope excludes producer authenticity, historical execution (one run is an observation, not a history), OS exit-status observation, stderr observation, canonical link argument-vector reproduction (the record pins no canonical argv), raw link argument-vector reproduction (absolute staging paths are unreproducible), optimizer/ABI/runtime/process semantics, grade and Lean promotion.\n",
};

// Stderr acceptance descriptors: the same artifact acceptance claim
// with the OS stderr-digest *observation* performed — completing the
// OS channel observation family (exit status, stdout, stderr). The
// pin makes the channel deterministic where one bounded run is
// deterministic; a run whose stderr drifts between producer and
// consumer fails closed as `Different`, never as a claimed match.
const MULTIFILE_STDERR_ACCEPTANCE: NativeAcceptance = NativeAcceptance {
    scope: "native_multifile_bundle_source_llvm_toolchain_reproduction_and_os_stderr_observation",
    performed: &["bundle_identity_binding", "pinned_member_merge_rederivation", "fresh_same_profile_source_recheck", "llvm_snapshot_translation_recheck", "toolchain_object_executable_reproduction", "canonical_link_argument_vector_reproduction", "os_stderr_observation"],
    not_performed: &["producer_authenticity", "historical_execution", "raw_link_argument_vector_reproduction", "optimizer_semantics", "abi_runtime_semantics", "os_exit_status_observation", "os_stdout_observation", "grade_promotion", "lean_kernel_checks"],
    text: "Native multifile artifact accepted within the pinned bundle/toolchain profile: identities bound, import closure re-merged from pinned members, source postcondition and LLVM snapshot freshly rechecked, pinned object and executable reproduced byte-identically, canonical link argument vector re-derived, reproduced executable's stderr digest observed matching the record pin.\nAcceptance scope excludes producer authenticity, historical execution (one run is an observation, not a history), OS exit-status observation, stdout observation, raw link argument-vector reproduction (absolute staging paths are unreproducible), optimizer/ABI/runtime/process semantics, grade and Lean promotion.\n",
};

const MULTIFILE_STDERR_ACCEPTANCE_ARGV_UNPINNED: NativeAcceptance = NativeAcceptance {
    scope: "native_multifile_bundle_source_llvm_toolchain_reproduction_and_os_stderr_observation",
    performed: &["bundle_identity_binding", "pinned_member_merge_rederivation", "fresh_same_profile_source_recheck", "llvm_snapshot_translation_recheck", "toolchain_object_executable_reproduction", "os_stderr_observation"],
    not_performed: &["producer_authenticity", "historical_execution", "canonical_link_argument_vector_reproduction", "raw_link_argument_vector_reproduction", "optimizer_semantics", "abi_runtime_semantics", "os_exit_status_observation", "os_stdout_observation", "grade_promotion", "lean_kernel_checks"],
    text: "Native multifile artifact accepted within the pinned bundle/toolchain profile: identities bound, import closure re-merged from pinned members, source postcondition and LLVM snapshot freshly rechecked, pinned object and executable reproduced byte-identically, reproduced executable's stderr digest observed matching the record pin.\nAcceptance scope excludes producer authenticity, historical execution (one run is an observation, not a history), OS exit-status observation, stdout observation, canonical link argument-vector reproduction (the record pins no canonical argv), raw link argument-vector reproduction (absolute staging paths are unreproducible), optimizer/ABI/runtime/process semantics, grade and Lean promotion.\n",
};

const STDERR_ACCEPTANCE: NativeAcceptance = NativeAcceptance {
    scope: "native_bundle_source_llvm_toolchain_reproduction_and_os_stderr_observation",
    performed: &["bundle_identity_binding", "fresh_same_profile_source_recheck", "llvm_snapshot_translation_recheck", "toolchain_object_executable_reproduction", "canonical_link_argument_vector_reproduction", "os_stderr_observation"],
    not_performed: &["producer_authenticity", "historical_execution", "raw_link_argument_vector_reproduction", "optimizer_semantics", "abi_runtime_semantics", "os_exit_status_observation", "os_stdout_observation", "grade_promotion", "lean_kernel_checks"],
    text: "Native artifact accepted within the pinned bundle/toolchain profile: identities bound, source postcondition and LLVM snapshot freshly rechecked, pinned object and executable reproduced byte-identically, canonical link argument vector re-derived, reproduced executable's stderr digest observed matching the record pin.\nAcceptance scope excludes producer authenticity, historical execution (one run is an observation, not a history), OS exit-status observation, stdout observation, raw link argument-vector reproduction (absolute staging paths are unreproducible), optimizer/ABI/runtime/process semantics, grade and Lean promotion.\n",
};

const STDERR_ACCEPTANCE_ARGV_UNPINNED: NativeAcceptance = NativeAcceptance {
    scope: "native_bundle_source_llvm_toolchain_reproduction_and_os_stderr_observation",
    performed: &["bundle_identity_binding", "fresh_same_profile_source_recheck", "llvm_snapshot_translation_recheck", "toolchain_object_executable_reproduction", "os_stderr_observation"],
    not_performed: &["producer_authenticity", "historical_execution", "canonical_link_argument_vector_reproduction", "raw_link_argument_vector_reproduction", "optimizer_semantics", "abi_runtime_semantics", "os_exit_status_observation", "os_stdout_observation", "grade_promotion", "lean_kernel_checks"],
    text: "Native artifact accepted within the pinned bundle/toolchain profile: identities bound, source postcondition and LLVM snapshot freshly rechecked, pinned object and executable reproduced byte-identically, reproduced executable's stderr digest observed matching the record pin.\nAcceptance scope excludes producer authenticity, historical execution (one run is an observation, not a history), OS exit-status observation, stdout observation, canonical link argument-vector reproduction (the record pins no canonical argv), raw link argument-vector reproduction (absolute staging paths are unreproducible), optimizer/ABI/runtime/process semantics, grade and Lean promotion.\n",
};

// Runtime-observation acceptance descriptors: all three channel
// observations come from ONE run, so `runtime_observation_semantics` is
// performed only when that same run observed all three.
const MULTIFILE_RUNTIME_OBSERVATION_ACCEPTANCE: NativeAcceptance = NativeAcceptance {
    scope: "native_multifile_bundle_source_llvm_toolchain_reproduction_and_runtime_observation_semantics",
    performed: &["bundle_identity_binding", "pinned_member_merge_rederivation", "fresh_same_profile_source_recheck", "llvm_snapshot_translation_recheck", "toolchain_object_executable_reproduction", "canonical_link_argument_vector_reproduction", "os_exit_status_observation", "os_stdout_observation", "os_stderr_observation", "runtime_observation_semantics"],
    not_performed: &["producer_authenticity", "historical_execution", "raw_link_argument_vector_reproduction", "optimizer_semantics", "abi_runtime_semantics", "grade_promotion", "lean_kernel_checks"],
    text: "Native multifile artifact accepted within the pinned bundle/toolchain profile: identities bound, import closure re-merged from pinned members, source postcondition and LLVM snapshot freshly rechecked, pinned object and executable reproduced byte-identically, canonical link argument vector re-derived, one invocation of the reproduced executable observed matching every runtime-observation pin (exit status, stdout and stderr).\nAcceptance scope excludes producer authenticity, historical execution (one run is an observation, not a history), raw link argument-vector reproduction (absolute staging paths are unreproducible), optimizer/ABI/runtime/process semantics, grade and Lean promotion.\n",
};

const MULTIFILE_RUNTIME_OBSERVATION_ACCEPTANCE_ARGV_UNPINNED: NativeAcceptance =
    NativeAcceptance {
        scope: "native_multifile_bundle_source_llvm_toolchain_reproduction_and_runtime_observation_semantics",
        performed: &[
            "bundle_identity_binding",
            "pinned_member_merge_rederivation",
            "fresh_same_profile_source_recheck",
            "llvm_snapshot_translation_recheck",
            "toolchain_object_executable_reproduction",
            "os_exit_status_observation",
            "os_stdout_observation",
            "os_stderr_observation",
            "runtime_observation_semantics",
        ],
        not_performed: &[
            "producer_authenticity",
            "historical_execution",
            "canonical_link_argument_vector_reproduction",
            "raw_link_argument_vector_reproduction",
            "optimizer_semantics",
            "abi_runtime_semantics",
            "grade_promotion",
            "lean_kernel_checks",
        ],
        text: "Native multifile artifact accepted within the pinned bundle/toolchain profile: identities bound, import closure re-merged from pinned members, source postcondition and LLVM snapshot freshly rechecked, pinned object and executable reproduced byte-identically, one invocation of the reproduced executable observed matching every runtime-observation pin (exit status, stdout and stderr).\nAcceptance scope excludes producer authenticity, historical execution (one run is an observation, not a history), canonical link argument-vector reproduction (the record pins no canonical argv), raw link argument-vector reproduction (absolute staging paths are unreproducible), optimizer/ABI/runtime/process semantics, grade and Lean promotion.\n",
    };

const RUNTIME_OBSERVATION_ACCEPTANCE: NativeAcceptance = NativeAcceptance {
    scope: "native_bundle_source_llvm_toolchain_reproduction_and_runtime_observation_semantics",
    performed: &["bundle_identity_binding", "fresh_same_profile_source_recheck", "llvm_snapshot_translation_recheck", "toolchain_object_executable_reproduction", "canonical_link_argument_vector_reproduction", "os_exit_status_observation", "os_stdout_observation", "os_stderr_observation", "runtime_observation_semantics"],
    not_performed: &["producer_authenticity", "historical_execution", "raw_link_argument_vector_reproduction", "optimizer_semantics", "abi_runtime_semantics", "grade_promotion", "lean_kernel_checks"],
    text: "Native artifact accepted within the pinned bundle/toolchain profile: identities bound, source postcondition and LLVM snapshot freshly rechecked, pinned object and executable reproduced byte-identically, canonical link argument vector re-derived, one invocation of the reproduced executable observed matching every runtime-observation pin (exit status, stdout and stderr).\nAcceptance scope excludes producer authenticity, historical execution (one run is an observation, not a history), raw link argument-vector reproduction (absolute staging paths are unreproducible), optimizer/ABI/runtime/process semantics, grade and Lean promotion.\n",
};

const RUNTIME_OBSERVATION_ACCEPTANCE_ARGV_UNPINNED: NativeAcceptance = NativeAcceptance {
    scope: "native_bundle_source_llvm_toolchain_reproduction_and_runtime_observation_semantics",
    performed: &["bundle_identity_binding", "fresh_same_profile_source_recheck", "llvm_snapshot_translation_recheck", "toolchain_object_executable_reproduction", "os_exit_status_observation", "os_stdout_observation", "os_stderr_observation", "runtime_observation_semantics"],
    not_performed: &["producer_authenticity", "historical_execution", "canonical_link_argument_vector_reproduction", "raw_link_argument_vector_reproduction", "optimizer_semantics", "abi_runtime_semantics", "grade_promotion", "lean_kernel_checks"],
    text: "Native artifact accepted within the pinned bundle/toolchain profile: identities bound, source postcondition and LLVM snapshot freshly rechecked, pinned object and executable reproduced byte-identically, one invocation of the reproduced executable observed matching every runtime-observation pin (exit status, stdout and stderr).\nAcceptance scope excludes producer authenticity, historical execution (one run is an observation, not a history), canonical link argument-vector reproduction (the record pins no canonical argv), raw link argument-vector reproduction (absolute staging paths are unreproducible), optimizer/ABI/runtime/process semantics, grade and Lean promotion.\n",
};

/// On an accepted outcome emits one anonymous acceptance descriptor — the
/// same evaluation the paired `recheck-*` command runs, reported in the
/// acceptance vocabulary. The descriptor binds no subject, digest or
/// provenance so it cannot be replayed as Evidence; non-accepted outcomes
/// write nothing. The runtime-observation envelope contains only fixed
/// invocation constants, not subject, digest or provenance.
fn write_native_acceptance(
    outcome: Outcome,
    json: bool,
    acceptance: &NativeAcceptance,
    evidence_bytes: Option<&[u8]>,
    pin_envelope: Option<&'static str>,
    output: &mut impl std::io::Write,
) -> Result<Outcome, BundleError> {
    if outcome != Outcome::EquivalentWithinProfile {
        return Ok(outcome);
    }
    let bytes = if json {
        let mut value = serde_json::json!({
            "schema_version": "mumei.native-evidence-acceptance/v1",
            "acceptance": "evaluated",
            "accepted": true,
            "scope": acceptance.scope,
            "checks_performed": acceptance.performed,
            "checks_not_performed": acceptance.not_performed,
        });
        if acceptance
            .performed
            .iter()
            .any(|check| OBSERVATION_ACCEPTANCE_CHECKS.contains(check))
        {
            value["invocation"] = observation_invocation_envelope();
            value["host"] = observation_host();
            if let Some(pin_envelope) = pin_envelope {
                value["pin_envelope"] = serde_json::json!(pin_envelope);
            }
        }
        // Optional binding: sha256 of the accepted record's bytes lets a
        // holder prove WHICH record this descriptor evaluated, without the
        // descriptor echoing any claim or digest it could replay as identity.
        // Artifact acceptance has no record input, so the key stays absent.
        if let Some(bytes) = evidence_bytes {
            use sha2::{Digest, Sha256};
            value["evidence_fingerprint"] =
                serde_json::json!(format!("{:x}", Sha256::digest(bytes)));
        }
        let mut bytes = serde_json::to_vec_pretty(&value)
            .map_err(|_| BundleError("cannot encode native acceptance descriptor"))?;
        bytes.push(b'\n');
        bytes
    } else {
        acceptance.text.as_bytes().to_vec()
    };
    output
        .write_all(&bytes)
        .map_err(|_| BundleError("cannot write native acceptance descriptor"))?;
    Ok(outcome)
}

/// Read only the record's `schema` field — the dispatch key between the
/// record families. Unknown schemas error here so the family loaders
/// never see a cross-parsed record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordFamily {
    Single,
    Module,
    Multifile,
}

fn record_family(record: &[u8]) -> Result<RecordFamily, BundleError> {
    match serde_json::from_slice::<serde_json::Value>(record)
        .map_err(|_| BundleError("invalid record"))?
        .get("schema")
        .and_then(|schema| schema.as_str())
    {
        Some(MODULE_SCHEMA) => Ok(RecordFamily::Module),
        Some(MULTIFILE_SCHEMA) => Ok(RecordFamily::Multifile),
        Some(SCHEMA) => Ok(RecordFamily::Single),
        _ => Err(BundleError("unsupported schema, scope or settings")),
    }
}

/// Read the pinned original member texts of a multifile bundle — every
/// file under `sources/` is keyed by its `/`-separated relative path and
/// must pass the member-name grammar; the member count stays bounded.
fn source_members(bundle: &Path) -> Result<BTreeMap<String, String>, BundleError> {
    let mut members = BTreeMap::new();
    collect_source_members(bundle.join(SOURCES_DIR), "", 0, &mut members)?;
    Ok(members)
}

/// Walk `sources/` recursively: directories descend, files become
/// members keyed by their `/`-separated relative path, anything else
/// (symlinks, sockets) is rejected honestly. `depth` caps the walk at
/// the member grammar's segment bound — deeper trees can only hold
/// member keys the grammar would reject anyway, so a hostile bundle
/// cannot exhaust the stack.
fn collect_source_members(
    dir: PathBuf,
    prefix: &str,
    depth: usize,
    members: &mut BTreeMap<String, String>,
) -> Result<(), BundleError> {
    if depth > 16 {
        return Err(BundleError("source member path too deep"));
    }
    for entry in fs::read_dir(&dir).map_err(|_| BundleError("cannot read source members"))? {
        let entry = entry.map_err(|_| BundleError("cannot read source members"))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| BundleError("invalid source member name"))?;
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            collect_source_members(
                entry.path(),
                &format!("{prefix}{name}/"),
                depth + 1,
                members,
            )?;
            continue;
        }
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            return Err(BundleError("unexpected source member"));
        }
        let key = format!("{prefix}{name}");
        if member_name(&key).ok().as_deref() != Some(key.as_str()) {
            return Err(BundleError("invalid source member name"));
        }
        if members.len() >= MAX_SOURCE_FILES {
            return Err(BundleError("too many source members"));
        }
        members.insert(key, bounded_source_text(&entry.path())?);
    }
    Ok(())
}

/// The multifile inputs a consumer supplies: the `--source` file is the
/// ROOT member — its basename names the root and its bytes must equal
/// the pinned member, so a consumer cannot point the check at a
/// different file while the bundle's pins do the work.
fn multifile_inputs(
    bundle: &Path,
    source: &Path,
) -> Result<(BTreeMap<String, String>, String), BundleError> {
    let members = source_members(bundle)?;
    let root = member_name(
        source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(BundleError("invalid source member name"))?,
    )?;
    if members.get(&root) != Some(&bounded_source_text(source)?) {
        return Err(BundleError("root source member not pinned"));
    }
    Ok((members, root))
}

fn read_regular(path: &Path, limit: u64) -> Result<Vec<u8>, BundleError> {
    super::input::read_regular(path, limit).map_err(BundleError)
}

fn source_text(path: &Path) -> Result<String, BundleError> {
    String::from_utf8(read_regular(path, MAX_COMPONENT_BYTES)?)
        .map_err(|_| BundleError("source is not UTF-8"))
}

fn bounded_source_text(path: &Path) -> Result<String, BundleError> {
    String::from_utf8(read_regular(path, SOURCE_BYTES)?)
        .map_err(|_| BundleError("source is not UTF-8"))
}

fn components(directory: &Path) -> Result<BTreeMap<BundleFile, Vec<u8>>, BundleError> {
    BundleFile::ALL
        .into_iter()
        .map(|role| {
            Ok((
                role,
                read_regular(&directory.join(role.file_name()), MAX_COMPONENT_BYTES)?,
            ))
        })
        .collect()
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), BundleError> {
    fs::write(path, bytes).map_err(|_| BundleError("cannot stage component"))
}

fn link_recorded_bundle_object(
    object: &Path,
    stage: &Path,
    executable: &Path,
) -> Result<LinkBuild, BundleError> {
    let object_bytes = read_regular(object, MAX_COMPONENT_BYTES)?;
    let surface = object_symbol_surface(&object_bytes)
        .map_err(|_| BundleError("native object symbol parsing failed"))?;
    let needs_runtime = surface
        .undefined
        .iter()
        .any(|symbol| EMBEDDED_RUNTIME_EXPORTS.contains(&symbol.as_str()));
    if needs_runtime {
        let runtime = stage.join("mumei_runtime.c");
        write(&runtime, MUMEI_RUNTIME_C.as_bytes())?;
        linker::link_recorded_with_runtime(&[object.to_path_buf()], &runtime, executable)
            .map_err(|_| BundleError("native link or toolchain observation failed"))
    } else {
        linker::link_recorded(&[object.to_path_buf()], executable)
            .map_err(|_| BundleError("native link or toolchain observation failed"))
    }
}

fn build(input: &Path, output: &Path) -> Result<(String, &'static str), BundleError> {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        return Err(BundleError("unsupported host for first native profile"));
    }
    if fs::symlink_metadata(output).is_ok() {
        return Err(BundleError("output already exists"));
    }
    let source = source_text(input)?;
    // `parse_module` (lenient) — the checked pipeline consumes `import`
    // items, so it could never see this dispatch key.
    let items = parser::parse_module(&source);
    if items
        .iter()
        .any(|item| matches!(item, parser::Item::Import(_)))
    {
        return build_multifile(&source, input, output).map(|id| (id, MODULE_SCOPE));
    }
    let atoms = items
        .iter()
        .filter(|item| matches!(item, parser::Item::Atom(_)))
        .count();
    // A single-atom module still takes the module path when it declares
    // refined scalar types, structs or enums — `build_single` admits
    // only a lone atom.
    if atoms > 1
        || items.iter().any(|item| {
            matches!(
                item,
                parser::Item::TypeDef(_)
                    | parser::Item::StructDef(_)
                    | parser::Item::EnumDef(_)
                    | parser::Item::EffectDef(_)
            ) || matches!(
                item,
                parser::Item::Atom(atom)
                    if atom.params.iter().any(|param| param.type_name.as_deref() == Some("[i64]"))
            )
        })
    {
        build_module(&source, input, output).map(|id| (id, MODULE_SCOPE))
    } else {
        build_single(&source, input, output).map(|id| (id, SCOPE))
    }
}

fn build_single(source: &str, input: &Path, output: &Path) -> Result<String, BundleError> {
    let (binding, atom) = prepare_source(source)?;
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let stage = tempfile::Builder::new()
        .prefix(".mumei-native-")
        .tempdir_in(parent)
        .map_err(|_| BundleError("cannot stage bundle"))?;
    let proof_work = tempfile::Builder::new()
        .prefix(".mumei-proof-")
        .tempdir_in(parent)
        .map_err(|_| BundleError("cannot stage proof"))?;
    let mut env = verification::ModuleEnv::default();
    env.register_atom(&atom);
    let lowered = hir::lower_atom_to_hir(&atom);
    verification::verify_with_verification_config(
        &lowered,
        proof_work.path(),
        &env,
        &verification::VerificationConfig::default(),
    )
    .map_err(|_| BundleError("source verification failed"))?;
    env.mark_verified(&atom.name);
    let results = HashMap::from([(
        "main".to_owned(),
        (
            proof_cert::status::Z3_UNSAT.to_owned(),
            proof_cert::status::VERIFIED.to_owned(),
        ),
    )]);
    let cert =
        proof_cert::generate_certificate(SOURCE_LABEL, &[&atom], &results, &env, None, None, None);
    write(
        &stage.path().join(BundleFile::Certificate.file_name()),
        &serde_json::to_vec_pretty(&cert)
            .map_err(|_| BundleError("certificate encoding failed"))?,
    )?;
    let ir = stage.path().join(BundleFile::LlvmIr.file_name());
    let object = stage.path().join(BundleFile::Object.file_name());
    mumei_emit_llvm::binary::compile_atoms_to_binary_ll(&[lowered], &env, &[], &ir)
        .map_err(|_| BundleError("native IR emission failed"))?;
    let object_build = mumei_emit_llvm::codegen::compile_llvm_ir_to_object_recorded(&ir, &object)
        .map_err(|_| BundleError("native object emission failed"))?;
    let mut link_build = link_recorded_bundle_object(
        &object,
        stage.path(),
        &stage.path().join(BundleFile::Executable.file_name()),
    )?;
    link_build.expected_exit_status =
        observed_exit_status(&stage.path().join(BundleFile::Executable.file_name()));
    link_build.expected_stdout_sha256 =
        observed_stdout(&stage.path().join(BundleFile::Executable.file_name()));
    link_build.expected_stderr_sha256 =
        observed_stderr(&stage.path().join(BundleFile::Executable.file_name()));
    let has_observation_pin = link_build.expected_exit_status.is_some()
        || link_build.expected_stdout_sha256.is_some()
        || link_build.expected_stderr_sha256.is_some();
    link_build.observation_envelope = has_observation_pin.then(pinned_observation_envelope);
    let files = components(stage.path())?;
    let settings = BuildSettings::expected_for(&link_build);
    let record = NativeBundleRecord {
        schema: SCHEMA.into(),
        profile: PROFILE.into(),
        scope: SCOPE.into(),
        system_scope: SYSTEM_SCOPE.into(),
        source: binding,
        object_build,
        link_build,
        settings,
        files: files
            .iter()
            .map(|(role, bytes)| (*role, ContentIdentity::of(bytes)))
            .collect(),
    };
    let bytes =
        serde_json::to_vec_pretty(&record).map_err(|_| BundleError("record encoding failed"))?;
    let id = sha256(&bytes);
    check_identity(&bytes, &id, source, &files)?;
    if source_text(input)? != source {
        return Err(BundleError("source changed during build"));
    }
    write(&stage.path().join(RECORD_FILE), &bytes)?;
    publish(stage.path(), output)?;
    let _ = stage.keep();
    Ok(id)
}

/// The emit pipeline's products: the pinned component snapshot plus the
/// object/link toolchain observations the record carries.
type EmittedModule = (BTreeMap<BundleFile, Vec<u8>>, ObjectBuild, LinkBuild);

/// Verify every admitted atom, emit the shared IR/object/executable into
/// `stage` and return the component snapshot plus toolchain
/// observations — the pipeline the module and multifile records both pin.
fn emit_module(
    atoms: &[parser::Atom],
    types: &[parser::RefinedType],
    structs: &[parser::StructDef],
    enums: &[parser::EnumDef],
    stage: &Path,
    proof_work: &Path,
    admission: RefinedAdmission,
) -> Result<EmittedModule, BundleError> {
    let mut env = verification::ModuleEnv::default();
    // Register the module's refined scalar, struct and enum
    // declarations before the atoms — signature members typed by an
    // alias, a struct or an enum resolve through the env the same way
    // `mumei verify`'s module environment does.
    for typedef in types {
        env.register_type(typedef);
    }
    for struct_def in structs {
        env.register_struct(struct_def);
    }
    for enum_def in enums {
        env.register_enum(enum_def);
    }
    for atom in atoms {
        env.register_atom(atom);
    }
    // Callees are admitted by contract (extract_pure_i64_module_contracts
    // already ran the profile gates in the prepare step), so sibling
    // calls verify in any declaration order; mark_verified keeps the env
    // consistent for the certificate.
    let mut lowered = Vec::with_capacity(atoms.len());
    for atom in atoms {
        let hir_atom = hir::lower_atom_to_hir_with_env(atom, Some(&env));
        verification::verify_with_verification_config(
            &hir_atom,
            proof_work,
            &env,
            &verification::VerificationConfig {
                // Composite v19 admits the IEEE 754 `f64` operators in
                // source contracts, so the contract must verify in the
                // same encoding — the default exact-rational `Real`
                // model would prove clauses a NaN input refutes (e.g.
                // `ensures: result == x + y`).
                ieee754_f64: admission.admits_f64_arithmetic(),
                ..Default::default()
            },
        )
        .map_err(|_| BundleError("source verification failed"))?;
        env.mark_verified(&atom.name);
        lowered.push(hir_atom);
    }
    let results: HashMap<String, (String, String)> = atoms
        .iter()
        .map(|atom| {
            (
                atom.name.clone(),
                (
                    proof_cert::status::Z3_UNSAT.to_owned(),
                    proof_cert::status::VERIFIED.to_owned(),
                ),
            )
        })
        .collect();
    let atom_refs: Vec<&parser::Atom> = atoms.iter().collect();
    let cert = proof_cert::generate_certificate(
        SOURCE_LABEL,
        &atom_refs,
        &results,
        &env,
        None,
        None,
        None,
    );
    write(
        &stage.join(BundleFile::Certificate.file_name()),
        &serde_json::to_vec_pretty(&cert)
            .map_err(|_| BundleError("certificate encoding failed"))?,
    )?;
    let ir = stage.join(BundleFile::LlvmIr.file_name());
    let object = stage.join(BundleFile::Object.file_name());
    mumei_emit_llvm::binary::compile_atoms_to_binary_ll(&lowered, &env, &[], &ir)
        .map_err(|_| BundleError("native IR emission failed"))?;
    let object_build = mumei_emit_llvm::codegen::compile_llvm_ir_to_object_recorded(&ir, &object)
        .map_err(|_| BundleError("native object emission failed"))?;
    let mut link_build = link_recorded_bundle_object(
        &object,
        stage,
        &stage.join(BundleFile::Executable.file_name()),
    )?;
    link_build.expected_exit_status =
        observed_exit_status(&stage.join(BundleFile::Executable.file_name()));
    link_build.expected_stdout_sha256 =
        observed_stdout(&stage.join(BundleFile::Executable.file_name()));
    link_build.expected_stderr_sha256 =
        observed_stderr(&stage.join(BundleFile::Executable.file_name()));
    let has_observation_pin = link_build.expected_exit_status.is_some()
        || link_build.expected_stdout_sha256.is_some()
        || link_build.expected_stderr_sha256.is_some();
    link_build.observation_envelope = has_observation_pin.then(pinned_observation_envelope);
    Ok((components(stage)?, object_build, link_build))
}

/// The record profile a module-family bundle pins for the source profile
/// its gate derived: `[v1, .., v30]` for the wrapping, refined and
/// composite v1–v29 fragments. Each recheck re-derives under exactly
/// that version's admission, never a wider one.
fn record_profile(source_profile: &str, versions: [&'static str; 30]) -> &'static str {
    let [v1, v2, v3, v4, v5, v6, v7, v8, v9, v10, v11, v12, v13, v14, v15, v16, v17, v18, v19, v20, v21, v22, v23, v24, v25, v26, v27, v28, v29, v30] =
        versions;
    match source_profile {
        semantic_contract::PURE_I64_COMPOSITE_PROFILE_V29 => v30,
        semantic_contract::PURE_I64_COMPOSITE_PROFILE_V26 => v29,
        semantic_contract::PURE_I64_COMPOSITE_PROFILE_V25 => v28,
        semantic_contract::PURE_I64_COMPOSITE_PROFILE_V24 => v27,
        semantic_contract::PURE_I64_COMPOSITE_PROFILE_V23 => v26,
        semantic_contract::PURE_I64_COMPOSITE_PROFILE_V22 => v25,
        semantic_contract::PURE_I64_COMPOSITE_PROFILE_V21 => v24,
        semantic_contract::PURE_I64_COMPOSITE_PROFILE_V20 => v23,
        semantic_contract::PURE_I64_COMPOSITE_PROFILE_V19 => v22,
        semantic_contract::PURE_I64_COMPOSITE_PROFILE_V18 => v21,
        semantic_contract::PURE_I64_COMPOSITE_PROFILE_V17 => v20,
        semantic_contract::PURE_I64_COMPOSITE_PROFILE_V16 => v19,
        semantic_contract::PURE_I64_COMPOSITE_PROFILE_V14 => v17,
        semantic_contract::PURE_I64_COMPOSITE_PROFILE_V15 => v18,
        semantic_contract::PURE_I64_COMPOSITE_PROFILE_V13 => v16,
        semantic_contract::PURE_I64_COMPOSITE_PROFILE_V11 => v15,
        semantic_contract::PURE_I64_COMPOSITE_MEMORY_PROFILE => v14,
        semantic_contract::PURE_I64_COMPOSITE_PROFILE_V10 => v13,
        semantic_contract::PURE_I64_COMPOSITE_PROFILE_V9 => v12,
        semantic_contract::PURE_I64_COMPOSITE_PROFILE_V8 => v11,
        semantic_contract::PURE_I64_COMPOSITE_PROFILE_V7 => v10,
        semantic_contract::PURE_I64_COMPOSITE_PROFILE_V6 => v9,
        semantic_contract::PURE_I64_COMPOSITE_PROFILE_V5 => v8,
        semantic_contract::PURE_I64_COMPOSITE_PROFILE_V4 => v7,
        semantic_contract::PURE_I64_COMPOSITE_PROFILE_V3 => v6,
        semantic_contract::PURE_I64_COMPOSITE_PROFILE_V2 => v5,
        semantic_contract::PURE_I64_COMPOSITE_PROFILE => v4,
        semantic_contract::PURE_I64_REFINED_PROFILE_V2 => v3,
        semantic_contract::PURE_I64_REFINED_PROFILE => v2,
        _ => v1,
    }
}

/// The `leaf_widths` pin a v24+ record must carry: the declared leaf
/// widths re-derived under the producer's own admission when the source
/// profile declares `i128`/`u128` leaves; empty on every older profile
/// so the field stays absent from the serialized record.
fn leaf_widths_for_record(
    source_profile: &str,
    source: &str,
    admission: RefinedAdmission,
) -> Result<BTreeMap<String, Vec<u64>>, BundleError> {
    if matches!(
        source_profile,
        semantic_contract::PURE_I64_COMPOSITE_PROFILE_V21
            | semantic_contract::PURE_I64_COMPOSITE_PROFILE_V22
            | semantic_contract::PURE_I64_COMPOSITE_PROFILE_V23
            | semantic_contract::PURE_I64_COMPOSITE_PROFILE_V24
            | semantic_contract::PURE_I64_COMPOSITE_PROFILE_V25
            | semantic_contract::PURE_I64_COMPOSITE_PROFILE_V26
            | semantic_contract::PURE_I64_COMPOSITE_PROFILE_V29
    ) {
        semantic_contract::module_leaf_widths(source, admission)
            .map_err(|_| BundleError("source outside bounded module profile"))
    } else {
        Ok(BTreeMap::new())
    }
}

fn ensure_module_loop_exit_domains(
    source: &str,
    admission: RefinedAdmission,
) -> Result<(), BundleError> {
    let mut config = z3::Config::new();
    config.set_timeout_msec(BuildSettings::default().solver_timeout_ms);
    config.set_param_value("rlimit", &RECHECK_RESOURCE_LIMIT.to_string());
    let context = z3::Context::new(&config);
    let Ok(atoms) =
        semantic_contract::encode_pure_i64_module_obligations(source, &context, admission)
    else {
        return Ok(());
    };
    let solver = z3::Solver::new(&context);
    if mumei_core::native_bundle::check_module_loop_exit_domains(&atoms, &solver)? {
        return Err(BundleError("source domain and loop exits inconclusive"));
    }
    Ok(())
}

fn build_module(source: &str, input: &Path, output: &Path) -> Result<String, BundleError> {
    // The producer always runs the widest (composite) admission; the
    // record's `profile` names a wider version only when the source
    // actually declares what needs it, so a plain module keeps pinning v1.
    let admission = RefinedAdmission::CompositeV29;
    let (binding, atoms, types, structs, enums) = prepare_module_source(source, admission)?;
    ensure_module_loop_exit_domains(source, admission)?;
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let stage = tempfile::Builder::new()
        .prefix(".mumei-native-")
        .tempdir_in(parent)
        .map_err(|_| BundleError("cannot stage bundle"))?;
    let proof_work = tempfile::Builder::new()
        .prefix(".mumei-proof-")
        .tempdir_in(parent)
        .map_err(|_| BundleError("cannot stage proof"))?;
    let (files, object_build, link_build) = emit_module(
        &atoms,
        &types,
        &structs,
        &enums,
        stage.path(),
        proof_work.path(),
        admission,
    )?;
    let settings = BuildSettings::expected_for(&link_build);
    let record = ModuleNativeBundleRecord {
        schema: MODULE_SCHEMA.into(),
        profile: record_profile(
            &binding.source_profile,
            [
                MODULE_PROFILE,
                MODULE_PROFILE_V2,
                MODULE_PROFILE_V3,
                MODULE_PROFILE_V4,
                MODULE_PROFILE_V5,
                MODULE_PROFILE_V6,
                MODULE_PROFILE_V7,
                MODULE_PROFILE_V8,
                MODULE_PROFILE_V9,
                MODULE_PROFILE_V10,
                MODULE_PROFILE_V11,
                MODULE_PROFILE_V12,
                MODULE_PROFILE_V13,
                MODULE_PROFILE_V14,
                MODULE_PROFILE_V15,
                MODULE_PROFILE_V16,
                MODULE_PROFILE_V17,
                MODULE_PROFILE_V18,
                MODULE_PROFILE_V19,
                MODULE_PROFILE_V20,
                MODULE_PROFILE_V21,
                MODULE_PROFILE_V22,
                MODULE_PROFILE_V23,
                MODULE_PROFILE_V24,
                MODULE_PROFILE_V25,
                MODULE_PROFILE_V26,
                MODULE_PROFILE_V27,
                MODULE_PROFILE_V28,
                MODULE_PROFILE_V29,
                MODULE_PROFILE_V30,
            ],
        )
        .into(),
        scope: MODULE_SCOPE.into(),
        system_scope: SYSTEM_SCOPE.into(),
        source: binding.clone(),
        object_build,
        link_build,
        settings,
        files: files
            .iter()
            .map(|(role, bytes)| (*role, ContentIdentity::of(bytes)))
            .collect(),
        leaf_widths: leaf_widths_for_record(&binding.source_profile, source, admission)?,
    };
    let bytes =
        serde_json::to_vec_pretty(&record).map_err(|_| BundleError("record encoding failed"))?;
    let id = sha256(&bytes);
    check_module_identity(&bytes, &id, source, &files)?;
    if source_text(input)? != source {
        return Err(BundleError("source changed during build"));
    }
    write(&stage.path().join(RECORD_FILE), &bytes)?;
    publish(stage.path(), output)?;
    let _ = stage.keep();
    Ok(id)
}

/// Build a multifile bundle: read the root file's transitive imports
/// through the language's own resolution chain — file-relative to the
/// importing member's directory first, then the `std/` fallbacks —
/// merge the closure into the canonical text the module pipeline
/// compiles, and pin every original member under `sources/`. Members
/// may live in subdirectories of the root's directory
/// (`import "sub/dep"`) or come from the compiler-shipped /
/// MUMEI_STD_PATH stdlib (`import "std/option"`, pinned under its
/// importer-prefixed member key); `..` and absolute paths stay outside
/// the member grammar.
fn build_multifile(source: &str, input: &Path, output: &Path) -> Result<String, BundleError> {
    if source.len() as u64 > SOURCE_BYTES {
        return Err(BundleError("unsupported component type or size"));
    }
    let root = member_name(
        input
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(BundleError("invalid source member name"))?,
    )?;
    let directory = input
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    // Collect the transitive import closure: each member's own imports
    // resolve file-relative to the IMPORTING member's directory —
    // through the language's own `resolver::resolve_path`, so the
    // producer shares the full resolution chain (importer-relative
    // first, then the `std/` fallbacks for shipped or MUMEI_STD_PATH
    // modules) instead of duplicating it — bounded by
    // MAX_SOURCE_FILES. Visited members are never requeued, so cycles
    // cannot loop here — merge_import_closure rejects them honestly
    // afterward.
    let mut members = BTreeMap::from([(root.clone(), source.to_owned())]);
    // The path each member's bytes were read from — needed by the
    // importer-directory step below and again by the post-build drift
    // check, since a fallback member's bytes can live outside the
    // root's directory entirely.
    // The root's recorded path is `input` AS GIVEN, not canonicalized:
    // the pipeline resolves a root's imports beside the caller-named
    // path (src/pipeline.rs), so canonicalizing here would resolve a
    // symlinked root's neighbours beside the target instead.
    let mut resolved_paths =
        BTreeMap::<String, PathBuf>::from([(root.clone(), input.to_path_buf())]);
    let mut queue = std::collections::VecDeque::from([root.clone()]);
    while let Some(current) = queue.pop_front() {
        let text = members[&current].clone();
        // Imports resolve relative to the DIRECTORY THE MEMBER'S BYTES
        // WERE ACTUALLY READ FROM — not the bundle-key namespace — so a
        // member pulled in via a fallback keeps resolving its own
        // imports beside its real file (the language's resolver does
        // the same for the resolved file's parent).
        let importer_dir = resolved_paths
            .get(&current)
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| directory.to_path_buf());
        for item in parser::parse_module(&text) {
            let parser::Item::Import(decl) = item else {
                continue;
            };
            let member = resolve_member(&current, &decl.path)?;
            let resolved = resolver::resolve_path(&decl.path, &importer_dir)
                .map_err(|_| BundleError("cannot read source members"))?;
            if let Some(existing) = resolved_paths.get(&member) {
                // `resolved` comes canonicalized out of resolve_path;
                // compare on the same footing — the root's recorded
                // path stays as-given for importer_dir (see above), so
                // canonicalize lazily here instead.
                if existing.canonicalize().unwrap_or_else(|_| existing.clone()) == resolved {
                    continue;
                }
                // The member namespace derives keys from the bundle-root
                // tree, but fallback members resolve imports beside
                // their REAL files — so one key can name two different
                // files (e.g. the root's `import "std/helper"` picks the
                // project file while a fallback member's `import
                // "helper"` picks its stdlib neighbour). The language
                // binds them to different modules; the bundle cannot
                // express that, so reject honestly rather than bundle
                // the wrong implementation.
                return Err(BundleError("import resolution collides outside profile"));
            }
            if members.len() >= MAX_SOURCE_FILES {
                return Err(BundleError("too many source members"));
            }
            members.insert(member.clone(), bounded_source_text(&resolved)?);
            resolved_paths.insert(member.clone(), resolved);
            queue.push_back(member);
        }
    }
    let admission = RefinedAdmission::CompositeV29;
    let prepared = prepare_multifile_source(&root, &members, admission)?;
    ensure_module_loop_exit_domains(&prepared.merged, admission)?;
    let (binding, graph, atoms, types, structs, enums) = (
        prepared.binding,
        prepared.import_graph_sha256,
        prepared.atoms,
        prepared.types,
        prepared.structs,
        prepared.enums,
    );
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let stage = tempfile::Builder::new()
        .prefix(".mumei-native-")
        .tempdir_in(parent)
        .map_err(|_| BundleError("cannot stage bundle"))?;
    let proof_work = tempfile::Builder::new()
        .prefix(".mumei-proof-")
        .tempdir_in(parent)
        .map_err(|_| BundleError("cannot stage proof"))?;
    let (files, object_build, link_build) = emit_module(
        &atoms,
        &types,
        &structs,
        &enums,
        stage.path(),
        proof_work.path(),
        admission,
    )?;
    let settings = BuildSettings::expected_for(&link_build);
    let sources_dir = stage.path().join(SOURCES_DIR);
    fs::create_dir(&sources_dir).map_err(|_| BundleError("cannot stage source members"))?;
    for (name, text) in &members {
        let member_path = sources_dir.join(name);
        if let Some(parent) = member_path.parent() {
            fs::create_dir_all(parent).map_err(|_| BundleError("cannot stage source members"))?;
        }
        write(&member_path, text.as_bytes())?;
    }
    let record = MultifileNativeBundleRecord {
        schema: MULTIFILE_SCHEMA.into(),
        profile: record_profile(
            &binding.source_profile,
            [
                MULTIFILE_PROFILE,
                MULTIFILE_PROFILE_V2,
                MULTIFILE_PROFILE_V3,
                MULTIFILE_PROFILE_V4,
                MULTIFILE_PROFILE_V5,
                MULTIFILE_PROFILE_V6,
                MULTIFILE_PROFILE_V7,
                MULTIFILE_PROFILE_V8,
                MULTIFILE_PROFILE_V9,
                MULTIFILE_PROFILE_V10,
                MULTIFILE_PROFILE_V11,
                MULTIFILE_PROFILE_V12,
                MULTIFILE_PROFILE_V13,
                MULTIFILE_PROFILE_V14,
                MULTIFILE_PROFILE_V15,
                MULTIFILE_PROFILE_V16,
                MULTIFILE_PROFILE_V17,
                MULTIFILE_PROFILE_V18,
                MULTIFILE_PROFILE_V19,
                MULTIFILE_PROFILE_V20,
                MULTIFILE_PROFILE_V21,
                MULTIFILE_PROFILE_V22,
                MULTIFILE_PROFILE_V23,
                MULTIFILE_PROFILE_V24,
                MULTIFILE_PROFILE_V25,
                MULTIFILE_PROFILE_V26,
                MULTIFILE_PROFILE_V27,
                MULTIFILE_PROFILE_V28,
                MULTIFILE_PROFILE_V29,
                MULTIFILE_PROFILE_V30,
            ],
        )
        .into(),
        scope: MODULE_SCOPE.into(),
        system_scope: SYSTEM_SCOPE.into(),
        source: binding.clone(),
        source_files: SourceFilesPin {
            root: root.clone(),
            import_graph_sha256: graph,
        },
        object_build,
        link_build,
        settings,
        files: files
            .iter()
            .map(|(role, bytes)| (*role, ContentIdentity::of(bytes)))
            .collect(),
        sources: members
            .iter()
            .map(|(name, text)| (name.clone(), ContentIdentity::of(text.as_bytes())))
            .collect(),
        leaf_widths: leaf_widths_for_record(&binding.source_profile, &prepared.merged, admission)?,
    };
    let bytes =
        serde_json::to_vec_pretty(&record).map_err(|_| BundleError("record encoding failed"))?;
    let id = sha256(&bytes);
    check_multifile_identity(&bytes, &id, &root, &members, &files)?;
    if bounded_source_text(input)? != *source {
        return Err(BundleError("source changed during build"));
    }
    for member in members.keys() {
        if member == &root {
            continue;
        }
        if bounded_source_text(&resolved_paths[member])? != members[member] {
            return Err(BundleError("source changed during build"));
        }
    }
    write(&stage.path().join(RECORD_FILE), &bytes)?;
    publish(stage.path(), output)?;
    let _ = stage.keep();
    Ok(id)
}

fn check(bundle: &Path, source: &Path, expected_id: &str) -> Result<&'static str, BundleError> {
    let record = read_regular(&bundle.join(RECORD_FILE), 1024 * 1024)?;
    match record_family(&record)? {
        RecordFamily::Multifile => {
            load_pinned_multifile_record(&record, expected_id)?;
            let (members, root) = multifile_inputs(bundle, source)?;
            check_multifile_identity(&record, expected_id, &root, &members, &components(bundle)?)?;
            Ok(MODULE_SCOPE)
        }
        RecordFamily::Module => {
            load_pinned_module_record(&record, expected_id)?;
            check_module_identity(
                &record,
                expected_id,
                &source_text(source)?,
                &components(bundle)?,
            )?;
            Ok(MODULE_SCOPE)
        }
        RecordFamily::Single => {
            load_pinned_record(&record, expected_id)?;
            check_identity(
                &record,
                expected_id,
                &source_text(source)?,
                &components(bundle)?,
            )?;
            Ok(SCOPE)
        }
    }
}

fn recheck(
    bundle: &Path,
    source: &Path,
    expected_id: &str,
) -> Result<(SourceRecheck, &'static str), BundleError> {
    let record = read_regular(&bundle.join(RECORD_FILE), 1024 * 1024)?;
    match record_family(&record)? {
        RecordFamily::Multifile => {
            let (members, root) = multifile_inputs(bundle, source)?;
            return recheck_multifile_source(
                &record,
                expected_id,
                &root,
                &members,
                &components(bundle)?,
            )
            .map(|result| (result, MULTIFILE_RECHECK_PROFILE));
        }
        RecordFamily::Module => {
            return recheck_module_source(
                &record,
                expected_id,
                &bounded_source_text(source)?,
                &components(bundle)?,
            )
            .map(|result| (result, MODULE_RECHECK_PROFILE));
        }
        RecordFamily::Single => (),
    }
    let source = bounded_source_text(source)?;
    load_pinned_record(&record, expected_id)?;
    recheck_source(&record, expected_id, &source, &components(bundle)?)
        .map(|result| (result, RECHECK_PROFILE))
}

fn recheck_llvm(
    bundle: &Path,
    source: &Path,
    expected_id: &str,
) -> Result<LlvmRecheck, BundleError> {
    recheck_llvm_loaded(bundle, source, expected_id).map(|(result, _, _)| result)
}

/// `recheck-llvm` plus the pinned bundle snapshot its checks already
/// verified — the record's toolchain observations and the once-read
/// component bytes — so downstream stages compare against the same
/// snapshot instead of re-reading files that could change in between.
fn recheck_llvm_loaded(
    bundle: &Path,
    source: &Path,
    expected_id: &str,
) -> Result<(LlvmRecheck, LoadedBundle, RecordFamily), BundleError> {
    let bytes = read_regular(&bundle.join(RECORD_FILE), 1024 * 1024)?;
    let files = components(bundle)?;
    if record_family(&bytes)? == RecordFamily::Multifile {
        let record = load_pinned_multifile_record(&bytes, expected_id)?;
        let mut loaded = LoadedBundle {
            object_build: record.object_build.clone(),
            link_build: record.link_build.clone(),
            files,
            source_text: None,
            record_profile: Some(record.profile.clone()),
        };
        let (members, root) = multifile_inputs(bundle, source)?;
        match recheck_multifile_source(&bytes, expected_id, &root, &members, &loaded.files)? {
            SourceRecheck::Refuted => return Err(BundleError("source postcondition refuted")),
            SourceRecheck::Inconclusive => {
                return Ok((LlvmRecheck::Inconclusive, loaded, RecordFamily::Multifile));
            }
            SourceRecheck::RecheckedWithinProfile => (),
        }
        let merged = merge_import_closure(
            &root,
            &members,
            multifile_refined_admission(&record.profile)?,
        )
        .map_err(|_| BundleError("import closure merge failed"))?
        .0;
        loaded.source_text = Some(merged.clone());
        let llvm_ir = std::str::from_utf8(&loaded.files[&BundleFile::LlvmIr])
            .map_err(|_| BundleError("native LLVM snapshot is not UTF-8"))?;
        let report = tv::validate_native_module(
            &merged,
            llvm_ir,
            Options::default(),
            multifile_refined_admission(&record.profile)?,
        )
        .map_err(|_| BundleError("native LLVM snapshot outside bounded module profile"))?;
        if loaded.object_build.target_triple != report.target
            || loaded.object_build.data_layout != report.data_layout
        {
            return Err(BundleError(
                "native LLVM snapshot/toolchain observation mismatch",
            ));
        }
        return Ok((
            match report.outcome {
                Outcome::EquivalentWithinProfile => LlvmRecheck::MultifileRechecked(
                    MULTIFILE_RECHECK_PROFILE,
                    Box::new(evidence::from_multifile_recheck(
                        expected_id,
                        &record,
                        &report,
                        &merged,
                    )?),
                ),
                Outcome::Different | Outcome::UndefinedBehavior => LlvmRecheck::Different,
                Outcome::Inconclusive => LlvmRecheck::Inconclusive,
            },
            loaded,
            RecordFamily::Multifile,
        ));
    }
    let source = bounded_source_text(source)?;
    if record_family(&bytes)? == RecordFamily::Module {
        let record = load_pinned_module_record(&bytes, expected_id)?;
        let loaded = LoadedBundle {
            object_build: record.object_build.clone(),
            link_build: record.link_build.clone(),
            files,
            source_text: Some(source.clone()),
            record_profile: Some(record.profile.clone()),
        };
        match recheck_module_source(&bytes, expected_id, &source, &loaded.files)? {
            SourceRecheck::Refuted => return Err(BundleError("source postcondition refuted")),
            SourceRecheck::Inconclusive => {
                return Ok((LlvmRecheck::Inconclusive, loaded, RecordFamily::Module));
            }
            SourceRecheck::RecheckedWithinProfile => (),
        }
        let llvm_ir = std::str::from_utf8(&loaded.files[&BundleFile::LlvmIr])
            .map_err(|_| BundleError("native LLVM snapshot is not UTF-8"))?;
        let report = tv::validate_native_module(
            &source,
            llvm_ir,
            Options::default(),
            module_refined_admission(&record.profile)?,
        )
        .map_err(|_| BundleError("native LLVM snapshot outside bounded module profile"))?;
        if loaded.object_build.target_triple != report.target
            || loaded.object_build.data_layout != report.data_layout
        {
            return Err(BundleError(
                "native LLVM snapshot/toolchain observation mismatch",
            ));
        }
        return Ok((
            match report.outcome {
                Outcome::EquivalentWithinProfile => LlvmRecheck::ModuleRechecked(
                    report.profile,
                    Box::new(evidence::from_module_recheck(
                        expected_id,
                        &record,
                        &report,
                        &source,
                    )?),
                ),
                Outcome::Different | Outcome::UndefinedBehavior => LlvmRecheck::Different,
                Outcome::Inconclusive => LlvmRecheck::Inconclusive,
            },
            loaded,
            RecordFamily::Module,
        ));
    }
    let record = load_pinned_record(&bytes, expected_id)?;
    let loaded = LoadedBundle {
        object_build: record.object_build.clone(),
        link_build: record.link_build.clone(),
        files,
        source_text: Some(source.clone()),
        record_profile: None,
    };
    match recheck_source(&bytes, expected_id, &source, &loaded.files)? {
        SourceRecheck::Refuted => return Err(BundleError("source postcondition refuted")),
        SourceRecheck::Inconclusive => {
            return Ok((LlvmRecheck::Inconclusive, loaded, RecordFamily::Single));
        }
        SourceRecheck::RecheckedWithinProfile => (),
    }
    let llvm_ir = std::str::from_utf8(&loaded.files[&BundleFile::LlvmIr])
        .map_err(|_| BundleError("native LLVM snapshot is not UTF-8"))?;
    let report = tv::validate_native_main(&source, llvm_ir, Options::default())
        .map_err(|_| BundleError("native LLVM snapshot outside bounded translation profile"))?;
    if loaded.object_build.target_triple != report.target
        || loaded.object_build.data_layout != report.data_layout
    {
        return Err(BundleError(
            "native LLVM snapshot/toolchain observation mismatch",
        ));
    }
    Ok((
        match report.outcome {
            Outcome::EquivalentWithinProfile => LlvmRecheck::Rechecked(Box::new(
                evidence::from_recheck(expected_id, &record, &report)?,
            )),
            Outcome::Different | Outcome::UndefinedBehavior => LlvmRecheck::Different,
            Outcome::Inconclusive => LlvmRecheck::Inconclusive,
        },
        loaded,
        RecordFamily::Single,
    ))
}

fn recheck_evidence(
    bundle: &Path,
    source: &Path,
    evidence_path: &Path,
    expected_id: &str,
) -> Result<Outcome, BundleError> {
    recheck_evidence_bytes(
        bundle,
        source,
        &read_regular(evidence_path, MAX_EVIDENCE_BYTES)?,
        expected_id,
    )
}

/// Same recheck as `recheck_evidence` but over already-read record bytes:
/// callers that fingerprint the descriptor to the record must hash exactly
/// the bytes the recheck decoded, not a second read of the file.
fn recheck_evidence_bytes(
    bundle: &Path,
    source: &Path,
    evidence_bytes: &[u8],
    expected_id: &str,
) -> Result<Outcome, BundleError> {
    if !is_digest(expected_id) {
        return Err(BundleError("invalid expected build ID"));
    }
    let saved = evidence::decode(evidence_bytes)?;
    compare_evidence(recheck_llvm(bundle, source, expected_id)?, &saved)
}

fn compare_evidence(
    result: LlvmRecheck,
    saved: &evidence::ConsumerEvidence,
) -> Result<Outcome, BundleError> {
    match (result, saved) {
        (LlvmRecheck::Rechecked(fresh), evidence::ConsumerEvidence::Single(saved))
            if *fresh == **saved =>
        {
            Ok(Outcome::EquivalentWithinProfile)
        }
        (LlvmRecheck::ModuleRechecked(_, fresh), evidence::ConsumerEvidence::Module(saved))
            if *fresh == *saved =>
        {
            Ok(Outcome::EquivalentWithinProfile)
        }
        (
            LlvmRecheck::MultifileRechecked(_, fresh),
            evidence::ConsumerEvidence::Multifile(saved),
        ) if *fresh == *saved => Ok(Outcome::EquivalentWithinProfile),
        (LlvmRecheck::Rechecked(_), evidence::ConsumerEvidence::Single(_))
        | (LlvmRecheck::ModuleRechecked(..), evidence::ConsumerEvidence::Module(_))
        | (LlvmRecheck::MultifileRechecked(..), evidence::ConsumerEvidence::Multifile(_)) => Err(
            BundleError("saved native evidence differs from fresh consumer report"),
        ),
        (
            LlvmRecheck::Rechecked(_)
            | LlvmRecheck::ModuleRechecked(..)
            | LlvmRecheck::MultifileRechecked(..),
            _,
        ) => Err(BundleError(
            "saved native evidence belongs to the other bundle family",
        )),
        (LlvmRecheck::Different, _) => Ok(Outcome::Different),
        (LlvmRecheck::Inconclusive, _) => Ok(Outcome::Inconclusive),
    }
}

/// The full consumer chain plus toolchain determinism: after the same
/// identity, source and LLVM-snapshot rechecks `recheck-llvm` runs, the
/// pinned `program.ll` is re-emitted to an object and that object —
/// proven byte-identical to the pinned `program.o` — is re-linked into
/// an executable, each under the recorded toolchain. The produced bytes must equal the pinned artifacts and the
/// observed toolchain fields must match the record — a bundle whose
/// object or executable is not what this toolchain emits from its pinned
/// inputs is `Different`.
///
/// `link_build.arguments_sha256` is deliberately not compared: it pins
/// the build-time argument vector including absolute staging paths, so
/// it is a build observation no recheck can reproduce. The optional
/// `canonical_arguments_sha256` pins the same vector with those paths
/// replaced by stable placeholders — re-derived here and compared when
/// the record carries it. Executing either artifact, checking OS exit
/// status and ABI/runtime semantics stay out of scope.
/// What the artifact stage established — the outcome, the record
/// family, whether the canonical argv comparison actually ran, the
/// record's pinned exit status and stdout digest, and the staging
/// directory (kept alive so `recheck-exit-status`/`recheck-stdout` can
/// run the reproduced executable).
struct ArtifactRecheckResult {
    result: ArtifactRecheck,
    family: RecordFamily,
    canonical_argv_compared: bool,
    canonical_argv_hash: Option<String>,
    record_profile: Option<String>,
    pin_envelope: &'static str,
    expected_exit_status: Option<u32>,
    expected_stdout_sha256: Option<String>,
    expected_stderr_sha256: Option<String>,
    stage: Option<tempfile::TempDir>,
    source_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ObjectSymbolSurface {
    defined: BTreeSet<String>,
    undefined: BTreeSet<String>,
    calls: BTreeMap<String, BTreeSet<String>>,
    call_targets: BTreeMap<String, BTreeMap<u64, machine_tv::CallTarget>>,
    call_edges_checkable: bool,
}

struct ObjectRecheckData {
    stage: tempfile::TempDir,
    family: RecordFamily,
    canonical_argv_hash: Option<String>,
    admission: RefinedAdmission,
    ir_surface: tv::IrFunctionSurface,
    object_surface: ObjectSymbolSurface,
    object_link_correspondence: bool,
    llvm_bytes: Vec<u8>,
    object_bytes: Vec<u8>,
    executable_bytes: Vec<u8>,
    object_translation: Option<ObjectTranslationRecord>,
    resolved_call_sites: usize,
    source_text: Option<String>,
}

enum ObjectRecheckPipeline {
    Rechecked(Box<ObjectRecheckData>),
    Different(&'static str),
    Inconclusive(&'static str),
}

struct ObjectFunctionRange {
    name: String,
    section: object::SectionIndex,
    start: u64,
    end: u64,
}

struct ObjectLinkFunction {
    name: String,
    section: object::SectionIndex,
    section_name: String,
    object_address: u64,
    object_size: u64,
    executable_address: u64,
    object_bytes: Vec<u8>,
    executable_bytes: Vec<u8>,
}

struct ObjectLinkRelocation {
    // `None` for relocations outside any enumerated function (e.g.
    // `.eh_frame` unwind records), which the byte-level link check
    // skips but the whole-relocation record still lists.
    function_index: Option<usize>,
    section_name: String,
    offset_in_section: u64,
    offset: usize,
    kind: String,
    target: String,
    // Section the target symbol belongs to, when the relocation
    // references a section symbol (e.g. `.text + 0x20`); the exe-side
    // base of that input section plus `target_value` gives S.
    target_section: Option<object::SectionIndex>,
    target_value: u64,
    addend: i64,
    resolved_value: Option<i64>,
    // Offset inside the same-named executable section where the
    // applied field was located, for non-Text entries resolved by
    // the slot search. `None` until a unique slot is claimed.
    applied_offset: Option<u64>,
    // `assumed_external_callee` when the target is a loader import —
    // the relocation's runtime binding stays an assumption in the
    // same `assumed_*` category machine-TV uses for external callees
    // (it claims resolution-by-loader only, not register preservation,
    // so the longer callee-saved name is deliberately not reused).
    assumption: Option<&'static str>,
}

/// Report name for a relocation entry's ELF kind. PC32/PLT32 get
/// their symbolic names; other ELF types keep their numeric `r_type`
/// so the record still identifies the original kind, and non-ELF
/// relocations degrade to `other`.
fn relocation_kind_name(flags: RelocationFlags) -> String {
    match flags {
        RelocationFlags::Elf { r_type } if r_type == object::elf::R_X86_64_PLT32 => {
            "R_X86_64_PLT32".to_owned()
        }
        RelocationFlags::Elf { r_type } if r_type == object::elf::R_X86_64_PC32 => {
            "R_X86_64_PC32".to_owned()
        }
        RelocationFlags::Elf { r_type } => format!("elf:{r_type}"),
        _ => "other".to_owned(),
    }
}

/// Metadata-only record of the LLVM-IR -> object -> linked-executable
/// correspondence, emitted so consumers can inspect the translation
/// without any equivalence being claimed on it (Step 1 of
/// docs/DESIGN_OBJECT_TRANSLATION_CORRESPONDENCE.md).
#[derive(Debug, Clone)]
struct ObjectTranslationRecord {
    functions: Vec<ObjectFunctionRecord>,
    relocations: Vec<ObjectRelocationRecord>,
}

#[derive(Debug, Clone)]
struct ObjectFunctionRecord {
    ir_function: String,
    object_symbol: String,
    object_section: String,
    object_offset: u64,
    object_size: u64,
    executable_address: Option<u64>,
}

#[derive(Debug, Clone)]
struct ObjectRelocationRecord {
    object_section: String,
    object_offset: u64,
    kind: String,
    target_symbol: String,
    addend: i64,
    resolved_value: Option<i64>,
    // Offset inside the same-named executable section where the
    // applied field was verified, for non-code relocations located by
    // the unique-slot search; `None` for code relocations (their site
    // is derived from the owning function) and unresolved entries.
    applied_offset: Option<u64>,
    // Same assumption marker as `ObjectLinkRelocation.assumption`.
    assumption: Option<&'static str>,
}

fn object_translation_record(
    functions: &[ObjectLinkFunction],
    relocations: &[ObjectLinkRelocation],
) -> ObjectTranslationRecord {
    ObjectTranslationRecord {
        functions: functions
            .iter()
            .map(|function| ObjectFunctionRecord {
                ir_function: function.name.clone(),
                object_symbol: function.name.clone(),
                object_section: function.section_name.clone(),
                object_offset: function.object_address,
                object_size: function.object_size,
                executable_address: Some(function.executable_address),
            })
            .collect(),
        relocations: relocations
            .iter()
            .map(|relocation| ObjectRelocationRecord {
                object_section: relocation.section_name.clone(),
                object_offset: relocation.offset_in_section,
                kind: relocation.kind.clone(),
                target_symbol: relocation.target.clone(),
                addend: relocation.addend,
                resolved_value: relocation.resolved_value,
                applied_offset: relocation.applied_offset,
                assumption: relocation.assumption,
            })
            .collect(),
    }
}

fn object_symbol_surface(bytes: &[u8]) -> Result<ObjectSymbolSurface, ()> {
    let file = object::File::parse(bytes).map_err(|_| ())?;
    let mut surface = ObjectSymbolSurface {
        call_edges_checkable: true,
        ..ObjectSymbolSurface::default()
    };
    let mut ranges = Vec::new();
    for symbol in file.symbols() {
        if symbol.is_undefined() {
            let name = symbol.name().map_err(|_| ())?;
            if !name.is_empty() {
                surface.undefined.insert(name.to_owned());
            }
        } else if symbol.is_definition() && symbol.kind() == SymbolKind::Text {
            if !symbol.is_global() {
                surface.call_edges_checkable = false;
                continue;
            }
            let name = symbol.name().map_err(|_| ())?.to_owned();
            surface.defined.insert(name.clone());
            surface.calls.entry(name.clone()).or_default();
            surface.call_targets.entry(name.clone()).or_default();
            let Some(section) = symbol.section_index() else {
                surface.call_edges_checkable = false;
                continue;
            };
            let size = symbol.size();
            let Some(end) = symbol.address().checked_add(size) else {
                surface.call_edges_checkable = false;
                continue;
            };
            if size == 0 {
                surface.call_edges_checkable = false;
                continue;
            }
            ranges.push(ObjectFunctionRange {
                name,
                section,
                start: symbol.address(),
                end,
            });
        }
    }

    for (index, range) in ranges.iter().enumerate() {
        for other in ranges.iter().skip(index + 1) {
            if range.section == other.section && range.start < other.end && other.start < range.end
            {
                surface.call_edges_checkable = false;
            }
        }
    }

    for section in file.sections() {
        if section.kind() != SectionKind::Text {
            continue;
        }
        let section_index = section.index();
        for (offset, relocation) in section.relocations() {
            let mut containing = ranges.iter().filter(|range| {
                range.section == section_index && range.start <= offset && offset < range.end
            });
            let Some(function) = containing.next() else {
                surface.call_edges_checkable = false;
                continue;
            };
            if containing.next().is_some() {
                surface.call_edges_checkable = false;
                continue;
            }
            if relocation.subtractor().is_some() {
                surface.call_edges_checkable = false;
                continue;
            }
            let RelocationTarget::Symbol(symbol_index) = relocation.target() else {
                surface.call_edges_checkable = false;
                continue;
            };
            let Ok(target) = file.symbol_by_index(symbol_index) else {
                surface.call_edges_checkable = false;
                continue;
            };
            if target.kind() == SymbolKind::Section {
                surface.call_edges_checkable = false;
                continue;
            }
            let Ok(name) = target.name() else {
                surface.call_edges_checkable = false;
                continue;
            };
            if !surface.defined.contains(name) && !surface.undefined.contains(name) {
                surface.call_edges_checkable = false;
                continue;
            }
            if let Some(callees) = surface.calls.get_mut(&function.name) {
                callees.insert(name.to_owned());
            } else {
                surface.call_edges_checkable = false;
            }
            let Some(relative_offset) = offset.checked_sub(function.start) else {
                surface.call_edges_checkable = false;
                continue;
            };
            let targets = surface
                .call_targets
                .entry(function.name.clone())
                .or_default();
            let relocation_kind = match relocation.flags() {
                RelocationFlags::Elf { r_type }
                    if r_type == object::elf::R_X86_64_PLT32
                        && relocation.kind() == object::RelocationKind::PltRelative =>
                {
                    machine_tv::CallRelocationKind::ElfX86_64Plt32
                }
                RelocationFlags::Elf { r_type }
                    if r_type == object::elf::R_X86_64_PC32
                        && relocation.kind() == object::RelocationKind::Relative =>
                {
                    machine_tv::CallRelocationKind::ElfX86_64Pc32
                }
                _ => machine_tv::CallRelocationKind::Other,
            };
            if targets
                .insert(
                    relative_offset,
                    machine_tv::CallTarget {
                        name: name.to_owned(),
                        relocation_kind,
                        relocation_size: relocation.size(),
                        addend: relocation.addend(),
                    },
                )
                .is_some()
            {
                surface.call_edges_checkable = false;
            }
        }
    }
    Ok(surface)
}

fn object_symbols_correspond(ir: &tv::IrFunctionSurface, object: &ObjectSymbolSurface) -> bool {
    object.defined == ir.defined
        && object.undefined.iter().all(|name| {
            ir.declared.contains(name)
                || machine_tv::is_ti3_libcall(name)
                || ir.libc_calls.values().any(|callees| callees.contains(name))
        })
}

fn object_call_edges_correspond(
    ir: &tv::IrFunctionSurface,
    object: &ObjectSymbolSurface,
) -> Option<bool> {
    // compiler-rt `__*ti3` calls are injected by the backend for
    // i128/u128 operators the IR spells as plain instructions, so they
    // have no IR call edge; the machine-TV lifter models their effects
    // instead. A ti3 edge that the same caller's IR call set names is a
    // real edge (explicit call or user-defined function), so keep it.
    let mut object_calls = object.calls.clone();
    for (caller, callees) in object_calls.iter_mut() {
        // A ti3 edge survives only when the same caller's IR call set
        // names it — backend-injected calls have no such edge even
        // when another function calls the helper explicitly.
        let ir_callees = ir.calls.get(caller);
        callees.retain(|name| {
            if machine_tv::is_ti3_libcall(name) {
                return ir_callees.is_some_and(|set| set.contains(name));
            }
            // A `llvm.memcpy`-style intrinsic lowers to a libc call the
            // IR never spells — the edge is backend-injected when the
            // caller's intrinsic surface names it.
            if ir
                .libc_calls
                .get(caller)
                .is_some_and(|set| set.contains(name))
            {
                return false;
            }
            true
        });
    }
    (ir.call_edges_checkable && object.call_edges_checkable).then(|| ir.calls == object_calls)
}

fn object_link_corresponds(
    object_bytes: &[u8],
    executable_bytes: &[u8],
) -> Result<Option<(bool, usize, ObjectTranslationRecord)>, ()> {
    let object_file = object::File::parse(object_bytes).map_err(|_| ())?;
    let executable_file = object::File::parse(executable_bytes).map_err(|_| ())?;
    if object_file.format() != BinaryFormat::Elf
        || executable_file.format() != BinaryFormat::Elf
        || object_file.architecture() != Architecture::X86_64
        || executable_file.architecture() != Architecture::X86_64
        || !object_file.is_little_endian()
        || !executable_file.is_little_endian()
    {
        return Ok(None);
    }

    let mut executable_symbols =
        BTreeMap::<String, Vec<(u64, u64, Option<object::SectionIndex>)>>::new();
    for symbol in executable_file.symbols() {
        if symbol.is_global() && symbol.is_definition() && symbol.kind() == SymbolKind::Text {
            if let Ok(name) = symbol.name() {
                executable_symbols
                    .entry(name.to_owned())
                    .or_default()
                    .push((symbol.address(), symbol.size(), symbol.section_index()));
            }
        }
    }

    // Symbols the link leaves to the loader (undefined imports). Calls
    // to them resolve through a PLT stub the linker emits; the Nth
    // R_X86_64_JUMP_SLOT entry in .rela.plt owns the Nth PLT stub —
    // .plt.sec stubs are 16 bytes from the section base, classic .plt
    // stubs follow the 16-byte resolver entry.
    let executable_imports: BTreeSet<String> = executable_file
        .symbols()
        .chain(executable_file.dynamic_symbols())
        .filter(|symbol| symbol.is_global() && !symbol.is_definition())
        .filter_map(|symbol| symbol.name().ok().map(str::to_owned))
        .collect();
    let mut plt_stub_addresses = BTreeMap::<String, u64>::new();
    // Accepted PLT areas for unlocatable-stub fields: every section the
    // linker may place call stubs in (.plt/.plt.sec/.plt.got), kept as
    // separate ranges so inter-section gaps stay rejected.
    let mut plt_ranges = Vec::<(u64, u64)>::new();
    for section in executable_file.sections() {
        if !matches!(section.name(), Ok(".plt") | Ok(".plt.sec") | Ok(".plt.got")) {
            continue;
        }
        let Some(section_end) = section.address().checked_add(section.size()) else {
            continue;
        };
        plt_ranges.push((section.address(), section_end));
    }
    let plt_stub_layout = executable_file
        .sections()
        .find(|section| section.name() == Ok(".plt.sec"))
        .map(|section| (section.address(), 0u64))
        .or_else(|| {
            executable_file
                .sections()
                .find(|section| section.name() == Ok(".plt"))
                .map(|section| (section.address(), 16u64))
        });
    if let Some((plt_base, first_stub_offset)) = plt_stub_layout {
        let dynsym_names: BTreeMap<u32, String> = executable_file
            .dynamic_symbols()
            .filter_map(|symbol| {
                symbol
                    .name()
                    .ok()
                    .map(|name| (symbol.index().0 as u32, name.to_owned()))
            })
            .collect();
        let mut stub_index = 0u64;
        for (_address, relocation) in executable_file.dynamic_relocations().into_iter().flatten() {
            if !matches!(
                relocation.flags(),
                RelocationFlags::Elf { r_type }
                    if r_type == object::elf::R_X86_64_JUMP_SLOT
            ) {
                continue;
            }
            if let RelocationTarget::Symbol(symbol_index) = relocation.target() {
                if let Some(name) = dynsym_names.get(&(symbol_index.0 as u32)) {
                    plt_stub_addresses
                        .entry(name.clone())
                        .or_insert(plt_base + first_stub_offset + stub_index * 16);
                }
            }
            stub_index += 1;
        }
    }

    let mut functions = Vec::new();
    let mut object_defined = BTreeSet::new();
    let mut executable_addresses = BTreeMap::new();
    for symbol in object_file.symbols() {
        if !symbol.is_global() || !symbol.is_definition() || symbol.kind() != SymbolKind::Text {
            continue;
        }
        let Ok(name) = symbol.name() else {
            return Ok(None);
        };
        if name.is_empty() || !object_defined.insert(name.to_owned()) {
            return Ok(None);
        }
        let Some(section_index) = symbol.section_index() else {
            return Ok(None);
        };
        let Ok(object_section) = object_file.section_by_index(section_index) else {
            return Ok(None);
        };
        if object_section.kind() != SectionKind::Text {
            return Ok(None);
        }
        let object_size = symbol.size();
        let Some(object_end) = symbol.address().checked_add(object_size) else {
            return Ok(None);
        };
        let Ok(Some(object_bytes)) = object_section.data_range(symbol.address(), object_size)
        else {
            return Ok(None);
        };
        if object_bytes.len() as u64 != object_size {
            return Ok(None);
        }

        let Some(executable_candidates) = executable_symbols.get(name) else {
            return Ok(None);
        };
        if executable_candidates.len() != 1 {
            return Ok(None);
        }
        let (executable_address, executable_size, executable_section_index) =
            executable_candidates[0];
        let Some(executable_section_index) = executable_section_index else {
            return Ok(None);
        };
        let Ok(executable_section) = executable_file.section_by_index(executable_section_index)
        else {
            return Ok(None);
        };
        if executable_section.kind() != SectionKind::Text {
            return Ok(None);
        }
        let Some(_executable_end) = executable_address.checked_add(executable_size) else {
            return Ok(None);
        };
        let Ok(Some(executable_function_bytes)) =
            executable_section.data_range(executable_address, executable_size)
        else {
            return Ok(None);
        };
        if executable_function_bytes.len() as u64 != executable_size {
            return Ok(None);
        }
        if object_size != executable_size {
            return Ok(Some((false, 0, object_translation_record(&functions, &[]))));
        }

        if functions.iter().any(|function: &ObjectLinkFunction| {
            function.section == section_index
                && function.object_address < object_end
                && symbol.address() < function.object_address + function.object_size
        }) {
            return Ok(None);
        }
        executable_addresses.insert(name.to_owned(), executable_address);
        let section_name = object_section.name().unwrap_or_default().to_owned();
        functions.push(ObjectLinkFunction {
            name: name.to_owned(),
            section: section_index,
            section_name,
            object_address: symbol.address(),
            object_size,
            executable_address,
            object_bytes: object_bytes.to_vec(),
            executable_bytes: executable_function_bytes.to_vec(),
        });
    }

    // Step 4 of docs/DESIGN_OBJECT_TRANSLATION_CORRESPONDENCE.md: every
    // relocation entry is recorded, not only call sites in code. Text
    // relocations keep the strict gates below (the byte-level link
    // check must understand each in-function entry); relocations in
    // non-Text sections (`.eh_frame` unwind metadata, data tables, …)
    // are always recorded and are resolved afterwards against the
    // linked executable when their kind admits recomputation.
    let mut relocations = Vec::new();
    for section in object_file.sections() {
        let section_index = section.index();
        let section_name = section.name().unwrap_or_default().to_owned();
        let is_text = section.kind() == SectionKind::Text;
        for (offset, relocation) in section.relocations() {
            let kind = relocation_kind_name(relocation.flags());
            if !is_text {
                let (target_name, target_section, target_value) = match relocation.target() {
                    RelocationTarget::Symbol(symbol_index) => {
                        match object_file.symbol_by_index(symbol_index) {
                            Ok(symbol) => {
                                let name = symbol.name().unwrap_or_default();
                                if name.is_empty() {
                                    // Section symbols carry no name;
                                    // record the section's name like
                                    // `readelf` displays (`.text + N`).
                                    let section_name = symbol
                                        .section_index()
                                        .and_then(|index| object_file.section_by_index(index).ok())
                                        .and_then(|section| section.name().ok().map(str::to_owned))
                                        .unwrap_or_default();
                                    (section_name, symbol.section_index(), symbol.address())
                                } else {
                                    (name.to_owned(), None, symbol.address())
                                }
                            }
                            Err(_) => (String::new(), None, 0),
                        }
                    }
                    _ => (String::new(), None, 0),
                };
                relocations.push(ObjectLinkRelocation {
                    function_index: None,
                    section_name: section_name.clone(),
                    offset_in_section: offset,
                    offset: usize::MAX,
                    kind,
                    target: target_name.clone(),
                    target_section,
                    target_value,
                    addend: relocation.addend(),
                    resolved_value: None,
                    applied_offset: None,
                    assumption: executable_imports
                        .contains(target_name.as_str())
                        .then_some("assumed_external_callee"),
                });
                continue;
            }
            let mut containing = functions.iter().enumerate().filter(|(_, function)| {
                function.section == section_index
                    && function.object_address <= offset
                    && offset < function.object_address + function.object_size
            });
            let Some((function_index, function)) = containing.next() else {
                return Ok(None);
            };
            if containing.next().is_some() {
                return Ok(None);
            }
            if !matches!(kind.as_str(), "R_X86_64_PLT32" | "R_X86_64_PC32")
                || relocation.size() != 32
            {
                return Ok(None);
            }
            let RelocationTarget::Symbol(symbol_index) = relocation.target() else {
                return Ok(None);
            };
            let Ok(target) = object_file.symbol_by_index(symbol_index) else {
                return Ok(None);
            };
            let Ok(target_name) = target.name() else {
                return Ok(None);
            };
            if !object_defined.contains(target_name) {
                // A compiler-rt `__*ti3` reference stays checkable when
                // the linked executable carries exactly one symbol of
                // that name — the relocation must then resolve to it.
                if machine_tv::is_ti3_libcall(target_name) {
                    let Some(candidates) = executable_symbols.get(target_name) else {
                        return Ok(None);
                    };
                    if candidates.len() != 1 {
                        return Ok(None);
                    }
                    executable_addresses.insert(target_name.to_owned(), candidates[0].0);
                } else if let Some(&stub_address) = plt_stub_addresses.get(target_name) {
                    // Loader imports resolve through the PLT stub the
                    // linker emitted for them — verify the call field
                    // points at that stub.
                    executable_addresses.insert(target_name.to_owned(), stub_address);
                } else if let Some(candidates) = executable_symbols.get(target_name) {
                    // An object-external reference satisfied by a
                    // definition in the linked executable (e.g. another
                    // object on the link line).
                    if candidates.len() != 1 {
                        return Ok(None);
                    }
                    executable_addresses.insert(target_name.to_owned(), candidates[0].0);
                } else if !executable_imports.contains(target_name) {
                    // Neither a known callee nor a loader import — the
                    // target is unverifiable, so the whole check stays
                    // inconclusive.
                    return Ok(None);
                }
            }
            let Some(relative_offset) = offset.checked_sub(function.object_address) else {
                return Ok(None);
            };
            let Ok(relative_offset) = usize::try_from(relative_offset) else {
                return Ok(None);
            };
            let Some(field_end) = relative_offset.checked_add(4) else {
                return Ok(None);
            };
            let Ok(function_size) = usize::try_from(function.object_size) else {
                return Ok(None);
            };
            if field_end > function_size {
                return Ok(None);
            }
            relocations.push(ObjectLinkRelocation {
                function_index: Some(function_index),
                section_name: section_name.clone(),
                offset_in_section: offset,
                offset: relative_offset,
                kind,
                target: target_name.to_owned(),
                target_section: target.section_index(),
                target_value: target.address(),
                addend: relocation.addend(),
                resolved_value: None,
                applied_offset: None,
                assumption: executable_imports
                    .contains(target_name)
                    .then_some("assumed_external_callee"),
            });
        }
    }

    // Step 4: resolve the non-Text entries too, so every enumerated
    // record can be matched against the field actually applied in the
    // linked executable. A PC32/PLT32 entry resolves when the
    // same-named executable section holds a 32-bit field equal to
    // `target + addend − place` at exactly one slot no other entry
    // claims; entries whose formula matches several fields (or none)
    // stay `resolved_value: None` — an ambiguous applied position is
    // reported unchecked rather than verified against a guessed slot.
    let mut section_deltas = BTreeMap::new();
    for function in &functions {
        // Every function of an input section shares one exe-side base:
        // the linker never splits an input section's contents.
        section_deltas.entry(function.section.0).or_insert(
            i64::try_from(function.executable_address).unwrap_or(0)
                - i64::try_from(function.object_address).unwrap_or(0),
        );
    }
    let exe_section_bytes = |name: &str| -> Option<(u64, Vec<u8>)> {
        executable_file
            .sections()
            .find(|section| section.name() == Ok(name))
            .and_then(|section| {
                section
                    .uncompressed_data()
                    .ok()
                    .map(|data| (section.address(), data.into_owned()))
            })
    };
    let target_base = |relocation: &ObjectLinkRelocation| -> Option<i64> {
        if let Some(&address) = executable_addresses.get(&relocation.target) {
            return i64::try_from(address).ok();
        }
        if let Some(candidates) = executable_symbols.get(&relocation.target) {
            if candidates.len() == 1 {
                return i64::try_from(candidates[0].0).ok();
            }
        }
        let section_index = relocation.target_section?;
        if let Some(&delta) = section_deltas.get(&section_index.0) {
            return delta.checked_add(i64::try_from(relocation.target_value).ok()?);
        }
        // A section-symbol target without mapped functions falls back
        // to the same-named output section's base; a wrong guess just
        // leaves the entry unresolved rather than resolving falsely.
        let section_name = object_file
            .section_by_index(section_index)
            .ok()
            .and_then(|section| section.name().ok().map(str::to_owned))?;
        let (base, _) = exe_section_bytes(&section_name)?;
        i64::try_from(base)
            .ok()?
            .checked_add(i64::try_from(relocation.target_value).ok()?)
    };
    let mut pending: Vec<(usize, String, i64, i64, Vec<u64>)> = Vec::new();
    for (index, relocation) in relocations.iter().enumerate() {
        if relocation.function_index.is_some()
            || !matches!(relocation.kind.as_str(), "R_X86_64_PC32" | "R_X86_64_PLT32")
        {
            continue;
        }
        let Some((section_base, section_data)) = exe_section_bytes(&relocation.section_name) else {
            continue;
        };
        let Ok(section_base) = i64::try_from(section_base) else {
            continue;
        };
        let Some(target) =
            target_base(relocation).and_then(|base| base.checked_add(relocation.addend))
        else {
            continue;
        };
        let mut candidates = Vec::new();
        for place in 0..section_data.len().saturating_sub(3) {
            let Ok(field_bytes) = <[u8; 4]>::try_from(&section_data[place..place + 4]) else {
                continue;
            };
            let Ok(place_offset) = i64::try_from(place) else {
                continue;
            };
            if i64::from(i32::from_le_bytes(field_bytes)) == target - (section_base + place_offset)
            {
                candidates.push(place as u64);
            }
        }
        if !candidates.is_empty() {
            pending.push((
                index,
                relocation.section_name.clone(),
                target,
                section_base,
                candidates,
            ));
        }
    }
    // Distinct-slot assignment: the fewest-candidates entries claim
    // first so one applied field cannot verify two records. An entry
    // resolves only when exactly one unclaimed slot matches — a field
    // that coincidentally satisfies the formula elsewhere in the
    // section (e.g. an unrelated `.eh_frame` word) must not stand in
    // for the applied position, so ambiguity stays unresolved.
    pending.sort_by_key(|(_, _, _, _, candidates)| candidates.len());
    let mut taken = BTreeSet::<(String, u64)>::new();
    for (index, section_name, target, section_base, candidates) in pending {
        let mut free = candidates
            .iter()
            .filter(|place| !taken.contains(&(section_name.clone(), **place)));
        let (Some(&place), None) = (free.next(), free.next()) else {
            continue;
        };
        taken.insert((section_name, place));
        relocations[index].resolved_value =
            Some(target - (section_base + i64::try_from(place).unwrap_or(0)));
        relocations[index].applied_offset = Some(place);
    }

    let mut resolved_call_sites = 0;
    for (function_index, function) in functions.iter().enumerate() {
        let mut function_relocations: Vec<usize> = relocations
            .iter()
            .enumerate()
            .filter(|(_, relocation)| relocation.function_index == Some(function_index))
            .map(|(index, _)| index)
            .collect();
        function_relocations.sort_by_key(|index| relocations[*index].offset);
        let mut compared_through = 0;
        for relocation_index in function_relocations {
            let relocation = &relocations[relocation_index];
            if relocation.offset < compared_through {
                return Ok(None);
            }
            if function.object_bytes[compared_through..relocation.offset]
                != function.executable_bytes[compared_through..relocation.offset]
            {
                return Ok(Some((
                    false,
                    resolved_call_sites,
                    object_translation_record(&functions, &relocations),
                )));
            }
            let field_end = relocation.offset + 4;
            let Some(target_address) = executable_addresses.get(&relocation.target).copied() else {
                // A loader import whose PLT stub could not be mapped:
                // the call field must still point inside the PLT
                // region — a field retargeted anywhere else is a real
                // mismatch. When it does, the field bytes are left
                // unverified and the relocation stays unchecked in
                // `relocation_resolved_values`.
                if executable_imports.contains(&relocation.target) {
                    let Some(actual_bytes) =
                        function.executable_bytes.get(relocation.offset..field_end)
                    else {
                        return Ok(None);
                    };
                    let Ok(actual_bytes) = <[u8; 4]>::try_from(actual_bytes) else {
                        return Ok(None);
                    };
                    let actual_value = i64::from(i32::from_le_bytes(actual_bytes));
                    let implied_target = i64::try_from(function.executable_address)
                        .ok()
                        .and_then(|address| {
                            address.checked_add(u64::try_from(relocation.offset).ok()? as i64)
                        })
                        .and_then(|place| {
                            place
                                .checked_add(actual_value)
                                .and_then(|target| target.checked_sub(relocation.addend))
                        });
                    let in_plt = implied_target
                        .and_then(|target| u64::try_from(target).ok())
                        .is_some_and(|target| {
                            plt_ranges
                                .iter()
                                .any(|(start, end)| *start <= target && target < *end)
                        });
                    if !in_plt {
                        return Ok(Some((
                            false,
                            resolved_call_sites,
                            object_translation_record(&functions, &relocations),
                        )));
                    }
                    compared_through = field_end;
                    continue;
                }
                return Ok(None);
            };
            let Ok(relative_offset) = u64::try_from(relocation.offset) else {
                return Ok(None);
            };
            let Some(place_address) = function.executable_address.checked_add(relative_offset)
            else {
                return Ok(Some((
                    false,
                    resolved_call_sites,
                    object_translation_record(&functions, &relocations),
                )));
            };
            let Ok(target_address) = i64::try_from(target_address) else {
                return Ok(Some((
                    false,
                    resolved_call_sites,
                    object_translation_record(&functions, &relocations),
                )));
            };
            let Ok(place_address) = i64::try_from(place_address) else {
                return Ok(Some((
                    false,
                    resolved_call_sites,
                    object_translation_record(&functions, &relocations),
                )));
            };
            let Some(resolved_value) = target_address
                .checked_add(relocation.addend)
                .and_then(|value| value.checked_sub(place_address))
            else {
                return Ok(Some((
                    false,
                    resolved_call_sites,
                    object_translation_record(&functions, &relocations),
                )));
            };
            let Ok(resolved_value) = i32::try_from(resolved_value) else {
                return Ok(Some((
                    false,
                    resolved_call_sites,
                    object_translation_record(&functions, &relocations),
                )));
            };
            let Some(actual_bytes) = function.executable_bytes.get(relocation.offset..field_end)
            else {
                return Ok(None);
            };
            let Ok(actual_bytes) = <[u8; 4]>::try_from(actual_bytes) else {
                return Ok(None);
            };
            let actual_value = i32::from_le_bytes(actual_bytes);
            if actual_value != resolved_value {
                return Ok(Some((
                    false,
                    resolved_call_sites,
                    object_translation_record(&functions, &relocations),
                )));
            }
            relocations[relocation_index].resolved_value = Some(i64::from(resolved_value));
            compared_through = field_end;
            resolved_call_sites += 1;
        }
        if function.object_bytes[compared_through..]
            != function.executable_bytes[compared_through..]
        {
            return Ok(Some((
                false,
                resolved_call_sites,
                object_translation_record(&functions, &relocations),
            )));
        }
    }
    Ok(Some((
        true,
        resolved_call_sites,
        object_translation_record(&functions, &relocations),
    )))
}

fn recheck_object_pipeline(
    bundle: &Path,
    source: &Path,
    expected_id: &str,
    allow_uncheckable_object_link: bool,
) -> Result<ObjectRecheckPipeline, BundleError> {
    let recheck = recheck_artifact(bundle, source, expected_id)?;
    let ArtifactRecheckResult {
        result,
        family,
        canonical_argv_hash,
        record_profile,
        stage,
        source_text,
        ..
    } = recheck;
    match result {
        ArtifactRecheck::Different => {
            return Ok(ObjectRecheckPipeline::Different(
                "pinned artifact differs from the toolchain's output for the pinned inputs",
            ));
        }
        ArtifactRecheck::Inconclusive => {
            return Ok(ObjectRecheckPipeline::Inconclusive(
                "Native source/LLVM recheck inconclusive; no object symbol correspondence established.",
            ));
        }
        ArtifactRecheck::Rechecked => (),
    }
    let admission = machine_tv_admission_for_record(family, record_profile.as_deref())?;
    let Some(stage) = stage else {
        return Ok(ObjectRecheckPipeline::Inconclusive(
            "Native object symbol surface inconclusive; reproduced artifacts are unavailable.",
        ));
    };
    let ir = fs::read(stage.path().join(BundleFile::LlvmIr.file_name()));
    let object_bytes = fs::read(stage.path().join(BundleFile::Object.file_name()));
    let (Ok(ir), Ok(object_bytes)) = (ir, object_bytes) else {
        return Ok(ObjectRecheckPipeline::Inconclusive(
            "Native object symbol surface inconclusive; staged inputs could not be read.",
        ));
    };
    let (Ok(ir_surface), Ok(object_surface)) = (
        tv::ir_function_surface(&ir),
        object_symbol_surface(&object_bytes),
    ) else {
        return Ok(ObjectRecheckPipeline::Inconclusive(
            "Native object symbol surface inconclusive; LLVM IR or object parsing failed.",
        ));
    };
    if !object_symbols_correspond(&ir_surface, &object_surface) {
        return Ok(ObjectRecheckPipeline::Different(
            "native object symbol surface differs from LLVM IR",
        ));
    }
    match object_call_edges_correspond(&ir_surface, &object_surface) {
        None => {
            return Ok(ObjectRecheckPipeline::Inconclusive(
                "Native object call-edge correspondence inconclusive; direct call edges are not checkable.",
            ));
        }
        Some(false) => {
            return Ok(ObjectRecheckPipeline::Different(
                "native object call edges differ from LLVM IR",
            ));
        }
        Some(true) => (),
    }
    let executable = match fs::read(stage.path().join(BundleFile::Executable.file_name())) {
        Ok(executable) => executable,
        Err(_) => {
            return Ok(ObjectRecheckPipeline::Inconclusive(
                    "Native object link correspondence inconclusive; staged executable could not be read.",
                ));
        }
    };
    let (object_link_correspondence, link_corresponds, resolved_call_sites, object_translation) =
        match object_link_corresponds(&object_bytes, &executable) {
            Ok(Some((corresponds, resolved, record))) => {
                (true, corresponds, resolved, Some(record))
            }
            Ok(None) | Err(()) if allow_uncheckable_object_link => (false, true, 0, None),
            Ok(None) | Err(()) => {
                return Ok(ObjectRecheckPipeline::Inconclusive(
                    "Native object link correspondence inconclusive; linked function bytes or call targets are not checkable.",
                ));
            }
        };
    if !link_corresponds {
        return Ok(ObjectRecheckPipeline::Different(
            "linked executable function bytes or call targets differ from the object",
        ));
    }
    Ok(ObjectRecheckPipeline::Rechecked(Box::new(
        ObjectRecheckData {
            stage,
            family,
            canonical_argv_hash,
            admission,
            ir_surface,
            object_surface,
            object_link_correspondence,
            llvm_bytes: ir,
            object_bytes,
            executable_bytes: executable,
            object_translation,
            resolved_call_sites,
            source_text,
        },
    )))
}

fn machine_tv_admission_for_record(
    family: RecordFamily,
    record_profile: Option<&str>,
) -> Result<RefinedAdmission, BundleError> {
    match family {
        RecordFamily::Single => {
            // The single-file LLVM recheck uses `validate_native_main`,
            // which has no refined-admission argument.
            Ok(RefinedAdmission::Reject)
        }
        RecordFamily::Module => module_refined_admission(
            record_profile.ok_or(BundleError("native module record profile is unavailable"))?,
        ),
        RecordFamily::Multifile => multifile_refined_admission(record_profile.ok_or(
            BundleError("native multifile record profile is unavailable"),
        )?),
    }
}

fn object_function_bytes(bytes: &[u8], function_name: &str) -> Option<(u64, Vec<u8>)> {
    let file = object::File::parse(bytes).ok()?;
    let symbol = file.symbols().find(|symbol| {
        symbol.name().ok() == Some(function_name)
            && symbol.is_global()
            && symbol.is_definition()
            && symbol.kind() == SymbolKind::Text
    })?;
    let section = file.section_by_index(symbol.section_index()?).ok()?;
    let function_bytes = section.data_range(symbol.address(), symbol.size()).ok()??;
    if u64::try_from(function_bytes.len()).ok()? != symbol.size() {
        return None;
    }
    Some((symbol.address(), function_bytes.to_vec()))
}

fn write_object_recheck(
    data: ObjectRecheckData,
    expected_build_id: &str,
    json: bool,
) -> Result<(), BundleError> {
    let ObjectRecheckData {
        stage,
        ir_surface,
        object_surface,
        resolved_call_sites,
        ..
    } = data;
    let _stage = stage;
    if json {
        let descriptor = serde_json::json!({
            "schema_version": "mumei.object-symbol-correspondence/v1",
            "scope": OBJECT_SYMBOL_CORRESPONDENCE_SCOPE,
            "build_id": expected_build_id,
            "defined_functions": ir_surface.defined,
            "external_references": object_surface.undefined,
            "checks_performed": [
                "toolchain_object_executable_reproduction",
                "object_symbol_correspondence",
                "object_call_edge_correspondence",
                "object_link_correspondence",
            ],
            "checks_not_performed": [
                "optimizer_semantics",
                "raw_link_argument_vector_reproduction",
                "abi_runtime_semantics",
                "os_exit_status_observation",
                "os_stdout_observation",
                "os_stderr_observation",
                "runtime_observation_semantics",
            ],
            "call_edges": ir_surface.calls,
            "resolved_call_sites": resolved_call_sites,
        });
        serde_json::to_vec_pretty(&descriptor)
            .map_err(|_| BundleError("cannot encode object symbol descriptor"))
            .and_then(|mut bytes| {
                bytes.push(b'\n');
                std::io::stdout()
                    .lock()
                    .write_all(&bytes)
                    .map_err(|_| BundleError("cannot write object symbol descriptor"))
            })
    } else {
        let call_count: usize = ir_surface.calls.values().map(BTreeSet::len).sum();
        println!(
            "Native object symbol surface, direct call edges and linked placement correspond to the pinned LLVM IR and executable: {} defined functions, {call_count} call edges, {resolved_call_sites} resolved call sites.\nScope: {OBJECT_SYMBOL_CORRESPONDENCE_SCOPE}",
            ir_surface.defined.len(),
        );
        Ok(())
    }
}

#[derive(Default)]
struct MachineTvLoopCounts {
    equivalent_bounded_loop_count: usize,
    equivalent_source_loop_count: usize,
}

fn machine_tv_callee_saved_preservation_checks(
    equivalent_count: usize,
    preserved_count: usize,
) -> (Vec<&'static str>, Vec<&'static str>) {
    if equivalent_count == 0 {
        return (Vec::new(), Vec::new());
    }
    if preserved_count == equivalent_count {
        (
            vec!["machine_code_callee_saved_register_preservation"],
            vec!["machine_code_callee_saved_preservation_assumed_for_external_callees"],
        )
    } else {
        (
            Vec::new(),
            vec!["machine_code_callee_saved_register_preservation"],
        )
    }
}

fn machine_tv_entry_wrapper_checks(
    applicable: bool,
    outcome: Option<FunctionOutcome>,
) -> (Vec<&'static str>, Vec<&'static str>) {
    if !applicable {
        return (Vec::new(), Vec::new());
    }
    if outcome == Some(FunctionOutcome::Equivalent) {
        (vec!["machine_code_entry_wrapper_tv"], Vec::new())
    } else {
        (Vec::new(), vec!["machine_code_entry_wrapper_tv"])
    }
}

fn machine_tv_input_domain(source_loops: bool, refined_signatures: bool) -> String {
    let domain = if refined_signatures {
        MACHINE_CODE_TV_REFINED_INPUT_DOMAIN
    } else {
        MACHINE_CODE_TV_INPUT_DOMAIN
    };
    if source_loops {
        format!(
            "{domain}_and_loops_exiting_within_bound_{}",
            semantic_contract::LOOP_BOUND
        )
    } else {
        domain.to_owned()
    }
}

fn machine_tv_coverage_checks(
    equivalent_leaf_count: usize,
    equivalent_call_count: usize,
    equivalent_forward_branch_count: usize,
    equivalent_acyclic_branch_count: usize,
    equivalent_conditional_call_count: usize,
    loop_counts: MachineTvLoopCounts,
    has_cyclic_or_loop_bound_outcome: bool,
) -> (Vec<&'static str>, Vec<&'static str>) {
    let mut checks_performed = Vec::new();
    let mut checks_not_performed = Vec::new();
    if equivalent_leaf_count > 0 {
        checks_performed.push("machine_code_leaf_function_tv");
    } else {
        checks_not_performed.push("machine_code_leaf_function_tv");
    }
    if equivalent_call_count > 0 {
        checks_performed.push("machine_code_direct_call_site_tv");
    } else {
        checks_not_performed.push("machine_code_direct_call_site_tv");
    }
    if equivalent_forward_branch_count > 0 {
        checks_performed.push("machine_code_forward_branch_function_tv");
    } else {
        checks_not_performed.push("machine_code_forward_branch_function_tv");
    }
    if equivalent_acyclic_branch_count > 0 {
        checks_performed.push("machine_code_acyclic_branch_function_tv");
    } else {
        checks_not_performed.push("machine_code_acyclic_branch_function_tv");
    }
    if equivalent_conditional_call_count > 0 {
        checks_performed.push("machine_code_conditional_call_function_tv");
    } else {
        checks_not_performed.push("machine_code_conditional_call_function_tv");
    }
    if loop_counts.equivalent_bounded_loop_count > 0 {
        checks_performed.push("machine_code_bounded_loop_function_tv");
    } else {
        checks_not_performed.push("machine_code_bounded_loop_function_tv");
    }
    if loop_counts.equivalent_source_loop_count > 0 {
        checks_not_performed.extend([
            "machine_code_tv_loop_executions_beyond_bound",
            "runtime_cancellation_semantics",
        ]);
    }
    if has_cyclic_or_loop_bound_outcome {
        checks_not_performed.push("machine_code_tv_cyclic_control_flow_functions");
    }
    (checks_performed, checks_not_performed)
}

fn machine_tv_f64_scalar_checks(
    equivalent_f64_leaf_count: usize,
) -> (Vec<&'static str>, Vec<&'static str>) {
    if equivalent_f64_leaf_count == 0 {
        return (Vec::new(), Vec::new());
    }
    (
        vec!["machine_code_f64_scalar_function_tv"],
        vec![
            "machine_code_tv_mxcsr_control_state",
            "machine_code_tv_nan_payload_bits",
        ],
    )
}

fn machine_tv_composite_aggregate_checks(
    equivalent_composite_aggregate_count: usize,
    equivalent_wide_composite_aggregate_count: usize,
) -> (Vec<&'static str>, Vec<&'static str>) {
    if equivalent_composite_aggregate_count == 0 {
        return (Vec::new(), Vec::new());
    }
    (
        vec!["machine_code_composite_register_aggregate_tv"],
        if equivalent_wide_composite_aggregate_count > 0 {
            vec!["machine_code_tv_sysv_memory_class_aggregate_abi"]
        } else {
            Vec::new()
        },
    )
}

struct MachineTvCounts {
    equivalent_count: usize,
    equivalent_callee_saved_count: usize,
    equivalent_callee_saved_preserved_count: usize,
    equivalent_leaf_count: usize,
    equivalent_call_count: usize,
    equivalent_forward_branch_count: usize,
    equivalent_acyclic_branch_count: usize,
    equivalent_conditional_call_count: usize,
    equivalent_bounded_loop_count: usize,
    equivalent_source_loop_count: usize,
    equivalent_refined_signature_count: usize,
    equivalent_f64_leaf_count: usize,
    equivalent_composite_aggregate_count: usize,
    equivalent_f64_composite_aggregate_count: usize,
    equivalent_wide_composite_aggregate_count: usize,
    equivalent_wide_composite_aggregate_convention_count: usize,
    equivalent_wide_scalar_count: usize,
    equivalent_red_zone_frame_memory_count: usize,
    equivalent_allocated_frame_memory_count: usize,
    object_function_count: usize,
}

struct MachineTvEvaluation {
    functions: BTreeMap<String, serde_json::Value>,
    outcomes: BTreeMap<String, FunctionOutcome>,
    counts: MachineTvCounts,
    coverage: (Vec<&'static str>, Vec<&'static str>),
    input_domain: String,
    scope: &'static str,
    entry_wrapper_applicable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObjectMachineTvAcceptanceDecision {
    Accepted,
    Different,
    Inconclusive,
}

fn machine_tv_acceptance_decision(
    outcomes: &BTreeMap<String, FunctionOutcome>,
    entry_wrapper_applicable: bool,
) -> ObjectMachineTvAcceptanceDecision {
    if outcomes
        .values()
        .any(|outcome| *outcome == FunctionOutcome::Different)
    {
        return ObjectMachineTvAcceptanceDecision::Different;
    }
    if entry_wrapper_applicable {
        match outcomes.get("main") {
            Some(FunctionOutcome::Equivalent) => (),
            Some(FunctionOutcome::Different) => {
                return ObjectMachineTvAcceptanceDecision::Different;
            }
            Some(FunctionOutcome::Unknown | FunctionOutcome::OutsideProfile(_)) | None => {
                return ObjectMachineTvAcceptanceDecision::Inconclusive;
            }
        }
    }
    let mut source_atom_count = 0;
    for (name, outcome) in outcomes {
        if entry_wrapper_applicable && name == "main" {
            continue;
        }
        match outcome {
            FunctionOutcome::Equivalent => source_atom_count += 1,
            FunctionOutcome::OutsideProfile("not_a_source_atom") => (),
            FunctionOutcome::Different => {
                return ObjectMachineTvAcceptanceDecision::Different;
            }
            FunctionOutcome::Unknown | FunctionOutcome::OutsideProfile(_) => {
                return ObjectMachineTvAcceptanceDecision::Inconclusive;
            }
        }
    }
    if source_atom_count > 0 {
        ObjectMachineTvAcceptanceDecision::Accepted
    } else {
        ObjectMachineTvAcceptanceDecision::Inconclusive
    }
}

/// Correspondence items the object-translation acceptance requires
/// `checked` (DESIGN_OBJECT_ACCEPTANCE.md §4-2): shared single source of truth
/// with `mumei_core::object_acceptance`.
const OBJECT_TRANSLATION_ACCEPTANCE_ITEMS: [&str; 10] =
    mumei_core::object_acceptance::REQUIRED_CORRESPONDENCE_ITEMS;

/// Object-translation acceptance decision over the
/// `object_translation.correspondence.checks` item list
/// (DESIGN_OBJECT_ACCEPTANCE.md §4 / §6 Step 1): every required item
/// `checked` — and nothing else left `unchecked`/`unknown` — is
/// `Accepted`; any `inconsistent` item is `Different` (reject); a
/// missing required item or anything undecided is `Inconclusive`.
/// Fail-closed by construction. Step 2 wires it into the
/// `accept-object` emit path together with the machine-TV decision
/// and the canonical argv gate.
fn object_translation_acceptance_decision(
    checks: &[serde_json::Value],
) -> ObjectMachineTvAcceptanceDecision {
    if checks
        .iter()
        .any(|check| check["result"].as_str() == Some("inconsistent"))
    {
        return ObjectMachineTvAcceptanceDecision::Different;
    }
    let every_emitted_checked = checks
        .iter()
        .all(|check| check["result"].as_str() == Some("checked"));
    let all_required_checked = OBJECT_TRANSLATION_ACCEPTANCE_ITEMS.iter().all(|required| {
        checks.iter().any(|check| {
            check["item"].as_str() == Some(required) && check["result"].as_str() == Some("checked")
        })
    });
    if every_emitted_checked && all_required_checked {
        ObjectMachineTvAcceptanceDecision::Accepted
    } else {
        ObjectMachineTvAcceptanceDecision::Inconclusive
    }
}

/// Design memo §7 Step 2: emit the source↔machine within-profile gate
/// as metadata — whether every source-atom function came back
/// `equivalent_within_profile`, which functions did not, and the
/// residual unchecked domains. Recording only: no verdict, grade or
/// checks-list change.
fn source_machine_tv_within_profile_json(
    evaluation: &MachineTvEvaluation,
    unchecked_domains: &[&str],
    object_link_correspondence: bool,
) -> serde_json::Value {
    let accepted =
        machine_tv_acceptance_decision(&evaluation.outcomes, evaluation.entry_wrapper_applicable)
            == ObjectMachineTvAcceptanceDecision::Accepted;
    let mut non_equivalent = Vec::new();
    for (name, outcome) in &evaluation.outcomes {
        let (label, reason) = match outcome {
            FunctionOutcome::Equivalent => continue,
            FunctionOutcome::Different => ("different", None),
            FunctionOutcome::Unknown => ("unknown", None),
            FunctionOutcome::OutsideProfile(reason) => ("outside_profile", Some(*reason)),
        };
        non_equivalent.push(match reason {
            Some(reason) => {
                serde_json::json!({"name": name, "outcome": label, "reason": reason})
            }
            None => serde_json::json!({"name": name, "outcome": label}),
        });
    }
    let status = if accepted {
        "checked_source_machine_tv_within_profile"
    } else {
        "unchecked_not_translation_validated"
    };
    serde_json::json!({
        "status": status,
        "non_equivalent_functions": non_equivalent,
        "unchecked_domains": unchecked_domains,
        // The gate covers source↔machine equivalence only; the link
        // edge's own status is named explicitly so consumers do not read
        // `checked` as covering it.
        "object_link_correspondence": if object_link_correspondence {
            "checked_object_link_correspondence"
        } else {
            "unchecked_not_translation_validated"
        },
    })
}

/// Text-path counterpart of `source_machine_tv_within_profile_json`:
/// one line naming the gate status and the residual domain count.
fn source_machine_tv_within_profile_line(
    evaluation: &MachineTvEvaluation,
    unchecked_domain_count: usize,
) -> String {
    let accepted =
        machine_tv_acceptance_decision(&evaluation.outcomes, evaluation.entry_wrapper_applicable)
            == ObjectMachineTvAcceptanceDecision::Accepted;
    let status = if accepted {
        "checked_source_machine_tv_within_profile"
    } else {
        "unchecked_not_translation_validated"
    };
    let residual = evaluation
        .outcomes
        .values()
        .filter(|outcome| **outcome != FunctionOutcome::Equivalent)
        .count();
    format!(
        "Source machine-TV within profile: {status} ({residual} non-equivalent functions, {unchecked_domain_count} unchecked domains)."
    )
}

fn evaluate_machine_tv(data: &ObjectRecheckData) -> Result<MachineTvEvaluation, &'static str> {
    let source_text = data
        .source_text
        .as_deref()
        .ok_or("Machine-code TV inconclusive; the checked source snapshot is unavailable.")?;
    let mut config = z3::Config::new();
    config.set_timeout_msec(u64::from(machine_tv::TIMEOUT_MS));
    config.set_param_value("rlimit", &machine_tv::RESOURCE_LIMIT.to_string());
    let context = z3::Context::new(&config);
    let models = match semantic_contract::encode_pure_i64_module(
        source_text,
        &context,
        true,
        data.admission,
    ) {
        Ok(models) => models,
        Err(_) => {
            return Err(
                "Machine-code TV inconclusive; source is outside the pure-i64 model profile.",
            )
        }
    };
    let mut source_models = BTreeMap::new();
    for model in models {
        let symbol_name = tv::native_function_name(&model.function_name).to_owned();
        if source_models.insert(symbol_name, model).is_some() {
            return Err("Machine-code TV inconclusive; source atom names do not map uniquely to object functions.");
        }
    }
    // Step 3a: scalar-i64 functions compare against the LLVM-IR
    // encoding instead of the source model's expected value. The IR
    // module parses once and is shared across functions; when no IR is
    // available the source-model comparison stays.
    let mut parsed_ir = None;
    let mut parsed_ir_failed = false;
    let entry_wrapper_applicable = data.object_surface.defined.contains("main")
        && data.object_surface.defined.contains("__mumei_user_main")
        && source_models.get("__mumei_user_main").is_some_and(|model| {
            machine_tv::supports_signature(model)
                && !model.result_leaf_doubles.iter().any(|is_double| *is_double)
        });
    let mut functions = BTreeMap::new();
    let mut outcomes = BTreeMap::new();
    let mut equivalent_count = 0usize;
    let mut equivalent_callee_saved_count = 0usize;
    let mut equivalent_callee_saved_preserved_count = 0usize;
    let mut equivalent_leaf_count = 0usize;
    let mut equivalent_call_count = 0usize;
    let mut equivalent_forward_branch_count = 0usize;
    let mut equivalent_acyclic_branch_count = 0usize;
    let mut equivalent_conditional_call_count = 0usize;
    let mut equivalent_bounded_loop_count = 0usize;
    let mut equivalent_source_loop_count = 0usize;
    let mut equivalent_refined_signature_count = 0usize;
    let mut equivalent_f64_leaf_count = 0usize;
    let mut equivalent_composite_aggregate_count = 0usize;
    let mut equivalent_f64_composite_aggregate_count = 0usize;
    let mut equivalent_wide_composite_aggregate_count = 0usize;
    let mut equivalent_wide_composite_aggregate_convention_count = 0usize;
    let mut equivalent_wide_scalar_count = 0usize;
    let mut equivalent_red_zone_frame_memory_count = 0usize;
    let mut equivalent_allocated_frame_memory_count = 0usize;
    let mut has_cyclic_or_loop_bound_outcome = false;
    for function_name in &data.object_surface.defined {
        let is_entry_wrapper = function_name == "main";
        let has_refined_signature = source_models
            .get(function_name)
            .is_some_and(|model| model.refined_signature);
        let has_f64_leaves = source_models.get(function_name).is_some_and(|model| {
            model
                .param_leaf_doubles
                .iter()
                .flatten()
                .any(|is_double| *is_double)
                || model.result_leaf_doubles.iter().any(|is_double| *is_double)
                || model.call_sites.iter().any(|call_site| {
                    call_site
                        .arg_leaf_doubles
                        .iter()
                        .any(|is_double| *is_double)
                        || call_site
                            .result_leaf_doubles
                            .iter()
                            .any(|is_double| *is_double)
                })
        });
        let has_composite_aggregate = source_models
            .get(function_name)
            .is_some_and(machine_tv_model_uses_composite_aggregate);
        let has_f64_composite_aggregate = source_models
            .get(function_name)
            .is_some_and(machine_tv_model_uses_f64_composite_aggregate);
        let has_wide_composite_aggregate = source_models
            .get(function_name)
            .is_some_and(machine_tv_model_uses_wide_composite_aggregate);
        let has_wide_composite_aggregate_convention = source_models
            .get(function_name)
            .is_some_and(machine_tv_model_uses_wide_composite_aggregate_convention);
        let has_wide_scalar = source_models
            .get(function_name)
            .is_some_and(machine_tv_model_uses_wide_scalar);
        let has_source_loop = source_models
            .get(function_name)
            .is_some_and(|model| model.loop_bound_exits.is_some());
        let has_source_calls = source_models
            .get(function_name)
            .is_some_and(|model| !model.call_sites.is_empty());
        let mut branch_shape = machine_tv::BranchShape::None;
        let mut has_conditional_call = false;
        let mut has_reachable_cycle = false;
        let mut callee_saved_preservation = None;
        let mut frame_memory_use = machine_tv::FrameMemoryUse::default();
        let outcome = if is_entry_wrapper {
            if entry_wrapper_applicable {
                if let Some((address, bytes)) =
                    object_function_bytes(&data.object_bytes, function_name)
                {
                    let call_targets = data
                        .object_surface
                        .call_targets
                        .get(function_name)
                        .cloned()
                        .unwrap_or_default();
                    let (outcome, preservation) = machine_tv::check_entry_wrapper_with_preservation(
                        &bytes,
                        address,
                        &call_targets,
                        &context,
                    );
                    callee_saved_preservation = preservation;
                    outcome
                } else {
                    FunctionOutcome::OutsideProfile("unsupported_instruction")
                }
            } else {
                FunctionOutcome::OutsideProfile("not_a_source_atom")
            }
        } else if let Some(model) = source_models.get(function_name) {
            if model.inputs.len() > machine_tv::MAX_ARGUMENT_COUNT {
                FunctionOutcome::OutsideProfile("argument_count")
            } else if let Some((address, bytes)) =
                object_function_bytes(&data.object_bytes, function_name)
            {
                let call_targets = data
                    .object_surface
                    .call_targets
                    .get(function_name)
                    .cloned()
                    .unwrap_or_default();
                has_reachable_cycle =
                    machine_tv::disassemble(&bytes, address)
                        .ok()
                        .is_some_and(|instructions| {
                            machine_tv::function_has_reachable_cycle(
                                &instructions,
                                bytes.len() as u64,
                                &call_targets,
                            )
                        });
                branch_shape = machine_tv::branch_shape(&bytes, address, &call_targets);
                has_conditional_call =
                    machine_tv::has_conditional_branch_and_call(&bytes, address, &call_targets);
                // Step 3a: in the scalar-i64 subset the expected value
                // comes from the LLVM-IR encoding rather than the
                // source model. Without IR, or when the function
                // cannot be encoded, the source-model comparison stays
                // — the `source_machine_tv` claim is machine↔source
                // either way (IR edges are bound to the source by the
                // hir→llvm_ir translation validation).
                let ir_encoding = if machine_tv::ir_object_signature_liftable(model)
                    && !data.llvm_bytes.is_empty()
                {
                    if parsed_ir.is_none() && !parsed_ir_failed {
                        match unsafe { tv::parse_ir_module_for_comparison(&data.llvm_bytes) } {
                            Ok(module) => parsed_ir = Some(module),
                            Err(_) => parsed_ir_failed = true,
                        }
                    }
                    let symbol = tv::native_function_name(&model.function_name).to_owned();
                    parsed_ir.as_ref().and_then(|module| unsafe {
                        module
                            .function(&symbol)
                            .and_then(|raw| {
                                tv::encode_parsed_function_for_comparison(
                                    raw, model, false, &context,
                                )
                            })
                            .ok()
                    })
                } else {
                    None
                };
                let (outcome, preservation, frame_memory_use_result) = match ir_encoding {
                    Some((ir_returns, ir_definedness)) => {
                        machine_tv::check_function_with_frame_memory_against_ir(
                            &bytes,
                            address,
                            model,
                            &ir_returns,
                            &ir_definedness,
                            &call_targets,
                            &context,
                        )
                    }
                    None => machine_tv::check_function_with_frame_memory(
                        &bytes,
                        address,
                        model,
                        &call_targets,
                        &context,
                    ),
                };
                callee_saved_preservation = preservation;
                frame_memory_use = frame_memory_use_result;
                outcome
            } else {
                FunctionOutcome::OutsideProfile("unsupported_instruction")
            }
        } else {
            FunctionOutcome::OutsideProfile("not_a_source_atom")
        };
        let value = match outcome {
            FunctionOutcome::Equivalent => {
                equivalent_callee_saved_count += 1;
                if !is_entry_wrapper {
                    equivalent_count += 1;
                }
                let preservation = callee_saved_preservation.ok_or(
                    "Machine-code TV equivalence result is missing callee-saved preservation.",
                )?;
                let preservation_label = match preservation {
                    machine_tv::CalleeSavedPreservation::Preserved => {
                        equivalent_callee_saved_preserved_count += 1;
                        "preserved"
                    }
                    machine_tv::CalleeSavedPreservation::Violated => "violated",
                    machine_tv::CalleeSavedPreservation::Unknown => "unknown",
                };
                if !is_entry_wrapper {
                    if has_refined_signature {
                        equivalent_refined_signature_count += 1;
                    }
                    if has_f64_leaves {
                        equivalent_f64_leaf_count += 1;
                    }
                    if has_composite_aggregate {
                        equivalent_composite_aggregate_count += 1;
                    }
                    if has_f64_composite_aggregate {
                        equivalent_f64_composite_aggregate_count += 1;
                    }
                    if has_wide_composite_aggregate {
                        equivalent_wide_composite_aggregate_count += 1;
                    }
                    if has_wide_composite_aggregate_convention {
                        equivalent_wide_composite_aggregate_convention_count += 1;
                    }
                    if has_wide_scalar {
                        equivalent_wide_scalar_count += 1;
                    }
                    if frame_memory_use.red_zone {
                        equivalent_red_zone_frame_memory_count += 1;
                    }
                    if frame_memory_use.allocated {
                        equivalent_allocated_frame_memory_count += 1;
                    }
                    if has_source_loop {
                        equivalent_source_loop_count += 1;
                    }
                    if has_reachable_cycle {
                        equivalent_bounded_loop_count += 1;
                    }
                    if has_source_calls {
                        equivalent_call_count += 1;
                        if has_conditional_call {
                            equivalent_conditional_call_count += 1;
                        }
                    } else if !has_reachable_cycle {
                        match branch_shape {
                            machine_tv::BranchShape::None => equivalent_leaf_count += 1,
                            machine_tv::BranchShape::Forward => {
                                equivalent_forward_branch_count += 1;
                            }
                            machine_tv::BranchShape::AcyclicBackward => {
                                equivalent_acyclic_branch_count += 1;
                            }
                        }
                    }
                }
                serde_json::json!({
                    "outcome": "equivalent_within_profile",
                    "callee_saved_preservation": preservation_label,
                })
            }
            FunctionOutcome::Different => serde_json::json!({"outcome": "different"}),
            FunctionOutcome::Unknown => serde_json::json!({"outcome": "unknown"}),
            FunctionOutcome::OutsideProfile(reason) => {
                if matches!(reason, "cyclic_control_flow" | "loop_bound") {
                    has_cyclic_or_loop_bound_outcome = true;
                }
                serde_json::json!({"outcome": "outside_profile", "reason": reason})
            }
        };
        let value = if is_entry_wrapper {
            let mut value = value;
            if let serde_json::Value::Object(entry) = &mut value {
                entry.insert("role".to_owned(), serde_json::json!("entry_wrapper"));
            }
            value
        } else {
            value
        };
        functions.insert(function_name.clone(), value);
        outcomes.insert(function_name.clone(), outcome);
    }
    let counts = MachineTvCounts {
        equivalent_count,
        equivalent_callee_saved_count,
        equivalent_callee_saved_preserved_count,
        equivalent_leaf_count,
        equivalent_call_count,
        equivalent_forward_branch_count,
        equivalent_acyclic_branch_count,
        equivalent_conditional_call_count,
        equivalent_bounded_loop_count,
        equivalent_source_loop_count,
        equivalent_refined_signature_count,
        equivalent_f64_leaf_count,
        equivalent_composite_aggregate_count,
        equivalent_f64_composite_aggregate_count,
        equivalent_wide_composite_aggregate_count,
        equivalent_wide_composite_aggregate_convention_count,
        equivalent_wide_scalar_count,
        equivalent_red_zone_frame_memory_count,
        equivalent_allocated_frame_memory_count,
        object_function_count: data.object_surface.defined.len(),
    };
    let mut coverage = machine_tv_coverage_checks(
        counts.equivalent_leaf_count,
        counts.equivalent_call_count,
        counts.equivalent_forward_branch_count,
        counts.equivalent_acyclic_branch_count,
        counts.equivalent_conditional_call_count,
        MachineTvLoopCounts {
            equivalent_bounded_loop_count: counts.equivalent_bounded_loop_count,
            equivalent_source_loop_count: counts.equivalent_source_loop_count,
        },
        has_cyclic_or_loop_bound_outcome,
    );
    let callee_saved_coverage = machine_tv_callee_saved_preservation_checks(
        counts.equivalent_callee_saved_count,
        counts.equivalent_callee_saved_preserved_count,
    );
    let f64_coverage = machine_tv_f64_scalar_checks(counts.equivalent_f64_leaf_count);
    coverage.0.extend(f64_coverage.0);
    coverage.1.extend(f64_coverage.1);
    let composite_aggregate_coverage = machine_tv_composite_aggregate_checks(
        counts.equivalent_composite_aggregate_count,
        counts.equivalent_wide_composite_aggregate_count,
    );
    coverage.0.extend(composite_aggregate_coverage.0);
    coverage.1.extend(composite_aggregate_coverage.1);
    coverage.0.extend(callee_saved_coverage.0);
    coverage.1.extend(callee_saved_coverage.1);
    let entry_wrapper_coverage =
        machine_tv_entry_wrapper_checks(entry_wrapper_applicable, outcomes.get("main").copied());
    coverage.0.extend(entry_wrapper_coverage.0);
    coverage.1.extend(entry_wrapper_coverage.1);
    let scope = if data.object_link_correspondence {
        MACHINE_CODE_TV_SCOPE
    } else {
        MACHINE_CODE_TV_WITHOUT_OBJECT_LINK_SCOPE
    };
    let input_domain = machine_tv_input_domain(
        counts.equivalent_source_loop_count > 0,
        counts.equivalent_refined_signature_count > 0,
    );
    Ok(MachineTvEvaluation {
        functions,
        outcomes,
        counts,
        coverage,
        input_domain,
        scope,
        entry_wrapper_applicable,
    })
}

fn write_machine_tv(
    data: ObjectRecheckData,
    expected_build_id: &str,
    json: bool,
) -> Result<(), BundleError> {
    let evaluation = match evaluate_machine_tv(&data) {
        Ok(evaluation) => evaluation,
        Err(message) => {
            eprintln!("{message}");
            drop(data);
            std::process::exit(3);
        }
    };
    for (function_name, outcome) in &evaluation.outcomes {
        match outcome {
            FunctionOutcome::Different => {
                if function_name == "main" && evaluation.entry_wrapper_applicable {
                    eprintln!("machine code of the entry wrapper differs from its model");
                } else {
                    eprintln!("machine code of {function_name} differs from its source atom");
                }
                drop(data);
                std::process::exit(1);
            }
            FunctionOutcome::Unknown => {
                eprintln!("Machine-code TV inconclusive; Z3 returned unknown.");
                drop(data);
                std::process::exit(3);
            }
            FunctionOutcome::OutsideProfile(_)
                if function_name == "main" && evaluation.entry_wrapper_applicable =>
            {
                eprintln!("Machine-code TV inconclusive; entry wrapper is outside the profile.");
                drop(data);
                std::process::exit(3);
            }
            _ => (),
        }
    }
    if json {
        let mut checks_performed = vec![
            "toolchain_object_executable_reproduction",
            "object_symbol_correspondence",
            "object_call_edge_correspondence",
        ];
        let mut checks_not_performed = vec![
            "optimizer_semantics",
            "raw_link_argument_vector_reproduction",
            "abi_runtime_semantics",
            "os_exit_status_observation",
            "os_stdout_observation",
            "os_stderr_observation",
            "runtime_observation_semantics",
        ];
        if data.object_link_correspondence {
            checks_performed.push("object_link_correspondence");
        } else {
            checks_not_performed.push("object_link_correspondence");
        }
        checks_performed.extend(evaluation.coverage.0.iter().copied());
        checks_not_performed.extend(evaluation.coverage.1.iter().copied());
        let record = data.object_translation.clone().unwrap_or_else(|| {
            object_translation_parse(&data.object_bytes, &data.executable_bytes)
        });
        let correspondence = object_translation_correspondence_json(&data, &record);
        let descriptor = serde_json::json!({
            "schema_version": "mumei.machine-code-tv/v1",
            "scope": evaluation.scope,
            "build_id": expected_build_id,
            "register_convention_assumed": machine_tv_register_convention_assumed_with_frame_memory(
                evaluation.counts.equivalent_f64_leaf_count,
                evaluation.counts.equivalent_composite_aggregate_count,
                evaluation.counts.equivalent_f64_composite_aggregate_count,
                evaluation
                    .counts
                    .equivalent_wide_composite_aggregate_convention_count,
                evaluation.counts.equivalent_wide_scalar_count,
                evaluation.counts.equivalent_red_zone_frame_memory_count,
                evaluation.counts.equivalent_allocated_frame_memory_count,
            ),
            "input_domain": evaluation.input_domain,
            "functions": evaluation.functions,
            "object_translation": object_translation_json(
                &data,
                if machine_tv_acceptance_decision(
                    &evaluation.outcomes,
                    evaluation.entry_wrapper_applicable,
                ) == ObjectMachineTvAcceptanceDecision::Accepted
                {
                    "checked_source_machine_tv_within_profile"
                } else {
                    "unchecked_not_translation_validated"
                },
                &record,
                &correspondence,
            ),
            "source_machine_tv": source_machine_tv_within_profile_json(
                &evaluation,
                &checks_not_performed,
                data.object_link_correspondence,
            ),
            "checks_performed": checks_performed,
            "checks_not_performed": checks_not_performed,
        });
        serde_json::to_vec_pretty(&descriptor)
            .map_err(|_| BundleError("cannot encode machine-code TV descriptor"))
            .and_then(|mut bytes| {
                bytes.push(b'\n');
                std::io::stdout()
                    .lock()
                    .write_all(&bytes)
                    .map_err(|_| BundleError("cannot write machine-code TV descriptor"))
            })
    } else {
        let wrapper_summary =
            if evaluation.outcomes.get("main") == Some(&FunctionOutcome::Equivalent) {
                " Entry wrapper is equivalent within the machine-code TV profile."
            } else {
                ""
            };
        println!(
            "Machine-code TV: {} of {} object functions equivalent to their source atoms within the acyclic-branch, conditional-call, and bounded-loop profile ({} bounded-loop functions).{wrapper_summary}\nScope: {}",
            evaluation.counts.equivalent_count,
            evaluation.counts.object_function_count,
            evaluation.counts.equivalent_bounded_loop_count,
            evaluation.scope,
        );
        // Mirrors the JSON descriptor's checks_not_performed list: the
        // seven fixed domains, object_link_correspondence when the link
        // check did not run, plus the coverage list.
        let unchecked_domain_count =
            7 + usize::from(!data.object_link_correspondence) + evaluation.coverage.1.len();
        println!(
            "{}",
            source_machine_tv_within_profile_line(&evaluation, unchecked_domain_count)
        );
        for line in object_translation_correspondence_text(&data) {
            println!("{line}");
        }
        Ok(())
    }
}

/// Step 1 of docs/DESIGN_OBJECT_TRANSLATION_CORRESPONDENCE.md: emit the
/// object-stage translation record as metadata only. The relations keep
/// `unchecked_not_translation_validated` (or the honest link-check name)
/// — no promotion is claimed here.
/// Best-effort parse of the object/executable pair for the metadata
/// record. Unlike `object_link_corresponds` it never bails on
/// uncheckable details: it lists every defined text symbol and every
/// relocation in every section it finds, leaving `executable_address` /
/// `resolved_value` empty when they cannot be computed.
fn object_translation_parse(
    object_bytes: &[u8],
    executable_bytes: &[u8],
) -> ObjectTranslationRecord {
    let mut record = ObjectTranslationRecord {
        functions: Vec::new(),
        relocations: Vec::new(),
    };
    let (Ok(object_file), Ok(executable_file)) = (
        object::File::parse(object_bytes),
        object::File::parse(executable_bytes),
    ) else {
        return record;
    };
    let executable_symbols = executable_text_symbols(&executable_file);
    let executable_address_of = |name: &str| -> Option<u64> {
        executable_symbols
            .get(name)
            .filter(|addresses| addresses.len() == 1)
            .map(|addresses| addresses[0])
    };
    // Loader imports (global non-definition symbols) keep the same
    // `assumed_external_callee` marker the strict path records.
    let executable_imports: BTreeSet<String> = executable_file
        .symbols()
        .chain(executable_file.dynamic_symbols())
        .filter(|symbol| symbol.is_global() && !symbol.is_definition())
        .filter_map(|symbol| symbol.name().ok().map(str::to_owned))
        .collect();
    for symbol in object_file.symbols() {
        if !symbol.is_global() || !symbol.is_definition() || symbol.kind() != SymbolKind::Text {
            continue;
        }
        let Ok(name) = symbol.name() else {
            continue;
        };
        let section_name = symbol
            .section_index()
            .and_then(|index| object_file.section_by_index(index).ok())
            .and_then(|section| section.name().ok())
            .unwrap_or_default()
            .to_owned();
        record.functions.push(ObjectFunctionRecord {
            ir_function: name.to_owned(),
            object_symbol: name.to_owned(),
            object_section: section_name,
            object_offset: symbol.address(),
            object_size: symbol.size(),
            executable_address: executable_address_of(name),
        });
    }
    for section in object_file.sections() {
        let section_name = section.name().unwrap_or_default().to_owned();
        for (offset, relocation) in section.relocations() {
            let kind = relocation_kind_name(relocation.flags());
            let target_name = match relocation.target() {
                RelocationTarget::Symbol(symbol_index) => object_file
                    .symbol_by_index(symbol_index)
                    .ok()
                    .map(|target| {
                        let name = target.name().unwrap_or_default();
                        if name.is_empty() {
                            // Section symbols carry no name; record
                            // the section's name like `readelf` shows
                            // (`.text + N`).
                            target
                                .section_index()
                                .and_then(|index| object_file.section_by_index(index).ok())
                                .and_then(|section| section.name().ok().map(str::to_owned))
                                .unwrap_or_default()
                        } else {
                            name.to_owned()
                        }
                    })
                    .unwrap_or_default(),
                _ => String::new(),
            };
            // resolved_value is the ELF pc-relative formula value
            // (target + addend - place); it needs the target's linked
            // address and the containing function's linked address.
            let resolved_value = executable_address_of(&target_name).and_then(|target_address| {
                let function = record.functions.iter().find(|function| {
                    function.object_section == section_name
                        && function.object_offset <= offset
                        && offset < function.object_offset + function.object_size
                })?;
                let relative = offset.checked_sub(function.object_offset)?;
                let place = function.executable_address?.checked_add(relative)?;
                i64::try_from(target_address)
                    .ok()?
                    .checked_add(relocation.addend())?
                    .checked_sub(i64::try_from(place).ok()?)
            });
            record.relocations.push(ObjectRelocationRecord {
                object_section: section_name.clone(),
                object_offset: offset,
                kind,
                target_symbol: target_name.clone(),
                addend: relocation.addend(),
                resolved_value,
                applied_offset: None,
                assumption: executable_imports
                    .contains(target_name.as_str())
                    .then_some("assumed_external_callee"),
            });
        }
    }
    record
}

/// Global defined text symbols of the linked executable, by name.
fn executable_text_symbols<'a, R: object::ReadRef<'a>>(
    executable_file: &object::File<'a, R>,
) -> BTreeMap<String, Vec<u64>> {
    let mut symbols = BTreeMap::<String, Vec<u64>>::new();
    for symbol in executable_file.symbols() {
        if symbol.is_global() && symbol.is_definition() && symbol.kind() == SymbolKind::Text {
            if let Ok(name) = symbol.name() {
                symbols
                    .entry(name.to_owned())
                    .or_default()
                    .push(symbol.address());
            }
        }
    }
    symbols
}

/// A defined, named symbol living outside text sections, with its
/// byte content when the section carries file bytes.
struct ObjectDataSymbol {
    section_name: String,
    size: u64,
    /// `None` for `.bss`-style uninitialized sections — size is the
    /// whole claim there, and the runtime value is zero.
    data: Option<Vec<u8>>,
}

impl ObjectDataSymbol {
    /// The symbol's contents — the section bytes, or an all-zero
    /// image of `size` bytes for uninitialized sections.
    fn contents(&self) -> Vec<u8> {
        self.data
            .clone()
            .unwrap_or_else(|| vec![0; self.size as usize])
    }
}

/// Data-section surface of a compiled object: every defined, named,
/// non-text symbol — including locals — mapped to its section and
/// bytes, plus the raw contents of each initialized data section.
/// Anonymous `unnamed_addr` constants land in a section's bytes
/// without a symbol (LLVM merges identical constants), so the
/// section contents are needed for the content-match fallback.
/// Assembler-local `.L*` labels and section/file symbols are
/// skipped: they never name an IR global.
#[derive(Default)]
struct ObjectDataSurface {
    symbols: BTreeMap<String, ObjectDataSymbol>,
    section_contents: BTreeMap<String, Vec<u8>>,
}

fn object_data_surface(bytes: &[u8]) -> Result<ObjectDataSurface, ()> {
    let file = object::File::parse(bytes).map_err(|_| ())?;
    let mut surface = ObjectDataSurface::default();
    for section in file.sections() {
        if matches!(
            section.kind(),
            SectionKind::Data | SectionKind::ReadOnlyData | SectionKind::ReadOnlyString
        ) {
            surface.section_contents.insert(
                section.name().map_err(|_| ())?.to_owned(),
                section.uncompressed_data().map_err(|_| ())?.to_vec(),
            );
        }
    }
    for symbol in file.symbols() {
        if !symbol.is_definition() {
            continue;
        }
        if matches!(
            symbol.kind(),
            SymbolKind::Text | SymbolKind::Section | SymbolKind::File
        ) {
            continue;
        }
        let Some(section_index) = symbol.section_index() else {
            continue;
        };
        let section = file.section_by_index(section_index).map_err(|_| ())?;
        if section.kind() == SectionKind::Text {
            continue;
        }
        let name = symbol.name().map_err(|_| ())?;
        if name.is_empty() || name.starts_with('.') {
            continue;
        }
        let size = symbol.size();
        let data = if section.kind() == SectionKind::UninitializedData {
            None
        } else {
            let base = symbol.address().checked_sub(section.address()).ok_or(())?;
            let end = base.checked_add(size).ok_or(())?;
            let bytes = section.uncompressed_data().map_err(|_| ())?;
            let end_usize = usize::try_from(end).map_err(|_| ())?;
            let base_usize = usize::try_from(base).map_err(|_| ())?;
            if end_usize > bytes.len() {
                return Err(());
            }
            Some(bytes[base_usize..end_usize].to_vec())
        };
        surface.symbols.insert(
            name.to_owned(),
            ObjectDataSymbol {
                section_name: section.name().map_err(|_| ())?.to_owned(),
                size,
                data,
            },
        );
    }
    Ok(surface)
}

/// Binding-class surface of a compiled object: every defined, named
/// symbol — text and data, locals included — mapped to its ELF
/// binding (`global`/`local`/`weak`). Names bound by two different
/// symbols land in `conflicts`: the correspondence check cannot say
/// which one an IR entity produced, so they stay unverifiable.
/// Section/file symbols, `.L*`-style anonymous labels and undefined
/// references carry no definition-side binding and are skipped.
#[derive(Default)]
struct ObjectSymbolBindingSurface {
    bindings: BTreeMap<String, &'static str>,
    conflicts: BTreeSet<String>,
}

fn object_symbol_binding_surface(bytes: &[u8]) -> Result<ObjectSymbolBindingSurface, ()> {
    let file = object::File::parse(bytes).map_err(|_| ())?;
    let mut surface = ObjectSymbolBindingSurface::default();
    for symbol in file.symbols() {
        if symbol.is_undefined() {
            continue;
        }
        if matches!(symbol.kind(), SymbolKind::Section | SymbolKind::File) {
            continue;
        }
        let name = symbol.name().map_err(|_| ())?;
        if name.is_empty() || name.starts_with('.') {
            continue;
        }
        let binding = if symbol.is_weak() {
            "weak"
        } else if symbol.is_common() || symbol.is_global() {
            "global"
        } else if symbol.is_local() {
            "local"
        } else {
            continue;
        };
        if surface.bindings.insert(name.to_owned(), binding).is_some() {
            surface.conflicts.insert(name.to_owned());
        }
    }
    Ok(surface)
}

/// Step 2 correspondence inspection of the Step 1 metadata record:
/// compares the emitted function table against the IR-defined surface
/// and checks every relocation record for a resolvable target and an
/// in-bounds offset. Each item reports `checked` (the comparison ran
/// and passed), `unchecked` (it could not run — e.g. no IR surface or
/// values that cannot be recomputed), or `inconsistent` with the
/// offending names/offsets in `detail`. The record stays honest
/// metadata: an inconsistency is reported here, never promoted into a
/// verdict or a performed check.
/// Optional IR-derived comparison inputs for the correspondence
/// checks — each `None` keeps its item `unchecked` (fail-closed).
#[derive(Default)]
struct IrCorrespondenceInputs<'a> {
    equivalence: Option<&'a BTreeMap<String, &'static str>>,
    hir_llvm: Option<&'a BTreeMap<String, &'static str>>,
    data_globals: Option<&'a BTreeMap<String, Option<Vec<u8>>>>,
    symbol_attributes: Option<&'a tv::IrSymbolAttributes>,
}

fn object_translation_correspondence_checks(
    ir_surface: &tv::IrFunctionSurface,
    executable_bytes: &[u8],
    record: &ObjectTranslationRecord,
    ir: IrCorrespondenceInputs<'_>,
    object_bytes: &[u8],
) -> serde_json::Value {
    let check = |item: &str, result: &str, detail: String| serde_json::json!({"item": item, "result": result, "detail": detail});
    let mut checks = Vec::new();

    // function_coverage / function_count: object-side function
    // enumeration against the IR-defined surface.
    if ir_surface.defined.is_empty() {
        checks.push(check(
            "function_coverage",
            "unchecked",
            "no IR-defined function surface to compare".to_owned(),
        ));
        checks.push(check(
            "function_count",
            "unchecked",
            "no IR-defined function surface to compare".to_owned(),
        ));
    } else {
        let mut missing = Vec::new();
        for name in &ir_surface.defined {
            if !record
                .functions
                .iter()
                .any(|function| function.ir_function == *name)
            {
                missing.push(name.clone());
            }
        }
        let mut extra = Vec::new();
        for function in &record.functions {
            if !ir_surface.defined.contains(&function.ir_function) {
                extra.push(function.ir_function.clone());
            }
        }
        if missing.is_empty() && extra.is_empty() {
            checks.push(check(
                "function_coverage",
                "checked",
                format!(
                    "{} IR-defined functions each map to one object record",
                    ir_surface.defined.len()
                ),
            ));
        } else {
            checks.push(check(
                "function_coverage",
                "inconsistent",
                format!("unmapped IR functions: {missing:?}; unmapped object records: {extra:?}"),
            ));
        }
        if record.functions.len() == ir_surface.defined.len() {
            checks.push(check(
                "function_count",
                "checked",
                format!("{} function records", record.functions.len()),
            ));
        } else {
            checks.push(check(
                "function_count",
                "inconsistent",
                format!(
                    "{} function records vs {} IR-defined functions",
                    record.functions.len(),
                    ir_surface.defined.len()
                ),
            ));
        }
    }

    // relocation_targets / relocation_bounds / relocation_resolved_values.
    let executable_file = object::File::parse(executable_bytes).ok();
    let executable_symbols = executable_file
        .as_ref()
        .map(executable_text_symbols)
        .unwrap_or_default();
    // Symbols the link leaves to the loader — undefined imports and
    // dynamic symbols. They resolve through the same
    // assumed-external-callee vocabulary machine-TV uses for external
    // calls, so they are resolvable here rather than inconsistent.
    let executable_imports: BTreeSet<String> = executable_file
        .as_ref()
        .map(|file| {
            file.symbols()
                .chain(file.dynamic_symbols())
                .filter(|symbol| symbol.is_global() && !symbol.is_definition())
                .filter_map(|symbol| symbol.name().ok().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let resolvable_target = |target: &str| -> bool {
        record
            .functions
            .iter()
            .any(|function| function.object_symbol == target)
            || executable_symbols.contains_key(target)
            || executable_imports.contains(target)
            || ir_surface.declared.contains(target)
            || machine_tv::is_ti3_libcall(target)
            // Section-symbol relocation targets (`.text + N` in
            // `.eh_frame` and friends) name their own section.
            || executable_file
                .as_ref()
                .is_some_and(|file| file.sections().any(|s| s.name() == Ok(target)))
    };
    let unresolvable: Vec<String> = record
        .relocations
        .iter()
        .filter(|relocation| !resolvable_target(&relocation.target_symbol))
        .map(|relocation| relocation.target_symbol.clone())
        .collect();
    if executable_file.is_none() {
        checks.push(check(
            "relocation_targets",
            "unchecked",
            "executable could not be parsed".to_owned(),
        ));
    } else if unresolvable.is_empty() {
        let assumed_external: Vec<&str> = record
            .relocations
            .iter()
            .map(|relocation| relocation.target_symbol.as_str())
            .filter(|target| executable_imports.contains(*target))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        checks.push(check(
            "relocation_targets",
            "checked",
            if assumed_external.is_empty() {
                format!("{} relocation targets resolvable", record.relocations.len())
            } else {
                format!(
                    "{} relocation targets resolvable ({} assumed-external: {assumed_external:?})",
                    record.relocations.len(),
                    assumed_external.len()
                )
            },
        ));
    } else {
        checks.push(check(
            "relocation_targets",
            "inconsistent",
            format!("unresolvable relocation targets: {unresolvable:?}"),
        ));
    }
    // Bounds apply to relocations inside code sections — entries in
    // other sections (`.eh_frame` and friends) cannot sit inside a
    // function record by construction, so they are exempt and noted.
    let code_sections: BTreeSet<&str> = record
        .functions
        .iter()
        .map(|function| function.object_section.as_str())
        .collect();
    let out_of_bounds: Vec<(String, u64)> = record
        .relocations
        .iter()
        .filter(|relocation| code_sections.contains(relocation.object_section.as_str()))
        .filter(|relocation| {
            !record.functions.iter().any(|function| {
                function.object_section == relocation.object_section
                    && function.object_offset <= relocation.object_offset
                    && relocation.object_offset < function.object_offset + function.object_size
            })
        })
        .map(|relocation| (relocation.object_section.clone(), relocation.object_offset))
        .collect();
    let exempt = record
        .relocations
        .iter()
        .filter(|relocation| !code_sections.contains(relocation.object_section.as_str()))
        .count();
    if out_of_bounds.is_empty() {
        checks.push(check(
            "relocation_bounds",
            "checked",
            if exempt == 0 {
                format!(
                    "{} relocations inside an enumerated function",
                    record.relocations.len()
                )
            } else {
                format!(
                    "{} relocations inside an enumerated function ({exempt} outside code sections)",
                    record.relocations.len() - exempt
                )
            },
        ));
    } else {
        checks.push(check(
            "relocation_bounds",
            "inconsistent",
            format!("relocations outside every function record: {out_of_bounds:?}"),
        ));
    }
    let resolved = record
        .relocations
        .iter()
        .filter(|relocation| relocation.resolved_value.is_some())
        .count();
    if resolved == record.relocations.len() {
        checks.push(check(
            "relocation_resolved_values",
            "checked",
            format!("{resolved} relocation values recomputed"),
        ));
    } else {
        checks.push(check(
            "relocation_resolved_values",
            "unchecked",
            format!(
                "{resolved}/{} relocation values recomputed",
                record.relocations.len()
            ),
        ));
    }

    // relocation_applied_values (Step 4 全件照合): every record's
    // recomputed value must be the value the linker actually applied —
    // at the owning function's exe site for code relocations, and at
    // the recorded `applied_offset` in the same-named section for
    // non-code entries (`.eh_frame` entries get merged, so their slot
    // is located by a unique-value search and then re-read at that
    // exact offset). A record without a recomputed value stays
    // unresolved, never assumed.
    let exe_sections: Vec<_> = executable_file
        .as_ref()
        .map(|file| file.sections().collect())
        .unwrap_or_default();
    let mut verified = 0usize;
    let mut unresolved = 0usize;
    let mut mismatched = Vec::new();
    for relocation in &record.relocations {
        let Some(resolved) = relocation.resolved_value else {
            unresolved += 1;
            continue;
        };
        let function = record.functions.iter().find(|function| {
            function.object_section == relocation.object_section
                && function.object_offset <= relocation.object_offset
                && relocation.object_offset < function.object_offset + function.object_size
        });
        match function {
            Some(function) => {
                let applied = function
                    .executable_address
                    .and_then(|base| {
                        base.checked_add(relocation.object_offset - function.object_offset)
                    })
                    .and_then(|place| {
                        exe_sections
                            .iter()
                            .find(|section| {
                                section.address() <= place
                                    && place + 4 <= section.address() + section.size()
                            })
                            .and_then(|section| section.data_range(place, 4).ok().flatten())
                    })
                    .map(|bytes| i64::from(i32::from_le_bytes(bytes.try_into().unwrap())));
                match applied {
                    Some(actual) if actual == resolved => verified += 1,
                    Some(_) => mismatched
                        .push((relocation.object_section.clone(), relocation.object_offset)),
                    None => unresolved += 1,
                }
            }
            None => {
                let section = exe_sections
                    .iter()
                    .find(|section| section.name() == Ok(relocation.object_section.as_str()));
                match section {
                    None => unresolved += 1,
                    Some(section) => {
                        let Ok(data) = section.uncompressed_data() else {
                            unresolved += 1;
                            continue;
                        };
                        // A located `applied_offset` pins the check to
                        // the exact field — an unreadable or mismatching
                        // slot is a real inconsistency, not a reason to
                        // scan again. Records without one (the fallback
                        // parse path never locates a slot) fall back to
                        // value existence in the section.
                        let found = match relocation.applied_offset {
                            Some(offset) => usize::try_from(offset)
                                .ok()
                                .and_then(|offset| data.get(offset..offset + 4))
                                .is_some_and(|bytes| {
                                    i64::from(i32::from_le_bytes(bytes.try_into().unwrap()))
                                        == resolved
                                }),
                            None => (0..data.len().saturating_sub(3)).any(|place| {
                                i64::from(i32::from_le_bytes(
                                    data[place..place + 4].try_into().unwrap(),
                                )) == resolved
                            }),
                        };
                        if found {
                            verified += 1;
                        } else {
                            mismatched.push((
                                relocation.object_section.clone(),
                                relocation.object_offset,
                            ));
                        }
                    }
                }
            }
        }
    }
    if !mismatched.is_empty() {
        checks.push(check(
            "relocation_applied_values",
            "inconsistent",
            format!("applied fields disagree with the record: {mismatched:?}"),
        ));
    } else if unresolved > 0 {
        checks.push(check(
            "relocation_applied_values",
            "unchecked",
            format!(
                "{verified}/{} applied values verified ({unresolved} unresolved)",
                record.relocations.len()
            ),
        ));
    } else {
        checks.push(check(
            "relocation_applied_values",
            "checked",
            format!("{verified} applied values verified"),
        ));
    }

    // llvm_ir_object_equivalence: direct LLVM-IR ↔ object comparison
    // over the shared source-model inputs (design memo §7 Step 3
    // slice). Recorded only; verdict/grade promotion stays deferred.
    match ir.equivalence {
        None => checks.push(check(
            "llvm_ir_object_equivalence",
            "unchecked",
            "no source model or LLVM IR available for direct comparison".to_owned(),
        )),
        Some(outcomes) => {
            // Outcomes: `equivalent` / `different` / `unknown` /
            // a skip reason (`unsupported_signature`, `unsupported_ir`,
            // `unsupported_instruction`, or a future `NotLiftable`
            // reason passed through verbatim). `unknown` means the
            // comparison ran and the solver could not decide — it is
            // not a skip and keeps the item `unchecked` (fail-closed;
            // DESIGN_LLVM_OBJECT_CORRESPONDENCE.md §3-7).
            let compared = outcomes
                .iter()
                .filter(|(_, outcome)| matches!(**outcome, "equivalent" | "different"))
                .count();
            let different: Vec<&String> = outcomes
                .iter()
                .filter(|(_, outcome)| **outcome == "different")
                .map(|(name, _)| name)
                .collect();
            let unknown: Vec<&String> = outcomes
                .iter()
                .filter(|(_, outcome)| **outcome == "unknown")
                .map(|(name, _)| name)
                .collect();
            let detail = outcomes
                .iter()
                .map(|(name, outcome)| format!("{name}: {outcome}"))
                .collect::<Vec<_>>()
                .join(", ");
            if !different.is_empty() {
                checks.push(check(
                    "llvm_ir_object_equivalence",
                    "inconsistent",
                    format!("divergent functions ({})", detail),
                ));
            } else if !unknown.is_empty() {
                checks.push(check(
                    "llvm_ir_object_equivalence",
                    "unchecked",
                    format!(
                        "{} function(s) attempted but undecidable ({detail})",
                        unknown.len()
                    ),
                ));
            } else if compared == 0 {
                checks.push(check(
                    "llvm_ir_object_equivalence",
                    "unchecked",
                    format!("no source-atom function liftable in the scalar-i64 subset ({detail})"),
                ));
            } else {
                checks.push(check(
                    "llvm_ir_object_equivalence",
                    "checked",
                    format!("{compared} liftable function(s) equivalent ({detail})"),
                ));
            }
        }
    }

    // hir_llvm_model_correspondence: the HIR-anchored per-atom model
    // comparison against the emitted IR. `checked` when every atom the
    // encoder admits proved equivalent (atoms outside the subset stay
    // honestly uncovered in `detail`), `inconsistent` on any
    // divergence, `unchecked` when nothing was compared.
    match ir.hir_llvm {
        Some(outcomes) if !outcomes.is_empty() => {
            let diverged: Vec<&String> = outcomes
                .iter()
                .filter(|(_, outcome)| **outcome == "inconsistent")
                .map(|(name, _)| name)
                .collect();
            let checked = outcomes
                .values()
                .filter(|outcome| **outcome == "checked")
                .count();
            let uncovered = outcomes.len() - checked - diverged.len();
            if !diverged.is_empty() {
                checks.push(check(
                    "hir_llvm_model_correspondence",
                    "inconsistent",
                    format!(
                        "{} of {} atoms diverge from their IR model: {diverged:?}",
                        diverged.len(),
                        outcomes.len(),
                    ),
                ));
            } else if checked == 0 {
                checks.push(check(
                    "hir_llvm_model_correspondence",
                    "unchecked",
                    format!("{} atoms outside the hir→llvm_ir leaf subset", uncovered,),
                ));
            } else {
                checks.push(check(
                    "hir_llvm_model_correspondence",
                    "checked",
                    format!(
                        "{checked} of {} atoms equivalent within the leaf subset ({uncovered} outside)",
                        outcomes.len(),
                    ),
                ));
            }
        }
        _ => checks.push(check(
            "hir_llvm_model_correspondence",
            "unchecked",
            "no source atoms or LLVM IR to compare".to_owned(),
        )),
    }

    // object_data_correspondence (design memo Step 3): every IR data
    // global's initializer bytes ↔ the like-named non-text symbol's
    // contents. `checked` when each derivable global matched — a
    // name-absent global still counts when an object data symbol
    // holds identical bytes, because LLVM merges identical
    // `unnamed_addr` constants — `inconsistent` on byte divergence,
    // `unchecked` when a global is missing from the object or its
    // initializer is outside the derivable shapes. Kept out of the
    // llvm_ir→object_code edge's promotion gates: the relation name
    // covers instruction selection only (memo §7(c)).
    let object_data = object_data_surface(object_bytes).ok();
    match (ir.data_globals, object_data.as_ref()) {
        (None, _) | (_, None) => checks.push(check(
            "object_data_correspondence",
            "unchecked",
            "IR data globals or object data symbols unavailable".to_owned(),
        )),
        (Some(globals), Some(surface)) if globals.is_empty() => {
            // Anonymous data sections carry bytes that no object
            // symbol names; with no IR globals they cannot be matched,
            // so only a fully empty data surface verifies.
            let anonymous_bytes: usize = surface.section_contents.values().map(Vec::len).sum();
            checks.push(if surface.symbols.is_empty() && anonymous_bytes == 0 {
                check(
                    "object_data_correspondence",
                    "checked",
                    "no IR data globals or object data symbols to compare".to_owned(),
                )
            } else {
                check(
                    "object_data_correspondence",
                    "unchecked",
                    format!(
                        "{} object data symbols and {anonymous_bytes} anonymous data bytes with no IR globals to compare",
                        surface.symbols.len()
                    ),
                )
            });
        }
        (Some(globals), Some(surface)) => {
            let symbols = &surface.symbols;
            // A name-absent global still verifies when its exact
            // initializer bytes sit in a data section: LLVM merges
            // identical `unnamed_addr` constants under whichever name
            // survives, and emits anonymous constants with no symbol
            // at all (`.rodata.str1.1` is symbol-free).
            let bytes_recorded = |expected: &[u8]| {
                !expected.is_empty()
                    && (symbols.values().any(|symbol| symbol.contents() == expected)
                        || surface.section_contents.values().any(|data| {
                            data.windows(expected.len())
                                .any(|window| window == expected)
                        }))
            };
            let mut compared = 0usize;
            let mut diverged = 0usize;
            let mut unresolved = 0usize;
            let detail = globals
                .iter()
                .map(|(name, initializer)| match initializer {
                    None => {
                        unresolved += 1;
                        format!("{name}: underivable")
                    }
                    Some(expected) => match symbols.get(name) {
                        Some(symbol) if symbol.contents() == *expected => {
                            compared += 1;
                            format!("{name}: equivalent ({})", symbol.section_name)
                        }
                        Some(symbol) => {
                            diverged += 1;
                            format!("{name}: different ({})", symbol.section_name)
                        }
                        None if bytes_recorded(expected) => {
                            compared += 1;
                            format!("{name}: merged")
                        }
                        None => {
                            unresolved += 1;
                            format!("{name}: missing")
                        }
                    },
                })
                .collect::<Vec<_>>()
                .join(", ");
            if diverged > 0 {
                checks.push(check(
                    "object_data_correspondence",
                    "inconsistent",
                    format!("data initializers disagree with object contents ({detail})"),
                ));
            } else if unresolved > 0 {
                checks.push(check(
                    "object_data_correspondence",
                    "unchecked",
                    format!(
                        "{compared} of {} IR data globals verified ({detail})",
                        globals.len()
                    ),
                ));
            } else {
                checks.push(check(
                    "object_data_correspondence",
                    "checked",
                    format!("{compared} IR data globals verified ({detail})"),
                ));
            }
        }
    }

    // object_symbol_attribute_correspondence (design memo Step 3'):
    // each defined named IR entity's linkage ↔ the like-named object
    // symbol's binding class (global/local/weak). `checked` when every
    // derivable entity matched — `inconsistent` on a binding
    // mismatch, `unchecked` when a symbol is missing, multiply
    // defined, or its linkage is outside the derivable subset.
    // `unnamed_addr` entities are excluded: LLVM may merge or drop
    // their symbols so no binding claim survives — for globals the
    // bytes are covered by object_data_correspondence; unnamed_addr
    // functions simply carry no attribute claim. Calling convention is
    // not representable in ELF symbols, so binding is the honest
    // subset. Kept out of the llvm_ir→object_code edge's promotion
    // gate: the relation covers instruction selection only.
    let object_bindings = object_symbol_binding_surface(object_bytes).ok();
    match (ir.symbol_attributes, object_bindings.as_ref()) {
        (None, _) | (_, None) => checks.push(check(
            "object_symbol_attribute_correspondence",
            "unchecked",
            "IR symbol attributes or object symbol bindings unavailable".to_owned(),
        )),
        (Some(attributes), Some(surface)) if attributes.expected.is_empty() => {
            checks.push(if surface.bindings.is_empty() {
                check(
                    "object_symbol_attribute_correspondence",
                    "checked",
                    format!(
                        "no derivable IR entities or object symbols to compare; {} unnamed_addr entities excluded",
                        attributes.unnamed_addr_excluded
                    ),
                )
            } else {
                check(
                    "object_symbol_attribute_correspondence",
                    "unchecked",
                    format!(
                        "{} object symbols with no derivable IR entities to compare",
                        surface.bindings.len()
                    ),
                )
            });
        }
        (Some(attributes), Some(surface)) => {
            let mut compared = 0usize;
            let mut diverged = 0usize;
            let mut unresolved = 0usize;
            let mut detail = attributes
                .expected
                .iter()
                .map(|(name, expected)| match expected {
                    None => {
                        unresolved += 1;
                        format!("{name}: underivable")
                    }
                    Some(expected_binding) if surface.conflicts.contains(name) => {
                        unresolved += 1;
                        format!("{name}: conflicting symbols")
                    }
                    Some(expected_binding) => match surface.bindings.get(name) {
                        Some(&binding) if binding == *expected_binding => {
                            compared += 1;
                            format!("{name}: equivalent ({binding})")
                        }
                        Some(&binding) => {
                            diverged += 1;
                            format!("{name}: different (expected {expected_binding}, object {binding})")
                        }
                        None => {
                            unresolved += 1;
                            format!("{name}: missing")
                        }
                    },
                })
                .collect::<Vec<_>>();
            if attributes.unnamed_addr_excluded > 0 {
                detail.push(format!(
                    "{} unnamed_addr entities excluded",
                    attributes.unnamed_addr_excluded
                ));
            }
            let detail = detail.join(", ");
            if diverged > 0 {
                checks.push(check(
                    "object_symbol_attribute_correspondence",
                    "inconsistent",
                    format!("symbol bindings disagree with IR linkage ({detail})"),
                ));
            } else if unresolved > 0 {
                checks.push(check(
                    "object_symbol_attribute_correspondence",
                    "unchecked",
                    format!(
                        "{compared} of {} IR entities verified ({detail})",
                        attributes.expected.len()
                    ),
                ));
            } else {
                checks.push(check(
                    "object_symbol_attribute_correspondence",
                    "checked",
                    format!("{compared} IR entities verified ({detail})"),
                ));
            }
        }
    }

    let any_inconsistent = checks
        .iter()
        .any(|check| check["result"].as_str() == Some("inconsistent"));
    let all_checked = checks
        .iter()
        .all(|check| check["result"].as_str() == Some("checked"));
    let status = if all_checked {
        "checked_correspondence"
    } else if any_inconsistent {
        "inconsistent_correspondence"
    } else {
        "unchecked_not_translation_validated"
    };
    serde_json::json!({
        "status": status,
        "checks": checks,
    })
}

/// Map an `IrObjectEquivalence` verdict to the recorded outcome:
/// `unknown` for an attempted-but-undecidable comparison
/// (`solver_unknown` — fail-closed, never a skip), and the verbatim
/// `NotLiftable` reason for anything the subset could not compare.
fn ir_object_equivalence_outcome(equivalence: machine_tv::IrObjectEquivalence) -> &'static str {
    match equivalence {
        machine_tv::IrObjectEquivalence::Equivalent => "equivalent",
        machine_tv::IrObjectEquivalence::Different => "different",
        machine_tv::IrObjectEquivalence::NotLiftable("solver_unknown") => "unknown",
        machine_tv::IrObjectEquivalence::NotLiftable(reason) => reason,
    }
}

/// Direct LLVM-IR ↔ object-function comparison over the shared
/// `model.inputs` (design memo §7 Step 3 slice): per function,
/// `"equivalent"`, `"different"`, `"unknown"` (the comparison ran and
/// the solver could not decide — fail-closed, never counted as
/// skipped), or the verbatim `NotLiftable` reason (`unsupported_signature`,
/// `unsupported_ir`, `unsupported_instruction`) when the function is
/// outside the admitted subset or its IR could not be encoded.
/// `None` when the checked source snapshot or LLVM IR is
/// unavailable — the item then records `unchecked`.
fn llvm_ir_object_equivalence_results(
    data: &ObjectRecheckData,
    record: &ObjectTranslationRecord,
) -> Option<BTreeMap<String, &'static str>> {
    let source_text = data.source_text.as_deref()?;
    if data.llvm_bytes.is_empty() {
        return None;
    }
    let mut config = z3::Config::new();
    config.set_timeout_msec(u64::from(machine_tv::TIMEOUT_MS));
    config.set_param_value("rlimit", &machine_tv::RESOURCE_LIMIT.to_string());
    let context = z3::Context::new(&config);
    let models =
        semantic_contract::encode_pure_i64_module(source_text, &context, true, data.admission)
            .ok()?;
    // Parse the LLVM module once and share it across functions; models
    // outside the scalar-i64 signature subset skip before the encode.
    let mut parsed = None;
    let mut outcomes = BTreeMap::new();
    for model in &models {
        let symbol = tv::native_function_name(&model.function_name).to_owned();
        if !record
            .functions
            .iter()
            .any(|function| function.ir_function == symbol)
        {
            continue;
        }
        let Some((address, bytes)) = object_function_bytes(&data.object_bytes, &symbol) else {
            continue;
        };
        let outcome = if !machine_tv::ir_object_signature_liftable(model) {
            "unsupported_signature"
        } else {
            if parsed.is_none() {
                parsed = unsafe { tv::parse_ir_module_for_comparison(&data.llvm_bytes) }.ok();
            }
            let encoded = parsed.as_ref().and_then(|module| unsafe {
                module
                    .function(&symbol)
                    .and_then(|raw| {
                        tv::encode_parsed_function_for_comparison(raw, model, false, &context)
                    })
                    .ok()
            });
            let call_targets = data
                .object_surface
                .call_targets
                .get(&symbol)
                .cloned()
                .unwrap_or_default();
            match encoded {
                None => "unsupported_ir",
                Some((ir_returns, ir_definedness)) => {
                    ir_object_equivalence_outcome(machine_tv::check_function_against_ir(
                        &bytes,
                        address,
                        model,
                        &ir_returns,
                        &ir_definedness,
                        &call_targets,
                        &context,
                    ))
                }
            }
        };
        outcomes.insert(symbol, outcome);
    }
    Some(outcomes)
}

/// Per-atom `hir → llvm_ir` model-correspondence outcomes for the
/// recheck descriptor (`docs/DESIGN_HIR_LLVM_CORRESPONDENCE.md`):
/// each parsed atom is lowered against the same module env the emit
/// used and compared with the emitted LLVM IR by
/// `validate_hir_pure_i64_leaf`. `checked` marks an atom proved
/// equivalent within the leaf subset, `inconsistent` a concrete
/// divergence, and `unchecked` an atom outside the subset — the edge
/// claims the method, never coverage it does not have. `None` when
/// no source text or IR bytes are on hand to run it at all.
fn hir_llvm_atom_outcomes(data: &ObjectRecheckData) -> Option<BTreeMap<String, &'static str>> {
    let source = data.source_text.as_deref()?;
    let llvm_ir = std::str::from_utf8(&data.llvm_bytes).ok()?;
    let mut env = verification::ModuleEnv::default();
    let mut atoms = Vec::new();
    let mut struct_defs = Vec::new();
    for item in parser::parse_module(source) {
        match &item {
            parser::Item::TypeDef(typedef) => env.register_type(typedef),
            parser::Item::StructDef(def) => env.register_struct(def),
            parser::Item::EnumDef(def) => env.register_enum(def),
            parser::Item::Atom(atom) => env.register_atom(atom),
            _ => {}
        }
        match item {
            parser::Item::Atom(atom) => atoms.push(atom),
            parser::Item::StructDef(def) => struct_defs.push(def),
            _ => {}
        }
    }
    let (module_header, members) = llvm_module_members(llvm_ir);
    // Call-site leaf shapes come from the declared signatures, so a
    // composite parameter or result flattens to its leaf slots the way
    // the IR `call` encoding splices them — a callee absent here keeps
    // the legacy all-scalar assumption. Refined aliases resolve to
    // their machine base (`Pos` → `i64`), matching how the IR operand
    // carries the leaf.
    let calls: BTreeMap<String, semantic_contract::HirCallLeafSig> = atoms
        .iter()
        .map(|atom| {
            (
                atom.name.clone(),
                semantic_contract::HirCallLeafSig {
                    params: atom
                        .params
                        .iter()
                        .map(|param| {
                            param
                                .type_name
                                .as_deref()
                                .map(|ty| env.resolve_base_type(ty))
                        })
                        .collect(),
                    result: atom
                        .return_type
                        .as_deref()
                        .map(|ty| env.resolve_base_type(ty)),
                },
            )
        })
        .collect();
    // Generic struct declarations specialize per instantiation at
    // build time; the recheck applies the same substitution from each
    // atom's own signature (`Pair<i64>` → field `T` binds `i64`) so a
    // concrete all-i64 instantiation is not dropped by the raw `T`.
    let mut outcomes = BTreeMap::new();
    for atom in &atoms {
        let structs = specialized_struct_map(atom, &struct_defs, &calls);
        // The bundle emitter renames the user `main` atom to
        // `__mumei_user_main` and adds a C `main` wrapper — the model
        // comparison must target the user member, not the wrapper.
        let original_name = atom.name.clone();
        let (member_name, atom) = if atom.name == "main" {
            let mut renamed = atom.clone();
            renamed.name = "__mumei_user_main".to_owned();
            ("__mumei_user_main".to_owned(), renamed)
        } else {
            (atom.name.clone(), atom.clone())
        };
        // `check_module` gates every member it sees; the per-atom
        // comparison only owes the atom's own call graph. Slice the
        // emitted module down to that closure so an unrelated member
        // outside the subset — the C `main` wrapper, or a sibling
        // atom the encoder does not admit — does not fail this atom's
        // check. A callee outside the subset still fails closed.
        let mut keep = BTreeSet::from([member_name.clone()]);
        let mut stack = vec![member_name.clone()];
        while let Some(name) = stack.pop() {
            if let Some((_, calls)) = members.get(&name) {
                for call in calls {
                    if keep.insert(call.clone()) {
                        stack.push(call.clone());
                    }
                }
            }
        }
        let subset_ir = format!(
            "{}{}",
            module_header,
            members
                .iter()
                .filter(|(name, _)| keep.contains(*name))
                .map(|(_, (text, _))| text.clone())
                .collect::<String>(),
        );
        let hir_atom = hir::lower_atom_to_hir_with_env(&atom, Some(&env));
        let outcome = match tv::validate_hir_pure_i64_leaf(
            &hir_atom,
            &structs,
            &calls,
            &subset_ir,
            true,
            tv::Options::default(),
        ) {
            Ok(tv::Outcome::EquivalentWithinProfile) => "checked",
            Ok(tv::Outcome::Different | tv::Outcome::UndefinedBehavior) => "inconsistent",
            _ => "unchecked",
        };
        outcomes.insert(original_name, outcome);
    }
    Some(outcomes)
}

/// The `structs` map for one atom: plain definitions as declared,
/// generic definitions specialized by the instantiation the atom's
/// own signature uses (`p: Pair<i64>` binds `T` to `i64`) — and by
/// every callee signature, since a generic instantiation can appear
/// only in a callee's signature (`caller() { mk3(1, 2, 3) }` where
/// `mk3` returns `Triple<i64>`) while the call-site flattening
/// resolves the callee's type strings verbatim against this map. A
/// def the atom does not instantiate concretely keeps its raw field
/// types and fails the encoder's leaf-subset admission honestly.
fn specialized_struct_map(
    atom: &parser::Atom,
    defs: &[parser::StructDef],
    calls: &BTreeMap<String, semantic_contract::HirCallLeafSig>,
) -> BTreeMap<String, Vec<(String, String)>> {
    // `Name<arg, ...>` → (Name, [args]); signature type strings are
    // parser-produced, so a shallow split is a reliable shape.
    let instantiation = |type_name: &str| -> Option<(String, Vec<String>)> {
        let (name, rest) = type_name.split_once('<')?;
        let args = rest
            .strip_suffix('>')?
            .split(',')
            .map(|arg| arg.trim().to_owned())
            .collect();
        Some((name.to_owned(), args))
    };
    // The encoder admits a parameter by looking its type string up
    // verbatim, so each entry keys on the whole instantiation
    // (`Pair<i64>`), not the bare definition name. Nested arguments
    // (`Box<Pair<i64>>` needs `Pair<i64>` admitted too) register
    // recursively.
    let mut instantiations = BTreeMap::<String, Vec<String>>::new();
    let mut worklist: Vec<String> = atom
        .params
        .iter()
        .filter_map(|param| param.type_name.clone())
        .chain(atom.return_type.clone())
        .chain(calls.values().flat_map(|sig| {
            sig.params
                .iter()
                .flatten()
                .cloned()
                .chain(sig.result.clone())
        }))
        .collect();
    while let Some(type_name) = worklist.pop() {
        if let Some((name, args)) = instantiation(&type_name) {
            worklist.extend(args.iter().cloned());
            instantiations.entry(type_name).or_insert_with(|| {
                defs.iter()
                    .find(|def| def.name == name)
                    .map(|_| args)
                    .unwrap_or_default()
            });
        }
    }
    let mut map = BTreeMap::<String, Vec<(String, String)>>::new();
    for def in defs {
        map.insert(
            def.name.clone(),
            def.fields
                .iter()
                .map(|field| (field.name.clone(), field.type_name.clone()))
                .collect(),
        );
    }
    for (type_name, args) in &instantiations {
        let Some(def) = defs.iter().find(|def| {
            instantiation(type_name)
                .map(|(name, _)| name == def.name)
                .unwrap_or(false)
        }) else {
            continue;
        };
        if args.len() != def.type_params.len() {
            continue;
        }
        map.insert(
            type_name.clone(),
            def.fields
                .iter()
                .map(|field| {
                    let type_name = def
                        .type_params
                        .iter()
                        .position(|param| param == &field.type_name)
                        .map(|index| args[index].clone())
                        .unwrap_or_else(|| field.type_name.clone());
                    (field.name.clone(), type_name)
                })
                .collect(),
        );
    }
    map
}

/// Splits an emitted LLVM IR module into its header (everything
/// outside `define` blocks — target, declarations, attributes) and
/// each `define` block keyed by function name with its direct `call`
/// targets. The IR is machine-generated and never hand-edited, so
/// `define`/`}` line boundaries are reliable delimiters.
fn llvm_module_members(llvm_ir: &str) -> (String, BTreeMap<String, (String, Vec<String>)>) {
    let mut header = String::new();
    let mut members = BTreeMap::<String, (String, Vec<String>)>::new();
    let mut lines = llvm_ir.lines();
    while let Some(line) = lines.next() {
        if !line.starts_with("define ") {
            // Global definitions (`@name = constant ...`) belong to the
            // member that referenced them — an unrelated member's string
            // literal in the header would fail every atom's module check.
            if !line.starts_with('@') {
                header.push_str(line);
                header.push('\n');
            }
            continue;
        }
        let name = line
            .split('@')
            .nth(1)
            .and_then(|rest| rest.split('(').next())
            .unwrap_or_default()
            .to_owned();
        let mut text = String::from(line);
        text.push('\n');
        let mut calls = Vec::new();
        for body_line in lines.by_ref() {
            text.push_str(body_line);
            text.push('\n');
            if body_line.contains('@') {
                if let Some(target) = body_line
                    .split('@')
                    .nth(1)
                    .and_then(|rest| rest.split(['(', ' ', ',']).next())
                {
                    calls.push(target.to_owned());
                }
            }
            if body_line == "}" {
                break;
            }
        }
        members.insert(name, (text, calls));
    }
    (header, members)
}

/// Correspondence check block shared by the JSON descriptors and the
/// text report: `{"status", "checks": [{item, result, detail}]}` with
/// `checked` / `unchecked` / `inconsistent` results.
fn object_translation_correspondence_json(
    data: &ObjectRecheckData,
    record: &ObjectTranslationRecord,
) -> serde_json::Value {
    let ir_equivalence = llvm_ir_object_equivalence_results(data, record);
    let hir_llvm = hir_llvm_atom_outcomes(data);
    let ir_data_globals = tv::ir_data_globals(&data.llvm_bytes).ok();
    let ir_symbol_attributes = tv::ir_symbol_attributes(&data.llvm_bytes).ok();
    object_translation_correspondence_checks(
        &data.ir_surface,
        data.executable_bytes.as_slice(),
        record,
        IrCorrespondenceInputs {
            equivalence: ir_equivalence.as_ref(),
            hir_llvm: hir_llvm.as_ref(),
            data_globals: ir_data_globals.as_ref(),
            symbol_attributes: ir_symbol_attributes.as_ref(),
        },
        data.object_bytes.as_slice(),
    )
}

/// Human-facing report lines for the correspondence block (design memo
/// remaining item): the aggregate status plus one diagnostic line per
/// `inconsistent` item so a divergence is named on the text path —
/// before this it was only visible in `--json` output.
fn object_translation_correspondence_report_lines(
    correspondence: &serde_json::Value,
) -> Vec<String> {
    let status = correspondence["status"].as_str().unwrap_or("unknown");
    let checks = correspondence["checks"].as_array();
    let checked = checks
        .map(|checks| {
            checks
                .iter()
                .filter(|check| check["result"].as_str() == Some("checked"))
                .count()
        })
        .unwrap_or(0);
    let total = checks.map(|checks| checks.len()).unwrap_or(0);
    let mut lines = vec![format!(
        "Object translation correspondence: {status} ({checked} of {total} items checked)."
    )];
    if let Some(checks) = checks {
        for check in checks {
            if check["result"].as_str() == Some("inconsistent") {
                let item = check["item"].as_str().unwrap_or("unknown");
                let detail = check["detail"].as_str().unwrap_or("no detail recorded");
                lines.push(format!("  inconsistent: {item} — {detail}"));
            }
        }
    }
    lines
}

/// Correspondence block evaluated for the text report of a command
/// that already loaded `ObjectRecheckData` (recheck-machine-tv and
/// accept-object). Runs the same checks as the JSON descriptor.
fn object_translation_correspondence_text(data: &ObjectRecheckData) -> Vec<String> {
    let record = data
        .object_translation
        .clone()
        .unwrap_or_else(|| object_translation_parse(&data.object_bytes, &data.executable_bytes));
    let correspondence = object_translation_correspondence_json(data, &record);
    object_translation_correspondence_report_lines(&correspondence)
}

fn object_translation_json(
    data: &ObjectRecheckData,
    machine_tv_validation: &str,
    record: &ObjectTranslationRecord,
    correspondence: &serde_json::Value,
) -> serde_json::Value {
    let object_sha256 = {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(&data.object_bytes))
    };
    let executable_sha256 = {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(&data.executable_bytes))
    };
    let link_validation = if data.object_link_correspondence {
        "checked_object_link_correspondence"
    } else {
        "unchecked_not_translation_validated"
    };
    let status = if correspondence["status"].as_str() == Some("checked_correspondence") {
        "checked_correspondence"
    } else {
        "metadata_only"
    };
    // The `hir → llvm_ir` and `llvm_ir → object_code` edges report
    // their check items' aggregates: `checked_correspondence` only
    // when the item checked, otherwise the honest fail-closed
    // vocabulary. The item detail names divergent or uncovered atoms.
    let check_item_result = |item: &str| {
        correspondence["checks"]
            .as_array()
            .and_then(|checks| {
                checks
                    .iter()
                    .find(|check| check["item"].as_str() == Some(item))
            })
            .and_then(|check| check["result"].as_str())
    };
    let hir_llvm_result =
        edge_result_for_item_result(check_item_result("hir_llvm_model_correspondence"));
    // `llvm_ir → object_code` promotion conditions
    // (DESIGN_LLVM_OBJECT_CORRESPONDENCE.md §4): the direct comparison
    // item must check AND every gate it requires must actually have
    // run — function enumeration, the relocation record items, the
    // link correspondence and the canonical argv reproduction. The
    // symbol and call-edge correspondences are upstream hard gates:
    // a descriptor only exists after they passed.
    let llvm_ir_object_result = llvm_ir_object_edge_result(
        check_item_result("llvm_ir_object_equivalence"),
        &LLVM_IR_OBJECT_GATE_ITEMS
            .iter()
            .map(|item| check_item_result(item))
            .collect::<Vec<_>>(),
        data.object_link_correspondence,
        data.canonical_argv_hash.is_some(),
    );
    serde_json::json!({
        "status": status,
        "transformations": [
            {
                // `validation` names the IR↔object model comparison the
                // recheck runs per liftable function; `result` reports
                // `checked_correspondence` only when the
                // `llvm_ir_object_equivalence` item checked and the
                // memo's promotion conditions (function enumeration,
                // relocation record, link correspondence, canonical
                // argv) all ran — a report whose functions are outside
                // the subset, diverged, or undecidable does not claim
                // this edge was validated.
                "from": "llvm_ir",
                "to": "object_code",
                "relation": "llc_instruction_selection_register_allocation",
                "output_identity": object_sha256,
                "validation": "checked_llvm_ir_object_model_correspondence",
                "result": llvm_ir_object_result,
            },
            {
                "from": "object_code",
                "to": "linked_executable",
                "relation": "static_link_relocation_placement",
                "output_identity": executable_sha256,
                "validation": link_validation,
                // `validation` names the method applied; the outcome of
                // the link-correspondence check is reported separately
                // so the two are never conflated in one field.
                "result": if data.object_link_correspondence {
                    "checked_correspondence"
                } else {
                    "unchecked_not_translation_validated"
                },
            },
            {
                // Machine code is evaluated in memory during the
                // machine-TV check and never persisted, so there is no
                // artifact digest to pin for this edge. Appended last
                // so the two digest-bearing entries keep the indices
                // consumers already read (0: object, 1: executable);
                // `validation` mirrors the check block's verdict —
                // a report with residual unverified functions does
                // not claim this edge was checked.
                "from": "source_atom",
                "to": "machine_code",
                "relation": "machine_code_translation_validation",
                "output_identity": null,
                "validation": machine_tv_validation,
            },
            {
                // `validation` names the HIR-anchored model comparison
                // the recheck runs per atom; `result` mirrors the
                // `hir_llvm_model_correspondence` check item so a
                // report whose atoms are outside the subset — or
                // whose IR diverged — does not claim this edge was
                // validated. Appended last; the digest-bearing
                // indices (0, 1) do not move.
                "from": "hir",
                "to": "llvm_ir",
                "relation": "existing_llvm_emitter",
                "output_identity": null,
                "validation": "checked_hir_llvm_model_correspondence",
                "result": hir_llvm_result,
            },
        ],
        "functions": record
            .functions
            .iter()
            .map(|function| serde_json::json!({
                "ir_function": function.ir_function,
                "object_symbol": function.object_symbol,
                "object_section": function.object_section,
                "object_offset": function.object_offset,
                "object_size": function.object_size,
                "executable_address": function.executable_address,
            }))
            .collect::<Vec<_>>(),
        "relocations": record
            .relocations
            .iter()
            .map(|relocation| serde_json::json!({
                "object_section": relocation.object_section,
                "object_offset": relocation.object_offset,
                "kind": relocation.kind,
                "target_symbol": relocation.target_symbol,
                "addend": relocation.addend,
                "resolved_value": relocation.resolved_value,
                "applied_offset": relocation.applied_offset,
                "assumption": relocation.assumption,
            }))
            .collect::<Vec<_>>(),
        "correspondence": correspondence,
    })
}

/// Map a check item's `result` to the vocabulary a transformation
/// edge reports: `inconsistent` means validation that ran and failed —
/// not an edge that never ran, so the two stay apart; a missing item
/// or any other result is fail-closed `unchecked`.
fn edge_result_for_item_result(item_result: Option<&str>) -> &'static str {
    match item_result {
        Some("checked") => "checked_correspondence",
        Some("inconsistent") => "inconsistent_correspondence",
        _ => "unchecked_not_translation_validated",
    }
}

/// Correspondence items the `llvm_ir → object_code` edge requires
/// before it may report `checked_correspondence`
/// (DESIGN_LLVM_OBJECT_CORRESPONDENCE.md §4-1/§4-4): the function
/// enumeration and the relocation record items.
const LLVM_IR_OBJECT_GATE_ITEMS: [&str; 6] = [
    "function_coverage",
    "function_count",
    "relocation_targets",
    "relocation_bounds",
    "relocation_resolved_values",
    "relocation_applied_values",
];

/// `llvm_ir → object_code` edge result per the design memo's promotion
/// conditions (§4): `checked_correspondence` only when the
/// `llvm_ir_object_equivalence` item checked, every gate item checked,
/// and the link correspondence and canonical argv reproduction ran.
/// Any `inconsistent` item is validation that ran and failed; a
/// missing, `unchecked` or `unknown` item keeps the edge fail-closed.
fn llvm_ir_object_edge_result(
    equivalence_item: Option<&str>,
    gate_items: &[Option<&str>],
    object_link_correspondence: bool,
    canonical_argv_compared: bool,
) -> &'static str {
    if equivalence_item == Some("inconsistent") || gate_items.contains(&Some("inconsistent")) {
        return "inconsistent_correspondence";
    }
    if equivalence_item == Some("checked")
        && gate_items.iter().all(|result| *result == Some("checked"))
        && object_link_correspondence
        && canonical_argv_compared
    {
        return "checked_correspondence";
    }
    "unchecked_not_translation_validated"
}

fn object_machine_tv_source_atom_count(outcomes: &BTreeMap<String, FunctionOutcome>) -> usize {
    outcomes
        .iter()
        .filter(|(name, outcome)| {
            name.as_str() != "main"
                && !matches!(
                    **outcome,
                    FunctionOutcome::OutsideProfile("not_a_source_atom")
                )
        })
        .count()
}

fn object_machine_tv_acceptance_scope(object_link_correspondence: bool) -> &'static str {
    if object_link_correspondence {
        mumei_core::object_acceptance::SCOPE_WITH_OBJECT_LINK
    } else {
        mumei_core::object_acceptance::SCOPE_WITHOUT_OBJECT_LINK
    }
}

fn object_machine_tv_acceptance_checks(
    family: RecordFamily,
    canonical_argv_compared: bool,
    object_link_correspondence: bool,
    entry_wrapper_equivalent: bool,
    coverage: &(Vec<&'static str>, Vec<&'static str>),
) -> (Vec<&'static str>, Vec<&'static str>) {
    let mut checks_performed = vec!["bundle_identity_binding"];
    if family == RecordFamily::Multifile {
        checks_performed.push("pinned_member_merge_rederivation");
    }
    checks_performed.extend([
        "fresh_same_profile_source_recheck",
        "llvm_snapshot_translation_recheck",
        "toolchain_object_executable_reproduction",
    ]);
    if canonical_argv_compared {
        checks_performed.push("canonical_link_argument_vector_reproduction");
    }
    checks_performed.extend([
        "object_symbol_correspondence",
        "object_call_edge_correspondence",
    ]);
    if object_link_correspondence {
        checks_performed.push("object_link_correspondence");
    }
    checks_performed.extend(OBJECT_TRANSLATION_ACCEPTANCE_ITEMS);
    checks_performed.extend(coverage.0.iter().copied());
    checks_performed.push("object_source_machine_code_correspondence");

    let mut checks_not_performed = vec![
        "producer_authenticity",
        "descriptor_authenticity",
        "historical_execution",
    ];
    if !canonical_argv_compared {
        checks_not_performed.push("canonical_link_argument_vector_reproduction");
    }
    checks_not_performed.push("raw_link_argument_vector_reproduction");
    if !object_link_correspondence {
        checks_not_performed.push("object_link_correspondence");
    }
    if !entry_wrapper_equivalent {
        checks_not_performed.push("object_entry_wrapper_semantics");
    }
    checks_not_performed.extend([
        "register_convention_validation",
        "optimizer_semantics",
        "abi_runtime_semantics",
        "os_exit_status_observation",
        "os_stdout_observation",
        "os_stderr_observation",
        "runtime_observation_semantics",
    ]);
    checks_not_performed.extend(coverage.1.iter().copied());
    checks_not_performed.extend(["grade_promotion", "lean_kernel_checks"]);

    let mut unique_performed = Vec::with_capacity(checks_performed.len());
    for check in checks_performed {
        if !unique_performed.contains(&check) {
            unique_performed.push(check);
        }
    }
    let mut unique_not_performed = Vec::with_capacity(checks_not_performed.len());
    for check in checks_not_performed {
        if !unique_performed.contains(&check) && !unique_not_performed.contains(&check) {
            unique_not_performed.push(check);
        }
    }
    (unique_performed, unique_not_performed)
}

struct AcceptedObject {
    data: ObjectRecheckData,
    evaluation: MachineTvEvaluation,
    record: ObjectTranslationRecord,
    correspondence: serde_json::Value,
    canonical_argv_hash: String,
}

enum ObjectAcceptance {
    Accepted(Box<AcceptedObject>),
    Inconclusive(String),
}

/// The `accept-object` decision: the artifact, object-correspondence and
/// machine-TV pipeline, then every acceptance gate of
/// DESIGN_OBJECT_ACCEPTANCE.md §4. Differences are errors; anything
/// undecided is `Inconclusive` and never yields a descriptor.
fn evaluate_object_acceptance(
    bundle: &Path,
    source: &Path,
    expected_build_id: &str,
) -> Result<ObjectAcceptance, BundleError> {
    let data = match recheck_object_pipeline(bundle, source, expected_build_id, true)? {
        ObjectRecheckPipeline::Different(message) => return Err(BundleError(message)),
        ObjectRecheckPipeline::Inconclusive(message) => {
            return Ok(ObjectAcceptance::Inconclusive(message.to_owned()))
        }
        ObjectRecheckPipeline::Rechecked(data) => *data,
    };
    let evaluation = match evaluate_machine_tv(&data) {
        Ok(evaluation) => evaluation,
        Err(message) => return Ok(ObjectAcceptance::Inconclusive(message.to_owned())),
    };
    match machine_tv_acceptance_decision(&evaluation.outcomes, evaluation.entry_wrapper_applicable)
    {
        ObjectMachineTvAcceptanceDecision::Accepted => {}
        ObjectMachineTvAcceptanceDecision::Different => {
            return Err(BundleError(
                "reproduced object machine code differs from the verified source",
            ))
        }
        ObjectMachineTvAcceptanceDecision::Inconclusive => {
            return Ok(ObjectAcceptance::Inconclusive(
                "Machine-code TV inconclusive; not every source-atom function was proved equivalent"
                    .to_owned(),
            ))
        }
    }
    // DESIGN_OBJECT_ACCEPTANCE.md §4: acceptance additionally requires the
    // canonical argv reproduction and every correspondence item checked —
    // the machine-TV gate alone must not carry `accepted` (Step 2).
    let Some(canonical_argv_hash) = data.canonical_argv_hash.clone() else {
        return Ok(ObjectAcceptance::Inconclusive(
            "Canonical link argument vector not reproduced".to_owned(),
        ));
    };
    let record = data
        .object_translation
        .clone()
        .unwrap_or_else(|| object_translation_parse(&data.object_bytes, &data.executable_bytes));
    let correspondence = object_translation_correspondence_json(&data, &record);
    let empty: Vec<serde_json::Value> = Vec::new();
    let checks = correspondence["checks"].as_array().unwrap_or(&empty);
    let items_where = |keep: fn(Option<&str>) -> bool| {
        checks
            .iter()
            .filter(|check| keep(check["result"].as_str()))
            .filter_map(|check| check["item"].as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match object_translation_acceptance_decision(checks) {
        ObjectMachineTvAcceptanceDecision::Accepted => {}
        ObjectMachineTvAcceptanceDecision::Different => {
            return Err(BundleError(Box::leak(
                format!(
                    "object translation correspondence inconsistent with the pinned LLVM IR ({})",
                    items_where(|result| result == Some("inconsistent"))
                )
                .into_boxed_str(),
            )))
        }
        ObjectMachineTvAcceptanceDecision::Inconclusive => {
            return Ok(ObjectAcceptance::Inconclusive(format!(
                "Object translation correspondence not fully checked ({})",
                items_where(|result| result != Some("checked"))
            )))
        }
    }
    Ok(ObjectAcceptance::Accepted(Box::new(AcceptedObject {
        data,
        evaluation,
        record,
        correspondence,
        canonical_argv_hash,
    })))
}

/// Consumer-side replay of a saved `accept-object --json` descriptor: the
/// descriptor is compared with a fresh acceptance of the same pinned inputs,
/// never trusted for its own claims. The fingerprint is checked first so a
/// descriptor for other attempt inputs is reported as unbound rather than
/// as a field-level difference.
fn recheck_object_acceptance(
    bundle: &Path,
    source: &Path,
    descriptor: &Path,
    expected_build_id: &str,
    signature: &crate::cli::SignatureArgs,
    json_output: bool,
) -> Result<(), BundleError> {
    if !is_digest(expected_build_id) {
        return Err(BundleError("invalid expected build ID"));
    }
    let saved_bytes = read_regular(descriptor, MAX_EVIDENCE_BYTES)?;
    let saved = object_acceptance::decode(&saved_bytes)?;
    let descriptor_digest = producer_signature::subject_digest(&saved_bytes);
    let signature_acceptance = super::evidence::load_signature_acceptance(
        super::evidence::signature_options(signature),
        &descriptor_digest,
        None,
    )
    .map_err(BundleError)?;
    if signature_acceptance
        .as_ref()
        .is_some_and(|acceptance| !acceptance.binding.failed_fields().is_empty())
    {
        return Err(BundleError(
            "saved object acceptance descriptor producer signature binding failed",
        ));
    }
    let accepted = match evaluate_object_acceptance(bundle, source, expected_build_id)? {
        ObjectAcceptance::Accepted(accepted) => accepted,
        ObjectAcceptance::Inconclusive(message) => {
            eprintln!(
                "{}; no object acceptance consistency result established.",
                message.trim_end_matches('.')
            );
            std::process::exit(3);
        }
    };
    let fresh = object_machine_tv_acceptance_descriptor(&accepted, expected_build_id)?;
    drop(accepted);
    if saved["evidence_fingerprint"] != fresh["evidence_fingerprint"] {
        return Err(BundleError(
            "saved object acceptance descriptor is not bound to the fresh attempt inputs",
        ));
    }
    if saved != fresh {
        return Err(BundleError(
            "saved object acceptance descriptor differs from the fresh object acceptance",
        ));
    }
    if json_output {
        let supplied = |present: bool| {
            if present {
                "supplied"
            } else {
                "not_supplied"
            }
        };
        let mut checks_performed = vec!["fresh_object_acceptance", "descriptor_pin_binding"];
        let mut checks_not_performed = vec![
            "record_freshness",
            "producer_authenticity",
            "historical_execution",
            "register_convention",
            "abi_runtime_semantics",
            "os_semantics",
        ];
        checks_not_performed.push(if signature_acceptance.is_some() {
            "descriptor_authenticity_beyond_signature_binding"
        } else {
            "descriptor_authenticity"
        });
        if let Some(acceptance) = signature_acceptance.as_ref() {
            checks_performed.push("producer_signature_binding");
            if acceptance.max_age_checked {
                checks_performed.push("producer_signature_envelope_freshness");
            }
            if acceptance.full_authenticity_chain {
                checks_not_performed.retain(|check| *check != "producer_authenticity");
            }
        }
        let mut document = serde_json::json!({
            "schema_version": "mumei.native-object-acceptance-recheck/v1",
            "recheck": "evaluated",
            "consistent": true,
            "scope": fresh["scope"].clone(),
            "expected_build_id": expected_build_id,
            "descriptor_digest": descriptor_digest,
            "evidence_fingerprint": fresh["evidence_fingerprint"].clone(),
            "trust_inputs": {
                "expected_build_id": "required_separately_trusted_pin",
                "producer_key": supplied(signature.producer_key.is_some()),
                "producer_key_id_pin": supplied(signature.expected_producer_key.is_some()),
                "reference_time": supplied(signature.reference_time.is_some()),
                "max_age_seconds": supplied(signature.max_age_seconds.is_some()),
                "key_registry": supplied(signature.key_registry.is_some()),
                "registry_signature": supplied(signature.registry_signature.is_some()),
                "registry_key": supplied(signature.registry_key.is_some()),
                "revocation_list": supplied(signature.revocation_list.is_some()),
                "revocation_signature": supplied(signature.revocation_signature.is_some()),
                "revocation_key": supplied(signature.revocation_key.is_some()),
                "min_revocation_version": supplied(signature.min_revocation_version.is_some()),
            },
            "checks_performed": checks_performed,
            "checks_not_performed": checks_not_performed,
        });
        if let Some(acceptance) = signature_acceptance.as_ref() {
            document["producer_signature"] = serde_json::to_value(&acceptance.binding)
                .map_err(|_| BundleError("cannot encode producer signature binding"))?;
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&document)
                .map_err(|_| BundleError("cannot encode object acceptance recheck"))?
        );
    } else if signature_acceptance.is_some() {
        println!(
            "Saved native object acceptance descriptor consistent with a fresh object acceptance of the pinned inputs.\nScope: {}\nProducer signature binding: checked for the exact saved descriptor bytes; optional key-label, freshness, revocation and registry gates only as explicitly supplied.\nDescriptor authenticity beyond that binding, real-world producer identity, historical execution, register convention, ABI/runtime and OS semantics: not checked.",
            fresh["scope"].as_str().unwrap_or_default()
        );
    } else {
        println!(
            "Saved native object acceptance descriptor consistent with a fresh object acceptance of the pinned inputs.\nScope: {}\nDescriptor authenticity, producer authenticity and historical execution: not checked.",
            fresh["scope"].as_str().unwrap_or_default()
        );
    }
    Ok(())
}

fn report_object_acceptance_inconclusive(message: &str) -> ! {
    eprintln!("{}; object not accepted.", message.trim_end_matches('.'));
    std::process::exit(3);
}

fn object_evidence_fingerprint(
    build_id: &str,
    source_sha256: &str,
    llvm_sha256: &str,
    object_sha256: &str,
    canonical_argv_hash: &str,
    emit_scope: &str,
) -> Result<String, BundleError> {
    let pins = BTreeMap::from([
        ("build_id", build_id),
        ("source_sha256", source_sha256),
        ("llvm_sha256", llvm_sha256),
        ("object_sha256", object_sha256),
        ("canonical_argv_hash", canonical_argv_hash),
        ("emit_scope", emit_scope),
    ]);
    let bytes = serde_json::to_vec(&pins)
        .map_err(|_| BundleError("cannot encode native object acceptance pins"))?;
    Ok(sha256(&bytes))
}

const OBJECT_DESCRIPTOR_SIGNATURE_PUBLISH_ERROR: BundleError =
    BundleError("cannot publish native object descriptor signature");

fn stage_object_descriptor_signature(
    descriptor: &[u8],
    path: &Path,
    key_file: &Path,
    producer_key_id: &str,
    produced_at: Option<&str>,
) -> Result<tempfile::NamedTempFile, BundleError> {
    if fs::symlink_metadata(path).is_ok() {
        return Err(OBJECT_DESCRIPTOR_SIGNATURE_PUBLISH_ERROR);
    }
    if descriptor.len() as u64 > producer_signature::MAX_SUBJECT_BYTES {
        return Err(BundleError(
            "native object descriptor exceeds the signing limit",
        ));
    }
    let envelope = super::evidence::sign_record(descriptor, key_file, producer_key_id, produced_at)
        .map_err(BundleError)?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut staged = tempfile::NamedTempFile::new_in(parent)
        .map_err(|_| BundleError("cannot stage native object descriptor signature"))?;
    serde_json::to_writer_pretty(&mut staged, &envelope)
        .map_err(|_| BundleError("cannot write native object descriptor signature"))?;
    staged
        .write_all(b"\n")
        .map_err(|_| BundleError("cannot write native object descriptor signature"))?;
    Ok(staged)
}

fn object_machine_tv_acceptance_descriptor(
    accepted: &AcceptedObject,
    expected_build_id: &str,
) -> Result<serde_json::Value, BundleError> {
    let AcceptedObject {
        data,
        evaluation,
        record,
        correspondence,
        canonical_argv_hash,
    } = accepted;
    let scope = object_machine_tv_acceptance_scope(data.object_link_correspondence);
    let source = data.source_text.as_deref().ok_or(BundleError(
        "missing rechecked source for native object acceptance",
    ))?;
    let llvm_sha256 = sha256(&data.llvm_bytes);
    let object_sha256 = sha256(&data.object_bytes);
    let evidence_fingerprint = object_evidence_fingerprint(
        expected_build_id,
        &sha256(source.as_bytes()),
        &llvm_sha256,
        &object_sha256,
        canonical_argv_hash,
        scope,
    )?;
    let (checks_performed, checks_not_performed) = object_machine_tv_acceptance_checks(
        data.family,
        true,
        data.object_link_correspondence,
        evaluation.entry_wrapper_applicable
            && evaluation.outcomes.get("main") == Some(&FunctionOutcome::Equivalent),
        &evaluation.coverage,
    );
    let descriptor = serde_json::json!({
        "schema_version": "mumei.native-evidence-acceptance/v1",
        "acceptance": "evaluated",
        "accepted": true,
        "scope": scope,
        "checks_performed": checks_performed,
        "checks_not_performed": checks_not_performed,
        "build_id": expected_build_id,
        "llvm_sha256": llvm_sha256,
        "object_sha256": object_sha256,
        "evidence_fingerprint": evidence_fingerprint,
        "register_convention_assumed": machine_tv_register_convention_assumed_with_frame_memory(
            evaluation.counts.equivalent_f64_leaf_count,
            evaluation.counts.equivalent_composite_aggregate_count,
            evaluation.counts.equivalent_f64_composite_aggregate_count,
            evaluation
                .counts
                .equivalent_wide_composite_aggregate_convention_count,
            evaluation.counts.equivalent_wide_scalar_count,
            evaluation.counts.equivalent_red_zone_frame_memory_count,
            evaluation.counts.equivalent_allocated_frame_memory_count,
        ),
        "input_domain": &evaluation.input_domain,
        "functions": &evaluation.functions,
        "object_translation": object_translation_json(
            data,
            if machine_tv_acceptance_decision(
                &evaluation.outcomes,
                evaluation.entry_wrapper_applicable,
            ) == ObjectMachineTvAcceptanceDecision::Accepted
            {
                "checked_source_machine_tv_within_profile"
            } else {
                "unchecked_not_translation_validated"
            },
            record,
            correspondence,
        ),
        "source_machine_tv": source_machine_tv_within_profile_json(
            evaluation,
            &checks_not_performed,
            data.object_link_correspondence,
        ),
    });
    Ok(descriptor)
}

fn write_object_machine_tv_acceptance(
    accepted: &AcceptedObject,
    expected_build_id: &str,
    output: &ObjectDescriptorOutputArgs,
) -> Result<(), BundleError> {
    let AcceptedObject {
        data,
        evaluation,
        correspondence,
        ..
    } = accepted;
    let scope = object_machine_tv_acceptance_scope(data.object_link_correspondence);
    if output.json {
        let descriptor = object_machine_tv_acceptance_descriptor(accepted, expected_build_id)?;
        let mut bytes = serde_json::to_vec_pretty(&descriptor)
            .map_err(|_| BundleError("cannot encode native object acceptance descriptor"))?;
        bytes.push(b'\n');
        let staged_signature = if let Some(signature_output) = &output.signature_output {
            let (Some(key_file), Some(producer_key_id)) =
                (&output.key_file, &output.producer_key_id)
            else {
                return Err(BundleError(
                    "descriptor signing requires a key file and producer key id",
                ));
            };
            let path = Path::new(signature_output);
            let staged = stage_object_descriptor_signature(
                &bytes,
                path,
                Path::new(key_file),
                producer_key_id,
                output.produced_at.as_deref(),
            )?;
            Some((staged, path))
        } else {
            None
        };
        let mut stdout = std::io::stdout().lock();
        stdout
            .write_all(&bytes)
            .and_then(|()| stdout.flush())
            .map_err(|_| BundleError("cannot write native object acceptance descriptor"))?;
        if let Some((staged, path)) = staged_signature {
            staged
                .persist_noclobber(path)
                .map_err(|_| OBJECT_DESCRIPTOR_SIGNATURE_PUBLISH_ERROR)?;
        }
        Ok(())
    } else {
        let wrapper_equivalent = evaluation.entry_wrapper_applicable
            && evaluation.outcomes.get("main") == Some(&FunctionOutcome::Equivalent);
        let wrapper_summary = if wrapper_equivalent {
            " Entry wrapper is equivalent within the machine-code TV profile."
        } else {
            ""
        };
        println!(
            "Native object accepted within the machine-code TV profile: {} source-atom functions equivalent under the assumed register convention and input domain.{wrapper_summary}",
            object_machine_tv_source_atom_count(&evaluation.outcomes),
        );
        let wrapper_exclusion = if wrapper_equivalent {
            ""
        } else {
            "object-entry wrapper semantics, "
        };
        println!(
            "Acceptance scope excludes producer and descriptor authenticity, historical execution, {wrapper_exclusion}register-convention validation, optimizer/ABI/runtime/OS observations, grade promotion, and Lean checks; scope: {scope}."
        );
        // Same residual-domain count as the JSON descriptor's
        // checks_not_performed — producer authenticity, register
        // convention and the other fixed domains included.
        let (_, unchecked_domains) = object_machine_tv_acceptance_checks(
            data.family,
            true,
            data.object_link_correspondence,
            wrapper_equivalent,
            &evaluation.coverage,
        );
        println!(
            "{}",
            source_machine_tv_within_profile_line(evaluation, unchecked_domains.len())
        );
        for line in object_translation_correspondence_report_lines(correspondence) {
            println!("{line}");
        }
        Ok(())
    }
}

fn observation_pins_comparable(link_build: &LinkBuild) -> bool {
    match &link_build.observation_envelope {
        None => true,
        Some(envelope) => envelope == &pinned_observation_envelope(),
    }
}

impl ArtifactRecheckResult {
    fn different(family: RecordFamily) -> Self {
        Self {
            result: ArtifactRecheck::Different,
            family,
            canonical_argv_compared: false,
            canonical_argv_hash: None,
            record_profile: None,
            pin_envelope: "unrecorded",
            expected_exit_status: None,
            expected_stdout_sha256: None,
            expected_stderr_sha256: None,
            stage: None,
            source_text: None,
        }
    }

    fn inconclusive(family: RecordFamily) -> Self {
        Self {
            result: ArtifactRecheck::Inconclusive,
            family,
            canonical_argv_compared: false,
            canonical_argv_hash: None,
            record_profile: None,
            pin_envelope: "unrecorded",
            expected_exit_status: None,
            expected_stdout_sha256: None,
            expected_stderr_sha256: None,
            stage: None,
            source_text: None,
        }
    }
}

fn recheck_artifact(
    bundle: &Path,
    source: &Path,
    expected_id: &str,
) -> Result<ArtifactRecheckResult, BundleError> {
    // Everything the artifact stage compares comes from the same
    // once-read snapshot `recheck-llvm` verified — re-reading the record
    // or any component here would let a mid-command swap substitute a
    // self-consistent foreign IR/object/executable set.
    let (result, loaded, family) = recheck_llvm_loaded(bundle, source, expected_id)?;
    match result {
        LlvmRecheck::Rechecked(_)
        | LlvmRecheck::ModuleRechecked(..)
        | LlvmRecheck::MultifileRechecked(..) => (),
        LlvmRecheck::Different => return Ok(ArtifactRecheckResult::different(family)),
        LlvmRecheck::Inconclusive => return Ok(ArtifactRecheckResult::inconclusive(family)),
    }
    let LoadedBundle {
        object_build,
        link_build,
        files,
        source_text,
        record_profile,
    } = loaded;
    let link_runtime = match link_build.runtime_source_sha256.as_deref() {
        Some(pinned) if pinned == sha256(MUMEI_RUNTIME_C.as_bytes()) => true,
        Some(_) => {
            eprintln!("Recorded runtime source differs from the current embedded runtime; artifact recheck is inconclusive.");
            return Ok(ArtifactRecheckResult::inconclusive(family));
        }
        None => false,
    };
    let parent = bundle
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let stage = tempfile::Builder::new()
        .prefix(".mumei-artifact-")
        .tempdir_in(parent)
        .map_err(|_| BundleError("cannot stage artifact recheck"))?;
    let runtime_path = if link_runtime {
        let path = stage.path().join("mumei_runtime.c");
        write(&path, MUMEI_RUNTIME_C.as_bytes())?;
        Some(path)
    } else {
        None
    };
    let ir_path = stage.path().join(BundleFile::LlvmIr.file_name());
    write(&ir_path, &files[&BundleFile::LlvmIr])?;
    let object_input_path = stage.path().join("recheck-input.ll");
    // `compile_llvm_ir_to_object_recorded` rebinds and rewrites its input, so
    // staged `program.ll` must stay byte-identical to the pin.
    write(&object_input_path, &files[&BundleFile::LlvmIr])?;
    let object_path = stage.path().join(BundleFile::Object.file_name());
    let fresh_object = mumei_emit_llvm::codegen::compile_llvm_ir_to_object_recorded(
        &object_input_path,
        &object_path,
    );
    fs::remove_file(&object_input_path)
        .map_err(|_| BundleError("cannot remove staged native LLVM recheck input"))?;
    let fresh_object = fresh_object.map_err(|_| BundleError("native object re-emission failed"))?;
    if fresh_object != object_build
        || read_regular(&object_path, MAX_COMPONENT_BYTES)? != files[&BundleFile::Object]
    {
        return Ok(ArtifactRecheckResult::different(family));
    }
    let executable_path = stage.path().join(BundleFile::Executable.file_name());
    let fresh_link = match runtime_path.as_deref() {
        Some(runtime) => linker::link_recorded_with_runtime(
            std::slice::from_ref(&object_path),
            runtime,
            &executable_path,
        ),
        None => linker::link_recorded(&[object_path], &executable_path),
    }
    .map_err(|_| BundleError("native link or toolchain observation failed"))?;
    if fresh_link.driver != link_build.driver
        || fresh_link.driver_version != link_build.driver_version
        || fresh_link.driver_target != link_build.driver_target
        || fresh_link.linker != link_build.linker
        || fresh_link.linker_version != link_build.linker_version
        || fresh_link.runtime_source_sha256 != link_build.runtime_source_sha256
        || read_regular(&executable_path, MAX_COMPONENT_BYTES)? != files[&BundleFile::Executable]
    {
        return Ok(ArtifactRecheckResult::different(family));
    }
    // Records pinned before `canonical_arguments_sha256` existed carry
    // no pin to compare — the comparison is honest-absent for them.
    if link_build
        .canonical_arguments_sha256
        .as_deref()
        .is_some_and(|pinned| fresh_link.canonical_arguments_sha256.as_deref() != Some(pinned))
    {
        return Ok(ArtifactRecheckResult::different(family));
    }
    // Report whether the canonical argv comparison actually ran so the
    // acceptance descriptor never claims it for a pinless record.
    let pins_comparable = observation_pins_comparable(&link_build);
    let pin_envelope = if pins_comparable && link_build.observation_envelope.is_some() {
        "recorded"
    } else {
        "unrecorded"
    };
    Ok(ArtifactRecheckResult {
        result: ArtifactRecheck::Rechecked,
        family,
        canonical_argv_compared: link_build.canonical_arguments_sha256.is_some(),
        record_profile,
        pin_envelope,
        expected_exit_status: pins_comparable
            .then_some(link_build.expected_exit_status)
            .flatten(),
        expected_stdout_sha256: pins_comparable
            .then(|| link_build.expected_stdout_sha256.clone())
            .flatten(),
        expected_stderr_sha256: pins_comparable
            .then(|| link_build.expected_stderr_sha256.clone())
            .flatten(),
        canonical_argv_hash: link_build.canonical_arguments_sha256,
        stage: Some(stage),
        source_text,
    })
}

#[cfg(target_os = "linux")]
fn publish(stage: &Path, output: &Path) -> Result<(), BundleError> {
    let stage = std::ffi::CString::new(stage.as_os_str().as_encoded_bytes())
        .map_err(|_| BundleError("invalid staging path"))?;
    let output = std::ffi::CString::new(output.as_os_str().as_encoded_bytes())
        .map_err(|_| BundleError("invalid output path"))?;
    // SAFETY: both NUL-terminated paths remain alive; NOREPLACE prevents clobbering a concurrent destination.
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            stage.as_ptr(),
            libc::AT_FDCWD,
            output.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result != 0 {
        return Err(BundleError(
            "cannot publish bundle without replacing destination",
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn publish(_: &Path, _: &Path) -> Result<(), BundleError> {
    Err(BundleError("unsupported host"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_toolchain_observation_records_effective_identity() {
        let observation = native_toolchain_observation_with_link(linker::LinkToolchain {
            driver: "cc".into(),
            driver_version: "11.4.0".into(),
            driver_target: "x86_64-linux-gnu".into(),
            linker: "gnu-ld".into(),
            linker_version: "2.38".into(),
        });
        let z3 = verification::linked_z3_version();
        let (llvm_version, target_triple) = mumei_emit_llvm::codegen::linked_llvm_toolchain();

        assert_eq!(
            observation["schema_version"],
            "mumei.native-toolchain-observation/v1"
        );
        assert_eq!(
            observation["scope"],
            "effective_toolchain_identity_observation_only/no_environment_or_artifact_semantic_attestation"
        );
        assert_eq!(observation["host"], observation_host());
        assert_eq!(observation["target_triple"], target_triple);
        assert_eq!(observation["solvers"]["z3"]["binding"], "linked");
        assert_eq!(
            observation["solvers"]["z3"]["version"],
            format!("{}.{}.{}.{}", z3.0, z3.1, z3.2, z3.3)
        );
        assert_eq!(observation["llvm"]["binding"], "linked");
        assert_eq!(observation["llvm"]["version"], llvm_version);
        assert_eq!(observation["link"]["driver"], "cc");
        assert_eq!(observation["link"]["driver_version"], "11.4.0");
        assert_eq!(observation["link"]["driver_target"], "x86_64-linux-gnu");
        assert_eq!(observation["link"]["linker"], "gnu-ld");
        assert_eq!(observation["link"]["linker_version"], "2.38");
        assert!(observation.get("environment").is_none());
        assert!(observation.get("path").is_none());
    }

    #[test]
    fn llvm_ir_object_edge_result_requires_every_promotion_gate() {
        let checked_gates: Vec<Option<&str>> = LLVM_IR_OBJECT_GATE_ITEMS
            .iter()
            .map(|_| Some("checked"))
            .collect();
        // All promotion conditions met.
        assert_eq!(
            llvm_ir_object_edge_result(Some("checked"), &checked_gates, true, true),
            "checked_correspondence"
        );
        // The link correspondence not running keeps the edge unchecked.
        assert_eq!(
            llvm_ir_object_edge_result(Some("checked"), &checked_gates, false, true),
            "unchecked_not_translation_validated"
        );
        // So does an unpinned canonical argv.
        assert_eq!(
            llvm_ir_object_edge_result(Some("checked"), &checked_gates, true, false),
            "unchecked_not_translation_validated"
        );
        // A gate item that stayed unchecked (or is absent) is
        // fail-closed — never counted as checked.
        let mut gates = checked_gates.clone();
        gates[2] = Some("unchecked");
        assert_eq!(
            llvm_ir_object_edge_result(Some("checked"), &gates, true, true),
            "unchecked_not_translation_validated"
        );
        let mut gates = checked_gates.clone();
        gates[4] = None;
        assert_eq!(
            llvm_ir_object_edge_result(Some("checked"), &gates, true, true),
            "unchecked_not_translation_validated"
        );
        // A solver-undecidable comparison item is not a skip.
        assert_eq!(
            llvm_ir_object_edge_result(Some("unknown"), &checked_gates, true, true),
            "unchecked_not_translation_validated"
        );
        // Validation that ran and failed is inconsistent, on the item
        // or on any gate.
        assert_eq!(
            llvm_ir_object_edge_result(Some("inconsistent"), &checked_gates, true, true),
            "inconsistent_correspondence"
        );
        let mut gates = checked_gates;
        gates[0] = Some("inconsistent");
        assert_eq!(
            llvm_ir_object_edge_result(Some("checked"), &gates, true, true),
            "inconsistent_correspondence"
        );
    }

    #[test]
    fn edge_result_for_item_result_keeps_inconsistent_apart_from_unchecked() {
        assert_eq!(
            edge_result_for_item_result(Some("checked")),
            "checked_correspondence"
        );
        // Validation that ran and failed is not a skipped edge.
        assert_eq!(
            edge_result_for_item_result(Some("inconsistent")),
            "inconsistent_correspondence"
        );
        for result in [Some("unchecked"), Some("unknown"), None] {
            assert_eq!(
                edge_result_for_item_result(result),
                "unchecked_not_translation_validated"
            );
        }
    }

    #[test]
    fn object_machine_tv_acceptance_decision_is_fail_closed() {
        let accepted = BTreeMap::from([
            ("__mumei_user_main".to_owned(), FunctionOutcome::Equivalent),
            ("main".to_owned(), FunctionOutcome::Equivalent),
        ]);
        assert_eq!(
            machine_tv_acceptance_decision(&accepted, true),
            ObjectMachineTvAcceptanceDecision::Accepted
        );
        assert_eq!(
            machine_tv_acceptance_decision(
                &BTreeMap::from([("main".to_owned(), FunctionOutcome::Equivalent)]),
                true,
            ),
            ObjectMachineTvAcceptanceDecision::Inconclusive
        );

        let different_source = BTreeMap::from([
            ("__mumei_user_main".to_owned(), FunctionOutcome::Different),
            ("main".to_owned(), FunctionOutcome::Equivalent),
        ]);
        assert_eq!(
            machine_tv_acceptance_decision(&different_source, true),
            ObjectMachineTvAcceptanceDecision::Different
        );

        let different_wrapper = BTreeMap::from([
            ("__mumei_user_main".to_owned(), FunctionOutcome::Equivalent),
            ("main".to_owned(), FunctionOutcome::Different),
        ]);
        assert_eq!(
            machine_tv_acceptance_decision(&different_wrapper, true),
            ObjectMachineTvAcceptanceDecision::Different
        );

        let unknown_wrapper = BTreeMap::from([
            ("__mumei_user_main".to_owned(), FunctionOutcome::Equivalent),
            ("main".to_owned(), FunctionOutcome::Unknown),
        ]);
        assert_eq!(
            machine_tv_acceptance_decision(&unknown_wrapper, true),
            ObjectMachineTvAcceptanceDecision::Inconclusive
        );

        let outside_wrapper = BTreeMap::from([
            ("__mumei_user_main".to_owned(), FunctionOutcome::Equivalent),
            (
                "main".to_owned(),
                FunctionOutcome::OutsideProfile("unsupported_instruction"),
            ),
        ]);
        assert_eq!(
            machine_tv_acceptance_decision(&outside_wrapper, true),
            ObjectMachineTvAcceptanceDecision::Inconclusive
        );

        let unknown_source = BTreeMap::from([
            ("__mumei_user_main".to_owned(), FunctionOutcome::Unknown),
            ("main".to_owned(), FunctionOutcome::Equivalent),
        ]);
        assert_eq!(
            machine_tv_acceptance_decision(&unknown_source, true),
            ObjectMachineTvAcceptanceDecision::Inconclusive
        );

        let absent_wrapper = BTreeMap::from([
            ("__mumei_user_main".to_owned(), FunctionOutcome::Equivalent),
            (
                "main".to_owned(),
                FunctionOutcome::OutsideProfile("not_a_source_atom"),
            ),
        ]);
        assert_eq!(
            machine_tv_acceptance_decision(&absent_wrapper, false),
            ObjectMachineTvAcceptanceDecision::Accepted
        );

        let unsupported_source = BTreeMap::from([
            (
                "__mumei_user_main".to_owned(),
                FunctionOutcome::OutsideProfile("unsupported_instruction"),
            ),
            (
                "main".to_owned(),
                FunctionOutcome::OutsideProfile("not_a_source_atom"),
            ),
        ]);
        assert_eq!(
            machine_tv_acceptance_decision(&unsupported_source, false),
            ObjectMachineTvAcceptanceDecision::Inconclusive
        );

        let only_non_source = BTreeMap::from([(
            "main".to_owned(),
            FunctionOutcome::OutsideProfile("not_a_source_atom"),
        )]);
        assert_eq!(
            machine_tv_acceptance_decision(&only_non_source, false),
            ObjectMachineTvAcceptanceDecision::Inconclusive
        );
    }

    #[test]
    fn object_translation_acceptance_decision_is_fail_closed() {
        let item = |name: &str, result: &str| serde_json::json!({"item": name, "result": result, "detail": "test"});
        let checked_all: Vec<serde_json::Value> = OBJECT_TRANSLATION_ACCEPTANCE_ITEMS
            .iter()
            .map(|name| item(name, "checked"))
            .collect();
        assert_eq!(
            object_translation_acceptance_decision(&checked_all),
            ObjectMachineTvAcceptanceDecision::Accepted
        );

        // One validation that ran and failed rejects the whole surface.
        let mut diverged = checked_all.clone();
        diverged[2] = item("llvm_ir_object_equivalence", "inconsistent");
        assert_eq!(
            object_translation_acceptance_decision(&diverged),
            ObjectMachineTvAcceptanceDecision::Different
        );

        // Anything undecided — unchecked, unknown, or an
        // unrecognized result — is inconclusive, never accepted.
        for result in ["unchecked", "unknown", "skipped", ""] {
            let mut undecided = checked_all.clone();
            undecided[6] = item("relocation_applied_values", result);
            assert_eq!(
                object_translation_acceptance_decision(&undecided),
                ObjectMachineTvAcceptanceDecision::Inconclusive,
                "result {result:?}"
            );
        }

        // A required item absent from the list is inconclusive.
        assert_eq!(
            object_translation_acceptance_decision(&checked_all[1..]),
            ObjectMachineTvAcceptanceDecision::Inconclusive
        );
        assert_eq!(
            object_translation_acceptance_decision(&[]),
            ObjectMachineTvAcceptanceDecision::Inconclusive
        );

        // An extra undecided item outside the required set still
        // blocks acceptance — the whole surface must be checked.
        let mut extra_unchecked = checked_all.clone();
        extra_unchecked.push(item("future_item", "unchecked"));
        assert_eq!(
            object_translation_acceptance_decision(&extra_unchecked),
            ObjectMachineTvAcceptanceDecision::Inconclusive
        );
        let mut extra_checked = checked_all;
        extra_checked.push(item("future_item", "checked"));
        assert_eq!(
            object_translation_acceptance_decision(&extra_checked),
            ObjectMachineTvAcceptanceDecision::Accepted
        );
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn evaluate_machine_tv_compares_scalar_i64_against_ir_encoding() {
        // Step 3a: scalar-i64 functions take their expected return
        // from the LLVM-IR encoding. A parseable but divergent IR
        // reports `Different` even though the source model and the
        // object agree — proving the IR path, not the source-model
        // fallback, runs in the main evaluation.
        let source = "atom add(x: i64, y: i64) -> i64 requires: true; ensures: result == x + y; body: { x + y };";
        let matching_ir = r#"target triple = "x86_64-unknown-linux-gnu"
define i64 @add(i64 %x, i64 %y) {
entry:
  %r = add i64 %x, %y
  ret i64 %r
}
"#;
        let divergent_ir = matching_ir.replace("add i64", "sub i64");

        let stage = tempfile::tempdir().unwrap();
        let ir_path = stage.path().join("in.ll");
        let object_path = stage.path().join("in.o");
        fs::write(&ir_path, matching_ir).unwrap();
        mumei_emit_llvm::codegen::compile_llvm_ir_to_object_recorded(&ir_path, &object_path)
            .unwrap();
        let object_bytes = fs::read(&object_path).unwrap();
        let object_surface = object_symbol_surface(&object_bytes).unwrap();
        assert!(object_surface.defined.contains("add"));

        let make_data = |llvm_bytes: Vec<u8>| ObjectRecheckData {
            stage: tempfile::tempdir().unwrap(),
            family: RecordFamily::Single,
            canonical_argv_hash: None,
            admission: RefinedAdmission::Reject,
            ir_surface: tv::IrFunctionSurface {
                defined: BTreeSet::new(),
                declared: BTreeSet::new(),
                calls: BTreeMap::new(),
                libc_calls: BTreeMap::new(),
                call_edges_checkable: false,
            },
            object_surface: object_surface.clone(),
            object_link_correspondence: true,
            llvm_bytes,
            object_bytes: object_bytes.clone(),
            executable_bytes: Vec::new(),
            object_translation: None,
            resolved_call_sites: 0,
            source_text: Some(source.to_owned()),
        };

        // IR edges agree with the machine code.
        let evaluation = evaluate_machine_tv(&make_data(matching_ir.as_bytes().to_vec())).unwrap();
        assert_eq!(
            evaluation.outcomes.get("add"),
            Some(&FunctionOutcome::Equivalent)
        );
        // A parseable IR that disagrees with the object: only the
        // IR-edge comparison can produce `Different` here, since the
        // source model and the machine code agree.
        let evaluation = evaluate_machine_tv(&make_data(divergent_ir.into_bytes())).unwrap();
        assert_eq!(
            evaluation.outcomes.get("add"),
            Some(&FunctionOutcome::Different)
        );
        // Without IR the source-model fallback still proves
        // equivalence (the claim is machine↔source either way).
        let evaluation = evaluate_machine_tv(&make_data(Vec::new())).unwrap();
        assert_eq!(
            evaluation.outcomes.get("add"),
            Some(&FunctionOutcome::Equivalent)
        );
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn hir_llvm_atom_outcomes_cover_main_calls_globals_and_generic_structs() {
        // The recheck compares each atom's HIR against a sliced
        // member subset of the bundle IR. This fixture pins the three
        // edges that dropped atoms out of the subset:
        //  - `caller` calls `main()`, which the emitter renames to
        //    `__mumei_user_main` — the call rename map must translate;
        //  - `@greeting` is an unrelated global: kept in the subset
        //    header it would fail every atom's module check;
        //  - `pair_sum` takes `Pair<i64>` — the generic declaration's
        //    `T` must specialize to `i64` from the atom's signature.
        let source = concat!(
            "struct Pair<T> { a: T, b: T }\n",
            "atom pair_sum(p: Pair<i64>) -> i64 requires: true; ensures: true; body: { p.a + p.b };\n",
            "atom main() -> i64 requires: true; ensures: true; body: { 7 };\n",
            "atom caller() -> i64 requires: true; ensures: true; body: { main() };\n",
            "atom bad(x: i64) -> i64 requires: true; ensures: true; body: { x + 1 };\n",
        );
        let llvm_ir = r#"target triple = "x86_64-pc-linux-gnu"
@greeting = internal constant [5 x i8] c"hi\0A\00\00"

define i64 @__mumei_user_main() {
entry:
  ret i64 7
}
define i64 @caller() {
entry:
  %r = call i64 @__mumei_user_main()
  ret i64 %r
}
define i64 @bad(i64 %x) {
entry:
  %r = sub i64 %x, 1
  ret i64 %r
}
define i64 @pair_sum({ i64, i64 } %0) {
entry:
  %p.a = extractvalue { i64, i64 } %0, 0
  %p.b = extractvalue { i64, i64 } %0, 1
  %r = add i64 %p.a, %p.b
  ret i64 %r
}
define i32 @main(i32 %argc, ptr %argv) {
entry:
  %r = call i64 @__mumei_user_main()
  ret i32 0
}
"#;
        let data = ObjectRecheckData {
            stage: tempfile::tempdir().unwrap(),
            family: RecordFamily::Single,
            canonical_argv_hash: None,
            admission: RefinedAdmission::Reject,
            ir_surface: tv::IrFunctionSurface {
                defined: BTreeSet::new(),
                declared: BTreeSet::new(),
                calls: BTreeMap::new(),
                libc_calls: BTreeMap::new(),
                call_edges_checkable: false,
            },
            object_surface: ObjectSymbolSurface {
                defined: BTreeSet::new(),
                undefined: BTreeSet::new(),
                calls: BTreeMap::new(),
                call_targets: BTreeMap::new(),
                call_edges_checkable: false,
            },
            object_link_correspondence: false,
            llvm_bytes: llvm_ir.as_bytes().to_vec(),
            object_bytes: Vec::new(),
            executable_bytes: Vec::new(),
            object_translation: None,
            resolved_call_sites: 0,
            source_text: Some(source.to_owned()),
        };
        let outcomes = hir_llvm_atom_outcomes(&data).unwrap();
        assert_eq!(outcomes.get("main"), Some(&"checked"));
        assert_eq!(outcomes.get("caller"), Some(&"checked"));
        assert_eq!(outcomes.get("pair_sum"), Some(&"checked"));
        // A member that disagrees with its HIR is validation that ran
        // and failed — named `inconsistent`, never `unchecked`.
        assert_eq!(outcomes.get("bad"), Some(&"inconsistent"));
    }

    #[test]
    fn machine_tv_entry_wrapper_check_coverage_tracks_applicability_and_outcome() {
        assert_eq!(
            machine_tv_entry_wrapper_checks(false, None),
            (Vec::new(), Vec::new())
        );
        assert_eq!(
            machine_tv_entry_wrapper_checks(true, Some(FunctionOutcome::Equivalent)),
            (vec!["machine_code_entry_wrapper_tv"], Vec::new())
        );
        for outcome in [
            FunctionOutcome::Different,
            FunctionOutcome::Unknown,
            FunctionOutcome::OutsideProfile("unsupported_instruction"),
        ] {
            assert_eq!(
                machine_tv_entry_wrapper_checks(true, Some(outcome)),
                (Vec::new(), vec!["machine_code_entry_wrapper_tv"])
            );
        }
    }

    #[test]
    fn object_evidence_fingerprint_binds_each_canonical_input() {
        let pins = ["build", "source", "llvm", "object", "argv", "scope"];
        let fingerprint = |[build, source, llvm, object, argv, scope]: [&str; 6]| {
            object_evidence_fingerprint(build, source, llvm, object, argv, scope).unwrap()
        };
        let expected = sha256(
            br#"{"build_id":"build","canonical_argv_hash":"argv","emit_scope":"scope","llvm_sha256":"llvm","object_sha256":"object","source_sha256":"source"}"#,
        );
        let original = fingerprint(pins);
        assert_eq!(original, expected);
        assert_eq!(fingerprint(pins), original);
        for index in 0..pins.len() {
            let mut changed = pins;
            changed[index] = "different";
            assert_ne!(fingerprint(changed), original, "unchanged pin {index}");
        }
    }

    #[test]
    fn object_machine_tv_acceptance_checks_follow_record_and_recheck_gates() {
        for entry_wrapper_equivalent in [false, true] {
            let mut coverage = (
                vec![
                    "machine_code_leaf_function_tv",
                    "machine_code_leaf_function_tv",
                ],
                vec![
                    "machine_code_direct_call_site_tv",
                    "machine_code_leaf_function_tv",
                ],
            );
            if entry_wrapper_equivalent {
                coverage.0.push("machine_code_entry_wrapper_tv");
            }
            for family in [
                RecordFamily::Single,
                RecordFamily::Module,
                RecordFamily::Multifile,
            ] {
                for canonical_argv_compared in [false, true] {
                    for object_link_correspondence in [false, true] {
                        let (performed, not_performed) = object_machine_tv_acceptance_checks(
                            family,
                            canonical_argv_compared,
                            object_link_correspondence,
                            entry_wrapper_equivalent,
                            &coverage,
                        );
                        assert_eq!(performed.first(), Some(&"bundle_identity_binding"));
                        assert_eq!(
                            performed.contains(&"pinned_member_merge_rederivation"),
                            family == RecordFamily::Multifile
                        );
                        assert_eq!(
                            performed.contains(&"canonical_link_argument_vector_reproduction"),
                            canonical_argv_compared
                        );
                        assert_eq!(
                            not_performed.contains(&"canonical_link_argument_vector_reproduction"),
                            !canonical_argv_compared
                        );
                        assert_eq!(
                            performed.contains(&"object_link_correspondence"),
                            object_link_correspondence
                        );
                        assert_eq!(
                            not_performed.contains(&"object_link_correspondence"),
                            !object_link_correspondence
                        );
                        assert!(performed.contains(&"machine_code_leaf_function_tv"));
                        assert!(not_performed.contains(&"machine_code_direct_call_site_tv"));
                        assert_eq!(
                            performed.contains(&"machine_code_entry_wrapper_tv"),
                            entry_wrapper_equivalent
                        );
                        assert_eq!(
                            not_performed.contains(&"object_entry_wrapper_semantics"),
                            !entry_wrapper_equivalent
                        );
                        assert_eq!(
                            performed.last(),
                            Some(&"object_source_machine_code_correspondence")
                        );
                        assert!(!performed.contains(&"object_translation_correspondence"));
                        assert!(!not_performed.contains(&"object_translation_correspondence"));
                        let performed_set: BTreeSet<_> = performed.iter().copied().collect();
                        let not_performed_set: BTreeSet<_> =
                            not_performed.iter().copied().collect();
                        assert_eq!(performed_set.len(), performed.len());
                        assert_eq!(not_performed_set.len(), not_performed.len());
                        assert!(performed_set.is_disjoint(&not_performed_set));
                    }
                }
            }
        }
        assert_eq!(
            object_machine_tv_acceptance_scope(false),
            "native_bundle_object_translation_correspondence_without_object_link"
        );
    }

    #[test]
    fn machine_tv_admission_matches_the_record_family_profile() {
        assert_eq!(
            machine_tv_admission_for_record(RecordFamily::Single, None).unwrap(),
            RefinedAdmission::Reject
        );
        assert_eq!(
            machine_tv_admission_for_record(RecordFamily::Module, Some(MODULE_PROFILE_V2)).unwrap(),
            RefinedAdmission::V1
        );
        assert_eq!(
            machine_tv_admission_for_record(RecordFamily::Module, Some(MODULE_PROFILE)).unwrap(),
            RefinedAdmission::Reject
        );
        assert_eq!(
            machine_tv_admission_for_record(RecordFamily::Multifile, Some(MULTIFILE_PROFILE_V2))
                .unwrap(),
            RefinedAdmission::V1
        );
        assert_eq!(
            machine_tv_admission_for_record(RecordFamily::Multifile, Some(MULTIFILE_PROFILE))
                .unwrap(),
            RefinedAdmission::Reject
        );
        assert!(machine_tv_admission_for_record(RecordFamily::Module, None).is_err());
    }

    #[test]
    fn machine_tv_descriptor_omits_leaf_check_for_caller_only_equivalence() {
        let (checks_performed, checks_not_performed) =
            machine_tv_coverage_checks(0, 1, 0, 0, 0, MachineTvLoopCounts::default(), false);
        assert_eq!(checks_performed, vec!["machine_code_direct_call_site_tv"]);
        assert_eq!(
            checks_not_performed,
            vec![
                "machine_code_leaf_function_tv",
                "machine_code_forward_branch_function_tv",
                "machine_code_acyclic_branch_function_tv",
                "machine_code_conditional_call_function_tv",
                "machine_code_bounded_loop_function_tv",
            ]
        );
    }

    #[test]
    fn machine_tv_descriptor_counts_branch_functions_separately_from_leaves() {
        let (checks_performed, checks_not_performed) =
            machine_tv_coverage_checks(0, 0, 1, 0, 0, MachineTvLoopCounts::default(), false);
        assert_eq!(
            checks_performed,
            vec!["machine_code_forward_branch_function_tv"]
        );
        assert_eq!(
            checks_not_performed,
            vec![
                "machine_code_leaf_function_tv",
                "machine_code_direct_call_site_tv",
                "machine_code_acyclic_branch_function_tv",
                "machine_code_conditional_call_function_tv",
                "machine_code_bounded_loop_function_tv",
            ]
        );
    }

    #[test]
    fn machine_tv_descriptor_counts_acyclic_backward_branches_separately() {
        let (checks_performed, checks_not_performed) =
            machine_tv_coverage_checks(0, 0, 0, 1, 0, MachineTvLoopCounts::default(), false);
        assert_eq!(
            checks_performed,
            vec!["machine_code_acyclic_branch_function_tv"]
        );
        assert_eq!(
            checks_not_performed,
            vec![
                "machine_code_leaf_function_tv",
                "machine_code_direct_call_site_tv",
                "machine_code_forward_branch_function_tv",
                "machine_code_conditional_call_function_tv",
                "machine_code_bounded_loop_function_tv",
            ]
        );
    }

    #[test]
    fn machine_tv_descriptor_counts_conditional_call_functions_as_callers() {
        let (checks_performed, checks_not_performed) =
            machine_tv_coverage_checks(0, 1, 0, 0, 1, MachineTvLoopCounts::default(), false);
        assert_eq!(
            checks_performed,
            vec![
                "machine_code_direct_call_site_tv",
                "machine_code_conditional_call_function_tv"
            ]
        );
        assert_eq!(
            checks_not_performed,
            vec![
                "machine_code_leaf_function_tv",
                "machine_code_forward_branch_function_tv",
                "machine_code_acyclic_branch_function_tv",
                "machine_code_bounded_loop_function_tv",
            ]
        );
    }

    #[test]
    fn machine_tv_descriptor_counts_bounded_loop_functions() {
        let loop_counts = MachineTvLoopCounts {
            equivalent_bounded_loop_count: 1,
            equivalent_source_loop_count: 1,
        };
        let (checks_performed, checks_not_performed) =
            machine_tv_coverage_checks(0, 0, 0, 0, 0, loop_counts, false);
        assert_eq!(
            checks_performed,
            vec!["machine_code_bounded_loop_function_tv"]
        );
        assert_eq!(
            checks_not_performed,
            vec![
                "machine_code_leaf_function_tv",
                "machine_code_direct_call_site_tv",
                "machine_code_forward_branch_function_tv",
                "machine_code_acyclic_branch_function_tv",
                "machine_code_conditional_call_function_tv",
                "machine_code_tv_loop_executions_beyond_bound",
                "runtime_cancellation_semantics",
            ]
        );
    }

    #[test]
    fn machine_tv_descriptor_tracks_source_loops_with_acyclic_machine_code() {
        let loop_counts = MachineTvLoopCounts {
            equivalent_bounded_loop_count: 0,
            equivalent_source_loop_count: 1,
        };
        let (checks_performed, checks_not_performed) =
            machine_tv_coverage_checks(0, 0, 0, 0, 0, loop_counts, false);
        assert_eq!(
            machine_tv_input_domain(true, false),
            format!(
                "{MACHINE_CODE_TV_INPUT_DOMAIN}_and_loops_exiting_within_bound_{}",
                semantic_contract::LOOP_BOUND
            )
        );
        assert!(!checks_performed.contains(&"machine_code_bounded_loop_function_tv"));
        assert!(checks_not_performed.contains(&"machine_code_bounded_loop_function_tv"));
        for check in [
            "machine_code_tv_loop_executions_beyond_bound",
            "runtime_cancellation_semantics",
        ] {
            assert!(!checks_performed.contains(&check));
            assert!(checks_not_performed.contains(&check));
        }
    }

    #[test]
    fn machine_tv_refined_domain_label_matches_the_llvm_module_domain() {
        assert_eq!(
            machine_tv_input_domain(false, true),
            "module_members_signed_i64_inputs_with_refined_scalar_signature_predicate_domains_not_process_exit_status"
        );
        assert_eq!(
            machine_tv_input_domain(true, true),
            format!(
                "{MACHINE_CODE_TV_REFINED_INPUT_DOMAIN}_and_loops_exiting_within_bound_{}",
                semantic_contract::LOOP_BOUND
            )
        );
        assert_eq!(
            machine_tv_input_domain(true, false),
            format!(
                "{MACHINE_CODE_TV_INPUT_DOMAIN}_and_loops_exiting_within_bound_{}",
                semantic_contract::LOOP_BOUND
            )
        );
        assert_eq!(
            machine_tv_input_domain(false, false),
            MACHINE_CODE_TV_INPUT_DOMAIN
        );
    }

    #[test]
    fn machine_tv_descriptor_reports_cyclic_outcomes_conditionally() {
        let (_, not_performed) =
            machine_tv_coverage_checks(0, 0, 0, 0, 0, MachineTvLoopCounts::default(), true);
        assert!(not_performed.contains(&"machine_code_tv_cyclic_control_flow_functions"));
        let (_, not_performed) =
            machine_tv_coverage_checks(0, 0, 0, 0, 0, MachineTvLoopCounts::default(), false);
        assert!(!not_performed.contains(&"machine_code_tv_cyclic_control_flow_functions"));
    }

    #[test]
    fn machine_tv_descriptor_tracks_callee_saved_preservation_coverage() {
        let (performed, not_performed) = machine_tv_callee_saved_preservation_checks(2, 2);
        assert_eq!(
            performed,
            vec!["machine_code_callee_saved_register_preservation"]
        );
        assert_eq!(
            not_performed,
            vec!["machine_code_callee_saved_preservation_assumed_for_external_callees"]
        );
        let performed_set: BTreeSet<_> = performed.iter().copied().collect();
        let not_performed_set: BTreeSet<_> = not_performed.iter().copied().collect();
        assert_eq!(performed_set.len(), performed.len());
        assert_eq!(not_performed_set.len(), not_performed.len());
        assert!(performed_set.is_disjoint(&not_performed_set));

        let (performed, not_performed) = machine_tv_callee_saved_preservation_checks(2, 1);
        assert!(performed.is_empty());
        assert_eq!(
            not_performed,
            vec!["machine_code_callee_saved_register_preservation"]
        );

        let (performed, not_performed) = machine_tv_callee_saved_preservation_checks(0, 0);
        assert!(performed.is_empty());
        assert!(not_performed.is_empty());
    }

    #[test]
    fn machine_tv_descriptor_tracks_f64_scalar_coverage_and_assumptions() {
        assert_eq!(
            machine_tv_register_convention_assumed(1, 0, 0, 0),
            "sysv_x86_64_integer_args_rdi_rsi_rdx_rcx_r8_r9_f64_args_xmm0_7_result_rax_or_xmm0"
        );
        assert_eq!(
            machine_tv_register_convention_assumed(0, 0, 0, 0),
            "sysv_x86_64_integer_args_rdi_rsi_rdx_rcx_r8_r9_result_rax"
        );
        assert_eq!(
            machine_tv_register_convention_assumed_with_wide_scalars(0, 0, 0, 0, 1),
            MACHINE_CODE_REGISTER_CONVENTION_WITH_WIDE_SCALARS
        );
        assert_eq!(
            machine_tv_register_convention_assumed_with_wide_scalars(1, 0, 0, 0, 1),
            MACHINE_CODE_F64_REGISTER_CONVENTION_WITH_WIDE_SCALARS
        );
        assert_eq!(
            machine_tv_register_convention_assumed(0, 1, 0, 0),
            "sysv_x86_64_integer_args_rdi_rsi_rdx_rcx_r8_r9_result_rax_two_eightbyte_integer_aggregates_in_consecutive_gprs_result_rax_rdx"
        );
        assert_eq!(
            machine_tv_register_convention_assumed(1, 1, 0, 0),
            "sysv_x86_64_integer_args_rdi_rsi_rdx_rcx_r8_r9_f64_args_xmm0_7_result_rax_or_xmm0_two_eightbyte_integer_aggregates_in_consecutive_gprs_result_rax_rdx"
        );
        assert_eq!(
            machine_tv_register_convention_assumed(1, 1, 1, 0),
            "sysv_x86_64_integer_args_rdi_rsi_rdx_rcx_r8_r9_f64_args_xmm0_7_result_rax_or_xmm0_two_eightbyte_aggregates_classified_integer_gprs_sse_xmm_result_rax_rdx_xmm0_xmm1"
        );
        assert_eq!(
            machine_tv_register_convention_assumed(0, 1, 0, 1),
            MACHINE_CODE_REGISTER_CONVENTION_WITH_LLVM_AGGREGATES
        );
        assert_eq!(
            machine_tv_register_convention_assumed(1, 1, 1, 1),
            MACHINE_CODE_F64_REGISTER_CONVENTION_WITH_LLVM_AGGREGATES
        );
        let (performed, not_performed) = machine_tv_f64_scalar_checks(1);
        assert_eq!(performed, vec!["machine_code_f64_scalar_function_tv"]);
        assert_eq!(
            not_performed,
            vec![
                "machine_code_tv_mxcsr_control_state",
                "machine_code_tv_nan_payload_bits",
            ]
        );
        let performed_set: BTreeSet<_> = performed.iter().copied().collect();
        let not_performed_set: BTreeSet<_> = not_performed.iter().copied().collect();
        assert_eq!(performed_set.len(), performed.len());
        assert_eq!(not_performed_set.len(), not_performed.len());
        assert!(performed_set.is_disjoint(&not_performed_set));

        let (performed, not_performed) = machine_tv_composite_aggregate_checks(1, 0);
        assert_eq!(
            performed,
            vec!["machine_code_composite_register_aggregate_tv"]
        );
        assert!(not_performed.is_empty());
        let (performed, not_performed) = machine_tv_composite_aggregate_checks(1, 1);
        assert_eq!(
            performed,
            vec!["machine_code_composite_register_aggregate_tv"]
        );
        assert_eq!(
            not_performed,
            vec!["machine_code_tv_sysv_memory_class_aggregate_abi"]
        );
        let (performed, not_performed) = machine_tv_composite_aggregate_checks(0, 0);
        assert!(performed.is_empty());
        assert!(not_performed.is_empty());

        let (performed, not_performed) = machine_tv_f64_scalar_checks(0);
        assert!(performed.is_empty());
        assert!(not_performed.is_empty());
    }

    #[test]
    fn machine_tv_descriptor_counts_call_site_only_struct_aggregates() {
        let config = z3::Config::new();
        let context = z3::Context::new(&config);
        let mut model = semantic_contract::PureI64Body::synthetic_scalar(
            "caller",
            z3::ast::BV::from_u64(&context, 0, 64),
        );
        model.call_sites.push(semantic_contract::CallSite {
            callee: "sum".to_owned(),
            args: vec![
                z3::ast::BV::from_u64(&context, 1, 64),
                z3::ast::BV::from_u64(&context, 2, 64),
            ],
            callback_arguments: Vec::new(),
            arg_leaf_doubles: vec![false, false],
            argument_shapes: vec![semantic_contract::CallValueShape::Struct {
                leaf_count: 2,
                has_enum: false,
                has_f64: false,
            }],
            has_composite_arguments_or_result: true,
            result: z3::ast::BV::from_u64(&context, 0, 64),
            result_is_double: false,
            result_store: None,
            result_shape: semantic_contract::CallValueShape::Scalar { leaf_count: 1 },
            result_leaf_doubles: vec![false],
            guard: z3::ast::Bool::from_bool(&context, true),
        });
        assert!(model.param_shapes.is_empty());
        assert_eq!(model.result_shape, None);
        assert!(machine_tv_model_uses_composite_aggregate(&model));
        assert_eq!(
            machine_tv_register_convention_assumed(0, 1, 0, 0),
            "sysv_x86_64_integer_args_rdi_rsi_rdx_rcx_r8_r9_result_rax_two_eightbyte_integer_aggregates_in_consecutive_gprs_result_rax_rdx"
        );
        assert_eq!(
            machine_tv_composite_aggregate_checks(1, 0).0,
            vec!["machine_code_composite_register_aggregate_tv"]
        );

        model.call_sites[0].argument_shapes = vec![semantic_contract::CallValueShape::Struct {
            leaf_count: 3,
            has_enum: false,
            has_f64: false,
        }];
        assert!(machine_tv_model_uses_wide_composite_aggregate(&model));
        assert!(machine_tv_model_uses_wide_composite_aggregate_convention(
            &model
        ));
        assert_eq!(
            machine_tv_register_convention_assumed(0, 1, 0, 1),
            MACHINE_CODE_REGISTER_CONVENTION_WITH_LLVM_AGGREGATES
        );
        assert_eq!(
            machine_tv_composite_aggregate_checks(1, 1).1,
            vec!["machine_code_tv_sysv_memory_class_aggregate_abi"]
        );

        model.call_sites[0].arg_leaf_doubles = vec![false, true];
        model.call_sites[0].argument_shapes = vec![semantic_contract::CallValueShape::Struct {
            leaf_count: 2,
            has_enum: false,
            has_f64: true,
        }];
        assert!(machine_tv_model_uses_f64_composite_aggregate(&model));
        assert_eq!(
            machine_tv_register_convention_assumed(1, 1, 1, 0),
            MACHINE_CODE_F64_REGISTER_CONVENTION_WITH_SSE_AGGREGATES
        );

        // An all-INTEGER call result the lifter resolves through `sret`
        // no longer counts toward the assumed memory-class ABI, but the
        // convention string still names the wider register budget.
        model.call_sites[0].arg_leaf_doubles = vec![false];
        model.call_sites[0].argument_shapes =
            vec![semantic_contract::CallValueShape::Scalar { leaf_count: 1 }];
        model.call_sites[0].result_leaf_doubles = vec![false; 5];
        model.call_sites[0].result_shape = semantic_contract::CallValueShape::Struct {
            leaf_count: 5,
            has_enum: false,
            has_f64: false,
        };
        assert!(!machine_tv_model_uses_wide_composite_aggregate(&model));
        assert!(machine_tv_model_uses_wide_composite_aggregate_convention(
            &model
        ));
        assert_eq!(
            machine_tv_register_convention_assumed(0, 1, 0, 1),
            MACHINE_CODE_REGISTER_CONVENTION_WITH_LLVM_AGGREGATES
        );
        assert_eq!(
            machine_tv_composite_aggregate_checks(1, 0).1,
            Vec::<&'static str>::new()
        );
    }

    #[test]
    fn object_symbols_correspond_only_when_definitions_match_and_references_are_declared() {
        let ir = tv::IrFunctionSurface {
            defined: BTreeSet::from(["main".to_owned(), "helper".to_owned()]),
            declared: BTreeSet::from(["external".to_owned(), "optional".to_owned()]),
            calls: BTreeMap::from([
                ("helper".to_owned(), BTreeSet::new()),
                ("main".to_owned(), BTreeSet::new()),
            ]),
            libc_calls: BTreeMap::new(),
            call_edges_checkable: true,
        };
        let object = |defined: &[&str], undefined: &[&str]| ObjectSymbolSurface {
            defined: defined.iter().map(|name| (*name).to_owned()).collect(),
            undefined: undefined.iter().map(|name| (*name).to_owned()).collect(),
            calls: BTreeMap::new(),
            call_targets: BTreeMap::new(),
            call_edges_checkable: true,
        };

        assert!(object_symbols_correspond(
            &ir,
            &object(&["main", "helper"], &["external"])
        ));
        assert!(!object_symbols_correspond(
            &ir,
            &object(&["main", "helper", "extra"], &["external"])
        ));
        assert!(!object_symbols_correspond(
            &ir,
            &object(&["main"], &["external"])
        ));
        assert!(!object_symbols_correspond(
            &ir,
            &object(&["main", "helper"], &["undeclared"])
        ));
        assert!(object_symbols_correspond(
            &ir,
            &object(&["main", "helper"], &["optional"])
        ));
        // Backend-injected compiler-rt 128-bit helpers count as
        // expected undefined symbols even though the IR never
        // declares them; other undeclared references still fail.
        assert!(object_symbols_correspond(
            &ir,
            &object(&["main", "helper"], &["__divti3", "__umodti3", "__multi3"])
        ));
        assert!(!object_symbols_correspond(
            &ir,
            &object(&["main", "helper"], &["__divti3", "undeclared"])
        ));
    }

    #[test]
    fn object_call_edges_correspond_ignores_ti3_libcall_edges() {
        let ir = tv::IrFunctionSurface {
            defined: BTreeSet::from(["div128".to_owned()]),
            declared: BTreeSet::new(),
            calls: BTreeMap::from([("div128".to_owned(), BTreeSet::new())]),
            libc_calls: BTreeMap::new(),
            call_edges_checkable: true,
        };
        let object = ObjectSymbolSurface {
            defined: ir.defined.clone(),
            undefined: BTreeSet::from(["__udivti3".to_owned()]),
            calls: BTreeMap::from([(
                "div128".to_owned(),
                BTreeSet::from(["__udivti3".to_owned()]),
            )]),
            call_targets: BTreeMap::new(),
            call_edges_checkable: true,
        };
        assert_eq!(object_call_edges_correspond(&ir, &object), Some(true));

        let undeclared_edge = ObjectSymbolSurface {
            calls: BTreeMap::from([("div128".to_owned(), BTreeSet::from(["other".to_owned()]))]),
            ..object.clone()
        };
        assert_eq!(
            object_call_edges_correspond(&ir, &undeclared_edge),
            Some(false)
        );

        // An IR that explicitly declares and calls `__udivti3` keeps
        // its edge: stripping it would reject a correct object.
        let explicit_ir = tv::IrFunctionSurface {
            declared: BTreeSet::from(["__udivti3".to_owned()]),
            calls: BTreeMap::from([(
                "div128".to_owned(),
                BTreeSet::from(["__udivti3".to_owned()]),
            )]),
            ..ir.clone()
        };
        assert_eq!(
            object_call_edges_correspond(&explicit_ir, &object),
            Some(true)
        );

        // When a different IR function calls the helper explicitly,
        // this caller's injected edge is still stripped.
        let other_caller_ir = tv::IrFunctionSurface {
            defined: BTreeSet::from(["div128".to_owned(), "explicit".to_owned()]),
            declared: BTreeSet::from(["__udivti3".to_owned()]),
            calls: BTreeMap::from([
                ("div128".to_owned(), BTreeSet::new()),
                (
                    "explicit".to_owned(),
                    BTreeSet::from(["__udivti3".to_owned()]),
                ),
            ]),
            libc_calls: BTreeMap::new(),
            call_edges_checkable: true,
        };
        let other_object = ObjectSymbolSurface {
            defined: other_caller_ir.defined.clone(),
            calls: BTreeMap::from([
                (
                    "div128".to_owned(),
                    BTreeSet::from(["__udivti3".to_owned()]),
                ),
                (
                    "explicit".to_owned(),
                    BTreeSet::from(["__udivti3".to_owned()]),
                ),
            ]),
            ..object.clone()
        };
        assert_eq!(
            object_call_edges_correspond(&other_caller_ir, &other_object),
            Some(true)
        );
    }

    #[test]
    fn object_call_edges_correspond_requires_checkable_equal_call_sets() {
        let calls = BTreeMap::from([
            ("other".to_owned(), BTreeSet::new()),
            (
                "visible".to_owned(),
                BTreeSet::from(["external_ref".to_owned(), "other".to_owned()]),
            ),
        ]);
        let ir = tv::IrFunctionSurface {
            defined: BTreeSet::from(["other".to_owned(), "visible".to_owned()]),
            declared: BTreeSet::from(["external_ref".to_owned()]),
            calls: calls.clone(),
            libc_calls: BTreeMap::new(),
            call_edges_checkable: true,
        };
        let object = ObjectSymbolSurface {
            defined: ir.defined.clone(),
            undefined: ir.declared.clone(),
            calls: calls.clone(),
            call_targets: BTreeMap::new(),
            call_edges_checkable: true,
        };
        assert_eq!(object_call_edges_correspond(&ir, &object), Some(true));

        let missing_edge = ObjectSymbolSurface {
            calls: BTreeMap::from([
                ("other".to_owned(), BTreeSet::new()),
                (
                    "visible".to_owned(),
                    BTreeSet::from(["external_ref".to_owned()]),
                ),
            ]),
            ..object.clone()
        };
        assert_eq!(
            object_call_edges_correspond(&ir, &missing_edge),
            Some(false)
        );

        let extra_edge = ObjectSymbolSurface {
            calls: BTreeMap::from([
                (
                    "other".to_owned(),
                    BTreeSet::from(["external_ref".to_owned()]),
                ),
                (
                    "visible".to_owned(),
                    BTreeSet::from(["external_ref".to_owned(), "other".to_owned()]),
                ),
            ]),
            ..object.clone()
        };
        assert_eq!(object_call_edges_correspond(&ir, &extra_edge), Some(false));

        let mut unchecked_ir = ir.clone();
        unchecked_ir.call_edges_checkable = false;
        assert_eq!(object_call_edges_correspond(&unchecked_ir, &object), None);
        let unchecked_object = ObjectSymbolSurface {
            call_edges_checkable: false,
            ..object
        };
        assert_eq!(object_call_edges_correspond(&ir, &unchecked_object), None);
    }

    #[test]
    fn object_symbol_surface_parses_compiled_object_symbols() {
        let dir = tempfile::tempdir().unwrap();
        let ir_path = dir.path().join("symbols.ll");
        let object_path = dir.path().join("symbols.o");
        let ir = br#"
declare i64 @external_ref(i64)

define internal i64 @local_helper() {
entry:
  ret i64 1
}

define i64 @visible() {
entry:
  %external = call i64 @external_ref(i64 2)
  %local = call i64 @local_helper()
  %sum = add i64 %external, %local
  ret i64 %sum
}
"#;
        fs::write(&ir_path, ir).unwrap();
        mumei_emit_llvm::codegen::compile_llvm_ir_to_object_recorded(&ir_path, &object_path)
            .unwrap();

        let surface = object_symbol_surface(&fs::read(&object_path).unwrap()).unwrap();
        assert_eq!(surface.defined, BTreeSet::from(["visible".to_owned()]));
        assert_eq!(
            surface.undefined,
            BTreeSet::from(["external_ref".to_owned()])
        );
        let ir_surface = tv::ir_function_surface(ir).unwrap();
        assert!(object_symbols_correspond(&ir_surface, &surface));
        assert!(!ir_surface.call_edges_checkable);
        assert!(
            !surface.call_edges_checkable,
            "observed object surface: {surface:#?}"
        );
        assert_eq!(object_call_edges_correspond(&ir_surface, &surface), None);
        assert!(object_symbol_surface(b"not an object").is_err());
    }

    #[test]
    fn object_symbol_surface_tracks_checkable_direct_call_edges() {
        let dir = tempfile::tempdir().unwrap();
        let ir_path = dir.path().join("calls.ll");
        let object_path = dir.path().join("calls.o");
        let ir = br#"
declare i64 @external_ref(i64)

define i64 @other() {
entry:
  ret i64 3
}

define i64 @visible() {
entry:
  %external = call i64 @external_ref(i64 2)
  %other = call i64 @other()
  %sum = add i64 %external, %other
  ret i64 %sum
}
"#;
        fs::write(&ir_path, ir).unwrap();
        mumei_emit_llvm::codegen::compile_llvm_ir_to_object_recorded(&ir_path, &object_path)
            .unwrap();

        let ir_surface = tv::ir_function_surface(ir).unwrap();
        let object_surface = object_symbol_surface(&fs::read(&object_path).unwrap()).unwrap();
        let expected = BTreeMap::from([
            ("other".to_owned(), BTreeSet::new()),
            (
                "visible".to_owned(),
                BTreeSet::from(["external_ref".to_owned(), "other".to_owned()]),
            ),
        ]);
        assert!(ir_surface.call_edges_checkable);
        assert!(object_surface.call_edges_checkable);
        assert_eq!(ir_surface.calls, expected);
        assert_eq!(object_surface.calls, expected);
        assert_eq!(
            object_surface
                .call_targets
                .get("visible")
                .unwrap()
                .values()
                .map(|target| target.name.clone())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["external_ref".to_owned(), "other".to_owned()])
        );
        assert!(object_surface.call_targets.get("other").unwrap().is_empty());
        assert_eq!(
            object_call_edges_correspond(&ir_surface, &object_surface),
            Some(true)
        );
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn machine_tv_rejects_patched_rela_addend_from_object_surface() {
        let dir = tempfile::tempdir().unwrap();
        let ir_path = dir.path().join("calls.ll");
        let object_path = dir.path().join("calls.o");
        let ir = br#"
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
        fs::write(&ir_path, ir).unwrap();
        mumei_emit_llvm::codegen::compile_llvm_ir_to_object_recorded(&ir_path, &object_path)
            .unwrap();
        let mut object_bytes = fs::read(&object_path).unwrap();
        let addend_file_offset = {
            let file = object::File::parse(object_bytes.as_slice()).unwrap();
            assert_eq!(file.format(), BinaryFormat::Elf);
            assert_eq!(file.architecture(), Architecture::X86_64);
            let text_section = file.section_by_name(".text").unwrap();
            let rela_text_section = file.section_by_name(".rela.text").unwrap();
            let (rela_file_offset, rela_size) = rela_text_section.file_range().unwrap();
            let caller = file
                .symbols()
                .find(|symbol| {
                    symbol.name().ok() == Some("caller")
                        && symbol.is_global()
                        && symbol.is_definition()
                        && symbol.kind() == SymbolKind::Text
                })
                .unwrap();
            let caller_start = caller.address();
            let caller_end = caller_start.checked_add(caller.size()).unwrap();
            let relocation_entry_index = text_section
                .relocations()
                .enumerate()
                .find_map(|(entry_index, (offset, relocation))| {
                    if !(caller_start <= offset && offset < caller_end)
                        || relocation.size() != 32
                        || relocation.has_implicit_addend()
                        || relocation.addend() != -4
                        || !matches!(
                            relocation.flags(),
                            RelocationFlags::Elf { r_type }
                                if r_type == object::elf::R_X86_64_PLT32
                                    || r_type == object::elf::R_X86_64_PC32
                        )
                    {
                        return None;
                    }
                    let RelocationTarget::Symbol(symbol_index) = relocation.target() else {
                        return None;
                    };
                    let target = file.symbol_by_index(symbol_index).ok()?;
                    (target.name().ok() == Some("callee")).then_some(entry_index)
                })
                .unwrap();
            let addend_offset = rela_file_offset
                .checked_add(
                    u64::try_from(relocation_entry_index)
                        .unwrap()
                        .checked_mul(24)
                        .unwrap(),
                )
                .unwrap()
                .checked_add(16)
                .unwrap();
            assert!(addend_offset
                .checked_add(8)
                .is_some_and(|end| end <= rela_file_offset + rela_size));
            usize::try_from(addend_offset).unwrap()
        };
        object_bytes[addend_file_offset..addend_file_offset + 8]
            .copy_from_slice(&(-3i64).to_le_bytes());

        let surface = object_symbol_surface(&object_bytes).unwrap();
        let call_targets = surface.call_targets.get("caller").unwrap();
        assert_eq!(call_targets.len(), 1);
        assert_eq!(call_targets.values().next().unwrap().addend, -3);
        let (address, function_bytes) = object_function_bytes(&object_bytes, "caller").unwrap();
        let mut config = z3::Config::new();
        config.set_timeout_msec(u64::from(machine_tv::TIMEOUT_MS));
        config.set_param_value("rlimit", &machine_tv::RESOURCE_LIMIT.to_string());
        let context = z3::Context::new(&config);
        let source = "atom callee(x: i64) -> i64 requires: true; ensures: true; body: { x };\n\
            atom caller(x: i64) -> i64 requires: true; ensures: true; body: { callee(x) };";
        let model = semantic_contract::encode_pure_i64_module(
            source,
            &context,
            true,
            RefinedAdmission::Reject,
        )
        .unwrap()
        .into_iter()
        .find(|model| model.function_name == "caller")
        .unwrap();
        assert_eq!(
            machine_tv::check_function(&function_bytes, address, &model, call_targets, &context),
            FunctionOutcome::OutsideProfile("unsupported_instruction")
        );
    }

    fn executable_symbol_file_offset(bytes: &[u8], name: &str, offset: u64) -> usize {
        let file = object::File::parse(bytes).unwrap();
        let symbol = file
            .symbols()
            .find(|symbol| {
                symbol.name().ok() == Some(name)
                    && symbol.is_global()
                    && symbol.is_definition()
                    && symbol.kind() == SymbolKind::Text
            })
            .unwrap();
        assert!(offset < symbol.size());
        let section = file
            .section_by_index(symbol.section_index().unwrap())
            .unwrap();
        let (section_file_offset, _) = section.file_range().unwrap();
        let symbol_section_offset = symbol.address().checked_sub(section.address()).unwrap();
        usize::try_from(
            section_file_offset
                .checked_add(symbol_section_offset)
                .unwrap()
                .checked_add(offset)
                .unwrap(),
        )
        .unwrap()
    }

    fn object_call_relocation_offset(
        object_bytes: &[u8],
        function_name: &str,
        target_name: &str,
    ) -> u64 {
        let file = object::File::parse(object_bytes).unwrap();
        let function = file
            .symbols()
            .find(|symbol| {
                symbol.name().ok() == Some(function_name)
                    && symbol.is_global()
                    && symbol.is_definition()
                    && symbol.kind() == SymbolKind::Text
            })
            .unwrap();
        let section_index = function.section_index().unwrap();
        let function_end = function.address().checked_add(function.size()).unwrap();
        for section in file.sections() {
            if section.index() != section_index || section.kind() != SectionKind::Text {
                continue;
            }
            for (offset, relocation) in section.relocations() {
                if !(function.address() <= offset && offset < function_end) {
                    continue;
                }
                let RelocationTarget::Symbol(symbol_index) = relocation.target() else {
                    continue;
                };
                let target = file.symbol_by_index(symbol_index).unwrap();
                if target.name().ok() == Some(target_name) {
                    return offset - function.address();
                }
            }
        }
        panic!("missing object call relocation");
    }

    #[test]
    fn object_link_correspondence_checks_function_bytes_and_resolved_calls() {
        let dir = tempfile::tempdir().unwrap();
        let ir_path = dir.path().join("module.ll");
        let object_path = dir.path().join("module.o");
        let executable_path = dir.path().join("module");
        let ir = br#"
define i64 @helper(i64 %argument) {
entry:
  %value = add i64 %argument, 1
  ret i64 %value
}

define i64 @__mumei_user_main() {
entry:
  %value = call i64 @helper(i64 5)
  ret i64 %value
}

define i32 @main() {
entry:
  %value = call i64 @__mumei_user_main()
  %exit = trunc i64 %value to i32
  ret i32 %exit
}
"#;
        fs::write(&ir_path, ir).unwrap();
        mumei_emit_llvm::codegen::compile_llvm_ir_to_object_recorded(&ir_path, &object_path)
            .unwrap();
        linker::link_recorded(std::slice::from_ref(&object_path), &executable_path).unwrap();

        let object_bytes = fs::read(&object_path).unwrap();
        let executable_bytes = fs::read(&executable_path).unwrap();
        let comparison = object_link_corresponds(&object_bytes, &executable_bytes)
            .unwrap()
            .unwrap();
        assert!(comparison.0);
        assert_eq!(comparison.1, 2);

        // Step 1 metadata: the record names every function's object and
        // executable placement, and every relocation's section, apply
        // position, kind, target, addend and resolved value.
        let record = &comparison.2;
        let function_names: Vec<&str> = record
            .functions
            .iter()
            .map(|function| function.object_symbol.as_str())
            .collect();
        for name in ["helper", "main", "__mumei_user_main"] {
            assert!(
                function_names.contains(&name),
                "missing function record for {name}"
            );
        }
        for function in &record.functions {
            assert_eq!(function.ir_function, function.object_symbol);
            assert!(!function.object_section.is_empty());
            assert!(function.object_size > 0);
            assert!(function.executable_address.unwrap_or(0) > 0);
        }
        // Step 4 全件照合: the record lists every relocation entry —
        // the two `.text` call sites plus the `.eh_frame` unwind
        // records the object always carries.
        let text_relocs: Vec<_> = record
            .relocations
            .iter()
            .filter(|relocation| relocation.object_section == ".text")
            .collect();
        assert_eq!(text_relocs.len(), 2);
        let reloc_targets: Vec<&str> = text_relocs
            .iter()
            .map(|relocation| relocation.target_symbol.as_str())
            .collect();
        for target in ["helper", "__mumei_user_main"] {
            assert!(
                reloc_targets.contains(&target),
                "missing relocation to {target}"
            );
        }
        for relocation in &text_relocs {
            assert_eq!(relocation.kind, "R_X86_64_PLT32");
            assert_eq!(relocation.addend, -4);
            assert!(relocation.resolved_value.is_some());
        }
        assert!(
            record
                .relocations
                .iter()
                .any(|relocation| relocation.object_section == ".eh_frame"),
            "non-Text relocations must be recorded too"
        );
        for relocation in &record.relocations {
            assert!(relocation.resolved_value.is_some());
        }

        let helper_offset = executable_symbol_file_offset(&executable_bytes, "helper", 0);
        let mut altered_helper = executable_bytes.clone();
        altered_helper[helper_offset] ^= 1;
        assert!(matches!(
            object_link_corresponds(&object_bytes, &altered_helper),
            Ok(Some((false, _, _)))
        ));

        let call_field_offset =
            object_call_relocation_offset(&object_bytes, "__mumei_user_main", "helper");
        assert_eq!(call_field_offset, 7);
        let call_field_file_offset = executable_symbol_file_offset(
            &executable_bytes,
            "__mumei_user_main",
            call_field_offset,
        );
        let mut altered_call = executable_bytes.clone();
        altered_call[call_field_file_offset] ^= 1;
        assert!(matches!(
            object_link_corresponds(&object_bytes, &altered_call),
            Ok(Some((false, _, _)))
        ));

        assert!(object_link_corresponds(&object_bytes, b"not an executable").is_err());
    }

    fn executable_script(path: &Path, body: &str) {
        std::fs::write(path, body).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    #[test]
    fn refuted_or_inconclusive_queries_never_write_json_or_text() {
        for json in [false, true] {
            for (result, expected) in [
                (LlvmRecheck::Different, Outcome::Different),
                (LlvmRecheck::Inconclusive, Outcome::Inconclusive),
            ] {
                let mut output = Vec::new();
                assert_eq!(
                    write_llvm_recheck(result, json, &mut output).unwrap(),
                    expected
                );
                assert!(output.is_empty());
            }
        }
    }

    #[test]
    fn stdout_observation_fails_closed_one_byte_past_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let printer = dir.path().join("printer.sh");
        std::fs::write(&printer, "#!/bin/sh\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&printer).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&printer, perms).unwrap();
        }
        // Exactly the cap is observable; one byte past it is not —
        // a pin must cover the whole stream, never a truncation. Assert
        // on the inner result so a failure names its reason.
        for (bytes, expected) in [
            (OBSERVED_CHANNEL_MAX_BYTES, true),
            (OBSERVED_CHANNEL_MAX_BYTES + 1, false),
        ] {
            std::fs::write(&printer, format!("#!/bin/sh\nhead -c {bytes} /dev/zero\n")).unwrap();
            let observed = observed_stream_inner(&printer, ObservedChannel::Stdout);
            assert_eq!(observed.is_ok(), expected, "{bytes} bytes: {observed:?}");
        }
        // The observable digest covers every emitted byte.
        std::fs::write(&printer, "#!/bin/sh\nprintf 'out'\n").unwrap();
        assert_eq!(observed_stdout(&printer), Some(sha256(b"out")));
        // A missing executable is unobservable, not a pin.
        assert_eq!(observed_stdout(&dir.path().join("missing")), None);
    }

    #[test]
    fn stderr_observation_drains_the_stderr_channel() {
        let dir = tempfile::tempdir().unwrap();
        let printer = dir.path().join("printer.sh");
        std::fs::write(&printer, "#!/bin/sh\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&printer).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&printer, perms).unwrap();
        }
        // Distinct payloads per channel: a swapped drain would digest
        // the stdout bytes — or the empty stream — and fail here.
        std::fs::write(&printer, "#!/bin/sh\nprintf 'out'\nprintf 'err' >&2\n").unwrap();
        assert_eq!(observed_stderr(&printer), Some(sha256(b"err")));
        assert_eq!(observed_stdout(&printer), Some(sha256(b"out")));
        // The cap bounds stderr the same way: exactly the cap is
        // observable, one byte past it is not.
        for (bytes, expected) in [
            (OBSERVED_CHANNEL_MAX_BYTES, true),
            (OBSERVED_CHANNEL_MAX_BYTES + 1, false),
        ] {
            std::fs::write(
                &printer,
                format!("#!/bin/sh\nhead -c {bytes} /dev/zero >&2\n"),
            )
            .unwrap();
            let observed = observed_stream_inner(&printer, ObservedChannel::Stderr);
            assert_eq!(observed.is_ok(), expected, "{bytes} bytes: {observed:?}");
        }
        // A missing executable is unobservable, not a pin.
        assert_eq!(observed_stderr(&dir.path().join("missing")), None);
    }

    #[test]
    fn invocation_observation_captures_both_channels_and_exit_status_once() {
        let dir = tempfile::tempdir().unwrap();
        let printer = dir.path().join("printer.sh");
        executable_script(
            &printer,
            "#!/bin/sh\nprintf 'out'\nprintf 'err' >&2\nexit 7\n",
        );

        let observation = observed_invocation(&printer).unwrap();
        assert_eq!(observation.exit_status, 7);
        assert_eq!(observation.stdout_sha256, sha256(b"out"));
        assert_eq!(observation.stderr_sha256, sha256(b"err"));
    }

    #[test]
    fn observation_command_resolves_a_relative_executable() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let printer = sub.join("printer.sh");
        executable_script(
            &printer,
            "#!/bin/sh\nprintf '%s|%s' \"${HOME-unset}\" \"$(pwd -P)\"",
        );
        let canonical_dir = sub.canonicalize().unwrap();
        let expected = format!("unset|{}", canonical_dir.display());
        let observation = observed_invocation(&printer).unwrap();
        assert_eq!(observation.stdout_sha256, sha256(expected.as_bytes()));

        let command = observation_command(Path::new("rel/exe"));
        let program = Path::new(command.get_program());
        assert!(program.is_absolute());
        assert!(program.ends_with(Path::new("rel/exe")));
        let expected_cwd = std::env::current_dir().unwrap().join("rel");
        assert_eq!(command.get_current_dir(), Some(expected_cwd.as_path()));
    }

    #[test]
    fn invocation_observation_uses_an_empty_environment_and_executable_directory() {
        let dir = tempfile::tempdir().unwrap();
        let canonical_dir = dir.path().canonicalize().unwrap();
        let printer = dir.path().join("printer.sh");
        executable_script(
            &printer,
            "#!/bin/sh\nprintf '%s|%s' \"${HOME-unset}\" \"$(pwd -P)\"",
        );
        let expected = format!("unset|{}", canonical_dir.display());
        let observation = observed_invocation(&printer).unwrap();
        assert_eq!(observation.stdout_sha256, sha256(expected.as_bytes()));
    }

    #[test]
    fn stdout_observation_uses_an_empty_environment_and_executable_directory() {
        let dir = tempfile::tempdir().unwrap();
        let canonical_dir = dir.path().canonicalize().unwrap();
        let printer = dir.path().join("printer.sh");
        executable_script(
            &printer,
            "#!/bin/sh\nprintf '%s|%s' \"${HOME-unset}\" \"$(pwd -P)\"",
        );
        let expected = format!("unset|{}", canonical_dir.display());
        assert_eq!(
            observed_stream_inner(&printer, ObservedChannel::Stdout).unwrap(),
            sha256(expected.as_bytes())
        );
    }

    #[test]
    fn exit_status_observation_uses_an_empty_environment_and_executable_directory() {
        let dir = tempfile::tempdir().unwrap();
        let canonical_dir = dir.path().canonicalize().unwrap();
        let executable = dir.path().join("check-context.sh");
        executable_script(
            &executable,
            &format!(
                "#!/bin/sh\n[ -z \"${{HOME+x}}\" ] && [ \"$(pwd -P)\" = \"{}\" ] && exit 0\nexit 9\n",
                canonical_dir.display()
            ),
        );
        assert_eq!(observed_exit_status(&executable), Some(0));
    }

    #[test]
    fn invocation_observation_caps_each_channel_independently() {
        let dir = tempfile::tempdir().unwrap();
        let printer = dir.path().join("printer.sh");
        for (stdout_bytes, stderr_bytes, over_cap) in [
            (
                OBSERVED_CHANNEL_MAX_BYTES,
                OBSERVED_CHANNEL_MAX_BYTES,
                false,
            ),
            (
                OBSERVED_CHANNEL_MAX_BYTES + 1,
                OBSERVED_CHANNEL_MAX_BYTES,
                true,
            ),
            (
                OBSERVED_CHANNEL_MAX_BYTES,
                OBSERVED_CHANNEL_MAX_BYTES + 1,
                true,
            ),
        ] {
            executable_script(
                &printer,
                &format!(
                    "#!/bin/sh\nhead -c {stdout_bytes} /dev/zero\nhead -c {stderr_bytes} /dev/zero >&2\n"
                ),
            );
            let result = observed_invocation(&printer);
            if over_cap {
                assert_eq!(result, Err(ObservedStreamFailure::OutputOverCap));
            } else {
                let observation = result.unwrap();
                let zeroes = vec![0; OBSERVED_CHANNEL_MAX_BYTES];
                assert_eq!(observation.stdout_sha256, sha256(&zeroes));
                assert_eq!(observation.stderr_sha256, sha256(&zeroes));
            }
        }
    }

    #[test]
    fn invocation_observation_maps_signal_to_inconclusive() {
        let dir = tempfile::tempdir().unwrap();
        let killer = dir.path().join("killer.sh");
        executable_script(&killer, "#!/bin/sh\nkill -9 $$\n");
        assert_eq!(
            observed_invocation(&killer),
            Err(ObservedStreamFailure::TimedOutOrKilled)
        );
    }

    #[test]
    fn invocation_observation_reports_spawn_failure() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            observed_invocation(&dir.path().join("missing")),
            Err(ObservedStreamFailure::SpawnFailed)
        );
    }

    #[test]
    fn observation_pins_are_comparable_only_for_matching_or_legacy_envelopes() {
        let link_build = |observation_envelope| LinkBuild {
            driver: "cc".into(),
            driver_version: "1".into(),
            driver_target: "x86_64-linux".into(),
            linker: "gnu-ld".into(),
            linker_version: "1".into(),
            arguments_sha256: "args".into(),
            runtime_source_sha256: None,
            canonical_arguments_sha256: None,
            expected_exit_status: None,
            expected_stdout_sha256: None,
            expected_stderr_sha256: None,
            observation_envelope,
        };
        assert!(observation_pins_comparable(&link_build(None)));
        assert!(observation_pins_comparable(&link_build(Some(
            pinned_observation_envelope()
        ))));
        assert!(!observation_pins_comparable(&link_build(Some(
            ObservationEnvelope {
                environment: "inherited".into(),
                working_directory: OBSERVATION_WORKING_DIRECTORY.into(),
            }
        ))));
    }

    #[test]
    fn observation_envelope_and_host_are_in_observation_acceptances() {
        let observation_acceptances = [
            &MULTIFILE_EXIT_STATUS_ACCEPTANCE,
            &MULTIFILE_EXIT_STATUS_ACCEPTANCE_ARGV_UNPINNED,
            &EXIT_STATUS_ACCEPTANCE,
            &EXIT_STATUS_ACCEPTANCE_ARGV_UNPINNED,
            &MULTIFILE_STDOUT_ACCEPTANCE,
            &MULTIFILE_STDOUT_ACCEPTANCE_ARGV_UNPINNED,
            &STDOUT_ACCEPTANCE,
            &STDOUT_ACCEPTANCE_ARGV_UNPINNED,
            &MULTIFILE_STDERR_ACCEPTANCE,
            &MULTIFILE_STDERR_ACCEPTANCE_ARGV_UNPINNED,
            &STDERR_ACCEPTANCE,
            &STDERR_ACCEPTANCE_ARGV_UNPINNED,
            &MULTIFILE_RUNTIME_OBSERVATION_ACCEPTANCE,
            &MULTIFILE_RUNTIME_OBSERVATION_ACCEPTANCE_ARGV_UNPINNED,
            &RUNTIME_OBSERVATION_ACCEPTANCE,
            &RUNTIME_OBSERVATION_ACCEPTANCE_ARGV_UNPINNED,
        ];
        assert_eq!(observation_acceptances.len(), 16);
        for acceptance in observation_acceptances {
            let mut json_output = Vec::new();
            write_native_acceptance(
                Outcome::EquivalentWithinProfile,
                true,
                acceptance,
                None,
                Some("recorded"),
                &mut json_output,
            )
            .unwrap();
            let descriptor: serde_json::Value = serde_json::from_slice(&json_output).unwrap();
            assert_eq!(descriptor["invocation"], observation_invocation_envelope());
            assert_eq!(descriptor["invocation"]["timeout_ms"], 30000);
            assert_eq!(descriptor["invocation"]["channel_cap_bytes"], 65536);
            assert_eq!(descriptor["host"], observation_host());
            assert_eq!(descriptor["host"]["os"], std::env::consts::OS);
            assert_eq!(descriptor["pin_envelope"], "recorded");

            let mut text_output = Vec::new();
            write_native_acceptance(
                Outcome::EquivalentWithinProfile,
                false,
                acceptance,
                None,
                Some("recorded"),
                &mut text_output,
            )
            .unwrap();
            assert_eq!(text_output, acceptance.text.as_bytes());
        }

        for acceptance in [
            &EVIDENCE_ACCEPTANCE,
            &MULTIFILE_EVIDENCE_ACCEPTANCE,
            &MULTIFILE_ARTIFACT_ACCEPTANCE,
            &MULTIFILE_ARTIFACT_ACCEPTANCE_ARGV_UNPINNED,
            &ARTIFACT_ACCEPTANCE,
            &ARTIFACT_ACCEPTANCE_ARGV_UNPINNED,
        ] {
            let mut json_output = Vec::new();
            write_native_acceptance(
                Outcome::EquivalentWithinProfile,
                true,
                acceptance,
                None,
                Some("recorded"),
                &mut json_output,
            )
            .unwrap();
            let descriptor: serde_json::Value = serde_json::from_slice(&json_output).unwrap();
            assert!(descriptor.get("invocation").is_none());
            assert!(descriptor.get("host").is_none());
            assert!(descriptor.get("pin_envelope").is_none());

            let mut text_output = Vec::new();
            write_native_acceptance(
                Outcome::EquivalentWithinProfile,
                false,
                acceptance,
                None,
                Some("recorded"),
                &mut text_output,
            )
            .unwrap();
            assert_eq!(text_output, acceptance.text.as_bytes());
        }

        let mut unpinned_output = Vec::new();
        write_native_acceptance(
            Outcome::EquivalentWithinProfile,
            true,
            &STDERR_ACCEPTANCE,
            None,
            None,
            &mut unpinned_output,
        )
        .unwrap();
        let unpinned: serde_json::Value = serde_json::from_slice(&unpinned_output).unwrap();
        assert_eq!(unpinned["invocation"], observation_invocation_envelope());
        assert_eq!(unpinned["host"], observation_host());
        assert!(unpinned.get("pin_envelope").is_none());
    }

    #[test]
    fn native_acceptance_descriptors_use_canonical_runtime_check_names() {
        let acceptances = [
            &EVIDENCE_ACCEPTANCE,
            &MULTIFILE_EVIDENCE_ACCEPTANCE,
            &MULTIFILE_ARTIFACT_ACCEPTANCE,
            &MULTIFILE_ARTIFACT_ACCEPTANCE_ARGV_UNPINNED,
            &ARTIFACT_ACCEPTANCE,
            &ARTIFACT_ACCEPTANCE_ARGV_UNPINNED,
            &MULTIFILE_EXIT_STATUS_ACCEPTANCE,
            &MULTIFILE_EXIT_STATUS_ACCEPTANCE_ARGV_UNPINNED,
            &EXIT_STATUS_ACCEPTANCE,
            &EXIT_STATUS_ACCEPTANCE_ARGV_UNPINNED,
            &MULTIFILE_STDOUT_ACCEPTANCE,
            &MULTIFILE_STDOUT_ACCEPTANCE_ARGV_UNPINNED,
            &STDOUT_ACCEPTANCE,
            &STDOUT_ACCEPTANCE_ARGV_UNPINNED,
            &MULTIFILE_STDERR_ACCEPTANCE,
            &MULTIFILE_STDERR_ACCEPTANCE_ARGV_UNPINNED,
            &STDERR_ACCEPTANCE,
            &STDERR_ACCEPTANCE_ARGV_UNPINNED,
            &MULTIFILE_RUNTIME_OBSERVATION_ACCEPTANCE,
            &MULTIFILE_RUNTIME_OBSERVATION_ACCEPTANCE_ARGV_UNPINNED,
            &RUNTIME_OBSERVATION_ACCEPTANCE,
            &RUNTIME_OBSERVATION_ACCEPTANCE_ARGV_UNPINNED,
        ];
        // Update this literal when adding a NativeAcceptance descriptor.
        assert_eq!(acceptances.len(), 22);
        let canonical_runtime_checks = [
            "os_exit_status_observation",
            "os_stdout_observation",
            "os_stderr_observation",
            "runtime_observation_semantics",
        ];
        let runtime_observation_acceptances = [
            &MULTIFILE_RUNTIME_OBSERVATION_ACCEPTANCE,
            &MULTIFILE_RUNTIME_OBSERVATION_ACCEPTANCE_ARGV_UNPINNED,
            &RUNTIME_OBSERVATION_ACCEPTANCE,
            &RUNTIME_OBSERVATION_ACCEPTANCE_ARGV_UNPINNED,
        ];

        for acceptance in acceptances {
            for check in acceptance.performed.iter().chain(acceptance.not_performed) {
                assert_ne!(*check, "os_exit_status");
                if (check.starts_with("os_") && check.ends_with("_observation"))
                    || *check == "runtime_observation_semantics"
                {
                    assert!(
                        canonical_runtime_checks.contains(check),
                        "unknown runtime observation check: {check}"
                    );
                }
            }
            for check in acceptance.performed {
                assert!(!acceptance.not_performed.contains(check));
            }
        }

        for acceptance in runtime_observation_acceptances {
            for check in canonical_runtime_checks {
                assert!(acceptance.performed.contains(&check));
            }
        }
    }

    #[test]
    fn claimed_success_cannot_replace_a_fresh_refuted_or_inconclusive_result() {
        let saved = evidence::decode(include_bytes!(
            "../../tests/fixtures/verification-evidence/native_consumer_shape.json"
        ))
        .unwrap();
        for (fresh, expected) in [
            (LlvmRecheck::Different, Outcome::Different),
            (LlvmRecheck::Inconclusive, Outcome::Inconclusive),
        ] {
            assert_eq!(compare_evidence(fresh, &saved).unwrap(), expected);
        }
    }

    #[test]
    fn object_translation_correspondence_checks_report_checked_and_inconsistent_items() {
        let dir = tempfile::tempdir().unwrap();
        let ir_path = dir.path().join("module.ll");
        let object_path = dir.path().join("module.o");
        let executable_path = dir.path().join("module");
        let ir = br#"
@greeting = private unnamed_addr constant [6 x i8] c"hello\00"
@counter = global i32 7
@scratch = global [8 x i8] zeroinitializer

define i64 @helper(i64 %argument) {
entry:
  %value = add i64 %argument, 1
  ret i64 %value
}

define i64 @__mumei_user_main() {
entry:
  %value = call i64 @helper(i64 5)
  ret i64 %value
}

define i32 @main() {
entry:
  %value = call i64 @__mumei_user_main()
  %exit = trunc i64 %value to i32
  ret i32 %exit
}
"#;
        fs::write(&ir_path, ir).unwrap();
        mumei_emit_llvm::codegen::compile_llvm_ir_to_object_recorded(&ir_path, &object_path)
            .unwrap();
        linker::link_recorded(std::slice::from_ref(&object_path), &executable_path).unwrap();
        let object_bytes = fs::read(&object_path).unwrap();
        let executable_bytes = fs::read(&executable_path).unwrap();
        let ir_surface = tv::ir_function_surface(ir).unwrap();
        let ir_data = tv::ir_data_globals(ir).unwrap();
        let ir_attrs = tv::ir_symbol_attributes(ir).unwrap();
        assert_eq!(
            ir_data,
            BTreeMap::from([
                ("counter".to_owned(), Some(vec![7, 0, 0, 0])),
                ("greeting".to_owned(), Some(b"hello\0".as_slice().to_vec())),
                ("scratch".to_owned(), Some(vec![0; 8])),
            ])
        );
        let record = &object_link_corresponds(&object_bytes, &executable_bytes)
            .unwrap()
            .unwrap()
            .2;

        let results_of = |report: &serde_json::Value| -> BTreeMap<String, String> {
            report["checks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|check| {
                    (
                        check["item"].as_str().unwrap().to_owned(),
                        check["result"].as_str().unwrap().to_owned(),
                    )
                })
                .collect()
        };
        // A liftable function comparing equivalent makes the direct
        // IR ↔ object item `checked`; every metadata item checked and
        // the aggregate stays `checked_correspondence`.
        let equivalent = BTreeMap::from([
            ("helper".to_owned(), "equivalent"),
            ("__mumei_user_main".to_owned(), "unsupported_signature"),
            ("main".to_owned(), "unsupported_signature"),
        ]);
        let hir_llvm = BTreeMap::from([
            ("helper".to_owned(), "checked"),
            ("main".to_owned(), "checked"),
        ]);
        let report = object_translation_correspondence_checks(
            &ir_surface,
            &executable_bytes,
            record,
            IrCorrespondenceInputs {
                equivalence: Some(&equivalent),
                hir_llvm: Some(&hir_llvm),
                data_globals: Some(&ir_data),
                symbol_attributes: Some(&ir_attrs),
            },
            &object_bytes,
        );
        assert_eq!(report["status"], "checked_correspondence");
        let lines = object_translation_correspondence_report_lines(&report);
        assert_eq!(lines.len(), 1);
        assert_eq!(
            lines[0],
            "Object translation correspondence: checked_correspondence (10 of 10 items checked)."
        );
        let results = results_of(&report);
        assert_eq!(
            results,
            BTreeMap::from([
                ("function_coverage".to_owned(), "checked".to_owned()),
                ("function_count".to_owned(), "checked".to_owned()),
                ("relocation_targets".to_owned(), "checked".to_owned()),
                ("relocation_bounds".to_owned(), "checked".to_owned()),
                (
                    "relocation_resolved_values".to_owned(),
                    "checked".to_owned()
                ),
                ("relocation_applied_values".to_owned(), "checked".to_owned()),
                (
                    "llvm_ir_object_equivalence".to_owned(),
                    "checked".to_owned()
                ),
                (
                    "hir_llvm_model_correspondence".to_owned(),
                    "checked".to_owned()
                ),
                (
                    "object_data_correspondence".to_owned(),
                    "checked".to_owned()
                ),
                (
                    "object_symbol_attribute_correspondence".to_owned(),
                    "checked".to_owned()
                ),
            ])
        );

        // A divergent function makes the item — and the aggregate —
        // inconsistent even when every metadata item checked.
        let divergent = BTreeMap::from([("helper".to_owned(), "different")]);
        let report = object_translation_correspondence_checks(
            &ir_surface,
            &executable_bytes,
            record,
            IrCorrespondenceInputs {
                equivalence: Some(&divergent),
                ..Default::default()
            },
            &object_bytes,
        );
        assert_eq!(report["status"], "inconsistent_correspondence");
        assert_eq!(
            results_of(&report)["llvm_ir_object_equivalence"],
            "inconsistent"
        );
        // The text report names the inconsistent item and its detail
        // so the divergence is visible without `--json`.
        let lines = object_translation_correspondence_report_lines(&report);
        assert!(
            lines[0].starts_with("Object translation correspondence: inconsistent_correspondence")
        );
        assert!(lines.iter().any(|line| {
            line.contains("inconsistent: llvm_ir_object_equivalence")
                && line.contains("helper: different")
        }));

        // A divergent atom makes the hir→llvm_ir item inconsistent —
        // and the aggregate follows.
        let hir_diverged = BTreeMap::from([("helper".to_owned(), "inconsistent")]);
        let report = object_translation_correspondence_checks(
            &ir_surface,
            &executable_bytes,
            record,
            IrCorrespondenceInputs {
                equivalence: Some(&equivalent),
                hir_llvm: Some(&hir_diverged),
                ..Default::default()
            },
            &object_bytes,
        );
        assert_eq!(report["status"], "inconsistent_correspondence");
        assert_eq!(
            results_of(&report)["hir_llvm_model_correspondence"],
            "inconsistent"
        );

        // Atoms all outside the leaf subset keep the item unchecked.
        let hir_outside = BTreeMap::from([("helper".to_owned(), "unchecked")]);
        let report = object_translation_correspondence_checks(
            &ir_surface,
            &executable_bytes,
            record,
            IrCorrespondenceInputs {
                equivalence: Some(&equivalent),
                hir_llvm: Some(&hir_outside),
                ..Default::default()
            },
            &object_bytes,
        );
        assert_eq!(report["status"], "unchecked_not_translation_validated");
        assert_eq!(
            results_of(&report)["hir_llvm_model_correspondence"],
            "unchecked"
        );

        // Nothing liftable keeps the item honestly `unchecked`, which
        // drops the aggregate back to unchecked_not_translation_validated.
        let unliftable = BTreeMap::from([("helper".to_owned(), "unsupported_signature")]);
        let report = object_translation_correspondence_checks(
            &ir_surface,
            &executable_bytes,
            record,
            IrCorrespondenceInputs {
                equivalence: Some(&unliftable),
                ..Default::default()
            },
            &object_bytes,
        );
        assert_eq!(report["status"], "unchecked_not_translation_validated");
        assert_eq!(
            results_of(&report)["llvm_ir_object_equivalence"],
            "unchecked"
        );

        // An attempted-but-undecidable comparison (`unknown`) is not a
        // skip: the item stays `unchecked` even when other functions
        // compared equivalent (fail-closed).
        let undecidable = BTreeMap::from([
            ("helper".to_owned(), "equivalent"),
            ("main".to_owned(), "unknown"),
        ]);
        let report = object_translation_correspondence_checks(
            &ir_surface,
            &executable_bytes,
            record,
            IrCorrespondenceInputs {
                equivalence: Some(&undecidable),
                hir_llvm: Some(&hir_llvm),
                ..Default::default()
            },
            &object_bytes,
        );
        assert_eq!(report["status"], "unchecked_not_translation_validated");
        assert_eq!(
            results_of(&report)["llvm_ir_object_equivalence"],
            "unchecked"
        );
        let detail = report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["item"] == "llvm_ir_object_equivalence")
            .unwrap()["detail"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(detail.contains("main: unknown"), "{detail}");

        // A record missing a function is unmappable and count-mismatched.
        let mut short = record.clone();
        short.functions.pop();
        let report = object_translation_correspondence_checks(
            &ir_surface,
            &executable_bytes,
            &short,
            IrCorrespondenceInputs::default(),
            &object_bytes,
        );
        assert_eq!(report["status"], "inconsistent_correspondence");
        let results = results_of(&report);
        assert_eq!(results["function_coverage"], "inconsistent");
        assert_eq!(results["function_count"], "inconsistent");

        // An extra record function not in the IR surface is unmappable.
        let mut extra = record.clone();
        extra.functions.push(ObjectFunctionRecord {
            ir_function: "phantom".to_owned(),
            object_symbol: "phantom".to_owned(),
            object_section: ".text".to_owned(),
            object_offset: 0,
            object_size: 1,
            executable_address: Some(0),
        });
        let results = results_of(&object_translation_correspondence_checks(
            &ir_surface,
            &executable_bytes,
            &extra,
            IrCorrespondenceInputs::default(),
            &object_bytes,
        ));
        assert_eq!(results["function_coverage"], "inconsistent");
        assert_eq!(results["function_count"], "inconsistent");

        // A relocation to an unknown symbol is an inconsistent target.
        let mut bogus_target = record.clone();
        bogus_target.relocations[0].target_symbol = "nonexistent_target".to_owned();
        let results = results_of(&object_translation_correspondence_checks(
            &ir_surface,
            &executable_bytes,
            &bogus_target,
            IrCorrespondenceInputs::default(),
            &object_bytes,
        ));
        assert_eq!(results["relocation_targets"], "inconsistent");

        // A relocation outside every function record is out of bounds.
        let mut out_of_bounds = record.clone();
        out_of_bounds.relocations[0].object_offset = u64::MAX - 7;
        let results = results_of(&object_translation_correspondence_checks(
            &ir_surface,
            &executable_bytes,
            &out_of_bounds,
            IrCorrespondenceInputs::default(),
            &object_bytes,
        ));
        assert_eq!(results["relocation_bounds"], "inconsistent");

        // A relocation without a recomputed value stays honestly unchecked.
        let mut unresolved = record.clone();
        unresolved.relocations[0].resolved_value = None;
        let report = object_translation_correspondence_checks(
            &ir_surface,
            &executable_bytes,
            &unresolved,
            IrCorrespondenceInputs::default(),
            &object_bytes,
        );
        assert_eq!(report["status"], "unchecked_not_translation_validated");
        let results = results_of(&report);
        assert_eq!(results["relocation_resolved_values"], "unchecked");

        // Without an IR surface the function checks stay unchecked.
        let report = object_translation_correspondence_checks(
            &tv::IrFunctionSurface::default(),
            &executable_bytes,
            record,
            IrCorrespondenceInputs::default(),
            &object_bytes,
        );
        assert_eq!(report["status"], "unchecked_not_translation_validated");
        let results = results_of(&report);
        assert_eq!(results["function_coverage"], "unchecked");
        assert_eq!(results["function_count"], "unchecked");
    }

    #[test]
    fn object_data_correspondence_reports_equivalent_merged_missing_and_different() {
        let dir = tempfile::tempdir().unwrap();
        let ir_path = dir.path().join("data.ll");
        let object_path = dir.path().join("data.o");
        let executable_path = dir.path().join("data");
        // `@greeting` gets no object symbol: mergeable `unnamed_addr`
        // string constants land in `.rodata.str1.1` anonymously, so
        // only the byte-content fallback verifies it.
        let ir = br#"
@greeting = private unnamed_addr constant [6 x i8] c"hello\00"
@counter = global i32 7
@scratch = global [8 x i8] zeroinitializer

define i32 @main() {
entry:
  ret i32 0
}
"#;
        fs::write(&ir_path, ir).unwrap();
        mumei_emit_llvm::codegen::compile_llvm_ir_to_object_recorded(&ir_path, &object_path)
            .unwrap();
        linker::link_recorded(std::slice::from_ref(&object_path), &executable_path).unwrap();
        let object_bytes = fs::read(&object_path).unwrap();
        let executable_bytes = fs::read(&executable_path).unwrap();
        let ir_surface = tv::ir_function_surface(ir).unwrap();
        let ir_data = tv::ir_data_globals(ir).unwrap();
        let record = &object_link_corresponds(&object_bytes, &executable_bytes)
            .unwrap()
            .unwrap()
            .2;

        let item = |report: &serde_json::Value| -> (String, String) {
            let item = report["checks"]
                .as_array()
                .unwrap()
                .iter()
                .find(|check| check["item"] == "object_data_correspondence")
                .unwrap();
            (
                item["result"].as_str().unwrap().to_owned(),
                item["detail"].as_str().unwrap().to_owned(),
            )
        };

        // Every derivable global verifies: named data/bss symbols by
        // content, the anonymous string constant via the section-byte
        // fallback (`merged`).
        let report = object_translation_correspondence_checks(
            &ir_surface,
            &executable_bytes,
            record,
            IrCorrespondenceInputs {
                data_globals: Some(&ir_data),
                ..Default::default()
            },
            &object_bytes,
        );
        let (result, detail) = item(&report);
        assert_eq!(result, "checked");
        assert!(detail.contains("counter: equivalent"), "{detail}");
        assert!(detail.contains("scratch: equivalent"), "{detail}");
        assert!(detail.contains("greeting: merged"), "{detail}");

        // A global the object never emits stays unverifiable — the
        // item reports `unchecked`, not `inconsistent`.
        let mut with_missing = ir_data.clone();
        with_missing.insert("hidden".to_owned(), Some(vec![1, 2, 3]));
        let report = object_translation_correspondence_checks(
            &ir_surface,
            &executable_bytes,
            record,
            IrCorrespondenceInputs {
                data_globals: Some(&with_missing),
                ..Default::default()
            },
            &object_bytes,
        );
        let (result, detail) = item(&report);
        assert_eq!(result, "unchecked");
        assert!(detail.contains("hidden: missing"), "{detail}");

        // An initializer outside the derivable shapes is unverifiable,
        // never treated as empty data.
        let mut with_underivable = ir_data.clone();
        with_underivable.insert("opaque".to_owned(), None);
        let report = object_translation_correspondence_checks(
            &ir_surface,
            &executable_bytes,
            record,
            IrCorrespondenceInputs {
                data_globals: Some(&with_underivable),
                ..Default::default()
            },
            &object_bytes,
        );
        let (result, detail) = item(&report);
        assert_eq!(result, "unchecked");
        assert!(detail.contains("opaque: underivable"), "{detail}");

        // Integer widths whose byte-store size is not a power of two
        // (`i24` allocates 4 bytes but stores 3) are underivable too —
        // comparing them against the object's padded image would
        // report a false `inconsistent`.
        let padded = tv::ir_data_globals(
            br#"@pair = global [2 x i24] [i24 1, i24 2]
@word = global i24 7
@aligned = global i64 7"#,
        )
        .unwrap();
        assert_eq!(padded["pair"], None);
        assert_eq!(padded["word"], None);
        assert_eq!(padded["aligned"], Some(7u64.to_le_bytes().to_vec()));

        // Object data with no IR globals stays unverifiable when the
        // sections hold anonymous bytes — only a fully empty surface
        // is trivially checked.
        let no_globals: BTreeMap<String, Option<Vec<u8>>> = BTreeMap::new();
        let report = object_translation_correspondence_checks(
            &ir_surface,
            &executable_bytes,
            record,
            IrCorrespondenceInputs {
                data_globals: Some(&no_globals),
                ..Default::default()
            },
            &object_bytes,
        );
        let (result, detail) = item(&report);
        assert_eq!(result, "unchecked");
        assert!(detail.contains("anonymous data bytes"), "{detail}");

        // The same outcome must hold when the object carries *only*
        // anonymous data — no named data symbols at all. A module of a
        // single `unnamed_addr` string constant compiles to exactly
        // that: `.rodata.str1.1` bytes with an empty symbol map.
        let anonymous_ir_path = dir.path().join("anonymous.ll");
        let anonymous_object_path = dir.path().join("anonymous.o");
        let anonymous_ir = br#"@lone = private unnamed_addr constant [4 x i8] c"hey\00""#;
        fs::write(&anonymous_ir_path, anonymous_ir).unwrap();
        mumei_emit_llvm::codegen::compile_llvm_ir_to_object_recorded(
            &anonymous_ir_path,
            &anonymous_object_path,
        )
        .unwrap();
        let anonymous_object = fs::read(&anonymous_object_path).unwrap();
        let anonymous_surface = object_data_surface(&anonymous_object).unwrap();
        assert!(anonymous_surface.symbols.is_empty());
        assert!(!anonymous_surface.section_contents.is_empty());
        let report = object_translation_correspondence_checks(
            &ir_surface,
            &executable_bytes,
            record,
            IrCorrespondenceInputs {
                data_globals: Some(&no_globals),
                ..Default::default()
            },
            &anonymous_object,
        );
        let (result, detail) = item(&report);
        assert_eq!(result, "unchecked");
        assert!(detail.contains("anonymous data bytes"), "{detail}");

        // A named symbol whose bytes disagree is a real inconsistency.
        let mut with_diverged = ir_data.clone();
        with_diverged.insert("counter".to_owned(), Some(vec![1, 2, 3, 4]));
        let report = object_translation_correspondence_checks(
            &ir_surface,
            &executable_bytes,
            record,
            IrCorrespondenceInputs {
                data_globals: Some(&with_diverged),
                ..Default::default()
            },
            &object_bytes,
        );
        let (result, detail) = item(&report);
        assert_eq!(result, "inconsistent");
        assert!(detail.contains("counter: different"), "{detail}");
        assert_eq!(report["status"], "inconsistent_correspondence");
    }

    #[test]
    fn object_symbol_attribute_correspondence_checks_bindings_against_ir_linkage() {
        let dir = tempfile::tempdir().unwrap();
        let ir_path = dir.path().join("attrs.ll");
        let object_path = dir.path().join("attrs.o");
        let executable_path = dir.path().join("attrs");
        // `@greeting` carries `unnamed_addr`: LLVM may merge or drop
        // its symbol, so it is excluded from the binding domain —
        // counted in `unnamed_addr_excluded`, never compared.
        let ir = br#"
@greeting = private unnamed_addr constant [6 x i8] c"hello\00"
@counter = global i32 7
@internal_count = internal global i32 3
@fallback = weak global i32 9

define i32 @main() {
entry:
  ret i32 0
}

define internal i32 @helper() {
entry:
  ret i32 3
}
"#;
        fs::write(&ir_path, ir).unwrap();
        mumei_emit_llvm::codegen::compile_llvm_ir_to_object_recorded(&ir_path, &object_path)
            .unwrap();
        linker::link_recorded(std::slice::from_ref(&object_path), &executable_path).unwrap();
        let object_bytes = fs::read(&object_path).unwrap();
        let executable_bytes = fs::read(&executable_path).unwrap();
        let ir_surface = tv::ir_function_surface(ir).unwrap();
        let record = &object_link_corresponds(&object_bytes, &executable_bytes)
            .unwrap()
            .unwrap()
            .2;
        let ir_attrs = tv::ir_symbol_attributes(ir).unwrap();
        assert_eq!(ir_attrs.unnamed_addr_excluded, 1);
        assert_eq!(ir_attrs.expected["counter"], Some("global"));
        assert_eq!(ir_attrs.expected["internal_count"], Some("local"));
        assert_eq!(ir_attrs.expected["fallback"], Some("weak"));
        assert_eq!(ir_attrs.expected["main"], Some("global"));
        assert_eq!(ir_attrs.expected["helper"], Some("local"));

        let item = |report: &serde_json::Value| -> (String, String) {
            let item = report["checks"]
                .as_array()
                .unwrap()
                .iter()
                .find(|check| check["item"] == "object_symbol_attribute_correspondence")
                .unwrap();
            (
                item["result"].as_str().unwrap().to_owned(),
                item["detail"].as_str().unwrap().to_owned(),
            )
        };

        // Every derivable entity verifies against its object binding.
        let report = object_translation_correspondence_checks(
            &ir_surface,
            &executable_bytes,
            record,
            IrCorrespondenceInputs {
                symbol_attributes: Some(&ir_attrs),
                ..Default::default()
            },
            &object_bytes,
        );
        let (result, detail) = item(&report);
        assert_eq!(result, "checked");
        assert!(detail.contains("counter: equivalent (global)"), "{detail}");
        assert!(
            detail.contains("internal_count: equivalent (local)"),
            "{detail}"
        );
        assert!(detail.contains("fallback: equivalent (weak)"), "{detail}");
        assert!(detail.contains("helper: equivalent (local)"), "{detail}");
        assert!(
            detail.contains("1 unnamed_addr entities excluded"),
            "{detail}"
        );

        // A symbol the object never emits stays unverifiable — the
        // item reports `unchecked`, not `inconsistent`.
        let mut with_missing = ir_attrs.clone();
        with_missing
            .expected
            .insert("ghost".to_owned(), Some("global"));
        let report = object_translation_correspondence_checks(
            &ir_surface,
            &executable_bytes,
            record,
            IrCorrespondenceInputs {
                symbol_attributes: Some(&with_missing),
                ..Default::default()
            },
            &object_bytes,
        );
        let (result, detail) = item(&report);
        assert_eq!(result, "unchecked");
        assert!(detail.contains("ghost: missing"), "{detail}");

        // A binding that disagrees with the IR linkage is a real
        // inconsistency.
        let mut diverged = ir_attrs.clone();
        diverged
            .expected
            .insert("counter".to_owned(), Some("local"));
        let report = object_translation_correspondence_checks(
            &ir_surface,
            &executable_bytes,
            record,
            IrCorrespondenceInputs {
                symbol_attributes: Some(&diverged),
                ..Default::default()
            },
            &object_bytes,
        );
        let (result, detail) = item(&report);
        assert_eq!(result, "inconsistent");
        assert!(detail.contains("counter: different"), "{detail}");
        assert_eq!(report["status"], "inconsistent_correspondence");

        // A linkage with no derivable object binding is unverifiable,
        // never treated as a fixed class.
        let mut underivable = ir_attrs.clone();
        underivable.expected.insert("opaque".to_owned(), None);
        let report = object_translation_correspondence_checks(
            &ir_surface,
            &executable_bytes,
            record,
            IrCorrespondenceInputs {
                symbol_attributes: Some(&underivable),
                ..Default::default()
            },
            &object_bytes,
        );
        let (result, detail) = item(&report);
        assert_eq!(result, "unchecked");
        assert!(detail.contains("opaque: underivable"), "{detail}");

        // `available_externally` definitions emit no object symbol, so
        // their linkage maps to no derivable binding at all.
        let opaque = tv::ir_symbol_attributes(
            br#"@ghost_def = available_externally global i32 0
@real = global i32 1"#,
        )
        .unwrap();
        assert_eq!(opaque.expected["ghost_def"], None);
        assert_eq!(opaque.expected["real"], Some("global"));
    }

    #[test]
    fn object_translation_correspondence_treats_loader_imports_as_assumed_external() {
        let dir = tempfile::tempdir().unwrap();
        let ir_path = dir.path().join("module.ll");
        let object_path = dir.path().join("module.o");
        let executable_path = dir.path().join("module");
        // A `llvm.memcpy` call lowers to a backend-injected `memcpy`
        // relocation the IR never declares. The link keeps it an import
        // (libc), so the check reports it assumed-external rather than
        // an unresolvable target.
        let ir = br#"
declare void @llvm.memcpy.p0.p0.i64(ptr, ptr, i64, i1)

define i64 @copier() {
entry:
  %dst = alloca [4096 x i8]
  %src = alloca [4096 x i8]
  call void @llvm.memcpy.p0.p0.i64(ptr %dst, ptr %src, i64 4096, i1 false)
  %byte = load i8, ptr %dst
  %value = zext i8 %byte to i64
  ret i64 %value
}

define i32 @main() {
entry:
  %value = call i64 @copier()
  %exit = trunc i64 %value to i32
  ret i32 %exit
}
"#;
        fs::write(&ir_path, ir).unwrap();
        mumei_emit_llvm::codegen::compile_llvm_ir_to_object_recorded(&ir_path, &object_path)
            .unwrap();
        linker::link_recorded(std::slice::from_ref(&object_path), &executable_path).unwrap();
        let object_bytes = fs::read(&object_path).unwrap();
        let executable_bytes = fs::read(&executable_path).unwrap();
        let ir_surface = tv::ir_function_surface(ir).unwrap();
        // The symbol and call-edge surfaces pass the CLI gates too:
        // `llvm.memcpy`'s `memcpy` is a backend-injected libc call.
        let object_surface = object_symbol_surface(&object_bytes).unwrap();
        assert!(object_surface.undefined.contains("memcpy"));
        assert!(object_symbols_correspond(&ir_surface, &object_surface));
        assert_eq!(
            object_call_edges_correspond(&ir_surface, &object_surface),
            Some(true)
        );
        let (corresponds, resolved_call_sites, record_) =
            object_link_corresponds(&object_bytes, &executable_bytes)
                .unwrap()
                .unwrap();
        let record = &record_;
        // The `memcpy` relocation is recorded, and its call field is
        // verified to point at the PLT stub the linker emitted for it.
        let memcpy_relocation = record
            .relocations
            .iter()
            .find(|relocation| relocation.target_symbol == "memcpy")
            .unwrap();
        assert!(memcpy_relocation.resolved_value.is_some());
        // The import carries the shared `assumed_*` category marker;
        // internal targets carry none.
        assert_eq!(
            memcpy_relocation.assumption,
            Some("assumed_external_callee")
        );
        assert!(record.relocations.iter().all(|relocation| relocation
            .assumption
            .is_none_or(|a| a == "assumed_external_callee")));
        assert!(corresponds);
        assert!(resolved_call_sites >= 1);

        let report = object_translation_correspondence_checks(
            &ir_surface,
            &executable_bytes,
            record,
            IrCorrespondenceInputs::default(),
            &object_bytes,
        );
        let targets = report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["item"] == "relocation_targets")
            .unwrap();
        assert_eq!(targets["result"], "checked");
        assert!(targets["detail"]
            .as_str()
            .unwrap()
            .contains("assumed-external: [\"memcpy\"]"));
    }

    #[test]
    fn ir_object_equivalence_outcome_separates_unknown_from_skip_reasons() {
        // `solver_unknown` is an attempted-but-undecidable comparison —
        // it maps to `unknown` (fail-closed), not a skip reason; every
        // other `NotLiftable` reason passes through verbatim.
        use machine_tv::IrObjectEquivalence;
        assert_eq!(
            ir_object_equivalence_outcome(IrObjectEquivalence::Equivalent),
            "equivalent"
        );
        assert_eq!(
            ir_object_equivalence_outcome(IrObjectEquivalence::Different),
            "different"
        );
        assert_eq!(
            ir_object_equivalence_outcome(IrObjectEquivalence::NotLiftable("solver_unknown")),
            "unknown"
        );
        for reason in [
            "unsupported_signature",
            "unsupported_ir",
            "unsupported_instruction",
            "lift_failed",
            "no_liftable_path",
        ] {
            assert_eq!(
                ir_object_equivalence_outcome(IrObjectEquivalence::NotLiftable(reason)),
                reason
            );
        }
    }

    #[test]
    fn source_machine_tv_names_link_edge_status_separately() {
        // The gate covers source↔machine equivalence only — its `checked`
        // status must never be read as covering the link edge. With the
        // link check unperformed the record names it unchecked even when
        // every function compared equivalent.
        let mut outcomes = BTreeMap::new();
        outcomes.insert("main".to_owned(), FunctionOutcome::Equivalent);
        let evaluation = MachineTvEvaluation {
            functions: BTreeMap::new(),
            outcomes,
            counts: MachineTvCounts {
                equivalent_count: 1,
                equivalent_callee_saved_count: 0,
                equivalent_callee_saved_preserved_count: 0,
                equivalent_leaf_count: 1,
                equivalent_call_count: 0,
                equivalent_forward_branch_count: 0,
                equivalent_acyclic_branch_count: 0,
                equivalent_conditional_call_count: 0,
                equivalent_bounded_loop_count: 0,
                equivalent_source_loop_count: 0,
                equivalent_refined_signature_count: 0,
                equivalent_f64_leaf_count: 0,
                equivalent_composite_aggregate_count: 0,
                equivalent_f64_composite_aggregate_count: 0,
                equivalent_wide_composite_aggregate_count: 0,
                equivalent_wide_composite_aggregate_convention_count: 0,
                equivalent_wide_scalar_count: 0,
                equivalent_red_zone_frame_memory_count: 0,
                equivalent_allocated_frame_memory_count: 0,
                object_function_count: 1,
            },
            coverage: (vec![], vec!["object_link_correspondence"]),
            input_domain: String::new(),
            scope: "test",
            entry_wrapper_applicable: false,
        };
        let unchecked = source_machine_tv_within_profile_json(
            &evaluation,
            &["object_link_correspondence"],
            false,
        );
        assert_eq!(
            unchecked["status"],
            "checked_source_machine_tv_within_profile"
        );
        assert_eq!(
            unchecked["object_link_correspondence"],
            "unchecked_not_translation_validated"
        );
        let checked = source_machine_tv_within_profile_json(&evaluation, &[], true);
        assert_eq!(
            checked["object_link_correspondence"],
            "checked_object_link_correspondence"
        );
    }
}
