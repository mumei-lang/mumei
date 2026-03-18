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

/// Top-level proof certificate for a Mumei source file.
#[derive(Debug, Serialize, Deserialize)]
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
}

/// Per-atom verification certificate.
#[derive(Debug, Serialize, Deserialize)]
pub struct AtomCertificate {
    /// Atom name
    pub name: String,
    /// Z3 solver check result: "unsat" (proven), "sat" (counter-example found), "unknown", "skipped"
    pub z3_check_result: String,
    /// SHA-256 hash of the atom's source text (requires + ensures + body)
    pub content_hash: String,
    /// Verification status: "verified", "failed", "skipped", "trusted"
    pub status: String,
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
pub fn generate_certificate(
    file: &str,
    atoms: &[&crate::parser::Atom],
    verification_results: &HashMap<String, (String, String)>,
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
            AtomCertificate {
                name: atom.name.clone(),
                z3_check_result: z3_result,
                content_hash,
                status,
            }
        })
        .collect();

    ProofCertificate {
        version: "1.0".to_string(),
        timestamp: now,
        mumei_version: env!("CARGO_PKG_VERSION").to_string(),
        z3_version: get_z3_version(),
        file: file.to_string(),
        atoms: atom_certs,
    }
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
