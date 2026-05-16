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

## Lean escalation policy

Escalate to Lean when the intended property is inherently nonlinear, inductive, trigger-sensitive, or recursive. A warning does not mean the specification is wrong; it means the spec is outside the Z3-stable fragment and should be reviewed before relying on first-pass SMT automation.
