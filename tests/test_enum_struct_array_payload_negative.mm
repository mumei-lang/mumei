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
