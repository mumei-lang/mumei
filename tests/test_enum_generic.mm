// Generic enum declarations on the Datatype path.
// `enum Wrap<T>` is monomorphized into concrete `Wrap<i64>`/`Wrap<bool>`
// instances; a pattern or constructor qualified by the generic name
// (`Wrap::W`) resolves to the instance the scrutinee's declared type (or
// the sole registered instance) pins.

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

// The payload binds through the instance's selector.
atom genp(e: Wrap<i64>) -> i64
    requires: e.tag == 0;
    ensures: result == 0;
    body: {
        match e {
            Wrap::W(v) => v - v,
            _ => 1
        }
    }

// A second instance shares the `Wrap::W` qualifier: the declared
// parameter type still disambiguates each match.
atom genb(e: Wrap<bool>) -> i64
    requires: true;
    ensures: result == 0 || result == 1;
    body: {
        match e {
            Wrap::W(v) => 1,
            _ => 0
        }
    }

atom both(e: Wrap<i64>, b: Wrap<bool>) -> i64
    requires: true;
    ensures: result == 0 || result == 1 || result == 2;
    body: {
        match e {
            Wrap::W(v) => {
                match b {
                    Wrap::W(x) => 2,
                    _ => 1
                }
            },
            _ => 0
        }
    }
