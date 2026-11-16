use crate::{cli::NativeBundleCommand, linker};
use mumei_core::{hir, native_bundle::*, parser, proof_cert, verification};
use mumei_emit_llvm::translation_validation::{self as tv, Options, Outcome};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::Path,
};

mod evidence;

enum LlvmRecheck {
    Rechecked(Box<evidence::NativeConsumerEvidence>),
    /// Module bundles carry the per-atom consumer evidence format; the
    /// payload is the translation profile plus that report.
    ModuleRechecked(&'static str, Box<evidence::NativeModuleConsumerEvidence>),
    Different,
    Inconclusive,
}

const MAX_COMPONENT_BYTES: u64 = 32 * 1024 * 1024;
/// Bounded like the record, not the single-atom evidence size: a module
/// report carries `1 + 2·atoms` records.
const MAX_EVIDENCE_BYTES: u64 = 1024 * 1024;
const LLVM_RECHECK_SCOPE: &str =
    "source_postcondition_identity_and_llvm_snapshot_only/no_authenticity_or_binary_semantic_proof";
const ARTIFACT_RECHECK_SCOPE: &str =
    "source_postcondition_llvm_snapshot_and_toolchain_identity_only/no_execution_or_runtime_semantics";

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
            match recheck_artifact(Path::new(&bundle), Path::new(&source), &expected_build_id) {
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
        LlvmRecheck::Different => Ok(Outcome::Different),
        LlvmRecheck::Inconclusive => Ok(Outcome::Inconclusive),
    }
}

/// Read only the record's `schema` field — the dispatch key between the
/// single-atom and module record families. Unknown schemas error here so
/// the family loaders never see a cross-parsed record.
fn is_module_record(record: &[u8]) -> Result<bool, BundleError> {
    match serde_json::from_slice::<serde_json::Value>(record)
        .map_err(|_| BundleError("invalid record"))?
        .get("schema")
        .and_then(|schema| schema.as_str())
    {
        Some(MODULE_SCHEMA) => Ok(true),
        Some(SCHEMA) => Ok(false),
        _ => Err(BundleError("unsupported schema, scope or settings")),
    }
}

fn read_regular(path: &Path, limit: u64) -> Result<Vec<u8>, BundleError> {
    super::input::read_regular(path, limit).map_err(BundleError)
}

