// =============================================================
// Test: indirect lambda calls — negative cases. Every atom must
// fail verification (genuine counterexamples or fail-closed
// rejects), never a silent wrong "verified".
// =============================================================

// The call inlines the lambda body, so `result == 5` is a real
// counterexample (f(3) == 4), not an uninterpreted guess.
atom call_lambda_wrong_post() -> i64
requires: true;
ensures: result == 5;
body: {
    let f = |a| a + 1
    f(3)
};

// Arity mismatch: fails closed during body evaluation.
atom call_lambda_arity_mismatch() -> i64
requires: true;
ensures: result == 4;
body: {
    let f = |a| a + 1
    f(3, 4)
};

// Rebinding to a non-lambda drops the binding — `f(3)` resolves no
// body and must fail closed, not answer with the stale lambda.
atom call_lambda_rebound_scalar() -> i64
requires: true;
ensures: result == 4;
body: {
    let f = |a| a + 1
    f = 0
    f(3)
};

// Lambdas cannot self-reference — `f` is unbound inside its own
// body, so this fails closed (no fixpoint is constructed).
atom call_lambda_recursive() -> i64
requires: true;
ensures: result == 4;
body: {
    let f = |a| f(a)
    f(3)
};

// A name bound to *different* lambdas on the two `if` branches is
// dropped at the merge (there is no `ite` of closures) — a call
// after the branch fails closed.
atom call_lambda_divergent_branches() -> i64
requires: true;
ensures: result == 4;
body: {
    let c = 1
    if c > 0 {
        let f = |a| a + 1
        0
    } else {
        let f = |a| a - 1
        0
    }
    f(3)
};
