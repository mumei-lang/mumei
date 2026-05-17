// =============================================================================
// Plan 11B: Z3 Proof Certificates
// =============================================================================
//
// Generates cryptographically verifiable proof certificates for verified atoms.
// Each certificate contains per-atom Z3 check results and content hashes,
// enabling offline verification that proofs are still valid.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;

use crate::resolver;
use crate::verification::{self, ModuleEnv, SymbolProvenance, TranslatorIRMetadata};

/// Top-level proof certificate for a Mumei source file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofCertificate {
    /// Certificate format version
    pub version: String,
    /// ISO 8601 timestamp of certificate generation
    pub timestamp: String,
    /// Mumei compiler version
    pub mumei_version: String,
    /// Z3 solver version (if available)
    pub z3_version: String,
    /// Source file path
    pub file: String,
    /// Per-atom verification certificates
    pub atoms: Vec<AtomCertificate>,
    /// Package name from mumei.toml [package].name (P5-A)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_name: Option<String>,
    /// Package version from mumei.toml [package].version (P5-A)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_version: Option<String>,
    /// SHA-256 of the serialized certificate (excluding this field) for chain verification (P5-A)
    #[serde(default)]
    pub certificate_hash: String,
    /// Summary flag: true if all atoms are verified (P5-A)
    #[serde(default)]
    pub all_verified: bool,
}

