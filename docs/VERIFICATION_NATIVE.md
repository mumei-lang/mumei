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
It carries no subject or provenance and is not reusable Evidence, a certificate
or an acceptance receipt; neither common `check` nor this recheck accepts it as
Evidence. Its only binding is `evidence_fingerprint` — the SHA-256 hex of the
rechecked record's input bytes — which lets a holder prove *which* record the
diagnostic covers without replaying any claim or digest as identity. It does not prove historical producer execution/authenticity, binary
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

### Detached producer-signature envelope

`mumei evidence sign <record> --key-file <PATH> --producer-key-id <ID>
[--json]` is the first detached producer-authenticity surface. Any bounded
regular file (at most 4 MiB (4,194,304 bytes); `--stdin` reads the same bound)
is hashed and signed over its raw bytes — the record is never parsed, decoded or
canonicalized, so non-UTF8 bytes sign identically and every pin on the record
stays byte-identical. `subject_digest` is the record's SHA-256 lowercase hex,
the same convention as `evidence_fingerprint`, so a consumer reproduces it
with `sha256sum`. The signed payload is the digest's ASCII bytes, matching the
`crypto_verify_signature` FFI convention, and the signature is emitted as
128-char lowercase hex.

`--key-file` must be a regular file holding exactly the 64-char lowercase-hex
ed25519 secret (32 bytes): short, long, uppercase, prefixed, whitespace-padded
and non-hex material, plus missing and non-regular files, fail closed with
static diagnostics that never echo the key. Decoded key bytes are zeroized
after use. `producer_key_id` is a caller-declared label the consumer pins
separately — it is never derived from the key bytes, so relabeling does not
change the signature. `--json` emits the closed five-field
`mumei.producer-signature/v1` envelope; text mode prints a static scope
summary carrying only the digest.

The envelope is detached: it travels beside the record, never inside it. A
valid envelope binds only the declared key to the declared digest — it
asserts no key or producer authenticity, freshness, revocation, claim truth
or acceptance, and it is not Evidence, a certificate or a receipt.
Representation contract:
[`schema/producer-signature.schema.json`](../schema/producer-signature.schema.json).
Verifying against a pinned public key is the consumer side; key distribution,
root-key revocation and producer attestation remain open.

`mumei evidence accept` adds opt-in detached signature binding on its
`--requirement` and `--requirement-list` routes. The same binding is available
on `accept-list` and `accept-source`: `accept-list` signs the list envelope's
raw bytes, while `accept-source` signs the saved record's raw bytes. Each
checks the subject digest, Ed25519 signature and binding, with optional
producer-key pinning, declared-reference freshness, registry-window checks and
signed registry/revocation documents. Because `producer_key_id` is unsigned,
registry and revocation checks require `--expected-producer-key` to bind that
label to the supplied producer public key. A false supplied check rejects the
acceptance; signature evidence cannot promote an otherwise rejected or
inconclusive requirement.

For single-record `accept` and `accept-source`, `produced_at_signed` is emitted
when the record declares `produced_at`. A true value means the verified
signature covers the record containing that exact timestamp and the envelope
declares the same string; it means “the producer signed this timestamp,” not
“the timestamp is true.” The envelope stamp is unsigned, and the producer's
clock is not authenticated. `accept-list` does not emit this check because its
envelope stamp describes the list, not each record. Key rotation and root-key
revocation (#886 deferred), and clock authenticity remain open. Revocation-list
v2 documents carry a positive `list_version`, `issued_at`, and optional
`not_after`; `--min-revocation-version` requires a root-signed list and checks
the pinned minimum and declared time window, reporting
`revocation_list_current`. This is a stateless check: replay protection holds
only up to the consumer's pinned minimum version, and `not_after` (checked
under the same flag) bounds how long an older signed list stays usable. V1
lists remain usable without this check.
Malformed anchor documents and public-key encodings fail closed; an object
envelope with a nonmatching closed shape is reported as a failed binding.

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

The serialized `transformations` list exposes each stage edge, its optional
output identity and its validation status. Extraction validates none of these
edges; the LLVM edge remains unchecked, while C09's opt-in
`native-bundle recheck-llvm` is a separate check. Unsupported constructs still
fail closed, and no edge is labeled lossy.

The opt-in `mumei build --semantic-contract` path writes a sidecar beside
emitted LLVM IR, using the CompositeV26 module profile. Its SHA-256 binds the
exact `.ll` bytes only; correspondence remains unchecked, and it is not emitted
by default. The consumer recheck below is diagnostic only, not acceptance.

`mumei evidence recheck-sidecar <SIDECAR> --source <SRC> --artifact <LL>`
checks the sidecar shape and artifact hash, freshly extracts the CompositeV26
contracts from caller-supplied source, and rechecks their module obligations.
Existing effect-free v24 sidecars remain byte-identical and recheck under the
widened admission.
This diagnostic does not establish producer authenticity, source-to-LLVM
correspondence, artifact semantics or acceptance; correspondence remains
unchecked.

`mumei evidence accept-sidecar <SIDECAR> --source <SRC> --artifact <LL>`
emits a bounded acceptance descriptor after the same fresh extraction,
artifact-hash check and module-obligation sweep. An optional producer signature
binds the exact sidecar bytes using the existing signature-binding rules; it
does not by itself establish producer authenticity. The descriptor records
that source-to-LLVM correspondence, artifact semantics, parser/encoder/solver
correctness, grade promotion and Lean-kernel checks remain unperformed.
`mumei evidence sign` accepts subjects up to 4 MiB (4,194,304 bytes), matching
the sidecar consumer cap.

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
unchecked even when they pass. General obligation decomposition, stable IR
contracts and formal translation correspondence remain open C02/C03/C08/C09
work.

```bash
cargo test -p mumei-core semantic_contract --lib
cargo test --test test_semantic_contract
```

### Read-only region and slice profile

