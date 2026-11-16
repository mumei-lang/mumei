//! Identity binding for a deliberately restricted native build. Not artifact proof.

use crate::{parser, proof_cert, semantic_contract, verification};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const SCHEMA: &str = "mumei.native-bundle/v1";
pub const PROFILE: &str = "mumei.native-pure-i64-main-linux-x86_64/v1";
pub const SOURCE_LABEL: &str = "native-source.mm";
pub const RECORD_FILE: &str = "native-bundle.json";
pub const SCOPE: &str = "identity_consistency_only/no_authenticity_or_artifact_semantic_proof";
pub const SYSTEM_SCOPE: &str = "system_crt_libraries_environment_and_tool_execution_not_attested";
pub const RECHECK_SCOPE: &str =
    "source_postcondition_and_identity_only/no_translation_authenticity_or_binary_semantic_proof";
pub const RECHECK_PROFILE: &str = "mumei.native-source-recheck/v1";
pub const RECHECK_RESOURCE_LIMIT: u32 = 1_000_000;

/// Multi-atom module bundle profile: one zero-argument `main` plus sibling
/// atoms admitted under `mumei.pure-i64-wrapping-scalar/v1` as call targets.
/// The fragment inherits wrapping i64 add/sub/mul — a semantic widening
/// beyond strict C03, pinned in the record's `profile`.
pub const MODULE_SCHEMA: &str = "mumei.native-module-bundle/v1";
pub const MODULE_PROFILE: &str = "mumei.native-pure-i64-module-linux-x86_64/v1";
/// v2 profile: the module gate additionally admits refined scalar type
/// declarations (`type Port = i64 where ...`) and signature members
/// typed by them. The fragment's atoms and their verification model
/// are unchanged — the record's `profile` names which admission ran so
/// a consumer re-derives the binding under exactly the producer's gate.
pub const MODULE_PROFILE_V2: &str = "mumei.native-pure-i64-module-linux-x86_64/v2";
pub const MODULE_SCOPE: &str =
    "identity_consistency_only/no_authenticity_or_artifact_semantic_proof";
pub const MODULE_RECHECK_PROFILE: &str = "mumei.native-module-source-recheck/v1";

/// The admission mode a pinned module/multifile record demands: v1
/// records re-derive under the literal-scalar gate, v2 records under
/// the refined-scalar gate. Anything else is not this schema's record.
fn module_admit_refined(profile: &str) -> Result<bool, BundleError> {
    match profile {
        MODULE_PROFILE => Ok(false),
        MODULE_PROFILE_V2 => Ok(true),
        _ => Err(BundleError("unsupported schema, scope or settings")),
    }
}

/// Multi-file module bundle profile: the module profile's import-closure
/// extension. The producer merges a root `.mm` member and the files its
/// single-level same-directory `import` declarations name into one
/// canonical text — the language's own merged-namespace model makes the
/// merge semantics-preserving for this fragment (a qualified `h.f(` call
/// collects as the bare callee `f`, and the complete callee map is
/// declaration-order-insensitive). The merged text is what gets compiled
/// and verified; the original members are pinned under `sources/` and
/// the file-level import graph in `source_files`.
pub const MULTIFILE_SCHEMA: &str = "mumei.native-multifile-bundle/v1";
pub const MULTIFILE_PROFILE: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v1";
/// Multifile v2: the same refined-scalar widening as `MODULE_PROFILE_V2`
/// over the import-closure merge.
pub const MULTIFILE_PROFILE_V2: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v2";
pub const MULTIFILE_RECHECK_PROFILE: &str = "mumei.native-multifile-source-recheck/v1";

fn multifile_admit_refined(profile: &str) -> Result<bool, BundleError> {
    match profile {
        MULTIFILE_PROFILE => Ok(false),
        MULTIFILE_PROFILE_V2 => Ok(true),
        _ => Err(BundleError("unsupported schema, scope or settings")),
    }
}
/// Bundle subdirectory holding the pinned original member texts.
pub const SOURCES_DIR: &str = "sources";
pub const MAX_SOURCE_FILES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceRecheck {
    RecheckedWithinProfile,
    Refuted,
    Inconclusive,
}

/// Recheck an exact source snapshot under C03 after the existing identity gates.
/// No project configuration, proof cache, orchestration environment or tools are read.
pub fn recheck_source(
    record_bytes: &[u8],
    expected_id: &str,
    source: &str,
    files: &BTreeMap<BundleFile, Vec<u8>>,
) -> Result<SourceRecheck, BundleError> {
    use z3::{Config, Context, Solver};
    load_pinned_record(record_bytes, expected_id)?;
    let mut config = Config::new();
    config.set_timeout_msec(BuildSettings::default().solver_timeout_ms);
    config.set_param_value("rlimit", &RECHECK_RESOURCE_LIMIT.to_string());
    let context = Context::new(&config);
    let obligation = semantic_contract::encode_pure_i64_obligation(source, &context)
        .map_err(|_| BundleError("source outside bounded C03 recheck profile"))?;
    // This also enforces zero-argument main and a literal true precondition.
    check_identity(record_bytes, expected_id, source, files)?;
    let solver = Solver::new(&context);
    solver.assert(&obligation);
    Ok(source_recheck_result(solver.check()))
}

fn source_recheck_result(result: z3::SatResult) -> SourceRecheck {
    match result {
        z3::SatResult::Unsat => SourceRecheck::RecheckedWithinProfile,
        z3::SatResult::Sat => SourceRecheck::Refuted,
        z3::SatResult::Unknown => SourceRecheck::Inconclusive,
    }
}

