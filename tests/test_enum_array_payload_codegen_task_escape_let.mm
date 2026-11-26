enum Ev {
    N,
    D(xs: [i64])
}

atom make() -> Ev
requires: true;
ensures:
    match result {
        Ev::D(ys) => len(ys) == 2 && ys[1] == 2,
        Ev::N => false
    };
body: {
    task {
        let xs = [1, 2];
        Ev::D(xs)
    }
};

atom main() -> i64
requires: true;
ensures: result == 2;
body: match make() {
    Ev::D(ys) => ys[1],
    Ev::N => 0
};
