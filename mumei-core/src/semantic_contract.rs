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
/// The widened fragment's refined-scalar extension: module-level
/// `type Name = <scalar> where <predicate>;` declarations and their use
/// in atom signatures. A declaration's predicate joins the value's
/// domain — a refined parameter constrains the checked input, a refined
/// result constrains the checked postcondition and a call site must
/// establish the callee's refined precondition. The predicates are
/// producer-declared obligations under the same encoder, not claims
/// about wider language semantics.
pub const PURE_I64_REFINED_PROFILE: &str = "mumei.pure-i64-refined-scalar/v1";
/// v2 widens the admitted declaration fragment the same way the
/// verifier's boundary enforcement widened its encoding: a `bool` leaf
/// binds the operand as `bool` (not `Int`), and a base that names
/// another declaration composes the whole chain's conjuncts instead of
/// dropping the ancestors. A module that uses either names this
/// profile; a module inside the v1 fragment keeps the v1 name so a v1
/// record's pinned profile stays reproducible under its own gate.
pub const PURE_I64_REFINED_PROFILE_V2: &str = "mumei.pure-i64-refined-scalar/v2";
/// The composite extension: module-level non-generic `struct`
/// declarations whose fields are all profile scalars (literal or
/// refined), struct-typed atom parameters and `p.f` field reads on them.
/// A struct parameter is flattened to one scalar input per field in
/// declaration order; the struct value itself is never a scalar, so it
/// only passes through to a sibling taking the same struct. Struct
/// construction, struct results and struct-valued `let`s stay outside.
/// A module that declares no struct keeps its refined or wrapping name.
pub const PURE_I64_COMPOSITE_PROFILE: &str = "mumei.pure-i64-composite/v1";

/// How much of the refined-scalar declaration fragment a gate admits:
/// `Reject` keeps `Item::TypeDef` out of the item vocabulary entirely,
/// `V1` admits single-level declarations over the i64-family scalar
/// vocabulary only (the `pure-i64-refined-scalar/v1` fragment), and `V2`
/// additionally admits chained declarations and `bool` leaves (the v2
/// fragment). `Composite` keeps the V2 declaration fragment and adds
/// all-scalar `struct` declarations as module members
/// (`pure-i64-composite/v1`). A pinned record names which level produced it, so a
/// rechecker re-derives under the record's own admission — never a
/// wider one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefinedAdmission {
    Reject,
    V1,
    V2,
    Composite,
}

impl RefinedAdmission {
    /// Whether any `Item::TypeDef` declaration is in the item
    /// vocabulary at all.
    pub fn admits_typedefs(self) -> bool {
        self != Self::Reject
    }

    /// Whether `Item::StructDef` declarations are in the item vocabulary.
    pub fn admits_structs(self) -> bool {
        self == Self::Composite
    }
}

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
    let (items, refined, structs) =
        admit_pure_i64_atoms(source, allow_wrapping_arithmetic, RefinedAdmission::Reject)?;
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
        &refined,
        &structs,
        &ModuleEnv::default(),
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
    admission: RefinedAdmission,
) -> Result<Vec<SemanticContract>, SemanticContractError> {
    if source_name.trim().is_empty() || target_triple.is_some_and(|target| target.trim().is_empty())
    {
        return Err(SemanticContractError::Unsupported(
            "missing source or target identity",
        ));
    }
    let (items, refined, structs) = admit_pure_i64_atoms(source, true, admission)?;
    // The translator-IR pin must see the same type environment the
    // verifier's certificate does — a `Port`-typed binder carries its
    // refinement annotation only when the alias is registered, and a
    // struct-typed binder resolves only against registered structs.
    let mut cert_env = ModuleEnv::default();
    for item in &items {
        match item {
            Item::TypeDef(typedef) => cert_env.register_type(typedef),
            Item::StructDef(struct_def) => cert_env.register_struct(struct_def),
            _ => (),
        }
    }
    // A declaration-only module under the refined profile carries no
    // contract — the nonempty-items gate no longer implies an atom.
    if !items.iter().any(|item| matches!(item, Item::Atom(_))) {
        return Err(SemanticContractError::Unsupported(
            "expected at least one atom",
        ));
    }
    items
        .into_iter()
        .filter_map(|item| match item {
            Item::Atom(atom) => Some(atom_contract(
                atom,
                source,
                source_name,
                target_triple,
                true,
                &refined,
                &structs,
                &cert_env,
            )),
            // Type and struct declarations carry no per-atom contract;
            // their admission was the gate above.
            _ => None,
        })
        .collect()
}

/// The profile name the module gate would pin for `source` under
/// `admission` — the same `fragment_profile` the contract builder
/// emits, so a consumer reporting the source profile names exactly
/// what the obligation pin does.
pub fn module_source_profile(
    source: &str,
    admission: RefinedAdmission,
) -> Result<&'static str, SemanticContractError> {
    let (_items, refined, structs) = admit_pure_i64_atoms(source, true, admission)?;
    Ok(fragment_profile(true, &refined, &structs))
}

/// Shared module admission for the pure-i64 fragment: float reject, parse,
/// nonempty, callee-map construction and the per-atom
/// `validate_atom`/`check_atom_contract` gates. Returns the admitted items
/// in declaration order plus the module's resolved refined-alias and
/// struct maps; `admission` widens the item vocabulary to `Item::TypeDef`
/// declarations (`PURE_I64_REFINED_PROFILE`/`_V2`) and, at `Composite`,
/// `Item::StructDef` declarations (`PURE_I64_COMPOSITE_PROFILE`) while
/// the scalar fragment itself is unchanged. The caller decides the
/// arity gate.
fn admit_pure_i64_atoms(
    source: &str,
    allow_wrapping_arithmetic: bool,
    admission: RefinedAdmission,
) -> Result<(Vec<Item>, RefinedEnv, StructEnv), SemanticContractError> {
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
    let mut atom_names = BTreeSet::new();
    for item in &items {
        match item {
            Item::Atom(sibling) => {
                if !atom_names.insert(sibling.name.clone()) {
                    return Err(SemanticContractError::Unsupported("duplicate atom name"));
                }
            }
            Item::TypeDef(_) if admission.admits_typedefs() => (),
            Item::StructDef(_) if admission.admits_structs() => (),
            _ => {
                return Err(SemanticContractError::Unsupported("non-atom declarations"));
            }
        }
    }
    // Refined declarations share the module's name space with atoms: a
    // type named like an atom would make `T(` ambiguous between a type
    // position and a call, so the admission is closed over both maps.
    let refined = build_refined_env(&items, admission)?;
    let structs = build_struct_env(&items, &refined)?;
    if refined
        .keys()
        .chain(structs.keys())
        .any(|name| atom_names.contains(name))
    {
        return Err(SemanticContractError::Unsupported(
            "type name collides with an atom name",
        ));
    }
    let callees: BTreeMap<String, CalleeSig> = items
        .iter()
        .filter_map(|item| {
            let Item::Atom(sibling) = item else {
                return None;
            };
            Some((
                sibling.name.clone(),
                CalleeSig {
                    param_kinds: sibling
                        .params
                        .iter()
                        .map(|p| param_shape(p, &refined, &structs))
                        .collect(),
                    return_kind: return_kind(sibling, &refined),
                },
            ))
        })
        .collect();
    // Declaration order is irrelevant to call resolution: the callee map is
    // complete before any atom's own contract is checked, so a forward call
    // to a later sibling resolves the same as a backward one.
    for item in &items {
        let Item::Atom(sibling) = item else {
            continue;
        };
        validate_atom(sibling, allow_wrapping_arithmetic, &refined, &structs)?;
        // Every atom's contract is checked against the complete callee map,
        // not only the tail's: module validation compares each atom
        // independently, so an undeclared call target in any sibling must
        // fail closed here rather than encode as an uninterpreted
        // application that trivially agrees with an external IR member.
        check_atom_contract(
            sibling,
            &callees,
            allow_wrapping_arithmetic,
            &refined,
            &structs,
        )?;
    }
    Ok((items, refined, structs))
}

/// Build the `SemanticContract` for one already-admitted atom — binders,
/// specification hash, HIR/MIR/translator-IR identities and the profile
/// fields all describe this atom while `source_sha256` binds the whole
/// module text.
/// The scalar type a binder reports for its declared type name: literal
/// scalar names map directly; a declared refined alias reports the literal
/// its chain resolves to (`x: Port` lowers to `i64` like the emitter).
/// Anything else is unreachable past admission — kept `"i64"` so a
/// non-admitted caller still gets the lowered default instead of a panic.
fn binder_scalar_type(type_name: Option<&str>, refined: &RefinedEnv) -> &'static str {
    match type_name {
        Some("bool") => "bool",
        Some("i8") => "i8",
        Some("i16") => "i16",
        Some("i32") => "i32",
        Some("u8") => "u8",
        Some("u16") => "u16",
        Some("u32") => "u32",
        Some(name) => refined.get(name).map(|alias| alias.base).unwrap_or("i64"),
        None => "i64",
    }
}

/// The fragment profile a module's contracts name: the bounded C03
/// fragment, the widened wrapping fragment, its refined-scalar
/// extension when the module admitted any `type` declarations, or the
/// composite extension when it admitted any `struct` declarations.
fn fragment_profile(
    allow_wrapping_arithmetic: bool,
    refined: &RefinedEnv,
    structs: &StructEnv,
) -> &'static str {
    if !allow_wrapping_arithmetic {
        PURE_I64_PROFILE
    } else if !structs.is_empty() {
        PURE_I64_COMPOSITE_PROFILE
    } else if refined.is_empty() {
        PURE_I64_WRAPPING_PROFILE
    } else if refined
        .values()
        .any(|alias| alias.kind == ScalarKind::Bool || alias.conjuncts.len() > 1)
    {
        PURE_I64_REFINED_PROFILE_V2
    } else {
        PURE_I64_REFINED_PROFILE
    }
}

