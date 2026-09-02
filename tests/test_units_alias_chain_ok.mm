type Usd = i64 unit USD;
type Money = Usd;
type Cash = Money;

atom total(c: Cash, m: Money, u: Usd) -> Money
    requires: true;
    ensures: result == c + m + u;
    body: c + m + u;

atom pick(c: bool, m: Money, u: Usd) -> Usd
    requires: true;
    ensures: true;
    body: if c { m } else { u };
