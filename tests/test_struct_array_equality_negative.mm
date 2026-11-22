struct P {
    x: i64
}

atom loop_rewrite_keeps_equality(ps: [P], qs: [P]) -> bool
requires: len(ps) == 1 && len(qs) == 1 && ps == qs;
ensures: ps == qs;
body: {
    let i = 0;
    while i < 1
    invariant: i >= 0 && i <= 1
    decreases: 1 - i
    {
        ps[0] = P { x: 9 };
        i = i + 1
    };
    true
};

atom field_write_keeps_equality(ps: [P], qs: [P]) -> bool
requires: len(ps) == 1 && len(qs) == 1 && ps == qs;
ensures: ps == qs;
body: {
    ps[0].x = 9;
    true
};
