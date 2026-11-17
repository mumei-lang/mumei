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
