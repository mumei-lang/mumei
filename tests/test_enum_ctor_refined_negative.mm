// Regression: constructing an enum with a payload that violates the
// declared field type must be rejected — a `Chan` carrying `p = 0` is not
// a valid `Port`, and an unchecked constructor would let a later match
// assume the refined bounds on a violating value.
type Port = i64 where v >= 1 && v <= 65535;

enum Chan {
    Tcp(p: Port),
    Idle
}

atom make_bad() -> Chan
    requires: true;
    ensures: true;
    body: {
        Chan::Tcp(0)
    }
