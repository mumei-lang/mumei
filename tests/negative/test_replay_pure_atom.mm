// Pure atom (no effects:) performing a non-deterministic source is rejected
// by effect containment.
effect Random;

atom roll(seed: i64) -> i64
ensures: result >= 0;
body: {
    let r = perform Random.next(seed);
    if r >= 0 { r } else { 0 - r }
};