#[allow(clippy::too_many_arguments)]
fn atom_contract(
    mut atom: Atom,
    source: &str,
    source_name: &str,
    target_triple: Option<&str>,
    allow_wrapping_arithmetic: bool,
    refined: &RefinedEnv,
    structs: &StructEnv,
    cert_env: &ModuleEnv,
) -> Result<SemanticContract, SemanticContractError> {
    atom.span.file = source_name.to_owned();
    let profile = fragment_profile(allow_wrapping_arithmetic, refined, structs);

    // A struct parameter reports one scalar binder per field (`p.x`) —
    // the flat expansion the obligation encodes — never a scalar binder
    // for the struct value itself.
    let mut binders: Vec<_> = atom
        .params
        .iter()
        .flat_map(|p| match struct_fields(p, structs) {
            Some(fields) => fields
                .iter()
                .map(|f| (format!("{}.{}", p.name, f.name), Some(f.type_name.as_str())))
                .collect::<Vec<_>>(),
            None => vec![(p.name.clone(), p.type_name.as_deref())],
        })
        .map(|(name, type_name)| TypedBinder {
            name,
            scalar_type: binder_scalar_type(type_name, refined),
            role: "parameter",
            provenance: atom.span.clone(),
            span_precision: "atom",
        })
        .collect();
    binders.push(TypedBinder {
        name: "result".to_owned(),
        scalar_type: binder_scalar_type(atom.return_type.as_deref(), refined),
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
        "profile": profile,
        // The declared parameter types are part of the specification
        // identity: `b: bool` and `b: i64` admit different input domains
        // and must not share one specification hash.
        // A struct parameter's identity includes its declared field
        // layout: reordering or retyping `Point`'s fields changes the
        // input domain. Scalar parameters keep their original shape so
        // non-composite specification hashes are unchanged.
        "params": atom
            .params
            .iter()
            .map(|p| match struct_fields(p, structs) {
                Some(fields) => serde_json::json!({
                    "name": p.name,
                    "type": p.type_name,
                    "fields": fields
                        .iter()
                        .map(|f| serde_json::json!({"name": f.name, "type": f.type_name}))
                        .collect::<Vec<_>>(),
                }),
                None => serde_json::json!({"name": p.name, "type": p.type_name}),
            })
            .collect::<Vec<_>>(),
        "return_type": atom.return_type,
        "requires": atom.requires, "ensures": atom.ensures,
    }))?;
    let composite_params = atom
        .params
        .iter()
        .any(|p| struct_fields(p, structs).is_some());
    let hir = lower_atom_to_hir(&atom);
    let mir = lower_hir_to_mir(&hir);
    Ok(SemanticContract {
        schema_version: SEMANTIC_CONTRACT_VERSION,
        profile,
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
        translator_ir: build_translator_ir_metadata(&atom, cert_env),
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
        .chain(composite_params.then_some("struct_parameters_flattened_to_scalar_fields"))
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

/// One sibling atom's call-site signature: the kind each declared
/// parameter carries and the return kind a call expression takes. The
/// arity map alone cannot type a call through a widened signature — a
/// bool parameter admits only bool arguments, and a bool return keeps
/// the call Bool-typed.
struct CalleeSig {
    param_kinds: Vec<ValueKind>,
    return_kind: ScalarKind,
}

/// The shape a declared parameter carries: a profile scalar, or a
/// struct of the module's admitted struct map, which is never a scalar
/// value — it only exposes field reads and passes through to a sibling
/// parameter of the same struct.
#[derive(Clone, PartialEq, Eq)]
enum ValueKind {
    Scalar(ScalarKind),
    Struct(String),
}

/// The kind-checking environment shared by every expression position of
/// one atom: the module's callee map, the atom's struct-typed parameters
/// (parameter name → struct name) and the module's struct map.
struct KindEnv<'a> {
    callees: &'a BTreeMap<String, CalleeSig>,
    composites: BTreeMap<String, &'a str>,
    structs: &'a StructEnv,
}

impl KindEnv<'_> {
    /// The fields a struct-typed parameter exposes, unless a scalar
    /// binding of the same name (a `let` or a match-arm binder) shadows
    /// it on the current path.
    fn composite(
        &self,
        name: &str,
        scope: &BTreeMap<String, ScalarKind>,
    ) -> Option<(&str, &[ResolvedField])> {
        if scope.contains_key(name) {
            return None;
        }
        let struct_name = *self.composites.get(name)?;
        Some((struct_name, self.structs.get(struct_name)?.as_slice()))
    }
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
    refined: &RefinedEnv,
) -> Option<ScalarKind> {
    let name = param.type_name.as_deref()?;
    // A declared refined alias resolves to its scalar base; any other
    // undeclared type name stays outside the profile rather than fall to
    // the `i64` default.
    let kind = match name {
        "i64" => ScalarKind::I64,
        "bool" if admit_widened => ScalarKind::Bool,
        "i8" | "i16" | "i32" | "u8" | "u16" | "u32" if admit_widened => ScalarKind::I64,
        // A bool-leaf alias gives the parameter a boolean signature —
        // the same boundary literal `bool` carries.
        other => match refined.get(other) {
            Some(alias) if alias.kind != ScalarKind::Bool || admit_widened => alias.kind,
            _ => return None,
        },
    };
    plain_param(param, name).then_some(kind)
}

/// An owned, by-value parameter whose type reference is exactly the
/// declared name — no type arguments, effects, capabilities, borrows or
/// function contracts.
fn plain_param(param: &crate::parser::ast::Param, name: &str) -> bool {
    !(param.type_ref.as_ref().is_some_and(|ty| {
        ty.name != name
            || !ty.type_args.is_empty()
            || ty.effect_set.is_some()
            || ty.capability.is_some()
    }) || param.consume
        || param.is_ref
        || param.is_ref_mut
        || param.fn_contract_requires.is_some()
        || param.fn_contract_ensures.is_some())
}

/// `declared_param_kind` widened by the module's struct map: a parameter
/// typed by an admitted struct carries that struct's shape. Struct
/// names never overlap the scalar or refined names (`build_struct_env`).
fn declared_param_shape(
    param: &crate::parser::ast::Param,
    admit_widened: bool,
    refined: &RefinedEnv,
    structs: &StructEnv,
) -> Option<ValueKind> {
    match param.type_name.as_deref() {
        Some(name) if structs.contains_key(name) => {
            plain_param(param, name).then(|| ValueKind::Struct(name.to_owned()))
        }
        _ => declared_param_kind(param, admit_widened, refined).map(ValueKind::Scalar),
    }
}

/// The admitted struct fields of a struct-typed parameter, `None` for a
/// scalar parameter.
fn struct_fields<'a>(
    param: &crate::parser::ast::Param,
    structs: &'a StructEnv,
) -> Option<&'a [ResolvedField]> {
    structs.get(param.type_name.as_deref()?).map(Vec::as_slice)
}

/// `param_kind` for a parameter that already passed `validate_atom`,
/// keeping its struct shape.
fn param_shape(
    param: &crate::parser::ast::Param,
    refined: &RefinedEnv,
    structs: &StructEnv,
) -> ValueKind {
    match param.type_name.as_deref() {
        Some(name) if structs.contains_key(name) => ValueKind::Struct(name.to_owned()),
        _ => ValueKind::Scalar(param_kind(param, refined)),
    }
}

/// The parameter kind inside a contract that already passed
/// `validate_atom` — unvalidated sources never reach the kind checker.
/// A refined alias reports its resolved base kind (`Port` → `i64`).
fn param_kind(param: &crate::parser::ast::Param, refined: &RefinedEnv) -> ScalarKind {
    match param.type_name.as_deref() {
        Some("bool") => ScalarKind::Bool,
        Some(name) => refined
            .get(name)
            .map(|alias| alias.kind)
            .unwrap_or(ScalarKind::I64),
        None => ScalarKind::I64,
    }
}

/// The kind the declared return type gives in this profile — `i64`,
/// `bool` when the widened profile admitted the declaration, or a refined
/// alias's resolved base kind.
fn return_kind(atom: &Atom, refined: &RefinedEnv) -> ScalarKind {
    match atom.return_type.as_deref() {
        Some("bool") => ScalarKind::Bool,
        Some(name) => refined
            .get(name)
            .map(|alias| alias.kind)
            .unwrap_or(ScalarKind::I64),
        None => ScalarKind::I64,
    }
}

/// The kind every module atom's call expression takes, keyed by name:
/// a bool-returning callee's call lowers to the profile's i64 0/1
/// bitvector, and `source_expr` needs the callee's declared return kind
/// to materialize that convention at the call site.
fn call_return_kinds(items: &[Item], refined: &RefinedEnv) -> BTreeMap<String, ScalarKind> {
    items
        .iter()
        .filter_map(|item| {
            let Item::Atom(atom) = item else {
                return None;
            };
            Some((atom.name.clone(), return_kind(atom, refined)))
        })
        .collect()
}

/// The tail-atom contract admission shared by every module atom:
/// requires/ensures must be boolean expressions and the body a statement
/// of the declared result kind — all under call resolution against the
/// complete same-module callee map.
fn check_atom_contract(
    atom: &Atom,
    callees: &BTreeMap<String, CalleeSig>,
    allow_wrapping_arithmetic: bool,
    refined: &RefinedEnv,
    structs: &StructEnv,
) -> Result<(), SemanticContractError> {
    // Struct parameters stay out of the scalar scope — so they are
    // neither scalar operands nor rebindable — and live in the
    // parallel composite map that field reads and pass-through calls
    // consult.
    let mut scope: BTreeMap<String, ScalarKind> = BTreeMap::new();
    let mut composites: BTreeMap<String, &str> = BTreeMap::new();
    for p in &atom.params {
        match param_shape(p, refined, structs) {
            ValueKind::Scalar(kind) => {
                scope.insert(p.name.clone(), kind);
            }
            ValueKind::Struct(_) => {
                let name = p.type_name.as_deref().unwrap_or_default();
                composites.insert(p.name.clone(), name);
            }
        }
    }
    let env = KindEnv {
        callees,
        composites,
        structs,
    };
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
            &env,
        )?,
        ScalarKind::Bool,
    )?;
    let result_kind = return_kind(atom, refined);
    let mut ensures_scope = scope.clone();
    ensures_scope.insert("result".to_owned(), result_kind);
    expect_kind(
        expression_kind(
            &ensures,
            &ensures_scope,
            true,
            allow_wrapping_arithmetic,
            &BTreeSet::new(),
            &env,
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
            &env,
        )?,
        result_kind,
    )?;
    Ok(())
}

