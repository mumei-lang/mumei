// `let t = f(..)` must inherit the callee's `ensures: len(result) == k`:
// the caller's `len_t` symbol is the *same* len symbol the callee asserted
// on, so `len(t)`/bounds on `t` see the guaranteed length instead of a
// fresh unconstrained one.

atom make3(n: i64) -> [i64]
requires: n >= 0;
ensures: len(result) == 3;
effects: [];
body: {
    [1, 2, 3]
};

atom caller_len(n: i64) -> i64
requires: n >= 0;
ensures: result == 3;
effects: [];
body: {
    let t = make3(n);
    len(t)
};

// `len(t) - 1` is a provably in-bounds index once the len propagates.
atom caller_index(n: i64) -> i64
requires: n >= 0;
ensures: result >= result;
effects: [];
body: {
    let t = make3(n);
    t[len(t) - 1]
};

// `len(f(..))` on a call result directly (no `let` binding).
atom caller_direct(n: i64) -> i64
requires: n >= 0;
ensures: result == 3;
effects: [];
body: {
    len(make3(n))
};

// Both branches are call results — the merged ite length is constrained
// by each callee's own `len(result)` ensures.
atom make4(n: i64) -> [i64]
requires: n >= 0;
ensures: len(result) == 4;
effects: [];
body: {
    [1, 2, 3, 4]
};

atom caller_if(n: i64, c: bool) -> i64
requires: n >= 0 && (c == true || c == false);
ensures: result == 3 || result == 4;
effects: [];
body: {
    let t = if c { make3(n) } else { make4(n) };
    len(t)
};

// A caller-side store does not change the length (fat pointer is len+ptr).
atom caller_store(n: i64) -> i64
requires: n >= 0;
ensures: result == 3;
effects: [];
body: {
    let t = make3(n);
    t[0] = 9;
    len(t)
};
