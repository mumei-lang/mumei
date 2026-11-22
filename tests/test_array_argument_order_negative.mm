struct P {
    x: i64,
    y: i64
}

atom require_first_one(ps: [P], marker: i64) -> bool
requires: len(ps) > 0 && ps[0].x == 1 && marker == 0;
ensures: result == true;
body: true;

atom direct_struct_call(ps: [P]) -> bool
requires: len(ps) > 0 && ps[0].x == 1;
ensures: result == true;
body: require_first_one(ps, {
    ps[0] = P { x: 9, y: 9 };
    0
});

atom atom_ref_struct_call(ps: [P]) -> bool
requires: len(ps) > 0 && ps[0].x == 1;
ensures: result == true;
body: call(atom_ref(require_first_one), ps, {
    ps[0] = P { x: 9, y: 9 };
    0
});

atom lambda_struct_call(ps: [P]) -> bool
requires: len(ps) > 0 && ps[0].x == 1;
ensures: result == true;
body: {
    let f = |a: [P], marker: i64| marker == 0;
    f(ps, {
        ps[0] = P { x: 9, y: 9 };
        0
    })
};

atom observe_first(xs: [i64], marker: i64) -> i64
requires: len(xs) > 0 && marker == 0;
ensures: true;
body: xs[0];

atom scalar_call(xs: [i64]) -> i64
requires: len(xs) > 0 && xs[0] == 1;
ensures: result == 9;
body: observe_first(xs, {
    xs[0] = 9;
    0
});
