atom f() -> i64
requires: true;
ensures: result == 22;
body: {
    let xs = [1, 2, 3];
    let c = xs[0] == 1;
    if c {
        xs = [10, 20];
    } else {
        xs = [5, 6, 7, 8];
    }
    xs[1] + len(xs)
}

atom g() -> i64
requires: true;
ensures: result == 10;
body: {
    let ys = [1, 2, 3];
    let d = ys[0] == 2;
    if d {
        ys = [10, 20];
    } else {
        ys = [5, 6, 7, 8];
    }
    ys[1] + len(ys)
}

atom main() -> i64
requires: true;
ensures: result == 32;
body: f() + g()
