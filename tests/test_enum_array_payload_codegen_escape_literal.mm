enum Ev {
    N,
    D(xs: [i64])
}

atom make() -> Ev
requires: true;
ensures: true;
body: Ev::D([1, 2]);

atom main() -> i64
requires: true;
ensures: result == 0;
body: 0;
