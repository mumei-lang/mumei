// =============================================================
// tests/test_fixed_point.mm — Fixed-Point Arithmetic E2E Test
// =============================================================
// Integration test for std/math/fixed_point.mm module.
// Usage: mumei check tests/test_fixed_point.mm
import "std/math/fixed_point" as fp;

// Test: addition of two fixed-point values
atom test_fp_add()
    requires: true;
    ensures: result == 30000;
    body: {
        fp::fp_add(10000, 20000)
    };

// Test: integer to fixed-point conversion (3 * 10000 = 30000)
atom test_fp_from_int()
    requires: true;
    ensures: result == 30000;
    body: {
        fp::fp_from_int(3)
    };

// Test: positive check returns 1 for positive value
atom test_fp_is_positive()
    requires: true;
    ensures: result == 1;
    body: {
        fp::fp_is_positive(10000)
    };

// Test: absolute value of negative fixed-point
atom test_fp_abs()
    requires: true;
    ensures: result == 50000;
    body: {
        fp::fp_abs(0 - 50000)
    };
