// A widened leaf bound stops at the declared width — `x >= 1` under
// `W::A(x)` is NOT free: `W::A(0)` is a legitimate input, so this atom
// must fail verification rather than report the leaf strictly positive.

enum W {
    A(u64),
    B(i32),
    C(u32),
    D
}

atom too_tight(e: W) -> i64
requires: true;
ensures: result == 1;
body: {
    match e {
        W::A(x) => if x >= 1 { 1 } else { 0 },
        _ => 1
    }
}

// Nested payloads: the inner `u64` bound also stops at the declared
// width — `x >= 1` inside `Outer::Wrap(Inner::A(x))` is refuted.
enum Inner {
    A(u64),
    B
}

enum Outer {
    Wrap(Inner),
    C
}

atom nested_too_tight(e: Outer) -> i64
requires: true;
ensures: result == 1;
body: {
    match e {
        Outer::Wrap(i) => match i {
            Inner::A(x) => if x >= 1 { 1 } else { 0 },
            _ => 1
        },
        _ => 1
    }
}
