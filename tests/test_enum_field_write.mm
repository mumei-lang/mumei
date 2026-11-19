// Enum payload writes `e.p = v` (`pure-i64-composite/v16`).
// A payload write is defined only where the tag provably carries the
// one variant owning `p`: the verifier rebuilds `e = V(sel_0(e),…,v,…)`
// under an `is-V(e)` obligation, and the emitter lowers it to an
// `insertvalue` at aggregate index `1 + i` (slot i after the tag).

enum Chan {
    Tcp(port: i64),
    Udp
}

enum Pair {
    Both(l: i64, r: i64),
    Empty
}

// A written payload reads its new value; the tag keeps its proof.
atom write_slot(p: i64) -> Chan
requires: p > 0;
ensures: result == Chan::Tcp(p);
body: {
    let c = Chan::Tcp(0);
    c.port = p;
    c
};

// A payload write in tail position yields the written leaf value.
atom write_slot_tail(p: i64) -> i64
requires: true;
ensures: result == p;
body: {
    let c = Chan::Tcp(0);
    c.port = p
};

// Under match narrowing the write lands on the bound scrutinee —
// `is-Tcp(c)` holds inside the arm, so the write is defined.
atom write_match(c: Chan, p: i64) -> Chan
requires: true;
ensures: match c { Chan::Tcp(w) => result == Chan::Tcp(p), _ => result == c };
body: {
    match c {
        Chan::Tcp(port) => { c.port = p; c }
        _ => c
    }
};

// Writing one payload slot keeps the sibling slot's projection.
atom write_keeps_sibling(l: i64, r: i64) -> Pair
requires: true;
ensures: match result { Pair::Both(l2, r2) => l2 == l && r2 == r, _ => false };
body: {
    let e = Pair::Both(l, 0);
    e.r = r;
    e
};

// A read of the untouched slot tracks the original payload.
atom read_sibling(l: i64, r: i64) -> i64
requires: true;
ensures: result == r;
body: {
    let e = Pair::Both(l, r);
    e.l = 0;
    match e {
        Pair::Both(l2, r2) => r2,
        _ => -1
    }
};

// A struct declaring the same field name must not hijack an enum
// payload write — the receiver's declared type wins (emit side).
struct Shadow { l: i64 }

atom write_shadowed_field(l: i64, r: i64) -> Pair
requires: true;
ensures: match result { Pair::Both(l2, r2) => l2 == l && r2 == r, _ => false };
body: {
    let e = Pair::Both(0, r);
    e.l = l;
    e
};
