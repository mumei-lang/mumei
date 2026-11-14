//! An opt-in, deliberately small description of one scalar source obligation.
//! Extraction is not verification or translation validation.

use crate::hir::lower_atom_to_hir;
use crate::mir::lower_hir_to_mir;
use crate::parser::{
    parse_body_expr_checked, parse_expression_checked, parse_module_checked, Atom, Expr, Item, Op,
    Span, Stmt, TrustLevel,
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
    Context,
};

pub const SEMANTIC_CONTRACT_VERSION: &str = "mumei.semantic-contract/v1";
pub const PURE_I64_PROFILE: &str = "mumei.pure-i64-comparison/v1";
/// The wider source fragment checked inside the LLVM equivalence profile:
/// the same scalar comparisons plus wrapping i64 add/sub/mul, total
/// bitwise `&`/`|`/`^`, guarded div/rem/shl/shr (defined only for nonzero
/// divisors excluding
/// i64::MIN / -1 and shift amounts in 0..64), block-scoped `let`
/// bindings, scalar reassignments on the linear path, nested blocks and
/// discarded pure expression statements. Kept distinct from the
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

/// Extract a single explicit-i64 atom without imports, calls, arithmetic or effects.
/// The target is caller-supplied metadata, not a claim of backend availability.
/// `allow_wrapping_arithmetic` admits i64 add/sub/mul as mod-2^64 wrapping
/// operations, total bitwise `&`/`|`/`^`, plus guarded div/rem/shl/shr (a
/// partial domain: nonzero divisor, no i64::MIN / -1, shift amount in 0..64),
/// block-scoped `let` bindings, scalar reassignments on the linear path
/// (a rebinding cannot escape an `if` branch, expression block or nested
/// block), discarded pure expression statements and `bool` parameters
/// and results lowered to i64 carrying 0/1;
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
    let mut items = parse_module_checked(source).map_err(|_| SemanticContractError::Parse)?;
    if items.len() != 1 {
        return Err(SemanticContractError::Unsupported(
            "expected exactly one atom",
        ));
    }
    let Some(Item::Atom(mut atom)) = items.pop() else {
        return Err(SemanticContractError::Unsupported("non-atom declarations"));
    };
    validate_atom(&atom, allow_wrapping_arithmetic)?;
    atom.span.file = source_name.to_owned();
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
        expression_kind(&requires, &scope, false, allow_wrapping_arithmetic)?,
        ScalarKind::Bool,
    )?;
    let result_kind = return_kind(&atom);
    let mut ensures_scope = scope.clone();
    ensures_scope.insert("result".to_owned(), result_kind);
    expect_kind(
        expression_kind(&ensures, &ensures_scope, true, allow_wrapping_arithmetic)?,
        ScalarKind::Bool,
    )?;
    expect_kind(
        statement_kind(
            &body,
            &scope,
            false,
            allow_wrapping_arithmetic,
            &scope.keys().cloned().collect(),
        )?,
        result_kind,
    )?;

    let mut binders: Vec<_> = atom
        .params
        .iter()
        .map(|p| TypedBinder {
            name: p.name.clone(),
            scalar_type: if param_kind(p) == ScalarKind::Bool {
                "bool"
            } else {
                "i64"
            },
            role: "parameter",
            provenance: atom.span.clone(),
            span_precision: "atom",
        })
        .collect();
    binders.push(TypedBinder {
        name: "result".to_owned(),
        scalar_type: if result_kind == ScalarKind::Bool {
            "bool"
        } else {
            "i64"
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
        .chain(allow_wrapping_arithmetic.then_some("bool_results_lowered_to_i64_zero_one"))
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
                "No unsigned division, logical shifts, heap, calls, FFI, ownership, effects or concurrency in this profile. Arithmetic wraps mod 2^64; overflow is not flagged or proven. Division, remainder and shifts bind inputs only where defined."
            } else {
                "No arithmetic, heap, calls, FFI, ownership, effects or concurrency in this profile."
            },
        ],
    })
}

