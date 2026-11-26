enum C {
    R,
    G(v: i64)
}

enum D {
    Other
}

struct P {
    c: C
}

atom store_wrong_enum(ps: [P]) -> bool
requires: len(ps) > 0;
ensures: result == true;
body: {
    ps[0].c = D::Other;
    true
};
