use encoding_rs::{Encoding, SHIFT_JIS};
use mumei_core::parser::{ImportDecl, Item};
use mumei_core::{ast, manifest, parser, resolver, verification};
use std::fs;
use std::path::Path;

#[allow(dead_code)]
pub(crate) fn load_source(input: &str) -> String {
    read_source_file(input).unwrap_or_else(|_| {
        eprintln!("❌ Error: Could not read Mumei source file '{}'", input);
        std::process::exit(1);
    })
}

pub(crate) fn decode_source_bytes(bytes: &[u8]) -> Option<String> {
    if let Some((encoding, bom_len)) = Encoding::for_bom(bytes) {
        let (source, _, had_errors) = encoding.decode(&bytes[bom_len..]);
        if !had_errors {
            return Some(source.into_owned());
        }
    } else if let Ok(source) = std::str::from_utf8(bytes) {
        return Some(source.to_owned());
    } else {
        let (source, _, had_errors) = SHIFT_JIS.decode(bytes);
        if !had_errors {
            return Some(source.into_owned());
        }
    }

    None
}

pub(crate) fn read_source_file<P: AsRef<Path>>(path: P) -> Result<String, String> {
    let path = path.as_ref();
    let bytes = fs::read(path).map_err(|err| err.to_string())?;
    decode_source_bytes(&bytes).ok_or_else(|| "unsupported source encoding".to_string())
}

/// Z3 が利用可能かチェックし、なければ親切なメッセージで終了する
pub(crate) fn check_z3_available() {
    use std::process::Command as Cmd;
    if Cmd::new("z3").arg("--version").output().is_err() {
        eprintln!("❌ Error: Z3 solver not found.");
        eprintln!();
        eprintln!("   Mumei requires Z3 for formal verification.");
        eprintln!("   Install it with one of:");
        eprintln!("     macOS:  brew install z3");
        eprintln!("     Ubuntu: sudo apt-get install libz3-dev");
        eprintln!("     Auto:   mumei setup");
        eprintln!();
        eprintln!("   After installing, run `mumei inspect` to verify.");
        std::process::exit(1);
    }
}

/// parse → resolve → monomorphize → ModuleEnv に全定義を登録
/// ソースコード文字列も返す（miette リッチ出力のため）
pub(crate) fn load_and_prepare(
    input: &str,
) -> (Vec<Item>, verification::ModuleEnv, Vec<ImportDecl>, String) {
    load_and_prepare_with_full_options(input, false, false)
}

/// P5-C: load_and_prepare with strict_imports option.
#[allow(dead_code)]
pub(crate) fn load_and_prepare_with_options(
    input: &str,
    strict_imports: bool,
) -> (Vec<Item>, verification::ModuleEnv, Vec<ImportDecl>, String) {
    load_and_prepare_with_full_options(input, strict_imports, false)
}

/// PR 2: load_and_prepare with both strict_imports and allow_lean_verified.
/// `allow_lean_verified == true` widens the resolver so it accepts
/// mumei-lean-emitted `lean_verified` certificates as `"proven"`.
pub(crate) fn load_and_prepare_with_full_options(
    input: &str,
    strict_imports: bool,
    allow_lean_verified: bool,
) -> (Vec<Item>, verification::ModuleEnv, Vec<ImportDecl>, String) {
    try_load_and_prepare_with_full_options(input, strict_imports, allow_lean_verified)
        .unwrap_or_else(|e| {
            eprintln!("  ❌ {e}");
            std::process::exit(1);
        })
}

/// Non-exiting variant of `load_and_prepare` — returns `Err` on read/resolve
/// failures instead of calling `std::process::exit(1)`.
pub(crate) fn try_load_and_prepare(
    input: &str,
) -> Result<(Vec<Item>, verification::ModuleEnv, Vec<ImportDecl>, String), String> {
    try_load_and_prepare_with_full_options(input, false, false)
}

