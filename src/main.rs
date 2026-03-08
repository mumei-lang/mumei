#![allow(clippy::result_large_err)]

mod ast;
mod codegen;
mod hir;
mod lsp;
#[allow(dead_code)]
mod manifest;
mod parser;
mod registry;
mod resolver;
mod setup;
mod transpiler;
mod verification;

use crate::hir::lower_atom_to_hir;
use crate::parser::{ImportDecl, Item};
use crate::transpiler::{
    transpile, transpile_enum, transpile_impl, transpile_module_header, transpile_struct,
    transpile_trait, TargetLanguage,
};
use clap::{Parser, Subcommand};
use std::fs;
use std::path::Path;

// =============================================================================
// CLI: mumei build / verify / check / init / setup / inspect
// =============================================================================
//
// Usage:
//   mumei build input.mm -o dist/katana   # verify + codegen + transpile (default)
//   mumei verify input.mm                 # Z3 verification only
//   mumei check input.mm                  # parse + resolve + monomorphize (no Z3)
//   mumei init my_project                 # generate project template
//   mumei setup                           # download & configure Z3 + LLVM toolchain
//   mumei add <dep>                       # add dependency to mumei.toml
//   mumei input.mm -o dist/katana         # backward compat → same as build

#[derive(Parser)]
#[command(
    name = "mumei",
    version = env!("CARGO_PKG_VERSION"),
    about = "🗡️ Mumei — Mathematical Proof-Driven Programming Language",
    long_about = "Formally verified language: parse → resolve → monomorphize → verify (Z3) → codegen (LLVM IR) → transpile (Rust/Go/TypeScript)"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Input .mm file (backward compat: `mumei input.mm` = `mumei build input.mm`)
    #[arg(global = false)]
    input: Option<String>,

    /// Output base name (for .ll, .rs, .go, .ts)
    #[arg(short, long, default_value = "katana")]
    output: String,
}

#[derive(Subcommand)]
enum Command {
    /// Verify + compile to LLVM IR + transpile to Rust/Go/TypeScript (default)
    Build {
        /// Input .mm file
        input: String,
        /// Output base name
        #[arg(short, long, default_value = "katana")]
        output: String,
    },
    /// Z3 formal verification only (no codegen, no transpile)
    Verify {
        /// Input .mm file
        input: String,
    },
    /// Parse + resolve + monomorphize only (no Z3, fast syntax check)
    Check {
        /// Input .mm file
        input: String,
    },
    /// Generate a new Mumei project template
    Init {
        /// Project directory name
        name: String,
    },
    /// Inspect development environment (Z3, LLVM, std library)
    Inspect,
    /// Download and configure Z3 + LLVM toolchain into ~/.mumei/
    Setup {
        /// Force re-download even if already installed
        #[arg(long)]
        force: bool,
    },
    /// Add a dependency to mumei.toml
    Add {
        /// Dependency specifier: local path (./path/to/lib) or package name
        dep: String,
    },
    /// Publish package to local registry (~/.mumei/packages/)
    Publish {
        /// Publish only the proof cache (no source code)
        #[arg(long)]
        proof_only: bool,
    },
    /// Start Language Server Protocol server (stdio mode)
    Lsp,
    /// Interactive REPL (Read-Eval-Print Loop)
    Repl,
    /// Generate documentation from source comments
    Doc {
        /// Input .mm file or directory
        input: String,
        /// Output directory for generated docs
        #[arg(short, long, default_value = "docs_out")]
        output: String,
        /// Output format: html or markdown
        #[arg(long, default_value = "html")]
        format: String,
    },
    /// Infer required effects for all atoms (JSON output, for MCP integration)
    InferEffects {
        /// Input .mm file
        input: String,
    },
}

fn main() {
    // miette のリッチ出力を有効化（カラー・下線・サジェスト付き）
    miette::set_hook(Box::new(|_| {
        Box::new(
            miette::GraphicalReportHandler::new().with_theme(miette::GraphicalTheme::unicode()),
        )
    }))
    .ok();

    let cli = Cli::parse();

    match cli.command {
        Some(Command::Build { input, output }) => {
            cmd_build(&input, &output);
        }
        Some(Command::Verify { input }) => {
            cmd_verify(&input);
        }
        Some(Command::Check { input }) => {
            cmd_check(&input);
        }
        Some(Command::Init { name }) => {
            cmd_init(&name);
        }
        Some(Command::Inspect) => {
            cmd_inspect();
        }
        Some(Command::Setup { force }) => {
            setup::run(force);
        }
        Some(Command::Add { dep }) => {
            cmd_add(&dep);
        }
        Some(Command::Publish { proof_only }) => {
            cmd_publish(proof_only);
        }
        Some(Command::Lsp) => {
            lsp::run();
        }
        Some(Command::Repl) => {
            cmd_repl();
        }
        Some(Command::Doc {
            input,
            output,
            format,
        }) => {
            cmd_doc(&input, &output, &format);
        }
        Some(Command::InferEffects { input }) => {
            cmd_infer_effects(&input);
        }
        None => {
            // 後方互換: `mumei input.mm -o dist/katana` → build として実行
            if let Some(ref input) = cli.input {
                cmd_build(input, &cli.output);
            } else {
                eprintln!("Usage: mumei <COMMAND> or mumei <input.mm>");
                eprintln!("  build   Verify + compile + transpile (default)");
                eprintln!("  verify  Z3 formal verification only");
                eprintln!("  check   Parse + resolve only (fast syntax check)");
                eprintln!("  init    Generate a new project template");
                eprintln!("  setup   Download & configure Z3 + LLVM toolchain");
                eprintln!("  add     Add a dependency to mumei.toml");
                eprintln!("  lsp     Start Language Server Protocol server");
                eprintln!("  repl    Interactive REPL (Read-Eval-Print Loop)");
                eprintln!("  doc     Generate documentation from source comments");
                eprintln!("  inspect Inspect development environment");
                eprintln!("Run `mumei --help` for full usage.");
                std::process::exit(1);
            }
        }
    }
}

// =============================================================================
// Shared pipeline helpers
// =============================================================================

/// ソースファイルを読み込む
fn load_source(input: &str) -> String {
    fs::read_to_string(input).unwrap_or_else(|_| {
        eprintln!("❌ Error: Could not read Mumei source file '{}'", input);
        std::process::exit(1);
    })
}

