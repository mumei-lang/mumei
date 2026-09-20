// Negative: literal out-of-bounds read must be rejected — `a` has len 3.
atom lit_oob() -> i64
ensures: result >= 0;
body: {
    let a = [1, 2, 3];
    a[3]
};

// Negative: a while-loop that reassigns `a` must not keep the pre-loop
// literal's store chain — post-loop `a` is a fresh havoc array, so the
// stale claim `a[0] == 1` (from the original [1,2]) must not verify.
atom while_reassign_stale() -> i64
ensures: result == 1;
body: {
    let a = [1, 2];
    let i = 0;
    while i < 1
    invariant: i >= 0 && i <= 1
    decreases: 1 - i
    {
        a = [3, 4, 5];
        i = i + 1
    };
    a[0]
};
