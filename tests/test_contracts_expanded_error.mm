// =============================================================
// tests/test_contracts_expanded_error.mm — Contract Violation Tests
// =============================================================
// This file is expected to FAIL verification.
// Each atom deliberately violates a contract's requires clause
// to confirm that Z3 rejects invalid call sites.
//
// Usage: mumei verify tests/test_contracts_expanded_error.mm
// Expected: verification errors for all atoms below.

import "std/contracts" as contracts;

// --- safe_subtract: a < b (underflow) ---
// safe_subtract requires: a >= b
// Here a=3, b=10 — should fail.
atom bad_safe_subtract_underflow()
    requires: true;
    ensures: result >= 0;
    body: contracts::safe_subtract(3, 10);

// --- bounded_increment: val < 0 ---
// bounded_increment requires: val >= 0 && max_val > 0
// Here val=-1 — should fail.
atom bad_bounded_increment_negative()
    requires: true;
    ensures: result >= 0;
    body: contracts::bounded_increment(0 - 1, 10);

// --- bounded_decrement: val < min_val ---
// bounded_decrement requires: val >= min_val && min_val >= 0
// Here val=2, min_val=5 — should fail.
atom bad_bounded_decrement_below_min()
    requires: true;
    ensures: result >= 0;
    body: contracts::bounded_decrement(2, 5);

// --- bounded_decrement: min_val < 0 ---
// bounded_decrement requires: min_val >= 0
// Here min_val=-1 — should fail.
atom bad_bounded_decrement_negative_min()
    requires: true;
    ensures: result >= 0;
    body: contracts::bounded_decrement(5, 0 - 1);
