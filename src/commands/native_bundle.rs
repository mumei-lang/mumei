use crate::{cli::NativeBundleCommand, linker};
use mumei_core::{hir, native_bundle::*, parser, proof_cert, resolver, verification};
use mumei_emit_llvm::translation_validation::{self as tv, Options, Outcome};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
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
const EXIT_STATUS_RECHECK_SCOPE: &str =
    "artifact_reproduction_and_os_exit_status_observation_only/no_runtime_abi_or_semantic_proof";
/// The build/recheck single-run cap for observing an executable's exit
/// status: a bounded pure-i64 `main` is expected to return promptly, but
/// the observation must never hang a command.
const EXIT_STATUS_TIMEOUT: Duration = Duration::from_secs(30);

/// Run `executable` once with no arguments and closed stdio, returning
/// its 0..=255 exit code. `None` on spawn failure, timeout or signal
/// termination — never a claim about program semantics, only an OS
/// exit-status observation.
fn observed_exit_status(executable: &Path) -> Option<u32> {
    let mut child = std::process::Command::new(executable)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + EXIT_STATUS_TIMEOUT;
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
                .and_then(|(acceptance, outcome, saved)| write_native_acceptance(outcome, json, acceptance, Some(&saved), &mut std::io::stdout().lock()))
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
                }, None, &mut std::io::stdout().lock()))
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
                            std::process::exit(3);
                        }
                    }
                }
                ArtifactRecheckResult {
                    result: ArtifactRecheck::Rechecked,
                    expected_exit_status: None,
                    ..
                } => {
                    eprintln!("Record pins no expected exit status; inconclusive.");
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
    not_performed: &["producer_authenticity", "historical_execution", "object_linker_artifact_recheck", "optimizer_semantics", "abi_runtime_semantics", "os_exit_status", "grade_promotion", "lean_kernel_checks"],
    text: "Native Evidence accepted within the pinned bundle/source/LLVM profile: identities bound, source postcondition and LLVM snapshot freshly rechecked, saved report consistent.\nAcceptance scope excludes producer authenticity, historical execution, artifact/object/linker recheck, optimizer/ABI/runtime/OS semantics, grade and Lean promotion.\n",
};

const MULTIFILE_EVIDENCE_ACCEPTANCE: NativeAcceptance = NativeAcceptance {
    scope: "native_multifile_bundle_identity_merge_source_llvm_and_saved_evidence_consistency",
    performed: &["bundle_identity_binding", "pinned_member_merge_rederivation", "fresh_same_profile_source_recheck", "llvm_snapshot_translation_recheck", "saved_evidence_consistency"],
    not_performed: &["producer_authenticity", "historical_execution", "object_linker_artifact_recheck", "optimizer_semantics", "abi_runtime_semantics", "os_exit_status", "grade_promotion", "lean_kernel_checks"],
    text: "Native multifile Evidence accepted within the pinned bundle/source/LLVM profile: identities bound, import closure re-merged from pinned members, source postcondition and LLVM snapshot freshly rechecked, saved report consistent.\nAcceptance scope excludes producer authenticity, historical execution, artifact/object/linker recheck, optimizer/ABI/runtime/OS semantics, grade and Lean promotion.\n",
};

const MULTIFILE_ARTIFACT_ACCEPTANCE: NativeAcceptance = NativeAcceptance {
    scope: "native_multifile_bundle_source_llvm_and_toolchain_artifact_reproduction",
    performed: &["bundle_identity_binding", "pinned_member_merge_rederivation", "fresh_same_profile_source_recheck", "llvm_snapshot_translation_recheck", "toolchain_object_executable_reproduction", "canonical_link_argument_vector_reproduction"],
    not_performed: &["producer_authenticity", "historical_execution", "raw_link_argument_vector_reproduction", "optimizer_semantics", "abi_runtime_semantics", "os_exit_status", "grade_promotion", "lean_kernel_checks"],
    text: "Native multifile artifact accepted within the pinned bundle/toolchain profile: identities bound, import closure re-merged from pinned members, source postcondition and LLVM snapshot freshly rechecked, pinned object and executable reproduced byte-identically, canonical link argument vector re-derived.\nAcceptance scope excludes producer authenticity, historical execution, raw link argument-vector reproduction (absolute staging paths are unreproducible), optimizer/ABI/runtime/OS exit-status semantics, grade and Lean promotion.\n",
};

const MULTIFILE_ARTIFACT_ACCEPTANCE_ARGV_UNPINNED: NativeAcceptance = NativeAcceptance {
    scope: "native_multifile_bundle_source_llvm_and_toolchain_artifact_reproduction",
    performed: &["bundle_identity_binding", "pinned_member_merge_rederivation", "fresh_same_profile_source_recheck", "llvm_snapshot_translation_recheck", "toolchain_object_executable_reproduction"],
    not_performed: &["producer_authenticity", "historical_execution", "canonical_link_argument_vector_reproduction", "raw_link_argument_vector_reproduction", "optimizer_semantics", "abi_runtime_semantics", "os_exit_status", "grade_promotion", "lean_kernel_checks"],
    text: "Native multifile artifact accepted within the pinned bundle/toolchain profile: identities bound, import closure re-merged from pinned members, source postcondition and LLVM snapshot freshly rechecked, pinned object and executable reproduced byte-identically.\nAcceptance scope excludes producer authenticity, historical execution, canonical link argument-vector reproduction (the record pins no canonical argv), raw link argument-vector reproduction (absolute staging paths are unreproducible), optimizer/ABI/runtime/OS exit-status semantics, grade and Lean promotion.\n",
};

