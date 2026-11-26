// Regression: a violating constructor inside a *reachable* match arm must
// still be rejected. Arm bodies are evaluated under the negations of the
// preceding arms plus the arm's own condition — if the window mistakenly
// includes later arms' negations (the trailing `_` arm contributes
// `not true`), every earlier arm goes vacuous and this construction slips
// through unchecked.
type Port = i64 where v >= 1 && v <= 65535;

enum Chan {
    Tcp(p: Port),
    Idle
}

atom sel_bad(x: i64) -> Chan
    requires: x >= 0 && x <= 2;
    ensures: true;
    body: {
        match x {
            1 => Chan::Tcp(0),
            _ => Chan::Idle
        }
    }
