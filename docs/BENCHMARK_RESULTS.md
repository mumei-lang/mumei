# Benchmark Results

Time-series benchmark results for the mumei verification pipeline.

`python3 benchmarks/run_benchmarks.py --forge-feedback <path>` additionally emits
a `mumei.benchmark_forge_feedback/v1` document that maps each category's weakness
score (expected-outcome match rate, counterexample catch rate, trusted ratio,
plus Z3 / Lean solver-time signals) to a negative priority delta over the stdlib
domains in `CATEGORY_STD_DOMAINS`. Feed it to the vStd pipeline with
`python -m agent forge --benchmark-feedback <path>` or
`python -m agent proliferate --benchmark-feedback <path>`; it only reorders work
the pipeline already planned.

## Benchmark Run — 2026-07-06 13:13 UTC

### Stdlib Health Summary

| Modules | Atoms | Proven | Trusted | Trusted Ratio |
|---------|-------|--------|---------|---------------|
| 58 | 338 | 338 | 0 | 0.0000 |

### Category Results

| Category | Files | Atoms | Trusted | Success Rate | Avg Solver Time |
|----------|-------|-------|---------|--------------|-----------------|
| dafny_puzzles | 3 | 3 | 0 | 100.00% | 0.120s |
| svcomp_style | 3 | 3 | 0 | 100.00% | 0.064s |

<details><summary>Per-file details</summary>

#### dafny_puzzles

| File | Atoms | Trusted | Verified | Solver Time |
|------|-------|---------|----------|-------------|
| absolute_value.mm | 1 | 0 | PASS | 0.194s |
| max.mm | 1 | 0 | PASS | 0.110s |
| swap.mm | 1 | 0 | PASS | 0.057s |

#### svcomp_style

| File | Atoms | Trusted | Verified | Solver Time |
|------|-------|---------|----------|-------------|
| array_bounds.mm | 1 | 0 | PASS | 0.057s |
| integer_overflow.mm | 1 | 0 | PASS | 0.077s |
| loop_invariant.mm | 1 | 0 | PASS | 0.057s |

</details>

---

## Benchmark Run — 2026-07-06 13:30 UTC

### Stdlib Health Summary

| Modules | Atoms | Proven | Trusted | Trusted Ratio |
|---------|-------|--------|---------|---------------|
| 58 | 339 | 339 | 0 | 0.0000 |

### Category Results

| Category | Files | Atoms | Trusted | Success Rate | Avg Solver Time |
|----------|-------|-------|---------|--------------|-----------------|
| dafny_puzzles | 3 | 3 | 0 | 100.00% | 0.014s |
| svcomp_style | 3 | 3 | 0 | 100.00% | 0.014s |

<details><summary>Per-file details</summary>

#### dafny_puzzles

| File | Atoms | Trusted | Verified | Solver Time |
|------|-------|---------|----------|-------------|
| absolute_value.mm | 1 | 0 | PASS | 0.015s |
| max.mm | 1 | 0 | PASS | 0.014s |
| swap.mm | 1 | 0 | PASS | 0.014s |

#### svcomp_style

| File | Atoms | Trusted | Verified | Solver Time |
|------|-------|---------|----------|-------------|
| array_bounds.mm | 1 | 0 | PASS | 0.014s |
| integer_overflow.mm | 1 | 0 | PASS | 0.014s |
| loop_invariant.mm | 1 | 0 | PASS | 0.014s |

</details>

---

## Benchmark Run — 2026-07-26 10:52 UTC

### Stdlib Health Summary

| Modules | Atoms | Proven | Trusted | Trusted Ratio |
|---------|-------|--------|---------|---------------|
| 59 | 343 | 343 | 0 | 0.0000 |

### Category Results

Success Rate is the share of files whose verification outcome matched the
expected outcome (`expected: PASS` or `expected: FAIL`). Counterexample Catch
is the share of `expected: FAIL` files the verifier correctly rejected.

| Category | Files | Atoms | Trusted | Success Rate | Counterexample Catch | Avg Solver Time | Avg Lean Solver Time |
|----------|-------|-------|---------|--------------|----------------------|-----------------|----------------------|
| arithmetic | 8 | 23 | 0 | 100.00% | 100.00% (3/3) | 0.241s | SKIP |
| concurrency | 8 | 18 | 0 | 100.00% | 100.00% (4/4) | 0.110s | SKIP |
| dafny_puzzles | 3 | 3 | 0 | 100.00% | n/a (0/0) | 0.080s | SKIP |
| domain_compliance | 9 | 23 | 0 | 100.00% | 100.00% (4/4) | 0.147s | SKIP |
| state_machine | 6 | 14 | 0 | 100.00% | 100.00% (3/3) | 0.098s | SKIP |
| svcomp_style | 3 | 3 | 0 | 100.00% | n/a (0/0) | 0.063s | SKIP |

