// Regression: a clause calling a translated builtin (`len`) must not mark the
// counterexample "spurious" — the failure below is genuine (the element read
// is unconstrained, so `result >= 0` is falsifiable). Builtin call names must
// not be reported as uninterpreted_function dependencies.
atom len_const(arr: [i64]) -> i64
requires: len(arr) == 3;
ensures: result >= 0;
body: {
    arr[2]
};
