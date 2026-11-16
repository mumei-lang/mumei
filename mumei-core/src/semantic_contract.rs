//! An opt-in, deliberately small description of one scalar source obligation.
//! Extraction is not verification or translation validation.

use crate::hir::lower_atom_to_hir;
use crate::mir::lower_hir_to_mir;
use crate::parser::{
    parse_body_expr_checked, parse_expression_checked, parse_module_checked, Atom, Expr, Item, Op,
    Pattern, Span, Stmt, TrustLevel,
};
use crate::proof_cert::compute_atom_content_hash_v2;
use crate::verification::{
    build_translator_ir_metadata, ModuleEnv, TranslatorIRMetadata, LEAN_BRIDGE_LEMMA_HASH,
    LEAN_TRANSLATOR_VERSION,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use z3::{
    ast::{Ast, Bool, BV},
    Context, FuncDecl, Sort,
};

pub const SEMANTIC_CONTRACT_VERSION: &str = "mumei.semantic-contract/v1";
pub const PURE_I64_PROFILE: &str = "mumei.pure-i64-comparison/v1";
/// The wider source fragment checked inside the LLVM equivalence profile:
/// the same scalar comparisons plus wrapping i64 add/sub/mul, total
/// bitwise `&`/`|`/`^`, guarded div/rem/shl/shr (defined only for nonzero
/// divisors excluding
/// i64::MIN / -1 and shift amounts in 0..64), block-scoped `let`
/// bindings, scalar reassignments on the linear path, nested blocks,
/// discarded pure expression statements, `bool` parameters and results
/// lowered to i64 carrying 0/1, first-match-order `match` on an i64
/// scrutinee with literal patterns and a trailing catch-all arm, and
/// calls modeled as uninterpreted call-site functions over i64
/// arguments (the callee's own contract is a separate obligation). Kept
/// distinct from the
/// bounded C03 contract, which still excludes arithmetic and sequences.
pub const PURE_I64_WRAPPING_PROFILE: &str = "mumei.pure-i64-wrapping-scalar/v1";

#[derive(Debug, thiserror::Error)]
pub enum SemanticContractError {
    #[error("source or expression failed checked parsing")]
    Parse,
    #[error("unsupported by the pure i64 comparison profile: {0}")]
    Unsupported(&'static str),
    #[error("cannot encode an internal representation")]
    Encoding,
}

#[derive(Debug, Clone, Serialize)]
pub struct TypedBinder {
    pub name: String,
    pub scalar_type: &'static str,
    pub role: &'static str,
    pub provenance: Span,
    pub span_precision: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScalarSpecification {
    pub binders: Vec<TypedBinder>,
    pub requires: String,
    pub ensures: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepresentationIdentity {
    pub encoding: &'static str,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NumericSemantics {
    pub source_integer: &'static str,
    pub verifier_integer: &'static str,
    pub llvm_integer: &'static str,
    pub comparisons: &'static str,
    pub arithmetic: &'static str,
    pub division: &'static str,
    pub shifts: &'static str,
    pub bitwise: &'static str,
    pub conversions: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct SemanticContract {
    pub schema_version: &'static str,
    pub profile: &'static str,
    pub atom: String,
    pub source_sha256: String,
    pub atom_content_hash: String,
    pub specification_sha256: String,
    pub implementation_sha256: String,
    pub obligation_id: String,
    pub specification: ScalarSpecification,
    pub source_span: Span,
    pub numeric_semantics: NumericSemantics,
    pub hir: RepresentationIdentity,
    pub mir: RepresentationIdentity,
    pub translator_ir: TranslatorIRMetadata,
    pub translator_version: &'static str,
    pub bridge_lemma_hash: &'static str,
    pub compiler_version: &'static str,
    pub requested_target_triple: Option<String>,
    pub backend_requirements: Vec<&'static str>,
    pub requested_route: &'static str,
    pub decomposition: &'static str,
    pub normalization: &'static str,
    pub correspondence: &'static str,
    pub assumptions: Vec<&'static str>,
    pub exclusions: Vec<&'static str>,
}

/// Extract the last atom of a module of explicit-i64 atoms without imports,
/// arithmetic or effects; the widened profile also admits calls as
/// call-site comparisons. Sibling atoms must satisfy the same atom profile.
/// The target is caller-supplied metadata, not a claim of backend availability.
/// `allow_wrapping_arithmetic` admits i64 add/sub/mul as mod-2^64 wrapping
/// operations, total bitwise `&`/`|`/`^`, plus guarded div/rem/shl/shr (a
/// partial domain: nonzero divisor, no i64::MIN / -1, shift amount in 0..64),
/// block-scoped `let` bindings, scalar reassignments on the linear path
/// (a rebinding cannot escape an `if` branch, expression block or nested
/// block), discarded pure expression statements, `bool` parameters
/// and results lowered to i64 carrying 0/1, `match` on an i64
/// scrutinee with literal patterns and a trailing catch-all arm, and
/// calls to i64-valued siblings modeled as uninterpreted call-site
/// functions (a callee's own contract is a separate obligation);
/// the bounded C03
/// contract always passes `false` so the build
/// and recheck paths keep agreeing on a fragment with no arithmetic.
pub fn extract_pure_i64_contract(
    source: &str,
    source_name: &str,
    target_triple: Option<&str>,
    allow_wrapping_arithmetic: bool,
) -> Result<SemanticContract, SemanticContractError> {
    if source_name.trim().is_empty() || target_triple.is_some_and(|target| target.trim().is_empty())
    {
        return Err(SemanticContractError::Unsupported(
            "missing source or target identity",
        ));
    }
    let items = admit_pure_i64_atoms(source, allow_wrapping_arithmetic)?;
    // The bounded profile stays a single-atom contract — sibling atoms
    // only serve as call targets, which the widened profile alone admits.
    if !allow_wrapping_arithmetic && items.len() != 1 {
        return Err(SemanticContractError::Unsupported(
            "expected exactly one atom",
        ));
    }
    let Some(Item::Atom(atom)) = items.into_iter().last() else {
        return Err(SemanticContractError::Unsupported("non-atom declarations"));
    };
    atom_contract(
        atom,
        source,
        source_name,
        target_triple,
        allow_wrapping_arithmetic,
    )
}

/// Extract one `SemanticContract` per atom of a multi-atom module under the
/// widened (wrapping) profile, in declaration order. The single-atom
/// contract stays the bounded fragment; siblings are admitted only where
/// the widened profile already covers them — every atom must independently
/// satisfy `validate_atom` and the complete-callee-map contract check, and
/// each emitted contract's `source_sha256` binds the whole module text.
pub fn extract_pure_i64_module_contracts(
    source: &str,
    source_name: &str,
    target_triple: Option<&str>,
) -> Result<Vec<SemanticContract>, SemanticContractError> {
    if source_name.trim().is_empty() || target_triple.is_some_and(|target| target.trim().is_empty())
    {
        return Err(SemanticContractError::Unsupported(
            "missing source or target identity",
        ));
    }
    admit_pure_i64_atoms(source, true)?
        .into_iter()
        .map(|item| match item {
            Item::Atom(atom) => atom_contract(atom, source, source_name, target_triple, true),
            _ => Err(SemanticContractError::Unsupported("non-atom declarations")),
        })
        .collect()
}

/// Shared module admission for the pure-i64 fragment: float reject, parse,
/// nonempty, callee-map construction and the per-atom
/// `validate_atom`/`check_atom_contract` gates. Returns the atom items in
/// declaration order. The caller decides the arity gate.
fn admit_pure_i64_atoms(
    source: &str,
    allow_wrapping_arithmetic: bool,
) -> Result<Vec<Item>, SemanticContractError> {
    // Item text reconstruction can print 1.0 as 1. Reject float tokens before
    // that normalization erases the category required by this integer profile.
    let tokens = crate::parser::lexer::Lexer::new(source)
        .tokenize_checked()
        .map_err(|_| SemanticContractError::Parse)?;
    if tokens
        .iter()
        .any(|token| matches!(token.token, crate::parser::token::Token::FloatLit(_)))
    {
        return Err(SemanticContractError::Unsupported(
            "floating-point literals",
        ));
    }
    let items = parse_module_checked(source).map_err(|_| SemanticContractError::Parse)?;
    if items.is_empty() {
        return Err(SemanticContractError::Unsupported(
            "expected at least one atom",
        ));
    }
    // Sibling atoms are potential call targets: each must independently
    // satisfy the atom profile. A call itself is modeled as an
    // uninterpreted function site — a callee's own contract is a separate
    // per-function obligation.
    let mut callees = BTreeMap::new();
    for item in &items {
        let Item::Atom(sibling) = item else {
            return Err(SemanticContractError::Unsupported("non-atom declarations"));
        };
        if callees
            .insert(sibling.name.clone(), sibling.params.len())
            .is_some()
        {
            return Err(SemanticContractError::Unsupported("duplicate atom name"));
        }
    }
    // Declaration order is irrelevant to call resolution: the callee map is
    // complete before any atom's own contract is checked, so a forward call
    // to a later sibling resolves the same as a backward one.
    for item in &items {
        let Item::Atom(sibling) = item else {
            return Err(SemanticContractError::Unsupported("non-atom declarations"));
        };
        validate_atom(sibling, allow_wrapping_arithmetic)?;
        // Every atom's contract is checked against the complete callee map,
        // not only the tail's: module validation compares each atom
        // independently, so an undeclared call target in any sibling must
        // fail closed here rather than encode as an uninterpreted
        // application that trivially agrees with an external IR member.
        check_atom_contract(sibling, &callees, allow_wrapping_arithmetic)?;
    }
    Ok(items)
}

/// Build the `SemanticContract` for one already-admitted atom — binders,
/// specification hash, HIR/MIR/translator-IR identities and the profile
/// fields all describe this atom while `source_sha256` binds the whole
/// module text.
fn atom_contract(
    mut atom: Atom,
    source: &str,
    source_name: &str,
    target_triple: Option<&str>,
    allow_wrapping_arithmetic: bool,
) -> Result<SemanticContract, SemanticContractError> {
    atom.span.file = source_name.to_owned();

    let mut binders: Vec<_> = atom
        .params
        .iter()
        .map(|p| TypedBinder {
            name: p.name.clone(),
            scalar_type: match p.type_name.as_deref() {
                Some("bool") => "bool",
                Some("i8") => "i8",
                Some("i16") => "i16",
                Some("i32") => "i32",
                Some("u8") => "u8",
                Some("u16") => "u16",
                Some("u32") => "u32",
                _ => "i64",
            },
            role: "parameter",
            provenance: atom.span.clone(),
            span_precision: "atom",
        })
        .collect();
    binders.push(TypedBinder {
        name: "result".to_owned(),
        scalar_type: match atom.return_type.as_deref() {
            Some("bool") => "bool",
            Some("i8") => "i8",
            Some("i16") => "i16",
            Some("i32") => "i32",
            Some("u8") => "u8",
            Some("u16") => "u16",
            Some("u32") => "u32",
            _ => "i64",
        },
        role: "result",
        provenance: atom.span.clone(),
        span_precision: "atom",
    });
    let specification = ScalarSpecification {
        binders,
        requires: atom.requires.clone(),
        ensures: atom.ensures.clone(),
    };
    // Provenance is separate from the logical identity; file relocation does not
    // change the obligation, while the full source digest still binds its text.
    let specification_sha256 = json_digest(&serde_json::json!({
        "profile": if allow_wrapping_arithmetic {
            PURE_I64_WRAPPING_PROFILE
        } else {
            PURE_I64_PROFILE
        },
        // The declared parameter types are part of the specification
        // identity: `b: bool` and `b: i64` admit different input domains
        // and must not share one specification hash.
        "params": atom
            .params
            .iter()
            .map(|p| serde_json::json!({"name": p.name, "type": p.type_name}))
            .collect::<Vec<_>>(),
        "return_type": atom.return_type,
        "requires": atom.requires, "ensures": atom.ensures,
    }))?;
    let hir = lower_atom_to_hir(&atom);
    let mir = lower_hir_to_mir(&hir);
    Ok(SemanticContract {
        schema_version: SEMANTIC_CONTRACT_VERSION,
        profile: if allow_wrapping_arithmetic {
            PURE_I64_WRAPPING_PROFILE
        } else {
            PURE_I64_PROFILE
        },
        atom: atom.name.clone(),
        source_sha256: digest(source.as_bytes()),
        atom_content_hash: compute_atom_content_hash_v2(&atom),
        specification_sha256: specification_sha256.clone(),
        implementation_sha256: digest(atom.body_expr.as_bytes()),
        obligation_id: format!("{}:postcondition:{}", atom.name, specification_sha256),
        specification,
        source_span: atom.span.clone(),
        numeric_semantics: NumericSemantics {
            source_integer: "signed_i64",
            verifier_integer: "mathematical_integer",
            llvm_integer: "i64_twos_complement",
            comparisons: "signed",
            arithmetic: if allow_wrapping_arithmetic {
                "wrapping_i64_add_sub_mul_no_overflow_claim"
            } else {
                "unsupported_no_overflow_claim"
            },
            division: if allow_wrapping_arithmetic {
                "signed_i64_truncating_defined_only_for_nonzero_divisor_excluding_min_div_neg1"
            } else {
                "unsupported"
            },
            shifts: if allow_wrapping_arithmetic {
                "shl_ashr_defined_only_for_amounts_0_to_63"
            } else {
                "unsupported"
            },
            bitwise: if allow_wrapping_arithmetic {
                "i64_bitand_bitor_bitxor_total"
            } else {
                "unsupported"
            },
            conversions: "unsupported",
        },
        // HIR has no stable serialization. This v0 diagnostic fingerprint is
        // compiler-local, not a portable semantic IR or a cross-version cache key.
        hir: RepresentationIdentity {
            encoding: "mumei.hir-body.debug/v0",
            sha256: digest(format!("{:?}", hir.body).as_bytes()),
        },
        mir: RepresentationIdentity {
            encoding: "mumei.mir-body.json/v0",
            sha256: json_digest(&mir)?,
        },
        translator_ir: build_translator_ir_metadata(&atom, &ModuleEnv::default()),
        translator_version: LEAN_TRANSLATOR_VERSION,
        bridge_lemma_hash: LEAN_BRIDGE_LEMMA_HASH,
        compiler_version: env!("CARGO_PKG_VERSION"),
        requested_target_triple: target_triple.map(str::to_owned),
        backend_requirements: vec![
            "mathematical_integer",
            "boolean",
            "signed_comparison",
            "ite",
        ]
        .into_iter()
        .chain(allow_wrapping_arithmetic.then_some("wrapping_i64_arithmetic"))
        .chain(allow_wrapping_arithmetic.then_some("i64_bitwise_total"))
        .chain(allow_wrapping_arithmetic.then_some("lexical_scoped_let_bindings"))
        .chain(allow_wrapping_arithmetic.then_some("scalar_reassignment_linear_path"))
        .chain(allow_wrapping_arithmetic.then_some("nested_block_sequences"))
        .chain(allow_wrapping_arithmetic.then_some("discarded_pure_expression_statements"))
        .chain(allow_wrapping_arithmetic.then_some("guarded_i64_division_shift_partial_domain"))
        .chain(allow_wrapping_arithmetic.then_some("bool_parameters_lowered_to_i64_zero_one"))
        .chain(allow_wrapping_arithmetic.then_some("subrange_integer_parameters_lowered_to_i64"))
        .chain(allow_wrapping_arithmetic.then_some("subrange_integer_results_lowered_to_i64"))
        .chain(allow_wrapping_arithmetic.then_some("bool_results_lowered_to_i64_zero_one"))
        .chain(allow_wrapping_arithmetic.then_some("first_match_order_i64_match_with_catch_all"))
        .chain(allow_wrapping_arithmetic.then_some("match_arm_boolean_guards"))
        .chain(allow_wrapping_arithmetic.then_some("conditional_path_reassignment_ite_merge"))
        .chain(allow_wrapping_arithmetic.then_some("match_arm_reassignment_ite_merge"))
        .chain(allow_wrapping_arithmetic.then_some("bounded_while_loop_unrolling"))
        .collect(),
        requested_route: "standard_z3_not_executed_by_extractor",
        decomposition: "no_common_obligation_decomposition",
        normalization:
            "checked_parser_comparison_chains_to_conjunction_not_independently_validated",
        correspondence: "unchecked_not_translation_validated",
        assumptions: vec![if allow_wrapping_arithmetic {
            "Inputs are representable signed 64-bit integers; declared booleans carry 0/1."
        } else {
            "Inputs are representable signed 64-bit integers."
        }],
        exclusions: vec![
            "Extraction does not prove the postcondition or precondition satisfiability.",
            "HIR/MIR fingerprints identify internal snapshots, not semantic equivalence.",
            "Target availability, LLVM lowering and final machine code are not verified.",
            if allow_wrapping_arithmetic {
                "No unsigned division, logical shifts, heap, FFI, ownership, effects or concurrency in this profile. Arithmetic wraps mod 2^64; overflow is not flagged or proven. Division, remainder and shifts bind inputs only where defined. Calls are call-site comparisons only: the callee's own contract is a separate obligation. `while` loops are claimed only where every guard fails within the profile bound (4 evaluations); deeper executions are outside the compared domain."
            } else {
                "No arithmetic, heap, calls, FFI, ownership, effects or concurrency in this profile."
            },
        ],
    })
}

/// The scalar kind a declared parameter type gives in this profile, when
/// the declaration is admissible: `i64` always; `bool` and the narrower
/// subrange integer types only when `admit_widened` (the widened
/// profile's `allow_wrapping_arithmetic` flag). Subrange types are
/// carried by i64 like `bool`; the declared signature then bounds the
/// checked input to the type's range (`param_domain`). `u64`, `usize`
/// and `isize` stay out: a full-width unsigned domain cannot be
/// expressed as an i64 range bound.
fn declared_param_kind(
    param: &crate::parser::ast::Param,
    admit_widened: bool,
) -> Option<ScalarKind> {
    let name = param.type_name.as_deref()?;
    let kind = match name {
        "i64" => ScalarKind::I64,
        "bool" if admit_widened => ScalarKind::Bool,
        "i8" | "i16" | "i32" | "u8" | "u16" | "u32" if admit_widened => ScalarKind::I64,
        _ => return None,
    };
    if param.type_ref.as_ref().is_some_and(|ty| {
        ty.name != name
            || !ty.type_args.is_empty()
            || ty.effect_set.is_some()
            || ty.capability.is_some()
    }) || param.consume
        || param.is_ref
        || param.is_ref_mut
        || param.fn_contract_requires.is_some()
        || param.fn_contract_ensures.is_some()
    {
        return None;
    }
    Some(kind)
}

/// The parameter kind inside a contract that already passed
/// `validate_atom` — unvalidated sources never reach the kind checker.
fn param_kind(param: &crate::parser::ast::Param) -> ScalarKind {
    if param.type_name.as_deref() == Some("bool") {
        ScalarKind::Bool
    } else {
        ScalarKind::I64
    }
}

/// The kind the declared return type gives in this profile — `i64`, or
/// `bool` when the widened profile admitted the declaration.
fn return_kind(atom: &Atom) -> ScalarKind {
    if atom.return_type.as_deref() == Some("bool") {
        ScalarKind::Bool
    } else {
        ScalarKind::I64
    }
}

/// The tail-atom contract admission shared by every module atom:
/// requires/ensures must be boolean expressions and the body a statement
/// of the declared result kind — all under call resolution against the
/// complete same-module callee map.
fn check_atom_contract(
    atom: &Atom,
    callees: &BTreeMap<String, usize>,
    allow_wrapping_arithmetic: bool,
) -> Result<(), SemanticContractError> {
    let scope: BTreeMap<String, ScalarKind> = atom
        .params
        .iter()
        .map(|p| (p.name.clone(), param_kind(p)))
        .collect();
    let requires =
        parse_expression_checked(&atom.requires).map_err(|_| SemanticContractError::Parse)?;
    let ensures =
        parse_expression_checked(&atom.ensures).map_err(|_| SemanticContractError::Parse)?;
    let body =
        parse_body_expr_checked(&atom.body_expr).map_err(|_| SemanticContractError::Parse)?;
    expect_kind(
        expression_kind(
            &requires,
            &scope,
            false,
            allow_wrapping_arithmetic,
            &BTreeSet::new(),
            callees,
        )?,
        ScalarKind::Bool,
    )?;
    let result_kind = return_kind(atom);
    let mut ensures_scope = scope.clone();
    ensures_scope.insert("result".to_owned(), result_kind);
    expect_kind(
        expression_kind(
            &ensures,
            &ensures_scope,
            true,
            allow_wrapping_arithmetic,
            &BTreeSet::new(),
            callees,
        )?,
        ScalarKind::Bool,
    )?;
    expect_kind(
        statement_kind(
            &body,
            &scope,
            false,
            allow_wrapping_arithmetic,
            &scope.keys().cloned().collect(),
            callees,
        )?,
        result_kind,
    )?;
    Ok(())
}

fn validate_atom(atom: &Atom, admit_widened: bool) -> Result<(), SemanticContractError> {
    // A declared subrange integer result lowers to an i64 return like
    // the parameters — the emitter emits no truncation, so the checked
    // result is the body's full i64 value. `u64`-sized results stay out
    // on the same signature boundary as the parameters. Admitting them
    // is likewise widened-profile only.
    let return_ok = match atom.return_type.as_deref() {
        Some("i64") => true,
        Some("bool") | Some("i8" | "i16" | "i32" | "u8" | "u16" | "u32") => admit_widened,
        _ => false,
    };
    if !return_ok
        || atom
            .params
            .iter()
            .any(|p| declared_param_kind(p, admit_widened).is_none())
    {
        return Err(SemanticContractError::Unsupported(
            "explicit owned scalar i64 signature required",
        ));
    }
    let mut names = BTreeSet::new();
    if atom
        .params
        .iter()
        .any(|p| matches!(p.name.as_str(), "result" | "true" | "false") || !names.insert(&p.name))
    {
        return Err(SemanticContractError::Unsupported(
            "duplicate or reserved binder",
        ));
    }
    if atom.trust_level != TrustLevel::Verified
        || atom.is_async
        || atom.has_consumed_params()
        || !atom.type_params.is_empty()
        || !atom.where_bounds.is_empty()
        || !atom.resources.is_empty()
        || !atom.effects.is_empty()
        || !atom.effect_pre.is_empty()
        || !atom.effect_post.is_empty()
        || !atom.forall_constraints.is_empty()
        || !atom.clause_modes.is_empty()
        || !atom.clause_labels.is_empty()
        || !atom.covers.is_empty()
        || !atom.spec_metadata.is_empty()
        || atom.invariant.is_some()
        || atom.decreases.is_some()
        || atom.max_unroll.is_some()
        || atom.trace_id.is_some()
    {
        return Err(SemanticContractError::Unsupported(
            "semantic modes, assumptions or annotations outside profile",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScalarKind {
    I64,
    Bool,
}

fn expect_kind(actual: ScalarKind, expected: ScalarKind) -> Result<(), SemanticContractError> {
    if actual == expected {
        Ok(())
    } else {
        Err(SemanticContractError::Unsupported("scalar kind mismatch"))
    }
}

/// True when the statement subtree holds a `while` at any depth —
/// statements nested inside expression blocks, `if` branches, match
/// arms and task/acquire bodies all count. Admission uses it to keep
/// nested loops out of the bounded-unroll profile.
fn contains_while_stmt(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::While { .. } => true,
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => contains_while_expr(value),
        Stmt::ArrayStore { index, value, .. } => {
            contains_while_expr(index) || contains_while_expr(value)
        }
        Stmt::Block(stmts, _)
        | Stmt::TaskGroup {
            children: stmts, ..
        } => stmts.iter().any(contains_while_stmt),
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => contains_while_stmt(body),
        Stmt::Expr(value, _) => contains_while_expr(value),
        Stmt::Cancel { .. } => false,
    }
}

fn contains_while_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Number(_)
        | Expr::Float(_)
        | Expr::StringLit(_)
        | Expr::Variable(_)
        | Expr::AtomRef { .. } => false,
        Expr::ArrayLit(items) | Expr::Call(_, items) | Expr::Perform { args: items, .. } => {
            items.iter().any(contains_while_expr)
        }
        Expr::ArrayAccess(_, index) => contains_while_expr(index),
        Expr::BinaryOp(left, _, right) => contains_while_expr(left) || contains_while_expr(right),
        Expr::Block(body) | Expr::Async { body } | Expr::Lambda { body, .. } => {
            contains_while_stmt(body)
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            contains_while_expr(cond)
                || contains_while_stmt(then_branch)
                || contains_while_stmt(else_branch)
        }
        Expr::StructInit { fields, .. } => {
            fields.iter().any(|(_, value)| contains_while_expr(value))
        }
        Expr::FieldAccess(target, _) | Expr::Await { expr: target } => contains_while_expr(target),
        Expr::Match { target, arms } => {
            contains_while_expr(target)
                || arms.iter().any(|arm| {
                    arm.guard.as_deref().is_some_and(contains_while_expr)
                        || contains_while_stmt(&arm.body)
                })
        }
        Expr::CallRef { callee, args } => {
            contains_while_expr(callee) || args.iter().any(contains_while_expr)
        }
        Expr::ChanSend { channel, value } => {
            contains_while_expr(channel) || contains_while_expr(value)
        }
        Expr::ChanRecv { channel } => contains_while_expr(channel),
    }
}

/// `assignable` names the bindings an assignment may rebind: parameters
/// and `let`s bound on the current linear path. `if` branches and
/// expression blocks get an empty set — the source model cannot merge a
/// rebinding that escapes a conditional path, so those stay rejected.
fn statement_kind(
    stmt: &Stmt,
    scope: &BTreeMap<String, ScalarKind>,
    result_in_scope: bool,
    allow_wrapping_arithmetic: bool,
    assignable: &BTreeSet<String>,
    callees: &BTreeMap<String, usize>,
) -> Result<ScalarKind, SemanticContractError> {
    match stmt {
        Stmt::Expr(expr, _) => expression_kind(
            expr,
            scope,
            result_in_scope,
            allow_wrapping_arithmetic,
            assignable,
            callees,
        ),
        Stmt::Assign { var, value, .. } if allow_wrapping_arithmetic => {
            if !assignable.contains(var) {
                return Err(SemanticContractError::Unsupported(
                    "assignment to a name not bound on the linear path",
                ));
            }
            let kind = expression_kind(
                value,
                scope,
                result_in_scope,
                allow_wrapping_arithmetic,
                &BTreeSet::new(),
                callees,
            )?;
            expect_kind(kind, scope[var])?;
            Ok(scope[var])
        }
        Stmt::Block(stmts, _) if stmts.len() == 1 => statement_kind(
            &stmts[0],
            scope,
            result_in_scope,
            allow_wrapping_arithmetic,
            assignable,
            callees,
        ),
        // `let` bindings, scalar reassignments and pure expression
        // statements end in a value statement; the widened equivalence
        // profile admits them, the bounded C03 fragment stays closed.
        // Bindings are lexical to the block and a discarded expression's
        // partial operations still record definedness obligations.
        Stmt::Block(stmts, _) if !stmts.is_empty() && allow_wrapping_arithmetic => {
            let mut inner = scope.clone();
            let mut inner_assignable = assignable.clone();
            let (lets, last) = stmts.split_at(stmts.len() - 1);
            for stmt in lets {
                match stmt {
                    Stmt::Let { var, value, .. } => {
                        let kind = expression_kind(
                            value,
                            &inner,
                            result_in_scope,
                            allow_wrapping_arithmetic,
                            &BTreeSet::new(),
                            callees,
                        )?;
                        inner.insert(var.clone(), kind);
                        inner_assignable.insert(var.clone());
                    }
                    Stmt::Assign { var, value, .. } => {
                        if !inner_assignable.contains(var) {
                            return Err(SemanticContractError::Unsupported(
                                "assignment to a name not bound on the linear path",
                            ));
                        }
                        let kind = expression_kind(
                            value,
                            &inner,
                            result_in_scope,
                            allow_wrapping_arithmetic,
                            &BTreeSet::new(),
                            callees,
                        )?;
                        expect_kind(kind, inner[var])?;
                    }
                    Stmt::Expr(value, _) => {
                        expression_kind(
                            value,
                            &inner,
                            result_in_scope,
                            allow_wrapping_arithmetic,
                            &inner_assignable,
                            callees,
                        )?;
                    }
                    // A `while` is admitted only mid-block under the
                    // widened profile: its unit value can never be the
                    // block tail, its condition must be `bool`, and its
                    // body follows the same linear-path rules. A loop
                    // nested at any depth stays closed — the bounded
                    // unroll bounds iterations per loop instance, which
                    // nesting would break.
                    Stmt::While { cond, body, .. } => {
                        let kind = expression_kind(
                            cond,
                            &inner,
                            result_in_scope,
                            allow_wrapping_arithmetic,
                            &BTreeSet::new(),
                            callees,
                        )?;
                        if kind != ScalarKind::Bool {
                            return Err(SemanticContractError::Unsupported(
                                "non-boolean while condition",
                            ));
                        }
                        if contains_while_stmt(body) {
                            return Err(SemanticContractError::Unsupported(
                                "nested while outside bounded-loop profile",
                            ));
                        }
                        statement_kind(
                            body,
                            &inner,
                            result_in_scope,
                            allow_wrapping_arithmetic,
                            &inner_assignable,
                            callees,
                        )?;
                    }
                    _ => {
                        return Err(SemanticContractError::Unsupported(
                            "statement outside a let/assignment/expression sequence",
                        ));
                    }
                }
            }
            statement_kind(
                &last[0],
                &inner,
                result_in_scope,
                allow_wrapping_arithmetic,
                &inner_assignable,
                callees,
            )
        }
        _ => Err(SemanticContractError::Unsupported(
            "statement, mutation or empty body",
        )),
    }
}

/// `assignable` lists the linear-path names an `if` branch may rebind:
/// assignments to them escape the branch as an `ite` merge on the
/// encoder side. Every other expression position passes an empty set,
/// so writes inside operands, guards and match arms stay closed.
fn expression_kind(
    expr: &Expr,
    scope: &BTreeMap<String, ScalarKind>,
    result_in_scope: bool,
    allow_wrapping_arithmetic: bool,
    assignable: &BTreeSet<String>,
    callees: &BTreeMap<String, usize>,
) -> Result<ScalarKind, SemanticContractError> {
    match expr {
        Expr::Number(_) => Ok(ScalarKind::I64),
        Expr::Variable(name) if matches!(name.as_str(), "true" | "false") => Ok(ScalarKind::Bool),
        // Lexical bindings (including a shadowing `let result`) win over
        // the implicit result name, so the kind checker and the BV encoder
        // see the same binding.
        Expr::Variable(name) => scope
            .get(name.as_str())
            .copied()
            .or((result_in_scope && name == "result").then_some(ScalarKind::I64))
            .ok_or(SemanticContractError::Unsupported(
                "expression or unbound name",
            )),
        Expr::BinaryOp(left, op, right) => {
            let left = expression_kind(
                left,
                scope,
                result_in_scope,
                allow_wrapping_arithmetic,
                &BTreeSet::new(),
                callees,
            )?;
            let right = expression_kind(
                right,
                scope,
                result_in_scope,
                allow_wrapping_arithmetic,
                &BTreeSet::new(),
                callees,
            )?;
            match op {
                Op::Eq | Op::Neq if left == right => Ok(ScalarKind::Bool),
                Op::Lt | Op::Le | Op::Gt | Op::Ge if left == ScalarKind::I64 && right == left => {
                    Ok(ScalarKind::Bool)
                }
                Op::And | Op::Or | Op::Implies if left == ScalarKind::Bool && right == left => {
                    Ok(ScalarKind::Bool)
                }
                // i64 arithmetic wraps mod 2^64 — still no overflow claim.
                // Division, remainder and shifts are admitted as partial
                // operations: the caller's BV encoder records their defined
                // domain (nonzero divisor excluding i64::MIN / -1, shift
                // amount in 0..64) as explicit obligations.
                Op::Add
                | Op::Sub
                | Op::Mul
                | Op::Div
                | Op::Rem
                | Op::Shl
                | Op::Shr
                | Op::BitAnd
                | Op::BitOr
                | Op::BitXor
                    if allow_wrapping_arithmetic && left == ScalarKind::I64 && right == left =>
                {
                    Ok(ScalarKind::I64)
                }
                _ => Err(SemanticContractError::Unsupported(
                    "operator or operand kinds",
                )),
            }
        }
        Expr::Block(stmt) => statement_kind(
            stmt,
            scope,
            result_in_scope,
            allow_wrapping_arithmetic,
            &BTreeSet::new(),
            callees,
        ),
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            expect_kind(
                expression_kind(
                    cond,
                    scope,
                    result_in_scope,
                    allow_wrapping_arithmetic,
                    &BTreeSet::new(),
                    callees,
                )?,
                ScalarKind::Bool,
            )?;
            // Branch bodies may rebind names bound on the current
            // linear path; the encoder merges them under the branch
            // condition like the emitter's per-variable phi.
            let then_kind = statement_kind(
                then_branch,
                scope,
                result_in_scope,
                allow_wrapping_arithmetic,
                assignable,
                callees,
            )?;
            expect_kind(
                statement_kind(
                    else_branch,
                    scope,
                    result_in_scope,
                    allow_wrapping_arithmetic,
                    assignable,
                    callees,
                )?,
                then_kind,
            )?;
            Ok(then_kind)
        }
        // First-match-order `match` on an i64 scrutinee: literal patterns
        // test `target == n` and a `Variable`/`Wildcard` arm binds or
        // ignores the value; a trailing catch-all arm is required so the
        // chain is exhaustive without a solver check. An arm may add a
        // Boolean guard evaluated in the arm's scope, but the trailing
        // catch-all must be unguarded. Variant patterns, a non-i64
        // scrutinee and a literal last arm stay outside the profile.
        // Arm bodies may rebind linear-path names like `if` branches —
        // each write merges under the arm's own hit-condition.
        Expr::Match { target, arms } if allow_wrapping_arithmetic => {
            expect_kind(
                expression_kind(
                    target,
                    scope,
                    result_in_scope,
                    allow_wrapping_arithmetic,
                    &BTreeSet::new(),
                    callees,
                )?,
                ScalarKind::I64,
            )?;
            let Some(last) = arms.last() else {
                return Err(SemanticContractError::Unsupported("empty match"));
            };
            if !matches!(last.pattern, Pattern::Wildcard | Pattern::Variable(_)) {
                return Err(SemanticContractError::Unsupported(
                    "match without a catch-all arm",
                ));
            }
            if last.guard.is_some() {
                return Err(SemanticContractError::Unsupported(
                    "guarded match catch-all arm",
                ));
            }
            let mut kind = None;
            for arm in arms {
                let mut arm_scope = scope.clone();
                match &arm.pattern {
                    Pattern::Literal(_) | Pattern::Wildcard => {}
                    Pattern::Variable(name) => {
                        arm_scope.insert(name.clone(), ScalarKind::I64);
                    }
                    _ => {
                        return Err(SemanticContractError::Unsupported(
                            "match pattern outside literals and catch-all",
                        ));
                    }
                }
                if let Some(guard) = &arm.guard {
                    expect_kind(
                        expression_kind(
                            guard,
                            &arm_scope,
                            result_in_scope,
                            allow_wrapping_arithmetic,
                            &BTreeSet::new(),
                            callees,
                        )?,
                        ScalarKind::Bool,
                    )?;
                }
                // Arm bodies may rebind linear-path names like `if`
                // branches; a `Variable` pattern shadows an outer name
                // for this arm, so it leaves the assignable set.
                let mut arm_assignable = assignable.clone();
                if let Pattern::Variable(name) = &arm.pattern {
                    arm_assignable.remove(name);
                }
                let arm_kind = statement_kind(
                    &arm.body,
                    &arm_scope,
                    result_in_scope,
                    allow_wrapping_arithmetic,
                    &arm_assignable,
                    callees,
                )?;
                match kind {
                    None => kind = Some(arm_kind),
                    Some(first) => expect_kind(arm_kind, first)?,
                }
            }
            kind.ok_or(SemanticContractError::Unsupported("empty match"))
        }
        Expr::Call(name, args) if allow_wrapping_arithmetic => {
            // A call must name an atom of this module with a matching
            // arity: an undeclared or mismatched callee has no
            // counterpart to compare a call site against.
            match callees.get(name.as_str()) {
                Some(arity) if *arity == args.len() => {}
                _ => {
                    return Err(SemanticContractError::Unsupported(
                        "call target outside module",
                    ));
                }
            }
            // A call is i64-valued with i64 arguments: each argument
            // must kind-check as I64.
            for arg in args {
                expect_kind(
                    expression_kind(
                        arg,
                        scope,
                        result_in_scope,
                        allow_wrapping_arithmetic,
                        &BTreeSet::new(),
                        callees,
                    )?,
                    ScalarKind::I64,
                )?;
            }
            Ok(ScalarKind::I64)
        }
        _ => Err(SemanticContractError::Unsupported(
            "expression or unbound name",
        )),
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn json_digest(value: &impl Serialize) -> Result<String, SemanticContractError> {
    let value = serde_json::to_value(value).map_err(|_| SemanticContractError::Encoding)?;
    let bytes = serde_json::to_vec(&value).map_err(|_| SemanticContractError::Encoding)?;
    Ok(digest(&bytes))
}

/// Symbolic body for the C03 comparison fragment, independent of an emitter.
/// No solver check, pre/postcondition proof or LLVM claim is performed here.
/// One call site inside an encoded body: the callee name, the encoded
/// argument values, the uninterpreted application producing the result,
/// and the path guard under which the call executes — the same guard
/// the definedness obligations use, so a call under `if`/`match`/loop
/// control flow is bound only where it actually runs.
#[derive(Clone)]
pub struct CallSite<'ctx> {
    pub callee: String,
    pub args: Vec<BV<'ctx>>,
    pub result: BV<'ctx>,
    pub guard: Bool<'ctx>,
}

pub struct PureI64Body<'ctx> {
    pub function_name: String,
    pub source_sha256: String,
    pub inputs: Vec<BV<'ctx>>,
    pub value: BV<'ctx>,
    /// Every `Expr::Call` the body encoding produced, in encounter
    /// order. The translation-validation path ignores them (a call
    /// compares as an uninterpreted site on both sides); the source
    /// recheck binds each site to the callee's contract.
    pub call_sites: Vec<CallSite<'ctx>>,
    /// Definedness obligations for division, remainder and shifts, each an
    /// implication `guard -> operands defined` mirroring the safety rules
    /// `--bitvec-i64` verification requires. Empty unless the caller admitted
    /// those operators; their conjunction is the domain the source binds.
    pub definedness: Vec<Bool<'ctx>>,
    /// Domain the declared signature itself binds: an admitted `bool`
    /// parameter lowers to i64 carrying 0/1, so its entry is `input <=u 1`;
    /// an admitted subrange integer type (`i8`/`i16`/`i32`/`u8`/`u16`/`u32`)
    /// lowers to an i64 formal too, so its entries bound the input to the
    /// type's range. Empty without such parameters.
    pub param_domain: Vec<Bool<'ctx>>,
    /// True when a declared subrange integer parameter contributed to
    /// `param_domain` — a bool-only domain keeps its 0/1 report label,
    /// while any subrange bound reports the signature-bounded domain.
    pub subrange_params: bool,
    /// When the body contains a `while`, the conjunction of each loop's
    /// bounded-exit condition: `true` only on inputs where every loop's
    /// guard fails within `LOOP_BOUND` unrolled iterations. Equivalence
    /// is claimed only inside that domain — a path still iterating at
    /// the bound is outside the profile, matching the LLVM side's
    /// bounded path drop. `None` for bodies without a `while`.
    pub loop_bound_exits: Option<Bool<'ctx>>,
}

type BodyResult<T> = std::result::Result<T, SemanticContractError>;

/// `allow_wrapping_arithmetic` admits wrapping i64 add/sub/mul in the encoded
/// body; callers checking the bounded C03 fragment (recheck, evidence) pass
/// `false` so they keep refusing sources the build path would also refuse.
pub fn encode_pure_i64_body<'ctx>(
    source: &str,
    context: &'ctx Context,
    allow_wrapping_arithmetic: bool,
) -> BodyResult<PureI64Body<'ctx>> {
    let (source_sha256, mut items) = bounded_pure_i64_module(source, allow_wrapping_arithmetic)?;
    let Some(Item::Atom(atom)) = items.pop() else {
        return Err(SemanticContractError::Unsupported("source atom"));
    };
    encode_atom_body(&atom, context, source_sha256)
}

/// Encode every atom of the module under the same profile, in declaration
/// order. Each entry is an independent per-atom obligation: a call is an
/// uninterpreted function site, so an atom compares only its own body
/// against the same-named IR function — never the callee's body.
pub fn encode_pure_i64_module<'ctx>(
    source: &str,
    context: &'ctx Context,
    allow_wrapping_arithmetic: bool,
) -> BodyResult<Vec<PureI64Body<'ctx>>> {
    let (source_sha256, items) = bounded_pure_i64_module(source, allow_wrapping_arithmetic)?;
    items
        .iter()
        .map(|item| match item {
            Item::Atom(atom) => encode_atom_body(atom, context, source_sha256.clone()),
            _ => Err(SemanticContractError::Unsupported("non-atom declarations")),
        })
        .collect()
}

/// Shared admission for the per-atom encoders: input bounds, contract
/// extraction (which itself admits only same-module atoms of the profile)
/// and the module item list. Returns the module digest and the items.
fn bounded_pure_i64_module(
    source: &str,
    allow_wrapping_arithmetic: bool,
) -> BodyResult<(String, Vec<Item>)> {
    if source.len() > 65_536 {
        return Err(SemanticContractError::Unsupported("source size limit"));
    }
    let tokens = crate::parser::lexer::Lexer::new(source)
        .tokenize_checked()
        .map_err(|_| SemanticContractError::Parse)?;
    if tokens.len() > 256 {
        return Err(SemanticContractError::Unsupported("source token limit"));
    }
    let mut depth = 0usize;
    for token in tokens {
        use crate::parser::token::Token;
        match token.token {
            Token::LParen | Token::LBrace | Token::LBracket => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err(SemanticContractError::Unsupported("source nesting limit"));
                }
            }
            Token::RParen | Token::RBrace | Token::RBracket => depth = depth.saturating_sub(1),
            _ => (),
        }
    }
    let contract =
        extract_pure_i64_contract(source, "scalar-body.mm", None, allow_wrapping_arithmetic)?;
    let items = parse_module_checked(source).map_err(|_| SemanticContractError::Parse)?;
    Ok((contract.source_sha256, items))
}

