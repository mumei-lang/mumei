enum C {
    R(v: i64),
    G
}

enum Unit {
    A,
    B
}

atom h(xs: [C]) -> i64
requires: len(xs) > 0 && match xs[0] {
    C::R(v) => v > 5,
    C::G => false
};
ensures: result > 5;
body: match xs[0] {
    C::R(v) => v,
    C::G => 0
};

atom payload(x: C) -> i64
requires: true;
ensures: match x {
    C::R(v) => result == v,
    C::G => result == 0
};
body: match x {
    C::R(v) => v,
    C::G => 0
};

atom array_match(xs: [C], i: i64) -> i64
requires: i >= 0 && i < len(xs);
ensures: true;
body: match xs[i] {
    C::R(v) => v,
    C::G => 0
};

atom main() -> i64
requires: true;
ensures: true;
body: {
    let p = h([C::R(9)]);
    let xs = [C::R(5), C::G];
    xs[1] = C::R(7);
    let units = [Unit::A, Unit::B];
    units[1] = Unit::A;
    let unit_len = len(units);
    let sum = payload(xs[0]) + payload(xs[1]);
    if p > 5 && unit_len == 2 { sum } else { 0 }
};
