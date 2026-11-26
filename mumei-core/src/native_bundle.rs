//! Identity binding for a deliberately restricted native build. Not artifact proof.

use crate::semantic_contract::RefinedAdmission;
use crate::{parser, proof_cert, semantic_contract, verification};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const SCHEMA: &str = "mumei.native-bundle/v1";
pub const PROFILE: &str = "mumei.native-pure-i64-main-linux-x86_64/v1";
pub const SOURCE_LABEL: &str = "native-source.mm";
pub const RECORD_FILE: &str = "native-bundle.json";
pub const SCOPE: &str = "identity_consistency_only/no_authenticity_or_artifact_semantic_proof";
pub const SYSTEM_SCOPE: &str = "system_crt_libraries_environment_and_tool_execution_not_attested";
pub const RECHECK_SCOPE: &str =
    "source_postcondition_and_identity_only/no_translation_authenticity_or_binary_semantic_proof";
pub const RECHECK_PROFILE: &str = "mumei.native-source-recheck/v1";
pub const RECHECK_RESOURCE_LIMIT: u32 = 1_000_000;

/// Multi-atom module bundle profile: one zero-argument `main` plus sibling
/// atoms admitted under `mumei.pure-i64-wrapping-scalar/v1` as call targets.
/// The fragment inherits wrapping i64 add/sub/mul — a semantic widening
/// beyond strict C03, pinned in the record's `profile`.
pub const MODULE_SCHEMA: &str = "mumei.native-module-bundle/v1";
pub const MODULE_PROFILE: &str = "mumei.native-pure-i64-module-linux-x86_64/v1";
/// v2 profile: the module gate additionally admits refined scalar type
/// declarations (`type Port = i64 where ...`) and signature members
/// typed by them. The fragment's atoms and their verification model
/// are unchanged — the record's `profile` names which admission ran so
/// a consumer re-derives the binding under exactly the producer's gate.
pub const MODULE_PROFILE_V2: &str = "mumei.native-pure-i64-module-linux-x86_64/v2";
/// v3 profile: the module gate additionally admits the widened
/// refined-scalar fragment (`mumei.pure-i64-refined-scalar/v2`) —
/// chained declarations and `bool` leaves the verifier's boundary
/// enforcement now composes. v2 records keep the v1 member gate.
pub const MODULE_PROFILE_V3: &str = "mumei.native-pure-i64-module-linux-x86_64/v3";
/// v4 profile: the module gate additionally admits non-generic,
/// all-scalar-field `struct` declarations as members, struct-typed atom
/// parameters and field reads on them (`mumei.pure-i64-composite/v1`).
/// A wider admitted fragment, not deeper assurance — the source
/// recheck flattens each struct parameter to its scalar fields, while
/// the LLVM snapshot recheck keeps aggregate member signatures outside
/// its translation profile and fails closed on them.
pub const MODULE_PROFILE_V4: &str = "mumei.native-pure-i64-module-linux-x86_64/v4";
/// v5 profile: the composite gate additionally admits refined-alias
/// struct field types (`mumei.pure-i64-composite/v2`). v4 records keep
/// the literal `i64`/`bool` field gate.
pub const MODULE_PROFILE_V5: &str = "mumei.native-pure-i64-module-linux-x86_64/v5";
/// v6 profile: the composite gate additionally admits struct values —
/// struct results, `StructInit`, struct `let`s and call-result field
/// reads (`mumei.pure-i64-composite/v3`). v4/v5 records keep their own
/// gates.
pub const MODULE_PROFILE_V6: &str = "mumei.native-pure-i64-module-linux-x86_64/v6";
/// Module profile v7: the enum-member widening of
/// `mumei.pure-i64-composite/v4` — admitted `enum` declarations and
/// enum parameters flattened to a tag slot plus payload slots. v1–v6
/// records keep their own gates.
pub const MODULE_PROFILE_V7: &str = "mumei.native-pure-i64-module-linux-x86_64/v7";
/// Module profile v8: the composite gate additionally admits field
/// assignment `q.f = v` on admitted struct bindings
/// (`mumei.pure-i64-composite/v5`). v1–v7 records keep their own
/// gates.
pub const MODULE_PROFILE_V8: &str = "mumei.native-pure-i64-module-linux-x86_64/v8";
/// Module profile v9: the composite gate additionally admits enum
/// values — `E::V(..)` construction, `-> E` results and
/// enum-returning calls (`mumei.pure-i64-composite/v6`). v1–v8
/// records keep their own gates.
pub const MODULE_PROFILE_V9: &str = "mumei.native-pure-i64-module-linux-x86_64/v9";
/// Module profile v10: the composite gate additionally admits
/// branch-merged composite values — `if`/`match` expressions whose
/// tails produce the same struct or enum type, and composite writes
/// joined under a branch merge (`mumei.pure-i64-composite/v7`).
/// v1–v9 records keep their own gates.
pub const MODULE_PROFILE_V10: &str = "mumei.native-pure-i64-module-linux-x86_64/v10";
/// Module profile v11: the composite gate additionally admits
/// struct-valued fields — `struct Outer { inner: Inner, c: i64 }`
/// declarations, nested field reads `o.inner.a`, and composite
/// writes carried by nested-path binders
/// (`mumei.pure-i64-composite/v8`). v1–v10 records keep their own
/// gates.
pub const MODULE_PROFILE_V11: &str = "mumei.native-pure-i64-module-linux-x86_64/v11";
/// Module profile v12: the composite gate additionally admits
/// generic `struct` declarations — every concrete use is
/// instantiated by monomorphization before verification
/// (`mumei.pure-i64-composite/v9`). v1–v11 records keep their own
/// gates.
pub const MODULE_PROFILE_V12: &str = "mumei.native-pure-i64-module-linux-x86_64/v12";
/// Module profile v13: the composite gate additionally admits `u64`
/// scalar leaves under unsigned semantics
/// (`mumei.pure-i64-composite/v10`). v1–v12 records keep their own
/// gates.
pub const MODULE_PROFILE_V13: &str = "mumei.native-pure-i64-module-linux-x86_64/v13";
/// Module profile v14: the composite gate additionally admits `let`-
/// bound stack arrays under the bounded-memory model
/// (`mumei.pure-i64-composite-bounded-memory/v1`). v1–v13 records keep
/// their own gates.
pub const MODULE_PROFILE_V14: &str = "mumei.native-pure-i64-module-linux-x86_64/v14";
/// Module v15: the `f64` pass-through leaf widening of
/// `MODULE_PROFILE_V14` — one 64-bit slot per `f64` leaf carrying the
/// IEEE-754 bit pattern opaquely (no arithmetic or comparison reads
/// it). v1–v14 records keep their own gates.
pub const MODULE_PROFILE_V15: &str = "mumei.native-pure-i64-module-linux-x86_64/v15";
/// Module v16: the `Str` leaf widening of `MODULE_PROFILE_V15` — a
/// bounded inline byte sequence under the bounded-memory model
/// (`pure-i64-composite/v13`). v1–v15 records keep their own gates.
pub const MODULE_PROFILE_V16: &str = "mumei.native-pure-i64-module-linux-x86_64/v16";
/// Module v17: the enum-equality widening of `MODULE_PROFILE_V16` —
/// `e == e'`/`e != e'` on two values of one declared enum, a
/// positional compare over the tagged-union layout
/// (`pure-i64-composite/v14`). v1–v16 records keep their own gates.
pub const MODULE_PROFILE_V17: &str = "mumei.native-pure-i64-module-linux-x86_64/v17";
/// Module v18: the nested-field-write widening — a field assignment
/// may descend through struct-typed fields (`q.inner.a = v`, or a
/// whole-subtree write at depth) under `pure-i64-composite/v15`.
/// v1–v16 records keep their own gates; v17 names the enum-equality
/// widening on the composite ladder.
pub const MODULE_PROFILE_V18: &str = "mumei.native-pure-i64-module-linux-x86_64/v18";
/// Module v19: the enum-payload-write widening — `e.p = v` writes the
/// payload slot `p` of the variant the tag provably carries under the
/// current path (`pure-i64-composite/v16`). v1–v18 records keep their
/// own gates.
pub const MODULE_PROFILE_V19: &str = "mumei.native-pure-i64-module-linux-x86_64/v19";
/// Module v20: the enum-typed-struct-field widening — a field
/// `e: Color` flattens to a tag plus payload leaf slots exactly like
/// an enum parameter under `pure-i64-composite/v17`. (v19 names the
/// enum-payload-write widening on the composite ladder.) v1–v18
/// records keep their own gates.
pub const MODULE_PROFILE_V20: &str = "mumei.native-pure-i64-module-linux-x86_64/v20";
/// Module v21: the composite-enum-payload widening — an enum payload
/// field may name an admitted struct (`enum E { V(Point) }`), the
/// slot carrying the struct's flattened leaves as one aggregate value
/// under `pure-i64-composite/v18`. v1–v20 records keep their own
/// gates.
pub const MODULE_PROFILE_V21: &str = "mumei.native-pure-i64-module-linux-x86_64/v21";
/// Module v22: the `f64`-arithmetic widening of `MODULE_PROFILE_V21` —
/// source `+ - * / ==` on `f64` leaves admit under
/// `pure-i64-composite/v19`, decoding the bit-pattern slots as IEEE 754
/// binary64 under round-nearest-ties-to-even; the IR whitelist gains
/// the emitted `fadd`/`fsub`/`fmul`/`fdiv`/`fcmp oeq` shapes. Float
/// literals and `!=`/ordering stay outside. v1–v21 records keep their
/// own gates.
pub const MODULE_PROFILE_V22: &str = "mumei.native-pure-i64-module-linux-x86_64/v22";
/// v22 plus the `str_bound: N;` clause — atoms may declare a per-atom
/// `Str` bound (`pure-i64-composite/v20`).
pub const MODULE_PROFILE_V23: &str = "mumei.native-pure-i64-module-linux-x86_64/v23";
/// v23 plus two-slot `i128`/`u128` leaves — the record must pin
/// `leaf_widths` (`pure-i64-composite/v21`).
pub const MODULE_PROFILE_V24: &str = "mumei.native-pure-i64-module-linux-x86_64/v24";
/// v24 plus two-slot `i128`/`u128` leaves inside enum payload positions
/// (`pure-i64-composite/v22`).
pub const MODULE_PROFILE_V25: &str = "mumei.native-pure-i64-module-linux-x86_64/v25";
/// v25 plus one-level nested enum payloads (`pure-i64-composite/v23`).
pub const MODULE_PROFILE_V26: &str = "mumei.native-pure-i64-module-linux-x86_64/v26";
/// v26 plus compiler-inferred i64 returns on non-main atoms
/// (`pure-i64-composite/v24`).
pub const MODULE_PROFILE_V27: &str = "mumei.native-pure-i64-module-linux-x86_64/v27";
/// v27 plus simple stateful effect declarations and statement-position
/// `perform` (`pure-i64-composite/v25`).
pub const MODULE_PROFILE_V28: &str = "mumei.native-pure-i64-module-linux-x86_64/v28";
/// v28 plus read-only plain `[i64]` array parameters
/// (`pure-i64-composite/v26`).
pub const MODULE_PROFILE_V29: &str = "mumei.native-pure-i64-module-linux-x86_64/v29";
/// v29 plus `[i64]` return values bound as uninterpreted
/// `(len, element store)` pairs (`pure-i64-composite/v29`).
pub const MODULE_PROFILE_V30: &str = "mumei.native-pure-i64-module-linux-x86_64/v30";
pub const MODULE_SCOPE: &str =
    "identity_consistency_only/no_authenticity_or_artifact_semantic_proof";
pub const MODULE_RECHECK_PROFILE: &str = "mumei.native-module-source-recheck/v1";

/// The admission mode a pinned module/multifile record demands: v1
/// uses `Reject`, v2/v3 use `V1`/`V2`, and v4–v29 map in order to
/// `Composite` through `CompositeV26`; v30 maps to `CompositeV29`.
/// Anything else is not this schema's record.
pub fn module_refined_admission(profile: &str) -> Result<RefinedAdmission, BundleError> {
    match profile {
        MODULE_PROFILE => Ok(RefinedAdmission::Reject),
        MODULE_PROFILE_V2 => Ok(RefinedAdmission::V1),
        MODULE_PROFILE_V3 => Ok(RefinedAdmission::V2),
        MODULE_PROFILE_V4 => Ok(RefinedAdmission::Composite),
        MODULE_PROFILE_V5 => Ok(RefinedAdmission::CompositeV2),
        MODULE_PROFILE_V6 => Ok(RefinedAdmission::CompositeV3),
        MODULE_PROFILE_V7 => Ok(RefinedAdmission::CompositeV4),
        MODULE_PROFILE_V8 => Ok(RefinedAdmission::CompositeV5),
        MODULE_PROFILE_V9 => Ok(RefinedAdmission::CompositeV6),
        MODULE_PROFILE_V10 => Ok(RefinedAdmission::CompositeV7),
        MODULE_PROFILE_V11 => Ok(RefinedAdmission::CompositeV8),
        MODULE_PROFILE_V12 => Ok(RefinedAdmission::CompositeV9),
        MODULE_PROFILE_V13 => Ok(RefinedAdmission::CompositeV10),
        MODULE_PROFILE_V14 => Ok(RefinedAdmission::CompositeV11),
        MODULE_PROFILE_V15 => Ok(RefinedAdmission::CompositeV12),
        MODULE_PROFILE_V16 => Ok(RefinedAdmission::CompositeV13),
        MODULE_PROFILE_V17 => Ok(RefinedAdmission::CompositeV14),
        MODULE_PROFILE_V18 => Ok(RefinedAdmission::CompositeV15),
        MODULE_PROFILE_V19 => Ok(RefinedAdmission::CompositeV16),
        MODULE_PROFILE_V20 => Ok(RefinedAdmission::CompositeV17),
        MODULE_PROFILE_V21 => Ok(RefinedAdmission::CompositeV18),
        MODULE_PROFILE_V22 => Ok(RefinedAdmission::CompositeV19),
        MODULE_PROFILE_V23 => Ok(RefinedAdmission::CompositeV20),
        MODULE_PROFILE_V24 => Ok(RefinedAdmission::CompositeV21),
        MODULE_PROFILE_V25 => Ok(RefinedAdmission::CompositeV22),
        MODULE_PROFILE_V26 => Ok(RefinedAdmission::CompositeV23),
        MODULE_PROFILE_V27 => Ok(RefinedAdmission::CompositeV24),
        MODULE_PROFILE_V28 => Ok(RefinedAdmission::CompositeV25),
        MODULE_PROFILE_V29 => Ok(RefinedAdmission::CompositeV26),
        MODULE_PROFILE_V30 => Ok(RefinedAdmission::CompositeV29),
        _ => Err(BundleError("unsupported schema, scope or settings")),
    }
}

