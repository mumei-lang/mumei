use mumei_core::emitter::{Artifact, ArtifactKind, Emitter};
use mumei_core::hir::HirAtom;
use mumei_core::parser::ExternBlock;
use mumei_core::verification::{ModuleEnv, MumeiResult};
use serde::Serialize;
use std::path::Path;

/// A single parameter in the verified JSON output.
#[derive(Serialize, Debug)]
struct VerifiedParam {
    name: String,
    #[serde(rename = "type")]
    type_name: String,
    is_ref: bool,
    is_ref_mut: bool,
}

/// Top-level verified atom metadata.
#[derive(Serialize, Debug)]
struct VerifiedAtomJson {
    name: String,
    params: Vec<VerifiedParam>,
    requires: String,
    ensures: String,
    effects: Vec<String>,
    return_type: String,
    trust_level: String,
}

/// Emitter that outputs verified atom metadata as JSON.
pub struct VerifiedJsonEmitter;

impl Emitter for VerifiedJsonEmitter {
    fn emit(
        &self,
        hir_atom: &HirAtom,
        output_path: &Path,
        _module_env: &ModuleEnv,
        _extern_blocks: &[ExternBlock],
    ) -> MumeiResult<Vec<Artifact>> {
        let atom = &hir_atom.atom;

        let params: Vec<VerifiedParam> = atom
            .params
            .iter()
            .map(|p| VerifiedParam {
                name: p.name.clone(),
                type_name: p.type_name.clone().unwrap_or_else(|| "i64".to_string()),
                is_ref: p.is_ref,
                is_ref_mut: p.is_ref_mut,
            })
            .collect();

        let effects: Vec<String> = atom.effects.iter().map(|e| e.name.clone()).collect();

        let trust_level = format!("{:?}", atom.trust_level);

        let verified = VerifiedAtomJson {
            name: atom.name.clone(),
            params,
            requires: atom.requires.clone(),
            ensures: atom.ensures.clone(),
            effects,
            return_type: atom
                .return_type
                .clone()
                .unwrap_or_else(|| "i64".to_string()),
            trust_level,
        };

        let json_data = serde_json::to_string_pretty(&verified).map_err(|e| {
            mumei_core::verification::MumeiError::codegen(format!(
                "Failed to serialize verified JSON: {}",
                e
            ))
        })?;

        // NOTE: with_extension replaces the last extension. If atom.name contains a dot
        // (e.g., "net.get"), the output path may lose part of the name. This is a pre-existing
        // architectural pattern shared with LlvmEmitter (.ll) and CHeaderEmitter (.h).
        let json_path = output_path.with_extension("verified.json");

        Ok(vec![Artifact {
            name: json_path,
            data: json_data.into_bytes(),
            kind: ArtifactKind::Source,
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mumei_core::hir::{HirEffectSet, HirExpr, HirStmt};
    use mumei_core::parser::ast::{Atom, Expr, Param, Span, Stmt, TrustLevel};
    use mumei_core::verification::ModuleEnv;

    #[test]
    fn test_verified_json_emitter_basic() {
        let hir_atom = HirAtom {
            body: HirStmt::Expr(HirExpr::Number(0)),
            requires_hir: HirExpr::Number(1),
            ensures_hir: HirExpr::Number(1),
            atom: Atom {
                name: "safe_divide".to_string(),
                type_params: vec![],
                where_bounds: vec![],
                params: vec![
                    Param {
                        name: "a".to_string(),
                        type_name: Some("i64".to_string()),
                        type_ref: None,
                        is_ref: false,
                        is_ref_mut: false,
                        fn_contract_requires: None,
                        fn_contract_ensures: None,
                    },
                    Param {
                        name: "b".to_string(),
                        type_name: Some("i64".to_string()),
                        type_ref: None,
                        is_ref: false,
                        is_ref_mut: false,
                        fn_contract_requires: None,
                        fn_contract_ensures: None,
                    },
                ],
                requires: "b != 0".to_string(),
                forall_constraints: vec![],
                ensures: "result == a / b".to_string(),
                body_expr: "a / b".to_string(),
                consumed_params: vec![],
                resources: vec![],
                is_async: false,
                trust_level: TrustLevel::Verified,
                max_unroll: None,
                invariant: None,
                effects: vec![],
                return_type: Some("i64".to_string()),
                span: Span::default(),
                effect_pre: std::collections::HashMap::new(),
                effect_post: std::collections::HashMap::new(),
            },
            body_stmt: Stmt::Expr(Expr::Number(0), Span::default()),
            effect_set: HirEffectSet::default(),
        };

        let module_env = ModuleEnv::new();
        let artifacts = VerifiedJsonEmitter
            .emit(
                &hir_atom,
                std::path::Path::new("/tmp/safe_divide"),
                &module_env,
                &[],
            )
            .unwrap();

        assert_eq!(artifacts.len(), 1);
        assert_eq!(
            artifacts[0].name,
            std::path::PathBuf::from("/tmp/safe_divide.verified.json")
        );

        let json_str = String::from_utf8(artifacts[0].data.clone()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(value["name"], "safe_divide");
        assert_eq!(value["requires"], "b != 0");
        assert_eq!(value["ensures"], "result == a / b");
        assert_eq!(value["return_type"], "i64");
        assert_eq!(value["trust_level"], "Verified");
        assert_eq!(value["params"].as_array().unwrap().len(), 2);
        assert_eq!(value["params"][0]["name"], "a");
        assert_eq!(value["params"][0]["type"], "i64");
        assert_eq!(value["params"][0]["is_ref"], false);
        assert_eq!(value["params"][1]["name"], "b");
    }
}
