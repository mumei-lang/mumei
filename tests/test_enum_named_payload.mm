// Regression: a named payload field's declared type must drive the
// payload's sort and refinement — `On(b: bool)` binds `b` as Bool so
// `if b` and `if b => ...` guards type-check, and `On(p: Port)` assumes
// the alias's conjuncts under the arm's hit condition.
type Port = i64 where v >= 1 && v <= 65535;

enum Sig {
    On(b: bool),
    Off
}

atom read_if(s: Sig) -> i64
    requires: true;
    ensures: result == 0 || result == 1;
    body: {
        match s {
            Sig::On(b) => if b { 1 } else { 0 },
            _ => 0
        }
    }

atom read_guard(s: Sig) -> i64
    requires: true;
    ensures: result == 0 || result == 1;
    body: {
        match s {
            Sig::On(b) if b => 1,
            _ => 0
        }
    }

enum Chan {
    Tcp(p: Port),
    Idle
}

atom read_port(s: Chan) -> i64
    requires: true;
    ensures: result >= 0;
    body: {
        match s {
            Chan::Tcp(p) => p,
            _ => 0
        }
    }

atom make_ok() -> Chan
    requires: true;
    ensures: true;
    body: {
        Chan::Tcp(5)
    }
