atom f() -> i64
requires: true;
ensures: result == 22;
body: {
    let xs = [1, 2, 3];
    let k = 2;
    match k {
        1 => { xs = [7]; 0 }
        2 => { xs = [10, 20]; 0 }
        _ => { xs = [5, 6, 7, 8]; 0 }
    };
    xs[1] + len(xs)
}

atom main() -> i64
requires: true;
ensures: result == 22;
body: f()
