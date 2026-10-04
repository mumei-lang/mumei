---
layout: default
title: "Verifier Specification — Mumei"
description: "Declarative rules for what mumei verify decides: phase contracts, ensures outcomes, clause trust modes, cover clauses, trust boundaries, counterexample fidelity, exit codes, and recursive contracts."
keywords: "mumei verifier specification, phase contracts, ensures outcomes, trust boundary, cover, counterexample fidelity, recursive contracts, decreases"
---

# Verifier Specification

This document states, as rules, what `mumei verify` decides for an atom and
which evidence each decision rests on. It does not walk through the code; each
rule names the function that implements it so a reader can check the rule
against the source.

Conventions:

- Paths are relative to the repository root. `executor.rs` is short for
  `mumei-core/src/verification/executor.rs`, which hosts `verify_inner`, the
  per-atom pipeline.
- `R` is the atom's requires view, `B` the constraints produced by encoding the
  body, `Q` one ensures conjunct, and `ctx = R ∧ B`.
- "Sat", "Unsat", and "Unknown" are the three answers of one solver check.
- A rule written "X ⇒ Y" means the verifier reports Y whenever X holds, and in
  no other situation unless another rule says so.

The rules below describe the current behaviour of `develop`. When a rule and the
code disagree, the code is authoritative and this document has a bug; the
phase table is checked mechanically (see [Keeping this document in
sync](#keeping-this-document-in-sync)).

## 1. Phases and their contracts

The pipeline is a fixed sequence of phases. A phase may *require* facts that an
earlier phase *establishes*, and may *invalidate* a fact so that no later phase
can rely on it. Facts are declared only where data actually flows between
phases; the static checks in phases 0 and 1 are independent of one another and
declare no facts.

| Fact | Meaning |
|---|---|
| `solver_context` | The solver holds `ctx` for the atom: requires (body view) plus the body encoding |
| `body_result` | The encoded body result, still usable as `result` for postcondition checks |
| `body_result_value` | The solver term of the body result, used to read `result` from a model |
| `ensures_outcomes` | One outcome per ensures conjunct (section 2) |
| `context_reachability` | Whether `ctx` is satisfiable: `reachable`, `unreachable`, or `unknown` |
| `cover_results` | One result per `cover` clause (section 4) |

Rules:

- **P1 (order).** A phase may run only if every fact it requires has been
  established by an earlier phase and not invalidated since.
  `mumei-core/src/verification/phase_contract.rs::check_phase_order`, applied to
  `PHASE_CONTRACTS` by the `declared_phase_order_is_valid` unit test.
- **P2 (naming).** A phase recorded in metrics as `<name> (<outcome>)` — for
  example `Phase 6: final Z3 check (unknown)` — belongs to the contract named
  `<name>`. `phase_contract.rs::phase_contract`.
- **P3 (rejection is final).** A phase that rejects the atom returns an error
  from `verify_inner`; no later phase runs for that atom. `executor.rs::verify_inner`.

The table below is the contract table. Paths in the last column are relative to
`mumei-core/src/`.

<!-- phase-contracts:begin -->
| Phase | Requires | Establishes | Invalidates | Counterexample fidelity | Implemented by |
|---|---|---|---|---|---|
| `Phase 0-units: unit consistency` | — | — | — | — | `verification/support/units.rs::verify_unit_consistency` |
| `Phase 0-nominal: nominal struct types` | — | — | — | — | `verification/support/nominal_types.rs::verify_nominal_struct_types` |
| `Phase 0a: spec validation` | — | — | — | — | `verification/spec_validation.rs::check_spec_satisfiability_with_timeout` |
| `Phase 1a: resource hierarchy` | — | — | — | — | `verification/support/resource_safety.rs::verify_resource_hierarchy` |
| `Phase 1f: effect containment` | — | — | — | — | `verification/support/effects.rs::verify_effect_containment` |
| `Phase 1f-1: replayability` | — | — | — | — | `verification/support/replay.rs::verify_replayability` |
| `Phase 1b: BMC resource safety` | — | — | — | `bounded` | `verification/support/resource_safety.rs::verify_bmc_resource_safety` |
| `Phase 1c: async recursion depth` | — | — | — | — | `verification/support/resource_safety.rs::verify_async_recursion_depth` |
| `Phase 1d: atom invariant` | — | — | — | — | `verification/support/call_graph.rs::verify_atom_invariant` |
| `Phase 1e: call graph cycles` | — | — | — | — | `verification/support/call_graph.rs::verify_call_graph_cycles` |
| `Phase 1g: effect params` | — | — | — | — | `verification/support/effects.rs::verify_effect_params` |
| `Phase 1h-2: structured concurrency ownership` | — | — | — | — | `verification/support/task_ownership.rs::verify_task_ownership` |
| `Phase 1h: MIR move analysis` | — | — | — | — | `mir_analysis/move_analysis.rs::analyze_moves`, `mir_analysis/borrow_check.rs::check_borrows_with_callees` |
| `Phase 1i: vacuity checking` | — | — | — | — | `verification/vacuity.rs::check_spec_vacuity_for_hir` |
| `Phase 1j: temporal effects` | — | — | — | — | `mir_analysis/temporal_effects.rs::analyze_temporal_effects_with_contracts` |
| `Phase 4: body evaluation` | — | `solver_context`, `body_result`, `body_result_value` | — | `approximate` | `verification/executor.rs::verify_inner` |
| `Phase 5: ensures verification` | `solver_context`, `body_result`, `body_result_value` | `ensures_outcomes` | `body_result` | `approximate` | `verification/executor.rs::verify_inner`, `verification/vc_outcome.rs::classify` |
| `Phase 6: final Z3 check` | `solver_context`, `ensures_outcomes` | `context_reachability` | — | — | `verification/executor.rs::verify_inner` |
| `Phase 7: cover witnesses` | `solver_context`, `body_result_value` | `cover_results` | — | `exact` | `verification/executor.rs::verify_inner` |
<!-- phase-contracts:end -->

Notes on the table:

- `—` means "none". A phase whose fidelity is `—` never produces a
  counterexample of its own; section 6 says what fidelity is reported anyway.
- Phase 1i runs only when vacuity checking is enabled
  (`VerifyInnerOptions::enable_vacuity_check`); trusted atoms skip phases 4–7
  (rule T4).
- Phase 5 invalidates `body_result`, so a later phase that needs the body
  result must require `body_result_value` instead, as Phase 7 does.

## 2. Ensures outcomes

Every top-level conjunct `Q` of the body's ensures view (rule M2) gets exactly
one outcome. The outcome is a function of at most two solver checks made in a
frame on top of `ctx`:

- the **refutation query** `ctx ∧ ¬Q`, always made when `Q` lowers;
- the **holds query** `ctx ∧ Q`, made only when the refutation query is Sat.

`mumei-core/src/verification/vc_outcome.rs::classify` maps the answers; the
queries are issued in `executor.rs::verify_inner` (Phase 5), and conjuncts come
from `spec_validation.rs::split_top_level_conjunctions`.

| Outcome | Decided by | Meaning |
|---|---|---|
| `vacuous` | `context_reachability = unreachable` | No input satisfies `ctx`, so `Q` holds trivially. Never a verdict on its own (rule E3) |
| `proved` | refutation Unsat (context not unreachable) | `Q` holds for every input satisfying `ctx` |
| `always_false` | refutation Sat, holds Unsat | `Q` fails for every input satisfying `ctx`; the specification or the body is likely wrong |
| `fails_on_some_inputs` | refutation Sat, holds Sat | `Q` holds for some inputs and fails for others |
| `fails` | refutation Sat, holds Unknown | `Q` fails for at least one input; the solver could not say whether it ever holds |
| `unknown` | refutation Unknown, or the counterexample is a spurious candidate (rule E4) | Nothing is claimed about `Q` |
| `skipped` | `Q` could not be lowered to the solver (rule E5) | Nothing is claimed about `Q` |
| `assumed` | `Q` is an `ensures assume` clause (rule M3) | `Q` was trusted, not proved |

Rules:

- **E1 (refutation first).** The holds query is made only when the refutation
  query is Sat, so a `proved` or `unknown` conjunct costs one check, a
  `skipped` conjunct costs none, and a failing conjunct costs two. A
  refutation that returns Unknown ends Phase 5 at once: the atom fails with a
  "Z3 returned unknown" error, which is inconclusive (rule X2). `executor.rs::verify_inner` (Phase 5).
- **E2 (reachability is learned, never guessed).** `context_reachability`
  starts as `unknown`; a Sat refutation proves the context reachable, and
  Phase 6 sets it from its own check of `ctx` (`reachable` on Sat,
  `unreachable` on Unsat, `unknown` on Unknown). `executor.rs::verify_inner`.
- **E3 (vacuity is rejected).** An unsatisfiable `ctx` is a rejection
  ("Logic contradiction", Phase 6), not a success. Before rejecting, Phase 6
  reports every conjunct that Phase 5 classified as `proved` as `vacuous`
  instead and adds a "vacuous verification context" diagnostic, so the reader
  can see why the proof meant nothing. `executor.rs::verify_inner` (Phase 6).
  Phase 1i additionally flags vacuous specifications when vacuity checking is
  enabled
  (`vacuity.rs::check_spec_vacuity_for_hir`).
- **E4 (spurious counterexamples are not failures of `Q`).** When spurious
  detection is enabled and a Sat refutation's model does not reproduce the
  violation under Mumei semantics, the conjunct's outcome is `unknown`, not one
  of the failing outcomes, and the atom fails with a "spurious
  counterexample" error that counts as inconclusive (exit `3`, rule X4).
  `executor.rs::verify_inner`,
  `verification/spurious_detection.rs::validate_counterexample`.
- **E5 (unlowerable is not proved).** A conjunct the solver encoding does not
  support is `skipped`, and an atom with any skipped ensures conjunct fails with
  the `Unverifiable` error (exit `3`, rule X3). A conjunct that is empty or the
  literal `true` produces no outcome. Any other lowering error is a hard error.
  `executor.rs::lower_clause_with_skip`,
  `mumei-core/src/verification/types.rs::UNVERIFIABLE_ERROR_PREFIX`.
- **E6 (report shape).** The report lists `context_reachability` and one
  `ensures_outcomes` entry (`clause`, `outcome`, optional `label`) per conjunct.
  The `assumed` entries come first, in source order, followed by the checked
  conjuncts in source order. `executor.rs::ensures_outcome_summary`; field reference in
  [`REPORT_SCHEMA.md`](REPORT_SCHEMA.md).

## 3. Clause trust modes

A `requires` or `ensures` clause may be marked `assume` or `check`
(syntax in [`LANGUAGE.md`](LANGUAGE.md#clause-trust-modes)). The verifier never
sees the clause list directly; it reads one of four *views* of the contract.
Each view is the clause text with the conjuncts of one mode removed.

| View | Used for | Drops |
|---|---|---|
| `BodyRequires` | Assumptions while verifying the atom's own body | `requires check` |
| `CallerRequires` | Obligations a caller must prove | `requires assume` |
| `BodyEnsures` | Postconditions the body must prove | `ensures assume` |
| `CallerEnsures` | Facts a caller may assume after the call | `ensures check` |

`mumei-core/src/verification/contract_view.rs::contract_view`,
`contract_view.rs::dropped_conjuncts`.

Resulting matrix ("Proved" = an obligation is generated, "Assumed" = the clause
is added as a hypothesis):

| Clause | Inside the atom | At each call site |
|---|---|---|
| `requires: e;` | Assumed | Proved |
| `requires assume: e;` | Assumed | Not proved — trust boundary |
| `requires check: e;` | Not assumed | Proved |
| `ensures: e;` | Proved | Assumed |
| `ensures assume: e;` | Not proved — trust boundary | Assumed |
| `ensures check: e;` | Proved | Not assumed |

Rules:

- **M1 (no mode, no change).** An atom without trust modes gets its original
  `requires`/`ensures` text in every view, so its solver input and cache hash
  are unchanged. `contract_view.rs::contract_view` (early return when
  `clause_modes` is empty).
- **M2 (views are conjunct-exact).** A view removes a mode's conjuncts one
  occurrence at a time; a conjunct that appears both with and without the
  dropped mode is kept once for the remaining occurrence. `contract_view.rs`
  (`contract_view_with_dropped_conjuncts`).
- **M3 (assume is reported).** Each dropped `ensures assume` conjunct produces
  an `assumed` outcome and an "assumed ensures clause … was not proved"
  diagnostic instead of being silently ignored.
  `executor.rs::verify_inner` (loop over `dropped_conjuncts(atom,
  ContractView::BodyEnsures)`).
- **M4 (check is sound).** `check` never weakens what is proved; it only
  withholds a fact from the other side of the call. `requires check` is
  therefore *not* a trust boundary, and `ensures check` leaves the caller with
  less information, never more.
- **M5 (call sites read caller views).** Call encoding proves
  `CallerRequires` and assumes `CallerEnsures`.
  `mumei-core/src/verification/translator/expr.rs::expr_to_z3` (call
  encoding), `translator/constraints.rs::check_contract_subsumption`, and the
  same views in `support/dataflow_inference.rs` and `spurious_detection.rs`.

## 4. `cover` clauses

A `cover` clause asks for a witness: an input that satisfies the atom's
requires and drives the body to a state where the cover expression holds.

| Result | Decided by | Effect |
|---|---|---|
| `covered` | `R_cover ∧ B ∧ C` Sat | Recorded with a witness (parameter values read from the model) |
| unreachable | `R_cover ∧ B ∧ C` Unsat | The atom is rejected (`failure_type: "cover_unreachable"`, exit `1`) |
| `unknown` | the check is Unknown, or `C` cannot be checked (rule C4) | Recorded with a warning; nothing is claimed |

Here `C` is the cover expression together with any side obligations its
lowering produced (for example a non-zero divisor), and
`R_cover = BodyRequires ∧ (every requires check clause)`.

Rules:

- **C1 (after the verdict).** Covers are checked in Phase 7, after Phase 6
  accepted the atom; a rejected atom has no cover results.
  `executor.rs::verify_inner`.
- **C2 (`requires check` counts for covers).** A `requires check` clause is
  not assumed while proving the body (rule M4), but it still constrains which
  executions a caller can produce, so the cover frame adds it back. This is why
  the cover frame starts from the environment captured *before* the body was
  encoded (`cover_pre_body_env`): the clause is lowered over the parameters, not
  over values the body rebinds. `executor.rs::verify_inner` (Phase 7).
- **C3 (no encoding change).** Covers add queries only in their own push/pop
  frames after Phase 6; they never add a hypothesis to the frames that decide
  ensures outcomes or the final check.
- **C4 (fail closed to `unknown`).** A cover is `unknown`, never `covered` or
  unreachable, when: a `requires check` clause cannot be lowered (whether it
  is unsupported or its lowering errors); the cover itself uses a construct
  the solver encoding does not support (it is `skipped` by
  `lower_clause_with_skip`); it uses a bitwise operator under the default Int
  encoding; or it refers to `result` of a tuple-returning atom. Any other
  lowering error in the cover itself is a hard error that fails the atom, as
  for ensures (rule E5). `executor.rs::verify_inner` (Phase 7).
- **C5 (trusted atoms).** A trusted atom has no body verification, so its
  covers are not checked and a warning says so. `executor.rs::verify_inner`
  (`TrustLevel::Trusted` branch).

## 5. Trust boundaries

A trust boundary is a place where the verifier accepts a fact without proving
it. Every boundary is classified, reported, and recorded in the proof
certificate; none of them turns an `unknown` into `proved`.

| Kind | Arises when | Code |
|---|---|---|
| `trusted_atom` | The atom is declared `trusted` | `mumei-core/src/trust_boundary.rs::classify_trust_boundaries` |
| `extern_boundary` | The atom is (an alias of) an `extern` function | same |
| `effect_state_assumption` | The atom has an `effect_pre` assumption | same |
| `assumed_clause` | The atom has at least one `requires assume` or `ensures assume` clause | same |

Rules:

- **T1 (assumed clauses are recorded).** Every `assume` clause is written to the
  certificate's `assumed_clauses` list as `requires: <clause>` or
  `ensures: <clause>`. `mumei-core/src/proof_cert/generation.rs` (atom
  certificate construction), field in `proof_cert/models.rs`.
- **T2 (Lean kernel-axiom allowlist).** The Lean audit of a certificate atom
  passes only if every kernel axiom it reports is one of `propext`,
  `Classical.choice`, `Quot.sound`. Any other axiom, an audit status of
  `rejected`, `error`, or any unrecognised value, or `passed` without an axiom
  list makes the audit fail (`Rejected` or `Error`). An atom with no Lean
  metadata, or with neither an audit status nor an axiom list, is `Unaudited`
  (rule T4). `mumei-core/src/proof_cert/validation.rs::lean_axiom_audit`,
  `LEAN_STANDARD_KERNEL_AXIOMS`.
- **T3 (failed audits are named).** With `--allow-lean-verified`, a certificate
  atom whose audit is rejected or errored is reported as `axiom_rejected`,
  never as `proven`. `proof_cert/validation.rs::verify_certificate`. The same
  audit drives LSP certificate diagnostics
  (`src/lsp.rs::append_certificate_lean_escalation_diagnostics`) and Lean
  promotion in `mumei verify` (`src/commands/verify.rs`).
- **T4 (unaudited is not audited).** A certificate atom without audit metadata
  is `Unaudited`: accepted as legacy evidence by default, rejected by
  `verify-cert --strict --allow-lean-verified`, and always displayed as
  `unaudited` rather than `passed`. `validation.rs::lean_axiom_audit`,
  `src/commands/verify_cert.rs`. The full acceptance contract is in
  [`PROOF_CERTIFICATE.md`](PROOF_CERTIFICATE.md#lean_verified-acceptance-contract).
- **T5 (trusted atoms).** A trusted atom's contract is assumed and its body is
  not verified. `executor.rs::verify_inner` (`TrustLevel::Trusted` branch).

## 6. Counterexample fidelity

A reported counterexample carries `counterexample_fidelity`, which says how far
it can be believed:

| Fidelity | Meaning |
|---|---|
| `exact` | Replayed under Mumei semantics and the violation reproduced |
| `bounded` | Produced by a bounded-depth phase; a real violation within the bound, silent beyond it |
| `approximate` | Taken from the solver model without a successful replay |

Rules:

- **F1 (replay wins).** A counterexample whose replay status is `validated` is
  `exact`, whatever phase produced it.
  `mumei-core/src/verification/phase_contract.rs::counterexample_fidelity`,
  replay in `spurious_detection.rs::validate_counterexample`.
- **F2 (otherwise the phase decides).** Without a validated replay the fidelity
  is the producing phase's declared fidelity (the table in section 1), and
  `approximate` for a phase that declares none. Unknown or failed replays never
  upgrade a counterexample. `phase_contract.rs::counterexample_fidelity`.
- **F3 (cover witnesses are exact).** Phase 7 declares `exact`: a witness is a
  model of the very query that decided `covered`.

## 7. Exit codes

The process exit code is defined in [`CLI.md`](CLI.md#exit-codes); this section
only says which per-atom results feed each code.

| Code | Name | Code constant |
|---|---|---|
| `0` | Verified | `EXIT_VERIFIED` |
| `1` | Rejected | `EXIT_REJECTED` |
| `2` | Usage error | `EXIT_USAGE_ERROR` |
| `3` | Inconclusive | `EXIT_INCONCLUSIVE` |
| `4` | Input error | `EXIT_INPUT_ERROR` |
| `5` | Internal error | `EXIT_INTERNAL_ERROR` |

Rules (all in `src/commands/verify.rs`):

- **X1 (per-file outcome).** `VerifyOutcome::from_counts` decides, in this
  order: any internal error ⇒ `5`; any failed atom that is not merely
  solver-inconclusive ⇒ `1`; any inconclusive failure, unverifiable atom, or
  open Lean escalation ⇒ `3`; otherwise `0`.
- **X2 (what counts as inconclusive).** A failed atom is solver-inconclusive
  exactly when its solver result is `unknown`, `timeout`, `resource_limit`, or
  `spurious_candidate`.
  `is_solver_inconclusive`, with the result recovered from the error by
  `mumei-core/src/verification/types.rs::z3_result_from_error_message`.
- **X3 (outcome to code).** Under rule X1, for an atom that is not escalated to
  Lean: `always_false`, `fails_on_some_inputs`, `fails`, a contradiction
  (`vacuous`), and an unreachable cover reject (`1`); a
  `termination_measure_violation` (rule D4) rejects (`1`); a refutation or
  final check that returns Unknown is inconclusive (`3`); a `skipped`
  conjunct makes the atom unverifiable (`3`). `proved` and `assumed` do not
  lower the result.
- **X4 (spurious candidates).** The solver result of a spurious counterexample
  is `spurious_candidate`, which is in the inconclusive set (rule X2), so such
  an atom exits `3`, matching the conjunct's `unknown` outcome (rule E4). A
  candidate that does not replay neither proves nor refutes the clause. The
  same holds under `--escalate-lean` when Lean does not discharge the
  candidate.
- **X5 (directories).** A directory run exits with the most severe per-file
  outcome, ordered `5 > 4 > 1 > 3 > 0`. `VerifyOutcome::combine`.

## 8. Recursive contracts

An atom is *recursive* when it lies on a cycle of the atom call graph. The
graph has an edge for every call in the body and in the contracts (`requires`
and `ensures` in every clause mode, labelled or not), for direct calls and for
static `call(atom_ref(f), ..)` calls. A self-loop is a cycle.
`verification/support/recursion.rs::recursive_scc` computes the strongly
connected component (SCC) of an atom.

Rules:

- **D1 (eligibility).** A recursive SCC is *eligible* exactly when every member
  declares an atom-level `decreases: M;` whose measure is call-free and mentions
  only that member's own parameters, has no effects, no `ref mut` or `consume`
  parameters, is not `async`, has no type parameters, is at the default
  verified trust level, has no `ensures assume` clause, and has only scalar
  (`Int`- or `Bool`-sorted) parameters and result.
  `recursion.rs::member_unsupported_reason`.
- **D2 (congruent calls).** While the main verification of an atom runs
  (`executor.rs::verify_inner`), a call to a member `g` of an eligible SCC, from
  any caller, lowers to the application `rec_fn#g(args)` of one uninterpreted
  function per atom, instead of a fresh `call_<name>_<n>` constant. Calls with
  equal arguments therefore have equal results. Auxiliary contexts (spec
  validation, vacuity, property-based checks) keep fresh constants.
  `translator/context.rs::VCtx::callee_congruent`, `VCtx::rec_fn`.
  A call whose argument depends on a variable bound by an enclosing
  `forall`/`exists` is rejected first by `VCtx::reject_quantifier_dependent_call`,
  recursive calls included, so the enclosing clause is unverifiable (exit `3`)
  and no `rec_fn#` application or termination obligation is built for it.
- **D3 (assumed ensures).** At a congruent call the callee's caller-visible
  `ensures` is assumed as the implication
  `R(args) ⇒ CallerEnsures(args, rec_fn#g(args))`, where `R` is
  `contract_view.rs::caller_requires_obligation(g)`: the caller-view requires
  plus `g`'s top-level quantified requires conjuncts. For a call
  between two members of the same SCC, the antecedent also contains the path
  conditions at the call. A call in a body still checks the same `R`
  as an obligation, as for any other call. A recursive call reached while the
  callee's contract is already being instantiated uses the same application
  and does not instantiate the contract again.
  `translator/expr.rs::congruent_ensures_antecedent`.
- **D4 (termination obligation).** At every call from member `A` to member `B`
  of the same eligible SCC, in `A`'s body and in `A`'s `requires` and
  `ensures`, the verifier checks `0 <= M_A ∧ M_B(args) < M_A` under `A`'s
  body-view requires and the path conditions at the call, including the
  short-circuit guards of `&&`, `||`, and `if` inside a contract. Bit-vector
  measures are compared signed. If the check is not Unsat, or the two measures
  lower to incompatible sorts, the atom is rejected with `failure_type`
  `termination_measure_violation` (exit `1`) and a counterexample over `A`'s
  parameters. Checking contract-level calls is what rejects a specification
  such as `ensures: result == f(x) + 1;`, which would otherwise assume
  `f(x) == f(x) + 1`. `translator/expr.rs::termination_obligation_at_call`.
- **D5 (ineligible SCCs).** Calls into an ineligible recursive SCC keep fresh
  constants, and a call reached while the callee's contract is already being
  instantiated gets an unconstrained result and skips the callee's contract. When
  the atom's own contract calls into its SCC, the verifier adds an advisory
  `recursive_contract_needs_decreases` (some member has no `decreases`) or
  `recursive_contract_unsupported` (any other D1 failure) diagnostic. Neither
  changes the verdict. `recursion.rs::recursive_contract_hint_diagnostic`.
- **D6 (cache).** The `decreases` text is part of the atom hash and the proof
  hash, and a callee's `decreases` is part of every caller's proof hash.
  `mumei-core/src/resolver/cache.rs`. These rules are enabled by
  `VERIFIER_POLICY_VERSION` 7.

## Keeping this document in sync

The phase table in section 1 is compared against `PHASE_CONTRACTS` by
`mumei-core/tests/verifier_spec_doc.rs`, which runs in the `Verifier Spec`
workflow. The test reads the rows between the `phase-contracts:begin` and
`phase-contracts:end` markers and fails if a phase is added, removed, renamed,
reordered, or changes its requires, establishes, invalidates, or fidelity
entries. The `Implemented by` column is for readers and is not compared.

When you change a phase contract, update the table in the same PR; when you
change a rule elsewhere in this document, update the cited function too.
