enum Ev {
    N,
    D(xs: [i64])
}

atom pick(c: i64) -> i64
requires: c >= 0 && c <= 1;
ensures: true;
body: {
    let ys = [1, 2];
    let zs = [7, 8];
    match Ev::D(if c == 1 { let ys = zs; ys } else { ys }) {
        Ev::D(values) => values[1],
        Ev::N => 0
    }
};

atom main() -> i64
requires: true;
ensures: true;
body: {
    pick(1) * 10 + pick(0)
};
