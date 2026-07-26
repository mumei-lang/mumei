// Low-degree polynomial reasoning (nonlinear arithmetic) over narrow ranges so
// that the nonlinear fragment stays inside the CI solver budget.
// expected: PASS

atom square_is_nonnegative(x: i64)
requires: x >= -100 && x <= 100;
ensures: result == x * x && result >= 0;
body: x * x;

atom square_of_successor(x: i64)
requires: x >= 0 && x <= 100;
ensures: result >= 0;
body: (x + 1) * (x + 1);

atom product_of_nonnegatives(a: i64, b: i64)
requires: a >= 0 && a <= 100 && b >= 0 && b <= 100;
ensures: result == a * b && result >= 0;
body: a * b;

atom cube_on_small_nonneg(x: i64)
requires: x >= 0 && x <= 20;
ensures: result == x * x * x && result >= 0;
body: x * x * x;
