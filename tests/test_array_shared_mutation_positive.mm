struct S {
    xs: [i64]
}

enum E {
    D(xs: [i64]),
    N
}

atom specification_construction_does_not_share_runtime_array(xs: [i64]) -> i64
requires:
    len(xs) > 0
    && match E::D(xs) {
        E::D(_) => true,
        E::N => false
    };
ensures: result == 9;
body: {
    xs[0] = 9;
    xs[0]
};

atom first(xs: [i64]) -> i64
requires: len(xs) > 0;
ensures: result == xs[0];
body: xs[0];

atom length_only(xs: [i64]) -> i64
requires: len(xs) > 0;
ensures: result == len(xs);
body: len(xs);

atom read_only_builtin_on_field(s: S) -> i64
requires: len(s.xs) > 0;
ensures: result == len(s.xs);
body: length_only(s.xs);

atom read_only_builtin_on_shared_binding(s: S) -> i64
requires: len(s.xs) > 0;
ensures: result == len(s.xs);
body: {
    let v = s.xs;
    length_only(v)
};

atom lambda_parameter_shadows_shared_marker(s: S, b: [i64]) -> i64
requires: len(s.xs) > 0 && len(b) > 0;
ensures: result == 9;
body: {
    let xs = s.xs;
    let f = |xs: [i64]| {
        if len(xs) > 0 {
            xs[0] = 9;
            xs[0]
        } else {
            9
        }
    };
    f(b)
};

atom struct_field_length_remains_precise(s: S) -> i64
requires: len(s.xs) == 2;
ensures: result == 2;
body: {
    let v = s.xs;
    len(v)
};

atom struct_field_element_read_remains_precise(xs: [i64]) -> i64
requires: len(xs) > 0 && xs[0] == 7;
ensures: result == 7;
body: {
    let s = S { xs: xs };
    let v = s.xs;
    v[0]
};

atom repeated_enum_field_reads_remain_precise(e: E) -> bool
requires: match e {
    E::D(xs) => len(xs) == 2,
    E::N => false
};
ensures: result == true;
body: {
    let a = e.xs;
    let b = e.xs;
    len(a) == len(b)
};

atom ordinary_local_store_after_construction() -> i64
requires: true;
ensures: result == 9;
body: {
    let a = [1, 2];
    let a2 = [3, 4];
    let unrelated = E::D(a2);
    a[0] = 9;
    a[0]
};

atom nonwriting_field_call(s: S) -> i64
requires: len(s.xs) > 0;
ensures: true;
body: first(s.xs);

atom nonwriting_shared_variable_call(s: S) -> i64
requires: len(s.xs) > 0;
ensures: true;
body: {
    let v = s.xs;
    first(v)
};

atom shadow_shared_binding_then_store(s: S) -> i64
requires: len(s.xs) > 0;
ensures: result == 9;
body: {
    let v = s.xs;
    let v = [1, 2];
    v[0] = 9;
    v[0]
};
