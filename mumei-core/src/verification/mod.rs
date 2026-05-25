use crate::ast::TypeRef;
use crate::cross_spec::{CrossSpecResult, CrossSpecVerifier};
use crate::hir::HirAtom;
use crate::parser::{
    parse_body_expr, parse_expression, Atom, Effect, EffectDef, EnumDef, Expr, ImplDef, Item,
    JoinSemantics, MatchArm, Op, Pattern, QuantifierType, RefinedType, ResourceDef, ResourceMode,
    Span, Stmt, StructDef, TraitDef, TraitMethod, TrustLevel,
};
use miette::SourceSpan;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::Path;
use z3::ast::{Array, Ast, Bool, Dynamic, Float, Int, Real, String as Z3String};
use z3::{Config, Context, SatResult, Solver};

pub mod executor;
pub mod fragment;
pub mod module_env;
pub mod nlae_reporter;
pub mod property_based;
pub mod spec_validation;
pub mod spurious_detection;
pub(crate) mod support;
pub mod translator;
pub mod types;

#[cfg(test)]
mod tests;

pub use executor::*;
pub use fragment::*;
pub use module_env::*;
pub use nlae_reporter::*;
pub use property_based::*;
pub use spec_validation::*;
pub use spurious_detection::*;
pub use support::{
    infer_contracts_json, infer_effects_json, verify_impl, AllowedEffect, SecurityPolicy,
};
pub use types::*;

#[derive(thiserror::Error, Debug, Clone, serde::Serialize, serde::Deserialize)]
#[error("Spec contradiction in atom '{atom_name}': {message}")]
pub struct SpecContradiction {
    pub atom_name: String,
    pub kind: String,
    pub message: String,
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
        Self {
            atom_name: atom_name.into(),
            kind: kind.into(),
            message: message.into(),
            constraints,
            span,
        }
    }
}