fn encode_atom_body<'ctx>(
    atom: &crate::parser::ast::Atom,
    context: &'ctx Context,
    source_sha256: String,
) -> BodyResult<PureI64Body<'ctx>> {
    if atom.params.len() > 16 {
        return Err(SemanticContractError::Unsupported("parameter limit"));
    }
    let body =
        parse_body_expr_checked(&atom.body_expr).map_err(|_| SemanticContractError::Parse)?;
    let inputs: Vec<_> = (0..atom.params.len())
        .map(|i| BV::new_const(context, format!("input_{i}"), 64))
        .collect();
    let env: HashMap<String, SourceValue> = param_env(&atom.params, &inputs, context);
    let top = Bool::from_bool(context, true);
    let mut definedness = Vec::new();
    let assignable: BTreeSet<String> = atom.params.iter().map(|p| p.name.clone()).collect();
    // A `-> bool` body lowers to i64 carrying 0/1 — the emitter zexts
    // its i1 result, so the checked value is the same 0/1 materialization.
    // Top-level reassignments land in `top_writes`; nothing reads the
    // body afterwards, so the map is a sink for the write records.
    let mut top_writes = HashMap::new();
    let mut loop_exits = Vec::new();
    let mut call_sites = Vec::new();
    let value = match source_stmt(
        &body,
        context,
        &env,
        &top,
        &mut definedness,
        0,
        &assignable,
        &mut top_writes,
        &mut loop_exits,
        &mut call_sites,
    )? {
        SourceValue::Integer(value) => value,
        SourceValue::Boolean(value) => {
            value.ite(&BV::from_u64(context, 1, 64), &BV::from_u64(context, 0, 64))
        }
    };
    // A declared `bool` parameter lowers to i64 carrying 0/1, so the
    // declared signature restricts its input to {0, 1}: without this the
    // i64 range would make `b == true` look different at `input = 5`.
    // The same applies to the narrower integer types the emitter lowers
    // to an i64 formal — `x: i32` checks only inputs in `[-2^31, 2^31)`,
    // so `x == 3_000_000_000` stays equivalent to `false`.
    let param_domain: Vec<Bool> = atom
        .params
        .iter()
        .zip(&inputs)
        .flat_map(|(p, v)| match p.type_name.as_deref() {
            Some("bool") => vec![v.bvule(&BV::from_u64(context, 1, 64))],
            Some("u8") => vec![v.bvule(&BV::from_u64(context, u8::MAX as u64, 64))],
            Some("u16") => vec![v.bvule(&BV::from_u64(context, u16::MAX as u64, 64))],
            Some("u32") => vec![v.bvule(&BV::from_u64(context, u32::MAX as u64, 64))],
            Some("i8") => vec![
                v.bvsge(&BV::from_i64(context, i8::MIN as i64, 64)),
                v.bvsle(&BV::from_i64(context, i8::MAX as i64, 64)),
            ],
            Some("i16") => vec![
                v.bvsge(&BV::from_i64(context, i16::MIN as i64, 64)),
                v.bvsle(&BV::from_i64(context, i16::MAX as i64, 64)),
            ],
            Some("i32") => vec![
                v.bvsge(&BV::from_i64(context, i32::MIN as i64, 64)),
                v.bvsle(&BV::from_i64(context, i32::MAX as i64, 64)),
            ],
            _ => vec![],
        })
        .collect();
    let subrange_params = atom.params.iter().any(|p| {
        matches!(
            p.type_name.as_deref(),
            Some("i8" | "i16" | "i32" | "u8" | "u16" | "u32")
        )
    });
    let loop_bound_exits = if loop_exits.is_empty() {
        None
    } else {
        let refs: Vec<&Bool> = loop_exits.iter().collect();
        Some(Bool::and(context, &refs))
    };
    Ok(PureI64Body {
        function_name: atom.name.clone(),
        source_sha256,
        inputs,
        value,
        call_sites,
        definedness,
        param_domain,
        subrange_params,
        loop_bound_exits,
    })
}

