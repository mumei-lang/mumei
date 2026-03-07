// Negative test: Atom calls effectful function without declaring effects
// Expected: Effect inference suggests adding missing effects

effect FileWrite;

atom write_data(x: i64) -> i64
  effects: [FileWrite];
  requires: x >= 0;
  ensures: result == x;
  body: x;

// This atom calls write_data but does NOT declare FileWrite
// Effect inference should flag this as missing
atom caller_no_effects(x: i64) -> i64
  requires: x >= 0;
  ensures: result == x;
  body: x;
