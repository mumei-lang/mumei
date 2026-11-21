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
