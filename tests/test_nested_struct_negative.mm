struct Inner {
    a: i64,
    b: i64
}

struct Outer {
    inner: Inner,
    c: i64
}

atom bad_read(o: Outer) -> i64
requires: o.inner.a >= 0;
ensures: result >= 1;
body: {
    o.inner.a
};

atom bad_write(o: Outer, w: Inner) -> Outer
requires: true;
ensures: result.inner.a == w.b;
body: {
    o.inner = w;
    o
};

atom bad_result(o: Outer) -> Outer
requires: o.inner.a >= 0;
ensures: result.inner.a > o.inner.a;
body: {
    o
};

struct Deep3 {
    m: Inner,
    x: i64
}

struct Top {
    mid: Deep3,
    t: i64
}

atom mk_outer(x: i64) -> Top
requires: x >= 0;
ensures: result.mid.m.a == x;
body: {
    Top { mid: Deep3 { m: Inner { a: x, b: x }, x: x }, t: x }
};

atom bad_call_leaf() -> i64
requires: true;
ensures: result == 0;
body: {
    mk_outer(1).mid.m.a - mk_outer(2).mid.m.a
};

atom bad_deep_write(t: Top, w: Deep3) -> Top
requires: w.m.a >= 0;
ensures: result.mid.m.a == w.m.a + 1;
body: {
    t.mid = w;
    t
};
