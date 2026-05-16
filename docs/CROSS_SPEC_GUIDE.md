# Cross-Spec Verification Guide

Cross-spec verification checks whether atom-level specifications remain consistent when they are viewed as one system. Standard `mumei verify` proves each atom against its own `requires` and `ensures`; cross-spec verification adds a whole-program pass that compares contracts, shared invariants, and dependency structure across atoms.

This pass is enabled by default for new and existing projects. It makes system-level logical consistency the safe default while preserving an opt-out for projects that need a temporary performance escape hatch.

## What it verifies

### Contract consistency

For each caller-callee pair in the module dependency graph, Mumei compares the caller's contract against the callee's requirements. The pass reports inconsistent calls when a caller may invoke a callee without satisfying the callee's preconditions.

Example failure pattern:

```mumei
atom validate_balance(amount: i64) -> i64
  requires: amount >= 0;
  ensures: result >= 0;
  body: amount;

atom transfer(amount: i64) -> i64
  requires: amount >= -100;
  ensures: result >= 0;
  body: validate_balance(amount);
```

`transfer` permits `amount = -1`, but `validate_balance` requires `amount >= 0`, so the pair is not contract-consistent.

### Global invariants

Mumei infers repeated postconditions across atoms and records them as global invariants. These invariants help identify system-wide properties that multiple atoms preserve, such as non-negative balances or bounded collection lengths.

The report includes each inferred invariant, the atoms that contributed it, and a confidence score based on how broadly the invariant appears.

### Circular dependencies

Mumei builds an atom dependency graph and detects cycles. Cycles are reported in `cross_spec.json` so teams can review recursive or mutually dependent specifications before they hide inconsistent assumptions.

## Default behavior

`mumei verify` and `mumei build` use the manifest proof configuration when a `mumei.toml` is present. If a project has no explicit cross-spec setting, Mumei now treats it as enabled:

```toml
[proof]
cache = true
timeout_ms = 10000
cross_spec_verify = true
```

`mumei init` writes this setting into every new project's `mumei.toml`.

Cross-spec results are written to `cross_spec.json` in the report/output directory. For verification-only workflows, use `--report-dir` to control the location:

```bash
mumei verify --report-dir reports src/main.mm
```

## Performance impact

Cross-spec verification performs additional analysis over all known atoms and their dependency graph. The cost is usually small for typical modules, but it grows with:

- number of atoms;
- density of caller-callee relationships;
- number of inferred shared postconditions;
- large dependency cycles.

The regular proof cache remains enabled by default. If the pass becomes a bottleneck in a large project, first keep `cache = true` and narrow the verification target where possible.

## Disabling cross-spec verification

Prefer leaving the pass enabled in CI and release builds. To temporarily disable it for a project, set the manifest value to `false`:

```toml
[proof]
cache = true
timeout_ms = 10000
cross_spec_verify = false
```

This disables the default for commands that read `mumei.toml`. Re-enable it before merging contract-heavy or standard-library changes.

## Migrating existing projects

Existing projects do not need to edit `mumei.toml` to receive the safer default. After upgrading Mumei, run:

```bash
mumei verify --report-dir reports src/main.mm
```

Then review `reports/cross_spec.json`:

1. Check `summary.inconsistent_calls`; fix callers or strengthen contracts until it reaches `0`.
2. Review `global_invariants` for properties that should become explicit contracts.
3. Review `circular_dependencies`; keep intentional cycles documented and break accidental cycles.
4. Commit an explicit `cross_spec_verify = true` only if you want the manifest to document the default.

For temporary migration windows, set `cross_spec_verify = false`, file follow-up work for each reported issue, and remove the opt-out once the project is consistent.
