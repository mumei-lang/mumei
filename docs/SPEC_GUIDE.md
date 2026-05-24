# Proof-Friendly Specification Guide

This guide describes the decidable specification fragment that Mumei expects Z3 to verify reliably. Stay inside these patterns for first-pass verification; use Lean escalation for specifications that intentionally need stronger reasoning.

## Decidable fragment catalog

### Linear arithmetic

Use linear `i64` or `Nat` refinements with addition, subtraction, comparisons, and multiplication by constants.

Recommended:

```mumei
type Nat = i64 where v >= 0;

atom clamp_nonnegative(x: i64)
requires: true;
ensures: result >= 0 && result >= x;
body: { if x >= 0 { x } else { 0 } };

atom scaled(x: i64)
requires: x >= 0;
ensures: result == 3 * x && result >= x;
body: 3 * x;
```

Lean escalation candidates:

- multiplication of two symbolic variables, such as `x * y`
- symbolic division, modulo, or exponentiation
- nonlinear loop invariants, such as `result == i * i`
- algebraic equalities that require ring reasoning

### Array and sequence access

Every array or sequence read/write must have an explicit bounds condition of the form `0 <= i && i < len(a)` or an equivalent bounded quantifier range. Prefer single-index reads/writes and length-preserving updates.

Recommended:

```mumei
atom read_at(n: i64, i: i64)
requires: n >= 0 && i >= 0 && i < n;
ensures: result == arr[i];
body: arr[i];

atom update_zero(n: i64, i: i64)
requires: n >= 0 && i >= 0 && i < n;
ensures: result == 0;
body: {
    arr[i] = 0;
    arr[i]
};
```

Avoid specifications that require Z3 to infer bounds from unrelated arithmetic or nested index expressions. State the exact index range near the access.

### Quantifiers

Use `forall` only over bounded integer ranges or finite collections. Use `exists` when the witness is constructible from in-scope values.

Recommended:

```mumei
atom sorted_identity(n: i64)
requires: n >= 0 && forall(i, 0, n - 1, arr[i] <= arr[i + 1]);
ensures: result == n && forall(i, 0, result - 1, arr[i] <= arr[i + 1]);
body: n;
```

Keep quantifier bodies simple: linear arithmetic, a single array access pattern, or a single implication over a bounded range. Nested quantifiers, mixed `forall`/`exists`, and quantifiers over array reads are trigger-sensitive and may need Lean.

### Effects and temporal state

Stateful effects should be finite state machines with explicit transitions. Keep effect preconditions local to the current state and avoid encoding unbounded histories in contracts.

Recommended:

```mumei
effect File
    states: [Closed, Open];
    initial: Closed;
    transition open: Closed -> Open;
    transition write: Open -> Open;
    transition close: Open -> Closed;
```

Prefer small state sets, deterministic transitions, and explicit operation order in atom bodies.

## Anti-patterns

The verifier emits an `outside_decidable_fragment` warning when it detects patterns that are often unstable for Z3:

| Fragment tag | Typical pattern | Recommended response |
|---|---|---|
| `nonlinear_arithmetic` | `x * y`, symbolic `/`, `%`, polynomial invariant | Rewrite to linear bounds or escalate to Lean |
| `array_without_bounds` | `arr[i]` without `i >= 0 && i < n` | Add explicit bounds in `requires`, `ensures`, or quantifier range |
| `quantifier_alternation` | Mixed `forall` and `exists` obligations | Split the spec or provide a constructible witness |
| `trigger_sensitive_quantifier` | Quantifier over array access or nested quantifier | Bound the range tightly and simplify the body |
| `inductive_data_type` | Recursive enum/match shape | Prefer finite enum cases or Lean proofs |
| `recursive_invariant` | While loop or recursive invariant | Keep invariants linear and local, or escalate to Lean |
| `complex_temporal_effect` | Many states/transitions or implicit history | Reduce to finite explicit transitions |

## Property-based validation

Use property-based validation when a specification is outside Z3's most reliable fragment or you want an executable sanity check before Lean escalation.

