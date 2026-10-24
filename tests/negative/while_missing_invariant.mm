atom missing_invariant(n: i64) -> i64
requires: n >= 0;
ensures: result == n;
body: {
    let i = 0;
    while i < n {
        i = i + 1;
    };
    i
};
