// Fixed-point (scale 10000) arithmetic with explicit range control.
// expected: PASS

atom fp_from_int(x: i64)
requires: x >= -100000 && x <= 100000;
ensures: result == x * 10000;
body: x * 10000;

atom fp_add(a: i64, b: i64)
requires: a >= -1000000000 && a <= 1000000000 && b >= -1000000000 && b <= 1000000000;
ensures: result == a + b;
body: a + b;

atom fp_scale_down(a: i64)
requires: a >= 0 && a <= 1000000000;
ensures: result * 10000 <= a && result >= 0;
body: a / 10000;

atom fp_abs(a: i64)
requires: a >= -1000000000 && a <= 1000000000;
ensures: result >= 0 && (result == a || result == -a);
body: { if a >= 0 { a } else { -a } };

atom fp_percentage(amount: i64, basis_points: i64)
requires: amount >= 0 && amount <= 1000000 && basis_points >= 0 && basis_points <= 10000;
ensures: result >= 0 && result <= amount * 10000;
body: amount * basis_points;
