atom mk(n: i64) -> [i64]
requires: true;
ensures: len(result) == 2;
body: { [n, n] };

atom main() -> i64
requires: true;
ensures: true;
body: {
    let xs = [1, 2, 3];
    xs = mk(4);
    len(xs)
}
