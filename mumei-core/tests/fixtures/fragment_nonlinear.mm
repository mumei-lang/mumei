atom multiply_bounded(a: i64, b: i64) -> i64
requires: a >= 0 && b >= 0;
ensures: result == a * b;
body: a * b;
