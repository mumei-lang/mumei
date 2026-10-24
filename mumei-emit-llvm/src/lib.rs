#![allow(clippy::result_large_err)]

pub mod binary;
pub mod codegen;
pub mod jit;

/// Re-export inkwell's Context so downstream crates (e.g. the CLI binary)
/// can create a Context for the JIT engine without depending on inkwell directly.
pub use inkwell::context::Context as LlvmContext;

use mumei_core::emitter::{Artifact, ArtifactKind, EmitAtom, Emitter};
use mumei_core::parser::ExternBlock;
use mumei_core::verification::{ModuleEnv, MumeiError, MumeiResult};
use std::path::Path;

pub struct LlvmEmitter;

impl Emitter for LlvmEmitter {
    fn emit(
        &self,
        emit_atom: &EmitAtom<'_>,
        output_path: &Path,
        module_env: &ModuleEnv,
        extern_blocks: &[ExternBlock],
    ) -> MumeiResult<Vec<Artifact>> {
        codegen::compile(emit_atom, output_path, module_env, extern_blocks)?;
        let ll_path = output_path.with_extension("ll");
        let data = std::fs::read(&ll_path).map_err(|e| {
            MumeiError::codegen(format!(
                "Failed to read generated LLVM IR '{}': {}",
                ll_path.display(),
                e
            ))
        })?;
        Ok(vec![Artifact {
            name: ll_path,
            data,
            kind: ArtifactKind::Source,
        }])
    }
}
