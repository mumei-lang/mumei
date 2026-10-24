use crate::cli::CharterCommand;
use crate::pipeline::try_load_and_prepare_with_full_options_and_persistence;
use mumei_core::{charter, manifest};
use std::fs;
use std::path::PathBuf;

pub(crate) fn cmd_charter(command: CharterCommand) {
    let exit_code = match command {
        CharterCommand::Lock { charter, lock } => cmd_lock(&charter, lock.as_deref()),
        CharterCommand::Check {
            input,
            charter,
            lock,
            json,
        } => cmd_check(&input, &charter, lock.as_deref(), json),
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

fn cmd_check(input: &str, charter_args: &[String], lock_arg: Option<&str>, json: bool) -> i32 {
    let (root, charter_paths, lock_path) = match charter_paths_and_lock(charter_args, lock_arg) {
        Ok(options) => options,
        Err(error) => return input_error(&error),
    };

    let (implementation, module_env, _, _) =
        match try_load_and_prepare_with_full_options_and_persistence(input, false, false, false) {
            Ok(loaded) => loaded,
            Err(error) => return input_error(&error),
        };
    let report = match charter::check_charters(
        &root,
        &charter_paths,
        &lock_path,
        &implementation,
        &module_env,
    ) {
        Ok(report) => report,
        Err(error) => return input_error(&error),
    };
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
        let passed =
            report.lock.status == "ok" && report.atoms.iter().all(|atom| atom.status == "ok");
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

    if report.lock.status == "ok" && report.atoms.iter().all(|atom| atom.status == "ok") {
        0
    } else {
        1
    }
}

fn charter_paths_and_lock(
    charter_args: &[String],
    lock_arg: Option<&str>,
) -> Result<(PathBuf, Vec<String>, PathBuf), String> {
    let (root, config) = match manifest::find_and_load() {
        Some((root, manifest)) => (root, manifest.charter),
        None => (
            std::env::current_dir()
                .map_err(|error| format!("could not read current directory: {error}"))?,
            None,
        ),
    };
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

fn input_error(message: &str) -> i32 {
    eprintln!("charter input error: {message}");
    4
}
