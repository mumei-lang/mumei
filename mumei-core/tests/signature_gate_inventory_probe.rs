//! Measurement probe: rerun the signature-gate census over the repo's `.mm`
//! corpus after the omitted-return inference change (#1411). For every file
//! we run `extract_pure_i64_module_contracts` under the widest admission
//! (`CompositeV29`) and bucket the `SemanticContractError::Unsupported`
//! reason so the remaining blockers can be counted.

use mumei_core::semantic_contract::{
    extract_pure_i64_module_contracts, RefinedAdmission, SemanticContractError,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The census corpus: every `.mm` file git tracks under the repo root, so a
/// rerun against the same commit sees the same inputs. Falls back to a
/// filesystem walk when `git ls-files` is unavailable.
fn collect_mm_files(root: &Path) -> Vec<PathBuf> {
    if let Ok(output) = std::process::Command::new("git")
        .args(["-C", &*root.to_string_lossy(), "ls-files", "--", "*.mm"])
        .output()
    {
        if output.status.success() {
            let mut files: Vec<PathBuf> = String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter(|line| !line.is_empty())
                .map(|line| root.join(line))
                .collect();
            files.sort();
            if !files.is_empty() {
                return files;
            }
        }
    }
    let mut files = Vec::new();
    collect_mm_files_walk(root, &mut files);
    files
}

fn collect_mm_files_walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().map(|n| n == "target").unwrap_or(false) {
                continue;
            }
            collect_mm_files_walk(&path, out);
        } else if path.extension().map(|e| e == "mm").unwrap_or(false) {
            out.push(path);
        }
    }
}

#[test]
fn signature_gate_inventory() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = collect_mm_files(&root);
    files.sort();

    let mut admitted = 0usize;
    let mut buckets: BTreeMap<String, usize> = BTreeMap::new();
    let mut examples: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut non_unsupported = 0usize;
    let mut unreadable = 0usize;

    for path in &files {
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => {
                unreadable += 1;
                continue;
            }
        };
        match extract_pure_i64_module_contracts(
            &source,
            &path.to_string_lossy(),
            None,
            RefinedAdmission::CompositeV29,
        ) {
            Ok(_) => admitted += 1,
            Err(SemanticContractError::Unsupported(reason)) => {
                *buckets.entry(reason.to_string()).or_default() += 1;
                let list = examples.entry(reason.to_string()).or_default();
                if list.len() < 5 {
                    list.push(
                        path.strip_prefix(&root)
                            .unwrap_or(path)
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
            }
            Err(_) => non_unsupported += 1,
        }
    }

    eprintln!("files scanned: {}", files.len());
    eprintln!("unreadable (excluded): {}", unreadable);
    eprintln!("admitted: {}", admitted);
    eprintln!("non-unsupported errors: {}", non_unsupported);
    for (reason, count) in &buckets {
        eprintln!("{count:5}  {reason}");
        for ex in &examples[reason] {
            eprintln!("        {ex}");
        }
    }

    // Sharpen the signature bucket: among the `explicit owned scalar i64
    // signature required` files, how many would still fail even if every
    // omitted return inferred to `i64` (i.e. the blocker is NOT the omitted
    // return)? For each rejected file we replay the `atom_return_types`
    // fixpoint: a file is "omitted-return-shaped" when every atom's declared
    // return type is either `None` or `i64`, and "omitted-resolved" when the
    // fixpoint additionally proves `i64` for every omitted atom — there the
    // residual blocker is some other feature, not inference.
    let mut omitted_only = 0usize;
    let mut omitted_resolved_files = 0usize;
    let mut other_signature = 0usize;
    let mut omitted_atoms = 0usize;
    let mut uninferable = 0usize;
    let mut non_i64_inferred = 0usize;
    let mut resolved_i64 = 0usize;
    for path in &files {
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let Ok(items) = mumei_core::parser::parse_module_checked(&source) else {
            continue;
        };
        let atoms: Vec<_> = items
            .iter()
            .filter_map(|item| match item {
                mumei_core::parser::Item::Atom(atom) => Some(atom),
                _ => None,
            })
            .collect();
        if atoms.is_empty() {
            continue;
        }
        if !matches!(
            extract_pure_i64_module_contracts(
                &source,
                &path.to_string_lossy(),
                None,
                RefinedAdmission::CompositeV29,
            ),
            Err(SemanticContractError::Unsupported(reason))
                if reason == "explicit owned scalar i64 signature required"
        ) {
            continue;
        }
        // Fixpoint over declared + inferred returns, mirroring
        // `semantic_contract::atom_return_types`.
        let mut map: std::collections::HashMap<String, String> = atoms
            .iter()
            .filter_map(|a| a.return_type.clone().map(|t| (a.name.clone(), t)))
            .collect();
        let mut changed = true;
        while changed {
            changed = false;
            for a in &atoms {
                if map.contains_key(&a.name) {
                    continue;
                }
                if let Some(t) = mumei_core::mir::infer_atom_return_type_with(a, &map) {
                    map.insert(a.name.clone(), t);
                    changed = true;
                }
            }
        }
        let file_omitted: Vec<_> = atoms.iter().filter(|a| a.return_type.is_none()).collect();
        if atoms
            .iter()
            .all(|a| a.return_type.is_none() || a.return_type.as_deref() == Some("i64"))
        {
            omitted_only += 1;
            if file_omitted
                .iter()
                .all(|a| map.get(&a.name).map(|t| t.as_str()) == Some("i64"))
            {
                omitted_resolved_files += 1;
            }
            if omitted_only <= 12 {
                for a in &file_omitted {
                    let tail = a.body_expr.trim().chars().take(90).collect::<String>();
                    eprintln!(
                        "        OMITTED {} :: {} -> body: {tail}",
                        path.strip_prefix(&root).unwrap_or(path).display(),
                        a.name
                    );
                }
            }
        } else {
            other_signature += 1;
        }
        for a in &file_omitted {
            omitted_atoms += 1;
            match map.get(&a.name) {
                Some(t) if t == "i64" => resolved_i64 += 1,
                Some(_) => non_i64_inferred += 1,
                None => uninferable += 1,
            }
        }
    }
    eprintln!("signature-rejection detail: omitted-return-shaped {omitted_only} (of which fully-inferred-to-i64 {omitted_resolved_files}), other-signature {other_signature}");
    eprintln!("omitted-return atoms in rejected files: {omitted_atoms} (inferred i64 {resolved_i64}, inferred non-i64 {non_i64_inferred}, uninferable {uninferable})");
}
