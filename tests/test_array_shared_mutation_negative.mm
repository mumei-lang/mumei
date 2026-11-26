struct S {
    xs: [i64]
}

enum E {
    D(xs: [i64]),
    SD(s: S),
    N
}

atom struct_field_local_alias() -> i64
requires: true;
ensures: result == 1;
body: {
    let s = S { xs: [1, 2] };
    let v = s.xs;
    v[0] = 9;
    let w = s.xs;
    w[0]
};

atom struct_parameter_field_alias(s: S) -> i64
requires: len(s.xs) > 0;
ensures: result == 1;
body: {
    let v = s.xs;
    v[0] = 9;
    let w = s.xs;
    w[0]
};

atom enum_pattern_then_field_read() -> i64
requires: true;
ensures: result == 1;
body: {
    let e = E::D([1, 2]);
    match e {
        E::D(ys) => {
            ys[0] = 9;
            let zs = e.xs;
            zs[0]
        },
        E::SD(_) => 1,
        E::N => 1
    }
};

atom enum_pattern_across_rematch() -> i64
requires: true;
ensures: result == 1;
body: {
    let e = E::D([1, 2]);
    let r = match e {
        E::D(ys) => {
            ys[0] = 9;
            0
        },
        E::SD(_) => 0,
        E::N => 0
    };
    match e {
        E::D(zs) => zs[0] + r,
        E::SD(_) => 1,
        E::N => 1
    }
};

atom enum_constructor_source_alias() -> i64
requires: true;
ensures: result == 1;
body: {
    let a = [1, 2];
    let e = E::D(a);
    a[0] = 9;
    match e {
        E::D(z) => z[0],
        E::SD(_) => 1,
        E::N => 1
    }
};

atom struct_constructor_source_alias() -> i64
requires: true;
ensures: result == 1;
body: {
    let a = [1, 2];
    let s = S { xs: a };
    a[0] = 9;
    let w = s.xs;
    w[0]
};

atom spec_marker_does_not_hide_body_constructor(xs: [i64]) -> i64
requires:
    len(xs) > 0
    && match E::D(xs) {
        E::D(_) => true,
        E::SD(_) => false,
        E::N => false
    };
ensures: true;
body: {
    let e = E::D(xs);
    xs[0] = 9;
    0
};

atom shared_alias_chain(s: S) -> i64
requires: len(s.xs) > 0;
ensures: true;
body: {
    let v = s.xs;
    let w = v;
    w[0] = 9;
    0
};

atom shared_array_mutating_lambda_argument(s: S) -> i64
requires: len(s.xs) > 0;
ensures: true;
body: {
    let ys = [0];
    let v = s.xs;
    let f = |ys: [i64]| {
        if len(ys) > 0 {
            ys[0] = 1;
            0
        } else {
            0
        }
    };
    f(v)
};

atom write_array(xs: [i64]) -> i64
requires: len(xs) > 0;
ensures: result == 0;
body: {
    xs[0] = 9;
    0
};

atom shared_mutating_call(s: S) -> i64
requires: len(s.xs) > 0;
ensures: true;
body: {
    let v = s.xs;
    write_array(v)
};

atom shared_field_write_source(e: E, ys: [i64]) -> i64
requires:
    match e {
        E::SD(_) => true,
        E::D(_) => false,
        E::N => false
    }
    && len(ys) > 0;
ensures: true;
body: {
    e.s.xs = ys;
    ys[0] = 9;
    0
};

atom branch_shared_marker_survives_merge(s: S, c: bool) -> i64
requires: len(s.xs) > 0;
ensures: true;
body: {
    let v = [1, 2];
    if c {
        v = s.xs;
    } else {
        v = [3, 4];
    };
    v[0] = 9;
    0
};

atom loop_shared_marker_survives_loop(s: S) -> i64
requires: len(s.xs) > 0;
ensures: true;
body: {
    let v = [1, 2];
    let i = 0;
    while i < 1
    invariant: i >= 0 && i <= 1
    decreases: 1 - i
    {
        v = s.xs;
        i = i + 1
    };
    v[0] = 9;
    0
};

atom shared_store_inside_while(s: S) -> i64
requires: len(s.xs) > 0;
ensures: true;
body: {
    let v = s.xs;
    let i = 0;
    while i < 1
    invariant: i >= 0 && i <= 1
    decreases: 1 - i
    {
        v[0] = 9;
        i = i + 1
    };
    0
};

atom mutating_field_access_call(s: S) -> i64
requires: len(s.xs) > 0;
ensures: true;
body: write_array(s.xs);

atom locally_shadowed_builtin_writer(xs: [i64]) -> i64
requires: len(xs) > 0;
ensures: result == 0;
body: {
    let ys = [1];
    let is_empty = |ys: [i64]| {
        if len(ys) > 0 {
            ys[0] = 9;
            0
        } else {
            0
        }
    };
    is_empty(xs)
};

atom local_builtin_shadow_is_rejected(a: [i64]) -> i64
requires: len(a) == 1 && a[0] == 1;
ensures: result == 1;
body: {
    let s = S { xs: a };
    locally_shadowed_builtin_writer(s.xs);
    let w = s.xs;
    w[0]
};

atom mutates_through_alias(xs: [i64]) -> i64
requires: len(xs) > 0;
ensures: result == 0;
body: {
    let ys = xs;
    ys[0] = 9;
    0
};

atom lambda_alias_write_is_rejected(a: [i64]) -> i64
requires: len(a) == 1 && a[0] == 1;
ensures: result == 1;
body: {
    let s = S { xs: a };
    let xs = s.xs;
    let f = |xs: [i64]| {
        if len(xs) > 0 {
            let ys = xs;
            ys[0] = 9;
            0
        } else {
            0
        }
    };
    f(xs);
    let w = s.xs;
    w[0]
};

atom atom_alias_write_is_rejected(a: [i64]) -> i64
requires: len(a) == 1 && a[0] == 1;
ensures: result == 1;
body: {
    let s = S { xs: a };
    mutates_through_alias(s.xs);
    let w = s.xs;
    w[0]
};
