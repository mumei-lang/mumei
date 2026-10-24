atom infer_array_store(a: [i64], n: i64) -> i64
requires: len(a) >= 1 && n >= 0;
ensures: result == n;
body: {
    let i = 0;
    while i < n invariant: infer decreases: n - i {
        a = a;
        a[0] = 1;
        i = i + 1;
    };
    i
};
