// `let a = [e0, e1, …]` — array literals bind a concrete-length Z3 array.
// Previously a `[` prefix fell through to the expression catch-all and
// silently parsed as `Number(0)`, so nothing about the literal was tracked.

atom lit_read() -> i64
ensures: result == 20;
body: {
    let a = [10, 20, 30];
    a[1]
};

atom lit_store() -> i64
ensures: result == 99;
body: {
    let a = [1, 2, 3];
    a[1] = 99;
    a[1]
};

atom lit_len(arr: [i64]) -> i64
requires: len(arr) >= 3 && forall(i, 0, 3, arr[i] >= 0);
ensures: result >= 0;
body: {
    let a = arr;
    a[2]
};

// f64 element literals widen integer elements via sitofp.
atom lit_f64() -> f64
ensures: result == 2.5;
body: {
    let a = [1.0, 2.5, 4.0];
    a[1]
};

// bool element literals stay Bool-sorted.
atom lit_bool() -> bool
ensures: result == true;
body: {
    let a = [true, false, true];
    a[0]
};

// Rebinding to a new literal replaces the tracked array.
atom lit_reassign() -> i64
ensures: result == 7;
body: {
    let a = [1, 2, 3];
    a = [7, 9];
    a[0]
};

// `[e0, …]` as the body tail wires `__z3_arr_result`/`len_result` so
// ensures clauses can quantify over the returned array.
atom lit_tail() -> [i64]
ensures: forall(i, 0, 3, result[i] == i + 1);
body: {
    [1, 2, 3]
};

atom head_lit(arr: [i64]) -> i64
requires: len(arr) >= 1;
ensures: result == arr[0];
body: {
    arr[0]
};

// Array literal as a call argument — the callee's `len(arr) >= 1`
// contract must see the literal's concrete length.
atom lit_call_arg() -> i64
ensures: result == 7;
body: {
    head_lit([7, 8, 9])
};

// A store through the binding must be visible at the `result` tail —
// `__z3_arr_result` wires the post-store chain, not the original const.
atom lit_stored_tail(arr: [i64]) -> [i64]
requires: len(arr) >= 1;
ensures: result[0] == 99;
body: {
    let a = arr;
    a[0] = 99;
    a
};

// `let b = a` on a literal aliases the same tracked array.
atom lit_let_alias() -> i64
ensures: result == 2;
body: {
    let a = [1, 2, 3];
    let b = a;
    b[1]
};

// Symbolic elements from params are allowed.
atom lit_symbolic(x: i64, y: i64) -> i64
requires: x >= 0 && y >= 0;
ensures: result >= 0;
body: {
    let a = [x, y, x + y];
    a[0] + a[1]
};

// A non-mutating callee leaves the caller's tracked chain intact —
// `head(a)` returns a[0] and `a` keeps its literal contents afterwards.
atom lit_head(arr: [i64]) -> i64
requires: len(arr) >= 1;
ensures: result == arr[0];
body: {
    arr[0]
};
atom lit_call_pure() -> i64
ensures: result == 5;
body: {
    let a = [5, 6];
    lit_head(a)
};
