atom f() -> i64
requires: true;
ensures: result == 22;
body: {
    let a = [1, 2, 3];
    let b = [10, 20];
    let c = a[0] == 1;
    let xs = a;
    if c {
        xs = b;
    } else {
        xs = [1, 2, 3];
    }
    xs[1] + len(xs)
}

atom main() -> i64
requires: true;
ensures: result == 22;
body: f()
