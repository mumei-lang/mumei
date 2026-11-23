enum Ev {
    N,
    D(xs: [i64])
}

atom overwrite(xs: [i64]) -> Ev
requires: len(xs) == 2;
ensures:
    match result {
        Ev::D(ys) => len(ys) == 2 && ys[1] == 8,
        Ev::N => false
    };
body: {
    let e = Ev::D(xs);
    e.xs = [7, 8];
    e
};

atom main() -> i64
requires: true;
ensures: result == 8;
body: match overwrite([1, 2]) {
    Ev::D(ys) => ys[1],
    Ev::N => 0
};
