// P10-C negative: finite-ADT match missing an arm must fail exhaustiveness
// with the uncovered constructor named in the counterexample.

enum Color {
    Red,
    Green,
    Blue,
}

atom code_of(c: Color) -> i64
    requires: true;
    ensures: result >= 0;
    body: {
        match c {
            Red => 0,
            Green => 1
        }
    }
}
