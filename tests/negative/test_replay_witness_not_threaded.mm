// seed exists but the perform does not derive from it.
effect Random;

atom roll(seed: i64, x: i64) -> i64
effects: [Random];
ensures: result >= 0;
body: {
    let r = perform Random.next(x);
    if r >= 0 { r } else { 0 - r }
};
