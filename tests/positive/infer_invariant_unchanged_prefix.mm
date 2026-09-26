atom infer_unchanged_prefix(a: [i64]) -> i64
requires: len(a) >= 1;
ensures: result == len(a);
body: {
    a[0] = 1;
    let i = 1;
    while i < len(a) invariant: infer decreases: len(a) - i {
        a[i] = 0;
        i = i + 1;
    };
    i
};
