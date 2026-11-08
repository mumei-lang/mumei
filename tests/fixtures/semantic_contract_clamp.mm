atom clamp_nonnegative(x: i64) -> i64
requires: true;
ensures: (x < 0 => result == 0) && (x >= 0 => result == x);
body: { if x < 0 { 0 } else { x } };
