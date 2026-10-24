atom f() -> i64
requires: true;
ensures: result == 7;
body: {
    let xs = [1, 2, 3];
    let i = 0;
    let acc = 0;
    while i < 2
    invariant: i >= 0 && i <= 2 && acc == 2 * i
    decreases: 2 - i
    {
        let xs = [9, 9];
        acc = acc + len(xs);
        i = i + 1;
    }
    acc + len(xs)
}

atom main() -> i64
requires: true;
ensures: result == 7;
body: f()
