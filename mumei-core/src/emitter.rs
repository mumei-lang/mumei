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
use crate::lowering::{lower, LoweredType};
use crate::parser::ExternBlock;
use crate::verification::{ModuleEnv, MumeiError, MumeiResult};
use std::ffi::c_void;
use std::path::Path;

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
    /// Phase 3: non-source, non-header sidecar artefacts emitted by
    /// plugin emitters (e.g. proof bundles, `.proof-cert.json`-like
    /// metadata blobs, Wasm component manifests). The exact MIME of
    /// the payload is emitter-defined; the build pipeline should treat
    /// these as opaque bytes when copying them to disk.
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
    DecidableMetrics,
    ProofBook,
    /// P5-A: Generate .proof-cert.json alongside build output
    ProofCert,
    /// Emit Lean escalation candidate bundle alongside build output
    EscalationBundle,
    /// P7-B: Compile to standalone native binary via clang
    Binary,
    /// FFI glue code for Rust (NOT a transpiler — generates extern "C" bindings + safe wrappers)
    RustWrapper,
    /// FFI glue code for Python (NOT a transpiler — generates ctypes-based wrappers)
    PythonWrapper,
    /// Phase 3: an emitter loaded from an external dynamic library at
    /// `~/.mumei/emitters/<name>/libmumei_emit_<name>.{so,dll,dylib}`.
    /// Resolution is delegated to [`load_external_emitter`]. Used when
    /// the `--emit` CLI flag does not match any built-in target.
    External(String),
}

// =============================================================================
// Phase 3: external emitter plugin loader
// =============================================================================

/// Current emitter plugin ABI version. Bump when the Emitter trait
/// signature or HirAtom/ModuleEnv layout changes in a breaking way.
pub const EMITTER_ABI_VERSION: u32 = 1;

/// Trait-object wrapper for an emitter loaded out-of-process. Plugins
/// must be `Send + Sync` so they can be shared across threads in the
/// future build pipeline.
pub type BoxedEmitter = Box<dyn Emitter + Send + Sync>;

#[repr(C)]
struct RawEmitterTraitObject {
    data: *mut c_void,
    vtable: *mut c_void,
}

const _: () = {
    assert!(std::mem::size_of::<BoxedEmitter>() == std::mem::size_of::<RawEmitterTraitObject>());
    assert!(std::mem::align_of::<BoxedEmitter>() == std::mem::align_of::<RawEmitterTraitObject>());
};

/// C-compatible handle returned by `mumei_create_emitter`.
///
/// Rust trait objects are fat pointers, so plugins return their data
/// pointer and vtable pointer explicitly instead of returning
/// `*mut dyn Emitter` across the `extern "C"` boundary.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct EmitterPluginHandle {
    pub data: *mut c_void,
    pub vtable: *mut c_void,
}

impl EmitterPluginHandle {
    pub fn from_boxed(emitter: BoxedEmitter) -> Self {
        let raw: RawEmitterTraitObject = unsafe { std::mem::transmute(emitter) };
        Self {
            data: raw.data,
            vtable: raw.vtable,
        }
    }

    fn is_null(&self) -> bool {
        self.data.is_null() || self.vtable.is_null()
    }

    unsafe fn into_boxed(self) -> BoxedEmitter {
        let raw = RawEmitterTraitObject {
            data: self.data,
            vtable: self.vtable,
        };
        unsafe { std::mem::transmute(raw) }
    }
}

struct PanicSafeEmitter {
    inner: BoxedEmitter,
    _lib: libloading::Library,
}

impl Emitter for PanicSafeEmitter {
    fn emit(
        &self,
        hir_atom: &HirAtom,
        output_path: &Path,
        module_env: &ModuleEnv,
        extern_blocks: &[ExternBlock],
    ) -> MumeiResult<Vec<Artifact>> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.inner
                .emit(hir_atom, output_path, module_env, extern_blocks)
        }))
        .unwrap_or_else(|_| {
            Err(MumeiError::verification(
                "External emitter panicked during emit".to_string(),
            ))
        })
    }
}

