enum Ev {
    N,
    D(xs: [i64])
}

atom escape() -> Ev
requires: true;
ensures: true;
body: Ev::D({
    let ys = [1, 2];
    ys
});

atom main() -> i64
requires: true;
ensures: true;
body: 0;