<details><summary>Per-file details</summary>

#### arithmetic

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time |
|------|-------|---------|----------|--------|-------|-------------|------------------|
| abs_min_int_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.057s | SKIP |
| bounded_arithmetic.mm | 4 | 0 | PASS | PASS | yes | 0.386s | SKIP |
| fixed_point_scaling.mm | 5 | 0 | PASS | PASS | yes | 0.267s | SKIP |
| nonlinear_polynomial.mm | 4 | 0 | PASS | PASS | yes | 0.514s | SKIP |
| off_by_one_index_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.059s | SKIP |
| overflow_boundary.mm | 4 | 0 | PASS | PASS | yes | 0.282s | SKIP |
| saturating.mm | 3 | 0 | PASS | PASS | yes | 0.286s | SKIP |
| unbounded_add_overflow_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.075s | SKIP |

#### concurrency

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time |
|------|-------|---------|----------|--------|-------|-------------|------------------|
| double_move_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.045s | SKIP |
| exclusive_resource_reuse_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.056s | SKIP |
| linear_ownership.mm | 4 | 0 | PASS | PASS | yes | 0.168s | SKIP |
| lock_order_inversion_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.057s | SKIP |
| resource_ordering.mm | 3 | 0 | PASS | PASS | yes | 0.159s | SKIP |
| task_group_all.mm | 4 | 0 | PASS | PASS | yes | 0.195s | SKIP |
| task_group_any_winner.mm | 3 | 0 | PASS | PASS | yes | 0.159s | SKIP |
| use_after_move_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.044s | SKIP |

#### dafny_puzzles

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time |
|------|-------|---------|----------|--------|-------|-------------|------------------|
| absolute_value.mm | 1 | 0 | PASS | PASS | yes | 0.076s | SKIP |
| max.mm | 1 | 0 | PASS | PASS | yes | 0.106s | SKIP |
| swap.mm | 1 | 0 | PASS | PASS | yes | 0.058s | SKIP |

#### domain_compliance

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time |
|------|-------|---------|----------|--------|-------|-------------|------------------|
| defi_invariants.mm | 4 | 0 | PASS | PASS | yes | 0.237s | SKIP |
| defi_reentrancy_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.047s | SKIP |
| medical_dosage.mm | 4 | 0 | PASS | PASS | yes | 0.339s | SKIP |
| medical_overdose_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.081s | SKIP |
| ownership_protocol.mm | 3 | 0 | PASS | PASS | yes | 0.118s | SKIP |
| regtech_exhaustiveness.mm | 4 | 0 | PASS | PASS | yes | 0.196s | SKIP |
| regtech_missing_pep_arm_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.076s | SKIP |
| rtgs_balance_break_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.065s | SKIP |
| rtgs_balance_conservation.mm | 4 | 0 | PASS | PASS | yes | 0.166s | SKIP |

#### state_machine

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time |
|------|-------|---------|----------|--------|-------|-------------|------------------|
| escrow_transfer.mm | 3 | 0 | PASS | PASS | yes | 0.110s | SKIP |
| order_lifecycle.mm | 4 | 0 | PASS | PASS | yes | 0.171s | SKIP |
| session_auth.mm | 4 | 0 | PASS | PASS | yes | 0.173s | SKIP |
| skip_ship_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.047s | SKIP |
| transfer_without_accept_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.043s | SKIP |
| unauthenticated_read_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.042s | SKIP |

#### svcomp_style

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time |
|------|-------|---------|----------|--------|-------|-------------|------------------|
| array_bounds.mm | 1 | 0 | PASS | PASS | yes | 0.056s | SKIP |
| integer_overflow.mm | 1 | 0 | PASS | PASS | yes | 0.076s | SKIP |
| loop_invariant.mm | 1 | 0 | PASS | PASS | yes | 0.058s | SKIP |

</details>

---

## Benchmark Run — 2026-07-26 10:55 UTC

### Stdlib Health Summary

| Modules | Atoms | Proven | Trusted | Trusted Ratio |
|---------|-------|--------|---------|---------------|
| 59 | 343 | 343 | 0 | 0.0000 |

### Category Results

Success Rate is the share of files whose verification outcome matched the
expected outcome (`expected: PASS` or `expected: FAIL`). Counterexample Catch
is the share of `expected: FAIL` files the verifier correctly rejected.

