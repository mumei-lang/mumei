// =============================================================================
// Emitter Plugin Architecture — Phase 2
// =============================================================================
// Defines the `Emitter` trait and core types for code generation backends.
// Phase 2: workspace split with pub types for external emitter crates.
//   - `CHeaderEmitter`: generates `.h` header files from verified `HirAtom`
//   - `LlvmEmitter`: moved to `mumei-emit-llvm` crate
//   - `VerifiedJsonEmitter`: moved to `mumei-emit-json` crate
//
// See docs/CROSS_PROJECT_ROADMAP.md "Emitter Plugin Architecture" section for
// the full 3-phase plan.
// =============================================================================

use crate::hir::HirAtom;
use crate::parser::ExternBlock;
use crate::verification::{ModuleEnv, MumeiError, MumeiResult};
use std::path::{Path, PathBuf};

// =============================================================================
// Artifact abstraction (Roadmap #5 Phase 1)
// =============================================================================

/// Classification of emitted artifacts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArtifactKind {
    /// Reserved for future native binary output
    Binary,
    Source,
    Header,
    /// Phase 3: non-source / non-header metadata emitted alongside the
    /// primary build output. Examples include `verified.json` reports,
    /// proof-cert bundles, dependency manifests, and similar JSON / YAML
    /// sidecar files. External emitter plugins that produce structured
    /// metadata (rather than compilable code) should classify their
    /// artifacts under this variant.
    Metadata,
}

/// A single output artifact produced by an emitter.
#[derive(Clone, Debug)]
pub struct Artifact {
    pub name: std::path::PathBuf,
    pub data: Vec<u8>,
    pub kind: ArtifactKind,
}

/// Trait for code generation backends (emitter plugins).
/// Each emitter receives a verified HirAtom and produces output in its target format.
/// Returns a list of `Artifact`s; the caller is responsible for persisting them.
pub trait Emitter {
    fn emit(
        &self,
        hir_atom: &HirAtom,
        output_path: &Path,
        module_env: &ModuleEnv,
        extern_blocks: &[ExternBlock],
    ) -> MumeiResult<Vec<Artifact>>;
}

/// Available emit targets, selected via --emit CLI flag.
#[derive(Clone, Debug, Default)]
pub enum EmitTarget {
    #[default]
    LlvmIr,
    CHeader,
    VerifiedJson,
    ProofBook,
    /// P5-A: Generate .proof-cert.json alongside build output
    ProofCert,
    /// P7-B: Compile to standalone native binary via clang
    Binary,
    /// FFI glue code for Rust (NOT a transpiler — generates extern "C" bindings + safe wrappers)
    RustWrapper,
    /// FFI glue code for Python (NOT a transpiler — generates ctypes-based wrappers)
    PythonWrapper,
    /// Phase 3 (foundation): an external emitter plugin loaded at runtime
    /// from `~/.mumei/emitters/{name}/libmumei_emit_{name}.so` (or the
    /// platform-specific `.dll` / `.dylib`). The wrapped string is the
    /// emitter name as supplied to `--emit`, used for diagnostics and the
    /// dynamic-library lookup. See [`load_external_emitter`].
    External(String),
}

/// Phase 3 (foundation): trait-object handle for emitter plugins. External
/// emitters produced by [`load_external_emitter`] are returned in this
/// box so callers can store and dispatch them uniformly with the
/// statically-linked emitters.
pub type BoxedEmitter = Box<dyn Emitter + Send + Sync>;