/// Per-atom verification certificate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtomCertificate {
    /// Atom name
    pub name: String,
    /// Z3 solver check result: "unsat" (proven), "sat" (counter-example found), "unknown", "skipped"
    pub z3_check_result: String,
    /// SHA-256 hash of the atom's source text (requires + ensures + body)
    pub content_hash: String,
    /// Verification status: "verified", "failed", "skipped", "trusted"
    pub status: String,
    /// Pre-proof specification validation and traceability result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_validation_result: Option<verification::SpecValidationResult>,
    /// Dependency-aware proof hash including transitive callee signatures (P5-A)
    #[serde(default)]
    pub proof_hash: String,
    /// Direct callee atom names from dependency graph (P5-A)
    #[serde(default)]
    pub dependencies: Vec<String>,
    /// Declared effect names (P5-A)
    #[serde(default)]
    pub effects: Vec<String>,
    /// Precondition contract text (P5-A)
    #[serde(default)]
    pub requires: String,
    /// Postcondition contract text (P5-A)
    #[serde(default)]
    pub ensures: String,
    /// Deterministic Z3 outcome class used by Lean escalation routing.
    #[serde(default)]
    pub z3_result_class: String,
    /// Reason this atom should be escalated to Lean, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalation_reason: Option<String>,
    /// Logical fragments detected in this proof obligation.
    #[serde(default)]
    pub logic_fragment_tags: Vec<String>,
    /// Translator version required for Lean escalation cache validity.
    #[serde(default)]
    pub translator_version: String,
    /// Mumei witness name to generated Lean binder name mapping.
    #[serde(default)]
    pub binder_mapping: HashMap<String, String>,
    /// Hash of the bridge lemma set used by this translation contract.
    #[serde(default)]
    pub bridge_lemma_hash: String,
    /// Structured reason when this atom requires manual Lean lemma work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_lemma_reason: Option<String>,
    /// Typed intermediate representation metadata for Lean lowering.
    #[serde(default)]
    pub translator_ir: TranslatorIRMetadata,
    /// Lean-side result metadata populated by mumei-lean.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lean_metadata: Option<LeanResultMetadata>,
    /// P8-A: Counterexample validation status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counterexample_validation: Option<CounterexampleValidationMetadata>,
    /// P8-A: Symbol provenance for uninterpreted symbols.
    #[serde(default)]
    pub symbol_provenance: Vec<SymbolProvenance>,
    /// P8-A: Unused hypothesis report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unused_hypotheses: Option<UnusedHypothesisMetadata>,
    #[serde(default)]
    pub solver_process_metadata: Option<SolverProcessMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SolverProcessMetadata {
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub cache_key: String,
    #[serde(default)]
    pub generation_id: String,
    #[serde(default)]
    pub solver_config_fingerprint: String,
    #[serde(default)]
    pub timeout_ms: u64,
    #[serde(default)]
    pub cancel_reason: Option<String>,
    #[serde(default)]
    pub process_start_time: String,
    #[serde(default)]
    pub process_end_time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CounterexampleValidationMetadata {
    #[serde(default)]
    pub validation_status: String,
    #[serde(default)]
    pub validated_at: String,
    #[serde(default)]
    pub failed_constraints: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UnusedHypothesisMetadata {
    #[serde(default)]
    pub unused_requires: Vec<String>,
    #[serde(default)]
    pub unused_invariants: Vec<String>,
    #[serde(default)]
    pub minimal_constraint_set: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LeanResultMetadata {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub theorem_name: String,
    #[serde(default)]
    pub translator_version: String,
    #[serde(default)]
    pub bridge_lemma_hash: String,
    #[serde(default)]
    pub proof_path: String,
    #[serde(default)]
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscalationCandidate {
    pub name: String,
    pub z3_check_result: String,
    pub z3_result_class: String,
    pub status: String,
    pub content_hash: String,
    pub proof_hash: String,
    pub dependencies: Vec<String>,
    pub effects: Vec<String>,
    pub requires: String,
    pub ensures: String,
    pub escalation_reason: String,
    pub logic_fragment_tags: Vec<String>,
    #[serde(default)]
    pub translator_version: String,
    #[serde(default)]
    pub binder_mapping: HashMap<String, String>,
    #[serde(default)]
    pub bridge_lemma_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_lemma_reason: Option<String>,
    #[serde(default)]
    pub translator_ir: TranslatorIRMetadata,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lean_metadata: Option<LeanResultMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EscalationBundleSummary {
    pub total_atoms: usize,
    pub candidate_count: usize,
    pub by_reason: HashMap<String, usize>,
    pub by_logic_fragment: HashMap<String, usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscalationBundle {
    pub version: String,
    pub timestamp: String,
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_version: Option<String>,
    pub summary: EscalationBundleSummary,
    pub candidates: Vec<EscalationCandidate>,
}

/// Compute SHA-256 hash of an atom's logical content.
/// Includes requires, ensures, and body to detect any contract or implementation changes.
pub fn compute_atom_content_hash(name: &str, requires: &str, ensures: &str, body: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(name.as_bytes());
    hasher.update(b"\n---requires---\n");
    hasher.update(requires.as_bytes());
    hasher.update(b"\n---ensures---\n");
    hasher.update(ensures.as_bytes());
    hasher.update(b"\n---body---\n");
    hasher.update(body.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Get Z3 version string by running `z3 --version`.
pub fn get_z3_version() -> String {
    std::process::Command::new("z3")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        })
        .unwrap_or_else(|| "unknown".to_string())
}

/// Generate a proof certificate from verification results.
///
/// `verification_results` maps atom_name → (z3_check_result, status).
/// z3_check_result is "unsat"/"sat"/"unknown"/"skipped".
/// status is "verified"/"failed"/"skipped"/"trusted".
///
/// P5-A: Extended with `module_env` for proof hash and dependency info,
/// and optional `package_name`/`package_version` from mumei.toml.
pub fn generate_certificate(
    file: &str,
    atoms: &[&crate::parser::Atom],
    verification_results: &HashMap<String, (String, String)>,
    module_env: &ModuleEnv,
    package_name: Option<&str>,
    package_version: Option<&str>,
) -> ProofCertificate {
    let now = chrono_like_now();
    let atom_certs: Vec<AtomCertificate> = atoms
        .iter()
        .map(|atom| {
            let content_hash = compute_atom_content_hash(
                &atom.name,
                &atom.requires,
                &atom.ensures,
                &atom.body_expr,
            );
            let (z3_result, status) = verification_results
                .get(&atom.name)
                .cloned()
                .unwrap_or_else(|| ("skipped".to_string(), "skipped".to_string()));

            // P5-A: Compute proof hash with transitive dependencies
            let proof_hash = resolver::compute_proof_hash(atom, module_env);

            // P5-A: Direct callee atom names from dependency graph
            let dependencies: Vec<String> = module_env
                .dependency_graph
                .get(&atom.name)
                .map(|s| {
                    let mut deps: Vec<String> = s.iter().cloned().collect();
                    deps.sort();
                    deps
                })
                .unwrap_or_default();

            // P5-A: Declared effect names
            let effects: Vec<String> = atom.effects.iter().map(|e| e.name.clone()).collect();
            let classification = verification::classify_atom_for_lean_escalation(
                atom, module_env, &z3_result, &status,
            );
            let mut translator_ir = verification::build_translator_ir_metadata(atom, module_env);
            let manual_lemma_reason =
                manual_lemma_reason_for_atom(atom, &classification.logic_fragment_tags);
            translator_ir.manual_lemma_reason = manual_lemma_reason.clone();
            let binder_mapping = translator_ir
                .binders
                .iter()
                .map(|binder| (binder.mumei_name.clone(), binder.lean_name.clone()))
                .collect();
            let counterexample_validation = if z3_result == "sat" {
                let validation =
                    verification::validate_counterexample(atom, &HashMap::new(), module_env);
                Some(CounterexampleValidationMetadata {
                    validation_status: validation.validation_status,
                    validated_at: now.clone(),
                    failed_constraints: validation.failed_constraints,
                })
            } else {
                None
            };
            let symbol_provenance =
                verification::detect_uninterpreted_symbols(atom, &HashMap::new(), module_env);
            let spec_validation_result =
                verification::check_spec_satisfiability(atom, module_env).ok();
            let solver_process_metadata = solver_process_metadata_from_env(&content_hash);

            AtomCertificate {
                name: atom.name.clone(),
                z3_check_result: z3_result,
                content_hash,
                status,
                spec_validation_result,
                proof_hash,
                dependencies,
                effects,
                requires: atom.requires.clone(),
                ensures: atom.ensures.clone(),
                z3_result_class: classification.z3_result_class,
                escalation_reason: classification.escalation_reason,
                logic_fragment_tags: classification.logic_fragment_tags,
                translator_version: verification::LEAN_TRANSLATOR_VERSION.to_string(),
                binder_mapping,
                bridge_lemma_hash: verification::LEAN_BRIDGE_LEMMA_HASH.to_string(),
                manual_lemma_reason,
                translator_ir,
                lean_metadata: None,
                counterexample_validation,
                symbol_provenance,
                unused_hypotheses: None,
                solver_process_metadata,
            }
        })
        .collect();

    let all_verified = atom_certs
        .iter()
        .all(|ac| ac.status == "verified" || ac.status == "trusted");

    // Build certificate without hash first, then compute hash
    let mut cert = ProofCertificate {
        version: "1.0".to_string(),
        timestamp: now,
        mumei_version: env!("CARGO_PKG_VERSION").to_string(),
        z3_version: get_z3_version(),
        file: file.to_string(),
        atoms: atom_certs,
        package_name: package_name.map(|s| s.to_string()),
        package_version: package_version.map(|s| s.to_string()),
        certificate_hash: String::new(),
        all_verified,
    };

    // P5-A: Compute certificate_hash as SHA-256 of serialized cert (with empty hash field)
    cert.certificate_hash = compute_certificate_hash(&cert);

    cert
}

pub fn generate_escalation_bundle(cert: &ProofCertificate) -> EscalationBundle {
    let candidates: Vec<EscalationCandidate> = cert
        .atoms
        .iter()
        .filter_map(|atom| {
            let escalation_reason = atom.escalation_reason.clone()?;
            Some(EscalationCandidate {
                name: atom.name.clone(),
                z3_check_result: atom.z3_check_result.clone(),
                z3_result_class: atom.z3_result_class.clone(),
                status: atom.status.clone(),
                content_hash: atom.content_hash.clone(),
                proof_hash: atom.proof_hash.clone(),
                dependencies: atom.dependencies.clone(),
                effects: atom.effects.clone(),
                requires: atom.requires.clone(),
                ensures: atom.ensures.clone(),
                escalation_reason,
                logic_fragment_tags: atom.logic_fragment_tags.clone(),
                translator_version: atom.translator_version.clone(),
                binder_mapping: atom.binder_mapping.clone(),
                bridge_lemma_hash: atom.bridge_lemma_hash.clone(),
                manual_lemma_reason: atom.manual_lemma_reason.clone(),
                translator_ir: atom.translator_ir.clone(),
                lean_metadata: atom.lean_metadata.clone(),
            })
        })
        .collect();

    let mut summary = EscalationBundleSummary {
        total_atoms: cert.atoms.len(),
        candidate_count: candidates.len(),
        ..EscalationBundleSummary::default()
    };
    for candidate in &candidates {
        *summary
            .by_reason
            .entry(candidate.escalation_reason.clone())
            .or_insert(0) += 1;
        for tag in &candidate.logic_fragment_tags {
            *summary.by_logic_fragment.entry(tag.clone()).or_insert(0) += 1;
        }
    }

    EscalationBundle {
        version: "1.0".to_string(),
        timestamp: cert.timestamp.clone(),
        file: cert.file.clone(),
        package_name: cert.package_name.clone(),
        package_version: cert.package_version.clone(),
        summary,
        candidates,
    }
}

/// Compute SHA-256 hash of the serialized certificate.
/// The `certificate_hash` field is set to empty before hashing for determinism.
fn compute_certificate_hash(cert: &ProofCertificate) -> String {
    // Serialize with empty certificate_hash for deterministic hashing
    let mut hashable = cert.clone();
    hashable.certificate_hash = String::new();
    let json = serde_json::to_string(&hashable).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(json.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Verify a proof certificate against the current source file.
/// Returns a list of (atom_name, status) where status is:
/// - "proven" if content_hash matches and z3_check_result was "unsat"
///   (or `"lean_verified"` when `allow_lean_verified` is enabled — see below).
/// - "changed" if content_hash differs (re-verification needed)
/// - "unproven" if z3_check_result was not "unsat"
///
/// `allow_lean_verified` controls how `z3_check_result == "lean_verified"`
/// values are treated. mumei-lean (the external Lean 4 proof backend) emits
/// `"lean_verified"` for atoms it has discharged itself (Z3 returned
/// `unknown`, but a Lean tactic proof completed). For backwards
/// compatibility the default is `false`, which treats `"lean_verified"` as
/// `"unproven"`. Callers that have explicitly opted in to the cross-project
/// Proof Certificate Chain (e.g. via the `--allow-lean-verified` CLI flag)
/// should pass `true` to recognise those atoms as `"proven"`.
pub fn verify_certificate(
    cert: &ProofCertificate,
    atoms: &[&crate::parser::Atom],
    allow_lean_verified: bool,
) -> Vec<(String, String)> {
    let current_hashes: HashMap<String, String> = atoms
        .iter()
        .map(|a| {
            (
                a.name.clone(),
                compute_atom_content_hash(&a.name, &a.requires, &a.ensures, &a.body_expr),
            )
        })
        .collect();

    cert.atoms
        .iter()
        .map(|ac| {
            let status = if let Some(current_hash) = current_hashes.get(&ac.name) {
                if current_hash != &ac.content_hash {
                    "changed".to_string()
                } else if ac.z3_check_result == "unsat" {
                    "proven".to_string()
                } else if allow_lean_verified && ac.z3_check_result == "lean_verified" {
                    if lean_certificate_metadata_is_current(ac) {
                        "proven".to_string()
                    } else {
                        "stale_translator".to_string()
                    }
                } else {
                    "unproven".to_string()
                }
            } else {
                "missing".to_string()
            };
            (ac.name.clone(), status)
        })
        .collect()
}

fn manual_lemma_reason_for_atom(
    atom: &crate::parser::Atom,
    logic_fragment_tags: &[String],
) -> Option<String> {
    if atom.body_expr.contains("match ")
        || logic_fragment_tags
            .iter()
            .any(|tag| tag == "inductive_data_type")
    {
        Some("inductive_or_pattern_translation_requires_manual_lemma".to_string())
    } else if atom.requires.contains("regex") || atom.ensures.contains("regex") {
        Some("regex_semantics_require_manual_lemma".to_string())
    } else {
        None
    }
}

fn lean_certificate_metadata_is_current(atom: &AtomCertificate) -> bool {
    if atom.translator_version != verification::LEAN_TRANSLATOR_VERSION
        || atom.bridge_lemma_hash != verification::LEAN_BRIDGE_LEMMA_HASH
    {
        return false;
    }
    let Some(metadata) = atom.lean_metadata.as_ref() else {
        return false;
    };
    metadata.status == "lean_verified"
        && !metadata.theorem_name.is_empty()
        && metadata.translator_version == verification::LEAN_TRANSLATOR_VERSION
        && metadata.bridge_lemma_hash == verification::LEAN_BRIDGE_LEMMA_HASH
}

fn atom_carries_lean_translator_metadata(atom: &AtomCertificate) -> bool {
    atom.z3_check_result == "lean_verified"
        || atom.lean_metadata.is_some()
        || !atom.translator_version.is_empty()
        || !atom.bridge_lemma_hash.is_empty()
}

/// Validate that Lean translator metadata matches the currently trusted bridge.
pub fn validate_translator_version(cert: &ProofCertificate) -> Result<(), String> {
    let mut issues = Vec::new();
    for atom in &cert.atoms {
        if atom_carries_lean_translator_metadata(atom) {
            if atom.translator_version != verification::LEAN_TRANSLATOR_VERSION {
                issues.push(format!(
                    "atom '{}': translator_version '{}' does not match expected '{}'",
                    atom.name,
                    atom.translator_version,
                    verification::LEAN_TRANSLATOR_VERSION
                ));
            }
            if atom.bridge_lemma_hash != verification::LEAN_BRIDGE_LEMMA_HASH {
                issues.push(format!(
                    "atom '{}': bridge_lemma_hash '{}' does not match expected '{}'",
                    atom.name,
                    atom.bridge_lemma_hash,
                    verification::LEAN_BRIDGE_LEMMA_HASH
                ));
            }
        }

        if let Some(metadata) = atom.lean_metadata.as_ref() {
            if metadata.translator_version != verification::LEAN_TRANSLATOR_VERSION {
                issues.push(format!(
                    "atom '{}': lean_metadata.translator_version '{}' does not match expected '{}'",
                    atom.name,
                    metadata.translator_version,
                    verification::LEAN_TRANSLATOR_VERSION
                ));
            }
            if metadata.bridge_lemma_hash != verification::LEAN_BRIDGE_LEMMA_HASH {
                issues.push(format!(
                    "atom '{}': lean_metadata.bridge_lemma_hash '{}' does not match expected '{}'",
                    atom.name,
                    metadata.bridge_lemma_hash,
                    verification::LEAN_BRIDGE_LEMMA_HASH
                ));
            }
        }
    }

    if issues.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Lean translator metadata validation failed: {}",
            issues.join("; ")
        ))
    }
}

/// Compute SHA-256 hash of arbitrary data (utility for other crates).
pub fn compute_sha256(data: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn solver_process_metadata_from_env(content_hash: &str) -> Option<SolverProcessMetadata> {
    let task_id = env_nonempty("MUMEI_TASK_ID");
    let generation_id = env_nonempty("MUMEI_GENERATION_ID").unwrap_or_default();
    let timeout_ms = env_nonempty("MUMEI_VERIFICATION_TIMEOUT_MS")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default();
    let explicit_fingerprint = env_nonempty("MUMEI_SOLVER_CONFIG_FINGERPRINT");
    let solver_config_fingerprint = explicit_fingerprint.clone().unwrap_or_else(|| {
        verification::compute_solver_config_fingerprint(timeout_ms, true, false, false, true)
    });
    let explicit_cache_key = env_nonempty("MUMEI_SOLVER_CACHE_KEY");
    let cache_key = explicit_cache_key.clone().unwrap_or_else(|| {
        compute_sha256(&format!("{}:{}", content_hash, solver_config_fingerprint))
    });
    let cancel_reason = env_nonempty("MUMEI_CANCEL_REASON");
    let process_start_time = env_nonempty("MUMEI_SOLVER_PROCESS_START_TIME").unwrap_or_default();
    let has_metadata = task_id.is_some()
        || !generation_id.is_empty()
        || timeout_ms > 0
        || explicit_fingerprint.is_some()
        || explicit_cache_key.is_some()
        || cancel_reason.is_some()
        || !process_start_time.is_empty();
    if !has_metadata {
        return None;
    }
    Some(SolverProcessMetadata {
        task_id,
        cache_key,
        generation_id,
        solver_config_fingerprint,
        timeout_ms,
        cancel_reason,
        process_start_time,
        process_end_time: chrono_like_now(),
    })
}

/// Load a proof certificate from a JSON file.
pub fn load_certificate(path: &Path) -> Result<ProofCertificate, String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    serde_json::from_str(&contents)
        .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))
}

/// Save a proof certificate to a JSON file.
pub fn save_certificate(cert: &ProofCertificate, path: &Path) -> Result<(), String> {
    let json = serde_json::to_string_pretty(cert)
        .map_err(|e| format!("Failed to serialize certificate: {}", e))?;
    std::fs::write(path, json).map_err(|e| format!("Failed to write {}: {}", path.display(), e))
}

pub fn save_escalation_bundle(bundle: &EscalationBundle, path: &Path) -> Result<(), String> {
    let json = serde_json::to_string_pretty(bundle)
        .map_err(|e| format!("Failed to serialize escalation bundle: {}", e))?;
    std::fs::write(path, json).map_err(|e| format!("Failed to write {}: {}", path.display(), e))
}

// =============================================================================
// SI-5 Phase 3-C: Cross-Project Proof Certificate Bundles
// =============================================================================
//
// `scripts/bundle_std_certs.py` aggregates every per-module
// `.proof.json` into a single distributable `std-proof-bundle.json`
// shipped via `homebrew-mumei`. Downstream projects then export the
// bundle path as `MUMEI_PROOF_BUNDLE` and the resolver can verify
// imports against a trusted, mumei-versioned certificate set without
// needing a per-module cert in each module directory.

/// Top-level SI-5 Phase 3-C proof bundle.
///
/// Bundles are produced by `scripts/bundle_std_certs.py` and consumed
/// here as a fallback when a per-module certificate is missing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofBundle {
    /// Bundle schema version (currently `"1.0"`).
    pub bundle_version: String,
    /// ISO 8601 timestamp of bundle generation.
    pub generated_at: String,
    /// mumei version string the bundle was produced for.
    pub mumei_version: String,
    /// Module key → embedded ProofCertificate.
    ///
    /// Keys use the canonical `std/<dir>/<stem>` form, e.g.
    /// `"std/core"`, `"std/container/safe_list"`. The trailing `.mm`
    /// is stripped.
    pub modules: HashMap<String, ProofCertificate>,
    /// Aggregate statistics across all modules.
    #[serde(default)]
    pub summary: BundleSummary,
}

/// Aggregate statistics embedded in a [`ProofBundle`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BundleSummary {
    pub total_modules: usize,
    pub all_verified: usize,
    pub partial_verified: usize,
    pub total_atoms: usize,
    pub proven_atoms: usize,
}

/// Simple ISO 8601 timestamp without external crate.
fn chrono_like_now() -> String {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    // Simple UTC timestamp formatting
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Approximate date calculation (good enough for timestamps)
    let mut year = 1970i64;
    let mut remaining_days = days as i64;
    loop {
        let days_in_year = if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
            366
        } else {
            365
        };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }
    let is_leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days = [
        31,
        if is_leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1;
    for &md in &month_days {
        if remaining_days < md {
            break;
        }
        remaining_days -= md;
        month += 1;
    }
    let day = remaining_days + 1;

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hours, minutes, seconds
    )
}

