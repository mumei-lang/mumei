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

---

## Benchmark Run — 2026-07-26 15:58 UTC

### Stdlib Health Summary

| Modules | Atoms | Proven | Trusted | Trusted Ratio |
|---------|-------|--------|---------|---------------|
| 59 | 344 | 344 | 0 | 0.0000 |

### Category Results

Success Rate is the share of files whose verification outcome matched the
expected outcome (`expected: PASS` or `expected: FAIL`). Counterexample Catch
is the share of `expected: FAIL` files the verifier correctly rejected. Lean
Discharge is the share of escalated (Z3 `unknown`) obligations the mumei-lean
bridge returned as `lean_verified`; the parenthesised count is how many of them
the automatic tactic search discharged.

| Category | Files | Atoms | Trusted | Success Rate | Counterexample Catch | Avg Solver Time | Avg Lean Solver Time | Lean Discharge | Tactic Search |
|----------|-------|-------|---------|--------------|----------------------|-----------------|----------------------|----------------|---------------|
| arithmetic | 9 | 27 | 0 | 100.00% | 100.00% (3/3) | 0.045s | 9.014s | 100.00% (4/4) | 2 |
| concurrency | 8 | 18 | 0 | 100.00% | 100.00% (4/4) | 0.043s | SKIP | n/a (0/0) | 0 |
| dafny_puzzles | 3 | 3 | 0 | 100.00% | n/a (0/0) | 0.016s | SKIP | n/a (0/0) | 0 |
| domain_compliance | 10 | 25 | 0 | 100.00% | 100.00% (4/4) | 0.040s | 11.364s | 100.00% (2/2) | 2 |
| state_machine | 6 | 14 | 0 | 100.00% | 100.00% (3/3) | 0.030s | SKIP | n/a (0/0) | 0 |
| svcomp_style | 3 | 3 | 0 | 100.00% | n/a (0/0) | 0.013s | SKIP | n/a (0/0) | 0 |

<details><summary>Per-file details</summary>

#### arithmetic

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time | Escalated | lean_verified | Tactic Search |
|------|-------|---------|----------|--------|-------|-------------|------------------|-----------|---------------|---------------|
| abs_min_int_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.064s | SKIP | 0 | 0 | 0 |
| bounded_arithmetic.mm | 4 | 0 | PASS | PASS | yes | 0.015s | SKIP | 0 | 0 | 0 |
| finite_field_modular.mm | 4 | 0 | PASS | PASS | yes | 0.013s | 9.014s | 4 | 4 | 2 |
| fixed_point_scaling.mm | 5 | 0 | PASS | PASS | yes | 0.030s | SKIP | 0 | 0 | 0 |
| nonlinear_polynomial.mm | 4 | 0 | PASS | PASS | yes | 0.020s | SKIP | 0 | 0 | 0 |
| off_by_one_index_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.105s | SKIP | 0 | 0 | 0 |
| overflow_boundary.mm | 4 | 0 | PASS | PASS | yes | 0.024s | SKIP | 0 | 0 | 0 |
| saturating.mm | 3 | 0 | PASS | PASS | yes | 0.020s | SKIP | 0 | 0 | 0 |
| unbounded_add_overflow_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.112s | SKIP | 0 | 0 | 0 |

#### concurrency

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time | Escalated | lean_verified | Tactic Search |
|------|-------|---------|----------|--------|-------|-------------|------------------|-----------|---------------|---------------|
| double_move_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.067s | SKIP | 0 | 0 | 0 |
| exclusive_resource_reuse_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.073s | SKIP | 0 | 0 | 0 |
| linear_ownership.mm | 4 | 0 | PASS | PASS | yes | 0.017s | SKIP | 0 | 0 | 0 |
| lock_order_inversion_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.061s | SKIP | 0 | 0 | 0 |
| resource_ordering.mm | 3 | 0 | PASS | PASS | yes | 0.015s | SKIP | 0 | 0 | 0 |
| task_group_all.mm | 4 | 0 | PASS | PASS | yes | 0.019s | SKIP | 0 | 0 | 0 |
| task_group_any_winner.mm | 3 | 0 | PASS | PASS | yes | 0.021s | SKIP | 0 | 0 | 0 |
| use_after_move_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.074s | SKIP | 0 | 0 | 0 |

#### dafny_puzzles

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time | Escalated | lean_verified | Tactic Search |
|------|-------|---------|----------|--------|-------|-------------|------------------|-----------|---------------|---------------|
| absolute_value.mm | 1 | 0 | PASS | PASS | yes | 0.017s | SKIP | 0 | 0 | 0 |
| max.mm | 1 | 0 | PASS | PASS | yes | 0.015s | SKIP | 0 | 0 | 0 |
| swap.mm | 1 | 0 | PASS | PASS | yes | 0.016s | SKIP | 0 | 0 | 0 |

