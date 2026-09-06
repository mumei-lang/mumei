type Usd = i64 unit USD;
type Jpy = i64 unit JPY;

struct Price { usd: Usd, jpy: Jpy }
struct Quote { price: Price, qty: i64 }

atom bad_nested(q: Quote, u: Usd) -> Usd
    requires: true;
    ensures: true;
    body: q.price.jpy + u;
