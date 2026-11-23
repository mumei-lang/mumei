struct S {
    k: i64,
    xs: [i64]
}

struct Inner {
    xs: [i64]
}

struct Outer {
    inner: Inner
}

struct TwoArrays {
    left: [i64],
    right: [i64]
}

struct Words {
    xs: [u64]
}

struct Pt {
    x: i64,
    c: Color
}

struct CachePoint {
    x: i64
}

struct WidePt {
    x: i64,
    n: u32
}

enum Color {
    R,
    G(v: u32)
}

enum DirectArrayE {
    D(ps: [Pt]),
    N
}

enum CacheArrayE {
    D(ps: [CachePoint]),
    N
}

enum InnerArrayE {
    D(ps: [Pt]),
    N
}

enum OuterArrayE {
    D(inner: InnerArrayE),
    N
}

enum DirectWideE {
    D(ps: [WidePt]),
    N
}

enum E {
    D(s: S),
    N
}

enum NestedE {
    D(o: Outer),
    N
}

enum TwoArrayE {
    D(s: TwoArrays),
    N
}

enum WordE {
    D(s: Words),
    N
}

atom struct_payload_length_repro(s: S) -> i64
requires: len(s.xs) > 2;
ensures: result > 2;
body: {
    let e = E::D(s);
    match e {
        E::D(t) => len(t.xs),
        E::N => 3
    }
};

atom struct_literal_payload_length() -> i64
requires: true;
ensures: result == 3;
body: {
    let e = E::D(S { k: 1, xs: [1, 2, 3] });
    match e {
        E::D(t) => len(t.xs),
        E::N => 0
    }
};

atom struct_payload_preserves_element_fact(xs: [i64]) -> i64
requires: len(xs) > 0 && xs[0] == 7;
ensures: result == 7;
body: {
    let e = E::D(S { k: 1, xs: xs });
    match e {
        E::D(t) => {
            let v = t.xs;
            v[0]
        },
        E::N => 0
    }
};

atom struct_payload_index_is_in_bounds(xs: [i64]) -> i64
requires: len(xs) > 2;
ensures: result == xs[2];
body: {
    let e = E::D(S { k: 1, xs: xs });
    match e {
        E::D(t) => {
            let v = t.xs;
            v[2]
        },
        E::N => 0
    }
};

atom conditional_struct_payload_length(c: bool, s1: S, s2: S) -> i64
requires: len(s1.xs) == 2 && len(s2.xs) == 2;
ensures: result == 2;
body: {
    let e = E::D(if c { s1 } else { s2 });
    match e {
        E::D(t) => len(t.xs),
        E::N => 0
    }
};

atom nested_struct_array_payload_length(s: Inner) -> i64
requires: len(s.xs) > 1;
ensures: result > 1;
body: {
    let e = NestedE::D(Outer { inner: s });
    match e {
        NestedE::D(t) => len(t.inner.xs),
        NestedE::N => 0
    }
};

atom tagged_struct_array_direct_read(e: E) -> i64
requires: match e {
    E::D(s) => len(s.xs) == 3,
    E::N => false
};
ensures: result == 3;
body: {
    let a = e.s.xs;
    len(a)
};

atom tagged_struct_value_direct_read(e: E) -> i64
requires: match e {
    E::D(s) => len(s.xs) == 4,
    E::N => false
};
ensures: result == 4;
body: {
    let q = e.s;
    len(q.xs)
};

atom repeated_struct_array_reads_keep_length(e: E) -> bool
requires: match e {
    E::D(s) => len(s.xs) == 4,
    E::N => false
};
ensures: result == true;
body: {
    let a = e.s.xs;
    let b = e.s.xs;
    len(a) == len(b)
};

atom tagged_struct_array_field_write(e: E, ys: [i64]) -> i64
requires:
    match e {
        E::D(_) => true,
        E::N => false
    }
    && len(ys) == 5;
ensures: result == 5;
body: {
    e.s.xs = ys;
    let q = e.s;
    len(q.xs)
};

atom matched_struct_array_length_is_nonnegative(e: E) -> i64
requires: true;
ensures: result >= 0;
body: match e {
    E::D(t) => len(t.xs),
    E::N => 0
};

atom u64_struct_array_payload(xs: [u64]) -> u64
requires: len(xs) > 0 && xs[0] == 7;
ensures: result == 7;
body: {
    let e = WordE::D(Words { xs: xs });
    match e {
        WordE::D(t) => {
            let v = t.xs;
            v[0]
        },
        WordE::N => 0
    }
};

atom independent_array_fields_keep_lengths(left: [i64], right: [i64]) -> i64
requires: len(left) == 2 && len(right) == 3;
ensures: result == 5;
body: {
    let e = TwoArrayE::D(TwoArrays { left: left, right: right });
    match e {
        TwoArrayE::D(t) => len(t.left) + len(t.right),
        TwoArrayE::N => 0
    }
};

