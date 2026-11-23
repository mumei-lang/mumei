struct Pt {
    x: i64
}

enum E {
    N,
    D(ps: [Pt])
}

atom main() -> i64
requires: true;
ensures: true;
body: {
    let e = E::D([Pt { x: 12 }]);
    match e {
        E::D(ps) => ps[0].x,
        E::N => 0
    }
};
