// A binding made inside a (never called) lambda must not witness the outer perform.
effect Random;

atom roll_leak(seed: i64, x: i64) -> i64
effects: [Random];
ensures: result >= 0;
body: {
    let s = x;
    let f = |y| { s = seed; y };
    let r = perform Random.next(s);
    if r >= 0 { r } else { 0 - r }
};
