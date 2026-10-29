use crate::pipeline::{read_source_file, try_load_and_prepare_with_full_options_and_persistence};
use mumei_core::{controls, parser};

pub(crate) fn cmd_controls(input: &str, json: bool, max_mutants: usize, atom: Option<&str>) -> i32 {
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
    let report = match controls::analyze(
        input,
        &implementation,
        &generic_implementation,
        &module_env,
        atom,
        max_mutants,
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
    if report.status == "fail" {
        super::verify::EXIT_REJECTED
    } else {
        super::verify::EXIT_VERIFIED
    }
}

fn input_error(message: &str) -> i32 {
    eprintln!("❌ Error: {message}");
    super::verify::EXIT_INPUT_ERROR
}
