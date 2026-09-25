// Replayability: non-deterministic effects must flow through an explicit
// witness parameter (seed / timestamp / input) that is threaded into every
// perform. Same inputs => same trace.
effect Random;
effect Clock;
effect ExternalInput;

atom roll(seed: i64) -> i64
effects: [Random];
requires: seed >= 0;
ensures: result >= 0;
body: {
    let r = perform Random.next(seed);
    if r >= 0 { r } else { 0 - r }
};

atom stamp(timestamp: i64, x: i64) -> i64
effects: [Clock];
requires: x >= 0;
ensures: result >= 0;
body: {
    let now = perform Clock.now(timestamp);
    if now >= x { now - x } else { x - now }
};

atom read_input(input: i64) -> i64
effects: [ExternalInput];
ensures: result >= 0;
body: {
    let v = perform ExternalInput.read(input);
    if v >= 0 { v } else { 0 }
};

// Two performs with the same witness denote the same value.
atom replay_twice(seed: i64) -> i64
effects: [Random];
ensures: result == 0;
body: {
    let a = perform Random.next(seed);
    let b = perform Random.next(seed);
    a - b
};
