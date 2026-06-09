use mumei_core::emitter::Emitter;
use mumei_core::hir::lower_atom_to_hir_with_env;
use mumei_core::parser::Item;
use mumei_core::{emitter, hir, parser, verification};
use std::path::{Path, PathBuf};

pub(crate) fn collect_extern_blocks(
    module_env: &verification::ModuleEnv,
) -> Vec<parser::ExternBlock> {
    module_env.extern_blocks.clone()
}
pub(crate) fn dispatch_emit(
    target: &emitter::EmitTarget,
    external_emitter: Option<&(dyn Emitter + Send + Sync)>,
    hir_atom: &hir::HirAtom,
    output_path: &std::path::Path,
    module_env: &verification::ModuleEnv,
    extern_blocks: &[parser::ExternBlock],
) -> verification::MumeiResult<Vec<emitter::Artifact>> {
    match target {
        emitter::EmitTarget::LlvmIr => {
            mumei_emit_llvm::LlvmEmitter.emit(hir_atom, output_path, module_env, extern_blocks)
        }
        emitter::EmitTarget::CHeader => {
            emitter::CHeaderEmitter.emit(hir_atom, output_path, module_env, extern_blocks)
        }
        emitter::EmitTarget::VerifiedJson => mumei_emit_json::VerifiedJsonEmitter.emit(
            hir_atom,
            output_path,
            module_env,
            extern_blocks,
        ),
        emitter::EmitTarget::DecidableMetrics => Ok(vec![]),
        emitter::EmitTarget::ProofBook => mumei_emit_proofbook::ProofBookEmitter.emit(
            hir_atom,
            output_path,
            module_env,
            extern_blocks,
        ),
        emitter::EmitTarget::ProofCert | emitter::EmitTarget::EscalationBundle => {
            // Certificate-like emits are handled at the cmd_build level.
            Ok(vec![])
        }
        emitter::EmitTarget::Binary => {
            // P7-B: Binary emit is handled at a higher level (cmd_build);
            // at per-atom dispatch we return an empty artifact list.
            Ok(vec![])
        }
        emitter::EmitTarget::RustWrapper => mumei_emit_rust::RustWrapperEmitter.emit(
            hir_atom,
            output_path,
            module_env,
            extern_blocks,
        ),
        emitter::EmitTarget::PythonWrapper => mumei_emit_python::PythonWrapperEmitter.emit(
            hir_atom,
            output_path,
            module_env,
            extern_blocks,
        ),
        emitter::EmitTarget::External(name) => {
            let external_emitter = external_emitter.ok_or_else(|| {
                verification::MumeiError::verification(format!(
                    "External emitter '{name}' was not loaded"
                ))
            })?;
            external_emitter.emit(hir_atom, output_path, module_env, extern_blocks)
        }
    }
}

