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
mumei evidence check --stdin --json < record.json
```

This opt-in command uses the shared Core gate on **one common v1 record**, not
the three-record native report envelope, a proof certificate, a JSON array or
JSON Lines. It is an integration entry point for representation checking without
a Native bundle or a Rust caller, not a verifier/adapter execution path or a
general acceptance engine. It does not load project configuration/proof caches,
read record-selected artifacts, check hashes against files, authenticate a
producer, select a verifier or execute anything mentioned in the record.

Select exactly one input: a regular file or explicit `--stdin`, each with at most
65,536 bytes. Missing/conflicting input selection is a usage error (exit 2), not
an implicit stdin fallback; a positional `-` remains a literal file name.
Symlink/nonregular file inputs, malformed/non-UTF-8/trailing/deep JSON, duplicate/escaped keys, unknown
fields, missing nullable targets and invalid v1 combinations fail closed. The
bounded file reader is shared with Native consumer inputs without changing their
limits or diagnostics. On Unix it also uses `O_NOFOLLOW | O_NONBLOCK` for the
final component and checks the opened file type; this is not a claim of general
race-free/path-confinement handling on every platform or parent directory.

`--stdin` sends original bytes directly to the same Core decoder without a
temporary input file or JSON reserialization. It reads at most 65,537 bytes
(one extra byte detects overflow), then rejects oversized input without draining
the stream. This bounds bytes read, not time: a producer must close input and the
invoking host owns deadlines/cancellation. EOF does not prove producer completeness
or authenticity. Read failures cannot emit a success diagnostic; underlying
errors and private input are not quoted.
This is a raw UTF-8 byte interface for pipes/redirected files, not a terminal-text
encoding adapter. In particular, Windows console code-page conversion is not
performed; supply UTF-8 bytes through a pipe/redirected file or use file mode.
This adds no Agent process runner, automatic Core call, adapter normalization,
request/result binding, authority enforcement or Evidence acceptance.

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

### Bounded multi-record representation check

```sh
mumei evidence check-list evidence-list.json
mumei evidence check-list evidence-list.json --json
mumei evidence check-list --stdin --json < evidence-list.json
```

`evidence check` admits exactly one common v1 record, yet native report
envelopes already carry `1 + 2·N` records under family-specific schemas, so a
producer needing several records in one bounded input has no common gate.
`check-list` fills that gap with a **closed list envelope**
`mumei.verification-evidence-list/v1`: an object with only `schema_version` and
`evidence`, whose elements are common v1 records — not a three-record native
report envelope, a proof certificate or an arbitrary JSON array of values.

Every element keeps its raw input bytes and passes the same single-record gate
as `evidence check`, so duplicate-key, unknown-field and combination checks
apply inside each record exactly as they do standalone (a parsed `Value`
element would already have collapsed duplicate keys before the inner gate could
see them). Two bounds apply: the envelope is at most 65,536 bytes — the same
shared input limit — and carries at most 64 records, whichever trips first.
An empty `evidence` array is representation-valid: the gate states each
admitted record's shape, not that any record exists. The diagnostic adds
`record_count` and lists `cross_record_consistency` among the checks not
performed — no deduplication, ordering, agreement or coverage claim is implied,
and no record's truth is evaluated.

Input selection, rejection posture and exit codes are identical to
`evidence check`: one regular file or explicit `--stdin`, exit 0 only for a
fully valid envelope, exit 1 with no stdout otherwise, clap exit 2 on usage
errors. The JSON diagnostic is `mumei.evidence-representation-check/v1` with
`evidence_schema_version: mumei.verification-evidence-list/v1`; it remains an
anonymous diagnostic, not reusable Evidence, a certificate or an acceptance
receipt, and is not accepted as an Evidence input.

### Cross-record identity-pin consistency

`mumei evidence check-consistency <list>` (or `--stdin`) accepts the same
`mumei.verification-evidence-list/v1` envelope and checks syntactic
agreement on the identity pins records declare:

- records naming the same `subject.id` *and* `subject.sha256` must pin the
  same `subject.kind` — one claimed subject cannot carry two kinds. A
  `subject.id` alone is a producer label, not an identity: standalone
  source records all use `"source.mm"`, so two records sharing the id but
  pinning different content describe different subjects and stay
  independent;
- records naming the same `obligation_id` must pin the same `subject.id`,
  `subject.sha256`, `specification.id`/`sha256`, `semantics` and `method`
  — one claimed obligation cannot bind two subjects or specifications.

Outcome fields (`result`, `execution`, `scope`, `correspondence`) are never
compared: they are each record's claim, and a fresh record may legitimately
disagree with an older one there. Records sharing no pin are independent,
not conflicting. The JSON diagnostic `mumei.evidence-consistency-check/v1`
reports `consistent`, a bounded `conflicts` list (`index_a`, `index_b`,
`field`; at most 64 entries) with the true total in `conflict_count`, and
`record_count`, and still states `acceptance: not_evaluated` — agreement is
structural only. A forged pair that agrees passes: pin correctness, truth,
freshness and authenticity stay in `checks_not_performed`.

### Declared producer-identity gate

`evidence check`, `evidence check-list` and `evidence check-consistency`
each accept `--expected-adapter <id>`: after the representation gate, every
record's declared `provenance.adapter.id` must equal the expected value,
obtained through a separately trusted channel like `--expected-build-id`.
A record declaring any other adapter fails with a static diagnostic before
the success output, and an empty expectation is a request error — the
closed schema never admits an empty adapter id.

When the gate runs, the JSON diagnostic adds `checks_performed` naming
`representation_gate` (plus `cross_record_pin_agreement` for
`check-consistency`) and `declared_producer_identity`; without the flag the
diagnostic is byte-identical to the ungated form. `producer_authenticity`
stays in `checks_not_performed`: a matching *declared* id proves only that
the record names the expected adapter — nothing establishes the adapter
actually ran, which needs signed or attested provenance. The deep
`accept-*` consumers need no such flag: their fresh same-profile
`fresh == saved` comparison already binds the whole provenance block,
adapter included, to the rechecking toolchain.

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

### Standalone source Evidence acceptance evaluation

```sh
mumei evidence accept-source source-evidence.json --source source.mm
mumei evidence accept-source source-evidence.json --source source.mm --json
```

`mumei_core::source_evidence::accept(source, saved_bytes, Options)` and this
opt-in CLI evaluate the **same bounded pipeline** as the consistency recheck —
the common representation gate, a fresh caller-budgeted source query, and
equality of every decoded field — but report the result in the acceptance
vocabulary: `Accepted` only when representation is valid **and** the fresh
same-profile query agrees, `Rejected` on refutation, `Inconclusive` on
unknown/resource exhaustion; malformed/unsupported/mismatched inputs remain
static errors. Acceptance here means representation validity plus fresh
same-profile consistency under the declared restricted profile — nothing
stronger.

On acceptance, `--json` emits one static anonymous descriptor
`mumei.source-evidence-acceptance/v1` with `acceptance: evaluated`,
`accepted: true`, the restricted scope, `checks_performed`
(`representation_gate`, `fresh_same_profile_source_query`) and
`checks_not_performed` naming what stays unassessed: producer authenticity,
historical execution, record freshness, parser/encoder/solver correctness,
*independent* model correspondence (the same producer re-derives the record —
no second verifier runs), LLVM correspondence, artifact semantics, grade
promotion and Lean kernel checks. Text mode prints the same scope in prose.
The descriptor echoes no subject, digest or provenance: it is a diagnostic,
not reusable Evidence, a certificate or a grade. Its only binding is
`evidence_fingerprint` — the SHA-256 hex of the accepted record's input
bytes — which lets a holder prove *which* record was evaluated without the
descriptor replaying any claim or digest as identity. Non-acceptance paths
emit no stdout, including JSON mode; refutation and rejection exit 1,
inconclusive exits 3.

This is the first Core-side `acceptance: evaluated` surface, bounded to the
one profile it names. It is not a general acceptance engine, mapping between
profiles, multi-record admission, producer/record authentication, historical
freshness, an independent-verifier correspondence, artifact/final-executable
assurance, or a Claim Book/Seal Grade/Lean decision. Saved fields never select
files, tools, queries or budgets; existing commands and native/module evidence
paths are unchanged.

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

The next Agent C04/C11 permission slice compares host-declared requirements
against an explicit closed set of allowed categories before selection and
fallback. Missing declarations do not satisfy an explicit policy, while omitted
policy preserves legacy planning without implying approval. Selected declarations
and allowed policy are retained in explicit plan/preparation metadata.
This is not capability truth, a user/admin grant, an OS sandbox or permission to
execute code/transmit source data. Categories do not derive Mumei effects or
actual privileges from adapter location/implementation; later executors must
independently establish and enforce authority. C12's real-authority demand gate,
Core Evidence acceptance and existing assurance policies remain unchanged. See
the [permission-policy boundary](https://github.com/mumei-lang/mumei-agent/blob/develop/docs/ROADMAP.md#c04c11-declared-permission-policy-filtering).

Agent's next C11/C13 lifecycle contract binds bounded, immutable host-reported
attempt histories to prepared-request fingerprints. Start, cancel request/ack,
exit, failure and timeout are distinct observations; terminal histories cannot
be rewritten and exit code zero is not verification success. The contract does
not execute tools, read clocks, enforce authority/resources or prove process
cleanup. It does not replace common Evidence or Core acceptance. Global attempt
uniqueness, cross-attempt/backend arbitration, output ingestion and actual
execution remain open; C12's demand gate and existing assurance policies are
unchanged. See the [Agent lifecycle boundary](https://github.com/mumei-lang/mumei-agent/blob/develop/docs/ROADMAP.md#c11-bounded-host-observed-attempt-lifecycle).

Agent's same-attempt reconciliation follow-up accepts only snapshots with the
same request fingerprint, attempt number and timeout that preserve the current
event history as an exact prefix. Identical reports are idempotent, not fresh;
shorter/stale, divergent and terminal-rewriting reports fail without modifying
either record. Code zero, later elapsed counters or arrival order do not resolve
conflicting branches. Callers must retain and atomically advance their current
record and surface disagreements: independent callers can still fork from an
old common prefix. This is consistency within one metadata lineage, not a global
ledger, authentication/replay protection, actual execution or common Evidence
normalization/acceptance. Cross-attempt/backend arbitration, authority/resource
enforcement and C11 execution remain open; C12's demand gate, Core ownership and
existing assurance/Lean/native/LLVM/deferred-WASM policies are unchanged. See the
[Agent reconciliation boundary](https://github.com/mumei-lang/mumei-agent/blob/develop/docs/ROADMAP.md#c11-same-attempt-report-reconciliation).

Agent's sequential-record follow-up adds an immutable `AttemptSequence` for one
request fingerprint and per-attempt timeout, capped at 16 records numbered from 1
without gaps. Reserving the next ready record requires a terminal predecessor;
it neither starts nor authorizes execution. Reports can only extend an already
reserved attempt's known prefix; exact old terminal duplicates are idempotent,
while unknown ordinals, mixed bindings, rewinds, branches and terminal rewrites
fail. The current record is not a verification winner and code zero has no success
meaning. Sequence dictionaries contain ordered metadata, not output or Evidence.

This enforces local logical ordering only: hosts retain/atomically advance the
canonical value, and independent branches can still fork. Global identity,
replay/authenticity protection, total retry/resource budgets, retry scheduling,
cross-backend/result arbitration and output lineage/selection remain outside it.
Terminal metadata does not establish process cleanup or prevent physical overlap.
Existing outputs, explicit Core calls, Evidence meaning/acceptance and assurance
policies remain unchanged; C11/C13 remain partial, C12 demand-gated, with no
default route or external verifier. See the [Agent sequence boundary](https://github.com/mumei-lang/mumei-agent/blob/develop/docs/ROADMAP.md#c11-bounded-sequential-attempt-records).

Agent's bounded stdout follow-up prepares 0–65,536 opaque host-supplied bytes
against an exited attempt, a separately specified attempt number, the prepared
request/timeout and caller-supplied SHA-256, requiring explicit permission
metadata but not granting authority. Other states and digest/binding mismatches
fail; no truncation occurs. Empty, binary and malformed JSON remain opaque bytes,
and zero/nonzero exit codes do not establish verification success. A private
in-memory body is available explicitly; repr/JSON contain only attempt and
size/digest/fingerprint metadata, not source, commands, stderr or stdout text.
Generic reflection and explicit body access must not be logged. Digests are not
secret detection, declassification, provenance or completeness proofs.

Output reconciliation accepts only identical attempt histories and raw stdout;
conflicts fail without choosing a winner. Hosts must first reconcile their
current attempt, retain the canonical output and atomically publish updates;
binding alone cannot detect independent forks. This is not parsing, stream
collection, freshness/authentication/replay protection, actual execution,
adapter result normalization or Core-owned common Evidence checks/acceptance.
C11 execution and
cross-attempt/backend arbitration remain open; C12's demand gate and existing
assurance/Lean/native/LLVM/deferred-WASM policies are unchanged. See the
[Agent output boundary](https://github.com/mumei-lang/mumei-agent/blob/develop/docs/ROADMAP.md#c11-bounded-opaque-stdout-snapshots).

Agent's explicit JSON stdout decode follow-up checks only bounded transport:
one UTF-8 object, unique decoded keys, scalar Unicode strings, at most 32 nested
containers and 128 characters per number literal with representable decimal
exponents. Extra output, malformed syntax and non-JSON constants fail without
echoing input or chaining decoder payloads. Exact integer/Decimal values avoid
binary-float rounding; each private mutable tree is detached from the immutable
raw-byte snapshot. Neither the tree nor explicit bytes may be logged. Decode is
not canonicalization: downstream Core checks must consume original bytes.
An empty object or saved success claim can decode without being Evidence. Core
representation/meaning/acceptance, adapter result normalization and actual
execution/authority remain separate; default routes, C12's demand gate and
existing assurance/Lean/native/LLVM/deferred-WASM policies are unchanged. See the
[Agent JSON boundary](https://github.com/mumei-lang/mumei-agent/blob/develop/docs/ROADMAP.md#c11-bounded-json-stdout-decoding).

Agent also offers a separate opt-in POSIX local Core representation handoff.
An independently authorized host-trusted absolute executable receives original
stdout bytes through fixed `evidence check --stdin --json`, without environment
forwarding, command discovery, temporary files or reserialization. Concurrent
bounded pipes and a separate caller timeout protect this limited interaction;
process creation is not interruptible and direct-child cleanup has a bounded
extra grace period. This is not a general adapter runner or OS sandbox.
An optional host-owned standard `threading.Event` cancels only this Core call:
pre-cancel prevents spawn; pipe and exit waits poll within 50 ms without extending
the deadline. Observed cancellation closes pipes and kills/reaps the direct child
before a fixed cancellation error; cleanup failures remain check failures.
Process creation is not interruptible, scheduling is not hard real-time, and
completion can race with the last check. Keep the event set until return, not as
a set/clear pulse. No event mutation or attempt cancellation report is produced;
canonical lineage and completion/cancel arbitration remain host responsibilities.

An optional `attempt_sequence` keyword with an exact `AttemptSequence` value can
gate this Agent call before Core spawn/I/O. `check_output_binding()` requires an
exact `AttemptOutput` with the
same request fingerprint, ordinal, timeout and full event history as that value's
current record; empty/unreconciled sequences, old/future attempts and divergent
bindings/histories fail with fixed body-free errors. Omission/`None` preserves
existing sequence-independent calls; original bytes, diagnostics and schemas do
not change. This is a supplied-snapshot precondition, not global freshness:
stale sequence/output pairs can match, and competing bodies for the same current
record each pass binding. Hosts still reconcile canonical outputs and own atomic
storage, concurrent publication/completion and result selection. No lock/CAS,
completion-time revalidation, authentication/replay protection, process cleanup
proof, Evidence/request-contract matching, receipt or acceptance is implied.

Zero exit alone is insufficient: the complete known v1 diagnostic, no stderr and
no output overflow are required. Errors are static and no new receipt is returned.
Representation validity still relies on the host-trusted binary, not authenticity
of an anonymous saved diagnostic. Core acceptance, claim truth, request/result
matching, freshness, correspondence, adapter normalization and generic execution
remain separate; no default audit/CLI/MCP or assurance policy changes. See the
[Agent handoff boundary](https://github.com/mumei-lang/mumei-agent/blob/develop/docs/ROADMAP.md#c11-explicit-local-core-representation-handoff).

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
one `mumei.pure-i64-wrapping-scalar/v1` source unit — the C03 comparison fragment
extended with wrapping add/sub/mul, total bitwise `&`/`|`/`^`, guarded
signed division/remainder/shifts, block-scoped `let` bindings, scalar
reassignments on the linear path and inside `if` branches, discarded
pure expression
statements, `bool` parameters and results, subrange integer parameter
and result types (`i8`/`i16`/`i32`/`u8`/`u16`/`u32`), `match` on an i64
scrutinee with literal patterns, Boolean arm guards and a trailing
unguarded catch-all arm, and calls to sibling atoms —
and one caller-supplied LLVM
17 textual IR snapshot. The caller must explicitly pin the exact
`x86_64-unknown-linux-gnu` triple and the profile's `DATA_LAYOUT` on that snapshot;
the checker never mutates it, invokes optimization passes or executes code.

The checked relation is equality of the source **body** and LLVM return value
for **every concrete signed-i64 input** the declared signature admits —
a `bool` parameter bounds its i64 input to 0/1 —, independently of
requires/ensures.
Signed bitvector comparisons and wrapping add/sub/mul encode the C03
integer meaning over mod-2^64 wrap-around — the emitter never sets
nsw/nuw, and flagged IR arithmetic is rejected, so overflow still
carries no check and no claim. Signed `/`, `%`, `<<` and `>>` map to
truncating `sdiv`/`srem`/`shl`/`ashr` — the same semantics `verify
--bitvec-i64` gives the source operators — and bind the input only
where the operation is defined: a nonzero divisor other than
`i64::MIN / -1`, and a shift amount in `0..64`. The checked relation
is refinement over that partial domain: inputs the source leaves
unbound are don't-care, matching how the emitter's select-guarded
`srem` stays defined on `x % -1`. A source whose division or shift
binds no input at all (e.g. `x / 0`) is an error, mirroring the spec
verifier's rejection. Source `&`/`|`/`^` map to `and`/`or`/`xor` — bitwise
operations are defined on every i64 input, so they carry no domain
obligation and are checked for all inputs unconditionally. Source bodies
may sequence `let` bindings, scalar reassignments and pure expression
statements before the
final value statement: each `let` initializer, each rebinding and each
leading
expression is kind-checked and encoded in order under the enclosing
guard — a partial operation (division, shifts) records its definedness
obligation under that same guard whether or not its value is discarded —
and bindings are lexical, so a shadowing or branch-local `let` never
escapes its block. A discarded expression contributes no value but its
obligations still hold. A nested block in a leading position is checked
and encoded like a discarded expression — its value is unused and its
bindings stay lexical to the nested block. An `if` branch may rebind
names bound on the enclosing linear path: each written name merges
under the branch condition like the emitter's per-name phi, and a name
written on one side carries its prior value through the other, so
`if x > 0 { x = 1; x } else { x }; x` is checked and a partial
operation in a branch keeps its definedness obligation under the
refined guard. `match` arms in statement position rebind the same
way: each write merges under the arm-local hit — the accumulated
negations of every earlier arm's `pattern ∧ guard` plus this arm's
own — matching the emitter's match.merge per-name phi; a dead arm's
merge condition is unsatisfiable so its writes never escape, and a
`Variable` pattern binding shadows an outer name for its arm. The
merge reaches no other position: writes inside operands, `let`
initializers, expression blocks and nested blocks may only touch
names bound within them — `x + (if x > 0 { x = 1; x } else { x })`,
`let a = x; { a = 1; a }; a` and
`let v = match x { 0 => { r = 1; r }, _ => { r = 2; r } }; v`
stay rejected. Calls are admitted in expression and statement
position with i64 arguments and an i64 result. A `while` is admitted
mid-block only — inside a `Stmt::Block`'s non-final statements —
under a Boolean guard and with no nested loop anywhere inside its
body (nesting would break the shared per-iteration bound). Its
invariant and decreases expressions are the Z3 verifier's metadata;
the bounded profile ignores them. The body is unrolled `LOOP_BOUND`
(4) guard evaluations: each iteration evaluates the guard under
`entry-guard ∧ ¬exited` (its definedness obligations under the same
condition), executes the body under `alive ∧ guard`, merges each
escapable write as `ite(guard, body-value, current)` per name, and
records `exited |= ¬guard`. The conjunction of every loop's exit
predicate joins the compared domain, so the claim covers exactly the
inputs where every loop's guard fails within the bound — deeper
executions are out of profile, and a source whose exits leave no
covered input is an error rather than a vacuous equivalence. Other
statement forms (unbound assignment, value-position `while`) stay
unsupported.
A declared `bool` parameter lowers to i64 carrying 0/1 — the same
lowering `resolve_param_type` and the emitter's `icmp ne` condition use —
so its source value is the truth of `input != 0` and the signature
itself bounds the checked domain (`input <=u 1`); without that bound a
declared `b == true` would spuriously differ from `icmp eq i64 %b, 1`
at `input = 5`, an input the signature never admits. A declared
`-> bool` result lowers the same way — the emitter zexts its i1 result
to i64 — so the checked source value is the Boolean body's 0/1
materialization and an `i1` return is a signature error, not a value
difference. Boolean
expressions stay two-valued: a `bool` binding is not an i64 operand
(`b + 1` is rejected), a `-> bool` body must produce a Bool kind
(`if b { 1 } else { 0 }` is rejected),
and reassigning a bool to a non-bool value keeps the binding's kind.
Declared subrange integer types — `i8`/`i16`/`i32`/`u8`/`u16`/`u32` —
lower to an i64 formal the same way `resolve_param_type` lowers every
non-aggregate type, so their source value is the input itself and the
signature bounds the checked domain to the type's range (unsigned types
by `input <=u max`, signed types by `min <=s input <=s max`): `x == 200`
on an `i8` parameter is equivalent to `false` only because the bound
rules `x = 200` out. `u64`, `usize` and `isize` stay outside the
profile — a full-width unsigned domain cannot be expressed as an i64
range bound — and `u64`-sized results stay outside the profile on the
same signature boundary. A declared subrange integer result type
(`-> i8`/`i16`/`i32`/`u8`/`u16`/`u32`) lowers to an i64 return the same
way `resolve_return_type` lowers every non-aggregate type: the emitter
emits no truncation, so the checked result is the body's full i64
value and the binder discloses the declared type.
A `match` on an i64 scrutinee is checked in first-match order: each
literal pattern contributes one `target == n` test, an arm may add a
Boolean guard evaluated in the arm's scope, and an arm hits only when
`pattern ∧ guard` holds — the arm bodies are encoded as an ite chain
under the accumulated negations of every earlier hit, so a guarded or
shadowed arm never leaks its value. Linear-path names rebound inside
an arm body merge the same way — under the arm-local hit — so
`match x { 0 => { r = 1; r }, _ => { r = 2; r } }; r` is checked
against the emitted match.merge phi. Literal-only patterns cannot cover
i64, so the contract requires a trailing unguarded `Wildcard` or
`Variable` catch-all arm rather than a solver exhaustiveness check (a
guarded catch-all could still fall through); a `Variable` arm binds the
scrutinee value for its guard and body. Non-literal patterns, a non-i64
scrutinee, a non-Boolean guard and mixed-kind arm results are static
errors. On the IR side,
`unreachable` executes LLVM undefined behavior: the encoder records an
always-unsatisfiable definedness obligation on that path, so an
`unreachable` reachable on a defined input reports `UndefinedBehavior`
while a dead `unreachable` block stays harmless. The existing emitter
lowers `match` to a try-chain whose final else edge is a constant-true
branch into `match.merge` — it never emits `unreachable` — and the
checker compares that shape against the same first-match-order model.
The LLVM side is
separately
interpreted from the verified LLVM-C graph, not regenerated from the source HIR.
Its whitelist is scalar i1/i64 SSA: integer constants, comparisons (including
unsigned predicates so wrong signedness can be refuted), unflagged
and/or/xor on i1 and i64, unflagged add/sub/mul/sdiv/srem/shl/ashr, i1/i64
zext/trunc, select, phi, acyclic branches and returns and
`unreachable`. Parameter
mapping is positional; phi values follow actual predecessor edges.
The source model includes Boolean implication, but the existing emitter still
rejects implication in an executable body. Checking manually supplied IR for
that source form does not add or claim an emitter lowering.

Every function in the module is an ordinary C-call-convention function
with an i64 signature, external linkage and a defined body; declarations,
internal linkage, globals/aliases/ifuncs, inline assembly, attributes and
metadata are unsupported. A `call` is admitted only as a direct, plain
call — no operand bundles, tail calls, call-site attributes or
non-C calling conventions — to a defined in-module function whose
return and parameters are all i64. Both encoders then model the call as
the same uninterpreted function of its i64 arguments keyed by callee
name and arity: the comparison is a call-site comparison, so it holds
exactly when both sides call the same callee on equal arguments.
Whether a callee's own body implements its source contract is a
separate per-function validation obligation, not discharged here.
Memory/pointers, unsigned `udiv`/`urem`/`lshr`, flagged (nsw/nuw/exact)
arithmetic — a flag widens the poison domain and is rejected —,
FFI/intrinsics/indirect calls,
vectors/floats, undef/poison and freeze are unsupported. Cyclic IR is
admitted only when the source model carries loops: a path that revisits
a block beyond `LOOP_BOUND` visits is dropped as out-of-profile, which
bounds the loop header to the same 4 guard evaluations the source
unroll performs; a cycle against a loop-free source stays rejected.
The emitter's per-header `__mumei_task_group_should_cancel_current`
poll is admitted as a declaration-only runtime intrinsic whose result
binds to 0 — the claim covers executions where no cooperative
cancellation is requested. A module member declaring that intrinsic is
screened by name, not compared as an obligation. Every block is screened,
including dead blocks. Unsupported syntax, invalid IR, unmatched identities
and exhausted analysis budgets are errors, never successful partial checks.
The initial bounds are 64 KiB per input, 256 source tokens, 16 parameters,
48 syntax/expression levels, 64 LLVM blocks, 1,024 static instructions and 256
expanded path states. Input limits and parser normalization are not independent
proofs of the parser's correctness.

`validate_pure_i64_module` discharges the per-function obligations a
call-site comparison leaves open: it runs the same check once per source
atom against the same-named module member. Each atom is compared
independently — a callee's body is never inlined into the caller's check —
and the report carries one outcome per atom in declaration order plus an
aggregate (`Different` outranks `UndefinedBehavior`, which outranks
`Inconclusive`; the module is `EquivalentWithinProfile` only when every
entry is). Every source atom must name a module member or the whole check
refuses — a missing callee cannot even form its call-site comparison —
extra members are screened by the module profile but not compared, and
the solver budget applies per function.

`validate_native_module` applies the same per-atom check to native
snapshots — the emitted module that keeps its C `main` wrapper. The
`main` atom is compared against `__mumei_user_main` (the emitter's
rename of the user `main`) and must be zero-argument, matching the
wrapper's call shape; every other atom is compared against its
same-named member. The C `main` wrapper itself is not compared — it is
screened by the fixed call/trunc/ret shape, as in `validate_native_main`
— and every member keeps external linkage under the shared function
profile. A source atom literally named `__mumei_user_main` is rejected
(the name is the emitter-reserved rename target). The module must
contain exactly one `main` atom. The report profile is
`mumei.llvm17-native-module-snapshot/v1`, and its input-domain labels
mark per-member signed-i64 inputs as not attesting the process exit
status. Like the rest of this slice it attests only the supplied
snapshot — no object, linking, OS exit-status or final-executable
semantics.

`mumei build --emit binary` runs `validate_native_module` on the staged
merged IR after the host triple and data layout are bound — the exact
bytes the object emitter consumes — and writes an advisory
`<stem>.translation-validation.json` report
(`mumei.translation-validation-report/v1`: profile, both hashes,
per-function outcomes and the aggregate). `mumei run` performs the same
check on its staged merged IR, printing the advisory and writing the
report next to `-o` output when one is given (a default `run` leaves
no artifact). The check is advisory: a
`Different` or `UndefinedBehavior` outcome is reported on stderr but
does not block the link, and a module outside the bounded profile —
non-i64 atoms, extern declarations, or no `main` — simply leaves no
report behind.

The existing Z3 library checks a quantifier-free bitvector mismatch query
restricted to inputs where both sides are defined. Unsat yields
`EquivalentWithinProfile`; sat yields `Different`; unknown/resource
exhaustion yields `Inconclusive`, never equivalence — nonlinear
queries (a variable-divisor `srem`, operand-swapped refutations) can
legitimately outrun the budget and stay inconclusive. IR that poisons
on an input the source's own domain binds is reported separately as
`UndefinedBehavior`: no produced value is wrong, the operation itself is.
Default limits are 5,000 ms and 1,000,000 solver resource units; zero budgets
are rejected. Reports contain
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

### Multi-atom native module bundles

When the source declares more than one atom, `mumei native-bundle build` takes
the module path instead of the single-atom one and records
`mumei.native-module-bundle/v1` under profile
`mumei.native-pure-i64-module-linux-x86_64/v1`. The two record families share
the same member set and shape but carry disjoint schemas, so neither parses as
the other and `check`, `recheck-source` and `recheck-llvm` dispatch on the
record's own schema.

```sh
# module.mm — sibling atoms plus one entry point:
# atom helper(x: i64) -> i64 requires: x > 0; ensures: result == x + 1; body: { x + 1 };
# atom main() -> i64 requires: true; ensures: result == 6; body: { helper(5) };
mumei native-bundle build module.mm -o module-bundle
mumei native-bundle check module-bundle module.mm --expected-build-id <trusted-id>
mumei native-bundle recheck-source module-bundle module.mm --expected-build-id <trusted-id>
mumei native-bundle recheck-llvm module-bundle module.mm --expected-build-id <trusted-id>
```

The module producer keeps every Stage A/B toolchain gate — the same pinned
LLVM/cc/linker observations, atomic publish and fresh standard verification —
and additionally requires every top-level item to be an atom and exactly one
zero-argument `main` with a literal `true` precondition. The proof certificate
covers every atom in the module, and the record's source binding carries a
per-atom hash map (`atom_content_hash`, specification, implementation, HIR, MIR
and Translator IR identities) instead of the single-atom fields.

Calls between sibling atoms stay inside the widened i64 profile: each `Expr::Call`
lowers to an uninterpreted `mumei_call_<name>_<arity>` bit-vector function, and
`recheck-source` checks **each obligation independently** rather than folding
them into one formula:

- one postcondition obligation per atom — `requires ∧ call assumptions ∧ ¬ensures`,
  where a call site's result is assumed to satisfy the callee's `ensures`;
- one domain obligation per call site — `requires ∧ site guard ∧ ¬callee requires`,
  proven **without** the sibling ensures assumptions, so no circular bootstrap is
  possible.

Keeping the obligations separate matters: folded into a single conjunction, a
callee whose `requires` is `true` would contribute a vacuous `∧ false` and mask
a real counterexample elsewhere — the regression suite demonstrates exactly this
with a `requires: true` callee whose wrapped-i64 caller postcondition is still
refuted. `verify` itself works over mathematical integers, so a program that
verifies (e.g. `id(9223372036854775807) + 1` with `ensures: result > 0`) is
honestly refuted by the source recheck under the module's wrapping-i64
semantics, which the build inherits.

`recheck-llvm` on a module record runs the existing
`mumei.llvm17-native-module-snapshot/v1` translation validation — per-atom
LLVM body comparison plus the same `__mumei_user_main`/wrapper admission rules —
with the module triple/layout still bound to the record's object-build
observations. The native consumer Evidence surfaces dispatch on the record's
own schema family: single-atom bundles emit `mumei.native-consumer-evidence/v1`,
module bundles emit `mumei.native-module-consumer-evidence/v1` (described in
the producer section below).

Nothing is claimed beyond the single-atom scopes: the result is still
`identity_consistency_only` plus the source-postcondition and LLVM-snapshot
recheck scopes — no object/optimizer/linker/runtime/ABI/final-executable
semantics and no producer authenticity or attestation. The recorded
`source_dependencies_sha256` now pins the intra-source call graph: the
Claim Book convention of `{atom: [sorted direct callees]}` encoded as
canonical JSON — `{"main": ["helper"], "helper": []}` for the module above
and `{"main": []}` for the single-atom profile, whose `main` cannot call.
It is a call-graph pin, not a file-level `import` closure, which the
restricted profiles still do not admit. Records built before this pin
existed carry `sha256(b"[]")` instead, so a pre-change bundle fails the
fresh-binding comparison on the same source and must be rebuilt — the
digest is part of the bound identity, not a stable encoding across
toolchain versions.

Regression gate:
`LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 cargo test --test test_native_bundle --test test_translation_validation -p mumei-core`.

### Multifile import-closure bundles

When the source file declares `import` statements,
`mumei native-bundle build` takes the multifile path and records
`mumei.native-multifile-bundle/v1` under profile
`mumei.native-pure-i64-multifile-linux-x86_64/v1` — a third record family,
mutually unparseable with the single-atom and module schemas. The
file-level import closure admits a deliberately small shape:

- every member's imports resolve through the language resolver's own
  chain, file-relative against that member's own directory first:
  `import "helpers"` in `sub/a.mm` reads `sub/helpers.mm`, and
  `import "sub/dep"` descends into subdirectories of the root's
  directory. `..` segments, absolute paths, drive letters, `\`,
  leading-dot and empty segments are rejected, so member keys can only
  name files inside the root's subtree (at most 16 `/`-separated
  segments; the producer's own filesystem resolution follows the OS,
  exactly like the resolver's `canonicalize`). `std/`-prefixed imports
  additionally fall back to the language's shipped-stdlib chain
  (compiler-adjacent `std/`, working-directory `std/`), and any still
  unresolved import falls back to `MUMEI_STD_PATH` — the producer
  delegates to the language's own `resolve_path`, so every fallback
  matches the resolver byte-for-byte. Resolved bytes pin under the
  importer-prefixed member key, so consumers never re-resolve;
- each import sits alone on its line as `import "<member>" [as <alias>];`;
- members contain atoms and imports only — no other top-level items —
  and the root needs at least one import; nested and transitive imports
  close over the whole pinned member set, diamonds are merged once, and
  import cycles are rejected honestly;
- at most 64 members of at most 64 KiB each, named by the member grammar
  (`/`-separated nonempty `.mm` path segments — no `\`, `:` or
  leading-dot segments); an unaliased import's qualifier is the
  member's last-segment stem, so `x/dep` plus `y/dep` collide and need
  an explicit `as` alias;
- duplicate imports and self-imports are rejected, and no two imports in
  one file may bind the same qualifier — `import "a" as k; import "b" as k;`
  would merge every `k::f` into the same ambiguous bare name.

```sh
# app.mm
# import "helpers" as h;
# atom main() -> i64 requires: true; ensures: result > 0; body: { h::helper(0) };
mumei native-bundle build app.mm -o app-bundle   # reads helpers.mm from app.mm's directory
mumei native-bundle check app-bundle app.mm --expected-build-id <trusted-id>
mumei native-bundle recheck-source app-bundle app.mm --expected-build-id <trusted-id>
mumei native-bundle recheck-llvm app-bundle app.mm --expected-build-id <trusted-id>
mumei native-bundle recheck-artifact app-bundle app.mm --expected-build-id <trusted-id>
```

The producer merges the closure into one canonical text — a post-order
walk emits each member after its own imports (dependencies first,
siblings in declaration order), the root last — and the module pipeline
compiles that text. A member's namespace is file-local: its own import
lines are stripped and its own call qualifiers normalized, so
`h::helper(0)` merges as `helper(0)` — the same normalization the
language's resolved `ModuleEnv` performs. Qualified calls are namespace
syntax, not contract. Every
original member is written under `sources/<name>` at its `/`-separated
member key (real directories on disk) and pinned byte-for-byte in the
record's `sources` map; `source_files` pins the
root name plus `import_graph_sha256`, the file-level
`{member: [sorted imports]}` graph under the same canonical-JSON
convention as the atom-level `source_dependencies_sha256` pin.

Consumers never read a filesystem beyond the `--source` argument: its
basename names the root and its bytes must equal the pinned root member —
the remaining members come from the bundle's pinned `sources/`, and
every recheck re-derives the merge from them, so the checked text is
always the one the record binds. `check`, `recheck-source`,
`recheck-llvm` and `recheck-artifact` run the same module-scope
verification over the merged text, wrapping-i64 semantics included.
Consumer Evidence reaches multifile bundles through
`mumei.native-multifile-consumer-evidence/v1` — the module envelope
shape plus one merge-correspondence record (`2 + 2·atoms`): an
`identity_check`/`identity`-scoped record binds the file-level
`import_graph_sha256` pin to the re-derived merged text the LLVM
snapshot carries. `recheck-llvm --json` emits it, `recheck-evidence`
compares a saved envelope against the fresh one, and `accept-evidence`/
`accept-artifact` report the multifile acceptance scopes, which name
the pinned-member merge re-derivation among the performed checks and
keep the same honesty about what stays unchecked.

Nothing is claimed beyond the module scopes: same identity-plus-recheck
boundaries, no authenticity, and no import-alias pinning (the alias is
namespace sugar the merge erases). Refined scalar `type` declarations
are admitted under the v2 profiles described below; struct/enum member
items and alias pinning stay outside this slice. `std/` members follow the
language's own fallback chain: importer-relative first (a `std/`
member beside the root is an ordinary local subdirectory), then the
shipped toolchain stdlib and `MUMEI_STD_PATH`, which also serves as
the last-resort fallback for any still-unresolved import — each
fallback member then resolves its own imports beside its real file.

### Refined scalar member profiles (v2)

A member that declares refined scalar aliases —
`type Port = i64 where v >= 1 && v <= 65535;` — widens the admitted
fragment under `mumei.pure-i64-refined-scalar/v1`, and a bundle using
them records `mumei.native-pure-i64-module-linux-x86_64/v2` or
`mumei.native-pure-i64-multifile-linux-x86_64/v2`. The profile name is
part of the pinned identity — and it names exactly the fragment the
module needs: a module without type declarations still pins `/v1`, a
module whose declarations all resolve to integer-sort leaves with a
single conjunct pins `/v2` (member `mumei.pure-i64-refined-scalar/v1`),
and only a module needing the widened admission — a `bool` leaf or a
multi-conjunct chain — pins `/v3` (member
`mumei.pure-i64-refined-scalar/v2`). The record itself says which
admission produced it, and `check`/`recheck-source`/`recheck-llvm`
dispatch on the pinned name — a record is always re-admitted under
exactly its own admission, never a wider one.

Admission stays deliberately small and fail-closed. A `type` member is
accepted only under a refined profile (the strict profile still rejects
every non-atom item) and only when it carries no `unit` metadata — the
profile does not model units, so it refuses to discard them silently.
Under the v1 refined admission the base must be a literal scalar in the
integer-sort vocabulary (`i64`, `i8`–`u32`); under the v2 refined
admission (`mumei.pure-i64-refined-scalar/v2`, bundles pinned
`mumei.native-pure-i64-module-linux-x86_64/v3` or
`mumei.native-pure-i64-multifile-linux-x86_64/v3`) the base may also be
`bool` — the verifier binds the operand as `Bool` — or another declared
type name, composing every ancestor conjunct exactly the way the
verifier's chain enforcement does. Cyclic chains stay rejected in both.
`u64`/`usize`/`isize`/`f64`/`Str` leaves stay rejected in both: a
full-width unsigned domain cannot be expressed as an i64 bound and the
obligation vocabulary has no Real/String sorts — widening those needs
a new profile name again. The `where`
predicate must check as `Bool` over its declared operand under the
existing `expression_kind` gate, with no calls admitted inside it.
Duplicate type names and a type named like an atom are rejected, so
`T(` is never ambiguous between a type position and a call.

Signatures resolve through the declared environment: `x: Port` binds
`x` as the base scalar carrying `Port`'s predicates, while an undeclared
signature type name stays rejected as before — only a name the refined
environment resolves can stand in a signature. Alias predicates fold
into the obligations the profile already proves: a parameter's predicates join the signature
domain, so the atom assumes them as an antecedent and every call site
must establish them alongside the callee's `requires` (the same
`callee_precondition` obligation, with the conjuncts instantiated at
the site's argument values); a result's predicates join the
postcondition, so the callee must guarantee them at its `result`.
A predicate whose domain can be empty is refused by the existing
empty-domain gate. A call keeps the callee's declared signature kinds:
each argument must carry the callee parameter's kind (a `bool` —
literal or bool-leaf alias — admits only bool arguments, lowered to
the profile's i64 0/1 bitvector convention) and the call expression
itself takes the callee's return kind, so `-> Flag` results stay
boolean-typed through the call.

Nothing deeper is claimed: the fragment admitted by the profile grew,
the guarantees did not — the same identity-plus-recheck scopes apply,
with no optimizer/object/linker/runtime/ABI/final-executable semantics
and no producer authenticity or attestation. Struct/enum declarations
and `unit`-carrying aliases stay outside the profile.

Regression gate:
`LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 cargo test --test test_native_bundle --test test_translation_validation -p mumei-core`.

### Pinned toolchain artifact recheck

`mumei native-bundle recheck-artifact` is the next C10 consumer stage. It runs
the whole `recheck-llvm` chain — pinned identity, fresh source postcondition
and fresh LLVM snapshot — and then asks whether the pinned artifacts are what
the recorded toolchain actually emits from the pinned inputs:

```sh
mumei native-bundle recheck-artifact module-bundle module.mm \
  --expected-build-id <trusted-64-character-lowercase-sha256>
```

Two staged rebuilds run in a private staging directory inside the bundle's
parent:

- **object**: the pinned `program.ll` is re-emitted with the same fixed
  native target machine the build used; the produced bytes must equal the
  pinned `program.o` and the fresh `ObjectBuild` observation (triple, data
  layout, LLVM version) must equal the record's;
- **link**: the object just proven byte-identical to `program.o` is
  re-linked by the same recorded cc/gcc/clang driver; the produced bytes
  must equal the pinned `program` and the fresh driver/linker observations
  (driver name, version, target, linker family and version) must equal the
  record's.

Any byte or observation mismatch fails closed as `Different` (exit 1);
inconclusive source or translation queries still exit 3 with no artifact
claim. Dispatch follows the record's own schema, so module bundles recheck
through the same command. Like the earlier stages, the whole chain works
on one once-read snapshot: no record or component is re-read between the
`recheck-llvm` checks and the artifact comparisons, so the bytes being
rebuilt are exactly the bytes that passed identity and translation
verification.

`link_build.arguments_sha256` is deliberately not compared: it pins the
build-time argument vector including absolute staging paths, so it is a
build-time observation no recheck can reproduce. Records also pin the
optional `canonical_arguments_sha256` — the same vector with every
path-dependent argument replaced by a stable placeholder (`{output}`,
`{input:i}`, `{toolchain}`) — which the artifact recheck re-derives
from a fresh `link_command` and compares when the record carries it;
pre-field records skip the comparison honestly. The executable's byte
identity is what binds the link.

The success scope is
`source_postcondition_llvm_snapshot_and_toolchain_identity_only/no_execution_or_runtime_semantics`.
The check proves the pinned object and executable are byte-identical to what
this toolchain emits from the pinned IR and object — **determinism plus
pinned-artifact correspondence only**. Neither artifact is executed by this
command, and there is no ABI, runtime, optimizer-path, system-library or
attestation claim; a toolchain that emits different but semantically
equivalent bytes (a different LLVM version, for example) fails closed rather
than being normalized into agreement. The OS exit-status check is the
separate opt-in stage below.

### Pinned executable exit-status recheck

`mumei native-bundle recheck-exit-status` is the execution slice of the
consumer chain. It runs the whole `recheck-artifact` chain — pinned
identity, fresh source postcondition, fresh LLVM snapshot and
byte-identical object/executable reproduction — and then executes the
reproduced `program` once with no arguments and closed stdio, comparing
its OS exit status against the record's `link_build.expected_exit_status`
pin:

```sh
mumei native-bundle recheck-exit-status module-bundle module.mm \
  --expected-build-id <trusted-64-character-lowercase-sha256>
```

The producer pins `expected_exit_status` at build time by running the
just-linked executable once under the same no-argument/closed-stdio
conditions with a 30-second cap. Because the consumer runs the executable
it just proved byte-identical to the pin — never the bundle's own copy —
a matching exit status is an observation about the *pinned* artifact, not
about whatever bytes happen to sit in the bundle directory.

The outcome mapping stays honest about what cannot be evaluated: spawn
failure, timeout, signal termination and a record that pins no
`expected_exit_status` (pre-field records carry none) all exit 3 as
inconclusive rather than claiming a comparison that did not run; an
exit-status mismatch fails closed as `Different` (exit 1) after the
artifact reproduction itself succeeded.

The success scope is
`artifact_reproduction_and_os_exit_status_observation_only/no_runtime_abi_or_semantic_proof`.
A single execution under one environment is an OS-level observation — not
a runtime, ABI, process-semantics or historical-execution claim. On a
host whose OS/arch cannot run the pinned target triple, the observation
is inconclusive by construction.

`mumei native-bundle accept-exit-status` is the acceptance face of the
same stage: it runs the identical chain and, on a matching observation,
emits the anonymous acceptance descriptor (`--json` gives the
`mumei.native-evidence-acceptance/v1` JSON shape) with
`os_exit_status_observation` moved into `checks_performed`. The name is
deliberately the observation — `historical_execution`, ABI, runtime and
process semantics stay in `checks_not_performed`, and `accept-artifact`
keeps `os_exit_status` excluded since it performs no execution. A
mismatched status fails closed (exit 1); every unevaluable case exits 3
with no descriptor.

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

Module bundles emit the sibling envelope
`mumei.native-module-consumer-evidence/v1` — a separate schema so the two
families cannot cross-parse, mirroring `native-bundle`/`native-module-bundle`.
Same envelope fields and `mumei.verification-evidence/v1` record shape, but
`evidence` holds exactly `1 + 2·N` records in canonical order: one bundle
identity record, then per atom in the record's sorted binding map a
`<atom>_source_postcondition` record (subject: the shared whole-module source
digest; specification: the atom's contract identity) and a
`<atom>_source_to_llvm_user_body` record (subject: `program.ll`;
specification: `native-source.mm#<atom>.body` bound to the module source digest).
Records name the module profiles —
`mumei.native-pure-i64-module-linux-x86_64/v1` for identity,
`mumei.native-module-source-recheck/v1` for source postconditions and
`mumei.llvm17-native-module-snapshot/v1` for LLVM comparisons — so a module
report never carries single-atom profile strings, and the configuration
fingerprint records the same pair. An atom whose body contains a `while` is
verified only where the guard fails within the profile bound (4 evaluations),
so its postcondition and translation records claim `bounded` scope with a
`while_loop_guard_evaluations <= 4` bound rather than `unbounded`; the scope
is re-derived from the source text, not trusted from the report.
Module evidence is emitted only on aggregate equivalence, so every atom's
record reports `established`; there is no partial-success module report.
Scope, assumptions, exclusions and `acceptance: not_evaluated` are unchanged.

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
as a regular, non-symlink file of at most 1 MiB — bounded like the record,
not the single-atom report, because a module report carries `1 + 2·atoms`
records. A closed typed decoder dispatches on the saved file's own
`schema_version` into the single-atom or module family, and rejects
malformed/non-UTF-8 JSON, duplicate/unknown fields at every object level,
missing required fields and unknown schemas; the single-atom family still
requires exactly three records at decode, while a module report's atom count
is bound by the fresh-report comparison itself. Report strings and
artifact paths never select tools or files to execute/open. A saved file of
the other family — single-atom evidence against a module bundle or vice
versa — is rejected, never silently reinterpreted.

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

### Native Evidence acceptance evaluation

```sh
mumei native-bundle accept-evidence bundle source.mm evidence.json --expected-build-id <trusted-id> [--json]
```

`accept-evidence` runs **the same evaluation** as `recheck-evidence` — decode
of the saved report, the full identity/source/LLVM recheck chain and the
fresh-report equality comparison — but reports the result in the acceptance
vocabulary. Both the single-atom and module evidence families are dispatched
on the saved file's own `schema_version`, exactly as the replay consumer.

On acceptance, `--json` emits one static anonymous descriptor
`mumei.native-evidence-acceptance/v1` with `acceptance: evaluated`,
`accepted: true`, the scope
`native_bundle_identity_source_llvm_and_saved_evidence_consistency`, and
`checks_performed` naming `bundle_identity_binding`,
`fresh_same_profile_source_recheck`, `llvm_snapshot_translation_recheck` and
`saved_evidence_consistency`. `checks_not_performed` names what stays
unassessed: producer authenticity, historical execution, the pinned
object/linker/artifact recheck (a separate opt-in command), optimizer
semantics, ABI/runtime semantics, OS exit status, grade promotion and Lean
kernel checks. Text mode prints the same scope in prose.

The descriptor echoes no subject, digest or provenance — it is a diagnostic,
not reusable Evidence or a certificate, and the common representation gate
rejects it as Evidence input. Its only binding is `evidence_fingerprint`,
the SHA-256 hex of the accepted report's bytes, binding the descriptor to
the evaluated record without replaying any claim. Non-acceptance paths emit no stdout, including
JSON mode: refutation, difference, malformed/mismatched/foreign-family inputs
and invalid pins exit 1, an inconclusive fresh query exits 3. Acceptance adds
no new trust over the replay consumer — it names what was checked, nothing
more. General acceptance/mapping, authenticity, and final-artifact assurance
remain open.

### Native artifact acceptance evaluation

```sh
mumei native-bundle accept-artifact bundle source.mm --expected-build-id <trusted-id> [--json]
```

`accept-artifact` runs **the same evaluation** as `recheck-artifact` —
identity pin, fresh source and LLVM-snapshot rechecks, then pinned
`program.ll` re-emitted to an object and `program.o` re-linked to an
executable under the recorded toolchain, with the produced bytes required to
equal the pinned artifacts — and reports it in the acceptance vocabulary.
Both bundle families are supported.

On acceptance it emits one anonymous `mumei.native-evidence-acceptance/v1`
descriptor with scope
`native_bundle_source_llvm_and_toolchain_artifact_reproduction` and
`checks_performed` naming `bundle_identity_binding`,
`fresh_same_profile_source_recheck`, `llvm_snapshot_translation_recheck`,
`toolchain_object_executable_reproduction` and
`canonical_link_argument_vector_reproduction` — claimed only when the
record pins it; for a pre-field record the descriptor names it among
the checks not performed instead. `checks_not_performed` keeps producer
authenticity, historical execution, optimizer/ABI/runtime/OS semantics,
grade and Lean, and honestly names `raw_link_argument_vector_reproduction`
— the raw `arguments_sha256` build observation no recheck can
reproduce, as `recheck-artifact` documents.
Because artifact acceptance evaluates no record input, the descriptor emits
no `evidence_fingerprint` key; the bundle identity pin is its binding.

Acceptance is byte reproduction under the recorded toolchain, not execution,
exit-status, ABI/runtime or optimizer-semantics proof. Non-acceptance paths
emit no stdout: difference or failure exits 1, an inconclusive fresh query
exits 3.

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
