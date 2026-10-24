# Charter

A Charter is a human-owned statement of the contracts an implementation must
preserve. The gate checks signatures and contract refinement; it does not
compile or verify the bodies of Charter atoms.

## Format and lock

Charter files use the regular Mumei syntax and the `.charter.mm` suffix. Each
Charter atom is declared `trusted` with a placeholder body:

```mumei
trusted atom add_positive(x: i64) -> i64
requires: x >= 0;
ensures: result >= x + 1;
body: 0;
```

The body is ignored. `trusted` records that the contract is owned and reviewed
by a person, not that the placeholder body was checked.

`mumei charter lock` writes a deterministic `charter.lock` containing the
SHA-256 hash of each Charter file's bytes. Paths are relative to the project
directory and sorted in the lock. Editing, adding, removing, or renaming a
Charter requires running the lock command as a deliberate human step.

```bash
mumei charter lock --charter specs/core.charter.mm
```

Projects may declare defaults in `mumei.toml`:

```toml
[charter]
files = ["specs/core.charter.mm"]
lock = "charter.lock"
```

The `lock` value defaults to `charter.lock`. Repeated `--charter` arguments
select the Charter files for a command; with no such arguments, the configured
files are used.

## Check

```bash
mumei charter check src/main.mm
mumei charter check src/main.mm --charter specs/core.charter.mm --lock charter.lock --json
```

The check applies these gates in order:

1. Every selected Charter must be present in the lock and its file hash must
   match. Missing or changed files require a new lock.
2. Every Charter atom must have an implementation atom with the same name,
   parameter names and types, and return type. A bare method name can resolve
   to a uniquely matching `Type::method`.
3. The implementation must not require more than the Charter and must ensure
   the Charter's postcondition. These implications are checked with the
   implementation's parameter refinements. Empty `requires` and `ensures`
   clauses are treated as `true`. Unsupported clauses or solver uncertainty
   fail closed as `inconclusive`.

Failed implications include a counterexample mapping parameter names to the
values returned by the solver when available. The check writes
`charter_report.json` in the project directory. `--json` also prints that
report to standard output. The report schema is `mumei.charter-report/v1` and
contains per-atom results and a separate lock result.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | The lock, signature, and refinement checks passed. |
| `1` | A lock, coverage, refinement, or solver-certainty gate failed. |
| `4` | An input file, manifest, lock, or report could not be read or written. |

Charter checking is read-only with respect to the implementation and does not
run verification or modify its contracts.