const MAX_DEPTH: usize = 48;

/// Iterations a `while` is unrolled to on the source side, and the
/// per-block visit depth the LLVM side explores before dropping a path.
/// Both sides share one constant so the declared profile is symmetric:
/// a source loop whose guard fails within the bound meets an LLVM path
/// that leaves the cycle within the same bound.
pub const LOOP_BOUND: usize = 4;

/// Encode a counterexample to the C03 source contract, not LLVM correspondence.
/// Callers must separately guard against vacuous preconditions when required.
pub fn encode_pure_i64_obligation<'ctx>(
    source: &str,
    context: &'ctx Context,
) -> BodyResult<Bool<'ctx>> {
    let body = encode_pure_i64_body(source, context, false)?;
    let mut items = parse_module_checked(source).map_err(|_| SemanticContractError::Parse)?;
    let Some(Item::Atom(atom)) = items.pop() else {
        return Err(SemanticContractError::Unsupported("source atom"));
    };
    atom_obligation(&atom, &body, context)
}

/// Counterexample obligations for a multi-atom module under the widened
/// profile (`while` keeps its bounded-exit domain the same way as the
/// single-atom encoder). Each atom contributes one postcondition
/// obligation plus one domain obligation per call site — every entry is
/// an independent `unsat` requirement, so a vacuous conjunct can never
/// mask a real counterexample the way folding them into one formula
/// could.
///
/// A call to a sibling is an uninterpreted site whose result is assumed
/// to satisfy the callee's `ensures` — sound only because the sibling's
/// `requires` at the call's guard is its own obligation, and the
/// sibling's own postcondition appears as an entry of its own.
pub fn encode_pure_i64_module_obligations<'ctx>(
    source: &str,
    context: &'ctx Context,
) -> BodyResult<Vec<Bool<'ctx>>> {
    let (_, items) = bounded_pure_i64_module(source, true)?;
    let bodies = encode_pure_i64_module(source, context, true)?;
    let callees: BTreeMap<String, &crate::parser::ast::Atom> = items
        .iter()
        .filter_map(|item| match item {
            Item::Atom(atom) => Some((atom.name.clone(), atom)),
            _ => None,
        })
        .collect();
    let mut obligations = Vec::new();
    for (item, body) in items.iter().zip(bodies.iter()) {
        match item {
            Item::Atom(atom) => {
                obligations.extend(module_atom_obligations(atom, body, &callees, context)?)
            }
            _ => return Err(SemanticContractError::Unsupported("non-atom declarations")),
        }
    }
    Ok(obligations)
}

