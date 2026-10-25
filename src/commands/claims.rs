use crate::pipeline::try_load_and_prepare_with_full_options_and_persistence;
use mumei_core::claimbook::{
    attach_controls, create_claimbook, fails_discrimination_gate, limiting_premises_text,
    render_markdown, SealGrade,
};
use mumei_core::parser::Item;
use mumei_core::{controls, parser, proof_cert};
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) struct ClaimsOptions<'a> {
    pub(crate) cert_arg: Option<&'a str>,
    pub(crate) allow_lean_verified: bool,
    pub(crate) format: &'a str,
    pub(crate) out_dir: &'a str,
    pub(crate) controls_enabled: bool,
    pub(crate) max_mutants: usize,
    pub(crate) require_discriminating: bool,
    pub(crate) require_grade: Option<&'a str>,
}

pub(crate) fn cmd_claims(input: &str, options: ClaimsOptions<'_>) {
    let ClaimsOptions {
        cert_arg,
        allow_lean_verified,
        format,
        out_dir,
        controls_enabled,
        max_mutants,
        require_discriminating,
        require_grade,
    } = options;
    let cert_path = cert_arg
        .map(PathBuf::from)
        .unwrap_or_else(|| super::verify::proof_certificate_path(input, None));
    let cert = if cert_path.exists() {
        match proof_cert::load_certificate(&cert_path) {
            Ok(cert) => Some(cert),
            Err(error) => {
                eprintln!("❌ {error}");
                std::process::exit(super::verify::EXIT_INPUT_ERROR);
            }
        }
    } else if let Some(cert_arg) = cert_arg {
        eprintln!("❌ Proof certificate not found: {cert_arg}");
        std::process::exit(super::verify::EXIT_INPUT_ERROR);
    } else {
        None
    };

    let (items, module_env, _imports, _source) =
        match try_load_and_prepare_with_full_options_and_persistence(
            input,
            false,
            allow_lean_verified,
            false,
        ) {
            Ok(loaded) => loaded,
            Err(error) => {
                eprintln!("❌ {error}");
                std::process::exit(super::verify::EXIT_INPUT_ERROR);
            }
        };
    let controls_enabled = controls_enabled || require_discriminating;
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

    let mut book = create_claimbook(
        input,
        cert_path.display().to_string(),
        cert.as_ref(),
        &atoms,
        allow_lean_verified,
    );
    if controls_enabled {
        let report = controls::analyze(input, &items, &items, &module_env, None, max_mutants)
            .unwrap_or_else(|error| {
                eprintln!("❌ Error: {error}");
                std::process::exit(super::verify::EXIT_INPUT_ERROR);
            });
        attach_controls(&mut book, &report.atoms);
    }
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

    let mut rejected = false;
    if let Some(required_grade) = require_grade {
        let required_grade = match required_grade {
            "G0" => SealGrade::G0,
            "G1" => SealGrade::G1,
            "G2" => SealGrade::G2,
            "G3" => SealGrade::G3,
            "G4" => SealGrade::G4,
            "G5" => SealGrade::G5,
            _ => unreachable!("clap restricts required grade values"),
        };
        for claim in book
            .claims
            .iter()
            .filter(|claim| claim.effective_grade < required_grade)
        {
            rejected = true;
            eprintln!(
                "{}: effective {} ({}), required {}; limiting: {}",
                claim.atom,
                grade_code(claim.effective_grade),
                claim.effective_grade.label(),
                grade_code(required_grade),
                limiting_premises_text(&claim.limiting_premises)
            );
        }
    }
    if require_discriminating {
        for claim in &book.claims {
            let Some(discrimination) = &claim.discrimination else {
                continue;
            };
            if fails_discrimination_gate(discrimination) {
                rejected = true;
                let surviving = if discrimination.surviving.is_empty() {
                    "-".to_string()
                } else {
                    discrimination.surviving.join(", ")
                };
                eprintln!(
                    "{}: controls {} (tested {}, killed {}, survived {}, unanalyzable {}); detail: {}; surviving: {}",
                    claim.atom,
                    discrimination.status,
                    discrimination.tested,
                    discrimination.killed,
                    discrimination.survived,
                    discrimination.unanalyzable,
                    discrimination.detail,
                    surviving
                );
            }
        }
    }
    if rejected {
        std::process::exit(super::verify::EXIT_REJECTED);
    }
}

fn grade_code(grade: SealGrade) -> &'static str {
    match grade {
        SealGrade::G0 => "G0",
        SealGrade::G1 => "G1",
        SealGrade::G2 => "G2",
        SealGrade::G3 => "G3",
        SealGrade::G4 => "G4",
        SealGrade::G5 => "G5",
    }
}
