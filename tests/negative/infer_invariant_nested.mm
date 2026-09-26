atom infer_nested_bad(n: i64) -> i64
requires: n >= 0;
ensures: result == n + 1;
body: {
    let i = 0;
    while i < n invariant: infer decreases: n - i {
        let j = 0;
        while j < 1 invariant: infer decreases: 1 - j {
            i = i + 1;
            j = j + 1;
        };
    };
    i
};
