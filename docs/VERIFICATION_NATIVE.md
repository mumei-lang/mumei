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

The first local producer connection is the success-only
[`native-bundle recheck-llvm --json` report](#first-native-consumer-evidence-producer).
Further producer integration, trust-aware consumer validation and mapping to existing claim
evidence remain later C02 slices. A plugin cannot self-assert a new grade or use
this schema to bypass Lean/kernel or current certificate checks.

### Shared Core representation gate

`mumei_core::verification_evidence` provides the common v1 owned record model,
`decode(&[u8])` and `Evidence::validate()`. This is a runtime representation gate
for the existing schema, not an acceptance engine. It does not define new logic,
semantic profiles, verifier support or an executable universal IR.

`decode` accepts at most 65,536 bytes and checks closed JSON shape plus the
schema's vocabulary, nonempty text, lowercase SHA-256 syntax, unique/nonempty
list entries and method/execution/result/scope/correspondence combinations.
Bounded/finite scopes require explicit bounds; unbounded/identity scopes forbid
them. Timeouts/errors cannot claim conclusive success; finite observations,
abstract warnings and identity consistency retain their distinct meanings.
Required nullable fields still have to be present: `semantics.target_triple`
may be null, but may not be omitted. Optional translator/lemma metadata retains
the schema's allowance for absent/null/value inputs; absent and
null normalize to `None`, omitted on serialization.

Serde deserialization alone checks shape. Call `Evidence::validate()` on
constructed/deserialized records, or use `decode` for both checks. The byte cap
is a decoder resource policy, not a new JSON Schema rule; callers constructing
records in memory are responsible for their own resource limits. Static errors
and opaque record Debug formatting do not echo arbitrary metadata. Serialization
is not a secret scrubber: producers must still use approved non-secret fields.
Artifact paths and strings remain untrusted labels, never file/tool selectors.

The native producer and saved-report consumer now use this shared model and
gate while retaining their envelope, three-record order, independently trusted
build-ID pin, fresh queries and complete same-profile report comparison. Producer
JSON is unchanged. Replay now rejects an omitted nullable target, correcting the
previous permissive decoder to the already-required v1 schema field. Optional
common provenance fields can be absent/null; non-null additions must still match
fresh local provenance. Representation success is not trust, freshness,
authenticity, historical proof execution, semantic correspondence or truth,
proof acceptance, grade/Claim Book/Lean promotion or final-artifact assurance.

The shared synthetic matrix oracle covers 960 outcome/scope combinations and
192 correspondence combinations. Python recomputes it using the unchanged JSON
Schema; Rust checks the same cases without a Python/runtime-schema dependency.
Existing synthetic and native shapes, nested required/unknown fields, duplicates,
digests, lists, input bounds, optional metadata and privacy have separate gates.
These are representation checks only. Generic acceptance/mapping and additional
real producers/consumers remain open; no external verifier or WASM is enabled.

### Standalone representation-only CLI consumer

```sh
mumei evidence check record.json
mumei evidence check record.json --json
```

This opt-in command uses the shared Core gate on **one common v1 record**, not
the three-record native report envelope, a proof certificate, a JSON array or
JSON Lines. It is an integration entry point for representation checking without
a Native bundle or a Rust caller, not a verifier/adapter execution path or a
general acceptance engine. It does not load project configuration/proof caches,
read record-selected artifacts, check hashes against files, authenticate a
producer, select a verifier or execute anything mentioned in the record.

The input must be a regular file with at most 65,536 bytes. Symlink/nonregular
inputs, malformed/non-UTF-8/trailing/deep JSON, duplicate/escaped keys, unknown
fields, missing nullable targets and invalid v1 combinations fail closed. The
bounded file reader is shared with Native consumer inputs without changing their
limits or diagnostics. On Unix it also uses `O_NOFOLLOW | O_NONBLOCK` for the
final component and checks the opened file type; this is not a claim of general
race-free/path-confinement handling on every platform or parent directory.

Exit **0** means only representation-valid, including valid records describing
counterexamples, findings, unknown, timeout, unsupported or unchecked results.
It never asserts that the reported result is true or applies to the files on
disk. Exit **1** means input/representation/output failure. Input rejection emits
no stdout, including with `--json`; diagnostics contain no input values or paths.
Clap usage errors retain exit 2. There is no solver-inconclusive exit 3 because
this command makes no solver query.

Success text and JSON are static, anonymous diagnostics, identical across valid
inputs. JSON has `schema_version: mumei.evidence-representation-check/v1`,
`evidence_schema_version: mumei.verification-evidence/v1`, `representation: valid`,
`acceptance: not_evaluated` and `checks_not_performed` listing claim truth, file
identity, freshness, producer authenticity, semantic correspondence and artifact
semantics. It intentionally contains no subject, hash, provenance, raw report,
acceptance decision or grade; it is **not a record-bound/reusable Evidence,
certificate or acceptance receipt**, and is not accepted as an Evidence input.
Native `recheck-evidence` remains a distinct fresh/pinned same-profile operation.

This CLI checks representation only. Truth/trust/freshness, existing
certificate/Claim Book/Seal Grade/Lean gates, general acceptance/mapping and
final-artifact assurance remain separate/open. No external verifier, new
environment variable, proof grade or WASM support is introduced.

### Standalone fresh source Evidence producer

```sh
mumei evidence verify-source tests/fixtures/semantic_contract_clamp.mm
mumei evidence verify-source tests/fixtures/semantic_contract_clamp.mm --json > source-evidence.json
mumei evidence check source-evidence.json --json
```

This opt-in **producer**, separate from the representation-only `check`, runs a
fresh linked Z3 query through Core's existing bounded C03 bitvector encoder.
It accepts one pure-i64 comparison/conditional atom with explicit owned i64
parameters/return, at most 16 parameters and a **literal `true` precondition**.
Exactly one explicit, nonempty `requires:`, `ensures:` and `body:` clause is
required. Parser-default `true`, comments, empty or repeated clauses cannot
substitute for the written contract; explicit `ensures: true` remains a valid
tautological postcondition, not evidence for a stronger property.
False and nonliteral preconditions are rejected before querying, not accepted
by vacuity. Arithmetic, calls, imports, effects, heap, FFI, quantifiers and
concurrency remain unsupported. The encoder also bounds source tokens (256),
nesting/expression depth (48) and accepted syntax. Input uses the same 65,536-byte
regular-file reader as `evidence check`; symlinks/nonregular inputs are rejected.

`--solver-timeout` is milliseconds (default/max 10,000), and
`--solver-resource-limit` is Z3 resource units (default/max 1,000,000). Both
must be positive; zero/unlimited and above-profile budgets fail closed. No
project configuration, ambient feature-policy settings, proof cache, existing
certificate or native bundle is loaded. The Core API is
`mumei_core::source_evidence::verify(source, Options)`; the representation gate
remains independent and does not run Z3.

Only a fresh `unsat` counterexample query yields exit 0 and, with `--json`,
**one common v1 record**, not a native envelope or certificate. `sat` yields
exit 1, solver `unknown` (including resource exhaustion) yields exit 3, and
invalid input/profile/budget/I/O yields exit 1. All non-success paths have
empty stdout and static diagnostics; there is no partial record or model dump.
Text success is static and explicitly says acceptance is not evaluated.

The record binds the exact source bytes and normalized C03 specification by
SHA-256, uses constant source/specification labels rather than paths, source
names, binders or raw expressions, and records the actual linked Z3 version
and a fingerprint of producer/semantic/encoder versions, budgets and tool
configuration. No artifact paths are emitted or followed. Method is
`deductive_proof`, execution `completed`, result `established`, and scope
`unbounded` **over all representable signed-i64 inputs**, not arbitrary-precision
integers or unbounded syntax. The source-to-model correspondence is explicitly
`assumed`: the restricted parser/normalization/encoder and solver are trusted,
not independently proved. This is not a replay of the general verifier pipeline
and does not prove correspondence to that pipeline or LLVM lowering.

A saved record is unauthenticated metadata, **not an acceptance receipt**;
`evidence check` on it still checks only representation. The restricted fresh
consistency consumer below does not authenticate it or establish historical
freshness/execution.
Seal Grades, Claim Book, certificate serialization/trust, Lean checks and Native
identity/recheck/replay semantics are unchanged. LLVM correspondence, optimizer,
object/linker/runtime/ABI/OS-exit/final-executable semantics and general common
acceptance/mapping remain open. This is partial C02/C13 real-producer integration;
it neither completes C02/C10 nor adds an external verifier or WASM.

### Standalone source Evidence consistency recheck

```sh
mumei evidence recheck-source source-evidence.json --source source.mm
mumei evidence recheck-source source-evidence.json --source source.mm --json
```

`mumei_core::source_evidence::recheck(source, saved_bytes, Options)` and this
opt-in CLI connect the standalone producer to a bounded consumer. The common
representation gate first decodes exactly one record. The existing producer
then runs **a fresh query** from the independently supplied source and explicit
timeout/resource budgets. Only fresh unsat plus equality of **every decoded
field** with the freshly produced record succeeds. Source/specification hashes,
method/result/scope, assumptions/exclusions, assumed correspondence, actual
linked-Z3/package versions, configuration fingerprint and empty artifact paths
must all agree. JSON whitespace/object-key order does not matter; list order and
all field values do. Budget flags have the producer's defaults and must be set
explicitly to its original values when nondefault; saved metadata never chooses
the query, budget, profile, verifier, input file or configuration.

Both inputs independently use the 65,536-byte regular-file reader; the source
also retains every C03 producer restriction, including explicit nonempty
contracts and literal-true preconditions. Native report envelopes, certificates,
diagnostics, malformed/duplicate/unknown-field records and mismatched versions
or provenance are not compatible saved input. No record-selected path is opened
or executed. Project config, proof caches, claims and external tools are unused.

The Core result is `Consistent`, `Refuted` or `Inconclusive`, with static errors
for malformed/unsupported/mismatched inputs; it does not return stored Evidence.
Fresh sat yields CLI exit 1; unknown/resource exhaustion yields exit 3. Invalid
input/profile/budget, mismatch and I/O yield exit 1. Non-success paths emit no
stdout, including JSON mode. A rebound source hash and forged stored success
cannot replace the fresh query. Refuted/inconclusive describes that source
query, not validation of an unrelated stored report.

Success is only a **same-profile snapshot/record consistency** diagnostic. Text
and JSON are static and anonymous: JSON uses
`schema_version: mumei.source-evidence-recheck/v1`, `recheck: consistent`, the
restricted scope and `acceptance: not_evaluated`, with explicit exclusions.
It has no subject/digest/provenance and is not reusable Evidence, a certificate
or an acceptance receipt; neither common `check` nor this recheck accepts it as
Evidence. It does not prove historical producer execution/authenticity, binary
provenance, parser/encoder/solver correctness, independent model correspondence,
LLVM/object/linker/runtime/ABI/final-executable semantics, or general Evidence
acceptance. Grade/Claim Book/Lean and Native consumer policies are unchanged.
Wider producers/consumers and C02 acceptance/mapping remain open; external
verifier adoption and WASM remain separate/deferred.

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

Agent's opt-in metadata planner has a first exact result-contract slice:
`ResultContract` pairs one profile/method with scope, complete bound/assumption
identifiers and explicit correspondence relation/status. A constrained request
requires one equal registered offer; missing declarations are not wildcards and
fields cannot be combined across offers. Explicit selection, priority and
permitted preflight fallback preserve the request. No implication, scope/bound
ordering, assumption discharge or correspondence promotion is inferred.
Unconstrained queries retain their prior payload/behavior, not an implicit
assurance guarantee. Host/profile identifiers are routing metadata, not raw
Evidence fields or executable inputs. A claimed `validated` capability is not
checked correspondence, and selecting it is not accepting any result. Evidence
ingestion/validation, execution/cancellation, resource enforcement and runtime
reconciliation remain later work. See the
[local API boundary](https://github.com/mumei-lang/mumei-agent/blob/develop/docs/ROADMAP.md#c04c11-explicit-result-contract-routing).

The next metadata-only C04/C11 slice adds per-adapter host-reviewed non-secret
setting schemas: strict Boolean, bounded integer and closed identifier choice.
Defaults precede explicit per-adapter overrides. Unknown/malformed settings fail
before selection, including disabled candidates; fallback uses its own settings
without borrowing from the original adapter or weakening the result contract.
Only the validated selected projection is exposed/fingerprinted; raw overrides
are not serialized or hashed. The canonical configuration hash binds schema,
effective values/budgets, selected versions/location and the full capability
query. Schema-less legacy selected JSON stays unchanged. The host must review
declared defaults/labels as non-secret: this API is not a secret detector.
The hash is not a Core Evidence configuration identity, source/tool identity,
execution/authenticity receipt, permission grant or acceptance decision.
File/environment lookup, secret transport, actual execution/resource enforcement
and runtime reconciliation remain open; audit/Lean/native/LLVM/WASM policies are
unchanged. See the [typed settings contract](https://github.com/mumei-lang/mumei-agent/blob/develop/docs/ROADMAP.md#c04c11-typed-configuration-follow-up).

Agent's next C04/C11 preparation slice requires an explicit input-format offer
and exact result contract, replans against current host registry declarations,
and binds a bounded immutable UTF-8 source snapshot to an externally supplied
raw-byte SHA-256. The explicit manifest contains only format/digest/byte count,
selected plan metadata and a deterministic request fingerprint. It does not
emit source text or read files, imports, artifact paths or environment data.
This is byte-level input preparation, not Core parsing/semantic extraction,
normalized specification identity, actual tool/observation freshness,
authenticity, execution permission or Evidence acceptance. Old prepared records
are historical metadata, not tokens authorizing later execution. Future
executors still need independent authorization, current preflight, resource
enforcement and result reconciliation. Default routes and existing assurance
policies remain unchanged. See the [source preparation boundary](https://github.com/mumei-lang/mumei-agent/blob/develop/docs/ROADMAP.md#c04c11-bounded-source-input-preparation).

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

### First LLVM translation-validation slice

`mumei_emit_llvm::translation_validation::validate_pure_i64` is an opt-in,
in-process C09 pilot, not a new CLI route or common-evidence producer. It consumes
one C03 `mumei.pure-i64-comparison/v1` source unit and one caller-supplied LLVM
17 textual IR snapshot. The caller must explicitly pin the exact
`x86_64-unknown-linux-gnu` triple and the profile's `DATA_LAYOUT` on that snapshot;
the checker never mutates it, invokes optimization passes or executes code.

The checked relation is equality of the source **body** and LLVM return value
for **every concrete signed-i64 input**, independently of requires/ensures.
Because arithmetic is excluded, signed bitvector comparisons encode the C03
integer comparison meaning without overflow. The LLVM side is separately
interpreted from the verified LLVM-C graph, not regenerated from the source HIR.
Its whitelist is scalar i1/i64 SSA: integer constants, comparisons (including
unsigned predicates so wrong signedness can be refuted), bitwise Boolean
operations, i1/i64 zext/trunc, select, phi, acyclic branches and returns. Parameter
mapping is positional; phi values follow actual predecessor edges.
The source model includes Boolean implication, but the existing emitter still
rejects implication in an executable body. Checking manually supplied IR for
that source form does not add or claim an emitter lowering.

Only one ordinary C-call-convention function with an i64 signature is accepted.
Memory/pointers, arithmetic, division/remainder/shifts, calls/FFI/intrinsics,
loops, vectors/floats, globals/aliases/ifuncs, inline assembly, attributes,
metadata, undef/poison and freeze are unsupported. Every block is screened,
including dead blocks. Unsupported syntax, invalid IR, unmatched identities
and exhausted analysis budgets are errors, never successful partial checks.
The initial bounds are 64 KiB per input, 256 source tokens, 16 parameters,
48 syntax/expression levels, 64 LLVM blocks, 1,024 static instructions and 256
expanded path states. Input limits and parser normalization are not independent
proofs of the parser's correctness.

The existing Z3 library checks a quantifier-free bitvector mismatch query.
Unsat yields `EquivalentWithinProfile`; sat yields `Different`; unknown/resource
exhaustion yields `Inconclusive`, never equivalence. Default limits are 5,000 ms
and 1,000,000 solver resource units; zero budgets are rejected. Reports contain
profile, input domain, target/layout, budgets and exact source/IR hashes, but no
source excerpts, solver models or raw parser diagnostics. They are ephemeral,
unauthenticated results, not serialized certificates or a cache acceptance API.

The integration checks the actual CLI's emitted **pre-optimization** IR after
the caller explicitly pins its target/layout; it does not attest the snapshot's
pipeline history. Rechecking a changed IR requires invoking the checker on that
exact input. No result propagates across later optimization, target lowering,
object emission, linker, runtime or executable changes. The trusted base still
includes the existing checked parser/C03 profile, LLVM 17 parser/verifier, this
small encoding and Z3. Parser/HIR normalization itself is not independently
validated. A false source postcondition may coexist with equivalent bodies.

This is **partial C09**, not full LLVM semantics, source contract correctness,
native-bundle acceptance or final-binary verification. The C03 extractor still
reports unchecked correspondence on its own, C08 `build/check` still verifies
identity consistency only, and audit/Claim Book/Seal Grade/Lean gates do not
change. No external verifier, plugin or universal semantic IR is added.

Regression gate:
`LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 cargo test --test test_translation_validation --test test_semantic_contract`.

### First native identity bundle slice

`mumei native-bundle build` is an opt-in producer for
`mumei.native-pure-i64-main-linux-x86_64/v1`. It accepts exactly one `main()`
with explicit `i64` return and a literal `true` precondition, within the C03
pure comparison/conditional profile. Imports, calls, arithmetic, FFI, effects,
resources, trusted/unverified atoms and additional declarations are rejected.
The first producer supports Linux/x86_64, LLVM 17 and recognized cc/gcc/clang
drivers with GNU ld or LLD version observations; it is not a portable build
format for every existing Mumei program.

```sh
# native-main.mm:
# atom main() -> i64 requires: true; ensures: result == 7; body: { 7 };
mumei native-bundle build native-main.mm -o example-native-bundle
# Preserve the printed Build ID through a separately trusted channel.
mumei native-bundle check example-native-bundle native-main.mm \
  --expected-build-id <trusted-64-character-lowercase-sha256>
```

The output must not exist. A private staging directory is published atomically
with no replacement; failures leave no new bundle and do not overwrite an old
bundle. The fixed profile deliberately does not load project configuration,
imports or proof caches. It runs fresh standard Z3 verification, generates a
certificate without ambient harness metadata, lowers through the existing HIR
and native-main LLVM path, persists target-bound LLVM input, emits the object,
and links that object. It does not pair arbitrary caller-supplied files after
the fact. The source is rechecked for changes before publication.

`native-bundle.json` (`mumei.native-bundle/v1`) binds the full source hash,
specification/implementation/atom hashes, compiler-local HIR/MIR fingerprints,
Translator IR/version/bridge identities, an explicitly empty source-dependency
digest, the certificate, LLVM input, object and executable byte identities.
Build records include the actual LLVM target/data layout/version, observed C
driver/target/linker versions, a digest of the actual driver arguments, fixed
optimization/CPU/feature/relocation/code-model settings and the C entry/runtime
profile. Raw banners, argument paths, environments and source comments are not
copied into the record. LLVM input and object are retained, not regenerated by
the consumer. The existing std `ProofBundle` format remains unchanged.

The consumer requires a separately supplied digest of the exact record bytes;
**do not derive that expected ID from the untrusted bundle being checked**.
It rejects unknown schemas/settings/fields, source or compiler fingerprint
drift, missing/replaced members, symlinks/non-regular members, oversized files,
and certificates failing current hash/status/Translator IR gates. Lean,
trusted, unknown and stale certificate claims cannot substitute for this
profile's source certificate. It reads only fixed component names plus the
explicit source path; it does not execute the artifact, invoke a linker, replay
Z3 or create a new grade. Filesystem mode/ownership, dynamic libraries, system
CRT, sysroots, environment, tool execution and semantic translation are **not
attested**. Component reads are bounded snapshots, not filesystem locks.

The result is explicitly `identity_consistency_only`, never
`artifact_verified`. A party able to authorize a new build ID can rebind a
different executable to the same source certificate: the regression suite
demonstrates this with an executable returning a different result. Hashes and
a trusted reference do not prove the producer ran the recorded tools, source
proof truth, ELF/ABI validity or LLVM/linker correctness. Repeated builds need
not have the same ID (fresh certificate and invocation metadata). Signing,
attestation, hermetic dependency closure, wider runtime/ABI/program/target
coverage, common C02 evidence integration, C09 translation validation and the
full C10 consumer policy remain open. This is partial C08, not end-to-end
Proof-Carrying Executable assurance. General build/run behavior is unchanged.

### First clean source-recheck consumer slice

`mumei native-bundle recheck-source` is the opt-in **C10 Stage A** consumer.
The existing `native-bundle check` remains identity-only; this new command adds
a fresh source-postcondition query, not a stronger claim about the executable.

```sh
mumei native-bundle recheck-source example-native-bundle native-main.mm \
  --expected-build-id <trusted-64-character-lowercase-sha256>
```

The command requires the same separately trusted build ID and exact C08 source,
record, certificate, LLVM IR, object and executable snapshots. Current
identity/status/Translator IR gates still reject stale, replaced, missing,
symlinked, non-regular and unsupported inputs. The C03 source is additionally
bounded to 64 KiB, 256 tokens and 48 syntax/expression levels. Its existing
profile restrictions apply; C08 still requires one zero-argument `main()`, an
explicit i64 result and a literal `true` precondition, so a false precondition
cannot manufacture a vacuous successful recheck.

Core's C03 symbolic body and contract encoding checks `requires && !ensures`
using the existing Z3 library with fixed settings: the native profile's solver
10,000 ms timeout and 1,000,000 resource units. Unsat gives `RecheckedWithinProfile` (exit
0); sat gives `Refuted` (exit 1); unknown/resource exhaustion gives
`Inconclusive` (exit 3), never success. Malformed/unsupported inputs and failed
identity gates exit 1. Diagnostics are static; no source excerpts, solver models,
environment values or raw filesystem errors are printed.

The ephemeral result identifies `mumei.native-source-recheck/v1` and
`source_postcondition_and_identity_only/no_translation_authenticity_or_binary_semantic_proof`.
It reads copied, read-only snapshots without loading project configuration,
proof caches or orchestration settings, running a compiler/linker/Z3 process,
executing the artifact or writing a new certificate/evidence/cache entry. The
consumer binary and its existing Z3 library remain required trusted components;
this is not a hermetic deployment or an independently certified source parser.

A self-consistent forged source certificate may pass identity-only `check` but
cannot replace the fresh source query. Conversely, an authorized replacement
executable can still pass if its new byte identity is pinned and the source
postcondition holds: executable semantics are explicitly **not checked**.
Nothing becomes `artifact_verified`, `proved`, a common-evidence acceptance or
a new Seal Grade. Audit/Claim Book/Lean acceptance and general build/run stay
unchanged. C09 source-to-LLVM snapshot correspondence belongs to a separate
Stage B; optimizer/object/linker/runtime/ABI/final-binary assurance, authenticity,
dependency closure and full C10 policy remain open. No external verifier is
adopted.

Regression gate:
`LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 cargo test --test test_native_bundle`.

### First native LLVM snapshot consumer slice

`mumei native-bundle recheck-llvm` is the opt-in **C10 Stage B** slice. It keeps
`check` identity-only and `recheck-source` source-and-identity-only, and requires
both Stage A's fresh source-postcondition result and a new C09-derived native
LLVM snapshot check. It never substitutes a translation result for a source
postcondition proof.

```sh
mumei native-bundle recheck-llvm example-native-bundle native-main.mm \
  --expected-build-id <trusted-64-character-lowercase-sha256>
```

It reads the same pinned files once into bounded in-memory snapshots, passes
all Stage A gates, then parses and verifies the **actual `program.ll` bytes**
using the existing LLVM 17 library. It does not rewrite the IR, rename source
text or regenerate an alternative module. The native profile is
`mumei.llvm17-native-main-snapshot/v1`, distinct from C09's single-function
scalar profile; the latter is not broadened.

The new profile admits exactly two externally linked definitions:
`i64 @__mumei_user_main()` and `i32 @main(i32, ptr)`. Core's C03 model supplies
the zero-argument source i64 body; the existing bounded SSA interpreter and Z3
compare its full 64-bit result with the user-main LLVM definition. The wrapper
must be one block containing only a direct zero-argument call to that exact
definition, a truncation of its result to i32 and a return of that truncation.
Function/call attributes, operand bundles (including empty ones), alternate calling conventions, extra instructions,
functions/declarations, globals, aliases, inline assembly and metadata are not
admitted. The existing C09 body restrictions and bounds still apply: no memory,
calls, loops, poison/undef or unsupported operations; 64 KiB IR, 48 delimiter
levels, 64 body blocks, 1,024 body instructions and 256 expanded path states.

Only the exact triples `x86_64-pc-linux-gnu` and `x86_64-unknown-linux-gnu` and
C09's fixed x86_64 GNU data layout are admitted. The module triple/layout must
also equal the pinned record's object-build observations; no normalization or
implicit target substitution occurs. Other C08-buildable target spellings may
remain unsupported by this narrower consumer.

After Stage A's fixed 10,000 ms / 1,000,000-resource-unit source query, the
translation query uses fixed 5,000 ms / 1,000,000-resource-unit settings. Both
queries must be unsat for exit 0. A false postcondition or different LLVM body
exits 1, as do malformed, unsupported or inconsistent snapshots. Unknown or
resource exhaustion at either query exits 3 with no combined success claim.
Diagnostics are static and do not disclose source/IR excerpts or models. No
ambient settings, project configuration/cache, producer tools, external verifier
or artifact execution is used; no new certificate/evidence acceptance is written.

The ephemeral success scope is
`source_postcondition_identity_and_llvm_snapshot_only/no_authenticity_or_binary_semantic_proof`.
Checking the wrapper shape makes **low-32-bit truncation explicit**, not an
i64-to-OS-exit-code equivalence or an ABI/runtime guarantee. A user body differing
only above bit 31 is still rejected even if the wrapper's low 32 bits coincide.
Conversely, repinned garbage object/executable bytes may still pass when source
and LLVM checks hold: there is **no LLVM-to-object, optimization, backend, linker,
system-library, OS, runtime or final-executable semantic proof**, nor producer
authenticity or attestation that the object was emitted from these IR bytes.
The consumer binary, Core source model, Z3 and LLVM parser/verifier/interpreter
remain trusted components. Packaging is not a hermetic deployment.

This is partial C09/C10 integration, not full C10 or Proof-Carrying Executable
assurance. Common C02 evidence integration, broader profiles/targets, dependency
closure, authenticity and final-artifact assurance stay open. Nothing becomes
`artifact_verified`, unrestricted `proved` or a new Seal Grade; Claim Book, Lean
gates, general build/run and Mumei/Agent ownership remain unchanged.

Regression gates:
`LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 cargo test --test test_native_bundle --test test_translation_validation --test test_semantic_contract`.

### First native consumer Evidence producer

`mumei native-bundle recheck-llvm <bundle> <source> --expected-build-id <trusted-id> --json`
retains every Stage A/B gate and exit code. Without `--json`, the existing text
output is unchanged. JSON is emitted **only after both fresh queries succeed**;
identity failures, refutation, unsupported input and inconclusive outcomes emit
no report or partial Evidence on stdout. Diagnostics remain static on stderr.
The command does not write files or execute an artifact.

The `mumei.native-consumer-evidence/v1` envelope contains the trusted build-ID
pin, the existing snapshot-only scope, `acceptance: not_evaluated`, and exactly
three records conforming to `mumei.verification-evidence/v1`:

| Record | Method / scope | Meaning |
| --- | --- | --- |
| Native bundle identity | `identity_check` / `identity` | The pinned source, certificate and bundle-member identities are consistent. Semantic correspondence remains unchecked. |
| Source postcondition | `deductive_proof` / `unbounded` | Fresh C03 source query succeeded within the zero-argument-main profile. The source encoding/normalization correspondence remains an explicit trusted assumption. |
| Source-to-LLVM user body | `deductive_proof` / `unbounded` | Fresh full-i64 equivalence query succeeded within the native LLVM profile; only the truncating wrapper shape is checked. |

Here `unbounded` means a deductive query without finite replay or loop unrolling,
**not unrestricted source, all LLVM, system correctness or an unbounded integer
model**. The numeric model is the checked i64 source profile, whose arithmetic
and overflow operations remain unsupported. Fixed source/LLVM solver budgets
and observed target/layout are included in the approved configuration fingerprint;
profile names retain all syntax/size/path limits.

Subjects/specifications are tied to the exact snapshots already held in memory,
not reopened files: identity uses the pinned manifest as specification, source
postcondition uses the C03 contract identity, and LLVM correspondence uses the
full source digest that binds the particular body, not only its postcondition.
IDs and artifact paths are fixed logical labels/relative
bundle-member names, never caller paths. Provenance reports the running Mumei
version and linked Z3 version, not producer-supplied tool/version strings.
No source/LLVM text, models, environment, endpoint, command arguments, raw
configuration or credential-derived fingerprint material is included.

This is **partial C02/C10 producer integration**, not Evidence acceptance.
A saved or schema-valid report can be forged or become stale: schema validation
alone establishes neither truth, identity/freshness, provenance trust nor
authenticity. No external report ingestion, signature, cache reuse, evidence
acceptance, Claim Book/Seal Grade or Lean gate is added. Object/optimizer/linker,
ABI/runtime/OS exit-status/final-executable semantics and dependency/toolchain
closure remain unverified. Broader C02/C09/C10 and final-artifact assurance stay
open; external verifiers are not adopted.

### First saved native Evidence replay consumer

`mumei native-bundle recheck-evidence <bundle> <source> <evidence> --expected-build-id <trusted-id>`
compares a saved native consumer report with **fresh Stage B checks**, not with
the saved success claims alone. For example:

```sh
mumei native-bundle recheck-llvm bundle source.mm --expected-build-id <trusted-id> --json > evidence.json
mumei native-bundle recheck-evidence bundle source.mm evidence.json --expected-build-id <trusted-id>
```

The expected build ID must still come from a separately trusted channel, never
from the report. The report is an additional input, not a bundle member or a
replacement for source, IR, certificates or pinned identities. It is read once
as a regular, non-symlink file of at most 65,536 bytes. A closed typed decoder
rejects malformed/non-UTF-8 JSON, duplicate/unknown fields at every object level,
missing required fields and record counts other than three. Report strings and
artifact paths never select tools or files to execute/open.

After decoding, the consumer performs every C08/Stage A/B identity, source-query,
LLVM-body/wrapper and exact target/layout gate. Only a successful fresh result
is compared with the saved report. All decoded data must match: schema/build
identity/scope/acceptance, each subject/specification/obligation, semantics,
method/execution/result, correspondence, assumptions/exclusions, artifact labels
and local adapter/tool/configuration provenance. JSON object order and whitespace
do not matter; nullable targets must be explicitly present, including source
targets set to null. Records and all lists retain producer order. Unsupported profiles or different version/
configuration labels are rejected rather than silently migrated, even when a
different consumer might also verify the program. This is a deliberately narrow
same-profile/provenance replay, not a general common-Evidence schema validator
or cross-version/backend interchange policy.

Exit 0 means the report agrees with the fresh restricted checks. Missing,
stale/tampered, malformed, unsupported, refuted or mismatched inputs exit 1;
fresh solver unknown/resource exhaustion exits 3 and never establishes report
consistency. Failures have empty stdout and static diagnostics. The command
writes no files/certificates/new Evidence, loads no project config/cache, starts
no producer tools or external verifiers, and never runs the artifact. Existing
`check`, `recheck-source` and `recheck-llvm` output/exit behavior is unchanged.

**A match is not authenticity, historical proof execution, or proof acceptance.**
A report author can fabricate data that matches a successful fresh query; the
assurance comes from that current query and its trusted components, not the
report author. Saved `established` claims, rebound hashes or schema validity
cannot replace a fresh query. Repinned garbage object/executable bytes may still
match an updated report: only identity is checked for those bytes. The full-i64
LLVM body and wrapper shape remain distinct from OS exit status, object/backend/
linker/runtime/ABI/final-executable semantics. `acceptance: not_evaluated`, Seal
Grades, Claim Book and Lean trust/freshness gates remain unchanged. General C02
acceptance/mapping, broader C09/C10, authenticity and final-artifact assurance
stay open; no external verifier is adopted.

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
