// Stale-read wrong-verifies: each of these `ensures` is FALSE after the
// loop writes — the havoced post-state must not carry pre-loop facts.

// `a[0]` is 9 after the loop, not the entry 7 — must not verify.
atom stale_i64(a: [i64], n: i64) -> i64
requires: len(a) >= 1 && n >= 1 && a[0] == 7;
ensures: result == 7;
body: {
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n
    decreases: n - i
    {
        a[0] = 9;
        i = i + 1
    };
    a[0]
};

// Same stale read on a `[Str]` param — entry "x" is overwritten by "q".
atom stale_str(a: [Str], n: i64) -> i64
requires: len(a) >= 1 && n >= 1 && a[0] == "x";
ensures: result == 1;
body: {
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n
    decreases: n - i
    {
        a[0] = "q";
        i = i + 1
    };
    if a[0] == "x" { 1 } else { 0 }
};

// Claiming the stored value is also unprovable — the post-state is
// havoced, not the loop's last write.
atom last_write(a: [i64], n: i64) -> i64
requires: len(a) >= 1 && n >= 1;
ensures: result == 9;
body: {
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n
    decreases: n - i
    {
        a[0] = 9;
        i = i + 1
    };
    a[0]
};

// Aliases share the backing root: `b = a` then a loop write through `a`
// must havoc `b`'s view too — `b[0]` is not the entry 7 anymore.
atom stale_alias(a: [i64], n: i64) -> i64
requires: len(a) >= 1 && n >= 1 && a[0] == 7;
ensures: result == 7;
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
    b[0]
};

// Same via an alias whose `__z3_arr_b` slot was materialized by an
// earlier read — the slot must havoc with the shared root.
atom stale_alias_slot(a: [i64], n: i64) -> i64
requires: len(a) >= 1 && n >= 1 && a[0] == 7;
ensures: result == 7;
body: {
    let b = a;
    let x = b[0] + 0;
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n
    decreases: n - i
    {
        a[0] = 9;
        i = i + 1
    };
    b[0]
};