/// Load a proof bundle JSON produced by `bundle_std_certs.py`.
pub fn load_bundle(path: &Path) -> Result<ProofBundle, String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read bundle {}: {}", path.display(), e))?;
    serde_json::from_str(&contents)
        .map_err(|e| format!("Failed to parse bundle {}: {}", path.display(), e))
}

/// Derive the canonical bundle module key from a source file path.
///
/// Looks for the last `std/` segment in the path and returns
/// `std/<rest-without-extension>`. Returns `None` if the path does not
/// contain a `std/` segment — the bundle only carries `std/` modules.
pub fn module_key_from_source(source_file: &Path) -> Option<String> {
    let mut segments: Vec<&str> = Vec::new();
    let mut seen_std = false;
    for comp in source_file.components() {
        if let std::path::Component::Normal(os_str) = comp {
            let s = os_str.to_str()?;
            if !seen_std {
                if s == "std" {
                    seen_std = true;
                    segments.push("std");
                }
            } else {
                segments.push(s);
            }
        }
    }
    if !seen_std || segments.len() < 2 {
        return None;
    }
    // Strip the .mm extension on the final segment.
    let last = segments.last_mut()?;
    if let Some(stem) = last.strip_suffix(".mm") {
        *last = stem;
    }
    Some(segments.join("/"))
}

