// Widened integer enum payloads on the general verifier's Datatype path:
// `u64`/`u32`/`i32` leaves (`usize`/`isize` fold to `u64`/`i64`) join the
// `i64`/`f64`/`Str`/`bool` scalar set `field_is_scalar` admits, each bound
// to its declared width wherever an enum value is introduced — a negative
// `u64` leaf is not a value the ABI can produce, so `x >= 0` under
// `W::A(x)` holds unconditionally.

enum W {
    A(u64),
    B(i32),
    C(u32),
    D
}

// An enum with widened leaves round-trips through a call unchanged.
atom keep(e: W) -> W
requires: true;
ensures: result == e;
body: e

// `u64` leaf: non-negativity is free — without the domain assert the
// unconstrained Int leaf could be -1 and the else arm would reach.
atom nonneg(e: W) -> i64
requires: true;
ensures: result == 1;
body: {
    match e {
        W::A(x) => if x >= 0 { 1 } else { 0 },
        _ => 1
    }
}

// `i32` leaf: the declared lower bound holds.
atom i32floor(e: W) -> i64
requires: true;
ensures: result == 1;
body: {
    match e {
        W::B(x) => if x >= -2147483648 { 1 } else { 0 },
        _ => 1
    }
}

// `u32` leaf: the declared upper bound holds.
atom u32ceil(e: W) -> i64
requires: true;
ensures: result == 1;
body: {
    match e {
        W::C(x) => if x <= 4294967295 { 1 } else { 0 },
        _ => 1
    }
}

// `usize` resolves to `u64` before leaf collection: the same domain
// assert keeps a usize payload non-negative.
enum Sz {
    N(usize),
    Empty
}

atom usize_nonneg(e: Sz) -> i64
requires: true;
ensures: result == 1;
body: {
    match e {
        Sz::N(x) => if x >= 0 { 1 } else { 0 },
        _ => 1
    }
}

// Nested enum payloads: the inner `u64` leaf keeps its declared domain
// inside `Outer::Wrap` — the `enum:`-marked leaf is recursed into, not
// skipped.
enum Inner {
    A(u64),
    B
}

enum Outer {
    Wrap(Inner),
    C
}

atom nested_nonneg(e: Outer) -> i64
requires: true;
ensures: result == 1;
body: {
    match e {
        Outer::Wrap(i) => match i {
            Inner::A(x) => if x >= 0 { 1 } else { 0 },
            _ => 1
        },
        _ => 1
    }
}

// Payload-name registrations stop at the arm: the coverage pass binds
// `W::A(x)` for its guard, but after the match the outer `x` is still
// the declared `i64` — `x + y` must not trip the u64 mixing rule.
atom shadow_scope(e: W, y: i64) -> i64
requires: true;
ensures: result == 1;
body: {
    let x = y;
    let m = match e {
        W::A(x) if x >= 0 => x,
        _ => 0
    };
    if x + y == y + y { 1 } else { 0 }
}

// Deeper nesting: the domain walk descends four `enum:` levels — the
// emitted-conjunct budget keeps the bound, not the recursion depth.
enum N4 {
    A(u64),
    B
}

enum N3 {
    W(N4),
    Z
}

enum N2 {
    W(N3),
    Z
}

enum N1 {
    W(N2),
    Z
}

atom deep_chain(e: N1) -> i64
requires: true;
ensures: result == 1;
body: {
    match e {
        N1::W(a) => match a {
            N2::W(b) => match b {
                N3::W(c) => match c {
                    N4::A(x) => if x >= 0 { 1 } else { 0 },
                    _ => 1
                },
                _ => 1
            },
            _ => 1
        },
        _ => 1
    }
}