#### domain_compliance

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time | Escalated | lean_verified | Tactic Search |
|------|-------|---------|----------|--------|-------|-------------|------------------|-----------|---------------|---------------|
| defi_invariants.mm | 4 | 0 | PASS | PASS | yes | 0.018s | SKIP | 0 | 0 | 0 |
| defi_reentrancy_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.060s | SKIP | 0 | 0 | 0 |
| medical_dosage.mm | 4 | 0 | PASS | PASS | yes | 0.016s | SKIP | 0 | 0 | 0 |
| medical_overdose_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.089s | SKIP | 0 | 0 | 0 |
| modular_commitment.mm | 2 | 0 | PASS | PASS | yes | 0.015s | 11.364s | 2 | 2 | 2 |
| ownership_protocol.mm | 3 | 0 | PASS | PASS | yes | 0.015s | SKIP | 0 | 0 | 0 |
| regtech_exhaustiveness.mm | 4 | 0 | PASS | PASS | yes | 0.014s | SKIP | 0 | 0 | 0 |
| regtech_missing_pep_arm_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.079s | SKIP | 0 | 0 | 0 |
| rtgs_balance_break_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.078s | SKIP | 0 | 0 | 0 |
| rtgs_balance_conservation.mm | 4 | 0 | PASS | PASS | yes | 0.018s | SKIP | 0 | 0 | 0 |

#### state_machine

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time | Escalated | lean_verified | Tactic Search |
|------|-------|---------|----------|--------|-------|-------------|------------------|-----------|---------------|---------------|
| escrow_transfer.mm | 3 | 0 | PASS | PASS | yes | 0.016s | SKIP | 0 | 0 | 0 |
| order_lifecycle.mm | 4 | 0 | PASS | PASS | yes | 0.015s | SKIP | 0 | 0 | 0 |
| session_auth.mm | 4 | 0 | PASS | PASS | yes | 0.014s | SKIP | 0 | 0 | 0 |
| skip_ship_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.045s | SKIP | 0 | 0 | 0 |
| transfer_without_accept_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.045s | SKIP | 0 | 0 | 0 |
| unauthenticated_read_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.045s | SKIP | 0 | 0 | 0 |

#### svcomp_style

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time | Escalated | lean_verified | Tactic Search |
|------|-------|---------|----------|--------|-------|-------------|------------------|-----------|---------------|---------------|
| array_bounds.mm | 1 | 0 | PASS | PASS | yes | 0.013s | SKIP | 0 | 0 | 0 |
| integer_overflow.mm | 1 | 0 | PASS | PASS | yes | 0.013s | SKIP | 0 | 0 | 0 |
| loop_invariant.mm | 1 | 0 | PASS | PASS | yes | 0.013s | SKIP | 0 | 0 | 0 |

</details>

---

## Benchmark Run — 2026-07-27 06:37 UTC

### Stdlib Health Summary

| Modules | Atoms | Proven | Trusted | Trusted Ratio |
|---------|-------|--------|---------|---------------|
| 59 | 344 | 344 | 0 | 0.0000 |

### Category Results

Success Rate is the share of files whose verification outcome matched the
expected outcome (`expected: PASS` or `expected: FAIL`). Counterexample Catch
is the share of `expected: FAIL` files the verifier correctly rejected. Lean
Discharge is the share of escalated (Z3 `unknown`) obligations the mumei-lean
bridge returned as `lean_verified`; the parenthesised count is how many of them
the automatic tactic search discharged.

| Category | Files | Atoms | Trusted | Success Rate | Counterexample Catch | Avg Solver Time | Avg Lean Solver Time | Lean Discharge | Tactic Search |
|----------|-------|-------|---------|--------------|----------------------|-----------------|----------------------|----------------|---------------|
| arithmetic | 9 | 27 | 0 | 100.00% | 100.00% (3/3) | 0.214s | SKIP | n/a (0/0) | 0 |
| concurrency | 14 | 30 | 0 | 100.00% | 100.00% (9/9) | 0.098s | SKIP | n/a (0/0) | 0 |
| dafny_puzzles | 3 | 3 | 0 | 100.00% | n/a (0/0) | 0.082s | SKIP | n/a (0/0) | 0 |
| domain_compliance | 10 | 25 | 0 | 100.00% | 100.00% (4/4) | 0.136s | SKIP | n/a (0/0) | 0 |
| state_machine | 6 | 14 | 0 | 100.00% | 100.00% (3/3) | 0.104s | SKIP | n/a (0/0) | 0 |
| svcomp_style | 3 | 3 | 0 | 100.00% | n/a (0/0) | 0.066s | SKIP | n/a (0/0) | 0 |

<details><summary>Per-file details</summary>