/// Phase 3 (foundation, stub): resolve an external emitter plugin by name.
///
/// Looks for a dynamic library at:
///
/// - `~/.mumei/emitters/{name}/libmumei_emit_{name}.so` (Linux),
/// - `~/.mumei/emitters/{name}/libmumei_emit_{name}.dylib` (macOS), or
/// - `~/.mumei/emitters/{name}/mumei_emit_{name}.dll` (Windows).
///
/// External emitters MUST expose the C-ABI symbol
///
/// ```text
/// #[no_mangle]
/// extern "C" fn mumei_create_emitter() -> *mut dyn Emitter
/// ```
///
/// returning a heap-allocated `Emitter` whose vtable lives in the plugin's
/// own crate. Callers are responsible for keeping the returned
/// [`BoxedEmitter`] alive only while the host process is alive (the
/// plugin's `.so` is dlopen-ed once and never closed in this stub).
///
/// **This is a stub** that performs only the path resolution and existence
/// check; it does NOT yet `dlopen` the library. Returning `Err(...)` here
/// is the contract that lets the CLI fall through to its existing
/// "Unknown emit target" error path with a helpful diagnostic. A follow-up
/// PR will fill in `libloading::Library::new()` + the `extern "C"` symbol
/// lookup once the C-ABI is stabilized.
pub fn load_external_emitter(name: &str) -> MumeiResult<BoxedEmitter> {
    let mumei_home = crate::manifest::mumei_home();
    let emitter_dir = mumei_home.join("emitters").join(name);
    let candidates = external_emitter_library_candidates(&emitter_dir, name);
    let found = candidates.iter().find(|p| p.exists());

    match found {
        Some(path) => Err(MumeiError::codegen(format!(
            "External emitter '{}' was found at '{}', but dynamic loading \
             of emitter plugins is not yet implemented (Phase 3 foundation \
             stub). Implementations will need to dlopen this library and \
             call its `mumei_create_emitter()` entry point. Tracked in \
             docs/CROSS_PROJECT_ROADMAP.md.",
            name,
            path.display()
        ))),
        None => {
            let searched = candidates
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join("\n    ");
            Err(MumeiError::codegen(format!(
                "External emitter '{}' not found. Searched:\n    {}\n\
                 Install a plugin at one of these paths or pick a built-in \
                 --emit target (llvm-ir, c-header, verified-json, proof-book, \
                 proof-cert, binary, rust-wrapper, python-wrapper).",
                name, searched,
            )))
        }
    }
}

/// Phase 3 (foundation): build the candidate list of dynamic-library paths
/// that [`load_external_emitter`] inspects for plugin `name`. Exposed at
/// crate level so tests and downstream tooling can verify the resolution
/// rules without touching the filesystem.
pub fn external_emitter_library_candidates(emitter_dir: &Path, name: &str) -> Vec<PathBuf> {
    vec![
        emitter_dir.join(format!("libmumei_emit_{}.so", name)),
        emitter_dir.join(format!("libmumei_emit_{}.dylib", name)),
        emitter_dir.join(format!("mumei_emit_{}.dll", name)),
    ]
}

pub struct CHeaderEmitter;

/// Map a mumei type name to its C equivalent.
pub fn mumei_type_to_c(type_name: &str) -> &str {
    match type_name {
        "i64" => "int64_t",
        "i32" => "int32_t",
        "u64" => "uint64_t",
        "u32" => "uint32_t",
        "f64" => "double",
        "f32" => "float",
        "bool" => "int",
        "Str" | "String" => "const char*",
        "[i64]" => "const int64_t*",
        _ => "int64_t", // default fallback for refined types based on i64
    }
}

/// Map a mumei return type to its C equivalent, defaulting to int64_t.
pub fn mumei_return_type_to_c(type_name: Option<&str>, module_env: &ModuleEnv) -> String {
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
    ) -> MumeiResult<Vec<Artifact>> {
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

        // Doxygen documentation block
        let has_pre = atom.requires != "true";
        let has_post = atom.ensures != "true";
        if has_pre || has_post {
            content.push_str("/**\n");
            let c_fn_name = atom.name.replace("::", "_");
            content.push_str(&format!(" * @brief {}\n", c_fn_name));
            if has_pre {
                content.push_str(&format!(" * @pre {}\n", atom.requires));
            }
            if has_post {
                content.push_str(&format!(" * @post {}\n", atom.ensures));
            }
            content.push_str(" */\n");
        } else {
            // Even without contracts, add @brief
            let c_fn_name = atom.name.replace("::", "_");
            content.push_str(&format!("/** @brief {} */\n", c_fn_name));
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

        Ok(vec![Artifact {
            name: header_path,
            data: content.into_bytes(),
            kind: ArtifactKind::Header,
        }])
    }
}

