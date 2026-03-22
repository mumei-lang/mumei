// =============================================================
// tests/test_trait_constraints.mm — Trait Method Constraints Test
// =============================================================
// Integration test for trait method param_constraints Z3 injection (P2-B).
// Usage: mumei check tests/test_trait_constraints.mm

trait SafeDiv {
    fn div(a: Self, b: Self where v != 0) -> Self;
}

impl SafeDiv for i64 {
    fn div(a: i64, b: i64) -> i64 {
        a / b
    }
}

// This should pass: b != 0 is guaranteed by the requires clause.
atom safe_divide(a: i64, b: i64)
    requires: b != 0;
    ensures: true;
    body: a / b;

// This should fail verification: b could be 0 (no constraint on b).
atom unsafe_divide(a: i64, b: i64)
    requires: true;
    ensures: true;
    body: a / b;