#### arithmetic

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time | Escalated | lean_verified | Tactic Search |
|------|-------|---------|----------|--------|-------|-------------|------------------|-----------|---------------|---------------|
| abs_min_int_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.062s | SKIP | 0 | 0 | 0 |
| bounded_arithmetic.mm | 4 | 0 | PASS | PASS | yes | 0.400s | SKIP | 0 | 0 | 0 |
| finite_field_modular.mm | 4 | 0 | PASS | PASS | yes | 0.014s | SKIP | 4 | 0 | 0 |
| fixed_point_scaling.mm | 5 | 0 | PASS | PASS | yes | 0.261s | SKIP | 0 | 0 | 0 |
| nonlinear_polynomial.mm | 4 | 0 | PASS | PASS | yes | 0.515s | SKIP | 0 | 0 | 0 |
| off_by_one_index_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.062s | SKIP | 0 | 0 | 0 |
| overflow_boundary.mm | 4 | 0 | PASS | PASS | yes | 0.276s | SKIP | 0 | 0 | 0 |
| saturating.mm | 3 | 0 | PASS | PASS | yes | 0.262s | SKIP | 0 | 0 | 0 |
| unbounded_add_overflow_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.075s | SKIP | 0 | 0 | 0 |

#### concurrency

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time | Escalated | lean_verified | Tactic Search |
|------|-------|---------|----------|--------|-------|-------------|------------------|-----------|---------------|---------------|
| double_move_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.045s | SKIP | 0 | 0 | 0 |
| exclusive_resource_reuse_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.057s | SKIP | 0 | 0 | 0 |
| linear_ownership.mm | 4 | 0 | PASS | PASS | yes | 0.168s | SKIP | 0 | 0 | 0 |
| lock_order_inversion_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.057s | SKIP | 0 | 0 | 0 |
| resource_ordering.mm | 3 | 0 | PASS | PASS | yes | 0.166s | SKIP | 0 | 0 | 0 |
| task_cancel_dependent_read_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.063s | SKIP | 0 | 0 | 0 |
| task_concurrent_double_move_fail.mm | 2 | 0 | FAIL | FAIL | yes | 0.083s | SKIP | 0 | 0 | 0 |
| task_group_all.mm | 4 | 0 | PASS | PASS | yes | 0.197s | SKIP | 0 | 0 | 0 |
| task_group_any_winner.mm | 3 | 0 | PASS | PASS | yes | 0.160s | SKIP | 0 | 0 | 0 |
| task_move_while_sibling_reads_fail.mm | 2 | 0 | FAIL | FAIL | yes | 0.046s | SKIP | 0 | 0 | 0 |
| task_ownership.mm | 4 | 0 | PASS | PASS | yes | 0.176s | SKIP | 0 | 0 | 0 |
| task_shared_write_race_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.061s | SKIP | 0 | 0 | 0 |
| task_use_after_concurrent_move_fail.mm | 2 | 0 | FAIL | FAIL | yes | 0.047s | SKIP | 0 | 0 | 0 |
| use_after_move_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.044s | SKIP | 0 | 0 | 0 |

#### dafny_puzzles

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time | Escalated | lean_verified | Tactic Search |
|------|-------|---------|----------|--------|-------|-------------|------------------|-----------|---------------|---------------|
| absolute_value.mm | 1 | 0 | PASS | PASS | yes | 0.077s | SKIP | 0 | 0 | 0 |
| max.mm | 1 | 0 | PASS | PASS | yes | 0.111s | SKIP | 0 | 0 | 0 |
| swap.mm | 1 | 0 | PASS | PASS | yes | 0.058s | SKIP | 0 | 0 | 0 |

#### domain_compliance

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time | Escalated | lean_verified | Tactic Search |
|------|-------|---------|----------|--------|-------|-------------|------------------|-----------|---------------|---------------|
| defi_invariants.mm | 4 | 0 | PASS | PASS | yes | 0.232s | SKIP | 0 | 0 | 0 |
| defi_reentrancy_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.048s | SKIP | 0 | 0 | 0 |
| medical_dosage.mm | 4 | 0 | PASS | PASS | yes | 0.357s | SKIP | 0 | 0 | 0 |
| medical_overdose_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.079s | SKIP | 0 | 0 | 0 |
| modular_commitment.mm | 2 | 0 | PASS | PASS | yes | 0.013s | SKIP | 2 | 0 | 0 |
| ownership_protocol.mm | 3 | 0 | PASS | PASS | yes | 0.115s | SKIP | 0 | 0 | 0 |
| regtech_exhaustiveness.mm | 4 | 0 | PASS | PASS | yes | 0.201s | SKIP | 0 | 0 | 0 |
| regtech_missing_pep_arm_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.076s | SKIP | 0 | 0 | 0 |
| rtgs_balance_break_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.063s | SKIP | 0 | 0 | 0 |
| rtgs_balance_conservation.mm | 4 | 0 | PASS | PASS | yes | 0.180s | SKIP | 0 | 0 | 0 |

