type Usd = i64 unit USD;
type Jpy = i64 unit JPY;

atom to_jpy(a: Usd) -> Jpy
    requires: true;
    ensures: true;
    body: a;