/// Locate and load an external emitter plugin by name.
///
/// # Resolution
///
/// Looks for the platform-specific dynamic library under the user's
/// `~/.mumei/emitters/<name>/` directory (the same root where
/// `crate::manifest::mumei_home()` resolves):
///
/// - Linux:   `libmumei_emit_<name>.so`
/// - macOS:   `libmumei_emit_<name>.dylib`
/// - Windows: `mumei_emit_<name>.dll` (no `lib` prefix — matches Rust
///   `cdylib` output convention on Windows)
///
/// # Plugin contract
///
/// Loaded libraries must export an ABI-version function and a C-ABI factory symbol:
///
/// ```ignore
/// #[no_mangle]
/// pub extern "C" fn mumei_emitter_abi_version() -> u32 {
///     mumei_core::emitter::EMITTER_ABI_VERSION
/// }
///
/// /// Returns a heap-allocated emitter owned by the caller. The host frees the box via
/// /// `drop` when the build finishes.
/// #[no_mangle]
/// pub extern "C" fn mumei_create_emitter() -> mumei_core::emitter::EmitterPluginHandle;
/// ```
pub fn load_external_emitter(name: &str) -> MumeiResult<BoxedEmitter> {
    // Rust's `cdylib` output on Windows produces `<crate>.dll` *without*
    // a `lib` prefix, while Linux/macOS keep the `lib` prefix. Match that
    // convention so plugin authors can drop their compiled artefact in
    // place without renaming.
    let (prefix, ext) = if cfg!(target_os = "windows") {
        ("", ".dll")
    } else if cfg!(target_os = "macos") {
        ("lib", ".dylib")
    } else {
        ("lib", ".so")
    };
    let lib_filename = format!("{}mumei_emit_{}{}", prefix, name, ext);
    let lib_path = crate::manifest::mumei_home()
        .join("emitters")
        .join(name)
        .join(&lib_filename);

    if !lib_path.exists() {
        return Err(MumeiError::verification(format!(
            "External emitter '{name}' not found.\n  \
             Expected plugin at: {path}\n  \
             To install: place the compiled `{lib_filename}` (exporting `mumei_emitter_abi_version` and `mumei_create_emitter`) at that location.",
            name = name,
            path = lib_path.display(),
            lib_filename = lib_filename,
        )));
    }

    unsafe {
        let lib = libloading::Library::new(&lib_path).map_err(|e| {
            MumeiError::verification(format!(
                "Failed to load external emitter '{name}' from {}: {e}",
                lib_path.display()
            ))
        })?;

        let version = {
            let abi_version: libloading::Symbol<unsafe extern "C" fn() -> u32> =
                lib.get(b"mumei_emitter_abi_version").map_err(|e| {
                    MumeiError::verification(format!(
                        "External emitter '{name}' does not export mumei_emitter_abi_version: {e}"
                    ))
                })?;
            abi_version()
        };
        if version != EMITTER_ABI_VERSION {
            return Err(MumeiError::verification(format!(
                "External emitter '{name}' ABI version mismatch: expected {EMITTER_ABI_VERSION}, got {version}"
            )));
        }

        let handle = {
            let create: libloading::Symbol<unsafe extern "C" fn() -> EmitterPluginHandle> =
                lib.get(b"mumei_create_emitter").map_err(|e| {
                    MumeiError::verification(format!(
                        "External emitter '{name}' does not export mumei_create_emitter: {e}"
                    ))
                })?;
            create()
        };
        if handle.is_null() {
            return Err(MumeiError::verification(format!(
                "External emitter '{name}' returned a null handle from mumei_create_emitter"
            )));
        }

        let emitter = handle.into_boxed();
        let wrapped = PanicSafeEmitter {
            inner: emitter,
            _lib: lib,
        };
        Ok(Box::new(wrapped))
    }
}

// =============================================================================
// Built-in emitters
// =============================================================================

pub struct CHeaderEmitter;

fn format_lowered_type_to_c(lowered: &LoweredType) -> &'static str {
    match lowered {
        LoweredType::I64 => "int64_t",
        LoweredType::I32 => "int32_t",
        LoweredType::U64 => "uint64_t",
        LoweredType::U32 => "uint32_t",
        LoweredType::F64 => "double",
        LoweredType::F32 => "float",
        LoweredType::Bool => "int",
        LoweredType::Str => "const char*",
        LoweredType::Array(inner) if matches!(**inner, LoweredType::I64) => "const int64_t*",
        LoweredType::Array(_) | LoweredType::Other(_) => "int64_t",
    }
}

