// Reading the witness only in the condition does not make the value derived.
effect Random;

atom roll_guard(seed: i64, x: i64) -> i64
effects: [Random];
ensures: result >= 0;
body: {
    let s = if seed > 0 { x } else { x };
    let r = perform Random.next(s);
    if r >= 0 { r } else { 0 - r }
};
