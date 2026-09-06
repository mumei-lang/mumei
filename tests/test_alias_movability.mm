type Usd = i64 unit USD;
type Nat = i64 where v >= 0;
type Count = Nat;

atom clamp_usd(a: Usd, floor: Usd)
    requires: true;
    ensures: result >= floor;
    body: if a > floor { a } else { floor };

atom max_count(a: Count, b: Count)
    requires: true;
    ensures: result >= a && result >= b;
    body: if a > b { a } else { b };
