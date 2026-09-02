type Usd = i64 unit USD;
type Jpy = i64 unit JPY;

atom add_usd(a: Usd, b: Usd) -> Usd
    requires: true;
    ensures: result == a + b;
    body: a + b;

atom bad_caller(a: Usd, b: Jpy)
    requires: true;
    ensures: true;
    body: add_usd(a, b);
