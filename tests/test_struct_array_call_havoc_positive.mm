struct P {
    x: i64,
    y: i64
}

atom field_only_loop_preserves_other_leaf(ps: [P]) -> i64
requires: len(ps) > 0 && ps[0].x == 1 && ps[0].y == 7;
ensures: result == 7;
body: {
    let i = 0;
    while i < 1
    invariant: i >= 0 && i <= 1
    decreases: 1 - i
    {
        ps[0].x = 2;
        i = i + 1
    };
    ps[0].y
};
