enum Ev {
    N,
    D(xs: [i64])
}

atom make() -> Ev
requires: true;
ensures: true;
body: {
    let xs = [1, 2];
    let ys = xs;
    Ev::D(ys)
};

atom main() -> i64
requires: true;
ensures: result == 0;
body: 0;
