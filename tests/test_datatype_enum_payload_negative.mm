// P10-C negative: a missing payload variant must fail exhaustiveness with the
// constructor's payload types shown in the counterexample.

enum Shape {
    Point,
    Circle(i64),
}

atom area(s: Shape) -> i64
    requires: true;
    ensures: result >= 0;
    body: {
        match s {
            Point => 0
        }
    }
}
