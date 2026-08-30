// Zero-cost fixture: a fully proven, self-contained atom. The proof-aware
// runtime monitor emitter must generate nothing at all for it.
// expected: PASS

atom double_non_negative(x: i64) -> i64 {
    requires: x >= 0 && x < 1000;
    ensures: result == x + x;
    body: {
        x + x
    }
}
