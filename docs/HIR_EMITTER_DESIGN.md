---
layout: default
title: "HIR-Centred Emitter Design — Mumei"
description: "Design for making HIR the single input to every Mumei emitter: survey of the built-in emitters and the plugin ABI, migration order, risks, and guard tests."
keywords: "mumei HIR, emitter, plugin ABI, LLVM codegen, code generation design"
---

# HIR-Centred Emitter Design

Status: design only. This document does not change any emitter.

## Goal

Every emitter should read one input, the HIR of a verified atom, and nothing
from the parser's AST. Today the `Emitter` trait already receives a `HirAtom`,
but `HirAtom` carries the whole AST `Atom` and its `Stmt` body, and every
emitter reaches through `hir_atom.atom` for most of what it prints. The result
is two sources of truth for one atom: a structured body in HIR, and raw
strings (`requires`, `ensures`, `body_expr`) plus AST metadata in `Atom`.

Making HIR the single input gives:

- one place that decides what a contract clause *is* (its kind, trust mode,
  label, and lowered expression), so that wrappers and monitors cannot drift
  from what the verifier checked ([`VERIFIER_SPEC.md`](VERIFIER_SPEC.md),
  section 3);
- emitters that no longer re-parse or textually rewrite contract strings;
- a plugin interface that does not expose the parser's internal types.

Non-goals: changing what any emitter outputs, changing verification, and
changing proof hashes. In particular `proof_cert::compute_atom_content_hash_v2`
keeps hashing the AST `Atom`; the HIR must carry the hash (or the inputs to it)
rather than recompute it from a different representation.

## Current state

### The emitter interface

`mumei-core/src/emitter.rs`:

```rust
pub trait Emitter {
    fn emit(
        &self,
        hir_atom: &HirAtom,
        output_path: &Path,
        module_env: &ModuleEnv,
        extern_blocks: &[ExternBlock],
    ) -> MumeiResult<Vec<Artifact>>;
}
```

- Built-in targets are listed in `BUILTIN_EMIT_TARGETS` and dispatched by
  `src/codegen.rs::dispatch_emit`. `c-header` (`CHeaderEmitter`) lives in
  `mumei-core` itself; `binary`, `proof-cert`, `escalation-bundle`, and
  `decidable-metrics` are handled by the CLI, not by an `Emitter`.
- External plugins are dynamic libraries loaded by `load_external_emitter` /
  `load_external_emitter_from_path`. They export
  `mumei_emitter_abi_version() -> u32`, which must equal `EMITTER_ABI_VERSION`
  (currently `1`), and `mumei_create_emitter() -> EmitterPluginHandle`, a
  `#[repr(C)]` pair of the trait object's data and vtable pointers. Calls are
  wrapped in `PanicSafeEmitter`. See [`PLUGIN_GUIDE.md`](PLUGIN_GUIDE.md).
- The handle is a Rust trait object passed across a dynamic-library boundary.
  That only works when the plugin and the host were built from the same
  `mumei-core` with the same compiler, so in practice **the layout of
  `HirAtom`, `ModuleEnv`, `ExternBlock`, `Artifact`, and everything they
  contain is part of the ABI**, even though the version constant only guards
  the entry points.

### What `HirAtom` holds

`mumei-core/src/hir.rs`:

| Field | Type | Used by emitters? |
|---|---|---|
| `body` | `HirStmt` | LLVM only |
| `requires_hir` | `HirExpr` | No (the whole `requires` string lowered as one expression) |
| `ensures_hir` | `HirExpr` | No (same, for `ensures`) |
| `atom` | parser `Atom` | Every emitter |
| `body_stmt` | parser `Stmt` | No emitter (verification and MIR use it) |
| `effect_set` | `HirEffectSet` | LLVM (comment in the generated IR) |

HIR nodes also embed parser types: `HirExpr::BinaryOp` uses `parser::Op`,
task groups use `parser::JoinSemantics`, `HirMatchArm::pattern` is a
`parser::Pattern`, and `HirLambdaParam::type_ref` is an AST `TypeRef`. Type
names (`ty`, `type_name`, `return_type`) are strings resolved through
`ModuleEnv`.