pub(crate) fn try_load_and_prepare_with_full_options(
    input: &str,
    strict_imports: bool,
    allow_lean_verified: bool,
) -> Result<(Vec<Item>, verification::ModuleEnv, Vec<ImportDecl>, String), String> {
    let source =
        read_source_file(input).map_err(|e| format!("Could not read '{}': {}", input, e))?;
    let items = parser::parse_module(&source);

    let mut module_env = verification::ModuleEnv::new();
    verification::register_builtin_traits(&mut module_env);
    verification::register_builtin_effects(&mut module_env);
    let input_path = Path::new(input);
    let base_dir = input_path.parent().unwrap_or(Path::new("."));

    if let Err(e) = resolver::resolve_prelude(base_dir, &mut module_env) {
        eprintln!("  ⚠️  Prelude load warning: {}", e);
    }

    if let Some((proj_dir, m)) = manifest::find_and_load() {
        if strict_imports || allow_lean_verified {
            if let Err(e) = resolver::resolve_manifest_dependencies_with_full_options(
                &m,
                &proj_dir,
                &mut module_env,
                strict_imports,
                allow_lean_verified,
            ) {
                if strict_imports {
                    return Err(format!("Dependency resolution failed (strict mode): {}", e));
                }
                eprintln!("  ⚠️  Dependency resolution warning: {}", e);
            }
        } else if let Err(e) =
            resolver::resolve_manifest_dependencies(&m, &proj_dir, &mut module_env)
        {
            eprintln!("  ⚠️  Dependency resolution warning: {}", e);
        }
    }

    if strict_imports || allow_lean_verified {
        if let Err(e) = resolver::resolve_imports_with_full_options(
            &items,
            base_dir,
            &mut module_env,
            strict_imports,
            allow_lean_verified,
        ) {
            if strict_imports {
                return Err(format!("Import resolution failed (strict mode): {}", e));
            }
            return Err(format!("Import resolution failed: {}", e));
        }
    } else if let Err(e) = resolver::resolve_imports(&items, base_dir, &mut module_env) {
        return Err(format!("Import resolution failed: {}", e));
    }

    for item in &items {
        match item {
            Item::TraitDef(trait_def) => module_env.register_trait(trait_def),
            Item::ImplDef(impl_def) => module_env.register_impl(impl_def),
            Item::EffectDef(effect_def) => module_env.register_effect(effect_def),
            _ => {}
        }
    }

    let mut mono = ast::Monomorphizer::new();
    mono.collect(&items);
    let mut items = if mono.has_generics() {
        let mono_items = mono.monomorphize(&items, Some(&module_env));
        eprintln!(
            "  🔬 Monomorphization: {} generic instance(s) expanded.",
            mono.instances().len()
        );
        mono_items
    } else {
        items
    };
    annotate_source_file(&mut items, input);

    let mut imports: Vec<ImportDecl> = Vec::new();
    for item in &items {
        match item {
            Item::Import(decl) => imports.push(decl.clone()),
            Item::TypeDef(refined_type) => module_env.register_type(refined_type),
            Item::StructDef(struct_def) => module_env.register_struct(struct_def),
            Item::EnumDef(enum_def) => module_env.register_enum(enum_def),
            Item::Atom(atom) => module_env.register_atom(atom),
            Item::TraitDef(_) => {}
            Item::ImplDef(_) => {}
            Item::ResourceDef(resource_def) => module_env.register_resource(resource_def),
            Item::EffectDef(_) => {}
            Item::ImplBlock(impl_block) => {
                for method in &impl_block.methods {
                    let mut qualified = method.clone();
                    qualified.name = format!("{}::{}", impl_block.struct_name, method.name);
                    module_env.register_atom(&qualified);
                }
            }
            Item::ExternBlock(extern_block) => {
                module_env.register_extern_block(extern_block);
                for ext_fn in &extern_block.functions {
                    let params: Vec<parser::Param> = ext_fn
                        .param_types
                        .iter()
                        .enumerate()
                        .map(|(i, ty)| parser::Param {
                            name: ext_fn
                                .param_names
                                .get(i)
                                .cloned()
                                .unwrap_or_else(|| format!("arg{}", i)),
                            type_name: Some(ty.clone()),
                            type_ref: Some(parser::parse_type_ref(ty)),
                            is_ref: false,
                            is_ref_mut: false,
                            fn_contract_requires: None,
                            fn_contract_ensures: None,
                        })
                        .collect();

                    let atom = parser::Atom {
                        name: ext_fn.name.clone(),
                        type_params: vec![],
                        where_bounds: vec![],
                        params,
                        trace_id: None,
                        spec_metadata: std::collections::HashMap::new(),
                        requires: ext_fn
                            .requires
                            .clone()
                            .unwrap_or_else(|| "true".to_string()),
                        forall_constraints: vec![],
                        ensures: ext_fn.ensures.clone().unwrap_or_else(|| "true".to_string()),
                        body_expr: String::new(),
                        consumed_params: vec![],
                        resources: vec![],
                        is_async: false,
                        trust_level: parser::TrustLevel::Trusted,
                        max_unroll: None,
                        invariant: None,
                        effects: vec![],
                        return_type: Some(ext_fn.return_type.clone()),
                        span: ext_fn.span.clone(),
                        effect_pre: std::collections::HashMap::new(),
                        effect_post: std::collections::HashMap::new(),
                    };
                    module_env.register_atom(&atom);
                }
            }
        }
    }

    Ok((items, module_env, imports, source))
}

