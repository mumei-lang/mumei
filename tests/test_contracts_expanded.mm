// =============================================================
// tests/test_contracts_expanded.mm — Expanded Contracts Verification Test
// =============================================================
// Integration test for the new atoms added to std/contracts.mm.
// Usage: mumei verify tests/test_contracts_expanded.mm
import "std/contracts" as contracts;

// Test: safe_subtract with a > b
atom test_safe_subtract()
    requires: true;
    ensures: result == 7;
    body: {
        contracts::safe_subtract(10, 3)
    };

// Test: safe_subtract with a == b (edge case)
atom test_safe_subtract_equal()
    requires: true;
    ensures: result == 0;
    body: {
        contracts::safe_subtract(5, 5)
    };

// Test: bounded_increment below max
atom test_bounded_increment_below_max()
    requires: true;
    ensures: result == 6;
    body: {
        contracts::bounded_increment(5, 10)
    };

// Test: bounded_increment at max (should stay)
atom test_bounded_increment_at_max()
    requires: true;
    ensures: result == 10;
    body: {
        contracts::bounded_increment(10, 10)
    };

// Test: bounded_decrement above min
atom test_bounded_decrement_above_min()
    requires: true;
    ensures: result == 4;
    body: {
        contracts::bounded_decrement(5, 0)
    };

// Test: bounded_decrement at min (should stay)
atom test_bounded_decrement_at_min()
    requires: true;
    ensures: result == 0;
    body: {
        contracts::bounded_decrement(0, 0)
    };

// Test: sign of positive number
atom test_sign_positive()
    requires: true;
    ensures: result == 1;
    body: {
        contracts::sign(42)
    };

// Test: sign of negative number
atom test_sign_negative()
    requires: true;
    ensures: result == 0 - 1;
    body: {
        contracts::sign(0 - 5)
    };

// Test: sign of zero
atom test_sign_zero()
    requires: true;
    ensures: result == 0;
    body: {
        contracts::sign(0)
    };