fn validate_atom(
    atom: &Atom,
    admit_widened: bool,
    refined: &RefinedEnv,
    structs: &StructEnv,
) -> Result<(), SemanticContractError> {
    // A declared subrange integer result lowers to an i64 return like
    // the parameters — the emitter emits no truncation, so the checked
    // result is the body's full i64 value. `u64`-sized results stay out
    // on the same signature boundary as the parameters. Admitting them
    // is likewise widened-profile only. A declared refined alias admits
    // its resolved scalar base — which is never `u64`-sized by
    // construction (`build_refined_env` rejects non-scalar bases).
    let return_ok = match atom.return_type.as_deref() {
        Some("i64") => true,
        Some("bool") | Some("i8" | "i16" | "i32" | "u8" | "u16" | "u32") => admit_widened,
        Some(name) => refined
            .get(name)
            .is_some_and(|alias| alias.kind != ScalarKind::Bool || admit_widened),
        None => false,
    };
    if !return_ok
        || atom
            .params
            .iter()
            .any(|p| declared_param_shape(p, admit_widened, refined, structs).is_none())
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

/// One admitted refined scalar type declaration resolved against the
/// module's declaration map: the literal scalar name its base resolves
/// to, the kind that literal carries in this profile, and the
/// declaration's refinement conjuncts — each conjunct's declared operand
/// name is substituted with the use-site value at encoding time.
struct ResolvedAlias {
    base: &'static str,
    kind: ScalarKind,
    conjuncts: Vec<(String, String)>,
}

/// The module's admitted refined scalar declarations by type name. An
/// empty map is the plain wrapping fragment — a nonempty one is exactly
/// what `PURE_I64_REFINED_PROFILE` adds over `PURE_I64_WRAPPING_PROFILE`.
type RefinedEnv = BTreeMap<String, ResolvedAlias>;

/// One admitted struct field: its name, the declared scalar type name
/// (a literal profile scalar or a refined alias) and the kind it
/// carries.
struct ResolvedField {
    name: String,
    type_name: String,
    kind: ScalarKind,
}

/// The module's admitted struct declarations by name, fields in
/// declaration order — the order a struct parameter flattens in. An
/// empty map is every non-composite fragment.
type StructEnv = BTreeMap<String, Vec<ResolvedField>>;

/// Collect the module's `Item::StructDef` declarations (only present
/// past the item-vocabulary gate under `Composite` admission). Nothing
/// a declaration carries may be dropped silently, so generic
/// parameters, invariants, methods, field constraints and empty field
/// lists are rejected; every field must be a profile scalar — a
/// literal scalar name or an admitted refined alias (no struct-valued
/// fields, so no recursion). Names shadowing scalar profile names,
/// colliding with a refined type, duplicate structs and duplicate
/// fields fail closed.
fn build_struct_env(
    items: &[Item],
    refined: &RefinedEnv,
) -> Result<StructEnv, SemanticContractError> {
    let mut env = StructEnv::new();
    for item in items {
        let Item::StructDef(def) = item else {
            continue;
        };
        if !def.type_params.is_empty() {
            return Err(SemanticContractError::Unsupported(
                "generic struct declarations",
            ));
        }
        if !def.invariants.is_empty() {
            return Err(SemanticContractError::Unsupported("struct invariants"));
        }
        if !def.methods.is_empty() || !def.method_names.is_empty() {
            return Err(SemanticContractError::Unsupported("struct methods"));
        }
        if def.fields.is_empty() {
            return Err(SemanticContractError::Unsupported(
                "empty struct declarations",
            ));
        }
        if scalar_base(&def.name).is_some()
            || matches!(def.name.as_str(), "u64" | "usize" | "isize")
        {
            return Err(SemanticContractError::Unsupported(
                "type name shadows a scalar profile name",
            ));
        }
        if refined.contains_key(&def.name) {
            return Err(SemanticContractError::Unsupported(
                "struct name collides with a type name",
            ));
        }
        let mut names = BTreeSet::new();
        let mut fields = Vec::with_capacity(def.fields.len());
        for field in &def.fields {
            if !names.insert(field.name.as_str()) {
                return Err(SemanticContractError::Unsupported("duplicate struct field"));
            }
            if field.constraint.is_some() {
                return Err(SemanticContractError::Unsupported(
                    "constrained struct fields",
                ));
            }
            let ty = &field.type_ref;
            let kind = match scalar_base(&field.type_name) {
                Some((kind, _)) => Some(kind),
                None => refined.get(&field.type_name).map(|alias| alias.kind),
            };
            let Some(kind) = kind.filter(|_| {
                ty.name == field.type_name
                    && ty.type_args.is_empty()
                    && ty.effect_set.is_none()
                    && ty.capability.is_none()
            }) else {
                return Err(SemanticContractError::Unsupported(
                    "struct field type outside the scalar profile",
                ));
            };
            fields.push(ResolvedField {
                name: field.name.clone(),
                type_name: field.type_name.clone(),
                kind,
            });
        }
        if env.insert(def.name.clone(), fields).is_some() {
            return Err(SemanticContractError::Unsupported("duplicate struct name"));
        }
    }
    Ok(env)
}

/// The literal scalar name and kind a profile scalar type gives, or
/// `None` outside the vocabulary (`u64`/`usize`/`f64`/unknown names —
/// a full-width unsigned domain cannot be expressed as an i64 bound,
/// and the obligation vocabulary has no Real/String sorts). `bool` is
/// a V2 leaf: the verifier binds its operand as `Bool`, so admission
/// gates it on the V2 fragment rather than this table.
fn scalar_base(name: &str) -> Option<(ScalarKind, &'static str)> {
    match name {
        "bool" => Some((ScalarKind::Bool, "bool")),
        "i64" => Some((ScalarKind::I64, "i64")),
        "i8" => Some((ScalarKind::I64, "i8")),
        "i16" => Some((ScalarKind::I64, "i16")),
        "i32" => Some((ScalarKind::I64, "i32")),
        "u8" => Some((ScalarKind::I64, "u8")),
        "u16" => Some((ScalarKind::I64, "u16")),
        "u32" => Some((ScalarKind::I64, "u32")),
        _ => None,
    }
}

/// Collect the module's `Item::TypeDef` declarations into the resolved
/// alias map. Rejected honestly: `unit` tags (metadata this profile does
/// not model — dropping it would silently change the admitted fragment),
/// type names shadowing scalar profile names, duplicates, bases outside
/// the scalar vocabulary, and predicates that are not boolean
/// scalar-vocabulary expressions over their operand (checked with no
/// call targets, so a refinement cannot reach a sibling atom). Chained
/// declarations and `bool` leaves additionally require the V2
/// admission level — the verifier composes both, but the V1 record
/// gate keeps its original fragment.
fn build_refined_env(
    items: &[Item],
    admission: RefinedAdmission,
) -> Result<RefinedEnv, SemanticContractError> {
    let mut defs = BTreeMap::new();
    for item in items {
        let Item::TypeDef(def) = item else {
            continue;
        };
        if def.unit.is_some() {
            return Err(SemanticContractError::Unsupported(
                "unit-tagged type declarations",
            ));
        }
        if matches!(
            def.name.as_str(),
            "i64"
                | "bool"
                | "i8"
                | "i16"
                | "i32"
                | "u8"
                | "u16"
                | "u32"
                | "u64"
                | "usize"
                | "isize"
        ) {
            return Err(SemanticContractError::Unsupported(
                "type name shadows a scalar profile name",
            ));
        }
        if defs.insert(def.name.as_str(), def).is_some() {
            return Err(SemanticContractError::Unsupported("duplicate type name"));
        }
    }
    let mut env = RefinedEnv::new();
    for (name, def) in &defs {
        env.insert(name.to_string(), resolve_alias(def, &defs, admission)?);
    }
    Ok(env)
}

fn resolve_alias(
    def: &crate::parser::ast::RefinedType,
    defs: &BTreeMap<&str, &crate::parser::ast::RefinedType>,
    admission: RefinedAdmission,
) -> Result<ResolvedAlias, SemanticContractError> {
    // Walk the declaration chain to the literal base, accumulating each
    // declaration's own (operand, predicate) conjunct — the same
    // fold the verifier applies per link, so no ancestor is silently
    // dropped. A base naming another declaration needs V2 admission
    // (the V1 record gate keeps its single-level fragment); a cycle
    // is rejected fail-closed either way, matching the verifier's own
    // type error.
    let mut conjuncts: Vec<(String, String)> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current = def;
    let leaf = loop {
        if !seen.insert(current.name.as_str()) {
            return Err(SemanticContractError::Unsupported(
                "cyclic type declarations",
            ));
        }
        conjuncts.push((current.operand.clone(), current.predicate_raw.clone()));
        match defs.get(current._base_type.as_str()) {
            Some(next) => {
                if admission < RefinedAdmission::V2 {
                    return Err(SemanticContractError::Unsupported(
                        "chained type declarations",
                    ));
                }
                current = next;
            }
            None => break current._base_type.clone(),
        }
    };
    let (kind, base) = scalar_base(&leaf).ok_or(SemanticContractError::Unsupported(
        "type outside the scalar profile",
    ))?;
    if kind == ScalarKind::Bool && admission < RefinedAdmission::V2 {
        return Err(SemanticContractError::Unsupported(
            "bool-leaf type declarations",
        ));
    }
    // Every declaration's predicate must be a scalar-vocabulary boolean
    // expression over its own operand, which binds the leaf-typed value.
    for (operand, predicate_raw) in &conjuncts {
        let predicate =
            parse_expression_checked(predicate_raw).map_err(|_| SemanticContractError::Parse)?;
        expect_kind(
            expression_kind(
                &predicate,
                &BTreeMap::from([(operand.clone(), kind)]),
                false,
                true,
                &BTreeSet::new(),
                &KindEnv {
                    callees: &BTreeMap::new(),
                    composites: BTreeMap::new(),
                    structs: &StructEnv::new(),
                },
            )?,
            ScalarKind::Bool,
        )?;
    }
    Ok(ResolvedAlias {
        base,
        kind,
        conjuncts,
    })
}

/// True when the statement subtree holds a `while` at any depth —
/// statements nested inside expression blocks, `if` branches, match
/// arms and task/acquire bodies all count. Admission uses it to keep
/// nested loops out of the bounded-unroll profile; consumers use it to
/// describe which atoms carry the bounded loop-exit domain.
pub fn contains_while_stmt(stmt: &Stmt) -> bool {
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
    env: &KindEnv<'_>,
) -> Result<ScalarKind, SemanticContractError> {
    match stmt {
        Stmt::Expr(expr, _) => expression_kind(
            expr,
            scope,
            result_in_scope,
            allow_wrapping_arithmetic,
            assignable,
            env,
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
                env,
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
            env,
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
                            env,
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
                            env,
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
                            env,
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
                            env,
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
                            env,
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
                env,
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
    env: &KindEnv<'_>,
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
                env,
            )?;
            let right = expression_kind(
                right,
                scope,
                result_in_scope,
                allow_wrapping_arithmetic,
                &BTreeSet::new(),
                env,
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
            env,
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
                    env,
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
                env,
            )?;
            expect_kind(
                statement_kind(
                    else_branch,
                    scope,
                    result_in_scope,
                    allow_wrapping_arithmetic,
                    assignable,
                    env,
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
                    env,
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
                            env,
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
                    env,
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
            let Some(sig) = env.callees.get(name.as_str()) else {
                return Err(SemanticContractError::Unsupported(
                    "call target outside module",
                ));
            };
            if sig.param_kinds.len() != args.len() {
                return Err(SemanticContractError::Unsupported(
                    "call target outside module",
                ));
            }
            // Each argument carries the callee parameter's declared
            // kind — a widened bool parameter admits only bool
            // arguments — and the call takes the callee's return kind.
            // A struct parameter admits only an unshadowed struct
            // parameter of the same struct passed through by name: no
            // struct value is ever constructed or computed.
            for (arg, kind) in args.iter().zip(&sig.param_kinds) {
                match kind {
                    ValueKind::Scalar(kind) => expect_kind(
                        expression_kind(
                            arg,
                            scope,
                            result_in_scope,
                            allow_wrapping_arithmetic,
                            &BTreeSet::new(),
                            env,
                        )?,
                        *kind,
                    )?,
                    ValueKind::Struct(expected) => {
                        let passes = matches!(
                            arg,
                            Expr::Variable(name)
                                if env
                                    .composite(name, scope)
                                    .is_some_and(|(actual, _)| actual == expected)
                        );
                        if !passes {
                            return Err(SemanticContractError::Unsupported(
                                "struct argument outside parameter pass-through",
                            ));
                        }
                    }
                }
            }
            Ok(sig.return_kind)
        }
        // A field read names an unshadowed struct parameter directly and
        // takes the declared field's kind; nested, computed or unknown
        // targets stay outside.
        Expr::FieldAccess(target, field) => {
            let Expr::Variable(name) = target.as_ref() else {
                return Err(SemanticContractError::Unsupported(
                    "field read outside a struct parameter",
                ));
            };
            env.composite(name, scope)
                .and_then(|(_, fields)| fields.iter().find(|f| f.name == *field))
                .map(|f| f.kind)
                .ok_or(SemanticContractError::Unsupported(
                    "field read outside a struct parameter",
                ))
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
    /// The bool-carrier convention applied to call results: a callee
    /// whose declared return kind is `bool` produces an i64 in {0,1} at
    /// the call boundary, so each such application's result carries
    /// `result <=u 1`. Asserting the bound on the shared application
    /// term makes a forwarded call result (`-> Flag { f(x) }`, where
    /// the source materializes `ite(v != 0, 1, 0)` and the IR returns
    /// the raw `v`) compare equal under the convention — while a
    /// member that returns a literal `5` for a bool result still
    /// differs from the normalized source.
    pub call_result_bounds: Vec<Bool<'ctx>>,
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
    /// True when the atom's signature resolves through a refined scalar
    /// type declaration — its domain bound then also carries the
    /// declaration predicates encoded into `param_domain`.
    pub refined_signature: bool,
    /// True when a parameter is struct-typed: `inputs` then holds that
    /// parameter's fields flattened in declaration order, which no
    /// single scalar formal of the emitted member corresponds to.
    pub composite_signature: bool,
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
    let (source_sha256, mut items, refined, structs) =
        bounded_pure_i64_module(source, allow_wrapping_arithmetic, RefinedAdmission::Reject)?;
    let call_returns = call_return_kinds(&items, &refined);
    let Some(Item::Atom(atom)) = items.pop() else {
        return Err(SemanticContractError::Unsupported("source atom"));
    };
    encode_atom_body(
        &atom,
        context,
        source_sha256,
        &refined,
        &structs,
        &call_returns,
    )
}

/// Encode every atom of the module under the same profile, in declaration
/// order. Each entry is an independent per-atom obligation: a call is an
/// uninterpreted function site, so an atom compares only its own body
/// against the same-named IR function — never the callee's body.
pub fn encode_pure_i64_module<'ctx>(
    source: &str,
    context: &'ctx Context,
    allow_wrapping_arithmetic: bool,
    admission: RefinedAdmission,
) -> BodyResult<Vec<PureI64Body<'ctx>>> {
    let (source_sha256, items, refined, structs) =
        bounded_pure_i64_module(source, allow_wrapping_arithmetic, admission)?;
    let call_returns = call_return_kinds(&items, &refined);
    items
        .iter()
        .filter_map(|item| match item {
            Item::Atom(atom) => Some(encode_atom_body(
                atom,
                context,
                source_sha256.clone(),
                &refined,
                &structs,
                &call_returns,
            )),
            _ => None,
        })
        .collect()
}

/// Shared admission for the per-atom encoders: input bounds, module
/// admission (which itself gates every declared item of the profile)
/// and the module item list. Returns the module digest, the items and
/// the resolved refined-alias and struct maps — empty for callers that
/// keep the plain fragment (`admission = Reject`).
fn bounded_pure_i64_module(
    source: &str,
    allow_wrapping_arithmetic: bool,
    admission: RefinedAdmission,
) -> BodyResult<(String, Vec<Item>, RefinedEnv, StructEnv)> {
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
    let (items, refined, structs) =
        admit_pure_i64_atoms(source, allow_wrapping_arithmetic, admission)?;
    // The bounded profile keeps its single-atom arity gate here the same
    // way `extract_pure_i64_contract` applies it.
    if !allow_wrapping_arithmetic && items.len() != 1 {
        return Err(SemanticContractError::Unsupported(
            "expected exactly one atom",
        ));
    }
    Ok((digest(source.as_bytes()), items, refined, structs))
}

fn encode_atom_body<'ctx>(
    atom: &crate::parser::ast::Atom,
    context: &'ctx Context,
    source_sha256: String,
    refined: &RefinedEnv,
    structs: &StructEnv,
    call_returns: &BTreeMap<String, ScalarKind>,
) -> BodyResult<PureI64Body<'ctx>> {
    // A struct parameter contributes one input per field, so the limit
    // bounds the flattened signature.
    let slots = signature_slots(&atom.params, structs);
    if slots.len() > 16 {
        return Err(SemanticContractError::Unsupported("parameter limit"));
    }
    let body =
        parse_body_expr_checked(&atom.body_expr).map_err(|_| SemanticContractError::Parse)?;
    let inputs: Vec<_> = (0..slots.len())
        .map(|i| BV::new_const(context, format!("input_{i}"), 64))
        .collect();
    let env: HashMap<String, SourceValue> =
        param_env(&atom.params, &inputs, context, refined, structs)?;
    let top = Bool::from_bool(context, true);
    let mut definedness = Vec::new();
    let composite_signature = atom
        .params
        .iter()
        .any(|p| struct_fields(p, structs).is_some());
    let assignable: BTreeSet<String> = atom
        .params
        .iter()
        .filter(|p| struct_fields(p, structs).is_none())
        .map(|p| p.name.clone())
        .collect();
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
        call_returns,
    )? {
        SourceValue::Integer(value) => value,
        SourceValue::Boolean(value) => {
            value.ite(&BV::from_u64(context, 1, 64), &BV::from_u64(context, 0, 64))
        }
        SourceValue::Composite(_) => {
            return Err(SemanticContractError::Unsupported("source struct result"))
        }
    };
    let param_domain = signature_domain(&atom.params, &inputs, context, refined, structs)?;
    let subrange_params = slots
        .iter()
        .any(|slot| matches!(slot, Some("i8" | "i16" | "i32" | "u8" | "u16" | "u32")));
    let refined_signature = slots
        .iter()
        .any(|slot| slot.is_some_and(|n| refined.contains_key(n)))
        || atom
            .return_type
            .as_deref()
            .is_some_and(|n| refined.contains_key(n));
    let loop_bound_exits = if loop_exits.is_empty() {
        None
    } else {
        let refs: Vec<&Bool> = loop_exits.iter().collect();
        Some(Bool::and(context, &refs))
    };
    let call_result_bounds: Vec<Bool> = call_sites
        .iter()
        .filter(|site| call_returns.get(site.callee.as_str()) == Some(&ScalarKind::Bool))
        .map(|site| site.result.bvule(&BV::from_u64(context, 1, 64)))
        .collect();
    Ok(PureI64Body {
        function_name: atom.name.clone(),
        source_sha256,
        inputs,
        value,
        call_sites,
        call_result_bounds,
        definedness,
        param_domain,
        subrange_params,
        refined_signature,
        composite_signature,
        loop_bound_exits,
    })
}

