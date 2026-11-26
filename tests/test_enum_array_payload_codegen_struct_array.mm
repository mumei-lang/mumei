struct Pt {
    x: i64
}

enum Ev {
    N,
    D(xs: [Pt])
}

atom make(xs: [Pt]) -> i64
requires: true;
ensures: result == 0;
body: {
    let e = Ev::D(xs);
    0
};

atom main() -> i64
requires: true;
ensures: result == 0;
body: 0;
