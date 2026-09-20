// Stale-read wrong-verifies through loop-carried calls: each `ensures`
// is FALSE after the callee stores through its array param — the
// havoced post-state must not carry pre-loop facts.

atom arr_store(a: [i64], v: i64) -> i64
requires: len(a) >= 1;
ensures: result == 0;
body: {
    a[0] = v;
    0
};

atom str_store(a: [Str]) -> i64
requires: len(a) >= 1;
ensures: result == 0;
body: {
    a[0] = "q";
    0
};

atom gen_store<T>(a: [T], v: T) -> i64
requires: len(a) >= 1;
ensures: result == 0;
body: {
    a[0] = v;
    0
};

// `a[0]` is 0 after the loop (arr_store wrote 0), not the entry 7.
atom stale_call_i64(a: [i64], n: i64) -> i64
requires: len(a) >= 1 && n >= 1 && a[0] == 7;
ensures: result == 7;
body: {
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n
    decreases: n - i
    {
        let t = arr_store(a, 0);
        i = i + 1
    };
    a[0]
};

// Same stale read through a `[Str]` param — entry "x" is overwritten
// by "q" inside str_store.
atom stale_call_str(a: [Str], n: i64) -> i64
requires: len(a) >= 1 && n >= 1 && a[0] == "x";
ensures: result == 1;
body: {
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n
    decreases: n - i
    {
        let t = str_store(a);
        i = i + 1
    };
    if a[0] == "x" { 1 } else { 0 }
};

// `call(atom_ref(w), a)` resolves the atom directly at eval — the
// callee name is known statically, so `__z3_arr_a` must be marked.
atom stale_call_ref(a: [i64], n: i64) -> i64
requires: len(a) >= 1 && n >= 1 && a[0] == 7;
ensures: result == 7;
body: {
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n
    decreases: n - i
    {
        let t = call(atom_ref(arr_store), a, 0);
        i = i + 1
    };
    a[0]
};

// Explicit type-argument calls carry the instantiation in the callee
// name (`gen_store<i64>`) and resolve to the monomorphized atom — the
// stored-through param is still marked.
atom stale_call_generic(a: [i64], n: i64) -> i64
requires: len(a) >= 1 && n >= 1 && a[0] == 7;
ensures: result == 7;
body: {
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n
    decreases: n - i
    {
        let t = gen_store<i64>(a, 0);
        i = i + 1
    };
    a[0]
};

// A mutating call nested inside an array literal still runs at eval —
// `__z3_arr_a` must be marked through the literal's element walk.
atom stale_call_arraylit(a: [i64], n: i64) -> i64
requires: len(a) >= 1 && n >= 1 && a[0] == 7;
ensures: result == 7;
body: {
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n
    decreases: n - i
    {
        let m = [arr_store(a, 0), 1];
        i = i + 1
    };
    a[0]
};

// Calling the mutating callee on a moved-into local (`b = a`) marks
// `__z3_arr_b` — `b[0]` must not keep the entry fact post-loop.
atom stale_call_alias(a: [i64], n: i64) -> i64
requires: len(a) >= 1 && n >= 1 && a[0] == 7;
ensures: result == 7;
body: {
    let b = a;
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n
    decreases: n - i
    {
        let t = arr_store(b, 0);
        i = i + 1
    };
    b[0]
};