/// Multi-file module bundle profile: the module profile's import-closure
/// extension. The producer merges a root `.mm` member and the files its
/// single-level same-directory `import` declarations name into one
/// canonical text — the language's own merged-namespace model makes the
/// merge semantics-preserving for this fragment (a qualified `h.f(` call
/// collects as the bare callee `f`, and the complete callee map is
/// declaration-order-insensitive). The merged text is what gets compiled
/// and verified; the original members are pinned under `sources/` and
/// the file-level import graph in `source_files`.
pub const MULTIFILE_SCHEMA: &str = "mumei.native-multifile-bundle/v1";
pub const MULTIFILE_PROFILE: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v1";
/// Multifile v2: the same refined-scalar widening as `MODULE_PROFILE_V2`
/// over the import-closure merge.
pub const MULTIFILE_PROFILE_V2: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v2";
/// Multifile v3: the widened refined-scalar fragment of
/// `MODULE_PROFILE_V3` over the import-closure merge.
pub const MULTIFILE_PROFILE_V3: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v3";
/// Multifile v4: the composite fragment of `MODULE_PROFILE_V4` over the
/// import-closure merge — struct declarations ride the merge like
/// refined `type` declarations do.
pub const MULTIFILE_PROFILE_V4: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v4";
/// Multifile v5: the refined-alias field widening of `MODULE_PROFILE_V5`.
pub const MULTIFILE_PROFILE_V5: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v5";
/// Multifile v6: the struct-value widening of `MODULE_PROFILE_V6`.
pub const MULTIFILE_PROFILE_V6: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v6";
/// Multifile v7: the enum-member widening of `MODULE_PROFILE_V7`.
pub const MULTIFILE_PROFILE_V7: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v7";
/// Multifile v8: the field-assignment widening of `MODULE_PROFILE_V8`.
pub const MULTIFILE_PROFILE_V8: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v8";
/// Multifile v9: the enum-value widening of `MODULE_PROFILE_V9`.
pub const MULTIFILE_PROFILE_V9: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v9";
/// Multifile v10: the branch-merged-composite widening of
/// `MODULE_PROFILE_V10`.
pub const MULTIFILE_PROFILE_V10: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v10";
/// Multifile v11: the nested-struct-field widening of
/// `MODULE_PROFILE_V11`.
pub const MULTIFILE_PROFILE_V11: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v11";
/// Multifile v12: the generic-struct-declaration widening of
/// `MODULE_PROFILE_V12`.
pub const MULTIFILE_PROFILE_V12: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v12";
/// Multifile v13: the `u64`-leaf widening of `MODULE_PROFILE_V13`.
pub const MULTIFILE_PROFILE_V13: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v13";
/// Multifile v14: the bounded-memory-array widening of
/// `MODULE_PROFILE_V14`.
pub const MULTIFILE_PROFILE_V14: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v14";
/// Multifile v15: the `f64` pass-through leaf widening of
/// `MODULE_PROFILE_V15`.
pub const MULTIFILE_PROFILE_V15: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v15";
/// Multifile v16: the `Str` leaf widening of `MODULE_PROFILE_V16`.
pub const MULTIFILE_PROFILE_V16: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v16";
/// Multifile v17: the enum-equality widening of `MODULE_PROFILE_V17`.
pub const MULTIFILE_PROFILE_V17: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v17";
/// Multifile v18: the nested-field-write widening of
/// `MODULE_PROFILE_V18`.
pub const MULTIFILE_PROFILE_V18: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v18";
/// Multifile v19: the enum-payload-write widening of
/// `MODULE_PROFILE_V19`.
pub const MULTIFILE_PROFILE_V19: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v19";
/// Multifile v20: the enum-typed-struct-field widening of
/// `MODULE_PROFILE_V20` (v19 names the enum-payload-write widening).
pub const MULTIFILE_PROFILE_V20: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v20";
/// Multifile v21: the composite-enum-payload widening of
/// `MODULE_PROFILE_V21` — `pure-i64-composite/v18`.
pub const MULTIFILE_PROFILE_V21: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v21";
/// Multifile v22: the `f64`-arithmetic widening of
/// `MULTIFILE_PROFILE_V21` — the same `pure-i64-composite/v19`
/// admission and IR whitelist `MODULE_PROFILE_V22` names, under the
/// import-closure member shape. v1–v21 records keep their own gates.
pub const MULTIFILE_PROFILE_V22: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v22";
/// v22 plus the `str_bound: N;` clause — atoms may declare a per-atom
/// `Str` bound (`pure-i64-composite/v20`).
pub const MULTIFILE_PROFILE_V23: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v23";
/// v23 plus two-slot `i128`/`u128` leaves — the record must pin
/// `leaf_widths` (`pure-i64-composite/v21`).
pub const MULTIFILE_PROFILE_V24: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v24";
/// v24 plus two-slot `i128`/`u128` leaves inside enum payload positions
/// (`pure-i64-composite/v22`).
pub const MULTIFILE_PROFILE_V25: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v25";
/// v25 plus one-level nested enum payloads (`pure-i64-composite/v23`).
pub const MULTIFILE_PROFILE_V26: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v26";
/// Multifile v27: v26 plus compiler-inferred i64 returns on non-main
/// atoms (`pure-i64-composite/v24`).
pub const MULTIFILE_PROFILE_V27: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v27";
/// Multifile v28: v27 plus simple stateful effect declarations and
/// statement-position `perform` (`pure-i64-composite/v25`).
pub const MULTIFILE_PROFILE_V28: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v28";
/// Multifile v29: v28 plus read-only plain `[i64]` array parameters
/// (`pure-i64-composite/v26`).
pub const MULTIFILE_PROFILE_V29: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v29";
/// Multifile v30: v29 plus `[i64]` return values bound as uninterpreted
/// `(len, element store)` pairs (`pure-i64-composite/v29`).
pub const MULTIFILE_PROFILE_V30: &str = "mumei.native-pure-i64-multifile-linux-x86_64/v30";
pub const MULTIFILE_RECHECK_PROFILE: &str = "mumei.native-multifile-source-recheck/v1";

/// Decode a multifile record's pinned source admission; v29 uses
/// `CompositeV26`, while older profile numbers retain their own gates.
pub fn multifile_refined_admission(profile: &str) -> Result<RefinedAdmission, BundleError> {
    match profile {
        MULTIFILE_PROFILE => Ok(RefinedAdmission::Reject),
        MULTIFILE_PROFILE_V2 => Ok(RefinedAdmission::V1),
        MULTIFILE_PROFILE_V3 => Ok(RefinedAdmission::V2),
        MULTIFILE_PROFILE_V4 => Ok(RefinedAdmission::Composite),
        MULTIFILE_PROFILE_V5 => Ok(RefinedAdmission::CompositeV2),
        MULTIFILE_PROFILE_V6 => Ok(RefinedAdmission::CompositeV3),
        MULTIFILE_PROFILE_V7 => Ok(RefinedAdmission::CompositeV4),
        MULTIFILE_PROFILE_V8 => Ok(RefinedAdmission::CompositeV5),
        MULTIFILE_PROFILE_V9 => Ok(RefinedAdmission::CompositeV6),
        MULTIFILE_PROFILE_V10 => Ok(RefinedAdmission::CompositeV7),
        MULTIFILE_PROFILE_V11 => Ok(RefinedAdmission::CompositeV8),
        MULTIFILE_PROFILE_V12 => Ok(RefinedAdmission::CompositeV9),
        MULTIFILE_PROFILE_V13 => Ok(RefinedAdmission::CompositeV10),
        MULTIFILE_PROFILE_V14 => Ok(RefinedAdmission::CompositeV11),
        MULTIFILE_PROFILE_V15 => Ok(RefinedAdmission::CompositeV12),
        MULTIFILE_PROFILE_V16 => Ok(RefinedAdmission::CompositeV13),
        MULTIFILE_PROFILE_V17 => Ok(RefinedAdmission::CompositeV14),
        MULTIFILE_PROFILE_V18 => Ok(RefinedAdmission::CompositeV15),
        MULTIFILE_PROFILE_V19 => Ok(RefinedAdmission::CompositeV16),
        MULTIFILE_PROFILE_V20 => Ok(RefinedAdmission::CompositeV17),
        MULTIFILE_PROFILE_V21 => Ok(RefinedAdmission::CompositeV18),
        MULTIFILE_PROFILE_V22 => Ok(RefinedAdmission::CompositeV19),
        MULTIFILE_PROFILE_V23 => Ok(RefinedAdmission::CompositeV20),
        MULTIFILE_PROFILE_V24 => Ok(RefinedAdmission::CompositeV21),
        MULTIFILE_PROFILE_V25 => Ok(RefinedAdmission::CompositeV22),
        MULTIFILE_PROFILE_V26 => Ok(RefinedAdmission::CompositeV23),
        MULTIFILE_PROFILE_V27 => Ok(RefinedAdmission::CompositeV24),
        MULTIFILE_PROFILE_V28 => Ok(RefinedAdmission::CompositeV25),
        MULTIFILE_PROFILE_V29 => Ok(RefinedAdmission::CompositeV26),
        MULTIFILE_PROFILE_V30 => Ok(RefinedAdmission::CompositeV29),
        _ => Err(BundleError("unsupported schema, scope or settings")),
    }
}
/// Bundle subdirectory holding the pinned original member texts.
pub const SOURCES_DIR: &str = "sources";
pub const MAX_SOURCE_FILES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceRecheck {
    RecheckedWithinProfile,
    Refuted,
    Inconclusive,
}

/// Recheck an exact source snapshot under C03 after the existing identity gates.
/// No project configuration, proof cache, orchestration environment or tools are read.
pub fn recheck_source(
    record_bytes: &[u8],
    expected_id: &str,
    source: &str,
    files: &BTreeMap<BundleFile, Vec<u8>>,
) -> Result<SourceRecheck, BundleError> {
    use z3::{Config, Context, Solver};
    load_pinned_record(record_bytes, expected_id)?;
    let mut config = Config::new();
    config.set_timeout_msec(BuildSettings::default().solver_timeout_ms);
    config.set_param_value("rlimit", &RECHECK_RESOURCE_LIMIT.to_string());
    let context = Context::new(&config);
    let obligation = semantic_contract::encode_pure_i64_obligation(source, &context)
        .map_err(|_| BundleError("source outside bounded C03 recheck profile"))?;
    // This also enforces zero-argument main and a literal true precondition.
    check_identity(record_bytes, expected_id, source, files)?;
    let solver = Solver::new(&context);
    solver.assert(&obligation);
    Ok(source_recheck_result(solver.check()))
}

fn source_recheck_result(result: z3::SatResult) -> SourceRecheck {
    match result {
        z3::SatResult::Unsat => SourceRecheck::RecheckedWithinProfile,
        z3::SatResult::Sat => SourceRecheck::Refuted,
        z3::SatResult::Unknown => SourceRecheck::Inconclusive,
    }
}