/// The module-atom obligation family — every entry must be `unsat`:
///
/// - entry 0: `requires ∧ call_assumptions ∧ ¬ensures ∧ exits` — the
///   caller's own postcondition, assuming each callee honoured its
///   `ensures`;
/// - entries 1..: `requires ∧ site_guard ∧ ¬callee_requires` — the
///   caller establishes the callee's precondition on every path that
///   reaches the call, so the ensures assumption above is justified.
///
/// The domain obligations deliberately exclude the other sites' ensures
/// assumptions, so no circular justification is possible: a call whose
/// own domain needs a sibling's earlier ensures falls outside this
/// fragment instead of being bootstrapped.
/// Contract expressions that themselves contain calls keep those calls
/// uninterpreted — a callee's own contract is its own obligation.
fn module_atom_obligations<'ctx>(
    atom: &Atom,
    body: &PureI64Body<'ctx>,
    callees: &BTreeMap<String, &crate::parser::ast::Atom>,
    context: &'ctx Context,
) -> BodyResult<Vec<Bool<'ctx>>> {
    let mut env: HashMap<String, SourceValue> = param_env(&atom.params, &body.inputs, context);
    let requires =
        parse_expression_checked(&atom.requires).map_err(|_| SemanticContractError::Parse)?;
    let top = Bool::from_bool(context, true);
    let mut sink = Vec::new();
    let mut requires_exits = Vec::new();
    let requires = source_expr(
        &requires,
        context,
        &env,
        &top,
        &mut sink,
        0,
        &BTreeSet::new(),
        &mut HashMap::new(),
        &mut requires_exits,
        &mut Vec::new(),
    )?
    .boolean()?;
    env.insert(
        "result".to_string(),
        SourceValue::Integer(body.value.clone()),
    );
    let ensures =
        parse_expression_checked(&atom.ensures).map_err(|_| SemanticContractError::Parse)?;
    let mut ensures_exits = Vec::new();
    let ensures = source_expr(
        &ensures,
        context,
        &env,
        &top,
        &mut sink,
        0,
        &BTreeSet::new(),
        &mut HashMap::new(),
        &mut ensures_exits,
        &mut Vec::new(),
    )?
    .boolean()?;
    let mut assumptions = Vec::new();
    let mut domain_obligations = Vec::new();
    for site in &body.call_sites {
        let Some(callee) = callees.get(&site.callee) else {
            return Err(SemanticContractError::Unsupported(
                "call target outside module",
            ));
        };
        let site_env = param_env(&callee.params, &site.args, context);
        let callee_requires =
            parse_expression_checked(&callee.requires).map_err(|_| SemanticContractError::Parse)?;
        let mut site_exits = Vec::new();
        let callee_requires = source_expr(
            &callee_requires,
            context,
            &site_env,
            &top,
            &mut sink,
            0,
            &BTreeSet::new(),
            &mut HashMap::new(),
            &mut site_exits,
            &mut Vec::new(),
        )?
        .boolean()?;
        // A domain counterexample counts only where the callee's own
        // bounded-while encodings actually exited — same domain
        // convention the single-atom obligation applies to `requires`.
        let mut domain_terms = vec![requires.clone(), site.guard.clone(), callee_requires.not()];
        domain_terms.extend(site_exits);
        domain_obligations.push(Bool::and(context, &domain_terms.iter().collect::<Vec<_>>()));
        let mut ensures_env = site_env;
        ensures_env.insert(
            "result".to_string(),
            match return_kind(callee) {
                ScalarKind::Bool => {
                    SourceValue::Boolean(site.result._eq(&BV::from_u64(context, 0, 64)).not())
                }
                ScalarKind::I64 => SourceValue::Integer(site.result.clone()),
            },
        );
        let callee_ensures =
            parse_expression_checked(&callee.ensures).map_err(|_| SemanticContractError::Parse)?;
        let mut ensures_site_exits = Vec::new();
        let callee_ensures = source_expr(
            &callee_ensures,
            context,
            &ensures_env,
            &top,
            &mut sink,
            0,
            &BTreeSet::new(),
            &mut HashMap::new(),
            &mut ensures_site_exits,
            &mut Vec::new(),
        )?
        .boolean()?;
        // The assumption binds only where the callee's ensures encoding
        // stayed inside its bounded-while domain: outside it nothing is
        // claimed, matching the fragment's exit-domain convention.
        let exits_domain = Bool::and(context, &ensures_site_exits.iter().collect::<Vec<_>>());
        assumptions.push(exits_domain.implies(&callee_ensures));
    }
    let assumption = Bool::and(context, &assumptions.iter().collect::<Vec<_>>());
    let mut terms = vec![requires, Bool::and(context, &[&assumption, &ensures.not()])];
    for exits in requires_exits.iter().chain(ensures_exits.iter()) {
        terms.push(exits.clone());
    }
    let mut obligations = vec![Bool::and(context, &terms.iter().collect::<Vec<_>>())];
    obligations.extend(domain_obligations);
    Ok(obligations)
}

