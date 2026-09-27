atom scoped() -> i64
requires: true;
ensures: result == 12;
body: {
    let xs = [1, 2, 3];
    if xs[0] == 1 {
        let t = [9, 9, 9, 9];
        xs[1] = t[0];
    } else {
        let t = [4];
        xs[1] = t[0];
    }
    xs[1] + len(xs)
}

atom stored() -> i64
requires: true;
ensures: result == 50;
body: {
    let a = [1, 2, 3];
    if a[0] == 1 {
        a[1] = 50;
    } else {
        a[1] = 60;
    }
    a[1]
}

atom main() -> i64
requires: true;
ensures: result == 12;
body: scoped()