/// The declared scalar type name of every flattened input slot, in
/// order: a scalar parameter is one slot, a struct parameter one slot
/// per field in declaration order.
fn signature_slots<'a>(
    params: &'a [crate::parser::ast::Param],
    structs: &'a StructEnv,
) -> Vec<Option<&'a str>> {
    params
        .iter()
        .flat_map(|p| match struct_fields(p, structs) {
            Some(fields) => fields.iter().map(|f| Some(f.type_name.as_str())).collect(),
            None => vec![p.type_name.as_deref()],
        })
        .collect()
}

/// Bounds the declared signature places on each formal: `bool` and
/// subrange integer parameters lower to i64 formals carrying the type's
/// range — `bool` restricts the input to {0, 1} (without it `b == true`
/// would look different at `input = 5`) and `x: i32` checks only inputs
/// in `[-2^31, 2^31)`. Shared between an atom's own body encoding (as
/// `param_domain`) and a module call site's callee arguments.
fn signature_domain<'ctx>(
    params: &[crate::parser::ast::Param],
    inputs: &[BV<'ctx>],
    context: &'ctx Context,
    refined: &RefinedEnv,
    structs: &StructEnv,
) -> BodyResult<Vec<Bool<'ctx>>> {
    let mut domain = Vec::new();
    // A struct field's declared scalar type bounds its flattened slot
    // the same way it would bound a scalar parameter of that type.
    let slots = signature_slots(params, structs);
    if slots.len() != inputs.len() {
        return Err(SemanticContractError::Unsupported("source signature arity"));
    }
    for (type_name, v) in slots.into_iter().zip(inputs) {
        match type_name {
            Some("bool") => domain.push(v.bvule(&BV::from_u64(context, 1, 64))),
            Some("u8") => domain.push(v.bvule(&BV::from_u64(context, u8::MAX as u64, 64))),
            Some("u16") => domain.push(v.bvule(&BV::from_u64(context, u16::MAX as u64, 64))),
            Some("u32") => domain.push(v.bvule(&BV::from_u64(context, u32::MAX as u64, 64))),
            Some("i8") => domain.extend([
                v.bvsge(&BV::from_i64(context, i8::MIN as i64, 64)),
                v.bvsle(&BV::from_i64(context, i8::MAX as i64, 64)),
            ]),
            Some("i16") => domain.extend([
                v.bvsge(&BV::from_i64(context, i16::MIN as i64, 64)),
                v.bvsle(&BV::from_i64(context, i16::MAX as i64, 64)),
            ]),
            Some("i32") => domain.extend([
                v.bvsge(&BV::from_i64(context, i32::MIN as i64, 64)),
                v.bvsle(&BV::from_i64(context, i32::MAX as i64, 64)),
            ]),
            Some(name) => {
                if let Some(alias) = refined.get(name) {
                    // The alias's base bound still applies — a `u8`-based
                    // refinement keeps its range carrier, a `bool`-leaf
                    // its 0/1 domain — before the declaration's conjuncts
                    // tighten it.
                    domain.extend(scalar_domain_bounds(alias.base, v, context));
                    let operand = if alias.kind == ScalarKind::Bool {
                        SourceValue::Boolean(v._eq(&BV::from_u64(context, 0, 64)).not())
                    } else {
                        SourceValue::Integer(v.clone())
                    };
                    domain.extend(alias_predicates(alias, operand, context)?);
                }
            }
            None => (),
        }
    }
    Ok(domain)
}

/// The input bounds one literal profile scalar name contributes — the
/// same bounds `signature_domain` applies to a parameter declared with
/// that name. Shared so a refined alias's resolved base carries the
/// identical range its literal spelling would.
fn scalar_domain_bounds<'ctx>(base: &str, v: &BV<'ctx>, context: &'ctx Context) -> Vec<Bool<'ctx>> {
    match base {
        "bool" => vec![v.bvule(&BV::from_u64(context, 1, 64))],
        "u8" => vec![v.bvule(&BV::from_u64(context, u8::MAX as u64, 64))],
        "u16" => vec![v.bvule(&BV::from_u64(context, u16::MAX as u64, 64))],
        "u32" => vec![v.bvule(&BV::from_u64(context, u32::MAX as u64, 64))],
        "i8" => vec![
            v.bvsge(&BV::from_i64(context, i8::MIN as i64, 64)),
            v.bvsle(&BV::from_i64(context, i8::MAX as i64, 64)),
        ],
        "i16" => vec![
            v.bvsge(&BV::from_i64(context, i16::MIN as i64, 64)),
            v.bvsle(&BV::from_i64(context, i16::MAX as i64, 64)),
        ],
        "i32" => vec![
            v.bvsge(&BV::from_i64(context, i32::MIN as i64, 64)),
            v.bvsle(&BV::from_i64(context, i32::MAX as i64, 64)),
        ],
        _ => Vec::new(),
    }
}

/// Encode one resolved alias's refinement conjuncts at a use-site value:
/// each conjunct's declared operand binds the value and the predicate
/// text runs through the same `source_expr` the contract clauses use.
/// The definedness conditions the encoder raises for partial operations
/// inside a predicate join the output too — the refinement only holds
/// where the predicate is defined. Calls stay out by admission (the
/// declaration gate checked predicates with no call targets).
fn alias_predicates<'ctx>(
    alias: &ResolvedAlias,
    operand_value: SourceValue<'ctx>,
    context: &'ctx Context,
) -> BodyResult<Vec<Bool<'ctx>>> {
    let mut out = Vec::new();
    for (operand, text) in &alias.conjuncts {
        let predicate = parse_expression_checked(text).map_err(|_| SemanticContractError::Parse)?;
        let scope = HashMap::from([(operand.clone(), operand_value.clone())]);
        let top = Bool::from_bool(context, true);
        let mut definedness = Vec::new();
        let mut exits = Vec::new();
        let encoded = source_expr(
            &predicate,
            context,
            &scope,
            &top,
            &mut definedness,
            0,
            &BTreeSet::new(),
            &mut HashMap::new(),
            &mut exits,
            &mut Vec::new(),
            &BTreeMap::new(),
        )?
        .boolean()?;
        out.extend(definedness);
        out.extend(exits);
        out.push(encoded);
    }
    Ok(out)
}

