// =============================================================
// tests/test_contracts.mm — Verified Contracts E2E Test
// =============================================================
// Integration test for std/contracts.mm module.
// Usage: mumei check tests/test_contracts.mm
import "std/contracts" as contracts;

// Test: clamp below range returns min_val
atom test_clamp_lower()
    requires: true;
    ensures: result == 10;
    body: {
        contracts::clamp(5, 10, 20)
    };

// Test: clamp above range returns max_val
atom test_clamp_upper()
    requires: true;
    ensures: result == 20;
    body: {
        contracts::clamp(25, 10, 20)
    };

// Test: clamp within range returns identity
atom test_clamp_in_range()
    requires: true;
    ensures: result == 15;
    body: {
        contracts::clamp(15, 10, 20)
    };

// Test: valid port returns 1
atom test_is_valid_port_true()
    requires: true;
    ensures: result == 1;
    body: {
        contracts::is_valid_port(8080)
    };

// Test: invalid port (0 or 65536) returns 0
atom test_is_valid_port_false()
    requires: true;
    ensures: result == 0;
    body: {
        contracts::is_valid_port(0)
    };

// Test: division with non-zero divisor
atom test_safe_divide()
    requires: true;
    ensures: result == 5;
    body: {
        contracts::safe_divide(10, 2)
    };

// Test: abs of positive value
atom test_abs_positive()
    requires: true;
    ensures: result == 7;
    body: {
        contracts::abs_val(7)
    };

// Test: abs of negative value
atom test_abs_negative()
    requires: true;
    ensures: result == 7;
    body: {
        contracts::abs_val(0 - 7)
    };

// Test: max of two values
atom test_max_of()
    requires: true;
    ensures: result == 42;
    body: {
        contracts::max_of(17, 42)
    };

// Test: min of two values
atom test_min_of()
    requires: true;
    ensures: result == 17;
    body: {
        contracts::min_of(17, 42)
    };