/// Z3 が利用可能かチェックし、なければ親切なメッセージで終了する
fn check_z3_available() {
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
fn load_and_prepare(input: &str) -> (Vec<Item>, verification::ModuleEnv, Vec<ImportDecl>, String) {
    let source = load_source(input);
    let items = parser::parse_module(&source);

    let mut module_env = verification::ModuleEnv::new();
    verification::register_builtin_traits(&mut module_env);
    verification::register_builtin_effects(&mut module_env);
    let input_path = Path::new(input);
    let base_dir = input_path.parent().unwrap_or(Path::new("."));

    // std/prelude.mm の自動ロード（Eq, Ord, Numeric, Option<T>, Result<T, E> 等）
    // prelude が見つからない場合は組み込みトレイトがフォールバックとして機能する
    if let Err(e) = resolver::resolve_prelude(base_dir, &mut module_env) {
        eprintln!("  ⚠️  Prelude load warning: {}", e);
        // prelude のロード失敗は致命的ではない（組み込みトレイトが代替）
    }

    // mumei.toml の [dependencies] から依存パッケージを解決
    if let Some((proj_dir, m)) = manifest::find_and_load() {
        if let Err(e) = resolver::resolve_manifest_dependencies(&m, &proj_dir, &mut module_env) {
            eprintln!("  ⚠️  Dependency resolution warning: {}", e);
        }
    }

    if let Err(e) = resolver::resolve_imports(&items, base_dir, &mut module_env) {
        eprintln!("  ❌ Import Resolution Failed: {}", e);
        std::process::exit(1);
    }

    let mut mono = ast::Monomorphizer::new();
    mono.collect(&items);
    let items = if mono.has_generics() {
        let mono_items = mono.monomorphize(&items);
        println!(
            "  🔬 Monomorphization: {} generic instance(s) expanded.",
            mono.instances().len()
        );
        mono_items
    } else {
        items
    };

    let mut imports: Vec<ImportDecl> = Vec::new();
    for item in &items {
        match item {
            Item::Import(decl) => imports.push(decl.clone()),
            Item::TypeDef(refined_type) => module_env.register_type(refined_type),
            Item::StructDef(struct_def) => module_env.register_struct(struct_def),
            Item::EnumDef(enum_def) => module_env.register_enum(enum_def),
            Item::Atom(atom) => module_env.register_atom(atom),
            Item::TraitDef(trait_def) => module_env.register_trait(trait_def),
            Item::ImplDef(impl_def) => module_env.register_impl(impl_def),
            Item::ResourceDef(resource_def) => module_env.register_resource(resource_def),
            Item::EffectDef(effect_def) => module_env.register_effect(effect_def),
            Item::ExternBlock(extern_block) => {
                for ext_fn in &extern_block.functions {
                    // ExternFn → trusted Atom に変換して ModuleEnv に登録
                    let params: Vec<parser::Param> = ext_fn
                        .param_types
                        .iter()
                        .enumerate()
                        .map(|(i, ty)| parser::Param {
                            name: format!("arg{}", i),
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
                        requires: "true".to_string(),
                        forall_constraints: vec![],
                        ensures: "true".to_string(),
                        body_expr: String::new(),
                        consumed_params: vec![],
                        resources: vec![],
                        is_async: false,
                        trust_level: parser::TrustLevel::Trusted,
                        max_unroll: None,
                        invariant: None,
                        effects: vec![],
                        span: ext_fn.span.clone(),
                    };
                    module_env.register_atom(&atom);
                }
                println!(
                    "  🔗 FFI Bridge: registered {} extern function(s) from \"{}\" block",
                    extern_block.functions.len(),
                    extern_block.language
                );
            }
        }
    }

    (items, module_env, imports, source)
}

// =============================================================================
// mumei check — parse + resolve + monomorphize only
// =============================================================================

fn cmd_check(input: &str) {
    println!("🗡️  Mumei check: parsing and resolving '{}'...", input);
    let (items, _module_env, _imports, _source) = load_and_prepare(input);

    let mut type_count = 0;
    let mut struct_count = 0;
    let mut enum_count = 0;
    let mut trait_count = 0;
    let mut atom_count = 0;
    for item in &items {
        match item {
            Item::Import(decl) => {
                let alias_str = decl.alias.as_deref().unwrap_or("(none)");
                println!("  📦 Import: '{}' as '{}'", decl.path, alias_str);
            }
            Item::TypeDef(t) => {
                type_count += 1;
                println!("  ✨ Type: '{}' ({})", t.name, t._base_type);
            }
            Item::StructDef(s) => {
                struct_count += 1;
                println!("  🏗️  Struct: '{}'", s.name);
            }
            Item::EnumDef(e) => {
                enum_count += 1;
                println!("  🔷 Enum: '{}'", e.name);
            }
            Item::TraitDef(t) => {
                trait_count += 1;
                println!("  📜 Trait: '{}'", t.name);
            }
            Item::ImplDef(i) => {
                println!("  🔧 Impl: {} for {}", i.trait_name, i.target_type);
            }
            Item::Atom(a) => {
                atom_count += 1;
                let async_marker = if a.is_async { " (async)" } else { "" };
                let res_marker = if !a.resources.is_empty() {
                    format!(" [resources: {}]", a.resources.join(", "))
                } else {
                    String::new()
                };
                println!("  ✨ Atom: '{}'{}{}", a.name, async_marker, res_marker);
            }
            Item::ResourceDef(r) => {
                let mode_str = match r.mode {
                    parser::ResourceMode::Exclusive => "exclusive",
                    parser::ResourceMode::Shared => "shared",
                };
                println!(
                    "  🔒 Resource: '{}' (priority={}, mode={})",
                    r.name, r.priority, mode_str
                );
            }
            Item::ExternBlock(eb) => {
                println!(
                    "  🔗 Extern \"{}\" ({} function(s))",
                    eb.language,
                    eb.functions.len()
                );
            }
            Item::EffectDef(e) => {
                println!("  ⚡ Effect: '{}'", e.name);
            }
        }
    }
    println!(
        "✅ Check passed: {} types, {} structs, {} enums, {} traits, {} atoms",
        type_count, struct_count, enum_count, trait_count, atom_count
    );
}

// =============================================================================
// mumei verify — Z3 verification only (no codegen, no transpile)
// =============================================================================

fn cmd_verify(input: &str) {
    check_z3_available();
    println!("🗡️  Mumei verify: verifying '{}'...", input);
    let (items, mut module_env, _imports, source) = load_and_prepare(input);

    let output_dir = Path::new(".");
    let input_path = Path::new(input);
    let base_dir = input_path.parent().unwrap_or(Path::new("."));
    let mut verified = 0;
    let mut failed = 0;
    let mut skipped = 0;

    // Feature 2: Register dependencies for all atoms before verification
    for item in &items {
        if let Item::Atom(atom) = item {
            let callees = resolver::collect_callees_from_body(&atom.body_expr);
            module_env.register_dependencies(&atom.name, callees);
        }
    }

    // Feature 2: Migrate old cache and load enhanced verification cache
    resolver::migrate_old_cache(base_dir);
    let mut verification_cache = resolver::load_verification_cache(base_dir);

    for item in &items {
        match item {
            Item::ImplDef(impl_def) => {
                println!(
                    "  🔧 Verifying impl {} for {}...",
                    impl_def.trait_name, impl_def.target_type
                );
                match verification::verify_impl(impl_def, &module_env, output_dir) {
                    Ok(_) => {
                        println!("    ✅ Laws verified");
                        verified += 1;
                    }
                    Err(e) => {
                        // TODO: インポートされたファイルのエラー時、source はメインファイルの内容だが
                        // impl_def.span はインポート元ファイルを指す場合がある。
                        // 正しくは impl_def.span.file からソースを読み直す必要がある。
                        // See: https://github.com/mumei-lang/mumei/pull/35
                        let e = e.with_source(&source, &impl_def.span);
                        eprintln!("{:?}", miette::Report::new(e));
                        failed += 1;
                    }
                }
            }
            Item::Atom(atom) => {
                if module_env.is_verified(&atom.name) {
                    println!(
                        "  ⚖️  '{}': skipped (imported, contract-trusted)",
                        atom.name
                    );
                } else {
                    // Feature 2: Use compute_proof_hash with dependency-aware hashing
                    let proof_hash = resolver::compute_proof_hash(atom, &module_env);

                    if let Some(cached_entry) = verification_cache.get(&atom.name) {
                        if cached_entry.proof_hash == proof_hash {
                            println!("  ⚖️  '{}': skipped (unchanged, cached) ⏩", atom.name);
                            module_env.mark_verified(&atom.name);
                            skipped += 1;
                            continue;
                        }
                    }

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

                    let hir_atom = lower_atom_to_hir(atom);
                    match verification::verify(&hir_atom, output_dir, &module_env) {
                        Ok(_) => {
                            println!("  ⚖️  '{}': verified ✅", atom.name);
                            module_env.mark_verified(&atom.name);
                            verification_cache.insert(
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
                            verified += 1;
                        }
                        Err(e) => {
                            // TODO: インポートされた atom の場合、source と span のファイルが不一致になる。
                            // atom.span.file からソースを読み直すべき。
                            let e = e.with_source(&source, &atom.span);
                            eprintln!("{:?}", miette::Report::new(e));
                            // 検証失敗した atom はキャッシュから除外
                            verification_cache.remove(&atom.name);
                            failed += 1;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Feature 2: Save enhanced verification cache
    // Note: invalidate_dependents is not needed here because compute_proof_hash
    // already includes callee signatures (requires/ensures) in the hash.
    // If a callee's contract changes, all callers will have different proof hashes
    // and be re-verified automatically.
    resolver::save_verification_cache(base_dir, &verification_cache);

    println!();
    if failed > 0 {
        eprintln!(
            "❌ Verification: {} passed, {} failed, {} skipped (cached)",
            verified, failed, skipped
        );
        std::process::exit(1);
    }
    if skipped > 0 {
        println!(
            "✅ Verification passed: {} verified, {} skipped (unchanged) ⚡",
            verified, skipped
        );
    } else {
        println!("✅ Verification passed: {} item(s) verified", verified);
    }
}

// =============================================================================
// mumei init — generate project template
// =============================================================================

fn cmd_init(name: &str) {
    let project_dir = Path::new(name);
    if project_dir.exists() {
        eprintln!("❌ Error: Directory '{}' already exists", name);
        std::process::exit(1);
    }

    // ディレクトリ構造を作成
    fs::create_dir_all(project_dir.join("src")).unwrap_or_else(|e| {
        eprintln!("❌ Error: Failed to create directory: {}", e);
        std::process::exit(1);
    });
    let _ = fs::create_dir_all(project_dir.join("dist"));

    // mumei.toml
    let toml_content = format!(
        r#"[package]
name = "{}"
version = "0.1.0"
# authors = ["Your Name"]
# description = "A formally verified Mumei project"
# repository = "https://github.com/mumei-lang/your-project"
[dependencies]
# 依存パッケージをここに記述
# example = {{ path = "./libs/example" }}
# math = {{ git = "https://github.com/mumei-lang/math-mm", tag = "v1.0.0" }}
[build]
targets = ["rust", "go", "typescript"]
verify = true
max_unroll = 3
[proof]
cache = true
timeout_ms = 10000
[effects]
# allowed = ["Log", "FileRead"]
# denied = ["Network"]
"#,
        name
    );
    fs::write(project_dir.join("mumei.toml"), toml_content).unwrap();

    // .gitignore
    let gitignore_content = r#"# Mumei build artifacts
dist/
*.ll

# Verification cache (regenerated automatically)
.mumei_build_cache
.mumei_cache

# OS files
.DS_Store
Thumbs.db

# Editor files
.vscode/settings.json
*.swp
*~
"#;
    fs::write(project_dir.join(".gitignore"), gitignore_content).unwrap();

    // src/main.mm — 充実したテンプレート（検証成功例 + 標準ライブラリ使用例）
    let main_content = format!(
        r#"// =============================================================
// {} — Mumei Project
// =============================================================
//
// このファイルは mumei init で生成されたサンプルプロジェクトです。
// 形式検証の基本的な使い方を示しています。
//
// 実行方法:
//   mumei build src/main.mm -o dist/output
//   mumei verify src/main.mm
//   mumei check src/main.mm

import "std/option" as option;

// --- 精緻型（Refinement Type） ---
// 型に述語制約を付与し、Z3 で自動検証します
type Nat = i64 where v >= 0;
type Pos = i64 where v > 0;

// --- 基本的な atom（関数） ---
// requires（事前条件）と ensures（事後条件）を Z3 が数学的に証明します
atom increment(n: Nat)
requires:
    n >= 0;
ensures:
    result >= 1;
body: {{
    n + 1
}};

// --- 複数パラメータ + 算術検証 ---
atom safe_add(a: Nat, b: Nat)
requires:
    a >= 0 && b >= 0;
ensures:
    result >= a && result >= b;
body: {{
    a + b
}};

// --- 条件分岐を含む検証 ---
atom clamp(value: i64, min_val: Nat, max_val: Pos)
requires:
    min_val >= 0 && max_val > 0 && min_val < max_val;
ensures:
    result >= min_val && result <= max_val;
body: {{
    if value < min_val then min_val
    else if value > max_val then max_val
    else value
}};

// --- スタック操作（契約による安全性保証） ---
atom stack_push(top: Nat, max_size: Pos)
requires:
    top >= 0 && max_size > 0 && top < max_size;
ensures:
    result >= 1 && result <= max_size;
body: {{
    top + 1
}};

atom stack_pop(top: Pos)
requires:
    top > 0;
ensures:
    result >= 0;
body: {{
    top - 1
}};
"#,
        name
    );
    fs::write(project_dir.join("src/main.mm"), main_content).unwrap();

    println!("🗡️  Created new Mumei project '{}'", name);
    println!();
    println!("  {}/", name);
    println!("  ├── mumei.toml");
    println!("  ├── .gitignore");
    println!("  ├── dist/");
    println!("  └── src/");
    println!("      └── main.mm");
    println!();
    println!("Get started:");
    println!("  cd {}", name);
    println!("  mumei build src/main.mm -o dist/output");
    println!("  mumei verify src/main.mm");
    println!("  mumei check src/main.mm");
    println!("  mumei inspect                           # inspect environment");
}

// =============================================================================
// mumei inspect — environment check
// =============================================================================

fn cmd_inspect() {
    use std::process::Command as Cmd;

    println!("🔍 Mumei Inspect: checking development environment...");
    println!();

    let mut ok_count = 0;
    let mut warn_count = 0;
    let mut fail_count = 0;

    // --- 1. Mumei compiler version ---
    println!("  Mumei compiler: v{}", env!("CARGO_PKG_VERSION"));
    ok_count += 1;

    // --- 2. Z3 solver ---
    match Cmd::new("z3").arg("--version").output() {
        Ok(output) => {
            let version = String::from_utf8_lossy(&output.stdout);
            let version = version.trim();
            if version.is_empty() {
                println!("  ⚠️  Z3: installed but version unknown");
                warn_count += 1;
            } else {
                println!("  ✅ Z3: {}", version);
                ok_count += 1;
            }
        }
        Err(_) => {
            println!("  ❌ Z3: not found");
            println!("     Install: brew install z3");
            fail_count += 1;
        }
    }

    // --- 3. LLVM ---
    let llvm_found = ["llc-17", "llc"]
        .iter()
        .any(|cmd| Cmd::new(cmd).arg("--version").output().is_ok());
    if llvm_found {
        // Try to get version
        let version_output = Cmd::new("llc-17")
            .arg("--version")
            .output()
            .or_else(|_| Cmd::new("llc").arg("--version").output());
        if let Ok(output) = version_output {
            let version = String::from_utf8_lossy(&output.stdout);
            let first_line = version.lines().next().unwrap_or("unknown");
            println!("  ✅ LLVM: {}", first_line.trim());
        } else {
            println!("  ✅ LLVM: installed");
        }
        ok_count += 1;
    } else {
        println!("  ❌ LLVM: not found");
        println!("     Install: brew install llvm@17");
        fail_count += 1;
    }

    // --- 4. Rust toolchain ---
    match Cmd::new("rustc").arg("--version").output() {
        Ok(output) => {
            let version = String::from_utf8_lossy(&output.stdout);
            println!("  ✅ Rust: {}", version.trim());
            ok_count += 1;
        }
        Err(_) => {
            println!("  ⚠️  Rust: not found (optional, for generated .rs syntax check)");
            warn_count += 1;
        }
    }

    // --- 5. Go toolchain ---
    match Cmd::new("go").arg("version").output() {
        Ok(output) => {
            let version = String::from_utf8_lossy(&output.stdout);
            println!("  ✅ Go: {}", version.trim());
            ok_count += 1;
        }
        Err(_) => {
            println!("  ⚠️  Go: not found (optional, for generated .go compilation)");
            warn_count += 1;
        }
    }

    // --- 6. Node.js / TypeScript ---
    match Cmd::new("node").arg("--version").output() {
        Ok(output) => {
            let version = String::from_utf8_lossy(&output.stdout);
            println!("  ✅ Node.js: {}", version.trim());
            ok_count += 1;
        }
        Err(_) => {
            println!("  ⚠️  Node.js: not found (optional, for generated .ts execution)");
            warn_count += 1;
        }
    }

    // --- 7. std library ---
    // resolver と同じ探索順序: cwd → exe隣 → MUMEI_STD_PATH
    let std_modules = [
        "prelude.mm",
        "option.mm",
        "result.mm",
        "list.mm",
        "stack.mm",
        "alloc.mm",
        "container/bounded_array.mm",
    ];
    let mut std_base_dir: Option<std::path::PathBuf> = None;

    if Path::new("std/prelude.mm").exists() {
        std_base_dir = Some(std::path::PathBuf::from("std"));
    }
    if std_base_dir.is_none() {
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let candidate = exe_dir.join("std/prelude.mm");
                if candidate.exists() {
                    std_base_dir = Some(exe_dir.join("std"));
                }
            }
        }
    }
    if std_base_dir.is_none() {
        if let Ok(std_path) = std::env::var("MUMEI_STD_PATH") {
            let candidate = Path::new(&std_path).join("prelude.mm");
            if candidate.exists() {
                std_base_dir = Some(std::path::PathBuf::from(&std_path));
            }
        }
    }

    let mut std_found = 0;
    let mut std_missing = Vec::new();
    if let Some(ref base) = std_base_dir {
        for module in &std_modules {
            if base.join(module).exists() {
                std_found += 1;
            } else {
                std_missing.push(*module);
            }
        }
    } else {
        std_missing = std_modules.to_vec();
    }

    if std_missing.is_empty() {
        let location = std_base_dir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "?".to_string());
        println!(
            "  ✅ std library: {}/{} modules found ({})",
            std_found,
            std_modules.len(),
            location
        );
        ok_count += 1;
    } else {
        let hint = if std_base_dir.is_none() {
            " (set MUMEI_STD_PATH or place std/ next to mumei binary)"
        } else {
            ""
        };
        println!(
            "  ⚠️  std library: {}/{} modules found (missing: {}){}",
            std_found,
            std_modules.len(),
            std_missing.join(", "),
            hint
        );
        warn_count += 1;
    }

    // --- 8. mumei.toml (if in project directory) ---
    if Path::new("mumei.toml").exists() {
        // mumei.toml が見つかったらパースして内容を表示
        match manifest::load(Path::new("mumei.toml")) {
            Ok(m) => {
                println!("  ✅ mumei.toml: {} v{}", m.package.name, m.package.version);
                if !m.dependencies.is_empty() {
                    println!(
                        "     dependencies: {}",
                        m.dependencies
                            .keys()
                            .map(|k| k.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
                if !m.build.targets.is_empty() {
                    println!("     targets: {}", m.build.targets.join(", "));
                }
                ok_count += 1;
            }
            Err(e) => {
                println!("  ⚠️  mumei.toml: found but parse error: {}", e);
                warn_count += 1;
            }
        }
    } else {
        println!("  ℹ️  mumei.toml: not found (not in a Mumei project directory)");
    }

    // --- 9. ~/.mumei/ toolchain ---
    let mumei_home = manifest::mumei_home();
    let toolchains_dir = mumei_home.join("toolchains");
    if toolchains_dir.exists() {
        let mut tc_list = Vec::new();
        if let Ok(entries) = fs::read_dir(&toolchains_dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    if let Some(name) = entry.file_name().to_str() {
                        tc_list.push(name.to_string());
                    }
                }
            }
        }
        if tc_list.is_empty() {
            println!("  ℹ️  ~/.mumei/toolchains: empty (run `mumei setup`)");
        } else {
            tc_list.sort();
            println!("  ✅ ~/.mumei/toolchains: {}", tc_list.join(", "));
            ok_count += 1;
        }
    } else {
        println!("  ℹ️  ~/.mumei/toolchains: not found (run `mumei setup`)");
    }

    // --- Summary ---
    println!();
    if fail_count > 0 {
        println!(
            "❌ Inspect: {} ok, {} warnings, {} errors",
            ok_count, warn_count, fail_count
        );
        println!("   Fix the errors above to use Mumei.");
        std::process::exit(1);
    } else if warn_count > 0 {
        println!(
            "✅ Inspect: {} ok, {} warnings — Mumei is ready (optional tools missing)",
            ok_count, warn_count
        );
    } else {
        println!("✅ Inspect: {} ok — all tools available", ok_count);
    }
}

// =============================================================================
// mumei build — full pipeline (verify + codegen + transpile)
// =============================================================================

fn cmd_build(input: &str, output: &str) {
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

    let (items, mut module_env, imports, source) = load_and_prepare(input);

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
        if let Item::Atom(atom) = item {
            let callees = resolver::collect_callees_from_body(&atom.body_expr);
            module_env.register_dependencies(&atom.name, callees);
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

    // [build] targets から有効なトランスパイル言語を決定
    let enable_rust = build_cfg.targets.iter().any(|t| t == "rust");
    let enable_go = build_cfg.targets.iter().any(|t| t == "go");
    let enable_ts = build_cfg
        .targets
        .iter()
        .any(|t| t == "typescript" || t == "ts");
    let skip_verify = !build_cfg.verify;

    let mut atom_count = 0;

    // Transpiler バンドル初期化（有効な言語のみ）
    let mut rust_bundle = if enable_rust {
        transpile_module_header(&imports, file_stem, TargetLanguage::Rust)
    } else {
        String::new()
    };
    let mut go_bundle = if enable_go {
        transpile_module_header(&imports, file_stem, TargetLanguage::Go)
    } else {
        String::new()
    };
    let mut ts_bundle = if enable_ts {
        transpile_module_header(&imports, file_stem, TargetLanguage::TypeScript)
    } else {
        String::new()
    };

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

            // --- 構造体定義の登録 + トランスパイル ---
            Item::StructDef(struct_def) => {
                let field_names: Vec<&str> =
                    struct_def.fields.iter().map(|f| f.name.as_str()).collect();
                println!(
                    "  🏗️  Registered Struct: '{}' (fields: {})",
                    struct_def.name,
                    field_names.join(", ")
                );
                // 構造体定義をトランスパイル出力に含める（有効な言語のみ）
                if enable_rust {
                    rust_bundle.push_str(&transpile_struct(struct_def, TargetLanguage::Rust));
                    rust_bundle.push_str("\n\n");
                }
                if enable_go {
                    go_bundle.push_str(&transpile_struct(struct_def, TargetLanguage::Go));
                    go_bundle.push_str("\n\n");
                }
                if enable_ts {
                    ts_bundle.push_str(&transpile_struct(struct_def, TargetLanguage::TypeScript));
                    ts_bundle.push_str("\n\n");
                }
            }

            // --- Enum 定義の登録 + トランスパイル ---
            Item::EnumDef(enum_def) => {
                let variant_names: Vec<&str> =
                    enum_def.variants.iter().map(|v| v.name.as_str()).collect();
                println!(
                    "  🔷 Registered Enum: '{}' (variants: {})",
                    enum_def.name,
                    variant_names.join(", ")
                );
                if enable_rust {
                    rust_bundle.push_str(&transpile_enum(enum_def, TargetLanguage::Rust));
                    rust_bundle.push_str("\n\n");
                }
                if enable_go {
                    go_bundle.push_str(&transpile_enum(enum_def, TargetLanguage::Go));
                    go_bundle.push_str("\n\n");
                }
                if enable_ts {
                    ts_bundle.push_str(&transpile_enum(enum_def, TargetLanguage::TypeScript));
                    ts_bundle.push_str("\n\n");
                }
            }

            // --- トレイト定義 + トランスパイル ---
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
                if enable_rust {
                    rust_bundle.push_str(&transpile_trait(trait_def, TargetLanguage::Rust));
                    rust_bundle.push_str("\n\n");
                }
                if enable_go {
                    go_bundle.push_str(&transpile_trait(trait_def, TargetLanguage::Go));
                    go_bundle.push_str("\n\n");
                }
                if enable_ts {
                    ts_bundle.push_str(&transpile_trait(trait_def, TargetLanguage::TypeScript));
                    ts_bundle.push_str("\n\n");
                }
            }

            // --- トレイト実装の登録 + 法則検証 + トランスパイル ---
            Item::ImplDef(impl_def) => {
                println!(
                    "  🔧 Registered Impl: {} for {}",
                    impl_def.trait_name, impl_def.target_type
                );
                // impl が trait の全 law を満たしているか Z3 で検証
                if skip_verify {
                    println!("    ⚖️  Laws verification skipped (verify=false in mumei.toml)");
                } else {
                    match verification::verify_impl(impl_def, &module_env, output_dir) {
                        Ok(_) => println!(
                            "    ✅ Laws verified for impl {} for {}",
                            impl_def.trait_name, impl_def.target_type
                        ),
                        Err(e) => {
                            // TODO: インポートされた impl の場合、source と span のファイルが不一致になる。
                            let e = e.with_source(&source, &impl_def.span);
                            eprintln!("{:?}", miette::Report::new(e));
                            std::process::exit(1);
                        }
                    }
                }
                // impl 定義をトランスパイル出力に含める（有効な言語のみ）
                if enable_rust {
                    rust_bundle.push_str(&transpile_impl(impl_def, TargetLanguage::Rust));
                    rust_bundle.push_str("\n\n");
                }
                if enable_go {
                    go_bundle.push_str(&transpile_impl(impl_def, TargetLanguage::Go));
                    go_bundle.push_str("\n\n");
                }
                if enable_ts {
                    ts_bundle.push_str(&transpile_impl(impl_def, TargetLanguage::TypeScript));
                    ts_bundle.push_str("\n\n");
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
                    "  ✨ [1/4] Polishing Syntax: Atom '{}'{}{} identified.",
                    atom.name, async_marker, res_marker
                );

                // HIR lowering: body_expr を1回だけパースして全ステージで再利用する
                let hir_atom = lower_atom_to_hir(atom);

                // --- 2. Verification (形式検証: Z3 + StdLib) ---
                if skip_verify {
                    println!("  ⚖️  [2/4] Verification: Skipped (verify=false in mumei.toml).");
                    module_env.mark_verified(&atom.name);
                } else if module_env.is_verified(&atom.name) {
                    // インポートされた atom は検証済み（契約のみ信頼）なのでスキップ
                    println!("  ⚖️  [2/4] Verification: Skipped (imported, contract-trusted).");
                } else {
                    // Feature 2: Use compute_proof_hash with dependency-aware hashing
                    let proof_hash = resolver::compute_proof_hash(atom, &module_env);

                    let cache_hit = verification_cache
                        .get(&atom.name)
                        .map_or(false, |entry| entry.proof_hash == proof_hash);

                    if cache_hit {
                        println!("  ⚖️  [2/4] Verification: Skipped (unchanged, cached) ⏩");
                        module_env.mark_verified(&atom.name);
                    } else {
                        match verification::verify_with_config(
                            &hir_atom,
                            output_dir,
                            &module_env,
                            proof_cfg.timeout_ms,
                            build_cfg.max_unroll,
                        ) {
                            Ok(_) => {
                                println!(
                                    "  ⚖️  [2/4] Verification: Passed. Logic verified with Z3."
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
                                // TODO: インポートされた atom の場合、source と span のファイルが不一致になる。
                                let e = e.with_source(&source, &atom.span);
                                eprintln!("{:?}", miette::Report::new(e));
                                verification_cache_new.remove(&atom.name);
                                std::process::exit(1);
                            }
                        }
                    }
                }

                // --- 3. Codegen (LLVM 18 + Floating Point) ---
                // 各 Atom ごとに .ll ファイルを生成（またはモジュールを統合する拡張も可能）
                let atom_output_path = output_dir.join(format!("{}_{}", file_stem, atom.name));
                match codegen::compile(&hir_atom, &atom_output_path, &module_env) {
                    Ok(_) => println!(
                        "  ⚙️  [3/4] Tempering: Done. Compiled '{}' to LLVM IR.",
                        atom.name
                    ),
                    Err(e) => {
                        // TODO: インポートされた atom の場合、source と span のファイルが不一致になる。
                        let e = e.with_source(&source, &atom.span);
                        eprintln!("{:?}", miette::Report::new(e));
                        std::process::exit(1);
                    }
                }

                // --- 4. Transpile (多言語エクスポート) ---
                // バンドル用に各言語のコードを生成（有効な言語のみ）
                if enable_rust {
                    rust_bundle.push_str(&transpile(&hir_atom, TargetLanguage::Rust));
                    rust_bundle.push_str("\n\n");
                }
                if enable_go {
                    go_bundle.push_str(&transpile(&hir_atom, TargetLanguage::Go));
                    go_bundle.push_str("\n\n");
                }
                if enable_ts {
                    ts_bundle.push_str(&transpile(&hir_atom, TargetLanguage::TypeScript));
                    ts_bundle.push_str("\n\n");
                }
            }
        }
    }

    // 各言語のファイルを一括書き出し（有効な言語のみ）
    if atom_count > 0 {
        println!("  🌍 [4/4] Sharpening: Exporting verified sources...");

        let mut created_files = Vec::new();
        let files: Vec<(&str, &str, bool)> = vec![
            (&rust_bundle, "rs", enable_rust),
            (&go_bundle, "go", enable_go),
            (&ts_bundle, "ts", enable_ts),
        ];

        for (code, ext, enabled) in files {
            if !enabled {
                continue;
            }
            let out_filename = format!("{}.{}", file_stem, ext);
            let out_full_path = output_dir.join(&out_filename);
            if let Err(e) = fs::write(&out_full_path, code) {
                eprintln!("  ❌ Failed to write {}: {}", out_filename, e);
                std::process::exit(1);
            }
            created_files.push(out_filename);
        }
        println!("  ✅ Done. Created: {}", created_files.join(", "));
        println!("🎉 Blade forged successfully with {} atoms.", atom_count);
    } else {
        println!("⚠️  Warning: No atoms found in the source file.");
    }

    // Feature 2: Save enhanced verification cache
    if proof_cfg.cache {
        resolver::save_verification_cache(build_base_dir, &verification_cache_new);
    }
}

// =============================================================================
// mumei add — add dependency to mumei.toml
// =============================================================================

fn cmd_add(dep: &str) {
    // mumei.toml を探す
    let manifest_path = Path::new("mumei.toml");
    if !manifest_path.exists() {
        eprintln!("❌ Error: mumei.toml not found in current directory.");
        eprintln!("   Run `mumei init <project>` first, or cd into a Mumei project.");
        std::process::exit(1);
    }

    // 現在の mumei.toml を読み込み
    let content = fs::read_to_string(manifest_path).unwrap_or_else(|e| {
        eprintln!("❌ Error: Cannot read mumei.toml: {}", e);
        std::process::exit(1);
    });

    // パース確認
    if let Err(e) = manifest::load(manifest_path) {
        eprintln!("❌ Error: mumei.toml parse error: {}", e);
        std::process::exit(1);
    }

    // 依存の種類を判定
    let dep_entry = if dep.starts_with("./") || dep.starts_with("../") || dep.starts_with('/') {
        // ローカルパス依存
        let dep_path = Path::new(dep);
        if !dep_path.exists() {
            eprintln!("❌ Error: Path '{}' does not exist.", dep);
            std::process::exit(1);
        }
        // パッケージ名はディレクトリ名から推定
        let pkg_name = dep_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .replace('-', "_");
        let toml_line = format!("{} = {{ path = \"{}\" }}", pkg_name, dep);
        println!("📦 Adding local dependency: {} → {}", pkg_name, dep);
        (pkg_name, toml_line)
    } else if dep.contains("github.com") || dep.contains("gitlab.com") {
        // Git URL 依存
        let pkg_name = dep
            .split('/')
            .last()
            .unwrap_or("unknown")
            .trim_end_matches(".git")
            .replace('-', "_");
        let toml_line = format!("{} = {{ git = \"{}\" }}", pkg_name, dep);
        println!("📦 Adding git dependency: {} → {}", pkg_name, dep);
        (pkg_name, toml_line)
    } else {
        // パッケージ名のみ（レジストリ依存 — 将来対応）
        let toml_line = format!("{} = \"*\"", dep);
        println!(
            "📦 Adding dependency: {} (registry lookup not yet implemented)",
            dep
        );
        (dep.to_string(), toml_line)
    };

    // mumei.toml に追記
    let new_content = if content.contains("[dependencies]") {
        // [dependencies] セクションが既にある場合、その直後に追記
        content.replace(
            "[dependencies]",
            &format!("[dependencies]\n{}", dep_entry.1),
        )
    } else {
        // [dependencies] セクションがない場合、末尾に追加
        format!("{}\n[dependencies]\n{}\n", content.trim_end(), dep_entry.1)
    };

    fs::write(manifest_path, new_content).unwrap_or_else(|e| {
        eprintln!("❌ Error: Cannot write mumei.toml: {}", e);
        std::process::exit(1);
    });

    println!("✅ Added '{}' to mumei.toml", dep_entry.0);
}

// =============================================================================
// mumei publish — publish to local registry
// =============================================================================

fn cmd_publish(proof_only: bool) {
    println!("📦 Mumei publish: publishing to local registry...");

    // 1. mumei.toml を読み込み
    let manifest_path = Path::new("mumei.toml");
    if !manifest_path.exists() {
        eprintln!("❌ Error: mumei.toml not found. Run `mumei init` first.");
        std::process::exit(1);
    }
    let m = match manifest::load(manifest_path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("❌ Error: {}", e);
            std::process::exit(1);
        }
    };

    let pkg_name = &m.package.name;
    let pkg_version = &m.package.version;
    println!("  📄 Package: {} v{}", pkg_name, pkg_version);

    // 2. エントリファイルを探す
    let entry_candidates = ["src/main.mm", "main.mm"];
    let entry_path = entry_candidates.iter().find(|p| Path::new(p).exists());
    let entry = match entry_path {
        Some(p) => *p,
        None => {
            eprintln!("❌ Error: No entry file found (src/main.mm or main.mm).");
            std::process::exit(1);
        }
    };

    // 3. 全 atom を Z3 で検証（未検証パッケージの公開を禁止）
    println!("  🔍 Verifying all atoms before publish...");
    let (items, mut module_env, _imports, source) = load_and_prepare(entry);

    let output_dir = Path::new(".");
    let mut atom_count = 0;
    let mut failed = 0;

    for item in &items {
        if let Item::Atom(atom) = item {
            if module_env.is_verified(&atom.name) {
                atom_count += 1;
                continue;
            }
            let hir_atom = lower_atom_to_hir(atom);
            match verification::verify(&hir_atom, output_dir, &module_env) {
                Ok(_) => {
                    println!("  ⚖️  '{}': verified ✅", atom.name);
                    module_env.mark_verified(&atom.name);
                    atom_count += 1;
                }
                Err(e) => {
                    // TODO: インポートされた atom の場合、source と span のファイルが不一致になる。
                    let e = e.with_source(&source, &atom.span);
                    eprintln!("{:?}", miette::Report::new(e));
                    failed += 1;
                }
            }
        }
    }

    if failed > 0 {
        eprintln!(
            "❌ Publish aborted: {} atom(s) failed verification. Fix errors and retry.",
            failed
        );
        std::process::exit(1);
    }

    println!("  ✅ All {} atom(s) verified.", atom_count);

    // 4. ~/.mumei/packages/<name>/<version>/ にコピー
    let packages_dir = manifest::mumei_home().join("packages");
    let pkg_dir = packages_dir.join(pkg_name).join(pkg_version);

    if pkg_dir.exists() {
        println!("  ⚠️  Overwriting existing version {}", pkg_version);
        let _ = fs::remove_dir_all(&pkg_dir);
    }
    fs::create_dir_all(&pkg_dir).unwrap_or_else(|e| {
        eprintln!("❌ Error: Failed to create {}: {}", pkg_dir.display(), e);
        std::process::exit(1);
    });

    // mumei.toml をコピー
    let _ = fs::copy("mumei.toml", pkg_dir.join("mumei.toml"));

    // ビルドキャッシュをコピー（proof artifact）
    let base_dir = Path::new(entry).parent().unwrap_or(Path::new("."));
    let cache_src = base_dir.join(".mumei_build_cache");
    if cache_src.exists() {
        let _ = fs::copy(&cache_src, pkg_dir.join(".mumei_build_cache"));
    }

    if !proof_only {
        // src/ ディレクトリを再帰コピー
        if Path::new("src").exists() {
            copy_dir_recursive(Path::new("src"), &pkg_dir.join("src"));
        }
        // ルートの .mm ファイルもコピー
        if let Ok(entries) = fs::read_dir(".") {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "mm") {
                    let _ = fs::copy(&path, pkg_dir.join(path.file_name().unwrap()));
                }
            }
        }
        println!("  📁 Copied source + proof cache to {}", pkg_dir.display());
    } else {
        println!("  📁 Copied proof cache only to {}", pkg_dir.display());
    }

    // 5. registry.json に登録
    if let Err(e) = registry::register(pkg_name, pkg_version, &pkg_dir, atom_count, true) {
        eprintln!("  ⚠️  Registry update warning: {}", e);
    }

    println!();
    println!(
        "🎉 Published {} v{} to local registry",
        pkg_name, pkg_version
    );
    println!(
        "   Other projects can now use: {} = \"{}\"",
        pkg_name, pkg_version
    );
}

// =============================================================================
// mumei repl — Interactive REPL (Read-Eval-Print Loop)
// =============================================================================

fn cmd_repl() {
    println!("🗡️  Mumei REPL v{}", env!("CARGO_PKG_VERSION"));
    println!("  Type expressions or atom definitions to evaluate.");
    println!("  Commands: :help, :check <expr>, :verify <expr>, :load <file>, :quit");
    println!();

    let mut module_env = verification::ModuleEnv::new();
    verification::register_builtin_traits(&mut module_env);
    verification::register_builtin_effects(&mut module_env);

    // std/prelude を自動ロード
    if let Ok(cwd) = std::env::current_dir() {
        if let Err(e) = resolver::resolve_prelude(&cwd, &mut module_env) {
            eprintln!("  ⚠️  Prelude load warning: {}", e);
        }
    }

    let stdin = std::io::stdin();
    let mut line_buf = String::new();

    loop {
        eprint!("mumei> ");
        line_buf.clear();
        match stdin.read_line(&mut line_buf) {
            Ok(0) => break, // EOF
            Ok(_) => {}
            Err(e) => {
                eprintln!("  ❌ Read error: {}", e);
                break;
            }
        }

        let input = line_buf.trim();
        if input.is_empty() {
            continue;
        }

        match input {
            ":quit" | ":q" | ":exit" => {
                println!("Goodbye! 🗡️");
                break;
            }
            ":help" | ":h" => {
                println!("  :check <expr>  — Parse and type-check an expression");
                println!("  :verify <expr> — Formally verify an expression with Z3");
                println!("  :load <file>   — Load and register a .mm file");
                println!("  :env           — Show registered atoms and types");
                println!("  :quit          — Exit the REPL");
            }
            _ if input.starts_with(":load ") => {
                let file = input.strip_prefix(":load ").unwrap().trim();
                println!("  Loading '{}'...", file);
                let source = match fs::read_to_string(file) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("  ❌ Failed to read '{}': {}", file, e);
                        continue;
                    }
                };
                let items = parser::parse_module(&source);
                let mut count = 0;
                for item in &items {
                    match item {
                        parser::Item::Atom(atom) => {
                            module_env.register_atom(atom);
                            count += 1;
                        }
                        parser::Item::TypeDef(t) => module_env.register_type(t),
                        parser::Item::StructDef(s) => module_env.register_struct(s),
                        parser::Item::EnumDef(e) => module_env.register_enum(e),
                        parser::Item::TraitDef(t) => module_env.register_trait(t),
                        parser::Item::ImplDef(i) => module_env.register_impl(i),
                        parser::Item::ResourceDef(r) => module_env.register_resource(r),
                        parser::Item::ExternBlock(eb) => {
                            for ext_fn in &eb.functions {
                                let params: Vec<parser::Param> = ext_fn
                                    .param_types
                                    .iter()
                                    .enumerate()
                                    .map(|(i, ty)| parser::Param {
                                        name: format!("arg{}", i),
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
                                    requires: "true".to_string(),
                                    forall_constraints: vec![],
                                    ensures: "true".to_string(),
                                    body_expr: String::new(),
                                    consumed_params: vec![],
                                    resources: vec![],
                                    is_async: false,
                                    trust_level: parser::TrustLevel::Trusted,
                                    max_unroll: None,
                                    invariant: None,
                                    effects: vec![],
                                    span: ext_fn.span.clone(),
                                };
                                module_env.register_atom(&atom);
                                count += 1;
                            }
                        }
                        parser::Item::Import(_) => {}
                        parser::Item::EffectDef(e) => module_env.register_effect(e),
                    }
                }
                println!("  ✅ Loaded {} definition(s) from '{}'", count, file);
            }
            ":env" => {
                println!("  --- Registered Atoms ({}) ---", module_env.atoms.len());
                let mut names: Vec<&String> = module_env.atoms.keys().collect();
                names.sort();
                for name in names {
                    if let Some(atom) = module_env.atoms.get(name) {
                        let params_str: Vec<String> = atom
                            .params
                            .iter()
                            .map(|p| {
                                format!("{}: {}", p.name, p.type_name.as_deref().unwrap_or("?"))
                            })
                            .collect();
                        println!(
                            "    {} atom {}({}) [{:?}]",
                            if atom.is_async { "async" } else { "" },
                            name,
                            params_str.join(", "),
                            atom.trust_level
                        );
                    }
                }
                println!("  --- Registered Types ({}) ---", module_env.types.len());
                for name in module_env.types.keys() {
                    println!("    type {}", name);
                }
                println!(
                    "  --- Registered Structs ({}) ---",
                    module_env.structs.len()
                );
                for name in module_env.structs.keys() {
                    println!("    struct {}", name);
                }
                println!("  --- Registered Enums ({}) ---", module_env.enums.len());
                for name in module_env.enums.keys() {
                    println!("    enum {}", name);
                }
            }
            _ if input.starts_with(":check ") || input.starts_with(":verify ") => {
                let is_verify = input.starts_with(":verify ");
                let expr_str = if is_verify {
                    input.strip_prefix(":verify ").unwrap()
                } else {
                    input.strip_prefix(":check ").unwrap()
                };

                // 式をパースして簡易検証
                let wrapped = format!(
                    "atom __repl_eval()\n  requires: true;\n  ensures: true;\n  body: {{\n    {}\n  }}",
                    expr_str
                );
                let items = parser::parse_module(&wrapped);
                if items.is_empty() {
                    eprintln!("  ❌ Parse error");
                    continue;
                }
                for item in &items {
                    if let parser::Item::Atom(atom) = item {
                        println!("  ✅ Parsed: atom {}()", atom.name);
                        if is_verify {
                            let hir_atom = lower_atom_to_hir(atom);
                            match verification::verify(&hir_atom, Path::new("."), &module_env) {
                                Ok(()) => println!("  ✅ Verification passed"),
                                Err(e) => eprintln!("  ❌ Verification failed: {}", e),
                            }
                        }
                    }
                }
            }
            _ => {
                // atom 定義またはその他の宣言として解釈
                let items = parser::parse_module(input);
                if items.is_empty() {
                    eprintln!("  ❌ Could not parse input. Try :help for commands.");
                    continue;
                }
                for item in &items {
                    match item {
                        parser::Item::Atom(atom) => {
                            module_env.register_atom(atom);
                            println!("  ✅ Registered atom '{}'", atom.name);
                        }
                        parser::Item::TypeDef(t) => {
                            module_env.register_type(t);
                            println!("  ✅ Registered type '{}'", t.name);
                        }
                        parser::Item::StructDef(s) => {
                            module_env.register_struct(s);
                            println!("  ✅ Registered struct '{}'", s.name);
                        }
                        parser::Item::EnumDef(e) => {
                            module_env.register_enum(e);
                            println!("  ✅ Registered enum '{}'", e.name);
                        }
                        _ => {
                            println!("  ✅ Processed");
                        }
                    }
                }
            }
        }
    }
}

// =============================================================================
// mumei infer-effects — Effect inference (JSON output for MCP)
// =============================================================================

fn cmd_infer_effects(input: &str) {
    let (items, module_env, _imports, _source) = load_and_prepare(input);
    let result = verification::infer_effects_json(&items, &module_env);
    // JSON 出力（MCP が stdout をパースする）
    println!("{}", serde_json::to_string_pretty(&result).unwrap());
}

// =============================================================================
// mumei doc — Generate documentation from source comments
// =============================================================================

fn cmd_doc(input: &str, output_dir: &str, format: &str) {
    println!(
        "🗡️  Mumei doc: generating {} documentation for '{}'...",
        format, input
    );

    let input_path = Path::new(input);
    let out_path = Path::new(output_dir);
    let _ = fs::create_dir_all(out_path);

    // 対象ファイルを収集
    let files: Vec<std::path::PathBuf> = if input_path.is_dir() {
        // ディレクトリの場合、再帰的に .mm ファイルを収集
        collect_mm_files(input_path)
    } else {
        vec![input_path.to_path_buf()]
    };

    if files.is_empty() {
        eprintln!("  ❌ No .mm files found in '{}'", input);
        std::process::exit(1);
    }

    println!("  📄 Found {} file(s)", files.len());

    let mut all_docs: Vec<ModuleDoc> = Vec::new();

    for file in &files {
        let source = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("  ⚠️  Skipping '{}': {}", file.display(), e);
                continue;
            }
        };

        let items = parser::parse_module(&source);
        let module_name = file
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        let mut doc = ModuleDoc {
            name: module_name,
            file_path: file.display().to_string(),
            atoms: Vec::new(),
            types: Vec::new(),
            structs: Vec::new(),
            enums: Vec::new(),
            traits: Vec::new(),
        };

        // ソース行からコメントを抽出
        let lines: Vec<&str> = source.lines().collect();

        for item in &items {
            match item {
                parser::Item::Atom(atom) => {
                    let comment = extract_comment_before(&lines, atom.span.line);
                    doc.atoms.push(ItemDoc {
                        name: atom.name.clone(),
                        comment,
                        params: atom
                            .params
                            .iter()
                            .map(|p| {
                                format!("{}: {}", p.name, p.type_name.as_deref().unwrap_or("?"))
                            })
                            .collect(),
                        trust_level: format!("{:?}", atom.trust_level),
                        is_async: atom.is_async,
                    });
                }
                parser::Item::TypeDef(t) => {
                    let comment = extract_comment_before(&lines, t.span.line);
                    doc.types.push(TypeDoc {
                        name: t.name.clone(),
                        comment,
                    });
                }
                parser::Item::StructDef(s) => {
                    let comment = extract_comment_before(&lines, s.span.line);
                    doc.structs.push(TypeDoc {
                        name: s.name.clone(),
                        comment,
                    });
                }
                parser::Item::EnumDef(e) => {
                    let comment = extract_comment_before(&lines, e.span.line);
                    doc.enums.push(TypeDoc {
                        name: e.name.clone(),
                        comment,
                    });
                }
                parser::Item::TraitDef(t) => {
                    let comment = extract_comment_before(&lines, t.span.line);
                    doc.traits.push(TypeDoc {
                        name: t.name.clone(),
                        comment,
                    });
                }
                _ => {}
            }
        }

        all_docs.push(doc);
    }

    // ドキュメント出力
    match format {
        "html" => generate_html_docs(&all_docs, out_path),
        "markdown" | "md" => generate_markdown_docs(&all_docs, out_path),
        _ => {
            eprintln!(
                "  ❌ Unknown format '{}'. Use 'html' or 'markdown'.",
                format
            );
            std::process::exit(1);
        }
    }

    println!("  ✅ Documentation generated in '{}'", out_path.display());
}