/// The refinement conjuncts a declared type name contributes at one
/// symbolic value — parameters at their input, results at the body's
/// encoded value, call-site arguments at the actual argument. Empty for
/// literal scalar types and undeclared names.
fn refined_conjuncts_at<'ctx>(
    type_name: Option<&str>,
    value: SourceValue<'ctx>,
    refined: &RefinedEnv,
    context: &'ctx Context,
) -> BodyResult<Vec<Bool<'ctx>>> {
    let Some(name) = type_name else {
        return Ok(Vec::new());
    };
    let Some(alias) = refined.get(name) else {
        return Ok(Vec::new());
    };
    alias_predicates(alias, value, context)
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
    atom_obligation(&atom, &body, &BTreeMap::new(), context, &RefinedEnv::new())
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
    admission: RefinedAdmission,
) -> BodyResult<Vec<ModuleAtomObligations<'ctx>>> {
    let (source_sha256, items, refined, structs) =
        bounded_pure_i64_module(source, true, admission)?;
    let atom_items: Vec<&crate::parser::ast::Atom> = items
        .iter()
        .filter_map(|item| match item {
            Item::Atom(atom) => Some(atom),
            _ => None,
        })
        .collect();
    // The v1 nonempty gate guaranteed ≥1 atom because only atoms were
    // admitted; a declaration-only module under the refined profile has
    // items but nothing to check — vacuous obligations prove nothing.
    if atom_items.is_empty() {
        return Err(SemanticContractError::Unsupported(
            "expected at least one atom",
        ));
    }
    let call_returns = call_return_kinds(&items, &refined);
    let bodies: Vec<PureI64Body> = atom_items
        .iter()
        .map(|atom| {
            encode_atom_body(
                atom,
                context,
                source_sha256.clone(),
                &refined,
                &structs,
                &call_returns,
            )
        })
        .collect::<BodyResult<_>>()?;
    let callees: BTreeMap<String, &crate::parser::ast::Atom> = atom_items
        .iter()
        .map(|atom| (atom.name.clone(), *atom))
        .collect();
    let callee_bodies: BTreeMap<String, &PureI64Body> = atom_items
        .iter()
        .zip(bodies.iter())
        .map(|(atom, body)| (atom.name.clone(), body))
        .collect();
    let mut atoms = Vec::new();
    for (atom, body) in atom_items.iter().zip(bodies.iter()) {
        atoms.push(module_atom_obligations(
            atom,
            body,
            &callees,
            &callee_bodies,
            &call_returns,
            context,
            &refined,
            &structs,
        )?);
    }
    Ok(atoms)
}

/// One module atom's counterexample obligations plus the input domain
/// the profile binds for it. Every `obligations` entry must be `unsat`;
/// `domain` must be satisfiable for that to mean anything — an atom
/// whose signature bounds, guarded-operator definedness or
/// bounded-while exits leave no input is outside the profile, matching
/// the LLVM side's empty-domain rejection rather than reading vacuous
/// obligations as proven.
pub struct ModuleAtomObligations<'ctx> {
    pub atom: String,
    pub domain: Bool<'ctx>,
    pub obligations: Vec<Bool<'ctx>>,
}

/// The module-atom obligation family — every entry must be `unsat`:
///
/// - entry 0: `requires ∧ call_assumptions ∧ ¬ensures ∧ exits` — the
///   caller's own postcondition, assuming each callee honoured its
///   `ensures`;
/// - entries 1..: `requires ∧ site_guard ∧ ¬(callee_requires ∧ callee_domain)`
///   — the caller establishes the callee's precondition *and* stays
///   inside the domain the callee was verified on (its signature
///   bounds, guarded-operator definedness and bounded-while exits,
///   instantiated at the call arguments) on every path that reaches
///   the call, so the ensures assumption above is justified.
///
/// The domain obligations deliberately exclude the other sites' ensures
/// assumptions, so no circular justification is possible: a call whose
/// own domain needs a sibling's earlier ensures falls outside this
/// fragment instead of being bootstrapped.
/// Contract expressions that themselves contain calls keep those calls
/// uninterpreted — a callee's own contract is its own obligation.
#[allow(clippy::too_many_arguments)]
fn module_atom_obligations<'ctx>(
    atom: &Atom,
    body: &PureI64Body<'ctx>,
    callees: &BTreeMap<String, &crate::parser::ast::Atom>,
    callee_bodies: &BTreeMap<String, &PureI64Body<'ctx>>,
    call_returns: &BTreeMap<String, ScalarKind>,
    context: &'ctx Context,
    refined: &RefinedEnv,
    structs: &StructEnv,
) -> BodyResult<ModuleAtomObligations<'ctx>> {
    // The domain the widened profile binds for this atom: declared
    // signature bounds (bool/subrange parameters), guarded-operator
    // definedness and bounded-while exits — the same `src_dom` the
    // LLVM-side equivalence binds. Inputs outside it are not
    // counterexamples, so every obligation below carries the same
    // conjuncts.
    let mut source_domain: Vec<Bool> = body
        .param_domain
        .iter()
        .chain(body.definedness.iter())
        .cloned()
        .collect();
    if let Some(exits) = &body.loop_bound_exits {
        source_domain.push(exits.clone());
    }
    let mut env: HashMap<String, SourceValue> =
        param_env(&atom.params, &body.inputs, context, refined, structs)?;
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
        call_returns,
    )?
    .boolean()?;
    env.insert(
        "result".to_string(),
        match return_kind(atom, refined) {
            ScalarKind::Bool => {
                SourceValue::Boolean(body.value._eq(&BV::from_u64(context, 0, 64)).not())
            }
            ScalarKind::I64 => SourceValue::Integer(body.value.clone()),
        },
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
        call_returns,
    )?
    .boolean()?;
    // A refined result type adds its declaration conjuncts to the
    // postcondition the atom must satisfy — `-> Port` guarantees
    // `ensures` AND `v >= 1 && v <= 65535` at the encoded result.
    // `env["result"]` already carries the kind-typed operand — an
    // `Integer` for an i64 leaf and a `Boolean` for a bool leaf — so a
    // refined result's conjuncts evaluate on the same value the
    // ensures clause does.
    let result_operand = env["result"].clone();
    let mut post = vec![ensures];
    post.extend(refined_conjuncts_at(
        atom.return_type.as_deref(),
        result_operand,
        refined,
        context,
    )?);
    let post = Bool::and(context, &post.iter().collect::<Vec<_>>());
    let mut assumptions = Vec::new();
    let mut domain_obligations = Vec::new();
    for site in &body.call_sites {
        let Some(callee) = callees.get(&site.callee) else {
            return Err(SemanticContractError::Unsupported(
                "call target outside module",
            ));
        };
        let site_env = param_env(&callee.params, &site.args, context, refined, structs)?;
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
            call_returns,
        )?
        .boolean()?;
        // A domain counterexample counts only where the callee's own
        // bounded-while encodings actually exited — same domain
        // convention the single-atom obligation applies to `requires`.
        // The callee's declared signature bounds its arguments the same
        // way the caller's `param_domain` binds its inputs, and the
        // callee's own domain — guarded-operator definedness and
        // bounded-while exits, instantiated at the call arguments —
        // bounds where its `ensures` was ever proved: a reached call
        // whose arguments fall outside that domain is a domain
        // violation, not a verified call.
        let Some(callee_body) = callee_bodies.get(&site.callee) else {
            return Err(SemanticContractError::Unsupported(
                "call target outside module",
            ));
        };
        let substitutions: Vec<(&BV, &BV)> =
            callee_body.inputs.iter().zip(site.args.iter()).collect();
        let mut callee_precondition = vec![callee_requires];
        callee_precondition.extend(signature_domain(
            &callee.params,
            &site.args,
            context,
            refined,
            structs,
        )?);
        callee_precondition.extend(
            callee_body
                .definedness
                .iter()
                .map(|defined| defined.substitute(&substitutions)),
        );
        if let Some(exits) = &callee_body.loop_bound_exits {
            callee_precondition.push(exits.substitute(&substitutions));
        }
        let callee_precondition =
            Bool::and(context, &callee_precondition.iter().collect::<Vec<_>>());
        let mut domain_terms = vec![
            requires.clone(),
            site.guard.clone(),
            callee_precondition.not(),
        ];
        domain_terms.extend(site_exits);
        domain_terms.extend(source_domain.iter().cloned());
        domain_obligations.push(Bool::and(context, &domain_terms.iter().collect::<Vec<_>>()));
        let mut ensures_env = site_env;
        ensures_env.insert(
            "result".to_string(),
            match return_kind(callee, refined) {
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
            call_returns,
        )?
        .boolean()?;
        // A refined callee result type strengthens the assumption the
        // site makes the same way it strengthens the callee's own
        // postcondition: the site result satisfies the declaration's
        // conjuncts at the call's actual result value.
        let mut callee_post = vec![callee_ensures];
        callee_post.extend(refined_conjuncts_at(
            callee.return_type.as_deref(),
            ensures_env["result"].clone(),
            refined,
            context,
        )?);
        let callee_post = Bool::and(context, &callee_post.iter().collect::<Vec<_>>());
        // The assumption binds only where the call is actually reached
        // (its guard) and the callee's ensures encoding stayed inside
        // its bounded-while domain: outside the guard the site result
        // is discarded by the caller's `ite`, so assuming the ensures
        // unconditionally would let an ensures that is unsatisfiable at
        // off-guard arguments — outside the domain obligation's reach —
        // falsify the whole formula and mask a real violation.
        let exits_domain = Bool::and(context, &ensures_site_exits.iter().collect::<Vec<_>>());
        let site_domain = Bool::and(context, &[&site.guard, &exits_domain]);
        assumptions.push(site_domain.implies(&callee_post));
    }
    let assumption = Bool::and(context, &assumptions.iter().collect::<Vec<_>>());
    let mut terms = vec![requires, Bool::and(context, &[&assumption, &post.not()])];
    for exits in requires_exits.iter().chain(ensures_exits.iter()) {
        terms.push(exits.clone());
    }
    terms.extend(source_domain.iter().cloned());
    let mut obligations = vec![Bool::and(context, &terms.iter().collect::<Vec<_>>())];
    obligations.extend(domain_obligations);
    Ok(ModuleAtomObligations {
        atom: atom.name.clone(),
        domain: Bool::and(context, &source_domain.iter().collect::<Vec<_>>()),
        obligations,
    })
}

