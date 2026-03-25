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
use crate::verification::{ModuleEnv, MumeiResult};
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
}
