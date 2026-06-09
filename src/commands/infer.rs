use crate::pipeline::*;
use mumei_core::verification;
use std::path::Path;

pub(crate) fn cmd_infer_effects(input: &str) {
    let input_path = Path::new(input);
    if input_path.is_dir() {
        let mut files = collect_mm_files(input_path);
        files.sort();
        if files.is_empty() {
            eprintln!("❌ No .mm files found in '{}'", input);
            std::process::exit(1);
        }
        let mut entries: Vec<serde_json::Value> = Vec::new();
        for file in &files {
            let file_str = file.to_string_lossy().to_string();
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                try_load_and_prepare(&file_str).map(|(items, module_env, _imports, _source)| {
                    verification::infer_effects_json(&items, &module_env)
                })
            })) {
                Ok(Ok(result)) => {
                    entries.push(serde_json::json!({
                        "file": file_str,
                        "result": result,
                    }));
                }
                Ok(Err(error)) => {
                    entries.push(serde_json::json!({
                        "file": file_str,
                        "result": null,
                        "error": error,
                    }));
                }
                Err(_) => {
                    entries.push(serde_json::json!({
                        "file": file_str,
                        "result": null,
                        "error": "parse error",
                    }));
                }
            }
        }
        println!("{}", serde_json::to_string_pretty(&entries).unwrap());
    } else {
        let (items, module_env, _imports, _source) = load_and_prepare(input);
        let result = verification::infer_effects_json(&items, &module_env);
        println!("{}", serde_json::to_string_pretty(&result).unwrap());
    }
}

/// Plan 13-3: CLI command for contract inference
pub(crate) fn cmd_infer_contracts(input: &str) {
    let input_path = Path::new(input);
    if input_path.is_dir() {
        let mut files = collect_mm_files(input_path);
        files.sort();
        if files.is_empty() {
            eprintln!("❌ No .mm files found in '{}'", input);
            std::process::exit(1);
        }
        let mut entries: Vec<serde_json::Value> = Vec::new();
        for file in &files {
            let file_str = file.to_string_lossy().to_string();
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                try_load_and_prepare(&file_str).map(|(items, module_env, _imports, _source)| {
                    verification::infer_contracts_json(&items, &module_env)
                })
            })) {
                Ok(Ok(result)) => {
                    entries.push(serde_json::json!({
                        "file": file_str,
                        "result": result,
                    }));
                }
                Ok(Err(error)) => {
                    entries.push(serde_json::json!({
                        "file": file_str,
                        "result": null,
                        "error": error,
                    }));
                }
                Err(_) => {
                    entries.push(serde_json::json!({
                        "file": file_str,
                        "result": null,
                        "error": "parse error",
                    }));
                }
            }
        }
        println!("{}", serde_json::to_string_pretty(&entries).unwrap());
    } else {
        let (items, module_env, _imports, _source) = load_and_prepare(input);
        let result = verification::infer_contracts_json(&items, &module_env);
        println!("{}", serde_json::to_string_pretty(&result).unwrap());
    }
}

// =============================================================================
// mumei doc — Generate documentation from source comments
// =============================================================================