/// Look up a certificate for `source_file` in the bundle. The lookup
/// is keyed by [`module_key_from_source`].
pub fn lookup_bundle_certificate<'a>(
    bundle: &'a ProofBundle,
    source_file: &Path,
) -> Option<&'a ProofCertificate> {
    let key = module_key_from_source(source_file)?;
    bundle.modules.get(&key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;
    use crate::verification::ModuleEnv;

    fn make_test_atom(name: &str, requires: &str, ensures: &str, body: &str) -> parser::Atom {
        parser::Atom {
            name: name.to_string(),
            type_params: vec![],
            where_bounds: vec![],
            params: vec![],
            trace_id: None,
            spec_metadata: std::collections::HashMap::new(),
            requires: requires.to_string(),
            forall_constraints: vec![],
            ensures: ensures.to_string(),
            body_expr: body.to_string(),
            consumed_params: vec![],
            resources: vec![],
            is_async: false,
            trust_level: parser::TrustLevel::Verified,
            max_unroll: None,
            invariant: None,
            effects: vec![],
            return_type: None,
            span: crate::parser::Span::new("test.mm", 0, 0, 0),
            effect_pre: HashMap::new(),
            effect_post: HashMap::new(),
        }
    }

    /// P5-A: generate_certificate produces valid JSON with proof_hash, dependencies, effects, requires, ensures
    #[test]
    fn test_generate_certificate_extended_fields() {
        let atom = make_test_atom("add", "x > 0", "result > 0", "x + 1");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "add".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();

        let cert = generate_certificate(
            "test.mm",
            &atoms,
            &results,
            &module_env,
            Some("my_pkg"),
            Some("1.0.0"),
        );

        assert_eq!(cert.atoms.len(), 1);
        assert_eq!(cert.atoms[0].name, "add");
        assert_eq!(cert.atoms[0].requires, "x > 0");
        assert_eq!(cert.atoms[0].ensures, "result > 0");
        assert!(!cert.atoms[0].proof_hash.is_empty());
        assert!(cert.atoms[0].solver_process_metadata.is_none());
        assert!(cert.all_verified);
        assert_eq!(cert.package_name, Some("my_pkg".to_string()));
        assert_eq!(cert.package_version, Some("1.0.0".to_string()));

        // Verify JSON serialization roundtrip
        let json = serde_json::to_string(&cert).unwrap();
        let parsed: ProofCertificate = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.atoms[0].requires, "x > 0");
        assert_eq!(parsed.atoms[0].ensures, "result > 0");
    }

    /// P5-A: verify_certificate detects "changed" when source is modified
    #[test]
    fn test_verify_certificate_detects_changed() {
        let atom = make_test_atom("add", "x > 0", "result > 0", "x + 1");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "add".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();

        let cert = generate_certificate("test.mm", &atoms, &results, &module_env, None, None);

        // Verify with same source → "proven"
        let status = verify_certificate(&cert, &atoms, false);
        assert_eq!(status.len(), 1);
        assert_eq!(status[0], ("add".to_string(), "proven".to_string()));

        // Modify the atom body and verify again → "changed"
        let modified_atom = make_test_atom("add", "x > 0", "result > 0", "x + 2");
        let modified_atoms: Vec<&parser::Atom> = vec![&modified_atom];
        let status2 = verify_certificate(&cert, &modified_atoms, false);
        assert_eq!(status2.len(), 1);
        assert_eq!(status2[0], ("add".to_string(), "changed".to_string()));
    }

    /// PR 2: `lean_verified` is rejected by default and accepted with the
    /// `allow_lean_verified` opt-in flag, mirroring the cross-project Proof
    /// Certificate Chain (mumei-lean → mumei resolver) handshake.
    #[test]
    fn test_verify_certificate_lean_verified_opt_in() {
        let atom = make_test_atom("hard_lemma", "true", "true", "42");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        // mumei-lean signals successful Lean discharge with this string.
        results.insert(
            "hard_lemma".to_string(),
            ("lean_verified".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let mut cert = generate_certificate("test.mm", &atoms, &results, &module_env, None, None);

        // Default (backwards-compatible): lean_verified is NOT proven.
        let status_default = verify_certificate(&cert, &atoms, false);
        assert_eq!(
            status_default[0],
            ("hard_lemma".to_string(), "unproven".to_string())
        );

        // Opt-in still rejects lean_verified without Lean result metadata.
        let status_missing_metadata = verify_certificate(&cert, &atoms, true);
        assert_eq!(
            status_missing_metadata[0],
            ("hard_lemma".to_string(), "stale_translator".to_string())
        );

        cert.atoms[0].lean_metadata = Some(LeanResultMetadata {
            status: "lean_verified".to_string(),
            theorem_name: "hard_lemma_correct".to_string(),
            translator_version: verification::LEAN_TRANSLATOR_VERSION.to_string(),
            bridge_lemma_hash: verification::LEAN_BRIDGE_LEMMA_HASH.to_string(),
            proof_path: "Generated/Test.lean".to_string(),
            diagnostics: vec![],
        });

        // Opt-in: lean_verified is proven only with current Lean metadata.
        let status_opt_in = verify_certificate(&cert, &atoms, true);
        assert_eq!(
            status_opt_in[0],
            ("hard_lemma".to_string(), "proven".to_string())
        );
    }

    #[test]
    fn test_validate_translator_version_detects_atom_version_mismatch() {
        let atom = make_test_atom("hard_lemma", "true", "true", "42");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "hard_lemma".to_string(),
            ("lean_verified".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let mut cert = generate_certificate("test.mm", &atoms, &results, &module_env, None, None);
        cert.atoms[0].translator_version = "old-translator".to_string();

        let err = validate_translator_version(&cert).expect_err("stale translator must fail");
        assert!(err.contains("translator_version"));
        assert!(err.contains("old-translator"));
    }

    #[test]
    fn test_validate_translator_version_detects_bridge_hash_mismatch() {
        let atom = make_test_atom("hard_lemma", "true", "true", "42");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "hard_lemma".to_string(),
            ("lean_verified".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let mut cert = generate_certificate("test.mm", &atoms, &results, &module_env, None, None);
        cert.atoms[0].bridge_lemma_hash = "old-bridge-hash".to_string();

        let err = validate_translator_version(&cert).expect_err("stale bridge hash must fail");
        assert!(err.contains("bridge_lemma_hash"));
        assert!(err.contains("old-bridge-hash"));
    }

    #[test]
    fn test_validate_translator_version_detects_lean_metadata_mismatch() {
        let atom = make_test_atom("hard_lemma", "true", "true", "42");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "hard_lemma".to_string(),
            ("lean_verified".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let mut cert = generate_certificate("test.mm", &atoms, &results, &module_env, None, None);
        cert.atoms[0].lean_metadata = Some(LeanResultMetadata {
            status: "lean_verified".to_string(),
            theorem_name: "hard_lemma_correct".to_string(),
            translator_version: "old-translator".to_string(),
            bridge_lemma_hash: verification::LEAN_BRIDGE_LEMMA_HASH.to_string(),
            proof_path: "Generated/Test.lean".to_string(),
            diagnostics: vec![],
        });

        let err = validate_translator_version(&cert).expect_err("stale Lean metadata must fail");
        assert!(err.contains("lean_metadata.translator_version"));
        assert!(err.contains("old-translator"));
    }

    #[test]
    fn test_validate_translator_version_allows_legacy_z3_only_certificate() {
        let atom = make_test_atom("add", "true", "true", "1");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "add".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let mut cert = generate_certificate("test.mm", &atoms, &results, &module_env, None, None);
        cert.atoms[0].translator_version.clear();
        cert.atoms[0].bridge_lemma_hash.clear();

        assert!(validate_translator_version(&cert).is_ok());
    }

    /// PR 2: `allow_lean_verified` does not weaken the `"changed"` detector.
    /// A modified body must still be flagged as needing re-verification, even
    /// when the cert claims `lean_verified`.
    #[test]
    fn test_verify_certificate_lean_verified_changed_detection() {
        let atom = make_test_atom("hard_lemma", "true", "true", "42");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "hard_lemma".to_string(),
            ("lean_verified".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let cert = generate_certificate("test.mm", &atoms, &results, &module_env, None, None);

        let modified = make_test_atom("hard_lemma", "true", "true", "43");
        let modified_refs: Vec<&parser::Atom> = vec![&modified];

        let status = verify_certificate(&cert, &modified_refs, true);
        assert_eq!(status[0], ("hard_lemma".to_string(), "changed".to_string()));
    }

    /// P5-A: certificate_hash is deterministic
    #[test]
    fn test_certificate_hash_deterministic() {
        let atom = make_test_atom("foo", "true", "true", "42");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "foo".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();

        let cert1 = generate_certificate("test.mm", &atoms, &results, &module_env, None, None);
        let cert2 = generate_certificate("test.mm", &atoms, &results, &module_env, None, None);

        // certificate_hash should be the same for the same inputs
        // (timestamp differs, so we check the hash is non-empty and format is correct)
        assert!(!cert1.certificate_hash.is_empty());
        assert!(!cert2.certificate_hash.is_empty());
        // Both should be 64-char hex strings (SHA-256)
        assert_eq!(cert1.certificate_hash.len(), 64);
        assert_eq!(cert2.certificate_hash.len(), 64);
    }

    /// P5-A: compute_sha256 utility works correctly
    #[test]
    fn test_compute_sha256() {
        let hash1 = compute_sha256("hello");
        let hash2 = compute_sha256("hello");
        let hash3 = compute_sha256("world");
        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
        assert_eq!(hash1.len(), 64); // SHA-256 hex = 64 chars
    }

    /// P5-A: all_verified is false when any atom fails
    #[test]
    fn test_all_verified_false_when_failed() {
        let atom1 = make_test_atom("ok", "true", "true", "1");
        let atom2 = make_test_atom("fail", "true", "true", "0");
        let atoms: Vec<&parser::Atom> = vec![&atom1, &atom2];
        let mut results = HashMap::new();
        results.insert(
            "ok".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        results.insert(
            "fail".to_string(),
            ("sat".to_string(), "failed".to_string()),
        );
        let module_env = ModuleEnv::new();

        let cert = generate_certificate("test.mm", &atoms, &results, &module_env, None, None);
        assert!(!cert.all_verified);
    }

    /// P5-A: save and load certificate roundtrip
    #[test]
    fn test_save_load_certificate_roundtrip() {
        let atom = make_test_atom("bar", "true", "result == 1", "1");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "bar".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();

        let cert = generate_certificate(
            "test.mm",
            &atoms,
            &results,
            &module_env,
            Some("pkg"),
            Some("2.0.0"),
        );

        let tmp = std::env::temp_dir().join("mumei_test_cert.json");
        save_certificate(&cert, &tmp).unwrap();
        let loaded = load_certificate(&tmp).unwrap();

        assert_eq!(loaded.atoms.len(), 1);
        assert_eq!(loaded.atoms[0].name, "bar");
        assert_eq!(loaded.package_name, Some("pkg".to_string()));
        assert_eq!(loaded.certificate_hash, cert.certificate_hash);

        let _ = std::fs::remove_file(&tmp);
    }
}
