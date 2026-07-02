use super::generation::compute_atom_content_hash;
use super::models::{AtomCertificate, ProofCertificate};
use super::status;
use crate::verification;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

pub(crate) fn compute_certificate_hash(cert: &ProofCertificate) -> String {
    // Serialize with empty certificate_hash for deterministic hashing
    let mut hashable = cert.clone();
    hashable.certificate_hash = String::new();
    let json = serde_json::to_string(&hashable).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(json.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn refresh_certificate_integrity(cert: &mut ProofCertificate) {
    cert.all_verified = cert.atoms.iter().all(|ac| {
        (ac.status == status::TRUSTED
            || (ac.status == status::VERIFIED
                && (ac.z3_check_result == status::Z3_UNSAT
                    || (ac.z3_check_result == status::Z3_LEAN_VERIFIED
                        && lean_certificate_metadata_is_current(ac)))))
            && ac
                .spec_validation_result
                .as_ref()
                .map(|validation| validation.is_satisfiable)
                .unwrap_or(true)
    });
    cert.certificate_hash = String::new();
    cert.certificate_hash = compute_certificate_hash(cert);
}

pub(crate) fn parse_unsat_core_labels(z3_result: &str) -> Option<Vec<String>> {
    let (_, labels) = z3_result.split_once("unsat_core=")?;
    let labels = labels.trim();
    let labels = labels
        .strip_prefix('[')
        .and_then(|labels| labels.split_once(']').map(|(labels, _)| labels))
        .unwrap_or(labels);
    let labels = labels
        .split(',')
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(|label| label.trim_matches('"').trim_matches('\'').to_string())
        .collect::<Vec<_>>();
    Some(labels)
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
                } else if ac.z3_check_result == status::Z3_UNSAT {
                    "proven".to_string()
                } else if allow_lean_verified && ac.z3_check_result == status::Z3_LEAN_VERIFIED {
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

pub(crate) fn manual_lemma_reason_for_atom(
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
    let Some(metadata) = atom
        .lean_result_metadata
        .as_ref()
        .or(atom.lean_metadata.as_ref())
    else {
        return false;
    };
    metadata.status == status::LEAN_STATUS_VERIFIED
        && !metadata.theorem_name.is_empty()
        && metadata.translator_version == verification::LEAN_TRANSLATOR_VERSION
        && metadata.bridge_lemma_hash == verification::LEAN_BRIDGE_LEMMA_HASH
}

/// Validate that an atom certificate matches the expected Lean translator bridge.
pub fn validate_translator_version(
    cert: &AtomCertificate,
    expected_version: &str,
    expected_hash: &str,
) -> Result<(), String> {
    let mut issues = Vec::new();
    if cert.translator_version != expected_version {
        issues.push(format!(
            "translator_version '{}' does not match expected '{}'",
            cert.translator_version, expected_version
        ));
    }
    if cert.bridge_lemma_hash != expected_hash {
        issues.push(format!(
            "bridge_lemma_hash '{}' does not match expected '{}'",
            cert.bridge_lemma_hash, expected_hash
        ));
    }

    if let Some(metadata) = cert
        .lean_result_metadata
        .as_ref()
        .or(cert.lean_metadata.as_ref())
    {
        if metadata.translator_version != expected_version {
            issues.push(format!(
                "lean_metadata.translator_version '{}' does not match expected '{}'",
                metadata.translator_version, expected_version
            ));
        }
        if metadata.bridge_lemma_hash != expected_hash {
            issues.push(format!(
                "lean_metadata.bridge_lemma_hash '{}' does not match expected '{}'",
                metadata.bridge_lemma_hash, expected_hash
            ));
        }
    }

    if issues.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "atom '{}': Lean translator metadata validation failed: {}",
            cert.name,
            issues.join("; ")
        ))
    }
}

/// Validate that an atom certificate matches the expected Lean translator bridge
/// and carries semantic-gap lowering metadata required by its logic fragment.
pub fn validate_translator_version_with_semantics(
    cert: &AtomCertificate,
    expected_version: &str,
    expected_hash: &str,
    required_lowering_rules: &[String],
) -> Result<(), String> {
    let mut issues = Vec::new();
    if let Err(err) = validate_translator_version(cert, expected_version, expected_hash) {
        issues.push(err);
    }

    let missing_rules: Vec<&String> = required_lowering_rules
        .iter()
        .filter(|rule| !cert.translator_ir.lowering_rules.contains(*rule))
        .collect();
    if !missing_rules.is_empty() {
        issues.push(format!(
            "missing required lowering rules: {}",
            missing_rules
                .iter()
                .map(|rule| rule.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    if issues.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "atom '{}': Lean translator metadata validation failed: {}",
            cert.name,
            issues.join("; ")
        ))
    }
}

/// Get required TranslatorIR lowering rules from logic-fragment tags.
pub fn get_required_lowering_rules(logic_fragment_tags: &[String]) -> Vec<String> {
    let mut rules = vec![
        "type_system_mapping".to_string(),
        "contract_lowering".to_string(),
    ];

    for tag in logic_fragment_tags {
        match tag.as_str() {
            "array_operations" | "array_without_bounds" => {
                rules.push("array_bounds_bridge".to_string());
            }
            "string_operations" => {
                rules.push("string_regex_bridge".to_string());
            }
            "integer_arithmetic" | "nonlinear_arithmetic" => {
                rules.push("integer_overflow_bridge".to_string());
            }
            "quantifiers" | "quantifier_alternation" | "trigger_sensitive_quantifier" => {
                rules.push("refinement_predicate_lowering".to_string());
            }
            "finite_field" => {
                rules.push("finite_field_lowering".to_string());
                rules.push("mathlib4_bridge".to_string());
            }
            "group_theory" => {
                rules.push("group_theory_lowering".to_string());
                rules.push("mathlib4_bridge".to_string());
            }
            _ => {}
        }
    }

    rules.sort();
    rules.dedup();
    rules
}

pub fn validate_certificate_translator_versions(cert: &ProofCertificate) -> Result<(), String> {
    let issues: Vec<String> = cert
        .atoms
        .iter()
        .filter_map(|atom| {
            validate_translator_version(
                atom,
                verification::LEAN_TRANSLATOR_VERSION,
                verification::LEAN_BRIDGE_LEMMA_HASH,
            )
            .err()
        })
        .collect();

    if issues.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Lean translator metadata validation failed: {}",
            issues.join("; ")
        ))
    }
}
