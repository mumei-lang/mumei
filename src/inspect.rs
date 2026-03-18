// =============================================================================
// Plan 11A: mumei inspect --ai — Structured JSON inspection report
// =============================================================================
//
// Generates a structured JSON report of a Mumei source file for AI agent
// consumption. Includes all atoms, enums, structs, effects, and verification
// status.

use crate::parser::{Atom, EnumDef, Item, StructDef};
use crate::verification::ModuleEnv;
use serde::Serialize;

/// Top-level inspection report for a Mumei source file.
#[derive(Debug, Serialize)]
pub struct InspectReport {
    /// Mumei compiler version
    pub version: String,
    /// Source file path
    pub file: String,
    /// All atoms in the file
    pub atoms: Vec<AtomReport>,
    /// All enum definitions
    pub enums: Vec<EnumReport>,
    /// All struct definitions
    pub structs: Vec<StructReport>,
    /// All effect definitions
    pub effects: Vec<EffectReport>,
    /// Verification summary
    pub verification: VerificationSummary,
}

/// Report for a single atom (function).
#[derive(Debug, Serialize)]
pub struct AtomReport {
    pub name: String,
    pub params: Vec<ParamReport>,
    pub return_type: Option<String>,
    pub requires: String,
    pub ensures: String,
    pub effects: Vec<String>,
    pub trust_level: String,
    pub is_async: bool,
    pub verification_status: String,
}

/// Report for a single parameter.
#[derive(Debug, Serialize)]
pub struct ParamReport {
    pub name: String,
    pub type_name: Option<String>,
    pub is_ref: bool,
    pub is_ref_mut: bool,
}

/// Report for an effect definition.
#[derive(Debug, Serialize)]
pub struct EffectReport {
    pub name: String,
    pub params: Vec<String>,
    pub constraint: Option<String>,
    pub includes: Vec<String>,
}

/// Report for an enum definition.
#[derive(Debug, Serialize)]
pub struct EnumReport {
    pub name: String,
    pub type_params: Vec<String>,
    pub variants: Vec<VariantReport>,
}

/// Report for an enum variant.
#[derive(Debug, Serialize)]
pub struct VariantReport {
    pub name: String,
    pub fields: Vec<String>,
}

/// Report for a struct definition.
#[derive(Debug, Serialize)]
pub struct StructReport {
    pub name: String,
    pub type_params: Vec<String>,
    pub fields: Vec<FieldReport>,
}

/// Report for a struct field.
#[derive(Debug, Serialize)]
pub struct FieldReport {
    pub name: String,
    pub type_name: String,
}

/// Summary of verification results.
#[derive(Debug, Serialize)]
pub struct VerificationSummary {
    pub total_atoms: usize,
    pub verified: usize,
    pub failed: usize,
    pub skipped: usize,
}

/// Generate an inspection report from parsed items and module environment.
pub fn generate_report(
    file: &str,
    items: &[Item],
    module_env: &ModuleEnv,
    verification_results: &std::collections::HashMap<String, bool>,
) -> InspectReport {
    let mut atoms = Vec::new();
    let mut enums = Vec::new();
    let mut structs = Vec::new();
    let mut effects = Vec::new();
    let mut verified = 0;
    let mut failed = 0;
    let mut skipped = 0;

    for item in items {
        match item {
            Item::Atom(atom) => {
                let status = match verification_results.get(&atom.name) {
                    Some(true) => {
                        verified += 1;
                        "verified"
                    }
                    Some(false) => {
                        failed += 1;
                        "failed"
                    }
                    None => {
                        skipped += 1;
                        "skipped"
                    }
                };
                atoms.push(atom_to_report(atom, status));
            }
            Item::EnumDef(enum_def) => {
                enums.push(enum_to_report(enum_def));
            }
            Item::StructDef(struct_def) => {
                structs.push(struct_to_report(struct_def));
            }
            Item::EffectDef(effect_def) => {
                effects.push(EffectReport {
                    name: effect_def.name.clone(),
                    params: effect_def.params.iter().map(|p| p.name.clone()).collect(),
                    constraint: effect_def.constraint.clone(),
                    includes: effect_def.includes.clone(),
                });
            }
            _ => {}
        }
    }

    // Also check ModuleEnv for effects registered from std/prelude
    for (name, def) in &module_env.effects {
        if !effects.iter().any(|e| &e.name == name) {
            effects.push(EffectReport {
                name: name.clone(),
                params: def.params.iter().map(|p| p.name.clone()).collect(),
                constraint: def.constraint.clone(),
                includes: def.includes.clone(),
            });
        }
    }

    InspectReport {
        version: env!("CARGO_PKG_VERSION").to_string(),
        file: file.to_string(),
        atoms,
        enums,
        structs,
        effects,
        verification: VerificationSummary {
            total_atoms: verified + failed + skipped,
            verified,
            failed,
            skipped,
        },
    }
}

fn atom_to_report(atom: &Atom, status: &str) -> AtomReport {
    AtomReport {
        name: atom.name.clone(),
        params: atom
            .params
            .iter()
            .map(|p| ParamReport {
                name: p.name.clone(),
                type_name: p.type_name.clone(),
                is_ref: p.is_ref,
                is_ref_mut: p.is_ref_mut,
            })
            .collect(),
        return_type: atom.return_type.clone(),
        requires: atom.requires.clone(),
        ensures: atom.ensures.clone(),
        effects: atom.effects.iter().map(|e| e.name.clone()).collect(),
        trust_level: format!("{:?}", atom.trust_level),
        is_async: atom.is_async,
        verification_status: status.to_string(),
    }
}

fn enum_to_report(enum_def: &EnumDef) -> EnumReport {
    EnumReport {
        name: enum_def.name.clone(),
        type_params: enum_def.type_params.clone(),
        variants: enum_def
            .variants
            .iter()
            .map(|v| VariantReport {
                name: v.name.clone(),
                fields: v.fields.clone(),
            })
            .collect(),
    }
}

fn struct_to_report(struct_def: &StructDef) -> StructReport {
    StructReport {
        name: struct_def.name.clone(),
        type_params: struct_def.type_params.clone(),
        fields: struct_def
            .fields
            .iter()
            .map(|f| FieldReport {
                name: f.name.clone(),
                type_name: f.type_name.clone(),
            })
            .collect(),
    }
}
