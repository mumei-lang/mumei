enum E {
    A,
    B
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

atom stale_read(ps: [P]) -> i64
requires: len(ps) > 1 && ps[1].x == 3;
ensures: result == 3;
body: {
    ps[1] = P { x: 1, y: 2 };
    ps[1].x
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
