enum Ev {
    N,
    D(xs: [i64])
}

atom main() -> i64
requires: true;
ensures: result == 8;
body: {
    let ys = [1, 2];
    let zs = [7, 8];
    match Ev::D({ let ys = zs; ys }) {
        Ev::D(values) => values[1],
        Ev::N => 0
    }
};
