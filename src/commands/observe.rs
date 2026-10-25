use crate::pipeline::{read_source_file, try_load_and_prepare_with_full_options_and_persistence};
use mumei_core::{observe, parser};
use std::fs;

pub(crate) fn cmd_observe(input: &str, harvest: &str, json: bool, atom: Option<&str>) -> i32 {
    let (implementation, module_env, _, _) =
        match try_load_and_prepare_with_full_options_and_persistence(input, false, false, false) {
            Ok(loaded) => loaded,
            Err(error) => return input_error(&error),
        };
    let source = match read_source_file(input) {
        Ok(source) => source,
        Err(error) => return input_error(&format!("could not read '{input}': {error}")),
    };
    let generic_implementation = match parser::parse_module_checked(&source) {
        Ok(items) => items,
        Err(errors) => {
            return input_error(&format!(
                "could not parse implementation '{input}': {}",
                errors.join("\n")
            ))
        }
    };
    let harvest_json = match fs::read_to_string(harvest) {
        Ok(content) => content,
        Err(error) => return input_error(&format!("could not read harvest '{harvest}': {error}")),
    };
    let report = match observe::analyze(
        input,
        harvest,
        &harvest_json,
        &implementation,
        &generic_implementation,
        &module_env,
        atom,
    ) {
        Ok(report) => report,
        Err(error) => return input_error(&error),
    };
    if json {
        match report.json() {
            Ok(output) => println!("{output}"),
            Err(error) => return input_error(&error.to_string()),
        }
    } else {
        println!("{}", report.human());
    }
    if report.totals["contradicts_ensures"] > 0 || report.totals["requires_admits_rejected"] > 0 {
        super::verify::EXIT_REJECTED
    } else {
        super::verify::EXIT_VERIFIED
    }
}

fn input_error(message: &str) -> i32 {
    eprintln!("❌ Error: {message}");
    super::verify::EXIT_INPUT_ERROR
}