/// The scalar kind a declared parameter type gives in this profile, when
/// the declaration is admissible: `i64` always, `bool` only when
/// `admit_bool` (the widened profile's `allow_wrapping_arithmetic` flag).
fn declared_param_kind(param: &crate::parser::ast::Param, admit_bool: bool) -> Option<ScalarKind> {
    let name = param.type_name.as_deref()?;
    let kind = match name {
        "i64" => ScalarKind::I64,
        "bool" if admit_bool => ScalarKind::Bool,
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

fn validate_atom(atom: &Atom, admit_bool: bool) -> Result<(), SemanticContractError> {
    let return_ok = match atom.return_type.as_deref() {
        Some("i64") => true,
        Some("bool") => admit_bool,
        _ => false,
    };
    if !return_ok
        || atom
            .params
            .iter()
            .any(|p| declared_param_kind(p, admit_bool).is_none())
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
) -> Result<ScalarKind, SemanticContractError> {
    match stmt {
        Stmt::Expr(expr, _) => {
            expression_kind(expr, scope, result_in_scope, allow_wrapping_arithmetic)
        }
        Stmt::Block(stmts, _) if stmts.len() == 1 => statement_kind(
            &stmts[0],
            scope,
            result_in_scope,
            allow_wrapping_arithmetic,
            assignable,
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
                        )?;
                        expect_kind(kind, inner[var])?;
                    }
                    Stmt::Expr(value, _) => {
                        expression_kind(value, &inner, result_in_scope, allow_wrapping_arithmetic)?;
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
            )
        }
        _ => Err(SemanticContractError::Unsupported(
            "statement, mutation or empty body",
        )),
    }
}

fn expression_kind(
    expr: &Expr,
    scope: &BTreeMap<String, ScalarKind>,
    result_in_scope: bool,
    allow_wrapping_arithmetic: bool,
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
            let left = expression_kind(left, scope, result_in_scope, allow_wrapping_arithmetic)?;
            let right = expression_kind(right, scope, result_in_scope, allow_wrapping_arithmetic)?;
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
        ),
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            expect_kind(
                expression_kind(cond, scope, result_in_scope, allow_wrapping_arithmetic)?,
                ScalarKind::Bool,
            )?;
            let then_kind = statement_kind(
                then_branch,
                scope,
                result_in_scope,
                allow_wrapping_arithmetic,
                &BTreeSet::new(),
            )?;
            expect_kind(
                statement_kind(
                    else_branch,
                    scope,
                    result_in_scope,
                    allow_wrapping_arithmetic,
                    &BTreeSet::new(),
                )?,
                then_kind,
            )?;
            Ok(then_kind)
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
pub struct PureI64Body<'ctx> {
    pub function_name: String,
    pub source_sha256: String,
    pub inputs: Vec<BV<'ctx>>,
    pub value: BV<'ctx>,
    /// Definedness obligations for division, remainder and shifts, each an
    /// implication `guard -> operands defined` mirroring the safety rules
    /// `--bitvec-i64` verification requires. Empty unless the caller admitted
    /// those operators; their conjunction is the domain the source binds.
    pub definedness: Vec<Bool<'ctx>>,
    /// Domain the declared signature itself binds: an admitted `bool`
    /// parameter lowers to i64 carrying 0/1, so its entry is `input <=u 1`.
    /// Empty without `bool` parameters.
    pub param_domain: Vec<Bool<'ctx>>,
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
    let mut items = parse_module_checked(source).map_err(|_| SemanticContractError::Parse)?;
    let Some(Item::Atom(atom)) = items.pop() else {
        return Err(SemanticContractError::Unsupported("source atom"));
    };
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
    let value = match source_stmt(&body, context, &env, &top, &mut definedness, 0, &assignable)? {
        SourceValue::Integer(value) => value,
        SourceValue::Boolean(value) => {
            value.ite(&BV::from_u64(context, 1, 64), &BV::from_u64(context, 0, 64))
        }
    };
    // A declared `bool` parameter lowers to i64 carrying 0/1, so the
    // declared signature restricts its input to {0, 1}: without this the
    // i64 range would make `b == true` look different at `input = 5`.
    let param_domain: Vec<Bool> = atom
        .params
        .iter()
        .zip(&inputs)
        .filter(|(p, _)| param_kind(p) == ScalarKind::Bool)
        .map(|(_, v)| v.bvule(&BV::from_u64(context, 1, 64)))
        .collect();
    Ok(PureI64Body {
        function_name: atom.name,
        source_sha256: contract.source_sha256,
        inputs,
        value,
        definedness,
        param_domain,
    })
}

