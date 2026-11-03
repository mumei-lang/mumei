// Contract expressions that can fault at runtime (i64 overflow) while staying
// inside the monitor's expression subset: the monitor must report the failed
// evaluation instead of unwinding through the monitored call. The verifier
// models integers as unbounded, so the contract itself is accepted.
// expected: PASS

trusted atom risky_square(x: i64) -> i64 {
    requires: x * x >= 0;
    ensures: result * result >= 0;
    body: {
        x
    }
}
