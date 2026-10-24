// An assignment inside a lambda that may run before the perform drops the witness.
effect Random;

atom roll_lambda_drop(seed: i64, x: i64) -> i64
effects: [Random];
ensures: result >= 0;
body: {
    let s = seed;
    let f = |y| { s = x; y };
    let r = perform Random.next(s);
    if r >= 0 { r } else { 0 - r }
};