#### state_machine

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time | Escalated | lean_verified | Tactic Search |
|------|-------|---------|----------|--------|-------|-------------|------------------|-----------|---------------|---------------|
| escrow_transfer.mm | 3 | 0 | PASS | PASS | yes | 0.139s | SKIP | 0 | 0 | 0 |
| order_lifecycle.mm | 4 | 0 | PASS | PASS | yes | 0.170s | SKIP | 0 | 0 | 0 |
| session_auth.mm | 4 | 0 | PASS | PASS | yes | 0.170s | SKIP | 0 | 0 | 0 |
| skip_ship_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.047s | SKIP | 0 | 0 | 0 |
| transfer_without_accept_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.044s | SKIP | 0 | 0 | 0 |
| unauthenticated_read_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.051s | SKIP | 0 | 0 | 0 |

#### svcomp_style

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time | Escalated | lean_verified | Tactic Search |
|------|-------|---------|----------|--------|-------|-------------|------------------|-----------|---------------|---------------|
| array_bounds.mm | 1 | 0 | PASS | PASS | yes | 0.061s | SKIP | 0 | 0 | 0 |
| integer_overflow.mm | 1 | 0 | PASS | PASS | yes | 0.077s | SKIP | 0 | 0 | 0 |
| loop_invariant.mm | 1 | 0 | PASS | PASS | yes | 0.061s | SKIP | 0 | 0 | 0 |

</details>

---

## Benchmark Run — 2026-07-27 09:00 UTC

### Stdlib Health Summary

| Modules | Atoms | Proven | Trusted | Trusted Ratio |
|---------|-------|--------|---------|---------------|
| 59 | 344 | 344 | 0 | 0.0000 |

### Category Results

Success Rate is the share of files whose verification outcome matched the
expected outcome (`expected: PASS` or `expected: FAIL`). Counterexample Catch
is the share of `expected: FAIL` files the verifier correctly rejected. Lean
Discharge is the share of escalated (Z3 `unknown`) obligations the mumei-lean
bridge returned as `lean_verified`; the parenthesised count is how many of them
the automatic tactic search discharged.

| Category | Files | Atoms | Trusted | Success Rate | Counterexample Catch | Avg Solver Time | Avg Lean Solver Time | Lean Discharge | Tactic Search |
|----------|-------|-------|---------|--------------|----------------------|-----------------|----------------------|----------------|---------------|
| arithmetic | 9 | 27 | 0 | 100.00% | 100.00% (3/3) | 0.218s | SKIP | n/a (0/0) | 0 |
| concurrency | 15 | 33 | 0 | 100.00% | 100.00% (10/10) | 0.100s | SKIP | n/a (0/0) | 0 |
| dafny_puzzles | 3 | 3 | 0 | 100.00% | n/a (0/0) | 0.084s | SKIP | n/a (0/0) | 0 |
| domain_compliance | 10 | 25 | 0 | 100.00% | 100.00% (4/4) | 0.135s | SKIP | n/a (0/0) | 0 |
| state_machine | 6 | 14 | 0 | 100.00% | 100.00% (3/3) | 0.099s | SKIP | n/a (0/0) | 0 |
| svcomp_style | 3 | 3 | 0 | 100.00% | n/a (0/0) | 0.071s | SKIP | n/a (0/0) | 0 |

<details><summary>Per-file details</summary>

#### arithmetic

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time | Escalated | lean_verified | Tactic Search |
|------|-------|---------|----------|--------|-------|-------------|------------------|-----------|---------------|---------------|
| abs_min_int_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.066s | SKIP | 0 | 0 | 0 |
| bounded_arithmetic.mm | 4 | 0 | PASS | PASS | yes | 0.407s | SKIP | 0 | 0 | 0 |
| finite_field_modular.mm | 4 | 0 | PASS | PASS | yes | 0.014s | SKIP | 4 | 0 | 0 |
| fixed_point_scaling.mm | 5 | 0 | PASS | PASS | yes | 0.266s | SKIP | 0 | 0 | 0 |
| nonlinear_polynomial.mm | 4 | 0 | PASS | PASS | yes | 0.521s | SKIP | 0 | 0 | 0 |
| off_by_one_index_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.061s | SKIP | 0 | 0 | 0 |
| overflow_boundary.mm | 4 | 0 | PASS | PASS | yes | 0.277s | SKIP | 0 | 0 | 0 |
| saturating.mm | 3 | 0 | PASS | PASS | yes | 0.272s | SKIP | 0 | 0 | 0 |
| unbounded_add_overflow_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.082s | SKIP | 0 | 0 | 0 |

