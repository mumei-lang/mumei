// P10-D / C-2 fixture: nonlinear atoms that stay outside the nlsat-first window
// and therefore keep the Lean escalation path (`escalation_candidate` under
// --escalate-lean). Each atom violates exactly one conservative criterion.
// degree 3 > NLSAT_FIRST_MAX_DEGREE
atom cube_small(x: i64) -> i64
requires: x >= 0 && x <= 20;
ensures: result >= 0;
body: x * x * x;

// unbounded variables (one-sided bounds only)
atom half_bounded_product(a: i64, b: i64) -> i64
requires: a >= 0 && b >= 0;
ensures: result >= 0;
body: a * b;

// 4 variables > NLSAT_FIRST_MAX_VARIABLES
atom four_variable_sum_of_products(a: i64, b: i64, c: i64, d: i64) -> i64
requires: a >= 0 && a <= 10 && b >= 0 && b <= 10 && c >= 0 && c <= 10 && d >= 0 && d <= 10;
ensures: result >= 0;
body: a * b + c * d;
