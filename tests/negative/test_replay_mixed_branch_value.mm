// The chosen value is `x` on one branch, so it is not guaranteed seed-derived.
effect Random;

atom roll_mixed(seed: i64, x: i64, flag: i64) -> i64
effects: [Random];
ensures: result >= 0;
body: {
    let s = if flag > 0 { seed } else { x };
    let r = perform Random.next(s);
    if r >= 0 { r } else { 0 - r }
};
