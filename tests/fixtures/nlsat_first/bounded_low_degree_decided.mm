// P10-D / C-2 fixture: bounded (both sides, literal), degree <= 2, <= 3 variables.
// Tagged `nonlinear_arithmetic` but routed to nlsat-first; Z3 decides unsat and
// the atom is final (`verified`, no Lean escalation candidate).
atom bounded_product(a: i64, b: i64) -> i64
requires: a >= 0 && a <= 1000 && b >= 0 && b <= 1000;
ensures: result >= 0 && result <= 1000000;
body: a * b;

atom bounded_square_plus_linear(x: i64, y: i64, z: i64) -> i64
requires: x >= -50 && x <= 50 && y >= 0 && y <= 100 && z >= 0 && z <= 10;
ensures: result >= 0;
body: x * x + y * z;

atom bounded_quotient(n: i64, d: i64) -> i64
requires: n >= 0 && n <= 100000 && d >= 1 && d <= 1000;
ensures: result >= 0 && result <= n;
body: n / d;
