enum Ev {
    N,
    D(xs: [i64])
}

atom same(e: Ev) -> bool
requires: true;
ensures: result == true;
body: e == e;

atom main() -> i64
requires: true;
ensures: result == 0;
body: 0;
