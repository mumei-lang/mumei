// =============================================================
// Test: Semantic feedback — verification should produce feedback
// =============================================================
// An atom with a precondition that constrains a range.
// Used to verify semantic feedback generation for constraint violations.

type BoundedAge = i64 where v >= 0 && v <= 120;

atom validate_age(age: BoundedAge) -> i64
    requires: age >= 0 && age <= 120;
    ensures: result == age;
    body: age;

// This atom should pass verification — precondition matches body
atom safe_division(a: i64, b: i64) -> i64
    requires: b != 0;
    ensures: true;
    body: a / b;
