struct P {
    x: i64,
    y: i64
}

atom require_first_one(ps: [P], marker: i64) -> bool
requires: len(ps) > 0 && ps[0].x == 1 && marker == 0;
ensures: result == true;
body: true;

atom distinct_array_roots(ps: [P], qs: [P]) -> bool
requires: len(ps) > 0 && ps[0].x == 1 && len(qs) > 0;
ensures: result == true;
body: require_first_one(ps, {
    qs[0] = P { x: 9, y: 9 };
    0
});
