atom infer_unprovable(n: i64) -> i64
requires: n >= 0;
ensures: result == n * n;
body: {
    let i = 0;
    let sum = 0;
    while i < n invariant: infer decreases: n - i {
        sum = sum + i;
        i = i + 1;
    };
    sum
};
