type Usd = i64 unit USD;
type Jpy = i64 unit JPY;

atom add_mixed(a: Usd, b: Jpy)
    requires: true;
    ensures: result == a + b;
    body: a + b;
