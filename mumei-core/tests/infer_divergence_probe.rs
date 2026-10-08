//! Regression probe for step 5 of the HIR emitter migration: enumerate atoms
//! where `signature.inferred_return_type` (stored during env-aware HIR
//! lowering) disagrees with `mir::infer_atom_return_type` (which infers on a
//! body lowered WITHOUT a ModuleEnv). `lower_atom_to_hir_with_env` recomputes
//! the inferred type on a no-env body so the two results agree by
//! construction; this test scans the repo .mm corpus and fails if an
//! atom ever diverges, plus a targeted case that exercised the gap.

use mumei_core::parser::{self, Item};
use mumei_core::verification::{self, ModuleEnv};
use std::path::{Path, PathBuf};

fn collect_mm_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_mm_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("mm") {
            out.push(path);
        }
    }
}

fn register_items(items: &[Item], env: &mut ModuleEnv) {
    for item in items {
        match item {
            Item::Atom(a) => env.register_atom(a),
            Item::TypeDef(t) => env.register_type(t),
            Item::StructDef(s) => env.register_struct(s),
            Item::EnumDef(e) => env.register_enum(e),
            Item::TraitDef(t) => env.register_trait(t),
            Item::ImplDef(i) => env.register_impl(i),
            Item::ResourceDef(r) => env.register_resource(r),
            Item::EffectDef(e) => env.register_effect(e),
            Item::CapabilityDef(c) => env.register_capability(c),
            Item::ExternBlock(eb) => {
                env.register_extern_block(eb);
                for ext_fn in &eb.functions {
                    let atom = mumei_core::trust_boundary::extern_fn_as_trusted_atom(ext_fn);
                    env.register_atom(&atom);
                }
            }
            Item::ImplBlock(ib) => {
                for method in &ib.methods {
                    let mut qualified = method.clone();
                    qualified.name = format!("{}::{}", ib.struct_name, method.name);
                    env.register_atom(&qualified);
                }
            }
            Item::Import(_) => {}
        }
    }
}

#[test]
fn enumerate_inferred_return_type_divergences() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let mut files = Vec::new();
    for dir in ["std", "benchmarks", "tests", "examples"] {
        collect_mm_files(&root.join(dir), &mut files);
    }
    if let Ok(extra) = std::env::var("MUMEI_PROBE_EXTRA_DIR") {
        collect_mm_files(Path::new(&extra), &mut files);
    }
    for entry in std::fs::read_dir(&root).unwrap().flatten() {
        let p = entry.path();
        if p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("mm") {
            files.push(p);
        }
    }
    files.sort();

    // Parse every file once; build a union ModuleEnv covering the corpus so
    // intra- and cross-file call resolution both work.
    let mut parsed: Vec<(PathBuf, Vec<Item>)> = Vec::new();
    let mut env = ModuleEnv::new();
    verification::register_builtin_traits(&mut env);
    verification::register_builtin_effects(&mut env);
    let mut parse_failures = 0usize;
    for file in &files {
        let Ok(source) = std::fs::read_to_string(file) else {
            continue;
        };
        match parser::parse_module_checked(&source) {
            Ok(items) => {
                register_items(&items, &mut env);
                parsed.push((file.clone(), items));
            }
            Err(_) => parse_failures += 1,
        }
    }

    let mut total_atoms = 0usize;
    let mut without_return_type = 0usize;
    let mut divergences: Vec<String> = Vec::new();
    for (file, items) in &parsed {
        for item in items {
            let atoms: Vec<&parser::Atom> = match item {
                Item::Atom(a) => vec![a],
                Item::ImplBlock(ib) => ib.methods.iter().collect(),
                _ => vec![],
            };
            for atom in atoms {
                total_atoms += 1;
                if atom.return_type.is_some() {
                    continue;
                }
                without_return_type += 1;
                let ast_inference = mumei_core::mir::infer_atom_return_type(atom);
                let hir = mumei_core::hir::lower_atom_to_hir_with_env(atom, Some(&env));
                let sig_inference = hir.signature.inferred_return_type;
                if ast_inference != sig_inference {
                    divergences.push(format!(
                        "{} :: {} -> ast={:?} signature={:?}",
                        file.display(),
                        atom.name,
                        ast_inference,
                        sig_inference
                    ));
                }
            }
        }
    }

    println!(
        "\n=== probe: {} files ({} parse failures skipped), {} atoms, {} without declared return type ===",
        parsed.len(),
        parse_failures,
        total_atoms,
        without_return_type
    );
    assert!(
        divergences.is_empty(),
        "inferred_return_type must equal infer_atom_return_type for every atom:\n{}",
        divergences.join("\n")
    );
}

/// The gap the no-env recompute in `lower_atom_to_hir_with_env` closes: env
/// lowering annotates `let x = g()` with g's declared return type, and
/// inference over that annotated body can find a type the AST path cannot.
/// `signature.inferred_return_type` must still equal the AST-path result.
#[test]
fn env_lowering_does_not_change_inferred_return_type() {
    let source = r#"
atom g() -> f64
requires: true;
ensures: true;
body: { 1.0 };

atom f()
requires: true;
ensures: true;
body: {
    let x = g();
    x
};
"#;
    let items = parser::parse_module_checked(source).expect("fixture parses");
    let mut env = ModuleEnv::new();
    let mut f_atom = None;
    for item in &items {
        register_items(std::slice::from_ref(item), &mut env);
        if let Item::Atom(a) = item {
            if a.name == "f" {
                f_atom = Some(a.clone());
            }
        }
    }
    let f_atom = f_atom.expect("fixture has atom f");

    // On the env-lowered body the let-binding carries `f64`, so inference
    // over it WOULD find `f64` — the pre-fix divergence.
    let env_body = {
        let stmt = mumei_core::parser::parse_body_expr(&f_atom.body_expr);
        mumei_core::hir::lower_stmt_with_env(&stmt, Some(&env))
    };
    assert_eq!(
        mumei_core::mir::infer_return_type_from_hir(&f_atom.params, &env_body),
        Some("f64".to_string()),
        "env-lowered body carries the let annotation that diverges"
    );

    // The stored signature must nevertheless match the AST-path result.
    let hir = mumei_core::hir::lower_atom_to_hir_with_env(&f_atom, Some(&env));
    assert_eq!(
        hir.signature.inferred_return_type,
        mumei_core::mir::infer_atom_return_type(&f_atom),
        "inferred_return_type must equal the no-env (AST) result"
    );
}
