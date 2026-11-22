struct Inner {
    y: i64
}

struct P {
    x: i64,
    y: i64
}

struct Nested {
    inner: Inner,
    x: i64
}

struct U {
    value: u32
}

struct U64Value {
    value: u64
}

struct ScalarLeaves {
    real: f64,
    flag: bool,
    text: Str
}

struct Holder {
    value: P
}

atom field_fact(ps: [P]) -> i64
requires: len(ps) > 0 && ps[0].x > 5;
ensures: result > 5;
body: ps[0].x;

atom forall_field_fact(xs: [P]) -> i64
requires: len(xs) > 0 && forall(i, 0, len(xs), xs[i].x > 0);
ensures: result > 0;
body: {
    let p = xs[0];
    p.x
};

atom store_then_read(ps: [P]) -> i64
requires: len(ps) > 1;
ensures: result == 1;
body: {
    ps[1] = P { x: 1, y: 2 };
    ps[1].x
};

atom store_struct_variable(ps: [P], p: P) -> i64
requires: len(ps) > 0 && p.x == 5;
ensures: result == 5;
body: {
    ps[0] = p;
    ps[0].x
};

atom store_struct_field_access(ps: [P], holder: Holder) -> i64
requires: len(ps) > 0 && holder.value.x == 6;
ensures: result == 6;
body: {
    ps[0] = holder.value;
    ps[0].x
};

atom store_preserves_other_index(ps: [P]) -> i64
requires: len(ps) > 1 && ps[1].x == 7;
ensures: result == 7;
body: {
    let i = 0;
    let j = 1;
    ps[i] = P { x: 1, y: 2 };
    ps[j].x
};

atom bind_element(ps: [P], i: i64) -> i64
requires: i >= 0 && i < len(ps);
ensures: result == ps[i].x + ps[i].y;
body: {
    let p = ps[i];
    p.x + p.y
};

atom literal_fields() -> i64
requires: true;
ensures: result == 7;
body: {
    let ps = [P { x: 1, y: 2 }, P { x: 3, y: 4 }];
    len(ps) + ps[0].x + ps[1].y
};

atom nested_leaf(ps: [Nested]) -> i64
requires: len(ps) > 0 && ps[0].inner.y > 9;
ensures: result > 9;
body: ps[0].inner.y;

atom u32_leaf(ps: [U]) -> u32
requires: len(ps) > 0;
ensures: result >= 0;
body: ps[0].value;

atom u64_struct_param_non_overflow(p: U64Value) -> bool
requires: p.value < 100;
ensures: result == true;
body: p.value + 1 > p.value;

atom u64_array_param_non_overflow(ps: [U64Value]) -> bool
requires: len(ps) > 0 && ps[0].value < 100;
ensures: result == true;
body: ps[0].value + 1 > ps[0].value;

atom u64_array_binding_non_overflow(ps: [U64Value]) -> bool
requires: len(ps) > 0 && ps[0].value < 100;
ensures: result == true;
body: {
    let p = ps[0];
    p.value + 1 > p.value
};

atom scalar_leaf_sorts() -> bool
requires: true;
ensures: result == true;
body: {
    let ps = [ScalarLeaves { real: 1.5, flag: true, text: "ok" }];
    ps[0].real == 1.5 && ps[0].flag && ps[0].text == "ok"
};

atom loop_store() -> i64
requires: true;
ensures: result == 2;
body: {
    let ps = [P { x: 0, y: 0 }, P { x: 0, y: 0 }];
    let i = 0;
    while i < 2
    invariant: i >= 0 && i <= 2
    decreases: 2 - i
    {
        ps[i] = P { x: i, y: i };
        i = i + 1
    };
    len(ps)
};

atom array_alias(ps: [P]) -> i64
requires: len(ps) > 0 && ps[0].x == 9;
ensures: result == 9;
body: {
    let qs = ps;
    qs[0].x
};

atom make_struct_array() -> [P]
requires: true;
ensures: len(result) == 1 && result[0].x == 5;
body: [P { x: 5, y: 6 }];

atom call_result_array() -> i64
requires: true;
ensures: result == 5;
body: {
    let ps = make_struct_array();
    ps[0].x
};
