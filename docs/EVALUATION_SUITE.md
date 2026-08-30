# Evaluation Suite Results

Time-series results of the six-axis evaluation suite of `PAPER_DRAFT.md` §7,
measured by `benchmarks/evaluation_suite.py` over the controlled benchmark
task set in `benchmarks/`.

## Evaluation Suite Run — 2026-08-30 05:18 UTC

`budget_policy_fingerprint`: `sha256:evaluation-suite-default`. An axis reports `SKIP` when
its input is absent (no `mumei` binary, or no agent repair data in the
proof certificates), never a substituted value.

### Axis Summary (PAPER_DRAFT.md §7)

| Axis | Status | Result |
|------|--------|--------|
| proof success rate | MEASURED | 100.00% (46/46 files) |
| repair convergence | SKIP | SKIP |
| counterexample quality | MEASURED | 100.00% (20/20 caught) |
| trust surface | MEASURED | 0 trusted / 105 atoms, 0 FFI declarations, 6 Lean escalation candidates |
| user burden | MEASURED | 2.4381 clauses/atom, 1.7871 spec/impl tokens |
| runtime artifact utility | MEASURED | 93.27% (97/104 emissions) |

### Per-Category Results

| Category | Files | Success Rate | Counterexample Catch | Trusted / Atoms | Clauses/Atom | Spec/Impl Tokens | Artifact Emission | Repair Convergence |
|----------|-------|--------------|----------------------|-----------------|--------------|------------------|-------------------|--------------------|
| arithmetic | 9 | 100.00% | 100.00% | 0 / 27 | 2.0000 | 3.5354 | 87.50% (21/24) | SKIP |
| concurrency | 15 | 100.00% | 100.00% | 0 / 33 | 2.0000 | 0.8939 | 100.00% (20/20) | SKIP |
| dafny_puzzles | 3 | 100.00% | SKIP | 0 / 3 | 2.0000 | 1.6970 | 100.00% (12/12) | SKIP |
| domain_compliance | 10 | 100.00% | 100.00% | 0 / 25 | 2.7200 | 2.0936 | 83.33% (20/24) | SKIP |
| state_machine | 6 | 100.00% | 100.00% | 0 / 14 | 4.0000 | 1.7844 | 100.00% (12/12) | SKIP |
| svcomp_style | 3 | 100.00% | SKIP | 0 / 3 | 2.0000 | 1.5000 | 100.00% (12/12) | SKIP |

### User Burden by Clause Kind

| Category | requires | ensures | invariant | effect_pre | effect_post | Spec Tokens | Impl Tokens |
|----------|---------:|--------:|----------:|-----------:|------------:|------------:|------------:|
| arithmetic | 27 | 27 | 0 | 0 | 0 | 799 | 226 |
| concurrency | 33 | 33 | 0 | 0 | 0 | 497 | 556 |
| dafny_puzzles | 3 | 3 | 0 | 0 | 0 | 56 | 33 |
| domain_compliance | 25 | 25 | 0 | 9 | 9 | 850 | 406 |
| state_machine | 14 | 14 | 0 | 14 | 14 | 298 | 167 |
| svcomp_style | 3 | 3 | 0 | 0 | 0 | 102 | 68 |

### Runtime Artifact Utility by Target

| Category | `llvm-ir` | `c-header` | `verified-json` | `proof-cert` | Files |
|----------|------|------|------|------|------|
| arithmetic | 5 | 5 | 5 | 6 | 6 |
| concurrency | 5 | 5 | 5 | 5 | 5 |
| dafny_puzzles | 3 | 3 | 3 | 3 | 3 |
| domain_compliance | 4 | 5 | 5 | 6 | 6 |
| state_machine | 3 | 3 | 3 | 3 | 3 |
| svcomp_style | 3 | 3 | 3 | 3 | 3 |

### Artifact Emission Gaps

| Category | File | Target |
|----------|------|--------|
| arithmetic | `finite_field_modular.mm` | `llvm-ir` |
| arithmetic | `finite_field_modular.mm` | `c-header` |
| arithmetic | `finite_field_modular.mm` | `verified-json` |
| domain_compliance | `modular_commitment.mm` | `llvm-ir` |
| domain_compliance | `modular_commitment.mm` | `c-header` |
| domain_compliance | `modular_commitment.mm` | `verified-json` |
| domain_compliance | `regtech_exhaustiveness.mm` | `llvm-ir` |
