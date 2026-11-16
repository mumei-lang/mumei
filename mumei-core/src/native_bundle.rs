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
pub const MODULE_SCOPE: &str =
    "identity_consistency_only/no_authenticity_or_artifact_semantic_proof";
pub const MODULE_RECHECK_PROFILE: &str = "mumei.native-module-source-recheck/v1";

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
pub const MULTIFILE_RECHECK_PROFILE: &str = "mumei.native-multifile-source-recheck/v1";
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
/// every item is an atom satisfying the per-atom gates, and exactly one is
/// `main` — zero-argument with a literal `true` precondition, matching the
/// profile the C `main` wrapper attests.
pub fn prepare_module_source(
    source: &str,
) -> Result<(ModuleSourceBinding, Vec<parser::Atom>), BundleError> {
    let contracts =
        semantic_contract::extract_pure_i64_module_contracts(source, SOURCE_LABEL, None)
            .map_err(|_| BundleError("unsupported or invalid module source"))?;
    let mut items =
        parser::parse_module_checked(source).map_err(|_| BundleError("invalid source"))?;
    let mut atoms = Vec::with_capacity(items.len());
    let mut mains = 0usize;
    for item in items.drain(..) {
        let parser::Item::Atom(mut atom) = item else {
            return Err(BundleError("non-atom declarations"));
        };
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
    ))
}

/// Normalize an import path or member key to its `<stem>.mm` member name
/// under the slice-1 grammar: a bare basename — no directory component,
/// dot segments, or host/drive qualifiers, and a nonempty stem. `std/`
/// and subdirectory or parent-relative imports all fail the no-`/` rule.
pub fn member_name(path: &str) -> Result<String, BundleError> {
    let stem = path.strip_suffix(".mm").unwrap_or(path);
    if stem.is_empty()
        || stem.starts_with('.')
        || stem.bytes().any(|b| matches!(b, b'/' | b'\\' | b':'))
    {
        return Err(BundleError("import outside same-directory profile"));
    }
    Ok(format!("{stem}.mm"))
}

/// Derive the canonical merged text for an import closure: parse the
/// root member, walk its single-level `import` declarations in
/// declaration order, and concatenate each imported member's text
/// (atoms only — nested imports stay outside this profile) followed by
/// the root's text with its import lines stripped. Returns the merged
/// text and the imported member names in order. Determinism comes from
/// the pinned member map, so a recheck re-derives byte-identical text.
pub fn merge_import_closure(
    root_name: &str,
    sources: &BTreeMap<String, String>,
) -> Result<(String, Vec<String>), BundleError> {
    let root = sources
        .get(root_name)
        .ok_or(BundleError("root source member not pinned"))?;
    // `parse_module` (not the checked entry point) because the checked
    // pipeline consumes `import` items; whatever the lenient scan cannot
    // see stays in the text and fails the merged admission downstream.
    let items = parser::parse_module(root);
    let mut imports: Vec<String> = Vec::new();
    let mut import_lines = std::collections::BTreeSet::new();
    // The qualifier each import brings into scope — its alias when
    // declared, else the member stem — so merged calls resolve as the
    // bare callee names the module profile's callee map keys on.
    let mut qualifiers: Vec<String> = Vec::new();
    for item in &items {
        match item {
            parser::Item::Import(decl) => {
                let member = member_name(&decl.path)?;
                if member == root_name {
                    return Err(BundleError("self-import outside profile"));
                }
                if imports.contains(&member) {
                    return Err(BundleError("duplicate import"));
                }
                if !sources.contains_key(&member) {
                    return Err(BundleError("imported source member not pinned"));
                }
                qualifiers.push(
                    decl.alias
                        .clone()
                        .unwrap_or_else(|| member.trim_end_matches(".mm").to_owned()),
                );
                imports.push(member);
                import_lines.insert(decl.span.line);
            }
            parser::Item::Atom(_) => (),
            _ => return Err(BundleError("declaration outside multifile profile")),
        }
    }
    if imports.is_empty() {
        return Err(BundleError("expected at least one import"));
    }
    // Every import must sit alone on its line so stripping cannot leave
    // residue: the line the decl's span names must be the whole
    // `import "..." [as x];` statement.
    for line in &import_lines {
        match root.lines().nth(line.saturating_sub(1)).map(str::trim) {
            Some(text) if text.starts_with("import ") && text.ends_with(';') => (),
            _ => return Err(BundleError("multiline or embedded import outside profile")),
        }
    }
    let mut parts = Vec::with_capacity(imports.len() + 1);
    for member in &imports {
        let text = &sources[member];
        let imported = parser::parse_module(text);
        if imported
            .iter()
            .any(|item| !matches!(item, parser::Item::Atom(_)))
        {
            return Err(BundleError(
                "nested imports or non-atom declarations outside profile",
            ));
        }
        parts.push(text.trim_end().to_owned());
    }
    let stripped: Vec<&str> = root
        .lines()
        .enumerate()
        .filter(|(n, _)| !import_lines.contains(&(n + 1)))
        .map(|(_, line)| line)
        .collect();
    // Qualified calls are namespace syntax, not part of the atom
    // contract: the callee map keys on bare names, so `h::helper(`
    // merges as `helper(` — the same normalization the language's own
    // resolved ModuleEnv performs.
    let mut merged_root = stripped.join("\n");
    for qualifier in &qualifiers {
        merged_root = merged_root.replace(&format!("{qualifier}::"), "");
    }
    parts.push(merged_root.trim_end().to_owned());
    Ok((format!("{}\n", parts.join("\n")), imports))
}

