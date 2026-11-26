struct P {
    x: i64,
    y: i64
}

atom write_y(ps: [P]) -> i64
requires: len(ps) > 0;
ensures: ps[0].y == 0;
body: {
    ps[0].y = 0;
    0
};

atom mixed_field_store_and_writer_stale_read(ps: [P]) -> i64
requires: len(ps) > 0 && ps[0].x == 1 && ps[0].y == 7;
ensures: result == 7;
body: {
    let i = 0;
    while i < 1
    invariant: i >= 0 && i <= 1
    decreases: 1 - i
    {
        ps[0].x = 2;
        let ignored = write_y(ps);
        i = i + 1
    };
    ps[0].y
};

atom mixed_alias_field_store_and_writer_stale_read(ps: [P]) -> i64
requires: len(ps) > 0 && ps[0].x == 1 && ps[0].y == 7;
ensures: result == 7;
body: {
    let alias = ps;
    let i = 0;
    while i < 1
    invariant: i >= 0 && i <= 1
    decreases: 1 - i
    {
        alias[0].x = 2;
        let ignored = write_y(alias);
        i = i + 1
    };
    alias[0].y
};

atom call_havoc_prevents_spec_ensures_unsat(ps: [P]) -> bool
requires: len(ps) > 0 && ps[0].y == 7;
ensures: ps[0].y == 0;
body: {
    let ignored = write_y(ps);
    true
};
