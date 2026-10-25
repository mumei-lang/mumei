use crate::cli::CharterCommand;
use crate::pipeline::try_load_and_prepare_with_full_options_and_persistence;
use mumei_core::claimbook::{create_claimbook, SealGrade};
use mumei_core::parser::Item;
use mumei_core::{charter, manifest, parser, proof_cert};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

pub(crate) fn cmd_charter(command: CharterCommand) {
    let exit_code = match command {
        CharterCommand::Lock { charter, lock } => cmd_lock(&charter, lock.as_deref()),
        CharterCommand::Check {
            input,
            charter,
            lock,
            cert,
            require_grade,
            allow_lean_verified,
            json,
        } => cmd_check(
            &input,
            &charter,
            lock.as_deref(),
            cert.as_deref(),
            require_grade.as_deref(),
            allow_lean_verified,
            json,
        ),
    };
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

fn cmd_lock(charter_args: &[String], lock_arg: Option<&str>) -> i32 {
    let (root, charter_paths, lock_path) = match charter_paths_and_lock(charter_args, lock_arg) {
        Ok(options) => options,
        Err(error) => return input_error(&error),
    };
    match charter::write_lock(&root, &charter_paths, &lock_path) {
        Ok(()) => {
            println!("Charter lock written: {}", root.join(&lock_path).display());
            0
        }
        Err(error) => input_error(&error),
    }
}

fn cmd_check(
    input: &str,
    charter_args: &[String],
    lock_arg: Option<&str>,
    cert_arg: Option<&str>,
    require_grade: Option<&str>,
    allow_lean_verified: bool,
    json: bool,
) -> i32 {
    if require_grade.is_some() && cert_arg.is_none() {
        return input_error("--require-grade requires --cert");
    }
    let (root, charter_paths, lock_path) = match charter_paths_and_lock(charter_args, lock_arg) {
        Ok(options) => options,
        Err(error) => return input_error(&error),
    };

    let cert_path = cert_arg.map(PathBuf::from);
    let cert = if let Some(path) = &cert_path {
        if !path.exists() {
            return input_error(&format!("Proof certificate not found: {}", path.display()));
        }
        match proof_cert::load_certificate(path) {
            Ok(cert) => Some(cert),
            Err(error) => {
                return input_error(&format!(
                    "could not load proof certificate '{}': {error}",
                    path.display()
                ))
            }
        }
    } else {
        None
    };

    let (implementation, module_env, _, _) =
        match try_load_and_prepare_with_full_options_and_persistence(
            input,
            false,
            allow_lean_verified,
            false,
        ) {
            Ok(loaded) => loaded,
            Err(error) => return input_error(&error),
        };
    let generic_implementation =
        match parser::parse_module_checked(&crate::pipeline::load_source(input)) {
            Ok(items) => items,
            Err(errors) => {
                return input_error(&format!(
                    "could not parse implementation '{}': {}",
                    input,
                    errors.join("\n")
                ))
            }
        };
    let (mut report, matched_implementations) =
        match charter::check_charters_with_generics_and_matches(
            &root,
            &charter_paths,
            &lock_path,
            &implementation,
            &generic_implementation,
            &module_env,
        ) {
            Ok(result) => result,
            Err(error) => return input_error(&error),
        };
    if let (Some(cert_path), Some(cert)) = (&cert_path, &cert) {
        let mut atoms: Vec<&parser::Atom> = implementation
            .iter()
            .filter_map(|item| match item {
                Item::Atom(atom) => Some(atom),
                _ => None,
            })
            .collect();
        let mut qualified_methods = Vec::new();
        for item in &implementation {
            if let Item::ImplBlock(block) = item {
                for method in &block.methods {
                    let mut qualified = method.clone();
                    qualified.name = format!("{}::{}", block.struct_name, method.name);
                    qualified_methods.push(qualified);
                }
            }
        }
        atoms.extend(qualified_methods.iter());
        let mut atom_names: HashSet<String> = atoms.iter().map(|atom| atom.name.clone()).collect();
        let mut generic_atoms = Vec::new();
        let mut generic_qualified_methods = Vec::new();
        for item in &generic_implementation {
            match item {
                Item::Atom(atom)
                    if !atom.type_params.is_empty() && atom_names.insert(atom.name.clone()) =>
                {
                    generic_atoms.push(atom);
                }
                Item::ImplBlock(block) => {
                    for method in &block.methods {
                        if method.type_params.is_empty() {
                            continue;
                        }
                        let qualified_name = format!("{}::{}", block.struct_name, method.name);
                        if atom_names.insert(qualified_name.clone()) {
                            let mut qualified = method.clone();
                            qualified.name = qualified_name;
                            generic_qualified_methods.push(qualified);
                        }
                    }
                }
                _ => {}
            }
        }
        atoms.extend(generic_atoms);
        atoms.extend(generic_qualified_methods.iter());
        let book = create_claimbook(
            input,
            cert_path.display().to_string(),
            Some(cert),
            &atoms,
            allow_lean_verified,
            Some(&module_env),
        );
        let required_grade = parse_grade(require_grade.unwrap_or("G4"));
        for (atom, implementation_name) in report.atoms.iter_mut().zip(matched_implementations) {
            if atom.status != "ok" {
                continue;
            }
            let claim = implementation_name
                .as_deref()
                .and_then(|name| book.claims.iter().find(|claim| claim.atom == name));
            let Some(claim) = claim else {
                atom.status = "grade_below".to_string();
                atom.detail = "no claim for implementation atom".to_string();
                continue;
            };
            atom.effective_grade = Some(claim.effective_grade);
            atom.effective_grade_complete = Some(claim.effective_grade_complete);
            if claim.effective_grade < required_grade {
                atom.status = "grade_below".to_string();
                atom.detail = format!(
                    "effective grade {} is below the required {}",
                    grade_code(claim.effective_grade),
                    grade_code(required_grade)
                );
            }
        }
    }
    charter::finalize_charter_report(&mut report);
    let report_bytes = match serde_json::to_vec_pretty(&report) {
        Ok(bytes) => bytes,
        Err(error) => return input_error(&format!("could not serialize charter report: {error}")),
    };
    let report_path = root.join("charter_report.json");
    let mut file_bytes = report_bytes.clone();
    file_bytes.push(b'\n');
    if let Err(error) = fs::write(&report_path, file_bytes) {
        return input_error(&format!(
            "could not write charter report '{}': {error}",
            report_path.display()
        ));
    }

    if json {
        println!("{}", String::from_utf8_lossy(&report_bytes));
    } else {
        let passed = report.status == "pass";
        println!(
            "Charter check {}: {} atom(s); report: {}",
            if passed { "passed" } else { "failed" },
            report.atoms.len(),
            report_path.display()
        );
        for mismatch in &report.lock.mismatches {
            eprintln!("lock {}: {}", mismatch.path, mismatch.detail);
        }
        for atom in &report.atoms {
            if atom.status != "ok" {
                eprintln!("{}: {}: {}", atom.name, atom.status, atom.detail);
                if let Some(counterexample) = &atom.counterexample {
                    eprintln!("  counterexample: {counterexample:?}");
                }
            }
        }
    }

    if report.status == "pass" {
        0
    } else {
        1
    }
}

fn parse_grade(value: &str) -> SealGrade {
    match value {
        "G0" => SealGrade::G0,
        "G1" => SealGrade::G1,
        "G2" => SealGrade::G2,
        "G3" => SealGrade::G3,
        "G4" => SealGrade::G4,
        "G5" => SealGrade::G5,
        _ => unreachable!("clap restricts required grade values"),
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

fn charter_paths_and_lock(
    charter_args: &[String],
    lock_arg: Option<&str>,
) -> Result<(PathBuf, Vec<String>, PathBuf), String> {
    let (root, config) = match find_manifest_for_charter()? {
        Some((root, manifest)) => (root, manifest.charter),
        None => (
            std::env::current_dir()
                .map_err(|error| format!("could not read current directory: {error}"))?,
            None,
        ),
    };
    let root = root.canonicalize().map_err(|error| {
        format!(
            "could not resolve project root '{}': {error}",
            root.display()
        )
    })?;
    let charter_paths = if charter_args.is_empty() {
        config
            .as_ref()
            .map(|config| config.files.clone())
            .unwrap_or_default()
    } else {
        charter_args.to_vec()
    };
    if charter_paths.is_empty() {
        return Err(
            "no charter files configured; pass --charter or add [charter].files".to_string(),
        );
    }
    let lock_path = lock_arg
        .map(PathBuf::from)
        .or_else(|| config.map(|config| PathBuf::from(config.lock)))
        .unwrap_or_else(|| PathBuf::from("charter.lock"));
    Ok((root, charter_paths, lock_path))
}

fn find_manifest_for_charter() -> Result<Option<(PathBuf, manifest::Manifest)>, String> {
    let mut dir = std::env::current_dir()
        .map_err(|error| format!("could not read current directory: {error}"))?;
    loop {
        let path = dir.join("mumei.toml");
        if path.exists() {
            return manifest::load(&path)
                .map(|manifest| Some((dir, manifest)))
                .map_err(|error| format!("could not load manifest: {error}"));
        }
        if !dir.pop() {
            break;
        }
    }
    Ok(None)
}

fn input_error(message: &str) -> i32 {
    eprintln!("charter input error: {message}");
    4
}
