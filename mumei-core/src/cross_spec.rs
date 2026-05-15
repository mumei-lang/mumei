//! Cross-specification consistency verification.

#[path = "cross_spec/verifier.rs"]
mod verifier;

pub use verifier::{
    ContractConsistencyResult, CrossSpecResult, CrossSpecSummary, CrossSpecVerifier,
    DependencyNode, GlobalInvariant,
};
