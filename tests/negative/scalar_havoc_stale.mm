// =============================================================
// Negative: loop havoc must forget non-integer values too
// =============================================================
// `havoc_vars` used to fall through to `old.clone()` for any sort it did
// not special-case, so a `Str`/enum/f64 rebound inside a `while` kept its
// pre-loop constant in the post-loop environment — letting the post-loop
// `s == "x"` below prove `result == 1` even though the body assigns
// `s = "y"` (unsound). All atoms here must FAIL verification.

atom str_stale() -> i64
requires: true;
ensures: result == 1;
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
    if s == "x" { 1 } else { 0 }
};

enum Color { Red, Blue }

atom enum_stale() -> i64
requires: true;
ensures: result == 1;
effects: [];
body: {
    let c = Color::Red;
    let i = 0;
    while i < 1
    invariant: i >= 0
    decreases: 1 - i
    {
        c = Color::Blue;
        i = i + 1;
    }
    match c {
        Color::Red => 1,
        Color::Blue => 0
    }
};

atom f64_stale() -> i64
requires: true;
ensures: result == 1;
effects: [];
body: {
    let f = 1.5;
    let i = 0;
    while i < 1
    invariant: i >= 0
    decreases: 1 - i
    {
        f = 2.5;
        i = i + 1;
    }
    if f == 1.5 { 1 } else { 0 }
};