#[derive(Debug, thiserror::Error)]
#[error("native bundle rejected: {0}")]
pub struct BundleError(pub &'static str);

pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleFile {
    LlvmIr,
    Object,
    Certificate,
    Executable,
}

impl BundleFile {
    pub const ALL: [Self; 4] = [
        Self::LlvmIr,
        Self::Object,
        Self::Certificate,
        Self::Executable,
    ];
    pub fn file_name(self) -> &'static str {
        match self {
            Self::LlvmIr => "program.ll",
            Self::Object => "program.o",
            Self::Certificate => "proof-cert.json",
            Self::Executable => "program",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentIdentity {
    pub sha256: String,
    pub bytes: u64,
}

impl ContentIdentity {
    pub fn of(bytes: &[u8]) -> Self {
        Self {
            sha256: sha256(bytes),
            bytes: bytes.len() as u64,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceBinding {
    pub source_sha256: String,
    pub source_profile: String,
    pub specification_sha256: String,
    pub implementation_sha256: String,
    pub atom_content_hash: String,
    pub hir_encoding: String,
    pub hir_sha256: String,
    pub mir_encoding: String,
    pub mir_sha256: String,
    pub translator_ir_sha256: String,
    pub translator_version: String,
    pub bridge_lemma_hash: String,
    pub compiler_version: String,
    pub source_dependencies_sha256: String,
}

pub fn prepare_source(source: &str) -> Result<(SourceBinding, parser::Atom), BundleError> {
    let contract = semantic_contract::extract_pure_i64_contract(source, SOURCE_LABEL, None, false)
        .map_err(|_| BundleError("unsupported or invalid source"))?;
    let mut items =
        parser::parse_module_checked(source).map_err(|_| BundleError("invalid source"))?;
    let Some(parser::Item::Atom(mut atom)) = items.pop() else {
        return Err(BundleError("expected main atom"));
    };
    if atom.name != "main" || !atom.params.is_empty() || atom.requires.trim() != "true" {
        return Err(BundleError(
            "profile requires zero-argument main with literal true precondition",
        ));
    }
    atom.span.file = SOURCE_LABEL.to_owned();
    Ok((
        SourceBinding {
            source_sha256: contract.source_sha256,
            source_profile: contract.profile.to_owned(),
            specification_sha256: contract.specification_sha256,
            implementation_sha256: contract.implementation_sha256,
            atom_content_hash: contract.atom_content_hash,
            hir_encoding: contract.hir.encoding.to_owned(),
            hir_sha256: contract.hir.sha256,
            mir_encoding: contract.mir.encoding.to_owned(),
            mir_sha256: contract.mir.sha256,
            translator_ir_sha256: sha256(
                &serde_json::to_vec(&contract.translator_ir)
                    .map_err(|_| BundleError("IR encoding failed"))?,
            ),
            translator_version: contract.translator_version.to_owned(),
            bridge_lemma_hash: contract.bridge_lemma_hash.to_owned(),
            compiler_version: contract.compiler_version.to_owned(),
            source_dependencies_sha256: source_dependencies_sha256(std::slice::from_ref(&atom))?,
        },
        atom,
    ))
}

/// Pin the intra-source dependency map the same way the Claim Book records
/// it: every declared atom maps to its sorted direct callees via
/// `resolver::collect_callees_from_atom`, and `BTreeMap` keeps the JSON
/// encoding canonical. This is a call-graph pin, not a file-level import
/// closure — the restricted profiles admit no `import` items.
fn source_dependencies_sha256(atoms: &[parser::Atom]) -> Result<String, BundleError> {
    let dependencies: BTreeMap<String, Vec<String>> = atoms
        .iter()
        .map(|atom| {
            let mut callees: Vec<String> = crate::resolver::collect_callees_from_atom(atom)
                .into_iter()
                .collect();
            callees.sort();
            (atom.name.clone(), callees)
        })
        .collect();
    Ok(sha256(
        &serde_json::to_vec(&dependencies)
            .map_err(|_| BundleError("dependency encoding failed"))?,
    ))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectBuild {
    pub target_triple: String,
    pub data_layout: String,
    pub llvm_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkBuild {
    pub driver: String,
    pub driver_version: String,
    pub driver_target: String,
    pub linker: String,
    pub linker_version: String,
    pub arguments_sha256: String,
    /// sha256 of the argv with every path-dependent argument replaced by
    /// a stable placeholder (`{output}`, `{input:i}`, `{toolchain}`) —
    /// reproducible by a recheck on fresh staging paths, unlike the raw
    /// vector. Optional so records pinned before the field existed still
    /// deserialize; a recheck skips the comparison when it is absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_arguments_sha256: Option<String>,
    /// OS exit status observed by running the produced executable once at
    /// build time (no arguments, closed stdio). Optional so records pinned
    /// before the field existed still deserialize; a recheck that executes
    /// skips the comparison when it is absent — and it is absent whenever
    /// the build could not observe an exit code (spawn failure, timeout or
    /// signal termination rather than a 0..=255 code).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_exit_status: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildSettings {
    pub llvm_optimization: String,
    pub cpu: String,
    pub features: String,
    pub relocation: String,
    pub code_model: String,
    pub link_optimization: String,
    pub link_libraries: Vec<String>,
    pub runtime: String,
    pub entry_abi: String,
    pub project_config: String,
    pub proof_cache: String,
    pub solver_timeout_ms: u64,
}

impl Default for BuildSettings {
    fn default() -> Self {
        Self {
            llvm_optimization: "default".into(),
            cpu: "generic".into(),
            features: String::new(),
            relocation: "pic".into(),
            code_model: "default".into(),
            link_optimization: "O2".into(),
            link_libraries: ["m", "pthread", "dl", "z3"].map(str::to_owned).to_vec(),
            runtime: "system-crt-only/no-mumei-runtime".into(),
            entry_abi: "C-main-i32-from-i64".into(),
            project_config: "not-loaded/fixed-profile".into(),
            proof_cache: "disabled/fresh-z3".into(),
            solver_timeout_ms: verification::VerificationConfig::default().timeout_ms,
        }
    }
}

/// Per-atom contract identities inside a module bundle record — the same
/// fields `SourceBinding` carries for one atom, keyed by atom name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleAtomBinding {
    pub atom_content_hash: String,
    pub specification_sha256: String,
    pub implementation_sha256: String,
    pub hir_sha256: String,
    pub mir_sha256: String,
    pub translator_ir_sha256: String,
}

/// The module-level identity half of a `SourceBinding` — every
/// `ModuleAtomBinding` shares the whole-module `source_sha256`, so atom
/// sets cannot be shuffled into a different source snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleSourceBinding {
    pub source_sha256: String,
    pub source_profile: String,
    pub atoms: BTreeMap<String, ModuleAtomBinding>,
    pub hir_encoding: String,
    pub mir_encoding: String,
    pub translator_version: String,
    pub bridge_lemma_hash: String,
    pub compiler_version: String,
    pub source_dependencies_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleNativeBundleRecord {
    pub schema: String,
    pub profile: String,
    pub scope: String,
    pub system_scope: String,
    pub source: ModuleSourceBinding,
    pub object_build: ObjectBuild,
    pub link_build: LinkBuild,
    pub settings: BuildSettings,
    pub files: BTreeMap<BundleFile, ContentIdentity>,
}

/// The file-level pin half of a multifile record: which member ordered
/// the merge as root, and the pinned import graph over member names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceFilesPin {
    /// Member name of the root file — the file whose `import`
    /// declarations ordered the canonical merge.
    pub root: String,
    /// SHA-256 of the canonical JSON `{member: [direct imports, sorted]}`
    /// — every member appears; imported members map to an empty list
    /// because this slice admits no nested imports.
    pub import_graph_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MultifileNativeBundleRecord {
    pub schema: String,
    pub profile: String,
    pub scope: String,
    pub system_scope: String,
    /// Binding over the canonical merged text — the same shape and
    /// conventions as the module binding, with
    /// `source_dependencies_sha256` pinning the merged atom call graph.
    pub source: ModuleSourceBinding,
    pub source_files: SourceFilesPin,
    pub object_build: ObjectBuild,
    pub link_build: LinkBuild,
    pub settings: BuildSettings,
    pub files: BTreeMap<BundleFile, ContentIdentity>,
    /// Original member texts by member name (`app.mm`, `helpers.mm`),
    /// each pinned to the bytes the merge consumed — a consumer re-derives
    /// the merged text from these pins instead of trusting a filesystem.
    pub sources: BTreeMap<String, ContentIdentity>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeBundleRecord {
    pub schema: String,
    pub profile: String,
    pub scope: String,
    pub system_scope: String,
    pub source: SourceBinding,
    pub object_build: ObjectBuild,
    pub link_build: LinkBuild,
    pub settings: BuildSettings,
    pub files: BTreeMap<BundleFile, ContentIdentity>,
}

pub fn load_pinned_record(
    bytes: &[u8],
    expected_id: &str,
) -> Result<NativeBundleRecord, BundleError> {
    if !is_digest(expected_id) || sha256(bytes) != expected_id {
        return Err(BundleError("build identity mismatch"));
    }
    let record: NativeBundleRecord =
        serde_json::from_slice(bytes).map_err(|_| BundleError("invalid record"))?;
    if record.schema != SCHEMA
        || record.profile != PROFILE
        || record.scope != SCOPE
        || record.system_scope != SYSTEM_SCOPE
        || record.settings != BuildSettings::default()
        || record.files.len() != BundleFile::ALL.len()
        || BundleFile::ALL
            .iter()
            .any(|role| !record.files.contains_key(role))
    {
        return Err(BundleError("unsupported schema, scope or settings"));
    }
    if !record.object_build.target_triple.starts_with("x86_64-")
        || !record.object_build.target_triple.contains("linux")
        || record.object_build.data_layout.is_empty()
        || !record.object_build.llvm_version.starts_with("17.")
        || !record.link_build.driver_target.starts_with("x86_64-")
        || !record.link_build.driver_target.contains("linux")
        || !["cc", "gcc", "clang", "clang-17"].contains(&record.link_build.driver.as_str())
        || !["gnu-ld", "lld"].contains(&record.link_build.linker.as_str())
        || !is_digest(&record.link_build.arguments_sha256)
        || record
            .link_build
            .canonical_arguments_sha256
            .as_deref()
            .is_some_and(|pin| !is_digest(pin))
        || [
            &record.object_build.llvm_version,
            &record.link_build.driver_version,
            &record.link_build.linker_version,
        ]
        .iter()
        .any(|value| !numeric_version(value))
    {
        return Err(BundleError("unsupported toolchain observation"));
    }
    Ok(record)
}

fn numeric_version(value: &str) -> bool {
    (2..=4).contains(&value.split('.').count())
        && value
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

/// Compare snapshots to an externally supplied digest, not a proof or authenticity verdict.
pub fn check_identity(
    record_bytes: &[u8],
    expected_id: &str,
    source: &str,
    files: &BTreeMap<BundleFile, Vec<u8>>,
) -> Result<(), BundleError> {
    let record = load_pinned_record(record_bytes, expected_id)?;
    let (binding, atom) = prepare_source(source)?;
    if record.source != binding {
        return Err(BundleError("source or compiler identity mismatch"));
    }
    if files.len() != BundleFile::ALL.len()
        || BundleFile::ALL.iter().any(|role| {
            files
                .get(role)
                .is_none_or(|data| record.files.get(role) != Some(&ContentIdentity::of(data)))
        })
    {
        return Err(BundleError("missing or replaced bundle component"));
    }
    let cert: proof_cert::ProofCertificate =
        serde_json::from_slice(&files[&BundleFile::Certificate])
            .map_err(|_| BundleError("invalid certificate"))?;
    if !matches!(
        proof_cert::check_certificate_hash(&cert),
        proof_cert::CertificateHashCheck::Match
    ) || cert.atoms.len() != 1
        || cert.mumei_version != binding.compiler_version
        || proof_cert::validate_certificate_translator_versions(&cert).is_err()
        || cert.atoms[0].name != "main"
        || cert.atoms[0].content_hash != binding.atom_content_hash
        || cert.atoms[0].requires != atom.requires
        || cert.atoms[0].ensures != atom.ensures
        || cert.atoms[0].body_expr != atom.body_expr
        || !cert.atoms[0].assumed_clauses.is_empty()
        || !cert.atoms[0].forall_constraints.is_empty()
        || !cert.atoms[0].effects.is_empty()
        || sha256(
            &serde_json::to_vec(&cert.atoms[0].translator_ir)
                .map_err(|_| BundleError("IR encoding failed"))?,
        ) != binding.translator_ir_sha256
        || cert.atoms[0].status != proof_cert::status::VERIFIED
        || cert.atoms[0].z3_check_result != proof_cert::status::Z3_UNSAT
        || cert.atoms[0].reused_from_store
        || !cert.atoms[0].dependencies.is_empty()
        || proof_cert::verify_certificate(&cert, &[&atom], false)
            .iter()
            .any(|(_, status)| status != proof_cert::status::CERTIFICATE_PROVEN)
    {
        return Err(BundleError("certificate binding mismatch"));
    }
    Ok(())
}

/// Parse and admit a multi-atom module under the widened wrapping profile:
/// every item is an atom satisfying the per-atom gates — plus, when
/// `admit_refined`, refined scalar type declarations — and exactly one
/// atom is `main` — zero-argument with a literal `true` precondition,
/// matching the profile the C `main` wrapper attests. Returns the
/// declared refined types so the build path can register them with the
/// verifier's environment the same way `mumei verify` does.
pub fn prepare_module_source(
    source: &str,
    admit_refined: bool,
) -> Result<
    (
        ModuleSourceBinding,
        Vec<parser::Atom>,
        Vec<parser::RefinedType>,
    ),
    BundleError,
> {
    let contracts = semantic_contract::extract_pure_i64_module_contracts(
        source,
        SOURCE_LABEL,
        None,
        admit_refined,
    )
    .map_err(|_| BundleError("unsupported or invalid module source"))?;
    let mut items =
        parser::parse_module_checked(source).map_err(|_| BundleError("invalid source"))?;
    let mut atoms = Vec::with_capacity(items.len());
    let mut types = Vec::new();
    let mut mains = 0usize;
    for item in items.drain(..) {
        match item {
            parser::Item::Atom(mut atom) => {
                if atom.name == "main" {
                    mains += 1;
                    // The C `main` wrapper attests an i64 user body — the widened
                    // profile admits other scalar returns for siblings, but the
                    // entry point keeps the single-atom `-> i64` signature.
                    if !atom.params.is_empty()
                        || atom.requires.trim() != "true"
                        || atom.return_type.as_deref() != Some("i64")
                    {
                        return Err(BundleError(
                            "profile requires zero-argument i64 main with literal true precondition",
                        ));
                    }
                }
                atom.span.file = SOURCE_LABEL.to_owned();
                atoms.push(atom);
            }
            parser::Item::TypeDef(typedef) => types.push(typedef),
            _ => return Err(BundleError("non-atom declarations")),
        }
    }
    if mains != 1 {
        return Err(BundleError("profile requires exactly one main atom"));
    }
    let mut bindings = BTreeMap::new();
    for contract in &contracts {
        bindings.insert(
            contract.atom.clone(),
            ModuleAtomBinding {
                atom_content_hash: contract.atom_content_hash.clone(),
                specification_sha256: contract.specification_sha256.clone(),
                implementation_sha256: contract.implementation_sha256.clone(),
                hir_sha256: contract.hir.sha256.clone(),
                mir_sha256: contract.mir.sha256.clone(),
                translator_ir_sha256: sha256(
                    &serde_json::to_vec(&contract.translator_ir)
                        .map_err(|_| BundleError("IR encoding failed"))?,
                ),
            },
        );
    }
    let first = contracts
        .first()
        .ok_or(BundleError("expected at least one atom"))?;
    Ok((
        ModuleSourceBinding {
            source_sha256: first.source_sha256.clone(),
            source_profile: first.profile.to_owned(),
            atoms: bindings,
            hir_encoding: first.hir.encoding.to_owned(),
            mir_encoding: first.mir.encoding.to_owned(),
            translator_version: first.translator_version.to_owned(),
            bridge_lemma_hash: first.bridge_lemma_hash.to_owned(),
            compiler_version: first.compiler_version.to_owned(),
            source_dependencies_sha256: source_dependencies_sha256(&atoms)?,
        },
        atoms,
        types,
    ))
}

/// Normalize an import path or member key to its `<dir>/<stem>.mm`
/// member name under the member-path grammar: `/`-separated nonempty
/// segments, each free of `\\`, `:` and a leading dot, the last with a
/// nonempty stem. Absolute paths and `..`/`.`/leading-dot segments all
/// fail the grammar honestly (`std/` is an ordinary subdirectory —
/// the language resolver tries local files before the toolchain std
/// fallback). Members resolve file-relative: the key for an import in
/// `sub/b.mm` is `sub/` + the normalized path (see `resolve_member`).
pub fn member_name(path: &str) -> Result<String, BundleError> {
    const MAX_MEMBER_SEGMENTS: usize = 16;
    let stem = path.strip_suffix(".mm").unwrap_or(path);
    let bad = stem.is_empty()
        || stem.split('/').count() > MAX_MEMBER_SEGMENTS
        || stem.split('/').any(|seg| {
            seg.is_empty() || seg.starts_with('.') || seg.bytes().any(|b| matches!(b, b'\\' | b':'))
        });
    if bad {
        return Err(BundleError("import outside member-path profile"));
    }
    Ok(format!("{stem}.mm"))
}

/// Resolve an import declared inside `importer` to its member key.
/// Imports are file-relative (the language's resolver uses the
/// importing file's directory), so the importer's own directory
/// prefix joins in front; `..` is outside the grammar, so a dep can
/// never escape the importing directory's subtree.
pub fn resolve_member(importer: &str, path: &str) -> Result<String, BundleError> {
    let prefix = importer
        .rsplit_once('/')
        .map(|(dir, _)| format!("{dir}/"))
        .unwrap_or_default();
    // The composed path goes through the grammar so producer and
    // consumer agree on every bound — segment count included — for the
    // full key, not just the import's own suffix.
    member_name(&format!("{prefix}{path}"))
}

/// The qualifier an unaliased import binds: the LAST segment's stem,
/// so `import "sub/dep"` introduces `dep` (not `sub/dep`).
/// `trim_end_matches` (not a single `strip_suffix`) keeps the result a
/// spellable identifier: `dep.mm.mm` qualifies as `dep` since a
/// `.`-containing qualifier could never appear in source anyway.
fn member_stem(member: &str) -> &str {
    member
        .rsplit('/')
        .next()
        .unwrap_or(member)
        .trim_end_matches(".mm")
}

/// Derive the canonical merged text for an import closure: a
/// post-order depth-first walk from the root over the pinned member
/// map emits each member after its own imports (dependencies first,
/// siblings in declaration order), so a diamond member emits once at
/// its first visit and cycles fail honestly. Each member's own import
/// lines are stripped and its own import qualifiers normalized to bare
/// callee names — a member's namespace is file-local, matching the
/// language's resolved ModuleEnv. Returns the merged text and the full
/// `{member: [sorted imports]}` graph. Determinism comes from the
/// pinned member map, so a recheck re-derives byte-identical text.
pub fn merge_import_closure(
    root_name: &str,
    sources: &BTreeMap<String, String>,
) -> Result<(String, BTreeMap<String, Vec<String>>), BundleError> {
    if !sources.contains_key(root_name) {
        return Err(BundleError("root source member not pinned"));
    }
    let mut visiting = std::collections::BTreeSet::new();
    let mut emitted = std::collections::BTreeSet::new();
    let mut emitted_hashes = std::collections::BTreeSet::new();
    let mut graph = BTreeMap::new();
    let mut parts = Vec::new();
    visit_member(
        root_name,
        sources,
        &mut visiting,
        &mut emitted,
        &mut emitted_hashes,
        &mut graph,
        &mut parts,
    )?;
    if graph[root_name].is_empty() {
        return Err(BundleError("expected at least one import"));
    }
    Ok((format!("{}\n", parts.join("\n")), graph))
}

/// One DFS node: gather the member's imports under the same gates as
/// the root (single-line strict grammar, no duplicates, no
/// self-import, distinct qualifiers), recurse into them, then emit the
/// member's text with its import lines removed and its own qualifiers
/// stripped.
fn visit_member(
    member: &str,
    sources: &BTreeMap<String, String>,
    visiting: &mut std::collections::BTreeSet<String>,
    emitted: &mut std::collections::BTreeSet<String>,
    emitted_hashes: &mut std::collections::BTreeSet<String>,
    graph: &mut BTreeMap<String, Vec<String>>,
    parts: &mut Vec<String>,
) -> Result<(), BundleError> {
    if emitted.contains(member) {
        return Ok(());
    }
    if !visiting.insert(member.to_owned()) {
        return Err(BundleError("circular import outside profile"));
    }
    let text = sources
        .get(member)
        .ok_or(BundleError("imported source member not pinned"))?;
    let (imports, qualifiers, import_lines) = member_imports(member, text)?;
    let mut sorted = imports.clone();
    sorted.sort();
    graph.insert(member.to_owned(), sorted);
    for dep in &imports {
        visit_member(
            dep,
            sources,
            visiting,
            emitted,
            emitted_hashes,
            graph,
            parts,
        )?;
    }
    visiting.remove(member);
    // Qualified calls are namespace syntax, not part of the atom
    // contract: the callee map keys on bare names, so `h::helper(`
    // merges as `helper(` — the same normalization the language's own
    // resolved ModuleEnv performs.
    // A member with no import lines contributes its text verbatim —
    // normalizing through `lines().join("\n")` would rewrite CRLF
    // endings and change the merged bytes slice-1 bundles pinned.
    let mut body = if import_lines.is_empty() {
        text.to_owned()
    } else {
        text.lines()
            .enumerate()
            .filter(|(n, _)| !import_lines.contains(&(n + 1)))
            .map(|(_, line)| line)
            .collect::<Vec<_>>()
            .join("\n")
    };
    for qualifier in &qualifiers {
        body = strip_qualifier(&body, qualifier);
    }
    let body = body.trim_end().to_owned();
    // Two member keys can pin the same bytes — one real file reached
    // by two importers through the fallback chain (e.g. `import
    // "std/x"` from root and from `sub/dep.mm`, both falling back to
    // the shipped stdlib). The language loads a resolved path once
    // and ModuleEnv registration folds same-name atoms silently, so
    // emitting the body twice would only trip the merged module's
    // duplicate-atom gate: emit identical bodies once. Distinct
    // member texts still emit side by side and keep that gate.
    if emitted_hashes.insert(sha256(body.as_bytes())) {
        parts.push(body);
    }
    emitted.insert(member.to_owned());
    Ok(())
}

/// A member's gated imports: member keys in declaration order, the
/// qualifier each brings into scope, and the import lines to strip.
type MemberImports = (Vec<String>, Vec<String>, std::collections::BTreeSet<usize>);

/// Parse a member's `import` items and gate them: member keys resolved
/// file-relative and pinned in `sources`, no duplicate imports, no
/// self-import (post-resolution, so `sub/a.mm` importing `"a"` counts),
/// distinct qualifiers (alias or stem — two members bound to the same
/// namespace sugar would strip to the same bare-name space
/// ambiguously), atoms the only other allowed item, and every import
/// alone on its line as one strict `import "..." [as x];` statement
/// (a trailing `//` comment is trivia).
fn member_imports(member_key: &str, text: &str) -> Result<MemberImports, BundleError> {
    // `parse_module` (not the checked entry point) because the checked
    // pipeline consumes `import` items; whatever the lenient scan cannot
    // see stays in the text and fails the merged admission downstream.
    let items = parser::parse_module(text);
    let mut imports = Vec::new();
    let mut qualifiers = Vec::new();
    let mut seen_qualifiers = std::collections::BTreeSet::new();
    let mut import_lines = std::collections::BTreeSet::new();
    for item in &items {
        match item {
            parser::Item::Import(decl) => {
                let member = resolve_member(member_key, &decl.path)?;
                if member == member_key {
                    return Err(BundleError("self-import outside profile"));
                }
                if imports.contains(&member) {
                    return Err(BundleError("duplicate import"));
                }
                let qualifier = decl
                    .alias
                    .clone()
                    .unwrap_or_else(|| member_stem(&member).to_owned());
                if !seen_qualifiers.insert(qualifier.clone()) {
                    return Err(BundleError("duplicate import qualifier outside profile"));
                }
                qualifiers.push(qualifier);
                imports.push(member);
                import_lines.insert(decl.span.line);
            }
            parser::Item::Atom(_) => (),
            _ => return Err(BundleError("declaration outside multifile profile")),
        }
    }
    // Every import must sit alone on its line so stripping cannot leave
    // residue: the line the decl's span names must parse as exactly one
    // `import "..." [as x];` statement — an `import "a"; atom f()`
    // line would silently drop `f` from the merge otherwise.
    for line in &import_lines {
        let text = text
            .lines()
            .nth(line.saturating_sub(1))
            .map(str::trim)
            .unwrap_or_default();
        // A trailing `//` comment is trivia — judge the strict grammar
        // on the code before it. `//` inside the quoted member name
        // would already fail the member-name grammar downstream.
        let code = text.split("//").next().unwrap_or_default().trim_end();
        let items = parser::parse_module(code);
        // The lenient parser tolerates a missing `;`, but the profile
        // requires the strict statement form so a malformed member
        // cannot ride the merge into a verified bundle.
        if items.len() != 1 || !matches!(items[0], parser::Item::Import(_)) || !code.ends_with(';')
        {
            return Err(BundleError("multiline or embedded import outside profile"));
        }
    }
    Ok((imports, qualifiers, import_lines))
}

/// Remove every `<qualifier> <ws>* :: <ws>*` prefix where the qualifier
/// sits at an identifier boundary and the `::` introduces a callee name.
/// Boundary rules treat every non-ASCII byte as an identifier byte, so
/// `ph::f`, `xhelpers::f` and `éh::f` all survive a `h` qualifier; the
/// whitespace-tolerant match mirrors the parser, which accepts
/// `h :: helper` as the same qualified call as `h::helper`. A `::` not
/// followed by an identifier start is left alone so malformed syntax
/// still reaches merged admission and fails closed.
fn strip_qualifier(text: &str, qualifier: &str) -> String {
    fn is_ident_byte(byte: u8) -> bool {
        !byte.is_ascii() || byte.is_ascii_alphanumeric() || byte == b'_'
    }
    fn is_ident_start(byte: u8) -> bool {
        !byte.is_ascii() || byte.is_ascii_alphabetic() || byte == b'_'
    }
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let ws = |c: char| c.is_ascii_whitespace();
    // `i` is the copy cursor and `j` the scan cursor: a non-qualifying
    // occurrence advances only `j`, keeping `i` behind so the text up to
    // it is still emitted, while the boundary check sees the byte that
    // precedes the match in the original text.
    let mut i = 0;
    let mut j = 0;
    while let Some(rel) = text[j..].find(qualifier) {
        let pos = j + rel;
        // A match inside a `//` comment is comment text, not a call:
        // stripping it would splice the line's tail onto the next line
        // (`// h ::` before `atom f` becomes `// atom f`) and erase the
        // following declaration.
        let line_start = text[..pos].rfind('\n').map_or(0, |n| n + 1);
        if text[line_start..pos].contains("//") {
            j = pos + qualifier.len();
            continue;
        }
        let boundary = pos == 0 || !is_ident_byte(bytes[pos - 1]);
        let after = &text[pos + qualifier.len()..];
        let tail = after.trim_start_matches(ws);
        // `<qualifier> ws* :: ws* <ident-start>` — the ident-start keeps
        // `q :: 5` malformed syntax on its way to merged admission.
        let ws2 = (boundary && tail.starts_with("::"))
            .then(|| &tail["::".len()..])
            .and_then(|t| {
                let callee = t.trim_start_matches(ws);
                callee
                    .as_bytes()
                    .first()
                    .is_some_and(|b| is_ident_start(*b))
                    .then(|| t.len() - callee.len())
            });
        if let Some(ws2) = ws2 {
            let ws1 = after.len() - tail.len();
            out.push_str(&text[i..pos]);
            i = pos + qualifier.len() + ws1 + "::".len() + ws2;
            j = i;
        } else {
            j = pos + qualifier.len();
        }
    }
    out.push_str(&text[i..]);
    out
}

/// Pin the file-level import graph under the same canonical-JSON
/// convention the atom-level `source_dependencies_sha256` pin uses:
/// `{member: [sorted direct imports]}` for every member in the
/// closure.
pub fn import_graph_sha256(graph: &BTreeMap<String, Vec<String>>) -> String {
    sha256(&serde_json::to_vec(graph).expect("member names serialize"))
}

/// The full multifile derivation's products: the merged text's per-atom
/// binding, the file-graph pin, the admitted atoms and type
/// declarations, and the canonical merged source itself, so
/// emit/recheck feed the same bytes the binding describes.
pub struct PreparedMultifileSource {
    pub binding: ModuleSourceBinding,
    pub import_graph_sha256: String,
    pub atoms: Vec<parser::Atom>,
    pub types: Vec<parser::RefinedType>,
    pub merged: String,
}

/// Merge the import closure and admit the merged text under the module
/// gate (atom items plus, under `admit_refined`, refined `type`
/// declarations).
pub fn prepare_multifile_source(
    root_name: &str,
    sources: &BTreeMap<String, String>,
    admit_refined: bool,
) -> Result<PreparedMultifileSource, BundleError> {
    let (merged, graph) = merge_import_closure(root_name, sources)?;
    let (binding, atoms, types) = prepare_module_source(&merged, admit_refined)?;
    Ok(PreparedMultifileSource {
        binding,
        import_graph_sha256: import_graph_sha256(&graph),
        atoms,
        types,
        merged,
    })
}

/// Pin a module record the same way `load_pinned_record` pins a
/// single-atom one — the schema/profile/scope/settings gates differ only
/// in the constants they bind.
pub fn load_pinned_module_record(
    bytes: &[u8],
    expected_id: &str,
) -> Result<ModuleNativeBundleRecord, BundleError> {
    if !is_digest(expected_id) || sha256(bytes) != expected_id {
        return Err(BundleError("build identity mismatch"));
    }
    let record: ModuleNativeBundleRecord =
        serde_json::from_slice(bytes).map_err(|_| BundleError("invalid record"))?;
    if record.schema != MODULE_SCHEMA
        || ![MODULE_PROFILE, MODULE_PROFILE_V2].contains(&record.profile.as_str())
        || record.scope != MODULE_SCOPE
        || record.system_scope != SYSTEM_SCOPE
        || record.settings != BuildSettings::default()
        || record.files.len() != BundleFile::ALL.len()
        || BundleFile::ALL
            .iter()
            .any(|role| !record.files.contains_key(role))
    {
        return Err(BundleError("unsupported schema, scope or settings"));
    }
    if !record.object_build.target_triple.starts_with("x86_64-")
        || !record.object_build.target_triple.contains("linux")
        || record.object_build.data_layout.is_empty()
        || !record.object_build.llvm_version.starts_with("17.")
        || !record.link_build.driver_target.starts_with("x86_64-")
        || !record.link_build.driver_target.contains("linux")
        || !["cc", "gcc", "clang", "clang-17"].contains(&record.link_build.driver.as_str())
        || !["gnu-ld", "lld"].contains(&record.link_build.linker.as_str())
        || !is_digest(&record.link_build.arguments_sha256)
        || record
            .link_build
            .canonical_arguments_sha256
            .as_deref()
            .is_some_and(|pin| !is_digest(pin))
        || [
            &record.object_build.llvm_version,
            &record.link_build.driver_version,
            &record.link_build.linker_version,
        ]
        .iter()
        .any(|value| !numeric_version(value))
    {
        return Err(BundleError("unsupported toolchain observation"));
    }
    Ok(record)
}

/// Pin a multifile record the same way `load_pinned_module_record` pins
/// a module one, plus the member-set gates: nonempty and bounded, every
/// member name well-formed, every member pinned by digest, the declared
/// root a member, and the import-graph pin a digest.
pub fn load_pinned_multifile_record(
    bytes: &[u8],
    expected_id: &str,
) -> Result<MultifileNativeBundleRecord, BundleError> {
    if !is_digest(expected_id) || sha256(bytes) != expected_id {
        return Err(BundleError("build identity mismatch"));
    }
    let record: MultifileNativeBundleRecord =
        serde_json::from_slice(bytes).map_err(|_| BundleError("invalid record"))?;
    if record.schema != MULTIFILE_SCHEMA
        || ![MULTIFILE_PROFILE, MULTIFILE_PROFILE_V2].contains(&record.profile.as_str())
        || record.scope != MODULE_SCOPE
        || record.system_scope != SYSTEM_SCOPE
        || record.settings != BuildSettings::default()
        || record.files.len() != BundleFile::ALL.len()
        || BundleFile::ALL
            .iter()
            .any(|role| !record.files.contains_key(role))
    {
        return Err(BundleError("unsupported schema, scope or settings"));
    }
    if record.sources.is_empty()
        || record.sources.len() > MAX_SOURCE_FILES
        || record
            .sources
            .keys()
            .any(|name| member_name(name).ok().as_deref() != Some(name.as_str()))
        || !record.sources.contains_key(&record.source_files.root)
        || !is_digest(&record.source_files.import_graph_sha256)
        || record.sources.values().any(|pin| !is_digest(&pin.sha256))
    {
        return Err(BundleError("unsupported source member set"));
    }
    if !record.object_build.target_triple.starts_with("x86_64-")
        || !record.object_build.target_triple.contains("linux")
        || record.object_build.data_layout.is_empty()
        || !record.object_build.llvm_version.starts_with("17.")
        || !record.link_build.driver_target.starts_with("x86_64-")
        || !record.link_build.driver_target.contains("linux")
        || !["cc", "gcc", "clang", "clang-17"].contains(&record.link_build.driver.as_str())
        || !["gnu-ld", "lld"].contains(&record.link_build.linker.as_str())
        || !is_digest(&record.link_build.arguments_sha256)
        || record
            .link_build
            .canonical_arguments_sha256
            .as_deref()
            .is_some_and(|pin| !is_digest(pin))
        || [
            &record.object_build.llvm_version,
            &record.link_build.driver_version,
            &record.link_build.linker_version,
        ]
        .iter()
        .any(|value| !numeric_version(value))
    {
        return Err(BundleError("unsupported toolchain observation"));
    }
    Ok(record)
}

/// Compare a module bundle's snapshots to an externally supplied digest —
/// per-atom certificate binding, never a proof or authenticity verdict.
pub fn check_module_identity(
    record_bytes: &[u8],
    expected_id: &str,
    source: &str,
    files: &BTreeMap<BundleFile, Vec<u8>>,
) -> Result<(), BundleError> {
    let record = load_pinned_module_record(record_bytes, expected_id)?;
    let (binding, atoms, _types) =
        prepare_module_source(source, module_admit_refined(&record.profile)?)?;
    if record.source != binding {
        return Err(BundleError("source or compiler identity mismatch"));
    }
    check_module_members(&record.files, &binding, &atoms, files)
}

/// Compare a multifile bundle's snapshots to an externally supplied
/// digest: the supplied member texts must equal the pinned members
/// byte-for-byte, then the merge is re-derived from them and must bind
/// to the same source snapshot — the consumer never reads a filesystem.
pub fn check_multifile_identity(
    record_bytes: &[u8],
    expected_id: &str,
    root_name: &str,
    sources: &BTreeMap<String, String>,
    files: &BTreeMap<BundleFile, Vec<u8>>,
) -> Result<(), BundleError> {
    let record = load_pinned_multifile_record(record_bytes, expected_id)?;
    if record.source_files.root != root_name {
        return Err(BundleError("root source member mismatch"));
    }
    if sources.len() != record.sources.len()
        || sources.iter().any(|(name, text)| {
            record.sources.get(name) != Some(&ContentIdentity::of(text.as_bytes()))
        })
    {
        return Err(BundleError("missing or replaced source member"));
    }
    let prepared = prepare_multifile_source(
        root_name,
        sources,
        multifile_admit_refined(&record.profile)?,
    )?;
    if record.source != prepared.binding
        || record.source_files.import_graph_sha256 != prepared.import_graph_sha256
    {
        return Err(BundleError("source or compiler identity mismatch"));
    }
    check_module_members(&record.files, &prepared.binding, &prepared.atoms, files)
}

/// The member/certificate half of module-family identity — shared by the
/// module and multifile paths once the record's `files` pins, the merged
/// source binding and the atoms are established.
fn check_module_members(
    record_files: &BTreeMap<BundleFile, ContentIdentity>,
    binding: &ModuleSourceBinding,
    atoms: &[parser::Atom],
    files: &BTreeMap<BundleFile, Vec<u8>>,
) -> Result<(), BundleError> {
    if files.len() != BundleFile::ALL.len()
        || BundleFile::ALL.iter().any(|role| {
            files
                .get(role)
                .is_none_or(|data| record_files.get(role) != Some(&ContentIdentity::of(data)))
        })
    {
        return Err(BundleError("missing or replaced bundle component"));
    }
    let cert: proof_cert::ProofCertificate =
        serde_json::from_slice(&files[&BundleFile::Certificate])
            .map_err(|_| BundleError("invalid certificate"))?;
    if !matches!(
        proof_cert::check_certificate_hash(&cert),
        proof_cert::CertificateHashCheck::Match
    ) || cert.atoms.len() != atoms.len()
        || cert.mumei_version != binding.compiler_version
        || proof_cert::validate_certificate_translator_versions(&cert).is_err()
    {
        return Err(BundleError("certificate binding mismatch"));
    }
    for (atom, cert_atom) in atoms.iter().zip(cert.atoms.iter()) {
        let Some(atom_binding) = binding.atoms.get(&atom.name) else {
            return Err(BundleError("certificate binding mismatch"));
        };
        // The certificate annotates its translator IR with the
        // manual-lemma reason (`match`/inductive bodies always carry
        // one); the binding pins the extraction gate's IR identity,
        // which never carries that annotation.
        let mut cert_translator_ir = cert_atom.translator_ir.clone();
        cert_translator_ir.manual_lemma_reason = None;
        if cert_atom.name != atom.name
            || cert_atom.content_hash != atom_binding.atom_content_hash
            || cert_atom.requires != atom.requires
            || cert_atom.ensures != atom.ensures
            || cert_atom.body_expr != atom.body_expr
            || !cert_atom.assumed_clauses.is_empty()
            || !cert_atom.forall_constraints.is_empty()
            || !cert_atom.effects.is_empty()
            || sha256(
                &serde_json::to_vec(&cert_translator_ir)
                    .map_err(|_| BundleError("IR encoding failed"))?,
            ) != atom_binding.translator_ir_sha256
            || cert_atom.status != proof_cert::status::VERIFIED
            || cert_atom.z3_check_result != proof_cert::status::Z3_UNSAT
            || cert_atom.reused_from_store
            || !cert_atom.dependencies.is_empty()
        {
            return Err(BundleError("certificate binding mismatch"));
        }
    }
    let atom_refs: Vec<&parser::Atom> = atoms.iter().collect();
    if proof_cert::verify_certificate(&cert, &atom_refs, false)
        .iter()
        .any(|(_, status)| status != proof_cert::status::CERTIFICATE_PROVEN)
    {
        return Err(BundleError("certificate binding mismatch"));
    }
    Ok(())
}

/// Recheck every module atom's postcondition after the identity gates —
/// only `unsat` establishes the scoped result per atom; a refuted atom
/// refutes the module and an inconclusive one makes it inconclusive.
pub fn recheck_module_source(
    record_bytes: &[u8],
    expected_id: &str,
    source: &str,
    files: &BTreeMap<BundleFile, Vec<u8>>,
) -> Result<SourceRecheck, BundleError> {
    use z3::{Config, Context, Solver};
    let record = load_pinned_module_record(record_bytes, expected_id)?;
    let admit_refined = module_admit_refined(&record.profile)?;
    let mut config = Config::new();
    config.set_timeout_msec(BuildSettings::default().solver_timeout_ms);
    config.set_param_value("rlimit", &RECHECK_RESOURCE_LIMIT.to_string());
    let context = Context::new(&config);
    let atoms =
        semantic_contract::encode_pure_i64_module_obligations(source, &context, admit_refined)
            .map_err(|_| BundleError("source outside bounded module recheck profile"))?;
    check_module_identity(record_bytes, expected_id, source, files)?;
    let solver = Solver::new(&context);
    solve_module_obligations(&atoms, &solver)
}

/// Recheck a multifile bundle's merged module the same way — the merged
/// text is re-derived from the pinned member bytes inside
/// `check_multifile_identity`, so the obligations bind exactly the bytes
/// the record describes.
pub fn recheck_multifile_source(
    record_bytes: &[u8],
    expected_id: &str,
    root_name: &str,
    sources: &BTreeMap<String, String>,
    files: &BTreeMap<BundleFile, Vec<u8>>,
) -> Result<SourceRecheck, BundleError> {
    use z3::{Config, Context, Solver};
    let record = load_pinned_multifile_record(record_bytes, expected_id)?;
    if record.source_files.root != root_name {
        return Err(BundleError("root source member mismatch"));
    }
    let admit_refined = multifile_admit_refined(&record.profile)?;
    let mut config = Config::new();
    config.set_timeout_msec(BuildSettings::default().solver_timeout_ms);
    config.set_param_value("rlimit", &RECHECK_RESOURCE_LIMIT.to_string());
    let context = Context::new(&config);
    let prepared = prepare_multifile_source(root_name, sources, admit_refined)?;
    let atoms = semantic_contract::encode_pure_i64_module_obligations(
        &prepared.merged,
        &context,
        admit_refined,
    )
    .map_err(|_| BundleError("source outside bounded module recheck profile"))?;
    check_multifile_identity(record_bytes, expected_id, root_name, sources, files)?;
    let solver = Solver::new(&context);
    solve_module_obligations(&atoms, &solver)
}

/// The per-atom obligation sweep shared by the module and multifile
/// source rechecks.
fn solve_module_obligations(
    atoms: &[semantic_contract::ModuleAtomObligations<'_>],
    solver: &z3::Solver,
) -> Result<SourceRecheck, BundleError> {
    let mut inconclusive = false;
    for atom in atoms {
        // An atom whose declared domain is empty — every guarded
        // operator poisoned or no input inside its signature bounds —
        // is outside the profile: its obligations would be vacuous,
        // matching the LLVM side's empty-domain rejection.
        solver.reset();
        solver.assert(&atom.domain);
        match solver.check() {
            z3::SatResult::Sat => (),
            z3::SatResult::Unsat => return Err(BundleError("empty source domain")),
            z3::SatResult::Unknown => inconclusive = true,
        }
        for obligation in &atom.obligations {
            solver.reset();
            solver.assert(obligation);
            match solver.check() {
                z3::SatResult::Unsat => (),
                z3::SatResult::Sat => return Ok(SourceRecheck::Refuted),
                z3::SatResult::Unknown => inconclusive = true,
            }
        }
    }
    Ok(if inconclusive {
        SourceRecheck::Inconclusive
    } else {
        SourceRecheck::RecheckedWithinProfile
    })
}

#[cfg(test)]
mod multifile_tests {
    use super::*;

    const HELPERS: &str = "atom helper(x: i64) -> i64\n    requires: true;\n    ensures: result == x + 1;\n    body: { x + 1 };\n";
    const ROOT: &str = "import \"helpers\" as h;\n\natom main() -> i64\n    requires: true;\n    ensures: result > 0;\n    body: { h::helper(0) };\n";

    fn members() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("app.mm".to_owned(), ROOT.to_owned()),
            ("helpers.mm".to_owned(), HELPERS.to_owned()),
        ])
    }

    #[test]
    fn merge_strips_imports_and_orders_imported_first() {
        let (merged, graph) = merge_import_closure("app.mm", &members()).unwrap();
        assert_eq!(graph["app.mm"], ["helpers.mm"]);
        assert!(graph["helpers.mm"].is_empty());
        assert!(merged.starts_with(HELPERS.trim_end()));
        assert!(merged.contains("atom main"));
        assert!(merged.contains("body: { helper(0) };"));
        assert!(!merged.contains("import") && !merged.contains("h::"));
    }

    #[test]
    fn member_name_grammar() {
        assert_eq!(member_name("helpers").unwrap(), "helpers.mm");
        assert_eq!(member_name("helpers.mm").unwrap(), "helpers.mm");
        // Slash-separated member paths (subdirectories of the root's
        // directory) are admitted; `std/` names a local subdirectory
        // exactly as the language resolver tries local files first.
        assert_eq!(member_name("a/b.mm").unwrap(), "a/b.mm");
        assert_eq!(member_name("a/b").unwrap(), "a/b.mm");
        assert_eq!(member_name("std/x.mm").unwrap(), "std/x.mm");
        for bad in [
            "../up.mm",
            "a/../b.mm",
            "a//b.mm",
            "a/./b.mm",
            "/abs.mm",
            ".mm",
            ".hidden.mm",
            "a/.hidden.mm",
            "",
            "a\\b.mm",
            "a:b.mm",
        ] {
            assert!(member_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn resolve_member_anchors_at_the_importing_file() {
        // File-relative: a member's imports resolve against its own
        // directory, mirroring resolver::imports resolve_path.
        assert_eq!(resolve_member("app.mm", "dep").unwrap(), "dep.mm");
        assert_eq!(resolve_member("app.mm", "sub/dep").unwrap(), "sub/dep.mm");
        assert_eq!(resolve_member("sub/dep.mm", "leaf").unwrap(), "sub/leaf.mm");
        assert_eq!(resolve_member("sub/dep.mm", "a/b").unwrap(), "sub/a/b.mm");
        // Confinement: escaping the root's directory is rejected.
        assert!(resolve_member("sub/dep.mm", "../escape").is_err());
        assert!(resolve_member("app.mm", "..").is_err());
    }

    const DEEP: &str =
        "atom deep() -> i64\n    requires: true;\n    ensures: result == 1;\n    body: { 1 };";

    #[test]
    fn nested_imports_merge_in_post_order() {
        // app -> helpers -> deep: deep emits first, helpers next, root last.
        let mut nested = members();
        nested.insert("deep.mm".into(), DEEP.into());
        nested.insert(
            "helpers.mm".into(),
            format!("import \"deep\" as d;\n{HELPERS}"),
        );
        let (merged, graph) = merge_import_closure("app.mm", &nested).unwrap();
        assert_eq!(graph["app.mm"], ["helpers.mm"]);
        assert_eq!(graph["helpers.mm"], ["deep.mm"]);
        assert!(graph["deep.mm"].is_empty());
        let deep_at = merged.find("atom deep").unwrap();
        let helper_at = merged.find("atom helper").unwrap();
        let main_at = merged.find("atom main").unwrap();
        assert!(deep_at < helper_at && helper_at < main_at);
        assert!(!merged.contains("import"));
    }

    #[test]
    fn nested_member_strips_its_own_qualifier() {
        // A member's namespace is file-local: helpers calls `d::deep(0)`
        // which merges as bare `deep(0)`, and the root's `h` never leaks
        // into the member's text.
        let mut nested = members();
        nested.insert("deep.mm".into(), DEEP.into());
        nested.insert(
            "helpers.mm".into(),
            "import \"deep\" as d;\natom helper(x: i64) -> i64\n    requires: true;\n    ensures: result == x + 1;\n    body: { x + d::deep(0) };".into(),
        );
        let (merged, _) = merge_import_closure("app.mm", &nested).unwrap();
        assert!(merged.contains("body: { x + deep(0) };"));
        assert!(!merged.contains("d::") && !merged.contains("h::"));
    }

    #[test]
    fn diamond_imports_emit_each_member_once() {
        // a -> b, c; b -> shared; c -> shared: shared emits once.
        let shared =
            "atom shared() -> i64\n    requires: true;\n    ensures: result == 1;\n    body: { 1 };";
        let b = "import \"shared\" as s;\natom fb() -> i64\n    requires: true;\n    ensures: result == 1;\n    body: { s::shared() };";
        let c = "import \"shared\" as t;\natom fc() -> i64\n    requires: true;\n    ensures: result == 1;\n    body: { t::shared() };";
        let a = "import \"b\";\nimport \"c\";\natom main() -> i64\n    requires: true;\n    ensures: result > 0;\n    body: { fb() + fc() };";
        let sources = BTreeMap::from([
            ("app.mm".to_owned(), a.to_owned()),
            ("b.mm".to_owned(), b.to_owned()),
            ("c.mm".to_owned(), c.to_owned()),
            ("shared.mm".to_owned(), shared.to_owned()),
        ]);
        let (merged, graph) = merge_import_closure("app.mm", &sources).unwrap();
        assert_eq!(merged.matches("atom shared").count(), 1);
        assert_eq!(graph["b.mm"], ["shared.mm"]);
        assert_eq!(graph["c.mm"], ["shared.mm"]);
        assert!(merged.contains("body: { shared() };") && merged.contains("shared() }"));
    }

    #[test]
    fn cyclic_imports_are_rejected_honestly() {
        let a = "import \"b\";\natom fa() -> i64\n    requires: true;\n    ensures: result == 1;\n    body: { 1 };";
        let b = "import \"app\";\natom fb() -> i64\n    requires: true;\n    ensures: result == 1;\n    body: { 1 };";
        let sources = BTreeMap::from([
            ("app.mm".to_owned(), a.to_owned()),
            ("b.mm".to_owned(), b.to_owned()),
        ]);
        assert!(merge_import_closure("app.mm", &sources).is_err());
    }

    #[test]
    fn duplicate_qualifiers_are_rejected() {
        // `import "a" as k; import "b" as k;` binds both members to the
        // same namespace sugar: every `k::f` would strip to bare `f`
        // ambiguously.
        let member =
            "atom f() -> i64\n    requires: true;\n    ensures: result == 1;\n    body: { 1 };";
        let dup = BTreeMap::from([
            (
                "app.mm".to_owned(),
                "import \"a\" as k;\nimport \"b\" as k;\natom main() -> i64\n    requires: true;\n    ensures: result == 1;\n    body: { k::f() };".to_owned(),
            ),
            ("a.mm".to_owned(), member.to_owned()),
            ("b.mm".to_owned(), member.to_owned()),
        ]);
        assert!(merge_import_closure("app.mm", &dup).is_err());
        // The same ambiguity without aliases: `import "a"; import
        // "b";` is fine (qualifiers `a`/`b` differ), but a member named
        // `k.mm` aliased as `k` collides with an unaliased `k`.
        let alias_stem_dup = BTreeMap::from([
            (
                "app.mm".to_owned(),
                "import \"k\";\nimport \"a\" as k;\natom main() -> i64\n    requires: true;\n    ensures: result == 1;\n    body: { 1 };".to_owned(),
            ),
            ("a.mm".to_owned(), member.to_owned()),
            ("k.mm".to_owned(), member.to_owned()),
        ]);
        assert!(merge_import_closure("app.mm", &alias_stem_dup).is_err());
    }

    #[test]
    fn leaf_member_bytes_are_merged_verbatim() {
        // A CRLF leaf contributes its bytes untouched: the merge must
        // not silently normalize them, or a slice-1 bundle's pinned
        // merged sha could never re-derive.
        let mut crlf = members();
        crlf.insert(
            "helpers.mm".into(),
            "atom helper(x: i64) -> i64\r\n    requires: true;\r\n    ensures: result == x + 1;\r\n    body: { x + 1 };\r\n".into(),
        );
        let (merged, _) = merge_import_closure("app.mm", &crlf).unwrap();
        assert!(merged.contains("atom helper(x: i64) -> i64\r\n    requires: true;"));
    }

    #[test]
    fn unpinned_and_missing_members_rejected() {
        let only_root = BTreeMap::from([("app.mm".to_owned(), ROOT.to_owned())]);
        assert!(merge_import_closure("app.mm", &only_root).is_err());
        let missing_deep = BTreeMap::from([("app.mm".to_owned(), "import \"deep\";\natom main() -> i64\n    requires: true;\n    ensures: result == 1;\n    body: { 1 };".to_owned())]);
        assert!(merge_import_closure("app.mm", &missing_deep).is_err());
    }

    #[test]
    fn qualifier_stripping_respects_identifier_boundaries() {
        assert_eq!(strip_qualifier("h::f(0)", "h"), "f(0)");
        assert_eq!(strip_qualifier("x + h::f(0)", "h"), "x + f(0)");
        assert_eq!(strip_qualifier("ph::f", "h"), "ph::f");
        assert_eq!(strip_qualifier("_h::f", "h"), "_h::f");
        assert_eq!(strip_qualifier("helpers::f", "h"), "helpers::f");
        // A longer identifier starting with the qualifier keeps its `::`.
        assert_eq!(strip_qualifier("hh::f", "h"), "hh::f");
        // Non-ASCII bytes count as identifier bytes: `éh::f` is a call
        // through qualifier `éh`, not `h`.
        assert_eq!(strip_qualifier("éh::f", "h"), "éh::f");
        // The parser accepts whitespace around `::` in a qualified call.
        assert_eq!(strip_qualifier("h :: f(0)", "h"), "f(0)");
        assert_eq!(strip_qualifier("h  ::  f(0)", "h"), "f(0)");
        assert_eq!(strip_qualifier("h ::\nf(0)", "h"), "f(0)");
        // `q ::` not followed by a callee name is not qualified syntax —
        // left intact so merged admission rejects it.
        assert_eq!(strip_qualifier("x = h :: 5", "h"), "x = h :: 5");
        assert_eq!(strip_qualifier("x = h ::", "h"), "x = h ::");
        // A bare qualifier with no `::` is never touched.
        assert_eq!(strip_qualifier("let h = 3; h + 1", "h"), "let h = 3; h + 1");
    }

    #[test]
    fn qualifier_stripping_leaves_line_comments_alone() {
        // A qualifier look-alike inside a `//` comment is comment text:
        // stripping `// h ::` would splice the next line onto the
        // comment and erase the following declaration.
        assert_eq!(strip_qualifier("// h ::\natom f", "h"), "// h ::\natom f");
        assert_eq!(
            strip_qualifier("x = h::f // note h::g", "h"),
            "x = f // note h::g"
        );
        // A comment line still counts its `//` for later matches.
        assert_eq!(strip_qualifier("// note\nh::f(0)", "h"), "// note\nf(0)");
    }

    #[test]
    fn commented_import_line_and_qualified_comment_merge() {
        let commented = BTreeMap::from([
            (
                "app.mm".to_owned(),
                "// h :: is not a call\nimport \"helpers\" as h; // trailing note\n\natom main() -> i64\n    requires: true;\n    ensures: result > 0;\n    body: { h::helper(0) };\n".to_owned(),
            ),
            ("helpers.mm".to_owned(), HELPERS.to_owned()),
        ]);
        let (merged, graph) = merge_import_closure("app.mm", &commented).unwrap();
        assert_eq!(graph["app.mm"], ["helpers.mm"]);
        // The comment survives verbatim and `atom main` is not folded
        // into it.
        assert!(merged.contains("// h :: is not a call"));
        assert!(merged.contains("atom main"));
        assert!(merged.contains("body: { helper(0) };"));
    }

    #[test]
    fn import_without_semicolon_is_rejected() {
        let missing = BTreeMap::from([
            (
                "app.mm".to_owned(),
                ROOT.replacen("import \"helpers\" as h;", "import \"helpers\" as h", 1),
            ),
            ("helpers.mm".to_owned(), HELPERS.to_owned()),
        ]);
        assert!(merge_import_closure("app.mm", &missing).is_err());
    }

    #[test]
    fn import_sharing_a_line_with_another_declaration_is_rejected() {
        let shared = BTreeMap::from([
            (
                "app.mm".to_owned(),
                format!(
                    "import \"helpers\" as h; atom main() -> i64 requires: true; ensures: result > 0; body: {{ h::helper(0) }};"
                ),
            ),
            ("helpers.mm".to_owned(), HELPERS.to_owned()),
        ]);
        assert!(merge_import_closure("app.mm", &shared).is_err());
    }

    #[test]
    fn prepare_multifile_binds_merged_module() {
        let prepared = prepare_multifile_source("app.mm", &members(), false).unwrap();
        assert_eq!(
            prepared.binding.source_sha256,
            sha256(prepared.merged.as_bytes())
        );
        assert_eq!(prepared.binding.atoms.len(), 2);
        assert!(
            prepared.binding.atoms.contains_key("helper")
                && prepared.binding.atoms.contains_key("main")
        );
        assert!(is_digest(&prepared.import_graph_sha256));
        assert_eq!(prepared.atoms.len(), 2);
    }
}

#[cfg(test)]
mod source_recheck_tests {
    use super::*;
    use z3::{Config, Context, SatResult, Solver};

    #[test]
    fn only_unsat_can_establish_the_source_postcondition() {
        assert_eq!(
            source_recheck_result(SatResult::Unsat),
            SourceRecheck::RecheckedWithinProfile
        );
        assert_eq!(
            source_recheck_result(SatResult::Sat),
            SourceRecheck::Refuted
        );
        assert_eq!(
            source_recheck_result(SatResult::Unknown),
            SourceRecheck::Inconclusive
        );
    }

    #[test]
    fn source_obligation_models_conditions_signed_boundaries_and_real_exhaustion() {
        for (body, ensures, expected) in [
            ("7", "result == 7", SatResult::Unsat),
            ("7", "result == 8", SatResult::Sat),
            (
                "if 1 < 2 { 7 } else { 8 }",
                "(true => result == 7) && ((result < 8) == true)",
                SatResult::Unsat,
            ),
            ("-9223372036854775807", "result < 0", SatResult::Unsat),
            ("9223372036854775807", "result < 0", SatResult::Sat),
        ] {
            let config = Config::new();
            let context = Context::new(&config);
            let source = format!(
                "atom main() -> i64 requires: true; ensures: {ensures}; body: {{ {body} }};"
            );
            let obligation =
                semantic_contract::encode_pure_i64_obligation(&source, &context).unwrap();
            let solver = Solver::new(&context);
            solver.assert(&obligation);
            assert_eq!(solver.check(), expected);
        }
        for (requires, ensures, expected) in [
            ("x >= 0", "result == x", SatResult::Unsat),
            ("true", "result == x", SatResult::Sat),
            ("true", "result >= 0", SatResult::Unsat),
            ("false", "false", SatResult::Unsat),
        ] {
            let config = Config::new();
            let context = Context::new(&config);
            let source = format!("atom f(x: i64) -> i64 requires: {requires}; ensures: {ensures}; body: {{ if x < 0 {{ 0 }} else {{ x }} }};");
            let obligation =
                semantic_contract::encode_pure_i64_obligation(&source, &context).unwrap();
            let solver = Solver::new(&context);
            solver.assert(&obligation);
            assert_eq!(solver.check(), expected);
        }
        let mut config = Config::new();
        config.set_param_value("rlimit", "1");
        let context = Context::new(&config);
        let obligation = semantic_contract::encode_pure_i64_obligation("atom f(x: i64) -> i64 requires: true; ensures: result == x; body: { if x < 0 { 0 } else { x } };", &context).unwrap();
        let solver = Solver::new(&context);
        solver.assert(&obligation);
        assert_eq!(
            source_recheck_result(solver.check()),
            SourceRecheck::Inconclusive
        );
    }

    #[test]
    fn prepare_module_source_names_the_refined_profile_only_when_used() {
        // A module declaring or using refined scalar types binds the v2
        // source profile — and refuses the v1 admission outright, so a
        // v1 record could never have been built from it.
        let refined = "type Port = i64 where v >= 1 && v <= 65535;\n\
             atom main() -> i64 requires: true; ensures: result == 0; body: { 0 };";
        assert!(prepare_module_source(refined, false).is_err());
        let (binding, atoms, types) = prepare_module_source(refined, true).unwrap();
        assert_eq!(
            binding.source_profile,
            semantic_contract::PURE_I64_REFINED_PROFILE
        );
        assert_eq!(types.len(), 1);
        assert_eq!(atoms.len(), 1);
        // A plain module keeps the wrapping profile under the same gate.
        let plain = "atom main() -> i64 requires: true; ensures: result == 0; body: { 0 };";
        let (binding, _atoms, types) = prepare_module_source(plain, true).unwrap();
        assert_eq!(
            binding.source_profile,
            semantic_contract::PURE_I64_WRAPPING_PROFILE
        );
        assert!(types.is_empty());
        // The record profile mirrors the bound source profile, and both
        // admission modes decode — anything else is not this schema.
        assert!(!module_admit_refined(MODULE_PROFILE).unwrap());
        assert!(module_admit_refined(MODULE_PROFILE_V2).unwrap());
        assert!(!multifile_admit_refined(MULTIFILE_PROFILE).unwrap());
        assert!(multifile_admit_refined(MULTIFILE_PROFILE_V2).unwrap());
        assert!(module_admit_refined("mumei.other/v1").is_err());
        assert!(multifile_admit_refined("mumei.other/v1").is_err());
    }
}
