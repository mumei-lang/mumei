enum Ev {
    N,
    D(xs: [i64])
}

atom overwrite(xs: [i64]) -> Ev
requires: len(xs) == 2;
ensures:
    match result {
        Ev::D(values) => len(values) == 2 && values[1] == 8,
        Ev::N => false
    };
body: {
    let e = Ev::D(xs);
    let ys = [7, 8];
    e.xs = ys;
    e
};

atom main() -> i64
requires: true;
ensures: result == 8;
body: match overwrite([1, 2]) {
    Ev::D(values) => values[1],
    Ev::N => 0
};