| Category | Files | Atoms | Trusted | Success Rate | Counterexample Catch | Avg Solver Time | Avg Lean Solver Time |
|----------|-------|-------|---------|--------------|----------------------|-----------------|----------------------|
| arithmetic | 8 | 23 | 0 | 100.00% | 100.00% (3/3) | 0.032s | SKIP |
| concurrency | 8 | 18 | 0 | 100.00% | 100.00% (4/4) | 0.031s | SKIP |
| dafny_puzzles | 3 | 3 | 0 | 100.00% | n/a (0/0) | 0.012s | SKIP |
| domain_compliance | 9 | 23 | 0 | 100.00% | 100.00% (4/4) | 0.039s | SKIP |
| state_machine | 6 | 14 | 0 | 100.00% | 100.00% (3/3) | 0.030s | SKIP |
| svcomp_style | 3 | 3 | 0 | 100.00% | n/a (0/0) | 0.013s | SKIP |

<details><summary>Per-file details</summary>

#### arithmetic

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time |
|------|-------|---------|----------|--------|-------|-------------|------------------|
| abs_min_int_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.057s | SKIP |
| bounded_arithmetic.mm | 4 | 0 | PASS | PASS | yes | 0.014s | SKIP |
| fixed_point_scaling.mm | 5 | 0 | PASS | PASS | yes | 0.014s | SKIP |
| nonlinear_polynomial.mm | 4 | 0 | PASS | PASS | yes | 0.013s | SKIP |
| off_by_one_index_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.057s | SKIP |
| overflow_boundary.mm | 4 | 0 | PASS | PASS | yes | 0.014s | SKIP |
| saturating.mm | 3 | 0 | PASS | PASS | yes | 0.013s | SKIP |
| unbounded_add_overflow_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.073s | SKIP |

#### concurrency

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time |
|------|-------|---------|----------|--------|-------|-------------|------------------|
| double_move_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.044s | SKIP |
| exclusive_resource_reuse_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.055s | SKIP |
| linear_ownership.mm | 4 | 0 | PASS | PASS | yes | 0.014s | SKIP |
| lock_order_inversion_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.054s | SKIP |
| resource_ordering.mm | 3 | 0 | PASS | PASS | yes | 0.014s | SKIP |
| task_group_all.mm | 4 | 0 | PASS | PASS | yes | 0.013s | SKIP |
| task_group_any_winner.mm | 3 | 0 | PASS | PASS | yes | 0.013s | SKIP |
| use_after_move_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.043s | SKIP |

#### dafny_puzzles

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time |
|------|-------|---------|----------|--------|-------|-------------|------------------|
| absolute_value.mm | 1 | 0 | PASS | PASS | yes | 0.013s | SKIP |
| max.mm | 1 | 0 | PASS | PASS | yes | 0.012s | SKIP |
| swap.mm | 1 | 0 | PASS | PASS | yes | 0.012s | SKIP |

#### domain_compliance

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time |
|------|-------|---------|----------|--------|-------|-------------|------------------|
| defi_invariants.mm | 4 | 0 | PASS | PASS | yes | 0.014s | SKIP |
| defi_reentrancy_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.045s | SKIP |
| medical_dosage.mm | 4 | 0 | PASS | PASS | yes | 0.015s | SKIP |
| medical_overdose_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.081s | SKIP |
| ownership_protocol.mm | 3 | 0 | PASS | PASS | yes | 0.015s | SKIP |
| regtech_exhaustiveness.mm | 4 | 0 | PASS | PASS | yes | 0.015s | SKIP |
| regtech_missing_pep_arm_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.079s | SKIP |
| rtgs_balance_break_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.063s | SKIP |
| rtgs_balance_conservation.mm | 4 | 0 | PASS | PASS | yes | 0.020s | SKIP |

#### state_machine

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time |
|------|-------|---------|----------|--------|-------|-------------|------------------|
| escrow_transfer.mm | 3 | 0 | PASS | PASS | yes | 0.014s | SKIP |
| order_lifecycle.mm | 4 | 0 | PASS | PASS | yes | 0.015s | SKIP |
| session_auth.mm | 4 | 0 | PASS | PASS | yes | 0.014s | SKIP |
| skip_ship_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.047s | SKIP |
| transfer_without_accept_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.046s | SKIP |
| unauthenticated_read_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.045s | SKIP |

#### svcomp_style

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time |
|------|-------|---------|----------|--------|-------|-------------|------------------|
| array_bounds.mm | 1 | 0 | PASS | PASS | yes | 0.013s | SKIP |
| integer_overflow.mm | 1 | 0 | PASS | PASS | yes | 0.012s | SKIP |
| loop_invariant.mm | 1 | 0 | PASS | PASS | yes | 0.013s | SKIP |

</details>