`requires_hir` / `ensures_hir` are lowered from the full clause strings, so they
lose clause boundaries, labels, and trust modes; nothing reads them today.

### Survey of the built-in emitters

"AST reads" lists what each emitter takes from the parser types today; "HIR
replacement" names the HIR type that would provide it. Types marked *new* do not
exist yet and are introduced by the migration steps below.

| Emitter (target) | Entry point | AST reads today | Other inputs | HIR replacement |
|---|---|---|---|---|
| JSON (`verified-json`) | `mumei-emit-json::VerifiedJsonEmitter::emit` | `Atom.name`, `params` (`name`, `type_name`, `is_ref`, `is_ref_mut`), `requires`, `ensures` (strings), `effects[].name`, `return_type`, `trust_level` | — | `HirSignature` (*new*), `HirContract` (*new*) clause text, `HirEffectSet`, `HirAtomMeta.trust_level` (*new*) |
| Proof book (`proof-book`) | `mumei-emit-proofbook::ProofBookEmitter::emit` | `Atom.name`, `trust_level`, `is_async`, `params`, `return_type`, `requires`, `ensures`, `effects`, `effect_pre`, `effect_post`, `resources`; whole `Atom` for `compute_atom_content_hash_v2` | `ModuleEnv::resolve_base_type` | `HirSignature`, `HirContract`, `HirEffectSet` plus *new* effect-state and resource fields on `HirAtomMeta`; content hash carried in `HirAtomMeta` |
| Python (`python-wrapper`) | `mumei-emit-python::PythonWrapperEmitter::emit` | `Atom.name`, `params`, `return_type`, `requires`, `ensures`; contracts rewritten textually by `translate_contract_to_python` | — | `HirSignature`, `HirContract` clause `HirExpr` (print from the tree instead of rewriting strings) |
| Rust (`rust-wrapper`) | `mumei-emit-rust::RustWrapperEmitter::emit` | `Atom.name`, `params`, `return_type`, `requires`, `ensures`; contracts rewritten by `translate_contract_to_rust` | — | `HirSignature`, `HirContract` clause `HirExpr` |
| Runtime monitor (`runtime-monitor`) | `mumei-emit-monitor::RuntimeMonitorEmitter::emit`, `generate_monitor` | Whole `Atom` for `trust_boundary::classify_trust_boundaries`; `name`, `params`, `return_type`, `requires`, `ensures`, `trust_level`, `effect_pre`; contract strings filtered by the character whitelist in `monitor_condition` | `ExternBlock` (extern boundary detection), `ModuleEnv` (type resolution) | `HirSignature`, `HirContract` (clauses with mode, so a monitor can target exactly the assumed clauses), `HirAtomMeta.trust_boundaries` (*new*, computed once in core) |
| LLVM (`llvm-ir`, also `binary`, `run`, REPL JIT) | `mumei-emit-llvm::LlvmEmitter::emit` → `codegen::compile`; `codegen::compile_atom_into_module`; `binary.rs`; `jit.rs::compile_atom` | `Atom.name`, `params` (`driver.rs`), `return_type` and, when it is absent, `mir::infer_atom_return_type(&Atom)`, which re-lowers the AST (`lowering.rs::resolve_return_type`); `parser::Op`, `JoinSemantics`, `Pattern` inside HIR (`expr_emit.rs`, `pattern_emit.rs`); `binary.rs::rename_calls_in_atom` rewrites `body_expr`, `requires`, `ensures` strings | `ModuleEnv` (`EnumDef`, `StructDef`, type resolution), `ExternBlock` (FFI declarations) | `HirAtom.body` (already), `HirSignature` with a resolved return type, HIR-owned operator / join / pattern types (*new*: `HirBinOp`, `HirJoin`, `HirPattern`); renaming only on `HirStmt` |
| C header (`c-header`, in core) | `mumei-core/src/emitter.rs::CHeaderEmitter::emit` | `Atom.name`, `params`, `return_type`, `requires`, `ensures` | — | `HirSignature`, `HirContract` |

