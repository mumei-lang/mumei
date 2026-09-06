type Usd = i64 unit USD;
type Jpy = i64 unit JPY;
type Money = Usd;
type Cash = Money;

atom bad_add(c: Cash, j: Jpy) -> i64
    requires: true;
    ensures: true;
    body: c + j;
