// Negative test: Lambda with postcondition violation
// Expected: Verification failure — ensures result > 0 but body returns 0

atom apply_zero(x: i64) -> i64
    requires: x >= 0;
    ensures: result > 0;
    body: {
        let zero_fn = |a| 0;
        call(zero_fn, x)
    }
