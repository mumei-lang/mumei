// Counterexample case: naive absolute value claims a strictly positive result,
// but x == 0 (and the i64 minimum) refutes the postcondition.
// expected: FAIL

atom abs_is_strictly_positive(x: i64)
requires: true;
ensures: result > 0;
body: { if x >= 0 { x } else { -x } };
