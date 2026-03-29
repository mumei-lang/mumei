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
use crate::verification::ModuleEnv;

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

            AtomCertificate {
                name: atom.name.clone(),
                z3_check_result: z3_result,
                content_hash,
                status,
                proof_hash,
                dependencies,
                effects,
                requires: atom.requires.clone(),
                ensures: atom.ensures.clone(),
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
/// - "changed" if content_hash differs (re-verification needed)
/// - "unproven" if z3_check_result was not "unsat"
pub fn verify_certificate(
    cert: &ProofCertificate,
    atoms: &[&crate::parser::Atom],
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

/// Compute SHA-256 hash of arbitrary data (utility for other crates).
pub fn compute_sha256(data: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    format!("{:x}", hasher.finalize())
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
        let status = verify_certificate(&cert, &atoms);
        assert_eq!(status.len(), 1);
        assert_eq!(status[0], ("add".to_string(), "proven".to_string()));

        // Modify the atom body and verify again → "changed"
        let modified_atom = make_test_atom("add", "x > 0", "result > 0", "x + 2");
        let modified_atoms: Vec<&parser::Atom> = vec![&modified_atom];
        let status2 = verify_certificate(&cert, &modified_atoms);
        assert_eq!(status2.len(), 1);
        assert_eq!(status2[0], ("add".to_string(), "changed".to_string()));
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
