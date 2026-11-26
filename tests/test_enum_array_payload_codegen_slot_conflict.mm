enum Ev {
    D(xs: [i64]),
    Scalar(value: i64)
}

atom make(xs: [i64]) -> i64
requires: true;
ensures: result == 0;
body: {
    let e = Ev::D(xs);
    0
};

atom main() -> i64
requires: true;
ensures: result == 0;
body: 0;
