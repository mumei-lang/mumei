//! Trust-boundary classification (P23 Proof-Aware Observability).
//!
//! A *trust boundary* is a place where a proof stops being self-contained and
//! starts relying on an assumption about the outside world. Only those places
//! are worth instrumenting at runtime: everything the verifier proved outright
//! stays uninstrumented (zero cost).
//!
//! The classification deliberately reuses criteria that already exist
//! elsewhere in the toolchain:
//!
//! - `trust_level: trusted` — the same flag `mcp_server.py`'s
//!   `visualize_std_graph` paints as a yellow node.
//! - `extern` / FFI declarations — the atom is backed by foreign code the
//!   verifier never saw.
//! - `effect_pre` — the atom overrides the effect state machine's initial
//!   state, so the proof assumes a caller-provided protocol state.

use crate::parser::ast::ExternFn;
use crate::parser::{Atom, ExternBlock, Param, TrustLevel};
use std::collections::HashMap;

/// Why an atom sits on a trust boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustBoundaryKind {
    /// `trust_level: trusted` — the contract is assumed, not proven.
    TrustedAtom,
    /// Backed by an `extern` (FFI) declaration.
    ExternBoundary,
    /// `effect_pre` overrides the declared initial effect state.
    EffectStateAssumption,
}

impl TrustBoundaryKind {
    /// Stable identifier used in generated code and in tests.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TrustedAtom => "trusted_atom",
            Self::ExternBoundary => "extern_ffi",
            Self::EffectStateAssumption => "effect_pre_override",
        }
    }

    /// Human readable rationale emitted as a doc comment.
    pub fn rationale(self) -> &'static str {
        match self {
            Self::TrustedAtom => {
                "atom is declared `trusted`, so its contract is assumed rather than proven"
            }
            Self::ExternBoundary => {
                "atom is backed by an `extern` declaration the verifier cannot inspect"
            }
            Self::EffectStateAssumption => {
                "atom overrides the effect state machine's initial state via `effect_pre`"
            }
        }
    }
}

/// Classify the trust boundaries an atom sits on.
///
/// Returns an empty vector for fully proven, pure atoms — those must stay
/// zero-cost and receive no runtime monitor.
pub fn classify_trust_boundaries(
    atom: &Atom,
    extern_blocks: &[ExternBlock],
) -> Vec<TrustBoundaryKind> {
    let mut kinds = Vec::new();

    if atom.trust_level == TrustLevel::Trusted {
        kinds.push(TrustBoundaryKind::TrustedAtom);
    }

    let is_extern = extern_blocks
        .iter()
        .flat_map(|block| block.functions.iter())
        .any(|f| f.name == atom.name || is_alias_of_extern(atom, f));
    if is_extern {
        kinds.push(TrustBoundaryKind::ExternBoundary);
    }

    if !atom.effect_pre.is_empty() {
        kinds.push(TrustBoundaryKind::EffectStateAssumption);
    }

    kinds
}

/// Whether a namespaced atom is the alias registration of `ext_fn`.
///
/// Importing an `extern` block under an alias registers the atom a second time
/// as `alias::name` (see `resolver::imports`), so the alias must be recognised
/// as the same FFI boundary. A verified method that merely ends in the same
/// segment (`Device::read` next to `extern fn read`) must not: it is matched
/// against the shape [`extern_fn_as_trusted_atom`] produces — assumed contract,
/// no body, identical signature.
fn is_alias_of_extern(atom: &Atom, ext_fn: &ExternFn) -> bool {
    let Some(prefix) = atom.name.strip_suffix(ext_fn.name.as_str()) else {
        return false;
    };
    prefix.ends_with("::")
        && atom.trust_level == TrustLevel::Trusted
        && atom.body_expr.trim().is_empty()
        && atom.params.len() == ext_fn.param_types.len()
        && atom.return_type.as_deref() == Some(ext_fn.return_type.as_str())
}

/// Whether an atom needs a runtime monitor at all.
pub fn is_trust_boundary(atom: &Atom, extern_blocks: &[ExternBlock]) -> bool {
    !classify_trust_boundaries(atom, extern_blocks).is_empty()
}

