use crate::cli::Command;
use crate::codegen::*;
use crate::commands::verify::{save_cross_spec_report, save_decidable_fragment_metrics};
use crate::feedback::*;
use crate::linker;
use crate::pipeline::*;
use mumei_core::hir::lower_atom_to_hir_with_env;
use mumei_core::parser::Item;
use mumei_core::{
    emitter, manifest, mir, mir_analysis, parser, proof_cert, resolver, verification,
};
use std::fs;
use std::path::Path;

pub(crate) fn cmd_build_command(command: Command) {
    let Command::Build {
        input,
        output,
        emit,
        strict_imports,
        allow_lean_verified,
    } = command
    else {
        unreachable!("cmd_build_command called with non-build command");
    };
    let emit_target = emit_target_from_cli(&emit);
    cmd_build(
        &input,
        &output,
        &emit_target,
        strict_imports,
        allow_lean_verified,
    );
}

pub(crate) fn cmd_build_default(input: &str, output: &str) {
    cmd_build(input, output, &emitter::EmitTarget::LlvmIr, false, false);
}

fn emit_target_from_cli(emit: &str) -> emitter::EmitTarget {
    emitter::EmitTarget::from_cli(emit)
}

pub(crate) fn cmd_build(
    input: &str,
    output: &str,
    emit_target: &emitter::EmitTarget,
    strict_imports: bool,
    allow_lean_verified: bool,
) {
    let external_emitter = match emit_target {
        emitter::EmitTarget::External(name) => match emitter::load_external_emitter(name) {
            Ok(emitter) => Some(emitter),
            Err(err) => {
                eprintln!(
                    "\u{274c} Error: Unknown emit target '{}'. Valid built-in values: {}.",
                    name,
                    emitter::EmitTarget::builtin_cli_names()
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                eprintln!("  External plugin lookup failed: {}", err);
                std::process::exit(1);
            }
        },
        _ => None,
    };

    check_z3_available();
    println!("🗡️  Mumei: Forging the blade (Type System 2.0 + Generics enabled)...");

    // mumei.toml の自動検出と設定適用
    let manifest_config = manifest::find_and_load();
    let (build_cfg, proof_cfg) = if let Some((ref _proj_dir, ref m)) = manifest_config {
        println!(
            "  📄 Using mumei.toml: {} v{}",
            m.package.name, m.package.version
        );
        (m.build.clone(), m.proof.clone())
    } else {
        (
            manifest::BuildConfig::default(),
            manifest::ProofConfig::default(),
        )
    };

    let (items, mut module_env, _imports, source) =
        load_and_prepare_with_full_options(input, strict_imports, allow_lean_verified);

    let output_path = Path::new(output);
    let output_dir = output_path.parent().unwrap_or(Path::new("."));
    let file_stem = output_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(output);
    let input_path = Path::new(input);
    let build_base_dir = input_path.parent().unwrap_or(Path::new("."));

    // Feature 2: Register dependencies for all atoms before verification
    for item in &items {
        match item {
            Item::Atom(atom) => {
                let callees = resolver::collect_callees_from_body(&atom.body_expr);
                module_env.register_dependencies(&atom.name, callees);
            }
            Item::ImplBlock(impl_block) => {
                for method in &impl_block.methods {
                    let qualified_name = format!("{}::{}", impl_block.struct_name, method.name);
                    let callees = resolver::collect_callees_from_body(&method.body_expr);
                    module_env.register_dependencies(&qualified_name, callees);
                }
            }
            _ => {}
        }
    }

    // Feature 2: Migrate old cache and load enhanced verification cache
    resolver::migrate_old_cache(build_base_dir);
    let verification_cache = if proof_cfg.cache {
        resolver::load_verification_cache(build_base_dir)
    } else {
        std::collections::HashMap::new()
    };
    let mut verification_cache_new = verification_cache.clone();

    let skip_verify = !build_cfg.verify;
    let verification_config = verification::VerificationConfig {
        timeout_ms: proof_cfg.timeout_ms,
        global_max_unroll: build_cfg.max_unroll,
        enable_cross_spec_verification: proof_cfg.cross_spec_verify,
        collect_decidable_fragment_metrics: matches!(
            emit_target,
            emitter::EmitTarget::DecidableMetrics
        ),
        enable_spurious_detection: true,
        enable_vacuity_check: std::env::var("MUMEI_ENABLE_VACUITY_CHECK").unwrap_or_default()
            == "1",
        detect_loops: false,
        suggest_cegis: false,
        ieee754_f64: false,
        property_based_test: None,
    };
    let mut escalation_cert_results: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();

    let mut atom_count = 0;

    for item in &items {
        match item {
            // --- import 宣言（resolver で処理済み） ---
            Item::Import(import_decl) => {
                let alias_str = import_decl.alias.as_deref().unwrap_or("(none)");
                println!("  📦 Import: '{}' as '{}'", import_decl.path, alias_str);
            }

            // --- 精緻型の登録 ---
            Item::TypeDef(refined_type) => {
                println!(
                    "  ✨ Registered Refined Type: '{}' ({})",
                    refined_type.name, refined_type._base_type
                );
            }

            // --- 構造体定義の登録 ---
            Item::StructDef(struct_def) => {
                let field_names: Vec<&str> =
                    struct_def.fields.iter().map(|f| f.name.as_str()).collect();
                println!(
                    "  🏗️  Registered Struct: '{}' (fields: {})",
                    struct_def.name,
                    field_names.join(", ")
                );
            }

            // --- Enum 定義の登録 ---
            Item::EnumDef(enum_def) => {
                let variant_names: Vec<&str> =
                    enum_def.variants.iter().map(|v| v.name.as_str()).collect();
                println!(
                    "  🔷 Registered Enum: '{}' (variants: {})",
                    enum_def.name,
                    variant_names.join(", ")
                );
            }

            // --- トレイト定義 ---
            Item::TraitDef(trait_def) => {
                let method_names: Vec<&str> =
                    trait_def.methods.iter().map(|m| m.name.as_str()).collect();
                let law_names: Vec<&str> = trait_def.laws.iter().map(|(n, _)| n.as_str()).collect();
                println!(
                    "  📜 Registered Trait: '{}' (methods: {}, laws: {})",
                    trait_def.name,
                    method_names.join(", "),
                    law_names.join(", ")
                );
            }

            // --- トレイト実装の登録 + 法則検証 ---
            Item::ImplDef(impl_def) => {
                println!(
                    "  🔧 Registered Impl: {} for {}",
                    impl_def.trait_name, impl_def.target_type
                );
                // impl が trait の全 law を満たしているか Z3 で検証
                if skip_verify {
                    println!("    ⚖️  Laws verification skipped (verify=false in mumei.toml)");
                } else {
                    match verification::verify_impl_with_options(
                        impl_def,
                        &module_env,
                        output_dir,
                        verification_config.ieee754_f64,
                    ) {
                        Ok(_) => println!(
                            "    ✅ Laws verified for impl {} for {}",
                            impl_def.trait_name, impl_def.target_type
                        ),
                        Err(e) => {
                            let resolved = resolve_source_for_span(&source, &impl_def.span);
                            let e = e.with_source(&resolved, &impl_def.span);
                            eprintln!("{:?}", miette::Report::new(e));
                            std::process::exit(1);
                        }
                    }
                }
            }

            // --- リソース定義の登録 ---
            Item::ResourceDef(resource_def) => {
                let mode_str = match resource_def.mode {
                    parser::ResourceMode::Exclusive => "exclusive",
                    parser::ResourceMode::Shared => "shared",
                };
                println!(
                    "  🔒 Registered Resource: '{}' (priority={}, mode={})",
                    resource_def.name, resource_def.priority, mode_str
                );
            }

            // --- extern ブロック ---
            Item::ExternBlock(eb) => {
                println!(
                    "  🔗 Extern \"{}\" ({} function(s))",
                    eb.language,
                    eb.functions.len()
                );
            }

            // --- エフェクト定義 ---
            Item::EffectDef(effect_def) => {
                println!("  ⚡ Effect: '{}'", effect_def.name);
            }

            // --- impl ブロック (struct method) ---
            Item::ImplBlock(ib) => {
                println!(
                    "  🔧 ImplBlock: {} ({} method(s))",
                    ib.struct_name,
                    ib.methods.len()
                );
                for method in &ib.methods {
                    atom_count += 1;
                    let qualified_name = format!("{}::{}", ib.struct_name, method.name);
                    println!(
                        "  ✨ [1/3] Polishing Syntax: Atom '{}' identified.",
                        qualified_name
                    );

                    // Clone method with qualified name for consistent naming
                    // throughout HIR lowering, codegen, proof hash, and cache
                    let mut qualified_method = method.clone();
                    qualified_method.name = qualified_name.clone();

                    emit_decidable_fragment_warning(&qualified_method, &module_env, false);
                    let hir_atom = lower_atom_to_hir_with_env(&qualified_method, Some(&module_env));

                    let mir_body = mir::lower_hir_to_mir(&hir_atom);
                    match mir_body.check_analysis_budget() {
                        Ok(()) => {
                            let liveness = mir_analysis::compute_liveness(&mir_body);
                            let mut mir_body_mut = mir_body;
                            mir_analysis::insert_drops(&mut mir_body_mut, &liveness);
                        }
                        Err(msg) => {
                            eprintln!("  ⚠️  {}", msg);
                        }
                    }

                    if skip_verify {
                        println!("  ⚖️  [2/3] Verification: Skipped (verify=false in mumei.toml).");
                        module_env.mark_verified(&qualified_name);
                    } else if module_env.is_verified(&qualified_name) {
                        println!("  ⚖️  [2/3] Verification: Skipped (imported, contract-trusted).");
                    } else {
                        let proof_flags = if verification_config.enable_vacuity_check {
                            &["enable_vacuity_check"][..]
                        } else {
                            &[][..]
                        };
                        let proof_hash = resolver::compute_proof_hash_with_flags(
                            &qualified_method,
                            &module_env,
                            proof_flags,
                        );

                        let cache_hit = verification_cache
                            .get(&qualified_name)
                            .is_some_and(|entry| entry.proof_hash == proof_hash);

                        if cache_hit {
                            println!("  ⚖️  [2/3] Verification: Skipped (unchanged, cached) ⏩");
                            module_env.mark_verified(&qualified_name);
                        } else {
                            match verification::verify_with_verification_config(
                                &hir_atom,
                                output_dir,
                                &module_env,
                                &verification_config,
                            ) {
                                Ok(_) => {
                                    println!(
                                        "  ⚖️  [2/3] Verification: Passed. Logic verified with Z3."
                                    );
                                    module_env.mark_verified(&qualified_name);
                                    let deps: Vec<String> = module_env
                                        .dependency_graph
                                        .get(&qualified_name)
                                        .map(|s| s.iter().cloned().collect())
                                        .unwrap_or_default();
                                    let type_deps: Vec<String> = method
                                        .params
                                        .iter()
                                        .filter_map(|p| {
                                            p.type_ref.as_ref().map(|tr| tr.name.clone())
                                        })
                                        .filter(|tn| module_env.get_type(tn).is_some())
                                        .collect();
                                    verification_cache_new.insert(
                                        qualified_name.clone(),
                                        resolver::VerificationCacheEntry {
                                            proof_hash,
                                            result: "verified".to_string(),
                                            dependencies: deps,
                                            type_deps,
                                            timestamp: format!(
                                                "{}s",
                                                std::time::SystemTime::now()
                                                    .duration_since(std::time::UNIX_EPOCH)
                                                    .unwrap_or_default()
                                                    .as_secs()
                                            ),
                                        },
                                    );
                                }
                                Err(e) => {
                                    let error_text = format!("{e}");
                                    let z3_result =
                                        verification::z3_result_from_error_message(&error_text)
                                            .unwrap_or("sat")
                                            .to_string();
                                    let classification =
                                        verification::classify_atom_for_lean_escalation(
                                            &qualified_method,
                                            &module_env,
                                            &z3_result,
                                            "failed",
                                        );
                                    let resolved = resolve_source_for_span(&source, &method.span);
                                    let e = e.with_source(&resolved, &method.span);
                                    eprintln!("{:?}", miette::Report::new(e));
                                    verification_cache_new.remove(&qualified_name);
                                    if matches!(emit_target, emitter::EmitTarget::EscalationBundle)
                                        && classification.should_escalate
                                    {
                                        println!(
                                            "  ⚖️  [2/3] Verification: Deferred to Lean ({})",
                                            classification
                                                .escalation_reason
                                                .map(|reason| reason.as_str())
                                                .unwrap_or("lean_escalation")
                                        );
                                        escalation_cert_results.insert(
                                            qualified_name.clone(),
                                            (z3_result, "escalation_candidate".to_string()),
                                        );
                                        continue;
                                    }
                                    std::process::exit(1);
                                }
                            }
                        }
                    }

                    let safe_name = qualified_name.replace("::", "__");
                    let atom_output_path = output_dir.join(format!("{}_{}", file_stem, safe_name));
                    let extern_blocks = collect_extern_blocks(&module_env);
                    match dispatch_emit(
                        emit_target,
                        external_emitter.as_deref(),
                        &hir_atom,
                        &atom_output_path,
                        &module_env,
                        &extern_blocks,
                    ) {
                        Ok(artifacts) => {
                            for artifact in &artifacts {
                                let should_write = match emit_target {
                                    emitter::EmitTarget::LlvmIr => {
                                        artifact.kind != emitter::ArtifactKind::Source
                                    }
                                    // Phase 3 plugins are responsible for their
                                    // own ArtifactKind selection; we trust them
                                    // and write everything they emit.
                                    emitter::EmitTarget::External(_) => true,
                                    _ => true,
                                };
                                if should_write {
                                    if let Err(e) = std::fs::write(&artifact.name, &artifact.data) {
                                        eprintln!(
                                            "Failed to write artifact '{}': {}",
                                            artifact.name.display(),
                                            e
                                        );
                                        std::process::exit(1);
                                    }
                                }
                            }
                            if !matches!(emit_target, emitter::EmitTarget::DecidableMetrics) {
                                let target_desc = emit_target.label();
                                println!(
                                    "  ⚙️  [3/3] Tempering: Done. Compiled '{}' to {}.",
                                    qualified_name, target_desc
                                );
                            }
                        }
                        Err(e) => {
                            let resolved = resolve_source_for_span(&source, &method.span);
                            let e = e.with_source(&resolved, &method.span);
                            eprintln!("{:?}", miette::Report::new(e));
                            std::process::exit(1);
                        }
                    }
                }
            }

            // --- Atom の処理 ---
            Item::Atom(atom) => {
                atom_count += 1;
                let async_marker = if atom.is_async { " (async)" } else { "" };
                let res_marker = if !atom.resources.is_empty() {
                    format!(" [resources: {}]", atom.resources.join(", "))
                } else {
                    String::new()
                };
                println!(
                    "  ✨ [1/3] Polishing Syntax: Atom '{}'{}{} identified.",
                    atom.name, async_marker, res_marker
                );

                emit_decidable_fragment_warning(atom, &module_env, false);

                // HIR lowering: body_expr を1回だけパースして全ステージで再利用する
                let hir_atom = lower_atom_to_hir_with_env(atom, Some(&module_env));

                // MIR pipeline: lower to MIR and run analyses
                let mir_body = mir::lower_hir_to_mir(&hir_atom);
                match mir_body.check_analysis_budget() {
                    Ok(()) => {
                        let liveness = mir_analysis::compute_liveness(&mir_body);
                        let mut mir_body_mut = mir_body;
                        mir_analysis::insert_drops(&mut mir_body_mut, &liveness);
                    }
                    Err(msg) => {
                        eprintln!("  ⚠️  {}", msg);
                    }
                }

                // --- 2. Verification (形式検証: Z3 + StdLib) ---
                if skip_verify {
                    println!("  ⚖️  [2/3] Verification: Skipped (verify=false in mumei.toml).");
                    module_env.mark_verified(&atom.name);
                } else if module_env.is_verified(&atom.name) {
                    // インポートされた atom は検証済み（契約のみ信頼）なのでスキップ
                    println!("  ⚖️  [2/3] Verification: Skipped (imported, contract-trusted).");
                } else {
                    // Feature 2: Use compute_proof_hash with dependency-aware hashing
                    let proof_flags = if verification_config.enable_vacuity_check {
                        &["enable_vacuity_check"][..]
                    } else {
                        &[][..]
                    };
                    let proof_hash =
                        resolver::compute_proof_hash_with_flags(atom, &module_env, proof_flags);

                    let cache_hit = verification_cache
                        .get(&atom.name)
                        .is_some_and(|entry| entry.proof_hash == proof_hash);

                    if cache_hit {
                        println!("  ⚖️  [2/3] Verification: Skipped (unchanged, cached) ⏩");
                        module_env.mark_verified(&atom.name);
                    } else {
                        match verification::verify_with_verification_config(
                            &hir_atom,
                            output_dir,
                            &module_env,
                            &verification_config,
                        ) {
                            Ok(_) => {
                                println!(
                                    "  ⚖️  [2/3] Verification: Passed. Logic verified with Z3."
                                );
                                module_env.mark_verified(&atom.name);
                                // Collect dependency info for cache entry
                                let deps: Vec<String> = module_env
                                    .dependency_graph
                                    .get(&atom.name)
                                    .map(|s| s.iter().cloned().collect())
                                    .unwrap_or_default();
                                let type_deps: Vec<String> = atom
                                    .params
                                    .iter()
                                    .filter_map(|p| p.type_ref.as_ref().map(|tr| tr.name.clone()))
                                    .filter(|tn| module_env.get_type(tn).is_some())
                                    .collect();
                                verification_cache_new.insert(
                                    atom.name.clone(),
                                    resolver::VerificationCacheEntry {
                                        proof_hash,
                                        result: "verified".to_string(),
                                        dependencies: deps,
                                        type_deps,
                                        timestamp: format!(
                                            "{}s",
                                            std::time::SystemTime::now()
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .unwrap_or_default()
                                                .as_secs()
                                        ),
                                    },
                                );
                            }
                            Err(e) => {
                                let error_text = format!("{e}");
                                let z3_result =
                                    verification::z3_result_from_error_message(&error_text)
                                        .unwrap_or("sat")
                                        .to_string();
                                let classification =
                                    verification::classify_atom_for_lean_escalation(
                                        atom,
                                        &module_env,
                                        &z3_result,
                                        "failed",
                                    );
                                let resolved = resolve_source_for_span(&source, &atom.span);
                                let e = e.with_source(&resolved, &atom.span);
                                eprintln!("{:?}", miette::Report::new(e));
                                verification_cache_new.remove(&atom.name);
                                if matches!(emit_target, emitter::EmitTarget::EscalationBundle)
                                    && classification.should_escalate
                                {
                                    println!(
                                        "  ⚖️  [2/3] Verification: Deferred to Lean ({})",
                                        classification
                                            .escalation_reason
                                            .map(|reason| reason.as_str())
                                            .unwrap_or("lean_escalation")
                                    );
                                    escalation_cert_results.insert(
                                        atom.name.clone(),
                                        (z3_result, "escalation_candidate".to_string()),
                                    );
                                    continue;
                                }
                                std::process::exit(1);
                            }
                        }
                    }
                }

                // --- 3. Codegen / Emit ---
                // 各 Atom ごとにターゲット形式のファイルを生成
                let atom_output_path = output_dir.join(format!("{}_{}", file_stem, atom.name));
                let extern_blocks = collect_extern_blocks(&module_env);
                match dispatch_emit(
                    emit_target,
                    external_emitter.as_deref(),
                    &hir_atom,
                    &atom_output_path,
                    &module_env,
                    &extern_blocks,
                ) {
                    Ok(artifacts) => {
                        for artifact in &artifacts {
                            let should_write = match emit_target {
                                emitter::EmitTarget::LlvmIr => {
                                    artifact.kind != emitter::ArtifactKind::Source
                                }
                                emitter::EmitTarget::External(_) => true,
                                _ => true,
                            };
                            if should_write {
                                if let Err(e) = std::fs::write(&artifact.name, &artifact.data) {
                                    eprintln!(
                                        "Failed to write artifact '{}': {}",
                                        artifact.name.display(),
                                        e
                                    );
                                    std::process::exit(1);
                                }
                            }
                        }
                        if !matches!(emit_target, emitter::EmitTarget::DecidableMetrics) {
                            let target_desc = emit_target.label();
                            println!(
                                "  ⚙️  [3/3] Tempering: Done. Compiled '{}' to {}.",
                                atom.name, target_desc
                            );
                        }
                    }
                    Err(e) => {
                        let resolved = resolve_source_for_span(&source, &atom.span);
                        let e = e.with_source(&resolved, &atom.span);
                        eprintln!("{:?}", miette::Report::new(e));
                        std::process::exit(1);
                    }
                }
            }
        }
    }

    // P7-B: When --emit binary is requested, merge all atoms into a single
    // LLVM module with a C-compatible main wrapper and link to a native binary.
    if matches!(emit_target, emitter::EmitTarget::Binary) && atom_count > 0 {
        let extern_blocks = collect_extern_blocks(&module_env);
        let hir_atoms = collect_binary_hir_atoms(&items, &module_env);

        let object_path = output_dir.join(format!("{}_merged.o", file_stem));
        if let Err(e) = mumei_emit_llvm::binary::compile_atoms_to_binary_object(
            &hir_atoms,
            &module_env,
            &extern_blocks,
            &object_path,
        ) {
            eprintln!("❌ Binary codegen failed: {}", e);
            std::process::exit(1);
        }

        let binary_output = output_dir.join(file_stem);
        println!(
            "  🔗 Linking {} atom(s) to native binary...",
            hir_atoms.len()
        );
        let runtime_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/mumei_runtime.c");
        let runtime_stubs_path = output_dir.join(format!("{}_runtime_stubs.c", file_stem));
        if let Err(e) = write_effect_and_resource_runtime_stubs(&module_env, &runtime_stubs_path) {
            eprintln!("❌ Runtime stub generation failed: {}", e);
            std::process::exit(1);
        }
        let mut link_inputs = vec![object_path.clone(), runtime_stubs_path.clone()];
        let rust_ffi_lib = if uses_rust_ffi(&extern_blocks) {
            match generate_rust_ffi_staticlib(Path::new(env!("CARGO_MANIFEST_DIR")), output_dir) {
                Ok(path) => Some(path),
                Err(e) => {
                    eprintln!("❌ Rust FFI runtime build failed: {}", e);
                    std::process::exit(1);
                }
            }
        } else {
            None
        };
        if let Some(path) = &rust_ffi_lib {
            link_inputs.push(path.clone());
        }
        if let Err(e) = linker::link_to_binary(&link_inputs, &binary_output, Some(&runtime_path)) {
            eprintln!("❌ Linking failed: {}", e);
            std::process::exit(1);
        }
        println!("  ✅ Binary written to: {}", binary_output.display());
        // Clean up intermediate files
        let _ = fs::remove_file(&object_path);
        let _ = fs::remove_file(&runtime_stubs_path);
    }

    // P5-A: Generate proof certificate or Lean escalation bundle when requested
    if matches!(emit_target, emitter::EmitTarget::DecidableMetrics) {
        let metrics_path = output_dir.join(format!("{}.decidable-metrics.json", file_stem));
        match save_decidable_fragment_metrics(&module_env, &metrics_path, true) {
            Ok(()) => {}
            Err(err) => {
                eprintln!("❌ {err}");
                std::process::exit(1);
            }
        }
    }

    if matches!(
        emit_target,
        emitter::EmitTarget::ProofCert | emitter::EmitTarget::EscalationBundle
    ) && atom_count > 0
    {
        let mut cert_atoms: Vec<&parser::Atom> = items
            .iter()
            .filter_map(|item| {
                if let Item::Atom(a) = item {
                    Some(a)
                } else {
                    None
                }
            })
            .collect();
        let mut qualified_methods: Vec<parser::Atom> = Vec::new();
        for item in &items {
            if let Item::ImplBlock(ib) = item {
                for method in &ib.methods {
                    let mut qualified = method.clone();
                    qualified.name = format!("{}::{}", ib.struct_name, method.name);
                    qualified_methods.push(qualified);
                }
            }
        }
        for qm in &qualified_methods {
            cert_atoms.push(qm);
        }

        // Collect verification results, preserving Lean escalation candidates.
        let mut cert_results = escalation_cert_results;
        for atom_ref in &cert_atoms {
            if module_env.is_verified(&atom_ref.name) {
                cert_results
                    .entry(atom_ref.name.clone())
                    .or_insert_with(|| ("unsat".to_string(), "verified".to_string()));
            }
        }

        let (pkg_name, pkg_version) = if let Some((ref _proj_dir, ref m)) = manifest_config {
            (
                Some(m.package.name.as_str()),
                Some(m.package.version.as_str()),
            )
        } else {
            (None, None)
        };

        let cert = proof_cert::generate_certificate(
            input,
            &cert_atoms,
            &cert_results,
            &module_env,
            pkg_name,
            pkg_version,
            Some(proof_cert::HarnessCertificateMetadata {
                harness_contract: proof_cert::harness_contract_from_env(),
                intent_fidelity: proof_cert::intent_fidelity_from_env(),
                artifact_paths: proof_cert::artifact_paths_from_env(),
                budget_policy_fingerprint: proof_cert::budget_policy_fingerprint_from_env(),
            }),
        );

        let cert_path = output_dir.join(format!("{}.proof-cert.json", file_stem));
        if matches!(emit_target, emitter::EmitTarget::ProofCert) {
            match proof_cert::save_certificate(&cert, &cert_path) {
                Ok(()) => {
                    println!("  📜 Proof certificate written to: {}", cert_path.display());
                }
                Err(e) => {
                    eprintln!("  ⚠️  Failed to write proof certificate: {}", e);
                }
            }
        } else {
            let bundle = proof_cert::generate_escalation_bundle(&cert);
            let bundle_path = output_dir.join(format!("{}.escalation-bundle.json", file_stem));
            match proof_cert::save_escalation_bundle(&bundle, &bundle_path) {
                Ok(()) => {
                    println!(
                        "  Lean escalation bundle written to: {} ({} candidate(s))",
                        bundle_path.display(),
                        bundle.summary.candidate_count
                    );
                }
                Err(e) => {
                    eprintln!("  ⚠️  Failed to write escalation bundle: {}", e);
                }
            }
        }
    }

    if atom_count > 0 {
        println!("🎉 Blade forged successfully with {} atoms.", atom_count);
    } else {
        println!("⚠️  Warning: No atoms found in the source file.");
    }

    // Feature 2: Save enhanced verification cache
    if proof_cfg.cache {
        resolver::save_verification_cache(build_base_dir, &verification_cache_new);
    }

    if proof_cfg.cross_spec_verify {
        match save_cross_spec_report(&module_env, output_dir, true) {
            Ok(cross_spec_result) => {
                if cross_spec_result.summary.inconsistent_calls > 0 {
                    eprintln!(
                        "Warning: {} inconsistent contract calls detected",
                        cross_spec_result.summary.inconsistent_calls
                    );
                }
                if cross_spec_result.summary.circular_dependency_count > 0 {
                    eprintln!(
                        "Warning: {} circular dependencies detected",
                        cross_spec_result.summary.circular_dependency_count
                    );
                }
                if cross_spec_result.summary.global_invariant_conflict_count > 0 {
                    eprintln!(
                        "Warning: {} global invariant conflicts detected",
                        cross_spec_result.summary.global_invariant_conflict_count
                    );
                }
            }
            Err(e) => {
                eprintln!("  ⚠️  Failed to write cross-spec report: {}", e);
                std::process::exit(1);
            }
        }
    }
}

// =============================================================================
// P7-B: mumei run — build and execute a native binary
// =============================================================================
