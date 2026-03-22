// =============================================================
// Verified FFI Layer tests
// =============================================================
// Tests that extern functions with requires/ensures contracts
// are verified at call sites by Z3.

extern "Rust" {
    fn sqrt(x: f64) -> f64
        requires: x >= 0.0;
        ensures: result >= 0.0;

    fn abs(x: i64) -> i64
        requires: true;
        ensures: result >= 0;
}

// This should PASS: caller satisfies sqrt's requires (x >= 0.0)
atom safe_root(x: f64) -> f64
    requires: x >= 0.0;
    ensures: result >= 0.0;
    body: sqrt(x);

// This should PASS: abs has no meaningful precondition
atom safe_abs(x: i64) -> i64
    requires: true;
    ensures: result >= 0;
    body: abs(x);

// This should PASS: caller provides non-negative value
atom compute_distance(a: f64, b: f64) -> f64
    requires: a >= 0.0 && b >= 0.0;
    ensures: result >= 0.0;
    body: sqrt(a + b);

// Extern block without contracts (backward compatible)
extern "C" {
    fn rand() -> i64;
}
