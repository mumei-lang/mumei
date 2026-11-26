struct S {
    xs: [i64]
}

struct TwoArrays {
    left: [i64],
    right: [i64]
}

struct Point {
    x: i64
}

struct RecursivePayloadPoint {
    e: RecursiveStructArrayE
}

enum C {
    One
}

struct NestedArrays {
    xs: [[i64]]
}

struct StructArrays {
    xs: [Point]
}

struct EnumArrays {
    xs: [C]
}

enum E {
    D(s: S),
    N
}

enum E2 {
    D(s: S),
    N
}

enum TwoArrayE {
    D(s: TwoArrays),
    N
}

enum SideEffectE {
    Pair(s: S, k: i64),
    N
}

enum NestedArrayE {
    D(s: NestedArrays),
    N
}

enum StructArrayE {
    D(s: StructArrays),
    N
}

enum EnumArrayE {
    D(s: EnumArrays),
    N
}

enum DirectArrayE {
    D(ps: [Point]),
    N
}

enum InnerArrayE {
    D(ps: [Point]),
    N
}

enum OuterArrayE {
    D(inner: InnerArrayE),
    N
}

struct WidePoint {
    n: u32
}

enum DirectWideArrayE {
    D(ps: [WidePoint]),
    N
}

enum NestedArrayPayloadE {
    D(ps: [[i64]]),
    N
}

enum RecursiveArrayPayloadE {
    N,
    D(ps: [Point], next: RecursiveArrayPayloadE)
}

enum RecursiveStructArrayE {
    N,
    D(ps: [RecursivePayloadPoint])
}

enum GenericArrayPayloadE<T> {
    N,
    D(ps: [Point], value: T)
}

atom struct_array_call_source() -> [Point]
requires: true;
ensures: len(result) == 1 && result[0].x == 1;
body: [Point { x: 1 }];

atom overclaimed_struct_array_length(s: S) -> i64
requires: len(s.xs) > 2;
ensures: result > 3;
body: {
    let e = E::D(s);
    match e {
        E::D(t) => len(t.xs),
        E::N => 0
    }
};

atom out_of_bounds_struct_array_index() -> i64
requires: true;
ensures: result == 0;
body: {
    let e = E::D(S { xs: [1, 2, 3] });
    match e {
        E::D(t) => {
            let v = t.xs;
            v[3]
        },
        E::N => 0
    }
};

atom different_struct_fields_keep_lengths_separate(left: [i64], right: [i64]) -> bool
requires: len(left) == 2 && len(right) == 3;
ensures: result == true;
body: {
    let e = TwoArrayE::D(TwoArrays { left: left, right: right });
    match e {
        TwoArrayE::D(t) => len(t.left) == len(t.right),
        TwoArrayE::N => false
    }
};

atom different_enum_payloads_keep_lengths_separate(left: [i64], right: [i64]) -> bool
requires: len(left) == 2 && len(right) == 3;
ensures: result == true;
body: {
    let e1 = E::D(S { xs: left });
    let e2 = E2::D(S { xs: right });
    match e1 {
        E::D(s1) => match e2 {
            E2::D(s2) => len(s1.xs) == len(s2.xs),
            E2::N => false
        },
        E::N => false
    }
};

atom later_argument_mutation_does_not_change_struct_array_payload() -> i64
requires: true;
ensures: result == 9;
body: {
    let xs = [1];
    let s = S { xs: xs };
    let e = SideEffectE::Pair(s, { xs[0] = 9; 0 });
    match e {
        SideEffectE::Pair(t, _) => {
            let v = t.xs;
            v[0]
        },
        SideEffectE::N => 0
    }
};

atom untagged_struct_array_read(e: E) -> i64
requires: true;
ensures: result == 0;
body: len(e.s.xs);

atom untagged_struct_array_write(e: E, ys: [i64]) -> i64
requires: true;
ensures: result == 0;
body: {
    e.s.xs = ys;
    0
};

atom whole_struct_array_slot_write_rejected(e: E, s2: S) -> i64
requires: match e {
    E::D(_) => true,
    E::N => false
};
ensures: result == 0;
body: {
    e.s = s2;
    0
};

atom nested_array_in_struct_payload(s: NestedArrays) -> i64
requires: true;
ensures: result == 0;
body: {
    let e = NestedArrayE::D(s);
    0
};

atom struct_array_in_struct_payload(s: StructArrays) -> i64
requires: true;
ensures: result == 0;
body: {
    let e = StructArrayE::D(s);
    0
};

atom enum_array_in_struct_payload(s: EnumArrays) -> i64
requires: true;
ensures: result == 0;
body: {
    let e = EnumArrayE::D(s);
    0
};

atom direct_struct_array_payload_different_elements(xs: [Point], ys: [Point]) -> bool
requires: len(xs) > 0 && len(ys) > 0 && xs[0].x != ys[0].x;
ensures: result == true;
body: DirectArrayE::D(xs) == DirectArrayE::D(ys);

atom nested_enum_struct_array_equality_rejects_different_contents(xs: [Point], ys: [Point]) -> bool
requires: len(xs) > 0 && len(ys) > 0 && xs[0].x != ys[0].x;
ensures: result == true;
body: OuterArrayE::D(InnerArrayE::D(xs)) == OuterArrayE::D(InnerArrayE::D(ys));