fn source_text(path: &Path) -> Result<String, BundleError> {
    String::from_utf8(read_regular(path, MAX_COMPONENT_BYTES)?)
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
    let atoms = parser::parse_module_checked(&source)
        .map_err(|_| BundleError("invalid source"))?
        .into_iter()
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
    let link_build = linker::link_recorded(
        &[object],
        &stage.path().join(BundleFile::Executable.file_name()),
    )
    .map_err(|_| BundleError("native link or toolchain observation failed"))?;
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
    let mut env = verification::ModuleEnv::default();
    for atom in &atoms {
        env.register_atom(atom);
    }
    // Callees are admitted by contract (extract_pure_i64_module_contracts
    // already ran the profile gates in prepare_module_source), so sibling
    // calls verify in any declaration order; mark_verified keeps the env
    // consistent for the certificate.
    let mut lowered = Vec::with_capacity(atoms.len());
    for atom in &atoms {
        let hir_atom = hir::lower_atom_to_hir(atom);
        verification::verify_with_verification_config(
            &hir_atom,
            proof_work.path(),
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
        &stage.path().join(BundleFile::Certificate.file_name()),
        &serde_json::to_vec_pretty(&cert)
            .map_err(|_| BundleError("certificate encoding failed"))?,
    )?;
    let ir = stage.path().join(BundleFile::LlvmIr.file_name());
    let object = stage.path().join(BundleFile::Object.file_name());
    mumei_emit_llvm::binary::compile_atoms_to_binary_ll(&lowered, &env, &[], &ir)
        .map_err(|_| BundleError("native IR emission failed"))?;
    let object_build = mumei_emit_llvm::codegen::compile_llvm_ir_to_object_recorded(&ir, &object)
        .map_err(|_| BundleError("native object emission failed"))?;
    let link_build = linker::link_recorded(
        &[object],
        &stage.path().join(BundleFile::Executable.file_name()),
    )
    .map_err(|_| BundleError("native link or toolchain observation failed"))?;
    let files = components(stage.path())?;
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

fn check(bundle: &Path, source: &Path, expected_id: &str) -> Result<&'static str, BundleError> {
    let record = read_regular(&bundle.join(RECORD_FILE), 1024 * 1024)?;
    if is_module_record(&record)? {
        load_pinned_module_record(&record, expected_id)?;
        check_module_identity(
            &record,
            expected_id,
            &source_text(source)?,
            &components(bundle)?,
        )?;
        return Ok(MODULE_SCOPE);
    }
    load_pinned_record(&record, expected_id)?;
    check_identity(
        &record,
        expected_id,
        &source_text(source)?,
        &components(bundle)?,
    )?;
    Ok(SCOPE)
}

fn recheck(
    bundle: &Path,
    source: &Path,
    expected_id: &str,
) -> Result<(SourceRecheck, &'static str), BundleError> {
    let record = read_regular(&bundle.join(RECORD_FILE), 1024 * 1024)?;
    let source = String::from_utf8(read_regular(source, 65_536)?)
        .map_err(|_| BundleError("source is not UTF-8"))?;
    if is_module_record(&record)? {
        return recheck_module_source(&record, expected_id, &source, &components(bundle)?)
            .map(|result| (result, MODULE_RECHECK_PROFILE));
    }
    load_pinned_record(&record, expected_id)?;
    recheck_source(&record, expected_id, &source, &components(bundle)?)
        .map(|result| (result, RECHECK_PROFILE))
}

fn recheck_llvm(
    bundle: &Path,
    source: &Path,
    expected_id: &str,
) -> Result<LlvmRecheck, BundleError> {
    recheck_llvm_loaded(bundle, source, expected_id).map(|(result, _)| result)
}

/// `recheck-llvm` plus the pinned bundle snapshot its checks already
/// verified — the record's toolchain observations and the once-read
/// component bytes — so downstream stages compare against the same
/// snapshot instead of re-reading files that could change in between.
fn recheck_llvm_loaded(
    bundle: &Path,
    source: &Path,
    expected_id: &str,
) -> Result<(LlvmRecheck, LoadedBundle), BundleError> {
    let bytes = read_regular(&bundle.join(RECORD_FILE), 1024 * 1024)?;
    let source = String::from_utf8(read_regular(source, 65_536)?)
        .map_err(|_| BundleError("source is not UTF-8"))?;
    let files = components(bundle)?;
    if is_module_record(&bytes)? {
        let record = load_pinned_module_record(&bytes, expected_id)?;
        let loaded = LoadedBundle {
            object_build: record.object_build.clone(),
            link_build: record.link_build.clone(),
            files,
        };
        match recheck_module_source(&bytes, expected_id, &source, &loaded.files)? {
            SourceRecheck::Refuted => return Err(BundleError("source postcondition refuted")),
            SourceRecheck::Inconclusive => {
                return Ok((LlvmRecheck::Inconclusive, loaded));
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
                    )?),
                ),
                Outcome::Different | Outcome::UndefinedBehavior => LlvmRecheck::Different,
                Outcome::Inconclusive => LlvmRecheck::Inconclusive,
            },
            loaded,
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
            return Ok((LlvmRecheck::Inconclusive, loaded));
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
    ))
}

fn recheck_evidence(
    bundle: &Path,
    source: &Path,
    evidence_path: &Path,
    expected_id: &str,
) -> Result<Outcome, BundleError> {
    if !is_digest(expected_id) {
        return Err(BundleError("invalid expected build ID"));
    }
    let saved = evidence::decode(&read_regular(evidence_path, MAX_EVIDENCE_BYTES)?)?;
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
        (LlvmRecheck::Rechecked(_), evidence::ConsumerEvidence::Single(_))
        | (LlvmRecheck::ModuleRechecked(..), evidence::ConsumerEvidence::Module(_)) => Err(
            BundleError("saved native evidence differs from fresh consumer report"),
        ),
        (LlvmRecheck::Rechecked(_) | LlvmRecheck::ModuleRechecked(..), _) => Err(BundleError(
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
/// it is a build observation no recheck can reproduce. The executable's
/// byte identity is what binds the link. Executing either artifact,
/// checking OS exit status and ABI/runtime semantics stay out of scope.
fn recheck_artifact(
    bundle: &Path,
    source: &Path,
    expected_id: &str,
) -> Result<ArtifactRecheck, BundleError> {
    // Everything the artifact stage compares comes from the same
    // once-read snapshot `recheck-llvm` verified — re-reading the record
    // or any component here would let a mid-command swap substitute a
    // self-consistent foreign IR/object/executable set.
    let (result, loaded) = recheck_llvm_loaded(bundle, source, expected_id)?;
    match result {
        LlvmRecheck::Rechecked(_) | LlvmRecheck::ModuleRechecked(..) => (),
        LlvmRecheck::Different => return Ok(ArtifactRecheck::Different),
        LlvmRecheck::Inconclusive => return Ok(ArtifactRecheck::Inconclusive),
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
        return Ok(ArtifactRecheck::Different);
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
        return Ok(ArtifactRecheck::Different);
    }
    Ok(ArtifactRecheck::Rechecked)
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