```bash
mumei verify --property-based-test spec.mm
mumei verify --property-based-test --property-based-test-count 500 spec.mm
mumei verify --property-based-test --property-based-test-seed 12345 spec.mm
```

The validator synthesizes generators from refinement types and `requires` bounds:

```mumei
type Nat = i64 where v >= 0;
type Bounded = i64 where v >= MIN && v <= MAX;
```

- `Nat` generates boundary values near `0` plus deterministic random values in a bounded non-negative range.
- `Bounded` generates values inside the inferred `MIN..MAX` range.
- Complex predicates are scanned for direct comparisons such as `v >= n`, `v < n`, and `v == n`; unrecognized constraints fall back to a conservative integer range.
- Array-typed parameters generate small arrays, boundary lengths, and element values composed from the element generator.

On failure, Mumei shrinks the counterexample before reporting it:

- integers shrink toward `0` and inferred bounds using binary-search-like candidates
- arrays shrink by shortening length first, then shrinking individual elements
- `--property-based-test-seed` makes a failure reproducible

Best practices:

1. Keep refinement predicates close to simple bounds (`v >= 0`, `v <= max`) so generator synthesis is precise.
2. Put input-domain assumptions in `requires`; property-based validation discards generated inputs that do not satisfy preconditions.
3. Use a fixed seed in CI when investigating a failure.
4. Treat property-based success as a sanity check, not a proof. Z3/Lean verification remains the source of formal assurance.

## Recommended templates

### Linear refinement template

```mumei
type Bounded = i64 where v >= MIN && v <= MAX;

atom linear_step(x: i64, delta: i64)
requires: x >= MIN && x <= MAX && delta >= 0 && delta <= LIMIT;
ensures: result == x + delta && result >= x;
body: x + delta;
```

### Bounded array read template

```mumei
atom get(n: i64, i: i64)
requires: n >= 0 && i >= 0 && i < n;
ensures: result == arr[i];
body: arr[i];
```

### Length-preserving update template

```mumei
atom set(n: i64, i: i64, value: i64)
requires: n >= 0 && i >= 0 && i < n;
ensures: result == value;
body: {
    arr[i] = value;
    arr[i]
};
```

### Bounded universal template

```mumei
atom preserve_sorted(n: i64)
requires: n >= 0 && forall(i, 0, n - 1, arr[i] <= arr[i + 1]);
ensures: forall(i, 0, result - 1, arr[i] <= arr[i + 1]);
body: n;
```

### Constructible witness template

```mumei
atom choose_nonnegative(x: i64)
requires: true;
ensures: result >= 0 && (result == x || result == 0);
body: { if x >= 0 { x } else { 0 } };
```

Use this style instead of an existential postcondition when the returned value is the witness.

### Finite temporal effect template

```mumei
effect Session
    states: [Idle, Active, Closed];
    initial: Idle;
    transition start: Idle -> Active;
    transition finish: Active -> Closed;
```

Keep transition names aligned with `perform` operations so MIR temporal analysis can track them directly.

## Metrics and review cadence

Use `mumei verify --emit decidable-metrics --output decidable_metrics.json <file.mm>` to collect decidable-fragment warning metrics as JSON. The report includes `total_atoms_checked`, `atoms_with_warnings`, and per-tag `warning_counts`, which should be aggregated with P8-C metrics for generated specifications.

Track the following quarterly:

- `outside_decidable_fragment` warning rate: `atoms_with_warnings / total_atoms_checked`
- Z3 `unknown` rate from verification reports
- first-pass verification success rate for AI-generated specifications
- per-fragment warning counts for the seven tags in the anti-pattern table

Use the P8-C feedback loop after each quarterly rollup:

1. Identify fragment tags with the highest warning rate or largest regression.
2. Add or update atom-generation prompt guidance for those tags.
3. Add regression examples that reproduce the warning pattern.
4. Prefer Lean escalation templates when a warning represents intentional nonlinear, inductive, trigger-sensitive, or temporal reasoning.
5. Re-run the rollup and confirm progress toward the P8-D targets: 20% quarterly warning-rate reduction, Z3 `unknown` under 5%, and at least 85% first-pass verification success.