#### concurrency

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time | Escalated | lean_verified | Tactic Search |
|------|-------|---------|----------|--------|-------|-------------|------------------|-----------|---------------|---------------|
| double_move_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.046s | SKIP | 0 | 0 | 0 |
| exclusive_resource_reuse_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.056s | SKIP | 0 | 0 | 0 |
| linear_ownership.mm | 4 | 0 | PASS | PASS | yes | 0.174s | SKIP | 0 | 0 | 0 |
| lock_order_inversion_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.060s | SKIP | 0 | 0 | 0 |
| resource_ordering.mm | 3 | 0 | PASS | PASS | yes | 0.175s | SKIP | 0 | 0 | 0 |
| task_cancel_dependent_read_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.060s | SKIP | 0 | 0 | 0 |
| task_concurrent_double_move_fail.mm | 2 | 0 | FAIL | FAIL | yes | 0.082s | SKIP | 0 | 0 | 0 |
| task_group_all.mm | 4 | 0 | PASS | PASS | yes | 0.196s | SKIP | 0 | 0 | 0 |
| task_group_any_winner.mm | 3 | 0 | PASS | PASS | yes | 0.162s | SKIP | 0 | 0 | 0 |
| task_move_while_sibling_reads_fail.mm | 2 | 0 | FAIL | FAIL | yes | 0.046s | SKIP | 0 | 0 | 0 |
| task_ownership.mm | 5 | 0 | PASS | PASS | yes | 0.215s | SKIP | 0 | 0 | 0 |
| task_shared_write_race_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.061s | SKIP | 0 | 0 | 0 |
| task_struct_capture_double_move_fail.mm | 2 | 0 | FAIL | FAIL | yes | 0.083s | SKIP | 0 | 0 | 0 |
| task_use_after_concurrent_move_fail.mm | 2 | 0 | FAIL | FAIL | yes | 0.046s | SKIP | 0 | 0 | 0 |
| use_after_move_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.044s | SKIP | 0 | 0 | 0 |

#### dafny_puzzles

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time | Escalated | lean_verified | Tactic Search |
|------|-------|---------|----------|--------|-------|-------------|------------------|-----------|---------------|---------------|
| absolute_value.mm | 1 | 0 | PASS | PASS | yes | 0.088s | SKIP | 0 | 0 | 0 |
| max.mm | 1 | 0 | PASS | PASS | yes | 0.108s | SKIP | 0 | 0 | 0 |
| swap.mm | 1 | 0 | PASS | PASS | yes | 0.056s | SKIP | 0 | 0 | 0 |

#### domain_compliance

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time | Escalated | lean_verified | Tactic Search |
|------|-------|---------|----------|--------|-------|-------------|------------------|-----------|---------------|---------------|
| defi_invariants.mm | 4 | 0 | PASS | PASS | yes | 0.233s | SKIP | 0 | 0 | 0 |
| defi_reentrancy_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.051s | SKIP | 0 | 0 | 0 |
| medical_dosage.mm | 4 | 0 | PASS | PASS | yes | 0.347s | SKIP | 0 | 0 | 0 |
| medical_overdose_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.084s | SKIP | 0 | 0 | 0 |
| modular_commitment.mm | 2 | 0 | PASS | PASS | yes | 0.014s | SKIP | 2 | 0 | 0 |
| ownership_protocol.mm | 3 | 0 | PASS | PASS | yes | 0.112s | SKIP | 0 | 0 | 0 |
| regtech_exhaustiveness.mm | 4 | 0 | PASS | PASS | yes | 0.196s | SKIP | 0 | 0 | 0 |
| regtech_missing_pep_arm_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.078s | SKIP | 0 | 0 | 0 |
| rtgs_balance_break_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.064s | SKIP | 0 | 0 | 0 |
| rtgs_balance_conservation.mm | 4 | 0 | PASS | PASS | yes | 0.167s | SKIP | 0 | 0 | 0 |

#### state_machine

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time | Escalated | lean_verified | Tactic Search |
|------|-------|---------|----------|--------|-------|-------------|------------------|-----------|---------------|---------------|
| escrow_transfer.mm | 3 | 0 | PASS | PASS | yes | 0.134s | SKIP | 0 | 0 | 0 |
| order_lifecycle.mm | 4 | 0 | PASS | PASS | yes | 0.139s | SKIP | 0 | 0 | 0 |
| session_auth.mm | 4 | 0 | PASS | PASS | yes | 0.178s | SKIP | 0 | 0 | 0 |
| skip_ship_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.048s | SKIP | 0 | 0 | 0 |
| transfer_without_accept_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.048s | SKIP | 0 | 0 | 0 |
| unauthenticated_read_fail.mm | 1 | 0 | FAIL | FAIL | yes | 0.049s | SKIP | 0 | 0 | 0 |

#### svcomp_style

| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time | Escalated | lean_verified | Tactic Search |
|------|-------|---------|----------|--------|-------|-------------|------------------|-----------|---------------|---------------|
| array_bounds.mm | 1 | 0 | PASS | PASS | yes | 0.062s | SKIP | 0 | 0 | 0 |
| integer_overflow.mm | 1 | 0 | PASS | PASS | yes | 0.083s | SKIP | 0 | 0 | 0 |
| loop_invariant.mm | 1 | 0 | PASS | PASS | yes | 0.067s | SKIP | 0 | 0 | 0 |

