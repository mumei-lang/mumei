atom main() -> i64
requires: true;
ensures: true;
body: {
    let xs = [1, 2, 3];
    let i = 0;
    while i < 2
    invariant: i >= 0
    decreases: 2 - i
    {
        xs = [9, 9];
        i = i + 1;
    }
    len(xs)
}