/// Pin the file-level import graph under the same canonical-JSON
/// convention the atom-level `source_dependencies_sha256` pin uses.
fn import_graph_sha256(root_name: &str, imports: &[String]) -> String {
    let mut sorted = imports.to_vec();
    sorted.sort();
    let mut graph = BTreeMap::from([(root_name.to_owned(), sorted)]);
    for member in imports {
        graph.insert(member.clone(), Vec::new());
    }
    sha256(&serde_json::to_vec(&graph).expect("member names serialize"))
}

/// The full multifile derivation: merge the import closure, admit the
/// merged text under the module gate (now atom-only), and return its
/// per-atom binding, the file-graph pin, the atoms and the merged text
/// so emit/recheck feed the same bytes the binding describes.
pub fn prepare_multifile_source(
    root_name: &str,
    sources: &BTreeMap<String, String>,
) -> Result<(ModuleSourceBinding, String, Vec<parser::Atom>, String), BundleError> {
    let (merged, imports) = merge_import_closure(root_name, sources)?;
    let (binding, atoms) = prepare_module_source(&merged)?;
    Ok((
        binding,
        import_graph_sha256(root_name, &imports),
        atoms,
        merged,
    ))
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
        || record.profile != MODULE_PROFILE
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
        || record.profile != MULTIFILE_PROFILE
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
    let (binding, atoms) = prepare_module_source(source)?;
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
    let (binding, graph, atoms, _merged) = prepare_multifile_source(root_name, sources)?;
    if record.source != binding || record.source_files.import_graph_sha256 != graph {
        return Err(BundleError("source or compiler identity mismatch"));
    }
    check_module_members(&record.files, &binding, &atoms, files)
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
    load_pinned_module_record(record_bytes, expected_id)?;
    let mut config = Config::new();
    config.set_timeout_msec(BuildSettings::default().solver_timeout_ms);
    config.set_param_value("rlimit", &RECHECK_RESOURCE_LIMIT.to_string());
    let context = Context::new(&config);
    let atoms = semantic_contract::encode_pure_i64_module_obligations(source, &context)
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
    if load_pinned_multifile_record(record_bytes, expected_id)?
        .source_files
        .root
        != root_name
    {
        return Err(BundleError("root source member mismatch"));
    }
    let mut config = Config::new();
    config.set_timeout_msec(BuildSettings::default().solver_timeout_ms);
    config.set_param_value("rlimit", &RECHECK_RESOURCE_LIMIT.to_string());
    let context = Context::new(&config);
    let (_binding, _graph, _atoms, merged) = prepare_multifile_source(root_name, sources)?;
    let atoms = semantic_contract::encode_pure_i64_module_obligations(&merged, &context)
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
        let (merged, imports) = merge_import_closure("app.mm", &members()).unwrap();
        assert_eq!(imports, ["helpers.mm"]);
        assert!(merged.starts_with(HELPERS.trim_end()));
        assert!(merged.contains("atom main"));
        assert!(merged.contains("body: { helper(0) };"));
        assert!(!merged.contains("import") && !merged.contains("h::"));
    }

    #[test]
    fn member_name_grammar() {
        assert_eq!(member_name("helpers").unwrap(), "helpers.mm");
        assert_eq!(member_name("helpers.mm").unwrap(), "helpers.mm");
        for bad in [
            "../up.mm",
            "a/b.mm",
            "std/x.mm",
            "/abs.mm",
            ".mm",
            ".hidden.mm",
            "",
        ] {
            assert!(member_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn nested_imports_and_unpinned_members_rejected() {
        let mut nested = members();
        nested.insert(
            "deep.mm".into(),
            "atom deep()\n    requires: true;\n    ensures: result == 1;\n    body: { 1 };".into(),
        );
        nested.insert("helpers.mm".into(), format!("import \"deep\";\n{HELPERS}"));
        assert!(merge_import_closure("app.mm", &nested).is_err());
        let only_root = BTreeMap::from([("app.mm".to_owned(), ROOT.to_owned())]);
        assert!(merge_import_closure("app.mm", &only_root).is_err());
    }

    #[test]
    fn prepare_multifile_binds_merged_module() {
        let (binding, graph, atoms, merged) =
            prepare_multifile_source("app.mm", &members()).unwrap();
        assert_eq!(binding.source_sha256, sha256(merged.as_bytes()));
        assert_eq!(binding.atoms.len(), 2);
        assert!(binding.atoms.contains_key("helper") && binding.atoms.contains_key("main"));
        assert!(is_digest(&graph));
        assert_eq!(atoms.len(), 2);
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
}
