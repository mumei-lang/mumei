// =============================================================
// Test: Pure atom (no effects) — should pass verification
// =============================================================
// Atoms without effects: annotation are treated as Pure.
// They must not use perform expressions.

atom pure_add(a: i64, b: i64)
requires: true;
ensures: result == a + b;
body: a + b;

atom pure_identity(x: i64)
requires: x >= 0;
ensures: result == x;
body: x;