/// The per-atom `requires ∧ ¬ensures` obligation — shared by the
/// single-atom and module encoders so both profiles keep identical
/// contract semantics.
fn atom_obligation<'ctx>(
    atom: &Atom,
    body: &PureI64Body<'ctx>,
    call_returns: &BTreeMap<String, ScalarKind>,
    context: &'ctx Context,
    refined: &RefinedEnv,
) -> BodyResult<Bool<'ctx>> {
    let mut env: HashMap<String, SourceValue> = param_env(
        &atom.params,
        &body.inputs,
        context,
        refined,
        &StructEnv::new(),
    )?;
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
        call_returns,
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
        call_returns,
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
    /// A struct parameter's flattened fields in declaration order. Only
    /// a field read or a pass-through call argument consumes it; every
    /// scalar position rejects it.
    Composite(Vec<(String, SourceValue<'ctx>)>),
}

/// Bind declared parameters to their symbolic inputs: `i64` parameters
/// are 64-bit integers, `bool` parameters the truth value `input != 0`
/// (the emitter lowers `bool` to an i64 carrying 0/1, matching its
/// `icmp ne` condition). A struct parameter consumes one input per
/// field, each bound by its field's kind the same way.
fn param_env<'ctx>(
    params: &[crate::parser::ast::Param],
    inputs: &[BV<'ctx>],
    ctx: &'ctx Context,
    refined: &RefinedEnv,
    structs: &StructEnv,
) -> BodyResult<HashMap<String, SourceValue<'ctx>>> {
    if signature_slots(params, structs).len() != inputs.len() {
        return Err(SemanticContractError::Unsupported("source signature arity"));
    }
    let scalar = |kind: ScalarKind, v: &BV<'ctx>| {
        if kind == ScalarKind::Bool {
            SourceValue::Boolean(v._eq(&BV::from_u64(ctx, 0, 64)).not())
        } else {
            SourceValue::Integer(v.clone())
        }
    };
    let mut slots = inputs.iter();
    let mut env = HashMap::new();
    for p in params {
        let value = match struct_fields(p, structs) {
            Some(fields) => SourceValue::Composite(
                fields
                    .iter()
                    .zip(slots.by_ref())
                    .map(|(f, v)| (f.name.clone(), scalar(f.kind, v)))
                    .collect(),
            ),
            None => {
                let Some(v) = slots.next() else {
                    return Err(SemanticContractError::Unsupported("source signature arity"));
                };
                scalar(param_kind(p, refined), v)
            }
        };
        env.insert(p.name.clone(), value);
    }
    Ok(env)
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
    call_returns: &BTreeMap<String, ScalarKind>,
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
            call_returns,
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
                call_returns,
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
            call_returns,
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
                            call_returns,
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
                            call_returns,
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
                            call_returns,
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
                            call_returns,
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
                call_returns,
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
    call_returns: &BTreeMap<String, ScalarKind>,
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
            call_returns,
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
            call_returns,
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
    call_returns: &BTreeMap<String, ScalarKind>,
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
        Expr::FieldAccess(target, field) => {
            let Expr::Variable(n) = target.as_ref() else {
                return Err(SemanticContractError::Unsupported("source field read"));
            };
            let Some(SourceValue::Composite(fields)) = env.get(n.as_str()) else {
                return Err(SemanticContractError::Unsupported("source field read"));
            };
            fields
                .iter()
                .find(|(name, _)| name == field)
                .map(|(_, value)| value.clone())
                .ok_or(SemanticContractError::Unsupported("source field read"))?
        }
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
            call_returns,
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
                call_returns,
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
                call_returns,
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
                call_returns,
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
                    call_returns,
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
                    call_returns,
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
                call_returns,
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
                                call_returns,
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
                    call_returns,
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
            // Call arguments carry the profile's lowered representation:
            // the extraction gate checked each argument's kind against
            // the callee parameter, and a `bool` argument lowers to the
            // same i64 0/1 bitvector a bool parameter does — so the call
            // site and `signature_domain`/`param_env` see one encoding.
            let mut applied = Vec::with_capacity(args.len());
            for arg in args {
                let lower = |value: SourceValue<'ctx>| match value {
                    I(v) => Ok(v),
                    B(b) => Ok(b.ite(&BV::from_u64(ctx, 1, 64), &BV::from_u64(ctx, 0, 64))),
                    SourceValue::Composite(_) => Err(SemanticContractError::Unsupported(
                        "source nested struct value",
                    )),
                };
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
                    call_returns,
                )? {
                    // A passed-through struct parameter flattens into its
                    // field slots — the callee's `param_env` and
                    // `signature_domain` consume the same order.
                    SourceValue::Composite(fields) => {
                        for (_, field) in fields {
                            applied.push(lower(field)?);
                        }
                    }
                    scalar => applied.push(lower(scalar)?),
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
            // The uninterpreted result is a bitvector; a callee whose
            // return kind is bool reads it under the profile's 0/1
            // convention, matching `param_env` and `env["result"]`.
            match call_returns.get(name.as_str()) {
                Some(ScalarKind::Bool) => B(value._eq(&BV::from_u64(ctx, 0, 64)).not()),
                _ => I(value),
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
        let atoms =
            encode_pure_i64_module_obligations(source, &context, RefinedAdmission::Reject).unwrap();
        let mut outcomes = Vec::new();
        for atom in &atoms {
            solver.reset();
            solver.assert(&atom.domain);
            assert_eq!(solver.check(), z3::SatResult::Sat);
            for obligation in &atom.obligations {
                solver.reset();
                solver.assert(obligation);
                outcomes.push(solver.check());
            }
        }
        outcomes
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

    #[test]
    fn module_obligations_bind_the_call_assumption_to_the_site_guard() {
        // `f`'s ensures is unsatisfiable exactly where its `requires`
        // does not hold — `x <= 0`. The call in `helper` is reached only
        // under `x > 0`, so its domain check closes; but assuming the
        // ensures unconditionally would make the whole postcondition
        // formula vacuous and hide that `helper` returns 0 for `x < 0`.
        let source = "atom f(y: i64) -> i64 requires: y > 0; ensures: result >= 0 && (y > 0 || result < 0); body: { y };\natom helper(x: i64) -> i64 requires: true; ensures: result == x; body: { if x > 0 { f(x) } else { 0 } };\natom main() -> i64 requires: true; ensures: result == 0; body: { 0 };";
        let outcomes = module_outcomes(source);
        assert!(outcomes.iter().any(|result| *result == z3::SatResult::Sat));
        // The same module with a correct postcondition stays verified —
        // the guarded assumption still binds where the call executes.
        let fixed = source.replace("ensures: result == x;", "ensures: result >= 0;");
        assert!(module_outcomes(&fixed)
            .iter()
            .all(|result| *result == z3::SatResult::Unsat));
    }

    #[test]
    fn module_obligations_keep_bool_results_boolean() {
        // A `-> bool` sibling's own ensures must see `result` as a
        // Boolean: the widened profile admits bool results for non-main
        // atoms, and `result == true` encodes only against the 0/1
        // materialization.
        let source = "atom pick(x: i64) -> bool requires: true; ensures: result == true; body: { x == x };\natom main() -> i64 requires: true; ensures: result == 0; body: { 0 };";
        let outcomes = module_outcomes(source);
        assert!(outcomes
            .iter()
            .all(|result| *result == z3::SatResult::Unsat));
    }

    #[test]
    fn module_obligations_apply_the_signature_domain() {
        // `x: u8` is checked only over [0, 255]: without the param
        // domain conjunct `x = -1` refutes a contract that holds on
        // every declared input.
        let source = "atom idu(x: u8) -> i64 requires: true; ensures: result >= 0; body: { x };\natom main() -> i64 requires: true; ensures: result >= 0; body: { idu(1) };";
        let outcomes = module_outcomes(source);
        assert!(outcomes
            .iter()
            .all(|result| *result == z3::SatResult::Unsat));
        // A call site argument outside the callee's declared domain is
        // a domain violation of its own: the callee's obligations were
        // only ever checked inside its signature bounds.
        let out_of_range = source.replace("idu(1)", "idu(300)");
        assert!(module_outcomes(&out_of_range)
            .iter()
            .any(|result| *result == z3::SatResult::Sat));
    }

    #[test]
    fn module_obligations_apply_the_bounded_while_exit_domain() {
        // For `x > LOOP_BOUND` the unrolled loop still runs at the
        // bound: the model value says nothing there, so only inputs
        // where the loop exits may refute the postcondition.
        let source = "atom count(x: i64) -> i64 requires: x >= 0; ensures: result == x; body: { let i = 0; while i < x invariant: i >= 0 decreases: x - i { i = i + 1 }; i };\natom main() -> i64 requires: true; ensures: result == 0; body: { count(0) };";
        let outcomes = module_outcomes(source);
        assert!(outcomes
            .iter()
            .all(|result| *result == z3::SatResult::Unsat));
    }

    #[test]
    fn module_obligations_apply_the_callee_loop_domain_at_call_sites() {
        // `count`'s `while` is proved only where it exits inside
        // `LOOP_BOUND` unrolled iterations — inputs past the bound sit
        // outside its verified domain, so its `ensures` was never
        // checked there. A caller reaching the call with such arguments
        // may not assume that postcondition: the site is a domain
        // violation, not a verified call.
        let source = "atom count(n: i64) -> i64 requires: n >= 0; ensures: result == n; body: { let i = 0; while i < n invariant: i >= 0 decreases: n - i { i = i + 1 }; i };\natom main() -> i64 requires: true; ensures: result == 3; body: { count(3) };";
        assert!(module_outcomes(source)
            .iter()
            .all(|result| *result == z3::SatResult::Unsat));
        let out_of_bound = source.replace("count(3)", "count(10)");
        assert!(module_outcomes(&out_of_bound)
            .iter()
            .any(|result| *result == z3::SatResult::Sat));
    }

    fn refined_module_outcomes_admission(
        source: &str,
        admission: RefinedAdmission,
    ) -> Vec<z3::SatResult> {
        let mut config = z3::Config::new();
        config.set_timeout_msec(crate::verification::VerificationConfig::default().timeout_ms);
        let context = z3::Context::new(&config);
        let solver = z3::Solver::new(&context);
        let atoms = encode_pure_i64_module_obligations(source, &context, admission).unwrap();
        let mut outcomes = Vec::new();
        for atom in &atoms {
            solver.reset();
            solver.assert(&atom.domain);
            assert_eq!(solver.check(), z3::SatResult::Sat);
            for obligation in &atom.obligations {
                solver.reset();
                solver.assert(obligation);
                outcomes.push(solver.check());
            }
        }
        outcomes
    }

    fn refined_module_outcomes(source: &str) -> Vec<z3::SatResult> {
        refined_module_outcomes_admission(source, RefinedAdmission::V1)
    }

    #[test]
    fn refined_typedef_admission_gates_the_fragment() {
        let port = "type Port = i64 where v >= 1 && v <= 65535;\n\
             atom listen(x: Port) -> Port requires: true; ensures: result == x; body: { x };\n\
             atom main() -> i64 requires: true; ensures: result == 80; body: { listen(80) };";
        // The refined profile admits the declaration and signature use.
        let contracts =
            extract_pure_i64_module_contracts(port, "m.mm", None, RefinedAdmission::V1).unwrap();
        assert_eq!(contracts.len(), 2);
        assert_eq!(contracts[0].profile, PURE_I64_REFINED_PROFILE);
        // The strict fragment keeps refusing the same source — a record
        // pinned v1 could never describe it.
        assert!(
            extract_pure_i64_module_contracts(port, "m.mm", None, RefinedAdmission::Reject)
                .is_err()
        );
        // Alias chains stay out: the verifier applies only a
        // refinement's own predicate, so a declared base would silently
        // lose the ancestor conjuncts at build time.
        let chained = "type Small = i64 where v < 10;\n\
             type Tiny = Small where v > 0;\n\
             atom t(x: Tiny) -> i64 requires: true; ensures: result == x; body: { x };\n\
             atom main() -> i64 requires: true; ensures: result == 4; body: { t(4) };";
        assert!(
            extract_pure_i64_module_contracts(chained, "m.mm", None, RefinedAdmission::V1).is_err()
        );
        // A declaration-only module of typedefs plus atoms is fine.
        let unused_alias = "type Unused = i64 where v > 0;\n\
             atom main() -> i64 requires: true; ensures: result == 0; body: { 0 };";
        assert_eq!(
            extract_pure_i64_module_contracts(unused_alias, "m.mm", None, RefinedAdmission::V1)
                .unwrap()[0]
                .profile,
            PURE_I64_REFINED_PROFILE
        );
        // Fail-closed gates: every malformed declaration stays out.
        for source in [
            // unit metadata the profile does not model
            "type Speed = i64 @unit(\"m/s\") where v > 0;\natom main() -> i64 requires: true; ensures: result == 0; body: { 0 };",
            // scalar-vocabulary shadowing
            "type i64 = bool where v;\natom main() -> i64 requires: true; ensures: result == 0; body: { 0 };",
            // duplicate declaration names
            "type P = i64 where v > 0;\ntype P = i64 where v < 10;\natom main() -> i64 requires: true; ensures: result == 0; body: { 0 };",
            // cyclic alias — a base naming another declaration is a chain
            "type A = B where v > 0;\ntype B = A where v < 10;\natom main() -> i64 requires: true; ensures: result == 0; body: { 0 };",
            // bool base — the verifier's refinement operand encodes as Int
            "type Flag = bool where v;\natom f(x: Flag) -> i64 requires: true; ensures: result == x; body: { x };\natom main() -> i64 requires: true; ensures: result == 1; body: { f(1) };",
            // non-scalar base
            "type T = f64 where v > 0;\natom main() -> i64 requires: true; ensures: result == 0; body: { 0 };",
            // undeclared base name
            "type T = Missing where v > 0;\natom main() -> i64 requires: true; ensures: result == 0; body: { 0 };",
            // type name collides with an atom name
            "type helper = i64 where v > 0;\natom helper() -> i64 requires: true; ensures: result == 0; body: { 0 };\natom main() -> i64 requires: true; ensures: result == 0; body: { helper() };",
            // undeclared signature type stays rejected under both modes
            "atom f(x: Missing) -> i64 requires: true; ensures: result == x; body: { x };",
            // non-Bool predicate
            "type P = i64 where v + 1;\natom main() -> i64 requires: true; ensures: result == 0; body: { 0 };",
            // predicate referencing a name beyond its declared operand
            "type P = i64 where v > w;\natom main() -> i64 requires: true; ensures: result == 0; body: { 0 };",
        ] {
            assert!(
                extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::V1).is_err(),
                "unexpected acceptance: {source}"
            );
        }
        // A declaration-only module has items but no atom: under the v1
        // gate nonempty implied an atom, so the widened admission must
        // refuse it rather than return vacuous contracts/obligations.
        let decls_only = "type Port = i64 where v >= 1 && v <= 65535;";
        assert!(
            extract_pure_i64_module_contracts(decls_only, "m.mm", None, RefinedAdmission::V1)
                .is_err()
        );
        let mut config = z3::Config::new();
        config.set_timeout_msec(30_000);
        let context = z3::Context::new(&config);
        assert!(
            encode_pure_i64_module_obligations(decls_only, &context, RefinedAdmission::V1).is_err()
        );
    }

    #[test]
    fn refined_obligations_fold_param_result_and_call_site_predicates() {
        // The parameter refinement is an input domain the callee may
        // assume: `p >= 1` holds inside `widen` without a `requires`.
        let domain = "type Port = i64 where v >= 1 && v <= 65535;\n\
             atom widen(p: Port) -> i64 requires: true; ensures: result >= 1; body: { p };\n\
             atom main() -> i64 requires: true; ensures: result == 80; body: { 80 };";
        assert!(refined_module_outcomes(domain)
            .iter()
            .all(|result| *result == z3::SatResult::Unsat));
        // But the domain bounds only what the predicate says: `result
        // >= 2` fails at `p = 1` — the refinement is really checked,
        // not vacuously assumed.
        let too_strong = domain.replace("result >= 1", "result >= 2");
        assert!(refined_module_outcomes(&too_strong)
            .iter()
            .any(|result| *result == z3::SatResult::Sat));
        // The result refinement is a postcondition the body must
        // satisfy: `x + 1` meets its own `ensures` but leaves Port at
        // `x = 65535`, so the obligation is refutable.
        let result_out_of_range = "type Port = i64 where v >= 1 && v <= 65535;\n\
             atom bump(x: Port) -> Port requires: true; ensures: result == x + 1; body: { x + 1 };\n\
             atom main() -> i64 requires: true; ensures: result == 2; body: { bump(1) };";
        assert!(refined_module_outcomes(result_out_of_range)
            .iter()
            .any(|result| *result == z3::SatResult::Sat));
        // And a conforming body — the `requires` bounds `x` so `x + 1`
        // stays inside Port — keeps every obligation closed.
        let in_range = "type Port = i64 where v >= 1 && v <= 65535;\n\
             atom bump(x: Port) -> Port requires: x <= 65534; ensures: result == x + 1; body: { x + 1 };\n\
             atom main() -> i64 requires: true; ensures: result == 2; body: { bump(1) };";
        assert!(refined_module_outcomes(&in_range)
            .iter()
            .all(|result| *result == z3::SatResult::Unsat));
        // A call site must establish the callee's refined domain:
        // `check(70000)` is outside Port, so a domain obligation is
        // refutable; `check(80)` closes it.
        let call_site = "type Port = i64 where v >= 1 && v <= 65535;\n\
             atom check(p: Port) -> i64 requires: true; ensures: result == p; body: { p };\n\
             atom main() -> i64 requires: true; ensures: result == 80; body: { check(80) };";
        assert!(refined_module_outcomes(call_site)
            .iter()
            .all(|result| *result == z3::SatResult::Unsat));
        let bad_call = call_site.replace("check(80)", "check(70000)");
        assert!(refined_module_outcomes(&bad_call)
            .iter()
            .any(|result| *result == z3::SatResult::Sat));
        // The callee's refined result type also strengthens what the
        // call site may assume — `make()` yields a Port, so its caller
        // can rely on `>= 1`.
        let assumed = "type Port = i64 where v >= 1 && v <= 65535;\n\
             atom make() -> Port requires: true; ensures: result == 80; body: { 80 };\n\
             atom main() -> i64 requires: true; ensures: result >= 1; body: { make() };";
        assert!(refined_module_outcomes(assumed)
            .iter()
            .all(|result| *result == z3::SatResult::Unsat));
    }

    #[test]
    fn refined_typedef_v2_admits_chains_and_bool_leafs() {
        // Under V2 a chained declaration resolves its leaf and folds
        // every declared conjunct — matching the verifier's chain
        // composition — and the module names the v2 member profile.
        let chained = "type Small = i64 where v < 10;\n\
             type Tiny = Small where v > 0;\n\
             atom t(x: Tiny) -> i64 requires: true; ensures: result == x; body: { x };\n\
             atom main() -> i64 requires: true; ensures: result == 4; body: { t(4) };";
        let contracts =
            extract_pure_i64_module_contracts(chained, "m.mm", None, RefinedAdmission::V2).unwrap();
        assert_eq!(contracts[0].profile, PURE_I64_REFINED_PROFILE_V2);
        // The folded conjuncts are checked, not just admitted:
        // `t(15)` leaves `Small`'s `v < 10`, so a call obligation is
        // refutable; the conforming call closes every obligation.
        assert!(
            refined_module_outcomes_admission(chained, RefinedAdmission::V2)
                .iter()
                .all(|result| *result == z3::SatResult::Unsat)
        );
        let bad_call = chained.replace("t(4)", "t(15)");
        assert!(
            refined_module_outcomes_admission(&bad_call, RefinedAdmission::V2)
                .iter()
                .any(|result| *result == z3::SatResult::Sat)
        );
        // The ancestor conjunct binds too — `t(0)` satisfies `v < 10`
        // but not `Tiny`'s `v > 0`.
        let zero_call = chained.replace("t(4)", "t(0)");
        assert!(
            refined_module_outcomes_admission(&zero_call, RefinedAdmission::V2)
                .iter()
                .any(|result| *result == z3::SatResult::Sat)
        );
        // A bool leaf joins the same fragment at V2 and names v2;
        // its predicate sees the Bool operand, so `f(0)` — false —
        // leaves `Flag` while `f(1)` closes the call.
        let flag = "type Flag = bool where v == true;\n\
             atom f(x: Flag) -> Flag requires: true; ensures: result == x; body: { x };\n\
             atom main() -> Flag requires: true; ensures: result == f(true); body: { f(true) };";
        let contracts =
            extract_pure_i64_module_contracts(flag, "m.mm", None, RefinedAdmission::V2).unwrap();
        assert_eq!(contracts[0].profile, PURE_I64_REFINED_PROFILE_V2);
        assert!(
            refined_module_outcomes_admission(flag, RefinedAdmission::V2)
                .iter()
                .all(|result| *result == z3::SatResult::Unsat)
        );
        let false_call = flag.replace("f(true)", "f(false)");
        assert!(
            refined_module_outcomes_admission(&false_call, RefinedAdmission::V2)
                .iter()
                .any(|result| *result == z3::SatResult::Sat)
        );
        // A bool-leaf chain folds through to Bool as well.
        let bool_chain = "type B = bool where v == true;\n\
             type C = B where v == true;\n\
             atom f(x: C) -> C requires: true; ensures: result == x; body: { x };\n\
             atom main() -> C requires: true; ensures: result == f(true); body: { f(true) };";
        let contracts =
            extract_pure_i64_module_contracts(bool_chain, "m.mm", None, RefinedAdmission::V2)
                .unwrap();
        assert_eq!(contracts[0].profile, PURE_I64_REFINED_PROFILE_V2);
        assert!(
            refined_module_outcomes_admission(bool_chain, RefinedAdmission::V2)
                .iter()
                .all(|result| *result == z3::SatResult::Unsat)
        );
        // The narrow levels keep refusing both shapes — a v1 record
        // could never describe them.
        for source in [chained, flag, bool_chain] {
            assert!(
                extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::V1)
                    .is_err()
            );
            assert!(extract_pure_i64_module_contracts(
                source,
                "m.mm",
                None,
                RefinedAdmission::Reject
            )
            .is_err());
        }
        // Fail-closed gates that v2 does not lift: cycles, unresolvable
        // or non-scalar leaves, and predicates over undeclared names.
        for source in [
            "type A = B where v > 0;\ntype B = A where v < 10;\natom main() -> i64 requires: true; ensures: result == 0; body: { 0 };",
            "type T = Missing where v > 0;\natom main() -> i64 requires: true; ensures: result == 0; body: { 0 };",
            "type T = f64 where v > 0;\natom main() -> i64 requires: true; ensures: result == 0; body: { 0 };",
            "type T = u64 where v > 0;\natom main() -> i64 requires: true; ensures: result == 0; body: { 0 };",
        ] {
            assert!(
                extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::V2).is_err(),
                "unexpected acceptance: {source}"
            );
        }
    }

    const MAIN0: &str = "atom main() -> i64 requires: true; ensures: result == 0; body: { 0 };";

    fn composite(source: &str) -> Result<Vec<SemanticContract>, SemanticContractError> {
        extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::Composite)
    }

    fn composite_outcomes(source: &str) -> Vec<z3::SatResult> {
        refined_module_outcomes_admission(source, RefinedAdmission::Composite)
    }

    #[test]
    fn composite_admission_reads_struct_fields_in_every_contract_position() {
        let source = format!(
            "struct Point {{ x: i64, y: i64 }}\n\
             atom gx(p: Point) -> i64 requires: p.x >= 0 && p.y >= 0; ensures: result == p.x + p.y; body: {{ p.x + p.y }};\n{MAIN0}"
        );
        let contracts = composite(&source).unwrap();
        assert!(contracts
            .iter()
            .all(|c| c.profile == PURE_I64_COMPOSITE_PROFILE));
        assert_eq!(
            module_source_profile(&source, RefinedAdmission::Composite).unwrap(),
            PURE_I64_COMPOSITE_PROFILE
        );
        // The struct parameter reports one scalar binder per field —
        // never a scalar binder for the struct value itself.
        let binders: Vec<_> = contracts[0]
            .specification
            .binders
            .iter()
            .map(|b| (b.name.as_str(), b.scalar_type))
            .collect();
        assert_eq!(binders, [("p.x", "i64"), ("p.y", "i64"), ("result", "i64")]);
        assert!(contracts[0]
            .backend_requirements
            .contains(&"struct_parameters_flattened_to_scalar_fields"));
        assert!(!contracts[1]
            .backend_requirements
            .contains(&"struct_parameters_flattened_to_scalar_fields"));
        assert!(composite_outcomes(&source)
            .iter()
            .all(|result| *result == z3::SatResult::Unsat));
        // A false postcondition over the fields is refuted, not vacuous.
        let wrong = source.replace("result == p.x + p.y", "result == p.x");
        assert!(composite_outcomes(&wrong).contains(&z3::SatResult::Sat));
        // Narrower admission levels keep `struct` out of the vocabulary.
        for admission in [
            RefinedAdmission::Reject,
            RefinedAdmission::V1,
            RefinedAdmission::V2,
        ] {
            assert!(extract_pure_i64_module_contracts(&source, "m.mm", None, admission).is_err());
            assert!(module_source_profile(&source, admission).is_err());
        }
    }

    #[test]
    fn composite_admission_keeps_minimal_profile_names_without_structs() {
        let plain = format!(
            "atom id(x: i64) -> i64 requires: true; ensures: result == x; body: {{ x }};\n{MAIN0}"
        );
        assert_eq!(
            composite(&plain).unwrap()[0].profile,
            PURE_I64_WRAPPING_PROFILE
        );
        let refined = format!("type Port = i64 where v >= 1;\n{plain}");
        assert_eq!(
            composite(&refined).unwrap()[0].profile,
            PURE_I64_REFINED_PROFILE
        );
        let chained = format!("type A = i64 where v >= 0;\ntype B = A where v <= 9;\n{plain}");
        assert_eq!(
            composite(&chained).unwrap()[0].profile,
            PURE_I64_REFINED_PROFILE_V2
        );
        // A declared-but-unused struct still widens the module's
        // admitted fragment, so the record names it.
        let declared = format!("struct P {{ x: i64 }}\n{plain}");
        assert_eq!(
            composite(&declared).unwrap()[0].profile,
            PURE_I64_COMPOSITE_PROFILE
        );
        // Non-struct specification hashes are unchanged by the
        // composite admission.
        let v2 =
            extract_pure_i64_module_contracts(&plain, "m.mm", None, RefinedAdmission::V2).unwrap();
        assert_eq!(
            composite(&plain).unwrap()[0].specification_sha256,
            v2[0].specification_sha256
        );
    }

    #[test]
    fn composite_specification_identity_binds_the_field_layout() {
        let atom = "atom gx(p: P) -> i64 requires: true; ensures: result == p.x; body: { p.x };";
        let wide = format!("struct P {{ x: i64, y: i64 }}\n{atom}\n{MAIN0}");
        let narrow = format!("struct P {{ x: i32, y: i64 }}\n{atom}\n{MAIN0}");
        let swapped = format!("struct P {{ y: i64, x: i64 }}\n{atom}\n{MAIN0}");
        let wide = composite(&wide).unwrap();
        let narrow = composite(&narrow).unwrap();
        let swapped = composite(&swapped).unwrap();
        assert_ne!(wide[0].specification_sha256, narrow[0].specification_sha256);
        assert_ne!(
            wide[0].specification_sha256,
            swapped[0].specification_sha256
        );
        assert_eq!(narrow[0].specification.binders[0].scalar_type, "i32");
    }

    #[test]
    fn composite_fields_carry_their_declared_scalar_domains() {
        // `bool`, subrange and refined-alias fields bound their flattened
        // slot exactly like a scalar parameter of that type.
        let source = format!(
            "type Port = i64 where v >= 1 && v <= 65535;\n\
             struct S {{ on: bool, b: u8, port: Port }}\n\
             atom pick(s: S) -> i64 requires: true; ensures: result >= 0 && result <= 65535; body: {{ if s.on {{ s.port }} else {{ s.b }} }};\n{MAIN0}"
        );
        let contracts = composite(&source).unwrap();
        let binders: Vec<_> = contracts[0]
            .specification
            .binders
            .iter()
            .map(|b| (b.name.as_str(), b.scalar_type))
            .collect();
        assert_eq!(
            binders,
            [
                ("s.on", "bool"),
                ("s.b", "u8"),
                ("s.port", "i64"),
                ("result", "i64")
            ]
        );
        assert!(composite_outcomes(&source)
            .iter()
            .all(|result| *result == z3::SatResult::Unsat));
        // Outside the alias predicate the bound fails: the domain is
        // what makes the postcondition hold, not a vacuous encoding.
        let tight = source.replace("result <= 65535", "result <= 255");
        assert!(composite_outcomes(&tight).contains(&z3::SatResult::Sat));
        let context = z3::Context::new(&z3::Config::new());
        let bodies =
            encode_pure_i64_module(&source, &context, true, RefinedAdmission::Composite).unwrap();
        assert_eq!(bodies[0].inputs.len(), 3);
        assert!(bodies[0].composite_signature);
        assert!(bodies[0].refined_signature);
        assert!(bodies[0].subrange_params);
        assert!(!bodies[1].composite_signature);
    }

    #[test]
    fn composite_parameters_pass_through_to_same_struct_callees() {
        let source = format!(
            "struct P {{ x: i64, y: i64 }}\n\
             atom f(p: P) -> i64 requires: p.x > 0; ensures: result == p.x; body: {{ p.x }};\n\
             atom g(q: P, k: i64) -> i64 requires: q.x > 0; ensures: result == q.x + k; body: {{ f(q) + k }};\n{MAIN0}"
        );
        composite(&source).unwrap();
        assert!(composite_outcomes(&source)
            .iter()
            .all(|result| *result == z3::SatResult::Unsat));
        // The caller must establish the callee's precondition over the
        // passed fields: a weaker caller precondition is a domain
        // violation the call-site obligation refutes.
        let weak = source.replace("requires: q.x > 0", "requires: q.x >= 0");
        assert!(composite_outcomes(&weak).contains(&z3::SatResult::Sat));
        // The callee's refined field domain is part of what the call
        // site must establish too — the shared struct keeps it.
        let refined = format!(
            "type Pos = i64 where v > 0;\n\
             struct P {{ x: Pos }}\n\
             atom f(p: P) -> i64 requires: true; ensures: result > 0; body: {{ p.x }};\n\
             atom g(q: P) -> i64 requires: true; ensures: result > 0; body: {{ f(q) }};\n{MAIN0}"
        );
        assert!(composite_outcomes(&refined)
            .iter()
            .all(|result| *result == z3::SatResult::Unsat));
        // A `bool` field flattens to the i64 0/1 carrier on both sides of
        // the call, so a forwarded flag keeps its truth value.
        let flag = format!(
            "struct S {{ on: bool, x: i64 }}\n\
             atom f(s: S) -> bool requires: true; ensures: result == s.on; body: {{ s.on }};\n\
             atom g(t: S) -> bool requires: true; ensures: result == t.on; body: {{ f(t) }};\n{MAIN0}"
        );
        assert!(composite_outcomes(&flag)
            .iter()
            .all(|result| *result == z3::SatResult::Unsat));
        let negated = flag.replace("ensures: result == t.on", "ensures: result != t.on");
        assert!(composite_outcomes(&negated).contains(&z3::SatResult::Sat));
    }

    #[test]
    fn composite_admission_rejects_struct_declarations_outside_the_fragment() {
        let atom = "atom gx(p: P) -> i64 requires: true; ensures: result == 0; body: { 0 };";
        for decl in [
            // generic, invariant, methods
            "struct P<T> { x: T }",
            "struct P { x: i64, y: i64, invariant: self.x <= self.y }",
            "struct P { x: i64 }\nimpl P { atom get(self: P) -> i64 requires: true; ensures: result == self.x; body: self.x; }",
            // field constraints, non-scalar and struct-valued fields
            "struct P { x: i64 where v >= 0 }",
            "struct P { x: f64 }",
            "struct P { x: u64 }",
            "struct P { x: usize }",
            "struct P { x: Missing }",
            "struct Q { x: i64 }\nstruct P { q: Q }",
            "struct P { next: P }",
            "struct P { xs: [i64] }",
            // empty, duplicate field, duplicate struct
            "struct P { }",
            "struct P { x: i64, x: i64 }",
            "struct P { x: i64 }\nstruct P { y: i64 }",
            // name collisions with scalar names, types and atoms
            "struct P { x: i64 }\nstruct i64 { x: i64 }",
            "struct P { x: i64 }\nstruct bool { x: i64 }",
            "struct P { x: i64 }\nstruct u64 { x: i64 }",
            "type P = i64 where v > 0;\nstruct P { x: i64 }",
            "struct P { x: i64 }\nstruct gx { x: i64 }",
            "struct P { x: i64 }\nstruct main { x: i64 }",
        ] {
            let source = format!("{decl}\n{atom}\n{MAIN0}");
            assert!(composite(&source).is_err(), "unexpected acceptance: {decl}");
        }
    }

    #[test]
    fn composite_admission_rejects_struct_values_outside_reads_and_pass_through() {
        let decl = "struct P { x: i64, y: i64 }\nstruct Q { x: i64, y: i64 }";
        let f = "atom f(p: P) -> i64 requires: true; ensures: result == p.x; body: { p.x };";
        for atom in [
            // undeclared struct parameter type and struct results
            "atom h(p: R) -> i64 requires: true; ensures: result == 0; body: { 0 };",
            "atom h(p: P) -> P requires: true; ensures: true; body: { p };",
            // struct values in scalar positions
            "atom h(p: P) -> i64 requires: true; ensures: result == 0; body: { p };",
            "atom h(p: P) -> i64 requires: true; ensures: result == 0; body: { p + 1 };",
            "atom h(p: P) -> i64 requires: p == p; ensures: result == 0; body: { 0 };",
            "atom h(p: P) -> i64 requires: true; ensures: result == 0; body: { let q = p; 0 };",
            "atom h(p: P) -> i64 requires: true; ensures: result == 0; body: { if p { 0 } else { 1 } };",
            // unknown fields, nested and computed targets, result reads
            "atom h(p: P) -> i64 requires: true; ensures: result == p.z; body: { 0 };",
            "atom h(p: P) -> i64 requires: true; ensures: result == p.x.y; body: { 0 };",
            "atom h(x: i64) -> i64 requires: true; ensures: result == x.y; body: { 0 };",
            "atom h(p: P) -> i64 requires: true; ensures: result.x == 0; body: { 0 };",
            "atom h(p: P) -> i64 requires: true; ensures: result == f(p).x; body: { 0 };",
            // a scalar binding shadows the struct parameter
            "atom h(p: P) -> i64 requires: true; ensures: true; body: { let p = 1; p.x };",
            "atom h(p: P) -> i64 requires: true; ensures: true; body: { match 0 { p => p.x } };",
            // struct parameters are not rebindable
            "atom h(p: P) -> i64 requires: true; ensures: true; body: { p = p; 0 };",
            "atom h(p: P) -> i64 requires: true; ensures: true; body: { p.x = 1; 0 };",
            // struct construction
            "atom h(p: P) -> i64 requires: true; ensures: result == 0; body: { f(P { x: 0, y: 0 }) };",
            "atom h(p: P) -> i64 requires: true; ensures: result == 0; body: { let q = P { x: 0, y: 0 }; 0 };",
            // pass-through shape mismatches
            "atom h(q: Q) -> i64 requires: true; ensures: true; body: { f(q) };",
            "atom h(x: i64) -> i64 requires: true; ensures: true; body: { f(x) };",
            "atom h(p: P) -> i64 requires: true; ensures: true; body: { f(p.x) };",
            "atom h(p: P) -> i64 requires: true; ensures: true; body: { f(1) };",
            "atom h(p: P) -> i64 requires: true; ensures: true; body: { let p = 1; f(p) };",
            "atom h(p: P) -> i64 requires: true; ensures: true; body: { f(p, p) };",
            // a struct parameter passed where a scalar is expected
            "atom k(x: i64) -> i64 requires: true; ensures: true; body: { x };\natom h(p: P) -> i64 requires: true; ensures: true; body: { k(p) };",
            // borrowed or consumed struct parameters
            "atom h(ref p: P) -> i64 requires: true; ensures: true; body: { p.x };",
            "atom h(consume p: P) -> i64 requires: true; ensures: true; body: { p.x };",
        ] {
            let source = format!("{decl}\n{f}\n{atom}\n{MAIN0}");
            assert!(composite(&source).is_err(), "unexpected acceptance: {atom}");
        }
        // The same decl/callee pair is admitted on its own, so each
        // rejection above is the probed shape, not the surrounding text.
        composite(&format!("{decl}\n{f}\n{MAIN0}")).unwrap();
    }
}