pub(crate) fn annotate_source_file(items: &mut [Item], input: &str) {
    for item in items {
        match item {
            Item::Atom(atom) => annotate_atom_source_file(atom, input),
            Item::TypeDef(refined_type) => refined_type.span.file = input.to_string(),
            Item::StructDef(struct_def) => {
                struct_def.span.file = input.to_string();
                for method in &mut struct_def.methods {
                    annotate_atom_source_file(method, input);
                }
            }
            Item::EnumDef(enum_def) => enum_def.span.file = input.to_string(),
            Item::Import(decl) => decl.span.file = input.to_string(),
            Item::TraitDef(trait_def) => trait_def.span.file = input.to_string(),
            Item::ImplDef(impl_def) => impl_def.span.file = input.to_string(),
            Item::ResourceDef(resource_def) => resource_def.span.file = input.to_string(),
            Item::ExternBlock(extern_block) => {
                extern_block.span.file = input.to_string();
                for function in &mut extern_block.functions {
                    function.span.file = input.to_string();
                }
            }
            Item::EffectDef(effect_def) => effect_def.span.file = input.to_string(),
            Item::ImplBlock(impl_block) => {
                impl_block.span.file = input.to_string();
                for method in &mut impl_block.methods {
                    annotate_atom_source_file(method, input);
                }
            }
        }
    }
}

pub(crate) fn annotate_atom_source_file(atom: &mut parser::Atom, input: &str) {
    atom.span.file = input.to_string();
    atom.spec_metadata
        .insert("source_file".to_string(), input.to_string());
}

pub(crate) fn load_cross_spec_files(
    cross_spec_files: &[String],
    strict_imports: bool,
    allow_lean_verified: bool,
    items: &mut Vec<Item>,
    module_env: &mut verification::ModuleEnv,
    imports: &mut Vec<ImportDecl>,
    verbose: bool,
) {
    for file in cross_spec_files {
        if verbose {
            println!(
                "  🔗 Loading cross-spec file '{}' for shared human/MCP artifact mapping...",
                file
            );
        }
        let (mut extra_items, extra_env, mut extra_imports, _source) =
            load_and_prepare_with_full_options(file, strict_imports, allow_lean_verified);
        items.append(&mut extra_items);
        imports.append(&mut extra_imports);
        merge_module_env(module_env, extra_env);
    }
}

pub(crate) fn merge_module_env(
    target: &mut verification::ModuleEnv,
    source: verification::ModuleEnv,
) {
    target.types.extend(source.types);
    target.structs.extend(source.structs);
    target.atoms.extend(source.atoms);
    target.extern_blocks.extend(source.extern_blocks);
    target.enums.extend(source.enums);
    target.traits.extend(source.traits);
    target.impls.extend(source.impls);
    target.verified_cache.extend(source.verified_cache);
    target.resources.extend(source.resources);
    target.effects.extend(source.effects);
    target.effect_defs.extend(source.effect_defs);
    target.path_id_map.extend(source.path_id_map);
    target.next_path_id = target.next_path_id.max(source.next_path_id);
    target.prefix_ranges.extend(source.prefix_ranges);
    target.dependency_graph.extend(source.dependency_graph);
    for (atom, dependents) in source.reverse_deps {
        target
            .reverse_deps
            .entry(atom)
            .or_default()
            .extend(dependents);
    }
    if target.security_policy.is_none() {
        target.security_policy = source.security_policy;
    }
    for (method, entries) in source.method_trait_index {
        target
            .method_trait_index
            .entry(method)
            .or_default()
            .extend(entries);
    }
}

// =============================================================================
// mumei check — parse + resolve + monomorphize only
// =============================================================================

pub(crate) fn collect_mm_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut result = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                result.extend(collect_mm_files(&path));
            } else if path.extension().is_some_and(|ext| ext == "mm") {
                result.push(path);
            }
        }
    }
    result
}

pub(crate) fn copy_dir_recursive(src: &Path, dst: &Path) {
    let _ = fs::create_dir_all(dst);
    if let Ok(entries) = fs::read_dir(src) {
        for entry in entries.flatten() {
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());
            if src_path.is_dir() {
                copy_dir_recursive(&src_path, &dst_path);
            } else {
                let _ = fs::copy(&src_path, &dst_path);
            }
        }
    }
}
