// =============================================================
// Negative: callee len guarantees are real constraints, not vacuous
// =============================================================
// With call-result len propagation, `len(make3(n))` is pinned to 3 by the
// callee's `ensures: len(result) == 3` — so claiming `result == 4` must
// FAIL (if the len were an unconstrained fresh symbol this would be
// spuriously satisfiable).

atom make3(n: i64) -> [i64]
requires: n >= 0;
ensures: len(result) == 3;
effects: [];
body: {
    [1, 2, 3]
};

atom caller_wrong_len(n: i64) -> i64
requires: n >= 0;
ensures: result == 4;
effects: [];
body: {
    let t = make3(n);
    len(t)
};

// Index len(t) itself is out of bounds for a len-3 result.
atom caller_oob_index(n: i64) -> i64
requires: n >= 0;
ensures: result >= result;
effects: [];
body: {
    let t = make3(n);
    t[len(t)]
};
