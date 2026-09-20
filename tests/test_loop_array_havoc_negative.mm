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
