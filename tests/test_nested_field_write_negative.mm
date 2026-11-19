// Nested field write rejections (`pure-i64-composite/v15`).
// Every atom here must fail: wrong postconditions over the written
// leaf, a mid-path leaf dead-end, an unknown nested field, and a
// leaf write that violates the enclosing struct's invariant.

struct Inner {
    a: i64,
    b: i64
}

struct Outer {
    inner: Inner,
    c: i64
}

struct Guard {
    lo: i64,
    hi: i64,
    invariant: self.lo <= self.hi
}

struct Shielded {
    g: Guard,
    tag: i64
}

// The written leaf does not keep its input-time value.
atom bad_keeps_old(o: Outer, v: i64) -> i64
requires: o.inner.a == 0 && v == 5;
ensures: result == 0;
body: {
    o.inner.a = v;
    o.inner.a
};

// A mid-path leaf dead-ends the descent: `c` is i64, so `o.c.x`
// names no field.
atom bad_mid_path_leaf(o: Outer) -> i64
requires: true;
ensures: result == 1;
body: {
    o.c.x = 1;
    o.c.x
};

// An unknown nested field name is rejected.
atom bad_unknown_leaf(o: Outer) -> i64
requires: true;
ensures: result == 1;
body: {
    o.inner.zzz = 1;
    o.inner.zzz
};

// A leaf write that breaks the enclosing struct's invariant is
// refused — `lo > hi` must not be constructible through the
// nested-write path either.
atom bad_breaks_invariant(s: Shielded, v: i64) -> Shielded
requires: v > s.g.hi;
ensures: true;
body: {
    s.g.lo = v;
    s
};

// The sibling leaf does NOT take the written value — only the
// named path is updated.
atom bad_sibling_changed(o: Outer, v: i64) -> i64
requires: o.inner.b == 0 && v == 5;
ensures: result == 5;
body: {
    o.inner.a = v;
    o.inner.b
};

// A callee writing a nested field must havoc the caller's nested
// leaves — asserting `ensures q.inner.a == 0` against the input slot
// (5) must not collapse the caller's assumptions into a vacuous
// proof of a false postcondition.
atom nested_setter(q: Outer) -> Outer
requires: true;
ensures: q.inner.a == 0;
body: {
    q.inner.a = 0;
    q
};

atom bad_callee_havoc(q: Outer) -> i64
requires: q.inner.a == 5;
ensures: result == 999;
body: {
    let q2 = nested_setter(q);
    1
};
