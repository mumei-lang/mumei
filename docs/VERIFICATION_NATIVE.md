# Verification-Native architecture and acceptance contract

The [cross-project roadmap](CROSS_PROJECT_ROADMAP.md#verification-native-program-ordered-implementation)
owns implementation order. [The program tracker](https://github.com/mumei-lang/mumei/issues/784)
links C01–C14. This document defines responsibilities and acceptance boundaries,
not a second priority list or a claim that planned features already exist.

## 1. Goal: common specifications, not a universal verification language

Mumei evolves from primarily Z3-backed contract verification toward a common
specification language connected to multiple formal verification methods, while
retaining compilation and execution. Adding external-tool launchers to Agent
alone would not achieve this goal: the specification's meaning, assumptions,
verification obligations and transformation boundaries must also be explicit.

Keep the common core small: typed values, pre/postconditions, obligations,
assumptions, source provenance and explicit semantic settings. Domain-specific
profiles may add memory/permission, temporal/state, resource or concurrency
properties with their own semantics and backend capability requirements.
Do not take the union of every backend language or reduce every specification
to the weakest common denominator. A backend can reject an unsupported profile.

A common specification does not imply every backend can prove every property.
Each transformation must disclose whether it preserves the requested property,
uses a conservative approximation, needs assumptions, is unchecked or is
unsupported. Splitting an obligation also requires a justification that the
resulting obligations imply the original property. Unsupported clauses must
not disappear or be weakened silently.

Executable implementations and non-executable proof models are distinct.
Not every specification, quantifier or ghost/model construction must compile
to LLVM. The relationship between a proof model and an implementation must
itself be identified and, where claimed, checked.

## 2. Ownership and trust

| Component | Owns | Does not imply |
| --- | --- | --- |
| Mumei | Language, typed semantic/specification core, standard static/Z3 verifier, obligation contracts, common evidence/acceptance rules, LLVM emission | Every external verifier's implementation, every logic, or compiler correctness |
| Mumei Agent | Extraction/analysis, capability-based planning, budgets, adapters, execution/repair and evidence collection | LLM-controlled proof acceptance or a private grade system |
| mumei-lean | Lean translation, lemmas, proof/kernel/axiom checks, existing escalation and opt-in dual checks | Ownership of all verification or automatic source/native equivalence |
| LLVM backend | Target lowering, native generation, runtime/ABI integration | A proof of Mumei contracts or semantic preservation merely because compilation succeeded |
| mumei-demo | Reproducible scenarios, negative controls, consumer checks and honest evidence presentation | Production suitability or stronger guarantees than the collected evidence |

Existing audit keys, Claim Book G0–G5, intent fidelity, harness contracts,
artifact paths, budget fingerprints, dependency freshness and translator/lemma
hash checks remain authoritative. The initial schema adds no grade or promotion
rule. Unknown, timeout, partial translation, stale translators, parser failures,
ordinary audit findings and toolchain failures are not successful proofs.
Keep ordinary Lean escalation and opt-in dual checking distinct: the latter
does not rewrite the original Z3 result or independently erase assumptions.

### Explicit non-goals and execution boundary

| Avoid | Reason and required alternative |
| --- | --- |
| Implementing every logic inside Mumei | Unbounded language/verifier complexity; use a small core and explicit domain/backend contracts |
| Embedding every external tool in the compiler | Dependency and operational bloat; external adapter installation, selection and execution belong to Agent |
| Mapping external success directly to proved | Different scopes and assumptions; normalize evidence, then apply independent acceptance/trust rules |
| Making Semantic IR universal | A universal representation can hide semantic loss; keep backend-specific information and disclose transformation gaps |
| Running Z3 and external tools unconditionally in parallel | Uncontrolled cost and result reconciliation; use purpose-driven plans with explicit user policy, budget and compatible obligations |

Mumei's existing standard static/Z3 verification remains built in. The common
contract does not turn the compiler into an external-process orchestrator.
Agent owns external-tool selection, invocation, cancellation, resource control
and result collection. Mumei owns specification meaning and shared evidence
acceptance rules; backend-specific proof validation stays with the appropriate
trusted checker. Collecting a successful tool result is not accepting a proof.

Independent comparisons, dual checks and parallel runs may be useful when
explicitly requested or permitted for a defined purpose and budget, with
matching target/semantics/assumptions and a disagreement policy. They are not
the default consequence of installing multiple plugins. Existing opt-in Lean
dual checks are preserved, not changed into always-on parallel verification.

## 3. Extend the existing foundation

| Foundation | Existing basis | Added value |
| --- | --- | --- |
| Fragment classification | Logic fragment tags, Z3 result classes, nlsat strategy and Lean escalation metadata | Obligation-level requirements, compatibility and budgeted planning |
| Intermediate representations | HIR/MIR, HIR-only emitter view and Lean Translator IR | Explicit shared meaning and transformation relations, not another universal IR |
| Evidence | Proof certificates, bundles, Claim Book, freshness and semantic-gap diagnostics | Backend-neutral scope/provenance and implementation/artifact correspondence |
| Analysis | Tree-sitter function/expression/statement extraction; Go function-local path-sensitive must/may facts | Incremental source-mapped CFGs, per-language type/binding information, then resolved-call summaries |
| Foreign-code checks | Body ports and finite native replay | Separate proof of a port from correspondence to original code; replay remains finite observation |
| Execution | LLVM/native build and run paths, extensible emitters | Build-time evidence binding, then limited translation validation and clean-consumer checks |
| Evaluation | Charter, controls, observations, scenario evidence and missing/stale-artifact rejection | Cross-backend and native assurance regression cases |

HIR/MIR existing in the compiler does not prove agreement between Z3 lowering,
LLVM lowering and final machine code. In particular, the Z3 body-lowering path
and HIR-only emitter boundary need an explicit semantic correspondence contract.

## 4. Initial evidence contract (C02/C13)

The standalone [schema](../schema/verification-evidence.schema.json) is the
initial representation contract, not a change to proof-certificate output.
Its committed examples are **synthetic contract fixtures**, not generated
proofs or a successful external-verifier integration. Run:

```bash
python3 -m pytest tests/test_verification_evidence_schema.py tests/test_proof_cert_schema.py -q
```

The record separates:

- **Subject and specification:** content identity and whether the target is
  source, a model, lowered IR or an executable artifact.
- **Obligation and semantics:** property identity, integer model, semantic
  profile and target when relevant. A generic profile name is not a formal
  definition; later profiles need a versioned, documented meaning.
- **Method and result:** deductive proof, abstract interpretation, bounded
  model checking, finite execution, heuristic analysis or identity checking.
  A tool's completed process is independent of the obligation's result.
- **Scope and assumptions:** bounded/finite/identity/unbounded scope, explicit
  limits, assumptions and exclusions. Unbounded scope never means the whole
  real-world system is proved.
- **Correspondence:** the relation and its checked/assumed/unverified status.
  A proof on an unchecked port remains evidence about that port, not the
  original implementation.
- **Provenance:** adapter/tool/protocol versions, non-secret configuration
  fingerprint and optional translator/lemma identifiers, plus evidence paths.

The schema rejects inconsistent combinations such as a timeout that claims an
established result, finite tests labeled as a deductive success, or bounded
checking without recorded bounds. It does not prove the truth of the result,
validate hashes against actual files, check freshness, authenticate a producer,
or validate semantic correspondence. Consumers must still check all relevant
trust, identity, coverage and acceptance rules before accepting a claim.

Abstract-interpretation warnings use `result: finding` while retaining
`method: abstract_interpretation`. Over-approximation warnings can be spurious;
they are not concrete counterexamples or proofs. Any separately validated
concrete witness needs its own method/scope evidence, not a promotion of the
warning or a relabeling as a static heuristic.

No raw environment, credentials, remote endpoint, invocation arguments or
arbitrary tool configuration belong in this shared record. A fingerprint is
over an approved non-secret configuration projection, not secret values or
credential-derived material. Referenced tool artifacts also require safe
handling; a path is not proof that the referenced contents are safe.

Producer integration, consumer validation and mapping to existing claim
evidence are later C02 slices. A plugin cannot self-assert a new grade or use
this schema to bypass Lean/kernel or current certificate checks.

## 5. Minimal semantic contract (C03)

Start with a pure integer/Boolean function: for a signed 64-bit input, return
zero for negative values and the input otherwise. Fix the precondition,
postcondition, body, typed binders, result binding and source spans. This
example needs no heap, effects, FFI, quantifiers or concurrency and can avoid
overflow while still exposing conditional/result-binding translation errors.

The initial design contract must identify:

1. The original specification and implementation hashes, source locations,
   typed binders, precondition and postcondition.
2. The obligation and original conjunction/quantifier structure. Any
   normalization, substitution, decomposition or approximation has a recorded
   relation to the original property.
3. Numeric interpretation: mathematical integers versus fixed-width signed or
   unsigned values; checked versus wrapping arithmetic; division/remainder,
   shifts and conversion behavior. A mathematical proof cannot silently
   certify machine arithmetic. Existing division/remainder semantics also have
   a separate [open correctness issue](https://github.com/mumei-lang/mumei/issues/781).
4. Existing HIR/MIR and Translator IR identities and transformation versions,
   with backend requirements and unsupported constructs.
5. The selected solver/proof route, declared assumptions, emitted LLVM target
   and the exact boundary of any correspondence claim.

### First supported scalar slice

`mumei_core::semantic_contract::extract_pure_i64_contract` is an opt-in library
API for `mumei.pure-i64-comparison/v1`. It accepts exactly one atom with explicit
owned `i64` parameters/return, integer constants, Boolean conditions, signed
comparisons and conditional expressions. It uses existing checked parsing and
HIR/MIR lowering, not a new grammar, solver, evaluator or universal IR.
Arithmetic (including overflow), division, shifts, conversions, calls, heap,
FFI, mutation, local bindings, quantifiers, trust overrides and effect/ownership
annotations are rejected rather than silently dropped. Boolean values are
conditions in this first profile, not public Boolean parameters/return values.
Checked lexing rejects failed numeric conversions and nonfinite float literals
before parser recovery or text reconstruction can replace them with zero or an
identifier. The existing integer literal representation is `i64`, independently
of parameter types: this also rejects a single oversized literal in a `u64`
contract. It does not restrict `u64` variables to the signed range; larger
mathematical constants can currently be built from in-range literals and
unbounded contract arithmetic. General width-aware literal representation is
still future work, not silently supplied by this profile.

Raw float tokens, including integer-valued, underflowing
and overflowing float literals, are rejected by this profile before item text
reconstruction can erase their category. This is not an exact-decimal or IEEE
floating-point validation profile. The current `-9223372036854775808` spelling is
also rejected because its positive magnitude is not representable by the lexer;
this does not exclude that input value from the function's typed input domain.
Legacy tolerant parsing is not changed into a validation API by this slice.

The serializable description binds full-source, specification and implementation
hashes; typed/result binders with explicitly atom-level span precision; the
original parsed contract text; obligation identity; numeric semantics; existing
Translator IR/translator/lemma metadata; and compiler-local HIR/MIR fingerprints.
The parser's comparison-chain normalization is disclosed, not independently
certified. The HIR debug and MIR JSON `v0` fingerprints are opaque diagnostics,
not a stable interchange format, cross-version cache key or equivalence proof.
`compiler_version` alone is not an exact compiler/toolchain build identity.

An optional `requested_target_triple` is a caller request, never a binding to
emitted code or a capability/availability claim. `None` serializes as null. The
existing LLVM-IR emitter does not bind a target triple in the emitted module;
actual target/build/artifact binding remains C08 work. The recorded standard-Z3
route is requested only: extraction does not invoke Z3, Lean or LLVM, establish
precondition satisfiability, or mark any result as proved. Neither current
certificate serialization nor Claim Book grades are changed.

The real CLI integration regression uses the clamp fixture to:

- generate/re-check an actual Z3 certificate and match its content/contract and
  Translator IR goal with the extracted description;
- generate LLVM with the existing build path and observe native results at
  `i64::MIN`, -1, 0, 1 and `i64::MAX` using a separate fixed C driver;
- reject an intentionally reversed LLVM signed comparison under that same
  finite driver, a false postcondition under the unchanged body, and a defective
  body under the unchanged postcondition.

These native observations are finite regression evidence, not universal LLVM
translation validation or binary correctness. Correspondence remains explicitly
unchecked even when they pass. General obligation decomposition, production
sidecar emission/consumer acceptance, stable IR contracts and formal translation
correspondence remain open C02/C03/C08/C09 work.

```bash
cargo test -p mumei-core semantic_contract --lib
cargo test --test test_semantic_contract
```

## 6. Replaceable planner and verifier plugins (C04/C11)

Design the extension boundary before selecting external verifiers. Users must
be able to register, choose, configure, disable and replace adapters, including
their own adapters, without editing planner core. This is independent of which
tools the project eventually ships or supports by default.

| Boundary | Required contract |
| --- | --- |
| Planner core | Match obligations and required assurance to registered capabilities, compatibility, user policy and budgets; keep tool commands/parsers out of core |
| Capability registry | Adapter identity, adapter/protocol version, accepted inputs, semantic profiles, logic/domain support, result meanings and constraints; registration is not availability or trust |
| User configuration | Enable/disable, explicit versus automatic selection, candidate ordering, tool version/location, tool-specific settings, timeout/resources and permitted fallback; define precedence and show effective settings |
| Adapter lifecycle | Separate support/preflight checks, input preparation, execution/cancellation and result normalization; preserve native evidence and tool-specific semantics |
| Explanation | Show selected/excluded candidates and reasons, transformations, limits, effective non-secret settings and fallback history |
| Trust/acceptance | Backend self-reports do not define proof acceptance; changing an adapter does not imply equivalent semantics, coverage or assurance |
| Execution policy | User/admin-controlled installation and enablement; no LLM-driven arbitrary code/plugin downloads; authorize file/network/remote data access and bound runtime resources |
| Reproducibility | Protocol/tool/adapter versions, input identities and approved non-secret settings fingerprint; reject incompatible versions, unknown settings and malformed/partial results |

Begin with deterministic routing and existing Z3/Lean paths plus test adapters.
LLMs may suggest plans but cannot override acceptance rules. An explicitly
selected missing or unsupported tool reports that reason. Fallback is permitted
only by user policy, is recorded, and must not lower a required guarantee.
Planning must not execute every registered backend. Parallel/comparison plans
require an explicit purpose, permitted user policy, budget and reconciliation
rules, and must remain distinct from ordinary single-route verification.
Unknown, timeout, unavailable toolchain, unsupported translation and unresolved
proof goals need different diagnostics/remedies rather than one retry loop.

Conflicting results must not be resolved by picking the successful one. First
check that tools saw the same obligation, semantics, inputs and assumptions;
preserve unresolved disagreement for investigation.

"Plugin" describes the extension boundary, not a decision to use shared
libraries. Subprocess, container and explicitly configured remote transports
are possible later choices. Do not reuse the emitter-plugin ABI by assumption.

Acceptance includes registering and replacing a test adapter without core
edits; explicit and automatic selection; disablement; stable budgets;
incompatible/absent tools; unknown settings; denied/permitted fallback;
timeouts; malformed, missing and conflicting results; secret-free diagnostics;
and no promotion from an untrusted adapter's claim.

## 7. Analysis expansion (C05–C07)

Connect CFGs to the existing Go facts incrementally. Preserve source spans,
must/may distinctions, guarded division/bounds, nil/resource state, early
returns, reassignment, shadowing and loop handling. Unsupported constructs stay
visible and conservative; do not rewrite the entire analysis first.

Next extend one language at a time with type widths, binding identity,
imports/callees and language-specific semantics. Tree-sitter provides syntax,
not complete type/alias/dispatch resolution; compiler information can supply
the missing semantics. Only then add summaries over resolved calls. Missing
call-graph edges, recursion, FFI and dynamic dispatch cannot be assumed safe.
Measure false positives and missed defects independently.

## 8. Execution and assurance stages (C08–C10)

| Stage | Supports | Does not establish |
| --- | --- | --- |
| Evidence-bound native bundle | Source/spec/dependency/certificate/artifact identity, recorded build conditions and freshness | Semantic correctness or producer authenticity |
| Translation-validated subset | Property correspondence between the specified model and explicitly identified LLVM IR under declared semantics | Unchecked optimizations, target backend, machine code, linking, FFI or runtime |
| End-to-end assurance (longer term) | A checked or explicitly trusted chain to final artifacts for a specified property | Arbitrary program/environment correctness |

Capture source/contract/dependency/proof/artifact hashes, relevant semantic and
LLVM IR hashes, compiler/linker/LLVM versions, optimization and meaning-affecting
settings, target triple, runtime/ABI, build records, verification scope and
recheck instructions during the build. Consumers reject stale, missing or
replaced members; arbitrary files paired afterward are insufficient.

Hashes prove consistency only. An attacker able to replace the whole manifest
requires a separate authenticity/signature/attestation policy. Reproducible
builds are useful but are not semantic preservation proofs.

For translation validation, first fix a pure numeric/Boolean subset, target
and optimizer profile. Account for overflow, division, shifts, LLVM undefined
behavior and poison. Exclude pointers, aliasing, FFI, effects and concurrency
initially; reject rather than promote unsupported cases. A pre-optimization IR
check cannot automatically certify post-optimization IR or the final binary.
Existing source/Z3/Lean bridge lemmas do not already cover LLVM generation.

## 9. External verifier candidates, not adoption decisions

Before any adoption check current maintenance, license/distribution/commercial
terms, CI reproducibility, language/LLVM/toolchain compatibility and operating
cost. Evaluate one concrete use case at a time through the plugin boundary.

| Candidate | Possible role and trigger |
| --- | --- |
| Kani | Early Rust audit feasibility pilot; preserve harness assumptions, unwinding assertions and unsupported features. A requested unwind bound alone is not success. |
| SAW/Crucible | Limited LLVM implementation/specification correspondence pilot; first check compatibility with emitted IR and numeric semantics. |
| Viper | Memory/heap/permission model study; source-to-permission-model correspondence is necessary. It is not simply a Z3 replacement. |
| TLA+/TLC | Concrete protocol/concurrency/state-transition demand; preserve finite state scope and fairness, and distinguish model from implementation. |
| Frama-C Eva/WP | C audit demand; distinguish sound abstract analysis from deductive contract verification. Agent's present Layer B front door does not include C. |
| CodeQL/Semgrep | Supplementary findings and paths; no warnings is not a contract proof. Check use/distribution conditions. |
| Dafny/F* | Demand-led contract/effect/termination work only after showing incremental value and transformation cost beyond existing Z3/Lean. |
| Rocq/Coq, Isabelle/HOL, Agda | Deferred proof-backend diversification; first improve existing Lean translation quality. |
| Prusti | Maintenance/toolchain investigation before a Rust permission-verification pilot. |
| dReal | Specific nonlinear-real/transcendental demand; unsat concerns the original formula, while delta-sat concerns a delta-relaxed formula. Preserve delta/domain and real versus machine-number distinctions. |
| KeYmaera X | Concrete differential-equation/hybrid-control demand, not general-purpose initial audit scope. |
| Additional SMT solvers | Measured fragment/performance/independence need; extra solvers do not fix incorrect specifications or translations. |

These are alternative evaluation directions, not a fixed product integration
sequence. Planner design must not depend on adopting any of them.

## 10. Effects, authority and deferred work (C12/C14)

Extend existing Effect/Resource/Ownership/Temporal/Capability Stage 1 models
only with explicit host/FFI and real-authority correspondence. A proof about
allowed effects does not automatically certify the host's actual permissions,
actor identity, policy or resource handling. Dynamic delegation/revocation
remains demand-gated, not activated by this program.

WASM remains deferred/TODO behind native evidence/runtime stability and
concrete demand. A future design must specify target, WASI/host ABI, runtime
dependencies, capability boundaries, sandbox assumptions and evidence recheck.
Adding an output format is not verified WASM.

## 11. Regression and delivery rules

- Keep fixed-contract buggy/fixed pairs and test weak contracts, unreachable
  assumptions, incorrect body ports, guard invalidation, shadowing and callee
  replacement. Count false acceptance before optimizing success rates.
- Check missing/replaced/stale certificates, dependencies, targets, build
  settings and artifacts, and unsupported/older evidence versions.
- Exercise plugins' selection/configuration/version/fallback failure cases.
  A partial tool result must not make a complete claim succeed.
- Display proof/model evidence, finite observations, assumptions, exclusions
  and unverified connections together. Keep audit/Claim Book semantics intact.
- Report false positives, unsupported/inconclusive results, environment
  failures, supported-obligation/translation coverage, time and compute budget
  separately. Specification weakening or reduced scope is not a solver win.
- Deliver initial contracts and synthetic fixtures before external adapters.
  Later producers, consumers, semantic vertical slices, real planner routing,
  CFGs and native checks remain open until their own acceptance tests pass.
