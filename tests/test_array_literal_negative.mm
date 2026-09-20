// Negative: literal out-of-bounds read must be rejected — `a` has len 3.
atom lit_oob() -> i64
ensures: result >= 0;
body: {
    let a = [1, 2, 3];
    a[3]
};
