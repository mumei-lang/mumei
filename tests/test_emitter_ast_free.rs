//! Step 7 of the HIR emitter migration: emitter-facing code must not reach
//! the AST. `Emitter::emit` receives `&EmitAtom<'_>` (HIR-only), so no
//! `mumei-emit-*` source file may read `HirAtom.atom`/`HirAtom.body_stmt`
//! or reference the parser types that HIR now owns (`Op`, `Pattern`,
//! `JoinSemantics`). `parser::ExternBlock`/`EnumDef`/`StructDef` remain
//! allowed — they arrive via `ModuleEnv` and the emit signature, not via
//! the atom.

use std::path::{Path, PathBuf};

const EMIT_CRATES: &[&str] = &[
    "mumei-emit-llvm",
    "mumei-emit-json",
    "mumei-emit-proofbook",
    "mumei-emit-monitor",
    "mumei-emit-python",
    "mumei-emit-rust",
];

const FORBIDDEN: &[&str] = &[
    "hir_atom.atom",
    "hir_atom.body_stmt",
    "emit_atom.atom",
    "emit_atom.body_stmt",
    "parser::Op",
    "parser::Pattern",
    "parser::JoinSemantics",
    "parser::ast::Op",
    "parser::ast::Pattern",
    "parser::ast::JoinSemantics",
];

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
            for needle in FORBIDDEN {
                if text.contains(needle) {
                    offenders.push(format!("{} contains `{needle}`", file.display()));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "emit crates must not reach the AST:\n{}",
        offenders.join("\n")
    );
}
