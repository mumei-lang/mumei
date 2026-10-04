use crate::pipeline::*;
use mumei_core::parser::Item;
use mumei_core::{parser, proof_cert};
use std::path::Path;

pub(crate) fn cmd_verify_cert(
    cert_path: &str,
    input: &str,
    allow_lean_verified: bool,
    strict: bool,
) {
    println!(
        "🔍 Mumei verify-cert: checking '{}' against '{}'...",
        cert_path, input
    );
    if allow_lean_verified {
        println!("  ℹ️  --allow-lean-verified: lean_verified atoms will be accepted as proven");
    }

    let cert = match proof_cert::load_certificate(Path::new(cert_path)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("❌ {}", e);
            std::process::exit(1);
        }
    };

    let (items, _module_env, _imports, _source) =
        load_and_prepare_with_full_options(input, false, allow_lean_verified);

    let mut atom_refs: Vec<&parser::Atom> = items
        .iter()
        .filter_map(|item| {
            if let Item::Atom(a) = item {
                Some(a)
            } else {
                None
            }
        })
        .collect();
    // Also include ImplBlock methods for certificate verification (with qualified names)
    let mut qualified_methods: Vec<parser::Atom> = Vec::new();
    for item in &items {
        if let Item::ImplBlock(ib) = item {
            for method in &ib.methods {
                let mut qualified = method.clone();
                qualified.name = format!("{}::{}", ib.struct_name, method.name);
                qualified_methods.push(qualified);
            }
        }
    }
    for qm in &qualified_methods {
        atom_refs.push(qm);
    }

    let results = proof_cert::verify_certificate(&cert, &atom_refs, allow_lean_verified);

    let mut proven = 0;
    let mut changed = 0;
    let mut unproven = 0;
    let mut axiom_rejected = 0;
    let mut missing = 0;

    for (name, status) in &results {
        let icon = match status.as_str() {
            "proven" => {
                proven += 1;
                "✅"
            }
            "changed" => {
                changed += 1;
                "⚠️"
            }
            "unproven" => {
                unproven += 1;
                "❓"
            }
            "axiom_rejected" => {
                axiom_rejected += 1;
                "❌"
            }
            _ => {
                missing += 1;
                "❌"
            }
        };
        println!("  {} {}: {}", icon, name, status);

        // P5-A: Print extended fields for each atom certificate
        if let Some(ac) = cert.atoms.iter().find(|a| a.name == *name) {
            if !ac.proof_hash.is_empty() {
                println!("      proof_hash: {}", ac.proof_hash);
            }
            if !ac.dependencies.is_empty() {
                println!("      dependencies: [{}]", ac.dependencies.join(", "));
            }
            if !ac.effects.is_empty() {
                println!("      effects: [{}]", ac.effects.join(", "));
            }
            if !ac.requires.is_empty() {
                println!("      requires: {}", ac.requires);
            }
            if !ac.ensures.is_empty() {
                println!("      ensures: {}", ac.ensures);
            }
            if ac.z3_check_result == "lean_verified"
                || ac
                    .lean_result_metadata
                    .as_ref()
                    .or(ac.lean_metadata.as_ref())
                    .is_some()
            {
                let lean_metadata = ac
                    .lean_result_metadata
                    .as_ref()
                    .or(ac.lean_metadata.as_ref());
                if let Some(metadata) = lean_metadata {
                    if let Some(kernel_axioms) = metadata.kernel_axioms.as_ref() {
                        println!("      kernel_axioms: {:?}", kernel_axioms);
                    }
                }
                let audit = match proof_cert::lean_axiom_audit(ac) {
                    proof_cert::LeanAxiomAudit::Passed { .. } => "passed".to_string(),
                    proof_cert::LeanAxiomAudit::Rejected { disallowed } => {
                        format!("rejected (disallowed: {:?})", disallowed)
                    }
                    proof_cert::LeanAxiomAudit::Error => "error".to_string(),
                    proof_cert::LeanAxiomAudit::Unaudited => "unaudited".to_string(),
                };
                println!("      axiom_audit: {}", audit);
            }
        }
    }

    println!();
    // P5-A: Print package metadata if present
    if let Some(ref pkg) = cert.package_name {
        println!(
            "Package: {} v{}",
            pkg,
            cert.package_version.as_deref().unwrap_or("?")
        );
    }
    println!(
        "Certificate: {} (generated {} by mumei v{})",
        cert_path, cert.timestamp, cert.mumei_version
    );
    println!("Certificate hash: {}", cert.certificate_hash);
    // The stored hash is evidence only once it has been re-derived: without
    // this a hand-edited certificate is echoed back unchallenged.
    let mut tampered = false;
    let mut unhashed = false;
    match proof_cert::check_certificate_hash(&cert) {
        proof_cert::CertificateHashCheck::Match => {
            println!("  ✅ certificate_hash recomputed: match");
        }
        proof_cert::CertificateHashCheck::Absent => {
            unhashed = true;
            println!("  ⚠️  certificate_hash absent: integrity cannot be checked");
        }
        proof_cert::CertificateHashCheck::Mismatch {
            recomputed,
            generator_version,
            comparable,
            ..
        } => {
            if comparable {
                tampered = true;
                eprintln!(
                    "  ❌ certificate_hash mismatch: recomputed {}. The certificate was modified after generation.",
                    recomputed
                );
            } else {
                println!(
                    "  ⚠️  certificate_hash mismatch: recomputed {} (certificate written by mumei v{}, this build is v{}). Serialization may differ across versions; re-run `mumei verify --proof-cert` to refresh.",
                    recomputed,
                    generator_version,
                    env!("CARGO_PKG_VERSION")
                );
            }
        }
    }
    println!("All verified: {}", cert.all_verified);
    println!(
        "Results: {} proven, {} changed, {} unproven, {} axiom_rejected, {} missing",
        proven, changed, unproven, axiom_rejected, missing
    );

    if changed > 0 {
        println!();
        println!("⚠️  {} atom(s) have changed since certification. Re-run `mumei verify --proof-cert` to update.", changed);
    }
    let unaudited_lean_atoms: Vec<&str> = if strict && allow_lean_verified {
        results
            .iter()
            .filter_map(|(name, status)| {
                if status != "proven" {
                    return None;
                }
                let atom = cert
                    .atoms
                    .iter()
                    .find(|atom| atom.name == *name && atom.z3_check_result == "lean_verified")?;
                if matches!(
                    proof_cert::lean_axiom_audit(atom),
                    proof_cert::LeanAxiomAudit::Unaudited
                ) {
                    Some(name.as_str())
                } else {
                    None
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    let strict_unaudited_failure = !unaudited_lean_atoms.is_empty();
    let strict_failure = strict && (changed > 0 || unhashed || strict_unaudited_failure);
    if strict_unaudited_failure {
        eprintln!(
            "❌ --strict: --allow-lean-verified accepted unaudited Lean atom(s): {}. Re-run mumei-lean to attach kernel axiom audit metadata.",
            unaudited_lean_atoms.join(", ")
        );
    }
    if strict && (changed > 0 || unhashed) {
        println!();
        eprintln!(
            "❌ --strict: certificate '{}' no longer matches '{}' ({} changed atom(s), certificate_hash {}).",
            cert_path,
            input,
            changed,
            if unhashed { "absent" } else { "present" }
        );
    }
    if tampered || strict_failure || unproven > 0 || axiom_rejected > 0 || missing > 0 {
        std::process::exit(1);
    }
}

// =============================================================================
// Emitter dispatch — routes to the correct emitter crate
// =============================================================================
