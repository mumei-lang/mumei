// `f64` leaf parameters, fields and results (`pure-i64-composite/v11`).
// A declared `f64` is a pass-through 64-bit leaf carrying the IEEE-754
// bit pattern opaquely: it may be stored, copied, returned, placed in a
// struct field or enum payload and merged across branches, but never
// read by arithmetic or comparison — no `f64` literal exists.

atom copy(a: f64) -> f64
requires: true;
ensures: true;
body: {
    a
};

// Call-result pass-through: a callee's f64 result feeds a caller's f64.
atom dup(x: f64) -> f64
requires: true;
ensures: true;
body: {
    copy(x)
};

struct Box {
    v: f64,
    tag: i64
}

atom wrap(x: f64, t: i64) -> Box
requires: t >= 0;
ensures: result.tag == t;
body: {
    Box{v: x, tag: t}
};

atom unwrap(b: Box) -> f64
requires: b.tag >= 0;
ensures: true;
body: {
    b.v
};

// A `let`-bound f64 rebinding keeps its bit-pattern slot.
atom relay(x: f64) -> f64
requires: true;
ensures: true;
body: {
    let y = x;
    y
};

// Branch merge: the f64 slot selects like any leaf.
atom pick(flag: bool, x: f64, y: f64) -> f64
requires: true;
ensures: true;
body: {
    if flag { x } else { y }
};

enum Rate {
    Fixed(f64),
    None
}

atom as_rate(x: f64) -> Rate
requires: true;
ensures: true;
body: {
    Rate::Fixed(x)
};

// The match arm binds the f64 payload opaquely and returns it.
atom rate_value(r: Rate, fallback: f64) -> f64
requires: true;
ensures: true;
body: {
    match r { Rate::Fixed(v) => v, _ => fallback }
};

// An empty variant lowers to a constant aggregate whose f64 slot is a
// `double 0.0` leaf (BUG: constant aggregates must admit FP elements).
atom none_rate() -> Rate
requires: true;
ensures: true;
body: {
    Rate::None
};

atom main() -> i64
requires: true;
ensures: result == 1;
body: {
    1
};