Other `HirAtom` consumers that read `hir_atom.atom` and must keep working:
verification (`verification/executor.rs::verify_inner` reads `Atom` and
`body_stmt` throughout), `src/commands/{build,run,repl,inspect,doc,verify}.rs`,
and `src/lsp.rs`. They are not emitters and are out of scope, which is why the
design keeps `HirAtom.atom` until the last step instead of removing it.

## Target shape

New HIR types, owned by `mumei-core/src/hir.rs` and built once by
`lower_atom_to_hir_with_env`:

```rust
pub struct HirParam {
    pub name: String,
    pub consume: bool,       // preserves the declared `consume` marker
    pub ty: Option<String>,   // as written; resolution stays in ModuleEnv
    pub by_ref: HirRefKind,   // Value | Ref | RefMut
}

pub struct HirDeclaredEffect {
    pub name: String,
    pub negated: bool,
}

pub struct HirSignature {
    pub name: String,
    pub params: Vec<HirParam>,
    pub effects: Vec<HirDeclaredEffect>,     // declaration order, duplicates kept
    pub return_type: Option<String>,          // as written
    pub inferred_return_type: Option<String>, // from the HIR body when absent
    pub is_async: bool,
}

pub struct HirClause {
    pub kind: HirClauseKind,   // Requires | Ensures | Cover
    pub mode: HirClauseMode,   // Plain | Assume | Check
    pub label: Option<String>,
    pub text: String,          // source text, for printing
    pub expr: Option<HirExpr>, // None when the clause cannot be lowered
}

pub struct HirContract {
    pub clauses: Vec<HirClause>,
    pub requires_text: String, // conjoined source text, for byte-compatible printing
    pub ensures_text: String,  // conjoined source text, for byte-compatible printing
}

pub struct HirAtomMeta {
    pub trust_level: HirTrustLevel, // mirrors parser::TrustLevel
    pub trust_boundaries: Vec<TrustBoundaryKind>,
    pub effect_pre: ..., pub effect_post: ..., pub resources: ...,
    pub content_hash: String,  // compute_atom_content_hash_v2 on the AST atom
    pub span: Span,            // source location only, no AST structure
}
```

`HirAtom` gains `signature`, `contract`, and `meta` next to the existing
fields. The exact field types for effect state and resources follow whatever
the AST holds today; the point is that emitters read them from HIR.

Rules the target shape must keep:

- **One contract source.** `HirContract` is built from the same clause list that
  `verification/contract_view.rs` reads, so an emitter's view of "the requires
  of this atom" is the verifier's. Emitters that need the caller-side
  contract filter clauses by mode exactly as `contract_view` does; the
  filtering helper lives in core next to `contract_view`, not in each emitter.
- **Fail closed.** A clause that cannot be lowered has `expr: None`. Emitters
  that need an expression (Python, Rust, monitor) must keep today's behaviour
  for such a clause — the monitor skips it with its existing "not lowered"
  path, wrappers print the text as documentation only — and must never treat
  a missing expression as `true`.
- **Same bytes out.** For every atom, each emitter's artifact is byte-identical
  before and after its migration step.

## Migration order

Each step is one PR, lands independently, and leaves every emitter's output
unchanged.

1. **Add the HIR metadata, read nothing yet.** Add `HirSignature`,
   `HirContract`, and `HirAtomMeta`, populate them in `lower_atom_to_hir` /
   `lower_atom_to_hir_with_env`, keep `atom` and `body_stmt`. Remove or
   repurpose the unused `requires_hir` / `ensures_hir` (their only readers are
   test constructors). Bumps `EMITTER_ABI_VERSION` (see Risks).
2. **Move the leaf emitters.** JSON, C header, and proof book read only
   metadata and contract text; switch them to `signature` / `contract` /
   `meta`. Step 1 could not reproduce their output because it lost the
   `consume` marker, exact `requires`/`ensures` text, and ordered declared
   effects. Step 2 adds those fields and bumps the emitter ABI to 3. Proof book
   takes `content_hash` from `meta` instead of hashing the AST.
3. **Move the wrappers.** Python and Rust print contract expressions from
   `HirClause::expr` with a small HIR printer instead of
   `translate_contract_to_python` / `translate_contract_to_rust` string
   rewriting. Clauses whose `expr` is `None` keep their current handling.
