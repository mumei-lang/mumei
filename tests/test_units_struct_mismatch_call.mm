type Usd = i64 unit USD;
type Jpy = i64 unit JPY;

struct Price { usd: Usd, jpy: Jpy }

atom mk_price(u: Usd, j: Jpy) -> Price
    requires: true;
    ensures: result.usd == u && result.jpy == j;
    body: Price { usd: u, jpy: j };

atom bad_call(u: Usd, j: Jpy) -> Usd
    requires: true;
    ensures: true;
    body: mk_price(u, j).jpy + u;
