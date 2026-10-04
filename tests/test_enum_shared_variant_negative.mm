// Negative: two enums sharing variant names. The uncovered-arm counterexample
// must name the target's own enum (A), not the same-named variant's other
// owner (B) even when arm patterns are bare.

enum B {
    Y,
    Z,
}

enum A {
    X,
    Y,
    Z,
}

atom code_of_a(a: A) -> i64
    requires: true;
    ensures: result >= 0;
    body: {
        match a {
            Y => 1,
            Z => 2
        }
    }
