// Positive side of the constructor-refinement contract: a payload that
// satisfies its declared field type constructs fine — including inside a
// match arm whose condition pins the value (`x = 1` here, inside Port's
// 1..=65535).
type Port = i64 where v >= 1 && v <= 65535;

enum Chan {
    Tcp(p: Port),
    Idle
}

atom make_ok() -> Chan
    requires: true;
    ensures: true;
    body: {
        Chan::Tcp(5)
    }

atom sel(x: i64) -> Chan
    requires: x >= 0 && x <= 2;
    ensures: true;
    body: {
        match x {
            1 => Chan::Tcp(x),
            _ => Chan::Idle
        }
    }
