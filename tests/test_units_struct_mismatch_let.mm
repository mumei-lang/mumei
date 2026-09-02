type Usd = i64 unit USD;
type Jpy = i64 unit JPY;

struct Price { usd: Usd, jpy: Jpy }

atom bad_let(u: Usd, j: Jpy) -> Usd
    requires: true;
    ensures: true;
    body: {
        let p = Price { usd: u, jpy: j };
        p.jpy + u
    };
