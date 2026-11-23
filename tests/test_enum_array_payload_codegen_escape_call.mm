enum Ev {
    N,
    D(xs: [i64])
}

atom wrap(xs: [i64]) -> Ev
requires: len(xs) == 2;
ensures:
    match result {
        Ev::D(ys) => len(ys) == 2,
        Ev::N => false
    };
body: Ev::D(xs);

atom escape() -> Ev
requires: true;
ensures:
    match result {
        Ev::D(ys) => len(ys) == 2,
        Ev::N => false
    };
body: wrap([1, 2]);
