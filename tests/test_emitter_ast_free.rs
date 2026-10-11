//! Step 7 of the HIR emitter migration: emitter-facing code must not reach
//! the AST. `Emitter::emit` receives `&EmitAtom<'_>` (HIR-only), so no
//! `mumei-emit-*` production source file may read `HirAtom.atom` /
//! `HirAtom.body_stmt` — under any binding name (`renamed.atom`,
//! `x.body_stmt`) — or reference the parser types that HIR now owns (`Op`,
//! `Pattern`, `JoinSemantics`). `parser::ExternBlock`/`EnumDef`/`StructDef`
//! remain allowed — they arrive via `ModuleEnv` and the emit signature, not
//! via the atom.
//!
//! Test modules are excluded: `#[cfg(test)]` fixtures legitimately build
//! `Atom`/`HirAtom` values to feed the emitters. The one production-side
//! `.atom` field access that is not `HirAtom.atom` is the generated-monitor
//! `Violation` runtime struct's field (`violation.atom` inside the emitted
//! monitor source template), which is whitelisted explicitly.

use std::path::{Path, PathBuf};

const EMIT_CRATES: &[&str] = &[
    "mumei-emit-llvm",
    "mumei-emit-json",
    "mumei-emit-proofbook",
    "mumei-emit-monitor",
    "mumei-emit-python",
    "mumei-emit-rust",
];

/// Parser-type references forbidden anywhere in production emitter code.
const FORBIDDEN: &[&str] = &[
    "parser::Op",
    "parser::Pattern",
    "parser::JoinSemantics",
    "parser::ast::Op",
    "parser::ast::Pattern",
    "parser::ast::JoinSemantics",
];

/// `.atom` / `.body_stmt` field access is checked per occurrence so a read
/// through a renamed binding (`let x = hir_atom; x.atom`) is still caught.
const FORBIDDEN_FIELDS: &[&str] = &[".atom", ".body_stmt"];

/// Receiver immediately preceding `.atom` that is NOT a `HirAtom`: the
/// emitted monitor template's `Violation` struct carries an `atom` field.
const ALLOWED_ATOM_RECEIVER: &str = "violation";

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read emit crate dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Test modules may construct `Atom`/`HirAtom` fixtures; only the production
/// part of each file (everything before `#[cfg(test)]`) is scanned.
fn production_part(text: &str) -> &str {
    text.split("#[cfg(test)]").next().unwrap_or(text)
}

fn offending_field_accesses(text: &str) -> Vec<String> {
    let mut hits = Vec::new();
    for field in FORBIDDEN_FIELDS {
        for (idx, _) in text.match_indices(field) {
            let receiver = text[..idx]
                .chars()
                .rev()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect::<String>();
            let receiver: String = receiver.chars().rev().collect();
            if *field == ".atom" && receiver == ALLOWED_ATOM_RECEIVER {
                continue;
            }
            hits.push(format!("{receiver}{field} (offset {idx})"));
        }
    }
    hits
}

#[test]
fn emit_crates_do_not_reference_atom_ast() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut offenders = Vec::new();
    for krate in EMIT_CRATES {
        let src = root.join(krate).join("src");
        assert!(src.is_dir(), "missing {krate}/src");
        let mut files = Vec::new();
        collect_rs_files(&src, &mut files);
        for file in files {
            let text = std::fs::read_to_string(&file).expect("read source file");
            let prod = production_part(&text);
            for needle in FORBIDDEN {
                if prod.contains(needle) {
                    offenders.push(format!("{} contains `{needle}`", file.display()));
                }
            }
            for hit in offending_field_accesses(prod) {
                offenders.push(format!("{} accesses `{hit}`", file.display()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "emit crates must not reach the AST:\n{}",
        offenders.join("\n")
    );
}
