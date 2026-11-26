// Nested field writes `q.inner.a = v` (`pure-i64-composite/v15`).
// A leaf write rebuilds only the path to that leaf: sibling leaves
// keep their facts at every depth, struct invariants and `where`
// field constraints are re-checked against the merged fields, and a
// tail-position write yields the written leaf value.

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

struct Deep3 {
    m: Inner,
    x: i64
}

struct Top {
    mid: Deep3,
    t: i64
}

// A written nested leaf reads its new value; its sibling leaf and the
// outer-level sibling keep their input-time facts.
atom write_leaf(o: Outer, v: i64) -> Outer
requires: v >= 0;
ensures: result.inner.a == v && result.inner.b == o.inner.b && result.c == o.c;
body: {
    o.inner.a = v;
    o
};

// A nested write in tail position yields the written leaf value.
atom write_leaf_tail(o: Outer, v: i64) -> i64
requires: v >= 0;
ensures: result == v;
body: {
    o.inner.a = v
};

// A conditional nested write merges per-branch leaf values; the
// untouched sibling still sees its input fact on either arm.
atom write_branch(o: Outer, v: i64, w: i64) -> Outer
requires: v >= 0 && w >= 0;
ensures: result.inner.a >= 0 && result.inner.b == o.inner.b;
body: {
    if o.c > 0 {
        o.inner.a = v;
    } else {
        o.inner.a = w;
    }
    o
};

// Three levels deep: the write descends `tp.mid.m` and rebuilds both
// enclosing aggregates — mid.x and t keep their facts.
atom write_deep(tp: Top, v: i64) -> Top
requires: v >= 0;
ensures: result.mid.m.a == v && result.mid.m.b == tp.mid.m.b
    && result.mid.x == tp.mid.x && result.t == tp.t;
body: {
    tp.mid.m.a = v;
    tp
};

// A mid-level leaf write keeps the declared invariant of the
// enclosing struct when the new value satisfies it.
atom write_keeps_inner_invariant(s: Shielded, v: i64) -> Shielded
requires: v >= s.g.lo;
ensures: result.g.hi == v && result.g.lo == s.g.lo && result.tag == s.tag;
body: {
    s.g.hi = v;
    s
};

// The leaf's own `where` constraint gates the write — `lo` accepts
// any value (its clause is on hi) but a fresh invariant check keeps
// `hi` at least `lo`.
atom write_respects_constraints(s: Shielded, v: i64) -> i64
requires: v <= s.g.hi;
ensures: result == s.g.lo;
body: {
    s.g.lo = v;
    s.g.lo
};

// A struct-typed leaf under a nested path still takes the desugared
// whole-subtree write for deeper paths — here the LEAF is written
// through two struct levels at once.
atom write_mid_subtree(tp: Top, w: Inner) -> Top
requires: w.a >= 0;
ensures: result.mid.m.a == w.a && result.mid.x == tp.mid.x;
body: {
    tp.mid.m = w;
    tp
};

// A tail-position write whose RHS reads the same leaf evaluates the
// RHS once, before the write — result equals the post-write leaf
// (input + 1), not the post-write leaf plus one again.
atom write_self_read_tail(o: Outer) -> i64
requires: o.inner.a >= 0 && o.inner.a < 100;
ensures: result == o.inner.a;
body: {
    o.inner.a = o.inner.a + 1
};

struct NonNeg {
    a: i64 where v >= 0
}

struct HoldNonNeg {
    n: NonNeg,
    t: i64
}

// A nested write inside a branch only has to satisfy the leaf's
// `where` clause under that branch's path condition — the else arm
// never reaches the write.
atom write_branch_constraint(h: HoldNonNeg, v: i64) -> HoldNonNeg
requires: true;
ensures: result.n.a >= 0;
body: {
    if v >= 0 {
        h.n.a = v;
    } else {
        h.n.a = 0;
    }
    h
};
