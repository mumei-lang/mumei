use crate::pipeline::*;
use mumei_core::parser::Item;
use mumei_core::{parser, proof_cert};
use std::path::Path;

pub(crate) fn cmd_verify_cert(cert_path: &str, input: &str, allow_lean_verified: bool) {
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
    println!("All verified: {}", cert.all_verified);
    println!(
        "Results: {} proven, {} changed, {} unproven, {} missing",
        proven, changed, unproven, missing
    );

    if changed > 0 {
        println!();
        println!("⚠️  {} atom(s) have changed since certification. Re-run `mumei verify --proof-cert` to update.", changed);
    }
    if unproven > 0 || missing > 0 {
        std::process::exit(1);
    }
}

// =============================================================================
// Emitter dispatch — routes to the correct emitter crate
// =============================================================================
