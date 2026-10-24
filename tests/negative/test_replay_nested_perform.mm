// A perform nested in an array literal must still thread the witness.
effect Random;

atom roll_nested(seed: i64) -> i64
effects: [Random];
ensures: result >= 0;
body: {
    let xs = [perform Random.next(42)];
    let r = xs[0] + seed;
    if r >= 0 { r } else { 0 - r }
};
