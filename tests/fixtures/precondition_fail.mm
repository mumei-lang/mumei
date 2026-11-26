// Fixture: precondition (requires) violation.
// safe_divide requires b != 0, but the caller passes b = 0.

atom safe_divide(a: i64, b: i64) -> i64
    requires: b != 0;
    ensures: result == a / b;
    body: a / b;

atom main() -> i64
    body: safe_divide(10, 0);
