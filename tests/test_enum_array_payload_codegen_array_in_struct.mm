struct Wrapped {
    values: [i64]
}

enum Ev {
    N,
    D(value: Wrapped)
}

atom make(xs: [i64]) -> i64
requires: true;
ensures: result == 0;
body: {
    let wrapped = Wrapped { values: xs };
    let e = Ev::D(wrapped);
    0
};

atom main() -> i64
requires: true;
ensures: result == 0;
body: 0;