/// The trusted atom an `extern` declaration stands for.
///
/// An FFI function has no body to verify, so its `requires`/`ensures` are
/// assumptions: the atom carries them with `trust_level: trusted`, which is
/// what both the resolver and the runtime monitor emitter consume.
pub fn extern_fn_as_trusted_atom(ext_fn: &ExternFn) -> Atom {
    let params = ext_fn
        .param_types
        .iter()
        .enumerate()
        .map(|(i, ty)| Param {
            name: ext_fn
                .param_names
                .get(i)
                .cloned()
                .unwrap_or_else(|| format!("arg{}", i)),
            type_name: Some(ty.clone()),
            type_ref: Some(crate::parser::parse_type_ref(ty)),
            is_ref: false,
            is_ref_mut: false,
            fn_contract_requires: None,
            fn_contract_ensures: None,
        })
        .collect();

    Atom {
        name: ext_fn.name.clone(),
        type_params: vec![],
        where_bounds: vec![],
        params,
        trace_id: None,
        spec_metadata: HashMap::new(),
        requires: ext_fn
            .requires
            .clone()
            .unwrap_or_else(|| "true".to_string()),
        forall_constraints: vec![],
        ensures: ext_fn.ensures.clone().unwrap_or_else(|| "true".to_string()),
        body_expr: String::new(),
        consumed_params: vec![],
        resources: vec![],
        is_async: false,
        trust_level: TrustLevel::Trusted,
        max_unroll: None,
        invariant: None,
        effects: vec![],
        return_type: Some(ext_fn.return_type.clone()),
        span: ext_fn.span.clone(),
        effect_pre: HashMap::new(),
        effect_post: HashMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::ast::{ExternFn, Span};
    use std::collections::HashMap;

    fn atom(name: &str) -> Atom {
        Atom {
            name: name.to_string(),
            type_params: vec![],
            where_bounds: vec![],
            params: vec![],
            trace_id: None,
            spec_metadata: HashMap::new(),
            requires: "true".to_string(),
            forall_constraints: vec![],
            ensures: "true".to_string(),
            body_expr: "0".to_string(),
            consumed_params: vec![],
            resources: vec![],
            is_async: false,
            trust_level: TrustLevel::Verified,
            max_unroll: None,
            invariant: None,
            effects: vec![],
            return_type: None,
            span: Span::default(),
            effect_pre: HashMap::new(),
            effect_post: HashMap::new(),
        }
    }

    fn extern_block(fn_name: &str) -> ExternBlock {
        ExternBlock {
            language: "C".to_string(),
            functions: vec![ExternFn {
                name: fn_name.to_string(),
                param_names: vec![],
                param_types: vec![],
                return_type: "i64".to_string(),
                requires: None,
                ensures: None,
                span: Span::default(),
            }],
            span: Span::default(),
        }
    }

    #[test]
    fn proven_pure_atom_is_not_a_trust_boundary() {
        assert!(classify_trust_boundaries(&atom("pure_add"), &[]).is_empty());
        assert!(!is_trust_boundary(&atom("pure_add"), &[]));
    }

    #[test]
    fn trusted_atom_is_a_boundary() {
        let mut a = atom("read_clock");
        a.trust_level = TrustLevel::Trusted;
        assert_eq!(
            classify_trust_boundaries(&a, &[]),
            vec![TrustBoundaryKind::TrustedAtom]
        );
    }

    #[test]
    fn extern_backed_atom_is_a_boundary() {
        let blocks = vec![extern_block("native_hash")];
        assert_eq!(
            classify_trust_boundaries(&atom("native_hash"), &blocks),
            vec![TrustBoundaryKind::ExternBoundary]
        );
    }

    #[test]
    fn an_alias_of_an_extern_atom_is_a_boundary() {
        let blocks = vec![extern_block("native_hash")];
        let mut a = atom("ffi::native_hash");
        a.trust_level = TrustLevel::Trusted;
        a.body_expr = String::new();
        a.return_type = Some("i64".to_string());
        assert_eq!(
            classify_trust_boundaries(&a, &blocks),
            vec![
                TrustBoundaryKind::TrustedAtom,
                TrustBoundaryKind::ExternBoundary
            ]
        );
    }

    #[test]
    fn a_verified_method_sharing_an_extern_name_is_not_a_boundary() {
        let blocks = vec![extern_block("read")];
        assert!(classify_trust_boundaries(&atom("Device::read"), &blocks).is_empty());
    }

    #[test]
    fn effect_pre_override_is_a_boundary() {
        let mut a = atom("send_request");
        a.effect_pre
            .insert("OrderChannel".to_string(), "Idle".to_string());
        assert_eq!(
            classify_trust_boundaries(&a, &[]),
            vec![TrustBoundaryKind::EffectStateAssumption]
        );
    }
}
