use crate::pipeline::{read_source_file, try_load_and_prepare_with_full_options_and_persistence};
use mumei_core::{erosion, parser};

pub(crate) fn cmd_erosion(candidate: &str, baseline: &str, json: bool) -> i32 {
    let (candidate_items, module_env, _, _) =
        match try_load_and_prepare_with_full_options_and_persistence(candidate, false, false, false)
        {
            Ok(loaded) => loaded,
            Err(error) => return input_error(&error),
        };
    let candidate_source = match read_source_file(candidate) {
        Ok(source) => source,
        Err(error) => {
            return input_error(&format!("could not read candidate '{candidate}': {error}"))
        }
    };
    let generic_candidate = match parser::parse_module_checked(&candidate_source) {
        Ok(items) => items,
        Err(errors) => {
            return input_error(&format!(
                "could not parse candidate '{candidate}': {}",
                errors.join("\n")
            ))
        }
    };
    let baseline_source = match read_source_file(baseline) {
        Ok(source) => source,
        Err(error) => {
            return input_error(&format!("could not read baseline '{baseline}': {error}"))
        }
    };
    let baseline_items = match parser::parse_module_checked(&baseline_source) {
        Ok(items) => items,
        Err(errors) => {
            return input_error(&format!(
                "could not parse baseline '{baseline}': {}",
                errors.join("\n")
            ))
        }
    };

    let report = erosion::compare_contracts(
        baseline,
        candidate,
        &baseline_items,
        &candidate_items,
        &generic_candidate,
        &module_env,
    );
    if json {
        match serde_json::to_string(&report) {
            Ok(output) => println!("{output}"),
            Err(error) => {
                return input_error(&format!("could not serialize erosion report: {error}"))
            }
        }
    } else {
        println!(
            "Contract erosion {}: {} checked, {} eroded, {} added.",
            report.status,
            report.totals.checked,
            report.totals.eroded,
            report.added.len()
        );
        for atom in report.atoms.iter().filter(|atom| {
            matches!(
                atom.status.as_str(),
                "pre_weakened"
                    | "post_weakened"
                    | "missing"
                    | "signature_mismatch"
                    | "inconclusive"
            )
        }) {
            println!("  {}: {} ({})", atom.name, atom.status, atom.detail);
        }
    }

    if report.status == "pass" {
        0
    } else {
        1
    }
}

fn input_error(message: &str) -> i32 {
    eprintln!("erosion input error: {message}");
    4
}