atom direct_struct_array_payload_stale_reconstruction(ps: [Point]) -> i64
requires: len(ps) > 0 && ps[0].x == 3;
ensures: result == 3;
body: {
    let e = DirectArrayE::D(ps);
    match e {
        DirectArrayE::D(_) => {
            let rebuilt = DirectArrayE::D([Point { x: 4 }]);
            match rebuilt {
                DirectArrayE::D(q) => q[0].x,
                DirectArrayE::N => 0
            }
        },
        DirectArrayE::N => 0
    }
};

atom call_result_array_leaf_cache_ignored_after_store() -> i64
requires: true;
ensures: result == 1;
body: {
    let qs = struct_array_call_source();
    qs[0].x = 9;
    let e = DirectArrayE::D(qs);
    match e {
        DirectArrayE::D(payload) => payload[0].x,
        DirectArrayE::N => 0
    }
};

atom match_result_array_leaf_cache_ignored_after_store() -> i64
requires: true;
ensures: result == 1;
body: {
    let source = [Point { x: 1 }];
    let qs = match C::One {
        C::One => source
    };
    qs[0].x = 9;
    let e = DirectArrayE::D(qs);
    match e {
        DirectArrayE::D(payload) => payload[0].x,
        DirectArrayE::N => 0
    }
};

atom match_result_shadowed_struct_array_does_not_reuse_outer(e: DirectArrayE, ps: [Point]) -> i64
requires:
    len(ps) > 0
    && ps[0].x == 1
    && match e {
        DirectArrayE::D(payload) => len(payload) > 0 && payload[0].x == 9,
        DirectArrayE::N => false
    };
ensures: result == 1;
body: {
    let selected = match e {
        DirectArrayE::D(ps) => ps,
        DirectArrayE::N => ps
    };
    selected[0].x
};

atom block_tail_array_payload_shared_write_rejected(ps: [Point]) -> i64
requires: len(ps) > 0;
ensures: result == 0;
body: {
    let e = DirectArrayE::D({ ps });
    ps[0].x = 1;
    0
};

atom alias_array_payload_shared_write_rejected(ps: [Point]) -> i64
requires: len(ps) > 0;
ensures: result == 0;
body: {
    let alias = ps;
    let e = DirectArrayE::D(alias);
    ps[0].x = 1;
    0
};

atom branch_array_payload_shared_write_rejected(ps: [Point], qs: [Point], choose: bool) -> i64
requires: len(ps) > 0 && len(qs) > 0;
ensures: result == 0;
body: {
    let e = DirectArrayE::D(if choose { ps } else { qs });
    ps[0].x = 1;
    0
};

atom tagged_direct_struct_array_payload_read_remains_closed(e: DirectArrayE) -> i64
requires: match e {
    DirectArrayE::D(ps) => len(ps) > 0 && ps[0].x == 12,
    DirectArrayE::N => false
};
ensures: result == 12;
body: {
    let payload = e.ps;
    payload[0].x
};

atom direct_struct_array_u32_constructor_domain() -> bool
requires: true;
ensures: result == true;
body: {
    let e = DirectWideArrayE::D([WidePoint { n: 4294967296 }]);
    e == e
};

atom direct_struct_array_payload_field_write_rejected(ps: [Point]) -> i64
requires: len(ps) > 0;
ensures: result == 0;
body: {
    let e = DirectArrayE::D(ps);
    match e {
        DirectArrayE::D(q) => {
            q[0].x = 1;
            0
        },
        DirectArrayE::N => 0
    }
};

atom direct_struct_array_payload_whole_element_write_rejected(ps: [Point]) -> i64
requires: len(ps) > 0;
ensures: result == 0;
body: {
    let e = DirectArrayE::D(ps);
    match e {
        DirectArrayE::D(q) => {
            q[0] = Point { x: 1 };
            0
        },
        DirectArrayE::N => 0
    }
};

atom direct_struct_array_payload_alias_write_rejected(ps: [Point]) -> i64
requires: len(ps) > 0;
ensures: result == 0;
body: {
    let e = DirectArrayE::D(ps);
    match e {
        DirectArrayE::D(q) => {
            let alias = q;
            alias[0].x = 1;
            0
        },
        DirectArrayE::N => 0
    }
};

atom direct_struct_array_payload_callee_write_rejected(ps: [Point]) -> i64
requires: len(ps) > 0;
ensures: result == 0;
body: {
    let e = DirectArrayE::D(ps);
    match e {
        DirectArrayE::D(q) => direct_array_writer(q),
        DirectArrayE::N => 0
    }
};

atom direct_array_writer(ps: [Point]) -> i64
requires: len(ps) > 0;
ensures: result == 0;
body: {
    ps[0].x = 1;
    0
};

atom direct_struct_array_nested_array_remains_closed(ps: [[i64]]) -> i64
requires: true;
ensures: result == 0;
body: {
    let e = NestedArrayPayloadE::D(ps);
    0
};

atom direct_struct_array_generic_enum_remains_closed(ps: [Point]) -> i64
requires: true;
ensures: result == 0;
body: {
    let e = GenericArrayPayloadE::D(ps, 1);
    0
};
atom direct_struct_array_recursive_enum_remains_closed(ps: [Point], next: RecursiveArrayPayloadE) -> i64
requires: true;
ensures: result == 0;
body: {
    let e = RecursiveArrayPayloadE::D(ps, next);
    match e {
        RecursiveArrayPayloadE::D(_, _) => 0,
        RecursiveArrayPayloadE::N => 0
    }
};

atom recursive_struct_enum_array_payload_fails_closed(ps: [RecursivePayloadPoint]) -> i64
requires: true;
ensures: result == 0;
body: {
    let e = RecursiveStructArrayE::D(ps);
    0
};
