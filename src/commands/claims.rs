use crate::pipeline::try_load_and_prepare_with_full_options_and_persistence;
use mumei_core::claimbook::{create_claimbook, render_markdown};
use mumei_core::parser::Item;
use mumei_core::{parser, proof_cert};
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) fn cmd_claims(
    input: &str,
    cert_arg: Option<&str>,
    allow_lean_verified: bool,
    format: &str,
    out_dir: &str,
) {
    let cert_path = cert_arg
        .map(PathBuf::from)
        .unwrap_or_else(|| super::verify::proof_certificate_path(input, None));
    let cert = if cert_path.exists() {
        match proof_cert::load_certificate(&cert_path) {
            Ok(cert) => Some(cert),
            Err(error) => {
                eprintln!("❌ {error}");
                std::process::exit(1);
            }
        }
    } else if let Some(cert_arg) = cert_arg {
        eprintln!("❌ Proof certificate not found: {cert_arg}");
        std::process::exit(1);
    } else {
        None
    };

    let (items, _module_env, _imports, _source) =
        match try_load_and_prepare_with_full_options_and_persistence(
            input,
            false,
            allow_lean_verified,
            false,
        ) {
            Ok(loaded) => loaded,
            Err(error) => {
                eprintln!("❌ {error}");
                std::process::exit(1);
            }
        };
    let mut atoms: Vec<&parser::Atom> = items
        .iter()
        .filter_map(|item| {
            if let Item::Atom(atom) = item {
                Some(atom)
            } else {
                None
            }
        })
        .collect();
    let mut qualified_methods = Vec::new();
    for item in &items {
        if let Item::ImplBlock(implementation) = item {
            for method in &implementation.methods {
                let mut qualified = method.clone();
                qualified.name = format!("{}::{}", implementation.struct_name, method.name);
                qualified_methods.push(qualified);
            }
        }
    }
    atoms.extend(qualified_methods.iter());

    let book = create_claimbook(
        input,
        cert_path.display().to_string(),
        cert.as_ref(),
        &atoms,
        allow_lean_verified,
    );
    let output_dir = Path::new(out_dir);
    if let Err(error) = fs::create_dir_all(output_dir) {
        eprintln!(
            "❌ Could not create output directory '{}': {error}",
            out_dir
        );
        std::process::exit(super::verify::EXIT_INTERNAL_ERROR);
    }

    if format == "json" || format == "both" {
        let path = output_dir.join("claimbook.json");
        let json = match serde_json::to_string_pretty(&book) {
            Ok(json) => json,
            Err(error) => {
                eprintln!("❌ Could not serialize Claim Book: {error}");
                std::process::exit(super::verify::EXIT_INTERNAL_ERROR);
            }
        };
        if let Err(error) = fs::write(&path, json) {
            eprintln!("❌ Could not write '{}': {error}", path.display());
            std::process::exit(super::verify::EXIT_INTERNAL_ERROR);
        }
    }
    if format == "md" || format == "both" {
        let path = output_dir.join("CLAIMBOOK.md");
        if let Err(error) = fs::write(&path, render_markdown(&book)) {
            eprintln!("❌ Could not write '{}': {error}", path.display());
            std::process::exit(super::verify::EXIT_INTERNAL_ERROR);
        }
    }

    let grades = book
        .summary
        .by_effective_grade
        .iter()
        .map(|(grade, count)| format!("{grade:?}({})={count}", grade.label()))
        .collect::<Vec<_>>()
        .join(", ");
    let freshness = book
        .summary
        .by_freshness
        .iter()
        .map(|(state, count)| format!("{state:?}={count}").to_lowercase())
        .collect::<Vec<_>>()
        .join(", ");
    println!(
        "Claim Book: {} atoms; effective grades: {}; freshness: {}",
        book.summary.total, grades, freshness
    );
}
