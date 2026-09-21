// `while`-loop bodies that *call* an atom which stores through an array
// param must havoc the caller's array post-state: the call never marks
// `a[i] = v` as a direct assignment, so `collect_assigned_vars` resolves
// the callee with the same predicate `havoc_array_args` uses at eval
// (`atom_stores_to_array` on the callee's params). These atoms only
// assert facts that stay provable under a correctly havoced post-state.

atom arr_store(a: [i64], v: i64) -> i64
requires: len(a) >= 1;
ensures: result == 0;
body: {
    a[0] = v;
    0
};

atom arr_read(a: [i64]) -> i64
requires: len(a) >= 1;
ensures: result == a[0];
body: {
    a[0]
};

// Mutating callee in the loop: post-loop reads see an unconstrained
// element — trivially-true claims still verify.
atom call_mut_bounded(a: [i64], n: i64) -> i64
requires: len(a) >= 1 && n >= 1;
ensures: result >= 0;
body: {
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n
    decreases: n - i
    {
        let t = arr_store(a, 9);
        i = i + 1
    };
    if a[0] >= -1000000 { 1 } else { 0 }
};

// `len(a)` is never havoced (a called atom cannot resize the caller's
// array), so its entry value still proves post-loop.
atom call_mut_len(a: [i64], n: i64) -> i64
requires: len(a) == 3 && n >= 1;
ensures: result == 3;
body: {
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n
    decreases: n - i
    {
        let t = arr_store(a, 9);
        i = i + 1
    };
    len(a)
};

// Pure callee in the loop: the array is NOT havoced, so `a[0]` keeps its
// entry fact — this must stay provable (no over-havoc).
atom call_pure_keeps_value(a: [i64], n: i64) -> i64
requires: len(a) >= 1 && n >= 1 && a[0] == 7;
ensures: result == 7;
body: {
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n
    decreases: n - i
    {
        let t = arr_read(a);
        i = i + 1
    };
    a[0]
};

atom touch_first(a: [i64], keep: [i64]) -> i64
requires: len(a) >= 1 && len(keep) >= 1;
ensures: result == 0;
body: {
    a[0] = 0;
    0
};

// Only the param the callee stores through is havoced — the sibling
// array arg keeps its entry value post-loop.
atom call_marks_only_stored_arg(a: [i64], keep: [i64], n: i64) -> i64
requires: len(a) >= 1 && len(keep) >= 1 && n >= 1 && keep[0] == 5;
ensures: result == 5;
body: {
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n
    decreases: n - i
    {
        let t = touch_first(a, keep);
        i = i + 1
    };
    keep[0]
};

// `call(f, a)` through a `let`-bound `atom_ref` takes the dynamic-call
// path at eval (the callee name is the *variable* `f`, which no atom is
// named) — that path cannot see which atom `f` refers to, so it havoc's
// every array-bound arg. The array is havoced, so only trivially-true
// claims about it survive.
atom call_let_bound_ref_bounded(a: [i64], n: i64) -> i64
requires: len(a) >= 1 && n >= 1 && a[0] == 7;
ensures: result >= 0;
body: {
    let f = atom_ref(arr_read);
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n
    decreases: n - i
    {
        let t = call(f, a);
        i = i + 1
    };
    if a[0] >= -1000000 { 1 } else { 0 }
};

// A non-storing lambda callee is havoced precisely: `x[0]` is only read,
// so `a[0]` keeps its entry fact — this must stay provable.
atom call_lambda_pure_keeps_value(a: [i64]) -> i64
requires: len(a) >= 1 && a[0] == 7;
ensures: result == 7;
body: {
    let t = call(|x| { x[0] }, a);
    a[0]
};

// A storing lambda havoc's the array — the element is unconstrained
// post-call, so only claims that hold of any element still verify
// (and `len` is never havoced).
atom call_lambda_store_bounded(a: [i64]) -> i64
requires: len(a) >= 1 && a[0] == 7;
ensures: result >= 0;
body: {
    let t = call(|x| { if len(x) >= 1 { x[0] = 0; 0 } else { 0 } }, a);
    if a[0] >= -1000000 && len(a) >= 1 { 1 } else { 0 }
};

// A non-storing lambda inside the loop is likewise unmarked — `a[0]`
// keeps its entry fact post-loop.
atom call_lambda_pure_loop_keeps_value(a: [i64], n: i64) -> i64
requires: len(a) >= 1 && n >= 1 && a[0] == 7;
ensures: result == 7;
body: {
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n
    decreases: n - i
    {
        let t = call(|x| { x[0] }, a);
        i = i + 1
    };
    a[0]
};

// A lambda that only READS a captured array is not a store — `a[0]`
// keeps its entry fact (capture sweep precision).
atom call_lambda_capture_read_keeps(a: [i64]) -> i64
requires: len(a) >= 1 && a[0] == 7;
ensures: result == 7;
body: {
    let g = |x| { a[0] + x };
    let t = call(g, 1);
    a[0]
};
