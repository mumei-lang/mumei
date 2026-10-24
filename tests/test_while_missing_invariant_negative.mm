// Negative fixture: `while` without `invariant` must be a clean syntax
// error, not a panic.
atom missing_invariant(n: i64)
    requires: n >= 0;
    ensures: result >= 0;
    body: {
        let i = 0;
        while i < n {
            i = i + 1
        };
        i
    };