[C03 #787](https://github.com/mumei-lang/mumei/issues/787) defines
`mumei.readonly-i64-region-slice/v1`, a separate source-model profile,
not a widening of the scalar comparison or composite profiles. Its first
route is opt-in and uses Z3 only. There is no Lean or executable lowering,
artifact sidecar, or C02 Evidence acceptance route for this profile.

#### First source-model slice

```text
mumei evidence check-readonly-slice <SOURCE>
```

The command admits exactly one atom with exactly one explicit `requires`,
exactly one explicit `ensures` and one explicit `body` clause, at least one
shared `ref name: [i64]` input, optional owned `i64` scalar inputs, and an
explicit `i64` result. Input is limited to 65,536 bytes and 256 lexer tokens;
either limit fails closed. Its scalar fragment has comparisons, short-circuit
Boolean operators and conditionals; it admits `len(a)` and `a[i]` only on
admitted input views. Arithmetic, whole-view values, locals, subviews, mutable
parameters, quantifiers, field access, qualified calls and other calls are
rejected. A comparison inside parentheses is also rejected as a parser-resource
boundary; write comparison chains flat or split them with Boolean operators.
Empty slices are valid inputs but cannot be indexed.

The encoder reuses the signed BV64 comparison, short-circuit guard and
array bounds encodings. The cell function is shared by all formals and
indexed by symbolic region and mathematical absolute offset; it does not
use the local-array out-of-bounds zero fallback. Each query is separate:
`requires_bounds` uses only the well-formed entry views, `input_domain`
checks that those views and the precondition are jointly satisfiable,
`body_bounds` and `ensures_bounds` check the guarded loads under the
precondition, and `postcondition` checks the stated result. Each bounds
query reports the result for its whole source clause. It is not permissible
to assume a failed bounds goal in order to prove the postcondition.

The JSON diagnostic pins source, specification and implementation identities
and records binders with atom-level provenance, assumptions and each executed
query. Exit 0 means established within this model; exit 1 with a diagnostic
means a SAT countermodel to a named obligation; exit 1 without a diagnostic
rejects unsupported or invalid input; exit 3 means empty input domain or Z3
inconclusive. The diagnostic is neither reusable Evidence nor an artifact
sidecar; it leaves Lean/LLVM unsupported and correspondence unchecked. No
default verifier, certificate, audit key, Seal Grade or Lean gate is changed.

#### Baseline in the current pipeline

| Stage | Existing capability and limit |
| --- | --- |
| Checked parser → HIR → MIR | [HIR](../mumei-core/src/hir.rs) represents `ArrayAccess`; [MIR](../mumei-core/src/mir.rs) represents indexed places, moves, `Ref`/`RefMut` and parameter modes. [MIR borrow checking](../mumei-core/src/mir_analysis/borrow_check.rs) checks moves and loan conflicts. These compiler facts are useful inputs, not a proof of allocation provenance or native pointer correspondence. |
| C03 source obligations | [Core's extractor/encoder](../mumei-core/src/semantic_contract.rs) uses bounded local-array models (`array_read` models fallback to zero) and, under `CompositeV26`, read-only owned `[i64]` formals with signed in-bounds obligations and `array_param_len_nonnegative`. `CompositeV29` can also describe read-only array results as model pairs. Neither the length/store pair nor a local store ID asserts native pointer validity, lifetime or initialization. The scalar comparison profile still rejects these constructs. |
| Lean and LLVM | [Translator IR metadata](../mumei-core/src/verification/types.rs) records binders and provenance; that metadata is not a memory lowering. The [LLVM emitter](../mumei-emit-llvm/src/codegen/expr_emit.rs) and [LLVM validation pilot](../mumei-emit-llvm/src/translation_validation.rs) already have restricted array paths for v26. A checked index on the existing `{ i64, ptr }` representation cannot on its own establish that the pointer is live, aligned or backed by initialized cells. |

#### Design for the broader fragment

Start with a single atom returning a scalar, using the comparison profile's
explicit scalar semantics plus read-only `[i64]` inputs, `len`, indexed
loads and non-escaping subviews. Subviews are semantic operations, not new
source syntax: a future adapter must map an exact checked source construct
or reject it. Start without calls, loops, stores or region-valued results.

- **Regions and ownership.** An abstract region ID names a live caller-owned
  contiguous allocation of `N` initialized `i64` cells at entry. The atom
  receives shared read permission for the call only; the contents and
  lifetime are stable throughout it. A view is `(region, start, len)` in
  element units. Creating a subview reuses the same region; it never allocates
  or transfers ownership. No view escapes, and region IDs are not addresses.
- **Bounds.** Require `0 <= start <= N` and `0 <= len <= N - start`.
  Require `len <= i64::MAX` for every view's source-visible length.
  Source lengths and indices are signed `i64`; a validated index is converted
  exactly to a nonnegative mathematical offset. A subview `(off, count)`
  requires `0 <= off <= len` and `0 <= count <= len - off`, and uses
  `(region, start + off, count)`. A load requires `0 <= i < len` and reads
  cell `start + i`. Empty views are allowed but cannot be dereferenced.
  Offset arithmetic is mathematical, bounded by the parent, and must not
  be implemented as silently wrapping source or target address arithmetic.
- **Aliasing.** Give each formal view a symbolic region ID, start and length.
  Lowering leaves region equality between formals unconstrained: the proof
  must cover every caller choice allowed by the contract, including equal
  regions with overlapping offsets and distinct regions. A backend must not
  fix formals to equal or distinct regions. A single immutable store per
  region ensures that the same region and absolute offset give the same
  value; equal cell values do not imply aliasing. Do not expose pointer
  comparison or pointer identity as a source operation.
- **Null and initialization.** The model has no null, dangling or
  uninitialized references. An executable boundary must establish that the
  view is backed by the declared live allocation and has the required
  permission and initialized extent. For a first native subset, require a
  non-null, correctly aligned backing pointer even for an empty view; do
  not interpret null as an empty slice or synthesize zero on an invalid load.
  This is a proposed admission rule, not a change to current emitter behavior.

For example, a subview with `off = 1` and `count = 1` requires the
parent length to be at least two. Reading its index zero then denotes
the parent's index one. Two nonempty overlapping views of that cell must
agree, without any assumption that their complete lengths or contents agree.

#### Obligations and backend admission

Separate entry assumptions (live region, initialization, stable shared
permission and well-formed input views) from goals. Check definedness of the
precondition under entry assumptions alone; then use that precondition and
the actual evaluation guard at each body or postcondition access to prove
subview/load definedness before using the selected value. Prove the original
postcondition for the defined body result, and independently check that the
admitted input domain and precondition are satisfiable to flag vacuity. Keep
guards, failed bounds, undischarged assumptions and postcondition failures
distinct; do not assert a failed goal into the model as an assumption.

- **Z3 model:** Use opaque region IDs, an extent per region, and one
  logical store indexed by region and nonnegative mathematical offset.
  Encode signed scalar values
  as BV64 with signed comparisons; convert source indices and lengths
  exactly to the integer offset domain. Under the original assumptions
  and guard, query the negation of each definedness goal and of the
  postcondition. A total SMT store's value outside the view is never a source
  value. SAT is a countermodel to that goal; unknown or timeout is not a proof.
- **Lean candidate:** Interpret the same region table as immutable finite
  sequences of 64-bit values, with the same signed interpretation at the
  scalar boundary. Construct a bounded index (e.g. `Fin N`) only after
  proving the view and access inequalities; read through the parent region,
  not a fresh sequence per view. Generate explicit goals for the same
  assumptions, guards and postcondition. If the translator cannot express or
  relate any of them to the original obligation, report the profile as
  unsupported, not a theorem about a weakened fragment. No change to Lean
  activation, kernel/replay, axiom, trust or freshness gates.
- **Executable subset:** Executable code must have a checked ABI and memory
  boundary for the actual target, including representable byte offset/extent,
  alignment and the exact guarded load behavior. A symbolic region table or
  a Lean model need not compile; model proof does not establish those physical
  premises. Neither route may substitute LLVM's existing zero fallback for
  an undischarged definedness goal or claim that a requested target was built.

Reject unresolved region identity or access semantics, unsupported element
types, nullable or raw pointers, casts and pointer/integer round trips,
arbitrary address arithmetic, uninitialized cells, mutable borrows/stores,
allocation/free, escaping views, FFI and concurrent/volatile/atomic access.
Region identity is unresolved when a view cannot be traced to a formal's
symbolic region, directly or through checked subviews (for example, a local
with no traced origin, a return value or an unsupported operation).
A future mutable profile would need explicit exclusive permissions, store
versions and frame obligations; no availability or fallback to it is implied.
Distinguish unsupported source or lowering from a violated obligation and
from an unavailable backend. Do not erase a view operation, choose an
uninterpreted value for an unsupported load, or fall back to a scalar proof.

#### Provenance and assurance boundaries

Keep original source/specification and implementation identities; record
typed binder/view origins, checked source spans (with precision), region and
permission assumptions, access/guard spans and obligation IDs. Bind the
profile/semantic version and each transformation's input/output identity,
relation and validation status through HIR/MIR and backend goals. A normalized
view or proof-model store is not a native address binding. When an edge is
unchecked, keep it unchecked.

- **C02 (#786):** Record assumptions, exact profile, method, scope and
  unchecked edges in evidence; schema validity or a successful model query
  cannot establish executable premises or promote a descriptor.
- **C08 (#788):** Bind actual ABI/target, build and artifact provenance.
  Identity and signed provenance do not establish memory correspondence.
- **C09 (#789):** Separately validate guarded memory operations against the
  actual LLVM model and, only where supported, later artifact stages.
  Existing limited array LLVM checks do not unfreeze the deferred
  liftable-subset expansion or establish object-level pointer semantics.
- **C10 (#790):** Re-check applicable evidence and refuse an executable
  claim when the required memory/binding/correspondence checks are missing.
  Source-only acceptance must remain source-only.

Existing audit keys, certificate serialization, Seal Grades and Lean
trust/freshness decisions are unchanged. The initial source trace groups
accesses by clause. Finer access-to-goal traces, subview regressions and a
Lean lowering are follow-ups. Enabling an executable route also requires
independent checks for the advertised executable correspondence. Finite
observations are regressions, not substitutes for those checks.

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

Current status (2026-11-26): the Agent roadmap records the
[C04 planner scope](https://github.com/mumei-lang/mumei-agent/blob/develop/docs/ROADMAP.md#c04-planner-acceptance-completion)
and
[C11 plugin-contract scope](https://github.com/mumei-lang/mumei-agent/blob/develop/docs/ROADMAP.md#c11-plugin-contract-acceptance-completion)
as complete as verification-native planner and orchestration contracts.
Completion does not adopt an external verifier, make
adapter self-reports proof, or create a default external-verifier route. The
paragraphs below are chronological implementation slices: wording such as
"next", "remain open" or "remain partial" describes the boundary at the time
that slice landed, not a claim that the overall planner/plugin contract is
still unimplemented. Independent verifier adoption, semantic correspondence,
evidence acceptance and promotion policy remain separate later work.

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
extra members are screened by the module profile but not compared. Edge
mismatch and undefined-behavior disjunctions are checked as independent
solver queries; the configured timeout/resource budget applies to each
query rather than to a whole-function total, while any solver `unknown`
still keeps the outcome inconclusive.

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

The linker includes the embedded `mumei_runtime.c` only when the emitted
object has an undefined symbol matching a function exported by that runtime.
When linked, the record pins `runtime_source_sha256`; older records and
runtime-free builds omit the optional field. Rechecks relink the runtime only
when that recorded hash matches the current embedded source, otherwise they
fail closed as inconclusive. The runtime is a build input only: its behavior
and semantics are outside every verified claim. `settings.runtime` must be
`system-crt+embedded-mumei-runtime-c` exactly when the hash is present and
`system-crt-only/no-mumei-runtime` otherwise.

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
Repinned object/executable bytes may still pass `recheck-llvm` when source and
LLVM checks hold; the separate `recheck-object` checks the public function
symbol surface and direct call edges against the IR, and checks linked
placement and resolved PC-relative call sites for the program's own functions.
CRT/startup code, the dynamic loader, libc, external symbols, non-call
relocations and instruction semantics are not checked; this is not an ABI
or general linker-semantics claim. Nor is there producer authenticity or
attestation that the object was emitted from these IR bytes.
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
`f64`/`Str` leaves stay rejected in those refined-scalar
admissions — the obligation vocabulary has no Real/String sorts.
An `f64` alias base does join at `mumei.pure-i64-composite/v12`, the
same gate literal `f64` leaves carry: the alias parameter binds its
predicates through `fp.eq` exactly like a literal `f64` parameter,
an `==` predicate needs the v19 arithmetic surface, ordering
predicates stay out for good, and float literals remain rejected at
the token level in every admission. `Str` bases stay rejected —
there is still no String sort. `u64`/`usize`/`isize`
leaves join at `mumei.pure-i64-composite/v10` (the same admission the
literal signature leaves need): `usize` folds to the `u64` domain,
`isize` to `i64`, and alias predicates on a `u64` base compare
unsigned exactly like a literal `u64` parameter's clauses.
`i128`/`u128` leaves join at `mumei.pure-i64-composite/v21` (v22
inside enum payloads): the alias parameter flattens to the same two
64-bit input slots the literal leaf takes, and its predicates apply
to the combined 128-bit operand — unsigned for `u128`, signed for
`i128` — under the same tag-conditional domain convention every enum
payload leaf uses. The `where`
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

The `mumei.pure-i64-composite/v24` source admission permits an atom
without a declared return type only when compiler inference resolves
its result to exactly `i64`; all other inferred types remain rejected.
Its specification identity retains `return_type: null` and adds
`inferred_return_type: "i64"`, and a module pins v24 only when an atom
uses this rule. Native module and multifile bundles admit this source
profile at bundle v27, pinning `/v27` only when a non-main atom uses
inferred `i64`; earlier bundle versions keep their original admission.
The `main` atom still requires an explicit `-> i64`, and the single-atom
producer remains unchanged.

The C03 `mumei.pure-i64-composite/v25` source admission adds simple
in-module stateful effect declarations and atom `effects`, `effect_pre`
and `effect_post` annotations. Declarations must have no parameters,
constraints/refinements, includes or parents; atom annotations must
refer to those declarations and their declared states. A `perform`
operation is admitted only as a discarded non-final block statement;
its arguments use the existing scalar checker, and its value is a no-op
in this contract. Effect names are recorded in declaration order,
pre/post maps are sorted, and the whole-source digest binds declarations.
The declarations, sets and pre/post states are recorded but not modeled.
Effect containment, temporal pre/post conformance and transition order
remain separate verifier obligations outside this contract; none of
these features is marked validated. Modules without effect features keep
their existing profile pin and serialized contract bytes. Native module
and multifile bundles use profile v28 for v25 sources, while effect-free
bundles retain their previous profile pins. Semantic-contract sidecar
production, recheck and acceptance now use CompositeV26; effect-free
sidecar bytes remain unchanged.

The C03 `mumei.pure-i64-composite/v26` source admission extends V25
only with exact plain owned `[i64]` parameters. Each parameter is modeled
as an unconstrained BV64-to-BV64 store and a BV64 length; the source
domain assumes the length is nonnegative. Elements remain unconstrained.
`len(a)` and `a[i]` are admitted in the body and contract clauses as
read-only operations. Every indexed read from a parameter adds a signed
in-bounds obligation (`0 <= i && i < len(a)`), discharged separately
from the postcondition. Reads in the body and `ensures` use the
postcondition's `requires` and call/loop assumptions. Reads in `requires`
are checked under the source domain, the bounded-loop exit hypotheses
collected while encoding `requires`, and the read's own short-circuit path
guard, without assuming `requires` itself. These checks do not narrow the
source domain. Although out-of-bounds
reads retain the existing zero-value encoding, a contract is not accepted
unless every parameter-read obligation is proved.

Array parameters cannot be mutated, aliased or used as whole values,
passed to calls or `perform`, or returned. Local-array behavior is
unchanged. Native translation validation requires the exact LLVM
`{ i64, ptr }` formal, models its extracted length and pointer, and
matches i64-indexed GEPs and loads to the source store with guarded
signed bounds obligations. Pointer stores, calls, escape, `phi`,
`select`, `ptrtoint` and unsupported uses fail closed. The object-level
machine-code TV does not support array-parameter signatures and reports
them outside profile, making object acceptance inconclusive rather than
skipping them.

The length model assumes nonnegative lengths. This is an in-language
producer invariant, not a check performed at FFI boundaries. Native
module and multifile bundles pin V26 sources at profile v29; sidecar
production, recheck and acceptance use CompositeV27 when callback
parameters are present. Callback-free V26/V27 sidecar bytes remain
unchanged.

### Scalar callback parameters in semantic-contract sidecars (v27)

The C03 `mumei.pure-i64-composite/v27` admission is limited to semantic-
contract sidecars. It adds scalar-only `atom_ref` function-type parameters
whose argument and result leaves are already supported by the bounded scalar
model. Arrays, `Str`, structs, enums, generic or nested function types, and
unsupported leaves remain outside the profile. Callback contracts retain the
parameter type and clauses in the sidecar. Contract identifiers must use the
verifier's positional aliases (`arg0`, `arg1`, …, `x`, `y`, and `result`);
other identifiers fail closed.

Each callback parameter is modeled as a fresh uninterpreted function named
`fparam_<atom>_<parameter>`, with bit-vector arguments and result at the
existing leaf widths. A call through that parameter returns the function
application. Under the call's path guard and clause-specific hypotheses, the
checker creates definedness obligations for callback argument domains and
the callback's `requires`. The callback result domain and `ensures` are
assumptions. Passing a direct `atom_ref(g)` to such a parameter requires an
exact signature match; when a callback contract is present, module
obligations check that the receiving contract's precondition implies `g`'s
precondition and that `g`'s precondition and postcondition imply the
receiving postcondition. These checks use `g`'s contract, not its body.

Closures, callback values outside a direct callback argument, callback
binding/aliasing/returning/storing/passing onward, non-parameter indirect
calls, callback parameters in `perform` arguments, signature mismatches, and
calls through a passed callback from the receiving atom's own contracts are
rejected for this slice. The disclosure
`callback_contract_assumed_not_runtime_checked` identifies callback
contracts used as assumptions. Callback-free V26 and V27 contract output
remains byte-identical.

Native bundle production/recheck and translation validation remain on
CompositeV26 and reject callback-parameter sources; V27 does not expand
native-bundle or LLVM correspondence guarantees. Sidecar identity still binds
only the sidecar and emitted artifact bytes as documented above; callback
contracts are not runtime checked.

### Inferred refined-alias returns in semantic-contract sidecars (v28)

The C03 `mumei.pure-i64-composite/v28` profile admits an omitted atom return
type when compiler inference names a refined alias whose resolved base is
exactly `i64`. The sidecar erases that return to `i64`: it generates no
refinement-predicate obligation for the result, and callers assume only the
callee's declared `ensures`, not the alias predicate. This is deliberate
because type propagation is not a checked range guarantee — for example,
`Nat + Nat` can wrap outside `Nat` under the existing i64 arithmetic model.
An explicit `-> Nat` keeps its existing result obligation and caller-side
refinement assumption. The `inferred_refined_return_erased_to_base` disclosure
marks use of this weaker inferred-return interpretation. Refined aliases over
non-i64 bases remain unsupported.

V28 is limited to semantic-contract sidecars. Native-bundle consumers and
LLVM translation validation remain pinned to CompositeV26. Sidecars without
inferred refined-alias returns retain byte-identical V27/V28 contract output.

Nothing deeper is claimed: the fragment admitted by the profile grew,
the guarantees did not — the same identity-plus-recheck scopes apply,
with no optimizer/object/linker/runtime/ABI/final-executable semantics
and no producer authenticity or attestation. Enum declarations and
`unit`-carrying aliases stay outside the profile; struct declarations
are admitted only by the composite profile below.

Regression gate:
`LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 cargo test --test test_native_bundle --test test_translation_validation -p mumei-core`.

### Composite struct member profile (v4)

A module may also declare non-generic structs whose fields are all
`i64` or `bool` — `struct Point { x: i64, y: i64 }` — use them as atom
parameter types and read their fields (`p.x`) in `requires`, `ensures`
and bodies. Such a module builds under the member profile
`mumei.pure-i64-composite/v1` and pins
`mumei.native-pure-i64-module-linux-x86_64/v4` or
`mumei.native-pure-i64-multifile-linux-x86_64/v4`. The producer always
runs this widest admission, but the record names it only when the
module actually declares a struct: a module without structs keeps
pinning `/v1`–`/v3` exactly as before, and `check`/`recheck-source`/
`recheck-llvm` re-admit a `/v4` record under the composite gate only.

The encoding is a flat scalar expansion with no new sorts. A struct
parameter becomes one scalar input per field in declaration order, each
bound by its field's declared type exactly like a scalar parameter of
that type (a `bool` field carries the i64 0/1 convention). The
struct value itself is never a scalar: it may only be read through a
field or passed by name to a sibling parameter of the same struct,
which flattens to the same field slots — so the call-site obligation
still checks the callee's `requires` and field domains over the passed
fields. The contract binders report the flattened fields (`p.x`), and
the specification identity includes the struct's field layout.

Admission is fail-closed. Rejected: generic structs, struct invariants,
`impl` methods, `where` constraints on fields, empty structs, duplicate
fields or structs, struct names shadowing a scalar name or colliding
with a `type` or atom name, and fields other than `i64`/`bool`
(`u64`/`usize`/`f64`/arrays/other structs, so no recursion). Under
`/v4`, refined-alias fields stay out as well.

`mumei.pure-i64-composite/v2` widens the field vocabulary to
refined-alias types whose chain ends in an `i64` or `bool` leaf
(`type Pos = i64 where v > 0; struct P { x: Pos }`), under the same
chain rules as refined scalar parameters: chained declarations compose,
and units, cycles and `u64`/`f64`/`Str` leaves stay rejected. The
verifier now assumes a field's whole alias chain for a struct parameter,
the same as for a scalar parameter of that type, and checks it at every
struct literal. The flattened field slot therefore has the same domain in
the build and in every recheck. Such a module pins
`mumei.native-pure-i64-module-linux-x86_64/v5` or
`mumei.native-pure-i64-multifile-linux-x86_64/v5`. A module whose fields
are all literal `i64`/`bool` keeps pinning `/v4`, and a `/v4` record is
re-admitted under the literal-field gate only. Subrange fields
(`i8`–`u32`, literal or as an alias leaf) stay out: the verifier still
binds a struct field by its base sort, without the subrange bound a
scalar parameter of that type carries. A binder spelled like a
synthesized field name (`p_x`, `__struct_p_x` for a struct parameter
`p` with field `x`) is rejected — the verifier and emitter resolve
`p.x` through those names before the struct value itself. Under
`/v4` and `/v5`, struct construction (`P { .. }`), struct results,
struct-valued `let`s, struct values in scalar positions, nested or
computed field targets, a field read through a name a scalar binding
shadows, rebinding a struct parameter, and pass-through arguments of
another struct or shape all stay outside.

`mumei.pure-i64-composite/v3` widens the fragment from struct parameters
to struct values. An atom may return an admitted struct
(`atom make(a: i64) -> P ... ensures: result.x == a`), with `result.f`
binding the returned field slot in `ensures`. A body may build a struct
literal that sets every declared field exactly once from a scalar
expression of the field's kind, bind a struct-valued `let`
(`let q = make(a); q.x`), read a field of a struct-returning call
(`make(a).x`) and pass any of these struct values to a same-struct
parameter. A struct literal still checks each refined-alias field's
predicates when it is built, and a struct result's postcondition
includes those field predicates at the returned slots. A struct-valued
call becomes one uninterpreted application per field slot. Such a
module pins `mumei.native-pure-i64-module-linux-x86_64/v6` or
`mumei.native-pure-i64-multifile-linux-x86_64/v6` only when some atom
uses a struct value. Modules that use only struct parameters keep
pinning `/v4` or `/v5`, and `/v4`/`/v5` records are re-admitted under
their own gates. Still rejected: a struct value in a scalar position,
struct values merged by `if`/`match`, field reads on a struct literal or
on a branch, nested struct literals, rebinding a struct `let`,
assigning to a field (`q.x = ...`), and binders spelled like a struct
`let`'s synthesized field names (`q_x`, `__struct_q_x`) anywhere in the
atom.

The LLVM snapshot recheck pairs the aggregate formal the emitter
produces for a struct parameter with the source's flattened slots: a
literal, non-packed `{ i64, ... }` formal (1–16 `i64` elements, a
`bool` field being its 0/1 i64 carrier) must have exactly the struct's
field count, element `i` is the struct's `i`-th declared field, a
one-index `extractvalue` reads that slot, and a formal passed through
to a call flattens into the same field arguments the source call site
applies. Field order is compared positionally, so a snapshot reading
the fields in a different order is `Different`. For `/v6` struct
values, an aggregate may also be an `insertvalue` chain (one index, an
i64 value), a constant aggregate whose elements are integers or `undef`
(the emitter folds literal fields into one), or a struct-returning
call's result. A call result is one uninterpreted application per slot,
the same applications the source call site makes. A returned aggregate
of exactly the struct's field count compares slot by slot with the
source's struct result. A slot that no `insertvalue` or constant
element defines is never guessed: reading, passing or returning it fails
closed. Aggregates merged by `phi`/`select`, stored, nested, packed,
mixed-width, or paired with a scalar source parameter or result still
fail closed. `check`, `recheck-source`, `recheck-llvm` and the stages
built on it all reach `/v4`–`/v7` bundles.

`mumei.pure-i64-composite/v4` widens the fragment to enum members. A
module may declare a non-generic, non-recursive `enum` whose variant
payloads are `i64`/`bool` scalars or refined aliases on those leaves
— uniform across the variants at each payload index — and take the
enum as an atom parameter. An enum parameter flattens to a tag slot
plus one slot per payload index up to the widest variant's arity: the
tag is the variant ordinal bounded `0 <= tag < variants` and slot `i`
is the `i`-th declared payload, matching the `{ i64, ... }` aggregate
the emitter builds (tag `insertvalue` first, payloads bit-preserved).
A `match` on an enum parameter admits `Variant` arms (binding the
declared payload positionally) and requires a final unguarded `_`
arm; the arm's hit condition is `tag == ordinal`. Enum parameters
pass through by name to a same-enum sibling parameter, flattening to
tag plus payload slots at the call site, and `let` binds the value
pass-through. Such a module pins
`mumei.native-pure-i64-module-linux-x86_64/v7` or
`mumei.native-pure-i64-multifile-linux-x86_64/v7` only when it
declares an enum; enum-free modules keep their narrower names and
older records re-admit under their own gates. Still rejected: enum
construction and results, `Literal` and catch-all `Variable` arms on
an enum scrutinee, variant arms with the wrong arity or an undeclared
variant, mixed-type payload indices, recursive or generic enums, enum
values in scalar positions, field reads on an enum, and rebinding an
enum `let`.
As with the refined profiles, the admitted fragment grew and the
assurance did not: no translation, optimizer/object/linker/runtime/ABI
or final-executable semantics and no producer authenticity or
attestation are claimed.

Regression gate:
`LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 cargo test --test test_native_bundle -p mumei-core`.

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

### Pinned object symbols, call edges, and linked placement correspondence

`mumei native-bundle recheck-object` runs the full artifact recheck and then
compares the reproduced `program.o` symbol surface and call edges with the
pinned `program.ll`. The externally visible IR function definitions must equal
the object's non-local defined text symbols, and each non-empty undefined
object symbol must be declared in the IR — or be a compiler-rt `__*ti3`
helper, or a libc call the backend injects for an `llvm.memcpy.*`/
`llvm.memmove.*`/`llvm.memset.*` intrinsic edge the same caller's IR
surface records (`memcpy`/`memmove`/`memset`). For each externally visible function,
the direct callees in the IR must equal the call-relocation targets inside that
function's object byte range.

It then checks each object function against its same-named global defined text
symbol in the reproduced executable: sizes must match, and function bytes must
match except at supported four-byte x86-64 `PLT32`/`PC32` relocation fields.
Each field must equal the resolved PC-relative value for its target's
executable address. Only the program's own functions are compared; CRT/startup
code, the dynamic loader, libc and external symbols are outside this check
(their relocation fields are still verified against the emitted PLT stubs).

Local/private definitions, indirect calls, unsupported formats or relocations,
external relocation targets that resolve to neither a linked executable
symbol nor a loader import, and ranges that cannot be sliced or assigned
unambiguously make the relevant check inconclusive (exit 3). A loader
import — a global non-definition symbol in the executable's symbol or
dynamic-symbol tables — resolves through the PLT stub the linker emits
for it (the Nth `R_X86_64_JUMP_SLOT` in `.rela.plt` owns the Nth 16-byte
`.plt.sec` stub, or the Nth `.plt` stub after the 16-byte resolver
entry), so its call field is verified like any other target; an import
whose stub cannot be located must still have its call field point inside
the PLT region (any `.plt`, `.plt.sec` or `.plt.got` section —
inter-section gaps do not count) — a field retargeted elsewhere is a
mismatch — but keeps
its field bytes unverified rather than making the check inconclusive. A symbol,
call-edge, function-byte, size or resolved-call-target mismatch exits 1. This
is the third bounded C10 Option B step. Non-call relocations and instruction
semantics are not checked; this is not general linker-semantics or ABI
validation. Full object-code translation validation remains the deeper Option B
endpoint.

### Acyclic-branch direct-call C10 Option B TV profile

`mumei native-bundle recheck-machine-tv` first reproduces the artifact and
checks object symbols and call edges. It checks linked object correspondence
when checkable, then disassembles the reproduced object's
functions by linear sweep and uses Z3 to compare straight-line and acyclic
control-flow functions in the fixed machine-instruction profile with their
`pure-i64` source-atom models. Direct calls are matched in order to the source
`CallSite`s and compared as uninterpreted sites: their argument expressions
must match the first six SysV integer argument registers and any stack-passed
argument slots, while each call result is the corresponding source call-result
term. On each path, a machine call skips
source sites whose guards are provably false on that path and matches the
first site whose guard is provably true; an undecided guard is
`conditional_call`.
If an external runtime call target resolves to neither a linked executable
symbol nor a loader import, it prevents linked-object correspondence and
`recheck-object` remains inconclusive; machine-TV reports that link check as
not performed and can still report the loop function as outside profile.

The assumed convention places integer arguments in `rdi`, `rsi`, `rdx`,
`rcx`, `r8`, and `r9`, with the result in `rax`; it is declared and assumed,
not verified. Caller-saved registers are fresh after calls and callee-saved
registers are preserved by assumption. The register convention and
callee-saved preservation are not ABI claims. Direct final `jmp` tail calls
with at most six arguments are treated as calls followed by returns; wider
tail calls are outside the profile. Functions containing both
conditional branches and calls use the per-path guarded call matching
described below. A small abstract stack handles register `pushq`/`popq`,
immediate `%rsp` adjustments, and outgoing stack-argument setup using
`pushq` or `movq` register/immediate stores to `off(%rsp)`; `pushq` may
also read an incoming stack-argument slot or a recorded frame slot
directly, the form LLVM uses to forward a spilled argument to a callee.
For up to 16 integer parameters/call arguments, the assumed convention places
incoming argument `k >= 7` at entry-relative offset `+8*(k-6)` from `%rsp`,
and outgoing argument `k >= 7` at call-time offset `+8*(k-7)`. This is an
assumption, not an ABI claim. The incoming return-address/argument area is
read-only. Only 64-bit `movq` loads and `addq`/`subq`/`imulq`/`andq`/`orq`/`xorq`/`cmpq`/`testq`
source reads from aligned, declared incoming slots are supported; other memory
forms and invalid slots remain outside the profile. Loads from frame slots
below the caller's argument area read back only values the function itself
stored there on the same path (register spill and reload); unrecorded offsets
stay closed. A 128-bit `movaps`/`movups`/`movdqa`/`movdqu`
`N(%rsp)`↔xmm transfer moves two adjacent slots at once — the vectorized
form LLVM uses to forward a pair of spilled arguments into a callee's
outgoing area — with each qword half resolving like the scalar forms
(either incoming-leaf class, or a recorded frame store; stores stay
inside the frame). Writes to entry-relative offsets `>= 0` and tail calls
with more than six arguments are outside the profile. Outgoing stack-argument
slots are discarded after a call, so later memory reads from them fail closed.
`%spl` byte writes are modeled, but make the stack pointer untrackable, so a
function containing one cannot complete a supported return path.

Conditional branches and local unconditional jumps must target a decoded
instruction start inside the function. Bounded loops use `LOOP_BOUND = 4` and
the same `loop_bound_exits` input domain as LLVM translation validation. A
per-path sixth visit to an instruction is dropped only when its path
conditions, input bindings, parameter domains, definedness, call-result
bounds, and `loop_bound_exits` prove Unsat; Sat or Unknown returns
`loop_bound`. Multiple `retq` instructions are supported, and every explored
path must return with the stack balanced; falling off the function is outside
the profile. Targets outside the function or not at an instruction boundary
are `branch_target`.

`cmpb`/`cmpl`/`cmpq` and `testb`/`testl`/`testq` set flags without changing
registers. `subl`/`subq` and `xorl`/`xorq` also set flags; `andl`/`andq` and
`orl`/`orq` update their destination and set logical-result flags.
Compare/subtract flags retain operand width and model signed and unsigned
comparisons, equality, and the result sign bit. Test, and, or, and xor flags
model zero/sign with carry and overflow clear. The
supported condition codes are `e`/`z`, `ne`/`nz`, `l`/`nge`, `le`/`ng`,
`g`/`nle`, `ge`/`nl`, `b`/`c`/`nae`, `be`/`na`, `a`/`nbe`,
`ae`/`nb`/`nc`, `s`, and `ns`. `cmov<cc>l` and `cmov<cc>q` with register
operands are modeled as conditional expressions. `set<cc>` with an 8-bit
register destination writes the condition as zero or one; memory destinations
are unsupported.

The supported byte-register views are `al`, `cl`, `dl`, `bl`, `spl`, `bpl`,
`sil`, `dil`, and `r8b` through `r15b`. An 8-bit write preserves bits 63..8;
32-bit writes zero-extend. `movb` models an immediate or 8-bit register source
written to an 8-bit register destination other than `%spl`; memory operands
remain unsupported.
`ah`, `bh`, `ch`, `dh`, and 16-bit registers are unsupported.
`movzbl`/`movzbq` and `movsbl`/`movsbq` zero- or sign-extend an 8-bit register
source.

`shlq`/`shll`, `shrq`/`shrl`, and `sarq`/`sarl` accept an immediate count, the
implicit count of one printed without a count operand, or a `%cl` count. Counts
are masked to six bits for 64-bit operations and five bits for 32-bit
operations; a masked zero leaves the destination unchanged, and 32-bit shift
results zero-extend. Source definedness requires `0 <= n < 64`, excluding
negative and 64-or-larger counts. A masked-zero shift preserves the prior
flags. For nonzero shifts, only `e`/`z` and `ne`/`nz` conditions are modeled
from the result's zero flag; other condition codes after a shift are
unsupported. With
a symbolic `%cl` count, the bounded path context must prove whether the masked
count is zero or nonzero to select those flag behaviors; otherwise flags
become unknown and a later flag read is outside the profile.

Register-only `incl`/`incq` and `decl`/`decq` wrap at the operand width, with
32-bit results zero-extended. Their flags are modeled only through the result
zero flag, so only `e`/`z` and `ne`/`nz` reads are supported after these
instructions. Register-only `negl`/`negq` model `0 - operand` and its compare
flags, including carry and signed overflow. `notl`/`notq` complement a
register without changing flags. These unary operations do not admit
memory-operand forms.

`leaq` models address arithmetic, not a memory access. Its supported forms
include a base plus index with implicit scale 1, with or without displacement,
as well as explicit scales 1, 2, 4, or 8. A missing base with an index is
supported; other unmodeled addressing forms remain outside the profile.

`cqto` sign-fills `%rax` into `%rdx`; `cltd` sign-fills `%eax` into `%edx`,
with the usual zero-extension of the 32-bit destination. Register-source
`idivq`, `divq`, and `divl` model the architectural dividend, quotient, and
remainder widths. A register-source one-operand `imulq` models the signed
128-bit product in `%rdx:%rax`. Each division path must prove a nonzero
divisor and that the quotient fits the destination width under the bounded
input domain, loop-exit domain when present, source definedness, call-result
bounds, path conditions, and input/register bindings. A satisfiable violation of that
obligation is `division_trap`; a solver `unknown` stays inconclusive rather than becoming
either a trap witness or a successful comparison. Memory operands and other division forms
remain unsupported. This is a bounded no-trap obligation, not a runtime or ABI guarantee.

`mov`, `movabsq`, `lea`, `not`, `push`, `pop`, and `nop` preserve flags.
Initial flags and flags after other arithmetic or calls are unknown, so a
condition that reads them is outside the profile. At conditional forks, paths
proved infeasible under the declared domain, loop-exit domain when present,
definedness, call-result bounds, input/register bindings, and accumulated path conditions are pruned; paths
with Sat or Unknown solver results are retained. Symbolic execution is bounded
at 64 completed paths; exceeding that bound is `path_bound`.

The `input_domain` is
`signed_i64_inputs_with_signature_bounded_param_domains_where_division_and_shift_defined_not_restricted_by_requires`.
The comparison asserts the source model's signature-bounded parameter domains,
division/shift definedness, and bounded-loop exit domain when present, but does
not restrict inputs by `requires`.
When the equivalent source model has a loop, machine-TV JSON `input_domain`
appends `_and_loops_exiting_within_bound_{LOOP_BOUND}`. This label follows
source loops even if LLVM fully unrolls them into acyclic machine code;
loop-free bundles keep the base label above.
For a module or multifile bundle, machine-TV re-derives source models with the
same `module_refined_admission` or `multifile_refined_admission` helper as the
LLVM recheck, using the profile pinned in the once-read bundle record. The
single-file LLVM recheck has no admission argument, so its machine-TV path uses
`RefinedAdmission::Reject`.
If any equivalent source model has a refined signature, `input_domain` uses the
LLVM module label
`module_members_signed_i64_inputs_with_refined_scalar_signature_predicate_domains_not_process_exit_status`,
with the loop-bound suffix above when applicable. Refined scalar parameter
predicates are asserted through `param_domain`; return refinements remain
postconditions and are not added to the machine return-value equality.
Models with post-state writes, bounded strings, unsupported composite or enum
body features, or unsupported f64 operations fail closed as
`unsupported_signature`. Leaf-function bounded arrays may use qword
`%rsp`-relative red-zone accesses only when each effective offset is proved
within `-128..=-8`; the model assumes the red zone is not clobbered. Leaf
red-zone accesses in functions with calls (`frame_memory_call`) or stack
adjustments (`frame_memory_stack_adjust`) remain fail-closed. Non-leaf bounded
arrays may use one allocated entry-relative frame region per path, with each
qword access proved inside that region. Calls are allowed only under the
explicit assumption that callees write below their return address and that no
stack-argument slot occupies the region; this is not a validated ABI claim.
The region closes when `%rsp` is deallocated past its lower bound and cannot be
reopened. Accesses to deallocated or reallocated regions fail as
`frame_memory_bounds`, and a stack-argument call whose slot would be routed
through the region fails as `call_arguments`. Non-qword accesses and
`.rodata`/`Str` memory remain fail-closed. Equivalent functions using this
allocated-memory model carry
`_allocated_frame_memory_assumed_callee_isolated`. Scalar subranges and
refined signatures remain supported because their input restrictions are
carried by `param_domain`.
`u64` leaves are also admitted as full-width 64-bit bitvectors with no
additional parameter-domain constraint. Unsigned comparisons, division,
remainder and logical right shift retain their unsigned bitvector semantics;
division and shifts are compared only where defined.
Refined predicates over `u64` bases compare unsigned; documented v10 literal
wrapping (`0 - 1` is `u64::MAX` in a `u64` context) is unchanged, and general
width-aware literal spelling above `i64::MAX` remains open.
`i128`/`u128` remain
supported as scalar bitvectors assembled from two unconstrained 64-bit input
slots, low then high. Their argument pair must fit in consecutive remaining
GPRs, and scalar results use `rax`/`rdx` (low/high). The bounded machine
profile models add/subtract, negation, signed/unsigned comparison,
pass-through, direct scalar calls and variable shifts. A variable
`shl`/`lshr`/`ashr i128` lowers to the `shld`/`shrd` double-shift
pair plus the `test $0x40,%cl`/`cmov` fallback for `count >= 64`;
the lifter models the double shift as a 128-bit concat shift and the
fallback sequence follows the normal `test`/`cmov` path, so the whole
emitted sequence compares over the source's defined shift domain.
Wide multiplication remains
fail-closed as `unsupported_instruction`: the bounded solver returned
`Unknown` for LLVM's limb-product sequence. Wide struct leaves remain
fail-closed. The compiler-rt 128-bit helpers the backend emits for
`sdiv`/`srem`/`udiv`/`urem`/`mul i128` — `__divti3`, `__modti3`,
`__udivti3`, `__umodti3` and `__multi3` — are modeled directly: no
source call site is consumed, the lifter applies their SysV register-
pair convention (operands in `rsi:rdi` and `rcx:rdx`, result in
`rdx:rax`), the division and remainder helpers carry the same
nonzero-divisor obligation as `divq`/`idivq`, and a callee name that a
remaining source call site resolves to is a user function rather than
compiler-rt, so it follows the normal call path. The object/link
correspondence treats backend-injected ti3 symbols as expected
undefined symbols whose relocations must resolve to the linked helper;
a ti3 name the IR itself declares or calls stays a real call edge.
Whether a given helper satisfies the source operator is still
checked per function — `srem i128` stays inconclusive because the
source-to-LLVM recheck does not decide it.
Unknown per-path equivalence queries retry the identical obligations with
`simplify → smt` under the same budget; constant-divisor magic-number
sequences may remain inconclusive.
Larger multiply-heavy functions may remain inconclusive within the solver
budget; this is fail-closed and does not imply inequivalence.
Outside-profile reasons include `argument_count`, `branch_target`,
`call_arguments`, `call_order`, `conditional_call`, `cyclic_control_flow`,
`division_trap`, `frame_memory_bounds`, `frame_memory_call`,
`frame_memory_stack_adjust`, `loop_bound`, `not_a_source_atom`, `path_bound`,
`unsupported_flags`, `unsupported_instruction`, and `unsupported_signature`.
A direct-call callee
or count mismatch is reported as `call_order`. Direct calls in branching
functions are matched per path against ordered source `CallSite`s. Provably
false source guards are skipped under the parameter domain, loop-exit domain
when present, definedness, call-result bounds, current path conditions, and input-to-initial-register
bindings; a guard that is undecided, or a machine call with no active source
site, is `conditional_call`. The emitted cancellation-poll call returns zero
under the no-cancellation-requested assumption and clobbers the other
caller-saved registers; cancellation and runtime semantics are not verified.
A missing outgoing stack slot is
`call_arguments`; an argument count above 16 is `argument_count`. At path
completion, remaining source sites must all be provably false. Relocation
coverage is checked across the union of
completed paths. A terminal direct tail-call `jmp` is supported on a branch
arm; a tail-call jump that is not the final decoded instruction remains
outside the profile as `unsupported_instruction`. Unsupported-signature
functions with reachable cycles remain `cyclic_control_flow`; other
unsupported signatures, invalid branch targets, other instructions outside the
profile, and functions without a matching source atom are outside the profile.
The C `main` wrapper,
CRT/startup code, dynamic loader, libc and runtime are not covered. This
remains a bounded per-instruction Option B
profile, not full object-code translation validation or runtime assurance.
The descriptor performs `machine_code_leaf_function_tv` only when an
equivalent call-free, branch-free function is checked,
`machine_code_direct_call_site_tv` only when an equivalent caller is checked,
`machine_code_forward_branch_function_tv` only when an equivalent function
has branches that all go forward, and
`machine_code_acyclic_branch_function_tv` only when an equivalent function
has a backward but acyclic branch. The new
`machine_code_conditional_call_function_tv` is performed when an equivalent
function has both a conditional branch and a call; those functions remain
counted as direct-call functions and are not also counted as forward- or
acyclic-branch functions. `machine_code_bounded_loop_function_tv` is performed
only when an equivalent machine function has a reachable loop. The descriptor
lists `machine_code_tv_loop_executions_beyond_bound` and
`runtime_cancellation_semantics` as not performed whenever an equivalent
source loop is present, including when LLVM fully unrolls it into acyclic
machine code. It lists
`machine_code_tv_cyclic_control_flow_functions` as not performed only when a
function outcome is `cyclic_control_flow` or `loop_bound`. The not-performed
vocabulary changed from `machine_code_tv_backward_branch_functions` to
`machine_code_tv_cyclic_control_flow_functions`; mumei-agent consumers must
adopt the new bounded-loop descriptor names and this conditional vocabulary.
Each Equivalent function also reports `callee_saved_preservation` as
`preserved`, `violated`, or `unknown`, independently of its return-value
equivalence result. `machine_code_callee_saved_register_preservation` is
performed only when at least one function is Equivalent and all such functions
are preserved; otherwise it is not performed when an Equivalent function
exists. When performed,
`machine_code_callee_saved_preservation_assumed_for_external_callees` remains
not performed. The machine-TV call model assumes direct callees and the
cancellation poll preserve callee-saved registers, so the claim is
compositional within a module only when every source callee is independently
proved preserved; external and poll callees remain assumptions. The register
argument/result convention itself remains assumed, not validated. Agent
consumers must adopt the new preservation check names and function field.

The machine-TV and object-acceptance JSON descriptors also carry
`object_translation`: a metadata record (Step 1 of
`docs/DESIGN_OBJECT_TRANSLATION_CORRESPONDENCE.md`) that emits the
`source_atom -> machine_code`, `llvm_ir -> object_code` and
`object_code -> linked_executable` transformation relations — the same
three edges `SemanticContract::emit_native_object_transformations`
declares on contracts prepared under this profile — plus their
`output_identity` digests, a per-function
correspondence table (`ir_function`, `object_symbol`, `object_section`,
`object_offset`, `object_size`, `executable_address` when resolvable), and
every text-section relocation with its section, offset, kind, target symbol,
addend, and the computed pc-relative `resolved_value` when it can be derived.
The source-to-machine edge names
`checked_source_machine_tv_within_profile` when the machine-TV
evaluation accepted every source-atom function — and, when a
`main` entry wrapper applies to the module, only when that wrapper
itself evaluated Equivalent — and
`unchecked_not_translation_validated` when residual unverified
functions remain — the edge mirrors the check block's verdict rather
than claiming coverage it does not have. It carries no
`output_identity` — machine code is evaluated in memory and never
persisted as an artifact — and is appended last so the two
digest-bearing edges keep their positions in the list. `llvm_ir -> object_code`
names the `checked_llvm_ir_object_model_correspondence` method and reports
the promotion result in a separate `result` key —
`checked_correspondence` only when the `llvm_ir_object_equivalence`
item checked and the design memo's promotion conditions all ran:
the function-enumeration and relocation-record correspondence items
checked, the link correspondence ran, and the canonical argv
reproduction compared; `inconsistent_correspondence` when the
`llvm_ir_object_equivalence` item or any of the correspondence gate
items found a divergence; `unchecked_not_translation_validated`
otherwise — so a report whose functions are outside the liftable
subset, solver-undecidable, or missing a promotion gate does
not claim the edge was validated. A link or canonical-argv mismatch
never appears in this field: it ends the recheck pipeline as
`Different` before a report exists, while a skipped link or argv
comparison leaves the edge unchecked rather than inconsistent. The link
edge names the `checked_object_link_correspondence` method only when that
check ran and reports the check's outcome in a separate `result` key so
method and result are never conflated in one field.
Emitting this record performs no `object_translation_correspondence` check —
it stays absent from both check lists as before.

Step 2 adds `object_translation.correspondence`: an inspection of the emitted
metadata. Each entry of `correspondence.checks`
reports one item's `result` — `checked` when the comparison ran and passed,
`unchecked` when it could not run, or `inconsistent` with the offending names
or offsets in `detail`. Step 3 of the record pipeline (aggregate status
wiring) aggregates those results:
`correspondence.status` is `checked_correspondence` only when every item
checked, `inconsistent_correspondence` when any item is inconsistent, and
`unchecked_not_translation_validated` otherwise; `object_translation.status`
moves from `metadata_only` to `checked_correspondence` on the same condition.
Neither is a verdict-bearing claim — the record stays a report and no check,
grade or verdict is promoted. The items are `function_coverage` (every IR-defined
function maps to exactly one record function and vice versa),
`function_count`, `relocation_targets` (every relocation target resolves to a
record function, a linked executable symbol, an IR-declared callee, a
compiler-rt helper, or a loader import the link leaves to the dynamic
linker — backend-injected calls such as `llvm.memcpy`'s `memcpy`
relocation land here; each is resolvable through the assumed-external
vocabulary and named in `detail` as
`assumed-external: [names]`; the relocation record itself marks these
targets with `assumption: "assumed_external_callee"` — the same
`assumed_*` assumption category machine-TV uses for external callees,
under a distinct name because the claim is loader resolution only,
not callee-saved register preservation), `relocation_bounds` (every relocation offset falls inside
an enumerated function's section range), `relocation_resolved_values`
(every relocation's pc-relative value recomputed; an import's call field
is verified against the PLT stub the linker emitted for it — the Nth
`R_X86_64_JUMP_SLOT` in `.rela.plt` owns the Nth 16-byte `.plt.sec`
stub, or the Nth `.plt` stub after the 16-byte resolver entry — and an
import whose stub cannot be located stays honestly `unchecked` with its
field bytes unverified, though the field must still point inside the PLT
region (any `.plt`, `.plt.sec` or `.plt.got` section — inter-section
gaps do not count) or the link check reports a mismatch), and
`llvm_ir_object_equivalence` (the design memo's §7 Step 3 direct
comparison: each source atom's LLVM-IR encoding and its object function,
lifted by the machine-TV lifter, are compared over the shared
`model.inputs`; on every lifted path the machine return must equal the
IR return of a covering edge whose definedness obligations hold —
definedness domains are checked for divergence in the IR→object
direction only, so an object that produces a value where the IR is
undefined reports `inconsistent`, while an IR edge that diverges where
the source domain is unbounded may surface as `inconsistent` or
`unchecked` pending review of the domain boundary). Divergence is
reported only for inputs the source defines: the path assumptions
assert the source parameter domain and definedness obligations, so
inputs where the source itself is undefined — e.g. a 64-bit shift by
64 or more, which x86 answers by masking — are excluded rather than
reported as IR↔object differences. The comparison is
scoped to the admitted signature subset — scalar 64-bit parameters
and scalar results carrying integer or f64 bit-patterns (f64 compares
pass-through/select only; FP arithmetic itself stays out), scalar
`i128`/`u128` on wide-scalar modules, all-`i64` aggregate results
(register-resident or `sret`), scalar-`i64` call sites, and bounded
local memory; loops and wider shapes stay out. Functions outside the subset, or whose IR fails to
encode, record their verbatim `NotLiftable` reason
(`unsupported_signature`, `unsupported_ir`, `unsupported_instruction`,
`lift_failed`, `no_liftable_path`) in `detail`; a comparison
that ran but the solver could not decide records `unknown`. The item is
`checked` when at least one function compared equivalent, none
diverged, and none is `unknown`; `inconsistent` when any function
diverged, and `unchecked` when none could be lifted or any stayed
undecidable (`unknown` is fail-closed — never counted as skipped). An
`inconsistent` item is
reported in place; it is not promoted into a verdict or a performed check.
A further item, `hir_llvm_model_correspondence`, runs the HIR-anchored
model comparison of `docs/DESIGN_HIR_LLVM_CORRESPONDENCE.md`: each parsed
source atom is lowered against the bundle's module env and compared with
the emitted LLVM IR by `validate_hir_pure_i64_leaf`, over the member
subset the atom's call graph reaches (the emitter's `__mumei_user_main`
member stands in for the `main` atom; the C `main` wrapper stays out of
scope). Call sites are modeled the way the IR `call` encoding models
them: scalar arguments pass their leaf, an all-`i64` aggregate argument
splices its leaf slots in declaration order, and a composite result
applies one uninterpreted `mumei_call_field` function per field —
resolved against the module's declared callee signatures, with any
parameter or result type outside the leaf subset failing closed. A
generic instantiation that appears only in a callee's signature
(`caller() { mk3(1, 2, 3) }`) is specialized from the callee's
signature the same way the atom's own is. The item is `checked` when every compared atom proved equivalent
within the leaf subset — atoms outside the subset are named in `detail`,
never silently covered — `inconsistent` on any divergence, and
`unchecked` when no source atoms or IR were on hand. The
`hir → llvm_ir` transformation entry reports the same aggregate as
`result` — `checked_correspondence` when every compared atom proved
equivalent, `inconsistent_correspondence` on any divergence, and
`unchecked_not_translation_validated` when validation could not run —
so `validation: "checked_hir_llvm_model_correspondence"` names the
method while `result` stays honest about coverage.

A further item, `object_data_correspondence`, covers the non-`.text`
sections the instruction-selection checks do not: every IR data
global's initializer bytes are compared against the like-named object
data symbol's contents — `.bss` symbols carry no file bytes, so their
claim is an all-zero image of the symbol size. An IR global whose
name is absent still verifies when its exact initializer bytes sit in
an initialized data section, because LLVM merges identical
`unnamed_addr` constants and emits anonymous ones with no symbol at
all (`.rodata.str1.1` is symbol-free); the `detail` entry records
that outcome as `merged`. The item is `checked` when every derivable
global verified (and trivially `checked` when neither side defines
any data), `inconsistent` on byte divergence, and `unchecked` when a
global is missing from the object or its initializer is outside the
derivable shapes — power-of-two-byte integers up to 64 bits (i8–i64;
`i24`-style widths whose store size is not a power of two allocate
padded bytes LLVM does not describe without a data layout, so they
stay `underivable`), f32/f64, i8 strings, and uniform-element arrays
of those; pointers, structs and wider shapes stay `underivable` in
`detail`, never zero-filled. This item stays
out of the `llvm_ir → object_code` edge's promotion gate — the
relation name covers instruction selection only.

A sibling item, `object_symbol_attribute_correspondence`, covers the
symbol-attribute gap the name/definition checks do not: every defined,
named IR entity — function or global — contributes its linkage, mapped
to the expected ELF binding class (`global`/`local`/`weak`), and is
compared against the like-named object symbol's binding. Entities
carrying `unnamed_addr` are excluded entirely (LLVM may merge or drop
their symbols, so no binding claim survives — for globals the bytes
are covered by `object_data_correspondence`; `unnamed_addr` functions
simply carry no attribute claim), and linkages with no byte-exact
object counterpart (`available_externally`, `appending`, DLL and ghost
variants) report `underivable`. Calling convention is not
representable in ELF symbols, so binding comparison is the honest
subset. The item is `checked` when every derivable entity matched,
`inconsistent` on a binding mismatch, and `unchecked` when a symbol is
missing, bound by conflicting duplicates, or underivable. Like the
data item, it stays out of the `.text` edge's promotion gate.

Step 5 of the record pipeline surfaces the aggregate status on the text
path too: `recheck-machine-tv` and `accept-object` without `--json`
print `Object translation correspondence: <status> (N of M items
checked).` and one `inconsistent: <item> — <detail>` diagnostic line per
inconsistent item, so a divergence is named without reading the JSON
descriptor. The lines are a report only — they change neither the
record nor any verdict, grade, or performed-check claim.

The same descriptors carry `source_machine_tv`: the design memo's §7
Step 2 gate emitted as metadata. `source_machine_tv.status` is
`checked_source_machine_tv_within_profile` when the machine-TV decision
is accepted — every source-atom function and, when applicable, the entry
wrapper came back `equivalent_within_profile` — and
`unchecked_not_translation_validated` otherwise.
`non_equivalent_functions` enumerates each function that did not compare
equivalent with its `outcome` (and `reason` for `outside_profile`), and
`unchecked_domains` lists the descriptor's `checks_not_performed` names,
so the residual unverified domain is enumerable from the record. The
gate covers source↔machine equivalence only:
`source_machine_tv.object_link_correspondence` names the link edge's own
status (`checked_object_link_correspondence` or
`unchecked_not_translation_validated`) so `checked` is not misread as
covering it. Both
text paths print `Source machine-TV within profile: <status> ...` after
the summary. This is a report only — no check, grade or verdict is
promoted.

Scalar f64 leaves are admitted only under a record profile that admits f64
arithmetic. The bounded SSE2 profile handles register-only `movsd`, `movapd`,
`movaps`, `movq`, `addsd`, `subsd`, `mulsd`, `divsd`, and `cmpeqsd`; f64
parameters use the first eight XMM argument registers independently of the
integer argument registers, and f64 results use the low 64 bits of `xmm0`.
f64 results compare equal when their bit patterns match or both are NaN, so
NaN payloads are not claimed. Arithmetic models round to nearest, ties to even,
and implement the specified x86 NaN propagation. MXCSR RNE, masked exceptions,
and disabled FTZ/DAZ are assumptions, reported as
`machine_code_tv_mxcsr_control_state`; `machine_code_tv_nan_payload_bits` is
also not performed. When an equivalent function has an f64 leaf, including
inside a struct or a call shape,
`machine_code_f64_scalar_function_tv` is performed. Memory operands fail
closed. Register-only `ucomisd` and `comisd` compare the low 64-bit XMM
operands and model `ZF = unordered || fp.eq`, `CF = unordered || fp.lt`,
`PF = unordered`, with `OF = SF = AF = 0`. Their conditions are supported for
branches, `setcc`, and `cmovcc`; memory operands remain outside the profile.
Parity conditions after integer flag-producing instructions remain outside the
profile because integer PF is not modeled. Agent consumers must adopt the
three f64 check names.
Scalar f64 parameters and f64 leaves in admitted struct parameters and
direct-call arguments use `xmm0`–`xmm7`, independently of the integer
argument registers; overflow arguments of both classes continue in one stack
area laid out in declaration order. Call arguments first use NaN-aware f64
equality alongside exact integer equality and output matching. A second query
must prove that every machine f64 argument's low 64 bits exactly equal the
source argument bits under the same path assumptions.
This admits pass-through f64 calls without assuming that a callee ignores NaN
payloads; a computed f64 argument whose exact NaN bits cannot be proved, or an
f64 tail call, remains outside the profile as `call_arguments`. f64 call
arguments past `xmm7` spill onto the outgoing argument stack in declaration
order shared with integer overflow arguments; the caller must write each
spilled `f64` leaf with a `movsd %xmm, N(%rsp)` store, and the stored slot
carries the same NaN-aware and exact-bit obligations as a register argument.
f64 call
results remain supported and are bound from the low 64 bits of `xmm0`. When an
equivalent function has an f64 leaf, including inside a struct or a call shape,
`register_convention_assumed` names the XMM argument/result registers with
`sysv_x86_64_integer_args_rdi_rsi_rdx_rcx_r8_r9_f64_args_xmm0_7_result_rax_or_xmm0`;
otherwise the descriptor retains the integer-only convention string.

One- through sixteen-leaf struct parameters and direct-call values may
contain integer and/or f64 leaves — the admission fragment's own sixteen-leaf
flattening limit bounds the largest struct a source can declare. Each leaf
uses its own assumed register class: integer leaves consume the next GPR from
`rdi`–`r9`, and f64 leaves consume the next XMM register from `xmm0`–`xmm7`.
LLVM `expand` lowering flattens aggregates leaf-by-leaf into the scalar
argument sequence, so leaves without a free register in their class spill
onto the argument stack exactly like scalar stack arguments: integer
leaves past `r9`, and f64 leaves past `xmm7`. The stack slots hold the
overflow leaves of both classes in declaration order. Spilled f64 leaves
are read either through a `movsd` load (low half written, upper half
zeroed) or directly as an SSE memory operand
(`addsd`/`subsd`/`mulsd`/`divsd`/`ucomisd`/`comisd` with an `N(%rsp)`
source); an INTEGER instruction reading an f64 slot, or an SSE
instruction reading an integer slot, fails closed. Aggregates with
seventeen or more leaves fail closed (`argument_count` for function
parameters, `call_arguments` for direct-call arguments). The source admission fragment already
rejects struct declarations wider than sixteen leaves, so these checks
fire only on hand-written object inputs, not compiler output.

Register-resident aggregate results follow a per-class register
budget: at most three INTEGER leaves (`rax`, `rdx`, `rcx`) and at most
two SSE leaves (`xmm0`, `xmm1`) — a five-leaf `{i64,f64,i64,f64,i64}`
still returns in registers. The `llc-17 -O2` probe observed three
INTEGER leaves returned in `rax`, `rdx`, `rcx`, and a mixed
`{i64,f64,i64,f64}` result returned in `rax`, `xmm0`, `rdx`, `xmm1`.
The expected fourth INTEGER result register `r8` was not observed:
once INTEGER leaves overflow the budget the whole aggregate is
lowered through a hidden sret pointer and memory stores, `f64` leaves
landing as `movsd` stores. A result whose INTEGER leaves still fit
but carries three or more `f64` leaves reaches for the x87 stack
register for the third SSE leaf, which this model does not support;
those shapes fail closed (`unsupported_signature` for function
results, `call_arguments` for call results). Aggregate results of
four to sixteen leaves are admitted through the SysV `sret` convention on
the callee side: `rdi` carries the hidden
result-buffer pointer, INTEGER arguments shift one register right,
`movq`/`movsd` stores through a register still holding that pointer
write the buffer leaves, and `rax` must return the pointer. Unwritten
buffer slots compare
against fresh constants, so a dropped leaf store is `different`, and a
callee that does not return the pointer fails its own obligation. The caller side is
modeled too: a call whose result needs `sret` reads `rdi` as the
caller-allocated buffer pointer (declared INTEGER arguments begin at
`rsi`), recovers the buffer's frame offset from `rdi - rsp`, records
each result leaf in the frame slot map, and returns the pointer in
`rax`, so `N(%rsp)` reads and reads through a register still holding
the pointer both resolve to the same leaf slots, `movsd` reads and
writes of `f64` leaves included. This is LLVM
first-class aggregate lowering plus the
modeled sret store convention, not general SysV C-ABI aggregate
classification. Function-result f64 leaves use NaN-aware matching, integer
leaves compare exactly, and supported struct call results bind the leaves
into the corresponding registers after caller-saved clobbers.

F64 leaves in struct call arguments use the scalar f64 call obligations:
Query A compares NaN-aware values, and Query B must prove exact argument bits
under the same path assumptions. If that exact proof is satisfiable or
unknown, the function fails closed as `call_arguments`, not `different`.
Struct field writes, nested field writes and branch- or match-merged struct
values are admitted when their source values flatten to the modeled register
leaves. These are local value updates, not memory modeling; LLVM stack-memory
lowering remains outside the profile. Enum signatures and values, including
branch-built results, `match` projections, scalar equality and payload writes,
are admitted when their flattened tag and payload leaves fit the existing
register classes and aggregate limits, including the existing 3–4-leaf
first-class lowering preflights. Every modeled leaf is compared exactly;
LLVM's first-class aggregate lowering zero-initializes inactive payload slots.
This is an assumption about that LLVM lowering, not a validated ABI. Function-
signature and direct-call aggregates used as parameters or call arguments with
seventeen or more leaves, or as results outside the admitted shapes —
register-resident within the per-class budgets, or four-to-sixteen-leaf sret
— remain outside the profile. Enum call results share the
register-resident and `sret` paths struct call results use — the tag
leaf is always INTEGER and each payload leaf reports its own class, so
a four-to-sixteen-leaf enum result whose INTEGER leaves overflow comes
back through the caller's `sret` buffer (`f64` payload leaves as
`movsd` bit patterns), while results that still fit the per-class
register budgets stay register-resident. Enum results whose SSE leaves
overflow the two SSE slots keep reaching for the x87 register and stay
fail-closed.
Local struct values inside a body are not subject to this aggregate limit
because the source model flattens them. i128/u128 leaves inside aggregate
parameters, call arguments and aggregate results flatten into the same
per-slot sequence LLVM emits — each wide leaf is two consecutive INTEGER
slots, low half first, matching the source model's `[lo, hi]` slot order.
The two halves are independent scalars in the argument sequence: when
registers run out mid-pair LLVM keeps the low half in the last register
and spills only the high half onto the argument stack —
while bounded
memory, and tail calls with struct arguments or results also remain outside
the profile. i128/u128 leaves inside enum payloads flatten into the same
per-slot sequence — two consecutive INTEGER slots each, low half first — so a
wide-payload enum stays inside the aggregate leaf bounds. When an equivalent function uses a struct
aggregate, the descriptor performs `machine_code_composite_register_aggregate_tv`.
Aggregates wider than two leaves use the convention suffix
`_llvm_first_class_aggregates_split_into_registers_result_rax_rdx_rcx_xmm0_xmm1`
on the active integer or f64 `register_convention_assumed` string.
Aggregate uses that still stand for an assumed SysV memory-class ABI —
wide aggregate parameters and aggregate call arguments — add
`machine_code_tv_sysv_memory_class_aggregate_abi` to checks not performed;
a modeled register-resident or sret result the lifter validates, on
either side of the call, does not.
This is bounded evidence for the observed LLVM lowering, not SysV C-ABI
classification or general ABI validation.

### Object-stage machine-code acceptance (C10 Option B)

`mumei native-bundle accept-object` runs the same artifact, object-correspondence
and machine-TV pipeline as `recheck-machine-tv`, then emits an acceptance
descriptor only when at least one source-atom function is present, every
source-atom function defined in the reproduced object is
`equivalent_within_profile`, the applicable C `main` entry wrapper is
`equivalent_within_profile`, every remaining object function has the exact
`outside_profile` reason `not_a_source_atom`, the canonical link argument
vector was reproduced, and every `object_translation.correspondence` check item
(function coverage/count, the LLVM-IR↔object equivalence, the four relocation
record items, the HIR↔LLVM model correspondence, the object data and symbol
attribute correspondences) is `checked` (DESIGN_OBJECT_ACCEPTANCE.md §4). A
machine-code difference or an `inconsistent` correspondence item exits 1.
Unknown, any other outside-profile result, no source-atom functions, an
unchecked/unknown correspondence item, or a missing canonical argv pin exits 3
with no descriptor; identity or artifact mismatches exit 1 as well.

When the source `main` model has the supported i64-carrier signature and
`__mumei_user_main` is defined, `machine_code_entry_wrapper_tv` checks that the
wrapper makes one unconditional zero-argument call to `__mumei_user_main` and
returns the low 32 bits of its result. `argc` and `argv` remain fresh, unused
inputs. The source signature gate selects the LLVM truncation path rather than
the float `fptosi` path. This does not claim CRT or loader behavior, or OS
exit-status semantics.

On acceptance, `--json` emits the
`mumei.native-evidence-acceptance/v1` family with `acceptance: "evaluated"` and
`accepted: true`. It reports the expected build ID and SHA-256 digests of the
staged, reproduced `program.ll` and `program.o`, together with the machine-TV
function entries, input domain and dynamically composed checks. Its
`object_source_machine_code_correspondence` check means every source-atom
function in the reproduced `program.o` matches its verified source model within
the machine-code TV profile, under the assumed register convention and
input domain. The descriptor's `scope` is
`native_bundle_object_translation_correspondence`; if object-to-executable
link correspondence is uncheckable, it uses
`native_bundle_object_translation_correspondence_without_object_link` and
leaves `object_link_correspondence` not performed.

The descriptor's `evidence_fingerprint` is the lowercase, unprefixed SHA-256
hex digest of a compact UTF-8 JSON object with exactly these keys, in lexical
order: `build_id`, `canonical_argv_hash`, `emit_scope`, `llvm_sha256`,
`object_sha256`, `source_sha256`. All values are strings. Use the trusted
build ID, the rechecked record's `link_build.canonical_arguments_sha256`, the
emitted `scope` (including the suffix on the no-link path), and the SHA-256
digests of the reproduced LLVM IR, reproduced object and rechecked source,
respectively. For a multifile bundle, use the digest of the canonical merged
source (`source.source_sha256` in the record), not of the root file alone.
An independently derived matching pin binds the descriptor to the same attempt
inputs; it does not authenticate the descriptor or its producer.
`descriptor_authenticity` remains in `checks_not_performed`.

This is not reusable Evidence or a certificate and does not promote a grade.
The register convention is assumed, not validated; CRT and loader behavior,
producer authenticity, historical execution, optimizer semantics, ABI/runtime
semantics, OS exit-status observations, and grade/Lean checks remain outside
the claim. The accepted descriptor omits `object_entry_wrapper_semantics` only
when the bounded entry-wrapper check is equivalent.
Core deliberately does not emit the agent-owned
`object_translation_correspondence` name. The future mumei-agent
`native-object-accept` route maps Core's
`object_source_machine_code_correspondence` check to its
`object_translation_correspondence` check.

#### Detached object descriptor signing (partial C08)

Pass `--json --key-file KEY --producer-key-id LABEL --signature-output SIG`
to `native-bundle accept-object` to emit an optional detached
[`mumei.producer-signature/v1` envelope](#detached-producer-signature-envelope).
Only a successful pinned acceptance reaches signing. The Ed25519 signature
covers the SHA-256 digest of the exact descriptor bytes written to stdout,
including the trailing newline, for descriptors up to 4 MiB. Save stdout
as-is: reformatting the JSON invalidates the binding. The signature is staged
first and published without replacing an existing file, only after stdout is
written and flushed. `--produced-at RFC3339` optionally stamps that declared
timestamp into the envelope for a consumer-side freshness check. Leap seconds
(`23:59:60`) are rejected because the freshness model uses civil-time
nanosecond bounds rather than a leap-second table. Like the key label it is
unsigned metadata, and omitting it means no envelope freshness can be
established. If publication fails, the command exits unsuccessfully even
though stdout may contain the descriptor. Consume the signed result only after
the command succeeds and you have both outputs.

Verification recomputes the digest from the saved descriptor and verifies the
signature over the lowercase hex digest using an independently pinned public
key. The key label is unsigned. Neither signing nor verifying the byte binding
establishes identity or authority of the producer, freshness, or historical
execution. The descriptor and its attempt fingerprint are unchanged, including
`producer_authenticity` and `descriptor_authenticity` remaining not performed.
This adds neither consumer trust policy nor common Evidence, grade promotion,
or an `artifact_verified` claim.

### Saved object acceptance replay

`mumei native-bundle recheck-object-acceptance <bundle> <source> <descriptor> --expected-build-id <trusted-id>`
is the consumer side of `accept-object`. It checks a saved descriptor against a
**fresh object acceptance** of the same pinned inputs, never against the saved
`accepted: true` alone. For example:

```sh
mumei native-bundle accept-object bundle source.mm --expected-build-id <trusted-id> --json > object-acceptance.json
mumei native-bundle recheck-object-acceptance bundle source.mm object-acceptance.json --expected-build-id <trusted-id>
```

The expected build ID must come from a separately trusted channel, never from
the descriptor. The descriptor is read once as a regular, non-symlink file of
at most 1 MiB. Malformed or non-UTF-8 JSON and duplicate keys at any object
level are rejected. So is any envelope other than an evaluated, accepted
`mumei.native-evidence-acceptance/v1` descriptor with one of the two object
scopes and a lowercase SHA-256 `evidence_fingerprint`. That includes saved
`accept-evidence` descriptors and saved Evidence reports. Descriptor strings
never select tools or files to open.

The command then reruns the full `accept-object` pipeline and gates. Different
artifacts or machine code exit 1. An inconclusive fresh acceptance exits 3 and
establishes no consistency result. When the fresh acceptance succeeds, the
consumer recomputes `evidence_fingerprint` as specified above. A saved
fingerprint that differs is rejected as not bound to the fresh attempt inputs,
which covers a descriptor from another bundle or source. With the fingerprint
bound, the saved descriptor must equal the fresh descriptor field for field. A
missing, added or edited field anywhere is rejected, including in
`checks_performed`, `checks_not_performed`, `functions` and the object
translation record. Success prints the scope and exits 0. Nothing is written to
stdout on rejection. Flat two-field `{double,i64}` and `{i64,double}` return
pairs, which C09 admits through `xmm0` and `rax`, replay the same way. A saved
descriptor for one field order is rejected as unbound against the other.

The consumer can additionally pin a detached producer signature with the same
`--signature` / `--producer-key` chain flags used by Evidence acceptance. The
signature subject is the SHA-256 digest of the **exact saved descriptor bytes**,
not the descriptor's attempt-input `evidence_fingerprint`; even an insignificant
byte edit breaks the binding before the fresh acceptance runs. A malformed
envelope, wrong public key, failed key-label pin or failed explicitly supplied
freshness/registry/revocation gate rejects the replay. Omitting `--signature`
keeps the legacy consistency-only behavior. These gates check only the supplied
cryptographic chain: key labels and declared times remain claims unless the
caller pins and anchors them separately, and real-world producer identity,
historical execution, register convention, CRT/loader, ABI/runtime and OS
semantics remain outside the result.

A match means only that the saved descriptor is what a fresh `accept-object`
emits for these pinned inputs. Without a valid supplied signature chain it does
not check descriptor or producer authenticity; with one it checks only that
byte binding and the explicitly supplied key-label/freshness/registry/revocation
gates. It never checks historical execution, and it accepts no reusable
Evidence, certificate or grade. Every claim outside the `accept-object` scope
stays outside it here.

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
keeps `os_exit_status_observation` excluded since it performs no execution. A
mismatched status fails closed (exit 1); every unevaluable case exits 3
with no descriptor.

### Pinned executable stdout recheck

`mumei native-bundle recheck-stdout` is the adjacent channel-observation
slice. It runs the same `recheck-artifact` chain and then executes the
reproduced `program` once with no arguments and closed stdin/stderr,
capturing at most 64 KiB of stdout and comparing its SHA-256 against the
record's `link_build.expected_stdout_sha256` pin:

```sh
mumei native-bundle recheck-stdout module-bundle module.mm \
  --expected-build-id <trusted-64-character-lowercase-sha256>
```

The producer pins `expected_stdout_sha256` at build time under the same
no-argument/closed-stdin conditions with a 30-second cap; a silent
program pins the empty-stream digest. Because the consumer executes the
byte-identical reproduction — never the bundle's own copy — a matching
digest is an observation about the *pinned* artifact.

The outcome mapping stays honest about what cannot be evaluated: spawn
failure, timeout, signal termination, output over the capture cap and a
record that pins no `expected_stdout_sha256` (pre-field records carry
none) all exit 3 as inconclusive; a digest mismatch fails closed as
`Different` (exit 1) after the artifact reproduction itself succeeded.
The field is optional in the record so pre-field records still
deserialize, and a skipped comparison is exactly what inconclusive
reports.

The success scope is
`artifact_reproduction_and_os_stdout_observation_only/no_runtime_abi_or_semantic_proof`.
A single execution under one environment is an OS-level observation of
one channel — not a runtime, ABI, process-semantics, stderr or
historical-execution claim; this command observes stdout only and does
not compare the exit status.

`mumei native-bundle accept-stdout` is the acceptance face: it runs the
identical chain and, on a matching observation, emits the anonymous
acceptance descriptor with `os_stdout_observation` in `checks_performed`
and `os_exit_status_observation`, `os_stderr_observation`, `historical_execution`,
optimizer/ABI/runtime semantics, grade and Lean in
`checks_not_performed`. Every unevaluable case exits 3 with no
descriptor; a mismatched digest exits 1.

### Pinned executable stderr recheck

`mumei native-bundle recheck-stderr` completes the channel-observation
trio (exit status, stdout, stderr). It runs the same `recheck-artifact`
chain and then executes the reproduced `program` once with no arguments
and closed stdin/stdout, capturing at most 64 KiB of stderr and
comparing its SHA-256 against the record's
`link_build.expected_stderr_sha256` pin:

```sh
mumei native-bundle recheck-stderr module-bundle module.mm \
  --expected-build-id <trusted-64-character-lowercase-sha256>
```

The producer pins `expected_stderr_sha256` at build time under the same
no-argument/closed-stdin conditions with a 30-second cap; a program
silent on stderr pins the empty-stream digest. Because the consumer
executes the byte-identical reproduction — never the bundle's own copy
— a matching digest is an observation about the *pinned* artifact, and
a reproduced program that diverges only through stderr can no longer be
claimed as reproduced.

The outcome mapping stays honest about what cannot be evaluated: spawn
failure, timeout, signal termination, output over the capture cap and a
record that pins no `expected_stderr_sha256` (pre-field records carry
none) all exit 3 as inconclusive; a digest mismatch fails closed as
`Different` (exit 1) after the artifact reproduction itself succeeded.
The field is optional in the record so pre-field records still
deserialize, and a skipped comparison is exactly what inconclusive
reports.

The success scope is
`artifact_reproduction_and_os_stderr_observation_only/no_runtime_abi_or_semantic_proof`.
A single execution under one environment is an OS-level observation of
one channel — not a runtime, ABI, process-semantics, exit-status,
stdout or historical-execution claim; this command observes stderr only.

`mumei native-bundle accept-stderr` is the acceptance face: it runs the
identical chain and, on a matching observation, emits the anonymous
acceptance descriptor with `os_stderr_observation` in
`checks_performed` and `os_exit_status_observation`, `os_stdout_observation`,
`historical_execution`, optimizer/ABI/runtime semantics, grade and Lean
in `checks_not_performed`. Every unevaluable case exits 3 with no
descriptor; a mismatched digest exits 1.

### Pinned executable runtime-observation recheck

`mumei native-bundle recheck-observations` completes all three pinned
runtime observations in one invocation of the byte-identical executable
reproduced by `recheck-artifact`:

```sh
mumei native-bundle recheck-observations module-bundle module.mm \
  --expected-build-id <trusted-64-character-lowercase-sha256>
mumei native-bundle recheck-observations module-bundle module.mm \
  --expected-build-id <trusted-64-character-lowercase-sha256> --json
```

The observer closes stdin, supplies no arguments, and captures stdout and
stderr concurrently, with the same 30-second timeout and 64 KiB cap per
channel as the individual observation commands. It compares the resulting
OS exit status, stdout SHA-256 and stderr SHA-256 against all three
`link_build` pins. This is one invocation; the producer currently obtains
those pins with three separate build-time runs (`observed_exit_status`,
`observed_stdout` and `observed_stderr`). The combined recheck therefore
tests whether all three observations agree on the same fresh execution,
without changing how any pin is produced.

If any pin is absent, the reproduced executable is unavailable, or a
spawn, timeout, signal, reader or capture-cap failure prevents a complete
observation, the command exits 3 as inconclusive and emits no descriptor.
A complete observation that differs from any pin exits 1 and prints no
stdout. A match exits 0. Text mode reports the three matching channels;
`--json` emits `mumei.runtime-observation/v1` with verdict
`observed_matching`, the trusted build ID, the fixed invocation envelope,
all three observed values, the four checks performed and the six declared
checks not performed. This descriptor is not reusable Evidence or a
certificate.

The scope is
`artifact_reproduction_and_runtime_observation_semantics_on_one_invocation_only/no_runtime_abi_or_semantic_proof`.
It is a conjunction of three OS-level observations from one run, not a
runtime/ABI, process-semantics, optimizer-semantics or
historical-execution claim. Composition landed via `accept-observations`
and the mumei-agent `native-observations-accept` route (mumei-agent#1039).

Both JSON descriptors report the same invocation envelope:
`argv: []`, `stdin: "closed"`, `timeout_ms: 30000`,
`channel_cap_bytes: 65536`, `environment: "empty"` and
`working_directory: "executable_directory"`. Multiple invocations,
non-empty argv, stdin bytes and CPU/memory resource limits are not
supported or claimed; only the 30-second timeout and 64 KiB per-channel
cap are enforced, and none of those properties is covered by
`runtime_observation_semantics`.

Every native observation uses this spawn policy: build-time pins,
per-channel `recheck-*`/`accept-*`, and
`recheck-observations`/`accept-observations`. New records optionally pin
the context of their `expected_*` observations in
`link_build.observation_envelope`; it is absent when no observation pin
was obtained. Pins recorded under a different envelope are not compared,
so runtime rechecks are inconclusive (exit 3). Legacy records without the
field are still compared and runtime descriptors report
`pin_envelope: "unrecorded"`; matching recorded envelopes report
`pin_envelope: "recorded"`.
Per-channel `accept-exit-status`, `accept-stdout` and `accept-stderr` JSON
acceptance descriptors also carry `invocation`, `host` and `pin_envelope`.
As before, none of these properties is covered by
`runtime_observation_semantics`.

Both runtime-observation descriptors also carry `host` (`os`, `arch`,
`family`), the running consumer binary's compile-time target, recorded
as an observation of where the invocation ran and never as a claim. The
producer's host, libc/loader/kernel versions and CPU features are not
recorded or compared; because the record pins no host,
`runtime_observation_semantics` is scoped to "some host of that os/arch",
not to an attested environment.

### Pinned executable runtime-observation acceptance

`mumei native-bundle accept-observations` runs the same artifact
reproduction and one-invocation comparison as `recheck-observations`,
then emits an anonymous acceptance descriptor only when the reproduced
executable matches all three runtime-observation pins:

```sh
mumei native-bundle accept-observations module-bundle module.mm \
  --expected-build-id <trusted-64-character-lowercase-sha256>
mumei native-bundle accept-observations module-bundle module.mm \
  --expected-build-id <trusted-64-character-lowercase-sha256> --json
```

Exit 0 means the artifact chain and the one-run observation matched; exit
1 means the artifact or an observation differed; exit 3 means a pin or
observation was inconclusive. JSON emits
`mumei.native-evidence-acceptance/v1` with `accepted: true` and scope
`native_bundle_source_llvm_toolchain_reproduction_and_runtime_observation_semantics`
(multifile bundles use the corresponding
`native_multifile_bundle_...` scope). Its `checks_performed` includes
`os_exit_status_observation`, `os_stdout_observation`,
`os_stderr_observation` and `runtime_observation_semantics` after the
artifact-reproduction checks. `checks_not_performed` retains producer
authenticity, historical execution, raw argv reproduction,
optimizer/ABI/runtime semantics, grade promotion and Lean checks; the
canonical-argv check is also not performed when its pin is absent.

This is the only acceptance descriptor that performs
`runtime_observation_semantics`: each per-channel `accept-*` command runs
its channel observation separately, so those descriptors cannot claim
that all channels matched in the same invocation. The descriptor is not
reusable Evidence or a certificate. The Agent
`native-observations-accept` route (mumei-agent#1039) now performs
`runtime_observation_semantics` from this descriptor. The Agent decides
acceptance from Core's exit code and its own declared-input recheck; it
stores the JSON but does not validate the `invocation` envelope, which
is what Core reports about its own run, not a condition the Agent checks.
These descriptors name the exit-status observation only as
`os_exit_status_observation` (formerly bare `os_exit_status` in the
not-performed lists of the non-exit-status descriptors).

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
semantics, ABI/runtime semantics, OS exit status, OS stdout observation,
OS stderr observation, grade promotion and Lean kernel checks. Text mode
prints the same scope in prose.

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
OS exit status, OS stdout observation, OS stderr observation, grade and
Lean, and honestly names
`raw_link_argument_vector_reproduction`
— the raw `arguments_sha256` build observation no recheck can
reproduce, as `recheck-artifact` documents.
Because artifact acceptance evaluates no record input, the descriptor emits
no `evidence_fingerprint` key; the bundle identity pin is its binding.

Acceptance is byte reproduction under the recorded toolchain, not execution,
exit-status, ABI/runtime or optimizer-semantics proof. Non-acceptance paths
emit no stdout: difference or failure exits 1, an inconclusive fresh query
exits 3.

### Object-route acceptance descriptor contract and Common Evidence projection

```sh
mumei evidence check-object-acceptance <descriptor.json | --stdin> [--json | --emit-evidence]
```

The object-route `mumei.native-evidence-acceptance/v1` descriptor emitted by
`mumei native-bundle accept-object --json` now has a closed JSON Schema
(`schema/native-object-acceptance.schema.json`) and a strict Core decoder
(`mumei_core::object_acceptance::decode`). It validates required fields,
64-character lowercase SHA-256 digests (`build_id`, `llvm_sha256`,
`object_sha256`, `evidence_fingerprint`), disjointness between
`checks_performed` and `checks_not_performed`, Core-owned check names
(rejecting agent-owned `object_translation_correspondence`), scope consistency
with `source_machine_tv.object_link_correspondence` and transformation edges,
and coverage of all ten required `object_translation.correspondence.checks`
items.

With `--emit-evidence`, Core projects a validated descriptor into a bounded
`mumei.verification-evidence/v1` record (`method: "bounded_model_checking"`,
`scope.kind: "bounded"`, `semantics.integer_model: "wrapping_i64"`,
`provenance.configuration_fingerprint: evidence_fingerprint`) carrying every
`checks_not_performed` entry in `exclusions` and the assumed register
convention / input domain in `scope.bounds` and `assumptions`. This is
representation validation and projection only: it does not rerun Core
rechecks, authenticate the producer or descriptor, validate ABI/runtime/OS
semantics, or promote a Seal Grade.

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
