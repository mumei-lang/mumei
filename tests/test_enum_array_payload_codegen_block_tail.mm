enum Ev {
    N,
    D(xs: [i64])
}

atom main() -> i64
requires: true;
ensures: result == 9;
body: {
    let xs = [3, 9];
    match Ev::D({
        let ys = xs;
        ys
    }) {
        Ev::D(values) => values[1],
        Ev::N => 0
    }
};
