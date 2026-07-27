// Counterexample case: the contract claims the sum of two i64 values never
// decreases, but the precondition does not bound the operands, so the sum can
// overflow. Verification must produce a counterexample.
// expected: FAIL

atom unchecked_add_is_monotone(a: i64, b: i64)
requires: true;
ensures: result >= a && result >= b;
body: a + b;
