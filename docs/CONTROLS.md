# Contract controls

`mumei controls <input.mm>` checks whether verified atom contracts reject
deterministic, one-site changes to their implementations. It first verifies
each original atom through Mumei's normal verification path, then verifies
each generated mutation independently. The command is read-only: it does not
write a project cache or report files.

```text
mumei controls <input.mm> [--json] [--max-mutants N] [--atom NAME]
```

The default limit is 32 mutants per atom. Mutations are produced in stable
source order and include operator flips, condition flips, array-index offsets,
and integer constants changed to zero or one. No-op mutations are omitted.
Methods are reported with qualified names such as `Point::get`. Trusted,
generic, and originally unverifiable atoms are reported as `not_checked`.

The generator skips a small set of syntactically obvious equivalent operator
flips, such as `x + 0` to `x - 0` and `x * 1` to `x / 1`, including float
literal equivalents. This check is intentionally limited: other semantically
equivalent mutants can still survive and need human judgment.

## Outcomes

| Status | Meaning |
|---|---|
| `discriminating` | At least one tested mutation was rejected and none survived. |
| `weak` | At least one tested mutation still verifies. |
| `inconclusive` | Mutations were tested, but none produced a confirmed rejection or a survivor. |
| `no_mutants` | No supported mutation was available for the atom. |
| `not_checked` | The atom is trusted, generic, or its original body did not verify. |

For each mutant, `killed` means the verifier returned a structured
`Rejected` verdict, `survived` means verification succeeded, and
`unanalyzable` covers every other error, including unknown, timeout, resource
limits, unsupported constructs, translation failures, and unmarked errors.
This classification fails safe: an unmarked rejection is counted as
unanalyzable. That can under-count kills and make a report inconclusive, but
cannot overstate contract strength.

The report uses schema `mumei.controls-report/v1`. JSON contains the input,
overall `pass` or `fail` status, source-ordered atom reports, stable mutant
identifiers, and deterministic totals. Overall status is `fail` only when at
least one atom is `weak`.

| Exit code | Meaning |
|---|---|
| `0` | No atom is weak. |
| `1` | At least one atom is weak. |
| `4` | Missing or unparsable input, or an unknown `--atom`. |

## Relation to vacuity checking

`verify --enable-vacuity-check` runs the same deterministic, one-site mutation
engine as `mumei controls`, but checks up to ten mutants per atom through the
real verifier and fails the atom if any mutant survives. Unanalyzable mutants
are not counted as kills, and no-op mutations such as `Pow` flipped to `Pow`
are omitted. `mumei controls` is a separate, read-only report that provides
per-mutant outcomes and configurable limits. See the [verifier
specification](VERIFIER_SPEC.md) and [CLI reference](CLI.md).
