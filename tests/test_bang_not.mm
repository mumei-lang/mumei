// Prefix `!` desugars to `if e { false } else { true }` — verified as a
// Bool negation and emitted as a 0/1 branch.

atom bang_negate(b: i64) -> i64
  requires: b == 0 || b == 1;
  ensures: (b == 0 && result == 1) || (b == 1 && result == 0);
  body: {
    if !(b == 0) { 0 } else { 1 }
  }

atom bang_double(x: i64) -> i64
  requires: x >= 0;
  ensures: result == x;
  body: {
    let f = !(x >= 0)
    if !!f { 0 - x } else { x }
  }

atom bang_spec(x: i64) -> i64
  requires: !(x < 0);
  ensures: !(result <= 0);
  body: {
    x + 1
  }
