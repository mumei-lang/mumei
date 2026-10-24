// Loop havoc must forget the VALUE of scalar non-integer bindings while
// keeping their sort. Previously `havoc_vars`'s catch-all cloned the old
// Z3 const, so a `Str`/enum/f64 assigned inside a `while` kept its
// pre-loop value in the post-loop environment (unsound — see
// tests/negative/scalar_havoc_stale.mm). These atoms exercise the fixed
// behaviour in the positive direction: a havoced `Str` is still a `Str`.

atom str_havoc_len(flag: bool) -> i64
requires: flag == flag;
ensures: result >= 0;
effects: [];
body: {
    let s = "x";
    let i = 0;
    while i < 1
    invariant: i >= 0
    decreases: 1 - i
    {
        s = "y";
        i = i + 1;
    }
    len(s)
};

// The loop invariant may still talk about a havoced `Str` — it is
// re-asserted each iteration, so `s == "y"` is provable post-loop only
// because the invariant (not the stale pre-loop value) establishes it.
atom str_havoc_invariant() -> bool
requires: true;
ensures: result == true;
effects: [];
body: {
    let s = "y";
    let i = 0;
    while i < 1
    invariant: s == "y" && i >= 0 && i <= 1
    decreases: 1 - i
    {
        s = "y";
        i = i + 1;
    }
    s == "y"
};
