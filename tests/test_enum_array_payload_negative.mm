struct Point {
    x: i64
}

struct WrappedArray {
    values: [i64]
}

enum Ev {
    Data(xs: [i64]),
    Idle
}

enum SideEffectEv {
    Pair(xs: [i64], k: i64),
    Idle
}

enum NestedArray {
    Data(xs: [[i64]]),
    Idle
}

enum StructArray {
    Data(xs: [Point]),
    Idle
}

enum StructPayload {
    Data(value: WrappedArray),
    Idle
}

atom false_length_claim(xs: [i64]) -> i64
requires: len(xs) > 2;
ensures: result > 3;
body: {
    let e = Ev::Data(xs);
    match e {
        Ev::Data(ys) => len(ys),
        Ev::Idle => 0
    }
};

atom out_of_bounds_payload_read() -> i64
requires: true;
ensures: result >= 0;
body: {
    let e = Ev::Data([1, 2, 3]);
    match e {
        Ev::Data(ys) => ys[3],
        Ev::Idle => 0
    }
};

atom independent_payload_lengths(a: [i64], b: [i64]) -> bool
requires: len(a) == 2 && len(b) == 3;
ensures: result == true;
body: {
    let e1 = Ev::Data(a);
    let e2 = Ev::Data(b);
    match e1 {
        Ev::Data(xs) => match e2 {
            Ev::Data(ys) => len(xs) == len(ys),
            Ev::Idle => false
        },
        Ev::Idle => false
    }
};

atom different_direct_payload_roots_keep_lengths_separate(e1: Ev, e2: Ev) -> bool
requires:
    match e1 {
        Ev::Data(xs) => len(xs) == 2,
        Ev::Idle => false
    }
    &&
    match e2 {
        Ev::Data(ys) => len(ys) == 3,
        Ev::Idle => false
    };
ensures: result == true;
body: {
    let a = e1.xs;
    let b = e2.xs;
    len(a) == len(b)
};

atom stale_array_constructor_uses_updated_elements(xs: [i64]) -> i64
requires: len(xs) > 0 && xs[0] == 1;
ensures: result == 1;
body: {
    let a = xs;
    a[0] = 5;
    let e = Ev::Data(a);
    match e {
        Ev::Data(ys) => ys[0],
        Ev::Idle => 0
    }
};

atom later_argument_mutation_does_not_change_array_payload() -> i64
requires: true;
ensures: result == 9;
body: {
    let xs = [1];
    let e = SideEffectEv::Pair(xs, { xs[0] = 9; 0 });
    match e {
        SideEffectEv::Pair(ys, _) => ys[0],
        SideEffectEv::Idle => 0
    }
};

atom stale_array_tagged_write_uses_updated_elements(e: Ev, xs: [i64]) -> i64
requires:
    match e {
        Ev::Data(_) => true,
        Ev::Idle => false
    }
    && len(xs) > 0 && xs[0] == 1;
ensures: result == 1;
body: {
    let a = xs;
    a[0] = 5;
    e.xs = a;
    match e {
        Ev::Data(ys) => ys[0],
        Ev::Idle => 0
    }
};

atom match_tail_uses_outer_shadowed_length(ys: [i64], xs: [i64]) -> i64
requires: len(ys) == 5 && len(xs) == 3;
ensures: result == 5;
body: {
    let e = Ev::Data(xs);
    let r = match e {
        Ev::Data(ys) => ys,
        Ev::Idle => xs
    };
    len(r)
};

atom array_inside_struct_payload(values: [i64]) -> i64
requires: true;
ensures: result == 0;
body: {
    let e = StructPayload::Data(WrappedArray { values: values });
    0
};

atom nested_array_payload(xs: [[i64]]) -> i64
requires: true;
ensures: result == 0;
body: {
    let e = NestedArray::Data(xs);
    0
};

atom struct_array_payload(xs: [Point]) -> i64
requires: true;
ensures: result == 0;
body: {
    let e = StructArray::Data(xs);
    0
};

atom untagged_array_read(e: Ev) -> i64
requires: true;
ensures: result == 0;
body: len(e.xs);