#[derive(Debug, thiserror::Error)]
#[error("native bundle rejected: {0}")]
pub struct BundleError(pub &'static str);

pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleFile {
    LlvmIr,
    Object,
    Certificate,
    Executable,
}

impl BundleFile {
    pub const ALL: [Self; 4] = [
        Self::LlvmIr,
        Self::Object,
        Self::Certificate,
        Self::Executable,
    ];
    pub fn file_name(self) -> &'static str {
        match self {
            Self::LlvmIr => "program.ll",
            Self::Object => "program.o",
            Self::Certificate => "proof-cert.json",
            Self::Executable => "program",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentIdentity {
    pub sha256: String,
    pub bytes: u64,
}

impl ContentIdentity {
    pub fn of(bytes: &[u8]) -> Self {
        Self {
            sha256: sha256(bytes),
            bytes: bytes.len() as u64,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceBinding {
    pub source_sha256: String,
    pub source_profile: String,
    pub specification_sha256: String,
    pub implementation_sha256: String,
    pub atom_content_hash: String,
    pub hir_encoding: String,
    pub hir_sha256: String,
    pub mir_encoding: String,
    pub mir_sha256: String,
    pub translator_ir_sha256: String,
    pub translator_version: String,
    pub bridge_lemma_hash: String,
    pub compiler_version: String,
    pub source_dependencies_sha256: String,
}

pub fn prepare_source(source: &str) -> Result<(SourceBinding, parser::Atom), BundleError> {
    let mut contract =
        semantic_contract::extract_pure_i64_contract(source, SOURCE_LABEL, None, false)
            .map_err(|_| BundleError("unsupported or invalid source"))?;
    // The native-bundle profile declares the object-stage relations on
    // its contracts; plain `verify` extractions keep the six originals.
    contract.emit_native_object_transformations();
    contract.emit_native_hir_llvm_transformation();
    let mut items =
        parser::parse_module_checked(source).map_err(|_| BundleError("invalid source"))?;
    let Some(parser::Item::Atom(mut atom)) = items.pop() else {
        return Err(BundleError("expected main atom"));
    };
    if atom.name != "main" || !atom.params.is_empty() || atom.requires.trim() != "true" {
        return Err(BundleError(
            "profile requires zero-argument main with literal true precondition",
        ));
    }
    atom.span.file = SOURCE_LABEL.to_owned();
    Ok((
        SourceBinding {
            source_sha256: contract.source_sha256,
            source_profile: contract.profile.to_owned(),
            specification_sha256: contract.specification_sha256,
            implementation_sha256: contract.implementation_sha256,
            atom_content_hash: contract.atom_content_hash,
            hir_encoding: contract.hir.encoding.to_owned(),
            hir_sha256: contract.hir.sha256,
            mir_encoding: contract.mir.encoding.to_owned(),
            mir_sha256: contract.mir.sha256,
            translator_ir_sha256: sha256(
                &serde_json::to_vec(&contract.translator_ir)
                    .map_err(|_| BundleError("IR encoding failed"))?,
            ),
            translator_version: contract.translator_version.to_owned(),
            bridge_lemma_hash: contract.bridge_lemma_hash.to_owned(),
            compiler_version: contract.compiler_version.to_owned(),
            source_dependencies_sha256: source_dependencies_sha256(std::slice::from_ref(&atom))?,
        },
        atom,
    ))
}

/// Pin the intra-source dependency map the same way the Claim Book records
/// it: every declared atom maps to its sorted direct callees via
/// `resolver::collect_callees_from_atom`, and `BTreeMap` keeps the JSON
/// encoding canonical. This is a call-graph pin, not a file-level import
/// closure — the restricted profiles admit no `import` items.
fn source_dependencies_sha256(atoms: &[parser::Atom]) -> Result<String, BundleError> {
    let dependencies: BTreeMap<String, Vec<String>> = atoms
        .iter()
        .map(|atom| {
            let mut callees: Vec<String> = crate::resolver::collect_callees_from_atom(atom)
                .into_iter()
                .collect();
            callees.sort();
            (atom.name.clone(), callees)
        })
        .collect();
    Ok(sha256(
        &serde_json::to_vec(&dependencies)
            .map_err(|_| BundleError("dependency encoding failed"))?,
    ))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectBuild {
    pub target_triple: String,
    pub data_layout: String,
    pub llvm_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkBuild {
    pub driver: String,
    pub driver_version: String,
    pub driver_target: String,
    pub linker: String,
    pub linker_version: String,
    pub arguments_sha256: String,
    /// SHA-256 of the embedded runtime C source when it was linked.
    /// Optional so records that did not link the runtime remain unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_source_sha256: Option<String>,
    /// sha256 of the argv with every path-dependent argument replaced by
    /// a stable placeholder (`{output}`, `{input:i}`, `{toolchain}`) —
    /// reproducible by a recheck on fresh staging paths, unlike the raw
    /// vector. Optional so records pinned before the field existed still
    /// deserialize; a recheck skips the comparison when it is absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_arguments_sha256: Option<String>,
    /// OS exit status observed by running the produced executable once at
    /// build time (no arguments, closed stdio). Optional so records pinned
    /// before the field existed still deserialize; a recheck that executes
    /// skips the comparison when it is absent — and it is absent whenever
    /// the build could not observe an exit code (spawn failure, timeout or
    /// signal termination rather than a 0..=255 code).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_exit_status: Option<u32>,
    /// SHA-256 hex of the stdout bytes observed by running the produced
    /// executable once at build time (no arguments, closed stdin, drained
    /// stdout capped at 64 KiB). Optional so records pinned before the field
    /// existed still deserialize; a recheck that executes skips the
    /// comparison when it is absent — and it is absent whenever the build
    /// could not observe the channel (spawn failure, timeout, signal
    /// termination or output over the capture cap).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_stdout_sha256: Option<String>,
    /// SHA-256 hex of the stderr bytes observed by running the produced
    /// executable once at build time under the same conditions as the
    /// stdout pin. Optional so records pinned before the field existed
    /// still deserialize; a recheck that executes skips the comparison
    /// when it is absent — and it is absent whenever the build could not
    /// observe the channel (spawn failure, timeout, signal termination
    /// or output over the capture cap).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_stderr_sha256: Option<String>,
    /// Spawn context the `expected_*` pins were observed under. Optional so
    /// records pinned before the field existed still deserialize; absent
    /// whenever no observation pin was observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation_envelope: Option<ObservationEnvelope>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationEnvelope {
    pub environment: String,
    pub working_directory: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildSettings {
    pub llvm_optimization: String,
    pub cpu: String,
    pub features: String,
    pub relocation: String,
    pub code_model: String,
    pub link_optimization: String,
    pub link_libraries: Vec<String>,
    pub runtime: String,
    pub entry_abi: String,
    pub project_config: String,
    pub proof_cache: String,
    pub solver_timeout_ms: u64,
}

pub const RUNTIME_SYSTEM_CRT_ONLY: &str = "system-crt-only/no-mumei-runtime";
pub const RUNTIME_SYSTEM_CRT_WITH_EMBEDDED_MUMEI_C: &str = "system-crt+embedded-mumei-runtime-c";

impl BuildSettings {
    pub fn expected_for(link_build: &LinkBuild) -> Self {
        let mut settings = Self::default();
        if link_build.runtime_source_sha256.is_some() {
            settings.runtime = RUNTIME_SYSTEM_CRT_WITH_EMBEDDED_MUMEI_C.to_owned();
        }
        settings
    }
}

#[cfg(test)]
mod observation_envelope_tests {
    use super::*;
    use serde_json::{json, Value};

    fn legacy_link_build() -> Value {
        json!({
            "driver": "cc",
            "driver_version": "1",
            "driver_target": "x86_64-linux",
            "linker": "gnu-ld",
            "linker_version": "1",
            "arguments_sha256": "args",
        })
    }

    #[test]
    fn observation_envelope_is_optional_and_omitted_when_absent() {
        let value = legacy_link_build();
        let link_build: LinkBuild = serde_json::from_value(value).unwrap();
        assert_eq!(link_build.observation_envelope, None);

        let serialized = serde_json::to_value(link_build).unwrap();
        assert!(serialized.get("observation_envelope").is_none());
    }

    #[test]
    fn expected_settings_follow_the_recorded_runtime_source() {
        let mut link_build: LinkBuild = serde_json::from_value(legacy_link_build()).unwrap();
        assert_eq!(
            BuildSettings::expected_for(&link_build).runtime,
            RUNTIME_SYSTEM_CRT_ONLY
        );

        link_build.runtime_source_sha256 = Some("runtime-hash".into());
        assert_eq!(
            BuildSettings::expected_for(&link_build).runtime,
            RUNTIME_SYSTEM_CRT_WITH_EMBEDDED_MUMEI_C
        );
    }

    #[test]
    fn observation_envelope_roundtrips_and_rejects_unknown_fields() {
        let mut value = legacy_link_build();
        value["observation_envelope"] = json!({
            "environment": "empty",
            "working_directory": "executable_directory",
        });
        let link_build: LinkBuild = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            link_build.observation_envelope,
            Some(ObservationEnvelope {
                environment: "empty".into(),
                working_directory: "executable_directory".into(),
            })
        );
        assert_eq!(serde_json::to_value(link_build).unwrap(), value);

        value["observation_envelope"]["unexpected"] = json!(true);
        assert!(serde_json::from_value::<LinkBuild>(value).is_err());
    }
}

impl Default for BuildSettings {
    fn default() -> Self {
        Self {
            llvm_optimization: "default".into(),
            cpu: "generic".into(),
            features: String::new(),
            relocation: "pic".into(),
            code_model: "default".into(),
            link_optimization: "O2".into(),
            link_libraries: ["m", "pthread", "dl", "z3"].map(str::to_owned).to_vec(),
            runtime: RUNTIME_SYSTEM_CRT_ONLY.into(),
            entry_abi: "C-main-i32-from-i64".into(),
            project_config: "not-loaded/fixed-profile".into(),
            proof_cache: "disabled/fresh-z3".into(),
            solver_timeout_ms: verification::VerificationConfig::default().timeout_ms,
        }
    }
}

/// Per-atom contract identities inside a module bundle record — the same
/// fields `SourceBinding` carries for one atom, keyed by atom name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleAtomBinding {
    pub atom_content_hash: String,
    pub specification_sha256: String,
    pub implementation_sha256: String,
    pub hir_sha256: String,
    pub mir_sha256: String,
    pub translator_ir_sha256: String,
}

/// The module-level identity half of a `SourceBinding` — every
/// `ModuleAtomBinding` shares the whole-module `source_sha256`, so atom
/// sets cannot be shuffled into a different source snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleSourceBinding {
    pub source_sha256: String,
    pub source_profile: String,
    pub atoms: BTreeMap<String, ModuleAtomBinding>,
    pub hir_encoding: String,
    pub mir_encoding: String,
    pub translator_version: String,
    pub bridge_lemma_hash: String,
    pub compiler_version: String,
    pub source_dependencies_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleNativeBundleRecord {
    pub schema: String,
    pub profile: String,
    pub scope: String,
    pub system_scope: String,
    pub source: ModuleSourceBinding,
    pub object_build: ObjectBuild,
    pub link_build: LinkBuild,
    pub settings: BuildSettings,
    pub files: BTreeMap<BundleFile, ContentIdentity>,
    /// v24: per-atom declared leaf widths (`64` scalar/enum, `128`
    /// wide) in signature order — pins the two-slot pairing
    /// `pure-i64-composite/v21` modules carry. Absent/empty on older
    /// profiles, required non-empty under v24.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub leaf_widths: BTreeMap<String, Vec<u64>>,
}

/// The file-level pin half of a multifile record: which member ordered
/// the merge as root, and the pinned import graph over member names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceFilesPin {
    /// Member name of the root file — the file whose `import`
    /// declarations ordered the canonical merge.
    pub root: String,
    /// SHA-256 of the canonical JSON `{member: [direct imports, sorted]}`
    /// — every member appears; imported members map to an empty list
    /// because this slice admits no nested imports.
    pub import_graph_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MultifileNativeBundleRecord {
    pub schema: String,
    pub profile: String,
    pub scope: String,
    pub system_scope: String,
    /// Binding over the canonical merged text — the same shape and
    /// conventions as the module binding, with
    /// `source_dependencies_sha256` pinning the merged atom call graph.
    pub source: ModuleSourceBinding,
    pub source_files: SourceFilesPin,
    pub object_build: ObjectBuild,
    pub link_build: LinkBuild,
    pub settings: BuildSettings,
    pub files: BTreeMap<BundleFile, ContentIdentity>,
    /// Original member texts by member name (`app.mm`, `helpers.mm`),
    /// each pinned to the bytes the merge consumed — a consumer re-derives
    /// the merged text from these pins instead of trusting a filesystem.
    pub sources: BTreeMap<String, ContentIdentity>,
    /// v24: per-atom declared leaf widths — same semantics as the
    /// module record's `leaf_widths`, over the merged text.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub leaf_widths: BTreeMap<String, Vec<u64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeBundleRecord {
    pub schema: String,
    pub profile: String,
    pub scope: String,
    pub system_scope: String,
    pub source: SourceBinding,
    pub object_build: ObjectBuild,
    pub link_build: LinkBuild,
    pub settings: BuildSettings,
    pub files: BTreeMap<BundleFile, ContentIdentity>,
}

pub fn load_pinned_record(
    bytes: &[u8],
    expected_id: &str,
) -> Result<NativeBundleRecord, BundleError> {
    if !is_digest(expected_id) || sha256(bytes) != expected_id {
        return Err(BundleError("build identity mismatch"));
    }
    let record: NativeBundleRecord =
        serde_json::from_slice(bytes).map_err(|_| BundleError("invalid record"))?;
    if record.schema != SCHEMA
        || record.profile != PROFILE
        || record.scope != SCOPE
        || record.system_scope != SYSTEM_SCOPE
        || record.settings != BuildSettings::expected_for(&record.link_build)
        || record.files.len() != BundleFile::ALL.len()
        || BundleFile::ALL
            .iter()
            .any(|role| !record.files.contains_key(role))
    {
        return Err(BundleError("unsupported schema, scope or settings"));
    }
    if !record.object_build.target_triple.starts_with("x86_64-")
        || !record.object_build.target_triple.contains("linux")
        || record.object_build.data_layout.is_empty()
        || !record.object_build.llvm_version.starts_with("17.")
        || !record.link_build.driver_target.starts_with("x86_64-")
        || !record.link_build.driver_target.contains("linux")
        || !["cc", "gcc", "clang", "clang-17"].contains(&record.link_build.driver.as_str())
        || !["gnu-ld", "lld"].contains(&record.link_build.linker.as_str())
        || !is_digest(&record.link_build.arguments_sha256)
        || record
            .link_build
            .canonical_arguments_sha256
            .as_deref()
            .is_some_and(|pin| !is_digest(pin))
        || [
            &record.object_build.llvm_version,
            &record.link_build.driver_version,
            &record.link_build.linker_version,
        ]
        .iter()
        .any(|value| !numeric_version(value))
    {
        return Err(BundleError("unsupported toolchain observation"));
    }
    Ok(record)
}

fn numeric_version(value: &str) -> bool {
    (2..=4).contains(&value.split('.').count())
        && value
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

/// Compare snapshots to an externally supplied digest, not a proof or authenticity verdict.
pub fn check_identity(
    record_bytes: &[u8],
    expected_id: &str,
    source: &str,
    files: &BTreeMap<BundleFile, Vec<u8>>,
) -> Result<(), BundleError> {
    let record = load_pinned_record(record_bytes, expected_id)?;
    let (binding, atom) = prepare_source(source)?;
    if record.source != binding {
        return Err(BundleError("source or compiler identity mismatch"));
    }
    if files.len() != BundleFile::ALL.len()
        || BundleFile::ALL.iter().any(|role| {
            files
                .get(role)
                .is_none_or(|data| record.files.get(role) != Some(&ContentIdentity::of(data)))
        })
    {
        return Err(BundleError("missing or replaced bundle component"));
    }
    let cert: proof_cert::ProofCertificate =
        serde_json::from_slice(&files[&BundleFile::Certificate])
            .map_err(|_| BundleError("invalid certificate"))?;
    if !matches!(
        proof_cert::check_certificate_hash(&cert),
        proof_cert::CertificateHashCheck::Match
    ) || cert.atoms.len() != 1
        || cert.mumei_version != binding.compiler_version
        || proof_cert::validate_certificate_translator_versions(&cert).is_err()
        || cert.atoms[0].name != "main"
        || cert.atoms[0].content_hash != binding.atom_content_hash
        || cert.atoms[0].requires != atom.requires
        || cert.atoms[0].ensures != atom.ensures
        || cert.atoms[0].body_expr != atom.body_expr
        || !cert.atoms[0].assumed_clauses.is_empty()
        || !cert.atoms[0].forall_constraints.is_empty()
        || cert.atoms[0].effects
            != atom
                .effects
                .iter()
                .map(|effect| effect.name.clone())
                .collect::<Vec<_>>()
        || sha256(
            &serde_json::to_vec(&cert.atoms[0].translator_ir)
                .map_err(|_| BundleError("IR encoding failed"))?,
        ) != binding.translator_ir_sha256
        || cert.atoms[0].status != proof_cert::status::VERIFIED
        || cert.atoms[0].z3_check_result != proof_cert::status::Z3_UNSAT
        || cert.atoms[0].reused_from_store
        || !cert.atoms[0].dependencies.is_empty()
        || proof_cert::verify_certificate(&cert, &[&atom], false)
            .iter()
            .any(|(_, status)| status != proof_cert::status::CERTIFICATE_PROVEN)
    {
        return Err(BundleError("certificate binding mismatch"));
    }
    Ok(())
}

/// Parse and admit a multi-atom module under the widened wrapping profile:
/// every item is an atom satisfying the per-atom gates — plus, when
/// `admission` permits, refined scalar types, structs, enums and simple
/// stateful effects — and exactly one atom is `main`, zero-argument with
/// a literal `true` precondition, matching the profile the C `main` wrapper attests.
/// Returns the declared refined types and structs so the build path can
/// register them with the verifier's environment the same way
/// `mumei verify` does.
pub fn prepare_module_source(
    source: &str,
    admission: RefinedAdmission,
) -> Result<ModuleSourceParts, BundleError> {
    let mut contracts =
        semantic_contract::extract_pure_i64_module_contracts(source, SOURCE_LABEL, None, admission)
            .map_err(|_| BundleError("unsupported or invalid module source"))?;
    // Same profile-conditional relation emit as `prepare_source`.
    for contract in &mut contracts {
        contract.emit_native_object_transformations();
        contract.emit_native_hir_llvm_transformation();
    }
    let mut items =
        parser::parse_module_checked(source).map_err(|_| BundleError("invalid source"))?;
    // Under composite v9 the admission above instantiated every generic
    // struct declaration — the emit inputs must see the same concrete
    // declarations the verifier admitted.
    if items
        .iter()
        .any(|item| matches!(item, parser::Item::StructDef(def) if !def.type_params.is_empty()))
    {
        items = semantic_contract::monomorphize_generic_structs(items)
            .map_err(|_| BundleError("invalid source"))?;
    }
    let mut atoms = Vec::with_capacity(items.len());
    let mut types = Vec::new();
    let mut structs = Vec::new();
    let mut enums = Vec::new();
    let mut mains = 0usize;
    for item in items.drain(..) {
        match item {
            parser::Item::Atom(mut atom) => {
                if atom.name == "main" {
                    mains += 1;
                    // The C `main` wrapper attests an i64 user body — the widened
                    // profile admits other scalar returns for siblings, but the
                    // entry point keeps the single-atom `-> i64` signature.
                    if !atom.params.is_empty()
                        || atom.requires.trim() != "true"
                        || atom.return_type.as_deref() != Some("i64")
                    {
                        return Err(BundleError(
                            "profile requires zero-argument i64 main with literal true precondition",
                        ));
                    }
                }
                atom.span.file = SOURCE_LABEL.to_owned();
                atoms.push(atom);
            }
            parser::Item::TypeDef(typedef) => types.push(typedef),
            parser::Item::StructDef(struct_def) => structs.push(struct_def),
            parser::Item::EnumDef(enum_def) => enums.push(enum_def),
            parser::Item::EffectDef(_) if admission.admits_effects() => (),
            _ => return Err(BundleError("non-atom declarations")),
        }
    }
    if mains != 1 {
        return Err(BundleError("profile requires exactly one main atom"));
    }
    let mut bindings = BTreeMap::new();
    for contract in &contracts {
        bindings.insert(
            contract.atom.clone(),
            ModuleAtomBinding {
                atom_content_hash: contract.atom_content_hash.clone(),
                specification_sha256: contract.specification_sha256.clone(),
                implementation_sha256: contract.implementation_sha256.clone(),
                hir_sha256: contract.hir.sha256.clone(),
                mir_sha256: contract.mir.sha256.clone(),
                translator_ir_sha256: sha256(
                    &serde_json::to_vec(&contract.translator_ir)
                        .map_err(|_| BundleError("IR encoding failed"))?,
                ),
            },
        );
    }
    let first = contracts
        .first()
        .ok_or(BundleError("expected at least one atom"))?;
    Ok((
        ModuleSourceBinding {
            source_sha256: first.source_sha256.clone(),
            source_profile: first.profile.to_owned(),
            atoms: bindings,
            hir_encoding: first.hir.encoding.to_owned(),
            mir_encoding: first.mir.encoding.to_owned(),
            translator_version: first.translator_version.to_owned(),
            bridge_lemma_hash: first.bridge_lemma_hash.to_owned(),
            compiler_version: first.compiler_version.to_owned(),
            source_dependencies_sha256: source_dependencies_sha256(&atoms)?,
        },
        atoms,
        types,
        structs,
        enums,
    ))
}

/// The module derivation's products: the source binding, the admitted
/// atoms and the declared refined types, structs and enums.
pub type ModuleSourceParts = (
    ModuleSourceBinding,
    Vec<parser::Atom>,
    Vec<parser::RefinedType>,
    Vec<parser::StructDef>,
    Vec<parser::EnumDef>,
);

/// Normalize an import path or member key to its `<dir>/<stem>.mm`
/// member name under the member-path grammar: `/`-separated nonempty
/// segments, each free of `\\`, `:` and a leading dot, the last with a
/// nonempty stem. Absolute paths and `..`/`.`/leading-dot segments all
/// fail the grammar honestly (`std/` is an ordinary subdirectory —
/// the language resolver tries local files before the toolchain std
/// fallback). Members resolve file-relative: the key for an import in
/// `sub/b.mm` is `sub/` + the normalized path (see `resolve_member`).
pub fn member_name(path: &str) -> Result<String, BundleError> {
    const MAX_MEMBER_SEGMENTS: usize = 16;
    let stem = path.strip_suffix(".mm").unwrap_or(path);
    let bad = stem.is_empty()
        || stem.split('/').count() > MAX_MEMBER_SEGMENTS
        || stem.split('/').any(|seg| {
            seg.is_empty() || seg.starts_with('.') || seg.bytes().any(|b| matches!(b, b'\\' | b':'))
        });
    if bad {
        return Err(BundleError("import outside member-path profile"));
    }
    Ok(format!("{stem}.mm"))
}

/// Resolve an import declared inside `importer` to its member key.
/// Imports are file-relative (the language's resolver uses the
/// importing file's directory), so the importer's own directory
/// prefix joins in front; `..` is outside the grammar, so a dep can
/// never escape the importing directory's subtree.
pub fn resolve_member(importer: &str, path: &str) -> Result<String, BundleError> {
    let prefix = importer
        .rsplit_once('/')
        .map(|(dir, _)| format!("{dir}/"))
        .unwrap_or_default();
    // The composed path goes through the grammar so producer and
    // consumer agree on every bound — segment count included — for the
    // full key, not just the import's own suffix.
    member_name(&format!("{prefix}{path}"))
}

/// The qualifier an unaliased import binds: the LAST segment's stem,
/// so `import "sub/dep"` introduces `dep` (not `sub/dep`).
/// `trim_end_matches` (not a single `strip_suffix`) keeps the result a
/// spellable identifier: `dep.mm.mm` qualifies as `dep` since a
/// `.`-containing qualifier could never appear in source anyway.
fn member_stem(member: &str) -> &str {
    member
        .rsplit('/')
        .next()
        .unwrap_or(member)
        .trim_end_matches(".mm")
}

/// Derive the canonical merged text for an import closure: a
/// post-order depth-first walk from the root over the pinned member
/// map emits each member after its own imports (dependencies first,
/// siblings in declaration order), so a diamond member emits once at
/// its first visit and cycles fail honestly. Each member's own import
/// lines are stripped and its own import qualifiers normalized to bare
/// callee names — a member's namespace is file-local, matching the
/// language's resolved ModuleEnv. Returns the merged text and the full
/// `{member: [sorted imports]}` graph. Determinism comes from the
/// pinned member map, so a recheck re-derives byte-identical text.
pub fn merge_import_closure(
    root_name: &str,
    sources: &BTreeMap<String, String>,
    admission: RefinedAdmission,
) -> Result<(String, BTreeMap<String, Vec<String>>), BundleError> {
    if !sources.contains_key(root_name) {
        return Err(BundleError("root source member not pinned"));
    }
    let mut state = MergeWalk {
        admission,
        visiting: std::collections::BTreeSet::new(),
        emitted: std::collections::BTreeSet::new(),
        emitted_hashes: std::collections::BTreeSet::new(),
        graph: BTreeMap::new(),
        parts: Vec::new(),
    };
    visit_member(root_name, sources, &mut state)?;
    if state.graph[root_name].is_empty() {
        return Err(BundleError("expected at least one import"));
    }
    Ok((format!("{}\n", state.parts.join("\n")), state.graph))
}

/// The walk state shared across the DFS: the refined-members gate plus
/// the cycle-detection set, the emitted member set and dedup hashes,
/// and the accumulated graph/parts.
struct MergeWalk {
    admission: RefinedAdmission,
    visiting: std::collections::BTreeSet<String>,
    emitted: std::collections::BTreeSet<String>,
    emitted_hashes: std::collections::BTreeSet<String>,
    graph: BTreeMap<String, Vec<String>>,
    parts: Vec<String>,
}

/// One DFS node: gather the member's imports under the same gates as
/// the root (single-line strict grammar, no duplicates, no
/// self-import, distinct qualifiers), recurse into them, then emit the
/// member's text with its import lines removed and its own qualifiers
/// stripped.
fn visit_member(
    member: &str,
    sources: &BTreeMap<String, String>,
    state: &mut MergeWalk,
) -> Result<(), BundleError> {
    if state.emitted.contains(member) {
        return Ok(());
    }
    if !state.visiting.insert(member.to_owned()) {
        return Err(BundleError("circular import outside profile"));
    }
    let text = sources
        .get(member)
        .ok_or(BundleError("imported source member not pinned"))?;
    let (imports, qualifiers, import_lines) = member_imports(member, text, state.admission)?;
    let mut sorted = imports.clone();
    sorted.sort();
    state.graph.insert(member.to_owned(), sorted);
    for dep in &imports {
        visit_member(dep, sources, state)?;
    }
    state.visiting.remove(member);
    // Qualified calls are namespace syntax, not part of the atom
    // contract: the callee map keys on bare names, so `h::helper(`
    // merges as `helper(` — the same normalization the language's own
    // resolved ModuleEnv performs.
    // A member with no import lines contributes its text verbatim —
    // normalizing through `lines().join("\n")` would rewrite CRLF
    // endings and change the merged bytes slice-1 bundles pinned.
    let mut body = if import_lines.is_empty() {
        text.to_owned()
    } else {
        text.lines()
            .enumerate()
            .filter(|(n, _)| !import_lines.contains(&(n + 1)))
            .map(|(_, line)| line)
            .collect::<Vec<_>>()
            .join("\n")
    };
    for qualifier in &qualifiers {
        body = strip_qualifier(&body, qualifier);
    }
    let body = body.trim_end().to_owned();
    // Two member keys can pin the same bytes — one real file reached
    // by two importers through the fallback chain (e.g. `import
    // "std/x"` from root and from `sub/dep.mm`, both falling back to
    // the shipped stdlib). The language loads a resolved path once
    // and ModuleEnv registration folds same-name atoms silently, so
    // emitting the body twice would only trip the merged module's
    // duplicate-atom gate: emit identical bodies once. Distinct
    // member texts still emit side by side and keep that gate.
    if state.emitted_hashes.insert(sha256(body.as_bytes())) {
        state.parts.push(body);
    }
    state.emitted.insert(member.to_owned());
    Ok(())
}

/// A member's gated imports: member keys in declaration order, the
/// qualifier each brings into scope, and the import lines to strip.
type MemberImports = (Vec<String>, Vec<String>, std::collections::BTreeSet<usize>);

/// Parse a member's `import` items and gate them: member keys resolved
/// file-relative and pinned in `sources`, no duplicate imports, no
/// self-import (post-resolution, so `sub/a.mm` importing `"a"` counts),
/// distinct qualifiers (alias or stem — two members bound to the same
/// namespace sugar would strip to the same bare-name space
/// ambiguously), atoms plus the declarations the pinned admission
/// permits, and every import
/// alone on its line as one strict `import "..." [as x];` statement
/// (a trailing `//` comment is trivia).
fn member_imports(
    member_key: &str,
    text: &str,
    admission: RefinedAdmission,
) -> Result<MemberImports, BundleError> {
    // `parse_module` (not the checked entry point) because the checked
    // pipeline consumes `import` items; whatever the lenient scan cannot
    // see stays in the text and fails the merged admission downstream.
    let items = parser::parse_module(text);
    let mut imports = Vec::new();
    let mut qualifiers = Vec::new();
    let mut seen_qualifiers = std::collections::BTreeSet::new();
    let mut import_lines = std::collections::BTreeSet::new();
    for item in &items {
        match item {
            parser::Item::Import(decl) => {
                let member = resolve_member(member_key, &decl.path)?;
                if member == member_key {
                    return Err(BundleError("self-import outside profile"));
                }
                if imports.contains(&member) {
                    return Err(BundleError("duplicate import"));
                }
                let qualifier = decl
                    .alias
                    .clone()
                    .unwrap_or_else(|| member_stem(&member).to_owned());
                if !seen_qualifiers.insert(qualifier.clone()) {
                    return Err(BundleError("duplicate import qualifier outside profile"));
                }
                qualifiers.push(qualifier);
                imports.push(member);
                import_lines.insert(decl.span.line);
            }
            parser::Item::Atom(_) => (),
            // Refined scalar and struct declarations ride the merge only
            // when the pinned profile admits them — the merged admission
            // gate still validates each one.
            parser::Item::TypeDef(_) if admission.admits_typedefs() => (),
            parser::Item::StructDef(_) if admission.admits_structs() => (),
            parser::Item::EnumDef(_) if admission.admits_enums() => (),
            parser::Item::EffectDef(_) if admission.admits_effects() => (),
            _ => return Err(BundleError("declaration outside multifile profile")),
        }
    }
    // Every import must sit alone on its line so stripping cannot leave
    // residue: the line the decl's span names must parse as exactly one
    // `import "..." [as x];` statement — an `import "a"; atom f()`
    // line would silently drop `f` from the merge otherwise.
    for line in &import_lines {
        let text = text
            .lines()
            .nth(line.saturating_sub(1))
            .map(str::trim)
            .unwrap_or_default();
        // A trailing `//` comment is trivia — judge the strict grammar
        // on the code before it. `//` inside the quoted member name
        // would already fail the member-name grammar downstream.
        let code = text.split("//").next().unwrap_or_default().trim_end();
        let items = parser::parse_module(code);
        // The lenient parser tolerates a missing `;`, but the profile
        // requires the strict statement form so a malformed member
        // cannot ride the merge into a verified bundle.
        if items.len() != 1 || !matches!(items[0], parser::Item::Import(_)) || !code.ends_with(';')
        {
            return Err(BundleError("multiline or embedded import outside profile"));
        }
    }
    Ok((imports, qualifiers, import_lines))
}

/// Remove every `<qualifier> <ws>* :: <ws>*` prefix where the qualifier
/// sits at an identifier boundary and the `::` introduces a callee name.
/// Boundary rules treat every non-ASCII byte as an identifier byte, so
/// `ph::f`, `xhelpers::f` and `éh::f` all survive a `h` qualifier; the
/// whitespace-tolerant match mirrors the parser, which accepts
/// `h :: helper` as the same qualified call as `h::helper`. A `::` not
/// followed by an identifier start is left alone so malformed syntax
/// still reaches merged admission and fails closed.
fn strip_qualifier(text: &str, qualifier: &str) -> String {
    fn is_ident_byte(byte: u8) -> bool {
        !byte.is_ascii() || byte.is_ascii_alphanumeric() || byte == b'_'
    }
    fn is_ident_start(byte: u8) -> bool {
        !byte.is_ascii() || byte.is_ascii_alphabetic() || byte == b'_'
    }
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let ws = |c: char| c.is_ascii_whitespace();
    // `i` is the copy cursor and `j` the scan cursor: a non-qualifying
    // occurrence advances only `j`, keeping `i` behind so the text up to
    // it is still emitted, while the boundary check sees the byte that
    // precedes the match in the original text.
    let mut i = 0;
    let mut j = 0;
    while let Some(rel) = text[j..].find(qualifier) {
        let pos = j + rel;
        // A match inside a `//` comment is comment text, not a call:
        // stripping it would splice the line's tail onto the next line
        // (`// h ::` before `atom f` becomes `// atom f`) and erase the
        // following declaration.
        let line_start = text[..pos].rfind('\n').map_or(0, |n| n + 1);
        if text[line_start..pos].contains("//") {
            j = pos + qualifier.len();
            continue;
        }
        let boundary = pos == 0 || !is_ident_byte(bytes[pos - 1]);
        let after = &text[pos + qualifier.len()..];
        let tail = after.trim_start_matches(ws);
        // `<qualifier> ws* :: ws* <ident-start>` — the ident-start keeps
        // `q :: 5` malformed syntax on its way to merged admission.
        let ws2 = (boundary && tail.starts_with("::"))
            .then(|| &tail["::".len()..])
            .and_then(|t| {
                let callee = t.trim_start_matches(ws);
                callee
                    .as_bytes()
                    .first()
                    .is_some_and(|b| is_ident_start(*b))
                    .then(|| t.len() - callee.len())
            });
        if let Some(ws2) = ws2 {
            let ws1 = after.len() - tail.len();
            out.push_str(&text[i..pos]);
            i = pos + qualifier.len() + ws1 + "::".len() + ws2;
            j = i;
        } else {
            j = pos + qualifier.len();
        }
    }
    out.push_str(&text[i..]);
    out
}

/// Pin the file-level import graph under the same canonical-JSON
/// convention the atom-level `source_dependencies_sha256` pin uses:
/// `{member: [sorted direct imports]}` for every member in the
/// closure.
pub fn import_graph_sha256(graph: &BTreeMap<String, Vec<String>>) -> String {
    sha256(&serde_json::to_vec(graph).expect("member names serialize"))
}

/// The full multifile derivation's products: the merged text's per-atom
/// binding, the file-graph pin, the admitted atoms, declarations and
/// canonical merged source itself, so
/// emit/recheck feed the same bytes the binding describes.
pub struct PreparedMultifileSource {
    pub binding: ModuleSourceBinding,
    pub import_graph_sha256: String,
    pub atoms: Vec<parser::Atom>,
    pub types: Vec<parser::RefinedType>,
    pub structs: Vec<parser::StructDef>,
    pub enums: Vec<parser::EnumDef>,
    pub merged: String,
}

/// Merge the import closure and admit the merged text under the module
/// gate (atom items plus, under `admission`, refined `type`, `struct`,
/// `enum` and simple stateful `effect` declarations).
pub fn prepare_multifile_source(
    root_name: &str,
    sources: &BTreeMap<String, String>,
    admission: RefinedAdmission,
) -> Result<PreparedMultifileSource, BundleError> {
    let (merged, graph) = merge_import_closure(root_name, sources, admission)?;
    let (binding, atoms, types, structs, enums) = prepare_module_source(&merged, admission)?;
    Ok(PreparedMultifileSource {
        binding,
        import_graph_sha256: import_graph_sha256(&graph),
        atoms,
        types,
        structs,
        enums,
        merged,
    })
}

/// Pin a module record the same way `load_pinned_record` pins a
/// single-atom one — the schema/profile/scope/settings gates differ only
/// in the constants they bind.
pub fn load_pinned_module_record(
    bytes: &[u8],
    expected_id: &str,
) -> Result<ModuleNativeBundleRecord, BundleError> {
    if !is_digest(expected_id) || sha256(bytes) != expected_id {
        return Err(BundleError("build identity mismatch"));
    }
    let record: ModuleNativeBundleRecord =
        serde_json::from_slice(bytes).map_err(|_| BundleError("invalid record"))?;
    if record.schema != MODULE_SCHEMA
        || ![
            MODULE_PROFILE,
            MODULE_PROFILE_V2,
            MODULE_PROFILE_V3,
            MODULE_PROFILE_V4,
            MODULE_PROFILE_V5,
            MODULE_PROFILE_V6,
            MODULE_PROFILE_V7,
            MODULE_PROFILE_V8,
            MODULE_PROFILE_V9,
            MODULE_PROFILE_V10,
            MODULE_PROFILE_V11,
            MODULE_PROFILE_V12,
            MODULE_PROFILE_V13,
            MODULE_PROFILE_V14,
            MODULE_PROFILE_V15,
            MODULE_PROFILE_V16,
            MODULE_PROFILE_V17,
            MODULE_PROFILE_V18,
            MODULE_PROFILE_V19,
            MODULE_PROFILE_V20,
            MODULE_PROFILE_V21,
            MODULE_PROFILE_V22,
            MODULE_PROFILE_V23,
            MODULE_PROFILE_V24,
            MODULE_PROFILE_V25,
            MODULE_PROFILE_V26,
            MODULE_PROFILE_V27,
            MODULE_PROFILE_V28,
            MODULE_PROFILE_V29,
            MODULE_PROFILE_V30,
        ]
        .contains(&record.profile.as_str())
        || record.scope != MODULE_SCOPE
        || record.system_scope != SYSTEM_SCOPE
        || record.settings != BuildSettings::expected_for(&record.link_build)
        || record.files.len() != BundleFile::ALL.len()
        || BundleFile::ALL
            .iter()
            .any(|role| !record.files.contains_key(role))
    {
        return Err(BundleError("unsupported schema, scope or settings"));
    }
    if !record.object_build.target_triple.starts_with("x86_64-")
        || !record.object_build.target_triple.contains("linux")
        || record.object_build.data_layout.is_empty()
        || !record.object_build.llvm_version.starts_with("17.")
        || !record.link_build.driver_target.starts_with("x86_64-")
        || !record.link_build.driver_target.contains("linux")
        || !["cc", "gcc", "clang", "clang-17"].contains(&record.link_build.driver.as_str())
        || !["gnu-ld", "lld"].contains(&record.link_build.linker.as_str())
        || !is_digest(&record.link_build.arguments_sha256)
        || record
            .link_build
            .canonical_arguments_sha256
            .as_deref()
            .is_some_and(|pin| !is_digest(pin))
        || [
            &record.object_build.llvm_version,
            &record.link_build.driver_version,
            &record.link_build.linker_version,
        ]
        .iter()
        .any(|value| !numeric_version(value))
    {
        return Err(BundleError("unsupported toolchain observation"));
    }
    Ok(record)
}

/// Pin a multifile record the same way `load_pinned_module_record` pins
/// a module one, plus the member-set gates: nonempty and bounded, every
/// member name well-formed, every member pinned by digest, the declared
/// root a member, and the import-graph pin a digest.
pub fn load_pinned_multifile_record(
    bytes: &[u8],
    expected_id: &str,
) -> Result<MultifileNativeBundleRecord, BundleError> {
    if !is_digest(expected_id) || sha256(bytes) != expected_id {
        return Err(BundleError("build identity mismatch"));
    }
    let record: MultifileNativeBundleRecord =
        serde_json::from_slice(bytes).map_err(|_| BundleError("invalid record"))?;
    if record.schema != MULTIFILE_SCHEMA
        || ![
            MULTIFILE_PROFILE,
            MULTIFILE_PROFILE_V2,
            MULTIFILE_PROFILE_V3,
            MULTIFILE_PROFILE_V4,
            MULTIFILE_PROFILE_V5,
            MULTIFILE_PROFILE_V6,
            MULTIFILE_PROFILE_V7,
            MULTIFILE_PROFILE_V8,
            MULTIFILE_PROFILE_V9,
            MULTIFILE_PROFILE_V10,
            MULTIFILE_PROFILE_V11,
            MULTIFILE_PROFILE_V12,
            MULTIFILE_PROFILE_V13,
            MULTIFILE_PROFILE_V14,
            MULTIFILE_PROFILE_V15,
            MULTIFILE_PROFILE_V16,
            MULTIFILE_PROFILE_V17,
            MULTIFILE_PROFILE_V18,
            MULTIFILE_PROFILE_V19,
            MULTIFILE_PROFILE_V20,
            MULTIFILE_PROFILE_V21,
            MULTIFILE_PROFILE_V22,
            MULTIFILE_PROFILE_V23,
            MULTIFILE_PROFILE_V24,
            MULTIFILE_PROFILE_V25,
            MULTIFILE_PROFILE_V26,
            MULTIFILE_PROFILE_V27,
            MULTIFILE_PROFILE_V28,
            MULTIFILE_PROFILE_V29,
            MULTIFILE_PROFILE_V30,
        ]
        .contains(&record.profile.as_str())
        || record.scope != MODULE_SCOPE
        || record.system_scope != SYSTEM_SCOPE
        || record.settings != BuildSettings::expected_for(&record.link_build)
        || record.files.len() != BundleFile::ALL.len()
        || BundleFile::ALL
            .iter()
            .any(|role| !record.files.contains_key(role))
    {
        return Err(BundleError("unsupported schema, scope or settings"));
    }
    if record.sources.is_empty()
        || record.sources.len() > MAX_SOURCE_FILES
        || record
            .sources
            .keys()
            .any(|name| member_name(name).ok().as_deref() != Some(name.as_str()))
        || !record.sources.contains_key(&record.source_files.root)
        || !is_digest(&record.source_files.import_graph_sha256)
        || record.sources.values().any(|pin| !is_digest(&pin.sha256))
    {
        return Err(BundleError("unsupported source member set"));
    }
    if !record.object_build.target_triple.starts_with("x86_64-")
        || !record.object_build.target_triple.contains("linux")
        || record.object_build.data_layout.is_empty()
        || !record.object_build.llvm_version.starts_with("17.")
        || !record.link_build.driver_target.starts_with("x86_64-")
        || !record.link_build.driver_target.contains("linux")
        || !["cc", "gcc", "clang", "clang-17"].contains(&record.link_build.driver.as_str())
        || !["gnu-ld", "lld"].contains(&record.link_build.linker.as_str())
        || !is_digest(&record.link_build.arguments_sha256)
        || record
            .link_build
            .canonical_arguments_sha256
            .as_deref()
            .is_some_and(|pin| !is_digest(pin))
        || [
            &record.object_build.llvm_version,
            &record.link_build.driver_version,
            &record.link_build.linker_version,
        ]
        .iter()
        .any(|value| !numeric_version(value))
    {
        return Err(BundleError("unsupported toolchain observation"));
    }
    Ok(record)
}

/// Compare a module bundle's snapshots to an externally supplied digest —
/// per-atom certificate binding, never a proof or authenticity verdict.
pub fn check_module_identity(
    record_bytes: &[u8],
    expected_id: &str,
    source: &str,
    files: &BTreeMap<BundleFile, Vec<u8>>,
) -> Result<(), BundleError> {
    let record = load_pinned_module_record(record_bytes, expected_id)?;
    let admission = module_refined_admission(&record.profile)?;
    let (binding, atoms, _types, _structs, _enums) = prepare_module_source(source, admission)?;
    if record.source != binding {
        return Err(BundleError("source or compiler identity mismatch"));
    }
    check_leaf_widths(&record.leaf_widths, source, admission, &record.profile)?;
    check_module_members(&record.files, &binding, &atoms, files)
}

/// A v24+ record must pin the two-slot pairing it describes; every
/// other profile must leave the field out — rechecking re-derives the
/// declared widths under the record's own admission so a pinned
/// pairing cannot smuggle a wrong slot split past the comparison.
fn check_leaf_widths(
    pinned: &BTreeMap<String, Vec<u64>>,
    source: &str,
    admission: RefinedAdmission,
    profile: &str,
) -> Result<(), BundleError> {
    let is_v24 = matches!(
        profile,
        MODULE_PROFILE_V24
            | MODULE_PROFILE_V25
            | MODULE_PROFILE_V26
            | MODULE_PROFILE_V27
            | MODULE_PROFILE_V28
            | MODULE_PROFILE_V29
            | MULTIFILE_PROFILE_V24
            | MULTIFILE_PROFILE_V25
            | MULTIFILE_PROFILE_V26
            | MULTIFILE_PROFILE_V27
            | MULTIFILE_PROFILE_V28
            | MULTIFILE_PROFILE_V29
            | MODULE_PROFILE_V30
            | MULTIFILE_PROFILE_V30
    );
    // v24 pins only when a wide leaf is actually declared in an atom
    // signature, so a v24 record must show a 128 somewhere. v25 pins on
    // the *declaration* of a wide enum payload — a module may declare
    // `enum F { A(i128) }` without any atom consuming it, in which case
    // every re-derived width is 64 and only the pairing equality binds.
    let is_v24_only = matches!(profile, MODULE_PROFILE_V24 | MULTIFILE_PROFILE_V24);
    let expected = semantic_contract::module_leaf_widths(source, admission)
        .map_err(|_| BundleError("source outside bounded module profile"))?;
    let has_wide = expected.values().flatten().any(|width| *width == 128);
    if is_v24 {
        if pinned != &expected || (is_v24_only && !has_wide) {
            return Err(BundleError("leaf-width pairing mismatch"));
        }
    } else if !pinned.is_empty() || has_wide {
        return Err(BundleError("leaf-width pairing mismatch"));
    }
    Ok(())
}

/// Compare a multifile bundle's snapshots to an externally supplied
/// digest: the supplied member texts must equal the pinned members
/// byte-for-byte, then the merge is re-derived from them and must bind
/// to the same source snapshot — the consumer never reads a filesystem.
pub fn check_multifile_identity(
    record_bytes: &[u8],
    expected_id: &str,
    root_name: &str,
    sources: &BTreeMap<String, String>,
    files: &BTreeMap<BundleFile, Vec<u8>>,
) -> Result<(), BundleError> {
    let record = load_pinned_multifile_record(record_bytes, expected_id)?;
    if record.source_files.root != root_name {
        return Err(BundleError("root source member mismatch"));
    }
    if sources.len() != record.sources.len()
        || sources.iter().any(|(name, text)| {
            record.sources.get(name) != Some(&ContentIdentity::of(text.as_bytes()))
        })
    {
        return Err(BundleError("missing or replaced source member"));
    }
    let admission = multifile_refined_admission(&record.profile)?;
    let prepared = prepare_multifile_source(root_name, sources, admission)?;
    if record.source != prepared.binding
        || record.source_files.import_graph_sha256 != prepared.import_graph_sha256
    {
        return Err(BundleError("source or compiler identity mismatch"));
    }
    check_leaf_widths(
        &record.leaf_widths,
        &prepared.merged,
        admission,
        &record.profile,
    )?;
    check_module_members(&record.files, &prepared.binding, &prepared.atoms, files)
}

/// The member/certificate half of module-family identity — shared by the
/// module and multifile paths once the record's `files` pins, the merged
/// source binding and the atoms are established.
fn check_module_members(
    record_files: &BTreeMap<BundleFile, ContentIdentity>,
    binding: &ModuleSourceBinding,
    atoms: &[parser::Atom],
    files: &BTreeMap<BundleFile, Vec<u8>>,
) -> Result<(), BundleError> {
    if files.len() != BundleFile::ALL.len()
        || BundleFile::ALL.iter().any(|role| {
            files
                .get(role)
                .is_none_or(|data| record_files.get(role) != Some(&ContentIdentity::of(data)))
        })
    {
        return Err(BundleError("missing or replaced bundle component"));
    }
    let cert: proof_cert::ProofCertificate =
        serde_json::from_slice(&files[&BundleFile::Certificate])
            .map_err(|_| BundleError("invalid certificate"))?;
    if !matches!(
        proof_cert::check_certificate_hash(&cert),
        proof_cert::CertificateHashCheck::Match
    ) || cert.atoms.len() != atoms.len()
        || cert.mumei_version != binding.compiler_version
        || proof_cert::validate_certificate_translator_versions(&cert).is_err()
    {
        return Err(BundleError("certificate binding mismatch"));
    }
    for (atom, cert_atom) in atoms.iter().zip(cert.atoms.iter()) {
        let Some(atom_binding) = binding.atoms.get(&atom.name) else {
            return Err(BundleError("certificate binding mismatch"));
        };
        let effects: Vec<String> = atom
            .effects
            .iter()
            .map(|effect| effect.name.clone())
            .collect();
        // The certificate annotates its translator IR with the
        // manual-lemma reason (`match`/inductive bodies always carry
        // one); the binding pins the extraction gate's IR identity,
        // which never carries that annotation.
        let mut cert_translator_ir = cert_atom.translator_ir.clone();
        cert_translator_ir.manual_lemma_reason = None;
        if cert_atom.name != atom.name
            || cert_atom.content_hash != atom_binding.atom_content_hash
            || cert_atom.requires != atom.requires
            || cert_atom.ensures != atom.ensures
            || cert_atom.body_expr != atom.body_expr
            || !cert_atom.assumed_clauses.is_empty()
            || !cert_atom.forall_constraints.is_empty()
            || cert_atom.effects != effects
            || sha256(
                &serde_json::to_vec(&cert_translator_ir)
                    .map_err(|_| BundleError("IR encoding failed"))?,
            ) != atom_binding.translator_ir_sha256
            || cert_atom.status != proof_cert::status::VERIFIED
            || cert_atom.z3_check_result != proof_cert::status::Z3_UNSAT
            || cert_atom.reused_from_store
            || !cert_atom.dependencies.is_empty()
        {
            return Err(BundleError("certificate binding mismatch"));
        }
    }
    let atom_refs: Vec<&parser::Atom> = atoms.iter().collect();
    if proof_cert::verify_certificate(&cert, &atom_refs, false)
        .iter()
        .any(|(_, status)| status != proof_cert::status::CERTIFICATE_PROVEN)
    {
        return Err(BundleError("certificate binding mismatch"));
    }
    Ok(())
}

/// Recheck every module atom's postcondition after the identity gates —
/// only `unsat` establishes the scoped result per atom; a refuted atom
/// refutes the module and an inconclusive one makes it inconclusive.
pub fn recheck_module_source(
    record_bytes: &[u8],
    expected_id: &str,
    source: &str,
    files: &BTreeMap<BundleFile, Vec<u8>>,
) -> Result<SourceRecheck, BundleError> {
    use z3::{Config, Context, Solver};
    let record = load_pinned_module_record(record_bytes, expected_id)?;
    let admission = module_refined_admission(&record.profile)?;
    let mut config = Config::new();
    config.set_timeout_msec(BuildSettings::default().solver_timeout_ms);
    config.set_param_value("rlimit", &RECHECK_RESOURCE_LIMIT.to_string());
    let context = Context::new(&config);
    let atoms = semantic_contract::encode_pure_i64_module_obligations(source, &context, admission)
        .map_err(|_| BundleError("source outside bounded module recheck profile"))?;
    check_module_identity(record_bytes, expected_id, source, files)?;
    let solver = Solver::new(&context);
    solve_module_obligations(&atoms, &solver)
}

/// Recheck a multifile bundle's merged module the same way — the merged
/// text is re-derived from the pinned member bytes inside
/// `check_multifile_identity`, so the obligations bind exactly the bytes
/// the record describes.
pub fn recheck_multifile_source(
    record_bytes: &[u8],
    expected_id: &str,
    root_name: &str,
    sources: &BTreeMap<String, String>,
    files: &BTreeMap<BundleFile, Vec<u8>>,
) -> Result<SourceRecheck, BundleError> {
    use z3::{Config, Context, Solver};
    let record = load_pinned_multifile_record(record_bytes, expected_id)?;
    if record.source_files.root != root_name {
        return Err(BundleError("root source member mismatch"));
    }
    let admission = multifile_refined_admission(&record.profile)?;
    let mut config = Config::new();
    config.set_timeout_msec(BuildSettings::default().solver_timeout_ms);
    config.set_param_value("rlimit", &RECHECK_RESOURCE_LIMIT.to_string());
    let context = Context::new(&config);
    let prepared = prepare_multifile_source(root_name, sources, admission)?;
    let atoms = semantic_contract::encode_pure_i64_module_obligations(
        &prepared.merged,
        &context,
        admission,
    )
    .map_err(|_| BundleError("source outside bounded module recheck profile"))?;
    check_multifile_identity(record_bytes, expected_id, root_name, sources, files)?;
    let solver = Solver::new(&context);
    solve_module_obligations(&atoms, &solver)
}

/// The per-atom obligation sweep shared by the module and multifile
/// source rechecks.
fn check_atom_loop_exit_domain(
    atom: &semantic_contract::ModuleAtomObligations<'_>,
    solver: &z3::Solver,
) -> Result<bool, BundleError> {
    if atom.loop_exits.as_bool() == Some(true) {
        return Ok(false);
    }
    let mut inconclusive = false;
    solver.reset();
    solver.assert(&atom.domain);
    solver.assert(&atom.loop_exits);
    match solver.check() {
        z3::SatResult::Sat => (),
        z3::SatResult::Unsat => {
            return Err(BundleError(
                "source domain and loop exits empty on all inputs",
            ))
        }
        z3::SatResult::Unknown => inconclusive = true,
    }
    solver.reset();
    solver.assert(&atom.domain);
    solver.assert(&atom.postcondition_hypotheses);
    solver.assert(&atom.loop_exits);
    match solver.check() {
        z3::SatResult::Sat => Ok(inconclusive),
        z3::SatResult::Unsat => Err(BundleError(
            "postcondition hypotheses and loop exits empty on all inputs",
        )),
        z3::SatResult::Unknown => Ok(true),
    }
}

/// Check whether each atom's input domain intersects the bounded-loop
/// exit hypotheses and postcondition assumptions used by its obligations.
/// Returns true if any query is inconclusive.
pub fn check_module_loop_exit_domains(
    atoms: &[semantic_contract::ModuleAtomObligations<'_>],
    solver: &z3::Solver,
) -> Result<bool, BundleError> {
    let mut inconclusive = false;
    for atom in atoms {
        inconclusive |= check_atom_loop_exit_domain(atom, solver)?;
    }
    Ok(inconclusive)
}

pub fn solve_module_obligations(
    atoms: &[semantic_contract::ModuleAtomObligations<'_>],
    solver: &z3::Solver,
) -> Result<SourceRecheck, BundleError> {
    let mut inconclusive = false;
    for atom in atoms {
        // An atom whose declared domain is empty — every guarded
        // operator poisoned or no input inside its signature bounds —
        // is outside the profile: its obligations would be vacuous,
        // matching the LLVM side's empty-domain rejection.
        solver.reset();
        solver.assert(&atom.domain);
        match solver.check() {
            z3::SatResult::Sat => (),
            z3::SatResult::Unsat => return Err(BundleError("empty source domain")),
            z3::SatResult::Unknown => inconclusive = true,
        }
        inconclusive |= check_atom_loop_exit_domain(atom, solver)?;
        for obligation in &atom.obligations {
            solver.reset();
            solver.assert(obligation);
            match solver.check() {
                z3::SatResult::Unsat => (),
                z3::SatResult::Sat => return Ok(SourceRecheck::Refuted),
                z3::SatResult::Unknown => inconclusive = true,
            }
        }
    }
    Ok(if inconclusive {
        SourceRecheck::Inconclusive
    } else {
        SourceRecheck::RecheckedWithinProfile
    })
}

#[cfg(test)]
mod multifile_tests {
    use super::*;

    const HELPERS: &str = "atom helper(x: i64) -> i64\n    requires: true;\n    ensures: result == x + 1;\n    body: { x + 1 };\n";
    const ROOT: &str = "import \"helpers\" as h;\n\natom main() -> i64\n    requires: true;\n    ensures: result > 0;\n    body: { h::helper(0) };\n";

    fn members() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("app.mm".to_owned(), ROOT.to_owned()),
            ("helpers.mm".to_owned(), HELPERS.to_owned()),
        ])
    }

    #[test]
    fn merge_strips_imports_and_orders_imported_first() {
        let (merged, graph) =
            merge_import_closure("app.mm", &members(), RefinedAdmission::Reject).unwrap();
        assert_eq!(graph["app.mm"], ["helpers.mm"]);
        assert!(graph["helpers.mm"].is_empty());
        assert!(merged.starts_with(HELPERS.trim_end()));
        assert!(merged.contains("atom main"));
        assert!(merged.contains("body: { helper(0) };"));
        assert!(!merged.contains("import") && !merged.contains("h::"));
    }

    #[test]
    fn member_name_grammar() {
        assert_eq!(member_name("helpers").unwrap(), "helpers.mm");
        assert_eq!(member_name("helpers.mm").unwrap(), "helpers.mm");
        // Slash-separated member paths (subdirectories of the root's
        // directory) are admitted; `std/` names a local subdirectory
        // exactly as the language resolver tries local files first.
        assert_eq!(member_name("a/b.mm").unwrap(), "a/b.mm");
        assert_eq!(member_name("a/b").unwrap(), "a/b.mm");
        assert_eq!(member_name("std/x.mm").unwrap(), "std/x.mm");
        for bad in [
            "../up.mm",
            "a/../b.mm",
            "a//b.mm",
            "a/./b.mm",
            "/abs.mm",
            ".mm",
            ".hidden.mm",
            "a/.hidden.mm",
            "",
            "a\\b.mm",
            "a:b.mm",
        ] {
            assert!(member_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn resolve_member_anchors_at_the_importing_file() {
        // File-relative: a member's imports resolve against its own
        // directory, mirroring resolver::imports resolve_path.
        assert_eq!(resolve_member("app.mm", "dep").unwrap(), "dep.mm");
        assert_eq!(resolve_member("app.mm", "sub/dep").unwrap(), "sub/dep.mm");
        assert_eq!(resolve_member("sub/dep.mm", "leaf").unwrap(), "sub/leaf.mm");
        assert_eq!(resolve_member("sub/dep.mm", "a/b").unwrap(), "sub/a/b.mm");
        // Confinement: escaping the root's directory is rejected.
        assert!(resolve_member("sub/dep.mm", "../escape").is_err());
        assert!(resolve_member("app.mm", "..").is_err());
    }

    const DEEP: &str =
        "atom deep() -> i64\n    requires: true;\n    ensures: result == 1;\n    body: { 1 };";

    #[test]
    fn nested_imports_merge_in_post_order() {
        // app -> helpers -> deep: deep emits first, helpers next, root last.
        let mut nested = members();
        nested.insert("deep.mm".into(), DEEP.into());
        nested.insert(
            "helpers.mm".into(),
            format!("import \"deep\" as d;\n{HELPERS}"),
        );
        let (merged, graph) =
            merge_import_closure("app.mm", &nested, RefinedAdmission::Reject).unwrap();
        assert_eq!(graph["app.mm"], ["helpers.mm"]);
        assert_eq!(graph["helpers.mm"], ["deep.mm"]);
        assert!(graph["deep.mm"].is_empty());
        let deep_at = merged.find("atom deep").unwrap();
        let helper_at = merged.find("atom helper").unwrap();
        let main_at = merged.find("atom main").unwrap();
        assert!(deep_at < helper_at && helper_at < main_at);
        assert!(!merged.contains("import"));
    }

    #[test]
    fn nested_member_strips_its_own_qualifier() {
        // A member's namespace is file-local: helpers calls `d::deep(0)`
        // which merges as bare `deep(0)`, and the root's `h` never leaks
        // into the member's text.
        let mut nested = members();
        nested.insert("deep.mm".into(), DEEP.into());
        nested.insert(
            "helpers.mm".into(),
            "import \"deep\" as d;\natom helper(x: i64) -> i64\n    requires: true;\n    ensures: result == x + 1;\n    body: { x + d::deep(0) };".into(),
        );
        let (merged, _) =
            merge_import_closure("app.mm", &nested, RefinedAdmission::Reject).unwrap();
        assert!(merged.contains("body: { x + deep(0) };"));
        assert!(!merged.contains("d::") && !merged.contains("h::"));
    }

    #[test]
    fn diamond_imports_emit_each_member_once() {
        // a -> b, c; b -> shared; c -> shared: shared emits once.
        let shared =
            "atom shared() -> i64\n    requires: true;\n    ensures: result == 1;\n    body: { 1 };";
        let b = "import \"shared\" as s;\natom fb() -> i64\n    requires: true;\n    ensures: result == 1;\n    body: { s::shared() };";
        let c = "import \"shared\" as t;\natom fc() -> i64\n    requires: true;\n    ensures: result == 1;\n    body: { t::shared() };";
        let a = "import \"b\";\nimport \"c\";\natom main() -> i64\n    requires: true;\n    ensures: result > 0;\n    body: { fb() + fc() };";
        let sources = BTreeMap::from([
            ("app.mm".to_owned(), a.to_owned()),
            ("b.mm".to_owned(), b.to_owned()),
            ("c.mm".to_owned(), c.to_owned()),
            ("shared.mm".to_owned(), shared.to_owned()),
        ]);
        let (merged, graph) =
            merge_import_closure("app.mm", &sources, RefinedAdmission::Reject).unwrap();
        assert_eq!(merged.matches("atom shared").count(), 1);
        assert_eq!(graph["b.mm"], ["shared.mm"]);
        assert_eq!(graph["c.mm"], ["shared.mm"]);
        assert!(merged.contains("body: { shared() };") && merged.contains("shared() }"));
    }

    #[test]
    fn cyclic_imports_are_rejected_honestly() {
        let a = "import \"b\";\natom fa() -> i64\n    requires: true;\n    ensures: result == 1;\n    body: { 1 };";
        let b = "import \"app\";\natom fb() -> i64\n    requires: true;\n    ensures: result == 1;\n    body: { 1 };";
        let sources = BTreeMap::from([
            ("app.mm".to_owned(), a.to_owned()),
            ("b.mm".to_owned(), b.to_owned()),
        ]);
        assert!(merge_import_closure("app.mm", &sources, RefinedAdmission::Reject).is_err());
    }

    #[test]
    fn duplicate_qualifiers_are_rejected() {
        // `import "a" as k; import "b" as k;` binds both members to the
        // same namespace sugar: every `k::f` would strip to bare `f`
        // ambiguously.
        let member =
            "atom f() -> i64\n    requires: true;\n    ensures: result == 1;\n    body: { 1 };";
        let dup = BTreeMap::from([
            (
                "app.mm".to_owned(),
                "import \"a\" as k;\nimport \"b\" as k;\natom main() -> i64\n    requires: true;\n    ensures: result == 1;\n    body: { k::f() };".to_owned(),
            ),
            ("a.mm".to_owned(), member.to_owned()),
            ("b.mm".to_owned(), member.to_owned()),
        ]);
        assert!(merge_import_closure("app.mm", &dup, RefinedAdmission::Reject).is_err());
        // The same ambiguity without aliases: `import "a"; import
        // "b";` is fine (qualifiers `a`/`b` differ), but a member named
        // `k.mm` aliased as `k` collides with an unaliased `k`.
        let alias_stem_dup = BTreeMap::from([
            (
                "app.mm".to_owned(),
                "import \"k\";\nimport \"a\" as k;\natom main() -> i64\n    requires: true;\n    ensures: result == 1;\n    body: { 1 };".to_owned(),
            ),
            ("a.mm".to_owned(), member.to_owned()),
            ("k.mm".to_owned(), member.to_owned()),
        ]);
        assert!(merge_import_closure("app.mm", &alias_stem_dup, RefinedAdmission::Reject).is_err());
    }

    #[test]
    fn leaf_member_bytes_are_merged_verbatim() {
        // A CRLF leaf contributes its bytes untouched: the merge must
        // not silently normalize them, or a slice-1 bundle's pinned
        // merged sha could never re-derive.
        let mut crlf = members();
        crlf.insert(
            "helpers.mm".into(),
            "atom helper(x: i64) -> i64\r\n    requires: true;\r\n    ensures: result == x + 1;\r\n    body: { x + 1 };\r\n".into(),
        );
        let (merged, _) = merge_import_closure("app.mm", &crlf, RefinedAdmission::Reject).unwrap();
        assert!(merged.contains("atom helper(x: i64) -> i64\r\n    requires: true;"));
    }

    #[test]
    fn unpinned_and_missing_members_rejected() {
        let only_root = BTreeMap::from([("app.mm".to_owned(), ROOT.to_owned())]);
        assert!(merge_import_closure("app.mm", &only_root, RefinedAdmission::Reject).is_err());
        let missing_deep = BTreeMap::from([("app.mm".to_owned(), "import \"deep\";\natom main() -> i64\n    requires: true;\n    ensures: result == 1;\n    body: { 1 };".to_owned())]);
        assert!(merge_import_closure("app.mm", &missing_deep, RefinedAdmission::Reject).is_err());
    }

    #[test]
    fn qualifier_stripping_respects_identifier_boundaries() {
        assert_eq!(strip_qualifier("h::f(0)", "h"), "f(0)");
        assert_eq!(strip_qualifier("x + h::f(0)", "h"), "x + f(0)");
        assert_eq!(strip_qualifier("ph::f", "h"), "ph::f");
        assert_eq!(strip_qualifier("_h::f", "h"), "_h::f");
        assert_eq!(strip_qualifier("helpers::f", "h"), "helpers::f");
        // A longer identifier starting with the qualifier keeps its `::`.
        assert_eq!(strip_qualifier("hh::f", "h"), "hh::f");
        // Non-ASCII bytes count as identifier bytes: `éh::f` is a call
        // through qualifier `éh`, not `h`.
        assert_eq!(strip_qualifier("éh::f", "h"), "éh::f");
        // The parser accepts whitespace around `::` in a qualified call.
        assert_eq!(strip_qualifier("h :: f(0)", "h"), "f(0)");
        assert_eq!(strip_qualifier("h  ::  f(0)", "h"), "f(0)");
        assert_eq!(strip_qualifier("h ::\nf(0)", "h"), "f(0)");
        // `q ::` not followed by a callee name is not qualified syntax —
        // left intact so merged admission rejects it.
        assert_eq!(strip_qualifier("x = h :: 5", "h"), "x = h :: 5");
        assert_eq!(strip_qualifier("x = h ::", "h"), "x = h ::");
        // A bare qualifier with no `::` is never touched.
        assert_eq!(strip_qualifier("let h = 3; h + 1", "h"), "let h = 3; h + 1");
    }

    #[test]
    fn qualifier_stripping_leaves_line_comments_alone() {
        // A qualifier look-alike inside a `//` comment is comment text:
        // stripping `// h ::` would splice the next line onto the
        // comment and erase the following declaration.
        assert_eq!(strip_qualifier("// h ::\natom f", "h"), "// h ::\natom f");
        assert_eq!(
            strip_qualifier("x = h::f // note h::g", "h"),
            "x = f // note h::g"
        );
        // A comment line still counts its `//` for later matches.
        assert_eq!(strip_qualifier("// note\nh::f(0)", "h"), "// note\nf(0)");
    }

    #[test]
    fn commented_import_line_and_qualified_comment_merge() {
        let commented = BTreeMap::from([
            (
                "app.mm".to_owned(),
                "// h :: is not a call\nimport \"helpers\" as h; // trailing note\n\natom main() -> i64\n    requires: true;\n    ensures: result > 0;\n    body: { h::helper(0) };\n".to_owned(),
            ),
            ("helpers.mm".to_owned(), HELPERS.to_owned()),
        ]);
        let (merged, graph) =
            merge_import_closure("app.mm", &commented, RefinedAdmission::Reject).unwrap();
        assert_eq!(graph["app.mm"], ["helpers.mm"]);
        // The comment survives verbatim and `atom main` is not folded
        // into it.
        assert!(merged.contains("// h :: is not a call"));
        assert!(merged.contains("atom main"));
        assert!(merged.contains("body: { helper(0) };"));
    }

    #[test]
    fn import_without_semicolon_is_rejected() {
        let missing = BTreeMap::from([
            (
                "app.mm".to_owned(),
                ROOT.replacen("import \"helpers\" as h;", "import \"helpers\" as h", 1),
            ),
            ("helpers.mm".to_owned(), HELPERS.to_owned()),
        ]);
        assert!(merge_import_closure("app.mm", &missing, RefinedAdmission::Reject).is_err());
    }

    #[test]
    fn import_sharing_a_line_with_another_declaration_is_rejected() {
        let shared = BTreeMap::from([
            (
                "app.mm".to_owned(),
                "import \"helpers\" as h; atom main() -> i64 requires: true; ensures: result > 0; body: { h::helper(0) };"
                    .to_owned(),
            ),
            ("helpers.mm".to_owned(), HELPERS.to_owned()),
        ]);
        assert!(merge_import_closure("app.mm", &shared, RefinedAdmission::Reject).is_err());
    }

    #[test]
    fn prepare_multifile_binds_merged_module() {
        let prepared =
            prepare_multifile_source("app.mm", &members(), RefinedAdmission::Reject).unwrap();
        assert_eq!(
            prepared.binding.source_sha256,
            sha256(prepared.merged.as_bytes())
        );
        assert_eq!(prepared.binding.atoms.len(), 2);
        assert!(
            prepared.binding.atoms.contains_key("helper")
                && prepared.binding.atoms.contains_key("main")
        );
        assert!(is_digest(&prepared.import_graph_sha256));
        assert_eq!(prepared.atoms.len(), 2);
    }
}

#[cfg(test)]
mod source_recheck_tests {
    use super::*;
    use z3::{Config, Context, SatResult, Solver};

    fn check_module_source(source: &str) -> Result<SourceRecheck, BundleError> {
        let mut config = Config::new();
        config.set_timeout_msec(BuildSettings::default().solver_timeout_ms);
        let context = Context::new(&config);
        let atoms = semantic_contract::encode_pure_i64_module_obligations(
            source,
            &context,
            RefinedAdmission::CompositeV26,
        )
        .unwrap();
        let solver = Solver::new(&context);
        solve_module_obligations(&atoms, &solver)
    }

    #[test]
    fn only_unsat_can_establish_the_source_postcondition() {
        assert_eq!(
            source_recheck_result(SatResult::Unsat),
            SourceRecheck::RecheckedWithinProfile
        );
        assert_eq!(
            source_recheck_result(SatResult::Sat),
            SourceRecheck::Refuted
        );
        assert_eq!(
            source_recheck_result(SatResult::Unknown),
            SourceRecheck::Inconclusive
        );
    }

    #[test]
    fn source_obligation_models_conditions_signed_boundaries_and_real_exhaustion() {
        for (body, ensures, expected) in [
            ("7", "result == 7", SatResult::Unsat),
            ("7", "result == 8", SatResult::Sat),
            (
                "if 1 < 2 { 7 } else { 8 }",
                "(true => result == 7) && ((result < 8) == true)",
                SatResult::Unsat,
            ),
            ("-9223372036854775807", "result < 0", SatResult::Unsat),
            ("9223372036854775807", "result < 0", SatResult::Sat),
        ] {
            let config = Config::new();
            let context = Context::new(&config);
            let source = format!(
                "atom main() -> i64 requires: true; ensures: {ensures}; body: {{ {body} }};"
            );
            let obligation =
                semantic_contract::encode_pure_i64_obligation(&source, &context).unwrap();
            let solver = Solver::new(&context);
            solver.assert(&obligation);
            assert_eq!(solver.check(), expected);
        }
        for (requires, ensures, expected) in [
            ("x >= 0", "result == x", SatResult::Unsat),
            ("true", "result == x", SatResult::Sat),
            ("true", "result >= 0", SatResult::Unsat),
            ("false", "false", SatResult::Unsat),
        ] {
            let config = Config::new();
            let context = Context::new(&config);
            let source = format!("atom f(x: i64) -> i64 requires: {requires}; ensures: {ensures}; body: {{ if x < 0 {{ 0 }} else {{ x }} }};");
            let obligation =
                semantic_contract::encode_pure_i64_obligation(&source, &context).unwrap();
            let solver = Solver::new(&context);
            solver.assert(&obligation);
            assert_eq!(solver.check(), expected);
        }
        let mut config = Config::new();
        config.set_param_value("rlimit", "1");
        let context = Context::new(&config);
        let obligation = semantic_contract::encode_pure_i64_obligation("atom f(x: i64) -> i64 requires: true; ensures: result == x; body: { if x < 0 { 0 } else { x } };", &context).unwrap();
        let solver = Solver::new(&context);
        solver.assert(&obligation);
        assert_eq!(
            source_recheck_result(solver.check()),
            SourceRecheck::Inconclusive
        );
    }

    #[test]
    fn module_recheck_rejects_non_exiting_requires_loop_domain() {
        let source = "atom helper() -> i64\n\
            requires: { let j = 0; while true invariant: j >= 0 decreases: 4 { j = j + 1 }; true };\n\
            ensures: result == 0;\n\
            body: 0;\n\
            atom main() -> i64 requires: true; ensures: result == 0; body: 0;";
        let error = check_module_source(source).unwrap_err();
        assert_eq!(error.0, "source domain and loop exits empty on all inputs");
    }

    #[test]
    fn module_recheck_rejects_requires_with_no_bounded_loop_exit_inputs() {
        let source = "atom count(n: i64) -> i64\n\
            requires: n >= 10 && n <= 20;\n\
            ensures: result == 999;\n\
            body: { let i = 0; while i < n invariant: i >= 0 && i <= n decreases: n - i { i = i + 1 }; i };";
        let config = Config::new();
        let context = Context::new(&config);
        let atoms = semantic_contract::encode_pure_i64_module_obligations(
            source,
            &context,
            RefinedAdmission::CompositeV26,
        )
        .unwrap();
        assert_eq!(atoms.len(), 1);
        let atom = &atoms[0];
        let solver = Solver::new(&context);
        solver.assert(&atom.domain);
        solver.assert(&atom.loop_exits);
        assert_eq!(solver.check(), SatResult::Sat);
        for obligation in &atom.obligations {
            solver.reset();
            solver.assert(obligation);
            assert_eq!(solver.check(), SatResult::Unsat);
        }
        let error = check_module_loop_exit_domains(&atoms, &solver).unwrap_err();
        assert_eq!(
            error.0,
            "postcondition hypotheses and loop exits empty on all inputs"
        );

        solver.reset();
        solver.assert(&atom.domain);
        solver.assert(&atom.postcondition_hypotheses);
        solver.assert(&atom.loop_exits);
        assert_eq!(solver.check(), SatResult::Unsat);

        let control = "atom count(n: i64) -> i64\n\
            requires: n >= 0 && n <= 2;\n\
            ensures: result == n;\n\
            body: { let i = 0; while i < n invariant: i >= 0 && i <= n decreases: n - i { i = i + 1 }; i };";
        let control_config = Config::new();
        let control_context = Context::new(&control_config);
        let control_atoms = semantic_contract::encode_pure_i64_module_obligations(
            control,
            &control_context,
            RefinedAdmission::CompositeV26,
        )
        .unwrap();
        let control_solver = Solver::new(&control_context);
        assert!(!check_module_loop_exit_domains(&control_atoms, &control_solver).unwrap());
        assert_eq!(
            check_module_source(control).unwrap(),
            SourceRecheck::RecheckedWithinProfile
        );
    }

    #[test]
    fn module_recheck_checks_loop_domain_under_postcondition_hypotheses() {
        let source = "atom id(x: i64) -> i64 requires: true; ensures: result == x; body: x;\n\
            atom count(n: i64) -> i64\n\
            requires: n >= 10 && n <= 20;\n\
            ensures: result == 999;\n\
            body: { let m = id(n); let i = 0; while i < m invariant: i >= 0 decreases: m - i { i = i + 1 }; i };";
        let config = Config::new();
        let context = Context::new(&config);
        let atoms = semantic_contract::encode_pure_i64_module_obligations(
            source,
            &context,
            RefinedAdmission::CompositeV26,
        )
        .unwrap();
        let solver = Solver::new(&context);
        for atom in &atoms {
            solver.reset();
            solver.assert(&atom.domain);
            solver.assert(&atom.loop_exits);
            assert_eq!(solver.check(), SatResult::Sat, "{}", atom.atom);
            for obligation in &atom.obligations {
                solver.reset();
                solver.assert(obligation);
                assert_eq!(solver.check(), SatResult::Unsat, "{}", atom.atom);
            }
        }

        let error = check_module_loop_exit_domains(&atoms, &solver).unwrap_err();
        assert_eq!(
            error.0,
            "postcondition hypotheses and loop exits empty on all inputs"
        );
    }

    #[test]
    fn module_recheck_rejects_non_exiting_body_loop_and_accepts_partial_exit_domain() {
        let non_exiting = "atom f() -> i64 requires: true; ensures: true; body: { let j = 0; while true invariant: j >= 0 decreases: 4 { j = j + 1 }; 0 };";
        let error = check_module_source(non_exiting).unwrap_err();
        assert_eq!(error.0, "empty source domain");

        let exits_on_some_inputs = "atom count(n: i64) -> i64\n\
            requires: n >= 0 && n <= 2;\n\
            ensures: result == n;\n\
            body: { let i = 0; while i < n invariant: i >= 0 && i <= n decreases: n - i { i = i + 1 }; i };\n\
            atom main() -> i64 requires: true; ensures: result == 2; body: { count(2) };";
        assert_eq!(
            check_module_source(exits_on_some_inputs).unwrap(),
            SourceRecheck::RecheckedWithinProfile
        );
    }

    #[test]
    fn prepare_module_source_names_the_refined_profile_only_when_used() {
        // A module declaring or using refined scalar types binds the v2
        // source profile — and refuses the v1 admission outright, so a
        // v1 record could never have been built from it.
        let refined = "type Port = i64 where v >= 1 && v <= 65535;\n\
             atom main() -> i64 requires: true; ensures: result == 0; body: { 0 };";
        assert!(prepare_module_source(refined, RefinedAdmission::Reject).is_err());
        let (binding, atoms, types, _, _) =
            prepare_module_source(refined, RefinedAdmission::V1).unwrap();
        assert_eq!(
            binding.source_profile,
            semantic_contract::PURE_I64_REFINED_PROFILE
        );
        assert_eq!(types.len(), 1);
        assert_eq!(atoms.len(), 1);
        // A plain module keeps the wrapping profile under the same gate.
        let plain = "atom main() -> i64 requires: true; ensures: result == 0; body: { 0 };";
        let (binding, _atoms, types, _, _) =
            prepare_module_source(plain, RefinedAdmission::V1).unwrap();
        assert_eq!(
            binding.source_profile,
            semantic_contract::PURE_I64_WRAPPING_PROFILE
        );
        assert!(types.is_empty());
        // The record profile mirrors the bound source profile, and both
        // admission modes decode — anything else is not this schema.
        assert_eq!(
            module_refined_admission(MODULE_PROFILE).unwrap(),
            RefinedAdmission::Reject
        );
        assert_eq!(
            module_refined_admission(MODULE_PROFILE_V2).unwrap(),
            RefinedAdmission::V1
        );
        assert_eq!(
            module_refined_admission(MODULE_PROFILE_V3).unwrap(),
            RefinedAdmission::V2
        );
        assert_eq!(
            multifile_refined_admission(MULTIFILE_PROFILE).unwrap(),
            RefinedAdmission::Reject
        );
        assert_eq!(
            multifile_refined_admission(MULTIFILE_PROFILE_V2).unwrap(),
            RefinedAdmission::V1
        );
        assert_eq!(
            multifile_refined_admission(MULTIFILE_PROFILE_V3).unwrap(),
            RefinedAdmission::V2
        );
        assert_eq!(
            module_refined_admission(MODULE_PROFILE_V4).unwrap(),
            RefinedAdmission::Composite
        );
        assert_eq!(
            multifile_refined_admission(MULTIFILE_PROFILE_V4).unwrap(),
            RefinedAdmission::Composite
        );
        assert_eq!(
            module_refined_admission(MODULE_PROFILE_V18).unwrap(),
            RefinedAdmission::CompositeV15
        );
        assert_eq!(
            multifile_refined_admission(MULTIFILE_PROFILE_V18).unwrap(),
            RefinedAdmission::CompositeV15
        );
        assert_eq!(
            module_refined_admission(MODULE_PROFILE_V20).unwrap(),
            RefinedAdmission::CompositeV17
        );
        assert_eq!(
            multifile_refined_admission(MULTIFILE_PROFILE_V20).unwrap(),
            RefinedAdmission::CompositeV17
        );
        assert_eq!(
            module_refined_admission(MODULE_PROFILE_V27).unwrap(),
            RefinedAdmission::CompositeV24
        );
        assert_eq!(
            module_refined_admission(MODULE_PROFILE_V28).unwrap(),
            RefinedAdmission::CompositeV25
        );
        assert_eq!(
            module_refined_admission(MODULE_PROFILE_V29).unwrap(),
            RefinedAdmission::CompositeV26
        );
        assert_eq!(
            module_refined_admission(MODULE_PROFILE_V30).unwrap(),
            RefinedAdmission::CompositeV29
        );
        assert_eq!(
            multifile_refined_admission(MULTIFILE_PROFILE_V27).unwrap(),
            RefinedAdmission::CompositeV24
        );
        assert_eq!(
            multifile_refined_admission(MULTIFILE_PROFILE_V28).unwrap(),
            RefinedAdmission::CompositeV25
        );
        assert_eq!(
            multifile_refined_admission(MULTIFILE_PROFILE_V29).unwrap(),
            RefinedAdmission::CompositeV26
        );
        assert_eq!(
            multifile_refined_admission(MULTIFILE_PROFILE_V30).unwrap(),
            RefinedAdmission::CompositeV29
        );
        assert!(module_refined_admission("mumei.other/v1").is_err());
        assert!(multifile_refined_admission("mumei.other/v1").is_err());
        // The module and multifile families never decode each other's
        // composite profile.
        assert!(module_refined_admission(MULTIFILE_PROFILE_V4).is_err());
        assert_eq!(
            module_refined_admission(MODULE_PROFILE_V5).unwrap(),
            RefinedAdmission::CompositeV2
        );
        assert_eq!(
            multifile_refined_admission(MULTIFILE_PROFILE_V5).unwrap(),
            RefinedAdmission::CompositeV2
        );
        assert!(multifile_refined_admission(MODULE_PROFILE_V4).is_err());
        assert_eq!(
            module_refined_admission(MODULE_PROFILE_V6).unwrap(),
            RefinedAdmission::CompositeV3
        );
        assert_eq!(
            multifile_refined_admission(MULTIFILE_PROFILE_V6).unwrap(),
            RefinedAdmission::CompositeV3
        );
        assert!(module_refined_admission(MULTIFILE_PROFILE_V6).is_err());
    }

    const COMPOSITE: &str = "struct Point { x: i64, y: i64 }\n\
         atom gx(p: Point) -> i64 requires: true; ensures: result == p.x; body: { p.x };\n\
         atom main() -> i64 requires: true; ensures: result == 0; body: { 0 };";

    #[test]
    fn prepare_module_source_names_the_composite_profile_only_with_structs() {
        for admission in [
            RefinedAdmission::Reject,
            RefinedAdmission::V1,
            RefinedAdmission::V2,
        ] {
            assert!(prepare_module_source(COMPOSITE, admission).is_err());
        }
        let (binding, atoms, types, structs, enums) =
            prepare_module_source(COMPOSITE, RefinedAdmission::Composite).unwrap();
        assert_eq!(
            binding.source_profile,
            semantic_contract::PURE_I64_COMPOSITE_PROFILE
        );
        assert_eq!(atoms.len(), 2);
        assert!(types.is_empty());
        assert_eq!(structs.len(), 1);
        assert_eq!(structs[0].name, "Point");
        assert!(enums.is_empty());
        // The composite gate leaves plain and refined modules on their
        // own narrower names, with byte-identical bindings.
        let plain = "atom main() -> i64 requires: true; ensures: result == 0; body: { 0 };";
        let refined = "type Port = i64 where v >= 1;\n\
             atom main() -> i64 requires: true; ensures: result == 0; body: { 0 };";
        for (source, narrow) in [
            (plain, RefinedAdmission::Reject),
            (refined, RefinedAdmission::V1),
        ] {
            let (wide, _, _, structs, _) =
                prepare_module_source(source, RefinedAdmission::Composite).unwrap();
            assert!(structs.is_empty());
            assert_eq!(wide, prepare_module_source(source, narrow).unwrap().0);
        }
    }

    #[test]
    fn struct_members_ride_the_merge_only_under_composite_admission() {
        let sources = BTreeMap::from([
            ("shapes.mm".to_owned(), format!("{}\n", COMPOSITE.replace("\natom main() -> i64 requires: true; ensures: result == 0; body: { 0 };", ""))),
            (
                "app.mm".to_owned(),
                "import \"shapes\" as s;\n\natom main() -> i64 requires: true; ensures: result == 0; body: { 0 };\n"
                    .to_owned(),
            ),
        ]);
        for admission in [
            RefinedAdmission::Reject,
            RefinedAdmission::V1,
            RefinedAdmission::V2,
        ] {
            assert!(merge_import_closure("app.mm", &sources, admission).is_err());
        }
        let (merged, _) =
            merge_import_closure("app.mm", &sources, RefinedAdmission::Composite).unwrap();
        assert!(merged.starts_with("struct Point"));
        let prepared =
            prepare_multifile_source("app.mm", &sources, RefinedAdmission::Composite).unwrap();
        assert_eq!(prepared.structs.len(), 1);
        assert_eq!(
            prepared.binding.source_profile,
            semantic_contract::PURE_I64_COMPOSITE_PROFILE
        );
    }

    #[test]
    fn enum_members_ride_the_module_and_merge_only_under_composite_v4() {
        let enum_source = "enum Shape {\n    Point,\n    Circle(r: i64),\n    Rect(w: i64, h: i64)\n}\n\
             atom tag_of(s: Shape) -> i64 requires: true; ensures: result >= 0 && result <= 2; body: { match s { Shape::Circle(r) => 1, Shape::Rect(w, h) => 2, _ => 0 } };\n\
             atom main() -> i64 requires: true; ensures: result == 0; body: { 0 };";
        for admission in [
            RefinedAdmission::Reject,
            RefinedAdmission::V1,
            RefinedAdmission::V2,
            RefinedAdmission::Composite,
            RefinedAdmission::CompositeV2,
            RefinedAdmission::CompositeV3,
        ] {
            assert!(prepare_module_source(enum_source, admission).is_err());
        }
        let (binding, atoms, _types, _structs, enums) =
            prepare_module_source(enum_source, RefinedAdmission::CompositeV4).unwrap();
        assert_eq!(enums.len(), 1);
        assert_eq!(enums[0].name, "Shape");
        assert_eq!(atoms.len(), 2);
        assert_eq!(
            binding.source_profile,
            semantic_contract::PURE_I64_COMPOSITE_PROFILE_V4
        );
        // The merge carries enum declarations under composite v4 only.
        let sources = BTreeMap::from([
            ("shapes.mm".to_owned(), enum_source.rsplit_once("\natom main()").unwrap().0.to_owned() + "\n"),
            (
                "app.mm".to_owned(),
                "import \"shapes\" as s;\n\natom main() -> i64 requires: true; ensures: result == 0; body: { 0 };\n"
                    .to_owned(),
            ),
        ]);
        assert!(merge_import_closure("app.mm", &sources, RefinedAdmission::CompositeV3).is_err());
        let prepared =
            prepare_multifile_source("app.mm", &sources, RefinedAdmission::CompositeV4).unwrap();
        assert_eq!(prepared.enums.len(), 1);
        assert_eq!(
            prepared.binding.source_profile,
            semantic_contract::PURE_I64_COMPOSITE_PROFILE_V4
        );
    }
}