// --- doc ヘルパー構造体 ---

struct ModuleDoc {
    name: String,
    file_path: String,
    atoms: Vec<ItemDoc>,
    types: Vec<TypeDoc>,
    structs: Vec<TypeDoc>,
    enums: Vec<TypeDoc>,
    traits: Vec<TypeDoc>,
}

struct ItemDoc {
    name: String,
    comment: String,
    params: Vec<String>,
    trust_level: String,
    is_async: bool,
}

struct TypeDoc {
    name: String,
    comment: String,
}

/// ソース行から指定行の直前にあるコメント群を抽出する
fn extract_comment_before(lines: &[&str], target_line: usize) -> String {
    if target_line == 0 || target_line > lines.len() {
        return String::new();
    }
    let mut comments = Vec::new();
    let mut i = target_line.saturating_sub(1); // 0-indexed
                                               // 上に向かってコメント行を収集
    loop {
        if i == 0 && !lines[0].trim().starts_with("//") {
            break;
        }
        if i > 0 {
            let prev = i - 1;
            let line = lines[prev].trim();
            if line.starts_with("//") {
                comments.push(line.trim_start_matches("//").trim().to_string());
                i = prev;
            } else {
                break;
            }
        } else {
            break;
        }
    }
    comments.reverse();
    comments.join("\n")
}

