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
    let contract = semantic_contract::extract_pure_i64_contract(source, SOURCE_LABEL, None)
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
            source_dependencies_sha256: sha256(b"[]"),
        },
        atom,
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
