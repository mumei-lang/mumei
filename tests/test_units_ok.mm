// Units of measure: matching units are accepted.
// Units are type-level tags only; Z3 still sees plain Int/Real.

type Usd = i64 unit USD;
type Jpy = i64 unit JPY;
type Meter = f64 unit Meter;
type Second = f64 unit Second;
type NonNegUsd = i64 unit USD where v >= 0;

atom add_usd(a: Usd, b: Usd) -> Usd
    requires: true;
    ensures: result == a + b;
    body: a + b;

atom sub_usd(a: NonNegUsd, b: NonNegUsd) -> Usd
    requires: a >= b;
    ensures: result >= 0;
    body: a - b;

atom cheaper(a: Usd, b: Usd) -> bool
    requires: true;
    ensures: result == (a < b);
    body: a < b;

atom scale_usd(a: Usd, k: i64) -> Usd
    requires: true;
    ensures: result == a * k;
    body: a * k;

atom add_meters(a: Meter, b: Meter) -> Meter
    requires: true;
    ensures: result == a + b;
    body: a + b;

atom total(a: Usd, b: Usd) -> Usd
    requires: true;
    ensures: result == add_usd(a, b);
    body: {
        let s = add_usd(a, b);
        s
    }

atom plain(a: i64, b: i64)
    requires: true;
    ensures: result == a + b;
    body: a + b;