// =============================================================================
// Tests
// =============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir::{HirEffectSet, HirExpr, HirStmt};
    use crate::parser::ast::{Expr, Param, Span, Stmt, TrustLevel};

    /// Helper: build a minimal HirAtom for testing CHeaderEmitter.
    fn make_hir_atom(
        name: &str,
        params: Vec<Param>,
        requires: &str,
        ensures: &str,
        return_type: Option<String>,
    ) -> HirAtom {
        use crate::parser::ast::Atom;
        HirAtom {
            body: HirStmt::Expr(HirExpr::Number(0)),
            requires_hir: HirExpr::Number(1),
            ensures_hir: HirExpr::Number(1),
            atom: Atom {
                name: name.to_string(),
                type_params: vec![],
                where_bounds: vec![],
                params,
                requires: requires.to_string(),
                forall_constraints: vec![],
                ensures: ensures.to_string(),
                body_expr: "0".to_string(),
                consumed_params: vec![],
                resources: vec![],
                is_async: false,
                trust_level: TrustLevel::Verified,
                max_unroll: None,
                invariant: None,
                effects: vec![],
                return_type,
                span: Span::default(),
                effect_pre: std::collections::HashMap::new(),
                effect_post: std::collections::HashMap::new(),
            },
            body_stmt: Stmt::Expr(Expr::Number(0), Span::default()),
            effect_set: HirEffectSet::default(),
        }
    }

    fn make_param(name: &str, type_name: Option<&str>) -> Param {
        Param {
            name: name.to_string(),
            type_name: type_name.map(|s| s.to_string()),
            type_ref: None,
            is_ref: false,
            is_ref_mut: false,
            fn_contract_requires: None,
            fn_contract_ensures: None,
        }
    }

    // ---- mumei_type_to_c tests ----

    #[test]
    fn test_mumei_type_to_c_i32() {
        assert_eq!(mumei_type_to_c("i32"), "int32_t");
    }

    #[test]
    fn test_mumei_type_to_c_u32() {
        assert_eq!(mumei_type_to_c("u32"), "uint32_t");
    }

    #[test]
    fn test_mumei_type_to_c_f32() {
        assert_eq!(mumei_type_to_c("f32"), "float");
    }

    #[test]
    fn test_mumei_type_to_c_existing_mappings() {
        assert_eq!(mumei_type_to_c("i64"), "int64_t");
        assert_eq!(mumei_type_to_c("u64"), "uint64_t");
        assert_eq!(mumei_type_to_c("f64"), "double");
        assert_eq!(mumei_type_to_c("bool"), "int");
        assert_eq!(mumei_type_to_c("Str"), "const char*");
        assert_eq!(mumei_type_to_c("String"), "const char*");
    }

    // ---- CHeaderEmitter Doxygen format tests ----

    #[test]
    fn test_cheader_doxygen_pre_post() {
        let hir = make_hir_atom(
            "safe_div",
            vec![
                make_param("dividend", Some("i64")),
                make_param("divisor", Some("i64")),
            ],
            "divisor != 0",
            "result * divisor == dividend",
            Some("i64".to_string()),
        );
        let module_env = ModuleEnv::new();
        let artifacts = CHeaderEmitter
            .emit(&hir, Path::new("/tmp/safe_div"), &module_env, &[])
            .unwrap();

        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].kind, ArtifactKind::Header);

        let content = String::from_utf8(artifacts[0].data.clone()).unwrap();
        assert!(content.contains("@brief safe_div"), "missing @brief");
        assert!(content.contains("@pre divisor != 0"), "missing @pre");
        assert!(
            content.contains("@post result * divisor == dividend"),
            "missing @post"
        );
        assert!(content.contains("/**"), "missing Doxygen open");
        assert!(content.contains(" */"), "missing Doxygen close");
        // Should NOT contain old-style comments
        assert!(!content.contains("/* requires:"));
        assert!(!content.contains("/* ensures:"));
    }

    #[test]
    fn test_cheader_doxygen_brief_only_no_contracts() {
        let hir = make_hir_atom("noop", vec![], "true", "true", None);
        let module_env = ModuleEnv::new();
        let artifacts = CHeaderEmitter
            .emit(&hir, Path::new("/tmp/noop"), &module_env, &[])
            .unwrap();

        let content = String::from_utf8(artifacts[0].data.clone()).unwrap();
        assert!(content.contains("/** @brief noop */"), "missing @brief");
        assert!(!content.contains("@pre"), "should not have @pre");
        assert!(!content.contains("@post"), "should not have @post");
    }

    #[test]
    fn test_cheader_artifact_kind_and_path() {
        let hir = make_hir_atom("foo", vec![], "true", "true", None);
        let module_env = ModuleEnv::new();
        let artifacts = CHeaderEmitter
            .emit(&hir, Path::new("/tmp/foo"), &module_env, &[])
            .unwrap();

        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].kind, ArtifactKind::Header);
        assert_eq!(artifacts[0].name, std::path::PathBuf::from("/tmp/foo.h"));
    }

    #[test]
    fn test_cheader_header_guard() {
        let hir = make_hir_atom("safe_div", vec![], "x != 0", "true", None);
        let module_env = ModuleEnv::new();
        let artifacts = CHeaderEmitter
            .emit(&hir, Path::new("/tmp/safe_div"), &module_env, &[])
            .unwrap();

        let content = String::from_utf8(artifacts[0].data.clone()).unwrap();
        assert!(content.contains("#ifndef SAFE_DIV_H"));
        assert!(content.contains("#define SAFE_DIV_H"));
        assert!(content.contains("#endif /* SAFE_DIV_H */"));
    }

    #[test]
    fn test_cheader_expanded_type_mappings() {
        let hir = make_hir_atom(
            "typed_fn",
            vec![
                make_param("a", Some("i32")),
                make_param("b", Some("u32")),
                make_param("c", Some("f32")),
            ],
            "true",
            "true",
            Some("i32".to_string()),
        );
        let module_env = ModuleEnv::new();
        let artifacts = CHeaderEmitter
            .emit(&hir, Path::new("/tmp/typed_fn"), &module_env, &[])
            .unwrap();

        let content = String::from_utf8(artifacts[0].data.clone()).unwrap();
        assert!(content.contains("int32_t a"), "missing int32_t a");
        assert!(content.contains("uint32_t b"), "missing uint32_t b");
        assert!(content.contains("float c"), "missing float c");
        assert!(
            content.contains("extern int32_t typed_fn"),
            "missing return type int32_t"
        );
    }

    #[test]
    fn test_cheader_full_output_format() {
        let hir = make_hir_atom(
            "safe_div",
            vec![
                make_param("dividend", Some("i64")),
                make_param("divisor", Some("i64")),
            ],
            "divisor != 0",
            "result * divisor == dividend",
            Some("i64".to_string()),
        );
        let module_env = ModuleEnv::new();
        let artifacts = CHeaderEmitter
            .emit(&hir, Path::new("/tmp/safe_div"), &module_env, &[])
            .unwrap();

        let content = String::from_utf8(artifacts[0].data.clone()).unwrap();
        // Verify the expected output structure
        assert!(content.contains("#ifndef SAFE_DIV_H"));
        assert!(content.contains("#include <stdint.h>"));
        assert!(content.contains("extern int64_t safe_div(int64_t dividend, int64_t divisor);"));
        assert!(content.contains("#endif /* SAFE_DIV_H */"));
    }

    // ---- Phase 3 (foundation) emitter plugin tests ----

    /// `EmitTarget::External` carries the supplied plugin name verbatim so
    /// it can be threaded through dispatch and printed in CLI status.
    #[test]
    fn test_emit_target_external_round_trips_name() {
        let target = EmitTarget::External("wasm".to_string());
        match &target {
            EmitTarget::External(name) => assert_eq!(name, "wasm"),
            _ => panic!("expected External variant, got {:?}", target),
        }
        // A second instance with a different name must not compare equal
        // structurally (we don't impl `Eq` on EmitTarget but we do exercise
        // the `Debug` formatter to lock down at least minimal stability).
        let other = EmitTarget::External("cuda".to_string());
        let dbg_a = format!("{:?}", target);
        let dbg_b = format!("{:?}", other);
        assert!(dbg_a.contains("wasm"));
        assert!(dbg_b.contains("cuda"));
        assert_ne!(dbg_a, dbg_b);
    }

    /// `ArtifactKind::Metadata` is a distinct variant so emitters that
    /// produce non-source / non-header sidecar files (verified.json,
    /// proof bundles, dependency reports, …) can declare their kind
    /// honestly.
    #[test]
    fn test_artifact_kind_metadata_distinct() {
        assert_ne!(ArtifactKind::Metadata, ArtifactKind::Source);
        assert_ne!(ArtifactKind::Metadata, ArtifactKind::Header);
        assert_ne!(ArtifactKind::Metadata, ArtifactKind::Binary);
        // Round-trip through Debug so any future rename of the variant
        // forces this test to be updated.
        assert_eq!(format!("{:?}", ArtifactKind::Metadata), "Metadata");
    }

    /// `external_emitter_library_candidates` returns the three
    /// platform-specific filename patterns documented on
    /// [`load_external_emitter`].
    #[test]
    fn test_external_emitter_library_candidates_shape() {
        let dir = std::path::PathBuf::from("/tmp/mumei_emitters/wasm");
        let candidates = external_emitter_library_candidates(&dir, "wasm");
        assert_eq!(candidates.len(), 3);
        assert_eq!(
            candidates[0],
            std::path::PathBuf::from("/tmp/mumei_emitters/wasm/libmumei_emit_wasm.so")
        );
        assert_eq!(
            candidates[1],
            std::path::PathBuf::from("/tmp/mumei_emitters/wasm/libmumei_emit_wasm.dylib")
        );
        assert_eq!(
            candidates[2],
            std::path::PathBuf::from("/tmp/mumei_emitters/wasm/mumei_emit_wasm.dll")
        );
    }

    /// When no plugin file exists at the expected paths,
    /// `load_external_emitter` returns an `Err` whose message lists the
    /// candidate paths it searched. This is the path that the CLI
    /// fallback in `src/main.rs` relies on to print a helpful diagnostic.
    #[test]
    fn test_load_external_emitter_missing_returns_err_with_paths() {
        // Use a name that is overwhelmingly unlikely to exist on disk, even
        // accidentally. The default mumei_home() is honoured so this also
        // validates the `~/.mumei/emitters/{name}` resolution rule.
        let unique = format!(
            "task_1c_phase3_missing_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        let result = load_external_emitter(&unique);
        let err = match result {
            Ok(_) => panic!("missing plugin must error, but load_external_emitter returned Ok"),
            Err(e) => e,
        };
        let msg = format!("{}", err);
        assert!(
            msg.contains(&unique),
            "error must mention the requested name '{unique}', got: {msg}"
        );
        assert!(
            msg.contains("not found"),
            "error must say 'not found' when no library is present, got: {msg}"
        );
        assert!(
            msg.contains(&format!("libmumei_emit_{unique}.so")),
            "error must list the .so candidate path, got: {msg}"
        );
    }
}