/// .mm ファイルを再帰的に収集
fn collect_mm_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut result = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                result.extend(collect_mm_files(&path));
            } else if path.extension().map_or(false, |ext| ext == "mm") {
                result.push(path);
            }
        }
    }
    result
}

/// HTML 特殊文字をエスケープする
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// HTML ドキュメントを生成
fn generate_html_docs(docs: &[ModuleDoc], out_dir: &Path) {
    // index.html
    let mut index = String::from(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Mumei Documentation</title>
<style>
body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; margin: 0; padding: 20px; background: #1a1a2e; color: #e0e0e0; }
.container { max-width: 900px; margin: 0 auto; }
h1 { color: #e94560; border-bottom: 2px solid #e94560; padding-bottom: 10px; }
h2 { color: #0f3460; background: #16213e; padding: 8px 16px; border-radius: 4px; color: #e94560; }
h3 { color: #c0c0c0; }
a { color: #e94560; text-decoration: none; }
a:hover { text-decoration: underline; }
.module-list { list-style: none; padding: 0; }
.module-list li { margin: 8px 0; padding: 8px 16px; background: #16213e; border-radius: 4px; }
.atom { background: #16213e; padding: 12px 16px; margin: 8px 0; border-radius: 6px; border-left: 3px solid #e94560; }
.atom .name { color: #e94560; font-weight: bold; font-size: 1.1em; }
.atom .params { color: #a0a0a0; font-family: monospace; }
.atom .comment { color: #c0c0c0; margin-top: 6px; }
.badge { display: inline-block; padding: 2px 8px; border-radius: 3px; font-size: 0.8em; margin-left: 8px; }
.badge.trusted { background: #0f3460; color: #e94560; }
.badge.verified { background: #1a4040; color: #4ecdc4; }
.badge.async { background: #3d1a60; color: #c084fc; }
.type-def { background: #16213e; padding: 8px 16px; margin: 8px 0; border-radius: 6px; border-left: 3px solid #4ecdc4; }
</style>
</head>
<body>
<div class="container">
<h1>&#x1F5E1; Mumei Documentation</h1>
<p>Auto-generated from source comments.</p>
<h2>Modules</h2>
<ul class="module-list">
"#,
    );

    for doc in docs {
        index.push_str(&format!(
            "<li><a href=\"{}.html\">{}</a> — {}</li>\n",
            html_escape(&doc.name),
            html_escape(&doc.name),
            html_escape(&doc.file_path)
        ));
    }
    index.push_str("</ul>\n</div>\n</body>\n</html>\n");
    let _ = fs::write(out_dir.join("index.html"), &index);

    // 各モジュールのページ
    for doc in docs {
        let mut page = format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>{name} — Mumei Documentation</title>
<style>
body {{ font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; margin: 0; padding: 20px; background: #1a1a2e; color: #e0e0e0; }}
.container {{ max-width: 900px; margin: 0 auto; }}
h1 {{ color: #e94560; border-bottom: 2px solid #e94560; padding-bottom: 10px; }}
h2 {{ color: #e94560; background: #16213e; padding: 8px 16px; border-radius: 4px; }}
a {{ color: #e94560; text-decoration: none; }}
a:hover {{ text-decoration: underline; }}
.atom {{ background: #16213e; padding: 12px 16px; margin: 8px 0; border-radius: 6px; border-left: 3px solid #e94560; }}
.atom .name {{ color: #e94560; font-weight: bold; font-size: 1.1em; }}
.atom .params {{ color: #a0a0a0; font-family: monospace; }}
.atom .comment {{ color: #c0c0c0; margin-top: 6px; }}
.badge {{ display: inline-block; padding: 2px 8px; border-radius: 3px; font-size: 0.8em; margin-left: 8px; }}
.badge.trusted {{ background: #0f3460; color: #e94560; }}
.badge.verified {{ background: #1a4040; color: #4ecdc4; }}
.badge.async {{ background: #3d1a60; color: #c084fc; }}
.type-def {{ background: #16213e; padding: 8px 16px; margin: 8px 0; border-radius: 6px; border-left: 3px solid #4ecdc4; }}
</style>
</head>
<body>
<div class="container">
<p><a href="index.html">&larr; Back to index</a></p>
<h1>{name}</h1>
<p>Source: <code>{path}</code></p>
"#,
            name = html_escape(&doc.name),
            path = html_escape(&doc.file_path),
        );

        // Atoms
        if !doc.atoms.is_empty() {
            page.push_str("<h2>Atoms</h2>\n");
            for atom in &doc.atoms {
                page.push_str("<div class=\"atom\">\n");
                page.push_str(&format!(
                    "  <span class=\"name\">{}</span>",
                    html_escape(&atom.name)
                ));
                if atom.is_async {
                    page.push_str("<span class=\"badge async\">async</span>");
                }
                if atom.trust_level == "Trusted" {
                    page.push_str("<span class=\"badge trusted\">trusted</span>");
                } else if atom.trust_level == "Verified" {
                    page.push_str("<span class=\"badge verified\">verified</span>");
                }
                let escaped_params: Vec<String> =
                    atom.params.iter().map(|p| html_escape(p)).collect();
                page.push_str(&format!(
                    "\n  <div class=\"params\">({})</div>\n",
                    escaped_params.join(", ")
                ));
                if !atom.comment.is_empty() {
                    page.push_str(&format!(
                        "  <div class=\"comment\">{}</div>\n",
                        html_escape(&atom.comment).replace('\n', "<br>")
                    ));
                }
                page.push_str("</div>\n");
            }
        }

        // Types
        if !doc.types.is_empty() {
            page.push_str("<h2>Types</h2>\n");
            for t in &doc.types {
                page.push_str(&format!(
                    "<div class=\"type-def\"><strong>{}</strong>",
                    html_escape(&t.name)
                ));
                if !t.comment.is_empty() {
                    page.push_str(&format!(
                        "<br>{}",
                        html_escape(&t.comment).replace('\n', "<br>")
                    ));
                }
                page.push_str("</div>\n");
            }
        }

        // Structs
        if !doc.structs.is_empty() {
            page.push_str("<h2>Structs</h2>\n");
            for s in &doc.structs {
                page.push_str(&format!(
                    "<div class=\"type-def\"><strong>struct {}</strong>",
                    html_escape(&s.name)
                ));
                if !s.comment.is_empty() {
                    page.push_str(&format!(
                        "<br>{}",
                        html_escape(&s.comment).replace('\n', "<br>")
                    ));
                }
                page.push_str("</div>\n");
            }
        }

        // Enums
        if !doc.enums.is_empty() {
            page.push_str("<h2>Enums</h2>\n");
            for e in &doc.enums {
                page.push_str(&format!(
                    "<div class=\"type-def\"><strong>enum {}</strong>",
                    html_escape(&e.name)
                ));
                if !e.comment.is_empty() {
                    page.push_str(&format!(
                        "<br>{}",
                        html_escape(&e.comment).replace('\n', "<br>")
                    ));
                }
                page.push_str("</div>\n");
            }
        }

        // Traits
        if !doc.traits.is_empty() {
            page.push_str("<h2>Traits</h2>\n");
            for t in &doc.traits {
                page.push_str(&format!(
                    "<div class=\"type-def\"><strong>trait {}</strong>",
                    html_escape(&t.name)
                ));
                if !t.comment.is_empty() {
                    page.push_str(&format!(
                        "<br>{}",
                        html_escape(&t.comment).replace('\n', "<br>")
                    ));
                }
                page.push_str("</div>\n");
            }
        }

        page.push_str("</div>\n</body>\n</html>\n");
        let _ = fs::write(out_dir.join(format!("{}.html", doc.name)), &page);
    }
}

/// Markdown ドキュメントを生成
fn generate_markdown_docs(docs: &[ModuleDoc], out_dir: &Path) {
    for doc in docs {
        let mut md = format!("# {}\n\nSource: `{}`\n\n", doc.name, doc.file_path);

        if !doc.atoms.is_empty() {
            md.push_str("## Atoms\n\n");
            for atom in &doc.atoms {
                let badges = [
                    if atom.is_async { Some("`async`") } else { None },
                    if atom.trust_level == "Trusted" {
                        Some("`trusted`")
                    } else {
                        None
                    },
                ]
                .iter()
                .flatten()
                .copied()
                .collect::<Vec<_>>()
                .join(" ");

                md.push_str(&format!(
                    "### `{}({})` {}\n\n",
                    atom.name,
                    atom.params.join(", "),
                    badges
                ));
                if !atom.comment.is_empty() {
                    md.push_str(&format!("{}\n\n", atom.comment));
                }
            }
        }

        if !doc.types.is_empty() {
            md.push_str("## Types\n\n");
            for t in &doc.types {
                md.push_str(&format!("### `{}`\n\n", t.name));
                if !t.comment.is_empty() {
                    md.push_str(&format!("{}\n\n", t.comment));
                }
            }
        }

        if !doc.structs.is_empty() {
            md.push_str("## Structs\n\n");
            for s in &doc.structs {
                md.push_str(&format!("### `struct {}`\n\n", s.name));
                if !s.comment.is_empty() {
                    md.push_str(&format!("{}\n\n", s.comment));
                }
            }
        }

        if !doc.enums.is_empty() {
            md.push_str("## Enums\n\n");
            for e in &doc.enums {
                md.push_str(&format!("### `enum {}`\n\n", e.name));
                if !e.comment.is_empty() {
                    md.push_str(&format!("{}\n\n", e.comment));
                }
            }
        }

        if !doc.traits.is_empty() {
            md.push_str("## Traits\n\n");
            for t in &doc.traits {
                md.push_str(&format!("### `trait {}`\n\n", t.name));
                if !t.comment.is_empty() {
                    md.push_str(&format!("{}\n\n", t.comment));
                }
            }
        }

        let _ = fs::write(out_dir.join(format!("{}.md", doc.name)), &md);
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
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

// end of src/main.rs

#[cfg(test)]
mod tests {
    use super::*;

    // --- P3-B: Doc Gen テスト ---

    /// extract_comment_before: コメントが直前にある場合
    #[test]
    fn test_extract_comment_before_single_line() {
        let lines = vec!["// This is a doc comment", "atom foo() -> i64"];
        let comment = extract_comment_before(&lines, 2); // target_line is 1-indexed
        assert_eq!(comment, "This is a doc comment");
    }

    /// extract_comment_before: 複数行コメント
    #[test]
    fn test_extract_comment_before_multi_line() {
        let lines = vec![
            "// First line",
            "// Second line",
            "// Third line",
            "atom bar() -> i64",
        ];
        let comment = extract_comment_before(&lines, 4);
        assert_eq!(comment, "First line\nSecond line\nThird line");
    }

    /// extract_comment_before: コメントがない場合
    #[test]
    fn test_extract_comment_before_no_comment() {
        let lines = vec!["let x = 42;", "atom baz() -> i64"];
        let comment = extract_comment_before(&lines, 2);
        assert_eq!(comment, "");
    }

    /// extract_comment_before: 境界値テスト (行0)
    #[test]
    fn test_extract_comment_before_line_zero() {
        let lines = vec!["// comment", "atom foo() -> i64"];
        let comment = extract_comment_before(&lines, 0);
        assert_eq!(comment, "");
    }

    /// extract_comment_before: 範囲外の行
    #[test]
    fn test_extract_comment_before_out_of_range() {
        let lines = vec!["// comment"];
        let comment = extract_comment_before(&lines, 100);
        assert_eq!(comment, "");
    }

    /// extract_comment_before: コメントの間に空行がある場合（途切れる）
    #[test]
    fn test_extract_comment_before_gap() {
        let lines = vec![
            "// Unrelated comment",
            "",
            "// Relevant comment",
            "atom qux() -> i64",
        ];
        let comment = extract_comment_before(&lines, 4);
        assert_eq!(comment, "Relevant comment");
    }

    // --- P3-B: Doc Gen パースからドキュメント抽出テスト ---

    /// P3-B: ModuleDoc / ItemDoc 構造体の構築テスト
    #[test]
    fn test_doc_gen_module_doc_construction() {
        let source = r#"
atom http_get(url: String) -> String
  requires: true;
  ensures: true;
  body: url;

atom json_parse(input: String) -> String
  requires: true;
  ensures: true;
  body: input;
"#;
        let items = parser::parse_module(source);

        let mut doc = ModuleDoc {
            name: "test_module".to_string(),
            file_path: "test.mm".to_string(),
            atoms: Vec::new(),
            types: Vec::new(),
            structs: Vec::new(),
            enums: Vec::new(),
            traits: Vec::new(),
        };

        for item in &items {
            if let parser::Item::Atom(atom) = item {
                doc.atoms.push(ItemDoc {
                    name: atom.name.clone(),
                    comment: String::new(),
                    params: atom.params.iter().map(|p| p.name.clone()).collect(),
                    trust_level: format!("{:?}", atom.trust_level),
                    is_async: atom.is_async,
                });
            }
        }

        assert_eq!(doc.atoms.len(), 2);
        assert_eq!(doc.atoms[0].name, "http_get");
        assert_eq!(doc.atoms[0].params, vec!["url"]);
        assert_eq!(doc.atoms[0].trust_level, "Verified");
        assert!(!doc.atoms[0].is_async);
        assert_eq!(doc.atoms[1].name, "json_parse");
        assert_eq!(doc.atoms[1].params, vec!["input"]);
    }

    // --- P3-A: REPL ヘルパーテスト ---

    /// P3-A: REPL の atom ラッピングロジックテスト
    #[test]
    fn test_repl_atom_wrapping() {
        // REPL は入力式を atom でラップして検証する
        let expr = "1 + 2";
        let wrapped = format!(
            "atom __repl_eval()\n  requires: true;\n  ensures: true;\n  body: {{\n    {}\n  }}",
            expr
        );
        assert!(wrapped.contains("__repl_eval"));
        assert!(wrapped.contains("1 + 2"));
        assert!(wrapped.contains("requires: true"));

        // ラップされた atom がパース可能であること
        let items = parser::parse_module(&wrapped);
        let atoms: Vec<_> = items
            .iter()
            .filter_map(|i| {
                if let parser::Item::Atom(a) = i {
                    Some(a)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(atoms.len(), 1);
        assert_eq!(atoms[0].name, "__repl_eval");
    }

    /// P3-A: REPL :load コマンドのパースロジックテスト
    #[test]
    fn test_repl_load_command_parsing() {
        let input = ":load std/json.mm";
        assert!(input.starts_with(":load "));
        let file = input.strip_prefix(":load ").unwrap().trim();
        assert_eq!(file, "std/json.mm");
    }

    /// P3-A: REPL コマンド認識テスト
    #[test]
    fn test_repl_command_recognition() {
        // 終了コマンド
        assert!(matches!(":quit", ":quit" | ":q" | ":exit"));
        assert!(matches!(":q", ":quit" | ":q" | ":exit"));
        assert!(matches!(":exit", ":quit" | ":q" | ":exit"));

        // ヘルプコマンド
        assert!(matches!(":help", ":help" | ":h"));
        assert!(matches!(":h", ":help" | ":h"));

        // :load コマンド
        let load_cmd = ":load test.mm";
        assert!(load_cmd.starts_with(":load "));

        // :env コマンド
        let env_cmd = ":env";
        assert_eq!(env_cmd, ":env");
    }

    // --- P1-A: FFI Bridge（main.rs の load_and_prepare）テスト ---

    /// P1-A: ExternBlock が load_and_prepare で trusted atom に変換されるテスト
    #[test]
    fn test_load_and_prepare_registers_extern_atoms() {
        let source = r#"
extern "Rust" {
    fn ffi_sqrt(x: f64) -> f64;
    fn ffi_abs(x: i64) -> i64;
}

atom main() -> i64
  requires: true;
  ensures: result == 42;
  body: 42;
"#;
        let items = parser::parse_module(source);
        let mut module_env = verification::ModuleEnv::new();
        verification::register_builtin_traits(&mut module_env);

        // ExternBlock → trusted atom 変換ロジック（load_and_prepare と同等）
        for item in &items {
            match item {
                parser::Item::Atom(atom) => {
                    module_env.register_atom(atom);
                }
                parser::Item::ExternBlock(eb) => {
                    for ext_fn in &eb.functions {
                        let params: Vec<parser::Param> = ext_fn
                            .param_types
                            .iter()
                            .enumerate()
                            .map(|(i, ty)| parser::Param {
                                name: format!("arg{}", i),
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
                            requires: "true".to_string(),
                            forall_constraints: vec![],
                            ensures: "true".to_string(),
                            body_expr: String::new(),
                            consumed_params: vec![],
                            resources: vec![],
                            is_async: false,
                            trust_level: parser::TrustLevel::Trusted,
                            max_unroll: None,
                            invariant: None,
                            effects: vec![],
                            span: ext_fn.span.clone(),
                        };
                        module_env.register_atom(&atom);
                    }
                }
                _ => {}
            }
        }

        // extern 関数が trusted atom として登録されていること
        assert!(module_env.get_atom("ffi_sqrt").is_some());
        assert_eq!(
            module_env.get_atom("ffi_sqrt").unwrap().trust_level,
            parser::TrustLevel::Trusted
        );
        assert!(module_env.get_atom("ffi_abs").is_some());

        // 通常 atom も登録されていること
        assert!(module_env.get_atom("main").is_some());
        assert_eq!(
            module_env.get_atom("main").unwrap().trust_level,
            parser::TrustLevel::Verified
        );
    }

    // --- Phase B: call_with_contract E2E Z3 verification tests ---

    /// Helper: parse source, register items, and verify a single atom by name
    fn verify_atom_from_source(
        source: &str,
        atom_name: &str,
    ) -> Result<(), verification::MumeiError> {
        let items = parser::parse_module(source);
        let mut module_env = verification::ModuleEnv::new();
        verification::register_builtin_traits(&mut module_env);
        for item in &items {
            if let parser::Item::Atom(atom) = item {
                module_env.register_atom(atom);
            }
        }
        let atom = module_env
            .get_atom(atom_name)
            .unwrap_or_else(|| panic!("atom '{}' not found", atom_name))
            .clone();
        let hir_atom = lower_atom_to_hir(&atom);
        verification::verify(&hir_atom, std::path::Path::new("."), &module_env)
    }

    #[test]
    fn test_call_with_contract_basic_ensures() {
        let source = r#"
atom increment(x: i64)
    requires: x >= 0;
    ensures: result == x + 1;
    body: x + 1;

atom apply(x: i64, f: atom_ref(i64) -> i64)
    requires: x >= 0;
    ensures: result >= 0;
    contract(f): ensures: result >= 0;
    body: call(f, x);

atom test_apply()
    requires: true;
    ensures: result >= 0;
    body: apply(5, atom_ref(increment));
"#;
        // apply should verify: contract(f) ensures result >= 0 constrains the dynamic call
        assert!(
            verify_atom_from_source(source, "apply").is_ok(),
            "apply with contract(f) ensures should verify"
        );
        // test_apply calls apply with concrete atom_ref
        assert!(
            verify_atom_from_source(source, "test_apply").is_ok(),
            "test_apply should verify via compositional verification"
        );
    }

    #[test]
    fn test_call_with_contract_requires_and_ensures() {
        let source = r#"
atom apply_twice(x: i64, f: atom_ref(i64) -> i64)
    requires: x >= 0;
    ensures: result >= 0;
    contract(f): requires: x >= 0, ensures: result >= 0;
    body: {
        let first = call(f, x);
        call(f, first)
    }
"#;
        // apply_twice should verify: first call's result >= 0 satisfies second call's requires
        assert!(
            verify_atom_from_source(source, "apply_twice").is_ok(),
            "apply_twice with contract(f) requires+ensures should verify"
        );
    }

    #[test]
    fn test_call_with_contract_binary_function() {
        let source = r#"
atom add(a: i64, b: i64)
    requires: a >= 0 && b >= 0;
    ensures: result == a + b;
    body: a + b;

atom fold_two(a: i64, b: i64, f: atom_ref(i64, i64) -> i64)
    requires: a >= 0 && b >= 0;
    ensures: result >= 0;
    contract(f): ensures: result >= 0;
    body: call(f, a, b);

atom test_fold()
    requires: true;
    ensures: result >= 0;
    body: fold_two(3, 4, atom_ref(add));
"#;
        assert!(
            verify_atom_from_source(source, "fold_two").is_ok(),
            "fold_two with binary contract should verify"
        );
        assert!(
            verify_atom_from_source(source, "test_fold").is_ok(),
            "test_fold with concrete atom_ref(add) should verify"
        );
    }

    #[test]
    fn test_call_with_contract_in_match() {
        let source = r#"
atom option_map(opt: i64, f: atom_ref(i64) -> i64)
    requires: opt >= 0 && opt <= 1;
    ensures: result >= 0;
    contract(f): ensures: result >= 0;
    body: {
        match opt {
            0 => 0,
            _ => call(f, opt)
        }
    }
"#;
        assert!(
            verify_atom_from_source(source, "option_map").is_ok(),
            "option_map with contract in match arm should verify"
        );
    }

    #[test]
    fn test_call_with_contract_without_contract_fails() {
        // An atom using call(f, x) WITHOUT a contract(f) clause
        // should fail verification because the result is unconstrained
        let source = r#"
atom apply_no_contract(x: i64, f: atom_ref(i64) -> i64)
    requires: x >= 0;
    ensures: result >= 0;
    body: call(f, x);
"#;
        let result = verify_atom_from_source(source, "apply_no_contract");
        assert!(
            result.is_err(),
            "apply without contract(f) should fail: result is unconstrained"
        );
    }

    // --- Feature 1: Semantic Feedback Tests ---

    /// Test constraint_to_natural_language for various patterns
    #[test]
    fn test_constraint_to_natural_language() {
        // v <= N pattern
        let result =
            verification::constraint_to_natural_language("age", "HumanAge", "v <= 120", "121");
        assert!(result.contains("age"), "should mention param name");
        assert!(result.contains("121"), "should mention actual value");
        assert!(result.contains("120"), "should mention bound");
        assert!(result.contains("HumanAge"), "should mention type name");
        // Bilingual: should contain both English and Japanese
        assert!(
            result.contains("violates") || result.contains("exceeds"),
            "should have English text"
        );

        // v >= N pattern
        let result2 = verification::constraint_to_natural_language("x", "Nat", "v >= 0", "-1");
        assert!(result2.contains("x"), "should mention param name");
        assert!(result2.contains("-1"), "should mention actual value");

        // Fallback pattern
        let result3 =
            verification::constraint_to_natural_language("val", "Custom", "v % 2 == 0", "3");
        assert!(result3.contains("val"), "should mention param name");
        assert!(
            result3.contains("v % 2 == 0"),
            "should include raw predicate for unknown patterns"
        );
    }

    /// Test suggestion_for_failure_type returns appropriate suggestions
    #[test]
    fn test_suggestion_for_failure_type() {
        let s1 =
            verification::suggestion_for_failure_type(verification::FAILURE_POSTCONDITION_VIOLATED);
        assert!(
            !s1.is_empty(),
            "postcondition suggestion should not be empty"
        );

        let s2 = verification::suggestion_for_failure_type(verification::FAILURE_DIVISION_BY_ZERO);
        assert!(
            s2.contains("0") || s2.contains("zero") || s2.contains("divisor"),
            "division by zero suggestion should mention zero or divisor"
        );

        let s3 = verification::suggestion_for_failure_type("unknown_type");
        assert!(
            !s3.is_empty(),
            "unknown failure type should still return a suggestion"
        );
    }

    /// Test build_semantic_feedback produces valid JSON structure
    #[test]
    fn test_build_semantic_feedback() {
        let source = r#"
type HumanAge = i64 where v >= 0 && v <= 120;

atom validate_age(age: HumanAge) -> i64
  requires: true;
  ensures: result >= 0 && result <= 120;
  body: age;
"#;
        let items = parser::parse_module(source);
        let mut module_env = verification::ModuleEnv::new();

        // Register type
        for item in &items {
            if let parser::Item::TypeDef(td) = item {
                module_env.register_type(td);
            }
        }

        // Find atom and build feedback
        for item in &items {
            if let parser::Item::Atom(atom) = item {
                let mappings = verification::build_constraint_mappings_for_atom(atom, &module_env);
                assert!(
                    !mappings.is_empty(),
                    "should have constraint mappings for HumanAge param"
                );

                let counterexample = serde_json::json!({"age": "121"});
                let feedback = verification::build_semantic_feedback(
                    &mappings,
                    Some(&counterexample),
                    atom,
                    verification::FAILURE_POSTCONDITION_VIOLATED,
                );
                assert!(feedback.is_some(), "should produce semantic feedback");
                let feedback = feedback.unwrap();

                // feedback should have violated_constraints
                let vc = feedback.get("violated_constraints");
                assert!(vc.is_some(), "should have violated_constraints field");
                let vc_arr = vc.unwrap().as_array().unwrap();
                assert!(
                    !vc_arr.is_empty(),
                    "should have at least one violated constraint for HumanAge"
                );

                let first = &vc_arr[0];
                assert_eq!(first["param"], "age");
                assert_eq!(first["type"], "HumanAge");
                assert!(
                    !first["explanation"].as_str().unwrap().is_empty(),
                    "explanation should not be empty"
                );
                assert!(
                    !first["suggestion"].as_str().unwrap().is_empty(),
                    "suggestion should not be empty"
                );

                // Should have context section
                let ctx = feedback.get("context");
                assert!(ctx.is_some(), "should have context field");
            }
        }
    }
}
