struct Inner {
    x: i64
}

struct S {
    inner: Inner,
    inner_x: i64
}

atom scalar_path_false_claim(s: S) -> i64
    requires: true;
    ensures: s.inner.x == s.inner_x;
    body: 0
