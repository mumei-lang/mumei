enum C {
    R,
    G(v: i64)
}

struct P {
    x: i64,
    c: C
}

atom set_enum_field(ps: [P]) -> bool
requires: len(ps) > 0 && ps[0].c == C::R;
ensures: ps[0].c == C::G(1);
body: {
    ps[0].c = C::G(1);
    true
};