4. **Move the runtime monitor.** Read trust boundaries from
   `meta.trust_boundaries` (computed in core by `classify_trust_boundaries`
   from `ModuleEnv::extern_blocks`, which the lowering already receives), and clause expressions from `contract`. The
   character whitelist in `monitor_condition` becomes a check on the HIR
   expression's node kinds, which must accept exactly the same clauses as
   today; anything else is not lowered, as now.
5. **Move LLVM metadata reads.** `driver.rs` and `lowering.rs::resolve_return_type`
   read `signature`; `inferred_return_type` replaces the call to
   `mir::infer_atom_return_type(&Atom)`. `binary.rs` stops calling
   `rename_calls_in_atom` on strings and renames only in HIR
   (`rename_calls_in_hir_stmt` already exists); the renamed signature name is
   set on `signature.name`.
6. **Own the operator, join, and pattern types in HIR.** Introduce
   `HirBinOp`, `HirJoin`, and `HirPattern` with one-to-one conversions from the
   parser types, and switch `expr_emit.rs` / `pattern_emit.rs`. This is the
   largest LLVM change and is purely structural.
7. **Drop the AST from the emitter interface.** Once no emitter reads
   `hir_atom.atom` or `body_stmt`, stop exposing them to emitters: pass a
   separate `EmitAtom` view that holds only HIR to `Emitter::emit` (see
   "Decisions"). Verification and
   the CLI keep using the AST. `ExternBlock` and `ModuleEnv` are replaced by
   HIR-owned summaries only if a later need appears; they are not required for
   the goal. Bumps `EMITTER_ABI_VERSION`.

Steps 2–4 are independent of each other and of steps 5–6, so they can land in
any order after step 1.

## Risks

### Plugin ABI compatibility

- Adding fields to `HirAtom` (step 1) changes its layout. A plugin compiled
  against the old `mumei-core` and loaded by a new host would read the struct
  with the wrong layout. The version check guards only the entry points, so
  every step that changes a type reachable from `Emitter::emit` must bump
  `EMITTER_ABI_VERSION`; old plugins then fail to load with the existing
  "ABI version mismatch" error instead of misbehaving.
- Three bumps are planned (steps 1, 2, and 7). Step 2 added `consume`,
  `requires_text`/`ensures_text`, and ordered declared effects because the
  step-1 HIR shape could not reproduce leaf-emitter output, and bumped the ABI
  to 3. If step 6 changes `HirExpr` variants separately, it requires another
  bump; grouping steps 6 and 7 keeps the plan to three.
- Plugins that read `hir_atom.atom` keep working until step 7. Step 7 is a
  breaking change for them and needs a note in [`PLUGIN_GUIDE.md`](PLUGIN_GUIDE.md)
  and the changelog, with the HIR fields to use instead.
- A stable C-level ABI (serialising the HIR across the boundary) is a
  separate decision and not part of this plan.

### LLVM code generation

- `mir::infer_atom_return_type` re-lowers the AST and returns a type name that
  decides the LLVM function signature. If `inferred_return_type` is computed
  differently, the emitted IR changes. Step 5 must compute it with the same
  function, called once during lowering.
- `binary.rs` renames `main` to `__mumei_user_main` both in HIR and in the
  `Atom` strings. Codegen must not read the strings anywhere after step 5, or
  a self-recursive `main` would call the C wrapper.
- Pattern and enum codegen depend on `ModuleEnv` enum/struct definitions
  (`pattern_emit.rs`, `lowering.rs`). Step 6 keeps `ModuleEnv` as the source of
  layouts; only the pattern syntax moves to HIR.
- JIT and REPL compile atoms through the same `compile_atom_into_module`; any
  change there affects `mumei run` and `mumei repl`, not only `--emit llvm-ir`.

### Contract semantics

- Wrappers and monitors currently print `Atom.requires` / `Atom.ensures`, which
  include `assume` and `check` clauses alike. Moving to `HirContract` makes the
  mode visible. Whether a wrapper should assert `requires assume` clauses at
  runtime, or a monitor should check `ensures check` clauses, is a behavioural
  decision; the migration keeps today's behaviour (all clauses, as printed
  now) and leaves any change to a separate PR. The policy that PR follows is
  under "Decisions" below.