const MAX_DEPTH: usize = 48;

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
    let mut env: HashMap<String, SourceValue> = param_env(&atom.params, &body.inputs, context);
    let requires =
        parse_expression_checked(&atom.requires).map_err(|_| SemanticContractError::Parse)?;
    let top = Bool::from_bool(context, true);
    let mut sink = Vec::new();
    let requires = source_expr(&requires, context, &env, &top, &mut sink, 0)?.boolean()?;
    env.insert("result".to_string(), SourceValue::Integer(body.value));
    let ensures =
        parse_expression_checked(&atom.ensures).map_err(|_| SemanticContractError::Parse)?;
    let ensures = source_expr(&ensures, context, &env, &top, &mut sink, 0)?.boolean()?;
    Ok(Bool::and(context, &[&requires, &ensures.not()]))
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
/// current linear path may be rebound, because the source model cannot
/// merge an assignment that escapes an `if` branch or expression block.
fn source_stmt<'ctx>(
    stmt: &Stmt,
    ctx: &'ctx Context,
    env: &HashMap<String, SourceValue<'ctx>>,
    guard: &Bool<'ctx>,
    definedness: &mut Vec<Bool<'ctx>>,
    depth: usize,
    assignable: &BTreeSet<String>,
) -> BodyResult<SourceValue<'ctx>> {
    if depth > MAX_DEPTH {
        return Err(SemanticContractError::Unsupported("source depth limit"));
    }
    match stmt {
        Stmt::Expr(e, _) => source_expr(e, ctx, env, guard, definedness, depth + 1),
        Stmt::Block(stmts, _) if stmts.len() == 1 => source_stmt(
            &stmts[0],
            ctx,
            env,
            guard,
            definedness,
            depth + 1,
            assignable,
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
            let (lets, last) = stmts.split_at(stmts.len() - 1);
            for stmt in lets {
                match stmt {
                    Stmt::Let { var, value, .. } => {
                        let bound = source_expr(value, ctx, &inner, guard, definedness, depth + 1)?;
                        inner.insert(var.clone(), bound);
                        inner_assignable.insert(var.clone());
                    }
                    Stmt::Assign { var, value, .. } => {
                        if !inner_assignable.contains(var) {
                            return Err(SemanticContractError::Unsupported(
                                "assignment to a name not bound on the linear path",
                            ));
                        }
                        let bound = source_expr(value, ctx, &inner, guard, definedness, depth + 1)?;
                        inner.insert(var.clone(), bound);
                    }
                    Stmt::Expr(value, _) => {
                        source_expr(value, ctx, &inner, guard, definedness, depth + 1)?;
                    }
                    _ => {
                        return Err(SemanticContractError::Unsupported(
                            "statement outside a let/assignment/expression sequence",
                        ));
                    }
                }
            }
            source_stmt(
                &last[0],
                ctx,
                &inner,
                guard,
                definedness,
                depth + 1,
                &inner_assignable,
            )
        }
        _ => Err(SemanticContractError::Unsupported(
            "unsupported source statement",
        )),
    }
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

fn source_expr<'ctx>(
    expr: &Expr,
    ctx: &'ctx Context,
    env: &HashMap<String, SourceValue<'ctx>>,
    guard: &Bool<'ctx>,
    definedness: &mut Vec<Bool<'ctx>>,
    depth: usize,
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
        Expr::Block(s) => {
            source_stmt(s, ctx, env, guard, definedness, depth + 1, &BTreeSet::new())?
        }
        Expr::BinaryOp(a, op, b) => {
            let a = source_expr(a, ctx, env, guard, definedness, depth + 1)?;
            // `&&`/`||`/`=>` short-circuit like the emitter and
            // `--bitvec-i64` verification: the right operand's partial
            // operations bind only when the left one lets it run.
            let b_guard = match (&a, op) {
                (B(ab), Op::And) | (B(ab), Op::Implies) => Bool::and(ctx, &[guard, ab]),
                (B(ab), Op::Or) => Bool::and(ctx, &[guard, &ab.not()]),
                _ => guard.clone(),
            };
            let b = source_expr(b, ctx, env, &b_guard, definedness, depth + 1)?;
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
            let cond = source_expr(cond, ctx, env, guard, definedness, depth + 1)?.boolean()?;
            match (
                source_stmt(
                    then_branch,
                    ctx,
                    env,
                    &Bool::and(ctx, &[guard, &cond]),
                    definedness,
                    depth + 1,
                    &BTreeSet::new(),
                )?,
                source_stmt(
                    else_branch,
                    ctx,
                    env,
                    &Bool::and(ctx, &[guard, &cond.not()]),
                    definedness,
                    depth + 1,
                    &BTreeSet::new(),
                )?,
            ) {
                (I(a), I(b)) => I(cond.ite(&a, &b)),
                (B(a), B(b)) => B(cond.ite(&a, &b)),
                _ => return Err(SemanticContractError::Unsupported("source branch types")),
            }
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
        ] {
            let source = SOURCE.replace("if x < 0 { 0 } else { x }", body);
            assert!(extract(&source).is_err());
            assert!(extract_pure_i64_contract(&source, "clamp.mm", Some(TARGET), true).is_ok());
        }
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
            format!("{SOURCE}\n{SOURCE}"),
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
}
