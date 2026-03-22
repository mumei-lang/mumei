// =============================================================================
// Emitter Plugin Architecture — Phase 1
// =============================================================================
// Defines the `Emitter` trait and enum-based static dispatch for code generation
// backends. Phase 1 includes:
//   - `LlvmEmitter`: wraps existing `codegen::compile()` (LLVM IR backend)
//   - `CHeaderEmitter`: generates `.h` header files from verified `HirAtom`
//
// See docs/CROSS_PROJECT_ROADMAP.md "Emitter Plugin Architecture" section for
// the full 3-phase plan.
// =============================================================================

use crate::hir::HirAtom;
use crate::parser::ExternBlock;
use crate::verification::{ModuleEnv, MumeiResult};
use std::path::Path;

/// Trait for code generation backends (emitter plugins).
/// Each emitter receives a verified HirAtom and produces output in its target format.
pub(crate) trait Emitter {
    fn emit(
        &self,
        hir_atom: &HirAtom,
        output_path: &Path,
        module_env: &ModuleEnv,
        extern_blocks: &[ExternBlock],
    ) -> MumeiResult<()>;
}

/// Available emit targets, selected via --emit CLI flag.
#[derive(Clone, Debug, Default)]
pub(crate) enum EmitTarget {
    #[default]
    LlvmIr,
    CHeader,
}

pub(crate) struct LlvmEmitter;

impl Emitter for LlvmEmitter {
    fn emit(
        &self,
        hir_atom: &HirAtom,
        output_path: &Path,
        module_env: &ModuleEnv,
        extern_blocks: &[ExternBlock],
    ) -> MumeiResult<()> {
        crate::codegen::compile(hir_atom, output_path, module_env, extern_blocks)
    }
}

pub(crate) struct CHeaderEmitter;

/// Map a mumei type name to its C equivalent.
fn mumei_type_to_c(type_name: &str) -> &str {
    match type_name {
        "i64" => "int64_t",
        "f64" => "double",
        "bool" => "int",
        "Str" | "String" => "const char*",
        "u64" => "uint64_t",
        "[i64]" => "const int64_t*",
        _ => "int64_t", // default fallback for refined types based on i64
    }
}

/// Map a mumei return type to its C equivalent, defaulting to int64_t.
fn mumei_return_type_to_c(type_name: Option<&str>, module_env: &ModuleEnv) -> String {
    match type_name {
        Some(name) => {
            let base = module_env.resolve_base_type(name);
            mumei_type_to_c(&base).to_string()
        }
        None => "int64_t".to_string(),
    }
}

impl Emitter for CHeaderEmitter {
    fn emit(
        &self,
        hir_atom: &HirAtom,
        output_path: &Path,
        module_env: &ModuleEnv,
        _extern_blocks: &[ExternBlock],
    ) -> MumeiResult<()> {
        let atom = &hir_atom.atom;
        let header_path = output_path.with_extension("h");

        // Generate header guard name from atom name (uppercase + _H)
        let guard_name = format!(
            "{}_H",
            atom.name
                .to_uppercase()
                .replace("::", "_")
                .replace('-', "_")
        );

        let mut content = String::new();

        // Header guard
        content.push_str(&format!("#ifndef {}\n", guard_name));
        content.push_str(&format!("#define {}\n", guard_name));
        content.push('\n');

        // Required includes
        content.push_str("#include <stdint.h>\n");
        content.push('\n');

        // Contract documentation as comments
        if atom.requires != "true" {
            content.push_str(&format!("/* requires: {} */\n", atom.requires));
        }
        if atom.ensures != "true" {
            content.push_str(&format!("/* ensures: {} */\n", atom.ensures));
        }
        if atom.requires != "true" || atom.ensures != "true" {
            content.push('\n');
        }

        // Build parameter list
        let params: Vec<String> = atom
            .params
            .iter()
            .map(|p| {
                let c_type = match &p.type_name {
                    Some(tn) => {
                        let base = module_env.resolve_base_type(tn);
                        mumei_type_to_c(&base).to_string()
                    }
                    None => "int64_t".to_string(),
                };
                format!("{} {}", c_type, p.name)
            })
            .collect();

        let params_str = if params.is_empty() {
            "void".to_string()
        } else {
            params.join(", ")
        };

        // Return type
        let return_type = mumei_return_type_to_c(atom.return_type.as_deref(), module_env);

        // Function name: replace :: with _ for C compatibility
        let c_fn_name = atom.name.replace("::", "_");

        // extern "C" function declaration
        content.push_str(&format!(
            "extern {} {}({});\n",
            return_type, c_fn_name, params_str
        ));

        content.push('\n');
        content.push_str(&format!("#endif /* {} */\n", guard_name));

        std::fs::write(&header_path, content).map_err(|e| {
            crate::verification::MumeiError::codegen(format!(
                "Failed to write C header file '{}': {}",
                header_path.display(),
                e
            ))
        })?;

        Ok(())
    }
}

/// Dispatch to the appropriate emitter based on target.
pub(crate) fn emit(
    target: &EmitTarget,
    hir_atom: &HirAtom,
    output_path: &Path,
    module_env: &ModuleEnv,
    extern_blocks: &[ExternBlock],
) -> MumeiResult<()> {
    match target {
        EmitTarget::LlvmIr => LlvmEmitter.emit(hir_atom, output_path, module_env, extern_blocks),
        EmitTarget::CHeader => {
            CHeaderEmitter.emit(hir_atom, output_path, module_env, extern_blocks)
        }
    }
}