pub(crate) fn sanitized_runtime_symbol(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub(crate) fn write_effect_and_resource_runtime_stubs(
    module_env: &verification::ModuleEnv,
    output_path: &Path,
) -> Result<(), String> {
    use std::fmt::Write;

    let mut source =
        String::from("#include <pthread.h>\n#include <stdint.h>\n#include <stddef.h>\n\n");
    let mut resources: Vec<_> = module_env.resources.keys().cloned().collect();
    resources.sort();
    for resource in resources {
        let symbol = sanitized_runtime_symbol(&resource);
        writeln!(
            source,
            "pthread_mutex_t __mumei_resource_{symbol} = PTHREAD_MUTEX_INITIALIZER;"
        )
        .map_err(|err| err.to_string())?;
    }
    if !module_env.resources.is_empty() {
        source.push('\n');
    }

    let mut effects: Vec<_> = module_env.effect_defs.keys().cloned().collect();
    effects.sort();
    for effect in effects {
        let symbol = sanitized_runtime_symbol(&effect);
        writeln!(
            source,
            "int64_t __effect_{symbol}_stub(int64_t value) {{ (void)value; return 0; }}"
        )
        .map_err(|err| err.to_string())?;
    }

    if source.ends_with("\n\n") {
        source.push_str("/* No named resources or effect stubs required. */\n");
    }
    std::fs::write(output_path, source).map_err(|err| {
        format!(
            "failed to write runtime stubs '{}': {err}",
            output_path.display()
        )
    })
}

pub(crate) fn uses_rust_ffi(extern_blocks: &[parser::ExternBlock]) -> bool {
    extern_blocks
        .iter()
        .any(|block| block.language == "Rust" && !block.functions.is_empty())
}

pub(crate) fn generate_rust_ffi_staticlib(
    crate_dir: &Path,
    output_dir: &Path,
) -> Result<PathBuf, String> {
    let ffi_lib_dir = output_dir.join("mumei_ffi_staticlib");
    if ffi_lib_dir.exists() {
        std::fs::remove_dir_all(&ffi_lib_dir).map_err(|err| {
            format!(
                "failed to remove stale FFI build dir '{}': {err}",
                ffi_lib_dir.display()
            )
        })?;
    }
    std::fs::create_dir_all(ffi_lib_dir.join("src")).map_err(|err| {
        format!(
            "failed to create FFI build dir '{}': {err}",
            ffi_lib_dir.display()
        )
    })?;

    let core_dir = crate_dir.join("mumei-core");
    let cargo_toml = "[package]\nname = \"mumei-ffi-staticlib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\nname = \"mumei_ffi_staticlib\"\ncrate-type = [\"staticlib\"]\n\n[dependencies]\nlazy_static = \"1.4\"\nserde_json = \"1.0\"\nreqwest = { version = \"0.12\", default-features = false, features = [\"blocking\", \"json\", \"rustls-tls\"] }\n";
    std::fs::write(ffi_lib_dir.join("Cargo.toml"), cargo_toml).map_err(|err| {
        format!(
            "failed to write FFI Cargo.toml in '{}': {err}",
            ffi_lib_dir.display()
        )
    })?;
    std::fs::write(
        ffi_lib_dir.join("src/lib.rs"),
        format!(
            "mod json {{ include!({:?}); }}\nmod http {{ include!({:?}); }}\nmod http_server {{ include!({:?}); }}\nmod file {{ include!({:?}); }}\n",
            core_dir.join("src/ffi/json.rs"),
            core_dir.join("src/ffi/http.rs"),
            core_dir.join("src/ffi/http_server.rs"),
            core_dir.join("src/ffi/file.rs"),
        ),
    )
    .map_err(|err| {
        format!(
            "failed to write FFI lib.rs in '{}': {err}",
            ffi_lib_dir.display()
        )
    })?;

    let mut cmd = std::process::Command::new("cargo");
    cmd.arg("build")
        .arg("--release")
        .arg("--manifest-path")
        .arg(ffi_lib_dir.join("Cargo.toml"));
    for var in ["LLVM_SYS_170_PREFIX", "LIBCLANG_PATH"] {
        if let Ok(value) = std::env::var(var) {
            cmd.env(var, value);
        }
    }
    let output = cmd
        .output()
        .map_err(|err| format!("failed to execute cargo for Rust FFI staticlib: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "Rust FFI staticlib build failed (exit {}):\n{}{}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let lib_name = if cfg!(windows) {
        "mumei_ffi_staticlib.lib"
    } else {
        "libmumei_ffi_staticlib.a"
    };
    let lib_path = ffi_lib_dir.join("target/release").join(lib_name);
    if lib_path.exists() {
        Ok(lib_path)
    } else {
        Err(format!(
            "Rust FFI staticlib build finished but '{}' was not produced",
            lib_path.display()
        ))
    }
}

pub(crate) fn collect_hir_calls_from_atom(hir_atom: &hir::HirAtom, calls: &mut Vec<String>) {
    collect_hir_calls_from_stmt(&hir_atom.body, calls);
}

pub(crate) fn collect_hir_calls_from_stmt(stmt: &hir::HirStmt, calls: &mut Vec<String>) {
    match stmt {
        hir::HirStmt::Let { value, .. } => collect_hir_calls_from_expr(value, calls),
        hir::HirStmt::Assign { value, .. } => collect_hir_calls_from_expr(value, calls),
        hir::HirStmt::ArrayStore { index, value, .. } => {
            collect_hir_calls_from_expr(index, calls);
            collect_hir_calls_from_expr(value, calls);
        }
        hir::HirStmt::While {
            cond,
            invariant,
            decreases,
            body,
        } => {
            collect_hir_calls_from_expr(cond, calls);
            collect_hir_calls_from_expr(invariant, calls);
            if let Some(decreases) = decreases {
                collect_hir_calls_from_expr(decreases, calls);
            }
            collect_hir_calls_from_stmt(body, calls);
        }
        hir::HirStmt::Block { stmts, tail_expr } => {
            for stmt in stmts {
                collect_hir_calls_from_stmt(stmt, calls);
            }
            if let Some(tail_expr) = tail_expr {
                collect_hir_calls_from_expr(tail_expr, calls);
            }
        }
        hir::HirStmt::Acquire { body, .. } => collect_hir_calls_from_stmt(body, calls),
        hir::HirStmt::Expr(expr) => collect_hir_calls_from_expr(expr, calls),
    }
}

pub(crate) fn collect_hir_calls_from_expr(expr: &hir::HirExpr, calls: &mut Vec<String>) {
    match expr {
        hir::HirExpr::Call { name, args, .. } => {
            calls.push(name.replace('.', "::"));
            for arg in args {
                collect_hir_calls_from_expr(arg, calls);
            }
        }
        hir::HirExpr::BinaryOp(lhs, _, rhs) => {
            collect_hir_calls_from_expr(lhs, calls);
            collect_hir_calls_from_expr(rhs, calls);
        }
        hir::HirExpr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_hir_calls_from_expr(cond, calls);
            collect_hir_calls_from_stmt(then_branch, calls);
            collect_hir_calls_from_stmt(else_branch, calls);
        }
        hir::HirExpr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_hir_calls_from_expr(value, calls);
            }
        }
        hir::HirExpr::FieldAccess(inner, _) => collect_hir_calls_from_expr(inner, calls),
        hir::HirExpr::ArrayAccess(_, index) => collect_hir_calls_from_expr(index, calls),
        hir::HirExpr::Match { target, arms } => {
            collect_hir_calls_from_expr(target, calls);
            for arm in arms {
                collect_hir_calls_from_stmt(&arm.body, calls);
                if let Some(guard) = &arm.guard {
                    collect_hir_calls_from_expr(guard, calls);
                }
            }
        }
        hir::HirExpr::CallRef { callee, args } => {
            collect_hir_calls_from_expr(callee, calls);
            for arg in args {
                collect_hir_calls_from_expr(arg, calls);
            }
        }
        hir::HirExpr::Async { body } | hir::HirExpr::Task { body, .. } => {
            collect_hir_calls_from_stmt(body, calls);
        }
        hir::HirExpr::Await { expr } => collect_hir_calls_from_expr(expr, calls),
        hir::HirExpr::Perform { args, .. } | hir::HirExpr::VariantInit { fields: args, .. } => {
            for arg in args {
                collect_hir_calls_from_expr(arg, calls);
            }
        }
        hir::HirExpr::TaskGroup { children, .. } => {
            for child in children {
                collect_hir_calls_from_stmt(child, calls);
            }
        }
        hir::HirExpr::Lambda { body, .. } => collect_hir_calls_from_stmt(body, calls),
        hir::HirExpr::ChanSend { channel, value } => {
            collect_hir_calls_from_expr(channel, calls);
            collect_hir_calls_from_expr(value, calls);
        }
        hir::HirExpr::ChanRecv { channel } => collect_hir_calls_from_expr(channel, calls),
        hir::HirExpr::Number(_)
        | hir::HirExpr::Float(_)
        | hir::HirExpr::StringLit(_)
        | hir::HirExpr::Variable(_)
        | hir::HirExpr::AtomRef { .. } => {}
    }
}

