# Ensures Lemma Export: Trigger Design Specification

Status: **design only**. Nothing in this document is implemented yet. The
syntax in [§4.3](#43-opt-in-annotation-syntax-hypothetical) is a proposal and
the current parser rejects it. Every report key, diagnostic code, and
certificate field introduced here is a *proposed* addition. An implementation
PR must document the keys it actually ships in
[`REPORT_SCHEMA.md`](REPORT_SCHEMA.md),
[`LSP_DIAGNOSTIC_DATA.md`](LSP_DIAGNOSTIC_DATA.md), and
[`PROOF_CERTIFICATE.md`](PROOF_CERTIFICATE.md).

This is the trigger design the P33 roadmap names as the prerequisite for ensures
lemma export (see [`ROADMAP.md`](ROADMAP.md), "P33: 検証診断と信頼境界の明示化").
P33 Wave 2 deferred the feature for this reason: a caller can already assume a
callee's `ensures` at each call, but injecting the same fact automatically, as a
quantified fact, only has array `select` as a usable trigger today, and
arithmetic facts would depend on MBQI, which makes both run time and verdicts
unstable.

## Contents

1. [Background: what calls do today](#1-background-what-calls-do-today)
2. [Which ensures clauses are exportable](#2-which-ensures-clauses-are-exportable)
3. [The exported fact and when it is used](#3-the-exported-fact-and-when-it-is-used)
4. [Trigger selection](#4-trigger-selection)
5. [Solver configuration and determinism](#5-solver-configuration-and-determinism)
6. [Cache hashes and the byte-identity rule](#6-cache-hashes-and-the-byte-identity-rule)
7. [Interaction with `cover`](#7-interaction-with-cover)
8. [Interaction with Lean escalation and the axiom audit](#8-interaction-with-lean-escalation-and-the-axiom-audit)
9. [Fail-closed rules, diagnostics, and report keys](#9-fail-closed-rules-diagnostics-and-report-keys)
10. [Measurement plan](#10-measurement-plan)
11. [Worked examples](#11-worked-examples)
12. [Implementation checklist](#12-implementation-checklist)
13. [Open questions](#13-open-questions)

## Terminology

- **Exporting atom** (or *callee*): an atom with at least one `ensures` clause
  marked for export.
- **Importing atom** (or *caller*): an atom whose verification condition
  contains at least one call to an exporting atom.
- **Exported fact**: the quantified formula built from one exported clause
  ([§3.1](#31-form-of-an-exported-fact)).
- **Result function**: the uninterpreted function symbol that stands for the
  exporting atom's return value in the exported fact.
- **Trigger** (also *pattern*): a term, or a set of terms (a *multi-pattern*),
  attached to a quantifier. The solver instantiates the quantifier only for
  ground terms in the current context that match the trigger
  (*E-matching*).
- **Matching loop**: a set of quantified facts where instantiating one fact
  creates a new ground term that matches a trigger again, without bound.
- **Per-call instantiation**: today's call lowering, which asserts the callee's
  caller-visible `ensures` once per call site with the actual arguments
  substituted.
- **Under a binder**: inside the body of a `forall(...)` / `exists(...)`
  quantifier, where a call's arguments mention the bound variable.

## 1. Background: what calls do today

These facts describe `develop` as of this document and were checked against the
source and with the current `mumei verify`.

- **Contract views.** `mumei-core/src/verification/contract_view.rs` filters
  clauses by trust mode. `ContractView::CallerEnsures` drops `ensures check`,
  `ContractView::BodyEnsures` drops `ensures assume`,
  `ContractView::BodyRequires` drops `requires check`, and
  `ContractView::CallerRequires` drops `requires assume`. These match the table
  in [`LANGUAGE.md`](LANGUAGE.md) "Clause Trust Modes".
- **Per-call instantiation.** For each call `g(a1, ..., an)` the translator
  (`verification/translator/expr.rs`) checks that the caller-visible `requires`
  hold for the actual arguments, creates a fresh result constant named
  `call_<name>_<n>` from a process-wide counter, binds the parameters to the
  actual arguments, lowers the caller-visible `ensures`, and asserts them.
  Two calls with the same arguments get two unrelated result constants.
- **Quantifiers.** `forall(i, lo, hi, body)` lowers to a solver quantifier over
  an integer bound variable. Array reads `arr[e]` whose index mentions the bound
  variable become E-matching patterns. Terms containing `if`/ITE are rejected as
  triggers (`is_admissible_trigger` in `verification/translator/z3_types.rs`).
  If no pattern is found the quantifier is asserted without an explicit pattern.
- **Solver settings.** `configure_array_quantifier_params` in
  `verification/executor.rs` sets `smt.mbqi = true` for atoms with array
  quantifiers. `compute_solver_config_fingerprint` hashes
  `z3.timeout_ms`, `smt.mbqi`, `string_constraints`, `array_forall`, and
  `spurious_detection`.
- **Proof hash.** `compute_proof_hash` in `mumei-core/src/resolver/cache.rs`
  covers the atom's contract and body, covers, clause modes, parameter modes,
  and the transitive callees' contracts, clause modes, signatures, and
  semantics. `VERIFIER_POLICY_VERSION` is `3`.

Three current behaviours motivate this design. They are reproduced in
[§11](#11-worked-examples).

1. A call under a binder does not receive its callee's `ensures` in a usable
   form, so a postcondition such as `forall(i, 0, n, abs_val(arr[i]) >= 0)`
   fails even though `abs_val` guarantees `result >= 0`
   ([Example C](#example-c-call-under-a-binder-in-ensures)).
2. Two calls with identical arguments are not known to return the same value,
   because each gets its own fresh constant
   ([Example D](#example-d-congruence-between-two-equal-calls)).
3. A call under a binder in `requires` is currently over-approximated in the
   unsafe direction: the verifier proves `arr[0] == arr[1]` from a `requires`
   that only says `forall(i, 0, n, ident(arr[i]) == arr[i])`
   ([Example E](#example-e-pre-existing-unsoundness-for-calls-under-a-binder-in-requires)).
   This is a pre-existing soundness bug. Its likely cause is that the fresh
   result constant is created once, outside the binder, so every instance of
   the quantifier shares one value. Fixing it changes solver input for atoms
   that do not use lemma export, so it is fixed in a separate PR,
   [#672](https://github.com/mumei-lang/mumei/pull/672), which makes such calls
   fail closed as `unverifiable`. The implementation of this spec depends on
   that fix (see [§13](#13-open-questions)). The export path specified here must not
   reproduce it: an exported call under a binder is always a result-function
   application, never a hoisted constant.

## 2. Which ensures clauses are exportable

Export is **opt-in per clause** ([§4.3](#43-opt-in-annotation-syntax-hypothetical)).
An atom without any export annotation is never an exporting atom, and nothing in
this specification changes how it or its callers are verified.

### 2.1 Clause trust mode

| Clause | Exportable | Trust of the exported fact |
|---|---|---|
| `ensures: e;` | Yes | `proved` |
| `ensures check: e;` | **Never** | — |
| `ensures assume: e;` | Yes, as a recorded trust boundary | `assumed` |

- **`ensures check`** is hidden from callers by definition (it is not part of
  `CallerEnsures`). Exporting it would leak it to callers. Writing an export
  annotation on an `ensures check` clause is an input error
  (`lemma_export_hidden_clause`, exit code `4`), not a silent no-op, so a
  specification that contradicts itself is never accepted.
- **`ensures assume`** is already caller-visible and already appears in the
  exporting atom's certificate as an `assumed_clauses` entry and in the trust
  surface as `TrustBoundaryKind::AssumedClause`. Its exported fact keeps
  `trust: "assumed"` everywhere it is recorded: the exporter's
  `lemma_exports`, every importer's `lemma_imports`, the importer's
  certificate, and any `cover` result that ran with it in context. An assumed
  fact must never be recorded or rendered as `proved`.
- **Plain `ensures`** is exported with `trust: "proved"`, subject to the
  eligibility rules below.

### 2.2 Eligibility of the exporting atom

The exported fact claims something about **every** argument tuple that satisfies
the precondition, including tuples that only appear in specifications and are
never executed. Today's per-call instantiation only makes a claim about calls
that are executed (partial correctness). The quantified form is therefore
stricter. A clause is exported only when all of these hold; otherwise it is
rejected with the listed reason ([§9.2](#92-rejection-reasons)):

| # | Condition | Rejection reason |
|---|---|---|
| E1 | The clause mode is plain or `assume`. | `hidden_clause` (input error) |
| E2 | The exporting atom's own verification succeeded in this run or came from a valid cache entry, and the clause's `ensures_outcomes` entry is `proved` (plain) or `assumed` (`assume`). `vacuous`, `always_false`, `fails_on_some_inputs`, `fails`, `unknown`, and `skipped` all block export. | `clause_not_proved` |
| E3 | The atom is not `trusted` / `unverified`, is not `extern`, and has no declared effects, no `ref` / `ref mut` / `consume` parameters, and is not `async`. The result must be a function of the parameters alone. | `impure_or_trusted` |
| E4 | The body is known to terminate: no loops, or every loop has a checked `decreases` measure, and no recursion (direct or mutual). The rule is transitive: every atom the body calls must satisfy E3 and E4 as well, so a postcondition that relies on a callee's partial-correctness `ensures` is never exported. Recursive atoms are not exportable until mumei checks a recursion measure. | `termination_not_established` |
| E5 | Every parameter lowers to a sort the result function can take: `Int`, `Real`, `Bool`, the bit-vector sorts, or a one-dimensional array of those. Array parameters are passed together with their length ([§3.1](#31-form-of-an-exported-fact)). The return type is one of the scalar sorts; array returns, tuple returns, structs, enums, strings, `atom_ref` parameters, and generic parameters are not exportable in the first version. | `unsupported_signature` |
| E6 | The clause and the antecedent ([§3.1](#31-form-of-an-exported-fact)) lower without the call-site fallbacks (no `ClauseLoweringOutcome` other than `Lowered`). Every call inside them is itself to an exporting atom, so it can be lowered as a result-function application. | `unlowerable` / `unexported_dependency` |
| E7 | The antecedent contains no quantifier. Quantifiers in the clause itself are allowed only as `forall` with an admissible array `select` pattern, which is what today's lowering already produces. | `quantified_antecedent` / `unsupported_quantifier` |
| E8 | A safe trigger exists ([§4](#4-trigger-selection)). | `no_safe_trigger`, `arithmetic_trigger`, `trigger_incomplete`, `matching_loop` |

E4 is what keeps `ensures` like `x == 0` on a non-terminating body (see
[Example F](#example-f-why-termination-is-required)) from becoming the
inconsistent fact `forall x. x == 0`. Without termination, the exported fact
can be false for some inputs, and a false quantified fact makes every importing
atom vacuously provable.

Apart from E1, which is an input error, rejection never changes the exporting
atom's own verdict. A rejected clause is still proved or assumed exactly as
today; it just is not exported.

## 3. The exported fact and when it is used

### 3.1 Form of an exported fact

For an exporting atom `g(p1: T1, ..., pn: Tn) -> R` and an exported clause `E`,
the exported fact is:

```text
forall p1: S1, ..., pn: Sn.
    { trigger_1 } ... { trigger_k }
    BodyRequires_g(p1, ..., pn)  =>  E[result := lemma_fn_g(p1, ..., pn)]
```

- `Si` is the solver sort of `Ti`, and `lemma_fn_g : S1 x ... x Sn -> S_R` is
  the result function. `lemma_fn_g` is this document's shorthand; the actual
  solver symbol is given in the last bullet of this list.
- The antecedent is the conjunction of the `ContractView::BodyRequires` clauses
  (plain `requires` and `requires assume`). That is the context in which the
  clause was proved against the body, so the implication is sound. Using
  `CallerRequires` instead would be unsound when a `requires assume` clause
  exists, because the clause may depend on it. A `requires check` clause is not
  in the antecedent; the clause was proved without it, so leaving it out only
  makes the fact stronger and is still sound.
- Refinement-type constraints on parameters (`x: Nat`) are part of the
  antecedent in the same way they are part of the body context today.
- **One fact per exported clause.** Clauses are not conjoined, so each fact
  keeps its own trust, label, and trigger, and a rejected clause does not block
  its siblings.
- **Array parameters carry their length.** The translator models an array's
  length as a separate symbol (`len_<name>`), not as part of the array sort.
  That symbol is in the sort of the active `i64` encoding: `Int` normally and a
  64-bit bit-vector when bit-vector mode is on. Quantifying over the array alone would leave `len(arr)` free, and a fact
  such as `result == len(arr)` could then equate the lengths of unrelated
  arrays. So each array parameter `a` contributes two bound variables, the
  array and `len_a`, both are arguments of the result function, and the
  antecedent includes `len_a >= 0` (a signed comparison in bit-vector mode).
  The bound length variable, the result-function signature, and the trigger
  all use the same length sort the translator uses for that context. At a call site the length argument is the
  caller's length term for the actual argument. If the caller has no length
  term for it, the call is not lowered through the result function (§5.2).
- If `n = 0`, there is nothing to quantify. The fact is the ground formula
  `BodyRequires_g() => E[result := lemma_fn_g()]` and needs no trigger.
- The result function name is deterministic and has no counter. It is the
  SMT-LIB quoted symbol `|lemma_fn#<resolved atom name>|`, where the resolved
  name is the one used for callee lookup (module-qualified for imported
  atoms). `#` cannot occur in a mumei identifier or path, so the name cannot
  collide with user symbols or with `call_<name>_<n>`.

### 3.2 When the exported fact is used instead of per-call instantiation

Per-call instantiation stays the default. The rules below apply only to calls
whose callee has at least one exported clause after eligibility checks. Calls to
non-exporting atoms are lowered exactly as today, in every position.

**R1. Ground calls (no bound variable in the arguments).** The call is lowered
as today (caller-visible `requires` check, then every caller-visible `ensures`
asserted as a ground instance) with one change: the result term is
`lemma_fn_g(a1, ..., an)` instead of a fresh `call_<name>_<n>` constant. Ground
instances are still asserted, so ground reasoning never depends on E-matching.
The change gives congruence for free: two calls with equal arguments have equal
results ([Example D](#example-d-congruence-between-two-equal-calls)).
Non-exported caller-visible clauses (for example a plain `ensures` that was
rejected for export) are still asserted as ground instances here. That is the
same fact the caller assumes today, so this is not a new trust assumption.

**R2. Calls under a binder.** A call whose arguments mention a bound variable
cannot be instantiated per call. For an exporting callee it is lowered as the
result-function application `lemma_fn_g(a1, ..., an)` inside the quantifier
body, and the solver obtains the callee's facts only through the exported
quantified facts. There is no call-site `requires` obligation for such a call:
calls in specification position are not executed, and the exported fact is
guarded by the callee's `requires`, so it says nothing about arguments outside
the precondition. This is the case the quantified fact exists for, and it is
the only case where it *replaces* per-call instantiation rather than adding to
it.

A call under a binder to a **non-exporting** callee is lowered exactly as
today. That path has the pre-existing problem described in
[§1](#1-background-what-calls-do-today); fixing it is out of scope here.

**R3. When the quantified fact is asserted.** For an importing atom, collect the
set `X` of exporting callees `g` such that `lemma_fn_g` occurs under a binder
anywhere in the atom's verification condition (requires, ensures, body,
loop invariants, covers), plus the closure of `X` under the dependency edges of
[§4.4](#44-matching-loop-check). The exported facts of exactly those callees are
asserted, once each, at the start of the solver context, before the body. If
`X` is empty, no quantified fact is asserted: ground instances from R1 already
contain everything an instantiation could add for ground terms.

R3 keeps quantifiers out of every context that does not need them. An importer
that only calls an exporting callee on ground arguments gets the R1 renaming and
nothing else.

**R4. Atoms that do not opt in.** An atom uses the feature if it has an export
annotation or if its transitive callee set (the same set `compute_proof_hash`
walks) contains an exporting clause. Atoms that do not use the feature keep the
same solver input, the same proof hash, and the same report output, byte for
byte ([§6](#6-cache-hashes-and-the-byte-identity-rule)). An exporting atom's own
verification condition does not change either: exporting only adds report and
certificate metadata.

## 4. Trigger selection

### 4.1 Admissible trigger terms

A term `t` is an **admissible trigger term** for an exported fact when all of
these hold:

- **T1. Uninterpreted head.** `t` is an application of a result function
  (`lemma_fn_h(...)` for an exporting `h`, including `g` itself) or an array
  read `select(a, i)`. Arithmetic (`+`, `-`, `*`, `/`, `%`), comparisons,
  boolean connectives, equality, `if` / ITE, casts, and bit-vector operators are
  never trigger heads.
- **T2. Arguments.** Each argument is a bound variable, a literal, or itself
  an admissible trigger term. An argument such as `x + 1` disqualifies `t`. This is the rule that
  rejects arithmetic-only triggers and the most common source of matching
  loops.
- **T3. No ITE anywhere in `t`.** This is the existing `is_admissible_trigger`
  rule, applied unchanged.
- **T4. No quantifier inside `t`.**

A **trigger** for the fact is a set of admissible trigger terms (a multi-pattern)
whose combined free variables are exactly the fact's bound variables
`p1, ..., pn`. A fact may have several alternative triggers; the solver may
instantiate on any one of them.

### 4.2 Automatic rule

If the clause has no explicit `trigger(...)` annotation, the trigger is chosen
automatically and there is exactly one:

```text
{ lemma_fn_g(p1, ..., pn) }
```

It always satisfies T1–T4 and covers every bound variable, because the
arguments are the bound variables themselves. It fires exactly when the
importer's context contains a call to `g`, ground or under a binder, which
matches the intent "a caller that mentions `g` knows what `g` guarantees".

The automatic rule deliberately does not mine other terms from the clause. A
clause such as `a * a >= 0` that does not mention `result` still gets the
self-application trigger. It will only fire where the importer mentions
`g(...)`, which is the same reach as today's explicit lemma-atom call
([Example G](#example-g-arithmetic-only-lemma-atom)). Supplying arithmetic
facts *without* a call is exactly the MBQI-dependent behaviour the roadmap ruled
out, and this design keeps it out.

The automatic trigger is still subject to the matching-loop check
([§4.4](#44-matching-loop-check)), and to E5 (a parameter that cannot be a bound
variable of a supported sort makes the fact unexportable).

### 4.3 Opt-in annotation syntax (hypothetical)

**This syntax does not exist yet.** The current parser rejects `export` with
`unknown clause trust mode 'export' ... expected assume or check`
([Example H](#example-h-the-proposed-syntax-is-rejected-today)).

The annotation goes after the optional trust mode and before the optional
label, so it composes with both existing features:

```mumei
atom abs_val(x: i64)
    requires: true;
    ensures export "nonneg": result >= 0;
    ensures export trigger(abs_val(x)) "exact": result == x || result == 0 - x;
    body: if x >= 0 { x } else { 0 - x };
```

Grammar (EBNF; `label` and `expr` are the existing productions from
`parser/item.rs`; `export` and `trigger` are contextual identifiers, parsed the
same way `assume`, `check`, and `cover` are today, so no new keywords are
reserved):

```ebnf
ensures_clause  = "ensures" [ trust_mode ] [ export_spec ] [ label ] ":" expr ";" ;
trust_mode      = "assume" | "check" ;
export_spec     = "export" { trigger_group } ;
trigger_group   = "trigger" "(" trigger_term { "," trigger_term } ")" ;
trigger_term    = call_term | index_term ;
call_term       = ident "(" [ trigger_arg { "," trigger_arg } ] ")" ;
index_term      = ident "[" trigger_arg "]" ;
trigger_arg     = ident | int_literal | bool_literal | call_term | index_term ;
```

Semantics:

- `ensures export` with no `trigger_group` uses the automatic rule.
- Each `trigger_group` is one alternative trigger; the terms inside one group
  form a multi-pattern. Groups are kept in source order.
- A `call_term` names an atom. The atom being declared stands for its own result
  function; any other atom must be an exporting atom, and stands for its result
  function. Calls to non-exporting atoms or to non-atom functions are rejected
  (`arithmetic_trigger`, because they have no uninterpreted head we can use).
- `trigger_arg` identifiers must be parameters of the atom. `result` is not
  allowed inside a trigger; write the atom's own call instead.
- `trigger_arg` grammatically excludes arithmetic, so `trigger(f(x + 1))` and
  `trigger(x * y)` are parse errors in the new production. The checker still
  applies T1–T4 after name resolution.
- Every group must cover every parameter (`trigger_incomplete` otherwise).
- If any group is rejected, the whole clause is rejected rather than exported
  with the remaining groups. A user who wrote a trigger meant that trigger, and
  silently dropping it would change instantiation behaviour without notice.
- `ensures check export ...` is an input error (`lemma_export_hidden_clause`).
- `requires`, `cover`, and loop invariants do not accept `export`.

The annotation is per clause on purpose: a per-atom switch would export clauses
whose triggers the author never looked at.

### 4.4 Matching-loop check

The check is syntactic and conservative. It runs over the set of exported facts
that would be asserted together in one importer (R3), but the per-exporter part
can be computed once when the exporting atom is checked.

1. For each exported fact `F` with triggers `T_F`, let `B_F` be its lowered
   antecedent and clause after substitution. Collect the set `A_F` of
   result-function applications and array reads in `B_F`.
2. Discard from `A_F` every term that is syntactically identical to a term of
   some trigger in `T_F`. Instantiating `F` on such a term creates no new term.
3. Add an edge `F -> G` for every remaining term in `A_F` whose head
   (result function or array) is the head of a term in some trigger of `G`.
   Self-edges `F -> F` count.
4. Reject every fact that lies on a cycle (`matching_loop`), and every fact
   with an edge to a rejected fact. Acyclic chains are allowed; their
   instantiation depth is bounded by the longest path, which the report records
   as `max_instantiation_depth`.

Examples:

- `ensures: result >= 0` on `abs_val`: `A_F` is `{abs_val(x)}`, which is the
  trigger itself. No edge, exported.
- `ensures: n == 0 || result == n + tri(n - 1)` on `tri`: `A_F` contains
  `tri(n - 1)`, which matches the trigger head `tri`. Self-edge, rejected. (It
  would also fail T2 if written as a user trigger, and E4 because `tri` is
  recursive.)
- A fact whose clause reads `arr[i + 1]` with a user trigger `arr[i]`:
  `select(arr, i + 1)` is not the trigger term but has the trigger head, so it
  is a self-edge and is rejected.

The check is deliberately stronger than necessary. Some rejected facts would not
actually loop in the solver, but a false rejection only costs completeness,
while a missed loop costs time-outs that look like `unknown` verdicts across
unrelated atoms.

### 4.5 When no safe trigger exists

If E8 fails, the clause is not exported and a warning-level diagnostic says why
([§9](#9-fail-closed-rules-diagnostics-and-report-keys)). Importers verify
without the fact: ground calls still get per-call instantiation, and calls under
a binder fall back to today's lowering only if the callee has no exported clause
at all. If the callee has *some* exported clauses but not this one, calls under
a binder see only the exported ones, and the missing clause is simply not known
inside quantifiers. An importer that needed it fails or is `unknown`; it never
passes because of the rejection.

## 5. Solver configuration and determinism

### 5.1 Settings

When R3 asserts at least one quantified exported fact in a solver context, that
context is configured as follows. Contexts with no exported fact use today's
settings unchanged.

| Parameter | Value | Why |
|---|---|---|
| `smt.mbqi` | `false` | Pure E-matching. Instantiation is driven only by the triggers of §4, so verdicts do not depend on model search. This also applies when the atom has array quantifiers, which today run with `smt.mbqi = true`. |
| `smt.qi.max_instances` | `10000` (constant `LEMMA_EXPORT_MAX_INSTANCES`) | A hard cap on instantiations, as a second line of defence against loops §4.4 did not catch. Hitting it produces `unknown`. |
| `smt.qi.eager_threshold` | unchanged (default `10.0`) | Changing it would make results depend on another tuning knob. |
| `smt.random_seed` | unchanged (current behaviour) | Determinism is already provided by the existing fixed defaults. |
| Patterns | explicit on every exported fact | An exported fact is never asserted without a pattern; that would leave instantiation to the solver's own pattern inference. |

Turning MBQI off can make an atom that also uses array quantifiers weaker than
it would be with MBQI on. That is acceptable because the atom opted in (directly
or through a callee) and the outcome is `unknown` or a failure, never a false
proof. The measurement plan ([§10](#10-measurement-plan)) checks how often this
happens. If it happens often, the fallback is an explicit mode, not a silent
switch back to MBQI (see [§13](#13-open-questions)).

### 5.2 `unknown` and resource limits

- `unknown` from any phase, including `canceled`, time-out, or the
  `smt.qi.max_instances` cap, stays `unknown` (exit code `3` if it is the
  overall verdict). The report adds `lemma_export.unknown_reason` with the
  solver's `reason-unknown` text when exported facts were in context, so a user
  can tell instantiation trouble from ordinary arithmetic incompleteness.
- An exported fact that fails to lower in an importer (for example because the
  importer's semantics flags differ from the exporter's, see
  `callee_semantics_match_caller`) is not asserted, and the call sites that
  needed it are lowered as R1 ground calls where possible. A call under a binder
  that can then not be lowered makes the atom `unknown`, with a
  `lemma_import_unlowerable` diagnostic ([§9.2](#92-rejection-reasons)); it is
  never treated as true.

### 5.3 Determinism

- Result-function names are counter-free ([§3.1](#31-form-of-an-exported-fact)).
- Exported facts are asserted in a canonical order: by resolved callee name, then
  by the clause's index in the callee's `ensures` list.
- Triggers keep source order (annotation) or are the single automatic trigger.
- Bound variables are named after the callee's parameters, prefixed with
  `lemma_bv#`, so names never depend on the importer.
- The cap and solver parameters are constants, and they are part of the solver
  fingerprint ([§6.2](#62-solver-configuration-fingerprint)).

Existing call result constants (`call_<name>_<n>`) still use a process-wide
counter for non-exporting callees. That does not affect cache hashes (they hash
source, not solver terms) and is out of scope.

## 6. Cache hashes and the byte-identity rule

### 6.1 Proof hash

`VERIFIER_POLICY_VERSION` is **not** bumped: bumping it would change every hash,
including those of atoms that never use the feature. Instead,
`compute_proof_hash` gains two sections that are appended only when non-empty,
which is the same pattern used for clause modes and covers:

- **Exporting atom.** If the atom has at least one export annotation, append
  `lemma_export_v1:` followed by, for each annotated clause in source order, the
  clause index, label, trust mode, and the canonical text of each trigger group
  (or `auto`). The atom's own verdict does not depend on this, but its report
  and certificate do, so its hash must change when an annotation changes.
- **Importing atom.** If the transitive callee set contains an exporting clause,
  append `lemma_import_v1:` followed by the canonical list of
  `(callee, clause index, trust, triggers, status)` for every annotated clause
  in the transitive callee set, plus the constant `LEMMA_EXPORT_MAX_INSTANCES`.
  `status` is `exported` or the rejection reason. It is included because E2
  makes the export set depend on the exporter's verdict: if an exporter's clause
  goes from `proved` to `unknown`, its importers must not reuse a cached result
  that relied on it. The
  callees' contract text is already in the transitive section, so a change to an
  exported clause body already changes the importer's hash.

An atom with neither section produces the same bytes as today, and therefore the
same hash.

### 6.2 Solver configuration fingerprint

`compute_solver_config_fingerprint` keeps its current payload. When exported
facts are asserted, it appends `;lemma_export=v1;qi.max_instances=10000` and
the effective `smt.mbqi=false`. The append only happens when the count is
non-zero, so existing fingerprints are unchanged.

### 6.3 What "byte-identical" is checked against

The implementation PR must add a regression test that, for every atom in
`std/` and `tests/` that does not use the feature in the sense of R4 (no export
annotation on the atom and none in its transitive callee set; today that is
every atom):

1. the SMT-LIB text of each solver context (`Solver::to_string()` captured by a
   test-only hook) is identical with the feature compiled in and with it
   disabled;
2. the proof hash is identical to the value computed on the base commit;
3. `report.json` is identical apart from timing fields.

Item 2 can reuse `scripts/std_proof_baseline.json` and the `stdlib-proof-gate`
workflow for `std/`.

## 7. Interaction with `cover`

`cover` stays a reachability query: is there an execution satisfying `requires`
that reaches a state where the cover expression holds? Exported facts do not
change what is asked, only what is known about callee results.

- **Same context.** The cover phase runs in the same solver context as the
  final check, so the exported facts asserted by R3 and the ground instances of
  R1 are present, under the same §5 settings. Leaving them out would let the
  solver pick callee results that the callee can never return, and report a
  witness that is not a real execution.
- **`sat` → `covered`.** The witness format is unchanged. When exported facts
  were in context, each cover result gains
  `relies_on_lemmas: [{callee, clause, trust}]`, listed in the canonical order of
  §5.3. The key is omitted when empty.
- **`unsat` → unreachable.** This remains a failure (exit code `1`). If any fact
  in context has `trust: "assumed"`, the failure reason says that
  unreachability depends on assumed facts and names them, because a wrong
  assumption can make a reachable cover look unreachable.
- **`unknown` → `unknown`.** E-matching without MBQI is incomplete for
  satisfiable queries with quantifiers, so cover checks are where `unknown` is
  most likely to appear. A cover is never promoted to `covered` or treated as
  unreachable on `unknown`.
- Exported facts are never derived from `cover` clauses, and `cover` clauses do
  not accept the `export` annotation.

## 8. Interaction with Lean escalation and the axiom audit

The Lean side must never accept a result that depends on an exported fact
without that dependency being visible and audited.

- **Hypotheses, never axioms.** When an importing atom is escalated to Lean,
  each exported fact it used is emitted as an explicit hypothesis of the
  generated theorem (a binder such as `(h_abs_val_nonneg : ∀ x, ...)`), never as
  a Lean `axiom` declaration. The kernel axiom audit described in
  [`PROOF_CERTIFICATE.md`](PROOF_CERTIFICATE.md) is therefore unchanged: only
  `propext`, `Classical.choice`, and `Quot.sound` are accepted, and any other
  axiom still yields `axiom_rejected`. A translator that cannot express a fact
  as a hypothesis does not escalate the atom; the status stays as it was
  (`unknown`), never `lean_verified`.
- **Discharging the hypotheses.** A `lean_verified` importer certificate lists
  its hypotheses in `lemma_imports` ([§9.3](#93-report-and-certificate-fields)).
  `verify-cert` accepts it only if every listed fact refers to an exporter entry
  in the same certificate set with a matching clause hash and an accepted status:
  `proven` or `lean_verified` for `trust: "proved"`. A fact with
  `trust: "assumed"` is never discharged; the importer's trust surface includes
  it, as it already includes the exporter's `AssumedClause`.
- **Exporters.** Exporting does not require the exporting atom to be Lean
  verified. Its exported fact carries the exporter's own status, and the rules
  above decide whether an importer's Lean result can be accepted.
- **Rejected or missing audit.** If the importer's Lean metadata is missing,
  stale, or `axiom_rejected`, or an exporter it depends on is in that state, the
  importer is not promoted. Existing `lean_escalation.status` values are reused;
  no new status is introduced.

## 9. Fail-closed rules, diagnostics, and report keys

### 9.1 Rules

1. A rejected export never changes the exporting atom's verdict (except for
   the `lemma_export_hidden_clause` input error) and never strengthens an
   importer's verdict.
2. `unknown` from any phase stays `unknown`. No exported fact is ever used to
   turn `unknown` into `proved`, `covered`, or unreachable.
3. A clause that cannot be lowered for export is not exported; a call site that
   then cannot be lowered is `unknown`, never assumed to hold.
4. An assumed exported fact is reported as assumed in every output that mentions
   it.
5. A matching loop, a missing trigger, or an arithmetic-only trigger is a
   rejection, never a fallback to an unpatterned quantifier or to MBQI.
6. Exit codes stay as documented in [`CLI.md`](CLI.md) "Exit codes". The only
   new error is `lemma_export_hidden_clause`, which is an input error (`4`).
   Every other rejection is a warning and does not change the exit code by
   itself.

### 9.2 Rejection reasons

All rejections except `hidden_clause` are warnings attached to the clause's span.
The diagnostic `code` is `lemma_export_rejected` and the reason is in
`data.lemma_export.reason`. The proposed reasons are:

| Reason | Meaning |
|---|---|
| `hidden_clause` | Export annotation on `ensures check` (error, code `lemma_export_hidden_clause`). |
| `clause_not_proved` | The clause outcome is not `proved` / `assumed` (E2). |
| `impure_or_trusted` | Trusted, unverified, extern, effectful, `ref` / `consume` parameter, or async (E3). |
| `termination_not_established` | Recursive, or a loop without a checked `decreases` (E4). |
| `unsupported_signature` | A parameter or return sort cannot be a bound variable or result function (E5). |
| `unlowerable` | The clause or antecedent needs a lowering fallback (E6). |
| `unexported_dependency` | The clause calls a non-exporting atom (E6). |
| `quantified_antecedent` | The `requires` contain a quantifier (E7). |
| `unsupported_quantifier` | The clause contains `exists`, or a `forall` without an admissible pattern (E7). |
| `no_safe_trigger` | No candidate trigger survived T1–T4. |
| `arithmetic_trigger` | A user trigger has an interpreted head or arithmetic argument. |
| `trigger_incomplete` | A user trigger group does not cover every parameter. |
| `matching_loop` | The fact lies on, or depends on, a cycle (§4.4). |

Example message: `ensures #1 of 'tri' is not exported: matching loop (tri -> tri via tri(n - 1))`.

One more code is reported on the importer side, not the exporter: a warning
`lemma_import_unlowerable` when an exported fact cannot be lowered in this
importer (for example a semantics mismatch, §5.2). The affected atom's verdict
is `unknown` unless it fails for another reason first.

### 9.3 Report and certificate fields

All proposed fields are omitted when empty, so reports of atoms that do not use
the feature are unchanged.

`report.json` of an **exporting** atom (the two entries come from `abs_val`
and `tri`; they are shown in one list for brevity):

```json
"lemma_exports": [
  {
    "clause": "result >= 0",
    "label": "nonneg",
    "trust": "proved",
    "status": "exported",
    "triggers": [["abs_val(x)"]],
    "trigger_source": "auto",
    "max_instantiation_depth": 1
  },
  {
    "clause": "n == 0 || result == n + tri(n - 1)",
    "label": null,
    "trust": "proved",
    "status": "rejected",
    "reason": "matching_loop",
    "detail": "tri -> tri via tri(n - 1)"
  }
]
```

`report.json` of an **importing** atom:

```json
"lemma_imports": [
  { "callee": "abs_val", "clause": "result >= 0", "label": "nonneg",
    "trust": "proved", "asserted": "quantified" }
],
"lemma_export": {
  "solver": { "smt.mbqi": false, "qi.max_instances": 10000 },
  "quant_instantiations": 3,
  "unknown_reason": null
}
```

`asserted` is `"quantified"` when R3 asserted the fact and `"ground"` when only
R1 ground instances were used. `quant_instantiations` comes from the solver's
statistics and is informational: it is excluded from the proof hash and from
any comparison that checks determinism.

Proof certificate (`AtomCertificate`): `lemma_exports` (same shape as the report,
without the statistics) on exporters, and `lemma_imports` with an added
`clause_hash` on importers. Both use `skip_serializing_if = "Vec::is_empty"`.

LSP (`mumei-z3` source): `data.lemma_export.reason`, `data.lemma_export.callee`,
`data.lemma_export.trust`, and `data.lemma_export.detail`. The implementation PR
documents them in [`LSP_DIAGNOSTIC_DATA.md`](LSP_DIAGNOSTIC_DATA.md).

No new CLI flag is proposed. The feature is driven by annotations, and the
constants in §5 are fixed so results do not depend on flags.

## 10. Measurement plan

The implementation PR, or the PR right after it, adds a measurement script under
`scripts/` (for example `scripts/measure_lemma_export.py`, in the style of
`scripts/measure_composability.py`) and records results in
[`BENCHMARK_RESULTS.md`](BENCHMARK_RESULTS.md).

### 10.1 Corpora

**Byte-identity corpus** (must show zero change):

- every `.mm` file in `std/` (the `stdlib-proof-gate` set, compared against
  `scripts/std_proof_baseline.json`);
- every `tests/*.mm` file.

**Feature corpus** (annotations added in a measurement branch, not in `std/`):

- Calls inside quantified specifications: new fixtures modelled on
  [Examples C, D, and E](#11-worked-examples), plus `abs` / `min_max` /
  `clamp` from `std/math/abs.mm`, `std/math/min_max.mm`, `std/math/clamp.mm`
  used under `forall(...)` over arrays.
- Composition-heavy std modules whose atoms call each other:
  `std/list.mm`, `std/option.mm`, `std/result.mm`, `std/compliance.mm`,
  `std/settlement.mm`, `std/container/verified_vector.mm`.
- Array-quantifier atoms, to measure the cost of MBQI off when they are also
  importers: `std/container/sorted_map.mm`, `std/container/bounded_array.mm`,
  `tests/test_array_forall_ensures.mm`, `tests/test_array_forall_store_chain.mm`,
  `tests/test_sorted_map.mm`, `tests/test_sorted_map_regression.mm`,
  `tests/test_verified_sort.mm`.
- Rejection fixtures, one per reason in §9.2, including the matching-loop shape
  of [Example I](#example-i-matching-loop-rejected).

### 10.2 Metrics

| Metric | How | Acceptance |
|---|---|---|
| Solver input identity | SMT-LIB diff per context, byte-identity corpus | No difference |
| Proof hash identity | Compare with base commit | No difference |
| Report identity | `report.json` diff, ignoring timing | No difference |
| Verdict change | Per atom, feature corpus, with and without annotations | No `proved` → `failed`; any `proved` → `unknown` listed and explained |
| `unknown` rate | Share of feature-corpus atoms that end `unknown` | Reported; regressions block |
| Wall time and solver time | `metrics.record_phase` totals, median of 5 runs | Reported per atom; > 2x on any atom is investigated |
| Quantifier instantiations | Solver `quant-instantiations` statistic | Reported; hitting the cap counts as a failure of the matching-loop check |
| Determinism | 5 runs with a fresh cache directory each, plus a run with a different file order | Identical verdicts, hashes, and reports apart from timing |
| Cache behaviour | Hit/miss counts on a second run; miss set after editing one exported clause | Only the exporter and its importers miss |
| Report size | Bytes of `report.json` per atom | Reported |
| Rejection coverage | One fixture per reason | Every reason observed with the expected diagnostic |

## 11. Worked examples

Each example was run with the current `mumei verify --json` on `develop`
(debug build), each in a fresh directory so no `.mumei_cache` could restore an
older result. "Today" is what the current verifier does; "With export" is what
this specification requires once implemented. Examples that need the
annotation are hypothetical and are marked so.

### Example A: per-call instantiation (current syntax, verified today)

```mumei
atom increment(n: i64)
    requires: n >= 0;
    ensures: result == n + 1;
    body: n + 1;

atom add_two(n: i64)
    requires: n >= 0;
    ensures: result == n + 2;
    body: {
        let x = increment(n);
        increment(x)
    };
```

Today: verified, exit `0`. This is the baseline that must not change. Neither
atom has an export annotation, so solver input, hash, and report stay
byte-identical.

### Example B: `ensures check` is hidden, `ensures assume` is visible (current syntax)

Replacing `increment`'s clause with `ensures check "exact": result == n + 1;`
makes `add_two` fail today (exit `1`, "Call to 'increment': precondition
(requires) not satisfied at call site"), because `add_two` no longer knows
`x >= 0`. With `ensures assume "exact": result == n + 1;` instead, `add_two`
verifies (exit `0`) and the assumption is a recorded trust boundary of
`increment`.

With export: `ensures check export ...` is an input error. `ensures assume
export ...` would be exported with `trust: "assumed"`.

### Example C: call under a binder in `ensures`

Current syntax:

```mumei
atom abs_val(x: i64)
    requires: true;
    ensures: result >= 0 && (result == x || result == 0 - x);
    body: if x >= 0 { x } else { 0 - x };

atom all_abs_nonneg(arr: [i64], n: i64)
    requires: n >= 0;
    ensures: forall(i, 0, n, abs_val(arr[i]) >= 0);
    body: n;
```

Today: `all_abs_nonneg` fails (exit `1`, "Postcondition (ensures) is not
satisfied"). It is true, but the call under the binder gets no usable fact.

With export (hypothetical syntax), change `abs_val`'s clause to
`ensures export: result >= 0 && (result == x || result == 0 - x);`. The fact is
`forall x. { lemma_fn_abs_val(x) } true => lemma_fn_abs_val(x) >= 0 && ...`.
The negated goal contains `lemma_fn_abs_val(select(arr, i))`, which matches the
trigger, and the expected verdict is verified. A hand-written SMT-LIB encoding
of this query with `smt.mbqi=false` returns `unsat` (that is, proved) with one
quantifier instantiation on Z3 4.14.1.

### Example D: congruence between two equal calls

Current syntax:

```mumei
atom tri(n: i64)
    requires: n >= 0;
    ensures: result >= 0;
    body: if n == 0 { 0 } else { n + tri(n - 1) };

atom tri_step(n: i64)
    requires: n >= 1;
    ensures: result == n + tri(n - 1);
    body: n + tri(n - 1);
```

Today: `tri_step` fails (exit `1`, "Spurious counterexample detected"), because
the call in `body` and the call in `ensures` are two unrelated constants.

With export: `tri` is recursive, so E4 rejects export for it
(`termination_not_established`) and nothing changes. The example still shows
what R1 buys for a non-recursive exporter: with result-function terms the two
calls are the same term `lemma_fn_tri(n - 1)`, and a hand-written SMT-LIB
encoding returns `unsat` without any quantifier instantiation. Whether a
congruence-only mode should be offered to recursive atoms is an open question.

### Example E: pre-existing unsoundness for calls under a binder in `requires`

Current syntax:

```mumei
atom ident(x: i64)
    requires: true;
    ensures: result == x;
    body: x;

atom all_equal_probe(arr: [i64], n: i64)
    requires: n >= 2 && len(arr) >= n && forall(i, 0, n, ident(arr[i]) == arr[i]);
    ensures: arr[0] == arr[1];
    body: n;
```

Today: **verified** (exit `0`, `ensures_outcomes` = `proved`). This is wrong:
the precondition is a tautology given `ident`'s contract, and it says nothing
about `arr[0]` and `arr[1]`. Replacing `ident(arr[i])` by `arr[i]` makes the same
atom fail, as it should. This is a pre-existing bug and is not fixed by this
document. [#672](https://github.com/mumei-lang/mumei/pull/672) fixes it fail-closed:
with that change the atom is reported `unverifiable` (exit `3`).

With export (hypothetical syntax `ensures export: result == x;` on `ident`):
the call is `lemma_fn_ident(select(arr, i))` under the binder (R2), and the
expected verdict is a failure. A hand-written SMT-LIB encoding with
`smt.mbqi=false` returns `sat` (the goal is not proved), as it should.

### Example F: why termination is required

Current syntax:

```mumei
atom only_zero(x: i64)
    requires: true;
    ensures: x == 0;
    body: if x == 0 { 0 } else { only_zero(x) };

atom uses_only_zero(y: i64)
    requires: true;
    ensures: y == 0;
    body: only_zero(y);
```

Today: both verify (exit `0`). That is correct for partial correctness: the
call only returns when `y == 0`. As a quantified fact, `ensures x == 0` would
become `forall x. x == 0`, which is false and would make every importer that
mentions `only_zero` anywhere, even in a specification, vacuously provable. E4
rejects it (`termination_not_established`).

### Example G: arithmetic-only lemma atom

Current syntax:

```mumei
atom lemma_square_nonneg(a: i64)
    requires: true;
    ensures: a * a >= 0;
    body: 0;

atom uses_square(a: i64, b: i64)
    requires: true;
    ensures: a * a + b * b >= 0;
    body: {
        let u = lemma_square_nonneg(a);
        let v = lemma_square_nonneg(b);
        0
    };
```

Today: verified (exit `0`). The lemma is supplied by explicit calls. With export
(hypothetical), `ensures export: a * a >= 0;` gets the automatic trigger
`lemma_square_nonneg(a)` and behaves the same way: it fires where the lemma
atom is mentioned. A user trigger `trigger(a * a)` is rejected: it is not valid
in the `trigger_arg` grammar, and even as a term it has an arithmetic head
(`arithmetic_trigger`). Supplying `a * a >= 0` with no call would need MBQI or an
arithmetic trigger, which this design rules out.

### Example H: the proposed syntax is rejected today

```mumei
atom abs_val(x: i64)
    requires: true;
    ensures export trigger(abs_val(x)) "nonneg": result >= 0;
    body: if x >= 0 { x } else { 0 - x };
```

Hypothetical syntax. Today: input error, exit `4`:
`unknown clause trust mode 'export' at 3:13 — expected assume or check` and
`expected :, found trigger at 3:20`. The implementation PR must keep this an
input error for `ensures check export`.

### Example I: matching loop (rejected)

```mumei
atom tri(n: i64)
    requires: n >= 0;
    ensures: result >= 0 && (n == 0 || result == n + tri(n - 1));
    body: if n == 0 { 0 } else { n + tri(n - 1) };
```

Current syntax, but today it **crashes** the verifier: `mumei verify` aborts with
"thread 'main' has overflowed its stack" (exit `134`), because lowering
`tri`'s `ensures` lowers the call to `tri` inside it, which lowers `tri`'s
`ensures` again. This is a pre-existing bug, outside this document's scope.

With export (hypothetical `ensures export:`): rejected twice over, as
`termination_not_established` (E4) and `matching_loop` (§4.4,
`tri -> tri via tri(n - 1)`). A direct Z3 probe of the same shape,
`forall x. { f(x) } f(x) == f(x + 1)`, times out with MBQI on and with MBQI off,
which is the behaviour the check exists to keep out.

## 12. Implementation checklist

A suggested split, one concern per PR:

1. **Parser and AST.** `export_spec` on `ensures` clauses (contextual
   identifiers), `lemma_export_hidden_clause` input error, hash section for
   exporters. No change to verification.
2. **Eligibility and trigger checker.** E1–E8, T1–T4, §4.4, `lemma_exports` in the
   report and certificate, warnings. Still no change to importers.
3. **Importer lowering.** R1–R3, result functions, §5 solver settings, importer
   hash and fingerprint sections, `lemma_imports`, `relies_on_lemmas` on covers,
   the byte-identity regression test of §6.3.
4. **Lean.** Hypothesis emission, `verify-cert` checks of §8.
5. **Measurement.** §10 script and results.

Each step must keep the byte-identity test green.

## 13. Open questions

1. **Pre-existing unsoundness (Example E). Decided:** the fix lands first, in
   its own PR ([#672](https://github.com/mumei-lang/mumei/pull/672)), and is a
   prerequisite for the implementation (§12). It rejects a call whose arguments
   mention a bound variable as `unverifiable` (exit `3`) instead of sharing one
   result constant across instances. It does not bump
   `VERIFIER_POLICY_VERSION`; only atoms that contain the pattern, directly or
   through a callee, get an extra proof-hash marker. Under R2 such calls to
   exporting callees later become result-function applications; calls to
   non-exporting callees stay `unverifiable`.
2. **Recursion measures.** E4 rejects all recursive exporters. Should mumei add a
   checked `decreases` for recursive atoms, or a realizability check
   (`forall p. requires => exists r. ensures`) as an alternative way to make the
   exported fact consistent?
3. **Congruence-only mode.** R1's result-function renaming is useful even when a
   clause is not exportable (Example D). Should a non-exportable atom be able
   to opt into congruence only?
4. **MBQI for array importers.** §5 turns MBQI off for any context that contains
   an exported fact, including atoms that rely on MBQI for their own array
   quantifiers today. If §10 shows many `proved` → `unknown` changes, do we want
   an explicit per-atom mode, or should exported facts be the only quantifiers
   restricted (for example via `smt.mbqi.id`)?
5. **Instantiation cap.** Is `10000` the right value for
   `LEMMA_EXPORT_MAX_INSTANCES`, and should hitting it be reported separately
   from other `unknown` reasons in `failure_type`?
6. **Quantified antecedents.** E7 excludes `requires` with `forall`. Many std
   containers state their invariants that way (`sorted_map.mm`). Allowing them
   needs a rule for the polarity flip (a `forall` in the antecedent becomes an
   existential in the fact). Is that needed for the first useful version?
7. **Trusted atoms.** `trusted` atoms are excluded by E3. Should their `ensures`
   be exportable with `trust: "assumed"`, on the same footing as
   `ensures assume`?
8. **Cover witness replay.** §7 trusts the solver's `sat` model as today. Should
   covers that ran with exported facts replay the witness through the existing
   counterexample-fidelity path before reporting `covered`?
9. **Cross-module exports.** Exported facts from imported modules are covered by
   the transitive hash, but packaged certificates (`mumei add`) carry their own
   proof status. Is a packaged exporter's certificate enough to discharge an
   importer's Lean hypothesis, or must it be re-verified locally?
