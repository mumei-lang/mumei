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

// `call(f, a)` through a `let`-bound `atom_ref` lands on the dynamic
// path — the concrete target (`arr_store`, which writes a[0]=0) cannot
// be seen, so every array-bound arg is havoced and the stale `a[0]==7`
// claim must fail closed.
atom stale_dyn_call_let_bound_ref(a: [i64], n: i64) -> i64
requires: len(a) >= 1 && n >= 1 && a[0] == 7;
ensures: result == 7;
body: {
    let f = atom_ref(arr_store);
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n
    decreases: n - i
    {
        let t = call(f, a, 0);
        i = i + 1
    };
    a[0]
};

// Straight-line version: the dynamic `call(f, a, 0)` havoc's `a`
// immediately, so the post-call `a[0] == 7` read must fail closed.
atom stale_dyn_call_straightline(a: [i64]) -> i64
requires: len(a) >= 1 && a[0] == 7;
ensures: result == 7;
body: {
    let f = atom_ref(arr_store);
    let t = call(f, a, 0);
    a[0]
};

// A storing lambda callee writes through its param — the straight-line
// read after it must not keep the entry fact. (The store is guarded so
// the failure is the stale read itself, not a bounds error.)
atom stale_call_lambda_straightline(a: [i64]) -> i64
requires: len(a) >= 1 && a[0] == 7;
ensures: result == 7;
body: {
    let t = call(|x| { if len(x) >= 1 { x[0] = 0; 0 } else { 0 } }, a);
    a[0]
};

// Same storing lambda inside a loop: `__z3_arr_a` must be marked so the
// post-loop read can't claim the pre-loop element.
atom stale_call_lambda_in_loop(a: [i64], n: i64) -> i64
requires: len(a) >= 1 && n >= 1 && a[0] == 7;
ensures: result == 7;
body: {
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n
    decreases: n - i
    {
        let t = call(|x| { if len(x) >= 1 { x[0] = 0; 0 } else { 0 } }, a);
        i = i + 1
    };
    a[0]
};

// A let-bound lambda (`let g = |x| {...}; g(a)`) resolves through
// `local_lambdas` on the `Call` path — same stale read must fail.
atom stale_call_let_bound_lambda(a: [i64], n: i64) -> i64
requires: len(a) >= 1 && n >= 1 && a[0] == 7;
ensures: result == 7;
body: {
    let g = |x| { if len(x) >= 1 { x[0] = 0; 0 } else { 0 } };
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n
    decreases: n - i
    {
        let t = g(a);
        i = i + 1
    };
    a[0]
};

// A lambda storing through a CAPTURED variable (not a param) mutates
// the caller's array — the cloned call_env shares the slot, so the
// post-call read must not keep the entry fact.
atom stale_call_lambda_capture(a: [i64]) -> i64
requires: len(a) >= 1 && a[0] == 7;
ensures: result == 7;
body: {
    let g = |x| { a[0] = 0; x };
    let t = call(g, 1);
    a[0]
};

// Same capture-store through a loop-bound... bound before the loop:
// `__z3_arr_a` must be marked via the lambda's capture sweep.
atom stale_call_lambda_capture_loop(a: [i64], n: i64) -> i64
requires: len(a) >= 1 && n >= 1 && a[0] == 7;
ensures: result == 7;
body: {
    let g = |x| { a[0] = 0; x };
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n
    decreases: n - i
    {
        let t = g(0);
        i = i + 1
    };
    a[0]
};