pub(crate) fn collect_binary_hir_atoms(
    items: &[Item],
    module_env: &verification::ModuleEnv,
) -> Vec<hir::HirAtom> {
    let mut hir_atoms = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut pending_calls = Vec::new();

    for item in items {
        match item {
            Item::Atom(atom) => {
                if seen.insert(atom.name.clone()) {
                    let hir_atom = lower_atom_to_hir_with_env(atom, Some(module_env));
                    collect_hir_calls_from_atom(&hir_atom, &mut pending_calls);
                    hir_atoms.push(hir_atom);
                }
            }
            Item::ImplBlock(impl_block) => {
                for method in &impl_block.methods {
                    let mut qualified = method.clone();
                    qualified.name = format!("{}::{}", impl_block.struct_name, method.name);
                    if seen.insert(qualified.name.clone()) {
                        let hir_atom = lower_atom_to_hir_with_env(&qualified, Some(module_env));
                        collect_hir_calls_from_atom(&hir_atom, &mut pending_calls);
                        hir_atoms.push(hir_atom);
                    }
                }
            }
            _ => {}
        }
    }

    let mut queued = std::collections::HashSet::new();
    while let Some(name) = pending_calls.pop() {
        if !queued.insert(name.clone()) {
            continue;
        }
        let Some(atom) = module_env.atoms.get(&name) else {
            continue;
        };
        if seen.contains(&atom.name) {
            continue;
        }
        if atom.body_expr.trim().is_empty() {
            continue;
        }
        if atom.trust_level == parser::TrustLevel::Trusted {
            continue;
        }
        if seen.insert(atom.name.clone()) {
            let hir_atom = lower_atom_to_hir_with_env(atom, Some(module_env));
            collect_hir_calls_from_atom(&hir_atom, &mut pending_calls);
            hir_atoms.push(hir_atom);
        }
    }

    hir_atoms
}

// =============================================================================
// mumei build — full pipeline (verify + codegen)
// =============================================================================
