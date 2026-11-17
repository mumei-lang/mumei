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
