use crate::{cli::NativeBundleCommand, linker};
use mumei_core::{hir, native_bundle::*, proof_cert, verification};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::Path,
};

const MAX_COMPONENT_BYTES: u64 = 32 * 1024 * 1024;

pub(crate) fn run(command: NativeBundleCommand) {
    let result = match command {
        NativeBundleCommand::RecheckSource { bundle, source, expected_build_id } => {
            match recheck(Path::new(&bundle), Path::new(&source), &expected_build_id) {
                Ok(SourceRecheck::RecheckedWithinProfile) => {
                    println!("Pinned snapshot identities consistent; source postcondition freshly rechecked within C03.\nProfile: {RECHECK_PROFILE}\nScope: {RECHECK_SCOPE}\nLLVM/object/linker/runtime/executable semantics: not checked.");
                    Ok(())
                }
                Ok(SourceRecheck::Refuted) => Err(BundleError("source postcondition refuted")),
                Ok(SourceRecheck::Inconclusive) => {
                    eprintln!("Native source recheck inconclusive; no source proof established.");
                    std::process::exit(3);
                }
                Err(error) => Err(error),
            }
        },
        NativeBundleCommand::Build { input, output } => build(Path::new(&input), Path::new(&output)).map(|id| {
            println!("Native bundle built with fixed profile (project config/cache not loaded).\nBuild ID: {id}\nScope: {SCOPE}");
        }),
        NativeBundleCommand::Check { bundle, source, expected_build_id } => check(Path::new(&bundle), Path::new(&source), &expected_build_id).map(|()| {
            println!("Native bundle identity consistent with pinned build ID.\nScope: {SCOPE}");
        }),
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn read_regular(path: &Path, limit: u64) -> Result<Vec<u8>, BundleError> {
    use std::io::Read;
    let metadata = fs::symlink_metadata(path).map_err(|_| BundleError("missing component"))?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(BundleError("unsupported component type or size"));
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|_| BundleError("cannot open component"))?;
    if !file
        .metadata()
        .map_err(|_| BundleError("cannot inspect component"))?
        .is_file()
    {
        return Err(BundleError("component is not a regular file"));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| BundleError("cannot read component"))?;
    if bytes.len() as u64 > limit {
        return Err(BundleError("component too large"));
    }
    Ok(bytes)
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

fn build(input: &Path, output: &Path) -> Result<String, BundleError> {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        return Err(BundleError("unsupported host for first native profile"));
    }
    if fs::symlink_metadata(output).is_ok() {
        return Err(BundleError("output already exists"));
    }
    let source = source_text(input)?;
    let (binding, atom) = prepare_source(&source)?;
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
    check_identity(&bytes, &id, &source, &files)?;
    if source_text(input)? != source {
        return Err(BundleError("source changed during build"));
    }
    write(&stage.path().join(RECORD_FILE), &bytes)?;
    publish(stage.path(), output)?;
    let _ = stage.keep();
    Ok(id)
}

fn check(bundle: &Path, source: &Path, expected_id: &str) -> Result<(), BundleError> {
    let record = read_regular(&bundle.join(RECORD_FILE), 1024 * 1024)?;
    load_pinned_record(&record, expected_id)?;
    check_identity(
        &record,
        expected_id,
        &source_text(source)?,
        &components(bundle)?,
    )
}

fn recheck(bundle: &Path, source: &Path, expected_id: &str) -> Result<SourceRecheck, BundleError> {
    let record = read_regular(&bundle.join(RECORD_FILE), 1024 * 1024)?;
    load_pinned_record(&record, expected_id)?;
    let source = String::from_utf8(read_regular(source, 65_536)?)
        .map_err(|_| BundleError("source is not UTF-8"))?;
    recheck_source(&record, expected_id, &source, &components(bundle)?)
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
