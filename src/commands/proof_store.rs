use crate::cli::ProofStoreCommand;
use mumei_core::resolver::ProofStore;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

fn entries(root: &Path) -> Vec<PathBuf> {
    fn visit(path: &Path, result: &mut Vec<PathBuf>) {
        let Ok(children) = std::fs::read_dir(path) else {
            return;
        };
        for child in children.flatten() {
            let path = child.path();
            if path.is_dir() {
                visit(&path, result);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                result.push(path);
            }
        }
    }
    let mut result = Vec::new();
    visit(&root.join("v1"), &mut result);
    result
}

pub(crate) fn cmd_proof_store(command: ProofStoreCommand) {
    match command {
        ProofStoreCommand::Stats { dir, json } => {
            let store = match ProofStore::open(dir.as_deref().map(Path::new)) {
                Ok(store) => store,
                Err(error) => {
                    eprintln!("proof store unavailable: {error}");
                    std::process::exit(1);
                }
            };
            let files = entries(store.root());
            let bytes: u64 = files
                .iter()
                .filter_map(|path| path.metadata().ok())
                .map(|m| m.len())
                .sum();
            let invalid = files
                .iter()
                .filter(|path| store.validate_file(path).is_err())
                .count();
            if json {
                println!(
                    "{}",
                    serde_json::json!({"entries": files.len(), "total_bytes": bytes, "invalid": invalid})
                );
            } else {
                println!(
                    "entries: {}\ntotal bytes: {}\ninvalid: {}",
                    files.len(),
                    bytes,
                    invalid
                );
            }
        }
        ProofStoreCommand::Prune {
            dir,
            older_than_days,
            invalid,
        } => {
            if older_than_days.is_none() && !invalid {
                eprintln!("proof-store prune requires --older-than-days or --invalid");
                std::process::exit(4);
            }
            let store = match ProofStore::open(dir.as_deref().map(Path::new)) {
                Ok(store) => store,
                Err(error) => {
                    eprintln!("proof store unavailable: {error}");
                    std::process::exit(1);
                }
            };
            let age = older_than_days.map(|days| Duration::from_secs(days.saturating_mul(86_400)));
            let now = SystemTime::now();
            let mut removed = 0;
            for path in entries(store.root()) {
                let is_invalid = store.validate_file(&path).is_err();
                let is_old = age.is_some_and(|age| {
                    path.metadata()
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .is_some_and(|modified| {
                            now.duration_since(modified).unwrap_or_default() >= age
                        })
                });
                if ((invalid && is_invalid) || is_old) && std::fs::remove_file(path).is_ok() {
                    removed += 1;
                }
            }
            println!("removed: {removed}");
        }
    }
}
