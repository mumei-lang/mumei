enum Ev {
    N,
    D(xs: [i64])
}

atom read_payload(e: Ev) -> i64
requires:
    match e {
        Ev::D(values) => len(values) == 2 && values[1] == 8,
        Ev::N => false
    };
ensures: result == 10;
body: {
    let xs = e.xs;
    len(xs) + xs[1]
};

atom main() -> i64
requires: true;
ensures: result == 10;
body: read_payload(Ev::D([3, 8]));
