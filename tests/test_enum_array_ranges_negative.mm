enum C {
    R(v: i64),
    G
}

atom unbounded_i64_payload(xs: [C]) -> i64
requires: len(xs) > 0;
ensures: result >= 0;
body: match xs[0] {
    C::R(v) => v,
    C::G => 0
};