/// The per-atom `requires ∧ ¬ensures` obligation — shared by the
/// single-atom and module encoders so both profiles keep identical
/// contract semantics.
fn atom_obligation<'ctx>(
    atom: &Atom,
    body: &PureI64Body<'ctx>,
    context: &'ctx Context,
) -> BodyResult<Bool<'ctx>> {
    let mut env: HashMap<String, SourceValue> = param_env(&atom.params, &body.inputs, context);
    let requires =
        parse_expression_checked(&atom.requires).map_err(|_| SemanticContractError::Parse)?;
    let top = Bool::from_bool(context, true);
    let mut sink = Vec::new();
    // A `while` inside a requires/ensures block keeps its bound: an
    // input whose contract loop never exits within `LOOP_BOUND` falls
    // outside the checked domain instead of comparing a stale value.
    let mut requires_exits = Vec::new();
    let requires = source_expr(
        &requires,
        context,
        &env,
        &top,
        &mut sink,
        0,
        &BTreeSet::new(),
        &mut HashMap::new(),
        &mut requires_exits,
        &mut Vec::new(),
    )?
    .boolean()?;
    env.insert(
        "result".to_string(),
        SourceValue::Integer(body.value.clone()),
    );
    let ensures =
        parse_expression_checked(&atom.ensures).map_err(|_| SemanticContractError::Parse)?;
    let mut ensures_exits = Vec::new();
    let ensures = source_expr(
        &ensures,
        context,
        &env,
        &top,
        &mut sink,
        0,
        &BTreeSet::new(),
        &mut HashMap::new(),
        &mut ensures_exits,
        &mut Vec::new(),
    )?
    .boolean()?;
    let mut terms = vec![requires, ensures.not()];
    for exits in requires_exits.iter().chain(ensures_exits.iter()) {
        terms.push(exits.clone());
    }
    Ok(Bool::and(context, &terms.iter().collect::<Vec<_>>()))
}

#[derive(Clone)]
enum SourceValue<'ctx> {
    Integer(BV<'ctx>),
    Boolean(Bool<'ctx>),
}

/// Bind declared parameters to their symbolic inputs: `i64` parameters
/// are 64-bit integers, `bool` parameters the truth value `input != 0`
/// (the emitter lowers `bool` to an i64 carrying 0/1, matching its
/// `icmp ne` condition).
fn param_env<'ctx>(
    params: &[crate::parser::ast::Param],
    inputs: &[BV<'ctx>],
    ctx: &'ctx Context,
) -> HashMap<String, SourceValue<'ctx>> {
    params
        .iter()
        .zip(inputs)
        .map(|(p, v)| {
            let value = if param_kind(p) == ScalarKind::Bool {
                SourceValue::Boolean(v._eq(&BV::from_u64(ctx, 0, 64)).not())
            } else {
                SourceValue::Integer(v.clone())
            };
            (p.name.clone(), value)
        })
        .collect()
}
impl<'ctx> SourceValue<'ctx> {
    fn integer(self) -> BodyResult<BV<'ctx>> {
        match self {
            Self::Integer(v) => Ok(v),
            _ => Err(SemanticContractError::Unsupported(
                "source integer expected",
            )),
        }
    }
    fn boolean(self) -> BodyResult<Bool<'ctx>> {
        match self {
            Self::Boolean(v) => Ok(v),
            _ => Err(SemanticContractError::Unsupported(
                "source boolean expected",
            )),
        }
    }
}

/// `assignable` mirrors `statement_kind`: only names bound on the
/// current linear path may be rebound. `written` collects the final
/// symbolic value of every assignable name a statement rewrites, so an
/// enclosing `if` can merge branch writes under its condition and an
/// enclosing block can re-export the writes that escape it.
#[allow(clippy::too_many_arguments)]
fn source_stmt<'ctx>(
    stmt: &Stmt,
    ctx: &'ctx Context,
    env: &HashMap<String, SourceValue<'ctx>>,
    guard: &Bool<'ctx>,
    definedness: &mut Vec<Bool<'ctx>>,
    depth: usize,
    assignable: &BTreeSet<String>,
    written: &mut HashMap<String, SourceValue<'ctx>>,
    // `while` exits observed under bounded unrolling: one entry per
    // loop, `true` on every input where that loop's guard fails within
    // `LOOP_BOUND` iterations. The report bounds its equivalence claim
    // to inputs satisfying all of them.
    loop_exits: &mut Vec<Bool<'ctx>>,
    call_sites: &mut Vec<CallSite<'ctx>>,
) -> BodyResult<SourceValue<'ctx>> {
    if depth > MAX_DEPTH {
        return Err(SemanticContractError::Unsupported("source depth limit"));
    }
    match stmt {
        Stmt::Expr(e, _) => source_expr(
            e,
            ctx,
            env,
            guard,
            definedness,
            depth + 1,
            assignable,
            written,
            loop_exits,
            call_sites,
        ),
        Stmt::Assign { var, value, .. } => {
            if !assignable.contains(var) {
                return Err(SemanticContractError::Unsupported(
                    "assignment to a name not bound on the linear path",
                ));
            }
            let bound = source_expr(
                value,
                ctx,
                env,
                guard,
                definedness,
                depth + 1,
                &BTreeSet::new(),
                written,
                loop_exits,
                call_sites,
            )?;
            written.insert(var.clone(), bound.clone());
            Ok(bound)
        }
        Stmt::Block(stmts, _) if stmts.len() == 1 => source_stmt(
            &stmts[0],
            ctx,
            env,
            guard,
            definedness,
            depth + 1,
            assignable,
            written,
            loop_exits,
            call_sites,
        ),
        Stmt::Block(stmts, _) if !stmts.is_empty() => {
            // Sequential blocks bind names lexically: each `let`
            // initializer and each reassigning statement encodes under
            // the current guard (its partial operations still record
            // definedness obligations) and a bare expression statement
            // encodes and discards its value — its own definedness
            // obligations still apply — then the last statement yields
            // the block value under the wider scope.
            let mut inner = env.clone();
            let mut inner_assignable = assignable.clone();
            // Names an inner `let` shadowed lose their escaping status
            // for the rest of this block — writes to them stay local.
            let mut escapable = assignable.clone();
            let (lets, last) = stmts.split_at(stmts.len() - 1);
            for stmt in lets {
                match stmt {
                    Stmt::Let { var, value, .. } => {
                        let bound = source_expr(
                            value,
                            ctx,
                            &inner,
                            guard,
                            definedness,
                            depth + 1,
                            &BTreeSet::new(),
                            written,
                            loop_exits,
                            call_sites,
                        )?;
                        inner.insert(var.clone(), bound);
                        inner_assignable.insert(var.clone());
                        escapable.remove(var);
                    }
                    Stmt::Assign { var, value, .. } => {
                        if !inner_assignable.contains(var) {
                            return Err(SemanticContractError::Unsupported(
                                "assignment to a name not bound on the linear path",
                            ));
                        }
                        let bound = source_expr(
                            value,
                            ctx,
                            &inner,
                            guard,
                            definedness,
                            depth + 1,
                            &BTreeSet::new(),
                            written,
                            loop_exits,
                            call_sites,
                        )?;
                        inner.insert(var.clone(), bound.clone());
                        if escapable.contains(var) {
                            written.insert(var.clone(), bound);
                        }
                    }
                    Stmt::Expr(value, _) => {
                        let mut local = HashMap::new();
                        source_expr(
                            value,
                            ctx,
                            &inner,
                            guard,
                            definedness,
                            depth + 1,
                            &inner_assignable,
                            &mut local,
                            loop_exits,
                            call_sites,
                        )?;
                        for (name, bound) in local {
                            inner.insert(name.clone(), bound.clone());
                            if escapable.contains(&name) {
                                written.insert(name, bound);
                            }
                        }
                    }
                    Stmt::While { cond, body, .. } => {
                        let mut local = HashMap::new();
                        source_while(
                            cond,
                            body,
                            ctx,
                            &inner,
                            guard,
                            definedness,
                            depth + 1,
                            &inner_assignable,
                            &mut local,
                            loop_exits,
                            call_sites,
                        )?;
                        for (name, bound) in local {
                            inner.insert(name.clone(), bound.clone());
                            if escapable.contains(&name) {
                                written.insert(name, bound);
                            }
                        }
                    }
                    _ => {
                        return Err(SemanticContractError::Unsupported(
                            "statement outside a let/assignment/expression sequence",
                        ));
                    }
                }
            }
            let mut tail_writes = HashMap::new();
            let value = source_stmt(
                &last[0],
                ctx,
                &inner,
                guard,
                definedness,
                depth + 1,
                &inner_assignable,
                &mut tail_writes,
                loop_exits,
                call_sites,
            )?;
            for (name, bound) in tail_writes {
                if escapable.contains(&name) {
                    written.insert(name, bound);
                }
            }
            Ok(value)
        }
        _ => Err(SemanticContractError::Unsupported(
            "unsupported source statement",
        )),
    }
}

