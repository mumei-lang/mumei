// Generic enum ctor resolution — a sole registered instance.
// `-> Wrap<i64>` registers the `Wrap<i64>` instance; with no second
// instance in scope, `Wrap::W` resolves through it.

enum Wrap<T> {
    W(v: T),
    Empty
}

atom mk() -> Wrap<i64>
    requires: true;
    ensures: result.tag == 0;
    body: {
        Wrap::W(7)
    }

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

// `let e = Wrap::W(3)` must record e as Wrap<i64>, not the bare generic
// name — otherwise `e.tag` is an unconstrained int and this fails.
atom pick_tag() -> i64
    requires: true;
    ensures: result == 0;
    body: {
        let e = Wrap::W(3);
        e.tag
    }