## Tests that guard each step

The baseline guard for every step is "artifacts unchanged". Before step 1, add
golden-output tests that build a small fixed set of atoms (scalar, array,
struct, enum + match, effectful, trusted, extern, clause modes, `main`) for
every built-in target and compare the artifacts byte for byte; each later step
must keep them passing without updating the goldens.

| Step | Existing tests that must stay green | New tests |
|---|---|---|
| 1 | `cargo test -p mumei-core` (HIR lowering, `emitter.rs` unit tests including `test_emitter_abi_version_constant` and `test_emitter_plugin_handle_round_trips_boxed_emitter`), `tests/test_add_emitter.rs` | HIR metadata matches the AST for every field; a plugin built for the previous ABI version is refused by `load_external_emitter_from_path` |
| 2 | `mumei-emit-json` and `mumei-emit-proofbook` unit tests (including `test_content_hash_matches_proof_cert`), `CHeaderEmitter` unit tests in `emitter.rs` | Goldens for `verified-json`, `c-header`, `proof-book` |
| 3 | `mumei-emit-python` (`test_contract_translation`) and `mumei-emit-rust` unit tests | Goldens for both wrappers, including a clause that cannot be lowered |
| 4 | `mumei-emit-monitor` unit tests (`contracts_outside_the_expression_subset_are_not_lowered`, `proven_pure_atom_emits_no_monitor`), `tests/test_runtime_monitor.rs` | Every clause the old whitelist accepted is accepted by the HIR check and no other |
| 5 | `mumei-emit-llvm` unit tests, `tests/test_codegen_*.rs`, `tests/test_lambda_codegen.rs`, `tests/test_run.rs`, `tests/test_repl.rs` | IR goldens for atoms without an explicit return type; self-recursive `main` in a binary build |
| 6 | Same as step 5, plus `tests/test_concurrency.rs`, `tests/test_match_arm_scoping.rs`, `tests/test_pattern_lowercase_qual.rs` | Conversion round-trip from parser `Op` / `JoinSemantics` / `Pattern` to the HIR types for every variant |
| 7 | Everything above, `tests/test_add_emitter.rs`, the plugin example in `PLUGIN_GUIDE.md` | A compile-time check that no `mumei-emit-*` crate imports `mumei_core::parser` outside tests |

The `Verify Standard Library` workflow is a useful end-to-end signal for steps
5–6 because it builds the compiler, but it does not compare emitted artifacts;
the goldens above are what actually guard output.

## Decisions

These were open questions in the first draft of this design. The answers
below fix the direction; each behaviour change still lands in its own PR.

- **Emitter view (step 7).** Add a separate HIR-only `EmitAtom` view and
  hand that to `Emitter::emit`, instead of removing `atom` / `body_stmt` from
  `HirAtom`. Steps 1–6 add the HIR fields next to the AST, every emitter
  moves one at a time with byte-identical output, and the AST leaves the
  emitter interface only at step 7. External plugins see the AST disappear
  in that single breaking step.
- **Covers in `HirContract`.** `HirContract` carries `cover` clauses as
  `HirClauseKind::Cover`. JSON and the proof book may record or print them.
  Wrappers and the monitor ignore them: a cover is a reachability query
  answered at verification time, not a condition that must hold at runtime,
  so it must never become a runtime assertion.
- **Clause trust modes in wrappers and monitors.** Once step 3 or 4 has made
  `HirClause::mode` visible, a separate PR applies this policy:
  - Wrappers (Python, Rust) keep checking every `requires` clause at runtime,
    whatever its mode, because their callers are unverified.
  - The monitor adds runtime checks for `requires assume` and
    `ensures assume` clauses. Nothing proves them statically, so they are
    trust boundaries in the sense of [`VERIFIER_SPEC.md`](VERIFIER_SPEC.md).
  - `ensures check` clauses are proved inside the atom and are treated like
    plain `ensures` clauses.

  Until that PR lands, every emitter keeps today's behaviour (see "Contract
  semantics").
