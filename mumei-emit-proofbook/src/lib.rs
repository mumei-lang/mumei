use mumei_core::emitter::{Artifact, ArtifactKind, Emitter};
use mumei_core::hir::HirAtom;
use mumei_core::parser::ExternBlock;
use mumei_core::verification::{ModuleEnv, MumeiResult};
use sha2::{Digest, Sha256};
use std::path::Path;

pub struct ProofBookEmitter;

impl Emitter for ProofBookEmitter {
    fn emit(
        &self,
        hir_atom: &HirAtom,
        output_path: &Path,
        module_env: &ModuleEnv,
        _extern_blocks: &[ExternBlock],
    ) -> MumeiResult<Vec<Artifact>> {
        let atom = &hir_atom.atom;
        let mut md = String::new();

        // Title
        md.push_str(&format!("# Proof Certificate: `{}`\n\n", atom.name));

        // Metadata table
        md.push_str("## Metadata\n\n");
        md.push_str("| Field | Value |\n");
        md.push_str("|-------|-------|\n");
        md.push_str(&format!("| **Atom** | `{}` |\n", atom.name));
        md.push_str(&format!("| **Trust Level** | `{:?}` |\n", atom.trust_level));
        md.push_str(&format!(
            "| **Mumei Version** | `{}` |\n",
            env!("CARGO_PKG_VERSION")
        ));
        let content_hash =
            compute_content_hash(&atom.name, &atom.requires, &atom.ensures, &atom.body_expr);
        md.push_str(&format!(
            "| **Content Hash** | `{}` |\n",
            &content_hash[..16]
        ));
        if atom.is_async {
            md.push_str("| **Async** | Yes |\n");
        }
        md.push('\n');

        // Signature
        md.push_str("## Signature\n\n");
        md.push_str("```mumei\n");
        let params_str: String = atom
            .params
            .iter()
            .map(|p| {
                let type_name = p.type_name.as_deref().unwrap_or("i64");
                let resolved = module_env.resolve_base_type(type_name);
                if resolved != type_name {
                    format!("{}: {} (= {})", p.name, type_name, resolved)
                } else {
                    format!("{}: {}", p.name, type_name)
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        let ret_type = atom.return_type.as_deref().unwrap_or("i64");
        md.push_str(&format!(
            "atom {}({}) -> {}\n",
            atom.name, params_str, ret_type
        ));
        md.push_str("```\n\n");

        // Contracts section
        md.push_str("## Formal Contracts\n\n");
        md.push_str("### Precondition (`requires`)\n\n");
        if atom.requires == "true" {
            md.push_str("No precondition (accepts all inputs).\n\n");
        } else {
            md.push_str(&format!("```\n{}\n```\n\n", atom.requires));
        }
        md.push_str("### Postcondition (`ensures`)\n\n");
        if atom.ensures == "true" {
            md.push_str("No postcondition specified.\n\n");
        } else {
            md.push_str(&format!("```\n{}\n```\n\n", atom.ensures));
        }

        // Effects section (only if effects exist)
        let effects: Vec<String> = atom.effects.iter().map(|e| e.name.clone()).collect();
        if !effects.is_empty() {
            md.push_str("## Effects\n\n");
            md.push_str("| Effect | Description |\n");
            md.push_str("|--------|-------------|\n");
            for eff in &effects {
                md.push_str(&format!("| `{}` | Declared effect |\n", eff));
            }
            md.push('\n');
        }

        // Temporal contracts (effect_pre / effect_post)
        if !atom.effect_pre.is_empty() || !atom.effect_post.is_empty() {
            md.push_str("## Temporal Contracts\n\n");
            if !atom.effect_pre.is_empty() {
                md.push_str("### Pre-state (`effect_pre`)\n\n");
                for (effect, state) in &atom.effect_pre {
                    md.push_str(&format!("- `{}`: `{}`\n", effect, state));
                }
                md.push('\n');
            }
            if !atom.effect_post.is_empty() {
                md.push_str("### Post-state (`effect_post`)\n\n");
                for (effect, state) in &atom.effect_post {
                    md.push_str(&format!("- `{}`: `{}`\n", effect, state));
                }
                md.push('\n');
            }
        }

        // Verification status
        md.push_str("## Verification Status\n\n");
        match atom.trust_level {
            mumei_core::parser::ast::TrustLevel::Verified => {
                md.push_str("> **VERIFIED** — All contracts proven by Z3 SMT solver.\n\n");
            }
            mumei_core::parser::ast::TrustLevel::Trusted => {
                md.push_str("> **TRUSTED** — Contracts assumed correct (not verified by Z3).\n\n");
            }
            mumei_core::parser::ast::TrustLevel::Unverified => {
                md.push_str("> **UNVERIFIED** — Contracts not yet verified; use with caution.\n\n");
            }
        }

        // Resources (if any)
        if !atom.resources.is_empty() {
            md.push_str("## Resources\n\n");
            for res in &atom.resources {
                md.push_str(&format!("- `{}`\n", res));
            }
            md.push('\n');
        }

        // Footer
        md.push_str("---\n\n");
        md.push_str("*Generated by Mumei Proof-Book Emitter*\n");

        let proof_md_path = output_path.with_extension("proof.md");

        Ok(vec![Artifact {
            name: proof_md_path,
            data: md.into_bytes(),
            kind: ArtifactKind::Source,
        }])
    }
}

fn compute_content_hash(name: &str, requires: &str, ensures: &str, body: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(name.as_bytes());
    hasher.update(b"\n---requires---\n");
    hasher.update(requires.as_bytes());
    hasher.update(b"\n---ensures---\n");
    hasher.update(ensures.as_bytes());
    hasher.update(b"\n---body---\n");
    hasher.update(body.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mumei_core::hir::{HirEffectSet, HirExpr, HirStmt};
    use mumei_core::parser::ast::{Atom, Effect, Expr, Param, Span, Stmt, TrustLevel};
    use mumei_core::verification::ModuleEnv;
    use std::collections::HashMap;

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

    fn make_hir_atom(
        name: &str,
        params: Vec<Param>,
        requires: &str,
        ensures: &str,
        body_expr: &str,
        trust_level: TrustLevel,
        effects: Vec<Effect>,
        effect_pre: HashMap<String, String>,
        effect_post: HashMap<String, String>,
        resources: Vec<String>,
        is_async: bool,
        return_type: Option<String>,
    ) -> HirAtom {
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
                body_expr: body_expr.to_string(),
                consumed_params: vec![],
                resources,
                is_async,
                trust_level,
                max_unroll: None,
                invariant: None,
                effects,
                return_type,
                span: Span::default(),
                effect_pre,
                effect_post,
            },
            body_stmt: Stmt::Expr(Expr::Number(0), Span::default()),
            effect_set: HirEffectSet::default(),
        }
    }

    fn make_simple_effect(name: &str) -> Effect {
        Effect::simple(name)
    }

    #[test]
    fn test_basic_atom_with_contracts() {
        let hir = make_hir_atom(
            "safe_divide",
            vec![make_param("a", Some("i64")), make_param("b", Some("i64"))],
            "b != 0",
            "result == a / b",
            "a / b",
            TrustLevel::Verified,
            vec![],
            HashMap::new(),
            HashMap::new(),
            vec![],
            false,
            Some("i64".to_string()),
        );
        let module_env = ModuleEnv::new();
        let artifacts = ProofBookEmitter
            .emit(&hir, Path::new("/tmp/safe_divide"), &module_env, &[])
            .unwrap();

        assert_eq!(artifacts.len(), 1);
        let md = String::from_utf8(artifacts[0].data.clone()).unwrap();

        assert!(md.contains("# Proof Certificate: `safe_divide`"));
        assert!(md.contains("| **Atom** | `safe_divide` |"));
        assert!(md.contains("| **Trust Level** | `Verified` |"));
        assert!(md.contains("## Formal Contracts"));
        assert!(md.contains("```\nb != 0\n```"));
        assert!(md.contains("```\nresult == a / b\n```"));
        assert!(md.contains("## Signature"));
        assert!(md.contains("atom safe_divide(a: i64, b: i64) -> i64"));
        assert!(md.contains("> **VERIFIED**"));
    }

    #[test]
    fn test_atom_with_effects() {
        let hir = make_hir_atom(
            "write_log",
            vec![make_param("msg", Some("Str"))],
            "true",
            "true",
            "perform Log(msg)",
            TrustLevel::Verified,
            vec![make_simple_effect("Log"), make_simple_effect("IO")],
            HashMap::new(),
            HashMap::new(),
            vec![],
            false,
            None,
        );
        let module_env = ModuleEnv::new();
        let artifacts = ProofBookEmitter
            .emit(&hir, Path::new("/tmp/write_log"), &module_env, &[])
            .unwrap();

        let md = String::from_utf8(artifacts[0].data.clone()).unwrap();
        assert!(md.contains("## Effects"));
        assert!(md.contains("| `Log` | Declared effect |"));
        assert!(md.contains("| `IO` | Declared effect |"));
    }

    #[test]
    fn test_atom_with_temporal_contracts() {
        let mut effect_pre = HashMap::new();
        effect_pre.insert("File".to_string(), "Open".to_string());
        let mut effect_post = HashMap::new();
        effect_post.insert("File".to_string(), "Closed".to_string());

        let hir = make_hir_atom(
            "close_file",
            vec![],
            "true",
            "true",
            "perform FileClose",
            TrustLevel::Verified,
            vec![],
            effect_pre,
            effect_post,
            vec![],
            false,
            None,
        );
        let module_env = ModuleEnv::new();
        let artifacts = ProofBookEmitter
            .emit(&hir, Path::new("/tmp/close_file"), &module_env, &[])
            .unwrap();

        let md = String::from_utf8(artifacts[0].data.clone()).unwrap();
        assert!(md.contains("## Temporal Contracts"));
        assert!(md.contains("### Pre-state (`effect_pre`)"));
        assert!(md.contains("- `File`: `Open`"));
        assert!(md.contains("### Post-state (`effect_post`)"));
        assert!(md.contains("- `File`: `Closed`"));
    }

    #[test]
    fn test_trusted_atom_shows_trusted_status() {
        let hir = make_hir_atom(
            "ffi_call",
            vec![],
            "true",
            "true",
            "0",
            TrustLevel::Trusted,
            vec![],
            HashMap::new(),
            HashMap::new(),
            vec![],
            false,
            None,
        );
        let module_env = ModuleEnv::new();
        let artifacts = ProofBookEmitter
            .emit(&hir, Path::new("/tmp/ffi_call"), &module_env, &[])
            .unwrap();

        let md = String::from_utf8(artifacts[0].data.clone()).unwrap();
        assert!(md.contains("> **TRUSTED**"));
        assert!(!md.contains("> **VERIFIED**"));
    }

    #[test]
    fn test_no_precondition_message() {
        let hir = make_hir_atom(
            "simple_add",
            vec![make_param("a", Some("i64")), make_param("b", Some("i64"))],
            "true",
            "result == a + b",
            "a + b",
            TrustLevel::Verified,
            vec![],
            HashMap::new(),
            HashMap::new(),
            vec![],
            false,
            Some("i64".to_string()),
        );
        let module_env = ModuleEnv::new();
        let artifacts = ProofBookEmitter
            .emit(&hir, Path::new("/tmp/simple_add"), &module_env, &[])
            .unwrap();

        let md = String::from_utf8(artifacts[0].data.clone()).unwrap();
        assert!(md.contains("No precondition (accepts all inputs)."));
    }

    #[test]
    fn test_artifact_path_has_proof_md_extension() {
        let hir = make_hir_atom(
            "foo",
            vec![],
            "true",
            "true",
            "0",
            TrustLevel::Verified,
            vec![],
            HashMap::new(),
            HashMap::new(),
            vec![],
            false,
            None,
        );
        let module_env = ModuleEnv::new();
        let artifacts = ProofBookEmitter
            .emit(&hir, Path::new("/tmp/foo"), &module_env, &[])
            .unwrap();

        assert_eq!(
            artifacts[0].name,
            std::path::PathBuf::from("/tmp/foo.proof.md")
        );
        assert_eq!(artifacts[0].kind, ArtifactKind::Source);
    }

    #[test]
    fn test_content_hash_present_and_nonempty() {
        let hir = make_hir_atom(
            "hash_test",
            vec![],
            "true",
            "true",
            "42",
            TrustLevel::Verified,
            vec![],
            HashMap::new(),
            HashMap::new(),
            vec![],
            false,
            None,
        );
        let module_env = ModuleEnv::new();
        let artifacts = ProofBookEmitter
            .emit(&hir, Path::new("/tmp/hash_test"), &module_env, &[])
            .unwrap();

        let md = String::from_utf8(artifacts[0].data.clone()).unwrap();
        assert!(md.contains("| **Content Hash** | `"));
        // Hash should be 16 hex chars (truncated from full SHA-256)
        let hash_line = md.lines().find(|l| l.contains("Content Hash")).unwrap();
        let hash_value = hash_line.split('`').nth(1).unwrap();
        assert_eq!(hash_value.len(), 16);
        assert!(hash_value.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_resources_section() {
        let hir = make_hir_atom(
            "guarded",
            vec![],
            "true",
            "true",
            "0",
            TrustLevel::Verified,
            vec![],
            HashMap::new(),
            HashMap::new(),
            vec!["Mutex".to_string(), "FileHandle".to_string()],
            false,
            None,
        );
        let module_env = ModuleEnv::new();
        let artifacts = ProofBookEmitter
            .emit(&hir, Path::new("/tmp/guarded"), &module_env, &[])
            .unwrap();

        let md = String::from_utf8(artifacts[0].data.clone()).unwrap();
        assert!(md.contains("## Resources"));
        assert!(md.contains("- `Mutex`"));
        assert!(md.contains("- `FileHandle`"));
    }

    #[test]
    fn test_async_atom_metadata() {
        let hir = make_hir_atom(
            "fetch_data",
            vec![],
            "true",
            "true",
            "0",
            TrustLevel::Verified,
            vec![],
            HashMap::new(),
            HashMap::new(),
            vec![],
            true,
            None,
        );
        let module_env = ModuleEnv::new();
        let artifacts = ProofBookEmitter
            .emit(&hir, Path::new("/tmp/fetch_data"), &module_env, &[])
            .unwrap();

        let md = String::from_utf8(artifacts[0].data.clone()).unwrap();
        assert!(md.contains("| **Async** | Yes |"));
    }

    #[test]
    fn test_content_hash_matches_proof_cert() {
        let hash = compute_content_hash("test_atom", "x > 0", "result > 0", "x + 1");
        // Same algorithm as mumei-core/src/proof_cert.rs compute_atom_content_hash
        let mut hasher = Sha256::new();
        hasher.update(b"test_atom");
        hasher.update(b"\n---requires---\n");
        hasher.update(b"x > 0");
        hasher.update(b"\n---ensures---\n");
        hasher.update(b"result > 0");
        hasher.update(b"\n---body---\n");
        hasher.update(b"x + 1");
        let expected = format!("{:x}", hasher.finalize());
        assert_eq!(hash, expected);
    }
}
