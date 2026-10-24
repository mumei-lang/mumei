// `while`-loop array stores must havoc the array's post-state: a param
// array's `__z3_arr_<name>` slot materializes lazily on first access,
// so a body that only *writes* `a[i]` previously left the entry binding
// in place and post-loop reads saw pre-loop (stale) values.
// These atoms only assert facts that stay provable under a correctly
// havoced post-state.

// Post-loop reads see an unconstrained element — trivially-true claims
// still verify.
atom post_loop_bounded(a: [i64], n: i64) -> i64
requires: len(a) >= 1 && n >= 1;
ensures: result >= 0;
body: {
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n
    decreases: n - i
    {
        a[0] = 9;
        i = i + 1
    };
    if a[0] >= -1000000 { 1 } else { 0 }
};

// `len(a)` is never havoced (stores cannot resize), so its entry value
// still proves post-loop.
atom post_loop_len(a: [i64], n: i64) -> i64
requires: len(a) == 3 && n >= 1;
ensures: result == 3;
body: {
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n
    decreases: n - i
    {
        a[0] = 9;
        i = i + 1
    };
    len(a)
};

// An earlier read materializes the slot before the loop — havoced the
// same way (a read-then-write mix must behave identically).
atom post_loop_after_read(a: [i64], n: i64) -> i64
requires: len(a) >= 1 && n >= 1;
ensures: result >= 0;
body: {
    let x = a[0] + 0;
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n
    decreases: n - i
    {
        a[0] = 9;
        i = i + 1
    };
    if a[0] >= -1000000 { 1 } else { 0 }
};

// Aliased reads (`b = a`) havoc with the shared backing root — a claim
// that survives an unconstrained element still proves through the alias.
atom post_loop_alias(a: [i64], n: i64) -> i64
requires: len(a) >= 1 && n >= 1;
ensures: result >= 0;
body: {
    let b = a;
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n
    decreases: n - i
    {
        a[0] = 9;
        i = i + 1
    };
    if b[0] >= -1000000 { 1 } else { 0 }
};
