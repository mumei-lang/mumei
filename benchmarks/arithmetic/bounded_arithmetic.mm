// Bounded integer arithmetic within explicit range preconditions.
// expected: PASS

atom bounded_add(a: i64, b: i64)
requires: a >= 0 && a <= 1000000 && b >= 0 && b <= 1000000;
ensures: result == a + b && result >= 0 && result <= 2000000;
body: a + b;

atom bounded_sub(a: i64, b: i64)
requires: a >= 0 && a <= 1000000 && b >= 0 && b <= a;
ensures: result == a - b && result >= 0 && result <= a;
body: a - b;

atom bounded_mul(a: i64, b: i64)
requires: a >= 0 && a <= 1000 && b >= 0 && b <= 1000;
ensures: result == a * b && result >= 0 && result <= 1000000;
body: a * b;

atom bounded_neg(x: i64)
requires: x >= -1000000 && x <= 1000000;
ensures: result == -x && result >= -1000000 && result <= 1000000;
body: -x;
