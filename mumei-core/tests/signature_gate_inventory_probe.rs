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

fn collect_mm_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().map(|n| n == "target").unwrap_or(false) {
                continue;
            }
            collect_mm_files(&path, out);
        } else if path.extension().map(|e| e == "mm").unwrap_or(false) {
            out.push(path);
        }
    }
}

#[test]
fn signature_gate_inventory() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    collect_mm_files(&root, &mut files);
    files.sort();

    let mut admitted = 0usize;
    let mut buckets: BTreeMap<String, usize> = BTreeMap::new();
    let mut examples: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut non_unsupported = 0usize;

    for path in &files {
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => continue,
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
    // return)? Parse each file; a file is "omitted-return-blocked" when every
    // atom's declared return type is either `None` or `i64`.
    let mut omitted_only = 0usize;
    let mut other_signature = 0usize;
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
        match extract_pure_i64_module_contracts(
            &source,
            &path.to_string_lossy(),
            None,
            RefinedAdmission::CompositeV29,
        ) {
            Err(SemanticContractError::Unsupported(reason))
                if reason == "explicit owned scalar i64 signature required" =>
            {
                if atoms
                    .iter()
                    .all(|a| a.return_type.is_none() || a.return_type.as_deref() == Some("i64"))
                {
                    omitted_only += 1;
                    if omitted_only <= 12 {
                        for a in atoms.iter().filter(|a| a.return_type.is_none()) {
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
            }
            _ => (),
        }
    }
    eprintln!("signature-rejection detail: omitted-return-blocked {omitted_only}, other-signature {other_signature}");

    // Atom-level residual: over the same 72-file bucket, build the
    // fixpoint callee-return map exactly as `atom_return_types` does and
    // count the atoms whose omitted return still cannot infer to `i64`.
    let mut omitted_atoms = 0usize;
    let mut uninferable = 0usize;
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
        // Fixpoint over declared + inferred returns.
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
        for a in atoms.iter().filter(|a| a.return_type.is_none()) {
            omitted_atoms += 1;
            match map.get(&a.name) {
                Some(t) if t == "i64" => resolved_i64 += 1,
                _ => {
                    uninferable += 1;
                }
            }
        }
    }
    eprintln!("omitted-return atoms in rejected files: {omitted_atoms} (inferred i64 {resolved_i64}, still uninferable {uninferable})");
}
