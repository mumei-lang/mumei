// =============================================================
// Test: indirect calls through let-bound lambdas — `f(args)` and
// `call(f, args)` inline the bound lambda body (positive cases).
// =============================================================

atom call_lambda_basic() -> i64
requires: true;
ensures: result == 4;
body: {
    let f = |a| a + 1
    f(3)
};

atom call_lambda_two_args() -> i64
requires: true;
ensures: result == 12;
body: {
    let f = |a, b| a * b
    f(3, 4)
};

atom call_lambda_capture() -> i64
requires: true;
ensures: result == 11;
body: {
    let k = 10
    let f = |a| a + k
    f(1)
};

atom call_lambda_shadowed_param() -> i64
requires: true;
ensures: result == 7;
body: {
    let x = 3
    let f = |x| x + 4
    f(3)
};

atom call_lambda_callref() -> i64
requires: true;
ensures: result == 4;
body: {
    let f = |a| a + 1
    call(f, 3)
};

atom call_lambda_alias() -> i64
requires: true;
ensures: result == 4;
body: {
    let f = |a| a + 1
    let g = f
    g(3)
};

atom call_lambda_inline_literal() -> i64
requires: true;
ensures: result == 10;
body: {
    call(|a| a * 5, 2)
};

atom call_lambda_higher_order() -> i64
requires: true;
ensures: result == 6;
body: {
    let twice = |f, x| f(f(x))
    let inc = |a| a + 1
    twice(inc, 4)
};

atom call_lambda_survives_if() -> i64
requires: true;
ensures: result == 4;
body: {
    let f = |a| a + 1
    let c = 1
    if c > 0 {
        c = 0
    } else {
        c = 2
    }
    f(3)
};

atom call_lambda_branch_local() -> i64
requires: true;
ensures: result == 10;
body: {
    let c = 1
    if c > 0 {
        let f = |a| a * 2
        f(5)
    } else {
        0
    }
};

atom call_lambda_survives_while() -> i64
requires: true;
ensures: result == 4;
body: {
    let f = |a| a + 1
    let i = 0
    while i < 2
        invariant: i >= 0
        decreases: 2 - i
        { i = i + 1 }
    f(3)
};

atom call_lambda_in_match() -> i64
requires: true;
ensures: result == 4;
body: {
    let f = |a| a + 1
    match 1 { 1 => f(3), _ => 0 }
};
