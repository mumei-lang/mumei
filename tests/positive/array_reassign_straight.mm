atom f() -> i64
requires: true;
ensures: result == 22;
body: {
    let xs = [1, 2, 3];
    xs = [10, 20];
    xs[1] + len(xs)
}

atom main() -> i64
requires: true;
ensures: result == 22;
body: f()