</details>

## Scale Composability Run — 2026-08-28 (Priority 16)

Measured outside the benchmark harness by `scripts/measure_composability.py`
and `scripts/scale_trust_surface.py` over the five `*_scale` scenarios in
`mumei-lang/mumei-demo` (`make demo-scale`). Artifacts:
`benchmarks/composability/scale_composability.json` and
`benchmarks/composability/scale_trust_surface.json`.
`budget_policy_fingerprint`: `sha256:scale-default`.

### Scale and Trust Surface

| Case | Atoms | Depth | Certified | `verify-cert --strict` | App trusted | FFI | Z3 unknown → Lean | Z3 solver (s) |
|------|------:|------:|----------:|:----------------------:|------------:|----:|------------------:|--------------:|
| medical_device_scale | 34 | 7 | 34 | PASS | 0 | 0 | 0 | 2.391 |
| rtgs_settlement_scale | 30 | 5 | 30 | PASS | 0 | 0 | 0 | 1.317 |
| regtech_compliance_scale | 41 | 7 | 41 | PASS | 0 | 0 | 0 | 2.105 |
| defi_invariant_scale | 32 | 5 | 32 | PASS | 0 | 0 | 0 | 1.987 |
| ownership_transfer_scale | 35 | 6 | 35 | PASS | 0 | 0 | 0 | 1.970 |
| **total** | 172 | — | 172 | 5/5 | 0 | 0 | 0 | 9.77 |

`std/` trusted atoms remain 0 of 344.

### Atom-Local Composability (clause ablation)

Each `requires` / `ensures` / `effect_pre` / `effect_post` clause is removed in
turn and the case re-verified. A clause is *atom-local* when only its owning
atom fails, a *composition break* when a neighbouring atom fails too.

| Case | Probed clauses | Atom-local | Composition breaks | Slack | Closure ratio | Whole-system invariants | Neighbour-dependent |
|------|---------------:|-----------:|-------------------:|------:|--------------:|------------------------:|--------------------:|
| medical_device_scale | 196 | 62 | 62 | 72 | 0.5000 | 3 | 2 |
| rtgs_settlement_scale | 187 | 44 | 47 | 96 | 0.4835 | 4 | 3 |
| regtech_compliance_scale | 227 | 64 | 66 | 97 | 0.4923 | 2 | 1 |
| defi_invariant_scale | 190 | 40 | 48 | 102 | 0.4545 | 2 | 1 |
| ownership_transfer_scale | 214 | 61 | 54 | 99 | 0.5304 | 5 | 2 |
| **total** | 1014 | 271 | 277 | 466 | **0.4945** | 16 | 9 |

All 16 whole-system invariants close from declared atom contracts alone; 9 of
them stop closing if a neighbouring atom's contract is weakened.

### Composition-Break Patterns (modular verification inputs)

| Pattern | Count | Compiler surface |
|---------|------:|------------------|
| `call_site_precondition` | 86 | call-site `requires` propagation |
| `counterexample_replay_mismatch` | 86 | Z3 translation / Lean escalation path |
| `effect_state_obligation` | 58 | `effect_pre` / `effect_post` state chaining (Plan 24) |
| `neighbor_ensures_strengthening` | 47 | value contracts (`ensures`) of called atoms |

---

## Benchmark Run — 2026-08-30 06:19 UTC

### Stdlib Health Summary

| Modules | Atoms | Proven | Trusted | Trusted Ratio |
|---------|-------|--------|---------|---------------|
| 59 | 344 | 344 | 0 | 0.0000 |

### Category Results

Success Rate is the share of files whose verification outcome matched the
expected outcome (`expected: PASS` or `expected: FAIL`). Counterexample Catch
is the share of `expected: FAIL` files the verifier correctly rejected; a run
that never reached a verdict (timeout, unreadable input, crash) is reported under
No Verdict and never counted as a catch. Lean
Discharge is the share of escalated (Z3 `unknown`) obligations the mumei-lean
bridge returned as `lean_verified`; the parenthesised count is how many of them
the automatic tactic search discharged.

| Category | Files | Atoms | Trusted | Success Rate | Counterexample Catch | No Verdict | Avg Solver Time | Avg Lean Solver Time | Lean Discharge | Tactic Search |
|----------|-------|-------|---------|--------------|----------------------|------------|-----------------|----------------------|----------------|---------------|
| arithmetic | 9 | 27 | 0 | 100.00% | 100.00% (3/3) | 0 | 0.022s | SKIP | n/a (0/0) | 0 |
| concurrency | 15 | 33 | 0 | 100.00% | 100.00% (10/10) | 0 | 0.029s | SKIP | n/a (0/0) | 0 |
| dafny_puzzles | 3 | 3 | 0 | 100.00% | n/a (0/0) | 0 | 0.007s | SKIP | n/a (0/0) | 0 |
| domain_compliance | 10 | 25 | 0 | 100.00% | 100.00% (4/4) | 0 | 0.025s | SKIP | n/a (0/0) | 0 |
| state_machine | 6 | 14 | 0 | 100.00% | 100.00% (3/3) | 0 | 0.019s | SKIP | n/a (0/0) | 0 |
| svcomp_style | 3 | 3 | 0 | 100.00% | n/a (0/0) | 0 | 0.007s | SKIP | n/a (0/0) | 0 |

