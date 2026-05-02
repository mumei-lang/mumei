// Regression: fold_min_index / fold_max_index style early-return branches
// should verify without `trusted`.

atom fold_min_index_pattern(n: i64)
requires: n >= 0;
ensures: result >= 0 - 1 && result < n;
body: {
    if n == 0 { 0 - 1 }
    else {
        let min_idx = 0;
        let i = 1;
        while i < n
        invariant: i >= 1 && i <= n && min_idx >= 0 && min_idx < n
        decreases: n - i
        {
            min_idx = min_idx;
            i = i + 1;
        };
        min_idx
    }
};

atom fold_max_index_pattern(n: i64)
requires: n >= 0;
ensures: result >= 0 - 1 && result < n;
body: {
    if n == 0 { 0 - 1 }
    else {
        let max_idx = 0;
        let i = 1;
        while i < n
        invariant: i >= 1 && i <= n && max_idx >= 0 && max_idx < n
        decreases: n - i
        {
            max_idx = max_idx;
            i = i + 1;
        };
        max_idx
    }
};
