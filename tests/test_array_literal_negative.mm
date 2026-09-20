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

// Negative: a mutating callee writes through the shared data pointer —
// `a[0]` after `mutate(a)` must NOT verify as the pre-call element.
atom mutate(arr: [i64]) -> i64
requires: len(arr) >= 1;
ensures: result == arr[0];
body: {
    arr[0] = arr[0] + 10;
    arr[0]
};
atom post_call_stale() -> i64
ensures: result == 1;
body: {
    let a = [1, 2];
    mutate(a);
    a[0]
};
