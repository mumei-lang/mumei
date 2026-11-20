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
