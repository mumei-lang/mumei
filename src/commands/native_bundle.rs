use crate::{cli::NativeBundleCommand, codegen::MUMEI_RUNTIME_C, linker};
use mumei_core::semantic_contract::RefinedAdmission;
use mumei_core::{
    hir, native_bundle::*, parser, proof_cert, resolver, semantic_contract, verification,
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
    argument_shapes
        .iter()
        .any(|shape| matches!(shape, semantic_contract::CallValueShape::Struct { .. }))
        || matches!(
            result_shape,
            semantic_contract::CallValueShape::Struct { .. }
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
                // An all-INTEGER aggregate result past the three
                // register-resident INTEGER leaves comes back through
                // the `sret` convention the lifter now models — it is
                // validated, not an assumed memory-class ABI.
                && !((4..=16).contains(&leaf_count)
                    && model.result_leaf_doubles.len() == leaf_count
                    && model.result_leaf_doubles.iter().all(|is_double| !*is_double))
        })
        || model.call_sites.iter().any(|call_site| {
            call_site.argument_shapes.iter().any(|shape| {
                matches!(
                    shape,
                    semantic_contract::CallValueShape::Struct { leaf_count, .. }
                        if *leaf_count > 2
                )
            }) || matches!(
                call_site.result_shape,
                semantic_contract::CallValueShape::Struct { leaf_count, .. }
                    if leaf_count > 2
            )
        })
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
            call_site.argument_shapes.iter().any(|shape| {
                matches!(
                    shape,
                    semantic_contract::CallValueShape::Struct { has_f64: true, .. }
                )
            }) || matches!(
                call_site.result_shape,
                semantic_contract::CallValueShape::Struct { has_f64: true, .. }
            )
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
                report_object_acceptance_inconclusive(message);
            }
            Ok(ObjectRecheckPipeline::Rechecked(data)) => {
                let evaluation = match evaluate_machine_tv(&data) {
                    Ok(evaluation) => evaluation,
                    Err(message) => {
                        drop(data);
                        report_object_acceptance_inconclusive(message);
                    }
                };
                match machine_tv_acceptance_decision(
                    &evaluation.outcomes,
                    evaluation.entry_wrapper_applicable,
                ) {
                    ObjectMachineTvAcceptanceDecision::Accepted => {
                        write_object_machine_tv_acceptance(
                            &data,
                            &evaluation,
                            &expected_build_id,
                            json,
                        )
                    }
                    ObjectMachineTvAcceptanceDecision::Different => Err(BundleError(
                        "reproduced object machine code differs from the verified source",
                    )),
                    ObjectMachineTvAcceptanceDecision::Inconclusive => {
                        drop(data);
                        report_object_acceptance_inconclusive(
                            "Machine-code TV inconclusive; not every source-atom function was proved equivalent",
                        );
                    }
                }
            }
        },
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
                                        None,
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
                                        None,
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
                                        None,
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
            .contains(&"runtime_observation_semantics")
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
                parser::Item::TypeDef(_) | parser::Item::StructDef(_) | parser::Item::EnumDef(_)
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
/// its gate derived: `[v1, .., v25]` for the wrapping, refined v1,
/// refined v2 and the composite v1–v22 fragments. Each recheck
/// re-derives under exactly that version's admission, never a wider one.
fn record_profile(source_profile: &str, versions: [&'static str; 26]) -> &'static str {
    let [v1, v2, v3, v4, v5, v6, v7, v8, v9, v10, v11, v12, v13, v14, v15, v16, v17, v18, v19, v20, v21, v22, v23, v24, v25, v26] =
        versions;
    match source_profile {
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
    ) {
        semantic_contract::module_leaf_widths(source, admission)
            .map_err(|_| BundleError("source outside bounded module profile"))
    } else {
        Ok(BTreeMap::new())
    }
}

fn build_module(source: &str, input: &Path, output: &Path) -> Result<String, BundleError> {
    // The producer always runs the widest (composite) admission; the
    // record's `profile` names a wider version only when the source
    // actually declares what needs it, so a plain module keeps pinning v1.
    let admission = RefinedAdmission::CompositeV23;
    let (binding, atoms, types, structs, enums) = prepare_module_source(source, admission)?;
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
    let admission = RefinedAdmission::CompositeV23;
    let prepared = prepare_multifile_source(&root, &members, admission)?;
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
    canonical_argv_compared: bool,
    admission: RefinedAdmission,
    ir_surface: tv::IrFunctionSurface,
    object_surface: ObjectSymbolSurface,
    object_link_correspondence: bool,
    llvm_bytes: Vec<u8>,
    object_bytes: Vec<u8>,
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
    section: object::SectionIndex,
    object_address: u64,
    object_size: u64,
    executable_address: u64,
    object_bytes: Vec<u8>,
    executable_bytes: Vec<u8>,
}

struct ObjectLinkRelocation {
    function_index: usize,
    offset: usize,
    target: String,
    addend: i64,
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
    object.defined == ir.defined && object.undefined.is_subset(&ir.declared)
}

fn object_call_edges_correspond(
    ir: &tv::IrFunctionSurface,
    object: &ObjectSymbolSurface,
) -> Option<bool> {
    (ir.call_edges_checkable && object.call_edges_checkable).then(|| ir.calls == object.calls)
}

fn object_link_corresponds(
    object_bytes: &[u8],
    executable_bytes: &[u8],
) -> Result<Option<(bool, usize)>, ()> {
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
            return Ok(Some((false, 0)));
        }

        if functions.iter().any(|function: &ObjectLinkFunction| {
            function.section == section_index
                && function.object_address < object_end
                && symbol.address() < function.object_address + function.object_size
        }) {
            return Ok(None);
        }
        executable_addresses.insert(name.to_owned(), executable_address);
        functions.push(ObjectLinkFunction {
            section: section_index,
            object_address: symbol.address(),
            object_size,
            executable_address,
            object_bytes: object_bytes.to_vec(),
            executable_bytes: executable_function_bytes.to_vec(),
        });
    }

    let mut relocations = Vec::new();
    for section in object_file.sections() {
        if section.kind() != SectionKind::Text {
            continue;
        }
        let section_index = section.index();
        for (offset, relocation) in section.relocations() {
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
            if relocation.size() != 32
                || !matches!(
                    relocation.flags(),
                    RelocationFlags::Elf { r_type }
                        if r_type == object::elf::R_X86_64_PLT32
                            || r_type == object::elf::R_X86_64_PC32
                )
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
                return Ok(None);
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
                function_index,
                offset: relative_offset,
                target: target_name.to_owned(),
                addend: relocation.addend(),
            });
        }
    }

    let mut resolved_call_sites = 0;
    for (function_index, function) in functions.iter().enumerate() {
        let mut function_relocations: Vec<_> = relocations
            .iter()
            .filter(|relocation| relocation.function_index == function_index)
            .collect();
        function_relocations.sort_by_key(|relocation| relocation.offset);
        let mut compared_through = 0;
        for relocation in function_relocations {
            if relocation.offset < compared_through {
                return Ok(None);
            }
            if function.object_bytes[compared_through..relocation.offset]
                != function.executable_bytes[compared_through..relocation.offset]
            {
                return Ok(Some((false, resolved_call_sites)));
            }
            let field_end = relocation.offset + 4;
            let Some(target_address) = executable_addresses.get(&relocation.target).copied() else {
                return Ok(None);
            };
            let Ok(relative_offset) = u64::try_from(relocation.offset) else {
                return Ok(None);
            };
            let Some(place_address) = function.executable_address.checked_add(relative_offset)
            else {
                return Ok(Some((false, resolved_call_sites)));
            };
            let Ok(target_address) = i64::try_from(target_address) else {
                return Ok(Some((false, resolved_call_sites)));
            };
            let Ok(place_address) = i64::try_from(place_address) else {
                return Ok(Some((false, resolved_call_sites)));
            };
            let Some(resolved_value) = target_address
                .checked_add(relocation.addend)
                .and_then(|value| value.checked_sub(place_address))
            else {
                return Ok(Some((false, resolved_call_sites)));
            };
            let Ok(resolved_value) = i32::try_from(resolved_value) else {
                return Ok(Some((false, resolved_call_sites)));
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
                return Ok(Some((false, resolved_call_sites)));
            }
            compared_through = field_end;
            resolved_call_sites += 1;
        }
        if function.object_bytes[compared_through..]
            != function.executable_bytes[compared_through..]
        {
            return Ok(Some((false, resolved_call_sites)));
        }
    }
    Ok(Some((true, resolved_call_sites)))
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
        canonical_argv_compared,
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
    let (object_link_correspondence, link_corresponds, resolved_call_sites) =
        match object_link_corresponds(&object_bytes, &executable) {
            Ok(Some((corresponds, resolved))) => (true, corresponds, resolved),
            Ok(None) | Err(()) if allow_uncheckable_object_link => (false, true, 0),
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
            canonical_argv_compared,
            admission,
            ir_surface,
            object_surface,
            object_link_correspondence,
            llvm_bytes: ir,
            object_bytes,
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
                let (outcome, preservation, frame_memory_use_result) =
                    machine_tv::check_function_with_frame_memory(
                        &bytes,
                        address,
                        model,
                        &call_targets,
                        &context,
                    );
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
        let descriptor = serde_json::json!({
            "schema_version": "mumei.machine-code-tv/v1",
            "scope": evaluation.scope,
            "build_id": expected_build_id,
            "register_convention_assumed": machine_tv_register_convention_assumed_with_frame_memory(
                evaluation.counts.equivalent_f64_leaf_count,
                evaluation.counts.equivalent_composite_aggregate_count,
                evaluation.counts.equivalent_f64_composite_aggregate_count,
                evaluation.counts.equivalent_wide_composite_aggregate_count,
                evaluation.counts.equivalent_wide_scalar_count,
                evaluation.counts.equivalent_red_zone_frame_memory_count,
                evaluation.counts.equivalent_allocated_frame_memory_count,
            ),
            "input_domain": evaluation.input_domain,
            "functions": evaluation.functions,
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
        Ok(())
    }
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
        "native_bundle_object_machine_code_translation_correspondence"
    } else {
        "native_bundle_object_machine_code_translation_correspondence_without_object_link"
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
    checks_performed.extend(coverage.0.iter().copied());
    checks_performed.push("object_source_machine_code_correspondence");

    let mut checks_not_performed = vec!["producer_authenticity", "historical_execution"];
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

fn report_object_acceptance_inconclusive(message: &str) -> ! {
    eprintln!("{}; object not accepted.", message.trim_end_matches('.'));
    std::process::exit(3);
}

fn write_object_machine_tv_acceptance(
    data: &ObjectRecheckData,
    evaluation: &MachineTvEvaluation,
    expected_build_id: &str,
    json: bool,
) -> Result<(), BundleError> {
    let scope = object_machine_tv_acceptance_scope(data.object_link_correspondence);
    if json {
        use sha2::{Digest, Sha256};
        let (checks_performed, checks_not_performed) = object_machine_tv_acceptance_checks(
            data.family,
            data.canonical_argv_compared,
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
            "llvm_sha256": format!("{:x}", Sha256::digest(&data.llvm_bytes)),
            "object_sha256": format!("{:x}", Sha256::digest(&data.object_bytes)),
            "register_convention_assumed": machine_tv_register_convention_assumed_with_frame_memory(
                evaluation.counts.equivalent_f64_leaf_count,
                evaluation.counts.equivalent_composite_aggregate_count,
                evaluation.counts.equivalent_f64_composite_aggregate_count,
                evaluation.counts.equivalent_wide_composite_aggregate_count,
                evaluation.counts.equivalent_wide_scalar_count,
                evaluation.counts.equivalent_red_zone_frame_memory_count,
                evaluation.counts.equivalent_allocated_frame_memory_count,
            ),
            "input_domain": &evaluation.input_domain,
            "functions": &evaluation.functions,
        });
        let mut bytes = serde_json::to_vec_pretty(&descriptor)
            .map_err(|_| BundleError("cannot encode native object acceptance descriptor"))?;
        bytes.push(b'\n');
        std::io::stdout()
            .lock()
            .write_all(&bytes)
            .map_err(|_| BundleError("cannot write native object acceptance descriptor"))
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
            "Acceptance scope excludes producer authenticity, historical execution, {wrapper_exclusion}register-convention validation, optimizer/ABI/runtime/OS observations, grade promotion, and Lean checks; scope: {scope}."
        );
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
            "native_bundle_object_machine_code_translation_correspondence_without_object_link"
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
            arg_leaf_doubles: vec![false, false],
            argument_shapes: vec![semantic_contract::CallValueShape::Struct {
                leaf_count: 2,
                has_enum: false,
                has_f64: false,
            }],
            has_composite_arguments_or_result: true,
            result: z3::ast::BV::from_u64(&context, 0, 64),
            result_is_double: false,
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
        assert_eq!(
            object_link_corresponds(&object_bytes, &executable_bytes),
            Ok(Some((true, 2)))
        );

        let helper_offset = executable_symbol_file_offset(&executable_bytes, "helper", 0);
        let mut altered_helper = executable_bytes.clone();
        altered_helper[helper_offset] ^= 1;
        assert!(matches!(
            object_link_corresponds(&object_bytes, &altered_helper),
            Ok(Some((false, _)))
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
            Ok(Some((false, _)))
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
    fn observation_envelope_and_host_are_only_in_runtime_observation_acceptance() {
        let runtime_acceptances = [
            &MULTIFILE_RUNTIME_OBSERVATION_ACCEPTANCE,
            &MULTIFILE_RUNTIME_OBSERVATION_ACCEPTANCE_ARGV_UNPINNED,
            &RUNTIME_OBSERVATION_ACCEPTANCE,
            &RUNTIME_OBSERVATION_ACCEPTANCE_ARGV_UNPINNED,
        ];
        for acceptance in runtime_acceptances {
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
            &STDOUT_ACCEPTANCE,
            &EXIT_STATUS_ACCEPTANCE,
            &ARTIFACT_ACCEPTANCE,
        ] {
            let mut json_output = Vec::new();
            write_native_acceptance(
                Outcome::EquivalentWithinProfile,
                true,
                acceptance,
                None,
                None,
                &mut json_output,
            )
            .unwrap();
            let descriptor: serde_json::Value = serde_json::from_slice(&json_output).unwrap();
            assert!(descriptor.get("invocation").is_none());
            assert!(descriptor.get("host").is_none());

            let mut text_output = Vec::new();
            write_native_acceptance(
                Outcome::EquivalentWithinProfile,
                false,
                acceptance,
                None,
                None,
                &mut text_output,
            )
            .unwrap();
            assert_eq!(text_output, acceptance.text.as_bytes());
        }
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
}
