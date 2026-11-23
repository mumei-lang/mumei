enum Ev {
    N,
    D(xs: [i64])
}

enum FloatEv {
    N,
    D(xs: [f64])
}

atom length_payload(xs: [i64]) -> i64
requires: len(xs) == 2;
ensures: result == 2;
body: {
    let e = Ev::D(xs);
    match e {
        Ev::D(ys) => len(ys),
        Ev::N => 0
    }
};

atom second_payload(xs: [i64]) -> i64
requires: len(xs) == 2 && xs[1] == 9;
ensures: result == 9;
body: {
    let e = Ev::D(xs);
    match e {
        Ev::D(ys) => ys[1],
        Ev::N => 0
    }
};

atom consume(xs: [i64]) -> i64
requires: len(xs) == 2 && xs[1] == 9;
ensures: result == 9;
body: xs[1];

atom pass_bound_array(e: Ev) -> i64
requires:
    match e {
        Ev::D(xs) => len(xs) == 2 && xs[1] == 9,
        Ev::N => false
    };
ensures: result == 9;
body: match e {
    Ev::D(xs) => consume(xs),
    Ev::N => 0
};

atom wrap(xs: [i64]) -> Ev
requires: len(xs) == 2;
ensures:
    match result {
        Ev::D(ys) => len(ys) == 2,
        Ev::N => false
    };
body: Ev::D(xs);

atom match_returned_array() -> i64
requires: true;
ensures: result == 2;
body: match wrap([1, 2]) {
    Ev::D(xs) => len(xs),
    Ev::N => 0
};

atom float_length(xs: [f64]) -> i64
requires: len(xs) == 2;
ensures: result == 2;
body: {
    let e = FloatEv::D(xs);
    match e {
        FloatEv::D(ys) => len(ys),
        FloatEv::N => 0
    }
};

atom main() -> i64
requires: true;
ensures: result == 24;
body: {
    let length = length_payload([3, 9]);
    let second = second_payload([3, 9]);
    let passed = pass_bound_array(Ev::D([3, 9]));
    let returned = match_returned_array();
    let floats = float_length([1.0, 2.0]);
    length + second + passed + returned + floats
};
