atom infer_reserved(n: i64) -> i64
requires: n >= 0;
ensures: result == n;
body: {
    let __loop0_init_i = 0;
    let i = 0;
    while i < n invariant: infer decreases: n - i {
        i = i + 1;
    };
    i
};
