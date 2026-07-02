use crate::ast::TypeRef;
use crate::cross_spec::{CrossSpecResult, CrossSpecVerifier};
use crate::hir::HirAtom;
use crate::parser::{
    parse_body_expr, parse_expression, Atom, EffectDef, EnumDef, Expr, ImplDef, JoinSemantics,
    MatchArm, Op, Pattern, QuantifierType, RefinedType, ResourceDef, Span, Stmt, StructDef,
    TraitDef, TraitMethod, TrustLevel,
};
use crate::resolver::compute_contract_hash;
use miette::SourceSpan;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::Path;
use z3::ast::{Array, Ast, Bool, Dynamic, Float, Int, Real, String as Z3String};
use z3::{Config, Context, SatResult, Solver};

pub mod executor;
pub mod fragment;
pub mod loop_detector;
pub mod module_env;
pub mod mutation;
pub mod nlae_reporter;
pub mod profiler;
pub mod property_based;
pub mod spec_validation;
pub mod spurious_detection;
pub(crate) mod support;
pub mod translator;
pub mod types;
pub mod vacuity;

#[cfg(test)]
mod tests;

pub use executor::*;
pub use fragment::*;
pub use loop_detector::*;
pub use module_env::*;
pub use mutation::{apply_mutation, generate_mutations, MutationOperator, MutationResult};
pub use nlae_reporter::*;
pub use profiler::{ConstraintProfile, IncrementalProfiler, SolverHeatmap};
pub use property_based::*;
pub use spec_validation::*;
pub use spurious_detection::*;
pub use support::{
    build_data_flow_trace, infer_contracts_json, infer_effects_json, verify_impl, AllowedEffect,
    DataFlowTrace, ExecutionStep, SecurityPolicy, VariableMutation, VariableState, ViolationInfo,
};
pub use types::*;
pub use vacuity::{
    check_spec_vacuity, check_spec_vacuity_for_hir, VacuityCheckResult, VacuityError,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractManifest {
    pub version: String,
    pub contract_hashes: HashMap<String, String>,
    pub timestamp: u64,
}

pub fn generate_contract_manifest(module_env: &ModuleEnv) -> ContractManifest {
    let mut contract_hashes = HashMap::new();

    for (atom_name, atom) in &module_env.atoms {
        contract_hashes.insert(atom_name.clone(), compute_contract_hash(atom));
    }

    ContractManifest {
        version: "1.0".to_string(),
        contract_hashes,
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    }
}

pub fn verify_contract_integrity(atom: &Atom, manifest: &ContractManifest) -> MumeiResult<()> {
    let expected_hash = manifest.contract_hashes.get(&atom.name).ok_or_else(|| {
        MumeiError::verification_at(
            format!("Atom '{}' not found in contract manifest", atom.name),
            atom.span.clone(),
        )
    })?;

    let actual_hash = compute_contract_hash(atom);

    if expected_hash != &actual_hash {
        return Err(MumeiError::contract_mutation(
            atom.name.clone(),
            expected_hash.clone(),
            actual_hash,
            atom.span.clone(),
        ));
    }

    Ok(())
}

#[derive(thiserror::Error, Debug, Clone, serde::Serialize, serde::Deserialize)]
#[error("Spec contradiction in atom '{atom_name}': {message}")]
pub struct SpecContradiction {
    pub atom_name: String,
    pub kind: String,
    pub message: String,
    pub natural_language_explanation: String,
    pub suggested_fix: String,
    pub constraints: Vec<String>,
    pub span: Span,
}

impl SpecContradiction {
    pub fn new(
        atom_name: impl Into<String>,
        kind: impl Into<String>,
        message: impl Into<String>,
        constraints: Vec<String>,
        span: Span,
    ) -> Self {
        let atom_name = atom_name.into();
        let kind = kind.into();
        let message = message.into();
        let natural_language_explanation =
            natural_language_explanation(&atom_name, &kind, &message, &constraints);
        let suggested_fix = suggested_fix_for_contradiction(&kind, &constraints);
        Self {
            atom_name,
            kind,
            message,
            natural_language_explanation,
            suggested_fix,
            constraints,
            span,
        }
    }
}

fn natural_language_explanation(
    atom_name: &str,
    kind: &str,
    message: &str,
    constraints: &[String],
) -> String {
    let constraint_text = if constraints.is_empty() {
        "no concrete constraint text was captured".to_string()
    } else {
        constraints.join(" AND ")
    };
    match kind {
        "requires_unsat" => format!(
            "The preconditions for atom '{atom_name}' cannot all be true at the same time: {constraint_text}. {message}."
        ),
        "ensures_unsat" => format!(
            "The postconditions for atom '{atom_name}' contradict the preconditions or each other: {constraint_text}. {message}."
        ),
        kind if kind.starts_with("refinement_") => format!(
            "A refinement type used by atom '{atom_name}' has incompatible bounds: {constraint_text}. {message}."
        ),
        _ => format!(
            "The specification for atom '{atom_name}' is internally inconsistent ({kind}): {constraint_text}. {message}."
        ),
    }
}

fn suggested_fix_for_contradiction(kind: &str, constraints: &[String]) -> String {
    let joined = constraints.join(" AND ");
    match kind {
        "requires_unsat" => format!(
            "Relax or remove one of the preconditions so the input domain is non-empty. Review: {joined}"
        ),
        "ensures_unsat" => format!(
            "Align the postconditions with the preconditions, or split mutually exclusive outcomes into separate guarded atoms. Review: {joined}"
        ),
        kind if kind.starts_with("refinement_") => format!(
            "Adjust the refinement predicate or parameter constraints so at least one value inhabits the type. Review: {joined}"
        ),
        _ => format!(
            "Inspect the listed constraints and make one of them conditional, weaker, or domain-specific. Review: {joined}"
        ),
    }
}
