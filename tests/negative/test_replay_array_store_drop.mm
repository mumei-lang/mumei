// Overwriting an element with a non-witness value drops the array's witness provenance.
effect Random;

atom roll_array_drop(seed: i64, x: i64) -> i64
effects: [Random];
ensures: result >= 0;
body: {
    let a = [seed, seed];
    a[0] = x;
    let r = perform Random.next(a[0]);
    if r >= 0 { r } else { 0 - r }
};
