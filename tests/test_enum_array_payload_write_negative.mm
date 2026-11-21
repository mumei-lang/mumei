enum Ev {
    Data(xs: [i64]),
    Idle
}

atom untagged_array_write(e: Ev, ys: [i64]) -> i64
requires: true;
ensures: result == 0;
body: {
    e.xs = ys;
    0
};