<details><summary>Per-file details</summary>

#### arithmetic

| File | Atoms | Trusted | Expected | Actual | Match | Verify Status | Solver Time | Lean Solver Time | Escalated | lean_verified | Tactic Search |
|------|-------|---------|----------|--------|-------|---------------|-------------|------------------|-----------|---------------|---------------|
| abs_min_int_fail.mm | 1 | 0 | FAIL | FAIL | yes | MEASURED | 0.044s | SKIP | 0 | 0 | 0 |
| bounded_arithmetic.mm | 4 | 0 | PASS | PASS | yes | MEASURED | 0.009s | SKIP | 0 | 0 | 0 |
| finite_field_modular.mm | 4 | 0 | PASS | PASS | yes | MEASURED | 0.008s | SKIP | 4 | 0 | 0 |
| fixed_point_scaling.mm | 5 | 0 | PASS | PASS | yes | MEASURED | 0.008s | SKIP | 0 | 0 | 0 |
| nonlinear_polynomial.mm | 4 | 0 | PASS | PASS | yes | MEASURED | 0.008s | SKIP | 0 | 0 | 0 |
| off_by_one_index_fail.mm | 1 | 0 | FAIL | FAIL | yes | MEASURED | 0.051s | SKIP | 0 | 0 | 0 |
| overflow_boundary.mm | 4 | 0 | PASS | PASS | yes | MEASURED | 0.009s | SKIP | 0 | 0 | 0 |
| saturating.mm | 3 | 0 | PASS | PASS | yes | MEASURED | 0.008s | SKIP | 0 | 0 | 0 |
| unbounded_add_overflow_fail.mm | 1 | 0 | FAIL | FAIL | yes | MEASURED | 0.054s | SKIP | 0 | 0 | 0 |

#### concurrency

| File | Atoms | Trusted | Expected | Actual | Match | Verify Status | Solver Time | Lean Solver Time | Escalated | lean_verified | Tactic Search |
|------|-------|---------|----------|--------|-------|---------------|-------------|------------------|-----------|---------------|---------------|
| double_move_fail.mm | 1 | 0 | FAIL | FAIL | yes | MEASURED | 0.033s | SKIP | 0 | 0 | 0 |
| exclusive_resource_reuse_fail.mm | 1 | 0 | FAIL | FAIL | yes | MEASURED | 0.040s | SKIP | 0 | 0 | 0 |
| linear_ownership.mm | 4 | 0 | PASS | PASS | yes | MEASURED | 0.009s | SKIP | 0 | 0 | 0 |
| lock_order_inversion_fail.mm | 1 | 0 | FAIL | FAIL | yes | MEASURED | 0.040s | SKIP | 0 | 0 | 0 |
| resource_ordering.mm | 3 | 0 | PASS | PASS | yes | MEASURED | 0.009s | SKIP | 0 | 0 | 0 |
| task_cancel_dependent_read_fail.mm | 1 | 0 | FAIL | FAIL | yes | MEASURED | 0.042s | SKIP | 0 | 0 | 0 |
| task_concurrent_double_move_fail.mm | 2 | 0 | FAIL | FAIL | yes | MEASURED | 0.062s | SKIP | 0 | 0 | 0 |
| task_group_all.mm | 4 | 0 | PASS | PASS | yes | MEASURED | 0.009s | SKIP | 0 | 0 | 0 |
| task_group_any_winner.mm | 3 | 0 | PASS | PASS | yes | MEASURED | 0.008s | SKIP | 0 | 0 | 0 |
| task_move_while_sibling_reads_fail.mm | 2 | 0 | FAIL | FAIL | yes | MEASURED | 0.032s | SKIP | 0 | 0 | 0 |
| task_ownership.mm | 5 | 0 | PASS | PASS | yes | MEASURED | 0.008s | SKIP | 0 | 0 | 0 |
| task_shared_write_race_fail.mm | 1 | 0 | FAIL | FAIL | yes | MEASURED | 0.042s | SKIP | 0 | 0 | 0 |
| task_struct_capture_double_move_fail.mm | 2 | 0 | FAIL | FAIL | yes | MEASURED | 0.034s | SKIP | 0 | 0 | 0 |
| task_use_after_concurrent_move_fail.mm | 2 | 0 | FAIL | FAIL | yes | MEASURED | 0.040s | SKIP | 0 | 0 | 0 |
| use_after_move_fail.mm | 1 | 0 | FAIL | FAIL | yes | MEASURED | 0.032s | SKIP | 0 | 0 | 0 |

