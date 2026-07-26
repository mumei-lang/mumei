// Overflow boundary reasoning near the i64 limits.
// expected: PASS

atom add_at_upper_boundary(a: i64, b: i64)
requires: a <= 9223372036854775807 - b && b >= 0 && a >= 0;
ensures: result == a + b && result >= a && result >= b;
body: a + b;

atom sub_at_lower_boundary(a: i64, b: i64)
requires: a >= -9223372036854775807 + b && b >= 0;
ensures: result == a - b && result <= a;
body: a - b;

atom double_without_overflow(x: i64)
requires: x >= 0 && x <= 4611686018427387903;
ensures: result == x * 2 && result >= x;
body: x * 2;

atom halve_towards_zero(x: i64)
requires: x >= 0 && x <= 9223372036854775807;
ensures: result * 2 <= x && result >= 0;
body: x / 2;