/// Bounded unroll of an admitted `while`: each of `LOOP_BOUND` steps
/// re-evaluates the condition under the merged environment, executes the
/// body only on paths still looping (`guard ∧ ¬exited ∧ cond` carries
/// the body's definedness obligations), and merges each escaping write
/// under that same body guard. A path still iterating at the
/// bound is outside the profile: the collected `loop_exits` disjunction
/// bounds the reported equivalence domain to inputs where the loop
/// exits, matching the LLVM side's bounded path drop.
#[allow(clippy::too_many_arguments)]
fn source_while<'ctx>(
    cond: &Expr,
    body: &Stmt,
    ctx: &'ctx Context,
    env: &HashMap<String, SourceValue<'ctx>>,
    guard: &Bool<'ctx>,
    definedness: &mut Vec<Bool<'ctx>>,
    depth: usize,
    assignable: &BTreeSet<String>,
    written: &mut HashMap<String, SourceValue<'ctx>>,
    loop_exits: &mut Vec<Bool<'ctx>>,
    call_sites: &mut Vec<CallSite<'ctx>>,
) -> BodyResult<()> {
    use SourceValue::{Boolean as B, Integer as I};
    let mut cur = env.clone();
    let mut exited = Bool::from_bool(ctx, false);
    let mut merged: BTreeMap<String, SourceValue> = BTreeMap::new();
    for _ in 0..LOOP_BOUND {
        // The guard evaluates on every path that has not exited yet —
        // its partial operations obligate only that prefix.
        let alive = Bool::and(ctx, &[guard, &exited.not()]);
        let c = match source_expr(
            cond,
            ctx,
            &cur,
            &alive,
            definedness,
            depth + 1,
            &BTreeSet::new(),
            &mut HashMap::new(),
            loop_exits,
            call_sites,
        )? {
            B(b) => b,
            _ => {
                return Err(SemanticContractError::Unsupported(
                    "non-boolean while condition",
                ))
            }
        };
        let body_guard = Bool::and(ctx, &[&alive, &c]);
        let mut body_writes = HashMap::new();
        source_stmt(
            body,
            ctx,
            &cur,
            &body_guard,
            definedness,
            depth + 1,
            assignable,
            &mut body_writes,
            loop_exits,
            call_sites,
        )?;
        for (name, bound) in body_writes {
            // Writes escape only where the body actually ran: an
            // already-exited path keeps its entry values however the
            // stale condition evaluates.
            let value = match (cur.get(&name), &bound) {
                (Some(I(old)), I(new)) => I(body_guard.ite(new, old)),
                (Some(B(old)), B(new)) => B(body_guard.ite(new, old)),
                (None, _) => bound.clone(),
                _ => {
                    return Err(SemanticContractError::Unsupported(
                        "while rebinding kind mismatch",
                    ))
                }
            };
            merged.insert(name.clone(), value.clone());
            cur.insert(name, value);
        }
        exited = Bool::or(ctx, &[&exited, &c.not()]);
    }
    // The bound only applies where the loop is reached: a `while` inside
    // an untaken branch owes no exit on the paths that skip it.
    loop_exits.push(guard.implies(&exited));
    for (name, bound) in merged {
        written.insert(name, bound);
    }
    Ok(())
}

/// The domain LLVM and `--bitvec-i64` verification both give `sdiv`/`srem`:
/// nonzero divisor and not the overflowing `i64::MIN / -1` pair.
fn div_defined<'ctx>(a: &BV<'ctx>, b: &BV<'ctx>, ctx: &'ctx Context) -> Bool<'ctx> {
    let zero = BV::from_i64(ctx, 0, 64);
    let min = BV::from_i64(ctx, i64::MIN, 64);
    let minus_one = BV::from_i64(ctx, -1, 64);
    Bool::and(
        ctx,
        &[
            &b._eq(&zero).not(),
            &Bool::and(ctx, &[&a._eq(&min), &b._eq(&minus_one)]).not(),
        ],
    )
}

/// The domain `shl`/`ashr` share: a shift amount in 0..64 (an unsigned
/// comparison, so negative amounts are excluded too).
fn shift_defined<'ctx>(amount: &BV<'ctx>, ctx: &'ctx Context) -> Bool<'ctx> {
    amount.bvult(&BV::from_i64(ctx, 64, 64))
}

/// Uninterpreted function modelling a source-level call site: the LLVM
/// encoder maps `call @name(...)` to the declaration of the same name
/// and arity, so a call result compares only against the same callee's
/// result on equal arguments. Whether the callee's LLVM body itself
/// implements the source callee is a separate per-function obligation,
/// not part of this call-site comparison.
pub fn call_decl<'ctx>(ctx: &'ctx Context, name: &str, arity: usize) -> FuncDecl<'ctx> {
    let domain: Vec<Sort> = (0..arity).map(|_| Sort::bitvector(ctx, 64)).collect();
    let refs: Vec<&Sort> = domain.iter().collect();
    FuncDecl::new(
        ctx,
        format!("mumei_call_{name}_{arity}"),
        &refs,
        &Sort::bitvector(ctx, 64),
    )
}

