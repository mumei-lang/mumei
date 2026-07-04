use super::models::{EscalationBundle, HumanReviewQueue, ProofBundle, ProofCertificate};
use super::validation::validate_certificate_translator_versions;
use std::path::Path;

pub(crate) fn load_certificate_unvalidated(path: &Path) -> Result<ProofCertificate, String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    serde_json::from_str(&contents)
        .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))
}

/// Load a proof certificate from a JSON file.
pub fn load_certificate(path: &Path) -> Result<ProofCertificate, String> {
    let cert = load_certificate_unvalidated(path)?;
    validate_certificate_translator_versions(&cert)
        .map_err(|e| format!("Failed to validate {}: {}", path.display(), e))?;
    Ok(cert)
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

pub fn save_human_review_queue(queue: &HumanReviewQueue, path: &Path) -> Result<(), String> {
    let json = serde_json::to_string_pretty(queue)
        .map_err(|e| format!("Failed to serialize human review queue: {}", e))?;
    std::fs::write(path, json).map_err(|e| format!("Failed to write {}: {}", path.display(), e))
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
