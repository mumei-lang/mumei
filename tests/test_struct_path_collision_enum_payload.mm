struct Inner {
    x: i64
}

struct S {
    inner: Inner,
    inner_x: i64
}

enum E {
    A(s: S)
}

atom enum_payload_path_false_claim(e: E) -> i64
    requires: true;
    ensures: e.s.inner.x == e.s.inner_x;
    body: e.s.inner.x
