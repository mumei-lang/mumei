//! An opt-in, deliberately small description of one scalar source obligation.
//! Extraction is not verification or translation validation.

use crate::hir::lower_atom_to_hir;
use crate::mir::lower_hir_to_mir;
use crate::parser::{
    parse_body_expr_checked, parse_expression_checked, parse_module_checked, Atom, Expr, Item,
    MatchArm, Op, Pattern, Span, Stmt, TrustLevel,
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
    ast::{Array, Ast, Bool, BV},
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
/// v2 widens the composite fragment's field vocabulary to refined-alias
/// field types whose chain resolves to an `i64` or `bool` leaf: the
/// verifier now assumes a field's alias predicates for a struct
/// parameter the same way it does for a scalar parameter of that type,
/// so the flattened field slot's alias domain is one both sides bind. A
/// module whose fields are all literal `i64`/`bool` keeps the v1 name.
pub const PURE_I64_COMPOSITE_PROFILE_V2: &str = "mumei.pure-i64-composite/v2";
/// v3 widens the composite fragment from struct parameters to struct
/// values: atoms returning an admitted struct (`result.f` binds the
/// returned field slot), `StructInit` of an admitted struct from scalar
/// field expressions, struct-valued `let`s and field reads on a call's
/// struct result. A struct value is still never a scalar, never merged
/// across `if`/`match` branches and never rebound. A module that uses
/// none of those shapes keeps its v1/v2 name.
pub const PURE_I64_COMPOSITE_PROFILE_V3: &str = "mumei.pure-i64-composite/v3";
/// v4 adds `enum` declarations to the composite fragment: a
/// non-generic, non-recursive enum lowers to the tagged-union layout
/// the emitter emits — an i64 variant ordinal plus one i64 payload
/// slot per index up to the widest variant — so an enum parameter is
/// read by a `match` whose arms bind the payload slots the variant
/// carries. Enum construction, enum results and enum `let`s beyond
/// pass-through stay outside (`pure-i64-composite/v4`).
pub const PURE_I64_COMPOSITE_PROFILE_V4: &str = "mumei.pure-i64-composite/v4";
/// v5 admits field assignment on struct values: `q.f = v` where `q` is
/// a bound admitted struct binding (parameter or struct `let`) and `f`
/// a declared leaf field. The write re-establishes the field's
/// refined-type predicates and `where` constraint plus every struct
/// invariant, so a write that would break them is rejected rather than
/// assumed. Nested field paths, writes on enum bindings and writes
/// through branch-merged values stay outside
/// (`pure-i64-composite/v5`).
pub const PURE_I64_COMPOSITE_PROFILE_V5: &str = "mumei.pure-i64-composite/v5";
/// v6 admits producing enum values: construction `E::V(..)` and the
/// nullary `E::V`, enum results `-> E` (a tag plus the widest
/// variant's payload slots, the layout the emitter's tagged union
/// returns) and calls to enum-returning atoms. An enum value still
/// never merges across `if`/`match` branches and never takes a field
/// write (`pure-i64-composite/v6`).
pub const PURE_I64_COMPOSITE_PROFILE_V6: &str = "mumei.pure-i64-composite/v6";
/// v7 admits branch-merged composite values: `if`/`match` expressions
/// whose arms all produce the same struct or enum type merge the
/// flattened slots under `ite` — the aggregate `phi` the emitter
/// writes — and composite bindings written inside a branch merge the
/// same way. Merged arms must produce one type; mixed-type merges stay
/// outside (`pure-i64-composite/v7`).
pub const PURE_I64_COMPOSITE_PROFILE_V7: &str = "mumei.pure-i64-composite/v7";
/// v8 admits struct-valued fields: a non-generic, non-recursive
/// struct nested inside another, flattening to leaf slots in
/// declaration order (`o.inner.a`). Nested-path writes stay outside —
/// the parser does not even produce `q.inner.a = v` — and generic or
/// recursive declarations fail closed (`pure-i64-composite/v8`).
pub const PURE_I64_COMPOSITE_PROFILE_V8: &str = "mumei.pure-i64-composite/v8";
/// v9 admits generic `struct` declarations: each concrete
/// instantiation used in the module monomorphizes to an ordinary
/// non-generic declaration before admission, so instantiated fields
/// face the same leaf, nesting, recursion and shape gates a
/// hand-written declaration would. Generic `enum` or `atom`
/// declarations stay rejected, and instantiation is bounded so a
/// self-deepening type argument (`B<A<T>>`) cannot expand forever
/// (`pure-i64-composite/v9`).
pub const PURE_I64_COMPOSITE_PROFILE_V9: &str = "mumei.pure-i64-composite/v9";
/// v10 admits `u64` leaves — parameters, results, struct fields and
/// instantiated type arguments — under unsigned two's-complement
/// semantics: unsigned comparisons (`bvult`/`bvule`/`bvugt`/`bvuge`),
/// `bvudiv`/`bvurem` with only a nonzero-divisor obligation, `bvlshr`,
/// and no signed interpretation anywhere. Literals adapt to the
/// unsigned context without a suffix; mixing a `u64` operand with a
/// declared `i64` operand in one binary expression stays outside
/// (`pure-i64-composite/v10`).
pub const PURE_I64_COMPOSITE_PROFILE_V10: &str = "mumei.pure-i64-composite/v10";

/// v11 admits `f64` leaves as pass-through slots only — parameters,
/// results, struct fields and instantiated type arguments carry one
/// 64-bit slot holding the IEEE-754 bit pattern. The leaf may be
/// stored, copied, returned and merged across branches, but no
/// arithmetic or comparison inside the fragment reads it — the same
/// opaque treatment call-result slots already get, and the
/// signature-domain layer applies no bound since every bit pattern is
/// a valid carrier (`pure-i64-composite/v11`).
pub const PURE_I64_COMPOSITE_PROFILE_V11: &str = "mumei.pure-i64-composite/v11";
/// `Str` literal leaves (`pure-i64-composite/v13`): a `let`-bound
/// string literal is a bounded inline byte sequence — up to
/// `STR_BOUND` cells under the bounded-memory model, immutable,
/// `s[i]` reads a `u64` byte and `len(s)` its cell count
/// (`pure-i64-composite/v13`).
pub const PURE_I64_COMPOSITE_PROFILE_V13: &str = "mumei.pure-i64-composite/v13";
/// Enum scalar operations (`pure-i64-composite/v14`): equality `e ==
/// e'` between two values of one declared enum compares the whole
/// tagged-union layout — tag plus every payload slot — the same
/// positional `extractvalue`/`icmp` chain the emitter lowers it to
/// (`pure-i64-composite/v14`). Ordering, arithmetic and `.tag`
/// projections stay outside: the emitter emits no IR for them.
pub const PURE_I64_COMPOSITE_PROFILE_V14: &str = "mumei.pure-i64-composite/v14";

/// v15 admits field writes through a nested field path —
/// `q.inner.a = v` writes the leaf inside `q`'s `inner` subtree and
/// `q.inner = s` keeps its whole-subtree meaning; the write updates
/// the same flattened leaf slots the IR `insertvalue` chain touches
/// (`pure-i64-composite/v15`).
pub const PURE_I64_COMPOSITE_PROFILE_V15: &str = "mumei.pure-i64-composite/v15";
/// `pure-i64-composite/v16`: enum payload writes `e.p = v` — a named
/// payload field of the variant the binding's tag provably carries.
pub const PURE_I64_COMPOSITE_PROFILE_V16: &str = "mumei.pure-i64-composite/v16";

/// v17 admits enum-typed struct fields — a field `e: Color` flattens
/// to a tag plus payload leaf slots exactly like an enum parameter,
/// reads as a whole enum value (`match b.e`, `b.e.tag`, `b.e.p`),
/// and admits only whole-field writes `b.e = e2`
/// (`pure-i64-composite/v17`).
pub const PURE_I64_COMPOSITE_PROFILE_V17: &str = "mumei.pure-i64-composite/v17";
pub const PURE_I64_COMPOSITE_PROFILE_V18: &str = "mumei.pure-i64-composite/v18";

/// `pure-i64-composite/v19`: real `f64` arithmetic — the admitted
/// operators `+`, `-`, `*`, `/` and `==` on `f64` leaves decode the
/// bit-pattern slots as IEEE 754 binary64 and compute under
/// round-nearest-ties-to-even, the same semantics the emitted
/// `fadd`/`fsub`/`fmul`/`fdiv`/`fcmp oeq` carry. Float literals stay
/// rejected and `!=`/ordering remain outside the emitter surface.
pub const PURE_I64_COMPOSITE_PROFILE_V19: &str = "mumei.pure-i64-composite/v19";

/// `pure-i64-composite/v20`: a per-atom declared `Str` bound — the
/// `str_bound: N;` clause raises the `Str` literal cap from the
/// default `STR_BOUND` to the declared bound (at most
/// `STR_BOUND_MAX`). The inline byte-cell model scales linearly in
/// the literal's length, so the bound only gates admission.
pub const PURE_I64_COMPOSITE_PROFILE_V20: &str = "mumei.pure-i64-composite/v20";

/// `pure-i64-composite/v21`: `i128`/`u128` scalar leaves — a 128-bit
/// leaf carries two consecutive 64-bit slots in the flattened
/// signature, and the bundle record's `leaf_widths` declares the
/// pairing so a recheck can group the slots back.
pub const PURE_I64_COMPOSITE_PROFILE_V21: &str = "mumei.pure-i64-composite/v21";

/// `pure-i64-composite/v22`: `i128`/`u128` leaves inside enum payload
/// positions — `enum F { A(i128), B(i128) }` or `A(P), B(P)` with `P`
/// carrying a wide leaf. A wide payload leaf carries two consecutive
/// 64-bit slots under the same carriers convention a scalar payload
/// leaf uses. Payload positions typed differently across variants
/// (`A(i128), B(i64)`) stay rejected: a mixed-type union slot needs
/// tag-conditional slot types, which the flattened signature does not
/// express.
pub const PURE_I64_COMPOSITE_PROFILE_V22: &str = "mumei.pure-i64-composite/v22";

/// Maximum byte length a `Str` literal may declare under
/// `pure-i64-composite/v13`: one cell per byte under the
/// bounded-memory model.
pub const STR_BOUND: usize = 64;

/// Largest `str_bound: N;` an atom may declare — the inline cell
/// model stays bounded even for a declared bound.
pub const STR_BOUND_MAX: usize = 1024;

/// Bounded stack memory (`pure-i64-composite-bounded-memory/v1`): local
/// fixed-size array literals, `a[i]` reads, `a[i] = v` writes and `len`
/// — encoded as one SMT `BV64 -> BV64` array per binding, the model the
/// `bounded-memory` LLVM translation-validation profiles check against
/// the emitted `alloca`/GEP/`store`/`load` shapes. Out-of-bounds
/// accesses degrade exactly like the emitted code: reads yield `0`,
/// writes are skipped — no definedness obligation is recorded.
pub const PURE_I64_COMPOSITE_MEMORY_PROFILE: &str = "mumei.pure-i64-composite-bounded-memory/v1";

/// How much of the refined-scalar declaration fragment a gate admits:
/// `Reject` keeps `Item::TypeDef` out of the item vocabulary entirely,
/// `V1` admits single-level declarations over the i64-family scalar
/// vocabulary only (the `pure-i64-refined-scalar/v1` fragment), and `V2`
/// additionally admits chained declarations and `bool` leaves (the v2
/// fragment). `Composite` keeps the V2 declaration fragment and adds
/// all-scalar `struct` declarations as module members
/// (`pure-i64-composite/v1`); `CompositeV2` additionally admits
/// refined-alias field types (`pure-i64-composite/v2`); `CompositeV3`
/// additionally admits struct values — results, `StructInit`, struct
/// `let`s and call-result field reads (`pure-i64-composite/v3`);
/// `CompositeV4` additionally admits `enum` declarations, enum-typed
/// parameters and `match` arms on them (`pure-i64-composite/v4`);
/// `CompositeV5` additionally admits field assignment `q.f = v` on
/// admitted struct bindings (`pure-i64-composite/v5`);
/// `CompositeV6` additionally admits enum construction, enum results
/// and enum-returning calls (`pure-i64-composite/v6`);
/// `CompositeV7` additionally admits branch-merged composite values —
/// `if`/`match` expressions producing struct or enum values
/// (`pure-i64-composite/v7`). `CompositeV8` additionally admits
/// struct-valued fields — a non-generic, non-recursive struct nested
/// inside another (`pure-i64-composite/v8`). `CompositeV9`
/// additionally admits generic `struct` declarations whose concrete
/// instantiations monomorphize to the admitted fragment
/// (`pure-i64-composite/v9`). `CompositeV10` additionally admits `u64`
/// scalar leaves — parameters, results, fields and instantiated type
/// arguments carrying unsigned two's-complement semantics
/// (`pure-i64-composite/v10`). `CompositeV14` additionally admits
/// `e == e'`/`e != e'` equality between two values of one declared
/// enum (`pure-i64-composite/v14`). `CompositeV17` additionally
/// admits enum-typed struct fields — a declared enum nested inside a
/// struct flattens to the same tag + payload slots an enum parameter
/// carries, so `b.e` reads a whole enum value and `b.e = e2` writes
/// one (`pure-i64-composite/v17`). V16 is reserved for the
/// enum-payload-write profile (`e.p = v`) landing in parallel. A
/// pinned record names which level produced it, so a
/// rechecker re-derives under the record's own admission — never a
/// wider one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefinedAdmission {
    Reject,
    V1,
    V2,
    Composite,
    CompositeV2,
    CompositeV3,
    CompositeV4,
    CompositeV5,
    CompositeV6,
    CompositeV7,
    CompositeV8,
    CompositeV9,
    CompositeV10,
    CompositeV11,
    CompositeV12,
    CompositeV13,
    CompositeV14,
    CompositeV15,
    CompositeV16,
    CompositeV17,
    CompositeV18,
    CompositeV19,
    /// `CompositeV20` additionally admits a per-atom declared `Str`
    /// bound (`str_bound: N;`) — the literal cap becomes the declared
    /// bound instead of the default `STR_BOUND`
    /// (`pure-i64-composite/v20`).
    CompositeV20,
    /// `CompositeV21` additionally admits `i128`/`u128` leaves — each
    /// carries two consecutive 64-bit slots in the flattened
    /// signature and the record's `leaf_widths` declares the pairing
    /// (`pure-i64-composite/v21`).
    CompositeV21,
    /// `CompositeV22` additionally admits `i128`/`u128` leaves inside
    /// enum payload positions — directly or inside a payload struct
    /// (`pure-i64-composite/v22`).
    CompositeV22,
}

impl RefinedAdmission {
    /// Whether any `Item::TypeDef` declaration is in the item
    /// vocabulary at all.
    pub fn admits_typedefs(self) -> bool {
        self != Self::Reject
    }

    /// Whether `Item::StructDef` declarations are in the item vocabulary.
    pub fn admits_structs(self) -> bool {
        self >= Self::Composite
    }

    /// Whether struct values (results, `StructInit`, struct `let`s and
    /// call-result field reads) are in the expression vocabulary.
    pub fn admits_struct_values(self) -> bool {
        self >= Self::CompositeV3
    }

    /// Whether `Item::EnumDef` declarations and enum values
    /// (parameters, `match` scrutinees, pass-through calls and `let`s)
    /// are in the vocabulary.
    pub fn admits_enums(self) -> bool {
        self >= Self::CompositeV4
    }

    /// Whether field assignment `q.f = v` on an admitted struct binding
    /// is in the statement vocabulary.
    pub fn admits_field_writes(self) -> bool {
        self >= Self::CompositeV5
    }

    /// Whether enum values can be produced — construction `E::V(..)`,
    /// enum results and enum-returning calls
    /// (`pure-i64-composite/v6`).
    pub fn admits_enum_values(self) -> bool {
        self >= Self::CompositeV6
    }

    /// Whether composite values may merge across `if`/`match` branches
    /// (`pure-i64-composite/v7`).
    pub fn admits_merged_composites(self) -> bool {
        self >= Self::CompositeV7
    }

    /// Whether a struct field may itself be struct-typed — a
    /// non-generic, non-recursive nested struct
    /// (`pure-i64-composite/v8`).
    pub fn admits_nested_structs(self) -> bool {
        self >= Self::CompositeV8
    }

    /// Whether generic `struct` declarations — instantiated by
    /// monomorphization before the profile gates run
    /// (`pure-i64-composite/v9`).
    pub fn admits_generic_structs(self) -> bool {
        self >= Self::CompositeV9
    }

    /// Whether `u64` scalar leaves with unsigned semantics
    /// (`pure-i64-composite/v10`).
    pub fn admits_u64_leaves(self) -> bool {
        self >= Self::CompositeV10
    }

    /// Whether bounded stack memory — local array literals, indexed
    /// reads/writes and `len` — is admitted
    /// (`pure-i64-composite-bounded-memory/v1`).
    pub fn admits_bounded_memory(self) -> bool {
        self >= Self::CompositeV11
    }

    /// Whether `f64` scalar leaves are admitted pass-through — stored,
    /// copied, returned and merged, never read by an arithmetic or
    /// comparison operator (`pure-i64-composite/v11`).
    pub fn admits_f64_leaves(self) -> bool {
        self >= Self::CompositeV12
    }

    /// Whether `Str` literals are admitted as bounded inline byte
    /// sequences — one cell per byte under the bounded-memory model,
    /// immutable, `s[i]` reads a `u64` byte (`pure-i64-composite/v13`).
    pub fn admits_str_leaves(self) -> bool {
        self >= Self::CompositeV13
    }

    /// Whether `e == e'`/`e != e'` on two values of one declared enum
    /// is admitted — the whole tagged-union layout compares
    /// positionally, the shape the emitter lowers it to
    /// (`pure-i64-composite/v14`).
    pub fn admits_enum_scalar_ops(self) -> bool {
        self >= Self::CompositeV14
    }

    /// Whether a field write may traverse into a nested struct field
    /// — `q.inner.a = v` or a whole-subtree write at depth
    /// (`pure-i64-composite/v15`).
    pub fn admits_nested_writes(self) -> bool {
        self >= Self::CompositeV15
    }

    /// Whether `e.p = v` may write a named payload field of an enum
    /// binding (`pure-i64-composite/v16`) — the variant owning the
    /// field must be unique and the binding's tag provably `V`.
    pub fn admits_enum_writes(self) -> bool {
        self >= Self::CompositeV16
    }

    /// Whether a struct field may be enum-typed — it flattens to a tag
    /// plus payload leaf slots exactly like an enum parameter, reads
    /// as a whole enum value, and admits only whole-field writes
    /// `b.e = e2` (`pure-i64-composite/v17`).
    pub fn admits_enum_struct_fields(self) -> bool {
        self >= Self::CompositeV17
    }

    /// Whether an enum payload field may be struct-typed — the slot
    /// keeps one entry per declared field whose leaves expand into the
    /// flattened signature (`pure-i64-composite/v18`). Enum-typed
    /// payloads stay closed: a struct carrying an enum leaf is
    /// rejected as a payload, which also bounds the layout against
    /// struct↔enum cycles.
    pub fn admits_composite_payloads(self) -> bool {
        self >= Self::CompositeV18
    }

    /// Whether source `f64` arithmetic (`+ - * / ==` on `f64` leaves)
    /// is admitted with real IEEE 754 semantics
    /// (`pure-i64-composite/v19`).
    pub fn admits_f64_arithmetic(self) -> bool {
        self >= Self::CompositeV19
    }

    /// Whether the admission covers a per-atom declared `Str` bound
    /// (`str_bound: N;` on the atom). Only `CompositeV20` admits it.
    pub fn admits_str_bound(self) -> bool {
        self >= Self::CompositeV20
    }

    /// Whether `i128`/`u128` scalar leaves are admitted — each carries
    /// two consecutive 64-bit slots (`pure-i64-composite/v21`).
    pub fn admits_wide_leaves(self) -> bool {
        self >= Self::CompositeV21
    }

    /// Whether `i128`/`u128` leaves are admitted inside enum payload
    /// positions — a wide payload leaf carries two consecutive 64-bit
    /// slots under its carriers (`pure-i64-composite/v22`).
    pub fn admits_wide_payloads(self) -> bool {
        self >= Self::CompositeV22
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
    let (items, refined, structs, _, _, _, _, _, _, _, _, _, _, _, _, _, _, _, _) =
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
        &EnumEnv::new(),
        false,
        false,
        false,
        false,
        false,
        false,
        false,
        false,
        false,
        false,
        false,
        false,
        false,
        false,
        false,
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
    let (
        items,
        refined,
        structs,
        enums,
        struct_values,
        field_writes,
        enum_values,
        merged,
        generic,
        u64_leaves,
        f64_leaves,
        arrays,
        strs,
        enum_scalar_ops,
        nested_writes,
        enum_writes,
        f64_arith,
        str_bound_declared,
        wide_leaves,
    ) = admit_pure_i64_atoms(source, true, admission)?;
    // The translator-IR pin must see the same type environment the
    // verifier's certificate does — a `Port`-typed binder carries its
    // refinement annotation only when the alias is registered, and a
    // struct-typed binder resolves only against registered structs.
    let mut cert_env = ModuleEnv::default();
    for item in &items {
        match item {
            Item::TypeDef(typedef) => cert_env.register_type(typedef),
            Item::StructDef(struct_def) => cert_env.register_struct(struct_def),
            Item::EnumDef(enum_def) => cert_env.register_enum(enum_def),
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
                &enums,
                struct_values,
                field_writes,
                enum_values,
                merged,
                generic,
                u64_leaves,
                f64_leaves,
                arrays,
                strs,
                enum_scalar_ops,
                nested_writes,
                enum_writes,
                f64_arith,
                str_bound_declared,
                wide_leaves,
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
    let (
        _items,
        refined,
        structs,
        enums,
        struct_values,
        field_writes,
        enum_values,
        merged,
        generic,
        u64_leaves,
        f64_leaves,
        arrays,
        strs,
        enum_scalar_ops,
        nested_writes,
        enum_writes,
        f64_arithmetic,
        str_bound_declared,
        wide_leaves,
    ) = admit_pure_i64_atoms(source, true, admission)?;
    Ok(fragment_profile(
        true,
        &refined,
        &structs,
        &enums,
        struct_values,
        field_writes,
        enum_values,
        merged,
        generic,
        u64_leaves,
        f64_leaves,
        arrays,
        strs,
        enum_scalar_ops,
        nested_writes,
        enum_writes,
        f64_arithmetic,
        str_bound_declared,
        wide_leaves,
    ))
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
/// What `admit_pure_i64_atoms` returns: the admitted items, the refined
/// type environment, the struct and enum declaration environments, and
/// the four usage flags (`struct_values`, `field_writes`,
/// `enum_values`, `merged_composites`) the module's profile name
/// derives from.
type AdmittedModule = (
    Vec<Item>,
    RefinedEnv,
    StructEnv,
    EnumEnv,
    bool,
    bool,
    bool,
    bool,
    bool,
    bool,
    bool,
    bool,
    bool,
    bool,
    bool,
    bool,
    bool,
    bool,
    bool,
);

/// Every atom's declared leaf widths in signature order — `64` per
/// scalar or enum leaf, `128` per `i128`/`u128` leaf
/// (`pure-i64-composite/v21`). A v24 bundle record pins this pairing
/// (`leaf_widths`) so a rechecker re-derives it under the record's own
/// admission instead of trusting the producer.
pub fn module_leaf_widths(
    source: &str,
    admission: RefinedAdmission,
) -> Result<std::collections::BTreeMap<String, Vec<u64>>, SemanticContractError> {
    let (items, _refined, structs, enums, ..) = admit_pure_i64_atoms(source, true, admission)?;
    let mut widths = std::collections::BTreeMap::new();
    for item in items {
        if let Item::Atom(atom) = item {
            widths.insert(
                atom.name.clone(),
                signature_slots(&atom.params, &structs, &enums)
                    .iter()
                    .map(|slot| match slot {
                        SignatureSlot::WideScalar(_) | SignatureSlot::EnumWideField { .. } => 128,
                        _ => 64,
                    })
                    .collect::<Vec<u64>>(),
            );
        }
    }
    Ok(widths)
}

fn admit_pure_i64_atoms(
    source: &str,
    allow_wrapping_arithmetic: bool,
    admission: RefinedAdmission,
) -> Result<AdmittedModule, SemanticContractError> {
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
    // v9: generic `struct` declarations are admitted by instantiation —
    // every concrete use is monomorphized into an ordinary declaration
    // before the profile gates see it. Generic atoms and enums stay
    // closed: there is no contract- or variant-instantiation model for
    // them, so they are rejected up front either way.
    for item in &items {
        match item {
            Item::Atom(atom) if !atom.type_params.is_empty() => {
                return Err(SemanticContractError::Unsupported("generic atoms"));
            }
            Item::EnumDef(edef) if !edef.type_params.is_empty() => {
                return Err(SemanticContractError::Unsupported("generic enums"));
            }
            _ => {}
        }
    }
    let generic_structs_declared = items
        .iter()
        .any(|item| matches!(item, Item::StructDef(def) if !def.type_params.is_empty()));
    let mut items = items;
    if generic_structs_declared {
        if !admission.admits_generic_structs() {
            return Err(SemanticContractError::Unsupported(
                "generic struct declarations",
            ));
        }
        items = monomorphize_generic_structs(items)?;
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
            Item::EnumDef(_) if admission.admits_enums() => (),
            _ => {
                return Err(SemanticContractError::Unsupported("non-atom declarations"));
            }
        }
    }
    // Refined declarations share the module's name space with atoms: a
    // type named like an atom would make `T(` ambiguous between a type
    // position and a call, so the admission is closed over both maps.
    let refined = build_refined_env(&items, admission)?;
    let structs = build_struct_env(&items, &refined, admission)?;
    let enums = build_enum_env(&items, &refined, &structs, admission)?;
    if refined
        .keys()
        .chain(structs.keys())
        .chain(enums.keys())
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
                        .map(|p| param_shape(p, &refined, &structs, &enums))
                        .collect(),
                    return_kind: return_shape(sibling, &refined, &structs, &enums),
                },
            ))
        })
        .collect();
    // Declaration order is irrelevant to call resolution: the callee map is
    // complete before any atom's own contract is checked, so a forward call
    // to a later sibling resolves the same as a backward one.
    let struct_values_used = std::cell::Cell::new(false);
    let field_writes_used = std::cell::Cell::new(false);
    let enum_values_used = std::cell::Cell::new(false);
    let merged_composites_used = std::cell::Cell::new(false);
    let arrays_used = std::cell::Cell::new(false);
    let strs_used = std::cell::Cell::new(false);
    let enum_scalar_ops_used = std::cell::Cell::new(false);
    let nested_writes_used = std::cell::Cell::new(false);
    let enum_writes_used = std::cell::Cell::new(false);
    let f64_arithmetic_used = std::cell::Cell::new(false);
    for item in &items {
        let Item::Atom(sibling) = item else {
            continue;
        };
        validate_atom(
            sibling,
            allow_wrapping_arithmetic,
            admission.admits_u64_leaves(),
            admission.admits_f64_leaves(),
            &refined,
            &structs,
            &enums,
            admission.admits_struct_values(),
            admission.admits_enum_values(),
            admission.admits_str_bound(),
            admission.admits_wide_leaves(),
        )?;
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
            &enums,
            admission.admits_struct_values(),
            admission.admits_field_writes(),
            admission.admits_enum_values(),
            admission.admits_merged_composites(),
            admission.admits_bounded_memory(),
            admission.admits_str_leaves(),
            admission.admits_enum_scalar_ops(),
            admission.admits_nested_writes(),
            admission.admits_enum_writes(),
            admission.admits_f64_arithmetic(),
            &struct_values_used,
            &field_writes_used,
            &enum_values_used,
            &merged_composites_used,
            &arrays_used,
            &strs_used,
            &enum_scalar_ops_used,
            &nested_writes_used,
            &enum_writes_used,
            &f64_arithmetic_used,
        )?;
    }
    let u64_leaves = uses_u64_leaves(&items);
    let wide_leaves = uses_wide_leaves(&items);
    let f64_leaves = uses_f64_leaves(&items);
    // A declared `str_bound` pins v20 module-wide, the same
    // declaration-presence convention `has_composite_payloads`
    // follows: consumers read the bound from the atom clause.
    let str_bound_declared = items
        .iter()
        .any(|item| matches!(item, Item::Atom(atom) if atom.str_bound.is_some()));
    Ok((
        items,
        refined,
        structs,
        enums,
        struct_values_used.get(),
        field_writes_used.get(),
        enum_values_used.get(),
        merged_composites_used.get(),
        generic_structs_declared,
        u64_leaves,
        f64_leaves,
        arrays_used.get(),
        strs_used.get(),
        enum_scalar_ops_used.get(),
        nested_writes_used.get(),
        enum_writes_used.get(),
        f64_arithmetic_used.get(),
        str_bound_declared,
        wide_leaves,
    ))
}

/// Instantiate every generic `struct` declaration a module uses, as a
/// fixpoint: a generated instantiation can itself name still-new
/// instances (`struct Box<T> { p: Pair<T> }`). The instance bound keeps
/// `L<T> { n: L<L<T>> }`-style expansion from running away. Only call
/// sites under `RefinedAdmission::CompositeV9` reach this — callers
/// below it reject a generic declaration before instantiating.
pub fn monomorphize_generic_structs(items: Vec<Item>) -> Result<Vec<Item>, SemanticContractError> {
    let mut seen = BTreeSet::new();
    for item in &items {
        if let Item::StructDef(def) = item {
            if !def.type_params.is_empty() && !seen.insert(def.name.clone()) {
                return Err(SemanticContractError::Unsupported(
                    "duplicate generic struct name",
                ));
            }
        }
    }
    const MAX_GENERIC_STRUCT_INSTANCES: usize = 64;
    // An instance's display name encodes its whole type-argument tree, so
    // `L<T> { n: L<(T,T)> }`-style recursion doubles the name each round:
    // bound its length as well as the instance count, or the fixpoint
    // exhausts memory before reaching the count bound.
    const MAX_GENERIC_INSTANCE_NAME_LEN: usize = 512;
    let mut mono = crate::ast::Monomorphizer::new();
    let mut items = items;
    let mut produced = std::collections::HashSet::new();
    loop {
        mono.collect(&items);
        if mono.instances().len() > MAX_GENERIC_STRUCT_INSTANCES {
            return Err(SemanticContractError::Unsupported(
                "generic struct instantiation bound exceeded",
            ));
        }
        if mono
            .instances()
            .iter()
            .any(|name| name.len() > MAX_GENERIC_INSTANCE_NAME_LEN)
        {
            return Err(SemanticContractError::Unsupported(
                "generic struct instantiation type too large",
            ));
        }
        // `monomorphize` re-appends every collected instance each
        // call, so previously generated declarations are stripped
        // from the input first — termination is driven by the
        // instance set reaching a fixpoint, not by item count.
        let fresh: Vec<String> = mono
            .instances()
            .iter()
            .filter(|name| !produced.contains(*name))
            .cloned()
            .collect();
        if fresh.is_empty() {
            break;
        }
        let rest: Vec<Item> = items
            .into_iter()
            .filter(|item| !matches!(item, Item::StructDef(def) if produced.contains(&def.name)))
            .collect();
        items = mono.monomorphize(&rest, None);
        produced.extend(fresh);
    }
    if !mono.body_parse_failures.is_empty() {
        return Err(SemanticContractError::Unsupported(
            "generic struct body could not be monomorphized",
        ));
    }
    Ok(items)
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
        Some("u64") => "u64",
        Some("usize") => "usize",
        Some("isize") => "isize",
        Some("f64") => "f64",
        Some(name) => refined.get(name).map(|alias| alias.base).unwrap_or("i64"),
        None => "i64",
    }
}

/// The fragment profile a module's contracts name: the bounded C03
/// fragment, the widened wrapping fragment, its refined-scalar
/// extension when the module admitted any `type` declarations, or the
/// composite extension when it admitted any `struct` declarations —
/// its v3 name only when some atom uses a struct value (a struct
/// result, literal, `let` or call-result read).
#[allow(clippy::too_many_arguments)]
/// Whether the admitted module declares a `u64`-family leaf anywhere —
/// `u64` itself, `usize` or `isize`, as a parameter, a result, a struct
/// field (instantiated decls included, since monomorphization leaves
/// the field name behind) or an enum payload slot. Only `CompositeV10`
/// admission ever produces one.
fn uses_u64_leaves(items: &[Item]) -> bool {
    let target_width = |name: Option<&str>| matches!(name, Some("u64" | "usize" | "isize"));
    items.iter().any(|item| match item {
        Item::Atom(atom) => {
            atom.params
                .iter()
                .any(|p| target_width(p.type_name.as_deref()))
                || target_width(atom.return_type.as_deref())
        }
        Item::StructDef(def) => def
            .fields
            .iter()
            .any(|f| target_width(Some(f.type_name.as_str()))),
        Item::EnumDef(def) => def
            .variants
            .iter()
            .flat_map(|v| v.field_types.iter())
            .any(|ty| target_width(Some(ty.display_name().as_str()))),
        _ => false,
    })
}

/// Whether the admitted module declares a 128-bit leaf anywhere — a
/// parameter, a result, a struct field or an enum payload slot. Only
/// `CompositeV21` admission ever produces one.
fn uses_wide_leaves(items: &[Item]) -> bool {
    let wide = |name: Option<&str>| matches!(name, Some("i128" | "u128"));
    items.iter().any(|item| match item {
        Item::Atom(atom) => {
            atom.params.iter().any(|p| wide(p.type_name.as_deref()))
                || wide(atom.return_type.as_deref())
        }
        Item::StructDef(def) => def.fields.iter().any(|f| wide(Some(f.type_name.as_str()))),
        Item::EnumDef(def) => def
            .variants
            .iter()
            .flat_map(|v| v.field_types.iter())
            .any(|ty| wide(Some(ty.display_name().as_str()))),
        _ => false,
    })
}

/// Whether the admitted module declares an `f64` leaf anywhere — a
/// parameter, a result, a struct field or an enum payload slot. Only
/// `CompositeV12` admission ever produces one.
fn uses_f64_leaves(items: &[Item]) -> bool {
    items.iter().any(|item| match item {
        Item::Atom(atom) => {
            atom.params
                .iter()
                .any(|p| p.type_name.as_deref() == Some("f64"))
                || atom.return_type.as_deref() == Some("f64")
        }
        Item::StructDef(def) => def.fields.iter().any(|f| f.type_name == "f64"),
        Item::EnumDef(def) => def
            .variants
            .iter()
            .flat_map(|v| v.field_types.iter())
            .any(|ty| ty.display_name() == "f64"),
        _ => false,
    })
}

#[allow(clippy::too_many_arguments)]
fn fragment_profile(
    allow_wrapping_arithmetic: bool,
    refined: &RefinedEnv,
    structs: &StructEnv,
    enums: &EnumEnv,
    struct_values: bool,
    field_writes: bool,
    enum_values: bool,
    merged_composites: bool,
    generic_structs: bool,
    u64_leaves: bool,
    f64_leaves: bool,
    bounded_memory: bool,
    str_leaves: bool,
    enum_scalar_ops: bool,
    nested_writes: bool,
    enum_writes: bool,
    f64_arithmetic: bool,
    str_bound_declared: bool,
    wide_leaves: bool,
) -> &'static str {
    if !allow_wrapping_arithmetic {
        PURE_I64_PROFILE
    } else if has_wide_payload_leaves(enums, structs) {
        // A 128-bit leaf inside an enum payload is only ever admitted
        // under v22 — the strongest claim the ladder makes.
        PURE_I64_COMPOSITE_PROFILE_V22
    } else if wide_leaves {
        // A 128-bit leaf is only ever admitted under v21 — the
        // strongest claim the ladder makes.
        PURE_I64_COMPOSITE_PROFILE_V21
    } else if str_bound_declared {
        // A declared `str_bound` clause is only ever admitted under
        // v20 — the strongest claim the ladder makes.
        PURE_I64_COMPOSITE_PROFILE_V20
    } else if f64_arithmetic {
        // Real `f64` arithmetic is only ever admitted under v19 — the
        // IEEE decode/compute/re-encode it requires is the strongest
        // claim the ladder makes.
        PURE_I64_COMPOSITE_PROFILE_V19
    } else if has_composite_payloads(enums, structs) {
        // A composite-typed enum payload is only ever admitted under
        // v18 — a declared enum carrying one pins it even when no
        // atom constructs or matches the variant (same convention as
        // `has_nested_fields`).
        PURE_I64_COMPOSITE_PROFILE_V18
    } else if has_enum_fields(structs) {
        // An enum-typed struct field is only ever admitted under v17,
        // so a module containing one pins it — even when no atom
        // reads the field (same convention as `has_nested_fields`).
        PURE_I64_COMPOSITE_PROFILE_V17
    } else if enum_writes {
        // An enum payload write is only ever admitted under v16 — the
        // tag-gated slot update it requires is the deepest grammar the
        // ladder admits.
        PURE_I64_COMPOSITE_PROFILE_V16
    } else if nested_writes {
        // A nested-path write is only ever admitted under v15 — the
        // deepest grammar the ladder admits.
        PURE_I64_COMPOSITE_PROFILE_V15
    } else if enum_scalar_ops {
        // Enum equality is only ever admitted under v14 — the
        // positional-layout comparison it requires is the strongest
        // claim the ladder makes.
        PURE_I64_COMPOSITE_PROFILE_V14
    } else if str_leaves {
        // A `Str` literal is only ever admitted under v13 — the
        // bounded byte-cell encoding it requires is the strongest
        // claim the ladder makes.
        PURE_I64_COMPOSITE_PROFILE_V13
    } else if f64_leaves {
        // An `f64` leaf is only ever admitted under v12 — the
        // pass-through slot convention is the strongest claim the
        // ladder makes.
        PURE_I64_COMPOSITE_PROFILE_V11
    } else if bounded_memory {
        // Bounded memory is only ever admitted under CompositeV11 — it
        // outranks every earlier feature's profile name.
        PURE_I64_COMPOSITE_MEMORY_PROFILE
    } else if u64_leaves {
        // A `u64` leaf is only ever admitted under v10 — the unsigned
        // encoding it requires is the strongest claim the ladder makes.
        PURE_I64_COMPOSITE_PROFILE_V10
    } else if generic_structs {
        // A generic declaration is only ever admitted under v9: the
        // flag was set before monomorphization, so the instantiated
        // module pins v9 even when no instance used a lower feature.
        PURE_I64_COMPOSITE_PROFILE_V9
    } else if has_nested_fields(structs) {
        // A nested field is only ever admitted under v8, so a module
        // containing one pins it — even when no atom reads the field.
        PURE_I64_COMPOSITE_PROFILE_V8
    } else if merged_composites {
        PURE_I64_COMPOSITE_PROFILE_V7
    } else if enum_values {
        PURE_I64_COMPOSITE_PROFILE_V6
    } else if field_writes {
        PURE_I64_COMPOSITE_PROFILE_V5
    } else if !enums.is_empty() {
        PURE_I64_COMPOSITE_PROFILE_V4
    } else if struct_values {
        PURE_I64_COMPOSITE_PROFILE_V3
    } else if structs
        .values()
        .flatten()
        .any(|field| refined.contains_key(&field.type_name))
    {
        PURE_I64_COMPOSITE_PROFILE_V2
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

/// IEEE 754 binary64 arithmetic on bit-pattern leaves
/// (`pure-i64-composite/v19`). Both the source operator and the emitted
/// float instruction share this helper: the leaf's 64-bit slot decodes
/// to `Float`, computes under round-nearest-ties-to-even and re-encodes,
/// so the checked value is the hardware `f64` result — NaN payloads and
/// signed zero included. `fdiv` is total (x/0.0 yields ±inf), so no
/// definedness obligation is recorded.
/// The four IEEE 754 binary64 arithmetic operators
/// `pure-i64-composite/v19` admits — the same set the emitter lowers
/// to `fadd`/`fsub`/`fmul`/`fdiv`.
#[derive(Clone, Copy)]
pub enum F64Arith {
    Add,
    Sub,
    Mul,
    Div,
}

pub fn f64_arith<'ctx>(ctx: &'ctx Context, a: &BV<'ctx>, b: &BV<'ctx>, op: F64Arith) -> BV<'ctx> {
    use crate::verification::translator::z3_types::{
        float_from_ieee_bv, ieee_bv_from_float, round_nearest_even,
    };
    let rne = round_nearest_even(ctx);
    let fa = float_from_ieee_bv(ctx, a);
    let fb = float_from_ieee_bv(ctx, b);
    let result = match op {
        F64Arith::Add => rne.add(&fa, &fb),
        F64Arith::Sub => rne.sub(&fa, &fb),
        F64Arith::Mul => rne.mul(&fa, &fb),
        F64Arith::Div => rne.div(&fa, &fb),
    };
    ieee_bv_from_float(ctx, &result)
}

/// IEEE 754 binary64 equality (`fp.eq`) on bit-pattern leaves — NaN
/// compares unequal to everything including itself and +0.0 equals
/// -0.0, the predicate the emitted `fcmp oeq` tests
/// (`pure-i64-composite/v19`).
pub fn f64_eq<'ctx>(ctx: &'ctx Context, a: &BV<'ctx>, b: &BV<'ctx>) -> Bool<'ctx> {
    use crate::verification::translator::z3_types::{float_eq, float_from_ieee_bv};
    let fa = float_from_ieee_bv(ctx, a);
    let fb = float_from_ieee_bv(ctx, b);
    float_eq(ctx, &fa, &fb)
}

/// Whether a 64-bit bit-pattern leaf carries an IEEE 754 binary64 NaN —
/// the `fpa.isNaN` predicate on the decoded value.
pub fn f64_is_nan<'ctx>(ctx: &'ctx Context, a: &BV<'ctx>) -> Bool<'ctx> {
    use crate::verification::translator::z3_types::float_from_ieee_bv;
    let raw = crate::verification::translator::z3_types::raw_z3_context(ctx);
    let fa = float_from_ieee_bv(ctx, a);
    let ast = unsafe { z3_sys::Z3_mk_fpa_is_nan(raw, fa.get_z3_ast()) };
    unsafe { Bool::wrap(ctx, ast) }
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
    enums: &EnumEnv,
    struct_values: bool,
    field_writes: bool,
    enum_values: bool,
    merged_composites: bool,
    generic_structs: bool,
    u64_leaves: bool,
    f64_leaves: bool,
    bounded_memory: bool,
    str_leaves: bool,
    enum_scalar_ops: bool,
    nested_writes: bool,
    enum_writes: bool,
    f64_arithmetic: bool,
    str_bound_declared: bool,
    wide_leaves: bool,
    cert_env: &ModuleEnv,
) -> Result<SemanticContract, SemanticContractError> {
    atom.span.file = source_name.to_owned();
    let profile = fragment_profile(
        allow_wrapping_arithmetic,
        refined,
        structs,
        enums,
        struct_values,
        field_writes,
        enum_values,
        merged_composites,
        generic_structs,
        u64_leaves,
        f64_leaves,
        bounded_memory || str_leaves,
        str_leaves,
        enum_scalar_ops,
        nested_writes,
        enum_writes,
        f64_arithmetic,
        str_bound_declared,
        wide_leaves,
    );
    let result_fields = atom
        .return_type
        .as_deref()
        .and_then(|name| structs.get(name));

    // A struct parameter reports one scalar binder per field (`p.x`),
    // an enum parameter a tag binder (`p.tag`) plus one per payload
    // slot (`p.{i}`) — the flat expansion the obligation encodes —
    // never a scalar binder for the composite value itself.
    let mut binders: Vec<_> = atom
        .params
        .iter()
        .flat_map(|p| match struct_fields(p, structs) {
            Some(fields) => flattened_struct_fields(structs, enums, fields)
                .iter()
                .map(|(path, slot)| (format!("{}.{}", p.name, path), slot_type_name(slot)))
                .collect::<Vec<_>>(),
            None => match enum_variants(p, enums) {
                Some(variants) => {
                    let mut names = vec![(format!("{}.tag", p.name), None)];
                    names.extend(
                        enum_leaf_paths(variants, structs, enums)
                            .iter()
                            .map(|(path, type_name)| (format!("{}.{}", p.name, path), *type_name)),
                    );
                    names
                }
                None => vec![(p.name.clone(), p.type_name.as_deref())],
            },
        })
        .map(|(name, type_name)| TypedBinder {
            name,
            scalar_type: binder_scalar_type(type_name, refined),
            role: "parameter",
            provenance: atom.span.clone(),
            span_precision: "atom",
        })
        .collect();
    // A struct result reports one result binder per field (`result.x`),
    // an enum result a tag binder (`result.tag`) plus one per payload
    // slot (`result.{i}`) — the flat expansion the obligation encodes.
    let result_binders = match result_fields {
        Some(fields) => flattened_struct_fields(structs, enums, fields)
            .iter()
            .map(|(path, slot)| (format!("result.{}", path), slot_type_name(slot)))
            .collect(),
        None => match atom.return_type.as_deref().and_then(|name| enums.get(name)) {
            Some(variants) => {
                let mut names = vec![("result.tag".to_owned(), None)];
                names.extend(
                    enum_leaf_paths(variants, structs, enums)
                        .iter()
                        .map(|(path, type_name)| (format!("result.{path}"), *type_name)),
                );
                names
            }
            None => vec![("result".to_owned(), atom.return_type.as_deref())],
        },
    };
    binders.extend(
        result_binders
            .into_iter()
            .map(|(name, type_name)| TypedBinder {
                name,
                scalar_type: binder_scalar_type(type_name, refined),
                role: "result",
                provenance: atom.span.clone(),
                span_precision: "atom",
            }),
    );
    let specification = ScalarSpecification {
        binders,
        requires: atom.requires.clone(),
        ensures: atom.ensures.clone(),
    };
    // Provenance is separate from the logical identity; file relocation does not
    // change the obligation, while the full source digest still binds its text.
    let mut specification_identity = serde_json::json!({
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
                None => match enum_variants(p, enums) {
                    Some(variants) => serde_json::json!({
                        "name": p.name,
                        "type": p.type_name,
                        "variants": variants
                            .iter()
                            .map(|v| serde_json::json!({
                                "name": v.name,
                                "fields": v.fields
                                    .iter()
                                    .map(|f| serde_json::json!({"name": f.name, "type": f.type_name}))
                                    .collect::<Vec<_>>(),
                            }))
                            .collect::<Vec<_>>(),
                    }),
                    None => serde_json::json!({"name": p.name, "type": p.type_name}),
                },
            })
            .collect::<Vec<_>>(),
        "return_type": atom.return_type,
        "requires": atom.requires, "ensures": atom.ensures,
    });
    // A struct result's identity includes the returned field layout the
    // same way a struct parameter's does; scalar results keep their
    // original shape so existing specification hashes are unchanged.
    if let Some(fields) = result_fields {
        specification_identity["return_fields"] = fields
            .iter()
            .map(|f| serde_json::json!({"name": f.name, "type": f.type_name}))
            .collect();
    }
    // Admitted enum declarations bind the specification the same way
    // struct declarations do — reordering variants changes every tag.
    // The key only appears when a declaration exists so specification
    // hashes for non-enum sources keep their earlier shape.
    if !enums.is_empty() {
        specification_identity["enums"] = enums
            .iter()
            .map(|(name, variants)| {
                serde_json::json!({
                    "name": name,
                    "variants": variants.iter().map(|v| {
                        serde_json::json!({
                            "name": v.name,
                            "fields": v.fields.iter().map(|f| {
                                serde_json::json!({"name": f.name, "type": f.type_name})
                            }).collect::<Vec<_>>(),
                        })
                    }).collect::<Vec<_>>(),
                })
            })
            .collect();
    }
    let specification_sha256 = json_digest(&specification_identity)?;
    let composite_params = atom
        .params
        .iter()
        .any(|p| struct_fields(p, structs).is_some() || enum_variants(p, enums).is_some());
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
            source_integer: if u64_leaves {
                "signed_i64_and_unsigned_u64_leaves"
            } else {
                "signed_i64"
            },
            // `u64` leaves encode as `BV(64)` in every mode, so the
            // verifier's integer model is only partly mathematical once
            // they are admitted — the metadata must distinguish the
            // machine-word leaves from the `Int` ones.
            verifier_integer: if u64_leaves {
                "mathematical_integer_for_i64_bitvector_64_for_u64"
            } else {
                "mathematical_integer"
            },
            llvm_integer: "i64_twos_complement",
            comparisons: if u64_leaves {
                "signed_for_i64_unsigned_for_u64"
            } else {
                "signed"
            },
            arithmetic: if allow_wrapping_arithmetic {
                "wrapping_i64_add_sub_mul_no_overflow_claim"
            } else {
                "unsupported_no_overflow_claim"
            },
            division: if u64_leaves {
                "signed_i64_truncating_defined_only_for_nonzero_divisor_excluding_min_div_neg1_and_unsigned_u64_defined_only_for_nonzero_divisor"
            } else if allow_wrapping_arithmetic {
                "signed_i64_truncating_defined_only_for_nonzero_divisor_excluding_min_div_neg1"
            } else {
                "unsupported"
            },
            shifts: if u64_leaves {
                "shl_ashr_for_i64_lshr_for_u64_defined_only_for_amounts_0_to_63"
            } else if allow_wrapping_arithmetic {
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
        .chain(u64_leaves.then_some("unsigned_u64_leaves_bitvector_total"))
        .chain(f64_leaves.then_some("f64_leaves_pass_through_bit_pattern_no_arithmetic"))
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
        .chain(
            result_fields
                .is_some()
                .then_some("struct_results_flattened_to_scalar_fields"),
        )
        .collect(),
        requested_route: "standard_z3_not_executed_by_extractor",
        decomposition: "no_common_obligation_decomposition",
        normalization:
            "checked_parser_comparison_chains_to_conjunction_not_independently_validated",
        correspondence: "unchecked_not_translation_validated",
        assumptions: vec![if u64_leaves {
            "Inputs are representable signed 64-bit integers or declared unsigned 64-bit leaves; declared booleans carry 0/1."
        } else if allow_wrapping_arithmetic {
            "Inputs are representable signed 64-bit integers; declared booleans carry 0/1."
        } else {
            "Inputs are representable signed 64-bit integers."
        }],
        exclusions: vec![
            "Extraction does not prove the postcondition or precondition satisfiability.",
            "HIR/MIR fingerprints identify internal snapshots, not semantic equivalence.",
            "Target availability, LLVM lowering and final machine code are not verified.",
            if u64_leaves {
                "Unsigned division and logical right shift are admitted for `u64`/`usize`/`isize` leaves only (`usize` folds to `u64`, `isize` to `i64` on the 64-bit target); `i128`, heap, FFI, ownership, effects and concurrency are outside this profile. Arithmetic wraps mod 2^64; overflow is not flagged or proven. Division, remainder and shifts bind inputs only where defined. Calls are call-site comparisons only: the callee's own contract is a separate obligation. `while` loops are claimed only where every guard fails within the profile bound (4 evaluations); deeper executions are outside the compared domain."
            } else if allow_wrapping_arithmetic {
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
    return_kind: ValueKind,
}

/// The shape a declared parameter carries: a profile scalar, or a
/// struct of the module's admitted struct map, which is never a scalar
/// value — it only exposes field reads and passes through to a sibling
/// parameter of the same struct.
#[derive(Clone, PartialEq, Eq)]
enum ValueKind {
    Scalar(ScalarKind),
    Struct(String),
    /// An admitted enum: a tag ordinal plus shared payload slots —
    /// never a scalar value, read by `match` arms only.
    Enum(String),
}

/// The kind-checking environment shared by every expression position of
/// one atom: the module's callee map, the atom's struct-typed parameters
/// (parameter name → struct name) and the module's struct map.
#[derive(Clone)]
struct KindEnv<'a> {
    callees: &'a BTreeMap<String, CalleeSig>,
    composites: BTreeMap<String, &'a str>,
    structs: &'a StructEnv,
    /// The module's admitted enums: a composite name resolving to an
    /// enum name is a tag+payload value a `match` scrutinee or
    /// pass-through call consumes.
    enums: &'a EnumEnv,
    /// The names the verifier and emitter synthesize for a struct
    /// parameter's fields (`p_x`, `__struct_p_x`). Both resolve `p.x`
    /// by looking those names up before the struct value itself, so a
    /// binder spelled like one would silently stand in for the field.
    reserved: BTreeSet<String>,
    /// Every identifier token of the atom's body and parameter list —
    /// a struct-valued `let` checks its synthesized field names against
    /// all of them, bound before or after it, in or out of scope.
    identifiers: BTreeSet<String>,
    /// Whether struct values (`pure-i64-composite/v3`) are admitted.
    admit_struct_values: bool,
    /// Whether field assignment `q.f = v` on a struct binding is
    /// admitted (`pure-i64-composite/v5`).
    admit_field_writes: bool,
    /// Whether enum values (`E::V(..)` construction, `-> E` results,
    /// enum-returning calls) are admitted (`pure-i64-composite/v6`).
    admit_enum_values: bool,
    /// Whether `if`/`match` expressions producing composite values are
    /// admitted (`pure-i64-composite/v7`).
    admit_merged_composites: bool,
    /// The scalar kind the implicit `result` name carries — the atom's
    /// declared return kind, so a `u64` result keeps unsigned
    /// semantics in `ensures`.
    result_kind: ScalarKind,
    /// Set when the atom actually uses a struct-value shape, so the
    /// module names the v3 profile only when it needs it.
    struct_values_used: &'a std::cell::Cell<bool>,
    /// Set when the atom writes a struct field, so the module names
    /// the v5 profile only when it needs it.
    field_writes_used: &'a std::cell::Cell<bool>,
    /// Set when the atom produces an enum value, so the module names
    /// the v6 profile only when it needs it.
    enum_values_used: &'a std::cell::Cell<bool>,
    /// Set when the atom merges a composite value across branches, so
    /// the module names the v7 profile only when it needs it.
    merged_composites_used: &'a std::cell::Cell<bool>,
    /// Whether bounded stack memory — array literals, `a[i]` reads,
    /// `a[i] = v` writes and `len` — is admitted
    /// (`pure-i64-composite-bounded-memory/v1`).
    admit_bounded_memory: bool,
    /// Set when the atom uses any bounded-memory vocabulary, so the
    /// module names the bounded-memory profile only when it needs it.
    arrays_used: &'a std::cell::Cell<bool>,
    /// Whether `Str` literal leaves are admitted
    /// (`pure-i64-composite/v13`).
    admit_str: bool,
    /// Set when the atom binds or reads a `Str` literal, so the module
    /// names the v13 profile only when it needs it.
    strs_used: &'a std::cell::Cell<bool>,
    /// Whether `e == e'`/`e != e'` on two values of one declared enum
    /// is admitted (`pure-i64-composite/v14`).
    admit_enum_scalar_ops: bool,
    /// Set when the atom compares enum values, so the module names
    /// the v14 profile only when it needs it.
    enum_scalar_ops_used: &'a std::cell::Cell<bool>,
    /// Whether field writes may traverse nested field paths
    /// (`pure-i64-composite/v15`).
    admit_nested_writes: bool,
    /// Set when the atom writes through a nested field path, so the
    /// module names the v15 profile only when it needs it.
    nested_writes_used: &'a std::cell::Cell<bool>,
    /// Whether `e.p = v` may write a named payload field of an enum
    /// binding (`pure-i64-composite/v16`).
    admit_enum_writes: bool,
    /// Set when the atom writes an enum payload field, so the module
    /// names the v16 profile only when it needs it.
    enum_writes_used: &'a std::cell::Cell<bool>,
    /// Whether `f64` arithmetic (`+ - * / ==` on `f64` leaves) is
    /// admitted (`pure-i64-composite/v19`).
    admit_f64_arithmetic: bool,
    /// Set when the atom applies an `f64` operator, so the module
    /// names the v19 profile only when it needs it.
    f64_arithmetic_used: &'a std::cell::Cell<bool>,
    /// The atom's effective `Str` literal bound — `str_bound: N;`
    /// when declared, else `STR_BOUND`.
    str_bound: usize,
}

impl<'a> KindEnv<'a> {
    /// The struct an unshadowed struct-valued name (a struct parameter or
    /// struct `let`) carries, unless a scalar binding of the same name
    /// (a `let` or a match-arm binder) shadows it on the current path.
    fn composite_name(&self, name: &str, scope: &BTreeMap<String, ScalarKind>) -> Option<&'a str> {
        if scope.contains_key(name) {
            return None;
        }
        self.composites.get(name).copied()
    }

    /// Bind a struct-valued `let`: its synthesized field names
    /// (`q_x`, `__struct_q_x`) must not be spelled anywhere in the atom —
    /// the emitter resolves `q.x` through them first — and become
    /// reserved for every later binder. An enum-valued `let` has no
    /// synthesized names (only `match` reads it), so it binds and
    /// records the composite directly.
    fn bind_composite(
        &mut self,
        name: &str,
        type_name: &'a str,
    ) -> Result<(), SemanticContractError> {
        self.bind(name)?;
        if let Some(fields) = self.structs.get(type_name) {
            let paths = enum_path_map(self.enums, self.structs);
            for field in all_field_paths(self.structs, &paths, fields) {
                for synthesized in [
                    format!("{name}_{}", field.replace('.', "_")),
                    format!("__struct_{name}_{}", field.replace('.', "_")),
                ] {
                    if self.identifiers.contains(&synthesized) {
                        return Err(SemanticContractError::Unsupported(
                            "binder collides with a struct field name",
                        ));
                    }
                    self.reserved.insert(synthesized);
                }
            }
            self.struct_values_used.set(true);
        }
        // Enum-typed bindings do not mark `enum_values_used`: passing an
        // enum through `let`s and call args is already V4 vocabulary —
        // the flag is set only where a value is *produced* (constructor,
        // call result, atom result).
        self.composites.insert(name.to_owned(), type_name);
        Ok(())
    }

    /// Reject a binder that collides with a synthesized field name. With
    /// struct values admitted, a struct literal of `T` writes its fields
    /// under `__struct_T_f` in the verifier and emitter, so a binder
    /// named like a declared struct (whose `.f` reads resolve through
    /// that name) is rejected too; `__struct_T_f` itself is reserved.
    fn bind(&self, name: &str) -> Result<(), SemanticContractError> {
        if self.reserved.contains(name)
            || (self.admit_struct_values && self.structs.contains_key(name))
        {
            return Err(SemanticContractError::Unsupported(
                "binder collides with a struct field name",
            ));
        }
        Ok(())
    }
}

/// The scalar kind a declared parameter type gives in this profile, when
/// the declaration is admissible: `i64` always; `bool` and the narrower
/// subrange integer types only when `admit_widened` (the widened
/// profile's `allow_wrapping_arithmetic` flag). Subrange types are
/// carried by i64 like `bool`; the declared signature then bounds the
/// checked input to the type's range (`param_domain`). `u64`, `usize`
/// and `isize` cannot join the widened i64-equivalence profile either
/// way: a full-width domain cannot be expressed as an i64 range bound.
/// Under `admit_u64` (`pure-i64-composite/v10`) `u64` and `usize` both
/// carry `U64` — `usize` is the target's 64-bit unsigned width — while
/// `isize` carries `I64`.
fn declared_param_kind(
    param: &crate::parser::ast::Param,
    admit_widened: bool,
    admit_u64: bool,
    admit_f64: bool,
    admit_wide: bool,
    refined: &RefinedEnv,
) -> Option<ScalarKind> {
    let name = param.type_name.as_deref()?;
    // A declared refined alias resolves to its scalar base; any other
    // undeclared type name stays outside the profile rather than fall to
    // the `i64` default.
    let kind = match name {
        "i64" => ScalarKind::I64,
        "u64" | "usize" if admit_u64 => ScalarKind::U64,
        "isize" if admit_u64 => ScalarKind::I64,
        "f64" if admit_f64 => ScalarKind::F64,
        "i128" if admit_wide => ScalarKind::I128,
        "u128" if admit_wide => ScalarKind::U128,
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

/// An owned, by-value parameter whose type reference names exactly the
/// declared (possibly instantiated) type — no effects, capabilities,
/// borrows or function contracts. Type arguments are part of the
/// display name, so `p: Pair<i64>` matches the `Pair<i64>` instance
/// declaration while `p: S<undeclared>` still cannot name a declared
/// type.
fn plain_param(param: &crate::parser::ast::Param, name: &str) -> bool {
    !(param.type_ref.as_ref().is_some_and(|ty| {
        // Instantiated types carry the concrete display name
        // (`Pair<i64>`) — matching `ty.display_name()` covers both
        // plain and instantiated uses while still rejecting an
        // effect- or capability-annotated reference.
        ty.display_name() != name || ty.effect_set.is_some() || ty.capability.is_some()
    }) || param.consume
        || param.is_ref
        || param.is_ref_mut
        || param.fn_contract_requires.is_some()
        || param.fn_contract_ensures.is_some())
}

/// `declared_param_kind` widened by the module's struct map: a parameter
/// typed by an admitted struct carries that struct's shape. Struct
/// names never overlap the scalar or refined names (`build_struct_env`).
#[allow(clippy::too_many_arguments)]
fn declared_param_shape(
    param: &crate::parser::ast::Param,
    admit_widened: bool,
    admit_u64: bool,
    admit_f64: bool,
    admit_wide: bool,
    refined: &RefinedEnv,
    structs: &StructEnv,
    enums: &EnumEnv,
) -> Option<ValueKind> {
    match param.type_name.as_deref() {
        Some(name) if structs.contains_key(name) => {
            plain_param(param, name).then(|| ValueKind::Struct(name.to_owned()))
        }
        Some(name) if enums.contains_key(name) => {
            plain_param(param, name).then(|| ValueKind::Enum(name.to_owned()))
        }
        _ => declared_param_kind(
            param,
            admit_widened,
            admit_u64,
            admit_f64,
            admit_wide,
            refined,
        )
        .map(ValueKind::Scalar),
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
    enums: &EnumEnv,
) -> ValueKind {
    match param.type_name.as_deref() {
        Some(name) if structs.contains_key(name) => ValueKind::Struct(name.to_owned()),
        Some(name) if enums.contains_key(name) => ValueKind::Enum(name.to_owned()),
        _ => ValueKind::Scalar(param_kind(param, refined)),
    }
}

/// The parameter kind inside a contract that already passed
/// `validate_atom` — unvalidated sources never reach the kind checker.
/// A refined alias reports its resolved base kind (`Port` → `i64`).
fn param_kind(param: &crate::parser::ast::Param, refined: &RefinedEnv) -> ScalarKind {
    match param.type_name.as_deref() {
        Some("bool") => ScalarKind::Bool,
        // `u64` parameters only reach the kind checker under v10
        // admission — `validate_atom` rejects the leaf below it.
        // `usize` is the target's 64-bit unsigned width (folded to
        // `u64`), `isize` its signed width (folded to `i64`).
        Some("u64" | "usize") => ScalarKind::U64,
        Some("isize") => ScalarKind::I64,
        // `f64` likewise only reaches here under v12 admission; the
        // leaf is pass-through — no operator arm ever reads it.
        Some("f64") => ScalarKind::F64,
        // `i128`/`u128` only reach the kind checker under v21
        // admission (`validate_atom` gates the leaf below it).
        Some("i128") => ScalarKind::I128,
        Some("u128") => ScalarKind::U128,
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
        Some("u64" | "usize") => ScalarKind::U64,
        Some("isize") => ScalarKind::I64,
        Some("f64") => ScalarKind::F64,
        Some("i128") => ScalarKind::I128,
        Some("u128") => ScalarKind::U128,
        Some(name) => refined
            .get(name)
            .map(|alias| alias.kind)
            .unwrap_or(ScalarKind::I64),
        None => ScalarKind::I64,
    }
}

/// `return_kind` widened by the module's struct map: an atom returning
/// an admitted struct produces that struct's shape.
fn return_shape(
    atom: &Atom,
    refined: &RefinedEnv,
    structs: &StructEnv,
    enums: &EnumEnv,
) -> ValueKind {
    match atom.return_type.as_deref() {
        Some(name) if structs.contains_key(name) => ValueKind::Struct(name.to_owned()),
        Some(name) if enums.contains_key(name) => ValueKind::Enum(name.to_owned()),
        _ => ValueKind::Scalar(return_kind(atom, refined)),
    }
}

/// The shape every module atom's call expression takes, keyed by name:
/// a bool-returning callee's call lowers to the profile's i64 0/1
/// bitvector and a struct-returning callee's call to one slot per
/// field, so `source_expr` needs the callee's declared return shape to
/// materialize that convention at the call site.
fn call_return_kinds<'ctx>(
    items: &[Item],
    refined: &RefinedEnv,
    structs: &StructEnv,
    enums: &EnumEnv,
    admission: RefinedAdmission,
) -> CallShapes<'ctx> {
    CallShapes {
        returns: items
            .iter()
            .filter_map(|item| {
                let Item::Atom(atom) = item else {
                    return None;
                };
                Some((
                    atom.name.clone(),
                    return_shape(atom, refined, structs, enums),
                ))
            })
            .collect(),
        structs: structs
            .iter()
            .map(|(name, fields)| {
                (
                    name.clone(),
                    fields
                        .iter()
                        .map(|f| (f.name.clone(), f.shape.clone()))
                        .collect(),
                )
            })
            .collect(),
        enums: enums
            .iter()
            .map(|(name, variants)| {
                (
                    name.clone(),
                    variants
                        .iter()
                        .map(|v| {
                            (
                                v.name.clone(),
                                v.fields
                                    .iter()
                                    .map(|f| match &f.shape {
                                        ValueKind::Scalar(kind) => PayloadSlotKind::Scalar(*kind),
                                        ValueKind::Struct(name) => {
                                            PayloadSlotKind::Composite(name.clone())
                                        }
                                        ValueKind::Enum(_) => {
                                            unreachable!("enum payload fields are never enum-typed")
                                        }
                                    })
                                    .collect::<Vec<_>>(),
                            )
                        })
                        .collect(),
                )
            })
            .collect(),
        merged_composites: admission.admits_merged_composites(),
        next_array_id: std::cell::Cell::new(0),
        alloc_stores: std::cell::RefCell::new(HashMap::new()),
        str_bound: std::cell::Cell::new(STR_BOUND),
    }
}

/// One payload slot's kind-level shape: a scalar leaf, or — under
/// `pure-i64-composite/v18` — a struct the slot lowers as one
/// aggregate slot whose leaves expand in the flattened signature.
#[derive(Clone)]
enum PayloadSlotKind {
    Scalar(ScalarKind),
    Composite(String),
}

/// One enum variant: its name plus the kinds of its payload slots.
type EnumVariant = (String, Vec<PayloadSlotKind>);

/// One enum's variants in declaration order.
type EnumVariantLayout = Vec<EnumVariant>;

/// What each module atom's call produces — a scalar kind or an admitted
/// struct — plus the module's struct field layouts in declaration
/// order, so the source encoder can build struct literals and split a
/// struct-valued call result into its field slots.
#[derive(Default)]
struct CallShapes<'ctx> {
    returns: BTreeMap<String, ValueKind>,
    /// Each admitted struct's declared fields in declaration order —
    /// `(name, shape)` pairs where a `Struct` shape names a nested
    /// struct (`pure-i64-composite/v8`) whose fields this same map
    /// resolves.
    structs: BTreeMap<String, Vec<(String, ValueKind)>>,
    /// Every admitted enum's variants in declaration order — a match
    /// arm's `Pattern::Variant` resolves its ordinal and payload kinds
    /// through this map.
    /// Variant name plus its payload slot kinds, in declaration order.
    enums: BTreeMap<String, EnumVariantLayout>,
    /// Whether branch-merged composites (`pure-i64-composite/v7`) are
    /// admitted: the source layer fails closed on a `Composite`/`Enum`
    /// reaching a branch merge below that admission.
    merged_composites: bool,
    /// Mint for array `alloc` identities: each literal and each merge
    /// of bindings from different literals takes a fresh one so a
    /// later indexed write propagates to exactly the right aliases.
    next_array_id: std::cell::Cell<u64>,
    /// The canonical backing store per `alloc` — the model of the one
    /// data pointer the emitter emits for the alias group. Reads and
    /// indexed writes go through this map instead of each binding's
    /// `store` snapshot, so a write through one alias reaches every
    /// binding on the same backing, including a name a block `let`
    /// shadowed: the flat `inner` map overwrites its entry, so name
    /// lookup alone can no longer carry the update.
    alloc_stores: std::cell::RefCell<HashMap<u64, Array<'ctx>>>,
    /// The current atom's effective `Str` literal bound — set by
    /// `encode_atom_body` before each atom's body is encoded.
    str_bound: std::cell::Cell<usize>,
}

impl<'ctx> CallShapes<'ctx> {
    /// A fresh backing-store identity for an `Array` value: a
    /// literal's own, or a merged value's when its two sides came
    /// from different literals (reusing either side's id would let a
    /// write through the merge reach bindings that alias only one
    /// side).
    fn fresh_array_alloc(&self) -> u64 {
        let id = self.next_array_id.get();
        self.next_array_id.set(id + 1);
        id
    }

    fn returns_bool(&self, name: &str) -> bool {
        matches!(
            self.returns.get(name),
            Some(ValueKind::Scalar(ScalarKind::Bool))
        )
    }

    fn returns_u64(&self, name: &str) -> bool {
        matches!(
            self.returns.get(name),
            Some(ValueKind::Scalar(ScalarKind::U64))
        )
    }

    fn returns_f64(&self, name: &str) -> bool {
        matches!(
            self.returns.get(name),
            Some(ValueKind::Scalar(ScalarKind::F64))
        )
    }

    /// The callee's result is a 128-bit leaf (`i128`/`u128`,
    /// `pure-i64-composite/v21`) — two 64-bit slots on the record ABI.
    fn returns_wide(&self, name: &str) -> bool {
        matches!(
            self.returns.get(name),
            Some(ValueKind::Scalar(ScalarKind::I128 | ScalarKind::U128))
        )
    }

    fn returns_u128(&self, name: &str) -> bool {
        matches!(
            self.returns.get(name),
            Some(ValueKind::Scalar(ScalarKind::U128))
        )
    }

    /// The field layout of a struct-returning callee's result, in
    /// declaration order — nested fields carry their struct name.
    fn result_fields(&self, name: &str) -> Option<&[(String, ValueKind)]> {
        match self.returns.get(name)? {
            ValueKind::Struct(struct_name) => self.structs.get(struct_name).map(Vec::as_slice),
            _ => None,
        }
    }

    /// The leaf kinds of a field layout in flattening order — a
    /// nested field expands to its struct's leaves — the layout a
    /// lowered aggregate splits into.
    fn flattened_kinds(&self, fields: &[(String, ValueKind)]) -> Vec<ScalarKind> {
        fields
            .iter()
            .flat_map(|(_, shape)| match shape {
                // A 128-bit leaf occupies two consecutive 64-bit ABI
                // slots (`pure-i64-composite/v21`) — the kinds list is
                // per-64-bit-slot, so it contributes its kind twice.
                ValueKind::Scalar(kind @ (ScalarKind::I128 | ScalarKind::U128)) => {
                    vec![*kind, *kind]
                }
                ValueKind::Scalar(kind) => vec![*kind],
                ValueKind::Struct(child) => self
                    .structs
                    .get(child)
                    .map(|inner| self.flattened_kinds(inner))
                    .unwrap_or_default(),
                // An enum-typed field expands to a tag leaf plus one
                // payload leaf per widest-variant index — the layout
                // an enum parameter lowers in
                // (`pure-i64-composite/v17`); composite payload slots
                // contribute their struct's leaves (`v18`).
                ValueKind::Enum(enum_name) => self
                    .enums
                    .get(enum_name)
                    .map(|variants| {
                        let mut kinds = vec![ScalarKind::I64];
                        kinds.extend(self.enum_layout_leaf_kinds(variants));
                        kinds
                    })
                    .unwrap_or_default(),
            })
            .collect()
    }

    /// The enum name and variant layout of an enum-returning callee's
    /// result (`pure-i64-composite/v6`) — variants in declaration
    /// order, each with its payload slot kinds.
    fn result_enum<'s>(&'s self, name: &str) -> Option<(&'s str, &'s [EnumVariant])> {
        match self.returns.get(name)? {
            ValueKind::Enum(enum_name) => self
                .enums
                .get_key_value(enum_name)
                .map(|(n, variants)| (n.as_str(), variants.as_slice())),
            _ => None,
        }
    }

    /// The leaf kinds payload slot `i` expands to: one for a scalar
    /// slot, the struct's flattened leaves for a composite slot
    /// (`pure-i64-composite/v18`).
    fn enum_slot_leaf_kinds(
        &self,
        variants: &[(String, Vec<PayloadSlotKind>)],
        i: usize,
    ) -> Vec<ScalarKind> {
        match variants.iter().find_map(|(_, slots)| slots.get(i)) {
            Some(PayloadSlotKind::Composite(struct_name)) => self
                .structs
                .get(struct_name)
                .map(|fields| self.flattened_kinds(fields))
                .unwrap_or_default(),
            Some(PayloadSlotKind::Scalar(kind)) => vec![*kind],
            None => vec![ScalarKind::I64],
        }
    }

    /// The leaf kind of every payload leaf in order — the
    /// leaf-granular `enum_shape_slot_kinds` (`pure-i64-composite/v18`).
    fn enum_layout_leaf_kinds(
        &self,
        variants: &[(String, Vec<PayloadSlotKind>)],
    ) -> Vec<ScalarKind> {
        let arity = variants
            .iter()
            .map(|(_, slots)| slots.len())
            .max()
            .unwrap_or(0);
        (0..arity)
            .flat_map(|i| self.enum_slot_leaf_kinds(variants, i))
            .collect()
    }

    /// The payload leaf count an enum lowers to — the sum of every
    /// slot's leaf width (`pure-i64-composite/v18`): a wide leaf
    /// contributes two 64-bit slots (`pure-i64-composite/v22`).
    fn enum_layout_leaf_count(&self, variants: &[(String, Vec<PayloadSlotKind>)]) -> usize {
        self.enum_layout_leaf_kinds(variants)
            .iter()
            .map(|kind| match kind {
                ScalarKind::I128 | ScalarKind::U128 => 2,
                _ => 1,
            })
            .sum()
    }

    /// The declared struct name payload slot `i` carries, when it is a
    /// composite slot (`pure-i64-composite/v18`).
    fn enum_slot_struct<'s>(
        &'s self,
        variants: &'s [(String, Vec<PayloadSlotKind>)],
        i: usize,
    ) -> Option<&'s str> {
        match variants.iter().find_map(|(_, slots)| slots.get(i)) {
            Some(PayloadSlotKind::Composite(struct_name)) => Some(struct_name.as_str()),
            _ => None,
        }
    }

    /// The payload slots of `variants` rebuilt from its flattened
    /// leaves: each scalar slot consumes one leaf, a composite slot
    /// consumes its struct's whole leaf span as one `Composite`
    /// (`pure-i64-composite/v18`). `leaves` must yield at least
    /// `enum_layout_leaf_count(variants)` items.
    fn seed_enum_slots(
        &self,
        variants: &[(String, Vec<PayloadSlotKind>)],
        leaves: &mut std::slice::Iter<'_, BV<'ctx>>,
        ctx: &'ctx Context,
    ) -> Vec<SourceValue<'ctx>> {
        let mut slots = Vec::with_capacity(layout_arity(variants));
        for i in 0..layout_arity(variants) {
            let value = if let Some(struct_name) = self.enum_slot_struct(variants, i) {
                let inner = self.structs.get(struct_name).cloned().unwrap_or_default();
                composite_from_slots(self, &inner, leaves, ctx)
            } else {
                let kind = variants
                    .iter()
                    .find_map(|(_, fields)| match fields.get(i) {
                        Some(PayloadSlotKind::Scalar(kind)) => Some(*kind),
                        _ => None,
                    })
                    .unwrap_or(ScalarKind::I64);
                if matches!(kind, ScalarKind::I128 | ScalarKind::U128) {
                    // A wide payload leaf consumes two consecutive
                    // 64-bit leaves — the same [lo, hi] order a
                    // `WideScalar` signature slot uses
                    // (`pure-i64-composite/v22`).
                    let lo = leaves.next().unwrap_or_else(|| {
                        unreachable!("admitted enums always match their lowered arity")
                    });
                    let hi = leaves.next().unwrap_or_else(|| {
                        unreachable!("admitted enums always match their lowered arity")
                    });
                    let agg = hi.concat(lo);
                    if kind == ScalarKind::U128 {
                        SourceValue::UWide(agg)
                    } else {
                        SourceValue::Wide(agg)
                    }
                } else {
                    let leaf = leaves.next().unwrap_or_else(|| {
                        unreachable!("admitted enums always match their lowered arity")
                    });
                    enum_slot_value(kind, leaf, ctx)
                }
            };
            slots.push(value);
        }
        slots
    }

    /// The zero value payload slot `i` carries beyond a variant's
    /// arity (`pure-i64-composite/v18`).
    fn default_enum_slot(
        &self,
        variants: &[(String, Vec<PayloadSlotKind>)],
        i: usize,
        ctx: &'ctx Context,
    ) -> SourceValue<'ctx> {
        match self.enum_slot_struct(variants, i) {
            Some(struct_name) => default_composite_value(
                self,
                self.structs
                    .get(struct_name)
                    .cloned()
                    .unwrap_or_default()
                    .as_slice(),
                ctx,
            ),
            None => {
                let kind = variants
                    .iter()
                    .find_map(|(_, fields)| match fields.get(i) {
                        Some(PayloadSlotKind::Scalar(kind)) => Some(*kind),
                        _ => None,
                    })
                    .unwrap_or(ScalarKind::I64);
                match kind {
                    ScalarKind::I128 => SourceValue::Wide(BV::from_u64(ctx, 0, 128)),
                    ScalarKind::U128 => SourceValue::UWide(BV::from_u64(ctx, 0, 128)),
                    _ => enum_slot_value(kind, &BV::from_u64(ctx, 0, 64), ctx),
                }
            }
        }
    }
}

/// Positional payload-slot equality for enum values
/// (`pure-i64-composite/v14`): two slots of one declared enum share a
/// kind (`build_enum_env` rejects a disagreeing index), so the BV
/// `i64`/`u64` kinds compare bitwise — the `icmp` the emitter emits —
/// and `bool` slots compare as booleans, which coincides with the raw
/// compare because the signature domain keeps `bool` slots canonical
/// `{0,1}` under every tag.
fn enum_slot_eq<'ctx>(
    a: &SourceValue<'ctx>,
    b: &SourceValue<'ctx>,
    ctx: &'ctx Context,
) -> Option<Bool<'ctx>> {
    match (a, b) {
        (
            SourceValue::Integer(x) | SourceValue::Unsigned(x),
            SourceValue::Integer(y) | SourceValue::Unsigned(y),
        ) => Some(x._eq(y)),
        // A wide payload leaf compares its whole 128-bit value — the
        // emitter's `icmp` on the i128 union slot
        // (`pure-i64-composite/v22`).
        (
            SourceValue::Wide(x) | SourceValue::UWide(x),
            SourceValue::Wide(y) | SourceValue::UWide(y),
        ) => Some(x._eq(y)),
        (SourceValue::Boolean(x), SourceValue::Boolean(y)) => Some(x._eq(y)),
        // A composite payload slot compares leaf-by-leaf; every pair
        // must compare (`pure-i64-composite/v18`). Structs always
        // declare at least one field, so the fold never sees an empty
        // pair list.
        (SourceValue::Composite(xs), SourceValue::Composite(ys)) if xs.len() == ys.len() => xs
            .iter()
            .zip(ys.iter())
            .map(|((_, x), (_, y))| enum_slot_eq(x, y, ctx))
            .collect::<Option<Vec<_>>>()
            .and_then(|eqs| {
                eqs.into_iter()
                    .reduce(|acc, eq| Bool::and(ctx, &[&acc, &eq]))
            }),
        // `f64` slots stay out: the emitter compares them with
        // `fcmp oeq`, which bitwise `_eq` cannot model (NaN != NaN,
        // -0.0 == +0.0). `Str` slots need a content compare the model
        // does not encode. Both fail closed here.
        _ => None,
    }
}

/// The zero value a composite payload slot carries beyond a variant's
/// arity — every leaf zero, shaped like the seeded value
/// (`pure-i64-composite/v18`). Payload structs are scalar-leaved by
/// admission, so this never recurses into an enum.
fn default_composite_value<'ctx>(
    call_returns: &CallShapes<'ctx>,
    fields: &[(String, ValueKind)],
    ctx: &'ctx Context,
) -> SourceValue<'ctx> {
    SourceValue::Composite(
        fields
            .iter()
            .map(|(name, shape)| {
                let value = match shape {
                    ValueKind::Scalar(ScalarKind::I128) => {
                        SourceValue::Wide(BV::from_u64(ctx, 0, 128))
                    }
                    ValueKind::Scalar(ScalarKind::U128) => {
                        SourceValue::UWide(BV::from_u64(ctx, 0, 128))
                    }
                    ValueKind::Scalar(kind) => {
                        enum_slot_value(*kind, &BV::from_u64(ctx, 0, 64), ctx)
                    }
                    ValueKind::Struct(child) => default_composite_value(
                        call_returns,
                        call_returns
                            .structs
                            .get(child)
                            .cloned()
                            .unwrap_or_default()
                            .as_slice(),
                        ctx,
                    ),
                    ValueKind::Enum(_) => {
                        unreachable!("enum payloads admit scalar-leaved structs only")
                    }
                };
                (name.clone(), value)
            })
            .collect(),
    )
}

/// A scalar `SourceValue` of `kind` reading the lowered i64 `v` under
/// the profile's 0/1 `bool` convention — the same wrap a struct
/// field slot gets.
fn enum_slot_value<'ctx>(kind: ScalarKind, v: &BV<'ctx>, ctx: &'ctx Context) -> SourceValue<'ctx> {
    match kind {
        ScalarKind::Bool => SourceValue::Boolean(v._eq(&BV::from_u64(ctx, 0, 64)).not()),
        ScalarKind::U64 => SourceValue::Unsigned(v.clone()),
        ScalarKind::F64 => SourceValue::Float64(v.clone()),
        _ => SourceValue::Integer(v.clone()),
    }
}

/// The tail-atom contract admission shared by every module atom:
/// requires/ensures must be boolean expressions and the body a statement
/// of the declared result kind — all under call resolution against the
/// complete same-module callee map.
#[allow(clippy::too_many_arguments)]
fn check_atom_contract(
    atom: &Atom,
    callees: &BTreeMap<String, CalleeSig>,
    allow_wrapping_arithmetic: bool,
    refined: &RefinedEnv,
    structs: &StructEnv,
    enums: &EnumEnv,
    admit_struct_values: bool,
    admit_field_writes: bool,
    admit_enum_values: bool,
    admit_merged_composites: bool,
    admit_bounded_memory: bool,
    admit_str: bool,
    admit_enum_scalar_ops: bool,
    admit_nested_writes: bool,
    admit_enum_writes: bool,
    admit_f64_arithmetic: bool,
    struct_values_used: &std::cell::Cell<bool>,
    field_writes_used: &std::cell::Cell<bool>,
    enum_values_used: &std::cell::Cell<bool>,
    merged_composites_used: &std::cell::Cell<bool>,
    arrays_used: &std::cell::Cell<bool>,
    strs_used: &std::cell::Cell<bool>,
    enum_scalar_ops_used: &std::cell::Cell<bool>,
    nested_writes_used: &std::cell::Cell<bool>,
    enum_writes_used: &std::cell::Cell<bool>,
    f64_arithmetic_used: &std::cell::Cell<bool>,
) -> Result<(), SemanticContractError> {
    // Struct parameters stay out of the scalar scope — so they are
    // neither scalar operands nor rebindable — and live in the
    // parallel composite map that field reads and pass-through calls
    // consult.
    let mut scope: BTreeMap<String, ScalarKind> = BTreeMap::new();
    let mut composites: BTreeMap<String, &str> = BTreeMap::new();
    for p in &atom.params {
        match param_shape(p, refined, structs, enums) {
            ValueKind::Scalar(kind) => {
                scope.insert(p.name.clone(), kind);
            }
            ValueKind::Struct(_) | ValueKind::Enum(_) => {
                let name = p.type_name.as_deref().unwrap_or_default();
                composites.insert(p.name.clone(), name);
            }
        }
    }
    let enum_paths = enum_path_map(enums, structs);
    let mut reserved: BTreeSet<String> = composites
        .iter()
        .filter(|(_, type_name)| structs.contains_key(**type_name))
        .flat_map(|(param, struct_name)| {
            all_field_paths(structs, &enum_paths, &structs[*struct_name])
                .into_iter()
                .flat_map(move |path| {
                    let path = path.replace('.', "_");
                    [
                        format!("{param}_{path}"),
                        format!("__struct_{param}_{path}"),
                    ]
                })
        })
        .collect();
    if admit_struct_values {
        reserved.extend(structs.iter().flat_map(|(struct_name, fields)| {
            all_field_paths(structs, &enum_paths, fields)
                .into_iter()
                .map(move |path| format!("__struct_{struct_name}_{}", path.replace('.', "_")))
        }));
    }
    let identifiers: BTreeSet<String> = crate::parser::lexer::Lexer::new(&atom.body_expr)
        .tokenize_checked()
        .map_err(|_| SemanticContractError::Parse)?
        .into_iter()
        .filter_map(|token| match token.token {
            crate::parser::token::Token::Ident(name) => Some(name),
            _ => None,
        })
        .chain(atom.params.iter().map(|p| p.name.clone()))
        .collect();
    let env = KindEnv {
        callees,
        composites,
        structs,
        enums,
        reserved,
        identifiers,
        admit_struct_values,
        admit_field_writes,
        admit_enum_values,
        admit_merged_composites,
        result_kind: match return_shape(atom, refined, structs, enums) {
            ValueKind::Scalar(kind) => kind,
            // A composite `result` resolves through `composites`, not
            // the scalar scope — the scalar fallback never sees it.
            _ => ScalarKind::I64,
        },
        struct_values_used,
        field_writes_used,
        enum_values_used,
        merged_composites_used,
        admit_bounded_memory,
        arrays_used,
        admit_str,
        strs_used,
        admit_enum_scalar_ops,
        enum_scalar_ops_used,
        admit_nested_writes,
        nested_writes_used,
        admit_enum_writes,
        enum_writes_used,
        admit_f64_arithmetic,
        f64_arithmetic_used,
        str_bound: atom.str_bound.unwrap_or(STR_BOUND),
    };
    for p in &atom.params {
        env.bind(&p.name)?;
    }
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
    let result_kind = match return_shape(atom, refined, structs, enums) {
        ValueKind::Scalar(kind) => kind,
        // An enum result is never a scalar either: `ensures` sees
        // `result` as the enum-typed composite (read via `match`), and
        // the body must produce a value of exactly that enum.
        ValueKind::Enum(name) => {
            enum_values_used.set(true);
            let Some((enum_name, _)) = enums.get_key_value(name.as_str()) else {
                return Err(SemanticContractError::Unsupported(
                    "enum result outside the composite profile",
                ));
            };
            let mut ensures_env = env.clone();
            ensures_env
                .composites
                .insert("result".to_owned(), enum_name.as_str());
            expect_kind(
                expression_kind(
                    &ensures,
                    &scope,
                    false,
                    allow_wrapping_arithmetic,
                    &BTreeSet::new(),
                    &ensures_env,
                )?,
                ScalarKind::Bool,
            )?;
            let produced = statement_composite_kind(
                &body,
                &scope,
                false,
                allow_wrapping_arithmetic,
                &scope.keys().cloned().collect(),
                &env,
            )?;
            if produced != enum_name {
                return Err(SemanticContractError::Unsupported(
                    "enum result of a different enum",
                ));
            }
            return Ok(());
        }
        // A struct result is never a scalar: `ensures` sees `result`
        // only through field reads of the returned struct, and the
        // body must produce a value of exactly that struct.
        ValueKind::Struct(name) => {
            struct_values_used.set(true);
            let Some((struct_name, _)) = structs.get_key_value(name.as_str()) else {
                return Err(SemanticContractError::Unsupported(
                    "struct result outside the composite profile",
                ));
            };
            let mut ensures_env = env.clone();
            ensures_env
                .composites
                .insert("result".to_owned(), struct_name.as_str());
            expect_kind(
                expression_kind(
                    &ensures,
                    &scope,
                    false,
                    allow_wrapping_arithmetic,
                    &BTreeSet::new(),
                    &ensures_env,
                )?,
                ScalarKind::Bool,
            )?;
            let produced = statement_composite_kind(
                &body,
                &scope,
                false,
                allow_wrapping_arithmetic,
                &scope.keys().cloned().collect(),
                &env,
            )?;
            if produced != struct_name {
                return Err(SemanticContractError::Unsupported(
                    "struct result of a different struct",
                ));
            }
            return Ok(());
        }
    };
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

#[allow(clippy::too_many_arguments)]
fn validate_atom(
    atom: &Atom,
    admit_widened: bool,
    admit_u64: bool,
    admit_f64: bool,
    refined: &RefinedEnv,
    structs: &StructEnv,
    enums: &EnumEnv,
    admit_struct_values: bool,
    admit_enum_values: bool,
    admit_str_bound: bool,
    admit_wide: bool,
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
        Some("u64" | "usize" | "isize") => admit_u64,
        Some("f64") => admit_f64,
        Some("i128" | "u128") => admit_wide,
        Some("bool") | Some("i8" | "i16" | "i32" | "u8" | "u16" | "u32") => admit_widened,
        // An admitted struct result (`pure-i64-composite/v3`) returns
        // the struct's field slots. An admitted enum result
        // (`pure-i64-composite/v6`) returns its tag plus payload slots.
        Some(name) if structs.contains_key(name) => admit_struct_values,
        Some(name) if enums.contains_key(name) => admit_enum_values,
        Some(name) => refined
            .get(name)
            .is_some_and(|alias| alias.kind != ScalarKind::Bool || admit_widened),
        None => false,
    };
    if !return_ok
        || atom.params.iter().any(|p| {
            declared_param_shape(
                p,
                admit_widened,
                admit_u64,
                admit_f64,
                admit_wide,
                refined,
                structs,
                enums,
            )
            .is_none()
        })
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
        || (atom.str_bound.is_some() && !admit_str_bound)
        || atom.str_bound.is_some_and(|n| n > STR_BOUND_MAX)
    {
        return Err(SemanticContractError::Unsupported(
            "semantic modes, assumptions or annotations outside profile",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ScalarKind {
    I64,
    /// A `u64` leaf — full-width unsigned two's-complement semantics:
    /// unsigned comparisons, `udiv`/`urem`/`lshr` and no signed-domain
    /// interpretation anywhere.
    U64,
    Bool,
    /// A numeric literal before it meets an operand kind: literals
    /// adapt to whichever integer kind their context declares, so
    /// `a(u64) >= 0` and `s.count + 2` kind-check without a suffix.
    /// Only `Expr::Number` produces it; no declared type ever carries
    /// it, and it never survives into a `ValueKind`.
    Num,
    /// A `let`-bound fixed-size array of `i64` leaves
    /// (`pure-i64-composite-bounded-memory/v1`).
    ArrayI64,
    /// A `let`-bound fixed-size array of `u64` leaves.
    ArrayU64,
    /// An `f64` leaf — one 64-bit slot carrying the IEEE-754 bit
    /// pattern, pass-through only: stored, copied, returned and
    /// merged, but never an operand of arithmetic or comparison
    /// (`pure-i64-composite/v11`). No literal produces it and `Num`
    /// never adapts to it.
    F64,
    /// A `let`-bound `Str` literal — a bounded inline byte sequence
    /// (`pure-i64-composite/v13`). Not unifiable with `ArrayI64` or
    /// `ArrayU64`: it is immutable and its cells carry bytes.
    Str,
    /// An `i128` leaf — two consecutive 64-bit ABI slots carrying one
    /// signed 128-bit value (`pure-i64-composite/v21`). Literals adapt
    /// to it like any integer kind.
    I128,
    /// A `u128` leaf — the unsigned counterpart of `I128`: unsigned
    /// comparisons and `udiv`/`urem`/`lshr`, two ABI slots.
    U128,
}

fn expect_kind(actual: ScalarKind, expected: ScalarKind) -> Result<(), SemanticContractError> {
    // `Num` is the unresolved-literal kind: it adapts to either integer
    // kind on BOTH sides — a `let s = 0` binding stays `Num` until an
    // `i64`/`u64` write or use pins it, and that write itself must
    // check (`s = s + n` with `s: Num` accepts an `I64` value).
    if actual == expected
        || (actual == ScalarKind::Num
            && matches!(
                expected,
                ScalarKind::I64 | ScalarKind::U64 | ScalarKind::I128 | ScalarKind::U128
            ))
        || (expected == ScalarKind::Num
            && matches!(
                actual,
                ScalarKind::I64 | ScalarKind::U64 | ScalarKind::I128 | ScalarKind::U128
            ))
    {
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
    /// The field's shape: a profile leaf scalar, or — under
    /// `pure-i64-composite/v8` — another admitted struct named by
    /// `type_name` (the field reads and passes through as a composite
    /// value, never a scalar).
    shape: ValueKind,
}

impl ResolvedField {
    /// The leaf kind of a scalar field; `None` for a struct-typed field.
    fn leaf_kind(&self) -> Option<ScalarKind> {
        match self.shape {
            ValueKind::Scalar(kind) => Some(kind),
            _ => None,
        }
    }
}

/// The leaf fields of an admitted struct in declaration order —
/// `inner: Inner` expands to `inner.a`, `inner.b` — the order the
/// emitter's nested aggregate and every flattened signature slot lay
/// out in. Admission rejects recursive structs, so the walk always
/// terminates.
fn flatten_fields<'a>(
    structs: &'a StructEnv,
    enums: &'a EnumEnv,
    fields: &'a [ResolvedField],
    prefix: &str,
    out: &mut Vec<(String, SignatureSlot<'a>)>,
) {
    for field in fields {
        let path = if prefix.is_empty() {
            field.name.clone()
        } else {
            format!("{prefix}.{}", field.name)
        };
        match &field.shape {
            ValueKind::Struct(child) => {
                if let Some(child_fields) = structs.get(child) {
                    flatten_fields(structs, enums, child_fields, &path, out);
                }
            }
            // An enum-typed field lays out like an enum parameter: a
            // tag slot then the shared payload leaves — a composite
            // slot expands to one leaf per flattened field of its
            // struct (`pure-i64-composite/v17`, `/v18`).
            ValueKind::Enum(enum_name) => {
                if let Some(variants) = enums.get(enum_name) {
                    out.push((
                        format!("{path}.tag"),
                        SignatureSlot::EnumTag {
                            variants: variants.len() as u64,
                        },
                    ));
                    for i in 0..enum_max_arity(variants) {
                        let carriers: Vec<u64> = variants
                            .iter()
                            .enumerate()
                            .filter(|(_, v)| v.fields.get(i).is_some())
                            .map(|(ordinal, _)| ordinal as u64)
                            .collect();
                        match enum_slot_type(variants, i).and_then(|name| structs.get(name)) {
                            Some(fields) => out.extend(
                                flattened_struct_fields(structs, enums, fields)
                                    .iter()
                                    .map(|(inner, slot)| {
                                        let signature_slot = match slot {
                                            SignatureSlot::WideScalar(type_name) => {
                                                SignatureSlot::EnumWideField {
                                                    type_name: *type_name,
                                                    carriers: carriers.clone(),
                                                }
                                            }
                                            _ => SignatureSlot::EnumField {
                                                type_name: slot_type_name(slot),
                                                carriers: carriers.clone(),
                                            },
                                        };
                                        (format!("{path}.{i}.{inner}"), signature_slot)
                                    })
                                    .collect::<Vec<_>>(),
                            ),
                            None => {
                                let type_name = enum_slot_type(variants, i);
                                out.push((
                                    format!("{path}.{i}"),
                                    if matches!(type_name, Some("i128" | "u128")) {
                                        SignatureSlot::EnumWideField {
                                            type_name,
                                            carriers,
                                        }
                                    } else {
                                        SignatureSlot::EnumField {
                                            type_name,
                                            carriers,
                                        }
                                    },
                                ));
                            }
                        }
                    }
                }
            }
            _ if matches!(field.type_name.as_str(), "i128" | "u128") => out.push((
                path,
                SignatureSlot::WideScalar(Some(field.type_name.as_str())),
            )),
            _ => out.push((path, SignatureSlot::Scalar(Some(field.type_name.as_str())))),
        }
    }
}

/// Every dotted field path of an admitted struct — inner prefixes
/// included: `Outer { inner: Inner, c }` gives `inner`, `inner.a`,
/// `inner.b`, `c`. The verifier and emitter synthesize names from
/// every prefix, not just leaves. An enum-typed field contributes its
/// own path plus its leaf paths (`e`, `e.tag`, `e.0`, …) so the
/// flattened-name collision check covers every slot it mints.
fn all_field_paths(
    structs: &StructEnv,
    enum_paths: &BTreeMap<String, Vec<String>>,
    fields: &[ResolvedField],
) -> Vec<String> {
    let mut out = Vec::new();
    #[allow(clippy::too_many_arguments)]
    fn push_paths(
        structs: &StructEnv,
        enum_paths: &BTreeMap<String, Vec<String>>,
        fields: &[ResolvedField],
        prefix: &str,
        out: &mut Vec<String>,
    ) {
        for field in fields {
            let path = if prefix.is_empty() {
                field.name.clone()
            } else {
                format!("{prefix}.{}", field.name)
            };
            out.push(path.clone());
            match &field.shape {
                ValueKind::Struct(child) => {
                    if let Some(inner) = structs.get(child) {
                        push_paths(structs, enum_paths, inner, &path, out);
                    }
                }
                ValueKind::Enum(enum_name) => {
                    if let Some(paths) = enum_paths.get(enum_name) {
                        out.extend(paths.iter().map(|p| format!("{path}.{p}")));
                    }
                }
                _ => (),
            }
        }
    }
    push_paths(structs, enum_paths, fields, "", &mut out);
    out
}

/// The leaf-path suffixes (`tag`, `0`, `0.q`, …) each admitted enum
/// mints below a field path — the enum view `all_field_paths` needs
/// where a prefix walk expands a composite payload slot into its
/// inner paths (`pure-i64-composite/v18`).
fn enum_path_map(enums: &EnumEnv, structs: &StructEnv) -> BTreeMap<String, Vec<String>> {
    enums
        .iter()
        .map(|(name, variants)| {
            let mut paths = vec!["tag".to_owned()];
            for i in 0..enum_max_arity(variants) {
                paths.push(i.to_string());
                if let Some(fields) = enum_slot_type(variants, i).and_then(|name| structs.get(name))
                {
                    // A payload struct is enum-free, so an empty map
                    // walks its field paths without recursing here.
                    for inner in all_field_paths(structs, &BTreeMap::new(), fields) {
                        paths.push(format!("{i}.{inner}"));
                    }
                }
            }
            (name.clone(), paths)
        })
        .collect()
}

/// The declared type name a signature leaf carries, for the typed
/// binders the obligation emits — `None` on an enum tag slot, which
/// the binder layer reports like an enum parameter's own tag.
fn slot_type_name<'a>(slot: &SignatureSlot<'a>) -> Option<&'a str> {
    match slot {
        SignatureSlot::Scalar(type_name)
        | SignatureSlot::WideScalar(type_name)
        | SignatureSlot::EnumField { type_name, .. }
        | SignatureSlot::EnumWideField { type_name, .. } => *type_name,
        SignatureSlot::EnumTag { .. } => None,
    }
}

/// `flatten_fields` for a named struct's declared fields.
fn flattened_struct_fields<'a>(
    structs: &'a StructEnv,
    enums: &'a EnumEnv,
    fields: &'a [ResolvedField],
) -> Vec<(String, SignatureSlot<'a>)> {
    let mut out = Vec::new();
    flatten_fields(structs, enums, fields, "", &mut out);
    out
}

/// `flattened_struct_fields(..).len() > limit` without materializing the
/// leaves: each occurrence of a struct-typed field contributes its own
/// leaf slots, so a chain where every level names the same child type
/// twice doubles the count per level — a bounded walk must bail as soon
/// as the limit is passed rather than expanding first.
fn leaf_count_exceeds(
    structs: &StructEnv,
    enum_widths: &BTreeMap<String, usize>,
    fields: &[ResolvedField],
    limit: usize,
) -> bool {
    fn visit(
        structs: &StructEnv,
        enum_widths: &BTreeMap<String, usize>,
        fields: &[ResolvedField],
        count: &mut usize,
        limit: usize,
    ) -> bool {
        for field in fields {
            match &field.shape {
                ValueKind::Struct(child) => {
                    if let Some(inner) = structs.get(child) {
                        if visit(structs, enum_widths, inner, count, limit) {
                            return true;
                        }
                    }
                }
                // An enum-typed field mints a tag slot plus its
                // payload leaves — same layout as an enum parameter
                // (`pure-i64-composite/v17`, `/v18`).
                ValueKind::Enum(enum_name) => {
                    *count += enum_widths.get(enum_name).copied().unwrap_or(1);
                    if *count > limit {
                        return true;
                    }
                }
                _ => {
                    *count += match field.leaf_kind() {
                        Some(ScalarKind::I128 | ScalarKind::U128) => 2,
                        _ => 1,
                    };
                    if *count > limit {
                        return true;
                    }
                }
            }
        }
        false
    }
    let mut count = 0usize;
    visit(structs, enum_widths, fields, &mut count, limit)
}

/// A struct parameter's value from its lowered leaf slots: a scalar
/// field consumes one slot, a struct-typed field consumes its own
/// flattened leaves and becomes a nested `Composite`
/// (`pure-i64-composite/v8`).
fn seed_struct_composite<'ctx>(
    structs: &StructEnv,
    enums: &EnumEnv,
    fields: &[ResolvedField],
    slots: &mut std::slice::Iter<'_, BV<'ctx>>,
    ctx: &'ctx Context,
) -> BodyResult<SourceValue<'ctx>> {
    let mut values = Vec::with_capacity(fields.len());
    for field in fields {
        let value = match &field.shape {
            ValueKind::Struct(child) => {
                let inner = structs
                    .get(child)
                    .ok_or(SemanticContractError::Unsupported("source signature arity"))?;
                seed_struct_composite(structs, enums, inner, slots, ctx)?
            }
            // An enum-typed field consumes a tag slot plus one payload
            // slot per widest-variant index, seeded exactly like an
            // enum parameter (`pure-i64-composite/v17`).
            ValueKind::Enum(enum_name) => {
                let variants = enums
                    .get(enum_name)
                    .ok_or(SemanticContractError::Unsupported("source signature arity"))?;
                let tag = slots
                    .next()
                    .cloned()
                    .ok_or(SemanticContractError::Unsupported("source signature arity"))?;
                SourceValue::Enum {
                    name: enum_name.clone(),
                    tag,
                    slots: seed_enum_payload(variants, structs, enums, slots, ctx)?,
                }
            }
            _ => match field.leaf_kind() {
                Some(kind @ (ScalarKind::I128 | ScalarKind::U128)) => {
                    let lo = slots.next();
                    let hi = slots.next();
                    let (Some(lo), Some(hi)) = (lo, hi) else {
                        return Err(SemanticContractError::Unsupported("source signature arity"));
                    };
                    let agg = hi.concat(lo);
                    if kind == ScalarKind::U128 {
                        SourceValue::UWide(agg)
                    } else {
                        SourceValue::Wide(agg)
                    }
                }
                Some(ScalarKind::Bool) => {
                    let Some(v) = slots.next() else {
                        return Err(SemanticContractError::Unsupported("source signature arity"));
                    };
                    SourceValue::Boolean(v._eq(&BV::from_u64(ctx, 0, 64)).not())
                }
                _ => {
                    let Some(v) = slots.next() else {
                        return Err(SemanticContractError::Unsupported("source signature arity"));
                    };
                    match field.leaf_kind() {
                        Some(ScalarKind::U64) => SourceValue::Unsigned(v.clone()),
                        Some(ScalarKind::F64) => SourceValue::Float64(v.clone()),
                        _ => SourceValue::Integer(v.clone()),
                    }
                }
            },
        };
        values.push((field.name.clone(), value));
    }
    Ok(SourceValue::Composite(values))
}

/// The seeded payload `SourceValue`s of an enum value, in shared
/// position order: a composite slot consumes its struct's flattened
/// leaves (`pure-i64-composite/v18`), a wide leaf consumes two
/// consecutive 64-bit slots (`pure-i64-composite/v22`), any other
/// scalar leaf consumes one. Shared by `param_env`'s enum arm and
/// `seed_struct_composite`'s enum-typed field arm.
fn seed_enum_payload<'ctx>(
    variants: &[ResolvedVariant],
    structs: &StructEnv,
    enums: &EnumEnv,
    slots: &mut std::slice::Iter<'_, BV<'ctx>>,
    ctx: &'ctx Context,
) -> BodyResult<Vec<SourceValue<'ctx>>> {
    let mut payload = Vec::with_capacity(enum_max_arity(variants));
    for i in 0..enum_max_arity(variants) {
        if let Some(inner) = enum_slot_type(variants, i).and_then(|name| structs.get(name)) {
            payload.push(seed_struct_composite(structs, enums, inner, slots, ctx)?);
            continue;
        }
        let kind = variants
            .iter()
            .find_map(|v| v.fields.get(i).and_then(|f| f.leaf_kind()))
            .unwrap_or(ScalarKind::I64);
        let value = match kind {
            ScalarKind::I128 | ScalarKind::U128 => {
                let lo = slots.next();
                let hi = slots.next();
                let (Some(lo), Some(hi)) = (lo, hi) else {
                    return Err(SemanticContractError::Unsupported("source signature arity"));
                };
                let agg = hi.concat(lo);
                if kind == ScalarKind::U128 {
                    SourceValue::UWide(agg)
                } else {
                    SourceValue::Wide(agg)
                }
            }
            _ => {
                let Some(v) = slots.next() else {
                    return Err(SemanticContractError::Unsupported("source signature arity"));
                };
                enum_slot_value(kind, v, ctx)
            }
        };
        payload.push(value);
    }
    Ok(payload)
}

/// The flattened leaf bitvectors of a source value — the layout a
/// lowered aggregate or a call argument decomposes into.
fn lower_composite<'ctx>(
    value: SourceValue<'ctx>,
    ctx: &'ctx Context,
    out: &mut Vec<BV<'ctx>>,
) -> BodyResult<()> {
    match value {
        SourceValue::Composite(fields) => {
            for (_, field) in fields {
                lower_composite(field, ctx, out)?;
            }
            Ok(())
        }
        // An enum-typed field lowers tag-first like an enum
        // parameter (`pure-i64-composite/v17`); a composite slot
        // contributes its own flattened leaves (`v18`).
        SourceValue::Enum { tag, slots, .. } => {
            out.push(tag);
            for slot in slots {
                lower_composite(slot, ctx, out)?;
            }
            Ok(())
        }
        wide @ (SourceValue::Wide(_) | SourceValue::UWide(_)) => {
            // A 128-bit leaf lowers as two consecutive 64-bit slots —
            // lo half then hi half — the record's `leaf_widths`
            // pairing (`pure-i64-composite/v21`).
            let v = lower_scalar(wide, ctx)?;
            out.push(v.extract(63, 0));
            out.push(v.extract(127, 64));
            Ok(())
        }
        scalar => {
            out.push(lower_scalar(scalar, ctx)?);
            Ok(())
        }
    }
}

/// Whether any admitted struct declares a struct-typed field — the
/// marker that pins `pure-i64-composite/v8`: a nested declaration is
/// only ever constructed under that admission, so its presence alone
/// requires it (no per-atom usage flag is needed).
fn has_nested_fields(structs: &StructEnv) -> bool {
    structs
        .values()
        .flatten()
        .any(|f| matches!(f.shape, ValueKind::Struct(_)))
}

/// Whether any admitted struct declares an enum-typed field — the
/// marker that pins `pure-i64-composite/v17`, on the same convention
/// as `has_nested_fields`.
fn has_enum_fields(structs: &StructEnv) -> bool {
    structs
        .values()
        .flatten()
        .any(|f| matches!(f.shape, ValueKind::Enum(_)))
}

/// Whether any admitted enum declares a struct-typed payload field —
/// the marker that pins `pure-i64-composite/v18`, on the same
/// convention as `has_enum_fields`.
fn has_composite_payloads(enums: &EnumEnv, structs: &StructEnv) -> bool {
    let _ = structs;
    enums.values().flatten().any(|variant| {
        variant
            .fields
            .iter()
            .any(|f| matches!(f.shape, ValueKind::Struct(_)))
    })
}

/// Whether any admitted enum declares a 128-bit leaf in a payload
/// position — directly or inside a payload struct — the marker that
/// pins `pure-i64-composite/v22`, on the same convention as
/// `has_composite_payloads`.
fn has_wide_payload_leaves(enums: &EnumEnv, structs: &StructEnv) -> bool {
    fn fields_have_wide(structs: &StructEnv, fields: &[ResolvedField]) -> bool {
        fields.iter().any(|f| match &f.shape {
            ValueKind::Scalar(ScalarKind::I128 | ScalarKind::U128) => true,
            ValueKind::Struct(child) => structs
                .get(child)
                .map(|inner| fields_have_wide(structs, inner))
                .unwrap_or(false),
            _ => false,
        })
    }
    enums.values().flatten().any(|variant| {
        variant.fields.iter().any(|f| match &f.shape {
            ValueKind::Scalar(ScalarKind::I128 | ScalarKind::U128) => true,
            ValueKind::Struct(child) => structs
                .get(child)
                .map(|inner| fields_have_wide(structs, inner))
                .unwrap_or(false),
            _ => false,
        })
    })
}

/// The module's admitted struct declarations by name, fields in
/// declaration order — the order a struct parameter flattens in. An
/// empty map is every non-composite fragment.
type StructEnv = BTreeMap<String, Vec<ResolvedField>>;

/// One admitted enum variant: its name and the payload fields it
/// carries (each resolved like a struct field's scalar type).
struct ResolvedVariant {
    name: String,
    fields: Vec<ResolvedField>,
}

/// The module's admitted enum declarations by name, variants in
/// declaration order — a variant's ordinal is its index, so the tag
/// slot an enum parameter flattens to carries the same ordinals the
/// emitter's tagged union writes. An empty map is every non-composite
/// fragment.
type EnumEnv = BTreeMap<String, Vec<ResolvedVariant>>;

/// The payload-slot count an enum flattens to: the widest variant's
/// arity, matching the tagged-union layout the emitter emits.
fn enum_max_arity(variants: &[ResolvedVariant]) -> usize {
    variants.iter().map(|v| v.fields.len()).max().unwrap_or(0)
}

/// The payload-slot count of a kind-level variant layout — the widest
/// variant's arity (`pure-i64-composite/v18`).
fn layout_arity(variants: &[(String, Vec<PayloadSlotKind>)]) -> usize {
    variants.iter().map(|(_, f)| f.len()).max().unwrap_or(0)
}

/// The type name every variant that carries index `i` agrees on, or
/// `None` when the variants disagree — a mixed-type union slot would
/// need tag-conditional slot domains, which the flattened signature
/// does not express, so such an enum stays out.
fn enum_slot_type<'a>(variants: &'a [ResolvedVariant], i: usize) -> Option<&'a str> {
    // Uniformity stays by declared name, except the width aliases:
    // `usize`/`u64` and `isize`/`i64` name the same domain and carrier.
    // Comparing resolved kinds instead would conflate a refined alias
    // with its bare base — `A(Pos)` and `B(i64)` share `I64` but the
    // slot's declared domain differs, and the signature would apply
    // `Pos`'s conjunct to `B`'s payloads.
    let mut found: Option<&'a str> = None;
    for variant in variants {
        let Some(field) = variant.fields.get(i) else {
            continue;
        };
        match found {
            None => found = Some(field.type_name.as_str()),
            Some(name) if name == field.type_name => (),
            Some(name) if width_alias_pair(name, field.type_name.as_str()) => (),
            Some(_) => return None,
        }
    }
    found
}

/// The payload slot `field` names on `variants`: the single variant
/// that declares it — its ordinal, the field's index inside the
/// variant (the payload slot is one past it, after the tag), and the
/// resolved field. `None` when no variant declares `field`, or more
/// than one does — an ambiguous name cannot pin the tag obligation
/// the write requires.
fn enum_field_slot<'a>(
    variants: &'a [ResolvedVariant],
    field: &str,
) -> Option<(usize, usize, &'a ResolvedField)> {
    let mut found: Option<(usize, usize, &'a ResolvedField)> = None;
    for (ordinal, variant) in variants.iter().enumerate() {
        let Some(index) = variant.fields.iter().position(|f| f.name == field) else {
            continue;
        };
        if found.is_some() {
            return None;
        }
        found = Some((ordinal, index, &variant.fields[index]));
    }
    found
}

/// Whether every leaf of an admitted struct is scalar — no enum-typed
/// field anywhere inside. A struct-typed enum payload may only carry
/// such a struct: an enum leaf inside would nest tagged unions and
/// could close a struct↔enum size cycle (`pure-i64-composite/v18`).
/// Admitted structs are acyclic, but a visited guard keeps the walk
/// bounded even mid-admission.
fn struct_is_scalar_leaved(structs: &StructEnv, fields: &[ResolvedField]) -> bool {
    fn visit(
        structs: &StructEnv,
        fields: &[ResolvedField],
        visited: &mut BTreeSet<String>,
    ) -> bool {
        fields.iter().all(|field| match &field.shape {
            ValueKind::Scalar(_) => true,
            ValueKind::Enum(_) => false,
            ValueKind::Struct(child) => {
                // `visited` is a DFS path guard, not a done-set: a shared
                // child (`Pair { left: Point, right: Point }`) revisits a
                // type on a sibling branch and is still scalar-leaved, so
                // the name must leave the set when the subtree returns.
                if !visited.insert(child.clone()) {
                    false
                } else {
                    let ok = structs
                        .get(child)
                        .map(|inner| visit(structs, inner, visited))
                        .unwrap_or(false);
                    visited.remove(child);
                    ok
                }
            }
        })
    }
    visit(structs, fields, &mut BTreeSet::new())
}

/// The signature slots payload position `i` expands to under
/// `carriers`: one `EnumField` per scalar leaf, one `EnumWideField`
/// per 128-bit leaf (`pure-i64-composite/v22`) — a direct `i128`/`u128`
/// payload or a wide leaf inside a payload struct.
fn enum_slot_leaf_slots<'a>(
    variants: &'a [ResolvedVariant],
    structs: &'a StructEnv,
    enums: &'a EnumEnv,
    i: usize,
    carriers: Vec<u64>,
) -> Vec<SignatureSlot<'a>> {
    let Some(name) = enum_slot_type(variants, i) else {
        return Vec::new();
    };
    let field = |type_name: Option<&'a str>| SignatureSlot::EnumField {
        type_name,
        carriers: carriers.clone(),
    };
    let wide = |type_name: Option<&'a str>| SignatureSlot::EnumWideField {
        type_name,
        carriers: carriers.clone(),
    };
    match structs.get(name) {
        Some(fields) => flattened_struct_fields(structs, enums, fields)
            .iter()
            .map(|(_, slot)| match slot {
                SignatureSlot::WideScalar(type_name) => wide(*type_name),
                _ => field(slot_type_name(slot)),
            })
            .collect(),
        None if matches!(name, "i128" | "u128") => vec![wide(Some(name))],
        None => vec![field(Some(name))],
    }
}

/// The 64-bit input slots one signature leaf consumes: 2 for a wide
/// leaf (`pure-i64-composite/v21` and payload leaves under v22), 1
/// otherwise.
fn signature_slot_weight(slot: &SignatureSlot<'_>) -> usize {
    match slot {
        SignatureSlot::WideScalar(_) | SignatureSlot::EnumWideField { .. } => 2,
        _ => 1,
    }
}

/// The 64-bit input-slot width payload position `i` contributes: 1
/// for a scalar leaf, 2 for a wide leaf, the struct's flattened
/// slot-weight sum for a composite slot (`pure-i64-composite/v22`).
fn enum_slot_leaf_width(
    variants: &[ResolvedVariant],
    structs: &StructEnv,
    enums: &EnumEnv,
    i: usize,
) -> usize {
    enum_slot_leaf_slots(variants, structs, enums, i, Vec::new())
        .iter()
        .map(signature_slot_weight)
        .sum()
}

/// The payload leaf count an enum flattens to — the sum of every
/// slot's leaf width (`pure-i64-composite/v18`): equals
/// `enum_max_arity` when no slot is composite.
fn enum_leaf_count(variants: &[ResolvedVariant], structs: &StructEnv, enums: &EnumEnv) -> usize {
    (0..enum_max_arity(variants))
        .map(|i| enum_slot_leaf_width(variants, structs, enums, i))
        .sum()
}

/// The leaf-path suffix and leaf type name of every payload leaf in
/// order — `0`, `1.q`, … — the flat expansion the signature and the
/// binder list encode (`pure-i64-composite/v18`).
fn enum_leaf_paths<'a>(
    variants: &'a [ResolvedVariant],
    structs: &'a StructEnv,
    enums: &'a EnumEnv,
) -> Vec<(String, Option<&'a str>)> {
    let mut out = Vec::new();
    for i in 0..enum_max_arity(variants) {
        match enum_slot_type(variants, i).and_then(|name| structs.get(name)) {
            Some(fields) => out.extend(
                flattened_struct_fields(structs, enums, fields)
                    .iter()
                    .map(|(path, slot)| (format!("{i}.{path}"), slot_type_name(slot))),
            ),
            None => out.push((format!("{i}"), enum_slot_type(variants, i))),
        }
    }
    out
}

/// The declared type name an enum declaration's payload slot `i`
/// carries — the `enum_slot_type` uniform-name rule applied to the raw
/// declaration text, before `build_enum_env` resolves fields. `None`
/// when the variants disagree (the enum is rejected at build).
fn enum_decl_slot_name<'a>(def: &'a crate::parser::ast::EnumDef, i: usize) -> Option<&'a str> {
    let mut found: Option<&'a str> = None;
    for variant in &def.variants {
        let Some(field) = variant.fields.get(i) else {
            continue;
        };
        let type_name = match field.split_once(':') {
            Some((_, ty)) => ty.trim(),
            None => field.as_str(),
        };
        match found {
            None => found = Some(type_name),
            Some(name) if name == type_name => (),
            Some(name) if width_alias_pair(name, type_name) => (),
            Some(_) => return None,
        }
    }
    found
}

/// The lowered leaf width one occurrence of the declared type `name`
/// contributes — 1 for scalars and aliases, a struct's flattened leaf
/// count, an enum's tag plus leaf-expanded payload slots
/// (`pure-i64-composite/v18`). Declared-type recursion bottoms out at
/// `depth`: deeper chains are rejected by the depth and recursion
/// gates anyway, so a bounded estimate keeps the count finite for
/// `leaf_count_exceeds` before `EnumEnv` exists.
fn declared_leaf_width(items: &[Item], name: &str, depth: usize) -> usize {
    // Callers only distinguish `<= 16` (the admission budget) from
    // over-budget, so the walk caps at 17: without the cap a shared-child
    // DAG (`S_d { f0..f15: S_{d-1} }`) walks 16^d nodes before the
    // admission gate can reject it — the same exponential blow-up the
    // `enum_paths` gate removes from path materialisation.
    declared_leaf_width_capped(items, name, depth, 17)
}

fn declared_leaf_width_capped(items: &[Item], name: &str, depth: usize, cap: usize) -> usize {
    if depth == 0 {
        return usize::MAX / 4;
    }
    for item in items {
        match item {
            Item::StructDef(def) if def.name == name => {
                return def.fields.iter().fold(0usize, |count, field| {
                    if count > cap {
                        return count;
                    }
                    count.saturating_add(declared_leaf_width_capped(
                        items,
                        field.type_name.as_str(),
                        depth - 1,
                        cap,
                    ))
                });
            }
            Item::EnumDef(def) if def.name == name => {
                let arity = def
                    .variants
                    .iter()
                    .map(|v| v.fields.len())
                    .max()
                    .unwrap_or(0);
                return (0..arity).fold(1usize, |count, i| {
                    if count > cap {
                        return count;
                    }
                    count.saturating_add(
                        enum_decl_slot_name(def, i)
                            .map(|t| declared_leaf_width_capped(items, t, depth - 1, cap))
                            .unwrap_or(1),
                    )
                });
            }
            _ => (),
        }
    }
    // A 128-bit leaf carries two 64-bit slots
    // (`pure-i64-composite/v21`, `/v22`).
    if matches!(name, "i128" | "u128") {
        2
    } else {
        1
    }
}

/// Every field path a declared composite type contributes below its
/// binding — inner prefixes included, matching `all_field_paths`'s
/// coverage of the synthesized names. Struct fields contribute `f`
/// plus `f.{sub}` for a composite `f`; enum fields contribute `f`,
/// `f.tag` and per-slot `f.{i}` / `f.{i}.{sub}` leaf paths
/// (`pure-i64-composite/v18`). Bounded by `depth` like
/// `declared_leaf_width`.
fn declared_field_paths(items: &[Item], name: &str, depth: usize) -> Vec<String> {
    fn push_field_paths(
        items: &[Item],
        name: &str,
        prefix: &str,
        depth: usize,
        out: &mut Vec<String>,
    ) {
        if depth == 0 {
            return;
        }
        for item in items {
            match item {
                Item::StructDef(def) if def.name == name => {
                    for field in &def.fields {
                        let path = format!("{prefix}{}", field.name);
                        out.push(path.clone());
                        push_field_paths(
                            items,
                            field.type_name.as_str(),
                            &format!("{path}."),
                            depth - 1,
                            out,
                        );
                    }
                    return;
                }
                Item::EnumDef(def) if def.name == name => {
                    out.push(format!("{prefix}tag"));
                    let arity = def
                        .variants
                        .iter()
                        .map(|v| v.fields.len())
                        .max()
                        .unwrap_or(0);
                    for i in 0..arity {
                        let path = format!("{prefix}{i}");
                        out.push(path.clone());
                        if let Some(t) = enum_decl_slot_name(def, i) {
                            push_field_paths(items, t, &format!("{path}."), depth - 1, out);
                        }
                    }
                    return;
                }
                _ => (),
            }
        }
    }
    let mut out = Vec::new();
    push_field_paths(items, name, "", depth, &mut out);
    out
}

/// The declared-name pairs that fold to the same 64-bit leaf:
/// `usize`/`u64` (unsigned) and `isize`/`i64` (signed). Only builtin
/// names pair — a refined alias never does, since it carries a domain
/// conjunct its base lacks.
fn width_alias_pair(a: &str, b: &str) -> bool {
    matches!(
        (a, b),
        ("u64", "usize") | ("usize", "u64") | ("i64", "isize") | ("isize", "i64")
    )
}

/// Collect the module's `Item::EnumDef` declarations under
/// `CompositeV4` admission. Like structs, nothing a declaration
/// carries may be dropped silently: recursive or generic enums, empty
/// enums, duplicate variant names and duplicate fields in a variant
/// fail closed, and every payload field resolves the same way a
/// struct field does (literal `i64`/`bool` or a refined alias ending
/// in one — the chain's leaf must be uniform across a payload index's
/// variants by `enum_slot_type` at signature flattening). Names
/// shadowing scalar profile names, colliding with a refined or struct
/// type, or declared twice fail closed.
fn build_enum_env(
    items: &[Item],
    refined: &RefinedEnv,
    structs: &StructEnv,
    admission: RefinedAdmission,
) -> Result<EnumEnv, SemanticContractError> {
    let mut env = EnumEnv::new();
    for item in items {
        let Item::EnumDef(def) = item else {
            continue;
        };
        if def.is_recursive || !def.type_params.is_empty() {
            return Err(SemanticContractError::Unsupported(
                "recursive or generic enum declarations",
            ));
        }
        if def.variants.is_empty() {
            return Err(SemanticContractError::Unsupported(
                "empty enum declarations",
            ));
        }
        if scalar_base(&def.name).is_some()
            || matches!(def.name.as_str(), "u64" | "usize" | "isize")
        {
            return Err(SemanticContractError::Unsupported(
                "type name shadows a scalar profile name",
            ));
        }
        if refined.contains_key(&def.name) || structs.contains_key(&def.name) {
            return Err(SemanticContractError::Unsupported(
                "enum name collides with a type name",
            ));
        }
        if env.contains_key(&def.name) {
            return Err(SemanticContractError::Unsupported(
                "duplicate enum declaration",
            ));
        }
        let mut variant_names = BTreeSet::new();
        let mut variants = Vec::with_capacity(def.variants.len());
        for variant in &def.variants {
            if variant.is_recursive {
                return Err(SemanticContractError::Unsupported(
                    "recursive enum declarations",
                ));
            }
            if !variant_names.insert(variant.name.as_str()) {
                return Err(SemanticContractError::Unsupported("duplicate enum variant"));
            }
            if variant.fields.len() != variant.field_types.len() {
                return Err(SemanticContractError::Unsupported("enum variant shape"));
            }
            let mut names = BTreeSet::new();
            let mut fields = Vec::with_capacity(variant.fields.len());
            for (index, field) in variant.fields.iter().enumerate() {
                // The parser keeps a declared `name: type` field verbatim
                // (and `field_types` rides the same string); a bare entry
                // is a positional field whose text IS the type.
                let (name, type_name) = match field.split_once(':') {
                    Some((name, ty)) => (name.trim().to_owned(), ty.trim()),
                    // Positional fields carry no name to collide.
                    None => (format!("#{index}"), field.as_str()),
                };
                if name.is_empty() || type_name.is_empty() {
                    return Err(SemanticContractError::Unsupported("enum variant shape"));
                }
                if !names.insert(name.clone()) {
                    return Err(SemanticContractError::Unsupported("duplicate enum field"));
                }
                let ty = &variant.field_types[index];
                let kind = match type_name {
                    "i64" => Some(ScalarKind::I64),
                    "u64" | "usize" if admission.admits_u64_leaves() => Some(ScalarKind::U64),
                    "isize" if admission.admits_u64_leaves() => Some(ScalarKind::I64),
                    "f64" if admission.admits_f64_leaves() => Some(ScalarKind::F64),
                    "i128" if admission.admits_wide_payloads() => Some(ScalarKind::I128),
                    "u128" if admission.admits_wide_payloads() => Some(ScalarKind::U128),
                    "bool" => Some(ScalarKind::Bool),
                    name => refined
                        .get(name)
                        .filter(|alias| matches!(alias.base, "i64" | "bool"))
                        .map(|alias| alias.kind),
                };
                let shape = match kind {
                    Some(kind) => Some(ValueKind::Scalar(kind)),
                    // A struct-typed payload field is a composite slot:
                    // admitted only when every leaf of the struct is a
                    // scalar — an enum leaf inside would nest enums and
                    // could close a struct↔enum size cycle
                    // (`pure-i64-composite/v18`). Wide leaves ride the
                    // same flattening as scalar leaves under
                    // `pure-i64-composite/v22`.
                    None if admission.admits_composite_payloads() => structs
                        .get(type_name)
                        .filter(|fields| {
                            struct_is_scalar_leaved(structs, fields)
                                && (admission.admits_wide_payloads()
                                    || !flattened_struct_fields(structs, &env, fields).iter().any(
                                        |(_, slot)| matches!(slot, SignatureSlot::WideScalar(_)),
                                    ))
                        })
                        .map(|_| ValueKind::Struct(type_name.to_owned())),
                    None => None,
                };
                let Some(shape) = shape.filter(|_| {
                    ty.type_args.is_empty() && ty.effect_set.is_none() && ty.capability.is_none()
                }) else {
                    return Err(SemanticContractError::Unsupported(
                        "enum field type outside the scalar profile",
                    ));
                };
                fields.push(ResolvedField {
                    name: name.to_owned(),
                    type_name: type_name.to_owned(),
                    shape,
                });
            }
            variants.push(ResolvedVariant {
                name: variant.name.clone(),
                fields,
            });
        }
        // A payload index the variants type differently has no uniform
        // carrier: reject the whole enum now rather than guess per-tag.
        for i in 0..enum_max_arity(&variants) {
            if enum_slot_type(&variants, i).is_none() {
                return Err(SemanticContractError::Unsupported(
                    "enum payload slot typed differently across variants",
                ));
            }
        }
        env.insert(def.name.clone(), variants);
    }
    Ok(env)
}

/// The enum type name an enum-typed parameter carries, and its
/// admitted variants — `None` for a scalar or struct parameter.
fn enum_variants<'a>(
    param: &crate::parser::ast::Param,
    enums: &'a EnumEnv,
) -> Option<&'a [ResolvedVariant]> {
    enums.get(param.type_name.as_deref()?).map(Vec::as_slice)
}

/// Collect the module's `Item::StructDef` declarations (only present
/// past the item-vocabulary gate under `Composite` admission). Nothing
/// a declaration carries may be dropped silently, so generic
/// parameters, invariants, methods, field constraints and empty field
/// lists are rejected; every field must be `i64` or `bool` (no
/// struct-valued fields, so no recursion). Subrange and refined-alias
/// fields stay out: the verifier binds a struct field by its base sort
/// only, without the range or alias predicates a scalar parameter of
/// that type carries, so the recheck model would assume a domain the
/// build never verified against. Names shadowing scalar profile names,
/// colliding with a refined type, duplicate structs and duplicate
/// fields fail closed.
fn build_struct_env(
    items: &[Item],
    refined: &RefinedEnv,
    admission: RefinedAdmission,
) -> Result<StructEnv, SemanticContractError> {
    let nested_allowed = admission.admits_nested_structs();
    let enum_fields_allowed = admission.admits_enum_struct_fields();
    // The declared struct names, so a field can name a sibling or a
    // forward-declared struct under `pure-i64-composite/v8`.
    let declared_names: BTreeSet<&str> = items
        .iter()
        .filter_map(|item| match item {
            Item::StructDef(def) => Some(def.name.as_str()),
            _ => None,
        })
        .collect();
    // The declared enum names and each one's lowered leaf width and
    // leaf-path suffixes — enough for an enum-typed field's admission,
    // leaf count and path enumeration before `build_enum_env`
    // resolves payload details (`pure-i64-composite/v17`, `/v18`).
    // Both views are depth-bounded: a chain the recursion and depth
    // gates reject anyway only needs a finite estimate here.
    let enum_widths: BTreeMap<String, usize> = items
        .iter()
        .filter_map(|item| match item {
            Item::EnumDef(def) => Some((
                def.name.clone(),
                declared_leaf_width(items, def.name.as_str(), 8),
            )),
            _ => None,
        })
        .collect();
    let enum_paths: BTreeMap<String, Vec<String>> = items
        .iter()
        .filter_map(|item| match item {
            // `declared_field_paths` materialises every flattened path
            // BEFORE the leaf gate could reject a wide declaration, so an
            // out-of-budget enum still walks an exponential tree (16^d
            // paths at depth d). Gate on the precomputed leaf width first:
            // anything already over the `<= 16` budget fails admission
            // here without expanding a single path.
            Item::EnumDef(def)
                if enum_widths
                    .get(def.name.as_str())
                    .is_some_and(|width| *width <= 16) =>
            {
                Some((
                    def.name.clone(),
                    declared_field_paths(items, def.name.as_str(), 8),
                ))
            }
            _ => None,
        })
        .collect();
    // Every edge `outer -> inner` a struct-typed field would add, for
    // the recursion check: a value-typed field graph must be acyclic —
    // a cycle would demand an infinitely large aggregate.
    let mut edges: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for item in items {
        let Item::StructDef(def) = item else {
            continue;
        };
        let mut targets = Vec::new();
        for field in &def.fields {
            if declared_names.contains(field.type_name.as_str()) {
                if !nested_allowed {
                    return Err(SemanticContractError::Unsupported(
                        "struct field type outside the scalar profile",
                    ));
                }
                targets.push(field.type_name.as_str());
            }
        }
        edges.insert(def.name.as_str(), targets);
    }
    for name in &declared_names {
        let mut stack: Vec<&str> = vec![name];
        let mut visited: BTreeSet<&str> = BTreeSet::new();
        while let Some(current) = stack.pop() {
            if current == *name && !visited.is_empty() {
                return Err(SemanticContractError::Unsupported(
                    "recursive struct declarations",
                ));
            }
            if !visited.insert(current) {
                continue;
            }
            if let Some(targets) = edges.get(current) {
                stack.extend(targets.iter().copied());
            }
        }
    }
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
            // `CompositeV2` admits a refined alias whose chain (already
            // resolved by `build_refined_env`: no units, no cycles, scalar
            // leaves only) ends in `i64` or `bool` — the verifier assumes
            // its predicates for struct fields. Subrange leaves stay out:
            // the verifier binds a field by its base sort without the
            // subrange bound a scalar parameter of that type carries.
            // `CompositeV8` additionally admits a field whose type is
            // another admitted struct (`flatten_fields` decomposes it).
            let shape = match field.type_name.as_str() {
                "i64" => Some(ValueKind::Scalar(ScalarKind::I64)),
                "u64" | "usize" if admission.admits_u64_leaves() => {
                    Some(ValueKind::Scalar(ScalarKind::U64))
                }
                "isize" if admission.admits_u64_leaves() => {
                    Some(ValueKind::Scalar(ScalarKind::I64))
                }
                "f64" if admission.admits_f64_leaves() => Some(ValueKind::Scalar(ScalarKind::F64)),
                "i128" if admission.admits_wide_leaves() => {
                    Some(ValueKind::Scalar(ScalarKind::I128))
                }
                "u128" if admission.admits_wide_leaves() => {
                    Some(ValueKind::Scalar(ScalarKind::U128))
                }
                "bool" => Some(ValueKind::Scalar(ScalarKind::Bool)),
                name if nested_allowed && declared_names.contains(name) => {
                    Some(ValueKind::Struct(name.to_owned()))
                }
                // `CompositeV17` admits a field whose type is an
                // admitted enum: it flattens to a tag plus payload
                // leaf slots exactly like an enum parameter.
                name if enum_fields_allowed && enum_widths.contains_key(name) => {
                    Some(ValueKind::Enum(name.to_owned()))
                }
                name if admission >= RefinedAdmission::CompositeV2 => refined
                    .get(name)
                    .filter(|alias| matches!(alias.base, "i64" | "bool"))
                    .map(|alias| ValueKind::Scalar(alias.kind)),
                _ => None,
            };
            let Some(shape) = shape.filter(|_| {
                // `field.type_name` is the declared display name, so an
                // instantiated leaf (`inner: Pair<i64>`) matches by the
                // same convention `plain_param` uses.
                ty.display_name() == field.type_name
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
                shape,
            });
        }
        if env.insert(def.name.clone(), fields).is_some() {
            return Err(SemanticContractError::Unsupported("duplicate struct name"));
        }
    }
    for fields in env.values() {
        // The LLVM side admits literal aggregates of at most 16 i64
        // leaf slots; a struct that flattens past that (or nests
        // deeper than the walk bound below) stays outside the
        // fragment rather than risking a signature arity mismatch
        // the admission checker could not express. The count is a
        // capped walk rather than a materialized list — a chain of
        // declarations each carrying two fields of the same child
        // type doubles its leaf count per level, so expanding first
        // and counting later would exhaust memory before the limit
        // check could reject.
        if leaf_count_exceeds(&env, &enum_widths, fields, 16) || struct_depth(&env, fields, 0) > 8 {
            return Err(SemanticContractError::Unsupported(
                "struct declaration nests beyond the aggregate limit",
            ));
        }
        // Two different field paths can synthesize the same flattened
        // name (`Inner { a }` inside `Outer { inner: Inner, inner_a }`
        // both land on `inner_a`): the verifier binds leaves under
        // underscore-joined paths, so such a declaration would alias
        // two independent slots to one symbol and prove them equal.
        let mut flattened_names = BTreeSet::new();
        for path in all_field_paths(&env, &enum_paths, fields) {
            if !flattened_names.insert(path.replace('.', "_")) {
                return Err(SemanticContractError::Unsupported(
                    "struct field paths collide when flattened",
                ));
            }
        }
    }
    Ok(env)
}

/// The nesting depth of an admitted struct (`Inner { a }` inside
/// `Outer { inner: Inner }` gives `Outer` depth 2). Acyclic by
/// admission, so the walk terminates.
fn struct_depth(structs: &StructEnv, fields: &[ResolvedField], depth: usize) -> usize {
    fields
        .iter()
        .map(|field| match &field.shape {
            ValueKind::Struct(child) => structs
                .get(child)
                .map(|inner| struct_depth(structs, inner, depth + 1))
                .unwrap_or(depth + 1),
            _ => depth + 1,
        })
        .max()
        .unwrap_or(depth)
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
                    enums: &EnumEnv::new(),
                    reserved: BTreeSet::new(),
                    identifiers: BTreeSet::new(),
                    admit_struct_values: false,
                    admit_field_writes: false,
                    admit_enum_values: false,
                    admit_merged_composites: false,
                    result_kind: ScalarKind::I64,
                    struct_values_used: &std::cell::Cell::new(false),
                    field_writes_used: &std::cell::Cell::new(false),
                    enum_values_used: &std::cell::Cell::new(false),
                    merged_composites_used: &std::cell::Cell::new(false),
                    admit_bounded_memory: false,
                    arrays_used: &std::cell::Cell::new(false),
                    admit_str: false,
                    strs_used: &std::cell::Cell::new(false),
                    admit_enum_scalar_ops: false,
                    enum_scalar_ops_used: &std::cell::Cell::new(false),
                    admit_nested_writes: false,
                    nested_writes_used: &std::cell::Cell::new(false),
                    admit_enum_writes: false,
                    enum_writes_used: &std::cell::Cell::new(false),
                    admit_f64_arithmetic: false,
                    f64_arithmetic_used: &std::cell::Cell::new(false),
                    str_bound: STR_BOUND,
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
            // A field write in tail position yields the assigned
            // field's scalar value, same as a scalar rebind yields its
            // name's kind. A struct-typed field cannot yield a scalar
            // kind, so a tail-position whole-subtree write stays
            // closed (a block-statement write is admitted below).
            if let Some(shape) = field_write_shape(env, scope, var) {
                let ValueKind::Scalar(field_kind) = shape else {
                    return Err(SemanticContractError::Unsupported(
                        "struct field write in tail position",
                    ));
                };
                let field_kind = *field_kind;
                let kind = expression_kind(
                    value,
                    scope,
                    result_in_scope,
                    allow_wrapping_arithmetic,
                    &BTreeSet::new(),
                    env,
                )?;
                expect_kind(kind, field_kind)?;
                env.field_writes_used.set(true);
                return Ok(field_kind);
            }
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
            let (lets, last) = stmts.split_at(stmts.len() - 1);
            let (inner, inner_assignable, inner_env) = block_prefix_kinds(
                lets,
                scope,
                result_in_scope,
                allow_wrapping_arithmetic,
                assignable,
                env,
            )?;
            statement_kind(
                &last[0],
                &inner,
                result_in_scope,
                allow_wrapping_arithmetic,
                &inner_assignable,
                &inner_env,
            )
        }
        _ => Err(SemanticContractError::Unsupported(
            "statement, mutation or empty body",
        )),
    }
}

/// The non-tail statements of a sequential block: `let` bindings
/// (scalar, or struct-valued under `pure-i64-composite/v3`), scalar
/// reassignments, discarded pure expressions and mid-block `while`s.
/// Returns the scope, assignable set and kind environment the block
/// tail sees.
#[allow(clippy::type_complexity)]
fn block_prefix_kinds<'a>(
    lets: &[Stmt],
    scope: &BTreeMap<String, ScalarKind>,
    result_in_scope: bool,
    allow_wrapping_arithmetic: bool,
    assignable: &BTreeSet<String>,
    env: &KindEnv<'a>,
) -> Result<(BTreeMap<String, ScalarKind>, BTreeSet<String>, KindEnv<'a>), SemanticContractError> {
    let mut inner = scope.clone();
    let mut inner_assignable = assignable.clone();
    let mut inner_env = env.clone();
    for stmt in lets {
        match stmt {
            // A struct-valued `let` (`pure-i64-composite/v3`)
            // binds a struct name that exposes field reads
            // only — it leaves the scalar scope, so it is
            // neither a scalar operand nor rebindable.
            Stmt::Let { var, value, .. }
                if (inner_env.admit_struct_values || inner_env.admit_enum_values)
                    && is_composite_value(value, &inner, &inner_env) =>
            {
                let struct_name = composite_kind(
                    value,
                    &inner,
                    result_in_scope,
                    allow_wrapping_arithmetic,
                    &inner_assignable,
                    &inner_env,
                )?;
                // A `let` may re-bind a composite name only to a
                // value of the SAME struct type — a full re-init
                // overwrites every `q_f`/`__struct_q_f` slot, while a
                // different-typed shadow would leave stale slots the
                // branch merge then leaks into the outer binding.
                if inner_env
                    .composite_name(var, &inner)
                    .is_some_and(|tn| tn != struct_name)
                {
                    return Err(SemanticContractError::Unsupported(
                        "let shadows a struct binding with a different type",
                    ));
                }
                inner_env.bind_composite(var, struct_name)?;
                inner.remove(var);
                inner_assignable.remove(var);
            }
            Stmt::Let { var, value, .. } => {
                // A scalar `let` must not shadow a live struct
                // binding either: the composite's name-keyed slots
                // stay bound to the old values and reads through the
                // scalar would merge stale data.
                if inner_env
                    .composite_name(var, &inner)
                    .is_some_and(|tn| inner_env.structs.contains_key(tn))
                {
                    return Err(SemanticContractError::Unsupported(
                        "let shadows a struct binding with a different type",
                    ));
                }
                let kind = expression_kind(
                    value,
                    &inner,
                    result_in_scope,
                    allow_wrapping_arithmetic,
                    &BTreeSet::new(),
                    &inner_env,
                )?;
                inner_env.bind(var)?;
                inner.insert(var.clone(), kind);
                inner_assignable.insert(var.clone());
            }
            Stmt::Assign { var, value, .. } => {
                // A struct field write (`pure-i64-composite/v5`) is a
                // rebind of the composite binding: the base stays
                // bound, only the declared leaf field's kind must
                // match the value. Under `pure-i64-composite/v8` a
                // struct-typed field is a whole-subtree write: the
                // value must be a composite of exactly the declared
                // type.
                if let Some(shape) = field_write_shape(&inner_env, &inner, var) {
                    match shape {
                        ValueKind::Scalar(field_kind) => {
                            let field_kind = *field_kind;
                            let kind = expression_kind(
                                value,
                                &inner,
                                result_in_scope,
                                allow_wrapping_arithmetic,
                                &BTreeSet::new(),
                                &inner_env,
                            )?;
                            expect_kind(kind, field_kind)?;
                        }
                        // An enum-typed field takes a whole-enum
                        // write `b.e = e2` of the same declared enum
                        // (`pure-i64-composite/v17`) — checked exactly
                        // like a whole-subtree struct write.
                        ValueKind::Struct(field_type) | ValueKind::Enum(field_type) => {
                            let field_type = field_type.clone();
                            let Ok(found) = composite_kind(
                                value,
                                &inner,
                                result_in_scope,
                                allow_wrapping_arithmetic,
                                &BTreeSet::new(),
                                &inner_env,
                            ) else {
                                return Err(SemanticContractError::Unsupported(
                                    "struct field write value",
                                ));
                            };
                            if found != field_type.as_str() {
                                return Err(SemanticContractError::Unsupported(
                                    "struct field write type",
                                ));
                            }
                        }
                    }
                    inner_env.field_writes_used.set(true);
                    continue;
                }
                if !inner_assignable.contains(var) {
                    return Err(SemanticContractError::Unsupported(
                        "assignment to a name not bound on the linear path",
                    ));
                }
                // A `Str` binding is immutable (`pure-i64-composite/v13`):
                // neither a fresh literal nor any other value re-seats it.
                if inner.get(var) == Some(&ScalarKind::Str) {
                    return Err(SemanticContractError::Unsupported(
                        "assignment to a Str binding",
                    ));
                }
                let kind = expression_kind(
                    value,
                    &inner,
                    result_in_scope,
                    allow_wrapping_arithmetic,
                    &BTreeSet::new(),
                    &inner_env,
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
                    &inner_env,
                )?;
            }
            // An indexed write `a[i] = v` (`bounded-memory`): the name
            // must be a `let`-bound array on the linear path, the index
            // an `i64` and the value the element kind. The binding
            // stays bound — the write mutates the array's contents.
            Stmt::ArrayStore {
                array,
                index,
                value,
                ..
            } if inner_env.admit_bounded_memory => {
                let elem = match inner.get(array.as_str()) {
                    Some(ScalarKind::ArrayI64) => ScalarKind::I64,
                    Some(ScalarKind::ArrayU64) => ScalarKind::U64,
                    _ => {
                        return Err(SemanticContractError::Unsupported(
                            "array write on a non-array binding",
                        ))
                    }
                };
                if !inner_assignable.contains(array.as_str()) {
                    return Err(SemanticContractError::Unsupported(
                        "array write to a name not bound on the linear path",
                    ));
                }
                expect_kind(
                    expression_kind(
                        index,
                        &inner,
                        result_in_scope,
                        allow_wrapping_arithmetic,
                        &BTreeSet::new(),
                        &inner_env,
                    )?,
                    ScalarKind::I64,
                )?;
                expect_kind(
                    expression_kind(
                        value,
                        &inner,
                        result_in_scope,
                        allow_wrapping_arithmetic,
                        &BTreeSet::new(),
                        &inner_env,
                    )?,
                    elem,
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
                    &inner_env,
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
                    &inner_env,
                )?;
            }
            _ => {
                return Err(SemanticContractError::Unsupported(
                    "statement outside a let/assignment/expression sequence",
                ));
            }
        }
    }
    Ok((inner, inner_assignable, inner_env))
}

/// The declared shape a dotted `q.f` assignment writes: `q` an
/// unshadowed struct binding and `f` one of its declared fields —
/// under `pure-i64-composite/v5` admission only. Under
/// `pure-i64-composite/v15` the path may keep descending through
/// struct-typed fields — `q.inner.a = v` writes the leaf `a` inside
/// `q`'s `inner` subtree; a mid-path segment naming a non-struct
/// field or an undeclared field fails the walk. `None` keeps the
/// ordinary assignable check: a non-dotted target, a deeper `a.b.c`
/// path under a lower admission, a scalar or enum binding, or an
/// undeclared field all fall through and fail there. An enum binding
/// resolves through `composites` but never through `structs`, so enum
/// field writes stay closed. Under `pure-i64-composite/v8` the target
/// may name a struct-typed field — a whole-subtree write — which the
/// callers handle as a composite rebind, never a scalar assignable.
fn field_write_shape<'a>(
    env: &KindEnv<'a>,
    scope: &BTreeMap<String, ScalarKind>,
    var: &str,
) -> Option<&'a ValueKind> {
    if !env.admit_field_writes {
        return None;
    }
    let mut segments = var.split('.').peekable();
    let base = segments.next()?;
    let type_name = env.composite_name(base, scope)?;
    // An enum-typed base takes a single payload-field segment —
    // `e.p = v`. The field must name a payload of exactly one declared
    // variant (`pure-i64-composite/v16`): a deeper path, a field no
    // variant declares, or a name two variants share stays closed.
    if let Some(variants) = env.enums.get(type_name) {
        let field_name = segments.next()?;
        if segments.next().is_some() || !env.admit_enum_writes {
            return None;
        }
        let (_, _, field) = enum_field_slot(variants, field_name)?;
        env.enum_writes_used.set(true);
        return Some(&field.shape);
    }
    let mut struct_name = type_name;
    let mut declared = env.structs.get(struct_name)?;
    let mut shape = None;
    let mut depth = 0usize;
    while let Some(segment) = segments.next() {
        // A leaf shape mid-path dead-ends the descent — the next
        // segment must not resolve against the enclosing struct's
        // declared fields.
        if shape.is_some() {
            return None;
        }
        let field = declared.iter().find(|f| f.name == segment)?;
        depth += 1;
        match &field.shape {
            ValueKind::Struct(child) if segments.peek().is_some() => {
                struct_name = child;
                declared = env.structs.get(struct_name)?;
            }
            shape_kind => {
                shape = Some(shape_kind);
            }
        }
    }
    if depth > 1 {
        if !env.admit_nested_writes {
            return None;
        }
        env.nested_writes_used.set(true);
    }
    shape
}

/// The declared struct name a field-access target carries, if the
/// target positions a composite value: a bound composite name, a
/// composite-producing call, or a deeper `a.b` chain reaching a
/// struct-typed field (`pure-i64-composite/v8`). `None` (not an
/// error) when the target yields no struct — callers decide whether
/// that position is closed.
fn composite_target_name<'a>(
    target: &Expr,
    scope: &BTreeMap<String, ScalarKind>,
    result_in_scope: bool,
    allow_wrapping_arithmetic: bool,
    assignable: &BTreeSet<String>,
    env: &KindEnv<'a>,
) -> Result<Option<&'a str>, SemanticContractError> {
    match target {
        Expr::Variable(name) => Ok(env.composite_name(name, scope)),
        Expr::Call(..) if env.admit_struct_values => composite_kind(
            target,
            scope,
            result_in_scope,
            allow_wrapping_arithmetic,
            assignable,
            env,
        )
        .map(Some),
        Expr::FieldAccess(inner, field) => {
            let Some(struct_name) = composite_target_name(
                inner,
                scope,
                result_in_scope,
                allow_wrapping_arithmetic,
                assignable,
                env,
            )?
            else {
                return Ok(None);
            };
            Ok(env
                .structs
                .get(struct_name)
                .and_then(|fields| fields.iter().find(|f| f.name == *field))
                .and_then(|f| match &f.shape {
                    ValueKind::Struct(child) => Some(child.as_str()),
                    _ => None,
                }))
        }
        _ => Ok(None),
    }
}

/// Whether a `let` initializer is composite-valued: a struct literal,
/// an unshadowed composite name, an enum constructor
/// (`pure-i64-composite/v6`), or a call to a composite-returning
/// sibling. `composite_kind` re-checks every case, so this only
/// decides which `let` arm examines the initializer.
fn is_composite_value(
    expr: &Expr,
    scope: &BTreeMap<String, ScalarKind>,
    env: &KindEnv<'_>,
) -> bool {
    match expr {
        Expr::StructInit { .. } => true,
        Expr::Variable(name) => env.composite_name(name, scope).is_some(),
        Expr::FieldAccess(target, field) => {
            // A nullary `E::V`, or — under v8 — a read whose declared
            // field is itself a struct (`o.inner`), or — under v17 —
            // an enum-typed field (`b.e`).
            (env.admit_enum_values && enum_ctor(target, field, scope, env).is_some())
                || composite_target_name(target, scope, true, false, &BTreeSet::new(), env)
                    .ok()
                    .flatten()
                    .and_then(|struct_name| env.structs.get(struct_name))
                    .and_then(|fields| fields.iter().find(|f| f.name == *field))
                    .is_some_and(|f| matches!(f.shape, ValueKind::Struct(_) | ValueKind::Enum(_)))
        }
        Expr::Call(name, _) => {
            (env.admit_enum_values && enum_call_ctor(name, env).is_some())
                || env.callees.get(name.as_str()).is_some_and(|sig| {
                    matches!(sig.return_kind, ValueKind::Struct(_))
                        || (env.admit_enum_values && matches!(sig.return_kind, ValueKind::Enum(_)))
                })
        }
        // A branch-merged composite is structural only: both branch
        // tails read as composite positions. Checking the type
        // agreement or setting usage flags here would leak a failed
        // probe into the profile name — `composite_kind` does both
        // when the position is actually bound.
        Expr::IfThenElse {
            then_branch,
            else_branch,
            ..
        } => {
            env.admit_merged_composites
                && is_composite_stmt(then_branch, scope, env)
                && is_composite_stmt(else_branch, scope, env)
        }
        Expr::Match { arms, .. } => {
            env.admit_merged_composites
                && !arms.is_empty()
                && arms
                    .iter()
                    .all(|arm| is_composite_stmt(&arm.body, scope, env))
        }
        _ => false,
    }
}

/// Whether a statement writes a composite-typed binding (`q.f = v` on
/// a struct binding, or a `q = ..` rebind of one) — the write a
/// branch merge must join (`pure-i64-composite/v7`), so an `if`/`match`
/// whose VALUE stays scalar still names the merged profile. A
/// branch-local `let` never reaches the merge: `let`s are scoped and
/// skipped here.
fn stmt_writes_composite(
    stmt: &Stmt,
    scope: &BTreeMap<String, ScalarKind>,
    env: &KindEnv<'_>,
) -> bool {
    match stmt {
        Stmt::Expr(expr, _) => expr_writes_composite(expr, scope, env),
        Stmt::Assign { var, .. } => {
            let base = var.split('.').next().unwrap_or(var.as_str());
            env.composite_name(base, scope).is_some()
        }
        // The bound name is branch-local, but the right-hand side can
        // still nest a branch that writes an OUTER composite binding.
        Stmt::Let { value, .. } => expr_writes_composite(value, scope, env),
        Stmt::Block(stmts, _) => stmts
            .iter()
            .any(|inner| stmt_writes_composite(inner, scope, env)),
        _ => false,
    }
}

/// `stmt_writes_composite` through the expression forms that hold
/// statements (`if` branches, `match` arms, a `Block` expr) and the
/// subexpressions that can nest them.
fn expr_writes_composite(
    expr: &Expr,
    scope: &BTreeMap<String, ScalarKind>,
    env: &KindEnv<'_>,
) -> bool {
    match expr {
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            expr_writes_composite(cond, scope, env)
                || stmt_writes_composite(then_branch, scope, env)
                || stmt_writes_composite(else_branch, scope, env)
        }
        Expr::Match { target, arms } => {
            expr_writes_composite(target, scope, env)
                || arms
                    .iter()
                    .any(|arm| stmt_writes_composite(&arm.body, scope, env))
        }
        Expr::Block(stmt) => stmt_writes_composite(stmt, scope, env),
        Expr::BinaryOp(a, _, b) => {
            expr_writes_composite(a, scope, env) || expr_writes_composite(b, scope, env)
        }
        Expr::Call(_, args) => args
            .iter()
            .any(|arg| expr_writes_composite(arg, scope, env)),
        Expr::StructInit { fields, .. } => fields
            .iter()
            .any(|(_, value)| expr_writes_composite(value, scope, env)),
        Expr::FieldAccess(target, _) => expr_writes_composite(target, scope, env),
        Expr::ArrayLit(items) => items
            .iter()
            .any(|item| expr_writes_composite(item, scope, env)),
        Expr::ArrayAccess(_, index) => expr_writes_composite(index, scope, env),
        _ => false,
    }
}

/// Whether a statement's tail produces a composite value: an
/// expression statement or a block's last statement, recursively.
fn is_composite_stmt(stmt: &Stmt, scope: &BTreeMap<String, ScalarKind>, env: &KindEnv<'_>) -> bool {
    match stmt {
        Stmt::Expr(expr, _) => is_composite_value(expr, scope, env),
        Stmt::Block(stmts, _) => stmts
            .last()
            .is_some_and(|last| is_composite_stmt(last, scope, env)),
        _ => false,
    }
}

/// The enum a qualified `E::V` constructor names, spelled as a nullary
/// `FieldAccess` on the enum name — `None` when `target`/`field` does
/// not name a variant of an admitted enum.
fn enum_ctor<'a>(
    target: &Expr,
    field: &str,
    scope: &BTreeMap<String, ScalarKind>,
    env: &KindEnv<'a>,
) -> Option<(&'a str, &'a ResolvedVariant)> {
    let Expr::Variable(enum_name) = target else {
        return None;
    };
    // A bound name wins over the enum namespace: `Shape.x` where a
    // parameter or `let` is also named `Shape` reads that binding's
    // field, never a nullary variant constructor.
    if scope.contains_key(enum_name.as_str()) || env.composites.contains_key(enum_name.as_str()) {
        return None;
    }
    let (resolved_name, variants) = env.enums.get_key_value(enum_name.as_str())?;
    let variant = variants.iter().find(|v| v.name == field)?;
    Some((resolved_name.as_str(), variant))
}

/// The enum a call-spelled `E::V(args..)` constructor names — `None`
/// when `name` is not `Enum::Variant` for an admitted enum.
fn enum_call_ctor<'a>(name: &str, env: &KindEnv<'a>) -> Option<(&'a str, &'a ResolvedVariant)> {
    // A callee of the same qualified spelling (an impl method on a
    // struct sharing the enum's name) wins — it is real code the call
    // machinery dispatches, not a constructor.
    if env.callees.contains_key(name) {
        return None;
    }
    let (enum_name, variant_name) = name.split_once("::")?;
    let (resolved_name, variants) = env.enums.get_key_value(enum_name)?;
    let variant = variants.iter().find(|v| v.name == variant_name)?;
    Some((resolved_name.as_str(), variant))
}

/// `composite_kind` for a resolved enum constructor: arity and each
/// payload argument's scalar kind must match the variant's declared
/// fields exactly.
fn enum_ctor_kind<'a>(
    args: &[Expr],
    variant: &ResolvedVariant,
    scope: &BTreeMap<String, ScalarKind>,
    result_in_scope: bool,
    allow_wrapping_arithmetic: bool,
    env: &KindEnv<'a>,
) -> Result<(), SemanticContractError> {
    if args.len() != variant.fields.len() {
        return Err(SemanticContractError::Unsupported("enum constructor arity"));
    }
    for (arg, field) in args.iter().zip(variant.fields.iter()) {
        match &field.shape {
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
            // A composite payload argument must itself produce that
            // struct value (`pure-i64-composite/v18`).
            ValueKind::Struct(struct_name) => {
                let produced = composite_kind(
                    arg,
                    scope,
                    result_in_scope,
                    allow_wrapping_arithmetic,
                    &BTreeSet::new(),
                    env,
                )?;
                if produced != struct_name.as_str() {
                    return Err(SemanticContractError::Unsupported(
                        "enum constructor payload kind",
                    ));
                }
            }
            ValueKind::Enum(_) => {
                unreachable!("enum-typed payloads are never admitted")
            }
        }
    }
    env.enum_values_used.set(true);
    Ok(())
}

/// `statement_kind` for a struct-valued position (an atom's struct
/// result): a sequential block whose tail produces a struct value.
fn statement_composite_kind<'a>(
    stmt: &Stmt,
    scope: &BTreeMap<String, ScalarKind>,
    result_in_scope: bool,
    allow_wrapping_arithmetic: bool,
    assignable: &BTreeSet<String>,
    env: &KindEnv<'a>,
) -> Result<&'a str, SemanticContractError> {
    match stmt {
        Stmt::Expr(expr, _) => composite_kind(
            expr,
            scope,
            result_in_scope,
            allow_wrapping_arithmetic,
            assignable,
            env,
        ),
        Stmt::Block(stmts, _) if stmts.len() == 1 => statement_composite_kind(
            &stmts[0],
            scope,
            result_in_scope,
            allow_wrapping_arithmetic,
            assignable,
            env,
        ),
        Stmt::Block(stmts, _) if !stmts.is_empty() && allow_wrapping_arithmetic => {
            let (lets, last) = stmts.split_at(stmts.len() - 1);
            let (inner, inner_assignable, inner_env) = block_prefix_kinds(
                lets,
                scope,
                result_in_scope,
                allow_wrapping_arithmetic,
                assignable,
                env,
            )?;
            statement_composite_kind(
                &last[0],
                &inner,
                result_in_scope,
                allow_wrapping_arithmetic,
                &inner_assignable,
                &inner_env,
            )
        }
        _ => Err(SemanticContractError::Unsupported(
            "struct value outside the composite profile",
        )),
    }
}

/// The struct a struct-valued expression produces. Before
/// `pure-i64-composite/v3` only an unshadowed struct name qualifies (a
/// parameter passed through). v3 adds a struct literal initializing
/// every declared field exactly once from a scalar expression of the
/// field's kind, and a call to a struct-returning sibling; v6 adds enum
/// construction and enum-returning calls, and v7 adds `if`/`match`
/// expressions whose branches all produce the same composite type —
/// the aggregate `phi` the emitter writes. Nested struct literals and
/// anything else stay outside.
fn composite_kind<'a>(
    expr: &Expr,
    scope: &BTreeMap<String, ScalarKind>,
    result_in_scope: bool,
    allow_wrapping_arithmetic: bool,
    assignable: &BTreeSet<String>,
    env: &KindEnv<'a>,
) -> Result<&'a str, SemanticContractError> {
    match expr {
        Expr::Variable(name) => {
            env.composite_name(name, scope)
                .ok_or(SemanticContractError::Unsupported(
                    "struct argument outside parameter pass-through",
                ))
        }
        Expr::StructInit { type_name, fields } if env.admit_struct_values => {
            let Some((struct_name, declared)) = env.structs.get_key_value(type_name.as_str())
            else {
                return Err(SemanticContractError::Unsupported(
                    "struct literal of an undeclared struct",
                ));
            };
            if fields.len() != declared.len()
                || declared
                    .iter()
                    .any(|f| fields.iter().filter(|(name, _)| *name == f.name).count() != 1)
            {
                return Err(SemanticContractError::Unsupported(
                    "struct literal must set every declared field exactly once",
                ));
            }
            for (name, value) in fields {
                let Some(field) = declared.iter().find(|f| f.name == *name) else {
                    return Err(SemanticContractError::Unsupported(
                        "struct literal must set every declared field exactly once",
                    ));
                };
                match &field.shape {
                    ValueKind::Scalar(kind) => expect_kind(
                        expression_kind(
                            value,
                            scope,
                            result_in_scope,
                            allow_wrapping_arithmetic,
                            &BTreeSet::new(),
                            env,
                        )?,
                        *kind,
                    )?,
                    // A struct-typed field takes a composite argument of
                    // the same declared struct (`pure-i64-composite/v8`).
                    ValueKind::Struct(child) => {
                        let produced = composite_kind(
                            value,
                            scope,
                            result_in_scope,
                            allow_wrapping_arithmetic,
                            &BTreeSet::new(),
                            env,
                        )?;
                        if produced != child.as_str() {
                            return Err(SemanticContractError::Unsupported(
                                "struct literal field type mismatch",
                            ));
                        }
                    }
                    // An enum-typed field takes a composite argument
                    // of the same declared enum
                    // (`pure-i64-composite/v17`).
                    ValueKind::Enum(child) => {
                        let produced = composite_kind(
                            value,
                            scope,
                            result_in_scope,
                            allow_wrapping_arithmetic,
                            &BTreeSet::new(),
                            env,
                        )?;
                        if produced != child.as_str() {
                            return Err(SemanticContractError::Unsupported(
                                "struct literal field type mismatch",
                            ));
                        }
                    }
                }
            }
            env.struct_values_used.set(true);
            Ok(struct_name.as_str())
        }
        Expr::Call(name, args)
            if allow_wrapping_arithmetic && (env.admit_struct_values || env.admit_enum_values) =>
        {
            if env.admit_enum_values {
                if let Some((enum_name, variant)) = enum_call_ctor(name, env) {
                    enum_ctor_kind(
                        args,
                        variant,
                        scope,
                        result_in_scope,
                        allow_wrapping_arithmetic,
                        env,
                    )?;
                    return Ok(enum_name);
                }
            }
            let sig = check_call(
                name,
                args,
                scope,
                result_in_scope,
                allow_wrapping_arithmetic,
                assignable,
                env,
            )?;
            match &sig.return_kind {
                ValueKind::Enum(result) if env.admit_enum_values => {
                    let Some((enum_name, _)) = env.enums.get_key_value(result.as_str()) else {
                        return Err(SemanticContractError::Unsupported(
                            "enum value outside the composite profile",
                        ));
                    };
                    env.enum_values_used.set(true);
                    Ok(enum_name.as_str())
                }
                ValueKind::Struct(result) if env.admit_struct_values => {
                    let Some((struct_name, _)) = env.structs.get_key_value(result.as_str()) else {
                        return Err(SemanticContractError::Unsupported(
                            "struct value outside the composite profile",
                        ));
                    };
                    env.struct_values_used.set(true);
                    Ok(struct_name.as_str())
                }
                _ => Err(SemanticContractError::Unsupported(
                    "scalar call in a composite position",
                )),
            }
        }
        Expr::FieldAccess(target, field) if env.admit_enum_values => {
            // A nullary `E::V` parses as field access on the enum name.
            if let Some((enum_name, variant)) = enum_ctor(target, field, scope, env) {
                enum_ctor_kind(
                    &[],
                    variant,
                    scope,
                    result_in_scope,
                    allow_wrapping_arithmetic,
                    env,
                )?;
                return Ok(enum_name);
            }
            // An enum-typed field read `b.e` produces the whole enum
            // value (`pure-i64-composite/v17`); like any enum value it
            // passes through, merges, and serves as a `match`
            // scrutinee — no payload projection exists to kind-check.
            let Some(struct_name) = composite_target_name(
                target,
                scope,
                result_in_scope,
                allow_wrapping_arithmetic,
                assignable,
                env,
            )?
            else {
                return Err(SemanticContractError::Unsupported(
                    "enum constructor outside the composite profile",
                ));
            };
            env.structs
                .get(struct_name)
                .and_then(|fields| fields.iter().find(|f| f.name == *field))
                .and_then(|f| match &f.shape {
                    ValueKind::Enum(child) => Some(child.as_str()),
                    _ => None,
                })
                .ok_or(SemanticContractError::Unsupported(
                    "enum constructor outside the composite profile",
                ))
        }
        // A struct-typed field read (`o.inner`, `pure-i64-composite/v8`)
        // produces the nested struct — a composite value that passes
        // through, merges or initializes like any other.
        Expr::FieldAccess(target, field) => {
            let Some(struct_name) = composite_target_name(
                target,
                scope,
                result_in_scope,
                allow_wrapping_arithmetic,
                assignable,
                env,
            )?
            else {
                return Err(SemanticContractError::Unsupported(
                    "field read outside a struct value",
                ));
            };
            let Some(child) = env
                .structs
                .get(struct_name)
                .and_then(|fields| fields.iter().find(|f| f.name == *field))
                .and_then(|f| match &f.shape {
                    ValueKind::Struct(child) => Some(child.as_str()),
                    _ => None,
                })
            else {
                return Err(SemanticContractError::Unsupported(
                    "scalar field read in a composite position",
                ));
            };
            env.struct_values_used.set(true);
            Ok(child)
        }
        // A branch-merged composite (`pure-i64-composite/v7`): both
        // tails produce the same composite type and the encoder merges
        // the flattened slots under `ite` — the aggregate `phi` the
        // emitter writes. Branch bodies may still rebind names; the
        // write merge runs in the source layer.
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } if env.admit_merged_composites && allow_wrapping_arithmetic => {
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
            let then_name = statement_composite_kind(
                then_branch,
                scope,
                result_in_scope,
                allow_wrapping_arithmetic,
                assignable,
                env,
            )?;
            let else_name = statement_composite_kind(
                else_branch,
                scope,
                result_in_scope,
                allow_wrapping_arithmetic,
                assignable,
                env,
            )?;
            if then_name != else_name {
                return Err(SemanticContractError::Unsupported(
                    "branch-merged composite types",
                ));
            }
            env.merged_composites_used.set(true);
            Ok(then_name)
        }
        Expr::Match { target, arms }
            if env.admit_merged_composites && allow_wrapping_arithmetic =>
        {
            let arm_envs = match_arm_envs(
                target,
                arms,
                scope,
                result_in_scope,
                allow_wrapping_arithmetic,
                assignable,
                env,
            )?;
            let mut name = None;
            for (arm, (arm_scope, arm_assignable, arm_env)) in arms.iter().zip(arm_envs) {
                let arm_name = statement_composite_kind(
                    &arm.body,
                    &arm_scope,
                    result_in_scope,
                    allow_wrapping_arithmetic,
                    &arm_assignable,
                    &arm_env,
                )?;
                match name {
                    None => name = Some(arm_name),
                    Some(first) if first == arm_name => (),
                    Some(_) => {
                        return Err(SemanticContractError::Unsupported(
                            "match-merged composite types",
                        ));
                    }
                }
            }
            let name = name.ok_or(SemanticContractError::Unsupported("empty match"))?;
            env.merged_composites_used.set(true);
            Ok(name)
        }
        _ => Err(SemanticContractError::Unsupported(
            "struct value outside the composite profile",
        )),
    }
}

/// Resolve a call against the module's callee map and check each
/// argument against the callee parameter's declared shape: a scalar
/// parameter admits a scalar expression of its kind, a struct parameter
/// a struct value of the same struct (`composite_kind`).
fn check_call<'a>(
    name: &str,
    args: &[Expr],
    scope: &BTreeMap<String, ScalarKind>,
    result_in_scope: bool,
    allow_wrapping_arithmetic: bool,
    assignable: &BTreeSet<String>,
    env: &KindEnv<'a>,
) -> Result<&'a CalleeSig, SemanticContractError> {
    // A call must name an atom of this module with a matching
    // arity: an undeclared or mismatched callee has no
    // counterpart to compare a call site against.
    let Some(sig) = env.callees.get(name) else {
        return Err(SemanticContractError::Unsupported(
            "call target outside module",
        ));
    };
    if sig.param_kinds.len() != args.len() {
        return Err(SemanticContractError::Unsupported(
            "call target outside module",
        ));
    }
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
            ValueKind::Struct(expected) | ValueKind::Enum(expected) => {
                if composite_kind(
                    arg,
                    scope,
                    result_in_scope,
                    allow_wrapping_arithmetic,
                    assignable,
                    env,
                )
                .ok()
                    != Some(expected.as_str())
                {
                    return Err(SemanticContractError::Unsupported(
                        "struct argument outside parameter pass-through",
                    ));
                }
            }
        }
    }
    Ok(sig)
}

/// The scrutinee, pattern-vocabulary, catch-all and arm-guard checks a
/// `match` runs under the composite profile — returning each arm's
/// (`arm_scope`, `arm_assignable`) so the scalar and composite paths
/// admit the same match shapes and only differ in what the arm bodies
/// produce.
///
/// An enum scrutinee flips the arm vocabulary: `Variant` patterns bind
/// the payload slots of the variant they name, `Wildcard` is the
/// trailing catch-all, and literals or a variable-binding arm stay
/// out — the flattened enum value has no scalar to bind and
/// construction is never observed. An `i64` scrutinee takes `Literal`
/// patterns plus a trailing `Wildcard`/`Variable` catch-all.
#[allow(clippy::type_complexity)]
fn match_arm_envs<'a>(
    target: &Expr,
    arms: &[MatchArm],
    scope: &BTreeMap<String, ScalarKind>,
    result_in_scope: bool,
    allow_wrapping_arithmetic: bool,
    assignable: &BTreeSet<String>,
    env: &KindEnv<'a>,
) -> Result<Vec<(BTreeMap<String, ScalarKind>, BTreeSet<String>, KindEnv<'a>)>, SemanticContractError>
{
    let enum_scrutinee = match target {
        Expr::Variable(name) => env
            .composite_name(name, scope)
            .and_then(|type_name| env.enums.get(type_name)),
        // `match b.e` — an enum-typed field read
        // (`pure-i64-composite/v17`) — destructures like an enum
        // parameter scrutinee.
        Expr::FieldAccess(inner, field) => composite_target_name(
            inner,
            scope,
            result_in_scope,
            allow_wrapping_arithmetic,
            assignable,
            env,
        )
        .ok()
        .flatten()
        .and_then(|struct_name| env.structs.get(struct_name))
        .and_then(|fields| fields.iter().find(|f| f.name == *field))
        .and_then(|f| match &f.shape {
            ValueKind::Enum(enum_name) => env.enums.get(enum_name),
            _ => None,
        }),
        _ => None,
    };
    let mut out = Vec::with_capacity(arms.len());
    if let Some(variants) = enum_scrutinee {
        let Some(last) = arms.last() else {
            return Err(SemanticContractError::Unsupported("empty match"));
        };
        if !matches!(last.pattern, Pattern::Wildcard) || last.guard.is_some() {
            return Err(SemanticContractError::Unsupported(
                "match without a catch-all arm",
            ));
        }
        for arm in arms {
            let mut arm_scope = scope.clone();
            let mut arm_assignable = assignable.clone();
            // Composite payload binders need their own env entry —
            // `bind_composite` reserves the synthesized leaf names
            // (`pure-i64-composite/v18`). The clone shares the flag
            // cells, so profile markers still propagate.
            let mut arm_env = env.clone();
            match &arm.pattern {
                Pattern::Wildcard => {}
                Pattern::Variant {
                    variant_name,
                    fields,
                } => {
                    let short = variant_name.rsplit("::").next().unwrap_or(variant_name);
                    let Some(variant) = variants.iter().find(|v| v.name == short) else {
                        return Err(SemanticContractError::Unsupported(
                            "match variant outside enum",
                        ));
                    };
                    if variant.fields.len() != fields.len() {
                        return Err(SemanticContractError::Unsupported("match variant arity"));
                    }
                    for (field_pattern, field) in fields.iter().zip(&variant.fields) {
                        match field_pattern {
                            Pattern::Variable(name) => {
                                match &field.shape {
                                    ValueKind::Scalar(kind) => {
                                        env.bind(name)?;
                                        arm_scope.insert(name.clone(), *kind);
                                    }
                                    ValueKind::Struct(type_name) => {
                                        arm_env.bind_composite(name, type_name)?;
                                        // A bound composite shadows any
                                        // scalar of the same name in this
                                        // arm (`f(p, e) { E::V(p) => p.x }`):
                                        // `composite_name` prefers `scope`,
                                        // so a stale scalar entry would
                                        // make `p.x` an unbound leaf read.
                                        arm_scope.remove(name);
                                    }
                                    ValueKind::Enum(_) => {
                                        unreachable!("enum-typed payloads are never admitted")
                                    }
                                }
                                arm_assignable.remove(name);
                            }
                            Pattern::Wildcard => (),
                            _ => {
                                return Err(SemanticContractError::Unsupported(
                                    "match variant field pattern",
                                ));
                            }
                        }
                    }
                }
                _ => {
                    return Err(SemanticContractError::Unsupported(
                        "match pattern outside variants and catch-all",
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
                        &arm_env,
                    )?,
                    ScalarKind::Bool,
                )?;
            }
            out.push((arm_scope, arm_assignable, arm_env));
        }
        return Ok(out);
    }
    let scrutinee_kind = expression_kind(
        target,
        scope,
        result_in_scope,
        allow_wrapping_arithmetic,
        &BTreeSet::new(),
        env,
    )?;
    // An integer scrutinee — `i64` or `u64` (`pure-i64-composite/v10`),
    // literals adapt to either; arm bindings carry the scrutinee's kind
    // so a bound `u64` arm variable keeps unsigned semantics.
    if !matches!(
        scrutinee_kind,
        ScalarKind::I64 | ScalarKind::U64 | ScalarKind::Num
    ) {
        return Err(SemanticContractError::Unsupported("match scrutinee kind"));
    }
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
    for arm in arms {
        let mut arm_scope = scope.clone();
        let mut arm_assignable = assignable.clone();
        match &arm.pattern {
            Pattern::Literal(_) | Pattern::Wildcard => {}
            Pattern::Variable(name) => {
                env.bind(name)?;
                arm_scope.insert(name.clone(), scrutinee_kind);
                arm_assignable.remove(name);
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
        out.push((arm_scope, arm_assignable, env.clone()));
    }
    Ok(out)
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
        // Literals are kind-adaptive (`Num`): a `u64` operand context
        // and an `i64` one both accept them, so `a(u64) >= 0` kind-checks
        // like `a(i64) >= 0` does — with no numeric-suffix syntax.
        Expr::Number(_) => Ok(ScalarKind::Num),
        // A `Str` literal (`pure-i64-composite/v13`): a bounded inline
        // byte sequence, immutable, at most `STR_BOUND` cells under
        // the bounded-memory model.
        Expr::StringLit(s)
            if allow_wrapping_arithmetic && env.admit_str && s.len() <= env.str_bound =>
        {
            env.strs_used.set(true);
            env.arrays_used.set(true);
            Ok(ScalarKind::Str)
        }
        Expr::Variable(name) if matches!(name.as_str(), "true" | "false") => Ok(ScalarKind::Bool),
        // Lexical bindings (including a shadowing `let result`) win over
        // the implicit result name, so the kind checker and the BV encoder
        // see the same binding.
        Expr::Variable(name) => scope
            .get(name.as_str())
            .copied()
            .or((result_in_scope && name == "result").then_some(env.result_kind))
            .ok_or(SemanticContractError::Unsupported(
                "expression or unbound name",
            )),
        Expr::BinaryOp(left, op, right) => {
            // Enum equality (`pure-i64-composite/v14`): `e == e'` on
            // two values of one declared enum yields `bool` — the
            // whole tagged-union layout compares positionally, the
            // shape the emitter lowers it to. Only `==`/`!=`; ordering
            // and arithmetic on enum values stay outside.
            if matches!(op, Op::Eq | Op::Neq) && env.admit_enum_scalar_ops {
                if let (Ok(left_name), Ok(right_name)) = (
                    composite_kind(
                        left,
                        scope,
                        result_in_scope,
                        allow_wrapping_arithmetic,
                        &BTreeSet::new(),
                        env,
                    ),
                    composite_kind(
                        right,
                        scope,
                        result_in_scope,
                        allow_wrapping_arithmetic,
                        &BTreeSet::new(),
                        env,
                    ),
                ) {
                    if left_name == right_name && env.enums.contains_key(left_name) {
                        env.enum_scalar_ops_used.set(true);
                        return Ok(ScalarKind::Bool);
                    }
                }
            }
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
            // Literal kinds adapt to the operand they meet: the
            // operand kind a binary expression carries — `Num`
            // appearing on both sides resolves to `i64` (literal
            // arithmetic is signed until a `u64` context fixes it).
            let unified = match (left, right) {
                // A `Num` literal never adapts to `f64` — float
                // literals are outside the fragment, so `a + 1` fails
                // here rather than encoding `1` as an integer leaf.
                (ScalarKind::Num, k) | (k, ScalarKind::Num) if k != ScalarKind::F64 => k,
                (l, r) if l == r => l,
                _ => {
                    return Err(SemanticContractError::Unsupported(
                        "operator or operand kinds",
                    ))
                }
            };
            // Literal-only arithmetic keeps `Num` so `0 - 1` still
            // adapts to a `u64` context it meets higher up; `Num` in
            // `unified` reads as adaptive-i64 in the op arms below.

            match op {
                Op::Eq | Op::Neq
                    if matches!(
                        unified,
                        ScalarKind::I64
                            | ScalarKind::U64
                            | ScalarKind::I128
                            | ScalarKind::U128
                            | ScalarKind::Num
                            | ScalarKind::Bool
                    ) && (left != ScalarKind::Bool || right == left)
                        && (right != ScalarKind::Bool || left == right) =>
                {
                    Ok(ScalarKind::Bool)
                }
                Op::Lt | Op::Le | Op::Gt | Op::Ge
                    if matches!(
                        unified,
                        ScalarKind::I64
                            | ScalarKind::U64
                            | ScalarKind::I128
                            | ScalarKind::U128
                            | ScalarKind::Num
                    ) =>
                {
                    Ok(ScalarKind::Bool)
                }
                Op::And | Op::Or | Op::Implies if left == ScalarKind::Bool && right == left => {
                    Ok(ScalarKind::Bool)
                }
                // i64 arithmetic wraps mod 2^64 — still no overflow claim.
                // Division, remainder and shifts are admitted as partial
                // operations: the caller's BV encoder records their defined
                // domain (nonzero divisor excluding i64::MIN / -1, shift
                // amount in 0..64) as explicit obligations. A `u64`
                // operand unifies the pair to unsigned: the encoder
                // selects `bvudiv`/`bvurem`/`bvlshr` for it and drops the
                // `i64::MIN / -1` exclusion, which cannot occur unsigned.
                // `i128`/`u128` pairs (`pure-i64-composite/v21`) run the
                // same BV ops one width up — `u128` unsigned, `i128`
                // signed, with the `i128::MIN / -1` exclusion at the
                // 128-bit constants.
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
                    if allow_wrapping_arithmetic
                        && matches!(
                            unified,
                            ScalarKind::I64
                                | ScalarKind::U64
                                | ScalarKind::I128
                                | ScalarKind::U128
                                | ScalarKind::Num
                        ) =>
                {
                    Ok(unified)
                }
                // `f64` arithmetic (`pure-i64-composite/v19`): real
                // IEEE 754 binary64 semantics — the same ops the
                // emitter lowers to `fadd`/`fsub`/`fmul`/`fdiv`.
                // `!=` and ordering stay outside: the emitter never
                // emits them for float operands.
                Op::Add | Op::Sub | Op::Mul | Op::Div
                    if env.admit_f64_arithmetic && unified == ScalarKind::F64 =>
                {
                    env.f64_arithmetic_used.set(true);
                    Ok(ScalarKind::F64)
                }
                // `x == y` on `f64` lowers to `fcmp oeq` — IEEE
                // equality with NaN unequal and +0.0/-0.0 equal.
                Op::Eq if env.admit_f64_arithmetic && unified == ScalarKind::F64 => {
                    env.f64_arithmetic_used.set(true);
                    Ok(ScalarKind::Bool)
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
            // condition like the emitter's per-variable phi. A write
            // to a composite binding joins aggregate slots — the v7
            // merge — even when the `if` value itself stays scalar.
            let then_kind = statement_kind(
                then_branch,
                scope,
                result_in_scope,
                allow_wrapping_arithmetic,
                assignable,
                env,
            )?;
            let else_kind = statement_kind(
                else_branch,
                scope,
                result_in_scope,
                allow_wrapping_arithmetic,
                assignable,
                env,
            )?;
            // `Num` is adaptive on either side: `{ 0 } else { x }`
            // yields the declared integer kind, same as a binary
            // operand — but only toward `I64`/`U64`, never `Bool`.
            let branch_kind = match (then_kind, else_kind) {
                (
                    ScalarKind::Num,
                    k @ (ScalarKind::I64 | ScalarKind::U64 | ScalarKind::I128 | ScalarKind::U128),
                )
                | (
                    k @ (ScalarKind::I64 | ScalarKind::U64 | ScalarKind::I128 | ScalarKind::U128),
                    ScalarKind::Num,
                ) => k,
                // A `Str` value stays out of the branch-value merge for
                // slice A (`pure-i64-composite/v13`): branch-local `let`
                // literals still index/read inside their arm, but the
                // merged `if` value admits no `Str` tail — matching the
                // emitter's pointer-`if` which carries no length slot.
                (ScalarKind::Str, _) | (_, ScalarKind::Str) => {
                    return Err(SemanticContractError::Unsupported("str branch merge"))
                }
                (l, r) if l == r => l,
                _ => return Err(SemanticContractError::Unsupported("scalar kind mismatch")),
            };
            if stmt_writes_composite(then_branch, scope, env)
                || stmt_writes_composite(else_branch, scope, env)
            {
                env.merged_composites_used.set(true);
            }
            Ok(branch_kind)
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
            // The scrutinee/pattern/catch-all/guard checks live in
            // `match_arm_envs`; each arm body must agree on one scalar
            // kind, like the emitter's match.merge.
            let arm_envs = match_arm_envs(
                target,
                arms,
                scope,
                result_in_scope,
                allow_wrapping_arithmetic,
                assignable,
                env,
            )?;
            let mut kind = None;
            for (arm, (arm_scope, arm_assignable, arm_env)) in arms.iter().zip(arm_envs) {
                let arm_kind = statement_kind(
                    &arm.body,
                    &arm_scope,
                    result_in_scope,
                    allow_wrapping_arithmetic,
                    &arm_assignable,
                    &arm_env,
                )?;
                match kind {
                    None => kind = Some(arm_kind),
                    Some(first) => {
                        kind = Some(match (first, arm_kind) {
                            (
                                ScalarKind::Num,
                                k @ (ScalarKind::I64
                                | ScalarKind::U64
                                | ScalarKind::I128
                                | ScalarKind::U128),
                            )
                            | (
                                k @ (ScalarKind::I64
                                | ScalarKind::U64
                                | ScalarKind::I128
                                | ScalarKind::U128),
                                ScalarKind::Num,
                            ) => k,
                            // As in the `if` merge: no `Str` arm value in
                            // slice A (`pure-i64-composite/v13`).
                            (ScalarKind::Str, _) | (_, ScalarKind::Str) => {
                                return Err(SemanticContractError::Unsupported("str arm merge"))
                            }
                            (l, r) if l == r => l,
                            _ => {
                                return Err(SemanticContractError::Unsupported(
                                    "scalar kind mismatch",
                                ))
                            }
                        });
                    }
                }
            }
            // As in the `if` merge: an arm writing a composite binding
            // joins aggregate slots under the arm's hit.
            for arm in arms {
                if stmt_writes_composite(&arm.body, scope, env) {
                    env.merged_composites_used.set(true);
                    break;
                }
            }
            kind.ok_or(SemanticContractError::Unsupported("empty match"))
        }
        // A fixed-size array literal (`bounded-memory`): every element
        // must kind-check and unify to one integer kind — `Num`
        // elements adapt like binary operands. Bool or empty literals
        // stay outside: a bool cell would emit an `i1` store the
        // bounded-memory profile does not model, and `[]` has no
        // element kind to unify.
        Expr::ArrayLit(elems)
            if allow_wrapping_arithmetic && env.admit_bounded_memory && !elems.is_empty() =>
        {
            let mut elem_kind = ScalarKind::Num;
            for elem in elems {
                let kind = expression_kind(
                    elem,
                    scope,
                    result_in_scope,
                    allow_wrapping_arithmetic,
                    &BTreeSet::new(),
                    env,
                )?;
                elem_kind = match (elem_kind, kind) {
                    (ScalarKind::Num, k) | (k, ScalarKind::Num) => k,
                    (l, r) if l == r => l,
                    _ => {
                        return Err(SemanticContractError::Unsupported(
                            "array literal element kinds",
                        ))
                    }
                };
            }
            match elem_kind {
                ScalarKind::I64 | ScalarKind::Num => {
                    env.arrays_used.set(true);
                    Ok(ScalarKind::ArrayI64)
                }
                ScalarKind::U64 => {
                    env.arrays_used.set(true);
                    Ok(ScalarKind::ArrayU64)
                }
                _ => Err(SemanticContractError::Unsupported(
                    "array literal element kinds",
                )),
            }
        }
        // An indexed read `a[i]` (`bounded-memory`): the name must be a
        // `let`-bound array, the index an `i64` (literals adapt). The
        // read carries the array's element kind.
        Expr::ArrayAccess(name, index) if allow_wrapping_arithmetic && env.admit_bounded_memory => {
            let elem = match scope.get(name.as_str()) {
                Some(ScalarKind::ArrayI64) => ScalarKind::I64,
                Some(ScalarKind::ArrayU64) => ScalarKind::U64,
                // A `Str` index read yields the cell's byte — a `u64`
                // leaf (`pure-i64-composite/v13`).
                Some(ScalarKind::Str) if env.admit_str => ScalarKind::U64,
                _ => {
                    return Err(SemanticContractError::Unsupported(
                        "array access on a non-array binding",
                    ))
                }
            };
            expect_kind(
                expression_kind(
                    index,
                    scope,
                    result_in_scope,
                    allow_wrapping_arithmetic,
                    &BTreeSet::new(),
                    env,
                )?,
                ScalarKind::I64,
            )?;
            env.arrays_used.set(true);
            Ok(elem)
        }
        Expr::Call(name, args) if allow_wrapping_arithmetic => {
            // `len(a)` reads the stored element count as `i64` — the
            // emitted `arrlen` slot load. A scalar argument falls
            // through to the ordinary callee check, so an atom named
            // `len` keeps resolving as a call.
            if name == "len" && args.len() == 1 && env.admit_bounded_memory {
                if let Ok(kind) = expression_kind(
                    &args[0],
                    scope,
                    result_in_scope,
                    allow_wrapping_arithmetic,
                    &BTreeSet::new(),
                    env,
                ) {
                    if matches!(kind, ScalarKind::ArrayI64 | ScalarKind::ArrayU64)
                        || (kind == ScalarKind::Str && env.admit_str)
                    {
                        env.arrays_used.set(true);
                        return Ok(ScalarKind::I64);
                    }
                }
            }
            // Each argument carries the callee parameter's declared
            // kind — a widened bool parameter admits only bool
            // arguments — and the call takes the callee's return kind.
            // A struct-returning call is never a scalar; it is read
            // through a field access (`f(x).g`) or bound by a `let`.
            match &check_call(
                name,
                args,
                scope,
                result_in_scope,
                allow_wrapping_arithmetic,
                assignable,
                env,
            )?
            .return_kind
            {
                ValueKind::Scalar(kind) => Ok(*kind),
                ValueKind::Struct(_) | ValueKind::Enum(_) => Err(
                    SemanticContractError::Unsupported("struct value in a scalar position"),
                ),
            }
        }
        // A field read names an unshadowed struct parameter directly and
        // takes the declared leaf field's kind; `o.inner.a` descends
        // through the nested struct (`pure-i64-composite/v8`), while a
        // struct-valued field in a scalar position, a computed or
        // unknown target stays outside.
        Expr::FieldAccess(target, field) => {
            let struct_name = composite_target_name(
                target,
                scope,
                result_in_scope,
                allow_wrapping_arithmetic,
                assignable,
                env,
            )?;
            struct_name
                .and_then(|name| env.structs.get(name))
                .and_then(|fields| fields.iter().find(|f| f.name == *field))
                .and_then(|f| f.leaf_kind())
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
    /// The declared parameter shapes in order: `None` for a scalar
    /// parameter (one `inputs` slot), `Some(n)` for a struct parameter
    /// flattened into the next `n` slots in field declaration order —
    /// the layout a consumer pairs with an aggregate formal.
    pub param_shapes: Vec<Option<usize>>,
    /// The declared result shape: `None` for a scalar result (`value` is
    /// one i64), `Some(n)` for a struct result whose `value` is its `n`
    /// field slots concatenated in declaration order (field 0 in the
    /// most significant 64 bits, see `aggregate_value`).
    pub result_shape: Option<usize>,
    /// Per-parameter leaf kinds in declaration order, flattened exactly
    /// like `param_shapes`: `true` for a pass-through `f64` leaf
    /// (`pure-i64-composite/v11`), `false` for an integer leaf (`i64`,
    /// `bool`, `u64` — one LLVM `i64` each). The LLVM side pairs these
    /// with the shapes so an i64/f64 ABI swap cannot pass a
    /// leaf-count-only check.
    pub param_leaf_doubles: Vec<Vec<bool>>,
    /// The result's leaf kinds under the same convention: one entry for
    /// a scalar result, one per flattened slot for an aggregate result.
    /// Empty when the atom declares no return type.
    pub result_leaf_doubles: Vec<bool>,
    /// When the body contains a `while`, the conjunction of each loop's
    /// bounded-exit condition: `true` only on inputs where every loop's
    /// guard fails within `LOOP_BOUND` unrolled iterations. Equivalence
    /// is claimed only inside that domain — a path still iterating at
    /// the bound is outside the profile, matching the LLVM side's
    /// bounded path drop. `None` for bodies without a `while`.
    pub loop_bound_exits: Option<Bool<'ctx>>,
    /// Composite values top-level writes rebound — the post-state of a
    /// struct parameter or result binding after a `q.f = v` write.
    /// `ensures` reads these (the verifier and the emitted code both
    /// see post-write fields); scalar rebinds keep the established
    /// input-time view and stay out.
    post_writes: HashMap<String, SourceValue<'ctx>>,
    /// True when the module's atoms used bounded-memory vocabulary —
    /// array literals, indexed reads/writes or `len`. The IR encoder
    /// admits `alloca`/GEP/`load`/`store` shapes only for a model
    /// carrying this flag, and the report names the bounded-memory
    /// profile for it.
    pub bounded_memory: bool,
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
    let (source_sha256, mut items, refined, structs, enums, bounded_memory) =
        bounded_pure_i64_module(source, allow_wrapping_arithmetic, RefinedAdmission::Reject)?;
    let call_returns =
        call_return_kinds(&items, &refined, &structs, &enums, RefinedAdmission::Reject);
    let Some(Item::Atom(atom)) = items.pop() else {
        return Err(SemanticContractError::Unsupported("source atom"));
    };
    encode_atom_body(
        &atom,
        context,
        source_sha256,
        &refined,
        &structs,
        &enums,
        &call_returns,
        bounded_memory,
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
    let (source_sha256, items, refined, structs, enums, bounded_memory) =
        bounded_pure_i64_module(source, allow_wrapping_arithmetic, admission)?;
    let call_returns = call_return_kinds(&items, &refined, &structs, &enums, admission);
    items
        .iter()
        .filter_map(|item| match item {
            Item::Atom(atom) => Some(encode_atom_body(
                atom,
                context,
                source_sha256.clone(),
                &refined,
                &structs,
                &enums,
                &call_returns,
                bounded_memory,
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
) -> BodyResult<(String, Vec<Item>, RefinedEnv, StructEnv, EnumEnv, bool)> {
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
    let (items, refined, structs, enums, _, _, _, _, _, _, _, arrays, strs, _, _, _, _, _, _) =
        admit_pure_i64_atoms(source, allow_wrapping_arithmetic, admission)?;
    // The bounded profile keeps its single-atom arity gate here the same
    // way `extract_pure_i64_contract` applies it.
    if !allow_wrapping_arithmetic && items.len() != 1 {
        return Err(SemanticContractError::Unsupported(
            "expected exactly one atom",
        ));
    }
    Ok((
        digest(source.as_bytes()),
        items,
        refined,
        structs,
        enums,
        // A `Str` literal's byte cells live under the same bounded
        // stack-memory model as an admitted array literal.
        arrays || strs,
    ))
}

#[allow(clippy::too_many_arguments)]
fn encode_atom_body<'ctx>(
    atom: &crate::parser::ast::Atom,
    context: &'ctx Context,
    source_sha256: String,
    refined: &RefinedEnv,
    structs: &StructEnv,
    enums: &EnumEnv,
    call_returns: &CallShapes<'ctx>,
    bounded_memory: bool,
) -> BodyResult<PureI64Body<'ctx>> {
    // A struct parameter contributes one input per field, so the limit
    // bounds the flattened signature.
    let slots = signature_slots(&atom.params, structs, enums);
    if slots.len() > 16 {
        return Err(SemanticContractError::Unsupported("parameter limit"));
    }
    let body =
        parse_body_expr_checked(&atom.body_expr).map_err(|_| SemanticContractError::Parse)?;
    // A wide leaf consumes two consecutive 64-bit inputs — the same
    // weighted arity `param_env` counts.
    let input_slots: usize = slots.iter().map(signature_slot_weight).sum();
    let inputs: Vec<_> = (0..input_slots)
        .map(|i| BV::new_const(context, format!("input_{i}"), 64))
        .collect();
    let env: HashMap<String, SourceValue> =
        param_env(&atom.params, &inputs, context, refined, structs, enums)?;
    let top = Bool::from_bool(context, true);
    let mut definedness = Vec::new();
    let composite_signature = atom
        .params
        .iter()
        .any(|p| struct_fields(p, structs).is_some() || enum_variants(p, enums).is_some());
    let assignable: BTreeSet<String> = atom
        .params
        .iter()
        .filter(|p| struct_fields(p, structs).is_none() && enum_variants(p, enums).is_none())
        .map(|p| p.name.clone())
        .collect();
    // A `-> bool` body lowers to i64 carrying 0/1 — the emitter zexts
    // its i1 result, so the checked value is the same 0/1 materialization.
    // Top-level composite reassignments land in `top_writes`; the body
    // report carries them so `ensures` evaluates post-write fields like
    // the verifier and the emitted code do.
    let mut top_writes = HashMap::new();
    let mut loop_exits = Vec::new();
    let mut call_sites = Vec::new();
    call_returns
        .str_bound
        .set(atom.str_bound.unwrap_or(STR_BOUND));
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
        enums,
    )? {
        SourceValue::Integer(value)
        | SourceValue::Unsigned(value)
        | SourceValue::Number(value)
        | SourceValue::Wide(value)
        | SourceValue::UWide(value)
        | SourceValue::Float64(value) => value,
        SourceValue::Boolean(value) => {
            value.ite(&BV::from_u64(context, 1, 64), &BV::from_u64(context, 0, 64))
        }
        SourceValue::Array { .. } => {
            return Err(SemanticContractError::Unsupported(
                "array value in a result position",
            ))
        }
        // A struct result's value is its field slots concatenated in
        // declaration order — the layout the emitter's returned
        // aggregate decomposes into.
        SourceValue::Composite(fields) => {
            let mut leaves = Vec::new();
            for (_, field) in fields {
                lower_composite(field, context, &mut leaves)?;
            }
            aggregate_value(&leaves)
        }
        // An enum result's value is its tag slot followed by the shared
        // payload slots — the tagged-union layout the emitter's
        // returned aggregate decomposes into.
        SourceValue::Enum { tag, slots, .. } => aggregate_value(
            &std::iter::once(Ok(tag))
                .chain(slots.into_iter().map(|slot| lower_scalar(slot, context)))
                .collect::<BodyResult<Vec<_>>>()?,
        ),
    };
    let param_domain = signature_domain(&atom.params, &inputs, context, refined, structs, enums)?;
    let subrange_params = slots.iter().any(|slot| {
        matches!(
            slot,
            SignatureSlot::Scalar(Some("i8" | "i16" | "i32" | "u8" | "u16" | "u32"))
        )
    });
    let refined_signature = slots.iter().any(|slot| {
        let name = match slot {
            SignatureSlot::Scalar(name) | SignatureSlot::WideScalar(name) => *name,
            SignatureSlot::EnumField { type_name, .. }
            | SignatureSlot::EnumWideField { type_name, .. } => *type_name,
            SignatureSlot::EnumTag { .. } => None,
        };
        name.is_some_and(|n| refined.contains_key(n))
    }) || atom
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
        .flat_map(|site| {
            // A struct-valued call result carries the 0/1 convention on
            // each of its `bool` field slots.
            let slots: Vec<(BV, bool)> = match call_returns.result_fields(&site.callee) {
                Some(fields) => {
                    let leaf_kinds = call_returns.flattened_kinds(fields);
                    split_slots(&site.result, leaf_kinds.len())
                        .into_iter()
                        .zip(leaf_kinds)
                        .map(|(slot, kind)| (slot, kind == ScalarKind::Bool))
                        .collect()
                }
                None => match call_returns.result_enum(&site.callee) {
                    // An enum-valued call result carries the 0/1
                    // convention on each `bool` payload slot — the tag
                    // is always an i64 ordinal.
                    Some((_, variants)) => {
                        let kinds = call_returns.enum_layout_leaf_kinds(variants);
                        split_slots(&site.result, 1 + kinds.len())
                            .into_iter()
                            .zip(std::iter::once(&ScalarKind::I64).chain(kinds.iter()))
                            .map(|(slot, kind)| (slot, *kind == ScalarKind::Bool))
                            .collect()
                    }
                    None => vec![(site.result.clone(), call_returns.returns_bool(&site.callee))],
                },
            };
            slots
                .into_iter()
                .filter(|(_, bool_slot)| *bool_slot)
                .map(|(slot, _)| slot.bvule(&BV::from_u64(context, 1, 64)))
                .collect::<Vec<_>>()
        })
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
        param_shapes: atom
            .params
            .iter()
            .map(|p| {
                struct_fields(p, structs)
                    .map(|fields| {
                        flattened_struct_fields(structs, enums, fields)
                            .iter()
                            .map(|(_, slot)| signature_slot_weight(slot))
                            .sum::<usize>()
                    })
                    .or_else(|| {
                        enum_variants(p, enums).map(|v| 1 + enum_leaf_count(v, structs, enums))
                    })
            })
            .collect(),
        result_shape: atom.return_type.as_deref().and_then(|name| {
            structs
                .get(name)
                .map(|fields| {
                    flattened_struct_fields(structs, enums, fields)
                        .iter()
                        .map(|(_, slot)| signature_slot_weight(slot))
                        .sum::<usize>()
                })
                .or_else(|| {
                    enums
                        .get(name)
                        .map(|variants| 1 + enum_leaf_count(variants, structs, enums))
                })
        }),
        param_leaf_doubles: atom
            .params
            .iter()
            .map(|p| {
                struct_fields(p, structs)
                    .map(|fields| {
                        flattened_struct_fields(structs, enums, fields)
                            .iter()
                            .flat_map(|(_, slot)| {
                                // A 128-bit leaf is two 64-bit ABI slots
                                // (`pure-i64-composite/v21`) — never a
                                // `double` either way.
                                match slot {
                                    SignatureSlot::WideScalar(_)
                                    | SignatureSlot::EnumWideField { .. } => vec![false, false],
                                    _ => vec![slot_type_name(slot) == Some("f64")],
                                }
                            })
                            .collect()
                    })
                    .or_else(|| {
                        enum_variants(p, enums).map(|variants| {
                            let mut kinds = vec![false];
                            kinds.extend(
                                enum_leaf_paths(variants, structs, enums)
                                    .into_iter()
                                    .flat_map(|(_, type_name)| {
                                        // A wide payload leaf is two
                                        // 64-bit ABI slots — neither
                                        // is a `double`
                                        // (`pure-i64-composite/v22`).
                                        if matches!(type_name, Some("i128" | "u128")) {
                                            vec![false, false]
                                        } else {
                                            vec![type_name == Some("f64")]
                                        }
                                    }),
                            );
                            kinds
                        })
                    })
                    .unwrap_or_else(|| {
                        // A scalar 128-bit leaf pairs two 64-bit ABI
                        // slots — neither is a `double`.
                        if matches!(p.type_name.as_deref(), Some("i128") | Some("u128")) {
                            vec![false, false]
                        } else {
                            vec![p.type_name.as_deref() == Some("f64")]
                        }
                    })
            })
            .collect(),
        result_leaf_doubles: atom
            .return_type
            .as_deref()
            .map(|name| {
                structs
                    .get(name)
                    .map(|fields| {
                        flattened_struct_fields(structs, enums, fields)
                            .iter()
                            .flat_map(|(_, slot)| match slot {
                                SignatureSlot::WideScalar(_)
                                | SignatureSlot::EnumWideField { .. } => vec![false, false],
                                _ => vec![slot_type_name(slot) == Some("f64")],
                            })
                            .collect()
                    })
                    .or_else(|| {
                        enums.get(name).map(|variants| {
                            let mut kinds = vec![false];
                            kinds.extend(
                                enum_leaf_paths(variants, structs, enums)
                                    .into_iter()
                                    .flat_map(|(_, type_name)| {
                                        if matches!(type_name, Some("i128" | "u128")) {
                                            vec![false, false]
                                        } else {
                                            vec![type_name == Some("f64")]
                                        }
                                    }),
                            );
                            kinds
                        })
                    })
                    .unwrap_or_else(|| {
                        if matches!(name, "i128" | "u128") {
                            vec![false, false]
                        } else {
                            vec![name == "f64"]
                        }
                    })
            })
            .unwrap_or_default(),
        loop_bound_exits,
        bounded_memory,
        post_writes: top_writes
            .into_iter()
            .filter(|(_, value)| {
                matches!(value, SourceValue::Composite(_) | SourceValue::Array { .. })
            })
            .collect(),
    })
}

/// One flattened input slot of a parameter's lowered signature: a
/// scalar parameter contributes a `Scalar` slot carrying its declared
/// type name, an enum parameter contributes an `EnumTag` (the variant
/// ordinal, domain `0 <= tag < variants`) followed by one `EnumField`
/// per payload index up to the widest variant's arity — the tagged
/// union layout the emitter emits.
#[derive(Clone)]
enum SignatureSlot<'a> {
    Scalar(Option<&'a str>),
    /// A 128-bit leaf (`i128`/`u128`): one declared leaf occupying TWO
    /// consecutive 64-bit input slots (`pure-i64-composite/v21`). The
    /// variant keeps the pairing explicit — consumers that do not know
    /// about wide leaves fail closed instead of silently reading one
    /// half.
    WideScalar(Option<&'a str>),
    EnumTag {
        variants: u64,
    },
    /// One shared payload slot of an enum parameter: `carriers` lists
    /// the ordinals of the variants that declare a field at this index.
    EnumField {
        type_name: Option<&'a str>,
        carriers: Vec<u64>,
    },
    /// A 128-bit payload leaf (`pure-i64-composite/v22`): one declared
    /// payload leaf occupying TWO consecutive 64-bit input slots under
    /// the same carriers convention `EnumField` uses.
    EnumWideField {
        type_name: Option<&'a str>,
        // Wide leaves carry no range domain, so the carriers list is
        // model symmetry today — kept so the flattened signature
        // records the same metadata every payload leaf does.
        #[allow(dead_code)]
        carriers: Vec<u64>,
    },
}

/// The declared scalar type name of every flattened input slot, in
/// order: a scalar parameter is one slot, a struct parameter one slot
/// per field in declaration order, an enum parameter a tag slot plus
/// one slot per payload index (uniformly typed across variants —
/// `build_enum_env` rejects mixed-type slots).
fn signature_slots<'a>(
    params: &'a [crate::parser::ast::Param],
    structs: &'a StructEnv,
    enums: &'a EnumEnv,
) -> Vec<SignatureSlot<'a>> {
    params
        .iter()
        .flat_map(|p| match struct_fields(p, structs) {
            Some(fields) => flattened_struct_fields(structs, enums, fields)
                .into_iter()
                .map(|(_, slot)| slot)
                .collect(),
            None => match enum_variants(p, enums) {
                Some(variants) => {
                    let mut slots = vec![SignatureSlot::EnumTag {
                        variants: variants.len() as u64,
                    }];
                    // Each payload leaf gets its own slot — a
                    // composite slot expands to its struct's leaf
                    // names under the same carriers
                    // (`pure-i64-composite/v18`) and a wide leaf
                    // carries two input slots
                    // (`pure-i64-composite/v22`).
                    slots.extend((0..enum_max_arity(variants)).flat_map(|i| {
                        let carriers: Vec<u64> = variants
                            .iter()
                            .enumerate()
                            .filter(|(_, v)| v.fields.get(i).is_some())
                            .map(|(ordinal, _)| ordinal as u64)
                            .collect();
                        enum_slot_leaf_slots(variants, structs, enums, i, carriers)
                    }));
                    slots
                }
                None => match p.type_name.as_deref() {
                    Some("i128" | "u128") => {
                        vec![SignatureSlot::WideScalar(p.type_name.as_deref())]
                    }
                    _ => vec![SignatureSlot::Scalar(p.type_name.as_deref())],
                },
            },
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
    enums: &EnumEnv,
) -> BodyResult<Vec<Bool<'ctx>>> {
    let mut domain = Vec::new();
    // A struct field's declared scalar type bounds its flattened slot
    // the same way it would bound a scalar parameter of that type.
    let slots = signature_slots(params, structs, enums);
    // A wide leaf consumes two consecutive 64-bit inputs — the same
    // weighted arity `param_env` counts.
    if slots.iter().map(signature_slot_weight).sum::<usize>() != inputs.len() {
        return Err(SemanticContractError::Unsupported("source signature arity"));
    }
    // The tag slot of the enum parameter currently being walked — an
    // `EnumField` slot's domain binds only under tags whose variant
    // declares a field at that index; under every other tag the slot is
    // padding and unconstrained.
    let mut enum_tag: Option<&BV> = None;
    let mut inputs = inputs.iter();
    for slot in slots {
        if let SignatureSlot::WideScalar(_) | SignatureSlot::EnumWideField { .. } = slot {
            // A wide leaf occupies two consecutive 64-bit input slots,
            // each a full-width carrier — no range domain applies. An
            // enum payload's wide leaf keeps the enclosing `enum_tag`
            // (`pure-i64-composite/v22`).
            if matches!(slot, SignatureSlot::WideScalar(_)) {
                enum_tag = None;
            }
            if inputs.next().is_none() || inputs.next().is_none() {
                return Err(SemanticContractError::Unsupported("source signature arity"));
            }
            continue;
        }
        let Some(v) = inputs.next() else {
            return Err(SemanticContractError::Unsupported("source signature arity"));
        };
        match slot {
            SignatureSlot::EnumTag { variants } => {
                enum_tag = Some(v);
                // The tag carries the variant ordinal: bounded above by
                // the variant count (unsigned, so a negative bit pattern
                // fails too), matching the tagged union's domain.
                domain.push(v.bvult(&BV::from_u64(context, variants, 64)));
            }
            SignatureSlot::EnumField {
                type_name,
                carriers,
            } => {
                // A `bool` payload slot stays canonical under EVERY
                // tag: the source layer reads it as `v != 0` while the
                // emitter and LLVM pass the raw i64, so leaving the
                // padding value unconstrained under a non-carrier tag
                // would let a plain pass-through compare `ite(v != 0,
                // 1, 0)` against the raw `v`.
                if enum_slot_is_bool(type_name, refined) {
                    domain.push(v.bvule(&BV::from_u64(context, 1, 64)));
                }
                if carriers.is_empty() {
                    continue;
                }
                let Some(tag) = enum_tag else {
                    continue;
                };
                let carrier_tags = carriers
                    .iter()
                    .map(|ordinal| tag._eq(&BV::from_u64(context, *ordinal, 64)))
                    .collect::<Vec<_>>();
                let carries = Bool::or(context, &carrier_tags.iter().collect::<Vec<_>>());
                for conjunct in declared_slot_domain(type_name, v, context, refined)? {
                    domain.push(carries.implies(&conjunct));
                }
            }
            SignatureSlot::Scalar(type_name) => {
                enum_tag = None;
                domain.extend(declared_slot_domain(type_name, v, context, refined)?);
            }
            // Consumed above — a wide leaf takes two input slots.
            SignatureSlot::WideScalar(_) | SignatureSlot::EnumWideField { .. } => unreachable!(),
        }
    }
    Ok(domain)
}

/// The domain conjuncts a declared scalar type name contributes at one
/// flattened slot: literal range bounds plus a refined alias's base
/// bounds and declaration conjuncts. Shared by `signature_domain`'s
/// unconditional scalar slots and its tag-conditional enum payload
/// slots, and by `result_conjuncts_at`'s enum result arm.
fn declared_slot_domain<'ctx>(
    type_name: Option<&str>,
    v: &BV<'ctx>,
    context: &'ctx Context,
    refined: &RefinedEnv,
) -> BodyResult<Vec<Bool<'ctx>>> {
    let mut domain = Vec::new();
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
            &CallShapes::default(),
            &EnumEnv::new(),
        )?
        .boolean()?;
        out.extend(definedness);
        out.extend(exits);
        out.push(encoded);
    }
    Ok(out)
}

/// `refined_conjuncts_at` for a declared result type: a struct result
/// contributes each refined-alias field's conjuncts at that field's
/// slot — the same field domains `StructInit` checks at construction —
/// and an enum result contributes each payload slot's declared domain
/// conditioned on the tag selecting a carrier, matching the
/// tag-conditional enum-parameter convention `signature_domain` uses.
fn result_conjuncts_at<'ctx>(
    type_name: Option<&str>,
    value: SourceValue<'ctx>,
    refined: &RefinedEnv,
    structs: &StructEnv,
    enums: &EnumEnv,
    context: &'ctx Context,
) -> BodyResult<Vec<Bool<'ctx>>> {
    if let (Some(variants), SourceValue::Enum { tag, slots, .. }) =
        (type_name.and_then(|n| enums.get(n)), &value)
    {
        let mut out = Vec::new();
        for (i, slot_value) in slots.iter().enumerate() {
            let carrier_tags = variants
                .iter()
                .enumerate()
                .filter(|(_, v)| v.fields.get(i).is_some())
                .map(|(ordinal, _)| tag._eq(&BV::from_u64(context, ordinal as u64, 64)))
                .collect::<Vec<_>>();
            if carrier_tags.is_empty() {
                continue;
            }
            let carries = Bool::or(context, &carrier_tags.iter().collect::<Vec<_>>());
            let type_name = enum_slot_type(variants, i);
            let conjuncts = match slot_value {
                SourceValue::Integer(v) => declared_slot_domain(type_name, v, context, refined)?,
                SourceValue::Boolean(_) => {
                    refined_conjuncts_at(type_name, slot_value.clone(), refined, context)?
                }
                _ => continue,
            };
            out.extend(conjuncts.iter().map(|c| carries.implies(c)));
        }
        return Ok(out);
    }
    let (Some(fields), SourceValue::Composite(values)) =
        (type_name.and_then(|n| structs.get(n)), &value)
    else {
        return refined_conjuncts_at(type_name, value, refined, context);
    };
    let mut out = Vec::new();
    for (field, (_, field_value)) in fields.iter().zip(values) {
        // An enum-typed field carries its enum's payload-slot
        // conjuncts, so route it back through the enum arm above
        // rather than the scalar alias layer
        // (`pure-i64-composite/v17`).
        if matches!(field.shape, ValueKind::Enum(_)) {
            out.extend(result_conjuncts_at(
                Some(field.type_name.as_str()),
                field_value.clone(),
                refined,
                structs,
                enums,
                context,
            )?);
            continue;
        }
        out.extend(refined_conjuncts_at(
            Some(field.type_name.as_str()),
            field_value.clone(),
            refined,
            context,
        )?);
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
    atom_obligation(
        &atom,
        &body,
        &CallShapes::default(),
        context,
        &RefinedEnv::new(),
    )
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
    let (source_sha256, items, refined, structs, enums, bounded_memory) =
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
    let call_returns = call_return_kinds(&items, &refined, &structs, &enums, admission);
    let bodies: Vec<PureI64Body> = atom_items
        .iter()
        .map(|atom| {
            encode_atom_body(
                atom,
                context,
                source_sha256.clone(),
                &refined,
                &structs,
                &enums,
                &call_returns,
                bounded_memory,
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
            &enums,
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
    call_returns: &CallShapes<'ctx>,
    context: &'ctx Context,
    refined: &RefinedEnv,
    structs: &StructEnv,
    enums: &EnumEnv,
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
        param_env(&atom.params, &body.inputs, context, refined, structs, enums)?;
    let requires =
        parse_expression_checked(&atom.requires).map_err(|_| SemanticContractError::Parse)?;
    let top = Bool::from_bool(context, true);
    let mut sink = Vec::new();
    let mut requires_exits = Vec::new();
    // Contract literals belong to this atom's clause — the shared
    // `str_bound` cell still holds whatever the last encoded body left.
    call_returns
        .str_bound
        .set(atom.str_bound.unwrap_or(STR_BOUND));
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
        &EnumEnv::new(),
    )?
    .boolean()?;
    env.insert(
        "result".to_string(),
        result_value(&atom.name, &body.value, context, call_returns),
    );
    // `ensures` reads post-write composite fields — the same post-state
    // the verifier's env and the emitted code observe. Scalar rebinds
    // keep the established input-time view.
    for (name, value) in &body.post_writes {
        env.insert(name.clone(), value.clone());
    }
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
        &EnumEnv::new(),
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
    post.extend(result_conjuncts_at(
        atom.return_type.as_deref(),
        result_operand,
        refined,
        structs,
        enums,
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
        let site_env = param_env(&callee.params, &site.args, context, refined, structs, enums)?;
        let callee_requires =
            parse_expression_checked(&callee.requires).map_err(|_| SemanticContractError::Parse)?;
        // The clause belongs to the callee — its declared bound, not
        // the caller's.
        call_returns
            .str_bound
            .set(callee.str_bound.unwrap_or(STR_BOUND));
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
            enums,
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
            enums,
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
        // The callee's `ensures` reads post-write composite fields — the
        // same post-state its own obligation checks. Binding it to the
        // caller's input-time argument instead would let `requires
        // q.f == 0` and `ensures q.f == 1` on a writing callee collapse
        // the site's assumption into a contradiction, so instantiate
        // the callee's post-state composites at the call arguments.
        for (name, value) in &callee_body.post_writes {
            ensures_env.insert(name.clone(), substitute_source_value(value, &substitutions));
        }
        ensures_env.insert(
            "result".to_string(),
            result_value(&callee.name, &site.result, context, call_returns),
        );
        let callee_ensures =
            parse_expression_checked(&callee.ensures).map_err(|_| SemanticContractError::Parse)?;
        call_returns
            .str_bound
            .set(callee.str_bound.unwrap_or(STR_BOUND));
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
            enums,
        )?
        .boolean()?;
        // A refined callee result type strengthens the assumption the
        // site makes the same way it strengthens the callee's own
        // postcondition: the site result satisfies the declaration's
        // conjuncts at the call's actual result value.
        let mut callee_post = vec![callee_ensures];
        callee_post.extend(result_conjuncts_at(
            callee.return_type.as_deref(),
            ensures_env["result"].clone(),
            refined,
            structs,
            enums,
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
    call_returns: &CallShapes<'ctx>,
    context: &'ctx Context,
    refined: &RefinedEnv,
) -> BodyResult<Bool<'ctx>> {
    let mut env: HashMap<String, SourceValue> = param_env(
        &atom.params,
        &body.inputs,
        context,
        refined,
        &StructEnv::new(),
        &EnumEnv::new(),
    )?;
    let requires =
        parse_expression_checked(&atom.requires).map_err(|_| SemanticContractError::Parse)?;
    let top = Bool::from_bool(context, true);
    let mut sink = Vec::new();
    // Same shared-cell convention as `module_atom_obligations`: the
    // clause literals read this atom's declared bound.
    call_returns
        .str_bound
        .set(atom.str_bound.unwrap_or(STR_BOUND));
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
        &EnumEnv::new(),
    )?
    .boolean()?;
    env.insert(
        "result".to_string(),
        result_value(&atom.name, &body.value, context, call_returns),
    );
    // Same post-state composite view as `module_atom_obligations`.
    for (name, value) in &body.post_writes {
        env.insert(name.clone(), value.clone());
    }
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
        &EnumEnv::new(),
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
    /// An `i128` leaf (`pure-i64-composite/v21`): one `BV128` built
    /// `hi.concat(lo)` from the leaf's two 64-bit ABI slots — signed
    /// arithmetic and comparisons read the full width and every
    /// slot-boundary helper splits it back to two halves.
    Wide(BV<'ctx>),
    /// A `u128` leaf — the unsigned counterpart of `Wide`:
    /// `bvult`-family comparisons and `bvudiv`/`bvurem`/`bvlshr`.
    UWide(BV<'ctx>),
    /// A `u64` leaf — unsigned comparisons and `udiv`/`urem`/`lshr`
    /// (`pure-i64-composite/v10`). Mixing it with a declared `i64`
    /// operand in one binary operation fails closed; literals adapt.
    Unsigned(BV<'ctx>),
    /// A numeric literal before it meets an operand sort: it encodes
    /// like `Integer` against `i64`/`u8`-family operands and like
    /// `Unsigned` against `u64` operands — the literal-adaptive
    /// counterpart of `ScalarKind::Num`.
    Number(BV<'ctx>),
    /// An `f64` leaf — the slot carries the IEEE-754 bit pattern
    /// opaquely: the value may be stored, copied, returned and merged,
    /// but no arithmetic or comparison arm reads it
    /// (`pure-i64-composite/v11`).
    Float64(BV<'ctx>),
    Boolean(Bool<'ctx>),
    /// A struct parameter's flattened fields in declaration order. Only
    /// a field read or a pass-through call argument consumes it; every
    /// scalar position rejects it.
    Composite(Vec<(String, SourceValue<'ctx>)>),
    /// An enum parameter's flattened value: the declared enum's name,
    /// the tag slot carrying the variant ordinal, and one payload slot
    /// per index up to the widest variant's arity — the tagged-union
    /// layout the emitter emits. Only a `match` scrutinee or a
    /// pass-through call argument consumes it; every scalar position
    /// and every result position rejects it.
    Enum {
        name: String,
        tag: BV<'ctx>,
        slots: Vec<SourceValue<'ctx>>,
    },
    /// A `let`-bound fixed-size array (`bounded-memory`): cell contents
    /// as an SMT `BV64 -> BV64` array — the same index-domain the
    /// emitted `alloca`/GEP/`store`/`load` uses — plus the stored
    /// element count `len(a)` returns and the element kind reads
    /// produce.
    Array {
        store: Array<'ctx>,
        len: BV<'ctx>,
        elem: ScalarKind,
        /// Which array literal this binding's backing store descends
        /// from: `let b = a` shares `a`'s identity (the emitted code
        /// shares the data pointer), so an indexed write through one
        /// name reaches every alias. Mints from
        /// `CallShapes::next_array_id`.
        alloc: u64,
    },
}

/// Instantiate a callee-side [`SourceValue`] (expressed over the
/// callee's input consts) at a call site by substituting each formal
/// input bitvector with the site's actual argument.
/// Widen a `Num` literal to the 128-bit carrier when the other operand
/// is `i128`/`u128` (`pure-i64-composite/v21`) — its nonnegative `u64`
/// value zero-extends to the same bits, signed or unsigned.
fn widen_literal_operands<'ctx>(
    a: SourceValue<'ctx>,
    b: SourceValue<'ctx>,
) -> (SourceValue<'ctx>, SourceValue<'ctx>) {
    fn to_wide<'ctx>(v: &BV<'ctx>, like: &SourceValue<'ctx>) -> SourceValue<'ctx> {
        // Signed `i128` literals sign-extend (`-1` stays `-1`); unsigned
        // `u128` zero-extends — the same extension LLVM's `sext`/`zext`
        // apply at the boundary.
        let widened = if matches!(like, SourceValue::UWide(_)) {
            v.zero_ext(64)
        } else {
            v.sign_ext(64)
        };
        if matches!(like, SourceValue::UWide(_)) {
            SourceValue::UWide(widened)
        } else {
            SourceValue::Wide(widened)
        }
    }
    match (&a, &b) {
        (SourceValue::Wide(_) | SourceValue::UWide(_), SourceValue::Number(v)) => {
            let widened = to_wide(v, &a);
            (a, widened)
        }
        (SourceValue::Number(v), SourceValue::Wide(_) | SourceValue::UWide(_)) => {
            let widened = to_wide(v, &b);
            (widened, b)
        }
        _ => (a, b),
    }
}

fn substitute_source_value<'ctx>(
    value: &SourceValue<'ctx>,
    substitutions: &[(&BV<'ctx>, &BV<'ctx>)],
) -> SourceValue<'ctx> {
    match value {
        SourceValue::Integer(v) => SourceValue::Integer(v.substitute(substitutions)),
        SourceValue::Wide(v) => SourceValue::Wide(v.substitute(substitutions)),
        SourceValue::UWide(v) => SourceValue::UWide(v.substitute(substitutions)),
        SourceValue::Unsigned(v) => SourceValue::Unsigned(v.substitute(substitutions)),
        SourceValue::Float64(v) => SourceValue::Float64(v.substitute(substitutions)),
        SourceValue::Number(v) => SourceValue::Number(v.substitute(substitutions)),
        SourceValue::Boolean(v) => SourceValue::Boolean(v.substitute(substitutions)),
        SourceValue::Composite(fields) => SourceValue::Composite(
            fields
                .iter()
                .map(|(name, v)| (name.clone(), substitute_source_value(v, substitutions)))
                .collect(),
        ),
        SourceValue::Enum { name, tag, slots } => SourceValue::Enum {
            name: name.clone(),
            tag: tag.substitute(substitutions),
            slots: slots
                .iter()
                .map(|v| substitute_source_value(v, substitutions))
                .collect(),
        },
        SourceValue::Array {
            store,
            len,
            elem,
            alloc,
        } => SourceValue::Array {
            store: store.substitute(substitutions),
            len: len.substitute(substitutions),
            elem: *elem,
            alloc: *alloc,
        },
    }
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
    enums: &EnumEnv,
) -> BodyResult<HashMap<String, SourceValue<'ctx>>> {
    let arity: usize = signature_slots(params, structs, enums)
        .iter()
        .map(signature_slot_weight)
        .sum();
    if arity != inputs.len() {
        return Err(SemanticContractError::Unsupported("source signature arity"));
    }
    let scalar = |kind: ScalarKind, v: &BV<'ctx>| match kind {
        ScalarKind::Bool => SourceValue::Boolean(v._eq(&BV::from_u64(ctx, 0, 64)).not()),
        ScalarKind::U64 => SourceValue::Unsigned(v.clone()),
        ScalarKind::F64 => SourceValue::Float64(v.clone()),
        _ => SourceValue::Integer(v.clone()),
    };
    let mut slots = inputs.iter();
    let mut env = HashMap::new();
    for p in params {
        let value = match struct_fields(p, structs) {
            Some(fields) => seed_struct_composite(structs, enums, fields, &mut slots, ctx)?,
            None => match enum_variants(p, enums) {
                Some(variants) => {
                    let Some(tag) = slots.next() else {
                        return Err(SemanticContractError::Unsupported("source signature arity"));
                    };
                    SourceValue::Enum {
                        name: p.type_name.clone().unwrap_or_default(),
                        tag: tag.clone(),
                        slots: seed_enum_payload(variants, structs, enums, &mut slots, ctx)?,
                    }
                }
                None => {
                    let kind = param_kind(p, refined);
                    if matches!(kind, ScalarKind::I128 | ScalarKind::U128) {
                        let lo = slots.next();
                        let hi = slots.next();
                        let (Some(lo), Some(hi)) = (lo, hi) else {
                            return Err(SemanticContractError::Unsupported(
                                "source signature arity",
                            ));
                        };
                        let agg = hi.concat(lo);
                        if kind == ScalarKind::U128 {
                            SourceValue::UWide(agg)
                        } else {
                            SourceValue::Wide(agg)
                        }
                    } else {
                        let Some(v) = slots.next() else {
                            return Err(SemanticContractError::Unsupported(
                                "source signature arity",
                            ));
                        };
                        scalar(kind, v)
                    }
                }
            },
        };
        env.insert(p.name.clone(), value);
    }
    Ok(env)
}
impl<'ctx> SourceValue<'ctx> {
    fn integer(self) -> BodyResult<BV<'ctx>> {
        match self {
            Self::Integer(v) | Self::Unsigned(v) | Self::Number(v) => Ok(v),
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

/// The `q.f = v` half of an assignment, when `var` names a field of a
/// composite binding on the linear path (`pure-i64-composite/v5` —
/// `field_write_kind` at the kind layer already gated admission, so
/// reaching here with a composite target means the write is in the
/// fragment). The base needs no `assignable` entry: `assignable` names
/// scalar rebind targets, while a composite base's mutability is
/// inherent — `env.get(base)` holding a `Composite` already proves the
/// binding is live on the linear path. Under
/// `pure-i64-composite/v15` the path descends nested composites —
/// `q.inner.a = v` rewrites the leaf inside `q`'s `inner` subtree.
/// Returns the base name, the composite with the slot replaced and the
/// encoded field value, so the caller rebinds the base and yields the
/// leaf; `None` falls through to the ordinary assignable check.
#[allow(clippy::too_many_arguments)]
fn source_field_write<'ctx>(
    var: &str,
    value: &Expr,
    ctx: &'ctx Context,
    env: &HashMap<String, SourceValue<'ctx>>,
    guard: &Bool<'ctx>,
    definedness: &mut Vec<Bool<'ctx>>,
    depth: usize,
    written: &mut HashMap<String, SourceValue<'ctx>>,
    loop_exits: &mut Vec<Bool<'ctx>>,
    call_sites: &mut Vec<CallSite<'ctx>>,
    call_returns: &CallShapes<'ctx>,
    enums: &EnumEnv,
) -> BodyResult<Option<(String, SourceValue<'ctx>, SourceValue<'ctx>)>> {
    // The composite with the value bound into `path`: descends the
    // declared field names and rebuilds each level around the one
    // changed slot — the same `insertvalue` chain the emitter emits.
    fn update_at_path<'ctx>(
        fields: &[(String, SourceValue<'ctx>)],
        path: &[String],
        bound: &SourceValue<'ctx>,
    ) -> Option<SourceValue<'ctx>> {
        let (head, rest) = path.split_first()?;
        Some(SourceValue::Composite(
            fields
                .iter()
                .map(|(name, slot)| {
                    if name != head {
                        return Some((name.clone(), slot.clone()));
                    }
                    if rest.is_empty() {
                        return Some((name.clone(), bound.clone()));
                    }
                    let SourceValue::Composite(inner) = slot else {
                        return None;
                    };
                    Some((name.clone(), update_at_path(inner, rest, bound)?))
                })
                .collect::<Option<Vec<_>>>()?,
        ))
    }

    let mut segments = var.split('.');
    let base = segments.next().unwrap_or(var);
    let path: Vec<String> = segments.map(str::to_owned).collect();
    if path.is_empty() {
        return Ok(None);
    }
    if let Some(SourceValue::Enum { name, tag, slots }) = env.get(base) {
        // `e.p = v`: a single segment naming one variant's payload
        // field. The write is only defined when the tag provably
        // carries that variant — the same `is-V` gate the general
        // verifier's datatype model applies (`pure-i64-composite/v16`).
        if path.len() != 1 {
            return Ok(None);
        }
        let Some(variants) = enums.get(name.as_str()) else {
            return Ok(None);
        };
        let Some((ordinal, index, _)) = enum_field_slot(variants, &path[0]) else {
            return Ok(None);
        };
        if value.binds_name(base) {
            return Err(SemanticContractError::Unsupported(
                "field write value rebinds the base",
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
            enums,
        )?;
        // The slot keeps its declared leaf shape — same coercion a
        // composite field write applies.
        let bound = match (slots.get(index), &bound) {
            (Some(SourceValue::Integer(_)), SourceValue::Number(v)) => {
                SourceValue::Integer(v.clone())
            }
            (Some(SourceValue::Unsigned(_)), SourceValue::Number(v)) => {
                SourceValue::Unsigned(v.clone())
            }
            (_, bound) => bound.clone(),
        };
        definedness.push(guard.implies(&tag._eq(&BV::from_u64(ctx, ordinal as u64, 64))));
        let mut updated = slots.clone();
        if index >= updated.len() {
            return Ok(None);
        }
        updated[index] = bound.clone();
        return Ok(Some((
            base.to_string(),
            SourceValue::Enum {
                name: name.clone(),
                tag: tag.clone(),
                slots: updated,
            },
            bound,
        )));
    }
    let Some(SourceValue::Composite(fields)) = env.get(base) else {
        return Ok(None);
    };
    // Descend to the leaf slot the path names — a mid-path segment
    // must land on a nested composite, and every segment must be a
    // declared field of the composite it descends into.
    let mut level: &[(String, SourceValue<'ctx>)] = fields;
    let mut slot = None;
    for (i, segment) in path.iter().enumerate() {
        let Some((_, found)) = level.iter().find(|(name, _)| name == segment) else {
            return Ok(None);
        };
        if i + 1 == path.len() {
            slot = Some(found);
            break;
        }
        match found {
            SourceValue::Composite(inner) => level = inner,
            _ => return Ok(None),
        }
    }
    let Some(slot) = slot else {
        return Ok(None);
    };
    // A value that rebinds `base` (`q.x = { q.y = 2; 1 }`, a shadowing
    // `let q`, a whole-struct `q = …`) would leave the merged composite
    // and the emitted `insertvalue` disagreeing about which `q` they
    // update — reject rather than pick a winner.
    if value.binds_name(base) {
        return Err(SemanticContractError::Unsupported(
            "field write value rebinds the base",
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
        enums,
    )?;
    // The encoded slot shape cannot change: an integer field takes
    // only an integer value, a boolean field only a boolean, and a
    // struct field (`pure-i64-composite/v8`) only a composite — the
    // kind layer already checked the composite's declared type. A
    // `Number` bound lands under the slot's leaf, the same coercion a
    // struct literal applies, so the slot cannot drift signed where
    // the field is unsigned (`pure-i64-composite/v10`).
    let bound = match (slot, bound) {
        (SourceValue::Integer(_), SourceValue::Number(v)) => SourceValue::Integer(v),
        (SourceValue::Unsigned(_), SourceValue::Number(v)) => SourceValue::Unsigned(v),
        (_, bound) => bound,
    };
    // A whole-enum write `b.e = e2` (`pure-i64-composite/v17`): the
    // kind layer already required the same declared enum.
    let same_enum = match (slot, &bound) {
        (SourceValue::Enum { name: declared, .. }, SourceValue::Enum { name: written, .. }) => {
            declared == written
        }
        _ => false,
    };
    match (slot, &bound) {
        (SourceValue::Integer(_) | SourceValue::Number(_), SourceValue::Integer(_))
        | (SourceValue::Integer(_), SourceValue::Number(_))
        | (SourceValue::Unsigned(_) | SourceValue::Number(_), SourceValue::Unsigned(_))
        | (SourceValue::Unsigned(_), SourceValue::Number(_))
        | (SourceValue::Number(_), SourceValue::Number(_))
        | (SourceValue::Boolean(_), SourceValue::Boolean(_))
        | (SourceValue::Float64(_), SourceValue::Float64(_))
        | (SourceValue::Composite(_), SourceValue::Composite(_)) => {}
        _ if same_enum => {}
        _ => {
            return Err(SemanticContractError::Unsupported(
                "struct field write kind",
            ));
        }
    }
    let Some(updated) = update_at_path(fields, &path, &bound) else {
        return Ok(None);
    };
    Ok(Some((base.to_owned(), updated, bound)))
}

/// Whether a name rebound inside a block escapes into `written`: an
/// ordinary assignable does (`escapable` already excludes shadowed
/// names), and so does a composite bound outside the block — composite
/// parameters never enter `assignable`, so without this second clause
/// a field write inside an `if`/`match` arm or a `while` body would be
/// dropped silently instead of reaching the branch/loop merge, which
/// fails closed on composites. A `let` bound in the block stays out:
/// it is in `inner_assignable` and absent from the outer `env`.
fn write_escapes<'ctx>(
    env: &HashMap<String, SourceValue<'ctx>>,
    inner_assignable: &BTreeSet<String>,
    escapable: &BTreeSet<String>,
    name: &str,
) -> bool {
    escapable.contains(name)
        || (matches!(
            env.get(name),
            Some(SourceValue::Composite(_) | SourceValue::Enum { .. })
        ) && !inner_assignable.contains(name))
}

/// `assignable` mirrors `statement_kind`: only names bound on the
/// current linear path may be rebound. `written` collects the final
/// symbolic value of every assignable name a statement rewrites, so an
/// enclosing `if` can merge branch writes under its condition and an
/// enclosing block can re-export the writes that escape it.
#[allow(clippy::too_many_arguments)]
/// The value `cond` selects between: a scalar `ite`, or per-slot `ite`s
/// over the flattened fields of a `Composite`/`Enum` — the merge the
/// emitter's aggregate `phi` performs on branch-joined composites
/// (`pure-i64-composite/v7`). Both sides must flatten to the same
/// shape: equal struct field lists, equal enum name and slot count.
fn merge_source_values<'ctx>(
    cond: &Bool<'ctx>,
    a: SourceValue<'ctx>,
    b: SourceValue<'ctx>,
    call_returns: &CallShapes<'ctx>,
) -> BodyResult<SourceValue<'ctx>> {
    use SourceValue::{
        Boolean as B, Float64 as F, Integer as I, Number as N, UWide as UW, Unsigned as U,
        Wide as W,
    };
    let merged_composites = call_returns.merged_composites;
    match (a, b) {
        (N(a), N(b)) => Ok(N(cond.ite(&a, &b))),
        (U(a), U(b) | N(b)) | (N(a), U(b)) => Ok(U(cond.ite(&a, &b))),
        (I(a) | N(a), I(b) | N(b)) => Ok(I(cond.ite(&a, &b))),
        (B(a), B(b)) => Ok(B(cond.ite(&a, &b))),
        // Pass-through `f64`: the bit pattern merges like any leaf.
        (F(a), F(b)) => Ok(F(cond.ite(&a, &b))),
        // 128-bit leaves merge as one vector; a literal branch value
        // zero-extends into the carrier (`pure-i64-composite/v21`).
        (W(a), W(b)) => Ok(W(cond.ite(&a, &b))),
        (W(a), N(b)) => Ok(W(cond.ite(&a, &b.sign_ext(64)))),
        (N(a), W(b)) => Ok(W(cond.ite(&a.sign_ext(64), &b))),
        (UW(a), UW(b)) => Ok(UW(cond.ite(&a, &b))),
        (UW(a), N(b)) => Ok(UW(cond.ite(&a, &b.zero_ext(64)))),
        (N(a), UW(b)) => Ok(UW(cond.ite(&a.zero_ext(64), &b))),
        (SourceValue::Composite(a), SourceValue::Composite(b)) if merged_composites => {
            if a.len() != b.len() || a.iter().zip(b.iter()).any(|((an, _), (bn, _))| an != bn) {
                return Err(SemanticContractError::Unsupported("branch composite kinds"));
            }
            let mut fields = Vec::with_capacity(a.len());
            for ((name, av), (_, bv)) in a.into_iter().zip(b) {
                fields.push((name, merge_source_values(cond, av, bv, call_returns)?));
            }
            Ok(SourceValue::Composite(fields))
        }
        (
            SourceValue::Enum {
                name: an,
                tag: at,
                slots: a_slots,
            },
            SourceValue::Enum {
                name: bn,
                tag: bt,
                slots: b_slots,
            },
        ) if merged_composites && an == bn && a_slots.len() == b_slots.len() => {
            let mut slots = Vec::with_capacity(a_slots.len());
            for (av, bv) in a_slots.into_iter().zip(b_slots) {
                slots.push(merge_source_values(cond, av, bv, call_returns)?);
            }
            Ok(SourceValue::Enum {
                name: an,
                tag: cond.ite(&at, &bt),
                slots,
            })
        }
        // A branch writing `a[i] = v` rebinds the array under the
        // branch condition — the merge is the SMT array `ite` of the
        // store-chain and the incoming array, not a pointer `phi`
        // (the emitted IR keeps one data pointer and merges the
        // stored values). Two sides bound from different literals
        // mint a fresh `alloc`: keeping either side's id would let a
        // write through the merged name reach bindings that alias
        // only one side.
        (
            SourceValue::Array {
                store: as_,
                len: al,
                elem: ae,
                alloc: aa,
            },
            SourceValue::Array {
                store: bs,
                len: bl,
                elem: be,
                alloc: ba,
            },
        ) if ae == be => Ok(SourceValue::Array {
            store: cond.ite(&as_, &bs),
            len: cond.ite(&al, &bl),
            elem: ae,
            alloc: if aa == ba {
                aa
            } else {
                call_returns.fresh_array_alloc()
            },
        }),
        _ => Err(SemanticContractError::Unsupported("branch composite kinds")),
    }
}

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
    call_returns: &CallShapes<'ctx>,
    enums: &EnumEnv,
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
            enums,
        ),
        Stmt::Assign { var, value, .. } => {
            if let Some((base, updated, bound)) = source_field_write(
                var,
                value,
                ctx,
                env,
                guard,
                definedness,
                depth,
                written,
                loop_exits,
                call_sites,
                call_returns,
                enums,
            )? {
                // A field write rebinds the whole composite under the
                // base name: the escape record sees the updated fields
                // exactly like a scalar rebind of `base`.
                written.insert(base, updated);
                return Ok(bound);
            }
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
                enums,
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
            enums,
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
                            enums,
                        )?;
                        // Same shadow rule as the kind layer and the
                        // verifier: a composite's slots are name-keyed,
                        // so only a same-shape composite may re-bind the
                        // name — a scalar shadow would merge stale slots.
                        if matches!(inner.get(var), Some(SourceValue::Composite(_)))
                            && !matches!(bound, SourceValue::Composite(_))
                        {
                            return Err(SemanticContractError::Unsupported(
                                "let shadows a struct binding with a different type",
                            ));
                        }
                        // An array binding is name-keyed the same way:
                        // only another array may shadow it.
                        if matches!(inner.get(var), Some(SourceValue::Array { .. }))
                            && !matches!(bound, SourceValue::Array { .. })
                        {
                            return Err(SemanticContractError::Unsupported(
                                "let shadows an array binding with a different type",
                            ));
                        }
                        inner.insert(var.clone(), bound);
                        inner_assignable.insert(var.clone());
                        escapable.remove(var);
                    }
                    Stmt::Assign { var, value, .. } => {
                        if let Some((base, updated, _bound)) = source_field_write(
                            var,
                            value,
                            ctx,
                            &inner,
                            guard,
                            definedness,
                            depth,
                            written,
                            loop_exits,
                            call_sites,
                            call_returns,
                            enums,
                        )? {
                            // Same rebind as the tail-position arm,
                            // escaping per `write_escapes`.
                            inner.insert(base.clone(), updated.clone());
                            if write_escapes(env, &inner_assignable, &escapable, &base) {
                                written.insert(base, updated);
                            }
                            continue;
                        }
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
                            enums,
                        )?;
                        inner.insert(var.clone(), bound.clone());
                        if escapable.contains(var) {
                            written.insert(var.clone(), bound);
                        }
                    }
                    // An indexed write `a[i] = v` rebinds the array
                    // under its in-bounds guard — the emitted code
                    // skips the store entirely when the index is out
                    // of bounds, so OOB keeps the incoming array.
                    Stmt::ArrayStore {
                        array,
                        index,
                        value,
                        ..
                    } => {
                        let Some(SourceValue::Array {
                            store, len, alloc, ..
                        }) = inner.get(array)
                        else {
                            return Err(SemanticContractError::Unsupported(
                                "array write on a non-array binding",
                            ));
                        };
                        if !inner_assignable.contains(array.as_str()) {
                            return Err(SemanticContractError::Unsupported(
                                "array write to a name not bound on the linear path",
                            ));
                        }
                        // The assigned value evaluates before the index,
                        // matching the emitter's `val` then `idx` order:
                        // a write through an alias inside the index
                        // expression must not reach the value's reads.
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
                            enums,
                        )?
                        .integer()?;
                        let index = source_expr(
                            index,
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
                            enums,
                        )?
                        .integer()?;
                        // `let b = a` aliases the same backing store
                        // (`alloc`), the one data pointer the emitter
                        // emits for the group: the write updates the
                        // canonical per-`alloc` store so every binding
                        // on the backing sees it — including a name a
                        // block `let` shadowed, which the flat `inner`
                        // map can no longer reach.
                        let cur_store = canonical_store(call_returns, *alloc, store);
                        let new_store = array_in_bounds(ctx, &index, len)
                            .ite(&cur_store.store(&index, &bound), &cur_store);
                        // The store is a state mutation, so it binds
                        // only where this statement actually runs — a
                        // `false && { … a[i] = v … }` path keeps the
                        // pre-statement backing, like the emitted
                        // short-circuit branch that skips the block.
                        let new_store = guard.ite(&new_store, &cur_store);
                        call_returns
                            .alloc_stores
                            .borrow_mut()
                            .insert(*alloc, new_store.clone());
                        // The written name's own snapshot tracks the
                        // canonical store so name-level merges pair
                        // up-to-date branch sides, and its rebind
                        // escapes like any write.
                        if let Some(SourceValue::Array {
                            store: member_store,
                            ..
                        }) = inner.get_mut(array)
                        {
                            *member_store = new_store;
                        }
                        if escapable.contains(array.as_str()) {
                            if let Some(value) = inner.get(array) {
                                written.insert(array.clone(), value.clone());
                            }
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
                            enums,
                        )?;
                        for (name, bound) in local {
                            inner.insert(name.clone(), bound.clone());
                            if write_escapes(env, &inner_assignable, &escapable, &name) {
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
                            enums,
                        )?;
                        for (name, bound) in local {
                            inner.insert(name.clone(), bound.clone());
                            if write_escapes(env, &inner_assignable, &escapable, &name) {
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
                enums,
            )?;
            for (name, bound) in tail_writes {
                if write_escapes(env, &inner_assignable, &escapable, &name) {
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
    call_returns: &CallShapes<'ctx>,
    enums: &EnumEnv,
) -> BodyResult<()> {
    use SourceValue::{
        Boolean as B, Float64 as F, Integer as I, Number as N, UWide as UW, Unsigned as U,
        Wide as W,
    };
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
            enums,
        )? {
            B(b) => b,
            _ => {
                return Err(SemanticContractError::Unsupported(
                    "non-boolean while condition",
                ))
            }
        };
        let body_guard = Bool::and(ctx, &[&alive, &c]);
        let pre_body_stores = call_returns.alloc_stores.borrow().clone();
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
            enums,
        )?;
        let post_body_stores = call_returns.alloc_stores.borrow().clone();
        // The body's writes to a shared backing apply only where the
        // loop actually ran — the canonical map's counterpart of the
        // per-name `body_guard.ite` merge below.
        {
            let mut stores = call_returns.alloc_stores.borrow_mut();
            for (alloc, store) in stores.iter_mut() {
                if let Some(pre) = pre_body_stores.get(alloc) {
                    *store = body_guard.ite(store, pre);
                }
            }
        }
        for (name, bound) in body_writes {
            // Writes escape only where the body actually ran: an
            // already-exited path keeps its entry values however the
            // stale condition evaluates. A literal-bound `N` pins to the
            // kind its write produces (`let s = 0; s = s + n` → `I`), and
            // an unsigned slot keeps its `U` (`merge_source_values` uses
            // the same lattice for branch merges).
            let value = match (cur.get(&name), &bound) {
                (Some(N(old)), N(new)) => N(body_guard.ite(new, old)),
                (Some(U(old)), U(new) | N(new)) | (Some(N(old)), U(new)) => {
                    U(body_guard.ite(new, old))
                }
                (Some(I(old) | N(old)), I(new) | N(new)) => I(body_guard.ite(new, old)),
                (Some(B(old)), B(new)) => B(body_guard.ite(new, old)),
                (Some(F(old)), F(new)) => F(body_guard.ite(new, old)),
                // 128-bit leaves phi-merge as one vector; a literal
                // branch value extends its 64 bits into the carrier —
                // sign-extended for `i128`, zero-extended for `u128`
                // (`pure-i64-composite/v21`).
                (Some(W(old)), W(new)) => W(body_guard.ite(new, old)),
                (Some(W(old)), N(new)) => W(body_guard.ite(&new.sign_ext(64), old)),
                (Some(N(old)), W(new)) => W(body_guard.ite(new, &old.sign_ext(64))),
                (Some(UW(old)), UW(new)) => UW(body_guard.ite(new, old)),
                (Some(UW(old)), N(new)) => UW(body_guard.ite(&new.zero_ext(64), old)),
                (Some(N(old)), UW(new)) => UW(body_guard.ite(new, &old.zero_ext(64))),

                // A loop-carried `a[i] = v` keeps the merged array: the
                // body's write applies only where the loop actually ran.
                // A body rebinding `a = [...]` mints a fresh `alloc`,
                // same as a branch merge of different literals.
                (
                    Some(SourceValue::Array {
                        store: os,
                        len: ol,
                        elem: oe,
                        alloc: oa,
                    }),
                    SourceValue::Array {
                        store: ns,
                        len: nl,
                        elem: ne,
                        alloc: na,
                    },
                ) if oe == ne => {
                    let alloc = if oa == na {
                        *oa
                    } else {
                        // A body rebinding mints a fresh backing: its
                        // canonical store is the guard-merge of each
                        // side's canonical state.
                        let fresh = call_returns.fresh_array_alloc();
                        let body_store = post_body_stores
                            .get(na)
                            .cloned()
                            .unwrap_or_else(|| ns.clone());
                        let cur_store = pre_body_stores
                            .get(oa)
                            .cloned()
                            .unwrap_or_else(|| os.clone());
                        call_returns
                            .alloc_stores
                            .borrow_mut()
                            .insert(fresh, body_guard.ite(&body_store, &cur_store));
                        fresh
                    };
                    SourceValue::Array {
                        store: body_guard.ite(ns, os),
                        len: body_guard.ite(nl, ol),
                        elem: *oe,
                        alloc,
                    }
                }
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

/// The unsigned counterpart of `div_defined` (`pure-i64-composite/v10`):
/// `udiv`/`urem` only require a nonzero divisor — `u64::MAX / -1` is
/// an ordinary result, not an overflow.
fn udiv_defined<'ctx>(b: &BV<'ctx>, ctx: &'ctx Context) -> Bool<'ctx> {
    b._eq(&BV::from_i64(ctx, 0, 64)).not()
}

/// The domain `shl`/`ashr` share: a shift amount in 0..64 (an unsigned
/// comparison, so negative amounts are excluded too).
fn shift_defined<'ctx>(amount: &BV<'ctx>, ctx: &'ctx Context) -> Bool<'ctx> {
    amount.bvult(&BV::from_i64(ctx, 64, 64))
}

fn int128_min<'ctx>(ctx: &'ctx Context) -> BV<'ctx> {
    // `i128::MIN` = 1 << 127 — `BV::from_i64` cannot express it, so it
    // is built as a literal shift.
    BV::from_u64(ctx, 1, 128).bvshl(&BV::from_u64(ctx, 127, 128))
}

/// The `i128` counterpart of `div_defined`: nonzero divisor, and
/// `i128::MIN / -1` excluded — the only signed-division overflow.
fn div_defined_wide<'ctx>(a: &BV<'ctx>, b: &BV<'ctx>, ctx: &'ctx Context) -> Bool<'ctx> {
    let zero = BV::from_i64(ctx, 0, 128);
    let minus_one = BV::from_i64(ctx, -1, 128);
    Bool::and(
        ctx,
        &[
            &b._eq(&zero).not(),
            &Bool::and(ctx, &[&a._eq(&int128_min(ctx)), &b._eq(&minus_one)]).not(),
        ],
    )
}

fn udiv_defined_wide<'ctx>(b: &BV<'ctx>, ctx: &'ctx Context) -> Bool<'ctx> {
    b._eq(&BV::from_i64(ctx, 0, 128)).not()
}

/// The `i128`/`u128` counterpart of `shift_defined`: amount in 0..128.
fn shift_defined_wide<'ctx>(amount: &BV<'ctx>, ctx: &'ctx Context) -> Bool<'ctx> {
    amount.bvult(&BV::from_u64(ctx, 128, 128))
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

/// The uninterpreted function a struct-returning call's field slot
/// `index` applies — one per declared field, so a call site's struct
/// result is the same per-field applications on the source and LLVM
/// sides.
pub fn call_field_decl<'ctx>(
    ctx: &'ctx Context,
    name: &str,
    arity: usize,
    index: usize,
) -> FuncDecl<'ctx> {
    let domain: Vec<Sort> = (0..arity).map(|_| Sort::bitvector(ctx, 64)).collect();
    let refs: Vec<&Sort> = domain.iter().collect();
    FuncDecl::new(
        ctx,
        // `:` never appears in an identifier, so no `call_decl` name
        // (`mumei_call_<atom>_<arity>`) can spell a field slot's name.
        format!("mumei_call_field:{name}:{arity}:{index}"),
        &refs,
        &Sort::bitvector(ctx, 64),
    )
}

/// A struct value's field slots as one bitvector: slot 0 in the most
/// significant 64 bits, each later slot below it. Both the source
/// model and the LLVM encoder compare struct results in this layout.
pub fn aggregate_value<'ctx>(slots: &[BV<'ctx>]) -> BV<'ctx> {
    let (first, rest) = slots
        .split_first()
        .unwrap_or_else(|| unreachable!("admitted structs have at least one field"));
    rest.iter()
        .fold(first.clone(), |acc, slot| acc.concat(slot))
}

/// Split an `aggregate_value` back into its `n` field slots.
fn split_slots<'ctx>(value: &BV<'ctx>, n: usize) -> Vec<BV<'ctx>> {
    (0..n)
        .map(|i| {
            let hi = (64 * (n - i) - 1) as u32;
            value.extract(hi, hi - 63)
        })
        .collect()
}

/// A struct value from its leaf slots in flattening order, each read
/// by the field's kind under the profile's i64 0/1 `bool` convention —
/// a struct-typed field consumes its own leaves into a nested
/// `Composite` (`pure-i64-composite/v8`).
fn composite_from_slots<'ctx>(
    call_returns: &CallShapes<'ctx>,
    fields: &[(String, ValueKind)],
    slots: &mut std::slice::Iter<'_, BV<'ctx>>,
    ctx: &'ctx Context,
) -> SourceValue<'ctx> {
    SourceValue::Composite(
        fields
            .iter()
            .map(|(name, shape)| {
                let value = match shape {
                    ValueKind::Scalar(kind) => {
                        let slot = slots.next().cloned().unwrap_or_else(|| {
                            unreachable!("admitted structs always match their lowered arity")
                        });
                        match kind {
                            ScalarKind::Bool => {
                                SourceValue::Boolean(slot._eq(&BV::from_u64(ctx, 0, 64)).not())
                            }
                            ScalarKind::U64 => SourceValue::Unsigned(slot),
                            ScalarKind::F64 => SourceValue::Float64(slot),
                            ScalarKind::I128 | ScalarKind::U128 => {
                                let hi = slots.next().cloned().unwrap_or_else(|| {
                                    unreachable!(
                                        "admitted structs always match their lowered arity"
                                    )
                                });
                                let agg = hi.concat(&slot);
                                if *kind == ScalarKind::U128 {
                                    SourceValue::UWide(agg)
                                } else {
                                    SourceValue::Wide(agg)
                                }
                            }
                            _ => SourceValue::Integer(slot),
                        }
                    }
                    ValueKind::Struct(child) => {
                        let inner = call_returns.structs.get(child).cloned().unwrap_or_default();
                        composite_from_slots(call_returns, &inner, slots, ctx)
                    }
                    // An enum-typed field consumes a tag slot plus the
                    // shared payload leaf span — the same seed an enum
                    // parameter gets (`pure-i64-composite/v17`, `/v18`).
                    ValueKind::Enum(enum_name) => {
                        let variants = call_returns
                            .enums
                            .get(enum_name)
                            .cloned()
                            .unwrap_or_default();
                        let tag = slots.next().cloned().unwrap_or_else(|| {
                            unreachable!("admitted structs always match their lowered arity")
                        });
                        SourceValue::Enum {
                            name: enum_name.clone(),
                            tag,
                            slots: call_returns.seed_enum_slots(&variants, slots, ctx),
                        }
                    }
                };
                (name.clone(), value)
            })
            .collect(),
    )
}

/// A scalar value in the profile's lowered i64 representation.
fn lower_scalar<'ctx>(value: SourceValue<'ctx>, ctx: &'ctx Context) -> BodyResult<BV<'ctx>> {
    match value {
        SourceValue::Integer(v)
        | SourceValue::Unsigned(v)
        | SourceValue::Number(v)
        | SourceValue::Wide(v)
        | SourceValue::UWide(v)
        | SourceValue::Float64(v) => Ok(v),
        SourceValue::Boolean(b) => Ok(b.ite(&BV::from_u64(ctx, 1, 64), &BV::from_u64(ctx, 0, 64))),
        SourceValue::Composite(_) => Err(SemanticContractError::Unsupported(
            "source nested struct value",
        )),
        SourceValue::Enum { .. } => Err(SemanticContractError::Unsupported(
            "source enum value in a scalar position",
        )),
        SourceValue::Array { .. } => Err(SemanticContractError::Unsupported(
            "source array value in a scalar position",
        )),
    }
}

/// The in-bounds domain of an indexed array access: signed
/// `0 <= i < len`, matching the `icmp sge`/`icmp slt` pair the
/// emitter's bounds check emits.
fn array_in_bounds<'ctx>(ctx: &'ctx Context, index: &BV<'ctx>, len: &BV<'ctx>) -> Bool<'ctx> {
    Bool::and(
        ctx,
        &[&index.bvsge(&BV::from_u64(ctx, 0, 64)), &index.bvslt(len)],
    )
}

/// The bounded-memory read the emitted bounds-check + `load` + merged
/// `phi(loaded, 0)` encodes: `ite(0 <= i < len, select(arr, i), 0)` —
/// an out-of-bounds read yields `0` exactly like the emitter's merged
/// value, so no definedness obligation is recorded.
fn array_read<'ctx>(
    ctx: &'ctx Context,
    store: &Array<'ctx>,
    len: &BV<'ctx>,
    index: &BV<'ctx>,
) -> BV<'ctx> {
    let selected = store.select(index).as_bv().unwrap_or_else(|| {
        // The range sort is always BV64 — the fallback only keeps the
        // helper total.
        BV::from_u64(ctx, 0, 64)
    });
    array_in_bounds(ctx, index, len).ite(&selected, &BV::from_u64(ctx, 0, 64))
}

/// The canonical backing store for `alloc` — the model's shared data
/// pointer — or the binding's own `store` snapshot when the alloc has
/// no tracked entry (a `CallShapes::default()` context evaluating a
/// pre-seeded environment).
fn canonical_store<'ctx>(
    call_returns: &CallShapes<'ctx>,
    alloc: u64,
    snapshot: &Array<'ctx>,
) -> Array<'ctx> {
    call_returns
        .alloc_stores
        .borrow()
        .get(&alloc)
        .cloned()
        .unwrap_or_else(|| snapshot.clone())
}

/// Merge the canonical per-`alloc` stores after a branch: every
/// backing present before the branch becomes the `ite` of the two
/// sides' states, regardless of which source names still point at it —
/// a shadowed binding's array joins no `written` name but still shares
/// the merged backing. Backings minted inside a branch keep that
/// side's store (no binding on the other side can reference them).
fn merge_alloc_stores<'ctx>(
    call_returns: &CallShapes<'ctx>,
    cond: &Bool<'ctx>,
    then_stores: &HashMap<u64, Array<'ctx>>,
    else_stores: &HashMap<u64, Array<'ctx>>,
) {
    let mut merged: HashMap<u64, Array<'ctx>> = else_stores.clone();
    for (alloc, then_store) in then_stores {
        merged.insert(
            *alloc,
            match else_stores.get(alloc) {
                Some(else_store) => cond.ite(then_store, else_store),
                None => then_store.clone(),
            },
        );
    }
    *call_returns.alloc_stores.borrow_mut() = merged;
}

/// Seed the canonical store for a merged `Array` binding that the
/// name-level merge gave a fresh `alloc`: each side's store is that
/// side's canonical state for its own `alloc` (the binding's `store`
/// snapshot may predate later indexed writes), merged under `cond`.
/// A same-`alloc` merge is already covered by the surrounding
/// per-`alloc` map merge, so re-seeding would only rebuild its term.
fn seed_merged_alloc<'ctx>(
    call_returns: &CallShapes<'ctx>,
    cond: &Bool<'ctx>,
    merged: &SourceValue<'ctx>,
    then_side: &SourceValue<'ctx>,
    else_side: &SourceValue<'ctx>,
    then_stores: &HashMap<u64, Array<'ctx>>,
    else_stores: &HashMap<u64, Array<'ctx>>,
) {
    let (
        SourceValue::Array {
            alloc: then_alloc,
            store: then_snapshot,
            ..
        },
        SourceValue::Array {
            alloc: else_alloc,
            store: else_snapshot,
            ..
        },
        SourceValue::Array {
            alloc: merged_alloc,
            ..
        },
    ) = (then_side, else_side, merged)
    else {
        return;
    };
    if merged_alloc == then_alloc || merged_alloc == else_alloc {
        return;
    }
    let then_store = then_stores
        .get(then_alloc)
        .cloned()
        .unwrap_or_else(|| then_snapshot.clone());
    let else_store = else_stores
        .get(else_alloc)
        .cloned()
        .unwrap_or_else(|| else_snapshot.clone());
    call_returns
        .alloc_stores
        .borrow_mut()
        .insert(*merged_alloc, cond.ite(&then_store, &else_store));
}

/// The `result` binding of an atom's (or call site's) result value:
/// a struct result splits into its field slots, a `bool` result reads
/// the 0/1 convention, an i64 result binds directly.
fn result_value<'ctx>(
    atom: &str,
    value: &BV<'ctx>,
    ctx: &'ctx Context,
    call_returns: &CallShapes<'ctx>,
) -> SourceValue<'ctx> {
    if let Some(fields) = call_returns.result_fields(atom) {
        let leaf_count = call_returns.flattened_kinds(fields).len();
        let slots = split_slots(value, leaf_count);
        composite_from_slots(call_returns, fields, &mut slots.iter(), ctx)
    } else if let Some((enum_name, variants)) = call_returns.result_enum(atom) {
        // An enum result splits into its tag slot then the shared
        // payload leaf span — the layout `match` decomposes
        // (`pure-i64-composite/v18`).
        let flat = split_slots(value, 1 + call_returns.enum_layout_leaf_count(variants));
        SourceValue::Enum {
            name: enum_name.to_owned(),
            tag: flat[0].clone(),
            slots: call_returns.seed_enum_slots(variants, &mut flat[1..].iter(), ctx),
        }
    } else if call_returns.returns_wide(atom) {
        if call_returns.returns_u128(atom) {
            SourceValue::UWide(value.clone())
        } else {
            SourceValue::Wide(value.clone())
        }
    } else if call_returns.returns_u64(atom) {
        SourceValue::Unsigned(value.clone())
    } else if call_returns.returns_f64(atom) {
        SourceValue::Float64(value.clone())
    } else if call_returns.returns_bool(atom) {
        SourceValue::Boolean(value._eq(&BV::from_u64(ctx, 0, 64)).not())
    } else {
        SourceValue::Integer(value.clone())
    }
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
    call_returns: &CallShapes<'ctx>,
    enums: &EnumEnv,
) -> BodyResult<SourceValue<'ctx>> {
    use SourceValue::{
        Boolean as B, Float64 as F, Integer as I, Number as N, UWide as UW, Unsigned as U,
        Wide as W,
    };
    if depth > MAX_DEPTH {
        return Err(SemanticContractError::Unsupported("source depth limit"));
    }
    Ok(match expr {
        Expr::Number(n) => N(BV::from_i64(ctx, *n, 64)),
        Expr::Variable(n) if n == "true" || n == "false" => B(Bool::from_bool(ctx, n == "true")),
        Expr::Variable(n) => env
            .get(n.as_str())
            .cloned()
            .ok_or(SemanticContractError::Unsupported("source binding"))?,
        Expr::FieldAccess(target, field) => {
            // A nullary `E::V` — parsed as field access on the enum
            // name — constructs the variant directly; `match` reads
            // the produced value. A bound variable of the same name
            // wins: `Shape.x` on a `Shape` parameter or `let` is a
            // field read, never a constructor.
            if let Expr::Variable(enum_name) = target.as_ref() {
                if let Some((resolved_name, variants)) = call_returns
                    .enums
                    .get_key_value(enum_name)
                    .filter(|_| !env.contains_key(enum_name))
                {
                    let Some(ordinal) = variants.iter().position(|(v, _)| v == field) else {
                        return Err(SemanticContractError::Unsupported(
                            "undeclared enum variant",
                        ));
                    };
                    if !variants[ordinal].1.is_empty() {
                        return Err(SemanticContractError::Unsupported("enum constructor arity"));
                    }
                    return Ok(SourceValue::Enum {
                        name: resolved_name.clone(),
                        tag: BV::from_u64(ctx, ordinal as u64, 64),
                        slots: (0..layout_arity(variants))
                            .map(|i| call_returns.default_enum_slot(variants, i, ctx))
                            .collect(),
                    });
                }
            }
            if !matches!(
                target.as_ref(),
                Expr::Variable(_) | Expr::Call(..) | Expr::FieldAccess(..)
            ) {
                return Err(SemanticContractError::Unsupported("source field read"));
            }
            let SourceValue::Composite(fields) = source_expr(
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
                enums,
            )?
            else {
                return Err(SemanticContractError::Unsupported("source field read"));
            };
            fields
                .into_iter()
                .find(|(name, _)| name == field)
                .map(|(_, value)| value)
                .ok_or(SemanticContractError::Unsupported("source field read"))?
        }
        // A struct literal's fields encode in written order (each one's
        // partial operations record definedness under the current
        // guard), then the value lays out in declaration order.
        Expr::StructInit { type_name, fields } => {
            let Some(declared) = call_returns.structs.get(type_name.as_str()) else {
                return Err(SemanticContractError::Unsupported("source struct literal"));
            };
            let mut values = HashMap::new();
            for (name, value) in fields {
                let value = source_expr(
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
                    enums,
                )?;
                // A nested struct field takes a composite value —
                // everything else must be scalar (`v8` declarations
                // only ever produce these two shapes). A `Number` literal
                // lands under the declared leaf: `Unsigned` in a `u64`
                // field, `Integer` in an `i64` one — otherwise the slot
                // would keep adapting and later reads could go signed
                // where the field is unsigned (`pure-i64-composite/v10`).
                let Some((_, declared_shape)) =
                    declared.iter().find(|(decl_name, _)| decl_name == name)
                else {
                    return Err(SemanticContractError::Unsupported("source struct literal"));
                };
                let value = match (declared_shape, value) {
                    (ValueKind::Scalar(ScalarKind::I64), SourceValue::Number(v)) => {
                        SourceValue::Integer(v)
                    }
                    (ValueKind::Scalar(ScalarKind::U64), SourceValue::Number(v)) => {
                        SourceValue::Unsigned(v)
                    }
                    // A literal under a wide leaf extends its 64 bits
                    // into the carrier — sign for `i128`, zero for
                    // `u128` (`pure-i64-composite/v21`).
                    (ValueKind::Scalar(ScalarKind::I128), SourceValue::Number(v)) => {
                        SourceValue::Wide(v.sign_ext(64))
                    }
                    (ValueKind::Scalar(ScalarKind::U128), SourceValue::Number(v)) => {
                        SourceValue::UWide(v.zero_ext(64))
                    }
                    (_, value) => value,
                };
                let shape_ok = match (declared_shape, &value) {
                    // An enum-typed field takes an enum value of the
                    // SAME declared enum (`pure-i64-composite/v17`) —
                    // a different enum's value fails closed like the
                    // field-write check does.
                    (ValueKind::Enum(declared), SourceValue::Enum { name: written, .. }) => {
                        declared == written
                    }
                    (declared_shape, value) => matches!(
                        (declared_shape, value),
                        (
                            ValueKind::Scalar(ScalarKind::I64),
                            SourceValue::Integer(_) | SourceValue::Number(_)
                        ) | (
                            ValueKind::Scalar(ScalarKind::U64),
                            SourceValue::Unsigned(_) | SourceValue::Number(_)
                        ) | (ValueKind::Scalar(ScalarKind::Bool), SourceValue::Boolean(_))
                            | (ValueKind::Scalar(ScalarKind::F64), SourceValue::Float64(_))
                            | (ValueKind::Scalar(ScalarKind::I128), SourceValue::Wide(_))
                            | (ValueKind::Scalar(ScalarKind::U128), SourceValue::UWide(_))
                            | (ValueKind::Struct(_), SourceValue::Composite(_))
                    ),
                };
                if !shape_ok || values.insert(name, value).is_some() {
                    return Err(SemanticContractError::Unsupported("source struct literal"));
                }
            }
            let mut out = Vec::with_capacity(declared.len());
            for (name, _) in declared {
                let Some(value) = values.remove(name) else {
                    return Err(SemanticContractError::Unsupported("source struct literal"));
                };
                out.push((name.clone(), value));
            }
            if !values.is_empty() {
                return Err(SemanticContractError::Unsupported("source struct literal"));
            }
            SourceValue::Composite(out)
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
            enums,
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
                enums,
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
                enums,
            )?;
            // A literal against a 128-bit operand zero-extends into
            // the wide carrier — its nonnegative `u64` value is the
            // same bits in both widths (`pure-i64-composite/v21`).
            let (a, b) = widen_literal_operands(a, b);
            match (a, op, b) {
                (I(a) | N(a), Op::Eq, I(b) | N(b))
                | (U(a), Op::Eq, U(b) | N(b))
                | (N(a), Op::Eq, U(b)) => B(a._eq(&b)),
                (I(a) | N(a), Op::Neq, I(b) | N(b))
                | (U(a), Op::Neq, U(b) | N(b))
                | (N(a), Op::Neq, U(b)) => B(a._eq(&b).not()),
                (B(a), Op::Eq, B(b)) => B(a._eq(&b)),
                (B(a), Op::Neq, B(b)) => B(a._eq(&b).not()),
                // Signed comparisons: i64 operands plus literals.
                (I(a) | N(a), Op::Lt, I(b) | N(b)) => B(a.bvslt(&b)),
                (I(a) | N(a), Op::Le, I(b) | N(b)) => B(a.bvsle(&b)),
                (I(a) | N(a), Op::Gt, I(b) | N(b)) => B(a.bvsgt(&b)),
                (I(a) | N(a), Op::Ge, I(b) | N(b)) => B(a.bvsge(&b)),
                // Unsigned comparisons (`pure-i64-composite/v10`): a
                // `u64` operand fixes the comparison — literals adapt,
                // a declared `i64` operand on the other side fails
                // closed through to the reject arm.
                (U(a), Op::Lt, U(b) | N(b)) | (N(a), Op::Lt, U(b)) => B(a.bvult(&b)),
                (U(a), Op::Le, U(b) | N(b)) | (N(a), Op::Le, U(b)) => B(a.bvule(&b)),
                (U(a), Op::Gt, U(b) | N(b)) | (N(a), Op::Gt, U(b)) => B(a.bvugt(&b)),
                (U(a), Op::Ge, U(b) | N(b)) | (N(a), Op::Ge, U(b)) => B(a.bvuge(&b)),
                // Literal-only arithmetic keeps the literal carrier so
                // `0 - 1` still adapts when it meets a u64 operand.
                (N(a), Op::Add, N(b)) => N(a.bvadd(&b)),
                (N(a), Op::Sub, N(b)) => N(a.bvsub(&b)),
                (N(a), Op::Mul, N(b)) => N(a.bvmul(&b)),
                (N(a), Op::BitAnd, N(b)) => N(a.bvand(&b)),
                (N(a), Op::BitOr, N(b)) => N(a.bvor(&b)),
                (N(a), Op::BitXor, N(b)) => N(a.bvxor(&b)),
                (N(a), Op::Div, N(b)) => {
                    definedness.push(guard.implies(&div_defined(&a, &b, ctx)));
                    N(a.bvsdiv(&b))
                }
                (N(a), Op::Rem, N(b)) => {
                    definedness.push(guard.implies(&div_defined(&a, &b, ctx)));
                    N(a.bvsrem(&b))
                }
                (N(a), Op::Shl, N(b)) => {
                    definedness.push(guard.implies(&shift_defined(&b, ctx)));
                    N(a.bvshl(&b))
                }
                (N(a), Op::Shr, N(b)) => {
                    definedness.push(guard.implies(&shift_defined(&b, ctx)));
                    N(a.bvashr(&b))
                }
                // i64 arithmetic wraps mod 2^64, matching the emitted
                // `add`/`sub`/`mul` (the emitter never sets nsw/nuw).
                (I(a) | N(a), Op::Add, I(b) | N(b)) => I(a.bvadd(&b)),
                (I(a) | N(a), Op::Sub, I(b) | N(b)) => I(a.bvsub(&b)),
                (I(a) | N(a), Op::Mul, I(b) | N(b)) => I(a.bvmul(&b)),
                // Bitwise ops are total on i64 — no definedness obligation.
                (I(a) | N(a), Op::BitAnd, I(b) | N(b)) => I(a.bvand(&b)),
                (I(a) | N(a), Op::BitOr, I(b) | N(b)) => I(a.bvor(&b)),
                (I(a) | N(a), Op::BitXor, I(b) | N(b)) => I(a.bvxor(&b)),
                // `bvsdiv`/`bvsrem` truncate like the emitted `sdiv`/`srem`;
                // both sides are partial in the same way, so the defined
                // domain is an explicit obligation, not an assumption.
                (I(a) | N(a), Op::Div, I(b) | N(b)) => {
                    definedness.push(guard.implies(&div_defined(&a, &b, ctx)));
                    I(a.bvsdiv(&b))
                }
                (I(a) | N(a), Op::Rem, I(b) | N(b)) => {
                    definedness.push(guard.implies(&div_defined(&a, &b, ctx)));
                    I(a.bvsrem(&b))
                }
                (I(a) | N(a), Op::Shl, I(b) | N(b)) => {
                    definedness.push(guard.implies(&shift_defined(&b, ctx)));
                    I(a.bvshl(&b))
                }
                // `>>` is the arithmetic (sign-propagating) shift `ashr`.
                (I(a) | N(a), Op::Shr, I(b) | N(b)) => {
                    definedness.push(guard.implies(&shift_defined(&b, ctx)));
                    I(a.bvashr(&b))
                }
                // `u64` arithmetic: same wrapping add/sub/mul and the
                // same bitwise ops, but `bvudiv`/`bvurem`/`bvlshr` —
                // whose only domain bound is a nonzero divisor / in-range
                // amount (no `i64::MIN / -1` pair exists unsigned).
                (U(a), Op::Add, U(b) | N(b)) | (N(a), Op::Add, U(b)) => U(a.bvadd(&b)),
                (U(a), Op::Sub, U(b) | N(b)) | (N(a), Op::Sub, U(b)) => U(a.bvsub(&b)),
                (U(a), Op::Mul, U(b) | N(b)) | (N(a), Op::Mul, U(b)) => U(a.bvmul(&b)),
                (U(a), Op::BitAnd, U(b) | N(b)) | (N(a), Op::BitAnd, U(b)) => U(a.bvand(&b)),
                (U(a), Op::BitOr, U(b) | N(b)) | (N(a), Op::BitOr, U(b)) => U(a.bvor(&b)),
                (U(a), Op::BitXor, U(b) | N(b)) | (N(a), Op::BitXor, U(b)) => U(a.bvxor(&b)),
                (U(a), Op::Div, U(b) | N(b)) | (N(a), Op::Div, U(b)) => {
                    definedness.push(guard.implies(&udiv_defined(&b, ctx)));
                    U(a.bvudiv(&b))
                }
                (U(a), Op::Rem, U(b) | N(b)) | (N(a), Op::Rem, U(b)) => {
                    definedness.push(guard.implies(&udiv_defined(&b, ctx)));
                    U(a.bvurem(&b))
                }
                (U(a), Op::Shl, U(b) | N(b)) | (N(a), Op::Shl, U(b)) => {
                    definedness.push(guard.implies(&shift_defined(&b, ctx)));
                    U(a.bvshl(&b))
                }
                (U(a), Op::Shr, U(b) | N(b)) | (N(a), Op::Shr, U(b)) => {
                    definedness.push(guard.implies(&shift_defined(&b, ctx)));
                    U(a.bvlshr(&b))
                }
                // `i128` (`pure-i64-composite/v21`): the signed ops of
                // `i64` on the full 128-bit vector — literals arrive
                // already widened by `widen_literal_operands`, so a
                // surviving `Num` on the other side fails closed.
                (W(a), Op::Eq, W(b)) => B(a._eq(&b)),
                (W(a), Op::Neq, W(b)) => B(a._eq(&b).not()),
                (W(a), Op::Lt, W(b)) => B(a.bvslt(&b)),
                (W(a), Op::Le, W(b)) => B(a.bvsle(&b)),
                (W(a), Op::Gt, W(b)) => B(a.bvsgt(&b)),
                (W(a), Op::Ge, W(b)) => B(a.bvsge(&b)),
                (W(a), Op::Add, W(b)) => W(a.bvadd(&b)),
                (W(a), Op::Sub, W(b)) => W(a.bvsub(&b)),
                (W(a), Op::Mul, W(b)) => W(a.bvmul(&b)),
                (W(a), Op::BitAnd, W(b)) => W(a.bvand(&b)),
                (W(a), Op::BitOr, W(b)) => W(a.bvor(&b)),
                (W(a), Op::BitXor, W(b)) => W(a.bvxor(&b)),
                (W(a), Op::Div, W(b)) => {
                    definedness.push(guard.implies(&div_defined_wide(&a, &b, ctx)));
                    W(a.bvsdiv(&b))
                }
                (W(a), Op::Rem, W(b)) => {
                    definedness.push(guard.implies(&div_defined_wide(&a, &b, ctx)));
                    W(a.bvsrem(&b))
                }
                (W(a), Op::Shl, W(b)) => {
                    definedness.push(guard.implies(&shift_defined_wide(&b, ctx)));
                    W(a.bvshl(&b))
                }
                (W(a), Op::Shr, W(b)) => {
                    definedness.push(guard.implies(&shift_defined_wide(&b, ctx)));
                    W(a.bvashr(&b))
                }
                // `u128`: the unsigned counterparts — `bvult`-family
                // comparisons, `bvudiv`/`bvurem`/`bvlshr`, with only a
                // nonzero-divisor / in-range-amount domain bound.
                (UW(a), Op::Eq, UW(b)) => B(a._eq(&b)),
                (UW(a), Op::Neq, UW(b)) => B(a._eq(&b).not()),
                (UW(a), Op::Lt, UW(b)) => B(a.bvult(&b)),
                (UW(a), Op::Le, UW(b)) => B(a.bvule(&b)),
                (UW(a), Op::Gt, UW(b)) => B(a.bvugt(&b)),
                (UW(a), Op::Ge, UW(b)) => B(a.bvuge(&b)),
                (UW(a), Op::Add, UW(b)) => UW(a.bvadd(&b)),
                (UW(a), Op::Sub, UW(b)) => UW(a.bvsub(&b)),
                (UW(a), Op::Mul, UW(b)) => UW(a.bvmul(&b)),
                (UW(a), Op::BitAnd, UW(b)) => UW(a.bvand(&b)),
                (UW(a), Op::BitOr, UW(b)) => UW(a.bvor(&b)),
                (UW(a), Op::BitXor, UW(b)) => UW(a.bvxor(&b)),
                (UW(a), Op::Div, UW(b)) => {
                    definedness.push(guard.implies(&udiv_defined_wide(&b, ctx)));
                    UW(a.bvudiv(&b))
                }
                (UW(a), Op::Rem, UW(b)) => {
                    definedness.push(guard.implies(&udiv_defined_wide(&b, ctx)));
                    UW(a.bvurem(&b))
                }
                (UW(a), Op::Shl, UW(b)) => {
                    definedness.push(guard.implies(&shift_defined_wide(&b, ctx)));
                    UW(a.bvshl(&b))
                }
                (UW(a), Op::Shr, UW(b)) => {
                    definedness.push(guard.implies(&shift_defined_wide(&b, ctx)));
                    UW(a.bvlshr(&b))
                }
                (B(a), Op::And, B(b)) => B(Bool::and(ctx, &[&a, &b])),
                (B(a), Op::Or, B(b)) => B(Bool::or(ctx, &[&a, &b])),
                (B(a), Op::Implies, B(b)) => B(a.implies(&b)),
                // Enum equality (`pure-i64-composite/v14`): the emitter
                // lowers `e == e'` to a positional `extractvalue`/`icmp`
                // chain over the whole tagged-union layout — tag plus
                // every payload slot, including padding — so the model
                // compares all slots, not only the variant-declared
                // ones. Only one declared enum may meet on both sides.
                (
                    SourceValue::Enum {
                        name: an,
                        tag: at,
                        slots: asl,
                    },
                    Op::Eq,
                    SourceValue::Enum {
                        name: bn,
                        tag: bt,
                        slots: bsl,
                    },
                )
                | (
                    SourceValue::Enum {
                        name: an,
                        tag: at,
                        slots: asl,
                    },
                    Op::Neq,
                    SourceValue::Enum {
                        name: bn,
                        tag: bt,
                        slots: bsl,
                    },
                ) if an == bn && asl.len() == bsl.len() => {
                    let mut conjuncts = Vec::with_capacity(asl.len() + 1);
                    conjuncts.push(at._eq(&bt));
                    for (x, y) in asl.iter().zip(bsl.iter()) {
                        let Some(eq) = enum_slot_eq(x, y, ctx) else {
                            return Err(SemanticContractError::Unsupported("enum slot equality"));
                        };
                        conjuncts.push(eq);
                    }
                    let conjuncts: Vec<&Bool> = conjuncts.iter().collect();
                    let eq = Bool::and(ctx, &conjuncts);
                    B(match op {
                        Op::Neq => eq.not(),
                        _ => eq,
                    })
                }
                // `f64` arithmetic (`pure-i64-composite/v19`): the
                // bit-pattern leaf decodes to IEEE 754 binary64,
                // computes under round-nearest-ties-to-even and
                // re-encodes — the value the emitted
                // `fadd`/`fsub`/`fmul`/`fdiv` produces. `==` lowers
                // to `fcmp oeq`, the `fp.eq` IEEE equality.
                (F(a), Op::Add, F(b)) => F(f64_arith(ctx, &a, &b, F64Arith::Add)),
                (F(a), Op::Sub, F(b)) => F(f64_arith(ctx, &a, &b, F64Arith::Sub)),
                (F(a), Op::Mul, F(b)) => F(f64_arith(ctx, &a, &b, F64Arith::Mul)),
                (F(a), Op::Div, F(b)) => F(f64_arith(ctx, &a, &b, F64Arith::Div)),
                (F(a), Op::Eq, F(b)) => B(f64_eq(ctx, &a, &b)),
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
                enums,
            )?
            .boolean()?;
            // Branches may rebind linear-path names: each write merges
            // under the branch condition like the emitter's per-name
            // phi; a name written on one side keeps its prior value on
            // the other. Branch-local `let` bindings are lexical — they
            // never reach `then_writes`/`else_writes`.
            let mut then_writes = HashMap::new();
            let mut else_writes = HashMap::new();
            // The canonical per-`alloc` stores branch the same way the
            // name-level writes do: snapshot each side's end state and
            // merge every shared backing under the condition — a
            // shadowed binding's array joins no `written` name but
            // still shares the merged backing.
            let pre_alloc_stores = call_returns.alloc_stores.borrow().clone();
            let then_value = source_stmt(
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
                enums,
            )?;
            let then_alloc_stores = call_returns.alloc_stores.borrow().clone();
            *call_returns.alloc_stores.borrow_mut() = pre_alloc_stores;
            let else_value = source_stmt(
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
                enums,
            )?;
            let else_alloc_stores = call_returns.alloc_stores.borrow().clone();
            merge_alloc_stores(call_returns, &cond, &then_alloc_stores, &else_alloc_stores);
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
                let merged = merge_source_values(
                    &cond,
                    then_bound.clone(),
                    else_bound.clone(),
                    call_returns,
                )?;
                seed_merged_alloc(
                    call_returns,
                    &cond,
                    &merged,
                    &then_bound,
                    &else_bound,
                    &then_alloc_stores,
                    &else_alloc_stores,
                );
                written.insert(name.clone(), merged);
            }
            let merged_value =
                merge_source_values(&cond, then_value.clone(), else_value.clone(), call_returns)?;
            seed_merged_alloc(
                call_returns,
                &cond,
                &merged_value,
                &then_value,
                &else_value,
                &then_alloc_stores,
                &else_alloc_stores,
            );
            merged_value
        }
        // First-match order, like the emitter's try-chain and
        // `--bitvec-i64` verification: an arm runs under the negation of
        // every earlier arm's hit (`pattern ∧ guard`) plus its own; an
        // unguarded `Variable`/`Wildcard` arm matches everything that
        // remains, so any later arm is dead — it gets the literal
        // `false` miss term, which encodes it while binding nothing.
        Expr::Match { target, arms } => {
            let target_value = source_expr(
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
                enums,
            )?;
            // An enum scrutinee carries a tag plus the shared payload
            // slots of its tagged-union layout: a `Variant` arm's hit is
            // `tag == ordinal` and its binders take the slot values the
            // emitter's `extractvalue` reads produce.
            let enum_target = match &target_value {
                SourceValue::Enum { .. } => Some(&target_value),
                _ => None,
            };
            let target = match enum_target {
                Some(_) => None,
                None => Some(target_value.clone().integer()?),
            };
            let mut missed: Vec<Bool> = Vec::new();
            // Each arm also carries the canonical per-`alloc` state its
            // body produced, so a merged array-valued result seeds its
            // fresh backing from that arm's actual stores.
            let mut selected: Vec<(Bool, SourceValue, HashMap<u64, Array>)> = Vec::new();
            let mut fallback: Option<(SourceValue, HashMap<u64, Array>)> = None;
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
                let (pat_cond, bindings): (Option<Bool>, Vec<(String, SourceValue)>) =
                    match (&arm.pattern, enum_target) {
                        (Pattern::Literal(n), None) => (
                            Some(target.as_ref().unwrap()._eq(&BV::from_i64(ctx, *n, 64))),
                            Vec::new(),
                        ),
                        (Pattern::Variable(name), None) => {
                            // The arm binding keeps the scrutinee's scalar
                            // wrapper — a `u64` scrutinee binds an `Unsigned`
                            // (`pure-i64-composite/v10`), not a signed leaf.
                            let bound = match &target_value {
                                I(_) | U(_) | N(_) => target_value.clone(),
                                _ => {
                                    return Err(SemanticContractError::Unsupported(
                                        "match scrutinee not scalar",
                                    ));
                                }
                            };
                            (None, vec![(name.clone(), bound)])
                        }
                        (Pattern::Wildcard, _) => (None, Vec::new()),
                        (
                            Pattern::Variant {
                                variant_name,
                                fields,
                            },
                            Some(SourceValue::Enum { name, tag, slots }),
                        ) => {
                            let variants = call_returns.enums.get(name.as_str()).ok_or(
                                SemanticContractError::Unsupported("match scrutinee enum"),
                            )?;
                            let short = variant_name.rsplit("::").next().unwrap_or(variant_name);
                            let Some((ordinal, (_, payload_kinds))) = variants
                                .iter()
                                .enumerate()
                                .find(|(_, (variant, _))| variant == short)
                            else {
                                return Err(SemanticContractError::Unsupported(
                                    "match variant outside enum",
                                ));
                            };
                            if payload_kinds.len() != fields.len() {
                                return Err(SemanticContractError::Unsupported(
                                    "match variant arity",
                                ));
                            }
                            let mut bindings = Vec::with_capacity(fields.len());
                            for (field_pattern, slot) in fields.iter().zip(slots.iter()) {
                                match field_pattern {
                                    Pattern::Variable(name) => {
                                        bindings.push((name.clone(), slot.clone()));
                                    }
                                    Pattern::Wildcard => (),
                                    _ => {
                                        return Err(SemanticContractError::Unsupported(
                                            "match variant field pattern",
                                        ));
                                    }
                                }
                            }
                            (
                                Some(tag._eq(&BV::from_u64(ctx, ordinal as u64, 64))),
                                bindings,
                            )
                        }
                        (Pattern::Variant { .. }, _) | (Pattern::Variable(_), Some(_)) => {
                            return Err(SemanticContractError::Unsupported(
                                "match pattern outside variants and catch-all",
                            ));
                        }
                        (Pattern::Literal(_), Some(_)) => {
                            return Err(SemanticContractError::Unsupported(
                                "literal match arm on an enum",
                            ));
                        }
                    };
                // A binding shadows an outer name for this arm, so it
                // leaves the assignable set and its writes stay
                // arm-local.
                let mut arm_assignable = assignable.clone();
                for (name, _) in &bindings {
                    arm_assignable.remove(name);
                }
                let mut arm_env = env.clone();
                for (name, value) in bindings {
                    arm_env.insert(name, value);
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
                                enums,
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
                let pre_arm_stores = call_returns.alloc_stores.borrow().clone();
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
                    enums,
                )?;
                let post_arm_stores = call_returns.alloc_stores.borrow().clone();
                // Arm writes escape like `if` branch writes: each merges
                // under the arm-local hit (earlier arms' misses and this
                // arm's `pattern ∧ guard`), matching the emitter's
                // match.merge per-name phi; a dead arm's merge condition
                // is unsatisfiable so its writes never escape.
                let arm_hit = match &hit {
                    Some(h) => Bool::and(ctx, &[&arm_reach, h]),
                    None => arm_reach.clone(),
                };
                // The canonical per-`alloc` stores merge under the same
                // arm hit — a backing written through a shadowed name
                // has no `arm_writes` entry but still merges.
                {
                    let mut stores = call_returns.alloc_stores.borrow_mut();
                    for (alloc, store) in stores.iter_mut() {
                        if let Some(pre) = pre_arm_stores.get(alloc) {
                            *store = arm_hit.ite(store, pre);
                        }
                    }
                }
                for (name, bound) in arm_writes {
                    let prior = written
                        .get(&name)
                        .cloned()
                        .or_else(|| env.get(&name).cloned())
                        .ok_or(SemanticContractError::Unsupported("source binding"))?;
                    let merged =
                        merge_source_values(&arm_hit, bound.clone(), prior.clone(), call_returns)?;
                    seed_merged_alloc(
                        call_returns,
                        &arm_hit,
                        &merged,
                        &bound,
                        &prior,
                        &post_arm_stores,
                        &pre_arm_stores,
                    );
                    written.insert(name, merged);
                }
                if let Some(cond) = hit {
                    selected.push((cond, value, post_arm_stores));
                } else if fallback.is_none() {
                    fallback = Some((value, post_arm_stores));
                }
            }
            let Some((mut result, _)) = fallback else {
                return Err(SemanticContractError::Unsupported(
                    "match without a catch-all arm",
                ));
            };
            for (cond, value, value_stores) in selected.into_iter().rev() {
                let merged =
                    merge_source_values(&cond, value.clone(), result.clone(), call_returns)?;
                // The prior side's canonical state is the running map —
                // every earlier fold already merged into it.
                let prior_stores = call_returns.alloc_stores.borrow().clone();
                seed_merged_alloc(
                    call_returns,
                    &cond,
                    &merged,
                    &value,
                    &result,
                    &value_stores,
                    &prior_stores,
                );
                result = merged;
            }
            result
        }
        // A fixed-size array literal (`bounded-memory`): the zero-filled
        // `BV64 -> BV64` array with each element stored at its literal
        // index — the same cells the emitted `alloca [N x i64]` +
        // per-element `store` writes.
        Expr::ArrayLit(elems) => {
            if elems.is_empty() {
                return Err(SemanticContractError::Unsupported("empty array literal"));
            }
            let zero = BV::from_u64(ctx, 0, 64);
            let mut store = Array::const_array(ctx, &Sort::bitvector(ctx, 64), &zero);
            let mut unsigned = false;
            for (i, elem) in elems.iter().enumerate() {
                let value = source_expr(
                    elem,
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
                    enums,
                )?;
                let cell = match value {
                    I(v) | N(v) => v,
                    U(v) => {
                        unsigned = true;
                        v
                    }
                    _ => return Err(SemanticContractError::Unsupported("array literal element")),
                };
                store = store.store(&BV::from_u64(ctx, i as u64, 64), &cell);
            }
            let alloc = call_returns.fresh_array_alloc();
            // The canonical backing-store entry: every binding on this
            // `alloc` reads and writes through `alloc_stores`.
            call_returns
                .alloc_stores
                .borrow_mut()
                .insert(alloc, store.clone());
            SourceValue::Array {
                store,
                len: BV::from_u64(ctx, elems.len() as u64, 64),
                elem: if unsigned {
                    ScalarKind::U64
                } else {
                    ScalarKind::I64
                },
                alloc,
            }
        }
        // A `Str` literal (`pure-i64-composite/v13`): one `BV64 -> BV64`
        // array under the bounded-memory model, each byte stored as a
        // constant cell — the same cells the emitted `alloca [N x i64]`
        // + per-element `store` writes. `elem: Str` marks the backing
        // immutable: the kind gate refuses `s[i] = v` and `s = _`.
        Expr::StringLit(s) => {
            if s.len() > call_returns.str_bound.get() {
                return Err(SemanticContractError::Unsupported("str literal bound"));
            }
            let zero = BV::from_u64(ctx, 0, 64);
            let mut store = Array::const_array(ctx, &Sort::bitvector(ctx, 64), &zero);
            for (i, byte) in s.bytes().enumerate() {
                store = store.store(
                    &BV::from_u64(ctx, i as u64, 64),
                    &BV::from_u64(ctx, byte as u64, 64),
                );
            }
            let alloc = call_returns.fresh_array_alloc();
            call_returns
                .alloc_stores
                .borrow_mut()
                .insert(alloc, store.clone());
            SourceValue::Array {
                store,
                len: BV::from_u64(ctx, s.len() as u64, 64),
                elem: ScalarKind::Str,
                alloc,
            }
        }
        // An indexed read `a[i]`: the in-bounds `select` or the merged
        // `0` an out-of-bounds read yields — no definedness obligation,
        // matching the emitted bounds-check + `phi(loaded, 0)`.
        Expr::ArrayAccess(name, index) => {
            let Some(SourceValue::Array {
                store,
                len,
                elem,
                alloc,
            }) = env.get(name.as_str())
            else {
                return Err(SemanticContractError::Unsupported(
                    "array access on a non-array binding",
                ));
            };
            let index = source_expr(
                index,
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
                enums,
            )?
            .integer()?;
            // The canonical store carries every indexed write to this
            // backing — including ones a shadowed sibling wrote.
            let read = array_read(
                ctx,
                &canonical_store(call_returns, *alloc, store),
                len,
                &index,
            );
            match elem {
                // `s[i]` on a `Str` backing reads a byte — a `u64`
                // leaf like a `u64` cell (`pure-i64-composite/v13`).
                ScalarKind::U64 | ScalarKind::Str => U(read),
                _ => I(read),
            }
        }
        Expr::Call(name, args) => {
            // `len(a)` is the stored element count — no memory op on
            // the IR side (the let's SSA value carries it). Only a
            // binding (or inline literal) actually producing an array
            // intercepts; anything else resolves as an ordinary call,
            // so a user atom named `len` keeps working.
            if name == "len" && args.len() == 1 {
                let len = match &args[0] {
                    Expr::Variable(var) => env.get(var.as_str()).and_then(|value| {
                        if let SourceValue::Array { len, .. } = value {
                            Some(len.clone())
                        } else {
                            None
                        }
                    }),
                    Expr::ArrayLit(elems) => Some(BV::from_u64(ctx, elems.len() as u64, 64)),
                    _ => None,
                };
                if let Some(len) = len {
                    return Ok(I(len));
                }
            }
            // An `E::V(args..)` enum constructor produces the variant's
            // value directly — tag ordinal plus payload slots — not a
            // call site: it carries no obligation domain.
            if let Some((enum_name, variant_name)) = name.split_once("::") {
                if let Some((resolved_name, variants)) = call_returns
                    .enums
                    .get_key_value(enum_name)
                    .filter(|_| !call_returns.returns.contains_key(name))
                {
                    let Some(ordinal) = variants.iter().position(|(v, _)| v == variant_name) else {
                        return Err(SemanticContractError::Unsupported(
                            "undeclared enum variant",
                        ));
                    };
                    let payload_kinds = &variants[ordinal].1;
                    if args.len() != payload_kinds.len() {
                        return Err(SemanticContractError::Unsupported("enum constructor arity"));
                    }
                    let mut slots = Vec::with_capacity(layout_arity(variants));
                    for (arg, kind) in args.iter().zip(payload_kinds.iter()) {
                        let value = source_expr(
                            arg,
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
                            enums,
                        )?;
                        match kind {
                            // A wide payload leaf takes the `Wide`/
                            // `UWide` value as-is — the ctor-kind gate
                            // already matched its kind
                            // (`pure-i64-composite/v22`).
                            PayloadSlotKind::Scalar(ScalarKind::I128 | ScalarKind::U128) => {
                                match value {
                                    SourceValue::Wide(_) | SourceValue::UWide(_) => {
                                        slots.push(value)
                                    }
                                    _ => {
                                        return Err(SemanticContractError::Unsupported(
                                            "enum constructor payload kind",
                                        ))
                                    }
                                }
                            }
                            PayloadSlotKind::Scalar(kind) => {
                                slots.push(enum_slot_value(*kind, &lower_scalar(value, ctx)?, ctx))
                            }
                            // A composite payload takes the
                            // `SourceValue::Composite` whole — the
                            // ctor-kind gate already matched its
                            // struct name (`pure-i64-composite/v18`).
                            PayloadSlotKind::Composite(_) => match value {
                                SourceValue::Composite(_) => slots.push(value),
                                _ => {
                                    return Err(SemanticContractError::Unsupported(
                                        "enum constructor payload kind",
                                    ))
                                }
                            },
                        }
                    }
                    for i in payload_kinds.len()..layout_arity(variants) {
                        slots.push(call_returns.default_enum_slot(variants, i, ctx));
                    }
                    return Ok(SourceValue::Enum {
                        name: resolved_name.clone(),
                        tag: BV::from_u64(ctx, ordinal as u64, 64),
                        slots,
                    });
                }
            }
            // Call arguments carry the profile's lowered representation:
            // the extraction gate checked each argument's kind against
            // the callee parameter, and a `bool` argument lowers to the
            // same i64 0/1 bitvector a bool parameter does — so the call
            // site and `signature_domain`/`param_env` see one encoding.
            let mut applied = Vec::with_capacity(args.len());
            for arg in args {
                let lower = |value: SourceValue<'ctx>| lower_scalar(value, ctx);
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
                    enums,
                )? {
                    // A passed-through struct parameter flattens into its
                    // leaf slots — the callee's `param_env` and
                    // `signature_domain` consume the same order.
                    SourceValue::Composite(fields) => {
                        for (_, field) in fields {
                            lower_composite(field, ctx, &mut applied)?;
                        }
                    }
                    // A passed-through enum parameter flattens into its
                    // tag slot then the shared payload slots — the same
                    // order `signature_domain` and `param_env` consume.
                    SourceValue::Enum { tag, slots, .. } => {
                        applied.push(tag);
                        for slot in slots {
                            lower_composite(slot, ctx, &mut applied)?;
                        }
                    }
                    scalar => {
                        let lowered = lower(scalar)?;
                        if lowered.get_size() == 128 {
                            applied.push(lowered.extract(63, 0));
                            applied.push(lowered.extract(127, 64));
                        } else {
                            applied.push(lowered);
                        }
                    }
                }
            }
            let refs: Vec<&dyn Ast> = applied.iter().map(|v| v as &dyn Ast).collect();
            // A struct-returning call applies one uninterpreted function
            // per field slot; the site records their `aggregate_value`.
            if let Some(fields) = call_returns.result_fields(name) {
                let leaf_count = call_returns.flattened_kinds(fields).len();
                let slots = (0..leaf_count)
                    .map(|i| {
                        call_field_decl(ctx, name, applied.len(), i)
                            .apply(&refs)
                            .as_bv()
                    })
                    .collect::<Option<Vec<_>>>()
                    .ok_or(SemanticContractError::Unsupported(
                        "source call result kind",
                    ))?;
                call_sites.push(CallSite {
                    callee: name.clone(),
                    args: applied.clone(),
                    result: aggregate_value(&slots),
                    guard: guard.clone(),
                });
                return Ok(composite_from_slots(
                    call_returns,
                    fields,
                    &mut slots.iter(),
                    ctx,
                ));
            }
            // An enum-returning call applies one uninterpreted function
            // per flattened slot — the tag, then each shared payload
            // slot — and the site records their `aggregate_value`.
            if let Some((enum_name, variants)) = call_returns.result_enum(name) {
                let slots = (0..=call_returns.enum_layout_leaf_count(variants))
                    .map(|i| {
                        call_field_decl(ctx, name, applied.len(), i)
                            .apply(&refs)
                            .as_bv()
                    })
                    .collect::<Option<Vec<_>>>()
                    .ok_or(SemanticContractError::Unsupported(
                        "source call result kind",
                    ))?;
                call_sites.push(CallSite {
                    callee: name.clone(),
                    args: applied.clone(),
                    result: aggregate_value(&slots),
                    guard: guard.clone(),
                });
                return Ok(SourceValue::Enum {
                    name: enum_name.to_owned(),
                    tag: slots[0].clone(),
                    slots: call_returns.seed_enum_slots(variants, &mut slots[1..].iter(), ctx),
                });
            }
            // A wide scalar result is two consecutive 64-bit slots —
            // the same per-slot uninterpreted applications a composite
            // result gets (`pure-i64-composite/v21`).
            if call_returns.returns_wide(name) {
                let slots = (0..2)
                    .map(|i| {
                        call_field_decl(ctx, name, applied.len(), i)
                            .apply(&refs)
                            .as_bv()
                    })
                    .collect::<Option<Vec<_>>>()
                    .ok_or(SemanticContractError::Unsupported(
                        "source call result kind",
                    ))?;
                call_sites.push(CallSite {
                    callee: name.clone(),
                    args: applied.clone(),
                    result: aggregate_value(&slots),
                    guard: guard.clone(),
                });
                let agg = aggregate_value(&slots);
                return Ok(if call_returns.returns_u128(name) {
                    UW(agg)
                } else {
                    W(agg)
                });
            }
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
            if call_returns.returns_u64(name) {
                U(value)
            } else if call_returns.returns_f64(name) {
                F(value)
            } else if call_returns.returns_bool(name) {
                B(value._eq(&BV::from_u64(ctx, 0, 64)).not())
            } else {
                I(value)
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
        let atom = "atom gy(p: P) -> i64 requires: true; ensures: result == p.y; body: { p.y };";
        let wide = format!("struct P {{ x: i64, y: i64 }}\n{atom}\n{MAIN0}");
        let narrow = format!("struct P {{ x: bool, y: i64 }}\n{atom}\n{MAIN0}");
        let swapped = format!("struct P {{ y: i64, x: i64 }}\n{atom}\n{MAIN0}");
        let wide = composite(&wide).unwrap();
        let narrow = composite(&narrow).unwrap();
        let swapped = composite(&swapped).unwrap();
        assert_ne!(wide[0].specification_sha256, narrow[0].specification_sha256);
        assert_ne!(
            wide[0].specification_sha256,
            swapped[0].specification_sha256
        );
        assert_eq!(narrow[0].specification.binders[0].scalar_type, "bool");
    }

    #[test]
    fn composite_bool_fields_carry_the_zero_one_domain() {
        // A `bool` field binds its flattened slot as the i64 0/1 carrier,
        // exactly like a `bool` parameter.
        let source = format!(
            "struct S {{ on: bool, x: i64 }}\n\
             atom pick(s: S) -> i64 requires: true; ensures: result == 0 || result == s.x; body: {{ if s.on {{ s.x }} else {{ 0 }} }};\n{MAIN0}"
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
            [("s.on", "bool"), ("s.x", "i64"), ("result", "i64")]
        );
        assert!(composite_outcomes(&source)
            .iter()
            .all(|result| *result == z3::SatResult::Unsat));
        let wrong = source.replace("result == 0 || result == s.x", "result == s.x");
        assert!(composite_outcomes(&wrong).contains(&z3::SatResult::Sat));
        let context = z3::Context::new(&z3::Config::new());
        let bodies =
            encode_pure_i64_module(&source, &context, true, RefinedAdmission::Composite).unwrap();
        assert_eq!(bodies[0].inputs.len(), 2);
        assert_eq!(bodies[0].param_domain.len(), 1);
        assert!(bodies[0].composite_signature);
        assert!(!bodies[0].refined_signature);
        assert!(!bodies[0].subrange_params);
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
            // subrange and refined-alias fields: the verifier binds a
            // field by its base sort only, without their domains
            "struct P { x: u8 }",
            "struct P { x: i32 }",
            "type Pos = i64 where v > 0;\nstruct P { x: Pos }",
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
            // binders spelled like the synthesized field names the
            // verifier and emitter resolve `p.x` through
            "atom h(p: P, p_x: i64) -> i64 requires: true; ensures: result == p.x; body: { p.x };",
            "atom h(p_x: i64, p: P) -> i64 requires: true; ensures: true; body: { p.y };",
            "atom h(p: P, __struct_p_y: i64) -> i64 requires: true; ensures: true; body: { p.y };",
            "atom h(p: P, p_x: P) -> i64 requires: true; ensures: true; body: { p.y };",
            "atom h(p: P) -> i64 requires: true; ensures: true; body: { let p_x = 1; p.x };",
            "atom h(p: P) -> i64 requires: true; ensures: true; body: { match 0 { p_y => p.y } };",
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
        // The synthesized-name reservation is per struct parameter: a
        // `p_x` binder is fine where no struct parameter `p` exists, and a
        // struct parameter named `q` reserves only `q_x`/`q_y`.
        for atom in [
            "atom h(p_x: i64) -> i64 requires: true; ensures: result == p_x; body: { p_x };",
            "atom h(q: P, p_x: i64) -> i64 requires: true; ensures: result == q.x + p_x; body: { let p_y = q.y; q.x + p_x };",
        ] {
            composite(&format!("{decl}\n{f}\n{atom}\n{MAIN0}")).unwrap();
        }
    }

    #[test]
    fn composite_v3_admits_struct_results_literals_lets_and_call_result_reads() {
        let v3 = |source: &str| {
            extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::CompositeV3)
        };
        let outcomes =
            |source: &str| refined_module_outcomes_admission(source, RefinedAdmission::CompositeV3);
        let decl = "type Pos = i64 where v > 0;\nstruct P { x: Pos, y: i64 }";
        let make = "atom make(a: Pos) -> P requires: true; ensures: result.x == a; body: { P { y: 2, x: a } };";
        let source = format!(
            "{decl}\n{make}\n\
             atom gx(p: P) -> i64 requires: true; ensures: result >= 1; body: {{ p.x }};\n\
             atom usep(a: Pos) -> i64 requires: true; ensures: result == a; body: {{ make(a).x }};\n\
             atom uselet(a: Pos) -> i64 requires: true; ensures: result == a; body: {{ let q = make(a); let r = q; r.x }};\n\
             atom lit(a: i64) -> i64 requires: true; ensures: result == a; body: {{ let q = P {{ x: 5, y: a }}; q.y }};\n\
             atom arg(a: Pos) -> i64 requires: true; ensures: result >= 1; body: {{ gx(make(a)) + gx(P {{ x: a, y: 0 }}) - gx(make(a)) }};\n\
             atom fwd(a: Pos) -> P requires: true; ensures: result.x == a; body: {{ make(a) }};\n\
             atom blk(a: Pos) -> P requires: true; ensures: result.y == a; body: {{ let q = P {{ x: 1, y: a }}; q }};\n{MAIN0}"
        );
        let contracts = v3(&source).unwrap();
        assert!(contracts
            .iter()
            .all(|c| c.profile == PURE_I64_COMPOSITE_PROFILE_V3));
        // A struct result reports one result binder per field and binds
        // the returned layout into its specification identity.
        let make_contract = &contracts[0];
        let results: Vec<_> = make_contract
            .specification
            .binders
            .iter()
            .filter(|b| b.role == "result")
            .map(|b| (b.name.as_str(), b.scalar_type))
            .collect();
        assert_eq!(results, [("result.x", "i64"), ("result.y", "i64")]);
        let renamed = source
            .replace("struct P { x: Pos, y: i64 }", "struct P { x: Pos, z: i64 }")
            .replace("y: 2", "z: 2")
            .replace("y: a", "z: a")
            .replace("q.y", "q.z")
            .replace("y: 0", "z: 0")
            .replace("result.y", "result.z");
        assert_ne!(
            v3(&renamed).unwrap()[0].specification_sha256,
            make_contract.specification_sha256
        );
        // The bounded encoder's token budget is per module, so each shape
        // rechecks beside `make`/`gx` on its own.
        let gx = "atom gx(p: P) -> i64 requires: true; ensures: result >= 1; body: { p.x };";
        let pieces = [
            "atom usep(a: Pos) -> i64 requires: true; ensures: result == a; body: { make(a).x };",
            "atom uselet(a: Pos) -> i64 requires: true; ensures: result == a; body: { let q = make(a); let r = q; r.x };",
            "atom lit(a: i64) -> i64 requires: true; ensures: result == a; body: { let q = P { x: 5, y: a }; q.y };",
            "atom arg(a: Pos) -> i64 requires: true; ensures: result >= 1; body: { gx(P { x: a, y: 0 }) };",
            "atom fwd(a: Pos) -> P requires: true; ensures: result.x == a; body: { make(a) };",
            "atom blk(a: Pos) -> P requires: true; ensures: result.y == a; body: { let q = P { x: 1, y: a }; q };",
        ];
        let module = |piece: &str| format!("{decl}\n{make}\n{gx}\n{piece}\n{MAIN0}");
        for piece in pieces {
            assert!(
                outcomes(&module(piece))
                    .iter()
                    .all(|r| *r == z3::SatResult::Unsat),
                "not rechecked: {piece}"
            );
        }
        // Each obligation is the real one: a wrong returned field, a wrong
        // call-result read, a wrong struct-argument field and an alias
        // field domain violation refute.
        for (piece, from, to) in [
            (pieces[0], "{ P { y: 2, x: a } }", "{ P { y: a, x: 2 } }"),
            (pieces[0], "make(a).x", "make(a).y"),
            (pieces[2], "q.y }", "q.x }"),
            (pieces[3], "gx(P { x: a, y: 0 })", "gx(P { x: 0, y: a })"),
            (pieces[5], "P { x: 1, y: a }", "P { x: 0, y: a }"),
        ] {
            let source = module(piece);
            let broken = source.replace(from, to);
            assert_ne!(broken, source, "probe did not apply: {to}");
            assert!(
                outcomes(&broken).contains(&z3::SatResult::Sat),
                "probe not refuted: {to}"
            );
        }
        // A module using no struct value keeps its v1/v2 name under v3.
        let params_only = format!(
            "{decl}\natom gx(p: P) -> i64 requires: true; ensures: result >= 1; body: {{ p.x }};\n{MAIN0}"
        );
        assert_eq!(
            v3(&params_only).unwrap()[0].profile,
            PURE_I64_COMPOSITE_PROFILE_V2
        );
        let literal_only = format!("struct P {{ x: i64 }}\n{MAIN0}");
        assert_eq!(
            v3(&literal_only).unwrap()[0].profile,
            PURE_I64_COMPOSITE_PROFILE
        );
        // The composite v1/v2 gates keep struct values out.
        assert!(composite(&source).is_err());
        assert!(extract_pure_i64_module_contracts(
            &source,
            "m.mm",
            None,
            RefinedAdmission::CompositeV2
        )
        .is_err());
    }

    #[test]
    fn struct_call_slot_declarations_never_alias_scalar_call_declarations() {
        let context = z3::Context::new(&z3::Config::new());
        assert_ne!(
            call_field_decl(&context, "f", 1, 1).name(),
            call_decl(&context, "f_1_field", 1).name()
        );
        assert_ne!(
            call_field_decl(&context, "f", 1, 1).name(),
            call_field_decl(&context, "f", 1, 0).name()
        );
    }

    #[test]
    fn composite_v3_rejects_struct_values_outside_the_fragment() {
        let v3 = |source: &str| {
            extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::CompositeV3)
        };
        let decl = "struct P { x: i64, y: i64 }\nstruct Q { x: i64, y: i64 }";
        let make = "atom make(a: i64) -> P requires: true; ensures: result.x == a; body: { P { x: a, y: 0 } };";
        let base = format!("{decl}\n{make}\n{MAIN0}");
        v3(&base).unwrap();
        for atom in [
            // struct values in scalar positions
            "atom h(a: i64) -> i64 requires: true; ensures: true; body: { make(a) };",
            "atom h(a: i64) -> i64 requires: true; ensures: true; body: { make(a) + 1 };",
            "atom h(a: i64) -> i64 requires: true; ensures: true; body: { let q = make(a); q };",
            "atom h(a: i64) -> i64 requires: true; ensures: true; body: { P { x: a, y: a } };",
            "atom h(a: i64) -> P requires: true; ensures: result == 0; body: { make(a) };",
            "atom h(a: i64) -> P requires: true; ensures: result.z == 0; body: { make(a) };",
            // a struct result of another struct or a scalar body
            "atom h(a: i64) -> Q requires: true; ensures: true; body: { make(a) };",
            "atom h(a: i64) -> P requires: true; ensures: true; body: { a };",
            // struct literals: undeclared, missing, duplicate, unknown,
            // mistyped and nested fields
            "atom h(a: i64) -> P requires: true; ensures: true; body: { R { x: a } };",
            "atom h(a: i64) -> P requires: true; ensures: true; body: { P { x: a } };",
            "atom h(a: i64) -> P requires: true; ensures: true; body: { P { x: a, x: a } };",
            "atom h(a: i64) -> P requires: true; ensures: true; body: { P { x: a, z: a } };",
            "atom h(a: i64) -> P requires: true; ensures: true; body: { P { x: a, y: a, z: a } };",
            "atom h(a: i64) -> P requires: true; ensures: true; body: { P { x: true, y: a } };",
            "atom h(a: i64) -> P requires: true; ensures: true; body: { P { x: make(a), y: a } };",
            // struct values merged across branches or computed
            "atom h(a: i64) -> P requires: true; ensures: true; body: { if a > 0 { make(a) } else { make(0) } };",
            "atom h(a: i64) -> P requires: true; ensures: true; body: { match a { 0 => make(a), _ => make(1) } };",
            "atom h(a: i64) -> i64 requires: true; ensures: true; body: { if a > 0 { make(a) } else { make(0) }.x };",
            "atom h(a: i64) -> i64 requires: true; ensures: true; body: { P { x: a, y: a }.x };",
            // struct lets are not rebindable and fields are not assignable
            "atom h(a: i64) -> i64 requires: true; ensures: true; body: { let q = make(a); q = make(1); q.x };",
            "atom h(a: i64) -> i64 requires: true; ensures: true; body: { let q = make(a); q.x = 1; q.x };",
            // binders spelled like a struct let's synthesized field names
            "atom h(a: i64) -> i64 requires: true; ensures: true; body: { let q_x = 1; let q = make(a); q.x };",
            "atom h(a: i64) -> i64 requires: true; ensures: true; body: { let q = make(a); let __struct_q_y = 1; q.x };",
            "atom h(q_y: i64) -> i64 requires: true; ensures: true; body: { let q = make(q_y); q.x };",
            // binders named like a declared struct or spelling a struct
            // literal's synthesized field names (`__struct_P_x`)
            "atom h(a: i64) -> i64 requires: true; ensures: true; body: { let P = make(a); let r = P { x: 5, y: 6 }; P.x };",
            "atom h(P: P) -> i64 requires: true; ensures: true; body: { P.x };",
            "atom h(a: i64) -> i64 requires: true; ensures: true; body: { let Q = 1; Q };",
            "atom h(a: i64) -> i64 requires: true; ensures: true; body: { let __struct_P_x = 1; let r = P { x: a, y: 6 }; __struct_P_x };",
            "atom h(__struct_Q_y: i64) -> i64 requires: true; ensures: true; body: { 0 };",
            // a call result passed where the callee takes another struct
            "atom g(q: Q) -> i64 requires: true; ensures: true; body: { q.x };\natom h(a: i64) -> i64 requires: true; ensures: true; body: { g(make(a)) };",
        ] {
            let source = format!("{decl}\n{make}\n{atom}\n{MAIN0}");
            assert!(v3(&source).is_err(), "unexpected acceptance: {atom}");
        }
        // The synthesized-name reservation is per struct let: `p_x` is
        // fine beside a struct let named `q`.
        v3(&format!(
            "{decl}\n{make}\natom h(p_x: i64) -> i64 requires: true; ensures: result == p_x; body: {{ let q = make(p_x); q.x }};\n{MAIN0}"
        ))
        .unwrap();
    }

    #[test]
    fn composite_v2_admits_alias_fields_with_i64_or_bool_leaves() {
        let v2 = |source: &str| {
            extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::CompositeV2)
        };
        let source = format!(
            "type Pos = i64 where v > 0;\nstruct P {{ x: Pos, y: i64 }}\n\
             atom gx(p: P) -> i64 requires: true; ensures: result >= 1; body: {{ p.x }};\n\
             atom fwd(q: P) -> i64 requires: true; ensures: result >= 1; body: {{ gx(q) }};\n{MAIN0}"
        );
        assert!(v2(&source)
            .unwrap()
            .iter()
            .all(|c| c.profile == PURE_I64_COMPOSITE_PROFILE_V2));
        // The v1 composite gate keeps alias fields out.
        assert!(composite(&source).is_err());
        assert!(
            refined_module_outcomes_admission(&source, RefinedAdmission::CompositeV2)
                .iter()
                .all(|r| *r == z3::SatResult::Unsat)
        );
        let tight = source.replace("result >= 1; body: { p.x }", "result >= 2; body: { p.x }");
        assert!(
            refined_module_outcomes_admission(&tight, RefinedAdmission::CompositeV2)
                .contains(&z3::SatResult::Sat)
        );
        // Chains and bool leaves compose; literal-field modules keep v1.
        let chained = format!(
            "type A = i64 where v >= 0;\ntype B = A where v <= 9;\ntype F = bool where v == true;\n\
             struct P {{ x: B, on: F }}\n\
             atom gx(p: P) -> i64 requires: true; ensures: result >= 0 && result <= 9; body: {{ if p.on {{ p.x }} else {{ 0 }} }};\n{MAIN0}"
        );
        assert_eq!(
            v2(&chained).unwrap()[0].profile,
            PURE_I64_COMPOSITE_PROFILE_V2
        );
        assert!(
            refined_module_outcomes_admission(&chained, RefinedAdmission::CompositeV2)
                .iter()
                .all(|r| *r == z3::SatResult::Unsat)
        );
        let plain = format!("struct P {{ x: i64 }}\n{MAIN0}");
        assert_eq!(v2(&plain).unwrap()[0].profile, PURE_I64_COMPOSITE_PROFILE);
        // Subrange leaves, subrange literals and undeclared names stay out.
        for decl in [
            "type U = u8 where v > 0;\nstruct P { x: U }",
            "type S = i32 where v > 0;\nstruct P { x: S }",
            "struct P { x: u8 }",
            "struct P { x: Missing }",
            "type A = B where v > 0;\ntype B = A where v > 0;\nstruct P { x: A }",
            "type U = i64 unit USD;\nstruct P { x: U }",
        ] {
            assert!(
                v2(&format!("{decl}\n{MAIN0}")).is_err(),
                "unexpected acceptance: {decl}"
            );
        }
    }

    #[test]
    fn composite_v4_admits_enum_params_and_variant_match_arms() {
        let v4 = |source: &str| {
            extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::CompositeV4)
        };
        let outcomes =
            |source: &str| refined_module_outcomes_admission(source, RefinedAdmission::CompositeV4);
        let decl = "enum Shape { Point, Circle(r: i64), Rect(w: i64, h: i64) }";
        let source = format!(
            "{decl}\n\
             atom tag_of(s: Shape) -> i64 requires: true; ensures: result >= 0 && result <= 2; body: {{ match s {{ Shape::Circle(r) => 1, Shape::Rect(w, h) => 2, _ => 0 }} }};\n\
             atom read(s: Shape) -> i64 requires: true; ensures: true; body: {{ match s {{ Shape::Circle(r) => r, Shape::Rect(w, h) => w + h, _ => 0 }} }};\n\
             atom pass(s: Shape) -> i64 requires: true; ensures: result == tag_of(s); body: {{ let t = s; tag_of(t) }};\n{MAIN0}"
        );
        let contracts = v4(&source).unwrap();
        assert!(contracts
            .iter()
            .all(|c| c.profile == PURE_I64_COMPOSITE_PROFILE_V4));
        assert_eq!(
            module_source_profile(&source, RefinedAdmission::CompositeV4).unwrap(),
            PURE_I64_COMPOSITE_PROFILE_V4
        );
        // An enum parameter flattens to a tag binder plus one binder per
        // payload slot — never a scalar binder for the value itself.
        let binders: Vec<_> = contracts[0]
            .specification
            .binders
            .iter()
            .map(|b| (b.name.as_str(), b.scalar_type))
            .collect();
        assert_eq!(
            binders,
            [
                ("s.tag", "i64"),
                ("s.0", "i64"),
                ("s.1", "i64"),
                ("result", "i64")
            ]
        );
        assert!(outcomes(&source)
            .iter()
            .all(|result| *result == z3::SatResult::Unsat));
        // A false postcondition over the tag is refuted, not vacuous.
        let wrong = source.replace("result >= 0 && result <= 2", "result >= 0 && result <= 1");
        assert!(outcomes(&wrong).contains(&z3::SatResult::Sat));
        // Narrower admissions keep `enum` out of the vocabulary.
        for admission in [
            RefinedAdmission::Reject,
            RefinedAdmission::V1,
            RefinedAdmission::V2,
            RefinedAdmission::Composite,
            RefinedAdmission::CompositeV2,
            RefinedAdmission::CompositeV3,
        ] {
            assert!(extract_pure_i64_module_contracts(&source, "m.mm", None, admission).is_err());
            assert!(module_source_profile(&source, admission).is_err());
        }
        // Modules declaring no enums keep the narrower profile names.
        let plain = format!("struct P {{ x: i64 }}\n{MAIN0}");
        assert_eq!(v4(&plain).unwrap()[0].profile, PURE_I64_COMPOSITE_PROFILE);
    }

    #[test]
    fn composite_v4_rejects_enum_shapes_outside_the_fragment() {
        let v4 = |source: &str| {
            extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::CompositeV4)
        };
        let decl = "enum Shape { Point, Circle(r: i64), Rect(w: i64, h: i64) }";
        // Literal and catch-all Variable arms on an enum scrutinee are
        // outside the fragment — the tag domain is not a sparse i64 one.
        for bad in [
            format!("{decl}\natom t(s: Shape) -> i64 requires: true; ensures: true; body: {{ match s {{ Shape::Circle(r) => r, 7 => 9, _ => 0 }} }};\n{MAIN0}"),
            format!("{decl}\natom t(s: Shape) -> i64 requires: true; ensures: true; body: {{ match s {{ Shape::Circle(r) => r, other => 0 }} }};\n{MAIN0}"),
            format!("{decl}\natom t(s: Shape) -> i64 requires: true; ensures: true; body: {{ match s {{ Shape::Circle(r) => r, Shape::Rect(w, h) => 0 }} }};\n{MAIN0}"),
            format!("{decl}\natom t(s: Shape) -> i64 requires: true; ensures: true; body: {{ match s {{ Shape::Circle(r, x) => r, _ => 0 }} }};\n{MAIN0}"),
            format!("{decl}\natom t(s: Shape) -> i64 requires: true; ensures: true; body: {{ match s {{ Shape::Square => 1, _ => 0 }} }};\n{MAIN0}"),
            // Enum results, enum construction and aggregate field types
            // stay outside this slice.
            format!("{decl}\natom mk() -> Shape requires: true; ensures: true; body: {{ Shape::Point }};\n{MAIN0}"),
            format!("{decl}\natom mk() -> i64 requires: true; ensures: true; body: {{ let s = Shape::Point; 0 }};\n{MAIN0}"),
            format!("{decl}\natom t(s: Shape) -> i64 requires: true; ensures: true; body: {{ s.r }};\n{MAIN0}"),
            format!("{decl}\natom t(s: Shape) -> Shape requires: true; ensures: true; body: {{ s }};\n{MAIN0}"),
            format!("enum M {{ A(x: i64), B(b: bool, n: i64) }}\natom t(m: M) -> i64 requires: true; ensures: true; body: {{ match m {{ M::A(x) => x, _ => 0 }} }};\n{MAIN0}"),
            format!("enum M {{ A(p: Point) }}\nstruct Point {{ x: i64 }}\natom t(m: M) -> i64 requires: true; ensures: true; body: {{ match m {{ _ => 0 }} }};\n{MAIN0}"),
            format!("enum M {{ A(x: u8) }}\natom t(m: M) -> i64 requires: true; ensures: true; body: {{ match m {{ _ => 0 }} }};\n{MAIN0}"),
        ] {
            assert!(v4(&bad).is_err(), "unexpected acceptance: {bad}");
        }
        // Refined-alias fields ride the same env as struct fields.
        let source = format!(
            "type Port = i64 where v > 0;\nenum P {{ Tcp(p: Port), Idle }}\n\
             atom use(s: P) -> i64 requires: true; ensures: result >= 0; body: {{ match s {{ P::Tcp(p) => p - p + 1, _ => 0 }} }};\n{MAIN0}"
        );
        let contracts = v4(&source).unwrap();
        let binders: Vec<_> = contracts[0]
            .specification
            .binders
            .iter()
            .map(|b| (b.name.as_str(), b.scalar_type))
            .collect();
        assert_eq!(
            binders,
            [("s.tag", "i64"), ("s.0", "i64"), ("result", "i64")]
        );
        assert!(
            refined_module_outcomes_admission(&source, RefinedAdmission::CompositeV4)
                .iter()
                .all(|r| *r == z3::SatResult::Unsat)
        );
    }

    #[test]
    fn composite_v5_admits_struct_field_writes() {
        let v5 = |source: &str| {
            extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::CompositeV5)
        };
        let outcomes =
            |source: &str| refined_module_outcomes_admission(source, RefinedAdmission::CompositeV5);
        let decl = "struct P { x: i64, y: i64 }";
        // `q.f = v` on a parameter, on a `let`-bound literal and on a
        // moved owner, in tail and mid-block positions.
        let source = format!(
            "{decl}\n\
             atom bump(q: P) -> i64 requires: q.x == 0; ensures: result == 1 && q.x == 1; body: {{ q.x = q.x + 1; q.x }};\n\
             atom lit(a: i64) -> i64 requires: true; ensures: result == a + 1; body: {{ let s = P {{ x: a, y: 0 }}; s.x = s.x + 1; s.x }};\n\
             atom mid(q: P) -> i64 requires: true; ensures: result == 2; body: {{ q.y = q.x + 1; q.y + 1 - q.x }};\n{MAIN0}"
        );
        let contracts = v5(&source).unwrap();
        assert!(contracts
            .iter()
            .all(|c| c.profile == PURE_I64_COMPOSITE_PROFILE_V5));
        assert_eq!(
            module_source_profile(&source, RefinedAdmission::CompositeV5).unwrap(),
            PURE_I64_COMPOSITE_PROFILE_V5
        );
        assert!(outcomes(&source).iter().all(|r| *r == z3::SatResult::Unsat));
        // The written field's value feeds the obligation: off-by-one
        // postconditions on the written field and the untouched sibling
        // refute, so the equivalence recheck is the real one.
        let pieces = [
            "atom bump(q: P) -> i64 requires: q.x == 0; ensures: result == 1 && q.x == 1; body: { q.x = q.x + 1; q.x };",
            "atom lit(a: i64) -> i64 requires: true; ensures: result == a + 1; body: { let s = P { x: a, y: 0 }; s.x = s.x + 1; s.x };",
            "atom mid(q: P) -> i64 requires: true; ensures: result == 2; body: { q.y = q.x + 1; q.y + 1 - q.x };",
            "atom keep(q: P) -> i64 requires: true; ensures: result == q.y; body: { q.x = q.x + 1; q.y };",
        ];
        let module = |piece: &str| format!("{decl}\n{piece}\n{MAIN0}");
        for piece in pieces {
            assert!(
                outcomes(&module(piece))
                    .iter()
                    .all(|r| *r == z3::SatResult::Unsat),
                "not rechecked: {piece}"
            );
        }
        for (piece, from, to) in [
            // `q.x == 0` held on the *input* field — `ensures` reads
            // post-write fields, so the stale reading refutes.
            (pieces[0], "q.x == 1", "q.x == 0"),
            (pieces[1], "result == a + 1", "result == a"),
            (pieces[2], "result == 2", "result == 1"),
            (pieces[3], "result == q.y", "result == q.x"),
        ] {
            let source = module(piece);
            let broken = source.replace(from, to);
            assert_ne!(broken, source, "probe did not apply: {to}");
            assert!(
                outcomes(&broken).contains(&z3::SatResult::Sat),
                "probe not refuted: {to}"
            );
        }
        // A module using no field write keeps its older profile name.
        let read_only = format!(
            "{decl}\natom gx(q: P) -> i64 requires: true; ensures: result >= 0; body: {{ q.x }};\n{MAIN0}"
        );
        assert_eq!(
            v5(&read_only).unwrap()[0].profile,
            PURE_I64_COMPOSITE_PROFILE
        );
        // v4 keeps field writes out.
        assert!(extract_pure_i64_module_contracts(
            &source,
            "m.mm",
            None,
            RefinedAdmission::CompositeV4
        )
        .is_err());
    }

    #[test]
    fn composite_v5_rejects_field_writes_outside_the_fragment() {
        let v5 = |source: &str| {
            extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::CompositeV5)
        };
        let context = z3::Context::new(&z3::Config::new());
        let decl = "enum E { A(i64), B }\nstruct P { x: i64, y: i64 }";
        for (piece, why) in [
            // Undeclared field: falls through to the assignable check.
            (
                "atom bad(q: P) -> i64 requires: true; ensures: true; body: { q.z = 1; q.x };",
                "undeclared field write",
            ),
            // Enum binding: `composites` resolves `e` but `structs` does
            // not, so the write stays closed.
            (
                "atom bad(e: E, q: P) -> i64 requires: true; ensures: true; body: { e.x = 1; q.x };",
                "enum field write",
            ),
            // Write inside an `if` branch: the branch-write merge keeps
            // only scalar rebinding, so the composite write fails closed.
            (
                "atom bad(q: P, c: i64) -> i64 requires: true; ensures: true; body: { if c == 1 { q.x = q.x + 1; q.x } else { q.x } };",
                "branch-merged field write",
            ),
            // Write inside a `while`: the loop body's assignable set
            // excludes the escaping base.
            (
                "atom bad(q: P) -> i64 requires: true; ensures: true; body: { let i = 0; while i < 2 invariant: i >= 0 decreases: 2 - i { q.x = q.x + 1; i = i + 1 }; q.x };",
                "loop-carried field write",
            ),
        ] {
            let source = format!("{decl}\n{piece}\n{MAIN0}");
            // The source-layer encoder rejects each shape — a field
            // write inside an `if`/`while` reaches the merge as a
            // composite and fails closed instead of being dropped.
            assert!(
                encode_pure_i64_module_obligations(&source, &context, RefinedAdmission::CompositeV5)
                    .is_err(),
                "encoded: {why}"
            );
        }
        // The first two also fail at contract extraction; a branch or
        // loop write passes the kind layer (its profile name is `v5`)
        // and only the source layer closes it, so extraction stays `Ok`.
        for (piece, kind_closed) in [
            (
                "atom bad(q: P) -> i64 requires: true; ensures: true; body: { q.z = 1; q.x };",
                true,
            ),
            (
                "atom bad(e: E, q: P) -> i64 requires: true; ensures: true; body: { e.x = 1; q.x };",
                true,
            ),
            (
                "atom bad(q: P, c: i64) -> i64 requires: true; ensures: true; body: { if c == 1 { q.x = q.x + 1; q.x } else { q.x } };",
                false,
            ),
            (
                "atom bad(q: P) -> i64 requires: true; ensures: true; body: { let i = 0; while i < 2 invariant: i >= 0 decreases: 2 - i { q.x = q.x + 1; i = i + 1 }; q.x };",
                false,
            ),
        ] {
            let source = format!("{decl}\n{piece}\n{MAIN0}");
            assert_eq!(
                v5(&source).is_err(),
                kind_closed,
                "extraction mismatch"
            );
        }
    }

    #[test]
    fn composite_v6_admits_enum_construction_and_results() {
        let v6 = |source: &str| {
            extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::CompositeV6)
        };
        let outcomes =
            |source: &str| refined_module_outcomes_admission(source, RefinedAdmission::CompositeV6);
        let decl = "enum Shape { Dot, Edge(w: i64), Rect(w: i64, h: i64) }";
        // Enum construction in the tail, a nullary constructor behind a
        // `let`, an enum-returning sibling call, and a `match` on the
        // result value — the whole slice in one module.
        let source = format!(
            "{decl}\n\
             atom make_edge(w: i64) -> Shape requires: true; ensures: match result {{ Shape::Edge(v) => v == w, _ => false }}; body: {{ Shape::Edge(w) }};\n\
             atom make_dot() -> Shape requires: true; ensures: true; body: {{ let s = Shape::Dot; s }};\n\
             atom use_it(w: i64) -> i64 requires: w >= 0; ensures: result >= 0; body: {{ let s = make_edge(w); match s {{ Shape::Edge(v) => v, _ => 0 }} }};\n\
             atom pair(w: i64) -> i64 requires: true; ensures: result == w + 1; body: {{ let s = Shape::Rect(w, 1); match s {{ Shape::Rect(a, b) => a + b, _ => 0 }} }};\n{MAIN0}"
        );
        let contracts = v6(&source).unwrap();
        assert!(contracts
            .iter()
            .all(|c| c.profile == PURE_I64_COMPOSITE_PROFILE_V6));
        assert_eq!(
            module_source_profile(&source, RefinedAdmission::CompositeV6).unwrap(),
            PURE_I64_COMPOSITE_PROFILE_V6
        );
        // An enum result flattens to a tag binder plus one binder per
        // payload slot of the widest variant.
        let binders: Vec<_> = contracts[0]
            .specification
            .binders
            .iter()
            .map(|b| b.name.as_str())
            .collect();
        assert_eq!(binders, ["w", "result.tag", "result.0", "result.1"]);
        assert!(outcomes(&source)
            .iter()
            .all(|result| *result == z3::SatResult::Unsat));
        // A postcondition over the constructed tag that cannot hold is
        // refuted, not vacuous — the ctor pins `result.tag`, the
        // uninterpreted call result is pinned by the callee `ensures`.
        let wrong = source.replace(
            "ensures: match result { Shape::Edge(v) => v == w, _ => false };",
            "ensures: match result { Shape::Dot => true, _ => false };",
        );
        assert!(outcomes(&wrong).contains(&z3::SatResult::Sat));
        // Narrower admissions keep enum values out of the vocabulary.
        for admission in [
            RefinedAdmission::Reject,
            RefinedAdmission::V1,
            RefinedAdmission::V2,
            RefinedAdmission::Composite,
            RefinedAdmission::CompositeV2,
            RefinedAdmission::CompositeV3,
            RefinedAdmission::CompositeV4,
            RefinedAdmission::CompositeV5,
        ] {
            assert!(
                extract_pure_i64_module_contracts(&source, "m.mm", None, admission).is_err(),
                "{admission:?}"
            );
        }
        // A module that declares enums but never constructs or returns
        // them keeps the V4 profile — enum values are opt-in.
        let params_only = format!(
            "{decl}\natom read(s: Shape) -> i64 requires: true; ensures: result >= 0; body: {{ match s {{ Shape::Edge(v) => v, Shape::Rect(a, b) => a + b, _ => 0 }} }};\n{MAIN0}"
        );
        assert_eq!(
            module_source_profile(&params_only, RefinedAdmission::CompositeV6).unwrap(),
            PURE_I64_COMPOSITE_PROFILE_V4
        );
    }

    #[test]
    fn composite_v6_enum_payload_domains_are_tag_conditional() {
        let outcomes =
            |source: &str| refined_module_outcomes_admission(source, RefinedAdmission::CompositeV6);
        // Payload fields carry refined types: `Edge`'s `w` is positive.
        let decl = "type Pos = i64 where v > 0;\n\
                    enum Shape { Dot, Edge(w: Pos) }";
        // A constructor whose payload violates the field's refined
        // domain is refuted — `Shape::Edge(0)` cannot produce a valid
        // `Shape`.
        let bad_ctor = format!(
            "{decl}\natom mk() -> Shape requires: true; ensures: true; body: {{ Shape::Edge(0) }};\n{MAIN0}"
        );
        assert!(outcomes(&bad_ctor).contains(&z3::SatResult::Sat));
        // Under a different tag the same slot is padding: passing a
        // nullary `Shape::Dot` to an `Edge`-carrying signature is valid
        // — the payload domain binds only when the tag selects `Edge`.
        let dot_call = format!(
            "{decl}\n\
             atom read(s: Shape) -> i64 requires: true; ensures: result >= 0; body: {{ match s {{ Shape::Edge(v) => v, _ => 0 }} }};\n\
             atom use_it() -> i64 requires: true; ensures: result >= 0; body: {{ read(Shape::Dot) }};\n{MAIN0}"
        );
        assert!(outcomes(&dot_call)
            .iter()
            .all(|result| *result == z3::SatResult::Unsat));
        // A bound variable of the same name wins over the enum
        // namespace: `Shape.x` on a `Shape` parameter is a field read,
        // never a nullary constructor.
        let shadowed = format!(
            "{decl}\n\
             struct Point {{ x: i64, y: i64 }}\n\
             atom sum(Shape: Point) -> i64 requires: true; ensures: result == Shape.x + Shape.y; body: {{ Shape.x + Shape.y }};\n{MAIN0}"
        );
        assert!(extract_pure_i64_module_contracts(
            &shadowed,
            "m.mm",
            None,
            RefinedAdmission::CompositeV6
        )
        .is_ok());
        assert!(outcomes(&shadowed)
            .iter()
            .all(|result| *result == z3::SatResult::Unsat));
    }

    #[test]
    fn composite_v7_admits_branch_merged_composites() {
        let v7 = |source: &str| {
            extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::CompositeV7)
        };
        let outcomes =
            |source: &str| refined_module_outcomes_admission(source, RefinedAdmission::CompositeV7);
        let decl = "struct P { x: i64, y: i64 }\nenum Shape { Dot, Edge(w: i64) }";
        // `if`/`match` tails producing struct literals, parameters and
        // enum values — the slice's whole merge vocabulary in one
        // module.
        let module = |piece: &str| format!("{decl}\n{piece}\n{MAIN0}");
        for piece in [
            // `if` tails producing a parameter and a struct literal.
            "atom pick(p: P, c: i64) -> P requires: c >= 0 && p.x >= 0; ensures: result.x >= 0; body: { if c == 0 { p } else { P { x: c, y: c } } };",
            // `if` tails producing enum values — tag and payload merge.
            "atom pick_enum(s: Shape, c: i64) -> Shape requires: c >= 0; ensures: true; body: { if c == 0 { s } else { Shape::Edge(c) } };",
            // `match` arms producing struct literals.
            "atom pick_match(a: i64, b: i64, c: i64) -> P requires: a >= 0 && b >= 0; ensures: result.y >= 0; body: { match c { 0 => P { x: a, y: b }, _ => P { x: b, y: a } } };",
            // A field write inside a branch merges the composite.
            "atom merge_field(q: P, c: i64) -> i64 requires: q.x >= 0 && q.x < 100; ensures: result >= 0; body: { if c == 0 { q.x = q.x + 1; q.x } else { q.x } };",
        ] {
            let source = module(piece);
            assert!(
                v7(&source)
                    .unwrap()
                    .iter()
                    .all(|c| c.profile == PURE_I64_COMPOSITE_PROFILE_V7),
                "{piece}"
            );
            assert!(
                outcomes(&source)
                    .iter()
                    .all(|result| *result == z3::SatResult::Unsat),
                "{piece}"
            );
        }
        let source = module(
            "atom pick(p: P, c: i64) -> P requires: c >= 0 && p.x >= 0; ensures: result.x >= 0; body: { if c == 0 { p } else { P { x: c, y: c } } };",
        );
        assert_eq!(
            module_source_profile(&source, RefinedAdmission::CompositeV7).unwrap(),
            PURE_I64_COMPOSITE_PROFILE_V7
        );
        // A merged struct's field conjunct must hold on BOTH tails:
        // the else tail breaks `result.x >= 0`, so the obligation is
        // refuted, not vacuous.
        let wrong = source.replace("else { P { x: c, y: c } }", "else { P { x: 0 - 1, y: c } }");
        assert!(outcomes(&wrong).contains(&z3::SatResult::Sat));
        // Narrower admissions keep merged composites out of the
        // vocabulary.
        for admission in [
            RefinedAdmission::Reject,
            RefinedAdmission::V1,
            RefinedAdmission::V2,
            RefinedAdmission::Composite,
            RefinedAdmission::CompositeV2,
            RefinedAdmission::CompositeV3,
            RefinedAdmission::CompositeV4,
            RefinedAdmission::CompositeV5,
            RefinedAdmission::CompositeV6,
        ] {
            assert!(
                extract_pure_i64_module_contracts(&source, "m.mm", None, admission).is_err(),
                "{admission:?}"
            );
        }
        // A module whose `if` only merges scalars keeps the earlier
        // profile — branch-merged composites are opt-in.
        let scalars_only = format!(
            "struct P {{ x: i64, y: i64 }}\natom read(p: P, c: i64) -> i64 requires: c >= 0; ensures: result >= 0; body: {{ if c == 0 {{ p.x }} else {{ p.y }} }};\n{MAIN0}"
        );
        assert_eq!(
            module_source_profile(&scalars_only, RefinedAdmission::CompositeV7).unwrap(),
            PURE_I64_COMPOSITE_PROFILE
        );
    }

    #[test]
    fn composite_v7_rejects_merged_composites_outside_the_fragment() {
        let context = z3::Context::new(&z3::Config::new());
        let decl = "struct P { x: i64, y: i64 }\nstruct Q { z: i64 }\nenum Shape { Dot, Edge(w: i64) }\nenum M { A(x: i64) }";
        for (piece, why) in [
            // Branches producing different struct types.
            (
                "atom bad(p: P, r: Q, c: i64) -> i64 requires: true; ensures: true; body: { let s = if c == 0 { p } else { r }; s.x };",
                "mismatched struct tails",
            ),
            // A struct tail against a scalar tail.
            (
                "atom bad(p: P, c: i64) -> i64 requires: true; ensures: true; body: { let s = if c == 0 { p } else { 1 }; s.x };",
                "mixed composite/scalar tails",
            ),
            // Enum values merging against a different enum's values.
            (
                "atom bad(s: Shape, m: M, c: i64) -> i64 requires: true; ensures: true; body: { let e = if c == 0 { s } else { m }; match e { Shape::Edge(v) => v, _ => 0 } };",
                "mismatched enum tails",
            ),
            // A `while` body still keeps merged composites out — the
            // loop's merge stays scalar.
            (
                "atom bad(p: P) -> i64 requires: true; ensures: true; body: { let i = 0; while i < 2 invariant: i >= 0 decreases: 2 - i { i = i + 1 }; if i == 2 { p } else { p }.x };",
                "composite result off a loop tail",
            ),
        ] {
            let source = format!("{decl}\n{piece}\n{MAIN0}");
            assert!(
                extract_pure_i64_module_contracts(
                    &source,
                    "m.mm",
                    None,
                    RefinedAdmission::CompositeV7
                )
                .is_err(),
                "admitted: {why}"
            );
            assert!(
                encode_pure_i64_module_obligations(&source, &context, RefinedAdmission::CompositeV7)
                    .is_err(),
                "encoded: {why}"
            );
        }
    }

    #[test]
    fn composite_v6_rejects_enum_values_outside_the_fragment() {
        let v6 = |source: &str| {
            extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::CompositeV6)
        };
        let context = z3::Context::new(&z3::Config::new());
        let decl = "enum Shape { Dot, Edge(w: i64), Rect(w: i64, h: i64) }\nenum M { A(x: i64) }";
        for (piece, why) in [
            // Payload arity: `Edge` takes one field.
            (
                "atom bad() -> Shape requires: true; ensures: true; body: { Shape::Edge(1, 2) };",
                "enum constructor arity",
            ),
            // Payload kind: `w` is `i64`, `true` is `Bool`.
            (
                "atom bad() -> Shape requires: true; ensures: true; body: { Shape::Edge(true) };",
                "payload kind mismatch",
            ),
            // Undeclared variant.
            (
                "atom bad() -> Shape requires: true; ensures: true; body: { Shape::Hexagon };",
                "undeclared variant",
            ),
            // Nullary constructor given payload.
            (
                "atom bad() -> Shape requires: true; ensures: true; body: { Shape::Dot(1) };",
                "nullary constructor with payload",
            ),
            // Result of a different enum: `M::A` is not a `Shape`.
            (
                "atom bad() -> Shape requires: true; ensures: true; body: { M::A(1) };",
                "enum result of a different enum",
            ),
            // Call returning a different enum for a `Shape` result.
            (
                "atom mk() -> M requires: true; ensures: true; body: { M::A(1) };\natom bad() -> Shape requires: true; ensures: true; body: { mk() };",
                "call result of a different enum",
            ),
            // Branch-merged enum values are a later slice.
            (
                "atom bad(c: i64) -> Shape requires: true; ensures: true; body: { if c == 1 { Shape::Dot } else { Shape::Edge(c) } };",
                "branch-merged enum value",
            ),
            // Match arms producing enums merge to a composite result.
            (
                "atom bad(c: i64) -> Shape requires: true; ensures: true; body: { match c { 1 => Shape::Dot, _ => Shape::Edge(c) } };",
                "enum-producing match arms",
            ),
            // Field write on an enum binding stays closed.
            (
                "atom bad(s: Shape) -> i64 requires: true; ensures: true; body: { s.w = 1; 0 };",
                "enum field write",
            ),
        ] {
            let source = format!("{decl}\n{piece}\n{MAIN0}");
            assert!(
                encode_pure_i64_module_obligations(&source, &context, RefinedAdmission::CompositeV6)
                    .is_err(),
                "encoded: {why}"
            );
        }
        // Every shape above except the branch/loop-merge family also
        // fails at contract extraction.
        for piece in [
            "atom bad() -> Shape requires: true; ensures: true; body: { Shape::Edge(1, 2) };",
            "atom bad() -> Shape requires: true; ensures: true; body: { Shape::Edge(true) };",
            "atom bad() -> Shape requires: true; ensures: true; body: { Shape::Hexagon };",
            "atom bad() -> Shape requires: true; ensures: true; body: { Shape::Dot(1) };",
            "atom bad() -> Shape requires: true; ensures: true; body: { M::A(1) };",
            "atom mk() -> M requires: true; ensures: true; body: { M::A(1) };\natom bad() -> Shape requires: true; ensures: true; body: { mk() };",
            "atom bad(s: Shape) -> i64 requires: true; ensures: true; body: { s.w = 1; 0 };",
        ] {
            let source = format!("{decl}\n{piece}\n{MAIN0}");
            assert!(v6(&source).is_err(), "extraction accepted: {piece}");
        }
        // Enum equality (`e == Shape::Dot`, `e.tag`) stays outside the
        // fragment — `match` is the only enum consumer.
        let eq = format!(
            "{decl}\natom bad(s: Shape) -> i64 requires: true; ensures: true; body: {{ if s == Shape::Dot {{ 1 }} else {{ 0 }} }};\n{MAIN0}"
        );
        assert!(v6(&eq).is_err());
    }

    #[test]
    fn composite_v8_admits_nested_struct_fields() {
        let v8 = |source: &str| {
            extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::CompositeV8)
        };
        let outcomes =
            |source: &str| refined_module_outcomes_admission(source, RefinedAdmission::CompositeV8);
        let decl = "struct Inner { a: i64, b: i64 }\nstruct Outer { inner: Inner, c: i64 }";
        let module = |piece: &str| format!("{decl}\n{piece}\n{MAIN0}");
        for piece in [
            // Nested leaf reads in requires/ensures and the body.
            "atom read(o: Outer) -> i64 requires: o.inner.a >= 0; ensures: result >= 0; body: { o.inner.a };",
            // A nested literal init of the inner field.
            "atom make(a: i64, b: i64) -> Outer requires: a >= 0 && b >= 0; ensures: result.inner.b >= 0; body: { Outer { inner: Inner { a: a, b: b }, c: 0 } };",
            // A whole-subtree write `o.inner = w`.
            "atom set(o: Outer, w: Inner) -> Outer requires: w.a >= 0; ensures: result.inner.a == w.a; body: { o.inner = w; o };",
            // A nested field on a call result binding — the fragment
            // keeps call results opaque, so the read only has to
            // resolve, not satisfy a nontrivial bound.
            "atom use_call(o: Outer) -> i64 requires: o.inner.a >= 0; ensures: true; body: { let s = set(o, Inner { a: 1, b: 2 }); s.inner.a + s.inner.b - 3 };\natom set(o: Outer, w: Inner) -> Outer requires: true; ensures: true; body: { o.inner = w; o };",
            // A leaf write on a nested field's sibling still works.
            "atom bump(o: Outer) -> Outer requires: o.c >= 0 && o.c <= 100; ensures: result.c >= 1 && result.inner.a == o.inner.a; body: { o.c = o.c + 1; o };",
        ] {
            let source = module(piece);
            assert!(
                v8(&source)
                    .unwrap()
                    .iter()
                    .all(|c| c.profile == PURE_I64_COMPOSITE_PROFILE_V8),
                "{piece}"
            );
            assert!(
                outcomes(&source)
                    .iter()
                    .all(|result| *result == z3::SatResult::Unsat),
                "{piece}"
            );
        }
        let source = module(
            "atom read(o: Outer) -> i64 requires: o.inner.a >= 0; ensures: result >= 0; body: { o.inner.a };",
        );
        assert_eq!(
            module_source_profile(&source, RefinedAdmission::CompositeV8).unwrap(),
            PURE_I64_COMPOSITE_PROFILE_V8
        );
        // A wrong nested postcondition refutes, not vacuously passes:
        // `o.inner.a` is not bounded below by 1.
        let wrong = source.replace("ensures: result >= 0;", "ensures: result >= 1;");
        assert!(outcomes(&wrong).contains(&z3::SatResult::Sat));
        // Narrower admissions keep nested struct fields out.
        for admission in [
            RefinedAdmission::Reject,
            RefinedAdmission::V1,
            RefinedAdmission::V2,
            RefinedAdmission::Composite,
            RefinedAdmission::CompositeV2,
            RefinedAdmission::CompositeV3,
            RefinedAdmission::CompositeV4,
            RefinedAdmission::CompositeV5,
            RefinedAdmission::CompositeV6,
            RefinedAdmission::CompositeV7,
        ] {
            assert!(
                extract_pure_i64_module_contracts(&source, "m.mm", None, admission).is_err(),
                "{admission:?}"
            );
        }
        // A module without nested fields keeps the earlier profile —
        // v8 is opt-in by shape.
        let flat = format!(
            "struct P {{ x: i64, y: i64 }}\natom read(p: P) -> i64 requires: p.x >= 0; ensures: result >= 0; body: {{ p.x }};\n{MAIN0}"
        );
        assert_eq!(
            module_source_profile(&flat, RefinedAdmission::CompositeV8).unwrap(),
            PURE_I64_COMPOSITE_PROFILE
        );
    }

    #[test]
    fn composite_v9_admits_generic_structs() {
        let v9 = |source: &str| {
            extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::CompositeV9)
        };
        let outcomes =
            |source: &str| refined_module_outcomes_admission(source, RefinedAdmission::CompositeV9);
        let decl = "struct Pair<T> { a: T, b: T }\nstruct Box<T> { inner: T }";
        let module = |piece: &str| format!("{decl}\n{piece}\n{MAIN0}");
        for piece in [
            // An instantiated parameter type and a literal with an
            // explicit `Pair<i64>{…}` instantiation.
            "atom sum(p: Pair<i64>) -> i64 requires: p.a >= 0 && p.b >= 0 && p.a + p.b <= 100; ensures: result == p.a + p.b; body: { p.a + p.b };",
            "atom mk(a: i64) -> Pair<i64> requires: a >= 0; ensures: result.a == a && result.b == a; body: { Pair<i64>{a: a, b: a} };",
        ] {
            let source = module(piece);
            assert!(
                v9(&source)
                    .unwrap()
                    .iter()
                    .all(|c| c.profile == PURE_I64_COMPOSITE_PROFILE_V9),
                "{piece}"
            );
            assert!(
                outcomes(&source)
                    .iter()
                    .all(|result| *result == z3::SatResult::Unsat),
                "{piece}"
            );
        }
        // Admission (without the obligation encoder) covers the wider
        // instantiation shapes — a `bool` leaf, an instantiation nested
        // in another generic's field type, and a refined-alias leaf.
        for piece in [
            "atom flip(p: Pair<bool>) -> Pair<bool> requires: true; ensures: true; body: { p };",
            "atom boxed(p: Pair<i64>) -> Box<Pair<i64>> requires: true; ensures: true; body: { Box<Pair<i64>>{inner: p} };",
            "atom port_pair(p: Pair<Port>) -> Pair<Port> requires: true; ensures: true; body: { p };\ntype Port = i64 where v >= 1 && v <= 65535;",
        ] {
            let source = module(piece);
            assert!(
                v9(&source)
                    .unwrap()
                    .iter()
                    .all(|c| c.profile == PURE_I64_COMPOSITE_PROFILE_V9),
                "{piece}"
            );
        }
        let source = module(
            "atom sum(p: Pair<i64>) -> i64 requires: p.a >= 0 && p.b >= 0 && p.a + p.b <= 100; ensures: result == p.a + p.b; body: { p.a + p.b };",
        );
        assert_eq!(
            module_source_profile(&source, RefinedAdmission::CompositeV9).unwrap(),
            PURE_I64_COMPOSITE_PROFILE_V9
        );
        // A wrong postcondition refutes, not vacuously passes.
        let wrong = source.replace(
            "ensures: result == p.a + p.b;",
            "ensures: result == p.a + p.b + 1;",
        );
        assert!(outcomes(&wrong).contains(&z3::SatResult::Sat));
        // Narrower admissions keep generic declarations out.
        for admission in [
            RefinedAdmission::Reject,
            RefinedAdmission::V1,
            RefinedAdmission::V2,
            RefinedAdmission::Composite,
            RefinedAdmission::CompositeV2,
            RefinedAdmission::CompositeV3,
            RefinedAdmission::CompositeV4,
            RefinedAdmission::CompositeV5,
            RefinedAdmission::CompositeV6,
            RefinedAdmission::CompositeV7,
            RefinedAdmission::CompositeV8,
        ] {
            assert!(
                extract_pure_i64_module_contracts(&source, "m.mm", None, admission).is_err(),
                "{admission:?}"
            );
        }
        // A module without generic declarations keeps the earlier
        // profile — v9 is opt-in by declaration shape.
        let flat = format!(
            "struct P {{ x: i64, y: i64 }}\natom read(p: P) -> i64 requires: p.x >= 0; ensures: result >= 0; body: {{ p.x }};\n{MAIN0}"
        );
        assert_eq!(
            module_source_profile(&flat, RefinedAdmission::CompositeV9).unwrap(),
            PURE_I64_COMPOSITE_PROFILE
        );
    }

    #[test]
    fn composite_v9_bounds_type_argument_explosion() {
        // `L<T> { n: L<(T,T)> }` doubles the type-argument tree every
        // instantiation round — well before the 64-instance count bound
        // the name-length bound must reject it, not run out of memory.
        let items = crate::parser::parse_module(
            "struct L<T> { n: L<(T,T)> }\n             atom f(p: L<i64>) -> i64 requires: true; ensures: true; body: { 0 };",
        );
        let err = monomorphize_generic_structs(items).unwrap_err();
        assert!(
            matches!(err, SemanticContractError::Unsupported(msg) if msg.contains("too large")),
            "{err:?}"
        );
        // Same through the module gate.
        assert!(extract_pure_i64_module_contracts(
            "struct L<T> { n: L<(T,T)> }\n             atom f(p: L<i64>) -> i64 requires: true; ensures: true; body: { 0 };\n             atom main() -> i64 requires: true; ensures: true; body: { 0 };",
            "m.mm",
            None,
            RefinedAdmission::CompositeV9,
        )
        .is_err());
    }

    #[test]
    fn composite_v9_instantiates_only_concrete_nested_arguments() {
        // `Outer<T> { inner: Box<Pair<T>> }`: the `T` inside the generic
        // declaration is unresolved, so no phantom `Box<Pair<T>>`
        // declaration is generated — only the concrete chain.
        let items = crate::parser::parse_module(
            "struct Pair<T> { a: T, b: T }\n             struct Box<T> { inner: T }\n             struct Outer<T> { inner: Box<Pair<T>> }\n             atom f(o: Outer<i64>) -> i64 requires: true; ensures: true; body: { o.inner.inner.a };\n             atom main() -> i64 requires: true; ensures: true; body: { 0 };",
        );
        let mono = monomorphize_generic_structs(items).unwrap();
        let names: Vec<&str> = mono
            .iter()
            .filter_map(|item| match item {
                Item::StructDef(def) => Some(def.name.as_str()),
                _ => None,
            })
            .collect();
        assert!(names.contains(&"Outer<i64>"), "{names:?}");
        assert!(names.contains(&"Box<Pair<i64>>"), "{names:?}");
        assert!(names.contains(&"Pair<i64>"), "{names:?}");
        assert!(
            !names.iter().any(|name| name.contains('T')),
            "no unresolved declaration: {names:?}"
        );
        // A one-letter concrete struct used as a type argument is not a
        // type parameter: `Box<I>` instantiates like `Box<i64>`.
        let items = crate::parser::parse_module(
            "struct I { v: i64 }\n             struct Box<T> { inner: T }\n             atom f(b: Box<I>) -> i64 requires: true; ensures: true; body: { b.inner.v };\n             atom main() -> i64 requires: true; ensures: true; body: { 0 };",
        );
        let mono = monomorphize_generic_structs(items).unwrap();
        let names: Vec<&str> = mono
            .iter()
            .filter_map(|item| match item {
                Item::StructDef(def) => Some(def.name.as_str()),
                _ => None,
            })
            .collect();
        assert!(names.contains(&"Box<I>"), "{names:?}");
    }

    #[test]
    fn composite_v9_multi_param_structs_and_invariant_commas() {
        let v9 = |source: &str| {
            extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::CompositeV9)
        };
        // Commas inside type arguments — `Duo<i64, bool>` — must not
        // split a parameter or a struct-field declaration in two.
        let source = format!(
            "struct Duo<T, U> {{ first: T, second: U }}\n             struct Wrap {{ pair: Duo<i64, bool>, n: i64 }}\n             atom get(p: Duo<i64, bool>) -> i64 requires: p.first >= 0; ensures: result == p.first; body: {{ p.first }};\n             atom flag(w: Wrap) -> bool requires: true; ensures: result == w.pair.second; body: {{ w.pair.second }};\n             {{MAIN0}}"
        );
        let source = source.replace("{MAIN0}", MAIN0);
        assert!(v9(&source).is_ok(), "{}", source);
        // Splitting itself: the comma inside `Duo<i64, bool>` stays in
        // the field, while a comparison inside an `invariant` clause
        // still ends at the following comma — `<` there is an operator,
        // never a bracket.
        let items = crate::parser::parse_module(
            "struct Duo<T, U> { first: T, second: U }\n\
             struct R { lo: i64, hi: i64, invariant: self.lo < self.hi, n: i64, pair: Duo<i64, bool> }\n\
             struct W { x: i64 where v < 10, y: i64 where v <= 20, z: Duo<i64, bool> where v.first >= 0 }\n\
             atom g(p: Duo<i64, bool>, q: i64) -> i64 requires: true; ensures: true; body: { 0 };",
        );
        let r = items
            .iter()
            .find_map(|item| match item {
                Item::StructDef(def) if def.name == "R" => Some(def),
                _ => None,
            })
            .expect("struct R parsed");
        assert_eq!(r.invariants.len(), 1, "{r:?}");
        let field_names: Vec<&str> = r.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(field_names, ["lo", "hi", "n", "pair"], "{r:?}");
        let pair = r
            .fields
            .iter()
            .find(|f| f.name == "pair")
            .expect("pair field");
        assert_eq!(pair.type_name, "Duo<i64, bool>");
        // A `<`/`<=` inside a `where` constraint is a comparison, not a
        // type-argument bracket: the following fields must still split.
        let w = items
            .iter()
            .find_map(|item| match item {
                Item::StructDef(def) if def.name == "W" => Some(def),
                _ => None,
            })
            .expect("struct W parsed");
        let w_names: Vec<&str> = w.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(w_names, ["x", "y", "z"], "{w:?}");
        let g = items
            .iter()
            .find_map(|item| match item {
                Item::Atom(atom) if atom.name == "g" => Some(atom),
                _ => None,
            })
            .expect("atom g parsed");
        assert_eq!(g.params.len(), 2, "{g:?}");
        assert_eq!(g.params[0].type_name.as_deref(), Some("Duo<i64, bool>"));
    }

    #[test]
    fn composite_v9_rejects_generic_shapes_outside_the_fragment() {
        let v9 = |source: &str| {
            extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::CompositeV9)
        };
        // Generic atoms and enums stay closed: instantiation models
        // struct fields only, never a contract or a variant.
        assert!(v9(&format!(
            "atom id<T>(x: T) -> T requires: true; ensures: true; body: {{ x }};\n{MAIN0}"
        ))
        .is_err());
        assert!(v9(&format!(
            "enum Opt<T> {{ Some(T), None }}\natom f() -> i64 requires: true; ensures: true; body: {{ 0 }};\n{MAIN0}"
        ))
        .is_err());
        // An enum type argument is no admitted leaf kind.
        assert!(v9(&format!(
            "enum Rgb {{ R, G, B }}\nstruct Pair<T> {{ a: T, b: T }}\natom f(p: Pair<Rgb>) -> i64 requires: true; ensures: true; body: {{ 0 }};\n{MAIN0}"
        ))
        .is_err());
        // Neither is a float type argument.
        assert!(v9(&format!(
            "struct Pair<T> {{ a: T, b: T }}\natom f(p: Pair<f64>) -> i64 requires: true; ensures: true; body: {{ 0 }};\n{MAIN0}"
        ))
        .is_err());
        // An instance with no generic declaration stays unresolved.
        assert!(v9(&format!(
            "struct Pair<T> {{ a: T, b: T }}\natom f(p: Triple<i64>) -> i64 requires: true; ensures: true; body: {{ 0 }};\n{MAIN0}"
        ))
        .is_err());
        // Two generic declarations of one name are one declaration too
        // many.
        assert!(v9(&format!(
            "struct Pair<T> {{ a: T }}\nstruct Pair<T> {{ a: T }}\natom f() -> i64 requires: true; ensures: true; body: {{ 0 }};\n{MAIN0}"
        ))
        .is_err());
        // Self-expanding instantiation `L<T> { n: L<L<T>> }` hits the
        // instance bound instead of unfolding forever.
        assert!(v9(&format!(
            "struct L<T> {{ n: L<L<T>> }}\natom f(p: L<i64>) -> i64 requires: true; ensures: true; body: {{ 0 }};\n{MAIN0}"
        ))
        .is_err());
        // A literal naming an instance that was never used stays
        // unknown — `Box<i64>` is instantiated here on use, so this
        // one is fine only when declared… no: `Other<i64>` has no
        // generic declaration at all.
        assert!(v9(&format!(
            "struct Pair<T> {{ a: T, b: T }}\natom f() -> i64 requires: true; ensures: true; body: {{ let x = Other<i64>{{a: 1, b: 2}}; 0 }};\n{MAIN0}"
        ))
        .is_err());
    }

    #[test]
    fn composite_v10_admits_u64_leaves() {
        let v10 = |source: &str| {
            extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::CompositeV10)
        };
        let outcomes = |source: &str| {
            refined_module_outcomes_admission(source, RefinedAdmission::CompositeV10)
        };
        let module = |piece: &str| format!("{piece}\n{MAIN0}");
        for piece in [
            // A `u64` parameter: `a >= 0` is trivially true under the
            // unsigned reading, and `a - 1` wraps to u64::MAX at `a == 0`.
            "atom ge(a: u64) -> i64 requires: a >= 0; ensures: result == 1; body: { 1 };",
            "atom wrap(a: u64) -> u64 requires: a == 0; ensures: result == 0 - 1; body: { a - 1 };",
            "atom half(a: u64) -> u64 requires: a % 2 == 0; ensures: result * 2 == a; body: { a / 2 };",
            // A `u64` field inside a struct and a `u64` result.
            "struct B { t: u64 }
atom add(b: B, e: u64) -> B requires: true; ensures: result.t == b.t + e; body: { B{t: b.t + e} };",
            // Literals adapt: `e` declares u64 so `e < 10` is unsigned.
            "atom lt(e: u64) -> i64 requires: e < 10; ensures: result == 0; body: { 0 };",
        ] {
            let source = module(piece);
            match v10(&source) {
                Ok(cs) => assert!(
                    cs.iter()
                        .all(|c| c.profile == PURE_I64_COMPOSITE_PROFILE_V10),
                    "{piece}"
                ),
                Err(e) => panic!("{piece}: {e:?}"),
            }
            match std::panic::catch_unwind(|| outcomes(&source)) {
                Ok(res) => assert!(
                    res.iter().all(|r| *r == z3::SatResult::Unsat),
                    "{piece}"
                ),
                Err(_) => panic!("outcomes failed for: {piece}"),
            }
        }
        // v10 pins the profile: a u64 leaf anywhere in the module is what
        // selects it.
        let source =
            module("atom ge(a: u64) -> i64 requires: a >= 0; ensures: result == 1; body: { 1 };");
        assert_eq!(
            module_source_profile(&source, RefinedAdmission::CompositeV10).unwrap(),
            PURE_I64_COMPOSITE_PROFILE_V10
        );
        // `a < 0` is unsatisfiable for a u64 — the verifier's vacuity
        // phase rejects the atom rather than passing it vacuously; the
        // fragment obligation itself stays the honest `requires⇒ensures`
        // encoding, so what this layer proves is that `a < 0` reads
        // unsigned: `a bvult 0` is refutable, which the e2e negative
        // fixture turns into a rejection.
        // A wrong postcondition over u64 arithmetic refutes.
        let wrong = module(
            "atom bad(a: u64, b: u64) -> u64 requires: a == 1 && b == 1; ensures: result == 3; body: { a + b };",
        );
        assert!(outcomes(&wrong).contains(&z3::SatResult::Sat));
        // Narrower admissions keep u64 leaves out.
        for admission in [
            RefinedAdmission::Reject,
            RefinedAdmission::V1,
            RefinedAdmission::V2,
            RefinedAdmission::Composite,
            RefinedAdmission::CompositeV2,
            RefinedAdmission::CompositeV5,
            RefinedAdmission::CompositeV8,
            RefinedAdmission::CompositeV9,
        ] {
            assert!(
                extract_pure_i64_module_contracts(&source, "m.mm", None, admission).is_err(),
                "{admission:?}"
            );
        }
        // A module without u64 keeps the earlier profile.
        let flat = format!(
            "atom ge(a: i64) -> i64 requires: a >= 0; ensures: result == 1; body: {{ 1 }};\n{MAIN0}"
        );
        assert_ne!(
            module_source_profile(&flat, RefinedAdmission::CompositeV10).unwrap(),
            PURE_I64_COMPOSITE_PROFILE_V10
        );
    }

    #[test]
    fn composite_v10_admits_usize_isize_leaves() {
        let v10 = |source: &str| {
            extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::CompositeV10)
        };
        let outcomes = |source: &str| {
            refined_module_outcomes_admission(source, RefinedAdmission::CompositeV10)
        };
        let module = |piece: &str| format!("{piece}\n{MAIN0}");
        for piece in [
            // `usize` folds to `u64`: unsigned comparison and wrap.
            "atom ge(a: usize) -> i64 requires: a >= 0; ensures: result == 1; body: { 1 };",
            "atom wrap(a: usize) -> usize requires: a == 0; ensures: result == 0 - 1; body: { a - 1 };",
            "atom half(a: usize) -> usize requires: a % 2 == 0; ensures: result * 2 == a; body: { a / 2 };",
            // `isize` folds to `i64`: a negative bound is admissible.
            "atom neg(a: isize) -> isize requires: a < 0; ensures: result == a + 1; body: { a + 1 };",
            // Width-typed leaves inside a struct and an enum payload.
            "struct B { t: usize, s: isize }
atom add(b: B, e: usize) -> B requires: true; ensures: result.t == b.t + e; body: { B{t: b.t + e, s: b.s} };",
            // usize and u64 payload slots share the U64 kind.
            "enum E { A(usize), B(u64) }
atom mk(x: usize) -> E requires: true; ensures: true; body: { E::A(x) };",
        ] {
            let source = module(piece);
            match v10(&source) {
                Ok(cs) => assert!(
                    cs.iter()
                        .all(|c| c.profile == PURE_I64_COMPOSITE_PROFILE_V10),
                    "{piece}"
                ),
                Err(e) => panic!("{piece}: {e:?}"),
            }
            match std::panic::catch_unwind(|| outcomes(&source)) {
                Ok(res) => assert!(
                    res.iter().all(|r| *r == z3::SatResult::Unsat),
                    "{piece}"
                ),
                Err(_) => panic!("outcomes failed for: {piece}"),
            }
        }
        // A `usize` or `isize` leaf pins the v10 profile like `u64`.
        for ty in ["usize", "isize"] {
            let source = module(&format!(
                "atom ge(a: {ty}) -> i64 requires: a >= 0; ensures: result == 1; body: {{ 1 }};"
            ));
            assert_eq!(
                module_source_profile(&source, RefinedAdmission::CompositeV10).unwrap(),
                PURE_I64_COMPOSITE_PROFILE_V10,
                "{ty}"
            );
        }
        // Wrong `isize` arithmetic refutes under the signed reading.
        let wrong = module(
            "atom bad(a: isize) -> isize requires: a == 1; ensures: result == 3; body: { a + 1 };",
        );
        assert!(outcomes(&wrong).contains(&z3::SatResult::Sat));
        // Kind-uniform but domain-distinct payloads stay out: a refined
        // alias carries a domain conjunct its bare base lacks, so
        // `A(Pos)`/`B(i64)` has no uniform slot domain even though both
        // resolve to `I64`.
        for piece in [
            "type Pos = i64 where v > 0;
enum E { A(Pos), B(i64) }
atom mk(x: Pos) -> E requires: true; ensures: true; body: { E::A(x) };",
            "type Pos = i64 where v > 0;
enum E { A(i64), B(Pos) }
atom mk(x: i64) -> E requires: true; ensures: true; body: { E::A(x) };",
        ] {
            let source = module(piece);
            assert!(v10(&source).is_err(), "{piece}");
        }
        // Narrower admissions keep the width-typed leaves out.
        let source =
            module("atom ge(a: usize) -> i64 requires: a >= 0; ensures: result == 1; body: { 1 };");
        for admission in [
            RefinedAdmission::Reject,
            RefinedAdmission::V1,
            RefinedAdmission::CompositeV2,
            RefinedAdmission::CompositeV9,
        ] {
            assert!(
                extract_pure_i64_module_contracts(&source, "m.mm", None, admission).is_err(),
                "{admission:?}"
            );
        }
    }

    #[test]
    fn composite_v12_admits_f64_leaves() {
        let v12 = |source: &str| {
            extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::CompositeV12)
        };
        let outcomes = |source: &str| {
            refined_module_outcomes_admission(source, RefinedAdmission::CompositeV12)
        };
        let module = |piece: &str| format!("{piece}\n{MAIN0}");
        for piece in [
            // Param → result copy: the bit pattern flows opaquely.
            "atom copy(a: f64) -> f64 requires: true; ensures: true; body: { a };",
            // A call result stays a pass-through leaf.
            "atom copy(a: f64) -> f64 requires: true; ensures: true; body: { a };\natom dup(x: f64) -> f64 requires: true; ensures: true; body: { copy(x) };",
            // A `let` rebinding keeps the slot.
            "atom relay(x: f64) -> f64 requires: true; ensures: true; body: { let y = x; y };",
            // Branch merge selects the slot like any leaf.
            "atom pick(f: bool, x: f64, y: f64) -> f64 requires: true; ensures: true; body: { if f { x } else { y } };",
            // A struct field and its read-back.
            "struct B { v: f64, t: i64 }\natom wrap(x: f64, t: i64) -> B requires: t >= 0; ensures: result.t == t; body: { B{v: x, t: t} };\natom unwrap(b: B) -> f64 requires: b.t >= 0; ensures: true; body: { b.v };",
            // An enum payload slot and its match binding.
            "enum R { F(f64), N }\natom as_r(x: f64) -> R requires: true; ensures: true; body: { R::F(x) };\natom val(r: R, fb: f64) -> f64 requires: true; ensures: true; body: { match r { R::F(v) => v, _ => fb } };",
        ] {
            let source = module(piece);
            match v12(&source) {
                Ok(cs) => assert!(
                    cs.iter()
                        .all(|c| c.profile == PURE_I64_COMPOSITE_PROFILE_V11),
                    "{piece}"
                ),
                Err(e) => panic!("{piece}: {e:?}"),
            }
            match std::panic::catch_unwind(|| outcomes(&source)) {
                Ok(res) => assert!(
                    res.iter().all(|r| *r == z3::SatResult::Unsat),
                    "{piece}"
                ),
                Err(_) => panic!("outcomes failed for: {piece}"),
            }
        }
        // v12 pins the profile: an f64 leaf anywhere selects it.
        let source = module("atom copy(a: f64) -> f64 requires: true; ensures: true; body: { a };");
        assert_eq!(
            module_source_profile(&source, RefinedAdmission::CompositeV12).unwrap(),
            PURE_I64_COMPOSITE_PROFILE_V11
        );
        // Narrower admissions keep f64 leaves out — v11 (bounded
        // arrays) included.
        for admission in [
            RefinedAdmission::Reject,
            RefinedAdmission::V1,
            RefinedAdmission::V2,
            RefinedAdmission::Composite,
            RefinedAdmission::CompositeV2,
            RefinedAdmission::CompositeV5,
            RefinedAdmission::CompositeV8,
            RefinedAdmission::CompositeV9,
            RefinedAdmission::CompositeV10,
            RefinedAdmission::CompositeV11,
        ] {
            assert!(
                extract_pure_i64_module_contracts(&source, "m.mm", None, admission).is_err(),
                "{admission:?}"
            );
        }
        // A module without f64 keeps the earlier profile.
        let flat = format!(
            "atom ge(a: i64) -> i64 requires: a >= 0; ensures: result == 1; body: {{ 1 }};\n{MAIN0}"
        );
        assert_ne!(
            module_source_profile(&flat, RefinedAdmission::CompositeV12).unwrap(),
            PURE_I64_COMPOSITE_PROFILE_V11
        );
    }

    #[test]
    fn composite_v12_rejects_f64_reads() {
        let v12 = |source: &str| {
            extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::CompositeV12)
        };
        for (piece, why) in [
            // Comparison reads the bit pattern.
            (
                "atom c(a: f64) -> i64 requires: true; ensures: result == 0; body: { if a >= a { 0 } else { 1 } };",
                "f64 comparison",
            ),
            // Equality reads the bit pattern.
            (
                "atom c(a: f64) -> i64 requires: true; ensures: result == 0; body: { if a == a { 0 } else { 1 } };",
                "f64 equality",
            ),
            // Arithmetic reads the bit pattern.
            (
                "atom c(a: f64) -> f64 requires: true; ensures: true; body: { a + a };",
                "f64 arithmetic",
            ),
            // A declared f64 never mixes with a declared i64 operand.
            (
                "atom c(a: f64, b: i64) -> i64 requires: true; ensures: result == 0; body: { if a >= b { 0 } else { 1 } };",
                "f64/i64 mix",
            ),
            // A requires clause may not read it either.
            (
                "atom c(a: f64) -> f64 requires: a == a; ensures: true; body: { a };",
                "f64 requires read",
            ),
            // No numeric literal adapts to f64 — `Num` only adapts to
            // integer kinds, so a literal body under an f64 signature
            // stays out.
            (
                "atom c(a: f64) -> f64 requires: true; ensures: true; body: { 0 };",
                "f64 literal",
            ),
        ] {
            assert!(
                v12(&format!("{piece}\n{MAIN0}")).is_err(),
                "admitted: {why}"
            );
        }
    }

    #[test]
    fn composite_v13_admits_str_leaves() {
        let v13 = |source: &str| {
            extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::CompositeV13)
        };
        let outcomes = |source: &str| {
            refined_module_outcomes_admission(source, RefinedAdmission::CompositeV13)
        };
        let module = |piece: &str| format!("{piece}\n{MAIN0}");
        for piece in [
            // Byte read: "hi"[0] is 104.
            "atom fst() -> u64 requires: true; ensures: result == 104; body: { let s = \"hi\"; s[0] };",
            // `len` is the literal's byte count (an `i64`, like an
            // array's length).
            "atom n() -> i64 requires: true; ensures: result == 2; body: { let s = \"hi\"; len(s) };",
            // An out-of-bounds read yields 0, like an array's merged phi.
            "atom oob() -> u64 requires: true; ensures: result == 0; body: { let s = \"hi\"; s[9] };",
            "atom neg() -> u64 requires: true; ensures: result == 0; body: { let s = \"hi\"; s[0 - 1] };",
            // Bytes combine in u64 arithmetic.
            "atom sum() -> u64 requires: true; ensures: result == 195; body: { let s = \"ab\"; s[0] + s[1] };",
            // An index expression bounds the read.
            "atom idx(i: i64) -> u64 requires: i >= 0 && i < 2; ensures: result >= 97 && result <= 98; body: { let s = \"ab\"; s[i] };",
            // An alias binding reads the same cells.
            "atom alias() -> u64 requires: true; ensures: result == 104; body: { let s = \"hi\"; let t = s; t[0] };",
            // An empty literal reads out-of-bounds everywhere.
            "atom empty() -> u64 requires: true; ensures: result == 0; body: { let s = \"\"; s[0] };",
            "atom empty_len() -> i64 requires: true; ensures: result == 0; body: { let s = \"\"; len(s) };",
            // A non-ASCII literal is bytes (UTF-8): "é"[0] = 0xC3 = 195.
            "atom utf8() -> u64 requires: true; ensures: result == 195; body: { let s = \"é\"; s[0] };",
            // A branch-local literal reads inside its arm.
            "atom branch(c: i64) -> u64 requires: c >= 0; ensures: result == 104; body: { if c == 0 { let s = \"hi\"; s[0] } else { 104 } };",
        ] {
            let source = module(piece);
            match v13(&source) {
                Ok(cs) => assert!(
                    cs.iter()
                        .all(|c| c.profile == PURE_I64_COMPOSITE_PROFILE_V13),
                    "{piece}"
                ),
                Err(e) => panic!("{piece}: {e:?}"),
            }
            match std::panic::catch_unwind(|| outcomes(&source)) {
                Ok(res) => assert!(
                    res.iter().all(|r| *r == z3::SatResult::Unsat),
                    "{piece}"
                ),
                Err(_) => panic!("outcomes failed for: {piece}"),
            }
        }
        // A false byte claim refutes.
        let wrong = module(
            "atom fst() -> u64 requires: true; ensures: result == 105; body: { let s = \"hi\"; s[0] };",
        );
        match outcomes(&wrong).as_slice() {
            [z3::SatResult::Sat, ..] | [.., z3::SatResult::Sat] => (),
            res => panic!("expected a refuted postcondition, got {res:?}"),
        }
        // v13 pins the profile: a `Str` literal anywhere selects it.
        let source = module(
            "atom fst() -> u64 requires: true; ensures: result == 104; body: { let s = \"hi\"; s[0] };",
        );
        assert_eq!(
            module_source_profile(&source, RefinedAdmission::CompositeV13).unwrap(),
            PURE_I64_COMPOSITE_PROFILE_V13
        );
        // Narrower admissions keep `Str` literals out — v12 (f64
        // leaves) and the bounded-memory profile included.
        for admission in [
            RefinedAdmission::Reject,
            RefinedAdmission::V1,
            RefinedAdmission::V2,
            RefinedAdmission::Composite,
            RefinedAdmission::CompositeV5,
            RefinedAdmission::CompositeV10,
            RefinedAdmission::CompositeV11,
            RefinedAdmission::CompositeV12,
        ] {
            assert!(
                extract_pure_i64_module_contracts(&source, "m.mm", None, admission).is_err(),
                "{admission:?}"
            );
        }
        // A module without `Str` keeps the earlier profile.
        let flat = format!(
            "atom ge(a: i64) -> i64 requires: a >= 0; ensures: result == 1; body: {{ 1 }};\n{MAIN0}"
        );
        assert_ne!(
            module_source_profile(&flat, RefinedAdmission::CompositeV13).unwrap(),
            PURE_I64_COMPOSITE_PROFILE_V13
        );
    }

    #[test]
    fn composite_v13_rejects_str_shapes_outside_the_fragment() {
        let v13 = |source: &str| {
            extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::CompositeV13)
        };
        for (piece, why) in [
            // Indexed write: a `Str` backing is immutable.
            (
                "atom w() -> i64 requires: true; ensures: true; body: { let s = \"hi\"; s[0] = 97; s[0] };",
                "str indexed write",
            ),
            // Reassignment: the binding cannot re-seat a literal.
            (
                "atom r() -> i64 requires: true; ensures: true; body: { let s = \"hi\"; s = \"yo\"; s[0] };",
                "str reassignment",
            ),
            // The bound itself: past 64 bytes stays out.
            (
                "atom big() -> i64 requires: true; ensures: true; body: { let s = \"01234567890123456789012345678901234567890123456789012345678901234\"; s[0] };",
                "str bound",
            ),
            // Scalar position: a `Str` is not an integer operand.
            (
                "atom arith() -> i64 requires: true; ensures: true; body: { let s = \"hi\"; s + 1 };",
                "str arithmetic",
            ),
            // Equality between two `Str` values reads no defined slot.
            (
                "atom eq() -> bool requires: true; ensures: true; body: { let s = \"hi\"; s == s };",
                "str equality",
            ),
            // An `if`-expression value cannot merge `Str` tails yet.
            (
                "atom m(c: i64) -> i64 requires: true; ensures: true; body: { let s = if c == 0 { \"a\" } else { \"b\" }; s[0] };",
                "str branch merge",
            ),
            // Returning the `Str` itself mismatches an i64 signature.
            (
                "atom ret() -> i64 requires: true; ensures: true; body: { let s = \"hi\"; s };",
                "str return",
            ),
            // No `str` parameter: only literal-borne `Str` bindings.
            (
                "atom p(s: str) -> i64 requires: true; ensures: true; body: { s[0] };",
                "str param",
            ),
            // No `str` struct field either.
            (
                "struct B { s: str, n: i64 }\natom f(b: B) -> i64 requires: true; ensures: true; body: { b.n };",
                "str struct field",
            ),
            // No `str` enum payload.
            (
                "enum E { S(str), N }\natom f(e: E) -> i64 requires: true; ensures: true; body: { 0 };",
                "str enum payload",
            ),
        ] {
            assert!(
                v13(&format!("{piece}\n{MAIN0}")).is_err(),
                "admitted: {why}"
            );
        }
    }

    #[test]
    fn composite_v10_rejects_u64_shapes_outside_the_fragment() {
        let v10 = |source: &str| {
            extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::CompositeV10)
        };
        // A declared u64 operand must not mix with a declared non-u64
        // operand — the fragment stays closed on signed/unsigned mixes.
        for piece in [
            "atom mix(a: u64, b: i64) -> i64 requires: true; ensures: result == 0; body: { if a < b { 0 } else { 1 } };",
            "atom mix_ret(a: u64, b: i64) -> i64 requires: true; ensures: result == 0; body: { a + b };",
        ] {
            assert!(v10(&format!("{piece}\n{MAIN0}")).is_err(), "{piece}");
        }
        // `usize`/`isize` fold to `u64`/`i64` under the same admission,
        // so the operand-mix rule applies to a `usize`/`i64` mix too;
        // only widths with no 64-bit fold stay closed.
        for piece in [
            "atom f(a: i128) -> i64 requires: true; ensures: true; body: { 0 };",
            "atom mix(a: usize, b: i64) -> i64 requires: true; ensures: result == 0; body: { if a < b { 0 } else { 1 } };",
        ] {
            assert!(v10(&format!("{piece}\n{MAIN0}")).is_err(), "{piece}");
        }
        // Same-kind mixes stay open: `usize`/`u64` and `isize`/`i64`.
        for piece in [
            "atom ok(a: usize, b: u64) -> i64 requires: true; ensures: result == 0; body: { if a < b { 0 } else { 1 } };",
            "atom ok(a: isize, b: i64) -> i64 requires: true; ensures: result == 0; body: { a + b };",
        ] {
            assert!(v10(&format!("{piece}\n{MAIN0}")).is_ok(), "{piece}");
        }
    }

    #[test]
    fn composite_v8_rejects_nested_shapes_outside_the_fragment() {
        let context = z3::Context::new(&z3::Config::new());
        let decl = "struct Inner { a: i64, b: i64 }\nstruct Outer { inner: Inner, c: i64 }\nstruct Other { x: i64, y: i64 }";
        for (piece, why) in [
            // A leaf write through a nested path `o.inner.a = v` —
            // writes land on the top-level binding, not deeper paths.
            (
                "atom bad(o: Outer) -> i64 requires: true; ensures: true; body: { o.inner.a = 1; o.inner.a };",
                "nested-path leaf write",
            ),
            // A subtree write against a different struct type.
            (
                "atom bad(o: Outer, x: Other) -> i64 requires: true; ensures: true; body: { o.inner = x; 0 };",
                "mismatched subtree write",
            ),
            // A scalar where the field declares a struct.
            (
                "atom bad(o: Outer) -> i64 requires: true; ensures: true; body: { o.inner = 5; 0 };",
                "scalar subtree write",
            ),
            // A nested read through a name that is not a struct field.
            (
                "atom bad(o: Outer) -> i64 requires: true; ensures: true; body: { o.inner.zz };",
                "undeclared nested field",
            ),
        ] {
            let source = format!("{decl}\n{piece}\n{MAIN0}");
            assert!(
                encode_pure_i64_module_obligations(&source, &context, RefinedAdmission::CompositeV8)
                    .is_err(),
                "encoded: {why}"
            );
            assert!(
                extract_pure_i64_module_contracts(
                    &source,
                    "m.mm",
                    None,
                    RefinedAdmission::CompositeV8
                )
                .is_err(),
                "extraction accepted: {why}"
            );
        }
        // Structural bounds: a declaration deeper than 8 nesting
        // levels or wider than 16 leaves fails closed, and a cyclic
        // declaration cannot be flattened at all.
        let deep = (0..10)
            .map(|i| format!("struct D{i} {{ next: D{}, x: i64 }}", i + 1))
            .chain(std::iter::once("struct D10 { x: i64 }".to_string()))
            .collect::<Vec<_>>()
            .join("\n");
        for source in [
            format!("{deep}\natom bad(d: D0) -> i64 requires: true; ensures: true; body: {{ d.x }};\n{MAIN0}"),
            format!(
                "struct Wide {{ {} }}\natom bad(w: Wide) -> i64 requires: true; ensures: true; body: {{ w.f0 }};\n{MAIN0}",
                (0..17).map(|i| format!("f{i}: i64")).collect::<Vec<_>>().join(", ")
            ),
            format!(
                "struct C {{ next: C }}\natom bad(c: C) -> i64 requires: true; ensures: true; body: {{ 0 }};\n{MAIN0}"
            ),
        ] {
            assert!(
                extract_pure_i64_module_contracts(
                    &source,
                    "m.mm",
                    None,
                    RefinedAdmission::CompositeV8
                )
                .is_err(),
                "accepted bounded-shape violation"
            );
        }
    }

    fn composite_v18_outcomes(source: &str) -> Vec<z3::SatResult> {
        refined_module_outcomes_admission(source, RefinedAdmission::CompositeV18)
    }

    fn composite_v18_outcomes_or(source: &str, label: &str) -> Vec<z3::SatResult> {
        let mut config = z3::Config::new();
        config.set_timeout_msec(crate::verification::VerificationConfig::default().timeout_ms);
        let context = z3::Context::new(&config);
        let solver = z3::Solver::new(&context);
        let atoms =
            encode_pure_i64_module_obligations(source, &context, RefinedAdmission::CompositeV18)
                .unwrap_or_else(|e| panic!("{label}: {e:?}"));
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

    const COMPOSITE_PAYLOAD_DECLS: &str =
        "struct Point { x: i64, y: i64 }\nenum Event { Ping, Move(pos: Point) }\n";

    #[test]
    fn composite_enum_payloads_verify() {
        // Match binds the composite payload as a struct — `p.x`/`p.y`
        // read its leaves.
        assert!(composite_v18_outcomes_or(&format!(
            "{COMPOSITE_PAYLOAD_DECLS}atom step(e: Event) -> i64 requires: true; ensures: result >= 0; body: {{ match e {{ Event::Move(p) => if p.x + p.y >= 0 {{ p.x + p.y }} else {{ 0 }}, _ => 0 }} }};\n{MAIN0}"
        ), "v18-case1")
        .iter()
        .all(|result| *result == z3::SatResult::Unsat));
        // A struct literal ctor arg stores the flattened leaves.
        assert!(composite_v18_outcomes_or(&format!(
            "{COMPOSITE_PAYLOAD_DECLS}atom bump(x: i64) -> i64 requires: x >= 0 && x <= 100; ensures: result >= 1; body: {{ let e = Event::Move(Point {{ x: x, y: 1 }}); match e {{ Event::Move(p) => p.x + p.y, _ => 0 }} }};\n{MAIN0}"
        ), "v18-case2")
        .iter()
        .all(|result| *result == z3::SatResult::Unsat));
        // A bound composite ctor arg keeps its value.
        assert!(composite_v18_outcomes_or(&format!(
            "{COMPOSITE_PAYLOAD_DECLS}atom relay(p: Point) -> i64 requires: p.x >= 0; ensures: result >= 0; body: {{ let e = Event::Move(p); match e {{ Event::Move(q) => q.x, _ => 0 }} }};\n{MAIN0}"
        ), "v18-case3")
        .iter()
        .all(|result| *result == z3::SatResult::Unsat));
        // A ctor result feeds a match in the same atom. Enum-typed
        // atom results with composite payloads are a follow-on slice —
        // binding the multi-slot call return hits the scalar lowering.
        assert!(composite_v18_outcomes_or(&format!(
            "{COMPOSITE_PAYLOAD_DECLS}atom mk(x: i64) -> i64 requires: x >= 0 && x <= 100; ensures: result >= 0; body: {{ let e = Event::Move(Point {{ x: x, y: 0 }}); match e {{ Event::Move(p) => p.x, _ => 0 }} }};\n{MAIN0}"
        ), "v18-case4")
        .iter()
        .all(|result| *result == z3::SatResult::Unsat));
        // Equality compares the tag plus every composite leaf.
        assert!(composite_v18_outcomes_or(&format!(
            "{COMPOSITE_PAYLOAD_DECLS}atom same_move(p: Point, q: Point) -> i64 requires: p.x == q.x && p.y == q.y; ensures: result == 1; body: {{ let a = Event::Move(p); let b = Event::Move(q); if a == b {{ 1 }} else {{ 0 }} }};\n{MAIN0}"
        ), "v18-case5")
        .iter()
        .all(|result| *result == z3::SatResult::Unsat));
        // A whole-composite payload write rebuilds the slot.
        assert!(composite_v18_outcomes_or(&format!(
            "{COMPOSITE_PAYLOAD_DECLS}atom move_to(e: Event, p: Point) -> i64 requires: p.x >= 0; ensures: result >= 0; body: {{ e.pos = p; match e {{ Event::Move(q) => q.x, _ => 0 }} }};\n{MAIN0}"
        ), "v18-case6")
        .iter()
        .all(|result| *result == z3::SatResult::Unsat));
    }

    #[test]
    fn composite_enum_payloads_refute_wrong_postconditions() {
        // Reading the wrong leaf refutes: `p.y` is unconstrained.
        assert!(composite_v18_outcomes_or(&format!(
            "{COMPOSITE_PAYLOAD_DECLS}atom bad(x: i64) -> i64 requires: x >= 0 && x <= 100; ensures: result >= 2; body: {{ let e = Event::Move(Point {{ x: x, y: 0 }}); match e {{ Event::Move(p) => p.y, _ => 0 }} }};\n{MAIN0}"
        ), "v18-case7")
        .iter()
        .any(|result| *result == z3::SatResult::Sat));
        // Unequal composite leaves make `a == b` refutable.
        assert!(composite_v18_outcomes(&format!(
            "{COMPOSITE_PAYLOAD_DECLS}atom bad(p: Point, q: Point) -> i64 requires: p.x != q.x; ensures: result == 1; body: {{ let a = Event::Move(p); let b = Event::Move(q); if a == b {{ 1 }} else {{ 0 }} }};\n{MAIN0}"
        ))
        .iter()
        .any(|result| *result == z3::SatResult::Sat));
    }

    #[test]
    fn composite_enum_payloads_need_v18() {
        // The same declarations stay out of every narrower fragment.
        let source = format!("{COMPOSITE_PAYLOAD_DECLS}{MAIN0}");
        for admission in [
            RefinedAdmission::Reject,
            RefinedAdmission::V1,
            RefinedAdmission::V2,
            RefinedAdmission::Composite,
            RefinedAdmission::CompositeV5,
            RefinedAdmission::CompositeV10,
            RefinedAdmission::CompositeV17,
        ] {
            assert!(
                extract_pure_i64_module_contracts(&source, "m.mm", None, admission).is_err(),
                "composite payload admitted under {admission:?}"
            );
        }
    }

    #[test]
    fn composite_enum_payloads_reject_enum_leaved_and_mismatched() {
        // A payload struct carrying an enum field is refused — its
        // leaves are not scalars.
        let enum_leaved = "enum Tag { A, B }\nstruct Wrap { t: Tag }\nenum Ev { V(w: Wrap) }\n"
            .to_string()
            + MAIN0;
        assert!(extract_pure_i64_module_contracts(
            &enum_leaved,
            "m.mm",
            None,
            RefinedAdmission::CompositeV18
        )
        .is_err());
        // Slot types must agree across variants: `pos` as `Point` in
        // one variant and `i64` in another is refused.
        let mismatched = "struct Point { x: i64, y: i64 }\nenum Ev { A(pos: Point), B(pos: i64) }\n"
            .to_string() + MAIN0;
        assert!(extract_pure_i64_module_contracts(
            &mismatched,
            "m.mm",
            None,
            RefinedAdmission::CompositeV18
        )
        .is_err());
        // A scalar arg into a composite ctor slot is refused.
        let scalar_ctor = format!(
            "{COMPOSITE_PAYLOAD_DECLS}atom bad(v: i64) -> Event requires: true; ensures: true; body: {{ Event::Move(v) }};\n{MAIN0}"
        );
        assert!(extract_pure_i64_module_contracts(
            &scalar_ctor,
            "m.mm",
            None,
            RefinedAdmission::CompositeV18
        )
        .is_err());
    }

    #[test]
    fn composite_enum_payloads_admit_shared_child_structs() {
        // `Pair` names `Point` twice — a shared child is a DAG, not a
        // cycle, and stays scalar-leaved (`pure-i64-composite/v18`).
        let decls = "struct Point { x: i64, y: i64 }\nstruct Pair { left: Point, right: Point }\nenum Ev { V(p: Pair) }\n";
        assert!(
            extract_pure_i64_module_contracts(
                &format!("{decls}atom step(e: Ev) -> i64 requires: true; ensures: result >= 0; body: {{ match e {{ Ev::V(p) => p.left.x, _ => 0 }} }};\n{MAIN0}"),
                "m.mm",
                None,
                RefinedAdmission::CompositeV18
            )
            .is_ok(),
            "a shared child struct must admit like a scalar-leaved one"
        );
    }

    #[test]
    fn composite_enum_payloads_reject_wide_declarations_without_expanding() {
        // A >16-leaf declaration must fail the leaf budget WITHOUT
        // `declared_field_paths` materialising the exponential path set or
        // `declared_leaf_width` walking it — the cap + gate-on-width order
        // is what makes wide declarations cheap to reject rather than an
        // allocation hang (8 levels × 16 children = 16^8 naive nodes).
        let mut decls = String::from("struct S0 {\n");
        for i in 0..16 {
            decls.push_str(&format!("    f{i}: i64,\n"));
        }
        decls.push_str("}\n");
        for level in 1..8 {
            decls.push_str(&format!("struct S{level} {{\n"));
            for i in 0..16 {
                decls.push_str(&format!("    f{i}: S{},\n", level - 1));
            }
            decls.push_str("}\n");
        }
        decls.push_str("enum Ev { V(p: S3) }\n");
        assert!(
            extract_pure_i64_module_contracts(
                &format!("{decls}atom step(e: Ev) -> i64 requires: true; ensures: true; body: {{ 0 }};\n{MAIN0}"),
                "m.mm",
                None,
                RefinedAdmission::CompositeV18
            )
            .is_err(),
            "a >16-leaf declaration must stay outside v18"
        );
    }

    #[test]
    fn composite_enum_payload_binder_shadows_scalar() {
        // `E::Move(p)` binds a composite `p` that shadows the scalar
        // parameter `p`: inside the arm `p.x` resolves to the composite,
        // outside it `p` is still the plain parameter.
        let decls = "struct Pos { x: i64 }\nenum Event { Move(p: Pos) }\n";
        let report = extract_pure_i64_module_contracts(
            &format!("{decls}atom f(p: i64, e: Event) -> i64 requires: p >= 0; ensures: result >= 0; body: {{ match e {{ Event::Move(p) => if p.x >= 0 {{ p.x }} else {{ 0 }}, _ => p }} }};\n{MAIN0}"),
            "m.mm",
            None,
            RefinedAdmission::CompositeV18,
        );
        assert!(
            report.as_ref().is_ok_and(|contracts| !contracts.is_empty()),
            "bound composite must shadow the scalar param: {report:?}"
        );
    }

    #[test]
    fn composite_v19_admits_f64_arithmetic() {
        let v19 = |source: &str| {
            extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::CompositeV19)
        };
        let outcomes = |source: &str| {
            refined_module_outcomes_admission(source, RefinedAdmission::CompositeV19)
        };
        let module = |piece: &str| format!("{piece}\n{MAIN0}");
        for piece in [
            // Each admitted operator — add/sub/mul/div — on f64 leaves.
            // `ensures: result == x + y` would be refutable — NaN
            // inputs make `fp.eq` false, which is exactly the IEEE
            // equality the emitted `fcmp oeq` carries — so arithmetic
            // atoms keep `ensures: true`.
            "atom add(x: f64, y: f64) -> f64 requires: true; ensures: true; body: { x + y };",
            "atom sub(x: f64, y: f64) -> f64 requires: true; ensures: true; body: { x - y };",
            "atom mul(x: f64, y: f64) -> f64 requires: true; ensures: true; body: { x * y };",
            "atom div(x: f64, y: f64) -> f64 requires: true; ensures: true; body: { x / y };",
            // `x == y` reads as IEEE equality (`fcmp oeq`).
            "atom eq(x: f64, y: f64) -> bool requires: true; ensures: result == (x == y); body: { x == y };",
            // Compound expressions and requires both read the same way.
            "atom mix(x: f64, y: f64, z: f64) -> f64 requires: x == x; ensures: true; body: { x * y - x / z };",
        ] {
            let source = module(piece);
            match v19(&source) {
                Ok(cs) => assert!(
                    cs.iter()
                        .all(|c| c.profile == PURE_I64_COMPOSITE_PROFILE_V19),
                    "{piece}"
                ),
                Err(e) => panic!("{piece}: {e:?}"),
            }
            match std::panic::catch_unwind(|| outcomes(&source)) {
                Ok(res) => assert!(
                    res.iter().all(|r| *r == z3::SatResult::Unsat),
                    "{piece}"
                ),
                Err(_) => panic!("outcomes failed for: {piece}"),
            }
        }
        // v19 pins only when arithmetic is used: a pass-through-only
        // module keeps the v11 leaf profile.
        let copy = module("atom copy(a: f64) -> f64 requires: true; ensures: true; body: { a };");
        assert_eq!(
            module_source_profile(&copy, RefinedAdmission::CompositeV19).unwrap(),
            PURE_I64_COMPOSITE_PROFILE_V11
        );
        let source = module(
            "atom add(x: f64, y: f64) -> f64 requires: true; ensures: true; body: { x + y };",
        );
        assert_eq!(
            module_source_profile(&source, RefinedAdmission::CompositeV19).unwrap(),
            PURE_I64_COMPOSITE_PROFILE_V19
        );
        // Narrower admissions keep `f64` operators out — the v18
        // ladder top included.
        for admission in [
            RefinedAdmission::Reject,
            RefinedAdmission::CompositeV12,
            RefinedAdmission::CompositeV18,
        ] {
            assert!(
                extract_pure_i64_module_contracts(&source, "m.mm", None, admission).is_err(),
                "{admission:?}"
            );
        }
    }

    #[test]
    fn composite_v19_rejects_f64_ops_outside_the_emitter_surface() {
        let v19 = |source: &str| {
            extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::CompositeV19)
        };
        for (piece, why) in [
            // `!=` has no emitted counterpart — the emitter rejects it.
            (
                "atom c(a: f64, b: f64) -> bool requires: true; ensures: true; body: { a != b };",
                "f64 !=",
            ),
            // Ordering on `f64` is not emitted either.
            (
                "atom c(a: f64, b: f64) -> bool requires: true; ensures: true; body: { a < b };",
                "f64 lt",
            ),
            (
                "atom c(a: f64, b: f64) -> bool requires: true; ensures: true; body: { a >= b };",
                "f64 ge",
            ),
            // Remainder and bitwise ops stay integer-only.
            (
                "atom c(a: f64, b: f64) -> f64 requires: true; ensures: true; body: { a % b };",
                "f64 rem",
            ),
            (
                "atom c(a: f64, b: f64) -> f64 requires: true; ensures: true; body: { a & b };",
                "f64 bitand",
            ),
            // A `Num` literal never adapts to `f64` — `a + 1` fails
            // closed at the kind check, not later in the encoder.
            (
                "atom c(a: f64) -> f64 requires: true; ensures: true; body: { a + 1 };",
                "f64 literal mix",
            ),
            // A float literal is still outside the fragment.
            (
                "atom c(a: f64) -> f64 requires: true; ensures: true; body: { 1.5 };",
                "float literal",
            ),
        ] {
            assert!(
                v19(&format!("{piece}\n{MAIN0}")).is_err(),
                "admitted: {why}"
            );
        }
    }

    #[test]
    fn composite_v20_admits_a_declared_str_bound_clause() {
        let v20 = |source: &str| {
            extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::CompositeV20)
        };
        let outcomes = |source: &str| {
            refined_module_outcomes_admission(source, RefinedAdmission::CompositeV20)
        };
        let module = |piece: &str| format!("{piece}\n{MAIN0}");
        let lit = |n: usize| format!("\"{}\"", "a".repeat(n));
        for piece in [
            // A 100-byte literal needs `str_bound: 128;` — the default
            // `STR_BOUND` (64) would reject it.
            format!(
                "atom big() -> u64 requires: true; ensures: result == 97; str_bound: 128; body: {{ let s = {}; s[99] }};",
                lit(100)
            ),
            // A declared bound may narrow below the default.
            "atom tiny() -> u64 requires: true; ensures: result == 97; str_bound: 4; body: { let s = \"abc\"; s[0] };".to_string(),
        ] {
            let source = module(&piece);
            match v20(&source) {
                Ok(cs) => assert!(
                    cs.iter()
                        .all(|c| c.profile == PURE_I64_COMPOSITE_PROFILE_V20),
                    "{piece}"
                ),
                Err(e) => panic!("{piece}: {e:?}"),
            }
            match std::panic::catch_unwind(|| outcomes(&source)) {
                Ok(res) => assert!(
                    res.iter().all(|r| *r == z3::SatResult::Unsat),
                    "{piece}"
                ),
                Err(_) => panic!("outcomes failed for: {piece}"),
            }
        }
        // A declared bound below the literal's length still fails
        // closed — the bound is the cap, not a hint.
        let over_bound = module(&format!(
            "atom over() -> u64 requires: true; ensures: true; str_bound: 8; body: {{ let s = {}; s[0] }};",
            lit(9)
        ));
        assert!(v20(&over_bound).is_err());
        // The default `STR_BOUND` applies without the clause: a
        // 65-byte literal is outside the fragment.
        let over_default = module(&format!(
            "atom d() -> u64 requires: true; ensures: true; body: {{ let s = {}; s[0] }};",
            lit(65)
        ));
        assert!(v20(&over_default).is_err());
        // Above `STR_BOUND_MAX` the clause itself is refused, under
        // every admission.
        let too_big =
            module("atom m() -> i64 requires: true; ensures: true; str_bound: 2048; body: { 0 };");
        for admission in [
            RefinedAdmission::CompositeV20,
            RefinedAdmission::CompositeV19,
        ] {
            assert!(
                extract_pure_i64_module_contracts(&too_big, "m.mm", None, admission).is_err(),
                "{admission:?}"
            );
        }
        // The clause is v20-only: a narrower profile refuses it even
        // when the bound itself is small.
        let declared =
            module("atom m() -> i64 requires: true; ensures: true; str_bound: 8; body: { 0 };");
        assert!(extract_pure_i64_module_contracts(
            &declared,
            "m.mm",
            None,
            RefinedAdmission::CompositeV19
        )
        .is_err());
        // Declaring the clause pins v20 even when no `Str` literal is
        // used — the declaration-presence convention.
        assert_eq!(
            module_source_profile(&declared, RefinedAdmission::CompositeV20).unwrap(),
            PURE_I64_COMPOSITE_PROFILE_V20
        );
        // No clause and no `Str` literal keeps the wrapping profile —
        // the declaration alone is what pins v20.
        assert_eq!(
            module_source_profile(
                &module("atom m() -> i64 requires: true; ensures: true; body: { 0 };"),
                RefinedAdmission::CompositeV20
            )
            .unwrap(),
            PURE_I64_WRAPPING_PROFILE
        );
    }

    #[test]
    fn composite_v20_contract_literals_use_the_clause_atom_bound() {
        // A `requires`/`ensures` literal belongs to the atom that
        // declares the clause: with a later atom leaving `str_bound`
        // unset the shared cell would otherwise hold the default
        // `STR_BOUND` when the first atom's contract is checked —
        // a declared 100-byte clause literal must not be measured
        // against 64.
        let lit = format!("\"{}\"", "a".repeat(100));
        let requires_case = format!(
            "atom big(x: i64) -> i64 requires: len({lit}) == 100; ensures: result == x; str_bound: 128; body: {{ x }};\n\
             atom main() -> i64 requires: true; ensures: result == 0; body: {{ 0 }};"
        );
        let ensures_case = format!(
            "atom big(x: i64) -> i64 requires: true; ensures: len({lit}) == 100 || result == x; str_bound: 128; body: {{ x }};\n\
             atom main() -> i64 requires: true; ensures: result == 0; body: {{ 0 }};"
        );
        // Contract expressions are encoded inside
        // `module_atom_obligations`, which reads the shared cell —
        // declaration extraction alone never exercises the bug, so
        // these cases go through the full obligation encode. A `Str`
        // literal inside a clause is always rejected downstream
        // ("array value in a scalar position"); the bound check runs
        // first, so which error fires tells us which bound the cell
        // held: honoring the clause atom's bound 128 reaches the
        // scalar-position rejection, while a stale 64 (the bug)
        // stops earlier at "str literal bound".
        let encode_err = |source: &String| {
            let mut config = z3::Config::new();
            config.set_timeout_msec(crate::verification::VerificationConfig::default().timeout_ms);
            let context = z3::Context::new(&config);
            let result = encode_pure_i64_module_obligations(
                source,
                &context,
                RefinedAdmission::CompositeV20,
            );
            match result {
                Ok(_) => String::new(),
                Err(e) => format!("{e}"),
            }
        };
        for source in [requires_case, ensures_case] {
            let err = encode_err(&source);
            assert!(err.contains("scalar position"), "{source}: {err}");
        }
        // Same cell through the call-site path: the callee's own
        // clause must be encoded under the callee's declared 128,
        // not the caller's default 64 — a leaked 64 trips the bound
        // check before the scalar-position rejection can fire.
        let callee_bound_case = format!(
            "atom callee() -> i64 requires: len({lit}) == 100; ensures: result == 0; str_bound: 128; body: {{ 0 }};\n\
             atom main() -> i64 requires: true; ensures: result == 0; body: {{ callee() }};"
        );
        let err = encode_err(&callee_bound_case);
        assert!(
            err.contains("scalar position"),
            "{callee_bound_case}: {err}"
        );
        // And the inverse still fails closed at extraction: a callee
        // with no declared bound cannot use the 100-byte literal, no
        // matter what the caller declares.
        let callee_default_case = format!(
            "atom callee() -> i64 requires: len({lit}) == 100; ensures: result == 0; body: {{ 0 }};\n\
             atom main() -> i64 requires: true; ensures: result == 0; str_bound: 128; body: {{ callee() }};"
        );
        assert!(extract_pure_i64_module_contracts(
            &callee_default_case,
            "m.mm",
            None,
            RefinedAdmission::CompositeV20
        )
        .is_err());
    }

    #[test]
    fn composite_v21_admits_i128_u128_leaves() {
        let v21 = |source: &str| {
            extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::CompositeV21)
        };
        let outcomes = |source: &str| {
            refined_module_outcomes_admission(source, RefinedAdmission::CompositeV21)
        };
        let module = |piece: &str| format!("{piece}\n{MAIN0}");
        for (piece, profile) in [
            // Signed and unsigned leaves: arithmetic, comparisons and
            // a sign-extended literal fold.
            (
                "atom add(x: i128, y: i128) -> i128 requires: true; ensures: result == x + y; body: { x + y };",
                PURE_I64_COMPOSITE_PROFILE_V21,
            ),
            (
                "atom uadd(x: u128, y: u128) -> u128 requires: true; ensures: result == x + y; body: { x + y };",
                PURE_I64_COMPOSITE_PROFILE_V21,
            ),
            (
                "atom neg(x: i128) -> i128 requires: true; ensures: result == x - 1; body: { x + -1 };",
                PURE_I64_COMPOSITE_PROFILE_V21,
            ),
            (
                "atom eq(x: i128, y: i128) -> bool requires: true; ensures: result == (x == y); body: { x == y };",
                PURE_I64_COMPOSITE_PROFILE_V21,
            ),
            (
                "atom ult(x: u128, y: u128) -> bool requires: true; ensures: result == (x < y); body: { x < y };",
                PURE_I64_COMPOSITE_PROFILE_V21,
            ),
            (
                "atom slt(x: i128, y: i128) -> bool requires: true; ensures: result == (x < y); body: { x < y };",
                PURE_I64_COMPOSITE_PROFILE_V21,
            ),
            // A struct leaf carrying a 128-bit field and an i128 call
            // round trip ride the same two-slot pairing.
            (
                "struct S { a: i64, b: i128 }\natom get(s: S) -> i128 requires: true; ensures: result == s.b; body: { s.b };",
                PURE_I64_COMPOSITE_PROFILE_V21,
            ),
            (
                "atom id(x: i128) -> i128 requires: true; ensures: result == x; body: { x };\natom call(y: i128) -> i128 requires: true; ensures: result == y; body: { id(y) };",
                PURE_I64_COMPOSITE_PROFILE_V21,
            ),
        ] {
            let source = module(piece);
            match v21(&source) {
                Ok(cs) => assert!(
                    cs.iter().all(|c| c.profile == profile),
                    "{piece}"
                ),
                Err(e) => panic!("{piece}: {e:?}"),
            }
            match std::panic::catch_unwind(|| outcomes(&source)) {
                Ok(res) => assert!(
                    res.iter().all(|r| *r == z3::SatResult::Unsat),
                    "{piece}"
                ),
                Err(_) => panic!("outcomes failed for: {piece}"),
            }
        }
        // Declaring a wide leaf pins v21 even when the body only
        // passes it through — the declaration-presence convention.
        let pass =
            module("atom copy(a: i128) -> i128 requires: true; ensures: result == a; body: { a };");
        assert_eq!(
            module_source_profile(&pass, RefinedAdmission::CompositeV21).unwrap(),
            PURE_I64_COMPOSITE_PROFILE_V21
        );
        // No wide leaf keeps the narrower profile.
        assert_eq!(
            module_source_profile(
                &module("atom m() -> i64 requires: true; ensures: true; body: { 0 };"),
                RefinedAdmission::CompositeV21
            )
            .unwrap(),
            PURE_I64_WRAPPING_PROFILE
        );
    }

    #[test]
    fn composite_v21_keeps_wide_leaves_fail_closed_elsewhere() {
        let v21 = |source: &str| {
            extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::CompositeV21)
        };
        let module = |piece: &str| format!("{piece}\n{MAIN0}");
        // An `i128`/`u128` declaration under a narrower admission is
        // refused — the v20 ladder top included.
        let decl = module(
            "atom add(x: i128, y: i128) -> i128 requires: true; ensures: true; body: { x + y };",
        );
        for admission in [
            RefinedAdmission::Reject,
            RefinedAdmission::Composite,
            RefinedAdmission::CompositeV20,
        ] {
            assert!(
                extract_pure_i64_module_contracts(&decl, "m.mm", None, admission).is_err(),
                "{admission:?}"
            );
        }
        for (piece, why) in [
            // Signed/unsigned leaves never mix in one operand pair.
            (
                "atom mix(x: i128, y: u128) -> i128 requires: true; ensures: true; body: { x + y };",
                "i128/u128 mixing",
            ),
            // A 64-bit leaf never coerces into a wide operand.
            (
                "atom mix(x: i128, y: i64) -> i128 requires: true; ensures: true; body: { x + y };",
                "i64/i128 mixing",
            ),
            // Enum payloads stay 64-bit — no wide payload variant.
            (
                "enum E { V(i128) }\natom f(e: E) -> i128 requires: true; ensures: true; body: { 0 };",
                "wide enum payload",
            ),
        ] {
            assert!(v21(&module(piece)).is_err(), "admitted: {why}");
        }
    }

    #[test]
    fn module_leaf_widths_pairs_wide_slots() {
        let module = |piece: &str| format!("{piece}\n{MAIN0}");
        let source = module(
            "struct S { a: i64, b: i128 }\natom f(x: i64, s: S, u: u128) -> i64 requires: true; ensures: true; body: { x };\natom g() -> i128 requires: true; ensures: true; body: { 0 };",
        );
        let widths = module_leaf_widths(&source, RefinedAdmission::CompositeV21).unwrap();
        assert_eq!(widths["f"], vec![64, 64, 128, 128]);
        assert_eq!(widths["g"], Vec::<u64>::new());
        assert_eq!(widths["main"], Vec::<u64>::new());
        // Under a narrower admission the module does not parse as the
        // composite fragment at all.
        assert!(module_leaf_widths(&source, RefinedAdmission::CompositeV20).is_err());
    }

    #[test]
    fn composite_v22_admits_wide_enum_payloads() {
        let v22 = |source: &str| {
            extract_pure_i64_module_contracts(source, "m.mm", None, RefinedAdmission::CompositeV22)
        };
        let outcomes = |source: &str| {
            refined_module_outcomes_admission(source, RefinedAdmission::CompositeV22)
        };
        let module = |piece: &str| format!("{piece}\n{MAIN0}");
        for (piece, profile) in [
            // A uniform `i128` payload position occupies two 64-bit
            // input slots; the binder reads the paired leaf and a
            // catch-all literal adapts to the arm's wide kind.
            (
                "enum F { A(i128), B(i128) }\natom pick(f: F) -> i128 requires: true; ensures: true; body: { match f { F::A(x) => x, _ => 0 } };",
                PURE_I64_COMPOSITE_PROFILE_V22,
            ),
            // `u128` rides the same two-slot carrier.
            (
                "enum F { A(u128), B(u128) }\natom pick(f: F) -> bool requires: true; ensures: true; body: { match f { F::A(x) => x > 0, _ => false } };",
                PURE_I64_COMPOSITE_PROFILE_V22,
            ),
            // A wide leaf inside a struct-typed payload position.
            (
                "struct P { x: i128, y: i64 }\nenum F { A(P), B(P) }\natom pick(f: F) -> i64 requires: true; ensures: true; body: { match f { F::A(p) => p.y, _ => 0 } };",
                PURE_I64_COMPOSITE_PROFILE_V22,
            ),
            // An enum ctor whose payload position carries a wide leaf.
            (
                "enum F { A(i128), B(i128) }\natom wrap(x: i128) -> F requires: true; ensures: true; body: { F::A(x) };",
                PURE_I64_COMPOSITE_PROFILE_V22,
            ),
        ] {
            let source = module(piece);
            match v22(&source) {
                Ok(cs) => assert!(
                    cs.iter().all(|c| c.profile == profile),
                    "{piece}"
                ),
                Err(e) => panic!("{piece}: {e:?}"),
            }
            match std::panic::catch_unwind(|| outcomes(&source)) {
                Ok(res) => assert!(
                    res.iter().all(|r| *r == z3::SatResult::Unsat),
                    "{piece}"
                ),
                Err(_) => panic!("outcomes failed for: {piece}"),
            }
        }
        // A wrong postcondition over the bound wide leaf is refuted,
        // not vacuous.
        let wrong = module(
            "enum F { A(i128), B(i128) }\natom pick(f: F) -> i128 requires: true; ensures: result == 0; body: { match f { F::A(x) => x, _ => 0 } };",
        );
        assert!(outcomes(&wrong).contains(&z3::SatResult::Sat));
        // Mixed-type payload positions stay outside the fragment:
        // `A(i128), B(i64)` has no uniform slot type.
        let mixed = module(
            "enum F { A(i128), B(i64) }\natom pick(f: F) -> i64 requires: true; ensures: true; body: { match f { F::A(x) => 1, _ => 0 } };",
        );
        assert!(v22(&mixed).is_err());
        // Under a narrower admission a wide payload enum is refused.
        let decl = module(
            "enum F { A(i128), B(i128) }\natom pick(f: F) -> i128 requires: true; ensures: true; body: { match f { F::A(x) => x, _ => 0 } };",
        );
        for admission in [
            RefinedAdmission::Reject,
            RefinedAdmission::Composite,
            RefinedAdmission::CompositeV20,
            RefinedAdmission::CompositeV21,
        ] {
            assert!(
                extract_pure_i64_module_contracts(&decl, "m.mm", None, admission).is_err(),
                "{admission:?}"
            );
        }
        // `leaf_widths` reports one 128-bit leaf per wide payload
        // slot, tag excluded.
        let widths = module_leaf_widths(&decl, RefinedAdmission::CompositeV22).unwrap();
        assert_eq!(widths["pick"], vec![64, 128]);
    }
}

/// Whether a flattened enum payload slot lowers with the profile's
/// `bool` 0/1 convention: a literal `bool` field type or a bool-leaf
/// refined alias.
fn enum_slot_is_bool(type_name: Option<&str>, refined: &RefinedEnv) -> bool {
    match type_name {
        Some("bool") => true,
        Some(name) => refined
            .get(name)
            .is_some_and(|alias| alias.kind == ScalarKind::Bool),
        None => false,
    }
}
