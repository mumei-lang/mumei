// Random declared but no seed parameter: not replayable.
effect Random;

atom roll(x: i64) -> i64
effects: [Random];
ensures: result >= 0;
body: {
    let r = perform Random.next(x);
    if r >= 0 { r } else { 0 - r }
};
