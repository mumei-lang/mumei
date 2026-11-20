// Negative: a generic-qualifier ctor with no declared type is ambiguous
// once several instances of the enum are registered — it must fail
// closed with an ambiguity error, not pick a `Wrap<...>` at HashMap
// order.

enum Wrap<T> {
    W(v: T),
    Empty
}

atom gen(e: Wrap<i64>) -> i64
    requires: true;
    ensures: result == 0 || result == 1;
    body: {
        match e {
            Wrap::W(v) => 1,
            _ => 0
        }
    }

atom genb(e: Wrap<bool>) -> i64
    requires: true;
    ensures: result == 0 || result == 1;
    body: {
        match e {
            Wrap::W(v) => 1,
            _ => 0
        }
    }

// `Wrap::W(3)` could mean `Wrap<i64>` or `Wrap<bool>` — rejected.
atom pick() -> i64
    requires: true;
    ensures: result == 3;
    body: {
        let e = Wrap::W(3);
        match e {
            Wrap::W(v) => v,
            _ => 0
        }
    }