atom struct_array_length_contract_call(e: E) -> i64
requires: match e {
    E::D(s) => len(s.xs) > 0,
    E::N => false
};
ensures: result > 0;
body: {
    let q = e.s;
    struct_array_length_callee(q)
};

atom struct_array_length_callee(p: S) -> i64
requires: len(p.xs) > 0;
ensures: result > 0;
body: len(p.xs);

atom direct_struct_array_payload_match(ps: [Pt]) -> i64
requires: len(ps) > 0 && ps[0].x == 3;
ensures: result == 3;
body: {
    let e = DirectArrayE::D(ps);
    match e {
        DirectArrayE::D(q) => q[0].x,
        DirectArrayE::N => 0
    }
};

atom direct_struct_array_payload_literal() -> i64
requires: true;
ensures: result == 3;
body: {
    let e = DirectArrayE::D([Pt { x: 3, c: Color::R }]);
    match e {
        DirectArrayE::D(ps) => ps[0].x,
        DirectArrayE::N => 0
    }
};

atom direct_struct_array_payload_roundtrip(ps: [Pt]) -> bool
requires: len(ps) > 0 && ps[0].x == 3;
ensures: result == true;
body: {
    let e = DirectArrayE::D(ps);
    match e {
        DirectArrayE::D(q) => q[0].x == 3,
        DirectArrayE::N => false
    }
};

atom direct_struct_array_payload_forall(ps: [Pt]) -> bool
requires: forall(i, 0, len(ps), ps[i].x >= 0);
ensures: result == true;
body: {
    let e = DirectArrayE::D(ps);
    match e {
        DirectArrayE::D(q) => forall(i, 0, len(q), q[i].x >= 0),
        DirectArrayE::N => true
    }
};

atom direct_struct_array_enum_leaf(ps: [Pt]) -> u32
requires: len(ps) > 0 && ps[0].c == Color::G(7);
ensures: result == 7;
body: {
    let e = DirectArrayE::D(ps);
    match e {
        DirectArrayE::D(q) => match q[0].c {
            Color::G(v) => v,
            Color::R => 0
        },
        DirectArrayE::N => 0
    }
};

atom direct_struct_array_payload_callee(ps: [Pt]) -> i64
requires: len(ps) > 0 && ps[0].x == 3;
ensures: result == 3;
body: {
    let e = DirectArrayE::D(ps);
    match e {
        DirectArrayE::D(q) => struct_array_first_x(q),
        DirectArrayE::N => 0
    }
};

atom match_result_struct_array_leaf_snapshot() -> i64
requires: true;
ensures: result == 9;
body: {
    let outer = [Pt { x: 1, c: Color::R }];
    let e = DirectArrayE::D([Pt { x: 9, c: Color::R }]);
    let selected = match e {
        DirectArrayE::D(payload) => payload,
        DirectArrayE::N => outer
    };
    selected[0].x
};

atom cache_array_call_source() -> [CachePoint]
requires: true;
ensures: len(result) == 1 && result[0].x == 1;
body: [CachePoint { x: 1 }];

atom call_result_array_leaf_cache_tracks_store() -> i64
requires: true;
ensures: result == 9;
body: {
    let qs = cache_array_call_source();
    qs[0].x = 9;
    let e = CacheArrayE::D(qs);
    match e {
        CacheArrayE::D(payload) => payload[0].x,
        CacheArrayE::N => 0
    }
};

atom match_result_array_leaf_cache_tracks_store() -> i64
requires: true;
ensures: result == 9;
body: {
    let source = [CachePoint { x: 1 }];
    let qs = match Color::R {
        Color::R => source,
        Color::G(_) => source
    };
    qs[0].x = 9;
    let e = CacheArrayE::D(qs);
    match e {
        CacheArrayE::D(payload) => payload[0].x,
        CacheArrayE::N => 0
    }
};

atom direct_struct_array_payload_equality_unchanged(ps: [Pt]) -> bool
requires: len(ps) > 0;
ensures: result == true;
body: DirectArrayE::D(ps) == DirectArrayE::D(ps);

atom nested_enum_struct_array_equality_is_extensional(xs: [Pt], ys: [Pt]) -> bool
requires: InnerArrayE::D(xs) == InnerArrayE::D(ys);
ensures: result == true;
body: OuterArrayE::D(InnerArrayE::D(xs)) == OuterArrayE::D(InnerArrayE::D(ys));

atom struct_array_first_x(ps: [Pt]) -> i64
requires: len(ps) > 0;
ensures: result == ps[0].x;
body: ps[0].x;