const ARTIFACT_ACCEPTANCE: NativeAcceptance = NativeAcceptance {
    scope: "native_bundle_source_llvm_and_toolchain_artifact_reproduction",
    performed: &["bundle_identity_binding", "fresh_same_profile_source_recheck", "llvm_snapshot_translation_recheck", "toolchain_object_executable_reproduction", "canonical_link_argument_vector_reproduction"],
    not_performed: &["producer_authenticity", "historical_execution", "raw_link_argument_vector_reproduction", "optimizer_semantics", "abi_runtime_semantics", "os_exit_status", "grade_promotion", "lean_kernel_checks"],
    text: "Native artifact accepted within the pinned bundle/toolchain profile: identities bound, source postcondition and LLVM snapshot freshly rechecked, pinned object and executable reproduced byte-identically, canonical link argument vector re-derived.\nAcceptance scope excludes producer authenticity, historical execution, raw link argument-vector reproduction (absolute staging paths are unreproducible), optimizer/ABI/runtime/OS exit-status semantics, grade and Lean promotion.\n",
};

const ARTIFACT_ACCEPTANCE_ARGV_UNPINNED: NativeAcceptance = NativeAcceptance {
    scope: "native_bundle_source_llvm_and_toolchain_artifact_reproduction",
    performed: &["bundle_identity_binding", "fresh_same_profile_source_recheck", "llvm_snapshot_translation_recheck", "toolchain_object_executable_reproduction"],
    not_performed: &["producer_authenticity", "historical_execution", "canonical_link_argument_vector_reproduction", "raw_link_argument_vector_reproduction", "optimizer_semantics", "abi_runtime_semantics", "os_exit_status", "grade_promotion", "lean_kernel_checks"],
    text: "Native artifact accepted within the pinned bundle/toolchain profile: identities bound, source postcondition and LLVM snapshot freshly rechecked, pinned object and executable reproduced byte-identically.\nAcceptance scope excludes producer authenticity, historical execution, canonical link argument-vector reproduction (the record pins no canonical argv), raw link argument-vector reproduction (absolute staging paths are unreproducible), optimizer/ABI/runtime/OS exit-status semantics, grade and Lean promotion.\n",
};

