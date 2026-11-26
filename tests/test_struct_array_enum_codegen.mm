enum C {
    R,
    G(v: i64)
}

struct P {
    x: i64,
    c: C
}

atom main() -> i64
requires: true;
ensures: true;
body: {
    let ps = [P { x: 5, c: C::R }];
    ps[0].c = C::G(12);
    match ps[0].c {
        C::R => 0,
        C::G(v) => v
    }
};
