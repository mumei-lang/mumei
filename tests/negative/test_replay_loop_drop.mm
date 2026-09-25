// `s` is seed-derived on the first iteration only; later iterations perform with `x`.
effect Random;

atom roll_loop(seed: i64, x: i64, n: i64) -> i64
requires: n >= 0;
effects: [Random];
ensures: result >= 0;
body: {
    let s = seed;
    let i = 0;
    let acc = 0;
    while i < n
    invariant: i >= 0 && i <= n && acc >= 0
    decreases: n - i
    {
        let r = perform Random.next(s);
        acc = if r >= 0 { r } else { 0 - r };
        s = x;
        i = i + 1;
    }
    acc
};
