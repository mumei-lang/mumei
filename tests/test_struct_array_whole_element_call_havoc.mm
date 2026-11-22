struct P {
    x: i64,
    y: i64
}

atom write_whole_element(ps: [P]) -> i64
requires: len(ps) > 0;
ensures: ps[0].y == 0;
body: {
    ps[0] = P { x: 2, y: 0 };
    0
};

atom whole_element_store_and_writer_stale_read(ps: [P]) -> i64
requires: len(ps) > 0 && ps[0].x == 1 && ps[0].y == 7;
ensures: result == 7;
body: {
    let i = 0;
    while i < 1
    invariant: i >= 0 && i <= 1
    decreases: 1 - i
    {
        ps[0] = P { x: 2, y: 7 };
        let ignored = write_whole_element(ps);
        i = i + 1
    };
    ps[0].y
};