#[allow(clippy::too_many_arguments)]
fn source_expr<'ctx>(
    expr: &Expr,
    ctx: &'ctx Context,
    env: &HashMap<String, SourceValue<'ctx>>,
    guard: &Bool<'ctx>,
    definedness: &mut Vec<Bool<'ctx>>,
    depth: usize,
    assignable: &BTreeSet<String>,
    written: &mut HashMap<String, SourceValue<'ctx>>,
    loop_exits: &mut Vec<Bool<'ctx>>,
    call_sites: &mut Vec<CallSite<'ctx>>,
) -> BodyResult<SourceValue<'ctx>> {
    use SourceValue::{Boolean as B, Integer as I};
    if depth > MAX_DEPTH {
        return Err(SemanticContractError::Unsupported("source depth limit"));
    }
    Ok(match expr {
        Expr::Number(n) => I(BV::from_i64(ctx, *n, 64)),
        Expr::Variable(n) if n == "true" || n == "false" => B(Bool::from_bool(ctx, n == "true")),
        Expr::Variable(n) => env
            .get(n.as_str())
            .cloned()
            .ok_or(SemanticContractError::Unsupported("source binding"))?,
        Expr::Block(s) => source_stmt(
            s,
            ctx,
            env,
            guard,
            definedness,
            depth + 1,
            &BTreeSet::new(),
            written,
            loop_exits,
            call_sites,
        )?,
        Expr::BinaryOp(a, op, b) => {
            let a = source_expr(
                a,
                ctx,
                env,
                guard,
                definedness,
                depth + 1,
                &BTreeSet::new(),
                written,
                loop_exits,
                call_sites,
            )?;
            // `&&`/`||`/`=>` short-circuit like the emitter and
            // `--bitvec-i64` verification: the right operand's partial
            // operations bind only when the left one lets it run.
            let b_guard = match (&a, op) {
                (B(ab), Op::And) | (B(ab), Op::Implies) => Bool::and(ctx, &[guard, ab]),
                (B(ab), Op::Or) => Bool::and(ctx, &[guard, &ab.not()]),
                _ => guard.clone(),
            };
            let b = source_expr(
                b,
                ctx,
                env,
                &b_guard,
                definedness,
                depth + 1,
                &BTreeSet::new(),
                written,
                loop_exits,
                call_sites,
            )?;
            match (a, op, b) {
                (I(a), Op::Eq, I(b)) => B(a._eq(&b)),
                (I(a), Op::Neq, I(b)) => B(a._eq(&b).not()),
                (B(a), Op::Eq, B(b)) => B(a._eq(&b)),
                (B(a), Op::Neq, B(b)) => B(a._eq(&b).not()),
                (I(a), Op::Lt, I(b)) => B(a.bvslt(&b)),
                (I(a), Op::Le, I(b)) => B(a.bvsle(&b)),
                (I(a), Op::Gt, I(b)) => B(a.bvsgt(&b)),
                (I(a), Op::Ge, I(b)) => B(a.bvsge(&b)),
                // i64 arithmetic wraps mod 2^64, matching the emitted
                // `add`/`sub`/`mul` (the emitter never sets nsw/nuw).
                (I(a), Op::Add, I(b)) => I(a.bvadd(&b)),
                (I(a), Op::Sub, I(b)) => I(a.bvsub(&b)),
                (I(a), Op::Mul, I(b)) => I(a.bvmul(&b)),
                // Bitwise ops are total on i64 — no definedness obligation.
                (I(a), Op::BitAnd, I(b)) => I(a.bvand(&b)),
                (I(a), Op::BitOr, I(b)) => I(a.bvor(&b)),
                (I(a), Op::BitXor, I(b)) => I(a.bvxor(&b)),
                // `bvsdiv`/`bvsrem` truncate like the emitted `sdiv`/`srem`;
                // both sides are partial in the same way, so the defined
                // domain is an explicit obligation, not an assumption.
                (I(a), Op::Div, I(b)) => {
                    definedness.push(guard.implies(&div_defined(&a, &b, ctx)));
                    I(a.bvsdiv(&b))
                }
                (I(a), Op::Rem, I(b)) => {
                    definedness.push(guard.implies(&div_defined(&a, &b, ctx)));
                    I(a.bvsrem(&b))
                }
                (I(a), Op::Shl, I(b)) => {
                    definedness.push(guard.implies(&shift_defined(&b, ctx)));
                    I(a.bvshl(&b))
                }
                // `>>` is the arithmetic (sign-propagating) shift `ashr`.
                (I(a), Op::Shr, I(b)) => {
                    definedness.push(guard.implies(&shift_defined(&b, ctx)));
                    I(a.bvashr(&b))
                }
                (B(a), Op::And, B(b)) => B(Bool::and(ctx, &[&a, &b])),
                (B(a), Op::Or, B(b)) => B(Bool::or(ctx, &[&a, &b])),
                (B(a), Op::Implies, B(b)) => B(a.implies(&b)),
                _ => {
                    return Err(SemanticContractError::Unsupported(
                        "unsupported source operator",
                    ))
                }
            }
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            let cond = source_expr(
                cond,
                ctx,
                env,
                guard,
                definedness,
                depth + 1,
                &BTreeSet::new(),
                written,
                loop_exits,
                call_sites,
            )?
            .boolean()?;
            // Branches may rebind linear-path names: each write merges
            // under the branch condition like the emitter's per-name
            // phi; a name written on one side keeps its prior value on
            // the other. Branch-local `let` bindings are lexical — they
            // never reach `then_writes`/`else_writes`.
            let mut then_writes = HashMap::new();
            let mut else_writes = HashMap::new();
            let (then_value, else_value) = (
                source_stmt(
                    then_branch,
                    ctx,
                    env,
                    &Bool::and(ctx, &[guard, &cond]),
                    definedness,
                    depth + 1,
                    assignable,
                    &mut then_writes,
                    loop_exits,
                    call_sites,
                )?,
                source_stmt(
                    else_branch,
                    ctx,
                    env,
                    &Bool::and(ctx, &[guard, &cond.not()]),
                    definedness,
                    depth + 1,
                    assignable,
                    &mut else_writes,
                    loop_exits,
                    call_sites,
                )?,
            );
            let names: BTreeSet<&String> = then_writes.keys().chain(else_writes.keys()).collect();
            for name in names {
                let then_bound = match then_writes.get(name) {
                    Some(value) => value.clone(),
                    None => env
                        .get(name)
                        .cloned()
                        .ok_or(SemanticContractError::Unsupported("source binding"))?,
                };
                let else_bound = match else_writes.get(name) {
                    Some(value) => value.clone(),
                    None => env
                        .get(name)
                        .cloned()
                        .ok_or(SemanticContractError::Unsupported("source binding"))?,
                };
                let merged = match (then_bound, else_bound) {
                    (I(a), I(b)) => I(cond.ite(&a, &b)),
                    (B(a), B(b)) => B(cond.ite(&a, &b)),
                    _ => return Err(SemanticContractError::Unsupported("branch write kinds")),
                };
                written.insert(name.clone(), merged);
            }
            match (then_value, else_value) {
                (I(a), I(b)) => I(cond.ite(&a, &b)),
                (B(a), B(b)) => B(cond.ite(&a, &b)),
                _ => return Err(SemanticContractError::Unsupported("source branch types")),
            }
        }
        // First-match order, like the emitter's try-chain and
        // `--bitvec-i64` verification: an arm runs under the negation of
        // every earlier arm's hit (`pattern ∧ guard`) plus its own; an
        // unguarded `Variable`/`Wildcard` arm matches everything that
        // remains, so any later arm is dead — it gets the literal
        // `false` miss term, which encodes it while binding nothing.
        Expr::Match { target, arms } => {
            let target = source_expr(
                target,
                ctx,
                env,
                guard,
                definedness,
                depth + 1,
                &BTreeSet::new(),
                written,
                loop_exits,
                call_sites,
            )?
            .integer()?;
            let mut missed: Vec<Bool> = Vec::new();
            let mut selected: Vec<(Bool, SourceValue)> = Vec::new();
            let mut fallback = None;
            for arm in arms {
                // An arm after a catch-all is never reached: its body is
                // still encoded (under an unsatisfiable guard, so its
                // obligations are vacuous) but its value is dropped.
                let dead = fallback.is_some();
                // The try-block reach is the enclosing guard plus the
                // negation of every earlier arm's hit; the arm hits when
                // its pattern matches and its Boolean guard holds, like
                // the emitter's `pattern ∧ guard` full condition. An arm
                // guard is compiled inside the try block, so its partial
                // operators bind under the reach alone.
                let mut terms: Vec<&Bool> = Vec::with_capacity(missed.len() + 1);
                terms.push(guard);
                terms.extend(missed.iter());
                let reach = Bool::and(ctx, &terms);
                // The arm-local part of `reach`: earlier arms' negated
                // hits, without the enclosing guard (the caller merges
                // `written` under its own condition).
                let arm_reach = Bool::and(ctx, &terms[1..]);
                let (pat_cond, binding) = match &arm.pattern {
                    Pattern::Literal(n) => (Some(target._eq(&BV::from_i64(ctx, *n, 64))), None),
                    Pattern::Variable(name) => (None, Some(name.clone())),
                    Pattern::Wildcard => (None, None),
                    _ => {
                        return Err(SemanticContractError::Unsupported(
                            "match pattern outside literals and catch-all",
                        ));
                    }
                };
                // A `Variable` pattern shadows an outer name for this
                // arm, so it leaves the assignable set and its writes
                // stay arm-local.
                let mut arm_assignable = assignable.clone();
                if let Some(name) = &binding {
                    arm_assignable.remove(name);
                }
                let mut arm_env = env.clone();
                if let Some(name) = binding {
                    arm_env.insert(name, I(target.clone()));
                }
                let hit = if dead {
                    None
                } else {
                    let hit = match (&pat_cond, &arm.guard) {
                        (pattern, Some(g)) => {
                            let gv = source_expr(
                                g,
                                ctx,
                                &arm_env,
                                &reach,
                                definedness,
                                depth + 1,
                                &BTreeSet::new(),
                                written,
                                loop_exits,
                                call_sites,
                            )?
                            .boolean()?;
                            Some(match pattern {
                                Some(p) => Bool::and(ctx, &[p, &gv]),
                                None => gv,
                            })
                        }
                        (Some(p), None) => Some(p.clone()),
                        (None, None) => None,
                    };
                    missed.push(
                        hit.as_ref()
                            .map_or(Bool::from_bool(ctx, false), |h| h.not()),
                    );
                    hit
                };
                // The arm body runs on `reach ∧ hit`: an unguarded
                // catch-all adds nothing (it matches all that remains).
                let body_guard = match &hit {
                    Some(h) => Bool::and(ctx, &[&reach, h]),
                    None => reach.clone(),
                };
                let mut arm_writes = HashMap::new();
                let value = source_stmt(
                    &arm.body,
                    ctx,
                    &arm_env,
                    &body_guard,
                    definedness,
                    depth + 1,
                    &arm_assignable,
                    &mut arm_writes,
                    loop_exits,
                    call_sites,
                )?;
                // Arm writes escape like `if` branch writes: each merges
                // under the arm-local hit (earlier arms' misses and this
                // arm's `pattern ∧ guard`), matching the emitter's
                // match.merge per-name phi; a dead arm's merge condition
                // is unsatisfiable so its writes never escape.
                let arm_hit = match &hit {
                    Some(h) => Bool::and(ctx, &[&arm_reach, h]),
                    None => arm_reach.clone(),
                };
                for (name, bound) in arm_writes {
                    let prior = written
                        .get(&name)
                        .cloned()
                        .or_else(|| env.get(&name).cloned())
                        .ok_or(SemanticContractError::Unsupported("source binding"))?;
                    let merged = match (prior, bound) {
                        (I(a), I(b)) => I(arm_hit.ite(&b, &a)),
                        (B(a), B(b)) => B(arm_hit.ite(&b, &a)),
                        _ => {
                            return Err(SemanticContractError::Unsupported(
                                "match arm write kinds",
                            ));
                        }
                    };
                    written.insert(name, merged);
                }
                if let Some(cond) = hit {
                    selected.push((cond, value));
                } else if fallback.is_none() {
                    fallback = Some(value);
                }
            }
            let Some(mut result) = fallback else {
                return Err(SemanticContractError::Unsupported(
                    "match without a catch-all arm",
                ));
            };
            for (cond, value) in selected.into_iter().rev() {
                result = match (result, value) {
                    (I(rest), I(v)) => I(cond.ite(&v, &rest)),
                    (B(rest), B(v)) => B(cond.ite(&v, &rest)),
                    _ => return Err(SemanticContractError::Unsupported("source match arm kinds")),
                };
            }
            result
        }
        Expr::Call(name, args) => {
            // An i64-only fragment: each argument must lower to a
            // bitvector; a `bool` argument would need an i1 domain and
            // is outside this profile.
            let mut applied = Vec::with_capacity(args.len());
            for arg in args {
                match source_expr(
                    arg,
                    ctx,
                    env,
                    guard,
                    definedness,
                    depth + 1,
                    assignable,
                    written,
                    loop_exits,
                    call_sites,
                )? {
                    I(v) => applied.push(v),
                    B(_) => {
                        return Err(SemanticContractError::Unsupported(
                            "source call argument kind",
                        ));
                    }
                }
            }
            let refs: Vec<&dyn Ast> = applied.iter().map(|v| v as &dyn Ast).collect();
            let Some(value) = call_decl(ctx, name, applied.len()).apply(&refs).as_bv() else {
                return Err(SemanticContractError::Unsupported(
                    "source call result kind",
                ));
            };
            call_sites.push(CallSite {
                callee: name.clone(),
                args: applied.clone(),
                result: value.clone(),
                guard: guard.clone(),
            });
            I(value)
        }
        _ => {
            return Err(SemanticContractError::Unsupported(
                "unsupported source expression",
            ))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = include_str!("../../tests/fixtures/semantic_contract_clamp.mm");
    const TARGET: &str = "x86_64-unknown-linux-gnu";

    fn extract(source: &str) -> Result<SemanticContract, SemanticContractError> {
        extract_pure_i64_contract(source, "clamp.mm", Some(TARGET), false)
    }

    #[test]
    fn extracts_typed_obligation_without_claiming_verification() {
        let contract = extract(SOURCE).unwrap();
        assert_eq!(contract.schema_version, SEMANTIC_CONTRACT_VERSION);
        assert_eq!(contract.specification.binders.len(), 2);
        assert_eq!(contract.specification.binders[0].name, "x");
        assert_eq!(contract.specification.binders[1].role, "result");
        assert_eq!(contract.source_span.file, "clamp.mm");
        assert!(contract.source_span.line > 0);
        assert_eq!(
            contract.numeric_semantics.verifier_integer,
            "mathematical_integer"
        );
        assert_eq!(contract.numeric_semantics.comparisons, "signed");
        assert_eq!(
            contract.correspondence,
            "unchecked_not_translation_validated"
        );
        assert!(contract.translator_ir.theorem_goal.contains("result"));
        assert_eq!(contract.translator_ir.binders.len(), 2);
        let json = serde_json::to_value(contract).unwrap();
        for field in ["verified", "result", "grade", "verification_status"] {
            assert!(json.get(field).is_none());
        }
    }

    #[test]
    fn identities_are_repeatable_and_separate_specification_from_implementation() {
        let original = extract(SOURCE).unwrap();
        for _ in 0..20 {
            let repeated = extract(SOURCE).unwrap();
            assert_eq!(original.hir.sha256, repeated.hir.sha256);
            assert_eq!(original.mir.sha256, repeated.mir.sha256);
        }
        let different_body = extract(&SOURCE.replace("else { x }", "else { 0 }")).unwrap();
        assert_eq!(original.obligation_id, different_body.obligation_id);
        assert_eq!(
            original.specification_sha256,
            different_body.specification_sha256
        );
        assert_ne!(original.source_sha256, different_body.source_sha256);
        assert_ne!(
            original.implementation_sha256,
            different_body.implementation_sha256
        );
        assert_ne!(original.atom_content_hash, different_body.atom_content_hash);
        assert_ne!(original.hir.sha256, different_body.hir.sha256);
        assert_ne!(original.mir.sha256, different_body.mir.sha256);
        let relocated = extract_pure_i64_contract(SOURCE, "other.mm", Some(TARGET), false).unwrap();
        assert_eq!(original.obligation_id, relocated.obligation_id);
        assert_eq!(original.source_sha256, relocated.source_sha256);
        assert_ne!(original.source_span, relocated.source_span);
    }

    #[test]
    fn let_bindings_stay_outside_the_bounded_c03_fragment() {
        // The equivalence profile (allow_wrapping_arithmetic = true) admits
        // `let`/expression/assignment sequences; the bounded C03 contract
        // still rejects them, so build and recheck keep agreeing on one
        // source grammar.
        for body in [
            "let y = x; y",
            "x + 1; x",
            "let a = x; a = a + 1; a",
            "let a = x; { let a = a + 1; a }; a",
            "if x > 0 { x = 1; x } else { x }; x",
        ] {
            let source = SOURCE.replace("if x < 0 { 0 } else { x }", body);
            assert!(extract(&source).is_err());
            assert!(extract_pure_i64_contract(&source, "clamp.mm", Some(TARGET), true).is_ok());
        }
    }

    #[test]
    fn calls_resolve_only_to_declared_sibling_atoms_with_matching_arity() {
        // The widened profile admits calls only as call sites into a
        // declared sibling atom: an undeclared name, a mismatched arity
        // or a duplicate atom name has no counterpart to compare
        // against and stays rejected.
        let base = |body: &str| {
            format!(
                "atom g(x: i64) -> i64 requires: true; ensures: true; body: {{ x }}; atom f(y: i64) -> i64 requires: true; ensures: true; body: {{ {body} }};"
            )
        };
        for body in ["missing(y)", "g(y, y)", "g()"] {
            assert!(
                extract_pure_i64_contract(&base(body), "clamp.mm", Some(TARGET), true).is_err(),
                "unexpected acceptance: {body}"
            );
        }
        // A discarded statement-position call resolves the same way.
        let discarded = base("missing(y); y");
        assert!(extract_pure_i64_contract(&discarded, "clamp.mm", Some(TARGET), true).is_err());
        let duplicate = format!("{SOURCE}\n{SOURCE}");
        assert!(extract_pure_i64_contract(&duplicate, "clamp.mm", Some(TARGET), true).is_err());
        // The bounded contract keeps its single-atom gate: sibling
        // atoms serve only the widened profile's call targets.
        assert!(extract(&duplicate).is_err());
        // A declared sibling callee — and a self call — resolve.
        assert!(extract_pure_i64_contract(
            &base("g(y) + g(y + 1)"),
            "clamp.mm",
            Some(TARGET),
            true
        )
        .is_ok());
        let self_call = base("f(y)");
        assert!(extract_pure_i64_contract(&self_call, "clamp.mm", Some(TARGET), true).is_ok());
    }

    #[test]
    fn bool_parameters_stay_outside_the_bounded_c03_fragment() {
        // The widened equivalence profile admits `bool` parameters
        // (lowered to i64 carrying 0/1); the bounded C03 contract keeps
        // its i64-only signature.
        let bool_atom = "atom f(b: bool, x: i64) -> i64 requires: true; ensures: true; body: { if b { x } else { 0 } };";
        assert!(extract_pure_i64_contract(bool_atom, "clamp.mm", Some(TARGET), false).is_err());
        assert!(extract_pure_i64_contract(bool_atom, "clamp.mm", Some(TARGET), true).is_ok());
        // A `-> bool` result is likewise widened-profile only; the
        // bounded fragment keeps its i64-only signature.
        let bool_return = "atom f(x: i64) -> bool requires: true; ensures: true; body: { x > 0 };";
        assert!(extract_pure_i64_contract(bool_return, "clamp.mm", Some(TARGET), false).is_err());
        assert!(extract_pure_i64_contract(bool_return, "clamp.mm", Some(TARGET), true).is_ok());
    }

    #[test]
    fn subrange_integer_parameters_stay_outside_the_bounded_c03_fragment() {
        // The widened equivalence profile admits declared subrange
        // integer types (carried by an i64 formal, the signature
        // bounding the checked input); the bounded C03 contract keeps
        // its i64-only signature.
        let i32_atom = "atom f(x: i32) -> i64 requires: true; ensures: true; body: { x };";
        assert!(extract_pure_i64_contract(i32_atom, "clamp.mm", Some(TARGET), false).is_err());
        assert!(extract_pure_i64_contract(i32_atom, "clamp.mm", Some(TARGET), true).is_ok());
        // Full-width unsigned and target-width types stay out of both
        // profiles: a `u64`-sized domain cannot be expressed as an i64
        // range bound.
        for ty in ["u64", "usize", "isize", "i128"] {
            let atom =
                format!("atom f(x: {ty}) -> i64 requires: true; ensures: true; body: {{ x }};");
            assert!(
                extract_pure_i64_contract(&atom, "clamp.mm", Some(TARGET), true).is_err(),
                "{ty}"
            );
        }
        // A declared subrange integer result is admitted too — it lowers
        // to an i64 return with no truncation — while `u64`-sized
        // results stay out on the same boundary.
        let i32_return = "atom f(x: i64) -> i32 requires: true; ensures: true; body: { x };";
        assert!(extract_pure_i64_contract(i32_return, "clamp.mm", Some(TARGET), false).is_err());
        let _contract =
            extract_pure_i64_contract(i32_return, "clamp.mm", Some(TARGET), true).unwrap();
        for ty in ["u64", "usize", "isize", "i128"] {
            let atom =
                format!("atom f(x: i64) -> {ty} requires: true; ensures: true; body: {{ x }};");
            assert!(
                extract_pure_i64_contract(&atom, "clamp.mm", Some(TARGET), true).is_err(),
                "{ty}"
            );
        }
        // The binders disclose the declared types, not the i64 carrier.
        let contract = extract_pure_i64_contract(i32_atom, "clamp.mm", Some(TARGET), true).unwrap();
        assert_eq!(contract.specification.binders[0].scalar_type, "i32");
        let contract =
            extract_pure_i64_contract(i32_return, "clamp.mm", Some(TARGET), true).unwrap();
        assert_eq!(contract.specification.binders[1].scalar_type, "i32");
    }

    #[test]
    fn parameter_types_are_part_of_the_specification_identity() {
        // `b: bool` and `b: i64` admit different input domains; sharing a
        // specification hash would alias their obligation identities.
        let bool_contract = extract_pure_i64_contract(
            "atom f(b: bool, x: i64) -> i64 requires: true; ensures: true; body: { if b { x } else { 0 } };",
            "clamp.mm",
            Some(TARGET),
            true,
        )
        .unwrap();
        let int_contract = extract_pure_i64_contract(
            "atom f(b: i64, x: i64) -> i64 requires: true; ensures: true; body: { if b > 0 { x } else { 0 } };",
            "clamp.mm",
            Some(TARGET),
            true,
        )
        .unwrap();
        assert_ne!(
            bool_contract.specification_sha256,
            int_contract.specification_sha256
        );
        assert_ne!(bool_contract.obligation_id, int_contract.obligation_id);
        // `-> i64` and `-> bool` are different signatures: identical
        // params/requires/ensures still admit different result kinds.
        let int_result = extract_pure_i64_contract(
            "atom f(x: i64) -> i64 requires: true; ensures: true; body: { x };",
            "clamp.mm",
            Some(TARGET),
            true,
        )
        .unwrap();
        let bool_result = extract_pure_i64_contract(
            "atom f(x: i64) -> bool requires: true; ensures: true; body: { x > 0 };",
            "clamp.mm",
            Some(TARGET),
            true,
        )
        .unwrap();
        assert_ne!(
            int_result.specification_sha256,
            bool_result.specification_sha256
        );
        assert_ne!(int_result.obligation_id, bool_result.obligation_id);
    }

    #[test]
    fn false_postcondition_is_an_obligation_not_an_extraction_failure() {
        let source = SOURCE.replace("(x < 0 => result == 0) && (x >= 0 => result == x)", "false");
        let contract = extract(&source).unwrap();
        assert_eq!(contract.specification.ensures.trim(), "false");
        assert_ne!(
            contract.obligation_id,
            extract(SOURCE).unwrap().obligation_id
        );
    }

    #[test]
    fn rejects_numeric_literals_that_the_tolerant_lexer_recovers_as_zero() {
        for literal in [
            "9223372036854775808",
            "-9223372036854775808",
            "999999999999999999999999999999999999",
        ] {
            for source in [
                SOURCE.replace("requires: true", &format!("requires: x < {literal}")),
                SOURCE.replace(
                    "ensures: (x < 0 => result == 0) && (x >= 0 => result == x)",
                    &format!("ensures: result == {literal}"),
                ),
                SOURCE.replace("if x < 0 { 0 } else { x }", literal),
            ] {
                assert!(
                    matches!(extract(&source), Err(SemanticContractError::Parse)),
                    "accepted a lossy numeric token: {literal}"
                );
            }
        }
        assert!(
            extract(&SOURCE.replace("if x < 0 { 0 } else { x }", "9223372036854775807")).is_ok()
        );
        assert!(extract(&format!(
            "// 9223372036854775808 is only a comment\n{SOURCE}"
        ))
        .is_ok());
    }

    #[test]
    fn rejects_float_tokens_before_text_reconstruction_can_erase_their_type() {
        for literal in [
            "0.0".to_owned(),
            "1.0".to_owned(),
            format!("0.{}1", "0".repeat(400)),
        ] {
            for source in [
                SOURCE.replace("requires: true", &format!("requires: x < {literal}")),
                SOURCE.replace("if x < 0 { 0 } else { x }", &literal),
            ] {
                assert!(matches!(
                    extract(&source),
                    Err(SemanticContractError::Unsupported(
                        "floating-point literals"
                    ))
                ));
            }
        }
        assert!(extract(&SOURCE.replace(
            "if x < 0 { 0 } else { x }",
            &format!("{}.0", "9".repeat(400))
        ))
        .is_err());
        assert!(extract(&format!("// 0.0 and 1.0 are comments\n{SOURCE}")).is_ok());
    }

    #[test]
    fn parser_normalization_and_translator_versions_are_disclosed() {
        let source = SOURCE.replace("requires: true", "requires: 0 <= x <= 3");
        let contract = extract(&source).unwrap();
        assert!(contract
            .normalization
            .contains("comparison_chains_to_conjunction"));
        assert!(contract
            .normalization
            .contains("not_independently_validated"));
        assert_eq!(contract.translator_version, LEAN_TRANSLATOR_VERSION);
        assert_eq!(contract.bridge_lemma_hash, LEAN_BRIDGE_LEMMA_HASH);
        assert_eq!(contract.requested_target_triple.as_deref(), Some(TARGET));
    }

    #[test]
    fn supports_other_comparison_functions_and_contract_conditionals() {
        let source = "atom maximum(x: i64, y: i64) -> i64\nrequires: true;\nensures: if x > y { result == x } else { result == y };\nbody: { if x > y { x } else { y } };";
        let contract = extract(source).unwrap();
        for _ in 0..20 {
            assert_eq!(extract(source).unwrap().mir.sha256, contract.mir.sha256);
        }
        assert_eq!(contract.specification.binders.len(), 3);
    }

    #[test]
    fn fails_closed_on_constructs_outside_the_profile() {
        for body in [
            "x + 1",
            "x - 1",
            "x * 2",
            "x / 2",
            "x % 2",
            "x << 1",
            "x >> 1",
            "f(x)",
            "1.5",
            "\"text\"",
            "result",
            "missing",
            "true",
            "let y = x; y",
            "x = 0; x",
            "if x { 0 } else { x }",
            "if true { 0 } else { false }",
            "",
        ] {
            let source =
                format!("atom f(x: i64) -> i64 requires: true; ensures: true; body: {{ {body} }};");
            assert!(extract(&source).is_err(), "unexpected acceptance: {body}");
        }
        for source in [
            SOURCE.replace("x: i64", "x: f64"),
            SOURCE.replace("x: i64", "x: [i64]"),
            SOURCE.replace("x: i64", "x"),
            SOURCE.replace("x: i64", "ref x: i64"),
            SOURCE.replace("x: i64", "consume x: i64"),
            SOURCE.replace("-> i64", "-> bool"),
            SOURCE.replace("requires:", "semantics: bitvec; requires:"),
            SOURCE.replace("requires:", "effects: IO; requires:"),
            SOURCE.replace("atom ", "trusted atom "),
            SOURCE.replace("atom ", "async atom "),
            format!("type Nat = i64 where v >= 0;\n{SOURCE}"),
            // Sibling atoms are admitted as call targets, but each must
            // independently satisfy the atom profile.
            format!(
                "{SOURCE}\natom g(x: f64) -> i64 requires: true; ensures: true; body: {{ x }};"
            ),
            SOURCE.replace("requires:", "unknown_clause:"),
            SOURCE.replace("requires: true", "requires: result == 0"),
            SOURCE.replace("x: i64", "x: i64, x: i64"),
            SOURCE.replace("x: i64", "result: i64"),
        ] {
            assert!(extract(&source).is_err(), "unexpected acceptance: {source}");
        }
        assert!(extract_pure_i64_contract(SOURCE, "", Some(TARGET), false).is_err());
        assert!(extract_pure_i64_contract(SOURCE, "clamp.mm", Some(""), false).is_err());
        assert!(extract_pure_i64_contract(SOURCE, "clamp.mm", None, false)
            .unwrap()
            .requested_target_triple
            .is_none());
    }

    fn module_outcomes(source: &str) -> Vec<z3::SatResult> {
        let mut config = z3::Config::new();
        config.set_timeout_msec(crate::verification::VerificationConfig::default().timeout_ms);
        let context = z3::Context::new(&config);
        let solver = z3::Solver::new(&context);
        let obligations = encode_pure_i64_module_obligations(source, &context).unwrap();
        obligations
            .iter()
            .map(|obligation| {
                solver.reset();
                solver.assert(obligation);
                solver.check()
            })
            .collect()
    }

    #[test]
    fn module_obligations_keep_every_postcondition_and_call_domain_independent() {
        // An in-domain call: every obligation — the caller's and the
        // callee's postconditions plus the call-site domain — is unsat.
        let guarded = "atom helper(x: i64) -> i64 requires: x > 0; ensures: result == x + 1; body: { x + 1 };\natom main() -> i64 requires: true; ensures: result == 6; body: { helper(5) };";
        assert!(module_outcomes(guarded)
            .iter()
            .all(|result| *result == z3::SatResult::Unsat));
        // A call outside the callee's `requires`: one obligation is
        // refutable — and it must be refutable on its own, not masked
        // inside a conjunction that could collapse the whole check.
        let out_of_domain = guarded.replace("helper(5)", "helper(0)");
        assert!(module_outcomes(&out_of_domain)
            .iter()
            .any(|result| *result == z3::SatResult::Sat));
        // A `requires: true` callee has vacuous domain obligations; the
        // caller's postcondition must still be checked independently —
        // `id(2^63-1) + 1` wraps below zero in the widened profile even
        // though the call's domain check is trivially closed.
        let wrapped = "atom id(x: i64) -> i64 requires: true; ensures: result == x; body: { x };\natom main() -> i64 requires: true; ensures: result > 0; body: { id(9223372036854775807) + 1 };";
        let outcomes = module_outcomes(wrapped);
        assert!(outcomes.iter().any(|result| *result == z3::SatResult::Sat));
        assert!(outcomes
            .iter()
            .all(|result| *result != z3::SatResult::Unknown));
    }
}
