enum L {
    Nil,
    Cons(h: i64, t: L)
}

struct Holder {
    value: L
}

atom f(holder: Holder) -> i64
requires: true;
ensures: result == 0;
body: {
    0
};
