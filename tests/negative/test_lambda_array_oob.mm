atom lambda_array_store_requires_nonempty_argument(xs: [i64]) -> i64
requires: len(xs) == 0;
ensures: true;
body: {
    let f = |ys: [i64]| {
        ys[0] = 1;
        0
    };
    f(xs)
};
