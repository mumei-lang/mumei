// A lambda parameter named `seed` shadows the witness and does not count.
effect Random;

atom roll_shadow(seed: i64, x: i64) -> i64
effects: [Random];
ensures: result >= 0;
body: {
    let f = |seed: i64| -> i64 { perform Random.next(seed) };
    let r = f(x);
    if r >= 0 { r } else { 0 - r }
};