/// Map a mumei type name to its C equivalent.
pub fn mumei_type_to_c(type_name: &str) -> &str {
    format_lowered_type_to_c(&lower(type_name))
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
                trace_id: None,
                spec_metadata: std::collections::HashMap::new(),
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

    #[test]
    fn test_mumei_type_to_c_array_edge_cases() {
        assert_eq!(mumei_type_to_c("[f64]"), "int64_t");
        assert_eq!(mumei_type_to_c("[[i64]]"), "int64_t");
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

    // ============================================================
    // Phase 3: ArtifactKind / EmitTarget / load_external_emitter
    // ============================================================

    /// Phase 3: `ArtifactKind::Metadata` is a distinct variant so the
    /// build pipeline can route plugin metadata blobs differently from
    /// `Source` / `Header` / `Binary`.
    #[test]
    fn test_artifact_kind_metadata_distinct() {
        let m = ArtifactKind::Metadata;
        assert_ne!(m, ArtifactKind::Source);
        assert_ne!(m, ArtifactKind::Header);
        assert_ne!(m, ArtifactKind::Binary);
        // round-trip an Artifact{kind: Metadata}
        let art = Artifact {
            name: std::path::PathBuf::from("/tmp/plugin.meta.json"),
            data: br#"{"kind":"metadata"}"#.to_vec(),
            kind: ArtifactKind::Metadata,
        };
        assert_eq!(art.kind, ArtifactKind::Metadata);
    }

    /// Phase 3: `EmitTarget::External(name)` carries the requested
    /// plugin name so that `dispatch_emit` can resolve the dynamic
    /// library path at build time.
    #[test]
    fn test_emit_target_external_carries_name() {
        let t = EmitTarget::External("wasm".to_string());
        match &t {
            EmitTarget::External(name) => assert_eq!(name, "wasm"),
            other => panic!("expected External, got {:?}", other),
        }
        // Default is still LlvmIr so existing CLI defaults are unchanged.
        assert!(matches!(EmitTarget::default(), EmitTarget::LlvmIr));
    }

    /// Phase 3: `load_external_emitter` returns a structured "not
    /// found" error when the plugin file does not exist on disk. The
    /// error message must include both the plugin name and the
    /// expected install path so users can self-serve.
    #[test]
    fn test_load_external_emitter_missing_plugin_errors() {
        // Use a name that is exceedingly unlikely to be installed on
        // any contributor machine or CI runner.
        let res = load_external_emitter("definitely_not_installed_phase3_plugin");
        let err = match res {
            Ok(_) => panic!("missing plugin must be an Err"),
            Err(e) => e,
        };
        let msg = format!("{}", err);
        assert!(
            msg.contains("definitely_not_installed_phase3_plugin"),
            "error must mention plugin name; got: {msg}"
        );
        assert!(
            msg.contains("not found") || msg.contains("Expected plugin at"),
            "error must explain *why* the lookup failed; got: {msg}"
        );
        // Plugins live under ~/.mumei/emitters/<name>/. Check that the
        // hint references that directory so users know where to drop
        // their .so / .dylib / .dll.
        assert!(
            msg.contains(".mumei") && msg.contains("emitters"),
            "error must point to the ~/.mumei/emitters install path; got: {msg}"
        );
    }

    #[test]
    fn test_emitter_abi_version_constant() {
        assert_eq!(EMITTER_ABI_VERSION, 1);
    }

    #[test]
    fn test_emitter_plugin_handle_round_trips_boxed_emitter() {
        let handle = EmitterPluginHandle::from_boxed(Box::new(CHeaderEmitter));
        assert!(!handle.is_null());
        let boxed = unsafe { handle.into_boxed() };
        let hir = make_hir_atom("handle_round_trip", vec![], "true", "true", None);
        let module_env = ModuleEnv::new();
        let artifacts = boxed
            .emit(&hir, Path::new("/tmp/handle_round_trip"), &module_env, &[])
            .expect("round-tripped emitter should remain callable");
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].kind, ArtifactKind::Header);
    }

    struct PanickingEmitter;

    impl Emitter for PanickingEmitter {
        fn emit(
            &self,
            _hir_atom: &HirAtom,
            _output_path: &Path,
            _module_env: &ModuleEnv,
            _extern_blocks: &[ExternBlock],
        ) -> MumeiResult<Vec<Artifact>> {
            panic!("plugin emit failure");
        }
    }

    #[test]
    fn test_panic_safe_emitter_catches_panic() {
        let wrapped = PanicSafeEmitter {
            inner: Box::new(PanickingEmitter),
            #[cfg(unix)]
            _lib: libloading::os::unix::Library::this().into(),
            #[cfg(windows)]
            _lib: libloading::os::windows::Library::this().unwrap().into(),
        };
        let hir = make_hir_atom("panic_plugin", vec![], "true", "true", None);
        let module_env = ModuleEnv::new();
        let err = wrapped
            .emit(&hir, Path::new("/tmp/panic_plugin"), &module_env, &[])
            .unwrap_err();
        let msg = format!("{}", err);
        assert!(
            msg.contains("External emitter panicked during emit"),
            "panic must be converted to a verification error; got: {msg}"
        );
    }
}
