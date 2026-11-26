struct Inner {
    a: i64,
    b: i64
}

struct Outer {
    inner: Inner,
    c: i64
}

atom read_inner(o: Outer) -> i64
requires: o.inner.a >= 0;
ensures: result >= 0;
body: {
    o.inner.a
};

atom bump(o: Outer) -> Outer
requires: o.c >= 0 && o.c <= 100 && o.inner.a >= 0;
ensures: result.c >= 1 && result.inner.a == o.inner.a;
body: {
    o.c = o.c + 1;
    o
};

atom set_inner(o: Outer, w: Inner) -> Outer
requires: w.a >= 0;
ensures: result.inner.a == w.a && result.inner.b == w.b;
body: {
    o.inner = w;
    o
};

atom make_inner(a: i64) -> Inner
requires: a >= 0;
ensures: result.a == a;
body: {
    Inner { a: a, b: a + 1 }
};

atom main() -> i64
requires: true;
ensures: result >= 0;
body: {
    let o = Outer { inner: Inner { a: 3, b: 4 }, c: 5 };
    let s = set_inner(o, Inner { a: 7, b: 8 });
    s.inner.a + s.inner.b - o.inner.a - o.inner.b
};

struct Deep3 {
    m: Inner,
    x: i64
}

struct Top {
    mid: Deep3,
    t: i64
}

atom swap_mid(tp: Top, w: Deep3) -> Top
requires: w.m.a >= 0;
ensures: result.mid.m.a == w.m.a && result.mid.x == w.x;
body: {
    tp.mid = w;
    tp
};

atom mk_outer(x: i64) -> Top
requires: x >= 0;
ensures: result.mid.m.a == x;
body: {
    Top { mid: Deep3 { m: Inner { a: x, b: x }, x: x }, t: x }
};

atom call_nested_leaf() -> i64
requires: true;
ensures: result == 3;
body: {
    mk_outer(3).mid.m.a
};

atom diff_call_leaves() -> i64
requires: true;
ensures: result == -1;
body: {
    mk_outer(1).mid.m.a - mk_outer(2).mid.m.a
};
