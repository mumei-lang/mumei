// Only one path re-derives `s` from seed; the other leaves it as `x`.
effect Random;

atom roll_branch_drop(seed: i64, x: i64, flag: i64) -> i64
effects: [Random];
ensures: result >= 0;
body: {
    let s = x;
    if flag > 0 { s = seed; 0 } else { 0 };
    let r = perform Random.next(s);
    if r >= 0 { r } else { 0 - r }
};
