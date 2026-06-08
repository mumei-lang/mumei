//! Spec drift detection via proof-certificate content-hash comparison.
//!
//! Given two [`ProofCertificate`]s (old and new), this module identifies which
//! atoms have been added, removed, or modified by comparing their
//! `content_hash` fields.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};

use crate::proof_cert::ProofCertificate;

/// Report of specification drift between two proof certificates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrossSpecDriftReport {
    /// Atoms whose content_hash changed between old and new certificates.
    pub changed_atoms: Vec<String>,
    /// Atoms present only in the new certificate.
    pub new_atoms: Vec<String>,
    /// Atoms present only in the old certificate.
    pub removed_atoms: Vec<String>,
    /// True if any change was detected.
    pub drift_detected: bool,
}

/// Detect specification drift between old and new proof certificates.
///
/// Compares the `content_hash` of each atom. Atoms that exist in both
/// certificates but differ in hash are classified as "changed". Atoms
/// present only in one certificate are classified as "new" or "removed".
pub fn detect_spec_drift(
    old_cert: &ProofCertificate,
    new_cert: &ProofCertificate,
) -> CrossSpecDriftReport {
    let old_atoms: HashMap<&str, &str> = old_cert
        .atoms
        .iter()
        .map(|a| (a.name.as_str(), a.content_hash.as_str()))
        .collect();

    let new_atoms: HashMap<&str, &str> = new_cert
        .atoms
        .iter()
        .map(|a| (a.name.as_str(), a.content_hash.as_str()))
        .collect();

    let old_names: BTreeSet<&str> = old_atoms.keys().copied().collect();
    let new_names: BTreeSet<&str> = new_atoms.keys().copied().collect();

    let changed_atoms: Vec<String> = old_names
        .intersection(&new_names)
        .filter(|name| old_atoms[*name] != new_atoms[*name])
        .map(|s| s.to_string())
        .collect();

    let new_only: Vec<String> = new_names
        .difference(&old_names)
        .map(|s| s.to_string())
        .collect();

    let removed: Vec<String> = old_names
        .difference(&new_names)
        .map(|s| s.to_string())
        .collect();

    let drift_detected = !changed_atoms.is_empty() || !new_only.is_empty() || !removed.is_empty();

    CrossSpecDriftReport {
        changed_atoms,
        new_atoms: new_only,
        removed_atoms: removed,
        drift_detected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_cert(atoms: &[(&str, &str)]) -> ProofCertificate {
        let json = serde_json::json!({
            "version": "1.0",
            "timestamp": "2024-01-01T00:00:00Z",
            "mumei_version": "0.6.0",
            "z3_version": "4.12.0",
            "file": "test.mm",
            "certificate_hash": "",
            "all_verified": true,
            "atoms": atoms.iter().map(|(name, hash)| {
                serde_json::json!({
                    "name": name,
                    "content_hash": hash,
                    "z3_check_result": "unsat",
                    "status": "verified"
                })
            }).collect::<Vec<_>>()
        });
        serde_json::from_value(json).expect("test cert should deserialize")
    }

    #[test]
    fn test_no_drift() {
        let cert = make_cert(&[("add", "abc123"), ("sub", "def456")]);
        let report = detect_spec_drift(&cert, &cert);
        assert!(!report.drift_detected);
        assert!(report.changed_atoms.is_empty());
        assert!(report.new_atoms.is_empty());
        assert!(report.removed_atoms.is_empty());
    }

    #[test]
    fn test_changed_atom() {
        let old = make_cert(&[("add", "abc123"), ("sub", "def456")]);
        let new = make_cert(&[("add", "changed"), ("sub", "def456")]);
        let report = detect_spec_drift(&old, &new);
        assert!(report.drift_detected);
        assert_eq!(report.changed_atoms, vec!["add"]);
    }

    #[test]
    fn test_new_atom() {
        let old = make_cert(&[("add", "abc123")]);
        let new = make_cert(&[("add", "abc123"), ("mul", "new789")]);
        let report = detect_spec_drift(&old, &new);
        assert!(report.drift_detected);
        assert_eq!(report.new_atoms, vec!["mul"]);
    }

    #[test]
    fn test_removed_atom() {
        let old = make_cert(&[("add", "abc123"), ("deprecated", "old111")]);
        let new = make_cert(&[("add", "abc123")]);
        let report = detect_spec_drift(&old, &new);
        assert!(report.drift_detected);
        assert_eq!(report.removed_atoms, vec!["deprecated"]);
    }

    #[test]
    fn test_combined_drift() {
        let old = make_cert(&[("add", "abc123"), ("old_fn", "hash1")]);
        let new = make_cert(&[("add", "modified"), ("new_fn", "hash2")]);
        let report = detect_spec_drift(&old, &new);
        assert!(report.drift_detected);
        assert_eq!(report.changed_atoms, vec!["add"]);
        assert_eq!(report.new_atoms, vec!["new_fn"]);
        assert_eq!(report.removed_atoms, vec!["old_fn"]);
    }
}
