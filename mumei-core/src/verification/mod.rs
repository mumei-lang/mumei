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
pub use spurious_detection::*;
pub use support::{
    infer_contracts_json, infer_effects_json, verify_impl, AllowedEffect, SecurityPolicy,
};
pub use types::*;
