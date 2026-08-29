// Contract expressions that can fault at runtime (division by zero) while
// staying inside the monitor's expression subset: the monitor must report the
// failed evaluation instead of unwinding through the monitored call.
// expected: PASS

trusted atom risky_ratio(divisor: i64) -> i64 {
    requires: 100 / divisor > 0;
    ensures: 100 / result > 0;
    body: {
        divisor
    }
}
