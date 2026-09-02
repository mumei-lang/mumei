type Usd = i64 unit USD;
type Jpy = i64 unit JPY;

atom wrong_inc(x: i64) -> i64
    requires: true;
    ensures: result == x + 1;
    body: x + 2;

atom add_mixed(a: Usd, b: Jpy)
    requires: true;
    ensures: result == a + b;
    body: a + b;