## Escalation Metrics and Feedback Loop

Lean escalation metrics track which Z3-stable obligations need Lean, which translated successfully, and which categories need prompt or guideline updates.

Collect escalation metrics with `mumei verify --emit escalation-metrics <file.mm>`. The command writes `<file>.escalation-metrics.json`; pass `--no-emit escalation-metrics` to suppress emission in wrapper flows that request metrics by default. The JSON includes:

- `escalation_attempts`: Lean escalation candidates emitted from verification.
- `lean_successes`: candidates accepted as `lean_verified`.
- `partial_translation`: candidates where the Lean translator only produced a partial proof artifact.
- `manual_required`: candidates requiring a manual lemma or human review.
- `by_failure_reason`: escalation counts grouped by reason, such as Z3 unknown, timeout, resource limit, spurious counterexample, or trusted atom review.
- `successes_by_failure_reason`: `lean_verified` counts grouped by failure reason.
- `by_logic_fragment`: escalation counts grouped by fragment tag.
- `low_success_categories`: failure reasons whose success rate is below 50%.

Use the manual feedback loop when `low_success_categories` is non-empty:

1. Identify fragment tags with the highest warning rate or largest regression.
2. Add or update atom-generation prompt guidance for those tags.
3. Add regression examples that reproduce the warning or low-success pattern.
4. Prefer Lean escalation templates for intentional nonlinear, inductive, trigger-sensitive, or temporal reasoning.
5. Re-run the rollup and confirm progress toward the P8-D targets.

P8-C success targets:

- Z3 `unknown` obligation Lean escalation success rate: at least 70%.
- partial translation rate: below 20%.
- `lean_verified` certificate re-verification success rate: 100%.
- low-success category detection rate: 100%.

## Specification Validation

Before a proof obligation is submitted to Z3, Mumei runs `check_spec_satisfiability()` on each atom. This validation rejects specifications whose preconditions or refinements are already impossible, so proof attempts do not proceed from contradictory assumptions.

The validator checks:

- `requires` satisfiability with the atom parameters and refinement constraints asserted in Z3.
- refinement type satisfiability for all available refined types.
- each top-level `ensures` conjunct against `requires`.
- pairwise `ensures` relationships with `requires ∧ ensures_i ∧ ¬ensures_j`.

A contradiction is reported as `SpecContradiction` with a kind such as `requires_unsat`, `refinement_unsat`, or `ensures_unsat`. Treat these errors as specification bugs: relax the over-constrained clause, split the atom into clearer cases, or revise the natural-language requirement before attempting proof repair.

### Traceability metadata

Atoms carry optional traceability fields:

- `trace_id`: stable ID for the originating prompt, ticket, regulation clause, or forge task.
- `spec_metadata`: key/value links such as `source`, `requirement_id`, `prompt_hash`, or `reviewer`.

Proof certificates include `spec_validation_result`, which records `is_satisfiable`, `contradiction_details`, validation status, traceability hash, and traceability coverage. The hash is SHA-256 over `trace_id`, sorted `spec_metadata`, `requires`, and `ensures`, binding the formal contract to its natural-language source.

For MCP workflows, pass traceability through `validate_logic` or `forge_blade`:

```json
{
  "source_code": "atom increment(n: i64) -> i64 requires: n >= 0; ensures: result > n; body: n + 1;",
  "trace_id": "REQ-42",
  "spec_metadata": {
    "source": "forge_task",
    "requirement_id": "REQ-42"
  }
}
```

A complete traceability record should include a non-empty `trace_id`, at least one metadata key, a meaningful `requires`, and a meaningful `ensures`; this yields 100% coverage and satisfies the ≥95% coverage target.

## Lean escalation policy

Escalate to Lean when the intended property is inherently nonlinear, inductive, trigger-sensitive, or recursive. A warning does not mean the specification is wrong; it means the spec is outside the Z3-stable fragment and should be reviewed before relying on first-pass SMT automation.
