type Usd = i64 unit USD;
type Jpy = i64 unit JPY;

struct Price { usd: Usd, jpy: Jpy }
struct Quote { price: Price, qty: i64 }

atom mk_price(u: Usd, j: Jpy) -> Price
    requires: true;
    ensures: result.usd == u && result.jpy == j;
    body: Price { usd: u, jpy: j };

atom usd_via_let(u: Usd, j: Jpy) -> Usd
    requires: true;
    ensures: result == u + u;
    body: {
        let p = Price { usd: u, jpy: j };
        p.usd + u
    };

atom usd_via_call(u: Usd, j: Jpy) -> Usd
    requires: true;
    ensures: true;
    body: mk_price(u, j).usd + u;

atom usd_nested(q: Quote, u: Usd) -> Usd
    requires: true;
    ensures: result == q.price.usd + u;
    body: q.price.usd + u;
