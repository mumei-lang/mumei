enum E {
    A,
    B(next: E)
}

enum Choice {
    C,
    D
}

struct P {
    x: i64,
    y: i64
}

struct Q {
    x: i64,
    y: i64
}

struct EnumField {
    value: E
}

struct ArrayField {
    values: [i64]
}

struct GenericField<T> {
    value: T
}

struct U64Value {
    value: u64
}

struct S {
    values: [P]
}

atom mk_array_with_eleven() -> [P]
requires: true;
ensures: len(result) == 1 && result[0].x == 11;
body: [P { x: 11, y: 11 }];

atom mk_one() -> [P]
requires: true;
ensures: len(result) == 1 && result[0].x == 1;
body: [P { x: 1, y: 0 }];

atom mk_two() -> [P]
requires: true;
ensures: len(result) == 1 && result[0].x == 2;
body: [P { x: 2, y: 0 }];

atom require_second_one(qs: [P], b: [P]) -> bool
requires: len(qs) > 0 && len(b) > 0 && b[0].x == 1;
ensures: result == true;
body: true;

atom require_first_two(a: [P], b: [P]) -> bool
requires: len(a) > 0 && len(b) > 0 && a[0].x == 2;
ensures: result == true;
body: true;

atom call_block_param_shadow(xs: [P]) -> bool
requires: len(xs) > 0 && xs[0].x == 1;
ensures: result == true;
body: require_second_one(xs, {
    let qs = mk_two();
    qs
});

atom call_two_block_args() -> bool
requires: true;
ensures: result == true;
body: require_first_two({
    let qs = mk_one();
    qs
}, {
    let qs = mk_two();
    qs
});

atom call_if_param_shadow(xs: [P], c: bool) -> bool
requires: len(xs) > 0 && xs[0].x == 1;
ensures: result == true;
body: require_second_one(xs, if c {
    let qs = mk_two();
    qs
} else {
    let qs = mk_two();
    qs
});

atom call_two_if_args(c: bool, d: bool) -> bool
requires: true;
ensures: result == true;
body: require_first_two(if c {
    let qs = mk_one();
    qs
} else {
    let qs = mk_one();
    qs
}, if d {
    let qs = mk_two();
    qs
} else {
    let qs = mk_two();
    qs
});

atom call_match_param_shadow(xs: [P], c: Choice) -> bool
requires: len(xs) > 0 && xs[0].x == 1;
ensures: result == true;
body: require_second_one(xs, match c {
    Choice::C => {
        let qs = mk_two();
        qs
    },
    Choice::D => {
        let qs = mk_two();
        qs
    }
});

atom call_two_match_args(c: Choice, d: Choice) -> bool
requires: true;
ensures: result == true;
body: require_first_two(match c {
    Choice::C => {
        let qs = mk_one();
        qs
    },
    Choice::D => {
        let qs = mk_one();
        qs
    }
}, match d {
    Choice::C => {
        let qs = mk_two();
        qs
    },
    Choice::D => {
        let qs = mk_two();
        qs
    }
});

atom stale_read(ps: [P]) -> i64
requires: len(ps) > 1 && ps[1].x == 3;
ensures: result == 3;
body: {
    ps[1] = P { x: 1, y: 2 };
    ps[1].x
};

atom stale_block_tail_after_store() -> i64
requires: true;
ensures: result == 11;
body: {
    let ps = {
        let qs = mk_array_with_eleven();
        qs[0] = P { x: 9, y: 9 };
        qs
    };
    ps[0].x
};

atom unconstrained_field(ps: [P]) -> i64
requires: len(ps) > 0;
ensures: result > 0;
body: ps[0].x;

atom independent_fields(ps: [P]) -> i64
requires: len(ps) > 0 && ps[0].x > 5;
ensures: result > 5;
body: ps[0].y;

atom u64_array_wrap(values: [U64Value]) -> i64
requires: len(values) > 0;
ensures: result == 1;
body: if values[0].value + 1 > values[0].value { 1 } else { 0 };

atom u64_array_binding_wrap(values: [U64Value]) -> i64
requires: len(values) > 0;
ensures: result == 1;
body: {
    let value = values[0];
    if value.value + 1 > value.value { 1 } else { 0 }
};

atom bare_element_equality(ps: [P]) -> bool
requires: len(ps) > 1;
ensures: true;
body: ps[0] == ps[1];

atom mismatched_struct_store(ps: [P], q: Q) -> i64
requires: len(ps) > 0;
ensures: true;
body: {
    ps[0] = q;
    ps[0].x
};

atom enum_leaf(values: [EnumField]) -> i64
requires: len(values) > 0;
ensures: true;
body: values[0].value.tag;

atom array_leaf(values: [ArrayField]) -> i64
requires: len(values) > 0;
ensures: true;
body: len(values[0].values);

atom unresolved_generic_leaf(values: [GenericField]) -> i64
requires: len(values) > 0;
ensures: true;
body: values[0].value;

atom fresh_array(x: i64) -> [P]
requires: true;
ensures: len(result) == 1 && result[0].x == x;
body: [P { x: x, y: 0 }];

atom shadowed_fresh_array_bindings(a: i64, b: i64, c: bool, d: bool) -> i64
requires: true;
ensures: result == 7;
body: {
    let qs = if c { fresh_array(a) } else { fresh_array(a) };
    let first = qs[0].x;
    let qs = if d { fresh_array(b) } else { fresh_array(b) };
    let second = qs[0].x;
    first - second + 7
};

atom nested_struct_array(values: [[P]]) -> i64
requires: len(values) > 0;
ensures: true;
body: len(values[0]);

atom shared_field_array(s: S) -> i64
requires: len(s.values) > 0;
ensures: true;
body: {
    let values = s.values;
    values[0] = P { x: 1, y: 2 };
    values[0].x
};