/// On an accepted outcome emits one anonymous acceptance descriptor — the
/// same evaluation the paired `recheck-*` command runs, reported in the
/// acceptance vocabulary. The descriptor binds no subject, digest or
/// provenance so it cannot be replayed as Evidence; non-accepted outcomes
/// write nothing.
fn write_native_acceptance(
    outcome: Outcome,
    json: bool,
    acceptance: &NativeAcceptance,
    evidence_bytes: Option<&[u8]>,
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
    if atoms > 1 {
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
    let mut link_build = linker::link_recorded(
        &[object],
        &stage.path().join(BundleFile::Executable.file_name()),
    )
    .map_err(|_| BundleError("native link or toolchain observation failed"))?;
    link_build.expected_exit_status =
        observed_exit_status(&stage.path().join(BundleFile::Executable.file_name()));
    let files = components(stage.path())?;
    let record = NativeBundleRecord {
        schema: SCHEMA.into(),
        profile: PROFILE.into(),
        scope: SCOPE.into(),
        system_scope: SYSTEM_SCOPE.into(),
        source: binding,
        object_build,
        link_build,
        settings: BuildSettings::default(),
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
    stage: &Path,
    proof_work: &Path,
) -> Result<EmittedModule, BundleError> {
    let mut env = verification::ModuleEnv::default();
    for atom in atoms {
        env.register_atom(atom);
    }
    // Callees are admitted by contract (extract_pure_i64_module_contracts
    // already ran the profile gates in the prepare step), so sibling
    // calls verify in any declaration order; mark_verified keeps the env
    // consistent for the certificate.
    let mut lowered = Vec::with_capacity(atoms.len());
    for atom in atoms {
        let hir_atom = hir::lower_atom_to_hir(atom);
        verification::verify_with_verification_config(
            &hir_atom,
            proof_work,
            &env,
            &verification::VerificationConfig::default(),
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
    let mut link_build =
        linker::link_recorded(&[object], &stage.join(BundleFile::Executable.file_name()))
            .map_err(|_| BundleError("native link or toolchain observation failed"))?;
    link_build.expected_exit_status =
        observed_exit_status(&stage.join(BundleFile::Executable.file_name()));
    Ok((components(stage)?, object_build, link_build))
}

fn build_module(source: &str, input: &Path, output: &Path) -> Result<String, BundleError> {
    let (binding, atoms) = prepare_module_source(source)?;
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
    let (files, object_build, link_build) = emit_module(&atoms, stage.path(), proof_work.path())?;
    let record = ModuleNativeBundleRecord {
        schema: MODULE_SCHEMA.into(),
        profile: MODULE_PROFILE.into(),
        scope: MODULE_SCOPE.into(),
        system_scope: SYSTEM_SCOPE.into(),
        source: binding,
        object_build,
        link_build,
        settings: BuildSettings::default(),
        files: files
            .iter()
            .map(|(role, bytes)| (*role, ContentIdentity::of(bytes)))
            .collect(),
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
    let (binding, graph, atoms, _merged) = prepare_multifile_source(&root, &members)?;
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
    let (files, object_build, link_build) = emit_module(&atoms, stage.path(), proof_work.path())?;
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
        profile: MULTIFILE_PROFILE.into(),
        scope: MODULE_SCOPE.into(),
        system_scope: SYSTEM_SCOPE.into(),
        source: binding,
        source_files: SourceFilesPin {
            root: root.clone(),
            import_graph_sha256: graph,
        },
        object_build,
        link_build,
        settings: BuildSettings::default(),
        files: files
            .iter()
            .map(|(role, bytes)| (*role, ContentIdentity::of(bytes)))
            .collect(),
        sources: members
            .iter()
            .map(|(name, text)| (name.clone(), ContentIdentity::of(text.as_bytes())))
            .collect(),
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
        let loaded = LoadedBundle {
            object_build: record.object_build.clone(),
            link_build: record.link_build.clone(),
            files,
        };
        let (members, root) = multifile_inputs(bundle, source)?;
        match recheck_multifile_source(&bytes, expected_id, &root, &members, &loaded.files)? {
            SourceRecheck::Refuted => return Err(BundleError("source postcondition refuted")),
            SourceRecheck::Inconclusive => {
                return Ok((LlvmRecheck::Inconclusive, loaded, RecordFamily::Multifile));
            }
            SourceRecheck::RecheckedWithinProfile => (),
        }
        let merged = merge_import_closure(&root, &members)
            .map_err(|_| BundleError("import closure merge failed"))?
            .0;
        let llvm_ir = std::str::from_utf8(&loaded.files[&BundleFile::LlvmIr])
            .map_err(|_| BundleError("native LLVM snapshot is not UTF-8"))?;
        let report = tv::validate_native_module(&merged, llvm_ir, Options::default())
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
        let report = tv::validate_native_module(&source, llvm_ir, Options::default())
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
/// record's pinned exit status and the staging directory (kept alive so
/// `recheck-exit-status` can run the reproduced executable).
struct ArtifactRecheckResult {
    result: ArtifactRecheck,
    family: RecordFamily,
    canonical_argv_compared: bool,
    expected_exit_status: Option<u32>,
    stage: Option<tempfile::TempDir>,
}

impl ArtifactRecheckResult {
    fn different(family: RecordFamily) -> Self {
        Self {
            result: ArtifactRecheck::Different,
            family,
            canonical_argv_compared: false,
            expected_exit_status: None,
            stage: None,
        }
    }

    fn inconclusive(family: RecordFamily) -> Self {
        Self {
            result: ArtifactRecheck::Inconclusive,
            family,
            canonical_argv_compared: false,
            expected_exit_status: None,
            stage: None,
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
    } = loaded;
    let parent = bundle
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let stage = tempfile::Builder::new()
        .prefix(".mumei-artifact-")
        .tempdir_in(parent)
        .map_err(|_| BundleError("cannot stage artifact recheck"))?;
    let ir_path = stage.path().join(BundleFile::LlvmIr.file_name());
    write(&ir_path, &files[&BundleFile::LlvmIr])?;
    let object_path = stage.path().join(BundleFile::Object.file_name());
    let fresh_object =
        mumei_emit_llvm::codegen::compile_llvm_ir_to_object_recorded(&ir_path, &object_path)
            .map_err(|_| BundleError("native object re-emission failed"))?;
    if fresh_object != object_build
        || read_regular(&object_path, MAX_COMPONENT_BYTES)? != files[&BundleFile::Object]
    {
        return Ok(ArtifactRecheckResult::different(family));
    }
    let executable_path = stage.path().join(BundleFile::Executable.file_name());
    let fresh_link = linker::link_recorded(&[object_path], &executable_path)
        .map_err(|_| BundleError("native link or toolchain observation failed"))?;
    if fresh_link.driver != link_build.driver
        || fresh_link.driver_version != link_build.driver_version
        || fresh_link.driver_target != link_build.driver_target
        || fresh_link.linker != link_build.linker
        || fresh_link.linker_version != link_build.linker_version
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
    Ok(ArtifactRecheckResult {
        result: ArtifactRecheck::Rechecked,
        family,
        canonical_argv_compared: link_build.canonical_arguments_sha256.is_some(),
        expected_exit_status: link_build.expected_exit_status,
        stage: Some(stage),
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
