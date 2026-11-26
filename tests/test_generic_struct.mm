// Generic struct declarations (`pure-i64-composite/v9`).
// Generic `struct` declarations are admitted by instantiation: every
// concrete `Pair<i64>`/`Box<Pair<i64>>` use is monomorphized into an
// ordinary declaration before verification.

struct Pair<T> {
    a: T,
    b: T
}

struct Box<T> {
    inner: T
}

// A literal explicitly instantiates its declaration.
atom mk_pair(a: i64) -> Pair<i64>
requires: a >= 0;
ensures: result.a == a && result.b == a;
body: {
    Pair<i64>{a: a, b: a}
};

atom pair_sum(p: Pair<i64>) -> i64
requires: p.a >= 0 && p.b >= 0 && p.a + p.b <= 100;
ensures: result == p.a + p.b;
body: {
    p.a + p.b
};

atom swap_pair(p: Pair<i64>) -> Pair<i64>
requires: true;
ensures: result.a == p.b && result.b == p.a;
body: {
    Pair<i64>{a: p.b, b: p.a}
};

// A generic instantiation may itself be a field type.
atom box_pair(p: Pair<i64>) -> Box<Pair<i64>>
requires: true;
ensures: result.inner.a == p.a;
body: {
    Box<Pair<i64>>{inner: p}
};

atom unbox_a(b: Box<Pair<i64>>) -> i64
requires: true;
ensures: result == b.inner.a;
body: {
    b.inner.a
};

// A call result of instantiated type flattens the same way.
atom call_leaf(a: i64) -> i64
requires: a >= 0 && a <= 50;
ensures: result == a + a;
body: {
    pair_sum(mk_pair(a))
};
