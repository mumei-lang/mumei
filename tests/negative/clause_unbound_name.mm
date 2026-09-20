// =============================================================
// Negative: clause-scope phantom variables
// =============================================================
// requires/ensures/forall clauses must not reference undeclared names.
// A phantom name in `requires` makes the precondition trivially
// satisfiable (vacuous verify); in `ensures` it produces a spurious
// postcondition failure — both now fail closed at lowering.

atom bad_requires(n: i64)
requires: n >= 0 && arr[0] >= 0;
ensures: result >= 0;
body: n;

atom bad_ensures(n: i64)
requires: n >= 0;
ensures: result == missing_var;
body: n;

atom bad_forall(n: i64)
requires: forall(i, 0, n, arr[i] >= 0);
ensures: result >= 0;
body: n;