#### dafny_puzzles

| File | Atoms | Trusted | Expected | Actual | Match | Verify Status | Solver Time | Lean Solver Time | Escalated | lean_verified | Tactic Search |
|------|-------|---------|----------|--------|-------|---------------|-------------|------------------|-----------|---------------|---------------|
| absolute_value.mm | 1 | 0 | PASS | PASS | yes | MEASURED | 0.008s | SKIP | 0 | 0 | 0 |
| max.mm | 1 | 0 | PASS | PASS | yes | MEASURED | 0.007s | SKIP | 0 | 0 | 0 |
| swap.mm | 1 | 0 | PASS | PASS | yes | MEASURED | 0.007s | SKIP | 0 | 0 | 0 |

#### domain_compliance

| File | Atoms | Trusted | Expected | Actual | Match | Verify Status | Solver Time | Lean Solver Time | Escalated | lean_verified | Tactic Search |
|------|-------|---------|----------|--------|-------|---------------|-------------|------------------|-----------|---------------|---------------|
| defi_invariants.mm | 4 | 0 | PASS | PASS | yes | MEASURED | 0.009s | SKIP | 0 | 0 | 0 |
| defi_reentrancy_fail.mm | 1 | 0 | FAIL | FAIL | yes | MEASURED | 0.034s | SKIP | 0 | 0 | 0 |
| medical_dosage.mm | 4 | 0 | PASS | PASS | yes | MEASURED | 0.009s | SKIP | 0 | 0 | 0 |
| medical_overdose_fail.mm | 1 | 0 | FAIL | FAIL | yes | MEASURED | 0.064s | SKIP | 0 | 0 | 0 |
| modular_commitment.mm | 2 | 0 | PASS | PASS | yes | MEASURED | 0.008s | SKIP | 2 | 0 | 0 |
| ownership_protocol.mm | 3 | 0 | PASS | PASS | yes | MEASURED | 0.008s | SKIP | 0 | 0 | 0 |
| regtech_exhaustiveness.mm | 4 | 0 | PASS | PASS | yes | MEASURED | 0.008s | SKIP | 0 | 0 | 0 |
| regtech_missing_pep_arm_fail.mm | 1 | 0 | FAIL | FAIL | yes | MEASURED | 0.056s | SKIP | 0 | 0 | 0 |
| rtgs_balance_break_fail.mm | 1 | 0 | FAIL | FAIL | yes | MEASURED | 0.044s | SKIP | 0 | 0 | 0 |
| rtgs_balance_conservation.mm | 4 | 0 | PASS | PASS | yes | MEASURED | 0.009s | SKIP | 0 | 0 | 0 |

#### state_machine

| File | Atoms | Trusted | Expected | Actual | Match | Verify Status | Solver Time | Lean Solver Time | Escalated | lean_verified | Tactic Search |
|------|-------|---------|----------|--------|-------|---------------|-------------|------------------|-----------|---------------|---------------|
| escrow_transfer.mm | 3 | 0 | PASS | PASS | yes | MEASURED | 0.008s | SKIP | 0 | 0 | 0 |
| order_lifecycle.mm | 4 | 0 | PASS | PASS | yes | MEASURED | 0.008s | SKIP | 0 | 0 | 0 |
| session_auth.mm | 4 | 0 | PASS | PASS | yes | MEASURED | 0.008s | SKIP | 0 | 0 | 0 |
| skip_ship_fail.mm | 1 | 0 | FAIL | FAIL | yes | MEASURED | 0.031s | SKIP | 0 | 0 | 0 |
| transfer_without_accept_fail.mm | 1 | 0 | FAIL | FAIL | yes | MEASURED | 0.030s | SKIP | 0 | 0 | 0 |
| unauthenticated_read_fail.mm | 1 | 0 | FAIL | FAIL | yes | MEASURED | 0.031s | SKIP | 0 | 0 | 0 |

#### svcomp_style

| File | Atoms | Trusted | Expected | Actual | Match | Verify Status | Solver Time | Lean Solver Time | Escalated | lean_verified | Tactic Search |
|------|-------|---------|----------|--------|-------|---------------|-------------|------------------|-----------|---------------|---------------|
| array_bounds.mm | 1 | 0 | PASS | PASS | yes | MEASURED | 0.007s | SKIP | 0 | 0 | 0 |
| integer_overflow.mm | 1 | 0 | PASS | PASS | yes | MEASURED | 0.007s | SKIP | 0 | 0 | 0 |
| loop_invariant.mm | 1 | 0 | PASS | PASS | yes | MEASURED | 0.007s | SKIP | 0 | 0 | 0 |

</details>
